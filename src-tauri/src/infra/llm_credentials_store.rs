//! API keys for configured providers, sealed under the app master key.
//!
//! One encrypted blob holding a `provider id -> key` map, not a file per
//! provider: there are always a handful of entries, and one file is one
//! atomic write. Nothing here is ever returned across the IPC boundary —
//! callers outside this module get a `SecretString` to hand to a provider, or
//! the boolean [`has_api_key`], and the UI only ever learns whether a key
//! exists.

use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::path::PathBuf;

use secrecy::SecretString;
use zeroize::{Zeroize, Zeroizing};

use crate::domain::settings::CredentialsError;
use crate::infra::app_dir;
use crate::infra::master_key;
use crate::infra::secret_store::{self, SecretPurpose};

const FILE: &str = "llm_credentials.enc";
const PURPOSE: SecretPurpose = SecretPurpose::ProviderApiKey;

fn path() -> Result<PathBuf, String> {
    Ok(app_dir::dir()?.join(FILE))
}

/// The decrypted map, with every key wiped when it drops.
///
/// `serde_json` deserializes into ordinary `String`s, so the keys do land in
/// plain heap allocations on the way in; what this guarantees is that they are
/// not merely freed and left there for the next crash dump. Nothing hands this
/// type out, so the window is one function call wide.
#[derive(Default)]
struct Credentials(HashMap<String, String>);

