//! The Terminal tab's shells.
//!
//! Output does not go on an event. It is the data itself, in bytes, and only
//! the tab drawing that one terminal wants it — so it goes down the `Channel`
//! that tab passed to [`terminal_attach`], for as long as it stays attached.
//! [`TERMINAL_EVENT`] only says the list changed: one opened, ended or closed.

use std::sync::Arc;

use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter, Runtime, State};

use super::agent_state::AgentState;
use crate::domain::terminal::{TerminalChanged, TerminalEventSink, TerminalInfo, TerminalOutputSink, TerminalSize};
use crate::infra::terminal::Terminals;

/// A terminal opened, its shell ended, or it was closed.
pub const TERMINAL_EVENT: &str = "terminals:changed";

pub fn terminal_event_sink<R: Runtime>(app: &AppHandle<R>) -> TerminalEventSink {
    let app = app.clone();
    Arc::new(move |event: TerminalChanged| {
        let _ = app.emit(TERMINAL_EVENT, event);
    })
}

/// Raw, so it arrives as an `ArrayBuffer` for xterm.js to decode — not as a
/// JSON array of numbers, and not as text cut inside a character.
fn channel_screen(channel: Channel) -> TerminalOutputSink {
    Arc::new(move |bytes: &[u8]| {
        // A channel whose tab is gone: its cleanup detaches it.
        let _ = channel.send(InvokeResponseBody::Raw(bytes.to_vec()));
    })
}

fn size(cols: u16, rows: u16) -> Result<TerminalSize, String> {
    TerminalSize::new(cols, rows).map_err(|e| e.to_string())
}

/// In the open folder, at the size the tab measured.
#[tauri::command]
pub fn terminal_open(
    cols: u16,
    rows: u16,
    state: State<'_, Arc<AgentState>>,
    terminals: State<'_, Arc<Terminals>>,
) -> Result<TerminalInfo, String> {
    let size = size(cols, rows)?;
    terminals.open(&state.workspace()?, size).map_err(|e| e.to_string())
}

/// Read when the tab opens and on each [`TERMINAL_EVENT`].
#[tauri::command]
pub fn terminal_list(terminals: State<'_, Arc<Terminals>>) -> Vec<TerminalInfo> {
    terminals.list()
}

/// What it wrote so far, then everything it writes, down `on_output`.
#[tauri::command]
pub fn terminal_attach(id: u32, on_output: Channel, terminals: State<'_, Arc<Terminals>>) -> Result<(), String> {
    terminals.attach(id, on_output.id(), channel_screen(on_output)).map_err(|e| e.to_string())
}

/// `channel` is the id of the one attached: only that one is let go.
#[tauri::command]
pub fn terminal_detach(id: u32, channel: u32, terminals: State<'_, Arc<Terminals>>) -> Result<(), String> {
    terminals.detach(id, channel).map_err(|e| e.to_string())
}

/// What xterm.js says was typed or pasted.
#[tauri::command]
pub fn terminal_write(id: u32, data: String, terminals: State<'_, Arc<Terminals>>) -> Result<(), String> {
    terminals.write(id, data.into_bytes()).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn terminal_resize(id: u32, cols: u16, rows: u16, terminals: State<'_, Arc<Terminals>>) -> Result<(), String> {
    terminals.resize(id, size(cols, rows)?).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn terminal_close(id: u32, terminals: State<'_, Arc<Terminals>>) -> Result<(), String> {
    terminals.close(id).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{mpsc, Mutex};
    use std::time::Duration;
    use tauri::Listener;

    /// `src/lib/terminal.ts` listens on this name.
    #[test]
    fn the_channel_name_is_pinned() {
        assert_eq!(TERMINAL_EVENT, "terminals:changed");
    }

    #[test]
    fn a_change_is_reported_on_the_channel_by_id() {
        let app = tauri::test::mock_app();
        let (tx, rx) = mpsc::channel();
        app.handle().listen(TERMINAL_EVENT, move |event| {
            let _ = tx.send(event.payload().to_string());
        });
        terminal_event_sink(app.handle())(TerminalChanged { id: 4 });
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), r#"{"id":4}"#);
    }

    #[test]
    fn output_goes_down_the_channel_as_raw_bytes() {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let into = Arc::clone(&sent);
        let channel = Channel::new(move |body| {
            into.lock().unwrap().push(body);
            Ok(())
        });
        channel_screen(channel)(b"\x1b[31m\xd0");
        let sent = sent.lock().unwrap();
        match sent.as_slice() {
            [InvokeResponseBody::Raw(bytes)] => assert_eq!(bytes, b"\x1b[31m\xd0"),
            other => panic!("not one raw message: {other:?}"),
        }
    }

    #[test]
    fn a_zero_size_is_refused_before_a_shell_starts() {
        assert_eq!(size(0, 24).unwrap_err(), "a terminal cannot be 0×24");
    }
}
