//! Rewinding the open folder's files to before a message — the window sends
//! the changes its blocks recorded; see `services::rewind`.

use std::sync::Arc;

use tauri::State;

use crate::domain::rewind::{FileChange, FileRewind};
use crate::domain::tools::ToolScope;
use crate::services::rewind;

use super::blocking;
use super::chat::AgentState;

fn scope(state: &AgentState) -> Result<ToolScope, String> {
    ToolScope::new(&state.workspace()?).map_err(|e| e.to_string())
}

/// What a rewind would do, file by file. Touches nothing.
#[tauri::command]
pub async fn rewind_preview(changes: Vec<FileChange>, state: State<'_, Arc<AgentState>>) -> Result<Vec<FileRewind>, String> {
    let scope = scope(&state)?;
    blocking(move || Ok::<_, String>(rewind::preview(&scope, &changes))).await
}

/// Does it, and says how each file went.
#[tauri::command]
pub async fn rewind_apply(changes: Vec<FileChange>, state: State<'_, Arc<AgentState>>) -> Result<Vec<FileRewind>, String> {
    let scope = scope(&state)?;
    blocking(move || Ok::<_, String>(rewind::apply(&scope, &changes))).await
}
