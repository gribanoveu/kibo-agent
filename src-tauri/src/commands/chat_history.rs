//! The list of conversations, and one conversation restored.
//!
//! The window owns the transcript while a turn runs (see `commands::chat`);
//! this is where it puts it afterwards so a restart is not a fresh start.

use std::sync::Arc;

use serde_json::Value;
use tauri::State;

use crate::domain::chat_record::{ChatGroup, ChatModel, ChatRecord, ChatSummary, GroupList};
use crate::domain::llm::LlmMessage;
use crate::domain::tools::Task;
use crate::infra::chat_store;
use crate::services::{chat_title, llm_session};

use super::blocking;
use super::agent_state::AgentState;

/// Chats of the folder `workspace` names, newest first. `None` (Chat mode)
/// and no folder are an empty list: there is nothing to be wrong about yet,
/// and the sidebar already says so. The path is canonicalized and accepted
/// only as an open folder (F-8, invariant 1).
#[tauri::command]
pub async fn chat_list(
    workspace: Option<String>,
    state: State<'_, Arc<AgentState>>,
) -> Result<Vec<ChatSummary>, String> {
    let Some(workspace) = state.ui_workspace(&workspace)? else {
        return Ok(Vec::new());
    };
    blocking(move || chat_store::list(&workspace.display().to_string())).await
}

/// One conversation, restored. A turn that died with the app left its
/// journal: with no turn of this chat running, the journal is replayed
/// over the record and saved back at once (`services::chat_recovery`).
#[tauri::command]
pub async fn chat_load(id: String, state: State<'_, Arc<AgentState>>) -> Result<ChatRecord, String> {
    let state = Arc::clone(&state);
    blocking(move || {
        // Read here, beside the replay, not before the queue: a turn that
        // starts while this load waits must not be taken for a crash.
        let session = state.session(&id);
        let busy = session.is_running() || session.paused.load(std::sync::atomic::Ordering::SeqCst);
        crate::services::chat_recovery::load(&id, busy)
    })
    .await
}

/// The turn's transcript so far, written by the window at each round's
/// end: the blocks it draws, the checklist and plan as they stand. The
/// loop journals the history; this is the half only the window knows.
/// Best-effort — a failed write costs crash protection, never the turn.
#[tauri::command]
pub async fn chat_journal_blocks(
    id: String,
    turn_id: String,
    blocks: Value,
    todos: Vec<Task>,
    plan: Option<String>,
    from: usize,
) -> Result<(), String> {
    blocking(move || {
        let body = serde_json::json!({ "blocks": blocks, "todos": todos, "plan": plan, "from": from });
        chat_store::journal_append(&id, &turn_id, "blocks", &body, 0)
    })
    .await
}

/// Writes the conversation as it now stands. Called once a turn has ended —
/// saving mid-turn would store a transcript whose last tool call has no
/// result yet — and as a new chat's first turn starts, with its message only.
/// A save that ends the turn folds its journal away; one that lands while
/// the turn runs or pauses (the first save, a plan edited at the card)
/// leaves the journal for the turn to keep writing.
#[tauri::command]
pub async fn chat_save(
    state: State<'_, Arc<AgentState>>,
    id: String,
    messages: Vec<LlmMessage>,
    blocks: Value,
    todos: Vec<Task>,
    plan: Option<String>,
    branched_from: Option<String>,
    workspace: Option<String>,
    turn_id: Option<String>,
) -> Result<ChatSummary, String> {
    // A background chat still saves under its own checkout, even if another
    // chat has since opened a different folder in the window; a new one under
    // the folder it was typed in (`workspace`).
    let (workspace, _) = state.chat_workspace_in(&id, workspace.as_deref())?;
    let session = state.session(&id);
    let state = Arc::clone(&state);
    blocking(move || {
        // Read here, beside the write, not before the queue: the next turn
        // may start while this save waits, and its journal must survive.
        let mid_turn = session.is_running() || session.paused.load(std::sync::atomic::Ordering::SeqCst);
        let first = !chat_store::exists(&id).map_err(|e| e.to_string())?;
        let branch = crate::infra::git_head::current_branch(&workspace);
        let saved = if mid_turn {
            chat_store::save_keep_journal(
                &id,
                &workspace.display().to_string(),
                &messages,
                &blocks,
                &todos,
                plan.as_deref(),
                branched_from.as_deref(),
                // Read here, at the save, rather than sent: the window's copy of
                // HEAD lags a switch the agent made in the turn just ended.
                branch.as_deref(),
            )
        } else {
            // Fold only the turn this save ends: the next turn's base
            // snapshot, already written, is not this turn's to delete.
            chat_store::save_turn(
                &id,
                &workspace.display().to_string(),
                &messages,
                &blocks,
                &todos,
                plan.as_deref(),
                branched_from.as_deref(),
                branch.as_deref(),
                turn_id.as_deref(),
            )
        };
        let summary = saved.map_err(|e| e.to_string())?;
        // The chat speaks through the provider it was begun on: the first
        // save stamps it, later ones are a no-op. Best effort — the chat is
        // saved; an unpinned chat speaks through the active provider, as
        // every chat did before pins.
        let _ = crate::services::chat_model::stamp_if_absent(&id);
        if first {
            super::approval::remember_new_chat(&state, &id);
        }
        Ok::<_, String>(summary)
    })
    .await
}

