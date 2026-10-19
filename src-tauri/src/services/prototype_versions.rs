//! Restoring a Prototype chat's page to a version it presented
//! (`docs/31-prototype-role.md`, decision 8): the folder becomes what that
//! version holds, and the result is a version of its own — later versions
//! stay, so going back can be undone by going forward. A folder changed since
//! its last version is kept first, so a restore loses nothing.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Mutex;

use crate::domain::prototype::{PrototypeError, Restored};
use crate::infra::prototype_store;

/// The chats whose folder a turn, a restore or a rewind is working in now.
static IN_USE: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// The chat's folder held for one piece of work, until dropped.
pub struct Claim(String);

impl Drop for Claim {
    fn drop(&mut self) {
        if let Ok(mut in_use) = IN_USE.lock() {
            in_use.remove(&self.0);
        }
    }
}

/// Holds the chat's folder for a turn, a restore or a rewind; `Busy` while
/// another holds it. Refused rather than waited for: a turn runs for minutes,
/// and the window already keeps the user from asking for both at once — this
/// is for when it does not.
pub fn claim(chat_id: &str) -> Result<Claim, PrototypeError> {
    let mut in_use = IN_USE.lock().map_err(|_| PrototypeError::Busy)?;
    if !in_use.insert(chat_id.to_string()) {
        return Err(PrototypeError::Busy);
    }
    Ok(Claim(chat_id.to_string()))
}

/// [`restore`] of the chat `chat_id`'s folder, held against its turns.
pub fn restore_chat(chat_id: &str, n: u32) -> Result<Restored, PrototypeError> {
    let _claim = claim(chat_id)?;
    restore(&prototype_store::files_dir(chat_id)?, n)
}

