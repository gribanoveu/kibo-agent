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
use crate::infra::{chat_store, settings_store};

/// The chat as it should open: its saved record, or — when a journal is
/// there and no turn of this chat is running — the record with the journal
/// replayed over it, saved back at once. `busy` is the window's own answer
/// (a running or paused turn); a live turn's journal is its own, not a
/// crash's leavings.
pub fn load(id: &str, busy: bool) -> Result<ChatRecord, ChatError> {
    let mut record = chat_store::load(id)?;
    if !busy {
        let entries = chat_store::journal_entries(id)?;
        if let Some(recovered) = turn_journal::recover(&record.messages, &record.blocks, &record.todos, record.plan.as_deref(), &entries) {
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
            record = ChatRecord {
                messages: recovered.messages,
                blocks: recovered.blocks,
                todos: recovered.todos,
                plan: recovered.plan,
                ..record
            };
        }
    }
    // The pin the chat opens with is the one its turns resolve: a removed
    // provider is substituted here too, once and written, so the chip and
    // the turn say the same thing. Settings that cannot be read leave the
    // stored pin — the turn reports the settings error itself.
    if let Ok(settings) = settings_store::load() {
        record.model = crate::services::chat_model::pin(id, &settings.llm)?;
    }
    Ok(record)
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

    /// One live provider, so a pin naming it stands on opening.
    fn configured(id: &str, model: &str) {
        crate::infra::settings_store::save(&crate::domain::settings::AppSettings {
            llm: crate::domain::settings::LlmSettings {
                providers: vec![crate::domain::settings::ProviderConfig {
                    id: id.into(),
                    base_url: format!("https://{id}.example/v1"),
                    model: Some(model.into()),
                    ..Default::default()
                }],
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
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

    /// CM-1: the pin is the row's — a recovery saves the conversation whole,
    /// and the chat still speaks with its own provider afterwards, in the
    /// record the window is told and in the store.
    #[test]
    fn a_recovered_chat_keeps_its_model() {
        with_app_dir("recovery-model", || {
            saved("c1");
            configured("p", "m");
            let pin = crate::domain::chat_record::ChatModel { provider: "p".into(), model: Some("m".into()) };
            chat_store::set_model("c1", &pin).unwrap();
            chat_store::journal_append("c1", "c1/turn-1", "append", &json!([LlmMessage::assistant("working")]), 1).unwrap();

            let record = load("c1", false).unwrap();

            assert_eq!(record.messages.len(), 2, "the journal was replayed");
            assert_eq!(record.model, Some(pin.clone()));
            assert_eq!(chat_store::model("c1").unwrap(), Some(pin));
        });
    }

    /// CM-2: opening a chat whose provider was removed substitutes the active
    /// one — written into the chat, so the chip and the next turn agree.
    #[test]
    fn opening_a_chat_substitutes_a_removed_provider() {
        with_app_dir("recovery-model-substitute", || {
            saved("c1");
            configured("local", "qwen");
            chat_store::set_model("c1", &crate::domain::chat_record::ChatModel { provider: "gone".into(), model: Some("m".into()) }).unwrap();

            let record = load("c1", false).unwrap();

            let expected = crate::domain::chat_record::ChatModel { provider: "local".into(), model: Some("qwen".into()) };
            assert_eq!(record.model, Some(expected.clone()));
            assert_eq!(chat_store::model("c1").unwrap(), Some(expected), "written, not computed each open");
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
