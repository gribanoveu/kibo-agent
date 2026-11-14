//! A parsed call and what it gave back.

use super::*;

/// A parsed, validated call the model asked for.
///
/// Adjacently tagged (`{"tool": "readFile", "args": {…}}`) rather than a set
/// of functions, which buys one place to check permissions, one place to
/// serialize at the model boundary, one place to log and redact, and a wire
/// shape a test can pin. Adding a tool is a variant here, a variant in
/// [`ToolResult`], a module under `services::ai_tools::tools`, and an arm in
/// the dispatcher — nowhere else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "tool", content = "args", rename_all = "camelCase")]
pub enum ToolCall {
    ReadFile(ReadFileArgs),
    Grep(GrepArgs),
    ListFiles(ListFilesArgs),
    WriteFile(WriteFileArgs),
    EditFile(EditFileArgs),
    Delete(DeleteArgs),
    Move(MoveArgs),
    Todo(TodoArgs),
    GitStatus,
    GitDiff(GitDiffArgs),
    GitBlame(GitBlameArgs),
    GitLog(GitLogArgs),
    RunCommand(crate::domain::command_exec::CommandRequest),
    SemanticSearch(SemanticSearchArgs),
    Skill(SkillArgs),
    WritePlan(WritePlanArgs),
    ReadOutput(ReadOutputArgs),
    StopProcess(ProcessArgs),
    ReadTerminal(ReadTerminalArgs),
    ReportFinding(crate::domain::review::FindingArgs),
    Offer(OfferArgs),
    Explore(ExploreArgs),
    Review(ReviewArgs),
    Remember(RememberArgs),
    KubeList(KubeListArgs),
    KubeGet(KubeGetArgs),
    KubeEvents(KubeEventsArgs),
    KubeLogs(KubeLogsArgs),
    KubeTop(KubeTopArgs),
    KubeFieldHistory(KubeFieldHistoryArgs),
    KubeDiagnose(KubeDiagnoseArgs),
    KubeWaitRollout(KubeWaitRolloutArgs),
    KubeProbe(KubeProbeArgs),
    KubeRunbook(KubeRunbookArgs),
    KubeScale(KubeScaleArgs),
    KubeUndo(KubeUndoArgs),
    KubeSuspend(KubeSuspendArgs),
    KubeRolloutRestart(KubeRolloutRestartArgs),
    KubeRolloutUndo(KubeRolloutUndoArgs),
    KubeApply(KubeApplyArgs),
    KubeDelete(KubeDeleteArgs),
    WebSearch(WebSearchArgs),
    WebFetch(WebFetchArgs),
    DeepseekSearch(DeepseekSearchArgs),
    DeepseekFetch(WebFetchArgs),
    Board(BoardArgs),
    Mongo(MongoArgs),
    Present(PresentArgs),
    ToolSearch(ToolSearchArgs),
    Mcp(McpCallArgs),
}

