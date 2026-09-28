//! MCP servers as the user configures them.
//!
//! The format is the `mcpServers` object Claude Desktop and Cursor use, so a
//! configuration copied from a server's README pastes in unchanged. Fields
//! this app adds (`weight`, `timeoutSecs`) sit beside the standard ones, and
//! fields it does not know — `type`, another client's own — are kept rather
//! than dropped on the next save.
//!
//! Decisions behind this (names, approval, failure, weight, log) are in
//! `docs/06-port-plan.md`, stage 7, "Решения по MCP".

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;

use crate::domain::llm::LlmToolDefinition;

/// Loop weight of one call when the server does not say: as much as a
/// `grep`. Nothing is known about what a foreign tool costs, and the
/// iteration cap catches a weight that turns out wrong.
pub const DEFAULT_WEIGHT: u32 = 3;
pub const DEFAULT_TIMEOUT_SECS: u64 = 120;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpConfig {
    #[serde(default)]
    pub mcp_servers: BTreeMap<String, McpServerConfig>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerConfig {
    /// Empty for an HTTP server's entry, which has a `url` instead.
    #[serde(default)]
    pub command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// Where an HTTP server listens — Streamable HTTP, one endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Sent with every request to `url`; routinely a token, so never shown
    /// outside the file's own editor.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    /// Cline's and Cursor's spelling for a server kept but not started.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub disabled: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl McpServerConfig {
    pub fn weight(&self) -> u32 {
        self.weight.unwrap_or(DEFAULT_WEIGHT)
    }

    pub fn timeout_secs(&self) -> u64 {
        self.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS)
    }
}

/// One row of the MCP tab.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerItem {
    pub name: String,
    /// The command line as it will run, or the URL — for reading, not for
    /// running.
    pub command: String,
    pub enabled: bool,
    /// Why this server will not start, when that is already known from its
    /// entry alone.
    pub error: Option<String>,
    /// Something wrong that does not stop it from starting.
    pub warning: Option<String>,
    /// What its process is doing. `items` does not know — it reads only the
    /// file — and says `NotStarted`; the running servers fill it in.
    pub state: McpServerState,
}

/// A server's process, as the tab shows it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum McpServerState {
    /// Servers start with the first Agent turn after the file names them.
    #[default]
    NotStarted,
    Starting,
    Running { tools: Vec<McpToolInfo> },
    /// It was running and stopped; the next call to it starts it again.
    Exited { error: String },
    /// It never started. Not retried turn after turn — a server that hangs
    /// on start would cost every turn its timeout — until its entry changes
    /// or it is switched off and on.
    Failed { error: String },
}

/// One tool a running server offers, as the tab lists it. The schema is
/// the model's business, not the tab's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct McpToolInfo {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Error)]
pub enum McpConfigError {
    #[error("could not read the MCP configuration: {0}")]
    Read(String),
    #[error("the MCP configuration is not valid: {0}")]
    Parse(String),
    #[error("could not write the MCP configuration: {0}")]
    Write(String),
    #[error("no MCP server named {0:?}")]
    NotFound(String),
}

/// The server's part of a tool name, `mcp__<key>__<tool>`: what the
/// providers accept in a name (`[A-Za-z0-9_-]`), lowercased so two spellings
/// of one server cannot become two prefixes.
pub fn server_key(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c.to_ascii_lowercase() } else { '_' })
        .collect()
}

/// The rows of the tab, in name order, each with what is wrong with it.
pub fn items(config: &McpConfig) -> Vec<McpServerItem> {
    let mut seen: HashMap<String, &str> = HashMap::new();
    config
        .mcp_servers
        .iter()
        .map(|(name, server)| {
            let key = server_key(name);
            let clash = seen.get(key.as_str()).map(|first| {
                format!("clashes with \"{first}\": both become \"{key}\" in tool names — rename one")
            });
            seen.entry(key).or_insert(name);
            McpServerItem {
                name: name.clone(),
                command: server.url.clone().unwrap_or_else(|| command_line(server)),
                enabled: !server.disabled,
                error: clash.or_else(|| problem(name, server)),
                warning: warning(server),
                state: McpServerState::NotStarted,
            }
        })
        .collect()
}

