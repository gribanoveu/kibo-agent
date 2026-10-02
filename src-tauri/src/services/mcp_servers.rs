//! The MCP servers the app keeps running.
//!
//! Started for the first Agent turn that needs them and kept between turns —
//! a server's start can be a package download. An entry that changes, or is
//! switched off, stops its server; a workspace that changes stops them all,
//! because each runs in the workspace it was started for.
//!
//! **Failure** (decided in `docs/06-port-plan.md`, stage 7): a server that
//! stops in the middle of a turn fails the call it was on — never retried,
//! the server may have done the thing — and is started again, once per turn,
//! before the next call to it. The tool list the model was given stays as it
//! was for the rest of the turn.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::domain::mcp::{
    items, prompt_arguments, ConnectedServer, McpAnswer, McpCallResult, McpClient, McpConfig, McpError, McpPrompt,
    McpQuestion, McpServerConfig, McpServerState, McpTool, McpToolInfo, McpTools,
};
use crate::sync::lock;

/// Starts one server in a folder and completes its handshake — the stdio
/// process in the app, a scripted client in tests.
pub type Start =
    Arc<dyn Fn(&McpServerConfig, &Path, &dyn Fn() -> bool) -> Result<Arc<dyn McpClient>, McpError> + Send + Sync>;

pub struct McpServers {
    start: Start,
    pool: Mutex<Pool>,
}

#[derive(Default)]
struct Pool {
    cwd: Option<PathBuf>,
    slots: BTreeMap<String, Slot>,
}

struct Slot {
    config: McpServerConfig,
    state: SlotState,
}

enum SlotState {
    Starting,
    Running { server: Arc<Supervised>, tools: Vec<McpTool>, prompts: Vec<McpPrompt> },
    Failed(String),
}

impl McpServers {
    pub fn new(start: Start) -> Self {
        Self { start, pool: Mutex::default() }
    }

    /// Brings the running servers in line with `config` and returns their
    /// tools for one turn in `cwd`.
    ///
    /// New servers start side by side, and with the pool unlocked: the tab
    /// asks for their state while they start, and a first `npx` run can take
    /// its whole timeout. One that fails to start gives no tools; one stopped
    /// by the user's Stop is simply tried again next turn.
    pub fn for_turn(&self, config: &McpConfig, cwd: &Path, cancelled: &(dyn Fn() -> bool + Sync)) -> McpTools {
        let missing: Vec<(String, McpServerConfig)> = {
            let mut pool = lock(&self.pool);
            if pool.cwd.as_deref() != Some(cwd) {
                pool.slots.clear();
                pool.cwd = Some(cwd.to_path_buf());
            }
            let wanted = runnable(config);
            pool.slots.retain(|name, slot| keep(slot, wanted.get(name)));
            let missing: Vec<_> = wanted.into_iter().filter(|(name, _)| !pool.slots.contains_key(name)).collect();
            for (name, config) in &missing {
                pool.slots.insert(name.clone(), Slot { config: config.clone(), state: SlotState::Starting });
            }
            missing
        };

        let started: Vec<(String, Option<SlotState>)> = std::thread::scope(|scope| {
            let handles: Vec<_> = missing
                .into_iter()
                .map(|(name, config)| {
                    scope.spawn(move || {
                        let state = self.start_one(&config, cwd, cancelled);
                        (name, state)
                    })
                })
                .collect();
            handles.into_iter().filter_map(|handle| handle.join().ok()).collect()
        });

        self.refresh_stale(cwd);

        let mut pool = lock(&self.pool);
        for (name, state) in started {
            // Switched off or edited while it started: `prune` has removed
            // the slot, and the server just started is dropped — which stops
            // it.
            let Some(slot) = pool.slots.get_mut(&name) else { continue };
            match state {
                Some(state) => slot.state = state,
                None => {
                    pool.slots.remove(&name);
                }
            }
        }
        for slot in pool.slots.values() {
            if let SlotState::Running { server, .. } = &slot.state {
                server.restarted.store(false, Ordering::SeqCst);
            }
        }
        connected(&pool)
    }

