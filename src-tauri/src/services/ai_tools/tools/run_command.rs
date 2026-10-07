//! `runCommand` — the one tool that is not a filesystem operation.

use crate::domain::command_exec::{CommandError, CommandRequest, MAX_OUTPUT_CHARS};
use crate::domain::llm::LlmToolDefinition;
use crate::domain::tools::{ToolDeps, ToolError, ToolResult, ToolScope};
use crate::infra::process_runner::{self, Ran};

use super::super::resolve::resolve_existing;

pub fn run_command(
    scope: &ToolScope,
    request: &CommandRequest,
    deps: &ToolDeps,
) -> Result<ToolResult, ToolError> {
    if request.command.trim().is_empty() {
        return Err(ToolError::InvalidArguments {
            tool: "runCommand".to_string(),
            reason: "command must not be empty".to_string(),
        });
    }

    // The one path this tool has, resolved like every other. It keeps `cd`
    // honest; it does not contain the command, and nothing here should be read
    // as claiming otherwise — see `domain::command_exec`.
    let cwd = match &request.cwd {
        Some(path) if !path.is_empty() && path != "." => resolve_existing(scope, path)?,
        _ => scope.root().to_path_buf(),
    };

    if request.background == Some(true) {
        let processes = deps.processes.as_deref().ok_or_else(super::process::unavailable)?;
        let shown = request.cwd.as_deref().filter(|c| !c.is_empty()).unwrap_or(".");
        return Ok(ToolResult::ProcessStarted(processes.start(&deps.shell, &request.command, &cwd, shown)?));
    }

    // A note typed while it runs moves it to the background, so the model
    // hears the note now; one typed before it started has to wait its turn,
    // or every call after a note would be moved, each running beside the last.
    let typed = deps.notes_typed.map(|count| (count, count()));
    let woken = || typed.is_some_and(|(count, at)| count() != at);
    let stop = || deps.cancelled.is_some_and(|cancelled| cancelled());
    // With nowhere to move to — a hook, a test — the timeout kills as it always did.
    let ran = match deps.processes.as_deref() {
        Some(processes) => {
            let shown = request.cwd.as_deref().filter(|c| !c.is_empty()).unwrap_or(".");
            process_runner::run_or_move(&deps.shell, request, &cwd, shown, deps.output.as_ref(), processes, &woken, &stop)
        }
        None => process_runner::run(&deps.shell, request, &cwd, deps.output.as_ref()).map(Ran::Finished),
    };
    ran.map(|ran| match ran {
        Ran::Finished(output) => ToolResult::CommandRan(output),
        Ran::Moved { process, after_ms } => ToolResult::CommandMoved { process, after_ms },
    })
    .map_err(|e| match e {
            CommandError::Cwd(message) => ToolError::InvalidArguments {
                tool: "runCommand".to_string(),
                reason: message,
            },
            other => ToolError::Command(other.to_string()),
        })
}

