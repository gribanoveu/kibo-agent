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
//! from outside while it runs: the stop flag, the queue of notes typed at it,
//! the workspace it acts on, and the standing approval policy.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Manager, Runtime, State};

use crate::domain::command_exec::Shell;
use crate::domain::compaction::{ContextUsage, RequestFrame};
use crate::domain::prompt::TurnContext;
use crate::domain::conversation_mode::ConversationMode;
use crate::domain::settings::RememberScope;
use crate::domain::llm::{LlmMessage, LlmToolCall};
use crate::domain::tools::{ApprovalPolicy, CodeSearchFn, Task, ToolPreview, ToolScope};
use crate::domain::next_prompt::{self, FinishedTurn, TurnFeatures};
use crate::domain::turn::{
    ChatEventPayload, ChatEventSink, ChatStreamOutcome, ChatTurnEvent, PendingApproval, PendingToolCall, SteeringNote,
    ToolCallDecision,
};
use crate::services::ai_tools::preview;
use crate::services::llm_chat::{self, Place, SteeringQueue, Turn, TurnError};
use crate::services::context_compaction;
use crate::domain::review::ReviewDesk;
use crate::services::mcp_questions::McpQuestions;
use crate::services::mcp_servers::McpServers;
use crate::domain::mcp::McpAnswer;
use crate::domain::mcp::McpTools;
use crate::domain::hooks::Hooks;
use crate::infra::background::Processes;
use crate::infra::mcp_config;
use crate::services::llm_session;
use crate::services::workspace_index::WorkspaceIndex;

use super::chat_events::chat_event_sink;
use super::workspace_events::index_event_sink;

#[derive(Default)]
pub struct AgentState {
    /// One in-flight turn at a time, so one flag needs no turn id to
    /// disambiguate. Reset by `chat_start` and deliberately *not* by
    /// `chat_resume` — a stop pressed while an approval card was showing has
    /// to survive until the resumed turn reads it.
    cancel: AtomicBool,
    steering: SteeringQueue,
    workspace: Mutex<Option<PathBuf>>,
    /// Grows as the user answers "always allow"; not persisted, because a
    /// saved "never ask me" is a brake released a month ago and forgotten.
    approval: Mutex<ApprovalPolicy>,
    /// What the window's mode chip says right now. Resident rather than sent
    /// with each turn for the same reason as the approval policy: a paused
    /// turn is resumed from a checkpoint that predates the chip, and a mode
    /// threaded through both entry points would have to be re-sent correctly
    /// by the caller at the one moment it is easiest to forget. Not persisted
    /// — a mode is a decision about this conversation, not about the app.
    mode: Mutex<ConversationMode>,
    /// The change `/review` is reviewing, while its turn lasts — a pause for
    /// approval and the resume included, since `reportFinding` checks every
    /// finding against it. Set by `review_start`, cleared by `chat_start`:
    /// while it is here the turn runs in Review mode, whatever the chip says.
    review: Mutex<Option<Arc<ReviewDesk>>>,
    /// The running turn's signals for the next-prompt journal. Resident
    /// because a turn outlives one command: what came before a pause for
    /// approval still counts after the resume.
    next_prompt: Mutex<TurnFeatures>,
    /// The last turn that ended, by id, until the window journals it. Apart
    /// from the running one, so a queued message that starts the next turn
    /// before that call arrives does not wipe it.
    finished_turn: Mutex<Option<(String, FinishedTurn)>>,
}

impl AgentState {
    pub(crate) fn workspace(&self) -> Result<PathBuf, String> {
        self.workspace
            .lock()
            .map_err(|_| "workspace lock poisoned".to_string())?
            .clone()
            .ok_or_else(|| "no folder is open".to_string())
    }

    fn mode(&self) -> ConversationMode {
        // A poisoned lock loses the chip's setting, not the turn. Falling back
        // to the default would silently widen it, so the last successful read
        // is not available — `Agent` is the default and the honest answer is
        // to say the mode could not be read.
        self.mode
            .lock()
            .map_or(ConversationMode::default(), |mode| *mode)
    }

    // The next-prompt journal's side of a turn. A poisoned lock costs the
    // journal one row, never the turn.

    /// A fresh turn, in the mode it starts in. A resume is not one.
    /// The mode a turn runs in, and the change under review when it is one:
    /// Review while a review's change is here, whatever the chip says.
    fn turn_mode(&self) -> (ConversationMode, Option<Arc<ReviewDesk>>) {
        match self.review.lock().ok().and_then(|review| review.clone()) {
            Some(desk) => (ConversationMode::Review, Some(desk)),
            None => (self.mode(), None),
        }
    }

    /// A new turn: a review's when `review` is the change under review,
    /// otherwise an ordinary one — which is what ends a review's Review mode.
    fn begin_turn(&self, review: Option<Arc<ReviewDesk>>) {
        if let Ok(mut current) = self.review.lock() {
            *current = review;
        }
        if let Ok(mut features) = self.next_prompt.lock() {
            *features = TurnFeatures::new(self.mode());
        }
    }

    fn observe(&self, event: &ChatEventPayload) {
        if let Ok(mut features) = self.next_prompt.lock() {
            features.observe(event);
        }
    }

