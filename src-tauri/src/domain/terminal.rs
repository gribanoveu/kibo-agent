//! The user's own terminal: their login shell in a real PTY, drawn by xterm.js
//! in the Terminal tab. Not the agent's `runCommand` — that runs `/bin/sh` on
//! pipes, with a timeout and a cap on what the model reads; this one is a
//! person typing, so it gets a TTY and nothing is cut.
//!
//! The agent reaches them through [`UserTerminals`]: it reads what a screen
//! shows and types a command into one, never more.
//!
//! The shells are in `infra::terminal`; decisions in `docs/15-terminal.md`.

use std::collections::VecDeque;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Output kept per terminal while nothing draws it, replayed on the next
/// attach: a few screens of a busy build, not its whole history.
pub const MAX_SCROLLBACK_BYTES: usize = 512 * 1024;

/// Lines of history kept past the screen, for `readTerminal`.
pub const SCREEN_HISTORY: usize = 1000;

/// How far past a cut the replay looks for a line start. A line longer than
/// this is replayed from the middle rather than not at all.
const LINE_START_SEARCH: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalInfo {
    pub id: u32,
    /// What runs in it, for the tab's title: `zsh`, `cmd`.
    pub shell: String,
    pub state: TerminalState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum TerminalState {
    Running,
    /// The shell ended — `exit`, Ctrl+D, a crash. The tab stays until it is
    /// closed, so what it last printed can still be read. `None` when a signal
    /// ended it.
    Exited { code: Option<u32> },
}

/// Columns and rows, as xterm.js measured them. Never zero: a PTY that size
/// makes every full-screen program divide by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalSize {
    pub cols: u16,
    pub rows: u16,
}

impl TerminalSize {
    pub fn new(cols: u16, rows: u16) -> Result<Self, TerminalError> {
        if cols == 0 || rows == 0 {
            return Err(TerminalError::BadSize { cols, rows });
        }
        Ok(Self { cols, rows })
    }
}

/// Terminal `id` started, ended or was closed. A signal to read the list
/// again; the output itself goes to the attached [`TerminalOutputSink`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalChanged {
    pub id: u32,
}

pub type TerminalEventSink = Arc<dyn Fn(TerminalChanged) + Send + Sync>;

/// Where a terminal's output goes while it is on screen: raw bytes, escape
/// sequences and all — xterm.js is the one that reads them.
pub type TerminalOutputSink = Arc<dyn Fn(&[u8]) + Send + Sync>;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TerminalError {
    #[error("there is no terminal #{0}")]
    NotFound(u32),
    #[error("could not start the shell: {0}")]
    NotStarted(String),
    #[error("a terminal cannot be {cols}×{rows}")]
    BadSize { cols: u16, rows: u16 },
    #[error("terminal #{0} has ended")]
    Ended(u32),
    #[error("could not resize terminal #{id}: {reason}")]
    Resize { id: u32, reason: String },
    #[error("the user has no terminal open — the Terminal tab opens one")]
    NoneOpen,
}

/// What a terminal shows, as text: its last lines, history included,
/// without colours or cursor movements.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalScreen {
    pub id: u32,
    pub shell: String,
    pub state: TerminalState,
    /// A full-screen program — `vim`, `less`, `htop` — is drawing it; there
    /// is no history behind that screen.
    pub alternate: bool,
    /// The lines, oldest first. `output`, so the call log redacts it.
    pub output: String,
    /// Lines left out at the front by [`TerminalScreen::fit`].
    #[serde(default)]
    pub cut: usize,
}

impl TerminalScreen {
    /// At most `max` characters, the newest kept: whole lines go from the
    /// front, and a last line longer than all of it keeps its end. A screen
    /// is read for what happened last, unlike a command's output, whose
    /// first lines say what it was doing.
    pub fn fit(mut self, max: usize) -> Self {
        let mut total = self.output.chars().count();
        let lines: Vec<&str> = self.output.split('\n').collect();
        let mut from = 0;
        while total > max && from + 1 < lines.len() {
            total -= lines[from].chars().count() + 1;
            from += 1;
        }
        let kept = lines[from..].join("\n");
        self.output = kept.chars().skip(total.saturating_sub(max)).collect();
        self.cut = from;
        self
    }
}

/// The user's terminals as the agent sees them.
pub trait UserTerminals: Send + Sync {
    fn list(&self) -> Vec<TerminalInfo>;

    /// The last `lines` lines of terminal `id`, or of the newest one.
    fn screen(&self, id: Option<u32>, lines: usize) -> Result<TerminalScreen, TerminalError>;
}

