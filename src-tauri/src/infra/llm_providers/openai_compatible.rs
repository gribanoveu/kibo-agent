//! OpenAI-compatible chat completions.
//!
//! Blocking HTTP, because the loop above is sequential and a blocking client is
//! markedly easier to debug than an async one whose colour spreads through
//! every service that touches it.
//!
//! Both `chat` and `chat_stream` go through the streaming endpoint. That is not
//! tidiness: several OpenAI-compatible corporate proxies reject a
//! non-streaming `/chat/completions` with HTTP 400 — sometimes with an empty
//! body — while accepting the identical payload with `"stream": true`.
//!
//! Reasoning goes back on the same assistant message, under the name it came
//! in — `reasoning_content` or `reasoning` — carried in
//! `LlmMessage::native_content`. DeepSeek requires it once a request carries
//! tools (without it, the round after a thinking round with tool calls is a
//! 400), and its models are trained to keep that reasoning across the calls of
//! a task. OpenRouter sends `reasoning` and takes it back for the same reason.
//! Without it GLM there read one 47-line file over thirty times in one turn,
//! as if each round started from nothing (`agent_bench`, `migrate-records`).
//! A server that never sent reasoning never gets any back.

use std::collections::HashMap;

use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::domain::llm::{
    ChatRequest, ChatResponse, ChatStreamResult, ChatUsage, LlmError, LlmMessage, LlmModelInfo,
    LlmProvider, LlmRole, LlmToolCall,
};

/// Placeholder a configured request header can carry to get a fresh
/// correlation id per request — several corporate gateways require one.
pub const REQUEST_HEADER_VALUE_UUID: &str = "$uuid";

/// How much of an error response body is folded into the message. A provider's
/// error page can be an entire HTML document; the goal is a diagnosable line,
/// not a dump.
const ERROR_BODY_MAX_CHARS: usize = 2000;

/// The two names reasoning arrives under, and goes back under — see the
/// module comment.
const REASONING_CONTENT: &str = "reasoning_content";
const REASONING: &str = "reasoning";

pub struct OpenAiCompatibleProvider {
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

impl OpenAiCompatibleProvider {
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
        Self {
            agent,
            base_url,
            api_key,
            request_headers,
            temperature,
            top_p,
            max_tokens,
            reasoning_effort,
            models_url,
        }
    }

    /// The `Authorization` value, in a buffer that wipes itself on drop.
    ///
    /// `ureq` wants a `&str`, so the key unavoidably exists in a formatted copy
    /// for the length of the call. `Zeroizing` is what stops that copy being
    /// freed and left behind in the heap for the next crash dump.
    fn authorization(&self) -> Zeroizing<String> {
        Zeroizing::new(format!("Bearer {}", self.api_key.expose_secret()))
    }

    fn url(&self, suffix: &str) -> String {
        format!("{}/{suffix}", self.base_url.trim_end_matches('/'))
    }

    fn models_url(&self) -> String {
        self.models_url.clone().unwrap_or_else(|| self.url("models"))
    }

    fn body<'a>(&self, request: &'a ChatRequest, stream: bool) -> WireRequest<'a> {
        let tools: Vec<WireTool> = request
            .tools
            .iter()
            .map(|t| WireTool {
                kind: "function",
                function: WireFunction {
                    name: &t.name,
                    description: &t.description,
                    parameters: &t.parameters,
                },
            })
            .collect();

        WireRequest {
            model: &request.model,
            // In the order it was built: the system messages lead, the only
            // place several local chat templates accept one, and nothing adds
            // one later — the checklist lives in the history.
            messages: request.messages.iter().map(WireMessage::from).collect(),
            // Only sent when tools are actually offered: an empty `tools` with
            // `tool_choice: "auto"` is pointless, and some servers reject it.
            tool_choice: (!tools.is_empty()).then_some("auto"),
            tools,
            stream,
            stream_options: stream.then_some(StreamOptions { include_usage: true }),
            temperature: self.temperature,
            top_p: self.top_p,
            max_tokens: self.max_tokens,
            reasoning_effort: self.reasoning_effort.clone(),
        }
    }

    /// `(name, value)` pairs every request carries. Applied at each call site
    /// rather than through a shared helper: ureq's builder is typestate, and a
    /// generic over it costs more than saying this twice.
    fn extra_headers(&self) -> Vec<(String, String)> {
        self.request_headers
            .iter()
            .map(|(name, value)| (name.clone(), header_value(value)))
            .collect()
    }
}

impl LlmProvider for OpenAiCompatibleProvider {
    /// Runs the streaming endpoint and collects it. See the module comment for
    /// why there is no separate non-streaming path.
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
        let body = serde_json::to_value(self.body(&request, true)).map_err(|e| LlmError::Message(e.to_string()))?;
        let mut post = self
            .agent
            .post(self.url("chat/completions"))
            .header("Authorization", self.authorization().as_str());
        for (name, value) in self.extra_headers() {
            post = post.header(name, value);
        }
        let lines = super::StreamLines::send(post, body);
        let mut result = ChatStreamResult::default();
        let mut calls = ToolCallAccumulator::default();
        let mut echo_reasoning: Option<&'static str> = None;

