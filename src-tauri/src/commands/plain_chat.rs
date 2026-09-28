//! Chat mode, reachable from the window. The conversation is the window's,
//! as with an agent turn: each reply is sent the whole of it, and the window
//! saves it. A saved one opens, archives and goes with `chat_load`,
//! `chat_set_archived` and `chat_delete`, as the agent's do.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Runtime, State};

use crate::domain::chat_record::{ChatSummary, NO_FOLDER};
use crate::domain::chat_role::ChatRole;
use crate::domain::llm::{ChatStreamResult, LlmMessage};
use crate::infra::chat_store;
use crate::services::{llm_session, plain_chat};

use super::chat_events::chat_event_sink;

/// The stop flag of the reply being written. Apart from the agent's: a chat
/// and an agent turn may run at once, and a stop is for the one it was pressed in.
#[derive(Default)]
pub struct PlainChatState {
    cancel: AtomicBool,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleView {
    id: ChatRole,
    name: &'static str,
    description: &'static str,
}

#[tauri::command]
pub fn plain_chat_roles() -> Vec<RoleView> {
    ChatRole::ALL.iter().map(|&role| RoleView { id: role, name: role.name(), description: role.description() }).collect()
}

/// The model's reply to `messages` in `role`. Its text arrives on
/// `chat:turn-event` under `turn_id` as it is written; this resolves with all of it.
#[tauri::command]
pub async fn plain_chat_send<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, Arc<PlainChatState>>,
    turn_id: String,
    role: ChatRole,
    messages: Vec<LlmMessage>,
) -> Result<ChatStreamResult, String> {
    let state = state.inner().clone();
    state.cancel.store(false, Ordering::SeqCst);
    let events = chat_event_sink(&app, turn_id);
    tauri::async_runtime::spawn_blocking(move || {
        let session = llm_session::resolve(None).map_err(|e| e.to_string())?;
        plain_chat::reply(&session, role, messages, &events, &|| state.cancel.load(Ordering::SeqCst))
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("the chat thread failed: {e}"))?
}

/// Chat mode's conversations, newest first — whatever folder is open.
#[tauri::command]
pub fn plain_chat_list() -> Result<Vec<ChatSummary>, String> {
    chat_store::list(NO_FOLDER).map_err(|e| e.to_string())
}

/// Writes the conversation as it now stands: as a message is sent, and again
/// with the reply.
#[tauri::command]
pub fn plain_chat_save(id: String, role: ChatRole, messages: Vec<LlmMessage>) -> Result<ChatSummary, String> {
    chat_store::save_plain(&id, role, &messages).map_err(|e| e.to_string())
}

/// Returns at once; the reply stops at its next chunk.
#[tauri::command]
pub fn plain_chat_cancel(state: State<'_, Arc<PlainChatState>>) {
    state.cancel.store(true, Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_role_is_listed_by_its_wire_name() {
        let roles = serde_json::to_value(plain_chat_roles()).unwrap();
        assert_eq!(roles, serde_json::json!([{
            "id": "assistant",
            "name": "Assistant",
            "description": ChatRole::Assistant.description(),
        }]));
    }
}
