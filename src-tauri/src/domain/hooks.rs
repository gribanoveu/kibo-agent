//! Hooks: commands the user has run at points of a turn.
//!
//! The format is Claude Code's `hooks` object, so a hook written for it pastes
//! in: an event name, groups with a `matcher` over the tool name, and
//! `{"type": "command", "command": …, "timeout": …}` entries. The command gets
//! the event as JSON on stdin; exit code 2 blocks and its stderr is the
//! reason; any other failure is a warning and the turn goes on.
//!
//! Decisions behind this (global only, deny-only, fail-open) are in
//! `docs/06-port-plan.md`, stage 7, "Решения по хукам".
//!
//! **Tool names are this app's** (`runCommand`, `editFile`), not Claude
//! Code's (`Bash`, `Edit`): a matcher written for those matches nothing here.
//! One name stands for two tools: a matcher for `runCommand` also meets
//! `runInTerminal`, which runs the same kind of line in the user's shell —
//! a guard on commands must not be walked around by typing instead.

use std::collections::BTreeMap;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;

use crate::domain::command_exec::{CommandError, CommandOutput};

pub const DEFAULT_TIMEOUT_SECS: u32 = 60;
/// How many times Stop hooks may send the model back to work in one turn.
/// A hook that always exits 2 would otherwise keep the turn going until the
/// tool budget runs out; the hook is told (`stop_hook_active`) that it has
/// already done so, as in Claude Code.
pub const MAX_STOP_BLOCKS: u32 = 3;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HooksConfig {
    /// Keyed by event name. Events this app does not fire are kept, not
    /// refused: a configuration shared with Claude Code has them.
    #[serde(default)]
    pub hooks: BTreeMap<String, Vec<HookGroup>>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HookGroup {
    /// A regular expression over the whole tool name; empty or `*` is every
    /// tool. Not read for `Stop`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub matcher: String,
    #[serde(default)]
    pub hooks: Vec<HookCommand>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HookCommand {
    /// Only `command` runs here; Claude Code's `prompt` and `agent` hooks are
    /// kept in the file and skipped.
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub command: String,
    /// Seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u32>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl HookCommand {
    pub fn timeout_secs(&self) -> u32 {
        self.timeout.filter(|t| *t > 0).unwrap_or(DEFAULT_TIMEOUT_SECS)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookEvent {
    /// Before a call the user has approved: may refuse it.
    PreToolUse,
    /// After a call that succeeded: may tell the model something about it.
    PostToolUse,
    /// The model has finished: may send it back to work.
    Stop,
}

impl HookEvent {
    pub fn name(self) -> &'static str {
        match self {
            Self::PreToolUse => "PreToolUse",
            Self::PostToolUse => "PostToolUse",
            Self::Stop => "Stop",
        }
    }
}

#[derive(Debug, Error)]
pub enum HooksConfigError {
    #[error("could not read the hooks configuration: {0}")]
    Read(String),
    #[error("the hooks configuration is not valid: {0}")]
    Parse(String),
    #[error("could not write the hooks configuration: {0}")]
    Write(String),
}

pub fn parse(text: &str) -> Result<HooksConfig, HooksConfigError> {
    if text.trim().is_empty() {
        return Ok(HooksConfig::default());
    }
    serde_json::from_str(text).map_err(|e| HooksConfigError::Parse(e.to_string()))
}

/// The commands to run for `event` on `tool`, in the order the file lists
/// them. A matcher that is not a valid expression matches nothing — a typo
/// must not turn a hook meant for one tool into a hook on every tool.
pub fn matching<'a>(config: &'a HooksConfig, event: HookEvent, tool: Option<&str>) -> Vec<&'a HookCommand> {
    let Some(groups) = config.hooks.get(event.name()) else { return Vec::new() };
    groups
        .iter()
        .filter(|group| event == HookEvent::Stop || tool_names(tool.unwrap_or_default()).iter().any(|name| matches(&group.matcher, name)))
        .flat_map(|group| &group.hooks)
        .filter(|hook| hook.kind == "command" && !hook.command.trim().is_empty())
        .collect()
}

/// What a matcher is tried against for `tool`: its own name, and for a line
/// typed into the terminal, the command tool's too. The hook is still told
/// the real name, and `tool_input.command` has the same shape in both.
fn tool_names(tool: &str) -> Vec<&str> {
    use crate::domain::tools::ToolName;
    if tool == ToolName::RunInTerminal.wire_name() {
        vec![tool, ToolName::RunCommand.wire_name()]
    } else {
        vec![tool]
    }
}

fn matches(matcher: &str, tool: &str) -> bool {
    let matcher = matcher.trim();
    if matcher.is_empty() || matcher == "*" {
        return true;
    }
    Regex::new(&format!("^(?:{matcher})$")).is_ok_and(|re| re.is_match(tool))
}

/// One hook as the tab lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HookItem {
    pub event: String,
    pub matcher: String,
    pub command: String,
    pub timeout_secs: u32,
    /// Why it will not run as written, when the file alone says so.
    pub problem: Option<String>,
}

