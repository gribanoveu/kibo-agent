//! The agent loop and the small decisions a round makes on its own.
//!
//! This file grows in three steps (`docs/06-port-plan.md`, F-1.14): the
//! round-level rules below, then the loop that uses them, then steering. Each
//! rule here is pure and takes no provider, which is what makes it testable
//! without a model at the other end.

use std::cell::Cell;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Local;

use crate::domain::llm::{
    ChatRequest, ChatStreamResult, LlmError, LlmMessage, LlmRole, LlmToolCall, LlmToolDefinition,
    sanitize_tool_call_arguments,
};
use crate::domain::llm_retry::{MAX_ATTEMPTS, retry_delay};
use crate::domain::compaction::{self, RETRY_KEEP_LAST_MESSAGES};
use crate::domain::result_clearing;
use crate::domain::loop_guard::{self, Loop, LoopGuard, Settled};
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
};
use crate::domain::turn::{
    ChatDone, ChatEventPayload, ChatEventSink, ChatStreamOutcome, ChatTurnEvent, DecisionError,
    PendingApproval, PendingToolCall, SteeringNote, ToolCallDecision, ToolCallEvent,
    ToolResultEvent,
};
use crate::infra::llm_debug_log;
use crate::services::ai_tools::parse::{parse_tool_call, preflight_tool_call};
use crate::services::ai_tools::model_text::for_model;
use crate::services::ai_tools::resolve::{relative_to_root, resolve_existing};
use crate::domain::rewind::FileChange;
use crate::services::ai_tools::file_changes;
use crate::services::ai_tools::tools::{execute_tool, tool_definitions};
use crate::services::context_compaction;
use crate::services::llm_session::LlmSession;

/// How many model↔tool round trips one turn may run.
///
/// A backstop beside [`MAX_TOOL_BUDGET`], not a duplicate of it: a tool whose
/// weight is misconfigured to zero would otherwise make the loop unstoppable,
/// and a model stuck in a cycle must not be able to hold the UI in "thinking"
/// forever.
pub const MAX_TOOL_ITERATIONS: usize = 60;

/// The weighted ceiling, and the one that binds in practice. Counted in
/// [`ToolName::loop_weight`] units so that sixty cheap reads and sixty
/// repository-wide searches are not treated as the same amount of work.
pub const MAX_TOOL_BUDGET: u32 = 250;

/// How many times one turn answers an empty reply with a nudge rather than
/// ending. One: a model that says nothing twice running is not going to be
/// talked round by a third note, and each costs a round.
// ponytail: fixed at one; a counter per round if a provider's blank replies
// turn out to come in pairs.
const MAX_EMPTY_NUDGES: u32 = 1;

/// Said to a model whose reply had neither text nor a call. Without it the
/// turn ended there, as if finished: in `agent_bench`, four of GLM's six
/// failures were a blank reply after a round of reads — the task abandoned
/// with nothing said and nothing done.
const EMPTY_REPLY_NOTE: &str = "[Your last reply was empty — no text and no tool call. If the task is not finished, carry on with it; if it is, say what you did.]";

/// The same, when the reply was empty because the response length limit ran
/// out first — usually on thinking.
const EMPTY_TRUNCATED_NOTE: &str = "[Your last reply was cut off by the response length limit before it said anything. Carry on, and think more briefly this time.]";

/// What the model reads instead of a file it already has verbatim, earlier in
/// this same turn.
const REPEAT_READ_NOTE: &str = "This file was already read in this turn, over the same range, and has not changed since — the result is earlier in the conversation and is not repeated here.";

/// The same, for a search that ranked identically.
const REPEAT_SEARCH_NOTE: &str = "This search already ran in this turn with the same parameters and returned the same result — it is earlier in the conversation and is not repeated here.";

/// Appended to a tool error from a round the provider cut off.
const TRUNCATED_ROUND_NOTE: &str = "Note: the model's reply was cut off mid-way — the response length limit (max_tokens) ran out; the arguments were not malformed. Retry the call more compactly: shorter arguments, fewer files or lines at a time, splitting the work across several calls if needed.";

/// The cost of one round: [`ToolName::loop_weight`] over every call it
/// contains, since a round can bundle several.
///
/// A name that is not a tool costs `1` — the floor of the cheapest real tool —
/// so a hallucinated tool name still moves the budget forward instead of
/// letting the loop spin for free. An MCP call costs its server's `weight`.
pub fn round_cost(calls: &[LlmToolCall], mcp: &McpTools) -> u32 {
    calls
        .iter()
        .map(|call| match ToolName::from_wire_name(&call.name) {
            Some(ToolName::Mcp) => mcp.weight(&call.name),
            Some(tool) => tool.loop_weight(),
            None => 1,
        })
        .sum()
}

/// Replaces the body of a result the model has already been given, byte for
/// byte, earlier in this turn.
///
/// Not a correctness fix — re-reading is legitimate, and after a write it is
/// required. It is a context fix: the same file read three times is re-sent in
/// full on every subsequent round of the turn, and so is a search that keeps
/// returning the same hits.
///
/// Keyed on the tool name plus its raw arguments, so a different line range or
/// a different query is a different call, and gated on a hash of the payload,
/// so an edited file — or a search that now ranks differently — comes back in
/// full.
///
/// `result` is `None` for a failed call: an error is short and worth repeating
/// as often as it happens.
pub fn dedupe_repeat_result(
    seen: &mut HashMap<String, u64>,
    call: &LlmToolCall,
    result: Option<&ToolResult>,
    content: String,
) -> String {
    let note = match result {
        Some(ToolResult::File { .. }) => REPEAT_READ_NOTE,
        Some(ToolResult::GrepResults { .. }) => REPEAT_SEARCH_NOTE,
        _ => return content,
    };
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    let hash = hasher.finish();
    match seen.insert(format!("{}|{}", call.name, call.arguments), hash) {
        Some(previous) if previous == hash => note.to_string(),
        _ => content,
    }
}

/// Adds [`TRUNCATED_ROUND_NOTE`] to a failed call from a truncated round.
///
/// `finish_reason: "length"` means generation stopped because the response
/// budget ran out — mid-sentence, and just as easily mid-`arguments`. The
/// model is never told that budget exists, so the severed JSON comes back to
/// it as a bare "invalid arguments" error, which invites the obvious wrong
/// repair: send the same oversized call again.
///
/// Only to a **failed** call: one whose arguments happened to close before the
/// cut still executed correctly, and telling the model its successful result
/// was somehow damaged is worse than saying nothing.
///
/// Takes a plain `failed` flag rather than the outcome, because the two places
/// a tool error becomes content for the model do not share a type — the
/// preflight rejects a call before there is any result to inspect, and severed
/// arguments fail exactly there.
pub fn truncated_round_note(round_truncated: bool, failed: bool, content: String) -> String {
    if round_truncated && failed {
        format!("{content}\n\n{TRUNCATED_ROUND_NOTE}")
    } else {
        content
    }
}

/// Everything one turn needs that is not the conversation itself.
///
/// Cancellation and waiting arrive as functions rather than as a flag and a
/// `thread::sleep`, for the same reason the events do: the loop then runs in a
/// test at full speed, and neither the retry wait nor the stop button needs a
/// real clock to be exercised.
pub struct Turn<'a> {
    pub events: &'a ChatEventSink,
    pub session: &'a LlmSession,
    pub scope: &'a ToolScope,
    pub approval: &'a ApprovalPolicy,
    /// Which tools exist this turn, and what the model is told the
    /// conversation is for. Read once per turn: a mode changed while an
    /// approval card was showing does not rewrite decisions already made.
    pub mode: ConversationMode,
    /// Polled between rounds, after a round streams, between individual calls,
    /// and during a retry wait.
    pub cancelled: &'a dyn Fn() -> bool,
    /// Called in one-second slices while waiting to retry, so a stop takes
    /// effect during the wait rather than after it.
    pub sleep: &'a dyn Fn(Duration),
    /// Which shell runs a command line. A setting, not a search of `PATH` —
    /// see `domain::command_exec`.
    pub shell: &'a Shell,
    /// How the prompt names it — `domain::command_exec::describe_shell`.
    pub shell_described: &'a str,
    /// Takes whatever the user has typed since it was last called. Draining
    /// rather than reading is deliberate: a note handed to the model must
    /// leave the queue in the same step, or a round that is retried or
    /// interrupted can deliver it twice.
    pub take_steering: &'a dyn Fn() -> Vec<SteeringNote>,
    /// Search of the open folder's index, for `semanticSearch`; `None` when
    /// the folder has none, and the tool says so to the model.
    pub search: Option<CodeSearchFn>,
    /// The open folder's skills and the user's, listed in the prompt for
    /// `skill` to load. Read once per turn, so the prompt does not change
    /// between its rounds.
    pub skills: &'a [Skill],
    /// The open folder's instruction files, read once per turn like the skills.
    pub rules: &'a [RuleFile],
    /// Where each settled call's redacted record goes — the log on disk in
    /// the app, a list in a test. A port rather than a direct write so that
    /// a turn under test never touches whatever app directory another test
    /// has installed.
    pub log_call: &'a dyn Fn(ToolCallLogEntry),
    /// The chat's plan as the window holds it — the user's edits included.
    /// Read at the start of the turn: a `writePlan` in this turn reaches the
    /// model through its own call in the history until the next one.
    pub plan: Option<&'a str>,
    /// The main working tree, when the open folder is a worktree of it —
    /// said in the prompt, so the model keeps out of the user's checkout.
    pub worktree_of: Option<&'a std::path::Path>,
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
}



/// Notes typed while a turn is running, waiting for the next round.
///
/// Shared between the turn and whatever accepts the user's typing, so it owns
/// its own lock. A poisoned lock is recovered rather than propagated: losing
/// the queue must not take down a turn that is otherwise fine.
#[derive(Default)]
pub struct SteeringQueue(Mutex<Vec<SteeringNote>>);

impl SteeringQueue {
    pub fn push(&self, note: SteeringNote) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).push(note);
    }

    pub fn take(&self) -> Vec<SteeringNote> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Removes the note with this id, if it is still queued.
    ///
    /// `false` means it is already gone — a round picked it up while the user
    /// was reaching for cancel. What has been said to the model cannot be
    /// unsaid, and the answer is what lets the caller tell "withdrawn" from
    /// "too late".
    pub fn cancel(&self, id: &str) -> bool {
        let mut notes = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let before = notes.len();
        notes.retain(|note| note.id != id);
        notes.len() != before
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TurnError {
    #[error("{0}")]
    Provider(#[from] LlmError),
    #[error("the assistant did not finish within {rounds} rounds of tool calls. Ask it to continue if it still has work to do.")]
    Exhausted { rounds: u32 },
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
            state.history.push(tool_message(&id, "Not run: the user stopped the turn before this call.".to_string()));
        }
    }
    if !result.text.is_empty() {
        state.history.push(LlmMessage::assistant(result.text.clone()));
    }
    ChatDone { result, todos: state.todos, history: state.history }
}

/// The loop both entry points run.
///
/// `resume` carries a round whose calls are already known and already decided;
/// a fresh round asks the model instead. Everything after that point is the
/// same code, which is the reason a paused turn behaves like an ordinary one.
fn run(
    turn: &Turn,
    mut state: State,
    event_seq: u64,
    mut resume: Option<(Vec<LlmToolCall>, Vec<ToolCallDecision>)>,
) -> Result<ChatStreamOutcome, TurnError> {
    let events = Events::new(turn.events, event_seq);
    // `<tool>|<arguments>` → hash of what came back, see `dedupe_repeat_result`.
    // Not part of the checkpoint: after a pause the first repeat of a read
    // comes back in full once more, which costs context and loses nothing.
    let mut seen_results: HashMap<String, u64> = HashMap::new();
    // How often Stop hooks have sent the model back this turn. Not in the
    // checkpoint: a resume is the user's go-ahead, and starts the count over.
    let mut stop_blocks = 0;
    // Empty replies answered with a note this turn; see `MAX_EMPTY_NUDGES`.
    let mut empty_nudges = 0;
    // Rounds going in circles; see `domain::loop_guard`. Not in the
    // checkpoint either, for the same reason as `stop_blocks`.
    let mut guard = LoopGuard::default();
    // The calls of the round before, for the guard.
    let mut settled: Vec<Settled> = Vec::new();
    // Tokens this pass has spent, every request's prompt and reply: what a
    // review is asked to wrap up at. Not in the checkpoint — a resume counts
    // anew, and the note itself, in the history, is what keeps it to once.
    let mut spent: u64 = 0;

    loop {
        // Checkpoint one. Before the ceiling check as well, so a turn the user
        // stopped reports as cancelled rather than as having run out of rounds.
        if (turn.cancelled)() {
            return Ok(ChatStreamOutcome::Cancelled(ended(state, ChatStreamResult::default())));
        }
        if state.round >= MAX_TOOL_ITERATIONS as u32 || state.budget_used >= MAX_TOOL_BUDGET {
            return Err(TurnError::Exhausted {
                rounds: state.round,
            });
        }
        state.round += 1;
        let round = state.round;

        // Per round, not per turn: whether *this* round's reply was cut off.
        // A resumed round has no reply of its own — the round that produced
        // these calls already reported, and whatever note it earned is in the
        // history.
        let mut round_truncated = false;

        let (calls, decisions) = if let Some((calls, decisions)) = resume.take() {
            // Charged again on the resumed pass, exactly as `round` is counted
            // twice: otherwise pausing would be a way to buy budget.
            state.budget_used += round_cost(&calls, turn.mcp);
            (calls, decisions)
        } else {
            // Before the round is announced, so the notes and the boundary
            // land in the transcript in the order the history has them. A
            // round that ran nothing hands in nothing, which ends the streaks.
            if let Some(found) = guard.observe(&std::mem::take(&mut settled)) {
                state.history.push(note(turn, found.note()));
                events.emit(
                    round,
                    Some(format!("loop:{round}")),
                    ChatEventPayload::LoopReminded {
                        tool: found.tool().to_string(),
                        failing: matches!(found, Loop::SameError { .. }),
                    },
                );
            }
            if turn.mode == ConversationMode::Review {
                if let Some(said) = crate::domain::review::wrap_up(round - 1, spent, &state.history) {
                    state.history.push(note(turn, said));
                    events.emit(
                        round,
                        Some(format!("wrap-up:{round}")),
                        ChatEventPayload::WrapUpReminded { rounds: round - 1, tokens: spent },
                    );
                }
            }
            apply_steering(&events, round, &mut state.history, (turn.take_steering)());
            report_ended_processes(turn, &events, round, &mut state.history);
            clear_stale_results(turn.scope, &mut state, &mut seen_results);
            restore_checklist(&mut state.history, &state.todos);
            events.emit(round, Some(format!("round:{round}")), ChatEventPayload::RoundStarted);

            let result = match ask_the_model(
                turn,
                &events,
                round,
                &mut state.history,
                &state.todos,
            )? {
                Some(result) => result,
                // Cancelled during a retry wait.
                None => {
                    return Ok(ChatStreamOutcome::Cancelled(ended(state, ChatStreamResult::default())));
                }
            };

            round_truncated = result.truncated;
            if let Some(usage) = &result.usage {
                spent += u64::from(usage.prompt_tokens) + u64::from(usage.completion_tokens);
            }
            if let Some(usage) = result.usage {
                events.emit(
                    round,
                    Some(format!("round:{round}")),
                    ChatEventPayload::ContextUsage(usage),
                );
            }
            // Said outright rather than left to the deltas that streamed it,
            // and said before the two exits below: a round that is about to be
            // cancelled, or to pause on a confirmation, has still reported
            // what it said.
            events.emit(
                round,
                Some(format!("round:{round}")),
                ChatEventPayload::RoundCompleted {
                    text: result.text.clone(),
                    reasoning: result.reasoning.clone(),
                    truncated: result.truncated,
                },
            );

            // Checkpoint two. Before the pause check and before any call runs,
            // so a stop that landed as the round finished pre-empts the write
            // that round asked for — not merely the model's next sentence.
            if (turn.cancelled)() {
                return Ok(ChatStreamOutcome::Cancelled(ended(state, result)));
            }

            if result.tool_calls.is_empty() {
                // The model is done — unless the user said something while it
                // was answering. Ending the turn here would silently drop
                // what they typed, and they would have no way to tell it was
                // never seen.
                let waiting = (turn.take_steering)();
                // An empty reply is not an ending, and is never kept: an
                // assistant message with neither text nor calls is one
                // providers refuse. What the user typed meanwhile is reason
                // enough to go on; otherwise the model is sent back once. No
                // Stop hook is asked either way — nothing is finishing.
                if result.text.trim().is_empty() && (!waiting.is_empty() || empty_nudges < MAX_EMPTY_NUDGES) {
                    if waiting.is_empty() {
                        empty_nudges += 1;
                        let said = if result.truncated { EMPTY_TRUNCATED_NOTE } else { EMPTY_REPLY_NOTE };
                        state.history.push(note(turn, said.to_string()));
                    }
                    apply_steering(&events, round, &mut state.history, waiting);
                    continue;
                }
                // Only a turn that is really ending asks its Stop hooks; they
                // run every time — one may be a notification — but past the
                // cap a refusal no longer keeps the turn going.
                // A helper's answer is not the turn ending: that is the
                // calling turn's, and its Stop hooks run then.
                let refused = if waiting.is_empty() && turn.mode != ConversationMode::Explore {
                    let fields = serde_json::json!({ "stop_hook_active": stop_blocks > 0 });
                    match fire_hook(turn, &events, round, HookEvent::Stop, None, fields) {
                        Some(_) if stop_blocks >= MAX_STOP_BLOCKS => {
                            report_hook(&events, round, HookEvent::Stop, format!(
                                "Stop hooks kept the turn going {MAX_STOP_BLOCKS} times; it ends here anyway"
                            ), false);
                            None
                        }
                        refused => refused,
                    }
                } else {
                    None
                };
                if waiting.is_empty() && refused.is_none() {
                    return Ok(ChatStreamOutcome::Done(ended(state, result)));
                }
                state.history.push(LlmMessage {
                    role: LlmRole::Assistant,
                    content: (!result.text.is_empty()).then(|| result.text.clone()),
                    tool_call_id: None,
                    tool_calls: vec![],
                    native_content: result.native_content.clone(),
                });
                if let Some(reason) = refused {
                    stop_blocks += 1;
                    state.history.push(note(turn, format!(
                        "[A Stop hook did not let the turn end yet. It said:]\n{reason}"
                    )));
                }
                apply_steering(&events, round, &mut state.history, waiting);
                continue;
            }

            // The assistant's own turn goes back into the history before its
            // results do, so the next request shows the provider its own prior
            // request. `None` content for a tool-only turn is what the wire
            // actually says.
            state.history.push(LlmMessage {
                role: LlmRole::Assistant,
                content: (!result.text.is_empty()).then(|| result.text.clone()),
                tool_call_id: None,
                tool_calls: sanitize_tool_call_arguments(&result.tool_calls),
                native_content: result.native_content.clone(),
            });
            state.budget_used += round_cost(&result.tool_calls, turn.mcp);

            // Containment before approval: a write outside the workspace has
            // to fail as a tool error now, not show the user a card for an
            // operation that cannot happen. Severed arguments fail here too,
            // which is the case the truncation note exists for.
            let mut runnable: Vec<LlmToolCall> = Vec::new();
            for call in &result.tool_calls {
                match preflight_tool_call(turn.scope, turn.mode, &state.reads, call) {
                    Ok(()) => runnable.push(call.clone()),
                    Err(e) => {
                        report_call(&events, round, call);
                        // Refused before running — a path out of the folder,
                        // a write without a read — which is exactly what an
                        // audit trail is read for.
                        let args = parse_tool_call(call).map_or(serde_json::Value::Null, |p| tool_call_log::redact_args(&p));
                        log_call(turn, round, call, args, CallStatus::Error, Some(tool_call_log::redact_error(&e)), None, Instant::now());
                        let message = format!("Error: {e}");
                        settled.push(Settled {
                            tool: call.name.clone(),
                            arguments: call.arguments.clone(),
                            content: message.clone(),
                            error: Some(loop_guard::error_kind(&e)),
                        });
                        report_result(&events, round, &call.id, None, Some(&message), Vec::new());
                        state.history.push(tool_message(
                            &call.id,
                            truncated_round_note(round_truncated, true, message),
                        ));
                    }
                }
            }
            if runnable.is_empty() {
                // Every call in the round was refused before running. Let the
                // model react to the errors on the next round.
                continue;
            }

            let pending: Vec<PendingToolCall> = runnable
                .iter()
                .map(|call| {
                    let (requires_confirmation, reason) = needs_approval(turn.approval, call);
                    PendingToolCall {
                        requires_confirmation,
                        reason,
                        id: call.id.clone(),
                        name: call.name.clone(),
                        arguments: call.arguments.clone(),
                    }
                })
                .collect();
            if pending.iter().any(|call| call.requires_confirmation) {
                // The whole round pauses, including the calls that need no
                // decision: with nothing executed there is no partial round to
                // describe to whoever resumes it.
                return Ok(ChatStreamOutcome::PendingApproval(PendingApproval {
                    history: state.history,
                    round,
                    budget_used: state.budget_used,
                    event_seq: events.last_seq(),
                    calls: pending,
                    todos: state.todos,
                    reads: state.reads,
                }));
            }

            (runnable, Vec::new())
        };

        for call in &calls {
            // The "stopped between two calls of one round" case. Breaking
            // rather than returning here lets checkpoint one build the outcome,
            // so there is one place that decides what a cancelled turn looks
            // like.
            if (turn.cancelled)() {
                break;
            }
            report_call(&events, round, call);

            let decision = decisions.iter().find(|d| d.id == call.id);
            let started = Instant::now();
            // What the log may keep, gathered on the way: the arguments once
            // they have parsed, the error before it is flattened to text.
            let mut logged_args = serde_json::Value::Null;
            let mut logged_error = None;
            let mut error_kind = None;
            let mut changes = Vec::new();
            let denied = matches!(decision, Some(d) if !d.approved);
            let outcome = match decision {
                Some(d) if !d.approved => Err(denial(d)),
                _ => parse_tool_call(call)
                    .and_then(|parsed| {
                        logged_args = tool_call_log::redact_args(&parsed);
                        let fields = serde_json::json!({
                            "tool_name": call.name,
                            "tool_input": serde_json::from_str::<serde_json::Value>(&call.arguments).unwrap_or_default(),
                            "tool_use_id": call.id,
                        });
                        if let Some(reason) = fire_hook(turn, &events, round, HookEvent::PreToolUse, Some(&call.name), fields) {
                            return Err(crate::domain::tools::ToolError::BlockedByHook(reason));
                        }
                        // Built per call, because the id is what pairs a line
                        // of output with the call that produced it — a round
                        // may have started more than one.
                        let output = command_output_sink(turn.events, round, &call.id);
                        let explore = |task: &str| run_explore(turn, task, &output);
                        let deps = ToolDeps {
                            shell: turn.shell.clone(),
                            output: Some(output.clone()),
                            search: turn.search.clone(),
                            skills: turn.skills.to_vec(),
                            mcp: turn.mcp.clone(),
                            cancelled: Some(turn.cancelled),
                            processes: turn.processes.clone(),
                            terminals: turn.terminals.clone(),
                            review: turn.review.clone(),
                            explore: Some(&explore),
                        };
                        // Around the call, not inside the tool: one place sees
                        // every write, and no tool has to remember to report.
                        let watched = file_changes::before(turn.scope, &parsed);
                        let result = execute_tool(
                            turn.scope,
                            &parsed,
                            &mut state.reads,
                            &mut state.todos,
                            &deps,
                        );
                        changes = watched.map(file_changes::after).unwrap_or_default();
                        result
                    })
                    .map_err(|e| {
                        logged_error = Some(tool_call_log::redact_error(&e));
                        error_kind = Some(loop_guard::error_kind(&e));
                        format!("Error: {e}")
                    }),
            };
            // What a hook says about a call that ran goes to the model with
            // its result: the call happened, and cannot be refused any more.
            let post_hook = outcome.as_ref().ok().and_then(|result| {
                let fields = serde_json::json!({
                    "tool_name": call.name,
                    "tool_input": serde_json::from_str::<serde_json::Value>(&call.arguments).unwrap_or_default(),
                    "tool_response": result,
                    "tool_use_id": call.id,
                });
                fire_hook(turn, &events, round, HookEvent::PostToolUse, Some(&call.name), fields)
            });
            let status = match &outcome {
                Ok(_) => CallStatus::Ok,
                Err(_) if denied => CallStatus::Denied,
                Err(_) => CallStatus::Error,
            };
            // A denial's text is the user's own reason, not the tool's.
            let error = if denied { outcome.as_ref().err().cloned() } else { logged_error };
            let result = outcome.as_ref().ok().map(tool_call_log::redact_result);
            log_call(turn, round, call, logged_args, status, error, result, started);

            report_result(
                &events,
                round,
                &call.id,
                outcome.as_ref().ok(),
                outcome.as_ref().err().map(String::as_str),
                changes,
            );

            // What the model reads, as opposed to what the UI was given:
            // plain text in the shape each answer usually takes — see
            // `model_text`.
            let content = match &outcome {
                Ok(result) => for_model(result),
                Err(message) => message.clone(),
            };
            // What came back, before any note is added to it. A denial is the
            // user's answer, not the model going round.
            if !denied {
                settled.push(Settled {
                    tool: call.name.clone(),
                    arguments: call.arguments.clone(),
                    content: content.clone(),
                    error: error_kind,
                });
            }
            let content = truncated_round_note(round_truncated, outcome.is_err(), content);
            let content =
                dedupe_repeat_result(&mut seen_results, call, outcome.as_ref().ok(), content);
            let content = match post_hook {
                Some(said) => format!("{content}\n\n[A PostToolUse hook said:]\n{said}"),
                None => content,
            };
            state.history.push(tool_message(&call.id, content));
        }
    }
}

