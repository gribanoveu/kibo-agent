//! `present` — the Prototype role's page put in front of the user
//! (`docs/31-prototype-role.md`, P-4): the call names the page, the chat's
//! folder is kept as a version beside it, and the transcript shows a card.

use crate::domain::llm::LlmToolDefinition;
use crate::domain::tools::{PresentArgs, ToolError, ToolResult, ToolScope};
use crate::infra::prototype_store;
use crate::services::ai_tools::resolve::{relative_to_root, resolve_existing};

/// A card's name, not a sentence.
const MAX_TITLE_CHARS: usize = 80;

pub fn present(scope: &ToolScope, args: &PresentArgs) -> Result<ToolResult, ToolError> {
    let invalid = |reason: String| ToolError::InvalidArguments { tool: "present".into(), reason };
    let page = resolve_existing(scope, &args.path)?;
    let html = page.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("html") || e.eq_ignore_ascii_case("htm"));
    if !page.is_file() || !html {
        return Err(invalid(format!("{} is not an HTML page — present the .html file the user should see", args.path)));
    }
    let title = args.title.trim();
    if title.is_empty() {
        return Err(invalid("title is empty — name the page in a few words".into()));
    }
    if title.chars().count() > MAX_TITLE_CHARS {
        return Err(invalid(format!("the title is over {MAX_TITLE_CHARS} characters — a few words name the page")));
    }
    let path = relative_to_root(scope, &page)?;
    let version = prototype_store::snapshot(scope.root())?;
    Ok(ToolResult::Presented { path, title: title.to_string(), version })
}

pub(super) fn definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "present".to_string(),
        description: "Show the user a page you wrote in this chat's folder: it opens beside the chat, and the folder is kept as a numbered version the user can go back to. \
Call it once the page is ready to look at — after it is written, and again after each change worth seeing. A page that is written but not presented is not in front of the user."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The HTML page, relative to the chat's folder, e.g. index.html."
                },
                "title": {
                    "type": "string",
                    "description": "What the page is, in a few words, for its card: \"Coffee shop landing page\"."
                }
            },
            "required": ["path", "title"]
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const ID: &str = "3c5e7a9b-1d2f-4a6c-8e0b-2d4f6a8c0e1a";

    /// A chat's working folder, as `prototype_store` makes it, with a page.
    fn chat(label: &str) -> (ToolScope, std::path::PathBuf) {
        let files = crate::testing::temp_dir(label).join("prototypes").join(ID).join("files");
        fs::create_dir_all(&files).unwrap();
        fs::write(files.join("index.html"), "<h1>hi</h1>").unwrap();
        (ToolScope::new(&files).unwrap(), files)
    }

    fn args(path: &str, title: &str) -> PresentArgs {
        PresentArgs { path: path.into(), title: title.into() }
    }

    #[test]
    fn a_page_is_presented_and_its_folder_kept_as_the_next_version() {
        let (scope, files) = chat("present-ok");
        let shown = present(&scope, &args("index.html", "  Coffee shop  ")).unwrap();
        assert_eq!(shown, ToolResult::Presented { path: "index.html".into(), title: "Coffee shop".into(), version: 1 });
        assert!(files.parent().unwrap().join("versions").join("1").join("index.html").is_file());
        let again = present(&scope, &args("./index.html", "Coffee shop")).unwrap();
        assert!(matches!(again, ToolResult::Presented { version: 2, .. }), "{again:?}");
    }

    /// What could only be a mistake is refused with what to do instead, and
    /// no version is kept for it.
    #[test]
    fn what_is_not_a_page_in_the_folder_is_refused_and_keeps_nothing() {
        let (scope, files) = chat("present-refused");
        fs::write(files.join("notes.md"), "# notes").unwrap();
        fs::create_dir_all(files.join("site.html")).unwrap();
        for (call, said) in [
            (args("notes.md", "Notes"), "not an HTML page"),
            (args("site.html", "A folder"), "not an HTML page"),
            (args("missing.html", "Gone"), "not found"),
            (args("../outside.html", "Out"), "escapes tool root"),
            (args("index.html", "  "), "title is empty"),
            (args("index.html", &"x".repeat(MAX_TITLE_CHARS + 1)), "over 80 characters"),
        ] {
            let refused = present(&scope, &call).unwrap_err().to_string();
            assert!(refused.to_lowercase().contains(&said.to_lowercase()), "{call:?}: {refused}");
        }
        assert!(!files.parent().unwrap().join("versions").exists());
    }
}
