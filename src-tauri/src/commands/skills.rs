//! The skills and rules tabs: what the model is given besides the
//! conversation, and which of it is switched on.

use crate::services::skills::{self, SkillsView};

/// The open folder's skills and the user's; only the user's while no folder is open.
#[tauri::command]
pub fn skills_list(state: tauri::State<'_, std::sync::Arc<super::agent_state::AgentState>>) -> Result<SkillsView, String> {
    skills::list(state.workspace().ok().as_deref()).map_err(|e| e.to_string())
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

/// The open folder's instruction files; none while no folder is open.
#[tauri::command]
pub fn rules_list(state: tauri::State<'_, std::sync::Arc<super::agent_state::AgentState>>) -> Vec<crate::domain::project_rules::RuleListItem> {
    state.workspace().map(|root| crate::services::project_rules::list(&root)).unwrap_or_default()
}

#[tauri::command]
pub fn rules_set_enabled(path: String, enabled: bool) -> Result<(), String> {
    crate::services::project_rules::set_enabled(&path, enabled).map_err(|e| e.to_string())
}
