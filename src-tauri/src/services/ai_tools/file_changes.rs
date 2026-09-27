//! What a writing call did to each file it touched, as `domain::rewind` reads
//! it: the file before the call and after, its content kept in
//! `infra::file_history`.
//!
//! The files are known from the call's own arguments — a path, or every file
//! under a directory being moved or deleted. A command is not here: see the
//! module doc of `domain::rewind`.

use std::fs;
use std::path::PathBuf;

use crate::domain::rewind::{FileChange, FileState};
use crate::domain::tools::{ToolCall, ToolScope};
use crate::infra::file_history;

use super::resolve::{relative_to_root, resolve_existing, resolve_writable};
use super::tools::move_path::files_under;

/// Larger than this, a file is not kept: it cannot be put back.
pub const MAX_FILE_BYTES: u64 = 2 << 20;
/// Kept per side of one call — a directory of many files keeps what fits.
pub const MAX_CALL_BYTES: u64 = 4 << 20;
pub const MAX_FILES: usize = 64;

/// The files a call is about to change, as they are before it runs.
pub struct Watched {
    files: Vec<(String, PathBuf)>,
    before: Vec<FileState>,
}

/// `None` for a call that writes no file, or whose paths do not resolve —
/// it will fail on them itself.
pub fn before(scope: &ToolScope, call: &ToolCall) -> Option<Watched> {
    let files = touched(scope, call)?;
    let before = states(&files);
    Some(Watched { files, before })
}

/// What changed, each file whose state moved; nothing for a call that failed
/// before touching anything.
pub fn after(watched: Watched) -> Vec<FileChange> {
    let after = states(&watched.files);
    watched
        .files
        .into_iter()
        .zip(watched.before)
        .zip(after)
        .filter(|((_, before), after)| before != after || *before == FileState::Unstored)
        .filter(|((_, before), after)| !(*before == FileState::Absent && *after == FileState::Absent))
        .map(|(((path, _), before), after)| FileChange { path, before, after })
        .collect()
}

fn touched(scope: &ToolScope, call: &ToolCall) -> Option<Vec<(String, PathBuf)>> {
    let one = |path: &str| -> Option<Vec<(String, PathBuf)>> {
        let at = resolve_writable(scope, path).ok()?;
        Some(vec![(relative_to_root(scope, &at).ok()?, at)])
    };
    let under = |dir: &PathBuf| -> Vec<(String, PathBuf)> {
        files_under(dir).into_iter().filter_map(|f| Some((relative_to_root(scope, &f).ok()?, f))).collect()
    };
    match call {
        ToolCall::WriteFile(args) => one(&args.path),
        ToolCall::EditFile(args) => one(&args.path),
        ToolCall::DeleteFile(args) => one(&args.path),
        ToolCall::DeleteDirectory(args) => Some(under(&resolve_existing(scope, &args.path).ok()?)),
        ToolCall::Move(args) => {
            let from = resolve_existing(scope, &args.path).ok()?;
            let to = resolve_writable(scope, &args.new_path).ok()?;
            if !from.is_dir() {
                return Some([one(&args.path)?, one(&args.new_path)?].concat());
            }
            // Each file where it is and where it will be.
            let sources = under(&from);
            let targets = sources.iter().filter_map(|(_, f)| {
                let moved = to.join(f.strip_prefix(&from).ok()?);
                Some((relative_to_root(scope, &moved).ok()?, moved))
            });
            Some(sources.iter().cloned().chain(targets).collect())
        }
        _ => None,
    }
}

