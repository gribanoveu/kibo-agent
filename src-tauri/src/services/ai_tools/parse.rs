//! Turning the model's raw tool call into a typed one, and rejecting it early
//! when it cannot possibly be honoured.
//!
//! Defensive throughout against a model that gets the shape almost right: a
//! complete JSON object with trailing noise after it still parses, and an
//! argument error names the path to the field rather than a byte offset.
//!
//! A model getting its own call wrong is ordinary tool-calling behaviour, not a
//! fault: every error here is meant to go back as the call's result so the
//! model can correct itself, never to fail the turn.

use serde::de::DeserializeOwned;

use crate::domain::chat_role::ChatRole;
use crate::domain::conversation_mode::{self, ConversationMode};
use crate::domain::llm::LlmToolCall;
use crate::domain::tools::{McpCallArgs, ReadFiles, ToolCall, ToolError, ToolScope, WriteBlocked, MCP_PREFIX};

use super::resolve::resolve_writable;

pub fn parse_tool_call(call: &LlmToolCall) -> Result<ToolCall, ToolError> {
    // A connected server's tool. Its arguments are the server's to check
    // against its own schema; here only that they are an object.
    if call.name.starts_with(MCP_PREFIX) {
        let arguments: serde_json::Value = args(call)?;
        if !arguments.is_object() {
            return Err(ToolError::InvalidArguments { tool: call.name.clone(), reason: "arguments must be a JSON object".into() });
        }
        return Ok(ToolCall::Mcp(McpCallArgs { name: call.name.clone(), arguments }));
    }
    Ok(match call.name.as_str() {
        "readFile" => ToolCall::ReadFile(args(call)?),
        "grep" => ToolCall::Grep(args(call)?),
        "listFiles" => ToolCall::ListFiles(args(call)?),
        "writeFile" => ToolCall::WriteFile(args(call)?),
        "editFile" => ToolCall::EditFile(args(call)?),
        "createDirectory" => ToolCall::CreateDirectory(args(call)?),
        "deleteFile" => ToolCall::DeleteFile(args(call)?),
        "deleteDirectory" => ToolCall::DeleteDirectory(args(call)?),
        "move" => ToolCall::Move(args(call)?),
        "todo" => ToolCall::Todo(args(call)?),
        "gitDiff" => ToolCall::GitDiff(args(call)?),
        "gitBlame" => ToolCall::GitBlame(args(call)?),
        "gitLog" => ToolCall::GitLog(args(call)?),
        "runCommand" => ToolCall::RunCommand(args(call)?),
        "semanticSearch" => ToolCall::SemanticSearch(args(call)?),
        "skill" => ToolCall::Skill(args(call)?),
        "writePlan" => ToolCall::WritePlan(args(call)?),
        "reportFinding" => ToolCall::ReportFinding(args(call)?),
        "explore" => ToolCall::Explore(args(call)?),
        "remember" => ToolCall::Remember(args(call)?),
        "readOutput" => ToolCall::ReadOutput(args(call)?),
        "stopProcess" => ToolCall::StopProcess(args(call)?),
        "readTerminal" => ToolCall::ReadTerminal(args(call)?),
        "runInTerminal" => ToolCall::RunInTerminal(args(call)?),
        "kubeList" => ToolCall::KubeList(args(call)?),
        "kubeGet" => ToolCall::KubeGet(args(call)?),
        "kubeEvents" => ToolCall::KubeEvents(args(call)?),
        "kubeLogs" => ToolCall::KubeLogs(args(call)?),
        "kubeTop" => ToolCall::KubeTop(args(call)?),
        "kubeFieldHistory" => ToolCall::KubeFieldHistory(args(call)?),
        "kubeDiagnose" => ToolCall::KubeDiagnose(args(call)?),
        "kubeWaitRollout" => ToolCall::KubeWaitRollout(args(call)?),
        "kubeProbe" => ToolCall::KubeProbe(args(call)?),
        "kubeRunbook" => ToolCall::KubeRunbook(args(call)?),
        "kubeScale" => ToolCall::KubeScale(args(call)?),
        "kubeUndo" => ToolCall::KubeUndo(args(call)?),
        "kubeSuspend" => ToolCall::KubeSuspend(args(call)?),
        "kubeRolloutRestart" => ToolCall::KubeRolloutRestart(args(call)?),
        "kubeRolloutUndo" => ToolCall::KubeRolloutUndo(args(call)?),
        "kubeApply" => ToolCall::KubeApply(args(call)?),
        "kubeDelete" => ToolCall::KubeDelete(args(call)?),
        "webSearch" => ToolCall::WebSearch(args(call)?),
        "webFetch" => ToolCall::WebFetch(args(call)?),
        "toolSearch" => ToolCall::ToolSearch(args(call)?),
        // No arguments, so nothing to deserialize — and nothing for a model to
        // get wrong. Whatever it sent alongside is ignored rather than refused.
        "gitStatus" => ToolCall::GitStatus,
        other => return Err(ToolError::UnknownTool(other.to_string())),
    })
}

