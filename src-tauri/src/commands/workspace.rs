//! The open folder: opening it, the recent list, its branch and its index.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::domain::background::BackgroundProcesses;
use crate::infra::background::Processes;
use crate::infra::terminal::Terminals;
use crate::services::workspace_index::WorkspaceIndex;

use super::workspace_events::index_event_sink;
use super::agent_state::AgentState;

/// Opens a folder as the workspace. Kept as the frontend's entry point until
/// F-8.6; it is `workspace_add` — the folder joins the open set like any add.
#[tauri::command]
pub async fn workspace_open(
    path: String,
    keep_running: Option<bool>,
    app: AppHandle,
    state: State<'_, Arc<AgentState>>,
    index: State<'_, Arc<WorkspaceIndex>>,
    git_watch: State<'_, super::git::GitWatch>,
) -> Result<String, String> {
    workspace_add(path, keep_running, app, state, index, git_watch).await
}

/// Adds a folder to the open set and makes it the active one. Canonicalized
/// here, once, so every path a tool resolves later is measured against a real
/// directory rather than against whatever spelling the caller used.
///
/// Until F-8.3, switching the active folder still stops what ran in the
/// previous one (`services::folder_open::open`; `keep_running` is the
/// worktree-move exception) — from this step on only the registry is
/// many-folder. The rest — the git watch, the recent list, the index — is
/// `activate`, awaited off the UI thread; the index's first sync is not
/// awaited, and its failures arrive on `workspace-index:event`.
#[tauri::command]
pub async fn workspace_add(
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
    let resolved = crate::infra::paths::canonicalize_plain(Path::new(&path))
        .map_err(|e| format!("cannot open {path}: {e}"))?;
    if !resolved.is_dir() {
        return Err(format!("{path} is not a folder"));
    }
    state.register_open(resolved.clone());
    // Only a convenience for the next launch: an open folder stays open even
    // if the list could not be written.
    if let Err(e) = crate::infra::open_folders::record(&resolved.display().to_string()) {
        eprintln!("open folders not saved: {e}");
    }
    activate(resolved, keep_running == Some(true), &app, &state, &index, &git_watch).await
}

/// Makes `root` the folder the window's panes read: the git watch and the
/// index, and — unless `keep_running` — stopping what ran in the folder that
/// was active before.
async fn activate(
    root: PathBuf,
    keep_running: bool,
    app: &AppHandle,
    state: &AgentState,
    index: &Arc<WorkspaceIndex>,
    git_watch: &super::git::GitWatch,
) -> Result<String, String> {
    *state
        .workspace
        .lock()
        .map_err(|_| "workspace lock poisoned".to_string())? = Some(root.clone());
    let shown = root.display().to_string();
    super::git::watch_git(app, git_watch, &root, shown.clone());
    let index = Arc::clone(index);
    let sink = index_event_sink(app, shown.clone());
    let processes = app.try_state::<Arc<Processes>>().map(|processes| Arc::clone(&processes));
    let terminals = app.try_state::<Arc<Terminals>>().map(|terminals| Arc::clone(&terminals));
    let _ = tauri::async_runtime::spawn_blocking(move || {
        crate::services::folder_open::open(&root, keep_running, processes.as_deref(), terminals.as_deref(), &index, sink)
    })
    .await;
    Ok(shown)
}

/// Closes an open folder: out of the registry, its things stopped (F-8
/// invariant 4). Refused while a turn runs in it (invariant 5), and while
/// anything runs that cannot yet be attributed to a folder — F-8.3 gives
/// processes and terminals their folder and lifts that.
#[tauri::command]
pub async fn workspace_close(
    path: String,
    app: AppHandle,
    state: State<'_, Arc<AgentState>>,
    index: State<'_, Arc<WorkspaceIndex>>,
    git_watch: State<'_, super::git::GitWatch>,
) -> Result<(), String> {
    let root = state.require_open(&path)?;
    let was_active = state.workspace().ok().as_deref() == Some(root.as_path());
    close_refusal(&state, &root, anything_runs(&app))?;
    state.unregister_open(&root);
    if let Err(e) = crate::infra::open_folders::remove(&root.display().to_string()) {
        eprintln!("open folders not saved: {e}");
    }
    if was_active {
        if let Some(processes) = app.try_state::<Arc<Processes>>() {
            processes.stop_all();
        }
        if let Some(terminals) = app.try_state::<Arc<Terminals>>() {
            terminals.close_all();
        }
        let next = state.open_folders().into_iter().next();
        *state
            .workspace
            .lock()
            .map_err(|_| "workspace lock poisoned".to_string())? = next.clone();
        if let Some(next) = next {
            activate(next, false, &app, &state, &index, &git_watch).await?;
        }
    }
    Ok(())
}

