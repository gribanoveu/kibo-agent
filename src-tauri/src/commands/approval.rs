//! What a turn may do without asking: the approval policy, Auto and its
//! memory, and the conversation mode.

use std::sync::Arc;

use tauri::State;

use crate::domain::conversation_mode::PickedMode;
use crate::domain::settings::RememberScope;

use super::agent_state::AgentState;

/// "Always allow this tool", from an approval card. Lasts as long as the app
/// runs and no longer.
#[tauri::command]
pub fn approval_always_allow(tool: String, state: State<'_, Arc<AgentState>>) -> Result<(), String> {
    state
        .approval
        .lock()
        .map_err(|_| "approval lock poisoned".to_string())?
        .allow_always(&tool)
}

/// What the conversation is for: everything, or answering only. Takes effect
/// from the next turn, and never retroactively — a turn already paused on an
/// approval card runs the calls the user decided on.
#[tauri::command]
pub fn chat_set_mode(
    mode: PickedMode,
    state: State<'_, Arc<AgentState>>,
) -> Result<(), String> {
    set_mode(&state, mode)
}

/// Written first: a choice that cannot be kept is refused whole, and the chip,
/// which moves when this says yes, never shows what the backend is not doing.
fn set_mode(state: &AgentState, mode: PickedMode) -> Result<(), String> {
    crate::infra::settings_store::update(|s| s.conversation_mode = mode).map_err(|e| e.to_string())?;
    *state.mode.lock().map_err(|_| "mode lock poisoned".to_string())? = mode;
    Ok(())
}

/// The mode turns run in now, for the composer's panel when the window opens.
#[tauri::command]
pub fn chat_mode(state: State<'_, Arc<AgentState>>) -> PickedMode {
    state.picked()
}

/// Runs the whole turn without asking. Deliberately not persisted anywhere.
/// Async: a chat's folder is read from the chat store.
#[tauri::command]
pub async fn approval_set_unattended(
    unattended: bool,
    chat_id: Option<String>,
    state: State<'_, Arc<AgentState>>,
) -> Result<(), String> {
    let state = Arc::clone(&state);
    super::blocking(move || set_unattended(&state, unattended, chat_id.as_deref())).await
}

