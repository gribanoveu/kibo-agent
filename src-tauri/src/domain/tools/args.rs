//! The arguments of each tool, as the model sends them — Kubernetes and
//! the checklist have files of their own.

use super::*;

/// One MCP tool `toolSearch` found: the name it is called by now, its
/// server, and what the server says it does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FoundTool {
    pub name: String,
    pub server: String,
    pub description: String,
}

/// `toolSearch`. `limit` is the tool's default when absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ToolSearchArgs {
    pub query: String,
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_u32")]
    pub limit: Option<u32>,
}

/// `webSearch`. `maxResults` is the tool's default when absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WebSearchArgs {
    pub query: String,
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_u32")]
    pub max_results: Option<u32>,
    #[serde(default)]
    pub time_range: Option<crate::domain::web_search::TimeRange>,
}

/// `deepseekSearch`: `webSearch`'s, less `timeRange` — `after:` in the query says it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DeepseekSearchArgs {
    pub query: String,
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_u32")]
    pub max_results: Option<u32>,
}

/// `board`. One flat object rather than a tagged enum: which fields go with
/// which `op` is the tool's to check, and its error says so to the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct BoardArgs {
    pub op: BoardOp,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub column: Option<crate::domain::board::Column>,
    #[serde(default)]
    pub due: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum BoardOp {
    #[default]
    List,
    Add,
    Move,
    Edit,
}

/// `mongo`. Flat like `board`'s: which fields go with which `op` is the
/// tool's to check. The query texts are mongosh's, read as the page reads
/// them; an object sent instead of text is taken as its JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MongoArgs {
    pub op: MongoOp,
    /// A connection's name, as the user named it on the MongoDB page.
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_text")]
    pub connection: Option<String>,
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_text")]
    pub database: Option<String>,
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_text")]
    pub collection: Option<String>,
    /// For `collections`: part of a collection's name to look for.
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_text")]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_text")]
    pub filter: Option<String>,
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_text")]
    pub projection: Option<String>,
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_text")]
    pub sort: Option<String>,
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_text")]
    pub pipeline: Option<String>,
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_usize")]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum MongoOp {
    #[default]
    Collections,
    Schema,
    Find,
    Count,
    Aggregate,
    Indexes,
}

/// `webFetch`. `query` picks the passages of a long page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WebFetchArgs {
    pub url: String,
    #[serde(default)]
    pub query: Option<String>,
}

/// A call to an MCP tool: the full name the model used, and whatever it
/// passed. The arguments are the server's to check — only their being an
/// object is checked here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpCallArgs {
    pub name: String,
    pub arguments: serde_json::Value,
}

/// `readOutput` and `stopProcess`: which process. Optional in the type so a
/// missing number is the tool's error, worded for the model, not a parse
/// failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProcessArgs {
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_u32")]
    pub id: Option<u32>,
}

/// `readOutput`: which process, and how long to wait for it to end first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ReadOutputArgs {
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_u32")]
    pub id: Option<u32>,
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_u32")]
    pub wait_seconds: Option<u32>,
}

/// `readTerminal`: which terminal — the newest when absent — and how many
/// of its last lines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ReadTerminalArgs {
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_u32")]
    pub id: Option<u32>,
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_u32")]
    pub lines: Option<u32>,
}

/// `explore`: what to find out. The helper sees nothing else of the
/// conversation, so this is the whole brief — unless `fork`, when it starts
/// from the conversation itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ExploreArgs {
    pub task: String,
    #[serde(default)]
    pub fork: bool,
}

/// `review`: what to review. `from` and `to` name the range's ends; left out,
/// the app picks — uncommitted changes first, then this chat's work, then
/// the branch against its upstream or base. `focus` is the caller's accent,
/// one or two phrases, appended to the reviewer's brief as one line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ReviewArgs {
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub to: Option<String>,
    #[serde(default)]
    pub focus: Option<String>,
}

/// `remember`: one fact for later conversations in this folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RememberArgs {
    pub fact: String,
}

/// `present` arguments: the page, by its path in the chat's folder, and what
/// to call it on its card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PresentArgs {
    pub path: String,
    pub title: String,
}

/// `writePlan` arguments: the whole plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WritePlanArgs {
    pub content: String,
}

/// What `offer` can put to the user. Closed: free text would end up as a
/// button under every answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum OfferKind {
    #[default]
    Review,
    Commit,
    Worktree,
    PullRequest,
    Implement,
}

/// `offer` arguments: what, and why — the user sees both on the button.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct OfferArgs {
    pub kind: OfferKind,
    pub reason: String,
}

/// `skill` arguments: a skill by name, or one of its files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SkillArgs {
    pub name: String,
    #[serde(default)]
    pub path: Option<String>,
}

