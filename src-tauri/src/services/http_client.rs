//! The HTTP page's use cases over its two stores (`docs/45-http-client.md`):
//! a request sent and what came back kept, a Postman file imported into
//! whichever store it belongs to, and saved responses whose request is gone dropped.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex, MutexGuard, PoisonError};

use serde::Serialize;
use serde_json::Value;

use crate::domain::http_client::{Environment, HistoryEntry, HttpError, Outcome, Prepared, ResponseView, SavedResponse, Scope, Variable};
use crate::domain::http_scripts::{changes, overlaid, request_after, request_for_script, request_sent, response_for, ScriptReport, ScriptVars};
use crate::domain::postman::{environment_to_postman, read_import, Collection, Imported};
use crate::infra::postman_sandbox::{Ran, Sandbox, LIMITS};
use crate::infra::{http_agent, http_client, http_collections, http_env_store, settings_store};

/// Sends a collection's request — `edited`, the page's unsaved `request`, in
/// place of the stored one — into `environment`. What came back is on disk, as
/// the request's last response and in history, before the window gets it.
/// A variable found nowhere fails here, before anything is sent. So does a
/// pre-request script that fails: what it was for — a token, say — did not happen.
/// What scripts wrote is kept either way.
pub fn send(collection_id: &str, request_id: &str, edited: Option<Value>, environment: Option<&str>) -> Result<ResponseView, HttpError> {
    let mut collection = http_collections::get(collection_id)?;
    if let Some(edited) = edited {
        let item = collection.item_mut(request_id).ok_or_else(|| HttpError::NoSuchItem(request_id.into()))?;
        item.insert("request".into(), edited);
    }
    let environment = environment.map(http_env_store::get).transpose()?;
    let values = http_env_store::collection_values()?;
    let before = ScriptVars {
        globals: http_env_store::globals()?,
        collection: overlaid(collection.variables(), values.get(collection_id).map(Vec::as_slice).unwrap_or_default()),
        environment: environment.as_ref().map(|e| e.variables.clone()).unwrap_or_default(),
        local: Vec::new(),
    };
    let mut report = ScriptReport::default();
    let mut scripts = Scripts::new(&collection, collection_id, request_id, environment.as_ref(), before.clone(), &mut report)?;
    let sent = exchange(&mut collection, collection_id, request_id, environment.as_ref(), &mut scripts);
    let mask = scripts.mask();
    let after = scripts.vars;
    match write_variables(&before, &after, collection_id, environment.as_ref()) {
        Ok(changed) => report.changed = changed,
        Err(e) => {
            report.error.get_or_insert(format!("What the scripts wrote was not kept: {e}"));
        }
    }
    let sent = sent.map_err(|e| match e {
        HttpError::Script(why) => HttpError::Script(mask.hide(&why)),
        other => other,
    })?;
    save(collection_id, request_id, sent, report.hidden(&mask))
}

/// A request through its scripts, and what came back with its secrets hidden.
fn exchange(collection: &mut Collection, collection_id: &str, request_id: &str, environment: Option<&Environment>, scripts: &mut Scripts) -> Result<Sent, HttpError> {
    // The last to start, not the last past its scripts: one can wait a minute there.
    let key = format!("{collection_id}/{request_id}");
    let ticket = NEXT.fetch_add(1, Ordering::Relaxed);
    latest().insert(key.clone(), ticket);
    scripts.before_sending(collection, request_id)?;
    let prepared = collection.prepare(request_id, &scripts.vars.scope())?;
    let (outcome, body) = http_client::send(&prepared.sent, environment.and_then(|e| e.trusted_cert.as_deref()));
    let at = chrono::Utc::now().timestamp_millis();
    let name = environment.map(|e| e.name.clone());
    let written = collection.written_url(request_id).unwrap_or_default();
    remember(at, name.clone(), written, Some(request_id), &prepared, &outcome, &body, &scripts.mask());
    scripts.after_response(&prepared, &outcome, &body);
    // What the test scripts learned counts too: a token they stored is a secret's value.
    let outcome = outcome.hidden(&scripts.mask());
    Ok(Sent { key, ticket, saved: SavedResponse { at, environment: name, outcome, body, scripts: ScriptReport::default() } })
}

/// A request that went out, on its way to the store.
struct Sent {
    key: String,
    ticket: u64,
    saved: SavedResponse,
}

/// A store that fails now must not hide what came back: the request went out.
#[allow(clippy::too_many_arguments)]
fn remember(at: i64, environment: Option<String>, written: String, request_id: Option<&str>, prepared: &Prepared, outcome: &Outcome, body: &[u8], scope: &Scope) {
    let mut entry = HistoryEntry::new(at, environment, None, scope.hide(&written), &prepared.hidden(scope), outcome.clone().hidden(scope), body);
    entry.request_id = request_id.map(str::to_string);
    if let Err(e) = http_collections::record(entry) {
        eprintln!("http: a send not written to history: {e}");
    }
}

/// Whether a variable was written.
fn write_variables(before: &ScriptVars, after: &ScriptVars, collection_id: &str, environment: Option<&Environment>) -> Result<bool, HttpError> {
    let (globals, values, own) = (changes(&before.globals, &after.globals), changes(&before.collection, &after.collection), changes(&before.environment, &after.environment));
    let own = environment.map(|e| (e.id.as_str(), own.as_slice())).filter(|(_, c)| !c.is_empty());
    if globals.is_empty() && values.is_empty() && own.is_none() {
        return Ok(false);
    }
    http_env_store::write_changes(own, &globals, (collection_id, &values))?;
    Ok(true)
}

/// A send's scripts: none, when the collection has none or is not trusted.
struct Scripts<'a> {
    /// The variables as the send found them, and as the scripts have left them.
    start: ScriptVars,
    vars: ScriptVars,
    report: &'a mut ScriptReport,
    environment: Option<&'a Environment>,
    /// What runs before the request and after it, in order.
    before: Vec<String>,
    after: Vec<String>,
    sandbox: Option<Sandbox>,
    name: String,
}

impl<'a> Scripts<'a> {
    fn new(collection: &Collection, id: &str, request_id: &str, environment: Option<&'a Environment>, vars: ScriptVars, report: &'a mut ScriptReport) -> Result<Scripts<'a>, HttpError> {
        let digest = collection.scripts_digest();
        let trusted = digest.as_ref().is_some_and(|d| is_trusted(id, &d.hash));
        report.not_run = digest.is_some() && !trusted;
        let (before, after) = if trusted { (collection.scripts(request_id, "prerequest")?, collection.scripts(request_id, "test")?) } else { Default::default() };
        let name = collection.item_name(request_id).unwrap_or_default();
        Ok(Scripts { start: vars.clone(), vars, report, environment, before, after, sandbox: None, name })
    }

    fn mask(&self) -> Scope {
        ScriptVars::mask(&[&self.start, &self.vars])
    }

    /// One script over the variables as the last one left them.
    fn run(&mut self, listen: &str, code: &str, request: &Value, response: Option<&Value>) -> Result<Ran, HttpError> {
        if self.sandbox.is_none() {
            self.sandbox = Some(Sandbox::boot(LIMITS)?);
        }
        let (scope, mask) = (self.vars.scope(), self.mask());
        let environment = self.environment;
        let context = self.vars.context(request, response);
        let sandbox = self.sandbox.as_ref().expect("booted above");
        let ran = sandbox.run(listen, code, &self.name, context, &mut |sub| sub_request(sub, &scope, &mask, environment))?;
        self.vars.absorb(&ran.result);
        self.report.tests.extend(ran.tests.iter().cloned());
        self.report.console.extend(ran.console.iter().cloned());
        Ok(ran)
    }

    /// The pre-request scripts, each changing the request the next one gets.
    fn before_sending(&mut self, collection: &mut Collection, request_id: &str) -> Result<(), HttpError> {
        for code in std::mem::take(&mut self.before) {
            let item = collection.item_mut(request_id).ok_or_else(|| HttpError::NoSuchItem(request_id.into()))?;
            let request = item.get("request").cloned().unwrap_or_default();
            let ran = self.run("prerequest", &code, &request_for_script(&request), None)?;
            if let Some(after) = ran.result.get("request") {
                item.insert("request".into(), request_after(&request, after));
            }
            if let Some(error) = ran.error {
                return Err(HttpError::Script(format!("A pre-request script failed, and nothing was sent: {error}")));
            }
            if ran.skip_request {
                return Err(HttpError::Script("A pre-request script skipped the request.".into()));
            }
        }
        Ok(())
    }

    /// The test scripts, when there is a response to test. One that fails is said
    /// beside the response, which stands; the rest still run, as in Postman.
    fn after_response(&mut self, prepared: &Prepared, outcome: &Outcome, body: &[u8]) {
        let Outcome::Response { status, headers, elapsed_ms, .. } = outcome else { return };
        let (request, response) = (request_sent(&prepared.sent), response_for(*status, headers, body, *elapsed_ms));
        for code in std::mem::take(&mut self.after) {
            match self.run("test", &code, &request, Some(&response)) {
                Ok(ran) => {
                    if let Some(error) = ran.error {
                        self.report.error.get_or_insert(error);
                    }
                }
                // Past a limit the sandbox is not used again.
                Err(e) => {
                    self.report.error = Some(e.to_string());
                    return;
                }
            }
        }
    }
}