    /// Reads again the tool list of every running server whose list may no
    /// longer be its own — it said so, its time ran out, or it was started
    /// anew last turn. Here, between turns, and nowhere else: within a turn
    /// the model keeps the list it was given. A server that does not answer
    /// keeps the list it had, and the turn finds out when it calls.
    ///
    /// With the pool unlocked, like a start: a reading can take the server's
    /// whole timeout.
    fn refresh_stale(&self, cwd: &Path) {
        let stale: Vec<(String, Arc<Supervised>)> = {
            let pool = lock(&self.pool);
            if pool.cwd.as_deref() != Some(cwd) {
                return;
            }
            pool.slots
                .iter()
                .filter_map(|(name, slot)| match &slot.state {
                    SlotState::Running { server, .. } if server.tools_stale() => Some((name.clone(), Arc::clone(server))),
                    _ => None,
                })
                .collect()
        };
        for (name, stale) in stale {
            let Ok(listed) = stale.list_tools() else { continue };
            let offered = stale.list_prompts();
            let mut pool = lock(&self.pool);
            // Switched off or edited meanwhile: the slot is gone or another's.
            if let Some(SlotState::Running { server, tools, prompts }) = pool.slots.get_mut(&name).map(|slot| &mut slot.state) {
                if Arc::ptr_eq(server, &stale) {
                    *tools = listed;
                    if let Ok(offered) = offered {
                        *prompts = offered;
                    }
                }
            }
        }
    }

    /// The tools of the servers already running for `cwd` — what the next
    /// turn will offer, as far as can be known without starting anything.
    /// For the context meter: a server is not started to be measured.
    pub fn running(&self, cwd: &Path) -> McpTools {
        let pool = lock(&self.pool);
        if pool.cwd.as_deref() != Some(cwd) {
            return McpTools::default();
        }
        connected(&pool)
    }

    /// Starts one server now, on its own, and says what came of it — the
    /// tab asks when the user opens a server's row, to see its tools
    /// without spending an Agent turn on it. One already in the pool is
    /// left alone and simply reported.
    ///
    /// A server the file does not let run (switched off, or with something
    /// wrong in its entry) is not started: its row already says why.
    pub fn connect(&self, name: &str, config: &McpConfig, cwd: &Path) -> McpServerState {
        let Some(entry) = runnable(config).remove(name) else {
            return McpServerState::NotStarted;
        };
        let start_now = {
            let mut pool = lock(&self.pool);
            if pool.cwd.as_deref() != Some(cwd) {
                pool.slots.clear();
                pool.cwd = Some(cwd.to_path_buf());
            }
            let known = pool.slots.get_mut(name).is_some_and(|slot| keep(slot, Some(&entry)));
            if !known {
                pool.slots.insert(name.to_string(), Slot { config: entry.clone(), state: SlotState::Starting });
            }
            !known
        };
        if start_now {
            // Nothing to stop it: the tab has no Stop for a server it asked
            // to start, and the start bounded by the server's own timeout.
            let started = self.start_one(&entry, cwd, &|| false);
            let mut pool = lock(&self.pool);
            match started {
                Some(state) => {
                    if let Some(slot) = pool.slots.get_mut(name) {
                        slot.state = state;
                    }
                }
                None => {
                    pool.slots.remove(name);
                }
            }
        }
        self.state(name, &entry)
    }

    /// `None` when the user's Stop cut the start short: that says nothing
    /// about the server, so it is not recorded as a failure.
    fn start_one(&self, config: &McpServerConfig, cwd: &Path, cancelled: &dyn Fn() -> bool) -> Option<SlotState> {
        let started = (self.start)(config, cwd, cancelled).and_then(|client| Ok((client.list_tools()?, client)));
        match started {
            // Prompts are a convenience: a server whose list of them fails
            // still has its tools.
            Ok((tools, client)) => Some(SlotState::Running {
                prompts: client.list_prompts().unwrap_or_default(),
                server: Arc::new(Supervised {
                    config: config.clone(),
                    cwd: cwd.to_path_buf(),
                    start: Arc::clone(&self.start),
                    current: Mutex::new(client),
                    restarted: AtomicBool::new(false),
                    last_error: Mutex::new(None),
                }),
                tools,
            }),
            Err(McpError::Cancelled) => None,
            Err(error) => Some(SlotState::Failed(error.to_string())),
        }
    }

    /// Stops the servers whose entry is gone, changed or switched off, at
    /// once rather than at the next turn — switching a server off should
    /// stop it.
    pub fn prune(&self, config: &McpConfig) {
        let wanted = runnable(config);
        lock(&self.pool).slots.retain(|name, slot| keep(slot, wanted.get(name)));
    }

