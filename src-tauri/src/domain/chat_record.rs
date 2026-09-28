//! What a saved conversation looks like on disk, and nothing else.
//!
//! Its own file because it is a contract with two other parties: the frontend,
//! which reads these fields back to redraw a transcript, and every future
//! build of this app, which has to read what this one wrote. Changing a field
//! here is a data migration; changing it inside the store would look like an
//! edit to a struct.
//!
//! Two lists are kept, not one. `messages` is what the model sees and `blocks`
//! is what the reader sees, and they are not derivable from each other: one
//! tool call is two messages and one block, and a collapsed detail is a block
//! with no message at all. Reconstructing either from the other would lose
//! whichever half was guessed.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use super::chat_role::ChatRole;
use super::compaction::SUMMARY_PREFIX;
use super::llm::{LlmMessage, LlmRole};
use super::tools::Task;

/// Bumped when a field this build reads stops meaning what it meant. Adding a
/// field with `#[serde(default)]` is not that: an older record still loads.
pub const CHAT_SCHEMA_VERSION: u32 = 1;

/// Where Chat mode's conversations are filed: they are about no folder, and
/// no open folder's listing reaches them — a folder's path is never empty.
pub const NO_FOLDER: &str = "";

/// Longest title derived from a first message. Past this the sidebar elides it
/// anyway, and the rest is only weight in every listing.
const TITLE_CHARS: usize = 60;

#[derive(Debug, Error)]
pub enum ChatError {
    #[error("app directory: {0}")]
    AppDir(String),
    #[error("chat store: {0}")]
    Store(String),
    #[error("could not write the chat: {0}")]
    Write(String),
    #[error("this chat is not readable: {0}")]
    Parse(serde_json::Error),
    #[error("this chat was saved by a newer version of the app (format {0})")]
    UnsupportedVersion(u32),
    #[error("not a chat id: {0}")]
    BadId(String),
    #[error("no such chat: {0}")]
    NotFound(String),
}

/// One conversation, whole.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatRecord {
    pub schema_version: u32,
    pub id: String,
    /// The folder the conversation was about. The sidebar lists by it: a
    /// chat about another project is not this project's history.
    pub workspace: String,
    pub title: String,
    pub created_at: i64,
    pub updated_at: i64,
    /// What the model sees.
    pub messages: Vec<LlmMessage>,
    /// What the reader sees — opaque here on purpose. Block shapes belong to
    /// the frontend and change more often than this file; parsing them here
    /// would mean a Rust change every time a card gains a field, and a load
    /// that fails over a field Rust never uses.
    pub blocks: Value,
    pub todos: Vec<Task>,
    /// The plan document, as last written or edited. Absent in chats saved
    /// before plans existed, and in chats that never had one.
    #[serde(default)]
    pub plan: Option<String>,
    /// The chat this one was branched from, if it was. A branch starts as a
    /// copy of the earlier part of that chat, so it shares its title — this
    /// is what tells the two apart in the sidebar. Nothing is kept in step:
    /// the original may since have been deleted.
    #[serde(default)]
    pub branched_from: Option<String>,
    /// Who the model was in a Chat mode conversation; `None` for the agent's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<ChatRole>,
}

/// A row in the sidebar. Deliberately not the whole record: listing a folder
/// of long conversations to draw ten titles is the one operation that has to
/// stay cheap.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatSummary {
    pub id: String,
    pub title: String,
    pub updated_at: i64,
    pub branched_from: Option<String>,
    /// Filed away from the list. Kept by the store beside the record rather
    /// than in it, so a record alone says `false`.
    #[serde(default)]
    pub archived: bool,
}

impl From<&ChatRecord> for ChatSummary {
    fn from(record: &ChatRecord) -> Self {
        Self {
            id: record.id.clone(),
            title: record.title.clone(),
            updated_at: record.updated_at,
            branched_from: record.branched_from.clone(),
            archived: false,
        }
    }
}

