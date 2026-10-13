//! The catalog (`docs/32-catalog.md`): skills, commands, runbooks, MCP servers
//! and hooks someone published as files, listed by an `index.json` beside them
//! and read over plain HTTP.
//!
//! Everything in an index is a stranger's data. Its paths become files under
//! the app directory once installing exists (CT-2), so they are checked here,
//! before anything is shown: a path that could leave the folder it is meant
//! for never gets as far as a row.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use url::Url;

use super::mcp::{self, McpConfig, McpServerConfig};
use super::skills::validate_skill_name;

/// The catalog every install starts with — the app's own, on GitHub, read
/// through its raw files. The user's `catalog.sources` come after it.
pub const DEFAULT_SOURCE: &str = "https://raw.githubusercontent.com/gribanoveu/kibo-agent-catalog/main/";

/// The one file a source is asked for first.
pub const INDEX_FILE: &str = "index.json";

/// Past this an index is not a list of a few dozen entries.
pub const MAX_INDEX_BYTES: u64 = 1024 * 1024;

/// Past this a file is not a prompt or a config; the entry is refused rather than cut.
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// Past this a skill is not a skill but a repository.
pub const MAX_SKILL_FILES: usize = 100;

/// The file every skill has — the one the skills loader reads first.
pub const SKILL_MD: &str = "SKILL.md";

/// The index format this app reads. A newer one is refused rather than guessed at.
const INDEX_VERSION: u64 = 1;

/// The roles a runbook may be for — the folders `infra::runbooks_store` reads.
const RUNBOOK_ROLES: &[&str] = &["kubernetes"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ItemKind {
    Skill,
    Command,
    Runbook,
    Mcp,
    Hooks,
}

impl ItemKind {
    fn parse(kind: &str) -> Option<Self> {
        match kind {
            "skill" => Some(Self::Skill),
            "command" => Some(Self::Command),
            "runbook" => Some(Self::Runbook),
            "mcp" => Some(Self::Mcp),
            "hooks" => Some(Self::Hooks),
            _ => None,
        }
    }

    /// What the single file of a kind that has one must end with.
    fn extension(self) -> Option<&'static str> {
        match self {
            Self::Skill => None,
            Self::Command | Self::Runbook => Some(".md"),
            Self::Mcp | Self::Hooks => Some(".json"),
        }
    }
}

/// One entry of an index, checked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogItem {
    /// What it is installed under: a skill's folder, a command's or a
    /// runbook's file name, a key in `mcpServers`.
    pub id: String,
    pub kind: ItemKind,
    pub name: String,
    pub description: String,
    /// Compared for equality only: a different one is an update.
    pub rev: String,
    /// Relative to the source: a skill's folder, every other kind's one file.
    pub path: String,
    /// A skill's files, relative to `path`; empty for every other kind.
    pub files: Vec<String>,
    /// A runbook's role.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

/// An index as this app can use it: the entries that passed, and a line for
/// each one that did not — one broken entry is no reason to hide the rest.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogIndex {
    pub items: Vec<CatalogItem>,
    pub problems: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CatalogError {
    #[error("{0} is not a catalog address: {1}")]
    BadSource(String, String),
    #[error("the index is not JSON this app reads: {0}")]
    BadIndex(String),
    #[error("the index is format {0}; this app reads format {INDEX_VERSION} — a newer app reads it")]
    NewerIndex(u64),
    #[error("the catalog could not be reached: {0}")]
    Unavailable(String),
    #[error("the catalog answered {0}")]
    Status(u16),
    #[error("{0} is not in this catalog's list any more — refresh it")]
    NotListed(String),
    #[error("installing {0} from the catalog comes in a later version")]
    NotYet(String),
    #[error("{0} is already here — installed, or a file of that name is in its place")]
    Present(String),
    #[error("{0} was not installed from this catalog")]
    NotInstalled(String),
    #[error("the list of what the catalog installed: {0}")]
    Record(String),
    #[error("{0}")]
    Disk(String),
    #[error("the entry is not an MCP server this app reads: {0}")]
    BadEntry(String),
    #[error("{0} is not filled in")]
    MissingSecret(String),
    #[error("{0} has a line break or another control character in it — paste the value alone")]
    BadSecret(String),
}