    /// What the tab says about a server whose entry is `entry` now. A server
    /// running from an older entry is about to be replaced, so it is
    /// reported as not started.
    pub fn state(&self, name: &str, entry: &McpServerConfig) -> McpServerState {
        let pool = lock(&self.pool);
        let Some(slot) = pool.slots.get(name).filter(|slot| slot.config.launch() == entry.launch()) else {
            return McpServerState::NotStarted;
        };
        match &slot.state {
            SlotState::Starting => McpServerState::Starting,
            SlotState::Running { server, tools, .. } if server.is_alive() => McpServerState::Running {
                tools: tools.iter().map(|tool| McpToolInfo::new(tool, entry.exposure_of(&tool.name))).collect(),
                instructions: server.instructions(),
            },
            SlotState::Running { server, .. } => McpServerState::Exited {
                error: lock(&server.last_error).clone().unwrap_or_else(|| "the MCP server exited".into()),
            },
            SlotState::Failed(error) => McpServerState::Failed { error: error.clone() },
        }
    }

    /// The prompts of the servers running for `cwd`, by server — what the
    /// composer offers. Nothing is started to be asked.
    pub fn prompts(&self, cwd: &Path) -> Vec<(String, McpPrompt)> {
        let pool = lock(&self.pool);
        if pool.cwd.as_deref() != Some(cwd) {
            return Vec::new();
        }
        pool.slots
            .iter()
            .filter_map(|(name, slot)| match &slot.state {
                SlotState::Running { server, prompts, .. } if server.is_alive() => Some((name, prompts)),
                _ => None,
            })
            .flat_map(|(name, prompts)| prompts.iter().map(move |prompt| (name.clone(), prompt.clone())))
            .collect()
    }

    /// One prompt of a running server, written with what was typed after its
    /// name — given to its arguments by `domain::mcp::prompt_arguments`.
    pub fn get_prompt(&self, server: &str, name: &str, typed: &str, cwd: &Path) -> Result<String, McpError> {
        let (client, prompt) = {
            let pool = lock(&self.pool);
            let running = pool.slots.get(server).filter(|_| pool.cwd.as_deref() == Some(cwd)).and_then(|slot| match &slot.state {
                SlotState::Running { server, prompts, .. } => Some((server, prompts)),
                _ => None,
            });
            let Some((client, prompts)) = running else {
                return Err(McpError::NotStarted(format!("\"{server}\" is not running")));
            };
            let Some(prompt) = prompts.iter().find(|p| p.name == name) else {
                return Err(McpError::Protocol(format!("\"{server}\" offers no prompt {name:?}")));
            };
            (Arc::clone(client), prompt.clone())
        };
        client.get_prompt(name, &prompt_arguments(&prompt, typed))
    }

    /// When the app quits.
    pub fn stop_all(&self) {
        lock(&self.pool).slots.clear();
    }
}

fn connected(pool: &Pool) -> McpTools {
    let servers = pool
        .slots
        .iter()
        .filter_map(|(name, slot)| match &slot.state {
            SlotState::Running { server, tools, .. } => Some(ConnectedServer {
                name: name.clone(),
                config: slot.config.clone(),
                client: Arc::clone(server) as Arc<dyn McpClient>,
                tools: tools.clone(),
                instructions: server.instructions(),
            }),
            _ => None,
        })
        .collect();
    McpTools::new(servers)
}

/// Whether a running slot stays for `wanted`, its entry now: it does while
/// the entry starts the same process, and takes on the rest of it — which
/// tools the model sees is not a reason to restart a server.
fn keep(slot: &mut Slot, wanted: Option<&McpServerConfig>) -> bool {
    match wanted {
        Some(wanted) if wanted.launch() == slot.config.launch() => {
            slot.config = wanted.clone();
            true
        }
        _ => false,
    }
}

/// The entries that would start: switched on, and with nothing wrong that
/// the entry alone shows.
fn runnable(config: &McpConfig) -> BTreeMap<String, McpServerConfig> {
    items(config)
        .into_iter()
        .filter(|item| item.enabled && item.error.is_none())
        .map(|item| {
            let entry = config.mcp_servers[&item.name].clone();
            (item.name, entry)
        })
        .collect()
}

/// A server that is started again when it has stopped — at most once per
/// turn, and only before a call, never to repeat one.
struct Supervised {
    config: McpServerConfig,
    cwd: PathBuf,
    start: Start,
    current: Mutex<Arc<dyn McpClient>>,
    /// Reset by `for_turn`.
    restarted: AtomicBool,
    /// How it stopped, for the tab.
    last_error: Mutex<Option<String>>,
}

impl Supervised {
    fn live(&self, cancelled: &dyn Fn() -> bool) -> Result<Arc<dyn McpClient>, McpError> {
        let mut current = lock(&self.current);
        if current.is_alive() {
            return Ok(Arc::clone(&current));
        }
        if self.restarted.swap(true, Ordering::SeqCst) {
            return Err(McpError::NotRestarted);
        }
        *current = (self.start)(&self.config, &self.cwd, cancelled).inspect_err(|error| {
            *lock(&self.last_error) = Some(error.to_string());
        })?;
        Ok(Arc::clone(&current))
    }
}

