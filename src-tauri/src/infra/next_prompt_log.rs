//! The next-prompt journal: the `suggestions` table in `chats.db`
//! (`docs/20-next-prompt-suggestions.md`).
//!
//! One row per finished turn: the model input exactly as built, and what the
//! user sent next. It never leaves the machine; the user exports it to train
//! on. Until a model runs here, `model` and `suggestion` are empty and
//! `shown` is 0 — the rows are still an input paired with its true answer.

use rusqlite::params;

use crate::domain::chat_record::{self, ChatError};
use crate::infra::chat_store::{now, open, store};

/// Writes a turn's input; the id is what [`record_sent`] is given later.
pub fn record(chat_id: &str, input: &str) -> Result<String, ChatError> {
    chat_record::check_id(chat_id)?;
    let id = uuid::Uuid::new_v4().to_string();
    open()?
        .execute(
            "INSERT INTO suggestions (id, chat_id, created_at, model, input, suggestion, shown)
             VALUES (?1, ?2, ?3, '', ?4, '', 0)",
            params![id, chat_id, now(), input],
        )
        .map_err(store)?;
    Ok(id)
}

/// What the user sent after that turn. Once: the first answer is the answer.
pub fn record_sent(id: &str, sent: &str) -> Result<(), ChatError> {
    open()?
        .execute("UPDATE suggestions SET sent = ?2 WHERE id = ?1 AND sent IS NULL", params![id, sent])
        .map_err(store)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::with_app_dir;

    fn row(id: &str) -> (String, String, String, i64, i64, Option<String>) {
        open()
            .unwrap()
            .query_row(
                "SELECT chat_id, input, model, shown, used, sent FROM suggestions WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .unwrap()
    }

    #[test]
    fn a_turn_is_written_and_answered_once() {
        with_app_dir("next-prompt-log", || {
            let id = record("chat-1", "<mode>agent…").unwrap();
            assert_eq!(row(&id), ("chat-1".into(), "<mode>agent…".into(), String::new(), 0, 0, None));

            record_sent(&id, "давай далее").unwrap();
            record_sent(&id, "и ещё").unwrap();
            assert_eq!(row(&id).5.as_deref(), Some("давай далее"));
        });
    }

    #[test]
    fn a_chat_id_that_is_not_an_id_is_refused() {
        with_app_dir("next-prompt-log-id", || {
            assert!(record("../x", "i").is_err());
        });
    }
}
