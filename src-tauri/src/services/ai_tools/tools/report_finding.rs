//! `reportFinding` — how a review says something is wrong.
//!
//! Checked here, at once, against the change under review
//! (`domain::review::ReviewDesk`): a finding on a file the change did not
//! touch, or quoting code it did not add, comes back as an error the agent
//! can correct, rather than reaching the user misplaced.

use crate::domain::llm::LlmToolDefinition;
use crate::domain::review::FindingArgs;
use crate::domain::tools::{ToolDeps, ToolError, ToolResult};

pub fn report_finding(args: &FindingArgs, deps: &ToolDeps) -> Result<ToolResult, ToolError> {
    let desk = deps
        .review
        .as_deref()
        .ok_or_else(|| ToolError::Command("reportFinding is only available during a review".to_string()))?;
    let finding = desk.note(args)?;
    Ok(ToolResult::FindingNoted { path: finding.path, start_line: finding.start_line, end_line: finding.end_line })
}

pub(super) fn definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "reportFinding".to_string(),
        description: "Report one problem the change introduces. It is placed on the change by existingCode: one or a few consecutive lines the change added, copied exactly from the file's diff (without the leading +). Report each problem once, in a call of its own; a finding the tool refuses says why — correct it and call again."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "The file, as its diff names it." },
                "existingCode": { "type": "string", "description": "One or a few consecutive added lines the problem is in, copied exactly." },
                "title": { "type": "string", "description": "The problem in a few words." },
                "body": { "type": "string", "description": "One to three sentences: what goes wrong, and when — the input or state that triggers it." },
                "severity": {
                    "type": "string",
                    "enum": ["critical", "high", "medium", "low"],
                    "description": "critical: data loss, a security hole, a crash on a common path. high: wrong results on a realistic input. medium: an edge case. low: minor."
                },
                "category": { "type": "string", "enum": ["bug", "security", "performance", "maintainability", "other"] },
                "suggestion": { "type": ["string", "null"], "description": "The corrected code, when it is short and certain." }
            },
            "required": ["path", "existingCode", "title", "body", "severity", "category"]
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::review::{FileDiff, FileStatus, NewLine, ReviewDesk};
    use std::sync::Arc;

    fn deps() -> ToolDeps<'static> {
        let file = FileDiff {
            path: "a.rs".into(),
            status: FileStatus::Modified,
            binary: false,
            patch: String::new(),
            lines: vec![NewLine { hunk: 0, number: 7, added: true, text: "let x = y / 0;".into() }],
        };
        ToolDeps { review: Some(Arc::new(ReviewDesk::new(vec![file]))), ..ToolDeps::default() }
    }

    fn finding(code: &str) -> FindingArgs {
        FindingArgs { path: "a.rs".into(), existing_code: code.into(), title: "t".into(), body: "b".into(), ..Default::default() }
    }

    #[test]
    fn a_finding_on_the_change_is_kept_and_placed() {
        let deps = deps();
        let noted = report_finding(&finding("let x = y / 0;"), &deps).unwrap();
        assert_eq!(noted, ToolResult::FindingNoted { path: "a.rs".into(), start_line: 7, end_line: 7 });
        // Kept: the same report again is refused as a duplicate.
        let again = report_finding(&finding("let x = y / 0;"), &deps).unwrap_err();
        assert!(again.to_string().contains("already reported at a.rs:7"), "{again}");
    }

    #[test]
    fn a_misplaced_one_is_refused_with_the_way_to_fix_it() {
        let err = report_finding(&finding("let x = 1;"), &deps()).unwrap_err();
        assert!(err.to_string().contains("Copy one or a few consecutive `+` lines"), "{err}");
        let outside = report_finding(&finding(""), &ToolDeps::default()).unwrap_err();
        assert!(outside.to_string().contains("only available during a review"), "{outside}");
    }
}
