//! The MongoDB page (`docs/44-mongodb.md`): what a connection is, what crosses
//! to the window about it, and the query text the user types.
//!
//! The text is mongosh's, not strict JSON: keys unquoted, strings in either
//! quote, `ObjectId('…')`, `ISODate('…')`, `/regex/i`. [`parse`] turns it into
//! MongoDB Extended JSON — a `serde_json::Value` the driver's side reads — and
//! [`to_shell`] writes a document back the same way, from its canonical
//! Extended JSON, so a document opened for editing and saved unchanged keeps
//! its types: an `Int64` reads `NumberLong(5)`, a `Double` `5.0`, an `Int32` `5`.
//!
//! No driver types here: the driver turns Extended JSON into BSON and back.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;

/// A saved connection, as it is sealed: the URI carries the password.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Connection {
    pub id: String,
    pub name: String,
    pub uri: String,
    /// Open to the agent's `mongo` tool; off until the user ticks it. A file
    /// sealed before the box existed reads as off.
    #[serde(default)]
    pub agent: bool,
}

/// A connection as the window sees it: never the URI, only where it goes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionView {
    pub id: String,
    pub name: String,
    /// The URI with its password and token-like options masked.
    pub address: String,
    pub agent: bool,
}

impl From<&Connection> for ConnectionView {
    fn from(connection: &Connection) -> Self {
        Self { id: connection.id.clone(), name: connection.name.clone(), address: redact_uri(&connection.uri), agent: connection.agent }
    }
}

/// An index as the page lists it, its keys and partial filter as mongosh
/// writes them on one line: `{ name: 1, at: -1 }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexView {
    pub name: String,
    pub keys: String,
    pub unique: bool,
    pub sparse: bool,
    pub ttl_seconds: Option<u64>,
    pub partial: Option<String>,
}

/// A collection, by the connection it is reached through.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub connection: String,
    pub database: String,
    pub collection: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseInfo {
    pub name: String,
    pub size_on_disk: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CollectionInfo {
    pub name: String,
    /// `collection`, `view` or `timeseries`.
    pub kind: &'static str,
}

/// A find, its parts already read from the page's text.
#[derive(Debug, Clone, Default)]
pub struct FindSpec {
    pub filter: Map<String, Value>,
    pub projection: Map<String, Value>,
    pub sort: Map<String, Value>,
    pub skip: u64,
    /// A page's size.
    pub limit: usize,
}

/// The servers as the agent's `mongo` tool reads them (`docs/44-mongodb.md`,
/// M-8): saved connections by id, and what is in them. Reads only; which
/// connections the agent may use is the tool's to decide, from `agent`.
/// `services::mongo::Servers` over the app's clients; a test's own in memory.
pub trait MongoServers: Send + Sync {
    fn connections(&self) -> Result<Vec<ConnectionView>, MongoError>;
    fn databases(&self, connection: &str) -> Result<Vec<DatabaseInfo>, MongoError>;
    fn collections(&self, connection: &str, database: &str) -> Result<Vec<CollectionInfo>, MongoError>;
    /// The first `spec.limit` documents, nothing left open after them.
    fn find(&self, target: &Target, spec: FindSpec) -> Result<Vec<Value>, MongoError>;
    fn count(&self, target: &Target, filter: Map<String, Value>) -> Result<u64, MongoError>;
    /// The first `cap` results of a pipeline that writes nowhere.
    fn aggregate(&self, target: &Target, stages: Vec<Map<String, Value>>, cap: usize) -> Result<Vec<Value>, MongoError>;
    fn indexes(&self, target: &Target) -> Result<Vec<IndexView>, MongoError>;
}

/// A document as the model may see it: the value of a field named like a
/// credential (`password`, `apiKey`, `refresh_token`…) is [`REDACTED`],
/// wherever it nests, and in a string a URL's password or a `password=…` too.
/// A wide heuristic, as Kubernetes' is: a `tokens_used` goes as well.
// ponytail: by the field's name, so a query that renames it (`$project: { p: '$password' }`,
// `$objectToArray`) shows the value — the model has to ask for that; redact by value if it does.
pub fn redact_document(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, field) in map.iter_mut() {
                match secret_field(key) {
                    true => *field = Value::String(super::kube_view::REDACTED.into()),
                    false => redact_document(field),
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(redact_document),
        Value::String(text) => *text = super::kube_view::redact_text(text),
        _ => {}
    }
}

fn secret_field(key: &str) -> bool {
    let key = key.chars().filter(char::is_ascii_alphanumeric).collect::<String>().to_ascii_lowercase();
    ["password", "passwd", "secret", "token", "apikey", "credential", "privatekey"].iter().any(|word| key.contains(word))
}

/// One field of a sample: its path — nested fields with dots, as a filter
/// names them, an array's documents' fields under the array's — and in how
/// many documents each BSON type was found there, most first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldTypes {
    pub path: String,
    pub types: Vec<(&'static str, usize)>,
}

/// The fields of `documents`, in the order they were first met.
pub fn schema(documents: &[Value]) -> Vec<FieldTypes> {
    let mut fields: Vec<FieldTypes> = Vec::new();
    for document in documents {
        let mut seen = Vec::new();
        if let Value::Object(map) = document {
            walk_fields("", map, &mut seen);
        }
        for (path, bson) in seen {
            let at = match fields.iter().position(|f| f.path == path) {
                Some(at) => at,
                None => {
                    fields.push(FieldTypes { path, types: Vec::new() });
                    fields.len() - 1
                }
            };
            match fields[at].types.iter_mut().find(|(name, _)| *name == bson) {
                Some((_, count)) => *count += 1,
                None => fields[at].types.push((bson, 1)),
            }
        }
    }
    for field in &mut fields {
        field.types.sort_by_key(|&(_, count)| std::cmp::Reverse(count));
    }
    fields
}

/// Each path and type in one document, once: an array's documents name the
/// same field many times over.
fn walk_fields(prefix: &str, map: &Map<String, Value>, seen: &mut Vec<(String, &'static str)>) {
    for (key, value) in map {
        let path = if prefix.is_empty() { key.clone() } else { format!("{prefix}.{key}") };
        let found = (path.clone(), bson_type(value));
        if !seen.contains(&found) {
            seen.push(found);
        }
        let nested: Vec<&Map<String, Value>> = match value {
            Value::Object(inner) => vec![inner],
            Value::Array(items) => items.iter().filter_map(Value::as_object).collect(),
            _ => Vec::new(),
        };
        for inner in nested.into_iter().filter(|inner| special(inner).is_none()) {
            walk_fields(&path, inner, seen);
        }
    }
}

/// A value's BSON type, by the name the page shows it with.
fn bson_type(value: &Value) -> &'static str {
    match value {
        Value::Object(map) => special(map).map_or("Object", |(bson, _)| bson),
        Value::Array(_) => "Array",
        Value::Null => "Null",
        Value::Bool(_) => "Boolean",
        Value::Number(_) => "Number",
        Value::String(_) => "String",
    }
}

/// What the form sends. No `uri` on an edit keeps the saved one — the window
/// never had it to send back.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionDraft {
    pub id: Option<String>,
    pub name: String,
    pub uri: Option<String>,
    #[serde(default)]
    pub agent: bool,
}

