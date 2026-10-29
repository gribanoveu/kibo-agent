//! A Postman Collection v2.1, kept as the JSON it came in (`docs/45-http-client.md`,
//! decision 2). Kibo reads what it understands out of the tree and changes only
//! what the user edits, so what it does not know — scripts, examples, `formdata`,
//! `oauth2`, keys newer than the schema — is written back where it was, in the
//! order it was. A struct with a flattened `extra` would write its own fields
//! first and move `event` after `request`.

use base64::Engine;
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use serde_json::{Map, Value};

use super::http_client::{strip_json_comments, Environment, Filled, HttpError, Outgoing, Prepared, Scope, Variable};

pub struct Collection {
    /// Always an object: `parse` lets nothing else in.
    root: Value,
}

/// A request's auth once inheritance is done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Auth {
    None,
    Bearer { token: String },
    Basic { username: String, password: String },
    ApiKey { key: String, value: String, in_query: bool },
    /// `oauth2`, `digest` and the rest: kept in the file, not sent yet.
    Other(String),
}

impl Collection {
    pub fn parse(text: &str) -> Result<Collection, HttpError> {
        let wrong = |what: &str| Err(HttpError::NotACollection(what.into()));
        // v3 is a folder of YAML files, each with its `$kind`.
        const V3: &str = "it is Postman v3 — export the collection from Postman as v2.1";
        let root: Value = match serde_json::from_str(text) {
            Ok(root) => root,
            Err(_) if text.contains("$kind") => return wrong(V3),
            Err(e) => return wrong(&format!("not JSON — {e}")),
        };
        let schema = root.pointer("/info/schema").and_then(Value::as_str);
        match schema {
            Some(s) if s.contains("/collection/v2.1") => {}
            Some(s) if s.contains("/collection/v2.0") => return wrong("it is v2.0 — export it from Postman as v2.1"),
            Some(s) => return wrong(&format!("its schema is {s}")),
            None if root.get("_postman_variable_scope").is_some() => return wrong("it is a Postman environment"),
            None if root.get("requests").is_some() => return wrong("it is a Postman collection v1"),
            None if root.get("$kind").is_some() => return wrong(V3),
            None => return wrong("it has no info.schema"),
        }
        if root.get("item").is_some_and(|i| !i.is_array()) {
            return wrong("its item is not a list");
        }
        Ok(Collection { root })
    }

    /// The file as Postman writes it: two spaces, no line break at the end.
    pub fn to_text(&self) -> String {
        serde_json::to_string_pretty(&self.root).expect("a JSON value always serializes")
    }

    /// Gives every folder and request without an `id` one — Postman's export
    /// has none (HC-0), and a saved response is kept by its request's id.
    /// The only change Kibo makes to a file the user did not edit.
    pub fn ensure_ids(&mut self) -> usize {
        fn walk(items: &mut [Value], seen: &mut std::collections::HashSet<String>, added: &mut usize) {
            for obj in items.iter_mut().filter_map(Value::as_object_mut) {
                // A number would never match an id asked for, `../x` names no file to keep a response in,
                // and a second `abc` or `ABC` — one name on macOS and Windows — would share the first's.
                let id = obj.get("id").and_then(Value::as_str).filter(|id| is_safe_id(id)).map(str::to_ascii_lowercase);
                if !id.is_some_and(|id| seen.insert(id)) {
                    let fresh = uuid::Uuid::new_v4().to_string();
                    seen.insert(fresh.clone());
                    let id = Value::String(fresh);
                    match obj.get_mut("id") {
                        Some(other) => *other = id,
                        None => _ = obj.shift_insert(0, "id".into(), id),
                    }
                    *added += 1;
                }
                if let Some(Value::Array(children)) = obj.get_mut("item") {
                    walk(children, seen, added);
                }
            }
        }
        let mut added = 0;
        if let Some(Value::Array(items)) = self.root.get_mut("item") {
            walk(items, &mut Default::default(), &mut added);
        }
        added
    }

    pub fn name(&self) -> String {
        text_of(self.root.pointer("/info/name"))
    }

    /// Every request's id, at any depth: whose saved responses are still wanted.
    pub fn request_ids(&self) -> Vec<String> {
        fn walk(items: &[Value], ids: &mut Vec<String>) {
            for obj in items.iter().filter_map(Value::as_object) {
                if let (Some(_), Some(id)) = (obj.get("request"), obj.get("id").and_then(Value::as_str)) {
                    ids.push(id.to_string());
                }
                if let Some(Value::Array(children)) = obj.get("item") {
                    walk(children, ids);
                }
            }
        }
        let mut ids = Vec::new();
        if let Some(Value::Array(items)) = self.root.get("item") {
            walk(items, &mut ids);
        }
        ids
    }

    /// A request's address as written, `{{baseUrl}}` and all: what history lists it by.
    pub fn written_url(&self, id: &str) -> Option<String> {
        let request = self.chain(id)?.last()?.get("request")?;
        Some(match request.get("url") {
            Some(Value::Object(url)) => text_of(url.get("raw")),
            Some(url) => text_of(Some(url)),
            None => text_of(Some(request)),
        })
    }

