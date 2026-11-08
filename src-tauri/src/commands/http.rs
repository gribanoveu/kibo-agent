//! The HTTP page (`docs/45-http-client.md`), through `services::http_client`.
//! Each command answers with what it did; [`HTTP_EVENT`] is for what the page
//! did not ask for, the agent's sends.

use std::path::Path;
use std::sync::Arc;

use serde_json::Value;
use tauri::{AppHandle, Emitter, Runtime};

use crate::domain::http_client::{parse_curl, CookieRow, Environment, HistoryEntry, HttpChanged, HttpEventSink, ResponseView};
use crate::infra::{http_collections, http_cookies, http_env_store};
use crate::services::http_client::{self, CollectionView, Environments, ImportedAs, ScriptTrust};

use super::blocking;

/// The agent sent a request: history has one more.
pub const HTTP_EVENT: &str = "http:changed";

pub fn http_event_sink<R: Runtime>(app: &AppHandle<R>) -> HttpEventSink {
    let app = app.clone();
    Arc::new(move |event: HttpChanged| {
        let _ = app.emit(HTTP_EVENT, event);
    })
}

/// `request` is the page's unsaved edit of the request, sent in place of the stored one.
#[tauri::command]
pub async fn http_send(collection: String, request_id: String, request: Option<Value>, environment: Option<String>) -> Result<ResponseView, String> {
    blocking(move || http_client::send(&collection, &request_id, request, environment.as_deref())).await
}

#[tauri::command]
pub async fn http_response_last(collection: String, request_id: String) -> Result<Option<ResponseView>, String> {
    blocking(move || http_client::last_response(&collection, &request_id)).await
}

#[tauri::command]
pub async fn http_collection_list() -> Result<Vec<CollectionView>, String> {
    blocking(http_client::collections).await
}

#[tauri::command]
pub async fn http_collection_new(name: String) -> Result<CollectionView, String> {
    blocking(move || http_client::new_collection(&name)).await
}

#[tauri::command]
pub async fn http_collection_delete(id: String) -> Result<(), String> {
    blocking(move || http_client::delete_collection(&id)).await
}

/// A request's `request` object as the page edited it; the rest of its item stays as stored.
#[tauri::command]
pub async fn http_request_save(collection: String, request_id: String, request: Value) -> Result<(), String> {
    blocking(move || http_client::save_request(&collection, &request_id, request)).await
}

/// A new Postman item, last in the folder `parent` — or in the collection when there is none.
#[tauri::command]
pub async fn http_item_add(collection: String, parent: Option<String>, item: Value) -> Result<(), String> {
    blocking(move || http_client::add_item(&collection, parent.as_deref(), item)).await
}

/// `id` is a folder's or a request's, or the collection's own to rename the collection.
#[tauri::command]
pub async fn http_item_rename(collection: String, id: String, name: String) -> Result<(), String> {
    blocking(move || http_client::rename(&collection, &id, &name)).await
}

#[tauri::command]
pub async fn http_item_delete(collection: String, id: String) -> Result<(), String> {
    blocking(move || http_client::delete_item(&collection, &id)).await
}

/// A Postman collection, environment or globals file the system's dialog picked.
#[tauri::command]
pub async fn http_import(path: String) -> Result<ImportedAs, String> {
    blocking(move || http_client::import_file(Path::new(&path))).await
}

/// The stored collection at `dest`, a `.json` path from the save dialog.
#[tauri::command]
pub async fn http_export_collection(id: String, dest: String) -> Result<(), String> {
    blocking(move || http_client::export_collection(&id, Path::new(&dest))).await
}

/// The environment at `dest` as a Postman environment, its secrets empty.
#[tauri::command]
pub async fn http_export_environment(id: String, dest: String) -> Result<(), String> {
    blocking(move || http_client::export_environment(&id, Path::new(&dest))).await
}

#[tauri::command]
pub async fn http_environment_list() -> Result<Environments, String> {
    blocking(http_client::environments).await
}

/// The form's environment; a secret left masked or empty keeps its value, a new id is a new one.
#[tauri::command]
pub async fn http_environment_save(environment: Environment) -> Result<(), String> {
    blocking(move || http_client::save_environment(environment)).await
}

/// One saved secret's value, asked for by the form's eye button.
#[tauri::command]
pub async fn http_environment_secret(id: String, key: String) -> Result<String, String> {
    blocking(move || http_client::environment_secret(&id, &key)).await
}

#[tauri::command]
pub async fn http_environment_delete(id: String) -> Result<(), String> {
    blocking(move || http_env_store::delete(&id)).await
}

/// Every send kept, the oldest first.
#[tauri::command]
pub async fn http_history_list() -> Result<Vec<HistoryEntry>, String> {
    blocking(http_collections::history).await
}

/// Whether the collection's scripts run; the page asks before its first send.
#[tauri::command]
pub async fn http_script_trust(collection: String) -> Result<ScriptTrust, String> {
    blocking(move || http_client::script_trust(&collection)).await
}

/// The cookie jar as the page lists it: no value crosses here.
#[tauri::command]
pub async fn http_cookies() -> Result<Vec<CookieRow>, String> {
    blocking(http_cookies::list).await
}

#[tauri::command]
pub async fn http_cookie_delete(domain: String, path: String, name: String) -> Result<(), String> {
    blocking(move || http_cookies::delete(&domain, &path, &name)).await
}

#[tauri::command]
pub async fn http_cookies_clear() -> Result<(), String> {
    blocking(http_cookies::clear).await
}

/// Trust in the scripts `hash` names, the ones `http_script_trust` said.
#[tauri::command]
pub async fn http_script_trust_set(collection: String, hash: String) -> Result<(), String> {
    blocking(move || http_client::trust_scripts(&collection, &hash)).await
}

/// A pasted `curl …` as a Postman `request`; nothing is read or sent.
#[tauri::command]
pub fn http_parse_curl(command: String) -> Result<Value, String> {
    parse_curl(&command).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;
    use tauri::Listener;

    #[test]
    fn the_channel_name_is_the_one_the_frontend_listens_on() {
        assert_eq!(HTTP_EVENT, "http:changed");
    }

    #[test]
    fn a_send_reaches_the_window_as_the_event() {
        let app = tauri::test::mock_app();
        let (tx, rx) = mpsc::channel();
        app.handle().listen(HTTP_EVENT, move |event| {
            let _ = tx.send(event.payload().to_string());
        });
        http_event_sink(app.handle())(HttpChanged);
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "null");
    }
}
