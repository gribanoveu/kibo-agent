//! Starting a command, reading what it says, and making sure it is dead.
//!
//! ## Killing the tree, not the child
//!
//! `Child::kill()` signals exactly one process. A shell command is almost never
//! one process: `cargo test` spawns compilers and test binaries, and
//! `sh -c 'server & sleep 30'` leaves the server running after the shell it was
//! started from is gone. A timeout that kills only the direct child therefore
//! *looks* like it worked — the call returns — while the work it was supposed
//! to stop keeps running, holding the port, the lock, or the CPU.
//!
//! So the child is started in its own process group and the whole group is
//! signalled. That is what the grandchild test checks, and it is the one test
//! in this suite that has to wait in real time.
//!
//! ## Reading output
//!
//! Both streams are read by their own threads, because a command that fills the
//! pipe it is not being read from blocks forever, and because the model should
//! see a long build progressing rather than nothing until the end. Output is
//! read as bytes, not as lines of text: a compiler is free to emit a byte
//! sequence that is not UTF-8, and a reader that errors on it would lose the
//! whole run over one character.

use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::domain::background::{BackgroundProcesses, Feed, ProcessInfo};
use crate::domain::command_exec::{
    CommandError, CommandEvent, CommandOutput, CommandRequest, CommandSink, OutputStream,
    MAX_OUTPUT_CHARS, Shell, ShellFound, collapse_redraws, truncate_output,
};
use crate::infra::command_output_store;
use crate::sync::lock;

/// How often the child is checked while waiting. Short enough that a timeout
/// is accurate to a blink, long enough not to spin a core.
const POLL: Duration = Duration::from_millis(25);

/// Runs `request` to completion, to its timeout, or to a failure to start.
///
/// `cwd` is absolute and must already have been checked against whatever
/// boundary the caller keeps — this layer does not resolve paths. See
/// `domain::command_exec`'s module doc for why that boundary is thinner here
/// than for every other tool.
pub fn run(
    shell: &Shell,
    request: &CommandRequest,
    cwd: &Path,
    events: Option<&CommandSink>,
) -> Result<CommandOutput, CommandError> {
    run_with(shell, request, cwd, events, None, &[])
}

/// How a command run by [`run_or_move`] came back.
#[derive(Debug)]
pub enum Ran {
    Finished(CommandOutput),
    /// Still running at its timeout, it went on as this background process
    /// — after `after_ms` in the foreground.
    Moved { process: ProcessInfo, after_ms: u64 },
}

/// [`run`], except that a command still running at its timeout is not
/// killed but handed to `processes`, output so far and all: a build that
/// needed longer, or a server started without `background`, goes on instead
/// of being lost with everything it did. Killed after all when `processes`
/// is full.
pub fn run_or_move(
    shell: &Shell,
    request: &CommandRequest,
    cwd: &Path,
    shown_cwd: &str,
    events: Option<&CommandSink>,
    processes: &dyn BackgroundProcesses,
) -> Result<Ran, CommandError> {
    let adopt = |child: Child, backlog: &str| processes.adopt(child, &request.command, shown_cwd, backlog).ok();
    run_inner(shell, request, cwd, events, None, &[], Some(&adopt))
}

/// Takes a child at its deadline, with what it wrote so far; `None` when it
/// was refused, and the child ended.
type Adopt<'a> = &'a dyn Fn(Child, &str) -> Option<(ProcessInfo, [Feed; 2])>;

/// [`run`], with `input` written to the command's stdin and closed, and
/// `env` added to its environment — what a hook is given.
pub fn run_with(
    shell: &Shell,
    request: &CommandRequest,
    cwd: &Path,
    events: Option<&CommandSink>,
    input: Option<&str>,
    env: &[(&str, &str)],
) -> Result<CommandOutput, CommandError> {
    match run_inner(shell, request, cwd, events, input, env, None)? {
        Ran::Finished(output) => Ok(output),
        Ran::Moved { .. } => unreachable!("nothing to move to without `adopt`"),
    }
}