/// `pm.sendRequest`: the same sender and the same history as the page's own requests.
fn sub_request(request: &Value, scope: &Scope, mask: &Scope, environment: Option<&Environment>) -> Result<Value, HttpError> {
    let request = request_after(&Value::Null, request);
    let written = request.get("url").map(|u| u.get("raw").unwrap_or(u)).and_then(Value::as_str).unwrap_or_default().to_string();
    let mut one = Collection::new("sub-request", "sub-request");
    one.add(None, serde_json::json!({"id": "sub-request", "name": "sub-request", "request": request}))?;
    let prepared = one.prepare("sub-request", scope)?;
    let (outcome, body) = http_client::send(&prepared.sent, environment.and_then(|e| e.trusted_cert.as_deref()));
    let at = chrono::Utc::now().timestamp_millis();
    remember(at, environment.map(|e| e.name.clone()), written, None, &prepared, &outcome, &body, mask);
    match outcome {
        Outcome::Response { status, headers, elapsed_ms, .. } => Ok(response_for(status, &headers, &body, elapsed_ms)),
        Outcome::Failed { error } => Err(HttpError::Script(error)),
    }
}

/// Settings that cannot be read trust nothing.
fn is_trusted(collection: &str, hash: &str) -> bool {
    settings_store::load().unwrap_or_default().http_script_trust.get(collection).is_some_and(|trusted| trusted == hash)
}

/// Whether a collection's scripts run, for the window to ask before the first send.
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptTrust {
    /// Of the scripts as they are now; none when there are none to trust.
    pub hash: Option<String>,
    pub count: usize,
    pub trusted: bool,
}

pub fn script_trust(collection: &str) -> Result<ScriptTrust, HttpError> {
    let digest = http_collections::get(collection)?.scripts_digest();
    let trusted = digest.as_ref().is_some_and(|d| is_trusted(collection, &d.hash));
    Ok(digest.map_or(ScriptTrust { hash: None, count: 0, trusted: false }, |d| ScriptTrust { hash: Some(d.hash), count: d.count, trusted }))
}

/// Trust in the scripts the window showed: `hash` is theirs, so an import that
/// changed them since is not trusted by an answer about the old ones.
pub fn trust_scripts(collection: &str, hash: &str) -> Result<(), HttpError> {
    if http_collections::get(collection)?.scripts_digest().is_none_or(|d| d.hash != hash) {
        return Err(HttpError::Script("The collection's scripts changed — look at them again.".into()));
    }
    settings_store::update(|s| s.http_script_trust.insert(collection.to_string(), hash.to_string())).map_err(|e| HttpError::Store(e.to_string()))?;
    Ok(())
}

fn save(collection_id: &str, request_id: &str, sent: Sent, report: ScriptReport) -> Result<ResponseView, HttpError> {
    let Sent { key, ticket, saved } = sent;
    let saved = SavedResponse { scripts: report, ..saved };
    // Held through the write, so a later send that ends first is not overwritten by this one.
    let latest = latest();
    if latest.get(&key) == Some(&ticket) {
        if let Err(e) = http_collections::save_response(collection_id, request_id, &saved) {
            eprintln!("http: a response not saved: {e}");
        }
    }
    drop(latest);
    Ok(saved.into())
}

/// Sends numbered as they start; and the last to start, per `<collection>/<request>`.
/// The page shows the last send's response (decision 3), so it is the one kept.
// ponytail: one lock for every request's save — per-request locks if many save at once.
static NEXT: AtomicU64 = AtomicU64::new(0);
static LATEST: LazyLock<Mutex<HashMap<String, u64>>> = LazyLock::new(Mutex::default);

fn latest() -> MutexGuard<'static, HashMap<String, u64>> {
    LATEST.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The response a request's tab shows on opening; none before its first send.
pub fn last_response(collection_id: &str, request_id: &str) -> Result<Option<ResponseView>, HttpError> {
    Ok(http_collections::last_response(collection_id, request_id)?.map(ResponseView::from))
}

/// What an Import brought in, for the page to open it.
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ImportedAs {
    Collection { id: String, name: String },
    Environment { id: String, name: String },
    Globals { count: usize },
}

pub fn import(text: &str) -> Result<ImportedAs, HttpError> {
    match read_import(text)? {
        Imported::Collection(mut collection) => {
            let taken = http_collections::all()?.into_iter().filter_map(|(_, c)| c.ok()).flat_map(|c| c.ids()).collect();
            collection.ensure_ids_besides(&taken);
            Ok(ImportedAs::Collection { id: http_collections::add(&collection)?, name: collection.name() })
        }
        Imported::Environment(environment) => {
            let done = ImportedAs::Environment { id: environment.id.clone(), name: environment.name.clone() };
            http_env_store::add(environment)?;
            Ok(done)
        }
        Imported::Globals(globals) => {
            let count = globals.len();
            http_env_store::set_globals(globals)?;
            Ok(ImportedAs::Globals { count })
        }
    }
}

/// A collection as the window gets it: the file's tree, or why it does not read.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionView {
    pub id: String,
    pub file: Option<Value>,
    pub error: Option<String>,
    /// What scripts wrote to its variables, which lie over the file's.
    pub current: Vec<Variable>,
}

/// Every collection, as the page opens — when the responses of requests no
/// collection has any more go: one imported again with other ids leaves the
/// old ids' responses behind.
pub fn collections() -> Result<Vec<CollectionView>, HttpError> {
    let all = http_collections::all()?;
    let known: HashMap<String, Option<HashSet<String>>> =
        all.iter().map(|(id, collection)| (id.clone(), collection.as_ref().ok().map(|c| c.request_ids().into_iter().collect()))).collect();
    if let Err(e) = http_collections::prune_responses(&known) {
        eprintln!("http: saved responses not pruned: {e}");
    }
    // Sealed apart: a store that does not open must not take the collections with it.
    let mut values = http_env_store::collection_values().unwrap_or_else(|e| {
        eprintln!("http: script values not read: {e}");
        Default::default()
    });
    Ok(all
        .into_iter()
        .map(|(id, collection)| {
            let current = values.remove(&id).unwrap_or_default();
            match collection {
                Ok(c) => {
                    let mine = c.variables();
                    let shown = current.iter().map(|v| Variable { secret: mine.iter().any(|m| m.key == v.key && m.secret), ..v.clone() }.for_window()).collect();
                    CollectionView { id, file: Some(c.json().clone()), error: None, current: shown }
                }
                Err(e) => CollectionView { id, file: None, error: Some(e.to_string()), current: Vec::new() },
            }
        })
        .collect())
}

pub fn new_collection(name: &str) -> Result<CollectionView, HttpError> {
    let id = uuid::Uuid::new_v4().to_string();
    let collection = Collection::new(&id, name);
    http_collections::save(&id, &collection)?;
    Ok(CollectionView { id, file: Some(collection.json().clone()), error: None, current: Vec::new() })
}