/// Chat mode's preflight: the call parses, and the chat's role has the tool.
/// No containment: there is no folder to be inside, and a role's tools touch
/// none — [`crate::services::ai_tools::tools::dispatch`] refuses one that would.
pub fn preflight_chat_call(role: ChatRole, call: &LlmToolCall) -> Result<ToolCall, ToolError> {
    let parsed = parse_tool_call(call)?;
    if !role.tools().contains(&parsed.name()) {
        return Err(ToolError::NotOfferedInChat(parsed.name().wire_name().to_string()));
    }
    Ok(parsed)
}

/// Rejects what cannot succeed, before a human is asked to approve it.
///
/// A confirmation card for a write that was always going to fail costs the
/// user a decision and the turn a round trip, and teaches the model nothing it
/// could not have been told immediately.
pub fn preflight_tool_call(
    scope: &ToolScope,
    mode: ConversationMode,
    reads: &ReadFiles,
    call: &LlmToolCall,
) -> Result<(), ToolError> {
    let parsed = parse_tool_call(call)?;

    // The mode, enforced rather than described. The tool was left out of the
    // request, so asking for it means the model is working from memory of an
    // earlier round — and an error it can read is what corrects that, where a
    // sentence in the prompt did not.
    if !conversation_mode::offers(mode, parsed.name()) {
        let tool = parsed.name().wire_name().to_string();
        // A helper's limits are not a choice the user made, and "say what
        // would need to be done" is advice for the chat, not for a helper.
        if mode == ConversationMode::Explore {
            return Err(ToolError::Explore(format!(
                "`{tool}` is not available to a helper, which only reads and searches. Answer from what you have found, and say what is left to check."
            )));
        }
        return Err(ToolError::NotOfferedInMode(tool));
    }

    // Containment, checked here as well as inside each tool. The duplication is
    // the point: this runs before approval, the tool's own check runs before
    // the disk.
    let command_cwd: [&str; 1];
    let paths: &[&str] = match &parsed {
        ToolCall::WriteFile(a) => &[&a.path],
        ToolCall::EditFile(a) => &[&a.path],
        ToolCall::DeleteFile(a) => &[&a.path],
        ToolCall::CreateDirectory(a) => &[&a.path],
        ToolCall::DeleteDirectory(a) => &[&a.path],
        ToolCall::Move(a) => &[&a.path, &a.new_path],
        // The working directory only. What the command line then names is
        // beyond any check here — see `domain::command_exec`.
        ToolCall::RunCommand(request) => match &request.cwd {
            Some(cwd) if !cwd.is_empty() && cwd != "." => {
                command_cwd = [cwd.as_str()];
                &command_cwd
            }
            _ => &[],
        },
        _ => &[],
    };
    for path in paths {
        resolve_writable(scope, path)?;
    }

    // "You have not read this file" is knowable without touching the disk and
    // without asking anyone, so asking would be pure ceremony.
    let deleting = matches!(parsed, ToolCall::DeleteFile(_));
    if let Some((path, whole)) = match &parsed {
        ToolCall::WriteFile(a) => Some((a.path.as_str(), true)),
        ToolCall::DeleteFile(a) => Some((a.path.as_str(), true)),
        ToolCall::EditFile(a) => Some((a.path.as_str(), false)),
        _ => None,
    } {
        let resolved = resolve_writable(scope, path)?;
        if let Ok(current) = std::fs::read_to_string(&resolved) {
            let relative = super::resolve::relative_to_root(scope, &resolved)?;
            reads
                .check(&relative, &current, whole)
                .map_err(|blocked| match blocked {
                    // Said as a deletion: "before writing to it" sends the
                    // model looking for a write it never asked for.
                    WriteBlocked::NeverRead | WriteBlocked::ReadInPart if deleting => {
                        ToolError::DeleteNotReadInFull(relative.clone())
                    }
                    WriteBlocked::NeverRead => ToolError::FileNotRead(relative.clone()),
                    WriteBlocked::ReadInPart => ToolError::FileReadInPart(relative.clone()),
                    WriteBlocked::ChangedSinceRead => ToolError::FileChangedSinceRead(relative),
                })?;
        }
    }

    Ok(())
}

