//! A script the model drafts (`docs/45-http-client.md`, HC-11 decision 7): run in the
//! sandbox with nothing sent — a post-response on a send of this chat's, as the model
//! read it, a pre-request on the saved request — and kept apart from the collection,
//! where nothing runs it. This is what the run reports and the rules it is read by.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::http_client::{HistoryEntry, HttpError, Outcome, Outgoing, Scope, SECRET_MASK};
use super::http_scripts::{changes, ConsoleLine, ScriptVars, TestResult};

/// What a `pm.sendRequest` of a draft gets instead of a response.
pub const NOT_SENT: &str = "not sent: a draft sends nothing";

/// `httpDraftScript`'s call, its environment already found.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AgentDraft {
    pub request: String,
    /// `prerequest` or `test`, as Postman names them.
    pub listen: String,
    pub code: String,
    /// A send of this chat's by its id; the last of this request when left out.
    pub exchange: Option<String>,
    /// Values for this run only, over the environment's.
    pub variables: Vec<(String, String)>,
}

/// A `pm.sendRequest` the draft would have made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftSend {
    pub method: String,
    pub url: String,
    /// A hidden value went into it: where it goes is where a secret would.
    pub masked: bool,
    /// None of the collections' requests goes to its host.
    pub foreign: bool,
}

/// A variable a draft would write: `None` takes it away.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftWrite {
    /// `global`, `collection` or `environment`.
    pub scope: String,
    pub key: String,
    pub value: Option<String>,
}

/// A part of the request a pre-request draft changed: `method`, `url`, `header X`, `body`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestChange {
    pub part: String,
    pub before: Option<String>,
    pub after: Option<String>,
}

/// What a draft did, with nothing sent and nothing written.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DraftReport {
    /// The send a post-response ran on, by its id, and what it was: `GET url → 200`.
    pub exchange: Option<String>,
    pub ran_on: String,
    /// Where that send has `•••`: a test of it fails though the stand is right.
    pub masked: Vec<String>,
    /// Names the call gave values to.
    pub supplied: Vec<String>,
    /// Hidden variables the code names: `•••` in the run, so what it computes from one —
    /// a signature, say — is computed from the mask.
    pub hidden: Vec<String>,
    pub tests: Vec<TestResult>,
    pub console: Vec<ConsoleLine>,
    pub error: Option<String>,
    /// `pm.execution.skipRequest()`: the request would not go.
    pub skipped: bool,
    pub writes: Vec<DraftWrite>,
    pub sends: Vec<DraftSend>,
    /// A pre-request's: the request after it, against the saved one.
    pub request: Vec<RequestChange>,
    /// Why the request after it could not be put together.
    pub request_error: Option<String>,
}

/// The last draft of a request's script from one chat: what the page shows beside it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptDraft {
    pub collection: String,
    pub request: String,
    /// `Collection / Request`.
    pub name: String,
    pub listen: String,
    pub chat: String,
    pub chat_title: String,
    pub environment: String,
    pub code: String,
    pub at: i64,
    pub report: DraftReport,
}

/// The draft the page showed the user: its key and the code they read. A draft the model
/// replaced since is another, and nothing done to this one reaches it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftShown {
    pub collection: String,
    pub request: String,
    pub listen: String,
    pub chat: String,
    pub code: String,
}

#[cfg(test)]
impl ScriptDraft {
    /// A chat's draft of nothing in particular, for tests of where drafts go.
    pub fn of_chat(chat: &str) -> Self {
        let s = String::new;
        Self { collection: "c".into(), request: "r".into(), name: s(), listen: "test".into(), chat: chat.into(), chat_title: s(), environment: s(), code: "1;".into(), at: 0, report: DraftReport::default() }
    }
}

impl DraftShown {
    pub fn is(&self, d: &ScriptDraft) -> bool {
        (&d.collection, &d.request, &d.listen, &d.chat, &d.code) == (&self.collection, &self.request, &self.listen, &self.chat, &self.code)
    }
}

/// Every string in `value` with `scope`'s secrets and what it keeps masked: a request
/// as a draft's script gets it, a credential written into it by hand not there.
pub fn masked_strings(value: &mut Value, scope: &Scope) {
    match value {
        Value::String(text) => *text = scope.hide(text),
        Value::Array(items) => items.iter_mut().for_each(|v| masked_strings(v, scope)),
        Value::Object(fields) => fields.values_mut().for_each(|v| masked_strings(v, scope)),
        _ => {}
    }
}