impl McpClient for Supervised {
    fn list_tools(&self) -> Result<Vec<McpTool>, McpError> {
        lock(&self.current).list_tools()
    }

    fn call_tool(&self, name: &str, arguments: Value, cancelled: &dyn Fn() -> bool) -> Result<McpCallResult, McpError> {
        let result = self.live(cancelled)?.call_tool(name, arguments, cancelled);
        if let Err(error @ McpError::Exited { .. }) = &result {
            *lock(&self.last_error) = Some(error.to_string());
        }
        result
    }

    fn call_tool_asking(
        &self,
        name: &str,
        arguments: Value,
        cancelled: &dyn Fn() -> bool,
        ask: &dyn Fn(&McpQuestion) -> McpAnswer,
    ) -> Result<McpCallResult, McpError> {
        let result = self.live(cancelled)?.call_tool_asking(name, arguments, cancelled, ask);
        if let Err(error @ McpError::Exited { .. }) = &result {
            *lock(&self.last_error) = Some(error.to_string());
        }
        result
    }

    fn is_alive(&self) -> bool {
        lock(&self.current).is_alive()
    }

    fn instructions(&self) -> Option<String> {
        lock(&self.current).instructions()
    }

    fn list_prompts(&self) -> Result<Vec<McpPrompt>, McpError> {
        let current = Arc::clone(&lock(&self.current));
        current.list_prompts()
    }

    fn get_prompt(&self, name: &str, arguments: &BTreeMap<String, String>) -> Result<String, McpError> {
        let current = Arc::clone(&lock(&self.current));
        current.get_prompt(name, arguments)
    }

    /// A server started again mid-turn is another process, perhaps another
    /// version: what it offers is asked anew before the next turn.
    fn tools_stale(&self) -> bool {
        self.restarted.load(Ordering::SeqCst) || lock(&self.current).tools_stale()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::AtomicUsize;

    /// One process of a scripted server: it dies on the tool `crash`, and
    /// the tool `grow` makes it offer one tool more and say its list changed.
    /// A process started after the first is a newer one, and offers that
    /// tool from the start without saying anything.
    struct Process {
        alive: AtomicBool,
        run: usize,
        grown: AtomicBool,
        stale: AtomicBool,
    }

    impl Process {
        fn started(run: usize) -> Arc<dyn McpClient> {
            Arc::new(Self { alive: AtomicBool::new(true), run, grown: AtomicBool::new(run > 1), stale: AtomicBool::new(false) })
        }
    }

    impl McpClient for Process {
        fn list_tools(&self) -> Result<Vec<McpTool>, McpError> {
            self.stale.store(false, Ordering::SeqCst);
            let tool = |name: &str, description: &str| McpTool {
                name: name.into(),
                description: description.into(),
                input_schema: json!({"type": "object"}),
                ..Default::default()
            };
            let mut tools = vec![tool("echo", "Says it back"), tool("crash", "Dies")];
            if self.grown.load(Ordering::SeqCst) {
                tools.push(McpTool {
                    title: Some("Wipe everything".into()),
                    hints: crate::domain::mcp::McpToolHints { read_only: false, destructive: true },
                    ..tool("wipe", "Wipes")
                });
            }
            Ok(tools)
        }
        fn call_tool(&self, name: &str, _: Value, _: &dyn Fn() -> bool) -> Result<McpCallResult, McpError> {
            if name == "crash" {
                self.alive.store(false, Ordering::SeqCst);
                return Err(McpError::Exited { code: Some(1), stderr: "boom".into() });
            }
            if name == "grow" {
                self.grown.store(true, Ordering::SeqCst);
                self.stale.store(true, Ordering::SeqCst);
            }
            Ok(McpCallResult { text: format!("run {}", self.run), is_error: false })
        }
        fn is_alive(&self) -> bool {
            self.alive.load(Ordering::SeqCst)
        }
        fn instructions(&self) -> Option<String> {
            Some(format!("Echo first. (run {})", self.run))
        }
        fn tools_stale(&self) -> bool {
            self.stale.load(Ordering::SeqCst)
        }
        /// `greet`, and after `grow` also `part`.
        fn list_prompts(&self) -> Result<Vec<McpPrompt>, McpError> {
            let argument = |name: &str| crate::domain::mcp::McpPromptArgument { name: name.into(), ..Default::default() };
            let mut prompts = vec![McpPrompt { name: "greet".into(), arguments: vec![argument("who"), argument("how")], ..Default::default() }];
            if self.grown.load(Ordering::SeqCst) {
                prompts.push(McpPrompt { name: "part".into(), ..Default::default() });
            }
            Ok(prompts)
        }
        fn get_prompt(&self, name: &str, arguments: &BTreeMap<String, String>) -> Result<String, McpError> {
            Ok(format!("{name} {arguments:?} (run {})", self.run))
        }
    }

    /// Counts starts; a command named `broken` does not start, one named
    /// `once` starts only the first time.
    fn servers() -> (McpServers, Arc<AtomicUsize>) {
        let starts = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&starts);
        let start: Start = Arc::new(move |config, _, cancelled| {
            let run = counted.fetch_add(1, Ordering::SeqCst) + 1;
            if cancelled() {
                return Err(McpError::Cancelled);
            }
            if config.command == "broken" || (config.command == "once" && run > 1) {
                return Err(McpError::NotStarted("no such command".into()));
            }
            Ok(Process::started(run))
        });
        (McpServers::new(start), starts)
    }

