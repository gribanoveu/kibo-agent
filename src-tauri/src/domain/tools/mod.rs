//! Tool identity, loop cost, and the approval gate.
//!
//! Ported from Alfa Atlas `domain/ai_access.rs`, which was 646 lines. Most of
//! it did not survive: `AiAccessMode` (`DocsOnly`/`FullRepo`) was specific to a
//! documentation product — here the access root is always the repository, and
//! it is enforced where paths are resolved, not by a mode enum. The per-project
//! tool allowlist went with it: its own fallback returned every variant that
//! existed, and there was no UI to narrow it, so it guarded nothing. It belongs
//! back only once something can actually configure it.
//!
//! What is left is the part that was load-bearing: which tools may not run
//! unattended, what a call costs the loop, and the wire names the model uses.
//!
//! Here: `ToolName`, what each tool is and costs. Beside it: `approval` (the
//! gate), `call` (a parsed call and its result), `error`, `args` with
//! `kube_args` and `todo` (what the model sends), `scope` (where a call runs
//! and its preview), `read_files` (what was read before a write).

mod approval;
mod args;
mod call;
mod error;
mod kube_args;
mod read_files;
mod scope;
mod todo;
#[cfg(test)]
mod tests;

pub use approval::*;
pub use args::*;
pub use call::*;
pub use error::*;
pub use kube_args::*;
pub use read_files::*;
pub use scope::*;
pub use todo::*;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::domain::command_risk::{self, CommandRisk};

/// A tool's identity, known from the wire name alone — before its arguments
/// are parsed, and without borrowing them. That is what lets it key the
/// "always allow" set, the call log and the loop budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ToolName {
    ReadFile,
    Grep,
    ListFiles,
    WriteFile,
    EditFile,
    /// A file, or a folder with `recursive`.
    Delete,
    Move,
    Todo,
    GitDiff,
    GitBlame,
    GitLog,
    RunCommand,
    GitStatus,
    SemanticSearch,
    Skill,
    WritePlan,
    /// A background process's output since the last read.
    ReadOutput,
    StopProcess,
    /// What the user's own terminal shows.
    ReadTerminal,
    /// A problem found in a review, placed on the change — review workers only.
    ReportFinding,
    /// A next step put to the user as a button after the answer; runs nothing.
    Offer,
    /// A research task handed to a read-only helper turn with a context of
    /// its own; only its answer comes back.
    Explore,
    /// A change read by a helper turn in Review mode; its findings come back
    /// in the result (`docs/49-review-as-a-tool.md`). Reads and reports —
    /// it writes nothing itself.
    Review,
    /// A note for later conversations in this folder, checked by the model
    /// before it is kept.
    Remember,
    /// The Kubernetes role's reads of the cluster its chat is pinned to
    /// (`docs/21-kubernetes-mode.md`, K-3).
    KubeList,
    KubeGet,
    KubeEvents,
    KubeLogs,
    KubeTop,
    KubeFieldHistory,
    /// A workload's state, pods, events and the telling log in one call (K-4).
    KubeDiagnose,
    KubeWaitRollout,
    KubeProbe,
    KubeRunbook,
    /// The first of the role's changes (K-5a): replicas, with a backup.
    KubeScale,
    /// Puts a recorded change back from its backup (K-5b).
    KubeUndo,
    /// The simple changes (K-5c): a CronJob or Job stopped and resumed, a
    /// workload's pods replaced, a Deployment put back a revision.
    KubeSuspend,
    KubeRolloutRestart,
    KubeRolloutUndo,
    /// A manifest applied, an object deleted (K-5d).
    KubeApply,
    KubeDelete,
    /// A chat's search of the web (`docs/24-web-search.md`).
    WebSearch,
    /// A chat's read of one page the search found or the user named.
    WebFetch,
    /// The same two through DeepSeek's own search and page reader, with the
    /// DeepSeek provider's key (`Feature::DeepseekSearch`).
    DeepseekSearch,
    DeepseekFetch,
    /// The Assistant's board: cards kept for later, which the user sees on
    /// the Board page (`domain::board`).
    Board,
    /// The MongoDB connections the user opened to the agent, read: their
    /// collections, documents, counts, aggregations, indexes (`docs/44-mongodb.md`, M-8).
    Mongo,
    /// The Prototype role's page shown to the user, its folder kept as a
    /// version (`docs/31-prototype-role.md`, P-4).
    Present,
    /// Finds the MCP tools left out of the request (`Exposure::Deferred`)
    /// and declares them from the next round on.
    ToolSearch,
    /// Every tool of every connected MCP server. One variant for all of them:
    /// their names are the servers' and arrive at run time, so the identity
    /// that matters beyond this — for "always allow", for the weight — is
    /// the call's full name, `mcp__<server>__<tool>`.
    #[serde(rename = "mcp__")]
    Mcp,
}

