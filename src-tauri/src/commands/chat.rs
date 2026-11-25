//! The turn, reachable from the window.
//!
//! Every command here is thin on purpose: check what came across the wire,
//! call one service, flatten the error to a string. The decisions all live
//! below this file.
//!
//! ## What the backend does not remember
//!
//! The conversation is not kept here. `chat_start` receives the whole history
//! and `chat_resume` receives the whole checkpoint, which is what makes a
//! paused turn survive a reload, a crash, or being picked up by a different
//! window. What *is* resident is only what a running turn needs to be reached
//! from outside while it runs — and that is resident **per chat**: each
//! chat's turns are reached through their own `SessionHandle` (`agent_state`), so two chats'
//! turns can run side by side without one's stop or steering note touching
//! the other's.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use tauri::{AppHandle, Manager, Runtime, State};

use crate::domain::compaction::{ContextUsage, RequestFrame};
use crate::domain::image::ImagePart;
use crate::domain::llm::{LlmMessage, LlmToolCall};
use crate::domain::tools::{CodeSearchFn, Task, ToolPreview, ToolScope};
use crate::domain::next_prompt::{self};
use crate::domain::turn::{
    ChatEventPayload, ChatEventSink, ChatStreamOutcome, ChatTurnEvent, PendingApproval, PendingToolCall, SteeringNote,
    ToolCallDecision, TurnFailure, TurnRecovery,
};
use crate::services::ai_tools::preview;
use crate::services::llm_chat::{self, Turn, TurnError};
use crate::services::context_compaction;
use crate::services::mcp_questions::McpQuestions;
use crate::services::mcp_servers::McpServers;
use crate::domain::mcp::McpAnswer;
use crate::infra::background::Processes;
use crate::services::llm_session;
use crate::services::turn_setup::{self, FolderHandles, FolderTurn, TurnSetupError};
use crate::services::workspace_index::WorkspaceIndex;

use super::chat_events::chat_event_sink;

use super::agent_state::{AgentState, SessionHandle, RunningTurn};

/// What a start or a resume refuses with: a plain message (admission, a
/// closed folder), or a turn that failed with work to keep. Untagged, so
/// the window reads a string as before and an object as [`TurnFailure`].
#[derive(Debug, serde::Serialize)]
#[serde(untagged)]
pub enum ChatTurnError {
    Message(String),
    Failed(TurnFailure),
}

impl ChatTurnError {
    /// The message a window shows, whichever kind this is.
    pub fn message(&self) -> &str {
        match self {
            Self::Message(message) => message,
            Self::Failed(failure) => &failure.message,
        }
    }
}

/// The chat's request markers: the previous fingerprint lives in the
/// session, across turns; the row goes to the call log's database, hashes
/// and numbers only. The adapter is the one place that knows the chat and
/// the turn — the loop knows neither.
struct ChatMarkers {
    session: Arc<SessionHandle>,
    chat_id: String,
    turn_id: String,
}

impl crate::domain::request_marker::RequestMarkers for ChatMarkers {
    fn previous(&self) -> Option<crate::domain::request_marker::RequestFingerprint> {
        self.session.last_request.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    fn record(&self, record: crate::domain::request_marker::RequestRecord) {
        *self.session.last_request.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(record.fingerprint.clone());
        if let Some(anchor) = crate::domain::request_marker::Anchor::of(&record) {
            *self.session.last_anchor.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(anchor);
        }
        crate::infra::tool_call_log::append_request(&self.chat_id, &self.turn_id, &record);
    }

    fn last_anchor(&self) -> Option<crate::domain::request_marker::Anchor> {
        self.session.last_anchor.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }
}

/// The turn's journal on disk: the history's tail after each completed
/// round, the whole of it after a rewrite. Best-effort — a row that cannot
/// be written costs crash protection, never the turn. `len` is the count
/// of messages already journaled, read back on a resume so the tail is
/// counted from where the pause left it.
struct ChatJournal {
    chat_id: String,
    turn_id: String,
    len: AtomicUsize,
}

impl ChatJournal {
    /// The count is read from the store: a resume picks the tail up from
    /// where the pause left it, not from zero. A store that cannot be read
    /// gives no journal at all for this turn — counting from zero would
    /// journal the whole history as a tail and duplicate it on recovery.
    fn new(chat_id: String, turn_id: String) -> Option<Self> {
        let count = crate::infra::chat_store::journal_message_count(&chat_id, &turn_id).ok()?;
        Some(Self { chat_id, turn_id, len: AtomicUsize::new(count) })
    }
}

impl crate::domain::turn_journal::TurnJournal for ChatJournal {
    fn len(&self) -> usize {
        self.len.load(Ordering::SeqCst)
    }

    fn append(&self, messages: &[LlmMessage]) {
        if messages.is_empty() {
            return;
        }
        // A row that cannot be serialized is not written and not counted:
        // a null body the replay would skip must not move the tail past it.
        let Ok(body) = serde_json::to_value(messages) else { return };
        if crate::infra::chat_store::journal_append(&self.chat_id, &self.turn_id, "append", &body, messages.len()).is_ok() {
            self.len.fetch_add(messages.len(), Ordering::SeqCst);
        }
    }

    fn snapshot(&self, history: &[LlmMessage]) {
        let Ok(body) = serde_json::to_value(history) else { return };
        if crate::infra::chat_store::journal_append(&self.chat_id, &self.turn_id, "snapshot", &body, history.len()).is_ok() {
            self.len.store(history.len(), Ordering::SeqCst);
        }
    }
}

/// Starts a fresh turn and resolves when it ends, pauses, or is stopped.
///
/// `messages` is the whole conversation so far: the caller owns the
/// transcript, and this stays a function of its arguments.
#[tauri::command]
pub async fn chat_start<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, Arc<AgentState>>,
    turn_id: String,
    chat_id: String,
    messages: Vec<LlmMessage>,
    todos: Vec<Task>,
    plan: Option<String>,
    workspace: Option<String>,
) -> Result<ChatStreamOutcome, ChatTurnError> {
    let state = state.inner().clone();
    let (workspace, worktree_base) = state.chat_workspace_in(&chat_id, workspace.as_deref()).map_err(ChatTurnError::Message)?;
    let session = state.session(&chat_id);
    // Admission before anything about the session is touched: a refused
    // start must leave the running turn's stop flag and journal alone.
    let running = state.begin_running(&chat_id, parallel_limit(), &workspace).map_err(ChatTurnError::Message)?;
    state.still_open(&workspace).map_err(ChatTurnError::Message)?;
    // An unsaved chat stays in this folder when the window moves to another.
    session.home.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get_or_insert_with(|| workspace.clone());
    // A stray stop from a turn that already finished must not end this one
    // before it starts.
    session.cancel.store(false, Ordering::SeqCst);
    session.begin_turn(state.mode());
    // The prompt is counted even if the turn's setup then fails. The counter
    // opens SQLite and waits on its lock, so it runs off the loop, detached:
    // best-effort, and nothing downstream reads it.
    tauri::async_runtime::spawn_blocking(|| crate::infra::daily_metrics::record(&[(crate::domain::metrics::Metric::Prompts, "", 1)]));
    run_off_the_event_loop(app, state, session, running, turn_id, chat_id, workspace, worktree_base, plan, move |turn| {
        llm_chat::stream(turn, messages, todos)
    })
    .await
}

/// Continues a turn that paused for approval, with the decisions the user
/// made. The checkpoint goes back exactly as it arrived.
#[tauri::command]
pub async fn chat_resume<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, Arc<AgentState>>,
    turn_id: String,
    chat_id: String,
    checkpoint: PendingApproval,
    decisions: Vec<ToolCallDecision>,
    plan: Option<String>,
) -> Result<ChatStreamOutcome, ChatTurnError> {
    let state = state.inner().clone();
    let (workspace, worktree_base) = state.chat_workspace(&chat_id).map_err(ChatTurnError::Message)?;
    let session = state.session(&chat_id);
    let running = state.begin_resume(&chat_id, parallel_limit(), &workspace).map_err(ChatTurnError::Message)?;
    if let Err(closed) = state.still_open(&workspace) {
        // `begin_resume` took the card's pause: given back while the turn
        // still counts as running, so no fresh start slips in between and
        // orphans the card.
        session.paused.store(true, std::sync::atomic::Ordering::SeqCst);
        drop(running);
        return Err(ChatTurnError::Message(closed));
    }
    run_off_the_event_loop(app, state, session, running, turn_id, chat_id, workspace, worktree_base, plan, move |turn| {
        llm_chat::resume(turn, checkpoint, decisions)
    })
    .await
}

/// What the calls of a paused round would do, worked out without doing them.
///
/// Asked for the whole round at once: the card shows every call, and one round
/// trip beats one per call. Nothing here runs anything.
#[tauri::command]
pub async fn chat_preview(
    chat_id: String,
    calls: Vec<PendingToolCall>,
    state: State<'_, Arc<AgentState>>,
) -> Result<Vec<ToolPreview>, String> {
    let (workspace, worktree_base) = state.chat_workspace(&chat_id)?;
    let calls: Vec<LlmToolCall> = calls
        .into_iter()
        .map(|call| LlmToolCall {
            id: call.id,
            name: call.name,
            arguments: call.arguments,
        })
        .collect();
    // Reads the files the calls would change, and diffs them.
    super::blocking(move || {
        let scope = ToolScope::new(&workspace).map_err(|e| e.to_string())?;
        Ok::<_, String>(preview::preview_round(&scope, &calls, worktree_base.as_deref()))
    })
    .await
}

