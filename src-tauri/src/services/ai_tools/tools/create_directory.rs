//! `createDirectory` — a new folder, with any missing parents.
//!
//! Alfa Atlas also scaffolded one from a `restEndpoint` template. That was its
//! documentation product showing through and is not ported.

use crate::domain::llm::LlmToolDefinition;
use std::fs;

use crate::domain::tools::{CreateDirectoryArgs, ToolError, ToolResult, ToolScope};

use super::super::resolve::{relative_to_root, resolve_writable};

pub fn create_directory(
    scope: &ToolScope,
    args: &CreateDirectoryArgs,
) -> Result<ToolResult, ToolError> {
    let path = resolve_writable(scope, &args.path)?;
    // Creating something that is already there is almost always a mistaken
    // assumption about the tree. Saying so teaches the model more than
    // succeeding silently.
    if path.exists() {
        return Err(ToolError::AlreadyExists(args.path.clone()));
    }
    fs::create_dir_all(&path).map_err(ToolError::Io)?;
    Ok(ToolResult::DirectoryCreated {
        path: relative_to_root(scope, &path)?,
    })
}

/// What the model is told `createDirectory` is for.
pub(super) fn definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "createDirectory".to_string(),
        description: "Create a directory, including any missing parents. Refused if anything already exists at the path — a directory included — since what is there may hold files you have not seen."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Directory path relative to the workspace root."
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
    fn creates_the_directory_and_any_missing_parents() {
        let (scope, root, _) = fixture("mkdir-parents");

        let result = create_directory(&scope, &CreateDirectoryArgs { path: "a/b/c".into() })
            .expect("creates");

        assert!(root.join("a/b/c").is_dir());
        assert!(matches!(result, ToolResult::DirectoryCreated { ref path } if path == "a/b/c"));
    }

    /// Succeeding silently would leave the model believing it had just created
    /// something, when what is there may be full of files it has not seen.
    #[test]
    fn creating_over_something_that_exists_is_refused() {
        let (scope, root, _) = fixture("mkdir-exists");
        write(&root, "taken/file.txt", "x");

        assert!(matches!(
            create_directory(&scope, &CreateDirectoryArgs { path: "taken".into() }),
            Err(ToolError::AlreadyExists(_))
        ));
        assert!(root.join("taken/file.txt").exists(), "contents untouched");
    }

    #[test]
    fn a_path_leaving_the_root_is_refused() {
        let (scope, _, _) = fixture("mkdir-escape");
        assert!(matches!(
            create_directory(&scope, &CreateDirectoryArgs { path: "../outside".into() }),
            Err(ToolError::PathEscape(_))
        ));
    }

    #[test]
    fn the_call_wire_shape_is_stable_and_counts_as_risky() {
        let call: ToolCall =
            serde_json::from_str(r#"{"tool": "createDirectory", "args": {"path": "a"}}"#)
                .expect("parses");
        assert!(call.is_risky());
        let (scope, root, mut reads) = fixture("mkdir-dispatch");
        assert!(matches!(
            execute_tool(&scope, &call, &mut reads, &mut Vec::new(), &ToolDeps::default()),
            Ok(ToolResult::DirectoryCreated { .. })
        ));
        assert!(root.join("a").is_dir());
    }
}