/// A source's address as the base its paths are joined to: `https`, or
/// `http` on this machine only — what is fetched gets installed, and a
/// stranger on the network could rewrite it in flight. Ends with `/`, so a
/// base typed without one does not lose its last segment on joining.
pub fn source_base(source: &str) -> Result<Url, CatalogError> {
    let bad = |why: &str| CatalogError::BadSource(source.to_string(), why.to_string());
    let mut url = Url::parse(source.trim()).map_err(|e| bad(&e.to_string()))?;
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    match url.scheme() {
        "https" => {}
        "http" if local => {}
        "http" => return Err(bad("plain http is only for this machine (localhost)")),
        _ => return Err(bad("only https addresses are read")),
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(bad("an address with ? or # is not a folder"));
    }
    if !url.path().ends_with('/') {
        url.set_path(&format!("{}/", url.path()));
    }
    Ok(url)
}

/// The address of `path` within the source at `base` — `path` already
/// checked by [`relative_path`], so it stays under `base`.
pub fn file_url(base: &Url, path: &str) -> Result<Url, CatalogError> {
    base.join(path).map_err(|e| CatalogError::BadSource(base.to_string(), e.to_string()))
}

/// A path from an index, if it is one this app will write under a folder of
/// its own: relative, `/`-separated, and every segment plain — letters,
/// digits, `.`, `_`, `-`, and not `.` or `..`. Nothing else is needed to name
/// a catalog's files, and everything else (`..`, `\`, `C:`, `?`, `%2e`) is a
/// way out of the folder or of the URL.
fn relative_path(path: &str) -> Result<(), String> {
    for segment in path.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(format!("\"{path}\" leaves its folder or has an empty segment"));
        }
        if !segment.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')) {
            return Err(format!("\"{path}\" has a character other than letters, digits, . _ -"));
        }
    }
    Ok(())
}

#[derive(Deserialize)]
struct RawIndex {
    version: u64,
    #[serde(default)]
    items: Vec<Value>,
}

#[derive(Deserialize)]
struct RawItem {
    id: String,
    kind: String,
    name: String,
    #[serde(default)]
    description: String,
    rev: String,
    path: String,
    #[serde(default)]
    files: Vec<String>,
    #[serde(default)]
    role: Option<String>,
}

/// Reads an index. Only a file that is not an index at all fails; an entry
/// that is wrong becomes a line in `problems`, and the rest are listed.
pub fn parse_index(text: &str) -> Result<CatalogIndex, CatalogError> {
    let raw: RawIndex = serde_json::from_str(text).map_err(|e| CatalogError::BadIndex(e.to_string()))?;
    if raw.version != INDEX_VERSION {
        return Err(CatalogError::NewerIndex(raw.version));
    }
    let mut index = CatalogIndex::default();
    for (at, value) in raw.items.into_iter().enumerate() {
        // Named by its id when it has one, by its place otherwise.
        let label = value.get("id").and_then(Value::as_str).map_or_else(|| format!("entry {}", at + 1), str::to_string);
        let checked = serde_json::from_value::<RawItem>(value).map_err(|e| e.to_string()).and_then(check);
        match checked {
            Ok(item) if index.items.iter().any(|seen| seen.kind == item.kind && seen.id == item.id) => {
                index.problems.push(format!("{label}: listed twice for its kind; the first is kept"));
            }
            Ok(item) => index.items.push(item),
            Err(why) => index.problems.push(format!("{label}: {why}")),
        }
    }
    Ok(index)
}

