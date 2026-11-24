//! The branch checked out in a working tree — what the chat header shows
//! under the chat's title.

use std::path::{Path, PathBuf};

use git2::Repository;

/// The checked-out branch of the repository `root` is in, or the short commit
/// id when HEAD is detached. `None` outside a repository: an ordinary folder
/// is not an error, and the header then shows the folder instead.
///
/// A repository with no commits yet still has a branch — HEAD names it
/// before anything is on it — so that case reads the name from HEAD itself.
pub fn current_branch(root: &Path) -> Option<String> {
    checked_out_branch(root).or_else(|| {
        let repo = Repository::discover(root).ok()?;
        // Bound before returning: the head borrows `repo`.
        let short = repo.head().ok()?.target().map(|id| id.to_string().chars().take(7).collect());
        short
    })
}

/// The branch checked out, and only a branch: `None` for a detached HEAD,
/// where [`current_branch`] shows the short commit id instead, and outside a
/// repository. A pull request comes from a branch.
pub fn checked_out_branch(root: &Path) -> Option<String> {
    let repo = Repository::discover(root).ok()?;
    let branch = match repo.head() {
        Ok(head) if head.is_branch() => head.shorthand().ok().map(String::from),
        Ok(_) => None,
        Err(_) => {
            let head = repo.find_reference("HEAD").ok()?;
            let target = head.symbolic_target().ok()??;
            target.strip_prefix("refs/heads/").map(String::from)
        }
    };
    branch
}

/// The commit `HEAD` points at in the repository `root` is in — the full
/// sha, so the answer survives branch switches and worktrees, unlike the
/// branch name. `None` outside a repository and before the first commit,
/// where HEAD names a branch that has nothing on it.
pub fn current_commit(root: &Path) -> Option<String> {
    let repo = Repository::discover(root).ok()?;
    // Bound before returning: the `?` temporaries borrow `repo` and would
    // outlive it, as in `current_branch`.
    let commit = repo.head().ok()?.target().map(|id| id.to_string());
    commit
}

/// What of the repository a server could have heard about: the branch
/// checked out and where each remote's copy of it points — a push or a fetch
/// moves those. Staging, a local commit or an editor's `git status` move
/// nothing here. `None` outside a repository and on a detached HEAD.
pub fn pushed_state(root: &Path) -> Option<(String, Vec<String>)> {
    let branch = checked_out_branch(root)?;
    let repo = Repository::discover(root).ok()?;
    let remotes: Vec<String> = repo
        .references_glob(&format!("refs/remotes/*/{branch}"))
        .ok()?
        .flatten()
        .filter_map(|r| Some(format!("{} {}", r.name().ok()?, r.target()?)))
        .collect();
    // ponytail: in git's order, which packing the refs may change — one
    // needless read of the server then; sort here if that ever shows.
    Some((branch, remotes))
}

