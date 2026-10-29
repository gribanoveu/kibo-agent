//! The HTTP page's folder (`docs/45-http-client.md`, decisions 2 and 4):
//!
//! ```text
//! ~/.kibo/http/
//!   collections/<id>.postman_collection.json   as Postman exports it, named by id, never by its title
//!   responses/<collection-id>/<request-id>.enc  a request's last response, sealed
//!   history.enc                                 the last sends, sealed
//! ```
//!
//! A response is kept under its collection: the same export imported twice
//! has the same request ids, and deleting one copy must not take the other's.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use zeroize::Zeroizing;

use crate::domain::http_client::{trim_history, HistoryEntry, HttpError, SavedResponse};
use crate::domain::postman::{is_safe_id, Collection};
use crate::infra::app_dir;
use crate::infra::master_key;
use crate::infra::secret_store::{self, SecretPurpose};

const PURPOSE: SecretPurpose = SecretPurpose::HttpResponses;
const SUFFIX: &str = ".postman_collection.json";
/// Saved responses together stay under this; the oldest go first.
pub const RESPONSES_BYTES: u64 = 200 << 20;

/// The feature's data folder (`domain::features`).
pub fn dir() -> Result<PathBuf, HttpError> {
    Ok(app_dir::dir().map_err(|e| HttpError::Store(e.to_string()))?.join("http"))
}

fn checked(id: &str) -> Result<&str, HttpError> {
    if is_safe_id(id) {
        Ok(id)
    } else {
        Err(HttpError::BadId(id.into()))
    }
}

fn collection_path(id: &str) -> Result<PathBuf, HttpError> {
    Ok(dir()?.join("collections").join(format!("{}{SUFFIX}", checked(id)?)))
}

fn read(path: &Path) -> Result<Option<Vec<u8>>, HttpError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(HttpError::Store(format!("could not read {}: {e}", path.display()))),
    }
}

fn remove(path: &Path, result: std::io::Result<()>) -> Result<(), HttpError> {
    match result {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(HttpError::Store(format!("could not delete {}: {e}", path.display()))),
        _ => Ok(()),
    }
}

/// A collection's id, and the collection or why its file does not read.
pub type Stored = (String, Result<Collection, HttpError>);

/// Every collection by id, in the order of their ids; one that does not read says why, in its place.
pub fn all() -> Result<Vec<Stored>, HttpError> {
    let folder = dir()?.join("collections");
    let entries = match std::fs::read_dir(&folder) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(HttpError::Store(format!("could not read {}: {e}", folder.display()))),
    };
    let mut ids: Vec<String> = entries
        .filter_map(|e| e.ok()?.file_name().to_str()?.strip_suffix(SUFFIX).filter(|id| is_safe_id(id)).map(String::from))
        .collect();
    ids.sort();
    Ok(ids.into_iter().map(|id| (id.clone(), get(&id))).collect())
}

pub fn get(id: &str) -> Result<Collection, HttpError> {
    let path = collection_path(id)?;
    let bytes = read(&path)?.ok_or_else(|| HttpError::NoCollection(id.into()))?;
    Collection::parse(&String::from_utf8_lossy(&bytes))
}

/// Written whole, through a temporary file: a crash leaves the old file or the new one.
pub fn save(id: &str, collection: &Collection) -> Result<(), HttpError> {
    app_dir::write_private(&collection_path(id)?, collection.to_text().as_bytes()).map_err(HttpError::Store)
}

/// A collection that came in by Import, under a new id; returns it.
pub fn add(collection: &Collection) -> Result<String, HttpError> {
    let id = uuid::Uuid::new_v4().to_string();
    save(&id, collection)?;
    Ok(id)
}

/// The collection's file and its saved responses.
pub fn delete(id: &str) -> Result<(), HttpError> {
    let file = collection_path(id)?;
    remove(&file, std::fs::remove_file(&file))?;
    let responses = dir()?.join("responses").join(id);
    remove(&responses, std::fs::remove_dir_all(&responses))
}

fn response_path(collection: &str, request: &str) -> Result<PathBuf, HttpError> {
    Ok(dir()?.join("responses").join(checked(collection)?).join(format!("{}.enc", checked(request)?)))
}

