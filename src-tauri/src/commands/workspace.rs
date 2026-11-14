//! The open folder: opening it, the recent list, its branch and its index.

use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::infra::background::Processes;
use crate::services::workspace_index::WorkspaceIndex;

use super::workspace_events::index_event_sink;
use super::agent_state::AgentState;

/// Opens a folder as the workspace. Canonicalized here, once, so every path a
/// tool resolves later is measured against a real directory rather than
/// against whatever spelling the caller used.
///
/// Also makes it the indexed folder. The index opening is awaited — it reads
/// what was embedded before, off the UI thread — and its first sync is not.
/// An index that cannot open does not stop the folder opening: the reason
/// arrives on `workspace-index:event`, and the agent works without search.
#[tauri::command]
pub async fn workspace_open(
    path: String,
    keep_running: Option<bool>,
    app: AppHandle,
    state: State<'_, Arc<AgentState>>,
    index: State<'_, Arc<WorkspaceIndex>>,
    git_watch: State<'_, super::git::GitWatch>,
) -> Result<String, String> {
    // Without `\\?\` on Windows: it is what the folder is known by from
    // here on — the shell started in it (`cmd.exe` refuses that prefix and
    // opens in `C:\Windows`), the tools' root, the index's events, the keys
    // chats and trust are saved under.
    let resolved = crate::infra::paths::canonicalize_plain(std::path::Path::new(&path))
        .map_err(|e| format!("cannot open {path}: {e}"))?;
    if !resolved.is_dir() {
        return Err(format!("{path} is not a folder"));
    }
    *state
        .workspace
        .lock()
        .map_err(|_| "workspace lock poisoned".to_string())? = Some(resolved.clone());
    // A chat moved to its worktree must not stop the neighbouring chat's
    // processes. Ordinary folder switches keep the old behaviour.
    if keep_running != Some(true) {
        if let Some(processes) = app.try_state::<Arc<Processes>>() {
            processes.stop_all();
        }
        if let Some(terminals) = app.try_state::<Arc<crate::infra::terminal::Terminals>>() {
            terminals.close_all();
        }
    }

    let shown = resolved.display().to_string();
    // Only a convenience for the next launch: a folder that opened stays open
    // even if the list could not be written.
    if let Err(e) = crate::infra::recent_workspaces::record(&shown) {
        eprintln!("recent folders not saved: {e}");
    }
    super::git::watch_git(&app, &git_watch, &resolved, shown.clone());
    let index = Arc::clone(&index);
    let sink = index_event_sink(&app, shown.clone());
    // Reported through the sink; see above.
    let _ = tauri::async_runtime::spawn_blocking(move || index.open(&resolved, sink)).await;
    Ok(shown)
}

/// A folder opened lately, and whose worktree it is if it is one — so the
/// folder menu can show it under the repository it belongs to.
#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentFolder {
    path: String,
    worktree_of: Option<String>,
}

/// Folders opened lately that still exist, the last one first.
#[tauri::command]
pub fn workspace_recent() -> Vec<RecentFolder> {
    recent_folders(crate::infra::recent_workspaces::load())
}

/// Drops a folder from the list the window offers at launch. Nothing on disk
/// is touched — the entry goes, the folder and its chats stay. The open folder
/// may be dropped too: it stays open, and the next launch opens another one.
#[tauri::command]
pub fn workspace_forget(path: String) -> Result<(), String> {
    crate::infra::recent_workspaces::forget(&path).map_err(|e| e.to_string())
}

fn recent_folders(paths: Vec<String>) -> Vec<RecentFolder> {
    paths
        .into_iter()
        .map(|path| {
            let worktree_of = crate::infra::git_head::worktree_of(std::path::Path::new(&path));
            RecentFolder { worktree_of: worktree_of.map(|main| main.display().to_string()), path }
        })
        .collect()
}

#[tauri::command]
pub fn workspace_current(state: State<'_, Arc<AgentState>>) -> Option<String> {
    state
        .workspace
        .lock()
        .ok()?
        .clone()
        .map(|path| path.display().to_string())
}

/// The branch checked out in the open folder; `None` when it is not a
/// repository or nothing is open.
#[tauri::command]
pub async fn workspace_branch(state: State<'_, Arc<AgentState>>) -> Result<Option<String>, String> {
    let Ok(root) = state.workspace() else { return Ok(None) };
    super::blocking(move || Ok::<_, String>(crate::infra::git_head::current_branch(&root))).await
}