/// The hidden variables — `•••` as the model sees them — that `code` names, quoted
/// or as `{{name}}`: a name alone could be any word.
pub fn hidden_named(vars: &ScriptVars, code: &str) -> Vec<String> {
    let layers = [&vars.globals, &vars.collection, &vars.environment];
    let mut named: Vec<String> = Vec::new();
    for v in layers.into_iter().flatten().filter(|v| v.secret && v.value == SECRET_MASK) {
        let quoted = [format!("'{}'", v.key), format!("\"{}\"", v.key), format!("`{}`", v.key), format!("{{{{{}}}}}", v.key)];
        if quoted.iter().any(|q| code.contains(q.as_str())) && !named.contains(&v.key) {
            named.push(v.key.clone());
        }
    }
    named
}

/// Drafts kept at most, the oldest dropped first.
pub const DRAFTS: usize = 200;

/// `draft` in place of the one from the same chat for the same script, newest last.
pub fn keep_draft(drafts: &mut Vec<ScriptDraft>, draft: ScriptDraft) {
    drafts.retain(|d| (&d.collection, &d.request, &d.listen, &d.chat) != (&draft.collection, &draft.request, &draft.listen, &draft.chat));
    drafts.push(draft);
    let over = drafts.len().saturating_sub(DRAFTS);
    drafts.drain(..over);
}

/// The send a post-response draft runs on: `exchange`, or this chat's last of `request`.
/// Another chat's is not taken — what it read is not this model's.
pub fn fingerprint<'a>(history: &'a [HistoryEntry], chat: &str, request: &str, exchange: Option<&str>) -> Result<&'a HistoryEntry, HttpError> {
    let ours = |e: &&HistoryEntry| e.agent_chat_id.as_deref() == Some(chat);
    let found = match exchange.map(str::trim).filter(|e| !e.is_empty()) {
        Some(id) => {
            let bad = |why| HttpError::BadExchange { exchange: id.into(), why };
            let entry = history.iter().find(|e| e.id == id).ok_or_else(|| bad("not in History — it may have been dropped as old; send again"))?;
            if !ours(&entry) {
                return Err(bad("not a send of this chat's"));
            }
            if entry.request_id.as_deref() != Some(request) {
                return Err(bad("a send of another request"));
            }
            entry
        }
        None => history.iter().rev().filter(ours).find(|e| e.request_id.as_deref() == Some(request)).ok_or_else(|| HttpError::NoExchange(request.into()))?,
    };
    if !matches!(found.outcome, Outcome::Response { .. }) {
        return Err(HttpError::BadExchange { exchange: found.id.clone(), why: "it got no response; send again" });
    }
    Ok(found)
}

/// Where a send has `•••` in place of what went or came: the request's address,
/// headers by name and body; the response's headers by name and body — a JSON body
/// by the path of each string, `$.data.token`.
pub fn masked_places(request: &Outgoing, outcome: &Outcome, body: Option<&str>) -> Vec<String> {
    let has = |text: &str| text.contains(SECRET_MASK);
    let mut places = Vec::new();
    if has(&request.url) {
        places.push("the request's address".to_string());
    }
    places.extend(request.headers.iter().filter(|(_, v)| has(v)).map(|(k, _)| format!("the request's header {k}")));
    if request.body.as_deref().is_some_and(has) {
        places.push("the request's body".into());
    }
    if let Outcome::Response { headers, .. } = outcome {
        places.extend(headers.iter().filter(|(_, v)| has(v)).map(|(k, _)| format!("header {k}")));
    }
    if let Some(body) = body.filter(|b| has(b)) {
        match serde_json::from_str::<Value>(body) {
            Ok(json) => json_paths(&json, "$".into(), &mut places),
            Err(_) => places.push(format!("the body, {} times", body.matches(SECRET_MASK).count())),
        }
    }
    places
}

fn json_paths(value: &Value, path: String, out: &mut Vec<String>) {
    match value {
        Value::String(text) if text.contains(SECRET_MASK) => out.push(path),
        Value::Array(items) => items.iter().enumerate().for_each(|(i, v)| json_paths(v, format!("{path}[{i}]"), out)),
        Value::Object(fields) => fields.iter().for_each(|(k, v)| json_paths(v, format!("{path}.{k}"), out)),
        _ => {}
    }
}

/// What a pre-request changed of the request: the method, the address, each header by
/// name in any case, the body.
pub fn request_changes(before: &Outgoing, after: &Outgoing) -> Vec<RequestChange> {
    let change = |part: String, before: Option<String>, after: Option<String>| (before != after).then_some(RequestChange { part, before, after });
    let header = |request: &Outgoing, name: &str| {
        let values: Vec<&str> = request.headers.iter().filter(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str()).collect();
        (!values.is_empty()).then(|| values.join(", "))
    };
    let mut names: Vec<&str> = Vec::new();
    for (name, _) in before.headers.iter().chain(&after.headers) {
        if !names.iter().any(|n| n.eq_ignore_ascii_case(name)) {
            names.push(name);
        }
    }
    let mut changed: Vec<RequestChange> = Vec::new();
    changed.extend(change("method".into(), Some(before.method.clone()), Some(after.method.clone())));
    changed.extend(change("url".into(), Some(before.url.clone()), Some(after.url.clone())));
    changed.extend(names.iter().filter_map(|n| change(format!("header {n}"), header(before, n), header(after, n))));
    changed.extend(change("body".into(), before.body.clone(), after.body.clone()));
    changed
}

