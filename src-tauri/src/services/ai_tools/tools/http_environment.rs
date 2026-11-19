//! `httpEnvironment` — an HTTP environment the user opened to the agent, described
//! (`docs/45-http-client.md`, HC-10c): its access, the hosts its requests go to, its
//! variables — a hidden one by name only — and the collections' requests with their
//! ids, for `httpReadRequest` and `httpSend`. Nothing is sent.

use serde_json::json;

use super::http_send::stand;
use super::mcp::fit;
use crate::domain::http_client::{AgentAccess, HttpError, StandView};
use crate::domain::llm::LlmToolDefinition;
use crate::domain::tools::{HttpEnvironmentArgs, ToolDeps, ToolError, ToolResult};

/// The most of a description the model gets at once; the rest waits in a file.
const MOST: usize = 8000;

pub(super) fn definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "httpEnvironment".into(),
        description: "Describe an HTTP environment the user opened to the agent on the HTTP page: what it lets you do, the \
            hosts its requests go to, its variables — a hidden one by name only — and the requests in the user's collections, \
            each with its id. Call it first: httpReadRequest reads a request by its id, httpSend sends one."
            .into(),
        parameters: json!({
            "type": "object",
            "properties": {
                "environment": {"type": "string", "description": "Its name; may be left out when only one is open"}
            },
            "required": []
        }),
    }
}

pub fn http_environment(args: &HttpEnvironmentArgs, deps: &ToolDeps) -> Result<ToolResult, ToolError> {
    let stands = deps.http.as_deref().ok_or(HttpError::NoneOpen)?;
    let open = stands.open()?;
    let stand = stand(&open, args.environment.as_deref())?;
    let view = stands.describe(&stand.id)?;
    let summary = format!("{} · {} {}", view.name, view.requests.len(), if view.requests.len() == 1 { "request" } else { "requests" });
    Ok(ToolResult::HttpEnvironment { text: fit(described(&view), MOST), summary })
}

/// The environment as the model reads it before sending.
fn described(view: &StandView) -> String {
    let access = match view.access {
        AgentAccess::Off => "off",
        AgentAccess::ReadOnly => "read only: GET, HEAD and OPTIONS",
        AgentAccess::AskForWrites => "open to writes, each one after the user approves it",
        AgentAccess::WritesAsOtherTools => "open to writes, approved as the user approves other changes",
    };
    let mut lines = vec![format!("Environment \"{}\", {access}.", view.name)];
    if view.origins.is_empty() {
        lines.push("It sends nowhere yet: no request in the user's collections goes anywhere with it. Ask the user to add one on the HTTP page.".into());
    } else {
        lines.push("Its requests go to:".into());
        lines.extend(view.origins.iter().map(|o| format!("  {o}")));
    }
    if !view.variables.is_empty() {
        lines.push("Variables, written as {{name}}:".into());
        lines.extend(view.variables.iter().map(|(name, value)| match value {
            Some(value) => format!("  {name} = {value}"),
            None => format!("  {name} — hidden: filled in on sending, never shown"),
        }));
    }
    if !view.requests.is_empty() {
        lines.push("Requests in the user's collections:".into());
        lines.extend(view.requests.iter().map(|r| format!("  {r}")));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::tools::{ToolCall, ToolName};
    use crate::services::ai_tools::tools::http_send::tests::{described_as, Canned};

    fn view(origins: &[&str], requests: &[&str]) -> StandView {
        StandView {
            name: "Stage".into(),
            access: AgentAccess::ReadOnly,
            origins: origins.iter().map(|o| o.to_string()).collect(),
            variables: vec![("baseUrl".into(), Some("https://api.test".into())), ("token".into(), None)],
            requests: requests.iter().map(|r| r.to_string()).collect(),
        }
    }

    fn call(canned: std::sync::Arc<Canned>, args: &HttpEnvironmentArgs) -> Result<ToolResult, ToolError> {
        http_environment(args, &ToolDeps { http: Some(canned), ..ToolDeps::default() })
    }

    fn text_of(result: Result<ToolResult, ToolError>) -> String {
        match result {
            Ok(ToolResult::HttpEnvironment { text, .. }) => text,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn it_describes_the_environment_and_sends_nothing() {
        let canned = described_as(view(&["https://api.test"], &["Shop / List orders: GET {{baseUrl}}/orders (id r1)"]));
        let result = call(canned.clone(), &HttpEnvironmentArgs::default());
        assert!(matches!(&result, Ok(ToolResult::HttpEnvironment { summary, .. }) if summary == "Stage · 1 request"), "{result:?}");
        let expected = "Environment \"Stage\", read only: GET, HEAD and OPTIONS.\n\
            Its requests go to:\n  https://api.test\n\
            Variables, written as {{name}}:\n  baseUrl = https://api.test\n  token — hidden: filled in on sending, never shown\n\
            Requests in the user's collections:\n  Shop / List orders: GET {{baseUrl}}/orders (id r1)";
        assert_eq!(text_of(result), expected);
        assert_eq!(canned.asked(), ["describe id-Stage"], "nothing sent");
        assert!(!ToolName::HttpEnvironment.is_mutating());
        assert!(!ToolCall::HttpEnvironment(HttpEnvironmentArgs::default()).is_risky());
    }

    /// The model learns before sending whether a write will be refused, or asked about.
    #[test]
    fn the_description_says_what_a_write_meets() {
        for (access, said) in [
            (AgentAccess::AskForWrites, "open to writes, each one after the user approves it"),
            (AgentAccess::WritesAsOtherTools, "open to writes, approved as the user approves other changes"),
        ] {
            let canned = described_as(StandView { access, ..view(&[], &[]) });
            assert!(text_of(call(canned, &HttpEnvironmentArgs::default())).starts_with(&format!("Environment \"Stage\", {said}.")), "{access:?}");
        }
    }

    #[test]
    fn an_environment_with_nowhere_to_go_says_who_can_change_that() {
        let result = call(described_as(view(&[], &[])), &HttpEnvironmentArgs::default());
        assert!(matches!(&result, Ok(ToolResult::HttpEnvironment { summary, .. }) if summary == "Stage · 0 requests"), "{result:?}");
        let text = text_of(result);
        assert!(text.contains("It sends nowhere yet") && text.contains("add one on the HTTP page"), "{text}");
        assert!(!text.contains("Requests in"), "{text}");
    }

    #[test]
    fn nothing_open_says_where_to_open_it() {
        let told = "no environment is open to the agent — set its agent access on the HTTP page";
        assert_eq!(http_environment(&HttpEnvironmentArgs::default(), &ToolDeps::default()).unwrap_err().to_string(), told, "the feature off");
    }
}
