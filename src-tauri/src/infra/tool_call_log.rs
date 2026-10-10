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
";

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
        "INSERT INTO tool_calls (ts_ms, repo_root, round, provider_id, model, tool, args, status, error, result, duration_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            entry.ts_ms,
            entry.repo_root,
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

fn row(row: &Row) -> rusqlite::Result<ToolCallLogRow> {
    let json = |text: Option<String>| text.and_then(|t| serde_json::from_str(&t).ok());
    let status: String = row.get(8)?;
    Ok(ToolCallLogRow {
        id: row.get(0)?,
        entry: ToolCallLogEntry {
            ts_ms: row.get(1)?,
            repo_root: row.get(2)?,
            round: row.get(3)?,
            provider_id: row.get(4)?,
            model: row.get(5)?,
            tool: row.get(6)?,
            args: json(row.get(7)?).unwrap_or(serde_json::Value::Null),
            status: CallStatus::parse(&status).unwrap_or(CallStatus::Error),
            error: row.get(9)?,
            result: json(row.get(10)?),
            duration_ms: row.get(11)?,
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
        "SELECT id, ts_ms, repo_root, round, provider_id, model, tool, args, status, error, result, duration_ms
         FROM tool_calls {where_sql} ORDER BY ts_ms DESC, id DESC LIMIT {limit} OFFSET {offset}"
    ))?;
    let rows = stmt.query_map(params.as_slice(), row)?.collect::<Result<Vec<_>, _>>()?;
    Ok(ToolCallLogPage { rows, total })
}

/// Every row. Returns how many went.
pub fn clear() -> Result<usize, ToolCallLogError> {
    Ok(open()?.execute("DELETE FROM tool_calls", [])?)
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

    #[test]
    fn clear_empties_the_log() {
        with_app_dir("log-clear", || {
            append(&entry("readFile", CallStatus::Ok, now_ms()));
            assert_eq!(clear().unwrap(), 1);
            assert_eq!(query(&ToolCallLogFilter::default()).unwrap().total, 0);
        });
    }
}
