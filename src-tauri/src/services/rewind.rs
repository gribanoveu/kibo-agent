//! Rewinding the files of the open folder — the rules are `domain::rewind`'s;
//! this reads what the files are now and writes them back.

use std::fs;

use crate::domain::rewind::{self, FileChange, FileRewind, FileState, Skip};
use crate::domain::tools::ToolScope;
use crate::infra::file_history;
use crate::services::ai_tools::resolve::resolve_writable;

/// What a rewind of `changes` would do, file by file, touching nothing.
pub fn preview(scope: &ToolScope, changes: &[FileChange]) -> Vec<FileRewind> {
    rewind::plan(changes, |path| current(scope, path), file_history::has)
}

/// Does it. Each file is looked at again right before it is written — the
/// preview may be minutes old — and one that changed meanwhile is left as it
/// is, like one that failed to write. Returns every file with how it went.
pub fn apply(scope: &ToolScope, changes: &[FileChange]) -> Vec<FileRewind> {
    preview(scope, changes)
        .into_iter()
        .map(|mut file| {
            if file.skip.is_none() {
                file.skip = restore(scope, &file).err();
            }
            file
        })
        .collect()
}

fn restore(scope: &ToolScope, file: &FileRewind) -> Result<(), Skip> {
    if current(scope, &file.path) != file.expected {
        return Err(Skip::ChangedSince);
    }
    let path = resolve_writable(scope, &file.path).map_err(|_| Skip::UnsafePath)?;
    match &file.target {
        FileState::Absent => fs::remove_file(&path).map_err(|_| Skip::WriteFailed),
        FileState::Stored { hash } => {
            let content = file_history::load(hash).ok_or(Skip::Expired)?;
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|_| Skip::WriteFailed)?;
            }
            fs::write(&path, content).map_err(|_| Skip::WriteFailed)
        }
        FileState::Unstored => Err(Skip::NotKept),
    }
}

/// The file as it is now. Any size is hashed: this is compared, not kept.
fn current(scope: &ToolScope, path: &str) -> FileState {
    let Ok(at) = resolve_writable(scope, path) else { return FileState::Unstored };
    match fs::read(&at) {
        Ok(content) => FileState::Stored { hash: file_history::hash(&content) },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => FileState::Absent,
        Err(_) => FileState::Unstored,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::rewind::Action;
    use crate::testing::{temp_dir, with_app_dir};

    fn kept(content: &str) -> FileState {
        FileState::Stored { hash: file_history::keep(content.as_bytes()).unwrap() }
    }

    fn change(path: &str, before: FileState, after: FileState) -> FileChange {
        FileChange { path: path.into(), before, after }
    }

    #[test]
    fn files_go_back_created_ones_go_and_deleted_ones_return() {
        with_app_dir("rewind-apply", || {
            let root = temp_dir("rewind-apply-root");
            fs::write(root.join("a.rs"), "new").unwrap();
            fs::write(root.join("made.rs"), "made").unwrap();
            let scope = ToolScope::new(&root).unwrap();
            let changes = [
                change("a.rs", kept("old"), kept("new")),
                change("made.rs", FileState::Absent, kept("made")),
                change("gone/deep.rs", kept("was here"), FileState::Absent),
            ];

            let preview = preview(&scope, &changes);
            assert!(preview.iter().all(|f| f.skip.is_none()), "{preview:?}");
            assert_eq!(fs::read_to_string(root.join("a.rs")).unwrap(), "new", "a preview touches nothing");

            let done = apply(&scope, &changes);
            assert!(done.iter().all(|f| f.skip.is_none()), "{done:?}");
            assert_eq!(fs::read_to_string(root.join("a.rs")).unwrap(), "old");
            assert!(!root.join("made.rs").exists());
            assert_eq!(fs::read_to_string(root.join("gone/deep.rs")).unwrap(), "was here");
            assert_eq!(done.iter().map(|f| f.action).collect::<Vec<_>>(), [Action::Modified, Action::Deleted, Action::Created]);
        });
    }

    /// The user's own edit after the agent's is theirs: left, and said so.
    #[test]
    fn a_file_changed_since_is_not_written_over() {
        with_app_dir("rewind-mine", || {
            let root = temp_dir("rewind-mine-root");
            fs::write(root.join("a.rs"), "mine").unwrap();
            let scope = ToolScope::new(&root).unwrap();
            let done = apply(&scope, &[change("a.rs", kept("old"), kept("new"))]);
            assert_eq!(done[0].skip, Some(Skip::ChangedSince));
            assert_eq!(fs::read_to_string(root.join("a.rs")).unwrap(), "mine");
        });
    }

    /// Changed between the preview and the rewind itself.
    #[test]
    fn each_file_is_checked_again_right_before_it_is_written() {
        with_app_dir("rewind-late", || {
            let root = temp_dir("rewind-late-root");
            fs::write(root.join("a.rs"), "new").unwrap();
            let scope = ToolScope::new(&root).unwrap();
            let changes = [change("a.rs", kept("old"), kept("new"))];
            let file = preview(&scope, &changes).remove(0);
            fs::write(root.join("a.rs"), "typed meanwhile").unwrap();
            assert_eq!(restore(&scope, &file), Err(Skip::ChangedSince));
            assert_eq!(fs::read_to_string(root.join("a.rs")).unwrap(), "typed meanwhile");
        });
    }

    #[test]
    fn a_path_out_of_the_folder_is_never_written() {
        with_app_dir("rewind-escape", || {
            let root = temp_dir("rewind-escape-root");
            let scope = ToolScope::new(&root).unwrap();
            let done = apply(&scope, &[change("../escaped.rs", kept("x"), FileState::Absent)]);
            assert_eq!(done[0].skip, Some(Skip::UnsafePath));
            assert!(!root.parent().unwrap().join("escaped.rs").exists());
        });
    }
}
