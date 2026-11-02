//! The list of conversations, and one conversation restored.
//!
//! The window owns the transcript while a turn runs (see `commands::chat`);
//! this is where it puts it afterwards so a restart is not a fresh start.

use std::sync::Arc;

use serde_json::Value;
use tauri::State;

use crate::domain::chat_record::{ChatRecord, ChatSummary};
use crate::domain::llm::LlmMessage;
use crate::domain::tools::Task;
use crate::infra::chat_store;
use crate::services::{chat_title, llm_session};

use super::blocking;
use super::chat::AgentState;

/// Chats of the open folder, newest first. No folder open is an empty list:
/// there is nothing to be wrong about yet, and the sidebar already says so.
#[tauri::command]
pub async fn chat_list(state: State<'_, Arc<AgentState>>) -> Result<Vec<ChatSummary>, String> {
    let Ok(workspace) = state.workspace() else {
        return Ok(Vec::new());
    };
    blocking(move || chat_store::list(&workspace.display().to_string())).await
}

#[tauri::command]
pub async fn chat_load(id: String) -> Result<ChatRecord, String> {
    blocking(move || chat_store::load(&id)).await
}

/// Writes the conversation as it now stands. Called once a turn has ended —
/// saving mid-turn would store a transcript whose last tool call has no
/// result yet — and as a new chat's first turn starts, with its message only.
#[tauri::command]
pub async fn chat_save(
    state: State<'_, Arc<AgentState>>,
    id: String,
    messages: Vec<LlmMessage>,
    blocks: Value,
    todos: Vec<Task>,
    plan: Option<String>,
    branched_from: Option<String>,
) -> Result<ChatSummary, String> {
    // A background chat still saves under its own checkout, even if another
    // chat has since opened a different folder in the window.
    let (workspace, _) = state.chat_workspace(&id)?;
    let state = Arc::clone(&state);
    blocking(move || {
        let first = !chat_store::exists(&id).map_err(|e| e.to_string())?;
        let summary = chat_store::save(
            &id,
            &workspace.display().to_string(),
            &messages,
            &blocks,
            &todos,
            plan.as_deref(),
            branched_from.as_deref(),
            // Read here, at the save, rather than sent: the window's copy of
            // HEAD lags a switch the agent made in the turn just ended.
            crate::infra::git_head::current_branch(&workspace).as_deref(),
        )
        .map_err(|e| e.to_string())?;
        if first {
            super::chat::remember_new_chat(&state, &id);
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
    // Store the same canonical spelling cleanup and later turns compare against.
    let shown = path.canonicalize().map_err(|e| e.to_string())?.display().to_string();
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

/// A clean, unshared, closed worktree made for this chat can go with it.
/// Anything else stays on disk: deleting a transcript never deletes work.
fn cleanup_worktree(id: &str, state: &AgentState) -> Result<(), String> {
    let record = chat_store::load(id).map_err(|e| e.to_string())?;
    if record.role.is_none() {
        state.ensure_chat_idle(id)?;
    }
    if record.worktree_base.is_none() {
        return Ok(());
    }
    let Ok(root) = std::path::Path::new(&record.workspace).canonicalize() else { return Ok(()) };
    if state.workspace().ok().and_then(|open| open.canonicalize().ok()).as_deref() == Some(root.as_path()) {
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
        chat_store::save(id, &workspace.display().to_string(), &[LlmMessage::user("hi")], &serde_json::json!([]), &[], None, None, None).unwrap();
    }

    fn git_repo(label: &str) -> (PathBuf, String) {
        let root = temp_dir(label);
        git2::Repository::init(&root).unwrap();
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
            state.set_workspace_for_test(open.clone());
            let guard = state.begin_running("running", 3, &running.canonicalize().unwrap()).unwrap();

            cleanup_worktree("dirty", &state).unwrap();
            cleanup_worktree("shared", &state).unwrap();
            cleanup_worktree("open", &state).unwrap();
            assert!(cleanup_worktree("running", &state).unwrap_err().contains("running"));
            for path in [&dirty, &shared, &open, &running] {
                assert!(path.exists(), "{} must stay", path.display());
            }

            drop(guard);
            state.set_workspace_for_test(temp_dir("chat-delete-elsewhere"));
            cleanup_worktree("clean", &state).unwrap();
            assert!(!clean.exists());
            assert_eq!(std::fs::read_to_string(dirty.join("a.rs")).unwrap(), "do not delete\n");
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