    fn end_turn(&self, turn_id: String, outcome: &ChatStreamOutcome) {
        let finished = self.next_prompt.lock().ok().and_then(|features| features.finish(outcome));
        if let (Some(finished), Ok(mut slot)) = (finished, self.finished_turn.lock()) {
            *slot = Some((turn_id, finished));
        }
    }

    /// The finished turn if it is `turn_id`, once; another turn's stays put.
    fn take_finished(&self, turn_id: &str) -> Option<FinishedTurn> {
        let mut slot = self.finished_turn.lock().ok()?;
        if slot.as_ref().is_some_and(|(id, _)| id == turn_id) {
            slot.take().map(|(_, finished)| finished)
        } else {
            None
        }
    }

    fn approval(&self) -> Result<ApprovalPolicy, String> {
        Ok(self
            .approval
            .lock()
            .map_err(|_| "approval lock poisoned".to_string())?
            .clone())
    }
}

/// Opens a folder as the workspace. Canonicalized here, once, so every path a
/// tool resolves later is measured against a real directory rather than
/// against whatever spelling the caller used.
///
/// Also makes it the indexed folder. The index opening is awaited — it reads
/// what was embedded before, off the UI thread — and its first sync is not.
/// An index that cannot open does not stop the folder opening: the reason
/// arrives on `workspace-index:event`, and the agent works without search.
#[tauri::command]
pub async fn workspace_open(
    path: String,
    app: AppHandle,
    state: State<'_, Arc<AgentState>>,
    index: State<'_, Arc<WorkspaceIndex>>,
    git_watch: State<'_, super::git::GitWatch>,
) -> Result<String, String> {
    let resolved = PathBuf::from(&path)
        .canonicalize()
        .map_err(|e| format!("cannot open {path}: {e}"))?;
    if !resolved.is_dir() {
        return Err(format!("{path} is not a folder"));
    }
    *state
        .workspace
        .lock()
        .map_err(|_| "workspace lock poisoned".to_string())? = Some(resolved.clone());
    // They ran in the folder that was open, for the conversation about it.
    if let Some(processes) = app.try_state::<Arc<Processes>>() {
        processes.stop_all();
    }
    if let Some(terminals) = app.try_state::<Arc<crate::infra::terminal::Terminals>>() {
        terminals.close_all();
    }

    let shown = resolved.display().to_string();
    // Only a convenience for the next launch: a folder that opened stays open
    // even if the list could not be written.
    if let Err(e) = crate::infra::recent_workspaces::record(&shown) {
        eprintln!("recent folders not saved: {e}");
    }
    super::git::watch_git(&app, &git_watch, &resolved, shown.clone());
    let index = Arc::clone(&index);
    let sink = index_event_sink(&app, shown.clone());
    // Reported through the sink; see above.
    let _ = tauri::async_runtime::spawn_blocking(move || index.open(&resolved, sink)).await;
    Ok(shown)
}

/// A folder opened lately, and whose worktree it is if it is one — so the
/// folder menu can show it under the repository it belongs to.
#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentFolder {
    path: String,
    worktree_of: Option<String>,
}

/// Folders opened lately that still exist, the last one first.
#[tauri::command]
pub fn workspace_recent() -> Vec<RecentFolder> {
    recent_folders(crate::infra::recent_workspaces::load())
}

fn recent_folders(paths: Vec<String>) -> Vec<RecentFolder> {
    paths
        .into_iter()
        .map(|path| {
            let worktree_of = crate::infra::git_head::worktree_of(std::path::Path::new(&path));
            RecentFolder { worktree_of: worktree_of.map(|main| main.display().to_string()), path }
        })
        .collect()
}

#[tauri::command]
pub fn workspace_current(state: State<'_, Arc<AgentState>>) -> Option<String> {
    state
        .workspace
        .lock()
        .ok()?
        .clone()
        .map(|path| path.display().to_string())
}

/// The branch checked out in the open folder; `None` when it is not a
/// repository or nothing is open.
#[tauri::command]
pub fn workspace_branch(state: State<'_, Arc<AgentState>>) -> Option<String> {
    let root = state.workspace.lock().ok()?.clone()?;
    crate::infra::git_head::current_branch(&root)
}

/// The main working tree when the open folder is a worktree of it; `None`
/// for an ordinary folder or repository.
#[tauri::command]
pub fn workspace_worktree_of(state: State<'_, Arc<AgentState>>) -> Option<String> {
    let root = state.workspace.lock().ok()?.clone()?;
    crate::infra::git_head::worktree_of(&root).map(|path| path.display().to_string())
}

/// The open folder's index as it stands — what a window that was not
/// listening when the sync began (a reload, say) starts from before the next
/// `workspace-index:event`.
#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexSnapshot {
    /// The same string `workspace_open` returned, which the events carry too.
    root: String,
    syncing: bool,
    embedded: usize,
    skipped: usize,
    embedding_error: Option<String>,
}

#[tauri::command]
pub fn workspace_index_status(index: State<'_, Arc<WorkspaceIndex>>) -> Option<IndexSnapshot> {
    snapshot(&index)
}

