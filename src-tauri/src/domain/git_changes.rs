//! The working tree's changes as the Changes panel shows them: what is staged,
//! what is not, and how many lines each file adds and removes.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// One changed file. `path` is relative to the repository, which is also what
/// staging it takes back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangedFile {
    pub path: String,
    pub add: usize,
    pub del: usize,
}

/// A file with both staged and unstaged edits is in both lists, with each
/// side's own counts — as `git status` shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkingChanges {
    pub staged: Vec<ChangedFile>,
    pub unstaged: Vec<ChangedFile>,
}

/// Everything changed since the last commit, staged or not, added up — what
/// the chat header shows beside the branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeTotals {
    pub files: usize,
    pub add: usize,
    pub del: usize,
}

/// One commit of the History tab: its first line, who and when, and the
/// branches and tags that point at it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitSummary {
    /// Short id, as `git commit` reports it.
    pub id: String,
    pub summary: String,
    pub author: String,
    /// Seconds since the epoch.
    pub time: i64,
    /// HEAD points at it.
    pub head: bool,
    /// Short names of the other branches, local and remote, and the tags on it.
    /// The checked-out branch is left out: HEAD already says where it is.
    pub refs: Vec<String>,
}

/// The History tab: where HEAD is, how it stands against its upstream, and
/// the newest commits from it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHistory {
    /// The checked-out branch, or HEAD's short id when it is detached.
    pub branch: Option<String>,
    /// The branch it tracks, `None` when it tracks none.
    pub upstream: Option<String>,
    /// Commits on the branch that the upstream does not have, and the other way.
    pub ahead: usize,
    pub behind: usize,
    pub commits: Vec<CommitSummary>,
    /// Older commits are left beyond the limit asked for.
    pub more: bool,
}

/// Which two versions of a file the viewer compares, and what its path is
/// relative to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FileSide {
    /// The index against the working tree; the path is the repository's.
    Unstaged,
    /// HEAD against the index; the path is the repository's.
    Staged,
    /// HEAD against the disk; the path is the open folder's, which may be
    /// outside any repository — then there is nothing to compare against.
    Worktree,
}

/// Why a file is named but not drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Unviewable {
    Binary,
    TooLarge,
}

/// The two versions of a file, for the viewer to diff. `None` on a side the
/// file does not exist on: `old` for a new file, `new` for a deleted one.
/// Both are `None` when `unviewable` says why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileView {
    pub old: Option<String>,
    pub new: Option<String>,
    pub unviewable: Option<Unviewable>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum GitChangesError {
    #[error("the open folder is not in a git repository")]
    NotARepository,
    #[error("not a path inside the repository: {0}")]
    InvalidPath(String),
    #[error("the commit message is empty")]
    EmptyMessage,
    #[error("not a commit, branch or tag: {0}")]
    UnknownRef(String),
    #[error("nothing is staged to commit")]
    NothingStaged,
    #[error("git user.name and user.email are not set; set them with git config")]
    MissingIdentity,
    #[error("could not read {0}")]
    Read(String),
    #[error("git: {0}")]
    Git(String),
}
