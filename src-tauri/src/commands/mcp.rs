//! The MCP tab and its configuration editor.

use std::sync::Arc;

use serde::Serialize;
use tauri::State;

use crate::commands::chat::AgentState;
use crate::domain::mcp::{self, McpConfig, McpServerItem};
use crate::infra::mcp_config;
use crate::services::mcp_servers::McpServers;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpView {
    /// Where the file is, so the editor can say.
    pub path: String,
    /// The file as it stands, for the editor.
    pub text: String,
    pub servers: Vec<McpServerItem>,
}

fn view(config: McpConfig, servers: &McpServers) -> Result<McpView, String> {
    let items = mcp::items(&config)
        .into_iter()
        .map(|item| McpServerItem { state: servers.state(&item.name, &config.mcp_servers[&item.name]), ..item })
        .collect();
    Ok(McpView {
        path: mcp_config::path().map_err(|e| e.to_string())?.display().to_string(),
        text: mcp_config::read_text().map_err(|e| e.to_string())?,
        servers: items,
    })
}

/// A change stops what it switched off or changed at once; what it added
/// starts with the next Agent turn.
fn changed(config: McpConfig, servers: &McpServers) -> Result<McpView, String> {
    servers.prune(&config);
    view(config, servers)
}

#[tauri::command]
pub fn mcp_config_get(servers: State<'_, Arc<McpServers>>) -> Result<McpView, String> {
    view(mcp_config::load().map_err(|e| e.to_string())?, &servers)
}

#[tauri::command]
pub fn mcp_config_save(text: String, servers: State<'_, Arc<McpServers>>) -> Result<McpView, String> {
    changed(mcp_config::save_text(&text).map_err(|e| e.to_string())?, &servers)
}

#[tauri::command]
pub fn mcp_server_set_enabled(
    name: String,
    enabled: bool,
    servers: State<'_, Arc<McpServers>>,
) -> Result<McpView, String> {
    changed(mcp_config::set_enabled(&name, enabled).map_err(|e| e.to_string())?, &servers)
}

/// Starts one server now, without an Agent turn, so that opening its row in
/// the tab shows what it offers — the user checking a server they just
/// configured should not have to send a message first.
///
/// Off the event loop: a first `npx` run can take the server's whole
/// timeout, and the tab stays answerable meanwhile.
#[tauri::command]
pub async fn mcp_server_connect(
    name: String,
    state: State<'_, Arc<AgentState>>,
    servers: State<'_, Arc<McpServers>>,
) -> Result<McpView, String> {
    let workspace = state.workspace()?;
    let servers = Arc::clone(&servers);
    tauri::async_runtime::spawn_blocking(move || {
        let config = mcp_config::load().map_err(|e| e.to_string())?;
        servers.connect(&name, &config, &workspace);
        view(config, &servers)
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

/// The prompts of the servers running for the open folder. Starts nothing:
/// a server's prompts are offered once a turn or the tab has started it.
#[tauri::command]
pub fn mcp_prompts(state: State<'_, Arc<AgentState>>, servers: State<'_, Arc<McpServers>>) -> Vec<McpPromptItem> {
    let Ok(workspace) = state.workspace() else { return Vec::new() };
    servers.prompts(&workspace).into_iter().map(|(server, prompt)| McpPromptItem { server, prompt }).collect()
}

/// A prompt written with what was typed after its name — the text the
/// composer sends. Off the event loop: it is a request to the server.
#[tauri::command]
pub async fn mcp_prompt_get(
    server: String,
    name: String,
    typed: String,
    state: State<'_, Arc<AgentState>>,
    servers: State<'_, Arc<McpServers>>,
) -> Result<String, String> {
    let workspace = state.workspace()?;
    let servers = Arc::clone(&servers);
    tauri::async_runtime::spawn_blocking(move || servers.get_prompt(&server, &name, &typed, &workspace).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::mcp::{McpClient, McpError, McpServerState, McpTool};

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
            let servers = McpServers::new(Arc::new(|_, _, _| Ok(Arc::new(Idle) as Arc<dyn McpClient>)));
            servers.for_turn(&config, &crate::testing::temp_dir("cmd-mcp-changed-root"), &|| false);
            assert_eq!(view(config.clone(), &servers).unwrap().servers[0].state, McpServerState::Running { tools: vec![], instructions: None });

            let off = mcp_config::set_enabled("a", false).unwrap();
            let shown = changed(off, &servers).unwrap();
            assert_eq!(shown.servers[0].state, McpServerState::NotStarted);
            assert_eq!(servers.state("a", &config.mcp_servers["a"]), McpServerState::NotStarted, "stopped");
        });
    }
}