/// The main working tree of the repository `root` is a linked worktree of;
/// `None` for the main one itself, and outside a repository. Spelled as the
/// app spells a folder: git2 answers `C:/…` on Windows, which matches no
/// open or recent folder (`C:\…`).
pub fn worktree_of(root: &Path) -> Option<PathBuf> {
    let repo = Repository::discover(root).ok()?;
    if !repo.is_worktree() {
        return None;
    }
    // The shared `.git` sits in the main working tree.
    let main = repo.commondir().parent()?;
    Some(crate::infra::paths::canonicalize_plain(main).unwrap_or_else(|_| main.to_path_buf()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::temp_dir;
    use git2::Signature;

    fn commit(repo: &Repository) -> git2::Oid {
        let tree = repo.find_tree(repo.index().unwrap().write_tree().unwrap()).unwrap();
        let sig = Signature::now("Test", "test@example.com").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[]).unwrap()
    }

    /// The strip reads the server again only for what the server could know:
    /// a switch of branch or a push, not a commit or a staged file.
    #[test]
    fn what_was_pushed_moves_with_a_push_and_a_switch_only() {
        let dir = temp_dir("git-head-pushed");
        let repo = Repository::init(&dir).unwrap();
        let first = commit(&repo);
        let branch = checked_out_branch(&dir).unwrap();
        let start = pushed_state(&dir).unwrap();
        assert_eq!(start, (branch.clone(), vec![]));

        repo.reference(&format!("refs/remotes/origin/{branch}"), first, true, "push").unwrap();
        repo.reference("refs/remotes/origin/other", first, true, "another branch").unwrap();
        let pushed = pushed_state(&dir).unwrap();
        assert_eq!(pushed.1, [format!("refs/remotes/origin/{branch} {first}")]);

        let sig = Signature::now("Test", "test@example.com").unwrap();
        let tree = repo.find_commit(first).unwrap().tree().unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "local", &tree, &[&repo.find_commit(first).unwrap()]).unwrap();
        std::fs::write(dir.join("a.txt"), "a").unwrap();
        repo.index().unwrap().add_path(Path::new("a.txt")).unwrap();
        assert_eq!(pushed_state(&dir), Some(pushed.clone()), "a commit and a staged file are not news to the server");

        // A second push moves the ref that is already there.
        let local = repo.head().unwrap().target().unwrap();
        repo.reference(&format!("refs/remotes/origin/{branch}"), local, true, "push again").unwrap();
        assert_eq!(pushed_state(&dir).unwrap().1, [format!("refs/remotes/origin/{branch} {local}")]);

        repo.branch("feature/y", &repo.find_commit(first).unwrap(), false).unwrap();
        repo.set_head("refs/heads/feature/y").unwrap();
        assert_eq!(pushed_state(&dir), Some(("feature/y".to_string(), vec![])));

        repo.set_head_detached(first).unwrap();
        assert_eq!(pushed_state(&dir), None);
    }

    /// A pull request comes from a branch: a detached HEAD has none, where the
    /// header shows the short commit id.
    #[test]
    fn a_detached_head_checks_out_no_branch() {
        let dir = temp_dir("git-head-checked-out");
        let repo = Repository::init(&dir).unwrap();
        assert!(checked_out_branch(&dir).is_some(), "an unborn branch is still a branch");
        let id = commit(&repo);
        let branch = checked_out_branch(&dir).unwrap();
        assert_eq!(Some(branch.clone()), current_branch(&dir));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        assert_eq!(checked_out_branch(&dir.join("sub")), Some(branch), "a subfolder is in the repository");

        repo.set_head_detached(id).unwrap();
        assert_eq!(checked_out_branch(&dir), None);
        assert!(current_branch(&dir).is_some(), "the header still shows the commit");
        assert_eq!(checked_out_branch(&temp_dir("git-head-checked-out-plain")), None);
    }

    #[test]
    fn names_the_branch_even_before_the_first_commit() {
        let dir = temp_dir("git-head-branch");
        let repo = Repository::init(&dir).unwrap();
        repo.set_head("refs/heads/feature").unwrap();
        assert_eq!(current_branch(&dir).as_deref(), Some("feature"));

        commit(&repo);
        std::fs::create_dir(dir.join("sub")).unwrap();
        assert_eq!(current_branch(&dir.join("sub")).as_deref(), Some("feature"));
    }

    #[test]
    fn the_current_commit_is_the_heads_sha_and_nothing_before_it() {
        let dir = temp_dir("git-head-commit");
        let repo = Repository::init(&dir).unwrap();
        assert_eq!(current_commit(&dir), None, "no commits yet");

        let id = commit(&repo);
        assert_eq!(current_commit(&dir).as_deref(), Some(id.to_string().as_str()));

        assert_eq!(current_commit(&temp_dir("git-head-commit-plain")), None);
    }

    #[test]
    fn a_detached_head_is_its_short_commit_and_a_plain_folder_nothing() {
        let dir = temp_dir("git-head-detached");
        let repo = Repository::init(&dir).unwrap();
        let id = commit(&repo);
        repo.set_head_detached(id).unwrap();
        assert_eq!(current_branch(&dir), Some(id.to_string()[..7].to_string()));

        assert_eq!(current_branch(&temp_dir("git-head-plain")), None);
    }

    #[test]
    fn a_linked_worktree_names_its_main_one_and_the_main_one_nothing() {
        let dir = temp_dir("git-head-worktree");
        let repo = Repository::init(&dir).unwrap();
        repo.set_head("refs/heads/main").unwrap();
        commit(&repo);
        let linked = crate::infra::git_branches::add_worktree(&dir, "main", "t", &temp_dir("git-head-worktree-home")).unwrap();
        std::fs::create_dir(linked.join("sub")).unwrap();

        let main = dir.canonicalize().unwrap();
        assert_eq!(worktree_of(&linked).map(|p| p.canonicalize().unwrap()), Some(main.clone()));
        assert_eq!(worktree_of(&linked.join("sub")).map(|p| p.canonicalize().unwrap()), Some(main));
        assert_eq!(worktree_of(&dir), None);
        assert_eq!(worktree_of(&temp_dir("git-head-worktree-plain")), None);
    }
}