/// Each file's state, kept while the call's budget lasts.
fn states(files: &[(String, PathBuf)]) -> Vec<FileState> {
    let (mut bytes, mut kept) = (0u64, 0usize);
    files
        .iter()
        .map(|(_, path)| match fs::metadata(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => FileState::Absent,
            Ok(meta) if meta.is_file() && meta.len() <= MAX_FILE_BYTES && bytes + meta.len() <= MAX_CALL_BYTES && kept < MAX_FILES => {
                let Some(hash) = fs::read(path).ok().and_then(|content| file_history::keep(&content)) else {
                    return FileState::Unstored;
                };
                bytes += meta.len();
                kept += 1;
                FileState::Stored { hash }
            }
            _ => FileState::Unstored,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::tools::{DeleteDirectoryArgs, MoveArgs, WriteFileArgs};
    use crate::testing::{temp_dir, with_app_dir};

    fn stored(content: &[u8]) -> FileState {
        FileState::Stored { hash: file_history::hash(content) }
    }

    /// Watches `call`, lets `act` stand in for the tool, and returns what changed.
    fn around(root: &std::path::Path, call: ToolCall, act: impl FnOnce()) -> Vec<FileChange> {
        let scope = ToolScope::new(root).unwrap();
        let watched = before(&scope, &call).expect("a writing call");
        act();
        after(watched)
    }

    fn write(path: &str) -> ToolCall {
        ToolCall::WriteFile(WriteFileArgs { path: path.into(), content: String::new() })
    }

    #[test]
    fn a_write_is_recorded_before_and_after_with_both_kept() {
        with_app_dir("changes-write", || {
            let root = temp_dir("changes-write-root");
            fs::write(root.join("a.rs"), "old").unwrap();
            let changes = around(&root, write("a.rs"), || fs::write(root.join("a.rs"), "new").unwrap());
            assert_eq!(changes, [FileChange { path: "a.rs".into(), before: stored(b"old"), after: stored(b"new") }]);
            assert_eq!(file_history::load(&file_history::hash(b"old")).unwrap(), b"old");

            let created = around(&root, write("sub/new.rs"), || {
                fs::create_dir(root.join("sub")).unwrap();
                fs::write(root.join("sub/new.rs"), "x").unwrap();
            });
            assert_eq!(created[0].before, FileState::Absent);
            assert_eq!(created[0].path, "sub/new.rs");
        });
    }

    #[test]
    fn a_call_that_changed_nothing_records_nothing() {
        with_app_dir("changes-none", || {
            let root = temp_dir("changes-none-root");
            fs::write(root.join("a.rs"), "same").unwrap();
            assert!(around(&root, write("a.rs"), || {}).is_empty());
            assert!(around(&root, write("never.rs"), || {}).is_empty());
            let scope = ToolScope::new(&root).unwrap();
            assert!(before(&scope, &ToolCall::GitStatus).is_none(), "writes nothing");
            assert!(before(&scope, &write("../out.rs")).is_none(), "outside: the call fails on it itself");
        });
    }

    #[test]
    fn a_directory_moved_or_deleted_is_each_file_in_it() {
        with_app_dir("changes-dir", || {
            let root = temp_dir("changes-dir-root");
            fs::create_dir_all(root.join("src/deep")).unwrap();
            fs::write(root.join("src/a.rs"), "a").unwrap();
            fs::write(root.join("src/deep/b.rs"), "b").unwrap();

            let moved = around(&root, ToolCall::Move(MoveArgs { path: "src".into(), new_path: "lib".into() }), || {
                fs::rename(root.join("src"), root.join("lib")).unwrap()
            });
            let summary: Vec<(&str, &FileState, &FileState)> = moved.iter().map(|c| (c.path.as_str(), &c.before, &c.after)).collect();
            assert_eq!(
                summary,
                [
                    ("src/a.rs", &stored(b"a"), &FileState::Absent),
                    ("src/deep/b.rs", &stored(b"b"), &FileState::Absent),
                    ("lib/a.rs", &FileState::Absent, &stored(b"a")),
                    ("lib/deep/b.rs", &FileState::Absent, &stored(b"b")),
                ]
            );

            let deleted = around(&root, ToolCall::DeleteDirectory(DeleteDirectoryArgs { path: "lib".into(), recursive: Some(true) }), || {
                fs::remove_dir_all(root.join("lib")).unwrap()
            });
            assert_eq!(deleted.len(), 2);

            fs::write(root.join("one.rs"), "1").unwrap();
            let renamed = around(&root, ToolCall::Move(MoveArgs { path: "one.rs".into(), new_path: "two.rs".into() }), || {
                fs::rename(root.join("one.rs"), root.join("two.rs")).unwrap()
            });
            assert_eq!(
                renamed,
                [
                    FileChange { path: "one.rs".into(), before: stored(b"1"), after: FileState::Absent },
                    FileChange { path: "two.rs".into(), before: FileState::Absent, after: stored(b"1") },
                ]
            );
            assert!(deleted.iter().all(|c| c.after == FileState::Absent));
        });
    }

    /// Past the budget a file is still listed — as not kept, so a rewind
    /// says it cannot put it back rather than leaving it out unsaid.
    #[test]
    fn what_does_not_fit_is_listed_as_not_kept() {
        with_app_dir("changes-budget", || {
            let root = temp_dir("changes-budget-root");
            fs::write(root.join("big.bin"), vec![0u8; MAX_FILE_BYTES as usize + 1]).unwrap();
            let changes = around(&root, write("big.bin"), || fs::write(root.join("big.bin"), "small").unwrap());
            assert_eq!(changes[0].before, FileState::Unstored);
            assert_eq!(changes[0].after, stored(b"small"));
            // Big before and after: nothing to compare, so it is listed.
            fs::write(root.join("big.bin"), vec![0u8; MAX_FILE_BYTES as usize + 1]).unwrap();
            let still_big = around(&root, write("big.bin"), || fs::write(root.join("big.bin"), vec![1u8; MAX_FILE_BYTES as usize + 1]).unwrap());
            assert_eq!(still_big, [FileChange { path: "big.bin".into(), before: FileState::Unstored, after: FileState::Unstored }]);

            fs::create_dir(root.join("many")).unwrap();
            for i in 0..=MAX_FILES {
                fs::write(root.join(format!("many/{i:03}")), i.to_string()).unwrap();
            }
            let deleted = around(&root, ToolCall::DeleteDirectory(DeleteDirectoryArgs { path: "many".into(), recursive: Some(true) }), || {
                fs::remove_dir_all(root.join("many")).unwrap()
            });
            assert_eq!(deleted.len(), MAX_FILES + 1);
            assert_eq!(deleted.iter().filter(|c| c.before == FileState::Unstored).count(), 1);
        });
    }
}
