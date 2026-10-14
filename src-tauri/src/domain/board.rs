//! The board: what the user asked the Assistant to keep for later — "remind
//! me to file the memory request" — as cards in three columns, and the user's
//! own moves of them on the Board page. Done holds the last few finished; the
//! ones before go to the archive, which the page shows on request.
//!
//! The rules live here, the file in `infra::board_store`: one [`apply`] for
//! both the model's `board` tool and the page, so neither can do what the
//! other refuses.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Past this the board is a backlog nobody reads, and the model's `list`
/// would cost every turn that calls it. The archive does not count.
pub const MAX_CARDS: usize = 200;
/// Done shows what was just finished; a sixth sends the oldest to the archive.
pub const MAX_DONE: usize = 5;
/// The archive keeps the latest; the oldest past this are dropped.
pub const MAX_ARCHIVED: usize = 100;
pub const MAX_TITLE_CHARS: usize = 200;
pub const MAX_NOTE_CHARS: usize = 2000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Column {
    #[default]
    Todo,
    Doing,
    Done,
    Archived,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Card {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default)]
    pub column: Column,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum BoardChange {
    Add { title: String, note: Option<String> },
    Move { id: String, column: Column },
    /// The page's only: the model moves a card to Done instead. From any
    /// column, the archive's too.
    Remove { id: String },
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum BoardError {
    #[error("no card with id {id} — the board has: {}", if .ids.is_empty() { "no cards".to_string() } else { .ids.join(", ") })]
    NotFound { id: String, ids: Vec<String> },
    #[error("a card needs a title")]
    EmptyTitle,
    #[error("the title is {0} characters; keep it under {MAX_TITLE_CHARS} and put the rest in the note")]
    TitleTooLong(usize),
    #[error("the note is {0} characters; keep it under {MAX_NOTE_CHARS}")]
    NoteTooLong(usize),
    #[error("the board is full ({MAX_CARDS} cards) — ask the user to remove the ones no longer needed on the Board page")]
    Full,
    #[error("the board could not be read or saved: {0}")]
    Store(String),
}

/// The board changed — by the model or on the page. A signal to read it again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct BoardChanged;

pub type BoardEventSink = Arc<dyn Fn(BoardChanged) + Send + Sync>;

/// Where the board is kept: a port, so a tool test holds one in memory.
pub trait TaskBoard: Send + Sync {
    fn cards(&self) -> Result<Vec<Card>, BoardError>;
    /// Applies `change` and returns the board after it.
    fn change(&self, change: &BoardChange) -> Result<Vec<Card>, BoardError>;
}

pub fn apply(cards: &[Card], change: &BoardChange) -> Result<Vec<Card>, BoardError> {
    let mut next = cards.to_vec();
    match change {
        BoardChange::Add { title, note } => {
            let title = title.trim();
            if title.is_empty() {
                return Err(BoardError::EmptyTitle);
            }
            let title_chars = title.chars().count();
            if title_chars > MAX_TITLE_CHARS {
                return Err(BoardError::TitleTooLong(title_chars));
            }
            let note = note.as_deref().map(str::trim).filter(|n| !n.is_empty());
            if let Some(chars) = note.map(|n| n.chars().count()).filter(|&c| c > MAX_NOTE_CHARS) {
                return Err(BoardError::NoteTooLong(chars));
            }
            if cards.iter().filter(|c| c.column != Column::Archived).count() >= MAX_CARDS {
                return Err(BoardError::Full);
            }
            next.push(Card { id: next_id(cards), title: title.to_string(), note: note.map(str::to_string), column: Column::Todo });
        }
        BoardChange::Move { id, column } => {
            let at = position(cards, id)?;
            let mut card = next.remove(at);
            // Done keeps the order cards arrived in, so its first is the one
            // the archive takes first.
            let arrives = card.column != *column && *column == Column::Done;
            card.column = *column;
            if arrives {
                next.push(card);
            } else {
                next.insert(at, card);
            }
        }
        BoardChange::Remove { id } => {
            let at = position(cards, id)?;
            next.remove(at);
        }
    }
    Ok(settle(next))
}

/// Done down to [`MAX_DONE`], the oldest to the archive; the archive down to
/// [`MAX_ARCHIVED`], the oldest dropped.
fn settle(mut cards: Vec<Card>) -> Vec<Card> {
    let count = |cards: &[Card], column| cards.iter().filter(|c| c.column == column).count();
    while count(&cards, Column::Done) > MAX_DONE {
        let Some(at) = cards.iter().position(|c| c.column == Column::Done) else { break };
        let mut card = cards.remove(at);
        card.column = Column::Archived;
        cards.push(card);
    }
    while count(&cards, Column::Archived) > MAX_ARCHIVED {
        let Some(at) = cards.iter().position(|c| c.column == Column::Archived) else { break };
        cards.remove(at);
    }
    cards
}

/// Numbered on from the highest id there is, like the checklist: an id is not
/// handed out twice while its card is on the board. Once the card is removed
/// it may be — the board, unlike a turn, has no history that names it.
fn next_id(cards: &[Card]) -> String {
    let last = cards.iter().filter_map(|c| c.id.strip_prefix('c')?.parse::<u32>().ok()).max().unwrap_or(0);
    format!("c{}", last + 1)
}

fn position(cards: &[Card], id: &str) -> Result<usize, BoardError> {
    cards.iter().position(|c| c.id == id).ok_or_else(|| BoardError::NotFound {
        id: id.to_string(),
        ids: cards.iter().map(|c| c.id.clone()).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn add(cards: &[Card], title: &str) -> Vec<Card> {
        apply(cards, &BoardChange::Add { title: title.into(), note: None }).unwrap()
    }

    #[test]
    fn an_added_card_is_todo_with_the_next_id_and_trimmed_text() {
        let one = apply(&[], &BoardChange::Add { title: "  file the memory request  ".into(), note: Some("   ".into()) }).unwrap();
        let two = apply(&one, &BoardChange::Add { title: "renew cert".into(), note: Some("  wlbuh  ".into()) }).unwrap();
        assert_eq!(
            two,
            [
                Card { id: "c1".into(), title: "file the memory request".into(), note: None, column: Column::Todo },
                Card { id: "c2".into(), title: "renew cert".into(), note: Some("wlbuh".into()), column: Column::Todo },
            ]
        );
    }

    #[test]
    fn ids_go_on_from_the_highest_even_after_a_removal_in_the_middle() {
        let three = add(&add(&add(&[], "a"), "b"), "c");
        let without_b = apply(&three, &BoardChange::Remove { id: "c2".into() }).unwrap();
        assert_eq!(add(&without_b, "d").last().unwrap().id, "c4");
    }

    #[test]
    fn a_move_changes_only_that_cards_column() {
        let two = add(&add(&[], "a"), "b");
        let moved = apply(&two, &BoardChange::Move { id: "c2".into(), column: Column::Done }).unwrap();
        assert_eq!(moved.iter().map(|c| c.column).collect::<Vec<_>>(), [Column::Todo, Column::Done]);
    }

    #[test]
    fn an_unknown_id_names_the_ones_there_are() {
        let two = add(&add(&[], "a"), "b");
        let err = apply(&two, &BoardChange::Move { id: "c9".into(), column: Column::Done }).unwrap_err();
        assert_eq!(err.to_string(), "no card with id c9 — the board has: c1, c2");
        let err = apply(&[], &BoardChange::Remove { id: "c1".into() }).unwrap_err();
        assert_eq!(err.to_string(), "no card with id c1 — the board has: no cards");
    }

    #[test]
    fn a_bad_card_is_refused_and_the_board_is_unchanged() {
        assert_eq!(apply(&[], &BoardChange::Add { title: "  ".into(), note: None }), Err(BoardError::EmptyTitle));
        let long = "x".repeat(MAX_TITLE_CHARS + 1);
        assert_eq!(apply(&[], &BoardChange::Add { title: long, note: None }), Err(BoardError::TitleTooLong(MAX_TITLE_CHARS + 1)));
        let note = Some("x".repeat(MAX_NOTE_CHARS + 1));
        assert_eq!(apply(&[], &BoardChange::Add { title: "a".into(), note }), Err(BoardError::NoteTooLong(MAX_NOTE_CHARS + 1)));
        // At the limits exactly, it is kept.
        let title = "x".repeat(MAX_TITLE_CHARS);
        assert!(apply(&[], &BoardChange::Add { title, note: Some("y".repeat(MAX_NOTE_CHARS)) }).is_ok());
    }

    #[test]
    fn a_full_board_refuses_a_card_but_still_moves_and_removes() {
        let full: Vec<Card> = (1..=MAX_CARDS)
            .map(|i| Card { id: format!("c{i}"), title: "t".into(), note: None, column: Column::Todo })
            .collect();
        assert_eq!(apply(&full, &BoardChange::Add { title: "one more".into(), note: None }), Err(BoardError::Full));
        assert!(apply(&full, &BoardChange::Move { id: "c1".into(), column: Column::Done }).is_ok());
        assert_eq!(apply(&full, &BoardChange::Remove { id: "c1".into() }).unwrap().len(), MAX_CARDS - 1);
        let almost = &full[1..];
        assert_eq!(apply(almost, &BoardChange::Add { title: "fits".into(), note: None }).unwrap().len(), MAX_CARDS);
    }

    fn titles_in(cards: &[Card], column: Column) -> Vec<&str> {
        cards.iter().filter(|c| c.column == column).map(|c| c.title.as_str()).collect()
    }

    fn done(cards: &[Card], id: &str) -> Vec<Card> {
        apply(cards, &BoardChange::Move { id: id.into(), column: Column::Done }).unwrap()
    }

    #[test]
    fn done_keeps_the_last_five_finished_and_the_one_before_goes_to_the_archive() {
        let mut cards = Vec::new();
        for title in ["a", "b", "c", "d", "e", "f", "g"] {
            cards = add(&cards, title);
        }
        // Finished out of the order they were added: g first, a last.
        for id in ["c7", "c6", "c5", "c4", "c3", "c2"] {
            cards = done(&cards, id);
        }
        assert_eq!(titles_in(&cards, Column::Done), ["f", "e", "d", "c", "b"]);
        assert_eq!(titles_in(&cards, Column::Archived), ["g"]);

        cards = done(&cards, "c1");
        assert_eq!(titles_in(&cards, Column::Done), ["e", "d", "c", "b", "a"]);
        assert_eq!(titles_in(&cards, Column::Archived), ["g", "f"]);
    }

    #[test]
    fn a_move_within_done_keeps_its_place_and_one_back_out_comes_last() {
        let mut cards = add(&add(&add(&[], "a"), "b"), "c");
        cards = done(&done(&cards, "c2"), "c1");
        cards = done(&cards, "c2");
        assert_eq!(titles_in(&cards, Column::Done), ["b", "a"]);
        cards = apply(&cards, &BoardChange::Move { id: "c2".into(), column: Column::Todo }).unwrap();
        assert_eq!(titles_in(&cards, Column::Todo), ["c", "b"]);
    }

    #[test]
    fn the_archive_keeps_the_latest_and_does_not_fill_the_board() {
        let card = |i: usize, column| Card { id: format!("c{i}"), title: format!("t{i}"), note: None, column };
        let mut cards: Vec<Card> = (1..=MAX_ARCHIVED).map(|i| card(i, Column::Archived)).collect();
        cards.extend((MAX_ARCHIVED + 1..MAX_ARCHIVED + MAX_DONE + 2).map(|i| card(i, Column::Todo)));
        for i in MAX_ARCHIVED + 1..MAX_ARCHIVED + MAX_DONE + 2 {
            cards = done(&cards, &format!("c{i}"));
        }
        let archived = titles_in(&cards, Column::Archived);
        assert_eq!(archived.len(), MAX_ARCHIVED);
        assert_eq!((archived[0], *archived.last().unwrap()), ("t2", "t101"));

        // A full board of open cards with a full archive beside it still takes no more —
        // and one fewer open card makes room, whatever the archive holds.
        let mut full: Vec<Card> = (1..=MAX_CARDS).map(|i| card(i, Column::Todo)).collect();
        full.extend((MAX_CARDS + 1..=MAX_CARDS + MAX_ARCHIVED).map(|i| card(i, Column::Archived)));
        assert_eq!(apply(&full, &BoardChange::Add { title: "x".into(), note: None }), Err(BoardError::Full));
        let room = apply(&full, &BoardChange::Remove { id: "c1".into() }).unwrap();
        assert!(apply(&room, &BoardChange::Add { title: "x".into(), note: None }).is_ok());
    }

    #[test]
    fn an_archived_card_is_removed_like_any_other() {
        let cards = vec![Card { id: "c1".into(), title: "a".into(), note: None, column: Column::Archived }];
        assert_eq!(apply(&cards, &BoardChange::Remove { id: "c1".into() }).unwrap(), []);
    }

    #[test]
    fn the_wire_shape_is_camel_case_and_a_card_without_a_note_or_column_reads() {
        let card = Card { id: "c1".into(), title: "a".into(), note: None, column: Column::Doing };
        assert_eq!(serde_json::to_string(&card).unwrap(), r#"{"id":"c1","title":"a","column":"doing"}"#);
        let read: Card = serde_json::from_str(r#"{"id":"c1","title":"a"}"#).unwrap();
        assert_eq!(read.column, Column::Todo);
        let change: BoardChange = serde_json::from_str(r#"{"op":"move","id":"c1","column":"done"}"#).unwrap();
        assert_eq!(change, BoardChange::Move { id: "c1".into(), column: Column::Done });
    }
}
