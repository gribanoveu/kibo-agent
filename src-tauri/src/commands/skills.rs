//! The skills and rules tabs: what the model is given besides the
//! conversation, and which of it is switched on.

use crate::services::skills::{self, SkillsView};

/// The folder's skills and the user's; only the user's while `workspace` is
/// `None` (Chat mode). The path is canonicalized and accepted only as an open
/// folder (F-8, invariant 1).
#[tauri::command]
pub fn skills_list(
    workspace: Option<String>,
    state: tauri::State<'_, std::sync::Arc<super::agent_state::AgentState>>,
) -> Result<SkillsView, String> {
    let workspace = state.ui_workspace(&workspace)?;
    skills::list(workspace.as_deref()).map_err(|e| e.to_string())
}

/// A skills folder — `project`, `app`, `agents`, `claude` — read or not, for every repository.
#[tauri::command]
pub fn skills_set_source_enabled(id: String, enabled: bool) -> Result<(), String> {
    skills::set_source_enabled(&id, enabled).map_err(|e| e.to_string())
}

/// By name: a skill switched off is off in every folder and every repository.
#[tauri::command]
pub fn skills_set_enabled(name: String, enabled: bool) -> Result<(), String> {
    skills::set_enabled(&name, enabled).map_err(|e| e.to_string())
}

/// The folder's instruction files; none while `workspace` is `None` (Chat mode).
#[tauri::command]
pub fn rules_list(
    workspace: Option<String>,
    state: tauri::State<'_, std::sync::Arc<super::agent_state::AgentState>>,
) -> Result<Vec<crate::domain::project_rules::RuleListItem>, String> {
    let workspace = state.ui_workspace(&workspace)?;
    Ok(workspace.map(|root| crate::services::project_rules::list(&root)).unwrap_or_default())
}

#[tauri::command]
pub fn rules_set_enabled(path: String, enabled: bool) -> Result<(), String> {
    crate::services::project_rules::set_enabled(&path, enabled).map_err(|e| e.to_string())
}
