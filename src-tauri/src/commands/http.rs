//! The HTTP page (`docs/45-http-client.md`), through `services::http_client`.
//! No channel yet: only the page changes its stores, and each command answers
//! with what it did. `http:changed` comes with the agent's sends (HC-7).

use std::path::Path;

use serde_json::Value;

use crate::domain::http_client::{parse_curl, Environment, HistoryEntry, ResponseView};
use crate::infra::{http_collections, http_env_store};
use crate::services::http_client::{self, CollectionView, Environments, ImportedAs};

use super::blocking;

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

#[tauri::command]
pub async fn http_environment_delete(id: String) -> Result<(), String> {
    blocking(move || http_env_store::delete(&id)).await
}

/// Every send kept, the oldest first.
#[tauri::command]
pub async fn http_history_list() -> Result<Vec<HistoryEntry>, String> {
    blocking(http_collections::history).await
}

/// A pasted `curl …` as a Postman `request`; nothing is read or sent.
#[tauri::command]
pub fn http_parse_curl(command: String) -> Result<Value, String> {
    parse_curl(&command).map_err(|e| e.to_string())
}