fn sealed(plain: &[u8]) -> Result<Vec<u8>, HttpError> {
    let key = master_key::resolve().map_err(HttpError::Store)?;
    secret_store::seal(&key, PURPOSE, plain).map_err(|e| HttpError::Store(e.to_string()))
}

fn opened(path: &Path, blob: &[u8]) -> Result<Zeroizing<Vec<u8>>, HttpError> {
    let key = master_key::resolve().map_err(|e| HttpError::Store(format!("the key the responses are sealed under is out of reach: {e}")))?;
    secret_store::open(&key, PURPOSE, blob).map_err(|_| HttpError::Store(format!("{} does not open with this machine's key — delete that file to start it afresh", path.display())))
}

/// The request's last response in place of the one before, then the oldest
/// of all dropped while they come to more than 200 MB.
pub fn save_response(collection: &str, request: &str, response: &SavedResponse) -> Result<(), HttpError> {
    // The rest as JSON, a line break — JSON writes none of its own — then the body's bytes as they are.
    let mut plain = Zeroizing::new(serde_json::to_vec(response).map_err(|e| HttpError::Store(e.to_string()))?);
    plain.push(b'\n');
    plain.extend_from_slice(&response.body);
    app_dir::write_private(&response_path(collection, request)?, &sealed(&plain)?).map_err(HttpError::Store)?;
    // The response is saved by now: a trim that fails is not a save that did.
    if let Err(e) = keep_under(&dir()?.join("responses"), RESPONSES_BYTES) {
        eprintln!("http: saved responses not trimmed to 200 MB: {e}");
    }
    Ok(())
}

/// The saved responses under `root`: a folder per collection, `<request-id>.enc` in it.
/// Nothing else — a `.DS_Store` from Finder, a save's temporary file — is theirs to touch.
fn response_folders(root: &Path) -> impl Iterator<Item = std::fs::DirEntry> {
    let entries = std::fs::read_dir(root).into_iter().flatten().filter_map(Result::ok);
    entries.filter(|folder| folder.file_type().is_ok_and(|t| t.is_dir()))
}

fn enc_files(folder: &Path) -> impl Iterator<Item = std::fs::DirEntry> {
    let entries = std::fs::read_dir(folder).into_iter().flatten().filter_map(Result::ok);
    entries.filter(|file| file.file_name().to_str().is_some_and(|name| name.ends_with(".enc")))
}

fn response_files(root: &Path) -> impl Iterator<Item = std::fs::DirEntry> {
    response_folders(root).flat_map(|folder| enc_files(&folder.path()).collect::<Vec<_>>())
}

/// None when the request was never sent, or its response was dropped.
pub fn last_response(collection: &str, request: &str) -> Result<Option<SavedResponse>, HttpError> {
    let path = response_path(collection, request)?;
    let Some(blob) = read(&path)? else { return Ok(None) };
    let plain = opened(&path, &blob)?;
    let damaged = || HttpError::Store(format!("{} is damaged", path.display()));
    let split = plain.iter().position(|&b| b == b'\n').ok_or_else(damaged)?;
    let mut response: SavedResponse = serde_json::from_slice(&plain[..split]).map_err(|_| damaged())?;
    response.body = plain[split + 1..].to_vec();
    Ok(Some(response))
}

pub fn delete_response(collection: &str, request: &str) -> Result<(), HttpError> {
    let path = response_path(collection, request)?;
    remove(&path, std::fs::remove_file(&path))
}

/// Drops every response whose collection or request is gone. A collection
/// missing from `known` is gone; one there keeps the requests listed, or all
/// of them when its file did not read (None) — what it holds is not known.
pub fn prune_responses(known: &HashMap<String, Option<HashSet<String>>>) -> Result<(), HttpError> {
    let root = dir()?.join("responses");
    for folder in response_folders(&root) {
        let path = folder.path();
        let requests = match folder.file_name().to_str().and_then(|c| known.get(c)) {
            None => {
                remove(&path, std::fs::remove_dir_all(&path))?;
                continue;
            }
            Some(None) => continue,
            Some(Some(requests)) => requests,
        };
        for file in enc_files(&path) {
            let request = file.file_name().to_str().and_then(|f| f.strip_suffix(".enc")).map(String::from);
            if !request.is_some_and(|r| requests.contains(&r)) {
                remove(&file.path(), std::fs::remove_file(file.path()))?;
            }
        }
    }
    Ok(())
}

