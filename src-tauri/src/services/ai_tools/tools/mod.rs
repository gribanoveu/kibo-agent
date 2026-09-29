//! The tool boundary: one dispatcher, one place a call becomes an effect.
//!
//! Every call routes through [`execute_tool`], which is why permission
//! checking, logging and redaction each have exactly one site rather than one
//! per tool. Adding a tool is a variant in `ToolCall`, a variant in
//! `ToolResult`, a module here, an arm below, and a row in [`DEFINITIONS`].
//!
//! A tool's schema lives in its own module, beside the code that runs it. The
//! pair being one file is the point: a field added to an args struct without a
//! matching schema entry is a bug the model can never report, because it is
//! never told the field exists.
//!
//! `reads` and `todos` are the caller's, not this module's. It lives
//! here rather than inside the executor for the same reason the turn's todo
//! list does: the executor stays stateless, and the state survives an approval
//! pause. Alfa Atlas also threads dependencies through here; those arrive with
//! the tool that needs them.

use crate::domain::llm::LlmToolDefinition;
use crate::domain::tools::{
    ReadFiles, Task, ToolCall, ToolDeps, ToolError, ToolName, ToolResult, ToolScope,
};

pub mod create_directory;
pub mod delete_directory;
pub mod delete_file;
pub mod edit_file;
pub mod explore;
pub mod git;
pub mod grep;
pub mod cluster;
pub mod list_files;
pub mod move_path;
pub mod report_finding;
pub mod todo;
pub mod write_file;
pub mod read_file;
pub mod run_command;
pub mod semantic_search;
pub mod skill;
pub mod write_plan;
pub mod mcp;
pub mod process;
pub mod terminal;

/// One row: a tool and the function that builds its schema.
type ToolDefinitionRow = (ToolName, fn() -> LlmToolDefinition);

/// Every tool the model is told about, in the order it is shown them.
///
/// Paired with [`ToolName`] rather than listed as bare schemas so the coverage
/// test can be exact: a tool that exists but is never advertised is one the
/// model can only reach by guessing its name.
const DEFINITIONS: &[ToolDefinitionRow] = &[
    (ToolName::SemanticSearch, semantic_search::definition),
    (ToolName::ListFiles, list_files::definition),
    (ToolName::ReadFile, read_file::definition),
    (ToolName::Grep, grep::definition),
    (ToolName::GitStatus, git::status_definition),
    (ToolName::GitDiff, git::diff_definition),
    (ToolName::GitBlame, git::blame_definition),
    (ToolName::GitLog, git::log_definition),
    (ToolName::WriteFile, write_file::definition),
    (ToolName::EditFile, edit_file::definition),
    (ToolName::CreateDirectory, create_directory::definition),
    (ToolName::DeleteFile, delete_file::definition),
    (ToolName::DeleteDirectory, delete_directory::definition),
    (ToolName::Move, move_path::definition),
    (ToolName::WritePlan, write_plan::definition),
    (ToolName::ReportFinding, report_finding::definition),
    (ToolName::Explore, explore::definition),
    (ToolName::Todo, todo::definition),
    (ToolName::RunCommand, run_command::definition),
    (ToolName::Skill, skill::definition),
    (ToolName::ReadOutput, process::read_definition),
    (ToolName::StopProcess, process::stop_definition),
    (ToolName::ReadTerminal, terminal::read_definition),
    (ToolName::RunInTerminal, terminal::run_definition),
    (ToolName::KubeList, cluster::list_definition),
    (ToolName::KubeGet, cluster::get_definition),
    (ToolName::KubeEvents, cluster::events_definition),
    (ToolName::KubeLogs, cluster::logs_definition),
    (ToolName::KubeTop, cluster::top_definition),
    (ToolName::KubeFieldHistory, cluster::field_history_definition),
];

/// What the model is offered for a turn.
///
/// No filtering argument: this build has one axis of control, the approval
/// gate, and it acts on a call rather than on what may be described. Alfa
/// Atlas filters here by a project allowlist and a conversation mode; the
/// first is not ported (`docs/07-upstream-findings.md`, B-2), the second
/// arrives with conversation modes at stage 4 — and it will be a filter on
/// this list, not a second list.
pub fn tool_definitions() -> Vec<LlmToolDefinition> {
    DEFINITIONS.iter().map(|(_, build)| build()).collect()
}

