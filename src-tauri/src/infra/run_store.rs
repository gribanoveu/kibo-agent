//! The `runs` table in `chats.db`: a chat's helper runs and background
//! processes (`domain::runs`), one row each, written when it starts and again
//! when it ends.
//!
//! As in `chats`, the listing columns are real columns and the record is JSON
//! in `body`, versioned per row: a row this build cannot read is left alone
//! and not listed. A chat's rows go with it (`chat_store::delete`).

use rusqlite::{params, OptionalExtension};
use serde::{de::DeserializeOwned, Serialize};

use crate::domain::agents::AgentInfo;
use crate::domain::background::ProcessView;
use crate::domain::chat_record::ChatError;
use crate::domain::runs::{RunKind, RunLog, MAX_STORED_LISTED};
use crate::infra::chat_store::{now, open, store};

/// The `body` format. A row from a newer build is not listed.
const RUN_SCHEMA_VERSION: u32 = 1;

/// The log the app's registries write to.
pub struct RunStore;

fn kind_name(kind: RunKind) -> &'static str {
    match kind {
        RunKind::Agent => "agent",
        RunKind::Process => "process",
    }
}

/// Upserts run `id`. The first save is its start; `ended_at` is set once it
/// is no longer running.
fn save(chat: &str, kind: RunKind, id: u32, running: bool, record: &impl Serialize) -> Result<(), ChatError> {
    let body = serde_json::to_value(record).map_err(ChatError::Parse)?;
    // Both kinds serialize their state as `{"state": {"state": "exited", …}}`.
    let state = body["state"]["state"].as_str().unwrap_or_default().to_string();
    let at = now();
    open()?
        .execute(
            "INSERT INTO runs (kind, id, chat_id, version, started_at, ended_at, state, body)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(kind, id) DO UPDATE SET
               chat_id = excluded.chat_id,
               version = excluded.version,
               ended_at = excluded.ended_at,
               state = excluded.state,
               body = excluded.body",
            params![kind_name(kind), id, chat, RUN_SCHEMA_VERSION, at, (!running).then_some(at), state, body.to_string()],
        )
        .map_err(store)?;
    Ok(())
}

/// The newest [`MAX_STORED_LISTED`] of `chat`'s, oldest first; rows that do
/// not parse are skipped.
fn list<T: DeserializeOwned>(chat: &str, kind: RunKind) -> Result<Vec<T>, ChatError> {
    let conn = open()?;
    let mut stmt = conn
        .prepare(
            "SELECT body FROM runs WHERE chat_id = ?1 AND kind = ?2 AND version <= ?3
             ORDER BY id DESC LIMIT ?4",
        )
        .map_err(store)?;
    let bodies = stmt
        .query_map(params![chat, kind_name(kind), RUN_SCHEMA_VERSION, MAX_STORED_LISTED as i64], |row| row.get::<_, String>(0))
        .map_err(store)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(store)?;
    Ok(bodies.iter().rev().filter_map(|body| serde_json::from_str(body).ok()).collect())
}

fn last_id(kind: RunKind) -> Result<u32, ChatError> {
    open()?
        .query_row("SELECT MAX(id) FROM runs WHERE kind = ?1", [kind_name(kind)], |row| row.get::<_, Option<u32>>(0))
        .optional()
        .map(|id| id.flatten().unwrap_or(0))
        .map_err(store)
}

/// A record lost is said, not raised: the run goes on either way.
fn reported<T: Default>(what: &str, result: Result<T, ChatError>) -> T {
    match result {
        Ok(value) => value,
        Err(e) => {
            eprintln!("runs: {what}: {e}");
            T::default()
        }
    }
}

impl RunLog for RunStore {
    fn last_id(&self, kind: RunKind) -> u32 {
        reported("last id", last_id(kind))
    }

    fn save_agent(&self, chat: &str, run: &AgentInfo) {
        let running = run.state == crate::domain::agents::AgentState::Running;
        reported("save a helper run", save(chat, RunKind::Agent, run.id, running, run));
    }

    fn save_process(&self, chat: &str, process: &ProcessView) {
        reported("save a process", save(chat, RunKind::Process, process.info.id, process.info.running(), process));
    }

    fn agents(&self, chat: &str) -> Vec<AgentInfo> {
        reported("list helper runs", list(chat, RunKind::Agent))
    }