/// Oldest written first, by the file's time: the one just saved is the newest and stays.
fn keep_under(root: &Path, cap: u64) -> Result<(), HttpError> {
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = response_files(root)
        .filter_map(|file| {
            let meta = file.metadata().ok()?;
            Some((meta.modified().ok()?, meta.len(), file.path()))
        })
        .collect();
    files.sort();
    let mut total: u64 = files.iter().map(|(_, size, _)| size).sum();
    for (_, size, path) in files.iter().take(files.len().saturating_sub(1)) {
        if total <= cap {
            break;
        }
        remove(path, std::fs::remove_file(path))?;
        total -= size;
    }
    Ok(())
}

/// From the read to the write of a send's record: two at once would each drop the other's.
static RECORDING: Mutex<()> = Mutex::new(());

fn recording() -> MutexGuard<'static, ()> {
    RECORDING.lock().unwrap_or_else(PoisonError::into_inner)
}

fn history_path() -> Result<PathBuf, HttpError> {
    Ok(dir()?.join("history.enc"))
}

/// The sends, oldest first. A file that does not open says so rather than
/// reading as empty, which the next send would write over.
pub fn history() -> Result<Vec<HistoryEntry>, HttpError> {
    let path = history_path()?;
    let Some(blob) = read(&path)? else { return Ok(Vec::new()) };
    let plain = opened(&path, &blob)?;
    serde_json::from_slice(&plain).map_err(|_| HttpError::Store(format!("{} is damaged", path.display())))
}