#[derive(Debug, Error)]
pub enum MongoError {
    #[error("no saved connection \"{0}\" — it was deleted")]
    NoConnection(String),
    #[error("{0}")]
    BadUri(String),
    #[error("a connection needs a name")]
    NoName,
    #[error("{what}: {error}")]
    BadInput { what: &'static str, error: ParseError },
    #[error("{what} must be {expected}")]
    WrongShape { what: &'static str, expected: &'static str },
    #[error("{field}: {error}")]
    BadValue { field: String, error: ParseError },
    #[error("the _id cannot change — insert a copy and delete this one instead")]
    IdChanged,
    #[error("no document has that _id any more — it was deleted or changed elsewhere")]
    Gone,
    #[error("the server stopped it at its time limit — a narrower filter, or an index for it, is faster")]
    TimedOut,
    #[error("these results are closed — find again")]
    FindClosed,
    #[error("{inserted} of {total} went in before one was refused, and stay in: {reason}")]
    PartlyInserted { inserted: usize, total: usize, reason: String },
    #[error("{inserted} went in, but the write concern was not met — they may not be on every member yet: {reason}")]
    Unconfirmed { inserted: usize, reason: String },
    #[error("{0}")]
    Server(String),
    #[error("{0}")]
    Store(String),
    /// The agent's tool, with no saved connection ticked "Open to the agent".
    #[error("no MongoDB connection is open to the agent — the user opens one on the MongoDB page: edit it and turn on \"Open to the agent\"")]
    NoneOpen,
    #[error("no connection named \"{name}\" is open to the agent — open to it: {open}")]
    NotOpen { name: String, open: String },
    #[error("{count} connections open to the agent are named \"{name}\" — ask the user to rename one on the MongoDB page")]
    SameName { name: String, count: usize },
    #[error("this pipeline writes with {0}, and the agent only reads — leave the stage out")]
    Writes(&'static str),
}

/// A `mongodb://` or `mongodb+srv://` URI with a host. The driver checks the
/// rest when it connects; this is the form's early "that is not a URI".
pub fn validate_uri(uri: &str) -> Result<(), MongoError> {
    let uri = uri.trim();
    let Some(rest) = uri.strip_prefix("mongodb://").or_else(|| uri.strip_prefix("mongodb+srv://")) else {
        return Err(MongoError::BadUri("a connection string starts with mongodb:// or mongodb+srv://".into()));
    };
    let (user, hosts, options) = parts(rest);
    // `admin:2024?Spring@h` is the host `admin:2024` and options `Spring@h` to the driver: an `@` past the `?` is a `?` in the user's part.
    if options.is_some_and(|options| options.contains('@')) {
        return Err(MongoError::BadUri("there is an @ after the ? — a ? in the user name or password must be percent-encoded, %3F".into()));
    }
    // `tlsCertificateKeyFilePassword=pa&ss` would mask only `pa`: an option is a key and a value.
    if options.is_some_and(|options| options.split('&').any(|option| !option.is_empty() && !option.contains('='))) {
        return Err(MongoError::BadUri("an option has no = — an & in an option's value must be percent-encoded, %26".into()));
    }
    let hosts = hosts.split('/').next().unwrap_or_default();
    if hosts.is_empty() {
        return Err(MongoError::BadUri("the connection string names no host".into()));
    }
    // `u:p?ss@h` is the host `u` on the port `p` to the driver too: what looks like a port is a number.
    let mut ports = hosts.split(',').filter_map(|host| host.rsplit_once(':')).map(|(_, port)| port).filter(|port| !port.contains(']'));
    if ports.any(|port| port.is_empty() || !port.chars().all(|c| c.is_ascii_digit())) {
        return Err(MongoError::BadUri("a host's port is not a number — a ? in the password must be percent-encoded, %3F".into()));
    }
    // As the driver reads it: past the user name's first `:`, any of these ends the password early.
    let (name, password) = user.map_or(("", ""), |user| user.split_once(':').unwrap_or((user, "")));
    if name.chars().chain(password.chars()).any(|c| "/?#[]@:".contains(c)) {
        return Err(MongoError::BadUri("the user name or password has / ? # [ ] @ or : in it — percent-encode it, %2F for /".into()));
    }
    Ok(())
}

/// After the scheme, split as the driver splits it: the options from the first
/// `?`, then the user from the last `@` before them — so a password's `/` is
/// still the password's, wherever the hosts are taken to start.
fn parts(rest: &str) -> (Option<&str>, &str, Option<&str>) {
    let (before, options) = rest.split_once('?').map_or((rest, None), |(before, options)| (before, Some(options)));
    let (user, hosts) = before.rsplit_once('@').map_or((None, before), |(user, hosts)| (Some(user), hosts));
    (user, hosts, options)
}

const MASK: &str = "•••";

/// `mongodb://user:secret@host/?authMechanismProperties=AWS_SESSION_TOKEN:x`
/// → `mongodb://user:•••@host/?authMechanismProperties=•••`: the password,
/// and any option whose name says it holds one.
pub fn redact_uri(uri: &str) -> String {
    let (scheme, rest) = match uri.split_once("://") {
        Some((scheme, rest)) => (scheme, rest),
        None => return uri.to_string(),
    };
    let (user, hosts, options) = parts(rest);
    let user = match user {
        Some(user) => match user.split_once(':') {
            Some((name, _)) => format!("{name}:{MASK}@"),
            None => format!("{user}@"),
        },
        None => String::new(),
    };
    let options = match options {
        Some(options) => {
            let options: Vec<String> = options
                .split('&')
                .map(|option| match option.split_once('=') {
                    Some((key, _)) if secret_option(key) => format!("{key}={MASK}"),
                    _ => option.to_string(),
                })
                .collect();
            format!("?{}", options.join("&"))
        }
        None => String::new(),
    };
    format!("{scheme}://{user}{hosts}{options}")
}

fn secret_option(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    ["password", "token", "secret", "authmechanismproperties"].iter().any(|word| key.contains(word))
}

/// Where a parse stopped, counted from 1 as an editor counts.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("line {line}, column {column}: {message}")]
pub struct ParseError {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

/// A filter, a projection, a sort, an index's keys: one document. Nothing
/// typed is the empty one.
pub fn parse_document(text: &str, what: &'static str) -> Result<Map<String, Value>, MongoError> {
    if text.trim().is_empty() {
        return Ok(Map::new());
    }
    match parse(text).map_err(|error| MongoError::BadInput { what, error })? {
        Value::Object(map) => Ok(map),
        _ => Err(MongoError::WrongShape { what, expected: "a document, { … }" }),
    }
}

/// Documents to insert: one, or an array of them.
pub fn parse_documents(text: &str, what: &'static str) -> Result<Vec<Map<String, Value>>, MongoError> {
    const EXPECTED: &str = "a document { … } or an array of them [ { … }, … ]";
    let documents = parse_list(text, what, EXPECTED)?;
    if documents.is_empty() {
        return Err(MongoError::WrongShape { what, expected: EXPECTED });
    }
    Ok(documents)
}

/// An aggregation pipeline: an array of stages, or one stage alone. Nothing
/// typed is the empty pipeline, which returns the collection.
pub fn parse_pipeline(text: &str) -> Result<Vec<Map<String, Value>>, MongoError> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    parse_list(text, "the pipeline", "an array of stages, [ { $match: … }, … ]")
}

/// The stage that writes the results into a collection, `$out` or `$merge`:
/// a pipeline with one replaces or changes documents on the server, not only reads them.
pub fn writing_stage(stages: &[Map<String, Value>]) -> Option<&'static str> {
    stages.iter().flat_map(Map::keys).find_map(|key| ["$out", "$merge"].into_iter().find(|stage| stage == key))
}

/// One field changed on a card: `path` is its keys from the document down, an
/// array's item by its index; `text` its new value, as mongosh writes it.
#[derive(Debug, Clone, Deserialize)]
pub struct FieldChange {
    pub path: Vec<String>,
    pub text: String,
}

/// The update that sets every field `changes` names: `{ $set: { "a.0.b": … } }`.
pub fn set_fields(changes: &[FieldChange]) -> Result<Map<String, Value>, MongoError> {
    if changes.is_empty() {
        return Err(MongoError::WrongShape { what: "the change", expected: "at least one field" });
    }
    let mut set = Map::new();
    for FieldChange { path, text } in changes {
        if path.first().is_some_and(|key| key == "_id") {
            return Err(MongoError::IdChanged);
        }
        let field = dotted(path).ok_or(MongoError::WrongShape { what: "a field", expected: "named without a dot or a leading $ to be edited alone — edit the whole document" })?;
        let value = parse(text).map_err(|error| MongoError::BadValue { field: field.clone(), error })?;
        set.insert(field, value);
    }
    Ok(Map::from_iter([("$set".to_string(), Value::Object(set))]))
}

/// `filter` with one more condition, from a field on a card: `path` equal to
/// `text`, or with `exclude` not equal. The same field with another value
/// already there stays, the two then under `$and`.
pub fn add_condition(filter: &str, path: &[String], text: &str, exclude: bool) -> Result<String, MongoError> {
    let mut filter = parse_document(filter, "the filter")?;
    let field = dotted(path).ok_or(MongoError::WrongShape { what: "a field", expected: "named without a dot or a leading $ to be searched by" })?;
    let value = parse(text).map_err(|error| MongoError::BadValue { field: field.clone(), error })?;
    let condition = if exclude { Value::Object(Map::from_iter([("$ne".to_string(), value)])) } else { value };
    match filter.get(&field) {
        None => {
            filter.insert(field, condition);
        }
        Some(was) if *was == condition => {}
        Some(_) => {
            let one = Value::Object(Map::from_iter([(field, condition)]));
            match filter.entry("$and").or_insert_with(|| Value::Array(Vec::new())) {
                Value::Array(all) if all.contains(&one) => {}
                Value::Array(all) => all.push(one),
                _ => return Err(MongoError::WrongShape { what: "the filter's $and", expected: "an array of conditions" }),
            }
        }
    }
    Ok(to_shell_line(&Value::Object(filter)))
}

/// A field's keys as one dotted path — or `None` when a key would read as
/// another path in it: empty, with a dot, or with a leading `$`.
fn dotted(path: &[String]) -> Option<String> {
    (!path.is_empty() && !path.iter().any(|key| key.is_empty() || key.contains('.') || key.starts_with('$'))).then(|| path.join("."))
}

/// One document, or an array of nothing else.
fn parse_list(text: &str, what: &'static str, expected: &'static str) -> Result<Vec<Map<String, Value>>, MongoError> {
    let shape = || MongoError::WrongShape { what, expected };
    match parse(text).map_err(|error| MongoError::BadInput { what, error })? {
        Value::Object(map) => Ok(vec![map]),
        Value::Array(items) => items.into_iter().map(|item| if let Value::Object(map) = item { Ok(map) } else { Err(shape()) }).collect(),
        _ => Err(shape()),
    }
}

/// One value in mongosh's syntax, as Extended JSON.
pub fn parse(text: &str) -> Result<Value, ParseError> {
    let mut parser = Parser { chars: text.chars().collect(), at: 0, depth: 0 };
    let value = parser.value()?;
    parser.skip_space();
    if parser.at < parser.chars.len() {
        return Err(parser.error("unexpected text after the value"));
    }
    Ok(value)
}

/// How deep values may nest: the server's own limit for a command. Past it a
/// paste would only run the parser's recursion out of stack — the whole app with it.
const MAX_DEPTH: usize = 200;

struct Parser {
    chars: Vec<char>,
    at: usize,
    /// Values open around the one being read.
    depth: usize,
}

impl Parser {
    fn error(&self, message: impl Into<String>) -> ParseError {
        let before = &self.chars[..self.at.min(self.chars.len())];
        let line = before.iter().filter(|&&c| c == '\n').count() + 1;
        let column = before.iter().rev().take_while(|&&c| c != '\n').count() + 1;
        ParseError { line, column, message: message.into() }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.at).copied()
    }

