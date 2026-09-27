//! Staging and committing in the open folder's repository — the Changes
//! panel's four operations, over `git2`.
//!
//! The repository is discovered from the folder, so a folder inside a
//! repository works. Paths go both ways relative to the repository root.

use std::collections::{BTreeMap, HashMap};
use std::path::{Component, Path};

use git2::{Diff, DiffFormat, DiffOptions, ErrorCode, Repository, Signature};

use crate::domain::commit_message::StagedPatch;
use crate::domain::review::{FileDiff, FileStatus, NewLine};
use crate::domain::git_changes::{
    ChangeTotals, ChangedFile, CommitSummary, FileSide, FileView, GitChangesError, GitHistory, Unviewable, WorkingChanges,
};

pub fn changes(root: &Path) -> Result<WorkingChanges, GitChangesError> {
    let repo = open(root)?;
    let head = head_tree(&repo)?;
    let index = repo.index().map_err(git)?;

    let staged = repo.diff_tree_to_index(head.as_ref(), Some(&index), None).map_err(git)?;
    // Untracked content implies untracked files; a new folder is listed file
    // by file rather than as one entry.
    let mut options = DiffOptions::new();
    options.show_untracked_content(true).recurse_untracked_dirs(true);
    let unstaged = repo.diff_index_to_workdir(Some(&index), Some(&mut options)).map_err(git)?;

    Ok(WorkingChanges {
        staged: files(&staged)?,
        unstaged: files(&unstaged)?,
    })
}

/// What the working tree holds against HEAD, the index in between — so an
/// edit staged and then edited again counts once — new files included. Only
/// the open folder: a subfolder of a repository counts its own changes.
pub fn totals(root: &Path) -> Result<ChangeTotals, GitChangesError> {
    let repo = open(root)?;
    let head = head_tree(&repo)?;
    let mut options = DiffOptions::new();
    options.show_untracked_content(true).recurse_untracked_dirs(true);
    if let Some(folder) = folder_in_repo(&repo, root).filter(|folder| !folder.as_os_str().is_empty()) {
        options.pathspec(folder);
    }
    let diff = repo.diff_tree_to_workdir_with_index(head.as_ref(), Some(&mut options)).map_err(git)?;
    let stats = diff.stats().map_err(git)?;
    Ok(ChangeTotals { files: stats.files_changed(), add: stats.insertions(), del: stats.deletions() })
}

/// Unchanged lines shown on each side of a change. The agent reads more with
/// its tools where it needs to.
pub const REVIEW_CONTEXT_LINES: u32 = 10;

/// What a review reads: the working tree against HEAD, the index in between,
/// new files included, renames found — each file with its patch from the
/// first hunk on and the new side of every hunk. Paths are relative to the
/// open folder, as the agent's tools name them, and only its changes count.
pub fn review_diffs(root: &Path) -> Result<Vec<FileDiff>, GitChangesError> {
    let repo = open(root)?;
    let head = head_tree(&repo)?;
    let folder = folder_in_repo(&repo, root).filter(|folder| !folder.as_os_str().is_empty());
    let mut options = DiffOptions::new();
    options.show_untracked_content(true).recurse_untracked_dirs(true).context_lines(REVIEW_CONTEXT_LINES);
    if let Some(folder) = &folder {
        options.pathspec(folder);
    }
    let mut diff = repo.diff_tree_to_workdir_with_index(head.as_ref(), Some(&mut options)).map_err(git)?;
    diff.find_similar(Some(git2::DiffFindOptions::new().renames(true))).map_err(git)?;
    let prefix = folder.as_ref().map(|f| format!("{}/", f.to_string_lossy().replace('\\', "/"))).unwrap_or_default();
    let shown = |path: Option<&Path>| {
        let path = path.map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_default();
        path.strip_prefix(&prefix).map(String::from).unwrap_or(path)
    };

    let mut files = Vec::new();
    for index in 0..diff.deltas().len() {
        let Some((patch, lines)) = review_patch(&diff, index)? else { continue };
        let Some(delta) = diff.get_delta(index) else { continue };
        let path = shown(delta.new_file().path().or_else(|| delta.old_file().path()));
        let status = match delta.status() {
            git2::Delta::Added | git2::Delta::Untracked => FileStatus::Added,
            git2::Delta::Deleted => FileStatus::Deleted,
            git2::Delta::Renamed => FileStatus::Renamed { from: shown(delta.old_file().path()) },
            _ => FileStatus::Modified,
        };
        files.push(FileDiff { path, status, binary: delta.flags().is_binary(), patch, lines });
    }
    Ok(files)
}