fn problem(name: &str, server: &McpServerConfig) -> Option<String> {
    if name.trim().is_empty() {
        return Some("the server needs a name".into());
    }
    let has_command = !server.command.trim().is_empty();
    match &server.url {
        Some(_) if has_command => return Some("has both a command and a url — keep one".into()),
        Some(url) if url_parts(url).is_none() => {
            return Some(format!("url must be an http:// or https:// address with a host, not {url:?}"));
        }
        None if !has_command => return Some("no command to start it with".into()),
        _ => {}
    }
    if server.extra.get("type").and_then(Value::as_str) == Some("sse") {
        return Some("uses the old HTTP+SSE transport, which is not supported — only Streamable HTTP".into());
    }
    if server.weight == Some(0) {
        return Some("weight must be at least 1".into());
    }
    if server.timeout_secs == Some(0) {
        return Some("timeoutSecs must be at least 1".into());
    }
    None
}

/// Plain `http://` to anywhere but this machine: the headers — a token, as a
/// rule — travel readable by anyone on the way. Allowed, since a server on
/// the local network may have no TLS, but said.
fn warning(server: &McpServerConfig) -> Option<String> {
    let (scheme, host) = url_parts(server.url.as_deref()?)?;
    let local = host == "localhost" || host.starts_with("127.") || host == "[::1]";
    (scheme == "http" && !local)
        .then(|| format!("{host} is reached over plain http — its headers, and any token in them, are sent unencrypted"))
}

/// Scheme and host of an `http`/`https` URL, lowercased; `None` for anything
/// else. `http::Uri` lowercases those two schemes itself.
fn url_parts(url: &str) -> Option<(String, String)> {
    let uri: http::Uri = url.parse().ok()?;
    let scheme = uri.scheme_str()?.to_string();
    let host = uri.host().filter(|h| !h.is_empty())?.to_ascii_lowercase();
    (scheme == "http" || scheme == "https").then_some((scheme, host))
}

fn command_line(server: &McpServerConfig) -> String {
    std::iter::once(server.command.as_str())
        .chain(server.args.iter().map(String::as_str))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Parses the file's text. The whole file is refused only when it is not
/// JSON of this shape; a server that cannot run is a row with an error.
pub fn parse(text: &str) -> Result<McpConfig, McpConfigError> {
    if text.trim().is_empty() {
        return Ok(McpConfig::default());
    }
    serde_json::from_str(text).map_err(|e| McpConfigError::Parse(e.to_string()))
}

// ------------------------------------------------------------ the client

/// One tool a server offers, as the model will need to see it.
#[derive(Debug, Clone, PartialEq)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    /// A JSON Schema, passed through untouched like a built-in tool's.
    pub input_schema: Value,
}

/// What a call came back with, already reduced to the text a tool result is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpCallResult {
    pub text: String,
    /// The tool itself reported failure (`isError`): a result for the model
    /// to read, not a broken connection.
    pub is_error: bool,
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum McpError {
    #[error("could not start the MCP server: {0}")]
    NotStarted(String),
    #[error("the MCP server exited{}{}", code.map(|c| format!(" with code {c}")).unwrap_or_default(), last_output(stderr))]
    Exited { code: Option<i32>, stderr: String },
    /// An HTTP server's non-2xx answer, with the start of its body — the
    /// explanation is usually there.
    #[error("the MCP server answered HTTP {status}{}{}", status_hint(*status), last_output(body))]
    Http { status: u16, body: String },
    #[error("could not reach the MCP server: {0}")]
    Unreachable(String),
    #[error("the MCP server did not answer within {0} s")]
    Timeout(u64),
    #[error("cancelled")]
    Cancelled,
    #[error("the MCP server refused to start a session: {0}")]
    Handshake(String),
    #[error("the MCP server answered with an error: {message} (code {code})")]
    Server { code: i64, message: String },
    #[error("the MCP server's answer is not what the protocol says: {0}")]
    Protocol(String),
    #[error("the MCP server stopped again after its restart this turn; it is started once more with the next turn")]
    NotRestarted,
}

