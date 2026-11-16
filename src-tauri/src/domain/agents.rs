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

/// A run's streamed characters are signalled at most this often: deltas
/// arrive every few milliseconds, and each signal is the window re-reading
/// the list. Throttled where it changes, as `infra::background` does.
pub const STREAM_SIGNAL_MS: u64 = 100;

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
    /// The stream under way, for the live meter; absent in runs saved
    /// before it existed.
    #[serde(default)]
    pub progress: AgentProgress,
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

/// What a run's live speed estimate needs, accumulated as its rounds
/// stream — the same quantities the chat's own meter keeps (`liveSpeed`
/// in `src/lib/chatTurnReducer.ts`), which turns characters into tokens
/// at the ratio the finished rounds had. Time is a parameter, so the
/// rule stays pure and testable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentProgress {
    /// Epoch ms of the round under way's first streamed token; `None`
    /// before it has one.
    pub stream_since: Option<u64>,
    /// Characters the round under way has streamed: text, reasoning, call
    /// arguments.
    pub stream_chars: u64,
    /// The finished rounds' totals the chars-per-token ratio comes from.
    pub timed: AgentTimed,
}

/// Tokens, streaming time and characters of the rounds that were timed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentTimed {
    pub tokens: u64,
    pub ms: u64,
    pub chars: u64,
}

impl AgentProgress {
    /// A fresh round: the last one's characters are not this one's.
    pub fn round_started(&mut self) {
        self.stream_since = None;
        self.stream_chars = 0;
    }

    /// Characters arrived; the first of a round starts its clock.
    pub fn streamed(&mut self, chars: u64, now: u64) {
        self.stream_since = Some(self.stream_since.unwrap_or(now));
        self.stream_chars += chars;
    }

