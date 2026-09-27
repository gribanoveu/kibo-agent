//! The turn protocol: what a running turn reports, and how a paused one is
//! resumed.
//!
//! Everything a front end knows about a turn in flight arrives as one ordered
//! stream of [`ChatTurnEvent`], and the stream goes out through a sink rather
//! than through any UI type. That is what keeps the loop testable without a
//! window, and what lets the same loop serve a transcript, a log and a test.
//!
//! A turn ends one of three ways, and *pausing is one of them* — not an
//! exception routed around the normal path. A pause produces a complete
//! checkpoint the caller sends back to continue, because nothing here keeps
//! session state of its own between calls.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::domain::command_exec::OutputStream;
use crate::domain::llm::{ChatStreamResult, ChatUsage, LlmMessage};
use crate::domain::tools::{ReadFiles, Task, ToolResult};

/// How one turn ended.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", content = "value", rename_all = "camelCase")]
pub enum ChatStreamOutcome {
    Done(ChatDone),
    /// A round asked for something that needs a human. Nothing in that round
    /// has run.
    PendingApproval(PendingApproval),
    /// Stopped mid-flight by the user. Same shape as `Done` so a transcript
    /// needs no second code path — only a different label.
    Cancelled(ChatDone),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatDone {
    #[serde(flatten)]
    pub result: ChatStreamResult,
    pub todos: Vec<Task>,
    /// The conversation as the turn left it — every call and result, the
    /// answer last — for the window to send with the next message. Without
    /// it the next turn sees only the answers, and anything the model read but
    /// did not repeat in one is gone.
    ///
    /// Always valid to send on: a turn stopped between a round and its calls
    /// keeps what that round said, not the calls it never ran.
    pub history: Vec<LlmMessage>,
}

/// A whole round paused, unexecuted — and the entire state needed to continue.
///
/// Every field is here because leaving it out broke something specific:
///
/// - `history` — the conversation including this round's request. Nothing from
///   the round has run, so this is exactly where to pick up.
/// - `round` and `budget_used` — carried through so a turn cannot pause
///   repeatedly to escape the loop's ceilings. Without them, "ask again" is an
///   unlimited budget.
/// - `event_seq` — the last emitted sequence number. A resume continues from
///   it, so one monotonic stream survives the gap; starting again from zero
///   makes a listener that reconnects mid-turn unable to order anything.
/// - `calls` — every call the round asked for, in order, including the ones
///   needing no decision. They run on resume; dropping them would silently
///   discard half a round.
/// - `todos` — an earlier round of the same turn may already have changed the
///   checklist. This carries the loop's accumulated state, not "unchanged
///   since the turn began".
/// - `reads` — what the agent has read so far this turn. Alfa Atlas has no
///   such registry and so has six fields here; with one, leaving it out makes
///   the pause itself destructive: the approved write lands on a file the
///   resumed turn no longer remembers reading, and is refused for exactly the
///   reason the user just approved it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingApproval {
    pub history: Vec<LlmMessage>,
    pub round: u32,
    pub budget_used: u32,
    #[serde(default)]
    pub event_seq: u64,
    pub calls: Vec<PendingToolCall>,
    #[serde(default)]
    pub todos: Vec<Task>,
    #[serde(default)]
    pub reads: ReadFiles,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
    pub requires_confirmation: bool,
    /// Why it asks even though its tool is always allowed — `rewrites a
    /// remote (git push --force)`. Absent for an ordinary card.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// One answer to one pending call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallDecision {
    pub id: String,
    pub approved: bool,
    /// Why it was refused, in the user's words, passed to the model as the
    /// call's result.
    ///
    /// Not in Alfa Atlas, which reported a bare refusal — and a model told only
    /// "denied" characteristically tries the same call again, then a near
    /// variant of it. "Denied: use the existing helper instead" ends that in
    /// one round. Optional: a refusal with nothing to add stays a bare refusal.
    #[serde(default)]
    pub reason: Option<String>,
}