    /// A folder or request by id, to edit in place.
    pub fn item_mut(&mut self, id: &str) -> Option<&mut Map<String, Value>> {
        fn find<'a>(items: &'a mut [Value], id: &str) -> Option<&'a mut Map<String, Value>> {
            for obj in items.iter_mut().filter_map(Value::as_object_mut) {
                if obj.get("id").and_then(Value::as_str) == Some(id) {
                    return Some(obj);
                }
                if let Some(Value::Array(children)) = obj.get_mut("item") {
                    if let Some(found) = find(children, id) {
                        return Some(found);
                    }
                }
            }
            None
        }
        match self.root.get_mut("item") {
            Some(Value::Array(items)) => find(items, id),
            _ => None,
        }
    }

    /// The collection, its folders down to the item, and the item: whose auth it inherits.
    fn chain(&self, id: &str) -> Option<Vec<&Map<String, Value>>> {
        fn walk<'a>(items: &'a [Value], id: &str, path: &mut Vec<&'a Map<String, Value>>) -> bool {
            for obj in items.iter().filter_map(Value::as_object) {
                path.push(obj);
                if obj.get("id").and_then(Value::as_str) == Some(id) {
                    return true;
                }
                if let Some(Value::Array(children)) = obj.get("item") {
                    if walk(children, id, path) {
                        return true;
                    }
                }
                path.pop();
            }
            false
        }
        let root = self.root.as_object()?;
        let mut path = vec![root];
        walk(root.get("item")?.as_array()?, id, &mut path).then_some(path)
    }

    /// The collection's own variables; a `secret` one is masked as an environment's is.
    pub fn variables(&self) -> Vec<Variable> {
        let rows = self.root.get("variable").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
        rows.iter()
            .filter_map(|v| {
                Some(Variable {
                    key: v.get("key")?.as_str()?.to_string(),
                    value: text_of(v.get("value")),
                    secret: v.get("secret") == Some(&Value::Bool(true)) || v.get("type").and_then(Value::as_str) == Some("secret"),
                    enabled: v.get("disabled") != Some(&Value::Bool(true)),
                })
            })
            .collect()
    }

    /// A request's auth: its own, else the nearest folder's, else the collection's.
    /// `inherit` written out passes it on — Postman's runtime sends nothing for it
    /// (HC-0), but inheriting is what was chosen in its window.
    pub fn auth_for(&self, id: &str) -> Result<Auth, HttpError> {
        let chain = self.chain(id).ok_or_else(|| HttpError::NoSuchItem(id.into()))?;
        let (item, parents) = chain.split_last().expect("a chain holds the item");
        let own = item.get("request").and_then(|r| r.get("auth")).or_else(|| item.get("auth"));
        for auth in std::iter::once(own).chain(parents.iter().rev().map(|p| p.get("auth"))) {
            if let Some(found) = auth.and_then(auth_of) {
                return Ok(found);
            }
        }
        Ok(Auth::None)
    }

    /// The request `id` as it leaves, and as it is shown with secrets masked.
    pub fn prepare(&self, id: &str, scope: &Scope) -> Result<Prepared, HttpError> {
        let chain = self.chain(id).ok_or_else(|| HttpError::NoSuchItem(id.into()))?;
        let item = chain.last().ok_or_else(|| HttpError::NoSuchItem(id.into()))?;
        let request = item.get("request").ok_or_else(|| HttpError::NoSuchItem(id.into()))?;
        let auth = self.auth_for(id)?;
        if let Auth::Other(kind) = &auth {
            return Err(HttpError::UnsupportedAuth(kind.clone()));
        }
        let (method, url) = match request {
            Value::String(url) => ("GET".to_string(), url.clone()),
            r => (r.get("method").and_then(Value::as_str).unwrap_or("GET").to_uppercase(), url_of(r.get("url"))),
        };
        // Postman drops these methods' body, and its Content-Type, unless told not to (HC-0).
        let pruned = matches!(method.as_str(), "GET" | "HEAD" | "COPY" | "PURGE" | "UNLOCK")
            && item.get("protocolProfileBehavior").and_then(|p| p.get("disableBodyPruning")) != Some(&Value::Bool(true));
        let body = request.get("body").filter(|b| !pruned && b.get("disabled") != Some(&Value::Bool(true)));
        let mode = body.and_then(|b| b.get("mode")).and_then(Value::as_str);
        if let Some(other) = mode.filter(|m| !matches!(*m, "raw" | "urlencoded")) {
            return Err(HttpError::UnsupportedBody(other.into()));
        }

        // Every name found nowhere is gathered, so one error names them all.
        let mut missing = Vec::new();
        let mut fill = |text: &str| -> Result<Filled, HttpError> {
            match scope.fill(text) {
                Err(HttpError::UnknownVariables(names)) => {
                    missing.extend(names.into_iter().filter(|n| !missing.contains(n)).collect::<Vec<_>>());
                    Ok(Filled::default())
                }
                other => other,
            }
        };

        let mut out = Both::new(&method, fill(&url)?);
        for h in request.get("header").and_then(Value::as_array).into_iter().flatten() {
            let key = h.get("key").and_then(Value::as_str).unwrap_or_default();
            if !key.is_empty() && h.get("disabled") != Some(&Value::Bool(true)) {
                out.header(fill(key)?, fill(&text_of(h.get("value")))?);
            }
        }

        let has_authorization = out.has_header("authorization");
        match auth {
            Auth::Bearer { token } if !has_authorization => {
                let token = fill(&token)?;
                out.header(plain("Authorization"), Filled { sent: format!("Bearer {}", token.sent), shown: format!("Bearer {}", token.shown) });
            }
            Auth::Basic { username, password } if !has_authorization => {
                let (user, pass) = (fill(&username)?, fill(&password)?);
                let encode = |u: &str, p: &str| format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("{u}:{p}")));
                let sent = encode(&user.sent, &pass.sent);
                // Masked, it is shown as the window's curl shows it: `user:•••`, not encoded.
                let shown = if user.sent == user.shown && pass.sent == pass.shown { sent.clone() } else { format!("Basic {}:{}", user.shown, pass.shown) };
                out.header(plain("Authorization"), Filled { sent, shown });
            }
            Auth::ApiKey { key, value, in_query } => {
                let (key, value) = (fill(&key)?, fill(&value)?);
                if in_query {
                    out.query(key, value);
                } else {
                    out.header(key, value);
                }
            }
            _ => {}
        }

        match (mode, body) {
            (Some("raw"), Some(b)) => {
                let language = b.pointer("/options/raw/language").and_then(Value::as_str).unwrap_or("text");
                let raw = text_of(b.get("raw"));
                // Comments go first: a variable in a part commented out is not missing.
                let raw = if language == "json" { strip_json_comments(&raw) } else { raw };
                out.body(fill(&raw)?, content_type(language));
            }
            (Some("urlencoded"), Some(b)) => {
                let mut form = Filled::default();
                for f in b.get("urlencoded").and_then(Value::as_array).into_iter().flatten() {
                    let key = f.get("key").and_then(Value::as_str).unwrap_or_default();
                    if key.is_empty() || f.get("disabled") == Some(&Value::Bool(true)) {
                        continue;
                    }
                    let (k, v) = (fill(key)?, fill(&text_of(f.get("value")))?);
                    let sep = if form.sent.is_empty() { "" } else { "&" };
                    form.sent += &format!("{sep}{}={}", encode_form(&k.sent), encode_form(&v.sent));
                    form.shown += &format!("{sep}{}={}", encode_form(&k.shown), encode_form(&v.shown));
                }
                out.body(form, "application/x-www-form-urlencoded");
            }
            _ => {}
        }

        if !missing.is_empty() {
            return Err(HttpError::UnknownVariables(missing));
        }
        Ok(out.done())
    }
}

