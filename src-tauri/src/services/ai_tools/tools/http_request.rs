//! `httpRequest` — the HTTP environments the user opened to the agent
//! (`docs/45-http-client.md`, HC-7, HC-7a, HC-8). The model sends where a request
//! of the user's collections goes, and `{{name}}` is filled in by the environment:
//! a hidden variable's value never reaches the model. A write goes as the
//! environment's mode says — the card is the loop's (`ApprovalPolicy::stand_asks`).
//! Called without a `url` it describes the environment. A stand that does not
//! answer is a result, told as an error.

use serde_json::json;

use super::mcp::fit;
use crate::domain::http_client::{AgentAccess, AgentRequest, HttpError, OpenStand, Outcome, StandView};
use crate::domain::llm::LlmToolDefinition;
use crate::domain::tools::{ChangeDiff, HttpRequestArgs, ToolDeps, ToolError, ToolPreview, ToolResult};
use crate::services::text_diff::diff_stats;

/// The most of a response the model gets at once; the rest waits in a file.
const MOST: usize = 8000;
/// What a response says that is worth its tokens; `showHeaders` asks for the rest.
const SHOWN: &[&str] = &["content-type", "location", "retry-after", "x-request-id", "x-trace-id", "trace-id"];

pub(super) fn definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "httpRequest".into(),
        description: "Send a request into an HTTP environment the user opened to the agent on the HTTP page. GET, HEAD \
            and OPTIONS always go; another method goes as the environment allows — refused when it is read only, \
            otherwise after the user approves, when its mode asks. Call it first with no `url`: you get the environment's variables, the addresses you may send to, and the requests \
            in the user's collections. A `url` must go to one of those addresses — write it as the collections do, \
            `{{baseUrl}}/orders/42?expand=items`. Another host is refused: ask the user to add a request for it on the HTTP \
            page, or give them a curl command to run themselves. `{{name}}` in the url, a header or the body is filled in from the \
            environment; a hidden variable's value never reaches you: write `Authorization: Bearer {{token}}`. \
            A redirect is not followed: you get its status and `location`. No cookies are sent or kept."
            .into(),
        parameters: json!({
            "type": "object",
            "properties": {
                "environment": {"type": "string", "description": "Its name; may be left out when only one is open"},
                "url": {"type": "string", "description": "Where to send, `{{name}}` filled in; leave out to see what the environment holds"},
                "method": {"type": "string", "enum": ["GET", "HEAD", "OPTIONS", "POST", "PUT", "PATCH", "DELETE"], "description": "GET when left out"},
                "headers": {"type": "object", "additionalProperties": {"type": "string"}, "description": "Sent as written, `{{name}}` filled in"},
                "body": {"type": "string", "description": "Sent as written, `{{name}}` filled in; JSON goes as application/json unless a Content-Type says otherwise"},
                "showHeaders": {"type": "boolean", "description": "All the response's headers, not the few that usually matter"}
            },
            "required": []
        }),
    }
}

