//! Helper agents: every `explore` run, as the Agents tab shows it — what it
//! was asked, what it did, what it cost, how it ended — and the user's Stop.
//!
//! A run lives inside the call that started it: the calling turn waits for
//! it. What outlives it is the record, so the tab can still show what a
//! finished helper read and answered — a chat's runs go to its [`RunLog`] as
//! well, at the start and the end, and are listed from there once they are
//! gone from here. No I/O here, only the list and a lock, which is why it
//! sits in `domain` next to its sink.

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::domain::llm::ChatUsage;
use crate::domain::runs::{self, NoLog, RunKind, RunLog};

/// Finished runs kept for the tab, oldest dropped first.
pub const MAX_FINISHED: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInfo {
    pub id: u32,
    pub task: String,
    pub state: AgentState,
    /// One line per call it made: the tool and what it was pointed at.
    pub steps: Vec<String>,
    pub tokens: AgentTokens,
    /// Its closing answer, once it has one.
    pub answer: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum AgentState {
    Running,
    Done,
    Failed { reason: String },
    /// By the tab's Stop, or by the calling turn's.
    Stopped,
}

/// What the provider reported for every request of one run, summed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTokens {
    pub prompt: u64,
    /// The part of `prompt` read from the provider's cache.
    pub cached: u64,
    pub completion: u64,
}

impl AgentTokens {
    pub fn add(&mut self, usage: &ChatUsage) {
        self.prompt += u64::from(usage.prompt_tokens);
        self.cached += u64::from(usage.cached_tokens);
        self.completion += u64::from(usage.completion_tokens);
    }
}

/// A run started, took a step, spent tokens or ended. A signal to read the
/// list again, like `ProcessChanged`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AgentChanged {
    pub id: u32,
}

pub type AgentEventSink = Arc<dyn Fn(AgentChanged) + Send + Sync>;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AgentError {
    #[error("no helper agent #{0}")]
    NotFound(u32),
    #[error("helper agent #{0} is not running")]
    NotRunning(u32),
}

struct Run {
    info: AgentInfo,
    stop_asked: bool,
    /// The chat whose turn started it; `None` outside one.
    chat: Option<String>,
}

/// The record of every run. [`Agents::for_chat`] gives a turn its own handle
/// on the same record, so what it starts is listed under its chat.
pub struct Agents {
    runs: Arc<Mutex<(u32, Vec<Run>)>>,
    sink: AgentEventSink,
    chat: Option<String>,
    log: Arc<dyn RunLog>,
}

impl Default for Agents {
    fn default() -> Self {
        Self::new(Arc::new(|_| {}))
    }
}

impl Agents {
    pub fn new(sink: AgentEventSink) -> Self {
        Self::with_log(sink, Arc::new(NoLog))
    }

    /// Numbered on from the last run `log` holds, and a chat's runs saved there.
    pub fn with_log(sink: AgentEventSink, log: Arc<dyn RunLog>) -> Self {
        let runs = Arc::new(Mutex::new((log.last_id(RunKind::Agent), Vec::new())));
        Self { runs, sink, chat: None, log }
    }

    /// The same record, for a turn of `chat`: what it starts is that chat's.
    pub fn for_chat(&self, chat: &str) -> Agents {
        Agents {
            runs: Arc::clone(&self.runs),
            sink: Arc::clone(&self.sink),
            chat: Some(chat.to_string()),
            log: Arc::clone(&self.log),
        }
    }

    pub fn start(&self, task: &str) -> u32 {
        let id = {
            let mut runs = self.lock();
            runs.0 += 1;
            let id = runs.0;
            runs.1.push(Run {
                info: AgentInfo {
                    id,
                    task: task.to_string(),
                    state: AgentState::Running,
                    steps: Vec::new(),
                    tokens: AgentTokens::default(),
                    answer: None,
                },
                stop_asked: false,
                chat: self.chat.clone(),
            });
            id
        };
        self.save(id);
        (self.sink)(AgentChanged { id });
        id
    }

