//! The Terminal tab: the background processes, and the user's Stop. The tab
//! reads the list again when [`PROCESS_EVENT`] says one of them changed.

use std::sync::Arc;

use tauri::{AppHandle, Emitter, Runtime, State};

use super::agent_state::AgentState;
use super::blocking;
use crate::domain::background::{ProcessChanged, ProcessEventSink, ProcessInfo, ProcessView};
use crate::domain::command_exec::collapse_redraws;
use crate::infra::background::Processes;

/// A background process started, wrote, ended or was stopped.
pub const PROCESS_EVENT: &str = "processes:changed";

pub fn process_event_sink<R: Runtime>(app: &AppHandle<R>) -> ProcessEventSink {
    let app = app.clone();
    Arc::new(move |event: ProcessChanged| {
        let _ = app.emit(PROCESS_EVENT, event);
    })
}

/// Whose processes a list is of.
enum Whose<'a> {
    /// The open chat's, stored ones included — what another chat started is
    /// not this one's to show.
    Chat(&'a str),
    /// Every one of the folder running or kept since launch: what closing it
    /// stops (F-8.3).
    Folder(&'a std::path::Path),
}

fn views(processes: &Processes, whose: Whose) -> Vec<ProcessView> {
    let listed = match whose {
        Whose::Chat(chat) => processes.list_for(chat),
        Whose::Folder(root) => processes.list_in(root),
    };
    // Newest first: the one just started is the one being watched. The tail
    // as a terminal would show it: a countdown is its last value, not every one.
    listed
        .into_iter()
        .rev()
        .map(|view| ProcessView { tail: collapse_redraws(&view.tail).into_owned(), info: view.info })
        .collect()
}

/// Read when the tab opens and on each [`PROCESS_EVENT`]: chat `chat_id`'s,
/// or with none, the folder `workspace`'s — checked against the open set (F-8
/// invariant 1). Neither is an empty list.
#[tauri::command]
pub async fn processes_list(
    chat_id: Option<String>,
    workspace: Option<String>,
    state: State<'_, Arc<AgentState>>,
    processes: State<'_, Arc<Processes>>,
) -> Result<Vec<ProcessView>, String> {
    let root = if chat_id.is_none() { state.ui_workspace(&workspace)? } else { None };
    let processes = Arc::clone(&processes);
    blocking(move || {
        Ok::<_, String>(match (&chat_id, &root) {
            (Some(chat), _) => views(&processes, Whose::Chat(chat)),
            (None, Some(root)) => views(&processes, Whose::Folder(root)),
            (None, None) => Vec::new(),
        })
    })
    .await
}

/// The ones still running, newest first and without their output: the chat's
/// count, read on each [`PROCESS_EVENT`] — a tick of output costs no tails.
#[tauri::command]
pub fn processes_running(chat_id: String, processes: State<'_, Arc<Processes>>) -> Vec<ProcessInfo> {
    running(&processes, &chat_id)
}

fn running(processes: &Processes, chat: &str) -> Vec<ProcessInfo> {
    processes.running_for(chat).into_iter().rev().collect()
}

/// The model is told at its next round that the user stopped it.
#[tauri::command]
pub async fn process_stop(id: u32, chat_id: String, processes: State<'_, Arc<Processes>>) -> Result<Vec<ProcessView>, String> {
    let processes = Arc::clone(&processes);
    blocking(move || stop(&processes, id, &chat_id)).await
}

fn stop(processes: &Processes, id: u32, chat: &str) -> Result<Vec<ProcessView>, String> {
    processes.stop_by_user(id).map_err(|e| e.to_string())?;
    Ok(views(processes, Whose::Chat(chat)))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::domain::background::BackgroundProcesses;
    use crate::domain::command_exec::Shell;
    use std::sync::mpsc;
    use std::time::Duration;
    use tauri::Listener;

    /// `src/lib/chat.ts` listens on this name.
    #[test]
    fn the_channel_name_is_pinned() {
        assert_eq!(PROCESS_EVENT, "processes:changed");
    }

    #[test]
    fn a_process_is_reported_on_the_channel_by_id() {
        let app = tauri::test::mock_app();
        let (tx, rx) = mpsc::channel();
        app.handle().listen(PROCESS_EVENT, move |event| {
            let _ = tx.send(event.payload().to_string());
        });
        let processes = Processes::new(process_event_sink(app.handle()));
        processes.start(&Shell::default(), "sleep 30", &crate::testing::temp_dir("cmd-processes-event"), ".").unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), r#"{"id":1}"#);
    }

    #[test]
    fn the_tab_lists_newest_first_with_what_each_wrote() {
        let processes = Arc::new(Processes::default());
        let dir = crate::testing::temp_dir("cmd-processes");
        let chat = processes.for_chat("c", &dir);
        chat.start(&Shell::default(), "printf 'zero\\rfirst\\n'; sleep 30", &dir, ".").unwrap();
        chat.start(&Shell::default(), "sleep 30", &dir, "sub").unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !views(&processes, Whose::Chat("c"))[1].tail.contains("first") {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let shown = views(&processes, Whose::Chat("c"));
        assert_eq!(shown.iter().map(|v| v.info.id).collect::<Vec<_>>(), [2, 1]);
        assert_eq!(shown[0].info.cwd, "sub");
        let json = serde_json::to_value(&shown[1]).unwrap();
        assert_eq!((json["id"].clone(), json["tail"].clone()), (serde_json::json!(1), serde_json::json!("first\n")));

        processes.for_chat("other", &dir).start(&Shell::default(), "sleep 30", &dir, ".").unwrap();
        let elsewhere = crate::testing::temp_dir("cmd-processes-elsewhere");
        processes.for_chat("far", &elsewhere).start(&Shell::default(), "sleep 30", &elsewhere, ".").unwrap();
        assert_eq!(views(&processes, Whose::Chat("c")).len(), 2);
        // Without a chat, the folder's — what closing it stops — and not another's.
        assert_eq!(views(&processes, Whose::Folder(&dir)).len(), 3);
        processes.stop_all();
    }

    #[test]
    fn the_chat_counts_only_what_still_runs() {
        let processes = Arc::new(Processes::default());
        let dir = crate::testing::temp_dir("cmd-processes-running");
        let (chat, other) = (processes.for_chat("c", &dir), processes.for_chat("other", &dir));
        chat.start(&Shell::default(), "sleep 30", &dir, ".").unwrap();
        chat.start(&Shell::default(), "sleep 30", &dir, ".").unwrap();
        chat.start(&Shell::default(), "sleep 30", &dir, ".").unwrap();
        other.start(&Shell::default(), "sleep 30", &dir, ".").unwrap();
        processes.stop(2).unwrap();
        assert_eq!(running(&processes, "c").iter().map(|p| p.id).collect::<Vec<_>>(), [3, 1]);
    }

    /// The tab's Stop is the user's: the model hears of it at its next round.
    #[test]
    fn a_stop_from_the_tab_is_told_to_the_model() {
        let processes = Arc::new(Processes::default());
        let dir = crate::testing::temp_dir("cmd-processes-stop");
        processes.for_chat("c", &dir).start(&Shell::default(), "sleep 30", &dir, ".").unwrap();
        processes.for_chat("other", &dir).start(&Shell::default(), "sleep 30", &dir, ".").unwrap();
        let shown = stop(&processes, 1, "c").unwrap();
        // Answered with the open chat's list, which the tab draws as it is.
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].info.state, crate::domain::background::ProcessState::Stopped);
        assert_eq!(processes.take_ended().len(), 1);
        assert!(stop(&processes, 7, "c").unwrap_err().contains("no background process #7"));
    }
}