impl Deref for Credentials {
    type Target = HashMap<String, String>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Credentials {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for Credentials {
    fn drop(&mut self) {
        for key in self.0.values_mut() {
            key.zeroize();
        }
    }
}

/// The saved keys. No file yet is none saved; a file that cannot be read —
/// the master key out of reach, a blob sealed under another one, damage — is
/// an error that says so. Read as "no key", it reached the user as a provider's
/// 401 with nothing to say why.
fn load_all() -> Result<Credentials, CredentialsError> {
    let path = app_dir::dir()?.join(FILE);
    let shown = || path.display().to_string();
    let blob = match std::fs::read(&path) {
        Ok(blob) => blob,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Credentials::default()),
        Err(e) => return Err(CredentialsError::Read { path: shown(), reason: e.to_string() }),
    };
    let key = master_key::resolve().map_err(CredentialsError::MasterKey)?;
    let plain = secret_store::open(&key, PURPOSE, &blob).map_err(|_| CredentialsError::Foreign(shown()))?;
    serde_json::from_slice(&plain).map(Credentials).map_err(|_| CredentialsError::Damaged(shown()))
}

/// What a save starts from. A file that does not open is started over: the
/// keys in it are lost either way, and this is how the user gets back to
/// working ones. Safe for one reason — sealing needs the same master key that
/// opening does, so with the key out of reach the write fails too and the
/// file is left as it was.
fn load_for_save() -> Credentials {
    load_all().unwrap_or_default()
}

fn save_all(map: &Credentials) -> Result<(), String> {
    let plain = Zeroizing::new(
        serde_json::to_vec(&map.0).map_err(|e| format!("could not serialize credentials: {e}"))?,
    );
    let key = master_key::resolve()?;
    let blob = secret_store::seal(&key, PURPOSE, &plain).map_err(|e| e.to_string())?;
    app_dir::ensure()?;
    app_dir::write_private(&path()?, &blob)
}

pub fn save_api_key(provider_id: &str, api_key: &str) -> Result<(), String> {
    let mut map = load_for_save();
    map.insert(provider_id.to_string(), api_key.to_string());
    save_all(&map)
}

/// The stored key, for constructing a provider. Never crosses the IPC
/// boundary.
pub fn get_api_key(provider_id: &str) -> Result<Option<SecretString>, CredentialsError> {
    Ok(load_all()?.get(provider_id).map(|key| SecretString::from(key.as_str())))
}

pub fn has_api_key(provider_id: &str) -> Result<bool, CredentialsError> {
    Ok(load_all()?.contains_key(provider_id))
}

pub fn delete_api_key(provider_id: &str) -> Result<(), String> {
    let mut map = load_for_save();
    // `remove` moves the key out of the map, past the wipe-on-drop above — so
    // this copy gets wiped by hand.
    if let Some(mut removed) = map.remove(provider_id) {
        removed.zeroize();
        save_all(&map)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::with_app_dir;
    use secrecy::ExposeSecret;

    #[test]
    fn keys_round_trip_and_stay_apart() {
        with_app_dir("creds-round-trip", || {
            save_api_key("openai", "sk-one").unwrap();
            save_api_key("local", "sk-two").unwrap();

            assert_eq!(get_api_key("openai").unwrap().unwrap().expose_secret(), "sk-one");
            assert_eq!(get_api_key("local").unwrap().unwrap().expose_secret(), "sk-two");
            assert!(get_api_key("anthropic").unwrap().is_none());
        });
    }

    #[test]
    fn saving_one_key_leaves_the_others_alone() {
        with_app_dir("creds-preserve", || {
            save_api_key("openai", "sk-one").unwrap();
            save_api_key("local", "sk-two").unwrap();
            save_api_key("openai", "sk-rotated").unwrap();

            assert_eq!(get_api_key("openai").unwrap().unwrap().expose_secret(), "sk-rotated");
            assert_eq!(get_api_key("local").unwrap().unwrap().expose_secret(), "sk-two");
        });
    }

    #[test]
    fn deleting_one_key_leaves_the_others_alone() {
        with_app_dir("creds-delete", || {
            save_api_key("openai", "sk-one").unwrap();
            save_api_key("local", "sk-two").unwrap();

            delete_api_key("openai").unwrap();
            assert!(!has_api_key("openai").unwrap());
            assert!(has_api_key("local").unwrap());

            // Deleting what is not there is not an error, and must not
            // rewrite the file.
            delete_api_key("openai").unwrap();
            assert!(has_api_key("local").unwrap());
        });
    }

    /// The whole point of sealing: the file next to the settings must not
    /// carry the key in a form anything can read.
    #[test]
    fn the_stored_file_does_not_contain_the_key() {
        with_app_dir("creds-sealed", || {
            save_api_key("openai", "sk-live-abcdef123456").unwrap();

            let raw = std::fs::read(path().unwrap()).unwrap();
            assert!(
                !String::from_utf8_lossy(&raw).contains("sk-live"),
                "the key is on disk in the clear"
            );
            // The provider id is not secret and is not claimed to be; what
            // must not be there is the credential.
            assert!(raw.starts_with(b"ATLS"), "not the sealed format");
        });
    }

    /// A blob sealed under a different master key is unreadable, not a
    /// panic and not someone else's key — and it says so rather than reading
    /// as no key at all. Saving a key again starts the file over.
    #[test]
    fn a_blob_from_another_key_is_an_error_until_a_key_is_saved_again() {
        with_app_dir("creds-foreign", || {
            save_api_key("openai", "sk-one").unwrap();

            let stolen = secret_store::seal(
                &[7u8; 32],
                PURPOSE,
                br#"{"openai":"sk-attacker"}"#,
            )
            .unwrap();
            app_dir::write_private(&path().unwrap(), &stolen).unwrap();

            let refused = get_api_key("openai").expect_err("another key's blob").to_string();
            assert!(refused.contains("does not open"), "{refused}");
            assert!(!refused.contains("sk-"), "{refused}");
            assert!(has_api_key("openai").is_err());

            save_api_key("openai", "sk-again").unwrap();
            assert_eq!(get_api_key("openai").unwrap().unwrap().expose_secret(), "sk-again");
        });
    }

    #[test]
    fn no_file_yet_is_no_keys_not_an_error() {
        with_app_dir("creds-none", || {
            assert!(get_api_key("openai").unwrap().is_none());
            assert!(!has_api_key("openai").unwrap());
        });
    }
}
