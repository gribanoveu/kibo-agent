//! Background processes (CA-4.5): a command that outlives the call that
//! started it — a dev server, a watcher, a long build.
//!
//! Started by `runCommand` with `background: true`, read with `readOutput`,
//! stopped with `stopProcess` or from the Terminal tab. Each lives until it
//! exits, is stopped, the workspace changes, or the app quits — not the turn:
//! a dev server the agent started has to be there when the next message
//! asks it something. Decisions in `docs/06-port-plan.md`, stage 7.
//!
//! The port is here and the processes are in `infra::background`, because
//! `ToolDeps` carries it and `domain` does not reach into `infra`.

use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::domain::command_exec::Shell;

/// Running at once. A sixth start is refused with the list of what runs, so
/// the model stops one rather than piling up servers on ports.
pub const MAX_RUNNING: usize = 5;
/// Kept per process, newest last; what falls off the front is gone, and a
/// read that missed it says so.
pub const MAX_BUFFER_BYTES: usize = 1 << 20;
/// Finished processes kept for reading, oldest dropped first.
pub const MAX_FINISHED: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessInfo {
    pub id: u32,
    pub command: String,
    /// Relative to the workspace, as it was asked for; `.` for the root.
    pub cwd: String,
    pub state: ProcessState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum ProcessState {
    Running,
    /// Ended on its own; `None` when a signal ended it.
    Exited { code: Option<i32> },
    /// Stopped by `stopProcess`, the Terminal tab, or a workspace change.
    Stopped,
}

impl ProcessInfo {
    pub fn running(&self) -> bool {
        self.state == ProcessState::Running
    }

    /// One line for the model: `#2 \`npm run dev\` exited with code 1`.
    pub fn describe(&self) -> String {
        let how = match self.state {
            ProcessState::Running => "is running".to_string(),
            ProcessState::Exited { code: Some(code) } => format!("exited with code {code}"),
            ProcessState::Exited { code: None } => "was ended by a signal".to_string(),
            ProcessState::Stopped => "was stopped".to_string(),
        };
        format!("#{} `{}` {how}", self.id, self.command)
    }
}

/// What `readOutput` returns: everything the process wrote since the model
/// last read it, stdout and stderr together in the order they arrived.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessOutput {
    #[serde(flatten)]
    pub process: ProcessInfo,
    pub output: String,
    /// Some output since the last read had already fallen out of the buffer.
    pub missed: bool,
    /// `output` had its middle cut to fit what a tool result may carry.
    pub truncated: bool,
}

/// Something about process `id` changed: it started, wrote, ended or was
/// stopped. A signal to read the list again, not the change itself — the
/// Terminal tab re-reads, so a chatty process's output is signalled at a
/// steady rate rather than once per chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessChanged {
    pub id: u32,
}

pub type ProcessEventSink = Arc<dyn Fn(ProcessChanged) + Send + Sync>;

/// One of a process's streams, fed by whoever reads its pipe: `Some` for text
/// read, `None` once the pipe is closed.
pub type Feed = Box<dyn FnMut(Option<&str>) + Send>;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum BackgroundError {
    #[error("{MAX_RUNNING} background processes are already running ({0}). Stop one with stopProcess before starting another.")]
    TooMany(String),
    #[error("there is no background process #{0}")]
    NotFound(u32),
    #[error("could not start the command: {0}")]
    NotStarted(String),
}

pub trait BackgroundProcesses: Send + Sync {
    fn start(&self, shell: &Shell, command: &str, cwd: &Path, shown_cwd: &str) -> Result<ProcessInfo, BackgroundError>;

    /// Takes over a `runCommand` still running at its timeout: it goes on as
    /// one of these, `backlog` — what it wrote so far — unread in its output.
    /// The caller's readers feed its two streams from then on, one [`Feed`]
    /// each. Refused when too many run; the child is then ended here.
    fn adopt(
        &self,
        child: std::process::Child,
        command: &str,
        shown_cwd: &str,
        backlog: &str,
    ) -> Result<(ProcessInfo, [Feed; 2]), BackgroundError>;

    /// Moves the model's place to the end: the same output is not read twice.
    fn read(&self, id: u32) -> Result<ProcessOutput, BackgroundError>;

    /// Kills it and whatever it started. Stopping one that already ended
    /// is not an error — it says how it ended.
    fn stop(&self, id: u32) -> Result<ProcessInfo, BackgroundError>;

