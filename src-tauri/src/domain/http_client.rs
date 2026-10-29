//! The HTTP client's rules (`docs/45-http-client.md`): variables and their
//! `{{name}}`, an environment and what the window sees of it, a request as it
//! leaves, a JSON body's comments, a pasted cURL, and an agent's path held under
//! its environment's base address. No I/O — `postman.rs` reads collections.

use std::collections::HashMap;
use std::sync::LazyLock;

use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
use regex::Regex;
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
    #[error("a {0} body is kept from Postman and not sent yet")]
    UnsupportedBody(String),
    #[error("the host is fixed by the environment — pass a path")]
    AbsolutePath,
    #[error("{0} leaves the environment's base address")]
    OutsideBase(String),
    #[error("the environment's baseUrl is not an address: {0}")]
    BadBaseUrl(String),
    #[error("not a cURL command Kibo reads: {0}")]
    BadCurl(String),
    #[error("no environment {0}")]
    NoEnvironment(String),
    #[error("no collection {0}")]
    NoCollection(String),
    #[error("{0} is not an id Kibo keeps a file under")]
    BadId(String),
    #[error("{0}")]
    Store(String),
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
}

impl Environment {
    /// What the window gets: a secret's value never — the mask if it has one, "" if not.
    pub fn for_window(&self) -> Environment {
        let variables = self
            .variables
            .iter()
            .map(|v| Variable {
                value: if v.secret && !v.value.is_empty() { SECRET_MASK.into() } else { v.value.clone() },
                ..v.clone()
            })
            .collect();
        Environment { variables, ..self.clone() }
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

    /// The agent's base address: the `baseUrl` variable, as a collection writes `{{baseUrl}}`.
    pub fn base_url(&self) -> Option<&str> {
        self.variables.iter().find(|v| v.enabled && v.key == "baseUrl").map(|v| v.value.as_str())
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

    /// `text` with each secret's value masked — as it is, and as an address or a
    /// form encodes it: what comes back about a request (where it went, why it
    /// failed) is written from the address as sent.
    pub fn hide(&self, text: &str) -> String {
        let mut text = text.to_string();
        for secret in self.vars.values().filter(|v| v.secret && !v.value.is_empty()) {
            let forms = [secret.value.clone(), utf8_percent_encode(&secret.value, ADDRESS).to_string(), utf8_percent_encode(&secret.value, super::postman::FORM).to_string()];
            for form in forms {
                text = text.replace(&form, SECRET_MASK);
            }
        }
        text
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
}

/// The request to send, and the same request to show — in Preview and history — with secrets masked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepared {
    pub sent: Outgoing,
    pub shown: Outgoing,
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
}

impl From<SavedResponse> for ResponseView {
    fn from(saved: SavedResponse) -> ResponseView {
        let size = saved.body.len();
        ResponseView { at: saved.at, environment: saved.environment, outcome: saved.outcome, body: String::from_utf8(saved.body).ok(), size }
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
        HistoryEntry { id: uuid::Uuid::new_v4().to_string(), at, environment, agent_chat, template_url, request, outcome, response_body }
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

/// The agent's `path` under its environment's base address: relative, and not
/// out of the base's prefix by `..` (spelled as dots, `%2e` or `\`). An encoded
/// slash is refused too: the URL keeps `..%2f` one segment, a server may decode
/// it into one more `../`.
pub fn under_base(base: &str, path: &str) -> Result<String, HttpError> {
    static ABSOLUTE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^\s*([A-Za-z][A-Za-z0-9+.-]*:)?[/\\]{2}").expect("valid regex"));
    if ABSOLUTE.is_match(path) {
        return Err(HttpError::AbsolutePath);
    }
    let lower = path.to_ascii_lowercase();
    if lower.contains("%2f") || lower.contains("%5c") {
        return Err(HttpError::OutsideBase(path.into()));
    }
    // The message leaves the address out: it may carry a password.
    let mut root = Url::parse(base).map_err(|e| HttpError::BadBaseUrl(e.to_string()))?;
    if root.cannot_be_a_base() {
        return Err(HttpError::BadBaseUrl("it cannot hold a path".into()));
    }
    // The base is a folder: `/v1` holds `/v1/orders`, not `/v1x`.
    if !root.path().ends_with('/') {
        let folder = format!("{}/", root.path());
        root.set_path(&folder);
    }
    // `./` first: `documents:batchGet` is a path, not a scheme. The join drops the base's query.
    let relative = format!("./{}", path.trim_start_matches(['/', '\\']));
    let joined = root.join(&relative).map_err(|_| HttpError::OutsideBase(path.into()))?;
    if joined.origin() != root.origin() || !joined.path().starts_with(root.path()) {
        return Err(HttpError::OutsideBase(path.into()));
    }
    Ok(joined.into())
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
        let stored = Environment { id: "e".into(), name: "test".into(), variables: vec![secret("token", "s3cr3t"), secret("unset", ""), var("baseUrl", "https://api")], trusted_cert: None };
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
        assert_eq!(stored.base_url(), Some("https://api"));
        let mut off = stored.clone();
        off.variables[2].enabled = false;
        assert_eq!(off.base_url(), None, "a baseUrl turned off is not the base");
    }

    fn sent(body: &str) -> HistoryEntry {
        let out = |auth: &str| Outgoing { method: "POST".into(), url: "https://api/x".into(), headers: vec![("Authorization".into(), auth.into())], body: Some(body.into()) };
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
    fn a_body_that_is_not_text_reaches_the_window_as_its_size() {
        let saved = |body: Vec<u8>| SavedResponse { at: 1, environment: None, outcome: Outcome::Failed { error: String::new() }, body };
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
    fn a_path_stays_under_the_base() {
        let base = "https://api.example/v1";
        assert_eq!(under_base(base, "/orders?id=1").unwrap(), "https://api.example/v1/orders?id=1");
        assert_eq!(under_base(base, "orders/../items").unwrap(), "https://api.example/v1/items");
        assert_eq!(under_base("https://api.example", "").unwrap(), "https://api.example/");
        assert_eq!(under_base("https://api.example/v1?key=1#top", "").unwrap(), "https://api.example/v1/", "the base's own query stays behind");
        assert_eq!(under_base(base, "/documents:batchGet").unwrap(), "https://api.example/v1/documents:batchGet");
        assert_eq!(under_base(base, "items:run").unwrap(), "https://api.example/v1/items:run");
        for absolute in ["https://evil.example/x", "//evil.example/x", "\\\\evil.example\\x", "\t//evil.example/x", " https://evil.example"] {
            assert_eq!(under_base(base, absolute), Err(HttpError::AbsolutePath), "{absolute:?}");
        }
        for out in ["../v2/users", "/orders/../../admin", "%2e%2e/admin", "..\\admin", "./%2E%2E/admin", "..%2fadmin", "%2e%2e%2Fadmin", "..%5cadmin"] {
            assert_eq!(under_base(base, out), Err(HttpError::OutsideBase(out.into())), "{out}");
        }
        assert!(matches!(under_base("not a url", "/x"), Err(HttpError::BadBaseUrl(_))));
        assert_eq!(under_base("mailto:me@x", "/x"), Err(HttpError::BadBaseUrl("it cannot hold a path".into())));
        let hidden = under_base("https://u:pa55@", "/x").unwrap_err().to_string();
        assert!(!hidden.contains("pa55"), "{hidden}");
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
}
