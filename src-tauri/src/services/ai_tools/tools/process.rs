//! `readOutput` and `stopProcess` — the other two thirds of a background
//! process; `runCommand` with `background` is the first.

use std::time::{Duration, Instant};

use crate::domain::background::BackgroundProcesses;
use crate::domain::llm::LlmToolDefinition;
use crate::domain::tools::{ProcessArgs, ReadOutputArgs, ToolDeps, ToolError, ToolResult};

/// The longest `waitSeconds` is taken at — `runCommand`'s own ceiling.
const MAX_WAIT_SECONDS: u32 = 600;

/// How often a wait looks at the process. It is our own registry, not a
/// child: a tenth of a second is soon enough for a build that took minutes.
const POLL: Duration = Duration::from_millis(100);

pub fn read_output(args: &ReadOutputArgs, deps: &ToolDeps) -> Result<ToolResult, ToolError> {
    let (processes, id) = target("readOutput", args.id, deps)?;
    if let Some(seconds) = args.wait_seconds.filter(|s| *s > 0) {
        wait(processes, id, wait_limit(seconds), deps);
    }
    Ok(ToolResult::ProcessOutput(processes.read(id)?))
}

/// `waitSeconds` as asked, up to [`MAX_WAIT_SECONDS`].
fn wait_limit(seconds: u32) -> Duration {
    Duration::from_secs(seconds.min(MAX_WAIT_SECONDS).into())
}

/// Until `id` ends, the time is up, the turn is stopped, or the user writes
/// — what they wrote is to be answered now, not after the build.
fn wait(processes: &dyn BackgroundProcesses, id: u32, limit: Duration, deps: &ToolDeps) {
    let deadline = Instant::now() + limit;
    let typed = deps.notes_typed.map(|count| (count, count()));
    let running = || processes.list().iter().any(|p| p.id == id && p.running());
    while Instant::now() < deadline
        && running()
        && !deps.cancelled.is_some_and(|stop| stop())
        && !typed.is_some_and(|(count, at)| count() != at)
    {
        std::thread::sleep(POLL);
    }
}

pub fn stop_process(args: &ProcessArgs, deps: &ToolDeps) -> Result<ToolResult, ToolError> {
    let (processes, id) = target("stopProcess", args.id, deps)?;
    Ok(ToolResult::ProcessStopped(processes.stop(id)?))
}

fn target<'a>(
    tool: &str,
    id: Option<u32>,
    deps: &'a ToolDeps,
) -> Result<(&'a dyn BackgroundProcesses, u32), ToolError> {
    let id = id.ok_or_else(|| ToolError::InvalidArguments {
        tool: tool.to_string(),
        reason: "id is required: the number runCommand returned when it started the process".to_string(),
    })?;
    let processes = deps.processes.as_deref().ok_or_else(unavailable)?;
    Ok((processes, id))
}

pub(super) fn unavailable() -> ToolError {
    ToolError::Command("background processes are not available here".to_string())
}

pub(super) fn read_definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "readOutput".to_string(),
        description: "Read what a background process (started by runCommand with background: true) has written since you last read it — stdout and stderr together, in order — and whether it is still running or how it ended. Each call returns only new output. Check it after starting a server to see it came up, and before relying on it. To wait for a build or test run moved to the background, set waitSeconds rather than calling it again and again."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "id": { "type": "integer", "description": "The process number runCommand returned." },
                "waitSeconds": {
                    "type": "integer",
                    "description": "Wait up to this long, at most 600, for the process to end before reading. The wait ends early when the user writes to you."
                }
            },
            "required": ["id"]
        }),
    }
}

