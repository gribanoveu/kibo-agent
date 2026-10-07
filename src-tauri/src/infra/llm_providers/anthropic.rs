//! Anthropic's Messages API (`/v1/messages`).
//!
//! Where it differs from the OpenAI-compatible wire, and so what this module
//! is for:
//!
//! - The system prompt is a request field, not a message.
//! - A tool call is a `tool_use` content block inside the assistant message,
//!   and its result is a `tool_result` block inside a *user* message — there
//!   is no tool role. Several results of one round, and any text the user
//!   adds after them, are one message: the API wants roles to alternate.
//! - `max_tokens` is required.
//! - The stream is typed events (`content_block_delta`, `message_delta`, …)
//!   rather than chunks of a completion, and an error can arrive as an event
//!   after a `200`.
//!
//! Prompt caching is on for every request: three `cache_control` points,
//! see [`mark_cache_points`].
//!
//! Thinking: asked for through `reasoning_effort` (see [`thinking_params`]),
//! and — on models that think whether asked or not — arriving regardless.
//! Either way a round that thought is kept whole in
//! `ChatStreamResult::native_content` and sent back verbatim, because a
//! `thinking` block's signature must return unmodified and in place.
//! `docs/06-port-plan.md`, F-7.3.
//!
//! `base_url` includes the version, as it does for the OpenAI-compatible
//! provider: `https://api.anthropic.com/v1`.

use std::collections::HashMap;

use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use serde_json::{json, Value};
use zeroize::Zeroizing;

use super::openai_compatible::{header_value, ok_or_status_error};
use crate::domain::image::ImagePart;
use crate::domain::llm::{
    ChatRequest, ChatResponse, ChatStreamResult, ChatUsage, LlmError, LlmMessage, LlmModelInfo,
    LlmProvider, LlmRole, LlmToolCall,
};

const API_VERSION: &str = "2023-06-01";

/// Sent when the provider has no `max_tokens` of its own — the API has no
/// default. It caps thinking and the reply together, and the newer models
/// think unasked, so it is sized for a long agentic round. Within what every
/// current model accepts; a model that allows more gets more only when the
/// user says so.
const DEFAULT_MAX_TOKENS: u32 = 64_000;

pub struct AnthropicProvider {
    agent: ureq::Agent,
    base_url: String,
    api_key: SecretString,
    request_headers: HashMap<String, String>,
    temperature: Option<f32>,
    top_p: Option<f32>,
    max_tokens: Option<u32>,
    reasoning_effort: Option<String>,
    models_url: Option<String>,
}

impl AnthropicProvider {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        agent: ureq::Agent,
        base_url: String,
        api_key: SecretString,
        request_headers: HashMap<String, String>,
        temperature: Option<f32>,
        top_p: Option<f32>,
        max_tokens: Option<u32>,
        reasoning_effort: Option<String>,
        models_url: Option<String>,
    ) -> Self {
        Self { agent, base_url, api_key, request_headers, temperature, top_p, max_tokens, reasoning_effort, models_url }
    }

    fn url(&self, suffix: &str) -> String {
        format!("{}/{suffix}", self.base_url.trim_end_matches('/'))
    }

    fn models_url(&self) -> String {
        self.models_url.clone().unwrap_or_else(|| self.url("models"))
    }

    /// Wiped on drop, for the same reason as the OpenAI provider's
    /// `authorization`.
    fn api_key(&self) -> Zeroizing<String> {
        Zeroizing::new(self.api_key.expose_secret().to_string())
    }

    fn headers(&self) -> Vec<(String, String)> {
        let mut headers = vec![("anthropic-version".to_string(), API_VERSION.to_string())];
        headers.extend(
            self.request_headers.iter().map(|(name, value)| (name.clone(), header_value(value))),
        );
        headers
    }
}

