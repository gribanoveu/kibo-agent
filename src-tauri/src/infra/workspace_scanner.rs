//! Gitignore-aware walk of the repository, on the `ignore` crate.
//!
//! One walker for everything that needs to enumerate the tree: `grep`,
//! `listFiles`, and the index layer later on.
//!
//! **Dotfiles are visible here, unlike in Alfa Atlas.** That walk ran with
//! `hidden(true)`, which is right for a documentation editor and wrong for a
//! coding agent: it hides `.github/workflows`, `.gitignore`, `.eslintrc`,
//! `.env.example` — exactly the files an agent is asked about. `.git` itself is
//! skipped explicitly instead, because that is the one hidden directory nobody
//! means: thousands of loose objects, all binary, none of them source.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::SystemTime;

use ignore::{WalkBuilder, WalkState};

use crate::domain::tools::canonicalize_plain;

#[derive(Debug, Clone)]
pub struct ScannedFile {
    pub path: PathBuf,
    /// From the same `metadata` call as `size`, so a caller can cheaply
    /// pre-filter "definitely unchanged since I last looked" before reading
    /// any content.
    pub modified: SystemTime,
    pub size: u64,
}

/// A file or a directory. Carries no mtime or size — the listing that wants
/// directories does not care, and the walk that cares does not want them.
#[derive(Debug, Clone)]
pub struct ScannedEntry {
    pub path: PathBuf,
    pub is_dir: bool,
}

/// Every file under `root`, sorted by path so the order is reproducible.
///
/// `max_depth` follows `WalkBuilder`'s convention: `root` is depth 0, its
/// direct children depth 1. `None` is unlimited.
pub fn scan_files(root: &Path, max_depth: Option<usize>) -> io::Result<Vec<ScannedFile>> {
    let root = canonicalize_plain(root)?;

    let (tx, rx) = mpsc::channel::<ScannedFile>();
    builder(&root, max_depth).build_parallel().run(|| {
        let tx = tx.clone();
        let root = root.clone();
        Box::new(move |entry| {
            let Ok(entry) = entry else {
                return WalkState::Continue;
            };
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                return WalkState::Continue;
            }
            let path = entry.path().to_path_buf();
            // A symlink can point out of the tree; the walk followed it, the
            // caller's boundary did not move.
            if !path.starts_with(&root) {
                return WalkState::Continue;
            }
            let Ok(meta) = std::fs::metadata(&path) else {
                return WalkState::Continue;
            };
            let _ = tx.send(ScannedFile {
                path,
                modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                size: meta.len(),
            });
            WalkState::Continue
        })
    });
    drop(tx);

    let mut files: Vec<ScannedFile> = rx.into_iter().collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

