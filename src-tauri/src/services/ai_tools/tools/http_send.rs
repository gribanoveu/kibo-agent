//! `httpSend` — a request saved in the user's collections, sent into an HTTP
//! environment the user opened to the agent with the model's edits over it
//! (`docs/45-http-client.md`, HC-10c). The page's Send does the rest: auth,
//! `oauth2`, the scripts the user trusted, `{{name}}` filled in — a hidden value
//! never reaches the model. The host is the saved request's. A write goes as the
//! environment's mode says — the card is the loop's (`ApprovalPolicy::stand_asks`).
//! A stand that does not answer is a result, told as an error.

use serde_json::json;

use super::mcp::fit;
use crate::domain::http_client::{AgentEdits, AgentSend, HttpError, OpenStand, Outcome};
use crate::domain::http_scripts::ScriptReport;
use crate::domain::llm::LlmToolDefinition;
use crate::domain::tools::{ChangeDiff, HttpSendArgs, ToolDeps, ToolError, ToolPreview, ToolResult};
use crate::services::text_diff::diff_stats;

/// The most of a response the model gets at once; the rest waits in a file.
const MOST: usize = 8000;
/// What a response says that is worth its tokens; `showHeaders` asks for the rest.
const SHOWN: &[&str] = &["content-type", "location", "retry-after", "x-request-id", "x-trace-id", "trace-id"];

pub(super) fn definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "httpSend".into(),
        description: "Send a request saved in the user's collections into an HTTP environment the user opened to the agent, \
            as the HTTP page's Send does: its auth, OAuth 2.0 token and the scripts the user trusted are the app's, and a hidden \
            value never reaches you. `request` is an id from httpEnvironment; read the request with httpReadRequest first. \
            Change what the case needs: `method`; `path` — what follows the host part of its address, from one `/` (for \
            `{{baseUrl}}/orders/1`, `/orders/2`); `query` — the whole query; `headers` — by name, `null` takes one away; `body` — \
            the whole body, JSON going as application/json; `auth: \"none\"` — no auth at all; `variables` — values for \
            `{{name}}` in this send only. The host is always the saved request's: for another endpoint on it, send any request \
            that goes there with another method and path. GET, HEAD and OPTIONS always go; another method goes as the \
            environment allows — refused when it is read only, otherwise after the user approves, when its mode asks. You get \
            the status, the headers that matter, the scripts' tests and console, and the body. A redirect is not followed."
            .into(),
        parameters: json!({
            "type": "object",
            "properties": {
                "request": {"type": "string", "description": "A request's id, from httpEnvironment"},
                "environment": {"type": "string", "description": "Its name; may be left out when only one is open"},
                "method": {"type": "string", "enum": ["GET", "HEAD", "OPTIONS", "POST", "PUT", "PATCH", "DELETE"], "description": "The saved request's when left out"},
                "path": {"type": "string", "description": "In place of the saved one, from one `/`; `{{name}}` filled in"},
                "query": {"type": "string", "description": "The whole query in place of the saved one, `a=1&b={{name}}`; empty for none"},
                "headers": {"type": "object", "additionalProperties": {"type": ["string", "null"]}, "description": "Set by name over the saved ones, `{{name}}` filled in; null takes one away"},
                "body": {"type": "string", "description": "The whole body in place of the saved one, `{{name}}` filled in"},
                "auth": {"type": "string", "enum": ["none"], "description": "\"none\": no auth, the request's own or inherited"},
                "variables": {"type": "object", "additionalProperties": {"type": "string"}, "description": "Values for `{{name}}` in this send, over the environment's; not a hidden one, nor one in a host or an auth"},
                "showHeaders": {"type": "boolean", "description": "All the response's headers, not the few that usually matter"}
            },
            "required": ["request"]
        }),
    }
}