/// ponytail: the whole log is sealed and written again per send — 50 MB at
/// most, tens of milliseconds; an append-only log if sends get that frequent.
pub fn record(entry: HistoryEntry) -> Result<(), HttpError> {
    let _held = recording();
    let mut entries = history()?;
    entries.push(entry);
    trim_history(&mut entries);
    let plain = Zeroizing::new(serde_json::to_vec(&entries).map_err(|e| HttpError::Store(e.to_string()))?);
    app_dir::write_private(&history_path()?, &sealed(&plain)?).map_err(HttpError::Store)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::http_client::{Outcome, Outgoing, Prepared, HISTORY_ENTRIES};
    use crate::testing::with_app_dir;

    const PROBE: &str = include_str!("testdata/postman/probe.postman_collection.json");

    fn response(status: u16, body: &[u8]) -> SavedResponse {
        SavedResponse {
            at: 1_761_700_000_000,
            environment: Some("test".into()),
            outcome: Outcome::Response { status, headers: vec![("Content-Type".into(), "application/json".into())], url: "https://api/x".into(), redirects: vec![], elapsed_ms: 42 },
            body: body.to_vec(),
        }
    }

    fn entry(body: &str) -> HistoryEntry {
        let out = |auth: &str| Outgoing { method: "POST".into(), url: "https://api/x".into(), headers: vec![("Authorization".into(), auth.into())], body: Some(body.into()) };
        let prepared = Prepared { sent: out("Bearer t0k-secret"), shown: out("Bearer •••") };
        HistoryEntry::new(1, None, None, "{{baseUrl}}/x".into(), &prepared, Outcome::Failed { error: "timed out".into() }, b"")
    }

    #[test]
    fn a_collection_is_kept_by_its_id_whatever_its_name() {
        with_app_dir("http-collections-name", || {
            let named = PROBE.replacen("\"name\": \"", "\"name\": \"../x", 1);
            let id = add(&Collection::parse(&named).unwrap()).unwrap();
            let file = dir().unwrap().join("collections").join(format!("{id}{SUFFIX}"));
            assert_eq!(std::fs::read_to_string(&file).unwrap(), named, "written as it came");
            assert!(get(&id).unwrap().name().starts_with("../x"));
            let files: Vec<String> = std::fs::read_dir(dir().unwrap().join("collections")).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
            assert_eq!(files, [format!("{id}{SUFFIX}")], "one file, named by its id");
            assert_eq!(std::fs::read_dir(dir().unwrap()).unwrap().count(), 1, "nothing beside the collections folder");
            assert_eq!(all().unwrap().iter().map(|(i, _)| i.as_str()).collect::<Vec<_>>(), [id.as_str()]);
            assert_eq!(get("../evil").err(), Some(HttpError::BadId("../evil".into())));
            assert_eq!(get("nope").err(), Some(HttpError::NoCollection("nope".into())));
        });
    }

    #[test]
    fn a_broken_collection_is_listed_with_why() {
        with_app_dir("http-collections-broken", || {
            let ok = add(&Collection::parse(PROBE).unwrap()).unwrap();
            app_dir::write_private(&collection_path("zzz").unwrap(), b"{").unwrap();
            app_dir::write_private(&collection_path("000").unwrap(), b"{").unwrap();
            let listed = all().unwrap();
            let ids: Vec<&str> = listed.iter().map(|(id, _)| id.as_str()).collect();
            assert_eq!(ids, ["000", ok.as_str(), "zzz"], "in the order of their ids, not of the folder");
            assert!(listed.iter().any(|(id, c)| id == &ok && c.is_ok()));
            assert!(matches!(&listed.iter().find(|(id, _)| id == "zzz").unwrap().1, Err(HttpError::NotACollection(_))));
        });
    }

    #[test]
    fn a_response_reads_back_as_written_and_is_sealed() {
        with_app_dir("http-responses-round-trip", || {
            let body = b"{\"access_token\":\"tok-hunter2\"}\n\x00\xff binary";
            save_response("c", "r", &response(200, body)).unwrap();
            assert_eq!(last_response("c", "r").unwrap(), Some(response(200, body)));
            let raw = std::fs::read(response_path("c", "r").unwrap()).unwrap();
            assert!(!String::from_utf8_lossy(&raw).contains("hunter2"), "the token is on disk in the clear");

            let failed = SavedResponse { outcome: Outcome::Failed { error: "connection refused".into() }, body: Vec::new(), ..response(0, b"") };
            save_response("c", "r", &failed).unwrap();
            assert_eq!(last_response("c", "r").unwrap(), Some(failed), "the new one in place of the old");
            assert_eq!(last_response("c", "never").unwrap(), None);
            assert!(last_response("..", "r").is_err());
        });
    }

    #[test]
    fn deleting_a_collection_or_a_request_takes_its_responses() {
        with_app_dir("http-responses-delete", || {
            let a = add(&Collection::parse(PROBE).unwrap()).unwrap();
            let b = add(&Collection::parse(PROBE).unwrap()).unwrap();
            save_response(&a, "r1", &response(200, b"a")).unwrap();
            save_response(&a, "r2", &response(200, b"a2")).unwrap();
            save_response(&b, "r1", &response(200, b"b")).unwrap();
            delete_response(&a, "r2").unwrap();
            delete_response(&a, "r2").unwrap();
            assert_eq!(last_response(&a, "r2").unwrap(), None);
            delete(&a).unwrap();
            assert!(get(&a).is_err());
            assert_eq!(last_response(&a, "r1").unwrap(), None);
            assert_eq!(last_response(&b, "r1").unwrap().map(|r| r.body), Some(b"b".to_vec()), "the same ids in another copy stay");
        });
    }

    #[test]
    fn a_response_without_its_request_is_pruned() {
        with_app_dir("http-responses-prune", || {
            save_response("c", "kept", &response(200, b"1")).unwrap();
            save_response("c", "gone", &response(200, b"2")).unwrap();
            save_response("old", "r", &response(200, b"3")).unwrap();
            save_response("unread", "r", &response(200, b"4")).unwrap();
            let known = HashMap::from([("c".to_string(), Some(HashSet::from(["kept".to_string()]))), ("unread".to_string(), None)]);
            prune_responses(&known).unwrap();
            assert!(last_response("c", "kept").unwrap().is_some());
            assert_eq!(last_response("c", "gone").unwrap(), None);
            assert_eq!(last_response("old", "r").unwrap(), None);
            assert!(last_response("unread", "r").unwrap().is_some(), "a file that did not read keeps its responses");

            // Finder's file in the folder, and a save's temporary file beside a response: neither is a response.
            let root = dir().unwrap().join("responses");
            std::fs::write(root.join(".DS_Store"), b"x").unwrap();
            let temp = root.join("c").join("kept.123-4.tmp");
            std::fs::write(&temp, b"x").unwrap();
            prune_responses(&known).unwrap();
            assert!(temp.exists() && root.join(".DS_Store").exists());
            keep_under(&root, 0).unwrap();
            assert!(temp.exists(), "the trim leaves a save in progress alone");
        });
    }

    #[test]
    fn past_the_limit_the_oldest_responses_go_first() {
        with_app_dir("http-responses-cap", || {
            for (i, request) in ["r0", "r1", "r2"].iter().enumerate() {
                save_response("c", request, &response(200, &[b'x'; 1000])).unwrap();
                let file = std::fs::File::options().write(true).open(response_path("c", request).unwrap()).unwrap();
                file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000 + i as u64)).unwrap();
            }
            let one = std::fs::metadata(response_path("c", "r0").unwrap()).unwrap().len();
            keep_under(&dir().unwrap().join("responses"), one * 2).unwrap();
            assert_eq!(last_response("c", "r0").unwrap(), None, "the oldest went");
            assert!(last_response("c", "r1").unwrap().is_some() && last_response("c", "r2").unwrap().is_some());
            keep_under(&dir().unwrap().join("responses"), 0).unwrap();
            assert!(last_response("c", "r2").unwrap().is_some(), "the newest stays even alone over the limit");
        });
    }

    /// The limit is kept by the save itself. A sparse file stands for 200 MB of old responses.
    #[test]
    fn a_save_past_the_limit_drops_the_oldest() {
        with_app_dir("http-responses-save-cap", || {
            save_response("c", "old", &response(200, b"1")).unwrap();
            let file = std::fs::File::options().write(true).open(response_path("c", "old").unwrap()).unwrap();
            file.set_len(RESPONSES_BYTES).unwrap();
            file.set_modified(std::time::UNIX_EPOCH).unwrap();
            save_response("c", "new", &response(200, b"2")).unwrap();
            assert!(!response_path("c", "old").unwrap().exists());
            assert!(last_response("c", "new").unwrap().is_some());
        });
    }

    #[test]
    fn a_response_opens_only_as_a_response() {
        with_app_dir("http-responses-purpose", || {
            save_response("c", "r", &response(200, b"1")).unwrap();
            let key = master_key::resolve().unwrap();
            // A whole response, so only the purpose stands between it and being read.
            let mut plain = serde_json::to_vec(&response(200, b"")).unwrap();
            plain.extend_from_slice(b"\nbody");
            let other = secret_store::seal(&key, SecretPurpose::HttpEnvironments, &plain).unwrap();
            std::fs::write(response_path("c", "r").unwrap(), other).unwrap();
            assert!(last_response("c", "r").is_err());
        });
    }

    #[test]
    fn recording_keeps_the_last_200() {
        with_app_dir("http-history-trim", || {
            for i in 0..=HISTORY_ENTRIES {
                record(entry(&i.to_string())).unwrap();
            }
            let log = history().unwrap();
            assert_eq!(log.len(), HISTORY_ENTRIES);
            assert_eq!(log[0].request.body.as_deref(), Some("1"), "the first went");
        });
    }

    #[test]
    fn history_is_sealed_masked_and_kept_in_order() {
        with_app_dir("http-history", || {
            assert!(history().unwrap().is_empty());
            record(entry("first")).unwrap();
            record(entry("second")).unwrap();
            let log = history().unwrap();
            assert_eq!(log.iter().map(|e| e.request.body.as_deref().unwrap()).collect::<Vec<_>>(), ["first", "second"]);
            assert_eq!(log[0].request.headers[0].1, "Bearer •••");
            let raw = std::fs::read(history_path().unwrap()).unwrap();
            assert!(raw.starts_with(b"ATLS"));
            assert!(!String::from_utf8_lossy(&raw).contains("t0k-secret"));
            std::fs::write(history_path().unwrap(), b"not sealed").unwrap();
            assert!(history().is_err());
            assert!(record(entry("third")).is_err(), "a log that does not open is not written over");
        });
    }
}