/// Files *and* directories under `root`, sorted by path. `root` itself is
/// never an entry — a listing describes what is inside, not the thing asked
/// about.
///
/// Serial rather than parallel: a listing is depth-capped in practice, and the
/// ordering work is the same either way.
pub fn scan_entries(root: &Path, max_depth: Option<usize>) -> io::Result<Vec<ScannedEntry>> {
    let root = canonicalize_plain(root)?;

    let mut entries = Vec::new();
    for entry in builder(&root, max_depth).build() {
        let Ok(entry) = entry else { continue };
        let Some(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path().to_path_buf();
        if path == root || !path.starts_with(&root) {
            continue;
        }
        entries.push(ScannedEntry {
            path,
            is_dir: file_type.is_dir(),
        });
    }

    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(entries)
}

fn builder(root: &Path, max_depth: Option<usize>) -> WalkBuilder {
    let mut builder = WalkBuilder::new(root);
    builder
        // See the module comment: an agent needs dotfiles.
        .hidden(false)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        // `ignore` defaults to honouring `.gitignore` only inside a git
        // repository. Alfa Atlas left that default because its editor is
        // always opened on one; here a user can point the agent at any folder,
        // and a `.gitignore` sitting in it is an explicit statement about what
        // is noise whether or not `git init` was ever run.
        .require_git(false)
        // Ancestor `.gitignore` rules apply even when the repository sits
        // inside a larger ignored tree.
        .parents(true)
        .max_depth(max_depth)
        .filter_entry(|entry| entry.file_name() != ".git");
    builder
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::temp_dir;

    fn write(root: &Path, relative: &str, body: &str) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().expect("has a parent")).expect("dirs are creatable");
        std::fs::write(path, body).expect("file is writable");
    }

    fn names(root: &Path, files: &[ScannedFile]) -> Vec<String> {
        files
            .iter()
            .map(|f| {
                f.path
                    .strip_prefix(root)
                    .expect("under root")
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect()
    }

    #[test]
    fn finds_files_in_a_reproducible_order() {
        let dir = temp_dir("scan-order");
        write(&dir, "b.txt", "b");
        write(&dir, "a.txt", "a");
        write(&dir, "sub/c.txt", "c");
        let root = canonicalize_plain(&dir).unwrap();

        let files = scan_files(&root, None).unwrap();

        assert_eq!(names(&root, &files), ["a.txt", "b.txt", "sub/c.txt"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn honours_gitignore() {
        let dir = temp_dir("scan-gitignore");
        write(&dir, ".gitignore", "target/\n*.log\n");
        write(&dir, "keep.rs", "fn main() {}");
        write(&dir, "noisy.log", "spam");
        write(&dir, "target/debug/build", "binary");
        let root = canonicalize_plain(&dir).unwrap();

        let files = scan_files(&root, None).unwrap();

        assert_eq!(names(&root, &files), [".gitignore", "keep.rs"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The deliberate divergence from Alfa Atlas. An agent asked "why is CI
    /// failing" has to be able to see the workflow file.
    #[test]
    fn dotfiles_are_visible() {
        let dir = temp_dir("scan-dotfiles");
        write(&dir, ".github/workflows/ci.yml", "on: push");
        write(&dir, ".eslintrc", "{}");
        let root = canonicalize_plain(&dir).unwrap();

        let files = scan_files(&root, None).unwrap();

        assert_eq!(names(&root, &files), [".eslintrc", ".github/workflows/ci.yml"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The one hidden directory nobody means: thousands of loose objects, all
    /// binary. Visible dotfiles must not drag it in.
    #[test]
    fn the_git_directory_is_skipped() {
        let dir = temp_dir("scan-git");
        write(&dir, ".git/objects/ab/cdef", "binary");
        write(&dir, ".git/config", "[core]");
        write(&dir, "src/main.rs", "fn main() {}");
        let root = canonicalize_plain(&dir).unwrap();

        let files = scan_files(&root, None).unwrap();

        assert_eq!(names(&root, &files), ["src/main.rs"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn depth_caps_the_walk() {
        let dir = temp_dir("scan-depth");
        write(&dir, "top.txt", "0");
        write(&dir, "one/mid.txt", "1");
        write(&dir, "one/two/deep.txt", "2");
        let root = canonicalize_plain(&dir).unwrap();

        assert_eq!(names(&root, &scan_files(&root, Some(1)).unwrap()), ["top.txt"]);
        assert_eq!(
            names(&root, &scan_files(&root, Some(2)).unwrap()),
            ["one/mid.txt", "top.txt"]
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn entries_include_directories_but_never_the_root() {
        let dir = temp_dir("scan-entries");
        write(&dir, "sub/file.txt", "x");
        let root = canonicalize_plain(&dir).unwrap();

        let entries = scan_entries(&root, None).unwrap();
        let listed: Vec<(String, bool)> = entries
            .iter()
            .map(|e| {
                (
                    e.path
                        .strip_prefix(&root)
                        .expect("under root")
                        .to_string_lossy()
                        .replace('\\', "/"),
                    e.is_dir,
                )
            })
            .collect();

        assert_eq!(
            listed,
            [("sub".to_string(), true), ("sub/file.txt".to_string(), false)]
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_root_is_an_io_error() {
        let missing = temp_dir("scan-missing").join("gone");
        assert!(scan_files(&missing, None).is_err());
    }
}