fn check(raw: RawItem) -> Result<CatalogItem, String> {
    let kind = ItemKind::parse(&raw.kind).ok_or_else(|| format!("\"{}\" is not a kind this app installs", raw.kind))?;
    validate_skill_name(&raw.id).map_err(|e| e.to_string())?;
    if raw.name.trim().is_empty() {
        return Err("no name".into());
    }
    if raw.rev.trim().is_empty() {
        return Err("no rev".into());
    }
    relative_path(&raw.path)?;
    match kind.extension() {
        None if !raw.files.iter().any(|file| file == SKILL_MD) => return Err(format!("a skill lists its files, {SKILL_MD} among them")),
        None if raw.files.len() > MAX_SKILL_FILES => return Err(format!("more than {MAX_SKILL_FILES} files")),
        None => raw.files.iter().try_for_each(|file| relative_path(file))?,
        Some(_) if !raw.files.is_empty() => return Err("only a skill lists files".into()),
        Some(extension) if !raw.path.ends_with(extension) => {
            return Err(format!("\"{}\" is not a {extension} file", raw.path));
        }
        Some(_) => {}
    }
    let role = match (kind, raw.role) {
        (ItemKind::Runbook, Some(role)) if RUNBOOK_ROLES.contains(&role.as_str()) => Some(role),
        (ItemKind::Runbook, Some(role)) => return Err(format!("no role \"{role}\" here to read a runbook")),
        (ItemKind::Runbook, None) => return Err("a runbook names its role".into()),
        (_, _) => None,
    };
    Ok(CatalogItem {
        id: raw.id,
        kind,
        name: raw.name.trim().to_string(),
        description: raw.description.trim().to_string(),
        rev: raw.rev,
        path: raw.path,
        files: raw.files,
        role,
    })
}

/// What the catalog installed, one line per entry, kept in `installed.json`:
/// enough to tell an update from a local edit, and to remove it again even
/// once the catalog no longer lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Installed {
    /// The source's base address, as [`source_base`] spells it.
    pub source: String,
    pub kind: ItemKind,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    pub rev: String,
    /// A skill's files as written, relative to its folder; empty for a one-file kind.
    #[serde(default)]
    pub files: Vec<String>,
    /// [`content_hash`] of what was written.
    pub hash: String,
}

/// Where an entry goes, relative to the app directory — the folders the
/// skills, commands and runbooks are read from. `None` for a kind installed
/// into a config file instead (MCP, hooks: CT-3, CT-4), and for an id or role
/// that is not one of ours: a record on disk can be edited, and what it names
/// is deleted on Remove.
pub fn target(kind: ItemKind, id: &str, role: Option<&str>) -> Option<PathBuf> {
    validate_skill_name(id).ok()?;
    match kind {
        ItemKind::Skill => Some(PathBuf::from("skills").join(id)),
        ItemKind::Command => Some(PathBuf::from("commands").join(format!("{id}.md"))),
        ItemKind::Runbook => {
            let role = role.filter(|role| RUNBOOK_ROLES.contains(role))?;
            Some(PathBuf::from("runbooks").join(role).join(format!("{id}.md")))
        }
        ItemKind::Mcp | ItemKind::Hooks => None,
    }
}

