//! `<app dir>/tool_calls.db`: every tool call of every chat, already
//! redacted, kept for [`RETENTION_DAYS`].
//!
//! Ported from Alfa Atlas `infra/tool_call_log.rs`. Same contract: a write is
//! best-effort — a full disk or a locked file drops the row, never the call
//! that produced it — and retention is trimmed on write, so no background job.
//! A connection per call: one insert per tool call is not a hot path.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, Row, ToSql, params};
use thiserror::Error;

use crate::domain::request_marker::RequestRecord;
use crate::domain::tool_call_log::{CallStatus, ToolCallLogEntry, ToolCallLogFilter, ToolCallLogPage, ToolCallLogRow};
use crate::infra::app_dir;

const FILE: &str = "tool_calls.db";
pub const RETENTION_DAYS: i64 = 30;
const DAY_MS: i64 = 24 * 60 * 60 * 1000;
const DEFAULT_PAGE: i64 = 200;
const MAX_PAGE: i64 = 1000;

const SCHEMA: &str = "
PRAGMA journal_mode = WAL;
PRAGMA busy_timeout = 3000;
CREATE TABLE IF NOT EXISTS tool_calls (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  ts_ms       INTEGER NOT NULL,
  repo_root   TEXT NOT NULL,
  chat_id     TEXT NOT NULL DEFAULT '',
  turn_id     TEXT NOT NULL DEFAULT '',
  round       INTEGER NOT NULL,
  provider_id TEXT NOT NULL,
  model       TEXT NOT NULL,
  tool        TEXT NOT NULL,
  args        TEXT NOT NULL,
  status      TEXT NOT NULL,
  error       TEXT,
  result      TEXT,
  duration_ms INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS tool_calls_ts ON tool_calls(ts_ms DESC);
CREATE TABLE IF NOT EXISTS requests (
  id                INTEGER PRIMARY KEY AUTOINCREMENT,
  ts_ms             INTEGER NOT NULL,
  chat_id           TEXT NOT NULL,
  turn_id           TEXT NOT NULL,
  round             INTEGER NOT NULL,
  provider_id       TEXT NOT NULL,
  model             TEXT NOT NULL,
  prompt_tokens     INTEGER,
  cached_tokens     INTEGER,
  completion_tokens INTEGER,
  system_hash       TEXT NOT NULL,
  tools_hash        TEXT NOT NULL,
  message_hashes    TEXT NOT NULL,
  cause             TEXT
);
CREATE INDEX IF NOT EXISTS requests_ts ON requests(ts_ms DESC);
";

/// A database written before the chat and turn were kept: the columns are
/// added once, empty for the rows already there. `CREATE TABLE IF NOT EXISTS`
/// leaves an existing table as it is, so the migration is its own step.
fn migrate(conn: &Connection) -> Result<(), ToolCallLogError> {
    let mut stmt = conn.prepare("PRAGMA table_info(tool_calls)")?;
    let columns: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<_, _>>()?;
    // Each column on its own: a crash between two ALTERs leaves one added,
    // and the next open must still add the other. The names are constants.
    for column in ["chat_id", "turn_id"] {
        if !columns.iter().any(|c| c == column) {
            conn.execute_batch(&format!("ALTER TABLE tool_calls ADD COLUMN {column} TEXT NOT NULL DEFAULT '';"))?;
        }
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum ToolCallLogError {
    #[error("{0}")]
    AppDir(String),
    #[error("tool call log: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

fn path() -> Result<PathBuf, ToolCallLogError> {
    Ok(app_dir::ensure().map_err(ToolCallLogError::AppDir)?.join(FILE))
}

fn open() -> Result<Connection, ToolCallLogError> {
    let conn = Connection::open(path()?)?;
    conn.execute_batch(SCHEMA)?;
    migrate(&conn)?;
    Ok(conn)
}

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// Best-effort: a row that cannot be written is dropped. The call it
/// describes has already happened, and losing the audit line is better than
/// failing a turn over it.
pub fn append(entry: &ToolCallLogEntry) {
    let _ = try_append(entry);
}

/// What a turn writes through: `append`, or nothing when the user switched
/// the log off. The switch is read once, here, so a turn logs all its calls
/// or none of them. Unreadable settings leave it on — the default, and a log
/// with no file content in it has nothing to be careful about.
///
/// The call is counted for the Usage pane either way: a tool's name and a
/// count are not what the switch is for.
pub fn recorder() -> impl Fn(&ToolCallLogEntry) {
    let enabled = crate::infra::settings_store::load().map_or(true, |s| s.tool_log.enabled);
    move |entry| {
        crate::infra::daily_metrics::record(&[(crate::domain::metrics::Metric::ToolCalls, &entry.tool, 1)]);
        if enabled {
            append(entry)
        }
    }
}

fn try_append(entry: &ToolCallLogEntry) -> Result<(), ToolCallLogError> {
    let conn = open()?;
    conn.execute(
        "INSERT INTO tool_calls (ts_ms, repo_root, chat_id, turn_id, round, provider_id, model, tool, args, status, error, result, duration_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            entry.ts_ms,
            entry.repo_root,
            entry.chat_id,
            entry.turn_id,
            entry.round,
            entry.provider_id,
            entry.model,
            entry.tool,
            entry.args.to_string(),
            entry.status.as_str(),
            entry.error,
            entry.result.as_ref().map(|v| v.to_string()),
            entry.duration_ms,
        ],
    )?;
    conn.execute("DELETE FROM tool_calls WHERE ts_ms < ?1", params![now_ms() - RETENTION_DAYS * DAY_MS])?;
    Ok(())
}

/// One request's marker: hashes and numbers only, never content. Written
/// for every request of a folder turn — the analysis of a miss is worth a
/// row, and a row with no content in it has nothing to be careful about
/// (`docs/57-turn-journal.md`, TJ-3). Best-effort, as the call log is.
pub fn append_request(chat_id: &str, turn_id: &str, record: &RequestRecord) {
    let _ = try_append_request(chat_id, turn_id, record);
}

fn try_append_request(chat_id: &str, turn_id: &str, record: &RequestRecord) -> Result<(), ToolCallLogError> {
    let conn = open()?;
    let usage = record.usage;
    conn.execute(
        "INSERT INTO requests (ts_ms, chat_id, turn_id, round, provider_id, model, prompt_tokens, cached_tokens, completion_tokens, system_hash, tools_hash, message_hashes, cause)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            now_ms(),
            chat_id,
            turn_id,
            record.round,
            record.provider_id,
            record.model,
            usage.map(|u| u.prompt_tokens),
            usage.map(|u| u.cached_tokens),
            usage.map(|u| u.completion_tokens),
            record.fingerprint.system,
            record.fingerprint.tools,
            serde_json::to_string(&record.fingerprint.messages).unwrap_or_default(),
            record.cause.map(|c| serde_json::to_string(&c).unwrap_or_default()),
        ],
    )?;
    conn.execute("DELETE FROM requests WHERE ts_ms < ?1", params![now_ms() - RETENTION_DAYS * DAY_MS])?;
    Ok(())
}

/// The chat's last request that reported its cost, as the meter's anchor
/// after a reopen or a restart (docs/60): hashes and a number from the marker
/// row, never content. `None` when there is no such row, or its hashes cannot
/// be read — an unreadable fingerprint must not pass as an empty prefix, which
/// would compare Unchanged and double the count.
pub fn last_anchor(chat_id: &str) -> Option<crate::domain::request_marker::Anchor> {
    let conn = open().ok()?;
    let (prompt_tokens, provider_id, model, system, tools, hashes) = conn
        .query_row(
            "SELECT prompt_tokens, provider_id, model, system_hash, tools_hash, message_hashes FROM requests
             WHERE chat_id = ?1 AND prompt_tokens IS NOT NULL AND prompt_tokens > 0 ORDER BY ts_ms DESC, id DESC LIMIT 1",
            params![chat_id],
            |r| {
                Ok((
                    r.get::<_, u32>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                ))
            },
        )
        .ok()?;
    Some(crate::domain::request_marker::Anchor {
        prompt_tokens,
        provider_id,
        model,
        fingerprint: crate::domain::request_marker::RequestFingerprint {
            system,
            tools,
            messages: serde_json::from_str(&hashes).ok()?,
        },
    })
}

fn row(row: &Row) -> rusqlite::Result<ToolCallLogRow> {
    let json = |text: Option<String>| text.and_then(|t| serde_json::from_str(&t).ok());
    let status: String = row.get(10)?;
    Ok(ToolCallLogRow {
        id: row.get(0)?,
        entry: ToolCallLogEntry {
            ts_ms: row.get(1)?,
            repo_root: row.get(2)?,
            chat_id: row.get(3)?,
            turn_id: row.get(4)?,
            round: row.get(5)?,
            provider_id: row.get(6)?,
            model: row.get(7)?,
            tool: row.get(8)?,
            args: json(row.get(9)?).unwrap_or(serde_json::Value::Null),
            status: CallStatus::parse(&status).unwrap_or(CallStatus::Error),
            error: row.get(11)?,
            result: json(row.get(12)?),
            duration_ms: row.get(13)?,
        },
    })
}

/// Newest first. A missing database is an empty log.
pub fn query(filter: &ToolCallLogFilter) -> Result<ToolCallLogPage, ToolCallLogError> {
    let conn = open()?;
    let mut clauses = Vec::new();
    let mut values: Vec<Box<dyn ToSql>> = Vec::new();
    if let Some(root) = &filter.repo_root {
        clauses.push("repo_root = ?");
        values.push(Box::new(root.clone()));
    }
    if let Some(tool) = &filter.tool {
        clauses.push("tool = ?");
        values.push(Box::new(tool.clone()));
    }
    if let Some(status) = filter.status {
        clauses.push("status = ?");
        values.push(Box::new(status.as_str()));
    }
    if let Some(search) = filter.search.as_deref().filter(|s| !s.trim().is_empty()) {
        // `LIKE` is case-insensitive for ASCII only; the paths and tool
        // names this is for are ASCII in practice.
        clauses.push("(tool LIKE ? ESCAPE '\\' OR error LIKE ? ESCAPE '\\' OR args LIKE ? ESCAPE '\\')");
        let escaped = search.trim().replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
        for _ in 0..3 {
            values.push(Box::new(format!("%{escaped}%")));
        }
    }
    let where_sql = if clauses.is_empty() { String::new() } else { format!("WHERE {}", clauses.join(" AND ")) };
    let params: Vec<&dyn ToSql> = values.iter().map(|v| v.as_ref()).collect();

    let total = conn.query_row(&format!("SELECT COUNT(*) FROM tool_calls {where_sql}"), params.as_slice(), |r| r.get(0))?;
    let limit = filter.limit.unwrap_or(DEFAULT_PAGE).clamp(1, MAX_PAGE);
    let offset = filter.offset.unwrap_or(0).max(0);
    // `id` breaks ties: two calls of one round often share a millisecond.
    let mut stmt = conn.prepare(&format!(
        "SELECT id, ts_ms, repo_root, chat_id, turn_id, round, provider_id, model, tool, args, status, error, result, duration_ms
         FROM tool_calls {where_sql} ORDER BY ts_ms DESC, id DESC LIMIT {limit} OFFSET {offset}"
    ))?;
    let rows = stmt.query_map(params.as_slice(), row)?.collect::<Result<Vec<_>, _>>()?;
    Ok(ToolCallLogPage { rows, total })
}

/// Every row of both tables — the calls and the request markers are one
/// log to whoever presses "Clear log". Returns how many calls went.
pub fn clear() -> Result<usize, ToolCallLogError> {
    let conn = open()?;
    conn.execute("DELETE FROM requests", [])?;
    Ok(conn.execute("DELETE FROM tool_calls", [])?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::with_app_dir;
    use serde_json::json;

    fn entry(tool: &str, status: CallStatus, ts_ms: i64) -> ToolCallLogEntry {
        ToolCallLogEntry {
            ts_ms,
            repo_root: "/repo".into(),
            chat_id: "c1".into(),
            turn_id: "c1/turn-1".into(),
            round: 1,
            provider_id: "local".into(),
            model: "m".into(),
            tool: tool.into(),
            args: json!({"tool": tool, "args": {"path": "src/100%_done.rs"}}),
            status,
            error: (status == CallStatus::Error).then(|| "not found: src/x.rs".to_string()),
            result: (status == CallStatus::Ok).then(|| json!({"result": "file"})),
            duration_ms: 4,
        }
    }

    fn tools(page: &ToolCallLogPage) -> Vec<&str> {
        page.rows.iter().map(|r| r.entry.tool.as_str()).collect()
    }

    /// Counted for the Usage pane even with the log off: the switch is
    /// about what a call carried, not that it happened.
    #[test]
    fn a_call_is_counted_by_tool_whether_or_not_it_is_logged() {
        with_app_dir("log-counted", || {
            let mut settings = crate::infra::settings_store::load().unwrap();
            settings.tool_log.enabled = false;
            crate::infra::settings_store::save(&settings).unwrap();

            let record = recorder();
            record(&entry("readFile", CallStatus::Ok, now_ms()));
            record(&entry("readFile", CallStatus::Error, now_ms()));

            assert_eq!(query(&ToolCallLogFilter::default()).unwrap().total, 0);
            let calls = crate::infra::daily_metrics::read(&[crate::domain::metrics::Metric::ToolCalls]).unwrap();
            assert_eq!(calls.iter().map(|r| (r.key.as_str(), r.value)).collect::<Vec<_>>(), [("readFile", 2)]);
        });
    }

    #[test]
    fn an_entry_comes_back_as_it_went_in_newest_first() {
        with_app_dir("log-round-trip", || {
            let now = now_ms();
            let first = entry("readFile", CallStatus::Ok, now - 10);
            append(&first);
            append(&entry("grep", CallStatus::Error, now));

            let page = query(&ToolCallLogFilter::default()).unwrap();
            assert_eq!(page.total, 2);
            assert_eq!(tools(&page), ["grep", "readFile"]);
            assert_eq!(page.rows[1].entry, first);
        });
    }

    #[test]
    fn filters_narrow_and_combine() {
        with_app_dir("log-filter", || {
            let now = now_ms();
            append(&entry("readFile", CallStatus::Ok, now));
            append(&entry("grep", CallStatus::Error, now));
            append(&entry("editFile", CallStatus::Denied, now));
            append(&ToolCallLogEntry { repo_root: "/other".into(), ..entry("grep", CallStatus::Ok, now) });

            let q = |f: ToolCallLogFilter| tools(&query(&f).unwrap()).into_iter().map(String::from).collect::<Vec<_>>();
            assert_eq!(q(ToolCallLogFilter { status: Some(CallStatus::Denied), ..Default::default() }), ["editFile"]);
            assert_eq!(q(ToolCallLogFilter { tool: Some("grep".into()), ..Default::default() }).len(), 2);
            assert_eq!(
                q(ToolCallLogFilter { tool: Some("grep".into()), repo_root: Some("/repo".into()), ..Default::default() }),
                ["grep"]
            );
            // Search reaches the error and the arguments, and `%`/`_` in it are literal.
            assert_eq!(q(ToolCallLogFilter { search: Some("SRC/X".into()), ..Default::default() }), ["grep"]);
            assert_eq!(q(ToolCallLogFilter { search: Some("100%_d".into()), ..Default::default() }).len(), 4);
            assert!(q(ToolCallLogFilter { search: Some("100_%".into()), ..Default::default() }).is_empty());
            assert!(q(ToolCallLogFilter { search: Some("100%rs".into()), ..Default::default() }).is_empty());
        });
    }

    #[test]
    fn a_page_is_a_window_and_the_total_is_not() {
        with_app_dir("log-page", || {
            let now = now_ms();
            for i in 0..5 {
                append(&entry(&format!("t{i}"), CallStatus::Ok, now + i));
            }
            let page = query(&ToolCallLogFilter { limit: Some(2), offset: Some(1), ..Default::default() }).unwrap();
            assert_eq!(page.total, 5);
            assert_eq!(tools(&page), ["t3", "t2"]);
        });
    }

    #[test]
    fn rows_past_retention_go_on_the_next_write() {
        with_app_dir("log-retention", || {
            let now = now_ms();
            append(&entry("old", CallStatus::Ok, now - RETENTION_DAYS * DAY_MS - 1));
            append(&entry("new", CallStatus::Ok, now));
            assert_eq!(tools(&query(&ToolCallLogFilter::default()).unwrap()), ["new"]);
        });
    }

    #[test]
    fn a_switched_off_log_records_nothing() {
        with_app_dir("log-off", || {
            let mut settings = crate::infra::settings_store::load().unwrap();
            settings.tool_log.enabled = false;
            crate::infra::settings_store::save(&settings).unwrap();
            recorder()(&entry("readFile", CallStatus::Ok, now_ms()));
            assert_eq!(query(&ToolCallLogFilter::default()).unwrap().total, 0);

            settings.tool_log.enabled = true;
            crate::infra::settings_store::save(&settings).unwrap();
            recorder()(&entry("readFile", CallStatus::Ok, now_ms()));
            assert_eq!(query(&ToolCallLogFilter::default()).unwrap().total, 1);
        });
    }

    /// TJ-3: a request's marker comes back as it went in — hashes and
    /// numbers, the cause as its JSON kind. The first request has neither
    /// usage nor cause, and both stay null.
    #[test]
    fn a_request_marker_comes_back_as_it_went_in() {
        with_app_dir("log-request", || {
            let record = RequestRecord {
                round: 2,
                provider_id: "p".into(),
                model: "m".into(),
                usage: Some(crate::domain::llm::ChatUsage { prompt_tokens: 1000, completion_tokens: 5, total_tokens: 1005, cached_tokens: 900 }),
                fingerprint: crate::domain::request_marker::RequestFingerprint {
                    system: "s".into(),
                    tools: "t".into(),
                    messages: vec!["a".into(), "b".into()],
                },
                cause: Some(crate::domain::request_marker::MissCause::History { from: 1, of: 2 }),
            };
            append_request("c1", "c1/turn-1", &record);
            append_request("c1", "c1/turn-2", &RequestRecord { round: 1, usage: None, cause: None, ..record.clone() });

            let conn = open().unwrap();
            let mut stmt = conn
                .prepare("SELECT chat_id, turn_id, round, prompt_tokens, cached_tokens, completion_tokens, system_hash, tools_hash, message_hashes, cause FROM requests ORDER BY id")
                .unwrap();
            let rows: Vec<(String, String, u32, Option<u32>, Option<u32>, Option<u32>, String, String, String, Option<String>)> = stmt
                .query_map([], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?))
                })
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            assert_eq!(rows.len(), 2);
            assert_eq!(rows[0].0, "c1");
            assert_eq!(rows[0].1, "c1/turn-1");
            assert_eq!(rows[0].2, 2);
            assert_eq!(rows[0].3, Some(1000));
            assert_eq!(rows[0].4, Some(900));
            assert_eq!(rows[0].5, Some(5));
            assert_eq!(rows[0].6, "s");
            assert_eq!(rows[0].7, "t");
            assert_eq!(rows[0].8, r#"["a","b"]"#);
            assert_eq!(rows[0].9.as_deref(), Some(r#"{"kind":"history","from":1,"of":2}"#));
            assert_eq!(rows[1].3, None);
            assert_eq!(rows[1].9, None);
        });
    }

    #[test]
    fn request_markers_past_retention_go_on_the_next_write() {
        with_app_dir("log-request-retention", || {
            let record = RequestRecord {
                round: 1,
                provider_id: "p".into(),
                model: "m".into(),
                usage: None,
                fingerprint: crate::domain::request_marker::RequestFingerprint { system: "s".into(), tools: "t".into(), messages: vec![] },
                cause: None,
            };
            append_request("c1", "t1", &record);
            let conn = open().unwrap();
            conn.execute("UPDATE requests SET ts_ms = ?1", params![now_ms() - RETENTION_DAYS * DAY_MS - 1]).unwrap();
            drop(conn);
            append_request("c1", "t2", &record);
            let conn = open().unwrap();
            let left: i64 = conn.query_row("SELECT COUNT(*) FROM requests", [], |r| r.get(0)).unwrap();
            assert_eq!(left, 1);
        });
    }

    /// A database written before the chat and turn were kept gains the
    /// columns, empty for the rows already there; the log reads on.
    #[test]
    fn an_old_log_gains_the_chat_and_turn_columns() {
        with_app_dir("log-migrate", || {
            let conn = Connection::open(path().unwrap()).unwrap();
            conn.execute_batch(
                "CREATE TABLE tool_calls (
                  id INTEGER PRIMARY KEY AUTOINCREMENT, ts_ms INTEGER NOT NULL, repo_root TEXT NOT NULL,
                  round INTEGER NOT NULL, provider_id TEXT NOT NULL, model TEXT NOT NULL, tool TEXT NOT NULL,
                  args TEXT NOT NULL, status TEXT NOT NULL, error TEXT, result TEXT, duration_ms INTEGER NOT NULL);",
            )
            .unwrap();
            // A recent row: retention would trim one from 1970 on the next write.
            conn.execute(
                "INSERT INTO tool_calls (ts_ms, repo_root, round, provider_id, model, tool, args, status, duration_ms)
                 VALUES (?1, '/repo', 1, 'p', 'm', 'readFile', '{}', 'ok', 0)",
                params![now_ms() - 10],
            )
            .unwrap();
            drop(conn);

            let page = query(&ToolCallLogFilter::default()).unwrap();
            assert_eq!(page.total, 1);
            assert_eq!(page.rows[0].entry.chat_id, "");
            assert_eq!(page.rows[0].entry.turn_id, "");
            append(&entry("grep", CallStatus::Ok, now_ms()));
            let page = query(&ToolCallLogFilter::default()).unwrap();
            assert_eq!(page.total, 2);
            assert_eq!(page.rows[0].entry.chat_id, "c1");
        });
    }

    /// CU-2: the meter's anchor after a reopen is the last request that
    /// reported its cost — hashes and a number from the row, never content.
    #[test]
    fn the_last_anchor_is_the_last_costed_request() {
        with_app_dir("log-anchor", || {
            let record = |prompt: Option<u32>| RequestRecord {
                round: 1,
                provider_id: "p".into(),
                model: "m".into(),
                usage: prompt.map(|p| crate::domain::llm::ChatUsage { prompt_tokens: p, completion_tokens: 2, total_tokens: p + 2, cached_tokens: 0 }),
                fingerprint: crate::domain::request_marker::RequestFingerprint { system: "s".into(), tools: "t".into(), messages: vec!["a".into(), "b".into()] },
                cause: None,
            };
            assert!(last_anchor("c1").is_none());
            append_request("c1", "t1", &record(Some(500)));
            append_request("c1", "t2", &record(None));
            append_request("c1", "t3", &record(Some(0)));
            let anchor = last_anchor("c1").expect("the costed row anchors");
            assert_eq!(anchor.prompt_tokens, 500);
            assert_eq!(anchor.fingerprint.messages, vec!["a".to_string(), "b".to_string()]);
            assert_eq!((anchor.provider_id.as_str(), anchor.model.as_str()), ("p", "m"));
            assert!(last_anchor("c2").is_none());
            // A row whose hashes cannot be read anchors nothing: an empty
            // message list would compare Unchanged and double the count.
            let conn = open().unwrap();
            conn.execute("UPDATE requests SET message_hashes = 'not json'", []).unwrap();
            drop(conn);
            assert!(last_anchor("c1").is_none());
        });
    }

    #[test]
    fn clear_empties_the_log() {
        with_app_dir("log-clear", || {
            append(&entry("readFile", CallStatus::Ok, now_ms()));
            append_request("c1", "t1", &RequestRecord {
                round: 1,
                provider_id: "p".into(),
                model: "m".into(),
                usage: None,
                fingerprint: crate::domain::request_marker::RequestFingerprint { system: "s".into(), tools: "t".into(), messages: vec![] },
                cause: None,
            });
            assert_eq!(clear().unwrap(), 1);
            assert_eq!(query(&ToolCallLogFilter::default()).unwrap().total, 0);
            let conn = open().unwrap();
            let left: i64 = conn.query_row("SELECT COUNT(*) FROM requests", [], |r| r.get(0)).unwrap();
            assert_eq!(left, 0, "the markers are one log with the calls");
        });
    }

    /// A crash between the two ALTERs leaves one column added: the next
    /// open adds the other, not skips the migration for good.
    #[test]
    fn a_half_migrated_log_gains_the_other_column() {
        with_app_dir("log-half-migrate", || {
            let conn = Connection::open(path().unwrap()).unwrap();
            conn.execute_batch(
                "CREATE TABLE tool_calls (
                  id INTEGER PRIMARY KEY AUTOINCREMENT, ts_ms INTEGER NOT NULL, repo_root TEXT NOT NULL,
                  round INTEGER NOT NULL, provider_id TEXT NOT NULL, model TEXT NOT NULL, tool TEXT NOT NULL,
                  args TEXT NOT NULL, status TEXT NOT NULL, error TEXT, result TEXT, duration_ms INTEGER NOT NULL);
                 ALTER TABLE tool_calls ADD COLUMN chat_id TEXT NOT NULL DEFAULT '';",
            )
            .unwrap();
            drop(conn);
            append(&entry("grep", CallStatus::Ok, now_ms()));
            let page = query(&ToolCallLogFilter::default()).unwrap();
            assert_eq!(page.total, 1);
            assert_eq!(page.rows[0].entry.turn_id, "c1/turn-1");
        });
    }
}