pub fn execute_tool(
    scope: &ToolScope,
    call: &ToolCall,
    reads: &mut ReadFiles,
    todos: &mut Vec<Task>,
    deps: &ToolDeps,
) -> Result<ToolResult, ToolError> {
    dispatch(Some(scope), call, reads, todos, deps)
}

/// The one `match` every tool is a branch of. `scope` is `None` in Chat mode,
/// which has no folder: a tool that needs one says so rather than being handed
/// a folder that does not exist — the chat's preflight has already refused it.
pub fn dispatch(
    scope: Option<&ToolScope>,
    call: &ToolCall,
    reads: &mut ReadFiles,
    todos: &mut Vec<Task>,
    deps: &ToolDeps,
) -> Result<ToolResult, ToolError> {
    let folder = || scope.ok_or_else(|| ToolError::NoFolder(call.name().wire_name().to_string()));
    match call {
        ToolCall::ReadFile(args) => read_file::read_file(folder()?, args, reads),
        ToolCall::Grep(args) => grep::grep(folder()?, args),
        ToolCall::ListFiles(args) => list_files::list_files(folder()?, args),
        ToolCall::WriteFile(args) => write_file::write_file(folder()?, args, reads),
        ToolCall::EditFile(args) => edit_file::edit_file(folder()?, args, reads),
        ToolCall::CreateDirectory(args) => create_directory::create_directory(folder()?, args),
        ToolCall::DeleteFile(args) => delete_file::delete_file(folder()?, args, reads),
        ToolCall::DeleteDirectory(args) => delete_directory::delete_directory(folder()?, args),
        ToolCall::Move(args) => move_path::move_path(folder()?, args, reads),
        ToolCall::Todo(args) => todo::todo(todos, args),
        ToolCall::GitStatus => git::git_status(folder()?),
        ToolCall::GitDiff(args) => git::git_diff(folder()?, args),
        ToolCall::GitBlame(args) => git::git_blame(folder()?, args),
        ToolCall::GitLog(args) => git::git_log(folder()?, args),
        ToolCall::RunCommand(request) => run_command::run_command(folder()?, request, deps),
        ToolCall::SemanticSearch(args) => semantic_search::semantic_search(args, deps),
        ToolCall::Skill(args) => skill::skill(args, deps),
        ToolCall::WritePlan(args) => write_plan::write_plan(args),
        ToolCall::ReportFinding(args) => report_finding::report_finding(args, deps),
        ToolCall::Explore(args) => explore::explore(args, deps),
        ToolCall::ReadOutput(args) => process::read_output(args, deps),
        ToolCall::StopProcess(args) => process::stop_process(args, deps),
        ToolCall::ReadTerminal(args) => terminal::read_terminal(args, deps),
        ToolCall::RunInTerminal(args) => terminal::run_in_terminal(folder()?, args, deps),
        ToolCall::Mcp(args) => mcp::mcp(args, deps),
        ToolCall::KubeList(args) => cluster::kube_list(deps.kube, args),
        ToolCall::KubeGet(args) => cluster::kube_get(deps.kube, args),
        ToolCall::KubeEvents(args) => cluster::kube_events(deps.kube, args),
        ToolCall::KubeLogs(args) => cluster::kube_logs(deps.kube, args),
        ToolCall::KubeTop(args) => cluster::kube_top(deps.kube, args),
        ToolCall::KubeFieldHistory(args) => cluster::kube_field_history(deps.kube, args),
    }
}

#[cfg(test)]
mod definition_tests {
    use super::*;
    use crate::domain::llm::LlmToolCall;
    use crate::domain::tools::{
        DeleteDirectoryArgs, DeleteFileArgs, EditFileArgs, FileEdit, GitBlameArgs, GitDiffArgs, GitLogArgs,
        GrepArgs, ListFilesArgs, MoveArgs, ReadFileArgs, SemanticSearchArgs, SkillArgs, TodoArgs, WritePlanArgs, TodoUpdateStatus,
        WriteFileArgs,
        CreateDirectoryArgs, ProcessArgs, ReadTerminalArgs, RunInTerminalArgs,
        KubeEventsArgs, KubeFieldHistoryArgs, KubeGetArgs, KubeListArgs, KubeLogsArgs, KubeTopArgs,
    };
    use crate::services::ai_tools::parse::parse_tool_call;
    use std::collections::BTreeSet;

    /// Every tool with a schema of its own. MCP tools are described by their
    /// servers, at run time — see `domain::mcp::McpTools::definitions`.
    fn built_in() -> impl Iterator<Item = ToolName> {
        ToolName::ALL.iter().copied().filter(|t| *t != ToolName::Mcp)
    }