const EVENTS: [HookEvent; 3] = [HookEvent::PreToolUse, HookEvent::PostToolUse, HookEvent::Stop];

/// Every hook in the file, in its order, each with what is wrong with it.
pub fn items(config: &HooksConfig) -> Vec<HookItem> {
    config
        .hooks
        .iter()
        .flat_map(|(event, groups)| groups.iter().map(move |group| (event, group)))
        .flat_map(|(event, group)| {
            group.hooks.iter().map(move |hook| HookItem {
                event: event.clone(),
                matcher: group.matcher.clone(),
                command: hook.command.clone(),
                timeout_secs: hook.timeout_secs(),
                problem: problem(event, &group.matcher, hook),
            })
        })
        .collect()
}

fn problem(event: &str, matcher: &str, hook: &HookCommand) -> Option<String> {
    let Some(event) = EVENTS.into_iter().find(|e| e.name() == event) else {
        return Some(format!("{event} hooks do not run in this app — only PreToolUse, PostToolUse and Stop"));
    };
    if hook.kind != "command" {
        return Some(format!("\"{}\" hooks do not run in this app — only \"command\"", hook.kind));
    }
    if hook.command.trim().is_empty() {
        return Some("no command to run".into());
    }
    let matcher = matcher.trim();
    if event == HookEvent::Stop || matcher.is_empty() || matcher == "*" {
        return None;
    }
    if let Err(e) = Regex::new(&format!("^(?:{matcher})$")) {
        return Some(format!("the matcher is not a valid regular expression: {e}"));
    }
    // A server's tools are not known here; a matcher that names them is
    // taken at its word.
    let names_a_tool = matcher.contains("mcp__")
        || crate::domain::tools::ToolName::ALL.iter().any(|tool| matches(matcher, tool.wire_name()));
    (!names_a_tool).then(|| {
        "the matcher names none of this app's tools. They are called runCommand, editFile, writeFile, readFile… \
         — not Bash, Edit, Write, Read as in Claude Code"
            .to_string()
    })
}

/// What one hook's run means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookOutcome {
    Pass,
    /// Exit code 2; the text is its stderr, or a stand-in when it said
    /// nothing — a block with no reason still has to be explained to the model.
    Block(String),
    /// It failed some other way. The turn goes on.
    Warn(String),
}

pub fn outcome(hook: &HookCommand, run: Result<CommandOutput, CommandError>) -> HookOutcome {
    let output = match run {
        Ok(output) => output,
        Err(e) => return HookOutcome::Warn(format!("hook `{}` did not run: {e}", hook.command)),
    };
    if output.timed_out {
        return HookOutcome::Warn(format!(
            "hook `{}` did not finish within {} s and was stopped",
            hook.command,
            hook.timeout_secs()
        ));
    }
    let stderr = output.stderr.trim();
    match output.exit_code {
        Some(0) => HookOutcome::Pass,
        Some(2) if stderr.is_empty() => HookOutcome::Block(format!("hook `{}` exited with code 2", hook.command)),
        Some(2) => HookOutcome::Block(stderr.to_string()),
        code => {
            let code = code.map_or("no exit code".to_string(), |c| format!("code {c}"));
            let said = if stderr.is_empty() { String::new() } else { format!(": {stderr}") };
            HookOutcome::Warn(format!("hook `{}` failed with {code}{said}", hook.command))
        }
    }
}

