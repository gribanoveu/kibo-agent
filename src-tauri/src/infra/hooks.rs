//! `<app dir>/hooks.json`, and running one hook.
//!
//! Global only: a hook from a repository would be a stranger's command run on
//! every tool call of whoever opened it. Project hooks need a "trust this
//! folder" step first, which is its own feature.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::domain::command_exec::{CommandError, CommandOutput, CommandRequest};
use crate::domain::hooks::{self, HookCommand, HookGroup, HooksConfig, HooksConfigError};
use crate::infra::{app_dir, process_runner};

const FILE: &str = "hooks.json";

/// What an empty editor starts from.
pub const TEMPLATE: &str = "{\n  \"hooks\": {}\n}\n";

pub fn path() -> Result<PathBuf, HooksConfigError> {
    Ok(app_dir::dir().map_err(HooksConfigError::Read)?.join(FILE))
}

/// The file's text, or the template while there is none.
pub fn read_text() -> Result<String, HooksConfigError> {
    match fs::read_to_string(path()?) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(TEMPLATE.to_string()),
        Err(e) => Err(HooksConfigError::Read(e.to_string())),
    }
}

/// No file is no hooks.
pub fn load() -> Result<HooksConfig, HooksConfigError> {
    hooks::parse(&read_text()?)
}

/// Written as typed once it parses; text that does not parse leaves the file
/// as it was. Owner-only like the rest of the app directory: what it holds
/// runs on every tool call.
pub fn save_text(text: &str) -> Result<HooksConfig, HooksConfigError> {
    let config = hooks::parse(text)?;
    app_dir::write_private(&path()?, text.as_bytes()).map_err(HooksConfigError::Write)?;
    Ok(config)
}

/// `groups` added after what each event already has — by the catalog.
/// Rewrites the file from the parsed config, so its layout is not kept.
pub fn add_groups(groups: &BTreeMap<String, Vec<HookGroup>>) -> Result<HooksConfig, HooksConfigError> {
    rewrite(|config| {
        for (event, added) in groups {
            config.hooks.entry(event.clone()).or_default().extend(added.iter().cloned());
        }
    })
}

/// Each of `groups` taken out where an equal one is still there, once; one
/// edited since is not equal and stays. An event left with no group goes.
pub fn remove_groups(groups: &BTreeMap<String, Vec<HookGroup>>) -> Result<HooksConfig, HooksConfigError> {
    rewrite(|config| {
        for (event, removed) in groups {
            let Some(there) = config.hooks.get_mut(event) else { continue };
            for group in removed {
                if let Some(at) = there.iter().position(|g| g == group) {
                    there.remove(at);
                }
            }
            if there.is_empty() {
                config.hooks.remove(event);
            }
        }
    })
}

fn rewrite(edit: impl FnOnce(&mut HooksConfig)) -> Result<HooksConfig, HooksConfigError> {
    let mut config = load()?;
    edit(&mut config);
    let text = serde_json::to_string_pretty(&config).map_err(|e| HooksConfigError::Write(e.to_string()))?;
    app_dir::write_private(&path()?, format!("{text}\n").as_bytes()).map_err(HooksConfigError::Write)?;
    Ok(config)
}

