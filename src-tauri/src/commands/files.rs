//! The Files tab's folder tree, one folder at a time as it is unfolded, and
//! saving a file edited in the viewer.

use std::sync::Arc;

use tauri::State;

use super::chat::AgentState;
use crate::domain::file_tree::FolderListing;
use crate::domain::file_write::FileSave;
use crate::infra::{file_tree, file_write};

/// `dir` is relative to the open folder; empty for the folder itself. Off the
/// IPC loop: a folder's walk and git's status of it take a moment on a big one.
#[tauri::command]
pub async fn workspace_list(dir: String, state: State<'_, Arc<AgentState>>) -> Result<FolderListing, String> {
    let root = state.workspace()?;
    tauri::async_runtime::spawn_blocking(move || file_tree::list(&root, &dir))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// Saves a file of the open folder edited in the viewer. `expected` is the
/// text it was opened with: changed on disk since, it is not written and the
/// answer says so. `None` writes it regardless — the user chose their edit.
#[tauri::command]
pub async fn file_write(
    path: String,
    expected: Option<String>,
    content: String,
    state: State<'_, Arc<AgentState>>,
) -> Result<FileSave, String> {
    let root = state.workspace()?;
    tauri::async_runtime::spawn_blocking(move || file_write::write(&root, &path, expected.as_deref(), &content))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}
