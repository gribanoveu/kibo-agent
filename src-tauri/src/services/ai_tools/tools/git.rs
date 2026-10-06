//! `gitStatus`, `gitDiff`, `gitBlame`, `gitLog` — read-only history.
//!
//! Written against `git2` directly rather than ported. Alfa Atlas's tool is
//! 461 lines over a git layer of 4,400 more (clone, credentials, branches,
//! commits, push, pull, stash) because it *is* a git client. Three read
//! operations do not need any of that, and the crate is taken without its
//! network features for the same reason: nothing here talks to a remote.
//!
//! The repository is discovered from the scope root rather than assumed to be
//! it, so opening a subdirectory of a repository still works. Paths cross that
//! boundary in both directions — reported relative to the scope root like every
//! other tool, resolved relative to the repository for git itself.

use crate::domain::llm::LlmToolDefinition;
use std::fs;
use std::path::Path;

use chrono::{Local, TimeZone};
use git2::{BlameOptions, Repository, Status, StatusOptions};

use crate::domain::tools::{
    BlameHunk, FileDiffStats, GitBlameArgs, GitDiffArgs, GitFileDiff, GitFileStatus, GitLogArgs, GitUpstream,
    LogCommit, ToolError,
    ToolResult, ToolScope,
};
use crate::services::text_diff;

use super::super::resolve::{relative_to_root, resolve_existing, resolve_writable};

/// Cap on the lines one blame may cover. Past this the answer is a file dump
/// with commit hashes attached.
const MAX_BLAME_LINES: u32 = 400;
/// Cap on paths one status may carry across all three lists. A repository with
/// thousands of untracked files would otherwise bury the turn in one message.
const MAX_STATUS_ENTRIES: usize = 200;
/// Caps on a directory's diff: files, and the diff text across all of them.
/// A file past the text budget still shows its counts.
const MAX_DIFF_FILES: usize = 50;
pub(in crate::services::ai_tools) const MAX_DIFF_CHARS: usize = 20_000;
/// Files a `stat` diff of a directory lists: no text, so far more than
/// `MAX_DIFF_FILES` fit.
const MAX_STAT_FILES: usize = 500;
/// `gitLog` commits by default, and at most.
const DEFAULT_LOG_COMMITS: u32 = 20;
const MAX_LOG_COMMITS: u32 = 100;
/// Commits looked at before a filtered log gives up looking for more.
// ponytail: a diff per commit; a rare path in a long history stops here —
// an index of paths by commit if that shows up.
const MAX_LOG_SCANNED: usize = 5_000;

/// The working tree's changed paths.
///
/// The question `gitDiff` cannot answer: it takes a single file, so without
/// this the model has no way to find out *which* files changed, and falls back
/// to guessing paths.
pub fn git_status(scope: &ToolScope) -> Result<ToolResult, ToolError> {
    let repo = open(scope)?;
    let prefix = scope_prefix(scope, &repo)?;

    let mut options = StatusOptions::new();
    // Renames on the staged side: `git mv` otherwise reads as an addition and
    // an unrelated deletion, and the model loses that the file only moved.
    options
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .renames_head_to_index(true);
    let statuses = repo.statuses(Some(&mut options)).map_err(git_error)?;

    let (mut staged, mut unstaged, mut conflicted) = (Vec::new(), Vec::new(), Vec::new());
    for entry in statuses.iter() {
        let Ok(path) = entry.path() else { continue };
        // Dropped rather than rewritten: a change outside the scope root is
        // not this session's to report on.
        let Some(path) = path.strip_prefix(&prefix) else {
            continue;
        };
        let status = entry.status();
        // A rename shows where it came from: `old → new`, as git prints it.
        let rename = entry
            .head_to_index()
            .filter(|_| status.contains(Status::INDEX_RENAMED))
            .and_then(|delta| Some((in_scope(delta.old_file().path()?, &prefix)?, in_scope(delta.new_file().path()?, &prefix)?)));
        let shown = match rename {
            Some((old, new)) => format!("{old} → {new}"),
            None => path.to_string(),
        };
        let file = |letter: &str| GitFileStatus {
            path: shown.clone(),
            status: letter.to_string(),
        };

        if status.contains(Status::CONFLICTED) {
            conflicted.push(file("U"));
            continue;
        }
        if let Some(letter) = index_letter(status) {
            staged.push(file(letter));
        }
        if let Some(letter) = worktree_letter(status) {
            unstaged.push(file(letter));
        }
    }

    // Conflicts and staged work are the shorter, more decision-relevant lists,
    // so the cap eats into `unstaged` — which is what a repository full of
    // untracked files inflates — rather than trimming all three evenly.
    let kept = conflicted.len() + staged.len();
    let truncated = kept + unstaged.len() > MAX_STATUS_ENTRIES;
    unstaged.truncate(MAX_STATUS_ENTRIES.saturating_sub(kept));

    Ok(ToolResult::GitStatus {
        branch: repo.head().ok().and_then(|h| h.shorthand().ok().map(String::from)),
        upstream: upstream(&repo),
        staged,
        unstaged,
        conflicted,
        truncated,
    })
}

pub fn git_diff(scope: &ToolScope, args: &GitDiffArgs) -> Result<ToolResult, ToolError> {
    let repo = open(scope)?;
    let resolved = resolve_existing(scope, &args.path)?;
    let comparison = Comparison::of(&repo, args)?;
    let stat = args.stat == Some(true);

    if resolved.is_dir() {
        return directory_diff(scope, &repo, &comparison, &resolved, stat);
    }

    let repo_rel = repo_relative(scope, &repo, &resolved)?;
    let (mut diff, is_binary) = file_diff(&repo, &comparison, &repo_rel, &resolved)?;
    if stat {
        diff.unified_diff.clear();
        diff.truncated = false;
    }
    Ok(ToolResult::GitDiff {
        path: relative_to_root(scope, &resolved)?,
        label: label(&comparison, stat),
        diff,
        is_binary,
    })
}

/// What a diff compares: one commit against its parent, the index against
/// the working tree, or `HEAD` against the index.
enum Comparison<'r> {
    Commit(git2::Commit<'r>),
    Unstaged,
    Staged,
}

