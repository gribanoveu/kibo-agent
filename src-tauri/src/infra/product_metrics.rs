//! `<app dir>/product_metrics.db`: the events of `domain::product_metrics`,
//! waiting to be sent (`docs/67-app-metrics.md`).
//!
//! A file of its own rather than a table in `metrics.db`: the Usage counters
//! are kept forever and belong to that pane, these are a queue with a life of
//! their own and a "Delete data" of their own. Best-effort, like
//! `daily_metrics` — an event that cannot be written is dropped, never the
//! thing it describes.

use std::path::PathBuf;

use rusqlite::{Connection, OptionalExtension, params};
use thiserror::Error;

use crate::domain::product_metrics::{Event, ProductEvent, Stamp};
use crate::infra::{app_dir, platform};

const FILE: &str = "product_metrics.db";

/// How many unsent events the file keeps: past this the oldest are dropped as
/// the new ones are written. An install that never reaches the network would
/// otherwise grow a table of every launch it ever had.
pub const MAX_PENDING: i64 = 5000;

const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const APP_BUILD: &str = if cfg!(debug_assertions) { "debug" } else { "release" };

/// A row, as it is sent: `status` moves from `pending` to `sent` when a
/// delivery takes it, and `attempts`/`last_error` say why one did not.
const SCHEMA: &str = "
PRAGMA journal_mode = WAL;
PRAGMA busy_timeout = 3000;
CREATE TABLE IF NOT EXISTS installation (
  only_row   INTEGER PRIMARY KEY CHECK (only_row = 1),
  id         TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS events (
  seq          INTEGER PRIMARY KEY AUTOINCREMENT,
  id           TEXT NOT NULL UNIQUE,
  installation TEXT NOT NULL,
  occurred_at  TEXT NOT NULL,
  category     TEXT NOT NULL,
  name         TEXT NOT NULL,
  label        TEXT NOT NULL,
  event_type   TEXT NOT NULL,
  dimensions   TEXT NOT NULL,
  os_name      TEXT NOT NULL,
  os_version   TEXT NOT NULL,
  os_arch      TEXT NOT NULL,
  app_version  TEXT NOT NULL,
  app_build    TEXT NOT NULL,
  status       TEXT NOT NULL DEFAULT 'pending',
  attempts     INTEGER NOT NULL DEFAULT 0,
  last_error   TEXT,
  sent_at      TEXT
);
CREATE INDEX IF NOT EXISTS events_pending ON events (status, seq);
";

#[derive(Debug, Error)]
pub enum ProductMetricsError {
    #[error("{0}")]
    AppDir(String),
    /// The row was just written to a connection of this process's own.
    #[error("product metrics: the installation row did not stay")]
    NoInstallation,
    #[error("product metrics: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

fn path() -> Result<PathBuf, ProductMetricsError> {
    Ok(app_dir::ensure().map_err(ProductMetricsError::AppDir)?.join(FILE))
}

fn open() -> Result<Connection, ProductMetricsError> {
    let conn = Connection::open(path()?)?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

/// Records one event. Best-effort: the event describes something that has
/// already happened, and a full disk must not fail it.
pub fn record(event: ProductEvent) {
    let _ = try_record(event);
}

fn try_record(event: ProductEvent) -> Result<(), ProductMetricsError> {
    let conn = open()?;
    // Before the row: the installation is made by the first write, so a
    // journal holds an id whenever it holds anything at all, and every row
    // carries it.
    let installation = installation_id(&conn)?;
    append(&conn, &installation, event, MAX_PENDING)
}

/// One event in, the queue cut back to `keep` unsent rows.
fn append(conn: &Connection, installation: &str, event: ProductEvent, keep: i64) -> Result<(), ProductMetricsError> {
    insert(conn, installation, event)?;
    prune_to(conn, keep)
}

/// The row's `id.installation`: made on the first write and kept for the life
/// of the file. Deleting the file makes a new installation — which is what
/// deleting data is supposed to mean.
///
/// The table holds one row by construction (`only_row = 1`), so two first
/// writes at once — a launch and a turn, each on its own connection — make one
/// installation and both read the same id back.
pub fn installation_id(conn: &Connection) -> Result<String, ProductMetricsError> {
    match saved_installation(conn)? {
        Some(id) => Ok(id),
        None => create_installation(conn),
    }
}

/// The row itself, for the two first writes that both found none: `OR IGNORE`
/// on the one-row key, so the loser of the race reads the winner's id back
/// instead of failing above the journal it was about to write.
fn create_installation(conn: &Connection) -> Result<String, ProductMetricsError> {
    conn.execute(
        "INSERT OR IGNORE INTO installation (only_row, id, created_at) VALUES (1, ?1, ?2)",
        params![uuid::Uuid::new_v4().to_string(), now()],
    )?;
    saved_installation(conn)?.ok_or(ProductMetricsError::NoInstallation)
}

/// The one row: the schema keeps it the only one, so it is read without a
/// predicate — and a file written by a build before that key is still read.
fn saved_installation(conn: &Connection) -> Result<Option<String>, ProductMetricsError> {
    Ok(conn.query_row("SELECT id FROM installation", [], |row| row.get(0)).optional()?)
}

/// What a row carries besides its event: this process's own identity and the
/// facts of this run, read once.
fn stamp() -> Stamp {
    let facts = platform::facts();
    Stamp {
        id: uuid::Uuid::new_v4().to_string(),
        occurred_at: now(),
        os_name: facts.os_name.clone(),
        os_version: facts.os_version.clone(),
        os_arch: facts.os_arch.clone(),
        app_version: APP_VERSION,
        app_build: APP_BUILD,
    }
}

/// UTC with milliseconds — sortable as text, and the table's own `timestamp`
/// format. `Z` rather than an offset: one spelling of a moment on every machine.
fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn insert(conn: &Connection, installation: &str, event: ProductEvent) -> Result<(), ProductMetricsError> {
    let row = Event::of(event, stamp());
    // `OR IGNORE` on the row's own id: a write repeated with the same id —
    // a retry, a test — is the same event, not a second one.
    conn.execute(
        "INSERT OR IGNORE INTO events
           (id, installation, occurred_at, category, name, label, event_type, dimensions,
            os_name, os_version, os_arch, app_version, app_build)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            row.id,
            installation,
            row.occurred_at,
            row.category,
            row.name,
            row.label,
            row.event_type,
            serde_json::to_string(&row.dimensions).unwrap_or_else(|_| "[]".to_string()),
            row.os_name,
            row.os_version,
            row.os_arch,
            row.app_version,
            row.app_build,
        ],
    )?;
    Ok(())
}

/// Keeps the newest `keep` unsent events and drops the rest. Sent rows are
/// left to whatever cleans them up: this is the queue's ceiling, not the
/// journal's retention.
fn prune_to(conn: &Connection, keep: i64) -> Result<(), ProductMetricsError> {
    conn.execute(
        "DELETE FROM events WHERE seq IN (
           SELECT seq FROM events WHERE status = 'pending' ORDER BY seq DESC LIMIT -1 OFFSET ?1)",
        params![keep],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::with_app_dir;

    struct Row {
        seq: i64,
        id: String,
        installation: String,
        occurred_at: String,
        category: String,
        name: String,
        label: String,
        event_type: String,
        dimensions: String,
        os_name: String,
        os_version: String,
        os_arch: String,
        app_version: String,
        app_build: String,
        status: String,
        attempts: i64,
        sent_at: Option<String>,
    }

    /// Every row, oldest first.
    fn rows() -> Vec<Row> {
        let conn = open().expect("the journal opens");
        let mut statement = conn
            .prepare(
                "SELECT seq, id, installation, occurred_at, category, name, label, event_type, dimensions,
                        os_name, os_version, os_arch, app_version, app_build, status, attempts, sent_at
                 FROM events ORDER BY seq",
            )
            .expect("prepares");
        let rows = statement
            .query_map([], |row| {
                Ok(Row {
                    seq: row.get(0)?,
                    id: row.get(1)?,
                    installation: row.get(2)?,
                    occurred_at: row.get(3)?,
                    category: row.get(4)?,
                    name: row.get(5)?,
                    label: row.get(6)?,
                    event_type: row.get(7)?,
                    dimensions: row.get(8)?,
                    os_name: row.get(9)?,
                    os_version: row.get(10)?,
                    os_arch: row.get(11)?,
                    app_version: row.get(12)?,
                    app_build: row.get(13)?,
                    status: row.get(14)?,
                    attempts: row.get(15)?,
                    sent_at: row.get(16)?,
                })
            })
            .expect("queries");
        rows.map(|row| row.expect("reads")).collect()
    }

    fn installations() -> Vec<String> {
        let conn = open().expect("the journal opens");
        let mut statement = conn.prepare("SELECT id FROM installation").expect("prepares");
        let ids = statement.query_map([], |row| row.get::<_, String>(0)).expect("queries");
        ids.map(|id| id.expect("reads")).collect()
    }

    #[test]
    fn a_launch_is_one_pending_row_with_every_mandatory_field() {
        with_app_dir("product-metrics-row", || {
            record(ProductEvent::AppLaunched);

            let rows = rows();
            assert_eq!(rows.len(), 1, "one event, one row");
            let row = &rows[0];
            assert!(uuid::Uuid::parse_str(&row.id).is_ok(), "id.event is not a uuid: {}", row.id);
            assert_eq!(
                (row.category.as_str(), row.name.as_str(), row.label.as_str(), row.event_type.as_str()),
                ("app", "app.launch", "App launched", "app_metric")
            );
            assert_eq!(row.dimensions, "[]", "a launch fills no slot");
            let facts = platform::facts();
            assert_eq!(row.os_name, facts.os_name);
            assert_eq!(row.os_version, facts.os_version);
            assert_eq!(row.os_arch, facts.os_arch);
            assert_eq!(row.app_version, env!("CARGO_PKG_VERSION"));
            assert_eq!(row.app_build, if cfg!(debug_assertions) { "debug" } else { "release" });
            assert_eq!((row.status.as_str(), row.attempts), ("pending", 0));
            assert!(row.sent_at.is_none(), "nothing has been sent yet");
            assert_eq!(row.installation, installations()[0], "the row does not say which installation wrote it");

            let at = chrono::DateTime::parse_from_rfc3339(&row.occurred_at).expect("a moment");
            assert_eq!(at.offset().local_minus_utc(), 0, "not UTC: {}", row.occurred_at);
            assert!(row.occurred_at.ends_with('Z'), "not the table's spelling: {}", row.occurred_at);
            assert_eq!(row.occurred_at.len(), 24, "not milliseconds, so two events in one second tie: {}", row.occurred_at);
            let since = chrono::Utc::now() - at.with_timezone(&chrono::Utc);
            assert!(since.num_seconds().abs() < 60, "stamped {} ago", since);
        });
    }

    #[test]
    fn the_installation_is_one_row_that_outlives_the_process() {
        with_app_dir("product-metrics-installation", || {
            record(ProductEvent::AppLaunched);
            record(ProductEvent::AppLaunched);

            let installations = installations();
            assert_eq!(installations.len(), 1, "an installation per event: {installations:?}");
            assert!(uuid::Uuid::parse_str(&installations[0]).is_ok());

            // Another run of the app — a fresh connection — reads the same one.
            let conn = open().expect("opens");
            assert_eq!(installation_id(&conn).expect("reads"), installations[0]);
            let stored = app_dir::dir().expect("app dir").join(FILE);
            assert!(stored.exists(), "no {} in the app directory", stored.display());
        });
    }

    /// Both first writes read the table before either wrote: each goes to
    /// create the row, and both must come back with the one installation — the
    /// loser of the race reads the winner's id rather than failing.
    #[test]
    fn two_first_writes_that_both_found_none_keep_one_installation() {
        with_app_dir("product-metrics-installation-race", || {
            let (first, second) = (open().expect("opens"), open().expect("opens"));

            let a = create_installation(&first).expect("creates the installation");
            let b = create_installation(&second).expect("reads the installation that exists");

            assert_eq!(a, b, "two first writes made two installations");
            assert_eq!(installations(), [a]);
        });
    }

    #[test]
    fn every_write_of_a_run_lands_in_the_same_installation() {
        with_app_dir("product-metrics-installation-threads", || {
            let ids: Vec<String> = std::thread::scope(|scope| {
                let handles: Vec<_> = (0..4)
                    .map(|_| {
                        scope.spawn(|| {
                            let conn = open().expect("opens");
                            installation_id(&conn).expect("reads")
                        })
                    })
                    .collect();
                handles.into_iter().map(|handle| handle.join().expect("joins")).collect()
            });

            assert_eq!(installations().len(), 1, "writes at once made two installations");
            assert!(ids.iter().all(|id| *id == ids[0]), "two ids came back: {ids:?}");
        });
    }

    #[test]
    fn each_event_carries_its_own_id_and_its_own_moment() {
        with_app_dir("product-metrics-ids", || {
            record(ProductEvent::AppLaunched);
            record(ProductEvent::AppLaunched);

            let rows = rows();
            assert_eq!(rows.len(), 2);
            assert_ne!(rows[0].id, rows[1].id, "two events under one id");
            assert!(rows[1].seq > rows[0].seq);
            assert!(rows[0].occurred_at <= rows[1].occurred_at, "the journal goes backwards");
        });
    }

    #[test]
    fn the_queue_keeps_the_newest_events_and_drops_the_rest() {
        with_app_dir("product-metrics-prune", || {
            let conn = open().expect("opens");
            let installation = installation_id(&conn).expect("reads");
            // The id of the newest row as each write lands, so what survives
            // can be named and not merely counted.
            let mut newest = Vec::new();
            for _ in 0..5 {
                append(&conn, &installation, ProductEvent::AppLaunched, 2).expect("writes");
                newest.push(rows().last().expect("a row").id.clone());
            }

            let left: Vec<String> = rows().into_iter().map(|row| row.id).collect();
            assert_eq!(left, newest[3..], "not the newest two");
        });
    }

    #[test]
    fn pruning_leaves_what_was_already_sent_alone() {
        with_app_dir("product-metrics-prune-sent", || {
            let conn = open().expect("opens");
            let installation = installation_id(&conn).expect("reads");
            for _ in 0..3 {
                append(&conn, &installation, ProductEvent::AppLaunched, MAX_PENDING).expect("writes");
            }
            let all = rows();
            conn.execute("UPDATE events SET status = 'sent', sent_at = occurred_at WHERE id = ?1", params![all[0].id])
                .expect("marks");

            append(&conn, &installation, ProductEvent::AppLaunched, 1).expect("prunes");
            let newest = rows().last().expect("a row").id.clone();

            let left: Vec<(String, String)> = rows().into_iter().map(|row| (row.id, row.status)).collect();
            assert_eq!(
                left,
                [(all[0].id.clone(), "sent".to_string()), (newest, "pending".to_string())],
                "the sent row went with the pruned ones, or the newest pending one did"
            );
        });
    }
}
