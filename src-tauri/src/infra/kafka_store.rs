//! The Kafka page's connections, sealed under the app master key as the
//! providers' API keys are (`llm_credentials_store`): one blob, the list in it.
//! A connection carries a SASL password, so the whole list is sealed — names
//! and brokers too, which cost nothing to keep with it. Nothing here crosses
//! the IPC boundary but [`ConnectionView`], which has no password.

use std::path::PathBuf;
use std::sync::Mutex;

use zeroize::Zeroizing;

use crate::domain::kafka::{Connection, ConnectionView, KafkaError};
use crate::infra::app_dir;
use crate::infra::master_key;
use crate::infra::secret_store::{self, SecretPurpose};
use crate::sync::lock;

/// The file's name under the app directory — the feature's data, which
/// "Delete data" removes (`domain::features`).
pub const FILE: &str = "kafka_connections.enc";
const PURPOSE: SecretPurpose = SecretPurpose::KafkaConnections;

/// From the read to the write of a change: two saves at once would each write
/// the list without the other's connection.
static WRITING: Mutex<()> = Mutex::new(());

fn path() -> Result<PathBuf, KafkaError> {
    Ok(app_dir::dir().map_err(|e| KafkaError::Store(e.to_string()))?.join(FILE))
}

/// No file is none saved; a file that does not open says so, rather than
/// reading as an empty list the next save would write over. Every password in
/// it is wiped when the list drops (`domain::kafka::Sasl`).
fn load() -> Result<Vec<Connection>, KafkaError> {
    let path = path()?;
    let shown = path.display();
    let blob = match std::fs::read(&path) {
        Ok(blob) => blob,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(KafkaError::Store(format!("could not read {shown}: {e}"))),
    };
    let key = master_key::resolve().map_err(|e| KafkaError::Store(format!("the key the connections are sealed under is out of reach: {e}")))?;
    let plain = secret_store::open(&key, PURPOSE, &blob)
        .map_err(|_| KafkaError::Store(format!("{shown} does not open with this machine's key — delete the Kafka data in the Catalog and add the connections again")))?;
    serde_json::from_slice(&plain).map_err(|_| KafkaError::Store(format!("{shown} is damaged")))
}

fn save(connections: &[Connection]) -> Result<(), KafkaError> {
    let store = |e: String| KafkaError::Store(e);
    let plain = Zeroizing::new(serde_json::to_vec(connections).map_err(|e| store(e.to_string()))?);
    let key = master_key::resolve().map_err(store)?;
    let blob = secret_store::seal(&key, PURPOSE, &plain).map_err(|e| store(e.to_string()))?;
    app_dir::ensure().map_err(store)?;
    app_dir::write_private(&path()?, &blob).map_err(store)
}

/// What the window lists, in the order they were added.
pub fn list() -> Result<Vec<ConnectionView>, KafkaError> {
    Ok(load()?.iter().map(ConnectionView::from).collect())
}

/// A connection as the client needs it, password and all. Never crosses the
/// IPC boundary: the window asks for [`list`].
pub fn get(id: &str) -> Result<Connection, KafkaError> {
    load()?.into_iter().find(|connection| connection.id == id).ok_or_else(|| KafkaError::NoConnection(id.to_string()))
}

/// Adds `connection`, or replaces the one with its id. A password left empty
/// in an edit keeps the one already saved — the form never shows it, so it has
/// nothing to send back.
pub fn put(mut connection: Connection) -> Result<(), KafkaError> {
    let _held = lock(&WRITING);
    let mut connections = load()?;
    match connections.iter_mut().find(|c| c.id == connection.id) {
        Some(saved) => {
            if let (Some(new), Some(old)) = (connection.sasl.as_mut(), saved.sasl.as_mut()) {
                if new.password.is_empty() {
                    std::mem::swap(&mut new.password, &mut old.password);
                }
            }
            *saved = connection;
        }
        None => connections.push(connection),
    }
    save(&connections)
}

