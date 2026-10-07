//! The model-facing protocol: the shapes a provider speaks, and the trait that
//! speaks them.
//!
//! Provider-agnostic on purpose. An OpenAI-compatible server and Anthropic's
//! Messages API disagree about almost everything at the wire level; the
//! conversion lives in each implementation, and the loop above never learns
//! which one it is talking to.
//!
//! The turn's own event stream and its pause/resume types are not here — they
//! belong to the loop, not to the provider.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::image::ImagePart;

/// One tool call as the model produced it — a name and a JSON string it
/// generated, neither of which is trusted to be well-formed.
///
/// Turning this into a typed `ToolCall` is `services::ai_tools::parse`'s job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmToolCall {
    /// The provider's id for this call, echoed back with its result so a round
    /// with several calls can be matched up.
    pub id: String,
    pub name: String,
    /// Raw JSON. May be empty for a tool that takes no arguments, and may be
    /// malformed — a model getting its own call wrong is ordinary, and is fed
    /// back to it as a tool result rather than failing the turn.
    #[serde(default)]
    pub arguments: String,
}

/// Who is speaking in a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LlmRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmMessage {
    pub role: LlmRole,
    /// `None` for an assistant message that only requests tool calls, which
    /// matches the wire: providers send `content: null` there, not `""`.
    #[serde(default)]
    pub content: Option<String>,
    /// Which call this is the result of. Only ever set on a `Tool` message.
    #[serde(default)]
    pub tool_call_id: Option<String>,
    /// The calls an assistant message requested, sent back so the provider
    /// sees its own prior request when the matching results follow.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<LlmToolCall>,
    /// The provider's own content blocks for this assistant message, in the
    /// order it sent them, when they carry something the fields above cannot.
    ///
    /// Today that is Anthropic's thinking: a `thinking` block's signature must
    /// go back unmodified, in its place between the text and the calls, in the
    /// round that answers those calls — rebuilt from `content` and
    /// `tool_calls`, it would be gone or out of order and the API refuses the
    /// request. The provider that wrote it sends it verbatim; any other
    /// ignores it and uses the fields above, which always agree with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_content: Option<serde_json::Value>,
    /// Pictures the user attached, sanitized (`services::image_sanitize`).
    /// Only ever on a `User` message: DeepSeek refuses a picture in an
    /// assistant's. Each wire sends them before `content`. Absent from every
    /// message without one, so those serialize exactly as they did before.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ImagePart>,
}

impl LlmMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self::text(LlmRole::System, content)
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self::text(LlmRole::User, content)
    }

    /// What the user wrote, with the pictures they attached.
    pub fn user_with_images(content: impl Into<String>, images: Vec<ImagePart>) -> Self {
        Self { images, ..Self::user(content) }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self::text(LlmRole::Assistant, content)
    }

    /// The result of one tool call, addressed to the call it answers.
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: LlmRole::Tool,
            content: Some(content.into()),
            tool_call_id: Some(tool_call_id.into()),
            tool_calls: Vec::new(),
            native_content: None,
            images: Vec::new(),
        }
    }

    /// An assistant turn that asked for tools instead of answering.
    pub fn tool_requests(calls: Vec<LlmToolCall>) -> Self {
        Self {
            role: LlmRole::Assistant,
            content: None,
            tool_call_id: None,
            tool_calls: calls,
            native_content: None,
            images: Vec::new(),
        }
    }

    fn text(role: LlmRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: Some(content.into()),
            tool_call_id: None,
            tool_calls: Vec::new(),
            native_content: None,
            images: Vec::new(),
        }
    }
}

/// One callable tool, as the model needs to see it.
///
/// `parameters` is a raw JSON Schema object rather than a typed schema: this
/// client neither validates nor constructs schemas, it passes one through.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatRequest {
    pub messages: Vec<LlmMessage>,
    #[serde(default)]
    pub tools: Vec<LlmToolDefinition>,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatResponse {
    /// `None` when the model only requested tools this round.
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub tool_calls: Vec<LlmToolCall>,
    #[serde(default)]
    pub usage: Option<ChatUsage>,
}