fn set_unattended(state: &AgentState, unattended: bool, chat_id: Option<&str>) -> Result<(), String> {
    set_skip_all(state, unattended)?;
    // Remembered after it took effect: a settings file that cannot be read
    // costs the memory, not the choice made for this turn.
    // No folder, nothing to remember it for.
    let Some(folder) = auto_folder(state, chat_id) else { return Ok(()) };
    crate::infra::settings_store::update(|s| s.approval.set_auto(chat_id, &folder.display().to_string(), unattended))
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Auto is keyed by (chat, folder), and the folder is the chat's own: a chat
/// never leaves its checkout (F-8), so the choice must not follow whichever
/// folder the window happens to show. A folder-scope choice (`None` chat) and
/// a chat with no record yet key on the open folder.
fn auto_folder(state: &AgentState, chat_id: Option<&str>) -> Option<std::path::PathBuf> {
    match chat_id {
        Some(id) => state.chat_workspace(id).ok().map(|(root, _)| root),
        None => state.workspace().ok(),
    }
}

/// Puts back what was chosen for this chat, or for the open folder, and says
/// what it is. Ask when nothing was, or no folder is open.
#[tauri::command]
pub async fn approval_restore(chat_id: Option<String>, state: State<'_, Arc<AgentState>>) -> Result<bool, String> {
    let state = Arc::clone(&state);
    super::blocking(move || restore_unattended(&state, chat_id.as_deref())).await
}

fn restore_unattended(state: &AgentState, chat_id: Option<&str>) -> Result<bool, String> {
    let settings = crate::infra::settings_store::load().map_err(|e| e.to_string())?;
    let auto = auto_folder(state, chat_id)
        .is_some_and(|folder| settings.approval.is_auto(chat_id, &folder.display().to_string()));
    set_skip_all(state, auto)?;
    Ok(auto)
}

#[tauri::command]
pub fn approval_remember_get() -> Result<RememberScope, String> {
    Ok(crate::infra::settings_store::load().map_err(|e| e.to_string())?.approval.remember)
}

/// Refuses on settings it cannot read, rather than saving defaults over them.
#[tauri::command]
pub fn approval_remember_set(remember: RememberScope) -> Result<(), String> {
    crate::infra::settings_store::update(|s| s.approval.remember = remember).map_err(|e| e.to_string())
}

/// A chat switched to Auto before its first save had no id to be remembered
/// by; it has one now. Best effort: the chat is saved either way, and a
/// memory that failed to save only means it asks next time.
pub(super) fn remember_new_chat(state: &AgentState, id: &str) {
    let auto = state.approval.lock().is_ok_and(|approval| approval.skip_all);
    if !auto {
        return;
    }
    let _ = crate::infra::settings_store::update(|s| {
        s.approval.remember == RememberScope::Chat && s.approval.set_auto(Some(id), "", true)
    });
}

fn set_skip_all(state: &AgentState, unattended: bool) -> Result<(), String> {
    state.approval.lock().map_err(|_| "approval lock poisoned".to_string())?.skip_all = unattended;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::conversation_mode::ConversationMode;
    use crate::domain::tools::ToolName;
    use crate::testing::temp_dir;
    

    /// The state is reachable without a running app, which is what keeps these
    /// checks about behaviour rather than about Tauri.
    fn state() -> Arc<AgentState> {
        Arc::new(AgentState::default())
    }

    /// An analyst opens the app as they left it; the next window is the next
    /// `restored`.
    #[test]
    fn the_mode_picked_outlives_the_window() {
        crate::testing::with_app_dir("chat-mode-kept", || {
            assert_eq!(AgentState::restored().picked(), PickedMode::Agent, "nothing picked yet");
            std::fs::write(crate::infra::settings_store::path().unwrap(), "{}").unwrap();
            assert_eq!(AgentState::restored().picked(), PickedMode::Agent, "a file from before the choice");
            let state = state();
            set_mode(&state, PickedMode::Analyst).unwrap();
            assert_eq!(state.mode(), ConversationMode::Analyst);
            assert_eq!(AgentState::restored().mode(), ConversationMode::Analyst);
        });
    }

    /// Settings that cannot be read back to change refuse the choice: the
    /// backend stays where the chip stays. So at the start, a window opens in Agent.
    #[test]
    fn a_choice_that_cannot_be_kept_is_not_made() {
        crate::testing::with_app_dir("chat-mode-unwritable", || {
            let state = state();
            set_mode(&state, PickedMode::Analyst).unwrap();
            std::fs::write(crate::infra::settings_store::path().unwrap(), "not json").unwrap();
            assert!(set_mode(&state, PickedMode::Agent).is_err());
            assert_eq!(state.picked(), PickedMode::Analyst);
            assert_eq!(AgentState::restored().picked(), PickedMode::Agent);
        });
    }

    fn skip_all(state: &AgentState) -> bool {
        state.approval.lock().unwrap().skip_all
    }

    /// Auto is remembered for the chat, or for the folder, and put back from
    /// there; a chat switched to Auto before it had an id keeps it once saved.
    #[test]
    fn ask_or_auto_is_remembered_and_put_back() {
        crate::testing::with_app_dir("cmd-approval-memory", || {
            let state = state();
            // No folder: it takes effect, and nothing is remembered.
            set_unattended(&state, true, Some("c0")).unwrap();
            assert!(skip_all(&state));
            assert!(!restore_unattended(&state, Some("c0")).unwrap(), "no folder asks");

            *state.workspace.lock().unwrap() = Some(temp_dir("cmd-approval-folder").canonicalize().unwrap());
            set_unattended(&state, true, Some("c1")).unwrap();
            assert!(!restore_unattended(&state, Some("c2")).unwrap());
            assert!(!skip_all(&state), "the other chat asks, and the policy says so");
            assert!(restore_unattended(&state, Some("c1")).unwrap());
            assert!(skip_all(&state));

            // A new chat picked Auto with no id; its first save remembers it.
            set_unattended(&state, true, None).unwrap();
            remember_new_chat(&state, "c3");
            assert!(restore_unattended(&state, Some("c3")).unwrap());
            set_unattended(&state, false, Some("c3")).unwrap();
            remember_new_chat(&state, "c4");
            assert!(!restore_unattended(&state, Some("c4")).unwrap(), "saved under Ask, it asks");

            // Per repository, one choice for every chat, a new one included.
            let mut settings = crate::infra::settings_store::load().unwrap();
            settings.approval.remember = RememberScope::Repository;
            crate::infra::settings_store::save(&settings).unwrap();
            assert!(!restore_unattended(&state, Some("c1")).unwrap(), "the chat's choice is not the folder's");
            set_unattended(&state, true, Some("c1")).unwrap();
            assert!(restore_unattended(&state, None).unwrap());
            let before = crate::infra::settings_store::load().unwrap().approval;
            remember_new_chat(&state, "c5");
            assert_eq!(crate::infra::settings_store::load().unwrap().approval, before, "the folder already holds it");
        });
    }

    /// A saved chat's Auto is kept under the chat's own folder, not the one
    /// the window has active (F-8): per repository, it is that repository's.
    #[test]
    fn a_saved_chats_auto_is_kept_under_its_own_folder() {
        crate::testing::with_app_dir("cmd-approval-chat-folder", || {
            let state = state();
            let plain = |label| crate::infra::paths::canonicalize_plain(&temp_dir(label)).unwrap();
            let (home, active) = (plain("cmd-approval-home"), plain("cmd-approval-active"));
            crate::infra::chat_store::save("saved", &home.display().to_string(), &[], &serde_json::json!([]), &[], None, None, None)
                .unwrap();
            state.set_workspace_for_test(active.clone());
            crate::infra::settings_store::update(|s| s.approval.remember = RememberScope::Repository).unwrap();

            set_unattended(&state, true, Some("saved")).unwrap();

            let approval = crate::infra::settings_store::load().unwrap().approval;
            assert!(approval.is_auto(None, &home.display().to_string()), "kept under the chat's folder");
            assert!(!approval.is_auto(None, &active.display().to_string()), "not under the active one");
            assert!(!restore_unattended(&state, None).unwrap(), "the active folder asks");
            assert!(restore_unattended(&state, Some("saved")).unwrap());
        });
    }

    /// The card's third button. It lasts for the session and is not written
    /// anywhere — a saved "never ask me" is a brake nobody remembers releasing.
    #[test]
    fn always_allow_widens_the_policy_for_that_tool_only() {
        let state = state();
        state
            .approval
            .lock()
            .unwrap()
            .always_allowed
            .insert(ToolName::WriteFile);

        let policy = state.approval().unwrap();
        assert!(!policy.requires_approval(ToolName::WriteFile, true));
        assert!(policy.requires_approval(ToolName::Delete, true));
    }
}
