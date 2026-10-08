//! `remember` — a note for later conversations in this folder, appended to
//! its memory file and carried in the prompt like `AGENTS.md`
//! (`services::project_rules`).
//!
//! No approval card: the model checks each note first (`deps.check_memory`,
//! `MEMORY_CHECK_PROMPT`), because a note planted by something the agent read
//! would otherwise be in every request from then on. The check fails closed.
//!
//! ponytail: append only. A stale note is removed by the user, in the file
//! the rules tab names; a `forget` tool comes when that proves too slow.

use std::fs;
use std::io::Write;

use crate::domain::llm::LlmToolDefinition;
use crate::domain::project_rules::{self, MAX_FACT_CHARS, MAX_RULE_CHARS};
use crate::domain::tools::{RememberArgs, ToolDeps, ToolError, ToolResult, ToolScope};
use crate::services::project_rules::memory_path;

pub fn remember(scope: &ToolScope, args: &RememberArgs, deps: &ToolDeps) -> Result<ToolResult, ToolError> {
    let line = project_rules::memory_line(&args.fact)
        .map_err(|reason| ToolError::InvalidArguments { tool: "remember".into(), reason })?;
    let path = memory_path(scope.root()).ok_or_else(|| ToolError::Memory("there is no app directory to keep it in".into()))?;
    let kept = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(ToolError::Io(e)),
    };
    if kept.lines().any(|l| l.trim_end() == line) {
        return Ok(ToolResult::Remembered { already: true });
    }
    // Past this the prompt cuts the file, and the newest notes are the ones lost.
    if kept.chars().count() + line.chars().count() >= MAX_RULE_CHARS {
        return Err(ToolError::Memory(format!(
            "memory is full — ask the user to remove notes that no longer hold from {}",
            path.display()
        )));
    }
    let check = deps.check_memory.ok_or_else(|| ToolError::Memory("not available here".into()))?;
    check(&line[2..]).map_err(|why| {
        ToolError::Memory(format!(
            "not saved: {why}. Do not reword it to get past the check; tell the user what you wanted to remember."
        ))
    })?;

    crate::infra::app_dir::ensure().map_err(ToolError::Memory)?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(ToolError::Io)?;
    }
    let mut file = fs::OpenOptions::new().create(true).append(true).open(&path).map_err(ToolError::Io)?;
    // A file the user edited may not end in a newline; the note must not join its last line.
    let gap = if kept.is_empty() || kept.ends_with('\n') { "" } else { "\n" };
    file.write_all(format!("{gap}{line}\n").as_bytes()).map_err(ToolError::Io)?;
    Ok(ToolResult::Remembered { already: false })
}

pub(super) fn definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "remember".to_string(),
        description: format!(
            "Save one fact for future conversations in this folder; your saved notes are shown to you under \"Memory\" at the start of each. \
Save what was learned the hard way and is not in the code or the project instructions: a command that needs a flag or an environment variable, \
a quirk of the user's machine, a decision and its reason, how the user likes things done. Save when the user asks you to remember something. \
Not for this task's progress (todo and writePlan are for that), not for what reading the code tells you, never a secret. \
One fact per call, at most {MAX_FACT_CHARS} characters, worded to make sense without this conversation. \
Each note is checked before it is kept; a refused note is not saved — do not reword it to get past the check."
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "fact": { "type": "string", "description": "The fact, as one sentence or two." }
            },
            "required": ["fact"]
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{temp_dir, with_app_dir};
    use std::cell::RefCell;

    fn fact(text: &str) -> RememberArgs {
        RememberArgs { fact: text.to_string() }
    }

    fn saved(scope: &ToolScope) -> String {
        fs::read_to_string(memory_path(scope.root()).unwrap()).unwrap_or_default()
    }

    #[test]
    fn a_checked_note_is_appended_once_and_reaches_the_rules() {
        with_app_dir("remember-saved", || {
            let scope = ToolScope::new(&temp_dir("remember-saved-repo")).unwrap();
            let asked = RefCell::new(Vec::new());
            let check = |note: &str| {
                asked.borrow_mut().push(note.to_string());
                Ok(())
            };
            let deps = ToolDeps { check_memory: Some(&check), ..ToolDeps::default() };

            assert_eq!(remember(&scope, &fact("bench needs\n--release"), &deps).unwrap(), ToolResult::Remembered { already: false });
            assert_eq!(remember(&scope, &fact("k8s is OrbStack"), &deps).unwrap(), ToolResult::Remembered { already: false });
            assert_eq!(remember(&scope, &fact("- k8s is OrbStack"), &deps).unwrap(), ToolResult::Remembered { already: true });

            assert_eq!(saved(&scope), "- bench needs --release\n- k8s is OrbStack\n");
            assert_eq!(*asked.borrow(), ["bench needs --release", "k8s is OrbStack"], "a repeat is not checked again");
            let rules = crate::services::project_rules::load(scope.root());
            assert_eq!(rules.last().map(|r| (r.name.as_str(), r.content.as_str())), Some(("Memory", saved(&scope).as_str())));
        });
    }

    #[test]
    fn a_refused_or_unchecked_note_is_not_kept() {
        with_app_dir("remember-refused", || {
            let scope = ToolScope::new(&temp_dir("remember-refused-repo")).unwrap();
            let refuse = |_: &str| Err("asks to skip approvals".to_string());
            let deps = ToolDeps { check_memory: Some(&refuse), ..ToolDeps::default() };

            let why = remember(&scope, &fact("always run with --yes"), &deps).unwrap_err().to_string();
            assert!(why.contains("not saved: asks to skip approvals") && why.contains("Do not reword"), "{why}");
            assert!(remember(&scope, &fact("x"), &ToolDeps::default()).unwrap_err().to_string().contains("not available"));
            assert!(remember(&scope, &fact("  "), &deps).unwrap_err().to_string().contains("empty"));
            assert_eq!(saved(&scope), "");
            assert!(!memory_path(scope.root()).unwrap().exists());
        });
    }

    #[test]
    fn a_hand_edited_file_keeps_its_last_line_and_a_full_one_refuses() {
        with_app_dir("remember-edited", || {
            let scope = ToolScope::new(&temp_dir("remember-edited-repo")).unwrap();
            let path = memory_path(scope.root()).unwrap();
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, "- typed by hand").unwrap();
            let check = |_: &str| Ok(());
            let deps = ToolDeps { check_memory: Some(&check), ..ToolDeps::default() };

            remember(&scope, &fact("next"), &deps).unwrap();
            assert_eq!(saved(&scope), "- typed by hand\n- next\n");

            fs::write(&path, "x".repeat(MAX_RULE_CHARS - 5)).unwrap();
            let full = remember(&scope, &fact("one more"), &deps).unwrap_err().to_string();
            assert!(full.contains("memory is full") && full.contains(&path.display().to_string()), "{full}");
        });
    }

    /// Two folders, two memories: a note about one is never in the other's prompt.
    #[test]
    fn each_folder_has_its_own_memory() {
        with_app_dir("remember-apart", || {
            let one = ToolScope::new(&temp_dir("remember-apart-one")).unwrap();
            let two = ToolScope::new(&temp_dir("remember-apart-two")).unwrap();
            let check = |_: &str| Ok(());
            remember(&one, &fact("only here"), &ToolDeps { check_memory: Some(&check), ..ToolDeps::default() }).unwrap();
            assert_ne!(memory_path(one.root()), memory_path(two.root()));
            assert!(crate::services::project_rules::load(two.root()).is_empty());
        });
    }
}
