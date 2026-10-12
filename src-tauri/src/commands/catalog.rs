//! The catalog page (`docs/32-catalog.md`).

use crate::services::catalog::{self, SourceView};

/// Every catalog asked for its index — a conditional GET, so asking again
/// with nothing new costs a `304`. Off the IPC loop: it is the network.
#[tauri::command]
pub async fn catalog_list() -> Result<Vec<SourceView>, String> {
    tauri::async_runtime::spawn_blocking(catalog::list).await.map_err(|e| format!("the catalog thread failed: {e}"))
}