fn run_inner(
    shell: &Shell,
    request: &CommandRequest,
    cwd: &Path,
    events: Option<&CommandSink>,
    input: Option<&str>,
    env: &[(&str, &str)],
    adopt: Option<Adopt>,
) -> Result<Ran, CommandError> {
    if !cwd.is_dir() {
        return Err(CommandError::Cwd(format!(
            "{} is not a directory",
            cwd.display()
        )));
    }

    let mut command = Command::new(&shell.program);
    command
        .args(&shell.args)
        .arg(&request.command)
        .current_dir(cwd)
        // A command that waits for input nobody is going to type must fail at
        // once rather than hold the turn until the timeout.
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .envs(env.iter().copied())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    set_process_group(&mut command);
    super::login_path::apply(&mut command);

    let mut child = command
        .spawn()
        .map_err(|e| CommandError::NotStarted(format!("{}: {e}", shell.program)))?;

    // From a thread of its own: a command that does not read its stdin
    // before writing a pipe-full of output would otherwise deadlock with us.
    // Dropping the pipe afterwards is the end of input it may be waiting for.
    if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
        let input = input.to_string();
        thread::spawn(move || {
            let _ = std::io::Write::write_all(&mut stdin, input.as_bytes());
        });
    }

    let (stdout_to, stdout) = collect(child.stdout.take(), OutputStream::Stdout, events.cloned());
    let (stderr_to, stderr) = collect(child.stderr.take(), OutputStream::Stderr, events.cloned());

    let started = Instant::now();
    let deadline = started + request.timeout();
    let mut timed_out = false;
    let mut child = Some(child);
    let status = loop {
        let Some(running) = child.as_mut() else { break None };
        match running.try_wait() {
            Ok(Some(status)) => {
                // Killed even though it exited by itself. A command is allowed
                // to leave something running — `sh -c 'server & exit 0'` — and
                // that something inherited the write end of these pipes, so
                // the readers below would wait for *it* to finish, with the
                // timeout already behind us. A background process that outlives
                // its turn is CA-4.5 and is not this tool; here it is a call
                // that never returns.
                kill_tree(running);
                break Some(status);
            }
            Ok(None) => {}
            Err(e) => return Err(CommandError::Io(e.to_string())),
        }
        if Instant::now() >= deadline {
            timed_out = true;
            if let Some(adopt) = adopt {
                // Both streams held while it changes hands, so no chunk falls
                // between the backlog and the feeds.
                let mut out = lock(&stdout_to);
                let mut err = lock(&stderr_to);
                let backlog = format!("{}{}", out.kept(), err.kept());
                let Some(taken) = child.take() else { break None };
                if let Some((process, [out_feed, err_feed])) = adopt(taken, &backlog) {
                    *out = Target::Feed(out_feed);
                    *err = Target::Feed(err_feed);
                    let after_ms = started.elapsed().as_millis() as u64;
                    return Ok(Ran::Moved { process, after_ms });
                }
                // Refused and ended: reported as a timeout, as before.
                break None;
            }
            kill_tree(running);
            // Reaped so the process does not linger as a zombie; whatever it
            // exited with is not an answer to anything now.
            let _ = running.wait();
            break None;
        }
        thread::sleep(POLL);
    };

    // After the kill either way, so the readers see EOF and finish instead of
    // holding the call open for as long as whatever inherited the pipe runs.
    let _ = stdout.join();
    let _ = stderr.join();
    let stdout = lock(&stdout_to).take();
    let stderr = lock(&stderr_to).take();

    let (shown_out, stdout_cut) = truncate_output(&stdout, MAX_OUTPUT_CHARS);
    let (shown_err, stderr_cut) = truncate_output(&stderr, MAX_OUTPUT_CHARS);
    let truncated = stdout_cut || stderr_cut;
    let full_output = truncated
        .then(|| command_output_store::save(&collapse_redraws(&stdout), &collapse_redraws(&stderr)))
        .flatten()
        .map(|path| path.display().to_string());

    Ok(Ran::Finished(CommandOutput {
        stdout: shown_out,
        stderr: shown_err,
        exit_code: status.and_then(|s| s.code()),
        timed_out,
        truncated,
        duration_ms: started.elapsed().as_millis() as u64,
        full_output,
    }))
}

