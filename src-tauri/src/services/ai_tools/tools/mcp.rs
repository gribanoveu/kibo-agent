//! A call to a connected MCP server's tool — through the same dispatcher,
//! and so the same approval gate, budget and log, as every built-in tool.
//! Decisions behind it: `docs/06-port-plan.md`, stage 7, "Решения по MCP".

use crate::domain::mcp::McpError;
use crate::domain::tools::{McpCallArgs, ToolDeps, ToolError, ToolResult};

pub fn mcp(args: &McpCallArgs, deps: &ToolDeps) -> Result<ToolResult, ToolError> {
    // A name no connected server has: a guess, or a server that has gone
    // since the model saw it. Either way, the model's to correct.
    let entry = deps.mcp.get(&args.name).ok_or_else(|| ToolError::UnknownTool(args.name.clone()))?;
    let never = || false;
    let cancelled = deps.cancelled.unwrap_or(&never);
    match entry.client.call_tool(&entry.tool.name, args.arguments.clone(), cancelled) {
        Ok(result) if result.is_error => Err(ToolError::McpToolFailed(result.text)),
        Ok(result) => Ok(ToolResult::Mcp { text: result.text }),
        Err(error @ McpError::Cancelled) => Err(ToolError::McpUnavailable(error.to_string())),
        Err(error) => Err(ToolError::McpUnavailable(format!("\"{}\": {error}", entry.server))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::mcp::{ConnectedServer, McpCallResult, McpClient, McpTool, McpTools};
    use serde_json::{json, Value};
    use std::sync::{Arc, Mutex};

    /// Answers by the tool name it is asked for; remembers what it was asked.
    #[derive(Default)]
    pub(crate) struct Scripted {
        pub asked: Mutex<Vec<(String, Value)>>,
    }

    impl McpClient for Scripted {
        fn list_tools(&self) -> Result<Vec<McpTool>, McpError> {
            Ok(vec![])
        }
        fn call_tool(&self, name: &str, arguments: Value, cancelled: &dyn Fn() -> bool) -> Result<McpCallResult, McpError> {
            self.asked.lock().unwrap().push((name.to_string(), arguments));
            match name {
                "ok" => Ok(McpCallResult { text: "done".into(), is_error: false }),
                "fails" => Ok(McpCallResult { text: "no such issue".into(), is_error: true }),
                "waits" if cancelled() => Err(McpError::Cancelled),
                _ => Err(McpError::Exited { code: Some(1), stderr: String::new() }),
            }
        }
    }

    fn deps(client: Arc<Scripted>) -> McpTools {
        let tool = |name: &str| McpTool { name: name.into(), input_schema: json!({}), ..Default::default() };
        McpTools::new(vec![ConnectedServer {
            name: "gh".into(),
            weight: 3,
            client,
            tools: vec![tool("ok"), tool("fails"), tool("waits"), tool("dies")],
            instructions: None,
        }])
    }

    fn call(name: &str) -> McpCallArgs {
        McpCallArgs { name: name.into(), arguments: json!({"q": 1}) }
    }

    #[test]
    fn a_call_reaches_the_server_by_its_own_name_and_returns_its_text() {
        let client = Arc::new(Scripted::default());
        let deps = ToolDeps { mcp: deps(Arc::clone(&client)), ..ToolDeps::default() };
        assert_eq!(mcp(&call("mcp__gh__ok"), &deps).unwrap(), ToolResult::Mcp { text: "done".into() });
        assert_eq!(*client.asked.lock().unwrap(), [("ok".to_string(), json!({"q": 1}))]);
    }

    /// The tool's own failure is its text, for the model; the server going
    /// away is said as such, with its name.
    #[test]
    fn a_failing_tool_and_a_failing_server_are_different_errors() {
        let deps = ToolDeps { mcp: deps(Arc::new(Scripted::default())), ..ToolDeps::default() };
        assert!(matches!(mcp(&call("mcp__gh__fails"), &deps), Err(ToolError::McpToolFailed(t)) if t == "no such issue"));
        let err = mcp(&call("mcp__gh__dies"), &deps).unwrap_err();
        assert!(matches!(&err, ToolError::McpUnavailable(m) if m.contains("\"gh\"") && m.contains("exited")), "{err}");
    }

    #[test]
    fn a_name_no_server_has_is_an_unknown_tool() {
        let deps = ToolDeps { mcp: deps(Arc::new(Scripted::default())), ..ToolDeps::default() };
        assert!(matches!(mcp(&call("mcp__gh__nope"), &deps), Err(ToolError::UnknownTool(_))));
    }

    /// The stop button reaches a call that is waiting on a server.
    #[test]
    fn the_turns_stop_reaches_the_server_call() {
        let stop = || true;
        let deps = ToolDeps { mcp: deps(Arc::new(Scripted::default())), cancelled: Some(&stop), ..ToolDeps::default() };
        assert!(matches!(mcp(&call("mcp__gh__waits"), &deps), Err(ToolError::McpUnavailable(m)) if m == "cancelled"));
    }
}
