//! Branches of the open folder's repository: listing them, a checkout that
//! never overwrites uncommitted work, and a worktree started from one.
//!
//! The checkout is libgit2's *safe* strategy: it works out every file it
//! would touch before touching any, and refuses outright if one of them holds
//! changes — the same line `git switch` draws. Edits to files the two
//! branches share come along unchanged, which is not a conflict.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use git2::build::CheckoutBuilder;
use git2::{
    BranchType, CheckoutNotificationType, Commit, ErrorCode, Oid, Repository, RepositoryState, Status, StatusOptions,
    WorktreeAddOptions, WorktreeLockStatus, WorktreePruneOptions,
};

use crate::domain::git_branches::{
    keeps_branch, worktree_name, BranchRef, CheckoutOutcome, GitBranchError, WorktreeState, WORKTREE_BRANCH_PREFIX,
};

/// Local branches, then the remote ones with no local branch of that name.
pub fn list(root: &Path) -> Result<Vec<BranchRef>, GitBranchError> {
    let repo = open(root)?;
    let names = |kind| -> Result<Vec<String>, GitBranchError> {
        let mut names = Vec::new();
        for branch in repo.branches(Some(kind)).map_err(git)? {
            let (branch, _) = branch.map_err(git)?;
            // `origin/HEAD` is a pointer to another remote branch, not one of its own.
            if !matches!(branch.get().symbolic_target(), Ok(None)) {
                continue;
            }
            if let Ok(Some(name)) = branch.name() {
                names.push(name.to_string());
            }
        }
        names.sort();
        Ok(names)
    };
    let local = names(BranchType::Local)?;
    let remote = names(BranchType::Remote)?
        .into_iter()
        .filter(|name| !local.iter().any(|mine| Some(mine.as_str()) == local_name(name)));
    Ok(local
        .iter()
        .map(|name| BranchRef { name: name.clone(), remote: false })
        .chain(remote.map(|name| BranchRef { name, remote: true }))
        .collect())
}

/// Switches the folder to `name`. A remote branch becomes a local one of the
/// same name, tracking it.
pub fn checkout(root: &Path, name: &str) -> Result<CheckoutOutcome, GitBranchError> {
    let repo = open(root)?;
    ready(&repo)?;
    let target = resolve(&repo, name)?;
    let refname = format!("refs/heads/{}", target.local);
    if repo.head().ok().and_then(|head| head.name().ok().map(String::from)).as_deref() == Some(refname.as_str()) {
        return Ok(CheckoutOutcome::Switched);
    }
    if let Some(path) = checked_out_elsewhere(&repo, &refname) {
        return Err(GitBranchError::CheckedOutElsewhere { branch: target.local, path: path.display().to_string() });
    }

    let conflicts = RefCell::new(Vec::new());
    let mut options = CheckoutBuilder::new();
    options.safe().notify_on(CheckoutNotificationType::CONFLICT).notify(|_, path, _, _, _| {
        if let Some(path) = path {
            conflicts.borrow_mut().push(path.to_string_lossy().replace('\\', "/"));
        }
        true
    });
    match repo.checkout_tree(target.commit.as_object(), Some(&mut options)) {
        Ok(()) => {}
        Err(e) if e.code() == ErrorCode::Conflict => {
            drop(options);
            // Once each and in tree order: libgit2 walks both trees path by path.
            return Ok(CheckoutOutcome::Conflicts { paths: conflicts.into_inner() });
        }
        Err(e) => return Err(git(e)),
    }

    if let Some(upstream) = &target.upstream {
        let mut branch = repo.branch(&target.local, &target.commit, false).map_err(git)?;
        branch.set_upstream(Some(upstream)).map_err(git)?;
    }
    repo.set_head(&refname).map_err(git)?;
    Ok(CheckoutOutcome::Switched)
}

