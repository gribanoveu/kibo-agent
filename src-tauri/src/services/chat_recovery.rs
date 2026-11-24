//! A turn that died with the app: its journal, replayed over the saved
//! record when the chat is next opened (`docs/57-turn-journal.md`, TJ-4).
//!
//! The recovery is a load, not a resume: the turn may have stopped on a
//! command with a side effect, and repeating that without a human is not
//! ours to decide. What the turn did is kept — the history to send on and
//! the transcript to show — and the record is saved at once, the journal
//! folded into it and deleted in the same transaction.

use crate::domain::chat_record::{ChatError, ChatRecord};
use crate::domain::turn_journal;
use crate::infra::chat_store;

/// The chat as it should open: its saved record, or — when a journal is
/// there and no turn of this chat is running — the record with the journal
/// replayed over it, saved back at once. `busy` is the window's own answer
/// (a running or paused turn); a live turn's journal is its own, not a
/// crash's leavings.
pub fn load(id: &str, busy: bool) -> Result<ChatRecord, ChatError> {
    let record = chat_store::load(id)?;
    if busy {
        return Ok(record);
    }
    let entries = chat_store::journal_entries(id)?;
    let Some(recovered) = turn_journal::recover(&record.messages, &record.blocks, &record.todos, record.plan.as_deref(), &entries) else {
        return Ok(record);
    };
    chat_store::save(
        id,
        &record.workspace,
        &recovered.messages,
        &recovered.blocks,
        &recovered.todos,
        recovered.plan.as_deref(),
        record.branched_from.as_deref(),
        record.git_branch.as_deref(),
    )?;
    Ok(ChatRecord {
        messages: recovered.messages,
        blocks: recovered.blocks,
        todos: recovered.todos,
        plan: recovered.plan,
        ..record
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::llm::LlmMessage;
    use crate::testing::with_app_dir;
    use serde_json::json;

    fn saved(id: &str) {
        chat_store::save(id, "/repo", &[LlmMessage::user("go")], &json!([{ "kind": "user", "id": "user:0", "text": "go" }]), &[], None, None, None).unwrap();
    }

    /// The crash's leavings: a round's history and the window's transcript.
    /// The load replays them, marks the running call interrupted, saves the
    /// record and deletes the journal — the next load is an ordinary one.
    #[test]
    fn a_journal_with_no_turn_running_is_replayed_and_folded() {
        with_app_dir("recovery-fold", || {
            saved("c1");
            let added = vec![
                LlmMessage::assistant("reading"),
                LlmMessage { role: crate::domain::llm::LlmRole::Tool, content: Some("the file".into()), tool_call_id: Some("r1".into()), tool_calls: vec![], native_content: None, images: Vec::new() },
            ];
            chat_store::journal_append("c1", "c1/turn-1", "snapshot", &json!([LlmMessage::user("go")]), 1).unwrap();
            chat_store::journal_append("c1", "c1/turn-1", "append", &serde_json::to_value(&added).unwrap(), 2).unwrap();
            chat_store::journal_append("c1", "c1/turn-1", "blocks", &json!({
                "blocks": [
                    { "kind": "user", "id": "user:0", "text": "go" },
                    { "kind": "tool", "id": "r1", "round": 1, "name": "readFile", "arguments": "{}", "status": "running", "output": "" }
                ],
                "todos": [], "plan": "# Plan"
            }), 0).unwrap();

            let record = load("c1", false).unwrap();
            assert_eq!(record.messages.len(), 3);
            assert_eq!(record.plan.as_deref(), Some("# Plan"));
            let blocks = record.blocks.as_array().unwrap();
            assert_eq!(blocks[1]["status"], "failed");
            assert_eq!(blocks.last().unwrap()["text"], "The turn was interrupted — 1 rounds kept");
            // Saved at once, journal gone: the next load is the record itself.
            assert!(chat_store::journal_entries("c1").unwrap().is_empty());
            let again = load("c1", false).unwrap();
            assert_eq!(again.messages, record.messages);
            assert_eq!(again.blocks, record.blocks);
        });
    }

    /// A chat with earlier turns: the journal carried only the crashed
    /// turn's slice of the transcript, and the recovery splices it onto the
    /// record's blocks — the earlier turns are not lost.
    #[test]
    fn earlier_turns_survive_the_recovery() {
        with_app_dir("recovery-splice", || {
            let saved_blocks = json!([
                { "kind": "user", "id": "user:0", "text": "first" },
                { "kind": "message", "id": "message:1:1", "round": 1, "text": "answered" }
            ]);
            chat_store::save("c1", "/repo", &[LlmMessage::user("first"), LlmMessage::assistant("answered")], &saved_blocks, &[], None, None, None).unwrap();
            chat_store::journal_append("c1", "c1/turn-2", "snapshot", &json!([LlmMessage::user("first"), LlmMessage::assistant("answered"), LlmMessage::user("second")]), 3).unwrap();
            chat_store::journal_append("c1", "c1/turn-2", "append", &json!([LlmMessage::assistant("working")]), 1).unwrap();
            chat_store::journal_append("c1", "c1/turn-2", "blocks", &json!({
                "blocks": [{ "kind": "user", "id": "user:2", "text": "second" }],
                "todos": [], "plan": null, "from": 2
            }), 0).unwrap();

            let record = load("c1", false).unwrap();
            let blocks = record.blocks.as_array().unwrap();
            assert_eq!(blocks.len(), 4, "two saved, one of the turn, one notice");
            assert_eq!(blocks[0]["text"], "first");
            assert_eq!(blocks[1]["text"], "answered");
            assert_eq!(blocks[2]["text"], "second");
            assert_eq!(blocks[3]["kind"], "notice");
            assert_eq!(record.messages.len(), 4);
        });
    }

    /// A live turn's journal is its own: the load leaves it alone, and the
    /// record opens as saved.
    #[test]
    fn a_running_turns_journal_is_not_recovered() {
        with_app_dir("recovery-busy", || {
            saved("c1");
            chat_store::journal_append("c1", "c1/turn-1", "append", &json!([LlmMessage::assistant("working")]), 1).unwrap();

            let record = load("c1", true).unwrap();
            assert_eq!(record.messages, vec![LlmMessage::user("go")]);
            assert_eq!(chat_store::journal_entries("c1").unwrap().len(), 1, "the journal stays");
        });
    }

    /// No journal: the record stands, untouched.
    #[test]
    fn a_chat_without_a_journal_opens_as_saved() {
        with_app_dir("recovery-none", || {
            saved("c1");
            let record = load("c1", false).unwrap();
            assert_eq!(record.messages, vec![LlmMessage::user("go")]);
        });
    }
}
