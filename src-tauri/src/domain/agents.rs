//! Helper agents: every `explore` run, as the Agents tab shows it — what it
//! was asked, what it did, what it cost, how it ended — and the user's Stop.
//!
//! A run lives inside the call that started it: the calling turn waits for
//! it. What outlives it is the record, so the tab can still show what a
//! finished helper read and answered. No I/O here, only the list and a lock,
//! which is why it sits in `domain` next to its sink.

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::domain::llm::ChatUsage;

/// Finished runs kept for the tab, oldest dropped first.
pub const MAX_FINISHED: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
}

pub struct Agents {
    runs: Mutex<(u32, Vec<Run>)>,
    sink: AgentEventSink,
}

impl Default for Agents {
    fn default() -> Self {
        Self::new(Arc::new(|_| {}))
    }
}

impl Agents {
    pub fn new(sink: AgentEventSink) -> Self {
        Self { runs: Mutex::new((0, Vec::new())), sink }
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
            });
            id
        };
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
