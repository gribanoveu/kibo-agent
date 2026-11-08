//! The HTTP client's rules (`docs/45-http-client.md`): variables and their
//! `{{name}}`, an environment and what the window sees of it, a request as it
//! leaves, a JSON body's comments, a pasted cURL, and what the agent may send
//! into an environment and read back. No I/O — `postman.rs` reads collections.

use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, LazyLock};

use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
use regex::Regex;
use super::http_scripts::ScriptReport;
use super::postman::Collection;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use thiserror::Error;
use url::Url;

/// What stands in a secret's place wherever it is shown.
pub const SECRET_MASK: &str = "•••";

#[derive(Debug, Error, PartialEq, Eq)]
pub enum HttpError {
    #[error("not a Postman collection v2.1: {0}")]
    NotACollection(String),
    #[error("no request or folder {0} in the collection")]
    NoSuchItem(String),
    #[error("not found in the collection, the environment or globals: {}", .0.join(", "))]
    UnknownVariables(Vec<String>),
    #[error("{{{{{0}}}}} is a Postman dynamic variable not supported here")]
    UnsupportedDynamic(String),
    #[error("{{{{{0}}}}} refers to itself, or nests variables more than 10 deep")]
    VariableCycle(String),
    #[error("the variables fill in to more than 10 MB")]
    TooLarge,
    #[error("{0} came back masked with no saved value to keep — type its value again")]
    MaskWithoutValue(String),
    #[error("{0} auth is kept from Postman and not sent yet")]
    UnsupportedAuth(String),
    #[error("OAuth 2.0: {0}")]
    OAuth2(String),
    #[error("{0}")]
    Script(String),
    #[error("a {0} body is kept from Postman and not sent yet")]
    UnsupportedBody(String),
    #[error("{0} is not an http or https address once filled in")]
    NotAnAddress(String),
    #[error("{url} goes to {origin}, and no request in the user's collections goes there with \"{environment}\" (they go to: {trusted}) — ask the user to add a request for it on the HTTP page, or give them a curl command to run themselves")]
    Untrusted { url: String, origin: String, environment: String, trusted: String },
    #[error("the host is the url's — send no Host header")]
    HostHeader,
    #[error("not a cURL command Kibo reads: {0}")]
    BadCurl(String),
    #[error("no environment {0}")]
    NoEnvironment(String),
    #[error("no collection {0}")]
    NoCollection(String),
    #[error("the certificate: {0}")]
    BadCertificate(String),
    #[error("{0} is not an id Kibo keeps a file under")]
    BadId(String),
    #[error("{0}")]
    Store(String),
    #[error("no environment is open to the agent — set its agent access on the HTTP page")]
    NoneOpen,
    #[error("no environment \"{name}\" is open to the agent: {open}")]
    NotOpen { name: String, open: String },
    #[error("say which `environment`: {0}")]
    WhichEnvironment(String),
    #[error("{count} environments are named \"{name}\" — rename one on the HTTP page")]
    SameName { name: String, count: usize },
    #[error("\"{environment}\" is read only for the agent: {method} is refused — the user can change its agent access on the HTTP page")]
    ReadOnly { environment: String, method: String },
    #[error("{} not in \"{environment}\", which has: {known}", .names.join(", "))]
    UnknownIn { names: Vec<String>, environment: String, known: String },
    #[error("the request failed: {0}")]
    Unanswered(String),
}

/// A variable of an environment, a collection or globals — Postman's `{key, value}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Variable {
    pub key: String,
    pub value: String,
    #[serde(default)]
    pub secret: bool,
    #[serde(default = "on")]
    pub enabled: bool,
}

impl Variable {
    /// What the window gets: a secret's value never — the mask if it has one, "" if not.
    pub fn for_window(&self) -> Variable {
        let value = if self.secret && !self.value.is_empty() { SECRET_MASK.into() } else { self.value.clone() };
        Variable { value, ..self.clone() }
    }
}

fn on() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Environment {
    pub id: String,
    pub name: String,
    pub variables: Vec<Variable>,
    /// PEM of the server's own certificate, or its CA's: a stand on a self-signed
    /// certificate is trusted this way, never by turning the check off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trusted_cert: Option<String>,
    #[serde(default, skip_serializing_if = "AgentAccess::is_off")]
    pub agent: AgentAccess,
}

/// How far the agent may go in an environment (decision 6): nowhere, until the
/// user opens it. A file imported or from before this field is off. A read
/// never asks; what a write does is the mode's (`ApprovalPolicy::stand_asks`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentAccess {
    #[default]
    Off,
    ReadOnly,
    /// A write asks every time, even under "Always allow" and in Auto.
    AskForWrites,
    /// A write asks as another changing tool does.
    WritesAsOtherTools,
}

/// GET, HEAD and OPTIONS: what reads, by RFC — what a Read only environment takes.
pub fn is_read(method: &str) -> bool {
    matches!(method.trim().to_ascii_uppercase().as_str(), "GET" | "HEAD" | "OPTIONS")
}

impl AgentAccess {
    pub fn is_off(&self) -> bool {
        *self == AgentAccess::Off
    }
}

/// An environment the agent may name, and how far it may go there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenStand {
    pub id: String,
    pub name: String,
    pub access: AgentAccess,
}

/// What the model asked for, its `{{name}}` still written out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
}

/// What the user's collections make of one environment for the agent (HC-7a):
/// where it may send, and which variables it may not read. The requests are the
/// user's, written by hand or imported — not the model's — so where they go is trusted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reach {
    /// `scheme://host[:port]` of a request, filled in with the environment.
    pub origins: BTreeSet<String>,
    /// Names a collection writes where a credential goes.
    pub credentials: BTreeSet<String>,
    /// `Collection / Request: METHOD url`, the url as written, without its query.
    pub requests: Vec<String>,
}

impl Reach {
    /// `collections` each with its variables as a send sees them, the layers of the page.
    pub fn new(collections: &[(&Collection, Vec<Variable>)], globals: &[Variable], environment: &Environment) -> Reach {
        let credentials: BTreeSet<String> = collections.iter().flat_map(|(c, _)| c.credential_names()).collect();
        let mut reach = Reach { credentials, ..Reach::default() };
        let mine = environment.for_agent(&reach.credentials);
        let own: BTreeSet<&str> = mine.iter().filter(|v| v.enabled).map(|v| v.key.as_str()).collect();
        for (collection, variables) in collections {
            let theirs = hiding(variables, &reach.credentials);
            let scope = Scope::new(&[globals, &theirs, &mine]);
            for listed in collection.requests() {
                // An address with a secret in it could not be named to the model.
                if let Ok(filled) = scope.fill(&listed.url) {
                    if let Some(origin) = origin_of(&filled.sent).filter(|o| origin_of(&filled.shown).as_ref() == Some(o)) {
                        reach.origins.insert(origin);
                    }
                }
                // The agent fills in the environment's names only: the rest is written out
                // here, where it is no secret, so a request listed is one it can send.
                let others: Vec<String> = names_in(&listed.url).filter(|n| !n.starts_with('$') && !own.contains(n)).map(String::from).collect();
                let url = others.iter().fold(listed.url.clone(), |url, name| {
                    let written = format!("{{{{{name}}}}}");
                    match scope.fill(&written) {
                        Ok(filled) if filled.sent == filled.shown => url.replace(&written, &filled.sent),
                        _ => url,
                    }
                });
                let url = url.split('?').next().unwrap_or_default();
                reach.requests.push(format!("{} / {}: {} {url}", collection.name(), listed.name, listed.method));
            }
        }
        reach
    }
}

/// An environment as the agent's description gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandView {
    pub name: String,
    pub access: AgentAccess,
    pub origins: Vec<String>,
    /// Each variable on, with its value unless it is hidden.
    pub variables: Vec<(String, Option<String>)>,
    pub requests: Vec<String>,
}

/// A header, a field or a variable a credential goes in, by its name: its value is
/// never the model's to read. One named as an address is not — `authUrl`, `tokenHost`.
pub fn is_credential_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    !is_address_name(&name) && ["auth", "token", "key", "secret", "session", "password", "cookie"].iter().any(|word| name.contains(word))
}

/// `accessTokenUrl`, `redirect_uri`, `authHost`: where a credential goes, not one.
pub fn is_address_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    ["url", "uri", "host", "domain", "endpoint"].iter().any(|end| name.ends_with(end))
}

/// The `{{name}}`s in `text`.
pub fn names_in(text: &str) -> impl Iterator<Item = &str> {
    VARIABLE.captures_iter(text).filter_map(|m| m.get(1)).map(|m| m.as_str())
}

/// `http://` before an address with no scheme, as Postman takes `localhost:3000/x`.
/// A scheme is letters before the first `://` — not the `://` of an address in its query.
pub fn with_scheme(url: &str) -> Cow<'_, str> {
    let scheme = url.split_once("://").is_some_and(|(s, _)| s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')));
    if scheme { Cow::Borrowed(url) } else { Cow::Owned(format!("http://{url}")) }
}

/// An address's `scheme://host[:port]`; none for one that is not http or https.
fn origin_of(url: &str) -> Option<String> {
    let parsed = Url::parse(&with_scheme(url)).ok().filter(|u| matches!(u.scheme(), "http" | "https"))?;
    Some(parsed.origin().ascii_serialization())
}

/// `variables` with those named in `hidden`, or named as a credential, made secrets.
fn hiding(variables: &[Variable], hidden: &BTreeSet<String>) -> Vec<Variable> {
    variables.iter().map(|v| Variable { secret: v.secret || hidden.contains(&v.key) || is_credential_name(&v.key), ..v.clone() }).collect()
}

/// What the model may read of a response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentReply {
    pub outcome: Outcome,
    /// Text with its secrets masked; bytes that are not text as they came, for their size only.
    pub body: Vec<u8>,
    /// What the stand sent, before indents and masks.
    pub size: usize,
}

