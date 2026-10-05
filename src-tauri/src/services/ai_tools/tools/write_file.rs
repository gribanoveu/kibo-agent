//! `writeFile` — create a file, or replace one whole.
//!
//! Almost nothing of Alfa Atlas's version survives: it wrote only inside the
//! documentation subtree, only recognised document extensions, and rewrote
//! AsciiDoc macro brackets on the way to disk. What is left is the shape —
//! read the old content, write, return the diff — plus the guard that Atlas
//! did not have.

use crate::domain::llm::LlmToolDefinition;
use std::fs;

use crate::domain::tools::{
    ReadFiles, ToolError, ToolResult, ToolScope, WriteBlocked, WriteFileArgs,
};
use crate::services::text_diff;

use super::super::resolve::{relative_to_root, resolve_writable};

pub fn write_file(
    scope: &ToolScope,
    args: &WriteFileArgs,
    reads: &mut ReadFiles,
) -> Result<ToolResult, ToolError> {
    let path = resolve_writable(scope, &args.path)?;
    if path.is_dir() {
        return Err(ToolError::NotAFile(args.path.clone()));
    }
    let relative = relative_to_root(scope, &path)?;

    // A file that does not exist yet has nothing to have read and nothing to
    // lose, so the guard applies only to a replacement.
    let old = match fs::read_to_string(&path) {
        Ok(existing) => {
            reads
                .check(&relative, &existing, true)
                .map_err(|blocked| match blocked {
                    WriteBlocked::NeverRead => ToolError::FileNotRead(relative.clone()),
                    WriteBlocked::ReadInPart => ToolError::FileReadInPart(relative.clone()),
                    WriteBlocked::ChangedSinceRead => {
                        ToolError::FileChangedSinceRead(relative.clone())
                    }
                })?;
            existing
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(ToolError::Io(e)),
    };

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(ToolError::Io)?;
    }
    fs::write(&path, &args.content).map_err(ToolError::Io)?;

    // The agent now knows exactly what is there, so a second write to the same
    // path must not come back as "changed since you read it".
    reads.record_write(&relative, &args.content, true);

    Ok(ToolResult::FileWritten {
        path: relative,
        diff: text_diff::diff_stats(&old, &args.content),
    })
}

