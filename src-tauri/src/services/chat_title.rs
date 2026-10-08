//! A name for a new chat, asked of the active model while its first turn runs.
//!
//! One request with no tools and no history: the model is shown the first
//! message and nothing else, so the answer costs a few hundred tokens and
//! never reaches the conversation. Until it comes back, and for good when it
//! fails, the chat keeps the name `domain::chat_record::derive_title` took
//! from that message.

use crate::domain::chat_record::{self, ChatError, TITLE_PROMPT, TITLE_QUESTION_CHARS};
use crate::domain::llm::{ChatRequest, LlmMessage};
use crate::infra::{chat_store, llm_debug_log};
use crate::services::llm_session::LlmSession;

/// Names chat `id` from its first message. `None` when there was nothing to
/// name it from, the reply was not a name, or the chat was named already.
pub fn name(session: &LlmSession, id: &str) -> Result<Option<String>, ChatError> {
    let record = chat_store::load(id)?;
    let Some(question) = chat_record::first_question(&record.messages, &record.blocks) else {
        return Ok(None);
    };
    let Some(title) = ask(session, question)? else {
        return Ok(None);
    };
    Ok(chat_store::set_generated_title(id, &title)?.then_some(title))
}

fn ask(session: &LlmSession, question: &str) -> Result<Option<String>, ChatError> {
    if question.trim().is_empty() {
        return Ok(None);
    }
    let request = ChatRequest {
        messages: vec![
            LlmMessage::system(TITLE_PROMPT),
            LlmMessage::user(question.chars().take(TITLE_QUESTION_CHARS).collect::<String>()),
        ],
        tools: Vec::new(),
        model: session.model.clone(),
    };

    llm_debug_log::log_request(session.debug_logging, &session.provider_id, 0, &request);
    let response = session.provider.chat(request);
    llm_debug_log::log_response(session.debug_logging, &session.provider_id, 0, &response);

    Ok(chat_record::clean_title(&response?.content.unwrap_or_default()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::llm::{ChatResponse, ChatStreamResult, LlmError, LlmModelInfo, LlmProvider};
    use crate::testing::with_app_dir;
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    /// Answers every request with `answer`, keeping what it was asked.
    struct Namer {
        answer: Result<String, String>,
        asked: Mutex<Vec<ChatRequest>>,
    }

    impl LlmProvider for Namer {
        fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
            self.asked.lock().unwrap().push(request);
            match &self.answer {
                Ok(text) => Ok(ChatResponse { content: Some(text.clone()), tool_calls: Vec::new(), usage: None }),
                Err(e) => Err(LlmError::Provider(e.clone())),
            }
        }

        fn chat_stream(
            &self,
            _: ChatRequest,
            _: &dyn Fn(&str),
            _: &dyn Fn(&str),
            _: &dyn Fn(&str, &str, &str),
            _: &dyn Fn() -> bool,
        ) -> Result<ChatStreamResult, LlmError> {
            unreachable!("a title is never streamed")
        }

        fn list_models(&self) -> Result<Vec<LlmModelInfo>, LlmError> {
            unreachable!("a title never lists models")
        }
    }

    fn session(answer: Result<&str, &str>) -> (LlmSession, Arc<Namer>) {
        let namer = Arc::new(Namer {
            answer: answer.map(str::to_string).map_err(str::to_string),
            asked: Mutex::new(Vec::new()),
        });
        let session = LlmSession {
            provider: namer.clone(),
            provider_id: "test".to_string(),
            model: "m".to_string(),
            debug_logging: false,
            context_limit: None,
            reply_language: None,
            limits: Default::default(),
        };
        (session, namer)
    }

    fn save(id: &str, said: &str) {
        let blocks = json!([{ "kind": "user", "id": "user:0", "text": said }]);
        chat_store::save(id, "/repo", &[LlmMessage::user(said)], &blocks, &[], None, None).unwrap();
    }

    fn listed(id: &str) -> String {
        chat_store::list("/repo").unwrap().into_iter().find(|c| c.id == id).unwrap().title
    }

    #[test]
    fn the_model_s_name_replaces_the_first_message() {
        with_app_dir("chat-title-named", || {
            let long = format!("почему парсер теряет токен {}", "x".repeat(5000));
            save("one", &long);
            let (session, namer) = session(Ok("\"Парсер теряет токен.\""));

            assert_eq!(name(&session, "one").unwrap().as_deref(), Some("Парсер теряет токен"));
            assert_eq!(listed("one"), "Парсер теряет токен");

            let asked = namer.asked.lock().unwrap();
            assert_eq!(asked.len(), 1);
            assert!(asked[0].tools.is_empty());
            assert_eq!(asked[0].model, "m");
            assert_eq!(asked[0].messages[0].content.as_deref(), Some(TITLE_PROMPT));
            let shown = asked[0].messages[1].content.as_deref().unwrap();
            assert!(long.starts_with(shown));
            assert_eq!(shown.chars().count(), TITLE_QUESTION_CHARS);
        });
    }

    #[test]
    fn a_failed_or_useless_reply_keeps_the_first_message() {
        with_app_dir("chat-title-kept", || {
            save("one", "why is a token dropped?");

            let (failing, _) = session(Err("503"));
            assert!(matches!(name(&failing, "one"), Err(ChatError::Naming(_))));
            assert_eq!(listed("one"), "why is a token dropped?");

            let (rambling, _) = session(Ok(&"Because the lexer ".repeat(10)));
            assert_eq!(name(&rambling, "one").unwrap(), None);
            assert_eq!(listed("one"), "why is a token dropped?");
        });
    }

    #[test]
    fn a_chat_is_named_once_and_an_empty_one_not_at_all() {
        with_app_dir("chat-title-once", || {
            save("one", "first");
            save("empty", "   ");
            let (session, namer) = session(Ok("A name"));

            assert_eq!(name(&session, "one").unwrap().as_deref(), Some("A name"));
            assert_eq!(name(&session, "one").unwrap(), None);
            assert_eq!(name(&session, "empty").unwrap(), None);
            assert_eq!(namer.asked.lock().unwrap().len(), 2, "the empty chat is not sent");
            assert!(matches!(name(&session, "gone"), Err(ChatError::NotFound(_))));
        });
    }
}