/// Moves one saved agent chat to a clean worktree from its current branch.
/// The source checkout and every other chat are left exactly where they are.
#[tauri::command]
pub async fn chat_move_to_worktree(id: String, state: State<'_, Arc<AgentState>>) -> Result<String, String> {
    state.ensure_chat_idle(&id)?;
    let state = Arc::clone(&state);
    blocking(move || move_to_worktree(&id, &state)).await
}

fn move_to_worktree(id: &str, state: &AgentState) -> Result<String, String> {
    let record = chat_store::load(id).map_err(|e| e.to_string())?;
    if record.role.is_some() {
        return Err("only an agent chat can move to a worktree".into());
    }
    if record.worktree_base.is_some() {
        return Err("this chat is already in its own worktree".into());
    }
    state.ensure_chat_idle(id)?;
    let root = std::path::Path::new(&record.workspace).canonicalize().map_err(|e| e.to_string())?;
    let base = crate::infra::git_head::current_branch(&root)
        .ok_or_else(|| "the chat's folder is not on a git branch".to_string())?;
    let worktrees = crate::infra::app_dir::ensure()?.join("worktrees");
    let path = crate::infra::git_branches::add_agent_worktree(&root, &base, id, &worktrees).map_err(|e| e.to_string())?;
    // Store the plain spelling `chat_workspace`, cleanup and later turns
    // read it back under (no `\\?\` on Windows).
    let shown = crate::infra::paths::canonicalize_plain(&path).map_err(|e| e.to_string())?.display().to_string();
    chat_store::relocate(id, &shown, &base).map_err(|e| e.to_string())?;
    Ok(shown)
}

/// Names a new chat with the active provider's model, from its first message.
/// Called once that message is saved, beside the turn; the window re-reads the
/// list when it returns. A failure is only reported: the chat keeps the name it
/// was saved with.
#[tauri::command]
pub async fn chat_generate_title(id: String) -> Result<(), String> {
    // A request to the provider, which would freeze the IPC loop for its duration.
    blocking(move || {
        let session = llm_session::resolve_for(None, Some(&id)).map_err(|e| e.to_string())?;
        chat_title::name(&session, &id).map(|_| ()).map_err(|e| e.to_string())
    })
    .await
}

/// Writes one saved chat to a file the user picked, as Markdown. The path is
/// theirs — it comes back from the system save dialog — so nothing here
/// second-guesses where it points.
#[tauri::command]
pub async fn chat_export(id: String, path: String) -> Result<(), String> {
    blocking(move || chat_store::export(&id, std::path::Path::new(&path))).await
}

#[tauri::command]
pub async fn chat_set_archived(id: String, archived: bool) -> Result<(), String> {
    blocking(move || chat_store::set_archived(&id, archived)).await
}

/// The provider and model this chat speaks with — a pick made in its chip.
/// `model: None` is "auto": the provider's own pin decides. Like pinning a
/// chat to the sidebar, this is not conversation: `updated_at` stands.
#[tauri::command]
pub async fn chat_model_set(id: String, provider: String, model: Option<String>) -> Result<(), String> {
    blocking(move || chat_store::set_model(&id, &ChatModel { provider, model })).await
}