impl<'r> Comparison<'r> {
    fn of(repo: &'r Repository, args: &GitDiffArgs) -> Result<Self, ToolError> {
        if let Some(commit) = args.commit.as_deref().filter(|c| !c.is_empty()) {
            let object = repo.revparse_single(commit).map_err(git_error)?;
            return Ok(Comparison::Commit(object.peel_to_commit().map_err(git_error)?));
        }
        match args.scope.as_deref() {
            None | Some("unstaged") => Ok(Comparison::Unstaged),
            Some("staged") => Ok(Comparison::Staged),
            Some(other) => Err(ToolError::InvalidArguments {
                tool: "gitDiff".into(),
                reason: format!("scope must be \"unstaged\" or \"staged\" (got \"{other}\")"),
            }),
        }
    }

    fn label(&self) -> String {
        match self {
            Comparison::Commit(commit) => {
                let id = short(&commit.id().to_string());
                format!("{id}^ → {id}")
            }
            Comparison::Unstaged => "index → working tree".to_string(),
            Comparison::Staged => "HEAD → index".to_string(),
        }
    }
}

/// One file's two sides, diffed. `on_disk` is where the working-tree side is
/// read from.
fn file_diff(
    repo: &Repository,
    comparison: &Comparison,
    repo_rel: &str,
    on_disk: &Path,
) -> Result<(FileDiffStats, bool), ToolError> {
    let (original, modified) = match comparison {
        Comparison::Commit(commit) => {
            let after = tree_blob(repo, &commit.tree().map_err(git_error)?, repo_rel);
            let before = match commit.parent(0) {
                Ok(parent) => tree_blob(repo, &parent.tree().map_err(git_error)?, repo_rel),
                // A root commit has no parent: everything in it is new.
                Err(_) => Some(Vec::new()),
            };
            (before, after)
        }
        Comparison::Unstaged => (index_blob(repo, repo_rel), fs::read(on_disk).ok()),
        Comparison::Staged => (
            repo.head()
                .ok()
                .and_then(|h| h.peel_to_tree().ok())
                .and_then(|tree| tree_blob(repo, &tree, repo_rel)),
            index_blob(repo, repo_rel),
        ),
    };

    let original = original.unwrap_or_default();
    let modified = modified.unwrap_or_default();
    let is_binary = looks_binary(&original) || looks_binary(&modified);
    let diff = if is_binary {
        FileDiffStats::default()
    } else {
        text_diff::diff_stats(&String::from_utf8_lossy(&original), &String::from_utf8_lossy(&modified))
    };
    Ok((diff, is_binary))
}

/// Every changed file under a directory — the scope root being the whole
/// change. Each is diffed the way a single file is; what does not fit the
/// budget keeps its counts and loses its text, and the model can ask for
/// that file on its own.
/// What was compared, and whether only the totals were asked for — so a
/// header with no text under it is not read as an empty change.
fn label(comparison: &Comparison, stat: bool) -> String {
    let label = comparison.label();
    if stat { format!("{label}, totals only") } else { label }
}

fn directory_diff(
    scope: &ToolScope,
    repo: &Repository,
    comparison: &Comparison,
    dir: &Path,
    stat: bool,
) -> Result<ToolResult, ToolError> {
    let prefix = scope_prefix(scope, repo)?;
    let dir_rel = repo_relative(scope, repo, dir)?;
    // The scope root comes back as `.`, which as a pathspec matches nothing.
    let dir_rel = dir_rel.strip_suffix('.').filter(|rest| rest.is_empty() || rest.ends_with('/')).unwrap_or(&dir_rel);
    let dir_rel = dir_rel.trim_end_matches('/');

    let mut options = git2::DiffOptions::new();
    if !dir_rel.is_empty() {
        options.pathspec(dir_rel);
    }
    let changes = match comparison {
        Comparison::Commit(commit) => {
            let parent = commit.parent(0).ok().and_then(|p| p.tree().ok());
            let tree = commit.tree().map_err(git_error)?;
            repo.diff_tree_to_tree(parent.as_ref(), Some(&tree), Some(&mut options))
        }
        Comparison::Unstaged => {
            options.include_untracked(true).recurse_untracked_dirs(true);
            repo.diff_index_to_workdir(None, Some(&mut options))
        }
        Comparison::Staged => {
            let head = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
            repo.diff_tree_to_index(head.as_ref(), None, Some(&mut options))
        }
    }
    .map_err(git_error)?;

    let changed: Vec<String> = changes
        .deltas()
        .filter_map(|delta| {
            let file = delta.new_file().path().or_else(|| delta.old_file().path())?;
            Some(file.to_str()?.to_string())
        })
        .collect();

    let cap = if stat { MAX_STAT_FILES } else { MAX_DIFF_FILES };
    let truncated = changed.len() > cap;
    let workdir = repo.workdir().unwrap_or(scope.root());
    let mut budget = MAX_DIFF_CHARS;
    let mut files = Vec::new();
    for repo_rel in changed.into_iter().take(cap) {
        let (mut diff, is_binary) = file_diff(repo, comparison, &repo_rel, &workdir.join(&repo_rel))?;
        let size = diff.unified_diff.chars().count();
        if stat {
            diff.unified_diff.clear();
            diff.truncated = false;
        } else if size > budget {
            diff.unified_diff.clear();
            diff.truncated = true;
        } else {
            budget -= size;
        }
        let path = repo_rel.strip_prefix(&prefix).unwrap_or(&repo_rel).to_string();
        files.push(GitFileDiff { path, diff, is_binary });
    }

    Ok(ToolResult::GitDiffFiles {
        path: relative_to_root(scope, dir)?,
        label: label(comparison, stat),
        files,
        truncated,
    })
}

