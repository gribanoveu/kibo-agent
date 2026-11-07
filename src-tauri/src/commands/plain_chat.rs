//! Chat mode, reachable from the window. The conversation is the window's,
//! as with an agent turn: each reply is sent the whole of it, and the window
//! saves it. A saved one opens, archives and goes with `chat_load`,
//! `chat_set_archived` and `chat_delete`, as the agent's do.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::{AppHandle, Manager, Runtime, State};

use crate::domain::chat_record::{ChatSummary, NO_FOLDER};
use crate::domain::chat_role::ChatRole;
use crate::domain::kube::{KubeApi, KubePin, KubeSetup};
use crate::infra::kube_client::{ClusterApi, Clusters};
use crate::domain::llm::{LlmMessage, LlmToolCall};
use crate::domain::runbooks;
use crate::infra::runbooks_store;
use crate::domain::tools::{ApprovalPolicy, ToolPreview};
use crate::domain::turn::{ChatStreamOutcome, PendingApproval, PendingToolCall, ToolCallDecision};
use crate::services::llm_chat::TurnError;
use crate::services::llm_session::LlmSession;
use crate::infra::chat_store;
use crate::domain::compaction::{ContextUsage, RequestFrame};
use crate::services::turn_setup::{self, ChatTurnInput};
use crate::services::{context_compaction, kubeconfigs, llm_session, plain_chat};

use super::chat::CompactedHistory;
use super::chat_events::chat_event_sink;

/// One chat's stop flag and admission. Separate handles keep parallel replies
/// from stopping, replacing, or resuming one another.
#[derive(Default)]
struct PlainChatSession {
    cancel: AtomicBool,
    running: AtomicBool,
    paused: AtomicBool,
}

struct RunningPlainTurn(Arc<PlainChatSession>);

impl std::fmt::Debug for RunningPlainTurn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RunningPlainTurn")
    }
}

impl Drop for RunningPlainTurn {
    fn drop(&mut self) {
        self.0.running.store(false, Ordering::SeqCst);
    }
}

#[derive(Default)]
pub struct PlainChatState {
    sessions: Mutex<HashMap<String, Arc<PlainChatSession>>>,
    /// Grows as the user answers "Always" on a chat's card; not persisted,
    /// for the agent's reason — a saved "never ask me" is a brake released a
    /// month ago and forgotten.
    approval: Mutex<ApprovalPolicy>,
}

impl PlainChatState {
    fn session(&self, chat_id: &str) -> Arc<PlainChatSession> {
        let mut sessions = self.sessions.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        Arc::clone(sessions.entry(chat_id.to_string()).or_default())
    }

    fn begin(&self, chat_id: &str, limit: u32, resume: bool) -> Result<RunningPlainTurn, String> {
        let limit = limit.max(1);
        let mut sessions = self.sessions.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let session = Arc::clone(sessions.entry(chat_id.to_string()).or_default());
        if session.running.load(Ordering::SeqCst) {
            return Err("this chat's turn is already running".to_string());
        }
        if !resume && session.paused.load(Ordering::SeqCst) {
            return Err("this chat's turn is waiting for approval — resume or stop it first".to_string());
        }
        let running = sessions.values().filter(|session| session.running.load(Ordering::SeqCst)).count() as u32;
        if running >= limit {
            return Err(format!("{running} chat turns are already running — wait for one to finish, or raise the parallel-turns limit in Settings"));
        }
        session.running.store(true, Ordering::SeqCst);
        session.paused.store(false, Ordering::SeqCst);
        Ok(RunningPlainTurn(session))
    }
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

/// A turn of the chat `chat_id` in `role` on `messages`, the whole conversation
/// so far. Its text and calls arrive on `chat:turn-event` under `turn_id` as
/// they happen; this resolves with how it ended — done, stopped, or paused on
/// the approval card.
#[tauri::command]
pub async fn plain_chat_send<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, Arc<PlainChatState>>,
    turn_id: String,
    chat_id: String,
    role: ChatRole,
    kube: Option<KubePin>,
    messages: Vec<LlmMessage>,
) -> Result<ChatStreamOutcome, String> {
    let state = state.inner().clone();
    let running = state.begin(&chat_id, super::chat::parallel_limit(), false)?;
    let session = state.session(&chat_id);
    session.cancel.store(false, Ordering::SeqCst);
    crate::infra::daily_metrics::record(&[(crate::domain::metrics::Metric::Prompts, "", 1)]);
    run_turn(app, state, session, running, turn_id, chat_id, role, kube, move |chat| plain_chat::start(chat, messages)).await
}

