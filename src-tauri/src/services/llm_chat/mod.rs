//! The agent loop and the small decisions a round makes on its own.
//!
//! Here: the turn, its two entry points and the loop both run. Around it:
//! `round` asks the model, `calls` runs what it asked for, `helpers` starts
//! explore, fork and review agents, `notes` restores what the history is owed,
//! `offer` picks the tools a request carries, `events` reports outward. The
//! rules take no provider, which is what makes them testable without a model
//! at the other end (`tests/`, with the scripted provider in `tests/mod.rs`).

use std::cell::{Cell, RefCell};

use crate::domain::features::Features;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Local;

use crate::domain::llm::{ChatRequest, ChatStreamResult, LlmError, LlmMessage, LlmRole, LlmToolCall, LlmToolDefinition, sanitize_tool_call_arguments, StreamEvent};
use crate::domain::llm_retry::{MAX_ATTEMPTS, retry_delay};
use crate::domain::compaction::{self, KEEP_TAIL_PERCENT, RETRY_KEEP_TAIL_PERCENT};
use crate::domain::result_clearing;
use crate::domain::explore_answer;
use crate::domain::web_search::Web;
use crate::domain::loop_guard::{self, Loop, LoopGuard, Settled};
use crate::domain::chat_role::ChatRole;
use crate::domain::kube::{KubeApi, KubeChanges, KubeSetup, PinnedCluster};
use crate::domain::runbooks::Runbook;

use crate::domain::conversation_mode::{self, ConversationMode};
use crate::domain::prompt::{self, CHECKLIST_LEGEND};
use crate::domain::tool_call_log::{self, CallStatus, ToolCallLogEntry};
use crate::domain::project_rules::RuleFile;
use crate::domain::skills::Skill;
use crate::domain::mcp::McpTools;
use crate::domain::hooks::{HookEvent, Hooks, MAX_STOP_BLOCKS};
use crate::domain::background::{self, BackgroundProcesses};
use crate::domain::command_exec::{CommandEvent, CommandSink, Shell};
use crate::domain::tools::{
    ApprovalPolicy, CodeSearchFn, ReadFiles, Task, ToolDeps, ToolName, ToolResult, ToolScope,
    AFTER_DENIAL, TOOL_DENIED_PREFIX, TOOL_ERROR_PREFIX, TOOL_NOT_RUN_PREFIX,
};
use crate::domain::turn::{
    ChatDone, ChatEventPayload, ChatEventSink, ChatStreamOutcome, ChatTurnEvent, DecisionError,
    PendingApproval, PendingToolCall, SteeringNote, ToolCallDecision, ToolCallEvent,
    ToolResultEvent,
};
use crate::infra::llm_debug_log;
use crate::services::ai_tools::parse::{parse_tool_call, preflight_chat_call, preflight_paths, preflight_tool_call};
use crate::services::ai_tools::model_text::for_model;
use crate::services::ai_tools::resolve::{relative_to_root, resolve_existing};
use crate::domain::rewind::FileChange;
use crate::services::ai_tools::file_changes;
use crate::services::ai_tools::tools::{dispatch, tool_definitions};
use crate::services::context_compaction;
use crate::services::llm_session::LlmSession;

mod calls;
mod events;
mod helpers;
mod notes;
mod offer;
mod round;
#[cfg(test)]
mod tests;

use calls::*;
use events::*;
use helpers::*;
use notes::*;
use offer::*;
use round::*;
pub use round::estimate_request;
pub(crate) use offer::{tool_definitions_for, tool_definitions_for_role, waiting_tools};

