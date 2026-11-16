//! The pull request feature's servers, sealed under the app master key as the
//! providers' keys are (`llm_credentials_store`) and the MongoDB page's
//! connections are (`mongo_store`): one blob, the list in it.
//!
//! A token is in there, so the whole list is sealed, names and addresses with
//! it. What the window is handed is [`CodeHostServer`]: the address, the hosts
//! it answers to, the certificate — a PEM is not a secret — and whether a token
//! is set. Never the token.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};

use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::domain::code_host::{CodeHostError, CodeHostServer};
use crate::infra::app_dir;
use crate::infra::master_key;
use crate::infra::secret_store::{self, SecretPurpose};

/// The file's name under the app directory.
pub const FILE: &str = "code_hosts.enc";
const PURPOSE: SecretPurpose = SecretPurpose::CodeHosts;

/// From the read to the write of a change: two saves at once would each write
/// the list without the other's server.
static WRITING: Mutex<()> = Mutex::new(());

fn writing() -> MutexGuard<'static, ()> {
    WRITING.lock().unwrap_or_else(PoisonError::into_inner)
}

fn store_error(message: String) -> CodeHostError {
    CodeHostError::Store(message)
}

fn path() -> Result<PathBuf, CodeHostError> {
    Ok(app_dir::dir().map_err(|e| store_error(e.to_string()))?.join(FILE))
}

/// A server as the file holds it: the window's view, and the token. No `Debug`:
/// a derived one would print the token in any `{:?}` that met it.
#[derive(Clone, Serialize, Deserialize)]
struct Saved {
    #[serde(flatten)]
    server: CodeHostServer,
    #[serde(default)]
    token: String,
}

impl Saved {
    fn view(&self) -> CodeHostServer {
        CodeHostServer { token_set: !self.token.is_empty(), ..self.server.clone() }
    }
}

/// The decrypted list, every token wiped when it drops.
#[derive(Default)]
struct Servers(Vec<Saved>);

impl Drop for Servers {
    fn drop(&mut self) {
        for saved in &mut self.0 {
            saved.token.zeroize();
        }
    }
}

/// No file is none saved; a file that does not open says so, rather than
/// reading as an empty list the next save would write over.
fn load() -> Result<Servers, CodeHostError> {
    let path = path()?;
    let shown = path.display();
    let blob = match std::fs::read(&path) {
        Ok(blob) => blob,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Servers::default()),
        Err(e) => return Err(store_error(format!("could not read {shown}: {e}"))),
    };
    let key = master_key::resolve().map_err(|e| {
        store_error(format!("the key the servers are sealed under is out of reach: {e}"))
    })?;
    let plain = secret_store::open(&key, PURPOSE, &blob).map_err(|_| {
        store_error(format!(
            "{shown} does not open with this machine's key — delete the pull request data in the Catalog and add the server again"
        ))
    })?;
    serde_json::from_slice(&plain).map(Servers).map_err(|_| store_error(format!("{shown} is damaged")))
}

fn save(servers: &Servers) -> Result<(), CodeHostError> {
    let plain = zeroize::Zeroizing::new(
        serde_json::to_vec(&servers.0).map_err(|e| store_error(e.to_string()))?,
    );
    let key = master_key::resolve().map_err(|e| store_error(e.to_string()))?;
    let blob = secret_store::seal(&key, PURPOSE, &plain).map_err(|e| store_error(e.to_string()))?;
    app_dir::ensure().map_err(|e| store_error(e.to_string()))?;
    app_dir::write_private(&path()?, &blob).map_err(|e| store_error(e.to_string()))
}

/// What the window lists, in the order the servers were added.
pub fn list() -> Result<Vec<CodeHostServer>, CodeHostError> {
    Ok(load()?.0.iter().map(Saved::view).collect())
}

/// A server's token, for the client. Never crosses the IPC boundary.
pub fn token(id: &str) -> Result<SecretString, CodeHostError> {
    let servers = load()?;
    let saved = servers
        .0
        .iter()
        .find(|saved| saved.server.id == id)
        .ok_or_else(|| CodeHostError::NoServer(id.to_string()))?;
    if saved.token.is_empty() {
        return Err(CodeHostError::NoToken(id.to_string()));
    }
    Ok(SecretString::from(saved.token.as_str()))
}

/// Adds `server`, or replaces the one with its id.
///
/// `token` of `None` keeps the saved one: an edit in the form has no token to
/// send back — the window was never given it — so an empty field is "leave it",
/// not "erase it".
pub fn put(server: CodeHostServer, token: Option<String>) -> Result<(), CodeHostError> {
    let _held = writing();
    // An empty field is an empty field, not a token: the form sends whatever is
    // in it, and a blank one must not wipe a token that is working.
    // A token pasted with a newline after it is the token: kept as pasted, it
    // would make the `Authorization` header invalid.
    let token = token
        .map(|mut pasted| {
            let trimmed = pasted.trim().to_string();
            pasted.zeroize();
            trimmed
        })
        .filter(|token| !token.is_empty());
    let mut servers = load()?;
    match servers.0.iter_mut().find(|saved| saved.server.id == server.id) {
        Some(saved) => {
            // `None` keeps the saved token: the form was never given it, so an
            // empty field is "leave it", not "erase it".
            if let Some(token) = token {
                saved.token.zeroize();
                saved.token = token;
            }
            saved.server = server;
        }
        None => servers.0.push(Saved { server, token: token.unwrap_or_default() }),
    }
    save(&servers)
}

