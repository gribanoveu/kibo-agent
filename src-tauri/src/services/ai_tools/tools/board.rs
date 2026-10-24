//! `board` — the Assistant's cards for later: "remind me to file the memory
//! request" becomes a card the user finds on the Board page.
//!
//! No approval card: it writes only the app's own board, and what it wrote is
//! on screen. It cannot remove a card — the model moves one to Done, and the
//! user clears it. It edits one, so a detail added later lands on the card
//! already there rather than on a second.

use crate::domain::board::{BoardChange, BoardError};
use crate::domain::llm::LlmToolDefinition;
use crate::domain::tools::{BoardArgs, BoardOp, ToolDeps, ToolError, ToolResult};

pub fn board(args: &BoardArgs, deps: &ToolDeps) -> Result<ToolResult, ToolError> {
    let board = deps.board.as_ref().ok_or_else(|| BoardError::Store("there is no board in this chat".into()))?;
    let missing = |field: &str| ToolError::InvalidArguments { tool: "board".into(), reason: format!("op \"{}\" needs `{field}`", op_name(args.op)) };
    let cards = match args.op {
        BoardOp::List => board.cards()?,
        BoardOp::Add => {
            let title = args.title.clone().ok_or_else(|| missing("title"))?;
            board.change(&BoardChange::Add { title, note: args.note.clone(), due: args.due.clone() })?
        }
        BoardOp::Move => {
            let id = args.id.clone().ok_or_else(|| missing("id"))?;
            let column = args.column.ok_or_else(|| missing("column"))?;
            board.change(&BoardChange::Move { id, column, index: None })?
        }
        BoardOp::Edit => {
            let id = args.id.clone().ok_or_else(|| missing("id"))?;
            if args.title.is_none() && args.note.is_none() && args.due.is_none() {
                return Err(missing("title`, `note` or `due"));
            }
            board.change(&BoardChange::Edit { id, title: args.title.clone(), note: args.note.clone(), due: args.due.clone() })?
        }
    };
    Ok(ToolResult::Board { cards })
}

fn op_name(op: BoardOp) -> &'static str {
    match op {
        BoardOp::List => "list",
        BoardOp::Add => "add",
        BoardOp::Move => "move",
        BoardOp::Edit => "edit",
    }
}