/// Continues a turn paused on the approval card, with the user's answers. The
/// stop flag is left alone, as the agent's resume leaves it: a stop pressed
/// while the card was showing ends the resumed turn.
#[tauri::command]
pub async fn plain_chat_resume<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, Arc<PlainChatState>>,
    turn_id: String,
    chat_id: String,
    role: ChatRole,
    kube: Option<KubePin>,
    checkpoint: PendingApproval,
    decisions: Vec<ToolCallDecision>,
) -> Result<ChatStreamOutcome, String> {
    let state = state.inner().clone();
    let running = state.begin(&chat_id, super::chat::parallel_limit(), true)?;
    let session = state.session(&chat_id);
    run_turn(app, state, session, running, turn_id, chat_id, role, kube, move |chat| plain_chat::resume(chat, checkpoint, decisions)).await
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
    session: Arc<PlainChatSession>,
    running: RunningPlainTurn,
    turn_id: String,
    chat_id: String,
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
    let board: Arc<dyn crate::domain::board::TaskBoard> = app.state::<Arc<crate::infra::board_store::Board>>().inner().clone();
    let embeddings = app.state::<Arc<crate::services::workspace_index::WorkspaceIndex>>().embeddings();
    let knowledge = super::chat::knowledge_searcher_of(&app);
    let mongo = super::chat::mongo_servers_of(&app);
    let http = Some(super::chat::http_stands_of(&app, &chat_id));
    let approval = state.approval.lock().map_err(|_| "approval lock poisoned".to_string())?.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _running = running;
        let cancelled = || session.cancel.load(Ordering::SeqCst);
        let chat = ChatTurnInput { chat_id, role, kube, clusters, board, mongo, http, embeddings, knowledge, approval, events: &events, cancelled: &cancelled };
        let outcome = turn_setup::run_in_chat(chat, run).map_err(|e| e.to_string())?;
        session.paused.store(matches!(&outcome, ChatStreamOutcome::PendingApproval(_)), Ordering::SeqCst);
        Ok(outcome)
    })
    .await
    .map_err(|e| format!("the chat thread failed: {e}"))?
}

/// What the next request of a chat in `role` will cost, for its meter — the
/// agent's `chat_context_usage` with the role's prompt and tools. Off the
/// event loop: the prompt says what the pinned cluster answers, which asks it.
#[tauri::command]
pub async fn plain_chat_context_usage<R: Runtime>(
    app: AppHandle<R>,
    role: ChatRole,
    kube: Option<KubePin>,
    messages: Vec<LlmMessage>,
) -> Result<ContextUsage, String> {
    let clusters = app.state::<Arc<Clusters>>().inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (session, frame) = chat_frame(&clusters, role, kube.as_ref(), None)?;
        Ok(context_compaction::usage(&session, frame, &messages))
    })
    .await
    .map_err(|e| format!("the context thread failed: {e}"))?
}

/// The agent's `chat_compact` for a chat in `role`: sent before each message,
/// and by "Compact now" (`force`), against the role's own prompt and tools.
#[tauri::command]
pub async fn plain_chat_compact<R: Runtime>(
    app: AppHandle<R>,
    role: ChatRole,
    kube: Option<KubePin>,
    messages: Vec<LlmMessage>,
    force: bool,
    turn_id: String,
    chat_id: Option<String>,
) -> Result<Option<CompactedHistory>, String> {
    let clusters = app.state::<Arc<Clusters>>().inner().clone();
    let sink = chat_event_sink(&app, turn_id);
    tauri::async_runtime::spawn_blocking(move || {
        let (session, frame) = chat_frame(&clusters, role, kube.as_ref(), chat_id.as_deref())?;
        super::chat::compact(&session, frame, &messages, force, sink)
    })
    .await
    .map_err(|e| format!("the compaction thread failed: {e}"))?
}

/// The provider, and what a chat's request sends in front of the conversation.
/// What the session spends is counted under `chat`, when there is one yet.
fn chat_frame(clusters: &Clusters, role: ChatRole, kube: Option<&KubePin>, chat: Option<&str>) -> Result<(LlmSession, RequestFrame), String> {
    let session = llm_session::resolve_for(None, chat).map_err(|e| e.to_string())?;
    let kube = kubeconfigs::setup(kube, clusters).map_err(|e| e.to_string())?;
    let frame = context_compaction::chat_request_frame(role, &kube, &runbooks::merged(runbooks_store::own()),
        session.reply_language,
        crate::services::web_search::is_on(crate::domain::settings::WebSearchPlace::Chat),
        &crate::services::features::enabled(),
    );
    Ok((session, frame))
}