    fn skip_space(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.at += 1;
        }
    }

    fn expect(&mut self, wanted: char) -> Result<(), ParseError> {
        self.skip_space();
        if self.peek() == Some(wanted) {
            self.at += 1;
            Ok(())
        } else {
            Err(self.error(format!("expected '{wanted}'")))
        }
    }

    fn value(&mut self) -> Result<Value, ParseError> {
        if self.depth == MAX_DEPTH {
            return Err(self.error(format!("nested deeper than {MAX_DEPTH} levels")));
        }
        self.depth += 1;
        let value = self.one();
        self.depth -= 1;
        value
    }

    fn one(&mut self) -> Result<Value, ParseError> {
        self.skip_space();
        match self.peek() {
            None => Err(self.error("expected a value")),
            Some('{') => self.object(),
            Some('[') => self.array(),
            Some(quote @ ('"' | '\'')) => self.string(quote).map(Value::String),
            Some('/') => self.regex(),
            Some(c) if c == '-' || c == '.' || c.is_ascii_digit() => self.number(),
            Some(c) if c.is_alphabetic() || c == '_' || c == '$' => self.word(),
            Some(c) => Err(self.error(format!("unexpected '{c}'"))),
        }
    }

    /// Comma-separated items up to `close`, a trailing comma allowed; the
    /// opening bracket is already behind.
    fn items(&mut self, close: char, mut item: impl FnMut(&mut Self) -> Result<(), ParseError>) -> Result<(), ParseError> {
        loop {
            self.skip_space();
            if self.peek() == Some(close) {
                self.at += 1;
                return Ok(());
            }
            item(self)?;
            self.skip_space();
            match self.peek() {
                Some(',') => self.at += 1,
                Some(c) if c == close => {}
                _ => return Err(self.error(format!("expected ',' or '{close}'"))),
            }
        }
    }

    fn object(&mut self) -> Result<Value, ParseError> {
        self.at += 1;
        let mut map = Map::new();
        self.items('}', |p| {
            let key = p.key()?;
            p.expect(':')?;
            map.insert(key, p.value()?);
            Ok(())
        })?;
        Ok(Value::Object(map))
    }

    fn key(&mut self) -> Result<String, ParseError> {
        match self.peek() {
            Some(quote @ ('"' | '\'')) => self.string(quote),
            Some(c) if key_char(c) => {
                let start = self.at;
                while self.peek().is_some_and(key_char) {
                    self.at += 1;
                }
                Ok(self.chars[start..self.at].iter().collect())
            }
            _ => Err(self.error("expected a field name")),
        }
    }

    fn array(&mut self) -> Result<Value, ParseError> {
        self.at += 1;
        let mut items = Vec::new();
        self.items(']', |p| {
            items.push(p.value()?);
            Ok(())
        })?;
        Ok(Value::Array(items))
    }

    fn string(&mut self, quote: char) -> Result<String, ParseError> {
        self.at += 1;
        let mut out = String::new();
        loop {
            match self.peek() {
                None => return Err(self.error("the string is not closed")),
                Some(c) if c == quote => {
                    self.at += 1;
                    return Ok(out);
                }
                Some('\\') => {
                    self.at += 1;
                    let escaped = self.peek().ok_or_else(|| self.error("the string is not closed"))?;
                    self.at += 1;
                    match escaped {
                        'n' => out.push('\n'),
                        't' => out.push('\t'),
                        'r' => out.push('\r'),
                        'b' => out.push('\u{8}'),
                        'f' => out.push('\u{c}'),
                        'v' => out.push('\u{b}'),
                        '0' if !self.peek().is_some_and(|c| c.is_ascii_digit()) => out.push('\0'),
                        '0'..='9' => return Err(self.error("an octal escape is not read — write \\u0001 for code 1")),
                        'x' => {
                            let code = self.hex(2).and_then(char::from_u32).ok_or_else(|| self.error("expected two hex digits after \\x"))?;
                            out.push(code);
                        }
                        'u' => out.push(self.unicode()?),
                        // A line ended with a backslash goes on on the next.
                        '\n' => {}
                        '\r' => {
                            if self.peek() == Some('\n') {
                                self.at += 1;
                            }
                        }
                        // As JavaScript reads it: \' \" \\ \/ and any other are the character itself.
                        other => out.push(other),
                    }
                }
                Some(c) => {
                    out.push(c);
                    self.at += 1;
                }
            }
        }
    }

    /// `n` hex digits, taken only when all are there.
    fn hex(&mut self, n: usize) -> Option<u32> {
        let digits: String = self.chars.get(self.at..self.at + n)?.iter().collect();
        let code = digits.chars().all(|c| c.is_ascii_hexdigit()).then(|| u32::from_str_radix(&digits, 16).ok()).flatten()?;
        self.at += n;
        Some(code)
    }

    /// The code after `\u`: four hex digits, or any number of them in braces.
    fn code_point(&mut self) -> Result<u32, ParseError> {
        if self.peek() != Some('{') {
            return self.hex(4).ok_or_else(|| self.error("expected four hex digits after \\u"));
        }
        self.at += 1;
        let start = self.at;
        while self.peek().is_some_and(|c| c.is_ascii_hexdigit()) {
            self.at += 1;
        }
        let digits: String = self.chars[start..self.at].iter().collect();
        let code = u32::from_str_radix(&digits, 16).ok().filter(|_| self.peek() == Some('}'));
        self.at += 1;
        code.ok_or_else(|| self.error("expected hex digits and } after \\u{"))
    }

    /// After `\u`: a character. A surrogate pair, as JSON writes an emoji,
    /// is the one character, either half written either way.
    fn unicode(&mut self) -> Result<char, ParseError> {
        let code = self.code_point()?;
        if (0xD800..0xDC00).contains(&code) && self.chars.get(self.at..self.at + 2) == Some(&['\\', 'u']) {
            let at = self.at;
            self.at += 2;
            match self.code_point().ok() {
                Some(low @ 0xDC00..0xE000) => return Ok(char::from_u32(0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00)).unwrap_or(char::REPLACEMENT_CHARACTER)),
                _ => self.at = at,
            }
        }
        char::from_u32(code).ok_or_else(|| match code {
            0xD800..0xE000 => self.error(format!("\\u{code:04X} is half of a pair — its other half is missing")),
            _ => self.error(format!("\\u{{{code:X}}} is past the last character, 10FFFF")),
        })
    }

    fn regex(&mut self) -> Result<Value, ParseError> {
        self.at += 1;
        let mut pattern = String::new();
        loop {
            match self.peek() {
                None | Some('\n') => return Err(self.error("the /regex/ is not closed")),
                Some('/') => break,
                // An escape is two characters, so `\\` before the closing `/` does not escape it.
                Some('\\') => match self.chars.get(self.at + 1) {
                    None | Some('\n') => return Err(self.error("the /regex/ is not closed")),
                    Some('/') => {
                        pattern.push('/');
                        self.at += 2;
                    }
                    Some(&escaped) => {
                        pattern.extend(['\\', escaped]);
                        self.at += 2;
                    }
                },
                Some(c) => {
                    pattern.push(c);
                    self.at += 1;
                }
            }
        }
        self.at += 1;
        let mut options: Vec<char> = Vec::new();
        while let Some(c) = self.peek().filter(char::is_ascii_alphabetic) {
            if !"imsxlu".contains(c) {
                return Err(self.error(format!("'{c}' is not a regex option (i, m, s, x)")));
            }
            options.push(c);
            self.at += 1;
        }
        options.sort_unstable();
        Ok(serde_json::json!({ "$regularExpression": { "pattern": pattern, "options": options.into_iter().collect::<String>() } }))
    }

    fn number(&mut self) -> Result<Value, ParseError> {
        let start = self.at;
        if self.peek() == Some('-') {
            self.at += 1;
            self.skip_space();
            if self.peek().is_some_and(char::is_alphabetic) {
                return match self.word()? {
                    Value::Object(map) if map.get("$numberDouble") == Some(&Value::from("Infinity")) => Ok(serde_json::json!({ "$numberDouble": "-Infinity" })),
                    _ => Err(self.error("expected a number after '-'")),
                };
            }
        }
        let mut float = false;
        while let Some(c) = self.peek() {
            match c {
                '0'..='9' => {}
                '.' | 'e' | 'E' => float = true,
                '+' | '-' if matches!(self.chars.get(self.at - 1), Some('e' | 'E')) => {}
                _ => break,
            }
            self.at += 1;
        }
        // `- 5` is `-5`, as in mongosh.
        let text: String = self.chars[start..self.at].iter().filter(|c| !c.is_whitespace()).collect();
        let bad = || ParseError { message: format!("\"{text}\" is not a number"), ..self.error("") };
        if !float {
            if let Ok(int) = text.parse::<i64>() {
                return Ok(Value::from(int));
            }
        }
        let double: f64 = text.parse().map_err(|_| bad())?;
        serde_json::Number::from_f64(double).map(Value::Number).ok_or_else(bad)
    }

    /// `true`, `null`, `ObjectId('…')`, `new Date('…')`: a bare word, a call.
    fn word(&mut self) -> Result<Value, ParseError> {
        let start = self.at;
        let mut name = self.identifier();
        if name == "new" {
            self.skip_space();
            name = self.identifier();
        }
        match name.as_str() {
            "true" => return Ok(Value::Bool(true)),
            "false" => return Ok(Value::Bool(false)),
            "null" => return Ok(Value::Null),
            "undefined" => return Ok(serde_json::json!({ "$undefined": true })),
            "Infinity" => return Ok(serde_json::json!({ "$numberDouble": "Infinity" })),
            "NaN" => return Ok(serde_json::json!({ "$numberDouble": "NaN" })),
            _ => {}
        }
        self.skip_space();
        if self.peek() != Some('(') {
            self.at = start;
            return Err(self.error(format!("\"{name}\" is not a value — a string needs quotes")));
        }
        self.at += 1;
        let mut args = Vec::new();
        self.items(')', |p| {
            args.push(p.value()?);
            Ok(())
        })?;
        self.call(&name, args).map_err(|Mistake(message)| {
            self.at = start;
            self.error(message)
        })
    }

    fn identifier(&mut self) -> String {
        let start = self.at;
        while self.peek().is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '$') {
            self.at += 1;
        }
        self.chars[start..self.at].iter().collect()
    }

    /// What was wrong with the arguments, if anything; [`Parser::word`] adds where.
    fn call(&self, name: &str, args: Vec<Value>) -> Result<Value, Mistake> {
        let one_string = |what: &str| match args.as_slice() {
            [Value::String(s)] => Ok(s.clone()),
            _ => Err(format!("{name} takes {what} in quotes")),
        };
        // A number, or the same in quotes: `NumberLong(5)`, `NumberLong('5')`.
        let one_integer = || match args.as_slice() {
            [Value::String(s)] => s.trim().parse::<i64>().map_err(|_| format!("{name} takes a whole number")),
            [Value::Number(n)] => n.as_i64().ok_or_else(|| format!("{name} takes a whole number")),
            _ => Err(format!("{name} takes a whole number")),
        };
        match name {
            "ObjectId" => {
                let hex = one_string("24 hex digits")?;
                if hex.len() != 24 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Err("ObjectId takes 24 hex digits".into());
                }
                Ok(serde_json::json!({ "$oid": hex.to_ascii_lowercase() }))
            }
            "ISODate" | "Date" => match args.as_slice() {
                [Value::String(s)] => Ok(serde_json::json!({ "$date": { "$numberLong": iso_millis(s)?.to_string() } })),
                [Value::Number(n)] => n
                    .as_i64()
                    .map(|ms| serde_json::json!({ "$date": { "$numberLong": ms.to_string() } }))
                    .ok_or_else(|| format!("{name} takes milliseconds since 1970 as a whole number").into()),
                _ => Err(format!("{name} takes a date, '2024-05-01T10:00:00Z'").into()),
            },
            "NumberLong" | "Long" => Ok(serde_json::json!({ "$numberLong": one_integer()?.to_string() })),
            "NumberInt" | "Int32" => {
                let int = i32::try_from(one_integer()?).map_err(|_| format!("{name} takes a number that fits 32 bits"))?;
                Ok(serde_json::json!({ "$numberInt": int.to_string() }))
            }
            "Double" => match args.as_slice() {
                [Value::Number(n)] => Ok(serde_json::json!({ "$numberDouble": double_text(n.as_f64().unwrap_or_default()) })),
                _ => Err("Double takes a number".into()),
            },
            "NumberDecimal" | "Decimal128" => {
                let text = match args.as_slice() {
                    [Value::Number(n)] => n.to_string(),
                    _ => one_string("a decimal")?,
                };
                Ok(serde_json::json!({ "$numberDecimal": text }))
            }
            "UUID" => {
                let uuid = one_string("a UUID")?;
                let hex: String = uuid.chars().filter(|c| *c != '-').collect();
                if hex.len() != 32 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Err("UUID takes 32 hex digits, dashes allowed".into());
                }
                Ok(serde_json::json!({ "$uuid": dashed(&hex.to_ascii_lowercase()) }))
            }
            "BinData" => match args.as_slice() {
                [Value::Number(subtype), Value::String(base64)] => {
                    let subtype = subtype.as_u64().filter(|s| *s <= 255).ok_or("BinData's subtype is 0–255")?;
                    Ok(serde_json::json!({ "$binary": { "base64": base64, "subType": format!("{subtype:02x}") } }))
                }
                _ => Err("BinData takes a subtype and base64 in quotes: BinData(0, 'AQI=')".into()),
            },
            "Timestamp" => {
                let (t, i) = match args.as_slice() {
                    [Value::Number(t), Value::Number(i)] => (t.as_u64(), i.as_u64()),
                    [Value::Object(map)] => (map.get("t").and_then(Value::as_u64), map.get("i").and_then(Value::as_u64)),
                    _ => (None, None),
                };
                match (t, i) {
                    (Some(t), Some(i)) => Ok(serde_json::json!({ "$timestamp": { "t": t, "i": i } })),
                    _ => Err("Timestamp takes seconds and an increment: Timestamp({ t: 1700000000, i: 1 })".into()),
                }
            }
            "MinKey" if args.is_empty() => Ok(serde_json::json!({ "$minKey": 1 })),
            "MaxKey" if args.is_empty() => Ok(serde_json::json!({ "$maxKey": 1 })),
            _ => Err(format!("{name}(…) is not something this can read — ObjectId, ISODate, NumberLong, NumberDecimal, UUID…").into()),
        }
    }
}

