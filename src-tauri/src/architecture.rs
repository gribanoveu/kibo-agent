//! Guards for the layering in `AGENTS.md` ("Architecture", "Errors", "IPC
//! conventions"): which layer may name what, and that the window's side of
//! the IPC is in step with this one.
//!
//! Tests over the source tree, as in `data_policy`, because what has to be
//! caught is the *next* change. What breaks a rule today is listed beside it,
//! and the list only shrinks: an entry that no longer matches anything fails
//! too, so fixing an exception means deleting its line, and a new one means
//! arguing for it here (`docs/34-architecture-audit.md`).

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use crate::data_policy::{crate_dir, files};

struct Rule {
    /// Under `src/`.
    dir: &'static str,
    needles: &'static [&'static str],
    /// A line holding one of these is a permitted use the needles also match.
    except: &'static [&'static str],
    /// `(file under src/, needle)` pairs that break the rule today.
    known: &'static [(&'static str, &'static str)],
    why: &'static str,
}

const RULES: &[Rule] = &[
    Rule {
        dir: "domain",
        needles: &[
            "crate::infra", "crate::services", "crate::commands", "tauri::", "git2::", "ureq", "rusqlite",
            "dunce::", "std::fs", "std::process",
        ],
        except: &[],
        known: &[
            // `ToolScope::new` canonicalizes its root: to `services/ai_tools/resolve` (#8).
            ("domain/tools.rs", "dunce::"),
            // The `Child` in the processes' trait (#8).
            ("domain/background.rs", "std::process"),
        ],
        why: "domain/ is types and rules — no I/O, no framework types, nothing from the layers around it",
    },
    Rule {
        dir: "services",
        needles: &["crate::commands", "AppHandle", "Emitter", "tauri::"],
        except: &["tauri::async_runtime"],
        known: &[],
        why: "services/ report through a sink; of Tauri they use only its runtime",
    },
    Rule {
        dir: "services",
        needles: &["git2::"],
        except: &[],
        // The tools are I/O adapters (`docs/03-architecture.md` §3.5) — an
        // exception to write down rather than a move (#9).
        known: &[("services/ai_tools/tools/git.rs", "git2::")],
        why: "git2 belongs in infra/",
    },
    Rule {
        dir: "infra",
        needles: &["crate::services", "crate::commands", "AppHandle", "Emitter"],
        except: &[],
        known: &[],
        why: "infra/ implements what domain/ defines and knows nothing above it",
    },
];

/// `Result<…, String>` outside `commands/`: errors as data are `thiserror`
/// enums, flattened to a string only at the IPC boundary. Only goes down —
/// when it does, lower it here (#7).
const STRING_ERRORS: usize = 80;

/// The lines of `path` that are code: no comments, no `#[cfg(test)]` items.
///
/// ponytail: braces are counted without reading string literals; a test
/// module with an unbalanced `"{"` in it would cut the file short. Use `syn`
/// if that ever happens.
fn code_lines(path: &Path) -> Vec<String> {
    let text = fs::read_to_string(path).unwrap();
    let mut lines = Vec::new();
    let mut after_cfg_test = false;
    let mut depth = 0usize;
    for line in text.lines().map(str::trim) {
        if depth > 0 {
            depth = (depth + line.matches('{').count()).saturating_sub(line.matches('}').count());
            continue;
        }
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        if line == "#[cfg(test)]" {
            after_cfg_test = true;
            continue;
        }
        if after_cfg_test {
            after_cfg_test = false;
            depth = line.matches('{').count().saturating_sub(line.matches('}').count());
            continue;
        }
        lines.push(line.to_string());
    }
    lines
}

/// Every `.rs` file under `src/<dir>`, as its path under `src/` and its code.
fn sources(dir: &str) -> Vec<(String, Vec<String>)> {
    let src = crate_dir().join("src");
    let mut paths = Vec::new();
    files(&src.join(dir), &["rs"], &mut paths);
    paths
        .into_iter()
        .map(|path| {
            let relative = path.strip_prefix(&src).unwrap().to_string_lossy().replace('\\', "/");
            let code = code_lines(&path);
            (relative, code)
        })
        .collect()
}

