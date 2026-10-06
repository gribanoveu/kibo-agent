//! Running a command: what is asked for, what comes back, and how much of it
//! the model is allowed to read.
//!
//! ## This is the tool that ends containment
//!
//! Every other tool resolves its paths against the workspace root, and that
//! root is a real boundary (`services::ai_tools::resolve`). A command line is
//! not: `cat ../../.ssh/id_rsa` is a perfectly ordinary command, and no amount
//! of checking the working directory changes that. So `cwd` is resolved inside
//! the root for the same reason a `cd` is convenient, **not** as a security
//! measure, and the honest statement is that from here on the approval gate is
//! the only boundary there is.
//!
//! A sandbox (CA-4.7) is what would change that. There is a place for one in
//! the runner's interface and no implementation behind it; until there is,
//! nothing here should be described as containing anything.
//!
//! ## Why a shell, and which one
//!
//! The command runs through a shell, because a coding agent's commands are
//! shell commands: `cargo test 2>&1 | tail -20`, `cd crate && make`. Parsing
//! an argv ourselves would reject those, and the model would have to discover
//! by trial which half of its vocabulary works.
//!
//! Which shell is a setting, not a search of `PATH`. On Windows the difference
//! between `cmd`, PowerShell and Git Bash is not cosmetic — the same line means
//! different things in each — and a tool that silently picks one produces
//! failures nobody can reproduce.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Used when a call names no timeout. Long enough for a test suite, short
/// enough that a command waiting on input nobody will type does not hold the
/// turn all day.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

/// The ceiling a call cannot raise. A build that genuinely takes longer is a
/// background process (CA-4.5), not a tool call the turn waits on.
pub const MAX_TIMEOUT: Duration = Duration::from_secs(600);

/// Per stream. Beyond this the middle is dropped — see [`truncate_output`].
pub const MAX_OUTPUT_CHARS: usize = 30_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CommandRequest {
    /// One command line, as it would be typed.
    pub command: String,
    /// Where to run it, relative to the workspace root. `None` is the root.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Seconds. `None` — and `0`, which is what a model sends when it means
    /// "no limit" — both take [`DEFAULT_TIMEOUT`]: of the two readings of an
    /// ambiguous zero, "kill it instantly" is the one that is never wanted.
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_u32")]
    pub timeout_seconds: Option<u32>,
    /// Keep it running after the call returns — see `domain::background`.
    /// The timeout does not apply.
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::domain::flexible_args::opt_bool")]
    pub background: Option<bool>,
}

impl CommandRequest {
    pub fn timeout(&self) -> Duration {
        match self.timeout_seconds {
            None | Some(0) => DEFAULT_TIMEOUT,
            Some(seconds) => Duration::from_secs(seconds as u64).min(MAX_TIMEOUT),
        }
    }
}

/// What a probe of the shell found: the version variables bash and zsh set —
/// which they still set when they stand in for `sh`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShellFound {
    pub bash: Option<String>,
    pub zsh: Option<String>,
}

/// The shell as the model is told it, each turn.
///
/// `/bin/sh` is not one program. On macOS it is bash 3.2 in sh mode, which
/// runs `[[ ]]` and arrays without a word; on Debian and Ubuntu — most Linux
/// CI — it is dash, which refuses them. A script the agent checked by running
/// it here proves nothing about the other, and saying "POSIX sh" while the
/// shell accepts bash is how that goes unnoticed. So the line names what the
/// shell really is. Only for `sh`: a shell set by name is what it says, and
/// one that would not answer the probe (`cmd.exe`) is named and nothing more.
pub fn describe_shell(program: &str, found: Option<&ShellFound>) -> String {
    let is_sh = std::path::Path::new(program).file_name().is_some_and(|name| name == "sh");
    let Some(found) = found.filter(|_| is_sh) else { return program.to_string() };
    // `3.2.57(1)-release` is `3.2.57` to anyone reading it.
    let version = |v: &str| v.split('(').next().unwrap_or(v).to_string();
    let standing_in = match (&found.bash, &found.zsh) {
        (Some(v), _) => format!("bash {}", version(v)),
        (None, Some(v)) => format!("zsh {}", version(v)),
        (None, None) => return format!("{program} — a plain POSIX sh: bash-only syntax fails here"),
    };
    format!(
        "{program} — really {standing_in} in sh mode: bash-only syntax ([[ ]], arrays, `source`) runs here \
         but fails where /bin/sh is dash (Debian, Ubuntu, most Linux CI). Write anything that will run \
         elsewhere — a script, a CI step — in POSIX sh, and check it with `shellcheck -s sh`, not by running it here"
    )
}