/// Through `sh -c`, as Claude Code runs a hook — on Windows the shell picked
/// in Settings, as the agent's commands are — in the workspace, with
/// `CLAUDE_PROJECT_DIR` set to it — the variable hooks written for Claude
/// Code use to find their own scripts.
pub fn run(hook: &HookCommand, input: &str, cwd: &Path) -> Result<CommandOutput, CommandError> {
    let request = CommandRequest {
        command: hook.command.clone(),
        cwd: None,
        timeout_seconds: Some(hook.timeout_secs()),
        background: None,
    };
    let project = cwd.display().to_string();
    process_runner::run_with(&super::shells::for_commands(), &request, cwd, None, Some(input), &[("CLAUDE_PROJECT_DIR", &project)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::hooks::{outcome, HookOutcome};
    use crate::testing::{temp_dir, with_app_dir};

    #[test]
    fn groups_are_added_after_and_taken_out_only_while_unchanged() {
        with_app_dir("hooks-groups", || {
            let group = |command: &str| HookGroup { matcher: "runCommand".into(), hooks: vec![HookCommand { kind: "command".into(), command: command.into(), ..Default::default() }] };
            save_text(r#"{"hooks":{"PreToolUse":[{"matcher":"runCommand","hooks":[{"type":"command","command":"mine"}]}]},"other":1}"#).unwrap();
            let added = BTreeMap::from([("PreToolUse".to_string(), vec![group("a"), group("b")]), ("Stop".to_string(), vec![group("c")])]);
            add_groups(&added).unwrap();
            let config = load().unwrap();
            assert_eq!(config.hooks["PreToolUse"], [group("mine"), group("a"), group("b")]);
            assert_eq!(config.extra["other"], 1, "the rest of the file stays");

            // `b` edited here: it stays, the rest goes, and so does an event left empty.
            let text = read_text().unwrap().replace("\"b\"", "\"b2\"");
            save_text(&text).unwrap();
            remove_groups(&added).unwrap();
            let config = load().unwrap();
            assert_eq!(config.hooks["PreToolUse"], [group("mine"), group("b2")]);
            assert!(!config.hooks.contains_key("Stop"));
        });
    }

    #[test]
    fn a_group_written_twice_comes_out_once_per_record() {
        with_app_dir("hooks-groups-twice", || {
            let group = HookGroup { hooks: vec![HookCommand { kind: "command".into(), command: "x".into(), ..Default::default() }], ..Default::default() };
            let added = BTreeMap::from([("Stop".to_string(), vec![group.clone()])]);
            add_groups(&added).unwrap();
            add_groups(&added).unwrap();
            remove_groups(&added).unwrap();
            assert_eq!(load().unwrap().hooks["Stop"], [group]);
        });
    }

    #[test]
    fn no_file_is_no_hooks_and_a_broken_one_says_so() {
        with_app_dir("hooks-config", || {
            assert!(load().unwrap().hooks.is_empty());
            fs::write(path().unwrap(), "{").unwrap();
            assert!(matches!(load(), Err(HooksConfigError::Parse(_))));
            fs::write(path().unwrap(), r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"x"}]}]}}"#).unwrap();
            assert_eq!(load().unwrap().hooks["Stop"][0].hooks[0].command, "x");
        });
    }

    #[test]
    fn a_save_keeps_the_text_and_a_broken_one_is_refused() {
        with_app_dir("hooks-save", || {
            assert_eq!(read_text().unwrap(), TEMPLATE);
            let text = "{\"hooks\":{\"Stop\":[{\"hooks\":[{\"type\":\"command\",\"command\":\"say done\"}]}]}}";
            save_text(text).unwrap();
            assert_eq!(read_text().unwrap(), text);
            assert!(matches!(save_text("{"), Err(HooksConfigError::Parse(_))));
            assert_eq!(read_text().unwrap(), text);
        });
    }

    /// A guard written the Claude Code way: read the event, exit 2 with a
    /// reason.
    #[cfg(unix)]
    #[test]
    fn a_real_hook_reads_its_input_and_blocks() {
        let dir = temp_dir("hooks-run");
        let hook = HookCommand {
            kind: "command".into(),
            command: r#"grep -q '"tool_name":"runCommand"' && [ "$CLAUDE_PROJECT_DIR" = "$PWD" ] && echo 'no commands today' >&2 && exit 2; exit 0"#.into(),
            timeout: Some(10),
            ..Default::default()
        };
        let dir = dir.canonicalize().unwrap();
        let blocked = outcome(&hook, run(&hook, r#"{"tool_name":"runCommand"}"#, &dir));
        assert_eq!(blocked, HookOutcome::Block("no commands today".into()));
        assert_eq!(outcome(&hook, run(&hook, r#"{"tool_name":"readFile"}"#, &dir)), HookOutcome::Pass);
    }

    /// On Windows a hook runs in the shell picked in Settings: this one is
    /// PowerShell and nothing else.
    #[cfg(windows)]
    #[test]
    fn a_hook_runs_in_the_shell_picked() {
        with_app_dir("hooks-shell", || {
            let mut settings = crate::infra::settings_store::load().unwrap();
            settings.shell = Some(crate::domain::command_exec::ShellKind::WindowsPowershell);
            crate::infra::settings_store::save(&settings).unwrap();
            let hook = HookCommand {
                kind: "command".into(),
                command: "[Console]::Error.WriteLine('from powershell'); exit 2".into(),
                timeout: Some(30),
                ..Default::default()
            };
            let dir = temp_dir("hooks-shell-run");
            assert_eq!(outcome(&hook, run(&hook, "{}", &dir)), HookOutcome::Block("from powershell".into()));
        });
    }
}
