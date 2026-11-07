//! `httpRequest` — the HTTP environments the user opened to the agent, read
//! (`docs/45-http-client.md`, HC-7). The environment fixes the host and fills in
//! `{{name}}`: the model passes a path, names its variables, and never sees a
//! secret's value. A stand that does not answer is a result, told as an error.

use serde_json::json;

use super::mcp::fit;
use crate::domain::http_client::{AgentRequest, HttpError, OpenStand, Outcome};
use crate::domain::llm::LlmToolDefinition;
use crate::domain::tools::{HttpRequestArgs, ToolDeps, ToolError, ToolResult};

/// The most of a response the model gets at once; the rest waits in a file.
const MOST: usize = 8000;
/// What a response says that is worth its tokens; `showHeaders` asks for the rest.
const SHOWN: &[&str] = &["content-type", "location", "retry-after", "x-request-id", "x-trace-id", "trace-id"];

pub(super) fn definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "httpRequest".into(),
        description: "Read from an HTTP environment the user opened to the agent on the HTTP page: GET, HEAD or OPTIONS. \
            The host is the environment's `baseUrl`, so pass a path under it, with its query, never an address. \
            `{{name}}` in the path or a header is filled in from the environment, secrets too, and their values never \
            reach you: write `Authorization: Bearer {{token}}`. A name the environment lacks gets its variables listed. \
            A redirect is not followed: you get its status and `location`. No cookies are sent or kept."
            .into(),
        parameters: json!({
            "type": "object",
            "properties": {
                "environment": {"type": "string", "description": "Its name; may be left out when only one is open"},
                "method": {"type": "string", "enum": ["GET", "HEAD", "OPTIONS"]},
                "path": {"type": "string", "description": "Under the environment's baseUrl, e.g. /orders/42?expand=items"},
                "headers": {"type": "object", "additionalProperties": {"type": "string"}, "description": "Sent as written, `{{name}}` filled in"},
                "showHeaders": {"type": "boolean", "description": "All the response's headers, not the few that usually matter"}
            },
            "required": ["method", "path"]
        }),
    }
}

pub fn http_request(args: &HttpRequestArgs, deps: &ToolDeps) -> Result<ToolResult, ToolError> {
    let stands = deps.http.as_deref().ok_or(HttpError::NoneOpen)?;
    let open = stands.open()?;
    let stand = stand(&open, args.environment.as_deref())?;
    let headers = args.headers.iter().flatten().map(|(k, v)| (k.clone(), v.clone())).collect();
    let request = AgentRequest { method: args.method.clone(), path: args.path.clone(), headers };
    let reply = stands.send(&stand.id, &request)?;
    let (status, headers, elapsed_ms) = match reply.outcome {
        Outcome::Response { status, headers, elapsed_ms, .. } => (status, headers, elapsed_ms),
        Outcome::Failed { error } => return Err(HttpError::Unanswered(error).into()),
    };
    let first = format!("{} {} → {status} · {elapsed_ms} ms · {} bytes", args.method.trim().to_ascii_uppercase(), args.path, reply.size);
    let lines = headers.iter().filter(|(k, _)| args.show_headers == Some(true) || SHOWN.contains(&k.to_ascii_lowercase().as_str())).map(|(k, v)| format!("{k}: {v}"));
    let text = std::iter::once(first).chain(lines).chain(body_text(&reply.body)).collect::<Vec<_>>().join("\n");
    Ok(ToolResult::HttpRequest { text: fit(text, MOST), summary: format!("{status} · {elapsed_ms} ms") })
}

/// The environment by its name, any case; with none named, the only one open.
fn stand<'a>(open: &'a [OpenStand], name: Option<&str>) -> Result<&'a OpenStand, HttpError> {
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
    use std::sync::{Arc, Mutex};

    /// Its environments, one canned reply, and what it was asked.
    #[derive(Default)]
    struct Canned {
        open: Vec<OpenStand>,
        reply: Option<AgentReply>,
        asked: Mutex<Vec<(String, AgentRequest)>>,
    }

    impl HttpStands for Canned {
        fn open(&self) -> Result<Vec<OpenStand>, HttpError> {
            Ok(self.open.clone())
        }

        fn send(&self, id: &str, request: &AgentRequest) -> Result<AgentReply, HttpError> {
            self.asked.lock().unwrap().push((id.to_string(), request.clone()));
            Ok(self.reply.clone().unwrap())
        }
    }

    /// One environment that answers 200, for what is about the way there.
    pub(crate) fn answering() -> Arc<dyn HttpStands> {
        Arc::new(Canned { open: stands(&["Stage"]), reply: reply(200, &[], b"ok"), ..Canned::default() })
    }

    fn stands(names: &[&str]) -> Vec<OpenStand> {
        names.iter().map(|n| OpenStand { id: format!("id-{n}"), name: n.to_string() }).collect()
    }

    fn reply(status: u16, headers: &[(&str, &str)], body: &[u8]) -> Option<AgentReply> {
        let headers = headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        Some(AgentReply { outcome: Outcome::Response { status, headers, url: "http://stand/x".into(), redirects: vec![], elapsed_ms: 12 }, body: body.to_vec(), size: body.len() })
    }

    fn get(path: &str) -> HttpRequestArgs {
        HttpRequestArgs { method: "get".into(), path: path.into(), ..HttpRequestArgs::default() }
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

    /// No card asks for a read; with HC-8 the method decides, and this flips.
    #[test]
    fn reading_a_stand_asks_nobody() {
        assert!(!crate::domain::tools::ToolName::HttpRequest.is_mutating());
    }

    #[test]
    fn what_was_asked_reaches_the_stand_as_asked() {
        let headers = Some([("Authorization".to_string(), "Bearer {{token}}".to_string())].into());
        let args = HttpRequestArgs { headers, ..get("/orders?id=1") };
        let (_, asked) = call(Canned { open: stands(&["Stage"]), reply: reply(200, &[], b""), ..Canned::default() }, &args);
        let request = &asked[0].1;
        assert_eq!((request.method.as_str(), request.path.as_str()), ("get", "/orders?id=1"));
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