/// What was wrong with a constructor's arguments; [Parser::word] adds where.
struct Mistake(String);

impl From<String> for Mistake {
    fn from(message: String) -> Self {
        Self(message)
    }
}

impl From<&str> for Mistake {
    fn from(message: &str) -> Self {
        Self(message.to_string())
    }
}

fn key_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '$' | '.' | '-')
}

/// `2024-05-01`, `2024-05-01T10:00:00`, `2024-05-01T10:00:00.5+03:00`: what
/// mongosh's `ISODate` takes. Without a zone it is UTC, as there.
fn iso_millis(text: &str) -> Result<i64, Mistake> {
    use chrono::{DateTime, NaiveDate, NaiveDateTime};
    let text = text.trim();
    if let Ok(at) = DateTime::parse_from_rfc3339(text) {
        return Ok(at.timestamp_millis());
    }
    for format in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%dT%H:%M", "%Y-%m-%d %H:%M:%S%.f"] {
        if let Ok(at) = NaiveDateTime::parse_from_str(text, format) {
            return Ok(at.and_utc().timestamp_millis());
        }
    }
    NaiveDate::parse_from_str(text, "%Y-%m-%d")
        .ok()
        .and_then(|day| day.and_hms_opt(0, 0, 0))
        .map(|at| at.and_utc().timestamp_millis())
        .ok_or_else(|| format!("\"{text}\" is not a date — 2024-05-01 or 2024-05-01T10:00:00Z").into())
}

