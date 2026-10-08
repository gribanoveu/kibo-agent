//! The list of conversations, and one conversation restored.
//!
//! The window owns the transcript while a turn runs (see `commands::chat`);
//! this is where it puts it afterwards so a restart is not a fresh start.

use std::sync::Arc;

use serde_json::Value;
use tauri::State;

use crate::domain::chat_record::{ChatRecord, ChatSummary};
use crate::domain::llm::LlmMessage;
use crate::domain::tools::Task;
use crate::infra::chat_store;
use crate::services::{chat_title, llm_session};

use super::chat::AgentState;

/// Chats of the open folder, newest first. No folder open is an empty list:
/// there is nothing to be wrong about yet, and the sidebar already says so.
#[tauri::command]
pub fn chat_list(state: State<'_, Arc<AgentState>>) -> Result<Vec<ChatSummary>, String> {
    let Ok(workspace) = state.workspace() else {
        return Ok(Vec::new());
    };
    chat_store::list(&workspace.display().to_string()).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn chat_load(id: String) -> Result<ChatRecord, String> {
    chat_store::load(&id).map_err(|e| e.to_string())
}

/// Writes the conversation as it now stands. Called once a turn has ended —
/// saving mid-turn would store a transcript whose last tool call has no
/// result yet — and as a new chat's first turn starts, with its message only.
#[tauri::command]
pub fn chat_save(
    state: State<'_, Arc<AgentState>>,
    id: String,
    messages: Vec<LlmMessage>,
    blocks: Value,
    todos: Vec<Task>,
    plan: Option<String>,
    branched_from: Option<String>,
) -> Result<ChatSummary, String> {
    let workspace = state.workspace()?;
    let first = !chat_store::exists(&id).map_err(|e| e.to_string())?;
    let summary = chat_store::save(
        &id,
        &workspace.display().to_string(),
        &messages,
        &blocks,
        &todos,
        plan.as_deref(),
        branched_from.as_deref(),
    )
    .map_err(|e| e.to_string())?;
    if first {
        super::chat::remember_new_chat(&state, &id);
    }
    Ok(summary)
}

/// Names a new chat with the active provider's model, from its first message.
/// Called once that message is saved, beside the turn; the window re-reads the
/// list when it returns. A failure is only reported: the chat keeps the name it
/// was saved with.
#[tauri::command]
pub async fn chat_generate_title(id: String) -> Result<(), String> {
    // A request to the provider, which would freeze the IPC loop for its duration.
    tauri::async_runtime::spawn_blocking(move || {
        let session = llm_session::resolve(None).map_err(|e| e.to_string())?;
        chat_title::name(&session, &id).map(|_| ()).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Writes one saved chat to a file the user picked, as Markdown. The path is
/// theirs — it comes back from the system save dialog — so nothing here
/// second-guesses where it points.
#[tauri::command]
pub fn chat_export(id: String, path: String) -> Result<(), String> {
    chat_store::export(&id, std::path::Path::new(&path)).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn chat_set_archived(id: String, archived: bool) -> Result<(), String> {
    chat_store::set_archived(&id, archived).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn chat_delete(id: String) -> Result<(), String> {
    chat_store::delete(&id).map_err(|e| e.to_string())
}
