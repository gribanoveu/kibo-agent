//! The MongoDB page (`docs/43-mongodb.md`), through `services::mongo`. No
//! channel of its own: only the page changes what it shows, and each change
//! answers with what it changed — the list of connections after a save, a
//! count after an insert — or the page reads again.

use std::sync::Arc;

use serde_json::Value;
use tauri::State;

use crate::domain::mongo::{add_condition, ConnectionDraft, ConnectionView, FieldChange, IndexView};
use crate::infra::mongo_client::{CollectionInfo, DatabaseInfo, MongoClients};
use crate::services::mongo::{self, DocumentView, FindPage, FindRequest, Target};

use super::blocking;

type Clients<'a> = State<'a, Arc<MongoClients>>;

#[tauri::command]
pub async fn mongo_connections() -> Result<Vec<ConnectionView>, String> {
    blocking(mongo::connections).await
}

#[tauri::command]
pub async fn mongo_connection_save(draft: ConnectionDraft) -> Result<Vec<ConnectionView>, String> {
    blocking(move || mongo::save_connection(draft)).await
}

#[tauri::command]
pub async fn mongo_connection_delete(id: String) -> Result<Vec<ConnectionView>, String> {
    blocking(move || mongo::delete_connection(&id)).await
}

/// The server's version.
#[tauri::command]
pub async fn mongo_connection_test(draft: ConnectionDraft, clients: Clients<'_>) -> Result<String, String> {
    let clients = clients.inner().clone();
    blocking(move || mongo::test_connection(&clients, draft)).await
}

#[tauri::command]
pub async fn mongo_databases(connection: String, clients: Clients<'_>) -> Result<Vec<DatabaseInfo>, String> {
    let clients = clients.inner().clone();
    blocking(move || mongo::databases(&clients, &connection)).await
}

#[tauri::command]
pub async fn mongo_collections(connection: String, database: String, clients: Clients<'_>) -> Result<Vec<CollectionInfo>, String> {
    let clients = clients.inner().clone();
    blocking(move || mongo::collections(&clients, &connection, &database)).await
}

#[tauri::command]
pub async fn mongo_find(target: Target, request: FindRequest, clients: Clients<'_>) -> Result<FindPage, String> {
    let clients = clients.inner().clone();
    blocking(move || mongo::find(&clients, &target, &request)).await
}

#[tauri::command]
pub async fn mongo_find_more(next: u64, clients: Clients<'_>) -> Result<FindPage, String> {
    let clients = clients.inner().clone();
    blocking(move || mongo::find_more(&clients, next)).await
}

/// A find the page no longer reads: its cursor closed on the server.
#[tauri::command]
pub async fn mongo_find_close(next: u64, clients: Clients<'_>) -> Result<(), String> {
    let clients = clients.inner().clone();
    blocking(move || Ok::<_, std::convert::Infallible>(clients.find_close(next))).await
}

#[tauri::command]
pub async fn mongo_count(target: Target, filter: String, clients: Clients<'_>) -> Result<u64, String> {
    let clients = clients.inner().clone();
    blocking(move || mongo::count(&clients, &target, &filter)).await
}

#[tauri::command]
pub async fn mongo_aggregate(target: Target, pipeline: String, clients: Clients<'_>) -> Result<Vec<DocumentView>, String> {
    let clients = clients.inner().clone();
    blocking(move || mongo::aggregate(&clients, &target, &pipeline)).await
}

/// How many went in.
#[tauri::command]
pub async fn mongo_insert(target: Target, text: String, clients: Clients<'_>) -> Result<usize, String> {
    let clients = clients.inner().clone();
    blocking(move || mongo::insert(&clients, &target, &text)).await
}

#[tauri::command]
pub async fn mongo_replace(target: Target, id: Value, text: String, clients: Clients<'_>) -> Result<(), String> {
    let clients = clients.inner().clone();
    blocking(move || mongo::replace(&clients, &target, id, &text)).await
}

#[tauri::command]
pub async fn mongo_set_fields(target: Target, id: Value, changes: Vec<FieldChange>, clients: Clients<'_>) -> Result<(), String> {
    let clients = clients.inner().clone();
    blocking(move || mongo::set_fields(&clients, &target, id, &changes)).await
}

/// The filter with one more condition, from a field on a card: no server, only the text.
#[tauri::command]
pub fn mongo_filter_add(filter: String, path: Vec<String>, text: String, exclude: bool) -> Result<String, String> {
    add_condition(&filter, &path, &text, exclude).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn mongo_delete(target: Target, id: Value, clients: Clients<'_>) -> Result<(), String> {
    let clients = clients.inner().clone();
    blocking(move || mongo::delete(&clients, &target, id)).await
}

#[tauri::command]
pub async fn mongo_indexes(target: Target, clients: Clients<'_>) -> Result<Vec<IndexView>, String> {
    let clients = clients.inner().clone();
    blocking(move || mongo::indexes(&clients, &target)).await
}

/// The new index's name.
#[tauri::command]
pub async fn mongo_index_create(target: Target, keys: String, unique: bool, name: Option<String>, clients: Clients<'_>) -> Result<String, String> {
    let clients = clients.inner().clone();
    blocking(move || mongo::create_index(&clients, &target, &keys, unique, name)).await
}

#[tauri::command]
pub async fn mongo_index_drop(target: Target, name: String, clients: Clients<'_>) -> Result<(), String> {
    let clients = clients.inner().clone();
    blocking(move || mongo::drop_index(&clients, &target, &name)).await
}

#[tauri::command]
pub async fn mongo_collection_create(target: Target, clients: Clients<'_>) -> Result<(), String> {
    let clients = clients.inner().clone();
    blocking(move || mongo::create_collection(&clients, &target)).await
}

#[tauri::command]
pub async fn mongo_collection_drop(target: Target, clients: Clients<'_>) -> Result<(), String> {
    let clients = clients.inner().clone();
    blocking(move || mongo::drop_collection(&clients, &target)).await
}
