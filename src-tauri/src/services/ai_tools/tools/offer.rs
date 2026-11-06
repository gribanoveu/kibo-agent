//! `offer` — a next step put to the user, not taken: a button under the
//! answer that sends an ordinary message when pressed (`/review`, a request
//! to commit). Nothing runs here, so there is nothing to approve.
//!
//! The tool, not the prompt, decides whether there is anything to offer: a
//! review needs a change `/review` would read, a commit a tree that differs
//! from HEAD, and implementing a plan needs one. Otherwise the model reads
//! why not, and no button appears.

use crate::domain::llm::LlmToolDefinition;
use crate::domain::review;
use crate::domain::tools::{OfferArgs, OfferKind, ToolDeps, ToolError, ToolResult, ToolScope};
use crate::infra::{git_branches, git_changes};

pub fn offer(scope: &ToolScope, args: &OfferArgs, deps: &ToolDeps) -> Result<ToolResult, ToolError> {
    if args.reason.trim().is_empty() {
        return Err(ToolError::InvalidArguments {
            tool: "offer".into(),
            reason: "reason is empty — say in a few words why, the user reads it on the button".into(),
        });
    }
    match args.kind {
        OfferKind::Review | OfferKind::Commit => {
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
                _ => {}
            }
        }
        OfferKind::Worktree => {
            if deps.parallel.is_none_or(|parallel| parallel().is_empty()) {
                return Err(ToolError::Command(
                    "a worktree is offered only while another chat is running in this checkout".into(),
                ));
            }
            if crate::infra::git_head::current_branch(scope.root()).is_none() {
                return Err(ToolError::Command("a worktree is offered only from a folder on a git branch".into()));
            }
        }
        OfferKind::PullRequest => {
            let base = deps.worktree_base.ok_or_else(|| {
                ToolError::Command("a pull request is offered only from a chat moved to its own worktree".into())
            })?;
            git_branches::pr_ready(scope.root(), base).map_err(|e| ToolError::Command(e.to_string()))?;
        }
        OfferKind::Implement => {
            if !deps.has_plan {
                return Err(ToolError::Command("no plan to implement: write it with writePlan first".into()));
            }
        }
    }
    Ok(ToolResult::Offered { kind: args.kind })
}

