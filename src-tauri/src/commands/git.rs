//! The Changes tab: the open folder's staged and unstaged files, staging, and
//! committing. Off the IPC loop — a status of a large repository takes a while.
//!
//! The tab is told when to read again rather than asking on a timer. An edit
//! to the tree already arrives as the index's `syncStarted`; what that watcher
//! leaves out is `.git`, so a `git add` or a commit made in a terminal is
//! reported here, on [`GIT_EVENT`].

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime, State};

use super::chat::AgentState;
use crate::domain::git_branches::{BranchRef, CheckoutOutcome, WorktreeState};
use crate::domain::git_changes::{ChangeTotals, FileSide, FileView, GitChangesError, GitHistory, WorkingChanges};
use crate::infra::file_watcher::FileWatcher;
use crate::infra::{chat_store, git_branches, git_changes};
use crate::services::{commit_message, llm_session};

/// The open folder's repository changed its index, HEAD or refs.
pub const GIT_EVENT: &str = "workspace-git:changed";

/// The folder it is about, the string `workspace_open` returned: an event from
/// the folder just left must not refresh the one opened after it.
#[derive(Clone, Serialize)]
struct GitChanged {
    root: String,
}

/// The watch on the open folder's `.git`. Replaced when another folder opens.
#[derive(Default)]
pub struct GitWatch(Mutex<Option<FileWatcher>>);

/// Watches the repository `root` is in, in place of the previous folder's.
/// Outside a repository, or where it cannot be watched, nothing is reported —
/// the tab still reads again after its own actions and on tree edits.
pub fn watch_git<R: Runtime>(app: &AppHandle<R>, watch: &GitWatch, root: &Path, shown: String) {
    let mut current = watch.0.lock().unwrap_or_else(PoisonError::into_inner);
    *current = None;
    let Some(git_dir) = git_changes::git_dir(root) else { return };
    let app = app.clone();
    let started = FileWatcher::start(&git_dir, move || {
        let _ = app.emit(GIT_EVENT, GitChanged { root: shown.clone() });
    });
    match started {
        Ok(watcher) => *current = Some(watcher),
        Err(e) => eprintln!("{} is not watched, so git changes made elsewhere are not shown: {e}", git_dir.display()),
    }
}

async fn in_repo<T: Send + 'static, E: std::fmt::Display + Send + 'static>(
    state: &AgentState,
    op: impl FnOnce(PathBuf) -> Result<T, E> + Send + 'static,
) -> Result<T, String> {
    let root = state.workspace()?;
    super::blocking(move || op(root)).await
}

#[tauri::command]
pub async fn git_changes(state: State<'_, Arc<AgentState>>) -> Result<WorkingChanges, String> {
    in_repo(&state, |root| git_changes::changes(&root)).await
}

/// `None` outside a repository: an ordinary folder has nothing to count, and
/// the header shows nothing rather than an error.
#[tauri::command]
pub async fn git_totals(state: State<'_, Arc<AgentState>>) -> Result<Option<ChangeTotals>, String> {
    in_repo(&state, |root| match git_changes::totals(&root) {
        Err(GitChangesError::NotARepository) => Ok(None),
        other => other.map(Some),
    })
    .await
}

#[tauri::command]
pub async fn git_stage(paths: Vec<String>, state: State<'_, Arc<AgentState>>) -> Result<(), String> {
    in_repo(&state, move |root| git_changes::stage(&root, &paths)).await
}

#[tauri::command]
pub async fn git_unstage(paths: Vec<String>, state: State<'_, Arc<AgentState>>) -> Result<(), String> {
    in_repo(&state, move |root| git_changes::unstage(&root, &paths)).await
}

/// The new commit's short id.
#[tauri::command]
pub async fn git_commit(message: String, state: State<'_, Arc<AgentState>>) -> Result<String, String> {
    in_repo(&state, move |root| git_changes::commit(&root, &message)).await
}