/// Everything one turn needs that is not the conversation itself.
///
/// Cancellation and waiting arrive as functions rather than as a flag and a
/// `thread::sleep`, for the same reason the events do: the loop then runs in a
/// test at full speed, and neither the retry wait nor the stop button needs a
/// real clock to be exercised.
pub struct Turn<'a> {
    pub events: &'a ChatEventSink,
    pub session: &'a LlmSession,
    /// The open folder and its mode, or a chat's role — what the model is
    /// told and what it may call.
    pub place: Place<'a>,
    pub approval: &'a ApprovalPolicy,
    /// Polled between rounds, after a round streams, between individual calls,
    /// and during a retry wait.
    pub cancelled: &'a (dyn Fn() -> bool + Sync),
    /// Called in one-second slices while waiting to retry, so a stop takes
    /// effect during the wait rather than after it.
    pub sleep: &'a (dyn Fn(Duration) + Sync),
    /// Which shell runs a command line. A setting, not a search of `PATH` —
    /// see `domain::command_exec`.
    pub shell: &'a Shell,
    /// How the prompt names it — `domain::command_exec::describe_shell`.
    pub shell_described: &'a str,
    /// Takes whatever the user has typed since it was last called. Draining
    /// rather than reading is deliberate: a note handed to the model must
    /// leave the queue in the same step, or a round that is retried or
    /// interrupted can deliver it twice.
    pub take_steering: &'a (dyn Fn() -> Vec<SteeringNote> + Sync),
    /// [`SteeringQueue::typed`]: a command waited on steps aside to the
    /// background when it changes, so a note is not left waiting behind it.
    pub notes_typed: &'a (dyn Fn() -> u64 + Sync),
    /// Search of the open folder's index, for `semanticSearch`; `None` when
    /// the folder has none, and the tool says so to the model.
    pub search: Option<CodeSearchFn>,
    /// The open folder's skills and the user's, listed in the prompt for
    /// `skill` to load. Read once per turn, so the prompt does not change
    /// between its rounds.
    pub skills: &'a [Skill],
    /// The open folder's instruction files, read once per turn like the skills.
    pub rules: &'a [RuleFile],
    /// The rules as the chat's first turn read them, which the prompt keeps
    /// saying; what changed since reaches the model through the history
    /// ([`restore_standing`]). `None` says [`Turn::rules`] in the prompt.
    pub frozen: Option<&'a Frozen>,
    /// Where each settled call's redacted record goes — the log on disk in
    /// the app, a list in a test. A port rather than a direct write so that
    /// a turn under test never touches whatever app directory another test
    /// has installed.
    pub log_call: &'a (dyn Fn(ToolCallLogEntry) + Sync),
    /// The chat's plan as the window holds it — the user's edits included.
    /// Read at the start of the turn; it reaches the model through the
    /// history ([`restore_plan`]), never the prompt.
    pub plan: Option<&'a str>,
    /// The main working tree, when the open folder is a worktree of it —
    /// said in the prompt, so the model keeps out of the user's checkout.
    pub worktree_of: Option<&'a std::path::Path>,
    /// The commit `HEAD` pointed at when this chat was first saved; `None`
    /// for a chat without the marker. A review reads it as its base when
    /// the working tree is clean (`domain::review`'s table, rule 3).
    pub chat_head: Option<&'a str>,
    /// The connected MCP servers' tools. Read once per turn, like the
    /// skills: the tools a model was shown must not change between rounds.
    pub mcp: &'a McpTools,
    /// The user's hooks, read once per turn.
    pub hooks: &'a Hooks,
    /// Background processes, which outlive the turn; `None` has none.
    pub processes: Option<Arc<dyn BackgroundProcesses>>,
    /// The user's own terminals; `None` has none.
    pub terminals: Option<Arc<dyn crate::domain::terminal::UserTerminals>>,
    /// A review worker's files and findings; `None` outside a review.
    pub review: Option<Arc<crate::domain::review::ReviewDesk>>,
    /// The Agents tab's record of `explore` runs; `None` keeps a throwaway one.
    pub agents: Option<Arc<crate::domain::agents::Agents>>,
    /// Where an MCP server's question waits for the user's answer; `None`
    /// where there is no window to ask in, and the question is declined.
    pub questions: Option<Arc<crate::services::mcp_questions::McpQuestions>>,
    /// The board `board` keeps cards on; `None` has none.
    pub board: Option<Arc<dyn crate::domain::board::TaskBoard>>,
    /// The MongoDB servers `mongo` reads; `None` while the feature is off.
    pub mongo: Option<Arc<dyn crate::domain::mongo::MongoServers>>,
    /// DeepSeek's search and page reader, for `deepseekSearch` and
    /// `deepseekFetch`; `None` while the feature is off or there is no key.
    pub deepseek: Option<Web>,
    /// The features the user has on, read once a turn: a tool of one that is
    /// off is neither offered nor run (`domain::features`).
    pub features: crate::domain::features::Features,
    /// A forked `explore` helper: the calling turn's prompt and tools, so its
    /// start is read from the provider's cache, and of the tools only what
    /// Explore mode offers runs (`preflight`).
    pub fork: bool,
}