    fn config(entries: &[(&str, &str)]) -> McpConfig {
        let mut config = McpConfig::default();
        for (name, command) in entries {
            config.mcp_servers.insert(name.to_string(), McpServerConfig { command: command.to_string(), ..Default::default() });
        }
        config
    }

    const NO: &(dyn Fn() -> bool + Sync) = &|| false;

    fn call(tools: &McpTools, tool: &str) -> Result<String, McpError> {
        let entry = tools.get("mcp__a__echo").expect("the tool is offered");
        entry.client.call_tool(tool, json!({}), &|| false).map(|r| r.text)
    }

    fn cwd() -> PathBuf {
        PathBuf::from("/work")
    }

    #[test]
    fn a_server_starts_once_and_is_kept_between_turns() {
        let (servers, starts) = servers();
        let config = config(&[("a", "ok")]);
        assert_eq!(servers.state("a", &config.mcp_servers["a"]), McpServerState::NotStarted);

        let tools = servers.for_turn(&config, &cwd(), NO);
        assert_eq!(call(&tools, "echo").unwrap(), "run 1");
        servers.for_turn(&config, &cwd(), NO);
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        assert_eq!(
            servers.state("a", &config.mcp_servers["a"]),
            McpServerState::Running {
                tools: vec![
                    McpToolInfo { name: "echo".into(), description: "Says it back".into(), ..Default::default() },
                    McpToolInfo { name: "crash".into(), description: "Dies".into(), ..Default::default() },
                ],
                instructions: Some("Echo first. (run 1)".into()),
            },
            "the tab lists every tool the server offers, not just how many");
    }

    /// The done-when of F-7.4d: the model gets the error, the call is not
    /// repeated, and the next call goes to a server started again.
    #[test]
    fn a_crash_fails_its_call_and_the_next_call_restarts_the_server_once() {
        let (servers, starts) = servers();
        let config = config(&[("a", "ok")]);
        let tools = servers.for_turn(&config, &cwd(), NO);

        let error = call(&tools, "crash").unwrap_err();
        assert!(matches!(error, McpError::Exited { .. }), "{error:?}");
        assert_eq!(starts.load(Ordering::SeqCst), 1, "the failed call is not repeated on a new server");
        assert!(matches!(servers.state("a", &config.mcp_servers["a"]), McpServerState::Exited { error } if error.contains("boom")));

        assert_eq!(call(&tools, "echo").unwrap(), "run 2");
        call(&tools, "crash").unwrap_err();
        assert_eq!(call(&tools, "echo").unwrap_err(), McpError::NotRestarted, "one restart per turn");
        assert_eq!(starts.load(Ordering::SeqCst), 2);

        let tools = servers.for_turn(&config, &cwd(), NO);
        assert_eq!(call(&tools, "echo").unwrap(), "run 3", "a new turn may restart it again");
    }

    #[test]
    fn a_restart_that_fails_is_not_tried_again_that_turn() {
        let (servers, starts) = servers();
        let config = config(&[("a", "once")]);
        let tools = servers.for_turn(&config, &cwd(), NO);
        call(&tools, "crash").unwrap_err();
        assert!(matches!(call(&tools, "echo").unwrap_err(), McpError::NotStarted(_)));
        assert_eq!(call(&tools, "echo").unwrap_err(), McpError::NotRestarted);
        assert_eq!(starts.load(Ordering::SeqCst), 2);
        assert!(matches!(servers.state("a", &config.mcp_servers["a"]), McpServerState::Exited { error } if error.contains("no such command")));
    }