    fn definition_of(tool: ToolName) -> LlmToolDefinition {
        let (_, build) = DEFINITIONS
            .iter()
            .find(|(name, _)| *name == tool)
            .unwrap_or_else(|| panic!("{tool:?} is not advertised to the model"));
        build()
    }

    fn properties(tool: ToolName) -> BTreeSet<String> {
        definition_of(tool).parameters["properties"]
            .as_object()
            .expect("an object schema")
            .keys()
            .cloned()
            .collect()
    }

    /// The argument names the code actually accepts, read off a fully
    /// populated call rather than restated by hand — a field added to an args
    /// struct shows up here on its own.
    fn accepted_arguments(calls: &[ToolCall]) -> BTreeSet<String> {
        calls
            .iter()
            .flat_map(|call| {
                serde_json::to_value(call).expect("a call serializes")["args"]
                    .as_object()
                    .map(|args| args.keys().cloned().collect::<Vec<_>>())
                    .unwrap_or_default()
            })
            .collect()
    }

    /// One row per tool: the smallest call the model could send, and calls
    /// populating every field the tool takes.
    fn sample_calls(tool: ToolName) -> (&'static str, Vec<ToolCall>) {
        let path = || "a.rs".to_string();
        match tool {
            ToolName::ReadFile => (
                r#"{"path":"a.rs"}"#,
                vec![ToolCall::ReadFile(ReadFileArgs {
                    path: path(),
                    start_line: Some(1),
                    end_line: Some(9),
                    outline: Some(false),
                })],
            ),
            ToolName::Grep => (
                r#"{"pattern":"fn main"}"#,
                vec![ToolCall::Grep(GrepArgs {
                    pattern: "fn main".to_string(),
                    path: Some("src".to_string()),
                    glob: Some("*.rs".to_string()),
                    exclude: Some("src/docs/**".to_string()),
                    case_insensitive: Some(true),
                    max_results: Some(20),
                    context_lines: Some(2),
                })],
            ),
            ToolName::ListFiles => (
                "{}",
                vec![ToolCall::ListFiles(ListFilesArgs {
                    path: Some("src".to_string()),
                    depth: Some(2),
                    pattern: Some("*.rs".to_string()),
                })],
            ),
            ToolName::WriteFile => (
                r#"{"path":"a.rs","content":"x"}"#,
                vec![ToolCall::WriteFile(WriteFileArgs {
                    path: path(),
                    content: "x".to_string(),
                })],
            ),
            ToolName::EditFile => (
                r#"{"path":"a.rs","edits":[{"old":"a","new":"b"}]}"#,
                vec![ToolCall::EditFile(EditFileArgs {
                    path: path(),
                    edits: vec![FileEdit {
                        old: "a".to_string(),
                        new: "b".to_string(),
                    }],
                })],
            ),
            ToolName::CreateDirectory => (
                r#"{"path":"src"}"#,
                vec![ToolCall::CreateDirectory(CreateDirectoryArgs { path: path() })],
            ),
            ToolName::DeleteFile => (
                r#"{"path":"a.rs"}"#,
                vec![ToolCall::DeleteFile(DeleteFileArgs { path: path() })],
            ),
            ToolName::DeleteDirectory => (
                r#"{"path":"src"}"#,
                vec![ToolCall::DeleteDirectory(DeleteDirectoryArgs {
                    path: path(),
                    recursive: Some(true),
                })],
            ),
            ToolName::Move => (
                r#"{"path":"a.rs","newPath":"b.rs"}"#,
                vec![ToolCall::Move(MoveArgs {
                    path: path(),
                    new_path: "b.rs".to_string(),
                })],
            ),
            ToolName::Todo => (
                r#"{"op":"write","tasks":["do the thing"]}"#,
                // Both operations, because the schema describes the union of
                // their fields under one `op`.
                vec![
                    ToolCall::Todo(TodoArgs::Write {
                        tasks: vec!["do the thing".to_string()],
                    }),
                    ToolCall::Todo(TodoArgs::Update {
                        id: Some("t1".to_string()),
                        status: Some(TodoUpdateStatus::Completed),
                        note: Some("done".to_string()),
                    }),
                ],
            ),
            ToolName::GitStatus => ("{}", vec![ToolCall::GitStatus]),
            ToolName::GitDiff => (
                r#"{"path":"a.rs"}"#,
                vec![ToolCall::GitDiff(GitDiffArgs {
                    path: path(),
                    scope: Some("staged".to_string()),
                    commit: Some("HEAD~1".to_string()),
                    stat: None,
                })],
            ),
            ToolName::RunCommand => (
                r#"{"command":"cargo test"}"#,
                vec![ToolCall::RunCommand(crate::domain::command_exec::CommandRequest {
                    command: "cargo test".to_string(),
                    cwd: Some("crate".to_string()),
                    timeout_seconds: Some(30),
                    background: Some(true),
                })],
            ),
            ToolName::SemanticSearch => (
                r#"{"query":"where the index is kept current"}"#,
                vec![ToolCall::SemanticSearch(SemanticSearchArgs {
                    query: "where the index is kept current".to_string(),
                    fts: Some(vec!["sync".to_string()]),
                    queries: Some(vec!["how the index stays in step with the files".to_string()]),
                    top_k: Some(5),
                    preview: Some(true),
                    include_docs: Some(true),
                    glob: Some("src/**".to_string()),
                    exclude: Some("**/generated/**".to_string()),
                })],
            ),
            ToolName::WritePlan => (
                r##"{"content":"# Fix"}"##,
                vec![ToolCall::WritePlan(WritePlanArgs { content: "# Fix".to_string() })],
            ),
            ToolName::ReportFinding => (
                r#"{"path":"a.rs","existingCode":"x","title":"t","body":"b","severity":"high","category":"bug"}"#,
                vec![ToolCall::ReportFinding(crate::domain::review::FindingArgs {
                    path: path(),
                    existing_code: "x".to_string(),
                    title: "t".to_string(),
                    body: "b".to_string(),
                    severity: Some(crate::domain::review::Severity::High),
                    category: Some(crate::domain::review::Category::Bug),
                    suggestion: Some("y".to_string()),
                })],
            ),
            ToolName::Explore => (
                r#"{"task":"where is X"}"#,
                vec![ToolCall::Explore(crate::domain::tools::ExploreArgs { task: "where is X".to_string() })],
            ),
            ToolName::Skill => (
                r#"{"name":"release"}"#,
                vec![ToolCall::Skill(SkillArgs {
                    name: "release".to_string(),
                    path: Some("checklist.md".to_string()),
                })],
            ),
            ToolName::GitBlame => (
                r#"{"path":"a.rs"}"#,
                vec![ToolCall::GitBlame(GitBlameArgs {
                    path: path(),
                    start_line: Some(1),
                    end_line: Some(9),
                })],
            ),
            ToolName::GitLog => (
                r#"{"path":"a.rs","limit":5,"query":"fix"}"#,
                vec![ToolCall::GitLog(GitLogArgs {
                    path: Some(path()),
                    limit: Some(5),
                    query: Some("fix".to_string()),
                })],
            ),
            ToolName::ReadOutput => (r#"{"id":1}"#, vec![ToolCall::ReadOutput(ProcessArgs { id: Some(1) })]),
            ToolName::StopProcess => (r#"{"id":1}"#, vec![ToolCall::StopProcess(ProcessArgs { id: Some(1) })]),
            ToolName::ReadTerminal => (
                r#"{"id":"2","lines":40}"#,
                vec![ToolCall::ReadTerminal(ReadTerminalArgs { id: Some(2), lines: Some(40) })],
            ),
            ToolName::RunInTerminal => (
                r#"{"command":"npm run dev"}"#,
                vec![ToolCall::RunInTerminal(RunInTerminalArgs { command: "npm run dev".into(), id: None })],
            ),
            ToolName::KubeList => (
                r#"{"kind":"pods"}"#,
                vec![ToolCall::KubeList(KubeListArgs {
                    kind: "pods".into(),
                    namespace: Some("orders".into()),
                    label_selector: Some("app=api".into()),
                    field_selector: Some("status.phase=Running".into()),
                    fields: Some(vec!["spec.nodeName".into()]),
                    limit: Some(50),
                    continue_token: Some("token".into()),
                })],
            ),
            ToolName::KubeGet => (
                r#"{"kind":"Deployment","name":"api"}"#,
                vec![ToolCall::KubeGet(KubeGetArgs {
                    kind: "Deployment".into(),
                    name: "api".into(),
                    namespace: Some("orders".into()),
                    sections: Some(vec!["spec".into()]),
                })],
            ),
            ToolName::KubeEvents => (
                "{}",
                vec![ToolCall::KubeEvents(KubeEventsArgs {
                    namespace: Some("orders".into()),
                    kind: Some("Pod".into()),
                    name: Some("api-1".into()),
                    warnings_only: Some(true),
                })],
            ),
            ToolName::KubeLogs => (
                r#"{"pod":"api-1"}"#,
                vec![ToolCall::KubeLogs(KubeLogsArgs {
                    pod: Some("api-1".into()),
                    kind: Some("Deployment".into()),
                    name: Some("api".into()),
                    label_selector: Some("app=api".into()),
                    namespace: Some("orders".into()),
                    container: Some("app".into()),
                    previous: Some(true),
                    tail: Some(100),
                    since: Some("10m".into()),
                    grep: Some("error".into()),
                })],
            ),
            ToolName::KubeTop => (
                r#"{"kind":"pods"}"#,
                vec![ToolCall::KubeTop(KubeTopArgs { kind: "pods".into(), namespace: Some("orders".into()) })],
            ),
            ToolName::KubeFieldHistory => (
                r#"{"kind":"Service","name":"api"}"#,
                vec![ToolCall::KubeFieldHistory(KubeFieldHistoryArgs {
                    kind: "Service".into(),
                    name: "api".into(),
                    namespace: Some("orders".into()),
                    paths: Some(vec!["metadata.annotations".into()]),
                })],
            ),
            ToolName::Mcp => unreachable!("an MCP tool's schema is its server's; see built_in()"),
        }
    }

    /// A tool that exists but is never advertised is one the model can only
    /// reach by guessing its name.
    #[test]
    fn every_tool_is_advertised_exactly_once() {
        let advertised: Vec<ToolName> = DEFINITIONS.iter().map(|(tool, _)| *tool).collect();
        for tool in built_in() {
            assert_eq!(
                advertised.iter().filter(|&&t| t == tool).count(),
                1,
                "{tool:?} must appear in DEFINITIONS exactly once"
            );
        }
        assert_eq!(advertised.len(), built_in().count());
    }

    /// The name in the schema is the name the model will send, and the
    /// dispatcher matches on the wire name — a mismatch is a tool that can
    /// only ever answer "unknown tool".
    #[test]
    fn a_schema_is_named_by_its_wire_name() {
        for tool in built_in() {
            assert_eq!(definition_of(tool).name, tool.wire_name());
        }
        for definition in tool_definitions() {
            assert!(
                ToolName::from_wire_name(&definition.name).is_some(),
                "{} is advertised but unknown to the dispatcher",
                definition.name
            );
        }
    }

    /// The whole point of keeping the schema beside the implementation: an
    /// argument the code accepts but the schema never mentions is one the
    /// model cannot use, and a described argument the code ignores is a
    /// promise the tool does not keep.
    #[test]
    fn the_schema_and_the_arguments_describe_the_same_fields() {
        for tool in built_in() {
            let (_, samples) = sample_calls(tool);
            assert_eq!(
                properties(tool),
                accepted_arguments(&samples),
                "{tool:?}: the schema and the args struct disagree"
            );
        }
    }

    /// Nested arguments drift the same way, and are easier to forget.
    #[test]
    fn an_edits_entry_is_described_field_by_field() {
        let described: BTreeSet<String> = definition_of(ToolName::EditFile).parameters
            ["properties"]["edits"]["items"]["properties"]
            .as_object()
            .expect("edits items are an object schema")
            .keys()
            .cloned()
            .collect();
        let accepted: BTreeSet<String> = serde_json::to_value(FileEdit {
            old: "a".to_string(),
            new: "b".to_string(),
        })
        .unwrap()
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
        assert_eq!(described, accepted);
    }

    /// Everything the schema marks required must be enough on its own — a
    /// model that sends exactly what was asked for must not get a parse error.
    #[test]
    fn the_smallest_described_call_parses() {
        for tool in built_in() {
            let (minimal, _) = sample_calls(tool);
            let call = LlmToolCall {
                id: "call_1".to_string(),
                name: tool.wire_name().to_string(),
                arguments: minimal.to_string(),
            };
            let parsed = parse_tool_call(&call)
                .unwrap_or_else(|e| panic!("{tool:?}: the minimal call was refused: {e}"));
            assert_eq!(parsed.name(), tool);

            for required in definition_of(tool).parameters["required"]
                .as_array()
                .expect("a required list, even if empty")
            {
                let field = required.as_str().expect("a field name");
                let minimal: serde_json::Value = serde_json::from_str(minimal).unwrap();
                assert!(
                    minimal.get(field).is_some(),
                    "{tool:?}: {field} is required but the minimal call omits it"
                );
            }
        }
    }
}
