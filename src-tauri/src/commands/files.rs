//! The Files tab's folder tree, one folder at a time as it is unfolded, and
//! saving a file edited in the viewer.

use std::path::PathBuf;
use std::sync::Arc;

use tauri::State;

use super::chat::AgentState;
use crate::domain::file_tree::FolderListing;
use crate::domain::file_write::FileSave;
use crate::infra::{file_tree, file_write};

/// `dir` is relative to the open folder; empty for the folder itself. Off the
/// IPC loop: a folder's walk and git's status of it take a moment on a big one.
#[tauri::command]
pub async fn workspace_list(dir: String, state: State<'_, Arc<AgentState>>) -> Result<FolderListing, String> {
    let root = state.workspace()?;
    tauri::async_runtime::spawn_blocking(move || file_tree::list(&root, &dir))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// Saves a file of the open folder edited in the viewer. `root` is the folder
/// the edit was made in, as `workspace_open` returned it: another one open by
/// now, nothing is written. `expected` is the text it was opened with: changed
/// on disk since, it is not written and the answer says so. `None` writes it
/// regardless — the user chose their edit.
#[tauri::command]
pub async fn file_write(
    root: String,
    path: String,
    expected: Option<String>,
    content: String,
    state: State<'_, Arc<AgentState>>,
) -> Result<FileSave, String> {
    let root = open_folder(state.workspace()?, &root)?;
    tauri::async_runtime::spawn_blocking(move || file_write::write(&root, &path, expected.as_deref(), &content))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// The open folder, if it is the one asked about. An edit saved as its folder
/// is left — the editor goes, and saves on its way — would otherwise land in
/// the next one, at the same path: in a worktree of the same repository, over
/// the same text, so nothing would tell it apart.
fn open_folder(open: PathBuf, asked: &str) -> Result<PathBuf, String> {
    if open.display().to_string() == asked {
        Ok(open)
    } else {
        Err(format!("{asked} is no longer the open folder"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_save_goes_only_to_the_folder_it_was_made_in() {
        let open = PathBuf::from("/work/repo");
        assert_eq!(open_folder(open.clone(), "/work/repo"), Ok(open.clone()));
        assert_eq!(
            open_folder(open, "/work/repo-feature"),
            Err("/work/repo-feature is no longer the open folder".to_string())
        );
    }
}
