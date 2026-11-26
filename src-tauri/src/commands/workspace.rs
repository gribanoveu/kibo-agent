//! The open folder: opening it, the recent list, its branch and its index.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Manager, Runtime, State};

use crate::domain::workspace::WorkspaceError;
use crate::infra::background::Processes;
use crate::infra::terminal::Terminals;
use crate::services::mcp_servers::McpServers;
use crate::services::workspace_index::WorkspaceIndex;

use super::workspace_events::index_event_sink;
use super::agent_state::AgentState;

/// At launch, reopens every folder of `workspaces.json` still on disk (F-8.8):
/// all go into the open set in their order, and the first — the one activated
/// last, the folder of the chat viewed last — is made active. Only its index
/// opens; another folder's opens when a turn searches it (LZ-4). The file is
/// left as it was. With no file yet (a build that
/// kept no open set) the recent list's first folder opens, as launches did
/// before; with every folder closed, none. A reloaded window, its folders
/// still open, keeps them as they are. Answers the active folder.
#[tauri::command]
pub async fn workspaces_restore<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, Arc<AgentState>>,
    index: State<'_, Arc<WorkspaceIndex>>,
    git_watch: State<'_, super::git::GitWatch>,
) -> Result<Option<String>, String> {
    let _changing = state.folder_change.lock().await;
    if let Ok(active) = state.workspace() {
        return Ok(Some(active.display().to_string()));
    }
    let Some(saved) = crate::infra::open_folders::saved() else {
        let Some(last) = crate::infra::recent_workspaces::load().into_iter().next() else { return Ok(None) };
        return add(last, &app, &state, &index, &git_watch).await.map(Some);
    };
    let folders: Vec<PathBuf> = saved
        .iter()
        .filter_map(|folder| crate::infra::paths::canonicalize_plain(Path::new(folder)).ok())
        .filter(|root| root.is_dir())
        .collect();
    for root in folders.iter().rev() {
        state.register_open(root.clone());
    }
    let Some(active) = folders.first() else { return Ok(None) };
    activate(active.clone(), &app, &state, &index, &git_watch).await.map(Some)
}

/// Out of the open set and `workspaces.json`, and everything of it stopped
/// (`services::folder_open::close`) off the UI thread. Refused while a turn
/// runs in it (invariant 5) — checked with the registry held, so no turn
/// starts in it between the check and the close (`unregister_if_idle`).
async fn close_folder<R: Runtime>(
    root: PathBuf,
    app: &AppHandle<R>,
    state: &AgentState,
    index: &Arc<WorkspaceIndex>,
) -> Result<(), String> {
    if !state.unregister_if_idle(&root) {
        return Err(WorkspaceError::TurnRunning.to_string());
    }
    forget_open(&root);
    let processes = app.try_state::<Arc<Processes>>().map(|processes| Arc::clone(&processes));
    let terminals = app.try_state::<Arc<Terminals>>().map(|terminals| Arc::clone(&terminals));
    let mcp = app.try_state::<Arc<McpServers>>().map(|mcp| Arc::clone(&mcp));
    let index = Arc::clone(index);
    let _ = tauri::async_runtime::spawn_blocking(move || {
        crate::services::folder_open::close(&root, processes.as_deref(), terminals.as_deref(), mcp.as_deref(), &index)
    })
    .await;
    Ok(())
}

fn forget_open(root: &Path) {
    if let Err(e) = crate::infra::open_folders::remove(&root.display().to_string()) {
        eprintln!("open folders not saved: {e}");
    }
}

