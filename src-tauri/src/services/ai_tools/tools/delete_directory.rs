//! `deleteDirectory` — removal of a folder, non-recursive unless asked.
//!
//! Deliberately *not* guarded by the read registry, unlike `deleteFile`.
//! "Has the agent read this directory" has no meaning, and requiring a read of
//! every file inside would make the tool unusable on the trees it exists for.
//! What stands in its place is approval: this is a mutating tool, so a human
//! sees the path before anything happens. The non-recursive default is the
//! other half — an over-broad path costs one refusal rather than a tree.

use crate::domain::llm::LlmToolDefinition;
use std::fs;

use crate::domain::tools::{DeleteDirectoryArgs, ToolError, ToolResult, ToolScope};

use super::super::resolve::{relative_to_root, resolve_existing};

/// Paths a recursive delete names; past it, a count.
const MAX_LISTED: usize = 50;

pub fn delete_directory(
    scope: &ToolScope,
    args: &DeleteDirectoryArgs,
) -> Result<ToolResult, ToolError> {
    let path = resolve_existing(scope, &args.path)?;
    if !path.is_dir() {
        return Err(ToolError::NotFound(args.path.clone()));
    }
    if path == scope.root() {
        return Err(ToolError::PathEscape(args.path.clone()));
    }
    let relative = relative_to_root(scope, &path)?;

    let empty = fs::read_dir(&path)
        .map_err(ToolError::Io)?
        .next()
        .is_none();
    if !empty && !args.recursive.unwrap_or(false) {
        return Err(ToolError::DirectoryNotEmpty(relative));
    }

    let under = super::move_path::files_under(&path);
    let files = under.len();
    let listed = under.iter().take(MAX_LISTED).filter_map(|file| relative_to_root(scope, file).ok()).collect();
    if empty {
        fs::remove_dir(&path).map_err(ToolError::Io)?;
    } else {
        fs::remove_dir_all(&path).map_err(ToolError::Io)?;
    }

    Ok(ToolResult::DirectoryDeleted { path: relative, files, listed })
}

/// What the model is told `deleteDirectory` is for.
pub(super) fn definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "deleteDirectory".to_string(),
        description: "Delete a directory. Without `recursive` a directory with contents is refused, so a path that turned out broader than you meant costs one refusal instead of a tree."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Directory path relative to the workspace root. The workspace root itself cannot be deleted."
                },
                "recursive": {
                    "type": [
                        "boolean",
                        "null"
                    ],
                    "description": "Default false. True deletes the directory and everything under it. Prefer listing it first."
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
    use crate::domain::tools::{ReadFiles, ToolCall, ToolDeps};
    use crate::services::ai_tools::tools::execute_tool;
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

    #[test]
    fn removes_an_empty_directory() {
        let (scope, root, _) = fixture("rmdir-empty");
        std::fs::create_dir(root.join("empty")).expect("creatable");

        let result = delete_directory(
            &scope,
            &DeleteDirectoryArgs { path: "empty".into(), recursive: None },
        )
        .expect("removes");

        assert!(!root.join("empty").exists());
        assert!(matches!(result, ToolResult::DirectoryDeleted { ref path, files: 0, .. } if path == "empty"));
    }

    /// The default that makes an over-broad path cost one refusal instead of a
    /// tree.
    #[test]
    fn a_directory_with_contents_needs_recursive_said_out_loud() {
        let (scope, root, _) = fixture("rmdir-nonempty");
        write(&root, "full/a.txt", "x");

        let err = delete_directory(
            &scope,
            &DeleteDirectoryArgs { path: "full".into(), recursive: None },
        )
        .expect_err("not empty");

        assert!(matches!(err, ToolError::DirectoryNotEmpty(_)));
        assert!(root.join("full/a.txt").exists(), "nothing removed");
    }

    #[test]
    fn recursive_removes_the_whole_subtree() {
        let (scope, root, _) = fixture("rmdir-recursive");
        write(&root, "full/deep/a.txt", "x");
        write(&root, "full/b.txt", "x");

        let result = delete_directory(
            &scope,
            &DeleteDirectoryArgs { path: "full".into(), recursive: Some(true) },
        )
        .expect("removes");

        assert!(!root.join("full").exists());
        let ToolResult::DirectoryDeleted { files, listed, .. } = result else { panic!() };
        assert_eq!((files, listed), (2, vec!["full/b.txt".to_string(), "full/deep/a.txt".to_string()]));
    }

    /// Nothing addresses the scope root but a mistake, and honouring it would
    /// delete the whole workspace.
    #[test]
    fn the_root_itself_can_never_be_deleted() {
        let (scope, root, _) = fixture("rmdir-root");
        write(&root, "a.txt", "x");

        for path in [".", ""] {
            assert!(
                delete_directory(
                    &scope,
                    &DeleteDirectoryArgs { path: path.into(), recursive: Some(true) },
                )
                .is_err(),
                "{path:?} was accepted"
            );
        }
        assert!(root.join("a.txt").exists());
    }

    #[test]
    fn a_path_leaving_the_root_is_refused() {
        let (scope, _, _) = fixture("rmdir-escape");
        assert!(matches!(
            delete_directory(
                &scope,
                &DeleteDirectoryArgs { path: "../outside".into(), recursive: Some(true) },
            ),
            Err(ToolError::PathEscape(_))
        ));
    }

    #[test]
    fn the_call_wire_shape_is_stable_and_counts_as_risky() {
        let call: ToolCall = serde_json::from_str(
            r#"{"tool": "deleteDirectory", "args": {"path": "sub", "recursive": "yes"}}"#,
        )
        .expect("parses, spelled-out boolean and all");
        let ToolCall::DeleteDirectory(parsed) = &call else {
            panic!("wrong variant")
        };
        assert_eq!(parsed.recursive, Some(true));
        assert!(call.is_risky());

        let (scope, root, mut reads) = fixture("rmdir-dispatch");
        write(&root, "sub/a.txt", "x");
        assert!(matches!(
            execute_tool(&scope, &call, &mut reads, &mut Vec::new(), &ToolDeps::default()),
            Ok(ToolResult::DirectoryDeleted { .. })
        ));
        assert!(!root.join("sub").exists());
    }
}