/// One file of `diff`: its hunks as text, without the file's own header,
/// and the new side of every hunk.
fn review_patch(diff: &Diff, index: usize) -> Result<Option<(String, Vec<NewLine>)>, GitChangesError> {
    let Some(mut patch) = git2::Patch::from_diff(diff, index).map_err(git)? else { return Ok(None) };
    let mut lines = Vec::new();
    for hunk in 0..patch.num_hunks() {
        for at in 0..patch.num_lines_in_hunk(hunk).map_err(git)? {
            let line = patch.line_in_hunk(hunk, at).map_err(git)?;
            if let (origin @ ('+' | ' '), Some(number)) = (line.origin(), line.new_lineno()) {
                lines.push(NewLine {
                    hunk: hunk as u32,
                    number,
                    added: origin == '+',
                    text: String::from_utf8_lossy(line.content()).trim_end_matches(['\n', '\r']).to_string(),
                });
            }
        }
    }
    let text = patch.to_buf().map_err(git)?;
    let text = String::from_utf8_lossy(&text);
    // The file's own header is the prompt's; the hunks are the diff.
    Ok(Some((text.find("\n@@").map_or(String::new(), |at| text[at + 1..].to_string()), lines)))
}

/// A path that is gone from disk stages its deletion.
pub fn stage(root: &Path, paths: &[String]) -> Result<(), GitChangesError> {
    let repo = open(root)?;
    let workdir = repo.workdir().ok_or(GitChangesError::NotARepository)?.to_path_buf();
    let mut index = repo.index().map_err(git)?;
    for path in paths {
        let rel = relative(path)?;
        if workdir.join(rel).exists() {
            index.add_path(rel).map_err(git)?;
        } else {
            index.remove_path(rel).map_err(git)?;
        }
    }
    index.write().map_err(git)
}

pub fn unstage(root: &Path, paths: &[String]) -> Result<(), GitChangesError> {
    let repo = open(root)?;
    let rels = paths.iter().map(|p| relative(p)).collect::<Result<Vec<_>, _>>()?;
    // Bound before returning: the head borrows `repo`.
    let done = match repo.head() {
        Ok(head) => {
            let commit = head.peel_to_commit().map_err(git)?;
            repo.reset_default(Some(commit.as_object()), rels).map_err(git)
        }
        // No commit yet: unstaging is taking the file out of the index.
        Err(e) if e.code() == ErrorCode::UnbornBranch => {
            let mut index = repo.index().map_err(git)?;
            for rel in rels {
                index.remove_path(rel).map_err(git)?;
            }
            index.write().map_err(git)
        }
        Err(e) => Err(git(e)),
    };
    done
}

/// Commits the index on HEAD; returns the new commit's short id.
pub fn commit(root: &Path, message: &str) -> Result<String, GitChangesError> {
    let message = message.trim();
    if message.is_empty() {
        return Err(GitChangesError::EmptyMessage);
    }
    let repo = open(root)?;
    let head = head_tree(&repo)?;
    let mut index = repo.index().map_err(git)?;
    let staged = repo.diff_tree_to_index(head.as_ref(), Some(&index), None).map_err(git)?;
    if staged.deltas().len() == 0 {
        return Err(GitChangesError::NothingStaged);
    }

    let tree = repo.find_tree(index.write_tree().map_err(git)?).map_err(git)?;
    let sig = signature(&repo)?;
    let parent = match repo.head() {
        Ok(head) => Some(head.peel_to_commit().map_err(git)?),
        Err(e) if e.code() == ErrorCode::UnbornBranch => None,
        Err(e) => return Err(git(e)),
    };
    let parents: Vec<&git2::Commit> = parent.iter().collect();
    let oid = repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parents).map_err(git)?;
    Ok(oid.to_string().chars().take(7).collect())
}

/// What `commit` would record, file by file with its patch, sorted by path —
/// what a commit message is written from. Empty when nothing is staged.
pub fn staged_patches(root: &Path) -> Result<Vec<StagedPatch>, GitChangesError> {
    let repo = open(root)?;
    let head = head_tree(&repo)?;
    let index = repo.index().map_err(git)?;
    let diff = repo.diff_tree_to_index(head.as_ref(), Some(&index), None).map_err(git)?;
    let mut patches: BTreeMap<String, StagedPatch> = files(&diff)?
        .into_iter()
        .map(|f| (f.path.clone(), StagedPatch { path: f.path, add: f.add, del: f.del, patch: Some(String::new()) }))
        .collect();
    diff.print(DiffFormat::Patch, |delta, _, line| {
        let path = delta.new_file().path().or_else(|| delta.old_file().path());
        let Some(entry) = path.and_then(|p| patches.get_mut(&*p.to_string_lossy())) else { return true };
        match (line.origin(), entry.patch.as_mut()) {
            // The path is already the entry's own header.
            ('F', _) | (_, None) => {}
            ('B', _) => entry.patch = None,
            (origin @ ('+' | '-' | ' '), Some(patch)) => {
                patch.push(origin);
                patch.push_str(&String::from_utf8_lossy(line.content()));
            }
            (_, Some(patch)) => patch.push_str(&String::from_utf8_lossy(line.content())),
        }
        true
    })
    .map_err(git)?;
    Ok(patches.into_values().collect())
}