#[tauri::command]
pub async fn chat_set_pinned(id: String, pinned: bool) -> Result<(), String> {
    blocking(move || chat_store::set_pinned(&id, pinned)).await
}

/// The chat folders of a set — Chat mode's, or the one every project shares —
/// in the user's order.
#[tauri::command]
pub async fn chat_group_list(list: GroupList) -> Result<Vec<ChatGroup>, String> {
    blocking(move || chat_store::groups(list)).await
}

#[tauri::command]
pub async fn chat_group_create(list: GroupList, name: String) -> Result<ChatGroup, String> {
    blocking(move || chat_store::create_group(list, &name)).await
}

#[tauri::command]
pub async fn chat_group_rename(list: GroupList, id: String, name: String) -> Result<(), String> {
    blocking(move || chat_store::rename_group(list, &id, &name)).await
}

/// Removing the folder files its chats under no folder — the chats stay.
#[tauri::command]
pub async fn chat_group_delete(list: GroupList, id: String) -> Result<(), String> {
    blocking(move || chat_store::delete_group(list, &id)).await
}

/// Moves a chat into a folder, or out to no folder with `None`.
#[tauri::command]
pub async fn chat_set_group(id: String, group: Option<String>) -> Result<(), String> {
    blocking(move || chat_store::set_group(&id, group.as_deref())).await
}

#[tauri::command]
pub async fn chat_delete(
    id: String,
    state: State<'_, Arc<AgentState>>,
    processes: State<'_, Arc<crate::infra::background::Processes>>,
) -> Result<(), String> {
    let state = Arc::clone(&state);
    let processes = Arc::clone(&processes);
    blocking(move || {
        cleanup_worktree(&id, &state)?;
        crate::services::plain_chat::delete_chat(&id, &processes).map_err(|e| e.to_string())
    })
    .await
}

