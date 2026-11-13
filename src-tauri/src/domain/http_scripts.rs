//! A collection's scripts (`docs/45-http-client.md`, decision 8): what they are
//! given, what they change and what they report. `infra::postman_sandbox` runs them;
//! this is the JSON on either side of it.

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::http_client::{Outgoing, Scope, Variable};

/// `pm.test` and `tests[…]`, as the Tests tab shows them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestResult {
    pub name: String,
    pub passed: bool,
    #[serde(default)]
    pub skipped: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsoleLine {
    pub level: String,
    pub text: String,
}

/// What a send's scripts said; kept with the response, so a saved one shows it too.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ScriptReport {
    /// The scripts that did not run because they are off (HC-11a), by when and where:
    /// `pre-request of folder "Admin"`.
    pub off: Vec<String>,
    pub tests: Vec<TestResult>,
    pub console: Vec<ConsoleLine>,
    /// A test script that threw or ran out of time. The response stands.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// A variable was written: the window reads them again. Of this send only.
    #[serde(skip_deserializing)]
    pub changed: bool,
}

impl ScriptReport {
    pub fn is_empty(&self) -> bool {
        *self == ScriptReport::default()
    }

    /// Nothing of a secret's value stays in what is shown and stored.
    pub fn hidden(mut self, scope: &Scope) -> ScriptReport {
        for test in &mut self.tests {
            test.name = scope.hide(&test.name);
            test.error = test.error.as_deref().map(|e| scope.hide(e));
        }
        for line in &mut self.console {
            line.text = scope.hide(&line.text);
        }
        self.error = self.error.as_deref().map(|e| scope.hide(e));
        self
    }
}

/// A variable written, or gone when `None`.
pub type Change = (String, Option<String>);

/// What became of `before`, for the store to apply: two sends at once each write
/// what they changed, not the whole list as they read it.
pub fn changes(before: &[Variable], after: &[Variable]) -> Vec<Change> {
    let gone = before.iter().filter(|b| !after.iter().any(|a| a.key == b.key)).map(|b| (b.key.clone(), None));
    let written = after.iter().filter(|a| !before.iter().any(|b| b.key == a.key && b.value == a.value)).map(|a| (a.key.clone(), Some(a.value.clone())));
    gone.chain(written).collect()
}

/// `changes` over a stored list: a row there keeps whether it is secret and on.
pub fn apply(variables: &mut Vec<Variable>, changes: &[Change]) {
    for (key, value) in changes {
        match (value, variables.iter().position(|v| v.key == *key)) {
            (Some(value), Some(at)) => variables[at].value = value.clone(),
            (Some(value), None) => variables.push(Variable { key: key.clone(), value: value.clone(), secret: false, enabled: true }),
            (None, Some(at)) => {
                variables.remove(at);
            }
            (None, None) => {}
        }
    }
}

/// `values`, what scripts last wrote, over a collection's own variables.
pub fn overlaid(mut variables: Vec<Variable>, values: &[Variable]) -> Vec<Variable> {
    let written: Vec<Change> = values.iter().map(|v| (v.key.clone(), Some(v.value.clone()))).collect();
    apply(&mut variables, &written);
    variables
}

/// The variables of one send, as scripts see and change them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScriptVars {
    pub globals: Vec<Variable>,
    pub collection: Vec<Variable>,
    pub environment: Vec<Variable>,
    /// `pm.variables.set`: this send's only, and the strongest.
    pub local: Vec<Variable>,
}

impl ScriptVars {
    pub fn scope(&self) -> Scope {
        Scope::new(&[&self.globals, &self.collection, &self.environment, &self.local])
    }

    /// Every secret of these, old and new: a `Scope` keeps one value a key, and a
    /// script can leave a key holding something else.
    pub fn mask(all: &[&ScriptVars]) -> Scope {
        let layers = all.iter().flat_map(|v| [&v.globals, &v.collection, &v.environment, &v.local]).flatten();
        let secrets: Vec<Variable> = layers.enumerate().map(|(at, v)| Variable { key: at.to_string(), enabled: true, ..v.clone() }).collect();
        Scope::new(&[&secrets[..]])
    }

