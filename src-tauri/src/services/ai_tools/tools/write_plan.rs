//! `writePlan` — the conversation's plan, as a document the user reads and
//! edits in the Plan tab.
//!
//! Stateless like `todo`: the plan belongs to the chat, is sent into each
//! turn by the window and comes back out of this call's arguments. The
//! result is a receipt, not the text again — the model has just written it,
//! and echoing it would put every plan into the context twice.
//!
//! Replaces Alfa Atlas's plan store (`services/plans.rs`, four tools): a
//! plan here lives with the chat, beside the checklist, rather than in a
//! second store with a second list of steps.

use crate::domain::llm::LlmToolDefinition;
use crate::domain::tools::{ToolError, ToolResult, WritePlanArgs};

/// A plan is something a person reads through; past this it is a spec, and
/// every request would carry it.
pub const MAX_PLAN_CHARS: usize = 20_000;

pub fn write_plan(args: &WritePlanArgs) -> Result<ToolResult, ToolError> {
    let content = args.content.trim();
    let invalid = |reason: String| ToolError::InvalidArguments { tool: "writePlan".into(), reason };
    if content.is_empty() {
        return Err(invalid("content is empty — send the whole plan".into()));
    }
    let chars = content.chars().count();
    if chars > MAX_PLAN_CHARS {
        return Err(invalid(format!(
            "the plan is {chars} characters, over the {MAX_PLAN_CHARS} limit — say less per step"
        )));
    }
    Ok(ToolResult::PlanWritten { lines: content.lines().count() as u32 })
}

pub(super) fn definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "writePlan".to_string(),
        description: "Write the plan of work for this conversation as a Markdown document: a title, what changes and why, the files involved, the steps in order, and the risks or open questions. This is the plan itself — where the work is decided and explained. \
Each call replaces the whole plan, so send all of it. The user reads it in the Plan tab and may edit it; the version you are shown under \"Plan\" in the system prompt is the current one, including their edits. \
Outline its steps in the checklist with todo as well: the plan is the work, the checklist the short list you check it against. \
The plan is in every request in full, while old tool results are cleared and the older conversation is summarized as the context fills: on a long task, keep in it the facts you will need later — names, paths, line numbers — not only the steps."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "The whole plan, in Markdown."
                }
            },
            "required": ["content"]
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(content: &str) -> WritePlanArgs {
        WritePlanArgs { content: content.to_string() }
    }

    #[test]
    fn a_plan_is_acknowledged_by_its_size_not_echoed() {
        assert_eq!(write_plan(&args("# Fix\n\n1. read\n2. fix\n")).unwrap(), ToolResult::PlanWritten { lines: 4 });
    }

    #[test]
    fn an_empty_or_oversized_plan_is_refused_with_why() {
        let empty = write_plan(&args("  \n")).unwrap_err().to_string();
        assert!(empty.contains("content is empty"), "{empty}");
        assert!(write_plan(&args(&"я".repeat(MAX_PLAN_CHARS))).is_ok());
        let long = write_plan(&args(&"я".repeat(MAX_PLAN_CHARS + 1))).unwrap_err().to_string();
        assert!(long.contains("over the 20000 limit"), "{long}");
    }
}