/// Where HEAD is and up to `limit` commits reachable from it, newest first;
/// no commits before the first one.
pub fn history(root: &Path, limit: usize) -> Result<GitHistory, GitChangesError> {
    let repo = open(root)?;
    let branch = crate::infra::git_head::current_branch(root);
    let mut history = GitHistory { branch, upstream: None, ahead: 0, behind: 0, commits: Vec::new(), more: false };
    if head_tree(&repo)?.is_none() {
        return Ok(history);
    }
    let head = repo.head().map_err(git)?;
    let head_id = head.target();
    let checked_out = if head.is_branch() { head.name().ok().map(String::from) } else { None };
    if head.is_branch() {
        if let Ok(upstream) = git2::Branch::wrap(head).upstream() {
            history.upstream = upstream.name().ok().flatten().map(String::from);
            if let (Some(local), Some(remote)) = (head_id, upstream.get().target()) {
                (history.ahead, history.behind) = repo.graph_ahead_behind(local, remote).map_err(git)?;
            }
        }
    }

    // Every branch and tag by the commit it points at; a remote's HEAD only
    // repeats the branch it names.
    let mut refs: HashMap<git2::Oid, Vec<String>> = HashMap::new();
    for reference in repo.references().map_err(git)?.flatten() {
        let name = reference.name().unwrap_or_default();
        if reference.is_remote() && name.ends_with("/HEAD") || checked_out.as_deref() == Some(name) {
            continue;
        }
        let (Ok(commit), Ok(short)) = (reference.peel_to_commit(), reference.shorthand()) else { continue };
        if reference.is_branch() || reference.is_remote() || reference.is_tag() {
            refs.entry(commit.id()).or_default().push(short.to_string());
        }
    }

    let mut walk = repo.revwalk().map_err(git)?;
    walk.push_head().map_err(git)?;
    walk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::TIME).map_err(git)?;
    for id in walk {
        if history.commits.len() == limit {
            history.more = true;
            break;
        }
        let commit = repo.find_commit(id.map_err(git)?).map_err(git)?;
        let author = commit.author().name().unwrap_or_default().to_string();
        let mut on_it = refs.remove(&commit.id()).unwrap_or_default();
        on_it.sort();
        history.commits.push(CommitSummary {
            id: commit.id().to_string().chars().take(7).collect(),
            summary: commit.summary().map_err(git)?.unwrap_or_default().to_string(),
            author,
            time: commit.time().seconds(),
            head: Some(commit.id()) == head_id,
            refs: on_it,
        });
    }
    Ok(history)
}

/// Largest side the viewer is sent; past it the file is named, not drawn.
const MAX_VIEW_BYTES: usize = 1 << 20;

/// The two versions of `path` that `side` compares.
pub fn file_view(root: &Path, path: &str, side: FileSide) -> Result<FileView, GitChangesError> {
    let rel = relative(path)?;
    let (old, new) = match side {
        FileSide::Worktree => {
            let new = read_under(root, rel, path)?;
            let old = match Repository::discover(root) {
                Ok(repo) => match folder_in_repo(&repo, root) {
                    Some(folder) => head_blob(&repo, &folder.join(rel))?,
                    None => None,
                },
                Err(_) => None,
            };
            (old, new)
        }
        FileSide::Unstaged => {
            let repo = open(root)?;
            let workdir = repo.workdir().ok_or(GitChangesError::NotARepository)?;
            (index_blob(&repo, rel)?, read_under(workdir, rel, path)?)
        }
        FileSide::Staged => {
            let repo = open(root)?;
            (head_blob(&repo, rel)?, index_blob(&repo, rel)?)
        }
    };
    let unviewable = [&old, &new].into_iter().flatten().find_map(|bytes| {
        if bytes.len() > MAX_VIEW_BYTES {
            Some(Unviewable::TooLarge)
        } else if bytes.iter().take(8000).any(|&b| b == 0) {
            // Git's own test: a NUL near the start means binary.
            Some(Unviewable::Binary)
        } else {
            None
        }
    });
    let text = |bytes: Option<Vec<u8>>| bytes.map(|b| String::from_utf8_lossy(&b).into_owned());
    Ok(match unviewable {
        Some(_) => FileView { old: None, new: None, unviewable },
        None => FileView { old: text(old), new: text(new), unviewable },
    })
}

