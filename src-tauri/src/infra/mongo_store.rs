//! The MongoDB page's connections, sealed under the app master key as the
//! providers' API keys are (`llm_credentials_store`): one blob, the list in
//! it. A URI carries its password, so the whole list is sealed — names too,
//! which cost nothing to keep with it. Nothing here crosses the IPC boundary
//! but [`ConnectionView`], which has no URI.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};

use secrecy::SecretString;
use zeroize::{Zeroize, Zeroizing};

use crate::domain::mongo::{Connection, ConnectionView, MongoError};
use crate::infra::app_dir;
use crate::infra::master_key;
use crate::infra::secret_store::{self, SecretPurpose};

/// The file's name under the app directory — the feature's data, which
/// "Delete data" removes (`domain::features`).
pub const FILE: &str = "mongo_connections.enc";
const PURPOSE: SecretPurpose = SecretPurpose::MongoConnections;

/// From the read to the write of a change: two saves at once would each
/// write the list without the other's connection.
static WRITING: Mutex<()> = Mutex::new(());

fn writing() -> MutexGuard<'static, ()> {
    WRITING.lock().unwrap_or_else(PoisonError::into_inner)
}

fn path() -> Result<PathBuf, MongoError> {
    Ok(app_dir::dir().map_err(|e| MongoError::Store(e.to_string()))?.join(FILE))
}

/// The decrypted list, every URI wiped when it drops.
#[derive(Default)]
struct Connections(Vec<Connection>);

impl Drop for Connections {
    fn drop(&mut self) {
        for connection in &mut self.0 {
            connection.uri.zeroize();
        }
    }
}

/// No file is none saved; a file that does not open says so, rather than
/// reading as an empty list the next save would write over.
fn load() -> Result<Connections, MongoError> {
    let path = path()?;
    let shown = path.display();
    let blob = match std::fs::read(&path) {
        Ok(blob) => blob,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Connections::default()),
        Err(e) => return Err(MongoError::Store(format!("could not read {shown}: {e}"))),
    };
    let key = master_key::resolve().map_err(|e| MongoError::Store(format!("the key the connections are sealed under is out of reach: {e}")))?;
    let plain = secret_store::open(&key, PURPOSE, &blob)
        .map_err(|_| MongoError::Store(format!("{shown} does not open with this machine's key — delete the MongoDB data in the Catalog and add the connections again")))?;
    serde_json::from_slice(&plain).map(Connections).map_err(|_| MongoError::Store(format!("{shown} is damaged")))
}

fn save(connections: &Connections) -> Result<(), MongoError> {
    let store = |e: String| MongoError::Store(e);
    let plain = Zeroizing::new(serde_json::to_vec(&connections.0).map_err(|e| store(e.to_string()))?);
    let key = master_key::resolve().map_err(store)?;
    let blob = secret_store::seal(&key, PURPOSE, &plain).map_err(|e| store(e.to_string()))?;
    app_dir::ensure().map_err(store)?;
    app_dir::write_private(&path()?, &blob).map_err(store)
}

/// What the window lists, in the order they were added.
pub fn list() -> Result<Vec<ConnectionView>, MongoError> {
    Ok(load()?.0.iter().map(ConnectionView::from).collect())
}

/// A connection's URI, for the driver. Never crosses the IPC boundary.
pub fn uri(id: &str) -> Result<SecretString, MongoError> {
    load()?
        .0
        .iter()
        .find(|c| c.id == id)
        .map(|c| SecretString::from(c.uri.as_str()))
        .ok_or_else(|| MongoError::NoConnection(id.to_string()))
}

/// Adds `connection`, or replaces the one with its id.
pub fn put(connection: Connection) -> Result<(), MongoError> {
    let _held = writing();
    let mut connections = load()?;
    match connections.0.iter_mut().find(|c| c.id == connection.id) {
        Some(saved) => {
            // The URI it had goes too: `Connections` wipes only the ones still in the list.
            saved.uri.zeroize();
            *saved = connection;
        }
        None => connections.0.push(connection),
    }
    save(&connections)
}

/// A new name and agent box for a saved connection, its URI kept.
pub fn update(id: &str, name: &str, agent: bool) -> Result<(), MongoError> {
    let _held = writing();
    let mut connections = load()?;
    let saved = connections.0.iter_mut().find(|c| c.id == id).ok_or_else(|| MongoError::NoConnection(id.to_string()))?;
    saved.name = name.to_string();
    saved.agent = agent;
    save(&connections)
}

