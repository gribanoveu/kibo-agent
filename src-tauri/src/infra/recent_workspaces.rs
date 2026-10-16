//! `recent.json`: the folders opened lately, the last one first — what the
//! window reopens at launch and offers to switch to.
//!
//! Its own file rather than a field of `settings.json`: it is rewritten on
//! every folder opened, and a write that often has no business near the
//! user's provider configuration.

use std::fs;
use std::path::{Path, PathBuf};

use crate::domain::settings::RecentFoldersError;
use crate::infra::app_dir;

const FILE: &str = "recent.json";
/// Enough to switch between the projects someone is in this week.
pub const KEEP: usize = 8;

/// `opened` goes first; an earlier mention of it goes away; the oldest past
/// `KEEP` fall off.
pub fn remember(list: &[String], opened: &str) -> Vec<String> {
    std::iter::once(opened.to_string())
        .chain(list.iter().filter(|path| *path != opened).cloned())
        .take(KEEP)
        .collect()
}

fn path() -> Result<PathBuf, String> {
    Ok(app_dir::dir()?.join(FILE))
}

/// Folders that still exist, the last opened first. A missing or damaged
/// file is an empty list: losing it costs one folder picked by hand, and
/// nothing else is kept here.
pub fn load() -> Vec<String> {
    let Ok(text) = path().and_then(|p| fs::read_to_string(p).map_err(|e| e.to_string())) else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<String>>(&text)
        .unwrap_or_default()
        .into_iter()
        // `\\?\C:\…` is how older builds saved a folder on Windows;
        // it is opened under its plain name now, so the old entry is dropped.
        .filter(|folder| !folder.starts_with(r"\\?\") && Path::new(folder).is_dir())
        .collect()
}

pub fn record(opened: &str) -> Result<(), RecentFoldersError> {
    save(&remember(&load(), opened))
}

/// Drops `gone` from the list: the entry only, never the folder — its chats,
/// its index and whatever is in it stay where they are. One that is not in the
/// list is not an error: two open windows, and the other dropped it first.
/// Its worktrees go with it: the menu draws them under it, and left behind
/// they would surface as rows of their own.
pub fn forget(gone: &str) -> Result<(), RecentFoldersError> {
    let list = load();
    let gone = Path::new(gone);
    // Compared through `Path`, as `commands::git::removable` compares them: the
    // same folder spelled with or without its trailing separator is one entry.
    let left: Vec<&String> = list
        .iter()
        .filter(|path| {
            let path = Path::new(path.as_str());
            path != gone && crate::infra::git_head::worktree_of(path).as_deref() != Some(gone)
        })
        .collect();
    save(&left)
}

fn save(list: &impl serde::Serialize) -> Result<(), RecentFoldersError> {
    app_dir::ensure().map_err(RecentFoldersError)?;
    let text = serde_json::to_string_pretty(list).map_err(|e| RecentFoldersError(e.to_string()))?;
    let path = path().map_err(RecentFoldersError)?;
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
    fn the_folder_opened_goes_first_once() {
        assert_eq!(remember(&list(&["/a", "/b", "/c"]), "/b"), list(&["/b", "/a", "/c"]));
        assert_eq!(remember(&[], "/a"), list(&["/a"]));
    }

    #[test]
    fn the_oldest_fall_off() {
        let full: Vec<String> = (0..KEEP).map(|i| format!("/{i}")).collect();
        let next = remember(&full, "/new");
        assert_eq!(next.len(), KEEP);
        assert_eq!(next[0], "/new");
        assert!(!next.contains(&format!("/{}", KEEP - 1)));
    }

    #[test]
    fn recorded_folders_come_back_and_gone_ones_do_not() {
        with_app_dir("recent-workspaces", || {
            let kept = std::env::temp_dir();
            let kept = kept.to_str().unwrap();
            assert!(load().is_empty(), "nothing opened yet");
            record("/no/such/folder/anymore").unwrap();
            record(kept).unwrap();
            assert_eq!(load(), list(&[kept]));
        });
    }

    /// An entry an older build saved under `\\?\` is dropped: the folder is
    /// opened under its plain name now, and would be listed twice.
    #[test]
    fn a_folder_saved_under_the_prefix_is_dropped() {
        with_app_dir("recent-prefixed", || {
            let kept = std::env::temp_dir().display().to_string();
            app_dir::ensure().unwrap();
            fs::write(path().unwrap(), serde_json::to_string(&[format!(r"\\?\{kept}"), kept.clone()]).unwrap()).unwrap();
            assert_eq!(load(), list(&[&kept]));
        });
    }

    /// The entry goes and the folder stays; an entry that is not in the list
    /// at all changes nothing.
    #[test]
    fn a_forgotten_folder_leaves_the_list_and_the_disk_alone() {
        with_app_dir("recent-forget", || {
            let one = temp_dir("recent-forget-one");
            let two = temp_dir("recent-forget-two");
            let (one, two) = (one.display().to_string(), two.display().to_string());
            app_dir::ensure().unwrap();
            fs::write(path().unwrap(), serde_json::to_string(&[one.clone(), two.clone()]).unwrap()).unwrap();

            forget(&one).unwrap();
            assert_eq!(load(), list(&[&two]));
            assert!(Path::new(&one).is_dir(), "the folder itself is left as it is");

            forget("/no/such/folder/in/the/list").unwrap();
            assert_eq!(load(), list(&[&two]), "an entry that is not there changes nothing");
        });
    }

    /// A project's worktrees leave with it; another project's stay.
    #[test]
    fn a_forgotten_project_takes_its_worktrees_along() {
        with_app_dir("recent-forget-worktrees", || {
            let main = crate::infra::paths::canonicalize_plain(&temp_dir("recent-forget-main")).unwrap();
            let repo = git2::Repository::init(&main).unwrap();
            repo.set_head("refs/heads/main").unwrap();
            let tree = repo.find_tree(repo.index().unwrap().write_tree().unwrap()).unwrap();
            let sig = git2::Signature::now("Test", "test@example.com").unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[]).unwrap();
            let home = crate::infra::paths::canonicalize_plain(&temp_dir("recent-forget-main-home")).unwrap();
            let linked = crate::infra::git_branches::add_worktree(&main, "main", "t", &home).unwrap();
            let other = crate::infra::paths::canonicalize_plain(&temp_dir("recent-forget-other")).unwrap();
            let [main, linked, other] = [main, linked, other].map(|p| p.display().to_string());
            app_dir::ensure().unwrap();
            fs::write(path().unwrap(), serde_json::to_string(&[&linked, &main, &other]).unwrap()).unwrap();

            forget(&main).unwrap();
            assert_eq!(load(), list(&[&other]));
            assert!(Path::new(&linked).is_dir(), "the worktree itself is left as it is");
        });
    }

    /// The menu hands back the path it was given, and a folder spelled with a
    /// trailing separator is not a second entry.
    #[test]
    fn a_folder_spelled_with_a_trailing_separator_is_the_same_entry() {
        with_app_dir("recent-forget-spelled", || {
            let folder = temp_dir("recent-forget-spelled-folder").display().to_string();
            app_dir::ensure().unwrap();
            fs::write(path().unwrap(), serde_json::to_string(&[format!("{folder}/")]).unwrap()).unwrap();

            forget(&folder).unwrap();
            assert!(load().is_empty());
        });
    }

    #[test]
    fn a_damaged_file_is_an_empty_list() {
        with_app_dir("recent-damaged", || {
            app_dir::ensure().unwrap();
            fs::write(path().unwrap(), "{not json").unwrap();
            assert!(load().is_empty());
            let kept = std::env::temp_dir();
            record(kept.to_str().unwrap()).unwrap();
            assert_eq!(load().len(), 1, "and the next open writes over it");
        });
    }
}
