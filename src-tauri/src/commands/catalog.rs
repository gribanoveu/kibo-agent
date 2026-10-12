//! The catalog page (`docs/32-catalog.md`).

use crate::domain::catalog::{CatalogError, ItemKind};
use crate::services::catalog::{self, SourceView};

/// Every catalog asked for its index — a conditional GET, so asking again
/// with nothing new costs a `304`. Off the IPC loop: it is the network.
#[tauri::command]
pub async fn catalog_list() -> Result<Vec<SourceView>, String> {
    tauri::async_runtime::spawn_blocking(catalog::list).await.map_err(|e| format!("the catalog thread failed: {e}"))
}

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> Result<T, CatalogError> + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| format!("the catalog thread failed: {e}"))?
        .map_err(|e| e.to_string())
}

/// Installs a skill, command or runbook from `source`'s kept index — the one
/// the page shows. Its files come over the network, so off the IPC loop.
#[tauri::command]
pub async fn catalog_install(source: String, kind: ItemKind, id: String) -> Result<(), String> {
    blocking(move || catalog::install(&source, kind, &id)).await
}

/// Removes what `source` installed as `kind`/`id`.
#[tauri::command]
pub async fn catalog_remove(source: String, kind: ItemKind, id: String) -> Result<(), String> {
    blocking(move || catalog::remove(&source, kind, &id)).await
}

/// The text of an entry, for its details panel: a skill's `SKILL.md`, any
/// other kind's one file.
#[tauri::command]
pub async fn catalog_preview(source: String, kind: ItemKind, id: String) -> Result<String, String> {
    blocking(move || catalog::preview(&source, kind, &id)).await
}