fn double_text(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 && value.abs() < 1e16 {
        format!("{value:.1}")
    } else {
        value.to_string()
    }
}

/// A value in canonical Extended JSON, written as mongosh would show it and
/// [`parse`] reads it back: two-space indents, unquoted keys where they can be.
pub fn to_shell(value: &Value) -> String {
    let mut out = String::new();
    write_shell(value, Some(0), &mut out);
    out
}

/// The same on one line — `{ name: 1, at: -1 }` — for an index's keys.
pub fn to_shell_line(value: &Value) -> String {
    let mut out = String::new();
    write_shell(value, None, &mut out);
    out
}

/// `depth` is `None` on one line.
fn write_shell(value: &Value, depth: Option<usize>, out: &mut String) {
    let open = |out: &mut String, bracket: char| {
        out.push(bracket);
        out.push(if depth.is_some() { '\n' } else { ' ' });
    };
    let item_start = |out: &mut String| {
        if let Some(depth) = depth {
            out.push_str(&"  ".repeat(depth + 1));
        }
    };
    let item_end = |out: &mut String, last: bool| match (depth, last) {
        (Some(_), false) => out.push_str(",\n"),
        (Some(_), true) => out.push('\n'),
        (None, false) => out.push_str(", "),
        (None, true) => out.push(' '),
    };
    let close = |out: &mut String, bracket: char| {
        if let Some(depth) = depth {
            out.push_str(&"  ".repeat(depth));
        }
        out.push(bracket);
    };
    let inner = depth.map(|d| d + 1);
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => out.push_str(&quoted(s)),
        Value::Array(items) if items.is_empty() => out.push_str("[]"),
        Value::Array(items) => {
            open(out, '[');
            for (i, item) in items.iter().enumerate() {
                item_start(out);
                write_shell(item, inner, out);
                item_end(out, i + 1 == items.len());
            }
            close(out, ']');
        }
        Value::Object(map) => {
            if let Some((_, special)) = special(map) {
                out.push_str(&special);
                return;
            }
            if map.is_empty() {
                out.push_str("{}");
                return;
            }
            open(out, '{');
            for (i, (key, item)) in map.iter().enumerate() {
                item_start(out);
                out.push_str(&shell_key(key));
                out.push_str(": ");
                write_shell(item, inner, out);
                item_end(out, i + 1 == map.len());
            }
            close(out, '}');
        }
    }
}

/// A value as the page draws it, a field to a line: what nests folds, the
/// rest is its mongosh text with its BSON type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Shown {
    Object { fields: Vec<(String, Shown)> },
    Array { items: Vec<Shown> },
    Value { text: String, bson: &'static str },
}

/// A document's fields, each as [`Shown`]; an empty object or array is a value, `{}` or `[]`.
pub fn fields(map: &Map<String, Value>) -> Vec<(String, Shown)> {
    map.iter().map(|(key, item)| (key.clone(), shown(item))).collect()
}

fn shown(value: &Value) -> Shown {
    match value {
        Value::Array(items) if !items.is_empty() => Shown::Array { items: items.iter().map(shown).collect() },
        Value::Object(map) if !map.is_empty() => match special(map) {
            Some((bson, text)) => Shown::Value { text, bson },
            None => Shown::Object { fields: fields(map) },
        },
        _ => Shown::Value {
            text: to_shell_line(value),
            bson: match value {
                Value::Null => "Null",
                Value::Bool(_) => "Boolean",
                Value::Number(_) => "Number",
                Value::String(_) => "String",
                Value::Array(_) => "Array",
                Value::Object(_) => "Object",
            },
        },
    }
}

/// A BSON type that Extended JSON wraps in `{ "$…": … }`: its name and its constructor.
fn special(map: &Map<String, Value>) -> Option<(&'static str, String)> {
    if map.len() != 1 {
        return None;
    }
    let (tag, inner) = map.iter().next()?;
    let text = |v: &Value| v.as_str().map(str::to_string);
    Some(match tag.as_str() {
        "$oid" => ("ObjectId", format!("ObjectId({})", quoted(&text(inner)?))),
        "$numberInt" => ("Int32", text(inner)?),
        "$numberLong" => ("Int64", format!("NumberLong({})", text(inner)?)),
        "$numberDouble" => ("Double", text(inner)?),
        "$numberDecimal" => ("Decimal128", format!("NumberDecimal({})", quoted(&text(inner)?))),
        "$date" => {
            let millis: i64 = match inner {
                Value::Object(m) => m.get("$numberLong").and_then(Value::as_str)?.parse().ok()?,
                Value::String(s) => iso_millis(s).ok()?,
                Value::Number(n) => n.as_i64()?,
                _ => return None,
            };
            let text = match chrono::DateTime::from_timestamp_millis(millis) {
                Some(at) => format!("ISODate({})", quoted(&at.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())),
                None => format!("Date({millis})"),
            };
            ("Date", text)
        }
        "$binary" => {
            let base64 = inner.get("base64").and_then(Value::as_str)?;
            let subtype = inner.get("subType").and_then(Value::as_str)?;
            match (subtype, uuid_of(base64)) {
                ("04", Some(uuid)) => ("UUID", format!("UUID({})", quoted(&uuid))),
                _ => ("Binary", format!("BinData({}, {})", u8::from_str_radix(subtype, 16).ok()?, quoted(base64))),
            }
        }
        "$regularExpression" => {
            let pattern = inner.get("pattern").and_then(Value::as_str)?;
            let options = inner.get("options").and_then(Value::as_str)?;
            ("RegExp", format!("/{}/{options}", escape_slashes(pattern)))
        }
        "$timestamp" => ("Timestamp", format!("Timestamp({{ t: {}, i: {} }})", inner.get("t")?, inner.get("i")?)),
        "$minKey" => ("MinKey", "MinKey()".into()),
        "$maxKey" => ("MaxKey", "MaxKey()".into()),
        "$undefined" => ("Undefined", "undefined".into()),
        _ => return None,
    })
}