pub(super) fn stop_definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "stopProcess".to_string(),
        description: "Stop a background process and everything it started. Stop what you no longer need: at most five run at once."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "id": { "type": "integer", "description": "The process number runCommand returned." }
            },
            "required": ["id"]
        }),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::domain::background::{BackgroundError, ProcessOutput, ProcessState};
    use crate::domain::command_exec::CommandRequest;
    use crate::domain::tools::{ReadFiles, ToolCall, ToolScope};
    use crate::infra::background::Processes;
    use crate::services::ai_tools::tools::execute_tool;
    use crate::testing::temp_dir;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn run(root: &std::path::Path, deps: &ToolDeps, call: ToolCall) -> Result<ToolResult, ToolError> {
        execute_tool(&ToolScope::new(root).unwrap(), &call, &mut ReadFiles::default(), &mut Vec::new(), deps)
    }

    fn background(command: &str, cwd: Option<&str>) -> ToolCall {
        ToolCall::RunCommand(CommandRequest {
            command: command.into(),
            cwd: cwd.map(Into::into),
            timeout_seconds: Some(1),
            background: Some(true),
        })
    }

    /// Start, read, stop — the three tools over one process, which outlives
    /// its one-second timeout because a background process has none.
    #[test]
    fn a_process_is_started_read_and_stopped_through_the_tools() {
        let root = temp_dir("tool-bg");
        std::fs::create_dir(root.join("app")).unwrap();
        let deps = ToolDeps { processes: Some(Arc::new(Processes::default())), ..ToolDeps::default() };

        let ToolResult::ProcessStarted(info) = run(&root, &deps, background("pwd; echo ready; sleep 30", Some("app"))).unwrap() else {
            panic!("expected a start");
        };
        assert_eq!((info.id, info.cwd.as_str(), info.state), (1, "app", ProcessState::Running));

        let read = ToolCall::ReadOutput(ReadOutputArgs { id: Some(1), ..Default::default() });
        let mut seen = String::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !seen.contains("ready") {
            assert!(Instant::now() < deadline, "no output: {seen:?}");
            let ToolResult::ProcessOutput(out) = run(&root, &deps, read.clone()).unwrap() else { panic!() };
            seen.push_str(&out.output);
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(seen.contains("/app\n"), "{seen}");
        std::thread::sleep(Duration::from_millis(1200));
        let ToolResult::ProcessOutput(out) = run(&root, &deps, read).unwrap() else { panic!() };
        assert!(out.process.running(), "the timeout did not apply");

        let ToolResult::ProcessStopped(stopped) =
            run(&root, &deps, ToolCall::StopProcess(ProcessArgs { id: Some(1) })).unwrap()
        else {
            panic!()
        };
        assert_eq!(stopped.state, ProcessState::Stopped);
    }

    fn read_until(root: &std::path::Path, deps: &ToolDeps, id: u32, done: impl Fn(&ProcessOutput) -> bool) -> String {
        let mut seen = String::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let ToolResult::ProcessOutput(out) = run(root, deps, ToolCall::ReadOutput(ReadOutputArgs { id: Some(id), ..Default::default() })).unwrap() else {
                panic!()
            };
            seen.push_str(&out.output);
            if done(&out) {
                return seen;
            }
            assert!(Instant::now() < deadline, "not yet: {seen:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// A command still running at its timeout goes on in the background:
    /// nothing it wrote before or after the move is lost, and it ends as it
    /// would have.
    #[test]
    fn a_command_that_outlasts_its_timeout_moves_to_the_background() {
        let root = temp_dir("tool-move");
        let deps = ToolDeps { processes: Some(Arc::new(Processes::default())), ..ToolDeps::default() };
        let mut call = background("echo before; echo oops >&2; sleep 1.5; echo after; echo late >&2; exit 4", None);
        let ToolCall::RunCommand(request) = &mut call else { unreachable!() };
        request.background = None;

        let ToolResult::CommandMoved { process, after_ms } = run(&root, &deps, call).unwrap() else {
            panic!("expected a move");
        };
        assert_eq!((process.id, process.state), (1, ProcessState::Running));
        assert!((1000..1500).contains(&after_ms), "{after_ms}");

        let seen = read_until(&root, &deps, 1, |out| !out.process.running());
        assert!(seen.contains("before\n") && seen.contains("oops\n") && seen.contains("after\n") && seen.contains("late\n"), "{seen:?}");
        let ToolResult::ProcessOutput(end) = run(&root, &deps, ToolCall::ReadOutput(ReadOutputArgs { id: Some(1), ..Default::default() })).unwrap() else {
            panic!()
        };
        assert_eq!(end.process.state, ProcessState::Exited { code: Some(4) });
    }

    /// No room in the background: killed at its timeout, as before.
    #[test]
    fn with_the_background_full_a_command_is_killed_at_its_timeout() {
        let root = temp_dir("tool-move-full");
        let processes = Arc::new(Processes::default());
        for _ in 0..crate::domain::background::MAX_RUNNING {
            processes.start(&Default::default(), "sleep 30", &root, ".").unwrap();
        }
        let deps = ToolDeps { processes: Some(processes), ..ToolDeps::default() };
        let mut call = background("echo before; sleep 30", None);
        let ToolCall::RunCommand(request) = &mut call else { unreachable!() };
        request.background = None;

        let ToolResult::CommandRan(out) = run(&root, &deps, call).unwrap() else { panic!("expected a kill") };
        assert!(out.timed_out && out.exit_code.is_none());
        assert_eq!(out.stdout, "before\n");
    }

    /// A count of notes that goes up `after` from now, from another thread.
    fn typed_after(after: Duration) -> Arc<std::sync::atomic::AtomicU64> {
        let typed = Arc::new(std::sync::atomic::AtomicU64::new(3));
        let later = typed.clone();
        std::thread::spawn(move || {
            std::thread::sleep(after);
            later.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
        typed
    }

    fn foreground(command: &str, timeout: u32) -> ToolCall {
        ToolCall::RunCommand(CommandRequest {
            command: command.into(),
            cwd: None,
            timeout_seconds: Some(timeout),
            background: None,
        })
    }

    /// The user writes during a build: it goes on in the background at once,
    /// not at its timeout, so the model can answer them now.
    #[test]
    fn a_note_typed_while_a_command_runs_moves_it_to_the_background() {
        let root = temp_dir("tool-move-note");
        let typed = typed_after(Duration::from_secs(1));
        let count = || typed.load(std::sync::atomic::Ordering::SeqCst);
        let deps = ToolDeps { processes: Some(Arc::new(Processes::default())), notes_typed: Some(&count), ..ToolDeps::default() };

        // Timed around the call, not by `after_ms`: that starts at the spawn,
        // and the first spawn of a run waits on the login shell's PATH.
        let started = Instant::now();
        let ToolResult::CommandMoved { process, .. } = run(&root, &deps, foreground("echo building; sleep 30", 60)).unwrap() else {
            panic!("expected a move");
        };
        assert!((1000..5000).contains(&started.elapsed().as_millis()), "{:?}", started.elapsed());
        assert!(process.running());
        let seen = read_until(&root, &deps, process.id, |out| !out.output.is_empty());
        assert_eq!(seen, "building\n");
    }

    /// A note already waiting when the command starts is not a reason to
    /// move it — the next call of the round would move too, and run beside it.
    #[test]
    fn a_note_typed_before_a_command_started_lets_it_finish() {
        let root = temp_dir("tool-move-old-note");
        let count = || 7;
        let deps = ToolDeps { processes: Some(Arc::new(Processes::default())), notes_typed: Some(&count), ..ToolDeps::default() };

        let ToolResult::CommandRan(out) = run(&root, &deps, foreground("sleep 0.3; echo done", 60)).unwrap() else {
            panic!("expected it to finish");
        };
        assert_eq!(out.stdout, "done\n");
    }

    /// With no room in the background a note does not move it: the move
    /// would be refused, and a refused move ends the command.
    #[test]
    fn with_the_background_full_a_note_lets_the_command_finish() {
        let root = temp_dir("tool-move-note-full");
        let processes = Arc::new(Processes::default());
        for _ in 0..crate::domain::background::MAX_RUNNING {
            processes.start(&Default::default(), "sleep 30", &root, ".").unwrap();
        }
        let typed = typed_after(Duration::from_millis(100));
        let count = || typed.load(std::sync::atomic::Ordering::SeqCst);
        let deps = ToolDeps { processes: Some(processes), notes_typed: Some(&count), ..ToolDeps::default() };

        let ToolResult::CommandRan(out) = run(&root, &deps, foreground("sleep 0.6; echo done", 60)).unwrap() else {
            panic!("expected it to finish");
        };
        assert_eq!((out.stdout.as_str(), out.exit_code), ("done\n", Some(0)));
    }

    fn wait_for(id: u32, seconds: u32) -> ToolCall {
        ToolCall::ReadOutput(ReadOutputArgs { id: Some(id), wait_seconds: Some(seconds) })
    }

    /// A model that asks for an hour waits ten minutes, `runCommand`'s ceiling.
    #[test]
    fn a_wait_is_capped_at_ten_minutes() {
        assert_eq!(wait_limit(30), Duration::from_secs(30));
        assert_eq!(wait_limit(3600), Duration::from_secs(600));
    }

    /// `waitSeconds` reads once the process has ended, not before.
    #[test]
    fn read_output_waits_for_the_process_to_end() {
        let root = temp_dir("tool-wait-end");
        let deps = ToolDeps { processes: Some(Arc::new(Processes::default())), ..ToolDeps::default() };
        run(&root, &deps, background("sleep 0.5; echo done", None)).unwrap();

        let started = Instant::now();
        let ToolResult::ProcessOutput(out) = run(&root, &deps, wait_for(1, 20)).unwrap() else { panic!() };
        assert_eq!(out.process.state, ProcessState::Exited { code: Some(0) });
        assert_eq!(out.output, "done\n");
        assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
    }

    /// The wait ends at its limit, when the user writes, or when the turn
    /// is stopped — the process runs on each time.
    #[test]
    fn read_output_stops_waiting_at_its_limit_a_note_or_a_stop() {
        let root = temp_dir("tool-wait-early");
        let processes = Arc::new(Processes::default());
        let plain = ToolDeps { processes: Some(processes.clone()), ..ToolDeps::default() };
        run(&root, &plain, background("sleep 30", None)).unwrap();

        let started = Instant::now();
        let ToolResult::ProcessOutput(out) = run(&root, &plain, wait_for(1, 1)).unwrap() else { panic!() };
        assert!(out.process.running());
        assert!((1000..3000).contains(&started.elapsed().as_millis()), "{:?}", started.elapsed());

        let typed = typed_after(Duration::from_millis(200));
        let count = || typed.load(std::sync::atomic::Ordering::SeqCst);
        let noted = ToolDeps { processes: Some(processes.clone()), notes_typed: Some(&count), ..ToolDeps::default() };
        let started = Instant::now();
        let ToolResult::ProcessOutput(out) = run(&root, &noted, wait_for(1, 20)).unwrap() else { panic!() };
        assert!(out.process.running());
        assert!(started.elapsed() < Duration::from_secs(3), "a note: {:?}", started.elapsed());

        let stop_at = Instant::now() + Duration::from_millis(200);
        let stop = || Instant::now() >= stop_at;
        let stopped = ToolDeps { processes: Some(processes), cancelled: Some(&stop), ..ToolDeps::default() };
        let started = Instant::now();
        let ToolResult::ProcessOutput(out) = run(&root, &stopped, wait_for(1, 20)).unwrap() else { panic!() };
        assert!(out.process.running());
        assert!(started.elapsed() < Duration::from_secs(3), "a stop: {:?}", started.elapsed());
    }

    #[test]
    fn what_the_model_gets_wrong_is_said_to_it() {
        let root = temp_dir("tool-bg-wrong");
        let deps = ToolDeps { processes: Some(Arc::new(Processes::default())), ..ToolDeps::default() };
        let missing = run(&root, &deps, ToolCall::ReadOutput(ReadOutputArgs { id: None, ..Default::default() })).unwrap_err();
        assert!(missing.to_string().contains("id is required"), "{missing}");
        let unknown = run(&root, &deps, ToolCall::StopProcess(ProcessArgs { id: Some(9) })).unwrap_err();
        assert!(matches!(unknown, ToolError::Background(BackgroundError::NotFound(9))));
        let escape = run(&root, &deps, background("true", Some("../.."))).unwrap_err();
        assert!(matches!(escape, ToolError::PathEscape(_)), "{escape}");

        let none = ToolDeps::default();
        for call in [background("true", None), ToolCall::ReadOutput(ReadOutputArgs { id: Some(1), ..Default::default() })] {
            let err = run(&root, &none, call).unwrap_err();
            assert!(err.to_string().contains("not available"), "{err}");
        }
    }
}