/// Where a reader puts what it reads: kept for the result, or — once the
/// command has moved to the background — fed to the process it became.
enum Target {
    Keep(String),
    Feed(Feed),
}

impl Target {
    fn kept(&self) -> &str {
        match self {
            Target::Keep(text) => text,
            Target::Feed(_) => "",
        }
    }

    fn take(&mut self) -> String {
        match self {
            Target::Keep(text) => std::mem::take(text),
            Target::Feed(_) => String::new(),
        }
    }
}

/// Reads one stream to EOF on its own thread, reporting as it goes — into
/// the target it returns, which a move to the background redirects.
fn collect(
    mut pipe: Option<impl Read + Send + 'static>,
    stream: OutputStream,
    events: Option<CommandSink>,
) -> (Arc<Mutex<Target>>, thread::JoinHandle<()>) {
    let target = Arc::new(Mutex::new(Target::Keep(String::new())));
    let to = Arc::clone(&target);
    let reader = thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        while let Some(pipe) = pipe.as_mut() {
            match pipe.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    // Lossy on purpose: a chunk boundary can fall inside a
                    // multi-byte character, and losing one character is better
                    // than losing the run.
                    let chunk = String::from_utf8_lossy(&buffer[..n]).into_owned();
                    match &mut *lock(&to) {
                        Target::Keep(text) => {
                            text.push_str(&chunk);
                            if let Some(events) = &events {
                                events(CommandEvent { stream, chunk });
                            }
                        }
                        Target::Feed(feed) => feed(Some(&chunk)),
                    }
                }
            }
        }
        if let Target::Feed(feed) = &mut *lock(&to) {
            feed(None);
        }
    });
    (target, reader)
}

/// Asks the shell what it really is — see `domain::command_exec::describe_shell`.
/// `None` when it would not say: it failed to start, or it has no `-c` to ask
/// with (`cmd.exe`).
#[cfg(unix)]
pub fn probe_shell(shell: &Shell) -> Option<ShellFound> {
    // Not through `run`: that would resolve the login `PATH` first — a login
    // shell, seconds on a heavy `.zshrc` — and `printf` is a builtin.
    let out = Command::new(&shell.program)
        .args(&shell.args)
        .arg(r#"printf '%s\n%s\n' "${BASH_VERSION:-}" "${ZSH_VERSION:-}""#)
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut lines = stdout.lines().map(str::trim);
    let mut next = || lines.next().filter(|v| !v.is_empty()).map(String::from);
    Some(ShellFound { bash: next(), zsh: next() })
}

#[cfg(not(unix))]
pub fn probe_shell(_shell: &Shell) -> Option<ShellFound> {
    None
}

#[cfg(unix)]
pub(crate) fn set_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    // 0 means "a new group led by the child", so its own children join it and
    // one signal reaches all of them.
    command.process_group(0);
}

#[cfg(not(unix))]
pub(crate) fn set_process_group(_command: &mut Command) {}

#[cfg(unix)]
pub(crate) fn kill_tree(child: &mut Child) {
    // Negative pid addresses the group. The child leads its own group (see
    // `set_process_group`), so this is the group and nothing outside it.
    let pid = child.id() as i32;
    unsafe {
        libc::kill(-pid, libc::SIGKILL);
    }
}