/// What the calls on a chat's approval card would do: for a change to the
/// cluster, where and from what to what. Reads the cluster — a dry run — so
/// it runs off the event loop.
#[tauri::command]
pub async fn plain_chat_preview<R: Runtime>(
    app: AppHandle<R>,
    kube: Option<KubePin>,
    calls: Vec<PendingToolCall>,
) -> Result<Vec<ToolPreview>, String> {
    let clusters = app.state::<Arc<Clusters>>().inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let setup = kubeconfigs::setup(kube.as_ref(), &clusters).map_err(|e| e.to_string())?;
        let api = match &setup {
            KubeSetup::Pinned(target) => {
                Some(ClusterApi::new(clusters.clone(), std::path::Path::new(&target.config.path), &target.context))
            }
            _ => None,
        };
        let calls: Vec<LlmToolCall> =
            calls.into_iter().map(|call| LlmToolCall { id: call.id, name: call.name, arguments: call.arguments }).collect();
        Ok(plain_chat::preview(&setup, api.as_ref().map(|api| api as &dyn KubeApi), &calls))
    })
    .await
    .map_err(|e| format!("the preview thread failed: {e}"))?
}

/// Chat mode's conversations, newest first — whatever folder is open.
#[tauri::command]
pub async fn plain_chat_list() -> Result<Vec<ChatSummary>, String> {
    super::blocking(|| chat_store::list(NO_FOLDER)).await
}

/// Writes the conversation as it now stands: as a message is sent, and again
/// with the reply. `messages` is what the model is sent; `blocks` what the
/// window draws, thinking included — opaque here, as the agent's are.
#[tauri::command]
pub async fn plain_chat_save(
    id: String,
    role: ChatRole,
    kube: Option<KubePin>,
    messages: Vec<LlmMessage>,
    blocks: serde_json::Value,
) -> Result<ChatSummary, String> {
    super::blocking(move || chat_store::save_plain(&id, role, kube, &messages, &blocks)).await
}

/// Returns at once; this chat's reply stops at its next chunk.
#[tauri::command]
pub fn plain_chat_cancel(chat_id: String, state: State<'_, Arc<PlainChatState>>) {
    let session = state.session(&chat_id);
    session.cancel.store(true, Ordering::SeqCst);
    session.paused.store(false, Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chats_have_independent_stop_and_admission() {
        let state = PlainChatState::default();
        let one = state.begin("c1", 2, false).expect("the first chat runs");
        let _two = state.begin("c2", 2, false).expect("a second chat runs beside it");
        let refused = state.begin("c3", 2, false).expect_err("the limit binds across chats");
        assert!(refused.contains("limit"), "{refused}");

        state.session("c1").cancel.store(true, Ordering::SeqCst);
        assert!(!state.session("c2").cancel.load(Ordering::SeqCst), "one chat's stop is not another's");
        drop(one);
        state.session("c1").paused.store(true, Ordering::SeqCst);
        let paused = state.begin("c1", 2, false).expect_err("a fresh start cannot orphan the card");
        assert!(paused.contains("waiting for approval"), "{paused}");
        state.begin("c1", 2, true).expect("the card resumes its own chat");

        let state = Arc::new(state);
        let app = tauri::test::mock_app();
        app.manage(Arc::clone(&state));
        state.session("c1").paused.store(true, Ordering::SeqCst);
        plain_chat_cancel("c1".into(), app.state());
        assert!(!state.session("c1").paused.load(Ordering::SeqCst), "stop releases a chat whose card was lost");
        state.begin("c1", 2, false).expect("a fresh turn can start after stop");
    }

    /// A Chat-mode compaction is that chat's spending, as the agent's is.
    #[test]
    fn a_compaction_counts_under_its_chat() {
        crate::testing::with_app_dir("plain-compact-spent", || {
            crate::testing::spending_provider("the earlier part", 40, 6);
            let app = tauri::test::mock_app();
            app.manage(Arc::new(Clusters::default()));
            // Past the default window, so there is an older part to fold.
            let history = (0..40).map(|i| if i % 2 == 0 { LlmMessage::user(format!("question {i} {}", "x".repeat(20_000))) } else { LlmMessage::assistant(format!("answer {i}")) }).collect();
            let compacted = tauri::async_runtime::block_on(plain_chat_compact(
                app.handle().clone(),
                ChatRole::Assistant,
                None,
                history,
                true,
                "t".into(),
                Some("p8".into()),
            ));
            assert!(compacted.expect("compacted").is_some());
            assert_eq!(crate::testing::chat_tokens("p8"), 46);
        });
    }

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
        }, {
            "id": "prototype",
            "name": "Prototype",
            "description": ChatRole::Prototype.description(),
        }]));
    }
}
