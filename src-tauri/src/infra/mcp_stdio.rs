//! An MCP server started as a process and spoken to over its stdin and
//! stdout.
//!
//! The process is this module's: its `PATH`, its group, its stderr, its exit
//! code and its end. The protocol over the two pipes is `rmcp`'s
//! (`infra::mcp_rmcp`), which is handed them once the process runs.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::domain::mcp::{McpError, McpServerConfig};
use crate::infra::mcp_rmcp::{Failure, RmcpClient};
use crate::infra::process_runner::{kill_tree, set_process_group};
use crate::sync::lock;

/// Lines of the server's stderr kept to explain an exit.
const STDERR_LINES: usize = 20;
const STDERR_LINE_CHARS: usize = 500;
/// How long an exit waits for the rest of stderr when the pipe stays open.
const STDERR_GRACE: Duration = Duration::from_secs(1);

/// The server's process. Dropping it kills the process and everything it
/// started — `npx` runs the real server as a child, and killing only `npx`
/// would leave that one behind.
struct Process(Mutex<Child>);

impl Drop for Process {
    fn drop(&mut self) {
        stop(&mut lock(&self.0));
    }
}

/// Starts the process in `cwd` and opens the session with it, within the
/// server's own timeout — a first `npx` run downloads the package. The
/// process lives as long as the client: its group, its `PATH`, its stderr and
/// its exit code are this module's, and the SDK is handed the two pipes.
pub fn start(config: &McpServerConfig, cwd: &Path, cancelled: &dyn Fn() -> bool) -> Result<RmcpClient, McpError> {
    let mut command = Command::new(&config.command);
    // Before the entry's own `env`, which may set a `PATH` of its own.
    crate::infra::login_path::apply(&mut command);
    command
        .args(&config.args)
        .envs(&config.env)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    set_process_group(&mut command);
    let mut child = command.spawn().map_err(|e| McpError::NotStarted(format!("{}: {e}", config.command)))?;

    let (Some(stdin), Some(stdout), Some(stderr)) = (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        stop(&mut child);
        return Err(McpError::NotStarted("its standard streams were not available".into()));
    };
    let tail = keep_tail(stderr);
    let process = Arc::new(Process(Mutex::new(child)));
    let (exited, tail_too) = (Arc::clone(&process), Arc::clone(&tail));
    // Whatever the pipes report, what happened is that the process went.
    let describe = move |_: Failure<'_>| gone(&exited, &tail);
    // The stream closing is how an exit shows first; the process is asked
    // too, for one that is gone while its stdout is still held open by a
    // child of its own.
    let ended = move || {
        let running = matches!(lock(&process.0).try_wait(), Ok(None));
        (!running).then(|| gone(&process, &tail_too))
    };
    let pipes = || {
        let not_started = |e: std::io::Error| McpError::NotStarted(e.to_string());
        Ok((
            tokio::process::ChildStdout::from_std(stdout).map_err(not_started)?,
            tokio::process::ChildStdin::from_std(stdin).map_err(not_started)?,
        ))
    };
    let timeout = Duration::from_secs(config.timeout_secs());
    // A failure drops both closures, and with them the process.
    RmcpClient::connect(pipes, timeout, cancelled, Box::new(describe), Box::new(ended))
}

fn gone(process: &Process, tail: &Tail) -> McpError {
    let code = exit_code(&process.0);
    // What it wrote on its way out may still be in the pipe: read to the
    // pipe's end, or for a second — a child of its own can hold it open.
    let deadline = Instant::now() + STDERR_GRACE;
    while !tail.read.load(Ordering::Acquire) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    McpError::Exited { code, stderr: lock(&tail.lines).iter().cloned().collect::<Vec<_>>().join("\n") }
}

fn stop(child: &mut Child) {
    kill_tree(child);
    // The server itself as well, by its pid: if the group signal missed
    // it for any reason, `wait` below would block the app on a process
    // that is still reading its stdin.
    let _ = child.kill();
    let _ = child.wait();
}