/// What the model is told `writeFile` is for.
pub(super) fn definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "writeFile".to_string(),
        description: "Create a file, or replace an existing one whole. To change part of a file, use editFile: rewriting a whole file to alter a few lines costs context and risks losing everything you did not repeat. Refused if the file exists and this turn has not read it whole, or if it changed on disk since that read — read it again and reconcile rather than overwriting someone's work. Returns the line diff of what actually landed."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path relative to the workspace root. Missing parent directories are created."
                },
                "content": {
                    "type": "string",
                    "description": "The file's complete new content. Everything not included here is gone."
                }
            },
            "required": [
                "path",
                "content"
            ]
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::tools::{ReadFileArgs, ToolCall, ToolDeps};
    use crate::services::ai_tools::tools::{execute_tool, read_file::read_file};
    use crate::testing::temp_dir;
    use std::path::{Path, PathBuf};

    fn fixture(label: &str) -> (ToolScope, PathBuf, ReadFiles) {
        let dir = temp_dir(label);
        let scope = ToolScope::new(&dir).expect("root resolves");
        let root = scope.root().to_path_buf();
        (scope, root, ReadFiles::default())
    }

    fn write_args(path: &str, content: &str) -> WriteFileArgs {
        WriteFileArgs {
            path: path.to_string(),
            content: content.to_string(),
        }
    }

    /// Reads through the real tool, so the registry is populated exactly the
    /// way a model's own call would populate it.
    fn agent_reads(scope: &ToolScope, reads: &mut ReadFiles, path: &str, range: Option<(u32, u32)>) {
        let args = ReadFileArgs {
            path: path.to_string(),
            paths: None,
            start_line: range.map(|(s, _)| s),
            end_line: range.map(|(_, e)| e),
            outline: None,
        };
        read_file(scope, &args, reads).expect("read succeeds");
    }

    fn on_disk(root: &Path, relative: &str) -> String {
        std::fs::read_to_string(root.join(relative)).expect("file is readable")
    }

    #[test]
    fn a_new_file_needs_no_prior_read() {
        let (scope, root, mut reads) = fixture("write-new");

        let result = write_file(&scope, &write_args("notes.md", "hello\n"), &mut reads)
            .expect("a file that does not exist has nothing to lose");

        assert_eq!(on_disk(&root, "notes.md"), "hello\n");
        let ToolResult::FileWritten { path, diff } = result else {
            panic!("wrong result")
        };
        assert_eq!(path, "notes.md");
        assert_eq!((diff.lines_added, diff.lines_removed), (1, 0));
    }

    #[test]
    fn missing_parent_directories_are_created() {
        let (scope, root, mut reads) = fixture("write-parents");

        write_file(&scope, &write_args("a/b/c/deep.txt", "x\n"), &mut reads).expect("writes");

        assert_eq!(on_disk(&root, "a/b/c/deep.txt"), "x\n");
    }

    /// The guard that Alfa Atlas did not have. Replacing a file sight unseen
    /// destroys work the model cannot even report losing.
    #[test]
    fn replacing_an_unread_file_is_refused() {
        let (scope, root, mut reads) = fixture("write-unread");
        std::fs::write(root.join("existing.txt"), "precious\n").expect("writable");

        let err = write_file(&scope, &write_args("existing.txt", "clobbered\n"), &mut reads)
            .expect_err("never read");

        assert!(matches!(err, ToolError::FileNotRead(_)));
        assert_eq!(on_disk(&root, "existing.txt"), "precious\n", "left untouched");
        assert!(err.to_string().contains("read"), "the message says what to do");
    }

    #[test]
    fn replacing_a_file_read_in_full_is_allowed() {
        let (scope, root, mut reads) = fixture("write-read-first");
        std::fs::write(root.join("a.txt"), "one\ntwo\n").expect("writable");
        agent_reads(&scope, &mut reads, "a.txt", None);

        write_file(&scope, &write_args("a.txt", "one\nTWO\n"), &mut reads).expect("writes");

        assert_eq!(on_disk(&root, "a.txt"), "one\nTWO\n");
    }

    /// Reading 50 lines of 500 and then replacing the file destroys 450 nobody
    /// looked at. An anchored `editFile` is the way to change what you did see.
    #[test]
    fn replacing_a_file_read_only_in_part_is_refused() {
        let (scope, root, mut reads) = fixture("write-partial");
        let body: String = (1..=100).map(|i| format!("line {i}\n")).collect();
        std::fs::write(root.join("big.txt"), &body).expect("writable");
        agent_reads(&scope, &mut reads, "big.txt", Some((1, 10)));

        let err = write_file(&scope, &write_args("big.txt", "short\n"), &mut reads)
            .expect_err("only part was seen");

        assert!(matches!(err, ToolError::FileReadInPart(_)));
        assert_eq!(on_disk(&root, "big.txt"), body, "left untouched");
        assert!(err.to_string().contains("editFile"), "names the alternative");
    }

    /// The classic loss: the agent read, a person edited in their own editor,
    /// the agent wrote its stale copy over the top.
    #[test]
    fn writing_over_someone_elses_change_is_refused() {
        let (scope, root, mut reads) = fixture("write-stale");
        std::fs::write(root.join("a.txt"), "original\n").expect("writable");
        agent_reads(&scope, &mut reads, "a.txt", None);

        std::fs::write(root.join("a.txt"), "edited by a human\n").expect("writable");

        let err = write_file(&scope, &write_args("a.txt", "agent version\n"), &mut reads)
            .expect_err("changed under it");

        assert!(matches!(err, ToolError::FileChangedSinceRead(_)));
        assert_eq!(on_disk(&root, "a.txt"), "edited by a human\n", "left untouched");
    }

    /// After writing, the agent knows precisely what is there. Refusing its
    /// next write as stale would make consecutive writes impossible.
    #[test]
    fn a_second_write_to_the_same_path_is_allowed() {
        let (scope, root, mut reads) = fixture("write-twice");

        write_file(&scope, &write_args("a.txt", "first\n"), &mut reads).expect("creates");
        write_file(&scope, &write_args("a.txt", "second\n"), &mut reads).expect("rewrites");

        assert_eq!(on_disk(&root, "a.txt"), "second\n");
    }

    /// The registry is keyed by the canonical spelling, not by whatever the
    /// model typed — otherwise `./a.txt` and `a.txt` would be two files.
    #[test]
    fn the_registry_ignores_how_the_path_was_spelled() {
        let (scope, root, mut reads) = fixture("write-spelling");
        std::fs::write(root.join("a.txt"), "body\n").expect("writable");
        agent_reads(&scope, &mut reads, "./a.txt", None);

        write_file(&scope, &write_args("a.txt", "new\n"), &mut reads)
            .expect("same file, different spelling");
    }

    #[test]
    fn the_diff_reports_what_changed() {
        let (scope, root, mut reads) = fixture("write-diff");
        std::fs::write(root.join("a.txt"), "one\ntwo\nthree\n").expect("writable");
        agent_reads(&scope, &mut reads, "a.txt", None);

        let ToolResult::FileWritten { diff, .. } =
            write_file(&scope, &write_args("a.txt", "one\nTWO\nthree\nfour\n"), &mut reads).unwrap()
        else {
            panic!("wrong result")
        };

        assert_eq!((diff.lines_added, diff.lines_removed), (2, 1));
        assert!(diff.unified_diff.contains("+TWO"), "{}", diff.unified_diff);
    }

    #[test]
    fn a_path_leaving_the_root_is_refused() {
        let (scope, _root, mut reads) = fixture("write-escape");
        assert!(matches!(
            write_file(&scope, &write_args("../outside.txt", "x"), &mut reads),
            Err(ToolError::PathEscape(_))
        ));
    }

    #[test]
    fn writing_over_a_directory_is_refused() {
        let (scope, root, mut reads) = fixture("write-dir");
        std::fs::create_dir(root.join("sub")).expect("creatable");
        assert!(matches!(
            write_file(&scope, &write_args("sub", "x"), &mut reads),
            Err(ToolError::NotAFile(_))
        ));
    }

    #[test]
    fn the_call_wire_shape_is_stable_and_counts_as_risky() {
        let json = r#"{"tool": "writeFile", "args": {"path": "a.txt", "content": "x\n"}}"#;
        let call: ToolCall = serde_json::from_str(json).expect("parses");
        assert_eq!(call, ToolCall::WriteFile(write_args("a.txt", "x\n")));
        assert!(call.is_risky(), "a write needs a human");

        let (scope, root, mut reads) = fixture("write-dispatch");
        assert!(matches!(
            execute_tool(&scope, &call, &mut reads, &mut Vec::new(), &ToolDeps::default()),
            Ok(ToolResult::FileWritten { .. })
        ));
        assert_eq!(on_disk(&root, "a.txt"), "x\n");
    }

    /// An edit teaches the agent nothing about the parts of the file it never
    /// read, so "has seen the whole thing" must not be granted by writing.
    #[test]
    fn a_partial_write_does_not_upgrade_a_partial_read() {
        let mut reads = ReadFiles::default();
        reads.record("a.txt", "whole original", false);
        reads.record_write("a.txt", "whole edited", false);

        assert_eq!(
            reads.check("a.txt", "whole edited", true),
            Err(crate::domain::tools::WriteBlocked::ReadInPart)
        );
        assert_eq!(reads.check("a.txt", "whole edited", false), Ok(()));
    }
}