pub fn git_blame(scope: &ToolScope, args: &GitBlameArgs) -> Result<ToolResult, ToolError> {
    let repo = open(scope)?;
    let resolved = resolve_existing(scope, &args.path)?;
    if !resolved.is_file() {
        return Err(ToolError::NotAFile(args.path.clone()));
    }
    let repo_rel = repo_relative(scope, &repo, &resolved)?;

    let total = fs::read_to_string(&resolved)
        .map(|s| s.lines().count() as u32)
        .unwrap_or(0);
    let start = args.start_line.unwrap_or(1).max(1);
    let (end, truncated) = blame_range(start, args.end_line, total);

    let mut options = BlameOptions::new();
    options.min_line(start as usize).max_line(end as usize);
    let blame = repo
        .blame_file(Path::new(&repo_rel), Some(&mut options))
        .map_err(|e| blame_error(&args.path, e))?;

    let mut hunks = Vec::new();
    for hunk in blame.iter() {
        let commit = repo.find_commit(hunk.final_commit_id()).ok();
        let author = hunk
            .final_signature()
            .and_then(|s| s.name().ok().map(String::from))
            .unwrap_or_else(|| "unknown".to_string());
        hunks.push(BlameHunk {
            start_line: hunk.final_start_line() as u32,
            line_count: hunk.lines_in_hunk() as u32,
            commit: short(&hunk.final_commit_id().to_string()),
            author,
            date: commit
                .as_ref()
                .map(|c| format_time(c.time().seconds()))
                .unwrap_or_default(),
            summary: commit
                .as_ref()
                .and_then(|c| c.summary().ok().flatten().map(String::from))
                .unwrap_or_default(),
        });
    }

    Ok(ToolResult::GitBlame {
        path: relative_to_root(scope, &resolved)?,
        hunks,
        truncated,
    })
}

pub fn git_log(scope: &ToolScope, args: &GitLogArgs) -> Result<ToolResult, ToolError> {
    let repo = open(scope)?;
    // Resolved for containment only: a deleted file has a history too.
    let (shown, relative) = match args.path.as_deref() {
        Some(path) => {
            let relative = relative_to_root(scope, &resolve_writable(scope, path)?)?;
            (relative.clone(), if relative == "." { String::new() } else { relative })
        }
        None => (".".to_string(), String::new()),
    };
    // A folder inside the repository sees its own history, not the rest.
    let under = format!("{}{relative}", scope_prefix(scope, &repo)?);
    let under = under.trim_end_matches('/');
    let limit = args.limit.unwrap_or(DEFAULT_LOG_COMMITS).clamp(1, MAX_LOG_COMMITS) as usize;
    let query = args.query.as_deref().map(str::to_lowercase).filter(|q| !q.is_empty());

    let mut walk = repo.revwalk().map_err(git_error)?;
    if walk.push_head().is_err() {
        // No commit yet: an empty history, not an error.
        return Ok(ToolResult::GitLog { path: shown, commits: Vec::new(), truncated: false });
    }
    // Topological first: commits made within one second still come child
    // before parent.
    walk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::TIME).map_err(git_error)?;

    let mut commits = Vec::new();
    let mut truncated = false;
    for id in walk.take(MAX_LOG_SCANNED) {
        let commit = repo.find_commit(id.map_err(git_error)?).map_err(git_error)?;
        let message = commit.message().unwrap_or_default();
        if query.as_ref().is_some_and(|q| !message.to_lowercase().contains(q)) {
            continue;
        }
        // A merge's changes are the commits it brought in, listed on their
        // own; against its first parent it would repeat them under a path.
        if !under.is_empty() && commit.parent_count() > 1 {
            continue;
        }
        let parent = commit.parent(0).ok().and_then(|p| p.tree().ok());
        let mut options = git2::DiffOptions::new();
        if !under.is_empty() {
            options.pathspec(under);
        }
        let tree = commit.tree().map_err(git_error)?;
        let diff = repo.diff_tree_to_tree(parent.as_ref(), Some(&tree), Some(&mut options)).map_err(git_error)?;
        let files = diff.deltas().len() as u32;
        if files == 0 && !under.is_empty() {
            continue;
        }
        if commits.len() == limit {
            truncated = true;
            break;
        }
        commits.push(LogCommit {
            commit: short(&commit.id().to_string()),
            date: format_time(commit.time().seconds()),
            author: commit.author().name().ok().map(String::from).unwrap_or_else(|| "unknown".to_string()),
            summary: commit.summary().ok().flatten().unwrap_or_default().to_string(),
            files,
        });
    }
    Ok(ToolResult::GitLog { path: shown, commits, truncated })
}

/// Clamps the requested range to `MAX_BLAME_LINES`, reporting when it did.
fn blame_range(start: u32, end: Option<u32>, total: u32) -> (u32, bool) {
    let capped = start + MAX_BLAME_LINES - 1;
    match end {
        Some(end) => {
            let end = end.max(start);
            if end > capped {
                (capped, true)
            } else {
                (end, false)
            }
        }
        None if total == 0 => (capped, false),
        None if total > capped => (capped, true),
        None => (total.max(start), false),
    }
}

/// Discovers the repository containing the scope root, rather than assuming
/// the root is one — opening a subdirectory of a repository is ordinary.
fn open(scope: &ToolScope) -> Result<Repository, ToolError> {
    Repository::discover(scope.root())
        .map_err(|_| ToolError::Git(format!("{} is not inside a git repository", scope.root().display())))
}

/// The scope root's path relative to the repository, `/`-separated and ending
/// in `/`, or empty when they are the same directory.
fn scope_prefix(scope: &ToolScope, repo: &Repository) -> Result<String, ToolError> {
    let workdir = repo
        .workdir()
        .ok_or_else(|| ToolError::Git("a bare repository has no working tree".into()))?;
    // Plain, as the scope's root is: on Windows `canonicalize` adds `\\?\`.
    let workdir = crate::domain::tools::canonicalize_plain(workdir).map_err(ToolError::Io)?;
    match scope.root().strip_prefix(&workdir) {
        Ok(rel) if rel.as_os_str().is_empty() => Ok(String::new()),
        Ok(rel) => Ok(format!("{}/", rel.to_string_lossy().replace('\\', "/"))),
        Err(_) => Err(ToolError::Git(
            "the workspace root is outside the repository's working tree".into(),
        )),
    }
}

fn repo_relative(scope: &ToolScope, repo: &Repository, absolute: &Path) -> Result<String, ToolError> {
    Ok(format!(
        "{}{}",
        scope_prefix(scope, repo)?,
        relative_to_root(scope, absolute)?
    ))
}

