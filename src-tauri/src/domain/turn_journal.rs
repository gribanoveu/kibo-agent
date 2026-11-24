//! The turn's write-ahead journal: what a running turn puts on disk after
//! each completed round, so a crash or a closed window does not take the
//! turn's work with it (`docs/57-turn-journal.md`, TJ-4).
//!
//! Two halves, two writers. The loop journals the model's history — the
//! messages exactly as they were sent, reasoning blocks included, so the
//! next request after a recovery is the previous request's continuation
//! and the provider's cache can still hit. The window journals the
//! transcript it draws (blocks, checklist, plan), which the loop knows
//! nothing about. A normal save folds the journal into the chat's record
//! and deletes it in the same transaction; a load that finds a journal
//! with no turn running replays it instead.
//!
//! The turn itself is never continued automatically: it may have stopped
//! on a command with a side effect, and repeating that without a human is
//! not ours to decide.

use serde_json::Value;

use crate::domain::llm::LlmMessage;
use crate::domain::tools::Task;

/// Where a running turn's history goes. The adapter knows the chat, the
/// turn and the store; the loop only says what changed.
pub trait TurnJournal: Send + Sync {
    /// How many messages of this turn's history are already journaled —
    /// the tail to append is counted from here, and a resume picks it up
    /// across the pause.
    fn len(&self) -> usize;
    /// The messages the last completed round added, in order.
    fn append(&self, messages: &[LlmMessage]);
    /// The whole history, rewritten: a fold or a cleared result changed it
    /// in place, so a tail alone would replay wrong.
    fn snapshot(&self, history: &[LlmMessage]);
}

/// One journal row, parsed. A row that does not parse is skipped by the
/// store — a damaged line costs what it carried, not the recovery.
#[derive(Debug, Clone, PartialEq)]
pub enum JournalEntry {
    Append(Vec<LlmMessage>),
    Snapshot(Vec<LlmMessage>),
    /// The turn's own blocks, and where they join the saved transcript:
    /// `from` is the count of blocks before this turn. The window journals
    /// only its turn — the record's earlier turns are spliced back in front.
    Blocks { blocks: Value, todos: Vec<Task>, plan: Option<String>, from: usize },
}

impl JournalEntry {
    pub fn parse(kind: &str, body: &Value) -> Option<Self> {
        match kind {
            "append" => serde_json::from_value(body.clone()).ok().map(Self::Append),
            "snapshot" => serde_json::from_value(body.clone()).ok().map(Self::Snapshot),
            "blocks" => Some(Self::Blocks {
                blocks: body.get("blocks")?.clone(),
                todos: serde_json::from_value(body.get("todos")?.clone()).ok()?,
                plan: body.get("plan").and_then(Value::as_str).map(str::to_string),
                from: body.get("from").and_then(Value::as_u64).unwrap_or(0) as usize,
            }),
            _ => None,
        }
    }
}

/// What a crashed turn leaves behind: the history to send on, the
/// transcript to show, and the lists as the turn reached them.
#[derive(Debug, Clone, PartialEq)]
pub struct Recovered {
    pub messages: Vec<LlmMessage>,
    pub blocks: Value,
    pub todos: Vec<Task>,
    pub plan: Option<String>,
}

/// Said in the transcript of a recovered turn. The count is the completed
/// rounds the journal kept — the base snapshot is not one.
pub const INTERRUPTED: &str = "The turn was interrupted";

fn blocks_array(value: &Value) -> Vec<Value> {
    value.as_array().cloned().unwrap_or_default()
}