pub(super) fn definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "offer".to_string(),
        description: "Offer the user a next step as a button shown after your answer; nothing runs now, and pressing it sends the step as the user's next message. \
review: once, at the end of a task whose change is not trivial — several files, logic, concurrency, security, error handling; not for typos, docs or renames. Pressed, it runs a review helper that reads the change with a context of its own, and its findings come back to you to fix or dismiss. \
commit: when the task is done and checked, and the user has not said they commit themselves. \
worktree: while another chat is running in this checkout and this task needs file changes; pressing it moves only this chat to a clean worktree from the current branch. \
pullRequest: after committing this chat's worktree changes, when the branch is ahead of and not behind its recorded base; pressing it asks you to push and create the PR with confirmed commands. \
implement: after writePlan, when the user asked for a plan rather than the work; pressing it asks you to carry the plan out, so end your turn there. \
Do not also ask about it in your answer: the button is the question. Calling a kind again replaces its reason."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "kind": { "type": "string", "enum": ["review", "commit", "worktree", "pullRequest", "implement"] },
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
            assert_eq!(offer(&scope, &args(kind, "the counter"), &ToolDeps::default()).unwrap(), ToolResult::Offered { kind });
        }
    }

    #[test]
    fn a_clean_tree_has_nothing_to_offer() {
        let (_dir, scope) = folder("offer-clean");
        let review = offer(&scope, &args(OfferKind::Review, "x"), &ToolDeps::default()).unwrap_err();
        assert!(review.to_string().contains("nothing to review"), "{review}");
        let commit = offer(&scope, &args(OfferKind::Commit, "x"), &ToolDeps::default()).unwrap_err();
        assert!(commit.to_string().contains("nothing to commit"), "{commit}");
    }

    /// Only documentation changed: worth a commit, not a review — the same
    /// line `/review` draws.
    #[test]
    fn what_review_leaves_out_is_still_a_commit() {
        let (dir, scope) = folder("offer-docs");
        fs::write(dir.join("README.md"), "# notes\n").unwrap();
        let review = offer(&scope, &args(OfferKind::Review, "x"), &ToolDeps::default()).unwrap_err();
        assert!(review.to_string().contains("left out"), "{review}");
        assert!(offer(&scope, &args(OfferKind::Commit, "the notes"), &ToolDeps::default()).is_ok());
    }

    #[test]
    fn a_worktree_is_offered_only_beside_a_live_neighbour() {
        let (_dir, scope) = folder("offer-worktree");
        let refused = offer(&scope, &args(OfferKind::Worktree, "separate the edits"), &ToolDeps::default()).unwrap_err();
        assert!(refused.to_string().contains("another chat"), "{refused}");

        let parallel = || vec!["other chat".to_string()];
        let deps = ToolDeps { parallel: Some(&parallel), ..ToolDeps::default() };
        let outside = ToolScope::new(&temp_dir("offer-worktree-outside-git")).unwrap();
        let refused = offer(&outside, &args(OfferKind::Worktree, "separate the edits"), &deps).unwrap_err();
        assert!(refused.to_string().contains("git branch"), "{refused}");
        assert_eq!(
            offer(&scope, &args(OfferKind::Worktree, "separate the edits"), &deps).unwrap(),
            ToolResult::Offered { kind: OfferKind::Worktree }
        );
    }

    #[test]
    fn a_pull_request_needs_its_recorded_base_and_a_ready_worktree() {
        let (dir, scope) = folder("offer-pull-request");
        let refused = offer(&scope, &args(OfferKind::PullRequest, "publish it"), &ToolDeps::default()).unwrap_err();
        assert!(refused.to_string().contains("moved"), "{refused}");

        let base = crate::infra::git_head::current_branch(&dir).unwrap();
        let path = git_branches::add_agent_worktree(&dir, &base, "chat-1", &temp_dir("offer-pr-home")).unwrap();
        let scope = ToolScope::new(&path).unwrap();
        let deps = ToolDeps { worktree_base: Some(&base), ..ToolDeps::default() };
        assert!(offer(&scope, &args(OfferKind::PullRequest, "publish it"), &deps).is_err(), "no commit is not a PR");

        fs::write(path.join("a.rs"), "two\n").unwrap();
        git_changes::stage(&path, &["a.rs".into()]).unwrap();
        git_changes::commit(&path, "change").unwrap();
        assert_eq!(
            offer(&scope, &args(OfferKind::PullRequest, "publish it"), &deps).unwrap(),
            ToolResult::Offered { kind: OfferKind::PullRequest }
        );
    }

    #[test]
    fn a_plan_is_offered_only_once_there_is_one() {
        let (_dir, scope) = folder("offer-implement");
        let refused = offer(&scope, &args(OfferKind::Implement, "the parser"), &ToolDeps::default()).unwrap_err();
        assert!(refused.to_string().contains("no plan"), "{refused}");
        let deps = ToolDeps { has_plan: true, ..ToolDeps::default() };
        assert_eq!(
            offer(&scope, &args(OfferKind::Implement, "the parser"), &deps).unwrap(),
            ToolResult::Offered { kind: OfferKind::Implement }
        );
    }

    /// A kind left out of the schema is one the model is never told it can use.
    #[test]
    fn the_schema_lists_every_kind() {
        let schema = definition().parameters;
        let listed = schema["properties"]["kind"]["enum"].as_array().unwrap();
        for kind in [OfferKind::Review, OfferKind::Commit, OfferKind::Worktree, OfferKind::PullRequest, OfferKind::Implement] {
            assert!(listed.contains(&serde_json::to_value(kind).unwrap()), "{kind:?}");
        }
    }

    #[test]
    fn an_offer_says_why() {
        let (dir, scope) = folder("offer-reason");
        fs::write(dir.join("a.rs"), "two\n").unwrap();
        let err = offer(&scope, &args(OfferKind::Commit, "  "), &ToolDeps::default()).unwrap_err();
        assert!(err.to_string().contains("reason is empty"), "{err}");
    }
}