/// `explore`: `task` as a turn of its own in `ConversationMode::Explore`,
/// from an empty history, returning its closing answer.
///
/// It shares what the calling turn has — the model, the folder, the stop
/// button, the hooks, the log — and nothing of its conversation. Of its
/// events only the calls go out, as lines of the calling card's output, and
/// its usage goes to its record: its text is the result, and its tokens are
/// not this chat's context.
///
/// Recorded in the Agents tab from start to end, and stopped from there too:
/// a helper the user stops is an error the calling turn reads and carries on
/// from, while the chat's own Stop ends both.
///
/// Nothing it is offered asks for approval, so it never pauses; if it did,
/// there would be no card to answer, and it ends as an error instead.
fn run_explore(turn: &Turn, task: &str, progress: &CommandSink) -> Result<ToolResult, crate::domain::tools::ToolError> {
    use crate::domain::agents::AgentState;
    use crate::domain::tools::ToolError;
    // A throwaway record where the app keeps none, so a run is counted the
    // same way either way.
    let agents = turn.agents.clone().unwrap_or_default();
    let id = agents.start(task);
    let events: ChatEventSink = {
        let agents = Arc::clone(&agents);
        let progress = progress.clone();
        Arc::new(move |event: ChatTurnEvent| match event.event {
            ChatEventPayload::ToolCall(call) => {
                let step = explore_step(&call.name, &call.arguments);
                progress(CommandEvent {
                    stream: crate::domain::command_exec::OutputStream::Stdout,
                    chunk: format!("{step}\n"),
                });
                agents.step(id, step);
            }
            ChatEventPayload::ContextUsage(usage) => agents.spent(id, &usage),
            _ => {}
        })
    };
    let stopped = || (turn.cancelled)() || agents.stop_asked(id);
    let no_steering = Vec::new;
    let helper = Turn {
        events: &events,
        session: turn.session,
        scope: turn.scope,
        approval: &ApprovalPolicy::default(),
        mode: ConversationMode::Explore,
        cancelled: &stopped,
        sleep: turn.sleep,
        shell: turn.shell,
        shell_described: turn.shell_described,
        take_steering: &no_steering,
        search: turn.search.clone(),
        skills: turn.skills,
        rules: turn.rules,
        log_call: turn.log_call,
        plan: None,
        worktree_of: turn.worktree_of,
        mcp: &McpTools::default(),
        hooks: turn.hooks,
        // The calling turn's to report when they end.
        processes: None,
        terminals: turn.terminals.clone(),
        review: None,
        agents: None,
    };
    let failed = |reason: String| (AgentState::Failed { reason: reason.clone() }, Err(reason));
    let (state, answer) = match stream(&helper, vec![LlmMessage::user(task)], Vec::new()) {
        Ok(ChatStreamOutcome::Done(done)) if !done.result.text.trim().is_empty() => (AgentState::Done, Ok(done.result.text)),
        Ok(ChatStreamOutcome::Done(_)) => failed("the helper finished without an answer".to_string()),
        Ok(ChatStreamOutcome::Cancelled(_)) if (turn.cancelled)() => {
            (AgentState::Stopped, Err("stopped before it answered".to_string()))
        }
        Ok(ChatStreamOutcome::Cancelled(_)) => (
            AgentState::Stopped,
            Err("the user stopped the helper before it answered. Carry on without it; do not start it again for the same question unless the user asks".to_string()),
        ),
        Ok(ChatStreamOutcome::PendingApproval(_)) => failed("the helper asked for a call that needs approval".to_string()),
        Err(TurnError::Exhausted { rounds }) => failed(format!(
            "the helper used up its {rounds} rounds without answering. Give it a narrower task, or look yourself."
        )),
        Err(e) => failed(e.to_string()),
    };
    agents.finish(id, state, answer.as_ref().ok().cloned());
    let tokens = agents.get(id).map(|run| run.tokens).unwrap_or_default();
    answer.map(|text| ToolResult::Explored { text, agent: id, tokens }).map_err(ToolError::Explore)
}

/// One line of a helper's progress: the tool, and what it was pointed at.
fn explore_step(name: &str, arguments: &str) -> String {
    let args: serde_json::Value = serde_json::from_str(arguments).unwrap_or_default();
    let target = ["path", "pattern", "query"]
        .iter()
        .find_map(|key| args.get(key).and_then(|v| v.as_str()));
    match target {
        Some(target) => format!("{name} {target}"),
        None => name.to_string(),
    }
}

/// Replaces old tool results with stubs once the history has grown — the
/// rules, and why they suit a prompt cache, are `domain::result_clearing`'s —
/// and makes the turn forget them as well. A repeat of a cleared call must
/// come back in full rather than as "already above", and a file whose text
/// the model no longer has must be read again before it is replaced whole.
fn clear_stale_results(scope: &ToolScope, state: &mut State, seen_results: &mut HashMap<String, u64>) {
    for cleared in result_clearing::plan(&state.history) {
        state.history[cleared.index].content = Some(cleared.stub);
        seen_results.remove(&format!("{}|{}", cleared.tool, cleared.arguments));
        if ToolName::from_wire_name(&cleared.tool) != Some(ToolName::ReadFile) {
            continue;
        }
        // A path that no longer resolves has nothing left to forget.
        let path = serde_json::from_str::<serde_json::Value>(&cleared.arguments)
            .ok()
            .and_then(|args| args.get("path").and_then(|p| p.as_str()).map(str::to_string));
        let relative = path.and_then(|path| {
            resolve_existing(scope, &path).and_then(|resolved| relative_to_root(scope, &resolved)).ok()
        });
        if let Some(relative) = relative {
            state.reads.forget_whole(&relative);
        }
    }
}

/// Turns a command's output into turn events as it arrives.
///
/// Deliberately not routed through [`Events`]: that cursor is owned by the
/// loop's own thread, and output arrives on the runner's reader threads. These
/// events carry no sequence number of their own and are ordered by the call
/// they belong to, which is what a listener uses to append them to the right
/// card. The call's `ToolResult` remains the authoritative text.
fn command_output_sink(events: &ChatEventSink, round: u32, call_id: &str) -> CommandSink {
    let events = events.clone();
    let target_id = format!("round:{round}:tool:{call_id}");
    let call_id = call_id.to_string();
    Arc::new(move |event: CommandEvent| {
        events(ChatTurnEvent {
            seq: 0,
            round,
            target_id: Some(target_id.clone()),
            event: ChatEventPayload::CommandOutput {
                id: call_id.clone(),
                stream: event.stream,
                chunk: event.chunk,
            },
        });
    })
}

/// Adds queued notes to the conversation and says so, one event per note, so
/// the front end can retire each by id rather than by matching its text.
fn apply_steering(
    events: &Events,
    round: u32,
    history: &mut Vec<LlmMessage>,
    notes: Vec<SteeringNote>,
) {
    for note in notes {
        history.push(LlmMessage::user(note.prefixed()));
        events.emit(
            round,
            Some(format!("steer:{}", note.id)),
            ChatEventPayload::SteeringApplied {
                id: note.id,
                text: note.shown.unwrap_or(note.text),
            },
        );
    }
}

/// One round against the provider, retried while [`retry_delay`] allows it.
///
/// `Ok(None)` means the turn was cancelled during a wait — the caller turns
/// that into the same cancelled outcome as every other stopping point.
/// One round's request, made once more against a shorter history if the
/// provider says the conversation no longer fits.
///
/// The compaction is reactive on purpose: it happens because a request was
/// actually refused, not because an estimate guessed it would be. Once, and
/// only once — a second refusal after the history has already been summarized
/// is not about the history's length, and summarizing again would spend
/// another request to lose more of the conversation for nothing.
///
/// A pass that cannot help leaves `history` alone and the original refusal is
/// what the turn reports: "this conversation does not fit" is the useful
/// thing to read, and "the summarizer also failed" is not.
fn ask_the_model(
    turn: &Turn,
    events: &Events,
    round: u32,
    history: &mut Vec<LlmMessage>,
    todos: &[Task],
) -> Result<Option<ChatStreamResult>, TurnError> {
    let mut compacted = false;
    loop {
        let request = ChatRequest {
            messages: request_messages(turn, history),
            tools: tool_definitions_for(turn.mode, turn.mcp),
            model: turn.session.model.clone(),
        };
        let error = match stream_one_round(turn, events, round, request) {
            Ok(result) => return Ok(result),
            Err(TurnError::Provider(error)) if !compacted && too_long(&error) => error,
            Err(other) => return Err(other),
        };
        compacted = true;

        // Harder than a proactive pass would: the window is not nearly full,
        // it is already over.
        let started = || {
            events.emit(round, Some(format!("round:{round}")), ChatEventPayload::HistoryCompacting)
        };
        match context_compaction::compact(turn.session, history, RETRY_KEEP_LAST_MESSAGES, &started) {
            Ok(Some(shorter)) => {
                *history = shorter.history;
                // The summary may have folded the last list away.
                restore_checklist(history, todos);
                events.emit(
                    round,
                    Some(format!("round:{round}")),
                    ChatEventPayload::HistoryCompacted {
                        folded: shorter.folded,
                    },
                );
            }
            // Nothing could be folded, or the summarizer itself failed: report
            // what the model actually refused.
            Ok(None) | Err(_) => return Err(TurnError::Provider(error)),
        }
    }
}

/// What this mode advertises. Leaving a tool out of the request is the half
/// of the gate the model can see; [`preflight_tool_call`] is the half it
/// cannot, and both are needed — a model that used `writeFile` earlier in a
/// conversation calls it again from memory when the mode narrows.
pub(crate) fn tool_definitions_for(mode: ConversationMode, mcp: &McpTools) -> Vec<LlmToolDefinition> {
    tool_definitions()
        .into_iter()
        .chain(mcp.definitions())
        .filter(|definition| {
            ToolName::from_wire_name(&definition.name)
                .is_some_and(|tool| conversation_mode::offers(mode, tool))
        })
        .collect()
}

/// What a request of `turn` with `history` weighs, by the estimate the
/// compaction pass uses — for a caller that shows a cost before the provider
/// reports one.
pub fn estimate_request(turn: &Turn, history: &[LlmMessage]) -> usize {
    compaction::estimate_tokens(&request_messages(turn, history))
        + compaction::estimate_tool_schema_tokens(&tool_definitions_for(turn.mode, turn.mcp))
}

/// The request's messages: what the model is told, then the conversation.
/// Nothing after it — the checklist lives in the history
/// ([`restore_checklist`]), so each request extends the last.
///
/// The system part is rebuilt every round rather than pushed into `history`
/// once: a folder or a date frozen into the stored conversation would be
/// resent, wrong, for as long as the chat exists. It is the same every round
/// of a turn, so a prompt cache keeps it.
fn request_messages(turn: &Turn, history: &[LlmMessage]) -> Vec<LlmMessage> {
    let context = prompt::TurnContext {
        mode: turn.mode,
        workspace: turn.scope.root(),
        shell: turn.shell_described,
        today: &Local::now().format("%e %B %Y").to_string(),
        unattended: turn.approval.skip_all,
        skills: turn.skills,
        rules: turn.rules,
        plan: turn.plan,
        worktree_of: turn.worktree_of,
        language: turn.session.reply_language,
    };
    let mut messages = prompt::system_messages(&context);
    messages.extend_from_slice(history);
    messages
}

/// Puts the checklist back into the history when the history no longer shows
/// it as it stands — a summary folded the last `todo` result away, or a
/// branched chat cut it off while the list carried on — and otherwise leaves
/// the history alone.
///
/// Into the history, once, rather than onto the end of every request: the
/// history is only ever appended to, so each request is the previous one plus
/// what happened since, which is what every provider's prompt cache matches.
/// A checklist re-sent at the end of each request was a different tail every
/// time, and OpenAI's cache — which reuses whole earlier requests — kept
/// nothing past the system prompt: 6k of a 40k history (`agent_bench`, GPT via
/// OpenRouter). Codex does the same with its plan tool, and never re-sends it.
///
/// Onto the last tool result when that is where the history ends — it has not
/// been sent yet — and otherwise as a message of its own: a user's message is
/// left as they wrote it, since a branched chat finds it by its text.
fn restore_checklist(history: &mut Vec<LlmMessage>, todos: &[Task]) {
    if todos.is_empty() || history_shows(history, todos) {
        return;
    }
    let note = format!("[The checklist as it stands]\n{}", for_model(&ToolResult::Todo { tasks: todos.to_vec() }));
    match history.last_mut() {
        Some(last) if last.role == LlmRole::Tool => {
            let content = last.content.get_or_insert_with(String::new);
            content.push_str("\n\n");
            content.push_str(&note);
        }
        _ => history.push(LlmMessage::user(note)),
    }
}

/// Whether the latest checklist in the history — a `todo` result, or one put
/// back by [`restore_checklist`] — is the list exactly as it stands.
///
/// The latest, not any: a stale list is worse than none. The list only
/// changes through `todo`, whose every result shows it whole.
fn history_shows(history: &[LlmMessage], todos: &[Task]) -> bool {
    let current = for_model(&ToolResult::Todo { tasks: todos.to_vec() });
    history
        .iter()
        .rev()
        .filter_map(|m| m.content.as_deref())
        .find(|content| content.contains(CHECKLIST_LEGEND))
        .is_some_and(|shown| shows_exactly(shown, &current))
}

/// `list` appears in `content` whole: followed by nothing, or by a blank line
/// before whatever was added after it — never by another row, or a shorter
/// list would pass for the longer one it begins.
fn shows_exactly(content: &str, list: &str) -> bool {
    content.match_indices(list).any(|(at, _)| {
        let rest = &content[at + list.len()..];
        rest.is_empty() || rest.starts_with("\n\n")
    })
}

fn too_long(error: &LlmError) -> bool {
    compaction::is_context_length_error(&error.to_string())
}

fn stream_one_round(
    turn: &Turn,
    events: &Events,
    round: u32,
    request: ChatRequest,
) -> Result<Option<ChatStreamResult>, TurnError> {
    let mut attempt = 0;
    loop {
        llm_debug_log::log_request(turn.session.debug_logging, &turn.session.provider_id, round, &request);

        // Set by any callback below: once a byte of this attempt has reached
        // us, the round is no longer repeatable.
        let produced_output = Cell::new(false);
        let on_delta = |delta: &str| {
            produced_output.set(true);
            events.emit(
                round,
                Some(format!("round:{round}:text")),
                ChatEventPayload::Delta {
                    delta: delta.to_string(),
                },
            );
        };
        let on_reasoning = |delta: &str| {
            produced_output.set(true);
            events.emit(
                round,
                Some(format!("round:{round}:reasoning")),
                ChatEventPayload::Reasoning {
                    delta: delta.to_string(),
                },
            );
        };
        let on_tool_call_delta = |id: &str, name: &str, arguments: &str| {
            produced_output.set(true);
            events.emit(
                round,
                Some(format!("round:{round}:tool:{id}")),
                ChatEventPayload::ToolCallDelta(ToolCallEvent {
                    id: id.to_string(),
                    name: name.to_string(),
                    arguments: arguments.to_string(),
                }),
            );
        };

        let result = turn.session.provider.chat_stream(
            request.clone(),
            &on_delta,
            &on_reasoning,
            &on_tool_call_delta,
            turn.cancelled,
        );
        llm_debug_log::log_response(turn.session.debug_logging, &turn.session.provider_id, round, &result);

        let error = match result {
            Ok(result) => return Ok(Some(result)),
            Err(error) => error,
        };
        let Some(delay) = retry_delay(&error, attempt, produced_output.get()) else {
            return Err(TurnError::Provider(error));
        };
        attempt += 1;
        events.emit(
            round,
            Some(format!("round:{round}")),
            ChatEventPayload::Retrying {
                attempt,
                max_attempts: MAX_ATTEMPTS,
                delay_seconds: delay.as_secs(),
            },
        );
        if !wait(turn, delay) {
            return Ok(None);
        }
    }
}

