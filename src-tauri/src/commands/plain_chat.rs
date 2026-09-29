//! Chat mode, reachable from the window. The conversation is the window's,
//! as with an agent turn: each reply is sent the whole of it, and the window
//! saves it. A saved one opens, archives and goes with `chat_load`,
//! `chat_set_archived` and `chat_delete`, as the agent's do.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::{AppHandle, Manager, Runtime, State};

use crate::domain::chat_record::{ChatSummary, NO_FOLDER};
use crate::domain::chat_role::ChatRole;
use crate::domain::kube::KubePin;
use crate::infra::kube_client::Clusters;
use crate::domain::llm::LlmMessage;
use crate::domain::tool_call_log::ToolCallLogEntry;
use crate::domain::tools::ApprovalPolicy;
use crate::domain::turn::{ChatStreamOutcome, PendingApproval, ToolCallDecision};
use crate::services::llm_chat::TurnError;
use crate::infra::chat_store;
use crate::services::{kubeconfigs, llm_session, plain_chat};

use super::chat_events::chat_event_sink;

/// The stop flag of the reply being written. Apart from the agent's: a chat
/// and an agent turn may run at once, and a stop is for the one it was pressed in.
#[derive(Default)]
pub struct PlainChatState {
    cancel: AtomicBool,
    /// Grows as the user answers "Always" on a chat's card; not persisted,
    /// for the agent's reason — a saved "never ask me" is a brake released a
    /// month ago and forgotten.
    approval: Mutex<ApprovalPolicy>,
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

/// A turn in `role` on `messages`, the whole conversation so far. Its text and
/// calls arrive on `chat:turn-event` under `turn_id` as they happen; this
/// resolves with how it ended — done, stopped, or paused on the approval card.
#[tauri::command]
pub async fn plain_chat_send<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, Arc<PlainChatState>>,
    turn_id: String,
    role: ChatRole,
    kube: Option<KubePin>,
    messages: Vec<LlmMessage>,
) -> Result<ChatStreamOutcome, String> {
    let state = state.inner().clone();
    state.cancel.store(false, Ordering::SeqCst);
    run_turn(app, state, turn_id, role, kube, move |chat| plain_chat::start(chat, messages)).await
}

/// Continues a turn paused on the approval card, with the user's answers. The
/// stop flag is left alone, as the agent's resume leaves it: a stop pressed
/// while the card was showing ends the resumed turn.
#[tauri::command]
pub async fn plain_chat_resume<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, Arc<PlainChatState>>,
    turn_id: String,
    role: ChatRole,
    kube: Option<KubePin>,
    checkpoint: PendingApproval,
    decisions: Vec<ToolCallDecision>,
) -> Result<ChatStreamOutcome, String> {
    let state = state.inner().clone();
    run_turn(app, state, turn_id, role, kube, move |chat| plain_chat::resume(chat, checkpoint, decisions)).await
}

/// "Always allow this tool", from a chat's approval card — for Chat mode's
/// turns only, and as long as the app runs, as the agent's is.
#[tauri::command]
pub fn plain_chat_always_allow(tool: String, state: State<'_, Arc<PlainChatState>>) -> Result<(), String> {
    state.approval.lock().map_err(|_| "approval lock poisoned".to_string())?.allow_always(&tool)
}

/// Off the event loop: a turn is synchronous — provider calls and tool calls —
/// and holding the IPC loop would freeze the command that stops it.
async fn run_turn<R, F>(
    app: AppHandle<R>,
    state: Arc<PlainChatState>,
    turn_id: String,
    role: ChatRole,
    kube: Option<KubePin>,
    run: F,
) -> Result<ChatStreamOutcome, String>
where
    R: Runtime,
    F: FnOnce(&plain_chat::ChatTurn) -> Result<ChatStreamOutcome, TurnError> + Send + 'static,
{
    let events = chat_event_sink(&app, turn_id);
    let clusters = app.state::<Arc<Clusters>>().inner().clone();
    let approval = state.approval.lock().map_err(|_| "approval lock poisoned".to_string())?.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let session = llm_session::resolve(None).map_err(|e| e.to_string())?;
        // Asked every turn: a cluster that stopped answering, or a login that
        // expired, is what the model most needs to know. The client is kept,
        // so it is two quick requests, not a new login.
        let kube = kubeconfigs::setup(kube.as_ref(), &clusters).map_err(|e| e.to_string())?;
        let cancelled = || state.cancel.load(Ordering::SeqCst);
        let record = crate::infra::tool_call_log::recorder();
        let log_call = |entry: ToolCallLogEntry| record(&entry);
        let chat = plain_chat::ChatTurn {
            session: &session,
            role,
            kube: &kube,
            approval: &approval,
            events: &events,
            cancelled: &cancelled,
            log_call: &log_call,
        };
        run(&chat).map_err(|e| e.to_string())
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
/// with the reply. `messages` is what the model is sent; `blocks` what the
/// window draws, thinking included — opaque here, as the agent's are.
#[tauri::command]
pub fn plain_chat_save(
    id: String,
    role: ChatRole,
    kube: Option<KubePin>,
    messages: Vec<LlmMessage>,
    blocks: serde_json::Value,
) -> Result<ChatSummary, String> {
    chat_store::save_plain(&id, role, kube, &messages, &blocks).map_err(|e| e.to_string())
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
        }, {
            "id": "kubernetes",
            "name": "Kubernetes",
            "description": ChatRole::Kubernetes.description(),
        }]));
    }
}
