//! The HTTP page's environments and globals, sealed under the app master key
//! as MongoDB's connections are (`mongo_store`): one blob, everything in it.
//! The window gets an environment only through `Environment::for_window`.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

use crate::domain::http_client::{Environment, HttpError, Variable};
use crate::infra::app_dir;
use crate::infra::master_key;
use crate::infra::secret_store::{self, SecretPurpose};

/// Under the feature's data folder, which "Delete data" removes (`domain::features`).
const FILE: &str = "environments.enc";
const PURPOSE: SecretPurpose = SecretPurpose::HttpEnvironments;

/// From the read to the write of a change: two saves at once would each
/// write the list without the other's environment.
static WRITING: Mutex<()> = Mutex::new(());

fn writing() -> MutexGuard<'static, ()> {
    WRITING.lock().unwrap_or_else(PoisonError::into_inner)
}

fn path() -> Result<PathBuf, HttpError> {
    Ok(super::http_collections::dir()?.join(FILE))
}

/// The decrypted file, every value wiped when it drops.
#[derive(Default, Serialize, Deserialize)]
struct Stored {
    environments: Vec<Environment>,
    globals: Vec<Variable>,
}

impl Drop for Stored {
    fn drop(&mut self) {
        self.environments.iter_mut().for_each(wipe);
        wipe_all(&mut self.globals);
    }
}

/// Before an environment or globals leave the list: `Stored` wipes only what is still in it.
fn wipe(environment: &mut Environment) {
    wipe_all(&mut environment.variables);
}

fn wipe_all(variables: &mut [Variable]) {
    variables.iter_mut().for_each(|v| v.value.zeroize());
}

/// No file is none saved; a file that does not open says so, rather than
/// reading as an empty list the next save would write over.
fn load() -> Result<Stored, HttpError> {
    let path = path()?;
    let shown = path.display();
    let blob = match std::fs::read(&path) {
        Ok(blob) => blob,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Stored::default()),
        Err(e) => return Err(HttpError::Store(format!("could not read {shown}: {e}"))),
    };
    let key = master_key::resolve().map_err(|e| HttpError::Store(format!("the key the environments are sealed under is out of reach: {e}")))?;
    let plain = secret_store::open(&key, PURPOSE, &blob)
        // Not "Delete data": that is the whole folder, collections and all.
        .map_err(|_| HttpError::Store(format!("{shown} does not open with this machine's key — delete that file and add the environments again")))?;
    serde_json::from_slice(&plain).map_err(|_| HttpError::Store(format!("{shown} is damaged")))
}

fn save(stored: &Stored) -> Result<(), HttpError> {
    let plain = Zeroizing::new(serde_json::to_vec(stored).map_err(|e| HttpError::Store(e.to_string()))?);
    let key = master_key::resolve().map_err(HttpError::Store)?;
    let blob = secret_store::seal(&key, PURPOSE, &plain).map_err(|e| HttpError::Store(e.to_string()))?;
    app_dir::write_private(&path()?, &blob).map_err(HttpError::Store)
}

/// What the window lists, in the order they were added: a secret masked.
pub fn list() -> Result<Vec<Environment>, HttpError> {
    Ok(load()?.environments.iter().map(Environment::for_window).collect())
}

/// An environment whole, secrets and all, to send with. Never crosses the IPC boundary.
pub fn get(id: &str) -> Result<Environment, HttpError> {
    load()?.environments.iter().find(|e| e.id == id).cloned().ok_or_else(|| HttpError::NoEnvironment(id.into()))
}

/// The window's form saved over what is stored (`Environment::edited`): a
/// secret left masked or empty keeps its value. A new id is a new environment.
pub fn save_from_window(from_window: Environment) -> Result<(), HttpError> {
    let _held = writing();
    let mut stored = load()?;
    let position = stored.environments.iter().position(|e| e.id == from_window.id);
    let blank = Environment { id: String::new(), name: String::new(), variables: Vec::new() };
    let edited = position.map_or(&blank, |at| &stored.environments[at]).edited(from_window)?;
    match position {
        Some(at) => wipe(&mut std::mem::replace(&mut stored.environments[at], edited)),
        None => stored.environments.push(edited),
    }
    save(&stored)
}

/// An environment that came in by Import, its values as they were in the file.
pub fn add(environment: Environment) -> Result<(), HttpError> {
    let _held = writing();
    let mut stored = load()?;
    stored.environments.push(environment);
    save(&stored)
}