/// Why a close may not happen, if it may not — kept out of the command so the
/// refusals are testable without a Tauri app.
fn close_refusal(state: &AgentState, root: &Path, anything_runs: bool) -> Result<(), String> {
    if !state.root_is_idle(root) {
        return Err("an agent turn is running in this folder — stop it before closing the folder".to_string());
    }
    if anything_runs {
        return Err("processes or terminals are still running — stop them before closing the folder".to_string());
    }
    Ok(())
}

fn anything_runs(app: &AppHandle) -> bool {
    let processes = app.try_state::<Arc<Processes>>().is_some_and(|processes| !processes.list().is_empty());
    let terminals = app.try_state::<Arc<Terminals>>().is_some_and(|terminals| !terminals.list().is_empty());
    processes || terminals
}

/// An open folder, as the sidebar will draw it from F-8.6 on.
#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenFolder {
    path: String,
    /// The one the window's panes read by default; at most one while any
    /// folder is open.
    active: bool,
}

/// The folders open in the window, the last added first. `workspaces_list`,
/// not `workspace_list` — the latter is the Files tab's tree command.
#[tauri::command]
pub fn workspaces_list(state: State<'_, Arc<AgentState>>) -> Vec<OpenFolder> {
    let active = state.workspace().ok();
    state
        .open_folders()
        .into_iter()
        .map(|path| OpenFolder {
            active: active.as_deref() == Some(path.as_path()),
            path: path.display().to_string(),
        })
        .collect()
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
pub async fn workspace_branch(workspace: Option<String>, state: State<'_, Arc<AgentState>>) -> Result<Option<String>, String> {
    let Some(root) = state.ui_workspace(&workspace)? else { return Ok(None) };
    super::blocking(move || Ok::<_, String>(crate::infra::git_head::current_branch(&root))).await
}

/// The main working tree when the open folder is a worktree of it; `None`
/// for an ordinary folder or repository.
#[tauri::command]
pub async fn workspace_worktree_of(workspace: Option<String>, state: State<'_, Arc<AgentState>>) -> Result<Option<String>, String> {
    let Some(root) = state.ui_workspace(&workspace)? else { return Ok(None) };
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

    /// Invariant 5: closing a folder is refused while one of its turns runs.
    /// Until processes and terminals carry a folder (F-8.3), a close is also
    /// refused while any of them runs anywhere — stopping the close's folder
    /// could not touch only what is its.
    #[test]
    fn closing_a_folder_is_refused_while_it_cannot_be_left_safely() {
        let state = AgentState::default();
        let root = crate::infra::paths::canonicalize_plain(&temp_dir("ws-close-guard")).unwrap();
        state.register_open(root.clone());

        let running = state.begin_running("c1", 1, &root).unwrap();
        let refused = close_refusal(&state, &root, false).expect_err("a running turn holds the folder");
        assert!(refused.contains("turn"), "{refused}");
        drop(running);

        let refused = close_refusal(&state, &root, true).expect_err("unattributed runners hold the folder");
        assert!(refused.contains("processes"), "{refused}");
        close_refusal(&state, &root, false).expect("an idle, quiet folder closes");
    }

    /// The branch is read for the folder the window names (F-8, invariant 1),
    /// not whichever is active; no folder is `None`, a folder not open is an error.
    #[test]
    fn the_branch_is_read_for_the_folder_named() {
        let app = tauri::test::mock_app();
        let state = Arc::new(AgentState::default());
        app.manage(Arc::clone(&state));
        assert_eq!(tauri::async_runtime::block_on(workspace_branch(None, app.state())).unwrap(), None);

        let root = crate::infra::paths::canonicalize_plain(&temp_dir("ws-branch-repo")).unwrap();
        let repo = git2::Repository::init(&root).unwrap();
        let tree = repo.find_tree(repo.index().unwrap().write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("T", "t@example.com").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "c", &tree, &[]).unwrap();
        state.register_open(root.clone());
        // Active is another folder, without a repository: the answer must be
        // the named folder's branch, not the active folder's nothing.
        state.set_workspace_for_test(temp_dir("ws-branch-active"));
        let branch = tauri::async_runtime::block_on(workspace_branch(Some(root.display().to_string()), app.state())).unwrap();
        assert!(branch.is_some(), "the named repository's branch");

        let elsewhere = temp_dir("ws-branch-elsewhere");
        let err = tauri::async_runtime::block_on(workspace_branch(Some(elsewhere.display().to_string()), app.state())).unwrap_err();
        assert!(err.contains("not an open folder"), "{err}");
    }
}
