//! What a call would do, worked out without doing it.
//!
//! Only for a round that has paused: approving a write means approving its
//! contents, and `{"path":"Mapper.java"}` is not an answer to "what would
//! change". Everything here reads; nothing here writes.
//!
//! A preview can also fail, and that is worth as much as a successful one: an
//! edit whose anchor no longer matches is better refused before the user has
//! agreed to it than after.

use std::fs;

use crate::domain::llm::LlmToolCall;
use crate::domain::tools::{ToolCall, ToolPreview, ToolScope};
use crate::infra::workspace_scanner;
use crate::services::text_diff::diff_stats;

use super::parse::parse_tool_call;
use super::resolve::{resolve_existing, resolve_writable};
use super::tools::edit_file::apply_edits;

pub fn preview_tool_call(scope: &ToolScope, call: &LlmToolCall) -> ToolPreview {
    let parsed = match parse_tool_call(call) {
        Ok(parsed) => parsed,
        Err(e) => return ToolPreview::Failed { reason: e.to_string() },
    };

    match parsed {
        ToolCall::WriteFile(args) => match resolve_writable(scope, &args.path) {
            // A file that is not there yet diffs against nothing, which is
            // exactly right: every line shows as an addition.
            Ok(path) => diff_preview(
                args.path,
                &fs::read_to_string(&path).unwrap_or_default(),
                &args.content,
            ),
            Err(e) => ToolPreview::Failed { reason: e.to_string() },
        },

        ToolCall::EditFile(args) => match resolve_existing(scope, &args.path) {
            Ok(path) => match fs::read_to_string(&path) {
                Ok(current) => match apply_edits(&current, &args.edits) {
                    Ok(edited) => diff_preview(args.path, &current, &edited),
                    // The anchor does not match, or two edits overlap. Saying
                    // so now costs one glance; saying so after approval costs
                    // a round.
                    Err(e) => ToolPreview::Failed { reason: e.to_string() },
                },
                Err(e) => ToolPreview::Failed { reason: e.to_string() },
            },
            Err(e) => ToolPreview::Failed { reason: e.to_string() },
        },

        ToolCall::Delete(args) => match resolve_existing(scope, &args.path) {
            Ok(path) if !path.is_dir() => diff_preview(args.path, &fs::read_to_string(&path).unwrap_or_default(), ""),
            Ok(path) => ToolPreview::Removes {
                path: args.path,
                // Counted the same way the tools walk: ignored files are not
                // shown anywhere else either, and counting them here would
                // make `node_modules` the headline of every delete.
                files: workspace_scanner::scan_files(&path, None).map(|f| f.len()).unwrap_or(0),
            },
            Err(e) => ToolPreview::Failed { reason: e.to_string() },
        },

        ToolCall::RunCommand(request) => {
            let cwd = request.cwd.clone().unwrap_or_else(|| ".".to_string());
            match resolve_existing(scope, &cwd) {
                Ok(_) => ToolPreview::Command {
                    command: request.command,
                    cwd,
                },
                Err(e) => ToolPreview::Failed { reason: e.to_string() },
            }
        }

        // Where that terminal is now is the user's doing, not a path of ours.

        // A rename, a new directory, a read: the arguments already say it.
        _ => ToolPreview::Nothing,
    }
}

fn diff_preview(path: String, before: &str, after: &str) -> ToolPreview {
    ToolPreview::Diff {
        path,
        diff: diff_stats(before, after),
    }
}