    /// What `execute` takes as its context, `request` and `response` beside the scopes.
    pub fn context(&self, request: &Value, response: Option<&Value>) -> Value {
        let scope = |variables: &[Variable]| {
            let values: Vec<Value> = variables.iter().filter(|v| v.enabled).map(|v| json!({"key": v.key, "value": v.value, "type": "any"})).collect();
            json!({ "values": values })
        };
        let mut context = json!({
            "globals": scope(&self.globals),
            "collectionVariables": scope(&self.collection),
            "environment": scope(&self.environment),
            "_variables": scope(&self.local),
            "request": request,
        });
        if let Some(response) = response {
            context["response"] = response.clone();
        }
        context
    }

    /// The scopes as a script left them; a row turned off was never shown to it, and stays.
    pub fn absorb(&mut self, result: &Value) {
        let read = |old: &mut Vec<Variable>, name: &str| {
            let Some(values) = result.get(name).and_then(|s| s.get("values")).and_then(Value::as_array) else { return };
            let off = old.iter().filter(|v| !v.enabled).cloned();
            let on = values.iter().filter_map(|row| {
                let key = row.get("key")?.as_str()?.to_string();
                let secret = old.iter().any(|v| v.key == key && v.secret);
                Some(Variable { value: super::postman::text_of(row.get("value")), key, secret, enabled: true })
            });
            *old = off.chain(on).collect::<Vec<_>>();
        };
        read(&mut self.globals, "globals");
        read(&mut self.collection, "collectionVariables");
        read(&mut self.environment, "environment");
        read(&mut self.local, "_variables");
    }
}

/// An address out of the parts `postman-collection` keeps it in, as its `Url` writes it.
fn url_text(url: &Value) -> String {
    let Some(parts) = url.as_object() else { return super::postman::text_of(Some(url)) };
    let joined = |key: &str, by: &str| match parts.get(key) {
        Some(Value::Array(list)) => list.iter().map(|p| super::postman::text_of(Some(p))).collect::<Vec<_>>().join(by),
        other => super::postman::text_of(other),
    };
    let mut text = String::new();
    if let Some(protocol) = parts.get("protocol").and_then(Value::as_str) {
        text += protocol;
        text += "://";
    }
    if let Some(auth) = parts.get("auth").filter(|a| a.is_object()) {
        text += &super::postman::text_of(auth.get("user"));
        if let Some(password) = auth.get("password").filter(|p| !p.is_null()) {
            text += ":";
            text += &super::postman::text_of(Some(password));
        }
        text += "@";
    }
    text += &joined("host", ".");
    if let Some(port) = parts.get("port").and_then(Value::as_str) {
        text += ":";
        text += port;
    }
    if parts.get("path").and_then(Value::as_array).is_some_and(|p| !p.is_empty()) {
        text += "/";
        text += &joined("path", "/");
    }
    let query: Vec<String> = parts
        .get("query")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|q| q.get("disabled") != Some(&Value::Bool(true)))
        .map(|q| {
            let key = super::postman::text_of(q.get("key"));
            match q.get("value") {
                None | Some(Value::Null) => key,
                value => format!("{key}={}", super::postman::text_of(value)),
            }
        })
        .collect();
    if !query.is_empty() {
        text += "?";
        text += &query.join("&");
    }
    if let Some(hash) = parts.get("hash").and_then(Value::as_str) {
        text += "#";
        text += hash;
    }
    text
}

/// A stored `request` as a script gets it: its address as the text it is written in,
/// which is what `postman-collection` reads — an object with `raw` alone reads as empty.
pub fn request_for_script(request: &Value) -> Value {
    let mut request = request.clone();
    if let Some(raw) = request.get("url").and_then(|u| u.get("raw")).cloned() {
        request["url"] = raw;
    }
    request
}