/// One hash for what an entry wrote: each file's name and bytes, in the order
/// given, each length-prefixed so no two different sets hash alike.
pub fn content_hash(files: &[(&str, &[u8])]) -> String {
    let mut hasher = blake3::Hasher::new();
    for (name, bytes) in files {
        hasher.update(&(name.len() as u64).to_le_bytes());
        hasher.update(name.as_bytes());
        hasher.update(&(bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    }
    hasher.finalize().to_hex().to_string()
}

/// What a row says about an entry.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ItemState {
    /// Nothing in its place: Install.
    #[default]
    Available,
    /// As this catalog wrote it, at the rev it lists.
    Installed,
    /// As written, and the catalog has a newer rev.
    Update,
    /// Installed from here and edited since — an update would lose the edit.
    Changed,
    /// Something of that name is in its place that this catalog did not put
    /// there: the user's own, or another catalog's.
    Taken,
}

/// The state of `item` from `source`, given the record of its install, if
/// any, and the hash of what is in its place now (`None`: nothing is).
pub fn state(item: &CatalogItem, source: &str, record: Option<&Installed>, on_disk: Option<&str>) -> ItemState {
    match (record, on_disk) {
        (_, None) => ItemState::Available,
        (Some(record), Some(hash)) if record.source == source => {
            if record.hash != hash {
                ItemState::Changed
            } else if record.rev != item.rev {
                ItemState::Update
            } else {
                ItemState::Installed
            }
        }
        _ => ItemState::Taken,
    }
}

// ------------------------------------------------------------ MCP entries

/// A catalog's `mcp/<id>.json`: one `mcpServers` value, without its name —
/// the name is the entry's `id`. A command or a url, one of them.
pub fn parse_mcp(bytes: &[u8]) -> Result<McpServerConfig, CatalogError> {
    let server: McpServerConfig = serde_json::from_slice(bytes).map_err(|e| CatalogError::BadEntry(e.to_string()))?;
    if server.command.trim().is_empty() == server.url.is_none() {
        return Err(CatalogError::BadEntry("it has to have a command or a url, one of them".into()));
    }
    Ok(server)
}

/// The `${NAME}` placeholders in an entry's `env` and `headers` values, each
/// once, in the order they come. The catalog's convention, not the app's:
/// nothing reads them when a server starts, so each is filled in at install.
pub fn mcp_secrets(server: &McpServerConfig) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for value in server.env.values().chain(server.headers.values()) {
        let _ = substitute(value, &mut |name| {
            if !names.iter().any(|n| n == name) {
                names.push(name.to_string());
            }
            Ok(String::new())
        });
    }
    names
}

/// The entry with every placeholder replaced by its value, trimmed — a token
/// pasted with its newline is the token. Refused while one is empty, or has
/// a control character in it: a line break in a header is another header.
pub fn fill_secrets(server: &McpServerConfig, secrets: &BTreeMap<String, String>) -> Result<McpServerConfig, CatalogError> {
    let mut value_of = |name: &str| {
        let value = secrets.get(name).map(|v| v.trim()).filter(|v| !v.is_empty()).ok_or_else(|| CatalogError::MissingSecret(name.to_string()))?;
        if value.chars().any(char::is_control) {
            return Err(CatalogError::BadSecret(name.to_string()));
        }
        Ok(value.to_string())
    };
    let mut fill = |values: &BTreeMap<String, String>| -> Result<BTreeMap<String, String>, CatalogError> {
        values.iter().map(|(key, value)| Ok((key.clone(), substitute(value, &mut value_of)?))).collect()
    };
    Ok(McpServerConfig { env: fill(&server.env)?, headers: fill(&server.headers)?, ..server.clone() })
}

