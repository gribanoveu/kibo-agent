//! The HTTP client's rules (`docs/45-http-client.md`): variables and their
//! `{{name}}`, an environment and what the window sees of it, a request as it
//! leaves, a JSON body's comments, a pasted cURL, and what the agent may send
//! into an environment and read back. No I/O — `postman.rs` reads collections.

use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, LazyLock};

use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
use regex::Regex;
use super::http_draft::{AgentDraft, ScriptDraft};
use super::http_scripts::ScriptReport;
use super::postman::{address_parts, Collection};
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
    #[error("`path` is the path alone, from one `/`: {0} — the host is the saved request's; put a query in `query`")]
    BadPath(String),
    #[error("{0} leaves the saved request's host — the host is the template's: send from a request of the collections that goes there")]
    OtherHost(String),
    #[error("`variables` may not set {name}: {why}")]
    VariableRefused { name: String, why: &'static str },
    #[error("`auth` takes only \"none\": {0} — to send a wrong token, set the `Authorization` header")]
    BadAuth(String),
    #[error("the host is the url's — send no Host header")]
    HostHeader,
    #[error("a header's name is written out: {0} — no `{{{{name}}}}` in it")]
    HeaderName(String),
    #[error("`name` is what the user will find the case by on the HTTP page — give one")]
    NoCaseName,
    #[error("folder \"{0}\" signs in or runs scripts of its own, so a case in it would not go as the request it was made from — name another folder, or leave `folder` out")]
    CaseFolder(String),
    #[error("not a cURL command Kibo reads: {0}")]
    BadCurl(String),
    #[error("no environment {0}")]
    NoEnvironment(String),
    #[error("no saved secret {0} in this environment")]
    NoSecret(String),
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
    #[error("no request {0} in the user's collections — the ids are in httpEnvironment's description")]
    NoRequest(String),
    #[error("request {request} is in \"{collection}\", which sends nowhere with \"{environment}\": its requests are not open to the agent there")]
    SendsNowhere { request: String, collection: String, environment: String },
    #[error("`listen` is \"prerequest\" or \"test\", not \"{0}\"")]
    BadListen(String),
    #[error("no send of request {0} from this chat to run a post-response draft on — send it with httpSend first")]
    NoExchange(String),
    #[error("exchange {exchange}: {why}")]
    BadExchange { exchange: String, why: &'static str },
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

/// Headers a server may read as the method in place of the request line's.
const METHOD_OVERRIDES: [&str; 3] = ["x-http-method-override", "x-http-method", "x-method-override"];

/// What a request does to a server that honours a method override: a GET with
/// `X-HTTP-Method-Override: DELETE` deletes. An override that names a read, or
/// nothing, leaves the request line's method.
pub fn acts_as<'a>(method: &str, headers: impl IntoIterator<Item = (&'a str, &'a str)>) -> String {
    headers
        .into_iter()
        .filter(|(name, _)| METHOD_OVERRIDES.iter().any(|o| name.trim().eq_ignore_ascii_case(o)))
        .map(|(_, value)| value.trim().to_ascii_uppercase())
        .find(|value| !value.is_empty() && !is_read(value))
        .unwrap_or_else(|| method.trim().to_ascii_uppercase())
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

/// What the model asked `httpSend` for (HC-10c): a saved request by its id, the
/// edits over it, and values of its own for `{{name}}` — the `local` layer, over every other.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentSend {
    pub request: String,
    pub edits: AgentEdits,
    pub variables: Vec<(String, String)>,
}

/// The model's changes to a saved request, written as the collections write: `{{name}}` stays.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentEdits {
    pub method: Option<String>,
    /// What follows the part of the address that names the host, from one `/`.
    pub path: Option<String>,
    /// The whole query, in place of the template's.
    pub query: Option<String>,
    /// By name, any case; `None` takes the header away.
    pub headers: Vec<(String, Option<String>)>,
    /// The whole body, as raw text.
    pub body: Option<String>,
    /// No auth: neither the request's own nor what it inherits, `oauth2` too.
    pub no_auth: bool,
}

/// An agent's send worked out before the network — all that is refused is refused here.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentPlan {
    /// Which of the collections given the template is in.
    pub collection: usize,
    /// The template's `request` with the edits, to send in its place.
    pub request: Value,
    /// `Shop / Create order`.
    pub name: String,
    /// The method on the request line, and the one it acts as (`acts_as`).
    pub method: String,
    pub acts: String,
    /// The address as written, edits and all — `{{baseUrl}}/orders/2`.
    pub url: String,
    /// Names the collections write where a credential goes: the reply's mask hides them.
    pub credentials: BTreeSet<String>,
}

/// An edited request's method, upper case, `GET` when it names none, and its address as written.
fn method_and_url(request: &Value) -> (String, String) {
    let method = request.get("method").and_then(Value::as_str).map_or_else(|| "GET".to_string(), |m| m.trim().to_ascii_uppercase());
    let url = super::postman::raw_url(request.get("url"));
    (method, url)
}

/// `httpSaveRequest` (HC-10e): a case kept as a new request — the saved request
/// `request` with `edits`, under `name`, in `folder` of the template's collection.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AgentSave {
    pub request: String,
    pub name: String,
    pub folder: Option<String>,
    pub edits: AgentEdits,
}

/// A save worked out before anything is written; where it goes is read from the
/// collection as it is written to (`Collection::case_place`).
#[derive(Debug, Clone, PartialEq)]
pub struct SavePlan {
    /// Which of the collections given the template — and so the case — is in.
    pub collection: usize,
    /// The new item's id.
    pub id: String,
    /// The new item: its id, the name, the request; no scripts, no examples.
    pub item: Value,
    pub method: String,
    /// As written, with no query — a credential may be written there — and a password masked.
    pub url: String,
}

/// What `httpSaveRequest` did: the new request's id, to send it by, and where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSaved {
    pub id: String,
    pub place: String,
    pub method: String,
    pub url: String,
}

/// What `httpSend` brought back: the reply and the scripts' report, both under the agent's mask.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSent {
    pub method: String,
    /// As written, masked.
    pub url: String,
    pub reply: AgentReply,
    pub scripts: ScriptReport,
    /// Its entry in history, which a draft runs on (HC-11c); none when history was not written.
    pub exchange: Option<String>,
}

/// A draft's run worked out (HC-11c): where the request is, and the variables as the
/// model sees them — a hidden one as `•••`, its value nowhere in the sandbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftPlan {
    /// Which of the collections given the request is in.
    pub collection: usize,
    /// `[globals, the collection's, the environment's]`, as the model sees them.
    pub layers: [Vec<Variable>; 3],
    /// Where the collections' requests go with the environment.
    pub origins: BTreeSet<String>,
}

impl DraftPlan {
    /// An address none of the collections' requests goes to — or none at all.
    pub fn foreign(&self, url: &str) -> bool {
        origin_of(url).is_none_or(|origin| !self.origins.contains(&origin))
    }
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
    /// `Collection / Request: METHOD url (id …)`, the url as written, without its
    /// query — of the collections that send somewhere with the environment.
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
            let (mut sends, mut lines) = (false, Vec::new());
            for listed in collection.requests() {
                // An address with a secret in it could not be named to the model.
                if let Ok(filled) = scope.fill(&listed.url) {
                    if let Some(origin) = origin_of(&filled.sent).filter(|o| origin_of(&filled.shown).as_ref() == Some(o)) {
                        reach.origins.insert(origin);
                        sends = true;
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
                let (url, _) = super::postman::userinfo_masked(url);
                lines.push(format!("{} / {}: {} {url} (id {})", collection.name(), listed.name, listed.method, listed.id));
            }
            // A collection for another service is not what the user opened this environment for.
            if sends {
                reach.requests.extend(lines);
            }
        }
        reach
    }