/// Held while a collection file is written or deleted.
// ponytail: one lock for every collection — per-collection locks if writes ever queue behind each other.
fn writing() -> MutexGuard<'static, ()> {
    static WRITING: Mutex<()> = Mutex::new(());
    WRITING.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A stored collection read, changed and written, one change at a time: two
/// edits of one file at once would each write it without the other's.
fn change<T>(id: &str, edit: impl FnOnce(&mut Collection) -> Result<T, HttpError>) -> Result<T, HttpError> {
    let _held = writing();
    let mut collection = http_collections::get(id)?;
    let done = edit(&mut collection)?;
    http_collections::save(id, &collection)?;
    Ok(done)
}

/// Under the edits' lock: one that read the file before would write it back after.
pub fn delete_collection(id: &str) -> Result<(), HttpError> {
    let _held = writing();
    http_collections::delete(id)?;
    http_env_store::forget_collection(id)?;
    settings_store::update(|s| s.http_script_trust.remove(id)).map_err(|e| HttpError::Store(e.to_string()))?;
    Ok(())
}

/// The page's edit of a request, the rest of the file as it was.
pub fn save_request(collection: &str, request_id: &str, request: Value) -> Result<(), HttpError> {
    change(collection, |c| c.set_request(request_id, request))
}

pub fn add_item(collection: &str, parent: Option<&str>, item: Value) -> Result<(), HttpError> {
    change(collection, |c| c.add(parent, item))
}

/// The collection itself when `id` is its own, else its folder or request.
pub fn rename(collection: &str, id: &str, name: &str) -> Result<(), HttpError> {
    change(collection, |c| {
        if id == collection {
            c.set_name(name);
        } else {
            c.item_mut(id).ok_or_else(|| HttpError::NoSuchItem(id.into()))?.insert("name".into(), name.into());
        }
        Ok(())
    })
}

/// A folder or request gone, and the last responses of the requests it took.
pub fn delete_item(collection: &str, id: &str) -> Result<(), HttpError> {
    for request in change(collection, |c| c.remove(id))? {
        // Left behind, it goes when the page next opens.
        if let Err(e) = http_collections::delete_response(collection, &request) {
            eprintln!("http: a deleted request's response not deleted: {e}");
        }
    }
    Ok(())
}

/// The file at `path`, picked in the system's dialog, into the store it belongs to.
pub fn import_file(path: &Path) -> Result<ImportedAs, HttpError> {
    import(&std::fs::read_to_string(path).map_err(|e| HttpError::Store(format!("could not read {}: {e}", path.display())))?)
}

/// The stored collection as it is, at `dest`.
pub fn export_collection(id: &str, dest: &Path) -> Result<(), HttpError> {
    write_json(dest, &http_collections::get(id)?.to_text())
}

/// An environment as Postman imports it, its secrets empty.
pub fn export_environment(id: &str, dest: &Path) -> Result<(), HttpError> {
    write_json(dest, &environment_to_postman(&http_env_store::get(id)?, false))
}

/// Only to a `.json` file: `dest` comes from the window, which names it after a collection.
fn write_json(dest: &Path, text: &str) -> Result<(), HttpError> {
    if dest.extension().and_then(|e| e.to_str()) != Some("json") {
        return Err(HttpError::Store(format!("{} is not a .json file", dest.display())));
    }
    std::fs::write(dest, text).map_err(|e| HttpError::Store(format!("could not write {}: {e}", dest.display())))
}

/// The environments and globals as the window gets them, secrets masked.
#[derive(Debug, Serialize)]
pub struct Environments {
    pub environments: Vec<Environment>,
    pub globals: Vec<Variable>,
}

pub fn environments() -> Result<Environments, HttpError> {
    Ok(Environments { environments: http_env_store::list()?, globals: http_env_store::globals()?.iter().map(Variable::for_window).collect() })
}