/// 401 is how a server says it wants OAuth, which is not built yet
/// (`docs/17-mcp-http.md`); a token in `headers` is what works today.
/// 405 to a POST is almost always the old HTTP+SSE transport's `/sse`
/// endpoint, which takes only GET and is not supported.
fn status_hint(status: u16) -> &'static str {
    match status {
        401 => " — it wants a sign-in (OAuth), which this app does not do yet; a token in the entry's \"headers\" works if the server accepts one",
        405 => " — the url does not take POST; an address ending in /sse is the old SSE transport, which this app does not speak: use the server's Streamable HTTP address instead (for Gradio, /gradio_api/mcp/http/)",
        _ => "",
    }
}

fn last_output(stderr: &str) -> String {
    if stderr.trim().is_empty() {
        String::new()
    } else {
        format!(". Its last output:\n{stderr}")
    }
}

/// A connected server. The port between the loop and a transport: the stdio
/// client in `infra::mcp_stdio` is one implementation, and an HTTP one (the
/// place for `rmcp`, with OAuth) would be another — nothing above this trait
/// learns which.
///
/// Blocking, like `LlmProvider`, and for the same reasons.
pub trait McpClient: Send + Sync {
    fn list_tools(&self) -> Result<Vec<McpTool>, McpError>;

    /// `cancelled` is polled while waiting; a stop tells the server
    /// (`notifications/cancelled`) and returns `McpError::Cancelled` without
    /// waiting for it to agree.
    fn call_tool(
        &self,
        name: &str,
        arguments: Value,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<McpCallResult, McpError>;

    /// `false` once the server is known to be gone — its process exited, its
    /// stream closed. A client that cannot tell says `true`, and the next
    /// call finds out.
    fn is_alive(&self) -> bool {
        true
    }
}

// ------------------------------------------------------ the turn's view

/// A server that answered: what it is called, what a call to it costs, and
/// what it offers.
pub struct ConnectedServer {
    pub name: String,
    pub weight: u32,
    pub client: Arc<dyn McpClient>,
    pub tools: Vec<McpTool>,
}

/// One tool as the turn sees it: the name the model calls it by, and where
/// the call goes.
pub struct McpToolEntry {
    pub wire_name: String,
    pub server: String,
    pub tool: McpTool,
    pub weight: u32,
    pub client: Arc<dyn McpClient>,
}

/// Every connected server's tools, named for the model. Cheap to clone —
/// every call's `ToolDeps` carries one.
#[derive(Clone, Default)]
pub struct McpTools(Arc<Vec<McpToolEntry>>);

/// What providers accept as a tool name, OpenAI's and Anthropic's alike.
pub const MAX_TOOL_NAME_CHARS: usize = 64;

impl McpTools {
    /// Names every tool `mcp__<server key>__<tool>`. A name past the length
    /// limit is cut and given a hash of the whole, so two long names that
    /// share a beginning stay two names. A tool whose name collides with one
    /// already taken — two spellings a server offers that sanitize alike —
    /// is left out rather than made to shadow the first.
    pub fn new(servers: Vec<ConnectedServer>) -> Self {
        let mut taken = std::collections::HashSet::new();
        let mut entries = Vec::new();
        for server in servers {
            let key = server_key(&server.name);
            for tool in server.tools {
                let wire_name = tool_wire_name(&key, &tool.name);
                if !taken.insert(wire_name.clone()) {
                    continue;
                }
                entries.push(McpToolEntry {
                    wire_name,
                    server: server.name.clone(),
                    tool,
                    weight: server.weight,
                    client: Arc::clone(&server.client),
                });
            }
        }
        Self(Arc::new(entries))
    }

    pub fn get(&self, wire_name: &str) -> Option<&McpToolEntry> {
        self.0.iter().find(|entry| entry.wire_name == wire_name)
    }

