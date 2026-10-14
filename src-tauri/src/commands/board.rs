//! The Board page: the cards the Assistant keeps, and the user's moves of
//! them. The page reads the board again when [`BOARD_EVENT`] says it changed.

use std::sync::Arc;

use tauri::{AppHandle, Emitter, Runtime, State};

use crate::domain::board::{BoardChange, BoardChanged, BoardEventSink, Card, TaskBoard};
use crate::infra::board_store::Board;

/// A card added, moved or removed — by the model or on the page.
pub const BOARD_EVENT: &str = "board:changed";

pub fn board_event_sink<R: Runtime>(app: &AppHandle<R>) -> BoardEventSink {
    let app = app.clone();
    Arc::new(move |event: BoardChanged| {
        let _ = app.emit(BOARD_EVENT, event);
    })
}

#[tauri::command]
pub fn board_list(board: State<'_, Arc<Board>>) -> Result<Vec<Card>, String> {
    board.cards().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn board_change(change: BoardChange, board: State<'_, Arc<Board>>) -> Result<Vec<Card>, String> {
    board.change(&change).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;
    use tauri::Listener;

    #[test]
    fn the_channel_name_is_the_one_the_frontend_listens_on() {
        assert_eq!(BOARD_EVENT, "board:changed");
    }

    #[test]
    fn a_change_reaches_the_window_as_the_event() {
        let app = tauri::test::mock_app();
        let (tx, rx) = mpsc::channel();
        app.handle().listen(BOARD_EVENT, move |event| {
            let _ = tx.send(event.payload().to_string());
        });
        let path = crate::testing::temp_dir("board-command").join("board.json");
        Board::new(Some(path), board_event_sink(app.handle()))
            .change(&BoardChange::Add { title: "a".into(), note: None })
            .unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "null");
    }
}