impl AgentReply {
    /// With no `Set-Cookie` and every secret masked. JSON is indented before the
    /// masking: reading it undoes its escapes, and `\/` would hide a secret from it.
    pub fn new(outcome: Outcome, body: &[u8], scope: &Scope) -> AgentReply {
        let outcome = match outcome.hidden(scope) {
            Outcome::Response { status, headers, url, redirects, elapsed_ms } => {
                let headers = headers.iter().filter(|(name, _)| !name.eq_ignore_ascii_case("set-cookie")).map(|(k, v)| (scope.hide(k), scope.hide(v))).collect();
                Outcome::Response { status, headers, url, redirects, elapsed_ms }
            }
            failed => failed,
        };
        let size = body.len();
        let body = match std::str::from_utf8(body) {
            Ok(text) => {
                let indented = serde_json::from_str::<Value>(text).ok().and_then(|v| serde_json::to_string_pretty(&v).ok());
                scope.hide(indented.as_deref().unwrap_or(text)).into_bytes()
            }
            Err(_) => body.to_vec(),
        };
        AgentReply { outcome, body, size }
    }
}

/// The environments as the agent's `httpRequest` reaches them.
/// `services::http_client::Stands` over the stores; a test's own in memory.
pub trait HttpStands: Send + Sync {
    /// The ones not off: another is not named to the model, in a list or an error.
    fn open(&self) -> Result<Vec<OpenStand>, HttpError>;
    fn describe(&self, id: &str) -> Result<StandView, HttpError>;
    fn send(&self, id: &str, request: &AgentRequest) -> Result<AgentReply, HttpError>;
}

/// History gained an agent's send, for the page to read again.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct HttpChanged;

pub type HttpEventSink = Arc<dyn Fn(HttpChanged) + Send + Sync>;

impl Environment {
    /// What the window gets: a secret's value never — the mask if it has one, "" if not.
    pub fn for_window(&self) -> Environment {
        Environment { variables: self.variables.iter().map(Variable::for_window).collect(), ..self.clone() }
    }

    /// The window's edit over what is stored: a stored secret sent back empty
    /// or masked keeps its value, as the form leaves a secret untouched — even
    /// when the edit unmarks it. The mask with nothing stored under its name
    /// (a secret renamed) is refused rather than saved as the value.
    pub fn edited(&self, from_window: Environment) -> Result<Environment, HttpError> {
        let mut variables = Vec::with_capacity(from_window.variables.len());
        for mut v in from_window.variables {
            let untouched = v.value.is_empty() || v.value == SECRET_MASK;
            match self.variables.iter().find(|old| old.secret && old.key == v.key) {
                Some(old) if untouched => v.value = old.value.clone(),
                None if v.value == SECRET_MASK => return Err(HttpError::MaskWithoutValue(v.key)),
                _ => {}
            }
            variables.push(v);
        }
        Ok(Environment { variables, ..from_window })
    }

    /// Its variables as the agent has them: a credential the collections write
    /// is a secret, and so is any variable a secret's value names.
    fn for_agent(&self, credentials: &BTreeSet<String>) -> Vec<Variable> {
        let marked = self.variables.iter().filter(|v| v.secret || is_credential_name(&v.key)).map(|v| v.key.clone());
        let mut hidden: BTreeSet<String> = marked.chain(credentials.iter().cloned()).collect();
        loop {
            let named = self.variables.iter().filter(|v| hidden.contains(&v.key)).flat_map(|v| names_in(&v.value));
            let more: Vec<String> = named.filter(|n| !hidden.contains(*n)).map(String::from).collect();
            if more.is_empty() {
                return hiding(&self.variables, &hidden);
            }
            hidden.extend(more);
        }
    }

    /// What the agent is told of this environment: where it may send, the
    /// variables — a hidden one by name only — and the collections' requests.
    pub fn described(&self, reach: &Reach) -> Result<StandView, HttpError> {
        if self.agent.is_off() {
            return Err(HttpError::NoneOpen);
        }
        let mine = self.for_agent(&reach.credentials);
        let scope = Scope::new(&[&mine]);
        // A value can hold a secret's text without naming it.
        let variables = mine.iter().filter(|v| v.enabled).map(|v| (v.key.clone(), (!v.secret).then(|| scope.hide(&v.value)))).collect();
        let requests = reach.requests.iter().map(|r| scope.hide(r)).collect();
        Ok(StandView { name: self.name.clone(), access: self.agent, origins: reach.origins.iter().cloned().collect(), variables, requests })
    }

    /// The agent's request as it leaves, and the variables its reply is masked
    /// with. All that is refused is refused here, before the network: an address
    /// no request of the collections goes to does not reach even DNS.
    /// Only this environment's variables: globals are not what the user opened.
    pub fn agent_request(&self, request: &AgentRequest, reach: &Reach) -> Result<(Prepared, Scope), HttpError> {
        if self.agent.is_off() {
            return Err(HttpError::NoneOpen);
        }
        let method = request.method.trim().to_ascii_uppercase();
        if self.agent == AgentAccess::ReadOnly && !is_read(&method) {
            return Err(HttpError::ReadOnly { environment: self.name.clone(), method });
        }
        let scope = Scope::new(&[&self.for_agent(&reach.credentials)]);
        let fill = |text: &str| scope.fill(text).map_err(|e| self.unknown_named(e));
        // The messages have the url as the model wrote it: filled in, it can hold a secret.
        let filled = fill(&request.url)?;
        let url = Url::parse(&with_scheme(&filled.sent)).ok().filter(|u| matches!(u.scheme(), "http" | "https")).ok_or_else(|| HttpError::NotAnAddress(request.url.clone()))?;
        let origin = url.origin().ascii_serialization();
        if !reach.origins.contains(&origin) {
            let trusted = if reach.origins.is_empty() { "nowhere".to_string() } else { reach.origins.iter().cloned().collect::<Vec<_>>().join(", ") };
            // The host as the url crate writes it — lower case, punycode — is no longer
            // the secret's text, and a mask would not find it there.
            let origin = if origin_of(&filled.shown).as_ref() == Some(&origin) { origin } else { "a host with a hidden value in it".into() };
            return Err(HttpError::Untrusted { url: request.url.clone(), origin, environment: self.name.clone(), trusted });
        }
        let mut headers = Vec::with_capacity(request.headers.len() + 1);
        for (name, value) in &request.headers {
            if name.eq_ignore_ascii_case("host") {
                return Err(HttpError::HostHeader);
            }
            headers.push((name.clone(), value.clone(), fill(value)?));
        }
        let body = request.body.as_deref().map(fill).transpose()?;
        // Many APIs answer 415 to JSON without its type, and the model spends a step on it.
        let json = body.as_ref().is_some_and(|b| serde_json::from_str::<Value>(&b.sent).is_ok_and(|v| v.is_object() || v.is_array()));
        if json && !headers.iter().any(|(name, _, _)| name.eq_ignore_ascii_case("content-type")) {
            let plain = Filled { sent: "application/json".into(), shown: "application/json".into() };
            headers.push(("Content-Type".into(), String::new(), plain));
        }
        // A variable in a credential's header is masked whatever it says, by its last
        // word: `Bearer` stays, so history still says which scheme went. What the model
        // wrote out it knows, and a short value would mask every `1` in the reply.
        let credentials = headers
            .iter()
            .filter(|(name, written, _)| is_credential_name(name) && names_in(written).next().is_some())
            .filter_map(|(_, _, value)| value.sent.split_whitespace().last())
            .filter(|word| word.chars().count() >= 4);
        let scope = scope.keeping(credentials);
        let written = |pick: fn(&Filled) -> &String| headers.iter().map(|(k, _, v)| (k.to_string(), pick(v).clone())).collect();
        let sent = Outgoing { method, url: url.into(), headers: written(|f| &f.sent), body: body.as_ref().map(|b| b.sent.clone()), digest: None };
        let shown = Outgoing { url: scope.hide(&sent.url), headers: written(|f| &f.shown), body: body.map(|b| scope.hide(&b.shown)), ..sent.clone() };
        Ok((Prepared { sent, shown }, scope))
    }

    /// A name the model got wrong, with the names it may use: values stay home.
    fn unknown_named(&self, error: HttpError) -> HttpError {
        match error {
            HttpError::UnknownVariables(names) => {
                let known = self.variables.iter().filter(|v| v.enabled).map(|v| v.key.as_str()).collect::<Vec<_>>().join(", ");
                HttpError::UnknownIn { names, environment: self.name.clone(), known }
            }
            e => e,
        }
    }
}

/// `{{name}}`, as Postman writes a variable; the same pattern as the window's.
static VARIABLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\{\{\s*([^{}\s]+)\s*\}\}").expect("valid regex"));

/// What an address percent-encodes of a path or query, as the `url` crate does on sending.
const ADDRESS: &AsciiSet = &CONTROLS.add(b' ').add(b'"').add(b'#').add(b'<').add(b'>').add(b'`').add(b'{').add(b'}').add(b'\'');

/// A variable's value nests other variables at most this deep — Postman's own limit is 19.
const MAX_DEPTH: usize = 10;

/// Filled-in text grows no further: eight `{{next}}` in each of ten variables
/// would be gigabytes. The size of the largest response the page takes.
const MAX_FILLED: usize = 10 << 20;

/// Text filled in twice in one pass: as it is sent, and as it is shown — a
/// secret masked, a dynamic variable the same value in both.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Filled {
    pub sent: String,
    pub shown: String,
}

/// The variables a request sees, in Postman's order: globals, the collection's
/// over them, the environment over both. A variable turned off is not there.
#[derive(Debug, Default)]
pub struct Scope {
    vars: HashMap<String, Variable>,
}

impl Scope {
    /// Layers from the weakest: `[globals, collection, environment]`.
    pub fn new(layers: &[&[Variable]]) -> Scope {
        let vars = layers.iter().flat_map(|l| l.iter()).filter(|v| v.enabled).map(|v| (v.key.clone(), v.clone())).collect();
        Scope { vars }
    }

    /// `text` with every `{{name}}` filled in; all the names found nowhere in one error.
    pub fn fill(&self, text: &str) -> Result<Filled, HttpError> {
        let mut missing = Vec::new();
        let filled = self.fill_in(text, &mut Vec::new(), &mut missing)?;
        if missing.is_empty() { Ok(filled) } else { Err(HttpError::UnknownVariables(missing)) }
    }