/// A new worktree on a new branch started from `base`, under
/// `worktrees/<repository>/`, named with `stamp` — the time, as the caller
/// reads the clock. The open folder is not touched: nothing is checked out in
/// it and its changes stay where they are.
pub fn add_worktree(root: &Path, base: &str, stamp: &str, worktrees: &Path) -> Result<PathBuf, GitBranchError> {
    let repo = open(root)?;
    let start = resolve(&repo, base)?;
    let parent = worktrees.join(repository_name(&repo));
    // Taken names are skipped rather than reused: an old worktree of the same
    // name may still hold someone's work.
    for n in 1..=100 {
        let name = worktree_name(&start.local, stamp, n);
        let branch_name = format!("{WORKTREE_BRANCH_PREFIX}{name}");
        let path = parent.join(&name);
        // A registered worktree has its branch, so the two checks cover it too.
        if path.exists() || repo.find_branch(&branch_name, BranchType::Local).is_ok() {
            continue;
        }
        std::fs::create_dir_all(&parent).map_err(|e| GitBranchError::Git(format!("{}: {e}", parent.display())))?;
        let branch = repo.branch(&branch_name, &start.commit, false).map_err(git)?;
        let mut options = WorktreeAddOptions::new();
        options.reference(Some(branch.get()));
        return match repo.worktree(&name, &path, Some(&options)) {
            Ok(_) => Ok(path),
            Err(e) => {
                // The branch was made only for this worktree.
                let mut branch = branch;
                let _ = branch.delete();
                Err(git(e))
            }
        };
    }
    Err(GitBranchError::Git(format!("no free worktree name for {}", start.local)))
}

/// The linked worktree `root` is in, as removing it would find it.
pub fn worktree_state(root: &Path) -> Result<WorktreeState, GitBranchError> {
    let repo = open(root)?;
    if !repo.is_worktree() {
        return Err(GitBranchError::NotAWorktree);
    }
    let main = repo.commondir().parent().ok_or(GitBranchError::NotAWorktree)?.display().to_string();
    let head = repo.head().ok();
    let branch = head.as_ref().filter(|head| head.is_branch()).and_then(|head| head.shorthand().ok()).map(String::from);
    let own_commits = match head.as_ref().and_then(|head| head.target()) {
        Some(tip) => own_commits(&repo, tip, branch.as_deref())?,
        None => 0,
    };
    let keeps_branch = branch.as_deref().is_some_and(|name| keeps_branch(name, own_commits));
    Ok(WorktreeState { main, branch, dirty: dirty(&repo)?, own_commits, keeps_branch })
}

/// Removes the linked worktree at `path`, with its folder. Its branch goes
/// too when the app made it and nothing is on it that is not also elsewhere;
/// otherwise it stays, and is returned.
///
/// Checked again here, not trusted from an earlier look: between the dialog
/// and the click, a terminal may have written to it. Refused while anything
/// in it is uncommitted — the folder goes, and git would not get it back.
pub fn remove_worktree(path: &Path) -> Result<Option<String>, GitBranchError> {
    let target = path.canonicalize().map_err(|_| GitBranchError::NotAWorktree)?;
    let state = worktree_state(&target)?;
    // Only as one of the repository's registered worktrees — not whatever the
    // folder's `.git` file claims to be. The list is the shared `.git`'s, the
    // same from any of its working trees.
    let repo = open(&target)?;
    let names = repo.worktrees().map_err(git)?;
    let worktree = names
        .iter()
        .filter_map(|name| repo.find_worktree(name.ok().flatten()?).ok())
        .find(|wt| wt.path().canonicalize().ok().as_deref() == Some(target.as_path()))
        .ok_or(GitBranchError::NotAWorktree)?;

    if !state.dirty.is_empty() {
        return Err(GitBranchError::Dirty(state.dirty));
    }
    if state.branch.is_none() && state.own_commits > 0 {
        return Err(GitBranchError::Unreachable(state.own_commits));
    }
    if let WorktreeLockStatus::Locked(reason) = worktree.is_locked().map_err(git)? {
        return Err(GitBranchError::Locked(reason.unwrap_or_default()));
    }
    worktree
        .prune(Some(WorktreePruneOptions::new().valid(true).working_tree(true)))
        .map_err(git)?;

    let Some(branch) = state.branch else { return Ok(None) };
    if state.keeps_branch {
        return Ok(Some(branch));
    }
    repo.find_branch(&branch, BranchType::Local).and_then(|mut b| b.delete()).map_err(git)?;
    Ok(None)
}