impl LlmProvider for AnthropicProvider {
    fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        let result = self.chat_stream(request, &|_| {}, &|_| {}, &|_, _, _| {}, &|| false)?;
        Ok(ChatResponse {
            content: (!result.text.is_empty()).then_some(result.text),
            tool_calls: result.tool_calls,
            usage: result.usage,
        })
    }

    fn chat_stream(
        &self,
        request: ChatRequest,
        on_delta: &dyn Fn(&str),
        on_reasoning: &dyn Fn(&str),
        on_tool_call_delta: &dyn Fn(&str, &str, &str),
        cancelled: &dyn Fn() -> bool,
    ) -> Result<ChatStreamResult, LlmError> {
        let mut body = body(
            &request,
            self.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
            self.temperature,
            self.top_p,
        );
        if let Some(effort) = &self.reasoning_effort {
            for (key, value) in thinking_params(effort) {
                body[key] = value;
            }
        }
        let mut post = self.agent.post(self.url("messages")).header("x-api-key", self.api_key().as_str());
        for (name, value) in self.headers() {
            post = post.header(name, value);
        }
        let lines = super::StreamLines::send(post, body);
        let mut result = ChatStreamResult::default();
        let mut usage = Usage::default();
        let mut saw_usage = false;
        // By block index: a round may interleave text, thinking and calls.
        let mut calls: Vec<(usize, LlmToolCall)> = Vec::new();
        let mut blocks: Vec<(usize, Value)> = Vec::new();

        while let Some(line) = lines.next(cancelled)? {
            let Some(event) = parse_sse_line(&line)? else { continue };
            match event {
                StreamEvent::MessageStart { message } => {
                    usage = message.usage;
                    saw_usage = true;
                }
                StreamEvent::ContentBlockStart { index, content_block } => {
                    if content_block["type"] == "tool_use" {
                        let id = content_block["id"].as_str().unwrap_or_default().to_string();
                        let name = content_block["name"].as_str().unwrap_or_default().to_string();
                        on_tool_call_delta(&id, &name, "");
                        calls.push((index, LlmToolCall { id, name, arguments: String::new() }));
                    }
                    blocks.push((index, content_block));
                }
                StreamEvent::ContentBlockDelta { index, delta } => match delta {
                    BlockDelta::TextDelta { text } if !text.is_empty() => {
                        on_delta(&text);
                        result.text.push_str(&text);
                        append(&mut blocks, index, "text", &text);
                    }
                    BlockDelta::ThinkingDelta { thinking } if !thinking.is_empty() => {
                        on_reasoning(&thinking);
                        result.reasoning.push_str(&thinking);
                        append(&mut blocks, index, "thinking", &thinking);
                    }
                    BlockDelta::SignatureDelta { signature } => {
                        append(&mut blocks, index, "signature", &signature);
                    }
                    BlockDelta::InputJsonDelta { partial_json } => {
                        if let Some((_, call)) = calls.iter_mut().find(|(i, _)| *i == index) {
                            call.arguments.push_str(&partial_json);
                            on_tool_call_delta(&call.id, &call.name, &call.arguments);
                        }
                    }
                    _ => {}
                },
                StreamEvent::MessageDelta { delta, usage: more } => {
                    // Cumulative, not an increment, and some versions repeat
                    // the input counts here too.
                    if let Some(more) = more {
                        usage.output_tokens = more.output_tokens;
                        usage.input_tokens = more.input_tokens.or(usage.input_tokens);
                        usage.cache_creation_input_tokens =
                            more.cache_creation_input_tokens.or(usage.cache_creation_input_tokens);
                        usage.cache_read_input_tokens =
                            more.cache_read_input_tokens.or(usage.cache_read_input_tokens);
                        saw_usage = true;
                    }
                    if matches!(
                        delta.stop_reason.as_deref(),
                        Some("max_tokens" | "model_context_window_exceeded")
                    ) {
                        result.truncated = true;
                    }
                    // A `200` that declined: whatever streamed before it is
                    // not an answer, and asking again gets the same decline.
                    if delta.stop_reason.as_deref() == Some("refusal") {
                        return Err(refusal_error(delta.stop_details.unwrap_or_default()));
                    }
                }
                StreamEvent::MessageStop => break,
                StreamEvent::Error { error } => return Err(stream_error(error)),
                StreamEvent::Other => {}
            }
        }

        result.native_content = native_content(blocks, &calls);
        result.tool_calls = calls.into_iter().map(|(_, call)| call).collect();
        result.usage = saw_usage.then(|| usage.into());
        Ok(result)
    }

    fn list_models(&self) -> Result<Vec<LlmModelInfo>, LlmError> {
        // Paged, twenty by default — more than there are models is one page.
        let mut get = self.agent.get(self.models_url()).query("limit", "1000").header("x-api-key", self.api_key().as_str());
        for (name, value) in self.headers() {
            get = get.header(name, value);
        }
        let response = get.call().map_err(|e| LlmError::Http(e.to_string()))?;
        let mut response = ok_or_status_error(response)?;
        let parsed: ModelsList =
            response.body_mut().read_json().map_err(|e| LlmError::Http(e.to_string()))?;
        Ok(parsed.data.into_iter().map(|d| LlmModelInfo { id: d.id }).collect())
    }
}

/// What `reasoning_effort` asks for. A number is a thinking budget in
/// tokens — the older models' `enabled` thinking, which the newer ones
/// refuse. Anything else is an effort level (`low` … `max`) for adaptive
/// thinking, the only kind the newer ones take — with its summary asked for,
/// since the newer ones otherwise stream thinking blocks with no text and the
/// reasoning pane stays empty. Passed through as written,
/// like the OpenAI-compatible provider's: which levels a model accepts is
/// the API's to say.
fn thinking_params(effort: &str) -> Vec<(&'static str, Value)> {
    let effort = effort.trim();
    match effort.parse::<u32>() {
        Ok(budget) => vec![("thinking", json!({ "type": "enabled", "budget_tokens": budget }))],
        Err(_) => vec![
            ("thinking", json!({ "type": "adaptive", "display": "summarized" })),
            ("output_config", json!({ "effort": effort })),
        ],
    }
}

fn append(blocks: &mut [(usize, Value)], index: usize, field: &str, text: &str) {
    if let Some((_, block)) = blocks.iter_mut().find(|(i, _)| *i == index) {
        let joined = format!("{}{text}", block[field].as_str().unwrap_or_default());
        block[field] = Value::String(joined);
    }
}

/// The round as the API sent it, when it thought — `None` otherwise, and the
/// message is rebuilt from its text and calls as before.
///
/// Also `None` when a thinking block never got its signature: a stop in the
/// middle of one. Sent back like that it is refused outright; left out, the
/// round is at worst answered without it.
fn native_content(blocks: Vec<(usize, Value)>, calls: &[(usize, LlmToolCall)]) -> Option<Value> {
    let thought = blocks.iter().any(|(_, b)| matches!(b["type"].as_str(), Some("thinking" | "redacted_thinking")));
    let signed = blocks
        .iter()
        .filter(|(_, b)| b["type"] == "thinking")
        .all(|(_, b)| b["signature"].as_str().is_some_and(|s| !s.is_empty()));
    if !thought || !signed {
        return None;
    }
    let content = blocks
        .into_iter()
        .filter_map(|(index, mut block)| {
            match block["type"].as_str() {
                // The API refuses an empty text block on the way in.
                Some("text") if block["text"].as_str().is_none_or(|t| t.trim().is_empty()) => return None,
                // The arguments arrived as fragments; the opening block had
                // only `{}`. Same object the rebuilt message would carry.
                Some("tool_use") => {
                    let arguments = calls.iter().find(|(i, _)| *i == index).map_or("", |(_, c)| c.arguments.as_str());
                    block["input"] = object_or_empty(arguments);
                }
                _ => {}
            }
            Some(block)
        })
        .collect();
    Some(Value::Array(content))
}

