//! The board on disk: `~/.kibo/board.json`, one for the whole app — the
//! Assistant has no folder, and a card asked for in one chat is wanted in all.

use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::domain::board::{apply, BoardChange, BoardChanged, BoardError, BoardEventSink, Card, TaskBoard};

pub struct Board {
    /// `None` without a home directory: every call then says so.
    path: Option<PathBuf>,
    /// Read, change, write as one step: the model and the page may both be at it.
    lock: Mutex<()>,
    events: BoardEventSink,
}

impl Board {
    pub fn new(path: Option<PathBuf>, events: BoardEventSink) -> Self {
        Self { path, lock: Mutex::new(()), events }
    }

    /// The app directory's board.
    pub fn in_app_dir(events: BoardEventSink) -> Self {
        Self::new(super::app_dir::dir().ok().map(|dir| dir.join("board.json")), events)
    }

    fn path(&self) -> Result<&PathBuf, BoardError> {
        self.path.as_ref().ok_or_else(|| BoardError::Store("there is no app directory to keep it in".into()))
    }

    fn load(&self) -> Result<Vec<Card>, BoardError> {
        let path = self.path()?;
        match fs::read_to_string(path) {
            // A file that does not parse is refused, not replaced: the next
            // write would otherwise drop every card in it.
            Ok(text) => serde_json::from_str(&text).map_err(|e| BoardError::Store(format!("{} does not parse: {e}", path.display()))),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(BoardError::Store(format!("{}: {e}", path.display()))),
        }
    }
}

impl TaskBoard for Board {
    fn cards(&self) -> Result<Vec<Card>, BoardError> {
        let _held = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        self.load()
    }

    fn change(&self, change: &BoardChange) -> Result<Vec<Card>, BoardError> {
        let next = {
            let _held = self.lock.lock().unwrap_or_else(|e| e.into_inner());
            let next = apply(&self.load()?, change)?;
            let json = serde_json::to_vec_pretty(&next).map_err(|e| BoardError::Store(e.to_string()))?;
            super::app_dir::write_private(self.path()?, &json).map_err(BoardError::Store)?;
            next
        };
        (self.events)(BoardChanged);
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::board::Column;
    use crate::testing::temp_dir;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn board(path: PathBuf) -> (Board, Arc<AtomicUsize>) {
        let signals = Arc::new(AtomicUsize::new(0));
        let counted = signals.clone();
        (Board::new(Some(path), Arc::new(move |_| { counted.fetch_add(1, Ordering::SeqCst); })), signals)
    }

    #[test]
    fn a_change_is_kept_on_disk_and_signalled_once() {
        let path = temp_dir("board-store").join("board.json");
        let (first, signals) = board(path.clone());
        assert_eq!(first.cards().unwrap(), []);
        first.change(&BoardChange::Add { title: "file the request".into(), note: None, due: None }).unwrap();
        first.change(&BoardChange::Move { id: "c1".into(), column: Column::Doing, index: None }).unwrap();
        assert_eq!(signals.load(Ordering::SeqCst), 2);

        let (again, _) = board(path);
        let cards = again.cards().unwrap();
        assert_eq!((cards[0].title.as_str(), cards[0].column), ("file the request", Column::Doing));
    }

    #[test]
    fn a_refused_change_writes_nothing_and_signals_nothing() {
        let path = temp_dir("board-store-refused").join("board.json");
        let (board, signals) = board(path.clone());
        assert!(board.change(&BoardChange::Remove { id: "c1".into() }).is_err());
        assert!(!path.exists());
        assert_eq!(signals.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_file_that_does_not_parse_is_left_as_it_is() {
        let path = temp_dir("board-store-broken").join("board.json");
        fs::write(&path, "not json").unwrap();
        let (board, _) = board(path.clone());
        assert!(matches!(board.cards(), Err(BoardError::Store(_))));
        assert!(board.change(&BoardChange::Add { title: "a".into(), note: None, due: None }).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "not json");
    }

    #[test]
    fn without_an_app_directory_it_says_so() {
        let board = Board::new(None, Arc::new(|_| {}));
        assert!(board.cards().unwrap_err().to_string().contains("no app directory"));
    }
}
