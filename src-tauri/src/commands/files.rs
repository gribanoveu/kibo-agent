//! The Files tab's folder tree, one folder at a time as it is unfolded, and
//! saving a file edited in the viewer.

use std::sync::Arc;

use tauri::State;

use super::agent_state::AgentState;
use crate::domain::file_tree::FolderListing;
use crate::domain::file_write::FileSave;
use crate::infra::{file_tree, file_write};

/// `dir` is relative to the folder; empty for the folder itself. `workspace`
/// is the folder the Files tab shows, from the window: canonicalized and
/// accepted only as an open folder (F-8, invariant 1). Off the IPC loop: a
/// folder's walk and git's status of it take a moment on a big one.
#[tauri::command]
pub async fn workspace_list(
    dir: String,
    workspace: Option<String>,
    state: State<'_, Arc<AgentState>>,
) -> Result<FolderListing, String> {
    let root = state
        .ui_workspace(&workspace)?
        .ok_or_else(|| crate::domain::workspace::WorkspaceError::NoFolderOpen.to_string())?;
    tauri::async_runtime::spawn_blocking(move || file_tree::list(&root, &dir))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// Saves a file of an open folder edited in the viewer. `root` is the folder
/// the edit was made in: one closed since, nothing is written — a save on
/// its way out would otherwise land in the next folder at the same path: in
/// a worktree of the same repository, over the same text, so nothing would
/// tell it apart. `expected` is the text it was opened with: changed on disk
/// since, it is not written and the answer says so. `None` writes it
/// regardless — the user chose their edit.
#[tauri::command]
pub async fn file_write(
    root: String,
    path: String,
    expected: Option<String>,
    content: String,
    state: State<'_, Arc<AgentState>>,
) -> Result<FileSave, String> {
    let root = state.require_open(&root)?;
    tauri::async_runtime::spawn_blocking(move || file_write::write(&root, &path, expected.as_deref(), &content))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tauri::Manager;

    use super::*;
    use crate::infra::paths::canonicalize_plain;
    use crate::testing::temp_dir;

    /// A save on its way out of a closed folder writes nothing, even where
    /// the path still exists.
    #[test]
    fn a_closed_folder_takes_no_save() {
        let state = Arc::new(AgentState::default());
        let root = canonicalize_plain(&temp_dir("cmd-file-write-closed")).unwrap();
        std::fs::write(root.join("a.txt"), "old").unwrap();
        let app = tauri::test::mock_app();
        app.manage(Arc::clone(&state));
        let save = |text: &str| {
            tauri::async_runtime::block_on(file_write(
                root.display().to_string(),
                "a.txt".into(),
                None,
                text.into(),
                app.state(),
            ))
        };

        let refused = save("new").expect_err("a folder never opened takes nothing");
        assert!(refused.contains("not an open folder"), "{refused}");
        assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "old");

        state.register_open(root.clone());
        save("new").expect("an open folder takes the save");
        assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "new");
    }
}