impl ToolCall {
    pub fn name(&self) -> ToolName {
        match self {
            ToolCall::ReadFile(_) => ToolName::ReadFile,
            ToolCall::Grep(_) => ToolName::Grep,
            ToolCall::ListFiles(_) => ToolName::ListFiles,
            ToolCall::WriteFile(_) => ToolName::WriteFile,
            ToolCall::EditFile(_) => ToolName::EditFile,
            ToolCall::Delete(_) => ToolName::Delete,
            ToolCall::Move(_) => ToolName::Move,
            ToolCall::Todo(_) => ToolName::Todo,
            ToolCall::GitStatus => ToolName::GitStatus,
            ToolCall::GitDiff(_) => ToolName::GitDiff,
            ToolCall::GitBlame(_) => ToolName::GitBlame,
            ToolCall::GitLog(_) => ToolName::GitLog,
            ToolCall::SemanticSearch(_) => ToolName::SemanticSearch,
            ToolCall::RunCommand(_) => ToolName::RunCommand,
            ToolCall::Skill(_) => ToolName::Skill,
            ToolCall::WritePlan(_) => ToolName::WritePlan,
            ToolCall::ReadOutput(_) => ToolName::ReadOutput,
            ToolCall::StopProcess(_) => ToolName::StopProcess,
            ToolCall::ReadTerminal(_) => ToolName::ReadTerminal,
            ToolCall::ReportFinding(_) => ToolName::ReportFinding,
            ToolCall::Offer(_) => ToolName::Offer,
            ToolCall::Explore(_) => ToolName::Explore,
            ToolCall::Review(_) => ToolName::Review,
            ToolCall::Remember(_) => ToolName::Remember,
            ToolCall::KubeList(_) => ToolName::KubeList,
            ToolCall::KubeGet(_) => ToolName::KubeGet,
            ToolCall::KubeEvents(_) => ToolName::KubeEvents,
            ToolCall::KubeLogs(_) => ToolName::KubeLogs,
            ToolCall::KubeTop(_) => ToolName::KubeTop,
            ToolCall::KubeFieldHistory(_) => ToolName::KubeFieldHistory,
            ToolCall::KubeDiagnose(_) => ToolName::KubeDiagnose,
            ToolCall::KubeWaitRollout(_) => ToolName::KubeWaitRollout,
            ToolCall::KubeProbe(_) => ToolName::KubeProbe,
            ToolCall::KubeRunbook(_) => ToolName::KubeRunbook,
            ToolCall::KubeScale(_) => ToolName::KubeScale,
            ToolCall::KubeUndo(_) => ToolName::KubeUndo,
            ToolCall::KubeSuspend(_) => ToolName::KubeSuspend,
            ToolCall::KubeRolloutRestart(_) => ToolName::KubeRolloutRestart,
            ToolCall::KubeRolloutUndo(_) => ToolName::KubeRolloutUndo,
            ToolCall::KubeApply(_) => ToolName::KubeApply,
            ToolCall::KubeDelete(_) => ToolName::KubeDelete,
            ToolCall::WebSearch(_) => ToolName::WebSearch,
            ToolCall::WebFetch(_) => ToolName::WebFetch,
            ToolCall::DeepseekSearch(_) => ToolName::DeepseekSearch,
            ToolCall::DeepseekFetch(_) => ToolName::DeepseekFetch,
            ToolCall::Board(_) => ToolName::Board,
            ToolCall::Mongo(_) => ToolName::Mongo,
            ToolCall::Present(_) => ToolName::Present,
            ToolCall::ToolSearch(_) => ToolName::ToolSearch,
            ToolCall::Mcp(_) => ToolName::Mcp,
        }
    }

    /// Whether *this* call needs a human before it runs — the verdict
    /// [`ApprovalPolicy::requires_approval`] takes.
    ///
    /// Every tool so far answers from its identity alone. `runCommand` will be
    /// the first to answer from its arguments, which is why this is a method on
    /// the call rather than a lookup on the name.
    pub fn is_risky(&self) -> bool {
        match self {
            // The tool stays mutating — Analyst does not offer it —
            // but a line that only reads needs no card.
            ToolCall::RunCommand(request) => command_risk::classify(&request.command) != CommandRisk::ReadOnly,
            _ => self.name().is_mutating(),
        }
    }
}

