//! `readOutput` and `stopProcess` — the other two thirds of a background
//! process; `runCommand` with `background` is the first.

use crate::domain::background::BackgroundProcesses;
use crate::domain::llm::LlmToolDefinition;
use crate::domain::tools::{ProcessArgs, ToolDeps, ToolError, ToolResult};

pub fn read_output(args: &ProcessArgs, deps: &ToolDeps) -> Result<ToolResult, ToolError> {
    let (processes, id) = target("readOutput", args, deps)?;
    Ok(ToolResult::ProcessOutput(processes.read(id)?))
}

pub fn stop_process(args: &ProcessArgs, deps: &ToolDeps) -> Result<ToolResult, ToolError> {
    let (processes, id) = target("stopProcess", args, deps)?;
    Ok(ToolResult::ProcessStopped(processes.stop(id)?))
}

fn target<'a>(
    tool: &str,
    args: &ProcessArgs,
    deps: &'a ToolDeps,
) -> Result<(&'a dyn BackgroundProcesses, u32), ToolError> {
    let id = args.id.ok_or_else(|| ToolError::InvalidArguments {
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
        description: "Read what a background process (started by runCommand with background: true) has written since you last read it — stdout and stderr together, in order — and whether it is still running or how it ended. Each call returns only new output. Check it after starting a server to see it came up, and before relying on it."
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

        let read = ToolCall::ReadOutput(ProcessArgs { id: Some(1) });
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
            let ToolResult::ProcessOutput(out) = run(root, deps, ToolCall::ReadOutput(ProcessArgs { id: Some(id) })).unwrap() else {
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
        let ToolResult::ProcessOutput(end) = run(&root, &deps, ToolCall::ReadOutput(ProcessArgs { id: Some(1) })).unwrap() else {
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

    #[test]
    fn what_the_model_gets_wrong_is_said_to_it() {
        let root = temp_dir("tool-bg-wrong");
        let deps = ToolDeps { processes: Some(Arc::new(Processes::default())), ..ToolDeps::default() };
        let missing = run(&root, &deps, ToolCall::ReadOutput(ProcessArgs { id: None })).unwrap_err();
        assert!(missing.to_string().contains("id is required"), "{missing}");
        let unknown = run(&root, &deps, ToolCall::StopProcess(ProcessArgs { id: Some(9) })).unwrap_err();
        assert!(matches!(unknown, ToolError::Background(BackgroundError::NotFound(9))));
        let escape = run(&root, &deps, background("true", Some("../.."))).unwrap_err();
        assert!(matches!(escape, ToolError::PathEscape(_)), "{escape}");

        let none = ToolDeps::default();
        for call in [background("true", None), ToolCall::ReadOutput(ProcessArgs { id: Some(1) })] {
            let err = run(&root, &none, call).unwrap_err();
            assert!(err.to_string().contains("not available"), "{err}");
        }
    }
}