/// The part of a folder's prompt that the files behind it can change in the
/// middle of a chat: the instruction files, the memory among them.
///
/// Kept as the chat's first turn read it. The system prompt comes first in
/// every request, so a change to it is a cache miss on the whole
/// conversation behind it: an agent that kept `AGENTS.md` up to date after
/// each step paid for a 290k history in full on ten turns of thirteen. Said
/// in the prompt as it was, and as it is now in a note at the end of the
/// history, a change costs the note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frozen {
    pub rules: Vec<RuleFile>,
    /// The shared-tree warning as this chat's first turn saw it.
    pub parallel: Option<String>,
}


/// Where a turn works, which decides what the model is told and what it may
/// call. One enum rather than an optional folder beside a mode: the two go
/// together — Chat mode has no folder *and* no conversation mode, only a role.
#[derive(Clone, Copy)]
pub enum Place<'a> {
    /// The agent in the open folder. The mode is read once per turn: a mode
    /// changed while an approval card was showing does not rewrite decisions
    /// already made.
    Folder {
        scope: &'a ToolScope,
        mode: ConversationMode,
        /// What `webSearch` and `webFetch` reach — the agent's own choice in
        /// Settings → Web search; `None` while it is off, and then neither
        /// is offered.
        web: Option<&'a Web>,
        /// Asked live: another chat may begin after this one. The git guard
        /// reads it per command; the standing note reads it between rounds.
        parallel: &'a (dyn Fn() -> Vec<String> + Sync),
        /// The base this chat's worktree was moved from, if it was.
        worktree_base: Option<&'a str>,
    },
    /// Chat mode (`docs/21-kubernetes-mode.md`, K-1): no folder. The role is
    /// who the model is and all it may call; `kube` is what it is told of the
    /// user's cluster, and `cluster` what its tools read it through — `None`
    /// when no cluster is pinned.
    /// `changes` is where a change to it is recorded first.
    Chat {
        role: ChatRole,
        kube: &'a KubeSetup,
        cluster: Option<&'a dyn KubeApi>,
        changes: Option<&'a dyn KubeChanges>,
        /// The runbooks the prompt lists and `kubeRunbook` reads.
        runbooks: &'a [Runbook],
        /// What `webSearch` and `webFetch` reach; `None` while web search is
        /// off, and then neither is offered.
        web: Option<&'a Web>,
        /// The chat's own folder, for a role that works in one
        /// (`ChatRole::workspace`); `None` for every other.
        scope: Option<&'a ToolScope>,
    },
}

impl<'a> Place<'a> {
    /// The folder, where there is one.
    pub fn scope(&self) -> Option<&'a ToolScope> {
        match self {
            Place::Folder { scope, .. } => Some(scope),
            Place::Chat { scope, .. } => *scope,
        }
    }

    /// The conversation mode, where there is one.
    pub fn mode(&self) -> Option<ConversationMode> {
        match self {
            Place::Folder { mode, .. } => Some(*mode),
            Place::Chat { .. } => None,
        }
    }
}

/// Notes typed while a turn is running, waiting for the next round.
///
/// Shared between the turn and whatever accepts the user's typing, so it owns
/// its own lock. A poisoned lock is recovered rather than propagated: losing
/// the queue must not take down a turn that is otherwise fine.
#[derive(Default)]
pub struct SteeringQueue {
    notes: Mutex<Vec<SteeringNote>>,
    typed: AtomicU64,
}

