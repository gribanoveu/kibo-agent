//! `<app dir>/file-history/`: the content of files as the agent found and left
//! them, kept under their hash for a rewind (`domain::rewind`).
//!
//! Content-addressed, so a file written ten times over a morning costs one
//! copy per version, and the chat only carries hashes. Nothing ties a copy to
//! a chat: a copy untouched for [`RETENTION_DAYS`] is cleared, once a run,
//! and a rewind that needs it says it is gone. Kept again, a copy is fresh
//! again. Best-effort, like the command output store: a copy that cannot be
//! written leaves that file out of a rewind, never fails the write.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use crate::infra::app_dir;

const DIR: &str = "file-history";
pub const RETENTION_DAYS: u64 = 30;
const RETENTION: Duration = Duration::from_secs(RETENTION_DAYS * 24 * 60 * 60);

/// The name content is kept under.
pub fn hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// Keeps `bytes` and returns the hash it is under; `None` when it could not
/// be written.
pub fn keep(bytes: &[u8]) -> Option<String> {
    let dir = dir()?;
    let hash = hash(bytes);
    let path = dir.join(&hash);
    if path.exists() {
        // Fresh again: it is in use.
        let _ = fs::File::options().write(true).open(&path).and_then(|f| f.set_modified(SystemTime::now()));
    } else {
        app_dir::write_private(&path, bytes).ok()?;
    }
    Some(hash)
}

/// The content kept under `hash`, if it still is.
pub fn load(hash: &str) -> Option<Vec<u8>> {
    fs::read(app_dir::dir().ok()?.join(DIR).join(valid(hash)?)).ok()
}

pub fn has(hash: &str) -> bool {
    valid(hash).and_then(|h| Some(app_dir::dir().ok()?.join(DIR).join(h))).is_some_and(|p| p.is_file())
}

/// A hash is a file name here, so it has to look like one of ours: a hash
/// out of a saved chat is not trusted to be.
fn valid(hash: &str) -> Option<&str> {
    (hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())).then_some(hash)
}

fn dir() -> Option<PathBuf> {
    static PRUNED: AtomicBool = AtomicBool::new(false);
    let dir = app_dir::ensure().ok()?.join(DIR);
    fs::create_dir_all(&dir).ok()?;
    if !PRUNED.swap(true, Ordering::Relaxed) {
        prune(&dir);
    }
    Some(dir)
}

fn prune(dir: &std::path::Path) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|modified| now.duration_since(modified).is_ok_and(|age| age > RETENTION));
        if old {
            let _ = fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::with_app_dir;

    #[test]
    fn content_is_kept_once_under_its_hash_and_read_back() {
        with_app_dir("file-history", || {
            let a = keep(b"fn main() {}\n").expect("kept");
            assert_eq!(a, hash(b"fn main() {}\n"));
            assert_eq!(keep(b"fn main() {}\n").as_deref(), Some(a.as_str()));
            assert_eq!(load(&a).as_deref(), Some(&b"fn main() {}\n"[..]));
            assert!(has(&a));
            assert!(!has(&hash(b"never kept")));
        });
    }

    /// A hash comes back from a saved chat: never a path.
    #[test]
    fn a_hash_that_is_not_one_reads_nothing() {
        with_app_dir("file-history-bad", || {
            keep(b"x").expect("kept");
            fs::write(app_dir::dir().unwrap().join("settings.json"), "{}").unwrap();
            for bad in ["../settings.json", "", "zz", &"a".repeat(63)] {
                assert!(load(bad).is_none() && !has(bad), "{bad}");
            }
        });
    }

    #[test]
    fn an_old_copy_is_cleared_and_one_kept_again_is_fresh() {
        with_app_dir("file-history-prune", || {
            let dir = app_dir::ensure().unwrap().join(DIR);
            let old = keep(b"old").unwrap();
            let past = SystemTime::now() - RETENTION - Duration::from_secs(60);
            let set_old = |h: &str| fs::File::options().write(true).open(dir.join(h)).unwrap().set_modified(past).unwrap();
            set_old(&old);
            prune(&dir);
            assert!(!has(&old), "past retention");

            let reused = keep(b"reused").unwrap();
            set_old(&reused);
            keep(b"reused").unwrap();
            prune(&dir);
            assert!(has(&reused), "kept again, fresh again");
        });
    }
}