impl PendingApproval {
    /// The calls a human has to answer. The rest run on resume.
    pub fn awaiting_decision(&self) -> impl Iterator<Item = &PendingToolCall> {
        self.calls.iter().filter(|c| c.requires_confirmation)
    }

    /// Checks the decisions before any of them is acted on.
    ///
    /// Validated here rather than trusted, because the caller is across a
    /// process boundary: a resume that arrives missing one decision must not
    /// quietly fall back to running the call, and an id that matches nothing
    /// means the two sides disagree about what was asked — which is the moment
    /// to stop, not to guess.
    pub fn check_decisions(&self, decisions: &[ToolCallDecision]) -> Result<(), DecisionError> {
        for decision in decisions {
            if !self.calls.iter().any(|c| c.id == decision.id) {
                return Err(DecisionError::Unknown(decision.id.clone()));
            }
        }
        for call in self.awaiting_decision() {
            if !decisions.iter().any(|d| d.id == call.id) {
                return Err(DecisionError::Missing(call.id.clone()));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecisionError {
    #[error("no decision for tool call {0}, which needs one")]
    Missing(String),
    #[error("decision for unknown tool call {0}")]
    Unknown(String),
}

/// What the model is told a mid-turn note is: a clarification of the work in
/// progress, not a new task. Without the distinction a note like "use the
/// existing helper" reads as a fresh instruction, and the model starts over.
pub const STEERING_PREFIX: &str =
    "[Clarification from the user, not a new task — take it into account in the work in progress]: ";

/// One note the user typed while the turn was running.
///
/// Alfa Atlas also carries a `source`, because the app queues notes of its own
/// (a nudge when the model promised a diagram and drew none) and those must not
/// be attributed to the user — the model otherwise apologises to them for
/// something they never said. Those backstops are not ported (they belong to a
/// documentation product), so there is one kind of note here. The place a note
/// joins the history is the extension point, not a field nothing sets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SteeringNote {
    /// The note's own id. Cancelling needs it: two identical clarifications
    /// are indistinguishable by text.
    pub id: String,
    /// What the model is told.
    pub text: String,
    /// What the transcript shows instead, when that differs: `/review a.rs`
    /// for the page of prompt the command stands for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shown: Option<String>,
}

impl SteeringNote {
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            text: text.into(),
            shown: None,
        }
    }

    /// What goes into the history.
    pub fn prefixed(&self) -> String {
        format!("{STEERING_PREFIX}{}", self.text)
    }
}

/// One ordered event in a turn.
///
/// `seq` is monotonic across a pause and resume — the whole point, since those
/// are separate calls. `round` says which loop iteration produced it.
/// `target_id` gives an event that mutates something already shown a stable
/// identity, so applying it is an upsert rather than an append.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatTurnEvent {
    pub seq: u64,
    pub round: u32,
    #[serde(default)]
    pub target_id: Option<String>,
    #[serde(flatten)]
    pub event: ChatEventPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "camelCase")]
