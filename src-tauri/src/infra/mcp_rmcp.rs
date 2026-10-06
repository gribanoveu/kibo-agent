//! `domain::mcp::McpClient` over `rmcp`, the protocol's official Rust SDK.
//!
//! The protocol is the SDK's: both eras of it (a server of 2026-07-28 is asked
//! `server/discover` and spoken to without a session; an older one gets the
//! `initialize` handshake), request ids, the server's own requests. What is
//! kept here is what the app decided for itself and the SDK does not know —
//! `docs/22-rmcp-migration.md`, "Что нельзя потерять":
//!
//! - a call waits no longer than the server's own timeout and sees the
//!   turn's Stop within [`POLL`]; either way the server is told;
//! - a tool list with an entry the schema refuses still yields the others;
//! - what a failure means is the transport's to say ([`Failure`]): for a
//!   process, its exit code and the last lines of its stderr;
//! - a tool list is read again only between turns, when the server said it
//!   changed or the time it gave the list ran out ([`McpClient::tools_stale`]);
//! - a server's question in the middle of a call reaches the user through the
//!   caller, and the call's clock stops while the user answers — however the
//!   question came: as a request of the server's own (before 2026-07-28) or
//!   as an answer asking for input, the call then sent again (`docs/23-mcp-extension.md`, M-8).
//!
//! `rmcp` is async on tokio and everything above `McpClient` is blocking, so
//! each method is one `block_on` on Tauri's runtime — the same bridge as
//! `infra::kube_client`. Never call these from a task of that runtime.

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rmcp::model::{
    CallToolRequest, CallToolRequestParams, ClientConfig, ClientRequest, ElicitRequestParams, ElicitResult,
    GetPromptRequest, GetPromptRequestParams, Implementation, ListPromptsRequest, ListToolsRequest,
    PaginatedRequestParams, ProtocolVersion,
};
use rmcp::service::{
    ClientInitializeError, ClientLifecycleMode, ClientServiceExt, NotificationContext, Peer, PeerRequestOptions,
    RequestContext, RoleClient, RunningService, ServiceError,
};
use rmcp::transport::IntoTransport;
use rmcp::ClientHandler;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};

use crate::domain::mcp::{
    prompt, question, render_content, render_prompt, McpAnswer, McpCallResult, McpClient, McpError, McpPrompt,
    McpQuestion, McpTool, McpToolHints,
};
use crate::sync::lock;

/// How often a waiting call looks at its stop flag.
const POLL: Duration = Duration::from_millis(50);
/// Telling the server a call was abandoned is a courtesy; a transport that
/// cannot take the message does not hold the caller.
const CANCEL_GRACE: Duration = Duration::from_secs(1);
/// A server that pages its tool list forever is broken, not large.
const MAX_TOOL_PAGES: usize = 50;
/// A server that asks for input round after round is broken, not curious.
const MAX_INPUT_ROUNDS: usize = 10;

/// What went wrong below the protocol, for the transport to put into words.
pub enum Failure<'a> {
    /// The conversation is over.
    Closed,
    /// One message could not be sent; the text is the transport's own.
    Send(&'a str),
}

pub type Describe = Box<dyn Fn(Failure<'_>) -> McpError + Send + Sync>;
/// Why the conversation is over, if what lies under the transport says so —
/// a process that exited, a session that ended. Asked before every request,
/// every [`POLL`] while the handshake or a request waits,
/// and by `is_alive`; dropped with the client, and with it whatever it holds.
pub type Ended = Box<dyn Fn() -> Option<McpError> + Send + Sync>;

/// A question the server sent on its own, and where its answer goes.
type Asked = (McpQuestion, oneshot::Sender<McpAnswer>);

pub struct RmcpClient {
    /// Held for its end: dropping it closes the conversation — for an HTTP
    /// session, with the `DELETE` that ends it on the server.
    _service: RunningService<RoleClient, Listener>,
    peer: Peer<RoleClient>,
    timeout: Duration,
    describe: Describe,
    ended: Ended,
    /// The server said its tools or prompts changed since they were read.
    lists_changed: Arc<AtomicBool>,
    /// Until when the server said the list it gave may be kept; `None` when
    /// it gave no time, which is every server before 2026-07-28.
    tools_fresh_until: Mutex<Option<Instant>>,
    /// Where the server's own questions go while a call can carry them.
    desk: Desk,
}

/// The call that takes the server's questions now, if one does.
type Desk = Arc<Mutex<Option<mpsc::UnboundedSender<Asked>>>>;

/// This side of the conversation: who the client is, an ear for the lists
/// changing, and the way to the user for a question.
struct Listener {
    config: ClientConfig,
    lists_changed: Arc<AtomicBool>,
    desk: Desk,
}

impl ClientHandler for Listener {
    fn get_info(&self) -> ClientConfig {
        self.config.clone()
    }

    async fn on_tool_list_changed(&self, _context: NotificationContext<RoleClient>) {
        self.lists_changed.store(true, Ordering::SeqCst);
    }

    async fn on_prompt_list_changed(&self, _context: NotificationContext<RoleClient>) {
        self.lists_changed.store(true, Ordering::SeqCst);
    }

    /// A question outside a call that can carry it — or one this window
    /// cannot put — is declined: nobody would see it.
    async fn create_elicitation(
        &self,
        request: ElicitRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> Result<ElicitResult, rmcp::ErrorData> {
        let asked = serde_json::to_value(&request).ok().as_ref().and_then(question);
        let desk = lock(&self.desk).clone();
        let answer = match (asked.clone(), desk) {
            (Some(asked), Some(desk)) => {
                let (reply, answered) = oneshot::channel();
                match desk.send((asked, reply)) {
                    // The call ended before anyone asked: nobody declined it louder.
                    Ok(()) => answered.await.unwrap_or(McpAnswer::Decline),
                    Err(_) => McpAnswer::Decline,
                }
            }
            _ => McpAnswer::Decline,
        };
        let result = match &asked {
            Some(asked) => answer.to_result(asked),
            None => json!({ "action": "decline" }),
        };
        serde_json::from_value(result).map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))
    }
}

enum Stop {
    Cancelled,
    TimedOut,
    /// What lies under the transport is gone, though the stream may not say so.
    Ended(McpError),
}