    fn processes(&self, chat: &str) -> Vec<ProcessView> {
        reported("list processes", list(chat, RunKind::Process))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::agents::{AgentState, AgentTokens};
    use crate::domain::background::{ProcessInfo, ProcessState};
    use crate::testing::with_app_dir;

    fn agent(id: u32, state: AgentState) -> AgentInfo {
        AgentInfo {
            id,
            task: format!("t{id}"),
            state,
            steps: vec!["grep X".into()],
            tokens: AgentTokens { prompt: 10, cached: 4, completion: 2 },
            answer: Some("a.rs:1".into()),
        }
    }

    fn process(id: u32, state: ProcessState) -> ProcessView {
        ProcessView { info: ProcessInfo { id, command: "npm test".into(), cwd: "web".into(), state }, tail: "ok\n".into() }
    }

    fn row(kind: &str, id: u32) -> (String, i64, Option<i64>, String) {
        open()
            .unwrap()
            .query_row("SELECT chat_id, started_at, ended_at, state FROM runs WHERE kind = ?1 AND id = ?2", params![kind, id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .unwrap()
    }

    #[test]
    fn a_run_is_written_at_its_start_replaced_at_its_end_and_read_back_whole() {
        with_app_dir("run-store", || {
            RunStore.save_agent("a", &agent(1, AgentState::Running));
            let (chat, started, ended, state) = row("agent", 1);
            assert_eq!((chat.as_str(), ended, state.as_str()), ("a", None, "running"));

            std::thread::sleep(std::time::Duration::from_millis(5));
            let failed = agent(1, AgentState::Failed { reason: "no key".into() });
            RunStore.save_agent("a", &failed);
            let (_, started_again, ended, state) = row("agent", 1);
            assert_eq!((started_again, state.as_str()), (started, "failed"), "the start is kept");
            assert!(ended.is_some_and(|ended| ended > started), "{ended:?}");
            assert_eq!(RunStore.agents("a"), [failed]);

            let exited = process(1, ProcessState::Exited { code: Some(1) });
            RunStore.save_process("a", &exited);
            assert_eq!(RunStore.processes("a"), [exited], "a process #1 is not the helper #1");
            assert_eq!(row("process", 1).3, "exited");
            assert_eq!(RunStore.agents("a").len(), 1);
            assert!(row("process", 1).2.is_some());
        });
    }

    #[test]
    fn a_chat_reads_its_own_newest_and_numbers_go_on_from_the_highest() {
        with_app_dir("run-store-list", || {
            assert_eq!((RunStore.last_id(RunKind::Agent), RunStore.last_id(RunKind::Process)), (0, 0));
            for id in 1..=MAX_STORED_LISTED as u32 + 2 {
                RunStore.save_process("a", &process(id, ProcessState::Stopped));
            }
            RunStore.save_process("b", &process(100, ProcessState::Running));
            RunStore.save_agent("b", &agent(7, AgentState::Done));
            // Newer than every process of the chat: listed as one, it would take a place.
            RunStore.save_agent("a", &agent(60, AgentState::Done));

            let ids: Vec<u32> = RunStore.processes("a").iter().map(|p| p.info.id).collect();
            assert_eq!(ids, (3..=MAX_STORED_LISTED as u32 + 2).collect::<Vec<_>>(), "the newest, oldest first");
            assert_eq!(RunStore.processes("b")[0].info.state, ProcessState::Running);
            assert_eq!(RunStore.agents("a").iter().map(|r| r.id).collect::<Vec<_>>(), [60]);
            assert_eq!((RunStore.last_id(RunKind::Agent), RunStore.last_id(RunKind::Process)), (60, 100));
        });
    }

    /// A row a newer build wrote, or one that does not parse, is one run
    /// missing from the list — the rest still list.
    #[test]
    fn a_row_this_build_cannot_read_is_left_out() {
        with_app_dir("run-store-newer", || {
            for id in 1..=3 {
                RunStore.save_agent("a", &agent(id, AgentState::Done));
            }
            let conn = open().unwrap();
            conn.execute("UPDATE runs SET version = ?1 WHERE id = 1", [RUN_SCHEMA_VERSION + 1]).unwrap();
            conn.execute("UPDATE runs SET body = '{ not json' WHERE id = 2", []).unwrap();
            assert_eq!(RunStore.agents("a").iter().map(|r| r.id).collect::<Vec<_>>(), [3]);
        });
    }
}