fn index_blob(repo: &Repository, path: &str) -> Option<Vec<u8>> {
    let index = repo.index().ok()?;
    let entry = index.get_path(Path::new(path), 0)?;
    Some(repo.find_blob(entry.id).ok()?.content().to_vec())
}

fn tree_blob(repo: &Repository, tree: &git2::Tree, path: &str) -> Option<Vec<u8>> {
    let entry = tree.get_path(Path::new(path)).ok()?;
    Some(repo.find_blob(entry.id()).ok()?.content().to_vec())
}

/// The same first-block NUL sniff `grep` uses.
fn looks_binary(bytes: &[u8]) -> bool {
    bytes[..8192.min(bytes.len())].contains(&0)
}

fn short(hash: &str) -> String {
    hash.chars().take(8).collect()
}

fn format_time(seconds: i64) -> String {
    match Local.timestamp_opt(seconds, 0) {
        chrono::LocalResult::Single(time) => time.format("%Y-%m-%d").to_string(),
        _ => String::new(),
    }
}

/// A path from a git delta, relative to the scope root; `None` outside it.
fn in_scope<'a>(path: &'a Path, prefix: &str) -> Option<&'a str> {
    path.to_str()?.strip_prefix(prefix)
}

/// The tracking branch of the checked-out one, with the commit counts
/// between them. `None` when detached, unborn or tracking nothing.
fn upstream(repo: &Repository) -> Option<GitUpstream> {
    let head = repo.head().ok().filter(|h| h.is_branch())?;
    let local = head.target()?;
    let tracked = git2::Branch::wrap(head).upstream().ok()?;
    let name = tracked.name().ok()??.to_string();
    let (ahead, behind) = repo.graph_ahead_behind(local, tracked.get().target()?).ok()?;
    // git rewrites FETCH_HEAD on every fetch; a clone that never fetched has none.
    let fetched = fs::metadata(repo.path().join("FETCH_HEAD"))
        .and_then(|meta| meta.modified())
        .ok()
        .map(|time| chrono::DateTime::<Local>::from(time).format("%Y-%m-%d %H:%M").to_string());
    Some(GitUpstream { name, ahead, behind, fetched })
}

fn index_letter(status: Status) -> Option<&'static str> {
    if status.contains(Status::INDEX_NEW) {
        Some("A")
    } else if status.contains(Status::INDEX_DELETED) {
        Some("D")
    } else if status.contains(Status::INDEX_RENAMED) {
        Some("R")
    } else if status.intersects(Status::INDEX_MODIFIED | Status::INDEX_TYPECHANGE) {
        Some("M")
    } else {
        None
    }
}

fn worktree_letter(status: Status) -> Option<&'static str> {
    if status.contains(Status::WT_NEW) {
        Some("?")
    } else if status.contains(Status::WT_DELETED) {
        Some("D")
    } else if status.contains(Status::WT_RENAMED) {
        Some("R")
    } else if status.intersects(Status::WT_MODIFIED | Status::WT_TYPECHANGE) {
        Some("M")
    } else {
        None
    }
}

fn git_error(err: git2::Error) -> ToolError {
    ToolError::Git(err.message().to_string())
}

/// libgit2 answers a blame on a never-committed file with "the path '…' does
/// not exist in the given tree" — true about the tree, and useless here:
/// nothing in it says the file is simply new, so the natural reading is that
/// the path was wrong.
fn blame_error(path: &str, err: git2::Error) -> ToolError {
    if err.message().contains("does not exist in the given tree") {
        return ToolError::Git(format!(
            "{path} has no history yet — it is not committed, so it cannot be blamed. gitDiff shows its whole current content."
        ));
    }
    git_error(err)
}

/// What the model is told `gitStatus` is for.
pub(super) fn status_definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "gitStatus".to_string(),
        description: "What has changed in the working tree: modified, added, deleted, renamed, conflicted and untracked paths, plus the current branch. Read-only — this tool never stages, commits or pushes anything."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {},
            "required": []
        }),
    }
}

/// What the model is told `gitDiff` is for.
pub(super) fn diff_definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "gitDiff".to_string(),
        description: "The diff for a file, or for every changed file under a directory (\\\".\\\" for the whole change). Read-only. Use it to see your own uncommitted work, or what a particular commit did — a commit with path \\\".\\\" is everything it changed."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File or directory relative to the workspace root."
                },
                "scope": {
                    "type": [
                        "string",
                        "null"
                    ],
                    "enum": [
                        "unstaged",
                        "staged",
                        null
                    ],
                    "description": "\\\"unstaged\\\" (default) is the working tree against the index; \\\"staged\\\" is the index against HEAD. Ignored when `commit` is given."
                },
                "commit": {
                    "type": [
                        "string",
                        "null"
                    ],
                    "description": "A commit hash or ref. When given, diffs that commit against its parent."
                },
                "stat": {
                    "type": ["boolean", "null"],
                    "description": "Only how many lines each file gained and lost, no text — like git diff --stat. Cheap on a large change: look first, then ask for the files that matter."
                }
            },
            "required": [
                "path"
            ]
        }),
    }
}

/// What the model is told `gitLog` is for.
pub(super) fn log_definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "gitLog".to_string(),
        description: format!("Commit history, newest first: hash, date, author, the message's first line and how many files each changed. Read-only. Narrow it to a file or directory to see what changed there and when, or by words in the message. Read a commit's change with gitDiff and its `commit`. Renames are not followed: for a file that was moved, ask again with its old path. {DEFAULT_LOG_COMMITS} commits unless `limit` says otherwise, at most {MAX_LOG_COMMITS}."),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": ["string", "null"],
                    "description": "File or directory relative to the workspace root; only commits that changed something under it. It may be one that no longer exists. Omit for the whole workspace."
                },
                "limit": {
                    "type": ["integer", "null"],
                    "minimum": 1,
                    "maximum": MAX_LOG_COMMITS,
                    "description": "How many commits to return."
                },
                "query": {
                    "type": ["string", "null"],
                    "description": "Only commits whose message contains this, ignoring case — a ticket number, a word from the change."
                }
            },
            "required": []
        }),
    }
}