/// The `request` a pre-request script left, over the stored one it came from: its
/// address as text again, and what the sandbox does not hand back kept.
pub fn request_after(before: &Value, after: &Value) -> Value {
    let mut request = match before {
        Value::Object(fields) => fields.clone(),
        _ => Map::new(),
    };
    for key in ["method", "header", "auth"] {
        if let Some(value) = after.get(key) {
            request.insert(key.into(), value.clone());
        }
    }
    match after.get("body") {
        Some(body) => {
            request.insert("body".into(), body.clone());
        }
        None => {
            request.remove("body");
        }
    }
    if let Some(url) = after.get("url") {
        let raw = url_text(url);
        // The stored `variable` rows fill `:name` path segments, which stay in the text.
        let url = match before.get("url").and_then(|u| u.get("variable")) {
            Some(variable) => json!({"raw": raw, "variable": variable}),
            None => Value::String(raw),
        };
        request.insert("url".into(), url);
    }
    Value::Object(request)
}

/// The request as it left, for a test script's `pm.request`.
pub fn request_sent(sent: &Outgoing) -> Value {
    let header: Vec<Value> = sent.headers.iter().map(|(key, value)| json!({"key": key, "value": value})).collect();
    let mut request = json!({"method": sent.method, "url": sent.url, "header": header});
    if let Some(body) = &sent.body {
        request["body"] = json!({"mode": "raw", "raw": body});
    }
    request
}