    /// Whether the description lists request `id`: what the agent may read and run.
    pub fn lists(&self, id: &str) -> bool {
        let tail = format!(" (id {id})");
        self.requests.iter().any(|line| line.ends_with(&tail))
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

/// `value` unless it holds text of its own besides `{{name}}`s — with `scheme`, besides
/// a first word such as `Bearer` too: then a secret is written out in the collection,
/// and the agent gets the mask in its place (`Bearer •••`).
pub fn literal_masked(value: &str, scheme: bool) -> String {
    let trimmed = value.trim();
    let (word, rest) = match trimmed.split_once(char::is_whitespace) {
        Some((word, rest)) if scheme && word.chars().all(|c| c.is_ascii_alphabetic()) => (Some(word), rest),
        _ => (None, trimmed),
    };
    if VARIABLE.replace_all(rest, "").trim().is_empty() {
        return value.to_string();
    }
    word.map_or_else(|| SECRET_MASK.to_string(), |word| format!("{word} {SECRET_MASK}"))
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

/// The names whose values the agent never reads, over all `layers`: a secret, a
/// credential by its name or because the collections write it where one goes, and
/// whatever the value of one of those names.
pub fn agent_hidden(layers: &[&[Variable]], credentials: &BTreeSet<String>) -> BTreeSet<String> {
    let marked = layers.iter().flat_map(|l| l.iter()).filter(|v| v.secret || is_credential_name(&v.key)).map(|v| v.key.clone());
    named_by(marked.chain(credentials.iter().cloned()), layers)
}

/// `seed`, and every name a value of one of those writes, at any depth.
fn named_by(seed: impl IntoIterator<Item = String>, layers: &[&[Variable]]) -> BTreeSet<String> {
    let mut names: BTreeSet<String> = seed.into_iter().collect();
    loop {
        let named = layers.iter().flat_map(|l| l.iter()).filter(|v| names.contains(&v.key)).flat_map(|v| names_in(&v.value));
        let more: Vec<String> = named.filter(|n| !names.contains(*n)).map(String::from).collect();
        if more.is_empty() {
            return names;
        }
        names.extend(more);
    }
}

/// What a credential's header or query parameter carried as sent, written out or
/// filled in: its last word — `Bearer` is no secret — from 4 characters, as a
/// shorter one would mask every `1` in a reply.
pub fn credential_literals(sent: &Outgoing) -> Vec<String> {
    let headers = sent.headers.iter().filter(|(k, _)| is_credential_name(k)).filter_map(|(_, v)| v.split_whitespace().last()).map(String::from);
    let query: Vec<String> = Url::parse(&sent.url).map(|u| u.query_pairs().filter(|(k, _)| is_credential_name(k)).map(|(_, v)| v.into_owned()).collect()).unwrap_or_default();
    headers.chain(query).filter(|v| v.chars().count() >= 4).collect()
}

/// `variables` with those named in `hidden`, or named as a credential, made secrets.
pub fn hiding(variables: &[Variable], hidden: &BTreeSet<String>) -> Vec<Variable> {
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

/// The environments as the agent's `http*` tools reach them.
/// `services::http_client::Stands` over the stores; a test's own in memory.
pub trait HttpStands: Send + Sync {
    /// The ones not off: another is not named to the model, in a list or an error.
    fn open(&self) -> Result<Vec<OpenStand>, HttpError>;
    fn describe(&self, id: &str) -> Result<StandView, HttpError>;
    /// The method a send acts as and what it is — `POST {{baseUrl}}/orders, from "Shop / Create
    /// order"` — for its approval, before anything is sent. Refused as the send would be.
    fn sending(&self, id: &str, send: &AgentSend) -> Result<(String, String), HttpError>;
    fn send(&self, id: &str, send: &AgentSend) -> Result<AgentSent, HttpError>;
    /// The saved request `request` as environment `id` lets the agent read it (HC-10b).
    fn read(&self, id: &str, request: &str) -> Result<String, HttpError>;
    /// A case kept as a new request in the template's collection (HC-10e).
    fn save(&self, id: &str, save: &AgentSave) -> Result<AgentSaved, HttpError>;
    /// A script the model drafts, run with nothing sent, and kept for the page (HC-11c).
    fn draft(&self, id: &str, draft: &AgentDraft) -> Result<ScriptDraft, HttpError>;
}

/// History gained an agent's send, or a collection an agent's case, for the page to read again.
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
        hiding(&self.variables, &agent_hidden(&[&self.variables], credentials))
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

    /// A saved request as the agent reads it (HC-10b), masked with all this environment
    /// hides from it. Only one the description lists: a collection that sends nowhere
    /// with this environment is not part of what the user opened.
    pub fn read_request(&self, collections: &[(&Collection, Vec<Variable>)], globals: &[Variable], request: &str) -> Result<String, HttpError> {
        if self.agent.is_off() {
            return Err(HttpError::NoneOpen);
        }
        let Some((collection, variables)) = collections.iter().find(|(c, _)| c.request_ids().iter().any(|id| id == request)) else {
            return Err(HttpError::NoRequest(request.into()));
        };
        let reach = Reach::new(collections, globals, self);
        if !reach.lists(request) {
            return Err(HttpError::SendsNowhere { request: request.into(), collection: collection.name(), environment: self.name.clone() });
        }
        let mine = self.for_agent(&reach.credentials);
        let scope = Scope::new(&[&hiding(globals, &reach.credentials), &hiding(variables, &reach.credentials), &mine]);
        collection.read_for_agent(request, scope)
    }

    /// An agent's send (HC-10c): the saved request `send.request` with the model's
    /// edits, worked out against the user's collections and refused before the
    /// network — the environment off or read only to a write, a request it does not
    /// list, a `variables` entry that is hidden or decides where a request or a token
    /// goes, an address that leaves the template's host.
    pub fn agent_send(&self, collections: &[(&Collection, Vec<Variable>)], globals: &[Variable], send: &AgentSend) -> Result<AgentPlan, HttpError> {
        if self.agent.is_off() {
            return Err(HttpError::NoneOpen);
        }
        let id = send.request.trim();
        let Some(at) = collections.iter().position(|(c, _)| c.request_ids().iter().any(|r| r == id)) else {
            return Err(HttpError::NoRequest(id.into()));
        };
        let (collection, variables) = &collections[at];
        let reach = Reach::new(collections, globals, self);
        if !reach.lists(id) {
            return Err(HttpError::SendsNowhere { request: id.into(), collection: collection.name(), environment: self.name.clone() });
        }
        let request = collection.edited_for_agent(id, &send.edits)?;
        let (method, url) = method_and_url(&request);
        let rows = request.get("header").and_then(Value::as_array).into_iter().flatten().filter(|h| h.get("disabled") != Some(&Value::Bool(true)));
        let headers: Vec<(&str, &str)> = rows.filter_map(|h| Some((h.get("key")?.as_str()?, h.get("value")?.as_str()?))).collect();
        let acts = acts_as(&method, headers);
        if self.agent == AgentAccess::ReadOnly && !is_read(&acts) {
            return Err(HttpError::ReadOnly { environment: self.name.clone(), method: acts });
        }

        let layers: [&[Variable]; 3] = [globals, variables, &self.variables];
        let hidden = agent_hidden(&layers, &reach.credentials);
        let hosts = collections.iter().flat_map(|(c, _)| c.requests()).flat_map(|r| names_in(&address_parts(&r.url).0).map(String::from).collect::<Vec<_>>());
        let hosts = named_by(hosts, &layers);
        let auth = named_by(collections.iter().flat_map(|(c, _)| c.auth_names()), &layers);
        // A name filled in could be `Host` or a method override, which `acts` did not see.
        let keyed = request.get("header").and_then(Value::as_array).into_iter().flatten().filter_map(|h| h.get("key")?.as_str()).flat_map(|k| names_in(k).map(String::from).collect::<Vec<_>>());
        let keyed = named_by(keyed, &layers);
        for (name, _) in &send.variables {
            let why = match () {
                _ if hidden.contains(name) => "it is hidden",
                _ if hosts.contains(name) => "it decides where the collections' requests go",
                _ if auth.contains(name) => "it is part of how a request signs in",
                _ if keyed.contains(name) => "it names a header",
                _ => continue,
            };
            return Err(HttpError::VariableRefused { name: name.clone(), why });
        }

        // The host is the template's as the page fills it in, the model's own values aside.
        let written = collection.written_url(id).unwrap_or_default();
        let local: Vec<Variable> = send.variables.iter().map(|(key, value)| Variable { key: key.clone(), value: value.clone(), secret: false, enabled: true }).collect();
        let fill = |scope: &Scope, text: &str| scope.fill(text).map(|f| f.sent).map_err(|e| self.unknown_named(e));
        let theirs = origin_of(&fill(&Scope::new(&layers), &address_parts(&written).0)?).ok_or_else(|| HttpError::NotAnAddress(super::postman::userinfo_masked(&written).0))?;
        let ours = origin_of(&fill(&Scope::new(&[globals, variables, &self.variables, &local]), &url)?);
        if ours.as_ref() != Some(&theirs) {
            return Err(HttpError::OtherHost(super::postman::userinfo_masked(&url).0));
        }
        let url = super::postman::userinfo_masked(&url).0;
        let name = format!("{} / {}", collection.name(), collection.item_name(id).unwrap_or_default());
        Ok(AgentPlan { collection: at, request, name, method, acts, url, credentials: reach.credentials })
    }

    /// A draft's run (HC-11c), worked out against the user's collections: the request
    /// one this environment lists, and its variables as the model sees them. Nothing
    /// is sent, so the environment's mode does not count.
    pub fn agent_draft(&self, collections: &[(&Collection, Vec<Variable>)], globals: &[Variable], request: &str) -> Result<DraftPlan, HttpError> {
        if self.agent.is_off() {
            return Err(HttpError::NoneOpen);
        }
        let id = request.trim();
        let Some(at) = collections.iter().position(|(c, _)| c.request_ids().iter().any(|r| r == id)) else {
            return Err(HttpError::NoRequest(id.into()));
        };
        let (collection, variables) = &collections[at];
        let reach = Reach::new(collections, globals, self);
        if !reach.lists(id) {
            return Err(HttpError::SendsNowhere { request: id.into(), collection: collection.name(), environment: self.name.clone() });
        }
        let layers: [&[Variable]; 3] = [globals, variables, &self.variables];
        let hidden = agent_hidden(&layers, &reach.credentials);
        let seen = |layer: &[Variable]| hiding(layer, &hidden).iter().map(Variable::for_window).collect();
        Ok(DraftPlan { collection: at, layers: layers.map(seen), origins: reach.origins })
    }

    /// A case to keep (HC-10e), worked out against the user's collections: the
    /// template one this environment lists, the edits as `httpSend` takes them — the
    /// host stays the template's, as written — and the place in the template's own
    /// collection, where `{{baseUrl}}` means what it meant for the template. Nothing
    /// is sent, so the environment's mode does not count: any open one lets a case be kept.
    pub fn agent_save(&self, collections: &[(&Collection, Vec<Variable>)], globals: &[Variable], save: &AgentSave) -> Result<SavePlan, HttpError> {
        if self.agent.is_off() {
            return Err(HttpError::NoneOpen);
        }
        let id = save.request.trim();
        let Some(at) = collections.iter().position(|(c, _)| c.request_ids().iter().any(|r| r == id)) else {
            return Err(HttpError::NoRequest(id.into()));
        };
        let collection = collections[at].0;
        if !Reach::new(collections, globals, self).lists(id) {
            return Err(HttpError::SendsNowhere { request: id.into(), collection: collection.name(), environment: self.name.clone() });
        }
        let name = save.name.trim();
        if name.is_empty() {
            return Err(HttpError::NoCaseName);
        }
        let request = collection.edited_for_agent(id, &save.edits)?;
        let (method, url) = method_and_url(&request);
        let case = uuid::Uuid::new_v4().to_string();
        let item = json!({"id": case, "name": name, "request": request});
        let url = super::postman::userinfo_masked(url.split('?').next().unwrap_or_default()).0;
        Ok(SavePlan { collection: at, id: case, item, method, url })
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
pub(crate) static VARIABLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\{\{\s*([^{}\s]+)\s*\}\}").expect("valid regex"));

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
    pub(crate) fn keeping<'a>(mut self, values: impl IntoIterator<Item = &'a str>) -> Scope {
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
    /// The chat whose agent sent it, by its title; none when the page did.
    pub agent_chat: Option<String>,
    /// That chat's id: a title is neither one of a kind nor kept. What a draft's
    /// run finds the chat's sends by (HC-11c); none in an entry from before it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_chat_id: Option<String>,
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
        HistoryEntry { id: uuid::Uuid::new_v4().to_string(), at, environment, agent_chat, agent_chat_id: None, template_url, request_id: None, request, outcome, response_body }
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

    fn stand() -> Environment {
        opened(vec![var("baseUrl", "https://api.test/v1"), secret("token", "s3cr3t"), var("id", "42")])
    }

    /// Where a collection of `https://api.test` requests sends.
    fn reach() -> Reach {
        Reach { origins: ["https://api.test".to_string()].into(), ..Reach::default() }
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

    /// Shop under a bearer of `{{token}}`: `r`, a GET of `{{baseUrl}}/orders/:n?expand=1`
    /// with `X-Id: {{id}}` and `X-Trace: t`; `w`, a POST of `{}` to `{{baseUrl}}/orders`;
    /// `s`, a url alone; `o`, under an `oauth2` whose token comes from `{{authHost}}`;
    /// `t`, at `https://{{tenant}}.api.test`.
    fn shop() -> Collection {
        collection(
            json!([
                {"id": "r", "name": "Get one", "request": {"method": "GET", "url": {"raw": "{{baseUrl}}/orders/:n?expand=1", "variable": [{"key": "n", "value": "1"}]}, "header": [{"key": "X-Id", "value": "{{id}}"}, {"key": "x-trace", "value": "t"}]}},
                {"id": "w", "name": "Create", "request": {"method": "POST", "url": "{{baseUrl}}/orders", "body": {"mode": "raw", "raw": "{}"}}},
                {"id": "s", "name": "Plain", "request": "{{baseUrl}}/plain"},
                {"id": "o", "name": "Signed", "request": {"url": "{{baseUrl}}/me", "auth": {"type": "oauth2", "oauth2": [{"key": "accessTokenUrl", "value": "{{authHost}}/token"}]}}},
                {"id": "t", "name": "Tenant", "request": {"url": "https://{{tenant}}.api.test/x"}},
            ]),
            json!({"type": "bearer", "bearer": [{"key": "token", "value": "{{token}}"}]}),
            json!([{"key": "tenant", "value": "acme"}]),
        )
    }

    fn plan(environment: &Environment, send: AgentSend) -> Result<AgentPlan, HttpError> {
        let shop = shop();
        environment.agent_send(&[(&shop, shop.variables())], &[], &send)
    }

    fn of(request: &str, edits: AgentEdits) -> AgentSend {
        AgentSend { request: request.into(), edits, variables: vec![] }
    }

    fn with(request: &str, variables: &[(&str, &str)]) -> AgentSend {
        AgentSend { request: request.into(), edits: AgentEdits::default(), variables: variables.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect() }
    }

    fn writable() -> Environment {
        Environment { agent: AgentAccess::AskForWrites, ..stand() }
    }

    #[test]
    fn an_environment_off_takes_nothing_and_says_nothing_of_itself() {
        let off = Environment { agent: AgentAccess::Off, ..stand() };
        assert_eq!(plan(&off, of("r", AgentEdits::default())), Err(HttpError::NoneOpen));
        assert_eq!(off.described(&reach()), Err(HttpError::NoneOpen));
    }

    #[test]
    fn a_request_not_listed_is_not_sent() {
        assert_eq!(plan(&stand(), of("nope", AgentEdits::default())), Err(HttpError::NoRequest("nope".into())));
        let elsewhere = collection(json!([get("x", json!("ftp://files.test/x"))]), json!(null), json!(null));
        let told = stand().agent_send(&[(&elsewhere, vec![])], &[], &of("x", AgentEdits::default())).unwrap_err();
        assert_eq!(told, HttpError::SendsNowhere { request: "x".into(), collection: "Shop".into(), environment: "Stage".into() });
    }

    /// Read only takes what reads — by the method the send goes with, the template's
    /// unless the call names one, and an override header's over both.
    #[test]
    fn read_only_refuses_a_write_however_it_comes() {
        let sent = plan(&stand(), of("r", AgentEdits::default())).unwrap();
        assert_eq!((sent.method.as_str(), sent.acts.as_str(), sent.name.as_str()), ("GET", "GET", "Shop / Get one"));
        let refused = |method: &str| HttpError::ReadOnly { environment: "Stage".into(), method: method.into() };
        assert_eq!(plan(&stand(), of("w", AgentEdits::default())), Err(refused("POST")), "the template's");
        let posted = AgentEdits { method: Some(" post ".into()), ..AgentEdits::default() };
        assert_eq!(plan(&stand(), of("r", posted)), Err(refused("POST")), "the call's");
        let overridden = AgentEdits { headers: vec![("X-HTTP-Method-Override".into(), Some("delete".into()))], ..AgentEdits::default() };
        assert_eq!(plan(&stand(), of("r", overridden)), Err(refused("DELETE")), "a GET that deletes");
        let read = AgentEdits { method: Some("head".into()), ..AgentEdits::default() };
        assert_eq!(plan(&stand(), of("w", read)).unwrap().acts, "HEAD", "a read of a POST's address");
        for access in [AgentAccess::AskForWrites, AgentAccess::WritesAsOtherTools] {
            assert_eq!(plan(&Environment { agent: access, ..stand() }, of("w", AgentEdits::default())).unwrap().acts, "POST", "{access:?}");
        }
        let off = json!({"key": "X-HTTP-Method-Override", "value": "DELETE", "disabled": true});
        let shop = collection(json!([{"id": "d", "name": "D", "request": {"url": "{{baseUrl}}/d", "header": [off]}}]), json!(null), json!([]));
        let sent = stand().agent_send(&[(&shop, vec![])], &[], &of("d", AgentEdits::default()));
        assert_eq!(sent.map(|s| s.acts), Ok("GET".into()), "an override switched off does not go");
    }

    /// Each edit lands where the page reads it: the method, the path after the host's
    /// part with `:n` still filled, the whole query, headers by name in any case, the
    /// body as raw text — JSON with its language — and no auth.
    #[test]
    fn the_edits_are_written_into_the_saved_request() {
        let edits = AgentEdits {
            method: Some("put".into()),
            path: Some(" /orders/:n/{{orderId}} ".into()),
            query: Some("?a=1&b={{id}}".into()),
            headers: vec![("x-id".into(), Some("{{orderId}}".into())), ("X-TRACE".into(), None), ("Accept".into(), Some("text/plain".into()))],
            body: Some("{\"count\": {{n}}}".into()),
            no_auth: true,
        };
        let mut send = of("r", edits);
        send.variables = vec![("orderId".into(), "7".into())];
        let sent = plan(&writable(), send).unwrap();
        assert_eq!(
            sent.request,
            json!({
                "method": "PUT",
                "url": {"raw": "{{baseUrl}}/orders/:n/{{orderId}}?a=1&b={{id}}", "variable": [{"key": "n", "value": "1"}]},
                "header": [{"key": "x-id", "value": "{{orderId}}"}, {"key": "Accept", "value": "text/plain"}],
                "body": {"mode": "raw", "raw": "{\"count\": {{n}}}", "options": {"raw": {"language": "json"}}},
                "auth": {"type": "noauth"},
            })
        );
        assert_eq!((sent.method.as_str(), sent.url.as_str()), ("PUT", "{{baseUrl}}/orders/:n/{{orderId}}?a=1&b={{id}}"));

        let text = AgentEdits { body: Some("a=1".into()), query: Some(String::new()), ..AgentEdits::default() };
        let sent = plan(&writable(), of("w", text)).unwrap();
        assert_eq!((&sent.request["body"], &sent.request["url"]), (&json!({"mode": "raw", "raw": "a=1"}), &json!("{{baseUrl}}/orders")), "no type for text; an empty query is none");
        let plain = plan(&stand(), of("s", AgentEdits { path: Some("/other".into()), ..AgentEdits::default() })).unwrap();
        assert_eq!(plain.request, json!({"method": "GET", "url": "{{baseUrl}}/other"}), "a request written as its url alone");
        assert_eq!(plan(&stand(), of("r", AgentEdits::default())).unwrap().request, shop().json()["item"][0]["request"], "nothing asked, nothing changed");
    }

    /// The host is the saved request's: a path is from one `/` and starts no address of
    /// its own, and a header cannot name another host.
    #[test]
    fn a_path_cannot_leave_the_host() {
        for path in ["orders", "//evil.example/x", "/\\evil.example", "/x?y=1", "/x#top", "@evil.example/x"] {
            let edits = AgentEdits { path: Some(path.into()), ..AgentEdits::default() };
            assert_eq!(plan(&stand(), of("r", edits)), Err(HttpError::BadPath(path.into())), "{path}");
        }
        let at = plan(&stand(), with("r", &[("p", "@evil.example")]).tap(|s| s.edits.path = Some("/{{p}}".into()))).unwrap();
        assert_eq!(at.url, "{{baseUrl}}/{{p}}?expand=1", "after the host's part, `@` is in the path");
        let host = AgentEdits { headers: vec![("hOsT".into(), Some("evil.example".into()))], ..AgentEdits::default() };
        assert_eq!(plan(&stand(), of("r", host)), Err(HttpError::HostHeader));
        let named = AgentEdits { headers: vec![("{{h}}".into(), Some("DELETE".into()))], ..AgentEdits::default() };
        assert_eq!(plan(&stand(), with("r", &[("h", "X-HTTP-Method-Override")]).tap(|s| s.edits = named)), Err(HttpError::HeaderName("{{h}}".into())));
        assert_eq!(
            HttpError::BadPath("//evil".into()).to_string(),
            "`path` is the path alone, from one `/`: //evil — the host is the saved request's; put a query in `query`"
        );
    }

    /// `variables` is for what a case changes, not for where a request goes, how it
    /// signs in, or what the agent may not read.
    #[test]
    fn a_variable_that_decides_a_host_an_auth_or_is_hidden_is_refused() {
        let mut environment = stand();
        environment.variables.push(var("baseUrl2", "https://{{region}}.api.test"));
        environment.variables.push(var("region", "eu"));
        environment.variables.push(var("apiKey", "k"));
        let shop = collection(
            shop().json()["item"].clone().as_array().unwrap().iter().cloned().chain([get("u", json!("{{baseUrl2}}/x"))]).collect(),
            json!({"type": "bearer", "bearer": [{"key": "token", "value": "{{token}}"}]}),
            json!([{"key": "tenant", "value": "acme"}]),
        );
        let keyed = json!({"id": "k", "name": "Keyed", "request": {"url": "{{baseUrl}}/k", "header": [{"key": "{{hdr}}", "value": "DELETE"}]}});
        let shop = collection(shop.json()["item"].as_array().unwrap().iter().cloned().chain([keyed]).collect(), shop.json()["auth"].clone(), shop.json()["variable"].clone());
        let refused = |name: &str| shop_send(&environment, &shop, with("r", &[(name, "x")])).unwrap_err();
        let why = |name: &str, why: &'static str| HttpError::VariableRefused { name: name.into(), why };
        assert_eq!(refused("token"), why("token", "it is hidden"), "a secret");
        assert_eq!(refused("apiKey"), why("apiKey", "it is hidden"), "a credential by its name");
        assert_eq!(refused("baseUrl"), why("baseUrl", "it decides where the collections' requests go"));
        assert_eq!(refused("tenant"), why("tenant", "it decides where the collections' requests go"), "another request's host");
        assert_eq!(refused("region"), why("region", "it decides where the collections' requests go"), "named by a host's value");
        assert_eq!(refused("authHost"), why("authHost", "it is part of how a request signs in"), "where a token comes from");
        let keyed = shop_send(&environment, &shop, with("k", &[("hdr", "X-HTTP-Method-Override")])).unwrap_err();
        assert_eq!(keyed, why("hdr", "it names a header"), "the sent request's header name: Host or a method override");
        assert_eq!(refused("token").to_string(), "`variables` may not set token: it is hidden");
        let fine = shop_send(&environment, &shop, with("r", &[("id", "7"), ("orderId", "9")]));
        assert!(fine.is_ok(), "{fine:?}");
    }

    fn shop_send(environment: &Environment, shop: &Collection, send: AgentSend) -> Result<AgentPlan, HttpError> {
        environment.agent_send(&[(shop, shop.variables())], &[], &send)
    }

    /// A name found nowhere is said with the names there are, before anything is sent.
    #[test]
    fn a_name_not_there_is_said_before_sending() {
        let edits = AgentEdits { path: Some("/orders/{{nope}}".into()), ..AgentEdits::default() };
        let told = plan(&stand(), of("r", edits)).unwrap_err();
        assert_eq!(told, HttpError::UnknownIn { names: vec!["nope".into()], environment: "Stage".into(), known: "baseUrl, token, id".into() });
        let named = plan(&stand(), with("r", &[("nope", "1")]).tap(|s| s.edits.path = Some("/orders/{{nope}}".into())));
        assert!(named.is_ok(), "the model's own value: {named:?}");
    }

    /// A password written in the saved request's address is not the model's to read.
    #[test]
    fn a_password_in_the_templates_address_is_masked() {
        let shop = collection(json!([{"id": "p", "name": "P", "request": {"url": "https://admin:hunter2@api.test/x"}}]), json!(null), json!([]));
        let mut environment = stand();
        environment.variables.push(var("other", "https://api.test"));
        let sent = environment.agent_send(&[(&shop, vec![])], &[], &of("p", AgentEdits::default())).unwrap();
        assert_eq!(sent.url, "https://admin:•••@api.test/x");
        let kept = environment.agent_save(&[(&shop, vec![])], &[], &AgentSave { request: "p".into(), name: "P".into(), ..AgentSave::default() }).unwrap();
        assert_eq!(kept.url, "https://admin:•••@api.test/x", "and as a save tells it");
    }

    fn save(request: &str, folder: Option<&str>, edits: AgentEdits) -> AgentSave {
        AgentSave { request: request.into(), name: " Order 2, no auth ".into(), folder: folder.map(String::from), edits }
    }

    /// A case is the template with the edits, under its own id and name, with no
    /// scripts or examples; refused as a send would be, and with no name.
    #[test]
    fn a_case_is_the_template_with_the_edits() {
        let saved = |environment: &Environment, s: AgentSave| {
            let shop = shop();
            environment.agent_save(&[(&shop, shop.variables())], &[], &s)
        };
        let edits = AgentEdits { method: Some("delete".into()), path: Some("/orders/2".into()), headers: vec![("X-Id".into(), None)], no_auth: true, ..AgentEdits::default() };
        let plan = saved(&stand(), save("r", None, edits)).unwrap();
        assert_eq!((plan.method.as_str(), plan.url.as_str()), ("DELETE", "{{baseUrl}}/orders/2"), "read only: nothing is sent; no query told");
        assert_eq!(plan.item["name"], "Order 2, no auth");
        assert_eq!(plan.item["request"]["auth"], json!({"type": "noauth"}));
        assert_eq!(plan.item["request"]["header"], json!([{"key": "x-trace", "value": "t"}]));
        assert!(plan.item.get("event").is_none() && plan.item.get("response").is_none());
        let id = plan.item["id"].as_str().unwrap();
        assert_eq!(id, plan.id);
        assert!(super::super::postman::is_safe_id(id) && !shop().ids().contains(&id.to_string()), "{id}");
        assert_eq!(saved(&stand(), AgentSave { name: " ".into(), ..save("r", None, AgentEdits::default()) }), Err(HttpError::NoCaseName));
        let away = AgentEdits { path: Some("//evil.example/x".into()), ..AgentEdits::default() };
        assert_eq!(saved(&stand(), save("r", None, away)), Err(HttpError::BadPath("//evil.example/x".into())), "the host is the template's");
        assert_eq!(saved(&Environment { agent: AgentAccess::Off, ..stand() }, save("r", None, AgentEdits::default())), Err(HttpError::NoneOpen));
        assert_eq!(saved(&stand(), save("nope", None, AgentEdits::default())), Err(HttpError::NoRequest("nope".into())));
        let elsewhere = collection(json!([get("x", json!("ftp://files.test/x"))]), json!(null), json!(null));
        let unlisted = stand().agent_save(&[(&elsewhere, vec![])], &[], &save("x", None, AgentEdits::default()));
        assert_eq!(unlisted, Err(HttpError::SendsNowhere { request: "x".into(), collection: "Shop".into(), environment: "Stage".into() }));
    }

    /// A draft's run (HC-11c): of a request the environment lists, in any of its modes
    /// — nothing is sent — over the variables as the model sees them.
    #[test]
    fn a_draft_runs_on_a_listed_request_over_what_the_model_sees() {
        let shop = shop();
        let drafted = |environment: &Environment, request: &str| environment.agent_draft(&[(&shop, shop.variables())], &[secret("g", "glob4l")], request);
        let plan = drafted(&Environment { agent: AgentAccess::ReadOnly, ..stand() }, " w ").unwrap();
        assert_eq!(plan.collection, 0);
        let [globals, collection_vars, environment] = &plan.layers;
        assert_eq!((globals[0].key.as_str(), globals[0].value.as_str()), ("g", SECRET_MASK));
        assert_eq!(collection_vars.iter().find(|v| v.key == "tenant").map(|v| v.value.as_str()), Some("acme"));
        let values: Vec<(&str, &str)> = environment.iter().map(|v| (v.key.as_str(), v.value.as_str())).collect();
        assert_eq!(values, [("baseUrl", "https://api.test/v1"), ("token", SECRET_MASK), ("id", "42")]);
        assert!(!plan.foreign("https://api.test/other"));
        assert!(plan.foreign("https://evil.test/x") && plan.foreign("ftp://api.test/x"), "a host of none, or no http host");
        assert_eq!(drafted(&Environment { agent: AgentAccess::Off, ..stand() }, "w"), Err(HttpError::NoneOpen));
        assert_eq!(drafted(&stand(), "nope"), Err(HttpError::NoRequest("nope".into())));
        let elsewhere = collection(json!([get("x", json!("ftp://files.test/x"))]), json!(null), json!(null));
        let unlisted = stand().agent_draft(&[(&elsewhere, vec![])], &[], "x");
        assert_eq!(unlisted, Err(HttpError::SendsNowhere { request: "x".into(), collection: "Shop".into(), environment: "Stage".into() }));
    }

    /// Beside the template, or in a folder right inside its own by name — made there
    /// when there is none — so that the case inherits what the template does; a folder
    /// with an auth or scripts of its own is refused.
    #[test]
    fn a_case_goes_beside_its_template_or_in_a_folder_inside_its_own() {
        let shop = collection(
            json!([
                {"id": "f", "name": "Orders", "item": [
                    get("q", json!("{{baseUrl}}/q")),
                    {"id": "s", "name": "Signed", "auth": {"type": "bearer", "bearer": [{"key": "token", "value": "{{t}}"}]}, "item": []},
                    {"id": "e", "name": "Scripted", "event": [{"listen": "prerequest", "script": {"exec": ["login()"]}}], "item": []},
                    {"id": "i", "name": "Inherits", "auth": {"type": "inherit"}, "item": []},
                    {"id": "g", "name": "Deep", "item": [get("d", json!("{{baseUrl}}/d"))]},
                ]},
                {"id": "c", "name": "Cases", "item": []},
                get("top", json!("{{baseUrl}}/t")),
            ]),
            json!(null),
            json!([]),
        );
        let place = |request: &str, folder: Option<&str>| shop.case_place(request, folder);
        let at = |parent: Option<&str>, made: Option<&str>, place: &str| Ok((parent.map(String::from), made.map(String::from), place.to_string()));
        assert_eq!(place("q", None), at(Some("f"), None, "Shop / Orders"));
        assert_eq!(place("q", Some("")), at(Some("f"), None, "Shop / Orders"));
        assert_eq!(place("top", None), at(None, None, "Shop"));
        assert_eq!(place("d", None), at(Some("g"), None, "Shop / Orders / Deep"), "its own folder, not the outer one");
        assert_eq!(place("d", Some("x")), at(Some("g"), Some("x"), "Shop / Orders / Deep / x"));
        assert_eq!(place("top", Some(" cases ")), at(Some("c"), None, "Shop / Cases"), "by name, any case");
        assert_eq!(place("q", Some("inherits")), at(Some("i"), None, "Shop / Orders / Inherits"), "inherit is no auth of its own");
        assert_eq!(place("q", Some("Cases")), at(Some("f"), Some("Cases"), "Shop / Orders / Cases"), "only inside the template's own folder");
        assert_eq!(place("q", Some("q")), at(Some("f"), Some("q"), "Shop / Orders / q"), "a request is no folder");
        assert_eq!(place("q", Some("Signed")), Err(HttpError::CaseFolder("Signed".into())));
        assert_eq!(place("q", Some("scripted")), Err(HttpError::CaseFolder("Scripted".into())));
        assert_eq!(place("nope", None), Err(HttpError::NoSuchItem("nope".into())));
    }

    #[test]
    fn an_address_is_cut_where_the_models_edits_go() {
        let parts = |written: &str| {
            let (host, path, query) = address_parts(written);
            [host, path, query]
        };
        assert_eq!(parts("{{baseUrl}}/orders?x=1#top"), ["{{baseUrl}}", "/orders", "x=1"]);
        assert_eq!(parts("https://api.test:8443/v1/a"), ["https://api.test:8443", "/v1/a", ""]);
        assert_eq!(parts("{{baseUrl}}/go/https://x"), ["{{baseUrl}}", "/go/https://x", ""], "an address in the path is the path's");
        assert_eq!(parts("https://api.test?next=http://a/b"), ["https://api.test", "", "next=http://a/b"]);
        let tenant = plan(&stand(), of("t", AgentEdits { path: Some("/y".into()), ..AgentEdits::default() })).unwrap();
        assert_eq!(tenant.url, "https://{{tenant}}.api.test/y", "a host written out keeps its scheme");
    }

    trait Tap: Sized {
        fn tap(self, f: impl FnOnce(&mut Self)) -> Self;
    }

    impl Tap for AgentSend {
        fn tap(mut self, f: impl FnOnce(&mut Self)) -> Self {
            f(&mut self);
            self
        }
    }

    #[test]
    fn what_went_as_a_credential_is_kept_for_the_mask() {
        let headers = [("authorization", "Bearer pl41n"), ("X-Api-Key", "k3y-1"), ("X-Id", "1d42"), ("X-Token", "ab1")];
        let sent = Outgoing {
            method: "GET".into(),
            url: "https://api.test/x?api_key=q-val&p=pval&token=ab".into(),
            headers: headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            body: None,
            digest: None,
        };
        assert_eq!(credential_literals(&sent), ["pl41n", "k3y-1", "q-val"], "the scheme word, another header, a short value: none");
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
                "Shop / list: GET {{baseUrl}}/orders (id list)",
                "Shop / local: GET localhost:3000/health (id local)",
                "Shop / auth: GET https://auth.test:8443/token (id auth)",
                "Shop / global: GET https://global.test/x (id global)",
                "Shop / unknown: GET https://{{nope}}/x (id unknown)",
                "Shop / hidden: GET https://{{secretHost}}/x (id hidden)",
                "Shop / ftp: GET ftp://files.test/x (id ftp)",
                "Shop / plain: GET https://string.test/x (id plain)",
                "Shop / tenant: GET https://{{tenant}}.api.test/x (id tenant)",
                "Shop / named: GET {{baseUrl}}/{{sessionId}}/{{$guid}} (id named)",
            ],
            "the method in capitals, no query, the id last; the environment's names and hidden ones left for the agent"
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

        let hidden = agent_hidden(&[&environment.variables], &reach.credentials);
        assert!(hidden.contains("apiKey") && hidden.contains("deep") && !hidden.contains("plain"), "{hidden:?}");
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

    /// Shop: a folder Orders with a bearer written out and a script, its request
    /// `r` with credentials written out and a script holding the token's value; `own`
    /// with an auth of its own; `top` under the collection's `oauth2`.
    fn saved() -> Collection {
        collection(
            json!([{
                "id": "f", "name": "Orders",
                "auth": {"type": "bearer", "bearer": [{"key": "token", "value": "eyJliteral"}]},
                "event": [{"listen": "prerequest", "script": {"exec": ["pm.environment.set('seen', 1);"]}}],
                "item": [{
                    "id": "r", "name": "Get one",
                    "event": [
                        {"listen": "test", "script": {"exec": ["pm.test('ok', () => {", "  pm.response.to.have.status(200);", "});"]}},
                        {"listen": "prerequest", "script": {"exec": ["const k = 's3cr3t';"]}}
                    ],
                    "request": {
                        "method": "post",
                        "url": {"raw": "{{baseUrl}}/orders/:id?api_key=abc123&page=2", "query": [{"key": "api_key", "value": "abc123"}, {"key": "page", "value": "2", "disabled": true}], "variable": [{"key": "id", "value": "{{orderId}}"}]},
                        "header": [{"key": "Authorization", "value": "Bearer eyJliteral"}, {"key": "X-Token", "value": "{{token}}"}, {"key": "Accept", "value": "application/json"}],
                        "body": {"mode": "raw", "raw": "{\"name\": \"x\", \"password\": \"hunter2\", \"apiKey\": \"{{apiKey}}\"}", "options": {"raw": {"language": "json"}}}
                    }
                }, {
                    "id": "own", "name": "Own",
                    "request": {"url": "{{baseUrl}}/x?token=t0k3n", "auth": {"type": "basic", "basic": [{"key": "username", "value": "admin"}, {"key": "password", "value": "{{pass}}"}]}}
                }]
            }, get("top", json!("{{baseUrl}}/top"))]),
            json!({"type": "oauth2", "oauth2": [{"key": "accessTokenUrl", "value": "https://auth.test/token"}, {"key": "clientId", "value": "{{clientId}}"}, {"key": "clientSecret", "value": "literal-cs"}, {"key": "grant_type", "value": "client_credentials"}, {"key": "addTokenTo", "value": "header"}, {"key": "useBrowser", "value": false}]}),
            json!(null),
        )
    }

    fn read(environment: &Environment, collections: &[&Collection], request: &str) -> Result<String, HttpError> {
        let layers: Vec<(&Collection, Vec<Variable>)> = collections.iter().map(|c| (*c, c.variables())).collect();
        environment.read_request(&layers, &[], request)
    }

    #[test]
    fn a_saved_request_is_read_as_written_its_credentials_masked() {
        let shop = saved();
        assert_eq!(
            read(&stand(), &[&shop], "r").unwrap(),
            r#"Request "Shop / Orders / Get one", id r: POST {{baseUrl}}/orders/:id
Path variables:
  id: {{orderId}}
Query:
  api_key: •••
  page: 2 (off)
Headers:
  Authorization: Bearer •••
  X-Token: {{token}}
  Accept: application/json
Body, raw json:
{
  "name": "x",
  "password": "•••",
  "apiKey": "{{apiKey}}"
}
Auth: bearer, inherited from folder "Orders"
  token = •••
Variables it uses:
  baseUrl = https://api.test/v1
  orderId — not set
  token — hidden: filled in on sending, never shown
  apiKey — not set
Scripts, in the order they run:
  Pre-request script of folder "Orders":
    pm.environment.set('seen', 1);
  Pre-request script of the request: hidden — it holds a secret
  Test script of the request:
    pm.test('ok', () => {
      pm.response.to.have.status(200);
    });"#
        );
    }

    #[test]
    fn auth_is_named_with_the_level_it_comes_from_and_its_settings_shown() {
        let shop = saved();
        let own = read(&stand(), &[&shop], "own").unwrap();
        assert!(own.contains("Auth: basic, its own\n  username = •••\n  password = {{pass}}"), "{own}");
        assert!(own.contains("Request \"Shop / Orders / Own\", id own: GET {{baseUrl}}/x\nQuery:\n  token: •••\n"), "a string url's query read too: {own}");
        let top = read(&stand(), &[&shop], "top").unwrap();
        assert!(
            top.contains("Auth: oauth2, inherited from the collection\n  accessTokenUrl = https://auth.test/token\n  clientId = {{clientId}}\n  clientSecret = •••\n  grant_type = client_credentials\n  addTokenTo = header\n  useBrowser = false"),
            "{top}"
        );
        assert!(top.contains("Variables it uses:\n  baseUrl = https://api.test/v1\n  clientId — not set"), "the inherited auth's too: {top}");
        assert!(!top.contains("Scripts"), "no scripts, no heading: {top}");
    }

    #[test]
    fn a_request_is_read_only_where_the_description_lists_it() {
        let shop = saved();
        let other = Collection::parse(&json!({
            "info": {"name": "Billing", "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},
            "item": [get("b1", json!("{{billingUrl}}/invoices"))]
        }).to_string()).unwrap();
        let reach = Reach::new(&[(&shop, vec![]), (&other, vec![])], &[], &stand());
        assert!(reach.lists("r") && !reach.lists("b1") && !reach.lists("b"), "{:?}", reach.requests);
        assert!(reach.requests.iter().all(|line| !line.starts_with("Billing")), "a collection sending nowhere here is not listed");
        assert_eq!(
            read(&stand(), &[&shop, &other], "b1"),
            Err(HttpError::SendsNowhere { request: "b1".into(), collection: "Billing".into(), environment: "Stage".into() })
        );
        assert_eq!(read(&stand(), &[&shop, &other], "f"), Err(HttpError::NoRequest("f".into())), "a folder is no request");
        assert_eq!(read(&stand(), &[&shop], "nope"), Err(HttpError::NoRequest("nope".into())));
        let off = Environment { agent: AgentAccess::Off, ..stand() };
        assert_eq!(read(&off, &[&shop], "r"), Err(HttpError::NoneOpen));
    }

    #[test]
    fn a_hidden_value_is_masked_wherever_it_is_written() {
        let shop = saved();
        let mut environment = stand();
        environment.variables.push(var("apiKey", "k-1"));
        environment.variables.push(var("orderId", "hunter-s3cr3t"));
        let text = read(&environment, &[&shop], "r").unwrap();
        assert!(!text.contains("s3cr3t") && !text.contains("k-1"), "the token's value, and a credential's by its name: {text}");
        assert!(text.contains("  apiKey — hidden") && text.contains("  orderId = hunter-•••"), "{text}");

        // A credential by its name, among globals and the collection's variables: its
        // value is masked where the request happens to write it out.
        let layers = [(&shop, vec![var("sessionKey", "orders")])];
        let text = stand().read_request(&layers, &[var("apiSecret", "application/json")], "r").unwrap();
        assert!(text.contains("POST {{baseUrl}}/•••/:id") && text.contains("  Accept: •••"), "{text}");
    }

    /// One request `x` of Shop under `auth`, written as `request` says, with `scripts` of the request's own.
    fn one(request: Value, auth: Value, scripts: Value) -> Collection {
        collection(json!([{"id": "x", "name": "X \"quoted\"", "request": request, "event": scripts}]), auth, json!(null))
    }

    #[test]
    fn a_json_body_is_masked_under_a_credential_however_it_is_written() {
        let body = r#"{"count": {{n}}, "password": "hunter2", "apiSecret": 123456, "apiKeys": ["k1-long", "{{k2}}"], "secret": {"value": "deepvalue"}, "name": "kept"}"#;
        let shop = one(json!({"method": "POST", "url": "{{baseUrl}}/x", "body": {"mode": "raw", "raw": body}}), json!(null), json!([]));
        let text = read(&stand(), &[&shop], "x").unwrap();
        for secret in ["hunter2", "123456", "k1-long", "deepvalue"] {
            assert!(!text.contains(secret), "{secret}: {text}");
        }
        assert!(text.contains("\"count\": {{n}}") && text.contains("\"{{k2}}\"") && text.contains("\"name\": \"kept\""), "a variable outside a string stays one: {text}");

        let bare = r#"{"password": {{pwd}}, "token": "lit-secret"}"#;
        let shop = one(json!({"method": "POST", "url": "{{baseUrl}}/x", "body": {"mode": "raw", "raw": bare}}), json!(null), json!([]));
        let text = read(&stand(), &[&shop], "x").unwrap();
        assert!(text.contains("\"password\": {{pwd}},\n  \"token\": \"•••\""), "a variable under a credential stays one: {text}");

        let broken = r#"{"password": "hunter2", "token": 98765, "key": {{k}}, "name": "kept", oops"#;
        let shop = one(json!({"method": "POST", "url": "{{baseUrl}}/x", "body": {"mode": "raw", "raw": broken}}), json!(null), json!([]));
        let text = read(&stand(), &[&shop], "x").unwrap();
        assert!(text.contains(r#"{"password": "•••", "token": "•••", "key": {{k}}, "name": "kept", oops"#), "not JSON: its pairs masked where they stand: {text}");
    }

    #[test]
    fn a_variable_under_a_credential_in_a_body_is_hidden() {
        let shop = one(
            json!({"method": "POST", "url": "{{baseUrl}}/x", "body": {"mode": "graphql", "graphql": {"query": "mutation { login }", "variables": "{\"password\": \"{{pwd}}\", \"n\": {{n}}}"}}}),
            json!(null),
            json!([]),
        );
        let raw = one(json!({"method": "POST", "url": "{{baseUrl}}/x", "body": {"mode": "raw", "raw": "{\"auth\": {\"pass\": {{pass}}}}"}}), json!(null), json!([]));
        assert_eq!(shop.credential_names(), ["pwd"]);
        assert_eq!(raw.credential_names(), ["pass"]);
        let layers = [(&shop, vec![var("pwd", "hunter2"), var("n", "7")])];
        let text = stand().read_request(&layers, &[], "x").unwrap();
        assert!(text.contains("  pwd — hidden") && text.contains("  n = 7") && !text.contains("hunter2"), "{text}");
        assert!(text.contains("Body, graphql:\nmutation { login }\nIts variables:\n"), "{text}");
    }

    #[test]
    fn every_kind_of_auth_value_is_masked_but_its_settings_and_addresses() {
        let auth = json!({"type": "oauth2", "oauth2": [
            {"key": "tokenRequestParams", "value": [{"key": "audience", "value": "aud-secret"}]},
            {"key": "clientSecret", "value": 424242},
            {"key": "refreshTokenUrl", "value": "https://auth.test/refresh"},
            {"key": "scope", "value": "read"},
            {"key": "challengeAlgorithm", "value": null},
        ]});
        let text = read(&stand(), &[&one(json!("{{baseUrl}}/x"), auth, json!([]))], "x").unwrap();
        assert!(
            text.contains("  tokenRequestParams = •••\n  clientSecret = •••\n  refreshTokenUrl = https://auth.test/refresh\n  scope = read\n  challengeAlgorithm = \n"),
            "{text}"
        );
        let apikey = json!({"type": "apikey", "apikey": [{"key": "key", "value": "X-API-Key"}, {"key": "value", "value": "k3y-literal"}, {"key": "in", "value": "header"}]});
        let text = read(&stand(), &[&one(json!("{{baseUrl}}/x"), apikey, json!([]))], "x").unwrap();
        assert!(text.contains("Auth: apikey, inherited from the collection\n  key = X-API-Key\n  value = •••\n  in = header"), "{text}");
    }

    #[test]
    fn a_literal_masked_once_is_masked_wherever_else_it_is_written() {
        let auth = json!({"type": "bearer", "bearer": [{"key": "token", "value": "eyJliteral"}]});
        let script = json!([{"listen": "prerequest", "script": {"exec": ["pm.request.headers.upsert({key: 'Authorization', value: 'Bearer eyJliteral'});"]}}]);
        let request = json!({"url": "https://admin:hunter2@api.test/v1/x?note=eyJliteral", "header": [{"key": "X-Key", "value": "123"}, {"key": "X-Id", "value": "123"}, {"key": "Authorization", "value": "Bearer abcd1234"}, {"key": "X-Ref", "value": "abcd1234"}]});
        let text = read(&stand(), &[&one(request, auth, script)], "x").unwrap();
        assert!(!text.contains("eyJliteral") && !text.contains("hunter2"), "{text}");
        assert!(text.contains("GET https://admin:•••@api.test/v1/x\n"), "the password of an address: {text}");
        assert!(text.contains("  note: •••") && text.contains("Pre-request script of the request: hidden — it holds a secret"), "{text}");
        assert!(text.contains("  X-Key: •••\n  X-Id: 123\n"), "a short literal masks nothing else: {text}");
        assert!(text.contains("  Authorization: Bearer •••\n  X-Ref: •••\n"), "the token without its scheme word: {text}");
        let reach = Reach::new(&[(&one(json!("https://admin:hunter2@api.test/x"), json!(null), json!([])), vec![])], &[], &stand());
        assert_eq!(reach.requests, ["Shop / X \"quoted\": GET https://admin:•••@api.test/x (id x)"], "the description too");
    }

    #[test]
    fn every_kind_of_body_and_row_is_read() {
        let rows = |mode: &str| json!({"mode": mode, mode: [{"key": "password", "value": "pw-literal"}, {"key": "doc", "type": "file", "src": "/tmp/a.pdf"}, {"key": "name", "value": "{{name}}"}]});
        for mode in ["urlencoded", "formdata"] {
            let text = read(&stand(), &[&one(json!({"method": "POST", "url": "{{baseUrl}}/x", "body": rows(mode)}), json!(null), json!([]))], "x").unwrap();
            assert!(text.contains(&format!("Body, {mode}:\n  password: •••\n  doc: a file, /tmp/a.pdf\n  name: {{{{name}}}}\n")), "{text}");
        }
        let file = read(&stand(), &[&one(json!({"method": "PUT", "url": "{{baseUrl}}/x", "body": {"mode": "file", "file": {"src": "/tmp/b.bin"}}}), json!(null), json!([]))], "x").unwrap();
        assert!(file.contains("Body: a file, /tmp/b.bin"), "{file}");
        let other = read(&stand(), &[&one(json!({"method": "PUT", "url": "{{baseUrl}}/x", "body": {"mode": "binary"}}), json!(null), json!([]))], "x").unwrap();
        assert!(other.contains("Body: binary"), "{other}");
        let empty = read(&stand(), &[&one(json!({"method": "PUT", "url": "{{baseUrl}}/x", "body": {"mode": "raw", "raw": "  "}}), json!(null), json!([]))], "x").unwrap();
        assert!(!empty.contains("Body"), "{empty}");

        let url = json!({"raw": "{{baseUrl}}/users/:session_token?api_key=k3y-literal", "variable": [{"key": "session_token", "value": "s-literal"}]});
        let text = read(&stand(), &[&one(json!({"url": url}), json!(null), json!([]))], "x").unwrap();
        assert!(text.contains("Path variables:\n  session_token: •••\nQuery:\n  api_key: •••\n"), "a url object with no `query` reads its raw one: {text}");

        let mut cycle = stand();
        cycle.variables.push(var("loop", "{{loop}}"));
        let text = read(&cycle, &[&one(json!({"url": "{{baseUrl}}/x", "header": [{"key": "X-Loop", "value": "{{loop}}"}]}), json!(null), json!([]))], "x").unwrap();
        assert!(text.contains("  loop — {{loop}} refers to itself"), "{text}");
    }

    #[test]
    fn a_literal_is_masked_and_a_variable_left() {
        for (value, scheme, shown) in [
            ("{{token}}", true, "{{token}}"),
            ("Bearer {{token}}", true, "Bearer {{token}}"),
            (" {{ token }} ", false, " {{ token }} "),
            ("Bearer eyJ", true, "Bearer •••"),
            ("Bearer eyJ", false, "•••"),
            ("tk-{{part}}", true, "•••"),
            ("Bearer", true, "•••"),
            ("Digest-1 abc", true, "•••"),
            ("", true, ""),
        ] {
            assert_eq!(literal_masked(value, scheme), shown, "{value:?}");
        }
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
        Scope::new(&[&stand().variables])
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
