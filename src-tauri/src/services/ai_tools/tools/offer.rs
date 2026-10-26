//! `offer` — a next step put to the user, not taken: a button under the
//! answer that sends an ordinary message when pressed (`/review`, a request
//! to commit). Nothing runs here, so there is nothing to approve.
//!
//! The tool, not the prompt, decides whether there is anything to offer: a
//! review needs a change `/review` would read, a commit a tree that differs
//! from HEAD. Otherwise the model reads why not, and no button appears.

use crate::domain::llm::LlmToolDefinition;
use crate::domain::review;
use crate::domain::tools::{OfferArgs, OfferKind, ToolError, ToolResult, ToolScope};
use crate::infra::git_changes;

pub fn offer(scope: &ToolScope, args: &OfferArgs) -> Result<ToolResult, ToolError> {
    if args.reason.trim().is_empty() {
        return Err(ToolError::InvalidArguments {
            tool: "offer".into(),
            reason: "reason is empty — say in a few words why, the user reads it on the button".into(),
        });
    }
    let files = git_changes::review_diffs(scope.root()).map_err(|e| ToolError::Git(e.to_string()))?;
    match args.kind {
        OfferKind::Review => {
            let (kept, excluded) = review::select(files);
            if kept.is_empty() {
                return Err(ToolError::Command(review::nothing_to_review(excluded.len())));
            }
        }
        OfferKind::Commit if files.is_empty() => {
            return Err(ToolError::Command("nothing to commit: the working tree matches HEAD".into()));
        }
        OfferKind::Commit => {}
    }
    Ok(ToolResult::Offered { kind: args.kind })
}

pub(super) fn definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "offer".to_string(),
        description: "Offer the user a next step as a button shown after your answer; nothing runs now, and pressing it sends the step as the user's next message. \
review: once, at the end of a task whose change is not trivial — several files, logic, concurrency, security, error handling; not for typos, docs or renames. Pressed, it runs a review helper that reads the change with a context of its own, and its findings come back to you to fix or dismiss. \
commit: when the task is done and checked, and the user has not said they commit themselves. \
Do not also ask about it in your answer: the button is the question. Calling a kind again replaces its reason."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "kind": { "type": "string", "enum": ["review", "commit"] },
                "reason": { "type": "string", "description": "Why, in a few words the user reads on the button: what the change touched." }
            },
            "required": ["kind", "reason"]
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::temp_dir;
    use std::fs;

    fn folder(label: &str) -> (std::path::PathBuf, ToolScope) {
        let dir = temp_dir(label);
        let repo = git2::Repository::init(&dir).unwrap();
        let mut config = repo.config().unwrap();
        config.set_str("user.name", "Test").unwrap();
        config.set_str("user.email", "test@example.com").unwrap();
        fs::write(dir.join("a.rs"), "one\n").unwrap();
        git_changes::stage(&dir, &["a.rs".into()]).unwrap();
        git_changes::commit(&dir, "first").unwrap();
        let scope = ToolScope::new(&dir).unwrap();
        (dir, scope)
    }

    fn args(kind: OfferKind, reason: &str) -> OfferArgs {
        OfferArgs { kind, reason: reason.into() }
    }

    #[test]
    fn a_change_can_be_offered_for_review_and_commit() {
        let (dir, scope) = folder("offer-change");
        fs::write(dir.join("a.rs"), "two\n").unwrap();
        for kind in [OfferKind::Review, OfferKind::Commit] {
            assert_eq!(offer(&scope, &args(kind, "the counter")).unwrap(), ToolResult::Offered { kind });
        }
    }

    #[test]
    fn a_clean_tree_has_nothing_to_offer() {
        let (_dir, scope) = folder("offer-clean");
        let review = offer(&scope, &args(OfferKind::Review, "x")).unwrap_err();
        assert!(review.to_string().contains("nothing to review"), "{review}");
        let commit = offer(&scope, &args(OfferKind::Commit, "x")).unwrap_err();
        assert!(commit.to_string().contains("nothing to commit"), "{commit}");
    }

    /// Only documentation changed: worth a commit, not a review — the same
    /// line `/review` draws.
    #[test]
    fn what_review_leaves_out_is_still_a_commit() {
        let (dir, scope) = folder("offer-docs");
        fs::write(dir.join("README.md"), "# notes\n").unwrap();
        let review = offer(&scope, &args(OfferKind::Review, "x")).unwrap_err();
        assert!(review.to_string().contains("left out"), "{review}");
        assert!(offer(&scope, &args(OfferKind::Commit, "the notes")).is_ok());
    }

    #[test]
    fn an_offer_says_why() {
        let (dir, scope) = folder("offer-reason");
        fs::write(dir.join("a.rs"), "two\n").unwrap();
        let err = offer(&scope, &args(OfferKind::Commit, "  ")).unwrap_err();
        assert!(err.to_string().contains("reason is empty"), "{err}");
    }
}
