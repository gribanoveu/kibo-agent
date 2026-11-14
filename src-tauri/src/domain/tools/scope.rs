//! Where a call runs and what it is handed: the folder, the services it
//! reaches, and the preview a card shows before it runs.

use super::*;

/// The filesystem boundary a tool call executes against.
///
/// In Alfa Atlas this carried seven fields: a mode, three roots, a search
/// filter prefix, the tool allowlist, and external `@deps` roots. Six of them
/// existed to express "the model may read the whole repository but may only
/// write inside the documentation subtree" — a distinction a coding agent does
/// not have. What is left is the one thing that was always doing the work.
///
/// Constructed once per session so every tool resolves against an already
/// canonical root: a boundary compared against a path with `..` or a symlink
/// still in it is not a boundary.
#[derive(Debug, Clone)]
///
/// Built by `ToolScope::new` in `services::ai_tools::resolve`, which resolves
/// the root on disk — I/O this layer does not do.
pub struct ToolScope {
    pub(crate) root: PathBuf,
}

impl ToolScope {
    /// The canonical root. Every path a tool touches has to resolve under it —
    /// enforced in one place, `services::ai_tools::resolve`, rather than by
    /// each tool remembering to check.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// What a call would do, worked out without doing it.
///
/// Approving a write means approving its contents, and the arguments alone do
/// not show them: `{"path":"Mapper.java"}` is not an answer to "what would
/// change". Computed on demand for a paused round, never as part of running
/// one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ToolPreview {
    /// What the file would look like afterwards, against what is there now.
    #[serde(rename_all = "camelCase")]
    Diff { path: String, diff: FileDiffStats },
    /// How much a recursive delete would actually take. The scariest thing to
    /// approve is a path that turned out broader than it looked.
    #[serde(rename_all = "camelCase")]
    Removes { path: String, files: usize },
    /// A command line and where it would run.
    #[serde(rename_all = "camelCase")]
    Command { command: String, cwd: String },
    /// A change to a cluster: where, and what it would become — the server
    /// has already accepted it as a dry run.
    #[serde(rename_all = "camelCase")]
    Change {
        /// `context eks-prod · namespace payments` — read before agreeing.
        place: String,
        /// `Deployment/api: 3 → 0 replicas`.
        summary: String,
        /// What else to know first: whether it can be undone.
        notes: Vec<String>,
        /// The user marked this cluster as production: the card says so first.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        production: bool,
        /// A manifest's objects, each as it is against what it would become.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        diffs: Vec<ChangeDiff>,
    },
    /// The call would not succeed anyway, and this says why — better learned
    /// before approving it than after.
    #[serde(rename_all = "camelCase")]
    Failed { reason: String },
    /// Nothing worth showing beyond the arguments themselves.
    Nothing,
}

/// One object of a change to a cluster: `Deployment/api`, and its YAML now
/// against its YAML after.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeDiff {
    pub title: String,
    pub diff: FileDiffStats,
}