        while let Some(line) = lines.next(cancelled)? {
            match parse_sse_line(&line)? {
                SseLine::Ignore => {}
                SseLine::Done => break,
                SseLine::Chunk {
                    delta,
                    reasoning,
                    reasoning_field,
                    usage,
                    tool_calls,
                    finish_reason,
                } => {
                    echo_reasoning = echo_reasoning.or(reasoning_field);
                    if let Some(text) = delta {
                        on_delta(&text);
                        result.text.push_str(&text);
                    }
                    if let Some(text) = reasoning {
                        on_reasoning(&text);
                        result.reasoning.push_str(&text);
                    }
                    // Whichever chunk carries it, rather than assuming the
                    // trailing one does.
                    if usage.is_some() {
                        result.usage = usage;
                    }
                    if !tool_calls.is_empty() {
                        calls.ingest(tool_calls);
                        for (id, name, arguments) in calls.snapshots() {
                            on_tool_call_delta(&id, name, arguments);
                        }
                    }
                    // Latched, not assigned: exactly one chunk carries a
                    // finish reason, and a later usage-only chunk arriving
                    // with none must not erase what it said.
                    if finish_reason.as_deref() == Some("length") {
                        result.truncated = true;
                    }
                }
            }
        }

        result.tool_calls = calls.finish();
        if let Some(field) = echo_reasoning.filter(|_| !result.reasoning.is_empty()) {
            let mut native = serde_json::Map::new();
            native.insert(field.to_string(), serde_json::Value::String(result.reasoning.clone()));
            result.native_content = Some(serde_json::Value::Object(native));
        }
        Ok(result)
    }

    fn list_models(&self) -> Result<Vec<LlmModelInfo>, LlmError> {
        let mut get = self
            .agent
            .get(self.models_url())
            .header("Authorization", self.authorization().as_str());
        for (name, value) in self.extra_headers() {
            get = get.header(name, value);
        }
        let response = get.call().map_err(|e| LlmError::Http(e.to_string()))?;
        let mut response = ok_or_status_error(response)?;

        let parsed: ModelsList = response
            .body_mut()
            .read_json()
            .map_err(|e| LlmError::Http(e.to_string()))?;
        Ok(parsed
            .data
            .into_iter()
            .map(|d| LlmModelInfo { id: d.id })
            .collect())
    }
}

/// Turns a non-2xx response into an error that still carries the body.
///
/// The agent is built with `http_status_as_error(false)` precisely so the body
/// survives to here — ureq's own conversion discards it, which is how a
/// provider's explanation becomes an undiagnosable status number.
pub(super) fn ok_or_status_error(
    mut response: http::Response<ureq::Body>,
) -> Result<http::Response<ureq::Body>, LlmError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let headers = response.headers().clone();
    let body = response
        .body_mut()
        .read_to_string()
        .unwrap_or_else(|e| format!("<could not read the error body: {e}>"));
    let body = if body.chars().count() > ERROR_BODY_MAX_CHARS {
        format!(
            "{}… (truncated)",
            body.chars().take(ERROR_BODY_MAX_CHARS).collect::<String>()
        )
    } else {
        body
    };
    // 429 is the one status worth trying again unchanged, so it gets its own
    // variant rather than being recognised later by matching on the message.
    // 529 is Anthropic's "overloaded": not this caller's limit, but the same
    // advice — the request was fine, send it again later.
    if matches!(status.as_u16(), 429 | 529) {
        return Err(LlmError::RateLimited {
            retry_after_seconds: retry_after_seconds(&headers),
            message: format!("http status {}: {body}", status.as_u16()),
        });
    }
    // A gateway timing out or restarting under a long turn. 501 and 505 are
    // not here: they say the request itself is one the server will never take.
    if matches!(status.as_u16(), 500 | 502 | 503 | 504) {
        return Err(LlmError::Unavailable {
            retry_after_seconds: retry_after_seconds(&headers),
            message: format!("http status {}: {body}", status.as_u16()),
        });
    }
    Err(LlmError::Http(format!(
        "http status {}: {body}",
        status.as_u16()
    )))
}

/// `Retry-After` as the LLM gateways actually send it: a number of seconds.
///
/// The header also permits an HTTP date, which none of them use and which is
/// deliberately not parsed — an unreadable hint reads as no hint, and the
/// backoff covers that case anyway.
fn retry_after_seconds(headers: &http::HeaderMap) -> Option<u64> {
    headers
        .get("retry-after")?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()
}

pub(super) fn header_value(value: &str) -> String {
    if value == REQUEST_HEADER_VALUE_UUID {
        uuid::Uuid::new_v4().to_string()
    } else {
        value.to_string()
    }
}

/// Reasoning this provider kept on `message` under `field`, if any.
fn kept_reasoning<'a>(message: &'a LlmMessage, field: &str) -> Option<&'a str> {
    message.native_content.as_ref()?.get(field)?.as_str()
}

fn role_str(role: LlmRole) -> &'static str {
    match role {
        LlmRole::System => "system",
        LlmRole::User => "user",
        LlmRole::Assistant => "assistant",
        LlmRole::Tool => "tool",
    }
}

// ---------------------------------------------------------------- wire out

#[derive(Serialize)]
struct WireRequest<'a> {
    model: &'a str,
    messages: Vec<WireMessage<'a>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<WireTool<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<&'static str>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<StreamOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Owned rather than borrowed: it comes from the provider, not the
    /// request, and tying the body's lifetime to `self` buys nothing.
    reasoning_effort: Option<String>,
}

#[derive(Serialize)]
struct StreamOptions {
    include_usage: bool,
}