/// One streamed round.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatStreamResult {
    /// The authoritative full text — a safety net against a delta lost on its
    /// way to a listener, rather than something to re-derive from the deltas.
    pub text: String,
    /// A reasoning-capable model's thinking text, kept apart from `text`.
    /// Empty for every provider that never sends it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reasoning: String,
    #[serde(default)]
    pub usage: Option<ChatUsage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<LlmToolCall>,
    /// The provider stopped because the round ran out of response budget
    /// (`finish_reason: "length"`), not because the model was done.
    ///
    /// Worth carrying rather than dropping: the model is never told what that
    /// budget is, so a cut-off round is indistinguishable from a finished one
    /// by looking at the text. Left unread it produces two silent failures — a
    /// reply that stops mid-sentence and reads as deliberate, and a tool call
    /// whose arguments were severed halfway and come back as an unexplained
    /// parse error. `false` when the provider says nothing; nothing here
    /// infers truncation from the text itself.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    /// See [`LlmMessage::native_content`]; carried from the round into the
    /// assistant message the loop stores for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_content: Option<serde_json::Value>,
}

/// Token accounting for one round.
///
/// Every request resends the whole history, so `total_tokens` of the latest
/// round is the authoritative context usage at that point — not a per-round
/// statistic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    /// How much of `prompt_tokens` the provider read from its prompt cache
    /// rather than processing again — the part billed at a fraction. Zero
    /// when it said nothing, which most gateways do.
    #[serde(default)]
    pub cached_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmModelInfo {
    pub id: String,
}

#[derive(Debug, Error)]
pub enum LlmError {
    #[error("provider error: {0}")]
    Provider(String),
    /// The provider refused this request because of a rate limit, rather than
    /// because of anything about the request. Its own variant because it is
    /// the one failure that is worth trying again unchanged — see
    /// `domain::llm_retry`. `retry_after_seconds` is the server's hint when it
    /// sent a usable one.
    #[error("rate limited by the provider: {message}")]
    RateLimited {
        retry_after_seconds: Option<u64>,
        message: String,
    },
    /// The provider or a gateway in front of it failed on its own side — 500,
    /// 502, 503, 504. Nothing about the request is wrong, so, like a rate
    /// limit, it is worth sending again unchanged.
    #[error("the provider is unavailable: {message}")]
    Unavailable {
        retry_after_seconds: Option<u64>,
        message: String,
    },
    #[error("http error: {0}")]
    Http(String),
    #[error("tls configuration error: {0}")]
    Tls(String),
    #[error("{0}")]
    Message(String),
}

/// Replaces `arguments` with `"{}"` on any call whose arguments do not parse
/// as a JSON object, before they are echoed back in the next request.
///
/// A model occasionally streams malformed arguments — an observed case is
/// `{}""`. The wire format does not require the echoed copy to be valid JSON,
/// the field being opaque to the protocol, but at least one real gateway
/// answers 500 when it is not. Sanitizing regardless of provider is cheaper
/// than finding out which ones tolerate it.
///
/// Only the echo is sanitized. The original arguments still reach the parser,
/// which reports what was wrong with them — replacing them here would turn a
/// precise complaint into a silently empty call.
pub fn sanitize_tool_call_arguments(calls: &[LlmToolCall]) -> Vec<LlmToolCall> {
    calls
        .iter()
        .map(|call| LlmToolCall {
            id: call.id.clone(),
            name: call.name.clone(),
            arguments: if is_json_object(&call.arguments) {
                call.arguments.clone()
            } else {
                "{}".to_string()
            },
        })
        .collect()
}

fn is_json_object(arguments: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(arguments).is_ok_and(|v| v.is_object())
}

/// One model backend.
///
/// Synchronous on purpose: the agent loop is sequential by nature, a blocking
/// HTTP client is markedly easier to debug, and async would colour every
/// service above it. Callers run these on a blocking pool.
pub trait LlmProvider: Send + Sync {
    fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError>;

    /// Streams one round.
    ///
    /// The callbacks are `&dyn Fn` rather than generics because this trait is
    /// used as a trait object.
    ///
    /// - `on_delta` fires per non-empty text chunk.
    /// - `on_reasoning` is the same for thinking text. A chunk carries one or
    ///   the other, never both meaningfully, so they are separate callbacks.
    ///   Most providers never call it, which is fine.
    /// - `on_tool_call_delta` receives `(id, name, arguments)` with the
    ///   arguments accumulated *so far*, not the fragment. A long write
    ///   otherwise sits invisible until the stream ends, which looks like a
    ///   hung connection.
    /// - `cancelled` is polled between chunks, so a stop takes effect within
    ///   roughly one chunk instead of after the whole response. Returning
    ///   early this way is not an error: whatever accumulated comes back as a
    ///   normal result, and the caller decides what that means.
    fn chat_stream(
        &self,
        request: ChatRequest,
        on_delta: &dyn Fn(&str),
        on_reasoning: &dyn Fn(&str),
        on_tool_call_delta: &dyn Fn(&str, &str, &str),
        cancelled: &dyn Fn() -> bool,
    ) -> Result<ChatStreamResult, LlmError>;