#[test]
fn each_layer_names_only_what_it_may() {
    let mut failures = Vec::new();
    for rule in RULES {
        let mut seen = HashSet::new();
        for (file, code) in sources(rule.dir) {
            for line in &code {
                if rule.except.iter().any(|e| line.contains(e)) {
                    continue;
                }
                for &needle in rule.needles.iter().filter(|n| line.contains(**n)) {
                    if rule.known.contains(&(file.as_str(), needle)) {
                        seen.insert((file.clone(), needle));
                    } else {
                        failures.push(format!("{file}: `{needle}` — {}:\n    {line}", rule.why));
                    }
                }
            }
        }
        for &(file, needle) in rule.known {
            if !seen.contains(&(file.to_string(), needle)) {
                failures.push(format!("{file}: `{needle}` no longer appears — delete its line from `known`"));
            }
        }
    }
    assert!(failures.is_empty(), "layering (AGENTS.md, Architecture):\n{}", failures.join("\n"));
}

#[test]
fn string_errors_outside_commands_only_decrease() {
    let count = ["domain", "services", "infra"]
        .iter()
        .flat_map(|dir| sources(dir))
        .flat_map(|(_, code)| code)
        .filter(|line| line.contains("Result<") && line.contains(", String>"))
        .count();
    assert!(
        count <= STRING_ERRORS,
        "{count} `Result<…, String>` outside commands/, was {STRING_ERRORS} — a new one is a thiserror enum in domain/"
    );
    assert!(count == STRING_ERRORS, "down to {count} from {STRING_ERRORS} — lower STRING_ERRORS to {count}");
}

struct Command {
    name: String,
    sync: bool,
    /// From the signature to the closing brace.
    body: Vec<String>,
}

/// The `#[tauri::command]` functions under `commands/`.
fn commands() -> Vec<Command> {
    let mut found = Vec::new();
    for (_, code) in sources("commands") {
        let mut lines = code.iter();
        while let Some(line) = lines.next() {
            if !line.starts_with("#[tauri::command") {
                continue;
            }
            let signature = lines.find(|l| l.contains("fn ")).expect("a command attribute with no fn after it");
            let name = signature.split("fn ").nth(1).unwrap();
            let name = name.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
            let mut body = vec![signature.clone()];
            let mut opened = signature.contains('{');
            let mut depth = signature.matches('{').count() as i64 - signature.matches('}').count() as i64;
            while !opened || depth > 0 {
                let Some(line) = lines.next() else { break };
                opened |= line.contains('{');
                depth += line.matches('{').count() as i64 - line.matches('}').count() as i64;
                body.push(line.clone());
            }
            found.push(Command { name, sync: !signature.contains("async fn "), body });
        }
    }
    assert!(found.len() > 100, "read {} commands: the attribute or the folder moved?", found.len());
    found
}

/// What a command reaches that can take long enough to freeze the window:
/// SQLite, git, the keychain, the provider, the user's files.
const SLOW: &[&str] = &[
    "chat_store::", "tool_call_log::", "daily_metrics::", "next_prompt_log::", "llm_session::resolve", "list_models",
    "llm_credentials_store::", "has_saved_key", "master_key::", "git_head::", "git2::", "rewind::", "preview::",
];

/// Sync commands that reach one of [`SLOW`] today. Only shrinks.
const SLOW_SYNC_KNOWN: &[&str] = &[];

/// A synchronous command runs on the main thread (AGENTS.md, IPC conventions).
#[test]
fn a_command_that_waits_on_io_is_async() {
    let slow: Vec<String> = commands()
        .into_iter()
        .filter(|command| command.sync && command.body.iter().any(|line| SLOW.iter().any(|s| line.contains(s))))
        .map(|command| command.name)
        .collect();
    let new: Vec<&String> = slow.iter().filter(|name| !SLOW_SYNC_KNOWN.contains(&name.as_str())).collect();
    assert!(new.is_empty(), "synchronous commands that block the window — make them async with commands::blocking: {new:?}");
    let fixed: Vec<&&str> = SLOW_SYNC_KNOWN.iter().filter(|name| !slow.iter().any(|s| s == *name)).collect();
    assert!(fixed.is_empty(), "async now or no longer slow — delete from SLOW_SYNC_KNOWN: {fixed:?}");
}

/// The window's IPC layer: every `.ts` under `src/lib`, as one text.
fn window_lib() -> String {
    let mut paths = Vec::new();
    files(&crate_dir().join("../src/lib"), &["ts"], &mut paths);
    paths.iter().map(|p| fs::read_to_string(p).unwrap()).collect::<Vec<_>>().join("\n")
}