    /// A server that hangs on start would otherwise cost every turn its
    /// timeout.
    #[test]
    fn a_server_that_failed_to_start_waits_for_its_entry_to_change() {
        let (servers, starts) = servers();
        let broken = config(&[("a", "broken")]);
        assert!(servers.for_turn(&broken, &cwd(), NO).get("mcp__a__echo").is_none());
        servers.for_turn(&broken, &cwd(), NO);
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        assert!(matches!(servers.state("a", &broken.mcp_servers["a"]), McpServerState::Failed { error } if error.contains("no such command")));

        let fixed = config(&[("a", "ok")]);
        assert_eq!(servers.state("a", &fixed.mcp_servers["a"]), McpServerState::NotStarted);
        assert!(servers.for_turn(&fixed, &cwd(), NO).get("mcp__a__echo").is_some());
    }

    #[test]
    fn a_stop_during_the_start_is_not_a_failure() {
        let (servers, starts) = servers();
        let config = config(&[("a", "ok")]);
        assert!(servers.for_turn(&config, &cwd(), &|| true).get("mcp__a__echo").is_none());
        assert_eq!(servers.state("a", &config.mcp_servers["a"]), McpServerState::NotStarted);
        servers.for_turn(&config, &cwd(), NO);
        assert_eq!(starts.load(Ordering::SeqCst), 2, "tried again");
    }

    /// The meter asks what is running and starts nothing: before the first
    /// turn there is nothing, after it the turn's tools, and in another
    /// folder nothing again.
    #[test]
    fn running_reports_the_started_servers_and_starts_none() {
        let (servers, starts) = servers();
        let config = config(&[("a", "ok")]);
        assert!(servers.running(&cwd()).get("mcp__a__echo").is_none());
        assert_eq!(starts.load(Ordering::SeqCst), 0, "started to be measured");

        servers.for_turn(&config, &cwd(), NO);
        assert!(servers.running(&cwd()).get("mcp__a__echo").is_some());
        assert!(servers.running(Path::new("/elsewhere")).get("mcp__a__echo").is_none());
        assert_eq!(starts.load(Ordering::SeqCst), 1);
    }

    /// The tab's own start: opening a server's row shows its tools, and
    /// asking twice does not start it twice.
    #[test]
    fn connecting_starts_one_server_and_reports_what_it_offers() {
        let (servers, starts) = servers();
        let mut config = config(&[("a", "ok"), ("b", "ok")]);
        config.mcp_servers.get_mut("b").unwrap().disabled = true;

        let state = servers.connect("a", &config, &cwd());
        assert_eq!(
            state,
            McpServerState::Running {
                tools: vec![
                    McpToolInfo { name: "echo".into(), description: "Says it back".into(), ..Default::default() },
                    McpToolInfo { name: "crash".into(), description: "Dies".into(), ..Default::default() },
                ],
                instructions: Some("Echo first. (run 1)".into()),
            }
        );
        assert_eq!(servers.connect("a", &config, &cwd()), state, "the same server, not a new one");
        assert_eq!(starts.load(Ordering::SeqCst), 1);

        assert_eq!(servers.connect("b", &config, &cwd()), McpServerState::NotStarted, "switched off");
        assert_eq!(starts.load(Ordering::SeqCst), 1, "and not started");
    }

    /// An entry changed only in what the model sees is the same process: a
    /// turn and the tab's start both keep it, and both read the new rule.
    #[test]
    fn a_change_of_exposure_does_not_restart_the_server() {
        let (servers, starts) = servers();
        let before = config(&[("a", "ok")]);
        servers.for_turn(&before, &cwd(), NO);
        let mut after = before.clone();
        after.mcp_servers.get_mut("a").unwrap().show_tool("crash", false);
        assert!(
            matches!(servers.state("a", &after.mcp_servers["a"]), McpServerState::Running { .. }),
            "the file changed outside the app: still the running server"
        );

        let McpServerState::Running { tools, .. } = servers.connect("a", &after, &cwd()) else { panic!("not running") };
        assert_eq!(tools[1].exposure, crate::domain::mcp::Exposure::Hidden);
        assert!(servers.for_turn(&after, &cwd(), NO).get("mcp__a__crash").is_none());
        assert_eq!(starts.load(Ordering::SeqCst), 1);
    }

