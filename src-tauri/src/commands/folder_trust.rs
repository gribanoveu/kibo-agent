//! Whether the open folder is trusted with its own skills and `/` commands.

use std::sync::Arc;

use crate::services::folder_trust::{self, FolderTrustView};

/// `None` while no folder is open.
#[tauri::command]
pub fn folder_trust_get(state: tauri::State<'_, Arc<super::chat::AgentState>>) -> Option<FolderTrustView> {
    state.workspace().ok().map(|ws| folder_trust::view(&ws))
}

#[tauri::command]
pub fn folder_trust_set(trusted: bool, state: tauri::State<'_, Arc<super::chat::AgentState>>) -> Result<(), String> {
    folder_trust::set(&state.workspace()?, trusted).map_err(|e| e.to_string())
}
