//! `move` — rename a file or directory inside the scope.
//!
//! Alfa Atlas's version also rewrote `include::`/`xref:`/`$ref` references to
//! the moved file across the rest of the documentation. That is not ported;
//! for code, references are the compiler's business, not a filesystem tool's.
//! It is also why `move` costs less against the turn budget here than it did
//! there.
//!
//! No read-registry check: a move destroys nothing. The destination must be
//! free, so there is nothing to overwrite, and the source keeps its content.

use crate::domain::llm::LlmToolDefinition;
use std::fs;

use crate::domain::tools::{MoveArgs, ReadFiles, ToolError, ToolResult, ToolScope};

use super::super::resolve::{relative_to_root, resolve_existing, resolve_writable};

pub fn move_path(scope: &ToolScope, args: &MoveArgs, reads: &mut ReadFiles) -> Result<ToolResult, ToolError> {
    let from = resolve_existing(scope, &args.path)?;
    let to = resolve_writable(scope, &args.new_path)?;

    // Checked before the rename, never after: `fs::rename` would replace the
    // destination without a word.
    if to.exists() {
        return Err(ToolError::AlreadyExists(args.new_path.clone()));
    }
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(ToolError::Io)?;
    }
    let from_relative = relative_to_root(scope, &from)?;
    fs::rename(&from, &to).map_err(ToolError::Io)?;
    let to_relative = relative_to_root(scope, &to)?;
    reads.moved(&from_relative, &to_relative);
    let files = to.is_dir().then(|| count_files(&to));

    Ok(ToolResult::Moved { from: from_relative, to: to_relative, files })
}

/// Files anywhere under `dir`, not following links: "moved 0 files" is how
/// the model learns it moved an empty shell rather than the tree it meant.
pub(super) fn count_files(dir: &std::path::Path) -> usize {
    files_under(dir).len()
}

/// Every file under `dir`, not following links, in path order.
pub(crate) fn files_under(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<std::path::PathBuf> = entries
        .flatten()
        .flat_map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => files_under(&entry.path()),
            Ok(_) => vec![entry.path()],
            Err(_) => Vec::new(),
        })
        .collect();
    files.sort();
    files
}

