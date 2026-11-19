//! The board on disk: `~/.kibo/board.db`, one for the whole app — the
//! Assistant has no folder, and a card asked for in one chat is wanted in all.
//!
//! A card is a row, its tags and comments rows of their own, so the board
//! reads with plain SQL. A board kept before it was a database, `board.json`
//! beside it, is taken in on the first read and renamed `board.json.imported`.

use std::collections::HashMap;
use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::Mutex;

use rusqlite::{params, Connection, Transaction};

use crate::domain::board::{apply, with_keys, BoardChange, BoardChanged, BoardError, BoardEventSink, Card, Column, Comment, TaskBoard};

const FILE: &str = "board.db";
/// Where the board was kept before, and what it is renamed once taken in.
const OLD_FILE: &str = "board.json";
const IMPORTED: &str = "board.json.imported";

const SCHEMA: &str = "
PRAGMA journal_mode = WAL;
PRAGMA busy_timeout = 3000;
CREATE TABLE IF NOT EXISTS cards (
  id       TEXT PRIMARY KEY,
  position INTEGER NOT NULL,
  title    TEXT NOT NULL,
  note     TEXT,
  due      TEXT,
  status   TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS card_tags (
  card_id  TEXT NOT NULL,
  position INTEGER NOT NULL,
  tag      TEXT NOT NULL,
  PRIMARY KEY (card_id, position)
);
CREATE TABLE IF NOT EXISTS card_comments (
  card_id TEXT NOT NULL,
  id      INTEGER NOT NULL,
  text    TEXT NOT NULL,
  at      TEXT NOT NULL,
  PRIMARY KEY (card_id, id)
);
";

pub struct Board {
    /// The folder the database is in. `None` without a home directory: every
    /// call then says so.
    dir: Option<PathBuf>,
    /// Read, change, write as one step: the model and the page may both be at it.
    lock: Mutex<()>,
    events: BoardEventSink,
}

fn store(e: impl std::fmt::Display) -> BoardError {
    BoardError::Store(e.to_string())
}

fn status(column: Column) -> &'static str {
    match column {
        Column::Todo => "todo",
        Column::Doing => "doing",
        Column::Done => "done",
        Column::Archived => "archived",
    }
}

fn column(status: &str) -> Result<Column, BoardError> {
    [Column::Todo, Column::Doing, Column::Done, Column::Archived]
        .into_iter()
        .find(|c| self::status(*c) == status)
        .ok_or_else(|| BoardError::Store(format!("a card has the status {status:?}, which the board does not know")))
}

impl Board {
    pub fn new(dir: Option<PathBuf>, events: BoardEventSink) -> Self {
        Self { dir, lock: Mutex::new(()), events }
    }

    /// The app directory's board.
    pub fn in_app_dir(events: BoardEventSink) -> Self {
        Self::new(super::app_dir::dir().ok(), events)
    }

    fn open(&self) -> Result<Connection, BoardError> {
        let dir = self.dir.as_ref().ok_or_else(|| BoardError::Store("there is no app directory to keep it in".into()))?;
        fs::create_dir_all(dir).map_err(|e| BoardError::Store(format!("{}: {e}", dir.display())))?;
        let path = dir.join(FILE);
        let mut conn = Connection::open(&path).map_err(store)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
        }
        conn.execute_batch(SCHEMA).map_err(store)?;
        self.take_in_old_file(&mut conn)?;
        Ok(conn)
    }

    /// The JSON board, into an empty database, then renamed, so a board
    /// emptied later is not filled again from it. Committed before the rename:
    /// a rename that fails leaves the cards in a database the next open finds
    /// filled, and the file is only put aside then. A file that does not parse
    /// is refused and left as it is — the next write would drop its cards.
    fn take_in_old_file(&self, conn: &mut Connection) -> Result<(), BoardError> {
        let Some(dir) = self.dir.as_ref() else { return Ok(()) };
        let old = dir.join(OLD_FILE);
        let text = match fs::read_to_string(&old) {
            Ok(text) => text,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(BoardError::Store(format!("{}: {e}", old.display()))),
        };
        let tx = conn.transaction().map_err(store)?;
        let empty = tx.query_row("SELECT COUNT(*) FROM cards", [], |r| r.get::<_, i64>(0)).map_err(store)? == 0;
        if empty {
            let cards = serde_json::from_str(&text).map(with_keys).map_err(|e| BoardError::Store(format!("{} does not parse: {e}", old.display())))?;
            write(&tx, &cards)?;
        }
        tx.commit().map_err(store)?;
        fs::rename(&old, dir.join(IMPORTED)).map_err(|e| BoardError::Store(format!("{}: {e}", old.display())))
    }
}