/// A clean, unshared worktree no folder of the window is open in (F-8) can
/// go with its chat. Anything else stays on disk: deleting a transcript
/// never deletes work.
fn cleanup_worktree(id: &str, state: &AgentState) -> Result<(), String> {
    let record = chat_store::load(id).map_err(|e| e.to_string())?;
    if record.role.is_none() {
        state.ensure_chat_idle(id)?;
    }
    if record.worktree_base.is_none() {
        return Ok(());
    }
    let Ok(root) = crate::infra::paths::canonicalize_plain(std::path::Path::new(&record.workspace)) else { return Ok(()) };
    // One plain spelling for every check: the registry, the turn's running
    // root and the chat store's keys all come from `chat_workspace`, which
    // canonicalizes plain (no `\\?\` on Windows).
    if state.is_open(&root) {
        return Ok(());
    }
    if !state.root_is_idle(&root) {
        return Ok(());
    }
    if chat_store::list(&root.display().to_string()).map_err(|e| e.to_string())?.len() != 1 {
        return Ok(());
    }
    // A dirty or locked worktree is deliberately left whole. Its transcript
    // can still be deleted without risking the files.
    let _ = crate::infra::git_branches::remove_worktree(&root);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::temp_dir;
    use std::path::{Path, PathBuf};

    fn save_agent(id: &str, workspace: &Path) {
        let shown = crate::infra::paths::canonicalize_plain(workspace).unwrap();
        chat_store::save(id, &shown.display().to_string(), &[LlmMessage::user("hi")], &serde_json::json!([]), &[], None, None, None).unwrap();
    }

    fn git_repo(label: &str) -> (PathBuf, String) {
        let root = temp_dir(label);
        let repo = git2::Repository::init(&root).unwrap();
        let mut config = repo.config().unwrap();
        config.set_bool("core.autocrlf", false).unwrap();
        config.set_str("user.name", "Test").unwrap();
        config.set_str("user.email", "test@example.com").unwrap();
        std::fs::write(root.join("a.rs"), "one\n").unwrap();
        crate::infra::git_changes::stage(&root, &["a.rs".into()]).unwrap();
        crate::infra::git_changes::commit(&root, "first").unwrap();
        let base = crate::infra::git_head::current_branch(&root).unwrap();
        (root, base)
    }

    fn moved(id: &str, root: &Path, state: &AgentState) -> PathBuf {
        save_agent(id, root);
        PathBuf::from(move_to_worktree(id, state).unwrap())
    }

    #[test]
    fn a_chat_moves_once_without_carrying_the_sources_dirty_files() {
        crate::testing::with_app_dir("chat-move-worktree", || {
            let (root, base) = git_repo("chat-move-repo");
            std::fs::write(root.join("a.rs"), "dirty source\n").unwrap();
            let state = AgentState::default();

            let path = moved("move-me", &root, &state);
            let record = chat_store::load("move-me").unwrap();
            assert_eq!(record.workspace, path.display().to_string());
            assert_eq!(record.worktree_base.as_deref(), Some(base.as_str()));
            assert_eq!(std::fs::read_to_string(root.join("a.rs")).unwrap(), "dirty source\n");
            assert_eq!(std::fs::read_to_string(path.join("a.rs")).unwrap(), "one\n");
            assert!(move_to_worktree("move-me", &state).unwrap_err().contains("already"));
        });
    }

    #[test]
    fn deleting_a_transcript_keeps_every_worktree_that_could_still_be_needed() {
        crate::testing::with_app_dir("chat-delete-worktrees", || {
            let (root, _) = git_repo("chat-delete-repo");
            let state = AgentState::default();
            let dirty = moved("dirty", &root, &state);
            let shared = moved("shared", &root, &state);
            let open = moved("open", &root, &state);
            let running = moved("running", &root, &state);
            let clean = moved("clean", &root, &state);
            std::fs::write(dirty.join("a.rs"), "do not delete\n").unwrap();
            save_agent("shared-too", &shared);
            state.set_workspace_for_test(crate::infra::paths::canonicalize_plain(&open).unwrap());
            let guard = state.begin_running("running", 3, &crate::infra::paths::canonicalize_plain(&running).unwrap()).unwrap();

            cleanup_worktree("dirty", &state).unwrap();
            cleanup_worktree("shared", &state).unwrap();
            cleanup_worktree("open", &state).unwrap();
            assert!(cleanup_worktree("running", &state).unwrap_err().contains("running"));
            for path in [&dirty, &shared, &open, &running] {
                assert!(path.exists(), "{} must stay", path.display());
            }

            drop(guard);
            state.set_workspace_for_test(temp_dir("chat-delete-elsewhere"));
            // Still open beside the active one: the registry holds it, not the active mark.
            cleanup_worktree("open", &state).unwrap();
            assert!(open.exists(), "a folder still open stays even when another is active");
            cleanup_worktree("clean", &state).unwrap();
            assert!(!clean.exists());
            assert_eq!(std::fs::read_to_string(dirty.join("a.rs")).unwrap(), "do not delete\n");
        });
    }

    /// TJ-4: the window's blocks land in the journal under the chat and turn.
    #[test]
    fn the_windows_blocks_land_in_the_journal() {
        use tauri::Manager;
        crate::testing::with_app_dir("cmd-journal-blocks", || {
            let app = tauri::test::mock_app();
            app.manage(Arc::new(AgentState::default()));
            tauri::async_runtime::block_on(chat_journal_blocks(
                "c1".into(),
                "c1/turn-1".into(),
                serde_json::json!([{ "kind": "user", "id": "user:0", "text": "go" }]),
                vec![],
                None,
                0,
            ))
            .unwrap();
            let entries = chat_store::journal_entries("c1").unwrap();
            assert_eq!(entries.len(), 1);
            assert!(matches!(entries[0], crate::domain::turn_journal::JournalEntry::Blocks { .. }));
        });
    }

    /// A running turn's journal is its own: the load leaves it for the turn,
    /// and the record opens as saved.
    #[test]
    fn a_running_chats_journal_is_left_for_its_turn() {
        use tauri::Manager;
        crate::testing::with_app_dir("cmd-journal-busy", || {
            let app = tauri::test::mock_app();
            let state = Arc::new(AgentState::default());
            app.manage(Arc::clone(&state));
            let root = crate::infra::paths::canonicalize_plain(&temp_dir("cmd-journal-busy-root")).unwrap();
            state.register_open(root.clone());
            chat_store::save("c1", &root.display().to_string(), &[LlmMessage::user("go")], &serde_json::json!([]), &[], None, None, None).unwrap();
            chat_store::journal_append("c1", "c1/turn-1", "append", &serde_json::to_value(vec![LlmMessage::assistant("working")]).unwrap(), 1).unwrap();
            let _running = state.begin_running("c1", 3, &root).unwrap();

            let record = tauri::async_runtime::block_on(chat_load("c1".into(), app.state())).unwrap();
            assert_eq!(record.messages, vec![LlmMessage::user("go")]);
            assert_eq!(chat_store::journal_entries("c1").unwrap().len(), 1, "the journal stays");
        });
    }

    /// TJ-4: a save while the turn pauses for approval (a plan edited at
    /// the card) keeps the journal — the resume counts its tail from it,
    /// and folding it here would duplicate the history on a later crash.
    #[test]
    fn a_save_during_a_pause_keeps_the_journal() {
        use tauri::Manager;
        crate::testing::with_app_dir("cmd-journal-pause-save", || {
            let app = tauri::test::mock_app();
            let state = Arc::new(AgentState::default());
            app.manage(Arc::clone(&state));
            let root = crate::infra::paths::canonicalize_plain(&temp_dir("cmd-journal-pause-save-root")).unwrap();
            state.register_open(root.clone());
            state.set_workspace_for_test(root.clone());
            chat_store::save("c1", &root.display().to_string(), &[LlmMessage::user("go")], &serde_json::json!([]), &[], None, None, None).unwrap();
            chat_store::journal_append("c1", "c1/turn-1", "snapshot", &serde_json::to_value(vec![LlmMessage::user("go")]).unwrap(), 1).unwrap();
            state.session("c1").paused.store(true, std::sync::atomic::Ordering::SeqCst);

            tauri::async_runtime::block_on(chat_save(
                app.state(),
                "c1".into(),
                vec![LlmMessage::user("go")],
                serde_json::json!([]),
                vec![],
                Some("# Edited at the card".into()),
                None,
                None,
                None,
            ))
            .unwrap();
            assert_eq!(chat_store::journal_entries("c1").unwrap().len(), 1, "the pause's journal stays");
            assert_eq!(chat_store::load("c1").unwrap().plan.as_deref(), Some("# Edited at the card"));

            // The turn ends: the next save folds it.
            state.session("c1").paused.store(false, std::sync::atomic::Ordering::SeqCst);
            tauri::async_runtime::block_on(chat_save(
                app.state(),
                "c1".into(),
                vec![LlmMessage::user("go")],
                serde_json::json!([]),
                vec![],
                None,
                None,
                None,
                Some("c1/turn-1".into()),
            ))
            .unwrap();
            assert!(chat_store::journal_entries("c1").unwrap().is_empty());
        });
    }

    /// The race the whole-feature review found: the queue sends the next
    /// turn without waiting for the previous turn's save, so the next
    /// turn's base snapshot may already be on disk when that save's delete
    /// runs. The save folds only the turn it ends — the live one's journal
    /// survives it.
    #[test]
    fn a_save_does_not_fold_the_next_turns_journal() {
        use tauri::Manager;
        crate::testing::with_app_dir("cmd-journal-next-turn", || {
            let app = tauri::test::mock_app();
            let state = Arc::new(AgentState::default());
            app.manage(Arc::clone(&state));
            let root = crate::infra::paths::canonicalize_plain(&temp_dir("cmd-journal-next-turn-root")).unwrap();
            state.register_open(root.clone());
            state.set_workspace_for_test(root.clone());
            chat_store::save("c1", &root.display().to_string(), &[LlmMessage::user("go")], &serde_json::json!([]), &[], None, None, None).unwrap();
            let msg = serde_json::to_value(vec![LlmMessage::user("go")]).unwrap();
            chat_store::journal_append("c1", "c1/turn-1", "snapshot", &msg, 1).unwrap();
            // The next turn has already started and written its base.
            chat_store::journal_append("c1", "c1/turn-2", "snapshot", &msg, 1).unwrap();

            tauri::async_runtime::block_on(chat_save(
                app.state(),
                "c1".into(),
                vec![LlmMessage::user("go")],
                serde_json::json!([]),
                vec![],
                None,
                None,
                None,
                Some("c1/turn-1".into()),
            ))
            .unwrap();
            assert_eq!(chat_store::journal_message_count("c1", "c1/turn-1").unwrap(), 0, "the ended turn folded");
            assert_eq!(chat_store::journal_message_count("c1", "c1/turn-2").unwrap(), 1, "the live turn's base survives");
        });
    }

    /// CM-2: the first save pins the chat to the provider the conversation
    /// started on; a later save under another active provider does not move
    /// it, and a pick made in the chat stands through saves too.
    #[test]
    fn the_first_save_pins_the_chat_to_the_active_provider() {
        use tauri::Manager;
        use crate::domain::settings::{AppSettings, LlmSettings, ProviderConfig};
        crate::testing::with_app_dir("cmd-chat-model-stamp", || {
            let app = tauri::test::mock_app();
            let state = Arc::new(AgentState::default());
            app.manage(Arc::clone(&state));
            let root = crate::infra::paths::canonicalize_plain(&temp_dir("cmd-chat-model-stamp-root")).unwrap();
            state.register_open(root.clone());
            state.set_workspace_for_test(root.clone());
            let settings = |active: &str| AppSettings {
                llm: LlmSettings {
                    active_provider_id: Some(active.into()),
                    providers: ["a", "b"].iter().map(|id| ProviderConfig {
                        id: id.to_string(),
                        base_url: format!("https://{id}.example/v1"),
                        model: Some(format!("m{id}")),
                        ..Default::default()
                    }).collect(),
                    ..Default::default()
                },
                ..Default::default()
            };
            crate::infra::settings_store::save(&settings("a")).unwrap();
            let save = || tauri::async_runtime::block_on(chat_save(
                app.state(), "c1".into(), vec![LlmMessage::user("go")], serde_json::json!([]), vec![], None, None, None, None,
            )).unwrap();

            save();
            assert_eq!(chat_store::model("c1").unwrap(), Some(ChatModel { provider: "a".into(), model: Some("ma".into()) }));

            crate::infra::settings_store::save(&settings("b")).unwrap();
            save();
            assert_eq!(chat_store::model("c1").unwrap().unwrap().provider, "a", "a later save does not move the pin");

            chat_store::set_model("c1", &ChatModel { provider: "b".into(), model: None }).unwrap();
            save();
            assert_eq!(chat_store::model("c1").unwrap(), Some(ChatModel { provider: "b".into(), model: None }), "a pick stands");
        });
    }

    /// CM-2: the chip's pick reaches the chat's row, auto included.
    #[test]
    fn a_pick_in_the_chip_reaches_the_chats_row() {
        crate::testing::with_app_dir("cmd-chat-model-set", || {
            chat_store::save("c1", "/repo", &[LlmMessage::user("go")], &serde_json::json!([]), &[], None, None, None).unwrap();

            tauri::async_runtime::block_on(chat_model_set("c1".into(), "local".into(), Some("llama".into()))).unwrap();
            assert_eq!(chat_store::model("c1").unwrap(), Some(ChatModel { provider: "local".into(), model: Some("llama".into()) }));

            tauri::async_runtime::block_on(chat_model_set("c1".into(), "local".into(), None)).unwrap();
            assert_eq!(chat_store::model("c1").unwrap(), Some(ChatModel { provider: "local".into(), model: None }), "auto is a pin");

            assert!(tauri::async_runtime::block_on(chat_model_set("nope".into(), "local".into(), None)).is_err());
        });
    }

    /// Naming a chat is one of its model calls, counted under it.
    #[test]
    fn a_chats_title_counts_under_it() {
        crate::testing::with_app_dir("chat-title-spent", || {
            crate::testing::spending_provider("Fix the parser", 20, 3);
            let blocks = serde_json::json!([{ "kind": "user", "id": "user:0", "text": "fix the parser" }]);
            chat_store::save("t1", "/repo", &[LlmMessage::user("fix the parser")], &blocks, &[], None, None, None).unwrap();
            tauri::async_runtime::block_on(chat_generate_title("t1".into())).unwrap();
            assert_eq!(crate::testing::chat_tokens("t1"), 23);
        });
    }
}
