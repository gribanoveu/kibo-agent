//! `board` — the Assistant's cards for later: "remind me to file the memory
//! request" becomes a card the user finds on the Board page.
//!
//! No approval card: it writes only the app's own board, and what it wrote is
//! on screen. It cannot remove a card — the model moves one to Done, and the
//! user clears it.

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
            board.change(&BoardChange::Add { title, note: args.note.clone() })?
        }
        BoardOp::Move => {
            let id = args.id.clone().ok_or_else(|| missing("id"))?;
            let column = args.column.ok_or_else(|| missing("column"))?;
            board.change(&BoardChange::Move { id, column, index: None })?
        }
    };
    Ok(ToolResult::Board { cards })
}

fn op_name(op: BoardOp) -> &'static str {
    match op {
        BoardOp::List => "list",
        BoardOp::Add => "add",
        BoardOp::Move => "move",
    }
}

pub(super) fn definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "board".to_string(),
        description: "The user's board of things to do later, which they see on the Board page in the sidebar. \
            When the user asks you to remind them of something, note it, or keep it for later, add a card: a short \
            imperative title in the user's language, and in `note` the details they gave — names, numbers, links. \
            You cannot remind them at a time: when they name a day or a deadline, put its date in the title — the \
            prompt says what today is — and say the card is on the board. `list` shows every card with its id and \
            column, all but the archive, where Done's oldest go — call it before answering what is on the board, or \
            before moving a card you have not seen this turn. `move` puts a card in `todo`, `doing` or `done`; when the user says something is done or no longer \
            needed, move it to `done` — only the user removes cards."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "op": { "type": "string", "enum": ["list", "add", "move"] },
                "title": { "type": ["string", "null"], "description": "Only for op \"add\": the card's title." },
                "note": { "type": ["string", "null"], "description": "Only for op \"add\": details, optional." },
                "id": { "type": ["string", "null"], "description": "Only for op \"move\": the card's id, as `list` shows it." },
                "column": { "type": ["string", "null"], "enum": ["todo", "doing", "done", null], "description": "Only for op \"move\": where the card goes." }
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