/// The variables a script would have written, by scope; `pm.variables` is the send's
/// own and goes with it, so it is not among them.
pub fn draft_writes(before: &ScriptVars, after: &ScriptVars) -> Vec<DraftWrite> {
    let scopes = [("global", &before.globals, &after.globals), ("collection", &before.collection, &after.collection), ("environment", &before.environment, &after.environment)];
    scopes.into_iter().flat_map(|(scope, b, a)| changes(b, a).into_iter().map(move |(key, value)| DraftWrite { scope: scope.into(), key, value })).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::http_client::Variable;

    fn entry(id: &str, chat: Option<&str>, request: &str, outcome: Outcome) -> HistoryEntry {
        HistoryEntry {
            id: id.into(),
            at: 1,
            environment: Some("Stage".into()),
            agent_chat: chat.map(|c| format!("title of {c}")),
            agent_chat_id: chat.map(String::from),
            template_url: "{{baseUrl}}/orders".into(),
            request_id: Some(request.into()),
            request: Outgoing { method: "GET".into(), url: "https://shop.test/orders".into(), headers: vec![], body: None, digest: None },
            outcome,
            response_body: Some("{}".into()),
        }
    }

    fn ok() -> Outcome {
        Outcome::Response { status: 200, headers: vec![], url: "https://shop.test/orders".into(), redirects: vec![], elapsed_ms: 5 }
    }

    #[test]
    fn a_draft_runs_on_this_chats_last_send_of_the_request() {
        let history = [entry("e1", Some("c1"), "r1", ok()), entry("e2", Some("c1"), "r1", ok()), entry("e3", Some("c1"), "r2", ok()), entry("e4", Some("c2"), "r1", ok()), entry("e5", None, "r1", ok())];
        assert_eq!(fingerprint(&history, "c1", "r1", None).unwrap().id, "e2");
        assert_eq!(fingerprint(&history, "c1", "r1", Some(" e1 ")).unwrap().id, "e1");
        assert_eq!(fingerprint(&history, "c1", "r1", Some("")).unwrap().id, "e2", "an empty one is none");
        // Another chat's, the page's, another request's, one gone: refused, saying which.
        let refused = |exchange: Option<&str>| fingerprint(&history, "c1", "r1", exchange).unwrap_err().to_string();
        assert!(refused(Some("e4")).contains("not a send of this chat's"), "{}", refused(Some("e4")));
        assert!(refused(Some("e5")).contains("not a send of this chat's"));
        assert!(refused(Some("e3")).contains("another request"));
        assert!(refused(Some("e9")).contains("not in History"));
        assert!(matches!(fingerprint(&history, "c3", "r1", None), Err(HttpError::NoExchange(r)) if r == "r1"));
        // An entry from before chat ids: its title alone is not taken.
        let old = [HistoryEntry { agent_chat_id: None, ..entry("e1", Some("c1"), "r1", ok()) }];
        assert!(matches!(fingerprint(&old, "c1", "r1", None), Err(HttpError::NoExchange(_))));
    }

    #[test]
    fn a_send_with_no_response_is_no_fingerprint() {
        let history = [entry("e1", Some("c1"), "r1", ok()), entry("e2", Some("c1"), "r1", Outcome::Failed { error: "refused".into() })];
        let refused = fingerprint(&history, "c1", "r1", None).unwrap_err().to_string();
        assert!(refused.contains("e2") && refused.contains("no response"), "{refused}");
    }

    #[test]
    fn the_masked_places_are_named() {
        let request = Outgoing { method: "POST".into(), url: "https://a/x?key=•••".into(), headers: vec![("Authorization".into(), "Bearer •••".into()), ("Accept".into(), "*/*".into())], body: Some("{\"p\":\"•••\"}".into()), digest: None };
        let outcome = Outcome::Response { status: 200, headers: vec![("X-Token".into(), "•••".into()), ("Content-Type".into(), "application/json".into())], url: String::new(), redirects: vec![], elapsed_ms: 1 };
        let body = r#"{"data": {"token": "•••", "id": 7, "list": ["a", "x •••"]}}"#;
        assert_eq!(
            masked_places(&request, &outcome, Some(body)),
            ["the request's address", "the request's header Authorization", "the request's body", "header X-Token", "$.data.token", "$.data.list[1]"]
        );
        assert_eq!(masked_places(&request, &outcome, Some("token=••• and •••")).last().unwrap(), "the body, 2 times");
        let plain = Outgoing { url: "https://a/x".into(), headers: vec![], body: None, ..request };
        assert!(masked_places(&plain, &Outcome::Failed { error: String::new() }, Some("{}")).is_empty());
    }

    #[test]
    fn the_changes_to_a_request_are_by_part_and_header_name() {
        let out = |method: &str, url: &str, headers: &[(&str, &str)], body: Option<&str>| Outgoing {
            method: method.into(),
            url: url.into(),
            headers: headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            body: body.map(String::from),
            digest: None,
        };
        let before = out("GET", "https://a/x", &[("Accept", "*/*"), ("X-Old", "1")], None);
        let after = out("POST", "https://a/x?ts=5", &[("accept", "*/*"), ("X-Sig", "abc"), ("X-Sig", "def")], Some("{}"));
        let changed: Vec<(String, Option<String>, Option<String>)> = request_changes(&before, &after).into_iter().map(|c| (c.part, c.before, c.after)).collect();
        let s = |t: &str| Some(t.to_string());
        assert_eq!(
            changed,
            [
                ("method".into(), s("GET"), s("POST")),
                ("url".into(), s("https://a/x"), s("https://a/x?ts=5")),
                ("header X-Old".into(), s("1"), None),
                ("header X-Sig".into(), None, s("abc, def")),
                ("body".into(), None, s("{}")),
            ]
        );
        assert!(request_changes(&before, &before).is_empty());
    }

    #[test]
    fn writes_are_by_scope_and_the_sends_own_are_not_among_them() {
        let var = |k: &str, v: &str| Variable { key: k.into(), value: v.into(), secret: false, enabled: true };
        let before = ScriptVars { environment: vec![var("orderId", "1"), var("gone", "x")], ..Default::default() };
        let after = ScriptVars { globals: vec![var("g", "2")], collection: vec![var("c", "3")], environment: vec![var("orderId", "42")], local: vec![var("l", "4")] };
        let writes: Vec<(String, String, Option<String>)> = draft_writes(&before, &after).into_iter().map(|w| (w.scope, w.key, w.value)).collect();
        assert_eq!(
            writes,
            [
                ("global".into(), "g".into(), Some("2".into())),
                ("collection".into(), "c".into(), Some("3".into())),
                ("environment".into(), "gone".into(), None),
                ("environment".into(), "orderId".into(), Some("42".into())),
            ]
        );
    }

    #[test]
    fn what_is_given_is_masked_and_the_hidden_names_are_said() {
        let secret = |k: &str| Variable { key: k.into(), value: SECRET_MASK.into(), secret: true, enabled: true };
        let plain = |k: &str| Variable { key: k.into(), value: "1".into(), secret: false, enabled: true };
        let vars = ScriptVars { globals: vec![secret("g")], collection: vec![secret("c")], environment: vec![secret("token"), secret("id"), plain("baseUrl")], ..Default::default() };
        let code = "pm.environment.get('token'); x[\"g\"]; `c`; {{id}}; id; baseUrl";
        assert_eq!(hidden_named(&vars, code), ["g", "c", "token", "id"]);
        assert!(hidden_named(&vars, "pm.variables.get('other'); token").is_empty(), "a bare word is any word");

        let scope = Scope::new(&[]).keeping(["k3y-1234"]);
        let mut request = serde_json::json!({"header": [{"key": "X-Key", "value": "k3y-1234"}], "url": "https://a/x?k=k3y-1234", "n": 5});
        masked_strings(&mut request, &scope);
        assert_eq!(request, serde_json::json!({"header": [{"key": "X-Key", "value": "•••"}], "url": "https://a/x?k=•••", "n": 5}));

    }

    #[test]
    fn a_draft_takes_the_place_of_the_chats_last_for_that_script() {
        let draft = |request: &str, listen: &str, chat: &str, code: &str| ScriptDraft {
            collection: "c".into(),
            request: request.into(),
            name: String::new(),
            listen: listen.into(),
            chat: chat.into(),
            chat_title: String::new(),
            environment: String::new(),
            code: code.into(),
            at: 0,
            report: DraftReport::default(),
        };
        let mut drafts = vec![draft("r1", "test", "c1", "a"), draft("r1", "prerequest", "c1", "b"), draft("r1", "test", "c2", "c"), draft("r2", "test", "c1", "d")];
        keep_draft(&mut drafts, draft("r1", "test", "c1", "e"));
        assert_eq!(drafts.iter().map(|d| d.code.as_str()).collect::<String>(), "bcde");
        for i in 0..DRAFTS {
            keep_draft(&mut drafts, draft(&format!("n{i}"), "test", "c1", "x"));
        }
        assert_eq!(drafts.len(), DRAFTS);
        assert_eq!(drafts[0].request, "n0", "the oldest went first");
    }
}