    fn list(&self) -> Vec<ProcessInfo>;

    /// Processes that ended since the last call, each reported once — except
    /// those `stop` was asked to end, which the asker already knows about.
    fn take_ended(&self) -> Vec<ProcessInfo>;
}

/// What the model is told at the start of a round about processes that
/// ended while it was not looking. `None` when there is nothing to say.
pub fn ended_note(ended: &[ProcessInfo]) -> Option<String> {
    if ended.is_empty() {
        return None;
    }
    let lines: Vec<String> = ended.iter().map(|p| format!("- {}", p.describe())).collect();
    Some(format!(
        "[Background processes that ended since you last looked — readOutput has what they wrote:]\n{}",
        lines.join("\n")
    ))
}

/// Output kept for one process: the last [`MAX_BUFFER_BYTES`], with absolute
/// positions so a reader can tell what it missed.
#[derive(Debug, Default)]
pub struct OutputBuffer {
    text: String,
    /// Absolute position of `text`'s first byte in everything ever written.
    start: usize,
    /// Absolute position the model has read up to.
    read_to: usize,
}

impl OutputBuffer {
    /// Everything ever written, in bytes — grows with every push, so a change
    /// in it is new output, whatever the buffer dropped.
    pub fn written(&self) -> usize {
        self.start + self.text.len()
    }

    pub fn push(&mut self, chunk: &str) {
        self.text.push_str(chunk);
        if self.text.len() > MAX_BUFFER_BYTES {
            let mut cut = self.text.len() - MAX_BUFFER_BYTES;
            while !self.text.is_char_boundary(cut) {
                cut += 1;
            }
            self.text.drain(..cut);
            self.start += cut;
        }
    }

    /// What was written since the last call, and whether some of it was
    /// lost before it could be read.
    pub fn take_unread(&mut self) -> (String, bool) {
        let missed = self.read_to < self.start;
        let from = self.read_to.saturating_sub(self.start);
        let unread = self.text[from..].to_string();
        self.read_to = self.start + self.text.len();
        (unread, missed)
    }

    /// The last `max_bytes` of what is kept, for a person looking at it.
    pub fn tail(&self, max_bytes: usize) -> &str {
        let mut from = self.text.len().saturating_sub(max_bytes);
        while !self.text.is_char_boundary(from) {
            from += 1;
        }
        &self.text[from..]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_returns_only_what_is_new() {
        let mut buffer = OutputBuffer::default();
        buffer.push("one\n");
        assert_eq!(buffer.take_unread(), ("one\n".to_string(), false));
        assert_eq!(buffer.take_unread(), (String::new(), false));
        buffer.push("two\n");
        assert_eq!(buffer.take_unread(), ("two\n".to_string(), false));
    }

    #[test]
    fn output_past_the_limit_drops_the_oldest_and_a_late_read_says_so() {
        let mut buffer = OutputBuffer::default();
        buffer.push("старт\n");
        buffer.push(&"x".repeat(MAX_BUFFER_BYTES));
        let (unread, missed) = buffer.take_unread();
        assert!(missed);
        assert_eq!(unread.len(), MAX_BUFFER_BYTES);

        buffer.push("é");
        let (unread, missed) = buffer.take_unread();
        assert_eq!((unread.as_str(), missed), ("é", false), "caught up again");
    }

    /// Never cut inside a character.
    #[test]
    fn the_buffer_is_cut_on_a_character_boundary() {
        let mut buffer = OutputBuffer::default();
        buffer.push("x");
        buffer.push(&"é".repeat(MAX_BUFFER_BYTES / 2));
        // Two bytes over: the cut would land inside the first `é`.
        buffer.push("y");
        assert!(buffer.take_unread().0.starts_with('é'));
        assert_eq!(buffer.tail(4), "éy");
    }

    #[test]
    fn ended_processes_are_told_one_line_each() {
        let info = |id, state| ProcessInfo { id, command: format!("cmd{id}"), cwd: ".".into(), state };
        assert_eq!(ended_note(&[]), None);
        let note = ended_note(&[
            info(1, ProcessState::Exited { code: Some(1) }),
            info(2, ProcessState::Exited { code: None }),
            info(3, ProcessState::Stopped),
        ])
        .unwrap();
        assert!(note.ends_with(
            "- #1 `cmd1` exited with code 1\n- #2 `cmd2` was ended by a signal\n- #3 `cmd3` was stopped"
        ), "{note}");
    }
}
