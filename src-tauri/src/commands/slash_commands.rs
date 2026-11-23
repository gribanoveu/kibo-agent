//! The composer's `/` menu: the commands the user wrote in `.kibo/commands`.

use std::sync::Arc;

use crate::domain::slash_commands::CommandFile;
use crate::infra::slash_commands_store;
use crate::services::folder_trust;

/// The folder's command files and the user's; only the user's while
/// `workspace` is `None` (Chat mode), or while the folder is not trusted. The
/// built-in commands are the window's own and not listed. The path is
/// canonicalized and accepted only as an open folder (F-8, invariant 1).
#[tauri::command]
pub fn slash_commands_list(
    workspace: Option<String>,
    state: tauri::State<'_, Arc<super::agent_state::AgentState>>,
) -> Result<Vec<CommandFile>, String> {
    let workspace = state.ui_workspace(&workspace)?;
    Ok(slash_commands_store::list(folder_trust::readable(workspace.as_deref())))
}
