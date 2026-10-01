//! `toolSearch` — the way to an MCP server's tools that are not declared in
//! every request (`Exposure::Deferred`). What it finds is declared from the
//! next round on; `domain::mcp::loaded_tools` reads that back off the history.

use crate::domain::llm::LlmToolDefinition;
use crate::domain::tools::{FoundTool, ToolDeps, ToolError, ToolResult, ToolSearchArgs};

use super::semantic_search::shorten;

pub const DEFAULT_LIMIT: u32 = 5;
pub const MAX_LIMIT: u32 = 10;
/// A description is for choosing a tool, not for using it — the schema
/// arrives with the declaration.
const DESCRIPTION_CHARS: usize = 300;

pub fn tool_search(args: &ToolSearchArgs, deps: &ToolDeps) -> Result<ToolResult, ToolError> {
    if args.query.trim().is_empty() {
        return Err(ToolError::InvalidArguments {
            tool: "toolSearch".into(),
            reason: "`query` is empty — say what the tool should do, or name it".into(),
        });
    }
    let limit = args.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT) as usize;
    let tools = deps
        .mcp
        .search(&args.query, limit)
        .into_iter()
        .map(|entry| FoundTool {
            name: entry.wire_name.clone(),
            server: entry.server.clone(),
            description: shorten(&entry.tool.description, DESCRIPTION_CHARS),
        })
        .collect();
    Ok(ToolResult::ToolsFound { tools })
}

pub(super) fn definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "toolSearch".to_string(),
        description: format!(
            "Find tools of the MCP servers that are not declared to you yet — the prompt's \"MCP servers\" section says \
which servers have them and how many. Searches the tools' names and descriptions by words: name the action and the \
thing it acts on (\"create issue\", \"query table\"), or give a tool's exact name. The tools found are declared to you \
from your next step on, for the rest of the conversation — call them then as ordinary tools. Returns at most {MAX_LIMIT}."
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Words the tool's name or description would contain, or its exact name."
                },
                "limit": {
                    "type": ["integer", "null"],
                    "description": format!("How many tools, 1 to {MAX_LIMIT}; {DEFAULT_LIMIT} when absent.")
                }
            },
            "required": ["query"],
            "additionalProperties": false
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::mcp::{ConnectedServer, Exposure, McpCallResult, McpClient, McpError, McpServerConfig, McpTool, McpTools};
    use std::sync::Arc;

    struct Idle;
    impl McpClient for Idle {
        fn list_tools(&self) -> Result<Vec<McpTool>, McpError> {
            Ok(vec![])
        }
        fn call_tool(&self, _: &str, _: serde_json::Value, _: &dyn Fn() -> bool) -> Result<McpCallResult, McpError> {
            Err(McpError::Cancelled)
        }
    }

    fn deps(count: usize) -> McpTools {
        let tool = |i: usize| McpTool { name: format!("get_{i}"), description: "x".repeat(1000), ..Default::default() };
        McpTools::new(vec![ConnectedServer {
            name: "gh".into(),
            config: McpServerConfig { exposure: Some(Exposure::Deferred), ..Default::default() },
            client: Arc::new(Idle),
            tools: (0..count).map(tool).collect(),
            instructions: None,
        }])
    }

    fn search(query: &str, limit: Option<u32>, mcp: McpTools) -> Result<Vec<FoundTool>, ToolError> {
        let deps = ToolDeps { mcp, ..Default::default() };
        match tool_search(&ToolSearchArgs { query: query.into(), limit }, &deps)? {
            ToolResult::ToolsFound { tools } => Ok(tools),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn found_tools_come_with_their_server_and_a_short_description() {
        let found = search("get_3", None, deps(4)).unwrap();
        assert_eq!((found[0].name.as_str(), found[0].server.as_str()), ("mcp__gh__get_3", "gh"));
        assert!(found[0].description.chars().count() <= DESCRIPTION_CHARS + 1, "{}", found[0].description.len());
    }

    #[test]
    fn the_limit_has_a_default_and_a_ceiling() {
        assert_eq!(search("get", None, deps(20)).unwrap().len(), DEFAULT_LIMIT as usize);
        assert_eq!(search("get", Some(50), deps(20)).unwrap().len(), MAX_LIMIT as usize);
        assert_eq!(search("get", Some(0), deps(20)).unwrap().len(), 1);
    }

    #[test]
    fn an_empty_query_is_the_models_to_fix() {
        assert!(matches!(search("  ", None, deps(1)), Err(ToolError::InvalidArguments { .. })));
    }
}
