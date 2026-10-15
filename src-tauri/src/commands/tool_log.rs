//! The tool-call log window: reading it, emptying it, switching it off.

use crate::domain::tool_call_log::{ToolCallLogFilter, ToolCallLogPage};
use crate::infra::{settings_store, tool_call_log};

use super::blocking;

#[tauri::command]
pub async fn tool_log_query(filter: ToolCallLogFilter) -> Result<ToolCallLogPage, String> {
    blocking(move || tool_call_log::query(&filter)).await
}

#[tauri::command]
pub async fn tool_log_clear() -> Result<usize, String> {
    blocking(tool_call_log::clear).await
}

#[tauri::command]
pub fn tool_log_enabled_get() -> Result<bool, String> {
    Ok(settings_store::load().map_err(|e| e.to_string())?.tool_log.enabled)
}

/// Refuses on settings it cannot read, rather than saving defaults over them.
#[tauri::command]
pub fn tool_log_enabled_set(enabled: bool) -> Result<(), String> {
    let mut settings = settings_store::load().map_err(|e| e.to_string())?;
    settings.tool_log.enabled = enabled;
    settings_store::save(&settings).map_err(|e| e.to_string())
}
