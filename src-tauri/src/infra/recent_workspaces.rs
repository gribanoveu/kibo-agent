//! `recent.json`: the folders opened lately, the last one first — what the
//! window reopens at launch and offers to switch to.
//!
//! Its own file rather than a field of `settings.json`: it is rewritten on
//! every folder opened, and a write that often has no business near the
//! user's provider configuration.

use std::fs;
use std::path::{Path, PathBuf};

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

pub fn record(opened: &str) -> Result<(), String> {
    app_dir::ensure()?;
    let text = serde_json::to_string_pretty(&remember(&load(), opened)).map_err(|e| e.to_string())?;
    app_dir::write_private(&path()?, text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::with_app_dir;

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
