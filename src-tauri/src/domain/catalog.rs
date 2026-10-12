//! The catalog (`docs/32-catalog.md`): skills, commands, runbooks, MCP servers
//! and hooks someone published as files, listed by an `index.json` beside them
//! and read over plain HTTP.
//!
//! Everything in an index is a stranger's data. Its paths become files under
//! the app directory once installing exists (CT-2), so they are checked here,
//! before anything is shown: a path that could leave the folder it is meant
//! for never gets as far as a row.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use url::Url;

use super::skills::validate_skill_name;

/// The catalog every install starts with — the app's own, on GitHub, read
/// through its raw files. The user's `catalog.sources` come after it.
pub const DEFAULT_SOURCE: &str = "https://raw.githubusercontent.com/gribanoveu/kibo-agent-catalog/main/";

/// The one file a source is asked for first.
pub const INDEX_FILE: &str = "index.json";

/// Past this an index is not a list of a few dozen entries.
pub const MAX_INDEX_BYTES: u64 = 1024 * 1024;

/// The index format this app reads. A newer one is refused rather than guessed at.
const INDEX_VERSION: u64 = 1;

/// The roles a runbook may be for — the folders `infra::runbooks_store` reads.
const RUNBOOK_ROLES: &[&str] = &["kubernetes"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
        None if raw.files.is_empty() => return Err("a skill lists its files".into()),
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
    fn a_file_is_joined_under_its_source() {
        let base = source_base("https://raw.githubusercontent.com/o/r/main").unwrap();
        assert_eq!(file_url(&base, "skills/review/SKILL.md").unwrap().as_str(), "https://raw.githubusercontent.com/o/r/main/skills/review/SKILL.md");
        assert_eq!(file_url(&base, INDEX_FILE).unwrap().as_str(), "https://raw.githubusercontent.com/o/r/main/index.json");
    }
}
