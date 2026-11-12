//! `review` — a change read by a helper turn in Review mode, with a context
//! of its own.
//!
//! The helper sees the diff and nothing of this conversation; its findings
//! come back in the result as `ToolResult::Reviewed`, and the turn goes on
//! with them in hand. The range it reads is chosen without the model
//! (`domain::review::plan`): `from`/`to` name it, and without them the app
//! takes uncommitted changes first, then this chat's work, then the branch.
//! Running it needs the model, which the tool layer does not have —
//! `ToolDeps::review_run` is the loop's port for it.

use crate::domain::llm::LlmToolDefinition;
use crate::domain::tools::{ReviewArgs, ToolDeps, ToolError, ToolResult};

pub fn review(args: &ReviewArgs, deps: &ToolDeps) -> Result<ToolResult, ToolError> {
    let run = deps.review_run.ok_or_else(|| ToolError::Review("not available here".to_string()))?;
    run(args)
}

pub(super) fn definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "review".to_string(),
        description: "Hand the change in this folder to a review helper with a fresh context. It reads the change on its own, runs what proves a point, and reports findings with path:line, checked by the app; its report comes back to you in this conversation, so fix or dismiss each finding and say which. Call it before committing and after a non-trivial change — several files, logic, concurrency, security, error handling; not for typos, docs or renames — and when the user asks for a review. It reads and reports only: it never edits files, and any fixing is yours after it answers. \
from: where the change starts — a branch, tag or commit (`main`, `HEAD~2`). Without it the review takes what this chat changed: uncommitted work first, then this chat's commits, then the branch against its upstream. \
to: where it ends — by default the working tree (commits plus uncommitted); name a commit to review a commit range without uncommitted changes. \
focus: one or two phrases on what to look at first (\"error handling in the loader\") — an accent for the reviewer, not a topic; the review still stands on the diff."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "from": { "type": "string", "description": "Where the change starts: a branch, tag or commit. Leave out to review what this chat changed." },
                "to": { "type": "string", "description": "Where it ends: a commit for a commit range. Leave out for the working tree." },
                "focus": { "type": "string", "description": "One or two phrases on what to look at first." }
            },
            "required": []
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(from: Option<&str>, to: Option<&str>) -> ReviewArgs {
        ReviewArgs { from: from.map(str::to_string), to: to.map(str::to_string), focus: None }
    }

    fn reviewed(range: &str) -> ToolResult {
        ToolResult::Reviewed {
            findings: Vec::new(),
            text: format!("reviewed {range}"),
            range: range.to_string(),
            agent: 1,
            tokens: Default::default(),
            handed: false,
        }
    }

    #[test]
    fn the_range_goes_to_the_helper_and_its_report_comes_back() {
        let run = |args: &ReviewArgs| Ok(reviewed(&format!("{}→{}", args.from.as_deref().unwrap_or("HEAD"), args.to.as_deref().unwrap_or("workdir"))));
        let deps = ToolDeps { review_run: Some(&run), ..ToolDeps::default() };
        assert_eq!(review(&args(Some("main"), None), &deps).unwrap(), reviewed("main→workdir"));
        assert_eq!(review(&args(None, Some("abc1234")), &deps).unwrap(), reviewed("HEAD→abc1234"));
    }

    #[test]
    fn no_helper_is_an_error_the_model_can_read() {
        let err = review(&args(None, None), &ToolDeps::default()).unwrap_err();
        assert!(err.to_string().contains("not available"), "{err}");
    }
}
