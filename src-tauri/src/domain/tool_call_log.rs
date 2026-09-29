//! The tool-call log: one row per settled call, with every piece of file
//! content taken out before it is written (CA-12.1).
//!
//! Ported from Alfa Atlas `domain/tool_call_log.rs` plus the redaction half
//! of `infra/tool_call_log.rs`, which is pure and so belongs here.
//!
//! **Redaction is by field name, over the whole value, not per tool.**
//! Upstream listed, tool by tool, which fields carry text; a tool added
//! without a line in that list logged its content in full, and nothing
//! failed. Here any field called one of [`CONTENT_FIELDS`] is replaced
//! wherever it sits, so a new tool that names its text `content` is covered
//! without anyone remembering — and `every_tool_is_redacted`, an exhaustive
//! match over `ToolName`, makes a tool that names it something else fail to
//! compile its test until someone decides.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::tools::{ToolCall, ToolError, ToolResult};

/// Fields that carry file text, command output or instructions — anywhere
/// in a call's arguments or result. Paths, line numbers, counts, names and
/// exit codes stay: they are what the log is for.
pub const CONTENT_FIELDS: &[&str] = &[
    // readFile, writeFile, a skill's file
    "content",
    // grep and semanticSearch matches, and grep's context lines
    "text",
    "before",
    "after",
    // editFile
    "old",
    "new",
    // every diff: writes, deletes, gitDiff
    "unifiedDiff",
    // runCommand, and a background process's output
    "stdout",
    "stderr",
    "output",
    // skill
    "instructions",
];

pub const REDACTED: &str = "<redacted>";

/// How a call ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CallStatus {
    Ok,
    Error,
    /// The user said no at the approval card; the call never ran.
    Denied,
}

impl CallStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            CallStatus::Ok => "ok",
            CallStatus::Error => "error",
            CallStatus::Denied => "denied",
        }
    }

    pub fn parse(text: &str) -> Option<CallStatus> {
        [CallStatus::Ok, CallStatus::Error, CallStatus::Denied].into_iter().find(|s| s.as_str() == text)
    }
}

/// One settled call, already redacted, ready to store.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallLogEntry {
    /// Unix milliseconds.
    pub ts_ms: i64,
    pub repo_root: String,
    pub round: u32,
    pub provider_id: String,
    pub model: String,
    /// The wire name the model sent, known or not.
    pub tool: String,
    /// `null` when the arguments did not parse: raw arguments are the one
    /// shape redaction cannot see into, and a broken `writeFile` is a file.
    pub args: Value,
    pub status: CallStatus,
    pub error: Option<String>,
    pub result: Option<Value>,
    pub duration_ms: i64,
}

/// A stored entry, with its id.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallLogRow {
    pub id: i64,
    #[serde(flatten)]
    pub entry: ToolCallLogEntry,
}

/// Absent fields filter nothing.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallLogFilter {
    pub repo_root: Option<String>,
    pub tool: Option<String>,
    pub status: Option<CallStatus>,
    /// Case-insensitive substring of the tool, the error, or the (redacted)
    /// arguments — which is how a path is found.
    pub search: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallLogPage {
    pub rows: Vec<ToolCallLogRow>,
    /// Matching rows ignoring `limit`/`offset`.
    pub total: i64,
}

/// Every [`CONTENT_FIELDS`] field anywhere in `value`, replaced.
pub fn redact(mut value: Value) -> Value {
    redact_in_place(&mut value);
    value
}

fn redact_in_place(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, field) in map.iter_mut() {
                if CONTENT_FIELDS.contains(&key.as_str()) {
                    *field = Value::String(REDACTED.to_string());
                } else {
                    redact_in_place(field);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(redact_in_place),
        _ => {}
    }
}

pub fn redact_args(call: &ToolCall) -> Value {
    // A foreign tool's fields have names nobody here chose — `query`,
    // `body`, `sql` — so redacting by field name would let them through.
    // What is kept is their shape: which fields, and how big.
    if let ToolCall::Mcp(args) = call {
        return serde_json::json!({ "tool": "mcp", "args": { "name": args.name, "arguments": shape(&args.arguments) } });
    }
    redact(serde_json::to_value(call).unwrap_or(Value::Null))
}