pub(super) fn definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "board".to_string(),
        description: "The user's board of things to do later, which they see on the Board page in the sidebar. \
            When the user asks you to remind them of something, note it, or keep it for later, add a card: a short \
            imperative title in the user's language, and in `note` the details they gave — names, numbers, links. \
            You cannot remind them at a time: when they name a day or a deadline, put it in `due` as YYYY-MM-DD — the \
            prompt says what today is — and say the card is on the board; the page marks it once the day comes. `list` shows every card with its id and \
            column, all but the archive, where Done's oldest go — call it before answering what is on the board, or \
            before moving or editing a card you have not seen this turn. `move` puts a card in `todo`, `doing` or `done`; when the user says something is done or no longer \
            needed, move it to `done` — only the user removes cards. When the user adds to something already on the board — a detail, a date, a \
            new wording, a new due date — `edit` that card rather than adding a second: `note` replaces the whole note, so send the old one with the addition; \
            what you leave out stays as it was."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "op": { "type": "string", "enum": ["list", "add", "move", "edit"] },
                "title": { "type": ["string", "null"], "description": "For op \"add\": the card's title. For op \"edit\": its new title, or null to keep it." },
                "note": { "type": ["string", "null"], "description": "For op \"add\": details, optional. For op \"edit\": the whole new note, or null to keep it; an empty string clears it." },
                "id": { "type": ["string", "null"], "description": "For ops \"move\" and \"edit\": the card's id, as `list` shows it." },
                "column": { "type": ["string", "null"], "enum": ["todo", "doing", "done", null], "description": "Only for op \"move\": where the card goes." },
                "due": { "type": ["string", "null"], "description": "For ops \"add\" and \"edit\": the day it is due, YYYY-MM-DD, or null — for \"edit\", null keeps it and an empty string clears it." }
            },
            "required": ["op"],
            "additionalProperties": false
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::board::{apply, Card, Column, TaskBoard};
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct InMemory(Mutex<Vec<Card>>);

    impl TaskBoard for InMemory {
        fn cards(&self) -> Result<Vec<Card>, BoardError> {
            Ok(self.0.lock().unwrap().clone())
        }
        fn change(&self, change: &BoardChange) -> Result<Vec<Card>, BoardError> {
            let mut cards = self.0.lock().unwrap();
            *cards = apply(&cards, change)?;
            Ok(cards.clone())
        }
    }

    fn call(deps: &ToolDeps, json: &str) -> Result<ToolResult, ToolError> {
        board(&serde_json::from_str(json).unwrap(), deps)
    }

    #[test]
    fn add_then_move_then_list_returns_the_whole_board_each_time() {
        let deps = ToolDeps { board: Some(Arc::new(InMemory::default())), ..ToolDeps::default() };
        let added = call(&deps, r#"{"op":"add","title":"file the memory request","note":"wlbuh"}"#).unwrap();
        let ToolResult::Board { cards } = added else { panic!("{added:?}") };
        assert_eq!((cards[0].id.as_str(), cards[0].note.as_deref()), ("c1", Some("wlbuh")));

        call(&deps, r#"{"op":"move","id":"c1","column":"done"}"#).unwrap();
        let listed = call(&deps, r#"{"op":"list"}"#).unwrap();
        let ToolResult::Board { cards } = listed else { panic!("{listed:?}") };
        assert_eq!(cards[0].column, Column::Done);
    }

    #[test]
    fn a_missing_field_names_the_op_and_the_field() {
        let deps = ToolDeps { board: Some(Arc::new(InMemory::default())), ..ToolDeps::default() };
        let err = call(&deps, r#"{"op":"add"}"#).unwrap_err().to_string();
        assert!(err.contains("op \"add\" needs `title`"), "{err}");
        let err = call(&deps, r#"{"op":"move","id":"c1"}"#).unwrap_err().to_string();
        assert!(err.contains("op \"move\" needs `column`"), "{err}");
        let err = call(&deps, r#"{"op":"move","column":"done"}"#).unwrap_err().to_string();
        assert!(err.contains("op \"move\" needs `id`"), "{err}");
    }

    #[test]
    fn edit_changes_what_it_is_given_and_needs_an_id_and_something_to_change() {
        let deps = ToolDeps { board: Some(Arc::new(InMemory::default())), ..ToolDeps::default() };
        call(&deps, r#"{"op":"add","title":"renew cert","note":"wlbuh"}"#).unwrap();
        let edited = call(&deps, r#"{"op":"edit","id":"c1","title":null,"note":"wlbuh, expires 1 Nov"}"#).unwrap();
        let ToolResult::Board { cards } = edited else { panic!("{edited:?}") };
        assert_eq!((cards.len(), cards[0].title.as_str(), cards[0].note.as_deref()), (1, "renew cert", Some("wlbuh, expires 1 Nov")));

        let err = call(&deps, r#"{"op":"edit","title":"x"}"#).unwrap_err().to_string();
        assert!(err.contains("op \"edit\" needs `id`"), "{err}");
        let err = call(&deps, r#"{"op":"edit","id":"c1","title":null,"note":null,"due":null}"#).unwrap_err().to_string();
        assert!(err.contains("op \"edit\" needs `title`, `note` or `due`"), "{err}");
        let err = call(&deps, r#"{"op":"edit","id":"c1","title":"  "}"#).unwrap_err().to_string();
        assert!(err.contains("a card needs a title"), "{err}");
    }

    #[test]
    fn a_due_date_goes_on_with_add_and_edit() {
        let deps = ToolDeps { board: Some(Arc::new(InMemory::default())), ..ToolDeps::default() };
        let added = call(&deps, r#"{"op":"add","title":"renew cert","due":"2026-11-01"}"#).unwrap();
        let ToolResult::Board { cards } = added else { panic!("{added:?}") };
        assert_eq!(cards[0].due.as_deref(), Some("2026-11-01"));
        let edited = call(&deps, r#"{"op":"edit","id":"c1","due":"2026-11-03"}"#).unwrap();
        let ToolResult::Board { cards } = edited else { panic!("{edited:?}") };
        assert_eq!((cards[0].title.as_str(), cards[0].due.as_deref()), ("renew cert", Some("2026-11-03")));
        let err = call(&deps, r#"{"op":"add","title":"x","due":"next friday"}"#).unwrap_err().to_string();
        assert!(err.contains("give the due date as YYYY-MM-DD"), "{err}");
    }

    #[test]
    fn the_board_refusing_is_the_calls_error() {
        let deps = ToolDeps { board: Some(Arc::new(InMemory::default())), ..ToolDeps::default() };
        let err = call(&deps, r#"{"op":"move","id":"c7","column":"done"}"#).unwrap_err().to_string();
        assert!(err.contains("no card with id c7"), "{err}");
    }

    #[test]
    fn without_a_board_it_says_so() {
        let err = call(&ToolDeps::default(), r#"{"op":"list"}"#).unwrap_err().to_string();
        assert!(err.contains("no board"), "{err}");
    }
}