/// Adds a folder to the open set and makes it the active one. Canonicalized
/// here, once, so every path a tool resolves later is measured against a real
/// directory rather than against whatever spelling the caller used.
///
/// The folder active before stays open, its processes and shells running
/// (F-8.3); its index closes unless a turn there still searches it (LZ-4).
/// The rest — the git watch, the recent
/// list, the index — is `activate`, awaited off the UI thread; the index's
/// first sync is not awaited, and its failures arrive on `workspace-index:event`.
#[tauri::command]
pub async fn workspace_add<R: Runtime>(
    path: String,
    app: AppHandle<R>,
    state: State<'_, Arc<AgentState>>,
    index: State<'_, Arc<WorkspaceIndex>>,
    git_watch: State<'_, super::git::GitWatch>,
) -> Result<String, String> {
    let _changing = state.folder_change.lock().await;
    add(path, &app, &state, &index, &git_watch).await
}

async fn add<R: Runtime>(
    path: String,
    app: &AppHandle<R>,
    state: &AgentState,
    index: &Arc<WorkspaceIndex>,
    git_watch: &super::git::GitWatch,
) -> Result<String, String> {
    // Without `\\?\` on Windows: it is what the folder is known by from
    // here on — the shell started in it (`cmd.exe` refuses that prefix and
    // opens in `C:\Windows`), the tools' root, the index's events, the keys
    // chats and trust are saved under.
    let resolved = crate::infra::paths::canonicalize_plain(Path::new(&path))
        .map_err(|e| WorkspaceError::CannotOpen { path: path.clone(), reason: e.to_string() }.to_string())?;
    if !resolved.is_dir() {
        return Err(WorkspaceError::NotAFolder { path }.to_string());
    }
    state.register_open(resolved.clone());
    // Only a convenience for the next launch: an open folder stays open even
    // if the list could not be written.
    if let Err(e) = crate::infra::open_folders::record(&resolved.display().to_string()) {
        eprintln!("open folders not saved: {e}");
    }
    activate(resolved, app, state, index, git_watch).await
}

/// Makes `root` the folder the window's panes read: the git watch and the
/// index. The folder active before keeps everything but its index, which a
/// turn still running there holds open.
async fn activate<R: Runtime>(
    root: PathBuf,
    app: &AppHandle<R>,
    state: &AgentState,
    index: &Arc<WorkspaceIndex>,
    git_watch: &super::git::GitWatch,
) -> Result<String, String> {
    *state
        .workspace
        .lock()
        .map_err(|_| WorkspaceError::LockPoisoned { what: "workspace".into() }.to_string())? = Some(root.clone());
    let shown = root.display().to_string();
    super::git::watch_git(app, git_watch, &root, shown.clone());
    let index = Arc::clone(index);
    let sink = index_event_sink(app, shown.clone());
    let _ = tauri::async_runtime::spawn_blocking(move || crate::services::folder_open::open(&root, &index, sink)).await;
    Ok(shown)
}

/// Closes an open folder: out of the registry, its processes, shells and
/// index stopped, and nothing of another folder's (F-8 invariant 4). Refused
/// while a turn runs in it (invariant 5).
#[tauri::command]
pub async fn workspace_close<R: Runtime>(
    path: String,
    app: AppHandle<R>,
    state: State<'_, Arc<AgentState>>,
    index: State<'_, Arc<WorkspaceIndex>>,
    git_watch: State<'_, super::git::GitWatch>,
) -> Result<(), String> {
    let _changing = state.folder_change.lock().await;
    // A folder deleted or renamed while open no longer canonicalizes; it is
    // closed by the spelling it was registered under, or it could never go.
    let root = state.require_open(&path).or_else(|refused| {
        state.open_folders().into_iter().find(|open| open.as_os_str() == path.as_str()).ok_or(refused)
    })?;
    let was_active = state.workspace().ok().as_deref() == Some(root.as_path());
    close_folder(root, &app, &state, &index).await?;
    if was_active {
        let next = state.open_folders().into_iter().next();
        *state
            .workspace
            .lock()
            .map_err(|_| WorkspaceError::LockPoisoned { what: "workspace".into() }.to_string())? = next.clone();
        match next {
            Some(next) => drop(activate(next, &app, &state, &index, &git_watch).await?),
            None => git_watch.stop(),
        }
    }
    Ok(())
}

