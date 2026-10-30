//! A chat's helper runs and background processes, kept past the registries
//! that hold them while they run (`domain::agents`, `infra::background`): a
//! chat reopened later — or after a restart — still shows what its helpers
//! did and how its commands ended. The store is `infra::run_store`.

use crate::domain::agents::{AgentInfo, AgentState};
use crate::domain::background::{ProcessState, ProcessView};

/// Listed per chat and kind, newest kept: a chat that ran a thousand builds
/// shows the last of them, not a megabyte of tails.
pub const MAX_STORED_LISTED: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunKind {
    Agent,
    Process,
}

/// Where the registries write a run when it starts and when it ends. Saving
/// is best effort: a store that fails costs the record, never the turn.
pub trait RunLog: Send + Sync {
    /// The highest number stored for `kind`, for this launch to go on from:
    /// a stored #3 and a live #3 would be one row in the tab.
    fn last_id(&self, kind: RunKind) -> u32;
    /// A save over an earlier one of the same run replaces it.
    fn save_agent(&self, chat: &str, run: &AgentInfo);
    fn save_process(&self, chat: &str, process: &ProcessView);
    /// The newest [`MAX_STORED_LISTED`] of `chat`'s, oldest first.
    fn agents(&self, chat: &str) -> Vec<AgentInfo>;
    fn processes(&self, chat: &str) -> Vec<ProcessView>;
}

/// Nothing kept: tests, and runs outside the app.
pub struct NoLog;

impl RunLog for NoLog {
    fn last_id(&self, _: RunKind) -> u32 {
        0
    }
    fn save_agent(&self, _: &str, _: &AgentInfo) {}
    fn save_process(&self, _: &str, _: &ProcessView) {}
    fn agents(&self, _: &str) -> Vec<AgentInfo> {
        Vec::new()
    }
    fn processes(&self, _: &str) -> Vec<ProcessView> {
        Vec::new()
    }
}

/// A chat's runs as the tabs list them, oldest first: the live ones, and the
/// stored ones the registry no longer holds — from before a restart, or
/// pushed out by newer ones. Live wins: it is the same run, further along.
pub fn merge<T>(stored: Vec<T>, mut live: Vec<T>, id: impl Fn(&T) -> u32, cut_short: impl Fn(&mut T)) -> Vec<T> {
    for mut old in stored {
        if !live.iter().any(|run| id(run) == id(&old)) {
            cut_short(&mut old);
            live.push(old);
        }
    }
    live.sort_by_key(|run| id(run));
    live
}

/// A stored run still running that no registry holds was cut short: the app
/// quit under it.
pub fn agent_cut_short(run: &mut AgentInfo) {
    if run.state == AgentState::Running {
        run.state = AgentState::Stopped;
    }
}

pub fn process_cut_short(process: &mut ProcessView) {
    if process.info.running() {
        process.info.state = ProcessState::Stopped;
    }
}

/// The log in memory, for the registries' tests: what was saved, last save
/// per run, in the order first saved.
#[cfg(test)]
#[derive(Default)]
pub struct MemoryLog {
    pub agents: std::sync::Mutex<Vec<(String, AgentInfo)>>,
    pub processes: std::sync::Mutex<Vec<(String, ProcessView)>>,
}

#[cfg(test)]
impl RunLog for MemoryLog {
    fn last_id(&self, kind: RunKind) -> u32 {
        match kind {
            RunKind::Agent => self.agents.lock().unwrap().iter().map(|(_, run)| run.id).max(),
            RunKind::Process => self.processes.lock().unwrap().iter().map(|(_, p)| p.info.id).max(),
        }
        .unwrap_or(0)
    }
    fn save_agent(&self, chat: &str, run: &AgentInfo) {
        let mut saved = self.agents.lock().unwrap();
        match saved.iter_mut().find(|(_, old)| old.id == run.id) {
            Some(old) => *old = (chat.to_string(), run.clone()),
            None => saved.push((chat.to_string(), run.clone())),
        }
    }
    fn save_process(&self, chat: &str, process: &ProcessView) {
        let mut saved = self.processes.lock().unwrap();
        match saved.iter_mut().find(|(_, old)| old.info.id == process.info.id) {
            Some(old) => *old = (chat.to_string(), process.clone()),
            None => saved.push((chat.to_string(), process.clone())),
        }
    }
    fn agents(&self, chat: &str) -> Vec<AgentInfo> {
        self.agents.lock().unwrap().iter().filter(|(c, _)| c == chat).map(|(_, run)| run.clone()).collect()
    }
    fn processes(&self, chat: &str) -> Vec<ProcessView> {
        self.processes.lock().unwrap().iter().filter(|(c, _)| c == chat).map(|(_, p)| p.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::agents::AgentTokens;

    fn run(id: u32, state: AgentState) -> AgentInfo {
        AgentInfo { id, task: format!("t{id}"), state, steps: Vec::new(), tokens: AgentTokens::default(), answer: None }
    }

    #[test]
    fn stored_and_live_are_one_list_by_number_and_a_stale_running_one_is_stopped() {
        let stored = vec![run(1, AgentState::Done), run(2, AgentState::Running), run(4, AgentState::Running)];
        let live = vec![run(4, AgentState::Done), run(5, AgentState::Running)];
        let merged = merge(stored, live, |r| r.id, agent_cut_short);
        let shown: Vec<_> = merged.iter().map(|r| (r.id, r.state.clone())).collect();
        assert_eq!(
            shown,
            [(1, AgentState::Done), (2, AgentState::Stopped), (4, AgentState::Done), (5, AgentState::Running)]
        );
    }

    #[test]
    fn a_stored_process_still_running_is_shown_stopped_and_an_ended_one_as_it_ended() {
        use crate::domain::background::ProcessInfo;
        let view = |id, state| ProcessView { info: ProcessInfo { id, command: "x".into(), cwd: ".".into(), state }, tail: String::new() };
        let mut cut = view(1, ProcessState::Running);
        process_cut_short(&mut cut);
        let mut ended = view(2, ProcessState::Exited { code: Some(1) });
        process_cut_short(&mut ended);
        assert_eq!((cut.info.state, ended.info.state), (ProcessState::Stopped, ProcessState::Exited { code: Some(1) }));
    }
}
