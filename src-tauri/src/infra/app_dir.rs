//! Where the app keeps what it owns outside the repository: settings, sealed
//! credentials, the master-key fallback, logs.
//!
//! One module rather than a helper per store, because the directory is also
//! the test seam. Every store under it has to be redirectable to a throwaway
//! path, and a private copy of that seam per store only protects a store
//! against itself — two suites redirecting a process-global at the same time
//! is a flake, not a test. The lock lives here for the same reason.

use std::fs;
use std::path::{Path, PathBuf};

use crate::domain::settings::AppDirError;

/// The app directory. Not created — see [`ensure`].
#[cfg(not(test))]
pub fn dir() -> Result<PathBuf, AppDirError> {
    dirs::home_dir().map(|home| home.join(".kibo")).ok_or(AppDirError::NoHome)
}

/// Under test the real home is never used: the suite installs a throwaway
/// directory, and a test that forgot to gets an error rather than the
/// developer's own settings and keys.
#[cfg(test)]
pub fn dir() -> Result<PathBuf, AppDirError> {
    test_support::installed()
}

/// The app directory, created if missing and narrowed to owner-only.
///
/// It holds sealed credentials and the master-key fallback, so group and
/// other have no business reading it. `create_dir_all` applies the process
/// umask, which on a default account yields `0o755` — hence the explicit
/// tightening, on every call so a directory left behind by an older build is
/// fixed too. A failure to narrow is swallowed: not being able to reduce
/// permissions is never a reason to refuse to start.
pub fn ensure() -> Result<PathBuf, String> {
    let dir = dir()?;
    fs::create_dir_all(&dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(&dir) {
            let mut perms = meta.permissions();
            if perms.mode() & 0o077 != 0 {
                perms.set_mode(0o700);
                let _ = fs::set_permissions(&dir, perms);
            }
        }
    }

    Ok(dir)
}

/// Writes `bytes` so that no reader ever sees a half-written file, owner-only.
///
/// Write-in-place is the obvious thing and is wrong for both files this is
/// used for: a process that dies mid-write leaves settings that no longer
/// parse, or a sealed blob whose authentication tag no longer matches — and
/// in the second case every key in it is gone, not merely unreadable. A
/// temporary file plus a rename means the worst case is the previous
/// contents.
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let dir = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;

    // Beside the target, so the rename below stays within one filesystem.
    // One per write, not per process: two threads writing at once would
    // otherwise share it, and one's rename takes the file from under the other.
    static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let write = WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temp = path.with_extension(format!("{}-{write}.tmp", std::process::id()));
    fs::write(&temp, bytes).map_err(|e| format!("could not write {}: {e}", temp.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&temp, fs::Permissions::from_mode(0o600));
    }

    fs::rename(&temp, path).map_err(|e| {
        let _ = fs::remove_file(&temp);
        format!("could not replace {}: {e}", path.display())
    })
}

#[cfg(test)]
pub mod test_support {
    use super::*;
    use std::sync::{Mutex, MutexGuard};

    static LOCK: Mutex<()> = Mutex::new(());
    static INSTALLED: Mutex<Option<PathBuf>> = Mutex::new(None);

    pub fn installed() -> Result<PathBuf, AppDirError> {
        INSTALLED.lock().unwrap_or_else(|e| e.into_inner()).clone().ok_or(AppDirError::NotInstalled)
    }

    /// Serialises every test that touches the app directory, across modules.
    /// Hold the guard for the whole test: the installed directory and the
    /// master key are both process-global.
    pub fn lock() -> MutexGuard<'static, ()> {
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn install(dir: PathBuf) {
        *INSTALLED.lock().unwrap_or_else(|e| e.into_inner()) = Some(dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::temp_dir;

    /// Two threads writing the same file — a turn and Settings — both succeed,
    /// and what is left is one of the two whole.
    #[test]
    fn concurrent_private_writes_all_succeed() {
        let path = temp_dir("app-dir-concurrent").join("settings.json");
        let writer = |byte: u8| {
            let path = path.clone();
            std::thread::spawn(move || (0..40).map(|_| write_private(&path, &[byte; 4096])).collect::<Vec<_>>())
        };
        let (a, b) = (writer(b'a'), writer(b'b'));
        for result in a.join().unwrap().into_iter().chain(b.join().unwrap()) {
            assert_eq!(result, Ok(()));
        }
        let left = fs::read(&path).unwrap();
        assert!(left == [b'a'; 4096] || left == [b'b'; 4096]);
    }

    #[test]
    fn a_private_write_replaces_the_previous_contents_and_leaves_no_temp_file() {
        let dir = temp_dir("app-dir-write");
        let path = dir.join("settings.json");

        write_private(&path, b"first").unwrap();
        write_private(&path, b"second").unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"second");
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name != "settings.json")
            .collect();
        assert!(leftovers.is_empty(), "left behind {leftovers:?}");
    }

    #[cfg(unix)]
    #[test]
    fn a_private_write_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let path = temp_dir("app-dir-perms").join("secret.enc");
        write_private(&path, b"sealed").unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "readable by someone else");
    }
}