/// Reads one stored chat, reporting a version it does not understand as
/// exactly that.
///
/// The version is read before the record, because a newer format is not a
/// syntax error and must not be reported as one: the difference decides
/// whether the file is worth keeping untouched or worth telling the user
/// about. Unknown *fields* are ignored — that is serde's default and the
/// reason adding one does not need a new version.
pub fn parse(text: &str) -> Result<ChatRecord, ChatError> {
    let value: Value = serde_json::from_str(text).map_err(ChatError::Parse)?;
    let version = value
        .get("schemaVersion")
        .and_then(Value::as_u64)
        .unwrap_or(0) as u32;
    if version > CHAT_SCHEMA_VERSION {
        return Err(ChatError::UnsupportedVersion(version));
    }
    serde_json::from_value(value).map_err(ChatError::Parse)
}

/// A name for a conversation, taken from the first thing the user said.
///
/// Asking the model for one costs a request per chat and can fail; the first
/// line of the first question is what the user would have typed anyway.
///
/// Read from the transcript, which keeps every message. The model's copy is
/// only the fallback, past its summary: once a chat is compacted its first user
/// message is the summary, and every compacted chat had the summary's name.
pub fn derive_title(messages: &[LlmMessage], blocks: &Value) -> String {
    let bubble = blocks
        .as_array()
        .into_iter()
        .flatten()
        .find(|block| block.get("kind").and_then(Value::as_str) == Some("user"))
        .and_then(|block| block.get("text"))
        .and_then(Value::as_str);
    let first = bubble
        .or_else(|| {
            messages
                .iter()
                .filter(|message| message.role == LlmRole::User)
                .filter_map(|message| message.content.as_deref())
                .find(|text| !text.starts_with(SUMMARY_PREFIX))
        })
        .unwrap_or("");
    let line = first.lines().map(str::trim).find(|line| !line.is_empty());

    match line {
        None => "New chat".to_string(),
        Some(line) if line.chars().count() <= TITLE_CHARS => line.to_string(),
        Some(line) => {
            let cut: String = line.chars().take(TITLE_CHARS).collect();
            // Cut on a word where there is one nearby, so the title does not
            // end mid-word for the sake of four characters.
            let cut = match cut.rsplit_once(' ') {
                Some((head, _)) if head.chars().count() >= TITLE_CHARS / 2 => head.to_string(),
                _ => cut,
            };
            format!("{}…", cut.trim_end())
        }
    }
}