/// The stream closed because the process is exiting; give it a moment to
/// finish so its code can be reported, rather than none.
fn exit_code(child: &Mutex<Child>) -> Option<i32> {
    let deadline = Instant::now() + Duration::from_millis(500);
    loop {
        if let Ok(Some(status)) = lock(child).try_wait() {
            return status.code();
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The last lines of stderr, and whether the pipe has been read to its end.
#[derive(Default)]
struct Tail {
    lines: Mutex<VecDeque<String>>,
    read: AtomicBool,
}

/// The last lines of stderr, read as they come so the pipe never fills and
/// stalls the server.
fn keep_tail(stderr: impl std::io::Read + Send + 'static) -> Arc<Tail> {
    let tail: Arc<Tail> = Arc::default();
    let kept = Arc::clone(&tail);
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            let mut lines = lock(&kept.lines);
            if lines.len() == STDERR_LINES {
                lines.pop_front();
            }
            lines.push_back(line.chars().take(STDERR_LINE_CHARS).collect());
        }
        kept.read.store(true, Ordering::Release);
    });
    tail
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::mcp::{McpAnswer, McpClient};
    use serde_json::json;
    use std::collections::BTreeMap;

    fn sh(script: &str, timeout_secs: u64) -> McpServerConfig {
        McpServerConfig {
            command: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
            timeout_secs: Some(timeout_secs),
            ..Default::default()
        }
    }

    /// The whole path through a process: spawn, pipes, handshake, a call —
    /// a server written in `sh`, reading one line per request. It is of the
    /// `initialize` era, so it refuses the probe that comes first.
    #[cfg(unix)]
    #[test]
    fn a_process_is_started_and_spoken_to() {
        let script = r#"
            read discover; echo '{"jsonrpc":"2.0","id":0,"error":{"code":-32601,"message":"Method not found"}}'
            read init; echo '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-11-25","capabilities":{},"serverInfo":{"name":"sh","version":"1"},"instructions":"Say hi."}}'
            read initialized
            read call; echo '{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}'
            echo '{"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"hello '"$MCP_TEST_TOKEN"'"}]}}'
            read _
        "#;
        let dir = crate::testing::temp_dir("mcp-stdio-process");
        // `env` is where a server's token lives; it has to reach the process.
        let config = McpServerConfig { env: [("MCP_TEST_TOKEN".to_string(), "t0k3n".to_string())].into(), ..sh(script, 5) };
        let server = start(&config, &dir, &|| false).unwrap();
        assert_eq!(server.instructions().as_deref(), Some("Say hi."));
        assert!(!server.tools_stale());
        assert_eq!(server.call_tool("hi", json!({}), &|| false).unwrap().text, "hello t0k3n");
        let heard = Instant::now() + Duration::from_secs(3);
        while !server.tools_stale() && Instant::now() < heard {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(server.tools_stale(), "the server said its tools changed");
    }

    /// A server's prompts and questions reach it through the process too.
    #[cfg(unix)]
    #[test]
    fn prompts_and_questions_go_through_the_process() {
        let script = r#"
            read discover; echo '{"jsonrpc":"2.0","id":0,"error":{"code":-32601,"message":"Method not found"}}'
            read init; echo '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-11-25","capabilities":{"prompts":{}},"serverInfo":{"name":"sh","version":"1"}}}'
            read initialized
            read list; echo '{"jsonrpc":"2.0","id":2,"result":{"prompts":[{"name":"greet"}]}}'
            read get; echo '{"jsonrpc":"2.0","id":3,"result":{"messages":[{"role":"user","content":{"type":"text","text":"Hello."}}]}}'
            read call; echo '{"jsonrpc":"2.0","id":4,"result":{"resultType":"input_required","inputRequests":{"q":{"method":"elicitation/create","params":{"mode":"url","message":"Sign in","url":"https://a.example/in"}}}}}'
            read retry; echo '{"jsonrpc":"2.0","id":5,"result":{"content":[{"type":"text","text":"in"}]}}'
            read _
        "#;
        let dir = crate::testing::temp_dir("mcp-stdio-prompts");
        let server = start(&sh(script, 5), &dir, &|| false).unwrap();
        assert_eq!(server.list_prompts().unwrap()[0].name, "greet");
        assert_eq!(server.get_prompt("greet", &BTreeMap::new()).unwrap(), "Hello.");
        let asked = std::sync::Mutex::new(Vec::new());
        let done = server
            .call_tool_asking("login", json!({}), &|| false, &|q| {
                asked.lock().unwrap().push(q.clone());
                McpAnswer::Accept { content: Default::default() }
            })
            .unwrap();
        assert_eq!(done.text, "in");
        assert_eq!(asked.lock().unwrap().len(), 1);
    }

    /// Why an exit is worth its own variant: the code and the last lines of
    /// stderr are usually the whole explanation.
    #[cfg(unix)]
    #[test]
    fn a_process_that_dies_reports_its_code_and_stderr() {
        let dir = crate::testing::temp_dir("mcp-stdio-dies");
        let result = start(&sh("echo 'Error: GITHUB_TOKEN is not set' >&2; exit 7", 5), &dir, &|| false);
        let Err(err) = result else { panic!("started") };
        assert_eq!(err, McpError::Exited { code: Some(7), stderr: "Error: GITHUB_TOKEN is not set".into() });
    }

    /// The exit can be seen before the last of stderr is read: what the
    /// server wrote on its way out is waited for, as far as the pipe's end.
    #[cfg(unix)]
    #[test]
    fn stderr_still_on_its_way_when_the_process_exits_is_reported() {
        let dir = crate::testing::temp_dir("mcp-stdio-late-stderr");
        let result = start(&sh("(sleep 0.2; echo 'Error: late' >&2) >/dev/null & exit 7", 5), &dir, &|| false);
        let Err(err) = result else { panic!("started") };
        assert_eq!(err, McpError::Exited { code: Some(7), stderr: "Error: late".into() });
    }

    /// Read to its end, stderr is not waited on any longer; held open by a
    /// child of the server's, only for its grace, not for as long as the child.
    #[cfg(unix)]
    #[test]
    fn an_exit_waits_for_stderr_only_while_it_may_still_come() {
        let dir = crate::testing::temp_dir("mcp-stdio-stderr-wait");
        let began = Instant::now();
        let _ = start(&sh("echo gone >&2; exit 3", 30), &dir, &|| false);
        assert!(began.elapsed() < STDERR_GRACE * 3 / 4, "an ended pipe was waited on: {:?}", began.elapsed());

        let began = Instant::now();
        let result = start(&sh("sleep 30 >/dev/null & exit 3", 30), &dir, &|| false);
        assert!(matches!(result, Err(McpError::Exited { code: Some(3), .. })));
        assert!(began.elapsed() < STDERR_GRACE * 5, "waited on the child: {:?}", began.elapsed());
    }

    /// Only the end of a long stderr is kept — enough to explain an exit,
    /// not a whole log held in memory.
    #[cfg(unix)]
    #[test]
    fn only_the_last_lines_of_stderr_are_kept() {
        let dir = crate::testing::temp_dir("mcp-stdio-tail");
        let result = start(&sh("for i in $(seq 1 30); do echo line $i >&2; done; exit 1", 5), &dir, &|| false);
        let Err(McpError::Exited { stderr, .. }) = result else { panic!("expected an exit") };
        let lines: Vec<&str> = stderr.lines().collect();
        assert_eq!(lines.len(), STDERR_LINES);
        assert_eq!((lines[0], lines[STDERR_LINES - 1]), ("line 11", "line 30"));
    }

    /// What the restart in `services::mcp_servers` goes by: a server that
    /// died on a call is known dead before the next one is sent.
    #[cfg(unix)]
    #[test]
    fn a_process_that_exits_on_a_call_is_no_longer_alive() {
        let script = r#"
            read discover; echo '{"jsonrpc":"2.0","id":0,"error":{"code":-32601,"message":"Method not found"}}'
            read init; echo '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-11-25","capabilities":{},"serverInfo":{"name":"sh","version":"1"}}}'
            read initialized
            read call; echo 'crashed' >&2; exit 2
        "#;
        let dir = crate::testing::temp_dir("mcp-stdio-alive");
        let server = start(&sh(script, 5), &dir, &|| false).unwrap();
        assert!(server.is_alive());
        let err = server.call_tool("hi", json!({}), &|| false).unwrap_err();
        assert_eq!(err, McpError::Exited { code: Some(2), stderr: "crashed".into() });
        assert!(!server.is_alive());
    }

    /// Its stdout still open — held by a child it left behind — while the
    /// server itself is gone.
    #[cfg(unix)]
    #[test]
    fn a_server_whose_process_exited_is_not_alive_while_its_stream_stays_open() {
        let script = r#"
            read discover; echo '{"jsonrpc":"2.0","id":0,"error":{"code":-32601,"message":"Method not found"}}'
            read init; echo '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-11-25","capabilities":{},"serverInfo":{"name":"sh","version":"1"}}}'
            read initialized
            sleep 300 &
            exit 0
        "#;
        let dir = crate::testing::temp_dir("mcp-stdio-orphan");
        let server = start(&sh(script, 5), &dir, &|| false).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while server.is_alive() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!server.is_alive());
    }

    /// A server that runs but never opens the session is not left running:
    /// nothing holds it afterwards, so nothing would ever stop it.
    #[cfg(unix)]
    #[test]
    fn a_process_that_does_not_open_the_session_is_killed() {
        let dir = crate::testing::temp_dir("mcp-stdio-refuses");
        let pid_file = dir.join("server.pid");
        let script = format!(
            r#"echo $$ > {}
            read discover; echo '{{"jsonrpc":"2.0","id":0,"error":{{"code":-32601,"message":"Method not found"}}}}'
            read init; echo '{{"jsonrpc":"2.0","id":1,"error":{{"code":-32000,"message":"no token"}}}}'
            sleep 300"#,
            pid_file.display()
        );
        let Err(err) = start(&sh(&script, 5), &dir, &|| false) else { panic!("started") };
        assert!(matches!(&err, McpError::Handshake(m) if m.contains("no token")), "{err}");
        let pid: i32 = std::fs::read_to_string(&pid_file).unwrap().trim().parse().unwrap();
        assert_ne!(unsafe { libc::kill(pid, 0) }, 0, "the server outlived its failed start");
    }

    #[test]
    fn a_command_that_does_not_exist_is_not_started() {
        let dir = crate::testing::temp_dir("mcp-stdio-missing");
        let config = McpServerConfig { command: "definitely-not-a-command-4f2a".into(), ..Default::default() };
        assert!(matches!(start(&config, &dir, &|| false), Err(McpError::NotStarted(m)) if m.contains("definitely-not")));
    }

    /// Dropping the server takes its children with it: `npx` is only the
    /// parent of the real server.
    #[cfg(unix)]
    #[test]
    fn dropping_the_server_kills_what_it_started() {
        let dir = crate::testing::temp_dir("mcp-stdio-drop");
        let pid_file = dir.join("child.pid");
        let script = format!(
            r#"sleep 300 & echo $! > {}
            read discover; echo '{{"jsonrpc":"2.0","id":0,"error":{{"code":-32601,"message":"Method not found"}}}}'
            read init; echo '{{"jsonrpc":"2.0","id":1,"result":{{"protocolVersion":"2025-11-25","capabilities":{{}},"serverInfo":{{"name":"sh","version":"1"}}}}}}'
            read _; wait"#,
            pid_file.display()
        );
        let server = start(&sh(&script, 5), &dir, &|| false).unwrap();
        let pid: i32 = std::fs::read_to_string(&pid_file).unwrap().trim().parse().unwrap();
        assert_eq!(unsafe { libc::kill(pid, 0) }, 0, "the grandchild runs");

        drop(server);
        let deadline = Instant::now() + Duration::from_secs(3);
        while unsafe { libc::kill(pid, 0) } == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_ne!(unsafe { libc::kill(pid, 0) }, 0, "the grandchild outlived the server");
    }
}