/// Sleeps in one-second slices, checking for a stop between them. `false` if
/// the turn was cancelled before the wait was over — a minute-long wait that
/// ignored the stop button would look exactly like a hang.
fn wait(turn: &Turn, delay: Duration) -> bool {
    let slice = Duration::from_secs(1);
    let mut left = delay;
    while !left.is_zero() {
        if (turn.cancelled)() {
            return false;
        }
        let step = left.min(slice);
        (turn.sleep)(step);
        left -= step;
    }
    !(turn.cancelled)()
}

/// Whether this call has to be shown to a human first.
///
/// An unparseable call is never risky: it cannot run, and asking about a call
/// that is going to fail either way spends the user's attention on nothing.
fn needs_approval(policy: &ApprovalPolicy, call: &LlmToolCall) -> (bool, Option<String>) {
    match parse_tool_call(call) {
        Ok(parsed) if policy.requires_approval_for(&parsed) => (true, policy.approval_reason(&parsed)),
        _ => (false, None),
    }
}

/// What a refused call tells the model. The reason is the point: a model told
/// only "denied" tries the same call again, then a near variant of it.
/// One settled call, as the log keeps it. `args`, `error` and `result`
/// arrive already redacted.
#[allow(clippy::too_many_arguments)]
fn log_call(
    turn: &Turn,
    round: u32,
    call: &LlmToolCall,
    args: serde_json::Value,
    status: CallStatus,
    error: Option<String>,
    result: Option<serde_json::Value>,
    started: Instant,
) {
    (turn.log_call)(ToolCallLogEntry {
        ts_ms: crate::infra::tool_call_log::now_ms(),
        repo_root: turn.scope.root().display().to_string(),
        round,
        provider_id: turn.session.provider_id.clone(),
        model: turn.session.model.clone(),
        tool: call.name.clone(),
        args,
        status,
        error,
        result,
        duration_ms: started.elapsed().as_millis() as i64,
    });
}

fn denial(decision: &ToolCallDecision) -> String {
    match &decision.reason {
        Some(reason) if !reason.trim().is_empty() => {
            format!("Denied by the user: {reason}")
        }
        _ => "Denied by the user.".to_string(),
    }
}

fn tool_message(call_id: &str, content: String) -> LlmMessage {
    LlmMessage {
        role: LlmRole::Tool,
        content: Some(content),
        tool_call_id: Some(call_id.to_string()),
        tool_calls: vec![],
        native_content: None,
    }
}

/// A note of the loop's own, into the history as the user's turn — with the
/// reply language said again, see `prompt::with_language_reminder`. Not for
/// what the user typed: that is theirs, in their words.
fn note(turn: &Turn, text: String) -> LlmMessage {
    LlmMessage::user(prompt::with_language_reminder(text, turn.session.reply_language))
}

/// Tells the model which background processes ended since it last looked —
/// a dev server that died is otherwise invisible until something fails
/// against its port. Into the history rather than a tail note: a retried
/// request must not lose it, and it is said once.
fn report_ended_processes(turn: &Turn, events: &Events, round: u32, history: &mut Vec<LlmMessage>) {
    let Some(processes) = &turn.processes else { return };
    let ended = processes.take_ended();
    let Some(said) = background::ended_note(&ended) else { return };
    history.push(note(turn, said));
    events.emit(round, None, ChatEventPayload::ProcessesEnded { processes: ended });
}

/// Runs the hooks of one event and reports what they said; returns the
/// reason when they refused.
fn fire_hook(
    turn: &Turn,
    events: &Events,
    round: u32,
    event: HookEvent,
    tool: Option<&str>,
    fields: serde_json::Value,
) -> Option<String> {
    let verdict = turn.hooks.fire(event, tool, fields, turn.scope.root());
    for warning in verdict.warnings {
        report_hook(events, round, event, warning, false);
    }
    if let Some(reason) = &verdict.blocked {
        report_hook(events, round, event, reason.clone(), true);
    }
    verdict.blocked
}

fn report_hook(events: &Events, round: u32, event: HookEvent, message: String, blocked: bool) {
    events.emit(
        round,
        None,
        ChatEventPayload::HookFeedback { event: event.name().to_string(), message, blocked },
    );
}

fn report_call(events: &Events, round: u32, call: &LlmToolCall) {
    events.emit(
        round,
        Some(format!("round:{round}:tool:{}", call.id)),
        ChatEventPayload::ToolCall(ToolCallEvent {
            id: call.id.clone(),
            name: call.name.clone(),
            arguments: call.arguments.clone(),
        }),
    );
}