/// What a tool needs from the environment, beyond the workspace it acts on.
///
/// Empty until now, which is why the dispatcher took no such argument: every
/// tool so far needed a path and nothing else. `runCommand` needs a shell and
/// somewhere to send output as it arrives, and threading those through the one
/// dispatcher keeps the tool boundary where it is.
#[derive(Clone, Default)]
pub struct ToolDeps<'a> {
    pub shell: crate::domain::command_exec::Shell,
    /// Where a running command's output goes as it is produced. `None`
    /// collects it and reports it only at the end, which is what a test wants
    /// and what a turn must not do.
    pub output: Option<crate::domain::command_exec::CommandSink>,
    /// The open folder's index; `None` when there is none.
    pub search: Option<CodeSearchFn>,
    /// The skills this turn's prompt listed — the only ones `skill` loads,
    /// each from its own folder.
    pub skills: Vec<crate::domain::skills::Skill>,
    /// The connected servers' tools this turn offers.
    pub mcp: crate::domain::mcp::McpTools,
    /// The turn's stop button, for a tool that waits on someone else — an
    /// MCP call. `None` never stops.
    pub cancelled: Option<&'a dyn Fn() -> bool>,
    /// How many notes the user has typed in this turn, a count that only
    /// grows; a wait ends early when it changes. `None` never does.
    pub notes_typed: Option<&'a dyn Fn() -> u64>,
    /// Puts an MCP server's question (the server's name, the question) to
    /// the user and waits for the answer; `None` where nobody is there to
    /// ask, and the question is declined.
    pub ask: Option<&'a dyn Fn(&str, &crate::domain::mcp::McpQuestion) -> crate::domain::mcp::McpAnswer>,
    /// The background processes; `None` where there are none to have, and
    /// `runCommand` with `background` says so.
    pub processes: Option<std::sync::Arc<dyn crate::domain::background::BackgroundProcesses>>,
    /// A git tree mutation stays in the foreground while its admission lock is
    /// held: returning a live background process would release the guard early.
    pub tree_mutation: bool,
    /// The user's own terminals; `None` where there are none to have.
    pub terminals: Option<std::sync::Arc<dyn crate::domain::terminal::UserTerminals>>,
    /// A review worker's files and findings, for `reportFinding`; `None`
    /// outside a review.
    pub review: Option<std::sync::Arc<crate::domain::review::ReviewDesk>>,
    /// Runs `explore`'s task as a helper turn; `None` where there is no model
    /// to run it with.
    pub explore: Option<&'a dyn Fn(&ExploreArgs) -> Result<ToolResult, ToolError>>,
    /// Runs `review`'s range as a helper turn in Review mode; `None` where
    /// there is no model to run it with. The default is the readable refusal
    /// a test wants, as with `explore`.
    pub review_run: Option<&'a dyn Fn(&ReviewArgs) -> Result<ToolResult, ToolError>>,
    /// Asks the model whether text may be kept — a `remember` note — under
    /// the tool's guard prompt, which
    /// is answered SAVE or REFUSE; `Err` says why not, or why it could not be
    /// asked. `None` where there is no model.
    pub check: Option<&'a dyn Fn(&str, &str) -> Result<(), String>>,
    /// The cluster a Kubernetes chat is pinned to; `None` where there is
    /// none, or its kubeconfig could not be read.
    pub kube: Option<crate::domain::kube::PinnedCluster<'a>>,
    /// The runbooks this turn's prompt listed — the only ones `kubeRunbook`
    /// reads.
    pub runbooks: Vec<crate::domain::runbooks::Runbook>,
    /// The web a chat's `webSearch` and `webFetch` reach; `None` while web
    /// search is off.
    pub web: Option<crate::domain::web_search::Web>,
    /// What `deepseekSearch` and `deepseekFetch` reach the web with; `None`
    /// while the feature is off or no DeepSeek provider has a key.
    pub deepseek: Option<crate::domain::web_search::Web>,
    /// The conversation so far: `webFetch` opens only an address that is in it.
    pub history: &'a [crate::domain::llm::LlmMessage],
    /// The board `board` keeps cards on; `None` where there is none.
    pub board: Option<std::sync::Arc<dyn crate::domain::board::TaskBoard>>,
    /// The MongoDB servers `mongo` reads; `None` while the feature is off.
    pub mongo: Option<std::sync::Arc<dyn crate::domain::mongo::MongoServers>>,
    /// The app's own tools this turn offers through `toolSearch` only.
    pub waiting: Vec<crate::domain::llm::LlmToolDefinition>,
    /// Other turns in this checkout, for the worktree offer.
    pub parallel: Option<&'a dyn Fn() -> Vec<String>>,
    /// The base recorded when this chat moved to its worktree.
    pub worktree_base: Option<&'a str>,
    /// The commit `HEAD` pointed at when this chat was first saved — what a
    /// review counts as this chat's work. `None` for a chat without the
    /// marker; the offer of a review reads it so the button and the tool
    /// plan on the same facts.
    pub chat_head: Option<&'a str>,
    /// Whether the chat has a plan to implement: written this turn, or in the
    /// window with something in it.
    pub has_plan: bool,
}
