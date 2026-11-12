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
    self, ChatError, ChatGroup, ChatRecord, ChatSummary, CHAT_SCHEMA_VERSION, NO_FOLDER,
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
    Ok(conn)
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

/// What the chat is called in the list — the model's name for it first —
/// or `None` when there is no such chat: what names a prototype shown before
/// it had a `prototype.json` (`docs/38-artifacts.md`).
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
    git_branch: Option<&str>,
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
    upsert(ChatRecord {
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
    })
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
    upsert(record)
}

/// Writes `record`, stamped now and keeping the moment it was first saved.
fn upsert(mut record: ChatRecord) -> Result<ChatSummary, ChatError> {
    chat_record::check_id(&record.id)?;
    let conn = open()?;
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

    let body = serde_json::to_string(&record).map_err(ChatError::Parse)?;
    // An upsert, not INSERT OR REPLACE: a replaced row would lose `archived`,
    // `pinned`, `group_id` and `generated_title`. A new branch starts with its
    // original's name, as it starts with its first message. `head_at_start`
    // is written only by the INSERT half — the conflict branch deliberately
    // leaves it: a chat's start is its first save's, however often the chat
    // is saved afterwards.
    conn.execute(
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

/// The folders of one workspace, in the order the user arranged them. An
/// empty folder is listed too — otherwise a folder made ahead of its first
/// chat could not be renamed or deleted at all.
pub fn groups(workspace: &str) -> Result<Vec<ChatGroup>, ChatError> {
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

/// Whether another folder of the same workspace already carries the name —
/// case-insensitively, so "Release" and "release" are one folder to the eye.
fn name_taken(conn: &Connection, workspace: &str, name: &str, except_id: Option<&str>) -> Result<bool, ChatError> {
    conn.query_row(
        "SELECT COUNT(*) > 0 FROM chat_groups WHERE workspace = ?1 AND lower(name) = lower(?2) AND id != ?3",
        params![workspace, name, except_id.unwrap_or("")],
        |row| row.get(0),
    )
    .map_err(store)
}

/// Makes a folder at the end of the workspace's list.
pub fn create_group(workspace: &str, name: &str) -> Result<ChatGroup, ChatError> {
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
pub fn rename_group(workspace: &str, id: &str, name: &str) -> Result<(), ChatError> {
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
pub fn delete_group(workspace: &str, id: &str) -> Result<(), ChatError> {
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

/// Moves a chat into a folder of its own workspace, or out to no folder with
/// `None`. The folder has to belong to the same workspace: a chat about one
/// project never lands in another's list.
pub fn set_group(id: &str, group: Option<&str>) -> Result<(), ChatError> {
    chat_record::check_id(id)?;
    let conn = open()?;
    let workspace: Option<String> = conn
        .query_row("SELECT workspace FROM chats WHERE id = ?1", params![id], |row| row.get(0))
        .optional()
        .map_err(store)?;
    let workspace = workspace.ok_or_else(|| ChatError::NotFound(id.to_string()))?;
    if let Some(group) = group {
        let own: bool = conn
            .query_row("SELECT COUNT(*) > 0 FROM chat_groups WHERE id = ?1 AND workspace = ?2", params![group, workspace], |row| row.get(0))
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
                let info = ProcessInfo { id: chat.len() as u32, command: "x".into(), cwd: ".".into(), state: ProcessState::Stopped };
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
            let run = |id| AgentInfo { id, task: "t".into(), state: AgentState::Done, steps: Vec::new(), tokens: Default::default(), answer: None };
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
            let group = create_group("/repo", "release").unwrap();

            set_group("one", Some(&group.id)).unwrap();
            let held = |id: &str| list("/repo").unwrap().into_iter().find(|c| c.id == id).unwrap().group;
            assert_eq!(held("one").as_deref(), Some(group.id.as_str()));

            set_group("one", None).unwrap();
            assert_eq!(held("one"), None);
            assert!(matches!(set_group("nope", Some(&group.id)), Err(ChatError::NotFound(_))));
            assert!(matches!(set_group("../x", Some(&group.id)), Err(ChatError::BadId(_))));
            // The folder belongs to this workspace; another's is not found here.
            let other = create_group("/other", "release").unwrap();
            assert!(matches!(set_group("one", Some(&other.id)), Err(ChatError::NotFound(_))));
        });
    }

    /// Every turn saves the chat whole; a turn in a folder must not quietly
    /// file the chat back under no folder.
    #[test]
    fn saving_a_grouped_chat_keeps_its_folder() {
        with_app_dir("chat-store-group-save", || {
            save_one("one", "/repo", "first");
            let group = create_group("/repo", "release").unwrap();
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
            let group = create_group("/repo", "release").unwrap();
            set_group("one", Some(&group.id)).unwrap();

            delete_group("/repo", &group.id).unwrap();

            assert!(groups("/repo").unwrap().is_empty());
            let mut unfiled: Vec<String> = list("/repo").unwrap().into_iter().map(|c| (c.id, c.group)).filter(|(_, g)| g.is_none()).map(|(id, _)| id).collect();
            unfiled.sort();
            assert_eq!(unfiled, ["one", "two"]);
            assert!(matches!(delete_group("/repo", &group.id), Err(ChatError::NotFound(_))));
            // Another workspace's folder of the same name is not this one.
            let elsewhere = create_group("/other", "release").unwrap();
            assert!(matches!(delete_group("/repo", &elsewhere.id), Err(ChatError::NotFound(_))));
        });
    }

    /// A folder made ahead of its first chat is still a folder: it lists, so
    /// it can be renamed, filled and deleted.
    #[test]
    fn an_empty_folder_is_listed() {
        with_app_dir("chat-store-group-empty", || {
            create_group("/repo", "release").unwrap();
            let listed = groups("/repo").unwrap();
            assert_eq!(listed.len(), 1);
            assert_eq!(listed[0].name, "release");
            assert!(groups("/other").unwrap().is_empty());
        });
    }

    /// The order is the user's, not the alphabet's: positions are handed out
    /// as folders are made and listed back that way.
    #[test]
    fn folders_keep_the_order_they_were_made_in() {
        with_app_dir("chat-store-group-order-folders", || {
            create_group("/repo", "beta").unwrap();
            create_group("/repo", "alpha").unwrap();

            let names: Vec<String> = groups("/repo").unwrap().into_iter().map(|g| g.name).collect();
            assert_eq!(names, ["beta", "alpha"]);
        });
    }

    /// Names are compared trimmed and case-insensitively, per workspace:
    /// "Release" and "release" are one folder to the eye, and the same name
    /// is free in another project.
    #[test]
    fn a_folder_name_is_unique_within_its_workspace() {
        with_app_dir("chat-store-group-names", || {
            let made = create_group("/repo", " Release ").unwrap();
            assert_eq!(made.name, "Release");
            assert!(matches!(create_group("/repo", "release"), Err(ChatError::AlreadyExists(_))));
            let launch = create_group("/repo", "launch").unwrap();
            assert!(matches!(rename_group("/repo", &made.id, "Launch"), Err(ChatError::AlreadyExists(_))));
            rename_group("/repo", &made.id, " ship ").unwrap();
            assert_eq!(groups("/repo").unwrap().iter().find(|g| g.id == made.id).unwrap().name, "ship");
            create_group("/other", "release").unwrap();

            assert!(matches!(create_group("/repo", "   "), Err(ChatError::EmptyName)));
            assert!(matches!(rename_group("/repo", &launch.id, "  "), Err(ChatError::EmptyName)));
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
            let launch = create_group("/repo", "launch").unwrap();
            let release = create_group("/repo", "release").unwrap();
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
            assert!(groups("/repo").unwrap().is_empty());
            let group = create_group("/repo", "release").unwrap();
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
}