/// Gone is gone: deleting what is not there is not an error.
pub fn delete(id: &str) -> Result<(), CodeHostError> {
    let _held = writing();
    let mut servers = load()?;
    let before = servers.0.len();
    servers.0.iter_mut().filter(|saved| saved.server.id == id).for_each(|saved| saved.token.zeroize());
    servers.0.retain(|saved| saved.server.id != id);
    if servers.0.len() == before {
        return Ok(());
    }
    save(&servers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::with_app_dir;
    use secrecy::ExposeSecret;

    /// The Catalog's "Delete data" for the pull request strip removes this file.
    #[test]
    fn the_feature_s_data_is_this_file() {
        use crate::domain::features::Feature;
        assert_eq!(Feature::PullRequests.manifest().data, Some(FILE));
    }

    fn server(id: &str, base_url: &str) -> CodeHostServer {
        CodeHostServer {
            id: id.to_string(),
            base_url: base_url.to_string(),
            hosts: vec!["git.server.com".to_string()],
            trusted_cert_pem: Some("-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n".to_string()),
            token_set: false,
        }
    }

    #[test]
    fn servers_round_trip_and_the_list_holds_no_token() {
        with_app_dir("code-host-store-round-trip", || {
            assert!(list().unwrap().is_empty());
            put(server("a", "https://server.com"), Some("ATATT-secret-a".into())).unwrap();
            put(server("b", "https://other.example"), Some("ATATT-secret-b".into())).unwrap();

            assert_eq!(token("a").unwrap().expose_secret(), "ATATT-secret-a");
            let views = list().unwrap();
            assert_eq!(views.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
            assert!(views.iter().all(|s| s.token_set));
            // The certificate is not a secret, and the form shows it back.
            assert!(views[0].trusted_cert_pem.as_deref().unwrap_or_default().contains("BEGIN CERTIFICATE"));
            assert!(!serde_json::to_string(&views).unwrap().contains("ATATT"), "a token in the window's copy");
            assert!(matches!(token("c"), Err(CodeHostError::NoServer(id)) if id == "c"));

            // A server added with no token yet is "no token", and says so.
            put(server("c", "https://c.example"), None).unwrap();
            assert!(!list().unwrap()[2].token_set);
            assert!(matches!(token("c"), Err(CodeHostError::NoToken(id)) if id == "c"));

            // A blank field is not a token either, and leaves the saved one.
            put(server("a", "https://server.com"), Some("  ".to_string())).unwrap();
            assert_eq!(token("a").unwrap().expose_secret(), "ATATT-secret-a");
        });
    }

    /// A token copied out of a terminal comes with its newline, and a header
    /// with one in it is not a header.
    #[test]
    fn a_pasted_token_is_kept_without_the_space_around_it() {
        with_app_dir("code-host-store-trim", || {
            put(server("a", "https://server.com"), Some("  ATATT-secret\n".into())).unwrap();
            assert_eq!(token("a").unwrap().expose_secret(), "ATATT-secret");
        });
    }

    /// The form edits an address and sends no token; erasing the saved one
    /// would turn a working server into a refused one with nothing said.
    #[test]
    fn an_edit_without_a_token_keeps_the_saved_one() {
        with_app_dir("code-host-store-keep-token", || {
            put(server("a", "https://server.com"), Some("ATATT-secret".into())).unwrap();
            let mut edited = server("a", "https://moved.example");
            edited.hosts = Vec::new();
            put(edited, None).unwrap();

            assert_eq!(token("a").unwrap().expose_secret(), "ATATT-secret");
            let views = list().unwrap();
            assert_eq!(views.len(), 1);
            assert_eq!(views[0].base_url, "https://moved.example");
            assert!(views[0].token_set);
        });
    }

    #[test]
    fn deleting_one_leaves_the_others() {
        with_app_dir("code-host-store-delete", || {
            put(server("a", "https://a.example"), Some("t-a".into())).unwrap();
            put(server("b", "https://b.example"), Some("t-b".into())).unwrap();
            delete("a").unwrap();
            delete("a").unwrap();
            assert_eq!(list().unwrap().iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["b"]);
        });
    }

    /// A file that is there and cannot be read is not "none saved": read so,
    /// the next save would write an empty list over it.
    #[test]
    fn a_file_that_cannot_be_read_says_so() {
        with_app_dir("code-host-store-unreadable", || {
            app_dir::ensure().unwrap();
            std::fs::create_dir_all(path().unwrap()).unwrap();
            let refused = list().unwrap_err().to_string();
            assert!(refused.contains("could not read"), "{refused}");
        });
    }

    #[test]
    fn the_file_holds_no_token_and_opens_only_as_itself() {
        with_app_dir("code-host-store-sealed", || {
            put(server("a", "https://server.com"), Some("ATATT-hunter2".into())).unwrap();
            let raw = std::fs::read(path().unwrap()).unwrap();
            assert!(!String::from_utf8_lossy(&raw).contains("hunter2"), "a token is on disk in the clear");
            assert!(raw.starts_with(b"ATLS"));

            // A blob of another purpose under the same key does not open as this one.
            let key = master_key::resolve().unwrap();
            let other = secret_store::seal(&key, SecretPurpose::ProviderApiKey, br#"[{"id":"x"}]"#).unwrap();
            std::fs::write(path().unwrap(), other).unwrap();
            let refused = list().unwrap_err().to_string();
            assert!(refused.contains("does not open"), "{refused}");
            // And a save does not write over what it could not read.
            assert!(put(server("b", "https://b.example"), Some("t".into())).is_err());
        });
    }
}