/// The main working tree when the open folder is a worktree of it; `None`
/// for an ordinary folder or repository.
#[tauri::command]
pub async fn workspace_worktree_of(state: State<'_, Arc<AgentState>>) -> Result<Option<String>, String> {
    let Ok(root) = state.workspace() else { return Ok(None) };
    super::blocking(move || Ok::<_, String>(crate::infra::git_head::worktree_of(&root).map(|path| path.display().to_string())))
        .await
}

/// The open folder's index as it stands — what a window that was not
/// listening when the sync began (a reload, say) starts from before the next
/// `workspace-index:event`.
#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexSnapshot {
    /// The same string `workspace_open` returned, which the events carry too.
    root: String,
    syncing: bool,
    embedded: usize,
    skipped: usize,
    embedding_error: Option<String>,
}

#[tauri::command]
pub fn workspace_index_status(index: State<'_, Arc<WorkspaceIndex>>) -> Option<IndexSnapshot> {
    snapshot(&index)
}

fn snapshot(index: &WorkspaceIndex) -> Option<IndexSnapshot> {
    let indexer = index.current()?;
    let status = indexer.status();
    Some(IndexSnapshot {
        root: indexer.root().display().to_string(),
        syncing: status.syncing,
        embedded: status.embedded,
        skipped: status.skipped.len(),
        embedding_error: status.embedding_error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    
    use crate::testing::temp_dir;
    

    #[test]
    fn a_recent_worktree_names_the_repository_it_belongs_to() {
        let main = temp_dir("cmd-recent-main");
        let repo = git2::Repository::init(&main).unwrap();
        let tree = repo.find_tree(repo.index().unwrap().write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("T", "t@example.com").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "c", &tree, &[]).unwrap();
        let head = repo.head().unwrap().shorthand().unwrap().to_string();
        let linked =
            crate::infra::git_branches::add_worktree(&main, &head, "t", &temp_dir("cmd-recent-home")).unwrap();

        let folders = recent_folders(vec![linked.display().to_string(), main.display().to_string()]);
        let shown = |path: &str| std::path::Path::new(path).canonicalize().unwrap();
        assert_eq!(folders[0].path, linked.display().to_string());
        assert_eq!(folders[0].worktree_of.as_deref().map(shown), Some(main.canonicalize().unwrap()));
        assert_eq!(folders[1], RecentFolder { path: main.display().to_string(), worktree_of: None });
    }

    /// The snapshot names the folder the way the events do, and says why
    /// search by meaning is off once a sync has found out.
    #[test]
    fn the_index_snapshot_speaks_for_the_open_folder() {
        use crate::domain::embeddings::{Embedding, EmbeddingError, EmbeddingProvider};
        struct NoModel;
        impl EmbeddingProvider for NoModel {
            fn embed(&self, _: &[&str]) -> Result<Vec<Embedding>, EmbeddingError> {
                Err(EmbeddingError::Invalid("no model in this test".into()))
            }
            fn dimensions(&self) -> usize {
                2
            }
        }
        let index = WorkspaceIndex::new(temp_dir("cmd-snapshot-index"), Arc::new(NoModel));
        assert_eq!(snapshot(&index), None);

        let root = temp_dir("cmd-snapshot-repo").canonicalize().unwrap();
        std::fs::write(root.join("a.rs"), "fn a() {}\n").unwrap();
        std::fs::write(root.join("blob.bin"), [0u8, 159, 146, 150]).unwrap();
        let (tx, finished) = std::sync::mpsc::channel();
        let tx = Mutex::new(tx);
        index
            .open(
                &root,
                Arc::new(move |event| {
                    if matches!(event, crate::domain::workspace_index::IndexEvent::SyncFinished { .. }) {
                        let _ = tx.lock().unwrap().send(());
                    }
                }),
            )
            .unwrap();
        // Not "the first finish": FSEvents may still report the files written
        // above, and the sync that wakes is running when the first one ends.
        let shot = loop {
            finished.recv_timeout(std::time::Duration::from_secs(10)).expect("the sync never finished");
            let shot = snapshot(&index).unwrap();
            if !shot.syncing {
                break shot;
            }
        };
        assert_eq!(shot.root, root.display().to_string());
        assert_eq!((shot.embedded, shot.skipped), (0, 1));
        assert!(shot.embedding_error.as_deref().is_some_and(|e| e.contains("no model")), "{shot:?}");
    }
}