fn report_result(
    events: &Events,
    round: u32,
    call_id: &str,
    result: Option<&ToolResult>,
    error: Option<&str>,
    changes: Vec<FileChange>,
) {
    events.emit(
        round,
        Some(format!("round:{round}:tool:{call_id}")),
        ChatEventPayload::ToolResult(ToolResultEvent {
            id: call_id.to_string(),
            result: result.cloned(),
            error: error.map(str::to_string),
            changes,
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::domain::llm::{
        ChatResponse, ChatUsage, LlmModelInfo, LlmProvider, LlmToolDefinition,
    };
    use crate::domain::tools::{ToolScope, ToolName};
    use crate::domain::turn::{ChatTurnEvent, STEERING_PREFIX};
    use crate::testing::temp_dir;
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    // ---------------------------------------------------------------- doubles

    /// One scripted answer from the provider.
    enum Step {
        Reply(ChatStreamResult),
        Fail(LlmError),
        /// Streams text and *then* fails — the shape that must never be
        /// retried, since half the round has already been reported.
        StreamThenFail(&'static str, LlmError),
        /// Answers, and the user types while it does. The only way to queue a
        /// note *during* a turn when the provider is synchronous.
        ReplyWhileTheUserTypes(ChatStreamResult, &'static str),
    }

    fn text_while_typing(answer: &str, note: &'static str) -> Step {
        Step::ReplyWhileTheUserTypes(
            ChatStreamResult {
                text: answer.to_string(),
                ..Default::default()
            },
            note,
        )
    }

    fn asks_while_typing(calls: Vec<LlmToolCall>, note: &'static str) -> Step {
        Step::ReplyWhileTheUserTypes(
            ChatStreamResult {
                tool_calls: calls,
                ..Default::default()
            },
            note,
        )
    }

    fn text(answer: &str) -> Step {
        Step::Reply(ChatStreamResult {
            text: answer.to_string(),
            ..Default::default()
        })
    }

    fn asks(calls: Vec<LlmToolCall>) -> Step {
        Step::Reply(ChatStreamResult {
            tool_calls: calls,
            ..Default::default()
        })
    }

    fn wants(id: &str, name: &str, arguments: &str) -> LlmToolCall {
        LlmToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments: arguments.to_string(),
        }
    }

    struct Scripted {
        steps: Mutex<VecDeque<Step>>,
        requests: Mutex<Vec<ChatRequest>>,
        steering: Mutex<Option<Arc<SteeringQueue>>>,
        /// Summarizing requests, which arrive unstreamed and out of band —
        /// kept apart from `requests` so a test can say how many rounds there
        /// were without counting them.
        summaries: Mutex<Vec<ChatRequest>>,
    }

    impl Scripted {
        fn new(steps: Vec<Step>) -> Arc<Self> {
            Arc::new(Self {
                steps: Mutex::new(steps.into()),
                requests: Mutex::new(Vec::new()),
                steering: Mutex::new(None),
                summaries: Mutex::new(Vec::new()),
            })
        }

        fn requests(&self) -> Vec<ChatRequest> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl LlmProvider for Scripted {
        /// Only compaction gets here: the loop itself always streams.
        fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
            self.summaries.lock().unwrap().push(request);
            Ok(ChatResponse {
                content: Some("they were fixing the parser".to_string()),
                tool_calls: Vec::new(),
                usage: None,
            })
        }

        fn chat_stream(
            &self,
            request: ChatRequest,
            on_delta: &dyn Fn(&str),
            _: &dyn Fn(&str),
            _: &dyn Fn(&str, &str, &str),
            _: &dyn Fn() -> bool,
        ) -> Result<ChatStreamResult, LlmError> {
            self.requests.lock().unwrap().push(request);
            let step = self
                .steps
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| panic!("the loop asked for more rounds than the script has"));
            match step {
                Step::Reply(result) => {
                    if !result.text.is_empty() {
                        on_delta(&result.text);
                    }
                    Ok(result)
                }
                Step::Fail(error) => Err(error),
                Step::StreamThenFail(chunk, error) => {
                    on_delta(chunk);
                    Err(error)
                }
                Step::ReplyWhileTheUserTypes(result, note) => {
                    if let Some(queue) = self.steering.lock().unwrap().as_ref() {
                        queue.push(SteeringNote::user(note));
                    }
                    if !result.text.is_empty() {
                        on_delta(&result.text);
                    }
                    Ok(result)
                }
            }
        }

        fn list_models(&self) -> Result<Vec<LlmModelInfo>, LlmError> {
            unreachable!("the loop never lists models")
        }
    }

    /// Everything a turn needs, with the pieces a test wants to reach back
    /// into kept out here.
    struct Harness {
        provider: Arc<Scripted>,
        session: LlmSession,
        scope: ToolScope,
        root: PathBuf,
        events: ChatEventSink,
        log: Arc<Mutex<Vec<ChatTurnEvent>>>,
        approval: ApprovalPolicy,
        mode: ConversationMode,
        cancel_after: Arc<Mutex<Option<usize>>>,
        polls: Arc<Mutex<usize>>,
        slept: Arc<Mutex<Vec<Duration>>>,
        steering: Arc<SteeringQueue>,
        search: Option<CodeSearchFn>,
        skills: Vec<Skill>,
        rules: Vec<RuleFile>,
        logged: Arc<Mutex<Vec<ToolCallLogEntry>>>,
        plan: Option<String>,
        worktree_of: Option<PathBuf>,
        mcp: McpTools,
        hooks: Hooks,
        processes: Option<Arc<dyn BackgroundProcesses>>,
        terminals: Option<Arc<dyn crate::domain::terminal::UserTerminals>>,
        shell_described: String,
        agents: Option<Arc<crate::domain::agents::Agents>>,
    }

    fn harness(label: &str, steps: Vec<Step>) -> Harness {
        let root = temp_dir(label);
        let provider = Scripted::new(steps);
        let log: Arc<Mutex<Vec<ChatTurnEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = log.clone();
        let steering = Arc::new(SteeringQueue::default());
        *provider.steering.lock().unwrap() = Some(steering.clone());
        Harness {
            session: LlmSession {
                provider: provider.clone(),
                provider_id: "test".to_string(),
                model: "m".to_string(),
                debug_logging: false,
                context_limit: None,
                reply_language: None,
            },
            provider,
            scope: ToolScope::new(&root).expect("a scope over the temp root"),
            root,
            events: Arc::new(move |event| sink.lock().unwrap().push(event)),
            log,
            // Unattended by default: the approval gate has its own tests, and
            // every other test would otherwise pause on its first write.
            approval: ApprovalPolicy {
                skip_all: true,
                ..ApprovalPolicy::default()
            },
            mode: ConversationMode::Agent,
            cancel_after: Arc::new(Mutex::new(None)),
            polls: Arc::new(Mutex::new(0)),
            slept: Arc::new(Mutex::new(Vec::new())),
            steering,
            search: None,
            skills: Vec::new(),
            rules: Vec::new(),
            logged: Arc::new(Mutex::new(Vec::new())),
            plan: None,
            worktree_of: None,
            mcp: McpTools::default(),
            hooks: Hooks::default(),
            processes: None,
            terminals: None,
            shell_described: "/bin/sh".to_string(),
            agents: None,
        }
    }

    impl Harness {
        /// Reports "cancelled" from the `n`-th poll onwards, which is how a
        /// test picks the checkpoint it wants to stop at.
        fn cancel_at_poll(&self, n: usize) {
            *self.cancel_after.lock().unwrap() = Some(n);
        }

        fn events(&self) -> Vec<ChatTurnEvent> {
            self.log.lock().unwrap().clone()
        }

        fn run<T>(&self, f: impl FnOnce(&Turn) -> T) -> T {
            let cancel_after = self.cancel_after.clone();
            let polls = self.polls.clone();
            let cancelled = move || {
                let mut polls = polls.lock().unwrap();
                *polls += 1;
                matches!(*cancel_after.lock().unwrap(), Some(n) if *polls >= n)
            };
            let slept = self.slept.clone();
            let sleep = move |d: Duration| slept.lock().unwrap().push(d);
            let shell = Shell::default();
            let queue = self.steering.clone();
            let take_steering = move || queue.take();
            let logged = self.logged.clone();
            let log_call = move |entry: ToolCallLogEntry| logged.lock().unwrap().push(entry);
            let turn = Turn {
                events: &self.events,
                session: &self.session,
                scope: &self.scope,
                approval: &self.approval,
                mode: self.mode,
                cancelled: &cancelled,
                sleep: &sleep,
                take_steering: &take_steering,
                search: self.search.clone(),
                shell: &shell,
                shell_described: &self.shell_described,
                skills: &self.skills,
                rules: &self.rules,
                log_call: &log_call,
                plan: self.plan.as_deref(),
                worktree_of: self.worktree_of.as_deref(),
                mcp: &self.mcp,
                hooks: &self.hooks,
                processes: self.processes.clone(),
                terminals: self.terminals.clone(),
                review: None,
                agents: self.agents.clone(),
            };
            f(&turn)
        }
    }

    fn payloads(events: &[ChatTurnEvent]) -> Vec<String> {
        events
            .iter()
            .map(|e| match &e.event {
                ChatEventPayload::Delta { .. } => "delta".to_string(),
                ChatEventPayload::Reasoning { .. } => "reasoning".to_string(),
                ChatEventPayload::Retrying { .. } => "retrying".to_string(),
                ChatEventPayload::RoundStarted => "roundStarted".to_string(),
                ChatEventPayload::RoundCompleted { .. } => "roundCompleted".to_string(),
                ChatEventPayload::ToolCallDelta(_) => "toolCallDelta".to_string(),
                ChatEventPayload::ToolCall(c) => format!("toolCall:{}", c.id),
                ChatEventPayload::ToolResult(r) => format!("toolResult:{}", r.id),
                ChatEventPayload::ContextUsage(_) => "contextUsage".to_string(),
                ChatEventPayload::SteeringApplied { id, .. } => format!("steering:{id}"),
                ChatEventPayload::CommandOutput { id, .. } => format!("commandOutput:{id}"),
                ChatEventPayload::HistoryCompacted { folded } => format!("compacted:{folded}"),
                ChatEventPayload::HistoryCompacting => "compacting".to_string(),
                ChatEventPayload::HookFeedback { event, blocked, .. } => format!("hook:{event}:{blocked}"),
                ChatEventPayload::ProcessesEnded { processes } => format!("ended:{}", processes.len()),
                ChatEventPayload::LoopReminded { tool, failing } => format!("loop:{tool}:{failing}"),
                ChatEventPayload::WrapUpReminded { rounds, .. } => format!("wrap-up:{rounds}"),
            })
            .collect()
    }

    fn tool_contents(request: &ChatRequest) -> Vec<String> {
        request
            .messages
            .iter()
            .filter(|m| m.role == LlmRole::Tool)
            .filter_map(|m| m.content.clone())
            .collect()
    }


    fn call(name: &str, arguments: &str) -> LlmToolCall {
        LlmToolCall {
            id: "call_1".to_string(),
            name: name.to_string(),
            arguments: arguments.to_string(),
        }
    }

    fn file(content: &str) -> ToolResult {
        ToolResult::File {
            content: content.to_string(),
            start_line: 1,
            end_line: 1,
            total_lines: 1,
            clamped: false,
            truncated: false,
        }
    }

    fn grep() -> ToolResult {
        ToolResult::GrepResults { matches: vec![], truncated: false, total: 0, total_files: 0, total_is_floor: false, skipped: vec![] }
    }

    #[test]
    fn a_round_costs_the_weight_of_every_call_in_it() {
        let cost = round_cost(&[call("readFile", "{}"), call("grep", "{}")], &McpTools::default());
        assert_eq!(cost, ToolName::ReadFile.loop_weight() + ToolName::Grep.loop_weight());
    }

    #[test]
    fn a_round_with_no_calls_is_free() {
        assert_eq!(round_cost(&[], &McpTools::default()), 0);
    }

    /// Otherwise a model that keeps inventing tool names spins against a
    /// budget that never moves.
    #[test]
    fn an_invented_tool_name_still_costs_something() {
        assert_eq!(round_cost(&[call("summonDragon", "{}")], &McpTools::default()), 1);
    }

    #[test]
    fn the_same_read_twice_is_replaced_by_a_note() {
        let mut seen = HashMap::new();
        let call = call("readFile", r#"{"path":"a.rs"}"#);

        let first = dedupe_repeat_result(&mut seen, &call, Some(&file("x")), "x".to_string());
        let second = dedupe_repeat_result(&mut seen, &call, Some(&file("x")), "x".to_string());

        assert_eq!(first, "x");
        assert_eq!(second, REPEAT_READ_NOTE);
    }

    /// The gate that makes this safe after a write: the file changed, so the
    /// model must see it, however many times it has read that path before.
    #[test]
    fn a_changed_file_comes_back_in_full() {
        let mut seen = HashMap::new();
        let call = call("readFile", r#"{"path":"a.rs"}"#);

        dedupe_repeat_result(&mut seen, &call, Some(&file("before")), "before".to_string());
        let after =
            dedupe_repeat_result(&mut seen, &call, Some(&file("after")), "after".to_string());
        assert_eq!(after, "after");

        // And the next identical read is deduped against the *new* content,
        // not the one from before the write.
        let again =
            dedupe_repeat_result(&mut seen, &call, Some(&file("after")), "after".to_string());
        assert_eq!(again, REPEAT_READ_NOTE);
    }

    #[test]
    fn a_different_range_or_query_is_a_different_call() {
        let mut seen = HashMap::new();
        let body = "x".to_string();

        let first = call("readFile", r#"{"path":"a.rs","startLine":1}"#);
        let second = call("readFile", r#"{"path":"a.rs","startLine":40}"#);
        dedupe_repeat_result(&mut seen, &first, Some(&file("x")), body.clone());
        let other = dedupe_repeat_result(&mut seen, &second, Some(&file("x")), body.clone());

        assert_eq!(other, "x", "same content, but the model asked for something else");
    }

    #[test]
    fn a_repeated_search_is_replaced_by_its_own_note() {
        let mut seen = HashMap::new();
        let call = call("grep", r#"{"pattern":"fn main"}"#);

        dedupe_repeat_result(&mut seen, &call, Some(&grep()), "hits".to_string());
        let second = dedupe_repeat_result(&mut seen, &call, Some(&grep()), "hits".to_string());

        assert_eq!(second, REPEAT_SEARCH_NOTE);
    }

    fn task(id: &str, title: &str, status: crate::domain::tools::TodoStatus) -> Task {
        Task { id: id.into(), title: title.into(), status, note: None }
    }

    fn mentions_checklist(message: &LlmMessage) -> bool {
        message.content.as_deref().is_some_and(|c| c.contains(CHECKLIST_LEGEND))
    }

    /// Each request is the previous one plus what happened since — nothing
    /// re-sent at the end, which is what every prompt cache matches. The list
    /// the model works from is the last `todo` result.
    #[test]
    fn with_a_todo_result_in_the_history_nothing_is_added_and_each_request_extends_the_last() {
        let h = harness(
            "chat-checklist-prefix",
            vec![
                asks(vec![wants("t", "todo", r#"{"op":"write","tasks":["look","fix"]}"#)]),
                asks(vec![wants("l", "listFiles", "{}")]),
                asks(vec![wants("u", "todo", r#"{"op":"update","status":"completed"}"#)]),
                text("done"),
            ],
        );
        h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");
        let requests = h.provider.requests();
        for pair in requests.windows(2) {
            assert!(pair[1].messages.starts_with(&pair[0].messages), "a request is a prefix of the next");
        }
        let with_list: Vec<usize> = conversation_of(&requests[3])
            .iter()
            .enumerate()
            .filter(|(_, m)| mentions_checklist(m))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(with_list.len(), 2, "the two todo results, nothing else: {with_list:?}");
    }

    /// A list the history does not show — no result at all, as after a
    /// summary — goes into the history once, beside the user's message, and
    /// stays there: the next request extends it.
    #[test]
    fn a_list_the_history_does_not_show_goes_into_it_once() {
        use crate::domain::tools::TodoStatus::{InProgress, Pending};
        let todos = vec![task("t1", "look", InProgress), task("t2", "fix", Pending)];
        let h = harness("chat-checklist-restored", vec![asks(vec![wants("l", "listFiles", "{}")]), text("ok")]);

        let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], todos.clone())).expect("turn");

        let requests = h.provider.requests();
        let first = conversation_of(&requests[0]);
        assert_eq!(first[0].content.as_deref(), Some("go"), "the user's words untouched");
        assert!(first.last().is_some_and(|m| m.role == LlmRole::User && mentions_checklist(m)));
        assert!(requests[1].messages.starts_with(&requests[0].messages));
        assert_eq!(conversation_of(&requests[1]).iter().filter(|m| mentions_checklist(m)).count(), 1, "once");
        let ChatStreamOutcome::Done(done) = outcome else { panic!() };
        assert_eq!(done.history.iter().filter(|m| mentions_checklist(m)).count(), 1, "kept for the next turn");
    }

    /// The latest list in the history is an older one — a branched chat —
    /// and the history ends with a result not yet sent: the list joins it.
    #[test]
    fn a_stale_list_is_superseded_on_the_unsent_tool_result() {
        use crate::domain::tools::TodoStatus::{Completed, InProgress};
        let todos = vec![task("t1", "look", Completed), task("t2", "fix", InProgress)];
        let history = vec![
            LlmMessage::user("go"),
            LlmMessage { tool_calls: vec![wants("old", "todo", "{}")], ..LlmMessage::assistant("") },
            LlmMessage::tool_result("old", for_model(&ToolResult::Todo { tasks: vec![task("t1", "look", InProgress)] })),
            LlmMessage { tool_calls: vec![wants("l", "listFiles", "{}")], ..LlmMessage::assistant("") },
            LlmMessage::tool_result("l", "./\n└── a.txt"),
        ];
        let h = harness("chat-checklist-stale", vec![text("ok")]);

        h.run(|turn| stream(turn, history, todos.clone())).expect("turn");

        let last = conversation_of(&h.provider.requests()[0]).last().unwrap().clone();
        assert_eq!(last.role, LlmRole::Tool);
        let content = last.content.unwrap();
        assert!(content.starts_with("./\n└── a.txt\n\n[The checklist as it stands]"), "{content}");
        assert!(content.contains(&for_model(&ToolResult::Todo { tasks: todos })));
    }

    /// The latest list is what counts, not whether the current one appears
    /// somewhere: an earlier result showing it is overruled by a later one
    /// that shows something else.
    #[test]
    fn an_earlier_copy_of_the_list_does_not_count_once_a_later_one_differs() {
        use crate::domain::tools::TodoStatus::{InProgress, Pending};
        let todos = vec![task("t1", "look", InProgress)];
        let other = vec![task("t1", "look", InProgress), task("t2", "fix", Pending)];
        let history = vec![
            LlmMessage::user("go"),
            LlmMessage { tool_calls: vec![wants("a", "todo", "{}")], ..LlmMessage::assistant("") },
            LlmMessage::tool_result("a", for_model(&ToolResult::Todo { tasks: todos.clone() })),
            LlmMessage { tool_calls: vec![wants("b", "todo", "{}")], ..LlmMessage::assistant("") },
            LlmMessage::tool_result("b", for_model(&ToolResult::Todo { tasks: other })),
            LlmMessage::user("go on"),
        ];
        let h = harness("chat-checklist-latest", vec![text("ok")]);
        h.run(|turn| stream(turn, history.clone(), todos)).expect("turn");
        assert_eq!(conversation_of(&h.provider.requests()[0]).len(), history.len() + 1, "put back");
    }

    /// A summary made to fit the window folds the last `todo` result away;
    /// the retry carries the list, and the prompt's promise holds.
    #[test]
    fn a_list_folded_into_a_summary_comes_back_on_the_retry() {
        use crate::domain::tools::TodoStatus::{InProgress, Pending};
        let todos = vec![task("t1", "look", InProgress), task("t2", "fix", Pending)];
        let mut history = vec![
            LlmMessage::user("go"),
            LlmMessage { tool_calls: vec![wants("t", "todo", "{}")], ..LlmMessage::assistant("") },
            LlmMessage::tool_result("t", for_model(&ToolResult::Todo { tasks: todos.clone() })),
        ];
        history.extend(long_conversation());
        let h = harness("chat-checklist-summary", vec![Step::Fail(too_long_error()), text("done")]);

        h.run(|turn| stream(turn, history, todos)).expect("turn");

        let requests = h.provider.requests();
        assert_eq!(conversation_of(&requests[0]).iter().filter(|m| mentions_checklist(m)).count(), 1, "shown, nothing added");
        assert!(conversation_of(&requests[1]).iter().any(mentions_checklist), "back after the summary");
    }

    /// The latest list is current, a hook's word after it: nothing to add.
    #[test]
    fn a_current_list_with_a_hooks_note_after_it_is_enough() {
        use crate::domain::tools::TodoStatus::{InProgress, Pending};
        let todos = vec![task("t1", "look", InProgress), task("t2", "fix", Pending)];
        let history = vec![
            LlmMessage::user("go"),
            LlmMessage { tool_calls: vec![wants("now", "todo", "{}")], ..LlmMessage::assistant("") },
            LlmMessage::tool_result("now", format!("{}\n\n[A PostToolUse hook said:]\nfine", for_model(&ToolResult::Todo { tasks: todos.clone() }))),
            LlmMessage::user("go on"),
        ];
        let h = harness("chat-checklist-current", vec![text("ok")]);
        h.run(|turn| stream(turn, history.clone(), todos)).expect("turn");
        assert_eq!(conversation_of(&h.provider.requests()[0]).len(), history.len(), "nothing added");
    }

    fn last_message(request: &ChatRequest) -> &LlmMessage {
        request.messages.iter().rev().find(|m| m.role != LlmRole::System).unwrap()
    }

    /// A blank reply after a round of reads is sent back once, not taken for
    /// the end of the task.
    #[test]
    fn an_empty_reply_is_sent_back_once_instead_of_ending_the_turn() {
        let h = harness("chat-empty", vec![asks(vec![wants("r", "listFiles", "{}")]), text(""), text("done")]);

        let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");

        let ChatStreamOutcome::Done(done) = outcome else { panic!("{outcome:?}") };
        assert_eq!(done.result.text, "done");
        let requests = h.provider.requests();
        assert_eq!(requests.len(), 3);
        assert_eq!(last_message(&requests[2]).content.as_deref(), Some(EMPTY_REPLY_NOTE));
        assert_eq!(last_message(&requests[2]).role, LlmRole::User);
    }

    /// Once: a second blank reply in the same turn ends it.
    #[test]
    fn a_second_empty_reply_ends_the_turn() {
        let h = harness("chat-empty-twice", vec![text(""), text(""), text("never asked")]);

        let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");

        assert!(matches!(outcome, ChatStreamOutcome::Done(ref done) if done.result.text.is_empty()), "{outcome:?}");
        assert_eq!(h.provider.requests().len(), 2);
    }

    /// Blank because the length limit ran out first: the note says so, since
    /// "carry on" alone invites the same overlong thinking again — and the
    /// round reports that it was cut.
    #[test]
    fn an_empty_reply_cut_off_by_the_limit_says_so() {
        let cut = Step::Reply(ChatStreamResult { truncated: true, ..Default::default() });
        let h = harness("chat-empty-cut", vec![cut, text("done")]);

        h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");

        assert_eq!(last_message(&h.provider.requests()[1]).content.as_deref(), Some(EMPTY_TRUNCATED_NOTE));
        let cut_flags: Vec<bool> = h
            .events()
            .iter()
            .filter_map(|e| match &e.event {
                ChatEventPayload::RoundCompleted { truncated, .. } => Some(*truncated),
                _ => None,
            })
            .collect();
        assert_eq!(cut_flags, [true, false]);
    }

    /// Blank while the user typed: what they typed carries the turn on, with
    /// no nudge and no empty assistant message for a provider to refuse.
    #[test]
    fn an_empty_reply_while_the_user_types_goes_on_with_what_they_typed() {
        let h = harness("chat-empty-typed", vec![text_while_typing("", "also check b"), text("done")]);

        h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");

        let second = &h.provider.requests()[1];
        let conversation = conversation_of(second);
        assert!(conversation.iter().all(|m| m.role != LlmRole::Assistant), "no empty answer kept: {conversation:?}");
        assert!(last_message(second).content.as_deref().unwrap_or("").contains("also check b"));
        assert!(conversation.iter().all(|m| m.content.as_deref() != Some(EMPTY_REPLY_NOTE)));
    }

    /// Whitespace is not an answer either; any real text is.
    #[test]
    fn only_a_reply_with_something_in_it_ends_the_turn() {
        let h = harness("chat-empty-space", vec![text(" \n "), text("ok")]);
        h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");
        assert_eq!(h.provider.requests().len(), 2);

        let h = harness("chat-not-empty", vec![text("ok"), text("never asked")]);
        h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");
        assert_eq!(h.provider.requests().len(), 1);
    }

    /// A long turn: four big reads, then the first one is cleared. From then
    /// on the turn behaves as if the model had never seen it — the stub is
    /// what the model reads, a wholesale write needs a fresh read, and that
    /// read comes back in full rather than as "already above".
    #[test]
    fn a_cleared_read_is_gone_from_the_history_and_from_the_turn() {
        let read = |id: &str, path: &str| wants(id, "readFile", &format!(r#"{{"path":"{path}"}}"#));
        let h = harness(
            "chat-clearing",
            vec![
                asks(vec![read("r1", "a.txt")]),
                asks(vec![read("r2", "b.txt")]),
                asks(vec![read("r3", "c.txt")]),
                asks(vec![read("r4", "d.txt")]),
                asks(vec![wants("w", "writeFile", r#"{"path":"a.txt","content":"new"}"#)]),
                asks(vec![read("r5", "a.txt")]),
                text("done"),
            ],
        );
        for name in ["a.txt", "b.txt", "c.txt", "d.txt"] {
            std::fs::write(h.root.join(name), name.repeat(90_000 / name.len())).unwrap();
        }

        h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");
        let requests = h.provider.requests();

        // Round 5 is the first to start over the trigger: a.txt's result is
        // the one old enough to go.
        let fifth = tool_contents(&requests[4]);
        assert!(fifth[0].starts_with(result_clearing::STUB_PREFIX), "{}", &fifth[0][..80]);
        assert!(fifth[1..].iter().all(|c| c.starts_with("All 1 lines")), "the last three rounds stay");
        assert!(tool_contents(&requests[3])[0].starts_with("All 1 lines"), "not before the trigger");

        let sixth = tool_contents(&requests[5]);
        assert!(sixth.last().unwrap().contains("only read part of a.txt"), "{}", sixth.last().unwrap());
        assert_eq!(std::fs::read_to_string(h.root.join("a.txt")).unwrap().len(), 90_000, "not written");

        let seventh = tool_contents(&requests[6]);
        assert!(seventh.last().unwrap().starts_with("All 1 lines"), "read again in full");
    }

    /// Only results the model can re-derive from the transcript are worth
    /// suppressing. A write confirmation is one line and has to be seen every
    /// time, and an error is worth repeating as often as it happens.
    #[test]
    fn nothing_else_is_ever_suppressed() {
        let mut seen = HashMap::new();
        let written = ToolResult::DirectoryCreated {
            path: "src".to_string(),
        };
        let call = call("createDirectory", r#"{"path":"src"}"#);

        for _ in 0..3 {
            let content =
                dedupe_repeat_result(&mut seen, &call, Some(&written), "created".to_string());
            assert_eq!(content, "created");
        }
        for _ in 0..3 {
            let content = dedupe_repeat_result(&mut seen, &call, None, "failed".to_string());
            assert_eq!(content, "failed");
        }
    }

    #[test]
    fn a_failed_call_from_a_cut_off_round_is_told_why() {
        let note = truncated_round_note(true, true, "invalid arguments".to_string());
        assert!(note.starts_with("invalid arguments"), "{note}");
        assert!(note.contains("max_tokens"), "{note}");
    }

    /// A call that closed before the cut executed correctly; calling its
    /// result damaged would be worse than saying nothing.
    #[test]
    fn a_call_that_succeeded_is_not_told_the_round_was_cut() {
        assert_eq!(truncated_round_note(true, false, "ok".to_string()), "ok");
    }

    #[test]
    fn an_ordinary_failure_carries_no_note() {
        assert_eq!(
            truncated_round_note(false, true, "no such file".to_string()),
            "no such file"
        );
    }

    // ----------------------------------------------------- the system prompt

    /// Everything the two sides actually said, with what the app told the
    /// model stripped off the front.
    fn conversation_of(request: &ChatRequest) -> &[LlmMessage] {
        &request.messages[lead_of(request)..]
    }

    fn lead_of(request: &ChatRequest) -> usize {
        let lead = request
            .messages
            .iter()
            .take_while(|m| m.role == LlmRole::System)
            .count();
        assert_eq!(lead, 3, "the instructions, the mode, then this turn's facts");
        lead
    }

    /// The last of the leading system messages: the half that changes.
    fn facts_of(request: &ChatRequest) -> String {
        request.messages[lead_of(request) - 1]
            .content
            .clone()
            .expect("the facts are a message with content")
    }

    /// The model is told who it is and where it stands before it is asked
    /// anything. Without this the agent goes into a repository with no
    /// identity, no rules about tools, and no idea which folder is open.
    #[test]
    fn the_request_opens_with_the_prompt_and_then_the_conversation() {
        let h = harness("prompt-front", vec![text("done")]);

        h.run(|turn| stream(turn, vec![LlmMessage::user("hi")], vec![]))
            .expect("finishes");

        let requests = h.provider.requests();
        assert_eq!(
            requests[0].messages[0].content.as_deref(),
            Some(prompt::INSTRUCTIONS)
        );
        assert!(
            facts_of(&requests[0]).contains(&h.root.display().to_string()),
            "the open folder is not in the prompt"
        );
        assert_eq!(conversation_of(&requests[0]).len(), 1);
    }

    /// A round that thought goes back as the provider sent it: the next
    /// request, which answers its calls, is refused if the signed blocks are
    /// missing or rebuilt.
    #[test]
    fn a_rounds_native_content_is_carried_into_the_next_request() {
        let native = serde_json::json!([{"type": "thinking", "thinking": "hm", "signature": "s"}]);
        let h = harness(
            "native-content",
            vec![
                Step::Reply(ChatStreamResult {
                    tool_calls: vec![wants("t1", "listFiles", "{}")],
                    native_content: Some(native.clone()),
                    ..Default::default()
                }),
                text("done"),
            ],
        );

        h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]))
            .expect("finishes");

        let second = &h.provider.requests()[1];
        let asked = conversation_of(second)
            .iter()
            .find(|m| !m.tool_calls.is_empty())
            .expect("the round that called");
        assert_eq!(asked.native_content, Some(native));
    }

    /// The prompt is prepended at request time and belongs to no turn: a
    /// folder and a checklist frozen into the stored conversation would be
    /// saved to the chat file and resent, stale, for as long as it exists.
    #[test]
    fn the_prompt_never_enters_the_history_the_caller_keeps() {
        let mut h = harness(
            "prompt-not-history",
            vec![
                asks(vec![wants("w1", "writeFile", r#"{"path":"a.rs","content":"x"}"#)]),
                text("done"),
            ],
        );
        h.approval = asking();

        let ChatStreamOutcome::PendingApproval(pending) = h
            .run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]))
            .expect("pauses")
        else {
            panic!("expected a pause");
        };

        assert!(
            pending.history.iter().all(|m| m.role != LlmRole::System),
            "the prompt was stored with the conversation"
        );
    }

    /// The round after a `todo` call sees the list as it now is — in that
    /// call's result, the last thing in the request — and not in the prompt,
    /// where a list that changes would sit ahead of the history a cache reuses.
    #[test]
    fn the_checklist_follows_the_turn_in_the_todo_result() {
        let h = harness(
            "prompt-todo",
            vec![
                asks(vec![wants(
                    "t1",
                    "todo",
                    r#"{"op":"write","tasks":["read it","rewrite it"]}"#,
                )]),
                text("done"),
            ],
        );

        h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]))
            .expect("finishes");

        let requests = h.provider.requests();
        let last = |request: &ChatRequest| request.messages.last().cloned().expect("a message");
        assert!(!facts_of(&requests[0]).contains("Checklist") && !facts_of(&requests[1]).contains("Checklist"));
        assert_eq!(last(&requests[0]), LlmMessage::user("go"), "a list nobody had written yet");
        let checklist = last(&requests[1]);
        assert_eq!(checklist.role, LlmRole::Tool, "nothing after the conversation");
        let text = checklist.content.expect("text");
        assert!(text.contains("read it") && text.contains("rewrite it"));
    }

    /// A turn nobody is watching must not be told to expect an approval
    /// prompt, and a watched one must not be told the opposite.
    #[test]
    fn whether_anybody_is_watching_reaches_the_prompt() {
        let unattended = harness("prompt-unattended", vec![text("done")]);
        unattended
            .run(|turn| stream(turn, vec![LlmMessage::user("hi")], vec![]))
            .expect("finishes");

        let mut attended = harness("prompt-attended", vec![text("done")]);
        attended.approval = asking();
        attended
            .run(|turn| stream(turn, vec![LlmMessage::user("hi")], vec![]))
            .expect("finishes");

        let watched =
            |h: &Harness| facts_of(&h.provider.requests()[0]).contains("approved this turn in advance");
        assert!(watched(&unattended));
        assert!(!watched(&attended));
    }

    /// What the shell really is, as the command layer found it, is what the
    /// model reads — not the bare path.
    #[test]
    fn the_shell_reaches_the_prompt_as_described() {
        let mut h = harness("prompt-shell", vec![text("done")]);
        h.shell_described = "/bin/sh — really bash 3.2.57 in sh mode".to_string();
        h.run(|turn| stream(turn, vec![LlmMessage::user("hi")], vec![])).expect("finishes");
        assert!(facts_of(&h.provider.requests()[0]).contains("Shell for commands: /bin/sh — really bash 3.2.57 in sh mode"));
    }

    /// Half the gate: a tool the mode does not offer is not in the request.
    #[test]
    fn a_narrower_mode_advertises_fewer_tools() {
        let mut planning = harness("mode-advertised", vec![text("here is the plan")]);
        planning.mode = ConversationMode::Plan;

        planning
            .run(|turn| stream(turn, vec![LlmMessage::user("how would you do it?")], vec![]))
            .expect("finishes");

        let advertised: Vec<String> = planning.provider.requests()[0]
            .tools
            .iter()
            .map(|t| t.name.clone())
            .collect();
        assert!(advertised.contains(&"readFile".to_string()));
        assert!(!advertised.contains(&"writeFile".to_string()));
        assert!(!advertised.contains(&"runCommand".to_string()));
    }

    /// The other half, and the one that matters: a model that used `writeFile`
    /// earlier in the conversation calls it again from memory. Not advertising
    /// it does not stop that — refusing it does, and the refusal has to reach
    /// the model as a result it can act on rather than ending the turn.
    #[test]
    fn a_tool_the_mode_does_not_offer_is_refused_even_when_asked_for() {
        let mut planning = harness(
            "mode-refused",
            vec![
                asks(vec![wants(
                    "w1",
                    "writeFile",
                    r#"{"path":"a.rs","content":"new"}"#,
                )]),
                text("right — here is what I would change"),
            ],
        );
        planning.mode = ConversationMode::Plan;
        std::fs::write(planning.root.join("a.rs"), "old").unwrap();

        let outcome = planning
            .run(|turn| stream(turn, vec![LlmMessage::user("fix it")], vec![]))
            .expect("finishes rather than failing");

        let ChatStreamOutcome::Done(done) = outcome else {
            panic!("a refused tool must not pause or stop the turn");
        };
        assert_eq!(done.result.text, "right — here is what I would change");
        assert_eq!(
            std::fs::read_to_string(planning.root.join("a.rs")).unwrap(),
            "old",
            "the file was written in a mode that cannot write"
        );

        let told = told_errors(&planning);
        assert!(
            told.iter().any(|r| r.contains("not available in this conversation mode")),
            "the model was not told why: {told:?}"
        );
    }

    /// Every failure the model was handed back this turn.
    fn told_errors(h: &Harness) -> Vec<String> {
        h.events()
            .into_iter()
            .filter_map(|event| match event.event {
                ChatEventPayload::ToolResult(result) => result.error,
                _ => None,
            })
            .collect()
    }

    // ---------------------------------------------------------- loop guard

    fn loop_notes(request: &ChatRequest) -> Vec<String> {
        request
            .messages
            .iter()
            .filter(|m| m.role == LlmRole::User)
            .filter_map(|m| m.content.clone())
            .filter(|c| c.starts_with("[Loop guard]"))
            .collect()
    }

    fn loop_events(h: &Harness) -> Vec<String> {
        payloads(&h.events()).into_iter().filter(|p| p.starts_with("loop:")).collect()
    }

    /// The same listing three rounds running: the fourth request carries the
    /// note, once, and the reader is told too.
    #[test]
    fn the_same_call_three_rounds_running_is_pointed_out_once() {
        let steps = (1..=5).map(|n| asks(vec![wants(&format!("l{n}"), "listFiles", "{}")])).chain([text("done")]).collect();
        let h = harness("loop-guard-same", steps);

        let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("look")], vec![])).expect("turn");

        assert!(matches!(outcome, ChatStreamOutcome::Done(_)));
        let requests = h.provider.requests();
        assert!(loop_notes(&requests[2]).is_empty(), "two rounds are not a loop");
        assert_eq!(last_message(&requests[3]).content.as_deref(), Some(Loop::SameCall { tool: "listFiles".into() }.note().as_str()));
        assert_eq!(loop_notes(requests.last().unwrap()).len(), 1, "once a turn");
        assert_eq!(loop_events(&h), vec!["loop:listFiles:false"]);
    }

    /// The same failure with other arguments each time — a regex that never
    /// compiles fails in the tool itself, a file never read is refused before
    /// the call runs; both are the route, not the details.
    #[test]
    fn the_same_failure_three_rounds_running_is_pointed_out() {
        for (label, tool, args) in [
            ("loop-guard-regex", "grep", [r#"{"pattern":"("}"#, r#"{"pattern":"(a"}"#, r#"{"pattern":"(b"}"#]),
            ("loop-guard-unread", "writeFile", [
                r#"{"path":"a.rs","content":"x"}"#,
                r#"{"path":"a.rs","content":"y"}"#,
                r#"{"path":"a.rs","content":"z"}"#,
            ]),
        ] {
            let steps = args.iter().enumerate().map(|(n, a)| asks(vec![wants(&format!("c{n}"), tool, a)])).chain([text("done")]).collect();
            let h = harness(label, steps);
            if tool == "writeFile" {
                std::fs::write(h.root.join("a.rs"), "old").unwrap();
            }

            h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");

            assert_eq!(told_errors(&h).len(), 3, "{label}: every call failed");
            let last = h.provider.requests().last().unwrap().clone();
            assert_eq!(loop_notes(&last), vec![Loop::SameError { tool: tool.into() }.note()], "{label}");
            assert_eq!(loop_events(&h), vec![format!("loop:{tool}:true")], "{label}");
        }
    }

    /// Calls the user denied are their answer, not the model going round.
    #[test]
    fn denied_calls_are_not_counted() {
        let write = r#"{"path":"a.rs","content":"x"}"#;
        let mut h = harness(
            "loop-guard-denied",
            vec![asks(vec![wants("w1", "writeFile", write), wants("w2", "writeFile", write), wants("w3", "writeFile", write)]), text("ok")],
        );
        h.approval = asking();

        let paused = h.run(|turn| stream(turn, vec![LlmMessage::user("write")], vec![]));
        let Ok(ChatStreamOutcome::PendingApproval(pending)) = paused else { panic!("expected a pause") };
        let no = |id: &str| ToolCallDecision { id: id.to_string(), approved: false, reason: None };
        h.run(|turn| resume(turn, pending, vec![no("w1"), no("w2"), no("w3")])).expect("finishes");

        assert!(loop_notes(h.provider.requests().last().unwrap()).is_empty());
    }

    // ------------------------------------------------------------- the loop

    #[test]
    fn a_turn_with_no_tool_calls_answers_and_stops() {
        let h = harness("loop-plain", vec![text("done")]);

        let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("hi")], vec![]));

        let ChatStreamOutcome::Done(done) = outcome.expect("finishes") else {
            panic!("expected Done");
        };
        assert_eq!(done.result.text, "done");
        assert_eq!(h.provider.requests().len(), 1, "one round, one request");
    }

    fn too_long_error() -> LlmError {
        LlmError::Http(
            "http status 400: This model's maximum context length is 8192 tokens".to_string(),
        )
    }

    fn long_conversation() -> Vec<LlmMessage> {
        (0..40)
            .map(|i| {
                if i % 2 == 0 {
                    LlmMessage::user(format!("question {i}"))
                } else {
                    LlmMessage::assistant(format!("answer {i}"))
                }
            })
            .collect()
    }

    /// The failure this exists for: the provider refuses because the
    /// conversation no longer fits, and the turn ends. Now it makes room and
    /// asks again, and the user never learns there was a problem.
    #[test]
    fn a_conversation_that_no_longer_fits_is_summarized_and_asked_again() {
        let h = harness(
            "loop-too-long",
            vec![Step::Fail(too_long_error()), text("done")],
        );

        let outcome = h.run(|turn| stream(turn, long_conversation(), vec![]));

        let ChatStreamOutcome::Done(done) = outcome.expect("finishes") else {
            panic!("expected Done");
        };
        assert_eq!(done.result.text, "done");

        let requests = h.provider.requests();
        assert_eq!(requests.len(), 2, "the refused round and the retry");
        assert!(
            requests[1].messages.len() < requests[0].messages.len(),
            "asked again with the same history"
        );
        assert!(
            conversation_of(&requests[1])[0]
                .content
                .as_deref()
                .unwrap()
                .contains("they were fixing the parser"),
            "the summary opens the shorter conversation"
        );
        assert_eq!(h.provider.summaries.lock().unwrap().len(), 1);
    }

    /// History disappearing on its own is the thing to avoid: the model stops
    /// remembering what it was told, and nothing in the window says why. The
    /// summary takes seconds, so the start is said too, before it.
    #[test]
    fn the_transcript_is_told_that_history_was_folded_away() {
        let h = harness(
            "loop-too-long-event",
            vec![Step::Fail(too_long_error()), text("done")],
        );

        h.run(|turn| stream(turn, long_conversation(), vec![]))
            .expect("finishes");

        let compacted: Vec<String> = payloads(&h.events())
            .into_iter()
            .filter(|p| p.starts_with("compact"))
            .collect();
        assert_eq!(compacted, ["compacting", "compacted:34"]);
    }

    /// A second refusal after the history has already been summarized is not
    /// about its length. Summarizing again would spend another request to lose
    /// more of the conversation and fail anyway.
    #[test]
    fn a_conversation_is_summarized_once_and_then_the_refusal_stands() {
        let h = harness(
            "loop-too-long-twice",
            vec![Step::Fail(too_long_error()), Step::Fail(too_long_error())],
        );

        let outcome = h.run(|turn| stream(turn, long_conversation(), vec![]));

        assert!(matches!(outcome, Err(TurnError::Provider(_))), "{outcome:?}");
        assert_eq!(h.provider.summaries.lock().unwrap().len(), 1, "summarized twice");
    }

    /// Not every overflow is a long conversation: one enormous file read
    /// fills the window on its own, and there is nothing to summarize. The
    /// refusal is then the useful thing to report — and no request is spent
    /// discovering that.
    #[test]
    fn a_short_conversation_that_does_not_fit_is_reported_rather_than_summarized() {
        let h = harness("loop-too-long-short", vec![Step::Fail(too_long_error())]);

        let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("x".repeat(9_000))], vec![]));

        assert!(matches!(outcome, Err(TurnError::Provider(_))), "{outcome:?}");
        assert!(h.provider.summaries.lock().unwrap().is_empty());
    }

    /// Any other refusal is reported as it always was — answering one with a
    /// summarizing request costs money and loses history for nothing.
    #[test]
    fn another_kind_of_refusal_is_not_answered_by_summarizing() {
        let h = harness(
            "loop-other-error",
            vec![Step::Fail(LlmError::Http("http status 401: invalid api key".to_string()))],
        );

        let outcome = h.run(|turn| stream(turn, long_conversation(), vec![]));

        assert!(matches!(outcome, Err(TurnError::Provider(_))), "{outcome:?}");
        assert!(h.provider.summaries.lock().unwrap().is_empty());
    }

    /// The loop's actual job: a call runs, and what it produced goes back to
    /// the model as the next request's history.
    #[test]
    fn a_tool_result_reaches_the_next_round() {
        std::fs::write("/dev/null", "").ok();
        let h = harness(
            "loop-tool",
            vec![
                asks(vec![wants("c1", "createDirectory", r#"{"path":"src"}"#)]),
                text("made it"),
            ],
        );

        let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("make src")], vec![]));

        assert!(matches!(outcome.expect("finishes"), ChatStreamOutcome::Done(_)));
        assert!(h.root.join("src").is_dir(), "the tool actually ran");

        let second = &h.provider.requests()[1];
        let results = tool_contents(second);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], "Created directory src");
        // The assistant's own tool-call turn has to precede its results, or
        // the provider sees answers to a question it was never shown.
        let assistant = second
            .messages
            .iter()
            .find(|m| m.role == LlmRole::Assistant)
            .expect("the tool-call turn is in the history");
        assert_eq!(assistant.tool_calls.len(), 1);
    }

    /// The order is the contract: a listener pairs a call with its result by
    /// id, and orders everything by `seq`.
    #[test]
    fn events_are_numbered_in_order_and_pair_by_id() {
        let h = harness(
            "loop-events",
            vec![
                asks(vec![wants("c1", "createDirectory", r#"{"path":"a"}"#)]),
                text("ok"),
            ],
        );

        h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");

        let events = h.events();
        assert_eq!(
            payloads(&events),
            [
                "roundStarted",
                "roundCompleted",
                "toolCall:c1",
                "toolResult:c1",
                "roundStarted",
                "delta",
                "roundCompleted",
            ]
        );
        let seqs: Vec<u64> = events.iter().map(|e| e.seq).collect();
        assert_eq!(seqs, (1..=events.len() as u64).collect::<Vec<_>>());
        assert_eq!(events[0].round, 1);
        assert_eq!(events.last().unwrap().round, 2);
    }

    // ------------------------------------------------------------- approval

    fn asking() -> ApprovalPolicy {
        ApprovalPolicy::default()
    }

    /// "Always allow runCommand" lets the build through and still stops a
    /// force push — which the card then explains. A command that only reads
    /// needs nothing at all.
    #[test]
    fn a_command_past_undoing_asks_with_its_reason_and_a_read_does_not_ask() {
        let mut h = harness(
            "command-reason",
            vec![asks(vec![
                wants("r1", "runCommand", r#"{"command":"git status"}"#),
                wants("r2", "runCommand", r#"{"command":"git pf"}"#),
            ])],
        );
        h.approval = ApprovalPolicy::default();
        h.approval.allow_always("runCommand").unwrap();
        h.approval.git_aliases.insert("pf".into(), "push --force".into());

        let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]));
        let ChatStreamOutcome::PendingApproval(pending) = outcome.expect("pauses") else {
            panic!("expected a pause");
        };
        let asks: Vec<(bool, Option<&str>)> =
            pending.calls.iter().map(|c| (c.requires_confirmation, c.reason.as_deref())).collect();
        assert_eq!(asks, [(false, None), (true, Some("rewrites a remote (git push --force)"))]);
    }

    /// Nothing in the round runs — not even the calls that needed no decision.
    /// A half-executed round is a state nobody could describe to whoever
    /// resumes it.
    #[test]
    fn a_risky_call_pauses_the_whole_round_with_nothing_run() {
        let mut h = harness(
            "loop-pause",
            vec![asks(vec![
                wants("l1", "listFiles", "{}"),
                wants("w1", "writeFile", r#"{"path":"a.rs","content":"x"}"#),
            ])],
        );
        h.approval = asking();

        let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]));

        let ChatStreamOutcome::PendingApproval(pending) = outcome.expect("pauses") else {
            panic!("expected a pause");
        };
        assert_eq!(
            pending.calls.iter().map(|c| c.requires_confirmation).collect::<Vec<_>>(),
            [false, true],
            "both calls are carried, only the write needs an answer"
        );
        assert!(!h.root.join("a.rs").exists());
        assert!(
            !payloads(&h.events()).iter().any(|p| p.starts_with("toolCall:")),
            "the harmless call did not run either — nothing in the round did"
        );
        assert_eq!(pending.round, 1);
        assert!(pending.budget_used > 0, "a paused round is still charged");
        assert!(
            payloads(&h.events()).contains(&"roundCompleted".to_string()),
            "a round that pauses has still reported what it said"
        );
    }

    /// The registry has to cross the pause. Without it the write the user just
    /// approved is refused for never having read the file — the pause itself
    /// would be what broke it.
    #[test]
    fn a_read_from_before_the_pause_still_counts_after_it() {
        std::fs::write(temp_dir("loop-seed").join("ignored"), "").ok();
        let mut h = harness(
            "loop-resume-reads",
            vec![
                asks(vec![wants("r1", "readFile", r#"{"path":"a.rs"}"#)]),
                asks(vec![wants(
                    "w1",
                    "writeFile",
                    r#"{"path":"a.rs","content":"new"}"#,
                )]),
                text("written"),
            ],
        );
        h.approval = asking();
        std::fs::write(h.root.join("a.rs"), "old").unwrap();

        let paused = h.run(|turn| stream(turn, vec![LlmMessage::user("rewrite a.rs")], vec![]));
        let ChatStreamOutcome::PendingApproval(pending) = paused.expect("pauses") else {
            panic!("expected a pause");
        };

        let resumed = h.run(|turn| {
            resume(
                turn,
                pending,
                vec![ToolCallDecision {
                    id: "w1".to_string(),
                    approved: true,
                    reason: None,
                }],
            )
        });

        assert!(matches!(resumed.expect("finishes"), ChatStreamOutcome::Done(_)));
        assert_eq!(std::fs::read_to_string(h.root.join("a.rs")).unwrap(), "new");
    }

    /// A refusal is not a failure of the turn, and the reason is what stops
    /// the model from trying the same call again.
    #[test]
    fn a_denied_call_hands_the_model_the_reason_and_the_turn_continues() {
        let mut h = harness(
            "loop-denied",
            vec![
                asks(vec![wants("w1", "writeFile", r#"{"path":"a.rs","content":"x"}"#)]),
                text("understood"),
            ],
        );
        h.approval = asking();

        let paused = h.run(|turn| stream(turn, vec![LlmMessage::user("write it")], vec![]));
        let ChatStreamOutcome::PendingApproval(pending) = paused.expect("pauses") else {
            panic!("expected a pause");
        };
        let resumed = h.run(|turn| {
            resume(
                turn,
                pending,
                vec![ToolCallDecision {
                    id: "w1".to_string(),
                    approved: false,
                    reason: Some("use the existing helper".to_string()),
                }],
            )
        });

        assert!(matches!(resumed.expect("finishes"), ChatStreamOutcome::Done(_)));
        assert!(!h.root.join("a.rs").exists(), "a denied call must not run");
        let told = tool_contents(h.provider.requests().last().unwrap());
        assert!(told[0].contains("use the existing helper"), "{}", told[0]);
    }

    /// Every settled call reaches the log, each with how it ended — and none
    /// of the text the calls carried, whichever way they ended.
    #[test]
    fn every_call_is_logged_without_its_content() {
        const LEAK: &str = "LEAK-marker";
        let h = harness(
            "loop-log",
            vec![
                asks(vec![
                    wants("w1", "writeFile", &format!(r#"{{"path":"a.rs","content":"{LEAK}"}}"#)),
                    wants("r1", "readFile", r#"{"path":"a.rs"}"#),
                    wants("e1", "editFile", &format!(r#"{{"path":"a.rs","edits":[{{"old":"absent {LEAK}","new":"x"}}]}}"#)),
                    wants("b1", "writeFile", &format!(r#"{{"path":"b.rs","content":["{LEAK}"]}}"#)),
                ]),
                asks(vec![wants("w2", "writeFile", &format!(r#"{{"path":"c.rs","content":"{LEAK}"}}"#))]),
                text("done"),
            ],
        );
        h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");

        let logged = h.logged.lock().unwrap().clone();
        let summary: Vec<(&str, CallStatus)> = logged.iter().map(|e| (e.tool.as_str(), e.status)).collect();
        // In the order they settled: the broken one is refused before the
        // rest of its round runs.
        assert_eq!(
            summary,
            [
                ("writeFile", CallStatus::Error),
                ("writeFile", CallStatus::Ok),
                ("readFile", CallStatus::Ok),
                ("editFile", CallStatus::Error),
                ("writeFile", CallStatus::Ok),
            ]
        );
        for entry in &logged {
            let text = serde_json::to_string(entry).unwrap();
            assert!(!text.contains(LEAK), "{text}");
            assert_eq!((entry.provider_id.as_str(), entry.model.as_str()), ("test", "m"));
        }
        assert_eq!(logged[1].args["args"]["path"], "a.rs");
        assert_eq!(logged[0].args, serde_json::Value::Null, "unparsed arguments are not kept");
        assert!(logged[0].error.as_deref().is_some_and(|e| e.starts_with("invalid arguments for writeFile")));
        assert_eq!((logged[1].round, logged[4].round), (1, 2));
        assert_eq!(logged[3].error.as_deref(), Some("edit text not found"));
    }

    #[test]
    fn a_denial_is_logged_as_one_with_the_users_reason() {
        let mut h = harness(
            "loop-log-denied",
            vec![asks(vec![wants("w1", "writeFile", r#"{"path":"a.rs","content":"x"}"#)]), text("ok")],
        );
        h.approval = asking();
        let ChatStreamOutcome::PendingApproval(pending) =
            h.run(|turn| stream(turn, vec![LlmMessage::user("write it")], vec![])).expect("pauses")
        else {
            panic!("expected a pause");
        };
        assert!(h.logged.lock().unwrap().is_empty(), "a paused call has not settled");

        h.run(|turn| {
            resume(turn, pending, vec![ToolCallDecision { id: "w1".into(), approved: false, reason: Some("not now".into()) }])
        })
        .expect("finishes");

        let logged = h.logged.lock().unwrap().clone();
        assert_eq!(logged.len(), 1);
        assert_eq!(logged[0].status, CallStatus::Denied);
        assert_eq!(logged[0].error.as_deref(), Some("Denied by the user: not now"));
        assert!(logged[0].result.is_none());
    }

    /// Pausing must not be a way to buy more budget: the resumed pass charges
    /// the round again, exactly as `round` itself is counted twice.
    #[test]
    fn a_paused_round_is_charged_on_both_passes() {
        let write = |id: &str| wants(id, "writeFile", r#"{"path":"a.rs","content":"x"}"#);
        let mut h = harness(
            "loop-budget",
            vec![asks(vec![write("w1")]), asks(vec![write("w2")])],
        );
        h.approval = asking();
        let weight = ToolName::WriteFile.loop_weight();

        let ChatStreamOutcome::PendingApproval(first) = h
            .run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]))
            .expect("pauses")
        else {
            panic!("expected a pause");
        };
        assert_eq!(first.budget_used, weight);

        let approve = |id: &str| ToolCallDecision {
            id: id.to_string(),
            approved: true,
            reason: None,
        };
        let ChatStreamOutcome::PendingApproval(second) = h
            .run(|turn| resume(turn, first, vec![approve("w1")]))
            .expect("pauses again")
        else {
            panic!("expected a second pause");
        };

        assert_eq!(
            second.budget_used,
            weight * 3,
            "the resumed round is charged again, and then the new round"
        );
        assert_eq!(second.round, 3, "and the round counter moves the same way");
    }

    // ---------------------------------------------------------------- resume

    #[test]
    fn a_resume_missing_a_decision_is_refused() {
        let h = harness("loop-resume-missing", vec![]);
        let pending = PendingApproval {
            history: vec![LlmMessage {
                role: LlmRole::Assistant,
                content: None,
                tool_call_id: None,
                tool_calls: vec![wants("w1", "writeFile", "{}")],
                native_content: None,
            }],
            round: 1,
            budget_used: 2,
            event_seq: 4,
            calls: vec![PendingToolCall {
                id: "w1".to_string(),
                name: "writeFile".to_string(),
                arguments: "{}".to_string(),
                requires_confirmation: true,
                reason: None,
            }],
            todos: vec![],
            reads: ReadFiles::default(),
        };

        let err = h.run(|turn| resume(turn, pending, vec![])).expect_err("refused");
        assert!(matches!(err, TurnError::Decision(_)), "{err}");
    }

    /// Tool results with no request in front of them are rejected by the
    /// provider, far from the point where the mismatch could be explained.
    #[test]
    fn a_resume_whose_history_lost_the_tool_call_round_is_refused() {
        let h = harness("loop-resume-history", vec![]);
        let pending = PendingApproval {
            history: vec![LlmMessage::user("go")],
            round: 1,
            budget_used: 0,
            event_seq: 0,
            calls: vec![],
            todos: vec![],
            reads: ReadFiles::default(),
        };

        let err = h.run(|turn| resume(turn, pending, vec![])).expect_err("refused");
        assert!(matches!(err, TurnError::BadResume(_)), "{err}");

        // Only results may follow the round: anything else means it is over.
        let round = LlmMessage {
            role: LlmRole::Assistant,
            content: None,
            tool_call_id: None,
            tool_calls: vec![wants("w1", "writeFile", "{}")],
            native_content: None,
        };
        let pending = PendingApproval {
            history: vec![round, LlmMessage::user("go")],
            round: 1,
            budget_used: 0,
            event_seq: 0,
            calls: vec![],
            todos: vec![],
            reads: ReadFiles::default(),
        };
        let err = h.run(|turn| resume(turn, pending, vec![])).expect_err("refused");
        assert!(matches!(err, TurnError::BadResume(_)), "{err}");
    }

    /// One turn, one stream of numbers: a listener that reconnects after the
    /// pause cannot order anything if resuming starts again from zero.
    #[test]
    fn the_event_stream_continues_across_a_pause() {
        let mut h = harness(
            "loop-seq",
            vec![
                asks(vec![wants("w1", "writeFile", r#"{"path":"a.rs","content":"x"}"#)]),
                text("ok"),
            ],
        );
        h.approval = asking();

        let paused = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]));
        let ChatStreamOutcome::PendingApproval(pending) = paused.expect("pauses") else {
            panic!("expected a pause");
        };
        let before = h.events().last().expect("events").seq;
        assert_eq!(pending.event_seq, before);

        let decisions = vec![ToolCallDecision {
            id: "w1".to_string(),
            approved: true,
            reason: None,
        }];
        h.run(|turn| resume(turn, pending, decisions)).expect("finishes");

        let seqs: Vec<u64> = h.events().iter().map(|e| e.seq).collect();
        assert_eq!(seqs, (1..=seqs.len() as u64).collect::<Vec<_>>());
    }

    // ----------------------------------------------------------- cancelling

    #[test]
    fn a_stop_before_the_first_round_runs_nothing() {
        let h = harness("loop-cancel-early", vec![]);
        h.cancel_at_poll(1);

        let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]));

        assert!(matches!(outcome.expect("stops"), ChatStreamOutcome::Cancelled(_)));
        assert!(h.provider.requests().is_empty(), "the model was never asked");
    }

    /// The point of the second checkpoint: a stop that lands as the round
    /// finishes pre-empts the write that round asked for, not merely the
    /// model's next sentence.
    #[test]
    fn a_stop_as_the_round_finishes_pre_empts_its_tool_calls() {
        let h = harness(
            "loop-cancel-mid",
            vec![Step::Reply(ChatStreamResult {
                text: "making it".to_string(),
                tool_calls: vec![wants("w1", "createDirectory", r#"{"path":"never"}"#)],
                ..Default::default()
            })],
        );
        // Poll 1 is the top of the round; poll 2 is the provider's own
        // cancellation callback; poll 3 is the checkpoint after it returns.
        h.cancel_at_poll(3);

        let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]));

        let ChatStreamOutcome::Cancelled(done) = outcome.expect("stops") else {
            panic!("not cancelled")
        };
        assert!(!h.root.join("never").exists(), "the call was pre-empted");
        // A request for tools with no results after it is refused by the
        // provider on the next message: the unrun call gets one saying so.
        let shape: Vec<(LlmRole, Option<&str>)> =
            done.history.iter().map(|m| (m.role, m.tool_call_id.as_deref())).collect();
        assert_eq!(shape, [(LlmRole::User, None), (LlmRole::Assistant, None), (LlmRole::Tool, Some("w1"))]);
        assert!(done.history[2].content.as_deref().is_some_and(|c| c.starts_with("Not run")));
        assert_eq!(done.history[1].content.as_deref(), Some("making it"), "what the round said is kept, once");
        assert!(
            !payloads(&h.events()).iter().any(|p| p.starts_with("toolCall:")),
            "and was never even announced"
        );
    }

    // ------------------------------------------------------------- retrying

    fn rate_limited(seconds: u64) -> LlmError {
        LlmError::RateLimited {
            retry_after_seconds: Some(seconds),
            message: "slow down".to_string(),
        }
    }

    #[test]
    fn a_rate_limited_round_waits_and_runs_again() {
        let h = harness(
            "loop-retry",
            vec![Step::Fail(rate_limited(3)), text("second time lucky")],
        );

        let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]));

        let ChatStreamOutcome::Done(done) = outcome.expect("finishes") else {
            panic!("expected Done");
        };
        assert_eq!(done.result.text, "second time lucky");
        assert_eq!(h.provider.requests().len(), 2, "the same round, twice");
        assert_eq!(
            h.slept.lock().unwrap().iter().sum::<Duration>(),
            Duration::from_secs(3),
            "waited exactly as long as the server asked"
        );
        assert!(payloads(&h.events()).contains(&"retrying".to_string()));
    }

    /// The rule that makes retrying safe at all: half the round has already
    /// reached the transcript, and sending the request again would append the
    /// text twice.
    #[test]
    fn a_round_that_already_streamed_is_not_retried() {
        let h = harness(
            "loop-retry-unsafe",
            vec![Step::StreamThenFail("half an answer", rate_limited(1))],
        );

        let err = h
            .run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]))
            .expect_err("gives up");

        assert!(matches!(err, TurnError::Provider(LlmError::RateLimited { .. })), "{err}");
        assert_eq!(h.provider.requests().len(), 1, "asked once, never repeated");
        assert!(h.slept.lock().unwrap().is_empty());
    }

    /// A refusal the provider meant is not retried at all.
    #[test]
    fn a_considered_refusal_ends_the_turn() {
        let h = harness(
            "loop-refusal",
            vec![Step::Fail(LlmError::Http("http status 400: no such model".to_string()))],
        );

        let err = h
            .run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]))
            .expect_err("fails");

        assert!(matches!(err, TurnError::Provider(_)), "{err}");
        assert_eq!(h.provider.requests().len(), 1);
    }

    #[test]
    fn a_stop_during_a_retry_wait_takes_effect_inside_it() {
        let h = harness("loop-retry-cancel", vec![Step::Fail(rate_limited(60))]);
        // Past the round's own checkpoints, so the stop lands in the wait.
        h.cancel_at_poll(4);

        let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]));

        assert!(matches!(outcome.expect("stops"), ChatStreamOutcome::Cancelled(_)));
        assert!(
            h.slept.lock().unwrap().iter().sum::<Duration>() < Duration::from_secs(60),
            "the stop was not made to wait out the whole window"
        );
    }

    // -------------------------------------------------------------- ceilings

    /// A model that never stops asking for tools must not hold the turn open
    /// forever.
    #[test]
    fn a_turn_that_never_finishes_is_cut_off() {
        let steps = (0..MAX_TOOL_ITERATIONS + 1)
            .map(|i| {
                asks(vec![wants(
                    &format!("c{i}"),
                    "createDirectory",
                    &format!(r#"{{"path":"d{i}"}}"#),
                )])
            })
            .collect();
        let h = harness("loop-ceiling", steps);

        let err = h
            .run(|turn| stream(turn, vec![LlmMessage::user("loop forever")], vec![]))
            .expect_err("is cut off");

        assert!(matches!(err, TurnError::Exhausted { .. }), "{err}");
        assert!(h.provider.requests().len() <= MAX_TOOL_ITERATIONS);
    }

    // ------------------------------------------------------- what the model reads

    /// A call refused before it runs still has to be reported and answered,
    /// or the model is left waiting for a result that never comes.
    #[test]
    fn a_call_refused_by_the_preflight_is_reported_as_a_tool_error() {
        let h = harness(
            "loop-preflight",
            vec![
                asks(vec![wants(
                    "w1",
                    "writeFile",
                    r#"{"path":"../outside.rs","content":"x"}"#,
                )]),
                text("understood"),
            ],
        );

        h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");

        let told = tool_contents(h.provider.requests().last().unwrap());
        assert_eq!(told.len(), 1);
        assert!(told[0].starts_with("Error:"), "{}", told[0]);
        let events = payloads(&h.events());
        assert!(events.contains(&"toolCall:w1".to_string()));
        assert!(events.contains(&"toolResult:w1".to_string()));
    }

    /// The preflight answers a refused call before the round pauses for the
    /// rest, so the history then ends with that answer, not the request.
    #[test]
    fn a_round_with_a_refused_call_still_resumes_after_approval() {
        let mut h = harness(
            "loop-preflight-pause",
            vec![
                asks(vec![
                    wants("w0", "writeFile", r#"{"path":"../outside.rs","content":"x"}"#),
                    wants("w1", "writeFile", r#"{"path":"a.rs","content":"x"}"#),
                ]),
                text("done"),
            ],
        );
        h.approval = asking();
        let paused = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]));
        let ChatStreamOutcome::PendingApproval(pending) = paused.expect("pauses") else {
            panic!("expected a pause");
        };

        let approve = vec![ToolCallDecision { id: "w1".to_string(), approved: true, reason: None }];
        h.run(|turn| resume(turn, pending, approve)).expect("resumes");

        let told = tool_contents(h.provider.requests().last().unwrap());
        assert_eq!(told.len(), 2, "{told:?}");
        assert!(told[0].starts_with("Error:"), "{}", told[0]);
        assert!(h.root.join("a.rs").exists());
    }

    /// A listing goes to the model as a tree: a flat array of paths makes it
    /// rebuild the directory structure from N separate strings.
    #[test]
    fn a_listing_reaches_the_model_as_a_tree() {
        let h = harness(
            "loop-listing",
            vec![asks(vec![wants("l1", "listFiles", "{}")]), text("seen")],
        );
        std::fs::create_dir(h.root.join("src")).unwrap();
        std::fs::write(h.root.join("src/main.rs"), "fn main() {}").unwrap();

        h.run(|turn| stream(turn, vec![LlmMessage::user("what is here")], vec![]))
            .expect("finishes");

        let told = tool_contents(h.provider.requests().last().unwrap());
        assert!(told[0].contains("main.rs"), "{}", told[0]);
        assert!(!told[0].contains("\"isDir\""), "raw JSON, not a tree: {}", told[0]);
    }

    /// Token usage is reported once per round, and it is the whole context —
    /// every request resends the history.
    #[test]
    fn usage_is_reported_for_the_round_that_produced_it() {
        let h = harness(
            "loop-usage",
            vec![Step::Reply(ChatStreamResult {
                text: "done".to_string(),
                usage: Some(ChatUsage {
                    prompt_tokens: 100,
                    completion_tokens: 7,
                    total_tokens: 107,
                    cached_tokens: 90,
                }),
                ..Default::default()
            })],
        );

        h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");

        assert!(payloads(&h.events()).contains(&"contextUsage".to_string()));
    }

    /// The model is offered the tools this build actually has, every round.
    #[test]
    fn every_request_carries_the_tool_schemas() {
        let h = harness("loop-tools", vec![text("hi")]);

        h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");

        let requests = h.provider.requests();
        let offered: &[LlmToolDefinition] = &requests[0].tools;
        // Every built-in one but a review's own; MCP tools come from
        // servers, and none is connected.
        assert_eq!(offered.len(), ToolName::ALL.len() - 2);
    }

    /// A write in the loop says what it did to the file, for a rewind; a
    /// read says nothing.
    #[test]
    fn a_write_in_the_loop_reports_what_it_changed() {
        crate::testing::with_app_dir("chat-changes", || {
            let h = harness(
                "chat-changes",
                vec![
                    asks(vec![wants("r", "readFile", r#"{"path":"a.rs"}"#)]),
                    asks(vec![wants("w", "writeFile", r#"{"path":"a.rs","content":"new"}"#)]),
                    text("done"),
                ],
            );
            std::fs::write(h.root.join("a.rs"), "old").unwrap();
            h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");

            let log = h.log.lock().unwrap();
            let changes = |id: &str| {
                log.iter()
                    .find_map(|e| match &e.event {
                        ChatEventPayload::ToolResult(r) if r.id == id => Some(r.changes.clone()),
                        _ => None,
                    })
                    .unwrap()
            };
            assert!(changes("r").is_empty());
            let stored = |content: &[u8]| crate::domain::rewind::FileState::Stored { hash: crate::infra::file_history::hash(content) };
            assert_eq!(changes("w"), [FileChange { path: "a.rs".into(), before: stored(b"old"), after: stored(b"new") }]);
        });
    }

    // --------------------------------------------------- background processes

    /// Reports one ended process, once.
    struct EndedOnce(Mutex<Vec<crate::domain::background::ProcessInfo>>);
    impl BackgroundProcesses for EndedOnce {
        fn start(&self, _: &Shell, _: &str, _: &std::path::Path, _: &str) -> Result<crate::domain::background::ProcessInfo, crate::domain::background::BackgroundError> {
            unreachable!()
        }
        fn read(&self, _: u32) -> Result<crate::domain::background::ProcessOutput, crate::domain::background::BackgroundError> {
            unreachable!()
        }
        fn stop(&self, _: u32) -> Result<crate::domain::background::ProcessInfo, crate::domain::background::BackgroundError> {
            unreachable!()
        }
        fn list(&self) -> Vec<crate::domain::background::ProcessInfo> {
            vec![]
        }
        fn take_ended(&self) -> Vec<crate::domain::background::ProcessInfo> {
            std::mem::take(&mut self.0.lock().unwrap())
        }
        fn adopt(
            &self,
            _: std::process::Child,
            _: &str,
            _: &str,
            _: &str,
        ) -> Result<(crate::domain::background::ProcessInfo, [crate::domain::background::Feed; 2]), crate::domain::background::BackgroundError> {
            unreachable!()
        }
    }

    /// The turn hands its processes to the tools: a background start in the
    /// loop comes back as a number, not as a command that ran.
    #[cfg(unix)]
    #[test]
    fn a_background_start_in_the_loop_reaches_the_registry() {
        let mut h = harness(
            "bg-loop",
            vec![asks(vec![wants("b1", "runCommand", r#"{"command":"sleep 30","background":true}"#)]), text("ok")],
        );
        let processes = Arc::new(crate::infra::background::Processes::default());
        h.processes = Some(processes.clone());
        h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");

        let said = tool_contents(&h.provider.requests()[1]);
        assert!(said[0].starts_with("Started background process #1 in "), "{said:?}");
        assert!(processes.list()[0].running());
    }

    /// A dev server that died between turns is news the model gets before
    /// its next round — once, and in the history, so a retry keeps it.
    #[test]
    fn a_process_that_ended_is_told_to_the_model_once() {
        let mut h = harness("bg-ended", vec![text("I see"), text("ok")]);
        h.processes = Some(Arc::new(EndedOnce(Mutex::new(vec![crate::domain::background::ProcessInfo {
            id: 2,
            command: "npm run dev".into(),
            cwd: ".".into(),
            state: crate::domain::background::ProcessState::Exited { code: Some(1) },
        }]))));
        h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");
        h.run(|turn| stream(turn, vec![LlmMessage::user("again")], vec![])).expect("finishes");

        let requests = h.provider.requests();
        let told = |request: &ChatRequest| {
            request.messages.iter().any(|m| {
                m.role == LlmRole::User && m.content.as_deref().is_some_and(|c| c.contains("#2 `npm run dev` exited with code 1"))
            })
        };
        assert!(told(&requests[0]));
        assert!(!told(&requests[1]), "said once");
        assert!(payloads(&h.events()).contains(&"ended:1".to_string()));
    }

    // ---------------------------------------------------------------- hooks

    type HookInputs = Arc<Mutex<Vec<(String, serde_json::Value)>>>;

    /// Hooks answered by `answer(command, input) -> (exit code, stderr)`;
    /// every run is kept, with the input it was given.
    fn hooked(
        mut h: Harness,
        config: serde_json::Value,
        answer: impl Fn(&str, &serde_json::Value) -> (i32, &'static str) + Send + Sync + 'static,
    ) -> (Harness, HookInputs) {
        let inputs: HookInputs = Arc::default();
        let seen = Arc::clone(&inputs);
        h.hooks = Hooks::new(
            serde_json::from_value(config).unwrap(),
            Arc::new(move |hook: &crate::domain::hooks::HookCommand, input: &str, _: &std::path::Path| {
                let input: serde_json::Value = serde_json::from_str(input).unwrap();
                let (code, stderr) = answer(&hook.command, &input);
                seen.lock().unwrap().push((hook.command.clone(), input));
                Ok(crate::domain::command_exec::CommandOutput {
                    stdout: String::new(),
                    stderr: stderr.into(),
                    exit_code: Some(code),
                    timed_out: false,
                    truncated: false,
                    duration_ms: 0,
                    full_output: None,
                })
            }),
        );
        (h, inputs)
    }

    fn hook(event: &str, matcher: &str, command: &str) -> serde_json::Value {
        serde_json::json!({"hooks": {event: [{"matcher": matcher, "hooks": [{"type": "command", "command": command}]}]}})
    }

    fn feedback(h: &Harness) -> Vec<(String, String, bool)> {
        h.events()
            .into_iter()
            .filter_map(|e| match e.event {
                ChatEventPayload::HookFeedback { event, message, blocked } => Some((event, message, blocked)),
                _ => None,
            })
            .collect()
    }

    /// The approved call does not run; the model reads the hook's reason, the
    /// log says only that a hook blocked it.
    #[test]
    fn a_pre_tool_use_hook_that_exits_two_refuses_the_call() {
        let (h, inputs) = hooked(
            harness("hook-pre-block", vec![asks(vec![wants("w1", "writeFile", r#"{"path":"a.rs","content":"x"}"#)]), text("ok")]),
            hook("PreToolUse", "writeFile|editFile", "guard"),
            |_, _| (2, "no writes on Fridays"),
        );
        h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");

        assert!(!h.root.join("a.rs").exists());
        let said = tool_contents(&h.provider.requests()[1]);
        assert_eq!(said, ["Error: a hook refused this call: no writes on Fridays"]);
        assert_eq!(feedback(&h), [("PreToolUse".to_string(), "no writes on Fridays".to_string(), true)]);
        let input = &inputs.lock().unwrap()[0].1;
        assert_eq!(input["tool_name"], "writeFile");
        assert_eq!(input["tool_input"]["path"], "a.rs");
        assert_eq!(input["tool_use_id"], "w1");
        assert_eq!(input["hook_event_name"], "PreToolUse");
        let logged = h.logged.lock().unwrap().clone();
        assert_eq!(logged[0].error.as_deref(), Some("blocked by a hook"));
    }

    #[test]
    fn a_hook_that_fails_otherwise_only_warns_and_one_for_another_tool_does_not_run() {
        let config = serde_json::json!({"hooks": {"PreToolUse": [
            {"matcher": "writeFile", "hooks": [{"type": "command", "command": "broken"}]},
            {"matcher": "runCommand", "hooks": [{"type": "command", "command": "elsewhere"}]},
        ]}});
        let (h, inputs) = hooked(
            harness("hook-pre-warn", vec![asks(vec![wants("w1", "writeFile", r#"{"path":"a.rs","content":"x"}"#)]), text("ok")]),
            config,
            |_, _| (1, "jq: not found"),
        );
        h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");

        assert_eq!(std::fs::read_to_string(h.root.join("a.rs")).unwrap(), "x", "the call ran");
        assert_eq!(inputs.lock().unwrap().iter().map(|(c, _)| c.as_str()).collect::<Vec<_>>(), ["broken"]);
        assert_eq!(feedback(&h), [("PreToolUse".to_string(), "hook `broken` failed with code 1: jq: not found".to_string(), false)]);
    }

    /// A call the user denied never reaches the hooks: nothing is about to run.
    #[test]
    fn a_denied_call_does_not_ask_the_hooks() {
        let (mut h, inputs) = hooked(
            harness("hook-denied", vec![asks(vec![wants("w1", "writeFile", r#"{"path":"a.rs","content":"x"}"#)]), text("ok")]),
            hook("PreToolUse", "", "guard"),
            |_, _| (0, ""),
        );
        h.approval = asking();
        let Ok(ChatStreamOutcome::PendingApproval(pending)) = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])) else {
            panic!("expected a pause");
        };
        let no = vec![ToolCallDecision { id: "w1".into(), approved: false, reason: None }];
        h.run(|turn| resume(turn, pending, no)).expect("finishes");
        assert!(inputs.lock().unwrap().is_empty());
    }

    /// After the call it is too late to refuse; what the hook says goes to
    /// the model beside the result.
    #[test]
    fn a_post_tool_use_hook_speaks_to_the_model_after_the_call() {
        let (h, inputs) = hooked(
            harness("hook-post", vec![asks(vec![wants("w1", "writeFile", r#"{"path":"a.rs","content":"x"}"#)]), text("ok")]),
            hook("PostToolUse", "writeFile", "lint"),
            |_, _| (2, "a.rs: missing semicolon"),
        );
        h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");

        assert!(h.root.join("a.rs").exists(), "it ran");
        let said = tool_contents(&h.provider.requests()[1]);
        assert!(said[0].ends_with("\n\n[A PostToolUse hook said:]\na.rs: missing semicolon"), "{said:?}");
        assert!(!inputs.lock().unwrap()[0].1["tool_response"].is_null());
    }

    #[test]
    fn a_failed_call_does_not_reach_post_tool_use() {
        let (h, inputs) = hooked(
            harness("hook-post-failed", vec![asks(vec![wants("r1", "readFile", r#"{"path":"missing.rs"}"#)]), text("ok")]),
            hook("PostToolUse", "", "lint"),
            |_, _| (0, ""),
        );
        h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");
        assert!(inputs.lock().unwrap().is_empty());
    }

    /// "Run the tests before you stop": the hook sends the model back once,
    /// and lets it go when told it already has.
    #[test]
    fn a_stop_hook_sends_the_model_back_until_it_lets_go() {
        let (h, inputs) = hooked(
            harness("hook-stop", vec![text("done"), text("tests pass, done")]),
            hook("Stop", "", "check"),
            |_, input| if input["stop_hook_active"] == true { (0, "") } else { (2, "run the tests first") },
        );
        let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");
        let ChatStreamOutcome::Done(done) = outcome else { panic!("expected done") };
        assert_eq!(done.result.text, "tests pass, done");

        let second = &h.provider.requests()[1];
        let tail: Vec<_> = second.messages.iter().rev().take(2).collect();
        assert_eq!(tail[1].content.as_deref(), Some("done"));
        assert_eq!(tail[0].role, LlmRole::User);
        assert!(tail[0].content.as_deref().unwrap().ends_with("It said:]\nrun the tests first"));
        assert_eq!(inputs.lock().unwrap().len(), 2);
    }

    #[test]
    fn stop_hooks_keep_a_turn_going_only_so_many_times() {
        let steps = (0..=MAX_STOP_BLOCKS).map(|i| text(&format!("done {i}"))).collect();
        let (h, inputs) = hooked(harness("hook-stop-cap", steps), hook("Stop", "", "never"), |_, _| (2, "no"));
        h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");

        assert_eq!(h.provider.requests().len(), MAX_STOP_BLOCKS as usize + 1);
        assert_eq!(inputs.lock().unwrap().len(), MAX_STOP_BLOCKS as usize + 1, "the last one still runs");
        let last = feedback(&h).pop().unwrap();
        assert_eq!((last.1.contains("ends here anyway"), last.2), (true, false));
    }

    // ------------------------------------------------------------------ MCP

    /// A server that answers every call with its name and arguments.
    struct Echo;
    impl crate::domain::mcp::McpClient for Echo {
        fn list_tools(&self) -> Result<Vec<crate::domain::mcp::McpTool>, crate::domain::mcp::McpError> {
            Ok(vec![])
        }
        fn call_tool(
            &self,
            name: &str,
            arguments: serde_json::Value,
            _: &dyn Fn() -> bool,
        ) -> Result<crate::domain::mcp::McpCallResult, crate::domain::mcp::McpError> {
            Ok(crate::domain::mcp::McpCallResult { text: format!("{name} got {arguments}"), is_error: false })
        }
    }

    fn with_server(mut h: Harness, weight: u32) -> Harness {
        h.mcp = McpTools::new(vec![crate::domain::mcp::ConnectedServer {
            name: "tracker".into(),
            weight,
            client: Arc::new(Echo),
            tools: vec![crate::domain::mcp::McpTool {
                name: "find".into(),
                description: "Finds issues.".into(),
                input_schema: serde_json::json!({"type": "object"}),
            }],
        }]);
        h
    }

    /// Offered in Agent beside the built-in tools; not in Plan, which
    /// promises nothing changes, and a foreign tool promises nothing.
    #[test]
    fn a_servers_tools_are_offered_in_agent_mode_only() {
        let agent = with_server(harness("mcp-offered", vec![text("hi")]), 3);
        agent.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");
        let offered = &agent.provider.requests()[0].tools;
        let tracker = offered.iter().find(|t| t.name == "mcp__tracker__find").expect("offered");
        assert_eq!(tracker.description, "[MCP server \"tracker\"] Finds issues.");

        let mut plan = with_server(harness("mcp-plan", vec![asks(vec![wants("m1", "mcp__tracker__find", "{}")]), text("ok")]), 3);
        plan.mode = ConversationMode::Plan;
        plan.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");
        let requests = plan.provider.requests();
        assert!(requests[0].tools.iter().all(|t| !t.name.starts_with("mcp__")));
        let refused = requests[1].messages.iter().find(|m| m.tool_call_id.as_deref() == Some("m1")).unwrap();
        assert!(refused.content.as_deref().unwrap().contains("not available in this conversation mode"));
    }

    /// The same gate as a write: it asks, and the round is charged the
    /// server's weight, not a built-in tool's.
    #[test]
    fn a_call_asks_first_and_costs_its_servers_weight() {
        let mut h = with_server(harness("mcp-asks", vec![asks(vec![wants("m1", "mcp__tracker__find", r#"{"q":"crash"}"#)])]), 7);
        h.approval = asking();

        let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]));
        let ChatStreamOutcome::PendingApproval(pending) = outcome.expect("pauses") else {
            panic!("expected a pause");
        };
        assert!(pending.calls[0].requires_confirmation);
        assert_eq!(pending.budget_used, 7);
    }

    /// "Always allow" for one server tool holds in the loop, not only in the
    /// policy's own tests: the loop has to ask the MCP-aware gate.
    #[test]
    fn a_tool_allowed_always_runs_without_asking() {
        let mut h = with_server(harness("mcp-always", vec![asks(vec![wants("m1", "mcp__tracker__find", "{}")]), text("done")]), 3);
        h.approval = asking();
        h.approval.allow_always("mcp__tracker__find").unwrap();

        let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]));
        assert!(matches!(outcome, Ok(ChatStreamOutcome::Done(_))), "it paused");
    }

    /// Stop reaches a call that is waiting on a server, not only the loop
    /// around it: the server is told and the call ends as cancelled.
    #[test]
    fn stopping_the_turn_reaches_a_server_call_in_flight() {
        struct WaitsForStop(Arc<Mutex<Option<usize>>>);
        impl crate::domain::mcp::McpClient for WaitsForStop {
            fn list_tools(&self) -> Result<Vec<crate::domain::mcp::McpTool>, crate::domain::mcp::McpError> {
                Ok(vec![])
            }
            fn call_tool(
                &self,
                _: &str,
                _: serde_json::Value,
                cancelled: &dyn Fn() -> bool,
            ) -> Result<crate::domain::mcp::McpCallResult, crate::domain::mcp::McpError> {
                // The user presses Stop while this call is running.
                *self.0.lock().unwrap() = Some(0);
                for _ in 0..200 {
                    if cancelled() {
                        return Err(crate::domain::mcp::McpError::Cancelled);
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                Ok(crate::domain::mcp::McpCallResult { text: "never stopped".into(), is_error: false })
            }
        }
        let mut h = harness("mcp-stop", vec![asks(vec![wants("m1", "mcp__slow__wait", "{}")])]);
        h.mcp = McpTools::new(vec![crate::domain::mcp::ConnectedServer {
            name: "slow".into(),
            weight: 3,
            client: Arc::new(WaitsForStop(h.cancel_after.clone())),
            tools: vec![crate::domain::mcp::McpTool { name: "wait".into(), description: String::new(), input_schema: serde_json::json!({}) }],
        }]);

        let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]));
        assert!(matches!(outcome, Ok(ChatStreamOutcome::Cancelled(_))));
        let logged = h.logged.lock().unwrap();
        assert_eq!(logged[0].status, CallStatus::Error, "the call ran to its end instead: {:?}", logged[0]);
    }

    /// Run: the server's text reaches the model as it is, and the log line
    /// keeps the tool and the shape of what was sent — not the values.
    #[test]
    fn a_call_runs_through_the_log_and_its_text_reaches_the_model() {
        let h = with_server(
            harness("mcp-runs", vec![asks(vec![wants("m1", "mcp__tracker__find", r#"{"q":"secret words"}"#)]), text("done")]),
            3,
        );
        h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");

        let second = &h.provider.requests()[1];
        let result = second.messages.iter().find(|m| m.tool_call_id.as_deref() == Some("m1")).unwrap();
        assert_eq!(result.content.as_deref(), Some(r#"find got {"q":"secret words"}"#));

        let logged = h.logged.lock().unwrap();
        assert_eq!(logged[0].tool, "mcp__tracker__find");
        assert_eq!(logged[0].status, CallStatus::Ok);
        let line = serde_json::to_string(&*logged).unwrap();
        assert!(!line.contains("secret words"), "{line}");
        assert!(line.contains("<string, 12 chars>"), "{line}");
    }

    // ------------------------------------------------------------- steering

    fn user_messages(request: &ChatRequest) -> Vec<String> {
        request
            .messages
            .iter()
            .filter(|m| m.role == LlmRole::User)
            .filter_map(|m| m.content.clone())
            .collect()
    }

    /// The point of steering: what the user typed mid-turn reaches the model
    /// on the next round, marked as a clarification rather than a new task.
    #[test]
    fn a_note_typed_mid_turn_reaches_the_next_round() {
        let h = harness(
            "steer-applied",
            vec![
                asks_while_typing(
                    vec![wants("l1", "listFiles", "{}")],
                    "use the existing helper",
                ),
                text("understood"),
            ],
        );

        h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");

        let second = user_messages(&h.provider.requests()[1]);
        assert!(
            second.iter().any(|m| m.contains("use the existing helper")),
            "{second:?}"
        );
        assert!(
            second.iter().any(|m| m.starts_with(STEERING_PREFIX)),
            "a clarification, not a new task: {second:?}"
        );
        let applied: Vec<String> = payloads(&h.events())
            .into_iter()
            .filter(|p| p.starts_with("steering:"))
            .collect();
        assert_eq!(applied.len(), 1, "the note is announced by id: {applied:?}");
    }

    /// Ending the turn here would silently drop what the user typed, with
    /// nothing to tell them it was never seen.
    #[test]
    fn a_note_that_arrives_as_the_model_finishes_keeps_the_turn_going() {
        let h = harness(
            "steer-late",
            vec![
                text_while_typing("all done", "and rename it too"),
                text("also did the other thing"),
            ],
        );

        let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]));

        let ChatStreamOutcome::Done(done) = outcome.expect("finishes") else {
            panic!("expected Done");
        };
        assert_eq!(done.result.text, "also did the other thing");
        assert_eq!(h.provider.requests().len(), 2, "the turn ran another round");
        let second = user_messages(&h.provider.requests()[1]);
        assert!(second.iter().any(|m| m.contains("and rename it too")), "{second:?}");
        // The answer the model had already given stays in the conversation,
        // or the extra round reads as if it never spoke.
        assert!(h.provider.requests()[1]
            .messages
            .iter()
            .any(|m| m.role == LlmRole::Assistant && m.content.as_deref() == Some("all done")));
    }

    /// A note is handed over once. A round that is retried, or a turn that
    /// pauses in between, must not deliver it a second time.
    #[test]
    fn a_note_is_delivered_once() {
        let h = harness(
            "steer-once",
            vec![
                asks_while_typing(vec![wants("l1", "listFiles", "{}")], "watch the indentation"),
                asks(vec![wants("l2", "listFiles", "{}")]),
                text("done"),
            ],
        );

        h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");

        let delivered = user_messages(h.provider.requests().last().unwrap())
            .iter()
            .filter(|m| m.contains("watch the indentation"))
            .count();
        assert_eq!(delivered, 1);
    }

    /// The user typed it at a conversation that had already finished. It is
    /// their next message, not a clarification of a turn they cannot see.
    #[test]
    fn a_note_left_over_from_a_finished_turn_does_not_leak_into_the_next() {
        let h = harness("steer-leftover", vec![text("hi")]);
        h.steering.push(SteeringNote::user("from the previous turn"));

        h.run(|turn| stream(turn, vec![LlmMessage::user("a new question")], vec![]))
            .expect("finishes");

        let sent = user_messages(&h.provider.requests()[0]);
        assert_eq!(sent, ["a new question"]);
    }

    /// A `/` command typed mid-turn: the model gets its prompt, and the
    /// transcript the command as typed.
    #[test]
    fn a_note_shown_as_a_command_tells_the_model_its_prompt() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let seen = log.clone();
        let sink: ChatEventSink = Arc::new(move |event| seen.lock().unwrap().push(event));
        let events = Events::new(&sink, 0);
        let mut history = Vec::new();
        let note = SteeringNote { shown: Some("/review a.rs".to_string()), ..SteeringNote::user("Review a.rs line by line") };

        apply_steering(&events, 1, &mut history, vec![note, SteeringNote::user("and b.rs")]);

        assert_eq!(
            history.iter().map(|m| m.content.clone().unwrap_or_default()).collect::<Vec<_>>(),
            [format!("{STEERING_PREFIX}Review a.rs line by line"), format!("{STEERING_PREFIX}and b.rs")]
        );
        let shown: Vec<String> = log
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match &e.event {
                ChatEventPayload::SteeringApplied { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(shown, ["/review a.rs", "and b.rs"]);
    }

    #[test]
    fn a_queued_note_can_be_cancelled_until_a_round_takes_it() {
        let queue = SteeringQueue::default();
        let note = SteeringNote::user("never mind");
        let id = note.id.clone();
        queue.push(note);

        assert!(queue.cancel(&id), "still queued");
        assert!(queue.take().is_empty());
        // Cancelling what a round already picked up answers `false`: what has
        // been said to the model cannot be unsaid.
        assert!(!queue.cancel(&id));
    }

    /// Two identical clarifications are indistinguishable by text, which is
    /// why cancelling works by id.
    #[test]
    fn identical_notes_are_still_separate() {
        let queue = SteeringQueue::default();
        let first = SteeringNote::user("check the locale");
        let second = SteeringNote::user("check the locale");
        assert_ne!(first.id, second.id);
        let first_id = first.id.clone();
        queue.push(first);
        queue.push(second);

        assert!(queue.cancel(&first_id));
        assert_eq!(queue.take().len(), 1);
    }

    // --------------------------------------------------- the stage's own bar

    /// What stage one set out to produce: a turn that reads a file, changes
    /// it, and hands the model back what actually landed on disk — not just
    /// `{"path": "…"}`, which tells it nothing about whether the edit took.
    #[test]
    fn a_turn_reads_a_file_edits_it_and_is_told_what_changed() {
        let h = harness(
            "stage-one",
            vec![
                asks(vec![wants("r1", "readFile", r#"{"path":"lib.rs"}"#)]),
                asks(vec![wants(
                    "e1",
                    "editFile",
                    r#"{"path":"lib.rs","edits":[{"old":"one","new":"two"}]}"#,
                )]),
                text("renamed it"),
            ],
        );
        std::fs::write(h.root.join("lib.rs"), "fn one() {}\n").unwrap();

        let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("rename one to two")], vec![]));

        let ChatStreamOutcome::Done(done) = outcome.expect("finishes") else {
            panic!("not done")
        };
        // What the next message is sent with: the calls and what they
        // returned, not only the answer.
        let shape: Vec<(LlmRole, usize)> = done.history.iter().map(|m| (m.role, m.tool_calls.len())).collect();
        assert_eq!(
            shape,
            [
                (LlmRole::User, 0),
                (LlmRole::Assistant, 1),
                (LlmRole::Tool, 0),
                (LlmRole::Assistant, 1),
                (LlmRole::Tool, 0),
                (LlmRole::Assistant, 0),
            ]
        );
        assert_eq!(done.history[2].content.as_deref().map(|c| c.starts_with("All 1 lines:")), Some(true));
        assert_eq!(done.history.last().and_then(|m| m.content.as_deref()), Some("renamed it"));
        assert_eq!(
            std::fs::read_to_string(h.root.join("lib.rs")).unwrap(),
            "fn two() {}\n"
        );
        let told = tool_contents(h.provider.requests().last().unwrap());
        let edit = told.last().expect("the edit was reported");
        // Plain text, not a JSON string of escapes: the counts, then the diff.
        assert!(edit.starts_with("Edited lib.rs (+1 -1 lines)\n```diff\n"), "{edit}");
        assert!(edit.contains("\n-fn one"), "no diff itself: {edit}");
    }

    /// A git diff reaches the model the way a write does: a line, then the
    /// diff as a diff — not a JSON string of `\n` escapes.
    #[test]
    fn a_git_diff_reaches_the_model_as_a_diff() {
        let h = harness(
            "loop-git-diff",
            vec![asks(vec![wants("d1", "gitDiff", r#"{"path":"a.md"}"#)]), text("seen")],
        );
        std::fs::write(h.root.join("a.md"), "one\n").unwrap();
        let repo = git2::Repository::init(&h.root).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(std::path::Path::new("a.md")).unwrap();
        index.write().unwrap();
        std::fs::write(h.root.join("a.md"), "uno\n").unwrap();

        h.run(|turn| stream(turn, vec![LlmMessage::user("what changed")], vec![]))
            .expect("finishes");

        let told = tool_contents(h.provider.requests().last().unwrap());
        assert!(told[0].starts_with("Diff (index → working tree): a.md (+1 -1 lines)\n```diff\n"), "{}", told[0]);
        assert!(told[0].contains("\n-one\n+uno\n"), "{}", told[0]);
    }

    /// The turn hands the tool the folder's search; without it the model gets
    /// "unavailable" and a pointer to grep.
    #[test]
    fn a_turn_searches_through_the_search_it_was_given() {
        use crate::domain::code_search::{CodeMatch, CodeSearchResult, MatchSource, SearchMeta};
        let mut h = harness(
            "turn-search",
            vec![asks(vec![wants("s1", "semanticSearch", r#"{"query":"where sync lives"}"#)]), text("found it")],
        );
        h.search = Some(Arc::new(|_: &[&str], _: Option<&[String]>, _: usize, _: &crate::domain::code_search::SearchFilter| {
            Ok(CodeSearchResult {
                matches: vec![CodeMatch {
                    path: "src/sync.rs".into(),
                    start_line: 1,
                    end_line: 2,
                    name: None,
                    text: "fn sync() {}".into(),
                    source: MatchSource::Lexical,
                }],
                meta: SearchMeta { tiers_used: vec![MatchSource::Lexical], weak: false, hint: None },
            })
        }));

        h.run(|turn| stream(turn, vec![LlmMessage::user("where is sync")], vec![])).expect("finishes");

        let told = tool_contents(h.provider.requests().last().unwrap());
        assert!(told[0].contains("src/sync.rs"), "{told:?}");
    }

    /// The catalog the turn was given is what every request lists, and what
    /// the tool loads from — the model can load only what it was told exists.
    #[test]
    fn every_request_lists_the_turns_skills_and_the_tool_loads_them() {
        crate::testing::with_app_dir("turn-skills", || {
            crate::infra::skills_store::test_support::write_skill("release", "Cuts a release.", "Bump the version.");
            let mut h = harness(
                "turn-skills",
                vec![asks(vec![wants("k1", "skill", r#"{"name":"release"}"#)]), text("done")],
            );
            h.skills = vec![Skill {
                meta: crate::domain::skills::SkillMeta { name: "release".into(), description: "Cuts a release.".into() },
                dir: crate::infra::skills_store::dir().unwrap().join("release"),
            }];

            h.run(|turn| stream(turn, vec![LlmMessage::user("ship it")], vec![])).expect("finishes");

            let requests = h.provider.requests();
            let told = tool_contents(requests.last().unwrap());
            assert!(told[0].contains("Bump the version."), "{told:?}");
            assert_requests_list_release(requests);
        });
    }

    #[test]
    fn every_request_carries_the_plan_as_the_window_holds_it() {
        let mut h = harness("turn-plan", vec![asks(vec![wants("r1", "listFiles", r#"{"path":"."}"#)]), text("done")]);
        h.plan = Some("# Fix\n\n1. edited by the user".into());

        h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");

        for request in h.provider.requests() {
            assert!(
                request.messages.iter().any(|m| m.content.as_deref().is_some_and(|c| c.contains("1. edited by the user"))),
                "a request went out without the plan"
            );
        }
    }

    #[test]
    fn every_request_in_a_worktree_names_the_checkout_to_keep_out_of() {
        let mut h = harness("turn-worktree", vec![asks(vec![wants("r1", "listFiles", r#"{"path":"."}"#)]), text("done")]);
        h.worktree_of = Some(PathBuf::from("/work/the-main-checkout"));

        h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");

        for request in h.provider.requests() {
            assert!(
                request.messages.iter().any(|m| m.content.as_deref().is_some_and(|c| c.contains("git worktree of /work/the-main-checkout"))),
                "a request went out without saying it is a worktree"
            );
        }
    }

    #[test]
    fn every_request_carries_the_project_instructions() {
        let mut h = harness("turn-rules", vec![asks(vec![wants("r1", "listFiles", r#"{"path":"."}"#)]), text("done")]);
        h.rules = vec![RuleFile::new("AGENTS.md", "Run cargo test before saying done.")];

        h.run(|turn| stream(turn, vec![LlmMessage::user("fix it")], vec![])).expect("finishes");

        let requests = h.provider.requests();
        assert_eq!(requests.len(), 2);
        for request in requests {
            assert!(
                request.messages.iter().any(|m| m.content.as_deref().is_some_and(|c| c.contains("Run cargo test before saying done."))),
                "a request went out without the project instructions"
            );
        }
    }

    fn assert_requests_list_release(requests: Vec<ChatRequest>) {
        assert_eq!(requests.len(), 2);
        for request in requests {
            assert!(
                request.messages.iter().any(|m| m.content.as_deref().is_some_and(|c| c.contains("- release: Cuts a release."))),
                "a request went out without the skills"
            );
        }
    }

    /// Every field of the checkpoint, exercised through a real pause rather
    /// than by serializing a fixture: the checklist an earlier round built has
    /// to come out the other side, along with the history, the ceilings, the
    /// numbering and the read registry.
    #[test]
    fn the_whole_checkpoint_survives_a_real_pause() {
        let mut h = harness(
            "stage-one-checkpoint",
            vec![
                asks(vec![wants(
                    "t1",
                    "todo",
                    r#"{"op":"write","tasks":["read it","rewrite it"]}"#,
                )]),
                asks(vec![wants("r1", "readFile", r#"{"path":"a.rs"}"#)]),
                asks(vec![wants(
                    "w1",
                    "writeFile",
                    r#"{"path":"a.rs","content":"new"}"#,
                )]),
                text("done"),
            ],
        );
        h.approval = asking();
        std::fs::write(h.root.join("a.rs"), "old").unwrap();

        let ChatStreamOutcome::PendingApproval(pending) = h
            .run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]))
            .expect("pauses")
        else {
            panic!("expected a pause");
        };

        assert_eq!(pending.round, 3, "the ceiling cannot be reset by pausing");
        assert!(pending.budget_used > 0);
        assert_eq!(pending.event_seq, h.events().last().unwrap().seq);
        assert_eq!(pending.calls.len(), 1);
        assert_eq!(
            pending.todos.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(),
            ["read it", "rewrite it"],
            "the checklist an earlier round built"
        );
        assert!(pending.history.len() > 1);
        assert!(pending.reads.check("a.rs", "old", true).is_ok());

        let outcome = h.run(|turn| {
            resume(
                turn,
                pending,
                vec![ToolCallDecision {
                    id: "w1".to_string(),
                    approved: true,
                    reason: None,
                }],
            )
        });

        let ChatStreamOutcome::Done(done) = outcome.expect("finishes") else {
            panic!("expected Done");
        };
        assert_eq!(std::fs::read_to_string(h.root.join("a.rs")).unwrap(), "new");
        assert_eq!(done.todos.len(), 2, "and the checklist comes out with it");
    }

    // ---------------------------------------------------- running a command

    /// Scenario S-1, the reason stage two exists: the agent runs the tests,
    /// reads the failure, fixes the code, and runs them again. Nothing about
    /// it is mocked except the model's side of the conversation.
    #[test]
    fn the_agent_runs_a_failing_test_fixes_the_code_and_runs_it_again() {
        let h = harness(
            "s1",
            vec![
                asks(vec![wants("c1", "runCommand", r#"{"command":"sh check.sh"}"#)]),
                asks(vec![wants("r1", "readFile", r#"{"path":"answer.txt"}"#)]),
                asks(vec![wants(
                    "e1",
                    "editFile",
                    r#"{"path":"answer.txt","edits":[{"old":"41","new":"42"}]}"#,
                )]),
                asks(vec![wants("c2", "runCommand", r#"{"command":"sh check.sh"}"#)]),
                text("fixed: the answer was 41, it is now 42"),
            ],
        );
        std::fs::write(
            h.root.join("check.sh"),
            "if [ \"$(cat answer.txt)\" = \"42\" ]; then echo PASS; else echo \"FAIL: expected 42, got $(cat answer.txt)\"; exit 1; fi\n",
        )
        .unwrap();
        std::fs::write(h.root.join("answer.txt"), "41").unwrap();

        let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("make the test pass")], vec![]));

        assert!(matches!(outcome.expect("finishes"), ChatStreamOutcome::Done(_)));
        assert_eq!(std::fs::read_to_string(h.root.join("answer.txt")).unwrap(), "42");

        let rounds = h.provider.requests();
        // What the model was told after the first run: the failure, in full.
        let first_run = tool_contents(&rounds[1]).pop().expect("the run was reported");
        assert!(first_run.contains("expected 42, got 41"), "{first_run}");
        assert!(first_run.starts_with("Exit code 1 after "), "{first_run}");
        // And after the second: the pass.
        let second_run = tool_contents(rounds.last().unwrap()).pop().expect("reported");
        assert!(second_run.contains("PASS"), "{second_run}");
        assert!(second_run.starts_with("Exit code 0 after "), "{second_run}");
    }

    /// Output reaches the UI while the command is still running, tagged with
    /// the call it belongs to — otherwise a two-minute build is two minutes of
    /// nothing.
    #[test]
    fn command_output_is_reported_as_it_is_produced() {
        let h = harness(
            "cmd-stream",
            vec![
                asks(vec![wants(
                    "c1",
                    "runCommand",
                    r#"{"command":"echo working; echo trouble >&2"}"#,
                )]),
                text("done"),
            ],
        );

        h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");

        let chunks: Vec<(String, String)> = h
            .events()
            .into_iter()
            .filter_map(|e| match e.event {
                ChatEventPayload::CommandOutput { id, chunk, .. } => Some((id, chunk)),
                _ => None,
            })
            .collect();
        assert!(!chunks.is_empty(), "nothing was reported while it ran");
        assert!(chunks.iter().all(|(id, _)| id == "c1"), "tagged by call");
        let text: String = chunks.iter().map(|(_, chunk)| chunk.clone()).collect();
        assert!(text.contains("working") && text.contains("trouble"), "{text}");
    }

    /// A command line is not a tool name: `ls` and `rm -rf /` are the same
    /// call. Until the command itself is examined, every one of them asks.
    #[test]
    fn a_command_needs_approval_like_any_other_change() {
        let mut h = harness(
            "cmd-approval",
            vec![asks(vec![wants("c1", "runCommand", r#"{"command":"rm -rf ."}"#)])],
        );
        h.approval = asking();
        std::fs::write(h.root.join("keep.txt"), "x").unwrap();

        let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("clean up")], vec![]));

        let ChatStreamOutcome::PendingApproval(pending) = outcome.expect("pauses") else {
            panic!("expected a pause");
        };
        assert!(pending.calls[0].requires_confirmation);
        assert!(h.root.join("keep.txt").exists(), "nothing ran");
    }

    /// The language setting reaches the model with the session: every
    /// request of the turn is told it.
    #[test]
    fn the_session_s_reply_language_is_in_the_request() {
        let mut h = harness("chat-language", vec![text("готово")]);
        h.session.reply_language = Some("Russian");
        h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");
        let told = h.provider.requests()[0]
            .messages
            .iter()
            .any(|m| m.content.as_deref().is_some_and(|c| c.contains("Write everything you say in Russian")));
        assert!(told, "the language block is in the request");
    }

    /// Every note the loop adds ends by saying the language again: the
    /// model reads it just before it replies, where the system prompt at the
    /// top of a long English history no longer held (`/review`, Chinese).
    #[test]
    fn each_note_of_the_loop_repeats_the_reply_language() {
        const SAID: &str = "\n\n[Reply in Russian.]";
        let russian = |mut h: Harness| {
            h.session.reply_language = Some("Russian");
            h
        };
        let notes_in = |h: &Harness, starts: &str| -> Vec<String> {
            h.provider.requests().last().unwrap().messages.iter()
                .filter(|m| m.role == LlmRole::User)
                .filter_map(|m| m.content.clone())
                .filter(|c| c.starts_with(starts))
                .collect()
        };

        let empty = russian(harness("lang-empty", vec![text(""), text("ok")]));
        empty.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");
        assert_eq!(notes_in(&empty, "[Your last reply was empty"), [format!("{EMPTY_REPLY_NOTE}{SAID}")]);

        let circling = (1..=5).map(|n| asks(vec![wants(&format!("l{n}"), "listFiles", "{}")])).chain([text("done")]).collect();
        let circling = russian(harness("lang-loop", circling));
        circling.run(|turn| stream(turn, vec![LlmMessage::user("look")], vec![])).expect("turn");
        let loop_notes = notes_in(&circling, "");
        assert!(loop_notes.iter().any(|c| c.ends_with(SAID) && c.contains("listFiles")), "{loop_notes:?}");

        let mut ended = russian(harness("lang-ended", vec![text("I see")]));
        ended.processes = Some(Arc::new(EndedOnce(Mutex::new(vec![crate::domain::background::ProcessInfo {
            id: 2,
            command: "npm run dev".into(),
            cwd: ".".into(),
            state: crate::domain::background::ProcessState::Exited { code: Some(1) },
        }]))));
        ended.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");
        assert!(notes_in(&ended, "[Background processes")[0].ends_with(SAID));

        let reviewing = (0..11).map(|n| asks(vec![wants(&format!("l{n}"), "listFiles", &format!(r#"{{"path":"d{n}"}}"#))])).chain([text("done")]).collect();
        let mut review = russian(harness("lang-wrap-up", reviewing));
        review.mode = ConversationMode::Review;
        review.run(|turn| stream(turn, vec![LlmMessage::user("review")], vec![])).expect("turn");
        assert!(notes_in(&review, "[Review budget]")[0].ends_with(SAID));

        let (stopping, _) = hooked(
            russian(harness("lang-stop-hook", vec![text("done"), text("tests pass, done")])),
            hook("Stop", "", "check"),
            |_, input| if input["stop_hook_active"] == true { (0, "") } else { (2, "run the tests first") },
        );
        stopping.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");
        assert!(notes_in(&stopping, "[A Stop hook")[0].ends_with(SAID));

        // Auto says nothing more than the note itself.
        let auto = harness("lang-auto", vec![text(""), text("ok")]);
        auto.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");
        assert_eq!(notes_in(&auto, "[Your last reply was empty"), [EMPTY_REPLY_NOTE.to_string()]);
    }

    /// A review that goes on is asked once to wrap up — at its tenth round
    /// here, the provider reporting no usage — and an agent's turn never is.
    #[test]
    fn a_long_review_is_asked_once_to_wrap_up_and_an_agent_turn_never() {
        let steps = |label: &str| {
            let mut steps: Vec<Step> = (0..12).map(|n| asks(vec![wants(&format!("l{n}"), "listFiles", &format!(r#"{{"path":"d{n}"}}"#))])).collect();
            steps.push(text("done"));
            harness(label, steps)
        };
        let mut review = steps("chat-wrap-up-review");
        review.mode = ConversationMode::Review;
        review.run(|turn| stream(turn, vec![LlmMessage::user("review")], vec![])).expect("turn");
        let said: Vec<String> = payloads(&review.log.lock().unwrap()).into_iter().filter(|p| p.starts_with("wrap-up")).collect();
        assert_eq!(said, ["wrap-up:10"], "once, after the tenth round");
        let notes = review.provider.requests().last().unwrap().messages.iter().filter(|m| m.content.as_deref().is_some_and(|c| c.starts_with("[Review budget]"))).count();
        assert_eq!(notes, 1, "in the history the model reads");

        let agent = steps("chat-wrap-up-agent");
        agent.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");
        assert!(!payloads(&agent.log.lock().unwrap()).iter().any(|p| p.starts_with("wrap-up")));
    }

    /// Tokens count as well as rounds: a review whose requests are heavy is
    /// asked sooner — after its second round here, 400k spent by then.
    #[test]
    fn a_heavy_review_is_asked_to_wrap_up_by_its_tokens() {
        let heavy = |n: u32| {
            Step::Reply(ChatStreamResult {
                tool_calls: vec![wants(&format!("l{n}"), "listFiles", &format!(r#"{{"path":"d{n}"}}"#))],
                usage: Some(ChatUsage { prompt_tokens: 180_000, completion_tokens: 20_000, total_tokens: 200_000, cached_tokens: 0 }),
                ..Default::default()
            })
        };
        let mut h = harness("chat-wrap-up-tokens", vec![heavy(0), heavy(1), heavy(2), text("done")]);
        h.mode = ConversationMode::Review;
        h.run(|turn| stream(turn, vec![LlmMessage::user("review")], vec![])).expect("turn");
        let said: Vec<String> = payloads(&h.log.lock().unwrap()).into_iter().filter(|p| p.starts_with("wrap-up")).collect();
        assert_eq!(said, ["wrap-up:2"]);
    }

    // -------------------------------------------------------------- explore

    fn tool_names(request: &ChatRequest) -> Vec<String> {
        request.tools.iter().map(|t| t.name.clone()).collect()
    }

    /// The helper works in a conversation of its own, and the calling turn
    /// gets its answer — not the file it read to find it.
    #[test]
    fn explore_runs_a_helper_turn_and_hands_back_only_its_answer() {
        let h = harness(
            "explore-helper",
            vec![
                asks(vec![wants("e1", "explore", r#"{"task":"where is X"}"#)]),
                asks(vec![wants("r1", "readFile", r#"{"path":"a.rs"}"#)]),
                text("X is in a.rs:1"),
                text("done"),
            ],
        );
        std::fs::write(h.root.join("a.rs"), "let x = SECRET_LINE;").unwrap();

        let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");
        let ChatStreamOutcome::Done(done) = outcome else { panic!("expected done") };
        assert_eq!(done.result.text, "done");

        let requests = h.provider.requests();
        // The helper's first request: its own brief, reading tools only.
        let helper = &requests[1];
        let said: Vec<_> = helper.messages.iter().filter(|m| m.role != LlmRole::System).collect();
        assert_eq!(said.len(), 1);
        assert_eq!(said[0].content.as_deref(), Some("where is X"));
        assert!(helper.messages.iter().any(|m| m.content.as_deref().is_some_and(|c| c.contains("This conversation: Explore"))));
        let tools = tool_names(helper);
        assert!(tools.contains(&"readFile".to_string()));
        assert!(!tools.contains(&"explore".to_string()) && !tools.contains(&"writeFile".to_string()), "{tools:?}");
        // The caller was offered it, and gets the answer back and nothing else.
        assert!(tool_names(&requests[0]).contains(&"explore".to_string()));
        let back = tool_contents(&requests[3]);
        assert_eq!(back, vec!["X is in a.rs:1".to_string()]);
        assert!(requests[3].messages.iter().all(|m| !m.content.as_deref().unwrap_or("").contains("SECRET_LINE")));

        // What the user sees: the parent's call, with the helper's reads as
        // its output — not the helper's text or cards of its own.
        let events = h.events();
        let lines: Vec<String> = events
            .iter()
            .filter_map(|e| match &e.event {
                ChatEventPayload::CommandOutput { id, chunk, .. } if id == "e1" => Some(chunk.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(lines, vec!["readFile a.rs\n".to_string()]);
        let shown = payloads(&events);
        assert!(shown.contains(&"toolCall:e1".to_string()));
        assert!(!shown.contains(&"toolCall:r1".to_string()), "{shown:?}");
        assert!(events.iter().all(|e| !matches!(&e.event, ChatEventPayload::Delta { delta } if delta.contains("X is in"))));
    }

    /// A helper cannot send a helper: asked from memory, it is refused.
    #[test]
    fn a_helper_asking_to_explore_is_refused() {
        let h = harness(
            "explore-nested",
            vec![
                asks(vec![wants("e1", "explore", r#"{"task":"where is X"}"#)]),
                asks(vec![wants("e2", "explore", r#"{"task":"deeper"}"#)]),
                text("could not look deeper"),
                text("done"),
            ],
        );
        h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");
        let requests = h.provider.requests();
        assert_eq!(requests.len(), 4, "the refused call ran no third turn");
        let refused = tool_contents(&requests[2]);
        assert!(refused[0].contains("`explore` is not available to a helper"), "{refused:?}");
        assert!(!refused[0].contains("the user chose"), "{refused:?}");
        assert_eq!(tool_contents(&requests[3]), vec!["could not look deeper".to_string()]);
    }

    /// The Stop hooks are the calling turn's: a helper finishing is not the
    /// turn ending, and a hook that refused it would steer the wrong model.
    #[test]
    fn a_helper_finishing_does_not_run_the_stop_hooks() {
        let (h, inputs) = hooked(
            harness(
                "explore-stop",
                vec![asks(vec![wants("e1", "explore", r#"{"task":"where is X"}"#)]), text("X is here"), text("done")],
            ),
            hook("Stop", "", "check"),
            |_, _| (0, ""),
        );
        h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");
        assert_eq!(inputs.lock().unwrap().len(), 1);
    }

    /// A helper that says nothing, or runs out of rounds, is an error the
    /// caller can act on — not an empty answer taken for "nothing there".
    #[test]
    fn a_helper_without_an_answer_is_an_error() {
        let h = harness(
            "explore-blank",
            vec![
                asks(vec![wants("e1", "explore", r#"{"task":"where is X"}"#)]),
                text(""),
                text(""),
                text("done"),
            ],
        );
        h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");
        let back = tool_contents(&h.provider.requests()[3]);
        assert_eq!(back, vec!["Error: explore: the helper finished without an answer".to_string()]);
    }

    fn costing(step: Step, prompt: u32, cached: u32, completion: u32) -> Step {
        let Step::Reply(mut result) = step else { unreachable!() };
        result.usage = Some(ChatUsage { prompt_tokens: prompt, completion_tokens: completion, total_tokens: prompt + completion, cached_tokens: cached });
        Step::Reply(result)
    }

    /// The run is in the Agents tab from start to answer, with what it cost;
    /// the cost is on the card, and never on the chat's own context meter.
    #[test]
    fn a_helper_run_is_recorded_with_its_steps_answer_and_tokens() {
        use crate::domain::agents::{AgentState, AgentTokens, Agents};
        let mut h = harness(
            "explore-record",
            vec![
                asks(vec![wants("e1", "explore", r#"{"task":"where is X"}"#)]),
                costing(asks(vec![wants("g1", "grep", r#"{"pattern":"X"}"#)]), 1000, 600, 40),
                costing(text("X is in a.rs:1"), 1500, 1000, 20),
                text("done"),
            ],
        );
        let agents = Arc::new(Agents::default());
        h.agents = Some(Arc::clone(&agents));
        h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");

        let run = &agents.list()[0];
        assert_eq!((run.task.as_str(), &run.state, run.answer.as_deref()), ("where is X", &AgentState::Done, Some("X is in a.rs:1")));
        assert_eq!(run.steps, ["grep X"]);
        let spent = AgentTokens { prompt: 2500, cached: 1600, completion: 60 };
        assert_eq!(run.tokens, spent);
        let card = h.events().into_iter().find_map(|e| match e.event {
            ChatEventPayload::ToolResult(r) if r.id == "e1" => r.result,
            _ => None,
        });
        assert_eq!(card, Some(ToolResult::Explored { text: "X is in a.rs:1".into(), agent: run.id, tokens: spent }));
        assert!(!payloads(&h.events()).contains(&"contextUsage".to_string()), "the helper's usage is not the chat's");
    }

    /// Stop in the Agents tab ends the helper, not the turn: the caller is
    /// told why, and goes on.
    #[test]
    fn a_helper_stopped_from_the_tab_is_an_error_the_turn_carries_on_from() {
        use crate::domain::agents::{AgentChanged, AgentState, Agents};
        use std::sync::OnceLock;
        let mut h = harness(
            "explore-tab-stop",
            vec![asks(vec![wants("e1", "explore", r#"{"task":"where is X"}"#)]), text("done without it")],
        );
        // The user presses Stop as soon as the run shows up in the tab.
        let registry: Arc<OnceLock<Arc<Agents>>> = Arc::default();
        let reach = Arc::clone(&registry);
        let pressed = Arc::new(Mutex::new(false));
        let agents = Arc::new(Agents::new(Arc::new(move |event: AgentChanged| {
            let mut pressed = pressed.lock().unwrap();
            if !*pressed {
                *pressed = true;
                drop(pressed);
                reach.get().unwrap().stop(event.id).unwrap();
            }
        })));
        registry.set(Arc::clone(&agents)).ok();
        h.agents = Some(Arc::clone(&agents));

        let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");
        let ChatStreamOutcome::Done(done) = outcome else { panic!("the turn goes on") };
        assert_eq!(done.result.text, "done without it");
        let back = tool_contents(&h.provider.requests()[1]);
        assert!(back[0].starts_with("Error: explore: the user stopped the helper"), "{back:?}");
        assert_eq!(agents.list()[0].state, AgentState::Stopped);
    }

    /// Stop pressed while the helper works stops both, and the call says it
    /// was stopped — not an empty answer the model would take for "nothing
    /// there" when the chat is continued.
    #[test]
    fn a_stop_during_the_helper_ends_the_turn_and_says_so() {
        let h = harness("explore-stop-button", vec![asks(vec![wants("e1", "explore", r#"{"task":"where is X"}"#)])]);
        // Polls 1–3 are the calling turn's, up to its call; 4 is the helper's first.
        h.cancel_at_poll(4);
        let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");
        let ChatStreamOutcome::Cancelled(done) = outcome else { panic!("expected cancelled") };
        assert_eq!(h.provider.requests().len(), 1, "the helper never asked the model");
        let answer = done.history.iter().find(|m| m.tool_call_id.as_deref() == Some("e1")).unwrap();
        assert_eq!(answer.content.as_deref(), Some("Error: explore: stopped before it answered"));
    }

    #[test]
    fn a_helper_step_names_what_it_was_pointed_at() {
        assert_eq!(explore_step("grep", r#"{"pattern":"fn main","path":"src"}"#), "grep src");
        assert_eq!(explore_step("semanticSearch", r#"{"query":"where tokens refresh"}"#), "semanticSearch where tokens refresh");
        assert_eq!(explore_step("gitStatus", "{}"), "gitStatus");
        assert_eq!(explore_step("readFile", "{broken"), "readFile");
    }
}
