//! Rewinding a chat's files to before a message — the window sends the
//! changes its blocks recorded; see `services::rewind`. The agent's chat
//! rewinds the open folder; a Prototype chat its own folder, named by its id.

use std::path::PathBuf;
use std::sync::Arc;

use tauri::State;

use crate::domain::rewind::{FileChange, FileRewind};
use crate::domain::tools::ToolScope;
use crate::infra::prototype_store;
use crate::services::rewind;

use super::blocking;
use super::chat::AgentState;

/// The folder a rewind works in: the Prototype chat's when `chat_id` names
/// one, else the agent's open folder — asked only then, as a chat has none.
fn root(chat_id: Option<&str>, workspace: impl FnOnce() -> Result<PathBuf, String>) -> Result<PathBuf, String> {
    match chat_id {
        Some(id) => prototype_store::files_dir(id).map_err(|e| e.to_string()),
        None => workspace(),
    }
}

fn scope(state: &AgentState, chat_id: Option<&str>) -> Result<ToolScope, String> {
    ToolScope::new(&root(chat_id, || state.workspace())?).map_err(|e| e.to_string())
}

/// What a rewind would do, file by file. Touches nothing.
#[tauri::command]
pub async fn rewind_preview(
    changes: Vec<FileChange>,
    chat_id: Option<String>,
    state: State<'_, Arc<AgentState>>,
) -> Result<Vec<FileRewind>, String> {
    let scope = scope(&state, chat_id.as_deref())?;
    blocking(move || Ok::<_, String>(rewind::preview(&scope, &changes))).await
}

/// Does it, and says how each file went.
#[tauri::command]
pub async fn rewind_apply(
    changes: Vec<FileChange>,
    chat_id: Option<String>,
    state: State<'_, Arc<AgentState>>,
) -> Result<Vec<FileRewind>, String> {
    let scope = scope(&state, chat_id.as_deref())?;
    blocking(move || Ok::<_, String>(rewind::apply(&scope, &changes))).await
}

#[cfg(test)]
mod prototype_tests {
    use super::*;

    /// A Prototype chat rewinds its own folder, and never asks for the agent's.
    #[test]
    fn a_chat_named_rewinds_its_own_folder_and_the_agent_its_open_one() {
        crate::testing::with_app_dir("rewind-root", || {
            let id = "1f3e5d7c-9b2a-4c6e-8d0f-2a4c6e8a0c2e";
            let asked = std::cell::Cell::new(false);
            let chat = root(Some(id), || {
                asked.set(true);
                Ok(PathBuf::from("/open/folder"))
            })
            .unwrap();
            assert!(chat.ends_with(std::path::Path::new("prototypes").join(id).join("files")), "{chat:?}");
            assert!(!asked.get());
            assert_eq!(root(None, || Ok(PathBuf::from("/open/folder"))).unwrap(), PathBuf::from("/open/folder"));
            assert!(root(Some("../x"), || Ok(PathBuf::new())).is_err());
        });
    }
}
