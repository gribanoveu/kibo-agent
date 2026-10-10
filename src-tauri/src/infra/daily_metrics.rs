//! `<app dir>/metrics.db`: the counters of `domain::metrics`, one row per
//! local day, metric and key, kept forever.
//!
//! A file of its own rather than a table in `tool_calls.db`: that log keeps
//! 30 days, this keeps them all, and a year of one counter is 365 short rows.
//! Best-effort like the tool-call log: a write that fails drops the count,
//! never what was being counted.

use std::path::PathBuf;

use rusqlite::{Connection, params};
use thiserror::Error;

use crate::domain::metrics::{Count, DailyMetric, Metric};
use crate::infra::app_dir;

const FILE: &str = "metrics.db";

const SCHEMA: &str = "
PRAGMA journal_mode = WAL;
PRAGMA busy_timeout = 3000;
CREATE TABLE IF NOT EXISTS daily_metrics (
  day    TEXT NOT NULL,
  metric TEXT NOT NULL,
  key    TEXT NOT NULL,
  value  INTEGER NOT NULL,
  PRIMARY KEY (day, metric, key)
) WITHOUT ROWID;
";

#[derive(Debug, Error)]
pub enum DailyMetricsError {
    #[error("{0}")]
    AppDir(String),
    #[error("metrics: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

fn path() -> Result<PathBuf, DailyMetricsError> {
    Ok(app_dir::ensure().map_err(DailyMetricsError::AppDir)?.join(FILE))
}

fn open() -> Result<Connection, DailyMetricsError> {
    let conn = Connection::open(path()?)?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

/// Adds to today's counters — the user's local day, as the heatmap draws it.
/// Best-effort: see the module comment.
pub fn record(counts: &[Count]) {
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let _ = add(&today, counts);
}

/// In one transaction, so a day never holds half of one round.
fn add(day: &str, counts: &[Count]) -> Result<(), DailyMetricsError> {
    let mut conn = open()?;
    let tx = conn.transaction()?;
    for (metric, key, value) in counts {
        tx.execute(
            "INSERT INTO daily_metrics (day, metric, key, value) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(day, metric, key) DO UPDATE SET value = value + excluded.value",
            params![day, metric.as_str(), key, i64::try_from(*value).unwrap_or(i64::MAX)],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Every day `metrics` counted anything, oldest first. All of them: the
/// range the user picks is cut in the window.
pub fn read(metrics: &[Metric]) -> Result<Vec<DailyMetric>, DailyMetricsError> {
    let conn = open()?;
    let mut statement = conn.prepare("SELECT day, metric, key, value FROM daily_metrics ORDER BY day, metric, key")?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, i64>(3)?))
    })?;
    let mut found = Vec::new();
    for row in rows {
        let (day, name, key, value) = row?;
        // A name this build does not know was written by a newer one.
        if let Some(metric) = Metric::parse(&name).filter(|m| metrics.contains(m)) {
            found.push(DailyMetric { day, metric, key, value: u64::try_from(value).unwrap_or(0) });
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::with_app_dir;

    fn row(day: &str, metric: Metric, key: &str, value: u64) -> DailyMetric {
        DailyMetric { day: day.into(), metric, key: key.into(), value }
    }

    #[test]
    fn counts_of_one_day_and_key_add_up_and_days_come_back_oldest_first() {
        with_app_dir("metrics-days", || {
            add("2026-10-09", &[(Metric::PromptTokens, "", 1000), (Metric::ModelTokens, "qwen", 50)]).unwrap();
            add("2026-10-08", &[(Metric::PromptTokens, "", 10)]).unwrap();
            add("2026-10-09", &[(Metric::PromptTokens, "", 1200), (Metric::ModelTokens, "qwen", 70)]).unwrap();
            add("2026-10-09", &[(Metric::ModelTokens, "gpt", 5)]).unwrap();

            assert_eq!(
                read(&Metric::ALL).unwrap(),
                [
                    row("2026-10-08", Metric::PromptTokens, "", 10),
                    row("2026-10-09", Metric::ModelTokens, "gpt", 5),
                    row("2026-10-09", Metric::ModelTokens, "qwen", 120),
                    row("2026-10-09", Metric::PromptTokens, "", 2200),
                ]
            );
        });
    }

    #[test]
    fn only_the_metrics_asked_for_come_back_and_unknown_names_are_skipped() {
        with_app_dir("metrics-filter", || {
            add("2026-10-09", &[(Metric::PromptTokens, "", 5), (Metric::CachedTokens, "", 3)]).unwrap();
            open()
                .unwrap()
                .execute("INSERT INTO daily_metrics VALUES ('2026-10-09', 'fromTheFuture', '', 7)", [])
                .unwrap();

            assert_eq!(read(&[Metric::CachedTokens]).unwrap(), [row("2026-10-09", Metric::CachedTokens, "", 3)]);
        });
    }

    #[test]
    fn record_counts_against_today() {
        with_app_dir("metrics-today", || {
            record(&[(Metric::ToolCalls, "readFile", 1)]);
            let today = chrono::Local::now().format("%Y-%m-%d").to_string();
            assert_eq!(read(&Metric::ALL).unwrap(), [row(&today, Metric::ToolCalls, "readFile", 1)]);
        });
    }
}
