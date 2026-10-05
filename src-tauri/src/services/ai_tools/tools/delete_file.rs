//! `deleteFile` — removal of one file.
//!
//! Guarded the same way a wholesale replacement is, and for the same reason:
//! deleting a file the agent never read destroys work nobody looked at. The
//! whole-file requirement applies here too — a partial read is no basis for
//! removing the parts that were never seen.

use crate::domain::llm::LlmToolDefinition;
use std::fs;

use crate::domain::tools::{
    DeleteFileArgs, ReadFiles, ToolError, ToolResult, ToolScope, WriteBlocked,
};
use crate::services::text_diff;

use super::super::resolve::{relative_to_root, resolve_existing};

pub fn delete_file(
    scope: &ToolScope,
    args: &DeleteFileArgs,
    reads: &mut ReadFiles,
) -> Result<ToolResult, ToolError> {
    let path = resolve_existing(scope, &args.path)?;
    if !path.is_file() {
        return Err(ToolError::NotAFile(args.path.clone()));
    }
    let relative = relative_to_root(scope, &path)?;
    let old = fs::read_to_string(&path).map_err(ToolError::Io)?;

    reads
        .check(&relative, &old, true)
        .map_err(|blocked| match blocked {
            WriteBlocked::NeverRead | WriteBlocked::ReadInPart => ToolError::DeleteNotReadInFull(relative.clone()),
            WriteBlocked::ChangedSinceRead => ToolError::FileChangedSinceRead(relative.clone()),
        })?;

    fs::remove_file(&path).map_err(ToolError::Io)?;

    // The registry entry is left behind on purpose. It cannot mislead: a later
    // write to this path finds nothing on disk and skips the check entirely,
    // while a path recreated by somebody else fails the hash — which is the
    // right answer, not an accident.
    Ok(ToolResult::FileDeleted {
        path: relative,
        diff: text_diff::diff_stats(&old, ""),
    })
}

/// What the model is told `deleteFile` is for.
pub(super) fn definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "deleteFile".to_string(),
        description: "Delete one file. Refused unless this turn has read the file whole — deleting what you have not seen is how work disappears unnoticed. Returns the diff of what was removed, so the content is still in the transcript if it turns out to have been needed."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path relative to the workspace root."
                }
            },
            "required": [
                "path"
            ]
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::tools::{ReadFileArgs, ReadFiles, ToolCall, ToolDeps};
    use crate::services::ai_tools::tools::{execute_tool, read_file::read_file};
    use crate::testing::temp_dir;
    use std::path::PathBuf;

    fn fixture(label: &str) -> (ToolScope, PathBuf, ReadFiles) {
        let dir = temp_dir(label);
        let scope = ToolScope::new(&dir).expect("root resolves");
        let root = scope.root().to_path_buf();
        (scope, root, ReadFiles::default())
    }

    fn write(root: &std::path::Path, relative: &str, body: &str) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().expect("has a parent")).expect("dirs are creatable");
        std::fs::write(path, body).expect("file is writable");
    }

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

    #[test]
    fn deletes_a_file_the_agent_has_read() {
        let (scope, root, mut reads) = fixture("rm-read");
        write(&root, "a.txt", "one\ntwo\n");
        agent_reads(&scope, &mut reads, "a.txt", None);

        let result = delete_file(&scope, &DeleteFileArgs { path: "a.txt".into() }, &mut reads)
            .expect("deletes");

        assert!(!root.join("a.txt").exists());
        let ToolResult::FileDeleted { path, diff } = result else {
            panic!("wrong result")
        };
        assert_eq!(path, "a.txt");
        assert_eq!((diff.lines_added, diff.lines_removed), (0, 2), "the whole file went");
    }

    /// Deleting is destroying: the same guard a wholesale replacement gets.
    #[test]
    fn deleting_an_unread_file_is_refused() {
        let (scope, root, mut reads) = fixture("rm-unread");
        write(&root, "a.txt", "precious\n");

        let err = delete_file(&scope, &DeleteFileArgs { path: "a.txt".into() }, &mut reads)
            .expect_err("never read");

        assert!(matches!(err, ToolError::DeleteNotReadInFull(_)));
        assert!(err.to_string().contains("before deleting it"), "{err}");
        assert!(root.join("a.txt").exists(), "left in place");
    }

    /// A partial read is no basis for removing the parts never seen — the same
    /// line `writeFile` draws, for the same reason.
    #[test]
    fn deleting_a_file_read_only_in_part_is_refused() {
        let (scope, root, mut reads) = fixture("rm-partial");
        let body: String = (1..=100).map(|i| format!("line {i}\n")).collect();
        write(&root, "big.txt", &body);
        agent_reads(&scope, &mut reads, "big.txt", Some((1, 10)));

        assert!(matches!(
            delete_file(&scope, &DeleteFileArgs { path: "big.txt".into() }, &mut reads),
            Err(ToolError::DeleteNotReadInFull(_))
        ));
        assert!(root.join("big.txt").exists(), "left in place");
    }

    #[test]
    fn deleting_a_file_changed_since_it_was_read_is_refused() {
        let (scope, root, mut reads) = fixture("rm-stale");
        write(&root, "a.txt", "original\n");
        agent_reads(&scope, &mut reads, "a.txt", None);
        write(&root, "a.txt", "edited by a human\n");

        assert!(matches!(
            delete_file(&scope, &DeleteFileArgs { path: "a.txt".into() }, &mut reads),
            Err(ToolError::FileChangedSinceRead(_))
        ));
        assert!(root.join("a.txt").exists(), "left in place");
    }

    #[test]
    fn deleting_a_directory_is_not_this_tools_job() {
        let (scope, root, mut reads) = fixture("rm-dir");
        std::fs::create_dir(root.join("sub")).expect("creatable");
        assert!(matches!(
            delete_file(&scope, &DeleteFileArgs { path: "sub".into() }, &mut reads),
            Err(ToolError::NotAFile(_))
        ));
    }

    #[test]
    fn a_path_leaving_the_root_is_refused() {
        let (scope, _, mut reads) = fixture("rm-escape");
        assert!(matches!(
            delete_file(&scope, &DeleteFileArgs { path: "../a.txt".into() }, &mut reads),
            Err(ToolError::PathEscape(_))
        ));
    }

    #[test]
    fn the_call_wire_shape_is_stable_and_counts_as_risky() {
        let call: ToolCall =
            serde_json::from_str(r#"{"tool": "deleteFile", "args": {"path": "a.txt"}}"#)
                .expect("parses");
        assert!(call.is_risky());

        let (scope, root, mut reads) = fixture("rm-dispatch");
        write(&root, "a.txt", "x\n");
        agent_reads(&scope, &mut reads, "a.txt", None);
        assert!(matches!(
            execute_tool(&scope, &call, &mut reads, &mut Vec::new(), &ToolDeps::default()),
            Ok(ToolResult::FileDeleted { .. })
        ));
        assert!(!root.join("a.txt").exists());
    }
}
