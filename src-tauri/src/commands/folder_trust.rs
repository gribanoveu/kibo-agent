//! Whether the open folder is trusted with its own skills and `/` commands.

use std::sync::Arc;

use crate::services::folder_trust::{self, FolderTrustView};

/// `None` while `workspace` is `None` (Chat mode); otherwise the view of the
/// folder the window named — canonicalized, accepted only as an open folder
/// (F-8, invariant 1).
#[tauri::command]
pub fn folder_trust_get(
    workspace: Option<String>,
    state: tauri::State<'_, Arc<super::agent_state::AgentState>>,
) -> Result<Option<FolderTrustView>, String> {
    Ok(state.ui_workspace(&workspace)?.map(|ws| folder_trust::view(&ws)))
}

#[tauri::command]
pub fn folder_trust_set(
    workspace: Option<String>,
    trusted: bool,
    state: tauri::State<'_, Arc<super::agent_state::AgentState>>,
) -> Result<(), String> {
    let workspace = state
        .ui_workspace(&workspace)?
        .ok_or_else(|| "no folder is open".to_string())?;
    folder_trust::set(&workspace, trusted).map_err(|e| e.to_string())
}