/// Changed and untracked files; ignored ones are not work.
fn dirty(repo: &Repository) -> Result<Vec<String>, GitBranchError> {
    let mut options = StatusOptions::new();
    options.include_untracked(true).recurse_untracked_dirs(true).include_ignored(false);
    let statuses = repo.statuses(Some(&mut options)).map_err(git)?;
    Ok(statuses
        .iter()
        .filter(|entry| entry.status() != Status::CURRENT)
        .filter_map(|entry| entry.path().ok().map(String::from))
        .collect())
}

/// Commits reachable from `tip` and from no other branch or remote branch.
fn own_commits(repo: &Repository, tip: Oid, branch: Option<&str>) -> Result<usize, GitBranchError> {
    let own = branch.map(|name| format!("refs/heads/{name}"));
    let mut walk = repo.revwalk().map_err(git)?;
    walk.push(tip).map_err(git)?;
    for reference in repo.references().map_err(git)?.flatten() {
        let Ok(name) = reference.name() else { continue };
        let other_branch = name.starts_with("refs/heads/") || name.starts_with("refs/remotes/");
        if !other_branch || Some(name) == own.as_deref() {
            continue;
        }
        if let Some(id) = reference.target() {
            walk.hide(id).map_err(git)?;
        }
    }
    Ok(walk.count())
}

struct Target<'r> {
    /// The local branch the folder ends up on.
    local: String,
    commit: Commit<'r>,
    /// `origin/x` when `x` is to be created from it.
    upstream: Option<String>,
}

fn resolve<'r>(repo: &'r Repository, name: &str) -> Result<Target<'r>, GitBranchError> {
    let commit = |branch: git2::Branch<'r>| branch.get().peel_to_commit().map_err(git);
    if let Ok(branch) = repo.find_branch(name, BranchType::Local) {
        return Ok(Target { local: name.to_string(), commit: commit(branch)?, upstream: None });
    }
    let remote = repo.find_branch(name, BranchType::Remote).map_err(|_| GitBranchError::NoSuchBranch(name.to_string()))?;
    let local = local_name(name).ok_or_else(|| GitBranchError::NoSuchBranch(name.to_string()))?.to_string();
    // Listed only when missing, but a branch made since then is the one meant.
    if let Ok(branch) = repo.find_branch(&local, BranchType::Local) {
        return Ok(Target { local, commit: commit(branch)?, upstream: None });
    }
    Ok(Target { local, commit: commit(remote)?, upstream: Some(name.to_string()) })
}

/// `feature` for `origin/feature`.
fn local_name(remote: &str) -> Option<&str> {
    remote.split_once('/').map(|(_, name)| name).filter(|name| !name.is_empty())
}

/// A merge or rebase half done owns the working tree; switching under it
/// would strand it.
fn ready(repo: &Repository) -> Result<(), GitBranchError> {
    let doing = match repo.state() {
        RepositoryState::Clean => return Ok(()),
        RepositoryState::Merge => "merge",
        RepositoryState::Revert | RepositoryState::RevertSequence => "revert",
        RepositoryState::CherryPick | RepositoryState::CherryPickSequence => "cherry-pick",
        RepositoryState::Bisect => "bisect",
        RepositoryState::Rebase
        | RepositoryState::RebaseInteractive
        | RepositoryState::RebaseMerge
        | RepositoryState::ApplyMailbox
        | RepositoryState::ApplyMailboxOrRebase => "rebase",
    };
    Err(GitBranchError::Busy(doing.to_string()))
}

/// The other working tree that has `refname` checked out. libgit2 would let
/// HEAD point at it anyway, and the two folders would then fight over it.
fn checked_out_elsewhere(repo: &Repository, refname: &str) -> Option<PathBuf> {
    let here = repo.workdir().and_then(|dir| dir.canonicalize().ok());
    let mut trees = Vec::new();
    if repo.is_worktree() {
        trees.extend(Repository::open(repo.commondir()).ok());
    }
    if let Ok(names) = repo.worktrees() {
        for name in names.iter().filter_map(|name| name.ok().flatten()) {
            let tree = repo.find_worktree(name).ok().and_then(|wt| Repository::open_from_worktree(&wt).ok());
            trees.extend(tree);
        }
    }
    trees.into_iter().find_map(|tree| {
        let dir = tree.workdir()?.canonicalize().ok()?;
        let head = tree.head().ok()?;
        (Some(&dir) != here.as_ref() && head.name().ok() == Some(refname)).then_some(dir)
    })
}

