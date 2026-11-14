//! How a call fails, and how a result that did nothing begins.

use super::*;

/// How a tool result begins when its call did nothing: it failed, the user
/// refused it, or the turn stopped before it ran. Read back by the compaction's
/// file lists (`domain::compaction::with_file_lists`), which leave out a write
/// that never happened.
pub const TOOL_ERROR_PREFIX: &str = "Error: ";
pub const TOOL_DENIED_PREFIX: &str = "Denied by the user";
/// Said to the model after a denial, the one time it needs to know: the
/// prompt carries nothing about approval.
pub const AFTER_DENIAL: &str = "Do not retry this call or reach the same result another way: ask the user how to proceed, and cancel the checklist item it belonged to with this reason.";
pub const TOOL_NOT_RUN_PREFIX: &str = "Not run: ";

/// Why a tool call could not be carried out.
///
/// Data all the way to the boundary, per `AGENTS.md`: the string form exists
/// only because the model reads it. Every message is addressed to the model and
/// says what to do differently, not merely what went wrong.
#[derive(Debug, Error)]
pub enum ToolError {
    #[error("path escapes tool root: {0}")]
    PathEscape(String),
    /// A tool the current conversation mode does not offer. Worded for the
    /// model, which has to decide what to do next: the reason it is missing
    /// matters more than the fact, or it simply tries again.
    #[error(
        "the tool `{0}` is not available in this conversation mode — the user chose a mode that cannot change the repository. Say what would need to be done instead of doing it."
    )]
    NotOfferedInMode(String),
    /// A tool the chat's role does not have. Chat has no folder: the role's
    /// tools are all there is, and saying so is what stops a model reaching
    /// for the agent's from memory.
    #[error(
        "the tool `{0}` is not available in this chat — its role has no such tool, and there is no folder to work in. Answer from what you have, or say what the user could do."
    )]
    NotOfferedInChat(String),
    /// A tool that works in the open folder, called where there is none. The
    /// chat's preflight refuses these first; this is the dispatcher's own
    /// answer, so a tool never has to be handed a folder that does not exist.
    #[error("the tool `{0}` works in a folder, and this chat has none")]
    NoFolder(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("not a file: {0}")]
    NotAFile(String),
    #[error("io error: {0}")]
    Io(#[source] std::io::Error),
    /// A `glob`, `exclude` or `listFiles` `pattern` that does not compile as
    /// a glob.
    #[error("invalid glob pattern: {0}")]
    InvalidPattern(String),
    /// A `grep` `pattern` that does not compile as a regex — named apart from
    /// a glob, or the model fixes the wrong argument.
    #[error("invalid regex in `pattern`: {0}")]
    InvalidRegex(String),
    /// An `editFile` edit's `old` text appears nowhere in the file.
    /// `nearest` is the line most like its first one — usually the same line
    /// indented otherwise, or with a word changed — so the retry copies it
    /// rather than guessing again.
    #[error("edit text not found: {text}{}", closest_line(.nearest))]
    EditTextNotFound { text: String, nearest: Option<(u32, String)> },
    /// An `editFile` edit's `old` text appears more than once — which
    /// occurrence was meant is unknowable, so nothing is written. `.1` is the
    /// match count, so the model learns how much more context to include.
    #[error("edit text is not unique — matched {1} times: {0}")]
    EditTextAmbiguous(String, usize),
    /// The anchor starts or ends inside a word: `line on` in `line one`.
    /// Applied, the edit would rewrite part of a name, and the diff would
    /// look plausible enough to miss.
    #[error("edit text starts or ends inside a word — it matched within `{1}`; anchor on whole words: {0}")]
    EditInsideWord(String, String),
    /// The edit's `new` holds its `old`, and the file already holds `new`:
    /// it was applied before — a retry of a call that had in fact succeeded.
    /// Applied again it would add its insertion a second time.
    #[error("edit already applied — the file already contains its new text, and applying it again would duplicate what it adds: {0}")]
    EditAlreadyApplied(String),
    /// Which of several edits an anchor error is about. The call is refused
    /// whole, so without it the model rewrites every anchor to fix one.
    #[error("edit {index} of {of}: {reason}")]
    InEdit { index: usize, of: usize, reason: Box<ToolError> },
    /// Two edits in one call matched overlapping regions of the original
    /// content: applying both would be order-dependent or would corrupt one of
    /// them, so the whole call is rejected.
    #[error("edits overlap in the same region of the file")]
    EditsOverlap,
    /// `delete` of a directory with contents, without `recursive`.
    #[error("directory is not empty: {0}")]
    DirectoryNotEmpty(String),
    /// A `move` whose destination exists. Nothing is overwritten — the check
    /// happens before the rename, not after.
    #[error("already exists: {0}")]
    AlreadyExists(String),
    /// A wire name matching no known tool.
    #[error("unknown tool: {0}")]
    UnknownTool(String),
    /// The server could not be asked: gone, not answering, stopped. Worded
    /// for the model, which should not keep calling it.
    #[error("the MCP server is not available: {0}. Do not call its tools again in this turn unless the user asks.")]
    McpUnavailable(String),
    /// The tool ran and said it failed (`isError`). Its own text, for the
    /// model to act on.
    #[error("the tool reported an error: {0}")]
    McpToolFailed(String),
    /// A `PreToolUse` hook exited 2. Its stderr, for the model to act on —
    /// the user's rule, which the model should respect rather than route
    /// around.
    #[error("a hook refused this call: {0}")]
    BlockedByHook(String),
    /// Arguments that did not deserialize into the struct their tool expects.
    ///
    /// `reason` is JSON-path-annotated (`edits[1]: missing field \`old\``)
    /// rather than a byte offset into the raw JSON. The failure this exists for
    /// is a model getting one field name wrong deep inside an array argument:
    /// "line 1 column 7275" does not tell it which element to fix, a path does.
    #[error("invalid arguments for {tool}: {reason}")]
    InvalidArguments { tool: String, reason: String },
    /// A `todo write` whose new titles would push the list past its maximum —
    /// rejected outright rather than silently truncated, so the model decides
    /// what to drop or split instead of discovering later that it was cut.
    #[error("todo list already has {current} open or unfinished task(s); adding {adding} more would exceed the {max} maximum — complete or cancel the ones left, and once none is open the next write starts a new list")]
    TooManyTasks {
        current: usize,
        adding: usize,
        max: usize,
    },
    /// A new list longer than any list may be: nothing on it to finish
    /// first, so the way out is fewer, larger steps.
    #[error("a todo list holds at most {max} tasks, and this one has {adding} — merge small steps into one task, then write it again")]
    TodoListTooLong { adding: usize, max: usize },
    /// A `todo update` naming an id that is not in the list — usually a stale
    /// id from earlier in the conversation, or a checklist the model never
    /// created before trying to complete a task on it.
    ///
    /// `available` is `Some` when the caller knows the whole list, so the
    /// message can name the way out instead of leaving a dead end.
    #[error("{}", task_not_found_message(id, available))]
    TaskNotFound {
        id: String,
        available: Option<Vec<String>>,
    },
    /// A write to a file the agent never read. Not pedantry: replacing a file
    /// whose contents were never seen destroys work nobody looked at, and the
    /// model has no way to know what it just lost.
    #[error("read {0} before writing to it — a file you have not read may not be what you expect; an outline is not a read")]
    FileNotRead(String),
    /// A wholesale replacement of a file the agent has only read part of.
    #[error("you have only read part of {0} — read it in full before replacing it, or use editFile to change just the part you know")]
    FileReadInPart(String),
    /// `delete` of a file the agent has not read whole: what it removes
    /// comes back in the result, and that is only worth something when the
    /// agent has seen it.
    #[error("read {0} in full before deleting it — a file you have not read may not be what you expect; an outline is not a read")]
    DeleteNotReadInFull(String),
    /// The file moved under the agent between the read and the change. The
    /// classic loss: the agent read, a person edited in their own editor, the
    /// agent changed its stale copy over the top.
    #[error("{0} changed on disk since you read it — read it again before changing it: another running agent or a person may have edited it, and changing it from what you last saw would overwrite their work")]
    FileChangedSinceRead(String),
    #[error("git {0} would change the shared working tree while another agent turn is running — wait for it to finish, or work in a separate worktree")]
    GitTreeDuringParallel(String),
    /// A git read failed. Carried as a string so git types stay out of the
    /// tool boundary.
    /// Addressed to the model, which still has `grep` and `listFiles`.
    #[error("search is unavailable: {0} — use grep for exact text, or listFiles to look around")]
    SearchUnavailable(String),
    #[error("git error: {0}")]
    Git(String),
    /// The command never started, or could not be read. A command that started
    /// and failed is not this — that is a result with a non-zero exit code.
    #[error("{0}")]
    Command(String),
    #[error(transparent)]
    Background(#[from] crate::domain::background::BackgroundError),
    #[error(transparent)]
    Terminal(#[from] crate::domain::terminal::TerminalError),
    #[error(transparent)]
    Finding(#[from] crate::domain::review::FindingError),
    /// A skill that could not be loaded — unknown, or its `SKILL.md` broken.
    #[error("{0}")]
    Skill(String),
    /// The helper turn ended without an answer.
    #[error("explore: {0}")]
    Explore(String),
    /// The review could not be set up or run; the reason is the model's to
    /// read and carry back to the user.
    #[error("review: {0}")]
    Review(String),
    /// A `remember` note that was not kept, and why.
    #[error("remember: {0}")]
    Memory(String),
    /// A change asked for while "Allow changes" is off. The switch
    /// is the user's; the model says what it would do and where to turn it on.
    #[error("read-only: this chat may not change the cluster. Tell the user what you would change, and that they can allow it by turning on \"Allow changes\" in the cluster panel under the message box.")]
    KubeReadOnly,
    /// The backup could not be written, so nothing was changed.
    #[error("nothing was changed: the backup that makes it undoable could not be written — {0}")]
    KubeBackup(String),
    /// An undo of an object somebody else has changed since: the app does
    /// not write over what it did not write.
    #[error("not undone: {0}. Whether to change it again is the user's decision, not yours — tell them what you found and what it is now, and do nothing more to it unless they then ask.")]
    KubeChangedSince(String),
    /// A Kubernetes tool in a chat with no cluster to read. The prompt's note
    /// about the user's setup says why; the model tells the user what to fix.
    #[error("no cluster is pinned to this chat — tell the user what to set up, as the note about their cluster says")]
    NoCluster,
    /// `webSearch` or `webFetch` while web search is off: called from memory
    /// of an earlier turn.
    #[error("web search is off — tell the user they can turn it on in Settings → Web search, and answer without searching")]
    NoWebSearch,
    /// `deepseekSearch` or `deepseekFetch` with no DeepSeek provider to
    /// take the key from.
    #[error("no DeepSeek provider with an API key is set up — tell the user to add DeepSeek in Settings → Models, and answer without searching")]
    NoDeepseek,
    #[error(transparent)]
    WebSearch(#[from] crate::domain::web_search::WebSearchError),
    #[error(transparent)]
    Kube(#[from] crate::domain::kube::KubeError),
    #[error(transparent)]
    Board(#[from] crate::domain::board::BoardError),
    #[error(transparent)]
    Mongo(#[from] crate::domain::mongo::MongoError),
    #[error(transparent)]
    Prototype(#[from] crate::domain::prototype::PrototypeError),
    /// A tool of a feature the user has off — called from memory of a turn
    /// when it was on.
    #[error("the tool `{tool}` belongs to {feature}, which is turned off — tell the user they can turn it on on the Catalog page")]
    FeatureOff { tool: String, feature: String },
    /// A tool of a feature that is on, kept from the model on its Catalog panel.
    #[error("the user keeps the tools of {feature}, `{tool}` among them, from you — tell them they can give them to you on {feature}'s panel on the Catalog page")]
    ToolsKept { tool: String, feature: String },
}

fn task_not_found_message(id: &str, available: &Option<Vec<String>>) -> String {
    match available {
        Some(ids) if ids.is_empty() => format!(
            "no task with id: {id} — the checklist is empty, create tasks with op \"write\" before updating one"
        ),
        Some(ids) => format!("no task with id: {id} — current ids: {}", ids.join(", ")),
        None => format!("no task with id: {id}"),
    }
}

/// How [`ToolError::EditTextNotFound`] points at the line the anchor was
/// probably meant to be.
fn closest_line(nearest: &Option<(u32, String)>) -> String {
    match nearest {
        Some((line, text)) => format!(" — the closest line is {line}: `{text}`"),
        None => String::new(),
    }
}