/// Each top-level field of an MCP call's arguments, described instead of kept.
fn shape(arguments: &Value) -> Value {
    let describe = |value: &Value| -> Value {
        Value::String(match value {
            Value::String(s) => format!("<string, {} chars>", s.chars().count()),
            Value::Array(items) => format!("<array, {} items>", items.len()),
            Value::Object(map) => format!("<object, {} keys>", map.len()),
            Value::Number(_) => "<number>".into(),
            Value::Bool(_) => "<bool>".into(),
            Value::Null => "<null>".into(),
        })
    };
    match arguments {
        Value::Object(map) => Value::Object(map.iter().map(|(k, v)| (k.clone(), describe(v))).collect()),
        other => describe(other),
    }
}

pub fn redact_result(result: &ToolResult) -> Value {
    redact(serde_json::to_value(result).unwrap_or(Value::Null))
}

/// The error as the log keeps it. Two carry the text of an edit that did
/// not apply — the very content the rest of the log leaves out — and an
/// argument error quotes the value serde choked on (`invalid type: string
/// "…"`), which for an `edits` sent as one string is the whole edit.
pub fn redact_error(error: &ToolError) -> String {
    match error {
        ToolError::EditTextNotFound { .. } => "edit text not found".to_string(),
        ToolError::EditTextAmbiguous(_, count) => format!("edit text is not unique — matched {count} times"),
        ToolError::EditInsideWord(..) => "edit text starts or ends inside a word".to_string(),
        ToolError::InEdit { index, of, reason } => format!("edit {index} of {of}: {}", redact_error(reason)),
        // The tool's own words, which can be anything it read.
        ToolError::McpToolFailed(_) => "the MCP tool reported an error".to_string(),
        // Carries the server's last stderr lines.
        ToolError::McpUnavailable(_) => "the MCP server was not available".to_string(),
        // A script's stderr: whatever it chose to quote from the call.
        ToolError::BlockedByHook(_) => "blocked by a hook".to_string(),
        ToolError::InvalidArguments { tool, reason } => {
            let before_quote = reason.split('"').next().unwrap_or_default().trim_end();
            format!("invalid arguments for {tool}: {before_quote}")
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::command_exec::{CommandOutput, CommandRequest};
    use crate::domain::tools::*;

    const LEAK: &str = "LEAK-marker";

    fn diff() -> FileDiffStats {
        FileDiffStats { lines_added: 1, lines_removed: 2, unified_diff: LEAK.into(), truncated: false }
    }

    /// A call and a result for `tool`, with the marker in every field that
    /// holds file text. Exhaustive on purpose: a new tool does not compile
    /// here until someone has decided what in it is content.
    fn sample(tool: ToolName) -> (Option<ToolCall>, ToolResult) {
        let path = || "src/a.rs".to_string();
        let process = || crate::domain::background::ProcessInfo {
            id: 1,
            command: "npm run dev".into(),
            cwd: ".".into(),
            state: crate::domain::background::ProcessState::Running,
        };
        match tool {
            ToolName::ReadFile => (None, ToolResult::File { content: LEAK.into(), start_line: 1, end_line: 1, total_lines: 1, clamped: false, truncated: false }),
            ToolName::Grep => (
                None,
                ToolResult::GrepResults {
                    matches: vec![GrepMatch {
                        path: path(),
                        line: 3,
                        text: LEAK.into(),
                        before: vec![LEAK.into()],
                        after: vec![LEAK.into()],
                    }],
                    total: 1,
                    total_files: 1,
                    total_is_floor: false,
                    skipped: vec![],
                    truncated: false,
                },
            ),
            ToolName::ListFiles => (None, ToolResult::FileList { entries: vec![], truncated: false, stopped_at: None }),
            ToolName::WriteFile => (
                Some(ToolCall::WriteFile(WriteFileArgs { path: path(), content: LEAK.into() })),
                ToolResult::FileWritten { path: path(), diff: diff() },
            ),
            ToolName::EditFile => (
                Some(ToolCall::EditFile(EditFileArgs {
                    path: path(),
                    edits: vec![FileEdit { old: LEAK.into(), new: LEAK.into() }],
                })),
                ToolResult::FileEdited { path: path(), diff: diff() },
            ),
            ToolName::DeleteFile => (None, ToolResult::FileDeleted { path: path(), diff: diff() }),
            ToolName::CreateDirectory => (None, ToolResult::DirectoryCreated { path: path() }),
            ToolName::DeleteDirectory => (None, ToolResult::DirectoryDeleted { path: path(), files: 0, listed: vec![] }),
            ToolName::Move => (None, ToolResult::Moved { from: path(), to: path(), files: None }),
            ToolName::Todo => (None, ToolResult::Todo { tasks: vec![] }),
            ToolName::GitStatus => (
                None,
                ToolResult::GitStatus { branch: None, upstream: None, staged: vec![], unstaged: vec![], conflicted: vec![], truncated: false },
            ),
            ToolName::GitDiff => (
                None,
                ToolResult::GitDiff { path: path(), label: "index → working tree".into(), diff: diff(), is_binary: false },
            ),
            ToolName::GitBlame => (None, ToolResult::GitBlame { path: path(), hunks: vec![], truncated: false }),
            ToolName::GitLog => (None, ToolResult::GitLog { path: path(), commits: vec![], truncated: false }),
            ToolName::RunCommand => (
                Some(ToolCall::RunCommand(CommandRequest { command: "cargo test".into(), cwd: None, timeout_seconds: None, background: None })),
                ToolResult::CommandRan(CommandOutput {
                    stdout: LEAK.into(),
                    stderr: LEAK.into(),
                    ..CommandOutput::default()
                }),
            ),
            ToolName::SemanticSearch => (
                None,
                ToolResult::SearchResults {
                    matches: vec![crate::domain::code_search::CodeMatch {
                        path: path(),
                        start_line: 1,
                        end_line: 2,
                        name: Some("sync".into()),
                        text: LEAK.into(),
                        source: crate::domain::code_search::MatchSource::Lexical,
                    }],
                    meta: crate::domain::code_search::SearchMeta { tiers_used: vec![], weak: false, hint: None },
                },
            ),
            ToolName::WritePlan => (
                Some(ToolCall::WritePlan(WritePlanArgs { content: LEAK.into() })),
                ToolResult::PlanWritten { lines: 3 },
            ),
            ToolName::ReportFinding => (
                None,
                ToolResult::FindingNoted { path: path(), start_line: 3, end_line: 4 },
            ),
            // The task is the model's own brief and stays, as a command line
            // does; the answer quotes code, and goes as `text`.
            // What the cluster said — manifests, logs — goes as `text`; the
            // arguments are a kind, a name, a selector: what the log is for.
            ToolName::KubeList
            | ToolName::KubeGet
            | ToolName::KubeEvents
            | ToolName::KubeLogs
            | ToolName::KubeTop
            | ToolName::KubeFieldHistory => (None, ToolResult::Kube { text: LEAK.into(), summary: "3 pods".into() }),
            ToolName::Explore => (
                Some(ToolCall::Explore(ExploreArgs { task: "where is X".into() })),
                ToolResult::Explored { text: LEAK.into(), agent: 1, tokens: Default::default() },
            ),
            ToolName::Skill => (
                Some(ToolCall::Skill(SkillArgs { name: "release".into(), path: None })),
                ToolResult::Skill { name: "release".into(), instructions: LEAK.into(), files: vec![], from: String::new() },
            ),
            ToolName::ReadOutput => (
                Some(ToolCall::ReadOutput(crate::domain::tools::ProcessArgs { id: Some(1) })),
                ToolResult::ProcessOutput(crate::domain::background::ProcessOutput {
                    process: process(),
                    output: LEAK.into(),
                    missed: false,
                    truncated: false,
                }),
            ),
            ToolName::StopProcess => (
                Some(ToolCall::StopProcess(crate::domain::tools::ProcessArgs { id: Some(1) })),
                ToolResult::ProcessStopped(process()),
            ),
            ToolName::ReadTerminal => (
                Some(ToolCall::ReadTerminal(crate::domain::tools::ReadTerminalArgs { id: Some(1), lines: None })),
                ToolResult::TerminalScreen(crate::domain::terminal::TerminalScreen {
                    id: 1,
                    shell: "zsh".into(),
                    state: crate::domain::terminal::TerminalState::Running,
                    alternate: false,
                    output: LEAK.into(),
                    cut: 0,
                }),
            ),
            // The command line stays in the log, as runCommand's does.
            ToolName::RunInTerminal => (
                Some(ToolCall::RunInTerminal(crate::domain::tools::RunInTerminalArgs { command: "npm run dev".into(), id: None })),
                ToolResult::TerminalTyped {
                    terminal: crate::domain::terminal::TerminalInfo {
                        id: 1,
                        shell: "zsh".into(),
                        state: crate::domain::terminal::TerminalState::Running,
                    },
                    command: "npm run dev".into(),
                },
            ),
            ToolName::Mcp => (
                Some(ToolCall::Mcp(McpCallArgs {
                    name: "mcp__db__query".into(),
                    arguments: serde_json::json!({ "sql": LEAK, "params": [LEAK], "opts": { "k": LEAK } }),
                })),
                ToolResult::Mcp { text: LEAK.into() },
            ),
        }
    }

    #[test]
    fn every_tool_is_redacted() {
        for &tool in ToolName::ALL {
            let (call, result) = sample(tool);
            let logged = redact_result(&result).to_string();
            assert!(!logged.contains(LEAK), "{tool:?} result leaks: {logged}");
            if let Some(call) = call {
                let logged = redact_args(&call).to_string();
                assert!(!logged.contains(LEAK), "{tool:?} arguments leak: {logged}");
            }
        }
        // A skill's companion file shares the tool with a different shape.
        let file = ToolResult::SkillFile { name: "release".into(), path: "a.md".into(), content: LEAK.into() };
        assert!(!redact_result(&file).to_string().contains(LEAK));
    }

    /// What an MCP call's log line keeps: which tool, which fields, how big.
    #[test]
    fn an_mcp_call_is_logged_as_its_shape() {
        let (call, _) = sample(ToolName::Mcp);
        assert_eq!(
            redact_args(&call.unwrap()),
            serde_json::json!({ "tool": "mcp", "args": { "name": "mcp__db__query", "arguments": {
                "sql": "<string, 11 chars>", "params": "<array, 1 items>", "opts": "<object, 1 keys>"
            } } })
        );
        let odd = ToolCall::Mcp(McpCallArgs { name: "mcp__a__b".into(), arguments: serde_json::json!([1, true, null, 2.5]) });
        assert_eq!(redact_args(&odd)["args"]["arguments"], "<array, 4 items>");
        for error in [ToolError::McpToolFailed(LEAK.into()), ToolError::McpUnavailable(LEAK.into()), ToolError::BlockedByHook(LEAK.into())] {
            assert!(!redact_error(&error).contains(LEAK), "{error}");
        }
    }

    /// What the log is for survives: where, how much, and how it ended.
    #[test]
    fn paths_counts_and_exit_codes_stay() {
        let (_, result) = sample(ToolName::WriteFile);
        let logged = redact_result(&result);
        assert_eq!(logged["path"], "src/a.rs");
        assert_eq!(logged["diff"]["linesAdded"], 1);
        assert_eq!(logged["diff"]["unifiedDiff"], REDACTED);

        let ran = redact_result(&ToolResult::CommandRan(CommandOutput { exit_code: Some(2), ..CommandOutput::default() }));
        assert_eq!(ran["exitCode"], 2);
        let (call, _) = sample(ToolName::RunCommand);
        assert_eq!(redact_args(&call.unwrap())["args"]["command"], "cargo test");
    }

    #[test]
    fn an_edit_that_did_not_apply_is_logged_without_its_text() {
        let missing = ToolError::EditTextNotFound { text: LEAK.into(), nearest: Some((3, LEAK.into())) };
        assert_eq!(redact_error(&missing), "edit text not found");
        assert!(!redact_error(&ToolError::EditTextAmbiguous(LEAK.into(), 3)).contains(LEAK));
        assert!(!redact_error(&ToolError::EditInsideWord(LEAK.into(), LEAK.into())).contains(LEAK));
        let second = ToolError::InEdit { index: 2, of: 3, reason: Box::new(missing) };
        assert_eq!(redact_error(&second), "edit 2 of 3: edit text not found");
        assert_eq!(redact_error(&ToolError::NotFound("src/a.rs".into())), "not found: src/a.rs");
    }

    #[test]
    fn an_argument_error_keeps_where_and_drops_the_value_it_quotes() {
        let reason = format!("edits: invalid type: string \"{LEAK}\", expected a sequence");
        let logged = redact_error(&ToolError::InvalidArguments { tool: "editFile".into(), reason });
        assert_eq!(logged, "invalid arguments for editFile: edits: invalid type: string");
    }

    #[test]
    fn a_status_round_trips_through_its_stored_form() {
        for status in [CallStatus::Ok, CallStatus::Error, CallStatus::Denied] {
            assert_eq!(CallStatus::parse(status.as_str()), Some(status));
            assert_eq!(serde_json::to_value(status).unwrap(), status.as_str());
        }
    }
}