/// What the model is told `gitBlame` is for.
pub(super) fn blame_definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "gitBlame".to_string(),
        description: "Who last changed each line of a file, and in which commit. Read-only. Use it to find the change that introduced a line, and the message explaining why — then read that commit with gitDiff."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path relative to the workspace root."
                },
                "startLine": {
                    "type": [
                        "integer",
                        "null"
                    ],
                    "minimum": 1,
                    "description": "1-indexed first line, inclusive. Omit for the whole file — prefer a range, blame of a large file is long."
                },
                "endLine": {
                    "type": [
                        "integer",
                        "null"
                    ],
                    "minimum": 1,
                    "description": "1-indexed last line, inclusive."
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
    use crate::domain::tools::ToolCall;
    use crate::testing::temp_dir;
    use git2::{IndexAddOption, Signature};
    use std::path::PathBuf;

    /// A repository with one commit, and a scope over its working tree.
    fn repo_fixture(label: &str) -> (ToolScope, PathBuf, Repository) {
        let dir = temp_dir(label);
        let repo = Repository::init(&dir).expect("repository is creatable");
        write(&dir, "tracked.txt", "one\ntwo\n");
        commit(&repo, "initial commit");
        let scope = ToolScope::new(&dir).expect("root resolves");
        let root = scope.root().to_path_buf();
        (scope, root, repo)
    }

    fn write(root: &Path, relative: &str, body: &str) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().expect("has a parent")).expect("dirs are creatable");
        std::fs::write(path, body).expect("file is writable");
    }

    fn stage_all(repo: &Repository) {
        let mut index = repo.index().expect("index");
        index
            .add_all(["*"], IndexAddOption::DEFAULT, None)
            .expect("staged");
        index.write().expect("index written");
    }

    fn commit(repo: &Repository, message: &str) {
        stage_all(repo);
        let mut index = repo.index().expect("index");
        let tree = repo
            .find_tree(index.write_tree().expect("tree"))
            .expect("tree object");
        let who = Signature::now("Test Person", "test@example.com").expect("signature");
        let parents: Vec<git2::Commit> = repo
            .head()
            .ok()
            .and_then(|h| h.peel_to_commit().ok())
            .into_iter()
            .collect();
        let parent_refs: Vec<&git2::Commit> = parents.iter().collect();
        repo.commit(Some("HEAD"), &who, &who, message, &tree, &parent_refs)
            .expect("commit");
    }

    fn status_of(scope: &ToolScope) -> (Vec<GitFileStatus>, Vec<GitFileStatus>, Vec<GitFileStatus>, bool) {
        match git_status(scope).expect("status runs") {
            ToolResult::GitStatus {
                staged,
                unstaged,
                conflicted,
                truncated,
                ..
            } => (staged, unstaged, conflicted, truncated),
            other => panic!("unexpected {other:?}"),
        }
    }

    fn entries(files: &[GitFileStatus]) -> Vec<(&str, &str)> {
        files
            .iter()
            .map(|f| (f.path.as_str(), f.status.as_str()))
            .collect()
    }

    #[test]
    fn status_separates_staged_untracked_and_modified() {
        let (scope, root, repo) = repo_fixture("git-status");
        write(&root, "tracked.txt", "one\nCHANGED\n");
        write(&root, "brand-new.txt", "fresh\n");
        write(&root, "staged.txt", "staged\n");
        {
            let mut index = repo.index().expect("index");
            index
                .add_path(Path::new("staged.txt"))
                .expect("stage one file");
            index.write().expect("written");
        }

        let (staged, unstaged, conflicted, truncated) = status_of(&scope);

        assert_eq!(entries(&staged), [("staged.txt", "A")]);
        assert_eq!(
            entries(&unstaged),
            [("brand-new.txt", "?"), ("tracked.txt", "M")]
        );
        assert!(conflicted.is_empty());
        assert!(!truncated);
    }

    /// `git mv` is one move, not an addition and an unrelated deletion.
    #[test]
    fn a_staged_rename_is_one_entry_from_old_to_new() {
        let (scope, root, repo) = repo_fixture("git-rename");
        std::fs::rename(root.join("tracked.txt"), root.join("moved.txt")).expect("renamed");
        {
            let mut index = repo.index().expect("index");
            index.remove_path(Path::new("tracked.txt")).expect("old path unstaged");
            index.add_path(Path::new("moved.txt")).expect("new path staged");
            index.write().expect("written");
        }

        let (staged, unstaged, _, _) = status_of(&scope);

        assert_eq!(entries(&staged), [("tracked.txt → moved.txt", "R")]);
        assert!(unstaged.is_empty(), "{unstaged:?}");
    }

    fn log(scope: &ToolScope, args: GitLogArgs) -> (Vec<(String, u32)>, bool) {
        let ToolResult::GitLog { commits, truncated, .. } = git_log(scope, &args).expect("log runs") else {
            panic!("expected a log")
        };
        (commits.into_iter().map(|c| (c.summary, c.files)).collect(), truncated)
    }

    fn logged(summaries: &[(&str, u32)]) -> Vec<(String, u32)> {
        summaries.iter().map(|(s, n)| (s.to_string(), *n)).collect()
    }

    /// Newest first, each with how many files it changed; the limit says
    /// when it cut.
    #[test]
    fn log_lists_commits_newest_first_and_says_when_it_cut() {
        let (scope, root, repo) = repo_fixture("git-log");
        write(&root, "a.rs", "a\n");
        write(&root, "b.rs", "b\n");
        commit(&repo, "add two\n\nwith a body");

        let (commits, truncated) = log(&scope, GitLogArgs::default());
        assert_eq!(commits, logged(&[("add two", 2), ("initial commit", 1)]));
        assert!(!truncated);

        let (commits, truncated) = log(&scope, GitLogArgs { limit: Some(1), ..GitLogArgs::default() });
        assert_eq!(commits, logged(&[("add two", 2)]));
        assert!(truncated);
        let (_, truncated) = log(&scope, GitLogArgs { limit: Some(2), ..GitLogArgs::default() });
        assert!(!truncated, "exactly the limit is not a cut");
        let (commits, _) = log(&scope, GitLogArgs { limit: Some(0), ..GitLogArgs::default() });
        assert_eq!(commits.len(), 1, "a limit of nothing is one");
    }

    /// A clock set back makes a child older than its parent; it is still
    /// the newer change.
    #[test]
    fn log_puts_a_child_before_its_parent_whatever_the_clock_says() {
        let (scope, root, repo) = repo_fixture("git-log-skew");
        write(&root, "a.rs", "a\n");
        stage_all(&repo);
        let tree = repo.find_tree(repo.index().unwrap().write_tree().unwrap()).unwrap();
        let parent = repo.head().unwrap().peel_to_commit().unwrap();
        let past = Signature::new("Test Person", "test@example.com", &git2::Time::new(1_000, 0)).unwrap();
        repo.commit(Some("HEAD"), &past, &past, "from a skewed clock", &tree, &[&parent]).unwrap();

        assert_eq!(log(&scope, GitLogArgs::default()).0, logged(&[("from a skewed clock", 1), ("initial commit", 1)]));
    }

    /// A path keeps the commits that changed it — a deleted file included —
    /// and counts only its own files; a query keeps the messages that say it.
    #[test]
    fn log_narrows_by_path_and_by_message() {
        let (scope, root, repo) = repo_fixture("git-log-path");
        write(&root, "src/a.rs", "a\n");
        write(&root, "b.rs", "b\n");
        commit(&repo, "Fix TICKET-7 in both");
        write(&root, "b.rs", "b2\n");
        commit(&repo, "only b");
        std::fs::remove_file(root.join("src/a.rs")).unwrap();
        let mut index = repo.index().unwrap();
        index.remove_path(Path::new("src/a.rs")).unwrap();
        index.write().unwrap();
        commit(&repo, "drop a");

        let under = |path: &str| log(&scope, GitLogArgs { path: Some(path.into()), ..GitLogArgs::default() }).0;
        assert_eq!(under("src"), logged(&[("drop a", 1), ("Fix TICKET-7 in both", 1)]));
        assert_eq!(under("src/a.rs"), under("src"), "a file that is gone has a history");
        assert_eq!(under("."), log(&scope, GitLogArgs::default()).0);

        let (commits, _) = log(&scope, GitLogArgs { query: Some("Ticket-7".into()), ..GitLogArgs::default() });
        assert_eq!(commits, logged(&[("Fix TICKET-7 in both", 2)]));
        assert!(git_log(&scope, &GitLogArgs { path: Some("../x".into()), ..GitLogArgs::default() }).is_err());
    }

    /// A workspace inside a repository has the history of its own files.
    #[test]
    fn log_of_a_subfolder_is_its_own_history() {
        let dir = temp_dir("git-log-sub");
        let repo = Repository::init(&dir).expect("repository");
        write(&dir, "docs/guide.md", "hello\n");
        commit(&repo, "guide");
        write(&dir, "outside.txt", "x\n");
        commit(&repo, "outside");
        let scope = ToolScope::new(&dir.join("docs")).expect("root resolves");

        assert_eq!(log(&scope, GitLogArgs::default()).0, logged(&[("guide", 1)]));
        let ToolResult::GitLog { path, .. } =
            git_log(&scope, &GitLogArgs { path: Some("guide.md".into()), ..GitLogArgs::default() }).unwrap()
        else {
            panic!()
        };
        assert_eq!(path, "guide.md", "relative to the workspace, not the repository");
    }

    /// Under a path a merge would repeat what its branch already listed.
    #[test]
    fn log_under_a_path_leaves_merges_out() {
        let (scope, root, repo) = repo_fixture("git-log-merge");
        let base = repo.head().unwrap().peel_to_commit().unwrap();
        write(&root, "b.rs", "b\n");
        stage_all(&repo);
        let tree = repo.find_tree(repo.index().unwrap().write_tree().unwrap()).unwrap();
        let who = Signature::now("Test Person", "test@example.com").unwrap();
        let side = repo.find_commit(repo.commit(None, &who, &who, "side", &tree, &[&base]).unwrap()).unwrap();
        repo.commit(Some("HEAD"), &who, &who, "merge side", &tree, &[&base, &side]).unwrap();

        let under = log(&scope, GitLogArgs { path: Some("b.rs".into()), ..GitLogArgs::default() }).0;
        assert_eq!(under, logged(&[("side", 1)]));
        let all = log(&scope, GitLogArgs::default()).0;
        assert_eq!(all[0], ("merge side".to_string(), 1), "the whole log keeps it");
    }

    #[test]
    fn log_of_a_repository_with_no_commits_is_empty() {
        let dir = temp_dir("git-log-empty");
        Repository::init(&dir).unwrap();
        let scope = ToolScope::new(&dir).unwrap();
        assert_eq!(log(&scope, GitLogArgs::default()), (vec![], false));
    }

    /// Counted against the remote-tracking branch as it is locally; a branch
    /// tracking nothing has no upstream to be ahead of.
    #[test]
    fn status_counts_commits_apart_from_the_upstream() {
        let (scope, root, repo) = repo_fixture("git-upstream");
        let ToolResult::GitStatus { upstream, .. } = git_status(&scope).unwrap() else { panic!() };
        assert_eq!(upstream, None);

        let base = repo.head().unwrap().target().unwrap();
        repo.reference("refs/remotes/origin/main", base, true, "fetched").unwrap();
        repo.remote("origin", "https://example.invalid/repo.git").unwrap();
        git2::Branch::wrap(repo.head().unwrap()).set_upstream(Some("origin/main")).unwrap();
        write(&root, "tracked.txt", "changed\n");
        commit(&repo, "one more");
        write(&root, "tracked.txt", "again\n");
        commit(&repo, "and another");

        let ToolResult::GitStatus { upstream, .. } = git_status(&scope).unwrap() else { panic!() };
        assert_eq!(upstream, Some(GitUpstream { name: "origin/main".into(), ahead: 2, behind: 0, fetched: None }));
        std::fs::write(repo.path().join("FETCH_HEAD"), "").unwrap();
        let ToolResult::GitStatus { upstream, .. } = git_status(&scope).unwrap() else { panic!() };
        let fetched = upstream.and_then(|u| u.fetched).expect("a fetch recorded");
        assert!(fetched.starts_with(&Local::now().format("%Y-%m-%d").to_string()), "{fetched}");
    }

    #[test]
    fn status_reports_the_branch() {
        let (scope, _, _repo) = repo_fixture("git-branch");
        let ToolResult::GitStatus { branch, .. } = git_status(&scope).unwrap() else {
            panic!("wrong result")
        };
        assert!(branch.is_some(), "a repository with a commit has a branch");
    }

    /// The cap eats into untracked noise rather than trimming the short,
    /// decision-relevant lists evenly.
    #[test]
    fn the_status_cap_protects_staged_entries() {
        let (scope, root, repo) = repo_fixture("git-status-cap");
        for i in 0..(MAX_STATUS_ENTRIES + 50) {
            write(&root, &format!("noise/f{i}.txt"), "x");
        }
        write(&root, "important.txt", "x");
        {
            let mut index = repo.index().expect("index");
            index.add_path(Path::new("important.txt")).expect("staged");
            index.write().expect("written");
        }

        let (staged, unstaged, _, truncated) = status_of(&scope);

        assert_eq!(entries(&staged), [("important.txt", "A")]);
        assert!(truncated);
        assert_eq!(staged.len() + unstaged.len(), MAX_STATUS_ENTRIES);
    }

    #[test]
    fn unstaged_diff_compares_the_index_with_the_working_tree() {
        let (scope, root, _repo) = repo_fixture("git-diff-unstaged");
        write(&root, "tracked.txt", "one\nTWO\n");

        let ToolResult::GitDiff { label, diff, .. } = git_diff(
            &scope,
            &GitDiffArgs {
                path: "tracked.txt".into(),
                ..GitDiffArgs::default()
            },
        )
        .unwrap() else {
            panic!("wrong result")
        };

        assert_eq!(label, "index → working tree");
        assert_eq!((diff.lines_added, diff.lines_removed), (1, 1));
        assert!(diff.unified_diff.contains("+TWO"));
    }

    #[test]
    fn staged_diff_compares_head_with_the_index() {
        let (scope, root, repo) = repo_fixture("git-diff-staged");
        write(&root, "tracked.txt", "one\nTWO\n");
        stage_all(&repo);

        let ToolResult::GitDiff { label, diff, .. } = git_diff(
            &scope,
            &GitDiffArgs {
                path: "tracked.txt".into(),
                scope: Some("staged".into()),
                ..GitDiffArgs::default()
            },
        )
        .unwrap() else {
            panic!("wrong result")
        };

        assert_eq!(label, "HEAD → index");
        assert_eq!((diff.lines_added, diff.lines_removed), (1, 1));
    }

    fn diff_files(scope: &ToolScope, args: GitDiffArgs) -> Vec<(String, u32, u32, bool)> {
        let ToolResult::GitDiffFiles { mut files, .. } = git_diff(scope, &args).expect("diff runs") else {
            panic!("not a directory diff")
        };
        files.sort_by(|a, b| a.path.cmp(&b.path));
        files
            .into_iter()
            .map(|f| (f.path, f.diff.lines_added, f.diff.lines_removed, f.diff.unified_diff.is_empty()))
            .collect()
    }

    /// A directory is every changed file under it — untracked ones included —
    /// and nothing outside it; `.` is the whole change.
    #[test]
    fn a_directory_diff_covers_each_changed_file_under_it() {
        let (scope, root, repo) = repo_fixture("git-diff-dir");
        write(&root, "sub/a.txt", "a\n");
        commit(&repo, "add sub");
        write(&root, "sub/a.txt", "A\n");
        write(&root, "sub/new.txt", "n\n");
        write(&root, "tracked.txt", "one\nTWO\n");

        let sub = diff_files(&scope, GitDiffArgs { path: "sub".into(), ..GitDiffArgs::default() });
        assert_eq!(sub, [("sub/a.txt".into(), 1, 1, false), ("sub/new.txt".into(), 1, 0, false)]);

        let all = diff_files(&scope, GitDiffArgs { path: ".".into(), ..GitDiffArgs::default() });
        assert_eq!(all.iter().map(|f| f.0.as_str()).collect::<Vec<_>>(), ["sub/a.txt", "sub/new.txt", "tracked.txt"]);
    }

    /// What one commit did, across files, in one call.
    #[test]
    fn a_whole_commit_is_a_directory_diff_of_the_root() {
        let (scope, root, repo) = repo_fixture("git-diff-commit-dir");
        write(&root, "sub/a.txt", "a\n");
        write(&root, "sub/b.txt", "b\n");
        commit(&repo, "two files");

        let files = diff_files(&scope, GitDiffArgs { path: ".".into(), commit: Some("HEAD".into()), ..GitDiffArgs::default() });

        assert_eq!(files, [("sub/a.txt".into(), 1, 0, false), ("sub/b.txt".into(), 1, 0, false)]);
    }

    /// Past the file cap the rest is left out, and the result says so.
    #[test]
    fn past_the_file_cap_the_diff_says_it_left_some_out() {
        let (scope, root, _repo) = repo_fixture("git-diff-cap");
        for n in 0..=MAX_DIFF_FILES {
            write(&root, &format!("many/{n:03}.txt"), "x\n");
        }

        let ToolResult::GitDiffFiles { files, truncated, .. } =
            git_diff(&scope, &GitDiffArgs { path: "many".into(), ..GitDiffArgs::default() }).expect("diff runs")
        else {
            panic!("not a directory diff")
        };

        assert_eq!(files.len(), MAX_DIFF_FILES);
        assert!(truncated);
    }

    /// Totals only: every file's counts and no text, far past the text cap,
    /// and a label that says so — or the empty diffs read as no change.
    #[test]
    fn a_stat_diff_has_the_counts_without_the_text() {
        let (scope, root, _repo) = repo_fixture("git-diff-stat");
        for n in 0..=MAX_DIFF_FILES {
            write(&root, &format!("many/{n:03}.txt"), "x\ny\n");
        }
        write(&root, "tracked.txt", "one\nchanged\n");
        let stat = GitDiffArgs { path: "many".into(), stat: Some(true), ..GitDiffArgs::default() };

        let ToolResult::GitDiffFiles { files, truncated, label, .. } = git_diff(&scope, &stat).unwrap() else { panic!() };

        assert_eq!((files.len(), truncated), (MAX_DIFF_FILES + 1, false));
        assert!(files.iter().all(|f| f.diff.unified_diff.is_empty() && !f.diff.truncated && f.diff.lines_added == 2));
        assert!(label.ends_with(", totals only"), "{label}");

        let one = GitDiffArgs { path: "tracked.txt".into(), stat: Some(true), ..GitDiffArgs::default() };
        let ToolResult::GitDiff { diff, label, .. } = git_diff(&scope, &one).unwrap() else { panic!() };
        assert_eq!((diff.unified_diff.as_str(), diff.lines_added, diff.lines_removed), ("", 1, 1));
        assert!(label.ends_with(", totals only"));
    }

    /// Past the budget a file keeps its counts and loses its text, so the
    /// answer stays one message and says what it left out.
    #[test]
    fn past_the_budget_a_file_keeps_its_counts_but_not_its_text() {
        let (scope, root, _repo) = repo_fixture("git-diff-budget");
        let big = "x\n".repeat(5000);
        for name in ["a", "b", "c", "d"] {
            write(&root, &format!("big/{name}.txt"), &big);
        }

        let files = diff_files(&scope, GitDiffArgs { path: "big".into(), ..GitDiffArgs::default() });

        assert_eq!(files.len(), 4);
        assert!(files.iter().all(|f| f.1 == 5000), "every count is exact: {files:?}");
        assert_eq!(files.iter().filter(|f| f.3).count(), 1, "one diff did not fit: {files:?}");
    }

    #[test]
    fn an_unknown_diff_scope_is_refused() {
        let (scope, _, _repo) = repo_fixture("git-diff-scope");
        assert!(matches!(
            git_diff(
                &scope,
                &GitDiffArgs {
                    path: "tracked.txt".into(),
                    scope: Some("sideways".into()),
                    ..GitDiffArgs::default()
                }
            ),
            Err(ToolError::InvalidArguments { .. })
        ));
    }

    #[test]
    fn blame_reports_who_wrote_each_line() {
        let (scope, _, _repo) = repo_fixture("git-blame");

        let ToolResult::GitBlame { hunks, truncated, .. } = git_blame(
            &scope,
            &GitBlameArgs {
                path: "tracked.txt".into(),
                ..GitBlameArgs::default()
            },
        )
        .unwrap() else {
            panic!("wrong result")
        };

        assert!(!hunks.is_empty());
        assert_eq!(hunks[0].author, "Test Person");
        assert_eq!(hunks[0].summary, "initial commit");
        assert_eq!(hunks[0].commit.len(), 8, "short hash");
        assert!(!hunks[0].date.is_empty());
        assert!(!truncated);
    }

    /// libgit2's own message is about a tree, and reads as "your path was
    /// wrong" — which sends the model looking for a typo that is not there.
    #[test]
    fn blaming_an_uncommitted_file_explains_why_there_is_no_history() {
        let (scope, root, _repo) = repo_fixture("git-blame-new");
        write(&root, "brand-new.txt", "fresh\n");

        let err = git_blame(
            &scope,
            &GitBlameArgs {
                path: "brand-new.txt".into(),
                ..GitBlameArgs::default()
            },
        )
        .expect_err("no history");

        let message = err.to_string();
        assert!(message.contains("not committed"), "{message}");
        assert!(message.contains("gitDiff"), "names the alternative: {message}");
    }

    #[test]
    fn a_blame_range_is_clamped_and_says_when_it_was() {
        assert_eq!(blame_range(1, Some(50), 100), (50, false));
        assert_eq!(blame_range(1, Some(5000), 5000), (MAX_BLAME_LINES, true));
        assert_eq!(blame_range(1, None, 10), (10, false));
        assert_eq!(blame_range(1, None, 5000), (MAX_BLAME_LINES, true));
        assert_eq!(blame_range(10, Some(2), 100), (10, false), "inverted collapses");
    }

    #[test]
    fn a_directory_that_is_not_a_repository_says_so() {
        let dir = temp_dir("git-none");
        let scope = ToolScope::new(&dir).expect("root resolves");

        let err = git_status(&scope).expect_err("not a repository");

        assert!(err.to_string().contains("not inside a git repository"), "{err}");
    }

    /// Opening a subdirectory of a repository is ordinary, and paths still have
    /// to come back in the spelling `readFile` accepts.
    #[test]
    fn a_scope_inside_a_repository_reports_paths_relative_to_itself() {
        let dir = temp_dir("git-subdir");
        let repo = Repository::init(&dir).expect("repository");
        write(&dir, "docs/guide.md", "hello\n");
        write(&dir, "outside.txt", "not in scope\n");
        commit(&repo, "initial commit");
        write(&dir, "docs/guide.md", "hello\nmore\n");
        write(&dir, "outside.txt", "changed too\n");

        let scope = ToolScope::new(&dir.join("docs")).expect("root resolves");
        let (_, unstaged, _, _) = status_of(&scope);

        assert_eq!(
            entries(&unstaged),
            [("guide.md", "M")],
            "rebased onto the scope, and the change outside it dropped"
        );
    }

    #[test]
    fn the_call_wire_shapes_are_stable_and_read_only() {
        let status: ToolCall =
            serde_json::from_str(r#"{"tool": "gitStatus"}"#).expect("parses with no args");
        assert_eq!(status, ToolCall::GitStatus);
        assert!(!status.is_risky());

        let blame: ToolCall = serde_json::from_str(
            r#"{"tool": "gitBlame", "args": {"path": "a.txt", "startLine": "3"}}"#,
        )
        .expect("parses");
        let ToolCall::GitBlame(parsed) = &blame else {
            panic!("wrong variant")
        };
        assert_eq!(parsed.start_line, Some(3));
        assert!(!blame.is_risky());
    }
}
