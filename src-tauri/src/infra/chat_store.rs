//! Saved conversations: one row per chat in `<app dir>/chats.db`.
//!
//! The listing columns (`workspace`, `title`, `updated_at`, `branched_from`)
//! are real columns, so the sidebar is one indexed query and never parses a
//! transcript. The record itself is stored whole as JSON in `body`, in the
//! `domain::chat_record` format — that file stays the contract.
//!
//! `archived` is the one column not taken from the body: it is where the chat
//! is filed, not what was said, so saving a turn leaves it as it was.
//!
//! No database-wide version and no migrations. Upstream once refused to open
//! a database written by a newer build and thereby hid a user's entire
//! history behind one number. Here the version is per row: a chat this build
//! cannot read is one row missing from the list, left untouched, and the rest
//! open normally. A connection per call, as in `tool_call_log`: one write per
//! turn is not a hot path.

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;

use crate::domain::chat_export;
use crate::domain::compaction::SUMMARY_PREFIX;
use crate::domain::chat_record::{
    self, ChatError, ChatRecord, ChatSummary, CHAT_SCHEMA_VERSION, NO_FOLDER,
};
use crate::domain::chat_role::ChatRole;
use crate::domain::kube::KubePin;
use crate::domain::llm::LlmMessage;
use crate::domain::tools::Task;
use crate::infra::app_dir;

const FILE: &str = "chats.db";

const SCHEMA: &str = "
PRAGMA journal_mode = WAL;
PRAGMA busy_timeout = 3000;
CREATE TABLE IF NOT EXISTS chats (
  id             TEXT PRIMARY KEY,
  workspace      TEXT NOT NULL,
  schema_version INTEGER NOT NULL,
  title          TEXT NOT NULL,
  created_at     INTEGER NOT NULL,
  updated_at     INTEGER NOT NULL,
  branched_from  TEXT,
  body           TEXT NOT NULL,
  archived       INTEGER NOT NULL DEFAULT 0,
  role           TEXT
);
CREATE INDEX IF NOT EXISTS chats_workspace ON chats(workspace, updated_at DESC);
CREATE TABLE IF NOT EXISTS suggestions (
  id          TEXT PRIMARY KEY,
  chat_id     TEXT NOT NULL,
  created_at  INTEGER NOT NULL,
  model       TEXT NOT NULL,
  input       TEXT NOT NULL,
  suggestion  TEXT NOT NULL,
  shown       INTEGER NOT NULL,
  used        INTEGER NOT NULL DEFAULT 0,
  sent        TEXT
);
";

pub(crate) fn store(e: rusqlite::Error) -> ChatError {
    ChatError::Store(e.to_string())
}

pub(crate) fn open() -> Result<Connection, ChatError> {
    let path = app_dir::ensure().map_err(ChatError::AppDir)?.join(FILE);
    let conn = Connection::open(&path).map_err(store)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    conn.execute_batch(SCHEMA).map_err(store)?;
    // A database from before archiving has the table without the column.
    // Adding one with a default is all it takes; an older build reading the
    // file afterwards does not see it and is not harmed by it.
    let has_archived: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('chats') WHERE name = 'archived'",
            [],
            |row| row.get(0),
        )
        .map_err(store)?;
    if !has_archived {
        conn.execute_batch("ALTER TABLE chats ADD COLUMN archived INTEGER NOT NULL DEFAULT 0")
            .map_err(store)?;
    }
    // The same for a chat's role, which the list draws a sign from: null for
    // the agent's chats and for those saved before the column.
    let has_role: bool = conn
        .query_row("SELECT COUNT(*) > 0 FROM pragma_table_info('chats') WHERE name = 'role'", [], |row| row.get(0))
        .map_err(store)?;
    if !has_role {
        conn.execute_batch("ALTER TABLE chats ADD COLUMN role TEXT").map_err(store)?;
    }
    Ok(conn)
}