/// A model's safety classifier declined the request. Its category and
/// explanation are the only way the user learns why, so they go in the text.
fn refusal_error(details: StopDetails) -> LlmError {
    let category = details.category.map(|c| format!(" ({c})")).unwrap_or_default();
    let explanation = details.explanation.map(|e| format!(": {e}")).unwrap_or_default();
    LlmError::Provider(format!("the model declined this request{category}{explanation}"))
}

/// An `error` event arrives after the `200`, so `ok_or_status_error` never
/// sees it. Overloaded is the one worth retrying — the same as a 529 before
/// the stream began — and `llm_retry` still refuses if any output got out.
fn stream_error(error: ErrorBody) -> LlmError {
    if error.kind == "overloaded_error" {
        return LlmError::RateLimited { retry_after_seconds: None, message: error.message };
    }
    LlmError::Provider(format!("{}: {}", error.kind, error.message))
}

// ---------------------------------------------------------------- wire out

fn body(request: &ChatRequest, max_tokens: u32, temperature: Option<f32>, top_p: Option<f32>) -> Value {
    let messages = &request.messages;
    // What the app says before the conversation, and the conversation. Nothing
    // comes after it: the checklist lives in the history.
    let lead = messages.iter().take_while(|m| m.role == LlmRole::System).count();

    let mut system: Vec<Value> =
        messages[..lead].iter().filter_map(|m| text_block(m.content.as_deref())).collect();
    let mut conversation = wire_messages(&messages[lead..]);
    mark_cache_points(&mut system, &mut conversation);

    let tools: Vec<Value> = request
        .tools
        .iter()
        .map(|t| json!({ "name": t.name, "description": t.description, "input_schema": t.parameters }))
        .collect();

    let mut body = json!({
        "model": request.model,
        "max_tokens": max_tokens,
        "messages": conversation,
        "stream": true,
    });
    if !system.is_empty() {
        body["system"] = Value::Array(system);
    }
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools);
    }
    if let Some(temperature) = temperature {
        body["temperature"] = json!(temperature);
    }
    if let Some(top_p) = top_p {
        body["top_p"] = json!(top_p);
    }
    body
}

/// Where the cache is written and read. Three of the four the API allows:
///
/// - the end of the system prompt — tools and instructions, reused across
///   turns and after a compaction has rewritten the history;
/// - the last user message — this request's whole prefix, for the next round;
/// - the user message before it — exactly where the *previous* round wrote,
///   so this request reads it. The API looks back only about twenty blocks
///   from a point for an earlier entry, and a round of many parallel calls is
///   more than that; pointing at it outright does not depend on the reach.
///
/// Ephemeral, five minutes: rounds of a turn are seconds apart. A write costs
/// a quarter more than an uncached read, a hit a tenth; below the model's
/// minimum length the marker is ignored rather than refused.
fn mark_cache_points(system: &mut [Value], conversation: &mut [Value]) {
    let ephemeral = || json!({ "type": "ephemeral" });
    if let Some(last) = system.last_mut() {
        last["cache_control"] = ephemeral();
    }
    for message in conversation.iter_mut().rev().filter(|m| m["role"] == "user").take(2) {
        if let Some(block) = message["content"].as_array_mut().and_then(|c| c.last_mut()) {
            block["cache_control"] = ephemeral();
        }
    }
}