/// Exposed for the command layer, which has a whole paused round to describe.
pub fn preview_round(scope: &ToolScope, calls: &[LlmToolCall]) -> Vec<ToolPreview> {
    calls.iter().map(|call| preview_tool_call(scope, call)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::tools::FileDiffStats;
    use crate::testing::temp_dir;
    use std::path::Path;

    fn call(name: &str, arguments: &str) -> LlmToolCall {
        LlmToolCall {
            id: "c1".to_string(),
            name: name.to_string(),
            arguments: arguments.to_string(),
        }
    }

    fn preview(root: &Path, name: &str, arguments: &str) -> ToolPreview {
        let scope = ToolScope::new(root).expect("scope");
        preview_tool_call(&scope, &call(name, arguments))
    }

    fn diff(preview: ToolPreview) -> FileDiffStats {
        match preview {
            ToolPreview::Diff { diff, .. } => diff,
            other => panic!("expected a diff, got {other:?}"),
        }
    }

    #[test]
    fn a_write_shows_what_would_change_and_changes_nothing() {
        let root = temp_dir("preview-write");
        std::fs::write(root.join("a.rs"), "fn one() {}\n").unwrap();

        let shown = diff(preview(
            &root,
            "writeFile",
            r#"{"path":"a.rs","content":"fn two() {}\n"}"#,
        ));

        assert_eq!((shown.lines_added, shown.lines_removed), (1, 1));
        assert!(shown.unified_diff.contains("+fn two"), "{}", shown.unified_diff);
        assert_eq!(
            std::fs::read_to_string(root.join("a.rs")).unwrap(),
            "fn one() {}\n",
            "the file on disk is untouched"
        );
    }

    /// A file that does not exist yet diffs against nothing, so every line
    /// reads as an addition rather than as an error.
    #[test]
    fn a_new_file_is_all_additions() {
        let root = temp_dir("preview-new");
        let shown = diff(preview(
            &root,
            "writeFile",
            r#"{"path":"new.rs","content":"one\ntwo\n"}"#,
        ));

        assert_eq!((shown.lines_added, shown.lines_removed), (2, 0));
    }

    #[test]
    fn an_edit_is_previewed_through_its_anchors() {
        let root = temp_dir("preview-edit");
        std::fs::write(root.join("a.rs"), "let x = 41;\n").unwrap();

        let shown = diff(preview(
            &root,
            "editFile",
            r#"{"path":"a.rs","edits":[{"old":"41","new":"42"}]}"#,
        ));

        assert!(shown.unified_diff.contains("+let x = 42;"), "{}", shown.unified_diff);
    }

    /// Learning that an edit cannot apply *after* approving it costs a round
    /// and the user's trust in the card.
    #[test]
    fn an_edit_that_cannot_apply_says_so_before_it_is_approved() {
        let root = temp_dir("preview-edit-bad");
        std::fs::write(root.join("a.rs"), "let x = 41;\n").unwrap();

        let shown = preview(
            &root,
            "editFile",
            r#"{"path":"a.rs","edits":[{"old":"nowhere","new":"x"}]}"#,
        );

        let ToolPreview::Failed { reason } = shown else {
            panic!("expected a failure, got {shown:?}");
        };
        assert!(reason.contains("not found"), "{reason}");
    }

    #[test]
    fn a_delete_shows_the_whole_file_as_going_away() {
        let root = temp_dir("preview-delete");
        std::fs::write(root.join("a.rs"), "one\ntwo\nthree\n").unwrap();

        let shown = diff(preview(&root, "delete", r#"{"path":"a.rs"}"#));

        assert_eq!((shown.lines_added, shown.lines_removed), (0, 3));
    }

    /// The scariest thing to approve is a path that turned out broader than it
    /// looked.
    #[test]
    fn a_recursive_delete_counts_what_it_would_take() {
        let root = temp_dir("preview-rmdir");
        let dir = root.join("src");
        std::fs::create_dir(&dir).unwrap();
        for i in 0..3 {
            std::fs::write(dir.join(format!("f{i}.rs")), "x").unwrap();
        }

        let shown = preview(&root, "delete", r#"{"path":"src","recursive":true}"#);

        assert_eq!(
            shown,
            ToolPreview::Removes {
                path: "src".to_string(),
                files: 3
            }
        );
    }

    #[test]
    fn a_command_shows_where_it_would_run() {
        let root = temp_dir("preview-command");
        std::fs::create_dir(root.join("crate")).unwrap();

        let shown = preview(
            &root,
            "runCommand",
            r#"{"command":"cargo test","cwd":"crate"}"#,
        );

        assert_eq!(
            shown,
            ToolPreview::Command {
                command: "cargo test".to_string(),
                cwd: "crate".to_string()
            }
        );
    }

    #[test]
    fn a_path_outside_the_workspace_is_refused_here_too() {
        let root = temp_dir("preview-escape");
        let shown = preview(&root, "writeFile", r#"{"path":"../outside.rs","content":"x"}"#);
        assert!(matches!(shown, ToolPreview::Failed { .. }), "{shown:?}");
    }

    #[test]
    fn a_call_with_nothing_to_show_says_so() {
        let root = temp_dir("preview-nothing");
        assert_eq!(
            preview(&root, "move", r#"{"path":"a.rs","newPath":"b.rs"}"#),
            ToolPreview::Nothing
        );
    }

    /// A round pauses as a whole, and every call in it is described — the ones
    /// that need no decision included, since they run on resume too.
    #[test]
    fn a_whole_round_is_described_in_order() {
        let root = temp_dir("preview-round");
        std::fs::write(root.join("a.rs"), "x\n").unwrap();
        let scope = ToolScope::new(&root).expect("scope");

        let shown = preview_round(
            &scope,
            &[
                call("readFile", r#"{"path":"a.rs"}"#),
                call("writeFile", r#"{"path":"a.rs","content":"y\n"}"#),
            ],
        );

        assert_eq!(shown.len(), 2);
        assert_eq!(shown[0], ToolPreview::Nothing);
        assert!(matches!(shown[1], ToolPreview::Diff { .. }));
    }
}