/// A file under `base`, `None` when there is none. A symlink leading out of
/// `base` is refused, as is a folder.
fn read_under(base: &Path, rel: &Path, shown: &str) -> Result<Option<Vec<u8>>, GitChangesError> {
    let refused = || GitChangesError::InvalidPath(shown.to_string());
    let full = match base.join(rel).canonicalize() {
        Ok(full) => full,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(GitChangesError::Read(shown.to_string())),
    };
    let base = base.canonicalize().map_err(|_| refused())?;
    if !full.starts_with(&base) || full.is_dir() {
        return Err(refused());
    }
    std::fs::read(&full).map(Some).map_err(|_| GitChangesError::Read(shown.to_string()))
}

fn head_blob(repo: &Repository, rel: &Path) -> Result<Option<Vec<u8>>, GitChangesError> {
    let Some(tree) = head_tree(repo)? else { return Ok(None) };
    let entry = match tree.get_path(rel) {
        Ok(entry) => entry,
        Err(e) if e.code() == ErrorCode::NotFound => return Ok(None),
        Err(e) => return Err(git(e)),
    };
    let blob = entry.to_object(repo).and_then(|o| o.peel_to_blob()).map_err(git)?;
    Ok(Some(blob.content().to_vec()))
}

fn index_blob(repo: &Repository, rel: &Path) -> Result<Option<Vec<u8>>, GitChangesError> {
    let index = repo.index().map_err(git)?;
    let Some(entry) = index.get_path(rel, 0) else { return Ok(None) };
    let blob = repo.find_blob(entry.id).map_err(git)?;
    Ok(Some(blob.content().to_vec()))
}

/// `root` relative to the repository's working tree — empty when they are the
/// same folder. `None` when that cannot be told.
fn folder_in_repo(repo: &Repository, root: &Path) -> Option<std::path::PathBuf> {
    let workdir = repo.workdir()?.canonicalize().ok()?;
    Some(root.canonicalize().ok()?.strip_prefix(workdir).ok()?.to_path_buf())
}

/// The `.git` directory of the repository `root` is in — what staging and
/// committing write, and the tree's watcher leaves out. `None` outside one.
pub fn git_dir(root: &Path) -> Option<std::path::PathBuf> {
    Repository::discover(root).ok().map(|repo| repo.path().to_path_buf())
}

fn open(root: &Path) -> Result<Repository, GitChangesError> {
    Repository::discover(root).map_err(|_| GitChangesError::NotARepository)
}

fn head_tree(repo: &Repository) -> Result<Option<git2::Tree<'_>>, GitChangesError> {
    match repo.head() {
        Ok(head) => Ok(Some(head.peel_to_tree().map_err(git)?)),
        Err(e) if e.code() == ErrorCode::UnbornBranch || e.code() == ErrorCode::NotFound => Ok(None),
        Err(e) => Err(git(e)),
    }
}

/// Each file of a diff with its added and removed lines; a binary file counts
/// none. Sorted by path.
fn files(diff: &Diff) -> Result<Vec<ChangedFile>, GitChangesError> {
    let mut counts: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for delta in diff.deltas() {
        if let Some(path) = delta.new_file().path().or_else(|| delta.old_file().path()) {
            counts.entry(path.to_string_lossy().into_owned()).or_default();
        }
    }
    diff.foreach(
        &mut |_, _| true,
        None,
        None,
        Some(&mut |delta, _, line| {
            let path = delta.new_file().path().or_else(|| delta.old_file().path());
            if let Some(entry) = path.and_then(|p| counts.get_mut(&*p.to_string_lossy())) {
                match line.origin() {
                    '+' => entry.0 += 1,
                    '-' => entry.1 += 1,
                    _ => {}
                }
            }
            true
        }),
    )
    .map_err(git)?;
    Ok(counts.into_iter().map(|(path, (add, del))| ChangedFile { path, add, del }).collect())
}

/// A repository-relative path, refusing anything that would leave it — the
/// frontend sends these back, so they are checked rather than trusted.
fn relative(path: &str) -> Result<&Path, GitChangesError> {
    let p = Path::new(path);
    if path.is_empty() || !p.components().all(|c| matches!(c, Component::Normal(_))) {
        return Err(GitChangesError::InvalidPath(path.to_string()));
    }
    Ok(p)
}

fn signature(repo: &Repository) -> Result<Signature<'static>, GitChangesError> {
    let sig = repo.signature().map_err(|_| GitChangesError::MissingIdentity)?;
    let (Some(name), Some(email)) = (sig.name().ok(), sig.email().ok()) else {
        return Err(GitChangesError::MissingIdentity);
    };
    if name.trim().is_empty() || email.trim().is_empty() {
        return Err(GitChangesError::MissingIdentity);
    }
    Signature::now(name, email).map_err(git)
}

