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
use std::sync::atomic::Ordering;
use std::sync::Arc;

use tauri::{AppHandle, Manager, Runtime, State};

use crate::domain::compaction::{ContextUsage, RequestFrame};
use crate::domain::image::ImagePart;
use crate::domain::prompt::TurnContext;
use crate::domain::settings::WebSearchPlace;
use crate::domain::llm::{LlmMessage, LlmToolCall};
use crate::domain::tools::{CodeSearchFn, Task, ToolPreview, ToolScope};
use crate::domain::next_prompt::{self};
use crate::domain::turn::{
    ChatEventPayload, ChatEventSink, ChatStreamOutcome, ChatTurnEvent, PendingApproval, PendingToolCall, SteeringNote,
    ToolCallDecision,
};
use crate::services::ai_tools::preview;
use crate::services::llm_chat::{self, Turn, TurnError};
use crate::services::context_compaction;
use crate::services::mcp_questions::McpQuestions;
use crate::services::mcp_servers::McpServers;
use crate::domain::mcp::McpAnswer;
use crate::domain::mcp::McpTools;
use crate::infra::background::Processes;
use crate::services::llm_session;
use crate::services::turn_setup::{self, FolderHandles, FolderTurn};
use crate::services::workspace_index::WorkspaceIndex;

use super::chat_events::chat_event_sink;