pub fn http_send(args: &HttpSendArgs, deps: &ToolDeps) -> Result<ToolResult, ToolError> {
    let stands = deps.http.as_deref().ok_or(HttpError::NoneOpen)?;
    let open = stands.open()?;
    let stand = stand(&open, args.environment.as_deref())?;
    let sent = stands.send(&stand.id, &agent_send(args)?)?;
    let (status, headers, elapsed_ms) = match sent.reply.outcome {
        Outcome::Response { status, headers, elapsed_ms, .. } => (status, headers, elapsed_ms),
        Outcome::Failed { error } => return Err(HttpError::Unanswered(error).into()),
    };
    let first = format!("{} {} → {status} · {elapsed_ms} ms · {} bytes", sent.method, sent.url, sent.reply.size);
    let lines = headers.iter().filter(|(k, _)| args.show_headers == Some(true) || SHOWN.contains(&k.to_ascii_lowercase().as_str())).map(|(k, v)| format!("{k}: {v}"));
    // Before the body: a long one is cut, and the tests are the point of a case.
    let text = std::iter::once(first).chain(lines).chain(scripts_text(&sent.scripts)).chain(body_text(&sent.reply.body)).collect::<Vec<_>>().join("\n");
    Ok(ToolResult::HttpSend { text: fit(text, MOST), summary: format!("{status} · {elapsed_ms} ms") })
}

/// The call as the stands take it: `auth` is `"none"` or nothing.
pub(crate) fn agent_send(args: &HttpSendArgs) -> Result<AgentSend, HttpError> {
    let no_auth = match args.auth.as_deref().map(str::trim) {
        None | Some("") => false,
        Some(none) if none.eq_ignore_ascii_case("none") => true,
        Some(other) => return Err(HttpError::BadAuth(other.into())),
    };
    let edits = AgentEdits {
        method: args.method.clone(),
        path: args.path.clone(),
        query: args.query.clone(),
        headers: args.headers.iter().flatten().map(|(k, v)| (k.clone(), v.clone())).collect(),
        body: args.body.clone(),
        no_auth,
    };
    Ok(AgentSend { request: args.request.trim().to_string(), edits, variables: args.variables() })
}

/// What the scripts said: whether they ran, each test, a failure, the console.
fn scripts_text(report: &ScriptReport) -> Vec<String> {
    let mut lines = Vec::new();
    if report.not_run {
        lines.push("Scripts not run: the user has not trusted them on the HTTP page.".into());
    }
    if !report.tests.is_empty() {
        lines.push("Tests:".into());
        lines.extend(report.tests.iter().map(|t| {
            let mark = if t.skipped { "SKIP" } else if t.passed { "PASS" } else { "FAIL" };
            t.error.as_deref().map_or_else(|| format!("  {mark} {}", t.name), |e| format!("  {mark} {} — {e}", t.name))
        }));
    }
    if let Some(error) = &report.error {
        lines.push(format!("A script failed: {error}"));
    }
    if !report.console.is_empty() {
        lines.push("Console:".into());
        lines.extend(report.console.iter().map(|c| if c.level == "log" { format!("  {}", c.text) } else { format!("  [{}] {}", c.level, c.text) }));
    }
    lines
}