/// What every MCP tool's name starts with.
pub const MCP_PREFIX: &str = "mcp__";

impl ToolName {
    /// Every variant. Kept by hand, and guarded by `all_is_complete` below —
    /// the round-trip and weight tests are only worth what this list covers.
    pub const ALL: &'static [ToolName] = &[
        ToolName::ReadFile,
        ToolName::Grep,
        ToolName::ListFiles,
        ToolName::WriteFile,
        ToolName::EditFile,
        ToolName::Delete,
        ToolName::Move,
        ToolName::Todo,
        ToolName::GitDiff,
        ToolName::GitBlame,
        ToolName::GitLog,
        ToolName::GitStatus,
        ToolName::RunCommand,
        ToolName::SemanticSearch,
        ToolName::Skill,
        ToolName::WritePlan,
        ToolName::ReadOutput,
        ToolName::StopProcess,
        ToolName::ReadTerminal,
        ToolName::ReportFinding,
        ToolName::Offer,
        ToolName::Explore,
        ToolName::Review,
        ToolName::Remember,
        ToolName::KubeList,
        ToolName::KubeGet,
        ToolName::KubeEvents,
        ToolName::KubeLogs,
        ToolName::KubeTop,
        ToolName::KubeFieldHistory,
        ToolName::KubeDiagnose,
        ToolName::KubeWaitRollout,
        ToolName::KubeProbe,
        ToolName::KubeRunbook,
        ToolName::KubeScale,
        ToolName::KubeUndo,
        ToolName::KubeSuspend,
        ToolName::KubeRolloutRestart,
        ToolName::KubeRolloutUndo,
        ToolName::KubeApply,
        ToolName::KubeDelete,
        ToolName::WebSearch,
        ToolName::WebFetch,
        ToolName::DeepseekSearch,
        ToolName::DeepseekFetch,
        ToolName::Board,
        ToolName::Mongo,
        ToolName::Present,
        ToolName::ToolSearch,
        ToolName::Mcp,
    ];

    /// The name the model calls this tool by. Must match what `Serialize`
    /// produces — `serde_form_matches_wire_name` holds the two together.
    pub fn wire_name(self) -> &'static str {
        match self {
            ToolName::ReadFile => "readFile",
            ToolName::Grep => "grep",
            ToolName::ListFiles => "listFiles",
            ToolName::WriteFile => "writeFile",
            ToolName::EditFile => "editFile",
            ToolName::Delete => "delete",
            ToolName::Move => "move",
            ToolName::Todo => "todo",
            ToolName::GitDiff => "gitDiff",
            ToolName::GitBlame => "gitBlame",
            ToolName::GitLog => "gitLog",
            ToolName::RunCommand => "runCommand",
            ToolName::GitStatus => "gitStatus",
            ToolName::SemanticSearch => "semanticSearch",
            ToolName::Skill => "skill",
            ToolName::WritePlan => "writePlan",
            ToolName::ReadOutput => "readOutput",
            ToolName::StopProcess => "stopProcess",
            ToolName::ReadTerminal => "readTerminal",
            ToolName::ReportFinding => "reportFinding",
            ToolName::Offer => "offer",
            ToolName::Explore => "explore",
            ToolName::Review => "review",
            ToolName::Remember => "remember",
            ToolName::KubeList => "kubeList",
            ToolName::KubeGet => "kubeGet",
            ToolName::KubeEvents => "kubeEvents",
            ToolName::KubeLogs => "kubeLogs",
            ToolName::KubeTop => "kubeTop",
            ToolName::KubeFieldHistory => "kubeFieldHistory",
            ToolName::KubeDiagnose => "kubeDiagnose",
            ToolName::KubeWaitRollout => "kubeWaitRollout",
            ToolName::KubeProbe => "kubeProbe",
            ToolName::KubeRunbook => "kubeRunbook",
            ToolName::KubeScale => "kubeScale",
            ToolName::KubeUndo => "kubeUndo",
            ToolName::KubeSuspend => "kubeSuspend",
            ToolName::KubeRolloutRestart => "kubeRolloutRestart",
            ToolName::KubeRolloutUndo => "kubeRolloutUndo",
            ToolName::KubeApply => "kubeApply",
            ToolName::KubeDelete => "kubeDelete",
            ToolName::WebSearch => "webSearch",
            ToolName::WebFetch => "webFetch",
            ToolName::DeepseekSearch => "deepseekSearch",
            ToolName::DeepseekFetch => "deepseekFetch",
            ToolName::Board => "board",
            ToolName::Mongo => "mongo",
            ToolName::Present => "present",
            ToolName::ToolSearch => crate::domain::mcp::TOOL_SEARCH,
            // The prefix, not a name: no tool is called just this.
            ToolName::Mcp => MCP_PREFIX,
        }
    }

    /// Classifies a call before its arguments are known to parse — the loop
    /// has to decide "risky or not" on a call whose JSON may still be broken.
    pub fn from_wire_name(name: &str) -> Option<ToolName> {
        if name.starts_with(MCP_PREFIX) {
            return Some(ToolName::Mcp);
        }
        ToolName::ALL.iter().copied().find(|t| t.wire_name() == name)
    }

    /// Whether this tool changes the working tree. The default verdict fed to
    /// [`ApprovalPolicy::requires_approval`].
    ///
    /// True by identity alone, which holds for every tool here: a write is a
    /// write whatever its arguments. It stops holding at `runCommand`, where
    /// `ls` and `rm -rf /` are the same tool — which is why the gate takes the
    /// verdict as an argument instead of calling this itself.
    /// Declared in every request where `toolSearch` is offered, rather than
    /// found with it first (`docs/43-single-loop.md`, Ш9). The wildcard is the
    /// point: a tool not listed here — a new one too — costs a request nothing
    /// until the model asks for it. Listed are the tools of first need, those
    /// the prompt sends the model to by name (`remember`),
    /// `toolSearch` itself, and the role and MCP placeholders, which never wait.
    pub fn loads_up_front(self) -> bool {
        matches!(
            self,
            ToolName::ReadFile
                | ToolName::Grep
                | ToolName::ListFiles
                | ToolName::WriteFile
                | ToolName::EditFile
                | ToolName::Delete
                | ToolName::Todo
                | ToolName::GitDiff
                | ToolName::GitLog
                | ToolName::RunCommand
                | ToolName::GitStatus
                | ToolName::SemanticSearch
                | ToolName::Skill
                | ToolName::WritePlan
                | ToolName::ReadOutput
                | ToolName::StopProcess
                | ToolName::ReportFinding
                | ToolName::Offer
                | ToolName::Explore
                // `/review` names the tool in the command's message.
                | ToolName::Review
                | ToolName::Remember
                | ToolName::KubeList
                | ToolName::KubeGet
                | ToolName::KubeEvents
                | ToolName::KubeLogs
                | ToolName::KubeTop
                | ToolName::KubeFieldHistory
                | ToolName::KubeDiagnose
                | ToolName::KubeWaitRollout
                | ToolName::KubeProbe
                | ToolName::KubeRunbook
                | ToolName::KubeScale
                | ToolName::KubeUndo
                | ToolName::KubeSuspend
                | ToolName::KubeRolloutRestart
                | ToolName::KubeRolloutUndo
                | ToolName::KubeApply
                | ToolName::KubeDelete
                | ToolName::WebSearch
                | ToolName::WebFetch
                | ToolName::DeepseekSearch
                | ToolName::DeepseekFetch
                | ToolName::Present
                | ToolName::ToolSearch
                | ToolName::Mcp
        )
    }

    /// The tools that reach the web — offered together, or not at all.
    pub fn is_web(self) -> bool {
        matches!(self, ToolName::WebSearch | ToolName::WebFetch)
    }

    pub fn is_mutating(self) -> bool {
        // No wildcard: a tool left out of this would skip the approval gate,
        // so a new one has to be put on one side.
        match self {
            ToolName::WriteFile
            | ToolName::EditFile
            | ToolName::Delete
            | ToolName::Move
            // Every command is treated as mutating, because a command
            // line is not a tool name: `ls` and `rm -rf /` are the same
            // call with different arguments. Narrowing that back down is a
            // question about the command line, and it is asked in
            // `domain::command_risk`, not here.
            | ToolName::RunCommand
            // Changes the user's cluster.
            | ToolName::KubeScale
            | ToolName::KubeUndo
            | ToolName::KubeSuspend
            | ToolName::KubeRolloutRestart
            | ToolName::KubeRolloutUndo
            | ToolName::KubeApply
            | ToolName::KubeDelete
            // Nothing is known about what a foreign tool does, and its
            // server's own hints are untrusted by the specification: it
            // goes through the approval gate, in Analyst too, the one place
            // it is offered beside a promise to leave the code alone.
            | ToolName::Mcp => true,
            ToolName::ReadFile
            | ToolName::Grep
            | ToolName::ListFiles
            | ToolName::Todo
            | ToolName::GitDiff
            | ToolName::GitBlame
            | ToolName::GitLog
            | ToolName::GitStatus
            | ToolName::SemanticSearch
            | ToolName::Skill
            | ToolName::WritePlan
            | ToolName::ReadOutput
            | ToolName::StopProcess
            | ToolName::ReadTerminal
            | ToolName::ReportFinding
            | ToolName::Offer
            | ToolName::Explore
            // Reads a change and reports; the helper's own commands are
            // accounted by the rewind window, not a write here.
            | ToolName::Review
            | ToolName::Remember
            | ToolName::KubeList
            | ToolName::KubeGet
            | ToolName::KubeEvents
            | ToolName::KubeLogs
            | ToolName::KubeTop
            | ToolName::KubeFieldHistory
            | ToolName::KubeDiagnose
            | ToolName::KubeWaitRollout
            | ToolName::KubeProbe
            | ToolName::KubeRunbook
            | ToolName::WebSearch
            | ToolName::WebFetch
            | ToolName::DeepseekSearch
            | ToolName::DeepseekFetch
            | ToolName::Board
            // Reads; a pipeline that would write is refused before the server.
            | ToolName::Mongo
            // A copy of the chat's folder beside it; nothing there changes.
            | ToolName::Present
            | ToolName::ToolSearch => false,
        }
    }

    /// Whether calls of this tool asked in a row may run at once. Only reads
    /// that leave nothing behind but their answer — and the files they read,
    /// which the round gathers after them: what one returns cannot change what
    /// the one beside it would have. No wildcard: a new tool has to be put on
    /// one side, and left out it would run beside a write.
    pub fn runs_alongside(self) -> bool {
        match self {
            ToolName::ReadFile
            | ToolName::Grep
            | ToolName::ListFiles
            | ToolName::GitDiff
            | ToolName::GitBlame
            | ToolName::GitLog
            | ToolName::GitStatus
            | ToolName::SemanticSearch
            | ToolName::Explore
            | ToolName::WebSearch
            | ToolName::WebFetch
            | ToolName::DeepseekSearch
            | ToolName::DeepseekFetch
            // A server asked, nothing left open after it.
            | ToolName::Mongo => true,
            ToolName::WriteFile
            | ToolName::EditFile
            | ToolName::Delete
            | ToolName::Move
            | ToolName::RunCommand
            // The turn's own state: its list, its plan, the review's findings
            // in the order they were reported.
            | ToolName::Todo
            | ToolName::WritePlan
            | ToolName::ReportFinding
            | ToolName::Offer
            // One reviewer at a time: two in one reply are two helpers and
            // two proving builds at once, which gains nothing.
            | ToolName::Review
            // Change what the model sees or is told next.
            | ToolName::Skill
            | ToolName::ToolSearch
            // A process or a terminal, read up to where the last read left it.
            | ToolName::ReadOutput
            | ToolName::StopProcess
            | ToolName::ReadTerminal
            // Kept, after the model checks them.
            | ToolName::Remember
            | ToolName::Board
            | ToolName::Present
            // The cluster's reads wait on it and have a bench of their own to
            // show they gain; until then, one at a time.
            | ToolName::KubeList
            | ToolName::KubeGet
            | ToolName::KubeEvents
            | ToolName::KubeLogs
            | ToolName::KubeTop
            | ToolName::KubeFieldHistory
            | ToolName::KubeDiagnose
            | ToolName::KubeWaitRollout
            | ToolName::KubeProbe
            | ToolName::KubeRunbook
            | ToolName::KubeScale
            | ToolName::KubeUndo
            | ToolName::KubeSuspend
            | ToolName::KubeRolloutRestart
            | ToolName::KubeRolloutUndo
            | ToolName::KubeApply
            | ToolName::KubeDelete
            | ToolName::Mcp => false,
        }
    }

    /// Whether a rewind accounts for what a call did: its file changes are
    /// recorded on its block, or it made none a rewind should undo. A call of
    /// any other tool may have changed things that stay — the rewind dialog
    /// counts it. No wildcard: a new tool has to be put on one side.
    ///
    /// The window's copy is `ACCOUNTED` in `src/lib/rewind.ts`, kept equal by
    /// `architecture::every_tools_rewind_flag_matches_the_window`.
    pub fn rewind_sees(self) -> bool {
        match self {
            // Recorded on the block.
            ToolName::WriteFile | ToolName::EditFile | ToolName::Delete | ToolName::Move => true,
            // Reads, and state outside the folder that a rewind leaves alone
            // on purpose: the checklist, the plan, the board, a process.
            ToolName::ReadFile
            | ToolName::Grep
            | ToolName::ListFiles
            | ToolName::Todo
            | ToolName::GitStatus
            | ToolName::GitDiff
            | ToolName::GitBlame
            | ToolName::GitLog
            | ToolName::SemanticSearch
            | ToolName::WebSearch
            | ToolName::WebFetch
            | ToolName::DeepseekSearch
            | ToolName::DeepseekFetch
            | ToolName::ToolSearch
            | ToolName::Board
            | ToolName::Mongo
            // A version kept beside the folder: history, which a rewind leaves.
            | ToolName::Present
            | ToolName::Skill
            | ToolName::WritePlan
            | ToolName::ReadOutput
            | ToolName::StopProcess
            | ToolName::ReadTerminal
            // Reads only, in a turn of its own.
            | ToolName::Explore => true,
            // Anything at all; a command line is not a tool name.
            ToolName::RunCommand | ToolName::Mcp => false,
            // The folder's memory file, which stays.
            ToolName::Remember => false,
            // A review worker's, never in a chat that can be rewound.
            ToolName::ReportFinding => false,
            // Its helper may run commands to prove a finding (a build writes
            // `target/`) — the rewind window counts the call, not skips it.
            ToolName::Review => false,
            // A button under the answer; nothing outside the chat.
            ToolName::Offer => true,
            // The cluster, in a chat — no folder to rewind.
            ToolName::KubeList
            | ToolName::KubeGet
            | ToolName::KubeEvents
            | ToolName::KubeLogs
            | ToolName::KubeTop
            | ToolName::KubeFieldHistory
            | ToolName::KubeDiagnose
            | ToolName::KubeWaitRollout
            | ToolName::KubeProbe
            | ToolName::KubeRunbook
            | ToolName::KubeScale
            | ToolName::KubeUndo
            | ToolName::KubeSuspend
            | ToolName::KubeRolloutRestart
            | ToolName::KubeRolloutUndo
            | ToolName::KubeApply
            | ToolName::KubeDelete => false,
        }
    }

    /// What one call charges against the turn's tool budget — an estimate of
    /// cost, not of risk. A round costs the sum of its calls' weights, so a
    /// cheap run of reads lives long and an expensive one is cut off early.
    pub fn loop_weight(self) -> u32 {
        match self {
            // Bare filesystem or in-memory work.
            ToolName::ReadFile
            | ToolName::ListFiles
            | ToolName::Delete
            | ToolName::Todo
            // A file or two out of the app directory.
            | ToolName::Skill
            // Chat state, like the checklist.
            | ToolName::WritePlan
            // A buffer in memory; a kill.
            | ToolName::ReadOutput
            | ToolName::StopProcess
            // A screen in memory.
            | ToolName::ReadTerminal
            // Checked against a diff in memory.
            | ToolName::ReportFinding
            // One walk of the working tree, like gitStatus.
            | ToolName::Offer
            // A scan of the connected servers' tool lists, in memory.
            | ToolName::ToolSearch
            // A small file in the app directory, read and written.
            | ToolName::Board
            // A copy of a folder of a few pages.
            | ToolName::Present => 1,
            // A gitignore-aware walk plus a regex over many files.
            ToolName::Grep => 3,
            // Local git2 I/O plus diff/blame compaction.
            ToolName::GitDiff | ToolName::GitBlame => 2,
            // A walk of the history with a tree diff per commit.
            ToolName::GitLog => 2,
            // One walk of the working tree, no per-file blob reads.
            ToolName::GitStatus => 1,
            // Indexed lookups and a scan of the vectors; the first search
            // after the model was unloaded also loads it (~0.35 s).
            ToolName::SemanticSearch => 2,
            // Read, diff against the new content, write.
            ToolName::WriteFile | ToolName::EditFile => 2,
            // A rename. Atlas charged 2 because its `move` also rewrote
            // `include::`/`xref:` references in other documents; that part is
            // not ported, so the cost went with it.
            ToolName::Move => 1,
            // The only tool whose cost is unbounded: a build, a test suite,
            // a download. Weighted so a turn cannot spend itself entirely on
            // re-running things.
            ToolName::RunCommand => 5,
            // A whole turn of its own. That turn has its own ceiling, so this
            // only keeps a turn from delegating without end.
            ToolName::Explore => 5,
            // A whole review helper turn, the same shape and ceiling.
            ToolName::Review => 5,
            // A short request to the model, and a line appended.
            ToolName::Remember => 2,
            // A request or two to the cluster; logs may be ten pods' worth.
            ToolName::KubeList
            | ToolName::KubeGet
            | ToolName::KubeEvents
            | ToolName::KubeTop
            | ToolName::KubeFieldHistory => 2,
            ToolName::KubeLogs => 3,
            // Five or six reads of the cluster — the rounds it saves.
            ToolName::KubeDiagnose => 4,
            // One watch, however long: waiting is the round it saves.
            ToolName::KubeWaitRollout => 2,
            // A read or two, and a command or two in the pod.
            ToolName::KubeProbe => 2,
            // A text already in memory.
            ToolName::KubeRunbook => 1,
            // A read, a dry run, a backup, the change.
            ToolName::KubeScale
            | ToolName::KubeUndo
            | ToolName::KubeSuspend
            | ToolName::KubeRolloutRestart
            | ToolName::KubeRolloutUndo
            | ToolName::KubeApply
            | ToolName::KubeDelete => 3,
            // One request to the search service, a second or two — and with
            // SearXNG, the pages it found read side by side.
            ToolName::WebSearch => 2,
            // A page, and its redirects.
            ToolName::WebFetch => 2,
            // One request each; DeepSeek reads the page on its side.
            ToolName::DeepseekSearch | ToolName::DeepseekFetch => 2,
            // A request or two to a server; a search by name, one per database.
            ToolName::Mongo => 2,
            // The default; a server's own `weight` replaces it per call —
            // see `domain::mcp::McpTools::weight`.
            ToolName::Mcp => crate::domain::mcp::DEFAULT_WEIGHT,
        }
    }
}