    /// `text` with each secret's value masked — as it goes out, a `{{name}}` in it
    /// filled in, and as an address, a form or a JSON string encodes that: what comes
    /// back about a request (where it went, why it failed) is written from the
    /// address as sent. The longest first: one secret can be the start of another.
    pub fn hide(&self, text: &str) -> String {
        let secrets = self.vars.values().filter(|v| v.secret && !v.value.is_empty());
        let values = secrets.map(|v| self.fill(&v.value).map_or_else(|_| v.value.clone(), |filled| filled.sent)).filter(|v| !v.is_empty());
        let mut forms: Vec<String> = values
            .flat_map(|value| {
                let quoted = Value::from(value.as_str()).to_string();
                [utf8_percent_encode(&value, ADDRESS).to_string(), utf8_percent_encode(&value, super::postman::FORM).to_string(), quoted[1..quoted.len() - 1].to_string(), value]
            })
            .collect();
        forms.sort_by_key(|form| std::cmp::Reverse(form.len()));
        forms.iter().fold(text.to_string(), |text, form| text.replace(form, SECRET_MASK))
    }

    /// With `values` masked too, though no variable marks them.
    fn keeping<'a>(mut self, values: impl IntoIterator<Item = &'a str>) -> Scope {
        for (i, value) in values.into_iter().enumerate() {
            self.vars.insert(format!("\0kept{i}"), Variable { key: String::new(), value: value.into(), secret: true, enabled: true });
        }
        self
    }

    fn fill_in(&self, text: &str, path: &mut Vec<String>, missing: &mut Vec<String>) -> Result<Filled, HttpError> {
        let mut out = Filled::default();
        let mut at = 0;
        for m in VARIABLE.captures_iter(text) {
            let (whole, name) = (m.get(0).expect("match"), &m[1]);
            out.sent.push_str(&text[at..whole.start()]);
            out.shown.push_str(&text[at..whole.start()]);
            at = whole.end();
            let value = if let Some(dynamic) = name.strip_prefix('$') {
                let v = dynamic_value(dynamic).ok_or_else(|| HttpError::UnsupportedDynamic(name.into()))?;
                Filled { shown: v.clone(), sent: v }
            } else if let Some(var) = self.vars.get(name) {
                if path.iter().any(|p| p == name) || path.len() >= MAX_DEPTH {
                    return Err(HttpError::VariableCycle(name.into()));
                }
                path.push(name.into());
                let inner = self.fill_in(&var.value, path, missing)?;
                path.pop();
                if var.secret { Filled { sent: inner.sent, shown: SECRET_MASK.into() } } else { inner }
            } else {
                if !missing.iter().any(|n| n == name) {
                    missing.push(name.into());
                }
                Filled::default()
            };
            out.sent.push_str(&value.sent);
            out.shown.push_str(&value.shown);
            if out.sent.len().max(out.shown.len()) > MAX_FILLED {
                return Err(HttpError::TooLarge);
            }
        }
        out.sent.push_str(&text[at..]);
        out.shown.push_str(&text[at..]);
        Ok(out)
    }
}

/// Postman's dynamic variables Kibo makes; the rest of `{{$random…}}` is an error by name.
fn dynamic_value(name: &str) -> Option<String> {
    let now = chrono::Utc::now();
    Some(match name {
        "guid" | "randomUUID" => uuid::Uuid::new_v4().to_string(),
        "timestamp" => now.timestamp().to_string(),
        "isoTimestamp" => now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "randomInt" => rand::Rng::gen_range(&mut rand::thread_rng(), 0..=1000).to_string(),
        _ => return None,
    })
}

/// A request as it leaves: variables filled in, auth a header or a parameter, the body as text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Outgoing {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
    /// `digest`'s user and password, to answer a challenge with: only the sender
    /// sees them. What is shown and stored has no such field.
    #[serde(skip)]
    pub digest: Option<(String, String)>,
}

/// Where a send's cookies live, across sends and per domain.
pub trait CookieJar {
    /// The `Cookie` value for `url`, none when no cookie goes there.
    fn header_for(&self, url: &str) -> Option<String>;
    /// What `url` answered with: its `Set-Cookie` values.
    fn take(&self, url: &str, set_cookies: &[String]);
}

/// A cookie as the page lists it: its value stays in the store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CookieRow {
    pub domain: String,
    pub path: String,
    pub name: String,
    /// In milliseconds; none for one that lasts until the jar is cleared.
    pub expires_at: Option<i64>,
}

/// The request to send, and the same request to show — in Preview and history — with secrets masked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepared {
    pub sent: Outgoing,
    pub shown: Outgoing,
}

impl Prepared {
    /// With a secret's value masked wherever it stands: a script that read one and
    /// wrote it into a header wrote text, which `shown` has no way to tell from a value.
    pub fn hidden(&self, scope: &Scope) -> Prepared {
        let shown = &self.shown;
        let headers = shown.headers.iter().map(|(k, v)| (scope.hide(k), scope.hide(v))).collect();
        let shown = Outgoing { method: shown.method.clone(), url: scope.hide(&shown.url), headers, body: shown.body.as_deref().map(|b| scope.hide(b)), digest: None };
        Prepared { sent: self.sent.clone(), shown }
    }
}

/// What came back: a response, 4xx and 5xx too, or why none did. The body is apart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Outcome {
    #[serde(rename_all = "camelCase")]
    Response {
        status: u16,
        headers: Vec<(String, String)>,
        /// Where it came from, after any redirects.
        url: String,
        /// The addresses redirected through on the way, the first the request's own.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        redirects: Vec<String>,
        elapsed_ms: u64,
    },
    Failed { error: String },
}

impl Outcome {
    /// As the window and the disk may have it: the addresses and the error are
    /// the sender's, written from the request with its secrets filled in.
    pub fn hidden(self, scope: &Scope) -> Outcome {
        match self {
            Outcome::Response { status, headers, url, redirects, elapsed_ms } => {
                Outcome::Response { status, headers, url: scope.hide(&url), redirects: redirects.iter().map(|r| scope.hide(r)).collect(), elapsed_ms }
            }
            Outcome::Failed { error } => Outcome::Failed { error: scope.hide(&error) },
        }
    }
}

/// A request's last response, kept whole until the next send (decision 4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedResponse {
    /// Unix milliseconds.
    pub at: i64,
    pub environment: Option<String>,
    pub outcome: Outcome,
    /// Bytes, a picture's too; the store keeps them beside the rest, not in JSON.
    #[serde(skip)]
    pub body: Vec<u8>,
    /// What the send's scripts said; a response from before them has none.
    #[serde(default, skip_serializing_if = "ScriptReport::is_empty")]
    pub scripts: ScriptReport,
}

/// A response as the window gets it: the body as text when it is UTF-8, and its size either way.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseView {
    pub at: i64,
    pub environment: Option<String>,
    pub outcome: Outcome,
    pub body: Option<String>,
    pub size: usize,
    pub scripts: ScriptReport,
}

impl From<SavedResponse> for ResponseView {
    fn from(saved: SavedResponse) -> ResponseView {
        let size = saved.body.len();
        ResponseView { at: saved.at, environment: saved.environment, outcome: saved.outcome, body: String::from_utf8(saved.body).ok(), size, scripts: saved.scripts }
    }
}

/// History keeps this many sends, and no more than `HISTORY_BYTES` of them.
pub const HISTORY_ENTRIES: usize = 200;
pub const HISTORY_BYTES: usize = 50 << 20;
/// A body in history is cut here: the log says what happened, the request keeps the whole response.
pub const HISTORY_BODY: usize = 256 << 10;

/// One send in the log: the request as shown — a secret masked — and what came back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub id: String,
    pub at: i64,
    pub environment: Option<String>,
    /// The chat whose agent sent it; none when the page did.
    pub agent_chat: Option<String>,
    /// The address before its variables were filled in: what the list shows.
    pub template_url: String,
    /// The collection's request it was, which the entry opens; none for the agent's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    pub request: Outgoing,
    pub outcome: Outcome,
    pub response_body: Option<String>,
}

impl HistoryEntry {
    /// Built from the shown request only, so a secret's value has no way in.
    pub fn new(at: i64, environment: Option<String>, agent_chat: Option<String>, template_url: String, prepared: &Prepared, outcome: Outcome, body: &[u8]) -> HistoryEntry {
        let mut request = prepared.shown.clone();
        request.body = request.body.map(|b| loggable(b.as_bytes()));
        let response_body = matches!(outcome, Outcome::Response { .. }).then(|| loggable(body));
        HistoryEntry { id: uuid::Uuid::new_v4().to_string(), at, environment, agent_chat, template_url, request_id: None, request, outcome, response_body }
    }

    /// Near what it takes as JSON: a body with control characters is not kept
    /// (`loggable`), so an escape at most doubles a `"` or a `\`.
    fn size(&self) -> usize {
        let headers = |h: &[(String, String)]| h.iter().map(|(k, v)| k.len() + v.len()).sum::<usize>();
        let outcome = match &self.outcome {
            Outcome::Response { headers: h, url, redirects, .. } => headers(h) + url.len() + redirects.iter().map(String::len).sum::<usize>(),
            Outcome::Failed { error } => error.len(),
        };
        self.template_url.len() + self.request.url.len() + headers(&self.request.headers) + self.request.body.as_ref().map_or(0, String::len) + outcome + self.response_body.as_ref().map_or(0, String::len)
    }
}

/// The oldest sends go, oldest first, until the log holds 200 and 50 MB at most.
pub fn trim_history(entries: &mut Vec<HistoryEntry>) {
    let mut total: usize = entries.iter().map(HistoryEntry::size).sum();
    let mut drop = 0;
    while entries.len() - drop > HISTORY_ENTRIES || total > HISTORY_BYTES {
        total -= entries[drop].size();
        drop += 1;
    }
    entries.drain(..drop);
}

/// A body as the log keeps it: text cut at 256 KB; anything else — a picture,
/// protobuf, `\0` that JSON would write as six bytes — only its size.
fn loggable(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) if !text.chars().any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t')) => cut(text),
        _ => format!("binary, {} bytes", bytes.len()),
    }
}

fn cut(text: &str) -> String {
    if text.len() <= HISTORY_BODY {
        return text.to_string();
    }
    let end = (0..=HISTORY_BODY).rev().find(|&i| text.is_char_boundary(i)).unwrap_or(0);
    format!("{}\n… cut at 256.0 KB in history", &text[..end])
}