fn snapshot(index: &WorkspaceIndex) -> Option<IndexSnapshot> {
    let indexer = index.current()?;
    let status = indexer.status();
    Some(IndexSnapshot {
        root: indexer.root().display().to_string(),
        syncing: status.syncing,
        embedded: status.embedded,
        skipped: status.skipped.len(),
        embedding_error: status.embedding_error,
    })
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
    messages: Vec<LlmMessage>,
    todos: Vec<Task>,
    plan: Option<String>,
) -> Result<ChatStreamOutcome, String> {
    let state = state.inner().clone();
    // A stray stop from a turn that already finished must not end this one
    // before it starts.
    state.cancel.store(false, Ordering::SeqCst);
    state.begin_turn(None);
    run_off_the_event_loop(app, state, turn_id, plan, move |turn| {
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
    checkpoint: PendingApproval,
    decisions: Vec<ToolCallDecision>,
    plan: Option<String>,
) -> Result<ChatStreamOutcome, String> {
    let state = state.inner().clone();
    run_off_the_event_loop(app, state, turn_id, plan, move |turn| {
        llm_chat::resume(turn, checkpoint, decisions)
    })
    .await
}

/// What the calls of a paused round would do, worked out without doing them.
///
/// Asked for the whole round at once: the card shows every call, and one round
/// trip beats one per call. Nothing here runs anything.
#[tauri::command]
pub fn chat_preview(
    calls: Vec<PendingToolCall>,
    state: State<'_, Arc<AgentState>>,
) -> Result<Vec<ToolPreview>, String> {
    let workspace = state.workspace()?;
    let scope = ToolScope::new(&workspace).map_err(|e| e.to_string())?;
    let calls: Vec<LlmToolCall> = calls
        .into_iter()
        .map(|call| LlmToolCall {
            id: call.id,
            name: call.name,
            arguments: call.arguments,
        })
        .collect();
    Ok(preview::preview_round(&scope, &calls))
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
pub async fn chat_compact(
    messages: Vec<LlmMessage>,
    force: bool,
    plan: Option<String>,
    turn_id: String,
    app: AppHandle,
    state: State<'_, Arc<AgentState>>,
) -> Result<Option<CompactedHistory>, String> {
    let frame = next_request_frame(&app, &state, plan.as_deref());
    let sink = chat_event_sink(&app, turn_id);
    // A summary is an ordinary request to the provider, and a request on the
    // IPC loop freezes every other command for its duration.
    tauri::async_runtime::spawn_blocking(move || {
        let session = llm_session::resolve(None).map_err(|e| e.to_string())?;
        let started = || {
            sink(ChatTurnEvent {
                seq: 1,
                round: 0,
                target_id: None,
                event: ChatEventPayload::HistoryCompacting,
            })
        };
        context_compaction::compact_if_needed(&session, frame, &messages, force, &started)
            .map(|compacted| {
                compacted.map(|compacted| CompactedHistory {
                    history: compacted.history,
                    folded: compacted.folded,
                })
            })
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("the compaction thread failed: {e}"))?
}

/// The message that stands for the part of a chat being rewound away from —
/// `messages`, from the rewound message on — for the window to add after the
/// conversation it keeps. `None` when the summary came back empty.
#[tauri::command]
pub async fn chat_branch_summary(messages: Vec<LlmMessage>) -> Result<Option<LlmMessage>, String> {
    // A request to the provider: off the IPC loop, as a compaction's.
    tauri::async_runtime::spawn_blocking(move || {
        let session = llm_session::resolve(None).map_err(|e| e.to_string())?;
        context_compaction::summarize_branch(&session, &messages).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("the summary thread failed: {e}"))?
}

/// What the next request will cost, for the window's meter.
///
/// Asked rather than computed there: the numbers on the meter have to be the
/// ones that decide, or a reader watches a gauge that is not connected to the
/// thing it appears to measure. Cheap — arithmetic over the history, one
/// serialization of the tool schemas and a read of the rules and skills, no
/// provider call — so it is a plain command rather than another thread.
#[tauri::command]
pub fn chat_context_usage(
    messages: Vec<LlmMessage>,
    plan: Option<String>,
    app: AppHandle,
    state: State<'_, Arc<AgentState>>,
) -> Result<ContextUsage, String> {
    let session = llm_session::resolve(None).map_err(|e| e.to_string())?;
    Ok(context_compaction::usage(&session, next_request_frame(&app, &state, plan.as_deref()), &messages))
}

/// What the next turn will send in front of the conversation: the prompt
/// parts and tools `run_off_the_event_loop` gathers, read the same way — but
/// MCP servers only as they run now, since none is started to be measured,
/// and the shell by name, since probing it runs it. Without an open folder,
/// the prompt a turn there would have.
fn next_request_frame<R: Runtime>(app: &AppHandle<R>, state: &AgentState, plan: Option<&str>) -> RequestFrame {
    let workspace = state.workspace().ok();
    let root = workspace.as_deref();
    let mode = state.mode();
    let skills = crate::services::skills::enabled_catalog(root).unwrap_or_default();
    let rules = root.map(crate::services::project_rules::load).unwrap_or_default();
    let mcp = match (root, app.try_state::<Arc<McpServers>>()) {
        (Some(root), Some(servers)) if mode == ConversationMode::Agent => servers.running(root),
        _ => McpTools::default(),
    };
    let worktree_of = root.and_then(crate::infra::git_head::worktree_of);
    let shell = Shell::default();
    let today = chrono::Local::now().format("%e %B %Y").to_string();
    context_compaction::request_frame(
        &TurnContext {
            mode,
            workspace: root.unwrap_or(std::path::Path::new("")),
            shell: &shell.program,
            today: &today,
            skills: &skills,
            rules: &rules,
            plan,
            worktree_of: worktree_of.as_deref(),
            // Read like the rest of the frame: as the next turn will read it.
            language: crate::infra::settings_store::load().ok().and_then(|s| s.llm.reply_language.name()),
            mcp_servers: mcp.notes(),
        },
        &mcp,
    )
}

/// Journals the turn `turn_id` for the next-prompt model: its input, built
/// from what the turn did and `user`, the message that started it as the
/// transcript shows it. `None` when there is nothing to journal — the turn is
/// not the last one that ended, was journaled already, or said nothing.
#[tauri::command]
pub fn next_prompt_log(
    chat_id: String,
    turn_id: String,
    user: String,
    state: State<'_, Arc<AgentState>>,
) -> Result<Option<String>, String> {
    let Some(finished) = state.take_finished(&turn_id) else {
        return Ok(None);
    };
    let input = finished.model_input(&user, &home());
    crate::infra::next_prompt_log::record(&chat_id, &input).map(Some).map_err(|e| e.to_string())
}

/// What the user sent after the journaled turn `id`.
#[tauri::command]
pub fn next_prompt_sent(id: String, text: String) -> Result<(), String> {
    crate::infra::next_prompt_log::record_sent(&id, &next_prompt::clean(&text, &home())).map_err(|e| e.to_string())
}

fn home() -> String {
    dirs::home_dir().map(|home| home.display().to_string()).unwrap_or_default()
}

/// Asks the running turn to stop. Returns at once: the turn notices at its
/// next checkpoint, which is never more than one tool call away.
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

#[tauri::command]
pub fn chat_cancel(state: State<'_, Arc<AgentState>>) {
    state.cancel.store(true, Ordering::SeqCst);
}

/// Queues something the user typed while the turn was running. The id comes
/// back so the same note can be withdrawn before a round takes it. `shown`
/// is what the transcript shows in place of `text` — a `/` command.
#[tauri::command]
pub fn chat_steer(text: String, shown: Option<String>, state: State<'_, Arc<AgentState>>) -> String {
    let note = SteeringNote { shown, ..SteeringNote::user(text) };
    let id = note.id.clone();
    state.steering.push(note);
    id
}

/// `false` means the note is already gone — a round picked it up while the
/// user was reaching for cancel.
#[tauri::command]
pub fn chat_cancel_steer(id: String, state: State<'_, Arc<AgentState>>) -> bool {
    state.steering.cancel(&id)
}

/// "Always allow this tool", from an approval card. Lasts as long as the app
/// runs and no longer.
#[tauri::command]
pub fn approval_always_allow(tool: String, state: State<'_, Arc<AgentState>>) -> Result<(), String> {
    state
        .approval
        .lock()
        .map_err(|_| "approval lock poisoned".to_string())?
        .allow_always(&tool)
}

/// What the conversation is for: everything, planning only, or answering
/// only. Takes effect from the next turn, and never retroactively — a turn
/// already paused on an approval card runs the calls the user decided on.
#[tauri::command]
pub fn chat_set_mode(
    mode: ConversationMode,
    state: State<'_, Arc<AgentState>>,
) -> Result<(), String> {
    *state
        .mode
        .lock()
        .map_err(|_| "mode lock poisoned".to_string())? = mode;
    Ok(())
}

/// Runs the whole turn without asking. Deliberately not persisted anywhere.
#[tauri::command]
pub fn approval_set_unattended(
    unattended: bool,
    chat_id: Option<String>,
    state: State<'_, Arc<AgentState>>,
) -> Result<(), String> {
    set_unattended(&state, unattended, chat_id.as_deref())
}

fn set_unattended(state: &AgentState, unattended: bool, chat_id: Option<&str>) -> Result<(), String> {
    set_skip_all(state, unattended)?;
    // Remembered after it took effect: a settings file that cannot be read
    // costs the memory, not the choice made for this turn.
    // No folder open, nothing to remember it for.
    let Ok(folder) = state.workspace() else { return Ok(()) };
    let mut settings = crate::infra::settings_store::load().map_err(|e| e.to_string())?;
    if settings.approval.set_auto(chat_id, &folder.display().to_string(), unattended) {
        crate::infra::settings_store::save(&settings).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Puts back what was chosen for this chat, or for the open folder, and says
/// what it is. Ask when nothing was, or no folder is open.
#[tauri::command]
pub fn approval_restore(chat_id: Option<String>, state: State<'_, Arc<AgentState>>) -> Result<bool, String> {
    restore_unattended(&state, chat_id.as_deref())
}

fn restore_unattended(state: &AgentState, chat_id: Option<&str>) -> Result<bool, String> {
    let settings = crate::infra::settings_store::load().map_err(|e| e.to_string())?;
    let auto = state
        .workspace()
        .is_ok_and(|folder| settings.approval.is_auto(chat_id, &folder.display().to_string()));
    set_skip_all(state, auto)?;
    Ok(auto)
}

#[tauri::command]
pub fn approval_remember_get() -> Result<RememberScope, String> {
    Ok(crate::infra::settings_store::load().map_err(|e| e.to_string())?.approval.remember)
}

/// Refuses on settings it cannot read, rather than saving defaults over them.
#[tauri::command]
pub fn approval_remember_set(remember: RememberScope) -> Result<(), String> {
    let mut settings = crate::infra::settings_store::load().map_err(|e| e.to_string())?;
    settings.approval.remember = remember;
    crate::infra::settings_store::save(&settings).map_err(|e| e.to_string())
}

/// A chat switched to Auto before its first save had no id to be remembered
/// by; it has one now. Best effort: the chat is saved either way, and a
/// memory that failed to save only means it asks next time.
pub(super) fn remember_new_chat(state: &AgentState, id: &str) {
    let auto = state.approval.lock().is_ok_and(|approval| approval.skip_all);
    let Ok(mut settings) = crate::infra::settings_store::load() else { return };
    if auto && settings.approval.remember == RememberScope::Chat && settings.approval.set_auto(Some(id), "", true) {
        let _ = crate::infra::settings_store::save(&settings);
    }
}

fn set_skip_all(state: &AgentState, unattended: bool) -> Result<(), String> {
    state.approval.lock().map_err(|_| "approval lock poisoned".to_string())?.skip_all = unattended;
    Ok(())
}

/// The open folder's search, for the turn's tools. `None` when no folder's
/// index is open — or when `setup` never ran, as under a mock runtime.
fn searcher_of<R: Runtime>(app: &AppHandle<R>) -> Option<CodeSearchFn> {
    app.try_state::<Arc<WorkspaceIndex>>()?.searcher()
}

/// Only an Agent turn starts servers: the others cannot offer their tools,
/// and a server's start is already something done on the user's machine.
/// A configuration that does not parse costs the turn its servers, not the
/// turn — the tab says what is wrong with the file.
fn mcp_for_turn(
    servers: Option<&McpServers>,
    mode: ConversationMode,
    workspace: &std::path::Path,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> McpTools {
    match servers {
        Some(servers) if mode == ConversationMode::Agent => {
            servers.for_turn(&mcp_config::load().unwrap_or_default(), workspace, cancelled)
        }
        _ => McpTools::default(),
    }
}

/// The turn's policy with the workspace's git aliases in it. Read per turn:
/// an alias added in a terminal a minute ago counts.
fn with_git_aliases(mut policy: ApprovalPolicy, workspace: &std::path::Path) -> ApprovalPolicy {
    policy.git_aliases = crate::infra::git_aliases::read(workspace);
    policy
}

/// `/review`: a turn in Review mode over the open folder's working tree
/// against `HEAD`. What is sent is decided here (`domain::review::select`):
/// the diffs go in as the turn's message, added to `messages`, and the
/// change stays on `AgentState` for `reportFinding` until the next turn.
/// Otherwise an ordinary turn — its events, its pauses, its Stop.
#[tauri::command]
pub async fn review_start<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, Arc<AgentState>>,
    turn_id: String,
    messages: Vec<LlmMessage>,
    todos: Vec<Task>,
    plan: Option<String>,
) -> Result<ChatStreamOutcome, String> {
    let state = state.inner().clone();
    let workspace = state.workspace()?;
    let files = tauri::async_runtime::spawn_blocking(move || crate::infra::git_changes::review_diffs(&workspace))
        .await
        .map_err(|e| format!("reading the change failed: {e}"))?
        .map_err(|e| e.to_string())?;
    let (kept, excluded) = crate::domain::review::select(files);
    if kept.is_empty() {
        return Err(match excluded.len() {
            0 => "nothing to review: the working tree matches HEAD".to_string(),
            n => format!("nothing to review: the {n} changed files are all left out (binary, generated, secrets or documentation)"),
        });
    }
    let mut messages = messages;
    messages.push(LlmMessage::user(crate::domain::review::review_message(
        &kept,
        &excluded,
        llm_session::reply_language(),
    )));
    state.cancel.store(false, Ordering::SeqCst);
    state.begin_turn(Some(Arc::new(ReviewDesk::new(kept))));
    run_off_the_event_loop(app, state, turn_id, plan, move |turn| llm_chat::stream(turn, messages, todos)).await
}

/// Assembles a turn and runs it on a blocking thread.
///
/// Off the event loop because the whole turn is synchronous — provider calls,
/// tool calls and a process runner — and holding the IPC loop for minutes
/// freezes every other command, including the one that stops it.
async fn run_off_the_event_loop<R, F>(
    app: AppHandle<R>,
    state: Arc<AgentState>,
    turn_id: String,
    plan: Option<String>,
    run: F,
) -> Result<ChatStreamOutcome, String>
where
    R: Runtime,
    F: FnOnce(&Turn) -> Result<ChatStreamOutcome, TurnError> + Send + 'static,
{
    let workspace = state.workspace()?;
    let approval = with_git_aliases(state.approval()?, &workspace);
    let (mode, review) = state.turn_mode();
    let search = searcher_of(&app);
    let mcp_servers = app.try_state::<Arc<McpServers>>().map(|servers| Arc::clone(&servers));
    let processes = app
        .try_state::<Arc<Processes>>()
        .map(|processes| Arc::clone(&processes) as Arc<dyn crate::domain::background::BackgroundProcesses>);
    let agents = app.try_state::<Arc<crate::domain::agents::Agents>>().map(|agents| Arc::clone(&agents));
    let questions = app.try_state::<Arc<McpQuestions>>().map(|questions| Arc::clone(&questions));
    let terminals = app
        .try_state::<Arc<crate::infra::terminal::Terminals>>()
        .map(|terminals| Arc::clone(&terminals) as Arc<dyn crate::domain::terminal::UserTerminals>);

    tauri::async_runtime::spawn_blocking(move || {
        let emit = chat_event_sink(&app, turn_id.clone());
        let observed = Arc::clone(&state);
        let events: ChatEventSink = Arc::new(move |event: ChatTurnEvent| {
            observed.observe(&event.event);
            emit(event)
        });
        let session = llm_session::resolve(None).map_err(|e| e.to_string())?;
        let scope = ToolScope::new(&workspace).map_err(|e| e.to_string())?;
        let cancelled = || state.cancel.load(Ordering::SeqCst);
        let sleep = |d: Duration| std::thread::sleep(d);
        let take_steering = || state.steering.take();
        let notes_typed = || state.steering.typed();
        let shell = Shell::default();
        let shell_described = crate::domain::command_exec::describe_shell(
            &shell.program,
            crate::infra::process_runner::probe_shell(&shell).as_ref(),
        );
        // A skills folder that cannot be read costs the turn its skills, not
        // the turn itself: nothing the user asked for depends on it existing.
        let skills = crate::services::skills::enabled_catalog(Some(&workspace)).unwrap_or_default();
        let rules = crate::services::project_rules::load(&workspace);
        let record = crate::infra::tool_call_log::recorder();
        let log_call = |entry: crate::domain::tool_call_log::ToolCallLogEntry| record(&entry);
        // Started only where they can be offered. A configuration that does
        // not parse costs the turn its servers, not the turn: the tab says
        // what is wrong with the file.
        // A file that does not parse stops the turn rather than running it
        // without the hooks: a guard the user wrote, silently off, is worse
        // than a turn that says why it did not start.
        let hooks = crate::infra::hooks::load()
            .map(|config| Hooks::new(config, Arc::new(crate::infra::hooks::run)))
            .map_err(|e| format!("{e} — fix or remove {}", crate::infra::hooks::path().map(|p| p.display().to_string()).unwrap_or_default()))?;
        let mcp = mcp_for_turn(mcp_servers.as_deref(), mode, &workspace, &cancelled);
        let worktree_of = crate::infra::git_head::worktree_of(&workspace);

        let turn = Turn {
            events: &events,
            session: &session,
            place: Place::Folder { scope: &scope, mode },
            approval: &approval,
            cancelled: &cancelled,
            sleep: &sleep,
            take_steering: &take_steering,
            notes_typed: &notes_typed,
            shell: &shell,
            shell_described: &shell_described,
            search,
            skills: &skills,
            rules: &rules,
            log_call: &log_call,
            plan: plan.as_deref(),
            worktree_of: worktree_of.as_deref(),
            mcp: &mcp,
            hooks: &hooks,
            processes,
            terminals,
            review,
            agents,
            questions,
        };
        let outcome = run(&turn).map_err(|e| e.to_string())?;
        state.end_turn(turn_id, &outcome);
        Ok(outcome)
    })
    .await
    .map_err(|e| format!("the turn thread failed: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::tools::ToolName;
    use crate::testing::temp_dir;

    /// The state is reachable without a running app, which is what keeps these
    /// checks about behaviour rather than about Tauri.
    fn state() -> Arc<AgentState> {
        Arc::new(AgentState::default())
    }

    fn next_prompt_turn(state: &AgentState, id: &str, events: &[ChatEventPayload]) {
        events.iter().for_each(|event| state.observe(event));
        let done = crate::domain::turn::ChatDone { result: Default::default(), todos: Vec::new(), history: Vec::new(), limit_reached: None };
        state.end_turn(id.to_string(), &ChatStreamOutcome::Done(done));
    }

    fn said(text: &str) -> ChatEventPayload {
        ChatEventPayload::RoundCompleted { text: text.into(), reasoning: String::new(), truncated: false }
    }

    /// A late call for an older turn must not take the newer one's place in
    /// the journal, and a turn is journaled once.
    #[test]
    fn the_next_prompt_journal_takes_a_finished_turn_once_and_only_by_its_id() {
        let state = state();
        state.begin_turn(None);
        next_prompt_turn(&state, "turn-2", &[said("ok")]);

        assert!(state.take_finished("turn-1").is_none());
        assert!(state.take_finished("turn-2").is_some(), "another turn's is left where it was");
        assert!(state.take_finished("turn-2").is_none());
    }

    /// A new turn starts from nothing, in the mode it starts in — and does
    /// not wipe the last one before the window has journaled it.
    #[test]
    fn a_new_turn_starts_the_next_prompt_signals_afresh() {
        let state = state();
        state.begin_turn(None);
        next_prompt_turn(&state, "turn-1", &[said("first")]);
        *state.mode.lock().unwrap() = ConversationMode::Plan;
        state.begin_turn(None);
        next_prompt_turn(&state, "turn-2", &[said("second")]);

        let input = state.take_finished("turn-2").expect("journaled").model_input("u", "");
        assert!(input.starts_with("<mode>plan"), "{input}");
        assert!(input.ends_with("<agent>second"), "{input}");
    }

    #[test]
    fn a_recent_worktree_names_the_repository_it_belongs_to() {
        let main = temp_dir("cmd-recent-main");
        let repo = git2::Repository::init(&main).unwrap();
        let tree = repo.find_tree(repo.index().unwrap().write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("T", "t@example.com").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "c", &tree, &[]).unwrap();
        let head = repo.head().unwrap().shorthand().unwrap().to_string();
        let linked =
            crate::infra::git_branches::add_worktree(&main, &head, "t", &temp_dir("cmd-recent-home")).unwrap();

        let folders = recent_folders(vec![linked.display().to_string(), main.display().to_string()]);
        let shown = |path: &str| std::path::Path::new(path).canonicalize().unwrap();
        assert_eq!(folders[0].path, linked.display().to_string());
        assert_eq!(folders[0].worktree_of.as_deref().map(shown), Some(main.canonicalize().unwrap()));
        assert_eq!(folders[1], RecentFolder { path: main.display().to_string(), worktree_of: None });
    }

    #[test]
    fn only_an_agent_turn_starts_mcp_servers() {
        crate::testing::with_app_dir("cmd-mcp-mode", || {
            mcp_config::save_text(r#"{"mcpServers":{"a":{"command":"x"}}}"#).unwrap();
            let starts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let counted = Arc::clone(&starts);
            let servers = McpServers::new(Arc::new(move |_, _, _| {
                counted.fetch_add(1, Ordering::SeqCst);
                Err(crate::domain::mcp::McpError::NotStarted("test".into()))
            }));
            let root = temp_dir("cmd-mcp-mode-root");
            for mode in [ConversationMode::Plan, ConversationMode::Ask] {
                mcp_for_turn(Some(&servers), mode, &root, &|| false);
            }
            assert_eq!(starts.load(Ordering::SeqCst), 0);
            mcp_for_turn(Some(&servers), ConversationMode::Agent, &root, &|| false);
            assert_eq!(starts.load(Ordering::SeqCst), 1);
        });
    }

    fn skip_all(state: &AgentState) -> bool {
        state.approval.lock().unwrap().skip_all
    }

    /// Auto is remembered for the chat, or for the folder, and put back from
    /// there; a chat switched to Auto before it had an id keeps it once saved.
    #[test]
    fn ask_or_auto_is_remembered_and_put_back() {
        crate::testing::with_app_dir("cmd-approval-memory", || {
            let state = state();
            // No folder: it takes effect, and nothing is remembered.
            set_unattended(&state, true, Some("c0")).unwrap();
            assert!(skip_all(&state));
            assert!(!restore_unattended(&state, Some("c0")).unwrap(), "no folder asks");

            *state.workspace.lock().unwrap() = Some(temp_dir("cmd-approval-folder").canonicalize().unwrap());
            set_unattended(&state, true, Some("c1")).unwrap();
            assert!(!restore_unattended(&state, Some("c2")).unwrap());
            assert!(!skip_all(&state), "the other chat asks, and the policy says so");
            assert!(restore_unattended(&state, Some("c1")).unwrap());
            assert!(skip_all(&state));

            // A new chat picked Auto with no id; its first save remembers it.
            set_unattended(&state, true, None).unwrap();
            remember_new_chat(&state, "c3");
            assert!(restore_unattended(&state, Some("c3")).unwrap());
            set_unattended(&state, false, Some("c3")).unwrap();
            remember_new_chat(&state, "c4");
            assert!(!restore_unattended(&state, Some("c4")).unwrap(), "saved under Ask, it asks");

            // Per repository, one choice for every chat, a new one included.
            let mut settings = crate::infra::settings_store::load().unwrap();
            settings.approval.remember = RememberScope::Repository;
            crate::infra::settings_store::save(&settings).unwrap();
            assert!(!restore_unattended(&state, Some("c1")).unwrap(), "the chat's choice is not the folder's");
            set_unattended(&state, true, Some("c1")).unwrap();
            assert!(restore_unattended(&state, None).unwrap());
            let before = crate::infra::settings_store::load().unwrap().approval;
            remember_new_chat(&state, "c5");
            assert_eq!(crate::infra::settings_store::load().unwrap().approval, before, "the folder already holds it");
        });
    }

    #[test]
    fn a_turn_needs_a_folder_before_it_can_start() {
        let err = state().workspace().expect_err("nothing is open");
        assert!(err.contains("no folder"), "{err}");
    }

    #[test]
    fn opening_a_folder_canonicalizes_it_once() {
        let state = state();
        let root = temp_dir("cmd-workspace");
        std::fs::create_dir(root.join("inner")).unwrap();

        let spelled = format!("{}/inner/..", root.display());
        *state.workspace.lock().unwrap() = Some(
            PathBuf::from(&spelled)
                .canonicalize()
                .expect("canonicalizes"),
        );

        let opened = state.workspace().unwrap();
        assert!(!opened.to_string_lossy().contains(".."), "{opened:?}");
        assert!(opened.is_dir());
    }

    /// Each turn reads the folder's git aliases, keeping what the session
    /// already allowed.
    #[test]
    fn a_turn_s_policy_carries_the_workspace_s_git_aliases() {
        let dir = temp_dir("cmd-git-aliases");
        git2::Repository::init(&dir).unwrap().config().unwrap().set_str("alias.pf", "push --force").unwrap();
        let mut base = ApprovalPolicy::default();
        base.always_allowed.insert(ToolName::RunCommand);

        let policy = with_git_aliases(base, &dir);
        assert_eq!(policy.git_aliases.get("pf").map(String::as_str), Some("push --force"));
        assert!(policy.always_allowed.contains(&ToolName::RunCommand));
    }

    /// The card's third button. It lasts for the session and is not written
    /// anywhere — a saved "never ask me" is a brake nobody remembers releasing.
    #[test]
    fn always_allow_widens_the_policy_for_that_tool_only() {
        let state = state();
        state
            .approval
            .lock()
            .unwrap()
            .always_allowed
            .insert(ToolName::WriteFile);

        let policy = state.approval().unwrap();
        assert!(!policy.requires_approval(ToolName::WriteFile, true));
        assert!(policy.requires_approval(ToolName::DeleteFile, true));
    }

    /// A review's turn runs in Review mode — its resume too — and the next
    /// message's turn in the chip's mode again.
    #[test]
    fn a_review_runs_in_review_mode_until_the_next_turn_starts() {
        let state = state();
        *state.mode.lock().unwrap() = ConversationMode::Ask;
        assert!(matches!(state.turn_mode(), (ConversationMode::Ask, None)));
        state.begin_turn(Some(Arc::new(ReviewDesk::new(Vec::new()))));
        assert!(matches!(state.turn_mode(), (ConversationMode::Review, Some(_))), "and so on resume");
        state.begin_turn(None);
        assert!(matches!(state.turn_mode(), (ConversationMode::Ask, None)));
    }

    #[test]
    fn a_note_can_be_withdrawn_until_a_round_takes_it() {
        let state = state();
        let note = SteeringNote::user("actually, use the helper");
        let id = note.id.clone();
        state.steering.push(note);

        assert!(state.steering.cancel(&id));
        assert!(state.steering.take().is_empty());
        assert!(!state.steering.cancel(&id), "already gone");
    }

    /// A stop is remembered until a fresh turn clears it — that is what makes
    /// "stop while the approval card is showing" work, since the resumed turn
    /// reads the flag rather than being told again.
    #[test]
    fn a_stop_survives_until_the_next_fresh_turn() {
        let state = state();
        state.cancel.store(true, Ordering::SeqCst);
        assert!(state.cancel.load(Ordering::SeqCst));

        state.cancel.store(false, Ordering::SeqCst);
        assert!(!state.cancel.load(Ordering::SeqCst));
    }

    #[test]
    fn a_turn_gets_the_search_of_the_open_folder() {
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
        assert!(searcher_of(app.handle()).is_none(), "no index state at all");

        let index = Arc::new(WorkspaceIndex::new(temp_dir("cmd-search-index"), Arc::new(NoModel)));
        tauri::Manager::manage(&app, Arc::clone(&index));
        assert!(searcher_of(app.handle()).is_none(), "no folder open");

        index.open(&temp_dir("cmd-search-repo"), Arc::new(|_| {})).unwrap();
        assert!(searcher_of(app.handle()).is_some());
    }

    /// The snapshot names the folder the way the events do, and says why
    /// search by meaning is off once a sync has found out.
    #[test]
    fn the_index_snapshot_speaks_for_the_open_folder() {
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
        let index = WorkspaceIndex::new(temp_dir("cmd-snapshot-index"), Arc::new(NoModel));
        assert_eq!(snapshot(&index), None);

        let root = temp_dir("cmd-snapshot-repo").canonicalize().unwrap();
        std::fs::write(root.join("a.rs"), "fn a() {}\n").unwrap();
        std::fs::write(root.join("blob.bin"), [0u8, 159, 146, 150]).unwrap();
        let (tx, finished) = std::sync::mpsc::channel();
        let tx = Mutex::new(tx);
        index
            .open(
                &root,
                Arc::new(move |event| {
                    if matches!(event, crate::domain::workspace_index::IndexEvent::SyncFinished { .. }) {
                        let _ = tx.lock().unwrap().send(());
                    }
                }),
            )
            .unwrap();
        // Not "the first finish": FSEvents may still report the files written
        // above, and the sync that wakes is running when the first one ends.
        let shot = loop {
            finished.recv_timeout(Duration::from_secs(10)).expect("the sync never finished");
            let shot = snapshot(&index).unwrap();
            if !shot.syncing {
                break shot;
            }
        };
        assert_eq!(shot.root, root.display().to_string());
        assert_eq!((shot.embedded, shot.skipped), (0, 1));
        assert!(shot.embedding_error.as_deref().is_some_and(|e| e.contains("no model")), "{shot:?}");
    }
}
