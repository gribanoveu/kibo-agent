//! The Agents tab: every `explore` run, and the user's Stop. The tab reads
//! the list again when [`AGENT_EVENT`] says one of them changed.

use std::sync::Arc;

use tauri::{AppHandle, Emitter, Runtime, State};

use crate::domain::agents::{AgentChanged, AgentEventSink, AgentInfo, Agents};

/// A helper agent started, took a step, spent tokens or ended.
pub const AGENT_EVENT: &str = "agents:changed";

pub fn agent_event_sink<R: Runtime>(app: &AppHandle<R>) -> AgentEventSink {
    let app = app.clone();
    Arc::new(move |event: AgentChanged| {
        let _ = app.emit(AGENT_EVENT, event);
    })
}

/// The open chat's, newest first: the one just started is the one being watched.
fn newest_first(agents: &Agents, chat: &str) -> Vec<AgentInfo> {
    agents.list_for(chat).into_iter().rev().collect()
}

/// Read when the tab opens and on each [`AGENT_EVENT`]. The chat's stored
/// runs are read too, hence off the main thread.
#[tauri::command]
pub async fn agents_list(chat_id: String, agents: State<'_, Arc<Agents>>) -> Result<Vec<AgentInfo>, String> {
    let agents = Arc::clone(&agents);
    super::blocking(move || Ok::<_, String>(newest_first(&agents, &chat_id))).await
}

/// The helper ends at its next check; the turn that started it reads why.
#[tauri::command]
pub async fn agent_stop(id: u32, chat_id: String, agents: State<'_, Arc<Agents>>) -> Result<Vec<AgentInfo>, String> {
    let agents = Arc::clone(&agents);
    super::blocking(move || stop(&agents, id, &chat_id)).await
}

fn stop(agents: &Agents, id: u32, chat: &str) -> Result<Vec<AgentInfo>, String> {
    agents.stop(id).map_err(|e| e.to_string())?;
    Ok(newest_first(agents, chat))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;
    use tauri::Listener;

    /// `src/lib/chat.ts` listens on this name.
    #[test]
    fn the_channel_name_is_pinned() {
        assert_eq!(AGENT_EVENT, "agents:changed");
    }

    #[test]
    fn a_run_is_reported_on_the_channel_by_id() {
        let app = tauri::test::mock_app();
        let (tx, rx) = mpsc::channel();
        app.handle().listen(AGENT_EVENT, move |event| {
            let _ = tx.send(event.payload().to_string());
        });
        Agents::new(agent_event_sink(app.handle())).start("where is X");
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), r#"{"id":1}"#);
    }

    #[test]
    fn the_tab_lists_newest_first_and_stops_by_id() {
        let agents = Agents::default();
        let chat = agents.for_chat("c");
        chat.start("first");
        agents.for_chat("other").start("elsewhere");
        chat.start("second");
        assert_eq!(newest_first(&agents, "c").iter().map(|a| a.task.as_str()).collect::<Vec<_>>(), ["second", "first"]);
        assert_eq!(stop(&agents, 1, "c").unwrap().len(), 2);
        assert!(agents.stop_asked(1) && !agents.stop_asked(3));
        assert!(stop(&agents, 7, "c").unwrap_err().contains("no helper agent #7"));
    }
}