#[derive(Serialize)]
struct WireMessage<'a> {
    role: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_content: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tool_calls: Vec<WireToolCall<'a>>,
}

impl<'a> From<&'a LlmMessage> for WireMessage<'a> {
    fn from(message: &'a LlmMessage) -> Self {
        Self {
            role: role_str(message.role),
            content: message.content.as_deref(),
            // An object with these keys is ours; Anthropic's blocks are an array.
            reasoning_content: kept_reasoning(message, REASONING_CONTENT),
            reasoning: kept_reasoning(message, REASONING),
            tool_call_id: message.tool_call_id.as_deref(),
            tool_calls: message
                .tool_calls
                .iter()
                .map(|c| WireToolCall {
                    id: &c.id,
                    kind: "function",
                    function: WireToolCallFunction {
                        name: &c.name,
                        arguments: &c.arguments,
                    },
                })
                .collect(),
        }
    }
}

#[derive(Serialize)]
struct WireToolCall<'a> {
    id: &'a str,
    #[serde(rename = "type")]
    kind: &'static str,
    function: WireToolCallFunction<'a>,
}

#[derive(Serialize)]
struct WireToolCallFunction<'a> {
    name: &'a str,
    arguments: &'a str,
}

#[derive(Serialize)]
struct WireTool<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    function: WireFunction<'a>,
}

#[derive(Serialize)]
struct WireFunction<'a> {
    name: &'a str,
    description: &'a str,
    parameters: &'a serde_json::Value,
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
struct StreamChunk {
    #[serde(default)]
    choices: Vec<StreamChoice>,
    #[serde(default)]
    usage: Option<StreamUsage>,
}

#[derive(Deserialize)]
struct StreamChoice {
    #[serde(default)]
    delta: StreamDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Default, Deserialize)]
struct StreamDelta {
    #[serde(default)]
    content: Option<String>,
    /// Two spellings in the wild for the same thing.
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default)]
    reasoning: Option<String>,
    /// Null rather than absent on several servers, so this cannot be a plain
    /// `Vec` with `#[serde(default)]`.
    #[serde(default)]
    tool_calls: Option<Vec<StreamToolCall>>,
}

impl StreamDelta {
    fn reasoning_text(&mut self) -> Option<String> {
        self.reasoning_content
            .take()
            .or_else(|| self.reasoning.take())
            .filter(|s| !s.is_empty())
    }
}

/// One fragment of a tool call. A call's id and name arrive in one fragment and
/// its arguments across however many more share the same `index` — which is
/// what makes the accumulator necessary.
#[derive(Deserialize)]
struct StreamToolCall {
    #[serde(default)]
    index: Option<usize>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<StreamToolCallFunction>,
}

#[derive(Deserialize)]
struct StreamToolCallFunction {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Deserialize)]
struct StreamUsage {
    #[serde(default)]
    prompt_tokens: u32,
    #[serde(default)]
    completion_tokens: u32,
    #[serde(default)]
    total_tokens: u32,
    /// OpenAI's own spelling; its caching is automatic, so this is the only
    /// sign it happened. Gateways that do not cache leave it out.
    #[serde(default)]
    prompt_tokens_details: Option<PromptTokensDetails>,
    /// DeepSeek's spelling of the same number.
    #[serde(default)]
    prompt_cache_hit_tokens: Option<u32>,
}

#[derive(Deserialize)]
struct PromptTokensDetails {
    #[serde(default)]
    cached_tokens: Option<u32>,
}

impl From<StreamUsage> for ChatUsage {
    fn from(u: StreamUsage) -> Self {
        Self {
            prompt_tokens: u.prompt_tokens,
            completion_tokens: u.completion_tokens,
            total_tokens: u.total_tokens,
            cached_tokens: u
                .prompt_tokens_details
                .and_then(|d| d.cached_tokens)
                .or(u.prompt_cache_hit_tokens)
                .unwrap_or(0),
        }
    }
}

// -------------------------------------------------------------- sse parsing

#[derive(Debug, PartialEq)]
enum SseLine {
    Chunk {
        delta: Option<String>,
        reasoning: Option<String>,
        /// Which of the two names the reasoning came under — the one it goes
        /// back under.
        reasoning_field: Option<&'static str>,
        usage: Option<ChatUsage>,
        tool_calls: Vec<ParsedToolCallFragment>,
        finish_reason: Option<String>,
    },
    /// The terminal `data: [DONE]`.
    Done,
    /// A blank separator line, or anything else this protocol does not send.
    Ignore,
}

/// The parsed shape of one fragment, kept separate from the wire struct so the
/// accumulator and its tests do not depend on serde internals.
#[derive(Debug, Default, PartialEq, Clone)]
struct ParsedToolCallFragment {
    index: Option<usize>,
    id: Option<String>,
    name: Option<String>,
    arguments: Option<String>,
}