    /// What one call costs: the server's weight, or the default for a name
    /// no server has — a guess still moves the budget.
    pub fn weight(&self, wire_name: &str) -> u32 {
        self.get(wire_name).map_or(DEFAULT_WEIGHT, |entry| entry.weight)
    }

    /// The schemas the model is shown. The description says which server a
    /// tool belongs to: the model otherwise has no way to tell `search` on
    /// one from `search` on another, or either from a built-in tool.
    pub fn definitions(&self) -> Vec<LlmToolDefinition> {
        self.0
            .iter()
            .map(|entry| LlmToolDefinition {
                name: entry.wire_name.clone(),
                description: format!("[MCP server \"{}\"] {}", entry.server, entry.tool.description),
                parameters: entry.tool.input_schema.clone(),
            })
            .collect()
    }
}

fn tool_wire_name(server_key: &str, tool: &str) -> String {
    let tool: String =
        tool.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect();
    let full = format!("{}{server_key}__{tool}", crate::domain::tools::MCP_PREFIX);
    if full.len() <= MAX_TOOL_NAME_CHARS {
        return full;
    }
    // FNV-1a: stable across builds, which `DefaultHasher` does not promise,
    // and a name must not change under a saved "always allow".
    let hash = full.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    let suffix = format!("_{:08x}", hash as u32);
    format!("{}{suffix}", &full[..MAX_TOOL_NAME_CHARS - suffix.len()])
}

/// A `tools/call` result's `content` as one text for the model.
///
/// Text as it is. What cannot be text here — an image, audio, a binary
/// resource — is named in its place rather than dropped: a result that
/// silently lost its only block reads as an empty success. Structured
/// content is used only when there is no content at all, which the
/// specification allows and older servers do not do.
pub fn render_content(result: &Value) -> String {
    let parts: Vec<String> = result["content"]
        .as_array()
        .map(|blocks| blocks.iter().map(render_block).collect())
        .unwrap_or_default();
    if parts.is_empty() {
        return match result.get("structuredContent") {
            Some(structured) if !structured.is_null() => structured.to_string(),
            _ => String::new(),
        };
    }
    parts.join("\n")
}

fn render_block(block: &Value) -> String {
    let field = |name: &str| block[name].as_str().unwrap_or_default();
    match field("type") {
        "text" => field("text").to_string(),
        "image" | "audio" => format!("[{} omitted: {}]", field("type"), field("mimeType")),
        "resource" => match block["resource"]["text"].as_str() {
            Some(text) => text.to_string(),
            None => format!("[binary resource omitted: {}]", block["resource"]["uri"].as_str().unwrap_or_default()),
        },
        "resource_link" => format!("[resource: {}]", field("uri")),
        other => format!("[{other} content omitted]"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A README's snippet, verbatim — the reason for this format.
    #[test]
    fn a_claude_desktop_config_reads_as_is() {
        let config = parse(
            r#"{"mcpServers": {"github": {"command": "npx", "args": ["-y", "@modelcontextprotocol/server-github"],
                "env": {"GITHUB_PERSONAL_ACCESS_TOKEN": "t"}}}}"#,
        )
        .unwrap();
        let github = &config.mcp_servers["github"];
        assert_eq!(github.args, ["-y", "@modelcontextprotocol/server-github"]);
        assert_eq!(github.env["GITHUB_PERSONAL_ACCESS_TOKEN"], "t");
        assert_eq!((github.weight(), github.timeout_secs()), (DEFAULT_WEIGHT, DEFAULT_TIMEOUT_SECS));
        assert_eq!(
            items(&config),
            [McpServerItem {
                name: "github".into(),
                command: "npx -y @modelcontextprotocol/server-github".into(),
                enabled: true,
                error: None,
                warning: None,
                state: McpServerState::NotStarted,
            }]
        );
    }