/// Every hook of one event, taken together.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HookVerdict {
    /// The reasons of every hook that blocked, joined — `None` when none did.
    pub blocked: Option<String>,
    pub warnings: Vec<String>,
}

impl HookVerdict {
    pub fn from_outcomes(outcomes: impl IntoIterator<Item = HookOutcome>) -> Self {
        let mut reasons = Vec::new();
        let mut warnings = Vec::new();
        for outcome in outcomes {
            match outcome {
                HookOutcome::Pass => {}
                HookOutcome::Block(reason) => reasons.push(reason),
                HookOutcome::Warn(warning) => warnings.push(warning),
            }
        }
        Self { blocked: (!reasons.is_empty()).then(|| reasons.join("\n")), warnings }
    }
}

/// Runs one hook's command with the event on stdin, in the workspace — the
/// process runner in the app, a script in tests.
pub type RunHook =
    std::sync::Arc<dyn Fn(&HookCommand, &str, &std::path::Path) -> Result<CommandOutput, CommandError> + Send + Sync>;

/// The configured hooks and the way to run them, for one turn.
#[derive(Clone, Default)]
pub struct Hooks {
    config: HooksConfig,
    run: Option<RunHook>,
}

impl Hooks {
    pub fn new(config: HooksConfig, run: RunHook) -> Self {
        Self { config, run: Some(run) }
    }