/// What the model is told `runCommand` is for.
pub(super) fn definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "runCommand".to_string(),
        description: format!("Run a shell command in the workspace — build, test, lint, inspect. It runs under `/bin/sh -c` (`cmd.exe /C` on Windows), not the user's shell; what that sh really is here, the prompt says under \"Shell for commands\". PATH is the user's login PATH. Each call is a fresh shell: cd and exported variables do not carry over — use cwd. Nothing can type into it: a command that waits for input or opens an editor fails at once — pass --yes, -m and the like. The exit code, stdout and stderr all come back; a non-zero exit is an ordinary answer, not a failure of the call. The exit code is the whole line's — in `a; b` it is `b`'s, so use `&&` when an earlier failure matters. Past {MAX_OUTPUT_CHARS} characters a stream keeps its first and last {half} and marks what it dropped; both are saved whole for {days} days in the file the result names, outside the workspace — grep, head or tail it here instead of running the command again. A command still running at its timeout, or when the user writes to you while it runs, is not killed: the call returns and the command goes on as a background process — do not run it again, read it with readOutput, which can wait for it to end. For something that has to keep running — a dev server, a watcher — set background: the call returns at once with a process number, readOutput reads what it writes, stopProcess ends it, and you are told when one ends on its own.", half = MAX_OUTPUT_CHARS / 2, days = crate::infra::command_output_store::RETENTION_DAYS),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The command line, as you would type it. Pipes, redirection and `&&` work. Prefer the project's own tooling (its test runner, its formatter) over ad-hoc shell."
                },
                "cwd": {
                    "type": ["string", "null"],
                    "description": "Directory to run in, relative to the workspace root. Omit for the root itself."
                },
                "background": {
                    "type": ["boolean", "null"],
                    "description": "Keep it running after this call returns; no timeout applies. At most five at once."
                },
                "timeoutSeconds": {
                    "type": ["integer", "null"],
                    "minimum": 0,
                    "description": "How long to wait for it, up to 600. Omit for 120. A command still running then moves to the background, where it runs on — raise this for a slow build you want the result of in this call."
                }
            },
            "required": ["command"]
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::tools::{ReadFiles, ToolCall};
    use crate::services::ai_tools::tools::execute_tool;
    use crate::testing::temp_dir;
    use std::path::Path;

    fn ask(command: &str, cwd: Option<&str>) -> ToolCall {
        ToolCall::RunCommand(CommandRequest {
            command: command.to_string(),
            cwd: cwd.map(|c| c.to_string()),
            timeout_seconds: Some(10),
            background: None,
        })
    }

    fn run(root: &Path, call: ToolCall) -> Result<ToolResult, ToolError> {
        let scope = ToolScope::new(root).expect("scope");
        execute_tool(
            &scope,
            &call,
            &mut ReadFiles::default(),
            &mut Vec::new(),
            &ToolDeps::default(),
        )
    }

    fn output(result: ToolResult) -> crate::domain::command_exec::CommandOutput {
        match result {
            ToolResult::CommandRan(output) => output,
            other => panic!("expected a command result, got {other:?}"),
        }
    }

    #[test]
    fn a_command_runs_in_the_workspace_root_by_default() {
        let root = temp_dir("cmd-root");
        std::fs::write(root.join("here.txt"), "x").unwrap();

        let out = output(run(&root, ask("ls", None)).expect("runs"));

        assert!(out.stdout.contains("here.txt"), "{}", out.stdout);
        assert_eq!(out.exit_code, Some(0));
    }

    #[test]
    fn a_cwd_is_resolved_inside_the_workspace() {
        let root = temp_dir("cmd-cwd");
        std::fs::create_dir(root.join("crate")).unwrap();
        std::fs::write(root.join("crate/inner.txt"), "x").unwrap();

        let out = output(run(&root, ask("ls", Some("crate"))).expect("runs"));

        assert!(out.stdout.contains("inner.txt"), "{}", out.stdout);
    }

    /// The `cwd` goes through the same resolution as every other path, so it
    /// cannot point outside — which is a convenience, not containment: the
    /// command itself is free to name any path it likes.
    #[test]
    fn a_cwd_outside_the_workspace_is_refused() {
        let root = temp_dir("cmd-escape");
        let err = run(&root, ask("ls", Some("../.."))).expect_err("refused");
        assert!(matches!(err, ToolError::PathEscape(_)), "{err}");
    }

    #[test]
    fn a_cwd_that_does_not_exist_says_so() {
        let root = temp_dir("cmd-missing-cwd");
        let err = run(&root, ask("ls", Some("nope"))).expect_err("refused");
        assert!(matches!(err, ToolError::NotFound(_)), "{err}");
    }

    /// A model that sends an empty command has made a mistake it can fix;
    /// handing that to a shell produces a silent success instead.
    #[test]
    fn an_empty_command_is_refused() {
        let root = temp_dir("cmd-empty");
        let err = run(&root, ask("   ", None)).expect_err("refused");
        assert!(matches!(err, ToolError::InvalidArguments { .. }), "{err}");
    }

    /// The point of the tool: the model finds out that the thing it changed
    /// does not work.
    #[cfg(unix)]
    #[test]
    fn a_failing_command_comes_back_as_a_result_with_its_output() {
        let root = temp_dir("cmd-failing");

        let out = output(
            run(&root, ask("echo 'assertion failed' >&2; exit 1", None)).expect("runs"),
        );

        assert_eq!(out.exit_code, Some(1));
        assert!(out.stderr.contains("assertion failed"), "{}", out.stderr);
        assert!(!out.succeeded());
    }

    #[test]
    fn the_wire_shape_is_stable_and_counts_as_risky() {
        let call = ask("cargo test", None);
        let json = serde_json::to_value(&call).unwrap();
        assert_eq!(json["tool"], "runCommand");
        assert_eq!(json["args"]["command"], "cargo test");
        assert!(call.is_risky(), "a command line is never safe by its name alone");
    }
}