    /// The round ended with the provider's count. The wait for the first
    /// token is the provider reading the prompt, not sending — only the
    /// streaming time counts. An answer that arrived in one piece has no
    /// streaming time to divide by, and a provider that reports nothing
    /// has no tokens to weigh; either way the round still closes.
    pub fn round_timed(&mut self, completion_tokens: u64, now: u64) {
        let ms = self.stream_since.map_or(0, |since| now.saturating_sub(since));
        let chars = self.stream_chars;
        self.round_started();
        if ms < 250 || completion_tokens == 0 {
            return;
        }
        self.timed.tokens += completion_tokens;
        self.timed.ms += ms;
        self.timed.chars += chars;
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
    /// When streamed characters last signalled, for [`STREAM_SIGNAL_MS`].
    last_stream_signal: u64,
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

    /// The chat this handle starts runs under; `None` outside one.
    pub fn chat(&self) -> Option<&str> {
        self.chat.as_deref()
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
                    progress: AgentProgress::default(),
                },
                stop_asked: false,
                chat: self.chat.clone(),
                last_stream_signal: 0,
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

    /// The round's cost, and its streaming time for the live meter.
    pub fn spent(&self, id: u32, usage: &ChatUsage, now: u64) {
        self.update(id, |run| {
            run.info.tokens.add(usage);
            run.info.progress.round_timed(u64::from(usage.completion_tokens), now);
        });
    }

    /// A fresh round of the run's stream: the meter starts over, and the
    /// round's first token signals at once — the throttle is for the burst
    /// within a round, not for the round's start.
    pub fn round_started(&self, id: u32) {
        self.update(id, |run| {
            run.info.progress.round_started();
            run.last_stream_signal = 0;
        });
    }

    /// Characters of the run's stream, signalled at most once per
    /// [`STREAM_SIGNAL_MS`] — the round's end and its steps signal on their
    /// own, so nothing is left unsaid past the throttle.
    // ponytail: each signal is the window re-reading agents_list — a
    // progress channel of its own if ten reads a second per running
    // subagent ever shows.
    pub fn streamed(&self, id: u32, chars: u64, now: u64) {
        let signal = {
            let mut runs = self.lock();
            match runs.1.iter_mut().find(|run| run.info.id == id) {
                Some(run) => {
                    run.info.progress.streamed(chars, now);
                    let due = now.saturating_sub(run.last_stream_signal) >= STREAM_SIGNAL_MS;
                    if due {
                        run.last_stream_signal = now;
                    }
                    due
                }
                None => false,
            }
        };
        if signal {
            (self.sink)(AgentChanged { id });
        }
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
        agents.spent(id, &usage(1000, 800, 50), 1000);
        agents.spent(id, &usage(1200, 1000, 70), 2000);
        agents.finish(id, AgentState::Done, Some("a.rs:1".into()));

        let run = agents.get(id).unwrap();
        assert_eq!((run.task.as_str(), run.state.clone(), run.answer.as_deref()), ("where is X", AgentState::Done, Some("a.rs:1")));
        assert_eq!(run.steps, ["grep X"]);
        assert_eq!(run.tokens, AgentTokens { prompt: 2200, cached: 1800, completion: 120 });
        assert_eq!(*signals.lock().unwrap(), [id; 5]);
    }

    /// The meter's rule, the chat's own (`liveSpeed`): characters count from
    /// the round's first token, and only a round that streamed long enough
    /// with tokens reported weighs into the chars-per-token ratio.
    #[test]
    fn progress_counts_the_rounds_stream_and_the_ratio() {
        let mut p = AgentProgress::default();
        p.streamed(10, 1000);
        p.streamed(5, 1200);
        assert_eq!((p.stream_since, p.stream_chars), (Some(1000), 15));
        p.round_timed(50, 2000);
        assert_eq!(p.timed, AgentTimed { tokens: 50, ms: 1000, chars: 15 });
        assert_eq!((p.stream_since, p.stream_chars), (None, 0), "the round closes");

        // A provider that reports no tokens adds nothing to the ratio.
        p.streamed(7, 3000);
        p.round_timed(0, 4000);
        assert_eq!(p.timed, AgentTimed { tokens: 50, ms: 1000, chars: 15 });
        // An answer that arrived in one piece has no streaming time to divide by.
        p.streamed(3, 5000);
        p.round_timed(10, 5100);
        assert_eq!(p.timed, AgentTimed { tokens: 50, ms: 1000, chars: 15 });
        // A round that never streamed is not timed either.
        p.round_timed(10, 7000);
        assert_eq!(p.timed, AgentTimed { tokens: 50, ms: 1000, chars: 15 });

        // A fresh round starts over, and the next timed one adds to the ratio.
        p.streamed(9, 8000);
        p.round_started();
        assert_eq!((p.stream_since, p.stream_chars), (None, 0));
        p.streamed(20, 9000);
        p.round_timed(100, 9500);
        assert_eq!(p.timed, AgentTimed { tokens: 150, ms: 1500, chars: 35 });
    }

    #[test]
    fn stream_signals_are_throttled_but_steps_and_the_rounds_end_are_not() {
        let signals = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&signals);
        let agents = Agents::new(Arc::new(move |event: AgentChanged| seen.lock().unwrap().push(event.id)));
        let id = agents.start("t");
        agents.streamed(id, 4, 1000); // the first of the run signals at once
        agents.streamed(id, 4, 1000 + STREAM_SIGNAL_MS - 1); // within the window: kept, not signalled
        assert_eq!(*signals.lock().unwrap(), [id; 2]);
        assert_eq!(agents.get(id).unwrap().progress.stream_chars, 8);
        agents.streamed(id, 4, 1000 + STREAM_SIGNAL_MS); // the window is up
        assert_eq!(*signals.lock().unwrap(), [id; 3]);
        agents.step(id, "grep X".into()); // a step is rare and says so at once
        agents.spent(id, &usage(100, 0, 10), 1150); // so is the round's end
        assert_eq!(*signals.lock().unwrap(), [id; 5]);
        agents.streamed(id, 4, 1160); // still within the last stream signal's window
        assert_eq!(signals.lock().unwrap().len(), 5);
        agents.round_started(id); // a new round says so at once...
        agents.streamed(id, 4, 1170); // ...and so does its first token
        assert_eq!(*signals.lock().unwrap(), [id; 7]);
        agents.streamed(99, 4, 2000); // a run that is not there is not signalled
        assert_eq!(signals.lock().unwrap().len(), 7);
    }

    /// Runs saved before the meter existed still read: the field defaults.
    #[test]
    fn a_saved_run_without_progress_reads_with_none() {
        let old = serde_json::json!({
            "id": 1, "task": "t", "state": { "state": "done" }, "steps": [],
            "tokens": { "prompt": 10, "cached": 4, "completion": 2 }, "answer": "a.rs:1"
        });
        let run: AgentInfo = serde_json::from_value(old).unwrap();
        assert_eq!(run.progress, AgentProgress::default());
        assert_eq!(run.tokens, AgentTokens { prompt: 10, cached: 4, completion: 2 });
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