/// `readFile` arguments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ReadFileArgs {
    /// Path relative to the scope root. Empty when only `paths` was sent.
    #[serde(default)]
    pub path: String,
    /// Several files, each read whole, in one call — what the model otherwise
    /// asked a shell `for f in …; cat -n` loop for (263 of 1779 logged calls).
    /// `path`, when also sent, is read first.
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_string_list")]
    pub paths: Option<Vec<String>>,
    /// 1-indexed, inclusive. `None` reads from the start. Out-of-range values
    /// are clamped rather than rejected.
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_u32")]
    pub start_line: Option<u32>,
    /// 1-indexed, inclusive. `None` reads through the end.
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_u32")]
    pub end_line: Option<u32>,
    /// The file's declarations and headings with their lines, instead of its
    /// text. The range is ignored: there is no text to slice.
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_bool")]
    pub outline: Option<bool>,
}

/// The files a `readFile` call's raw arguments name: `path`, then `paths` —
/// a list, or one string as the parser also takes it.
pub fn read_paths(args: &serde_json::Value) -> Vec<String> {
    let many = match args.get("paths") {
        Some(serde_json::Value::Array(items)) => items.iter().collect(),
        Some(one) => vec![one],
        None => Vec::new(),
    };
    args.get("path")
        .into_iter()
        .chain(many)
        .filter_map(serde_json::Value::as_str)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect()
}

/// One file of a `readFile` with `paths`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadOne {
    /// As the model asked for it.
    pub path: String,
    /// Lines `1..=end_line` of it; empty when `error` is set.
    pub content: String,
    pub end_line: u32,
    pub total_lines: u32,
    /// Stopped before the end: the call's shared read limit ran out.
    #[serde(default)]
    pub truncated: bool,
    /// Why this one file could not be read. The others still were.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// One declaration or heading of a `readFile` outline, with the lines it
/// spans — a range the next `readFile` can take as it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutlineEntry {
    /// Qualified by what encloses it: `RepoIndexer.sync`.
    pub name: String,
    pub start_line: u32,
    pub end_line: u32,
}

/// `grep` arguments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GrepArgs {
    /// Rust `regex` syntax — no backreferences, no lookaround.
    pub pattern: String,
    /// A file or subdirectory to search under, relative to the scope root.
    /// `None`, `""` or `"."` searches everything.
    pub path: Option<String>,
    /// Which files to search: a glob over the file *name* (`*.java`), or over
    /// the path when it has a `/` in it (`src/main/**`).
    pub glob: Option<String>,
    /// Which files to leave out, as a glob over the path (`src/docs/**`).
    /// A project's documentation or generated code can bury the code the
    /// model is after, and one include glob cannot also exclude.
    #[serde(default)]
    pub exclude: Option<String>,
    /// Default false — an exact, case-sensitive match unless asked otherwise.
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_bool")]
    pub case_insensitive: Option<bool>,
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_usize")]
    pub max_results: Option<usize>,
    /// Lines of context around each hit. `None`/`0` returns the matching line
    /// alone. Exists for the model: without it every "what does this line
    /// actually do" costs a follow-up `readFile` round trip.
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_usize")]
    pub context_lines: Option<usize>,
    /// One hit per file, its first — `grep -l` with the line that proves it.
    /// `maxResults` then caps files. Asking "who uses each of these names"
    /// otherwise went to a shell loop over grep.
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_bool")]
    pub files_only: Option<bool>,
}

/// One line hit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrepMatch {
    /// Relative to the scope root, `/`-separated — the same spelling
    /// `readFile` takes, so a hit round-trips without editing.
    pub path: String,
    /// 1-indexed.
    pub line: u32,
    pub text: String,
    /// Lines before the hit, oldest first. Omitted from the wire form when
    /// empty, so a plain search's shape is unchanged by the feature existing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub before: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after: Vec<String>,
}

/// `listFiles` arguments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ListFilesArgs {
    /// Subdirectory relative to the scope root; `None`/`"."` lists the root.
    pub path: Option<String>,
    /// Levels below `path`, following the walker's convention: `path` itself
    /// is depth 0, its direct children depth 1. `Some(0)` is valid and means
    /// no descendants — not an error. `None` is unlimited.
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_u32")]
    pub depth: Option<u32>,
    /// Glob over each entry's file *name*, never its full path, so `"*.rs"`
    /// matches at any depth. Directories are kept regardless: this scopes
    /// which files come back, not the navigable structure.
    pub pattern: Option<String>,
}

/// One listing entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolFileEntry {
    /// Relative to the scope root, `/`-separated — the spelling `readFile`
    /// takes, so an entry round-trips without editing.
    pub path: String,
    pub is_dir: bool,
    /// Bytes, for a file too large for one `readFile` — the one size that
    /// changes what to do with it. `None` for the rest and for folders.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

/// `writeFile` arguments. Creates the file or replaces it whole.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WriteFileArgs {
    pub path: String,
    pub content: String,
}

/// The line-diff summary attached to every settled write.
///
/// Consumed by two independent readers: the UI's `+N −M` badge and diff view,
/// and the model itself — which otherwise sees only `{"path": "…"}` and has no
/// way to confirm what landed on disk. `lines_added`/`lines_removed` are the
/// true totals even when `unified_diff` was cut short.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FileDiffStats {
    pub lines_added: u32,
    pub lines_removed: u32,
    pub unified_diff: String,
    pub truncated: bool,
}