/// Gone is gone: deleting what is not there is not an error.
pub fn delete(id: &str) -> Result<(), HttpError> {
    let _held = writing();
    let mut stored = load()?;
    let before = stored.environments.len();
    stored.environments.iter_mut().filter(|e| e.id == id).for_each(wipe);
    stored.environments.retain(|e| e.id != id);
    if stored.environments.len() == before {
        return Ok(());
    }
    save(&stored)
}

/// Globals whole, to send with.
pub fn globals() -> Result<Vec<Variable>, HttpError> {
    Ok(load()?.globals.clone())
}

/// Globals imported from Postman take the place of the ones before, as there is one set.
pub fn set_globals(globals: Vec<Variable>) -> Result<(), HttpError> {
    let _held = writing();
    let mut stored = load()?;
    wipe_all(&mut std::mem::replace(&mut stored.globals, globals));
    save(&stored)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::with_app_dir;

    fn var(key: &str, value: &str, secret: bool) -> Variable {
        Variable { key: key.into(), value: value.into(), secret, enabled: true }
    }

    fn env(id: &str, variables: Vec<Variable>) -> Environment {
        Environment { id: id.into(), name: format!("env {id}"), variables }
    }

    #[test]
    fn the_file_is_under_the_features_data() {
        use crate::domain::features::Feature;
        with_app_dir("http-env-data", || {
            let data = app_dir::dir().unwrap().join(Feature::HttpClient.manifest().data.unwrap());
            assert!(path().unwrap().starts_with(data));
        });
    }

    #[test]
    fn the_window_gets_the_mask_and_a_form_without_the_secret_keeps_it() {
        with_app_dir("http-env-window", || {
            save_from_window(env("a", vec![var("baseUrl", "https://api", false), var("token", "t0k-secret", true)])).unwrap();
            let shown = list().unwrap();
            assert_eq!(shown[0].variables[1].value, "•••");
            let mut back = shown[0].clone();
            back.variables[0].value = "https://other".into();
            save_from_window(back).unwrap();
            let stored = get("a").unwrap();
            assert_eq!((stored.variables[0].value.as_str(), stored.variables[1].value.as_str()), ("https://other", "t0k-secret"));
            assert!(matches!(get("b"), Err(HttpError::NoEnvironment(id)) if id == "b"));
        });
    }

    #[test]
    fn a_new_environment_with_the_mask_is_refused() {
        with_app_dir("http-env-new-mask", || {
            let refused = save_from_window(env("n", vec![var("token", "•••", true)]));
            assert_eq!(refused, Err(HttpError::MaskWithoutValue("token".into())));
            assert!(list().unwrap().is_empty());
        });
    }

    #[test]
    fn the_file_holds_no_secret_and_opens_only_as_itself() {
        with_app_dir("http-env-sealed", || {
            add(env("a", vec![var("token", "hunter2-secret", true)])).unwrap();
            set_globals(vec![var("g", "global-hunter3", false)]).unwrap();
            let raw = std::fs::read(path().unwrap()).unwrap();
            let text = String::from_utf8_lossy(&raw);
            assert!(!text.contains("hunter2") && !text.contains("hunter3"), "a value is on disk in the clear");
            assert_eq!(globals().unwrap(), vec![var("g", "global-hunter3", false)]);

            let key = master_key::resolve().unwrap();
            let other = secret_store::seal(&key, SecretPurpose::MongoConnections, br#"{"environments":[],"globals":[]}"#).unwrap();
            std::fs::write(path().unwrap(), other).unwrap();
            let refused = list().unwrap_err().to_string();
            assert!(refused.contains("does not open"), "{refused}");
            assert!(add(env("b", vec![])).is_err(), "a save does not write over what it could not read");
            assert!(set_globals(vec![]).is_err());
        });
    }

    #[test]
    fn a_file_that_cannot_be_read_says_so() {
        with_app_dir("http-env-unreadable", || {
            std::fs::create_dir_all(path().unwrap()).unwrap();
            let refused = list().unwrap_err().to_string();
            assert!(refused.contains("could not read"), "{refused}");
        });
    }

    #[test]
    fn deleting_one_leaves_the_others_and_globals_are_replaced() {
        with_app_dir("http-env-delete", || {
            add(env("a", vec![])).unwrap();
            add(env("b", vec![])).unwrap();
            delete("a").unwrap();
            delete("a").unwrap();
            assert_eq!(list().unwrap().iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["b"]);
            set_globals(vec![var("x", "1", false)]).unwrap();
            set_globals(vec![var("y", "2", false)]).unwrap();
            assert_eq!(globals().unwrap(), vec![var("y", "2", false)]);
        });
    }
}