/// An id a file can be named by: what Postman and Kibo write, a UUID, and nothing with a path in it.
pub fn is_safe_id(id: &str) -> bool {
    // Windows' device names, `con.enc` included, are no files.
    let device = |name: &str| matches!(name, "con" | "prn" | "aux" | "nul") || (name.len() == 4 && (name.starts_with("com") || name.starts_with("lpt")) && name.as_bytes()[3].is_ascii_digit());
    (1..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') && !device(&id.to_ascii_lowercase())
}

/// What a file brought in by Import turned out to be.
pub enum Imported {
    Collection(Collection),
    Environment(Environment),
    Globals(Vec<Variable>),
}

/// A collection, an environment or globals, as Postman exports them; an
/// environment gets an id of its own, so the same file imported twice is two.
pub fn read_import(text: &str) -> Result<Imported, HttpError> {
    let Ok(root) = serde_json::from_str::<Value>(text) else {
        return Collection::parse(text).map(Imported::Collection);
    };
    let values = || -> Result<Vec<Variable>, HttpError> {
        let rows = root.get("values").map_or(Some(&[][..]), |v| v.as_array().map(Vec::as_slice));
        let rows = rows.ok_or_else(|| HttpError::NotACollection("its values are not a list".into()))?;
        Ok(rows
            .iter()
            .filter_map(|v| {
                Some(Variable {
                    key: v.get("key")?.as_str()?.to_string(),
                    value: text_of(v.get("value")),
                    secret: v.get("type").and_then(Value::as_str) == Some("secret"),
                    enabled: v.get("enabled") != Some(&Value::Bool(false)),
                })
            })
            .collect())
    };
    match root.get("_postman_variable_scope").and_then(Value::as_str) {
        Some("environment") => Ok(Imported::Environment(Environment { id: uuid::Uuid::new_v4().to_string(), name: text_of(root.get("name")), variables: values()?, trusted_cert: None })),
        Some("globals") => Ok(Imported::Globals(values()?)),
        Some(other) => Err(HttpError::NotACollection(format!("it is Postman {other} variables, which Kibo does not import"))),
        None => {
            let mut collection = Collection::parse(text)?;
            collection.ensure_ids();
            Ok(Imported::Collection(collection))
        }
    }
}

/// An environment as Postman imports it; a secret's value only when asked for, empty otherwise.
pub fn environment_to_postman(environment: &Environment, with_secrets: bool) -> String {
    variables_to_postman(serde_json::json!({"id": environment.id, "name": environment.name}), &environment.variables, "environment", with_secrets)
}

/// Globals as Postman imports them, the same way.
pub fn globals_to_postman(globals: &[Variable], with_secrets: bool) -> String {
    variables_to_postman(serde_json::json!({"name": "Globals"}), globals, "globals", with_secrets)
}

fn variables_to_postman(mut file: Value, variables: &[Variable], scope: &str, with_secrets: bool) -> String {
    let values: Vec<Value> = variables
        .iter()
        .map(|v| {
            let value = if v.secret && !with_secrets { "" } else { v.value.as_str() };
            serde_json::json!({"key": v.key, "value": value, "type": if v.secret { "secret" } else { "default" }, "enabled": v.enabled})
        })
        .collect();
    file["values"] = Value::Array(values);
    file["_postman_variable_scope"] = scope.into();
    serde_json::to_string_pretty(&file).expect("a JSON value always serializes")
}

/// A request built for sending and for showing at once.
struct Both {
    sent: Outgoing,
    shown: Outgoing,
}

impl Both {
    fn new(method: &str, url: Filled) -> Both {
        let make = |url: String| Outgoing { method: method.into(), url, headers: Vec::new(), body: None };
        Both { sent: make(url.sent), shown: make(url.shown) }
    }

    fn header(&mut self, key: Filled, value: Filled) {
        self.sent.headers.push((key.sent, value.sent));
        self.shown.headers.push((key.shown, value.shown));
    }

    fn has_header(&self, name: &str) -> bool {
        self.sent.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case(name))
    }

    fn query(&mut self, key: Filled, value: Filled) {
        for (out, k, v) in [(&mut self.sent, key.sent, value.sent), (&mut self.shown, key.shown, value.shown)] {
            let sep = if out.url.contains('?') { '&' } else { '?' };
            out.url = format!("{}{sep}{k}={v}", out.url);
        }
    }

    fn body(&mut self, body: Filled, content_type: &str) {
        if !self.has_header("content-type") {
            self.header(plain("Content-Type"), plain(content_type));
        }
        self.sent.body = Some(body.sent);
        self.shown.body = Some(body.shown);
    }

    fn done(self) -> Prepared {
        Prepared { sent: self.sent, shown: self.shown }
    }
}

fn plain(text: &str) -> Filled {
    Filled { sent: text.into(), shown: text.into() }
}

/// A value as text: Postman writes most as strings, some as numbers or `true`.
fn text_of(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    }
}

