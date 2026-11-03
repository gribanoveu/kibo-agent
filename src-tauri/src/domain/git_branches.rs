//! Switching the open folder's branch, and starting a worktree instead.
//!
//! **The app never overwrites work to switch.** A checkout that would change
//! a file holding uncommitted edits is refused with the list of those files —
//! not stashed, not forced, not merged. Deciding what happens to that work is
//! the user's, or the agent's when asked; a worktree is offered because it
//! needs no decision at all.

use serde::Serialize;
use thiserror::Error;

/// A branch a chat can start on. A remote one is listed only when there is no
/// local branch of the same name; checking it out creates that local branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchRef {
    /// `main`, or `origin/feature` for a remote one.
    pub name: String,
    pub remote: bool,
}

/// What a checkout did. A conflict is an answer, not a failure: the folder is
/// exactly as it was, and the window shows which files stood in the way.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum CheckoutOutcome {
    Switched,
    /// Paths relative to the repository: files with changes or untracked
    /// files that the other branch would overwrite.
    Conflicts { paths: Vec<String> },
}

#[derive(Debug, Error)]
pub enum GitBranchError {
    #[error("not a git repository")]
    NotARepository,
    #[error("no branch named {0}")]
    NoSuchBranch(String),
    /// Git keeps one branch in one working tree.
    #[error("{branch} is checked out in {path} — open that folder instead")]
    CheckedOutElsewhere { branch: String, path: String },
    #[error("the repository is in the middle of a {0} — finish or abort it first")]
    Busy(String),
    #[error("the open folder is not a git worktree")]
    NotAWorktree,
    /// Removing the folder would lose these; the app does not decide that.
    #[error("uncommitted changes in {} — commit or discard them first", .0.join(", "))]
    Dirty(Vec<String>),
    /// A detached HEAD's own commits are held by nothing but the worktree.
    #[error("{0} commits are on no branch — make a branch for them first")]
    Unreachable(usize),
    #[error("the worktree is locked: {0}")]
    Locked(String),
    #[error("{0}")]
    Git(String),
}

/// A linked worktree as removing it would find it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeState {
    /// The main working tree, which the window goes back to.
    pub main: String,
    /// `None` when HEAD is detached.
    pub branch: Option<String>,
    /// Changed and untracked files, relative to the worktree. Any at all and
    /// removal is refused.
    pub dirty: Vec<String>,
    /// Commits on HEAD that no other branch and no remote holds. The branch
    /// stays when there are any, so the work is not lost with the folder.
    pub own_commits: usize,
    /// Removal leaves the branch — [`keeps_branch`].
    pub keeps_branch: bool,
}

/// Prefix of every branch a worktree is started on, so the ones the app made
/// are told apart from the user's own.
pub const WORKTREE_BRANCH_PREFIX: &str = "kibo/";
/// A chat moved beside another running one gets its own, distinct namespace.
pub const AGENT_WORKTREE_BRANCH_PREFIX: &str = "agent/";

/// Whether removing a worktree leaves its branch: when commits are only on
/// it, or when the user made it — theirs, merged or not.
pub fn keeps_branch(branch: &str, own_commits: usize) -> bool {
    own_commits > 0 || !(branch.starts_with(WORKTREE_BRANCH_PREFIX) || branch.starts_with(AGENT_WORKTREE_BRANCH_PREFIX))
}

/// Where a worktree started from `base` at `stamp` is named, the `n`th try
/// counting from one: `main-0925-2214`, then `main-0925-2214-2` for a second
/// in the same minute. The time tells one worktree of a branch from the next
/// in the folder list. A branch name may hold `/`, a folder name may not.
pub fn worktree_name(base: &str, stamp: &str, n: usize) -> String {
    let kept = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.');
    let flat: String = base.chars().map(|c| if kept(c) { c } else { '-' }).collect();
    let flat = flat.trim_matches(|c| c == '-' || c == '.');
    let flat = if flat.is_empty() { "worktree" } else { flat };
    if n <= 1 {
        format!("{flat}-{stamp}")
    } else {
        format!("{flat}-{stamp}-{n}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_branch_is_kept_for_its_own_commits_or_for_being_the_user_s() {
        assert!(!keeps_branch("kibo/main-t", 0));
        assert!(keeps_branch("kibo/main-t", 1));
        assert!(keeps_branch("feature", 0));
    }

    #[test]
    fn a_worktree_is_named_after_its_base_and_counts_up() {
        assert_eq!(worktree_name("main", "0925-2214", 1), "main-0925-2214");
        assert_eq!(worktree_name("main", "0925-2214", 2), "main-0925-2214-2");
        assert_eq!(worktree_name("origin/feat/login", "t", 1), "origin-feat-login-t");
        assert_eq!(worktree_name("../..", "t", 1), "worktree-t");
        assert_eq!(worktree_name("fix bug", "t", 3), "fix-bug-t-3");
        assert_eq!(worktree_name("release/v1.2_rc", "t", 1), "release-v1.2_rc-t");
    }
}