    /// A server started from the tab is the one the next turn uses.
    #[test]
    fn a_turn_keeps_the_server_the_tab_started() {
        let (servers, starts) = servers();
        let config = config(&[("a", "ok")]);
        servers.connect("a", &config, &cwd());
        let tools = servers.for_turn(&config, &cwd(), NO);
        assert_eq!(call(&tools, "echo").unwrap(), "run 1");
        assert_eq!(starts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn switched_off_broken_and_changed_entries_do_not_run() {
        let (servers, starts) = servers();
        let mut config = config(&[("a", "ok"), ("b", "ok"), ("c", "")]);
        config.mcp_servers.get_mut("b").unwrap().disabled = true;
        let tools = servers.for_turn(&config, &cwd(), NO);
        assert_eq!(tools.definitions(&Default::default()).len(), 2, "a's two tools, and nothing from b or c");
        assert_eq!(starts.load(Ordering::SeqCst), 1, "only a");

        config.mcp_servers.get_mut("a").unwrap().args = vec!["--new".into()];
        servers.for_turn(&config, &cwd(), NO);
        assert_eq!(starts.load(Ordering::SeqCst), 2, "a changed entry is a new server");
    }

    #[test]
    fn switching_a_server_off_stops_it_at_once() {
        let (servers, _) = servers();
        let mut config = config(&[("a", "ok")]);
        let tools = servers.for_turn(&config, &cwd(), NO);
        config.mcp_servers.get_mut("a").unwrap().disabled = true;
        servers.prune(&config);
        let entry = &tools.get("mcp__a__echo").unwrap().client;
        assert_eq!(Arc::strong_count(entry), 2, "held by this turn's two tool entries alone, not by the pool");
        assert_eq!(servers.state("a", &config.mcp_servers["a"]), McpServerState::NotStarted);
    }

    #[test]
    fn a_server_switched_off_while_it_starts_does_not_stay() {
        let (release, wait) = std::sync::mpsc::channel::<()>();
        let wait = Mutex::new(wait);
        let start: Start = Arc::new(move |_, _, _| {
            wait.lock().unwrap().recv().unwrap();
            Ok(Process::started(1))
        });
        let servers = Arc::new(McpServers::new(start));
        let mut config = config(&[("a", "ok")]);
        let turn = {
            let (servers, config) = (Arc::clone(&servers), config.clone());
            std::thread::spawn(move || servers.for_turn(&config, &cwd(), NO).definitions(&Default::default()).len())
        };
        while servers.state("a", &config.mcp_servers["a"]) != McpServerState::Starting {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        config.mcp_servers.get_mut("a").unwrap().disabled = true;
        servers.prune(&config);
        release.send(()).unwrap();

        assert_eq!(turn.join().unwrap(), 0, "the turn does not get it");
        config.mcp_servers.get_mut("a").unwrap().disabled = false;
        assert_eq!(servers.state("a", &config.mcp_servers["a"]), McpServerState::NotStarted, "nor does the pool keep it");
    }

    /// On quit: the servers run in process groups of their own and would
    /// outlive the app.
    #[test]
    fn quitting_stops_every_server() {
        let (servers, _) = servers();
        let config = config(&[("a", "ok")]);
        drop(servers.for_turn(&config, &cwd(), NO));
        servers.stop_all();
        assert_eq!(servers.state("a", &config.mcp_servers["a"]), McpServerState::NotStarted);
    }

    #[test]
    fn another_workspace_starts_the_servers_again() {
        let (servers, starts) = servers();
        let config = config(&[("a", "ok")]);
        servers.for_turn(&config, &cwd(), NO);
        servers.for_turn(&config, Path::new("/elsewhere"), NO);
        assert_eq!(starts.load(Ordering::SeqCst), 2);
    }

    /// The done-when of M-4: a server that changed its list offers the new
    /// one from the next turn, with no restart — and not before: the turn
    /// keeps the list the model was given.
    #[test]
    fn a_list_the_server_changed_is_read_again_for_the_next_turn_and_not_before() {
        let (servers, starts) = servers();
        let config = config(&[("a", "ok")]);
        let tools = servers.for_turn(&config, &cwd(), NO);
        assert!(tools.get("mcp__a__wipe").is_none());

        call(&tools, "grow").unwrap();
        assert!(tools.get("mcp__a__wipe").is_none(), "the turn's own list moved under it");
        assert!(servers.running(&cwd()).get("mcp__a__wipe").is_none(), "read again mid-turn");

        let next = servers.for_turn(&config, &cwd(), NO);
        let wipe = next.get("mcp__a__wipe").expect("the new tool is offered");
        assert!(wipe.tool.hints.destructive);
        assert_eq!(starts.load(Ordering::SeqCst), 1, "restarted to be asked");
        let McpServerState::Running { tools: shown, .. } = servers.state("a", &config.mcp_servers["a"]) else { panic!("not running") };
        assert_eq!(
            shown[2],
            McpToolInfo {
                name: "wipe".into(),
                description: "Wipes".into(),
                title: Some("Wipe everything".into()),
                hints: crate::domain::mcp::McpToolHints { read_only: false, destructive: true },
                exposure: crate::domain::mcp::Exposure::Direct,
            },
            "the tab shows what the server says about it"
        );
    }

    /// A server started again mid-turn is another process: what it offers and
    /// what it says are asked of it, not remembered from the one that died.
    #[test]
    fn a_restarted_server_is_asked_again_what_it_offers_and_says() {
        let (servers, _) = servers();
        let config = config(&[("a", "ok")]);
        let tools = servers.for_turn(&config, &cwd(), NO);
        assert_eq!(tools.notes()[0].instructions.as_deref().unwrap(), "Echo first. (run 1)");
        call(&tools, "crash").unwrap_err();
        assert_eq!(call(&tools, "echo").unwrap(), "run 2");
        assert!(tools.get("mcp__a__wipe").is_none(), "the turn keeps the list it was given");

        let next = servers.for_turn(&config, &cwd(), NO);
        assert!(next.get("mcp__a__wipe").is_some(), "the list of the process that died");
        assert_eq!(next.notes()[0].instructions.as_deref().unwrap(), "Echo first. (run 2)");
    }

    /// The composer's list: the prompts of the servers running here, and a
    /// prompt written with what was typed given to its arguments.
    #[test]
    fn a_running_servers_prompts_are_offered_and_written_with_what_was_typed() {
        let (servers, starts) = servers();
        let config = config(&[("a", "ok")]);
        assert!(servers.prompts(&cwd()).is_empty(), "started to be listed");
        servers.for_turn(&config, &cwd(), NO);

        let offered = servers.prompts(&cwd());
        assert_eq!(offered.iter().map(|(server, p)| format!("{server}:{}", p.name)).collect::<Vec<_>>(), ["a:greet"]);
        assert!(servers.prompts(Path::new("/elsewhere")).is_empty());
        assert_eq!(
            servers.get_prompt("a", "greet", "world  warmly please", &cwd()).unwrap(),
            r#"greet {"how": "warmly please", "who": "world"} (run 1)"#
        );
        assert!(matches!(servers.get_prompt("b", "greet", "", &cwd()), Err(McpError::NotStarted(_))));
        assert!(matches!(servers.get_prompt("a", "nope", "", &cwd()), Err(McpError::Protocol(m)) if m.contains("nope")));
        assert!(matches!(servers.get_prompt("a", "greet", "", Path::new("/elsewhere")), Err(McpError::NotStarted(_))));
        assert_eq!(starts.load(Ordering::SeqCst), 1);

        let tools = servers.running(&cwd());
        call(&tools, "crash").unwrap_err();
        assert!(servers.prompts(&cwd()).is_empty(), "a server that went is not offered until it is back");
    }

    /// Read again with the tools, between turns.
    #[test]
    fn a_servers_new_prompts_are_offered_from_the_next_turn() {
        let (servers, _) = servers();
        let config = config(&[("a", "ok")]);
        let tools = servers.for_turn(&config, &cwd(), NO);
        call(&tools, "grow").unwrap();
        assert_eq!(servers.prompts(&cwd()).len(), 1);
        servers.for_turn(&config, &cwd(), NO);
        assert_eq!(servers.prompts(&cwd()).len(), 2);
    }

    /// A question a server asks goes through the server started again, like
    /// a plain call: the restart does not lose the way to the user.
    #[test]
    fn a_question_reaches_the_user_through_a_restarted_server() {
        let (servers, starts) = servers();
        let config = config(&[("a", "ok")]);
        let tools = servers.for_turn(&config, &cwd(), NO);
        call(&tools, "crash").unwrap_err();
        let entry = tools.get("mcp__a__echo").unwrap();
        let answered = entry.client.call_tool_asking("echo", json!({}), &|| false, &|_| McpAnswer::Decline).unwrap();
        assert_eq!(answered.text, "run 2");
        assert_eq!(starts.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn the_weight_comes_from_the_entry() {
        let (servers, _) = servers();
        let mut config = config(&[("a", "ok")]);
        config.mcp_servers.get_mut("a").unwrap().weight = Some(7);
        assert_eq!(servers.for_turn(&config, &cwd(), NO).weight("mcp__a__echo"), 7);
    }
}
