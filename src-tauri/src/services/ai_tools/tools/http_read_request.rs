//! `httpReadRequest` — a request saved in the user's collections, read as written
//! (`docs/45-http-client.md`, HC-10b): its address, headers, body, auth — its own
//! or the level it inherits — the variables it uses and its scripts. Nothing is
//! sent. A credential written out in the collection reaches the model masked, and
//! a script holding a hidden value's text is not shown.

use serde_json::json;

use super::http_request::stand;
use super::mcp::fit;
use crate::domain::http_client::HttpError;
use crate::domain::llm::LlmToolDefinition;
use crate::domain::tools::{HttpReadRequestArgs, ToolDeps, ToolError, ToolResult};

/// A request with long scripts is still one read.
const MOST: usize = 12000;

pub(super) fn definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "httpReadRequest".into(),
        description: "Read a request saved in the user's collections, as the user wrote it: its method, address, \
            parameters, headers, body, its auth and which folder or collection that comes from, the variables it uses, \
            and the scripts that run with it — where a token comes from, say. Nothing is sent. Its id is in \
            httpRequest's description of the environment (called with no `url`). A credential written out in the \
            collection is masked, and a script that holds a hidden value is not shown."
            .into(),
        parameters: json!({
            "type": "object",
            "properties": {
                "request": {"type": "string", "description": "Its id, from the environment's list of requests"},
                "environment": {"type": "string", "description": "Its name; may be left out when only one is open"}
            },
            "required": ["request"]
        }),
    }
}

pub fn http_read_request(args: &HttpReadRequestArgs, deps: &ToolDeps) -> Result<ToolResult, ToolError> {
    let stands = deps.http.as_deref().ok_or(HttpError::NoneOpen)?;
    let open = stands.open()?;
    let stand = stand(&open, args.environment.as_deref())?;
    let text = stands.read(&stand.id, args.request.trim())?;
    // The first line names it: `Request "Shop / Orders / Get one", id …` — a name may hold a quote.
    let first = text.lines().next().unwrap_or_default();
    let summary = first.strip_prefix("Request \"").and_then(|rest| rest.rsplit_once("\", id ")).map_or(args.request.trim(), |(path, _)| path).to_string();
    Ok(ToolResult::HttpReadRequest { text: fit(text, MOST), summary })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::ai_tools::tools::http_request::tests::{opened_as, reading};
    use crate::domain::http_client::AgentAccess;

    fn deps(http: Option<std::sync::Arc<dyn crate::domain::http_client::HttpStands>>) -> ToolDeps<'static> {
        ToolDeps { http, ..ToolDeps::default() }
    }

    fn args(request: &str, environment: Option<&str>) -> HttpReadRequestArgs {
        HttpReadRequestArgs { request: request.into(), environment: environment.map(String::from) }
    }

    #[test]
    fn the_request_is_read_in_the_environment_named_or_the_only_one() {
        let stands = reading(&["Stage"]);
        let ToolResult::HttpReadRequest { text, summary } = http_read_request(&args(" r1 ", None), &deps(Some(stands.clone()))).unwrap() else { panic!() };
        assert_eq!(summary, "Shop / Get one");
        assert!(text.contains("id r1"), "{text}");
        assert!(http_read_request(&args("r1", Some("stage")), &deps(Some(stands.clone()))).is_ok(), "any case");
        let ToolResult::HttpReadRequest { summary, .. } = http_read_request(&args("q1", None), &deps(Some(stands))).unwrap() else { panic!() };
        assert_eq!(summary, "Shop / Say \"hi\"", "a quote in a name stays");
    }

    #[test]
    fn with_none_open_or_two_unnamed_it_says_so() {
        let none = http_read_request(&args("r1", None), &deps(None)).unwrap_err().to_string();
        assert!(none.contains("no environment is open"), "{none}");
        let two = http_read_request(&args("r1", None), &deps(Some(reading(&["Stage", "Prod"])))).unwrap_err().to_string();
        assert!(two.contains("say which `environment`"), "{two}");
        let other = http_read_request(&args("r1", Some("Dev")), &deps(Some(opened_as(AgentAccess::ReadOnly)))).unwrap_err().to_string();
        assert!(other.contains("no environment \"Dev\""), "{other}");
    }
}