/// What a settled call gives back to the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "camelCase")]
pub enum ToolResult {
    #[serde(rename_all = "camelCase")]
    File {
        content: String,
        /// 1-indexed and inclusive — the range actually returned after
        /// clamping, not necessarily the one asked for. Both are `0` for an
        /// empty file: there is no line 1 to claim.
        start_line: u32,
        end_line: u32,
        total_lines: u32,
        /// The range asked for reached outside the file and was cut to fit.
        #[serde(default)]
        clamped: bool,
        /// The read stopped at `readFile`'s size limit before `end_line` of
        /// what was asked — the whole file, or a range too long to send.
        #[serde(default)]
        truncated: bool,
    },
    /// `readFile` with `paths`: each file in the order asked.
    Files { files: Vec<ReadOne> },
    /// `readFile` with `outline`: the file's shape rather than its text.
    /// Empty `entries` for a language with no parser, or a file declaring
    /// nothing — `totalLines` still says what reading it would cost.
    #[serde(rename_all = "camelCase")]
    FileOutline {
        path: String,
        entries: Vec<OutlineEntry>,
        total_lines: u32,
    },
    /// `truncated` means "there are more hits than these" — the cap was
    /// reached with matching still to do. A search that quietly stopped at a
    /// limit reads to the model as an exhaustive answer, which is the one
    /// thing `grep` is for.
    #[serde(rename_all = "camelCase")]
    GrepResults {
        matches: Vec<GrepMatch>,
        truncated: bool,
        /// Every hit, shown or not, and the files they are in — counted on
        /// past the cap, so a cut says how much it cut. `0` in results saved
        /// before it was counted.
        #[serde(default)]
        total: usize,
        #[serde(default)]
        total_files: usize,
        /// Counting stopped at its own limit: `total` is a floor.
        #[serde(default)]
        total_is_floor: bool,
        /// Text files not searched — too big, or not UTF-8 — by path. Binary
        /// files are not listed: nothing in them was ever text to find.
        #[serde(default)]
        skipped: Vec<String>,
    },
    /// `truncated` carries the same "there is more here than you are seeing"
    /// contract as `GrepResults`. It matters most on a vendored tree: a real
    /// `node_modules` is tens of thousands of entries, and a listing that
    /// quietly stopped at a cap reads to the model as the whole picture.
    #[serde(rename_all = "camelCase")]
    FileList {
        entries: Vec<ToolFileEntry>,
        truncated: bool,
        /// The `depth` the walk stopped at, when folders there had more in
        /// them: an empty or short listing is then "not looked", not "none".
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stopped_at: Option<u32>,
    },
    #[serde(rename_all = "camelCase")]
    FileWritten {
        path: String,
        diff: FileDiffStats,
    },
    #[serde(rename_all = "camelCase")]
    FileEdited {
        path: String,
        diff: FileDiffStats,
        /// Markdown headings the edit left more of than there were — most
        /// often a section inserted beside the one it meant to replace.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        repeated_headings: Vec<String>,
    },
    /// Carries the diff for the same reason a write does: the model should see
    /// the size of what it just removed, not only that something was removed.
    #[serde(rename_all = "camelCase")]
    FileDeleted {
        path: String,
        diff: FileDiffStats,
    },
    #[serde(rename_all = "camelCase")]
    DirectoryDeleted {
        path: String,
        /// Files that went with it — what `recursive` cost, since no read
        /// stands guard over a tree.
        #[serde(default)]
        files: usize,
        /// Which ones, up to a limit — what a deleted file's diff is to a
        /// file's `delete`: the trace of what an irreversible call removed.
        #[serde(default)]
        listed: Vec<String>,
    },
    #[serde(rename_all = "camelCase")]
    Moved {
        from: String,
        to: String,
        /// For a directory, how many files it carried; `None` for a file.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        files: Option<usize>,
    },
    /// The whole checklist after the change. Returned in full rather than as a
    /// delta because the caller owns the list and this is how it gets it back.
    #[serde(rename_all = "camelCase")]
    Todo { tasks: Vec<Task> },
    #[serde(rename_all = "camelCase")]
    GitStatus {
        branch: Option<String>,
        /// The branch's remote-tracking branch and how far apart they are.
        /// `None` when detached or with no upstream set.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        upstream: Option<GitUpstream>,
        staged: Vec<GitFileStatus>,
        unstaged: Vec<GitFileStatus>,
        conflicted: Vec<GitFileStatus>,
        truncated: bool,
    },
    #[serde(rename_all = "camelCase")]
    GitDiff {
        path: String,
        /// What was compared with what, e.g. `index → working tree`.
        label: String,
        diff: FileDiffStats,
        is_binary: bool,
        /// A commit's whole message, when a commit was diffed: `gitLog` gives
        /// its first line, and `gitBlame` sends the model here for the why.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    /// `gitDiff` on a directory: every changed file under it, each as its own
    /// diff. `truncated` means files were left out; a file whose diff did not
    /// fit the budget is listed with its counts and an empty, truncated diff.
    #[serde(rename_all = "camelCase")]
    GitDiffFiles {
        path: String,
        label: String,
        files: Vec<GitFileDiff>,
        truncated: bool,
        /// As on `GitDiff`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    GitBlame {
        path: String,
        hunks: Vec<BlameHunk>,
        truncated: bool,
    },
    /// Newest first. `truncated` when more commits matched than were kept.
    #[serde(rename_all = "camelCase")]
    GitLog {
        path: String,
        commits: Vec<LogCommit>,
        truncated: bool,
    },
    /// A command that ran. "Ran" is not "succeeded": the exit code is the
    /// answer, and a failing build is a perfectly good result.
    CommandRan(crate::domain::command_exec::CommandOutput),
    /// A finding kept, where it was placed.
    #[serde(rename_all = "camelCase")]
    FindingNoted {
        path: String,
        start_line: u32,
        end_line: u32,
    },
    /// An offer the user will see after the answer.
    Offered { kind: OfferKind },
    /// `runCommand` with `background`: it runs on, and this is its number.
    ProcessStarted(crate::domain::background::ProcessInfo),
    /// `runCommand` still running at its timeout: not killed, it runs on as
    /// this background process.
    #[serde(rename_all = "camelCase")]
    CommandMoved {
        #[serde(flatten)]
        process: crate::domain::background::ProcessInfo,
        after_ms: u64,
    },
    ProcessOutput(crate::domain::background::ProcessOutput),
    ProcessStopped(crate::domain::background::ProcessInfo),
    TerminalScreen(crate::domain::terminal::TerminalScreen),
    #[serde(rename_all = "camelCase")]
    SearchResults {
        matches: Vec<crate::domain::code_search::CodeMatch>,
        meta: crate::domain::code_search::SearchMeta,
    },
    /// A skill's `SKILL.md` body, frontmatter stripped, and the paths of the
    /// files beside it that `skill` with a `path` can fetch.
    #[serde(rename_all = "camelCase")]
    Skill {
        name: String,
        instructions: String,
        files: Vec<String>,
        /// Where it was found, said as the model should weigh it: the
        /// repository's own, or the user's from a folder every project shares.
        #[serde(default)]
        from: String,
    },
    #[serde(rename_all = "camelCase")]
    SkillFile {
        name: String,
        path: String,
        content: String,
    },
    /// The plan was replaced. The text is in the call's own arguments.
    #[serde(rename_all = "camelCase")]
    PlanWritten { lines: u32 },
    /// A note was kept — or was there already, word for word. The text is in
    /// the call's own arguments.
    Remembered { already: bool },
    /// An MCP tool's answer, already text.
    Mcp { text: String },
    /// The helper turn's closing answer, which run it was — its number in
    /// the Agents tab — and what it cost.
    Explored {
        text: String,
        agent: u32,
        tokens: crate::domain::agents::AgentTokens,
    },
    /// A review run as a helper over a change: what it kept and its closing
    /// answer. The findings always come back in the conversation; what is
    /// done about them is the calling turn's or the user's next message.
    Reviewed {
        findings: Vec<crate::domain::review::Finding>,
        text: String,
        agent: u32,
        tokens: crate::domain::agents::AgentTokens,
        /// Folder, refs and file selection; empty in results saved before R-0.
        #[serde(default)]
        range: String,
    },
    /// What a Kubernetes tool read, already shaped for the model — a table,
    /// YAML, a log — and a few words for the row in the transcript.
    Kube { text: String, summary: String },
    /// `webSearch`: the pages found, in the service's order.
    WebResults { hits: Vec<crate::domain::web_search::WebHit> },
    /// `webFetch`: the page, whole or the part asked for.
    WebPage { page: crate::domain::web_search::WebPage },
    /// `toolSearch`: the MCP tools found, best first — declared from the
    /// next round on.
    ToolsFound { tools: Vec<FoundTool> },
    /// `board`: the whole board after the call.
    Board { cards: Vec<crate::domain::board::Card> },
    /// `mongo`: what was read, already shaped for the model, and a few words
    /// for the row in the transcript.
    Mongo { text: String, summary: String },
    /// `present`: the page shown, and the version its folder was kept as.
    Presented { path: String, title: String, version: u32 },
}
