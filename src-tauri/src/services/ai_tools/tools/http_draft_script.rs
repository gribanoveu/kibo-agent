//! `httpDraftScript` — a script for a saved request, run in the page's sandbox with
//! nothing sent (`docs/45-http-client.md`, HC-11 decision 7, HC-11c): a post-response on
//! a send of this chat's as the model read it, a pre-request on the saved request. It
//! runs nowhere else until the user reads it on the HTTP page and moves it into the
//! request; what it writes is not kept, and its `pm.sendRequest` goes nowhere.

use serde_json::json;

use super::mcp::fit;
use crate::domain::http_client::HttpError;
use crate::domain::http_draft::{AgentDraft, ScriptDraft};
use crate::domain::llm::LlmToolDefinition;
use crate::domain::tools::{HttpDraftScriptArgs, ToolDeps, ToolError, ToolResult};

/// The most of a report the model gets at once.
const MOST: usize = 8000;

pub(super) fn definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "httpDraftScript".into(),
        description: "Draft a Postman script for a request saved in the user's collections and run it, with nothing sent: \
            `listen: \"test\"` is a post-response script (pm.test, pm.expect, pm.response), run on this chat's last httpSend \
            of that request — or the send `exchange` names, its id from httpSend — as you read it; send the request first. \
            `listen: \"prerequest\"` is a pre-request script, run on the saved request: you get the request it would make, \
            changed part by part. Variables are the environment's as you see them — a hidden one is `•••`, so a test or a \
            signature over one is over the mask; `variables` gives values for this run only, such as an id a draft before \
            would have set. You get the tests, the console, an error, the variables it would set — not set — and each \
            pm.sendRequest it would make, which is not sent. The draft is kept beside the request on the HTTP page and does \
            not run on Send until the user reads it and moves it into the request: do not count on it in this turn. Draft \
            one when the user asks for tests or checks to keep, and run it again after each fix."
            .into(),
        parameters: json!({
            "type": "object",
            "properties": {
                "request": {"type": "string", "description": "A request's id, from httpEnvironment"},
                "listen": {"type": "string", "enum": ["test", "prerequest"], "description": "\"test\": post-response; \"prerequest\": pre-request"},
                "code": {"type": "string", "description": "The whole script, JavaScript as Postman runs it"},
                "environment": {"type": "string", "description": "Its name; may be left out when only one is open"},
                "exchange": {"type": "string", "description": "For \"test\": the id of this chat's httpSend to run on; its last of the request when left out"},
                "variables": {"type": "object", "additionalProperties": {"type": "string"}, "description": "Values for `{{name}}` and pm.variables in this run only, over the environment's"}
            },
            "required": ["request", "listen", "code"]
        }),
    }
}

pub fn http_draft_script(args: &HttpDraftScriptArgs, deps: &ToolDeps) -> Result<ToolResult, ToolError> {
    let stands = deps.http.as_deref().ok_or(HttpError::NoneOpen)?;
    let open = stands.open()?;
    let stand = super::http_send::stand(&open, args.environment.as_deref())?;
    let draft = AgentDraft { request: args.request.clone(), listen: args.listen.clone(), code: args.code.clone(), exchange: args.exchange.clone(), variables: args.variables() };
    let kept = stands.draft(&stand.id, &draft)?;
    Ok(ToolResult::HttpDraftScript { text: fit(report_text(&kept, &draft.variables), MOST), summary: summary(&kept) })
}

/// The row's word on it: which script, and how its tests went.
fn summary(draft: &ScriptDraft) -> String {
    let kind = if draft.listen == "test" { "post-response" } else { "pre-request" };
    let report = &draft.report;
    let failed = report.tests.iter().filter(|t| !t.passed && !t.skipped).count();
    let passed = report.tests.iter().filter(|t| t.passed).count();
    match () {
        _ if report.error.is_some() => format!("{kind} · failed"),
        _ if report.tests.is_empty() => kind.to_string(),
        _ if failed > 0 => format!("{kind} · {failed} of {} failed", report.tests.len()),
        _ => format!("{kind} · {passed} passed"),
    }
}