/// `pm.response`, or what a `pm.sendRequest` callback gets.
pub fn response_for(status: u16, headers: &[(String, String)], body: &[u8], elapsed_ms: u64) -> Value {
    let header: Vec<Value> = headers.iter().map(|(key, value)| json!({"key": key, "value": value})).collect();
    json!({"code": status, "header": header, "body": String::from_utf8_lossy(body), "responseTime": elapsed_ms, "responseSize": body.len()})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn var(key: &str, value: &str) -> Variable {
        Variable { key: key.into(), value: value.into(), secret: false, enabled: true }
    }

    #[test]
    fn the_mask_holds_every_secret_of_every_layer_old_and_new() {
        let secret = |value: &str| Variable { secret: true, ..var("token", value) };
        let before = ScriptVars { environment: vec![secret("old-one")], ..Default::default() };
        let after = ScriptVars { environment: vec![secret("new-one")], local: vec![var("token", "plain")], ..Default::default() };
        let off = ScriptVars { globals: vec![Variable { enabled: false, ..secret("off-one") }], ..Default::default() };
        assert_eq!(ScriptVars::mask(&[&before, &after, &off]).hide("old-one new-one off-one plain"), "••• ••• ••• plain");
    }

    #[test]
    fn an_address_keeps_its_login() {
        let parts = json!({"protocol": "http", "auth": {"user": "u", "password": "p"}, "host": ["a", "test"], "port": "8080", "path": ["x"]});
        assert_eq!(url_text(&parts), "http://u:p@a.test:8080/x");
        assert_eq!(url_text(&json!({"protocol": "http", "auth": {"user": "u"}, "host": ["a"]})), "http://u@a");
    }

    #[test]
    fn only_what_changed_is_written_and_a_stored_row_keeps_what_it_is() {
        let before = [var("same", "1"), var("edited", "2"), var("gone", "3")];
        let after = [var("same", "1"), var("edited", "two"), var("new", "4")];
        let changed = changes(&before, &after);
        assert_eq!(changed, vec![("gone".to_string(), None), ("edited".to_string(), Some("two".to_string())), ("new".to_string(), Some("4".to_string()))]);

        let mut stored = vec![Variable { secret: true, enabled: false, ..var("edited", "2") }, var("gone", "3"), var("other", "kept")];
        apply(&mut stored, &changed);
        assert_eq!(stored, vec![Variable { key: "edited".into(), value: "two".into(), secret: true, enabled: false }, var("other", "kept"), var("new", "4")]);
    }

    #[test]
    fn what_scripts_wrote_lies_over_a_collections_variables() {
        let mine = Variable { secret: true, ..var("token", "") };
        let shown = overlaid(vec![mine, var("host", "a")], &[var("token", "t-1"), var("late", "b")]);
        assert_eq!(shown, vec![Variable { key: "token".into(), value: "t-1".into(), secret: true, enabled: true }, var("host", "a"), var("late", "b")]);
    }

    #[test]
    fn a_script_sees_what_is_on_and_leaves_what_is_off_alone() {
        let mut vars = ScriptVars {
            environment: vec![Variable { secret: true, ..var("token", "t") }, Variable { enabled: false, ..var("off", "x") }],
            ..ScriptVars::default()
        };
        let context = vars.context(&json!("https://a.test"), Some(&json!({"code": 200})));
        assert_eq!(context["environment"]["values"], json!([{"key": "token", "value": "t", "type": "any"}]));
        assert_eq!(context["response"]["code"], 200);

        vars.absorb(&json!({"environment": {"values": [{"key": "token", "value": "t2"}, {"key": "n", "value": 5}]}, "_variables": {"values": [{"key": "l", "value": "1"}]}}));
        assert_eq!(
            vars.environment,
            vec![Variable { enabled: false, ..var("off", "x") }, Variable { key: "token".into(), value: "t2".into(), secret: true, enabled: true }, var("n", "5")]
        );
        assert_eq!(vars.local, vec![var("l", "1")]);
        assert_eq!(vars.scope().fill("{{l}}").unwrap().sent, "1", "this send's own are in its scope");
    }

    #[test]
    fn an_address_comes_back_as_text_with_what_is_off_left_out() {
        let url = json!({"protocol": "https", "host": ["api", "test"], "port": "8443", "path": ["v1", ":id"],
            "query": [{"key": "a", "value": "1"}, {"key": "flag", "value": null}, {"key": "off", "value": "x", "disabled": true}], "hash": "top"});
        assert_eq!(url_text(&url), "https://api.test:8443/v1/:id?a=1&flag#top");
        assert_eq!(url_text(&json!({"host": ["{{baseUrl}}"], "path": []})), "{{baseUrl}}");
        assert_eq!(url_text(&json!("{{baseUrl}}/x")), "{{baseUrl}}/x");
    }

    #[test]
    fn a_request_a_script_left_keeps_what_it_was_not_given() {
        let before = json!({"description": "kept", "method": "POST", "url": {"raw": "http://a.test/:id", "variable": [{"key": "id", "value": "7"}]}, "body": {"mode": "raw", "raw": "{}"}});
        assert_eq!(request_for_script(&before)["url"], "http://a.test/:id");
        let after = json!({"method": "PUT", "url": {"protocol": "http", "host": ["a", "test"], "path": [":id"], "variable": [{"key": "id"}]}, "header": [{"key": "X-A", "value": "1"}]});
        assert_eq!(
            request_after(&before, &after),
            json!({"description": "kept", "method": "PUT", "url": {"raw": "http://a.test/:id", "variable": [{"key": "id", "value": "7"}]}, "header": [{"key": "X-A", "value": "1"}]})
        );
        assert_eq!(request_after(&json!("http://a.test"), &json!({"url": {"host": ["a"]}})), json!({"url": "a"}), "no path variables, plain text");
        assert_eq!(request_for_script(&json!("http://a.test")), json!("http://a.test"), "a request written as its address alone");
    }

    #[test]
    fn a_report_keeps_no_secret_and_an_empty_one_is_not_stored() {
        assert!(ScriptReport::default().is_empty());
        let scope = Scope::new(&[&[Variable { secret: true, ..var("t", "s3cret") }]]);
        let report = ScriptReport {
            tests: vec![TestResult { name: "got s3cret".into(), passed: false, skipped: false, error: Some("expected s3cret".into()) }],
            console: vec![ConsoleLine { level: "log".into(), text: "s3cret".into() }],
            error: Some("threw s3cret".into()),
            ..ScriptReport::default()
        }
        .hidden(&scope);
        assert!(!serde_json::to_string(&report).unwrap().contains("s3cret"));
    }

    #[test]
    fn a_saved_report_says_nothing_of_a_send_long_over() {
        let saved = serde_json::to_string(&ScriptReport { changed: true, ..ScriptReport::default() }).unwrap();
        assert!(serde_json::from_str::<ScriptReport>(&saved).unwrap().is_empty());
    }
}