pub fn restore(files: &Path, n: u32) -> Result<Restored, PrototypeError> {
    // Asked first: a version that is not there must not cost a snapshot.
    prototype_store::version_dir(files, n)?;
    let before = match prototype_store::latest(files)? {
        Some(latest) if prototype_store::same_as_version(files, latest)? => None,
        _ => Some(prototype_store::snapshot(files)?),
    };
    prototype_store::replace_with_version(files, n)?;
    let version = prototype_store::snapshot(files)?;
    Ok(Restored { from: n, version, before })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::tools::{EditFileArgs, FileEdit, ReadFileArgs, ReadFiles, ToolError, ToolScope};
    use crate::services::ai_tools::tools::{edit_file, read_file};
    use std::fs;
    use std::path::PathBuf;

    const ID: &str = "6b8d0f2a-4c6e-4a8b-9d1f-3e5a7c9b1d3f";

    fn chat(label: &str) -> (PathBuf, PathBuf) {
        let files = crate::testing::temp_dir(label).join("prototypes").join(ID).join("files");
        fs::create_dir_all(&files).unwrap();
        let versions = files.parent().unwrap().join("versions");
        (files, versions)
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }

    /// The folder becomes the version — its files back, what came since gone —
    /// and that is a version of its own; the later ones stay.
    #[test]
    fn a_restore_puts_the_version_back_as_a_new_version_and_keeps_the_later_ones() {
        let (files, versions) = chat("restore-back");
        fs::write(files.join("index.html"), "one").unwrap();
        prototype_store::snapshot(&files).unwrap();
        fs::write(files.join("index.html"), "two").unwrap();
        fs::write(files.join("extra.css"), "b{}").unwrap();
        prototype_store::snapshot(&files).unwrap();

        let restored = restore(&files, 1).unwrap();
        assert_eq!(restored, Restored { from: 1, version: 3, before: None });
        assert_eq!(fs::read_to_string(files.join("index.html")).unwrap(), "one");
        assert!(!files.join("extra.css").exists(), "what came after version 1 is gone");
        assert_eq!(names(&versions), ["1", "2", "3"]);
        assert_eq!(fs::read_to_string(versions.join("2").join("index.html")).unwrap(), "two");
        assert_eq!(fs::read_to_string(versions.join("3").join("index.html")).unwrap(), "one");
        assert!(!files.parent().unwrap().join(".files.old").exists() && !files.parent().unwrap().join(".files.partial").exists());
    }

    /// Changes the model made but never presented are kept as a version before
    /// they are replaced.
    #[test]
    fn what_changed_since_the_last_version_is_kept_first() {
        let (files, versions) = chat("restore-unsaved");
        fs::write(files.join("index.html"), "one").unwrap();
        prototype_store::snapshot(&files).unwrap();
        fs::write(files.join("index.html"), "edited, not presented").unwrap();

        let restored = restore(&files, 1).unwrap();
        assert_eq!(restored, Restored { from: 1, version: 3, before: Some(2) });
        assert_eq!(fs::read_to_string(versions.join("2").join("index.html")).unwrap(), "edited, not presented");
        assert_eq!(fs::read_to_string(files.join("index.html")).unwrap(), "one");
    }

    /// While a turn holds the folder, a restore is refused and touches nothing;
    /// once the turn lets go, it goes ahead.
    #[test]
    fn a_restore_waits_for_no_turn_and_touches_nothing_while_one_runs() {
        crate::testing::with_app_dir("restore-claimed", || {
            let id = "2e4a6c8e-0b1d-4f3a-9c5e-7a9b1d3f5a7c";
            let files = prototype_store::files_dir(id).unwrap();
            fs::write(files.join("index.html"), "one").unwrap();
            prototype_store::snapshot(&files).unwrap();
            fs::write(files.join("index.html"), "the turn is writing").unwrap();

            let turn = claim(id).unwrap();
            assert!(matches!(claim(id), Err(PrototypeError::Busy)), "one at a time");
            assert!(matches!(restore_chat(id, 1), Err(PrototypeError::Busy)));
            assert_eq!(fs::read_to_string(files.join("index.html")).unwrap(), "the turn is writing");
            assert!(claim("another-chat").is_ok(), "another chat's folder is its own");

            drop(turn);
            assert_eq!(restore_chat(id, 1).unwrap().from, 1);
            assert_eq!(fs::read_to_string(files.join("index.html")).unwrap(), "one");
        });
    }

    /// A version that is not there changes nothing and keeps nothing.
    #[test]
    fn a_version_that_is_not_there_is_refused_and_changes_nothing() {
        let (files, versions) = chat("restore-missing");
        fs::write(files.join("index.html"), "one").unwrap();
        prototype_store::snapshot(&files).unwrap();
        fs::write(files.join("index.html"), "edited").unwrap();
        assert!(matches!(restore(&files, 7), Err(PrototypeError::NoVersion(7))));
        assert_eq!(names(&versions), ["1"]);
        assert_eq!(fs::read_to_string(files.join("index.html")).unwrap(), "edited");
    }

    /// A rewind of the chat to before a message puts its folder back as the
    /// agent's rewind puts a repository back — and leaves the versions, which
    /// are history, as they are.
    #[test]
    fn a_rewind_puts_the_folder_back_and_leaves_the_versions() {
        crate::testing::with_app_dir("rewind-prototype", || {
            use crate::domain::rewind::{FileChange, FileState};
            use crate::infra::file_history;
            let (files, versions) = chat("rewind-prototype-chat");
            fs::write(files.join("index.html"), "before the message").unwrap();
            let before = file_history::keep(b"before the message").unwrap();
            fs::write(files.join("index.html"), "after it").unwrap();
            let after = file_history::keep(b"after it").unwrap();
            fs::write(files.join("new.css"), "a{}").unwrap();
            let css = file_history::keep(b"a{}").unwrap();
            prototype_store::snapshot(&files).unwrap();

            let changes = vec![
                FileChange { path: "index.html".into(), before: FileState::Stored { hash: before }, after: FileState::Stored { hash: after } },
                FileChange { path: "new.css".into(), before: FileState::Absent, after: FileState::Stored { hash: css } },
            ];
            let done = crate::services::rewind::apply(&ToolScope::new(&files).unwrap(), &changes);
            assert!(done.iter().all(|file| file.skip.is_none()), "{done:?}");
            assert_eq!(fs::read_to_string(files.join("index.html")).unwrap(), "before the message");
            assert!(!files.join("new.css").exists());
            assert_eq!(fs::read_to_string(versions.join("1").join("index.html")).unwrap(), "after it");
            assert!(versions.join("1").join("new.css").is_file());
        });
    }

    /// The model's reading of the page is out of date once it is restored:
    /// an edit made from it is refused, as an edit of a file the user changed is.
    #[test]
    fn an_edit_from_a_reading_before_the_restore_is_refused() {
        let (files, _) = chat("restore-reads");
        fs::write(files.join("index.html"), "<h1>one</h1>").unwrap();
        prototype_store::snapshot(&files).unwrap();
        fs::write(files.join("index.html"), "<h1>two</h1>").unwrap();
        prototype_store::snapshot(&files).unwrap();
        let scope = ToolScope::new(&files).unwrap();
        let mut reads = ReadFiles::default();
        read_file::read_file(&scope, &ReadFileArgs { path: "index.html".into(), ..Default::default() }, &mut reads).unwrap();

        restore(&files, 1).unwrap();
        let edit = EditFileArgs { path: "index.html".into(), edits: vec![FileEdit { old: "two".into(), new: "three".into() }] };
        let refused = edit_file::edit_file(&scope, &edit, &mut reads).unwrap_err();
        assert!(matches!(refused, ToolError::FileChangedSinceRead(_)), "{refused:?}");
    }
}