#[cfg(not(unix))]
pub(crate) fn kill_tree(child: &mut Child) {
    // `taskkill /T` walks the process tree at the moment it runs, so a
    // grandchild whose parent has already exited is not found — a job object
    // would catch those too. Written this way deliberately: an untested
    // `windows-sys` job object is a worse answer than a documented ceiling,
    // and this is the upgrade path when Windows is a supported target
    // (05-gaps §5.4).
    let _ = Command::new("taskkill")
        .args(["/T", "/F", "/PID", &child.id().to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = child.kill();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::temp_dir;

    fn ask(command: &str, timeout_seconds: Option<u32>) -> CommandRequest {
        CommandRequest {
            command: command.to_string(),
            cwd: None,
            timeout_seconds,
            background: None,
        }
    }

    /// After a move the readers feed the process it became, to the end: its
    /// output, then each pipe's close — without which its watch never ends.
    #[cfg(unix)]
    #[test]
    fn a_moved_command_feeds_its_streams_to_their_close() {
        use crate::domain::background::ProcessState;
        let fed = Arc::new(Mutex::new(Vec::<Option<String>>::new()));
        let adopted = Arc::new(Mutex::new(None::<(Child, String)>));
        let (sink, keep) = (Arc::clone(&fed), Arc::clone(&adopted));
        let adopt = move |child: Child, backlog: &str| {
            *keep.lock().unwrap() = Some((child, backlog.to_string()));
            let feed = || {
                let sink = Arc::clone(&sink);
                Box::new(move |text: Option<&str>| sink.lock().unwrap().push(text.map(String::from))) as Feed
            };
            let info = ProcessInfo { id: 1, command: String::new(), cwd: ".".into(), state: ProcessState::Running };
            Some((info, [feed(), feed()]))
        };

        let ran = run_inner(&Shell::default(), &ask("echo before; sleep 1.3; echo after", Some(1)), &temp_dir("run-move"), None, None, &[], Some(&adopt))
            .unwrap();
        assert!(matches!(ran, Ran::Moved { .. }), "{ran:?}");
        let (mut child, backlog) = adopted.lock().unwrap().take().unwrap();
        assert_eq!(backlog, "before\n");
        child.wait().unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        while fed.lock().unwrap().iter().filter(|f| f.is_none()).count() < 2 {
            assert!(Instant::now() < deadline, "{:?}", fed.lock().unwrap());
            thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(fed.lock().unwrap().iter().flatten().cloned().collect::<String>(), "after\n");
    }

    fn run_in(dir: &Path, command: &str) -> CommandOutput {
        run(&Shell::default(), &ask(command, Some(10)), dir, None).expect("runs")
    }

    /// What a hook is given: the event on stdin, the project in its env.
    #[cfg(unix)]
    /// A cut result says where the whole was saved; one that fits saves nothing.
    #[cfg(unix)]
    #[test]
    fn a_cut_output_is_saved_whole() {
        crate::testing::with_app_dir("cmd-full-output", || {
            let dir = temp_dir("cmd-full-output");
            let long = run_in(&dir, "seq 1 20000");
            assert!(long.truncated);
            let saved = std::fs::read_to_string(long.full_output.expect("saved")).unwrap();
            assert_eq!(saved.lines().count(), 20000, "the middle the result dropped is there");
            assert!(saved.contains("\n10000\n"));

            assert_eq!(run_in(&dir, "echo short").full_output, None);
        });
    }

    /// What the probe hears is each shell's own variables.
    #[cfg(unix)]
    #[test]
    fn a_probe_asks_the_shell_what_it_is() {
        let bash = Shell { program: "/bin/bash".into(), args: vec!["-c".into()] };
        let heard = probe_shell(&bash).expect("bash answers");
        assert!(heard.bash.is_some_and(|v| v.chars().next().is_some_and(|c| c.is_ascii_digit())));
        assert_eq!(heard.zsh, None);
        if Path::new("/bin/zsh").exists() {
            let zsh = probe_shell(&Shell { program: "/bin/zsh".into(), args: vec!["-c".into()] }).expect("zsh answers");
            assert!(zsh.zsh.is_some() && zsh.bash.is_none(), "{zsh:?}");
        }
        assert!(probe_shell(&Shell::default()).is_some(), "sh answers, whatever it is");
    }

    /// No answer, no claim.
    #[cfg(unix)]
    #[test]
    fn a_shell_that_does_not_answer_is_not_described() {
        let missing = Shell { program: "/no/such/shell".into(), args: vec!["-c".into()] };
        assert_eq!(probe_shell(&missing), None);
        // Runs `exit 3`; the probe's line is only its `$0`.
        let failing = Shell { program: "/bin/sh".into(), args: vec!["-c".into(), "exit 3".into()] };
        assert_eq!(probe_shell(&failing), None);
    }

    #[test]
    fn input_and_environment_reach_the_command() {
        let dir = temp_dir("run-input");
        let out = run_with(
            &Shell::default(),
            &ask("cat; printf ' %s' \"$HOOK_TEST\"", Some(10)),
            &dir,
            None,
            Some("{\"a\":1}"),
            &[("HOOK_TEST", "here")],
        )
        .expect("runs");
        assert_eq!(out.stdout, "{\"a\":1} here");
    }

    #[test]
    fn a_command_reports_its_output_and_its_code() {
        let dir = temp_dir("run-basic");
        let out = run_in(&dir, "echo hello");

        assert_eq!(out.stdout.trim(), "hello");
        assert_eq!(out.exit_code, Some(0));
        assert!(out.succeeded());
        assert!(!out.timed_out);
    }

    /// A command that started and failed is a result, not an error: the
    /// non-zero code is the answer the model asked for.
    #[test]
    fn a_failing_command_is_a_result() {
        let out = run_in(&temp_dir("run-failing"), "exit 3");
        assert_eq!(out.exit_code, Some(3));
        assert!(!out.succeeded());
    }

    #[test]
    fn the_two_streams_stay_apart() {
        let out = run_in(&temp_dir("run-streams"), "echo out; echo err >&2");
        assert_eq!(out.stdout.trim(), "out");
        assert_eq!(out.stderr.trim(), "err");
    }

    #[test]
    fn the_command_runs_where_it_was_told_to() {
        let dir = temp_dir("run-cwd");
        std::fs::write(dir.join("marker.txt"), "x").unwrap();
        let out = run_in(&dir, "ls");
        assert!(out.stdout.contains("marker.txt"), "{}", out.stdout);
    }

    #[test]
    fn a_missing_directory_is_refused_before_anything_starts() {
        let missing = temp_dir("run-cwd-missing").join("gone");
        let err = run(&Shell::default(), &ask("echo hi", None), &missing, None)
            .expect_err("no such directory");
        assert!(matches!(err, CommandError::Cwd(_)), "{err}");
    }

    #[test]
    fn a_shell_that_does_not_exist_says_so() {
        let shell = Shell {
            program: "/nonexistent/shell".to_string(),
            args: vec!["-c".to_string()],
        };
        let err = run(&shell, &ask("echo hi", None), &temp_dir("run-no-shell"), None)
            .expect_err("no such shell");
        assert!(matches!(err, CommandError::NotStarted(_)), "{err}");
    }

    /// Nothing is ever going to type an answer. Waiting for one until the
    /// timeout wastes the turn on a command that can only fail.
    #[test]
    fn a_command_waiting_for_input_ends_at_once() {
        let before = Instant::now();
        // `&&`, not `;`: with stdin closed `read` fails, and what is under
        // test is that it fails at once instead of waiting for a line.
        let out = run_in(&temp_dir("run-stdin"), "read line && echo \"got $line\"");
        assert!(before.elapsed() < Duration::from_secs(5), "it waited");
        assert!(!out.stdout.contains("got"), "{}", out.stdout);
    }

    #[test]
    fn output_is_streamed_as_it_arrives() {
        let seen: Arc<Mutex<Vec<CommandEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let sink_events = seen.clone();
        let sink: CommandSink = Arc::new(move |event| {
            sink_events.lock().unwrap().push(event);
        });

        let out = run(
            &Shell::default(),
            &ask("echo one; echo two >&2", Some(10)),
            &temp_dir("run-stream"),
            Some(&sink),
        )
        .expect("runs");

        let seen = seen.lock().unwrap();
        let text: String = seen.iter().map(|e| e.chunk.clone()).collect();
        assert!(text.contains("one") && text.contains("two"), "{text}");
        assert!(
            seen.iter().any(|e| e.stream == OutputStream::Stderr),
            "the stream each chunk came from is part of the report"
        );
        assert_eq!(out.stdout.trim(), "one");
    }

    #[test]
    fn a_flood_of_output_is_cut_and_says_so() {
        let out = run_in(
            &temp_dir("run-flood"),
            "for i in $(seq 1 20000); do echo \"line $i of noise\"; done",
        );
        assert!(out.truncated);
        assert!(out.stdout.contains("omitted"), "the cut is marked");
        assert!(out.stdout.contains("line 1 of noise"), "the beginning survives");
        assert!(out.stdout.contains("line 20000 of noise"), "and so does the end");
    }

    /// Output that is not valid UTF-8 must cost one character, not the run.
    #[test]
    fn output_that_is_not_text_does_not_lose_the_run() {
        let out = run_in(&temp_dir("run-binary"), "printf 'a\\xff\\xfeb'; exit 0");
        assert_eq!(out.exit_code, Some(0));
        assert!(out.stdout.contains('a') && out.stdout.contains('b'), "{:?}", out.stdout);
    }

    #[test]
    fn a_command_that_runs_too_long_is_stopped() {
        let out = run(
            &Shell::default(),
            &ask("sleep 30", Some(1)),
            &temp_dir("run-timeout"),
            None,
        )
        .expect("returns rather than hanging");

        assert!(out.timed_out);
        assert_eq!(out.exit_code, None, "killed, not exited");
        assert!(!out.succeeded(), "silence is not success");
    }

    /// A command may exit while something it started is still running, and
    /// that something inherited the pipes. Waiting for *it* to finish would
    /// hold the call open long past the timeout — indefinitely, for a daemon —
    /// and the timeout has already been left behind at that point.
    ///
    /// Found by a mutation run: with the group kill removed, the timeout tests
    /// took thirty seconds instead of one, which is the same mechanism seen
    /// from the other side.
    #[test]
    fn a_command_that_leaves_something_running_still_returns() {
        let before = Instant::now();
        let out = run(
            &Shell::default(),
            &ask("( sleep 30 ) & exit 0", Some(20)),
            &temp_dir("run-daemon"),
            None,
        )
        .expect("returns");

        assert!(
            before.elapsed() < Duration::from_secs(5),
            "waited {:?} for a process the command left behind",
            before.elapsed()
        );
        assert_eq!(out.exit_code, Some(0));
        assert!(!out.timed_out, "it exited on its own");
    }

    /// The reason the process group exists. `Child::kill()` would end the
    /// shell and leave `sleep` running, and the call would report success at
    /// stopping something that is still going.
    ///
    /// The one test here that waits in real time: the grandchild has to be
    /// given its chance to write before the absence of the file means
    /// anything.
    #[test]
    fn a_timeout_kills_the_grandchildren_too() {
        let dir = temp_dir("run-tree");
        let marker = dir.join("survivor.txt");
        let command = format!(
            "( sleep 2; echo alive > {} ) & sleep 30",
            marker.display()
        );

        let out = run(
            &Shell::default(),
            &ask(&command, Some(1)),
            &dir,
            None,
        )
        .expect("returns");
        assert!(out.timed_out);

        thread::sleep(Duration::from_secs(3));
        assert!(
            !marker.exists(),
            "the grandchild outlived the kill and wrote {}",
            marker.display()
        );
    }
}