fn git(err: git2::Error) -> GitChangesError {
    GitChangesError::Git(err.message().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::temp_dir;
    use std::fs;

    fn repo() -> (std::path::PathBuf, Repository) {
        let dir = temp_dir("git-changes");
        let repo = Repository::init(&dir).unwrap();
        let mut config = repo.config().unwrap();
        config.set_str("user.name", "Test").unwrap();
        config.set_str("user.email", "test@example.com").unwrap();
        (dir, repo)
    }

    fn file(path: &str, add: usize, del: usize) -> ChangedFile {
        ChangedFile { path: path.into(), add, del }
    }

    #[test]
    fn a_new_file_is_unstaged_until_staged_then_committed() {
        let (dir, _repo) = repo();
        fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
        fs::create_dir(dir.join("new")).unwrap();
        fs::write(dir.join("new/b.txt"), "b\n").unwrap();

        let before = changes(&dir).unwrap();
        assert_eq!(before.unstaged, vec![file("a.txt", 2, 0), file("new/b.txt", 1, 0)]);
        assert!(before.staged.is_empty());

        stage(&dir, &["a.txt".into(), "new/b.txt".into()]).unwrap();
        let staged = changes(&dir).unwrap();
        assert_eq!(staged.staged, vec![file("a.txt", 2, 0), file("new/b.txt", 1, 0)]);
        assert!(staged.unstaged.is_empty());

        let empty = history(&dir, 10).unwrap();
        assert!(empty.commits.is_empty() && !empty.more);
        let id = commit(&dir, "  first\n").unwrap();
        assert_eq!(id.len(), 7);
        assert_eq!(changes(&dir).unwrap(), WorkingChanges { staged: vec![], unstaged: vec![] });
    }

    #[test]
    fn staged_patches_hold_the_index_not_the_disk() {
        let (dir, _repo) = repo();
        fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
        fs::write(dir.join("logo.bin"), [0u8, 1, 2, 0]).unwrap();
        stage(&dir, &["a.txt".into(), "logo.bin".into()]).unwrap();
        commit(&dir, "first").unwrap();
        assert!(staged_patches(&dir).unwrap().is_empty());

        fs::write(dir.join("a.txt"), "one\n2\n").unwrap();
        fs::write(dir.join("logo.bin"), [0u8, 9, 9, 0]).unwrap();
        stage(&dir, &["a.txt".into(), "logo.bin".into()]).unwrap();
        // Edited after staging: not what the commit records.
        fs::write(dir.join("a.txt"), "one\n2\nunstaged\n").unwrap();

        let patches = staged_patches(&dir).unwrap();
        assert_eq!(patches.len(), 2);
        assert_eq!((patches[0].path.as_str(), patches[0].add, patches[0].del), ("a.txt", 1, 1));
        assert_eq!(patches[0].patch.as_deref(), Some("@@ -1,2 +1,2 @@\n one\n-two\n+2\n"));
        assert_eq!((patches[1].path.as_str(), patches[1].patch.as_deref()), ("logo.bin", None));
    }

    #[test]
    fn counts_each_side_and_unstages_back() {
        let (dir, _repo) = repo();
        fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
        stage(&dir, &["a.txt".into()]).unwrap();
        commit(&dir, "first").unwrap();

        fs::write(dir.join("a.txt"), "one\n2\nthree\n").unwrap();
        stage(&dir, &["a.txt".into()]).unwrap();
        fs::write(dir.join("a.txt"), "one\n2\nthree\nfour\n").unwrap();
        let both = changes(&dir).unwrap();
        assert_eq!(both.staged, vec![file("a.txt", 2, 1)]);
        assert_eq!(both.unstaged, vec![file("a.txt", 1, 0)]);

        unstage(&dir, &["a.txt".into()]).unwrap();
        let back = changes(&dir).unwrap();
        assert!(back.staged.is_empty());
        assert_eq!(back.unstaged, vec![file("a.txt", 3, 1)]);
    }

    #[test]
    fn totals_count_staged_and_unstaged_once_new_files_too_within_the_folder() {
        let (dir, _repo) = repo();
        fs::create_dir(dir.join("sub")).unwrap();
        fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
        fs::write(dir.join("sub/b.txt"), "b\n").unwrap();
        stage(&dir, &["a.txt".into(), "sub/b.txt".into()]).unwrap();
        commit(&dir, "first").unwrap();
        assert_eq!(totals(&dir).unwrap(), ChangeTotals { files: 0, add: 0, del: 0 });

        // Staged, then edited again: one change against HEAD, not two.
        fs::write(dir.join("a.txt"), "one\n2\n").unwrap();
        stage(&dir, &["a.txt".into()]).unwrap();
        fs::write(dir.join("a.txt"), "one\n2\n3\n").unwrap();
        fs::write(dir.join("sub/new.txt"), "x\ny\n").unwrap();
        fs::remove_file(dir.join("sub/b.txt")).unwrap();
        assert_eq!(totals(&dir).unwrap(), ChangeTotals { files: 3, add: 4, del: 2 });
        // The subfolder alone: its new file and its deletion.
        assert_eq!(totals(&dir.join("sub")).unwrap(), ChangeTotals { files: 2, add: 2, del: 1 });

        // An ignored file added anyway goes into the next commit, so it counts:
        // on disk alone it is ignored, and only the index says otherwise.
        fs::write(dir.join(".gitignore"), "*.log\n").unwrap();
        fs::write(dir.join("keep.log"), "l\n").unwrap();
        stage(&dir, &["keep.log".into()]).unwrap();
        assert_eq!(totals(&dir).unwrap(), ChangeTotals { files: 5, add: 6, del: 2 });
    }

    #[test]
    fn a_deleted_file_stages_its_deletion() {
        let (dir, _repo) = repo();
        fs::write(dir.join("a.txt"), "one\n").unwrap();
        stage(&dir, &["a.txt".into()]).unwrap();
        commit(&dir, "first").unwrap();
        fs::remove_file(dir.join("a.txt")).unwrap();

        stage(&dir, &["a.txt".into()]).unwrap();
        assert_eq!(changes(&dir).unwrap().staged, vec![file("a.txt", 0, 1)]);
    }

    #[test]
    fn unstaging_before_the_first_commit_takes_it_out_of_the_index() {
        let (dir, _repo) = repo();
        fs::write(dir.join("a.txt"), "one\n").unwrap();
        stage(&dir, &["a.txt".into()]).unwrap();
        unstage(&dir, &["a.txt".into()]).unwrap();
        let after = changes(&dir).unwrap();
        assert!(after.staged.is_empty());
        assert_eq!(after.unstaged, vec![file("a.txt", 1, 0)]);
    }

    fn commit_file(dir: &Path, n: usize, message: &str) -> String {
        fs::write(dir.join("a.txt"), format!("{n}\n")).unwrap();
        stage(dir, &["a.txt".into()]).unwrap();
        commit(dir, message).unwrap()
    }

    #[test]
    fn the_history_lists_commits_newest_first_up_to_the_limit() {
        let (dir, _repo) = repo();
        for (n, message) in ["first", "second\n\nbody", "third"].iter().enumerate() {
            commit_file(&dir, n, message);
        }
        let all = history(&dir, 10).unwrap();
        let summaries: Vec<&str> = all.commits.iter().map(|c| c.summary.as_str()).collect();
        assert_eq!(summaries, ["third", "second", "first"]);
        assert_eq!(all.commits[0].author, "Test");
        assert_eq!(all.commits[0].id.len(), 7);
        assert!(all.commits[0].time > 0);
        assert!(!all.more);
        let two = history(&dir, 2).unwrap();
        assert_eq!(two.commits.len(), 2);
        assert!(two.more);
        assert!(!history(&dir, 3).unwrap().more);
        assert_eq!(history(&temp_dir("git-log-plain"), 10), Err(GitChangesError::NotARepository));
    }

    #[test]
    fn marks_head_and_the_other_refs_and_counts_against_the_upstream() {
        let (dir, repo) = repo();
        let base = commit_file(&dir, 0, "base");
        let base = repo.revparse_single(&base).unwrap().peel_to_commit().unwrap();
        // Named to sort last, though git lists local branches first.
        repo.branch("zeta", &base, false).unwrap();
        repo.tag_lightweight("v1", base.as_object(), false).unwrap();
        // A remote branch at the base, tracked by the checked-out one.
        repo.reference("refs/remotes/origin/main", base.id(), false, "").unwrap();
        repo.reference_symbolic("refs/remotes/origin/HEAD", "refs/remotes/origin/main", false, "").unwrap();
        let branch = repo.head().unwrap().shorthand().unwrap().to_string();
        let mut config = repo.config().unwrap();
        config.set_str(&format!("branch.{branch}.remote"), "origin").unwrap();
        config.set_str(&format!("branch.{branch}.merge"), "refs/heads/main").unwrap();
        repo.remote("origin", "https://example.com/repo.git").unwrap();
        commit_file(&dir, 1, "tip");

        let h = history(&dir, 10).unwrap();
        assert_eq!(h.branch.as_deref(), Some(branch.as_str()));
        assert_eq!(h.upstream.as_deref(), Some("origin/main"));
        assert_eq!((h.ahead, h.behind), (1, 0));
        let marks: Vec<(bool, Vec<String>)> = h.commits.iter().map(|c| (c.head, c.refs.clone())).collect();
        assert_eq!(marks, [(true, vec![]), (false, vec!["origin/main".into(), "v1".into(), "zeta".into()])]);
    }

    fn view(old: Option<&str>, new: Option<&str>) -> FileView {
        FileView { old: old.map(String::from), new: new.map(String::from), unviewable: None }
    }

    #[test]
    fn the_viewer_compares_each_side_of_the_index() {
        let (dir, _repo) = repo();
        fs::write(dir.join("a.txt"), "one\n").unwrap();
        // Untracked: nothing in the index, nothing in HEAD.
        assert_eq!(file_view(&dir, "a.txt", FileSide::Unstaged), Ok(view(None, Some("one\n"))));
        stage(&dir, &["a.txt".into()]).unwrap();
        assert_eq!(file_view(&dir, "a.txt", FileSide::Staged), Ok(view(None, Some("one\n"))));
        commit(&dir, "first").unwrap();

        fs::write(dir.join("a.txt"), "two\n").unwrap();
        stage(&dir, &["a.txt".into()]).unwrap();
        fs::write(dir.join("a.txt"), "three\n").unwrap();
        assert_eq!(file_view(&dir, "a.txt", FileSide::Staged), Ok(view(Some("one\n"), Some("two\n"))));
        assert_eq!(file_view(&dir, "a.txt", FileSide::Unstaged), Ok(view(Some("two\n"), Some("three\n"))));
        assert_eq!(file_view(&dir, "a.txt", FileSide::Worktree), Ok(view(Some("one\n"), Some("three\n"))));

        fs::remove_file(dir.join("a.txt")).unwrap();
        assert_eq!(file_view(&dir, "a.txt", FileSide::Unstaged), Ok(view(Some("two\n"), None)));
    }

    #[test]
    fn the_worktree_side_takes_the_open_folders_paths_in_or_out_of_a_repository() {
        let (dir, _repo) = repo();
        fs::create_dir(dir.join("sub")).unwrap();
        fs::write(dir.join("sub/b.txt"), "old\n").unwrap();
        stage(&dir, &["sub/b.txt".into()]).unwrap();
        commit(&dir, "first").unwrap();
        fs::write(dir.join("sub/b.txt"), "new\n").unwrap();
        assert_eq!(file_view(&dir.join("sub"), "b.txt", FileSide::Worktree), Ok(view(Some("old\n"), Some("new\n"))));
        // New since the commit: HEAD has a tree, just not this file.
        fs::write(dir.join("sub/fresh.txt"), "f\n").unwrap();
        assert_eq!(file_view(&dir.join("sub"), "fresh.txt", FileSide::Worktree), Ok(view(None, Some("f\n"))));

        let plain = temp_dir("git-view-plain");
        fs::write(plain.join("c.txt"), "c\n").unwrap();
        assert_eq!(file_view(&plain, "c.txt", FileSide::Worktree), Ok(view(None, Some("c\n"))));
        assert_eq!(file_view(&plain, "c.txt", FileSide::Staged), Err(GitChangesError::NotARepository));
    }

    #[test]
    fn the_viewer_names_binary_and_huge_files_and_refuses_what_leaves_the_folder() {
        let (dir, _repo) = repo();
        fs::write(dir.join("bin"), [b'a', 0, b'b']).unwrap();
        assert_eq!(file_view(&dir, "bin", FileSide::Worktree).unwrap().unviewable, Some(Unviewable::Binary));
        fs::write(dir.join("big"), "x".repeat(MAX_VIEW_BYTES + 1)).unwrap();
        let big = file_view(&dir, "big", FileSide::Worktree).unwrap();
        assert_eq!((big.unviewable, big.new), (Some(Unviewable::TooLarge), None));
        fs::write(dir.join("edge"), "x".repeat(MAX_VIEW_BYTES)).unwrap();
        assert_eq!(file_view(&dir, "edge", FileSide::Worktree).unwrap().unviewable, None);

        for bad in ["../x", "/etc/passwd", ""] {
            assert_eq!(file_view(&dir, bad, FileSide::Worktree), Err(GitChangesError::InvalidPath(bad.into())));
        }
        fs::create_dir(dir.join("folder")).unwrap();
        assert_eq!(file_view(&dir, "folder", FileSide::Worktree), Err(GitChangesError::InvalidPath("folder".into())));
        #[cfg(unix)]
        {
            let outside = temp_dir("git-view-outside");
            fs::write(outside.join("secret"), "s").unwrap();
            std::os::unix::fs::symlink(outside.join("secret"), dir.join("link")).unwrap();
            assert_eq!(file_view(&dir, "link", FileSide::Worktree), Err(GitChangesError::InvalidPath("link".into())));
        }
        assert_eq!(file_view(&dir, "missing", FileSide::Worktree), Ok(view(None, None)));
    }

    #[test]
    fn refuses_what_it_cannot_commit_or_reach() {
        let (dir, _repo) = repo();
        assert_eq!(commit(&dir, "x"), Err(GitChangesError::NothingStaged));
        fs::write(dir.join("a.txt"), "one\n").unwrap();
        stage(&dir, &["a.txt".into()]).unwrap();
        assert_eq!(commit(&dir, "  \n"), Err(GitChangesError::EmptyMessage));
        for bad in ["../x", "/etc/passwd", "./a.txt", ""] {
            assert_eq!(stage(&dir, &[bad.into()]), Err(GitChangesError::InvalidPath(bad.into())));
        }
        let plain = temp_dir("git-changes-plain");
        assert_eq!(changes(&plain), Err(GitChangesError::NotARepository));
    }

    #[test]
    fn a_review_reads_every_change_against_head_with_its_new_lines() {
        let (dir, _repo) = repo();
        fs::write(dir.join("a.rs"), "one\ntwo\nthree\n").unwrap();
        fs::write(dir.join("gone.rs"), "bye\n").unwrap();
        fs::write(dir.join("moved.rs"), "a long enough line to be found as the same file\nand another\n").unwrap();
        stage(&dir, &["a.rs".into(), "gone.rs".into(), "moved.rs".into()]).unwrap();
        commit(&dir, "first").unwrap();

        fs::write(dir.join("a.rs"), "one\n2\nthree\nfour\n").unwrap();
        stage(&dir, &["a.rs".into()]).unwrap();
        fs::write(dir.join("a.rs"), "one\n2\nthree\nfour\nfive\n").unwrap();
        fs::remove_file(dir.join("gone.rs")).unwrap();
        fs::rename(dir.join("moved.rs"), dir.join("renamed.rs")).unwrap();
        stage(&dir, &["gone.rs".into(), "moved.rs".into(), "renamed.rs".into()]).unwrap();
        fs::create_dir(dir.join("new")).unwrap();
        fs::write(dir.join("new/b.rs"), "fresh\n").unwrap();
        fs::write(dir.join("logo.bin"), [0u8, 1, 2, 0]).unwrap();

        let files = review_diffs(&dir).unwrap();
        let summary: Vec<(&str, &FileStatus, bool)> = files.iter().map(|f| (f.path.as_str(), &f.status, f.binary)).collect();
        assert_eq!(
            summary,
            [
                ("a.rs", &FileStatus::Modified, false),
                ("gone.rs", &FileStatus::Deleted, false),
                ("logo.bin", &FileStatus::Added, true),
                ("new/b.rs", &FileStatus::Added, false),
                ("renamed.rs", &FileStatus::Renamed { from: "moved.rs".into() }, false),
            ]
        );
        let a = &files[0];
        let added: Vec<(u32, &str)> = a.lines.iter().filter(|l| l.added).map(|l| (l.number, l.text.as_str())).collect();
        assert_eq!(added, [(2, "2"), (4, "four"), (5, "five")], "staged and unstaged, against HEAD");
        assert!(a.patch.starts_with("@@ ") && a.patch.contains("+five\n"), "{}", a.patch);
        assert!(a.lines.iter().any(|l| !l.added && l.text == "one"), "context lines too");
    }

    /// The code around each change comes with it: ten lines each side.
    #[test]
    fn a_review_sees_ten_lines_around_a_change() {
        let (dir, _repo) = repo();
        let text = |changed: &str| (1..=40).map(|n| if n == 15 { changed.to_string() } else { format!("line {n}") }).collect::<Vec<_>>().join("\n") + "\n";
        fs::write(dir.join("a.rs"), text("line 15")).unwrap();
        stage(&dir, &["a.rs".into()]).unwrap();
        commit(&dir, "first").unwrap();
        fs::write(dir.join("a.rs"), text("changed")).unwrap();

        let files = review_diffs(&dir).unwrap();
        let numbers: Vec<u32> = files[0].lines.iter().map(|l| l.number).collect();
        assert_eq!(numbers, (5..=25).collect::<Vec<_>>());
    }

    #[test]
    fn a_review_of_a_subfolder_reads_its_own_changes_by_its_own_paths() {
        let (dir, _repo) = repo();
        fs::create_dir(dir.join("app")).unwrap();
        fs::write(dir.join("app/x.rs"), "x\n").unwrap();
        fs::write(dir.join("root.rs"), "r\n").unwrap();
        let files = review_diffs(&dir.join("app")).unwrap();
        assert_eq!(files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["x.rs"]);
        assert_eq!(review_diffs(&temp_dir("git-review-plain")), Err(GitChangesError::NotARepository));
    }
}