/// Gone is gone: deleting what is not there is not an error.
pub fn delete(id: &str) -> Result<(), MongoError> {
    let _held = writing();
    let mut connections = load()?;
    let before = connections.0.len();
    // Its URI wiped before it leaves the list: `Connections` wipes only the ones still in it.
    connections.0.iter_mut().filter(|c| c.id == id).for_each(|c| c.uri.zeroize());
    connections.0.retain(|c| c.id != id);
    if connections.0.len() == before {
        return Ok(());
    }
    save(&connections)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::with_app_dir;
    use secrecy::ExposeSecret;

    fn connection(id: &str, uri: &str) -> Connection {
        Connection { id: id.into(), name: format!("name {id}"), uri: uri.into(), agent: false }
    }

    /// "Delete data" in the Catalog removes this file.
    #[test]
    fn the_file_is_the_features_data() {
        use crate::domain::features::Feature;
        assert_eq!(Feature::Mongodb.manifest().data, Some(FILE));
    }

    #[test]
    fn connections_round_trip_and_the_list_shows_no_password() {
        with_app_dir("mongo-store-round-trip", || {
            assert!(list().unwrap().is_empty());
            put(connection("a", "mongodb://root:pw-a@h1")).unwrap();
            put(connection("b", "mongodb://h2")).unwrap();

            assert_eq!(uri("a").unwrap().expose_secret(), "mongodb://root:pw-a@h1");
            let views = list().unwrap();
            assert_eq!(views.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
            assert_eq!(views[0].address, "mongodb://root:•••@h1");
            assert!(matches!(uri("c"), Err(MongoError::NoConnection(id)) if id == "c"));
        });
    }

    #[test]
    fn a_put_replaces_by_id_and_an_update_keeps_the_uri() {
        with_app_dir("mongo-store-replace", || {
            put(connection("a", "mongodb://h1")).unwrap();
            put(connection("b", "mongodb://h2")).unwrap();
            put(connection("a", "mongodb://h3")).unwrap();
            assert_eq!(uri("a").unwrap().expose_secret(), "mongodb://h3");
            assert_eq!(list().unwrap().len(), 2);

            update("a", "Prod", true).unwrap();
            assert_eq!((list().unwrap()[0].name.as_str(), list().unwrap()[0].agent), ("Prod", true));
            assert_eq!(uri("a").unwrap().expose_secret(), "mongodb://h3");
            assert!(update("zzz", "x", false).is_err());
        });
    }

    /// A file that is there and cannot be read is not "none saved": read so,
    /// the next save would write an empty list over it.
    #[test]
    fn a_file_that_cannot_be_read_says_so() {
        with_app_dir("mongo-store-unreadable", || {
            app_dir::ensure().unwrap();
            std::fs::create_dir_all(path().unwrap()).unwrap();
            let refused = list().unwrap_err().to_string();
            assert!(refused.contains("could not read"), "{refused}");
        });
    }

    #[test]
    fn deleting_one_leaves_the_others() {
        with_app_dir("mongo-store-delete", || {
            put(connection("a", "mongodb://h1")).unwrap();
            put(connection("b", "mongodb://h2")).unwrap();
            delete("a").unwrap();
            delete("a").unwrap();
            assert_eq!(list().unwrap().iter().map(|v| v.id.as_str()).collect::<Vec<_>>(), ["b"]);
        });
    }

    /// A file sealed before the agent's box existed opens, every connection closed to the agent.
    #[test]
    fn a_file_from_before_the_agent_box_opens_with_it_off() {
        with_app_dir("mongo-store-old", || {
            put(connection("a", "mongodb://h")).unwrap();
            let key = master_key::resolve().unwrap();
            let old = secret_store::seal(&key, PURPOSE, br#"[{"id":"a","name":"Old","uri":"mongodb://h"}]"#).unwrap();
            std::fs::write(path().unwrap(), old).unwrap();
            let list = list().unwrap();
            assert_eq!((list[0].name.as_str(), list[0].agent), ("Old", false));
        });
    }

    #[test]
    fn the_file_holds_no_password_and_opens_only_as_itself() {
        with_app_dir("mongo-store-sealed", || {
            put(connection("a", "mongodb://root:hunter2-secret@h1")).unwrap();
            let raw = std::fs::read(path().unwrap()).unwrap();
            assert!(!String::from_utf8_lossy(&raw).contains("hunter2"), "the password is on disk in the clear");
            assert!(raw.starts_with(b"ATLS"));

            // A blob of another purpose under the same key does not open as this one.
            let key = master_key::resolve().unwrap();
            let other = secret_store::seal(&key, SecretPurpose::ProviderApiKey, br#"[{"id":"x","name":"x","uri":"mongodb://evil"}]"#).unwrap();
            std::fs::write(path().unwrap(), other).unwrap();
            let refused = list().unwrap_err().to_string();
            assert!(refused.contains("does not open"), "{refused}");
            // And a save does not write over what it could not read.
            assert!(put(connection("b", "mongodb://h2")).is_err());
        });
    }
}