impl SteeringQueue {
    pub fn push(&self, note: SteeringNote) {
        self.notes.lock().unwrap_or_else(|e| e.into_inner()).push(note);
        self.typed.fetch_add(1, Ordering::SeqCst);
    }

    pub fn take(&self) -> Vec<SteeringNote> {
        std::mem::take(&mut *self.notes.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// How many notes were ever pushed — a count that only grows, so a
    /// wait can tell a note typed during it from one already waiting.
    pub fn typed(&self) -> u64 {
        self.typed.load(Ordering::SeqCst)
    }

    /// Removes the note with this id, if it is still queued.
    ///
    /// `false` means it is already gone — a round picked it up while the user
    /// was reaching for cancel. What has been said to the model cannot be
    /// unsaid, and the answer is what lets the caller tell "withdrawn" from
    /// "too late".
    pub fn cancel(&self, id: &str) -> bool {
        let mut notes = self.notes.lock().unwrap_or_else(|e| e.into_inner());
        let before = notes.len();
        notes.retain(|note| note.id != id);
        notes.len() != before
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TurnError {
    #[error("{0}")]
    Provider(#[from] LlmError),
    #[error("cannot resume: {0}")]
    BadResume(String),
    #[error(transparent)]
    Decision(#[from] DecisionError),
}

/// Assigns each event its place in the stream. The cursor is restored from the
/// checkpoint on resume, which is what keeps one turn's numbering monotonic
/// across the gap.
struct Events<'a> {
    sink: &'a ChatEventSink,
    seq: Cell<u64>,
}

impl<'a> Events<'a> {
    fn new(sink: &'a ChatEventSink, seq: u64) -> Self {
        Self {
            sink,
            seq: Cell::new(seq),
        }
    }

    fn emit(&self, round: u32, target_id: Option<String>, event: ChatEventPayload) {
        let seq = self.seq.get().saturating_add(1);
        self.seq.set(seq);
        (self.sink)(ChatTurnEvent {
            seq,
            round,
            target_id,
            event,
        });
    }

    fn last_seq(&self) -> u64 {
        self.seq.get()
    }
}

/// What the loop carries from round to round, and what a pause has to hand
/// back. One struct rather than eight parameters, because the two entry points
/// below would otherwise have to keep the same order twice.
struct State {
    history: Vec<LlmMessage>,
    round: u32,
    budget_used: u32,
    todos: Vec<Task>,
    reads: ReadFiles,
    /// What `writePlan` last wrote in this turn; `None` until it does, and
    /// the window's plan ([`Turn::plan`]) is the current one until then.
    written_plan: Option<String>,
}

/// A fresh turn: run from the first round until the model stops asking for
/// tools, a call needs a human, or the turn is cancelled.
pub fn stream(
    turn: &Turn,
    messages: Vec<LlmMessage>,
    todos: Vec<Task>,
) -> Result<ChatStreamOutcome, TurnError> {
    // A note queued after the previous turn ended is not part of this one:
    // the user typed it at a conversation that had already finished, and it
    // reaches the model as their next message instead.
    let _ = (turn.take_steering)();
    let state = State {
        history: messages,
        round: 0,
        budget_used: 0,
        todos,
        reads: ReadFiles::default(),
        written_plan: None,
    };
    run(turn, state, 0, None)
}

/// Continues a turn that paused for approval.
///
/// Takes the checkpoint whole, exactly as [`ChatStreamOutcome::PendingApproval`]
/// handed it over. Alfa Atlas takes its six fields apart into six parameters and
/// relies on the front end to reassemble them — a forgotten field is then a
/// silently reset round ceiling rather than a compile error (see
/// `docs/07-upstream-findings.md`, B-4).
pub fn resume(
    turn: &Turn,
    checkpoint: PendingApproval,
    decisions: Vec<ToolCallDecision>,
) -> Result<ChatStreamOutcome, TurnError> {
    checkpoint.check_decisions(&decisions)?;

    // The history has to still end with the assistant's tool-call turn: the
    // resumed round appends this round's tool results, and results with no
    // request in front of them are rejected by the provider — long after the
    // point where the mismatch could be explained. Results already after it
    // are that round's too: calls the preflight refused are answered before
    // the pause.
    match checkpoint.history.iter().rev().find(|m| m.role != LlmRole::Tool) {
        Some(last) if last.role == LlmRole::Assistant && !last.tool_calls.is_empty() => {}
        _ => {
            return Err(TurnError::BadResume(
                "the history must end with the assistant's tool-call round".to_string(),
            ));
        }
    }

    let calls: Vec<LlmToolCall> = checkpoint
        .calls
        .iter()
        .map(|call| LlmToolCall {
            id: call.id.clone(),
            name: call.name.clone(),
            arguments: call.arguments.clone(),
        })
        .collect();
    let state = State {
        history: checkpoint.history,
        round: checkpoint.round,
        budget_used: checkpoint.budget_used,
        todos: checkpoint.todos,
        reads: checkpoint.reads,
        // The window follows `writePlan` while the turn runs, so a plan
        // written before the pause is the prompt's from here on.
        written_plan: None,
    };
    run(turn, state, checkpoint.event_seq, Some((calls, decisions)))
}

/// How a turn hands back: its last round's result, and the history the next
/// message is sent with.
///
/// A turn stopped with calls requested but not all run would leave requests
/// with no results after them, which the provider refuses on the next
/// message. Each gets a result saying it did not run — which is also what the
/// model should know about it.
///
/// The last round's text closes the history. A round is added to the history
/// only once its calls are about to run, and no exit here is past that point
/// — a turn stopped right after a round keeps what the round said, not the
/// calls it never ran.
fn ended(mut state: State, result: ChatStreamResult) -> ChatDone {
    if let Some(asked) = state.history.iter().rposition(|m| m.role == LlmRole::Assistant && !m.tool_calls.is_empty()) {
        let answered: Vec<&str> = state.history[asked + 1..].iter().filter_map(|m| m.tool_call_id.as_deref()).collect();
        let unrun: Vec<String> = state.history[asked]
            .tool_calls
            .iter()
            .filter(|call| !answered.contains(&call.id.as_str()))
            .map(|call| call.id.clone())
            .collect();
        for id in unrun {
            state.history.push(tool_message(&id, format!("{TOOL_NOT_RUN_PREFIX}the user stopped the turn before this call.")));
        }
    }
    if !result.text.is_empty() {
        state.history.push(LlmMessage::assistant(result.text.clone()));
    }
    ChatDone { result, todos: state.todos, history: state.history, limit_reached: None }
}

/// What one pass of the loop keeps and no more: none of it is in the
/// checkpoint, so a resume starts each anew.
#[derive(Default)]
struct Pass {
    /// `<tool>|<arguments>` → hash of what came back, see `dedupe_repeat_result`.
    /// After a pause the first repeat of a read comes back in full once more,
    /// which costs context and loses nothing.
    seen_results: HashMap<String, u64>,
    /// How often Stop hooks have sent the model back. A resume is the user's
    /// go-ahead, and starts the count over.
    stop_blocks: u32,
    /// Empty replies answered with a note; see `MAX_EMPTY_NUDGES`.
    empty_nudges: u32,
    /// Rounds going in circles; see `domain::loop_guard`.
    guard: LoopGuard,
    /// The calls of the round before, for the guard.
    settled: Vec<Settled>,
    /// Tokens this pass has spent, every request's prompt and reply: said with
    /// a review's wrap-up.
    spent: u64,
}

/// What asking the model for a round came to.
enum Next {
    /// Calls to run now, none needing a decision; `true` when the reply that
    /// asked for them was cut off.
    Run(Vec<LlmToolCall>, bool),
    /// Another round before anything runs: a note or a refusal went into the
    /// history for the model to react to.
    Again,
    /// The model is finished, and nothing keeps the turn going.
    Done(ChatStreamResult),
    Cancelled(ChatStreamResult),
    /// A call needs the user: the whole round waits for the card.
    Paused(Vec<PendingToolCall>),
}

/// The loop both entry points run.
///
/// `resume` carries a round whose calls are already known and already decided;
/// a fresh round asks the model instead ([`ask_round`]). Everything after that
/// point is the same code ([`run_calls`]), which is the reason a paused turn
/// behaves like an ordinary one. How a turn ends is decided here alone.
fn run(
    turn: &Turn,
    mut state: State,
    event_seq: u64,
    mut resume: Option<(Vec<LlmToolCall>, Vec<ToolCallDecision>)>,
) -> Result<ChatStreamOutcome, TurnError> {
    let events = Events::new(turn.events, event_seq);
    let mut pass = Pass::default();

    loop {
        // Checkpoint one. Before the ceiling check as well, so a turn the user
        // stopped reports as cancelled rather than as having run out of rounds.
        if (turn.cancelled)() {
            return Ok(ChatStreamOutcome::Cancelled(ended(state, ChatStreamResult::default())));
        }
        // The ceilings are the user's (`domain::settings::TurnLimits`).
        let limits = turn.session.limits;
        // Checked at the top of a round, so every call the last round asked
        // for has run and been answered: the history is whole.
        if state.round >= limits.rounds || state.budget_used >= limits.budget {
            let rounds = state.round;
            return Ok(ChatStreamOutcome::Done(ChatDone { limit_reached: Some(rounds), ..ended(state, ChatStreamResult::default()) }));
        }
        state.round += 1;
        let round = state.round;

        let (calls, decisions, round_truncated) = if let Some((calls, decisions)) = resume.take() {
            // Charged again on the resumed pass, exactly as `round` is counted
            // twice: otherwise pausing would be a way to buy budget.
            state.budget_used += round_cost(&calls, turn.mcp);
            // Never cut off: a resumed round has no reply of its own — the
            // round that produced these calls already reported, and whatever
            // note it earned is in the history.
            (calls, decisions, false)
        } else {
            match ask_round(turn, &events, round, &mut state, &mut pass)? {
                Next::Run(calls, truncated) => (calls, Vec::new(), truncated),
                Next::Again => continue,
                Next::Done(result) => return Ok(ChatStreamOutcome::Done(ended(state, result))),
                Next::Cancelled(result) => return Ok(ChatStreamOutcome::Cancelled(ended(state, result))),
                Next::Paused(calls) => {
                    return Ok(ChatStreamOutcome::PendingApproval(PendingApproval {
                        history: state.history,
                        round,
                        budget_used: state.budget_used,
                        event_seq: events.last_seq(),
                        calls,
                        todos: state.todos,
                        reads: state.reads,
                    }));
                }
            }
        };
        run_calls(turn, &events, round, &mut state, &mut pass, &calls, &decisions, round_truncated);
    }
}

/// The chat's cluster as its tools get it; `None` unless one is pinned and
/// its kubeconfig could be read.
pub fn pinned_cluster<'a>(turn: &Turn<'a>) -> Option<PinnedCluster<'a>> {
    match turn.place {
        Place::Chat { kube: KubeSetup::Pinned(target), cluster: Some(api), changes, .. } => Some(PinnedCluster {
            api,
            namespace: &target.namespace,
            kubeconfig: &target.config.name,
            context: &target.context,
            writes: target.writes,
            production: target.config.production,
            changes,
        }),
        _ => None,
    }
}

/// The date the prompt gives, with its weekday: "Tuesday, 13 October 2026" —
/// "remind me on Friday" needs both. The meter asks it too, so what it counts
/// is what is sent.
pub fn today() -> String {
    Local::now().format("%A, %-d %B %Y").to_string()
}

fn tool_message(call_id: &str, content: String) -> LlmMessage {
    LlmMessage {
        role: LlmRole::Tool,
        content: Some(content),
        tool_call_id: Some(call_id.to_string()),
        tool_calls: vec![],
        native_content: None,
        images: Vec::new(),
    }
}

/// A note of the loop's own, into the history as the user's turn — with the
/// reply language said again, see `prompt::with_language_reminder`. Not for
/// what the user typed: that is theirs, in their words.
fn note(turn: &Turn, text: String) -> LlmMessage {
    LlmMessage::user(prompt::with_language_reminder(text, turn.session.reply_language))
}