/// A commit message for what is staged, written by the active provider's
/// model; `draft` is what the box already holds.
#[tauri::command]
pub async fn git_commit_message(draft: String, state: State<'_, Arc<AgentState>>) -> Result<String, String> {
    let root = state.workspace()?;
    // A request to the provider, which would freeze the IPC loop for its duration.
    tauri::async_runtime::spawn_blocking(move || {
        let session = llm_session::resolve(None).map_err(|e| e.to_string())?;
        commit_message::generate(&session, &root, &draft).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Where HEAD is and the newest `limit` commits, for the History tab.
#[tauri::command]
pub async fn git_history(limit: usize, state: State<'_, Arc<AgentState>>) -> Result<GitHistory, String> {
    in_repo(&state, move |root| git_changes::history(&root, limit)).await
}

/// The two versions of a file the viewer diffs; see [`FileSide`] for which
/// and what `path` is relative to.
#[tauri::command]
pub async fn file_view(path: String, side: FileSide, state: State<'_, Arc<AgentState>>) -> Result<FileView, String> {
    in_repo(&state, move |root| git_changes::file_view(&root, &path, side)).await
}

/// The branches a new chat can start on: local ones, then remote ones with
/// no local branch of that name.
#[tauri::command]
pub async fn git_branches(state: State<'_, Arc<AgentState>>) -> Result<Vec<BranchRef>, String> {
    in_repo(&state, |root| git_branches::list(&root)).await
}

/// Switches the open folder to `branch`, or says which files stand in the
/// way — it never overwrites them.
#[tauri::command]
pub async fn git_checkout(branch: String, state: State<'_, Arc<AgentState>>) -> Result<CheckoutOutcome, String> {
    in_repo(&state, move |root| git_branches::checkout(&root, &branch)).await
}

/// Starts a worktree on a new branch from `base`, in the app's directory, and
/// returns its folder for the window to open. The open folder is left as it is.
#[tauri::command]
pub async fn git_worktree_add(base: String, state: State<'_, Arc<AgentState>>) -> Result<String, String> {
    let worktrees = crate::infra::app_dir::ensure()?.join("worktrees");
    let stamp = chrono::Local::now().format("%m%d-%H%M").to_string();
    let path = in_repo(&state, move |root| git_branches::add_worktree(&root, &base, &stamp, &worktrees)).await?;
    Ok(path.display().to_string())
}

/// A worktree as removing it would find it, and how many chats would go with
/// it — what the confirmation says before anything is done.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeCheck {
    #[serde(flatten)]
    state: WorktreeState,
    chats: usize,
}

/// `path` as a worktree the window may remove: a folder the app has opened,
/// and not the one open now — its processes, terminals and watchers run in
/// it until another folder is opened.
fn removable(path: &str, open: Option<&Path>, recent: &[String]) -> Result<PathBuf, String> {
    let target = Path::new(path).canonicalize().map_err(|e| format!("{path}: {e}"))?;
    let same = |folder: &Path| folder.canonicalize().is_ok_and(|folder| folder == target);
    if open.is_some_and(same) {
        return Err("this worktree is the open folder — switch to another one to remove it".into());
    }
    if !recent.iter().any(|folder| same(Path::new(folder))) {
        return Err(format!("{path} is not a folder Kibo has opened"));
    }
    Ok(target)
}

fn removable_now(path: &str, state: &AgentState) -> Result<PathBuf, String> {
    removable(path, state.workspace().ok().as_deref(), &crate::infra::recent_workspaces::load())
}

#[tauri::command]
pub async fn git_worktree_check(path: String, state: State<'_, Arc<AgentState>>) -> Result<WorktreeCheck, String> {
    let target = removable_now(&path, &state)?;
    tauri::async_runtime::spawn_blocking(move || {
        let worktree = git_branches::worktree_state(&target).map_err(|e| e.to_string())?;
        let chats = chat_store::list(&target.display().to_string()).map_err(|e| e.to_string())?.len();
        Ok(WorktreeCheck { state: worktree, chats })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeRemoved {
    /// The branch left behind for the commits only it has.
    branch_kept: Option<String>,
    chats_removed: usize,
}

/// Removes the worktree at `path` — one from the folder list, not the open
/// folder — and its chats.
#[tauri::command]
pub async fn git_worktree_remove(path: String, state: State<'_, Arc<AgentState>>) -> Result<WorktreeRemoved, String> {
    let target = removable_now(&path, &state)?;
    tauri::async_runtime::spawn_blocking(move || remove_with_chats(&target))
        .await
        .map_err(|e| e.to_string())?
}

/// The folder first: chats deleted for a worktree that then stayed would be
/// lost for nothing. Keyed by the same canonical spelling `workspace_open`
/// saved them under — `target` is already canonical.
fn remove_with_chats(target: &Path) -> Result<WorktreeRemoved, String> {
    let saved_under = target.display().to_string();
    let branch_kept = git_branches::remove_worktree(target).map_err(|e| e.to_string())?;
    let chats_removed = chat_store::delete_in(&saved_under)
        .map_err(|e| format!("the worktree is removed, but its chats are not: {e}"))?;
    Ok(WorktreeRemoved { branch_kept, chats_removed })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::temp_dir;
    use std::sync::mpsc;
    use std::time::Duration;
    use tauri::Listener;

    #[test]
    fn staging_outside_the_tab_is_reported_with_the_folder() {
        let dir = temp_dir("git-watch");
        git2::Repository::init(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "a\n").unwrap();

        let app = tauri::test::mock_app();
        let (tx, rx) = mpsc::channel();
        app.handle().listen(GIT_EVENT, move |event| {
            let _ = tx.send(event.payload().to_string());
        });
        let watch = GitWatch::default();
        watch_git(app.handle(), &watch, &dir, "shown".into());

        git_changes::stage(&dir, &["a.txt".into()]).unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), r#"{"root":"shown"}"#);
    }

    #[test]
    fn opening_a_folder_outside_a_repository_stops_the_previous_watch() {
        let app = tauri::test::mock_app();
        let watch = GitWatch::default();
        let repo = temp_dir("git-watch-repo");
        git2::Repository::init(&repo).unwrap();
        watch_git(app.handle(), &watch, &repo, "repo".into());
        assert!(watch.0.lock().unwrap().is_some());

        watch_git(app.handle(), &watch, &temp_dir("git-watch-plain"), "plain".into());
        assert!(watch.0.lock().unwrap().is_none());
    }

    #[test]
    fn a_removed_worktree_takes_its_chats_and_only_its_own() {
        crate::testing::with_app_dir("cmd-worktree-remove", || {
            let main = temp_dir("cmd-worktree-remove-main");
            let repo = git2::Repository::init(&main).unwrap();
            let tree = repo.find_tree(repo.index().unwrap().write_tree().unwrap()).unwrap();
            let sig = git2::Signature::now("T", "t@example.com").unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "c", &tree, &[]).unwrap();
            let head = repo.head().unwrap().shorthand().unwrap().to_string();
            let path = git_branches::add_worktree(&main, &head, "t", &temp_dir("cmd-worktree-remove-home")).unwrap();
            let shown = |dir: &Path| dir.canonicalize().unwrap().display().to_string();
            let save = |id: &str, folder: &str| {
                chat_store::save(id, folder, &[], &serde_json::json!([]), &[], None, None).unwrap();
            };
            save("in-worktree", &shown(&path));
            save("in-main", &shown(&main));

            // Dirty: refused, and the chats stay with it.
            std::fs::write(path.join("new.txt"), "x").unwrap();
            assert!(remove_with_chats(&path.canonicalize().unwrap()).is_err());
            assert_eq!(chat_store::list(&shown(&path)).unwrap().len(), 1);

            std::fs::remove_file(path.join("new.txt")).unwrap();
            let saved_under = shown(&path);
            let removed = remove_with_chats(&path.canonicalize().unwrap()).unwrap();
            assert_eq!(removed, WorktreeRemoved { branch_kept: None, chats_removed: 1 });
            assert!(!path.exists());
            assert!(chat_store::list(&saved_under).unwrap().is_empty());
            assert_eq!(chat_store::list(&shown(&main)).unwrap().len(), 1);
        });
    }

    /// The path is the window's: only a folder from the list, and never the
    /// one open, whatever spelling it arrives in.
    #[test]
    fn a_worktree_is_removable_only_from_the_list_and_not_while_open() {
        let listed = temp_dir("cmd-removable-listed");
        let recent = vec![listed.display().to_string()];
        let spelled = listed.join(".").display().to_string();

        assert_eq!(removable(&spelled, None, &recent).unwrap(), listed.canonicalize().unwrap());
        assert!(removable(&spelled, Some(&listed), &recent).unwrap_err().contains("switch to another"));
        let elsewhere = temp_dir("cmd-removable-elsewhere");
        assert!(removable(&elsewhere.display().to_string(), None, &recent).unwrap_err().contains("not a folder Kibo has opened"));
        assert!(removable("/no/such/folder", None, &recent).is_err());
    }

    /// `src/lib/chat.ts` listens on this name.
    #[test]
    fn the_channel_name_is_pinned() {
        assert_eq!(GIT_EVENT, "workspace-git:changed");
    }
}