/// The conversation without its system messages, in alternating roles.
fn wire_messages(messages: &[LlmMessage]) -> Vec<Value> {
    let mut out: Vec<(&'static str, Vec<Value>)> = Vec::new();
    for message in messages {
        let (role, blocks) = match message.role {
            LlmRole::System => continue,
            // The pictures first, then the text: Anthropic's own advice.
            LlmRole::User => ("user", message.images.iter().map(image_block).chain(text_block(message.content.as_deref())).collect()),
            // Verbatim: see `LlmMessage::native_content`.
            LlmRole::Assistant if message.native_content.as_ref().is_some_and(Value::is_array) => {
                ("assistant", message.native_content.as_ref().and_then(Value::as_array).cloned().unwrap_or_default())
            }
            LlmRole::Assistant => {
                let mut blocks: Vec<Value> = text_block(message.content.as_deref()).into_iter().collect();
                blocks.extend(message.tool_calls.iter().map(|call| {
                    json!({
                        "type": "tool_use",
                        "id": wire_id(&call.id),
                        "name": call.name,
                        "input": object_or_empty(&call.arguments),
                    })
                }));
                ("assistant", blocks)
            }
            LlmRole::Tool => (
                "user",
                vec![json!({
                    "type": "tool_result",
                    "tool_use_id": wire_id(message.tool_call_id.as_deref().unwrap_or_default()),
                    "content": message.content.as_deref().unwrap_or_default(),
                })],
            ),
        };
        // A message with nothing in it is dropped, not sent empty: the API
        // rejects an empty text block, and an assistant round that produced
        // neither text nor a call has nothing to say.
        if blocks.is_empty() {
            continue;
        }
        match out.last_mut() {
            Some((last, content)) if *last == role => content.extend(blocks),
            _ => out.push((role, blocks)),
        }
    }
    out.into_iter()
        .map(|(role, content)| json!({ "role": role, "content": content }))
        .collect()
}

fn image_block(image: &ImagePart) -> Value {
    json!({ "type": "image", "source": { "type": "base64", "media_type": image.media_type.as_str(), "data": image.data } })
}

fn text_block(text: Option<&str>) -> Option<Value> {
    text.filter(|t| !t.trim().is_empty()).map(|t| json!({ "type": "text", "text": t }))
}

/// `input` must be an object. `LlmChat` already sanitizes what it echoes, but
/// a history can outlive that — it is saved, and resumed under another build.
fn object_or_empty(arguments: &str) -> Value {
    match serde_json::from_str::<Value>(arguments) {
        Ok(value @ Value::Object(_)) => value,
        _ => json!({}),
    }
}

/// Anthropic accepts `[A-Za-z0-9_-]` in a call id; other providers do not
/// keep to that (`functions.readFile:0`), and a chat can change provider
/// half way. Applied to both ends of a pair, so they still match.
fn wire_id(id: &str) -> String {
    id.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect()
}

// ----------------------------------------------------------------- wire in

#[derive(Deserialize)]
struct ModelsList {
    #[serde(default)]
    data: Vec<ModelsListEntry>,
}

#[derive(Deserialize)]
struct ModelsListEntry {
    id: String,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum StreamEvent {
    MessageStart { message: StartMessage },
    /// Kept as sent: a thinking round goes back verbatim.
    ContentBlockStart { index: usize, content_block: Value },
    ContentBlockDelta { index: usize, delta: BlockDelta },
    MessageDelta {
        #[serde(default)]
        delta: MessageDeltaBody,
        #[serde(default)]
        usage: Option<DeltaUsage>,
    },
    MessageStop,
    Error { error: ErrorBody },
    /// `ping`, `content_block_stop`, and whatever is added next.
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct StartMessage {
    #[serde(default)]
    usage: Usage,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum BlockDelta {
    TextDelta { text: String },
    InputJsonDelta { partial_json: String },
    ThinkingDelta { thinking: String },
    SignatureDelta { signature: String },
    #[serde(other)]
    Other,
}

#[derive(Default, Deserialize)]
struct MessageDeltaBody {
    #[serde(default)]
    stop_reason: Option<String>,
    #[serde(default)]
    stop_details: Option<StopDetails>,
}

#[derive(Default, Deserialize)]
struct StopDetails {
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    explanation: Option<String>,
}

#[derive(Default, Deserialize)]
struct Usage {
    #[serde(default)]
    input_tokens: Option<u32>,
    #[serde(default)]
    output_tokens: u32,
    #[serde(default)]
    cache_creation_input_tokens: Option<u32>,
    #[serde(default)]
    cache_read_input_tokens: Option<u32>,
}

#[derive(Deserialize)]
struct DeltaUsage {
    #[serde(default)]
    input_tokens: Option<u32>,
    #[serde(default)]
    output_tokens: u32,
    #[serde(default)]
    cache_creation_input_tokens: Option<u32>,
    #[serde(default)]
    cache_read_input_tokens: Option<u32>,
}

#[derive(Deserialize)]
struct ErrorBody {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    message: String,
}

/// `input_tokens` counts only what was neither written to nor read from the
/// cache. The context the round actually used is all three, and that is what
/// `ChatUsage::prompt_tokens` means everywhere else.
impl From<Usage> for ChatUsage {
    fn from(u: Usage) -> Self {
        let prompt = u.input_tokens.unwrap_or(0)
            + u.cache_creation_input_tokens.unwrap_or(0)
            + u.cache_read_input_tokens.unwrap_or(0);
        ChatUsage {
            prompt_tokens: prompt,
            completion_tokens: u.output_tokens,
            total_tokens: prompt + u.output_tokens,
            cached_tokens: u.cache_read_input_tokens.unwrap_or(0),
        }
    }
}

/// `None` for anything but a `data:` line. The `event:` line before it only
/// repeats the `type` the data already carries.
fn parse_sse_line(line: &str) -> Result<Option<StreamEvent>, LlmError> {
    let Some(data) = line.trim_end_matches('\r').strip_prefix("data:") else {
        return Ok(None);
    };
    let data = data.trim();
    if data.is_empty() {
        return Ok(None);
    }
    serde_json::from_str(data)
        .map(Some)
        .map_err(|e| LlmError::Provider(format!("unreadable stream event: {e}: {data}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::llm::LlmToolDefinition;
    use crate::infra::llm_providers::openai_compatible::tests::{serve, serve_capturing, serve_with_headers};
    use crate::domain::settings::{ProviderConfig, ProviderKind};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    fn call(id: &str, arguments: &str) -> LlmToolCall {
        LlmToolCall { id: id.into(), name: "readFile".into(), arguments: arguments.into() }
    }

    fn request(messages: Vec<LlmMessage>) -> ChatRequest {
        ChatRequest { messages, tools: vec![], model: "claude".into() }
    }

    fn provider(base_url: String) -> AnthropicProvider {
        AnthropicProvider::new(
            crate::infra::http_agent::build_agent(None).expect("agent"),
            base_url,
            SecretString::from("sk-ant"),
            HashMap::new(),
            None,
            None,
            None,
            None,
            None,
        )
    }

    fn sse(events: &[&str]) -> String {
        events.iter().map(|e| format!("event: x\ndata: {e}\n\n")).collect()
    }

    fn stream(url: String) -> ChatStreamResult {
        provider(url)
            .chat_stream(request(vec![LlmMessage::user("hi")]), &|_| {}, &|_| {}, &|_, _, _| {}, &|| false)
            .expect("streams")
    }

    /// The whole conversion in one round trip: every system message into the
    /// field, a call and its result as blocks, and a round's two results plus
    /// the user's next words as one user message.
    #[test]
    fn a_turn_with_tools_becomes_alternating_block_messages() {
        let messages = vec![
            LlmMessage::system("be brief"),
            LlmMessage::system("the repo is /x"),
            LlmMessage::user("read both"),
            LlmMessage::tool_requests(vec![call("c1", r#"{"path":"a"}"#), call("c2", r#"{"path":"b"}"#)]),
            LlmMessage::tool_result("c1", "A"),
            LlmMessage::tool_result("c2", "B"),
            LlmMessage::user("and now?"),
            LlmMessage::assistant("done"),
        ];
        let wire = json!(wire_messages(&messages));
        let body = body(&request(messages), 100, Some(0.5), Some(0.9));

        assert_eq!(
            body["system"],
            json!([{"type":"text","text":"be brief"},{"type":"text","text":"the repo is /x","cache_control":{"type":"ephemeral"}}])
        );
        assert_eq!(
            wire,
            json!([
                {"role":"user","content":[{"type":"text","text":"read both"}]},
                {"role":"assistant","content":[
                    {"type":"tool_use","id":"c1","name":"readFile","input":{"path":"a"}},
                    {"type":"tool_use","id":"c2","name":"readFile","input":{"path":"b"}}
                ]},
                {"role":"user","content":[
                    {"type":"tool_result","tool_use_id":"c1","content":"A"},
                    {"type":"tool_result","tool_use_id":"c2","content":"B"},
                    {"type":"text","text":"and now?"}
                ]},
                {"role":"assistant","content":[{"type":"text","text":"done"}]}
            ])
        );
        assert_eq!((body["max_tokens"].as_u64(), body["temperature"].as_f64()), (Some(100), Some(0.5)));
        assert_eq!(body["top_p"].as_f64().map(|p| (p * 100.0).round()), Some(90.0));
        assert_eq!(body["stream"], true);
    }

    /// The three points: the end of the prompt, and the last two user
    /// messages, where the next round finds the prefix this one wrote.
    #[test]
    fn the_cache_points_end_the_prompt_and_the_last_two_user_messages() {
        let messages = vec![
            LlmMessage::system("rules"),
            LlmMessage::user("first"),
            LlmMessage::assistant("one"),
            LlmMessage::user("second"),
            LlmMessage::tool_requests(vec![call("c1", "{}")]),
            LlmMessage::tool_result("c1", "result"),
        ];
        let body = body(&request(messages), 1, None, None);
        let cached = json!({"type":"ephemeral"});

        assert_eq!(body["system"][0]["cache_control"], cached);
        let wire = body["messages"].as_array().unwrap();
        assert!(wire[0]["content"][0].get("cache_control").is_none(), "only the last two user messages");
        assert_eq!(wire[2]["content"][0]["cache_control"], cached);
        assert_eq!(
            wire[4]["content"],
            json!([
                {"type":"tool_result","tool_use_id":"c1","content":"result","cache_control":{"type":"ephemeral"}}
            ])
        );
        assert_eq!(wire.len(), 5);
    }

    fn image(data: &str) -> ImagePart {
        use crate::domain::image::ImageMediaType;
        ImagePart { media_type: ImageMediaType::Jpeg, data: data.into(), width: 1, height: 1 }
    }

    /// Pictures as base64 image blocks before the text, and the cache point
    /// still on the message's last block.
    #[test]
    fn pictures_go_as_image_blocks_before_the_text() {
        let messages = vec![LlmMessage::user_with_images("what is this?", vec![image("A"), image("B")])];
        let body = body(&request(messages), 1, None, None);
        assert_eq!(
            body["messages"][0]["content"],
            json!([
                {"type":"image","source":{"type":"base64","media_type":"image/jpeg","data":"A"}},
                {"type":"image","source":{"type":"base64","media_type":"image/jpeg","data":"B"}},
                {"type":"text","text":"what is this?","cache_control":{"type":"ephemeral"}}
            ])
        );
    }

    /// A picture sent without a word is still a message, not one dropped for
    /// having no text.
    #[test]
    fn a_picture_alone_is_still_sent() {
        let wire = json!(wire_messages(&[LlmMessage::user_with_images(" ", vec![image("A")])]));
        assert_eq!(
            wire,
            json!([{"role":"user","content":[{"type":"image","source":{"type":"base64","media_type":"image/jpeg","data":"A"}}]}])
        );
    }

    #[test]
    fn tools_are_sent_with_their_schema_as_input_schema() {
        let mut req = request(vec![LlmMessage::user("hi")]);
        req.tools = vec![LlmToolDefinition {
            name: "grep".into(),
            description: "search".into(),
            parameters: json!({"type":"object"}),
        }];
        let body = body(&req, 1, None, None);
        assert_eq!(body["tools"], json!([{"name":"grep","description":"search","input_schema":{"type":"object"}}]));
        assert!(body.get("temperature").is_none(), "unset is not sent");
        assert!(body.get("top_p").is_none(), "unset is not sent");
        assert!(body.get("system").is_none(), "no system messages, no field");
    }

    /// The API rejects an empty text block, and an assistant message with
    /// text beside its calls keeps both.
    #[test]
    fn empty_text_is_left_out_and_text_beside_calls_is_kept() {
        let mut with_text = LlmMessage::tool_requests(vec![call("c1", "{}")]);
        with_text.content = Some("Looking.".into());
        let messages = vec![
            LlmMessage::user("go"),
            LlmMessage::assistant("  "),
            LlmMessage::user("on"),
            with_text,
            LlmMessage::tool_result("c1", ""),
        ];
        assert_eq!(
            json!(wire_messages(&messages)),
            json!([
                {"role":"user","content":[{"type":"text","text":"go"},{"type":"text","text":"on"}]},
                {"role":"assistant","content":[
                    {"type":"text","text":"Looking."},
                    {"type":"tool_use","id":"c1","name":"readFile","input":{}}
                ]},
                {"role":"user","content":[{"type":"tool_result","tool_use_id":"c1","content":""}]}
            ])
        );
    }

    /// A chat started on another provider carries its ids and whatever it
    /// echoed; both must still be sendable here.
    #[test]
    fn foreign_ids_and_broken_arguments_are_made_sendable() {
        let messages = vec![
            LlmMessage::user("go"),
            LlmMessage::tool_requests(vec![call("functions.readFile:0", "{}\"\""), call("c2", "[1]")]),
            LlmMessage::tool_result("functions.readFile:0", "x"),
            LlmMessage::tool_result("c2", "y"),
        ];
        let wire = &json!(wire_messages(&messages));
        assert_eq!(wire[1]["content"][0]["id"], "functions_readFile_0");
        assert_eq!(wire[1]["content"][0]["input"], json!({}));
        assert_eq!(wire[1]["content"][1]["input"], json!({}), "valid JSON, but not an object");
        assert_eq!(wire[2]["content"][0]["tool_use_id"], "functions_readFile_0");
    }

    #[test]
    fn a_stream_is_collected_into_text_calls_and_usage() {
        let (url, server) = serve(
            "200 OK",
            sse(&[
                r#"{"type":"message_start","message":{"usage":{"input_tokens":10,"cache_creation_input_tokens":20,"cache_read_input_tokens":30,"output_tokens":1}}}"#,
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
                r#"{"type":"ping"}"#,
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Look"}}"#,
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"ing."}}"#,
                r#"{"type":"content_block_stop","index":0}"#,
                r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"readFile","input":{}}}"#,
                r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}"#,
                r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"\"a\"}"}}"#,
                r#"{"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_2","name":"grep","input":{}}}"#,
                r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":7}}"#,
                r#"{"type":"message_stop"}"#,
            ]),
        );
        let seen = Arc::new(Mutex::new(Vec::new()));
        let progress = Arc::clone(&seen);
        let result = provider(url)
            .chat_stream(
                request(vec![LlmMessage::user("hi")]),
                &|_| {},
                &|_| {},
                &move |id, _, args| progress.lock().unwrap().push(format!("{id}={args}")),
                &|| false,
            )
            .expect("streams");
        server.join().ok();

        assert_eq!(result.text, "Looking.");
        assert_eq!(
            result.tool_calls,
            vec![
                LlmToolCall { id: "toolu_1".into(), name: "readFile".into(), arguments: r#"{"path":"a"}"#.into() },
                LlmToolCall { id: "toolu_2".into(), name: "grep".into(), arguments: String::new() },
            ]
        );
        assert_eq!(
            *seen.lock().unwrap(),
            ["toolu_1=", r#"toolu_1={"path":"#, r#"toolu_1={"path":"a"}"#, "toolu_2="],
            "progress is the arguments so far, from the moment the call opens"
        );
        // Cached tokens are context too.
        assert_eq!(
            result.usage,
            Some(ChatUsage { prompt_tokens: 60, completion_tokens: 7, total_tokens: 67, cached_tokens: 30 })
        );
        assert!(!result.truncated);
    }

    /// What the configuration picks is what goes out: the endpoint, the key
    /// in Anthropic's header rather than a bearer token, the pinned version.
    #[test]
    fn the_request_goes_to_messages_with_anthropics_headers() {
        let (url, server) = serve_capturing(sse(&[r#"{"type":"message_stop"}"#]));
        let config = ProviderConfig {
            id: "claude".into(),
            kind: ProviderKind::Anthropic,
            base_url: format!("{url}/v1/"),
            ..Default::default()
        };
        crate::infra::llm_providers::provider_for(&config, Some(SecretString::from("sk-ant")))
            .expect("builds")
            .chat_stream(request(vec![LlmMessage::user("hi")]), &|_| {}, &|_| {}, &|_, _, _| {}, &|| false)
            .expect("streams");
        let sent = server.join().expect("served").to_lowercase();

        assert!(sent.starts_with("post /v1/messages "), "{sent}");
        assert!(sent.contains("x-api-key: sk-ant\r\n"), "{sent}");
        assert!(sent.contains("anthropic-version: 2023-06-01\r\n"), "{sent}");
        assert!(!sent.contains("authorization:"), "{sent}");
    }

    /// A proxy may hold the connection open after the message ends; the
    /// round is over at `message_stop`, whatever follows.
    #[test]
    fn the_round_ends_at_message_stop() {
        let (url, server) = serve(
            "200 OK",
            sse(&[
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"done"}}"#,
                r#"{"type":"message_stop"}"#,
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" and more"}}"#,
            ]),
        );
        let result = stream(url);
        server.join().ok();
        assert_eq!(result.text, "done");
    }

    #[test]
    fn cancelling_returns_what_had_arrived_so_far() {
        let (url, server) = serve(
            "200 OK",
            sse(&[
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"first"}}"#,
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"second"}}"#,
            ]),
        );
        let stop = AtomicBool::new(false);
        let result = provider(url)
            .chat_stream(
                request(vec![]),
                &|_| stop.store(true, Ordering::SeqCst),
                &|_| {},
                &|_, _, _| {},
                &|| stop.load(Ordering::SeqCst),
            )
            .expect("cancelling is not an error");
        server.join().ok();
        assert_eq!(result.text, "first");
    }

    /// Interleaved: thinking between the calls, and the whole round must go
    /// back in that order with the signatures intact.
    #[test]
    fn a_round_that_thought_is_kept_whole_and_in_order() {
        let (url, server) = serve(
            "200 OK",
            sse(&[
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}"#,
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Read "}}"#,
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"it."}}"#,
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig1"}}"#,
                r#"{"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}"#,
                r#"{"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_1","name":"readFile","input":{}}}"#,
                r#"{"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"a\"}"}}"#,
                r#"{"type":"content_block_start","index":3,"content_block":{"type":"redacted_thinking","data":"opaque"}}"#,
                r#"{"type":"content_block_start","index":4,"content_block":{"type":"text","text":""}}"#,
                r#"{"type":"content_block_delta","index":4,"delta":{"type":"text_delta","text":"Now b."}}"#,
                r#"{"type":"content_block_start","index":5,"content_block":{"type":"tool_use","id":"toolu_2","name":"readFile","input":{}}}"#,
                r#"{"type":"message_stop"}"#,
            ]),
        );
        let result = stream(url);
        server.join().ok();

        assert_eq!(result.reasoning, "Read it.");
        assert_eq!(
            result.native_content,
            Some(json!([
                {"type":"thinking","thinking":"Read it.","signature":"sig1"},
                {"type":"tool_use","id":"toolu_1","name":"readFile","input":{"path":"a"}},
                {"type":"redacted_thinking","data":"opaque"},
                {"type":"text","text":"Now b."},
                {"type":"tool_use","id":"toolu_2","name":"readFile","input":{}}
            ])),
            "the empty text block is dropped — the API refuses one on the way in"
        );
    }

    /// Nothing to preserve: the message is rebuilt from its fields, as before.
    #[test]
    fn a_round_without_thinking_has_no_native_content() {
        let (url, server) = serve(
            "200 OK",
            sse(&[
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}"#,
            ]),
        );
        let result = stream(url);
        server.join().ok();
        assert_eq!(result.native_content, None);
    }

    /// Redacted thinking has no text to show, and must go back all the same.
    #[test]
    fn a_round_with_only_redacted_thinking_is_kept() {
        let (url, server) = serve(
            "200 OK",
            sse(&[
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"redacted_thinking","data":"opaque"}}"#,
                r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"grep","input":{}}}"#,
            ]),
        );
        let result = stream(url);
        server.join().ok();
        assert_eq!(
            result.native_content,
            Some(json!([
                {"type":"redacted_thinking","data":"opaque"},
                {"type":"tool_use","id":"toolu_1","name":"grep","input":{}}
            ]))
        );
    }

    /// Stopped before the signature: refused if sent, so not kept.
    #[test]
    fn an_unsigned_thinking_block_is_not_kept() {
        let (url, server) = serve(
            "200 OK",
            sse(&[
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}"#,
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hm"}}"#,
            ]),
        );
        let result = stream(url);
        server.join().ok();
        assert_eq!(result.native_content, None);
        assert_eq!(result.reasoning, "hm", "still shown");
    }

    #[test]
    fn native_content_is_sent_verbatim_in_place_of_the_rebuilt_message() {
        let native = json!([
            {"type":"thinking","thinking":"t","signature":"s"},
            {"type":"tool_use","id":"toolu_1","name":"readFile","input":{}}
        ]);
        let mut asked = LlmMessage::tool_requests(vec![call("toolu_1", "{}")]);
        asked.native_content = Some(native.clone());
        let wire = wire_messages(&[LlmMessage::user("go"), asked, LlmMessage::tool_result("toolu_1", "x")]);
        assert_eq!(wire[1], json!({"role":"assistant","content":native}));
    }

    #[test]
    fn effort_asks_for_adaptive_thinking_and_a_number_for_a_budget() {
        assert_eq!(
            thinking_params(" high "),
            vec![
                ("thinking", json!({"type":"adaptive","display":"summarized"})),
                ("output_config", json!({"effort":"high"}))
            ]
        );
        assert_eq!(thinking_params("4096"), vec![("thinking", json!({"type":"enabled","budget_tokens":4096}))]);
    }

    /// Set in the provider's settings, it reaches the request; unset, the
    /// model's own default stands and nothing is sent.
    #[test]
    fn the_configured_effort_reaches_the_request() {
        for (effort, expected) in [(Some("high"), true), (None, false)] {
            let (url, server) = serve_capturing(sse(&[r#"{"type":"message_stop"}"#]));
            let config = ProviderConfig {
                id: "claude".into(),
                kind: ProviderKind::Anthropic,
                base_url: url,
                reasoning_effort: effort.map(str::to_string),
                ..Default::default()
            };
            crate::infra::llm_providers::provider_for(&config, Some(SecretString::from("k")))
                .expect("builds")
                .chat_stream(request(vec![LlmMessage::user("hi")]), &|_| {}, &|_| {}, &|_, _, _| {}, &|| false)
                .expect("streams");
            let sent = server.join().expect("served");
            let body: Value = serde_json::from_str(sent.split("\r\n\r\n").nth(1).expect("a body")).expect("json");
            assert_eq!(body.get("output_config") == Some(&json!({"effort":"high"})), expected, "{sent}");
            assert_eq!(
                body.get("thinking") == Some(&json!({"type":"adaptive","display":"summarized"})),
                expected,
                "{sent}"
            );
        }
    }

    /// A refusal arrives as an ordinary stop after a `200`; read as one, the
    /// turn ends in silence and the loop nudges the same request again.
    #[test]
    fn a_refusal_is_an_error_that_says_why() {
        let (url, server) = serve(
            "200 OK",
            sse(&[
                r#"{"type":"message_delta","delta":{"stop_reason":"refusal","stop_details":{"type":"refusal","category":"cyber","explanation":"declined"}},"usage":{"output_tokens":0}}"#,
            ]),
        );
        let err = provider(url)
            .chat_stream(request(vec![LlmMessage::user("hi")]), &|_| {}, &|_| {}, &|_, _, _| {}, &|| false)
            .expect_err("a refusal");
        server.join().ok();
        let message = err.to_string();
        assert!(message.contains("declined this request (cyber): declined"), "{message}");
    }

    #[test]
    fn running_out_of_max_tokens_is_a_truncated_round() {
        let (url, server) = serve(
            "200 OK",
            sse(&[
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"half a sen"}}"#,
                r#"{"type":"message_delta","delta":{"stop_reason":"max_tokens"},"usage":{"output_tokens":5}}"#,
            ]),
        );
        let result = stream(url);
        server.join().ok();
        assert!(result.truncated);
        assert_eq!(result.text, "half a sen");
    }

    #[test]
    fn thinking_is_reported_apart_from_the_answer() {
        let (url, server) = serve(
            "200 OK",
            sse(&[
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm"}}"#,
                r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"yes"}}"#,
            ]),
        );
        let result = stream(url);
        server.join().ok();
        assert_eq!((result.reasoning.as_str(), result.text.as_str()), ("hmm", "yes"));
    }

    /// After the `200`, so no status says so. Overloaded is retryable;
    /// anything else is the provider's answer.
    #[test]
    fn an_error_event_fails_the_round() {
        let (url, server) = serve(
            "200 OK",
            sse(&[r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#]),
        );
        let err = provider(url)
            .chat_stream(request(vec![]), &|_| {}, &|_| {}, &|_, _, _| {}, &|| false)
            .unwrap_err();
        server.join().ok();
        assert!(matches!(err, LlmError::RateLimited { retry_after_seconds: None, .. }), "{err}");

        let (url, server) = serve(
            "200 OK",
            sse(&[r#"{"type":"error","error":{"type":"api_error","message":"boom"}}"#]),
        );
        let err = provider(url)
            .chat_stream(request(vec![]), &|_| {}, &|_| {}, &|_, _, _| {}, &|| false)
            .unwrap_err();
        server.join().ok();
        assert!(matches!(&err, LlmError::Provider(m) if m == "api_error: boom"), "{err}");
    }

    /// Before the stream, overloaded is a status — 529, retried like 429.
    #[test]
    fn an_overloaded_status_is_retryable() {
        let (url, server) = serve_with_headers("529 Overloaded", "retry-after: 3\r\n", "{}".into());
        let err = provider(url)
            .chat_stream(request(vec![]), &|_| {}, &|_| {}, &|_, _, _| {}, &|| false)
            .unwrap_err();
        server.join().ok();
        assert!(matches!(err, LlmError::RateLimited { retry_after_seconds: Some(3), .. }), "{err}");
    }

    /// The API's own wording, which compaction must recognise to shrink the
    /// conversation instead of failing the turn.
    #[test]
    fn the_apis_too_long_error_reads_as_a_context_length_error() {
        let (url, server) = serve(
            "400 Bad Request",
            r#"{"type":"error","error":{"type":"invalid_request_error","message":"prompt is too long: 210000 tokens > 200000 maximum"}}"#.into(),
        );
        let err = provider(url)
            .chat_stream(request(vec![]), &|_| {}, &|_| {}, &|_, _, _| {}, &|| false)
            .unwrap_err();
        server.join().ok();
        assert!(crate::domain::compaction::is_context_length_error(&err.to_string()), "{err}");
    }

    #[test]
    fn models_are_listed() {
        let (url, server) = serve(
            "200 OK",
            r#"{"data":[{"id":"claude-opus-5","type":"model"},{"id":"claude-haiku-4-5","type":"model"}],"has_more":false}"#.into(),
        );
        let models = provider(url).list_models().expect("lists");
        server.join().ok();
        assert_eq!(
            models.into_iter().map(|m| m.id).collect::<Vec<_>>(),
            ["claude-opus-5", "claude-haiku-4-5"]
        );
    }

    #[test]
    fn a_malformed_event_is_a_provider_error() {
        assert!(matches!(parse_sse_line("data: {not json"), Err(LlmError::Provider(_))));
        assert!(matches!(parse_sse_line("event: ping"), Ok(None)));
        assert!(matches!(parse_sse_line("data: {\"type\":\"future_event\"}\r"), Ok(Some(StreamEvent::Other))));
    }
}
