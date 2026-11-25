//! Saved conversations: one row per chat in `<app dir>/chats.db`.
//!
//! The listing columns (`workspace`, `title`, `updated_at`, `branched_from`)
//! are real columns, so the sidebar is one indexed query and never parses a
//! transcript. The record itself is stored whole as JSON in `body`, in the
//! `domain::chat_record` format — that file stays the contract.
//!
//! `archived`, `pinned`, `group_id` and `generated_title` are the columns not
//! taken from the body: where the chat is filed, whether it is held above the
//! list, which folder holds it and what the model named it are not what was
//! said, so saving a turn leaves them as they were. A generated title is
//! listed in place of the one derived from the first message.
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
    self, ChatError, ChatGroup, ChatModel, ChatRecord, ChatSummary, GroupList, CHAT_SCHEMA_VERSION, NO_FOLDER,
};
use crate::domain::chat_role::ChatRole;
use crate::domain::kube::KubePin;
use crate::domain::llm::LlmMessage;
use crate::domain::tools::Task;
use crate::infra::app_dir;

const FILE: &str = "chats.db";

/// A record update is read-modify-write: relocating a chat and saving its last
/// turn must not each write back a whole record read before the other finished.
/// Saves are once a turn, so one process-wide lock is cheaper than a per-chat
/// registry; a per-chat lock is the way past if that ever becomes contention.
fn write_lock() -> std::sync::MutexGuard<'static, ()> {
    static WRITE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    WRITE.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

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
  pinned         INTEGER NOT NULL DEFAULT 0,
  group_id       TEXT,
  role           TEXT
);
CREATE INDEX IF NOT EXISTS chats_workspace ON chats(workspace, updated_at DESC);
CREATE TABLE IF NOT EXISTS chat_groups (
  id         TEXT PRIMARY KEY,
  workspace  TEXT NOT NULL,
  name       TEXT NOT NULL,
  position   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS chat_groups_workspace ON chat_groups(workspace, position);
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
CREATE TABLE IF NOT EXISTS runs (
  kind        TEXT NOT NULL,
  id          INTEGER NOT NULL,
  chat_id     TEXT NOT NULL,
  version     INTEGER NOT NULL,
  started_at  INTEGER NOT NULL,
  ended_at    INTEGER,
  state       TEXT NOT NULL,
  body        TEXT NOT NULL,
  PRIMARY KEY (kind, id)
);
CREATE INDEX IF NOT EXISTS runs_chat ON runs(chat_id, kind, id);
CREATE TABLE IF NOT EXISTS turn_journal (
  chat_id   TEXT NOT NULL,
  turn_id   TEXT NOT NULL,
  seq       INTEGER NOT NULL,
  kind      TEXT NOT NULL,
  body      TEXT NOT NULL,
  msg_count INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (chat_id, seq)
);
";

pub(crate) fn store(e: rusqlite::Error) -> ChatError {
    ChatError::Store(e.to_string())
}

pub(crate) fn open() -> Result<Connection, ChatError> {
    // Opening checks and, on an older database, changes the schema. Two first
    // turns may open together; serialize that short setup so neither sees the
    // other's schema write as `database is locked` or a duplicate column.
    static OPEN: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _opening = OPEN.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
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
    add_column(&conn, "archived", "INTEGER NOT NULL DEFAULT 0")?;
    // A database from before pinning has the table without the column; the
    // same add-with-default as `archived`.
    add_column(&conn, "pinned", "INTEGER NOT NULL DEFAULT 0")?;
    // Null until the chat is moved into a folder, and again when the folder
    // is deleted. The table itself is made by SCHEMA, like any other.
    add_column(&conn, "group_id", "TEXT")?;
    // The same for a chat's role, which the list draws a sign from: null for
    // the agent's chats and for those saved before the column.
    add_column(&conn, "role", "TEXT")?;
    // Null until the model has named the chat, and for good when it could not.
    add_column(&conn, "generated_title", "TEXT")?;
    // The branch a chat works on, listed so the window can tell it from the
    // folder's without loading the transcript.
    add_column(&conn, "git_branch", "TEXT")?;
    // The commit HEAD pointed at when the chat was first saved — what a
    // review counts as "this chat's work". NULL for rows saved before the
    // column and for chats begun before their folder's first commit.
    add_column(&conn, "head_at_start", "TEXT")?;
    // The provider and model this chat speaks with — its own pin, so a pick
    // in another chat never moves it. NULL for rows saved before the columns
    // and until the first save stamps them; a NULL `model` under a provider
    // means "auto", the provider's own pin.
    add_column(&conn, "model_provider", "TEXT")?;
    add_column(&conn, "model", "TEXT")?;
    share_project_folders(&conn)?;
    Ok(conn)
}

/// Folders were once filed per project; now every project shares one set
/// (`GroupList::Projects`). A database from before has them under their
/// projects' paths: they move to the shared set, one folder per name — the
/// later of two same-named folders gives its chats to the first and goes —
/// and are numbered again in the order they stood. A no-op once moved.
fn share_project_folders(conn: &Connection) -> Result<(), ChatError> {
    let shared = GroupList::Projects.key();
    let old: bool = conn
        .query_row("SELECT COUNT(*) > 0 FROM chat_groups WHERE workspace NOT IN (?1, ?2)", params![NO_FOLDER, shared], |row| row.get(0))
        .map_err(store)?;
    if !old {
        return Ok(());
    }
    // Immediate: two windows opening the store at once would otherwise both
    // read the old sets and merge them twice. The second one, let in after
    // the first commits, reads only the shared set and changes nothing.
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate).map_err(store)?;
    let folders: Vec<(String, String)> = {
        let mut stmt = tx
            .prepare(
                "SELECT id, name FROM chat_groups WHERE workspace != ?1
                 ORDER BY workspace != ?2, position, workspace, id",
            )
            .map_err(store)?;
        let rows = stmt.query_map(params![NO_FOLDER, shared], |row| Ok((row.get(0)?, row.get(1)?))).map_err(store)?;
        rows.collect::<Result<_, _>>().map_err(store)?
    };
    // ASCII case only, as `name_taken`'s `lower()` compares.
    let mut kept: Vec<(String, String)> = Vec::new();
    for (id, name) in folders {
        let key = name.to_ascii_lowercase();
        match kept.iter().find(|(_, other)| *other == key) {
            Some((first, _)) => {
                tx.execute("UPDATE chats SET group_id = ?1 WHERE group_id = ?2", params![first, id]).map_err(store)?;
                tx.execute("DELETE FROM chat_groups WHERE id = ?1", params![id]).map_err(store)?;
            }
            None => kept.push((id, key)),
        }
    }
    for (position, (id, _)) in kept.iter().enumerate() {
        tx.execute(
            "UPDATE chat_groups SET workspace = ?2, position = ?3 WHERE id = ?1",
            params![id, shared, position as i64 + 1],
        )
        .map_err(store)?;
    }
    tx.commit().map_err(store)
}

fn add_column(conn: &Connection, name: &str, definition: &str) -> Result<(), ChatError> {
    let has: bool = conn
        .query_row("SELECT COUNT(*) > 0 FROM pragma_table_info('chats') WHERE name = ?1", [name], |row| row.get(0))
        .map_err(store)?;
    if !has {
        conn.execute_batch(&format!("ALTER TABLE chats ADD COLUMN {name} {definition}")).map_err(store)?;
    }
    Ok(())
}