/// Replays the journal over the saved record, in the order it was written:
/// an append extends the history, a snapshot replaces it, a blocks row
/// replaces the transcript and the lists. `None` when nothing was
/// journaled — the record stands as it is.
///
/// Calls left running are marked interrupted, in the transcript only: a
/// note in the history would break the prefix the provider may still have
/// cached, and the model is told by the user's next message, not by us.
pub fn recover(
    messages: &[LlmMessage],
    blocks: &Value,
    todos: &[Task],
    plan: Option<&str>,
    entries: &[JournalEntry],
) -> Option<Recovered> {
    if entries.is_empty() {
        return None;
    }
    let mut history = messages.to_vec();
    let base_blocks = blocks_array(blocks);
    let mut shown = blocks.clone();
    let mut tasks = todos.to_vec();
    let mut written = plan.map(str::to_string);
    let mut rounds = 0usize;
    for entry in entries {
        match entry {
            JournalEntry::Append(added) => {
                history.extend(added.iter().cloned());
                rounds += 1;
            }
            JournalEntry::Snapshot(whole) => history = whole.clone(),
            JournalEntry::Blocks { blocks, todos, plan, from } => {
                // The turn's blocks join the saved transcript where the turn
                // began: the earlier turns are the record's, not lost to a
                // journal that only ever carried this turn's slice.
                let mut items: Vec<Value> = base_blocks.iter().take(*from).cloned().collect();
                items.extend(blocks_array(blocks));
                shown = Value::Array(items);
                tasks = todos.clone();
                written = plan.clone();
            }
        }
    }
    if let Some(items) = shown.as_array_mut() {
        for block in items.iter_mut() {
            let running_tool = block.get("kind").and_then(Value::as_str) == Some("tool")
                && block.get("status").and_then(Value::as_str) == Some("running");
            if running_tool {
                block["status"] = Value::String("failed".into());
                block["error"] = Value::String("Interrupted — the app closed before this call finished.".into());
            }
        }
        items.push(serde_json::json!({
            "kind": "notice",
            "id": format!("notice:{}", items.len()),
            "text": format!("{INTERRUPTED} — {rounds} rounds kept"),
        }));
    }
    Some(Recovered { messages: history, blocks: shown, todos: tasks, plan: written })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::llm::LlmRole;

    fn base() -> Vec<LlmMessage> {
        vec![LlmMessage::user("go")]
    }

    fn round_trip() -> Vec<LlmMessage> {
        vec![
            LlmMessage {
                role: LlmRole::Assistant,
                content: None,
                tool_call_id: None,
                tool_calls: vec![crate::domain::llm::LlmToolCall { id: "c1".into(), name: "readFile".into(), arguments: "{}".into() }],
                native_content: None,
                images: Vec::new(),
            },
            LlmMessage {
                role: LlmRole::Tool,
                content: Some("the file".into()),
                tool_call_id: Some("c1".into()),
                tool_calls: vec![],
                native_content: None,
                images: Vec::new(),
            },
        ]
    }

    fn blocks_json() -> Value {
        serde_json::json!([
            { "kind": "user", "id": "user:0", "text": "go" },
            { "kind": "tool", "id": "c1", "round": 1, "name": "readFile", "arguments": "{}", "status": "running", "output": "" }
        ])
    }

    /// The replay: appends extend, a snapshot replaces, the last blocks
    /// row wins — and the history is the wire form, reasoning included.
    #[test]
    fn the_journal_replays_over_the_saved_record() {
        let mut answered = round_trip();
        answered[0].native_content = Some(serde_json::json!({"thinking": "hm"}));
        let entries = vec![
            JournalEntry::Snapshot(base()),
            JournalEntry::Append(answered.clone()),
            JournalEntry::Blocks { blocks: blocks_json(), todos: vec![], plan: Some("# Plan".into()), from: 0 },
            JournalEntry::Append(vec![LlmMessage::assistant("done")]),
        ];
        let recovered = recover(&[], &Value::Null, &[], None, &entries).expect("something was journaled");
        let mut whole = base();
        whole.extend(answered);
        whole.push(LlmMessage::assistant("done"));
        assert_eq!(recovered.messages, whole);
        assert_eq!(recovered.plan.as_deref(), Some("# Plan"));
        // The call left running is interrupted in the transcript, and the
        // notice says how many rounds were kept — two appends, not the base.
        let items = recovered.blocks.as_array().unwrap();
        assert_eq!(items[1]["status"], "failed");
        assert!(items[1]["error"].as_str().unwrap().contains("Interrupted"));
        assert_eq!(items.last().unwrap()["kind"], "notice");
        assert_eq!(items.last().unwrap()["text"], "The turn was interrupted — 2 rounds kept");
        // The history carries no interruption note: the prefix stays whole.
        assert!(recovered.messages.iter().all(|m| m.content.as_deref() != Some("Interrupted — the app closed before this call finished.")));
    }

    /// A fold rewrote the history: the snapshot is the base from then on,
    /// not the saved record and not the appends before it.
    #[test]
    fn a_snapshot_replaces_everything_before_it() {
        let folded = vec![LlmMessage::user("[summary] earlier"), LlmMessage::user("and now?")];
        let entries = vec![
            JournalEntry::Snapshot(base()),
            JournalEntry::Append(round_trip()),
            JournalEntry::Snapshot(folded.clone()),
            JournalEntry::Append(vec![LlmMessage::assistant("done")]),
        ];
        let recovered = recover(&base(), &Value::Null, &[], None, &entries).unwrap();
        let mut whole = folded;
        whole.push(LlmMessage::assistant("done"));
        assert_eq!(recovered.messages, whole);
    }

    /// Nothing journaled: the record stands, and no notice is invented.
    #[test]
    fn an_empty_journal_recovers_nothing() {
        assert_eq!(recover(&base(), &blocks_json(), &[], None, &[]), None);
    }

    /// The transcript and lists come from the last blocks row; without
    /// one, the saved record's stand.
    #[test]
    fn the_lists_come_from_the_last_blocks_row() {
        let task = Task { id: "t1".into(), title: "read".into(), status: crate::domain::tools::TodoStatus::Completed, note: None };
        let entries = vec![
            JournalEntry::Append(round_trip()),
            JournalEntry::Blocks { blocks: blocks_json(), todos: vec![task.clone()], plan: None, from: 0 },
        ];
        let recovered = recover(&base(), &Value::Null, &[], Some("# Old"), &entries).unwrap();
        assert_eq!(recovered.todos, vec![task]);
        assert_eq!(recovered.plan, None, "the turn wrote no plan: the row's null stands");

        let no_blocks = recover(&base(), &blocks_json(), &[], Some("# Old"), &[JournalEntry::Append(round_trip())]).unwrap();
        assert_eq!(no_blocks.plan.as_deref(), Some("# Old"));
        // The saved record's blocks stand — with the same interruption
        // marking: the call they show running never finished.
        let items = no_blocks.blocks.as_array().unwrap();
        assert_eq!(items[0]["kind"], "user");
        assert_eq!(items[1]["status"], "failed");
        assert_eq!(items.last().unwrap()["kind"], "notice");
    }

    /// The crash in a chat with earlier turns: the journal carried only
    /// this turn's slice, and the recovery splices it onto the record's
    /// transcript at `from` — the earlier turns are not lost.
    #[test]
    fn the_turns_blocks_join_the_saved_transcript_where_it_began() {
        let saved = serde_json::json!([
            { "kind": "user", "id": "user:0", "text": "first" },
            { "kind": "message", "id": "message:1:1", "round": 1, "text": "answered" }
        ]);
        let turn = serde_json::json!([
            { "kind": "user", "id": "user:2", "text": "second" },
            { "kind": "tool", "id": "c9", "round": 1, "name": "readFile", "arguments": "{}", "status": "running", "output": "" }
        ]);
        let entries = vec![
            JournalEntry::Snapshot(base()),
            JournalEntry::Append(round_trip()),
            JournalEntry::Blocks { blocks: turn, todos: vec![], plan: None, from: 2 },
        ];
        let recovered = recover(&base(), &saved, &[], None, &entries).unwrap();
        let items = recovered.blocks.as_array().unwrap();
        assert_eq!(items.len(), 5, "two saved blocks, two of the turn, one notice");
        assert_eq!(items[0]["text"], "first");
        assert_eq!(items[1]["text"], "answered");
        assert_eq!(items[2]["text"], "second");
        assert_eq!(items[3]["status"], "failed");
        assert_eq!(items[4]["kind"], "notice");
    }

    /// A row's kind decides how its body reads; a body that does not fit
    /// its kind is skipped, not panicked on.
    #[test]
    fn a_row_parses_by_its_kind_and_a_bad_one_is_skipped() {
        let messages = serde_json::to_value(round_trip()).unwrap();
        assert!(matches!(JournalEntry::parse("append", &messages), Some(JournalEntry::Append(_))));
        assert!(matches!(JournalEntry::parse("snapshot", &messages), Some(JournalEntry::Snapshot(_))));
        let blocks = serde_json::json!({ "blocks": blocks_json(), "todos": [], "plan": null, "from": 2 });
        assert!(matches!(JournalEntry::parse("blocks", &blocks), Some(JournalEntry::Blocks { from: 2, .. })));
        // A row without `from` joins at the start — the whole transcript is the turn's.
        let no_from = serde_json::json!({ "blocks": blocks_json(), "todos": [], "plan": null });
        assert!(matches!(JournalEntry::parse("blocks", &no_from), Some(JournalEntry::Blocks { from: 0, .. })));
        assert_eq!(JournalEntry::parse("append", &serde_json::json!({"not": "a list"})), None);
        assert_eq!(JournalEntry::parse("tomorrow", &messages), None);
    }
}
