//! The MCP tab and its configuration editor.

use std::path::Path;
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime, State};

use crate::commands::agent_state::AgentState;
use crate::domain::mcp::{self, McpChanged, McpConfig, McpEventSink, McpServerItem, McpServerState};
use crate::infra::mcp_config;
use crate::services::mcp_servers::McpServers;

/// A server started, ended, stopped or listed its tools anew: the tab reads
/// its view again.
pub const MCP_EVENT: &str = "mcp:changed";

pub fn mcp_event_sink<R: Runtime>(app: &AppHandle<R>) -> McpEventSink {
    let app = app.clone();
    Arc::new(move |_: McpChanged| {
        let _ = app.emit(MCP_EVENT, ());
    })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpView {
    /// Where the file is, so the editor can say.
    pub path: String,
    /// The file as it stands, for the editor.
    pub text: String,
    pub servers: Vec<McpServerItem>,
}

/// The rows as they stand in the folder `cwd` — each folder runs servers of
/// its own (F-8.4). With no folder, nothing runs.
fn view(config: McpConfig, servers: &McpServers, cwd: Option<&Path>) -> Result<McpView, String> {
    let state = |name: &str| match cwd {
        Some(cwd) => servers.state(name, &config.mcp_servers[name], cwd),
        None => McpServerState::NotStarted,
    };
    let items = mcp::items(&config).into_iter().map(|item| McpServerItem { state: state(&item.name), ..item }).collect();
    Ok(McpView {
        path: mcp_config::path().map_err(|e| e.to_string())?.display().to_string(),
        text: mcp_config::read_text().map_err(|e| e.to_string())?,
        servers: items,
    })
}

/// A change stops what it switched off or changed at once; what it added
/// starts with the next Agent or Analyst turn.
/// The folder whose rows the tab shows: `None` for a Chat-mode window, and
/// for a folder no longer open — a close the window has not heard of yet.
/// The file is the user's and is written either way; only the rows' state
/// is per folder, and a closed one has none (invariant 1 holds: nothing is
/// read for a folder outside the registry).
fn shown_folder(state: &AgentState, workspace: &Option<String>) -> Option<std::path::PathBuf> {
    state.ui_workspace(workspace).ok().flatten()
}

/// Called once a start for `root` is over: a close that ran meanwhile took
/// the folder out of the registry and may have stopped its pool before the
/// start made it again — then the start stops it (F-8 invariant 4). A close
/// unregisters before it stops, so one of the two always sees the other.
fn unless_closed(state: &AgentState, servers: &McpServers, root: &Path) {
    if !state.is_open(root) {
        servers.stop(root);
    }
}

/// In every open folder: the file is one for all of them.
fn changed(config: McpConfig, servers: &McpServers, cwd: Option<&Path>) -> Result<McpView, String> {
    servers.prune(&config);
    view(config, servers, cwd)
}

/// The tab's rows for the folder `workspace` names — accepted only as an
/// open folder (F-8, invariant 1); `None` in a Chat-mode window.
#[tauri::command]
pub fn mcp_config_get(
    workspace: Option<String>,
    state: State<'_, Arc<AgentState>>,
    servers: State<'_, Arc<McpServers>>,
) -> Result<McpView, String> {
    let cwd = shown_folder(&state, &workspace);
    view(mcp_config::load().map_err(|e| e.to_string())?, &servers, cwd.as_deref())
}

#[tauri::command]
pub fn mcp_config_save(
    text: String,
    workspace: Option<String>,
    state: State<'_, Arc<AgentState>>,
    servers: State<'_, Arc<McpServers>>,
) -> Result<McpView, String> {
    let cwd = shown_folder(&state, &workspace);
    changed(mcp_config::save_text(&text).map_err(|e| e.to_string())?, &servers, cwd.as_deref())
}

#[tauri::command]
pub fn mcp_server_set_enabled(
    name: String,
    enabled: bool,
    workspace: Option<String>,
    state: State<'_, Arc<AgentState>>,
    servers: State<'_, Arc<McpServers>>,
) -> Result<McpView, String> {
    let cwd = shown_folder(&state, &workspace);
    changed(mcp_config::set_enabled(&name, enabled).map_err(|e| e.to_string())?, &servers, cwd.as_deref())
}

/// A tool's switch in the tab. Its server keeps running: what the model is
/// offered is read from the file at the next turn.
#[tauri::command]
pub fn mcp_tool_set_shown(
    server: String,
    tool: String,
    shown: bool,
    workspace: Option<String>,
    state: State<'_, Arc<AgentState>>,
    servers: State<'_, Arc<McpServers>>,
) -> Result<McpView, String> {
    let cwd = shown_folder(&state, &workspace);
    changed(mcp_config::set_tool_shown(&server, &tool, shown).map_err(|e| e.to_string())?, &servers, cwd.as_deref())
}

/// Starts one server now, without a turn, so that opening its row in
/// the tab shows what it offers — the user checking a server they just
/// configured should not have to send a message first.
///
/// Off the event loop: a first `npx` run can take the server's whole
/// timeout, and the tab stays answerable meanwhile.
#[tauri::command]
pub async fn mcp_server_connect(
    name: String,
    workspace: Option<String>,
    state: State<'_, Arc<AgentState>>,
    servers: State<'_, Arc<McpServers>>,
) -> Result<McpView, String> {
    let workspace = state
        .ui_workspace(&workspace)?
        .ok_or_else(|| crate::domain::workspace::WorkspaceError::NoFolderOpen.to_string())?;
    let (servers, state) = (Arc::clone(&servers), Arc::clone(&state));
    tauri::async_runtime::spawn_blocking(move || {
        let config = mcp_config::load().map_err(|e| e.to_string())?;
        servers.connect(&name, &config, &workspace);
        unless_closed(&state, &servers, &workspace);
        view(config, &servers, Some(&workspace))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// A prompt a running server offers, as the composer lists it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpPromptItem {
    pub server: String,
    #[serde(flatten)]
    pub prompt: crate::domain::mcp::McpPrompt,
}

/// The prompts of the servers running for the folder `workspace` names —
/// canonicalized, accepted only as an open folder (F-8, invariant 1). Starts
/// nothing: a server's prompts are offered once a turn, the tab or the `/`
/// menu has started it.
#[tauri::command]
pub fn mcp_prompts(
    workspace: Option<String>,
    state: State<'_, Arc<AgentState>>,
    servers: State<'_, Arc<McpServers>>,
) -> Result<Vec<McpPromptItem>, String> {
    let workspace = state.ui_workspace(&workspace)?;
    Ok(workspace.map_or_else(Vec::new, |root| prompt_items(&servers, &root)))
}

/// The same, once the servers not running yet are started — what the `/`
/// menu asks as it opens: typing `/` is asking what there is, and a server's
/// prompts should not wait for a first turn or the tab. A configuration that
/// does not parse starts nothing, as for a turn.
///
/// Off the event loop: a first `npx` run can take the server's whole
/// timeout, and the menu shows the other commands meanwhile.
#[tauri::command]
pub async fn mcp_prompts_start(
    workspace: Option<String>,
    state: State<'_, Arc<AgentState>>,
    servers: State<'_, Arc<McpServers>>,
) -> Result<Vec<McpPromptItem>, String> {
    let workspace = state
        .ui_workspace(&workspace)?
        .ok_or_else(|| crate::domain::workspace::WorkspaceError::NoFolderOpen.to_string())?;
    let (servers, state) = (Arc::clone(&servers), Arc::clone(&state));
    tauri::async_runtime::spawn_blocking(move || {
        if let Ok(config) = mcp_config::load() {
            servers.for_turn(&config, &workspace, &|| false);
            unless_closed(&state, &servers, &workspace);
        }
        prompt_items(&servers, &workspace)
    })
    .await
    .map_err(|e| e.to_string())
}

fn prompt_items(servers: &McpServers, workspace: &std::path::Path) -> Vec<McpPromptItem> {
    servers.prompts(workspace).into_iter().map(|(server, prompt)| McpPromptItem { server, prompt }).collect()
}

/// A prompt written with what was typed after its name — the text the
/// composer sends. Off the event loop: it is a request to the server.
#[tauri::command]
pub async fn mcp_prompt_get(
    server: String,
    name: String,
    typed: String,
    workspace: Option<String>,
    state: State<'_, Arc<AgentState>>,
    servers: State<'_, Arc<McpServers>>,
) -> Result<String, String> {
    let workspace = state
        .ui_workspace(&workspace)?
        .ok_or_else(|| crate::domain::workspace::WorkspaceError::NoFolderOpen.to_string())?;
    let servers = Arc::clone(&servers);
    tauri::async_runtime::spawn_blocking(move || servers.get_prompt(&server, &name, &typed, &workspace).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::mcp::{McpClient, McpError, McpTool};

    #[test]
    fn the_channel_name_is_pinned() {
        assert_eq!(MCP_EVENT, "mcp:changed");
    }

    struct Idle;
    impl McpClient for Idle {
        fn list_tools(&self) -> Result<Vec<McpTool>, McpError> {
            Ok(vec![])
        }
        fn call_tool(&self, _: &str, _: serde_json::Value, _: &dyn Fn() -> bool) -> Result<mcp::McpCallResult, McpError> {
            Err(McpError::Cancelled)
        }
    }

    /// The switch in the tab stops the server, rather than leaving it to the
    /// next turn, and the row says so at once.
    #[test]
    fn a_change_stops_what_it_switched_off_and_the_view_shows_it() {
        crate::testing::with_app_dir("cmd-mcp-changed", || {
            let config = mcp_config::save_text(r#"{"mcpServers":{"a":{"command":"x"}}}"#).unwrap();
            let servers = McpServers::new(Arc::new(|_, _, _| Ok(Arc::new(Idle) as Arc<dyn McpClient>)), Arc::new(|_| {}));
            let root = crate::testing::temp_dir("cmd-mcp-changed-root");
            let other = crate::testing::temp_dir("cmd-mcp-changed-other");
            servers.for_turn(&config, &root, &|| false);
            servers.for_turn(&config, &other, &|| false);
            assert_eq!(view(config.clone(), &servers, Some(&root)).unwrap().servers[0].state, McpServerState::Running { tools: vec![], instructions: None });
            assert_eq!(view(config.clone(), &servers, None).unwrap().servers[0].state, McpServerState::NotStarted, "no folder, nothing runs");

            let off = mcp_config::set_enabled("a", false).unwrap();
            let shown = changed(off, &servers, Some(&root)).unwrap();
            assert_eq!(shown.servers[0].state, McpServerState::NotStarted);
            assert_eq!(servers.state("a", &config.mcp_servers["a"], &root), McpServerState::NotStarted, "stopped");
            assert_eq!(servers.state("a", &config.mcp_servers["a"], &other), McpServerState::NotStarted, "in every folder");
        });
    }

    /// The folder closes while the tab or the `/` menu starts its servers:
    /// the close took it out of the registry, and the start, once over,
    /// stops what it brought up rather than leave it running until quit.
    #[test]
    fn a_start_in_a_folder_closed_meanwhile_leaves_nothing_running() {
        use tauri::Manager;
        crate::testing::with_app_dir("cmd-mcp-closed", || {
            let config = mcp_config::save_text(r#"{"mcpServers":{"a":{"command":"x"}}}"#).unwrap();
            let root = crate::infra::paths::canonicalize_plain(&crate::testing::temp_dir("cmd-mcp-closed-root")).unwrap();
            let state = Arc::new(AgentState::default());
            let closing = Arc::clone(&state);
            let servers = Arc::new(McpServers::new(
                Arc::new(move |_, cwd: &Path, _: &dyn Fn() -> bool| {
                    // The close's first half: out of the registry.
                    closing.unregister_if_idle(cwd);
                    Ok(Arc::new(Idle) as Arc<dyn McpClient>)
                }),
                Arc::new(|_| {}),
            ));
            let app = tauri::test::mock_app();
            app.manage(Arc::clone(&state));
            app.manage(Arc::clone(&servers));
            let shown = Some(root.display().to_string());

            state.register_open(root.clone());
            tauri::async_runtime::block_on(mcp_server_connect("a".into(), shown.clone(), app.state(), app.state())).unwrap();
            assert_eq!(servers.state("a", &config.mcp_servers["a"], &root), McpServerState::NotStarted, "the tab's start");

            state.register_open(root.clone());
            tauri::async_runtime::block_on(mcp_prompts_start(shown, app.state(), app.state())).unwrap();
            assert_eq!(servers.state("a", &config.mcp_servers["a"], &root), McpServerState::NotStarted, "the menu's start");
        });
    }

    /// The file is written whatever the window thinks is open: a folder it
    /// closed a moment ago shows no running servers, and loses no edit.
    #[test]
    fn a_change_is_saved_with_a_folder_that_closed_meanwhile() {
        use tauri::Manager;
        crate::testing::with_app_dir("cmd-mcp-stale", || {
            mcp_config::save_text(r#"{"mcpServers":{"a":{"command":"x"}}}"#).unwrap();
            let app = tauri::test::mock_app();
            app.manage(Arc::new(AgentState::default()));
            app.manage(Arc::new(McpServers::new(Arc::new(|_, _, _| Ok(Arc::new(Idle) as Arc<dyn McpClient>)), Arc::new(|_| {}))));
            let gone = Some(crate::testing::temp_dir("cmd-mcp-stale-root").display().to_string());

            let saved = mcp_server_set_enabled("a".into(), false, gone, app.state(), app.state()).unwrap();
            assert!(!saved.servers[0].enabled);
            assert!(mcp_config::load().unwrap().mcp_servers["a"].disabled, "written");
        });
    }

    struct Two;
    impl McpClient for Two {
        fn list_tools(&self) -> Result<Vec<McpTool>, McpError> {
            Ok(["find", "wipe"].map(|name| McpTool { name: name.into(), ..Default::default() }).to_vec())
        }
        fn call_tool(&self, _: &str, _: serde_json::Value, _: &dyn Fn() -> bool) -> Result<mcp::McpCallResult, McpError> {
            Err(McpError::Cancelled)
        }
    }

    /// Hiding a tool is not a reason to restart its server, and the row shows
    /// the tool hidden at once.
    #[test]
    fn a_tools_switch_leaves_the_server_running() {
        crate::testing::with_app_dir("cmd-mcp-tool-switch", || {
            let config = mcp_config::save_text(r#"{"mcpServers":{"a":{"command":"x"}}}"#).unwrap();
            let starts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let counted = Arc::clone(&starts);
            let servers = McpServers::new(Arc::new(move |_, _, _| {
                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(Arc::new(Two) as Arc<dyn McpClient>)
            }), Arc::new(|_| {}));
            let root = crate::testing::temp_dir("cmd-mcp-tool-switch-root");
            servers.for_turn(&config, &root, &|| false);

            let hidden = changed(mcp_config::set_tool_shown("a", "wipe", false).unwrap(), &servers, Some(&root)).unwrap();
            let McpServerState::Running { tools, .. } = &hidden.servers[0].state else { panic!("{:?}", hidden.servers[0].state) };
            assert_eq!(tools.iter().map(|t| t.exposure).collect::<Vec<_>>(), [mcp::Exposure::Direct, mcp::Exposure::Hidden]);

            let next = servers.for_turn(&mcp_config::load().unwrap(), &root, &|| false);
            assert!(next.get("mcp__a__wipe").is_none() && next.get("mcp__a__find").is_some(), "the next turn reads the switch");
            assert_eq!(starts.load(std::sync::atomic::Ordering::SeqCst), 1, "not restarted");
        });
    }
}
