//! The master key every sealed secret is encrypted under.
//!
//! It lives in a file beside the sealed blobs by default, and in the OS
//! keychain — macOS Keychain, Windows Credential Manager, Linux Secret Service
//! — when the user moves it there ([`move_to`]).
//!
//! Why the file is the default: on macOS a keychain item's ACL names the
//! binary that created it, and an ad-hoc signed build is a different binary
//! after every update, so the keychain asks again each time. A permission
//! prompt on first launch reads as distrust, and a "Deny" leaves every stored
//! key unreadable. The file is owner-only, which is what other CLI agents do.
//!
//! What the keychain adds: the file keeps the key in plaintext next to the
//! ciphertext it unlocks, so anything that can copy the app directory — a
//! backup, a synced home directory, "send me your config" — gets both halves.
//! The keychain removes the key from every file-copy route. It does **not**
//! defend against malware running as the same user: on all three platforms an
//! unlocked keychain is readable by that user's processes.
//!
//! ## A store that only pretends to work
//!
//! `keyring` 3.x makes its platform backends opt-in features; with none
//! enabled it compiles to an in-memory mock whose writes return `Ok(())` and
//! evaporate at process exit. A missing Secret Service on Linux fails less
//! quietly but just as fatally. So a successful write is never taken as proof:
//! a move writes the key and reads it back through a *separate* entry, which
//! the mock cannot satisfy because it keeps state per entry.
//!
//! ## Never minting a second key
//!
//! The marker file records where the key is. Once it says the keychain, a
//! keychain that cannot answer — a rebuilt binary refused by the item's ACL,
//! a denied prompt — is an error, not a reason to start over: minting a
//! replacement there would leave every sealed secret permanently unreadable.
//! For the same reason a move records the new place before it removes the
//! old copy.

use std::fs;
use std::sync::Mutex;

use aes_gcm::aead::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::infra::app_dir;

pub const KEY_LEN: usize = 32;
pub type MasterKey = Zeroizing<[u8; KEY_LEN]>;

const KEYRING_SERVICE: &str = "com.kibo.agent";
const KEYRING_USER: &str = "master-key";
const KEY_FILE: &str = "master.key";
/// Records *where* the key went, so a later run can tell "no key yet" from
/// "the key is somewhere I cannot read right now". Holds no key material.
const MARKER_FILE: &str = "master_key_store.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KeyStore {
    Keychain,
    File,
}

#[derive(Serialize, Deserialize)]
struct Marker {
    store: KeyStore,
}

static CACHE: Mutex<Option<[u8; KEY_LEN]>> = Mutex::new(None);

/// The master key, generating one on first use.
pub fn resolve() -> Result<MasterKey, String> {
    let mut cache = CACHE.lock().map_err(|_| "key cache is poisoned".to_string())?;
    if let Some(key) = *cache {
        return Ok(Zeroizing::new(key));
    }
    let key = load_or_create()?;
    *cache = Some(*key);
    Ok(key)
}

/// Where the key is kept. Reads only the marker, so asking never touches the
/// keychain; a key not made yet will go to the file.
pub fn store() -> KeyStore {
    recorded().unwrap_or(KeyStore::File)
}

/// Moves the key to `target`, making one first if there is none.
///
/// Each step is checked before the next, and the old copy goes last: a
/// failure anywhere leaves the key readable where the marker says it is.
pub fn move_to(target: KeyStore) -> Result<(), String> {
    let mut cache = CACHE.lock().map_err(|_| "key cache is poisoned".to_string())?;
    let key = match *cache {
        Some(key) => Zeroizing::new(key),
        None => load_or_create()?,
    };
    *cache = Some(*key);
    if recorded() == Some(target) {
        return Ok(());
    }

    match target {
        KeyStore::Keychain => {
            keychain_put(&key)?;
            // Read back through a separate entry — see "A store that only
            // pretends to work" above.
            if !matches!(keychain_get(), Ok(Some(ref back)) if **back == *key) {
                let _ = backend::delete(KEYRING_USER);
                return Err("the keychain did not give the key back — it stays in the file".to_string());
            }
            if let Err(e) = record(KeyStore::Keychain) {
                let _ = backend::delete(KEYRING_USER);
                return Err(e);
            }
            match fs::remove_file(app_dir::dir()?.join(KEY_FILE)) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(format!("the key is in the keychain, but its old file could not be removed: {e}")),
            }
        }
        KeyStore::File => {
            file_write(&key)?;
            if !matches!(file_read(), Ok(Some(ref back)) if **back == *key) {
                return Err("the key file did not read back — the key stays in the keychain".to_string());
            }
            record(KeyStore::File)?;
            backend::delete(KEYRING_USER)
                .map_err(|e| format!("the key is in the file, but the keychain copy could not be removed: {e}"))
        }
    }
}