/// The form saved; an empty certificate is none. One that does not parse is
/// refused here, rather than at every send as a stand that does not answer.
pub fn save_environment(mut from_window: Environment) -> Result<(), HttpError> {
    from_window.trusted_cert = from_window.trusted_cert.filter(|pem| !pem.trim().is_empty());
    if let Some(pem) = &from_window.trusted_cert {
        http_agent::parse_trusted_certs(pem).map_err(|e| HttpError::BadCertificate(e.0))?;
    }
    http_env_store::save_from_window(from_window)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::http_client::{Environment, Outcome, Variable};
    use crate::domain::postman::Collection;
    use crate::infra::http_client::tests::{closed_port, reply, serve, Seen};
    use crate::testing::with_app_dir;
    use serde_json::json;
    use std::sync::mpsc::Receiver;

    const PROBE: &str = include_str!("../infra/testdata/postman/probe.postman_collection.json");

    #[test]
    fn each_kind_of_file_lands_in_its_store_and_a_secret_is_sealed() {
        with_app_dir("http-import", || {
            let ImportedAs::Collection { id, name } = import(PROBE).unwrap() else { panic!("a collection") };
            assert_eq!(name, "wire_kibo_test");
            assert_eq!(http_collections::get(&id).unwrap().request_ids().len(), 10, "with the ids it was given");

            let env = r#"{"name": "Stage", "values": [{"key": "token", "value": "pm-hunter2", "type": "secret", "enabled": true}], "_postman_variable_scope": "environment"}"#;
            let ImportedAs::Environment { id, name } = import(env).unwrap() else { panic!("an environment") };
            assert_eq!(name, "Stage");
            assert_eq!(http_env_store::get(&id).unwrap().variables[0].value, "pm-hunter2");
            assert_eq!(http_env_store::list().unwrap()[0].variables[0].value, "•••");
            let raw = std::fs::read(http_collections::dir().unwrap().join("environments.enc")).unwrap();
            assert!(!String::from_utf8_lossy(&raw).contains("pm-hunter2"));

            let globals = r#"{"values": [{"key": "a", "value": "1"}, {"key": "b", "value": "2"}], "_postman_variable_scope": "globals"}"#;
            assert_eq!(import(globals).unwrap(), ImportedAs::Globals { count: 2 });
            assert!(matches!(import("{\"openapi\": \"3.0.0\"}"), Err(HttpError::NotACollection(_))));
        });
    }

    #[test]
    fn a_response_whose_request_is_gone_is_pruned() {
        with_app_dir("http-prune", || {
            let ImportedAs::Collection { id, .. } = import(PROBE).unwrap() else { panic!("a collection") };
            let request = http_collections::get(&id).unwrap().request_ids()[0].clone();
            let saved = SavedResponse { at: 1, environment: None, outcome: Outcome::Failed { error: "x".into() }, body: Vec::new(), scripts: Default::default() };
            http_collections::save_response(&id, &request, &saved).unwrap();
            http_collections::save_response(&id, "removed-request", &saved).unwrap();
            http_collections::save_response("removed-collection", "r", &saved).unwrap();
            let ImportedAs::Collection { id: broken, .. } = import(PROBE).unwrap() else { panic!("a collection") };
            http_collections::save_response(&broken, "r", &saved).unwrap();
            let file = http_collections::dir().unwrap().join("collections").join(format!("{broken}.postman_collection.json"));
            std::fs::write(file, "{ damaged").unwrap();
            collections().unwrap();
            assert!(http_collections::last_response(&id, &request).unwrap().is_some());
            assert_eq!(http_collections::last_response(&id, "removed-request").unwrap(), None);
            assert_eq!(http_collections::last_response("removed-collection", "r").unwrap(), None);
            assert!(http_collections::last_response(&broken, "r").unwrap().is_some(), "a file that does not read keeps its responses");
        });
    }

    #[test]
    fn a_broken_file_is_listed_with_why_beside_the_ones_that_read() {
        with_app_dir("http-list", || {
            let made = new_collection("Mine").unwrap();
            let ImportedAs::Collection { id: broken, .. } = import(PROBE).unwrap() else { panic!("a collection") };
            let file = http_collections::dir().unwrap().join("collections").join(format!("{broken}.postman_collection.json"));
            std::fs::write(file, "{ damaged").unwrap();
            let listed = collections().unwrap();
            let mine = listed.iter().find(|c| c.id == made.id).unwrap();
            assert_eq!(mine.file.as_ref().unwrap().pointer("/info/name"), Some(&json!("Mine")));
            let bad = listed.iter().find(|c| c.id == broken).unwrap();
            assert!(bad.file.is_none());
            assert!(bad.error.as_deref().unwrap().contains("not JSON"), "{:?}", bad.error);
        });
    }

    #[test]
    fn the_pages_edits_change_only_what_they_name_and_the_file_reads_back_as_postman() {
        with_app_dir("http-edit", || {
            let ImportedAs::Collection { id, .. } = import(PROBE).unwrap() else { panic!("a collection") };
            let before = http_collections::get(&id).unwrap();
            let request = before.request_ids()[0].clone();
            save_request(&id, &request, json!({"method": "DELETE", "url": "{{baseUrl}}/x"})).unwrap();
            rename(&id, &id, "Renamed").unwrap();
            rename(&id, &request, "Gone soon").unwrap();
            add_item(&id, None, json!({"id": "fresh-1", "name": "New", "request": {"method": "GET", "url": ""}})).unwrap();
            let after = http_collections::get(&id).unwrap();
            assert_eq!(after.name(), "Renamed");
            assert_eq!(after.written_url(&request).as_deref(), Some("{{baseUrl}}/x"));
            assert_eq!(after.request_ids().last().map(String::as_str), Some("fresh-1"));
            // Every other line is the file's own.
            let (old, new) = (before.to_text(), after.to_text());
            let changed = new.lines().filter(|line| !old.lines().any(|o| o == *line)).count();
            assert!(changed < 15, "{changed} lines changed");
            assert!(matches!(add_item(&id, None, json!({"id": "FRESH-1", "name": "Twin"})), Err(HttpError::BadId(_))));
            assert!(matches!(save_request(&id, "nothing", json!({})), Err(HttpError::NoSuchItem(_))));
        });
    }

    #[test]
    fn edits_at_once_each_land() {
        with_app_dir("http-edit-together", || {
            let id = new_collection("Busy").unwrap().id;
            let ids: Vec<String> = (0..12).map(|n| format!("r{n}")).collect();
            for r in &ids {
                add_item(&id, None, json!({"id": r, "name": r, "request": {"method": "GET", "url": ""}})).unwrap();
            }
            let threads: Vec<_> = ids
                .iter()
                .map(|r| {
                    let (id, r) = (id.clone(), r.clone());
                    std::thread::spawn(move || save_request(&id, &r, json!({"method": "POST", "url": format!("/{r}")})).unwrap())
                })
                .collect();
            threads.into_iter().for_each(|t| t.join().unwrap());
            let after = http_collections::get(&id).unwrap();
            for r in &ids {
                assert_eq!(after.written_url(r), Some(format!("/{r}")), "the edit of {r} was lost");
            }
        });
    }

    #[test]
    fn a_deleted_folder_takes_its_requests_responses() {
        with_app_dir("http-delete-item", || {
            let id = new_collection("Shop").unwrap().id;
            add_item(&id, None, json!({"id": "f", "name": "F", "item": []})).unwrap();
            add_item(&id, Some("f"), json!({"id": "in", "name": "In", "request": {"url": "/a"}})).unwrap();
            add_item(&id, None, json!({"id": "out", "name": "Out", "request": {"url": "/b"}})).unwrap();
            let saved = SavedResponse { at: 1, environment: None, outcome: Outcome::Failed { error: "x".into() }, body: Vec::new(), scripts: Default::default() };
            http_collections::save_response(&id, "in", &saved).unwrap();
            http_collections::save_response(&id, "out", &saved).unwrap();
            delete_item(&id, "f").unwrap();
            assert_eq!(http_collections::get(&id).unwrap().request_ids(), ["out"]);
            assert_eq!(http_collections::last_response(&id, "in").unwrap(), None);
            assert!(http_collections::last_response(&id, "out").unwrap().is_some());
        });
    }

    #[test]
    fn an_export_imported_twice_shares_no_ids() {
        with_app_dir("http-import-twice", || {
            // With the ids Postman gave it, as an export of a collection that was imported has.
            let mut withids = Collection::parse(PROBE).unwrap();
            withids.ensure_ids();
            let text = withids.to_text();
            let ImportedAs::Collection { id: first, .. } = import(&text).unwrap() else { panic!("a collection") };
            let ImportedAs::Collection { id: second, .. } = import(&text).unwrap() else { panic!("a collection") };
            let ids = |id: &str| http_collections::get(id).unwrap().ids().into_iter().collect::<HashSet<_>>();
            let (a, b) = (ids(&first), ids(&second));
            assert_eq!(a, withids.ids().into_iter().collect(), "the first keeps its own");
            assert_eq!(b.len(), 14);
            assert!(a.is_disjoint(&b));
        });
    }

    #[test]
    fn an_export_is_the_stored_file_and_only_to_a_json_file() {
        with_app_dir("http-export", || {
            let ImportedAs::Collection { id, .. } = import(PROBE).unwrap() else { panic!("a collection") };
            let dir = crate::testing::temp_dir("http-export-dest");
            let dest = dir.join("shop.postman_collection.json");
            export_collection(&id, &dest).unwrap();
            assert_eq!(std::fs::read_to_string(&dest).unwrap(), http_collections::get(&id).unwrap().to_text());
            let script = dir.join("run.sh");
            assert!(export_collection(&id, &script).is_err());
            assert!(!script.exists());

            stage("http://x".into());
            let env = dir.join("stage.json");
            export_environment("stage", &env).unwrap();
            let text = std::fs::read_to_string(&env).unwrap();
            assert!(text.contains("\"_postman_variable_scope\": \"environment\""), "{text}");
            assert!(!text.contains("s3cr3t-t0k"), "{text}");
        });
    }

    #[test]
    fn a_certificate_that_does_not_parse_is_refused_and_an_empty_one_is_none() {
        use crate::infra::http_agent::tests::CA;
        with_app_dir("http-env-cert", || {
            let env = |cert: &str| Environment { id: "e".into(), name: "E".into(), variables: vec![], trusted_cert: Some(cert.into()) };
            assert!(matches!(save_environment(env("not a certificate")), Err(HttpError::BadCertificate(_))));
            assert!(matches!(http_env_store::get("e"), Err(HttpError::NoEnvironment(_))), "nothing saved");
            save_environment(env("  \n")).unwrap();
            assert_eq!(http_env_store::get("e").unwrap().trusted_cert, None);
            save_environment(env(CA)).unwrap();
            assert_eq!(http_env_store::get("e").unwrap().trusted_cert.as_deref(), Some(CA));
        });
    }

    #[test]
    fn the_window_gets_globals_with_a_secret_masked() {
        with_app_dir("http-env-list", || {
            stage("http://x".into());
            let var = |key: &str, secret| Variable { key: key.into(), value: "pm-hunter2".into(), secret, enabled: true };
            http_env_store::set_globals(vec![var("token", true), var("plain", false)]).unwrap();
            let all = environments().unwrap();
            assert_eq!(all.environments[0].variables[1].value, "•••");
            assert_eq!(all.globals.iter().map(|v| v.value.as_str()).collect::<Vec<_>>(), ["•••", "pm-hunter2"]);
        });
    }

    /// Answers one request with `text`.
    fn serve_once(text: &str) -> (u16, Receiver<Seen>) {
        serve(vec![reply(text)])
    }

    /// What reached the stand, head and body as one text.
    fn wire(seen: &Receiver<Seen>) -> String {
        let seen = seen.recv().unwrap();
        seen.head + "\r\n" + &String::from_utf8(seen.body).unwrap()
    }

    fn shop() -> String {
        let collection = json!({
            "info": {"name": "Shop", "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},
            "auth": {"type": "bearer", "bearer": [{"key": "token", "value": "{{token}}", "type": "string"}]},
            "item": [{"id": "order", "name": "Order", "request": {
                "method": "POST", "url": {"raw": "{{baseUrl}}/orders"},
                "body": {"mode": "raw", "raw": "{\"sum\": {{sum}}}", "options": {"raw": {"language": "json"}}}
            }}],
            "variable": [{"key": "sum", "value": "0"}]
        });
        http_collections::add(&Collection::parse(&collection.to_string()).unwrap()).unwrap()
    }

    fn event(listen: &str, code: &str) -> Value {
        json!({"listen": listen, "script": {"type": "text/javascript", "exec": code.lines().collect::<Vec<_>>()}})
    }

    /// A collection with scripts at each level: `events` on the collection, the
    /// folder and the request, in that order.
    fn scripted(on_collection: Vec<Value>, on_folder: Vec<Value>, on_request: Vec<Value>) -> String {
        let collection = json!({
            "info": {"name": "Scripted", "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},
            "item": [{"id": "folder", "name": "Folder", "event": on_folder, "item": [
                {"id": "order", "name": "Order", "event": on_request, "request": {"method": "POST", "url": {"raw": "{{baseUrl}}/orders"}, "body": {"mode": "raw", "raw": "{}"}}},
            ]}],
            "event": on_collection,
            "variable": [{"key": "fromFile", "value": "file"}, {"key": "apiKey", "value": "", "type": "secret"}],
        });
        http_collections::add(&Collection::parse(&collection.to_string()).unwrap()).unwrap()
    }

    fn trust(id: &str) {
        trust_scripts(id, &script_trust(id).unwrap().hash.unwrap()).unwrap();
    }

    fn env_value(key: &str) -> Option<String> {
        http_env_store::get("stage").unwrap().variables.into_iter().find(|v| v.key == key).map(|v| v.value)
    }

    fn global(key: &str) -> Option<String> {
        http_env_store::globals().unwrap().into_iter().find(|v| v.key == key).map(|v| v.value)
    }

    fn stage(base: String) -> String {
        let var = |key: &str, value: String, secret| Variable { key: key.into(), value, secret, enabled: true };
        let variables = vec![var("baseUrl", base, false), var("token", "s3cr3t-t0k".into(), true)];
        http_env_store::add(Environment { id: "stage".into(), name: "Stage".into(), variables, trusted_cert: None }).unwrap();
        "stage".into()
    }

    #[test]
    fn a_send_is_on_disk_before_the_window_has_it_and_the_secret_only_on_the_wire() {
        with_app_dir("http-send", || {
            let (port, seen) = serve_once("HTTP/1.1 422 Unprocessable Entity\r\ncontent-length: 13\r\nconnection: close\r\n\r\n{\"sum\": \"<1\"}");
            let collection = shop();
            let env = stage(format!("http://127.0.0.1:{port}/v1"));
            // The collection's `sum` is over the globals'.
            http_env_store::set_globals(vec![Variable { key: "sum".into(), value: "9".into(), secret: false, enabled: true }]).unwrap();
            let view = send(&collection, "order", None, Some(&env)).unwrap();
            let wire = wire(&seen);
            assert!(wire.starts_with("POST /v1/orders HTTP/1.1"), "{wire}");
            assert!(wire.contains("Bearer s3cr3t-t0k"), "{wire}");
            assert!(wire.ends_with("\r\n\r\n{\"sum\": 0}"), "{wire}");

            assert!(matches!(view.outcome, Outcome::Response { status: 422, .. }), "{view:?}");
            assert_eq!((view.body.as_deref(), view.size, view.environment.as_deref()), (Some("{\"sum\": \"<1\"}"), 13, Some("Stage")));
            assert_eq!(last_response(&collection, "order").unwrap(), Some(view.clone()));

            let history = http_collections::history().unwrap();
            assert_eq!(history.len(), 1);
            let entry = &history[0];
            assert_eq!((entry.template_url.as_str(), entry.environment.as_deref(), entry.agent_chat.as_deref()), ("{{baseUrl}}/orders", Some("Stage"), None));
            assert_eq!(entry.request_id.as_deref(), Some("order"), "the entry opens its request");
            assert!(entry.request.headers.contains(&("Authorization".into(), "Bearer •••".into())), "{:?}", entry.request.headers);
            assert_eq!(entry.response_body.as_deref(), Some("{\"sum\": \"<1\"}"));
        });
    }

    #[test]
    fn the_pages_edit_is_sent_and_the_stored_request_left_as_it_was() {
        with_app_dir("http-send-edited", || {
            let (port, seen) = serve_once("HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
            let collection = shop();
            let env = stage(format!("http://127.0.0.1:{port}"));
            let before = http_collections::get(&collection).unwrap().to_text();
            let edited = json!({"method": "GET", "url": "{{baseUrl}}/orders/7", "auth": {"type": "noauth"}});
            send(&collection, "order", Some(edited), Some(&env)).unwrap();
            let wire = wire(&seen);
            assert!(wire.starts_with("GET /orders/7 HTTP/1.1"), "{wire}");
            assert!(!wire.to_ascii_lowercase().contains("authorization"), "{wire}");
            assert_eq!(http_collections::get(&collection).unwrap().to_text(), before);
            assert_eq!(http_collections::history().unwrap()[0].template_url, "{{baseUrl}}/orders/7");
            assert!(matches!(send(&collection, "missing", Some(json!({})), None), Err(HttpError::NoSuchItem(_))));
        });
    }

    #[test]
    fn the_environment_wins_over_the_collection_and_brings_its_certificate() {
        use crate::infra::http_agent::tests::{serve_tls_with, CA, LEAF, LEAF_KEY};
        with_app_dir("http-send-env", || {
            let port = serve_tls_with(LEAF, LEAF_KEY, "HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
            let collection = shop();
            let var = |key: &str, value: &str| Variable { key: key.into(), value: value.into(), secret: false, enabled: true };
            let variables = vec![var("baseUrl", &format!("https://127.0.0.1:{port}")), var("token", "t"), var("sum", "5")];
            http_env_store::add(Environment { id: "tls".into(), name: "TLS".into(), variables, trusted_cert: Some(CA.into()) }).unwrap();
            let view = send(&collection, "order", None, Some("tls")).unwrap();
            assert!(matches!(view.outcome, Outcome::Response { status: 200, .. }), "{view:?}");
            assert_eq!(http_collections::history().unwrap()[0].request.body.as_deref(), Some("{\"sum\": 5}"));
        });
    }

    #[test]
    fn a_stand_that_is_down_is_a_result_kept_like_a_response() {
        with_app_dir("http-send-down", || {
            let port = closed_port();
            let collection = shop();
            let env = stage(format!("http://127.0.0.1:{port}"));
            let view = send(&collection, "order", None, Some(&env)).unwrap();
            assert!(matches!(view.outcome, Outcome::Failed { .. }), "{view:?}");
            assert_eq!(last_response(&collection, "order").unwrap(), Some(view));
            assert_eq!(http_collections::history().unwrap().len(), 1);
        });
    }

    #[test]
    fn a_secret_in_the_address_reaches_neither_the_window_nor_the_disk() {
        with_app_dir("http-send-secret-url", || {
            let (port, seen) = serve(vec![
                reply("HTTP/1.1 302 Found\r\nlocation: /done\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"),
                reply("HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"),
            ]);
            let collection = shop();
            let env = stage(format!("http://127.0.0.1:{port}"));
            let apikey = json!([{"key": "key", "value": "k"}, {"key": "value", "value": "{{token}}"}, {"key": "in", "value": "query"}]);
            let edited = json!({"method": "GET", "url": "{{baseUrl}}/bot{{token}}/getMe", "auth": {"type": "apikey", "apikey": apikey}});
            let view = send(&collection, "order", Some(edited), Some(&env)).unwrap();
            assert!(wire(&seen).starts_with("GET /bots3cr3t-t0k/getMe?k=s3cr3t-t0k HTTP/1.1"));
            let Outcome::Response { redirects, url, .. } = &view.outcome else { panic!("{view:?}") };
            assert_eq!((redirects[0].as_str(), url.as_str()), (format!("http://127.0.0.1:{port}/bot•••/getMe?k=•••").as_str(), format!("http://127.0.0.1:{port}/done").as_str()));

            let bad = json!({"method": "GET", "url": "http://[{{token}}/x"});
            let failed = send(&collection, "order", Some(bad), Some(&env)).unwrap();
            assert!(matches!(&failed.outcome, Outcome::Failed { error } if error.contains("http://[•••/x")), "{failed:?}");

            let shown = [
                serde_json::to_string(&view).unwrap(),
                serde_json::to_string(&failed).unwrap(),
                serde_json::to_string(&http_collections::history().unwrap()).unwrap(),
                serde_json::to_string(&last_response(&collection, "order").unwrap()).unwrap(),
            ];
            assert!(shown.iter().all(|s| !s.contains("s3cr3t")), "{shown:?}");
        });
    }

    #[test]
    fn a_send_that_ends_after_a_later_one_does_not_replace_its_response() {
        with_app_dir("http-send-race", || {
            let (slow, _seen_slow) = serve(vec![Box::new(|_| {
                std::thread::sleep(std::time::Duration::from_millis(500));
                b"HTTP/1.1 200 OK\r\ncontent-length: 4\r\nconnection: close\r\n\r\nslow".to_vec()
            })]);
            let (fast, _seen_fast) = serve_once("HTTP/1.1 200 OK\r\ncontent-length: 4\r\nconnection: close\r\n\r\nfast");
            let collection = shop();
            let to = |port: u16| json!({"method": "GET", "url": format!("http://127.0.0.1:{port}/"), "auth": {"type": "noauth"}});
            let first = {
                let (collection, request) = (collection.clone(), to(slow));
                std::thread::spawn(move || send(&collection, "order", Some(request), None).unwrap())
            };
            std::thread::sleep(std::time::Duration::from_millis(150));
            assert_eq!(send(&collection, "order", Some(to(fast)), None).unwrap().body.as_deref(), Some("fast"));
            assert_eq!(first.join().unwrap().body.as_deref(), Some("slow"), "the window still gets it, to drop");
            assert_eq!(last_response(&collection, "order").unwrap().unwrap().body.as_deref(), Some("fast"));
            assert_eq!(http_collections::history().unwrap().len(), 2, "history logs both");
        });
    }

    #[test]
    fn a_history_that_does_not_open_does_not_cost_the_response() {
        with_app_dir("http-send-damaged", || {
            let (port, _seen) = serve_once("HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok");
            let collection = shop();
            std::fs::write(http_collections::dir().unwrap().join("history.enc"), b"not sealed").unwrap();
            let edited = json!({"method": "GET", "url": format!("http://127.0.0.1:{port}/"), "auth": {"type": "noauth"}});
            let view = send(&collection, "order", Some(edited), None).unwrap();
            assert_eq!(view.body.as_deref(), Some("ok"));
            assert_eq!(last_response(&collection, "order").unwrap(), Some(view));
            assert!(http_collections::history().is_err(), "left as it was, not overwritten");
        });
    }

    #[test]
    fn a_variable_found_nowhere_stops_the_send_and_leaves_no_trace() {
        with_app_dir("http-send-missing", || {
            let collection = shop();
            assert_eq!(send(&collection, "order", None, None), Err(HttpError::UnknownVariables(vec!["baseUrl".into(), "token".into()])));
            assert_eq!(send(&collection, "order", None, Some("gone")), Err(HttpError::NoEnvironment("gone".into())));
            assert_eq!(last_response(&collection, "order").unwrap(), None);
            assert!(http_collections::history().unwrap().is_empty());
        });
    }

    #[test]
    fn a_script_that_changes_a_header_changes_what_is_sent() {
        with_app_dir("http-script-header", || {
            let (port, seen) = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
            let id = scripted(vec![], vec![], vec![event("prerequest", "pm.request.headers.add({key: 'X-Trace', value: pm.environment.get('baseUrl').length + ''});\npm.request.url.addQueryParams('from=script');\npm.request.body.update('{\"changed\":1}');")]);
            trust(&id);
            let base = format!("http://127.0.0.1:{port}");
            let length = base.len();
            stage(base);
            let view = send(&id, "order", None, Some("stage")).unwrap();
            assert_eq!(view.scripts.error, None);
            assert!(matches!(view.outcome, Outcome::Response { .. }), "{:?}", view.outcome);
            let wire = wire(&seen);
            assert!(wire.contains(&format!("x-trace: {length}")) || wire.contains(&format!("X-Trace: {length}")), "{wire}");
            assert!(wire.starts_with("POST /orders?from=script "), "{wire}");
            assert!(wire.ends_with("{\"changed\":1}"), "{wire}");
        });
    }

    #[test]
    fn scripts_run_collection_folder_request_and_each_sees_the_last_ones_writes() {
        with_app_dir("http-script-order", || {
            let (port, _seen) = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
            let step = |name: &str| format!("pm.globals.set('order', (pm.globals.get('order') || '') + '{name}');");
            let id = scripted(
                vec![event("prerequest", &step("c")), event("test", &step("C"))],
                vec![event("prerequest", &step("f")), event("test", &step("F"))],
                vec![event("prerequest", &step("r")), event("test", &step("R"))],
            );
            trust(&id);
            stage(format!("http://127.0.0.1:{port}"));
            send(&id, "order", None, Some("stage")).unwrap();
            assert_eq!(global("order").as_deref(), Some("cfrCFR"));
        });
    }

    #[test]
    fn without_trust_no_script_runs_and_a_changed_script_asks_again() {
        with_app_dir("http-script-trust", || {
            let (port, _seen) = serve(vec![reply("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n"), reply("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n"), reply("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")]);
            let id = scripted(vec![], vec![], vec![event("prerequest", "pm.globals.set('ran', 'yes');")]);
            stage(format!("http://127.0.0.1:{port}"));

            let status = script_trust(&id).unwrap();
            assert_eq!((status.count, status.trusted), (1, false));
            let view = send(&id, "order", None, Some("stage")).unwrap();
            assert!(view.scripts.not_run);
            assert_eq!(global("ran"), None, "the request went without its script");

            trust(&id);
            assert!(script_trust(&id).unwrap().trusted);
            assert!(!send(&id, "order", None, Some("stage")).unwrap().scripts.not_run);
            assert_eq!(global("ran").as_deref(), Some("yes"));

            let old = script_trust(&id).unwrap().hash.unwrap();
            change(&id, |c| {
                let item = c.item_mut("order").unwrap();
                item.insert("event".into(), json!([event("prerequest", "pm.globals.set('ran', 'changed');")]));
                Ok(())
            })
            .unwrap();
            assert!(!script_trust(&id).unwrap().trusted, "another script, another answer");
            assert!(send(&id, "order", None, Some("stage")).unwrap().scripts.not_run);
            assert_eq!(global("ran").as_deref(), Some("yes"));
            assert!(trust_scripts(&id, &old).is_err(), "an answer about the old scripts is no answer about these");
        });
    }

    #[test]
    fn a_collection_with_no_scripts_has_nothing_to_trust() {
        with_app_dir("http-script-none", || {
            let id = shop();
            assert_eq!(script_trust(&id).unwrap(), ScriptTrust { hash: None, count: 0, trusted: false });
            assert!(trust_scripts(&id, "anything").is_err());
        });
    }

    #[test]
    fn what_scripts_write_outlives_the_send_and_stays_out_of_the_collections_file() {
        with_app_dir("http-script-vars", || {
            let (port, _seen) = serve(vec![reply("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n"), reply("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")]);
            let code = "pm.globals.set('access_token', 't-1');\npm.environment.set('seen', pm.collectionVariables.get('fromFile'));\npm.collectionVariables.set('fromFile', 'script');\npm.collectionVariables.set('token', 'c-1');\npm.variables.set('local', 'gone');\npm.environment.unset('baseUrl2');";
            let id = scripted(vec![], vec![], vec![event("prerequest", code)]);
            trust(&id);
            let file = http_collections::get(&id).unwrap().json().clone();
            stage(format!("http://127.0.0.1:{port}"));
            let view = send(&id, "order", None, Some("stage")).unwrap();

            assert!(view.scripts.changed);
            assert_eq!(global("access_token").as_deref(), Some("t-1"));
            assert_eq!(env_value("seen").as_deref(), Some("file"));
            assert_eq!(env_value("local"), None);
            assert_eq!(http_collections::get(&id).unwrap().json(), &file, "the file is as it came");
            let listed = collections().unwrap().into_iter().find(|c| c.id == id).unwrap();
            assert_eq!(listed.current.iter().map(|v| (v.key.as_str(), v.value.as_str())).collect::<Vec<_>>(), vec![("fromFile", "script"), ("token", "c-1")]);

            // The next send reads what this one wrote.
            change(&id, |c| {
                c.item_mut("order").unwrap().insert("event".into(), json!([event("prerequest", "pm.environment.set('again', pm.collectionVariables.get('token') + pm.globals.get('access_token'));")]));
                Ok(())
            })
            .unwrap();
            trust(&id);
            send(&id, "order", None, Some("stage")).unwrap();
            assert_eq!(env_value("again").as_deref(), Some("c-1t-1"));

            delete_collection(&id).unwrap();
            assert!(!http_env_store::collection_values().unwrap().contains_key(&id));
            assert!(!settings_store::load().unwrap().http_script_trust.contains_key(&id));
        });
    }

    #[test]
    fn a_script_that_writes_nothing_says_so() {
        with_app_dir("http-script-quiet", || {
            let (port, _seen) = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
            let id = scripted(vec![], vec![], vec![event("prerequest", "pm.environment.set('baseUrl', pm.environment.get('baseUrl'));")]);
            trust(&id);
            stage(format!("http://127.0.0.1:{port}"));
            assert!(!send(&id, "order", None, Some("stage")).unwrap().scripts.changed);
        });
    }

    #[test]
    fn send_request_goes_out_and_into_history_with_a_secret_masked() {
        with_app_dir("http-script-sub", || {
            let token = "HTTP/1.1 200 OK\r\nContent-Length: 20\r\n\r\n{\"access_token\":\"x\"}";
            let (port, seen) = serve(vec![reply(token), reply("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")]);
            let code = "pm.request.headers.add({key: 'X-Own', value: pm.environment.get('token')});\npm.sendRequest({url: pm.environment.get('baseUrl') + '/token?k=' + pm.environment.get('token'), method: 'POST', header: {'X-Key': pm.environment.get('token')}, body: {mode: 'urlencoded', urlencoded: [{key: 'grant_type', value: 'client_credentials'}]}}, (err, res) => { pm.globals.set('access_token', res.json().access_token); });";
            let id = scripted(vec![], vec![], vec![event("prerequest", code)]);
            trust(&id);
            stage(format!("http://127.0.0.1:{port}"));
            send(&id, "order", None, Some("stage")).unwrap();

            let first = wire(&seen);
            assert!(first.starts_with("POST /token?k=s3cr3t-t0k "), "{first}");
            assert!(first.ends_with("grant_type=client_credentials"), "{first}");
            assert!(wire(&seen).starts_with("POST /orders "));
            assert_eq!(global("access_token").as_deref(), Some("x"));

            let history = http_collections::history().unwrap();
            assert_eq!(history.len(), 2);
            let sub = history.iter().find(|h| h.request_id.is_none()).expect("the script's own request is in history");
            assert!(sub.request.url.ends_with("/token?k=•••"), "{}", sub.request.url);
            assert!(!serde_json::to_string(&history).unwrap().contains("s3cr3t-t0k"), "{history:?}");
        });
    }

    #[test]
    fn a_pre_request_script_that_fails_sends_nothing_and_keeps_what_it_wrote() {
        with_app_dir("http-script-throw", || {
            let id = scripted(vec![], vec![], vec![event("prerequest", "pm.globals.set('step', '1');\nthrow new Error('no ' + pm.environment.get('token'));")]);
            trust(&id);
            stage("http://127.0.0.1:9".into());
            let error = send(&id, "order", None, Some("stage")).unwrap_err().to_string();
            assert!(error.contains("nothing was sent"), "{error}");
            assert!(error.contains("Error: no •••"), "{error}");
            assert_eq!(global("step").as_deref(), Some("1"));
            assert!(http_collections::history().unwrap().is_empty());
        });
    }

    #[test]
    fn skipping_the_request_sends_nothing() {
        with_app_dir("http-script-skip", || {
            let id = scripted(vec![], vec![], vec![event("prerequest", "pm.execution.skipRequest();")]);
            trust(&id);
            stage("http://127.0.0.1:9".into());
            assert!(send(&id, "order", None, Some("stage")).unwrap_err().to_string().contains("skipped"));
            assert!(http_collections::history().unwrap().is_empty());
        });
    }

    #[test]
    fn tests_are_kept_with_the_response_and_a_test_that_throws_leaves_it_standing() {
        with_app_dir("http-script-tests", || {
            let (port, _seen) = serve_once("HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: 8\r\n\r\n{\"id\":7}");
            let first = "pm.test('created', () => pm.response.to.have.status(201));\npm.test('id is ' + pm.response.json().id, () => pm.expect(pm.response.json().id).to.eql(8));\nconsole.log(pm.environment.get('token'), pm.request.url.toString());\nthrow new Error('after the tests');";
            let second = "pm.globals.set('second', 'ran');";
            let id = scripted(vec![], vec![event("test", first)], vec![event("test", second)]);
            trust(&id);
            stage(format!("http://127.0.0.1:{port}"));
            let view = send(&id, "order", None, Some("stage")).unwrap();
            assert!(matches!(view.outcome, Outcome::Response { status: 201, .. }));
            assert_eq!(view.scripts.tests.iter().map(|t| (t.name.as_str(), t.passed)).collect::<Vec<_>>(), vec![("created", true), ("id is 7", false)]);
            assert_eq!(view.scripts.error.as_deref(), Some("Error: after the tests"));
            assert_eq!(global("second").as_deref(), Some("ran"), "the next script still runs");
            assert!(view.scripts.console[0].text.starts_with("••• http://127.0.0.1"), "{:?}", view.scripts.console);
            assert_eq!(last_response(&id, "order").unwrap().unwrap().scripts.tests.len(), 2, "the saved response has them too");
        });
    }

    #[test]
    fn with_no_response_there_is_nothing_to_test() {
        with_app_dir("http-script-down", || {
            let id = scripted(vec![], vec![], vec![event("test", "pm.globals.set('tested', 'yes');")]);
            trust(&id);
            stage("http://127.0.0.1:9".into());
            let view = send(&id, "order", None, Some("stage")).unwrap();
            assert!(matches!(view.outcome, Outcome::Failed { .. }));
            assert_eq!(global("tested"), None);
            assert_eq!(view.scripts.error, None);
        });
    }

    #[test]
    fn a_script_that_changes_nothing_changes_nothing_on_the_wire() {
        with_app_dir("http-script-same-wire", || {
            let answer = "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
            let (port, seen) = serve(vec![reply(answer), reply(answer)]);
            let file = |events: Value| {
                let request = json!({"method": "POST", "url": {"raw": format!("http://u:p@127.0.0.1:{port}/x?a=1&b#h")}, "header": [{"key": "X-On", "value": "1"}, {"key": "X-Off", "value": "0", "disabled": true}],
                    "body": {"mode": "raw", "raw": "{\"a\":1}", "options": {"raw": {"language": "json"}}}});
                let file = json!({"info": {"name": "C", "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"}, "item": [{"id": "order", "name": "Order", "event": events, "request": request}]});
                http_collections::add(&Collection::parse(&file.to_string()).unwrap()).unwrap()
            };
            let (plain, scripted) = (file(json!([])), file(json!([event("prerequest", "1;")])));
            trust(&scripted);
            send(&plain, "order", None, None).unwrap();
            send(&scripted, "order", None, None).unwrap();
            assert_eq!(wire(&seen), wire(&seen));
        });
    }

    #[test]
    fn a_later_send_wins_though_the_first_is_still_in_its_scripts() {
        with_app_dir("http-script-race", || {
            let reply_with = |text: &str| reply(&format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}", text.len()));
            let (port, _seen) = serve(vec![reply_with("second"), reply_with("first")]);
            let id = scripted(vec![], vec![], vec![event("prerequest", "setTimeout(() => {}, pm.globals.has('quick') ? 0 : 600);")]);
            trust(&id);
            stage(format!("http://127.0.0.1:{port}"));
            let first = {
                let id = id.clone();
                std::thread::spawn(move || send(&id, "order", None, Some("stage")).unwrap())
            };
            std::thread::sleep(std::time::Duration::from_millis(300));
            http_env_store::write_changes(None, &[("quick".into(), Some("1".into()))], ("", &[])).unwrap();
            assert_eq!(send(&id, "order", None, Some("stage")).unwrap().body.as_deref(), Some("second"));
            assert_eq!(first.join().unwrap().body.as_deref(), Some("first"));
            assert_eq!(last_response(&id, "order").unwrap().unwrap().body.as_deref(), Some("second"));
        });
    }

    #[test]
    fn a_secret_a_script_replaced_or_covered_is_still_masked_everywhere() {
        with_app_dir("http-script-old-secret", || {
            let answer = "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
            let (port, _seen) = serve(vec![reply(answer), reply(answer)]);
            let code = "const old = pm.environment.get('token');\npm.globals.set('copy', old);\npm.request.headers.add({key: 'X-Old', value: old});\npm.environment.set('token', 'n3w-t0k3n');\npm.variables.set('token', 'plain');";
            let next = "pm.sendRequest(pm.environment.get('baseUrl') + '/' + pm.globals.get('copy'), () => {});";
            let id = scripted(vec![], vec![event("prerequest", code)], vec![event("prerequest", next)]);
            trust(&id);
            stage(format!("http://127.0.0.1:{port}"));
            send(&id, "order", None, Some("stage")).unwrap();
            let history = serde_json::to_string(&http_collections::history().unwrap()).unwrap();
            assert!(!history.contains("s3cr3t-t0k"), "{history}");
        });
        with_app_dir("http-script-old-secret-error", || {
            let code = "const old = pm.environment.get('token');\npm.environment.set('token', 'new');\nthrow new Error(old);";
            let id = scripted(vec![], vec![], vec![event("prerequest", code)]);
            trust(&id);
            stage("http://127.0.0.1:9".into());
            let error = send(&id, "order", None, Some("stage")).unwrap_err().to_string();
            assert!(!error.contains("s3cr3t-t0k") && error.contains("•••"), "{error}");
        });
    }

    #[test]
    fn what_scripts_wrote_and_could_not_keep_is_said() {
        with_app_dir("http-script-unkept", || {
            let (port, seen) = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
            let id = scripted(vec![], vec![], vec![event("test", "setTimeout(() => pm.globals.set('late', '1'), 400);")]);
            trust(&id);
            stage(format!("http://127.0.0.1:{port}"));
            let sent = std::thread::spawn(move || send(&id, "order", None, Some("stage")).unwrap());
            seen.recv().unwrap();
            let store = http_collections::dir().unwrap().join("environments.enc");
            std::fs::write(&store, b"not sealed").unwrap();
            let view = sent.join().unwrap();
            assert!(view.scripts.error.as_deref().is_some_and(|e| e.contains("not kept")), "{:?}", view.scripts);
            assert!(!view.scripts.changed);
        });
    }

    #[test]
    fn a_loop_without_end_in_a_pre_request_script_sends_nothing() {
        with_app_dir("http-script-loop", || {
            let id = scripted(vec![], vec![], vec![event("prerequest", "while (true) {}")]);
            trust(&id);
            stage("http://127.0.0.1:9".into());
            assert_eq!(send(&id, "order", None, Some("stage")).unwrap_err().to_string(), "a script ran out of time");
            assert!(http_collections::history().unwrap().is_empty());
        });
    }

    #[test]
    fn a_test_script_past_the_limit_stops_the_rest_and_the_response_stands() {
        with_app_dir("http-script-test-loop", || {
            let (port, _seen) = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
            let id = scripted(vec![], vec![event("test", "while (true) {}")], vec![event("test", "pm.globals.set('after', 'ran');")]);
            trust(&id);
            stage(format!("http://127.0.0.1:{port}"));
            let view = send(&id, "order", None, Some("stage")).unwrap();
            assert!(matches!(view.outcome, Outcome::Response { status: 200, .. }));
            assert_eq!(view.scripts.error.as_deref(), Some("a script ran out of time"));
            assert_eq!(global("after"), None);
        });
    }

    #[test]
    fn what_a_test_script_makes_a_secret_is_masked_in_the_address_it_came_from() {
        with_app_dir("http-script-learned", || {
            let (port, _seen) = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
            let id = scripted(vec![], vec![], vec![event("test", "pm.environment.set('token', 'orders');")]);
            trust(&id);
            stage(format!("http://127.0.0.1:{port}"));
            let view = send(&id, "order", None, Some("stage")).unwrap();
            assert_eq!(env_value("token").as_deref(), Some("orders"));
            assert!(!serde_json::to_string(&view.outcome).unwrap().contains("orders"), "{:?}", view.outcome);
        });
    }

    #[test]
    fn a_collections_secret_a_script_wrote_reaches_the_window_masked() {
        with_app_dir("http-script-current", || {
            let (port, _seen) = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
            let id = scripted(vec![], vec![], vec![event("prerequest", "pm.collectionVariables.set('apiKey', 's3cr3t-k3y');\npm.collectionVariables.set('open', 'plain');")]);
            trust(&id);
            stage(format!("http://127.0.0.1:{port}"));
            send(&id, "order", None, Some("stage")).unwrap();
            let all = collections().unwrap();
            let current = &all.iter().find(|c| c.id == id).unwrap().current;
            assert!(current.iter().any(|v| v.key == "apiKey" && v.secret && v.value != "s3cr3t-k3y"), "{current:?}");
            assert!(current.iter().any(|v| v.key == "open" && v.value == "plain"), "{current:?}");
        });
    }

    #[test]
    fn a_script_s_request_that_fails_reaches_its_callback_as_an_error() {
        with_app_dir("http-script-sub-down", || {
            let (port, _seen) = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
            let code = format!("pm.sendRequest('http://127.0.0.1:{}/', (err, res) => pm.globals.set('said', err ? 'failed' : 'answered'));", closed_port());
            let id = scripted(vec![], vec![], vec![event("prerequest", &code)]);
            trust(&id);
            stage(format!("http://127.0.0.1:{port}"));
            send(&id, "order", None, Some("stage")).unwrap();
            assert_eq!(global("said").as_deref(), Some("failed"));
        });
    }

    #[test]
    fn a_test_script_sees_the_request_as_it_left() {
        with_app_dir("http-script-sent", || {
            let (port, _seen) = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
            let id = scripted(vec![], vec![], vec![event("test", "pm.test('body', () => pm.expect(pm.request.body.raw).to.eql('{}'));")]);
            trust(&id);
            stage(format!("http://127.0.0.1:{port}"));
            let view = send(&id, "order", None, Some("stage")).unwrap();
            assert_eq!(view.scripts.tests.iter().map(|t| t.passed).collect::<Vec<_>>(), vec![true], "{:?}", view.scripts.tests);
        });
    }

    #[test]
    fn a_secret_a_script_replaces_stays_hidden_old_and_new() {
        with_app_dir("http-script-secret", || {
            let (port, _seen) = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
            let code = "console.log(pm.environment.get('token'));\npm.environment.set('token', 'n3w-t0k3n');\nconsole.log(pm.environment.get('token'));";
            let id = scripted(vec![], vec![], vec![event("prerequest", code)]);
            trust(&id);
            stage(format!("http://127.0.0.1:{port}"));
            let view = send(&id, "order", None, Some("stage")).unwrap();
            assert_eq!(view.scripts.console.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), vec!["•••", "•••"]);
            let stored = http_env_store::get("stage").unwrap();
            assert!(stored.variables.iter().any(|v| v.key == "token" && v.secret && v.value == "n3w-t0k3n"), "still a secret, with its new value");
        });
    }
}
