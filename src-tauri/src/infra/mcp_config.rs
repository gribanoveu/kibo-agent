//! `<app dir>/mcp.json`: the MCP servers, in the `mcpServers` format.
//!
//! Its own file, not a section of `settings.json`, because a server's `env`
//! routinely holds a token (`GITHUB_PERSONAL_ACCESS_TOKEN`), and
//! `settings.json` promises to hold no secret — it is the file that gets
//! pasted into a bug report. This one is owner-only, like the rest of the
//! app directory, and is never shown anywhere but its own editor.
//!
//! What the user typed is written as typed once it parses: their layout and
//! key order survive, and a save from the editor is not a reformat.

use std::fs;
use std::path::PathBuf;

use crate::domain::mcp::{self, McpConfig, McpConfigError};
use crate::infra::app_dir;

const FILE: &str = "mcp.json";

/// What an empty editor starts from.
pub const TEMPLATE: &str = "{\n  \"mcpServers\": {}\n}\n";

pub fn path() -> Result<PathBuf, McpConfigError> {
    Ok(app_dir::dir().map_err(McpConfigError::Read)?.join(FILE))
}

/// The file's text, or the template while there is none.
pub fn read_text() -> Result<String, McpConfigError> {
    match fs::read_to_string(path()?) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(TEMPLATE.to_string()),
        Err(e) => Err(McpConfigError::Read(e.to_string())),
    }
}

pub fn load() -> Result<McpConfig, McpConfigError> {
    mcp::parse(&read_text()?)
}

/// Refuses text that does not parse, leaving the file as it was.
pub fn save_text(text: &str) -> Result<McpConfig, McpConfigError> {
    let config = mcp::parse(text)?;
    app_dir::write_private(&path()?, text.as_bytes()).map_err(McpConfigError::Write)?;
    Ok(config)
}

/// The switch in the tab. Rewrites the file from the parsed config, so this
/// one — unlike a save from the editor — does reformat it.
pub fn set_enabled(name: &str, enabled: bool) -> Result<McpConfig, McpConfigError> {
    change(name, |server| server.disabled = !enabled)
}

/// A tool's switch in the tab: offered to the model or hidden. Reformats the
/// file, as [`set_enabled`] does.
pub fn set_tool_shown(name: &str, tool: &str, shown: bool) -> Result<McpConfig, McpConfigError> {
    change(name, |server| server.show_tool(tool, shown))
}

fn change(name: &str, edit: impl FnOnce(&mut mcp::McpServerConfig)) -> Result<McpConfig, McpConfigError> {
    let mut config = load()?;
    edit(config.mcp_servers.get_mut(name).ok_or_else(|| McpConfigError::NotFound(name.to_string()))?);
    let text = serde_json::to_string_pretty(&config).map_err(|e| McpConfigError::Write(e.to_string()))?;
    app_dir::write_private(&path()?, format!("{text}\n").as_bytes()).map_err(McpConfigError::Write)?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::with_app_dir;

    #[test]
    fn nothing_saved_is_the_template_and_no_servers() {
        with_app_dir("mcp-config-empty", || {
            assert_eq!(read_text().unwrap(), TEMPLATE);
            assert!(load().unwrap().mcp_servers.is_empty());
        });
    }

    #[test]
    fn a_save_keeps_the_text_as_typed() {
        with_app_dir("mcp-config-save", || {
            let text = "{\"mcpServers\":{\"a\":{\"command\":\"x\"}}}";
            save_text(text).unwrap();
            assert_eq!(read_text().unwrap(), text);
            assert_eq!(load().unwrap().mcp_servers["a"].command, "x");
        });
    }

    /// A typo in the editor must not cost the servers that were working.
    #[test]
    fn text_that_does_not_parse_leaves_the_file_alone() {
        with_app_dir("mcp-config-refused", || {
            save_text(TEMPLATE).unwrap();
            assert!(matches!(save_text("{\"mcpServers\": {"), Err(McpConfigError::Parse(_))));
            assert_eq!(read_text().unwrap(), TEMPLATE);
        });
    }

    #[test]
    fn a_server_is_switched_off_and_on_by_name() {
        with_app_dir("mcp-config-toggle", || {
            save_text("{\"mcpServers\":{\"a\":{\"command\":\"x\",\"type\":\"stdio\"}}}").unwrap();
            assert!(set_enabled("a", false).unwrap().mcp_servers["a"].disabled);
            let config = load().unwrap();
            assert!(config.mcp_servers["a"].disabled);
            assert_eq!(config.mcp_servers["a"].extra["type"], "stdio", "the rest of the entry survives");
            assert!(!set_enabled("a", true).unwrap().mcp_servers["a"].disabled);
            assert!(matches!(set_enabled("b", true), Err(McpConfigError::NotFound(_))));
        });
    }

    #[test]
    fn a_tool_is_hidden_and_shown_by_name() {
        with_app_dir("mcp-config-tool", || {
            save_text(r#"{"mcpServers":{"a":{"command":"x"}}}"#).unwrap();
            assert_eq!(set_tool_shown("a", "wipe", false).unwrap().mcp_servers["a"].exposure_of("wipe"), mcp::Exposure::Hidden);
            assert_eq!(load().unwrap().mcp_servers["a"].exposure_of("wipe"), mcp::Exposure::Hidden, "on disk");
            assert!(set_tool_shown("a", "wipe", true).unwrap().mcp_servers["a"].tool_exposure.is_empty());
            assert!(matches!(set_tool_shown("b", "wipe", true), Err(McpConfigError::NotFound(_))));
        });
    }

    /// It holds tokens.
    #[cfg(unix)]
    #[test]
    fn the_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        with_app_dir("mcp-config-private", || {
            save_text(TEMPLATE).unwrap();
            let mode = fs::metadata(path().unwrap()).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0, "{mode:o}");
        });
    }
}