impl RmcpClient {
    /// Opens the conversation over what `transport` returns, within `timeout`.
    /// The transport is built inside the runtime: tokio's pipes and sockets
    /// register with it as they are made.
    pub fn connect<T, E, A>(
        transport: impl FnOnce() -> Result<T, McpError>,
        timeout: Duration,
        cancelled: &dyn Fn() -> bool,
        describe: Describe,
        ended: Ended,
    ) -> Result<Self, McpError>
    where
        T: IntoTransport<RoleClient, E, A>,
        E: std::error::Error + Send + Sync + 'static,
    {
        // Questions, both kinds: a form, and an address to open. Nothing
        // else is promised — no sampling, no roots.
        let capabilities = serde_json::from_value(json!({ "elicitation": { "form": {}, "url": {} } }))
            .map_err(|e| McpError::NotStarted(e.to_string()))?;
        let config = ClientConfig::new(capabilities, Implementation::new("kibo-agent", env!("CARGO_PKG_VERSION")));
        let lifecycle = ClientLifecycleMode::Auto {
            preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            legacy_version: Some(ProtocolVersion::V_2025_11_25),
        };
        let lists_changed = Arc::new(AtomicBool::new(false));
        let desk = Desk::default();
        let listener = Listener { config, lists_changed: Arc::clone(&lists_changed), desk: Arc::clone(&desk) };
        let service = tauri::async_runtime::block_on(async {
            let mut serving = Box::pin(listener.serve_with_lifecycle(transport()?, lifecycle));
            match wait(&mut serving, timeout, cancelled, &ended, None).await {
                Ok(served) => served.map_err(|e| handshake_error(e, &describe)),
                Err(Stop::Cancelled) => Err(McpError::Cancelled),
                Err(Stop::TimedOut) => Err(McpError::Timeout(timeout.as_secs())),
                Err(Stop::Ended(error)) => Err(error),
            }
        })?;
        Ok(Self {
            peer: service.peer().clone(),
            _service: service,
            timeout,
            describe,
            ended,
            lists_changed,
            tools_fresh_until: Mutex::default(),
            desk,
        })
    }

    /// One request and its answer as JSON. Through `Value` rather than the
    /// SDK's types: an answer its schema refuses arrives as a custom result,
    /// and what can be read from it still is.
    ///
    /// With `asking`, a question the server sends meanwhile is put to the
    /// user, and the time the user takes is not the server's.
    fn request(
        &self,
        request: ClientRequest,
        cancelled: &dyn Fn() -> bool,
        asking: Option<&mut Asking<'_>>,
    ) -> Result<Value, McpError> {
        if let Some(ended) = (self.ended)() {
            return Err(ended);
        }
        tauri::async_runtime::block_on(async {
            let mut handle = self
                .peer
                .send_cancellable_request(request, PeerRequestOptions::no_options())
                .await
                .map_err(|e| self.error(e))?;
            let stop = match wait(&mut handle.rx, self.timeout, cancelled, &self.ended, asking).await {
                Ok(Ok(Ok(result))) => return serde_json::to_value(result).map_err(|e| McpError::Protocol(e.to_string())),
                Ok(Ok(Err(error))) => return Err(self.error(error)),
                Ok(Err(_)) => return Err((self.describe)(Failure::Closed)),
                Err(stop) => stop,
            };
            // Whatever the server answers later is dropped by the SDK: nobody
            // is waiting for it.
            let (reason, error) = match stop {
                Stop::Cancelled => ("cancelled by the user", McpError::Cancelled),
                Stop::TimedOut => ("timed out", McpError::Timeout(self.timeout.as_secs())),
                // Nobody is left to tell.
                Stop::Ended(error) => return Err(error),
            };
            let _ = tokio::time::timeout(CANCEL_GRACE, handle.cancel(Some(reason.into()))).await;
            Err(error)
        })
    }

    fn error(&self, error: ServiceError) -> McpError {
        match error {
            ServiceError::McpError(data) => {
                McpError::Server { code: i64::from(data.code.0), message: data.message.into_owned() }
            }
            ServiceError::TransportClosed => (self.describe)(Failure::Closed),
            ServiceError::TransportSend(error) => (self.describe)(Failure::Send(&error.error.to_string())),
            ServiceError::Cancelled { .. } => McpError::Cancelled,
            ServiceError::Timeout { timeout } => McpError::Timeout(timeout.as_secs()),
            other => McpError::Protocol(other.to_string()),
        }
    }

    fn offers_prompts(&self) -> bool {
        self.peer.peer_info().is_some_and(|info| info.capabilities.prompts.is_some())
    }
}

/// A call's way to the user for the server's own questions.
struct Asking<'a> {
    questions: mpsc::UnboundedReceiver<Asked>,
    ask: &'a dyn Fn(&McpQuestion) -> McpAnswer,
}