/// `text` with each `${NAME}` replaced, in one pass: a value that itself
/// holds `${…}` stays as typed. `${` not followed by a name and `}` is text.
fn substitute(text: &str, value: &mut dyn FnMut(&str) -> Result<String, CatalogError>) -> Result<String, CatalogError> {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find('}').map(|end| &after[..end]).filter(|name| is_secret_name(name)) {
            Some(name) => {
                out.push_str(&value(name)?);
                rest = &after[name.len() + 1..];
            }
            None => {
                out.push_str("${");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    Ok(out)
}

fn is_secret_name(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// What an installed server's record holds: the entry as written, less the
/// MCP tab's own switches — turning a server or a tool off there is not a
/// change to what was installed.
pub fn mcp_hash(server: &McpServerConfig) -> String {
    let as_installed = McpServerConfig { disabled: false, tool_exposure: BTreeMap::new(), ..server.clone() };
    content_hash(&[("", &serde_json::to_vec(&as_installed).unwrap_or_default())])
}

/// [`state`] for a server: what is under its name in `mcp.json`. A server
/// named apart only by case or `_` takes the place too — both would become
/// one prefix in tool names (`mcp::server_key`).
pub fn mcp_state(item: &CatalogItem, source: &str, record: Option<&Installed>, config: &McpConfig) -> ItemState {
    let on_disk = config.mcp_servers.get(&item.id).map(mcp_hash);
    if on_disk.is_none() && config.mcp_servers.keys().any(|name| mcp::server_key(name) == item.id) {
        return ItemState::Taken;
    }
    state(item, source, record, on_disk.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index(items: &str) -> CatalogIndex {
        parse_index(&format!(r#"{{"version": 1, "items": [{items}]}}"#)).unwrap()
    }

    const SKILL: &str = r#"{"id": "review", "kind": "skill", "name": "Code review", "description": " Reviews. ",
        "rev": "1", "path": "skills/review", "files": ["SKILL.md", "refs/checklist.md"]}"#;

    #[test]
    fn reads_each_kind() {
        let read = index(&[
            SKILL,
            r#"{"id": "changelog", "kind": "command", "name": "/changelog", "rev": "2", "path": "commands/changelog.md"}"#,
            r#"{"id": "tls-cert", "kind": "runbook", "role": "kubernetes", "name": "TLS", "rev": "1", "path": "runbooks/kubernetes/tls-cert.md"}"#,
            r#"{"id": "github", "kind": "mcp", "name": "GitHub", "rev": "1", "path": "mcp/github.json"}"#,
            r#"{"id": "block-force-push", "kind": "hooks", "name": "Block", "rev": "1", "path": "hooks/block-force-push.json"}"#,
        ]
        .join(","));
        assert_eq!(read.problems, Vec::<String>::new());
        let kinds: Vec<_> = read.items.iter().map(|i| i.kind).collect();
        assert_eq!(kinds, [ItemKind::Skill, ItemKind::Command, ItemKind::Runbook, ItemKind::Mcp, ItemKind::Hooks]);
        let skill = &read.items[0];
        assert_eq!((skill.name.as_str(), skill.description.as_str()), ("Code review", "Reviews."));
        assert_eq!(skill.files, ["SKILL.md", "refs/checklist.md"]);
        assert_eq!(read.items[2].role.as_deref(), Some("kubernetes"));
        assert_eq!(read.items[1].role, None);
    }

    /// The catalog this app ships pointing at must stay readable by it.
    #[test]
    fn the_published_catalogs_shape_reads() {
        let published = r#"{"version": 1, "items": [
            {"id": "review", "kind": "skill", "name": "Code review", "description": "…", "rev": "1",
             "path": "skills/review", "files": ["SKILL.md", "checklist.md"]}]}"#;
        assert_eq!(parse_index(published).unwrap().items.len(), 1);
        assert!(source_base(DEFAULT_SOURCE).is_ok());
    }

    #[test]
    fn a_path_that_leaves_its_folder_is_refused() {
        for path in ["../x.md", "commands/../../x.md", "/etc/x.md", "commands//x.md", "commands/./x.md", "commands\\x.md",
            "C:/x.md", "commands/x.md?y", "commands/%2e%2e/x.md", "commands/x y.md", ""]
        {
            let entry = format!(r#"{{"id": "x", "kind": "command", "name": "X", "rev": "1", "path": "{}"}}"#, path.replace('\\', "\\\\"));
            let read = index(&entry);
            assert!(read.items.is_empty(), "{path} was let through");
            assert_eq!(read.problems.len(), 1, "{path}");
        }
        let file = index(r#"{"id": "x", "kind": "skill", "name": "X", "rev": "1", "path": "skills/x", "files": ["SKILL.md", "../../.zshrc"]}"#);
        assert!(file.items.is_empty());
        assert!(file.problems[0].starts_with("x: \"../../.zshrc\""), "{:?}", file.problems);
    }

    #[test]
    fn a_wrong_entry_is_a_problem_and_the_rest_are_listed() {
        let read = index(&[
            r#"{"id": "Bad Name", "kind": "command", "name": "X", "rev": "1", "path": "commands/x.md"}"#,
            r#"{"id": "x", "kind": "agent", "name": "X", "rev": "1", "path": "agents/x.md"}"#,
            r#"{"kind": "command", "name": "X", "rev": "1", "path": "commands/x.md"}"#,
            r#"{"id": "x", "kind": "command", "name": " ", "rev": "1", "path": "commands/x.md"}"#,
            r#"{"id": "x", "kind": "command", "name": "X", "rev": "", "path": "commands/x.md"}"#,
            r#"{"id": "x", "kind": "command", "name": "X", "rev": "1", "path": "commands/x.json"}"#,
            r#"{"id": "x", "kind": "mcp", "name": "X", "rev": "1", "path": "mcp/x.md"}"#,
            r#"{"id": "x", "kind": "command", "name": "X", "rev": "1", "path": "commands/x.md", "files": ["x.md"]}"#,
            r#"{"id": "x", "kind": "skill", "name": "X", "rev": "1", "path": "skills/x"}"#,
            r#"{"id": "x", "kind": "runbook", "name": "X", "rev": "1", "path": "runbooks/x.md"}"#,
            r#"{"id": "x", "kind": "runbook", "role": "docker", "name": "X", "rev": "1", "path": "runbooks/x.md"}"#,
            SKILL,
        ]
        .join(","));
        assert_eq!(read.items.len(), 1, "{:?}", read.problems);
        assert_eq!(read.problems.len(), 11, "{:?}", read.problems);
        assert!(read.problems[1].contains("\"agent\" is not a kind"), "{}", read.problems[1]);
        assert!(read.problems[2].starts_with("entry 3:"), "{}", read.problems[2]);
    }

    #[test]
    fn an_id_is_listed_once_per_kind() {
        let command = r#"{"id": "review", "kind": "command", "name": "/review", "rev": "1", "path": "commands/review.md"}"#;
        let read = index(&[SKILL, command, SKILL].join(","));
        assert_eq!(read.items.len(), 2);
        assert_eq!(read.problems, ["review: listed twice for its kind; the first is kept"]);
    }

    #[test]
    fn only_an_index_of_this_format_is_read() {
        assert_eq!(parse_index(r#"{"version": 2, "items": []}"#), Err(CatalogError::NewerIndex(2)));
        assert!(matches!(parse_index("<html>"), Err(CatalogError::BadIndex(_))));
        assert!(matches!(parse_index(r#"{"items": []}"#), Err(CatalogError::BadIndex(_))));
        assert_eq!(parse_index(r#"{"version": 1}"#).unwrap(), CatalogIndex::default());
    }

    #[test]
    fn a_source_is_https_or_this_machine() {
        assert_eq!(source_base("https://example.com/cat").unwrap().as_str(), "https://example.com/cat/");
        assert_eq!(source_base(" https://example.com/cat/ ").unwrap().as_str(), "https://example.com/cat/");
        assert!(source_base("http://localhost:8000/").is_ok());
        assert!(source_base("http://127.0.0.1/").is_ok());
        assert!(source_base("http://[::1]:9/").is_ok());
        for refused in ["http://example.com/", "file:///etc/", "ftp://example.com/", "https://example.com/?a=1", "https://example.com/#a", "not a url"] {
            assert!(matches!(source_base(refused), Err(CatalogError::BadSource(..))), "{refused}");
        }
    }

    #[test]
    fn a_skill_has_its_skill_md_and_not_too_many_files() {
        let skill = |files: &str| format!(r#"{{"id": "x", "kind": "skill", "name": "X", "rev": "1", "path": "skills/x", "files": [{files}]}}"#);
        assert!(index(&skill(r#""notes.md""#)).problems[0].contains("SKILL.md"));
        let most: Vec<String> = std::iter::once("\"SKILL.md\"".to_string()).chain((1..MAX_SKILL_FILES).map(|n| format!("\"f{n}.md\""))).collect();
        assert_eq!(index(&skill(&most.join(","))).items.len(), 1);
        let over = [most.clone(), vec!["\"one-more.md\"".into()]].concat();
        assert!(index(&skill(&over.join(","))).problems[0].contains("more than 100 files"));
    }

    #[test]
    fn each_kind_goes_to_the_folder_it_is_read_from() {
        assert_eq!(target(ItemKind::Skill, "review", None), Some(PathBuf::from("skills/review")));
        assert_eq!(target(ItemKind::Command, "changelog", None), Some(PathBuf::from("commands/changelog.md")));
        assert_eq!(target(ItemKind::Runbook, "tls", Some("kubernetes")), Some(PathBuf::from("runbooks/kubernetes/tls.md")));
        assert_eq!(target(ItemKind::Mcp, "github", None), None);
        assert_eq!(target(ItemKind::Hooks, "block", None), None);
        // What an edited installed.json could name instead.
        assert_eq!(target(ItemKind::Runbook, "tls", None), None);
        assert_eq!(target(ItemKind::Runbook, "tls", Some("../..")), None);
        assert_eq!(target(ItemKind::Skill, "../../Documents", None), None);
        assert_eq!(target(ItemKind::Command, "", None), None);
    }

    #[test]
    fn a_hash_tells_names_and_bytes_apart() {
        let one = content_hash(&[("a", b"bc")]);
        assert_eq!(one, content_hash(&[("a", b"bc")]));
        assert_ne!(one, content_hash(&[("ab", b"c")]));
        assert_ne!(one, content_hash(&[("a", b"bd")]));
        assert_ne!(one, content_hash(&[("a", b"bc"), ("d", b"")]));
        // Without each name's length, two empty files read as one file whose
        // name runs on through the first one's zero length.
        assert_ne!(content_hash(&[("x", b""), ("y", b"")]), content_hash(&[("x\0\0\0\0\0\0\0\0y", b"")]));
    }

    #[test]
    fn a_rows_state_comes_from_its_record_and_what_is_in_its_place() {
        let item = index(SKILL).items.remove(0);
        let record = Installed {
            source: "https://a.example/".into(),
            kind: ItemKind::Skill,
            id: "review".into(),
            role: None,
            rev: "1".into(),
            files: item.files.clone(),
            hash: "h".into(),
        };
        let at = |record: Option<&Installed>, on_disk: Option<&str>| state(&item, "https://a.example/", record, on_disk);
        assert_eq!(at(None, None), ItemState::Available);
        // Deleted by hand since: the record is stale, and installing again is fine.
        assert_eq!(at(Some(&record), None), ItemState::Available);
        assert_eq!(at(None, Some("h")), ItemState::Taken);
        assert_eq!(at(Some(&record), Some("h")), ItemState::Installed);
        assert_eq!(at(Some(&record), Some("edited")), ItemState::Changed);
        let older = Installed { rev: "0".into(), ..record.clone() };
        assert_eq!(at(Some(&older), Some("h")), ItemState::Update);
        assert_eq!(at(Some(&older), Some("edited")), ItemState::Changed);
        let elsewhere = Installed { source: "https://b.example/".into(), ..record };
        assert_eq!(at(Some(&elsewhere), Some("h")), ItemState::Taken);
    }

    #[test]
    fn a_file_is_joined_under_its_source() {
        let base = source_base("https://raw.githubusercontent.com/o/r/main").unwrap();
        assert_eq!(file_url(&base, "skills/review/SKILL.md").unwrap().as_str(), "https://raw.githubusercontent.com/o/r/main/skills/review/SKILL.md");
        assert_eq!(file_url(&base, INDEX_FILE).unwrap().as_str(), "https://raw.githubusercontent.com/o/r/main/index.json");
    }

    const GITHUB: &[u8] = br#"{"url": "https://api.example/mcp/", "headers": {"Authorization": "Bearer ${GITHUB_TOKEN}", "X-Org": "${ORG}-${ORG}"}, "env": {"A": "${A_1} and ${ 1} and $ORG and ${"}}"#;

    #[test]
    fn an_mcp_entry_has_a_command_or_a_url() {
        assert!(parse_mcp(GITHUB).is_ok());
        assert!(parse_mcp(br#"{"command": "npx", "args": ["-y", "x"]}"#).is_ok());
        for bad in [&br#"{}"#[..], br#"{"command": " "}"#, br#"{"command": "x", "url": "https://a/"}"#, b"[]", b"{"] {
            assert!(matches!(parse_mcp(bad), Err(CatalogError::BadEntry(_))), "{}", String::from_utf8_lossy(bad));
        }
    }

    #[test]
    fn placeholders_are_named_once_and_only_well_formed_ones() {
        assert_eq!(mcp_secrets(&parse_mcp(GITHUB).unwrap()), ["A_1", "GITHUB_TOKEN", "ORG"]);
        assert!(mcp_secrets(&parse_mcp(br#"{"command": "x", "args": ["${NOT_HERE}"]}"#).unwrap()).is_empty());
    }

    #[test]
    fn secrets_fill_every_placeholder_or_the_entry_is_refused() {
        let server = parse_mcp(GITHUB).unwrap();
        let secrets = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<BTreeMap<_, _>>();
        let filled = fill_secrets(&server, &secrets(&[("GITHUB_TOKEN", " ghp_1\n"), ("ORG", "${A_1}"), ("A_1", "a")])).unwrap();
        assert_eq!(filled.headers["Authorization"], "Bearer ghp_1");
        // One pass: a value holding a placeholder stays as typed.
        assert_eq!(filled.headers["X-Org"], "${A_1}-${A_1}");
        assert_eq!(filled.env["A"], "a and ${ 1} and $ORG and ${");
        assert_eq!(filled.url, server.url);

        assert_eq!(fill_secrets(&server, &secrets(&[("GITHUB_TOKEN", "t"), ("ORG", "o")])), Err(CatalogError::MissingSecret("A_1".into())));
        assert_eq!(fill_secrets(&server, &secrets(&[("GITHUB_TOKEN", "  "), ("ORG", "o"), ("A_1", "a")])), Err(CatalogError::MissingSecret("GITHUB_TOKEN".into())));
        assert_eq!(fill_secrets(&server, &secrets(&[("GITHUB_TOKEN", "t\r\nX-Evil: 1"), ("ORG", "o"), ("A_1", "a")])), Err(CatalogError::BadSecret("GITHUB_TOKEN".into())));
    }

    #[test]
    fn a_servers_state_reads_its_name_in_mcp_json() {
        let item = index(r#"{"id": "github", "kind": "mcp", "name": "GitHub", "rev": "1", "path": "mcp/github.json"}"#).items.remove(0);
        let server = parse_mcp(br#"{"command": "x"}"#).unwrap();
        let config = |pairs: &[(&str, &McpServerConfig)]| McpConfig { mcp_servers: pairs.iter().map(|(k, v)| (k.to_string(), (*v).clone())).collect() };
        let record = Installed { source: "https://a.example/".into(), kind: ItemKind::Mcp, id: "github".into(), role: None, rev: "1".into(), files: vec![], hash: mcp_hash(&server) };
        let at = |record: Option<&Installed>, config: &McpConfig| mcp_state(&item, "https://a.example/", record, config);

        assert_eq!(at(None, &config(&[("other", &server)])), ItemState::Available);
        assert_eq!(at(Some(&record), &config(&[("github", &server)])), ItemState::Installed);
        assert_eq!(at(None, &config(&[("github", &server)])), ItemState::Taken);
        assert_eq!(at(None, &config(&[("GitHub", &server)])), ItemState::Taken);
        assert_eq!(at(Some(&record), &config(&[])), ItemState::Available);
        // The tab's switches are not an edit; a changed command is.
        let off = McpServerConfig { disabled: true, ..server.clone() };
        let mut hidden = server.clone();
        hidden.show_tool("wipe", false);
        assert_eq!(at(Some(&record), &config(&[("github", &off)])), ItemState::Installed);
        assert_eq!(at(Some(&record), &config(&[("github", &hidden)])), ItemState::Installed);
        let edited = McpServerConfig { command: "y".into(), ..server.clone() };
        assert_eq!(at(Some(&record), &config(&[("github", &edited)])), ItemState::Changed);
    }
}
