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
//!   process, its exit code and the last lines of its stderr.
//!
//! `rmcp` is async on tokio and everything above `McpClient` is blocking, so
//! each method is one `block_on` on Tauri's runtime — the same bridge as
//! `infra::kube_client`. Never call these from a task of that runtime.

use std::future::Future;
use std::time::Duration;

use rmcp::model::{
    CallToolRequest, CallToolRequestParams, ClientCapabilities, ClientConfig, ClientRequest, Implementation,
    ListToolsRequest, PaginatedRequestParams, ProtocolVersion,
};
use rmcp::service::{
    ClientInitializeError, ClientLifecycleMode, ClientServiceExt, Peer, PeerRequestOptions, RoleClient, RunningService,
    ServiceError,
};
use rmcp::transport::IntoTransport;
use serde_json::{json, Value};

use crate::domain::mcp::{render_content, McpCallResult, McpClient, McpError, McpTool};

/// How often a waiting call looks at its stop flag.
const POLL: Duration = Duration::from_millis(50);
/// Telling the server a call was abandoned is a courtesy; a transport that
/// cannot take the message does not hold the caller.
const CANCEL_GRACE: Duration = Duration::from_secs(1);
/// A server that pages its tool list forever is broken, not large.
const MAX_TOOL_PAGES: usize = 50;

/// What went wrong below the protocol, for the transport to put into words.
pub enum Failure<'a> {
    /// The conversation is over.
    Closed,
    /// One message could not be sent; the text is the transport's own.
    Send(&'a str),
}

pub type Describe = Box<dyn Fn(Failure<'_>) -> McpError + Send + Sync>;

pub struct RmcpClient {
    /// Held for its end: dropping it closes the conversation — for an HTTP
    /// session, with the `DELETE` that ends it on the server.
    _service: RunningService<RoleClient, ClientConfig>,
    peer: Peer<RoleClient>,
    timeout: Duration,
    describe: Describe,
}

enum Stop {
    Cancelled,
    TimedOut,
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
    ) -> Result<Self, McpError>
    where
        T: IntoTransport<RoleClient, E, A>,
        E: std::error::Error + Send + Sync + 'static,
    {
        let config = ClientConfig::new(
            ClientCapabilities::default(),
            Implementation::new("kibo-agent", env!("CARGO_PKG_VERSION")),
        );
        let lifecycle = ClientLifecycleMode::Auto {
            preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            legacy_version: Some(ProtocolVersion::V_2025_11_25),
        };
        let service = tauri::async_runtime::block_on(async {
            let mut serving = Box::pin(config.serve_with_lifecycle(transport()?, lifecycle));
            match wait(&mut serving, timeout, cancelled).await {
                Ok(served) => served.map_err(|e| handshake_error(e, &describe)),
                Err(Stop::Cancelled) => Err(McpError::Cancelled),
                Err(Stop::TimedOut) => Err(McpError::Timeout(timeout.as_secs())),
            }
        })?;
        Ok(Self { peer: service.peer().clone(), _service: service, timeout, describe })
    }

    /// What the server said about using its tools, when it said anything.
    pub fn instructions(&self) -> Option<String> {
        self.peer.peer_info().and_then(|info| info.instructions.clone())
    }

    /// One request and its answer as JSON. Through `Value` rather than the
    /// SDK's types: an answer its schema refuses arrives as a custom result,
    /// and what can be read from it still is.
    fn request(&self, request: ClientRequest, cancelled: &dyn Fn() -> bool) -> Result<Value, McpError> {
        tauri::async_runtime::block_on(async {
            let mut handle = self
                .peer
                .send_cancellable_request(request, PeerRequestOptions::no_options())
                .await
                .map_err(|e| self.error(e))?;
            let stop = match wait(&mut handle.rx, self.timeout, cancelled).await {
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
}

impl McpClient for RmcpClient {
    fn list_tools(&self) -> Result<Vec<McpTool>, McpError> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_TOOL_PAGES {
            let params = PaginatedRequestParams::default().with_cursor(cursor.take());
            let request = ClientRequest::ListToolsRequest(ListToolsRequest::with_param(params));
            let page = self.request(request, &|| false)?;
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
        let mut params = CallToolRequestParams::new(name.to_string());
        if let Value::Object(arguments) = arguments {
            params = params.with_arguments(arguments);
        }
        let result = self.request(ClientRequest::CallToolRequest(CallToolRequest::new(params)), cancelled)?;
        // Only a client that declared it can answer may be asked for input,
        // and this one declares nothing; a server that asks anyway gets no
        // retry, and the model hears why.
        if result["resultType"] == "input_required" {
            return Err(McpError::Protocol(format!("{name} asked for input mid-call, which this app cannot give yet")));
        }
        Ok(McpCallResult { text: render_content(&result), is_error: result["isError"].as_bool().unwrap_or(false) })
    }

    fn is_alive(&self) -> bool {
        !self.peer.is_transport_closed()
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

/// `future`, unless the deadline or the stop flag comes first.
async fn wait<F: Future + Unpin>(future: &mut F, limit: Duration, cancelled: &dyn Fn() -> bool) -> Result<F::Output, Stop> {
    let deadline = tokio::time::sleep(limit);
    tokio::pin!(deadline);
    let mut poll = tokio::time::interval(POLL);
    loop {
        tokio::select! {
            biased;
            output = &mut *future => return Ok(output),
            _ = &mut deadline => return Err(Stop::TimedOut),
            _ = poll.tick() => {
                if cancelled() {
                    return Err(Stop::Cancelled);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
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
        let client = RmcpClient::connect(|| Ok(tokio::io::split(ours)), timeout, cancelled, Box::new(gone));
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
        assert_eq!(seen[1]["params"]["capabilities"], json!({}), "nothing is promised that is not built");
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

    /// Nothing here can answer a server's question yet, and nothing said it
    /// could; the call fails with the reason rather than being retried blind.
    #[test]
    fn a_call_that_asks_for_input_fails_and_says_why() {
        let (client, seen) = fake(SECOND, modern);
        let err = client.unwrap().call_tool("asks", json!({}), &|| false).unwrap_err();
        assert!(matches!(&err, McpError::Protocol(m) if m.contains("asked for input")), "{err}");
        assert_eq!(methods(&seen).iter().filter(|m| *m == "tools/call").count(), 1, "not retried");
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
                },
                McpTool { name: "open".into(), description: String::new(), input_schema: json!({ "type": "object" }) },
            ]
        );
        let seen = seen.lock().unwrap();
        let second = seen.iter().filter(|m| m["method"] == "tools/list").nth(1).unwrap();
        assert_eq!(second["params"]["cursor"], "p2");
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