/// What the draft did, for the model: where it ran, what is masked there, its tests,
/// console and error, what it would write and send, and for a pre-request the request.
fn report_text(draft: &ScriptDraft, supplied: &[(String, String)]) -> String {
    let report = &draft.report;
    let mut lines = Vec::new();
    lines.push(match &report.exchange {
        Some(id) => format!("Draft post-response of \"{}\" ran on exchange {id} ({}); nothing was sent.", draft.name, report.ran_on),
        None => format!("Draft pre-request of \"{}\" ran on the saved request with environment \"{}\"; nothing was sent.", draft.name, draft.environment),
    });
    if !report.masked.is_empty() {
        lines.push(format!("Masked (•••) in that exchange — a test of these fails though the API is right: {}.", report.masked.join(", ")));
    }
    if !report.hidden.is_empty() {
        lines.push(format!("Hidden, so ••• in this run — what it computes from them is computed from the mask: {}.", report.hidden.join(", ")));
    }
    if !supplied.is_empty() {
        let values: Vec<String> = supplied.iter().map(|(k, v)| format!("{k} = {v}")).collect();
        lines.push(format!("Supplied by you for this run: {}.", values.join(", ")));
    }
    if !report.tests.is_empty() {
        lines.push("Tests:".into());
        lines.extend(report.tests.iter().map(|t| {
            let mark = if t.skipped { "SKIP" } else if t.passed { "PASS" } else { "FAIL" };
            t.error.as_deref().map_or_else(|| format!("  {mark} {}", t.name), |e| format!("  {mark} {} — {e}", t.name))
        }));
    }
    if let Some(error) = &report.error {
        lines.push(format!("The script failed: {error}"));
    }
    if report.skipped {
        lines.push("It calls pm.execution.skipRequest(): the request would not go.".into());
    }
    if !report.console.is_empty() {
        lines.push("Console:".into());
        lines.extend(report.console.iter().map(|c| if c.level == "log" { format!("  {}", c.text) } else { format!("  [{}] {}", c.level, c.text) }));
    }
    if !report.writes.is_empty() {
        let writes: Vec<String> = report.writes.iter().map(|w| w.value.as_deref().map_or_else(|| format!("{} {} removed", w.scope, w.key), |v| format!("{} {} = {v}", w.scope, w.key))).collect();
        lines.push(format!("Would set — not set, a draft writes nothing: {}.", writes.join("; ")));
    }
    for send in &report.sends {
        let mut notes = Vec::new();
        if send.masked {
            notes.push("a hidden value goes in it");
        }
        if send.foreign {
            notes.push("a host none of the collections' requests goes to");
        }
        let notes = if notes.is_empty() { String::new() } else { format!(" — {}", notes.join("; ")) };
        lines.push(format!("pm.sendRequest, not sent: {} {}{notes}", send.method, send.url));
    }
    if draft.listen == "prerequest" {
        match (&report.request_error, report.request.is_empty()) {
            (Some(why), _) => lines.push(format!("The request after it could not be put together: {why}")),
            (None, true) => lines.push("The request it would make is the saved one, unchanged.".into()),
            (None, false) => {
                lines.push("The request it would make, against the saved one:".into());
                let shown = |v: &Option<String>| v.clone().unwrap_or_else(|| "(none)".into());
                lines.extend(report.request.iter().map(|c| format!("  {}: {} → {}", c.part, shown(&c.before), shown(&c.after))));
            }
        }
    }
    lines.push("Kept as a draft on the HTTP page: it runs on Send only once the user moves it into the request.".into());
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::http_client::HttpStands;
    use crate::domain::http_draft::{DraftReport, DraftSend, DraftWrite, RequestChange};
    use crate::domain::http_scripts::{ConsoleLine, TestResult};
    use crate::services::ai_tools::tools::http_send::tests::{reading, Canned};
    use std::sync::Arc;

    fn deps(http: Option<Arc<dyn HttpStands>>) -> ToolDeps<'static> {
        ToolDeps { http, ..ToolDeps::default() }
    }

    fn args(listen: &str) -> HttpDraftScriptArgs {
        HttpDraftScriptArgs { request: "r1".into(), listen: listen.into(), code: "pm.test('a', () => {});".into(), ..HttpDraftScriptArgs::default() }
    }

    fn test(name: &str, passed: bool, error: Option<&str>) -> TestResult {
        TestResult { name: name.into(), passed, skipped: false, error: error.map(String::from) }
    }

    fn run(report: DraftReport, call: &HttpDraftScriptArgs) -> (String, String, Arc<Canned>) {
        let stands = Arc::new(Canned::drafting(&["Stage"], report));
        let ToolResult::HttpDraftScript { text, summary } = http_draft_script(call, &deps(Some(stands.clone()))).unwrap() else { panic!() };
        (text, summary, stands)
    }

    /// Where it ran, what is masked there, what was supplied, the tests, what it
    /// would set and send — and that it runs on Send only once the user moves it.
    #[test]
    fn a_post_response_draft_says_what_it_ran_on_and_what_it_did() {
        let report = DraftReport {
            exchange: Some("e7".into()),
            ran_on: "GET https://shop.test/orders/1 → 200".into(),
            masked: vec!["$.token".into(), "header X-Token".into()],
            supplied: vec!["orderId".into()],
            hidden: vec!["token".into()],
            tests: vec![test("status", true, None), test("token", false, Some("expected '•••' to equal 'x'"))],
            console: vec![ConsoleLine { level: "log".into(), text: "made 7".into() }, ConsoleLine { level: "warn".into(), text: "slow".into() }],
            writes: vec![DraftWrite { scope: "environment".into(), key: "orderId".into(), value: Some("7".into()) }, DraftWrite { scope: "global".into(), key: "old".into(), value: None }],
            sends: vec![DraftSend { method: "POST".into(), url: "https://auth.other/t".into(), masked: true, foreign: true }, DraftSend { method: "GET".into(), url: "https://shop.test/x".into(), masked: false, foreign: false }],
            ..DraftReport::default()
        };
        let call = HttpDraftScriptArgs { variables: Some([("orderId".to_string(), serde_json::json!(7))].into()), ..args("test") };
        let (text, summary, stands) = run(report, &call);
        assert_eq!(
            text,
            "Draft post-response of \"Shop / Get one\" ran on exchange e7 (GET https://shop.test/orders/1 → 200); nothing was sent.\n\
             Masked (•••) in that exchange — a test of these fails though the API is right: $.token, header X-Token.\n\
             Hidden, so ••• in this run — what it computes from them is computed from the mask: token.\n\
             Supplied by you for this run: orderId = 7.\n\
             Tests:\n  PASS status\n  FAIL token — expected '•••' to equal 'x'\n\
             Console:\n  made 7\n  [warn] slow\n\
             Would set — not set, a draft writes nothing: environment orderId = 7; global old removed.\n\
             pm.sendRequest, not sent: POST https://auth.other/t — a hidden value goes in it; a host none of the collections' requests goes to\n\
             pm.sendRequest, not sent: GET https://shop.test/x\n\
             Kept as a draft on the HTTP page: it runs on Send only once the user moves it into the request."
        );
        assert_eq!(summary, "post-response · 1 of 2 failed");
        assert_eq!(stands.asked(), ["draft id-Stage r1 test"]);
    }

    /// A pre-request's request, part by part; one it could not put together says why.
    #[test]
    fn a_pre_request_draft_says_the_request_it_would_make() {
        let changed = vec![RequestChange { part: "header X-Sig".into(), before: None, after: Some("sig-•••".into()) }, RequestChange { part: "body".into(), before: Some("{}".into()), after: None }];
        let (text, summary, _) = run(DraftReport { request: changed, skipped: true, ..DraftReport::default() }, &args("prerequest"));
        assert!(text.starts_with("Draft pre-request of \"Shop / Get one\" ran on the saved request with environment \"Stage\"; nothing was sent.\n"), "{text}");
        assert!(text.contains("\nIt calls pm.execution.skipRequest(): the request would not go.\n"), "{text}");
        assert!(text.contains("\nThe request it would make, against the saved one:\n  header X-Sig: (none) → sig-•••\n  body: {} → (none)\n"), "{text}");
        assert_eq!(summary, "pre-request");
        let (text, _, _) = run(DraftReport::default(), &args("prerequest"));
        assert!(text.contains("\nThe request it would make is the saved one, unchanged.\n"), "{text}");
        let (text, summary, _) = run(DraftReport { request_error: Some("not found: x".into()), error: Some("boom".into()), ..DraftReport::default() }, &args("prerequest"));
        assert!(text.contains("\nThe script failed: boom\n") && text.contains("\nThe request after it could not be put together: not found: x\n"), "{text}");
        assert_eq!(summary, "pre-request · failed");
        // A post-response has no request of its own to tell.
        let (text, summary, _) = run(DraftReport { exchange: Some("e1".into()), tests: vec![test("a", true, None)], ..DraftReport::default() }, &args("test"));
        assert!(!text.contains("The request it would make"), "{text}");
        assert_eq!(summary, "post-response · 1 passed");
    }

    #[test]
    fn with_no_environment_open_or_two_unnamed_nothing_is_drafted() {
        let none = http_draft_script(&args("test"), &deps(None)).unwrap_err().to_string();
        assert!(none.contains("no environment is open"), "{none}");
        let two = http_draft_script(&args("test"), &deps(Some(reading(&["Stage", "Prod"])))).unwrap_err().to_string();
        assert!(two.contains("say which `environment`"), "{two}");
        let named = HttpDraftScriptArgs { environment: Some("prod".into()), ..args("test") };
        let stands = Arc::new(Canned::opened(&["Stage", "Prod"]));
        let ToolResult::HttpDraftScript { text, .. } = http_draft_script(&named, &deps(Some(stands.clone()))).unwrap() else { panic!() };
        assert_eq!(stands.asked(), ["draft id-Prod r1 test"]);
        assert!(!text.contains("Stage"), "{text}");
    }
}