/// Ids come back from the window and become the store's key. Anything
/// that is not a plain identifier is refused rather than sanitised: a chat id
/// is generated by the app, so an id with a slash in it is not a chat.
pub fn check_id(id: &str) -> Result<(), ChatError> {
    let ok = !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(())
    } else {
        Err(ChatError::BadId(id.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> ChatRecord {
        ChatRecord {
            schema_version: CHAT_SCHEMA_VERSION,
            id: "abc".to_string(),
            workspace: "/repo".to_string(),
            title: "Fix the parser".to_string(),
            created_at: 10,
            updated_at: 20,
            messages: vec![LlmMessage::user("fix the parser")],
            blocks: serde_json::json!([{ "kind": "user", "id": "user:0", "text": "fix the parser" }]),
            todos: Vec::new(),
            plan: None,
            branched_from: None,
            role: None,
        }
    }

    /// The whole point of the file: these names are what an older build will
    /// look for. A rename that only this build knows about is a chat that
    /// silently loses its transcript.
    #[test]
    fn the_stored_shape_is_the_documented_one() {
        let value = serde_json::to_value(record()).unwrap();
        let object = value.as_object().unwrap();

        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "blocks",
                "branchedFrom",
                "createdAt",
                "id",
                "messages",
                "plan",
                "schemaVersion",
                "title",
                "todos",
                "updatedAt",
                "workspace",
            ]
        );
        assert_eq!(object["schemaVersion"], 1);
    }

    /// Chats saved before plans existed have no `plan` key, and must still
    /// open — as chats without a plan.
    #[test]
    fn a_chat_saved_before_plans_opens_without_one() {
        let mut value = serde_json::to_value(record()).unwrap();
        value.as_object_mut().unwrap().remove("plan");
        let loaded: ChatRecord = serde_json::from_value(value).unwrap();
        assert_eq!(loaded.plan, None);
    }

    /// Every chat saved before branching existed is a chat that was not
    /// branched from anything.
    #[test]
    fn a_chat_saved_before_branches_is_not_a_branch() {
        let mut value = serde_json::to_value(record()).unwrap();
        value.as_object_mut().unwrap().remove("branchedFrom");
        let loaded: ChatRecord = serde_json::from_value(value).unwrap();
        assert_eq!(loaded.branched_from, None);
    }

    #[test]
    fn a_record_round_trips() {
        let text = serde_json::to_string(&record()).unwrap();
        assert_eq!(parse(&text).unwrap(), record());
    }

    /// A field this build has never heard of is the normal way the format
    /// grows. Refusing it would make every older build unable to open a chat
    /// a newer one merely annotated.
    #[test]
    fn an_unknown_field_is_ignored() {
        let mut value = serde_json::to_value(record()).unwrap();
        value["somethingNewer"] = serde_json::json!(true);

        assert_eq!(parse(&value.to_string()).unwrap(), record());
    }

    /// Told apart from damage on purpose: the store leaves a newer chat alone,
    /// and it can only do that if it knows which of the two this is.
    #[test]
    fn a_newer_format_is_reported_as_a_version_not_as_damage() {
        let mut value = serde_json::to_value(record()).unwrap();
        value["schemaVersion"] = serde_json::json!(CHAT_SCHEMA_VERSION + 1);

        match parse(&value.to_string()) {
            Err(ChatError::UnsupportedVersion(v)) => assert_eq!(v, CHAT_SCHEMA_VERSION + 1),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn damage_is_reported_as_damage() {
        assert!(matches!(parse("{ not json"), Err(ChatError::Parse(_))));
    }

    #[test]
    fn the_title_is_the_first_thing_the_user_said() {
        let messages = vec![
            LlmMessage::system("you are an agent"),
            LlmMessage::user("why does the parser drop the last token?"),
            LlmMessage::assistant("because…"),
        ];
        assert_eq!(
            derive_title(&messages, &Value::Null),
            "why does the parser drop the last token?"
        );
    }

    fn bubble(text: &str) -> Value {
        serde_json::json!([{ "kind": "notice", "id": "n", "text": "not this" }, { "kind": "user", "id": "user:1", "text": text }])
    }

    /// Compacted, the model's first user message is the summary. Every chat
    /// compacted was once named after it, and no two could be told apart.
    #[test]
    fn a_compacted_chat_keeps_the_name_of_its_first_question() {
        let messages = vec![
            LlmMessage::user(format!("{SUMMARY_PREFIX}\n\nearlier, the user asked about tokens")),
            LlmMessage::user("and now the lexer?"),
        ];
        assert_eq!(derive_title(&messages, &bubble("why does it drop the token?")), "why does it drop the token?");
        // With no transcript to read, the first message that is not the summary.
        assert_eq!(derive_title(&messages, &Value::Null), "and now the lexer?");
    }

    /// A chat begun with `/init` is named after the command, not the page of
    /// prompt it sent — the model's copy has the prompt, the bubble the command.
    #[test]
    fn a_chat_begun_with_a_command_is_named_after_the_command() {
        let blocks = serde_json::json!([{ "kind": "user", "id": "user:0", "text": "/init the IPC layer", "sent": "Your task is to study this repository" }]);
        let messages = [LlmMessage::user("Your task is to study this repository")];
        assert_eq!(derive_title(&messages, &blocks), "/init the IPC layer");
    }

    #[test]
    fn a_long_title_is_cut_on_a_word() {
        let long = "please look at the tokenizer and explain why the parser drops the final token";
        let title = derive_title(&[], &bubble(long));

        assert!(title.ends_with('…'), "{title}");
        assert!(title.chars().count() <= TITLE_CHARS + 1, "{title}");
        assert!(!title.contains("  "), "{title}");
        assert!(long.starts_with(title.trim_end_matches('…')), "{title}");
    }

    #[test]
    fn a_chat_with_nothing_said_still_has_a_name() {
        assert_eq!(derive_title(&[], &Value::Null), "New chat");
        assert_eq!(derive_title(&[LlmMessage::user("   ")], &Value::Null), "New chat");
    }

    /// The id is the store's key, and it arrives from the window.
    #[test]
    fn an_id_that_is_not_an_id_is_refused() {
        check_id("0f8c-4a11").unwrap();
        for bad in ["", "../settings", "a/b", "a.json", &"x".repeat(65)] {
            assert!(check_id(bad).is_err(), "accepted {bad:?}");
        }
    }
}