/// A request's address: `url` as text, or its `raw` with each `:name` path segment
/// replaced from `url.variable` — not in the query, as Postman does (HC-0).
fn url_of(url: Option<&Value>) -> String {
    let Some(Value::Object(u)) = url else { return text_of(url) };
    let raw = text_of(u.get("raw"));
    let vars = u.get("variable").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    let (path, query) = raw.split_at(raw.find('?').unwrap_or(raw.len()));
    let path: Vec<String> = path
        .split('/')
        .map(|segment| {
            let found = segment.strip_prefix(':').and_then(|name| vars.iter().find(|v| v.get("key").and_then(Value::as_str) == Some(name)));
            found.map_or_else(|| segment.to_string(), |v| text_of(v.get("value")))
        })
        .collect();
    path.join("/") + query
}

/// An auth object, or None where it passes the parent's on (`inherit`, `null`).
fn auth_of(v: &Value) -> Option<Auth> {
    let kind = v.get("type")?.as_str()?;
    let param = |name: &str| -> String {
        let rows = v.get(kind).and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
        text_of(rows.iter().find(|r| r.get("key").and_then(Value::as_str) == Some(name)).and_then(|r| r.get("value")))
    };
    Some(match kind {
        "inherit" => return None,
        "noauth" => Auth::None,
        "bearer" => Auth::Bearer { token: param("token") },
        "basic" => Auth::Basic { username: param("username"), password: param("password") },
        "apikey" => Auth::ApiKey { key: param("key"), value: param("value"), in_query: param("in") == "query" },
        other => Auth::Other(other.into()),
    })
}

/// What Postman sets for a raw body's language when the request sets no Content-Type.
fn content_type(language: &str) -> &'static str {
    match language {
        "json" => "application/json",
        "xml" => "application/xml",
        "html" => "text/html",
        "javascript" => "application/javascript",
        _ => "text/plain",
    }
}

/// `encodeURIComponent`, as the window encodes a form.
pub(crate) const FORM: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.').remove(b'!').remove(b'~').remove(b'*').remove(b'\'').remove(b'(').remove(b')');