pub(crate) fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Chats belonging to one workspace, most recently updated first — the order
/// the sidebar draws them in. Ties broken by id so the order is stable: two
/// chats saved in the same millisecond otherwise swap places between listings.
/// Rows from a newer build are not listed. Archived chats are listed too,
/// marked: which of them to show is the sidebar's filter.
pub fn list(workspace: &str) -> Result<Vec<ChatSummary>, ChatError> {
    let conn = open()?;
    rename_summary_titles(&conn, workspace)?;
    let mut stmt = conn
        .prepare(
            "SELECT id, title, updated_at, branched_from, archived, role FROM chats
             WHERE workspace = ?1 AND schema_version <= ?2
             ORDER BY updated_at DESC, id",
        )
        .map_err(store)?;
    let rows = stmt
        .query_map(params![workspace, CHAT_SCHEMA_VERSION], |row| {
            Ok(ChatSummary {
                id: row.get(0)?,
                title: row.get(1)?,
                updated_at: row.get(2)?,
                branched_from: row.get(3)?,
                archived: row.get(4)?,
                // A role a newer build wrote is no sign, not an error.
                role: row.get::<_, Option<String>>(5)?.and_then(|role| serde_json::from_value(serde_json::Value::String(role)).ok()),
            })
        })
        .map_err(store)?;
    rows.collect::<Result<_, _>>().map_err(store)
}