/// The text without its `//` and `/* */` comments — what Postman sends of a JSON
/// body. A comment that is a line of its own goes with its line. The same rules
/// as the window's `stripJsonComments`.
pub fn strip_json_comments(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let (mut i, mut copied) = (0, 0);
    while i < b.len() {
        let two = &b[i..(i + 2).min(b.len())];
        let comment_end = if two == b"//" {
            Some(text[i..].find('\n').map_or(b.len(), |n| i + n))
        } else if two == b"/*" {
            Some(text[i + 2..].find("*/").map_or(b.len(), |n| i + 2 + n + 2))
        } else {
            None
        };
        let Some(end) = comment_end else {
            i = piece_end(text, i).unwrap_or(i + 1);
            continue;
        };
        out.push_str(&text[copied..i]);
        copied = end;
        i = end;
        let rest = text[i..].trim_start_matches([' ', '\t']);
        let ends_line = rest.is_empty() || rest.starts_with('\n');
        let line_start = matches!(out.trim_end_matches([' ', '\t']).chars().last(), None | Some('\n'));
        if ends_line && line_start {
            out.truncate(out.trim_end_matches([' ', '\t']).len());
            i = b.len() - rest.len() + usize::from(rest.starts_with('\n'));
            copied = i;
        }
    }
    out.push_str(&text[copied..]);
    out
}

/// Past a string or a `{{variable}}` starting at `i`: their insides are not comments.
fn piece_end(text: &str, i: usize) -> Option<usize> {
    let b = text.as_bytes();
    if b[i] == b'"' {
        let mut j = i + 1;
        while j < b.len() && b[j] != b'"' {
            j += if b[j] == b'\\' { 2 } else { 1 };
        }
        return Some((j + 1).min(b.len()));
    }
    if b[i..].starts_with(b"{{") {
        return Some(text[i + 2..].find("}}").map_or(b.len(), |n| i + 2 + n + 2));
    }
    None
}

/// A `curl …` command, as DevTools copies it (Chrome's and Firefox's "Copy as
/// cURL" for bash), as a Postman v2.1 `request`.
pub fn parse_curl(command: &str) -> Result<Value, HttpError> {
    /// Options that take no value; the rest of a bundle like `-sSL` is more of them.
    const BARE: &[&str] = &[
        "-I", "--head", "--compressed", "-k", "--insecure", "-L", "--location", "-s", "--silent", "-S", "--show-error",
        "-i", "--include", "-v", "--verbose", "-g", "--globoff", "--http1.1", "--http2",
    ];
    let mut words: std::collections::VecDeque<String> = shell_words(command)?.into();
    if words.pop_front().as_deref() != Some("curl") {
        return Err(HttpError::BadCurl("it does not start with curl".into()));
    }
    let (mut method, mut url, mut headers, mut data, mut user) = (None, None, Vec::new(), Vec::<String>::new(), None);
    let mut head = false;
    while let Some(word) = words.pop_front() {
        if !word.starts_with('-') || word == "-" {
            if url.replace(word.clone()).is_some() {
                return Err(HttpError::BadCurl(format!("a second address {word}")));
            }
            continue;
        }
        // `-XPOST`, `-H'…'`: a short option with its value attached.
        let (flag, attached) = match word.strip_prefix('-').filter(|w| !w.starts_with('-')).and_then(|w| w.chars().next()) {
            Some(c) => (format!("-{c}"), Some(word[1 + c.len_utf8()..].to_string()).filter(|rest| !rest.is_empty())),
            None => (word.clone(), None),
        };
        if BARE.contains(&flag.as_str()) {
            head |= flag == "-I" || flag == "--head";
            if let Some(rest) = attached {
                words.push_front(format!("-{rest}"));
            }
            continue;
        }
        let mut value = || attached.clone().or_else(|| words.pop_front()).ok_or_else(|| HttpError::BadCurl(format!("{flag} without a value")));
        match flag.as_str() {
            "-X" | "--request" => method = Some(value()?),
            "-H" | "--header" => {
                let h = value()?;
                let (k, v) = h.split_once(':').ok_or_else(|| HttpError::BadCurl(format!("header without a colon: {h}")))?;
                headers.push((k.trim().to_string(), v.trim_start().to_string()));
            }
            "-d" | "--data" | "--data-raw" | "--data-binary" | "--data-ascii" => data.push(value()?),
            "-u" | "--user" => user = Some(value()?),
            "--url" => {
                let given = value()?;
                if url.replace(given.clone()).is_some() {
                    return Err(HttpError::BadCurl(format!("a second address {given}")));
                }
            }
            "-b" | "--cookie" => headers.push(("Cookie".into(), value()?)),
            "-A" | "--user-agent" => headers.push(("User-Agent".into(), value()?)),
            "-e" | "--referer" => headers.push(("Referer".into(), value()?)),
            other => return Err(HttpError::BadCurl(format!("unknown option {other}"))),
        }
    }
    let url = url.ok_or_else(|| HttpError::BadCurl("no address".into()))?;
    let body = (!data.is_empty()).then(|| data.join("&"));
    let method = method.unwrap_or_else(|| (if head { "HEAD" } else if body.is_some() { "POST" } else { "GET" }).into());
    let json_body = headers.iter().any(|(k, v)| k.eq_ignore_ascii_case("content-type") && v.contains("json"));
    let mut request = json!({
        "method": method.to_uppercase(),
        "header": headers.iter().map(|(k, v)| json!({"key": k, "value": v})).collect::<Vec<_>>(),
    });
    if let Some(raw) = body {
        request["body"] = json!({"mode": "raw", "raw": raw, "options": {"raw": {"language": if json_body { "json" } else { "text" }}}});
    }
    request["url"] = json!(url);
    if let Some(user) = user {
        let (name, password) = user.split_once(':').unwrap_or((&user, ""));
        request["auth"] = json!({"type": "basic", "basic": [
            {"key": "username", "value": name, "type": "string"},
            {"key": "password", "value": password, "type": "string"},
        ]});
    }
    Ok(request)
}

/// Words as bash splits them: `'…'`, `"…"` with its escapes, `$'…'` with C's,
/// a backslash outside quotes, and `\` at a line's end joining the next line.
fn shell_words(text: &str) -> Result<Vec<String>, HttpError> {
    let mut words = Vec::new();
    let mut word: Option<String> = None;
    let mut chars = text.chars().peekable();
    let unclosed = || HttpError::BadCurl("a quote is not closed".into());
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => words.extend(word.take()),
            '\\' => match chars.next() {
                Some('\n') => {}
                Some('\r') if chars.peek() == Some(&'\n') => {
                    chars.next();
                }
                Some(next) => word.get_or_insert_default().push(next),
                None => {}
            },
            '\'' => {
                let w = word.get_or_insert_default();
                loop {
                    match chars.next().ok_or_else(unclosed)? {
                        '\'' => break,
                        c => w.push(c),
                    }
                }
            }
            '"' => {
                let w = word.get_or_insert_default();
                loop {
                    match chars.next().ok_or_else(unclosed)? {
                        '"' => break,
                        '\\' => match chars.next().ok_or_else(unclosed)? {
                            c @ ('"' | '\\' | '$' | '`') => w.push(c),
                            '\n' => {}
                            c => {
                                w.push('\\');
                                w.push(c);
                            }
                        },
                        c => w.push(c),
                    }
                }
            }
            '$' if chars.peek() == Some(&'\'') => {
                chars.next();
                let w = word.get_or_insert_default();
                loop {
                    match chars.next().ok_or_else(unclosed)? {
                        '\'' => break,
                        '\\' => match chars.next().ok_or_else(unclosed)? {
                            'n' => w.push('\n'),
                            't' => w.push('\t'),
                            'r' => w.push('\r'),
                            'x' => w.push(hex_char(&mut chars, 2)?),
                            'u' => w.push(hex_char(&mut chars, 4)?),
                            c => w.push(c),
                        },
                        c => w.push(c),
                    }
                }
            }
            c => word.get_or_insert_default().push(c),
        }
    }
    words.extend(word);
    Ok(words)
}