impl McpClient for RmcpClient {
    fn list_tools(&self) -> Result<Vec<McpTool>, McpError> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        // Before asking, so a change announced while the answer is on its
        // way is not lost to this reading.
        self.lists_changed.store(false, Ordering::SeqCst);
        let asked = Instant::now();
        for page_number in 0..MAX_TOOL_PAGES {
            let params = PaginatedRequestParams::default().with_cursor(cursor.take());
            let request = ClientRequest::ListToolsRequest(ListToolsRequest::with_param(params));
            let page = self.request(request, &|| false, None)?;
            if page_number == 0 {
                let fresh_for = page["ttlMs"].as_u64().map(Duration::from_millis);
                *lock(&self.tools_fresh_until) = fresh_for.map(|ttl| asked + ttl);
            }
            let listed = page["tools"]
                .as_array()
                .ok_or_else(|| McpError::Protocol(format!("tools/list returned no tools array: {page}")))?;
            tools.extend(listed.iter().filter_map(tool));
            match page["nextCursor"].as_str() {
                Some(next) if !next.is_empty() => cursor = Some(next.to_string()),
                _ => return Ok(tools),
            }
        }
        Err(McpError::Protocol(format!("tools/list kept paging past {MAX_TOOL_PAGES} pages")))
    }

    fn call_tool(&self, name: &str, arguments: Value, cancelled: &dyn Fn() -> bool) -> Result<McpCallResult, McpError> {
        self.call_tool_asking(name, arguments, cancelled, &|_| McpAnswer::Decline)
    }

    fn call_tool_asking(
        &self,
        name: &str,
        arguments: Value,
        cancelled: &dyn Fn() -> bool,
        ask: &dyn Fn(&McpQuestion) -> McpAnswer,
    ) -> Result<McpCallResult, McpError> {
        let mut params = CallToolRequestParams::new(name.to_string());
        if let Value::Object(arguments) = arguments {
            params = params.with_arguments(arguments);
        }
        // This call takes the server's own questions until it ends. Two
        // calls to one server at once: the later one takes them.
        let (questions, taken) = mpsc::unbounded_channel();
        *lock(&self.desk) = Some(questions.clone());
        let mut asking = Asking { questions: taken, ask };
        let result = (|| {
            for _ in 0..MAX_INPUT_ROUNDS {
                let request = ClientRequest::CallToolRequest(CallToolRequest::new(params.clone()));
                let result = self.request(request, cancelled, Some(&mut asking))?;
                if result["resultType"] != "input_required" {
                    return Ok(McpCallResult { text: render_content(&result), is_error: result["isError"].as_bool().unwrap_or(false) });
                }
                // The server asked for input instead of answering: put its
                // questions, then send the call again with the answers.
                let mut answers = BTreeMap::new();
                for (key, asked) in result["inputRequests"].as_object().into_iter().flatten() {
                    if asked["method"] != "elicitation/create" {
                        return Err(McpError::Protocol(format!("{name} asked for {}, which this app does not offer", asked["method"])));
                    }
                    let answer = match question(&asked["params"]) {
                        Some(asked) => ask(&asked).to_result(&asked),
                        None => json!({ "action": "decline" }),
                    };
                    if cancelled() {
                        return Err(McpError::Cancelled);
                    }
                    answers.insert(key.clone(), answer);
                }
                params.input_responses = Some(answers);
                params.request_state = result["requestState"].as_str().map(str::to_string);
            }
            Err(McpError::Protocol(format!("{name} kept asking for input past {MAX_INPUT_ROUNDS} rounds")))
        })();
        let mut desk = lock(&self.desk);
        if desk.as_ref().is_some_and(|taking| taking.same_channel(&questions)) {
            *desk = None;
        }
        result
    }

    fn is_alive(&self) -> bool {
        !self.peer.is_transport_closed() && (self.ended)().is_none()
    }

    fn instructions(&self) -> Option<String> {
        self.peer.peer_info().and_then(|info| info.instructions.clone())
    }

    fn tools_stale(&self) -> bool {
        let expired = lock(&self.tools_fresh_until).is_some_and(|until| Instant::now() >= until);
        expired || self.lists_changed.load(Ordering::SeqCst)
    }

    /// Asked only of a server that said it has prompts: one that did not may
    /// not know the method at all.
    fn list_prompts(&self) -> Result<Vec<McpPrompt>, McpError> {
        if !self.offers_prompts() {
            return Ok(Vec::new());
        }
        let mut prompts = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_TOOL_PAGES {
            let params = PaginatedRequestParams::default().with_cursor(cursor.take());
            let page = self.request(ClientRequest::ListPromptsRequest(ListPromptsRequest::with_param(params)), &|| false, None)?;
            let listed = page["prompts"]
                .as_array()
                .ok_or_else(|| McpError::Protocol(format!("prompts/list returned no prompts array: {page}")))?;
            prompts.extend(listed.iter().filter_map(prompt));
            match page["nextCursor"].as_str() {
                Some(next) if !next.is_empty() => cursor = Some(next.to_string()),
                _ => return Ok(prompts),
            }
        }
        Err(McpError::Protocol(format!("prompts/list kept paging past {MAX_TOOL_PAGES} pages")))
    }

    fn get_prompt(&self, name: &str, arguments: &BTreeMap<String, String>) -> Result<String, McpError> {
        let mut params = GetPromptRequestParams::new(name);
        if !arguments.is_empty() {
            params = params.with_arguments(arguments.iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect());
        }
        let result = self.request(ClientRequest::GetPromptRequest(GetPromptRequest::new(params)), &|| false, None)?;
        // The composer is not a turn: there is nobody there to answer.
        if result["resultType"] == "input_required" {
            return Err(McpError::Protocol(format!("{name} asks for input before it can be written, which the composer cannot give")));
        }
        Ok(render_prompt(&result))
    }
}

/// A tool entry, or nothing for one without a name — there is no way to
/// call it.
fn tool(entry: &Value) -> Option<McpTool> {
    let name = entry["name"].as_str().filter(|n| !n.is_empty())?;
    Some(McpTool {
        name: name.to_string(),
        description: entry["description"].as_str().unwrap_or_default().to_string(),
        input_schema: match &entry["inputSchema"] {
            schema @ Value::Object(_) => schema.clone(),
            _ => json!({ "type": "object" }),
        },
        // The newer place for a title is the tool's own; the older one is
        // among its annotations.
        title: [&entry["title"], &entry["annotations"]["title"]]
            .iter()
            .find_map(|title| title.as_str().filter(|t| !t.trim().is_empty()))
            .map(str::to_string),
        hints: {
            let read_only = entry["annotations"]["readOnlyHint"] == true;
            // A tool that only reads destroys nothing, whatever else is said.
            McpToolHints { read_only, destructive: !read_only && entry["annotations"]["destructiveHint"] == true }
        },
    })
}

fn handshake_error(error: ClientInitializeError, describe: &Describe) -> McpError {
    match error {
        ClientInitializeError::JsonRpcError(data) => {
            McpError::Handshake(format!("{} (code {})", data.message, data.code.0))
        }
        // The probe's refusal is how an older server is recognised; the
        // handshake that followed is the one that says what is wrong.
        ClientInitializeError::LegacyFallbackFailed { fallback, .. } => handshake_error(*fallback, describe),
        error @ (ClientInitializeError::NoCompatibleProtocolVersion { .. }
        | ClientInitializeError::NoPreferredProtocolVersion) => McpError::Handshake(error.to_string()),
        ClientInitializeError::ConnectionClosed(_) => describe(Failure::Closed),
        ClientInitializeError::TransportError { error, .. } => describe(Failure::Send(&error.error.to_string())),
        ClientInitializeError::Cancelled => McpError::Cancelled,
        other => McpError::Protocol(other.to_string()),
    }
}