/// Gone is gone: deleting what is not there is not an error.
pub fn delete(id: &str) -> Result<(), KafkaError> {
    let _held = lock(&WRITING);
    let mut connections = load()?;
    let Some(at) = connections.iter().position(|c| c.id == id) else { return Ok(()) };
    connections.remove(at);
    save(&connections)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::kafka::{Sasl, SaslMechanism};
    use crate::testing::with_app_dir;

    fn connection(id: &str, brokers: &str) -> Connection {
        Connection {
            id: id.into(),
            name: format!("name {id}"),
            brokers: brokers.into(),
            tls: false,
            ca_file: None,
            sasl: Some(Sasl { mechanism: SaslMechanism::ScramSha512, username: "reports".into(), password: format!("pw-{id}") }),
            read_only: false,
            agent: false,
        }
    }

    /// "Delete data" in the Catalog removes this file.
    #[test]
    fn the_file_is_the_features_data() {
        use crate::domain::features::Feature;
        assert_eq!(Feature::Kafka.manifest().data, Some(FILE));
    }

    /// Not just the manifest's promise: "Delete data" takes the file.
    #[test]
    fn the_features_data_deletion_takes_the_file() {
        with_app_dir("kafka-store-delete-data", || {
            put(connection("a", "h1:9092")).unwrap();
            assert!(path().unwrap().exists());
            crate::services::features::delete_data("kafka").unwrap();
            assert!(!path().unwrap().exists(), "kafka_connections.enc went with the feature's data");
        });
    }

    #[test]
    fn connections_round_trip_and_the_list_shows_no_password() {
        with_app_dir("kafka-store-round-trip", || {
            assert!(list().unwrap().is_empty());
            put(connection("a", "h1:9092")).unwrap();
            put(connection("b", "h2:9092")).unwrap();

            let saved = get("a").unwrap();
            assert_eq!(saved.sasl.unwrap().password, "pw-a");
            let views = list().unwrap();
            assert_eq!(views.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
            assert_eq!(views[0].brokers, "h1:9092");
            assert!(views[0].has_password);
            assert_eq!(views[0].login.as_ref().map(|l| l.username.as_str()), Some("reports"));
            assert!(matches!(get("c"), Err(KafkaError::NoConnection(id)) if id == "c"));
        });
    }

    /// The form shows no password, so an edit sends an empty one and means
    /// "keep it" — not "remove it".
    #[test]
    fn a_put_with_an_empty_password_keeps_the_saved_one() {
        with_app_dir("kafka-store-empty-password", || {
            put(connection("a", "h1:9092")).unwrap();
            let mut edited = connection("a", "h9:9092");
            edited.sasl.as_mut().unwrap().password.clear();
            edited.name = "Prod".into();
            put(edited).unwrap();

            let saved = get("a").unwrap();
            assert_eq!(saved.brokers, "h9:9092", "the new brokers land");
            assert_eq!(saved.name, "Prod");
            assert_eq!(saved.sasl.unwrap().password, "pw-a", "the password stayed");
            assert_eq!(list().unwrap().len(), 1, "and it was a replacement, not an addition");

            let mut retyped = connection("a", "h9:9092");
            retyped.sasl.as_mut().unwrap().password = "new".into();
            put(retyped).unwrap();
            assert_eq!(get("a").unwrap().sasl.unwrap().password, "new");
        });
    }

    /// Turning SASL off is said by sending no login at all, not by an empty
    /// password — which the rule above reads as "keep it".
    #[test]
    fn a_put_without_a_login_takes_the_login_away() {
        with_app_dir("kafka-store-no-login", || {
            put(connection("a", "h1:9092")).unwrap();
            let mut plain = connection("a", "h1:9092");
            plain.sasl = None;
            put(plain).unwrap();
            assert!(get("a").unwrap().sasl.is_none());
            assert!(!list().unwrap()[0].has_password);
        });
    }

    #[test]
    fn the_flags_come_with_the_connection_and_are_not_reset_by_a_new_edit() {
        with_app_dir("kafka-store-flags", || {
            let mut prod = connection("a", "h1:9092");
            prod.read_only = true;
            prod.agent = true;
            put(prod).unwrap();

            let mut again = connection("a", "h2:9092");
            again.read_only = true;
            again.agent = true;
            put(again).unwrap();
            let view = &list().unwrap()[0];
            assert!(view.read_only && view.agent, "a new broker keeps both flags");

            let mut readable = connection("a", "h2:9092");
            readable.read_only = false;
            put(readable).unwrap();
            assert!(!list().unwrap()[0].read_only, "and clearing one clears it");
        });
    }

    /// A file that is there and cannot be read is not "none saved": read so,
    /// the next save would write an empty list over it.
    #[test]
    fn a_file_that_cannot_be_read_says_so() {
        with_app_dir("kafka-store-unreadable", || {
            app_dir::ensure().unwrap();
            std::fs::create_dir_all(path().unwrap()).unwrap();
            let refused = list().unwrap_err().to_string();
            assert!(refused.contains("could not read"), "{refused}");
            assert!(put(connection("a", "h1:9092")).is_err(), "a save must not write over what it could not read");
        });
    }

    #[test]
    fn deleting_one_leaves_the_others() {
        with_app_dir("kafka-store-delete", || {
            put(connection("a", "h1:9092")).unwrap();
            put(connection("b", "h2:9092")).unwrap();
            delete("a").unwrap();
            delete("a").unwrap();
            assert_eq!(list().unwrap().iter().map(|v| v.id.as_str()).collect::<Vec<_>>(), ["b"]);
        });
    }

    /// A file sealed before the read-only flag existed opens, that flag off:
    /// nothing in it says the cluster must not be written to.
    #[test]
    fn a_file_from_before_the_flags_opens_with_them_off() {
        with_app_dir("kafka-store-old", || {
            put(connection("a", "h1:9092")).unwrap();
            let key = master_key::resolve().unwrap();
            let old = secret_store::seal(&key, PURPOSE, br#"[{"id":"a","name":"Old","brokers":"h:9092"}]"#).unwrap();
            std::fs::write(path().unwrap(), old).unwrap();
            let list = list().unwrap();
            assert_eq!((list[0].name.as_str(), list[0].read_only, list[0].agent), ("Old", false, false));
        });
    }

    #[test]
    fn the_file_holds_no_password_and_opens_only_as_itself() {
        with_app_dir("kafka-store-sealed", || {
            put(connection("a", "h1:9092")).unwrap();
            let raw = std::fs::read(path().unwrap()).unwrap();
            assert!(!String::from_utf8_lossy(&raw).contains("pw-a"), "the password is on disk in the clear");
            assert!(raw.starts_with(b"ATLS"));

            // A blob of another purpose under the same key does not open as this one.
            let key = master_key::resolve().unwrap();
            let other = secret_store::seal(&key, SecretPurpose::MongoConnections, br#"[{"id":"x","name":"x","brokers":"evil:9092"}]"#).unwrap();
            std::fs::write(path().unwrap(), other).unwrap();
            let refused = list().unwrap_err().to_string();
            assert!(refused.contains("does not open"), "{refused}");
            // And a save does not write over what it could not read.
            assert!(put(connection("b", "h2:9092")).is_err());
        });
    }
}