/// The main working tree's folder name, the same from any of its worktrees.
fn repository_name(repo: &Repository) -> String {
    repo.commondir()
        .parent()
        .and_then(Path::file_name)
        .map_or_else(|| "repository".to_string(), |name| name.to_string_lossy().into_owned())
}

fn open(root: &Path) -> Result<Repository, GitBranchError> {
    let repo = Repository::discover(root).map_err(|_| GitBranchError::NotARepository)?;
    if repo.is_bare() {
        return Err(GitBranchError::NotARepository);
    }
    Ok(repo)
}

fn git(e: git2::Error) -> GitBranchError {
    GitBranchError::Git(e.message().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::temp_dir;
    use git2::{Oid, Signature};
    use std::fs;

    fn commit(repo: &Repository, update: &str, parent: Option<Oid>, files: &[(&str, &str)]) -> Oid {
        let mut tree = repo.treebuilder(None).unwrap();
        for (path, text) in files {
            tree.insert(path, repo.blob(text.as_bytes()).unwrap(), 0o100644).unwrap();
        }
        let tree = repo.find_tree(tree.write().unwrap()).unwrap();
        let sig = Signature::now("Test", "test@example.com").unwrap();
        let parents: Vec<Commit> = parent.map(|id| repo.find_commit(id).unwrap()).into_iter().collect();
        let parents: Vec<&Commit> = parents.iter().collect();
        repo.commit(Some(update), &sig, &sig, "c", &tree, &parents).unwrap()
    }

    /// `main` checked out with a.txt and b.txt; `other` changes b.txt and adds c.txt.
    fn two_branches(label: &str) -> (PathBuf, Repository) {
        let dir = temp_dir(label);
        let repo = Repository::init(&dir).unwrap();
        // Files as committed, whatever the machine's `core.autocrlf` says: a
        // Windows runner's makes them CRLF on checkout.
        repo.config().unwrap().set_bool("core.autocrlf", false).unwrap();
        repo.set_head("refs/heads/main").unwrap();
        let main = commit(&repo, "HEAD", None, &[("a.txt", "a\n"), ("b.txt", "b\n")]);
        commit(&repo, "refs/heads/other", Some(main), &[("a.txt", "a\n"), ("b.txt", "B\n"), ("c.txt", "c\n")]);
        repo.checkout_head(Some(CheckoutBuilder::new().force())).unwrap();
        (dir, repo)
    }

    fn head(dir: &Path) -> String {
        Repository::open(dir).unwrap().head().unwrap().shorthand().unwrap().to_string()
    }

    #[test]
    fn edits_the_branches_share_come_along() {
        let (dir, _repo) = two_branches("branches-carry");
        fs::write(dir.join("a.txt"), "mine\n").unwrap();

        assert_eq!(checkout(&dir, "other").unwrap(), CheckoutOutcome::Switched);
        assert_eq!(head(&dir), "other");
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "mine\n");
        assert_eq!(fs::read_to_string(dir.join("b.txt")).unwrap(), "B\n");
        assert_eq!(fs::read_to_string(dir.join("c.txt")).unwrap(), "c\n");
    }

    #[test]
    fn an_edit_the_other_branch_would_overwrite_stops_the_switch_and_changes_nothing() {
        let (dir, _repo) = two_branches("branches-conflict");
        fs::write(dir.join("a.txt"), "mine too\n").unwrap();
        fs::write(dir.join("b.txt"), "mine\n").unwrap();
        fs::write(dir.join("c.txt"), "untracked\n").unwrap();

        assert_eq!(
            checkout(&dir, "other").unwrap(),
            CheckoutOutcome::Conflicts { paths: vec!["b.txt".into(), "c.txt".into()] }
        );
        assert_eq!(head(&dir), "main");
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "mine too\n");
        assert_eq!(fs::read_to_string(dir.join("b.txt")).unwrap(), "mine\n");
        assert_eq!(fs::read_to_string(dir.join("c.txt")).unwrap(), "untracked\n");
    }

    #[test]
    fn the_branch_already_out_is_no_switch() {
        let (dir, _repo) = two_branches("branches-same");
        fs::write(dir.join("b.txt"), "mine\n").unwrap();
        assert_eq!(checkout(&dir, "main").unwrap(), CheckoutOutcome::Switched);
        assert_eq!(fs::read_to_string(dir.join("b.txt")).unwrap(), "mine\n");
    }

    #[test]
    fn a_remote_branch_is_listed_once_and_checked_out_as_a_local_one_tracking_it() {
        let (dir, repo) = two_branches("branches-remote");
        let main = repo.refname_to_id("refs/heads/main").unwrap();
        let other = repo.refname_to_id("refs/heads/other").unwrap();
        repo.remote("origin", "https://example.com/r.git").unwrap();
        repo.reference("refs/remotes/origin/main", main, false, "").unwrap();
        repo.reference("refs/remotes/origin/feature", other, false, "").unwrap();
        repo.reference_symbolic("refs/remotes/origin/HEAD", "refs/remotes/origin/main", false, "").unwrap();

        let names: Vec<(String, bool)> = list(&dir).unwrap().into_iter().map(|b| (b.name, b.remote)).collect();
        assert_eq!(
            names,
            vec![("main".into(), false), ("other".into(), false), ("origin/feature".into(), true)]
        );

        assert_eq!(checkout(&dir, "origin/feature").unwrap(), CheckoutOutcome::Switched);
        assert_eq!(head(&dir), "feature");
        let branch = repo.find_branch("feature", BranchType::Local).unwrap();
        assert_eq!(branch.upstream().unwrap().name().unwrap(), Some("origin/feature"));
        assert_eq!(fs::read_to_string(dir.join("c.txt")).unwrap(), "c\n");

        // A remote branch with a local one of its name is that local branch.
        assert_eq!(checkout(&dir, "origin/main").unwrap(), CheckoutOutcome::Switched);
        assert_eq!(head(&dir), "main");
    }

    #[test]
    fn a_half_done_merge_and_an_unknown_branch_are_refused() {
        let (dir, repo) = two_branches("branches-busy");
        assert!(matches!(checkout(&dir, "nope"), Err(GitBranchError::NoSuchBranch(_))));

        let other = repo.refname_to_id("refs/heads/other").unwrap();
        fs::write(repo.path().join("MERGE_HEAD"), format!("{other}\n")).unwrap();
        assert!(matches!(checkout(&dir, "other"), Err(GitBranchError::Busy(what)) if what == "merge"));
        assert_eq!(head(&dir), "main");
    }

    #[test]
    fn a_worktree_starts_on_a_new_branch_and_leaves_the_folder_alone() {
        let (dir, _repo) = two_branches("branches-worktree");
        fs::write(dir.join("b.txt"), "mine\n").unwrap();
        let worktrees = temp_dir("branches-worktree-home");

        let first = add_worktree(&dir, "other", "0925-2214", &worktrees).unwrap();
        let repo_name = dir.file_name().unwrap();
        assert_eq!(first, worktrees.join(repo_name).join("other-0925-2214"));
        assert_eq!(head(&first), "kibo/other-0925-2214");
        assert_eq!(fs::read_to_string(first.join("b.txt")).unwrap(), "B\n");
        assert_eq!(head(&dir), "main");
        assert_eq!(fs::read_to_string(dir.join("b.txt")).unwrap(), "mine\n");

        // The same branch again in the same minute: a second one, the first untouched.
        fs::write(first.join("a.txt"), "work in the first\n").unwrap();
        let second = add_worktree(&first, "other", "0925-2214", &worktrees).unwrap();
        assert_eq!(second, worktrees.join(repo_name).join("other-0925-2214-2"));
        assert_eq!(head(&second), "kibo/other-0925-2214-2");
        assert_eq!(fs::read_to_string(first.join("a.txt")).unwrap(), "work in the first\n");
    }

    #[test]
    fn a_leftover_folder_or_branch_of_the_name_is_passed_over() {
        let (dir, repo) = two_branches("branches-leftover");
        let worktrees = temp_dir("branches-leftover-home");
        let main = repo.find_commit(repo.refname_to_id("refs/heads/main").unwrap()).unwrap();
        repo.branch("kibo/main-t", &main, false).unwrap();
        fs::create_dir_all(worktrees.join(dir.file_name().unwrap()).join("main-t-2")).unwrap();

        let made = add_worktree(&dir, "main", "t", &worktrees).unwrap();
        assert_eq!(made.file_name().unwrap(), "main-t-3");
        assert_eq!(head(&made), "kibo/main-t-3");
    }

    #[test]
    fn a_branch_out_in_another_worktree_is_refused_from_either_side() {
        let (dir, _repo) = two_branches("branches-elsewhere");
        let worktree = add_worktree(&dir, "main", "t", &temp_dir("branches-elsewhere-home")).unwrap();

        let refused = checkout(&dir, "kibo/main-t");
        assert!(matches!(refused, Err(GitBranchError::CheckedOutElsewhere { ref branch, .. }) if branch == "kibo/main-t"));
        let refused = checkout(&worktree, "main");
        assert!(matches!(refused, Err(GitBranchError::CheckedOutElsewhere { ref branch, .. }) if branch == "main"));
        assert_eq!(head(&dir), "main");
        assert_eq!(head(&worktree), "kibo/main-t");
    }

    /// A worktree of `main` from `two_branches`, and its repository.
    fn worktree(label: &str) -> (PathBuf, Repository, PathBuf, Repository) {
        let (dir, repo) = two_branches(label);
        let path = add_worktree(&dir, "main", "t", &temp_dir(&format!("{label}-home"))).unwrap();
        let linked = Repository::open(&path).unwrap();
        (dir, repo, path, linked)
    }

    fn tip(repo: &Repository) -> Oid {
        repo.head().unwrap().target().unwrap()
    }

    #[test]
    fn a_clean_worktree_goes_with_its_branch_and_leaves_the_folder_it_came_from() {
        let (dir, repo, path, _linked) = worktree("remove-clean");
        fs::write(repo.path().join("info").join("exclude"), "*.log\n").unwrap();
        fs::write(path.join("build.log"), "ignored, not work\n").unwrap();
        fs::write(dir.join("b.txt"), "the main folder's own edit\n").unwrap();

        let state = worktree_state(&path).unwrap();
        assert_eq!(Path::new(&state.main).canonicalize().unwrap(), dir.canonicalize().unwrap());
        assert_eq!(state.branch.as_deref(), Some("kibo/main-t"));
        assert_eq!((state.dirty.len(), state.own_commits, state.keeps_branch), (0, 0, false));

        assert_eq!(remove_worktree(&path).unwrap(), None);
        assert!(!path.exists());
        assert!(repo.find_branch("kibo/main-t", BranchType::Local).is_err());
        assert_eq!(repo.worktrees().unwrap().len(), 0);
        assert_eq!(head(&dir), "main");
        assert_eq!(fs::read_to_string(dir.join("b.txt")).unwrap(), "the main folder's own edit\n");
    }

    #[test]
    fn uncommitted_work_keeps_the_worktree_whole() {
        let (_dir, repo, path, _linked) = worktree("remove-dirty");
        fs::write(path.join("a.txt"), "edited\n").unwrap();
        fs::write(path.join("new.txt"), "untracked\n").unwrap();

        assert_eq!(worktree_state(&path).unwrap().dirty, vec!["a.txt".to_string(), "new.txt".to_string()]);
        let refused = remove_worktree(&path);
        assert!(matches!(refused, Err(GitBranchError::Dirty(ref paths)) if paths.len() == 2), "{refused:?}");
        assert_eq!(fs::read_to_string(path.join("a.txt")).unwrap(), "edited\n");
        assert!(repo.find_branch("kibo/main-t", BranchType::Local).is_ok());
        assert_eq!(repo.worktrees().unwrap().len(), 1);
    }

    #[test]
    fn commits_only_the_worktree_has_keep_its_branch() {
        let (_dir, repo, path, linked) = worktree("remove-commits");
        let parent = tip(&linked);
        let made = commit(&linked, "HEAD", Some(parent), &[("a.txt", "a\n"), ("b.txt", "b\n"), ("d.txt", "d\n")]);
        linked.checkout_head(Some(CheckoutBuilder::new().force())).unwrap();
        let state = worktree_state(&path).unwrap();
        assert_eq!((state.own_commits, state.keeps_branch), (1, true));

        assert_eq!(remove_worktree(&path).unwrap(), Some("kibo/main-t".to_string()));
        assert!(!path.exists());
        let kept = repo.find_branch("kibo/main-t", BranchType::Local).unwrap();
        assert_eq!(kept.get().target(), Some(made));
    }

    /// Pushed or merged, the commits are held elsewhere: the branch may go.
    #[test]
    fn commits_a_remote_also_has_are_not_the_worktree_s_own() {
        let (_dir, repo, path, linked) = worktree("remove-pushed");
        let made = commit(&linked, "HEAD", Some(tip(&linked)), &[("a.txt", "a\n"), ("b.txt", "b\n")]);
        linked.checkout_head(Some(CheckoutBuilder::new().force())).unwrap();
        repo.reference("refs/remotes/origin/kibo/main-t", made, false, "").unwrap();

        assert_eq!(worktree_state(&path).unwrap().own_commits, 0);
        assert_eq!(remove_worktree(&path).unwrap(), None);
        assert!(repo.find_branch("kibo/main-t", BranchType::Local).is_err());
    }

    #[test]
    fn a_branch_the_user_made_is_kept_even_with_nothing_on_it() {
        let (_dir, repo) = two_branches("remove-users");
        let path = temp_dir("remove-users-home").join("theirs");
        let main = repo.find_commit(repo.refname_to_id("refs/heads/main").unwrap()).unwrap();
        let branch = repo.branch("theirs", &main, false).unwrap();
        let mut options = WorktreeAddOptions::new();
        options.reference(Some(branch.get()));
        repo.worktree("theirs", &path, Some(&options)).unwrap();

        assert_eq!(remove_worktree(&path).unwrap(), Some("theirs".to_string()));
        assert!(!path.exists());
        assert!(repo.find_branch("theirs", BranchType::Local).is_ok());
    }

    #[test]
    fn commits_on_no_branch_are_refused() {
        let (_dir, _repo, path, linked) = worktree("remove-detached");
        let loose = commit(&linked, "refs/heads/scratch", Some(tip(&linked)), &[("a.txt", "a\n"), ("b.txt", "b\n")]);
        linked.set_head_detached(loose).unwrap();
        linked.find_branch("scratch", BranchType::Local).unwrap().delete().unwrap();

        let state = worktree_state(&path).unwrap();
        assert_eq!((state.branch, state.own_commits), (None, 1));
        assert!(matches!(remove_worktree(&path), Err(GitBranchError::Unreachable(1))));
        assert!(path.exists());
    }

    #[test]
    fn a_locked_worktree_is_left_alone() {
        let (_dir, repo, path, _linked) = worktree("remove-locked");
        repo.find_worktree("main-t").unwrap().lock(Some("in use")).unwrap();
        assert!(matches!(remove_worktree(&path), Err(GitBranchError::Locked(ref why)) if why == "in use"));
        assert!(path.exists());
    }

    /// The path comes from the window: only a folder its repository has
    /// registered as a worktree is removed — not the main folder, not a plain
    /// one, and not one whose `.git` file merely points into a repository.
    #[test]
    fn only_a_registered_worktree_is_removed() {
        let (dir, _repo, path, _linked) = worktree("remove-foreign");
        let impostor = temp_dir("remove-foreign-impostor");
        fs::copy(path.join(".git"), impostor.join(".git")).unwrap();
        fs::write(impostor.join("mine.txt"), "not a worktree's\n").unwrap();

        assert!(matches!(remove_worktree(&dir), Err(GitBranchError::NotAWorktree)));
        assert!(matches!(remove_worktree(&temp_dir("remove-foreign-plain")), Err(GitBranchError::NotARepository)));
        let refused = remove_worktree(&impostor);
        assert!(matches!(refused, Err(GitBranchError::NotAWorktree) | Err(GitBranchError::Dirty(_))), "{refused:?}");
        assert!(dir.exists() && path.exists() && impostor.join("mine.txt").exists());
    }

    #[test]
    fn a_plain_folder_has_no_branches() {
        assert!(matches!(list(&temp_dir("branches-plain")), Err(GitBranchError::NotARepository)));
    }
}
