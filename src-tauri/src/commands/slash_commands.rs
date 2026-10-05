//! The composer's `/` menu: the commands the user wrote in `.kibo/commands`.

use std::sync::Arc;

use crate::domain::slash_commands::CommandFile;
use crate::infra::slash_commands_store;
use crate::services::folder_trust;

/// The open folder's command files and the user's; only the user's while no
/// folder is open, or while the open one is not trusted. The built-in commands
/// are the window's own and not listed.
#[tauri::command]
pub fn slash_commands_list(state: tauri::State<'_, Arc<super::chat::AgentState>>) -> Vec<CommandFile> {
    let workspace = state.workspace().ok();
    slash_commands_store::list(folder_trust::readable(workspace.as_deref()))
}