    /// Runs every hook of `event` that matches `tool`, one after another in
    /// the file's order, and says what they decided. `fields` is the
    /// event's own part of the input; the event name and `cwd` are added.
    pub fn fire(&self, event: HookEvent, tool: Option<&str>, fields: Value, cwd: &std::path::Path) -> HookVerdict {
        let Some(run) = &self.run else { return HookVerdict::default() };
        let hooks = matching(&self.config, event, tool);
        if hooks.is_empty() {
            return HookVerdict::default();
        }
        let mut input = match fields {
            Value::Object(map) => map,
            _ => Map::new(),
        };
        input.insert("hook_event_name".into(), event.name().into());
        input.insert("cwd".into(), cwd.display().to_string().into());
        let input = Value::Object(input).to_string();
        HookVerdict::from_outcomes(hooks.into_iter().map(|hook| outcome(hook, run(hook, &input, cwd))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn config(value: Value) -> HooksConfig {
        serde_json::from_value(value).unwrap()
    }

    fn commands(config: &HooksConfig, event: HookEvent, tool: Option<&str>) -> Vec<String> {
        matching(config, event, tool).into_iter().map(|h| h.command.clone()).collect()
    }

    /// Claude Code's own example shape.
    #[test]
    fn a_claude_code_configuration_parses_as_is() {
        let config = parse(
            r#"{"hooks": {"PostToolUse": [{"matcher": "editFile|writeFile",
                "hooks": [{"type": "command", "command": "prettier --write", "timeout": 30}]}]}}"#,
        )
        .unwrap();
        let hook = &config.hooks["PostToolUse"][0].hooks[0];
        assert_eq!((hook.command.as_str(), hook.timeout_secs()), ("prettier --write", 30));
        assert!(parse("").unwrap().hooks.is_empty());
        assert!(matches!(parse("{"), Err(HooksConfigError::Parse(_))));
    }

    #[test]
    fn what_this_app_does_not_run_survives_a_save() {
        let text = r#"{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"x"}]}],
            "Stop":[{"hooks":[{"type":"agent","prompt":"check","command":"echo"}]}]},"disableAllHooks":false}"#;
        let config = parse(text).unwrap();
        let again: HooksConfig = serde_json::from_value(serde_json::to_value(&config).unwrap()).unwrap();
        assert_eq!(again, config);
        assert_eq!(config.extra["disableAllHooks"], false);
        assert_eq!(config.hooks["Stop"][0].hooks[0].extra["prompt"], "check");
        assert!(matching(&config, HookEvent::Stop, None).is_empty(), "an agent hook does not run here, whatever else it carries");
    }

    #[test]
    fn a_matcher_is_a_whole_name_expression() {
        let config = config(json!({"hooks": {"PreToolUse": [
            {"matcher": "editFile|writeFile", "hooks": [{"type": "command", "command": "edits"}]},
            {"matcher": "run", "hooks": [{"type": "command", "command": "prefix"}]},
            {"matcher": "mcp__.*", "hooks": [{"type": "command", "command": "mcp"}]},
            {"matcher": "", "hooks": [{"type": "command", "command": "all"}]},
            {"matcher": "*", "hooks": [{"type": "command", "command": "star"}]},
            {"matcher": "(", "hooks": [{"type": "command", "command": "broken"}]},
        ]}}));
        assert_eq!(commands(&config, HookEvent::PreToolUse, Some("editFile")), ["edits", "all", "star"]);
        assert_eq!(commands(&config, HookEvent::PreToolUse, Some("runCommand")), ["all", "star"], "not a prefix match");
        assert_eq!(commands(&config, HookEvent::PreToolUse, Some("mcp__gh__issue")), ["mcp", "all", "star"]);
        assert!(commands(&config, HookEvent::PostToolUse, Some("editFile")).is_empty(), "another event");
    }

    /// A guard on commands also guards the line typed into the terminal —
    /// once per group, even when both names match — and not the other way.
    #[test]
    fn a_command_hook_meets_a_line_for_the_terminal() {
        let config = config(json!({"hooks": {"PreToolUse": [
            {"matcher": "runCommand", "hooks": [{"type": "command", "command": "guard"}]},
            {"matcher": "runCommand|runInTerminal", "hooks": [{"type": "command", "command": "both"}]},
            {"matcher": "runInTerminal", "hooks": [{"type": "command", "command": "terminal"}]},
            {"matcher": "editFile", "hooks": [{"type": "command", "command": "edits"}]},
        ]}}));
        assert_eq!(commands(&config, HookEvent::PreToolUse, Some("runInTerminal")), ["guard", "both", "terminal"]);
        assert_eq!(commands(&config, HookEvent::PreToolUse, Some("runCommand")), ["guard", "both"]);
        assert_eq!(commands(&config, HookEvent::PreToolUse, Some("readTerminal")), Vec::<String>::new());
    }

    #[test]
    fn a_stop_hook_ignores_its_matcher() {
        let config = config(json!({"hooks": {"Stop": [{"matcher": "nothing", "hooks": [{"type": "command", "command": "done"}]}]}}));
        assert_eq!(commands(&config, HookEvent::Stop, None), ["done"]);
    }

    fn ran(code: Option<i32>, stderr: &str, timed_out: bool) -> Result<CommandOutput, CommandError> {
        Ok(CommandOutput { stdout: "ignored".into(), stderr: stderr.into(), exit_code: code, timed_out, stopped: false, truncated: false, duration_ms: 0, full_output: None })
    }

    #[test]
    fn exit_two_blocks_with_stderr_and_other_failures_only_warn() {
        let hook = HookCommand { kind: "command".into(), command: "guard".into(), ..Default::default() };
        assert_eq!(outcome(&hook, ran(Some(0), "chatter", false)), HookOutcome::Pass);
        assert_eq!(outcome(&hook, ran(Some(2), " no prod \n", false)), HookOutcome::Block("no prod".into()));
        assert_eq!(outcome(&hook, ran(Some(2), "", false)), HookOutcome::Block("hook `guard` exited with code 2".into()));
        assert_eq!(outcome(&hook, ran(Some(1), "oops", false)), HookOutcome::Warn("hook `guard` failed with code 1: oops".into()));
        assert_eq!(outcome(&hook, ran(None, "", false)), HookOutcome::Warn("hook `guard` failed with no exit code".into()));
        assert_eq!(
            outcome(&hook, ran(Some(2), "late", true)),
            HookOutcome::Warn("hook `guard` did not finish within 60 s and was stopped".into()),
            "a hook stopped by its timeout does not block"
        );
        assert!(matches!(outcome(&hook, Err(CommandError::NotStarted("nope".into()))), HookOutcome::Warn(w) if w.contains("did not run")));
    }

    #[test]
    fn every_block_is_explained_together() {
        let verdict = HookVerdict::from_outcomes([
            HookOutcome::Block("a".into()),
            HookOutcome::Pass,
            HookOutcome::Warn("w".into()),
            HookOutcome::Block("b".into()),
        ]);
        assert_eq!(verdict, HookVerdict { blocked: Some("a\nb".into()), warnings: vec!["w".into()] });
        assert_eq!(HookVerdict::from_outcomes([HookOutcome::Pass]), HookVerdict::default());
    }

    #[test]
    fn firing_runs_the_matching_hooks_with_the_event_on_stdin() {
        use std::sync::{Arc, Mutex};
        let seen: Arc<Mutex<Vec<(String, Value)>>> = Arc::default();
        let log = Arc::clone(&seen);
        let hooks = Hooks::new(
            config(json!({"hooks": {"PreToolUse": [
                {"matcher": "runCommand", "hooks": [{"type": "command", "command": "guard"}, {"type": "command", "command": "flaky"}]},
                {"matcher": "readFile", "hooks": [{"type": "command", "command": "other"}]},
            ]}})),
            Arc::new(move |hook: &HookCommand, input: &str, _: &std::path::Path| {
                log.lock().unwrap().push((hook.command.clone(), serde_json::from_str(input).unwrap()));
                let code = if hook.command == "guard" { 2 } else { 1 };
                ran(Some(code), "said", false)
            }),
        );
        let verdict = hooks.fire(HookEvent::PreToolUse, Some("runCommand"), json!({"tool_name": "runCommand"}), std::path::Path::new("/w"));
        assert_eq!(verdict.blocked.as_deref(), Some("said"));
        assert_eq!(verdict.warnings, ["hook `flaky` failed with code 1: said"]);
        let seen = seen.lock().unwrap();
        assert_eq!(seen.iter().map(|(c, _)| c.as_str()).collect::<Vec<_>>(), ["guard", "flaky"]);
        assert_eq!(seen[0].1, json!({"tool_name": "runCommand", "hook_event_name": "PreToolUse", "cwd": "/w"}));

        assert_eq!(Hooks::default().fire(HookEvent::Stop, None, json!({}), std::path::Path::new("/w")), HookVerdict::default());
    }

    #[test]
    fn the_tab_lists_every_hook_and_says_which_will_not_run() {
        let config = config(json!({"hooks": {
            "PreToolUse": [
                {"matcher": "Bash", "hooks": [{"type": "command", "command": "claude-code-style"}]},
                {"matcher": "runCommand|Bash", "hooks": [{"type": "command", "command": "both", "timeout": 5}]},
                {"matcher": "mcp__github__.*", "hooks": [{"type": "command", "command": "server"}]},
                {"matcher": "(", "hooks": [{"type": "command", "command": "typo"}]},
                {"hooks": [{"type": "prompt", "prompt": "judge"}, {"type": "command", "command": " "}]},
            ],
            "Stop": [{"matcher": "anything", "hooks": [{"type": "command", "command": "notify"}]}],
            "SessionStart": [{"hooks": [{"type": "command", "command": "hello"}]}],
        }}));
        let listed = items(&config);
        let rows: Vec<(&str, &str, Option<&str>)> =
            listed.iter().map(|i| (i.event.as_str(), i.command.as_str(), i.problem.as_deref())).collect();
        let problem = |command: &str| rows.iter().find(|r| r.1 == command).unwrap().2;

        assert_eq!(rows.len(), 8);
        assert!(problem("claude-code-style").unwrap().contains("not Bash"));
        assert_eq!(problem("both"), None);
        assert_eq!(listed.iter().find(|i| i.command == "both").unwrap().timeout_secs, 5);
        assert_eq!(problem("server"), None);
        assert!(problem("typo").unwrap().contains("not a valid regular expression"));
        assert!(problem("").unwrap().contains("\"prompt\" hooks do not run"));
        assert_eq!(problem(" "), Some("no command to run"));
        assert_eq!(problem("notify"), None, "a Stop matcher is not read");
        assert!(problem("hello").unwrap().starts_with("SessionStart hooks do not run"));
    }

    #[test]
    fn a_zero_timeout_is_the_default() {
        let hook = HookCommand { timeout: Some(0), ..Default::default() };
        assert_eq!(hook.timeout_secs(), DEFAULT_TIMEOUT_SECS);
    }
}