/// Every file the board may have in the app directory. The journal before the
/// database: a journal left behind by a failed delete would otherwise be
/// replayed into the next, empty one.
const FILES: [&str; 5] = ["board.db-wal", "board.db-shm", FILE, OLD_FILE, IMPORTED];

/// Whether `dir` holds any of the board — a `board.json` not yet taken in counts.
pub fn has_data(dir: &std::path::Path) -> bool {
    FILES.iter().any(|file| dir.join(file).exists())
}

/// The board's files in `dir`, gone: the database with its journal, and a
/// board kept before it, taken in or not. For the Catalog's "Delete data",
/// while the feature is off — nothing holds the database open between calls.
pub fn delete(dir: &std::path::Path) -> Result<(), BoardError> {
    for file in FILES {
        let path = dir.join(file);
        match fs::remove_file(&path) {
            Err(e) if e.kind() != ErrorKind::NotFound => return Err(BoardError::Store(format!("could not delete {}: {e}", path.display()))),
            _ => {}
        }
    }
    Ok(())
}

fn read(conn: &Connection) -> Result<Vec<Card>, BoardError> {
    let mut tags: HashMap<String, Vec<String>> = HashMap::new();
    let mut rows = conn.prepare("SELECT card_id, tag FROM card_tags ORDER BY card_id, position").map_err(store)?;
    for row in rows.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).map_err(store)? {
        let (card, tag) = row.map_err(store)?;
        tags.entry(card).or_default().push(tag);
    }
    // Oldest first: a comment is numbered on from the highest on its card.
    let mut comments: HashMap<String, Vec<Comment>> = HashMap::new();
    let mut rows = conn.prepare("SELECT card_id, id, text, at FROM card_comments ORDER BY card_id, id").map_err(store)?;
    for row in rows
        .query_map([], |r| Ok((r.get::<_, String>(0)?, Comment { id: r.get(1)?, text: r.get(2)?, at: r.get(3)? })))
        .map_err(store)?
    {
        let (card, comment) = row.map_err(store)?;
        comments.entry(card).or_default().push(comment);
    }
    let mut rows = conn.prepare("SELECT id, title, note, due, status FROM cards ORDER BY position").map_err(store)?;
    let found = rows
        .query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, String>(4)?))
        })
        .map_err(store)?;
    let mut cards = Vec::new();
    for row in found {
        let (id, title, note, due, status) = row.map_err(store)?;
        cards.push(Card {
            column: column(&status)?,
            comments: comments.remove(&id).unwrap_or_default(),
            tags: tags.remove(&id).unwrap_or_default(),
            id,
            title,
            note,
            due,
        });
    }
    Ok(cards)
}

/// The whole board, in place of what was there.
// ponytail: rewrites every row on each change — a board holds a few hundred
// cards at most (MAX_CARDS plus the archive); write only what changed if that grows.
fn write(tx: &Transaction, cards: &[Card]) -> Result<(), BoardError> {
    tx.execute_batch("DELETE FROM card_comments; DELETE FROM card_tags; DELETE FROM cards;").map_err(store)?;
    let mut card_row = tx.prepare("INSERT INTO cards (id, position, title, note, due, status) VALUES (?1, ?2, ?3, ?4, ?5, ?6)").map_err(store)?;
    let mut tag_row = tx.prepare("INSERT INTO card_tags (card_id, position, tag) VALUES (?1, ?2, ?3)").map_err(store)?;
    let mut comment_row = tx.prepare("INSERT INTO card_comments (card_id, id, text, at) VALUES (?1, ?2, ?3, ?4)").map_err(store)?;
    for (position, card) in cards.iter().enumerate() {
        card_row.execute(params![card.id, position as i64, card.title, card.note, card.due, status(card.column)]).map_err(store)?;
        for (position, tag) in card.tags.iter().enumerate() {
            tag_row.execute(params![card.id, position as i64, tag]).map_err(store)?;
        }
        for comment in &card.comments {
            comment_row.execute(params![card.id, comment.id, comment.text, comment.at]).map_err(store)?;
        }
    }
    Ok(())
}