fn args<T: DeserializeOwned>(call: &LlmToolCall) -> Result<T, ToolError> {
    lenient_json_object(&call.arguments).map_err(|reason| ToolError::InvalidArguments {
        tool: call.name.clone(),
        reason,
    })
}

/// Deserializes one JSON object, ignoring anything after it.
///
/// Two accommodations, both for things models actually emit. Trailing noise
/// after a complete object is tolerated because nothing here calls `end()` — a
/// stream that closed with a stray brace or a repeated fragment still yields
/// the object it already produced. And the error carries the *path* to the
/// offending field rather than a byte offset: a model once got one edit right
/// and, several elements later in the same array, typed `oldText`/`newText`
/// instead of `old`/`new`. "at line 1 column 7275" gives no way to tell which
/// edit was wrong without counting characters; `edits[3]: unknown field
/// `oldText`` says it outright.
fn lenient_json_object<T: DeserializeOwned>(input: &str) -> Result<T, String> {
    let input = if input.trim().is_empty() { "{}" } else { input };
    let mut deserializer = serde_json::Deserializer::from_str(input);
    serde_path_to_error::deserialize(&mut deserializer)
        .map_err(|e| humanize_serde_error(&e.to_string()))
}

/// Rewrites serde's Rust vocabulary into terms a model can act on.
///
/// This string goes back as the failed call's result. `expected u32` and
/// `expected a sequence` describe Rust types the model has no reason to know;
/// the observed consequence of one was a model retrying the identical call and
/// then dropping the parameter rather than unquoting it. Quoted scalars are
/// handled before this point by `domain::flexible_args`, so whatever reaches
/// here is a real mistake — it should at least name the shape actually wanted.
fn humanize_serde_error(reason: &str) -> String {
    reason
        .replace("expected u32", "expected a number")
        .replace("expected u64", "expected a number")
        .replace("expected usize", "expected a number")
        .replace("expected a sequence", "expected a JSON array")
        .replace("expected a map", "expected a JSON object")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::tools::{ReadFileArgs, TodoArgs, TodoUpdateStatus};
    use crate::testing::temp_dir;
    use std::path::PathBuf;

    fn call(name: &str, arguments: &str) -> LlmToolCall {
        LlmToolCall {
            id: "call_1".into(),
            name: name.into(),
            arguments: arguments.into(),
        }
    }

    fn fixture(label: &str) -> (ToolScope, PathBuf, ReadFiles) {
        let dir = temp_dir(label);
        let scope = ToolScope::new(&dir).expect("root resolves");
        let root = scope.root().to_path_buf();
        (scope, root, ReadFiles::default())
    }

    /// Every wire name the model is given has to land on a variant. A tool that
    /// exists in the dispatcher but not here is unreachable.
    #[test]
    fn every_tool_name_parses() {
        let cases = [
            ("readFile", r#"{"path": "a"}"#),
            ("grep", r#"{"pattern": "x"}"#),
            ("listFiles", "{}"),
            ("writeFile", r#"{"path": "a", "content": "x"}"#),
            ("editFile", r#"{"path": "a", "edits": []}"#),
            ("createDirectory", r#"{"path": "a"}"#),
            ("deleteFile", r#"{"path": "a"}"#),
            ("deleteDirectory", r#"{"path": "a"}"#),
            ("move", r#"{"path": "a", "newPath": "b"}"#),
            ("todo", r#"{"op": "write", "tasks": ["x"]}"#),
            ("gitDiff", r#"{"path": "a"}"#),
            ("gitBlame", r#"{"path": "a"}"#),
            ("gitLog", "{}"),
            ("gitStatus", ""),
        ];
        for (name, arguments) in cases {
            let parsed = parse_tool_call(&call(name, arguments))
                .unwrap_or_else(|e| panic!("{name} failed to parse: {e}"));
            assert_eq!(parsed.name().wire_name(), name);
        }
    }

    /// An MCP call keeps its full name and its arguments as sent; they only
    /// have to be an object — a server's tool takes named parameters.
    #[test]
    fn an_mcp_call_parses_to_its_name_and_an_object() {
        assert_eq!(
            parse_tool_call(&call("mcp__gh__search", r#"{"q": "x"}"#)).unwrap(),
            ToolCall::Mcp(McpCallArgs { name: "mcp__gh__search".into(), arguments: serde_json::json!({"q": "x"}) })
        );
        assert_eq!(
            parse_tool_call(&call("mcp__gh__list", "")).unwrap(),
            ToolCall::Mcp(McpCallArgs { name: "mcp__gh__list".into(), arguments: serde_json::json!({}) })
        );
        for not_an_object in ["[1]", "\"q\"", "3"] {
            assert!(
                matches!(parse_tool_call(&call("mcp__gh__search", not_an_object)), Err(ToolError::InvalidArguments { .. })),
                "{not_an_object}"
            );
        }
    }

    #[test]
    fn an_unknown_tool_is_named_back() {
        let err = parse_tool_call(&call("summonDragon", "{}")).expect_err("no such tool");
        assert!(matches!(err, ToolError::UnknownTool(ref t) if t == "summonDragon"));
    }

    /// A tool with no arguments must not depend on the model sending `{}`.
    #[test]
    fn missing_arguments_read_as_an_empty_object() {
        assert_eq!(parse_tool_call(&call("gitStatus", "")).unwrap(), ToolCall::GitStatus);
        assert!(parse_tool_call(&call("listFiles", "   ")).is_ok());
    }

    /// A stream that closed with a repeated fragment or a stray brace still
    /// yields the object it already produced.
    #[test]
    fn trailing_noise_after_a_complete_object_is_tolerated() {
        let parsed = parse_tool_call(&call("readFile", r#"{"path": "a.txt"}} {"path":"#))
            .expect("the complete object is enough");
        assert_eq!(
            parsed,
            ToolCall::ReadFile(ReadFileArgs {
                path: "a.txt".into(),
                ..ReadFileArgs::default()
            })
        );
    }

    /// The failure this exists for: one edit right, another several elements
    /// later with the wrong field names. A byte offset cannot say which.
    #[test]
    fn a_bad_field_deep_in_an_array_names_its_path() {
        let err = parse_tool_call(&call(
            "editFile",
            r#"{"path": "a", "edits": [{"old": "x", "new": "y"}, {"oldText": "p", "newText": "q"}]}"#,
        ))
        .expect_err("second edit is malformed");

        let ToolError::InvalidArguments { tool, reason } = err else {
            panic!("wrong error")
        };
        assert_eq!(tool, "editFile");
        assert!(reason.contains("edits[1]"), "{reason}");
    }

    /// Rust's vocabulary describes types the model has no reason to know.
    #[test]
    fn serde_vocabulary_is_rewritten_for_the_model() {
        let err = parse_tool_call(&call("editFile", r#"{"path": "a", "edits": "not an array"}"#))
            .expect_err("edits must be an array");
        let ToolError::InvalidArguments { reason, .. } = err else {
            panic!("wrong error")
        };
        assert!(reason.contains("JSON array"), "{reason}");
        assert!(!reason.contains("sequence"), "{reason}");
    }

    /// Quoted scalars are handled before this point, and must survive it.
    #[test]
    fn quoted_scalars_still_parse() {
        let parsed = parse_tool_call(&call("readFile", r#"{"path": "a", "startLine": "3"}"#))
            .expect("parses");
        let ToolCall::ReadFile(args) = parsed else {
            panic!("wrong variant")
        };
        assert_eq!(args.start_line, Some(3));
    }

    #[test]
    fn todo_parses_from_its_op_discriminator() {
        let parsed = parse_tool_call(&call("todo", r#"{"op": "write", "tasks": ["a"]}"#)).unwrap();
        assert_eq!(
            parsed,
            ToolCall::Todo(TodoArgs::Write {
                tasks: vec!["a".into()]
            })
        );
    }

    /// What deepseek-flash sent in `agent_bench` instead of a well-formed
    /// call: each says plainly what it is, and each was a lost round.
    #[test]
    fn a_todo_call_with_a_wrong_or_missing_op_is_read_from_its_fields() {
        let todo = |arguments: &str| match parse_tool_call(&call("todo", arguments)) {
            Ok(ToolCall::Todo(args)) => Ok(args),
            Ok(other) => panic!("{other:?}"),
            Err(e) => Err(e.to_string()),
        };
        let write = |titles: &[&str]| Ok(TodoArgs::Write { tasks: titles.iter().map(|t| t.to_string()).collect() });
        // The list in `op`, as a JSON string, of objects or of strings.
        assert_eq!(todo(r#"{"op": "[\n  {\"title\": \"Read logs\"}, {\"title\": \"Fix A\"}\n]"}"#), write(&["Read logs", "Fix A"]));
        assert_eq!(todo(r#"{"op": "[\"a\", \"b\"]"}"#), write(&["a", "b"]));
        // `op` left out, or a title in it, with the list where it belongs.
        assert_eq!(todo(r#"{"tasks": ["a", "b"]}"#), write(&["a", "b"]));
        assert_eq!(todo(r#"{"op": "Demux log / write plan", "tasks": ["Demux log / write plan"]}"#), write(&["Demux log / write plan"]));
        assert_eq!(todo(r#"{"op": "write", "tasks": "[\"a\"]"}"#), write(&["a"]));
        assert_eq!(todo(r#"{"op": "write", "tasks": [{"content": "a"}]}"#), write(&["a"]));
        // No `op`, but an update's fields.
        assert_eq!(
            todo(r#"{"status": "completed", "note": "done"}"#),
            Ok(TodoArgs::Update { id: None, ids: vec![], status: Some(TodoUpdateStatus::Completed), note: Some("done".into()) })
        );
        // Undecidable: said, not guessed.
        let lone_title = todo(r#"{"op": "Demux log / write plan"}"#).unwrap_err();
        assert!(lone_title.contains("op must be \"write\"") && lone_title.contains("Demux log"), "{lone_title}");
        assert!(todo(r#"{"op": "write"}"#).unwrap_err().contains("needs tasks"));
        assert!(todo("{}").is_err());
    }

    /// A confirmation card for a write that was always going to fail costs a
    /// human decision and teaches the model nothing it could not hear at once.
    #[test]
    fn preflight_refuses_a_path_outside_the_root_before_anyone_is_asked() {
        let (scope, _, reads) = fixture("preflight-escape");
        let err = preflight_tool_call(
            &scope,
            ConversationMode::Agent,
            &reads,
            &call("writeFile", r#"{"path": "../outside.txt", "content": "x"}"#),
        )
        .expect_err("escapes the root");
        assert!(matches!(err, ToolError::PathEscape(_)));
    }

    #[test]
    fn preflight_refuses_a_write_to_an_unread_file() {
        let (scope, root, reads) = fixture("preflight-unread");
        std::fs::write(root.join("a.txt"), "precious\n").expect("writable");

        let err = preflight_tool_call(
            &scope,
            ConversationMode::Agent,
            &reads,
            &call("writeFile", r#"{"path": "a.txt", "content": "x"}"#),
        )
        .expect_err("never read");

        assert!(matches!(err, ToolError::FileNotRead(_)));
        assert!(err.to_string().ends_with("an outline is not a read"), "{err}");

        // A deletion is refused as one, read not at all or only in part.
        let delete = call("deleteFile", r#"{"path": "a.txt"}"#);
        let err = preflight_tool_call(&scope, ConversationMode::Agent, &reads, &delete).expect_err("never read");
        assert!(matches!(err, ToolError::DeleteNotReadInFull(_)), "{err}");
        let mut part = reads.clone();
        part.record("a.txt", "precious\n", false);
        let err = preflight_tool_call(&scope, ConversationMode::Agent, &part, &delete).expect_err("read in part");
        assert!(matches!(err, ToolError::DeleteNotReadInFull(_)), "{err}");
    }

    /// Creating a file is not a write over anything, so it must reach approval.
    #[test]
    fn preflight_lets_a_new_file_through() {
        let (scope, _, reads) = fixture("preflight-new");
        preflight_tool_call(
            &scope,
            ConversationMode::Agent,
            &reads,
            &call("writeFile", r#"{"path": "fresh.txt", "content": "x"}"#),
        )
        .expect("nothing to lose, nothing to check");
    }

    #[test]
    fn preflight_checks_both_ends_of_a_move() {
        let (scope, root, reads) = fixture("preflight-move");
        std::fs::write(root.join("a.txt"), "x").expect("writable");

        assert!(matches!(
            preflight_tool_call(
                &scope,
                ConversationMode::Agent,
                &reads,
                &call("move", r#"{"path": "a.txt", "newPath": "../b.txt"}"#)
            ),
            Err(ToolError::PathEscape(_))
        ));
    }

    /// Reads carry no risk, so preflight must not invent a reason to stop one.
    #[test]
    fn preflight_never_blocks_a_read() {
        let (scope, root, reads) = fixture("preflight-read");
        std::fs::write(root.join("a.txt"), "x").expect("writable");
        preflight_tool_call(&scope, ConversationMode::Agent, &reads, &call("readFile", r#"{"path": "a.txt"}"#))
            .expect("reading is always allowed");
        preflight_tool_call(&scope, ConversationMode::Agent, &reads, &call("gitStatus", "")).expect("also allowed");
    }

    /// An unparseable call is stopped here rather than becoming a card for
    /// something nobody can act on.
    #[test]
    fn preflight_surfaces_a_parse_failure() {
        let (scope, _, reads) = fixture("preflight-garbage");
        assert!(matches!(
            preflight_tool_call(&scope, ConversationMode::Agent, &reads, &call("writeFile", "not json at all")),
            Err(ToolError::InvalidArguments { .. })
        ));
    }
}