/// The last [`MAX_SCROLLBACK_BYTES`] a terminal wrote. Bytes, not text: a
/// chunk may end inside a character, and xterm.js joins them.
#[derive(Debug, Default)]
pub struct Scrollback {
    bytes: VecDeque<u8>,
}

impl Scrollback {
    pub fn push(&mut self, chunk: &[u8]) {
        self.bytes.extend(chunk);
        let excess = self.bytes.len().saturating_sub(MAX_SCROLLBACK_BYTES);
        if excess == 0 {
            return;
        }
        self.bytes.drain(..excess);
        // Start the replay on a line: a cut in the middle of one may be in
        // the middle of an escape sequence, which would draw as garbage.
        if let Some(newline) = self.bytes.iter().take(LINE_START_SEARCH).position(|&b| b == b'\n') {
            self.bytes.drain(..=newline);
        }
    }

    pub fn to_vec(&self) -> Vec<u8> {
        self.bytes.iter().copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_size_is_never_zero() {
        assert_eq!(TerminalSize::new(80, 24), Ok(TerminalSize { cols: 80, rows: 24 }));
        assert_eq!(TerminalSize::new(0, 24), Err(TerminalError::BadSize { cols: 0, rows: 24 }));
        assert_eq!(TerminalSize::new(80, 0), Err(TerminalError::BadSize { cols: 80, rows: 0 }));
    }

    #[test]
    fn the_scrollback_keeps_everything_under_the_limit() {
        let mut scrollback = Scrollback::default();
        scrollback.push(b"one\n");
        scrollback.push(b"\x1b[31mtwo");
        assert_eq!(scrollback.to_vec(), b"one\n\x1b[31mtwo");
    }

    /// Past the limit the oldest goes, up to the next line start.
    #[test]
    fn the_scrollback_drops_the_oldest_and_starts_on_a_line() {
        let mut scrollback = Scrollback::default();
        scrollback.push(b"\x1b[1mfirst\nsecond\n");
        // Nine over: the cut takes the escape and `first`, and leaves its newline.
        scrollback.push(&vec![b'a'; MAX_SCROLLBACK_BYTES - 8]);
        let kept = scrollback.to_vec();
        assert!(kept.starts_with(b"second\naaa"), "{:?}", String::from_utf8_lossy(&kept[..20]));
        assert_eq!(kept.len(), MAX_SCROLLBACK_BYTES - 8 + "second\n".len());
    }

    /// One endless line is replayed from its middle, not dropped whole.
    #[test]
    fn a_line_longer_than_the_search_is_kept_from_the_cut() {
        let mut scrollback = Scrollback::default();
        scrollback.push(&vec![b'a'; MAX_SCROLLBACK_BYTES]);
        scrollback.push(b"b\n");
        let kept = scrollback.to_vec();
        assert_eq!(kept.len(), MAX_SCROLLBACK_BYTES);
        assert!(kept.ends_with(b"ab\n"));
    }

    fn screen(output: &str) -> TerminalScreen {
        TerminalScreen { id: 1, shell: "zsh".into(), state: TerminalState::Running, alternate: false, output: output.into(), cut: 0 }
    }

    #[test]
    fn a_screen_that_fits_is_left_as_it_is() {
        assert_eq!(screen("ab\ncd").fit(5), screen("ab\ncd"));
    }

    /// Whole lines from the front, counted — in characters, not bytes.
    #[test]
    fn a_screen_too_long_keeps_its_last_lines() {
        let fitted = screen("один\nдва\nтри\nчетыре").fit(10);
        assert_eq!((fitted.output.as_str(), fitted.cut), ("три\nчетыре", 2));
        // Exactly at the limit, nothing more goes.
        assert_eq!(screen("один\nдва").fit(8).cut, 0);
        assert_eq!(screen("один\nдва").fit(7).output, "два");
    }

    #[test]
    fn a_last_line_longer_than_the_limit_keeps_its_end() {
        let fitted = screen("first\n0123456789").fit(4);
        assert_eq!((fitted.output.as_str(), fitted.cut), ("6789", 1));
    }

    #[test]
    fn the_state_is_tagged_for_the_frontend() {
        let info = TerminalInfo { id: 2, shell: "zsh".into(), state: TerminalState::Exited { code: Some(3) } };
        assert_eq!(
            serde_json::to_value(info).unwrap(),
            serde_json::json!({ "id": 2, "shell": "zsh", "state": { "state": "exited", "code": 3 } })
        );
    }
}