/// One anchored replacement. `old` must occur exactly once in the file's
/// current content — see `services::ai_tools::tools::edit_file`, where not
/// matching, matching more than once, and two edits overlapping all reject the
/// whole call before anything is written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEdit {
    pub old: String,
    pub new: String,
}

/// `editFile` arguments. The file must already exist — creating one stays
/// `writeFile`'s job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct EditFileArgs {
    pub path: String,
    pub edits: Vec<FileEdit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DeleteArgs {
    pub path: String,
    /// For a directory: omitted or false refuses one that has contents, so
    /// an over-broad path costs one refusal instead of a tree.
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_bool")]
    pub recursive: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MoveArgs {
    pub path: String,
    pub new_path: String,
}

/// `gitDiff` arguments — a file, or a directory file by file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GitDiffArgs {
    pub path: String,
    /// `"unstaged"` (default) or `"staged"`. Ignored when `commit` is set.
    pub scope: Option<String>,
    /// A commit hash or ref. When set, diffs that commit against its parent.
    pub commit: Option<String>,
    /// Totals per file only, no text — `git diff --stat`: the cheap look at
    /// a large change before asking for the files that matter.
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_bool")]
    pub stat: Option<bool>,
}

/// `semanticSearch` arguments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SemanticSearchArgs {
    /// Searched by meaning, whole, and for names and words.
    pub query: String,
    /// The words that must match as text, when they are not simply the
    /// query's own. The two halves want different inputs: meaning does best
    /// with a whole sentence, text ranking with its few load-bearing words —
    /// and the model knows which those are better than a tokenizer does.
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_string_list")]
    pub fts: Option<Vec<String>>,
    /// Other wordings of the same question, searched in the same call — the
    /// English of a Russian question, say.
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_string_list")]
    pub queries: Option<Vec<String>>,
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_usize")]
    pub top_k: Option<usize>,
    /// Longer text per match. Off by default: the result goes into the
    /// model's context, and `readFile` fetches a range more precisely.
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_bool")]
    pub preview: Option<bool>,
    /// Documentation too — docs folders and prose files. Off by default: a
    /// question about code drowns in design notes and API descriptions that
    /// share its words, and the model is told when some were left out.
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_bool")]
    pub include_docs: Option<bool>,
    /// Same as `grep`'s: a file name, or a path when it has a `/`.
    #[serde(default)]
    pub glob: Option<String>,
    /// Same as `grep`'s: a glob over the path to leave out.
    #[serde(default)]
    pub exclude: Option<String>,
}

/// Search of the open folder's code — `services::code_search::search` behind a
/// port, because the tools live below the service that owns the index.
/// `None` in [`ToolDeps`] when no folder's index is open.
pub type CodeSearchFn = std::sync::Arc<
    // The wordings (the query first), `fts`, how many, and what may be returned.
    dyn Fn(&[&str], Option<&[String]>, usize, &crate::domain::code_search::SearchFilter)
            -> Result<crate::domain::code_search::CodeSearchResult, String>
        + Send
        + Sync,
>;

/// `gitBlame` arguments. The range mirrors `readFile`'s.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GitBlameArgs {
    pub path: String,
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_u32")]
    pub start_line: Option<u32>,
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_u32")]
    pub end_line: Option<u32>,
}

/// `gitLog` arguments: every filter narrows, none is required.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GitLogArgs {
    /// A file or directory; only commits that changed something under it.
    /// It need not exist any more — the history of a deleted file is asked
    /// for too.
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default, deserialize_with = "crate::domain::flexible_args::opt_u32")]
    pub limit: Option<u32>,
    /// Kept when the message contains it, ignoring case.
    #[serde(default)]
    pub query: Option<String>,
}

/// One line of `gitLog`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogCommit {
    /// Short hash, as `gitDiff`'s `commit` takes it.
    pub commit: String,
    pub date: String,
    pub author: String,
    /// The message's first line.
    pub summary: String,
    /// Files it changed — under `path`, when one was given.
    pub files: u32,
}

/// As far as the last fetch knows: nothing here talks to the remote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitUpstream {
    /// `origin/main`.
    pub name: String,
    /// Local commits the upstream does not have.
    pub ahead: usize,
    /// Upstream commits the branch does not have.
    pub behind: usize,
    /// When the remote was last fetched, local time — how old "up to date"
    /// is. `None` when no fetch was ever recorded.
    #[serde(default)]
    pub fetched: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitFileStatus {
    /// Relative to the scope root, like every other path a tool reports.
    pub path: String,
    /// One letter: `M`, `A`, `D`, `R`, `U`, `?`.
    pub status: String,
}

/// One file of a directory's diff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitFileDiff {
    pub path: String,
    pub diff: FileDiffStats,
    pub is_binary: bool,
}

/// A run of consecutive lines that arrived in one commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlameHunk {
    pub start_line: u32,
    pub line_count: u32,
    /// Short hash — the long one costs context and buys nothing to read.
    pub commit: String,
    pub author: String,
    pub date: String,
    /// First line of the commit message.
    pub summary: String,
}
