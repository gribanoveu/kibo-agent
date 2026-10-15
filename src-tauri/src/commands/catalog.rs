//! The catalog page (`docs/32-catalog.md`).

use std::collections::BTreeMap;
use std::sync::Arc;

use tauri::State;

use crate::domain::catalog::ItemKind;
use crate::infra::mcp_config;
use crate::services::catalog::{self, Preview, SourceView, UpdatePreview};
use crate::services::mcp_servers::McpServers;

use super::blocking;

/// Every catalog asked for its index — a conditional GET, so asking again
/// with nothing new costs a `304`. Off the IPC loop: it is the network.
#[tauri::command]
pub async fn catalog_list() -> Result<Vec<SourceView>, String> {
    tauri::async_runtime::spawn_blocking(catalog::list).await.map_err(|e| format!("the catalog thread failed: {e}"))
}

/// Installs an entry from `source`'s kept index — the one the page shows; a
/// server with its `${NAME}`s filled from `secrets`. Its files come over the
/// network, so off the IPC loop. A server added starts with the next Agent turn.
#[tauri::command]
pub async fn catalog_install(source: String, kind: ItemKind, id: String, secrets: BTreeMap<String, String>) -> Result<(), String> {
    blocking(move || catalog::install(&source, kind, &id, &secrets)).await
}

/// Removes what `source` installed as `kind`/`id`; a server removed stops at
/// once, as one deleted in the MCP tab does.
#[tauri::command]
pub async fn catalog_remove(source: String, kind: ItemKind, id: String, servers: State<'_, Arc<McpServers>>) -> Result<(), String> {
    blocking(move || catalog::remove(&source, kind, &id)).await?;
    if kind == ItemKind::Mcp {
        servers.prune(&mcp_config::load().map_err(|e| e.to_string())?);
    }
    Ok(())
}

/// What updating an entry would change — each file's diff against what is
/// there now, the secrets to ask for, whether it was changed here.
#[tauri::command]
pub async fn catalog_update_preview(source: String, kind: ItemKind, id: String) -> Result<UpdatePreview, String> {
    blocking(move || catalog::update_preview(&source, kind, &id)).await
}

/// Writes the catalog's new version over what `source` installed; a server
/// keeps its secrets unless `secrets` gives them again. A server changed
/// stops, to start again as it now is with the next Agent turn.
#[tauri::command]
pub async fn catalog_update(
    source: String,
    kind: ItemKind,
    id: String,
    secrets: BTreeMap<String, String>,
    servers: State<'_, Arc<McpServers>>,
) -> Result<(), String> {
    blocking(move || catalog::update(&source, kind, &id, &secrets)).await?;
    if kind == ItemKind::Mcp {
        servers.prune(&mcp_config::load().map_err(|e| e.to_string())?);
    }
    Ok(())
}

/// The text of an entry, for its details panel: a skill's `SKILL.md`, any
/// other kind's one file — and a server's placeholders, for the install dialog.
#[tauri::command]
pub async fn catalog_preview(source: String, kind: ItemKind, id: String) -> Result<Preview, String> {
    blocking(move || catalog::preview(&source, kind, &id)).await
}