/// `future`, unless the deadline, the stop flag or the end of what lies under
/// the transport comes first — the last for a process that exited while a
/// child of its own holds its stdout open, so the stream never closes. A question
/// the server asks meanwhile is put through `asking`, and the deadline moves
/// by however long the user took: that time is not the server's.
async fn wait<F: Future + Unpin>(
    future: &mut F,
    limit: Duration,
    cancelled: &dyn Fn() -> bool,
    ended: &Ended,
    mut asking: Option<&mut Asking<'_>>,
) -> Result<F::Output, Stop> {
    let deadline = tokio::time::sleep(limit);
    tokio::pin!(deadline);
    let mut poll = tokio::time::interval(POLL);
    loop {
        tokio::select! {
            biased;
            output = &mut *future => return Ok(output),
            Some((asked, reply)) = async { asking.as_mut()?.questions.recv().await }, if asking.is_some() => {
                let started = Instant::now();
                // Blocks this thread until the user answers — the thread that
                // waits on this call and on nothing else.
                let answer = asking.as_ref().map_or(McpAnswer::Decline, |asking| (asking.ask)(&asked));
                let _ = reply.send(answer);
                let later = deadline.deadline() + started.elapsed();
                deadline.as_mut().reset(later);
            }
            _ = &mut deadline => return Err(Stop::TimedOut),
            _ = poll.tick() => {
                if cancelled() {
                    return Err(Stop::Cancelled);
                }
                if let Some(error) = ended() {
                    return Err(Stop::Ended(error));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::atomic::AtomicUsize;
    use std::time::Instant;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    type Seen = Arc<Mutex<Vec<Value>>>;

    /// A server on the other end of an in-memory pipe, answering each message
    /// it reads with whatever `answer` returns — an empty list for silence,
    /// `None` to hang up. Every message it received is kept, in order.
    fn fake(
        timeout: Duration,
        answer: impl Fn(&Value) -> Option<Vec<Value>> + Send + 'static,
    ) -> (Result<RmcpClient, McpError>, Seen) {
        fake_stopped_by(timeout, &|| false, answer)
    }

    fn fake_stopped_by(
        timeout: Duration,
        cancelled: &dyn Fn() -> bool,
        answer: impl Fn(&Value) -> Option<Vec<Value>> + Send + 'static,
    ) -> (Result<RmcpClient, McpError>, Seen) {
        let (ours, theirs) = tokio::io::duplex(64 * 1024);
        let seen: Seen = Arc::default();
        let log = Arc::clone(&seen);
        tauri::async_runtime::spawn(async move {
            let (reads, mut writes) = tokio::io::split(theirs);
            let mut lines = BufReader::new(reads).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(message) = serde_json::from_str::<Value>(&line) else { continue };
                log.lock().unwrap().push(message.clone());
                let Some(replies) = answer(&message) else { return };
                for reply in replies {
                    // A string starting with RAW: goes out as bare text, the
                    // way a server that logs to stdout sends it.
                    let line = match reply.as_str().and_then(|r| r.strip_prefix("RAW:")) {
                        Some(raw) => raw.to_string(),
                        None => reply.to_string(),
                    };
                    if writes.write_all(format!("{line}\n").as_bytes()).await.is_err() {
                        return;
                    }
                }
            }
        });
        let gone = |_: Failure<'_>| McpError::Exited { code: Some(3), stderr: "gone".into() };
        let client = RmcpClient::connect(|| Ok(tokio::io::split(ours)), timeout, cancelled, Box::new(gone), Box::new(|| None));
        (client, seen)
    }

    fn ok(request: &Value, result: Value) -> Option<Vec<Value>> {
        Some(vec![json!({ "jsonrpc": "2.0", "id": request["id"], "result": result })])
    }

    fn refuse(request: &Value, code: i64, message: &str) -> Option<Vec<Value>> {
        Some(vec![json!({ "jsonrpc": "2.0", "id": request["id"], "error": { "code": code, "message": message } })])
    }

    /// A well-behaved server of the `initialize` era: it does not know the
    /// probe, shakes hands, lists two pages of tools, and answers a call.
    fn legacy(request: &Value) -> Option<Vec<Value>> {
        match request["method"].as_str() {
            Some("server/discover") => refuse(request, -32601, "Method not found"),
            Some("initialize") => ok(
                request,
                json!({ "protocolVersion": "2025-06-18", "capabilities": { "tools": {} },
                    "serverInfo": { "name": "f", "version": "1" }, "instructions": "Search before you open." }),
            ),
            Some("tools/list") if request["params"]["cursor"].is_null() => ok(
                request,
                json!({ "tools": [
                    { "name": "search", "description": "Search issues", "inputSchema": { "type": "object", "properties": { "q": { "type": "string" } } } },
                    { "description": "no name, cannot be called" }
                ], "nextCursor": "p2" }),
            ),
            Some("tools/list") => ok(request, json!({ "tools": [{ "name": "open" }] })),
            Some("tools/call") => ok(
                request,
                json!({ "content": [{ "type": "text", "text": format!("called {}", request["params"]["name"]) }], "isError": request["params"]["arguments"]["fail"] == true }),
            ),
            _ => Some(vec![]),
        }
    }

    /// Shakes hands, then says nothing.
    fn silent_after_handshake(request: &Value) -> Option<Vec<Value>> {
        match request["method"].as_str() {
            Some("server/discover" | "initialize") => legacy(request),
            _ => Some(vec![]),
        }
    }

    /// A server of 2026-07-28: no handshake, no session.
    fn modern(request: &Value) -> Option<Vec<Value>> {
        match request["method"].as_str() {
            Some("server/discover") => ok(
                request,
                json!({ "resultType": "complete", "supportedVersions": ["2026-07-28"], "capabilities": { "tools": {} },
                    "instructions": "Stateless.", "ttlMs": 60000, "cacheScope": "public" }),
            ),
            Some("initialize") => refuse(request, -32601, "Method not found"),
            Some("tools/list") => ok(
                request,
                json!({ "resultType": "complete", "tools": [{ "name": "search", "inputSchema": { "type": "object" } }], "ttlMs": 0, "cacheScope": "private" }),
            ),
            Some("tools/call") if request["params"]["name"] == "asks" => ok(
                request,
                json!({ "resultType": "input_required", "inputRequests": { "who": { "method": "elicitation/create",
                    "params": { "mode": "form", "message": "Who?", "requestedSchema": { "type": "object", "properties": {} } } } } }),
            ),
            Some("tools/call") => ok(request, json!({ "resultType": "complete", "content": [{ "type": "text", "text": "found it" }] })),
            _ => Some(vec![]),
        }
    }

    const SECOND: Duration = Duration::from_secs(1);

    fn methods(seen: &Seen) -> Vec<String> {
        seen.lock().unwrap().iter().filter_map(|m| m["method"].as_str().map(str::to_string)).collect()
    }

    /// An older server is recognised by its refusal of the probe and gets
    /// the handshake it knows, acknowledged before anything else is asked.
    #[test]
    fn an_older_server_is_probed_then_shaken_hands_with() {
        let (client, seen) = fake(SECOND, legacy);
        let client = client.unwrap();
        client.list_tools().unwrap();

        assert_eq!(methods(&seen)[..4], ["server/discover", "initialize", "notifications/initialized", "tools/list"]);
        let seen = seen.lock().unwrap();
        assert_eq!(seen[1]["params"]["protocolVersion"], "2025-11-25", "the newest version with a handshake");
        assert_eq!(seen[1]["params"]["clientInfo"]["name"], "kibo-agent");
        assert_eq!(seen[1]["params"]["capabilities"], json!({ "elicitation": { "form": {}, "url": {} } }), "questions, and nothing else");
        assert_eq!(client.instructions().as_deref(), Some("Search before you open."));
    }

    /// A server of the new era is spoken to with no handshake at all: every
    /// request says who asks and in which version.
    #[test]
    fn a_server_of_the_new_era_gets_no_handshake() {
        let (client, seen) = fake(SECOND, modern);
        let client = client.unwrap();
        assert_eq!(client.list_tools().unwrap()[0].name, "search");
        assert_eq!(client.call_tool("search", json!({ "q": "x" }), &|| false).unwrap().text, "found it");

        assert_eq!(methods(&seen), ["server/discover", "tools/list", "tools/call"]);
        let seen = seen.lock().unwrap();
        for request in seen.iter() {
            let meta = &request["params"]["_meta"];
            assert_eq!(meta["io.modelcontextprotocol/protocolVersion"], "2026-07-28", "{request}");
            assert_eq!(meta["io.modelcontextprotocol/clientInfo"]["name"], "kibo-agent", "{request}");
        }
        assert_eq!(client.instructions().as_deref(), Some("Stateless."));
    }

    /// The user's side of a question, for a test: records what was asked
    /// and answers as told, after `takes`.
    struct User {
        asked: Mutex<Vec<McpQuestion>>,
        answer: Value,
        takes: Duration,
    }

    impl User {
        fn answering(answer: Value, takes: Duration) -> Self {
            Self { asked: Mutex::default(), answer, takes }
        }
        fn ask(&self, question: &McpQuestion) -> McpAnswer {
            self.asked.lock().unwrap().push(question.clone());
            std::thread::sleep(self.takes);
            serde_json::from_value(self.answer.clone()).unwrap()
        }
    }

    fn form(message: &str) -> Value {
        json!({ "mode": "form", "message": message, "requestedSchema": { "type": "object",
            "properties": { "repo": { "type": "string" } }, "required": ["repo"] } })
    }

    /// A server of the new era asks by answering "input required"; the
    /// question is put, and the call is sent again — a new request, with the
    /// answers and the server's state as it gave it.
    #[test]
    fn a_call_that_asks_for_input_is_put_to_the_user_and_sent_again_with_the_answer() {
        let (client, seen) = fake(SECOND, |request| match request["method"].as_str() {
            Some("tools/call") if request["params"]["inputResponses"].is_null() => ok(
                request,
                json!({ "resultType": "input_required", "inputRequests": { "where": { "method": "elicitation/create", "params": form("Which repository?") } },
                    "requestState": "opaque-1" }),
            ),
            Some("tools/call") => ok(
                request,
                json!({ "resultType": "complete", "content": [{ "type": "text", "text": format!("opened {}", request["params"]["inputResponses"]["where"]["content"]["repo"]) }] }),
            ),
            _ => modern(request),
        });
        let user = User::answering(json!({ "action": "accept", "content": { "repo": "a/b", "unasked": "x" } }), Duration::ZERO);
        let done = client.unwrap().call_tool_asking("open", json!({ "n": 1 }), &|| false, &|q| user.ask(q)).unwrap();

        assert_eq!(done.text, "opened \"a/b\"");
        assert!(matches!(&user.asked.lock().unwrap()[..], [McpQuestion::Form { message, .. }] if message == "Which repository?"));
        let seen = seen.lock().unwrap();
        let calls: Vec<&Value> = seen.iter().filter(|m| m["method"] == "tools/call").collect();
        assert_eq!(calls.len(), 2);
        assert_ne!(calls[0]["id"], calls[1]["id"], "the retry is a request of its own");
        let retry = &calls[1]["params"];
        assert_eq!((&retry["name"], &retry["arguments"]), (&json!("open"), &json!({ "n": 1 })));
        assert_eq!(retry["requestState"], "opaque-1", "given back as it came");
        assert_eq!(retry["inputResponses"]["where"], json!({ "action": "accept", "content": { "repo": "a/b" } }), "only what was asked");
    }

    /// Asked for something this app never said it gives: the call fails and
    /// says what, rather than being sent again empty-handed.
    #[test]
    fn a_call_that_asks_for_what_was_never_offered_fails_and_says_what() {
        let (client, seen) = fake(SECOND, |request| match request["method"].as_str() {
            Some("tools/call") => ok(
                request,
                json!({ "resultType": "input_required", "inputRequests": { "m": { "method": "sampling/createMessage", "params": {} } } }),
            ),
            _ => modern(request),
        });
        let err = client.unwrap().call_tool_asking("x", json!({}), &|| false, &|_| panic!("put to the user")).unwrap_err();
        assert!(matches!(&err, McpError::Protocol(m) if m.contains("sampling/createMessage")), "{err}");
        assert_eq!(methods(&seen).iter().filter(|m| *m == "tools/call").count(), 1);
    }

    /// A server that asks round after round is given up on, not answered
    /// forever.
    #[test]
    fn a_server_that_keeps_asking_is_given_up_on() {
        let (client, seen) = fake(SECOND, |request| match request["method"].as_str() {
            Some("tools/call") => ok(request, json!({ "resultType": "input_required", "requestState": "again" })),
            _ => modern(request),
        });
        let err = client.unwrap().call_tool_asking("x", json!({}), &|| false, &|_| McpAnswer::Decline).unwrap_err();
        assert!(matches!(&err, McpError::Protocol(m) if m.contains("10 rounds")), "{err}");
        assert_eq!(methods(&seen).iter().filter(|m| *m == "tools/call").count(), 10);
    }

    /// Stop pressed while the question is open ends the call there: the
    /// answer is not sent on.
    #[test]
    fn a_stop_while_the_user_is_asked_ends_the_call_without_sending_it_again() {
        let (client, seen) = fake(SECOND, |request| match request["method"].as_str() {
            Some("tools/call") => ok(
                request,
                json!({ "resultType": "input_required", "inputRequests": { "w": { "method": "elicitation/create", "params": form("?") } } }),
            ),
            _ => modern(request),
        });
        let stopped = AtomicBool::new(false);
        let err = client
            .unwrap()
            .call_tool_asking("x", json!({}), &|| stopped.load(Ordering::SeqCst), &|_| {
                stopped.store(true, Ordering::SeqCst);
                McpAnswer::Cancel
            })
            .unwrap_err();
        assert_eq!(err, McpError::Cancelled);
        assert_eq!(methods(&seen).iter().filter(|m| *m == "tools/call").count(), 1);
    }

    /// A server of the `initialize` era asks with a request of its own while
    /// the call is open. The user's time is not the server's: a question
    /// answered after the call's whole timeout still lets the call finish.
    #[test]
    fn an_older_servers_own_question_is_put_to_the_user_and_the_calls_clock_stops_meanwhile() {
        let call_id = Arc::new(Mutex::new(Value::Null));
        let held = Arc::clone(&call_id);
        let (client, seen) = fake(Duration::from_millis(300), move |request| match (request["method"].as_str(), &request["id"]) {
            (Some("tools/call"), id) => {
                *held.lock().unwrap() = id.clone();
                Some(vec![json!({ "jsonrpc": "2.0", "id": "q-1", "method": "elicitation/create", "params": form("Which repository?") })])
            }
            // The user's answer, as this client's reply to the server's request.
            (None, id) if id == "q-1" => {
                let repo = request["result"]["content"]["repo"].clone();
                ok(&json!({ "id": held.lock().unwrap().clone() }), json!({ "content": [{ "type": "text", "text": format!("opened {repo}") }] }))
            }
            _ => legacy(request),
        });
        let user = User::answering(json!({ "action": "accept", "content": { "repo": "a/b" } }), Duration::from_millis(600));
        let done = client.unwrap().call_tool_asking("open", json!({}), &|| false, &|q| user.ask(q)).unwrap();

        assert_eq!(done.text, "opened \"a/b\"");
        assert_eq!(user.asked.lock().unwrap().len(), 1);
        let reply = seen.lock().unwrap().iter().find(|m| m["id"] == "q-1").cloned().unwrap();
        assert_eq!(reply["result"]["action"], "accept");
        assert!(!call_id.lock().unwrap().is_null());
    }

    /// Nobody to ask — a plain call, or a question nobody can put — is an
    /// answered question, not a hung server.
    #[test]
    fn a_question_nobody_can_answer_is_declined() {
        let replies = AtomicUsize::new(0);
        let (client, seen) = fake(SECOND, move |request| match (request["method"].as_str(), &request["id"]) {
            (Some("tools/call"), _) => Some(vec![
                json!({ "jsonrpc": "2.0", "id": "q-1", "method": "elicitation/create", "params": form("?") }),
                json!({ "jsonrpc": "2.0", "id": "q-2", "method": "elicitation/create",
                    "params": { "mode": "form", "message": "?", "requestedSchema": { "properties": { "tags": { "type": "array" } } } } }),
            ]),
            // The call is answered once both questions are.
            (None, id) if id.is_string() && replies.fetch_add(1, Ordering::SeqCst) == 1 => {
                ok(&json!({ "id": 2 }), json!({ "content": [{ "type": "text", "text": "fine" }] }))
            }
            _ => legacy(request),
        });
        assert_eq!(client.unwrap().call_tool("x", json!({}), &|| false).unwrap().text, "fine");
        let deadline = Instant::now() + Duration::from_secs(3);
        while seen.lock().unwrap().iter().filter(|m| m["id"].is_string()).count() < 2 {
            assert!(Instant::now() < deadline, "a question was left unanswered: {:#?}", seen.lock().unwrap());
            std::thread::sleep(Duration::from_millis(10));
        }
        let seen = seen.lock().unwrap();
        let reply = |id: &str| seen.iter().find(|m| m["id"] == id).unwrap().clone();
        assert_eq!(reply("q-1")["result"], json!({ "action": "decline" }));
        // A form the protocol does not allow (a list field) the SDK refuses
        // before it reaches this app — with an error, which answers too.
        assert!(reply("q-2")["error"].is_object(), "{}", reply("q-2"));
    }

    /// Prompts are asked only of a server that said it has them.
    #[test]
    fn prompts_are_listed_and_written_by_a_server_that_has_them() {
        let (client, seen) = fake(SECOND, |request| match request["method"].as_str() {
            Some("initialize") => ok(
                request,
                json!({ "protocolVersion": "2025-06-18", "capabilities": { "prompts": {} }, "serverInfo": { "name": "f", "version": "1" } }),
            ),
            Some("prompts/list") => ok(request, json!({ "prompts": [{ "name": "review", "arguments": [{ "name": "pr", "required": true }] }, { "title": "nameless" }] })),
            Some("prompts/get") => ok(
                request,
                json!({ "messages": [{ "role": "user", "content": { "type": "text", "text": format!("Review PR {}.", request["params"]["arguments"]["pr"]) } }] }),
            ),
            _ => legacy(request),
        });
        let client = client.unwrap();
        let prompts = client.list_prompts().unwrap();
        assert_eq!(prompts.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["review"]);
        assert!(prompts[0].arguments[0].required);
        let given = BTreeMap::from([("pr".to_string(), "42".to_string())]);
        assert_eq!(client.get_prompt("review", &given).unwrap(), "Review PR \"42\".");
        assert_eq!(seen.lock().unwrap().iter().find(|m| m["method"] == "prompts/get").unwrap()["params"]["name"], "review");

        // The composer is nobody to answer a question: such a prompt fails.
        let (asking, _seen) = fake(SECOND, |request| match request["method"].as_str() {
            Some("initialize") => ok(
                request,
                json!({ "protocolVersion": "2025-06-18", "capabilities": { "prompts": {} }, "serverInfo": { "name": "f", "version": "1" } }),
            ),
            Some("prompts/get") => ok(request, json!({ "resultType": "input_required", "requestState": "s" })),
            _ => legacy(request),
        });
        let err = asking.unwrap().get_prompt("review", &BTreeMap::new()).unwrap_err();
        assert!(matches!(&err, McpError::Protocol(m) if m.contains("asks for input")), "{err}");

        let (quiet, seen) = fake(SECOND, legacy);
        assert_eq!(quiet.unwrap().list_prompts().unwrap(), []);
        assert!(!methods(&seen).contains(&"prompts/list".to_string()), "asked a server that has none");
    }

    /// A prompt list can change like a tool list, and is read again with it.
    #[test]
    fn a_prompt_list_the_server_says_changed_makes_the_lists_stale() {
        let (client, _seen) = fake(SECOND, |request| match request["method"].as_str() {
            Some("tools/call") => {
                let mut replies = vec![json!({ "jsonrpc": "2.0", "method": "notifications/prompts/list_changed" })];
                replies.extend(legacy(request)?);
                Some(replies)
            }
            _ => legacy(request),
        });
        let client = client.unwrap();
        client.list_tools().unwrap();
        client.call_tool("x", json!({}), &|| false).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !client.tools_stale() {
            assert!(Instant::now() < deadline, "not heard");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn tools_are_listed_across_pages_and_a_nameless_one_is_skipped() {
        let (client, seen) = fake(SECOND, legacy);
        let tools = client.unwrap().list_tools().unwrap();

        assert_eq!(
            tools,
            [
                McpTool {
                    name: "search".into(),
                    description: "Search issues".into(),
                    input_schema: json!({ "type": "object", "properties": { "q": { "type": "string" } } }),
                    ..Default::default()
                },
                McpTool { name: "open".into(), input_schema: json!({ "type": "object" }), ..Default::default() },
            ]
        );
        let seen = seen.lock().unwrap();
        let second = seen.iter().filter(|m| m["method"] == "tools/list").nth(1).unwrap();
        assert_eq!(second["params"]["cursor"], "p2");
    }

    /// What a server says about a tool is read where it said it, and only
    /// what it said outright: silence is not "destructive".
    #[test]
    fn a_tools_title_and_hints_are_read_as_the_server_gave_them() {
        let (client, _seen) = fake(SECOND, |request| match request["method"].as_str() {
            Some("tools/list") => ok(
                request,
                json!({ "tools": [
                    { "name": "list", "title": "List issues", "annotations": { "readOnlyHint": true, "destructiveHint": true } },
                    { "name": "drop", "annotations": { "title": "Drop a table", "destructiveHint": true } },
                    { "name": "edit", "annotations": { "readOnlyHint": false } },
                    { "name": "plain", "title": "  " }
                ] }),
            ),
            _ => legacy(request),
        });
        let tools = client.unwrap().list_tools().unwrap();
        let told: Vec<_> = tools.iter().map(|t| (t.title.as_deref(), t.hints.read_only, t.hints.destructive)).collect();
        assert_eq!(
            told,
            [(Some("List issues"), true, false), (Some("Drop a table"), false, true), (None, false, false), (None, false, false)]
        );
    }

    /// A server that says its tools changed is believed until they are read
    /// again — and the reading is what clears it.
    #[test]
    fn a_list_the_server_says_changed_is_stale_until_it_is_read_again() {
        let (client, _seen) = fake(SECOND, |request| match request["method"].as_str() {
            Some("tools/call") => {
                let mut replies = vec![json!({ "jsonrpc": "2.0", "method": "notifications/tools/list_changed" })];
                replies.extend(legacy(request)?);
                Some(replies)
            }
            _ => legacy(request),
        });
        let client = client.unwrap();
        client.list_tools().unwrap();
        assert!(!client.tools_stale());

        client.call_tool("enable_more", json!({}), &|| false).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !client.tools_stale() {
            assert!(Instant::now() < deadline, "the server's notice was not heard");
            std::thread::sleep(Duration::from_millis(10));
        }
        client.list_tools().unwrap();
        assert!(!client.tools_stale());
    }

    /// A server of the new era says how long its list may be kept; one that
    /// says nothing is kept until it says otherwise.
    #[test]
    fn a_list_is_stale_once_the_time_the_server_gave_it_runs_out() {
        let (client, _seen) = fake(SECOND, |request| match request["method"].as_str() {
            Some("tools/list") => ok(request, json!({ "resultType": "complete", "tools": [], "ttlMs": 150, "cacheScope": "private" })),
            _ => modern(request),
        });
        let client = client.unwrap();
        assert!(!client.tools_stale(), "nothing read, nothing to go stale");
        client.list_tools().unwrap();
        assert!(!client.tools_stale());
        std::thread::sleep(Duration::from_millis(250));
        assert!(client.tools_stale());

        let (legacy_client, _seen) = fake(SECOND, legacy);
        let legacy_client = legacy_client.unwrap();
        legacy_client.list_tools().unwrap();
        std::thread::sleep(Duration::from_millis(50));
        assert!(!legacy_client.tools_stale(), "no time given, no expiry");
    }

    #[test]
    fn a_server_that_pages_forever_is_given_up_on() {
        let (client, _seen) = fake(SECOND, |request| match request["method"].as_str() {
            Some("tools/list") => ok(request, json!({ "tools": [], "nextCursor": "again" })),
            _ => legacy(request),
        });
        let err = client.unwrap().list_tools().unwrap_err();
        assert!(matches!(&err, McpError::Protocol(m) if m.contains("50 pages")), "{err}");
    }

    #[test]
    fn a_call_returns_its_text_and_whether_the_tool_failed() {
        let (client, seen) = fake(SECOND, legacy);
        let client = client.unwrap();

        let done = client.call_tool("search", json!({ "q": "bug" }), &|| false).unwrap();
        assert_eq!(done, McpCallResult { text: "called \"search\"".into(), is_error: false });
        let failed = client.call_tool("search", json!({ "fail": true }), &|| false).unwrap();
        assert!(failed.is_error);
        let seen = seen.lock().unwrap();
        let call = seen.iter().find(|m| m["method"] == "tools/call").unwrap();
        assert_eq!((&call["params"]["name"], &call["params"]["arguments"]), (&json!("search"), &json!({ "q": "bug" })));
    }

    /// The model has to hear it rather than wait on a server that went
    /// quiet — and the server is told, so it can stop too.
    #[test]
    fn a_silent_server_times_out_and_is_told_to_stop() {
        let (client, seen) = fake(Duration::from_millis(200), silent_after_handshake);
        let client = client.unwrap();
        let started = Instant::now();

        assert_eq!(client.call_tool("slow", json!({}), &|| false), Err(McpError::Timeout(0)));
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(client.is_alive(), "one slow call does not end the conversation");
        let (call, cancel) = call_and_its_cancellation(&seen);
        assert_eq!(cancel["params"]["requestId"], call["id"]);
        assert_eq!(cancel["params"]["reason"], "timed out");
    }

    #[test]
    fn a_stop_returns_at_once_and_tells_the_server() {
        let (client, seen) = fake(Duration::from_secs(30), silent_after_handshake);
        let client = client.unwrap();
        let started = Instant::now();
        let stop_after = Instant::now() + Duration::from_millis(100);

        assert_eq!(client.call_tool("slow", json!({}), &|| Instant::now() > stop_after), Err(McpError::Cancelled));
        assert!(started.elapsed() < Duration::from_secs(2), "waited out the 30 s instead");
        let (call, cancel) = call_and_its_cancellation(&seen);
        assert_eq!(cancel["params"]["requestId"], call["id"]);
        assert_eq!(cancel["params"]["reason"], "cancelled by the user");
    }

    fn call_and_its_cancellation(seen: &Seen) -> (Value, Value) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let seen = seen.lock().unwrap().clone();
            let find = |method: &str| seen.iter().find(|m| m["method"] == method).cloned();
            if let (Some(call), Some(cancel)) = (find("tools/call"), find("notifications/cancelled")) {
                return (call, cancel);
            }
            assert!(Instant::now() < deadline, "the server was never told: {seen:#?}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// The start is bounded like a call: by the server's timeout and by Stop,
    /// not by however long the era probe would have waited.
    #[test]
    fn a_start_nobody_answers_gives_up_at_the_timeout_or_the_stop() {
        let started = Instant::now();
        let (client, _seen) = fake(Duration::from_millis(300), |_| Some(vec![]));
        assert_eq!(client.err(), Some(McpError::Timeout(0)));
        let (client, _seen) = fake_stopped_by(Duration::from_secs(30), &|| true, |_| Some(vec![]));
        assert_eq!(client.err(), Some(McpError::Cancelled));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    /// The server dying mid-call is reported at once, with what the process
    /// layer knows about why — not after the timeout.
    #[test]
    fn a_server_that_goes_away_mid_call_is_reported_at_once() {
        let (client, _seen) = fake(Duration::from_secs(30), |request| match request["method"].as_str() {
            Some("tools/call") => None,
            _ => legacy(request),
        });
        let client = client.unwrap();
        assert!(client.is_alive());
        let started = Instant::now();

        let err = client.call_tool("slow", json!({}), &|| false).unwrap_err();
        assert_eq!(err, McpError::Exited { code: Some(3), stderr: "gone".into() });
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(!client.is_alive(), "known gone without another call to find out");
        // And every call after it, without waiting either.
        assert_eq!(client.call_tool("again", json!({}), &|| false).unwrap_err(), err);
        assert_eq!(client.list_tools().unwrap_err(), err);
    }

    /// An error answer is the server's, and says so.
    #[test]
    fn an_error_reply_carries_the_servers_code_and_message() {
        let (client, _seen) = fake(SECOND, |request| match request["method"].as_str() {
            Some("tools/call") => refuse(request, -32602, "unknown tool"),
            _ => legacy(request),
        });
        assert_eq!(
            client.unwrap().call_tool("nope", json!({}), &|| false),
            Err(McpError::Server { code: -32602, message: "unknown tool".into() })
        );
    }

    /// The server's own requests are answered, a log line on stdout is
    /// skipped, and an answer to nobody is dropped — none of it disturbs the
    /// call in flight.
    #[test]
    fn noise_from_the_server_does_not_disturb_a_call() {
        let (client, seen) = fake(SECOND, |request| match request["method"].as_str() {
            Some("tools/call") => {
                let mut replies = vec![
                    json!({ "jsonrpc": "2.0", "id": "srv-1", "method": "ping" }),
                    json!({ "jsonrpc": "2.0", "id": "srv-2", "method": "roots/list" }),
                    json!({ "jsonrpc": "2.0", "method": "notifications/message", "params": { "level": "info", "data": "x" } }),
                    json!({ "jsonrpc": "2.0", "id": 999, "result": {} }),
                    Value::String("RAW:Server listening on stdio".into()),
                ];
                replies.extend(legacy(request)?);
                Some(replies)
            }
            _ => legacy(request),
        });
        let client = client.unwrap();
        assert_eq!(client.call_tool("search", json!({}), &|| false).unwrap().text, "called \"search\"");

        let deadline = Instant::now() + Duration::from_secs(3);
        let answered = loop {
            let answered: HashMap<String, Value> = seen
                .lock()
                .unwrap()
                .iter()
                .filter(|m| m["id"].is_string())
                .map(|m| (m["id"].as_str().unwrap().to_string(), m.clone()))
                .collect();
            if answered.len() == 2 {
                break answered;
            }
            assert!(Instant::now() < deadline, "the server was left waiting: {answered:?}");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(answered["srv-1"]["result"], json!({}));
        assert!(client.is_alive());
    }

    /// A refused handshake says what the server said.
    #[test]
    fn a_refused_handshake_carries_the_servers_reason() {
        let (client, _seen) = fake(SECOND, |request| match request["method"].as_str() {
            Some("initialize") => refuse(request, -32000, "no token"),
            _ => legacy(request),
        });
        let err = client.err().unwrap();
        assert!(matches!(&err, McpError::Handshake(m) if m == "no token (code -32000)"), "{err}");
    }

    /// A server gone before it answered anything is described by whoever
    /// knows why — the process, with its exit code.
    #[test]
    fn a_server_that_hangs_up_on_the_start_is_described_by_its_transport() {
        let (client, _seen) = fake(SECOND, |_| None);
        assert_eq!(client.err(), Some(McpError::Exited { code: Some(3), stderr: "gone".into() }));
    }

    #[test]
    fn a_handshake_answer_that_is_not_one_is_a_protocol_error() {
        let (client, _seen) = fake(SECOND, |request| match request["method"].as_str() {
            Some("initialize") => ok(request, json!({})),
            _ => legacy(request),
        });
        assert!(matches!(client.err(), Some(McpError::Protocol(_))));
    }
}
