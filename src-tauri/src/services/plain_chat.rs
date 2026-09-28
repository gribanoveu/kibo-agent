//! One answer in Chat mode: the role's prompt, the conversation, one streamed
//! round. No tool loop and no folder — see `domain::chat_role`.

use std::cell::Cell;

use crate::domain::chat_role::ChatRole;
use crate::domain::llm::{ChatRequest, ChatStreamResult, LlmError, LlmMessage, LlmToolDefinition};
use crate::domain::tools::ToolName;
use crate::domain::turn::{ChatEventPayload, ChatEventSink, ChatTurnEvent};
use crate::infra::llm_debug_log;
use crate::services::ai_tools::tools::tool_definitions;
use crate::services::llm_session::LlmSession;

/// The model's answer to `messages`, its text streamed to `events` as it arrives.
/// A stop keeps what was said so far.
pub fn reply(
    session: &LlmSession,
    role: ChatRole,
    messages: Vec<LlmMessage>,
    events: &ChatEventSink,
    cancelled: &dyn Fn() -> bool,
) -> Result<ChatStreamResult, LlmError> {
    let request = ChatRequest {
        messages: std::iter::once(LlmMessage::system(system_prompt(role, session.reply_language)))
            .chain(messages)
            .collect(),
        tools: definitions(role),
        model: session.model.clone(),
    };
    let seq = Cell::new(0);
    let emit = |target: &str, event: ChatEventPayload| {
        seq.set(seq.get() + 1);
        events(ChatTurnEvent { seq: seq.get(), round: 1, target_id: Some(target.to_string()), event });
    };
    let on_delta = |delta: &str| emit("round:1:text", ChatEventPayload::Delta { delta: delta.to_string() });
    let on_reasoning = |delta: &str| emit("round:1:reasoning", ChatEventPayload::Reasoning { delta: delta.to_string() });

    llm_debug_log::log_request(session.debug_logging, &session.provider_id, 1, &request);
    let result = session.provider.chat_stream(request, &on_delta, &on_reasoning, &|_, _, _| {}, cancelled);
    llm_debug_log::log_response(session.debug_logging, &session.provider_id, 1, &result);
    let result = result?;
    // ponytail: a role's tools are offered but not run — no role has any yet. The
    // first one that does brings a tool loop; until then a call fails loudly
    // rather than being dropped.
    if let Some(call) = result.tool_calls.first() {
        return Err(LlmError::Message(format!("the model called {}, and Chat does not run tools yet", call.name)));
    }
    Ok(result)
}

fn system_prompt(role: ChatRole, language: Option<&str>) -> String {
    match language {
        Some(language) => format!("{}\n\nReply in {language}.", role.prompt()),
        None => role.prompt().to_string(),
    }
}

/// The role's tools as the model sees them — its own set, nothing from the agent's.
fn definitions(role: ChatRole) -> Vec<LlmToolDefinition> {
    tool_definitions()
        .into_iter()
        .filter(|definition| ToolName::from_wire_name(&definition.name).is_some_and(|tool| role.tools().contains(&tool)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::llm::{ChatResponse, LlmModelInfo, LlmProvider, LlmRole, LlmToolCall};
    use std::sync::{Arc, Mutex};

    /// Streams `answer` in two deltas, and records what it was asked.
    struct Talker {
        answer: ChatStreamResult,
        asked: Mutex<Vec<ChatRequest>>,
    }

    impl LlmProvider for Talker {
        fn chat(&self, _: ChatRequest) -> Result<ChatResponse, LlmError> {
            unreachable!("a chat reply is streamed")
        }

        fn chat_stream(
            &self,
            request: ChatRequest,
            on_delta: &dyn Fn(&str),
            _: &dyn Fn(&str),
            _: &dyn Fn(&str, &str, &str),
            _: &dyn Fn() -> bool,
        ) -> Result<ChatStreamResult, LlmError> {
            self.asked.lock().unwrap().push(request);
            let (head, tail) = self.answer.text.split_at(self.answer.text.len() / 2);
            on_delta(head);
            on_delta(tail);
            Ok(self.answer.clone())
        }

        fn list_models(&self) -> Result<Vec<LlmModelInfo>, LlmError> {
            unreachable!("a chat reply never lists models")
        }
    }

    fn session(answer: ChatStreamResult, language: Option<&'static str>) -> (LlmSession, Arc<Talker>) {
        let talker = Arc::new(Talker { answer, asked: Mutex::new(Vec::new()) });
        let session = LlmSession {
            provider: talker.clone(),
            provider_id: "test".to_string(),
            model: "m".to_string(),
            debug_logging: false,
            context_limit: None,
            reply_language: language,
        };
        (session, talker)
    }

    fn said(text: &str) -> ChatStreamResult {
        ChatStreamResult { text: text.to_string(), ..Default::default() }
    }

    #[test]
    fn the_role_speaks_first_the_conversation_follows_and_no_tool_is_offered() {
        let (session, talker) = session(said("hello"), Some("Russian"));
        let seen: Arc<Mutex<Vec<ChatTurnEvent>>> = Arc::default();
        let heard = seen.clone();
        let events: ChatEventSink = Arc::new(move |event| heard.lock().unwrap().push(event));

        let result = reply(&session, ChatRole::Assistant, vec![LlmMessage::user("hi")], &events, &|| false).unwrap();

        assert_eq!(result.text, "hello");
        let asked = talker.asked.lock().unwrap();
        let messages = &asked[0].messages;
        assert_eq!(messages[0].role, LlmRole::System);
        let system = messages[0].content.as_deref().unwrap();
        assert!(system.starts_with(ChatRole::Assistant.prompt()) && system.ends_with("Reply in Russian."), "{system}");
        assert_eq!(messages[1], LlmMessage::user("hi"));
        assert!(asked[0].tools.is_empty());

        let seen = seen.lock().unwrap();
        let deltas: Vec<(u64, String)> = seen
            .iter()
            .map(|event| match &event.event {
                ChatEventPayload::Delta { delta } => (event.seq, delta.clone()),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(deltas, [(1, "he".to_string()), (2, "llo".to_string())]);
    }

    #[test]
    fn auto_language_adds_nothing_to_the_prompt() {
        assert_eq!(system_prompt(ChatRole::Assistant, None), ChatRole::Assistant.prompt());
    }

    /// A call nobody runs must not pass for an answer.
    #[test]
    fn a_tool_call_is_an_error_not_a_silent_answer() {
        let answer = ChatStreamResult {
            tool_calls: vec![LlmToolCall { id: "c".into(), name: "readFile".into(), arguments: "{}".into() }],
            ..said("")
        };
        let (session, _) = session(answer, None);
        let err = reply(&session, ChatRole::Assistant, Vec::new(), &(Arc::new(|_| {}) as ChatEventSink), &|| false)
            .unwrap_err();
        assert!(err.to_string().contains("readFile"), "{err}");
    }
}