impl TaskBoard for Board {
    fn cards(&self) -> Result<Vec<Card>, BoardError> {
        let _held = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        read(&self.open()?)
    }

    fn change(&self, change: &BoardChange) -> Result<Vec<Card>, BoardError> {
        let next = {
            let _held = self.lock.lock().unwrap_or_else(|e| e.into_inner());
            let mut conn = self.open()?;
            let tx = conn.transaction().map_err(store)?;
            let next = apply(&read(&tx)?, change)?;
            write(&tx, &next)?;
            tx.commit().map_err(store)?;
            next
        };
        (self.events)(BoardChanged);
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::temp_dir;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn board(dir: PathBuf) -> (Board, Arc<AtomicUsize>) {
        let signals = Arc::new(AtomicUsize::new(0));
        let counted = signals.clone();
        (Board::new(Some(dir), Arc::new(move |_| { counted.fetch_add(1, Ordering::SeqCst); })), signals)
    }

    fn add(title: &str) -> BoardChange {
        BoardChange::Add { title: title.into(), note: None, due: None, tags: vec![] }
    }

    #[test]
    fn a_change_is_kept_on_disk_and_signalled_once() {
        let dir = temp_dir("board-store");
        let (first, signals) = board(dir.clone());
        assert_eq!(first.cards().unwrap(), []);
        first.change(&add("file the request")).unwrap();
        first.change(&BoardChange::Move { id: "T1".into(), column: Column::Doing, index: None }).unwrap();
        assert_eq!(signals.load(Ordering::SeqCst), 2);

        let (again, _) = board(dir);
        let cards = again.cards().unwrap();
        assert_eq!((cards[0].title.as_str(), cards[0].column), ("file the request", Column::Doing));
    }

    /// Every field, the order of the cards, their tags and their comments
    /// read back as they were written — each column has a way to come out wrong.
    #[test]
    fn a_board_reads_back_as_it_was_written() {
        let dir = temp_dir("board-store-whole");
        let (board, _) = board(dir.clone());
        board.change(&add("first")).unwrap();
        board
            .change(&BoardChange::Add { title: "second".into(), note: Some("why".into()), due: Some("2026-12-01".into()), tags: vec!["Kibo".into(), "home".into()] })
            .unwrap();
        board.change(&BoardChange::Move { id: "T1".into(), column: Column::Archived, index: None }).unwrap();
        for text in ["one", "two", "three"] {
            board.change(&BoardChange::Comment { id: "T2".into(), text: text.into(), at: "2026-11-19T10:00:00Z".into() }).unwrap();
        }
        board.change(&BoardChange::Uncomment { id: "T2".into(), comment: 2 }).unwrap();
        let written = board.change(&BoardChange::Tags { id: "T2".into(), tags: vec!["b".into(), "a".into(), "c".into()] }).unwrap();

        let (again, _) = self::board(dir);
        let read = again.cards().unwrap();
        assert_eq!(read, written);
        assert_eq!(read.iter().map(|c| (c.id.as_str(), c.column)).collect::<Vec<_>>(), [("T2", Column::Todo), ("T1", Column::Archived)]);
        assert_eq!((read[0].note.as_deref(), read[0].due.as_deref()), (Some("why"), Some("2026-12-01")));
        assert_eq!(read[0].tags, ["b", "a", "c"]);
        assert_eq!(read[0].comments.iter().map(|c| (c.id, c.text.as_str())).collect::<Vec<_>>(), [(1, "one"), (3, "three")]);
        assert!(read[1].tags.is_empty() && read[1].comments.is_empty());
    }

    #[test]
    fn a_json_board_is_taken_in_once_with_t_keys_and_renamed() {
        let dir = temp_dir("board-store-import");
        fs::write(
            dir.join(OLD_FILE),
            r#"[{"id":"c2","title":"a","column":"todo","tags":["x"],"comments":[{"id":1,"text":"hi","at":"2026-11-01T00:00:00Z"}]},{"id":"c5","title":"b","column":"doing"}]"#,
        )
        .unwrap();
        let (board, _) = board(dir.clone());
        let cards = board.cards().unwrap();
        assert_eq!(cards.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["T2", "T5"]);
        assert_eq!((cards[0].tags.as_slice(), cards[0].comments[0].text.as_str()), (["x".to_string()].as_slice(), "hi"));
        assert!(!dir.join(OLD_FILE).exists() && dir.join(IMPORTED).exists());

        // Emptied, it stays empty: the file is not read again.
        for id in ["T2", "T5"] {
            board.change(&BoardChange::Remove { id: id.into() }).unwrap();
        }
        assert_eq!(board.cards().unwrap(), []);
    }

    /// A `board.json` that turns up beside a board already in the database
    /// is put aside, not merged in.
    #[test]
    fn a_json_board_beside_a_filled_database_does_not_replace_it() {
        let dir = temp_dir("board-store-both");
        let (board, _) = board(dir.clone());
        board.change(&add("kept")).unwrap();
        fs::write(dir.join(OLD_FILE), r#"[{"id":"T9","title":"old","column":"todo"}]"#).unwrap();
        assert_eq!(board.cards().unwrap().iter().map(|c| c.title.as_str()).collect::<Vec<_>>(), ["kept"]);
        assert!(!dir.join(OLD_FILE).exists());
    }

    #[test]
    fn a_refused_change_writes_nothing_and_signals_nothing() {
        let dir = temp_dir("board-store-refused");
        let (board, signals) = board(dir);
        board.change(&add("a")).unwrap();
        let before = board.cards().unwrap();
        assert!(board.change(&BoardChange::Remove { id: "T7".into() }).is_err());
        assert!(board.change(&BoardChange::Comment { id: "T1".into(), text: " ".into(), at: "2026-11-19T10:00:00Z".into() }).is_err());
        assert_eq!(board.cards().unwrap(), before);
        assert_eq!(signals.load(Ordering::SeqCst), 1);
    }

    /// The rename failing — here a folder already where the file would go —
    /// comes after the cards are in: the next open finds them, and the board
    /// does not come up empty for good.
    #[test]
    fn a_rename_that_fails_after_the_import_keeps_the_cards() {
        let dir = temp_dir("board-store-rename");
        fs::write(dir.join(OLD_FILE), r#"[{"id":"T1","title":"kept","column":"todo"}]"#).unwrap();
        fs::create_dir_all(dir.join(IMPORTED).join("in the way")).unwrap();
        let (board, _) = board(dir.clone());
        assert!(board.cards().is_err());
        let held: i64 = Connection::open(dir.join(FILE)).unwrap().query_row("SELECT COUNT(*) FROM cards", [], |r| r.get(0)).unwrap();
        assert_eq!(held, 1);
    }

    #[test]
    fn has_data_counts_every_file_of_the_board() {
        let dir = temp_dir("board-store-has-data");
        assert!(!has_data(&dir));
        for file in FILES {
            fs::write(dir.join(file), "").unwrap();
            assert!(has_data(&dir), "{file}");
            fs::remove_file(dir.join(file)).unwrap();
        }
    }

    #[test]
    fn a_json_file_that_does_not_parse_is_left_as_it_is() {
        let dir = temp_dir("board-store-broken");
        fs::write(dir.join(OLD_FILE), "not json").unwrap();
        let (board, _) = board(dir.clone());
        assert!(matches!(board.cards(), Err(BoardError::Store(_))));
        assert!(board.change(&add("a")).is_err());
        assert_eq!(fs::read_to_string(dir.join(OLD_FILE)).unwrap(), "not json");
        assert!(!dir.join(IMPORTED).exists());
    }

    #[test]
    fn a_status_the_board_does_not_know_is_refused() {
        let dir = temp_dir("board-store-status");
        let (board, _) = board(dir.clone());
        board.change(&add("a")).unwrap();
        Connection::open(dir.join(FILE)).unwrap().execute("UPDATE cards SET status = 'later'", []).unwrap();
        assert!(board.cards().unwrap_err().to_string().contains("later"));
    }

    #[test]
    fn without_an_app_directory_it_says_so() {
        let board = Board::new(None, Arc::new(|_| {}));
        assert!(board.cards().unwrap_err().to_string().contains("no app directory"));
    }
}