use super::agent_state::{AgentState, SessionHandle, RunningTurn};

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
) -> Result<ChatStreamOutcome, String> {
    let state = state.inner().clone();
    let (workspace, worktree_base) = state.chat_workspace(&chat_id)?;
    let session = state.session(&chat_id);
    // Admission before anything about the session is touched: a refused
    // start must leave the running turn's stop flag and journal alone.
    let running = state.begin_running(&chat_id, parallel_limit(), &workspace)?;
    // A stray stop from a turn that already finished must not end this one
    // before it starts.
    session.cancel.store(false, Ordering::SeqCst);
    session.begin_turn(state.mode());
    crate::infra::daily_metrics::record(&[(crate::domain::metrics::Metric::Prompts, "", 1)]);
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
) -> Result<ChatStreamOutcome, String> {
    let state = state.inner().clone();
    let (workspace, worktree_base) = state.chat_workspace(&chat_id)?;
    let session = state.session(&chat_id);
    let running = state.begin_resume(&chat_id, parallel_limit(), &workspace)?;
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
    let (workspace, _) = state.chat_workspace(&chat_id)?;
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
        Ok::<_, String>(preview::preview_round(&scope, &calls))
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
    let workspace = state.chat_prompt_workspace(&chat_id)?;
    let frame = next_request_frame(&app, &state, workspace);
    let sink = chat_event_sink(&app, turn_id);
    // A summary is an ordinary request to the provider, and a request on the
    // IPC loop freezes every other command for its duration.
    tauri::async_runtime::spawn_blocking(move || {
        let session = llm_session::resolve_for(None, Some(&chat_id)).map_err(|e| e.to_string())?;
        compact(&session, frame, &messages, force, sink)
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
    let started = || sink(ChatTurnEvent { seq: 1, round: 0, target_id: None, event: ChatEventPayload::HistoryCompacting });
    context_compaction::compact_if_needed(session, frame, messages, force, &started)
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
pub async fn chat_context_usage(
    messages: Vec<LlmMessage>,
    chat_id: String,
    app: AppHandle,
    state: State<'_, Arc<AgentState>>,
) -> Result<ContextUsage, String> {
    let workspace = state.chat_prompt_workspace(&chat_id)?;
    let state = Arc::clone(&state);
    super::blocking(move || {
        let session = llm_session::resolve(None).map_err(|e| e.to_string())?;
        Ok::<_, String>(context_compaction::usage(&session, next_request_frame(&app, &state, workspace), &messages))
    })
    .await
}

/// What the next turn will send in front of the conversation: the prompt
/// parts and tools `run_off_the_event_loop` gathers, read the same way — but
/// MCP servers only as they run now, since none is started to be measured,
/// and the shell by name, since probing it runs it. Without an open folder,
/// the prompt a turn there would have.
fn next_request_frame<R: Runtime>(app: &AppHandle<R>, state: &AgentState, workspace: Option<PathBuf>) -> RequestFrame {
    let root = workspace.as_deref();
    let mode = state.mode();
    let skills = crate::services::skills::enabled_catalog(root).unwrap_or_default();
    let rules = root.map(crate::services::project_rules::load).unwrap_or_default();
    let servers = app.try_state::<Arc<McpServers>>();
    let mcp = match (root, crate::services::turn_setup::servers_for(servers.as_ref().map(|s| s.as_ref()), mode)) {
        (Some(root), Some(servers)) => servers.running(root),
        _ => McpTools::default(),
    };
    let worktree_of = root.and_then(crate::infra::git_head::worktree_of);
    let shell = crate::infra::shells::for_commands();
    let today = crate::services::llm_chat::today();
    let features = crate::services::features::enabled();
    context_compaction::request_frame(
        &TurnContext {
            mode,
            workspace: root.unwrap_or(std::path::Path::new("")),
            shell: &shell.program,
            today: &today,
            skills: &skills,
            rules: &rules,
            worktree_of: worktree_of.as_deref(),
            parallel: None,
            // Read like the rest of the frame: as the next turn will read it.
            language: crate::infra::settings_store::load().ok().and_then(|s| s.llm.reply_language.name()),
            mcp_servers: mcp.notes(),
        },
        &mcp,
        crate::services::web_search::is_on(WebSearchPlace::Agent),
        &features,
    )
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

/// The chat folder's search, for the turn's tools. A background chat never
/// searches through the index of a folder opened after its turn started.
fn searcher_of<R: Runtime>(app: &AppHandle<R>, workspace: &std::path::Path) -> Option<CodeSearchFn> {
    let index = app.try_state::<Arc<WorkspaceIndex>>()?;
    let current = index.current()?;
    (current.root() == workspace).then(|| index.searcher()).flatten()
}

/// The MongoDB page's clients, as the agent's `mongo` tool reads through them.
pub(super) fn mongo_servers_of<R: Runtime>(app: &AppHandle<R>) -> Option<Arc<dyn crate::domain::mongo::MongoServers>> {
    let clients = app.try_state::<Arc<crate::infra::mongo_client::MongoClients>>()?;
    Some(Arc::new(crate::services::mongo::Servers(Arc::clone(&clients))))
}

/// Assembles a turn and runs it on a blocking thread.
///
/// The processes and helpers a turn of `chat_id` starts through: recorded as
/// that chat's, for the Processes and Agents tabs to list the open chat's.
fn chats_own(
    processes: Option<Arc<Processes>>,
    agents: Option<Arc<crate::domain::agents::Agents>>,
    chat_id: &str,
) -> (Option<Arc<dyn crate::domain::background::BackgroundProcesses>>, Option<Arc<crate::domain::agents::Agents>>) {
    (
        processes.map(|processes| Arc::new(processes.for_chat(chat_id)) as Arc<dyn crate::domain::background::BackgroundProcesses>),
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
) -> Result<ChatStreamOutcome, String>
where
    R: Runtime,
    F: FnOnce(&Turn) -> Result<ChatStreamOutcome, TurnError> + Send + 'static,
{
    let approval = state.approval()?;
    let mode = state.mode();
    let _running = running;
    let (processes, agents) = chats_own(
        app.try_state::<Arc<Processes>>().map(|processes| Arc::clone(&processes)),
        app.try_state::<Arc<crate::domain::agents::Agents>>().map(|agents| Arc::clone(&agents)),
        &chat_id,
    );
    let handles = FolderHandles {
        search: searcher_of(&app, &workspace),
        embeddings: app.try_state::<Arc<WorkspaceIndex>>().map(|index| index.embeddings()),
        mcp_servers: app.try_state::<Arc<McpServers>>().map(|servers| Arc::clone(&servers)),
        processes,
        agents,
        questions: app.try_state::<Arc<McpQuestions>>().map(|questions| Arc::clone(&questions)),
        terminals: app
            .try_state::<Arc<crate::infra::terminal::Terminals>>()
            .map(|terminals| Arc::clone(&terminals) as Arc<dyn crate::domain::terminal::UserTerminals>),
        board: app
            .try_state::<Arc<crate::infra::board_store::Board>>()
            .map(|board| Arc::clone(&board) as Arc<dyn crate::domain::board::TaskBoard>),
        mongo: mongo_servers_of(&app),
    };

    tauri::async_runtime::spawn_blocking(move || {
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
        let outcome = turn_setup::run_in_folder(folder, handles, run).map_err(|e| e.to_string())?;
        session.paused.store(matches!(&outcome, ChatStreamOutcome::PendingApproval(_)), Ordering::SeqCst);
        session.end_turn(turn_id, &outcome);
        Ok(outcome)
    })
    .await
    .map_err(|e| format!("the turn thread failed: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A turn's starts are its chat's, so the tabs list them under it.
    #[cfg(unix)]
    #[test]
    fn what_a_turn_starts_is_its_chats() {
        let (processes, agents) = (Arc::new(Processes::default()), Arc::new(crate::domain::agents::Agents::default()));
        let (theirs, helpers) = chats_own(Some(Arc::clone(&processes)), Some(Arc::clone(&agents)), "c1");
        let dir = crate::testing::temp_dir("cmd-chat-owned");
        theirs.unwrap().start(&crate::domain::command_exec::Shell::default(), "sleep 30", &dir, ".").unwrap();
        helpers.unwrap().start("where is X");
        assert_eq!((processes.list_for("c1").len(), agents.list_for("c1").len()), (1, 1));
        processes.stop_all();
    }
    
    use crate::testing::temp_dir;
    

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
            let own = temp_dir("cmd-chat-save-own").canonicalize().unwrap();
            let other = temp_dir("cmd-chat-save-other").canonicalize().unwrap();
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
            ))
            .unwrap();
            assert_eq!(crate::infra::chat_store::load("c1").unwrap().workspace, own.display().to_string());
        });
    }

    #[test]
    fn preview_and_resume_read_the_saved_chats_checkout() {
        crate::testing::with_app_dir("cmd-chat-resume-own-root", || {
            crate::testing::spending_provider("done", 10, 2);
            let own = temp_dir("cmd-chat-resume-own").canonicalize().unwrap();
            let other = temp_dir("cmd-chat-resume-other").canonicalize().unwrap();
            std::fs::write(own.join("a.txt"), "own\n").unwrap();
            std::fs::write(other.join("a.txt"), "other\n").unwrap();
            crate::infra::chat_store::save("c1", &own.display().to_string(), &[LlmMessage::user("hi")], &serde_json::json!([]), &[], None, None, None).unwrap();
            let state = state();
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
            let own = temp_dir("cmd-chat-compact-own").canonicalize().unwrap();
            let other = temp_dir("cmd-chat-compact-other").canonicalize().unwrap();
            std::fs::write(own.join("AGENTS.md"), "x".repeat(1_000_000)).unwrap();
            crate::infra::chat_store::save("c1", &own.display().to_string(), &[LlmMessage::user("hi")], &serde_json::json!([]), &[], None, None, None).unwrap();
            let state = state();
            state.set_workspace_for_test(other.clone());
            let app = tauri::test::mock_app();
            let other_frame = next_request_frame(app.handle(), &state, Some(other));
            let own_frame = next_request_frame(app.handle(), &state, Some(own));
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
            *state.workspace.lock().unwrap() = Some(temp_dir("cmd-chat-fresh").canonicalize().unwrap());
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
            ));
            let Ok(ChatStreamOutcome::Done(done)) = outcome else { panic!("the leftover stop ended the fresh turn: {outcome:?}") };
            assert_eq!(done.result.text, "hello");
        });
    }

    /// Journaling asks the chat's own session: another chat's id finds
    /// nothing there, and the turn's own chat journals it once.
    #[test]
    fn the_journal_is_the_chats_own() {
        crate::testing::with_app_dir("cmd-chat-journal-own", || {
            crate::testing::spending_provider("hello", 10, 2);
            let state = state();
            *state.workspace.lock().unwrap() = Some(temp_dir("cmd-chat-journal").canonicalize().unwrap());
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
            *state.workspace.lock().unwrap() = Some(temp_dir("cmd-chat-resume").canonicalize().unwrap());
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

    #[test]
    fn a_paused_turn_refuses_a_fresh_start_until_it_is_resumed() {
        crate::testing::with_app_dir("cmd-paused-chat-turn", || {
            crate::testing::spending_provider("unused", 10, 2);
            let state = state();
            let root = temp_dir("cmd-paused-chat-root").canonicalize().unwrap();
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
            *state.workspace.lock().unwrap() = Some(temp_dir("cmd-refused-keeps-stop-ws").canonicalize().unwrap());
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
            *state.workspace.lock().unwrap() = Some(temp_dir("cmd-two-turns-ws").canonicalize().unwrap());
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
        ))
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
            *state.workspace.lock().unwrap() = Some(temp_dir("cmd-same-chat-ws").canonicalize().unwrap());
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
            *state.workspace.lock().unwrap() = Some(temp_dir("cmd-parallel-limit-ws").canonicalize().unwrap());
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
            let root = temp_dir("cmd-chat-git-branch-repo").canonicalize().unwrap();
            git2::Repository::init(&root).unwrap().set_head("refs/heads/feature").unwrap();
            let state = state();
            *state.workspace.lock().unwrap() = Some(root.clone());
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
            ))
            .unwrap();
            assert_eq!(saved.git_branch.as_deref(), Some("feature"));
            let listed = crate::infra::chat_store::list(&root.display().to_string()).unwrap();
            assert_eq!(listed[0].git_branch.as_deref(), Some("feature"));
        });
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
        let root = temp_dir("cmd-search-repo");
        assert!(searcher_of(app.handle(), &root).is_none(), "no index state at all");

        let index = Arc::new(WorkspaceIndex::new(temp_dir("cmd-search-index"), Arc::new(NoModel)));
        tauri::Manager::manage(&app, Arc::clone(&index));
        assert!(searcher_of(app.handle(), &root).is_none(), "no folder open");

        index.open(&root, Arc::new(|_| {})).unwrap();
        assert!(searcher_of(app.handle(), &root).is_some());
        assert!(searcher_of(app.handle(), &temp_dir("cmd-search-other-repo")).is_none(), "a background chat cannot borrow another folder's index");
    }
}