/// An open folder, as the sidebar will draw it from F-8.6 on.
#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenFolder {
    path: String,
    /// The one the window's panes read by default; at most one while any
    /// folder is open.
    active: bool,
    /// The main checkout, when the folder is a linked worktree of it — so its
    /// section says whose it is.
    worktree_of: Option<String>,
}

/// The folders open in the window, the last added first. `workspaces_list`,
/// not `workspace_list` — the latter is the Files tab's tree command. Async:
/// whose worktree a folder is, git reads off the UI thread.
#[tauri::command]
pub async fn workspaces_list(state: State<'_, Arc<AgentState>>) -> Result<Vec<OpenFolder>, String> {
    let active = state.workspace().ok();
    let open = state.open_folders();
    super::blocking(move || Ok::<_, String>(open_folders(open, active.as_deref()))).await
}

fn open_folders(open: Vec<PathBuf>, active: Option<&Path>) -> Vec<OpenFolder> {
    open.into_iter()
        .map(|path| OpenFolder {
            active: active == Some(path.as_path()),
            worktree_of: crate::infra::git_head::worktree_of(&path).map(|main| main.display().to_string()),
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

/// An open folder's index as it stands — what a window that was not
/// listening when the sync began (a reload, say) starts from before the next
/// `workspace-index:event`.
#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexSnapshot {
    /// The same string `workspace_add` returned, which the events carry too.
    root: String,
    syncing: bool,
    embedded: usize,
    skipped: usize,
    embedding_error: Option<String>,
}

/// `None` for no folder, or one whose index is not open.
#[tauri::command]
pub fn workspace_index_status(
    workspace: Option<String>,
    state: State<'_, Arc<AgentState>>,
    index: State<'_, Arc<WorkspaceIndex>>,
) -> Result<Option<IndexSnapshot>, String> {
    let Some(root) = state.ui_workspace(&workspace)? else { return Ok(None) };
    Ok(snapshot(&index, &root))
}

fn snapshot(index: &WorkspaceIndex, root: &Path) -> Option<IndexSnapshot> {
    let indexer = index.indexer(root)?;
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
    use crate::domain::mcp::{McpCallResult, McpClient, McpConfig, McpError, McpServerConfig, McpTool};
    
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
        let index = Arc::new(WorkspaceIndex::new(temp_dir("cmd-snapshot-index"), Arc::new(NoModel)));
        let root = crate::infra::paths::canonicalize_plain(&temp_dir("cmd-snapshot-repo")).unwrap();
        assert_eq!(snapshot(&index, &root), None);

        std::fs::write(root.join("a.rs"), "fn a() {}\n").unwrap();
        std::fs::write(root.join("blob.bin"), [0u8, 159, 146, 150]).unwrap();
        let (tx, finished) = std::sync::mpsc::channel();
        let tx = Mutex::new(tx);
        index
            .activate(
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
            let shot = snapshot(&index, &root).unwrap();
            if !shot.syncing {
                break shot;
            }
        };
        assert_eq!(shot.root, root.display().to_string());
        assert_eq!((shot.embedded, shot.skipped), (0, 1));
        assert!(shot.embedding_error.as_deref().is_some_and(|e| e.contains("no model")), "{shot:?}");
        let neighbour = temp_dir("cmd-snapshot-neighbour");
        let _held = index.searcher(&neighbour, Arc::new(|_| {})).unwrap();
        assert_eq!(snapshot(&index, &root).unwrap().root, root.display().to_string(), "the folder asked about, not the last opened");
    }

    /// Invariant 5: closing a folder is refused while one of its turns runs,
    /// and only then — what else runs in it is its to stop (F-8.3).
    #[test]
    fn closing_a_folder_is_refused_while_a_turn_runs_in_it() {
        let state = AgentState::default();
        let root = crate::infra::paths::canonicalize_plain(&temp_dir("ws-close-guard")).unwrap();
        state.register_open(root.clone());

        let running = state.begin_running("c1", 1, &root).unwrap();
        assert!(!state.unregister_if_idle(&root), "a running turn holds the folder");
        assert!(state.is_open(&root));
        drop(running);
        assert!(state.unregister_if_idle(&root), "an idle folder closes");
        assert!(!state.is_open(&root));
        // A turn admitted after the close is refused, its mark cleared: no
        // turn runs in a closed folder, its processes out of reach.
        let late = state.begin_running("c2", 1, &root).unwrap();
        assert!(state.still_open(&root).is_err());
        drop(late);
        assert!(state.root_is_idle(&root));
    }

    /// Closing a folder stops following its index and leaves the other open
    /// folder's things alone (F-8.2, invariant 4); the folder left active is
    /// the one still open, and its index is followed again (LZ-4).
    #[test]
    fn closing_a_folder_closes_its_index_and_no_other() {
        crate::testing::with_app_dir("ws-close-index", || {
            let app = tauri::test::mock_app();
            let state = Arc::new(AgentState::default());
            let index = Arc::new(WorkspaceIndex::new(temp_dir("ws-close-index-store"), Arc::new(Flat)));
            app.manage(Arc::clone(&state));
            app.manage(Arc::clone(&index));
            app.manage(super::super::git::GitWatch::default());
            let mcp = Arc::new(McpServers::new(Arc::new(|_, _, _| Ok(Arc::new(Server) as Arc<dyn McpClient>)), Arc::new(|_| {})));
            app.manage(Arc::clone(&mcp));
            let mut config = McpConfig::default();
            config.mcp_servers.insert("s".into(), McpServerConfig { command: "x".into(), ..Default::default() });
            let serves = |root: &Path| mcp.running(root).get("mcp__s__t").is_some();
            let plain = |dir: PathBuf| crate::infra::paths::canonicalize_plain(&dir).unwrap();
            let (a, b) = (plain(temp_dir("ws-close-a")), plain(temp_dir("ws-close-b")));
            let add = |root: &Path| {
                tauri::async_runtime::block_on(workspace_add(
                    root.display().to_string(),
                    app.handle().clone(),
                    app.state(),
                    app.state(),
                    app.state(),
                ))
                .unwrap()
            };
            add(&a);
            add(&b);
            assert!(index.indexer(&a).is_none() && index.indexer(&b).is_some(), "only the active folder is indexed");
            mcp.for_turn(&config, &a, &|| false);
            mcp.for_turn(&config, &b, &|| false);

            tauri::async_runtime::block_on(workspace_close(
                b.display().to_string(),
                app.handle().clone(),
                app.state(),
                app.state(),
                app.state(),
            ))
            .unwrap();

            assert!(index.indexer(&b).is_none(), "the closed folder's index is closed");
            assert!(index.indexer(&a).is_some(), "the folder left active is indexed");
            assert!(!serves(&b) && serves(&a), "the closed folder's MCP servers stop, the other's stay");
            assert_eq!(state.open_folders(), vec![a.clone()]);
            assert_eq!(state.workspace().unwrap(), a);
        });
    }

    /// Adding and closing a folder wait for each other (`folder_change`): a
    /// double launch opens one index once, and a close never interleaves
    /// with an add of the same folder.
    #[test]
    fn adding_and_closing_a_folder_wait_for_one_in_progress() {
        crate::testing::with_app_dir("ws-change-lock", || {
            let app = tauri::test::mock_app();
            let state = Arc::new(AgentState::default());
            app.manage(Arc::clone(&state));
            app.manage(Arc::new(WorkspaceIndex::new(temp_dir("ws-change-lock-store"), Arc::new(Flat))));
            app.manage(super::super::git::GitWatch::default());
            let root = crate::infra::paths::canonicalize_plain(&temp_dir("ws-change-lock-root")).unwrap();
            let shown = root.display().to_string();
            let waits = |work: Box<dyn FnOnce(AppHandle<tauri::test::MockRuntime>) + Send>, done: &dyn Fn() -> bool| {
                let held = state.folder_change.blocking_lock();
                let handle = app.handle().clone();
                let worker = std::thread::spawn(move || work(handle));
                std::thread::sleep(std::time::Duration::from_millis(200));
                assert!(!done(), "went ahead while another change was in progress");
                drop(held);
                worker.join().unwrap();
                assert!(done());
            };

            let path = shown.clone();
            waits(
                Box::new(move |h| {
                    tauri::async_runtime::block_on(workspace_add(path, h.clone(), h.state(), h.state(), h.state()))
                        .unwrap();
                }),
                &|| state.is_open(&root),
            );
            let path = shown.clone();
            waits(
                Box::new(move |h| {
                    tauri::async_runtime::block_on(workspace_close(path, h.clone(), h.state(), h.state(), h.state()))
                        .unwrap();
                }),
                &|| !state.is_open(&root),
            );
        });
    }

    fn folder_app() -> (tauri::App<tauri::test::MockRuntime>, Arc<AgentState>, Arc<WorkspaceIndex>) {
        let app = tauri::test::mock_app();
        let state = Arc::new(AgentState::default());
        let index = Arc::new(WorkspaceIndex::new(temp_dir("ws-app-store"), Arc::new(Flat)));
        app.manage(Arc::clone(&state));
        app.manage(Arc::clone(&index));
        app.manage(super::super::git::GitWatch::default());
        (app, state, index)
    }

    fn git_repo(label: &str) -> PathBuf {
        let root = crate::infra::paths::canonicalize_plain(&temp_dir(label)).unwrap();
        git2::Repository::init(&root).unwrap();
        root
    }

    /// F-8.8: a launch reopens every folder still on disk, in the order they
    /// were, the one activated last active; a reloaded window keeps its own.
    #[test]
    fn a_launch_reopens_every_folder_the_last_activated_active() {
        crate::testing::with_app_dir("ws-restore", || {
            let plain = |label| crate::infra::paths::canonicalize_plain(&temp_dir(label)).unwrap();
            let (a, b, gone) = (git_repo("ws-restore-a"), plain("ws-restore-b"), plain("ws-restore-gone"));
            {
                let (app, _, _) = folder_app();
                for root in [&a, &gone, &b, &a] {
                    tauri::async_runtime::block_on(workspace_add(
                        root.display().to_string(),
                        app.handle().clone(),
                        app.state(),
                        app.state(),
                        app.state(),
                    ))
                    .unwrap();
                }
            }
            std::fs::remove_dir_all(&gone).unwrap();

            let (app, state, index) = folder_app();
            let restore = || {
                tauri::async_runtime::block_on(workspaces_restore(app.handle().clone(), app.state(), app.state(), app.state())).unwrap()
            };
            let file = crate::infra::open_folders::saved();
            assert_eq!(restore(), Some(a.display().to_string()), "the folder activated last is active");
            assert_eq!(state.open_folders(), vec![a.clone(), b.clone()], "every folder still on disk, in its order");
            assert_eq!(crate::infra::open_folders::saved(), file, "the file is left as it was");
            assert!(index.indexer(&a).is_some(), "the active folder is indexed once restored");
            assert!(index.indexer(&b).is_none(), "a folder in the background stays cold until a turn runs in it");
            assert!(app.state::<super::super::git::GitWatch>().is_watching(), "the active folder's .git is watched");

            state.set_workspace_for_test(b.clone());
            assert_eq!(restore(), Some(b.display().to_string()), "a reloaded window keeps its folders as they are");
            assert_eq!(state.open_folders(), vec![b.clone(), a.clone()]);

            // Every folder closed by hand: none comes back, not even the recent one.
            let (app, _, _) = folder_app();
            crate::infra::open_folders::remove(&a.display().to_string()).unwrap();
            crate::infra::open_folders::remove(&b.display().to_string()).unwrap();
            let none = tauri::async_runtime::block_on(workspaces_restore(app.handle().clone(), app.state(), app.state(), app.state()));
            assert_eq!(none, Ok(None), "every folder was closed");
        });
    }

    /// A linked worktree open beside its main checkout says whose it is; the
    /// main checkout and a plain folder say nothing.
    #[test]
    fn an_open_worktree_names_its_main_checkout() {
        let main = git_repo("ws-open-wt-main");
        let repo = git2::Repository::open(&main).unwrap();
        repo.set_head("refs/heads/main").unwrap();
        let tree = repo.find_tree(repo.index().unwrap().write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("Test", "test@example.com").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[]).unwrap();
        let home = crate::infra::paths::canonicalize_plain(&temp_dir("ws-open-wt-home")).unwrap();
        let linked = crate::infra::paths::canonicalize_plain(&crate::infra::git_branches::add_worktree(&main, "main", "fix", &home).unwrap()).unwrap();
        let plain = crate::infra::paths::canonicalize_plain(&temp_dir("ws-open-wt-plain")).unwrap();

        let listed = open_folders(vec![linked.clone(), main.clone(), plain.clone()], Some(&main));
        let whose: Vec<_> = listed.iter().map(|one| (one.active, one.worktree_of.clone())).collect();
        assert_eq!(whose, vec![(false, Some(main.display().to_string())), (true, None), (false, None)]);
    }

    /// A build that kept no open set: the folder opened last comes back.
    #[test]
    fn a_launch_with_no_open_set_saved_reopens_the_recent_folder() {
        crate::testing::with_app_dir("ws-restore-recent", || {
            let a = crate::infra::paths::canonicalize_plain(&temp_dir("ws-restore-recent-a")).unwrap();
            crate::infra::recent_workspaces::record(&a.display().to_string()).unwrap();
            let (app, state, _) = folder_app();
            let restored = tauri::async_runtime::block_on(workspaces_restore(app.handle().clone(), app.state(), app.state(), app.state()));
            assert_eq!(restored, Ok(Some(a.display().to_string())));
            assert_eq!(state.open_folders(), vec![a]);
        });
    }

    /// A folder deleted while open still closes, by the spelling it was
    /// opened under; closing the last folder stops watching its `.git`.
    #[test]
    fn a_vanished_folder_closes_and_the_last_close_stops_the_git_watch() {
        crate::testing::with_app_dir("ws-vanished", || {
            let (app, state, _) = folder_app();
            let close = |path: String| {
                tauri::async_runtime::block_on(workspace_close(path, app.handle().clone(), app.state(), app.state(), app.state()))
            };
            let root = git_repo("ws-vanished-repo");
            tauri::async_runtime::block_on(workspace_add(
                root.display().to_string(),
                app.handle().clone(),
                app.state(),
                app.state(),
                app.state(),
            ))
            .unwrap();
            assert!(app.state::<super::super::git::GitWatch>().is_watching());
            std::fs::remove_dir_all(&root).unwrap();

            close(root.display().to_string()).unwrap();

            assert!(state.open_folders().is_empty());
            assert!(!app.state::<super::super::git::GitWatch>().is_watching(), "the closed folder's .git is still watched");
            assert!(close(root.display().to_string()).is_err(), "a folder not open is refused");
        });
    }

    struct Server;
    impl McpClient for Server {
        fn list_tools(&self) -> Result<Vec<McpTool>, McpError> {
            Ok(vec![McpTool { name: "t".into(), ..Default::default() }])
        }
        fn call_tool(&self, _: &str, _: serde_json::Value, _: &dyn Fn() -> bool) -> Result<McpCallResult, McpError> {
            Err(McpError::Cancelled)
        }
    }

    struct Flat;
    impl crate::domain::embeddings::EmbeddingProvider for Flat {
        fn embed(&self, texts: &[&str]) -> Result<Vec<crate::domain::embeddings::Embedding>, crate::domain::embeddings::EmbeddingError> {
            Ok(texts.iter().map(|_| crate::domain::embeddings::Embedding(vec![1.0, 0.0])).collect())
        }
        fn dimensions(&self) -> usize {
            2
        }
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
