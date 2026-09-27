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

/// Newest first: the one just started is the one being watched.
fn newest_first(agents: &Agents) -> Vec<AgentInfo> {
    agents.list().into_iter().rev().collect()
}

/// Read when the tab opens and on each [`AGENT_EVENT`].
#[tauri::command]
pub fn agents_list(agents: State<'_, Arc<Agents>>) -> Vec<AgentInfo> {
    newest_first(&agents)
}

/// The helper ends at its next check; the turn that started it reads why.
#[tauri::command]
pub fn agent_stop(id: u32, agents: State<'_, Arc<Agents>>) -> Result<Vec<AgentInfo>, String> {
    stop(&agents, id)
}

fn stop(agents: &Agents, id: u32) -> Result<Vec<AgentInfo>, String> {
    agents.stop(id).map_err(|e| e.to_string())?;
    Ok(newest_first(agents))
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
        agents.start("first");
        agents.start("second");
        assert_eq!(newest_first(&agents).iter().map(|a| a.task.as_str()).collect::<Vec<_>>(), ["second", "first"]);
        stop(&agents, 1).unwrap();
        assert!(agents.stop_asked(1) && !agents.stop_asked(2));
        assert!(stop(&agents, 7).unwrap_err().contains("no helper agent #7"));
    }
}