/// Parses one SSE line. Pure and network-free, which is what makes the stream
/// format testable against fixed strings.
fn parse_sse_line(line: &str) -> Result<SseLine, LlmError> {
    // Some servers use CRLF. `lines()` strips only the `\n`, so a stray `\r`
    // otherwise survives into the payload and breaks the `[DONE]` comparison.
    let line = line.trim_end_matches('\r');
    let Some(data) = line.strip_prefix("data:") else {
        return Ok(SseLine::Ignore);
    };
    let data = data.trim();
    if data.is_empty() {
        return Ok(SseLine::Ignore);
    }
    if data == "[DONE]" {
        return Ok(SseLine::Done);
    }

    let chunk: StreamChunk =
        serde_json::from_str(data).map_err(|e| LlmError::Provider(e.to_string()))?;
    let usage = chunk.usage.map(ChatUsage::from);

    let Some(mut choice) = chunk.choices.into_iter().next() else {
        return Ok(SseLine::Chunk {
            delta: None,
            reasoning: None,
            reasoning_field: None,
            usage,
            tool_calls: Vec::new(),
            finish_reason: None,
        });
    };

    let has = |r: &Option<String>| r.as_deref().is_some_and(|r| !r.is_empty());
    let reasoning_field = if has(&choice.delta.reasoning_content) {
        Some(REASONING_CONTENT)
    } else if has(&choice.delta.reasoning) {
        Some(REASONING)
    } else {
        None
    };
    let reasoning = choice.delta.reasoning_text();
    Ok(SseLine::Chunk {
        // A role-only opening chunk usually carries `"content": ""`. Treating
        // that as a real delta opens an empty text block, which hides the
        // typing indicator behind something that looks like an answer.
        delta: choice.delta.content.filter(|s| !s.is_empty()),
        reasoning,
        reasoning_field,
        usage,
        tool_calls: choice
            .delta
            .tool_calls
            .unwrap_or_default()
            .into_iter()
            .map(|f| ParsedToolCallFragment {
                index: f.index,
                id: f.id,
                name: f.function.as_ref().and_then(|fun| fun.name.clone()),
                arguments: f.function.and_then(|fun| fun.arguments),
            })
            .collect(),
        finish_reason: choice.finish_reason.take(),
    })
}

/// Reassembles tool calls from the fragments an SSE stream delivers them in.
#[derive(Debug, Default)]
struct ToolCallAccumulator {
    /// A `Vec`, not a map, so calls come out in the order the model started
    /// them — that order becomes one assistant message's `tool_calls` array.
    entries: Vec<(usize, PartialToolCall)>,
    next_synthetic_index: usize,
}

#[derive(Debug, Default)]
struct PartialToolCall {
    id: String,
    name: String,
    arguments: String,
}

impl ToolCallAccumulator {
    fn ingest(&mut self, fragments: Vec<ParsedToolCallFragment>) {
        for fragment in fragments {
            // A fragment with no index becomes its own entry rather than being
            // guessed into the last one. Merging it wrongly would corrupt an
            // unrelated call's arguments with foreign text; an extra malformed
            // call merely fails to parse later and is reported back to the
            // model as a recoverable error.
            let index = fragment.index.unwrap_or_else(|| {
                let synthetic = 1_000_000 + self.next_synthetic_index;
                self.next_synthetic_index += 1;
                synthetic
            });
            let entry = match self.entries.iter().position(|(i, _)| *i == index) {
                Some(position) => &mut self.entries[position].1,
                None => {
                    self.entries.push((index, PartialToolCall::default()));
                    &mut self.entries.last_mut().expect("just pushed").1
                }
            };
            if let Some(id) = fragment.id {
                entry.id = id;
            }
            if let Some(name) = fragment.name {
                entry.name = name;
            }
            if let Some(arguments) = fragment.arguments {
                entry.arguments.push_str(&arguments);
            }
        }
    }

    /// Live snapshots of every call with anything in it yet.
    ///
    /// A call with no id yet is reported as `pending:{index}`: several proxies
    /// withhold the id until the final fragment, and skipping those makes a
    /// long write look like a hung stream.
    fn snapshots(&self) -> impl Iterator<Item = (String, &str, &str)> {
        self.entries.iter().filter_map(|(index, entry)| {
            if entry.id.is_empty() && entry.name.is_empty() && entry.arguments.is_empty() {
                return None;
            }
            let id = if entry.id.is_empty() {
                format!("pending:{index}")
            } else {
                entry.id.clone()
            };
            Some((id, entry.name.as_str(), entry.arguments.as_str()))
        })
    }

