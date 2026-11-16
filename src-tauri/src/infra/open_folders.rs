//! `workspaces.json`: the folders open in the window, the last added first.
//!
//! Unlike `recent.json` this is not a menu history — it is the set the window
//! works with (F-8, docs/10-multi-workspace.md): every add or close rewrites
//! it, and at launch the app reopens every folder still on disk (F-8.8). No
//! length cap: the list is the user's open projects, not a convenience list,
//! and closing a folder is a deliberate act.
//!
//! The error shape is shared with `recent_workspaces` — one String-newtype for
//! the app's own small folder-list files.

use std::fs;
use std::path::{Path, PathBuf};

use crate::domain::settings::RecentFoldersError;
use crate::infra::app_dir;

const FILE: &str = "workspaces.json";

/// `opened` goes first; an earlier mention of it goes away.
pub fn remember(list: &[String], opened: &str) -> Vec<String> {
    std::iter::once(opened.to_string())
        .chain(list.iter().filter(|path| *path != opened).cloned())
        .collect()
}

fn path() -> Result<PathBuf, RecentFoldersError> {
    app_dir::dir().map_err(|e| RecentFoldersError(e.to_string())).map(|dir| dir.join(FILE))
}

/// Open folders that still exist, the last added first. A missing or damaged
/// file is an empty list: the window starts closed, as after a fresh install.
pub fn load() -> Vec<String> {
    let Ok(text) = path().and_then(|p| fs::read_to_string(p).map_err(|e| RecentFoldersError(e.to_string()))) else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<String>>(&text)
        .unwrap_or_default()
        .into_iter()
        // `\\?\C:\…` is how older builds saved a folder on Windows; the plain
        // spelling is what the app opens and registers under.
        .filter(|folder| !folder.starts_with(r"\\?\") && Path::new(folder).is_dir())
        .collect()
}

pub fn record(opened: &str) -> Result<(), RecentFoldersError> {
    save(&remember(&load(), opened))
}

/// Drops `gone` from the open set. Only the exact folder — a worktree is open
/// in its own right and stays open when the main checkout closes (F-8
/// invariant 4: closing removes what is its, nothing of a neighbour's). A
/// folder that is not in the list is not an error.
pub fn remove(gone: &str) -> Result<(), RecentFoldersError> {
    let gone = Path::new(gone);
    let left: Vec<String> = load().into_iter().filter(|path| Path::new(path.as_str()) != gone).collect();
    save(&left)
}

fn save(list: &impl serde::Serialize) -> Result<(), RecentFoldersError> {
    app_dir::ensure().map_err(RecentFoldersError)?;
    let text = serde_json::to_string_pretty(list).map_err(|e| RecentFoldersError(e.to_string()))?;
    let path = path()?;
    app_dir::write_private(&path, text.as_bytes()).map_err(RecentFoldersError)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{temp_dir, with_app_dir};

    fn list(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_folder_added_goes_first_once() {
        assert_eq!(remember(&list(&["/a", "/b"]), "/b"), list(&["/b", "/a"]));
        assert_eq!(remember(&[], "/a"), list(&["/a"]));
        // no cap: open projects are not a convenience history
        let full: Vec<String> = (0..20).map(|i| format!("/{i}")).collect();
        let next = remember(&full, "/new");
        assert_eq!(next.len(), 21);
        assert_eq!(next[0], "/new");
    }

    #[test]
    fn recorded_folders_come_back_and_gone_ones_do_not() {
        with_app_dir("open-folders", || {
            let kept = std::env::temp_dir();
            let kept = kept.to_str().unwrap();
            assert!(load().is_empty(), "nothing open yet");
            record("/no/such/folder/anymore").unwrap();
            record(kept).unwrap();
            assert_eq!(load(), list(&[kept]));
        });
    }

    /// Closing a worktree does not close the project it belongs to, and the
    /// other way round: only the named folder leaves.
    #[test]
    fn removing_a_folder_leaves_its_worktree_and_neighbours() {
        with_app_dir("open-folders-remove", || {
            let main = crate::infra::paths::canonicalize_plain(&temp_dir("open-folders-main")).unwrap();
            let repo = git2::Repository::init(&main).unwrap();
            repo.set_head("refs/heads/main").unwrap();
            let tree = repo.find_tree(repo.index().unwrap().write_tree().unwrap()).unwrap();
            let sig = git2::Signature::now("Test", "test@example.com").unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[]).unwrap();
            let home = crate::infra::paths::canonicalize_plain(&temp_dir("open-folders-main-home")).unwrap();
            let linked = crate::infra::git_branches::add_worktree(&main, "main", "t", &home).unwrap();
            let other = crate::infra::paths::canonicalize_plain(&temp_dir("open-folders-other")).unwrap();
            let [main, linked, other] = [main, linked, other].map(|p| p.display().to_string());
            record(&linked).unwrap();
            record(&main).unwrap();
            record(&other).unwrap();

            remove(&main).unwrap();
            assert_eq!(load(), list(&[other.as_str(), linked.as_str()]));

            remove(&linked).unwrap();
            assert_eq!(load(), list(&[other.as_str()]));
        });
    }

    /// The entry goes and the folder stays; one that is not in the list is
    /// not an error either.
    #[test]
    fn a_removed_entry_leaves_the_folder_alone() {
        with_app_dir("open-folders-gone", || {
            let one = temp_dir("open-folders-one");
            let one_shown = one.display().to_string();
            record(&one_shown).unwrap();

            remove(&one_shown).unwrap();
            assert!(load().is_empty());
            assert!(one.is_dir(), "the folder itself is left as it is");

            remove("/no/such/folder/in/the/list").unwrap();
            assert!(load().is_empty());
        });
    }

    /// A folder spelled with a trailing separator is the same entry.
    #[test]
    fn a_folder_spelled_with_a_trailing_separator_is_the_same_entry() {
        with_app_dir("open-folders-spelled", || {
            let folder = temp_dir("open-folders-spelled-folder").display().to_string();
            record(&folder).unwrap();

            remove(&format!("{folder}/")).unwrap();
            assert!(load().is_empty());
        });
    }

    #[test]
    fn a_damaged_file_is_an_empty_list() {
        with_app_dir("open-folders-damaged", || {
            app_dir::ensure().unwrap();
            fs::write(path().unwrap(), "{not json").unwrap();
            assert!(load().is_empty());
            let kept = std::env::temp_dir();
            record(kept.to_str().unwrap()).unwrap();
            assert_eq!(load().len(), 1, "and the next open writes over it");
        });
    }
}
