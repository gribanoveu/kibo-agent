//! The Terminal tab: the background processes, and the user's Stop. The tab
//! reads the list again when [`PROCESS_EVENT`] says one of them changed.

use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime, State};

use crate::domain::background::{BackgroundProcesses, ProcessChanged, ProcessEventSink, ProcessInfo};
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

/// How much of each process's output the tab shows: its last screenfuls.
const TAIL_BYTES: usize = 8 * 1024;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessView {
    #[serde(flatten)]
    pub info: ProcessInfo,
    pub tail: String,
}

/// The open chat's — what another chat started is not this one's to show —
/// or, for `None`, every one: switching folders stops them all.
fn views(processes: &Processes, chat: Option<&str>) -> Vec<ProcessView> {
    let listed = match chat {
        Some(chat) => processes.list_for(chat),
        None => processes.list(),
    };
    // Newest first: the one just started is the one being watched.
    listed
        .into_iter()
        .rev()
        .map(|info| ProcessView { tail: shown_tail(processes, info.id), info })
        .collect()
}

/// As a terminal would show it: a countdown is its last value, not every one.
fn shown_tail(processes: &Processes, id: u32) -> String {
    collapse_redraws(&processes.tail(id, TAIL_BYTES).unwrap_or_default()).into_owned()
}

/// Read when the tab opens and on each [`PROCESS_EVENT`].
#[tauri::command]
pub fn processes_list(chat_id: Option<String>, processes: State<'_, Arc<Processes>>) -> Vec<ProcessView> {
    views(&processes, chat_id.as_deref())
}

/// The ones still running, newest first and without their output: the chat's
/// count, read on each [`PROCESS_EVENT`] — a tick of output costs no tails.
#[tauri::command]
pub fn processes_running(chat_id: String, processes: State<'_, Arc<Processes>>) -> Vec<ProcessInfo> {
    running(&processes, &chat_id)
}

fn running(processes: &Processes, chat: &str) -> Vec<ProcessInfo> {
    processes.list_for(chat).into_iter().rev().filter(ProcessInfo::running).collect()
}

/// The model is told at its next round that the user stopped it.
#[tauri::command]
pub fn process_stop(id: u32, chat_id: String, processes: State<'_, Arc<Processes>>) -> Result<Vec<ProcessView>, String> {
    stop(&processes, id, &chat_id)
}

fn stop(processes: &Processes, id: u32, chat: &str) -> Result<Vec<ProcessView>, String> {
    processes.stop_by_user(id).map_err(|e| e.to_string())?;
    Ok(views(processes, Some(chat)))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
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
        let chat = processes.for_chat("c");
        let dir = crate::testing::temp_dir("cmd-processes");
        chat.start(&Shell::default(), "printf 'zero\\rfirst\\n'; sleep 30", &dir, ".").unwrap();
        chat.start(&Shell::default(), "sleep 30", &dir, "sub").unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !views(&processes, Some("c"))[1].tail.contains("first") {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let shown = views(&processes, Some("c"));
        assert_eq!(shown.iter().map(|v| v.info.id).collect::<Vec<_>>(), [2, 1]);
        assert_eq!(shown[0].info.cwd, "sub");
        let json = serde_json::to_value(&shown[1]).unwrap();
        assert_eq!((json["id"].clone(), json["tail"].clone()), (serde_json::json!(1), serde_json::json!("first\n")));

        processes.for_chat("other").start(&Shell::default(), "sleep 30", &dir, ".").unwrap();
        assert_eq!(views(&processes, Some("c")).len(), 2);
        // Switching folders asks for every one: it stops them all.
        assert_eq!(views(&processes, None).len(), 3);
    }

    #[test]
    fn the_chat_counts_only_what_still_runs() {
        let processes = Arc::new(Processes::default());
        let (chat, other) = (processes.for_chat("c"), processes.for_chat("other"));
        let dir = crate::testing::temp_dir("cmd-processes-running");
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
        processes.for_chat("c").start(&Shell::default(), "sleep 30", &dir, ".").unwrap();
        processes.for_chat("other").start(&Shell::default(), "sleep 30", &dir, ".").unwrap();
        let shown = stop(&processes, 1, "c").unwrap();
        // Answered with the open chat's list, which the tab draws as it is.
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].info.state, crate::domain::background::ProcessState::Stopped);
        assert_eq!(processes.take_ended().len(), 1);
        assert!(stop(&processes, 7, "c").unwrap_err().contains("no background process #7"));
    }
}