/// The approval card's account of a call: where, which request, the edits —
/// as the model wrote them, `{{token}}` and not its value. Built from the call
/// alone, so it is the same in Agent and in Chat; the loop's reason names the
/// saved request and the method it goes with.
pub fn preview(args: &HttpSendArgs) -> ToolPreview {
    let place = args.environment.as_deref().map(str::trim).filter(|e| !e.is_empty()).map_or_else(|| "the environment open to the agent".to_string(), |e| format!("environment {e}"));
    let method = args.method.as_deref().map(str::trim).filter(|m| !m.is_empty()).map(str::to_ascii_uppercase);
    let query = args.query.as_deref().map(|q| format!("?{}", q.trim().trim_start_matches('?')));
    let address = [args.path.clone(), query].into_iter().flatten().collect::<String>();
    let line = [method, Some(address).filter(|a| !a.is_empty())].into_iter().flatten().collect::<Vec<_>>().join(" ");
    let summary = if line.is_empty() { format!("request {} as saved", args.request) } else { format!("request {}: {line}", args.request) };
    let mut notes: Vec<String> = args.headers.iter().flatten().map(|(k, v)| v.as_deref().map_or_else(|| format!("{k}: removed"), |v| format!("{k}: {v}"))).collect();
    if args.auth.is_some() {
        notes.push(format!("auth: {}", args.auth.as_deref().unwrap_or_default()));
    }
    notes.extend(args.variables().into_iter().map(|(k, v)| format!("{{{{{k}}}}} = {v}")));
    let diffs = args.body.iter().map(|body| ChangeDiff { title: "Body".into(), diff: diff_stats("", body) }).collect();
    ToolPreview::Change { place, summary, notes, production: false, diffs }
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
    use crate::domain::http_client::{acts_as, AgentAccess, AgentReply, AgentSent, HttpStands, StandView};
    use crate::domain::http_scripts::{ConsoleLine, TestResult};
    use crate::domain::tools::{ToolCall, ToolName};
    use std::sync::{Arc, Mutex};

    /// Its environments, one canned reply, and what it was asked. Two saved
    /// requests: `r1`, a GET of `{{baseUrl}}/orders/1`, and `w1`, a POST to `{{baseUrl}}/orders`.
    #[derive(Default)]
    pub(crate) struct Canned {
        open: Vec<OpenStand>,
        reply: Option<AgentReply>,
        scripts: ScriptReport,
        view: Option<StandView>,
        asked: Mutex<Vec<String>>,
        sends: Mutex<Vec<AgentSend>>,
        /// The environments' file could not be read.
        unreadable: bool,
    }

    impl Canned {
        pub(crate) fn asked(&self) -> Vec<String> {
            self.asked.lock().unwrap().clone()
        }

        fn template(request: &str) -> Result<(&'static str, &'static str, &'static str), HttpError> {
            match request {
                "r1" => Ok(("GET", "{{baseUrl}}/orders/1", "Shop / Get one")),
                "w1" => Ok(("POST", "{{baseUrl}}/orders", "Shop / Create")),
                // As the real one says when the environments' file could not be read.
                "s1" => Err(HttpError::Store("unreadable".into())),
                other => Err(HttpError::NoRequest(other.into())),
            }
        }

        /// What the call goes as: its method or the template's, its path or the template's.
        fn going(send: &AgentSend) -> Result<(String, String, String, String), HttpError> {
            let (method, url, name) = Self::template(&send.request)?;
            let method = send.edits.method.as_deref().map_or(method.to_string(), str::to_ascii_uppercase);
            let headers = send.edits.headers.iter().filter_map(|(k, v)| Some((k.as_str(), v.as_deref()?)));
            let acts = acts_as(&method, headers);
            let url = send.edits.path.as_deref().map_or(url.to_string(), |p| format!("{{{{baseUrl}}}}{p}"));
            Ok((acts, method, url, name.to_string()))
        }
    }

    impl HttpStands for Canned {
        fn open(&self) -> Result<Vec<OpenStand>, HttpError> {
            if self.unreadable {
                return Err(HttpError::Store("unreadable".into()));
            }
            Ok(self.open.clone())
        }

        fn describe(&self, id: &str) -> Result<StandView, HttpError> {
            self.asked.lock().unwrap().push(format!("describe {id}"));
            Ok(self.view.clone().unwrap())
        }

        fn sending(&self, _id: &str, send: &AgentSend) -> Result<(String, String), HttpError> {
            let (acts, method, url, name) = Self::going(send)?;
            Ok((acts, format!("{method} {url}, from \"{name}\"")))
        }

        fn send(&self, id: &str, send: &AgentSend) -> Result<AgentSent, HttpError> {
            self.asked.lock().unwrap().push(format!("send {id} {}", send.request));
            self.sends.lock().unwrap().push(send.clone());
            let (_, method, url, _) = Self::going(send)?;
            Ok(AgentSent { method, url, reply: self.reply.clone().unwrap(), scripts: self.scripts.clone() })
        }

        /// `r1` in any environment open; any other request is not there.
        fn read(&self, id: &str, request: &str) -> Result<String, HttpError> {
            match request {
                "r1" => Ok(format!("Request \"Shop / Get one\", id r1: GET {{{{baseUrl}}}}/orders/1\nread in {id}")),
                "q1" => Ok("Request \"Shop / Say \"hi\"\", id q1: GET /hi".to_string()),
                other => Err(HttpError::NoRequest(other.into())),
            }
        }
    }

    /// Environments by `names`, each reading the request `r1`.
    pub(crate) fn reading(names: &[&str]) -> Arc<dyn HttpStands> {
        Arc::new(Canned { open: stands(names), ..Canned::default() })
    }

    /// One environment that answers 200, for what is about the way there.
    pub(crate) fn answering() -> Arc<dyn HttpStands> {
        opened_as(AgentAccess::ReadOnly)
    }

    /// Environments that cannot be read.
    pub(crate) fn unreadable() -> Arc<dyn HttpStands> {
        Arc::new(Canned { unreadable: true, ..Canned::default() })
    }

    /// One environment, open to the agent as `access` says, that answers 200.
    pub(crate) fn opened_as(access: AgentAccess) -> Arc<dyn HttpStands> {
        let open = stands(&["Stage"]).into_iter().map(|s| OpenStand { access, ..s }).collect();
        Arc::new(Canned { open, reply: reply(200, &[], b"ok"), ..Canned::default() })
    }

    /// One environment described as `view` says.
    pub(crate) fn described_as(view: StandView) -> Arc<Canned> {
        Arc::new(Canned { open: stands(&["Stage"]), view: Some(view), ..Canned::default() })
    }

    fn stands(names: &[&str]) -> Vec<OpenStand> {
        names.iter().map(|n| OpenStand { id: format!("id-{n}"), name: n.to_string(), access: AgentAccess::ReadOnly }).collect()
    }

    fn reply(status: u16, headers: &[(&str, &str)], body: &[u8]) -> Option<AgentReply> {
        let headers = headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        Some(AgentReply { outcome: Outcome::Response { status, headers, url: "http://stand/x".into(), redirects: vec![], elapsed_ms: 12 }, body: body.to_vec(), size: body.len() })
    }

    fn canned(body: &[u8]) -> Canned {
        Canned { open: stands(&["Stage"]), reply: reply(200, &[], body), ..Canned::default() }
    }

    fn of(request: &str) -> HttpSendArgs {
        HttpSendArgs { request: request.into(), ..HttpSendArgs::default() }
    }

    fn call(canned: Canned, args: &HttpSendArgs) -> (Result<ToolResult, ToolError>, Vec<String>, Vec<AgentSend>) {
        let canned = Arc::new(canned);
        let deps = ToolDeps { http: Some(canned.clone()), ..ToolDeps::default() };
        let result = http_send(args, &deps);
        let sends = canned.sends.lock().unwrap().clone();
        (result, canned.asked(), sends)
    }

    fn text_of(result: Result<ToolResult, ToolError>) -> String {
        match result {
            Ok(ToolResult::HttpSend { text, .. }) => text,
            other => panic!("{other:?}"),
        }
    }

    fn error_of(result: Result<ToolResult, ToolError>) -> String {
        result.unwrap_err().to_string()
    }

    #[test]
    fn nothing_open_says_where_to_open_it() {
        let told = "no environment is open to the agent — set its agent access on the HTTP page";
        assert_eq!(http_send(&of("r1"), &ToolDeps::default()).unwrap_err().to_string(), told, "the feature off");
        assert_eq!(error_of(call(Canned::default(), &of("r1")).0), told);
        let named = HttpSendArgs { environment: Some("Stage".into()), ..of("r1") };
        assert_eq!(error_of(call(Canned::default(), &named).0), told, "no name is confirmed or denied");
    }

    #[test]
    fn the_only_one_open_needs_no_name_and_several_do() {
        let (result, asked, _) = call(canned(b""), &of("r1"));
        assert!(result.is_ok());
        assert_eq!(asked, ["send id-Stage r1"]);
        let (result, asked, _) = call(Canned { open: stands(&["Stage", "Local"]), ..Canned::default() }, &of("r1"));
        assert_eq!(error_of(result), "say which `environment`: \"Stage\", \"Local\"");
        assert!(asked.is_empty());
    }

    #[test]
    fn a_name_is_found_in_any_case_and_one_not_open_names_only_those_that_are() {
        let by_name = |name: &str| HttpSendArgs { environment: Some(name.into()), ..of("r1") };
        let (_, asked, _) = call(Canned { open: stands(&["Stage", "Local"]), reply: reply(200, &[], b""), ..Canned::default() }, &by_name(" local "));
        assert_eq!(asked, ["send id-Local r1"]);
        let (result, asked, _) = call(Canned { open: stands(&["Stage", "Local"]), ..Canned::default() }, &by_name("Prod"));
        assert_eq!(error_of(result), "no environment \"Prod\" is open to the agent: \"Stage\", \"Local\"");
        assert!(asked.is_empty());
        let (result, _, _) = call(Canned { open: stands(&["Stage", "stage"]), ..Canned::default() }, &by_name("STAGE"));
        assert_eq!(error_of(result), "2 environments are named \"STAGE\" — rename one on the HTTP page");
        let (result, _, _) = call(canned(b""), &HttpSendArgs { environment: Some(" ".into()), ..of("r1") });
        assert!(result.is_ok(), "an empty name is no name: {result:?}");
    }

    /// The edits reach the stands as the model wrote them, `{{name}}` and all.
    #[test]
    fn every_edit_reaches_the_stands_as_written() {
        let headers = Some([("Authorization".to_string(), Some("Bearer {{bad}}".to_string())), ("X-Trace".to_string(), None)].into());
        let variables = Some([("id".to_string(), json!(7)), ("name".to_string(), json!("Ann"))].into());
        let args = HttpSendArgs {
            method: Some("patch".into()),
            path: Some("/orders/{{id}}".into()),
            query: Some("expand=items".into()),
            headers,
            body: Some("{\"name\": \"{{name}}\"}".into()),
            auth: Some(" None ".into()),
            variables,
            ..of("w1")
        };
        let (_, _, sends) = call(canned(b""), &args);
        let edits = AgentEdits {
            method: Some("patch".into()),
            path: Some("/orders/{{id}}".into()),
            query: Some("expand=items".into()),
            headers: vec![("Authorization".into(), Some("Bearer {{bad}}".into())), ("X-Trace".into(), None)],
            body: Some("{\"name\": \"{{name}}\"}".into()),
            no_auth: true,
        };
        assert_eq!(sends, [AgentSend { request: "w1".into(), edits, variables: vec![("id".into(), "7".into()), ("name".into(), "Ann".into())] }]);
        let (result, asked, _) = call(canned(b""), &HttpSendArgs { auth: Some("bearer".into()), ..of("r1") });
        assert_eq!(error_of(result), "`auth` takes only \"none\": bearer — to send a wrong token, set the `Authorization` header");
        assert!(asked.is_empty());
        let parsed: HttpSendArgs = serde_json::from_str(r#"{"request": "r1", "headers": {"X-A": null, "X-B": "b"}, "auth": null, "method": null}"#).unwrap();
        assert_eq!((parsed.headers.unwrap()["X-A"].clone(), parsed.auth, parsed.method), (None, None, None), "a model's null is no edit, or a header taken away");
        let none: HttpSendArgs = serde_json::from_str(r#"{"request": "r1", "auth": "none"}"#).unwrap();
        assert_eq!(none.auth.as_deref(), Some("none"), "a word, not a model's way of saying nothing");
    }

    /// The tool writes, so Analyst never gets it. A call that names a read is not risky;
    /// one with no method is the template's, which the loop reads (`HttpStands::sending`).
    #[test]
    fn a_read_is_not_risky_and_a_write_or_the_unknown_is() {
        assert!(ToolName::HttpSend.is_mutating());
        let risky = |method: Option<&str>| ToolCall::HttpSend(HttpSendArgs { method: method.map(Into::into), ..of("r1") }).is_risky();
        for read in [Some("GET"), Some(" head "), Some("options")] {
            assert!(!risky(read), "{read:?}");
        }
        for write in [None, Some("POST"), Some("put"), Some("PATCH"), Some("DELETE")] {
            assert!(risky(write), "{write:?}");
        }
        let headers = Some([("X-HTTP-Method-Override".to_string(), Some("DELETE".to_string()))].into());
        assert!(ToolCall::HttpSend(HttpSendArgs { method: Some("GET".into()), headers, ..of("r1") }).is_risky(), "a GET that deletes");
    }

    /// The card says where, which request, and the edits as the model wrote them —
    /// `{{token}}`, never its value, which the call does not hold.
    #[test]
    fn the_card_shows_the_call_as_written() {
        let headers = Some([("Authorization".to_string(), Some("Bearer {{token}}".to_string())), ("X-Old".to_string(), None)].into());
        let variables = Some([("id".to_string(), json!(42))].into());
        let args = HttpSendArgs { environment: Some(" Stage ".into()), method: Some("post".into()), path: Some("/orders".into()), query: Some("?dry=1".into()), headers, body: Some("{\"a\": 1}".into()), auth: Some("none".into()), variables, ..of("w1") };
        let ToolPreview::Change { place, summary, notes, production, diffs } = preview(&args) else { panic!("a change") };
        assert_eq!((place.as_str(), summary.as_str(), production), ("environment Stage", "request w1: POST /orders?dry=1", false));
        assert_eq!(notes, ["Authorization: Bearer {{token}}", "X-Old: removed", "auth: none", "{{id}} = 42"]);
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].title, "Body");
        assert!(diffs[0].diff.unified_diff.contains("+{\"a\": 1}"), "{:?}", diffs[0].diff);

        let ToolPreview::Change { place, summary, notes, diffs, .. } = preview(&of("w1")) else { panic!("a change") };
        assert_eq!((place.as_str(), summary.as_str()), ("the environment open to the agent", "request w1 as saved"));
        assert!(notes.is_empty() && diffs.is_empty());
    }

    #[test]
    fn a_response_is_its_status_the_headers_that_matter_the_tests_and_its_body() {
        let headers = [("Content-Type", "application/json"), ("Server", "nginx"), ("Location", "/next")];
        let scripts = ScriptReport {
            tests: vec![
                TestResult { name: "Status is 201".into(), passed: true, skipped: false, error: None },
                TestResult { name: "Has id".into(), passed: false, skipped: false, error: Some("expected undefined to be a number".into()) },
                TestResult { name: "Later".into(), passed: false, skipped: true, error: None },
            ],
            console: vec![ConsoleLine { level: "log".into(), text: "made 7".into() }, ConsoleLine { level: "warn".into(), text: "slow".into() }],
            error: Some("ReferenceError: x is not defined".into()),
            ..ScriptReport::default()
        };
        let canned = || Canned { open: stands(&["Stage"]), reply: reply(422, &headers, b"{\n  \"error\": 1\n}"), scripts: scripts.clone(), ..Canned::default() };
        let (result, _, _) = call(canned(), &HttpSendArgs { path: Some("/orders/9".into()), ..of("r1") });
        let expected = "GET {{baseUrl}}/orders/9 → 422 · 12 ms · 16 bytes\nContent-Type: application/json\nLocation: /next\n\
            Tests:\n  PASS Status is 201\n  FAIL Has id — expected undefined to be a number\n  SKIP Later\n\
            A script failed: ReferenceError: x is not defined\n\
            Console:\n  made 7\n  [warn] slow\n\
            {\n  \"error\": 1\n}";
        assert!(matches!(&result, Ok(ToolResult::HttpSend { summary, .. }) if summary == "422 · 12 ms"), "{result:?}");
        assert_eq!(text_of(result), expected);
        let (result, _, _) = call(canned(), &HttpSendArgs { show_headers: Some(true), ..of("r1") });
        assert!(text_of(result).contains("Server: nginx"));
    }

    #[test]
    fn scripts_not_trusted_are_said() {
        let untrusted = Canned { scripts: ScriptReport { not_run: true, ..ScriptReport::default() }, ..canned(b"ok") };
        let (result, _, _) = call(untrusted, &of("r1"));
        assert_eq!(text_of(result), "GET {{baseUrl}}/orders/1 → 200 · 12 ms · 2 bytes\nScripts not run: the user has not trusted them on the HTTP page.\nok");
    }

    #[test]
    fn what_is_not_text_is_only_measured_and_nothing_adds_no_line() {
        let (result, _, _) = call(canned(&[0xff, 0xfe, 0]), &of("r1"));
        assert_eq!(text_of(result), "GET {{baseUrl}}/orders/1 → 200 · 12 ms · 3 bytes\n[not text]");
        let (result, _, _) = call(Canned { reply: reply(204, &[], b""), ..canned(b"") }, &of("w1"));
        assert_eq!(text_of(result), "POST {{baseUrl}}/orders → 204 · 12 ms · 0 bytes");
    }

    /// What the stand sent, not the indents added to it.
    #[test]
    fn the_size_said_is_the_stands() {
        let sized = Some(AgentReply { size: 7, ..reply(200, &[], b"{\n  \"a\": 1\n}").unwrap() });
        let (result, _, _) = call(Canned { reply: sized, ..canned(b"") }, &of("r1"));
        assert!(text_of(result).starts_with("GET {{baseUrl}}/orders/1 → 200 · 12 ms · 7 bytes\n"));
    }

    #[test]
    fn a_long_response_is_cut_and_says_where_the_rest_is() {
        crate::testing::with_app_dir("http-send-long", || {
            let body = "x".repeat(16_000);
            let (result, _, _) = call(canned(body.as_bytes()), &of("r1"));
            let text = text_of(result);
            assert!(text.chars().count() < 8_500, "{}", text.len());
            let path = text.split("is saved at ").nth(1).and_then(|rest| rest.split(" — ").next()).expect("the path");
            assert!(std::fs::read_to_string(path).unwrap().ends_with(&body), "the whole of it");
        });
    }

    #[test]
    fn a_stand_that_does_not_answer_is_said() {
        let failed = Some(AgentReply { outcome: Outcome::Failed { error: "no response in 30 s".into() }, body: vec![], size: 0 });
        let (result, _, _) = call(Canned { reply: failed, ..canned(b"") }, &of("r1"));
        assert_eq!(error_of(result), "the request failed: no response in 30 s");
    }
}
