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
    let repo = Repository::discover(root).ok()?;
    // Bound before returning: the references borrow `repo`, and a tail
    // expression's temporaries would outlive it.
    let branch = match repo.head() {
        Ok(head) if head.is_branch() => head.shorthand().ok().map(String::from),
        Ok(head) => head.target().map(|id| id.to_string().chars().take(7).collect()),
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

/// The main working tree of the repository `root` is a linked worktree of;
/// `None` for the main one itself, and outside a repository.
pub fn worktree_of(root: &Path) -> Option<PathBuf> {
    let repo = Repository::discover(root).ok()?;
    if !repo.is_worktree() {
        return None;
    }
    // The shared `.git` sits in the main working tree.
    repo.commondir().parent().map(Path::to_path_buf)
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