/// What the conversation looks like once the older part of it is a summary.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactedHistory {
    pub history: Vec<LlmMessage>,
    /// How many messages the one summary now stands for.
    pub folded: usize,
}

/// Shortens the conversation, if it is worth shortening.
///
/// Called by the window before it sends, and by the user asking outright
/// (`force`). It is the window that asks because the window owns the history
/// — but nothing about *when* or *how much* is decided there: this returns
/// `None` whenever the answer is "leave it alone", so the rules stay on this
/// side and cannot drift into a second copy written in TypeScript.
///
/// The start of a pass is said on `chat:turn-event` under `turn_id`, as a
/// turn's own pass says it: the summary takes seconds, and only this side
/// knows whether one is being made. The end is what this returns.
#[tauri::command]
pub async fn chat_compact<R: Runtime>(
    messages: Vec<LlmMessage>,
    force: bool,
    turn_id: String,
    chat_id: String,
    app: AppHandle<R>,
    state: State<'_, Arc<AgentState>>,
) -> Result<Option<CompactedHistory>, String> {
    let state = Arc::clone(&state);
    let sink = chat_event_sink(&app, turn_id);
    // A summary is an ordinary request to the provider, and a request on the
    // IPC loop freezes every other command for its duration. The measurement
    // waits too — it reads the chat record, probes the shell and reads the
    // marker row — so it all happens off the loop, as `chat_context_usage`.
    tauri::async_runtime::spawn_blocking(move || {
        let workspace = state.chat_prompt_workspace(&chat_id)?;
        let session = llm_session::resolve_for(None, Some(&chat_id)).map_err(|e| e.to_string())?;
        let (frame, system, tools) = next_request_frame(&app, &state, workspace);
        let anchor = chat_anchor(&state, &chat_id);
        let current = crate::domain::request_marker::fingerprint(&system, &tools, &messages);
        compact_anchored(&session, frame, &messages, force, sink, anchor.as_ref(), &current)
    })
    .await
    .map_err(|e| format!("the compaction thread failed: {e}"))?
}

/// Folds the older history when the next request is worth it — or at once,
/// `force` — saying on `sink` when a summary is being made. The agent's and
/// a chat's compaction differ only in the frame they measure against.
pub(super) fn compact(
    session: &llm_session::LlmSession,
    frame: RequestFrame,
    messages: &[LlmMessage],
    force: bool,
    sink: ChatEventSink,
) -> Result<Option<CompactedHistory>, String> {
    compact_anchored(session, frame, messages, force, sink, None, &crate::domain::request_marker::RequestFingerprint::empty())
}

/// The same pass with the provider's anchor: the threshold decides on the
/// anchored total, so a fold fires at the real share of the window (docs/60).
pub(super) fn compact_anchored(
    session: &llm_session::LlmSession,
    frame: RequestFrame,
    messages: &[LlmMessage],
    force: bool,
    sink: ChatEventSink,
    anchor: Option<&crate::domain::request_marker::Anchor>,
    current: &crate::domain::request_marker::RequestFingerprint,
) -> Result<Option<CompactedHistory>, String> {
    let started = || sink(ChatTurnEvent { seq: 1, round: 0, target_id: None, event: ChatEventPayload::HistoryCompacting });
    context_compaction::anchored_compact_if_needed(session, frame, messages, force, &started, anchor, current)
        .map(|compacted| compacted.map(|c| CompactedHistory { history: c.history, folded: c.folded }))
        .map_err(|e| e.to_string())
}