/// Which shell runs the command line, and how it is told to.
///
/// A setting rather than a search of `PATH`, for the reason in the module
/// doc. The defaults are only defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Shell {
    pub program: String,
    /// Everything before the command line itself — `["-c"]`, `["/C"]`.
    pub args: Vec<String>,
}

impl Default for Shell {
    fn default() -> Self {
        #[cfg(unix)]
        {
            Self {
                // `sh`, not the user's login shell: a command that works here
                // works for everyone on the project, which is not true of a
                // line that quietly depends on someone's zsh functions.
                program: "/bin/sh".to_string(),
                args: vec!["-c".to_string()],
            }
        }
        #[cfg(windows)]
        {
            Self {
                program: "cmd.exe".to_string(),
                args: vec!["/C".to_string()],
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OutputStream {
    Stdout,
    Stderr,
}

/// Output as it arrives, so a long command shows progress instead of looking
/// like a hang. One event type rather than two callbacks: a service that
/// reports several kinds of thing reports them through one enum.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandEvent {
    pub stream: OutputStream,
    pub chunk: String,
}

pub type CommandSink = Arc<dyn Fn(CommandEvent) + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    /// `None` when nothing exited normally — killed by the timeout, or by a
    /// signal. Distinguishing that from `Some(0)` is the whole question the
    /// model is asking.
    #[serde(default)]
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    /// Killed because the user pressed Stop while it ran.
    #[serde(default)]
    pub stopped: bool,
    /// Either stream had its middle dropped. Stated separately from the
    /// markers inside the text so a reader does not have to search for them.
    pub truncated: bool,
    /// From start to exit or kill. A slow build is worth knowing about before
    /// running it again, and the model has no clock of its own.
    #[serde(default)]
    pub duration_ms: u64,
    /// Where both streams were saved whole, when either was cut — so the
    /// model can read the middle rather than run the command again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_output: Option<String>,
}

impl CommandOutput {
    pub fn succeeded(&self) -> bool {
        self.exit_code == Some(0)
    }
}

#[derive(Debug, Error)]
pub enum CommandError {
    /// The command never started: no such shell, no such directory, no
    /// permission. Distinct from a command that started and failed, which is
    /// an ordinary [`CommandOutput`] with a non-zero code.
    #[error("could not start the command: {0}")]
    NotStarted(String),
    #[error("{0}")]
    Cwd(String),
    #[error("the command was started but could not be read: {0}")]
    Io(String),
}

/// `text` as a terminal would leave it: a carriage return goes back to the
/// start of the line, and what follows is written over it. A progress bar or
/// a countdown redraws one line hundreds of times; kept raw, every redraw
/// reached the model — and ran into the cut — glued into one line.
pub fn collapse_redraws(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains('\r') {
        return std::borrow::Cow::Borrowed(text);
    }
    // A Windows line end needs nothing of its own: the empty part after its
    // return writes over nothing.
    let lines: Vec<String> = text
        .split('\n')
        .map(|line| {
            let mut shown: Vec<char> = Vec::new();
            for part in line.split('\r') {
                let part: Vec<char> = part.chars().collect();
                let over = part.len().min(shown.len());
                shown.splice(..over, part);
            }
            shown.into_iter().collect()
        })
        .collect();
    std::borrow::Cow::Owned(lines.join("\n"))
}

/// Keeps the beginning and the end of `text`, dropping the middle — after
/// [`collapse_redraws`], so what is cut is what a terminal would have shown.
///
/// Head-only truncation is what a diff wants and the wrong shape here. A test
/// run's verdict is in the last lines (`test result: FAILED. 3 passed; 497
/// failed`) and the first real failure is near the top; cutting either end
/// throws away half of what was asked for. The marker says how many lines went
/// missing, so the model can ask for them specifically rather than re-running
/// the whole thing blind.
pub fn truncate_output(text: &str, max_chars: usize) -> (String, bool) {
    let text = &*collapse_redraws(text);
    if text.chars().count() <= max_chars {
        return (text.to_string(), false);
    }
    let lines: Vec<&str> = text.lines().collect();
    // Half the budget each way, less a little for the marker itself.
    let half = max_chars / 2;

    // Whole lines while they fit; then, if the one that did not is long, as
    // much of it as fits. Without that a log of a few very long lines loses
    // every line but the first and the last.
    let mut head: Vec<String> = Vec::new();
    let mut head_chars = 0;
    for line in &lines {
        let cost = line.chars().count() + 1;
        if head_chars + cost > half {
            let room = half - head_chars;
            if room >= PARTIAL_LINE_MIN {
                let kept: String = line.chars().take(room).collect();
                head_chars += room;
                head.push(format!("{kept}…"));
            }
            break;
        }
        head.push(line.to_string());
        head_chars += cost;
    }

    let mut tail: Vec<String> = Vec::new();
    let mut tail_chars = 0;
    for line in lines.iter().rev() {
        if tail.len() + head.len() >= lines.len() {
            break;
        }
        let cost = line.chars().count() + 1;
        if tail_chars + cost > half {
            let room = half - tail_chars;
            if room >= PARTIAL_LINE_MIN {
                let skip = line.chars().count() - room;
                let kept: String = line.chars().skip(skip).collect();
                tail_chars += room;
                tail.push(format!("…{kept}"));
            }
            break;
        }
        tail.push(line.to_string());
        tail_chars += cost;
    }
    tail.reverse();

    let dropped = lines.len() - head.len() - tail.len();
    let partial = head.last().is_some_and(|l| l.ends_with('…')) || tail.first().is_some_and(|l| l.starts_with('…'));
    let mut out = head.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    if partial {
        let omitted = text.chars().count().saturating_sub(head_chars + tail_chars);
        out.push_str(&format!("[... {omitted} characters omitted, {dropped} whole lines among them ...]\n"));
    } else {
        out.push_str(&format!("[... {dropped} lines omitted ...]\n"));
    }
    out.push_str(&tail.join("\n"));
    (out, true)
}

/// Below this, the end of a line is not worth keeping: a fragment that short
/// says nothing and reads as garbage.
const PARTIAL_LINE_MIN: usize = 80;

#[cfg(test)]
mod tests {
    use super::*;

    fn found(bash: Option<&str>, zsh: Option<&str>) -> ShellFound {
        ShellFound { bash: bash.map(Into::into), zsh: zsh.map(Into::into) }
    }

    /// macOS: `sh` is bash, which lets bash through — the model is told,
    /// and told what that does not prove.
    #[test]
    fn an_sh_that_is_bash_says_so_and_what_it_does_not_prove() {
        let line = describe_shell("/bin/sh", Some(&found(Some("3.2.57(1)-release"), None)));
        assert!(line.starts_with("/bin/sh — really bash 3.2.57 in sh mode:"), "{line}");
        assert!(line.contains("fails where /bin/sh is dash") && line.contains("shellcheck -s sh"), "{line}");
        let zsh = describe_shell("/bin/sh", Some(&found(None, Some("5.9"))));
        assert!(zsh.starts_with("/bin/sh — really zsh 5.9 in sh mode:"), "{zsh}");
    }

    #[test]
    fn a_plain_sh_is_named_as_one() {
        assert_eq!(
            describe_shell("/bin/sh", Some(&found(None, None))),
            "/bin/sh — a plain POSIX sh: bash-only syntax fails here"
        );
    }

    /// Nothing is claimed that the probe did not find, or about a shell
    /// set by name.
    #[test]
    fn an_unprobed_or_named_shell_is_only_named() {
        assert_eq!(describe_shell("/bin/sh", None), "/bin/sh");
        assert_eq!(describe_shell("/bin/bash", Some(&found(Some("5.2"), None))), "/bin/bash");
        assert_eq!(describe_shell("cmd.exe", None), "cmd.exe");
    }

    #[test]
    fn a_carriage_return_writes_over_the_line_as_a_terminal_would() {
        let countdown = "\rleft: 60 s\rleft: 59 s\rleft:  9 s";
        assert_eq!(collapse_redraws(countdown), "left:  9 s");
        // A shorter redraw leaves the end of the longer one, as on screen.
        assert_eq!(collapse_redraws("50% done\r75%"), "75% done");
        // A line left at a return keeps what it showed.
        assert_eq!(collapse_redraws("step 1\n42%\r"), "step 1\n42%");
        // Windows line ends are line ends, not redraws.
        assert_eq!(collapse_redraws("a\r\nb\r\n"), "a\nb\n");
        assert_eq!(collapse_redraws("plain\ntext"), "plain\ntext");
    }

    #[test]
    fn the_cut_counts_what_a_terminal_would_show() {
        let bar: String = (0..=1000).map(|i| format!("\r{i:>4}/1000")).collect();
        let (out, cut) = truncate_output(&format!("start\n{bar}\ndone"), 200);
        assert_eq!((out.as_str(), cut), ("start\n1000/1000\ndone", false));
    }

    fn request(timeout_seconds: Option<u32>) -> CommandRequest {
        CommandRequest {
            command: "cargo test".to_string(),
            cwd: None,
            timeout_seconds,
            background: None,
        }
    }

    #[test]
    fn an_unspecified_timeout_is_the_default() {
        assert_eq!(request(None).timeout(), DEFAULT_TIMEOUT);
    }

    /// A model that sends `0` means "no limit", never "kill it immediately" —
    /// and of the two readings only one is ever wanted.
    #[test]
    fn a_zero_timeout_is_read_as_unspecified() {
        assert_eq!(request(Some(0)).timeout(), DEFAULT_TIMEOUT);
    }

    #[test]
    fn a_timeout_is_honoured_up_to_the_ceiling() {
        assert_eq!(request(Some(5)).timeout(), Duration::from_secs(5));
        assert_eq!(request(Some(100_000)).timeout(), MAX_TIMEOUT);
    }

    /// "Started and failed" and "never started" are different answers, and
    /// only one of them is about the command the model wrote.
    #[test]
    fn a_non_zero_exit_is_a_result_not_an_error() {
        let failed = CommandOutput {
            exit_code: Some(1),
            ..Default::default()
        };
        assert!(!failed.succeeded());
        assert!(!failed.timed_out);
    }

    #[test]
    fn a_killed_command_has_no_exit_code() {
        let killed = CommandOutput {
            timed_out: true,
            exit_code: None,
            ..Default::default()
        };
        assert!(!killed.succeeded(), "silence is not success");
    }

    #[test]
    fn short_output_is_left_alone() {
        let (out, truncated) = truncate_output("one\ntwo\n", 100);
        assert_eq!(out, "one\ntwo\n");
        assert!(!truncated);
    }

    /// The verdict is at the end and the first failure is near the top. Losing
    /// either end throws away half of what was asked for.
    #[test]
    fn truncation_keeps_both_ends() {
        let mut lines: Vec<String> = vec!["first failure: assertion failed".to_string()];
        lines.extend((0..500).map(|i| format!("noise line {i}")));
        lines.push("test result: FAILED. 3 passed; 497 failed".to_string());
        let text = lines.join("\n");

        let (out, truncated) = truncate_output(&text, 400);

        assert!(truncated);
        assert!(out.starts_with("first failure"), "{out}");
        assert!(out.trim_end().ends_with("497 failed"), "{out}");
        assert!(out.chars().count() < text.chars().count());
    }

    /// The model has to be able to ask for what it did not get, rather than
    /// re-running the whole thing to see the middle.
    #[test]
    fn truncation_says_how_much_went_missing() {
        let text: String = (0..100)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");

        let (out, _) = truncate_output(&text, 120);

        let marker = out
            .lines()
            .find(|l| l.contains("omitted"))
            .expect("the cut is marked");
        let dropped: usize = marker
            .chars()
            .filter(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .expect("a count");
        let kept = out.lines().count() - 1;
        assert_eq!(dropped + kept, 100, "every line is either kept or counted");
    }

    /// Cutting by bytes would panic here, and cutting mid-line would make the
    /// output read as corrupted rather than merely incomplete.
    #[test]
    fn truncation_is_safe_on_multibyte_text() {
        let text: String = (0..200)
            .map(|i| format!("строка {i} — проверка"))
            .collect::<Vec<_>>()
            .join("\n");

        let (out, truncated) = truncate_output(&text, 300);

        assert!(truncated);
        assert!(out.contains("строка 0"));
        assert!(out.contains("строка 199"));
    }

    /// One line longer than the whole budget still has to come back as
    /// something, and still has to say that it was cut.
    /// A few long lines — a minified bundle, a JSON dump — keep what fits
    /// of the lines at the cut instead of losing all of them.
    #[test]
    fn long_lines_at_the_cut_keep_what_fits_of_them() {
        let text = ["A", "B", "C", "D"].map(|c| c.repeat(9_000)).join("\n");

        let (out, truncated) = truncate_output(&text, 30_000);

        assert!(truncated);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 5, "A, part of B, the mark, part of C, D");
        assert!(lines[1].starts_with('B') && lines[1].ends_with('…'), "{}", &lines[1][..20]);
        assert!(lines[3].starts_with('…') && lines[3].ends_with('C'));
        assert!(lines[2].starts_with("[... ") && lines[2].contains("characters omitted, 0 whole lines among them"), "{}", lines[2]);
        let omitted: usize = lines[2].split_whitespace().nth(1).unwrap().parse().unwrap();
        let kept: usize = [0, 1, 3, 4].iter().map(|&i| lines[i].trim_matches('…').chars().count()).sum();
        // Two newlines kept (after A, before D); the one between B and C went.
        assert_eq!(kept + omitted + 2, text.chars().count(), "every character kept or counted");
        assert!(out.chars().count() <= 30_000 + 100);
    }

    #[test]
    fn a_single_enormous_line_is_still_reported_as_cut() {
        let text = "x".repeat(10_000);
        let (out, truncated) = truncate_output(&text, 100);
        assert!(truncated);
        assert!(out.contains("omitted"), "{out}");
    }
}