pub enum ChatEventPayload {
    /// One chunk of answer text.
    Delta { delta: String },
    /// One chunk of a reasoning model's thinking, kept apart from the answer.
    Reasoning { delta: String },
    /// The request dropped before a single byte arrived and is being retried.
    /// Emitted before the wait, so the pause has a visible reason rather than
    /// looking like a hang.
    Retrying {
        attempt: u32,
        max_attempts: u32,
        delay_seconds: u64,
    },
    /// The older part of the conversation has been replaced by a summary.
    ///
    /// Said out loud because the alternative is history disappearing on its
    /// own: the model stops remembering something it was told, and nothing in
    /// the window explains why. `folded` is how many messages the summary now
    /// stands for — the transcript keeps showing all of them, since what the
    /// reader sees and what the model reads are different lists.
    HistoryCompacted { folded: usize },
    /// A summary of the older conversation has been asked for — the slow part
    /// of a pass, seconds long. Sent only once there is something to fold, so
    /// it is always followed by [`Self::HistoryCompacted`] or by the pass
    /// giving up; the window shows the pass as under way in between.
    HistoryCompacting,
    /// A note the user typed mid-turn has been added to the conversation.
    /// Carries the id so the front end can retire that queued note by
    /// identity rather than by matching its text. `text` is the note as the
    /// transcript shows it — its `shown`, when it has one.
    SteeringApplied { id: String, text: String },
    /// A fresh round is starting.
    ///
    /// Stated outright rather than inferred from whatever came next. Before
    /// this existed, only a tool call could close a text block — so a round
    /// that ended in prose and was followed by another round had its two
    /// answers concatenated mid-sentence, permanently, in the transcript and
    /// in everything replayed from it.
    RoundStarted,
    /// A round finished streaming, with the authoritative text it produced.
    ///
    /// Deltas are what build the prose, and a dropped delta is permanent once
    /// the transcript is saved. Reconciling only at the end of a *turn* leaves
    /// every round that was closed by a tool call unchecked — in the transcript
    /// that prompted this, those ended mid-word for the reader while the full
    /// text sat in the history and went to the model. Fires for every round,
    /// the last included, and before the pause check, so a round that stops for
    /// a confirmation has already reported what it said.
    RoundCompleted {
        text: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        reasoning: String,
        /// The provider stopped at the response length limit rather than
        /// because the model was done. Left out when false.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        truncated: bool,
    },
    /// A call's arguments are still arriving. Always followed by `ToolCall`
    /// with the same id unless the turn is cancelled first, and the JSON may
    /// be incomplete until then.
    ToolCallDelta(ToolCallEvent),
    /// A call is about to run. Always followed by exactly one `ToolResult`
    /// with the same id: listeners pair them by it, so the order is a
    /// contract rather than an accident.
    ToolCall(ToolCallEvent),
    ToolResult(ToolResultEvent),
    /// A line of a running command's output, as it is produced.
    ///
    /// Unlike every other event here this one carries `seq: 0`: it is written
    /// from the command's own reader threads, where the loop's cursor is not
    /// available. Ordering it against the rest of the stream is neither
    /// possible nor needed — it belongs to the call named by `id`, and the
    /// call's result is still the authoritative text.
    CommandOutput {
        id: String,
        stream: OutputStream,
        chunk: String,
    },
    /// A hook refused something or failed. `blocked` tells the two apart: a
    /// refusal changed what happened, a failure only did not run.
    HookFeedback {
        event: String,
        message: String,
        blocked: bool,
    },
    /// Background processes ended while the model was not looking; it has
    /// been told, and so is the transcript.
    ProcessesEnded {
        processes: Vec<crate::domain::background::ProcessInfo>,
    },
    /// The model was told it is going round in circles
    /// (`domain::loop_guard`): the same call and result, or the same failure,
    /// several rounds running. Said to the reader too, since the note is in
    /// the history the model reads. `failing` tells the two apart.
    LoopReminded { tool: String, failing: bool },
    /// `/review` has gone on long enough and was asked to wrap up
    /// (`domain::review::wrap_up`). Said to the reader too, since the note is
    /// in the history the model reads.
    WrapUpReminded { rounds: u32, tokens: u64 },
    /// Token usage as of the round that just finished. Since every request
    /// resends the whole history, this is the authoritative context size, not
    /// a per-round statistic.
    ContextUsage(ChatUsage),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallEvent {
    /// The model's own call id, which is what pairs this with its result
    /// however many other calls and rounds happen in between.
    pub id: String,
    pub name: String,
    /// Raw JSON, not pre-parsed — a listener that wants structure parses it.
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultEvent {
    pub id: String,
    /// `Some` on success — the typed result, not a rendered string. Turning it
    /// into something a person reads belongs to whoever is showing it.
    #[serde(default)]
    pub result: Option<ToolResult>,
    /// `Some` on failure — the same text the model receives.
    #[serde(default)]
    pub error: Option<String>,
    /// What the call did to files, for a later rewind to undo. Kept by the
    /// window with the call's block; the backend remembers none of it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<crate::domain::rewind::FileChange>,
}

/// Where a turn's events go.
///
/// A port, like `LlmProvider`: services reporting through it never learn what
/// is on the other side, and only the command layer turns these into IPC.
/// `Arc<dyn Fn>` rather than a generic because the sink is moved into the
/// streaming callbacks, which outlive the call that installed them.
pub type ChatEventSink = Arc<dyn Fn(ChatTurnEvent) + Send + Sync>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::llm::LlmToolCall;

    fn call(id: &str, requires_confirmation: bool) -> PendingToolCall {
        PendingToolCall {
            id: id.into(),
            name: "writeFile".into(),
            arguments: "{}".into(),
            requires_confirmation,
            reason: None,
        }
    }

    fn decision(id: &str, approved: bool) -> ToolCallDecision {
        ToolCallDecision {
            id: id.into(),
            approved,
            reason: None,
        }
    }

    fn pending(calls: Vec<PendingToolCall>) -> PendingApproval {
        let mut reads = ReadFiles::default();
        reads.record("a.rs", "fn main() {}", true);
        PendingApproval {
            history: vec![LlmMessage::user("do it")],
            round: 3,
            budget_used: 7,
            event_seq: 42,
            calls,
            todos: Vec::new(),
            reads,
        }
    }

    /// The checkpoint has to survive the round trip whole. A field lost on the
    /// way out is a ceiling reset, a renumbered event stream, or a round that
    /// silently drops half its calls.
    #[test]
    fn the_checkpoint_round_trips_intact() {
        let before = pending(vec![call("a", true), call("b", false)]);

        let json = serde_json::to_string(&before).expect("serializes");
        let after: PendingApproval = serde_json::from_str(&json).expect("parses back");

        assert_eq!(after.round, 3);
        assert_eq!(after.budget_used, 7);
        assert_eq!(after.event_seq, 42);
        assert_eq!(after.calls, before.calls);
        assert_eq!(after.history.len(), 1);
        // The registry has to come back too, or the write the user just
        // approved is refused for never having read the file.
        assert!(after.reads.check("a.rs", "fn main() {}", true).is_ok());
    }

    /// Every field by name, so a field that stops being part of the checkpoint
    /// — dropped, renamed, or marked skip — fails here rather than in a turn
    /// that silently resets a ceiling.
    #[test]
    fn the_checkpoint_carries_exactly_these_fields() {
        let json = serde_json::to_value(pending(vec![call("a", true)])).expect("serializes");
        let mut fields: Vec<&str> = json.as_object().expect("an object").keys().map(String::as_str).collect();
        fields.sort();
        assert_eq!(
            fields,
            ["budgetUsed", "calls", "eventSeq", "history", "reads", "round", "todos"]
        );
    }

    #[test]
    fn only_risky_calls_await_a_decision() {
        let approval = pending(vec![call("a", true), call("b", false), call("c", true)]);
        let ids: Vec<&str> = approval
            .awaiting_decision()
            .map(|c| c.id.as_str())
            .collect();
        assert_eq!(ids, ["a", "c"]);
    }

    /// A resume missing one decision must not fall back to running the call.
    #[test]
    fn a_missing_decision_is_refused() {
        let approval = pending(vec![call("a", true), call("b", true)]);

        let err = approval
            .check_decisions(&[decision("a", true)])
            .expect_err("b was never answered");

        assert_eq!(err, DecisionError::Missing("b".into()));
    }

    /// An id matching nothing means the two sides disagree about what was
    /// asked — the moment to stop rather than to guess.
    #[test]
    fn a_decision_for_an_unknown_call_is_refused() {
        let approval = pending(vec![call("a", true)]);
        assert_eq!(
            approval.check_decisions(&[decision("a", true), decision("z", true)]),
            Err(DecisionError::Unknown("z".into()))
        );
    }

    #[test]
    fn calls_needing_no_decision_need_none_supplied() {
        let approval = pending(vec![call("a", false), call("b", false)]);
        assert_eq!(approval.check_decisions(&[]), Ok(()));
    }

    /// A refusal is checked the same as an approval — what matters is that an
    /// answer arrived, not which way it went.
    #[test]
    fn a_refusal_counts_as_an_answer() {
        let approval = pending(vec![call("a", true)]);
        assert_eq!(approval.check_decisions(&[decision("a", false)]), Ok(()));
    }

    /// A model told only "denied" tries the same call again, then a near
    /// variant of it. The reason is what ends that in one round.
    #[test]
    fn a_refusal_can_carry_its_reason() {
        let json = r#"{"id": "a", "approved": false, "reason": "use the existing helper"}"#;
        let parsed: ToolCallDecision = serde_json::from_str(json).expect("parses");
        assert_eq!(parsed.reason.as_deref(), Some("use the existing helper"));

        let bare: ToolCallDecision =
            serde_json::from_str(r#"{"id": "a", "approved": false}"#).expect("parses");
        assert_eq!(bare.reason, None, "a reason is optional");
    }

    /// The discriminant and the sequence number are the whole protocol: a
    /// listener orders by `seq` and dispatches on `type`.
    #[test]
    fn an_event_carries_its_type_beside_its_sequence() {
        let event = ChatTurnEvent {
            seq: 9,
            round: 2,
            target_id: Some("block-1".into()),
            event: ChatEventPayload::Delta {
                delta: "hello".into(),
            },
        };

        let json = serde_json::to_value(&event).expect("serializes");
        assert_eq!(json["seq"], 9);
        assert_eq!(json["round"], 2);
        assert_eq!(json["targetId"], "block-1");
        assert_eq!(json["type"], "delta");
        assert_eq!(json["payload"]["delta"], "hello");
    }

    /// A payload-less event still has to say what it is.
    #[test]
    fn a_round_boundary_is_stated_not_inferred() {
        let json = serde_json::to_value(ChatTurnEvent {
            seq: 1,
            round: 1,
            target_id: None,
            event: ChatEventPayload::RoundStarted,
        })
        .expect("serializes");
        assert_eq!(json["type"], "roundStarted");
    }

    #[test]
    fn a_tool_call_and_its_result_share_an_id() {
        let started = ChatEventPayload::ToolCall(ToolCallEvent {
            id: "call_1".into(),
            name: "readFile".into(),
            arguments: r#"{"path":"a"}"#.into(),
        });
        let settled = ChatEventPayload::ToolResult(ToolResultEvent {
            id: "call_1".into(),
            result: None,
            error: Some("not found: a".into()),
            changes: Vec::new(),
        });

        let (a, b) = (
            serde_json::to_value(&started).unwrap(),
            serde_json::to_value(&settled).unwrap(),
        );
        assert_eq!(a["payload"]["id"], b["payload"]["id"]);
        assert_eq!(a["type"], "toolCall");
        assert_eq!(b["type"], "toolResult");
    }

    #[test]
    fn an_outcome_says_which_of_the_three_it_is() {
        let paused = ChatStreamOutcome::PendingApproval(pending(vec![call("a", true)]));
        let json = serde_json::to_value(&paused).expect("serializes");
        assert_eq!(json["status"], "pendingApproval");
        assert_eq!(json["value"]["round"], 3);
    }

    /// A tool call in the history must survive being written down and read
    /// back, or a resumed turn sends the model a request it never made.
    #[test]
    fn history_keeps_the_calls_a_round_requested() {
        let approval = PendingApproval {
            history: vec![LlmMessage::tool_requests(vec![LlmToolCall {
                id: "call_1".into(),
                name: "readFile".into(),
                arguments: r#"{"path":"a"}"#.into(),
            }])],
            ..pending(Vec::new())
        };

        let json = serde_json::to_string(&approval).expect("serializes");
        let after: PendingApproval = serde_json::from_str(&json).expect("parses back");

        assert_eq!(after.history[0].tool_calls.len(), 1);
        assert_eq!(after.history[0].tool_calls[0].id, "call_1");
    }
}
