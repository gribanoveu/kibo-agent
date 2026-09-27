//! `finishReview` — a review worker's closing word on its group: what the
//! change does, what it looked at, and what the diff could not settle. It is
//! what the user reads when nothing is found, and the context when something is.

use crate::domain::llm::LlmToolDefinition;
use crate::domain::review::SummaryArgs;
use crate::domain::tools::{ToolDeps, ToolError, ToolResult};

pub fn finish_review(args: &SummaryArgs, deps: &ToolDeps) -> Result<ToolResult, ToolError> {
    let desk = deps
        .review
        .as_deref()
        .ok_or_else(|| ToolError::Command("finishReview is only available during a review".to_string()))?;
    desk.summarize(args)?;
    Ok(ToolResult::SummaryNoted)
}

pub(super) fn definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "finishReview".to_string(),
        description: "Required in every review reply, exactly once, last: close your review of these files after your reportFinding calls — also when you found nothing. The user reads it beside the findings; a reply without it leaves them no summary."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "summary": { "type": "string", "description": "What the change does in these files, in one or two sentences." },
                "checked": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "The risks you looked at and found sound, a few words each: \"cancellation mid-request\", \"empty input\"."
                },
                "worthALook": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string" },
                            "note": { "type": "string", "description": "The doubt, and what code outside the diff would settle it." }
                        },
                        "required": ["path", "note"]
                    },
                    "description": "Doubts you could not prove or rule out from the code shown. Not findings: at most three, the ones worth the user's time."
                }
            },
            "required": ["summary", "checked"]
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::review::{FileDiff, FileStatus, ReviewDesk};
    use std::sync::Arc;

    #[test]
    fn a_summary_is_kept_on_the_desk_and_refused_outside_a_review() {
        let file = FileDiff { path: "a.rs".into(), status: FileStatus::Modified, binary: false, patch: String::new(), lines: Vec::new(), whole: false };
        let deps = ToolDeps { review: Some(Arc::new(ReviewDesk::new(vec![file]))), ..ToolDeps::default() };
        let args = SummaryArgs { summary: "Adds paging.".into(), ..Default::default() };
        assert_eq!(finish_review(&args, &deps).unwrap(), ToolResult::SummaryNoted);
        assert_eq!(deps.review.unwrap().take_summary().map(|s| s.summary), Some("Adds paging.".into()));
        let outside = finish_review(&args, &ToolDeps::default()).unwrap_err();
        assert!(outside.to_string().contains("only available during a review"), "{outside}");
    }
}