    fn list_models(&self) -> Result<Vec<LlmModelInfo>, LlmError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(arguments: &str) -> LlmToolCall {
        LlmToolCall {
            id: "call_1".into(),
            name: "readFile".into(),
            arguments: arguments.into(),
        }
    }

    #[test]
    fn valid_arguments_are_echoed_unchanged() {
        let calls = sanitize_tool_call_arguments(&[call(r#"{"path": "a.txt"}"#)]);
        assert_eq!(calls[0].arguments, r#"{"path": "a.txt"}"#);
    }

    /// The observed malformed case, and the shapes next to it. A gateway that
    /// validates the echoed copy answers 500 on any of them.
    #[test]
    fn malformed_arguments_become_an_empty_object() {
        for broken in [r#"{}"""#, "", "not json", "[1, 2]", r#""a string""#, "null"] {
            let calls = sanitize_tool_call_arguments(&[call(broken)]);
            assert_eq!(calls[0].arguments, "{}", "{broken:?} was echoed as-is");
        }
    }

    #[test]
    fn identity_is_preserved_even_when_arguments_are_replaced() {
        let calls = sanitize_tool_call_arguments(&[call("garbage")]);
        assert_eq!((calls[0].id.as_str(), calls[0].name.as_str()), ("call_1", "readFile"));
    }

    /// An assistant message that only requests tools carries `content: null`,
    /// which is what the wire actually says — `""` would read as an empty
    /// answer the model never gave.
    #[test]
    fn a_tool_request_message_has_null_content() {
        let message = LlmMessage::tool_requests(vec![call("{}")]);
        let json = serde_json::to_value(&message).expect("serializes");
        assert_eq!(json["content"], serde_json::Value::Null);
        assert_eq!(json["role"], "assistant");
    }

    /// An ordinary message must not grow an empty `toolCalls` array just
    /// because tool calling exists.
    #[test]
    fn a_plain_message_carries_no_tool_call_fields() {
        let json = serde_json::to_string(&LlmMessage::user("hello")).expect("serializes");
        assert_eq!(json, r#"{"role":"user","content":"hello","toolCallId":null}"#);
    }

    /// Pictures survive a saved chat's round trip, and a chat saved before
    /// pictures existed still loads.
    #[test]
    fn pictures_are_kept_and_old_messages_still_load() {
        use crate::domain::image::{ImageMediaType, ImagePart};
        let picture = ImagePart { media_type: ImageMediaType::Jpeg, data: "AA".into(), width: 2, height: 3 };
        let message = LlmMessage::user_with_images("this", vec![picture.clone()]);
        let json = serde_json::to_string(&message).expect("serializes");
        assert!(json.contains(r#""images":[{"mediaType":"image/jpeg","data":"AA","width":2,"height":3}]"#), "{json}");
        assert_eq!(serde_json::from_str::<LlmMessage>(&json).expect("loads"), message);
        assert_eq!((message.role, message.content.as_deref()), (LlmRole::User, Some("this")));

        let old: LlmMessage = serde_json::from_str(r#"{"role":"user","content":"hi"}"#).expect("loads");
        assert!(old.images.is_empty());
    }

    #[test]
    fn a_tool_result_names_the_call_it_answers() {
        let message = LlmMessage::tool_result("call_7", "contents");
        assert_eq!(message.role, LlmRole::Tool);
        assert_eq!(message.tool_call_id.as_deref(), Some("call_7"));
    }

    /// Absent fields must read as "the provider said nothing", not as a
    /// parse failure — half the providers omit half of these.
    #[test]
    fn a_minimal_stream_result_parses() {
        let result: ChatStreamResult = serde_json::from_str(r#"{"text": "hi"}"#).expect("parses");
        assert_eq!(result.text, "hi");
        assert!(result.usage.is_none());
        assert!(result.tool_calls.is_empty());
        assert!(!result.truncated, "nothing infers truncation from the text");
    }
}
