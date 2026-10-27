//! `toolSearch` — the way to the tools that are not declared in every
//! request: an MCP server's (`Exposure::Deferred`), and the app's own that do
//! not `loads_up_front`. What it finds is declared from the next round on;
//! `domain::mcp::loaded_tools` reads that back off the history.

use crate::domain::llm::LlmToolDefinition;
use crate::domain::mcp::match_score;
use crate::domain::tools::{FoundTool, ToolDeps, ToolError, ToolResult, ToolSearchArgs};

use super::semantic_search::shorten;

pub const DEFAULT_LIMIT: u32 = 5;
pub const MAX_LIMIT: u32 = 10;
/// A description is for choosing a tool, not for using it — the schema
/// arrives with the declaration.
const DESCRIPTION_CHARS: usize = 300;
/// How a found tool of the app's own is labelled where a server's name goes.
const BUILT_IN: &str = "built-in";
/// What a waiting tool is said to do in `toolSearch`'s own description.
const SUMMARY_CHARS: usize = 90;

pub fn tool_search(args: &ToolSearchArgs, deps: &ToolDeps) -> Result<ToolResult, ToolError> {
    if args.query.trim().is_empty() {
        return Err(ToolError::InvalidArguments {
            tool: "toolSearch".into(),
            reason: "`query` is empty — say what the tool should do, or name it".into(),
        });
    }
    let limit = args.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT) as usize;
    let own = deps.waiting.iter().map(|definition| {
        let found = FoundTool {
            name: definition.name.clone(),
            server: BUILT_IN.to_string(),
            description: shorten(&definition.description, DESCRIPTION_CHARS),
        };
        (match_score(&args.query, &[&definition.name], &definition.description), found)
    });
    let servers = deps.mcp.scored(&args.query).into_iter().map(|(score, entry)| {
        let found = FoundTool {
            name: entry.wire_name.clone(),
            server: entry.server.clone(),
            description: shorten(&entry.tool.description, DESCRIPTION_CHARS),
        };
        (score, found)
    });
    let mut scored: Vec<(usize, FoundTool)> = own.filter(|(score, _)| *score > 0).chain(servers).collect();
    // Stable: on a tie, the app's own before a server's, each in its order.
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(ToolResult::ToolsFound { tools: scored.into_iter().take(limit).map(|(_, found)| found).collect() })
}

/// The definition a turn sends: which of the app's own tools wait here, by
/// name and what each is for, and whether servers' tools do too — what the
/// model needs to know a search would find something.
pub fn definition_for(waiting: &[LlmToolDefinition], servers: bool) -> LlmToolDefinition {
    let mut definition = definition();
    if !waiting.is_empty() {
        let listed: Vec<String> = waiting
            .iter()
            .map(|tool| format!("`{}` — {}", tool.name, shorten(first_sentence(&tool.description), SUMMARY_CHARS)))
            .collect();
        definition.description.push_str(&format!(" Of this app's own tools, these wait here: {}.", listed.join("; ")));
    }
    if !servers {
        definition.description = definition.description.replace(SERVERS_CLAUSE, "");
    }
    definition
}

fn first_sentence(text: &str) -> &str {
    text.split_once(". ").map_or(text, |(first, _)| first).trim_end_matches('.')
}

/// What the description says of MCP servers — left out where none has tools waiting.
const SERVERS_CLAUSE: &str = " — and the MCP servers' that the prompt's \"MCP servers\" section says are found here";

pub(super) fn definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "toolSearch".to_string(),
        description: format!(
            "Find tools that are not declared to you yet{SERVERS_CLAUSE}. Searches the tools' names and descriptions by \
words: name the action and the thing it acts on (\"create issue\", \"move file\"), or give a tool's exact name. The tools \
found are declared to you from your next step on, for the rest of the conversation — call them then as ordinary tools. \
Returns at most {MAX_LIMIT}."
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

    fn own(name: &str, description: &str) -> LlmToolDefinition {
        LlmToolDefinition { name: name.into(), description: description.into(), parameters: serde_json::json!({}) }
    }

    /// The app's own waiting tools are found as a server's are, labelled
    /// built-in, and ranked with them by the same score.
    #[test]
    fn the_apps_own_tools_are_found_beside_the_servers() {
        let deps = ToolDeps {
            mcp: deps(2),
            waiting: vec![own("move", "Move or rename a file. More."), own("gitBlame", "Who last changed each line.")],
            ..Default::default()
        };
        let found = |query: &str| match tool_search(&ToolSearchArgs { query: query.into(), limit: None }, &deps).unwrap() {
            ToolResult::ToolsFound { tools } => tools,
            other => panic!("{other:?}"),
        };
        let moved = found("move file");
        assert_eq!((moved[0].name.as_str(), moved[0].server.as_str()), ("move", BUILT_IN));
        assert_eq!(moved.len(), 1, "nothing else says move: {moved:?}");
        let names: Vec<String> = found("get_1 gitBlame").into_iter().map(|t| t.name).collect();
        assert_eq!(names[..2], ["mcp__gh__get_1".to_string(), "gitBlame".to_string()], "{names:?}");
    }

    /// The definition names what waits, so the model knows a search finds it,
    /// and speaks of servers only when one has tools waiting.
    #[test]
    fn the_definition_says_what_waits_here() {
        let waiting = [own("move", "Move or rename a file. Fails if the target exists.")];
        let plain = definition_for(&waiting, false).description;
        assert!(plain.contains("these wait here: `move` — Move or rename a file."), "{plain}");
        assert!(!plain.contains("Fails if"), "only the first sentence: {plain}");
        assert!(!plain.contains("MCP"), "{plain}");
        assert!(definition_for(&[], true).description.contains("MCP servers"));
    }

    #[test]
    fn an_empty_query_is_the_models_to_fix() {
        assert!(matches!(search("  ", None, deps(1)), Err(ToolError::InvalidArguments { .. })));
    }
}
