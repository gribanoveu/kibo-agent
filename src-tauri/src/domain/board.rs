//! The board: what the user asked the Assistant to keep for later — "remind
//! me to file the memory request" — as cards in three columns, and the user's
//! own cards and moves of them on the Board page. Done holds the last few finished; the
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
    /// The day it is due, `YYYY-MM-DD`: a day, not a time — the app reminds
    /// no one, the page only marks it once the day is here or past.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
    #[serde(default)]
    pub column: Column,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum BoardChange {
    Add {
        title: String,
        note: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        due: Option<String>,
    },
    /// `index`, the page's only: the card's place among the column's other
    /// cards, past their end meaning last. Without it the card keeps its place.
    /// Done takes none — it keeps the order cards finished in, for the archive.
    Move {
        id: String,
        column: Column,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<usize>,
    },
    /// The title, the note, the due date in place, the column and the place in
    /// it kept: what is left out stays, an empty note or date clears it. The
    /// page sends all three; the model, what it adds to a card instead of a
    /// second one.
    Edit {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        due: Option<String>,
    },
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
    #[error("{0:?} is not a date — give the due date as YYYY-MM-DD")]
    BadDue(String),
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
        BoardChange::Add { title, note, due } => {
            let (title, note) = text(title, note.as_deref())?;
            let due = due_date(due.as_deref())?;
            if cards.iter().filter(|c| c.column != Column::Archived).count() >= MAX_CARDS {
                return Err(BoardError::Full);
            }
            next.push(Card { id: next_id(cards), title, note, due, column: Column::Todo });
        }
        BoardChange::Edit { id, title, note, due } => {
            let at = position(cards, id)?;
            let card = &cards[at];
            let (title, note) = text(title.as_deref().unwrap_or(&card.title), note.as_deref().or(card.note.as_deref()))?;
            let due = due_date(due.as_deref().or(card.due.as_deref()))?;
            next[at].title = title;
            next[at].note = note;
            next[at].due = due;
        }
        BoardChange::Move { id, column, index } => {
            let at = position(cards, id)?;
            let mut card = next.remove(at);
            // Done keeps the order cards arrived in, so its first is the one
            // the archive takes first.
            let arrives = card.column != *column && *column == Column::Done;
            card.column = *column;
            if arrives {
                next.push(card);
            } else if let Some(index) = index.filter(|_| *column != Column::Done) {
                let place = next.iter().enumerate().filter(|(_, c)| c.column == *column).map(|(i, _)| i).nth(index);
                next.insert(place.unwrap_or(next.len()), card);
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

/// A card's title and note, trimmed and checked — the same for a new card and an edited one.
fn text(title: &str, note: Option<&str>) -> Result<(String, Option<String>), BoardError> {
    let title = title.trim();
    if title.is_empty() {
        return Err(BoardError::EmptyTitle);
    }
    let title_chars = title.chars().count();
    if title_chars > MAX_TITLE_CHARS {
        return Err(BoardError::TitleTooLong(title_chars));
    }
    let note = note.map(str::trim).filter(|n| !n.is_empty());
    if let Some(chars) = note.map(|n| n.chars().count()).filter(|&c| c > MAX_NOTE_CHARS) {
        return Err(BoardError::NoteTooLong(chars));
    }
    Ok((title.to_string(), note.map(str::to_string)))
}

/// A due date, written back as `YYYY-MM-DD`; an empty one is none.
fn due_date(due: Option<&str>) -> Result<Option<String>, BoardError> {
    let Some(due) = due.map(str::trim).filter(|d| !d.is_empty()) else { return Ok(None) };
    chrono::NaiveDate::parse_from_str(due, "%Y-%m-%d")
        .map(|day| Some(day.format("%Y-%m-%d").to_string()))
        .map_err(|_| BoardError::BadDue(due.to_string()))
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
        apply(cards, &BoardChange::Add { title: title.into(), note: None, due: None }).unwrap()
    }

    #[test]
    fn an_added_card_is_todo_with_the_next_id_and_trimmed_text() {
        let one = apply(&[], &BoardChange::Add { title: "  file the memory request  ".into(), note: Some("   ".into()), due: None }).unwrap();
        let two = apply(&one, &BoardChange::Add { title: "renew cert".into(), note: Some("  wlbuh  ".into()), due: None }).unwrap();
        assert_eq!(
            two,
            [
                Card { id: "c1".into(), title: "file the memory request".into(), note: None, due: None, column: Column::Todo },
                Card { id: "c2".into(), title: "renew cert".into(), note: Some("wlbuh".into()), due: None, column: Column::Todo },
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
        let moved = apply(&two, &BoardChange::Move { id: "c2".into(), column: Column::Done, index: None }).unwrap();
        assert_eq!(moved.iter().map(|c| c.column).collect::<Vec<_>>(), [Column::Todo, Column::Done]);
    }

    fn put(cards: &[Card], id: &str, column: Column, index: usize) -> Vec<Card> {
        apply(cards, &BoardChange::Move { id: id.into(), column, index: Some(index) }).unwrap()
    }

    #[test]
    fn an_index_places_the_card_among_the_columns_others_and_past_their_end_last() {
        let mut cards = add(&add(&add(&[], "a"), "b"), "c");
        cards = put(&cards, "c2", Column::Doing, 0);
        assert_eq!(titles_in(&cards, Column::Todo), ["a", "c"]);

        assert_eq!(titles_in(&put(&cards, "c1", Column::Todo, 1), Column::Todo), ["c", "a"]);
        assert_eq!(titles_in(&put(&cards, "c3", Column::Todo, 0), Column::Todo), ["c", "a"]);
        assert_eq!(titles_in(&put(&cards, "c1", Column::Todo, 0), Column::Todo), ["a", "c"]);
        assert_eq!(titles_in(&put(&cards, "c1", Column::Todo, 9), Column::Todo), ["c", "a"]);

        // Into another column, at its top, between and at its end.
        let doing = |cards: &[Card]| titles_in(cards, Column::Doing).into_iter().map(str::to_string).collect::<Vec<_>>();
        assert_eq!(doing(&put(&cards, "c1", Column::Doing, 0)), ["a", "b"]);
        assert_eq!(doing(&put(&cards, "c1", Column::Doing, 1)), ["b", "a"]);
        let two = put(&cards, "c1", Column::Doing, 1);
        assert_eq!(doing(&put(&two, "c3", Column::Doing, 1)), ["b", "c", "a"]);
        assert_eq!(titles_in(&two, Column::Todo), ["c"]);
    }

    #[test]
    fn done_takes_no_index_and_keeps_the_order_cards_finished_in() {
        let mut cards = add(&add(&add(&[], "a"), "b"), "c");
        cards = done(&done(&cards, "c1"), "c2");
        assert_eq!(titles_in(&put(&cards, "c3", Column::Done, 0), Column::Done), ["a", "b", "c"]);
        assert_eq!(titles_in(&put(&cards, "c2", Column::Done, 0), Column::Done), ["a", "b"]);
    }

    #[test]
    fn an_unknown_id_names_the_ones_there_are() {
        let two = add(&add(&[], "a"), "b");
        let err = apply(&two, &BoardChange::Move { id: "c9".into(), column: Column::Done, index: None }).unwrap_err();
        assert_eq!(err.to_string(), "no card with id c9 — the board has: c1, c2");
        let err = apply(&[], &BoardChange::Remove { id: "c1".into() }).unwrap_err();
        assert_eq!(err.to_string(), "no card with id c1 — the board has: no cards");
    }

    #[test]
    fn a_bad_card_is_refused_and_the_board_is_unchanged() {
        assert_eq!(apply(&[], &BoardChange::Add { title: "  ".into(), note: None, due: None }), Err(BoardError::EmptyTitle));
        let long = "x".repeat(MAX_TITLE_CHARS + 1);
        assert_eq!(apply(&[], &BoardChange::Add { title: long, note: None, due: None }), Err(BoardError::TitleTooLong(MAX_TITLE_CHARS + 1)));
        let note = Some("x".repeat(MAX_NOTE_CHARS + 1));
        assert_eq!(apply(&[], &BoardChange::Add { title: "a".into(), note, due: None }), Err(BoardError::NoteTooLong(MAX_NOTE_CHARS + 1)));
        // At the limits exactly, it is kept.
        let title = "x".repeat(MAX_TITLE_CHARS);
        assert!(apply(&[], &BoardChange::Add { title, note: Some("y".repeat(MAX_NOTE_CHARS)), due: None }).is_ok());
    }

    #[test]
    fn a_full_board_refuses_a_card_but_still_moves_and_removes() {
        let full: Vec<Card> = (1..=MAX_CARDS)
            .map(|i| Card { id: format!("c{i}"), title: "t".into(), note: None, due: None, column: Column::Todo })
            .collect();
        assert_eq!(apply(&full, &BoardChange::Add { title: "one more".into(), note: None, due: None }), Err(BoardError::Full));
        assert!(apply(&full, &BoardChange::Move { id: "c1".into(), column: Column::Done, index: None }).is_ok());
        assert_eq!(apply(&full, &BoardChange::Remove { id: "c1".into() }).unwrap().len(), MAX_CARDS - 1);
        let almost = &full[1..];
        assert_eq!(apply(almost, &BoardChange::Add { title: "fits".into(), note: None, due: None }).unwrap().len(), MAX_CARDS);
    }

    fn titles_in(cards: &[Card], column: Column) -> Vec<&str> {
        cards.iter().filter(|c| c.column == column).map(|c| c.title.as_str()).collect()
    }

    fn done(cards: &[Card], id: &str) -> Vec<Card> {
        apply(cards, &BoardChange::Move { id: id.into(), column: Column::Done, index: None }).unwrap()
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
        cards = apply(&cards, &BoardChange::Move { id: "c2".into(), column: Column::Todo, index: None }).unwrap();
        assert_eq!(titles_in(&cards, Column::Todo), ["c", "b"]);
    }

    #[test]
    fn the_archive_keeps_the_latest_and_does_not_fill_the_board() {
        let card = |i: usize, column| Card { id: format!("c{i}"), title: format!("t{i}"), note: None, due: None, column };
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
        assert_eq!(apply(&full, &BoardChange::Add { title: "x".into(), note: None, due: None }), Err(BoardError::Full));
        let room = apply(&full, &BoardChange::Remove { id: "c1".into() }).unwrap();
        assert!(apply(&room, &BoardChange::Add { title: "x".into(), note: None, due: None }).is_ok());
    }

    #[test]
    fn an_edit_changes_only_the_text_trimmed_and_an_empty_note_clears_it() {
        let mut cards = add(&add(&add(&[], "a"), "b"), "c");
        cards = done(&cards, "c2");
        let edited = apply(&cards, &BoardChange::Edit { id: "c2".into(), title: Some("  bee  ".into()), note: Some(" stung ".into()), due: None }).unwrap();
        assert_eq!(edited[2], Card { id: "c2".into(), title: "bee".into(), note: Some("stung".into()), due: None, column: Column::Done });
        assert_eq!(edited[..2], cards[..2]);

        let cleared = apply(&edited, &BoardChange::Edit { id: "c2".into(), title: None, note: Some("  ".into()), due: None }).unwrap();
        assert_eq!((cleared[2].title.as_str(), cleared[2].note.as_deref()), ("bee", None));
    }

    #[test]
    fn what_an_edit_leaves_out_stays() {
        let cards = apply(&[], &BoardChange::Add { title: "renew cert".into(), note: Some("wlbuh".into()), due: None }).unwrap();
        let edit = |title: Option<&str>, note: Option<&str>| {
            let edited = apply(&cards, &BoardChange::Edit { id: "c1".into(), title: title.map(Into::into), note: note.map(Into::into), due: None }).unwrap();
            (edited[0].title.clone(), edited[0].note.clone())
        };
        assert_eq!(edit(None, Some("wlbuh, expires 1 Nov")), ("renew cert".into(), Some("wlbuh, expires 1 Nov".into())));
        assert_eq!(edit(Some("renew the cert"), None), ("renew the cert".into(), Some("wlbuh".into())));
        assert_eq!(edit(None, None), ("renew cert".into(), Some("wlbuh".into())));
    }

    #[test]
    fn a_due_date_is_a_real_day_written_back_as_yyyy_mm_dd_and_an_empty_one_is_none() {
        let due = |due: &str| apply(&[], &BoardChange::Add { title: "a".into(), note: None, due: Some(due.into()) }).map(|cards| cards[0].due.clone());
        assert_eq!(due(" 2026-11-01 "), Ok(Some("2026-11-01".into())));
        assert_eq!(due("2026-11-1"), Ok(Some("2026-11-01".into())));
        assert_eq!(due("  "), Ok(None));
        assert_eq!(due("2026-02-30"), Err(BoardError::BadDue("2026-02-30".into())));
        assert_eq!(due("tomorrow").unwrap_err().to_string(), "\"tomorrow\" is not a date — give the due date as YYYY-MM-DD");
    }

    #[test]
    fn an_edit_sets_moves_or_clears_the_due_date_and_leaving_it_out_keeps_it() {
        let cards = apply(&[], &BoardChange::Add { title: "a".into(), note: None, due: Some("2026-11-01".into()) }).unwrap();
        let edit = |due: Option<&str>| {
            apply(&cards, &BoardChange::Edit { id: "c1".into(), title: None, note: None, due: due.map(Into::into) }).map(|cards| cards[0].due.clone())
        };
        assert_eq!(edit(None), Ok(Some("2026-11-01".into())));
        assert_eq!(edit(Some("2026-12-24")), Ok(Some("2026-12-24".into())));
        assert_eq!(edit(Some("")), Ok(None));
        assert_eq!(edit(Some("soon")), Err(BoardError::BadDue("soon".into())));
    }

    #[test]
    fn an_edit_is_checked_as_a_new_card_is_and_a_full_board_still_takes_it() {
        let cards = add(&[], "a");
        let edit = |title: &str, note: Option<String>| apply(&cards, &BoardChange::Edit { id: "c1".into(), title: Some(title.into()), note, due: None });
        assert_eq!(edit(" ", None), Err(BoardError::EmptyTitle));
        assert_eq!(edit(&"x".repeat(MAX_TITLE_CHARS + 1), None), Err(BoardError::TitleTooLong(MAX_TITLE_CHARS + 1)));
        assert_eq!(edit("a", Some("y".repeat(MAX_NOTE_CHARS + 1))), Err(BoardError::NoteTooLong(MAX_NOTE_CHARS + 1)));
        let err = apply(&cards, &BoardChange::Edit { id: "c9".into(), title: Some("b".into()), note: None, due: None }).unwrap_err();
        assert_eq!(err.to_string(), "no card with id c9 — the board has: c1");

        let full: Vec<Card> = (1..=MAX_CARDS)
            .map(|i| Card { id: format!("c{i}"), title: "t".into(), note: None, due: None, column: Column::Todo })
            .collect();
        assert_eq!(apply(&full, &BoardChange::Edit { id: "c1".into(), title: Some("u".into()), note: None, due: None }).unwrap()[0].title, "u");
    }

    #[test]
    fn an_archived_card_is_removed_like_any_other() {
        let cards = vec![Card { id: "c1".into(), title: "a".into(), note: None, due: None, column: Column::Archived }];
        assert_eq!(apply(&cards, &BoardChange::Remove { id: "c1".into() }).unwrap(), []);
    }

    #[test]
    fn the_wire_shape_is_camel_case_and_a_card_without_a_note_or_column_reads() {
        let card = Card { id: "c1".into(), title: "a".into(), note: None, due: None, column: Column::Doing };
        assert_eq!(serde_json::to_string(&card).unwrap(), r#"{"id":"c1","title":"a","column":"doing"}"#);
        let due = Card { due: Some("2026-11-01".into()), ..card };
        assert_eq!(serde_json::to_string(&due).unwrap(), r#"{"id":"c1","title":"a","due":"2026-11-01","column":"doing"}"#);
        let change: BoardChange = serde_json::from_str(r#"{"op":"add","title":"a","note":null,"due":"2026-11-01"}"#).unwrap();
        assert_eq!(change, BoardChange::Add { title: "a".into(), note: None, due: Some("2026-11-01".into()) });
        let read: Card = serde_json::from_str(r#"{"id":"c1","title":"a"}"#).unwrap();
        assert_eq!(read.column, Column::Todo);
        let change: BoardChange = serde_json::from_str(r#"{"op":"move","id":"c1","column":"done"}"#).unwrap();
        assert_eq!(change, BoardChange::Move { id: "c1".into(), column: Column::Done, index: None });
        let change: BoardChange = serde_json::from_str(r#"{"op":"move","id":"c1","column":"todo","index":2}"#).unwrap();
        assert_eq!(change, BoardChange::Move { id: "c1".into(), column: Column::Todo, index: Some(2) });
        let change: BoardChange = serde_json::from_str(r#"{"op":"edit","id":"c1","title":"b","note":"n"}"#).unwrap();
        assert_eq!(change, BoardChange::Edit { id: "c1".into(), title: Some("b".into()), note: Some("n".into()), due: None });
        let change: BoardChange = serde_json::from_str(r#"{"op":"edit","id":"c1","note":""}"#).unwrap();
        assert_eq!(change, BoardChange::Edit { id: "c1".into(), title: None, note: Some("".into()), due: None });
    }
}
