//! Helpers shared by unit tests across modules. Compiled only under `cfg(test)`.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// A fresh directory under this test run's folder, unique per call.
///
/// No cleanup guard: a test that fails leaves its directory behind on purpose —
/// on a filesystem test that is usually the only evidence of what went wrong.
/// It stays until the next test run starts, which removes the folders of every
/// run that is over (see `run_root`).
pub fn temp_dir(label: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);

    let n = N.fetch_add(1, Ordering::Relaxed);
    let dir = run_root().join(format!("{label}-{n}"));
    std::fs::create_dir_all(&dir).expect("temp dir is creatable");
    dir
}

/// `kibo-tests/<pid>-<nanos>` under the system temp dir: this process's folder,
/// holding a locked `.lock` for as long as the process lives.
///
/// The first call sweeps the folders whose lock nobody holds — runs that ended,
/// passed or crashed, since the OS drops a lock with its process. Without it each
/// `cargo test` left a thousand folders behind, and a mutation run one set per mutant.
fn run_root() -> &'static Path {
    static ROOT: OnceLock<(PathBuf, File)> = OnceLock::new();
    &ROOT
        .get_or_init(|| {
            let base = std::env::temp_dir().join("kibo-tests");
            for entry in std::fs::read_dir(&base).into_iter().flatten().flatten() {
                // No `.lock` yet: a run between creating its folder and locking it.
                let Ok(lock) = File::open(entry.path().join(".lock")) else { continue };
                if lock.try_lock().is_ok() {
                    drop(lock);
                    let _ = std::fs::remove_dir_all(entry.path());
                }
            }
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock is after the epoch")
                .as_nanos();
            let root = base.join(format!("{}-{nanos}", std::process::id()));
            std::fs::create_dir_all(&root).expect("test run dir is creatable");
            let lock = File::create(root.join(".lock")).expect("test run lock is creatable");
            lock.lock().expect("test run lock is takeable");
            (root, lock)
        })
        .0
}

/// Runs `f` against a throwaway app directory with a working keychain, holding
/// the app-directory lock for the whole call.
///
/// Everything under `infra::app_dir` — settings, sealed credentials, logs — is
/// process-global state, so tests that touch it cannot run beside each other.
/// One helper rather than a copy per store, for the reason given on
/// `app_dir::test_support`.
pub fn with_app_dir<T>(label: &str, f: impl FnOnce() -> T) -> T {
    let _guard = crate::infra::app_dir::test_support::lock();
    crate::infra::app_dir::test_support::install(temp_dir(label));
    crate::infra::master_key::install_working_keychain_for_tests();
    f()
}