fn hex_char(chars: &mut std::iter::Peekable<std::str::Chars>, len: usize) -> Result<char, HttpError> {
    let digits: String = (0..len).filter_map(|_| chars.next_if(|c| c.is_ascii_hexdigit())).collect();
    u32::from_str_radix(&digits, 16)
        .ok()
        .and_then(char::from_u32)
        .ok_or_else(|| HttpError::BadCurl(format!("a bad escape \\{digits}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn var(key: &str, value: &str) -> Variable {
        Variable { key: key.into(), value: value.into(), secret: false, enabled: true }
    }

    fn secret(key: &str, value: &str) -> Variable {
        Variable { secret: true, ..var(key, value) }
    }

    #[test]
    fn a_later_layer_wins_and_a_variable_turned_off_is_not_there() {
        let globals = [var("host", "global"), var("only", "g")];
        let collection = [var("host", "collection")];
        let environment = [var("host", "env"), Variable { enabled: false, ..var("only", "off") }];
        let scope = Scope::new(&[&globals, &collection, &environment]);
        assert_eq!(scope.fill("{{host}} {{ only }}").unwrap().sent, "env g");
        let scope = Scope::new(&[&globals, &collection]);
        assert_eq!(scope.fill("{{host}}").unwrap().sent, "collection");
    }

    #[test]
    fn every_unknown_name_is_named_once() {
        let scope = Scope::new(&[&[var("a", "1")]]);
        assert_eq!(
            scope.fill("{{a}}/{{token}}/{{b}}/{{token}}"),
            Err(HttpError::UnknownVariables(vec!["token".into(), "b".into()]))
        );
    }

    #[test]
    fn a_value_with_variables_is_filled_in_too_and_a_cycle_is_an_error() {
        let vars = [var("base", "{{scheme}}://api"), var("scheme", "https"), var("loop", "x{{again}}"), var("again", "{{loop}}")];
        let scope = Scope::new(&[&vars]);
        assert_eq!(scope.fill("{{base}}/v1").unwrap().sent, "https://api/v1");
        assert_eq!(scope.fill("{{loop}}"), Err(HttpError::VariableCycle("loop".into())));
        // The one written in the text is named, not wherever a depth limit happens to stop.
        let three = [var("a", "{{b}}"), var("b", "{{c}}"), var("c", "{{a}}")];
        assert_eq!(Scope::new(&[&three]).fill("x {{a}}"), Err(HttpError::VariableCycle("a".into())));
    }

    #[test]
    fn a_secret_is_sent_and_never_shown_even_inside_another_variable() {
        let vars = [secret("token", "s3cr3t"), var("auth", "Bearer {{token}}")];
        let filled = Scope::new(&[&vars]).fill("{{auth}}").unwrap();
        assert_eq!(filled.sent, "Bearer s3cr3t");
        assert_eq!(filled.shown, "Bearer •••");
    }

    #[test]
    fn dynamic_variables_are_made_once_for_both_and_the_rest_are_named() {
        let scope = Scope::default();
        let filled = scope.fill("{{$guid}} {{$timestamp}} {{$isoTimestamp}} {{$randomInt}} {{$randomUUID}}").unwrap();
        assert_eq!(filled.sent, filled.shown);
        let shape = Regex::new(r"^[0-9a-f-]{36} \d{10} \d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z \d{1,4} [0-9a-f-]{36}$").unwrap();
        assert!(shape.is_match(&filled.sent), "{}", filled.sent);
        assert_eq!(scope.fill("{{$randomBankAccount}}"), Err(HttpError::UnsupportedDynamic("$randomBankAccount".into())));
    }

    #[test]
    fn the_window_never_gets_a_secrets_value_and_an_untouched_secret_keeps_it() {
        let stored = Environment { id: "e".into(), name: "test".into(), variables: vec![secret("token", "s3cr3t"), secret("unset", ""), var("baseUrl", "https://api")], trusted_cert: None, agent: AgentAccess::Off };
        let shown = stored.for_window();
        assert!(!serde_json::to_string(&shown).unwrap().contains("s3cr3t"));
        assert_eq!(shown.variables[0].value, SECRET_MASK);
        assert_eq!(shown.variables[1].value, "", "a secret with no value says so");
        assert_eq!(shown.variables[2].value, "https://api");

        let mut back = shown.clone();
        back.variables[1].value = "new".into();
        back.variables[2].value = "https://other".into();
        let edited = stored.edited(back.clone()).unwrap();
        assert_eq!(edited.variables[0].value, "s3cr3t", "the mask sent back keeps the value");
        assert_eq!(edited.variables[1].value, "new");
        assert_eq!(edited.variables[2].value, "https://other");
        back.variables[0].value = String::new();
        assert_eq!(stored.edited(back.clone()).unwrap().variables[0].value, "s3cr3t", "empty keeps it too");

        let mut unmarked = shown.clone();
        unmarked.variables[0].secret = false;
        assert_eq!(stored.edited(unmarked).unwrap().variables[0].value, "s3cr3t", "unmarking keeps the value, not the mask");
        let mut renamed = shown.clone();
        renamed.variables[0].key = "token2".into();
        assert_eq!(stored.edited(renamed), Err(HttpError::MaskWithoutValue("token2".into())));

        back.variables[0].value = "rotated".into();
        assert_eq!(stored.edited(back).unwrap().variables[0].value, "rotated");
    }

    fn sent(body: &str) -> HistoryEntry {
        let out = |auth: &str| Outgoing { method: "POST".into(), url: "https://api/x".into(), headers: vec![("Authorization".into(), auth.into())], body: Some(body.into()), digest: None };
        let prepared = Prepared { sent: out("Bearer t0k"), shown: out("Bearer •••") };
        let outcome = Outcome::Response { status: 200, headers: vec![], url: "https://api/x".into(), redirects: vec![], elapsed_ms: 1 };
        HistoryEntry::new(1, None, None, "{{baseUrl}}/x".into(), &prepared, outcome, body.as_bytes())
    }

    #[test]
    fn what_comes_back_about_a_request_hides_its_secrets_however_encoded() {
        let secret = |key: &str, value: &str| Variable { key: key.into(), value: value.into(), secret: true, enabled: true };
        let vars = [secret("token", "t0k"), secret("pass", "a b/é"), secret("unset", ""), var("open", "t0")];
        let scope = Scope::new(&[&vars]);
        assert_eq!(scope.hide("https://api/bot-t0k/x?pass=a%20b%2F%C3%A9&p2=a%20b/%C3%A9&p3=a b/é"), "https://api/bot-•••/x?pass=•••&p2=•••&p3=•••");
        assert_eq!(scope.hide("t0 stays"), "t0 stays");

        let outcome = Outcome::Response { status: 200, headers: vec![("x".into(), "t0k".into())], url: "https://api/t0k".into(), redirects: vec!["https://api/old?k=t0k".into()], elapsed_ms: 1 };
        let Outcome::Response { headers, url, redirects, .. } = outcome.hidden(&scope) else { panic!() };
        assert_eq!((url.as_str(), redirects[0].as_str()), ("https://api/•••", "https://api/old?k=•••"));
        assert_eq!(headers[0].1, "t0k", "the server's own headers are its answer, shown as they came");
        assert_eq!(Outcome::Failed { error: "http://[t0k is not an address".into() }.hidden(&scope), Outcome::Failed { error: "http://[••• is not an address".into() });
    }

    #[test]
    fn the_way_a_response_was_redirected_counts_toward_the_50_mb() {
        let e = sent("small");
        let mut redirected = e.clone();
        let Outcome::Response { redirects, .. } = &mut redirected.outcome else { panic!() };
        redirects.push("x".repeat(1000));
        assert_eq!(redirected.size(), e.size() + 1000);
    }

    #[test]
    fn a_secret_copied_as_text_is_masked_where_it_shows_and_goes_out_as_it_is() {
        let secret = vec![Variable { key: "t".into(), value: "s3cr3t".into(), secret: true, enabled: true }];
        let scope = Scope::new(&[&secret]);
        let out = Outgoing { method: "POST".into(), url: "http://a.test/s3cr3t".into(), headers: vec![("X-s3cr3t".into(), "Bearer s3cr3t".into())], body: Some("k=s3cr3t".into()), digest: None };
        let hidden = Prepared { sent: out.clone(), shown: out.clone() }.hidden(&scope);
        assert_eq!(hidden.sent, out);
        assert!(!format!("{:?}", hidden.shown).contains("s3cr3t"), "{:?}", hidden.shown);
    }

    #[test]
    fn a_response_saved_before_scripts_reads_and_a_report_does_not_say_changed_twice() {
        let old: SavedResponse = serde_json::from_str(r#"{"at": 1, "environment": null, "outcome": {"kind": "failed", "error": "x"}}"#).unwrap();
        assert!(old.scripts.is_empty());
        assert!(!serde_json::to_string(&old).unwrap().contains("scripts"), "nothing to say, nothing written");
        let line = crate::domain::http_scripts::ConsoleLine { level: "log".into(), text: "hi".into() };
        let said = ScriptReport { console: vec![line], changed: true, ..Default::default() };
        let saved = SavedResponse { scripts: said.clone(), ..old };
        let read: SavedResponse = serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
        assert_eq!(read.scripts, ScriptReport { changed: false, ..said });
    }

    #[test]
    fn a_body_that_is_not_text_reaches_the_window_as_its_size() {
        let saved = |body: Vec<u8>| SavedResponse { at: 1, environment: None, outcome: Outcome::Failed { error: String::new() }, body, scripts: Default::default() };
        let view = ResponseView::from(saved(vec![0x89, b'P', b'N', b'G', 0xff]));
        assert_eq!((view.body, view.size), (None, 5));
        let view = ResponseView::from(saved("é".into()));
        assert_eq!((view.body.as_deref(), view.size), (Some("é"), 2));
    }

    #[test]
    fn history_keeps_the_shown_request_and_cuts_bodies() {
        let e = sent("small");
        assert_eq!(e.request.headers[0].1, "Bearer •••");
        assert_eq!((e.request.body.as_deref(), e.response_body.as_deref()), (Some("small"), Some("small")));

        // A three-byte character across the line: cut before it, not through it.
        let big = format!("{}€tail", "a".repeat(HISTORY_BODY - 1));
        let e = sent(&big);
        let cut = format!("{}\n… cut at 256.0 KB in history", "a".repeat(HISTORY_BODY - 1));
        assert_eq!((e.request.body.as_deref(), e.response_body.as_deref()), (Some(cut.as_str()), Some(cut.as_str())));
        assert_eq!(sent(&"a".repeat(HISTORY_BODY)).response_body.unwrap().len(), HISTORY_BODY, "at the line, whole");

        let failed = HistoryEntry::new(1, None, None, String::new(), &Prepared { sent: e.request.clone(), shown: e.request.clone() }, Outcome::Failed { error: "refused".into() }, b"");
        assert_eq!(failed.response_body, None);

        let binary = HistoryEntry::new(1, None, None, String::new(), &Prepared { sent: e.request.clone(), shown: e.request.clone() }, Outcome::Response { status: 200, headers: vec![], url: String::new(), redirects: vec![], elapsed_ms: 1 }, &[0, 0, 0, 0xff]);
        assert_eq!(binary.response_body.as_deref(), Some("binary, 4 bytes"));
        assert_eq!(sent("a\0b").response_body.as_deref(), Some("binary, 3 bytes"), "valid UTF-8, but \\0 is six bytes in JSON");
        assert_eq!(sent("a\tb\r\nc").response_body.as_deref(), Some("a\tb\r\nc"));
    }

    #[test]
    fn history_drops_the_oldest_past_200_or_50_mb() {
        let mut log: Vec<HistoryEntry> = (0..205).map(|i| sent(&i.to_string())).collect();
        trim_history(&mut log);
        assert_eq!(log.len(), HISTORY_ENTRIES);
        assert_eq!(log[0].request.body.as_deref(), Some("5"), "the five oldest went");

        let quarter = "b".repeat(HISTORY_BODY);
        let mut log: Vec<HistoryEntry> = (0..120).map(|_| sent(&quarter)).collect();
        log.push(sent("newest"));
        trim_history(&mut log);
        let total: usize = log.iter().map(HistoryEntry::size).sum();
        assert!(total <= HISTORY_BYTES && total > HISTORY_BYTES - 2 * sent(&quarter).size(), "{total}");
        assert_eq!(log.last().unwrap().request.body.as_deref(), Some("newest"));
    }

    #[test]
    fn nesting_and_growth_are_bounded() {
        let chain: Vec<Variable> = (0..12).map(|i| var(&format!("v{i}"), &format!("{{{{v{}}}}}", i + 1))).chain([var("v12", "end")]).collect();
        assert_eq!(Scope::new(&[&chain]).fill("{{v2}}"), Err(HttpError::VariableCycle("v12".into())), "eleven deep");
        assert_eq!(Scope::new(&[&chain]).fill("{{v3}}").unwrap().sent, "end", "ten deep is fine");
        // Eight of the next in each: 8^9 copies if nothing stopped it.
        let laughs: Vec<Variable> = (0..9).map(|i| var(&format!("l{i}"), &format!("{{{{l{}}}}}", i + 1).repeat(8))).chain([var("l9", "ha")]).collect();
        assert_eq!(Scope::new(&[&laughs]).fill("{{l0}}"), Err(HttpError::TooLarge));
    }

    #[test]
    fn a_comment_on_its_own_line_goes_with_it_and_one_after_a_value_stays_on_its_line() {
        let text = "{\n  // the id\n  \"id\": 1, // one\n  /* gone */\n  \"url\": \"http://x//y\",\n  \"v\": \"{{a//b}}\"\n}";
        assert_eq!(strip_json_comments(text), "{\n  \"id\": 1, \n  \"url\": \"http://x//y\",\n  \"v\": \"{{a//b}}\"\n}");
        assert_eq!(strip_json_comments("[1 /* in */, 2] // end"), "[1 , 2] ");
        assert_eq!(strip_json_comments("\"esc \\\" // not\""), "\"esc \\\" // not\"");
        assert_eq!(strip_json_comments("{} /* unclosed"), "{} ");
        assert_eq!(strip_json_comments("// only"), "");
        assert_eq!(strip_json_comments("{\"a\": {{url//x}} }"), "{\"a\": {{url//x}} }", "a variable outside quotes is not a comment");
    }

    #[test]
    fn chromes_copy_as_curl_reads() {
        let chrome = "curl 'https://api.example/v1/orders?x=1' \\\n  -H 'accept: application/json' \\\n  -H 'content-type: application/json' \\\n  -b 'sid=abc' \\\n  --data-raw $'{\"note\":\"it\\'s\\nfine\"}' \\\n  --compressed";
        let r = parse_curl(chrome).unwrap();
        assert_eq!(r["method"], "POST");
        assert_eq!(r["url"], "https://api.example/v1/orders?x=1");
        assert_eq!(r["header"], json!([
            {"key": "accept", "value": "application/json"},
            {"key": "content-type", "value": "application/json"},
            {"key": "Cookie", "value": "sid=abc"},
        ]));
        assert_eq!(r["body"]["raw"], "{\"note\":\"it's\nfine\"}");
        assert_eq!(r["body"]["options"]["raw"]["language"], "json");
    }

    #[test]
    fn firefoxs_copy_as_curl_and_double_quotes_read() {
        let firefox = "curl 'https://api.example/v1/orders/7' -X PUT -H 'User-Agent: Mozilla/5.0' -H \"Authorization: Bearer {{token}}\" --data-raw 'a=1'";
        let r = parse_curl(firefox).unwrap();
        assert_eq!(r["method"], "PUT");
        assert_eq!(r["header"][1], json!({"key": "Authorization", "value": "Bearer {{token}}"}));
        assert_eq!(r["body"]["raw"], "a=1");
        assert_eq!(r["body"]["options"]["raw"]["language"], "text");
        let short = parse_curl("curl -XDELETE -u 'me:p:w' \"https://api.example/a\\$b\" -d x -d y").unwrap();
        assert_eq!(short["method"], "DELETE");
        assert_eq!(short["url"], "https://api.example/a$b");
        assert_eq!(short["body"]["raw"], "x&y");
        assert_eq!(short["auth"]["basic"][1]["value"], "p:w");
        assert_eq!(parse_curl("curl -I https://api.example").unwrap()["method"], "HEAD");
    }

    #[test]
    fn bundled_flags_and_the_rest_of_curls_options_read() {
        let r = parse_curl("curl -sX POST https://x/a").unwrap();
        assert_eq!((r["method"].as_str(), r["url"].as_str()), (Some("POST"), Some("https://x/a")));
        assert_eq!(parse_curl("curl -sSI https://x").unwrap()["method"], "HEAD");
        let r = parse_curl("curl -A 'agent/1' -e https://ref --url https://x/b -u me \\\r\n  -d $'a\\u00e9!'").unwrap();
        assert_eq!(r["header"], json!([{"key": "User-Agent", "value": "agent/1"}, {"key": "Referer", "value": "https://ref"}]));
        assert_eq!(r["url"], "https://x/b");
        assert_eq!(r["auth"]["basic"][0]["value"], "me");
        assert_eq!(r["auth"]["basic"][1]["value"], "", "no colon, no password");
        assert_eq!(r["body"]["raw"], "aé!");
    }

    #[test]
    fn what_curl_cannot_be_is_said() {
        assert!(matches!(parse_curl("wget https://x"), Err(HttpError::BadCurl(_))));
        assert_eq!(parse_curl("curl --proxy p https://x"), Err(HttpError::BadCurl("unknown option --proxy".into())));
        assert_eq!(parse_curl("curl 'https://x"), Err(HttpError::BadCurl("a quote is not closed".into())));
        assert_eq!(parse_curl("curl -H"), Err(HttpError::BadCurl("-H without a value".into())));
        assert_eq!(parse_curl("curl -s"), Err(HttpError::BadCurl("no address".into())));
        assert_eq!(parse_curl("curl -é https://x"), Err(HttpError::BadCurl("unknown option -é".into())), "not a panic");
        assert_eq!(parse_curl("curl https://x https://y"), Err(HttpError::BadCurl("a second address https://y".into())));
        assert_eq!(parse_curl("curl https://x --url https://y"), Err(HttpError::BadCurl("a second address https://y".into())));
    }

    fn opened(variables: Vec<Variable>) -> Environment {
        Environment { id: "e".into(), name: "Stage".into(), variables, trusted_cert: None, agent: AgentAccess::ReadOnly }
    }

    fn ask(method: &str, url: &str, headers: &[(&str, &str)]) -> AgentRequest {
        AgentRequest { method: method.into(), url: url.into(), headers: headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(), body: None }
    }

    fn stand() -> Environment {
        opened(vec![var("baseUrl", "https://api.test/v1"), secret("token", "s3cr3t"), var("id", "42")])
    }

    /// Where a collection of `https://api.test` requests sends.
    fn reach() -> Reach {
        Reach { origins: ["https://api.test".to_string()].into(), ..Reach::default() }
    }

    fn refused(environment: &Environment, request: AgentRequest) -> HttpError {
        environment.agent_request(&request, &reach()).map(|_| ()).unwrap_err()
    }

    fn agent_sends(environment: &Environment, request: AgentRequest) -> Prepared {
        environment.agent_request(&request, &reach()).unwrap().0
    }

    /// A collection named Shop, its items given ids by the caller.
    fn collection(items: Value, auth: Value, variables: Value) -> Collection {
        let root = json!({
            "info": {"name": "Shop", "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},
            "item": items, "auth": auth, "variable": variables
        });
        Collection::parse(&root.to_string()).unwrap()
    }

    fn get(id: &str, url: Value) -> Value {
        json!({"id": id, "name": id, "request": {"method": "get", "url": url}})
    }

    #[test]
    fn an_environment_off_takes_nothing_and_says_nothing_of_itself() {
        let off = Environment { agent: AgentAccess::Off, ..stand() };
        assert_eq!(refused(&off, ask("GET", "{{baseUrl}}/x", &[])), HttpError::NoneOpen);
        assert_eq!(off.described(&reach()), Err(HttpError::NoneOpen));
    }

    #[test]
    fn read_only_lets_through_what_reads_and_names_what_it_refuses() {
        for (asked, method) in [("GET", "GET"), (" head ", "HEAD"), ("options", "OPTIONS")] {
            assert_eq!(agent_sends(&stand(), ask(asked, "{{baseUrl}}/x", &[])).sent.method, method);
        }
        for method in ["POST", "PUT", "PATCH", "DELETE", "TRACE"] {
            assert_eq!(refused(&stand(), ask(method, "{{baseUrl}}/x", &[])), HttpError::ReadOnly { environment: "Stage".into(), method: method.into() });
        }
        assert_eq!(refused(&stand(), ask("post", "{{baseUrl}}/x", &[])).to_string(), "\"Stage\" is read only for the agent: POST is refused — the user can change its agent access on the HTTP page");
    }

    #[test]
    fn a_mode_open_to_writes_lets_every_method_through() {
        for access in [AgentAccess::AskForWrites, AgentAccess::WritesAsOtherTools] {
            let open = Environment { agent: access, ..stand() };
            for method in ["GET", "post", "PUT", "PATCH", "DELETE"] {
                assert_eq!(agent_sends(&open, ask(method, "{{baseUrl}}/x", &[])).sent.method, method.to_ascii_uppercase(), "{access:?}");
            }
        }
    }

    /// The body is filled in as the url is, and a secret in it is masked in what is shown and kept.
    #[test]
    fn a_body_goes_filled_in_and_a_secret_in_it_is_shown_masked() {
        let open = Environment { agent: AgentAccess::WritesAsOtherTools, ..stand() };
        let mut request = ask("POST", "{{baseUrl}}/orders", &[]);
        request.body = Some(r#"{"id": {{id}}, "key": "{{token}}"}"#.into());
        let prepared = agent_sends(&open, request);
        assert_eq!(prepared.sent.body.as_deref(), Some(r#"{"id": 42, "key": "s3cr3t"}"#));
        assert_eq!(prepared.shown.body.as_deref(), Some(r#"{"id": 42, "key": "•••"}"#));
        let mut unknown = ask("POST", "{{baseUrl}}/orders", &[]);
        unknown.body = Some("{{nope}}".into());
        assert!(matches!(refused(&open, unknown), HttpError::UnknownIn { names, .. } if names == ["nope"]));
    }

    /// JSON — an object or an array — with no type of its own goes as application/json;
    /// a type the model gave, in any case, stays the only one, and other text gets none.
    #[test]
    fn json_with_no_type_goes_as_json_and_nothing_else_gets_one() {
        let open = Environment { agent: AgentAccess::WritesAsOtherTools, ..stand() };
        let types = |body: &str, headers: &[(&str, &str)]| {
            let mut request = ask("POST", "{{baseUrl}}/x", headers);
            request.body = Some(body.into());
            let prepared = agent_sends(&open, request);
            assert_eq!(prepared.sent.headers, prepared.shown.headers);
            prepared.sent.headers.into_iter().filter(|(k, _)| k.eq_ignore_ascii_case("content-type")).map(|(_, v)| v).collect::<Vec<_>>()
        };
        assert_eq!(types(r#"{"a": {{id}}}"#, &[]), ["application/json"]);
        assert_eq!(types("[1, 2]", &[]), ["application/json"]);
        assert_eq!(types("{\"a\": 1}", &[("content-type", "text/plain")]), ["text/plain"]);
        for other in ["a=1&b=2", "42", "\"text\"", "{not json", ""] {
            assert_eq!(types(other, &[]), Vec::<String>::new(), "{other}");
        }
        assert_eq!(agent_sends(&open, ask("POST", "{{baseUrl}}/x", &[])).sent.headers, [], "no body, no type");
    }

    #[test]
    fn a_url_goes_filled_in_and_a_secret_in_it_is_shown_masked() {
        let prepared = agent_sends(&stand(), ask("GET", "{{baseUrl}}/orders/{{id}}?key={{token}}", &[]));
        assert_eq!(prepared.sent.url, "https://api.test/v1/orders/42?key=s3cr3t");
        assert_eq!(prepared.shown.url, "https://api.test/v1/orders/42?key=•••");
        assert_eq!((prepared.sent.body, prepared.shown.method.as_str()), (None, "GET"));
        assert_eq!(agent_sends(&stand(), ask("GET", "HTTPS://API.test/a/../b", &[])).sent.url, "https://api.test/b", "as the url crate sends it");
    }

    #[test]
    fn a_url_with_no_scheme_is_http_as_on_the_page() {
        let local = opened(vec![var("host", "localhost:3000")]);
        let reach = Reach { origins: ["http://localhost:3000".to_string()].into(), ..Reach::default() };
        assert_eq!(local.agent_request(&ask("GET", "{{host}}/v1/x?next=http://a", &[]), &reach).unwrap().0.sent.url, "http://localhost:3000/v1/x?next=http://a");
    }

    #[test]
    fn an_address_no_collection_goes_to_is_refused_and_says_what_to_do_instead() {
        let untrusted = |url: &str, origin: &str| HttpError::Untrusted { url: url.into(), origin: origin.into(), environment: "Stage".into(), trusted: "https://api.test".into() };
        for (url, origin) in [
            ("https://evil.test/x", "https://evil.test"),
            ("https://api.test@evil.test/x", "https://evil.test"),
            ("https://api.test.evil.test/", "https://api.test.evil.test"),
            ("//evil.test/x", "http://evil.test"),
            ("http://api.test/x", "http://api.test"),
            ("https://api.test:8443/x", "https://api.test:8443"),
        ] {
            assert_eq!(refused(&stand(), ask("GET", url, &[])), untrusted(url, origin), "{url}");
        }
        let told = refused(&stand(), ask("GET", "https://evil.test/x", &[])).to_string();
        assert_eq!(told, "https://evil.test/x goes to https://evil.test, and no request in the user's collections goes there with \"Stage\" (they go to: https://api.test) — ask the user to add a request for it on the HTTP page, or give them a curl command to run themselves");
        let nowhere = stand().agent_request(&ask("GET", "{{baseUrl}}/x", &[]), &Reach::default()).unwrap_err();
        assert!(matches!(&nowhere, HttpError::Untrusted { trusted, .. } if trusted == "nowhere"), "{nowhere:?}");
    }

    #[test]
    fn an_address_refused_names_no_secret() {
        let carried = opened(vec![secret("host", "s3cr3t.evil.test")]);
        let told = carried.agent_request(&ask("GET", "https://{{host}}/x", &[]), &reach()).unwrap_err();
        assert_eq!(told, HttpError::Untrusted { url: "https://{{host}}/x".into(), origin: "a host with a hidden value in it".into(), environment: "Stage".into(), trusted: "https://api.test".into() });
        // The url crate writes a host in lower case: the mask would no longer find it.
        let upper = opened(vec![secret("host", "AbC123xYz.evil.test")]);
        let told = upper.agent_request(&ask("GET", "https://{{host}}/x", &[]), &reach()).unwrap_err().to_string();
        assert!(!told.to_lowercase().contains("abc123xyz"), "{told}");
    }

    #[test]
    fn what_is_no_http_address_is_said_as_written() {
        assert_eq!(refused(&stand(), ask("GET", "ftp://api.test/x", &[])), HttpError::NotAnAddress("ftp://api.test/x".into()));
        assert_eq!(refused(&stand(), ask("GET", "http://[nope/x", &[])), HttpError::NotAnAddress("http://[nope/x".into()));
        let carried = opened(vec![secret("away", "http://[s3cr3t")]);
        assert_eq!(carried.agent_request(&ask("GET", "{{away}}", &[]), &reach()).unwrap_err(), HttpError::NotAnAddress("{{away}}".into()));
    }

    #[test]
    fn headers_go_filled_in_and_are_shown_masked_and_the_host_is_not_the_models() {
        let prepared = agent_sends(&stand(), ask("GET", "{{baseUrl}}/x", &[("Authorization", "Bearer {{token}}"), ("X-Id", "{{id}}")]));
        assert_eq!(prepared.sent.headers, [("Authorization".to_string(), "Bearer s3cr3t".to_string()), ("X-Id".to_string(), "42".to_string())]);
        assert_eq!(prepared.shown.headers, [("Authorization".to_string(), "Bearer •••".to_string()), ("X-Id".to_string(), "42".to_string())]);
        assert_eq!(refused(&stand(), ask("GET", "{{baseUrl}}/x", &[("hOsT", "evil.test")])), HttpError::HostHeader);
    }

    #[test]
    fn a_name_not_there_lists_the_names_that_are_and_no_value() {
        let mut environment = stand();
        environment.variables.push(Variable { enabled: false, ..var("hidden", "h") });
        let told = HttpError::UnknownIn { names: vec!["nope".into()], environment: "Stage".into(), known: "baseUrl, token, id".into() };
        assert_eq!(refused(&environment, ask("GET", "{{baseUrl}}/{{nope}}", &[])), told);
        assert_eq!(refused(&environment, ask("GET", "{{baseUrl}}/x", &[("X-A", "{{nope}}")])), told);
        assert_eq!(told.to_string(), "nope not in \"Stage\", which has: baseUrl, token, id");
        assert_eq!(refused(&stand(), ask("GET", "{{baseUrl}}/{{$random}}", &[])), HttpError::UnsupportedDynamic("$random".into()), "another error stands");
    }

    #[test]
    fn the_scope_goes_on_to_mask_the_reply() {
        let (_, scope) = stand().agent_request(&ask("GET", "{{baseUrl}}/x", &[]), &reach()).unwrap();
        assert_eq!(scope.hide("s3cr3t"), "•••");
    }

    #[test]
    fn a_credential_the_model_sends_is_masked_though_no_variable_marks_it() {
        let imported = opened(vec![var("baseUrl", "https://api.test"), var("t", "pl41n"), var("k", "k3y-1"), var("id", "1d42"), var("short", "ab1")]);
        let headers = [("authorization", "Bearer {{t}}"), ("X-Api-Key", "{{k}}"), ("X-Id", "{{id}}"), ("X-Session-Id", "wr1tten"), ("X-Token", "{{short}}")];
        let (prepared, scope) = imported.agent_request(&ask("GET", "{{baseUrl}}/x", &headers), &reach()).unwrap();
        assert_eq!(prepared.sent.headers[0].1, "Bearer pl41n");
        assert_eq!(prepared.hidden(&scope).shown.headers[0].1, "Bearer •••", "in history too");
        assert_eq!(scope.hide(r#"{"you":"pl41n","k":"k3y-1"}"#), r#"{"you":"•••","k":"•••"}"#);
        assert_eq!(scope.hide("1d42"), "1d42", "another header is what the variable says");
        assert_eq!(scope.hide("wr1tten"), "wr1tten", "what the model wrote out it knows");
        assert_eq!(scope.hide("ab1 of 3"), "ab1 of 3", "too short to mask without masking the reply");
    }

    #[test]
    fn the_collections_say_where_the_agent_may_go() {
        let shop = collection(
            json!([
                {"id": "f", "name": "Orders", "item": [get("list", json!({"raw": "{{baseUrl}}/orders?page=2"}))]},
                get("local", json!("localhost:3000/health")),
                get("auth", json!("{{authUrl}}/token")),
                get("global", json!("{{g}}/x")),
                get("unknown", json!("https://{{nope}}/x")),
                get("hidden", json!("https://{{secretHost}}/x")),
                get("ftp", json!("ftp://files.test/x")),
                {"id": "plain", "name": "plain", "request": "https://string.test/x"},
                get("tenant", json!("https://{{tenant}}.api.test/x")),
                get("named", json!("{{baseUrl}}/{{sessionId}}/{{$guid}}")),
            ]),
            json!({"type": "bearer", "bearer": [{"key": "token", "value": "{{tenant}}"}]}),
            json!([{"key": "authUrl", "value": "https://auth.test:8443"}, {"key": "secretHost", "value": "s3cr3t.test", "type": "secret"}, {"key": "tenant", "value": "acme"}, {"key": "sessionId", "value": "s3ss10n"}]),
        );
        let theirs = shop.variables();
        let reach = Reach::new(&[(&shop, theirs)], &[var("g", "https://global.test")], &stand());
        let origins: Vec<&str> = reach.origins.iter().map(String::as_str).collect();
        assert_eq!(origins, ["http://localhost:3000", "https://api.test", "https://auth.test:8443", "https://global.test", "https://string.test"], "a credential of the collection's in a host is no address");
        assert_eq!(
            reach.requests,
            [
                "Shop / list: GET {{baseUrl}}/orders",
                "Shop / local: GET localhost:3000/health",
                "Shop / auth: GET https://auth.test:8443/token",
                "Shop / global: GET https://global.test/x",
                "Shop / unknown: GET https://{{nope}}/x",
                "Shop / hidden: GET https://{{secretHost}}/x",
                "Shop / ftp: GET ftp://files.test/x",
                "Shop / plain: GET https://string.test/x",
                "Shop / tenant: GET https://{{tenant}}.api.test/x",
                "Shop / named: GET {{baseUrl}}/{{sessionId}}/{{$guid}}",
            ],
            "the method in capitals, no query; the environment's names and hidden ones left for the agent"
        );
    }

    #[test]
    fn a_variable_written_where_a_credential_goes_is_hidden_and_so_is_what_it_names() {
        let shop = collection(
            json!([{
                "id": "f", "name": "F",
                "auth": {"type": "apikey", "apikey": [{"key": "value", "value": "{{apiKey}}"}]},
                "item": [{
                    "id": "r", "name": "r",
                    "request": {
                        "url": "{{baseUrl}}/x",
                        "auth": {"type": "basic", "basic": [{"key": "username", "value": "{{user}}"}, {"key": "password", "value": "{{pass}}"}]},
                        "header": [{"key": "X-Session-Id", "value": "{{sess}}"}, {"key": "Accept", "value": "{{accept}}"}, {"key": "Authorization", "value": "{{authHeader}}"}],
                        "body": {"mode": "urlencoded", "urlencoded": [{"key": "password", "value": "{{pw}}"}, {"key": "name", "value": "{{plain}}"}]}
                    }
                }, {
                    "id": "q", "name": "q",
                    "request": {"url": {"raw": "{{baseUrl}}/q?api_key={{qk}}", "query": [{"key": "api_key", "value": "{{qk}}"}]}}
                }]
            }]),
            json!({"type": "oauth2", "oauth2": [{"key": "accessTokenUrl", "value": "{{baseUrl}}/oauth/token"}, {"key": "clientSecret", "value": "{{cs}}"}]}),
            json!(null),
        );
        let names = ["cs", "apiKey", "user", "pass", "sess", "accept", "authHeader", "inner", "plain", "deep", "pw", "qk", "apiToken", "part"];
        let mut variables: Vec<Variable> = names.iter().map(|n| var(n, &format!("{n}-value"))).collect();
        variables.push(var("baseUrl", "https://api.test"));
        variables[6].value = "Bearer {{inner}}".into();
        variables[7].value = "{{deep}}".into();
        variables[12].value = "tk-{{part}}".into();
        let environment = opened(variables);
        let reach = Reach::new(&[(&shop, vec![])], &[], &environment);
        assert!(reach.origins.contains("https://api.test"), "an oauth2 address is no credential: {reach:?}");
        let view = environment.described(&reach).unwrap();
        let hidden: Vec<&str> = view.variables.iter().filter(|(_, v)| v.is_none()).map(|(k, _)| k.as_str()).collect();
        assert_eq!(hidden, ["cs", "apiKey", "user", "pass", "sess", "authHeader", "inner", "deep", "pw", "qk", "apiToken", "part"], "a chain a → b → c, a field and a parameter by their key, a variable by its name");
        assert!(view.variables.contains(&("accept".to_string(), Some("accept-value".to_string()))));

        let (prepared, scope) = environment.agent_request(&ask("GET", "{{baseUrl}}/x?k={{apiKey}}&p={{plain}}", &[]), &reach).unwrap();
        assert_eq!(prepared.shown.url, "https://api.test/x?k=•••&p=plain-value", "hidden in what is shown");
        assert_eq!(scope.hide("deep-value"), "•••", "and in what comes back");
    }

    #[test]
    fn the_description_says_where_what_and_which_without_a_secret() {
        let environment = opened(vec![var("baseUrl", "https://api.test/v1"), secret("token", "s3cr3t"), var("echo", "x={{token}}&y=s3cr3t"), Variable { enabled: false, ..var("off", "1") }]);
        let reach = Reach { origins: ["https://b.test".to_string(), "https://a.test".to_string()].into(), requests: vec!["Shop / r: GET https://a.test/s3cr3t".into()], ..Reach::default() };
        let view = environment.described(&reach).unwrap();
        assert_eq!(view.name, "Stage");
        assert_eq!(view.access, AgentAccess::ReadOnly);
        assert_eq!(view.origins, ["https://a.test", "https://b.test"]);
        assert_eq!(view.variables, [("baseUrl".to_string(), Some("https://api.test/v1".to_string())), ("token".to_string(), None), ("echo".to_string(), Some("x={{token}}&y=•••".to_string()))]);
        assert_eq!(view.requests, ["Shop / r: GET https://a.test/•••"]);
    }

    #[test]
    fn a_credential_is_known_by_its_name_and_an_address_is_not_one() {
        for name in ["Authorization", "proxy-authorization", "Cookie", "X-API-Key", "api_key", "X-Auth-Token", "X-Session-Id", "Client-Secret", "X-Password", "X-Token", "password"] {
            assert!(is_credential_name(name), "{name}");
        }
        for name in ["Accept", "Content-Type", "X-Request-Id", "User-Agent", "accessTokenUrl", "redirect_uri", "authHost", "keycloakDomain", "tokenEndpoint", "baseUrl"] {
            assert!(!is_credential_name(name), "{name}");
        }
    }

    /// Thirty-two scopes: the map's order is new in each, and a short secret first would leave the long one's tail.
    #[test]
    fn a_secret_that_starts_another_leaves_nothing_of_it() {
        for _ in 0..32 {
            let scope = Scope::new(&[&[secret("key", "abc"), secret("key2", "abc-def")]]);
            assert_eq!(scope.hide("abc-def and abc"), "••• and •••");
        }
    }

    #[test]
    fn a_secret_built_of_variables_is_found_as_it_went_out() {
        let scope = Scope::new(&[&[secret("token", "pre-{{suffix}}"), var("suffix", "42")]]);
        assert_eq!(scope.hide("echo pre-42"), "echo •••");
        let blank = Scope::new(&[&[secret("token", "{{nothing}}"), var("nothing", "")]]);
        assert_eq!(blank.hide("text"), "text", "what fills in to nothing masks nothing");
        let unknown = Scope::new(&[&[secret("token", "a{{missing}}")]]);
        assert_eq!(unknown.hide("a{{missing}}"), "•••", "as written, when it does not fill in");
    }

    fn scope() -> Scope {
        stand().agent_request(&ask("GET", "{{baseUrl}}/x", &[]), &reach()).unwrap().1
    }

    fn response(headers: &[(&str, &str)]) -> Outcome {
        let headers = headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        Outcome::Response { status: 200, headers, url: "https://api.test/v1/x?key=s3cr3t".into(), redirects: vec![], elapsed_ms: 5 }
    }

    #[test]
    fn a_reply_has_no_set_cookie_and_no_secret_in_its_headers_or_where_it_came_from() {
        let reply = AgentReply::new(response(&[("Set-Cookie", "a=1"), ("set-cookie", "b=2"), ("X-Echo", "Bearer s3cr3t"), ("X-s3cr3t", "1")]), b"", &scope());
        let Outcome::Response { headers, url, .. } = reply.outcome else { panic!("a response") };
        assert_eq!(headers, [("X-Echo".to_string(), "Bearer •••".to_string()), ("X-•••".to_string(), "1".to_string())]);
        assert_eq!(url, "https://api.test/v1/x?key=•••");
    }

    #[test]
    fn a_failure_keeps_its_words_without_the_secret() {
        let reply = AgentReply::new(Outcome::Failed { error: "no route to ?key=s3cr3t".into() }, b"", &scope());
        assert_eq!(reply.outcome, Outcome::Failed { error: "no route to ?key=•••".into() });
    }

    #[test]
    fn a_body_comes_masked_json_with_its_indents_and_the_rest_as_it_was() {
        let body = |text: &[u8]| AgentReply::new(response(&[]), text, &scope()).body;
        assert_eq!(body(b"echo s3cr3t {"), "echo ••• {".as_bytes());
        assert_eq!(body(br#"{"b":1,"a":"s3cr3t"}"#), "{\n  \"b\": 1,\n  \"a\": \"•••\"\n}".as_bytes());
        assert_eq!(body(&[0xff, 0x00, 0xfe]), [0xff, 0x00, 0xfe], "not text: never shown, only measured");
        assert_eq!(AgentReply::new(response(&[]), br#"{"a":1}"#, &scope()).size, 7, "what the stand sent, not the indents");
    }

    #[test]
    fn a_secret_escaped_in_json_is_found_after_the_escape_is_read() {
        let slashed = Scope::new(&[&[secret("token", "a/b")]]);
        assert_eq!(AgentReply::new(response(&[]), br#"{"t":"a\/b"}"#, &slashed).body, "{\n  \"t\": \"•••\"\n}".as_bytes());
        let quoted = Scope::new(&[&[secret("token", "a\"b")]]);
        assert_eq!(AgentReply::new(response(&[]), br#"{"t":"a\"b"}"#, &quoted).body, "{\n  \"t\": \"•••\"\n}".as_bytes());
        assert_eq!(quoted.hide(r#"not json: a\"b"#), "not json: •••");
    }

    #[test]
    fn access_is_off_in_a_file_that_does_not_say_and_the_window_keeps_what_it_set() {
        let read: Environment = serde_json::from_value(json!({"id": "e", "name": "n", "variables": []})).unwrap();
        assert_eq!(read.agent, AgentAccess::Off);
        assert!(serde_json::to_value(&read).unwrap().get("agent").is_none());
        let open = serde_json::to_value(stand()).unwrap();
        assert_eq!(open["agent"], "readOnly");
        assert_eq!(stand().for_window().agent, AgentAccess::ReadOnly);
        let closed = Environment { agent: AgentAccess::Off, ..stand().for_window() };
        assert_eq!(stand().edited(closed).unwrap().agent, AgentAccess::Off);
    }
}