    pub fn step(&self, id: u32, line: String) {
        self.update(id, |run| run.info.steps.push(line));
    }

    pub fn spent(&self, id: u32, usage: &ChatUsage) {
        self.update(id, |run| run.info.tokens.add(usage));
    }

    /// Ends a run, and lets the oldest finished ones go past [`MAX_FINISHED`].
    pub fn finish(&self, id: u32, state: AgentState, answer: Option<String>) {
        self.update(id, |run| {
            run.info.state = state;
            run.info.answer = answer;
        });
        self.save(id);
        let mut runs = self.lock();
        let finished = runs.1.iter().filter(|run| run.info.state != AgentState::Running).count();
        let mut excess = finished.saturating_sub(MAX_FINISHED);
        runs.1.retain(|run| {
            let drop = excess > 0 && run.info.state != AgentState::Running;
            excess -= usize::from(drop);
            !drop
        });
    }

    /// The tab's Stop: the run sees it at its next check and ends there.
    pub fn stop(&self, id: u32) -> Result<(), AgentError> {
        {
            let mut runs = self.lock();
            let run = runs.1.iter_mut().find(|run| run.info.id == id).ok_or(AgentError::NotFound(id))?;
            if run.info.state != AgentState::Running {
                return Err(AgentError::NotRunning(id));
            }
            run.stop_asked = true;
        }
        (self.sink)(AgentChanged { id });
        Ok(())
    }

    pub fn stop_asked(&self, id: u32) -> bool {
        self.lock().1.iter().any(|run| run.info.id == id && run.stop_asked)
    }

    pub fn get(&self, id: u32) -> Option<AgentInfo> {
        self.lock().1.iter().find(|run| run.info.id == id).map(|run| run.info.clone())
    }

    /// Oldest first.
    pub fn list(&self) -> Vec<AgentInfo> {
        self.lock().1.iter().map(|run| run.info.clone()).collect()
    }

    /// The runs `chat` started, oldest first: the tab and Overview show the
    /// open chat's, not every chat's since launch — and its stored ones, from
    /// before a restart too.
    pub fn list_for(&self, chat: &str) -> Vec<AgentInfo> {
        let live =
            self.lock().1.iter().filter(|run| run.chat.as_deref() == Some(chat)).map(|run| run.info.clone()).collect();
        runs::merge(self.log.agents(chat), live, |run| run.id, runs::agent_cut_short)
    }

    /// Writes run `id` to the log as it stands, if a chat started it. Outside
    /// the lock: the log is a database.
    fn save(&self, id: u32) {
        let saved = self.lock().1.iter().find(|run| run.info.id == id).and_then(|run| Some((run.chat.clone()?, run.info.clone())));
        if let Some((chat, info)) = saved {
            self.log.save_agent(&chat, &info);
        }
    }

    fn update(&self, id: u32, change: impl FnOnce(&mut Run)) {
        let found = match self.lock().1.iter_mut().find(|run| run.info.id == id) {
            Some(run) => {
                change(run);
                true
            }
            None => false,
        };
        if found {
            (self.sink)(AgentChanged { id });
        }
    }

    /// A poisoned lock is recovered: losing the tab's list must not take a
    /// turn down with it.
    fn lock(&self) -> std::sync::MutexGuard<'_, (u32, Vec<Run>)> {
        self.runs.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(prompt: u32, cached: u32, completion: u32) -> ChatUsage {
        ChatUsage { prompt_tokens: prompt, completion_tokens: completion, total_tokens: prompt + completion, cached_tokens: cached }
    }