    /// Another client's fields survive a save made here.
    #[test]
    fn unknown_fields_are_kept() {
        let text = r#"{"mcpServers":{"a":{"command":"x","type":"stdio","autoApprove":["t"]}}}"#;
        let config = parse(text).unwrap();
        let written: Value = serde_json::to_value(&config).unwrap();
        assert_eq!(written["mcpServers"]["a"]["type"], "stdio");
        assert_eq!(written["mcpServers"]["a"]["autoApprove"], serde_json::json!(["t"]));
        assert!(written["mcpServers"]["a"].get("disabled").is_none(), "defaults are not written out");
    }

    /// A mixed config loads; the server this build cannot run says why.
    #[test]
    fn a_server_that_cannot_run_is_a_row_with_the_reason() {
        let config = parse(
            r#"{"mcpServers":{
                "remote":{"url":"https://example.com/mcp"},
                "nothing":{"args":["--flag"]},
                "free":{"command":"x","weight":0},
                "instant":{"command":"x","timeoutSecs":0},
                "off":{"command":"x","disabled":true}
            }}"#,
        )
        .unwrap();
        let rows: BTreeMap<String, McpServerItem> =
            items(&config).into_iter().map(|i| (i.name.clone(), i)).collect();
        assert_eq!(rows["remote"].error, None, "an HTTP server runs");
        assert_eq!(rows["remote"].command, "https://example.com/mcp");
        assert!(rows["nothing"].error.as_deref().unwrap().contains("no command"));
        assert_eq!(rows["nothing"].command, "--flag", "no leading space for the missing command");
        assert!(rows["free"].error.as_deref().unwrap().contains("weight"));
        assert!(rows["instant"].error.as_deref().unwrap().contains("timeoutSecs"));
        assert_eq!((rows["off"].enabled, rows["off"].error.clone()), (false, None));
    }

    /// Claude Code's and Cursor's HTTP entry, verbatim: the URL is the row's
    /// command, the headers go nowhere but the file.
    #[test]
    fn an_http_entry_reads_as_is_and_its_headers_stay_out_of_the_row() {
        let text = r#"{"mcpServers":{"ctx":{"type":"http","url":"https://mcp.example.com/mcp",
            "headers":{"Authorization":"Bearer secret"}}}}"#;
        let config = parse(text).unwrap();
        let ctx = &config.mcp_servers["ctx"];
        assert_eq!(ctx.url.as_deref(), Some("https://mcp.example.com/mcp"));
        assert_eq!(ctx.headers["Authorization"], "Bearer secret");
        let row = &items(&config)[0];
        assert_eq!(row.command, "https://mcp.example.com/mcp");
        assert_eq!(row.warning, None);
        assert!(!format!("{row:?}").contains("secret"));
        let written = serde_json::to_value(&config).unwrap();
        assert_eq!(written["mcpServers"]["ctx"]["type"], "http", "type survives a save");
        assert_eq!(written["mcpServers"]["ctx"]["headers"]["Authorization"], "Bearer secret");
        let stdio = serde_json::to_value(parse(r#"{"mcpServers":{"a":{"command":"x"}}}"#).unwrap()).unwrap();
        assert!(stdio["mcpServers"]["a"].get("url").is_none() && stdio["mcpServers"]["a"].get("headers").is_none());
    }

    #[test]
    fn an_http_entry_that_cannot_run_says_why() {
        let config = parse(
            r#"{"mcpServers":{
                "both":{"command":"x","url":"https://a.example/mcp"},
                "ftp":{"url":"ftp://a.example/mcp"},
                "hostless":{"url":"https:///mcp"},
                "port-only":{"url":"http://:8080/mcp"},
                "words":{"url":"not a url"},
                "spaced":{"url":" https://a.example/mcp"},
                "legacy":{"type":"sse","url":"https://a.example/sse"},
                "legacy-stdio":{"type":"sse","command":"x"}
            }}"#,
        )
        .unwrap();
        let rows: BTreeMap<String, McpServerItem> =
            items(&config).into_iter().map(|i| (i.name.clone(), i)).collect();
        let error = |name: &str| rows[name].error.clone().unwrap_or_default();
        assert!(error("both").contains("both a command and a url"), "{}", error("both"));
        for name in ["ftp", "hostless", "port-only", "words", "spaced"] {
            assert!(error(name).contains("http:// or https://"), "{name}: {}", error(name));
        }
        assert!(error("legacy").contains("HTTP+SSE"), "{}", error("legacy"));
        assert!(error("legacy-stdio").contains("HTTP+SSE"), "type is read whatever else the entry has");
    }

    /// Warned, not refused: a server on the local network may have no TLS.
    #[test]
    fn plain_http_off_this_machine_is_a_warning() {
        let config = parse(
            r#"{"mcpServers":{
                "lan":{"url":"HTTP://Box.lan:8080/mcp"},
                "tls":{"url":"https://box.lan/mcp"},
                "local":{"url":"http://localhost:3000/mcp"},
                "loop":{"url":"http://127.0.0.1:3000/mcp"},
                "v6":{"url":"http://[::1]:3000/mcp"},
                "cmd":{"command":"x"}
            }}"#,
        )
        .unwrap();
        let rows: BTreeMap<String, McpServerItem> =
            items(&config).into_iter().map(|i| (i.name.clone(), i)).collect();
        let warning = rows["lan"].warning.as_deref().unwrap();
        assert!(warning.contains("box.lan") && warning.contains("unencrypted"), "{warning}");
        for name in ["tls", "local", "loop", "v6", "cmd"] {
            assert_eq!(rows[name].warning, None, "{name}");
        }
        assert_eq!(rows["lan"].error, None, "a warning, not an error");
    }

    /// Two entries that would share a tool-name prefix: the second is the
    /// one reported, so the first keeps working.
    #[test]
    fn names_that_collapse_to_one_prefix_clash() {
        let config = parse(
            r#"{"mcpServers":{"My Server":{"command":"x"},"my server":{"command":"y"},"my_server":{"command":"z"}}}"#,
        )
        .unwrap();
        let rows = items(&config);
        assert_eq!(rows[0].error, None);
        // Every later one names the first, the one that keeps the prefix.
        for row in &rows[1..] {
            assert!(row.error.as_deref().unwrap().contains("\"My Server\""), "{:?}", row.error);
        }
    }

    struct Nothing;
    impl McpClient for Nothing {
        fn list_tools(&self) -> Result<Vec<McpTool>, McpError> {
            Ok(vec![])
        }
        fn call_tool(&self, _: &str, _: Value, _: &dyn Fn() -> bool) -> Result<McpCallResult, McpError> {
            Err(McpError::Cancelled)
        }
    }

    fn server(name: &str, weight: u32, tools: &[&str]) -> ConnectedServer {
        ConnectedServer {
            name: name.into(),
            weight,
            client: Arc::new(Nothing),
            tools: tools
                .iter()
                .map(|t| McpTool { name: t.to_string(), description: format!("does {t}"), input_schema: serde_json::json!({"type":"object"}) })
                .collect(),
        }
    }

    #[test]
    fn tools_are_named_for_their_server_and_described_as_its() {
        let tools = McpTools::new(vec![server("GitHub", 5, &["search_issues", "get.file"]), server("db", 3, &["search_issues"])]);
        let definitions = tools.definitions();
        let names: Vec<&str> = definitions.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["mcp__github__search_issues", "mcp__github__get_file", "mcp__db__search_issues"]);
        assert_eq!(definitions[0].description, "[MCP server \"GitHub\"] does search_issues");
        assert_eq!(tools.get("mcp__github__get_file").unwrap().tool.name, "get.file", "the server is called by its own name");
        assert_eq!((tools.weight("mcp__github__search_issues"), tools.weight("mcp__db__search_issues")), (5, 3));
        assert_eq!(tools.weight("mcp__gone__x"), DEFAULT_WEIGHT);
    }

    /// Two of a server's names that sanitize alike: the first keeps it.
    #[test]
    fn a_name_already_taken_is_not_offered_twice() {
        let tools = McpTools::new(vec![server("s", 3, &["a.b", "a_b"])]);
        assert_eq!(tools.definitions().len(), 1);
        assert_eq!(tools.get("mcp__s__a_b").unwrap().tool.name, "a.b");
    }

    /// Past 64 characters: cut, and told apart by a hash of the whole.
    #[test]
    fn a_long_name_is_cut_to_the_limit_and_stays_distinct() {
        let long = "x".repeat(80);
        let tools = McpTools::new(vec![server("s", 3, &[&format!("{long}_one"), &format!("{long}_two")])]);
        let names: Vec<String> = tools.definitions().into_iter().map(|d| d.name).collect();
        assert!(names.iter().all(|n| n.len() == MAX_TOOL_NAME_CHARS && n.starts_with("mcp__s__xxx")), "{names:?}");
        assert_ne!(names[0], names[1]);
        assert_eq!(tool_wire_name("s", &format!("{long}_one")), names[0], "stable");
    }

    #[test]
    fn a_server_key_is_what_a_tool_name_may_hold() {
        assert_eq!(server_key("My Server.v2"), "my_server_v2");
        assert_eq!(server_key("git-hub"), "git-hub");
    }

    #[test]
    fn content_becomes_one_text_and_what_cannot_be_text_is_named() {
        let result = serde_json::json!({"content": [
            {"type": "text", "text": "found 2"},
            {"type": "image", "data": "AAAA", "mimeType": "image/png"},
            {"type": "resource", "resource": {"uri": "file:///a", "text": "body"}},
            {"type": "resource", "resource": {"uri": "file:///b", "blob": "AAAA"}},
            {"type": "resource_link", "uri": "file:///c", "name": "c"},
            {"type": "hologram"}
        ]});
        assert_eq!(
            render_content(&result),
            "found 2\n[image omitted: image/png]\nbody\n[binary resource omitted: file:///b]\n[resource: file:///c]\n[hologram content omitted]"
        );
    }

    #[test]
    fn structured_content_stands_in_only_when_there_is_no_content() {
        assert_eq!(render_content(&serde_json::json!({"content": [], "structuredContent": {"n": 1}})), r#"{"n":1}"#);
        assert_eq!(
            render_content(&serde_json::json!({"content": [{"type": "text", "text": "t"}], "structuredContent": {"n": 1}})),
            "t"
        );
        assert_eq!(render_content(&serde_json::json!({})), "");
    }

    /// What the model and the tab read when a server dies.
    #[test]
    fn an_exit_says_the_code_and_the_last_output() {
        let err = McpError::Exited { code: Some(1), stderr: "Error: GITHUB_TOKEN is not set".into() };
        assert_eq!(err.to_string(), "the MCP server exited with code 1. Its last output:\nError: GITHUB_TOKEN is not set");
        assert_eq!(McpError::Exited { code: None, stderr: " ".into() }.to_string(), "the MCP server exited");
    }

    /// What the model and the tab read when an HTTP server refuses.
    #[test]
    fn an_http_refusal_says_the_status_and_the_body_and_401_names_oauth() {
        let err = McpError::Http { status: 500, body: "database is down".into() };
        assert_eq!(err.to_string(), "the MCP server answered HTTP 500. Its last output:\ndatabase is down");
        let err = McpError::Http { status: 401, body: String::new() };
        assert!(err.to_string().starts_with("the MCP server answered HTTP 401 — it wants a sign-in (OAuth)"), "{err}");
        assert!(err.to_string().contains("\"headers\""), "{err}");
        let err = McpError::Http { status: 405, body: "Method Not Allowed".into() };
        assert!(err.to_string().starts_with("the MCP server answered HTTP 405 — the url does not take POST; an address ending in /sse"), "{err}");
    }

    #[test]
    fn an_empty_file_is_no_servers_and_broken_json_is_refused() {
        assert_eq!(parse("  ").unwrap(), McpConfig::default());
        assert!(matches!(parse("{\"mcpServers\": ["), Err(McpConfigError::Parse(_))));
        assert!(matches!(parse(r#"{"mcpServers": {"a": {"args": "x"}}}"#), Err(McpConfigError::Parse(_))));
    }
}