/// A command is reachable only from `generate_handler!`, and called only
/// through its wrapper in `src/lib/` (AGENTS.md, IPC conventions).
#[test]
fn every_command_is_registered_and_wrapped() {
    let lib_rs = fs::read_to_string(crate_dir().join("src/lib.rs")).unwrap();
    let handler = lib_rs.split("generate_handler![").nth(1).and_then(|rest| rest.split(']').next()).unwrap();
    let registered: HashSet<&str> = handler.split(|c: char| !(c.is_alphanumeric() || c == '_')).collect();
    let window = window_lib();
    let failures: Vec<String> = commands()
        .into_iter()
        .flat_map(|Command { name, .. }| {
            let unregistered = (!registered.contains(name.as_str())).then(|| format!("{name}: not in generate_handler!"));
            let unwrapped = (!window.contains(&format!("\"{name}\""))).then(|| format!("{name}: no wrapper in src/lib/"));
            unregistered.into_iter().chain(unwrapped)
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Each event channel's name is a `pub const …EVENT` beside its emitter, and
/// the window's copy in `src/lib/` is a string that has to say the same.
#[test]
fn every_event_name_matches_the_window() {
    let window = window_lib();
    let names: Vec<String> = sources("commands")
        .into_iter()
        .flat_map(|(_, code)| code)
        .filter(|line| line.starts_with("pub const ") && line.contains("EVENT") && line.contains(": &str = \""))
        .filter_map(|line| line.split('"').nth(1).map(str::to_string))
        .collect();
    assert!(names.len() >= 8, "read {} event names: the constants moved?", names.len());
    let missing: Vec<&String> = names.iter().filter(|name| !window.contains(&format!("\"{name}\""))).collect();
    assert!(missing.is_empty(), "events the window does not listen for by this name: {missing:?}");
}

/// Which window may call which command, as the built app resolves it from
/// `capabilities/` and the commands `build.rs` declares: the main window all
/// of them, a Markdown viewer only its own — it shows a file and nothing more,
/// so a page it renders cannot write files or start a turn.
#[test]
fn a_viewer_window_may_call_only_its_own_commands() {
    let mut context: tauri::Context<tauri::test::MockRuntime> = tauri::generate_context!(test = true);
    let authority = context.runtime_authority_mut();
    let allowed = |command: &str, window: &str| {
        authority.resolve_access(command, window, window, &tauri::ipc::Origin::Local).is_some()
    };
    let viewer = format!("{}1", crate::commands::viewer::LABEL);
    let commands = commands();

    // Undeclared, the ACL knows no app command at all — and the runtime then
    // lets every window call every one (`webview/mod.rs`, `has_app_manifest`).
    assert!(
        commands.iter().any(|command| allowed(&command.name, "main")),
        "the app declares no commands, so every window may call every one — build.rs passes them to AppManifest"
    );
    let mut failures = Vec::new();
    for command in commands {
        let name = command.name.as_str();
        if !allowed(name, "main") {
            failures.push(format!("{name}: refused in the main window"));
        }
        if allowed(name, &viewer) != name.starts_with("viewer_") {
            failures.push(format!("{name}: {} in a viewer", if allowed(name, &viewer) { "allowed" } else { "refused" }));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// What a rewind accounts for is decided per tool on the backend
/// (`ToolName::rewind_sees`, a match the compiler holds complete); the window
/// counts the rest from its copy, `ACCOUNTED` in `src/lib/rewind.ts`. Names
/// there that are no tool any more are older chats' and stay.
#[test]
fn every_tools_rewind_flag_matches_the_window() {
    use crate::domain::tools::ToolName;
    let rewind = fs::read_to_string(crate_dir().join("../src/lib/rewind.ts")).unwrap();
    let set = rewind.split("const ACCOUNTED = new Set([").nth(1).and_then(|rest| rest.split("]);").next()).unwrap();
    let window: HashSet<&str> = code_ts(set).flat_map(|line| line.split('"').skip(1).step_by(2)).collect();
    assert!(window.len() > 10, "read {} names from ACCOUNTED: its shape changed?", window.len());
    let failures: Vec<String> = ToolName::ALL
        .iter()
        .filter(|tool| tool.rewind_sees() != window.contains(tool.wire_name()))
        .map(|tool| format!("{}: rewind_sees is {}, ACCOUNTED says {}", tool.wire_name(), tool.rewind_sees(), !tool.rewind_sees()))
        .collect();
    assert!(failures.is_empty(), "src/lib/rewind.ts is out of step with ToolName::rewind_sees:\n{}", failures.join("\n"));
}

/// TypeScript lines that are not `//` comments.
fn code_ts(text: &str) -> impl Iterator<Item = &str> {
    text.lines().map(str::trim).filter(|line| !line.starts_with("//"))
}