/// A master key that resolves, for a suite that only needs sealed storage to
/// work. Which keychain a test doubles, and how it misbehaves, stays this
/// module's own business — see `crate::testing::with_app_dir`.
#[cfg(test)]
pub(crate) fn install_working_keychain_for_tests() {
    tests::install_double(tests::Mode::Working);
}

fn load_or_create() -> Result<MasterKey, String> {
    if recorded() == Some(KeyStore::Keychain) {
        match keychain_get() {
            Ok(Some(key)) => return Ok(key),
            // Gone for good, which a new key cannot make worse.
            Ok(None) => {}
            // Could not answer. See the module comment: minting a replacement
            // here is what destroys every sealed secret.
            Err(e) => {
                return Err(format!(
                    "the master key is in the keychain but could not be read ({e}). \
                     Refusing to generate a new one, which would make every stored \
                     secret unreadable."
                ));
            }
        }
    }

    if let Some(key) = file_read()? {
        let _ = record(KeyStore::File);
        return Ok(key);
    }

    let mut fresh = [0u8; KEY_LEN];
    OsRng.fill_bytes(&mut fresh);
    let key = Zeroizing::new(fresh);
    file_write(&key)?;
    let _ = record(KeyStore::File);
    Ok(key)
}

fn keychain_get() -> Result<Option<MasterKey>, String> {
    match backend::get(KEYRING_USER)? {
        Some(bytes) if bytes.len() == KEY_LEN => {
            let mut key = [0u8; KEY_LEN];
            key.copy_from_slice(&bytes);
            Ok(Some(Zeroizing::new(key)))
        }
        // Present but the wrong size is corruption, not absence — treating it
        // as absence would mint a second key over the top of the first.
        Some(_) => Err("the stored master key is not the right size".to_string()),
        None => Ok(None),
    }
}

fn keychain_put(key: &[u8; KEY_LEN]) -> Result<(), String> {
    backend::set(KEYRING_USER, key)
}