/// Chats an older build named after their compaction summary get their name
/// back from their own transcript. Only those rows are read, and none match
/// once they have been renamed. A body that does not parse keeps the name it
/// has.
fn rename_summary_titles(conn: &Connection, workspace: &str) -> Result<(), ChatError> {
    let mut stmt = conn
        .prepare("SELECT id, body FROM chats WHERE workspace = ?1 AND instr(title, ?2) = 1")
        .map_err(store)?;
    let stale = stmt
        .query_map(params![workspace, SUMMARY_PREFIX], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(store)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(store)?;
    for (id, body) in stale {
        let Ok(record) = chat_record::parse(&body) else { continue };
        let title = chat_record::derive_title(&record.messages, &record.blocks);
        conn.execute("UPDATE chats SET title = ?1 WHERE id = ?2", params![title, id])
            .map_err(store)?;
    }
    Ok(())
}

pub fn load(id: &str) -> Result<ChatRecord, ChatError> {
    chat_record::check_id(id)?;
    let body: Option<String> = open()?
        .query_row("SELECT body FROM chats WHERE id = ?1", params![id], |row| row.get(0))
        .optional()
        .map_err(store)?;
    chat_record::parse(&body.ok_or_else(|| ChatError::NotFound(id.to_string()))?)
}

/// Whether a chat with this id is stored, readable or not.
pub fn exists(id: &str) -> Result<bool, ChatError> {
    chat_record::check_id(id)?;
    open()?
        .query_row("SELECT 1 FROM chats WHERE id = ?1", params![id], |_| Ok(()))
        .optional()
        .map(|row| row.is_some())
        .map_err(store)
}

/// Writes the conversation whole, keeping the moment it started.
///
/// Whole because a conversation is edited from both ends — a compaction
/// rewrites the middle, a retry replaces the tail — so an append log would
/// need its own rules for what supersedes what. At one write per turn the
/// simpler thing is also fast enough.
pub fn save(
    id: &str,
    workspace: &str,
    messages: &[LlmMessage],
    blocks: &Value,
    todos: &[Task],
    plan: Option<&str>,
    branched_from: Option<&str>,
) -> Result<ChatSummary, ChatError> {
    upsert(ChatRecord {
        schema_version: CHAT_SCHEMA_VERSION,
        id: id.to_string(),
        workspace: workspace.to_string(),
        title: chat_record::derive_title(messages, blocks),
        created_at: 0,
        updated_at: 0,
        messages: messages.to_vec(),
        blocks: blocks.clone(),
        todos: todos.to_vec(),
        plan: plan.map(str::to_string),
        branched_from: branched_from.map(str::to_string),
        role: None,
        kube: None,
    })
}

/// A Chat mode conversation, filed under no folder. `blocks` is its
/// transcript as the window draws it — the model's thinking with each answer,
/// which `messages`, what the model is sent again, leaves out.
pub fn save_plain(
    id: &str,
    role: ChatRole,
    kube: Option<KubePin>,
    messages: &[LlmMessage],
    blocks: &Value,
) -> Result<ChatSummary, ChatError> {
    upsert(ChatRecord {
        schema_version: CHAT_SCHEMA_VERSION,
        id: id.to_string(),
        workspace: NO_FOLDER.to_string(),
        title: chat_record::derive_title(messages, &Value::Null),
        created_at: 0,
        updated_at: 0,
        messages: messages.to_vec(),
        blocks: blocks.clone(),
        todos: Vec::new(),
        plan: None,
        branched_from: None,
        role: Some(role),
        kube,
    })
}

/// Writes `record`, stamped now and keeping the moment it was first saved.
fn upsert(mut record: ChatRecord) -> Result<ChatSummary, ChatError> {
    chat_record::check_id(&record.id)?;
    let conn = open()?;
    let stored: Option<(i64, bool)> = conn
        .query_row("SELECT created_at, archived FROM chats WHERE id = ?1", params![record.id], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .optional()
        .map_err(store)?;
    let archived = stored.is_some_and(|s| s.1);
    record.updated_at = now();
    record.created_at = stored.map_or(record.updated_at, |s| s.0);

    let body = serde_json::to_string(&record).map_err(ChatError::Parse)?;
    // An upsert, not INSERT OR REPLACE: a replaced row would lose `archived`.
    conn.execute(
        "INSERT INTO chats
           (id, workspace, schema_version, title, created_at, updated_at, branched_from, body, role)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(id) DO UPDATE SET
           workspace = excluded.workspace,
           schema_version = excluded.schema_version,
           title = excluded.title,
           created_at = excluded.created_at,
           updated_at = excluded.updated_at,
           branched_from = excluded.branched_from,
           body = excluded.body,
           role = excluded.role",
        params![
            record.id,
            record.workspace,
            record.schema_version,
            record.title,
            record.created_at,
            record.updated_at,
            record.branched_from,
            body,
            record.role.and_then(|role| serde_json::to_value(role).ok()?.as_str().map(str::to_string)),
        ],
    )
    .map_err(store)?;
    Ok(ChatSummary { archived, ..ChatSummary::from(&record) })
}

/// Writes one chat as a Markdown transcript, wherever the user chose to put
/// it — an ordinary file of theirs, not one of ours, so it is written plainly
/// rather than with the app directory's private permissions.
pub fn export(id: &str, path: &Path) -> Result<(), ChatError> {
    let record = load(id)?;
    fs::write(path, chat_export::to_markdown(&record)).map_err(|e| ChatError::Write(e.to_string()))
}

/// Files a chat away from the list, or back. The transcript is not touched,
/// nor is `updated_at`: archiving is not something said in the chat.
pub fn set_archived(id: &str, archived: bool) -> Result<(), ChatError> {
    chat_record::check_id(id)?;
    match open()?
        .execute("UPDATE chats SET archived = ?2 WHERE id = ?1", params![id, archived])
        .map_err(store)?
    {
        0 => Err(ChatError::NotFound(id.to_string())),
        _ => Ok(()),
    }
}

pub fn delete(id: &str) -> Result<(), ChatError> {
    chat_record::check_id(id)?;
    match open()?.execute("DELETE FROM chats WHERE id = ?1", params![id]).map_err(store)? {
        0 => Err(ChatError::NotFound(id.to_string())),
        _ => Ok(()),
    }
}

/// Every chat about `workspace` — a folder that is gone with them. How many.
pub fn delete_in(workspace: &str) -> Result<usize, ChatError> {
    open()?.execute("DELETE FROM chats WHERE workspace = ?1", params![workspace]).map_err(store)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::with_app_dir;

    fn blocks(text: &str) -> Value {
        serde_json::json!([{ "kind": "user", "id": "user:0", "text": text }])
    }

    fn body(id: &str) -> String {
        open().unwrap().query_row("SELECT body FROM chats WHERE id = ?1", [id], |r| r.get(0)).unwrap()
    }

    fn save_one(id: &str, workspace: &str, said: &str) -> ChatSummary {
        save(
            id,
            workspace,
            &[LlmMessage::user(said)],
            &blocks(said),
            &[],
            Some("# Plan"),
            None,
        )
        .unwrap()
    }

    fn compacted() -> Vec<LlmMessage> {
        vec![
            LlmMessage::user(format!("{SUMMARY_PREFIX}\n\nthe user asked about tokens")),
            LlmMessage::user("and the lexer?"),
        ]
    }

    /// Compacted, the model's history opens with the summary. The chat is
    /// still named after what the user first asked, as the transcript has it.
    #[test]
    fn a_compacted_chat_keeps_its_name() {
        with_app_dir("chat-store-compacted-title", || {
            let saved = save("one", "/repo", &compacted(), &blocks("why is a token dropped?"), &[], None, None)
                .unwrap();
            assert_eq!(saved.title, "why is a token dropped?");
        });
    }

    /// Chats an older build named after the summary get their name back the
    /// next time the folder's chats are listed; other folders are not touched.
    #[test]
    fn a_chat_named_after_its_summary_is_named_again() {
        with_app_dir("chat-store-summary-title", || {
            for (id, workspace) in [("one", "/repo"), ("two", "/other")] {
                save(id, workspace, &compacted(), &blocks("why is a token dropped?"), &[], None, None).unwrap();
                open()
                    .unwrap()
                    .execute("UPDATE chats SET title = ?1 WHERE id = ?2", params![SUMMARY_PREFIX, id])
                    .unwrap();
            }
            let titled = |id: &str| -> String {
                open().unwrap().query_row("SELECT title FROM chats WHERE id = ?1", [id], |r| r.get(0)).unwrap()
            };

            assert_eq!(list("/repo").unwrap()[0].title, "why is a token dropped?");
            assert_eq!(titled("one"), "why is a token dropped?");
            assert_eq!(titled("two"), SUMMARY_PREFIX);
        });
    }

    #[test]
    fn a_folder_s_chats_go_with_it_and_no_one_else_s() {
        with_app_dir("chat-store-delete-in", || {
            save_one("one", "/wt/main-1", "a");
            save_one("two", "/wt/main-1", "b");
            save_one("three", "/wt/main-10", "c");
            save_one("four", "/repo", "d");

            assert_eq!(delete_in("/wt/main-1").unwrap(), 2);
            assert!(list("/wt/main-1").unwrap().is_empty());
            assert_eq!(list("/wt/main-10").unwrap().len(), 1);
            assert_eq!(list("/repo").unwrap().len(), 1);
            assert_eq!(delete_in("/wt/main-1").unwrap(), 0);
        });
    }

    #[test]
    fn a_saved_chat_comes_back_whole() {
        with_app_dir("chat-store-round-trip", || {
            save_one("one", "/repo", "why does it drop the token?");

            let record = load("one").unwrap();
            assert_eq!(record.workspace, "/repo");
            assert_eq!(record.title, "why does it drop the token?");
            assert_eq!(record.messages, vec![LlmMessage::user("why does it drop the token?")]);
            assert_eq!(record.blocks, blocks("why does it drop the token?"));
            assert_eq!(record.plan.as_deref(), Some("# Plan"));
        });
    }

    /// The transcript goes where the user pointed, not into the app's own
    /// folder — the whole point of an export is that it leaves.
    #[test]
    fn a_chat_is_exported_as_markdown_where_it_was_asked_for() {
        with_app_dir("chat-store-export", || {
            save_one("one", "/repo", "why does it drop the token?");
            let path = app_dir::ensure().unwrap().join("transcript.md");

            export("one", &path).unwrap();

            let text = fs::read_to_string(&path).unwrap();
            assert!(text.starts_with("# why does it drop the token?"), "{text}");
            assert!(text.contains("## User"), "{text}");
        });
    }

    #[test]
    fn exporting_a_chat_that_is_not_there_says_so() {
        with_app_dir("chat-store-export-missing", || {
            let path = app_dir::ensure().unwrap().join("transcript.md");
            assert!(matches!(export("nope", &path), Err(ChatError::NotFound(_))));
        });
    }

    #[test]
    fn nothing_saved_yet_lists_as_nothing() {
        with_app_dir("chat-store-empty", || {
            assert!(list("/repo").unwrap().is_empty());
        });
    }

    /// The sidebar is per project. A chat about another folder appearing in
    /// this one's list is worse than no list.
    #[test]
    fn a_chat_belongs_to_the_folder_it_was_about() {
        with_app_dir("chat-store-scope", || {
            save_one("one", "/repo/a", "first");
            save_one("two", "/repo/b", "second");

            let ids: Vec<String> = list("/repo/a").unwrap().into_iter().map(|c| c.id).collect();
            assert_eq!(ids, ["one"]);
        });
    }

    #[test]
    fn the_list_is_most_recent_first() {
        with_app_dir("chat-store-order", || {
            save_one("older", "/repo", "first");
            save_one("newer", "/repo", "second");
            // Same millisecond is likely here; make the order the one under test.
            open()
                .unwrap()
                .execute("UPDATE chats SET updated_at = updated_at + 1000 WHERE id = 'newer'", [])
                .unwrap();

            let ids: Vec<String> = list("/repo").unwrap().into_iter().map(|c| c.id).collect();
            assert_eq!(ids, ["newer", "older"]);
        });
    }

    /// Resaving is the normal case — every turn does it — and it must not
    /// reset when the conversation began.
    #[test]
    fn saving_again_keeps_the_moment_the_chat_started() {
        with_app_dir("chat-store-created", || {
            save_one("one", "/repo", "first");
            let started = load("one").unwrap().created_at;

            std::thread::sleep(std::time::Duration::from_millis(5));
            save(
                "one",
                "/repo",
                &[LlmMessage::user("first"), LlmMessage::assistant("done")],
                &blocks("first"),
                &[],
                None,
                None,
            )
            .unwrap();

            let again = load("one").unwrap();
            assert_eq!(again.created_at, started);
            assert!(again.updated_at >= started);
            assert_eq!(again.messages.len(), 2);
        });
    }

    /// A damaged transcript fails to open on its own; the rest still list and open.
    #[test]
    fn a_damaged_chat_does_not_hide_the_others() {
        with_app_dir("chat-store-damaged", || {
            save_one("good", "/repo", "readable");
            save_one("bad", "/repo", "unreadable");
            open().unwrap().execute("UPDATE chats SET body = '{ not json' WHERE id = 'bad'", []).unwrap();

            let ids: Vec<String> = list("/repo").unwrap().into_iter().map(|c| c.id).collect();
            assert!(ids.contains(&"good".to_string()), "{ids:?}");
            assert!(load("good").is_ok());
            assert!(matches!(load("bad"), Err(ChatError::Parse(_))));
        });
    }

    /// A chat written by a newer build stays where it is, untouched and
    /// unlisted, rather than being reported as damage or quietly overwritten.
    #[test]
    fn a_chat_from_a_newer_build_is_left_alone() {
        with_app_dir("chat-store-newer", || {
            save_one("future", "/repo", "from tomorrow");
            let mut value: Value = serde_json::from_str(&body("future")).unwrap();
            value["schemaVersion"] = serde_json::json!(CHAT_SCHEMA_VERSION + 1);
            let raw = value.to_string();
            open()
                .unwrap()
                .execute(
                    "UPDATE chats SET schema_version = ?1, body = ?2 WHERE id = 'future'",
                    params![CHAT_SCHEMA_VERSION + 1, raw],
                )
                .unwrap();

            assert!(list("/repo").unwrap().is_empty());
            assert!(matches!(load("future"), Err(ChatError::UnsupportedVersion(_))));
            assert_eq!(body("future"), raw);
        });
    }

    /// The sidebar marks a branch by this, so it has to reach the listing.
    #[test]
    fn a_branch_says_where_it_came_from() {
        with_app_dir("chat-store-branch", || {
            save_one("one", "/repo", "first");
            save("two", "/repo", &[LlmMessage::user("first")], &blocks("first"), &[], None, Some("one")).unwrap();

            let listed = list("/repo").unwrap();
            let from = |id: &str| listed.iter().find(|c| c.id == id).unwrap().branched_from.clone();
            assert_eq!(from("one"), None);
            assert_eq!(from("two").as_deref(), Some("one"));
            assert_eq!(load("two").unwrap().branched_from.as_deref(), Some("one"));
        });
    }

    #[test]
    fn a_deleted_chat_is_gone() {
        with_app_dir("chat-store-delete", || {
            save_one("one", "/repo", "first");
            delete("one").unwrap();

            assert!(list("/repo").unwrap().is_empty());
            assert!(matches!(load("one"), Err(ChatError::NotFound(_))));
            assert!(matches!(delete("one"), Err(ChatError::NotFound(_))));
            assert!(!exists("one").unwrap());
        });
    }

    #[test]
    fn an_archived_chat_is_listed_as_archived_and_can_come_back() {
        with_app_dir("chat-store-archive", || {
            save_one("one", "/repo", "first");
            save_one("two", "/repo", "second");
            set_archived("one", true).unwrap();

            let archived = |id: &str| list("/repo").unwrap().into_iter().find(|c| c.id == id).unwrap().archived;
            assert!(archived("one"));
            assert!(!archived("two"));

            set_archived("one", false).unwrap();
            assert!(!archived("one"));
            assert!(matches!(set_archived("nope", true), Err(ChatError::NotFound(_))));
            assert!(matches!(set_archived("../x", true), Err(ChatError::BadId(_))));
        });
    }

    /// Every turn saves the chat whole; a turn in an archived chat must not
    /// quietly bring it back to the list.
    #[test]
    fn saving_an_archived_chat_keeps_it_archived() {
        with_app_dir("chat-store-archive-save", || {
            save_one("one", "/repo", "first");
            set_archived("one", true).unwrap();

            let saved = save_one("one", "/repo", "first");

            assert!(saved.archived);
            assert!(list("/repo").unwrap()[0].archived);
            assert!(!save_one("two", "/repo", "second").archived);
        });
    }

    /// A database written before archiving existed gains the column and keeps
    /// its chats, all of them unarchived.
    #[test]
    fn a_database_from_before_archiving_opens_with_nothing_archived() {
        with_app_dir("chat-store-archive-upgrade", || {
            let path = app_dir::ensure().unwrap().join(FILE);
            Connection::open(&path)
                .unwrap()
                .execute_batch(
                    "CREATE TABLE chats (
                       id TEXT PRIMARY KEY, workspace TEXT NOT NULL, schema_version INTEGER NOT NULL,
                       title TEXT NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
                       branched_from TEXT, body TEXT NOT NULL);
                     INSERT INTO chats VALUES ('old', '/repo', 1, 'old', 1, 1, NULL, '{}');",
                )
                .unwrap();

            let listed = list("/repo").unwrap();
            assert_eq!(listed.len(), 1);
            assert!(!listed[0].archived);
            set_archived("old", true).unwrap();
            assert!(list("/repo").unwrap()[0].archived);
        });
    }

    /// Chat mode's conversations are listed together, apart from every
    /// folder's, and keep their role.
    #[test]
    fn a_plain_chat_is_filed_under_no_folder_with_its_role() {
        with_app_dir("chat-store-plain", || {
            save_one("agent", "/repo", "fix it");
            let said = [LlmMessage::user("what is a monad?"), LlmMessage::assistant("a monoid in…")];
            let shown = serde_json::json!([{ "role": "assistant", "text": "a monoid in…", "reasoning": "hm" }]);
            let pin = KubePin { kubeconfig: "prod".into(), context: Some("eks".into()), namespace: Some("payments".into()), writes: false };
            let saved = save_plain("plain", ChatRole::Assistant, Some(pin.clone()), &said, &shown).unwrap();

            assert_eq!(saved.title, "what is a monad?");
            let ids = |folder: &str| list(folder).unwrap().into_iter().map(|c| c.id).collect::<Vec<_>>();
            assert_eq!(ids(NO_FOLDER), ["plain"]);
            // The list's row says who answers, without reading the body.
            save_plain("cluster", ChatRole::Kubernetes, None, &said, &shown).unwrap();
            let roles: Vec<_> = list(NO_FOLDER).unwrap().into_iter().map(|c| (c.id, c.role)).collect();
            assert!(roles.contains(&("plain".into(), Some(ChatRole::Assistant))) && roles.contains(&("cluster".into(), Some(ChatRole::Kubernetes))), "{roles:?}");
            assert_eq!(list("/repo").unwrap()[0].role, None);
            // A row from before the column, or with a role this build does not know.
            open().unwrap().execute("UPDATE chats SET role = CASE id WHEN 'plain' THEN NULL ELSE 'astrologer' END", []).unwrap();
            assert!(list(NO_FOLDER).unwrap().iter().all(|c| c.role.is_none()));
            delete("cluster").unwrap();
            assert_eq!(ids("/repo"), ["agent"]);
            let record = load("plain").unwrap();
            assert_eq!((record.role, record.messages, record.blocks), (Some(ChatRole::Assistant), said.to_vec(), shown));
            assert_eq!(record.kube, Some(pin));
            assert_eq!(load("agent").unwrap().role, None);
        });
    }

    /// The id arrives from the window; one that is not an id is refused before
    /// any query runs.
    #[test]
    fn an_id_that_is_not_an_id_never_reaches_the_filesystem() {
        with_app_dir("chat-store-bad-id", || {
            assert!(matches!(load("../settings"), Err(ChatError::BadId(_))));
            assert!(matches!(
                save("../settings", "/repo", &[], &Value::Null, &[], None, None),
                Err(ChatError::BadId(_))
            ));
        });
    }
}