    fn finish(self) -> Vec<LlmToolCall> {
        self.entries
            .into_iter()
            .map(|(_, e)| LlmToolCall {
                id: e.id,
                name: e.name,
                arguments: e.arguments,
            })
            .collect()
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::domain::llm::LlmToolDefinition;
    use std::io::Write;
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    fn chunk(json: &str) -> SseLine {
        parse_sse_line(&format!("data: {json}")).expect("parses")
    }

    fn fragments(line: SseLine) -> Vec<ParsedToolCallFragment> {
        match line {
            SseLine::Chunk { tool_calls, .. } => tool_calls,
            other => panic!("expected a chunk, got {other:?}"),
        }
    }

    fn delta_of(line: SseLine) -> Option<String> {
        match line {
            SseLine::Chunk { delta, .. } => delta,
            other => panic!("expected a chunk, got {other:?}"),
        }
    }

    #[test]
    fn a_text_chunk_yields_its_delta() {
        let line = chunk(r#"{"choices":[{"delta":{"content":"hello"}}]}"#);
        assert_eq!(delta_of(line), Some("hello".into()));
    }

    /// The opening chunk of most streams is role-only with `content: ""`.
    /// Treating that as a real delta opens an empty answer block, which hides
    /// the typing indicator behind something that looks like a reply.
    #[test]
    fn an_empty_content_chunk_is_not_a_delta() {
        let line = chunk(r#"{"choices":[{"delta":{"role":"assistant","content":""}}]}"#);
        assert_eq!(delta_of(line), None);
    }

    #[test]
    fn the_terminal_line_is_recognised() {
        assert_eq!(parse_sse_line("data: [DONE]").unwrap(), SseLine::Done);
    }

    /// `lines()` strips the `\n` and leaves the `\r`, so a CRLF server would
    /// otherwise never match `[DONE]` and the stream would run to EOF.
    #[test]
    fn carriage_returns_do_not_hide_the_terminal_line() {
        assert_eq!(parse_sse_line("data: [DONE]\r").unwrap(), SseLine::Done);
    }

    #[test]
    fn non_data_lines_are_ignored() {
        for line in ["", ": keep-alive", "event: message", "data:", "data:   "] {
            assert_eq!(parse_sse_line(line).unwrap(), SseLine::Ignore, "{line:?}");
        }
    }

    #[test]
    fn both_spellings_of_reasoning_are_accepted() {
        for field in ["reasoning_content", "reasoning"] {
            let line = chunk(&format!(r#"{{"choices":[{{"delta":{{"{field}":"thinking"}}}}]}}"#));
            match line {
                SseLine::Chunk { reasoning, .. } => {
                    assert_eq!(reasoning, Some("thinking".into()), "{field}")
                }
                other => panic!("{other:?}"),
            }
        }
    }

    /// DeepSeek reports its cache hits under its own name; without reading
    /// it, a DeepSeek turn looks as if nothing was ever cached.
    #[test]
    fn deepseeks_cache_hit_tokens_are_read_too() {
        let line = chunk(r#"{"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15,"prompt_cache_hit_tokens":7,"prompt_cache_miss_tokens":3}}"#);
        match line {
            SseLine::Chunk { usage: Some(u), .. } => assert_eq!(u.cached_tokens, 7),
            other => panic!("{other:?}"),
        }
    }

    fn stream_reasoning(field: &str) -> ChatStreamResult {
        let (url, server) = serve(
            "200 OK",
            sse(&[
                &format!(r#"{{"choices":[{{"delta":{{"{field}":"let me "}}}}]}}"#),
                &format!(r#"{{"choices":[{{"delta":{{"{field}":"look"}}}}]}}"#),
                r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"readFile","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#,
            ]),
        );
        let result = provider(url)
            .chat_stream(
                ChatRequest { messages: vec![], tools: vec![], model: "m".into() },
                &|_| {},
                &|_| {},
                &|_, _, _| {},
                &|| false,
            )
            .expect("streams");
        server.join().ok();
        result
    }

    /// Reasoning is kept to be sent back, under the name it came in:
    /// DeepSeek's `reasoning_content`, OpenRouter's `reasoning`.
    #[test]
    fn reasoning_is_kept_under_the_name_it_came_in() {
        for field in ["reasoning_content", "reasoning"] {
            let kept = stream_reasoning(field);
            assert_eq!(kept.reasoning, "let me look");
            assert_eq!(kept.native_content, Some(serde_json::json!({ field: "let me look" })), "{field}");
        }
    }

    /// Back on the assistant message it came with — and only from our own
    /// shape: Anthropic's blocks are an array and mean nothing here.
    #[test]
    fn kept_reasoning_goes_back_on_its_assistant_message() {
        let with = |native: Option<serde_json::Value>| {
            let mut message = LlmMessage::assistant("done");
            message.native_content = native;
            let request = ChatRequest { messages: vec![message], tools: vec![], model: "m".into() };
            serde_json::to_value(provider("http://x".into()).body(&request, false)).unwrap()["messages"][0].clone()
        };
        let deepseek = with(Some(serde_json::json!({"reasoning_content": "why"})));
        assert_eq!(deepseek["reasoning_content"], "why");
        assert!(deepseek.get("reasoning").is_none(), "one name, the one it came in");
        let openrouter = with(Some(serde_json::json!({"reasoning": "why"})));
        assert_eq!(openrouter["reasoning"], "why");
        assert!(openrouter.get("reasoning_content").is_none());
        let anthropic = with(Some(serde_json::json!([{"type": "thinking", "reasoning": "x"}])));
        assert!(anthropic.get("reasoning_content").is_none() && anthropic.get("reasoning").is_none());
        let none = with(None);
        assert!(none.get("reasoning_content").is_none() && none.get("reasoning").is_none());
    }

    /// Several servers send `null` rather than omitting the field.
    #[test]
    fn null_fields_are_not_a_parse_failure() {
        let line = chunk(r#"{"choices":[{"delta":{"content":null,"tool_calls":null}}],"usage":null}"#);
        assert_eq!(delta_of(line), None);
    }

    #[test]
    fn a_usage_only_chunk_carries_the_totals() {
        let line = chunk(r#"{"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#);
        match line {
            SseLine::Chunk { usage: Some(u), .. } => assert_eq!(u.total_tokens, 15),
            other => panic!("{other:?}"),
        }
    }

    /// OpenAI caches on its own; this field is the only sign it did.
    #[test]
    fn cached_tokens_are_read_when_the_provider_reports_them() {
        let usage = |json: &str| match chunk(json) {
            SseLine::Chunk { usage: Some(u), .. } => u,
            other => panic!("{other:?}"),
        };
        let cached = usage(r#"{"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15,"prompt_tokens_details":{"cached_tokens":8}}}"#);
        assert_eq!(cached.cached_tokens, 8);
        let silent = usage(r#"{"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15,"prompt_tokens_details":null}}"#);
        assert_eq!(silent.cached_tokens, 0);
    }

    #[test]
    fn malformed_chunk_json_is_a_provider_error() {
        assert!(matches!(
            parse_sse_line("data: {not json"),
            Err(LlmError::Provider(_))
        ));
    }

    /// The shape the accumulator exists for: id and name in one fragment, the
    /// arguments spread across every fragment after it.
    #[test]
    fn a_call_is_reassembled_from_its_fragments() {
        let mut acc = ToolCallAccumulator::default();
        acc.ingest(fragments(chunk(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"readFile","arguments":""}}]}}]}"#,
        )));
        acc.ingest(fragments(chunk(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"path\":"}}]}}]}"#,
        )));
        acc.ingest(fragments(chunk(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"a.txt\"}"}}]}}]}"#,
        )));

        let calls = acc.finish();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].name, "readFile");
        assert_eq!(calls[0].arguments, r#"{"path":"a.txt"}"#);
    }

    /// Two calls in one round must not braid together, and must come out in
    /// the order the model started them.
    #[test]
    fn parallel_calls_stay_separate_and_ordered() {
        let mut acc = ToolCallAccumulator::default();
        acc.ingest(vec![
            ParsedToolCallFragment { index: Some(0), id: Some("a".into()), name: Some("readFile".into()), arguments: Some("{\"p".into()) },
            ParsedToolCallFragment { index: Some(1), id: Some("b".into()), name: Some("grep".into()), arguments: Some("{\"q".into()) },
        ]);
        acc.ingest(vec![
            ParsedToolCallFragment { index: Some(1), arguments: Some("\":1}".into()), ..Default::default() },
            ParsedToolCallFragment { index: Some(0), arguments: Some("\":2}".into()), ..Default::default() },
        ]);

        let calls = acc.finish();
        assert_eq!(
            calls.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(calls[0].arguments, r#"{"p":2}"#);
        assert_eq!(calls[1].arguments, r#"{"q":1}"#);
    }

    /// Guessing an index-less fragment into the last entry would corrupt an
    /// unrelated call's arguments with foreign text. An extra malformed call
    /// merely fails to parse later and is reported back as recoverable.
    #[test]
    fn an_index_less_fragment_does_not_contaminate_another_call() {
        let mut acc = ToolCallAccumulator::default();
        acc.ingest(vec![ParsedToolCallFragment {
            index: Some(0),
            id: Some("a".into()),
            name: Some("readFile".into()),
            arguments: Some("{}".into()),
        }]);
        acc.ingest(vec![ParsedToolCallFragment {
            index: None,
            arguments: Some("stray".into()),
            ..Default::default()
        }]);

        let calls = acc.finish();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].arguments, "{}", "the real call is untouched");
    }

    /// Several proxies withhold the id until the final fragment. Skipping
    /// those snapshots makes a long write look like a hung stream.
    #[test]
    fn a_call_without_an_id_yet_still_reports_progress() {
        let mut acc = ToolCallAccumulator::default();
        acc.ingest(vec![ParsedToolCallFragment {
            index: Some(0),
            name: Some("writeFile".into()),
            arguments: Some("{\"pa".into()),
            ..Default::default()
        }]);

        let snapshots: Vec<(String, String)> = acc
            .snapshots()
            .map(|(id, _, args)| (id, args.to_string()))
            .collect();
        assert_eq!(snapshots, [("pending:0".to_string(), "{\"pa".to_string())]);
    }

    #[test]
    fn an_entirely_empty_entry_reports_nothing() {
        let mut acc = ToolCallAccumulator::default();
        acc.ingest(vec![ParsedToolCallFragment {
            index: Some(0),
            ..Default::default()
        }]);
        assert_eq!(acc.snapshots().count(), 0);
    }

    // ------------------------------------------------------------ live socket

    /// Serves one canned response over a real socket, so the streaming loop is
    /// exercised end to end rather than only its line parser.
    pub(in crate::infra::llm_providers) fn serve(status: &str, body: String) -> (String, std::thread::JoinHandle<()>) {
        serve_with_headers(status, "", body)
    }

    /// `extra` is appended verbatim, each line CRLF-terminated by the caller.
    pub(in crate::infra::llm_providers) fn serve_with_headers(
        status: &str,
        extra: &str,
        body: String,
    ) -> (String, std::thread::JoinHandle<()>) {
        let status = status.to_string();
        let extra = extra.to_string();
        let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
        let port = listener.local_addr().expect("addr").port();
        let handle = std::thread::spawn(move || {
            let Ok((mut socket, _)) = listener.accept() else {
                return;
            };
            // Read just enough of the request that the client is not writing
            // into a closed socket.
            let mut buffer = [0u8; 4096];
            let _ = std::io::Read::read(&mut socket, &mut buffer);
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(response.as_bytes());
            let _ = socket.flush();
        });
        (format!("http://127.0.0.1:{port}"), handle)
    }

    /// Like `serve`, and hands back the request it read — head and body.
    pub(in crate::infra::llm_providers) fn serve_capturing(
        body: String,
    ) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
        let port = listener.local_addr().expect("addr").port();
        let handle = std::thread::spawn(move || {
            let Ok((mut socket, _)) = listener.accept() else {
                return String::new();
            };
            let mut request = Vec::new();
            let mut buffer = [0u8; 65536];
            // Until the body promised by Content-Length is in.
            while let Ok(n @ 1..) = std::io::Read::read(&mut socket, &mut buffer) {
                request.extend_from_slice(&buffer[..n]);
                let text = String::from_utf8_lossy(&request);
                if let Some(end) = text.find("\r\n\r\n") {
                    let length = text[..end]
                        .lines()
                        .find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().to_string()))
                        .and_then(|v| v.parse::<usize>().ok())
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(response.as_bytes());
            let _ = socket.flush();
            String::from_utf8_lossy(&request).into_owned()
        });
        (format!("http://127.0.0.1:{port}"), handle)
    }

    fn provider(base_url: String) -> OpenAiCompatibleProvider {
        OpenAiCompatibleProvider::new(
            crate::infra::http_agent::build_agent(None).expect("agent"),
            base_url,
            SecretString::from("sk-test"),
            HashMap::new(),
            None,
            None,
            None,
            None,
            None,
        )
    }

    fn sse(lines: &[&str]) -> String {
        lines
            .iter()
            .map(|l| format!("data: {l}\n\n"))
            .collect::<String>()
            + "data: [DONE]\n\n"
    }

    #[test]
    fn a_stream_is_collected_into_text_and_calls() {
        let (url, server) = serve(
            "200 OK",
            sse(&[
                r#"{"choices":[{"delta":{"role":"assistant","content":""}}]}"#,
                r#"{"choices":[{"delta":{"content":"Look"}}]}"#,
                r#"{"choices":[{"delta":{"content":"ing."}}]}"#,
                r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"readFile","arguments":"{}"}}]}}]}"#,
                r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":3,"completion_tokens":4,"total_tokens":7}}"#,
            ]),
        );
        let deltas = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&deltas);

        let result = provider(url)
            .chat_stream(
                ChatRequest {
                    messages: vec![LlmMessage::user("hi")],
                    tools: Vec::new(),
                    model: "m".into(),
                },
                &move |d| seen.lock().unwrap().push(d.to_string()),
                &|_| {},
                &|_, _, _| {},
                &|| false,
            )
            .expect("streams");
        server.join().ok();

        assert_eq!(result.text, "Looking.");
        assert_eq!(*deltas.lock().unwrap(), ["Look", "ing."], "the empty opener was not a delta");
        assert_eq!(result.tool_calls.len(), 1);
        assert_eq!(result.usage.expect("usage").total_tokens, 7);
        assert!(!result.truncated);
    }

    /// The model is never told its response budget, so a cut-off round is
    /// indistinguishable from a finished one unless this is carried.
    #[test]
    fn a_length_finish_is_latched_through_a_later_chunk() {
        let (url, server) = serve(
            "200 OK",
            sse(&[
                r#"{"choices":[{"delta":{"content":"half a sen"}}]}"#,
                r#"{"choices":[{"delta":{},"finish_reason":"length"}]}"#,
                r#"{"choices":[],"usage":{"prompt_tokens":1,"completion_tokens":2,"total_tokens":3}}"#,
            ]),
        );

        let result = provider(url)
            .chat_stream(
                ChatRequest { messages: vec![], tools: vec![], model: "m".into() },
                &|_| {},
                &|_| {},
                &|_, _, _| {},
                &|| false,
            )
            .expect("streams");
        server.join().ok();

        assert!(result.truncated, "a usage-only chunk erased the finish reason");
    }

    /// A stop lands within roughly one chunk, and whatever had accumulated
    /// comes back as an ordinary result rather than an error.
    #[test]
    fn cancelling_returns_what_had_arrived_so_far() {
        let (url, server) = serve(
            "200 OK",
            sse(&[
                r#"{"choices":[{"delta":{"content":"first"}}]}"#,
                r#"{"choices":[{"delta":{"content":"second"}}]}"#,
                r#"{"choices":[{"delta":{"content":"third"}}]}"#,
            ]),
        );
        let stop = AtomicBool::new(false);

        let result = provider(url)
            .chat_stream(
                ChatRequest { messages: vec![], tools: vec![], model: "m".into() },
                &|_| stop.store(true, Ordering::SeqCst),
                &|_| {},
                &|_, _, _| {},
                &|| stop.load(Ordering::SeqCst),
            )
            .expect("cancelling is not an error");
        server.join().ok();

        assert_eq!(result.text, "first", "stopped after the first chunk");
    }

    /// The whole reason the agent disables ureq's status-to-error conversion:
    /// the provider's explanation is in the body.
    #[test]
    fn a_rejected_request_carries_the_providers_explanation() {
        let (url, server) = serve(
            "400 Bad Request",
            r#"{"error":{"message":"model \"m\" is not available to this key"}}"#.to_string(),
        );

        let err = provider(url)
            .chat_stream(
                ChatRequest { messages: vec![], tools: vec![], model: "m".into() },
                &|_| {},
                &|_| {},
                &|_, _, _| {},
                &|| false,
            )
            .expect_err("400");
        server.join().ok();

        let message = err.to_string();
        assert!(message.contains("400"), "{message}");
        assert!(message.contains("not available to this key"), "{message}");
    }

    /// A refusal the turn loop may act on, rather than one more opaque
    /// status: 429 is the only failure worth sending the same request again
    /// for, and the server's own hint is the only reliable idea of when.
    #[test]
    fn a_rate_limited_request_carries_the_servers_retry_hint() {
        let (url, server) = serve_with_headers(
            "429 Too Many Requests",
            "Retry-After: 30\r\n",
            r#"{"error":{"message":"rate limit reached"}}"#.to_string(),
        );

        let err = provider(url)
            .chat_stream(
                ChatRequest { messages: vec![], tools: vec![], model: "m".into() },
                &|_| {},
                &|_| {},
                &|_, _, _| {},
                &|| false,
            )
            .expect_err("429");
        server.join().ok();

        let LlmError::RateLimited { retry_after_seconds, message } = err else {
            panic!("expected a rate limit, got {err:?}");
        };
        assert_eq!(retry_after_seconds, Some(30));
        assert!(message.contains("rate limit reached"), "{message}");
    }

    /// The header's other permitted form, which no gateway uses and which is
    /// deliberately not parsed. It must read as "no hint" — not as zero, which
    /// would send the retry straight back into the same refusal.
    #[test]
    fn a_hint_in_a_form_we_do_not_read_is_no_hint() {
        let (url, server) = serve_with_headers(
            "429 Too Many Requests",
            "Retry-After: Wed, 21 Oct 2015 07:28:00 GMT\r\n",
            "{}".to_string(),
        );

        let err = provider(url).list_models().expect_err("429");
        server.join().ok();

        assert!(
            matches!(err, LlmError::RateLimited { retry_after_seconds: None, .. }),
            "{err:?}"
        );
    }

    /// Plenty of gateways send no hint at all. That is not an error — the
    /// backoff covers it — but it must not be mistaken for a hint of zero.
    #[test]
    fn a_rate_limit_without_a_hint_has_none() {
        let (url, server) = serve("429 Too Many Requests", "{}".to_string());

        let err = provider(url).list_models().expect_err("429");
        server.join().ok();

        assert!(
            matches!(err, LlmError::RateLimited { retry_after_seconds: None, .. }),
            "{err:?}"
        );
    }

    /// 5xx is the provider's own failure and retried; 501 is a refusal of
    /// the request itself and stays an ordinary error.
    #[test]
    fn a_server_error_is_unavailable_but_not_implemented_is_not() {
        for status in ["500 Internal Server Error", "502 Bad Gateway", "503 Service Unavailable", "504 Gateway Timeout"] {
            let (url, server) = serve_with_headers(status, "Retry-After: 5\r\n", "{}".to_string());
            let err = provider(url).list_models().expect_err(status);
            server.join().ok();
            assert!(matches!(err, LlmError::Unavailable { retry_after_seconds: Some(5), .. }), "{status}: {err:?}");
        }
        let (url, server) = serve("501 Not Implemented", "{}".to_string());
        let err = provider(url).list_models().expect_err("501");
        server.join().ok();
        assert!(matches!(err, LlmError::Http(_)), "{err:?}");
    }

    #[test]
    fn models_are_listed() {
        let (url, server) = serve("200 OK", r#"{"data":[{"id":"m1"},{"id":"m2"}]}"#.to_string());
        let models = provider(url).list_models().expect("lists");
        server.join().ok();
        assert_eq!(
            models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["m1", "m2"]
        );
    }

    /// Sampling goes out only when set: some reasoning models refuse any
    /// temperature but their own.
    #[test]
    fn sampling_is_sent_only_when_set() {
        let request = ChatRequest { messages: vec![], tools: vec![], model: "m".into() };
        let unset = serde_json::to_value(provider("http://unused".into()).body(&request, true)).unwrap();
        assert!(unset.get("temperature").is_none() && unset.get("top_p").is_none());

        let mut p = provider("http://unused".into());
        (p.temperature, p.top_p) = (Some(0.25), Some(0.5));
        let set = serde_json::to_value(p.body(&request, true)).unwrap();
        assert_eq!((set["temperature"].as_f64(), set["top_p"].as_f64()), (Some(0.25), Some(0.5)));
    }

    /// An empty `tools` array with `tool_choice: "auto"` is pointless, and some
    /// OpenAI-compatible servers reject the combination outright.
    #[test]
    fn tool_choice_is_only_sent_when_tools_are() {
        let p = provider("http://unused".into());
        let without = serde_json::to_value(p.body(
            &ChatRequest { messages: vec![], tools: vec![], model: "m".into() },
            true,
        ))
        .unwrap();
        assert!(without.get("tool_choice").is_none());
        assert!(without.get("tools").is_none());

        let with = serde_json::to_value(p.body(
            &ChatRequest {
                messages: vec![],
                tools: vec![LlmToolDefinition {
                    name: "readFile".into(),
                    description: "d".into(),
                    parameters: serde_json::json!({}),
                }],
                model: "m".into(),
            },
            true,
        ))
        .unwrap();
        assert_eq!(with["tool_choice"], "auto");
    }

    /// A gateway that wants a fresh correlation id per request must get a
    /// different one each time, not the literal placeholder.
    #[test]
    fn the_uuid_placeholder_becomes_a_fresh_value() {
        let first = header_value(REQUEST_HEADER_VALUE_UUID);
        let second = header_value(REQUEST_HEADER_VALUE_UUID);
        assert_ne!(first, second);
        assert_ne!(first, REQUEST_HEADER_VALUE_UUID);
        assert_eq!(header_value("static"), "static");
    }
}