    #[test]
    fn a_chats_handle_lists_its_runs_under_it_and_signals_on_the_same_sink() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let heard = Arc::clone(&seen);
        let agents = Agents::new(Arc::new(move |event: AgentChanged| heard.lock().unwrap().push(event.id)));
        let (mine, theirs) = (agents.for_chat("a"), agents.for_chat("b"));
        mine.start("first");
        theirs.start("second");
        agents.start("outside a chat");
        mine.start("third");
        let tasks = |chat| agents.list_for(chat).into_iter().map(|run| run.task).collect::<Vec<_>>();
        assert_eq!((tasks("a"), tasks("b"), tasks("c")), (vec!["first".to_string(), "third".into()], vec!["second".into()], vec![]));
        assert_eq!(theirs.list().len(), 4);
        assert_eq!(*seen.lock().unwrap(), [1, 2, 3, 4]);
    }

    #[test]
    fn a_run_is_recorded_from_start_to_answer_and_each_change_is_signalled() {
        let signals = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&signals);
        let agents = Agents::new(Arc::new(move |event: AgentChanged| seen.lock().unwrap().push(event.id)));
        let id = agents.start("where is X");
        agents.step(id, "grep X".into());
        agents.spent(id, &usage(1000, 800, 50));
        agents.spent(id, &usage(1200, 1000, 70));
        agents.finish(id, AgentState::Done, Some("a.rs:1".into()));

        let run = agents.get(id).unwrap();
        assert_eq!((run.task.as_str(), run.state.clone(), run.answer.as_deref()), ("where is X", AgentState::Done, Some("a.rs:1")));
        assert_eq!(run.steps, ["grep X"]);
        assert_eq!(run.tokens, AgentTokens { prompt: 2200, cached: 1800, completion: 120 });
        assert_eq!(*signals.lock().unwrap(), [id; 5]);
    }

    #[test]
    fn stop_is_asked_of_a_running_run_only() {
        let agents = Agents::default();
        let id = agents.start("t");
        assert!(!agents.stop_asked(id));
        agents.stop(id).unwrap();
        assert!(agents.stop_asked(id));
        agents.finish(id, AgentState::Stopped, None);
        assert_eq!(agents.stop(id), Err(AgentError::NotRunning(id)));
        assert_eq!(agents.stop(9), Err(AgentError::NotFound(9)));
    }

    /// A chat's runs are saved as they start and end, and listed from the
    /// log once the record has let them go — after a restart, numbered on.
    #[test]
    fn a_chats_runs_are_saved_and_listed_back_after_a_restart() {
        use crate::domain::runs::{MemoryLog, RunLog};
        let log = Arc::new(MemoryLog::default());
        let before = Agents::with_log(Arc::new(|_| {}), log.clone());
        let chat = before.for_chat("a");
        let done = chat.start("where is X");
        chat.step(done, "grep X".into());
        let saved_running = log.agents("a");
        chat.finish(done, AgentState::Done, Some("a.rs:1".into()));
        let cut = chat.start("never finished");
        before.start("outside a chat");
        assert_eq!(saved_running.iter().map(|r| r.state.clone()).collect::<Vec<_>>(), [AgentState::Running]);
        assert_eq!(log.agents("a").len(), 2, "the run outside a chat is not saved");

        let after = Agents::with_log(Arc::new(|_| {}), log.clone());
        let next = after.for_chat("a").start("again");
        assert_eq!(next, cut + 1, "numbered on from the log: the run outside a chat went with the launch");
        after.for_chat("b").start("elsewhere");
        let listed = after.list_for("a");
        let shown: Vec<_> = listed.iter().map(|r| (r.id, r.state.clone())).collect();
        assert_eq!(shown, [(done, AgentState::Done), (cut, AgentState::Stopped), (next, AgentState::Running)]);
        assert_eq!((listed[0].steps.as_slice(), listed[0].answer.as_deref()), (["grep X".to_string()].as_slice(), Some("a.rs:1")));
    }

    #[test]
    fn the_oldest_finished_runs_go_and_a_running_one_stays() {
        let agents = Agents::default();
        let running = agents.start("still going");
        for i in 0..MAX_FINISHED + 2 {
            let id = agents.start(&format!("t{i}"));
            agents.finish(id, AgentState::Done, None);
        }
        let ids: Vec<u32> = agents.list().iter().map(|run| run.id).collect();
        assert_eq!(ids.len(), MAX_FINISHED + 1);
        assert_eq!(ids[0], running);
        assert_eq!(ids[1], running + 3, "the two oldest finished are gone");
    }
}
