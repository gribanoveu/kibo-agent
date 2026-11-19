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
pub const MAX_COMMENT_CHARS: usize = 2000;
/// A card is a task, not a thread: past this a new remark is refused.
pub const MAX_COMMENTS: usize = 100;

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
    /// The user's remarks on it, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comments: Vec<Comment>,
}

/// A remark the user left on a card on the Board page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    /// Numbered within its card, on from the highest there is.
    pub id: u32,
    pub text: String,
    /// When it was written, RFC 3339 — the page's clock: the domain keeps none.
    pub at: String,
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
    /// The page's only: a remark added to a card, `at` the moment it was written.
    Comment { id: String, text: String, at: String },
    /// The page's only: a remark taken off a card.
    Uncomment { id: String, comment: u32 },
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
    #[error("a comment needs text")]
    EmptyComment,
    #[error("the comment is {0} characters; keep it under {MAX_COMMENT_CHARS}")]
    CommentTooLong(usize),
    #[error("the card has {MAX_COMMENTS} comments — remove the ones no longer needed")]
    TooManyComments,
    #[error("{0:?} is not a time — give it as RFC 3339")]
    BadTime(String),
    #[error("card {id} has no comment {comment}")]
    NoComment { id: String, comment: u32 },
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
            // On top of To do: the newest is the one looked for.
            let top = next.iter().position(|c| c.column == Column::Todo).unwrap_or(next.len());
            next.insert(top, Card { id: next_id(cards), title, note, due, column: Column::Todo, comments: Vec::new() });
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
        BoardChange::Comment { id, text, at: when } => {
            let card = &mut next[position(cards, id)?];
            let text = text.trim();
            if text.is_empty() {
                return Err(BoardError::EmptyComment);
            }
            let chars = text.chars().count();
            if chars > MAX_COMMENT_CHARS {
                return Err(BoardError::CommentTooLong(chars));
            }
            if card.comments.len() >= MAX_COMMENTS {
                return Err(BoardError::TooManyComments);
            }
            chrono::DateTime::parse_from_rfc3339(when.trim()).map_err(|_| BoardError::BadTime(when.clone()))?;
            let next_comment = card.comments.iter().map(|c| c.id).max().unwrap_or(0) + 1;
            card.comments.push(Comment { id: next_comment, text: text.to_string(), at: when.trim().to_string() });
        }
        BoardChange::Uncomment { id, comment } => {
            let card = &mut next[position(cards, id)?];
            let at = card.comments.iter().position(|c| c.id == *comment).ok_or_else(|| BoardError::NoComment { id: id.clone(), comment: *comment })?;
            card.comments.remove(at);
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

/// A card's key: `T` and its number, as a tracker's issue key reads. Numbered
/// on from the highest there is, like the checklist: a key is not handed out
/// twice while its card is on the board. Once the card is removed it may be —
/// the board, unlike a turn, has no history that names it.
fn next_id(cards: &[Card]) -> String {
    let last = cards.iter().filter_map(|c| number(&c.id)).max().unwrap_or(0);
    format!("T{}", last + 1)
}

/// The number in a key, in either case and in the `c3` cards were first
/// keyed by — a chat from before the change still names them so.
fn number(id: &str) -> Option<u32> {
    let id = id.trim();
    let digits = id.strip_prefix(['T', 't', 'C', 'c'])?;
    digits.bytes().all(|b| b.is_ascii_digit()).then(|| digits.parse().ok()).flatten()
}

/// A board kept before cards were keyed `T3` reads with their keys: the same
/// number, so a card is the one it was. Written back on the next change.
pub fn with_keys(mut cards: Vec<Card>) -> Vec<Card> {
    for card in &mut cards {
        if let Some(n) = number(&card.id) {
            card.id = format!("T{n}");
        }
    }
    cards
}

fn position(cards: &[Card], id: &str) -> Result<usize, BoardError> {
    let wanted = number(id);
    cards.iter().position(|c| c.id == id || (wanted.is_some() && number(&c.id) == wanted)).ok_or_else(|| BoardError::NotFound {
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
    fn an_added_card_is_todo_on_top_with_the_next_id_and_trimmed_text() {
        let one = apply(&[], &BoardChange::Add { title: "  file the memory request  ".into(), note: Some("   ".into()), due: None }).unwrap();
        let two = apply(&one, &BoardChange::Add { title: "renew cert".into(), note: Some("  wlbuh  ".into()), due: None }).unwrap();
        assert_eq!(
            two,
            [
                Card { id: "T2".into(), title: "renew cert".into(), note: Some("wlbuh".into()), due: None, column: Column::Todo, comments: vec![] },
                Card { id: "T1".into(), title: "file the memory request".into(), note: None, due: None, column: Column::Todo, comments: vec![] },
            ]
        );
        // On top of To do's cards, wherever they stand among the others.
        let started = apply(&add(&[], "a"), &BoardChange::Move { id: "T1".into(), column: Column::Doing, index: None }).unwrap();
        let cards = add(&add(&started, "b"), "c");
        assert_eq!(titles_in(&cards, Column::Todo), ["c", "b"]);
        assert_eq!(titles_in(&cards, Column::Doing), ["a"]);
    }

    #[test]
    fn ids_go_on_from_the_highest_even_after_a_removal_in_the_middle() {
        let three = add(&add(&add(&[], "a"), "b"), "c");
        let without_b = apply(&three, &BoardChange::Remove { id: "T2".into() }).unwrap();
        assert_eq!(add(&without_b, "d")[0].id, "T4");
    }

    #[test]
    fn a_key_is_t_and_a_number_found_in_either_case_and_by_the_first_c_keys() {
        let cards = add(&add(&[], "a"), "b");
        assert_eq!(cards.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["T2", "T1"]);
        let column = |cards: &[Card]| cards.iter().find(|c| c.title == "a").unwrap().column;
        for id in ["T1", "t1", "c1", "C1", " T1 "] {
            assert_eq!(column(&apply(&cards, &BoardChange::Move { id: id.into(), column: Column::Done, index: None }).unwrap()), Column::Done, "{id}");
        }
        for id in ["T", "T1x", "x1", "T-1", "T+1", "1"] {
            assert!(matches!(apply(&cards, &BoardChange::Remove { id: id.into() }), Err(BoardError::NotFound { .. })), "{id}");
        }
        // Numbered on from the highest, whichever way it was keyed.
        let old = vec![Card { id: "c7".into(), title: "old".into(), note: None, due: None, column: Column::Doing, comments: vec![] }];
        assert_eq!(add(&old, "new").iter().find(|c| c.title == "new").unwrap().id, "T8");
        // An id that is not a key is left as it is, and found as it is.
        let odd = vec![Card { id: "x".into(), ..old[0].clone() }, Card { id: "y".into(), ..old[0].clone() }];
        assert_eq!(with_keys(odd.clone())[0].id, "x");
        assert_eq!(apply(&odd, &BoardChange::Remove { id: "y".into() }).unwrap()[0].id, "x");
        assert_eq!(with_keys(old)[0].id, "T7");
    }

    #[test]
    fn a_move_changes_only_that_cards_column() {
        let two = add(&add(&[], "a"), "b");
        let moved = apply(&two, &BoardChange::Move { id: "T2".into(), column: Column::Done, index: None }).unwrap();
        assert_eq!(moved.iter().map(|c| c.column).collect::<Vec<_>>(), [Column::Todo, Column::Done]);
    }

    fn put(cards: &[Card], id: &str, column: Column, index: usize) -> Vec<Card> {
        apply(cards, &BoardChange::Move { id: id.into(), column, index: Some(index) }).unwrap()
    }

    #[test]
    fn an_index_places_the_card_among_the_columns_others_and_past_their_end_last() {
        let mut cards = add(&add(&add(&[], "a"), "b"), "c");
        cards = put(&cards, "T2", Column::Doing, 0);
        assert_eq!(titles_in(&cards, Column::Todo), ["c", "a"]);

        assert_eq!(titles_in(&put(&cards, "T1", Column::Todo, 1), Column::Todo), ["c", "a"]);
        assert_eq!(titles_in(&put(&cards, "T3", Column::Todo, 0), Column::Todo), ["c", "a"]);
        assert_eq!(titles_in(&put(&cards, "T1", Column::Todo, 0), Column::Todo), ["a", "c"]);
        assert_eq!(titles_in(&put(&cards, "T1", Column::Todo, 9), Column::Todo), ["c", "a"]);

        // Into another column, at its top, between and at its end.
        let doing = |cards: &[Card]| titles_in(cards, Column::Doing).into_iter().map(str::to_string).collect::<Vec<_>>();
        assert_eq!(doing(&put(&cards, "T1", Column::Doing, 0)), ["a", "b"]);
        assert_eq!(doing(&put(&cards, "T1", Column::Doing, 1)), ["b", "a"]);
        let two = put(&cards, "T1", Column::Doing, 1);
        assert_eq!(doing(&put(&two, "T3", Column::Doing, 1)), ["b", "c", "a"]);
        assert_eq!(titles_in(&two, Column::Todo), ["c"]);
    }

    #[test]
    fn done_takes_no_index_and_keeps_the_order_cards_finished_in() {
        let mut cards = add(&add(&add(&[], "a"), "b"), "c");
        cards = done(&done(&cards, "T1"), "T2");
        assert_eq!(titles_in(&put(&cards, "T3", Column::Done, 0), Column::Done), ["a", "b", "c"]);
        assert_eq!(titles_in(&put(&cards, "T2", Column::Done, 0), Column::Done), ["a", "b"]);
    }

    #[test]
    fn an_unknown_id_names_the_ones_there_are() {
        let two = add(&add(&[], "a"), "b");
        let err = apply(&two, &BoardChange::Move { id: "T9".into(), column: Column::Done, index: None }).unwrap_err();
        assert_eq!(err.to_string(), "no card with id T9 — the board has: T2, T1");
        let err = apply(&[], &BoardChange::Remove { id: "T1".into() }).unwrap_err();
        assert_eq!(err.to_string(), "no card with id T1 — the board has: no cards");
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
            .map(|i| Card { id: format!("T{i}"), title: "t".into(), note: None, due: None, column: Column::Todo, comments: vec![] })
            .collect();
        assert_eq!(apply(&full, &BoardChange::Add { title: "one more".into(), note: None, due: None }), Err(BoardError::Full));
        assert!(apply(&full, &BoardChange::Move { id: "T1".into(), column: Column::Done, index: None }).is_ok());
        assert_eq!(apply(&full, &BoardChange::Remove { id: "T1".into() }).unwrap().len(), MAX_CARDS - 1);
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
        for id in ["T7", "T6", "T5", "T4", "T3", "T2"] {
            cards = done(&cards, id);
        }
        assert_eq!(titles_in(&cards, Column::Done), ["f", "e", "d", "c", "b"]);
        assert_eq!(titles_in(&cards, Column::Archived), ["g"]);

        cards = done(&cards, "T1");
        assert_eq!(titles_in(&cards, Column::Done), ["e", "d", "c", "b", "a"]);
        assert_eq!(titles_in(&cards, Column::Archived), ["g", "f"]);
    }

    #[test]
    fn a_move_within_done_keeps_its_place_and_one_back_out_comes_last() {
        let mut cards = add(&add(&add(&[], "a"), "b"), "c");
        cards = done(&done(&cards, "T2"), "T1");
        cards = done(&cards, "T2");
        assert_eq!(titles_in(&cards, Column::Done), ["b", "a"]);
        cards = apply(&cards, &BoardChange::Move { id: "T2".into(), column: Column::Todo, index: None }).unwrap();
        assert_eq!(titles_in(&cards, Column::Todo), ["c", "b"]);
    }

    #[test]
    fn the_archive_keeps_the_latest_and_does_not_fill_the_board() {
        let card = |i: usize, column| Card { id: format!("T{i}"), title: format!("t{i}"), note: None, due: None, column, comments: vec![] };
        let mut cards: Vec<Card> = (1..=MAX_ARCHIVED).map(|i| card(i, Column::Archived)).collect();
        cards.extend((MAX_ARCHIVED + 1..MAX_ARCHIVED + MAX_DONE + 2).map(|i| card(i, Column::Todo)));
        for i in MAX_ARCHIVED + 1..MAX_ARCHIVED + MAX_DONE + 2 {
            cards = done(&cards, &format!("T{i}"));
        }
        let archived = titles_in(&cards, Column::Archived);
        assert_eq!(archived.len(), MAX_ARCHIVED);
        assert_eq!((archived[0], *archived.last().unwrap()), ("t2", "t101"));

        // A full board of open cards with a full archive beside it still takes no more —
        // and one fewer open card makes room, whatever the archive holds.
        let mut full: Vec<Card> = (1..=MAX_CARDS).map(|i| card(i, Column::Todo)).collect();
        full.extend((MAX_CARDS + 1..=MAX_CARDS + MAX_ARCHIVED).map(|i| card(i, Column::Archived)));
        assert_eq!(apply(&full, &BoardChange::Add { title: "x".into(), note: None, due: None }), Err(BoardError::Full));
        let room = apply(&full, &BoardChange::Remove { id: "T1".into() }).unwrap();
        assert!(apply(&room, &BoardChange::Add { title: "x".into(), note: None, due: None }).is_ok());
    }

    #[test]
    fn an_edit_changes_only_the_text_trimmed_and_an_empty_note_clears_it() {
        let mut cards = add(&add(&add(&[], "a"), "b"), "c");
        cards = done(&cards, "T2");
        let edited = apply(&cards, &BoardChange::Edit { id: "T2".into(), title: Some("  bee  ".into()), note: Some(" stung ".into()), due: None }).unwrap();
        assert_eq!(edited[2], Card { id: "T2".into(), title: "bee".into(), note: Some("stung".into()), due: None, column: Column::Done, comments: vec![] });
        assert_eq!(edited[..2], cards[..2]);

        let cleared = apply(&edited, &BoardChange::Edit { id: "T2".into(), title: None, note: Some("  ".into()), due: None }).unwrap();
        assert_eq!((cleared[2].title.as_str(), cleared[2].note.as_deref()), ("bee", None));
    }

    #[test]
    fn what_an_edit_leaves_out_stays() {
        let cards = apply(&[], &BoardChange::Add { title: "renew cert".into(), note: Some("wlbuh".into()), due: None }).unwrap();
        let edit = |title: Option<&str>, note: Option<&str>| {
            let edited = apply(&cards, &BoardChange::Edit { id: "T1".into(), title: title.map(Into::into), note: note.map(Into::into), due: None }).unwrap();
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
            apply(&cards, &BoardChange::Edit { id: "T1".into(), title: None, note: None, due: due.map(Into::into) }).map(|cards| cards[0].due.clone())
        };
        assert_eq!(edit(None), Ok(Some("2026-11-01".into())));
        assert_eq!(edit(Some("2026-12-24")), Ok(Some("2026-12-24".into())));
        assert_eq!(edit(Some("")), Ok(None));
        assert_eq!(edit(Some("soon")), Err(BoardError::BadDue("soon".into())));
    }

    #[test]
    fn an_edit_is_checked_as_a_new_card_is_and_a_full_board_still_takes_it() {
        let cards = add(&[], "a");
        let edit = |title: &str, note: Option<String>| apply(&cards, &BoardChange::Edit { id: "T1".into(), title: Some(title.into()), note, due: None });
        assert_eq!(edit(" ", None), Err(BoardError::EmptyTitle));
        assert_eq!(edit(&"x".repeat(MAX_TITLE_CHARS + 1), None), Err(BoardError::TitleTooLong(MAX_TITLE_CHARS + 1)));
        assert_eq!(edit("a", Some("y".repeat(MAX_NOTE_CHARS + 1))), Err(BoardError::NoteTooLong(MAX_NOTE_CHARS + 1)));
        let err = apply(&cards, &BoardChange::Edit { id: "T9".into(), title: Some("b".into()), note: None, due: None }).unwrap_err();
        assert_eq!(err.to_string(), "no card with id T9 — the board has: T1");

        let full: Vec<Card> = (1..=MAX_CARDS)
            .map(|i| Card { id: format!("T{i}"), title: "t".into(), note: None, due: None, column: Column::Todo, comments: vec![] })
            .collect();
        assert_eq!(apply(&full, &BoardChange::Edit { id: "T1".into(), title: Some("u".into()), note: None, due: None }).unwrap()[0].title, "u");
    }

    const NOW: &str = "2026-11-19T10:00:00.000Z";

    fn comment(cards: &[Card], id: &str, text: &str) -> Result<Vec<Card>, BoardError> {
        apply(cards, &BoardChange::Comment { id: id.into(), text: text.into(), at: NOW.into() })
    }

    #[test]
    fn a_comment_is_added_under_the_card_trimmed_numbered_and_dated() {
        let cards = add(&add(&[], "a"), "b");
        let one = comment(&cards, "T1", "  asked the bank  ").unwrap();
        let two = comment(&one, "T1", "they call back on Monday").unwrap();
        let card = two.iter().find(|c| c.id == "T1").unwrap();
        assert_eq!(
            card.comments,
            [
                Comment { id: 1, text: "asked the bank".into(), at: NOW.into() },
                Comment { id: 2, text: "they call back on Monday".into(), at: NOW.into() },
            ]
        );
        // The other card, the order and the columns are as they were.
        assert_eq!(two.iter().map(|c| (c.id.as_str(), c.comments.len())).collect::<Vec<_>>(), [("T2", 0), ("T1", 2)]);
    }

    #[test]
    fn a_comment_is_refused_empty_too_long_past_the_limit_badly_dated_or_on_no_card() {
        let cards = add(&[], "a");
        assert_eq!(comment(&cards, "T1", "  "), Err(BoardError::EmptyComment));
        assert_eq!(comment(&cards, "T1", &"x".repeat(MAX_COMMENT_CHARS + 1)), Err(BoardError::CommentTooLong(MAX_COMMENT_CHARS + 1)));
        assert!(comment(&cards, "T1", &"x".repeat(MAX_COMMENT_CHARS)).is_ok());
        assert_eq!(apply(&cards, &BoardChange::Comment { id: "T1".into(), text: "a".into(), at: "yesterday".into() }), Err(BoardError::BadTime("yesterday".into())));
        assert!(matches!(comment(&cards, "T9", "a"), Err(BoardError::NotFound { .. })));

        let mut full = cards.clone();
        for _ in 0..MAX_COMMENTS {
            full = comment(&full, "T1", "a").unwrap();
        }
        assert_eq!(comment(&full, "T1", "one more"), Err(BoardError::TooManyComments));
    }

    #[test]
    fn a_comment_taken_off_leaves_the_others_and_its_number_is_not_reused_while_a_later_one_stays() {
        let mut cards = add(&[], "a");
        for text in ["one", "two", "three"] {
            cards = comment(&cards, "T1", text).unwrap();
        }
        let without = apply(&cards, &BoardChange::Uncomment { id: "T1".into(), comment: 2 }).unwrap();
        assert_eq!(without[0].comments.iter().map(|c| (c.id, c.text.as_str())).collect::<Vec<_>>(), [(1, "one"), (3, "three")]);
        assert_eq!(comment(&without, "T1", "four").unwrap()[0].comments.last().unwrap().id, 4);
        assert_eq!(
            apply(&without, &BoardChange::Uncomment { id: "T1".into(), comment: 2 }),
            Err(BoardError::NoComment { id: "T1".into(), comment: 2 })
        );
        // An edit or a move keeps them.
        let edited = apply(&without, &BoardChange::Edit { id: "T1".into(), title: Some("b".into()), note: None, due: None }).unwrap();
        let moved = apply(&edited, &BoardChange::Move { id: "T1".into(), column: Column::Done, index: None }).unwrap();
        assert_eq!(moved[0].comments, without[0].comments);
    }

    #[test]
    fn an_archived_card_is_removed_like_any_other() {
        let cards = vec![Card { id: "T1".into(), title: "a".into(), note: None, due: None, column: Column::Archived, comments: vec![] }];
        assert_eq!(apply(&cards, &BoardChange::Remove { id: "T1".into() }).unwrap(), []);
    }

    #[test]
    fn the_wire_shape_is_camel_case_and_a_card_without_a_note_or_column_reads() {
        let card = Card { id: "T1".into(), title: "a".into(), note: None, due: None, column: Column::Doing, comments: vec![] };
        assert_eq!(serde_json::to_string(&card).unwrap(), r#"{"id":"T1","title":"a","column":"doing"}"#);
        let due = Card { due: Some("2026-11-01".into()), ..card.clone() };
        assert_eq!(serde_json::to_string(&due).unwrap(), r#"{"id":"T1","title":"a","due":"2026-11-01","column":"doing"}"#);
        let change: BoardChange = serde_json::from_str(r#"{"op":"add","title":"a","note":null,"due":"2026-11-01"}"#).unwrap();
        assert_eq!(change, BoardChange::Add { title: "a".into(), note: None, due: Some("2026-11-01".into()) });
        let read: Card = serde_json::from_str(r#"{"id":"T1","title":"a"}"#).unwrap();
        assert_eq!(read.column, Column::Todo);
        let change: BoardChange = serde_json::from_str(r#"{"op":"move","id":"T1","column":"done"}"#).unwrap();
        assert_eq!(change, BoardChange::Move { id: "T1".into(), column: Column::Done, index: None });
        let change: BoardChange = serde_json::from_str(r#"{"op":"move","id":"T1","column":"todo","index":2}"#).unwrap();
        assert_eq!(change, BoardChange::Move { id: "T1".into(), column: Column::Todo, index: Some(2) });
        let change: BoardChange = serde_json::from_str(r#"{"op":"edit","id":"T1","title":"b","note":"n"}"#).unwrap();
        assert_eq!(change, BoardChange::Edit { id: "T1".into(), title: Some("b".into()), note: Some("n".into()), due: None });
        let change: BoardChange = serde_json::from_str(r#"{"op":"edit","id":"T1","note":""}"#).unwrap();
        assert_eq!(change, BoardChange::Edit { id: "T1".into(), title: None, note: Some("".into()), due: None });
        let change: BoardChange = serde_json::from_str(r#"{"op":"comment","id":"T1","text":"t","at":"2026-11-19T10:00:00Z"}"#).unwrap();
        assert_eq!(change, BoardChange::Comment { id: "T1".into(), text: "t".into(), at: "2026-11-19T10:00:00Z".into() });
        let change: BoardChange = serde_json::from_str(r#"{"op":"uncomment","id":"T1","comment":3}"#).unwrap();
        assert_eq!(change, BoardChange::Uncomment { id: "T1".into(), comment: 3 });
        let commented = Card { comments: vec![Comment { id: 1, text: "t".into(), at: "2026-11-19T10:00:00Z".into() }], ..card };
        assert_eq!(
            serde_json::to_string(&commented).unwrap(),
            r#"{"id":"T1","title":"a","column":"doing","comments":[{"id":1,"text":"t","at":"2026-11-19T10:00:00Z"}]}"#
        );
    }
}