/// The message that stands for the part of a chat being rewound away from —
/// `messages`, from the rewound message on — for the window to add after the
/// conversation it keeps. `None` when the summary came back empty.
#[tauri::command]
pub async fn chat_branch_summary(messages: Vec<LlmMessage>, chat_id: String) -> Result<Option<LlmMessage>, String> {
    // A request to the provider: off the IPC loop, as a compaction's.
    tauri::async_runtime::spawn_blocking(move || {
        let session = llm_session::resolve_for(None, Some(&chat_id)).map_err(|e| e.to_string())?;
        context_compaction::summarize_branch(&session, &messages).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("the summary thread failed: {e}"))?
}

/// What the next request will cost, for the window's meter.
///
/// Asked rather than computed there: the numbers on the meter have to be the
/// ones that decide, or a reader watches a gauge that is not connected to the
/// thing it appears to measure. Not cheap enough for the main thread: the
/// session reads the key, and with no model pinned asks the provider for its
/// list and saves the first as the pin; the frame reads the rules, skills and
/// the repository.
#[tauri::command]
pub async fn chat_context_usage<R: Runtime>(
    messages: Vec<LlmMessage>,
    chat_id: String,
    app: AppHandle<R>,
    state: State<'_, Arc<AgentState>>,
) -> Result<ContextUsage, String> {
    let state = Arc::clone(&state);
    super::blocking(move || {
        let workspace = state.chat_prompt_workspace(&chat_id)?;
        // The chat's own provider: the meter measures the session the turn
        // will actually use (docs/64-chat-model.md).
        let session = llm_session::resolve_for(None, Some(&chat_id)).map_err(|e| e.to_string())?;
        let (frame, system, tools) = next_request_frame(&app, &state, workspace);
        let anchor = chat_anchor(&state, &chat_id);
        let current = crate::domain::request_marker::fingerprint(&system, &tools, &messages);
        Ok::<_, String>(context_compaction::anchored_usage(&session, frame, &messages, anchor.as_ref(), &current))
    })
    .await
}

/// What the next turn of the open window will send in front of the
/// conversation ([`turn_setup::next_folder_frame`]), in the mode picked now.
fn next_request_frame<R: Runtime>(app: &AppHandle<R>, state: &AgentState, workspace: Option<PathBuf>) -> (RequestFrame, Vec<LlmMessage>, Vec<crate::domain::llm::LlmToolDefinition>) {
    let servers = app.try_state::<Arc<McpServers>>();
    turn_setup::next_folder_frame(workspace.as_deref(), state.mode(), servers.as_ref().map(|s| s.as_ref()))
}

/// The chat's anchor for the meter: the session's last costed request, or the
/// marker row after a reopen or a restart (docs/60).
fn chat_anchor(state: &AgentState, chat_id: &str) -> Option<crate::domain::request_marker::Anchor> {
    state
        .session(chat_id)
        .last_anchor
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
        .or_else(|| crate::infra::tool_call_log::last_anchor(chat_id))
}

/// Journals the turn `turn_id` for the next-prompt model: its input, built
/// from what the turn did and `user`, the message that started it as the
/// transcript shows it. `None` when there is nothing to journal — the turn is
/// not the last one that ended, was journaled already, or said nothing.
#[tauri::command]
pub async fn next_prompt_log(
    chat_id: String,
    turn_id: String,
    user: String,
    state: State<'_, Arc<AgentState>>,
) -> Result<Option<String>, String> {
    let Some(finished) = state.session(&chat_id).take_finished(&turn_id) else {
        return Ok(None);
    };
    let input = finished.model_input(&user, &home());
    super::blocking(move || crate::infra::next_prompt_log::record(&chat_id, &input).map(Some)).await
}

/// What the user sent after the journaled turn `id`.
#[tauri::command]
pub async fn next_prompt_sent(id: String, text: String) -> Result<(), String> {
    super::blocking(move || crate::infra::next_prompt_log::record_sent(&id, &next_prompt::clean(&text, &home()))).await
}

fn home() -> String {
    dirs::home_dir().map(|home| home.display().to_string()).unwrap_or_default()
}

/// The window's answer to an MCP server's question, by the id the question
/// came with. A question no longer open — its call stopped — is said to be.
#[tauri::command]
pub fn chat_answer_mcp_question(id: String, answer: McpAnswer, questions: State<'_, Arc<McpQuestions>>) -> Result<(), String> {
    if questions.answer(&id, answer) {
        Ok(())
    } else {
        Err("the question is no longer open: its call has ended".into())
    }
}

/// Asks the running turn of `chat_id` to stop. Returns at once: the turn
/// notices at its next checkpoint, which is never more than one tool call
/// away. Another chat's turn is not its to stop.
#[tauri::command]
pub fn chat_cancel(chat_id: String, state: State<'_, Arc<AgentState>>) {
    let session = state.session(&chat_id);
    session.cancel.store(true, Ordering::SeqCst);
    session.paused.store(false, Ordering::SeqCst);
}

/// Queues something the user typed while the turn was running. The id comes
/// back so the same note can be withdrawn before a round takes it. `shown`
/// is what the transcript shows in place of `text` — a `/` command — and
/// `images` are the pictures attached to it, sent to the model with the note
/// and drawn on its bubble.
///
/// A ponytail: the pictures ride as this command's arguments, up to ten of
/// them at a megabyte each, and it is one of the few commands left synchronous
/// — the body is a `Vec::push`, and where Tauri reads the arguments is its
/// own. It has not been felt as a freeze; the ceiling is ten screenshots
/// typed into a running turn at once, and the way past it is to hand them over
/// by an id the turn already holds instead of through the call.
#[tauri::command]
pub fn chat_steer(
    text: String,
    shown: Option<String>,
    images: Option<Vec<ImagePart>>,
    chat_id: String,
    state: State<'_, Arc<AgentState>>,
) -> String {
    let note = SteeringNote { shown, images: images.unwrap_or_default(), ..SteeringNote::user(text) };
    let id = note.id.clone();
    state.session(&chat_id).steering.push(note);
    id
}

/// `false` means the note is already gone — a round picked it up while the
/// user was reaching for cancel.
#[tauri::command]
pub fn chat_cancel_steer(id: String, chat_id: String, state: State<'_, Arc<AgentState>>) -> bool {
    state.session(&chat_id).steering.cancel(&id)
}

/// The chat folder's search, for the turn's tools — that folder's index and no
/// other open one's (F-8.2). Opens the index if the folder is in the
/// background, and keeps it open while the turn, or a helper of it, holds the
/// searcher (LZ-4). Blocking: it may read the folder's store.
fn searcher_of<R: Runtime>(app: &AppHandle<R>, workspace: &std::path::Path) -> Option<CodeSearchFn> {
    let sink = super::workspace_events::index_event_sink(app, workspace.display().to_string());
    app.try_state::<Arc<WorkspaceIndex>>()?.searcher(workspace, sink)
}

/// The chat folder's shells, for the turn's terminal tool — not another open
/// folder's (F-8.3).
fn terminals_of<R: Runtime>(app: &AppHandle<R>, workspace: &std::path::Path) -> Option<Arc<dyn crate::domain::terminal::UserTerminals>> {
    let terminals = app.try_state::<Arc<crate::infra::terminal::Terminals>>()?;
    Some(Arc::new(terminals.for_workspace(workspace)))
}

/// The MongoDB page's clients, as the agent's `mongo` tool reads through them.
pub(super) fn mongo_servers_of<R: Runtime>(app: &AppHandle<R>) -> Option<Arc<dyn crate::domain::mongo::MongoServers>> {
    let clients = app.try_state::<Arc<crate::infra::mongo_client::MongoClients>>()?;
    Some(Arc::new(crate::services::mongo::Servers(Arc::clone(&clients))))
}

/// Assembles a turn and runs it on a blocking thread.
///
/// The processes and helpers a turn of `chat_id` starts through: recorded as
/// that chat's, for the Processes and Agents tabs to list the open chat's,
/// and the processes as `workspace`'s — the only ones the turn sees (F-8.3).
fn chats_own(
    processes: Option<Arc<Processes>>,
    agents: Option<Arc<crate::domain::agents::Agents>>,
    chat_id: &str,
    workspace: &std::path::Path,
) -> (Option<Arc<dyn crate::domain::background::BackgroundProcesses>>, Option<Arc<crate::domain::agents::Agents>>) {
    (
        processes.map(|processes| {
            Arc::new(processes.for_chat(chat_id, workspace)) as Arc<dyn crate::domain::background::BackgroundProcesses>
        }),
        agents.map(|agents| Arc::new(agents.for_chat(chat_id))),
    )
}

/// How many turns may run at once, from the settings. An unreadable settings
/// file costs the user's chosen limit, not the turn — the default stands in.
pub(crate) fn parallel_limit() -> u32 {
    crate::infra::settings_store::load()
        .map(|settings| settings.llm.turn_limits.parallel)
        .unwrap_or_else(|_| crate::domain::settings::TurnLimits::default().parallel)
}

/// Off the event loop because the whole turn is synchronous — provider calls,
/// tool calls and a process runner — and holding the IPC loop for minutes
/// freezes every other command, including the one that stops it. Everything
/// the running turn is reached through is the chat's own `session`, so a turn
/// of another chat running alongside neither stops it nor reads its notes.
///
/// `running` is the admission the caller took before touching the session;
/// the guard lives here until the turn ends and clears the mark on any way
/// out — finished, paused, stopped, refused or lost to a panicked thread.
async fn run_off_the_event_loop<R, F>(
    app: AppHandle<R>,
    state: Arc<AgentState>,
    session: Arc<SessionHandle>,
    running: RunningTurn,
    turn_id: String,
    chat_id: String,
    workspace: PathBuf,
    worktree_base: Option<String>,
    plan: Option<String>,
    run: F,
) -> Result<ChatStreamOutcome, ChatTurnError>
where
    R: Runtime,
    F: FnOnce(&Turn) -> Result<ChatStreamOutcome, TurnError> + Send + 'static,
{
    let approval = state.approval(&workspace).map_err(ChatTurnError::Message)?;
    let mode = state.mode();
    let _running = running;
    let (processes, agents) = chats_own(
        app.try_state::<Arc<Processes>>().map(|processes| Arc::clone(&processes)),
        app.try_state::<Arc<crate::domain::agents::Agents>>().map(|agents| Arc::clone(&agents)),
        &chat_id,
        &workspace,
    );
    let handles = FolderHandles {
        // On the turn's thread, below: it may read a store.
        search: None,
        embeddings: app.try_state::<Arc<WorkspaceIndex>>().map(|index| index.embeddings()),
        mcp_servers: app.try_state::<Arc<McpServers>>().map(|servers| Arc::clone(&servers)),
        processes,
        agents,
        questions: app.try_state::<Arc<McpQuestions>>().map(|questions| Arc::clone(&questions)),
        terminals: terminals_of(&app, &workspace),
        board: app
            .try_state::<Arc<crate::infra::board_store::Board>>()
            .map(|board| Arc::clone(&board) as Arc<dyn crate::domain::board::TaskBoard>),
        mongo: mongo_servers_of(&app),
        pull_requests: app
            .try_state::<Arc<crate::services::pull_requests::PullRequests>>()
            .map(|prs| Arc::clone(&prs) as Arc<dyn crate::domain::code_host::AgentPullRequests>),
        markers: Some(Arc::new(ChatMarkers {
            session: Arc::clone(&session),
            chat_id: chat_id.clone(),
            turn_id: turn_id.clone(),
        })),
        // Built on the turn's thread below: its count is read from the store.
        journal: None,
    };

    tauri::async_runtime::spawn_blocking(move || {
        // Built here, off the event loop: its count is read from the store.
        let journal: Option<Arc<dyn crate::domain::turn_journal::TurnJournal>> =
            ChatJournal::new(chat_id.clone(), turn_id.clone()).map(|j| Arc::new(j) as Arc<dyn crate::domain::turn_journal::TurnJournal>);
        let handles = FolderHandles { search: searcher_of(&app, &workspace), journal, ..handles };
        let emit = chat_event_sink(&app, turn_id.clone());
        let observed = Arc::clone(&session);
        let events: ChatEventSink = Arc::new(move |event: ChatTurnEvent| {
            observed.observe(&event.event);
            emit(event)
        });
        let cancelled = || session.cancel.load(Ordering::SeqCst);
        let take_steering = || session.steering.take();
        let notes_typed = || session.steering.typed();
        let parallel = || state.parallel_chats(&chat_id, &workspace);
        let folder = FolderTurn {
            chat_id: &chat_id,
            turn_id: &turn_id,
            workspace: &workspace,
            mode,
            approval,
            plan: plan.as_deref(),
            events: &events,
            cancelled: &cancelled,
            take_steering: &take_steering,
            notes_typed: &notes_typed,
            parallel: &parallel,
            worktree_base: worktree_base.as_deref(),
        };
        let outcome = turn_setup::run_in_folder(folder, handles, run).map_err(|e| match e {
            // The one failure that keeps the turn's work: the window saves
            // this history, checklist and plan as it saves a finished turn.
            TurnSetupError::Turn(TurnError::Failed { error, history, todos, plan }) => {
                ChatTurnError::Failed(TurnFailure {
                    message: error.to_string(),
                    recovery: Some(TurnRecovery { history, todos, plan }),
                })
            }
            other => ChatTurnError::Message(other.to_string()),
        })?;
        session.paused.store(matches!(&outcome, ChatStreamOutcome::PendingApproval(_)), Ordering::SeqCst);
        session.end_turn(turn_id, &outcome);
        Ok(outcome)
    })
    .await
    .map_err(|e| ChatTurnError::Message(format!("the turn thread failed: {e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A saved chat whose folder is not open does not run (F-8.3): a turn
    /// there would start processes nothing could stop or list. Refused after
    /// admission, so the mark is cleared again.
    #[test]
    fn a_turn_in_a_folder_not_open_is_refused() {
        crate::testing::with_app_dir("cmd-chat-closed-folder", || {
            let closed = crate::infra::paths::canonicalize_plain(&temp_dir("cmd-chat-closed")).unwrap();
            crate::infra::chat_store::save("c1", &closed.display().to_string(), &[LlmMessage::user("hi")], &serde_json::json!([]), &[], None, None, None)
                .unwrap();
            let state = state();
            state.set_workspace_for_test(temp_dir("cmd-chat-closed-active"));
            let app = tauri::test::mock_app();
            app.manage(Arc::clone(&state));

            let started = tauri::async_runtime::block_on(chat_start(
                app.handle().clone(),
                app.state(),
                "t1".into(),
                "c1".into(),
                vec![LlmMessage::user("hi")],
                Vec::new(),
                None,
                None,
            ));
            assert!(matches!(&started, Err(e) if e.message().contains("not an open folder")), "{started:?}");
            let checkpoint = PendingApproval {
                history: vec![LlmMessage::user("hi")],
                round: 1,
                budget_used: 0,
                event_seq: 0,
                calls: Vec::new(),
                todos: Vec::new(),
                reads: Default::default(),
            };
            let resumed = tauri::async_runtime::block_on(chat_resume(
                app.handle().clone(),
                app.state(),
                "t2".into(),
                "c1".into(),
                checkpoint,
                Vec::new(),
                None,
            ));
            assert!(matches!(&resumed, Err(e) if e.message().contains("not an open folder")), "{resumed:?}");
            assert!(state.root_is_idle(&closed), "the refused turn left its mark");
        });
    }

    /// A turn reads its own folder's shells, not a neighbour's (F-8.3).
    #[cfg(unix)]
    #[test]
    fn a_turn_reads_its_folders_shells_only() {
        let app = tauri::test::mock_app();
        let terminals = Arc::new(crate::infra::terminal::Terminals::with_shell("/bin/sh"));
        tauri::Manager::manage(&app, Arc::clone(&terminals));
        let (mine, theirs) = (temp_dir("cmd-shells-mine"), temp_dir("cmd-shells-theirs"));
        let size = crate::domain::terminal::TerminalSize::new(80, 24).unwrap();
        let own = terminals.open(&mine, size).unwrap();
        terminals.open(&theirs, size).unwrap();

        let seen = terminals_of(app.handle(), &mine).unwrap().list();

        assert_eq!(seen.iter().map(|t| t.id).collect::<Vec<_>>(), [own.id]);
        terminals.close_all();
    }

    /// A turn's starts are its chat's, so the tabs list them under it.
    #[cfg(unix)]
    #[test]
    fn what_a_turn_starts_is_its_chats() {
        let (processes, agents) = (Arc::new(Processes::default()), Arc::new(crate::domain::agents::Agents::default()));
        let dir = crate::testing::temp_dir("cmd-chat-owned");
        let (theirs, helpers) = chats_own(Some(Arc::clone(&processes)), Some(Arc::clone(&agents)), "c1", &dir);
        theirs.unwrap().start(&crate::domain::command_exec::Shell::default(), "sleep 30", None, &dir, ".").unwrap();
        helpers.unwrap().start("where is X");
        assert_eq!((processes.list_for("c1").len(), agents.list_for("c1").len()), (1, 1));
        assert_eq!(processes.list_in(&dir).len(), 1, "and its folder's");
        processes.stop_all();
    }
    
    use crate::testing::temp_dir;
    

    /// CM-2: the meter measures the chat's own provider — its context window,
    /// not the active provider's.
    #[test]
    fn the_meter_measures_the_chats_own_provider() {
        crate::testing::with_app_dir("cmd-chat-meter-pin", || {
            use crate::domain::settings::{AppSettings, LlmSettings, ProviderConfig};
            let provider = |id: &str, limit: u32| ProviderConfig {
                id: id.into(),
                base_url: format!("https://{id}.example/v1"),
                model: Some("m".into()),
                context_limit: Some(limit),
                ..Default::default()
            };
            crate::infra::settings_store::save(&AppSettings {
                llm: LlmSettings {
                    active_provider_id: Some("b".into()),
                    providers: vec![provider("a", 111_111), provider("b", 222_222)],
                    ..Default::default()
                },
                ..Default::default()
            })
            .unwrap();
            crate::infra::llm_credentials_store::save_api_key("a", "sk-a").unwrap();
            let root = crate::infra::paths::canonicalize_plain(&temp_dir("cmd-chat-meter-pin-root")).unwrap();
            crate::infra::chat_store::save("c1", &root.display().to_string(), &[LlmMessage::user("hi")], &serde_json::json!([]), &[], None, None, None).unwrap();
            crate::infra::chat_store::set_model("c1", &crate::domain::chat_record::ChatModel { provider: "a".into(), model: Some("m".into()) }).unwrap();
            let state = state();
            state.register_open(root);
            let app = tauri::test::mock_app();
            app.manage(state);

            let usage = tauri::async_runtime::block_on(chat_context_usage(
                vec![LlmMessage::user("hi")], "c1".into(), app.handle().clone(), app.state(),
            ))
            .unwrap();

            assert_eq!(usage.limit, Some(111_111), "the chat's provider's window, not the active one's");
        });
    }

    /// So is a compaction's summary, "Compact now" or before a message.
    #[test]
    fn a_compaction_counts_under_its_chat() {
        crate::testing::with_app_dir("chat-compact-spent", || {
            crate::testing::spending_provider("the earlier part", 40, 6);
            let app = tauri::test::mock_app();
            app.manage(state());
            // Past the default window, so there is an older part to fold.
            let history = (0..40).map(|i| if i % 2 == 0 { LlmMessage::user(format!("question {i} {}", "x".repeat(20_000))) } else { LlmMessage::assistant(format!("answer {i}")) }).collect();
            let compacted = tauri::async_runtime::block_on(chat_compact(history, true, "t".into(), "c8".into(), app.handle().clone(), app.state()));
            assert!(compacted.expect("compacted").is_some());
            assert_eq!(crate::testing::chat_tokens("c8"), 46);
        });
    }

    /// The summary a rewind asks for is the chat's spending too.
    #[test]
    fn a_branch_summary_counts_under_its_chat() {
        crate::testing::with_app_dir("chat-branch-spent", || {
            crate::testing::spending_provider("tried the parser", 30, 4);
            let summary = tauri::async_runtime::block_on(chat_branch_summary(vec![LlmMessage::user("go")], "c9".into()));
            assert!(summary.expect("summarized").is_some());
            assert_eq!(crate::testing::chat_tokens("c9"), 34);
        });
    }

    /// The state is reachable without a running app, which is what keeps these
    /// checks about behaviour rather than about Tauri.
    fn state() -> Arc<AgentState> {
        Arc::new(AgentState::default())
    }

    #[test]
    fn a_background_chat_saves_under_its_own_checkout() {
        crate::testing::with_app_dir("cmd-chat-save-own-root", || {
            let own = crate::infra::paths::canonicalize_plain(&temp_dir("cmd-chat-save-own")).unwrap();
            let other = crate::infra::paths::canonicalize_plain(&temp_dir("cmd-chat-save-other")).unwrap();
            crate::infra::chat_store::save("c1", &own.display().to_string(), &[LlmMessage::user("hi")], &serde_json::json!([]), &[], None, None, None).unwrap();
            let state = state();
            state.set_workspace_for_test(other);
            let app = tauri::test::mock_app();
            app.manage(state);

            tauri::async_runtime::block_on(super::super::chat_history::chat_save(
                app.state(),
                "c1".into(),
                vec![LlmMessage::user("hi"), LlmMessage::assistant("done")],
                serde_json::json!([]),
                Vec::new(),
                None,
                None,
                None,
                None,
            ))
            .unwrap();
            assert_eq!(crate::infra::chat_store::load("c1").unwrap().workspace, own.display().to_string());
        });
    }

    #[test]
    fn preview_and_resume_read_the_saved_chats_checkout() {
        crate::testing::with_app_dir("cmd-chat-resume-own-root", || {
            crate::testing::spending_provider("done", 10, 2);
            let own = crate::infra::paths::canonicalize_plain(&temp_dir("cmd-chat-resume-own")).unwrap();
            let other = crate::infra::paths::canonicalize_plain(&temp_dir("cmd-chat-resume-other")).unwrap();
            std::fs::write(own.join("a.txt"), "own\n").unwrap();
            std::fs::write(other.join("a.txt"), "other\n").unwrap();
            crate::infra::chat_store::save("c1", &own.display().to_string(), &[LlmMessage::user("hi")], &serde_json::json!([]), &[], None, None, None).unwrap();
            let state = state();
            // Open beside the active folder, as every chat's folder is (F-8).
            state.register_open(own.clone());
            state.set_workspace_for_test(other);
            let app = tauri::test::mock_app();
            app.manage(state);

            let call = PendingToolCall {
                id: "w1".into(),
                name: "writeFile".into(),
                arguments: r#"{"path":"a.txt","content":"after\n"}"#.into(),
                requires_confirmation: true,
                reason: None,
            };
            let shown = tauri::async_runtime::block_on(chat_preview("c1".into(), vec![call], app.state())).unwrap();
            let ToolPreview::Diff { diff, .. } = &shown[0] else { panic!("a write has a diff: {shown:?}") };
            assert!(diff.unified_diff.contains("own"), "{}", diff.unified_diff);
            assert!(!diff.unified_diff.contains("other"), "{}", diff.unified_diff);

            let read = LlmToolCall { id: "w1".into(), name: "readFile".into(), arguments: r#"{"path":"a.txt"}"#.into() };
            let checkpoint = PendingApproval {
                history: vec![LlmMessage::user("hi"), LlmMessage::tool_requests(vec![read])],
                round: 1,
                budget_used: 0,
                event_seq: 0,
                calls: vec![PendingToolCall { id: "w1".into(), name: "readFile".into(), arguments: r#"{"path":"a.txt"}"#.into(), requires_confirmation: true, reason: None }],
                todos: Vec::new(),
                reads: Default::default(),
            };
            let outcome = tauri::async_runtime::block_on(chat_resume(
                app.handle().clone(),
                app.state(),
                "t1".into(),
                "c1".into(),
                checkpoint,
                vec![ToolCallDecision { id: "w1".into(), approved: true, reason: None }],
                None,
            ));
            let Ok(ChatStreamOutcome::Done(done)) = outcome else { panic!("resume failed: {outcome:?}") };
            let result = done.history.iter().find(|message| message.tool_call_id.as_deref() == Some("w1")).unwrap();
            let content = result.content.as_deref().unwrap();
            assert!(content.contains("own\n"), "{content}");
            assert!(!content.contains("other"), "{content}");
        });
    }

    #[test]
    fn compaction_measures_the_saved_chats_prompt_not_the_open_folders() {
        crate::testing::with_app_dir("cmd-chat-compact-own-root", || {
            crate::testing::spending_provider("the earlier part", 10, 2);
            let own = crate::infra::paths::canonicalize_plain(&temp_dir("cmd-chat-compact-own")).unwrap();
            let other = crate::infra::paths::canonicalize_plain(&temp_dir("cmd-chat-compact-other")).unwrap();
            std::fs::write(own.join("AGENTS.md"), "x".repeat(1_000_000)).unwrap();
            crate::infra::chat_store::save("c1", &own.display().to_string(), &[LlmMessage::user("hi")], &serde_json::json!([]), &[], None, None, None).unwrap();
            let state = state();
            state.set_workspace_for_test(other.clone());
            let app = tauri::test::mock_app();
            let other_frame = next_request_frame(app.handle(), &state, Some(other)).0;
            let own_frame = next_request_frame(app.handle(), &state, Some(own)).0;
            app.manage(state);
            let mut history: Vec<_> = (0..40).map(|i| if i % 2 == 0 { LlmMessage::user(format!("question {i}")) } else { LlmMessage::assistant(format!("answer {i}")) }).collect();
            // Just under the threshold without the saved folder's rules, just
            // over with them: only the chat's own frame can trigger this pass.
            let limit = Some(crate::domain::settings::DEFAULT_CONTEXT_LIMIT);
            let baseline = ContextUsage::new(other_frame, &history, limit);
            let threshold = baseline.compacts_at.unwrap();
            history[0] = LlmMessage::user("x".repeat((threshold - baseline.total + 1) * 4));
            assert!(ContextUsage::new(other_frame, &history, limit).total < threshold);
            let own_usage = ContextUsage::new(own_frame, &history, limit);
            assert!(own_usage.total >= threshold);

            let compacted = tauri::async_runtime::block_on(chat_compact(history, false, "t1".into(), "c1".into(), app.handle().clone(), app.state()));
            assert!(compacted.expect("the saved folder's large frame compacts").is_some());
        });
    }

    /// The commands route by chat id: a stop, a note and its withdrawal land
    /// on that chat's handle and no other.
    #[test]
    fn the_commands_reach_their_own_chats_handle() {
        let state = state();
        let app = tauri::test::mock_app();
        app.manage(Arc::clone(&state));

        state.session("c1").paused.store(true, Ordering::SeqCst);
        chat_cancel("c1".into(), app.state());
        assert!(state.session("c1").cancel.load(Ordering::SeqCst));
        assert!(!state.session("c1").paused.load(Ordering::SeqCst), "stop releases a chat whose card was lost");
        assert!(!state.session("c2").cancel.load(Ordering::SeqCst), "the other chat's turn runs on");

        let id = chat_steer("use the helper".into(), None, None, "c2".into(), app.state());
        assert!(state.session("c1").steering.take().is_empty(), "not the other chat's note");
        assert!(!chat_cancel_steer(id.clone(), "c1".into(), app.state()), "the note is not the one chat's");
        assert!(chat_cancel_steer(id, "c2".into(), app.state()));
        assert!(state.session("c2").steering.take().is_empty());
    }

    /// A picture attached to the note rides on the note itself: the round that
    /// takes it sends both, and the bubble draws what was attached.
    #[test]
    fn a_note_carries_the_pictures_attached_to_it() {
        let state = state();
        let app = tauri::test::mock_app();
        app.manage(Arc::clone(&state));
        let picture = ImagePart {
            media_type: crate::domain::image::ImageMediaType::Png,
            data: "AA".into(),
            width: 3,
            height: 4,
        };

        chat_steer(String::new(), None, Some(vec![picture.clone()]), "c1".into(), app.state());
        let taken = state.session("c1").steering.take();

        assert_eq!(taken.len(), 1);
        assert_eq!(taken[0].images, vec![picture]);
        assert!(taken[0].text.is_empty(), "a picture alone is the whole note");
    }

    /// A stop left over from the chat's last turn is cleared when its fresh
    /// turn starts — the whole way from the command, or "stop while the
    /// approval card shows" would end the resumed turn's successor instead.
    #[test]
    fn a_fresh_turn_clears_its_chats_leftover_stop() {
        crate::testing::with_app_dir("cmd-chat-fresh-turn", || {
            crate::testing::spending_provider("hello", 10, 2);
            let state = state();
            state.set_workspace_for_test(crate::infra::paths::canonicalize_plain(&temp_dir("cmd-chat-fresh")).unwrap());
            let app = tauri::test::mock_app();
            app.manage(Arc::clone(&state));
            state.session("c1").cancel.store(true, Ordering::SeqCst);

            let outcome = tauri::async_runtime::block_on(chat_start(
                app.handle().clone(),
                app.state(),
                "t1".into(),
                "c1".into(),
                vec![LlmMessage::user("hi")],
                Vec::new(),
                None,
                None,
            ));
            let Ok(ChatStreamOutcome::Done(done)) = outcome else { panic!("the leftover stop ended the fresh turn: {outcome:?}") };
            assert_eq!(done.result.text, "hello");
        });
    }

    /// F-8.7: a new chat whose turn started in A is A's while the window
    /// moves to B — its first save, its next turn and its context count are
    /// filed under A, while a chat that has not started yet goes to B.
    #[test]
    fn an_unsaved_chat_stays_in_the_folder_its_turn_started_in() {
        crate::testing::with_app_dir("cmd-chat-home", || {
            crate::testing::spending_provider("hello", 10, 2);
            let state = state();
            let plain = |label| crate::infra::paths::canonicalize_plain(&temp_dir(label)).unwrap();
            let (a, b) = (plain("cmd-chat-home-a"), plain("cmd-chat-home-b"));
            state.set_workspace_for_test(a.clone());
            let app = tauri::test::mock_app();
            app.manage(Arc::clone(&state));
            let start = |chat: &str| {
                tauri::async_runtime::block_on(chat_start(
                    app.handle().clone(),
                    app.state(),
                    "t1".into(),
                    chat.into(),
                    vec![LlmMessage::user("hi")],
                    Vec::new(),
                    None,
                    None,
                ))
            };
            assert!(matches!(start("c1"), Ok(ChatStreamOutcome::Done(_))));

            state.set_workspace_for_test(b.clone());
            assert_eq!(state.chat_workspace("c1").unwrap().0, a, "the started chat stays in A");
            assert_eq!(state.chat_prompt_workspace("c1").unwrap(), Some(a.clone()));
            assert_eq!(state.chat_workspace("c2").unwrap().0, b, "one not started yet is the active folder's");
            assert!(matches!(start("c1"), Ok(ChatStreamOutcome::Done(_))), "its next turn runs in A");
            assert_eq!(state.chat_workspace("c1").unwrap().0, a);

            tauri::async_runtime::block_on(crate::commands::chat_history::chat_save(
                app.state(),
                "c1".into(),
                vec![LlmMessage::user("hi")],
                serde_json::json!([]),
                Vec::new(),
                None,
                None,
                None,
                None,
            ))
            .unwrap();
            assert_eq!(crate::infra::chat_store::load("c1").unwrap().workspace, a.display().to_string(), "filed under A");
        });
    }

    /// A new chat's message typed in A while the backend already made B
    /// active (an open of B on its way): saved and run in A, the folder the
    /// window showed it in. A folder that closed meanwhile is refused.
    #[test]
    fn a_new_chat_goes_to_the_folder_it_was_typed_in() {
        crate::testing::with_app_dir("cmd-chat-typed-in", || {
            crate::testing::spending_provider("hello", 10, 2);
            let state = state();
            let plain = |label| crate::infra::paths::canonicalize_plain(&temp_dir(label)).unwrap();
            let (a, b) = (plain("cmd-chat-typed-in-a"), plain("cmd-chat-typed-in-b"));
            state.set_workspace_for_test(a.clone());
            state.set_workspace_for_test(b.clone());
            let app = tauri::test::mock_app();
            app.manage(Arc::clone(&state));
            let typed_in = Some(a.display().to_string());

            tauri::async_runtime::block_on(crate::commands::chat_history::chat_save(
                app.state(),
                "n1".into(),
                vec![LlmMessage::user("hi")],
                serde_json::json!([]),
                Vec::new(),
                None,
                None,
                typed_in.clone(),
                None,
            ))
            .unwrap();
            assert_eq!(crate::infra::chat_store::load("n1").unwrap().workspace, a.display().to_string(), "saved under A");

            let start = |chat: &str, folder: Option<String>| {
                tauri::async_runtime::block_on(chat_start(
                    app.handle().clone(),
                    app.state(),
                    "t1".into(),
                    chat.into(),
                    vec![LlmMessage::user("hi")],
                    Vec::new(),
                    None,
                    folder,
                ))
            };
            assert!(matches!(start("n2", typed_in.clone()), Ok(ChatStreamOutcome::Done(_))));
            assert_eq!(state.chat_workspace("n2").unwrap().0, a, "its turn ran in A");
            let shown_elsewhere = b.display().to_string();
            assert_eq!(state.chat_workspace_in("n2", Some(&shown_elsewhere)).unwrap().0, a, "a started chat keeps its folder");

            assert!(state.unregister_if_idle(&a));
            let refused = start("n3", typed_in).expect_err("A is closed");
            assert!(refused.message().contains("not an open folder"), "{refused:?}");
        });
    }

    /// An unsaved chat whose folder closed (its first save failed) is not
    /// stranded there: it goes on in the active folder.
    #[test]
    fn an_unsaved_chat_whose_folder_closed_goes_on_in_the_active_one() {
        crate::testing::with_app_dir("cmd-chat-home-closed", || {
            crate::testing::spending_provider("hello", 10, 2);
            let state = state();
            let plain = |label| crate::infra::paths::canonicalize_plain(&temp_dir(label)).unwrap();
            let (a, b) = (plain("cmd-chat-home-closed-a"), plain("cmd-chat-home-closed-b"));
            state.set_workspace_for_test(a.clone());
            let app = tauri::test::mock_app();
            app.manage(Arc::clone(&state));
            let outcome = tauri::async_runtime::block_on(chat_start(
                app.handle().clone(),
                app.state(),
                "t1".into(),
                "c1".into(),
                vec![LlmMessage::user("hi")],
                Vec::new(),
                None,
                None,
            ));
            assert!(matches!(outcome, Ok(ChatStreamOutcome::Done(_))));
            state.set_workspace_for_test(b.clone());
            assert!(state.unregister_if_idle(&a), "A closes once its turn is over");

            assert_eq!(state.chat_workspace("c1").unwrap().0, b);
            let typed_in = a.display().to_string();
            assert_eq!(state.chat_workspace_in("c1", Some(&typed_in)).unwrap().0, b, "the window's closed folder is not asked");
            assert_eq!(state.chat_prompt_workspace("c1").unwrap(), Some(b));
        });
    }

    /// Journaling asks the chat's own session: another chat's id finds
    /// nothing there, and the turn's own chat journals it once.
    #[test]
    fn the_journal_is_the_chats_own() {
        crate::testing::with_app_dir("cmd-chat-journal-own", || {
            crate::testing::spending_provider("hello", 10, 2);
            let state = state();
            state.set_workspace_for_test(crate::infra::paths::canonicalize_plain(&temp_dir("cmd-chat-journal")).unwrap());
            let app = tauri::test::mock_app();
            app.manage(Arc::clone(&state));
            let outcome = tauri::async_runtime::block_on(chat_start(
                app.handle().clone(),
                app.state(),
                "t1".into(),
                "c1".into(),
                vec![LlmMessage::user("hi")],
                Vec::new(),
                None,
                None,
            ));
            assert!(matches!(outcome, Ok(ChatStreamOutcome::Done(_))), "{outcome:?}");

            let other = tauri::async_runtime::block_on(next_prompt_log("c2".into(), "t1".into(), "hi".into(), app.state()));
            assert!(matches!(other, Ok(None)), "another chat has nothing to journal: {other:?}");
            let own = tauri::async_runtime::block_on(next_prompt_log("c1".into(), "t1".into(), "hi".into(), app.state()));
            assert!(matches!(own, Ok(Some(_))), "the turn's own chat journals it: {own:?}");
        });
    }

    /// A resume deliberately does not clear the stop: pressed while the
    /// approval card was showing, it ends the resumed turn — and it is the
    /// resumed chat's own handle that says so.
    #[test]
    fn a_resumed_turn_reads_its_own_chats_stop() {
        crate::testing::with_app_dir("cmd-chat-resume-stop", || {
            crate::testing::spending_provider("hello", 10, 2);
            let state = state();
            state.set_workspace_for_test(crate::infra::paths::canonicalize_plain(&temp_dir("cmd-chat-resume")).unwrap());
            let app = tauri::test::mock_app();
            app.manage(Arc::clone(&state));
            let call = |id: &str| LlmToolCall { id: id.into(), name: "readFile".into(), arguments: "{\"path\":\"a.txt\"}".into() };
            let checkpoint = PendingApproval {
                history: vec![LlmMessage::user("hi"), LlmMessage::tool_requests(vec![call("w1")])],
                round: 1,
                budget_used: 0,
                event_seq: 0,
                calls: vec![PendingToolCall { id: "w1".into(), name: "readFile".into(), arguments: "{\"path\":\"a.txt\"}".into(), requires_confirmation: true, reason: None }],
                todos: Vec::new(),
                reads: Default::default(),
            };
            // Pressed while the card was showing; another chat's stop says
            // nothing here either way.
            state.session("c1").cancel.store(true, Ordering::SeqCst);

            let outcome = tauri::async_runtime::block_on(chat_resume(
                app.handle().clone(),
                app.state(),
                "t1".into(),
                "c1".into(),
                checkpoint,
                vec![ToolCallDecision { id: "w1".into(), approved: true, reason: None }],
                None,
            ));
            assert!(matches!(outcome, Ok(ChatStreamOutcome::Cancelled(_))), "{outcome:?}");
        });
    }

    /// TJ-4: the journal adapter counts from the store — a resume appends
    /// the tail from where the pause left it — and its writes land as rows.
    #[test]
    fn the_journal_adapter_counts_from_the_store_and_writes_rows() {
        use crate::domain::turn_journal::TurnJournal;
        crate::testing::with_app_dir("cmd-journal-adapter", || {
            let msg = |text: &str| serde_json::to_value(vec![LlmMessage::user(text)]).unwrap();
            crate::infra::chat_store::journal_append("c1", "t1", "snapshot", &msg("go"), 1).unwrap();
            crate::infra::chat_store::journal_append("c1", "t1", "append", &msg("a"), 1).unwrap();
            crate::infra::chat_store::journal_append("c1", "t1", "append", &msg("b"), 1).unwrap();

            let journal = ChatJournal::new("c1".into(), "t1".into()).expect("the store reads");
            assert_eq!(journal.len(), 3, "the pause left three messages journaled");
            journal.append(&[LlmMessage::user("next")]);
            assert_eq!(journal.len(), 4);
            assert_eq!(crate::infra::chat_store::journal_message_count("c1", "t1").unwrap(), 4);
            journal.snapshot(&[LlmMessage::user("folded")]);
            assert_eq!(journal.len(), 1);
            assert_eq!(crate::infra::chat_store::journal_message_count("c1", "t1").unwrap(), 1);
        });
    }

    /// TJ-3: the adapter keeps the fingerprint across turns — the next
    /// turn's first request compares with the last turn's last — and the
    /// row lands on disk under the chat and turn that wrote it.
    #[test]
    fn the_chat_markers_remember_across_turns_and_write_the_row() {
        use crate::domain::request_marker::{RequestFingerprint, RequestRecord, RequestMarkers};
        crate::testing::with_app_dir("cmd-markers", || {
            let session = Arc::new(SessionHandle::default());
            let fp = |system: &str| RequestFingerprint { system: system.into(), tools: "t".into(), messages: vec![] };
            let record = |system: &str| RequestRecord { round: 1, provider_id: "p".into(), model: "m".into(), usage: None, fingerprint: fp(system), cause: None };
            let first = ChatMarkers { session: Arc::clone(&session), chat_id: "c1".into(), turn_id: "c1/turn-1".into() };
            assert!(first.previous().is_none());
            first.record(record("s1"));
            // A new turn of the same chat compares with the previous turn's last.
            let second = ChatMarkers { session: Arc::clone(&session), chat_id: "c1".into(), turn_id: "c1/turn-2".into() };
            assert_eq!(second.previous(), Some(fp("s1")));
            second.record(record("s2"));
            assert_eq!(second.previous(), Some(fp("s2")));
            // Another chat's session remembers nothing of this one.
            let other = ChatMarkers { session: Arc::new(SessionHandle::default()), chat_id: "c2".into(), turn_id: "c2/turn-1".into() };
            assert!(other.previous().is_none());
            // CU-2: the anchor is the last request that reported its cost, and
            // it too survives the turn boundary; a request with no usage keeps it.
            assert!(second.last_anchor().is_none());
            let costed = RequestRecord {
                usage: Some(crate::domain::llm::ChatUsage { prompt_tokens: 500, completion_tokens: 2, total_tokens: 502, cached_tokens: 0 }),
                ..record("s3")
            };
            second.record(costed);
            let third = ChatMarkers { session: Arc::clone(&session), chat_id: "c1".into(), turn_id: "c1/turn-3".into() };
            assert_eq!(third.last_anchor().map(|a| a.prompt_tokens), Some(500));
            third.record(record("s4"));
            assert_eq!(third.last_anchor().map(|a| a.prompt_tokens), Some(500), "a request with no usage keeps the anchor");
            assert!(other.last_anchor().is_none());

            let conn = rusqlite::Connection::open(crate::infra::app_dir::ensure().unwrap().join("tool_calls.db")).unwrap();
            let rows: Vec<(String, String)> = conn
                .prepare("SELECT chat_id, turn_id FROM requests ORDER BY id")
                .unwrap()
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            assert_eq!(rows, vec![("c1".to_string(), "c1/turn-1".to_string()), ("c1".to_string(), "c1/turn-2".to_string()), ("c1".to_string(), "c1/turn-2".to_string()), ("c1".to_string(), "c1/turn-3".to_string())]);
        });
    }

    /// CU-3: the meter's anchor is the session's last costed request, and the
    /// marker row when the session has none — a reopen or a restart.
    #[test]
    fn the_meter_anchor_falls_back_to_the_marker_row() {
        use crate::domain::llm::ChatUsage;
        use crate::domain::request_marker::{RequestFingerprint, RequestRecord, RequestMarkers};
        crate::testing::with_app_dir("cmd-anchor-fallback", || {
            let state = state();
            assert!(chat_anchor(&state, "c1").is_none());
            let record = RequestRecord {
                round: 1,
                provider_id: "p".into(),
                model: "m".into(),
                usage: Some(ChatUsage { prompt_tokens: 500, completion_tokens: 0, total_tokens: 500, cached_tokens: 0 }),
                fingerprint: RequestFingerprint { system: "s".into(), tools: "t".into(), messages: vec![] },
                cause: None,
            };
            crate::infra::tool_call_log::append_request("c1", "t1", &record);
            assert_eq!(chat_anchor(&state, "c1").map(|a| a.prompt_tokens), Some(500));
            let markers = ChatMarkers { session: state.session("c1"), chat_id: "c1".into(), turn_id: "c1/turn-2".into() };
            markers.record(RequestRecord { usage: Some(ChatUsage { prompt_tokens: 700, ..record.usage.unwrap() }), ..record.clone() });
            assert_eq!(chat_anchor(&state, "c1").map(|a| a.prompt_tokens), Some(700), "the session's anchor wins over the row");
        });
    }

    /// TJ-2: a failed turn's work crosses the wire as an object — the
    /// message and the history to save — not as a bare string, which the
    /// window would show and drop.
    #[test]
    fn a_failed_turns_work_crosses_the_wire() {
        crate::testing::with_app_dir("cmd-turn-failed-wire", || {
            crate::testing::spending_provider("unused", 10, 2);
            let state = state();
            let folder = crate::infra::paths::canonicalize_plain(&temp_dir("cmd-turn-failed-wire")).unwrap();
            state.register_open(folder.clone());
            state.set_workspace_for_test(folder.clone());
            let app = tauri::test::mock_app();
            app.manage(Arc::clone(&state));
            let running = state.begin_running("c1", 3, &folder).expect("the turn runs");

            let outcome = tauri::async_runtime::block_on(run_off_the_event_loop(
                app.handle().clone(),
                Arc::clone(&state),
                state.session("c1"),
                running,
                "t1".into(),
                "c1".into(),
                folder.clone(),
                None,
                None,
                move |_| {
                    Err(TurnError::Failed {
                        error: crate::domain::llm::LlmError::Provider("peer closed connection".into()),
                        history: vec![LlmMessage::user("go")],
                        todos: Vec::new(),
                        plan: Some("the plan".into()),
                    })
                },
            ));

            let Err(err) = outcome else { panic!("expected a failure: {outcome:?}") };
            assert!(err.message().contains("peer closed connection"), "{err:?}");
            let wire = serde_json::to_value(&err).expect("serializes");
            assert_eq!(wire["recovery"]["history"][0]["content"], "go");
            assert_eq!(wire["recovery"]["plan"], "the plan");
            // A refusal with nothing to keep stays a plain string.
            let plain = ChatTurnError::Message("already running".into());
            assert_eq!(serde_json::to_value(&plain).unwrap(), serde_json::json!("already running"));
        });
    }

    /// F-8.5: a turn runs under the "always allow" of its own folder — not
    /// the active one's — and under the app's one Auto.
    #[test]
    fn a_turn_runs_under_its_folders_always_allow() {
        crate::testing::with_app_dir("cmd-turn-allowed", || {
            crate::testing::spending_provider("unused", 10, 2);
            let state = state();
            let plain = |label| crate::infra::paths::canonicalize_plain(&temp_dir(label)).unwrap();
            let (active, background) = (plain("cmd-turn-allowed-a"), plain("cmd-turn-allowed-b"));
            state.register_open(background.clone());
            state.set_workspace_for_test(active.clone());
            state.allow_always(&background, "writeFile").unwrap();
            state.allow_always(&active, "delete").unwrap();
            let app = tauri::test::mock_app();
            app.manage(Arc::clone(&state));
            let running = state.begin_running("c1", 3, &background).expect("the turn runs");
            let seen = Arc::new(Mutex::new(None));
            let record = Arc::clone(&seen);

            let outcome = tauri::async_runtime::block_on(run_off_the_event_loop(
                app.handle().clone(),
                Arc::clone(&state),
                state.session("c1"),
                running,
                "t1".into(),
                "c1".into(),
                background.clone(),
                None,
                None,
                move |turn| {
                    let asks = |tool| turn.approval.requires_approval(tool, true);
                    *record.lock().unwrap() = Some((asks(crate::domain::tools::ToolName::WriteFile), asks(crate::domain::tools::ToolName::Delete)));
                    Err(TurnError::BadResume("only looking".into()))
                },
            ));

            assert!(outcome.is_err());
            assert_eq!(*seen.lock().unwrap(), Some((false, true)), "B's answer lets writeFile through, A's does not reach B");
        });
    }

    #[test]
    fn a_paused_turn_refuses_a_fresh_start_until_it_is_resumed() {
        crate::testing::with_app_dir("cmd-paused-chat-turn", || {
            crate::testing::spending_provider("unused", 10, 2);
            let state = state();
            let root = crate::infra::paths::canonicalize_plain(&temp_dir("cmd-paused-chat-root")).unwrap();
            state.set_workspace_for_test(root.clone());
            let app = tauri::test::mock_app();
            app.manage(Arc::clone(&state));
            let session = state.session("c1");
            let running = state.begin_running("c1", 3, &root).expect("the first turn runs");
            let checkpoint = PendingApproval {
                history: vec![LlmMessage::user("go")],
                round: 1,
                budget_used: 0,
                event_seq: 0,
                calls: Vec::new(),
                todos: Vec::new(),
                reads: Default::default(),
            };
            let outcome = tauri::async_runtime::block_on(run_off_the_event_loop(
                app.handle().clone(),
                Arc::clone(&state),
                session,
                running,
                "t1".into(),
                "c1".into(),
                root.clone(),
                None,
                None,
                move |_| Ok(ChatStreamOutcome::PendingApproval(checkpoint)),
            ));
            assert!(matches!(outcome, Ok(ChatStreamOutcome::PendingApproval(_))), "{outcome:?}");

            let refused = state.begin_running("c1", 3, &root).expect_err("a fresh start cannot orphan the card");
            assert!(refused.contains("waiting for approval"), "{refused}");
            // A resume refused because the folder was closed leaves the card
            // waiting, not resumable by nothing.
            state.register_open(root.clone());
            assert!(state.unregister_if_idle(&root), "a card does not hold its folder open");
            let refused = tauri::async_runtime::block_on(chat_resume(
                app.handle().clone(),
                app.state(),
                "t2".into(),
                "c1".into(),
                PendingApproval {
                    history: vec![LlmMessage::user("go")],
                    round: 1,
                    budget_used: 0,
                    event_seq: 0,
                    calls: Vec::new(),
                    todos: Vec::new(),
                    reads: Default::default(),
                },
                Vec::new(),
                None,
            ))
            .expect_err("a closed folder resumes nothing");
            assert!(refused.message().contains("not an open folder"), "{refused:?}");
            let still = state.begin_running("c1", 3, &root).expect_err("the card still waits");
            assert!(still.contains("waiting for approval"), "{still}");
            drop(state.begin_resume("c1", 3, &root).expect("the card resumes its own turn"));
            state.begin_running("c1", 3, &root).expect("after the resume ends, a fresh turn may start");
        });
    }

    /// A refused start is only a refusal: the running turn's stop flag — a
    /// Stop already pressed, waiting for the next checkpoint — survives it.
    #[test]
    fn a_refused_start_leaves_the_running_turn_alone() {
        crate::testing::with_app_dir("cmd-refused-keeps-stop", || {
            let provider = HeldProvider::start();
            provider.install();
            let state = state();
            state.set_workspace_for_test(crate::infra::paths::canonicalize_plain(&temp_dir("cmd-refused-keeps-stop-ws")).unwrap());
            let app = tauri::test::mock_app();
            app.manage(Arc::clone(&state));

            let first_handle = app.handle().clone();
            std::thread::scope(|scope| {
                let first = scope.spawn(move || start_turn(&first_handle, "c1", "t1", "go"));
                provider.wait_request();

                state.session("c1").cancel.store(true, Ordering::SeqCst);
                let second_handle = app.handle().clone();
                let refused = expect_refusal(move || start_turn(&second_handle, "c1", "t2", "go"));
                assert!(refused.contains("already running"), "{refused}");
                assert!(state.session("c1").cancel.load(Ordering::SeqCst), "the refused start kept the stop");

                provider.open();
                // Done or Cancelled — either way the refusal kept the stop.
                assert!(first.join().expect("the first turn's thread").is_ok());
            });
        });
    }

    /// A provider that holds every answer until `open` is called and says
    /// through `arrived` as each request comes in — what a test keeps a turn
    /// mid-flight with. The server itself is `gated_server` beside the
    /// providers: network code lives there (`data_policy`).
    struct HeldProvider {
        port: u16,
        arrived: std::sync::mpsc::Receiver<()>,
        gate: Arc<(Mutex<bool>, std::sync::Condvar)>,
    }

    impl HeldProvider {
        fn start() -> Self {
            let (arrived_tx, arrived) = std::sync::mpsc::channel::<()>();
            let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
            let port = crate::infra::llm_providers::gated_server(
                crate::testing::spending_reply("hello", 10, 2),
                Arc::clone(&gate),
                arrived_tx,
            );
            Self { port, arrived, gate }
        }

        /// The settings' only provider is this server, key included.
        fn install(&self) {
            crate::testing::spending_provider_on(self.port);
        }

        /// One request has reached the provider — the turn is mid-flight.
        fn wait_request(&self) {
            self.arrived
                .recv_timeout(std::time::Duration::from_secs(10))
                .expect("the turn never asked the provider");
        }

        fn open(&self) {
            let (lock, cvar) = &*self.gate;
            *lock.lock().unwrap() = true;
            cvar.notify_all();
        }
    }

    /// The refusal F-8 kept is gone: two chats' turns run at once, each to
    /// its own outcome, each counted under its own chat.
    #[test]
    fn two_chats_turns_run_at_once() {
        crate::testing::with_app_dir("cmd-two-turns", || {
            let provider = HeldProvider::start();
            provider.install();
            let state = state();
            state.set_workspace_for_test(crate::infra::paths::canonicalize_plain(&temp_dir("cmd-two-turns-ws")).unwrap());
            let app = tauri::test::mock_app();
            app.manage(Arc::clone(&state));

            // The mock App itself does not cross threads; its handles do.
            let first_handle = app.handle().clone();
            let second_handle = app.handle().clone();
            std::thread::scope(|scope| {
                let first = scope.spawn(move || start_turn(&first_handle, "c1", "t1", "one"));
                let second = scope.spawn(move || start_turn(&second_handle, "c2", "t2", "two"));
                // Both requests in before either is answered: truly concurrent.
                provider.wait_request();
                provider.wait_request();
                provider.open();

                let one = first.join().expect("the first turn's thread");
                let two = second.join().expect("the second turn's thread");
                assert!(matches!(one, Ok(ChatStreamOutcome::Done(_))), "{one:?}");
                assert!(matches!(two, Ok(ChatStreamOutcome::Done(_))), "{two:?}");
            });
            assert_eq!(crate::testing::chat_tokens("c1"), 12);
            assert_eq!(crate::testing::chat_tokens("c2"), 12);
        });
    }

    /// `chat_start` through a handle of the test's mock app — the shape the
    /// concurrency tests need, a handle per thread.
    fn start_turn(
        app: &tauri::AppHandle<tauri::test::MockRuntime>,
        chat: &str,
        turn: &str,
        said: &str,
    ) -> Result<ChatStreamOutcome, String> {
        tauri::async_runtime::block_on(chat_start(
            app.clone(),
            app.state(),
            turn.into(),
            chat.into(),
            vec![LlmMessage::user(said)],
            Vec::new(),
            None,
            None,
        ))
        .map_err(|e| e.message().to_string())
    }

    /// A start — or a resume — that is expected to refuse, waited on with a
    /// timeout. A start that unexpectedly runs would block on the provider's
    /// closed gate and hang the test with it, so the refusal has to arrive
    /// in time. The side thread is detached: only a mutant ever strands it.
    fn expect_refusal<F>(run: F) -> String
    where
        F: FnOnce() -> Result<ChatStreamOutcome, String> + Send + 'static,
    {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(run());
        });
        match rx.recv_timeout(std::time::Duration::from_secs(10)) {
            Ok(Err(refusal)) => refusal,
            Ok(Ok(outcome)) => panic!("expected a refusal, the turn ran: {outcome:?}"),
            Err(_) => panic!("no refusal within 10 s — the start is running, not refusing"),
        }
    }

    /// A second start of the same chat — or a resume of it — refuses while
    /// its turn runs, and runs once that turn has finished: the running mark
    /// goes with the turn, not with the chat.
    #[test]
    fn a_second_turn_of_the_same_chat_waits_for_the_first() {
        crate::testing::with_app_dir("cmd-same-chat-turn", || {
            let provider = HeldProvider::start();
            provider.install();
            let state = state();
            state.set_workspace_for_test(crate::infra::paths::canonicalize_plain(&temp_dir("cmd-same-chat-ws")).unwrap());
            let app = tauri::test::mock_app();
            app.manage(Arc::clone(&state));

            let first_handle = app.handle().clone();
            std::thread::scope(|scope| {
                let first = scope.spawn(move || start_turn(&first_handle, "c1", "t1", "go"));
                provider.wait_request();

                let second_handle = app.handle().clone();
                let refused = expect_refusal(move || start_turn(&second_handle, "c1", "t2", "go"));
                assert!(refused.contains("already running"), "{refused}");

                let checkpoint = PendingApproval {
                    history: vec![
                        LlmMessage::user("go"),
                        LlmMessage::tool_requests(vec![LlmToolCall { id: "w1".into(), name: "readFile".into(), arguments: "{}".into() }]),
                    ],
                    round: 1,
                    budget_used: 0,
                    event_seq: 0,
                    calls: vec![PendingToolCall { id: "w1".into(), name: "readFile".into(), arguments: "{}".into(), requires_confirmation: true, reason: None }],
                    todos: Vec::new(),
                    reads: Default::default(),
                };
                // The command takes a `State`, which does not cross threads;
                // its body is this, with the Arc it clones from the State.
                let resume_handle = app.handle().clone();
                let resume_state = Arc::clone(&state);
                let resumed = expect_refusal(move || {
                    let workspace = resume_state.workspace()?;
                    let session = resume_state.session("c1");
                    let running = resume_state.begin_running("c1", parallel_limit(), &workspace)?;
                    tauri::async_runtime::block_on(run_off_the_event_loop(
                        resume_handle,
                        resume_state,
                        session,
                        running,
                        "t1".into(),
                        "c1".into(),
                        workspace,
                        None,
                        None,
                        move |turn| llm_chat::resume(turn, checkpoint, vec![ToolCallDecision { id: "w1".into(), approved: true, reason: None }]),
                    ))
                    .map_err(|e| e.message().to_string())
                });
                assert!(resumed.contains("already running"), "{resumed}");

                provider.open();
                assert!(matches!(first.join().expect("the first turn's thread"), Ok(ChatStreamOutcome::Done(_))));

                let again = start_turn(app.handle(), "c1", "t3", "go");
                assert!(matches!(again, Ok(ChatStreamOutcome::Done(_))), "finished, the chat starts again: {again:?}");
            });
        });
    }

    /// Past the limit a start refuses with why, and the limit is the
    /// settings', not a constant.
    #[test]
    fn the_parallel_limit_refuses_the_extra_turn() {
        crate::testing::with_app_dir("cmd-parallel-limit", || {
            let provider = HeldProvider::start();
            provider.install();
            crate::infra::settings_store::update(|s| s.llm.turn_limits.parallel = 1).expect("settings");
            let state = state();
            state.set_workspace_for_test(crate::infra::paths::canonicalize_plain(&temp_dir("cmd-parallel-limit-ws")).unwrap());
            let app = tauri::test::mock_app();
            app.manage(Arc::clone(&state));

            let first_handle = app.handle().clone();
            std::thread::scope(|scope| {
                let first = scope.spawn(move || start_turn(&first_handle, "c1", "t1", "go"));
                provider.wait_request();

                let second_handle = app.handle().clone();
                let refused = expect_refusal(move || start_turn(&second_handle, "c2", "t2", "go"));
                assert!(refused.contains("limit"), "{refused}");

                provider.open();
                assert!(matches!(first.join().expect("the first turn's thread"), Ok(ChatStreamOutcome::Done(_))));
            });
        });
    }

    /// A chat is saved with the branch its folder has out at that moment —
    /// read at the save, so a switch the turn made is the one kept.
    #[test]
    fn a_saved_chat_keeps_the_branch_its_folder_has_out() {
        crate::testing::with_app_dir("cmd-chat-git-branch", || {
            let root = crate::infra::paths::canonicalize_plain(&temp_dir("cmd-chat-git-branch-repo")).unwrap();
            git2::Repository::init(&root).unwrap().set_head("refs/heads/feature").unwrap();
            let state = state();
            state.set_workspace_for_test(root.clone());
            let app = tauri::test::mock_app();
            app.manage(state);

            let saved = tauri::async_runtime::block_on(super::super::chat_history::chat_save(
                app.state(),
                "c1".into(),
                vec![LlmMessage::user("hi")],
                serde_json::json!([]),
                Vec::new(),
                None,
                None,
                None,
                None,
            ))
            .unwrap();
            assert_eq!(saved.git_branch.as_deref(), Some("feature"));
            let listed = crate::infra::chat_store::list(&root.display().to_string()).unwrap();
            assert_eq!(listed[0].git_branch.as_deref(), Some("feature"));
        });
    }

    #[test]
    fn a_turn_opens_the_search_of_its_folder() {
        use crate::domain::embeddings::{Embedding, EmbeddingError, EmbeddingProvider};
        struct NoModel;
        impl EmbeddingProvider for NoModel {
            fn embed(&self, _: &[&str]) -> Result<Vec<Embedding>, EmbeddingError> {
                Err(EmbeddingError::Invalid("no model in this test".into()))
            }
            fn dimensions(&self) -> usize {
                2
            }
        }
        let app = tauri::test::mock_app();
        let root = temp_dir("cmd-search-repo");
        assert!(searcher_of(app.handle(), &root).is_none(), "no index state at all");

        let index = Arc::new(WorkspaceIndex::new(temp_dir("cmd-search-index"), Arc::new(NoModel)));
        tauri::Manager::manage(&app, Arc::clone(&index));
        let search = searcher_of(app.handle(), &root).expect("a cold folder is opened for the turn");
        assert!(index.indexer(&root).is_some(), "the turn's folder is indexed while its search lives");
        assert!(index.indexer(&temp_dir("cmd-search-other-repo")).is_none(), "and no other folder is");
        drop(search);
        assert!(index.indexer(&root).is_none(), "a folder in the background cools once its turn's search goes");
    }
}