pub fn http_request(args: &HttpRequestArgs, deps: &ToolDeps) -> Result<ToolResult, ToolError> {
    let stands = deps.http.as_deref().ok_or(HttpError::NoneOpen)?;
    let open = stands.open()?;
    let stand = stand(&open, args.environment.as_deref())?;
    let Some(url) = args.url.as_deref().filter(|u| !u.trim().is_empty()) else {
        let view = stands.describe(&stand.id)?;
        let summary = format!("{} · {} {}", view.name, view.origins.len(), if view.origins.len() == 1 { "address" } else { "addresses" });
        return Ok(ToolResult::HttpRequest { text: fit(described(&view), MOST), summary });
    };
    let method = args.method();
    let headers = args.headers.iter().flatten().map(|(k, v)| (k.clone(), v.clone())).collect();
    let request = AgentRequest { method: method.clone(), url: url.to_string(), headers, body: args.body.clone() };
    let reply = stands.send(&stand.id, &request)?;
    let (status, headers, elapsed_ms) = match reply.outcome {
        Outcome::Response { status, headers, elapsed_ms, .. } => (status, headers, elapsed_ms),
        Outcome::Failed { error } => return Err(HttpError::Unanswered(error).into()),
    };
    let first = format!("{method} {url} → {status} · {elapsed_ms} ms · {} bytes", reply.size);
    let lines = headers.iter().filter(|(k, _)| args.show_headers == Some(true) || SHOWN.contains(&k.to_ascii_lowercase().as_str())).map(|(k, v)| format!("{k}: {v}"));
    let text = std::iter::once(first).chain(lines).chain(body_text(&reply.body)).collect::<Vec<_>>().join("\n");
    Ok(ToolResult::HttpRequest { text: fit(text, MOST), summary: format!("{status} · {elapsed_ms} ms") })
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
        lines.push("It sends to these addresses, where the user's collections go:".into());
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

/// The approval card's account of a call: where, what, its headers and body —
/// as the model wrote them, `{{token}}` and not its value. Built from the call
/// alone, so it is the same in Agent and in Chat.
pub fn preview(args: &HttpRequestArgs) -> ToolPreview {
    let place = args.environment.as_deref().map(str::trim).filter(|e| !e.is_empty()).map_or_else(|| "the environment open to the agent".to_string(), |e| format!("environment {e}"));
    let notes = args.headers.iter().flatten().map(|(k, v)| format!("{k}: {v}")).collect();
    let diffs = args.body.iter().map(|body| ChangeDiff { title: "Body".into(), diff: diff_stats("", body) }).collect();
    ToolPreview::Change { place, summary: format!("{} {}", args.method(), args.url.as_deref().unwrap_or_default()), notes, production: false, diffs }
}

/// The environment by its name, any case; with none named, the only one open.
pub(crate) fn stand<'a>(open: &'a [OpenStand], name: Option<&str>) -> Result<&'a OpenStand, HttpError> {
    let names = || open.iter().map(|s| format!("\"{}\"", s.name)).collect::<Vec<_>>().join(", ");
    let Some(name) = name.map(str::trim).filter(|n| !n.is_empty()) else {
        return match open {
            [] => Err(HttpError::NoneOpen),
            [only] => Ok(only),
            _ => Err(HttpError::WhichEnvironment(names())),
        };
    };
    if open.is_empty() {
        return Err(HttpError::NoneOpen);
    }
    let named: Vec<&OpenStand> = open.iter().filter(|s| s.name.to_lowercase() == name.to_lowercase()).collect();
    match named[..] {
        [one] => Ok(one),
        [] => Err(HttpError::NotOpen { name: name.into(), open: names() }),
        _ => Err(HttpError::SameName { name: name.into(), count: named.len() }),
    }
}