fn encode_form(text: &str) -> String {
    utf8_percent_encode(text, FORM).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::http_client::{HistoryEntry, Outcome};
    use serde_json::json;

    const PROBE: &str = include_str!("../infra/testdata/postman/probe.postman_collection.json");

    fn wire(name: &str) -> String {
        let path = format!("{}/src/infra/testdata/postman/wire/{name}.http", env!("CARGO_MANIFEST_DIR"));
        String::from_utf8(std::fs::read(path).expect("wire file")).expect("utf-8")
    }

    /// A header's value as Postman sent it, and the body after the blank line.
    fn wire_header(name: &str, header: &str) -> String {
        let text = wire(name);
        let head = text.split("\r\n\r\n").next().unwrap_or_default();
        head.lines().find_map(|l| l.strip_prefix(&format!("{header}: "))).map(|v| v.trim_end().to_string()).unwrap_or_default()
    }

    fn wire_body(name: &str) -> String {
        wire(name).split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default()
    }

    fn probe() -> Collection {
        let mut c = Collection::parse(PROBE).unwrap();
        c.ensure_ids();
        c
    }

    fn id_of(c: &Collection, name: &str) -> String {
        fn walk(items: &[Value], name: &str) -> Option<String> {
            items.iter().find_map(|i| {
                if i["name"] == name {
                    return i["id"].as_str().map(String::from);
                }
                walk(i.get("item")?.as_array()?, name)
            })
        }
        walk(c.root["item"].as_array().unwrap(), name).unwrap_or_else(|| panic!("no {name}"))
    }

    fn var(key: &str, value: &str) -> Variable {
        Variable { key: key.into(), value: value.into(), secret: false, enabled: true }
    }

    fn prepare(c: &Collection, name: &str, environment: &[Variable]) -> Result<Prepared, HttpError> {
        let collection = c.variables();
        c.prepare(&id_of(c, name), &Scope::new(&[&collection, environment]))
    }

    fn header<'a>(o: &'a Outgoing, name: &str) -> Option<&'a str> {
        o.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    #[test]
    fn a_postman_export_is_written_back_byte_for_byte() {
        assert_eq!(Collection::parse(PROBE).unwrap().to_text(), PROBE);
    }

    #[test]
    fn every_folder_and_request_gets_an_id_and_nothing_else_changes() {
        let mut c = Collection::parse(PROBE).unwrap();
        assert_eq!(c.ensure_ids(), 14, "4 folders and 10 requests, none with an id");
        assert_eq!(c.ensure_ids(), 0, "an id is given once");
        let text = c.to_text();
        let without: Vec<&str> = text.lines().filter(|l| !l.trim_start().starts_with("\"id\": ")).collect();
        assert_eq!(without.join("\n"), PROBE);
        let ids: std::collections::HashSet<&str> = text.lines().filter_map(|l| l.trim_start().strip_prefix("\"id\": ")).collect();
        assert_eq!(ids.len(), 14, "each id is its own");
    }

    #[test]
    fn editing_one_request_changes_only_its_line() {
        let mut c = probe();
        let before = c.to_text();
        let id = id_of(&c, "urlencoded");
        c.item_mut(&id).unwrap()["request"]["method"] = json!("PUT");
        let after = c.to_text();
        let changed: Vec<(&str, &str)> = before.lines().zip(after.lines()).filter(|(a, b)| a != b).collect();
        assert_eq!(changed, vec![("            \"method\": \"POST\",", "            \"method\": \"PUT\",")]);
        assert_eq!(before.lines().count(), after.lines().count());
    }

    #[test]
    fn what_is_not_v2_1_is_named() {
        let named = |text: &str| match Collection::parse(text) {
            Err(HttpError::NotACollection(why)) => why,
            Ok(_) => panic!("{text} parsed"),
            Err(other) => panic!("{other:?}"),
        };
        assert_eq!(named(r#"{"info": {"schema": "https://schema.getpostman.com/json/collection/v2.0.0/collection.json"}}"#), "it is v2.0 — export it from Postman as v2.1");
        assert!(named(r#"{"name": "e", "values": [], "_postman_variable_scope": "environment"}"#).contains("environment"));
        assert!(named(r#"{"id": "x", "requests": []}"#).contains("v1"));
        assert!(named(r#"{"a": 1}"#).contains("info.schema"));
        assert!(named("not json").contains("not JSON"));
        assert!(named(r#"{"info": {"schema": "https://schema.postman.com/json/collection/v2.1.0/collection.json"}, "item": {}}"#).contains("item"));
    }

    #[test]
    fn raw_urlencoded_basic_and_apikey_leave_as_postman_sent_them() {
        let c = probe();
        let raw = prepare(&c, "raw-json", &[]).unwrap().sent;
        assert_eq!((raw.method.as_str(), raw.url.as_str()), ("POST", "http://127.0.0.1:8099/api/echo"));
        assert_eq!(raw.headers, vec![("Content-Type".to_string(), wire_header("01-raw-json", "Content-Type"))]);
        assert_eq!(raw.body.as_deref(), Some(wire_body("01-raw-json").as_str()));

        let form = prepare(&c, "urlencoded", &[]).unwrap().sent;
        assert_eq!(form.body.as_deref(), Some(wire_body("02-urlencoded").as_str()));
        assert_eq!(header(&form, "Content-Type"), Some(wire_header("02-urlencoded", "Content-Type").as_str()));

        let basic = prepare(&c, "basic-auth", &[]).unwrap().sent;
        assert_eq!(header(&basic, "Authorization"), Some(wire_header("05-basic-auth", "Authorization").as_str()));
        assert_eq!(basic.body, None);

        let key = prepare(&c, "apikey-header", &[]).unwrap().sent;
        assert_eq!(header(&key, "X-API-Key"), Some(wire_header("06-apikey-header", "X-API-Key").as_str()));
        let query = prepare(&c, "apikey-query", &[]).unwrap().sent;
        let line = wire("07-apikey-query");
        assert!(line.starts_with(&format!("GET {} ", query.url.trim_start_matches("http://127.0.0.1:8099"))), "{}", query.url);
    }

    #[test]
    fn what_is_kept_from_postman_but_not_sent_is_named() {
        let c = probe();
        assert_eq!(prepare(&c, "formdata-file", &[]), Err(HttpError::UnsupportedBody("formdata".into())));
        assert_eq!(prepare(&c, "graphql", &[]), Err(HttpError::UnsupportedBody("graphql".into())));
        assert_eq!(prepare(&c, "digest-auth", &[]), Err(HttpError::UnsupportedAuth("digest".into())));
        assert_eq!(prepare(&c, "oauth2", &[]), Err(HttpError::UnsupportedAuth("oauth2".into())));
        assert_eq!(c.prepare("nope", &Scope::default()), Err(HttpError::NoSuchItem("nope".into())));
        assert_eq!(c.prepare(&id_of(&c, "Body Types"), &Scope::default()), Err(HttpError::NoSuchItem(id_of(&c, "Body Types"))), "a folder is not sent");
    }

    #[test]
    fn inherit_passes_the_nearest_folders_auth_through_a_folder_without_one() {
        let c = probe();
        assert_eq!(prepare(&c, "inherited-get", &[]), Err(HttpError::UnknownVariables(vec!["access_token".into()])));
        let sent = prepare(&c, "inherited-get", &[var("access_token", "probe-token")]).unwrap().sent;
        assert_eq!(header(&sent, "Authorization"), Some("Bearer probe-token"));
    }

    #[test]
    fn the_nearest_auth_wins_noauth_stops_it_and_inherit_at_the_top_is_none() {
        let bearer = |t: &str| json!({"type": "bearer", "bearer": [{"key": "token", "value": t, "type": "string"}]});
        let request = |id: &str, auth: Value| json!({"id": id, "name": id, "request": {"method": "GET", "auth": auth, "url": "https://x"}});
        let c = Collection::parse(&json!({
            "info": {"schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},
            "auth": bearer("top"),
            "item": [
                {"id": "f", "name": "f", "auth": bearer("folder"), "item": [request("near", json!({"type": "inherit"})), request("off", json!({"type": "noauth"})), request("null", Value::Null)]},
                request("top", json!({"type": "inherit"})),
                {"id": "g", "name": "g", "auth": {"type": "inherit"}, "item": [{"id": "h", "name": "h", "item": [request("deep", json!({"type": "inherit"}))]}]},
            ],
        }).to_string()).unwrap();
        assert_eq!(c.auth_for("near"), Ok(Auth::Bearer { token: "folder".into() }));
        assert_eq!(c.auth_for("null"), Ok(Auth::Bearer { token: "folder".into() }), "auth: null inherits too");
        assert_eq!(c.auth_for("off"), Ok(Auth::None));
        assert_eq!(c.auth_for("top"), Ok(Auth::Bearer { token: "top".into() }));
        assert_eq!(c.auth_for("deep"), Ok(Auth::Bearer { token: "top".into() }), "through two folders that inherit");

        let mut open = Collection::parse(&json!({
            "info": {"schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},
            "auth": {"type": "inherit"},
            "item": [request("r", json!({"type": "inherit"}))],
        }).to_string()).unwrap();
        assert_eq!(open.auth_for("r"), Ok(Auth::None), "inherit at the collection is no auth");
        open.item_mut("r").unwrap()["request"]["header"] = json!([{"key": "Authorization", "value": "Mine"}]);
        open.root["auth"] = bearer("t");
        let sent = open.prepare("r", &Scope::default()).unwrap().sent;
        assert_eq!(sent.headers, vec![("Authorization".to_string(), "Mine".to_string())], "a header written by hand wins over auth");
    }

    #[test]
    fn the_environment_beats_the_collection_which_beats_globals() {
        let c = probe();
        let collection = c.variables();
        let id = id_of(&c, "basic-auth");
        let globals = [var("baseUrl", "https://global")];
        let env = [var("baseUrl", "https://staging")];
        assert_eq!(c.prepare(&id, &Scope::new(&[&globals, &collection, &env])).unwrap().sent.url, "https://staging/api/basic");
        assert_eq!(c.prepare(&id, &Scope::new(&[&globals, &collection])).unwrap().sent.url, "http://127.0.0.1:8099/api/basic");
    }

    #[test]
    fn a_secret_leaves_and_is_shown_masked_and_comments_never_leave() {
        let c = Collection::parse(&json!({
            "info": {"schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},
            "variable": [{"key": "pass", "value": "hunter2", "secret": true}, {"key": "off", "value": "x", "disabled": true}],
            "item": [
                {"id": "json", "name": "json", "request": {"method": "POST", "url": {"raw": "https://x/{{id}}"},
                    "auth": {"type": "bearer", "bearer": [{"key": "token", "value": "{{token}}"}]},
                    "header": [{"key": "X-Off", "value": "{{nowhere}}", "disabled": true}],
                    "body": {"mode": "raw", "raw": "{\n  // {{commented}}\n  \"a\": \"{{token}}\" /* {{also}} */\n}", "options": {"raw": {"language": "json"}}}}},
                {"id": "basic", "name": "basic", "request": {"method": "GET", "url": "https://x",
                    "auth": {"type": "basic", "basic": [{"key": "username", "value": "me"}, {"key": "password", "value": "{{pass}}"}]}}},
                {"id": "form", "name": "form", "request": {"method": "POST", "url": "https://x",
                    "body": {"mode": "urlencoded", "urlencoded": [{"key": "q", "value": "a b&{{token}}"}, {"key": "skip", "value": "1", "disabled": true}]}}},
            ],
        }).to_string()).unwrap();
        let collection = c.variables();
        let env = [var("id", "7"), Variable { secret: true, ..var("token", "t0k") }];
        let scope = Scope::new(&[&collection, &env]);

        let json = c.prepare("json", &scope).unwrap();
        assert_eq!(header(&json.sent, "Authorization"), Some("Bearer t0k"));
        assert_eq!(header(&json.shown, "Authorization"), Some("Bearer •••"));
        assert_eq!(json.sent.body.as_deref(), Some("{\n  \"a\": \"t0k\" \n}"));
        assert_eq!(json.shown.body.as_deref(), Some("{\n  \"a\": \"•••\" \n}"));
        assert_eq!(header(&json.sent, "Content-Type"), Some("application/json"));
        assert_eq!(json.sent.url, "https://x/7");

        let basic = c.prepare("basic", &scope).unwrap();
        assert_eq!(header(&basic.sent, "Authorization"), Some("Basic bWU6aHVudGVyMg=="));
        assert_eq!(header(&basic.shown, "Authorization"), Some("Basic me:•••"));

        let form = c.prepare("form", &scope).unwrap();
        assert_eq!(form.sent.body.as_deref(), Some("q=a%20b%26t0k"));
        assert_eq!(form.shown.body.as_deref(), Some("q=a%20b%26%E2%80%A2%E2%80%A2%E2%80%A2"));
        assert!(!format!("{:?}", [&json.shown, &basic.shown, &form.shown]).contains("t0k"));
        assert!(!format!("{:?}", [&json.shown, &basic.shown, &form.shown]).contains("hunter2"));
    }

    /// The requests behind wire 14 and 15, as sent with the Postman CLI.
    fn pruning() -> Collection {
        let json_body = json!({"mode": "raw", "raw": "{\"a\":1}", "options": {"raw": {"language": "json"}}});
        Collection::parse(&json!({
            "info": {"schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},
            "variable": [{"key": "v", "value": "nine"}],
            "item": [
                {"id": "get", "name": "get", "request": {"method": "get", "url": "http://127.0.0.1:8099/prune/get", "body": json_body}},
                {"id": "head", "name": "head", "request": {"method": "HEAD", "url": "https://x", "body": {"mode": "formdata", "formdata": []}}},
                {"id": "kept", "name": "kept", "protocolProfileBehavior": {"disableBodyPruning": true},
                    "request": {"method": "GET", "url": "https://x", "body": json_body}},
                {"id": "delete", "name": "delete", "request": {"method": "DELETE", "url": "https://x", "body": json_body}},
                {"id": "off", "name": "off", "request": {"method": "POST", "url": "https://x", "body": {"disabled": true, "mode": "raw", "raw": "{{nowhere}}"}}},
                {"id": "path", "name": "path", "request": {"method": "GET",
                    "url": {"raw": "http://127.0.0.1:8099/users/:id/orders/:id2?x=:id", "variable": [{"key": "id", "value": "7"}, {"key": "id2", "value": "{{v}}"}]}}},
            ],
        }).to_string()).unwrap()
    }

    #[test]
    fn a_get_body_is_dropped_with_its_content_type_unless_pruning_is_off() {
        let c = pruning();
        let scope = Scope::new(&[&c.variables()]);
        let get = c.prepare("get", &scope).unwrap().sent;
        assert_eq!(wire_body("14-get-body-pruned"), "");
        assert_eq!((get.body, get.headers), (None, vec![]), "as wire 14: no body, no Content-Type");
        assert_eq!(c.prepare("head", &scope).unwrap().sent.body, None, "a body dropped is not an unsupported one");
        let kept = c.prepare("kept", &scope).unwrap().sent;
        assert_eq!((kept.body.as_deref(), header(&kept, "Content-Type")), (Some("{\"a\":1}"), Some("application/json")));
        assert_eq!(c.prepare("delete", &scope).unwrap().sent.body.as_deref(), Some("{\"a\":1}"), "DELETE keeps it");
        assert_eq!(c.prepare("off", &scope).unwrap().sent.body, None, "a body turned off is not sent, nor its variables asked for");
    }

    #[test]
    fn a_request_is_listed_by_its_address_as_written() {
        let c = Collection::parse(&json!({
            "info": {"schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},
            "item": [
                {"id": "obj", "request": {"url": {"raw": "{{baseUrl}}/a/:id", "variable": [{"key": "id", "value": "7"}]}}},
                {"id": "str", "request": {"url": "{{baseUrl}}/b"}},
                {"id": "short", "request": "{{baseUrl}}/c"},
                {"id": "folder", "item": []}
            ]
        }).to_string()).unwrap();
        let written = |id| c.written_url(id);
        assert_eq!((written("obj"), written("str"), written("short")), (Some("{{baseUrl}}/a/:id".into()), Some("{{baseUrl}}/b".into()), Some("{{baseUrl}}/c".into())));
        assert_eq!((written("folder"), written("gone")), (None, None));
    }

    #[test]
    fn a_path_variable_fills_its_segment_and_not_the_query() {
        let c = pruning();
        let sent = c.prepare("path", &Scope::new(&[&c.variables()])).unwrap().sent;
        let line = wire("15-path-variables");
        assert!(line.starts_with(&format!("GET {} ", sent.url.trim_start_matches("http://127.0.0.1:8099"))), "{}", sent.url);
    }

    #[test]
    fn a_number_where_an_id_goes_is_replaced() {
        let mut c = Collection::parse(&json!({
            "info": {"schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},
            "item": [{"name": "a", "id": 5, "request": "https://x"}, {"id": "b", "name": "b", "request": "https://x"},
                {"id": "../../x", "name": "c", "item": [{"id": "", "name": "d", "request": "https://x"}]}],
        }).to_string()).unwrap();
        assert_eq!(c.ensure_ids(), 3, "a number, a path and an empty id");
        let id = c.root["item"][0]["id"].as_str().unwrap().to_string();
        assert_eq!(c.root["item"][0].as_object().unwrap().keys().collect::<Vec<_>>(), ["name", "id", "request"], "replaced where it was");
        assert!(c.prepare(&id, &Scope::default()).is_ok());
        assert_eq!(c.root["item"][1]["id"], "b");
        assert!(c.request_ids().iter().all(|i| is_safe_id(i)));
        assert_eq!(c.request_ids().len(), 3, "requests at any depth, not the folder");
        assert!(!is_safe_id(&"a".repeat(65)) && is_safe_id(&"a".repeat(64)) && is_safe_id("A_b-9"));
    }

    #[test]
    fn an_id_that_would_share_a_file_is_replaced() {
        let mut c = Collection::parse(&json!({
            "info": {"schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},
            "item": [{"id": "abc", "name": "a", "request": "https://x"}, {"id": "ABC", "name": "b", "request": "https://x"},
                {"id": "f", "name": "f", "item": [{"id": "abc", "name": "c", "request": "https://x"}, {"id": "Con", "name": "d", "request": "https://x"}]}],
        }).to_string()).unwrap();
        assert_eq!(c.ensure_ids(), 3, "ABC, the nested abc and Con");
        assert_eq!(c.root["item"][0]["id"], "abc", "the first keeps its id");
        let ids: std::collections::HashSet<String> = c.request_ids().iter().map(|i| i.to_ascii_lowercase()).collect();
        assert_eq!(ids.len(), 4);
        assert!(!is_safe_id("nul") && !is_safe_id("LPT1") && !is_safe_id("com9") && is_safe_id("com") && is_safe_id("comx") && is_safe_id("console"));
    }

    #[test]
    fn a_secret_in_the_body_is_masked_in_history() {
        let c = Collection::parse(&json!({
            "info": {"schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},
            "item": [{"id": "r", "name": "r", "request": {"method": "POST", "url": "https://x", "body": {"mode": "raw", "raw": "{\"password\": \"{{pass}}\"}"}}}],
        }).to_string()).unwrap();
        let env = [Variable { key: "pass".into(), value: "hunter2".into(), secret: true, enabled: true }];
        let prepared = c.prepare("r", &Scope::new(&[&env])).unwrap();
        let ok = Outcome::Response { status: 200, headers: vec![], url: "https://x".into(), redirects: vec![], elapsed_ms: 1 };
        let entry = HistoryEntry::new(1, None, None, "https://x".into(), &prepared, ok, b"{}");
        assert_eq!(entry.request.body.as_deref(), Some("{\"password\": \"•••\"}"));
        assert!(!serde_json::to_string(&entry).unwrap().contains("hunter2"));
    }

    #[test]
    fn globals_leave_for_postman_and_come_back() {
        let globals = vec![Variable { key: "k".into(), value: "s".into(), secret: true, enabled: true }];
        let out: Value = serde_json::from_str(&globals_to_postman(&globals, false)).unwrap();
        assert_eq!((out["_postman_variable_scope"].as_str(), out["values"][0]["value"].as_str()), (Some("globals"), Some("")));
        let Ok(Imported::Globals(back)) = read_import(&globals_to_postman(&globals, true)) else { panic!("globals") };
        assert_eq!(back, globals);
    }

    #[test]
    fn an_import_is_a_collection_an_environment_or_globals() {
        let Ok(Imported::Collection(c)) = read_import(PROBE) else { panic!("a collection") };
        assert_eq!(c.name(), "wire_kibo_test");
        assert_eq!(c.request_ids().len(), 10, "ids given on import");

        let env = r#"{"id": "pm-1", "name": "Stage", "values": [
            {"key": "baseUrl", "value": "https://stage", "type": "default", "enabled": true},
            {"key": "token", "value": "t0k", "type": "secret", "enabled": true},
            {"key": "off", "value": 1, "enabled": false}, {"value": "no key"}],
            "_postman_variable_scope": "environment", "_postman_exported_using": "Postman/12.0"}"#;
        let Ok(Imported::Environment(e)) = read_import(env) else { panic!("an environment") };
        assert_eq!(e.name, "Stage");
        assert_ne!(e.id, "pm-1", "its own id: imported twice is two");
        assert_eq!(e.variables, vec![
            Variable { key: "baseUrl".into(), value: "https://stage".into(), secret: false, enabled: true },
            Variable { key: "token".into(), value: "t0k".into(), secret: true, enabled: true },
            Variable { key: "off".into(), value: "1".into(), secret: false, enabled: false },
        ]);

        let globals = r#"{"name": "Globals", "values": [{"key": "g", "value": "v"}], "_postman_variable_scope": "globals"}"#;
        let Ok(Imported::Globals(g)) = read_import(globals) else { panic!("globals") };
        assert_eq!((g[0].key.as_str(), g[0].enabled), ("g", true), "no enabled is on, as in Postman");
    }

    #[test]
    fn what_is_not_imported_is_named() {
        let named = |text: &str| match read_import(text) {
            Err(HttpError::NotACollection(why)) => why,
            _ => panic!("{text} imported"),
        };
        assert_eq!(named("$kind: http-request\nmethod: GET\nurl: https://x\n"), "it is Postman v3 — export the collection from Postman as v2.1");
        assert_eq!(named(r#"{"$kind": "collection", "name": "x"}"#), "it is Postman v3 — export the collection from Postman as v2.1");
        assert!(named(r#"{"info": {"schema": "https://schema.getpostman.com/json/collection/v2.0.0/collection.json"}}"#).contains("v2.0"));
        assert!(named(r#"{"openapi": "3.0.0"}"#).contains("no info.schema"));
        assert!(named("GET https://x HTTP/1.1").contains("not JSON"));
        assert!(named(r#"{"values": [], "_postman_variable_scope": "collection"}"#).contains("collection variables"));
        assert!(named(r#"{"values": {}, "_postman_variable_scope": "environment"}"#).contains("not a list"));
    }

    #[test]
    fn an_environment_leaves_for_postman_with_its_secrets_empty_unless_asked() {
        let env = Environment { id: "e".into(), name: "Stage".into(), variables: vec![
            Variable { key: "token".into(), value: "t0k".into(), secret: true, enabled: true },
            Variable { key: "baseUrl".into(), value: "https://stage".into(), secret: false, enabled: false },
        ], trusted_cert: None };
        let out: Value = serde_json::from_str(&environment_to_postman(&env, false)).unwrap();
        assert_eq!(out["_postman_variable_scope"], "environment");
        assert_eq!(out["values"][0], json!({"key": "token", "value": "", "type": "secret", "enabled": true}));
        assert_eq!(out["values"][1], json!({"key": "baseUrl", "value": "https://stage", "type": "default", "enabled": false}));
        let with = environment_to_postman(&env, true);
        let Ok(Imported::Environment(back)) = read_import(&with) else { panic!("it imports back") };
        assert_eq!(back.variables, env.variables, "with its secrets it comes back as it left");
    }

    #[test]
    fn auth_meets_what_the_request_already_has() {
        let basic = json!({"type": "basic", "basic": [{"key": "username", "value": "me"}, {"key": "password", "value": "p"}]});
        let apikey = json!({"type": "apikey", "apikey": [{"key": "key", "value": "k"}, {"key": "value", "value": "v"}, {"key": "in", "value": "query"}]});
        let c = Collection::parse(&json!({
            "info": {"schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},
            "item": [
                {"id": "basic", "name": "basic", "request": {"method": "GET", "url": "https://x", "auth": basic, "header": [{"key": "authorization", "value": "Mine"}]}},
                {"id": "query", "name": "query", "request": {"method": "GET", "url": "https://x/?a=1", "auth": apikey}},
                {"id": "xml", "name": "xml", "request": {"method": "POST", "url": "https://x", "body": {"mode": "raw", "raw": "<a/>", "options": {"raw": {"language": "xml"}}}}},
            ],
        }).to_string()).unwrap();
        assert_eq!(c.prepare("basic", &Scope::default()).unwrap().sent.headers, vec![("authorization".to_string(), "Mine".to_string())]);
        assert_eq!(c.prepare("query", &Scope::default()).unwrap().sent.url, "https://x/?a=1&k=v");
        assert_eq!(header(&c.prepare("xml", &Scope::default()).unwrap().sent, "Content-Type"), Some("application/xml"));
    }

    #[test]
    fn every_unknown_variable_of_a_request_is_named_in_one_error() {
        let c = Collection::parse(&json!({
            "info": {"schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},
            "item": [{"id": "r", "name": "r", "request": {"method": "POST", "url": "{{host}}/x",
                "header": [{"key": "X-A", "value": "{{a}}"}], "body": {"mode": "raw", "raw": "{{host}} {{b}}"}}}],
        }).to_string()).unwrap();
        assert_eq!(c.prepare("r", &Scope::default()), Err(HttpError::UnknownVariables(vec!["host".into(), "a".into(), "b".into()])));
    }

    #[test]
    fn collection_variables_read_as_postman_writes_them() {
        let vars = probe().variables();
        assert_eq!(vars[0], var("baseUrl", "http://127.0.0.1:8099"));
        assert_eq!(vars[1], Variable { secret: true, ..var("auth_password_16qu", "") }, "a secret exported without its value");

        let c = Collection::parse(&json!({
            "info": {"schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},
            "variable": [{"key": "off", "value": "x", "disabled": true}, {"key": "n", "value": 3}, {"key": "s", "value": "v", "type": "secret"}],
            "item": [{"id": "r", "name": "r", "request": {"method": "GET", "url": "https://x/{{n}}/{{off}}"}}],
        }).to_string()).unwrap();
        assert_eq!(c.variables()[1], var("n", "3"));
        assert!(c.variables()[2].secret);
        assert_eq!(c.prepare("r", &Scope::new(&[&c.variables()])), Err(HttpError::UnknownVariables(vec!["off".into()])), "turned off is not there");
    }
}