/// A pattern between `/…/`: a `/` escaped unless it already is.
fn escape_slashes(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len());
    let mut escaped = false;
    for c in pattern.chars() {
        if c == '/' && !escaped {
            out.push('\\');
        }
        out.push(c);
        escaped = c == '\\' && !escaped;
    }
    out
}

/// Sixteen bytes in base64 as a dashed UUID.
fn uuid_of(base64: &str) -> Option<String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD.decode(base64).ok()?;
    if bytes.len() != 16 {
        return None;
    }
    Some(dashed(&bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()))
}

/// 32 hex digits as `8-4-4-4-12`.
fn dashed(hex: &str) -> String {
    format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

fn quoted(text: &str) -> String {
    Value::String(text.to_string()).to_string()
}

fn shell_key(key: &str) -> String {
    let mut chars = key.chars();
    let plain = chars.next().is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$') && chars.all(|c| c.is_alphanumeric() || c == '_' || c == '$');
    if plain { key.to_string() } else { quoted(key) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_uri_is_one_of_the_two_schemes_with_a_host() {
        assert!(validate_uri("mongodb://localhost:27017").is_ok());
        assert!(validate_uri("  mongodb+srv://u:p@cluster0.example.net/db?retryWrites=true ").is_ok());
        assert!(validate_uri("mongodb://u:p@/db").is_err());
        assert!(validate_uri("mongodb://").is_err());
        assert!(validate_uri("postgres://localhost").is_err());
        assert!(validate_uri("localhost:27017").is_err());
        // A reserved character in the password, unencoded: the driver would not read it, and the mask would miss it.
        for uri in ["mongodb://u:pa/ss@h", "mongodb://u:p?ss@h", "mongodb://u:p@ss@h", "mongodb://u:p:ss@h", "mongodb://u/x:p@h", "mongodb://h:", "mongodb://admin:2024?Spring@db.example.com", "mongodb://ad?min:pw@h", "mongodb://h/?tlsCertificateKeyFilePassword=pa&ss"] {
            assert!(matches!(validate_uri(uri), Err(MongoError::BadUri(m)) if m.contains("percent-encode")), "{uri}");
        }
        assert!(validate_uri("mongodb://u:pa%2Fss@h/db?authSource=admin").is_ok());
        assert!(validate_uri("mongodb://[::1]:27017,[::2],h2:27018/?w=1").is_ok());
        assert!(validate_uri("mongodb://u@h").is_ok());
        assert!(validate_uri("mongodb://h/?").is_ok(), "no options at all");
    }

    #[test]
    fn the_password_and_secret_options_are_masked_and_nothing_else() {
        assert_eq!(redact_uri("mongodb://admin:s3cr3t@db1:27017,db2/app?authSource=admin"), "mongodb://admin:•••@db1:27017,db2/app?authSource=admin");
        // An `@` in the password, encoded or not, takes the host from the last one.
        assert_eq!(redact_uri("mongodb://u:p@ss@h"), "mongodb://u:•••@h");
        assert_eq!(redact_uri("mongodb://u@h/?authMechanism=MONGODB-X509"), "mongodb://u@h/?authMechanism=MONGODB-X509");
        assert_eq!(
            redact_uri("mongodb+srv://h/?authMechanismProperties=AWS_SESSION_TOKEN:abc&tlsCertificateKeyFilePassword=x&w=1"),
            "mongodb+srv://h/?authMechanismProperties=•••&tlsCertificateKeyFilePassword=•••&w=1"
        );
        assert_eq!(redact_uri("mongodb://localhost"), "mongodb://localhost");
        assert!(!redact_uri("mongodb://a:hunter2@h?x=1").contains("hunter2"));
        // The user is taken from the last `@` before the options, as the driver takes it: a `/` in the password stays masked.
        assert_eq!(redact_uri("mongodb://u:pa/ss@h/db?w=1"), "mongodb://u:•••@h/db?w=1");
        assert_eq!(redact_uri("mongodb://h/db?w=1&x=@y"), "mongodb://h/db?w=1&x=@y");
    }

    #[test]
    fn mongosh_syntax_reads_as_extended_json() {
        let parsed = parse(r#"{ name: 'Ann', "the age": 42, big: 9007199254740993, ratio: 0.5, tags: ['a', "b",], on: true, gone: null, }"#).unwrap();
        assert_eq!(parsed, json!({ "name": "Ann", "the age": 42, "big": 9007199254740993i64, "ratio": 0.5, "tags": ["a", "b"], "on": true, "gone": null }));
        // Operators and dotted paths are keys as typed.
        assert_eq!(parse("{ $or: [{ 'a.b': { $gte: -3 } }, { a.c: { $exists: false } }] }").unwrap(), json!({ "$or": [{ "a.b": { "$gte": -3 } }, { "a.c": { "$exists": false } }] }));
        assert_eq!(parse("1e3").unwrap(), json!(1000.0));
        assert_eq!(parse(r#""a\"b\né""#).unwrap(), json!("a\"b\né"));
        assert_eq!(parse(r"'it\'s'").unwrap(), json!("it's"));
    }

    #[test]
    fn constructors_read_as_their_wrappers() {
        assert_eq!(parse("ObjectId('65F0A1B2C3D4E5F6A7B8C9D0')").unwrap(), json!({ "$oid": "65f0a1b2c3d4e5f6a7b8c9d0" }));
        assert_eq!(parse("ISODate('2024-05-01T10:00:00Z')").unwrap(), json!({ "$date": { "$numberLong": "1714557600000" } }));
        assert_eq!(parse("new Date('2024-05-01')").unwrap(), json!({ "$date": { "$numberLong": "1714521600000" } }));
        assert_eq!(parse("ISODate('2024-05-01T13:00:00+03:00')").unwrap(), parse("ISODate('2024-05-01T10:00:00')").unwrap());
        assert_eq!(parse("Date(5)").unwrap(), json!({ "$date": { "$numberLong": "5" } }));
        assert_eq!(parse("NumberLong(5)").unwrap(), json!({ "$numberLong": "5" }));
        assert_eq!(parse("Long('-7')").unwrap(), json!({ "$numberLong": "-7" }));
        assert_eq!(parse("NumberInt('5')").unwrap(), json!({ "$numberInt": "5" }));
        assert_eq!(parse("Double(5)").unwrap(), json!({ "$numberDouble": "5.0" }));
        assert_eq!(parse("NumberDecimal('1.10')").unwrap(), json!({ "$numberDecimal": "1.10" }));
        assert_eq!(parse("UUID('0123456789ABCDEF0123456789abcdef')").unwrap(), json!({ "$uuid": "01234567-89ab-cdef-0123-456789abcdef" }));
        assert_eq!(parse("BinData(0, 'AQI=')").unwrap(), json!({ "$binary": { "base64": "AQI=", "subType": "00" } }));
        assert_eq!(parse("Timestamp(1700000000, 2)").unwrap(), json!({ "$timestamp": { "t": 1700000000u64, "i": 2 } }));
        assert_eq!(parse("Timestamp({ t: 1, i: 2 })").unwrap(), json!({ "$timestamp": { "t": 1, "i": 2 } }));
        assert_eq!(parse("/^ann\\/b/mi").unwrap(), json!({ "$regularExpression": { "pattern": "^ann/b", "options": "im" } }));
        assert_eq!(parse(r"/a\\/").unwrap(), json!({ "$regularExpression": { "pattern": r"a\\", "options": "" } }), "an escaped backslash does not escape the closing slash");
        assert_eq!(parse(r"/\d+\./").unwrap(), json!({ "$regularExpression": { "pattern": r"\d+\.", "options": "" } }));
        assert!(parse(r"/a\").is_err());
        assert_eq!(parse("[MinKey(), MaxKey(), undefined, -Infinity, NaN]").unwrap(), json!([{ "$minKey": 1 }, { "$maxKey": 1 }, { "$undefined": true }, { "$numberDouble": "-Infinity" }, { "$numberDouble": "NaN" }]));
    }

    #[test]
    fn a_mistake_says_where_and_what() {
        let error = parse("{\n  a: 1\n  b: 2 }").unwrap_err();
        assert_eq!((error.line, error.column), (3, 3), "{error}");
        assert!(error.message.contains("','"), "{error}");
        assert!(parse("{ a: Ann }").unwrap_err().message.contains("needs quotes"));
        assert!(parse("ObjectId('abc')").unwrap_err().message.contains("24 hex"));
        assert!(parse("ObjectId('zzzzzzzzzzzzzzzzzzzzzzzz')").unwrap_err().message.contains("24 hex"), "24 characters, none of them hex");
        assert!(parse("NumberInt(5000000000)").unwrap_err().message.contains("32 bits"));
        assert!(parse("ISODate('yesterday')").unwrap_err().message.contains("not a date"));
        assert!(parse("Code('x')").unwrap_err().message.contains("not something"));
        assert!(parse("/a/q").unwrap_err().message.contains("regex option"));
        assert!(parse("{ a: 1 } x").is_err());
        assert!(parse("'open").is_err());
        assert!(parse("").is_err());
    }

    #[test]
    fn the_shapes_each_place_takes() {
        assert!(parse_document("  ", "the filter").unwrap().is_empty());
        assert_eq!(parse_document("{ a: 1 }", "the filter").unwrap()["a"], json!(1));
        let wrong = parse_document("[1]", "the filter").unwrap_err().to_string();
        assert!(wrong.starts_with("the filter must be a document"), "{wrong}");
        let bad = parse_document("{ a: }", "the sort").unwrap_err().to_string();
        assert!(bad.starts_with("the sort: line 1, column 6"), "{bad}");

        assert_eq!(parse_documents("{ a: 1 }", "the document").unwrap().len(), 1);
        assert_eq!(parse_documents("[{ a: 1 }, { a: 2 }]", "the documents").unwrap().len(), 2);
        assert!(parse_documents("[]", "the documents").is_err());
        assert!(parse_documents("[{ a: 1 }, 2]", "the documents").is_err());
        assert!(parse_documents("3", "the documents").is_err());

        assert!(parse_pipeline("").unwrap().is_empty());
        assert_eq!(parse_pipeline("{ $match: {} }").unwrap().len(), 1);
        assert_eq!(parse_pipeline("[{ $match: {} }, { $limit: 5 }]").unwrap().len(), 2);
        assert!(parse_pipeline("[1]").is_err());
        assert!(parse_pipeline("'x'").is_err());
    }

    #[test]
    fn a_document_is_written_as_mongosh_shows_it() {
        let canonical = json!({
            "_id": { "$oid": "65f0a1b2c3d4e5f6a7b8c9d0" },
            "name": "Ann \"A\"",
            "n32": { "$numberInt": "5" },
            "n64": { "$numberLong": "5" },
            "dbl": { "$numberDouble": "5.0" },
            "dec": { "$numberDecimal": "1.10" },
            "at": { "$date": { "$numberLong": "1714557600000" } },
            "id": { "$binary": { "base64": "ASNFZ4mrze8BI0VniavN7w==", "subType": "04" } },
            "bin": { "$binary": { "base64": "AQI=", "subType": "00" } },
            "re": { "$regularExpression": { "pattern": "a/b", "options": "i" } },
            "ts": { "$timestamp": { "t": 1, "i": 2 } },
            "the key": [],
            "empty": {},
            "list": [{ "$numberInt": "1" }, true, null],
        });
        let text = to_shell(&canonical);
        assert_eq!(
            text,
            r#"{
  _id: ObjectId("65f0a1b2c3d4e5f6a7b8c9d0"),
  name: "Ann \"A\"",
  n32: 5,
  n64: NumberLong(5),
  dbl: 5.0,
  dec: NumberDecimal("1.10"),
  at: ISODate("2024-05-01T10:00:00.000Z"),
  id: UUID("01234567-89ab-cdef-0123-456789abcdef"),
  bin: BinData(0, "AQI="),
  re: /a\/b/i,
  ts: Timestamp({ t: 1, i: 2 }),
  "the key": [],
  empty: {},
  list: [
    1,
    true,
    null
  ]
}"#
        );
    }

    /// What makes editing safe: a document written out and read back is the
    /// same BSON — every wrapper either survives or becomes the one the driver
    /// reads as the same type.
    #[test]
    fn a_written_document_reads_back_as_the_same_types() {
        let back = parse(&to_shell(&json!({
            "n32": { "$numberInt": "5" },
            "n64": { "$numberLong": "5" },
            "dbl": { "$numberDouble": "5.0" },
            "big": { "$numberDouble": "1e+300" },
            "at": { "$date": { "$numberLong": "1714557600123" } },
            "oid": { "$oid": "65f0a1b2c3d4e5f6a7b8c9d0" },
        })))
        .unwrap();
        assert_eq!(back["n32"], json!(5));
        assert!(back["n32"].as_i64().is_some(), "an integer stays an integer");
        assert_eq!(back["n64"], json!({ "$numberLong": "5" }));
        assert!(back["dbl"].is_f64(), "5.0 stays a double: {}", back["dbl"]);
        assert_eq!(back["big"].as_f64(), Some(1e300));
        assert_eq!(back["at"], json!({ "$date": { "$numberLong": "1714557600123" } }));
        assert_eq!(back["oid"], json!({ "$oid": "65f0a1b2c3d4e5f6a7b8c9d0" }));
        // A pattern as the server keeps it, written and read back unchanged: a document holding one can be saved.
        for pattern in ["a/b", r"a\\", r"a\\/b", r"^\d+\.\w", ""] {
            let regex = json!({ "$regularExpression": { "pattern": pattern, "options": "i" } });
            assert_eq!(parse(&to_shell(&regex)).unwrap(), regex, "{pattern}");
        }
        // A slash the server keeps escaped is not escaped again: it reads back as the same regex.
        let kept = json!({ "$regularExpression": { "pattern": r"a\/b", "options": "" } });
        assert_eq!(to_shell(&kept), r"/a\/b/");
        assert!(parse(&to_shell(&kept)).is_ok());
    }

    #[test]
    fn escapes_read_as_javascript_reads_them() {
        let read = |text: &str| parse(text).map(|v| v.as_str().map(String::from)).unwrap_or_else(|e| panic!("{text}: {e}"));
        for (text, wanted) in [
            (r"'\x41\x7e'", "A~"),
            (r"'\v\0x'", "\u{b}\u{0}x"),
            (r"'\u00e9\u{1F600}\u{41}'", "é😀A"),
            // As JSON writes an emoji: a surrogate pair is one character.
            (r#""\ud83d\ude00""#, "😀"),
            (r#""\uD83D\uDE00!""#, "😀!"),
            (r"'\uD83D\u{DE00}'", "😀"),
            (r"'\u{D83D}\u{DE00}'", "😀"),
            (r#"'\'\"\\\/\q'"#, "'\"\\/q"),
            ("'a\\\nb\\\r\nc'", "abc"),
        ] {
            assert_eq!(read(text).as_deref(), Some(wanted), "{text}");
        }
        for (text, why) in [
            (r"'\x4'", "two hex digits after \\x"),
            (r"'\xZZ'", "two hex digits after \\x"),
            (r"'\x+1'", "two hex digits after \\x"),
            (r"'\u+041'", "four hex digits after \\u"),
            (r"'\u12'", "four hex digits after \\u"),
            (r"'\u{}'", "hex digits and } after \\u{"),
            (r"'\u{41'", "hex digits and } after \\u{"),
            (r"'\u{110000}'", "\\u{110000} is past the last character"),
            (r"'\ud83d'", "\\uD83D is half of a pair"),
            (r"'\ud83d\u0041'", "\\uD83D is half of a pair"),
            (r"'\u{D83D}\u{41}'", "\\uD83D is half of a pair"),
            (r"'\ude00'", "\\uDE00 is half of a pair"),
            (r"'\1'", "an octal escape"),
            (r"'\01'", "an octal escape"),
        ] {
            let refused = parse(text).unwrap_err();
            assert!(refused.message.contains(why), "{text}: {refused}");
        }
        // What is written reads back as itself.
        let odd = "\u{0}\u{b}\u{8}é😀\\'\"";
        assert_eq!(parse(&to_shell(&json!(odd))).unwrap(), json!(odd));
    }

    #[test]
    fn values_nest_as_deep_as_the_server_takes_and_no_deeper() {
        let nested = |open: &str, close: &str, n: usize| format!("{}1{}", open.repeat(n), close.repeat(n));
        assert!(parse(&nested("[", "]", MAX_DEPTH - 1)).is_ok());
        assert!(parse(&nested("{ a: ", " }", MAX_DEPTH - 1)).is_ok());
        let deeper = parse(&nested("[", "]", MAX_DEPTH)).unwrap_err();
        assert_eq!((deeper.column, deeper.message.as_str()), (MAX_DEPTH + 1, "nested deeper than 200 levels"));
        // Through a constructor's arguments too, and a paste far past it is refused, not run out of stack.
        assert!(parse(&nested("NumberInt(", ")", MAX_DEPTH + 1)).unwrap_err().message.starts_with("nested deeper"));
        assert!(parse(&"[".repeat(1_000_000)).unwrap_err().message.starts_with("nested deeper"));
        // Siblings are not depth.
        assert!(parse(&format!("[{}]", vec!["[1]"; 1000].join(","))).is_ok());
    }

    #[test]
    fn a_pipeline_that_writes_is_told_by_its_stage() {
        let stages = |text| parse_pipeline(text).unwrap();
        assert_eq!(writing_stage(&stages("[{ $match: { a: 1 } }, { $out: 'copy' }]")), Some("$out"));
        assert_eq!(writing_stage(&stages("{ $merge: { into: 'copy' } }")), Some("$merge"));
        // A field or a value named so is not a stage.
        assert_eq!(writing_stage(&stages("[{ $match: { $out: 1 } }, { $project: { '$merge': 1 } }, { $set: { s: '$out' } }]")), None);
        assert_eq!(writing_stage(&[]), None);
    }

    #[test]
    fn one_line_for_an_index() {
        assert_eq!(to_shell_line(&json!({ "name": { "$numberInt": "1" }, "at": { "$numberInt": "-1" } })), "{ name: 1, at: -1 }");
        assert_eq!(to_shell_line(&json!({ "a": [1, { "b": "x" }], "e": {} })), "{ a: [ 1, { b: \"x\" } ], e: {} }");
    }

    #[test]
    fn fields_are_set_by_their_paths_with_the_types_they_were_written_in() {
        let one = |keys: &[&str], text: &str| FieldChange { path: keys.iter().map(|key| key.to_string()).collect(), text: text.into() };
        let set = |changes: &[FieldChange]| set_fields(changes).map(Value::Object);
        assert_eq!(set(&[one(&["op", "tags", "0"], "NumberLong(5)"), one(&["name"], "'Bea'")]).unwrap(), json!({ "$set": { "op.tags.0": { "$numberLong": "5" }, "name": "Bea" } }));
        assert!(matches!(set(&[]), Err(MongoError::WrongShape { what: "the change", .. })));
        assert!(matches!(set(&[one(&["name"], "1"), one(&["_id"], "1")]), Err(MongoError::IdChanged)));
        assert!(matches!(set(&[one(&["_id", "part"], "1")]), Err(MongoError::IdChanged)));
        for keys in [&[][..], &["a.b"], &["$inc"], &["op", ""]] {
            assert!(matches!(set(&[one(keys, "1")]), Err(MongoError::WrongShape { what: "a field", .. })), "{keys:?}");
        }
        let error = set(&[one(&["name"], "'ok'"), one(&["op", "n"], "Bea")]).unwrap_err();
        assert!(error.to_string().starts_with("op.n: line 1, column 1"), "{error}");
    }

    #[test]
    fn a_condition_joins_the_filter_and_a_second_value_of_a_field_goes_under_and() {
        let keys = |keys: &[&str]| keys.iter().map(|key| key.to_string()).collect::<Vec<_>>();
        let add = |filter: &str, path: &[&str], text: &str, exclude: bool| add_condition(filter, &keys(path), text, exclude);
        assert_eq!(add("", &["name"], "\"Ann\"", false).unwrap(), "{ name: \"Ann\" }");
        let both = add("{ name: 'Ann' }", &["tags", "n"], "NumberLong(5)", true).unwrap();
        assert_eq!(both, "{ name: \"Ann\", \"tags.n\": { $ne: NumberLong(5) } }");
        assert_eq!(add(&both, &["name"], "'Ann'", false).unwrap(), both);
        let and = add(&both, &["name"], "'Bea'", true).unwrap();
        assert_eq!(and, "{ name: \"Ann\", \"tags.n\": { $ne: NumberLong(5) }, $and: [ { name: { $ne: \"Bea\" } } ] }");
        assert_eq!(add(&and, &["name"], "'Bea'", true).unwrap(), and);
        assert!(matches!(add("{ name: 1, $and: 2 }", &["name"], "3", false), Err(MongoError::WrongShape { what: "the filter's $and", .. })));
        assert!(matches!(add("{ name", &["name"], "1", false), Err(MongoError::BadInput { what: "the filter", .. })));
        assert!(matches!(add("", &["a.b"], "1", false), Err(MongoError::WrongShape { what: "a field", .. })));
        assert!(add("", &["op", "n"], "Bea", false).unwrap_err().to_string().starts_with("op.n: line 1, column 1"));
    }

    #[test]
    fn fields_fold_what_nests_and_name_the_rest() {
        let doc = json!({
            "_id": { "$oid": "65f0a1b2c3d4e5f6a7b8c9d0" },
            "n": { "$numberLong": "5" },
            "ok": true,
            "tags": ["a", null],
            "op": { "at": { "$date": { "$numberLong": "0" } } },
            "none": [],
            "empty": {},
        });
        let shown = serde_json::to_value(fields(doc.as_object().unwrap())).unwrap();
        let value = |text: &str, bson: &str| json!({ "kind": "value", "text": text, "bson": bson });
        assert_eq!(
            shown,
            json!([
                ["_id", value("ObjectId(\"65f0a1b2c3d4e5f6a7b8c9d0\")", "ObjectId")],
                ["n", value("NumberLong(5)", "Int64")],
                ["ok", value("true", "Boolean")],
                ["tags", { "kind": "array", "items": [value("\"a\"", "String"), value("null", "Null")] }],
                ["op", { "kind": "object", "fields": [["at", value("ISODate(\"1970-01-01T00:00:00.000Z\")", "Date")]] }],
                ["none", value("[]", "Array")],
                ["empty", value("{}", "Object")],
            ])
        );
    }

    #[test]
    fn a_view_carries_no_password() {
        let connection = Connection { id: "c1".into(), name: "Prod".into(), uri: "mongodb://root:pw@h".into(), agent: false };
        let view = ConnectionView::from(&connection);
        assert_eq!(view.address, "mongodb://root:•••@h");
        assert!(!serde_json::to_string(&view).unwrap().contains("pw@"));
    }
}