fn file_read() -> Result<Option<MasterKey>, String> {
    let path = app_dir::dir()?.join(KEY_FILE);
    match fs::read(&path) {
        Ok(bytes) if bytes.len() == KEY_LEN => {
            let mut key = [0u8; KEY_LEN];
            key.copy_from_slice(&bytes);
            Ok(Some(Zeroizing::new(key)))
        }
        Ok(_) => Err(format!("{} is not a valid master key", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("could not read the master key: {e}")),
    }
}

/// Owner-only, and written whole or not at all: a half-written key file is a
/// key lost.
fn file_write(key: &[u8; KEY_LEN]) -> Result<(), String> {
    let dir = app_dir::ensure()?;
    app_dir::write_private(&dir.join(KEY_FILE), key)
}

fn recorded() -> Option<KeyStore> {
    let path = app_dir::dir().ok()?.join(MARKER_FILE);
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str::<Marker>(&text).ok().map(|m| m.store)
}

/// Written after every successful resolution, so the record follows reality
/// rather than intent. A move needs to know it landed; a load can ignore it.
fn record(store: KeyStore) -> Result<(), String> {
    let dir = app_dir::ensure()?;
    let text = serde_json::to_string(&Marker { store }).map_err(|e| e.to_string())?;
    app_dir::write_private(&dir.join(MARKER_FILE), text.as_bytes())
}

/// The keychain, or an in-process double under test.
///
/// The suite must never touch a real keychain: it would prompt on macOS, fail
/// in CI, and — worst — could overwrite the developer's own key and leave every
/// secret they have stored undecryptable.
#[cfg(not(test))]
mod backend {
    use super::{KEYRING_SERVICE, KEYRING_USER};

    fn entry(user: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new(KEYRING_SERVICE, user).map_err(|e| e.to_string())
    }

    pub fn get(user: &str) -> Result<Option<Vec<u8>>, String> {
        match entry(user)?.get_secret() {
            Ok(bytes) => Ok(Some(bytes)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    pub fn set(user: &str, value: &[u8]) -> Result<(), String> {
        entry(user)?.set_secret(value).map_err(|e| e.to_string())
    }

    pub fn delete(user: &str) -> Result<(), String> {
        match entry(user)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }

    // Referenced so the constant is not dead in this build.
    #[allow(dead_code)]
    fn _key_user() -> &'static str {
        KEYRING_USER
    }
}

#[cfg(test)]
use tests::backend;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::app_dir;
    use crate::testing::temp_dir;
    use std::collections::HashMap;
    use std::sync::MutexGuard;

    /// How the doubled keychain behaves for the test in progress.
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub(super) enum Mode {
        /// A real backend: values persist and any entry can read them.
        Working,
        /// `keyring`'s mock: writes succeed and are kept per entry, so reading
        /// through a different entry finds nothing.
        Mock,
        /// Working, except that the *existing* key entry is refused.
        ///
        /// This is the macOS ACL case, and the shape matters: a keychain item's
        /// ACL names the binary that created it, so a rebuilt binary is refused
        /// on that item while still free to create and read its own new ones.
        /// A fresh write and read would pass and say nothing at all about the
        /// item that matters — which is exactly why the marker exists.
        Unreadable,
    }

    struct TestState {
        mode: Mode,
        store: HashMap<String, Vec<u8>>,
        /// Whether anything was ever written.
        touched: bool,
    }

    static STATE: Mutex<Option<TestState>> = Mutex::new(None);

    pub(crate) mod backend {
        use super::STATE;

        pub fn get(user: &str) -> Result<Option<Vec<u8>>, String> {
            let guard = STATE.lock().expect("state");
            let state = guard.as_ref().expect("test state is installed");
            match state.mode {
                // Only the key itself is refused.
                super::Mode::Unreadable if user == super::KEYRING_USER => {
                    Err("access denied by the keychain".into())
                }
                // The mock keeps a write inside the entry that made it, so a
                // separate read never sees it.
                super::Mode::Mock => Ok(None),
                _ => Ok(state.store.get(user).cloned()),
            }
        }

        pub fn set(user: &str, value: &[u8]) -> Result<(), String> {
            let mut guard = STATE.lock().expect("state");
            let state = guard.as_mut().expect("test state is installed");
            state.touched = true;
            state.store.insert(user.to_string(), value.to_vec());
            Ok(())
        }

        pub fn delete(user: &str) -> Result<(), String> {
            let mut guard = STATE.lock().expect("state");
            guard.as_mut().expect("state").store.remove(user);
            Ok(())
        }
    }

    /// Installs a fresh app directory and doubled keychain, and clears the
    /// cached key. Returns the guard that serialises against every other test
    /// using the app directory, in this module and outside it.
    fn setup(label: &str, mode: Mode) -> MutexGuard<'static, ()> {
        let guard = app_dir::test_support::lock();
        app_dir::test_support::install(temp_dir(label));
        install_double(mode);
        guard
    }

    /// Installs the doubled keychain on its own, for a suite that already
    /// holds the app-directory lock and only needs a key that resolves —
    /// see `crate::testing::with_app_dir`.
    pub(crate) fn install_double(mode: Mode) {
        *STATE.lock().expect("state") = Some(TestState {
            mode,
            store: HashMap::new(),
            touched: false,
        });
        *CACHE.lock().expect("cache") = None;
    }

    fn touched() -> bool {
        STATE.lock().unwrap().as_ref().unwrap().touched
    }

    fn set_mode(mode: Mode) {
        STATE.lock().expect("state").as_mut().expect("state").mode = mode;
        *CACHE.lock().expect("cache") = None;
    }

    fn keychain_holds(user: &str) -> bool {
        STATE.lock().unwrap().as_ref().unwrap().store.contains_key(user)
    }

    fn key_file() -> std::path::PathBuf {
        app_dir::dir().unwrap().join(KEY_FILE)
    }

    /// The point of the default: a first run never asks the keychain, so the
    /// system never asks the user.
    #[test]
    fn a_new_key_goes_to_the_file_without_touching_the_keychain() {
        let _serial = setup("mk-default", Mode::Working);
        assert_eq!(store(), KeyStore::File, "a key not made yet goes to the file");

        let first = resolve().expect("mints a key");
        *CACHE.lock().unwrap() = None;
        let second = resolve().expect("finds the same key again");

        assert_eq!(*first, *second);
        assert_eq!(recorded(), Some(KeyStore::File));
        assert_eq!(store(), KeyStore::File);
        assert!(key_file().exists());
        assert!(!touched(), "a default run must not write to the keychain");
    }

    #[test]
    fn a_key_moved_to_the_keychain_leaves_no_file_and_survives_a_restart() {
        let _serial = setup("mk-to-keychain", Mode::Working);
        let original = resolve().expect("mints a key");

        move_to(KeyStore::Keychain).expect("moves");

        assert_eq!(store(), KeyStore::Keychain);
        assert!(!key_file().exists(), "the plaintext copy must go");
        assert!(keychain_holds(KEYRING_USER));
        *CACHE.lock().unwrap() = None;
        assert_eq!(*resolve().expect("reads it from the keychain"), *original);
    }

    #[test]
    fn a_key_moved_back_to_the_file_leaves_the_keychain_empty() {
        let _serial = setup("mk-to-file", Mode::Working);
        let original = resolve().expect("mints a key");
        move_to(KeyStore::Keychain).expect("moves in");

        move_to(KeyStore::File).expect("moves out");

        assert_eq!(store(), KeyStore::File);
        assert!(!keychain_holds(KEYRING_USER));
        *CACHE.lock().unwrap() = None;
        assert_eq!(*resolve().expect("reads it from the file"), *original);
    }

    /// Before any secret was stored there is no key yet; a move makes one
    /// where it was asked to go.
    #[test]
    fn moving_before_there_is_a_key_makes_one_in_the_keychain() {
        let _serial = setup("mk-move-fresh", Mode::Working);

        move_to(KeyStore::Keychain).expect("moves");

        assert!(!key_file().exists());
        let key = resolve().expect("resolves");
        *CACHE.lock().unwrap() = None;
        assert_eq!(*resolve().expect("reads it from the keychain"), *key);
    }

    /// The trap the read-back exists for: a store whose writes return Ok(()) and
    /// evaporate. Moving into it would lose the key at the next exit.
    #[test]
    fn a_mock_keychain_is_refused_and_the_key_stays_in_the_file() {
        let _serial = setup("mk-mock", Mode::Mock);
        let original = resolve().expect("mints a key");

        assert!(move_to(KeyStore::Keychain).is_err());

        assert_eq!(store(), KeyStore::File);
        assert!(key_file().exists());
        *CACHE.lock().unwrap() = None;
        assert_eq!(*resolve().expect("still in the file"), *original);
    }

    /// The data-loss guard. The key is in the keychain, the keychain refuses to
    /// answer, and minting a replacement would leave every sealed secret
    /// unreadable — so this fails loudly instead.
    #[test]
    fn an_unreadable_keychain_refuses_to_mint_a_second_key() {
        let _serial = setup("mk-unreadable", Mode::Working);
        let original = resolve().expect("mints a key");
        move_to(KeyStore::Keychain).expect("moves");

        set_mode(Mode::Unreadable);
        let err = resolve().expect_err("must not invent a new key");

        assert!(err.contains("Refusing to generate a new one"), "{err}");
        assert!(!key_file().exists(), "no second key was written");
        set_mode(Mode::Working);
        assert_eq!(*resolve().expect("recovers"), *original);
    }

    #[test]
    fn the_key_file_is_owner_only() {
        let _serial = setup("mk-perms", Mode::Working);
        resolve().expect("writes the file");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(key_file()).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "the key file is readable by others");
        }
    }

    /// A truncated or overwritten file is corruption, and treating it as
    /// absence would quietly mint a second key over the first.
    #[test]
    fn a_damaged_key_file_is_an_error_not_a_fresh_start() {
        let _serial = setup("mk-damaged", Mode::Working);
        resolve().expect("writes the file");
        fs::write(key_file(), b"too short").expect("writable");
        *CACHE.lock().unwrap() = None;

        assert!(resolve().is_err(), "a damaged key file must not be ignored");
    }

    #[test]
    fn the_key_is_the_right_size_and_not_all_zeroes() {
        let _serial = setup("mk-quality", Mode::Working);
        let key = resolve().expect("mints a key");
        assert_eq!(key.len(), KEY_LEN);
        assert!(key.iter().any(|b| *b != 0));
    }
}
