//! `httpSaveRequest` — a case kept as a new request in the user's collections
//! (`docs/45-http-client.md`, HC-10e): the saved request it starts from with the
//! model's edits, as `httpSend` takes them, under a name, beside the template or
//! in a folder of its collection. Nothing is sent and nothing of the user's is
//! changed; the case has no scripts. The card is the loop's, and always shows.

use serde_json::json;

use super::http_send::{agent_send, stand};
use crate::domain::http_client::{AgentSave, HttpError};
use crate::domain::llm::LlmToolDefinition;
use crate::domain::tools::{HttpSaveRequestArgs, ToolDeps, ToolError, ToolPreview, ToolResult};

pub(super) fn definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "httpSaveRequest".into(),
        description: "Keep a case as a new request in the user's collections, so the user can run it again from the HTTP \
            page: the saved request `from` with your edits, as httpSend takes them, under `name`. It goes in the collection \
            `from` is in — beside it, or in the folder `folder` right inside `from`'s own, made when there is none, so that \
            it signs in and runs scripts as `from` does. Nothing is sent \
            and nothing the user wrote changes. What you write is kept as written: the host stays `from`'s, `{{name}}` is \
            filled in when the request is sent. The case has no scripts. The user approves every save. You get the new \
            request's id, which httpSend takes."
            .into(),
        parameters: json!({
            "type": "object",
            "properties": {
                "from": {"type": "string", "description": "The id of the request the case starts from, from httpEnvironment"},
                "name": {"type": "string", "description": "What the user will find it by on the HTTP page"},
                "folder": {"type": "string", "description": "A folder right inside `from`'s own, by name, made when there is none; beside `from` when left out"},
                "environment": {"type": "string", "description": "Its name; may be left out when only one is open"},
                "method": {"type": "string", "enum": ["GET", "HEAD", "OPTIONS", "POST", "PUT", "PATCH", "DELETE"], "description": "`from`'s when left out"},
                "path": {"type": "string", "description": "In place of `from`'s, from one `/`"},
                "query": {"type": "string", "description": "The whole query in place of `from`'s, `a=1&b={{name}}`; empty for none"},
                "headers": {"type": "object", "additionalProperties": {"type": ["string", "null"]}, "description": "Set by name over `from`'s; null takes one away"},
                "body": {"type": "string", "description": "The whole body in place of `from`'s"},
                "auth": {"type": "string", "enum": ["none"], "description": "\"none\": no auth, `from`'s own or inherited"}
            },
            "required": ["from", "name"]
        }),
    }
}

pub fn http_save_request(args: &HttpSaveRequestArgs, deps: &ToolDeps) -> Result<ToolResult, ToolError> {
    let stands = deps.http.as_deref().ok_or(HttpError::NoneOpen)?;
    let open = stands.open()?;
    let stand = stand(&open, args.environment.as_deref())?;
    let send = agent_send(&args.as_send())?;
    let save = AgentSave { request: send.request, name: args.name.clone(), folder: args.folder.clone(), edits: send.edits };
    let saved = stands.save(&stand.id, &save)?;
    let text = format!(
        "Saved \"{}\" in {} as request {}: {} {} — httpSend {{\"request\": \"{}\"}} sends it",
        args.name.trim(),
        saved.place,
        saved.id,
        saved.method,
        saved.url,
        saved.id
    );
    Ok(ToolResult::HttpSaveRequest { text, summary: saved.place })
}

/// The approval card: the case's name and where it goes, then the edits as
/// `httpSend`'s card shows them.
pub fn preview(args: &HttpSaveRequestArgs) -> ToolPreview {
    match super::http_send::preview(&args.as_send()) {
        ToolPreview::Change { summary, notes, diffs, production, .. } => {
            let folder = args.folder.as_deref().map(str::trim).filter(|f| !f.is_empty());
            let place = folder.map_or_else(|| "your HTTP collections, beside the request".to_string(), |f| format!("your HTTP collections, folder {f}"));
            ToolPreview::Change { place, summary: format!("\"{}\" — {summary}", args.name.trim()), notes, production, diffs }
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::ai_tools::tools::http_send::tests::{opened_as, reading, unreadable, Canned};
    use crate::domain::http_client::{AgentAccess, HttpStands};
    use std::sync::Arc;

    fn deps(http: Option<Arc<dyn HttpStands>>) -> ToolDeps<'static> {
        ToolDeps { http, ..ToolDeps::default() }
    }

    fn args(from: &str, name: &str) -> HttpSaveRequestArgs {
        HttpSaveRequestArgs { from: from.into(), name: name.into(), ..HttpSaveRequestArgs::default() }
    }

    #[test]
    fn a_case_is_saved_in_the_environment_named_or_the_only_one() {
        let stands = Arc::new(Canned::opened(&["Stage"]));
        let call = HttpSaveRequestArgs { folder: Some("Cases".into()), path: Some("/orders/2".into()), auth: Some("none".into()), ..args(" r1 ", " No auth ") };
        let ToolResult::HttpSaveRequest { text, summary } = http_save_request(&call, &deps(Some(stands.clone()))).unwrap() else { panic!() };
        assert_eq!(summary, "Shop / Cases");
        assert_eq!(text, "Saved \"No auth\" in Shop / Cases as request n1: GET {{baseUrl}}/orders/2 — httpSend {\"request\": \"n1\"} sends it");
        assert_eq!(stands.asked(), ["save id-Stage r1 as \" No auth \" in Some(\"Cases\")"]);
        let saves = stands.saves();
        assert_eq!((saves[0].edits.path.as_deref(), saves[0].edits.no_auth), (Some("/orders/2"), true), "the edits as httpSend reads them");
    }

    #[test]
    fn what_cannot_be_saved_says_why_and_saves_nothing() {
        let none = http_save_request(&args("r1", "x"), &deps(None)).unwrap_err().to_string();
        assert!(none.contains("no environment is open"), "{none}");
        let two = http_save_request(&args("r1", "x"), &deps(Some(reading(&["Stage", "Prod"])))).unwrap_err().to_string();
        assert!(two.contains("say which `environment`"), "{two}");
        assert!(http_save_request(&args("r1", "x"), &deps(Some(unreadable()))).is_err());
        let stands = Arc::new(Canned::opened(&["Stage"]));
        let bad = HttpSaveRequestArgs { auth: Some("basic".into()), ..args("r1", "x") };
        assert!(matches!(http_save_request(&bad, &deps(Some(stands.clone()))), Err(ToolError::Http(HttpError::BadAuth(_)))));
        assert!(stands.asked().is_empty(), "refused before the stands");
        let missing = http_save_request(&args("zz", "x"), &deps(Some(opened_as(AgentAccess::ReadOnly)))).unwrap_err().to_string();
        assert!(missing.contains("zz"), "{missing}");
    }

    #[test]
    fn the_card_names_the_case_and_where_it_goes() {
        let call = HttpSaveRequestArgs { folder: Some(" Cases ".into()), method: Some("post".into()), body: Some("{}".into()), ..args("r1", " No auth ") };
        let ToolPreview::Change { place, summary, diffs, .. } = preview(&call) else { panic!() };
        assert_eq!(place, "your HTTP collections, folder Cases");
        assert_eq!(summary, "\"No auth\" — request r1: POST");
        assert_eq!(diffs.len(), 1, "the body");
        let ToolPreview::Change { place, summary, .. } = preview(&args("r1", "Same")) else { panic!() };
        assert_eq!((place.as_str(), summary.as_str()), ("your HTTP collections, beside the request", "\"Same\" — request r1 as saved"));
    }
}