/// The text, and of bytes that are not text only that they are not.
fn body_text(body: &[u8]) -> Option<String> {
    if body.is_empty() {
        return None;
    }
    Some(std::str::from_utf8(body).unwrap_or("[not text]").to_string())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::domain::http_client::{AgentReply, HttpStands};
    use crate::domain::tools::{ToolCall, ToolName};
    use std::sync::{Arc, Mutex};

    /// Its environments, one canned reply, and what it was asked.
    #[derive(Default)]
    struct Canned {
        open: Vec<OpenStand>,
        reply: Option<AgentReply>,
        view: Option<StandView>,
        asked: Mutex<Vec<(String, AgentRequest)>>,
        /// The environments' file could not be read.
        unreadable: bool,
    }

    impl HttpStands for Canned {
        fn open(&self) -> Result<Vec<OpenStand>, HttpError> {
            if self.unreadable {
                return Err(HttpError::NoneOpen);
            }
            Ok(self.open.clone())
        }

        fn describe(&self, id: &str) -> Result<StandView, HttpError> {
            self.asked.lock().unwrap().push((id.to_string(), AgentRequest { method: "describe".into(), url: String::new(), headers: vec![], body: None }));
            Ok(self.view.clone().unwrap())
        }

        fn send(&self, id: &str, request: &AgentRequest) -> Result<AgentReply, HttpError> {
            self.asked.lock().unwrap().push((id.to_string(), request.clone()));
            Ok(self.reply.clone().unwrap())
        }
    }

    /// One environment that answers 200, for what is about the way there.
    pub(crate) fn answering() -> Arc<dyn HttpStands> {
        opened_as(AgentAccess::ReadOnly)
    }

    /// Environments that cannot be read.
    pub(crate) fn unreadable() -> Arc<dyn HttpStands> {
        Arc::new(Canned { unreadable: true, ..Canned::default() })
    }

    /// The same, open to the agent as `access` says.
    pub(crate) fn opened_as(access: AgentAccess) -> Arc<dyn HttpStands> {
        let open = stands(&["Stage"]).into_iter().map(|s| OpenStand { access, ..s }).collect();
        Arc::new(Canned { open, reply: reply(200, &[], b"ok"), ..Canned::default() })
    }

    fn stands(names: &[&str]) -> Vec<OpenStand> {
        names.iter().map(|n| OpenStand { id: format!("id-{n}"), name: n.to_string(), access: AgentAccess::ReadOnly }).collect()
    }

    fn reply(status: u16, headers: &[(&str, &str)], body: &[u8]) -> Option<AgentReply> {
        let headers = headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        Some(AgentReply { outcome: Outcome::Response { status, headers, url: "http://stand/x".into(), redirects: vec![], elapsed_ms: 12 }, body: body.to_vec(), size: body.len() })
    }

    fn get(url: &str) -> HttpRequestArgs {
        HttpRequestArgs { method: Some("get".into()), url: Some(url.into()), ..HttpRequestArgs::default() }
    }

    fn call(canned: Canned, args: &HttpRequestArgs) -> (Result<ToolResult, ToolError>, Vec<(String, AgentRequest)>) {
        let canned = Arc::new(canned);
        let deps = ToolDeps { http: Some(canned.clone()), ..ToolDeps::default() };
        let result = http_request(args, &deps);
        let asked = canned.asked.lock().unwrap().clone();
        (result, asked)
    }

    fn text_of(result: Result<ToolResult, ToolError>) -> String {
        match result {
            Ok(ToolResult::HttpRequest { text, .. }) => text,
            other => panic!("{other:?}"),
        }
    }

    fn error_of(result: Result<ToolResult, ToolError>) -> String {
        result.unwrap_err().to_string()
    }

    #[test]
    fn nothing_open_says_where_to_open_it() {
        let told = "no environment is open to the agent — set its agent access on the HTTP page";
        assert_eq!(http_request(&get("/x"), &ToolDeps::default()).unwrap_err().to_string(), told, "the feature off");
        assert_eq!(error_of(call(Canned::default(), &get("/x")).0), told);
        let named = HttpRequestArgs { environment: Some("Stage".into()), ..get("/x") };
        assert_eq!(error_of(call(Canned::default(), &named).0), told, "no name is confirmed or denied");
    }

    #[test]
    fn the_only_one_open_needs_no_name_and_several_do() {
        let (result, asked) = call(Canned { open: stands(&["Stage"]), reply: reply(200, &[], b""), ..Canned::default() }, &get("/x"));
        assert!(result.is_ok());
        assert_eq!(asked[0].0, "id-Stage");
        let (result, asked) = call(Canned { open: stands(&["Stage", "Local"]), ..Canned::default() }, &get("/x"));
        assert_eq!(error_of(result), "say which `environment`: \"Stage\", \"Local\"");
        assert!(asked.is_empty());
    }

    #[test]
    fn a_name_is_found_in_any_case_and_one_not_open_names_only_those_that_are() {
        let by_name = |name: &str| HttpRequestArgs { environment: Some(name.into()), ..get("/x") };
        let (_, asked) = call(Canned { open: stands(&["Stage", "Local"]), reply: reply(200, &[], b""), ..Canned::default() }, &by_name(" local "));
        assert_eq!(asked[0].0, "id-Local");
        let (result, asked) = call(Canned { open: stands(&["Stage", "Local"]), ..Canned::default() }, &by_name("Prod"));
        assert_eq!(error_of(result), "no environment \"Prod\" is open to the agent: \"Stage\", \"Local\"");
        assert!(asked.is_empty());
        let (result, _) = call(Canned { open: stands(&["Stage", "stage"]), ..Canned::default() }, &by_name("STAGE"));
        assert_eq!(error_of(result), "2 environments are named \"STAGE\" — rename one on the HTTP page");
    }

    #[test]
    fn an_empty_name_is_no_name() {
        let args = HttpRequestArgs { environment: Some(" ".into()), ..get("/x") };
        let (result, asked) = call(Canned { open: stands(&["Stage"]), reply: reply(200, &[], b""), ..Canned::default() }, &args);
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(asked[0].0, "id-Stage");
    }

    /// The tool writes, so Analyst never gets it; a call is risky only when its method is
    /// not a read — no method at all is a GET.
    #[test]
    fn a_read_is_not_risky_and_a_write_is() {
        assert!(ToolName::HttpRequest.is_mutating());
        let risky = |method: Option<&str>| ToolCall::HttpRequest(HttpRequestArgs { method: method.map(Into::into), ..get("/x") }).is_risky();
        for read in [None, Some("GET"), Some(" head "), Some("options"), Some("")] {
            assert!(!risky(read), "{read:?}");
        }
        for write in ["POST", "put", "PATCH", "DELETE"] {
            assert!(risky(Some(write)), "{write}");
        }
    }

    /// The card says where, what, the headers and the body as the model wrote them —
    /// `{{token}}`, never its value, which the call does not hold.
    #[test]
    fn the_card_shows_the_call_as_written() {
        let headers = Some([("Authorization".to_string(), "Bearer {{token}}".to_string())].into());
        let args = HttpRequestArgs { environment: Some(" Stage ".into()), method: Some("post".into()), headers, body: Some("{\"a\": 1}".into()), ..get("{{baseUrl}}/orders") };
        let ToolPreview::Change { place, summary, notes, production, diffs } = preview(&args) else { panic!("a change") };
        assert_eq!((place.as_str(), summary.as_str(), production), ("environment Stage", "POST {{baseUrl}}/orders", false));
        assert_eq!(notes, ["Authorization: Bearer {{token}}"]);
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].title, "Body");
        assert!(diffs[0].diff.unified_diff.contains("+{\"a\": 1}"), "{:?}", diffs[0].diff);

        let ToolPreview::Change { place, summary, notes, diffs, .. } = preview(&HttpRequestArgs { method: None, ..get("/x") }) else { panic!("a change") };
        assert_eq!((place.as_str(), summary.as_str()), ("the environment open to the agent", "GET /x"));
        assert!(notes.is_empty() && diffs.is_empty());
    }

    #[test]
    fn a_body_reaches_the_stand_as_written() {
        let args = HttpRequestArgs { method: Some("put".into()), body: Some("{\"id\": {{id}}}".into()), ..get("/orders/1") };
        let (_, asked) = call(Canned { open: stands(&["Stage"]), reply: reply(200, &[], b""), ..Canned::default() }, &args);
        assert_eq!((asked[0].1.method.as_str(), asked[0].1.body.as_deref()), ("PUT", Some("{\"id\": {{id}}}")));
    }

    #[test]
    fn what_was_asked_reaches_the_stand_as_asked() {
        let headers = Some([("Authorization".to_string(), "Bearer {{token}}".to_string())].into());
        let args = HttpRequestArgs { headers, ..get("/orders?id=1") };
        let (_, asked) = call(Canned { open: stands(&["Stage"]), reply: reply(200, &[], b""), ..Canned::default() }, &args);
        let request = &asked[0].1;
        assert_eq!((request.method.as_str(), request.url.as_str()), ("GET", "/orders?id=1"));
        assert_eq!(request.headers, vec![("Authorization".to_string(), "Bearer {{token}}".to_string())]);
    }

    #[test]
    fn a_response_is_its_status_the_headers_that_matter_and_its_body() {
        let headers = [("Content-Type", "application/json"), ("Server", "nginx"), ("Location", "/next")];
        let canned = || Canned { open: stands(&["Stage"]), reply: reply(422, &headers, b"{\n  \"error\": 1\n}"), ..Canned::default() };
        let (result, _) = call(canned(), &get("/orders"));
        let expected = "GET /orders → 422 · 12 ms · 16 bytes\nContent-Type: application/json\nLocation: /next\n{\n  \"error\": 1\n}";
        assert!(matches!(&result, Ok(ToolResult::HttpRequest { summary, .. }) if summary == "422 · 12 ms"), "{result:?}");
        assert_eq!(text_of(result), expected);
        let (result, _) = call(canned(), &HttpRequestArgs { show_headers: Some(true), ..get("/orders") });
        assert!(text_of(result).contains("Server: nginx"));
    }

    #[test]
    fn what_is_not_text_is_only_measured_and_nothing_adds_no_line() {
        let (result, _) = call(Canned { open: stands(&["Stage"]), reply: reply(200, &[], &[0xff, 0xfe, 0]), ..Canned::default() }, &get("/x"));
        assert_eq!(text_of(result), "GET /x → 200 · 12 ms · 3 bytes\n[not text]");
        let (result, _) = call(Canned { open: stands(&["Stage"]), reply: reply(204, &[], b""), ..Canned::default() }, &get("/x"));
        assert_eq!(text_of(result), "GET /x → 204 · 12 ms · 0 bytes");
    }

    #[test]
    fn a_method_left_out_is_get() {
        let null: HttpRequestArgs = serde_json::from_str(r#"{"method": null, "url": "/x"}"#).unwrap();
        assert_eq!(null.method, None, "a model's null is no method, not a refused call");
        for method in [None, Some(""), Some("  ")] {
            let args = HttpRequestArgs { method: method.map(String::from), ..get("{{baseUrl}}/x") };
            let (result, asked) = call(Canned { open: stands(&["Stage"]), reply: reply(200, &[], b""), ..Canned::default() }, &args);
            assert_eq!(asked[0].1.method, "GET");
            assert!(text_of(result).starts_with("GET {{baseUrl}}/x → 200"));
        }
    }

    fn view(origins: &[&str], requests: &[&str]) -> Option<StandView> {
        Some(StandView {
            name: "Stage".into(),
            access: AgentAccess::ReadOnly,
            origins: origins.iter().map(|o| o.to_string()).collect(),
            variables: vec![("baseUrl".into(), Some("https://api.test".into())), ("token".into(), None)],
            requests: requests.iter().map(|r| r.to_string()).collect(),
        })
    }

    #[test]
    fn no_url_describes_the_environment_and_sends_nothing() {
        for url in [None, Some(" ".to_string())] {
            let canned = Canned { open: stands(&["Stage"]), view: view(&["https://api.test"], &["Shop / List orders: GET {{baseUrl}}/orders"]), ..Canned::default() };
            let (result, asked) = call(canned, &HttpRequestArgs { url, ..HttpRequestArgs::default() });
            assert_eq!(asked.iter().map(|(id, r)| (id.as_str(), r.method.as_str())).collect::<Vec<_>>(), [("id-Stage", "describe")]);
            assert!(matches!(&result, Ok(ToolResult::HttpRequest { summary, .. }) if summary == "Stage · 1 address"), "{result:?}");
            let expected = "Environment \"Stage\", read only: GET, HEAD and OPTIONS.\n\
                It sends to these addresses, where the user's collections go:\n  https://api.test\n\
                Variables, written as {{name}}:\n  baseUrl = https://api.test\n  token — hidden: filled in on sending, never shown\n\
                Requests in the user's collections:\n  Shop / List orders: GET {{baseUrl}}/orders";
            assert_eq!(text_of(result), expected);
        }
    }

    /// The model learns before sending whether a write will be refused, or asked about.
    #[test]
    fn the_description_says_what_a_write_meets() {
        for (access, said) in [
            (AgentAccess::AskForWrites, "open to writes, each one after the user approves it"),
            (AgentAccess::WritesAsOtherTools, "open to writes, approved as the user approves other changes"),
        ] {
            let view = view(&[], &[]).map(|v| StandView { access, ..v });
            let (result, _) = call(Canned { open: stands(&["Stage"]), view, ..Canned::default() }, &HttpRequestArgs::default());
            assert!(text_of(result).starts_with(&format!("Environment \"Stage\", {said}.")), "{access:?}");
        }
    }

    #[test]
    fn an_environment_with_nowhere_to_go_says_who_can_change_that() {
        let canned = Canned { open: stands(&["Stage"]), view: view(&[], &[]), ..Canned::default() };
        let (result, _) = call(canned, &HttpRequestArgs::default());
        assert!(matches!(&result, Ok(ToolResult::HttpRequest { summary, .. }) if summary == "Stage · 0 addresses"), "{result:?}");
        let text = text_of(result);
        assert!(text.contains("It sends nowhere yet") && text.contains("add one on the HTTP page"), "{text}");
        assert!(!text.contains("Requests in"), "{text}");
    }

    /// What the stand sent, not the indents added to it.
    #[test]
    fn the_size_said_is_the_stands() {
        let sized = Some(AgentReply { size: 7, ..reply(200, &[], b"{\n  \"a\": 1\n}").unwrap() });
        let (result, _) = call(Canned { open: stands(&["Stage"]), reply: sized, ..Canned::default() }, &get("/x"));
        assert!(text_of(result).starts_with("GET /x → 200 · 12 ms · 7 bytes\n"));
    }

    #[test]
    fn a_long_response_is_cut_and_says_where_the_rest_is() {
        crate::testing::with_app_dir("http-request-long", || {
            let body = "x".repeat(16_000);
            let (result, _) = call(Canned { open: stands(&["Stage"]), reply: reply(200, &[], body.as_bytes()), ..Canned::default() }, &get("/x"));
            let text = text_of(result);
            assert!(text.chars().count() < 8_500, "{}", text.len());
            let path = text.split("is saved at ").nth(1).and_then(|rest| rest.split(" — ").next()).expect("the path");
            assert!(std::fs::read_to_string(path).unwrap().ends_with(&body), "the whole of it");
        });
    }

    #[test]
    fn a_stand_that_does_not_answer_is_said() {
        let failed = Some(AgentReply { outcome: Outcome::Failed { error: "no response in 30 s".into() }, body: vec![], size: 0 });
        let (result, _) = call(Canned { open: stands(&["Stage"]), reply: failed, ..Canned::default() }, &get("/x"));
        assert_eq!(error_of(result), "the request failed: no response in 30 s");
    }
}