pub(crate) fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Chats belonging to one workspace in the order the sidebar draws them:
/// pinned first, then chats moved into a folder, then the rest by recency;
/// ties broken by id so the order is stable. Rows from a newer build are not
/// listed. Archived chats are listed too, marked: which of them to show is
/// the sidebar's filter.
pub fn list(workspace: &str) -> Result<Vec<ChatSummary>, ChatError> {
    let conn = open()?;
    rename_summary_titles(&conn, workspace)?;
    let mut stmt = conn
        .prepare(
            "SELECT id, COALESCE(generated_title, title), updated_at, branched_from, archived, role, git_branch, pinned, group_id FROM chats
             WHERE workspace = ?1 AND schema_version <= ?2
             ORDER BY pinned DESC, group_id IS NULL, updated_at DESC, id",
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
                git_branch: row.get(6)?,
                pinned: row.get(7)?,
                group: row.get(8)?,
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
    let row: Option<(String, Option<String>, Option<String>)> = open()?
        .query_row("SELECT body, model_provider, model FROM chats WHERE id = ?1", params![id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .optional()
        .map_err(store)?;
    let (body, provider, model) = row.ok_or_else(|| ChatError::NotFound(id.to_string()))?;
    let mut record = chat_record::parse(&body)?;
    // The pin is the columns', never the body's — the window is told it here.
    record.model = provider.map(|provider| ChatModel { provider, model });
    Ok(record)
}

/// One row of a turn's journal (`domain::turn_journal`): the history's
/// tail or whole, or the window's transcript. `seq` is handed out by the
/// insert itself: one `INSERT … SELECT MAX(seq)+1` is a single statement,
/// and SQLite lets one writer at a time, so two writers never share a
/// number.
pub fn journal_append(chat_id: &str, turn_id: &str, kind: &str, body: &Value, msg_count: usize) -> Result<(), ChatError> {
    chat_record::check_id(chat_id)?;
    open()?
        .execute(
            "INSERT INTO turn_journal (chat_id, turn_id, seq, kind, body, msg_count)
             VALUES (?1, ?2, (SELECT COALESCE(MAX(seq), 0) + 1 FROM turn_journal WHERE chat_id = ?1), ?3, ?4, ?5)",
            params![chat_id, turn_id, kind, body.to_string(), msg_count as i64],
        )
        .map_err(store)?;
    Ok(())
}

/// The chat's journal in the order it was written, parsed. A row that does
/// not parse is skipped: a damaged line costs what it carried, not the
/// recovery.
pub fn journal_entries(chat_id: &str) -> Result<Vec<crate::domain::turn_journal::JournalEntry>, ChatError> {
    chat_record::check_id(chat_id)?;
    let conn = open()?;
    let mut stmt = conn
        .prepare("SELECT kind, body FROM turn_journal WHERE chat_id = ?1 ORDER BY seq")
        .map_err(store)?;
    let rows = stmt
        .query_map([chat_id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
        .map_err(store)?;
    let mut entries = Vec::new();
    for row in rows {
        let (kind, body) = row.map_err(store)?;
        if let Ok(value) = serde_json::from_str::<Value>(&body) {
            if let Some(entry) = crate::domain::turn_journal::JournalEntry::parse(&kind, &value) {
                entries.push(entry);
            }
        }
    }
    Ok(entries)
}

/// How many messages of one turn's history its journal already holds: the
/// last snapshot's count plus the appends after it. A resume counts the
/// tail to append from here.
pub fn journal_message_count(chat_id: &str, turn_id: &str) -> Result<usize, ChatError> {
    chat_record::check_id(chat_id)?;
    let conn = open()?;
    let mut stmt = conn
        .prepare("SELECT kind, msg_count FROM turn_journal WHERE chat_id = ?1 AND turn_id = ?2 ORDER BY seq")
        .map_err(store)?;
    let rows = stmt
        .query_map(params![chat_id, turn_id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))
        .map_err(store)?;
    let mut count = 0i64;
    for row in rows {
        let (kind, n) = row.map_err(store)?;
        match kind.as_str() {
            "snapshot" => count = n,
            "append" => count += n,
            _ => {}
        }
    }
    Ok(count.max(0) as usize)
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

/// What the chat is called in the list — the model's name for it first —
/// or `None` when there is no such chat: what names a prototype shown before
/// it had a `prototype.json`.
pub fn title(id: &str) -> Result<Option<String>, ChatError> {
    chat_record::check_id(id)?;
    open()?
        .query_row("SELECT COALESCE(generated_title, title) FROM chats WHERE id = ?1", params![id], |row| row.get(0))
        .optional()
        .map_err(store)
}

/// The commit `HEAD` pointed at when the chat was first saved — one column,
/// no transcript. `None` for a chat without a marker: one saved before the
/// column existed, or begun before its folder's first commit.
pub fn head_at_start(id: &str) -> Result<Option<String>, ChatError> {
    chat_record::check_id(id)?;
    open()?
        .query_row("SELECT head_at_start FROM chats WHERE id = ?1", params![id], |row| row.get::<_, Option<String>>(0))
        .optional()
        .map(|found| found.flatten())
        .map_err(store)
}

/// The provider and model this chat speaks with — one column pair, no
/// transcript. `None` for a chat not pinned yet: one saved before the
/// columns existed, or before its first save stamped them.
pub fn model(id: &str) -> Result<Option<ChatModel>, ChatError> {
    chat_record::check_id(id)?;
    open()?
        .query_row("SELECT model_provider, model FROM chats WHERE id = ?1", params![id], |row| {
            let provider: Option<String> = row.get(0)?;
            let model: Option<String> = row.get(1)?;
            Ok(provider.map(|provider| ChatModel { provider, model }))
        })
        .optional()
        .map(|found| found.flatten())
        .map_err(store)
}

/// Rewrites the chat's pin — a pick made in this chat. Like archiving, this
/// is not something said in the chat: the transcript and `updated_at` are
/// not touched, so a pick does not move the chat in the sidebar.
pub fn set_model(id: &str, pin: &ChatModel) -> Result<(), ChatError> {
    chat_record::check_id(id)?;
    match open()?
        .execute("UPDATE chats SET model_provider = ?2, model = ?3 WHERE id = ?1", params![id, pin.provider, pin.model])
        .map_err(store)?
    {
        0 => Err(ChatError::NotFound(id.to_string())),
        _ => Ok(()),
    }
}

/// Stamps the pin on a chat that has none — its first save. A chat already
/// pinned keeps its pin: a repeated save is a no-op, and a chat saved before
/// the columns is stamped only once, by whichever save comes first.
pub fn stamp_model_if_absent(id: &str, pin: &ChatModel) -> Result<(), ChatError> {
    chat_record::check_id(id)?;
    open()?
        .execute(
            "UPDATE chats SET model_provider = ?2, model = ?3 WHERE id = ?1 AND model_provider IS NULL",
            params![id, pin.provider, pin.model],
        )
        .map_err(store)?;
    Ok(())
}

/// Writes the conversation whole, keeping the moment it started, and folds
/// the turn's journal away — this is the save that ends a turn (or recovers
/// one). A save mid-turn goes through [`save_keep_journal`].
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
    git_branch: Option<&str>,
) -> Result<ChatSummary, ChatError> {
    save_impl(id, workspace, messages, blocks, todos, plan, branched_from, git_branch, true, None)
}

/// As [`save`], but the journal folds for one turn only: the turn this
/// save ends. The next turn may already have started — the queue sends it
/// without waiting for this save — and its base snapshot must survive a
/// delete that is still in flight. `None` folds every turn's rows, as a
/// recovery does.
#[allow(clippy::too_many_arguments)]
pub fn save_turn(
    id: &str,
    workspace: &str,
    messages: &[LlmMessage],
    blocks: &Value,
    todos: &[Task],
    plan: Option<&str>,
    branched_from: Option<&str>,
    git_branch: Option<&str>,
    turn_id: Option<&str>,
) -> Result<ChatSummary, ChatError> {
    save_impl(id, workspace, messages, blocks, todos, plan, branched_from, git_branch, true, turn_id)
}

/// As [`save`], but the journal stays: a save that does not end the turn
/// (a plan edited while it pauses for approval) must leave it alone — the
/// resume counts its tail from it, and a recovery replays it.
#[allow(clippy::too_many_arguments)]
pub fn save_keep_journal(
    id: &str,
    workspace: &str,
    messages: &[LlmMessage],
    blocks: &Value,
    todos: &[Task],
    plan: Option<&str>,
    branched_from: Option<&str>,
    git_branch: Option<&str>,
) -> Result<ChatSummary, ChatError> {
    save_impl(id, workspace, messages, blocks, todos, plan, branched_from, git_branch, false, None)
}

#[allow(clippy::too_many_arguments)]
fn save_impl(
    id: &str,
    workspace: &str,
    messages: &[LlmMessage],
    blocks: &Value,
    todos: &[Task],
    plan: Option<&str>,
    branched_from: Option<&str>,
    git_branch: Option<&str>,
    fold_journal: bool,
    fold_turn: Option<&str>,
) -> Result<ChatSummary, ChatError> {
    let _write = write_lock();
    // An existing chat keeps its checkout: a save already in flight when the
    // chat moved to a worktree must not write the old folder or its branch back.
    // It keeps its start marker the same way: the first save stamps it, later
    // saves pass the stored one through, like `created_at`.
    let (workspace, worktree_base, git_branch, head_at_start) = match load(id) {
        Ok(record) => {
            let branch = if record.workspace == workspace {
                git_branch.map(str::to_string)
            } else {
                crate::infra::git_head::current_branch(Path::new(&record.workspace)).or(record.git_branch)
            };
            (record.workspace, record.worktree_base, branch, record.head_at_start)
        }
        Err(ChatError::NotFound(_)) => (
            workspace.to_string(),
            None,
            git_branch.map(str::to_string),
            crate::infra::git_head::current_commit(Path::new(workspace)),
        ),
        Err(e) => return Err(e),
    };
    #[cfg(test)]
    std::thread::yield_now();
    upsert(
        ChatRecord {
            schema_version: CHAT_SCHEMA_VERSION,
            id: id.to_string(),
            workspace,
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
            worktree_base,
            git_branch,
            head_at_start,
            model: None,
        },
        // A turn-ending save folds the journal into this record and deletes
        // it in the same transaction — a crash between the two would leave
        // a finished turn to be "recovered" over its own saved result.
        fold_journal,
        fold_turn,
    )
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
    let _write = write_lock();
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
        worktree_base: None,
        git_branch: None,
        head_at_start: None,
        model: None,
    }, false, None)
}

/// Moves only the chat's own record; the worktree has already been made.
pub fn relocate(id: &str, workspace: &str, base: &str) -> Result<ChatSummary, ChatError> {
    let _write = write_lock();
    let mut record = load(id)?;
    #[cfg(test)]
    std::thread::yield_now();
    record.workspace = workspace.to_string();
    record.worktree_base = Some(base.to_string());
    record.git_branch = crate::infra::git_head::current_branch(Path::new(workspace));
    // A move is not a turn ending: a running turn's journal stays.
    upsert(record, false, None)
}

/// Writes `record`, stamped now and keeping the moment it was first saved.
/// `fold_journal` deletes the chat's turn journal in the same transaction —
/// true only for a save that ends a turn (or recovers one); `fold_turn`
/// scopes the delete to one turn, `None` taking every turn's rows.
fn upsert(mut record: ChatRecord, fold_journal: bool, fold_turn: Option<&str>) -> Result<ChatSummary, ChatError> {
    chat_record::check_id(&record.id)?;
    let mut conn = open()?;
    let stored: Option<(i64, bool, Option<String>, bool, Option<String>)> = conn
        .query_row("SELECT created_at, archived, generated_title, pinned, group_id FROM chats WHERE id = ?1", params![record.id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
        })
        .optional()
        .map_err(store)?;
    record.updated_at = now();
    record.created_at = stored.as_ref().map_or(record.updated_at, |s| s.0);
    if stored.is_none() {
        // A chat's first save is its start: one session, for the Usage pane.
        crate::infra::daily_metrics::record(&[(crate::domain::metrics::Metric::Sessions, &record.workspace, 1)]);
    }
    let (archived, generated, pinned, group) = stored.map_or((false, None, false, None), |s| (s.1, s.2, s.3, s.4));

    // The pin lives in the columns, never in the body: a relocate upserts a
    // loaded record, whose field `load` filled from the column. Clearing it
    // here keeps the body the documented shape; the columns are not named
    // below, so the pin itself stays as it was.
    record.model = None;
    let body = serde_json::to_string(&record).map_err(ChatError::Parse)?;
    // An upsert, not INSERT OR REPLACE: a replaced row would lose `archived`,
    // `pinned`, `group_id` and `generated_title`. A new branch starts with its
    // original's name, as it starts with its first message. `head_at_start`
    // is written only by the INSERT half — the conflict branch deliberately
    // leaves it: a chat's start is its first save's, however often the chat
    // is saved afterwards.
    let tx = conn.transaction().map_err(store)?;
    tx.execute(
        "INSERT INTO chats
           (id, workspace, schema_version, title, created_at, updated_at, branched_from, body, role, git_branch, generated_title, head_at_start)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, (SELECT generated_title FROM chats WHERE id = ?7), ?11)
         ON CONFLICT(id) DO UPDATE SET
           workspace = excluded.workspace,
           schema_version = excluded.schema_version,
           title = excluded.title,
           created_at = excluded.created_at,
           updated_at = excluded.updated_at,
           branched_from = excluded.branched_from,
           body = excluded.body,
           role = excluded.role,
           git_branch = excluded.git_branch",
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
            record.git_branch,
            record.head_at_start,
        ],
    )
    .map_err(store)?;
    if fold_journal {
        match fold_turn {
            Some(turn) => tx
                .execute("DELETE FROM turn_journal WHERE chat_id = ?1 AND turn_id = ?2", params![record.id, turn])
                .map_err(store)?,
            None => tx.execute("DELETE FROM turn_journal WHERE chat_id = ?1", [&record.id]).map_err(store)?,
        };
    }
    tx.commit().map_err(store)?;
    let mut summary = ChatSummary { archived, pinned, group, ..ChatSummary::from(&record) };
    if let Some(title) = generated {
        summary.title = title;
    }
    Ok(summary)
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

/// Holds a chat above the list, or lets it sink back to its day. Like
/// archiving, this is filing, not conversation: the transcript and
/// `updated_at` are not touched, so pinning does not move the chat under
/// "Today" and the day's order stays the order of work.
pub fn set_pinned(id: &str, pinned: bool) -> Result<(), ChatError> {
    chat_record::check_id(id)?;
    match open()?
        .execute("UPDATE chats SET pinned = ?2 WHERE id = ?1", params![id, pinned])
        .map_err(store)?
    {
        0 => Err(ChatError::NotFound(id.to_string())),
        _ => Ok(()),
    }
}

/// The folders of one set, in the order the user arranged them. An empty
/// folder is listed too — otherwise a folder made ahead of its first chat
/// could not be renamed or deleted at all.
pub fn groups(list: GroupList) -> Result<Vec<ChatGroup>, ChatError> {
    let workspace = list.key();
    let conn = open()?;
    let mut stmt = conn
        .prepare("SELECT id, name, position FROM chat_groups WHERE workspace = ?1 ORDER BY position, id")
        .map_err(store)?;
    let rows = stmt
        .query_map([workspace], |row| {
            Ok(ChatGroup {
                id: row.get(0)?,
                name: row.get(1)?,
                position: row.get(2)?,
            })
        })
        .map_err(store)?;
    rows.collect::<Result<_, _>>().map_err(store)
}

/// A folder's name as it is stored and compared: trimmed, and empty after
/// trimming is no name at all.
fn checked_name(name: &str) -> Result<String, ChatError> {
    let name = name.trim();
    if name.is_empty() {
        Err(ChatError::EmptyName)
    } else {
        Ok(name.to_string())
    }
}

/// Whether another folder of the same set already carries the name —
/// case-insensitively, so "Release" and "release" are one folder to the eye.
fn name_taken(conn: &Connection, workspace: &str, name: &str, except_id: Option<&str>) -> Result<bool, ChatError> {
    conn.query_row(
        "SELECT COUNT(*) > 0 FROM chat_groups WHERE workspace = ?1 AND lower(name) = lower(?2) AND id != ?3",
        params![workspace, name, except_id.unwrap_or("")],
        |row| row.get(0),
    )
    .map_err(store)
}

/// Makes a folder at the end of the set's list.
pub fn create_group(list: GroupList, name: &str) -> Result<ChatGroup, ChatError> {
    let workspace = list.key();
    let _write = write_lock();
    let name = checked_name(name)?;
    let conn = open()?;
    if name_taken(&conn, workspace, &name, None)? {
        return Err(ChatError::AlreadyExists(name));
    }
    let position: i64 = conn
        .query_row("SELECT COALESCE(MAX(position), 0) + 1 FROM chat_groups WHERE workspace = ?1", [workspace], |row| row.get(0))
        .map_err(store)?;
    let group = ChatGroup {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        position,
    };
    conn.execute(
        "INSERT INTO chat_groups (id, workspace, name, position) VALUES (?1, ?2, ?3, ?4)",
        params![group.id, workspace, group.name, group.position],
    )
    .map_err(store)?;
    Ok(group)
}

/// Renames a folder. The name's rules are the creation's; another folder's
/// name is refused rather than silently numbered.
pub fn rename_group(list: GroupList, id: &str, name: &str) -> Result<(), ChatError> {
    let workspace = list.key();
    let _write = write_lock();
    let name = checked_name(name)?;
    let conn = open()?;
    let own: bool = conn
        .query_row("SELECT COUNT(*) > 0 FROM chat_groups WHERE id = ?1 AND workspace = ?2", params![id, workspace], |row| row.get(0))
        .map_err(store)?;
    if !own {
        return Err(ChatError::NotFound(id.to_string()));
    }
    if name_taken(&conn, workspace, &name, Some(id))? {
        return Err(ChatError::AlreadyExists(name));
    }
    conn.execute("UPDATE chat_groups SET name = ?2 WHERE id = ?1", params![id, name]).map_err(store)?;
    Ok(())
}

/// Removes the folder and files its chats under no folder, in one transaction:
/// deleting what reads as "remove the label" must not delete the
/// conversations it held.
pub fn delete_group(list: GroupList, id: &str) -> Result<(), ChatError> {
    let workspace = list.key();
    let _write = write_lock();
    let mut conn = open()?;
    let own: bool = conn
        .query_row("SELECT COUNT(*) > 0 FROM chat_groups WHERE id = ?1 AND workspace = ?2", params![id, workspace], |row| row.get(0))
        .map_err(store)?;
    if !own {
        return Err(ChatError::NotFound(id.to_string()));
    }
    let tx = conn.transaction().map_err(store)?;
    tx.execute("UPDATE chats SET group_id = NULL WHERE group_id = ?1", params![id]).map_err(store)?;
    tx.execute("DELETE FROM chat_groups WHERE id = ?1", params![id]).map_err(store)?;
    tx.commit().map_err(store)?;
    Ok(())
}

/// Moves a chat into a folder of its set, or out to no folder with `None`.
/// The folder has to be of the chat's set: a project's chat goes into the
/// projects' shared folders, never Chat mode's, and the other way round. The
/// write lock is taken because the
/// check and the write are two statements: without it, a folder deleted in
/// between would leave the chat pointing at a group that is gone.
pub fn set_group(id: &str, group: Option<&str>) -> Result<(), ChatError> {
    let _write = write_lock();
    chat_record::check_id(id)?;
    let conn = open()?;
    let workspace: Option<String> = conn
        .query_row("SELECT workspace FROM chats WHERE id = ?1", params![id], |row| row.get(0))
        .optional()
        .map_err(store)?;
    let workspace = workspace.ok_or_else(|| ChatError::NotFound(id.to_string()))?;
    if let Some(group) = group {
        let list = GroupList::of_workspace(&workspace).key();
        let own: bool = conn
            .query_row("SELECT COUNT(*) > 0 FROM chat_groups WHERE id = ?1 AND workspace = ?2", params![group, list], |row| row.get(0))
            .map_err(store)?;
        if !own {
            return Err(ChatError::NotFound(group.to_string()));
        }
    }
    match conn
        .execute("UPDATE chats SET group_id = ?2 WHERE id = ?1", params![id, group])
        .map_err(store)?
    {
        0 => Err(ChatError::NotFound(id.to_string())),
        _ => Ok(()),
    }
}

/// Names a chat for good. A chat named already keeps its name: `false`, as
/// for one deleted while the model was asked.
pub fn set_generated_title(id: &str, title: &str) -> Result<bool, ChatError> {
    chat_record::check_id(id)?;
    open()?
        .execute(
            "UPDATE chats SET generated_title = ?2 WHERE id = ?1 AND generated_title IS NULL",
            params![id, title],
        )
        .map(|changed| changed > 0)
        .map_err(store)
}

/// The chat, and the record of its helper runs and processes (`run_store`).
pub fn delete(id: &str) -> Result<(), ChatError> {
    chat_record::check_id(id)?;
    let conn = open()?;
    conn.execute("DELETE FROM runs WHERE chat_id = ?1", params![id]).map_err(store)?;
    conn.execute("DELETE FROM turn_journal WHERE chat_id = ?1", params![id]).map_err(store)?;
    match conn.execute("DELETE FROM chats WHERE id = ?1", params![id]).map_err(store)? {
        0 => Err(ChatError::NotFound(id.to_string())),
        _ => Ok(()),
    }
}

/// Every chat about `workspace` — a folder that is gone with them. How many.
pub fn delete_in(workspace: &str) -> Result<usize, ChatError> {
    let conn = open()?;
    conn.execute("DELETE FROM runs WHERE chat_id IN (SELECT id FROM chats WHERE workspace = ?1)", params![workspace])
        .map_err(store)?;
    conn.execute("DELETE FROM turn_journal WHERE chat_id IN (SELECT id FROM chats WHERE workspace = ?1)", params![workspace])
        .map_err(store)?;
    conn.execute("DELETE FROM chats WHERE workspace = ?1", params![workspace]).map_err(store)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::agents::{AgentInfo, AgentState};
    use crate::domain::background::{ProcessInfo, ProcessState, ProcessView};
    use crate::domain::runs::RunLog;
    use crate::infra::run_store::RunStore;
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
            None,
        )
        .unwrap()
    }

    /// A commit on top of `parent`'s, so a test can move the folder's HEAD
    /// between saves.
    fn commit_to(repo: &git2::Repository, parent: Option<&str>) -> String {
        std::fs::write(repo.workdir().unwrap().join("f.txt"), parent.unwrap_or("root")).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(std::path::Path::new("f.txt")).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("Test", "test@example.com").unwrap();
        let parent = parent.map(|sha| repo.find_commit(sha.parse().unwrap()).unwrap());
        let parents: Vec<&git2::Commit> = parent.iter().collect();
        repo.commit(Some("HEAD"), &sig, &sig, "commit", &tree, &parents).unwrap().to_string()
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
    fn a_chat_counts_as_one_session_of_its_folder_however_often_it_is_saved() {
        with_app_dir("chat-sessions", || {
            save_one("a", "/repo", "hi");
            save_one("a", "/repo", "hi again");
            save_one("b", "/other", "hello");

            let sessions = crate::infra::daily_metrics::read(&[crate::domain::metrics::Metric::Sessions]).unwrap();
            let counted: Vec<_> = sessions.iter().map(|r| (r.key.as_str(), r.value)).collect();
            assert_eq!(counted, [("/other", 1), ("/repo", 1)]);
        });
    }

    #[test]
    fn a_compacted_chat_keeps_its_name() {
        with_app_dir("chat-store-compacted-title", || {
            let saved = save("one", "/repo", &compacted(), &blocks("why is a token dropped?"), &[], None, None, None)
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
                save(id, workspace, &compacted(), &blocks("why is a token dropped?"), &[], None, None, None).unwrap();
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

            let ran = |chat: &str| {
                let info = ProcessInfo { id: chat.len() as u32, command: "x".into(), description: None, cwd: ".".into(), state: ProcessState::Stopped };
                RunStore.save_process(chat, &ProcessView { info, tail: String::new() });
            };
            let runs = |chat: &str| RunStore.processes(chat).len();
            ran("one");
            ran("three");
            assert_eq!(delete_in("/wt/main-1").unwrap(), 2);
            assert_eq!((runs("one"), runs("three")), (0, 1), "a gone chat's runs go with it");
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
            save("two", "/repo", &[LlmMessage::user("first")], &blocks("first"), &[], None, Some("one"), None).unwrap();

            let listed = list("/repo").unwrap();
            let from = |id: &str| listed.iter().find(|c| c.id == id).unwrap().branched_from.clone();
            assert_eq!(from("one"), None);
            assert_eq!(from("two").as_deref(), Some("one"));
            assert_eq!(load("two").unwrap().branched_from.as_deref(), Some("one"));
        });
    }

    /// Each chat keeps the git branch it was last saved on: another chat of the
    /// same folder moving to another branch does not move it, and its own next
    /// save does.
    #[test]
    fn a_chat_keeps_its_own_git_branch() {
        with_app_dir("chat-store-git-branch", || {
            let said = |id: &str, branch: Option<&str>| {
                save(id, "/repo", &[LlmMessage::user("hi")], &blocks("hi"), &[], None, None, branch).unwrap()
            };
            assert_eq!(said("a", Some("main")).git_branch.as_deref(), Some("main"));
            said("b", Some("feature"));

            let branch_of = |id: &str| list("/repo").unwrap().into_iter().find(|c| c.id == id).unwrap().git_branch;
            assert_eq!(branch_of("a").as_deref(), Some("main"));
            assert_eq!(branch_of("b").as_deref(), Some("feature"));
            assert_eq!(load("a").unwrap().git_branch.as_deref(), Some("main"));

            said("a", Some("feature"));
            assert_eq!(branch_of("a").as_deref(), Some("feature"));
            said("a", None);
            assert_eq!(branch_of("a"), None);
        });
    }

    #[test]
    fn a_save_already_in_flight_cannot_move_a_chat_back_from_its_worktree() {
        with_app_dir("chat-store-save-after-move", || {
            save_one("one", "/repo", "first");
            relocate("one", "/repo-worktree", "main").unwrap();

            save_one("one", "/repo", "finished meanwhile");

            let record = load("one").unwrap();
            assert_eq!(record.workspace, "/repo-worktree");
            assert_eq!(record.worktree_base.as_deref(), Some("main"));
        });
    }

    #[test]
    fn concurrent_save_and_relocate_keep_both_the_transcript_and_the_worktree() {
        with_app_dir("chat-store-save-relocate-race", || {
            for round in 0..16 {
                let id = format!("race-{round}");
                save_one(&id, "/repo", "first");
                let start = std::sync::Barrier::new(2);
                std::thread::scope(|threads| {
                    threads.spawn(|| {
                        start.wait();
                        save_one(&id, "/repo", "finished meanwhile");
                    });
                    threads.spawn(|| {
                        start.wait();
                        relocate(&id, "/repo-worktree", "main").unwrap();
                    });
                });

                let record = load(&id).unwrap();
                assert_eq!(record.workspace, "/repo-worktree", "round {round}");
                assert_eq!(record.worktree_base.as_deref(), Some("main"), "round {round}");
                assert_eq!(record.messages, vec![LlmMessage::user("finished meanwhile")], "round {round}");
                assert_eq!(record.blocks, blocks("finished meanwhile"), "round {round}");
            }
        });
    }

    /// Saving a turn derives the title again; the model's name has to outlast it.
    #[test]
    fn a_generated_title_outlasts_the_next_save_and_comes_first() {
        with_app_dir("chat-store-generated-title", || {
            save_one("one", "/repo", "why is a token dropped?");
            assert!(set_generated_title("one", "Lost token").unwrap());
            assert!(!set_generated_title("one", "Another name").unwrap(), "named twice");
            assert!(!set_generated_title("gone", "Name").unwrap());

            let saved = save_one("one", "/repo", "why is a token dropped?");
            assert_eq!(saved.title, "Lost token");
            assert_eq!(title("one").unwrap().as_deref(), Some("Lost token"), "one chat's name, as the list has it");
            assert_eq!(title("gone").unwrap(), None);
            assert_eq!(list("/repo").unwrap()[0].title, "Lost token");

            save("two", "/repo", &[LlmMessage::user("q")], &blocks("q"), &[], None, Some("one"), None).unwrap();
            save_one("three", "/repo", "untouched");
            let title = |id: &str| list("/repo").unwrap().into_iter().find(|c| c.id == id).unwrap().title;
            assert_eq!(title("two"), "Lost token", "a branch starts with its original's name");
            assert_eq!(title("three"), "untouched");
        });
    }

    #[test]
    fn a_deleted_chat_is_gone() {
        with_app_dir("chat-store-delete", || {
            save_one("one", "/repo", "first");
            save_one("two", "/repo", "second");
            let run = |id| AgentInfo { id, task: "t".into(), state: AgentState::Done, steps: Vec::new(), tokens: Default::default(), answer: None, progress: Default::default(), call: None };
            RunStore.save_agent("one", &run(1));
            RunStore.save_agent("two", &run(2));
            delete("one").unwrap();
            assert!(RunStore.agents("one").is_empty(), "its runs go with it");
            assert_eq!(RunStore.agents("two").len(), 1);
            delete("two").unwrap();

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
            assert_eq!(listed[0].git_branch, None);
            set_archived("old", true).unwrap();
            assert!(list("/repo").unwrap()[0].archived);
        });
    }

    /// A database written before pinning existed gains the column and keeps
    /// its chats, none of them pinned.
    #[test]
    fn a_database_from_before_pinning_opens_with_nothing_pinned() {
        with_app_dir("chat-store-pin-upgrade", || {
            let path = app_dir::ensure().unwrap().join(FILE);
            Connection::open(&path)
                .unwrap()
                .execute_batch(
                    "CREATE TABLE chats (
                       id TEXT PRIMARY KEY, workspace TEXT NOT NULL, schema_version INTEGER NOT NULL,
                       title TEXT NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
                       branched_from TEXT, body TEXT NOT NULL, archived INTEGER NOT NULL DEFAULT 0);
                     INSERT INTO chats VALUES ('old', '/repo', 1, 'old', 1, 1, NULL, '{}', 0);",
                )
                .unwrap();

            let listed = list("/repo").unwrap();
            assert_eq!(listed.len(), 1);
            assert!(!listed[0].pinned);
            set_pinned("old", true).unwrap();
            assert!(list("/repo").unwrap()[0].pinned);
        });
    }

    #[test]
    fn a_pinned_chat_is_listed_as_pinned_and_can_sink_back() {
        with_app_dir("chat-store-pin", || {
            save_one("one", "/repo", "first");
            save_one("two", "/repo", "second");
            set_pinned("one", true).unwrap();

            let pinned = |id: &str| list("/repo").unwrap().into_iter().find(|c| c.id == id).unwrap().pinned;
            assert!(pinned("one"));
            assert!(!pinned("two"));

            set_pinned("one", false).unwrap();
            assert!(!pinned("one"));
            assert!(matches!(set_pinned("nope", true), Err(ChatError::NotFound(_))));
            assert!(matches!(set_pinned("../x", true), Err(ChatError::BadId(_))));
        });
    }

    /// Every turn saves the chat whole; a turn in a pinned chat must not
    /// quietly let it sink back to its day.
    #[test]
    fn saving_a_pinned_chat_keeps_it_pinned() {
        with_app_dir("chat-store-pin-save", || {
            save_one("one", "/repo", "first");
            set_pinned("one", true).unwrap();

            let saved = save_one("one", "/repo", "first");

            assert!(saved.pinned);
            assert!(list("/repo").unwrap()[0].pinned);
            assert!(!save_one("two", "/repo", "second").pinned);
        });
    }

    /// Pinned chats come ahead of every day group, newest within each pin
    /// state: the order the sidebar draws, one rule for every consumer.
    #[test]
    fn pinned_chats_come_first_and_then_the_list_is_by_time() {
        with_app_dir("chat-store-pin-order", || {
            save_one("old-pin", "/repo", "a");
            save_one("new-pin", "/repo", "b");
            save_one("fresh", "/repo", "c");
            set_pinned("old-pin", true).unwrap();
            set_pinned("new-pin", true).unwrap();
            open()
                .unwrap()
                .execute(
                    "UPDATE chats SET updated_at = updated_at + CASE id
                       WHEN 'new-pin' THEN 2000 WHEN 'fresh' THEN 1000 ELSE 0 END",
                    [],
                )
                .unwrap();

            let ids: Vec<String> = list("/repo").unwrap().into_iter().map(|c| c.id).collect();
            assert_eq!(ids, ["new-pin", "old-pin", "fresh"]);
        });
    }

    /// A chat moves into a folder and back out, and the listing says which
    /// folder holds it without opening the transcript.
    #[test]
    fn a_chat_moves_into_a_folder_and_back_out() {
        with_app_dir("chat-store-group-set", || {
            save_one("one", "/repo", "first");
            let group = create_group(GroupList::Projects, "release").unwrap();

            set_group("one", Some(&group.id)).unwrap();
            let held = |id: &str| list("/repo").unwrap().into_iter().find(|c| c.id == id).unwrap().group;
            assert_eq!(held("one").as_deref(), Some(group.id.as_str()));

            set_group("one", None).unwrap();
            assert_eq!(held("one"), None);
            assert!(matches!(set_group("nope", Some(&group.id)), Err(ChatError::NotFound(_))));
            assert!(matches!(set_group("../x", Some(&group.id)), Err(ChatError::BadId(_))));
            // Chat mode's folders are another set; a project's chat finds none of them.
            let other = create_group(GroupList::Chat, "release").unwrap();
            assert!(matches!(set_group("one", Some(&other.id)), Err(ChatError::NotFound(_))));
        });
    }

    /// The projects share their folders: one holds chats of several projects,
    /// each still listed under its own project; Chat mode's chats keep to
    /// Chat mode's folders.
    #[test]
    fn a_folder_holds_chats_of_several_projects() {
        with_app_dir("chat-store-group-shared", || {
            save_one("api", "/work/api", "first");
            save_one("web", "/work/web", "second");
            let release = create_group(GroupList::Projects, "release").unwrap();
            set_group("api", Some(&release.id)).unwrap();
            set_group("web", Some(&release.id)).unwrap();

            assert_eq!(list("/work/api").unwrap()[0].group.as_deref(), Some(release.id.as_str()));
            assert_eq!(list("/work/web").unwrap()[0].group.as_deref(), Some(release.id.as_str()));
            assert!(matches!(create_group(GroupList::Projects, "Release"), Err(ChatError::AlreadyExists(_))));

            save_plain("plain", ChatRole::Assistant, None, &[LlmMessage::user("hi")], &serde_json::json!([])).unwrap();
            assert!(matches!(set_group("plain", Some(&release.id)), Err(ChatError::NotFound(_))));
            let notes = create_group(GroupList::Chat, "notes").unwrap();
            set_group("plain", Some(&notes.id)).unwrap();
        });
    }

    /// A database from when every project had its own folders: they move to
    /// the shared set, a name met twice becomes one folder holding the chats
    /// of both, and they are numbered in the order they stood; Chat mode's
    /// stay its own. Opening again changes nothing.
    #[test]
    fn folders_filed_per_project_become_one_shared_set() {
        with_app_dir("chat-store-group-migrate", || {
            save_one("a1", "/work/api", "first");
            save_one("w1", "/work/web", "second");
            let conn = open().unwrap();
            conn.execute_batch(
                "INSERT INTO chat_groups VALUES ('g-api-rel', '/work/api', 'release', 1);
                 INSERT INTO chat_groups VALUES ('g-api-ops', '/work/api', 'ops', 2);
                 INSERT INTO chat_groups VALUES ('g-web-rel', '/work/web', 'Release', 1);
                 INSERT INTO chat_groups VALUES ('g-chat', '', 'release', 1);
                 UPDATE chats SET group_id = 'g-api-rel' WHERE id = 'a1';
                 UPDATE chats SET group_id = 'g-web-rel' WHERE id = 'w1';",
            )
            .unwrap();
            drop(conn);

            let shared = || groups(GroupList::Projects).unwrap().into_iter().map(|g| (g.id, g.name, g.position)).collect::<Vec<_>>();
            let moved = shared();
            assert_eq!(moved.iter().map(|g| g.0.as_str()).collect::<Vec<_>>(), ["g-api-rel", "g-api-ops"]);
            assert_eq!(moved.iter().map(|g| g.2).collect::<Vec<_>>(), [1, 2], "numbered again in the order they stood");
            assert_eq!(list("/work/web").unwrap()[0].group.as_deref(), Some("g-api-rel"), "the second release's chat is in the first");
            assert_eq!(list("/work/api").unwrap()[0].group.as_deref(), Some("g-api-rel"));
            assert_eq!(groups(GroupList::Chat).unwrap().len(), 1, "Chat mode's folder is its own");
            assert_eq!(shared(), moved, "opening again changes nothing");
            let rows: i64 = open().unwrap().query_row("SELECT COUNT(*) FROM chat_groups", [], |row| row.get(0)).unwrap();
            assert_eq!(rows, 3, "the merged folder is gone, not left behind");
        });
    }

    /// Once shared, opening the store writes nothing to the folders: a gap
    /// a deleted folder left in the order stays as it is.
    #[test]
    fn shared_folders_are_not_rewritten_on_open() {
        with_app_dir("chat-store-group-no-rewrite", || {
            create_group(GroupList::Projects, "a").unwrap();
            let gone = create_group(GroupList::Projects, "b").unwrap();
            create_group(GroupList::Projects, "c").unwrap();
            delete_group(GroupList::Projects, &gone.id).unwrap();

            let positions: Vec<i64> = groups(GroupList::Projects).unwrap().into_iter().map(|g| g.position).collect();
            assert_eq!(positions, [1, 3]);
        });
    }

    /// Every turn saves the chat whole; a turn in a folder must not quietly
    /// file the chat back under no folder.
    #[test]
    fn saving_a_grouped_chat_keeps_its_folder() {
        with_app_dir("chat-store-group-save", || {
            save_one("one", "/repo", "first");
            let group = create_group(GroupList::Projects, "release").unwrap();
            set_group("one", Some(&group.id)).unwrap();

            let saved = save_one("one", "/repo", "first");

            assert_eq!(saved.group.as_deref(), Some(group.id.as_str()));
            assert_eq!(list("/repo").unwrap()[0].group.as_deref(), Some(group.id.as_str()));
        });
    }

    /// Removing a folder is removing the label: its chats stay, filed under
    /// no folder.
    #[test]
    fn deleting_a_folder_leaves_its_chats_unfiled() {
        with_app_dir("chat-store-group-delete", || {
            save_one("one", "/repo", "first");
            save_one("two", "/repo", "second");
            let group = create_group(GroupList::Projects, "release").unwrap();
            set_group("one", Some(&group.id)).unwrap();

            delete_group(GroupList::Projects, &group.id).unwrap();

            assert!(groups(GroupList::Projects).unwrap().is_empty());
            let mut unfiled: Vec<String> = list("/repo").unwrap().into_iter().map(|c| (c.id, c.group)).filter(|(_, g)| g.is_none()).map(|(id, _)| id).collect();
            unfiled.sort();
            assert_eq!(unfiled, ["one", "two"]);
            assert!(matches!(delete_group(GroupList::Projects, &group.id), Err(ChatError::NotFound(_))));
            // Chat mode's folder of the same name is not this one.
            let elsewhere = create_group(GroupList::Chat, "release").unwrap();
            assert!(matches!(delete_group(GroupList::Projects, &elsewhere.id), Err(ChatError::NotFound(_))));
        });
    }

    /// A folder made ahead of its first chat is still a folder: it lists, so
    /// it can be renamed, filled and deleted.
    #[test]
    fn an_empty_folder_is_listed() {
        with_app_dir("chat-store-group-empty", || {
            create_group(GroupList::Projects, "release").unwrap();
            let listed = groups(GroupList::Projects).unwrap();
            assert_eq!(listed.len(), 1);
            assert_eq!(listed[0].name, "release");
            assert!(groups(GroupList::Chat).unwrap().is_empty());
        });
    }

    /// The order is the user's, not the alphabet's: positions are handed out
    /// as folders are made and listed back that way.
    #[test]
    fn folders_keep_the_order_they_were_made_in() {
        with_app_dir("chat-store-group-order-folders", || {
            create_group(GroupList::Projects, "beta").unwrap();
            create_group(GroupList::Projects, "alpha").unwrap();

            let names: Vec<String> = groups(GroupList::Projects).unwrap().into_iter().map(|g| g.name).collect();
            assert_eq!(names, ["beta", "alpha"]);
        });
    }

    /// Names are compared trimmed and case-insensitively, per set: "Release"
    /// and "release" are one folder to the eye, and the same name is free in
    /// Chat mode's set.
    #[test]
    fn a_folder_name_is_unique_within_its_workspace() {
        with_app_dir("chat-store-group-names", || {
            let made = create_group(GroupList::Projects, " Release ").unwrap();
            assert_eq!(made.name, "Release");
            assert!(matches!(create_group(GroupList::Projects, "release"), Err(ChatError::AlreadyExists(_))));
            let launch = create_group(GroupList::Projects, "launch").unwrap();
            assert!(matches!(rename_group(GroupList::Projects, &made.id, "Launch"), Err(ChatError::AlreadyExists(_))));
            rename_group(GroupList::Projects, &made.id, " ship ").unwrap();
            assert_eq!(groups(GroupList::Projects).unwrap().iter().find(|g| g.id == made.id).unwrap().name, "ship");
            create_group(GroupList::Chat, "release").unwrap();

            assert!(matches!(create_group(GroupList::Projects, "   "), Err(ChatError::EmptyName)));
            assert!(matches!(rename_group(GroupList::Projects, &launch.id, "  "), Err(ChatError::EmptyName)));
        });
    }

    /// The order the sidebar draws: pinned first, then foldered chats in the
    /// folders' arrangement, then the rest by recency.
    #[test]
    fn the_list_runs_pinned_then_folders_then_the_rest_by_time() {
        with_app_dir("chat-store-group-order", || {
            save_one("pinned", "/repo", "a");
            save_one("in-release", "/repo", "b");
            save_one("in-launch", "/repo", "c");
            save_one("loose", "/repo", "d");
            let launch = create_group(GroupList::Projects, "launch").unwrap();
            let release = create_group(GroupList::Projects, "release").unwrap();
            set_group("in-launch", Some(&launch.id)).unwrap();
            set_group("in-release", Some(&release.id)).unwrap();
            set_pinned("pinned", true).unwrap();
            open()
                .unwrap()
                .execute("UPDATE chats SET updated_at = updated_at + 1000 WHERE id = 'loose'", [])
                .unwrap();

            let ids: Vec<String> = list("/repo").unwrap().into_iter().map(|c| c.id).collect();
            assert_eq!(ids, ["pinned", "in-launch", "in-release", "loose"]);
        });
    }

    /// A database written before folders existed gains the table and the
    /// column and keeps its chats, none of them foldered.
    #[test]
    fn a_database_from_before_folders_opens_with_nothing_foldered() {
        with_app_dir("chat-store-group-upgrade", || {
            let path = app_dir::ensure().unwrap().join(FILE);
            Connection::open(&path)
                .unwrap()
                .execute_batch(
                    "CREATE TABLE chats (
                       id TEXT PRIMARY KEY, workspace TEXT NOT NULL, schema_version INTEGER NOT NULL,
                       title TEXT NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
                       branched_from TEXT, body TEXT NOT NULL, archived INTEGER NOT NULL DEFAULT 0,
                       pinned INTEGER NOT NULL DEFAULT 0);
                     INSERT INTO chats VALUES ('old', '/repo', 1, 'old', 1, 1, NULL, '{}', 0, 0);",
                )
                .unwrap();

            let listed = list("/repo").unwrap();
            assert_eq!(listed.len(), 1);
            assert_eq!(listed[0].group, None);
            assert!(groups(GroupList::Projects).unwrap().is_empty());
            let group = create_group(GroupList::Projects, "release").unwrap();
            set_group("old", Some(&group.id)).unwrap();
            assert_eq!(list("/repo").unwrap()[0].group.as_deref(), Some(group.id.as_str()));
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
                save("../settings", "/repo", &[], &Value::Null, &[], None, None, None),
                Err(ChatError::BadId(_))
            ));
        });
    }

    /// The chat's start is measured once: the first save stamps HEAD's
    /// commit — the record, the column, and the cheap reader all agree — and
    /// no later save, not even one after the folder's HEAD has moved on,
    /// rewrites it.
    #[test]
    fn a_chat_keeps_the_head_of_its_first_save() {
        with_app_dir("chat-store-head-start", || {
            let dir = crate::testing::temp_dir("chat-store-head-repo");
            let repo = git2::Repository::init(&dir).unwrap();
            let first = commit_to(&repo, None);
            let root = dir.display().to_string();
            save_one("one", &root, "first");

            assert_eq!(head_at_start("one").unwrap().as_deref(), Some(first.as_str()));
            assert_eq!(load("one").unwrap().head_at_start.as_deref(), Some(first.as_str()));

            let second = commit_to(&repo, Some(&first));
            assert_ne!(first, second);
            save_one("one", &root, "saved again, HEAD has moved");

            assert_eq!(head_at_start("one").unwrap().as_deref(), Some(first.as_str()), "written once");
            assert_eq!(load("one").unwrap().head_at_start.as_deref(), Some(first.as_str()));
        });
    }

    /// A chat begun before its folder's first commit has no marker, and keeps
    /// having none: write-once means never, not "on the next save".
    #[test]
    fn a_chat_started_before_the_first_commit_never_gets_a_marker() {
        with_app_dir("chat-store-head-unborn", || {
            let dir = crate::testing::temp_dir("chat-store-head-unborn-repo");
            let repo = git2::Repository::init(&dir).unwrap();
            let root = dir.display().to_string();
            save_one("one", &root, "before any commit");
            assert_eq!(head_at_start("one").unwrap(), None);

            commit_to(&repo, None);
            save_one("one", &root, "after the folder's first commit");
            assert_eq!(head_at_start("one").unwrap(), None, "still none: only the first save writes");
        });
    }

    /// TJ-4: the journal rows come back in the order they were written,
    /// and the message count is the last snapshot plus the appends after it.
    #[test]
    fn journal_rows_come_back_in_order_and_count_their_messages() {
        with_app_dir("chat-journal-rows", || {
            save_one("one", "/repo", "go");
            let msg = |text: &str| serde_json::to_value(vec![LlmMessage::user(text)]).unwrap();
            journal_append("one", "t1", "snapshot", &msg("go"), 1).unwrap();
            journal_append("one", "t1", "append", &msg("a"), 1).unwrap();
            journal_append("one", "t1", "blocks", &serde_json::json!({"blocks": [], "todos": [], "plan": null}), 0).unwrap();
            journal_append("one", "t1", "append", &msg("b"), 1).unwrap();
            journal_append("one", "t2", "snapshot", &msg("fresh"), 1).unwrap();

            let entries = journal_entries("one").unwrap();
            assert_eq!(entries.len(), 5);
            assert!(matches!(entries[0], crate::domain::turn_journal::JournalEntry::Snapshot(_)));
            assert!(matches!(entries[2], crate::domain::turn_journal::JournalEntry::Blocks { .. }));
            assert_eq!(journal_message_count("one", "t1").unwrap(), 3);
            assert_eq!(journal_message_count("one", "t2").unwrap(), 1, "another turn's rows are its own");
            // A damaged row is skipped, not fatal.
            open().unwrap().execute("UPDATE turn_journal SET body = '{ not json' WHERE seq = 2", []).unwrap();
            assert_eq!(journal_entries("one").unwrap().len(), 4);
        });
    }

    /// The turn's save folds the journal away in the same transaction; a
    /// move (relocate) is not a turn ending and leaves it. Deleting the
    /// chat takes its journal with it.
    #[test]
    fn a_save_folds_the_journal_and_a_move_does_not() {
        with_app_dir("chat-journal-fold", || {
            save_one("one", "/repo", "go");
            let msg = serde_json::to_value(vec![LlmMessage::user("go")]).unwrap();
            journal_append("one", "t1", "snapshot", &msg, 1).unwrap();

            relocate("one", "/repo-wt", "main").unwrap();
            assert_eq!(journal_entries("one").unwrap().len(), 1, "a move leaves the journal");

            save_one("one", "/repo-wt", "done");
            assert!(journal_entries("one").unwrap().is_empty(), "the save folded it");

            journal_append("one", "t2", "snapshot", &msg, 1).unwrap();
            delete("one").unwrap();
            assert!(journal_entries("one").unwrap().is_empty(), "a deleted chat's journal goes with it");
        });
    }

    /// A turn's save folds only its own rows: the next turn's base, already
    /// written, is not this turn's to delete.
    #[test]
    fn a_turns_save_folds_only_its_own_journal() {
        with_app_dir("chat-journal-turn-fold", || {
            save_one("one", "/repo", "go");
            let msg = serde_json::to_value(vec![LlmMessage::user("go")]).unwrap();
            journal_append("one", "t1", "snapshot", &msg, 1).unwrap();
            journal_append("one", "t2", "snapshot", &msg, 1).unwrap();
            save_turn("one", "/repo", &[LlmMessage::user("done")], &serde_json::json!([]), &[], None, None, None, Some("t1")).unwrap();
            assert_eq!(journal_message_count("one", "t1").unwrap(), 0);
            assert_eq!(journal_message_count("one", "t2").unwrap(), 1, "the next turn's base survives");
            // Without a turn, every row goes — as a recovery folds them.
            save_turn("one", "/repo", &[LlmMessage::user("done")], &serde_json::json!([]), &[], None, None, None, None).unwrap();
            assert!(journal_entries("one").unwrap().is_empty());
        });
    }

    /// A row an older build wrote — no column value, no key in the body —
    /// opens as a chat without a marker and is saved again without gaining
    /// one.
    #[test]
    fn a_chat_saved_before_the_marker_keeps_having_none() {
        with_app_dir("chat-store-head-old-row", || {
            let dir = crate::testing::temp_dir("chat-store-head-old-repo");
            let repo = git2::Repository::init(&dir).unwrap();
            commit_to(&repo, None);
            let root = dir.display().to_string();
            save_one("old", &root, "written by an older build");
            // Mimic that older build: the column did not exist and the body
            // has no headAtStart key.
            open().unwrap().execute("UPDATE chats SET head_at_start = NULL WHERE id = 'old'", []).unwrap();
            let mut value: Value = serde_json::from_str(&body("old")).unwrap();
            value.as_object_mut().unwrap().remove("headAtStart");
            open()
                .unwrap()
                .execute("UPDATE chats SET body = ?1 WHERE id = 'old'", params![value.to_string()])
                .unwrap();

            save_one("old", &root, "saved again by this build");

            assert_eq!(head_at_start("old").unwrap(), None);
            assert!(!body("old").contains("headAtStart"), "the body stays without the key");
        });
    }

    fn chat_pin(provider: &str, model: Option<&str>) -> ChatModel {
        ChatModel { provider: provider.to_string(), model: model.map(str::to_string) }
    }

    /// CM-1: the pin is the row's, not the body's. Every writer that saves
    /// the conversation whole leaves it as it was — an agent's save, a move
    /// to a worktree — and none of them puts it into the body.
    #[test]
    fn a_pin_survives_every_save_and_never_enters_the_body() {
        with_app_dir("chat-store-model-saves", || {
            save_one("one", "/repo", "first");
            set_model("one", &chat_pin("deepseek", Some("deepseek-chat"))).unwrap();

            save_one("one", "/repo", "second");
            assert_eq!(model("one").unwrap(), Some(chat_pin("deepseek", Some("deepseek-chat"))));
            assert_eq!(load("one").unwrap().model, Some(chat_pin("deepseek", Some("deepseek-chat"))), "the window is told");

            relocate("one", "/repo-wt", "main").unwrap();
            assert_eq!(model("one").unwrap(), Some(chat_pin("deepseek", Some("deepseek-chat"))), "a move keeps it");
            let value: Value = serde_json::from_str(&body("one")).unwrap();
            assert!(value.get("model").is_none(), "the body stays without the pin: {value}");
        });
    }

    /// Chat mode's conversations are rows of the same table: a save of one
    /// keeps its pin the same way.
    #[test]
    fn a_plain_chat_keeps_its_model_too() {
        with_app_dir("chat-store-model-plain", || {
            let said = [LlmMessage::user("what is a monad?")];
            save_plain("plain", ChatRole::Assistant, None, &said, &blocks("what is a monad?")).unwrap();
            set_model("plain", &chat_pin("alfagen", None)).unwrap();

            save_plain("plain", ChatRole::Assistant, None, &said, &blocks("what is a monad?")).unwrap();

            assert_eq!(model("plain").unwrap(), Some(chat_pin("alfagen", None)), "auto is a pin, not an absence");
            assert_eq!(load("plain").unwrap().model, Some(chat_pin("alfagen", None)));
        });
    }

    /// A pick is not something said in the chat: the transcript and
    /// `updated_at` are not touched, so the sidebar's order does not move.
    #[test]
    fn picking_a_model_does_not_move_the_chat_in_the_list() {
        with_app_dir("chat-store-model-quiet", || {
            save_one("one", "/repo", "first");
            // A moment old enough that any rewrite would show.
            open().unwrap().execute("UPDATE chats SET updated_at = 1 WHERE id = 'one'", []).unwrap();
            let before = body("one");

            set_model("one", &chat_pin("p", Some("m"))).unwrap();

            let at: i64 = open().unwrap().query_row("SELECT updated_at FROM chats WHERE id = 'one'", [], |r| r.get(0)).unwrap();
            assert_eq!(at, 1, "updated_at stands");
            assert_eq!(body("one"), before, "the transcript stands");
        });
    }

    /// A database written before chat models existed gains the columns and
    /// keeps its chats, none of them pinned; the first stamp pins one.
    #[test]
    fn a_database_from_before_chat_models_opens_unpinned() {
        with_app_dir("chat-store-model-upgrade", || {
            let path = app_dir::ensure().unwrap().join(FILE);
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE chats (
                   id TEXT PRIMARY KEY, workspace TEXT NOT NULL, schema_version INTEGER NOT NULL,
                   title TEXT NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
                   branched_from TEXT, body TEXT NOT NULL, archived INTEGER NOT NULL DEFAULT 0);",
            )
            .unwrap();
            let old_body = serde_json::json!({
                "schemaVersion": 1, "id": "old", "workspace": "/repo", "title": "old",
                "createdAt": 1, "updatedAt": 1, "messages": [], "blocks": [], "todos": []
            });
            conn.execute(
                "INSERT INTO chats VALUES ('old', '/repo', 1, 'old', 1, 1, NULL, ?1, 0)",
                params![old_body.to_string()],
            )
            .unwrap();
            drop(conn);

            assert_eq!(list("/repo").unwrap().len(), 1);
            assert_eq!(model("old").unwrap(), None);
            assert_eq!(load("old").unwrap().model, None);

            stamp_model_if_absent("old", &chat_pin("p", None)).unwrap();
            assert_eq!(model("old").unwrap(), Some(chat_pin("p", None)));
        });
    }

    /// The first save stamps; a repeated save is a no-op, and a pick made
    /// in the chat is never stamped over.
    #[test]
    fn a_stamp_never_rewrites_a_pin_already_set() {
        with_app_dir("chat-store-model-stamp", || {
            save_one("one", "/repo", "first");
            stamp_model_if_absent("one", &chat_pin("first-provider", None)).unwrap();
            stamp_model_if_absent("one", &chat_pin("second-provider", Some("m"))).unwrap();
            assert_eq!(model("one").unwrap(), Some(chat_pin("first-provider", None)), "the first stamp stands");

            set_model("one", &chat_pin("picked", Some("m"))).unwrap();
            stamp_model_if_absent("one", &chat_pin("third", None)).unwrap();
            assert_eq!(model("one").unwrap(), Some(chat_pin("picked", Some("m"))), "a pick stands too");
        });
    }

    #[test]
    fn a_model_pick_on_a_chat_that_is_not_there_says_so() {
        with_app_dir("chat-store-model-missing", || {
            assert!(matches!(set_model("nope", &chat_pin("p", None)), Err(ChatError::NotFound(_))));
            assert!(matches!(set_model("../x", &chat_pin("p", None)), Err(ChatError::BadId(_))));
            assert!(matches!(model("../x"), Err(ChatError::BadId(_))));
            assert_eq!(model("nope").unwrap(), None);
            // A stamp on a missing row is a no-op, not an error: the first
            // save has already written the row it stamps.
            stamp_model_if_absent("nope", &chat_pin("p", None)).unwrap();
            assert_eq!(model("nope").unwrap(), None);
        });
    }
}