/// What the model is told `move` is for.
pub(super) fn definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "move".to_string(),
        description: "Move or rename a file or directory. Nothing else is updated: imports, includes and links that named the old path keep naming it, and fixing them is your job — grep for the old path after moving. What you read under the old path counts as read under the new one: a moved file can be written or deleted without reading it again."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Existing path, relative to the workspace root."
                },
                "newPath": {
                    "type": "string",
                    "description": "Destination path, relative to the workspace root. Refused if something is already there."
                }
            },
            "required": [
                "path",
                "newPath"
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
            start_line: range.map(|(s, _)| s),
            end_line: range.map(|(_, e)| e),
            outline: None,
        };
        read_file(scope, &args, reads).expect("read succeeds");
    }

    fn args(path: &str, new_path: &str) -> MoveArgs {
        MoveArgs { path: path.into(), new_path: new_path.into() }
    }

    #[test]
    fn renames_a_file() {
        let (scope, root, _) = fixture("mv-file");
        write(&root, "a.txt", "body\n");

        let result = move_path(&scope, &args("a.txt", "b.txt"), &mut ReadFiles::default()).expect("moves");

        assert!(!root.join("a.txt").exists());
        assert_eq!(std::fs::read_to_string(root.join("b.txt")).unwrap(), "body\n");
        let ToolResult::Moved { from, to, files } = result else {
            panic!("wrong result")
        };
        assert_eq!((from.as_str(), to.as_str(), files), ("a.txt", "b.txt", None));
    }

    #[test]
    fn renames_a_directory_with_its_contents() {
        let (scope, root, _) = fixture("mv-dir");
        write(&root, "old/deep/a.txt", "body\n");
        write(&root, "old/b.txt", "body\n");
        std::fs::create_dir_all(root.join("old/empty")).unwrap();

        let result = move_path(&scope, &args("old", "new"), &mut ReadFiles::default()).expect("moves");

        assert!(matches!(result, ToolResult::Moved { files: Some(2), .. }), "{result:?}");
        assert!(!root.join("old").exists());
        assert_eq!(
            std::fs::read_to_string(root.join("new/deep/a.txt")).unwrap(),
            "body\n"
        );
    }

    #[test]
    fn missing_destination_parents_are_created() {
        let (scope, root, _) = fixture("mv-parents");
        write(&root, "a.txt", "body\n");

        move_path(&scope, &args("a.txt", "x/y/z.txt"), &mut ReadFiles::default()).expect("moves");

        assert!(root.join("x/y/z.txt").exists());
    }

    /// `fs::rename` replaces the destination without a word. Nothing this tool
    /// does may destroy a file, which is also why it needs no read check.
    #[test]
    fn moving_onto_an_existing_path_is_refused() {
        let (scope, root, _) = fixture("mv-occupied");
        write(&root, "a.txt", "source\n");
        write(&root, "b.txt", "precious\n");

        let err = move_path(&scope, &args("a.txt", "b.txt"), &mut ReadFiles::default()).expect_err("occupied");

        assert!(matches!(err, ToolError::AlreadyExists(_)));
        assert_eq!(std::fs::read_to_string(root.join("b.txt")).unwrap(), "precious\n");
        assert!(root.join("a.txt").exists(), "source left alone too");
    }

    #[test]
    fn moving_something_that_is_not_there_is_refused() {
        let (scope, _, _) = fixture("mv-absent");
        assert!(matches!(
            move_path(&scope, &args("nope.txt", "b.txt"), &mut ReadFiles::default()),
            Err(ToolError::NotFound(_))
        ));
    }

    #[test]
    fn neither_end_may_leave_the_root() {
        let (scope, root, _) = fixture("mv-escape");
        write(&root, "a.txt", "body\n");

        assert!(matches!(
            move_path(&scope, &args("a.txt", "../b.txt"), &mut ReadFiles::default()),
            Err(ToolError::PathEscape(_))
        ));
        assert!(matches!(
            move_path(&scope, &args("../a.txt", "b.txt"), &mut ReadFiles::default()),
            Err(ToolError::PathEscape(_))
        ));
        assert!(root.join("a.txt").exists());
    }

    /// Moving destroys nothing, so no prior read is asked for — the point
    /// being that the guard is about losing content, not about ceremony.
    #[test]
    fn moving_an_unread_file_is_allowed() {
        let (scope, root, _) = fixture("mv-unread");
        write(&root, "a.txt", "never read\n");

        move_path(&scope, &args("a.txt", "b.txt"), &mut ReadFiles::default()).expect("a move loses nothing");

        assert_eq!(std::fs::read_to_string(root.join("b.txt")).unwrap(), "never read\n");
    }

    /// The content did not change, so what was read follows the file: after a
    /// move it can be changed without being read again — a file, or anything
    /// read under a moved directory.
    #[test]
    fn what_was_read_follows_the_move() {
        let (scope, root, mut reads) = fixture("mv-reads");
        write(&root, "a.txt", "body\n");
        write(&root, "dir/b.txt", "inner\n");
        agent_reads(&scope, &mut reads, "a.txt", None);
        agent_reads(&scope, &mut reads, "dir/b.txt", None);

        move_path(&scope, &args("a.txt", "moved/a.txt"), &mut reads).expect("moves");
        move_path(&scope, &args("dir", "other"), &mut reads).expect("moves");

        assert_eq!(reads.check("moved/a.txt", "body\n", true), Ok(()));
        assert_eq!(reads.check("other/b.txt", "inner\n", true), Ok(()));
        assert!(reads.check("a.txt", "body\n", true).is_err(), "the old path is no longer vouched for");
    }

    #[test]
    fn the_call_wire_shape_is_stable_and_counts_as_risky() {
        let call: ToolCall = serde_json::from_str(
            r#"{"tool": "move", "args": {"path": "a.txt", "newPath": "b.txt"}}"#,
        )
        .expect("parses");
        assert_eq!(call, ToolCall::Move(args("a.txt", "b.txt")));
        assert!(call.is_risky());

        let (scope, root, mut reads) = fixture("mv-dispatch");
        write(&root, "a.txt", "x\n");
        assert!(matches!(
            execute_tool(&scope, &call, &mut reads, &mut Vec::new(), &ToolDeps::default()),
            Ok(ToolResult::Moved { .. })
        ));
        assert!(root.join("b.txt").exists());
    }
}
