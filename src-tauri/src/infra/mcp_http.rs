//! An MCP server reached at a URL: the Streamable HTTP transport.
//!
//! The transport and the protocol over it are `rmcp`'s (`infra::mcp_rmcp`),
//! on its own `reqwest` client; this module says where to connect and with which headers,
//! and puts the transport's failures into the app's words
//! (`docs/22-rmcp-migration.md`).
//!
//! Left out on purpose: OAuth (a 401 says so; `docs/18-mcp-oauth.md`) and the
//! old HTTP+SSE transport.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::StreamableHttpClientTransport;
use serde_json::Value;

use crate::domain::mcp::{McpAnswer, McpCallResult, McpClient, McpError, McpPrompt, McpQuestion, McpServerConfig, McpTool};
use crate::infra::mcp_rmcp::{Failure, RmcpClient};

/// Enough of an error body to explain it; a server that answers an error
/// with a whole HTML page is cut here.
const ERROR_BODY_CHARS: usize = 1000;
/// A conversation with a server at a URL. Dropping it ends the session on
/// the server too, if the server keeps sessions.
pub struct HttpServer {
    client: RmcpClient,
    /// Why the conversation ended, for every call after it.
    ended: Ended,
}

type Ended = Arc<Mutex<Option<McpError>>>;

impl HttpServer {
    /// Opens the conversation within the server's own timeout.
    pub fn start(config: &McpServerConfig, cancelled: &dyn Fn() -> bool) -> Result<Self, McpError> {
        let url = config.url.clone().ok_or_else(|| McpError::NotStarted("the entry has no url".into()))?;
        let timeout = Duration::from_secs(config.timeout_secs());
        let mut headers = HashMap::new();
        for (name, value) in &config.headers {
            let name = http::HeaderName::try_from(name.as_str())
                .map_err(|_| McpError::NotStarted(format!("{name:?} is not a header name")))?;
            let value = http::HeaderValue::try_from(value.as_str())
                .map_err(|_| McpError::NotStarted(format!("the value of {name} cannot be sent as a header")))?;
            headers.insert(name, value);
        }
        // The SDK's `reqwest` is built without a TLS provider of its own; the
        // app's is ring, as for the model provider and the cluster.
        let _ = rustls::crypto::ring::default_provider().install_default();
        // A forgotten session is not quietly replaced: the call that met it
        // fails, and `services::mcp_servers` starts the server again — once a
        // turn, like a process that exited.
        let transport = StreamableHttpClientTransportConfig::with_uri(url)
            .custom_headers(headers)
            .reinit_on_expired_session(false);

        let ended = Ended::default();
        let said = Arc::clone(&ended);
        let client = RmcpClient::connect(
            // The SDK's own HTTP client: it follows no redirect, so the
            // entry's headers — a token, as a rule — reach this URL only.
            || Ok(StreamableHttpClientTransport::from_config(transport)),
            timeout,
            cancelled,
            Box::new(move |failure| describe(failure, &said)),
        )?;
        Ok(Self { client, ended })
    }

    fn ended(&self) -> Result<(), McpError> {
        lock(&self.ended).clone().map_or(Ok(()), Err)
    }
}

impl McpClient for HttpServer {
    fn list_tools(&self) -> Result<Vec<McpTool>, McpError> {
        self.ended()?;
        self.client.list_tools()
    }

    fn call_tool(&self, name: &str, arguments: Value, cancelled: &dyn Fn() -> bool) -> Result<McpCallResult, McpError> {
        self.ended()?;
        self.client.call_tool(name, arguments, cancelled)
    }

    fn call_tool_asking(
        &self,
        name: &str,
        arguments: Value,
        cancelled: &dyn Fn() -> bool,
        ask: &dyn Fn(&McpQuestion) -> McpAnswer,
    ) -> Result<McpCallResult, McpError> {
        self.ended()?;
        self.client.call_tool_asking(name, arguments, cancelled, ask)
    }

    fn list_prompts(&self) -> Result<Vec<McpPrompt>, McpError> {
        self.ended()?;
        self.client.list_prompts()
    }

    fn get_prompt(&self, name: &str, arguments: &BTreeMap<String, String>) -> Result<String, McpError> {
        self.ended()?;
        self.client.get_prompt(name, arguments)
    }

    fn instructions(&self) -> Option<String> {
        self.client.instructions()
    }

    fn tools_stale(&self) -> bool {
        self.client.tools_stale()
    }

    fn is_alive(&self) -> bool {
        self.client.is_alive() && lock(&self.ended).is_none()
    }
}

/// What a failure of the transport means. A refusal with a status is that
/// request's alone; a session the server forgot and a connection that broke
/// end the conversation, for this call and every one after it.
fn describe(failure: Failure<'_>, ended: &Mutex<Option<McpError>>) -> McpError {
    let fatal = match failure {
        Failure::Closed => McpError::Unreachable("the connection ended".into()),
        Failure::Send(text) => match refusal(text) {
            Some(refused) => return refused,
            None if text.contains("Session expired (HTTP 404)") => McpError::Http { status: 404, body: String::new() },
            None => McpError::Unreachable(text.to_string()),
        },
    };
    lock(ended).get_or_insert(fatal).clone()
}

/// The status and the start of the body of a non-2xx answer, read back out
/// of the transport's own wording — it keeps them nowhere else. A challenge
/// for a sign-in loses its body to the SDK, which keeps the challenge instead.
fn refusal(text: &str) -> Option<McpError> {
    if text.contains("Auth required") {
        return Some(McpError::Http { status: 401, body: String::new() });
    }
    if text.contains("Insufficient scope") {
        return Some(McpError::Http { status: 403, body: String::new() });
    }
    let (_, answer) = text.split_once("unexpected server response: HTTP ")?;
    let status = answer.get(..3)?.parse().ok()?;
    let body = answer.split_once(": ").map_or("", |(_, body)| body);
    Some(McpError::Http { status, body: body.chars().take(ERROR_BODY_CHARS).collect() })
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::time::Instant;

    /// One request as the fake server read it. `CLOSED` is not a request:
    /// the client hung up on a connection held open (status 1).
    #[derive(Debug, Clone)]
    struct Seen {
        at: Instant,
        method: String,
        path: String,
        headers: HashMap<String, String>,
        body: Value,
    }

    /// Status 0 hangs up without a word; 1 holds the connection open, saying
    /// nothing, until the client closes it.
    struct Answer {
        status: u16,
        headers: Vec<(&'static str, String)>,
        body: String,
    }

    type Log = Arc<Mutex<Vec<Seen>>>;

    /// A server on a free local port, answering each request with `answer`
    /// on a thread of its own — a slow answer holds up nothing else.
    fn serve(answer: impl Fn(&Seen) -> Answer + Send + Sync + 'static) -> (String, Log) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://127.0.0.1:{}/mcp", listener.local_addr().unwrap().port());
        let log: Log = Arc::default();
        let seen = Arc::clone(&log);
        let answer = Arc::new(answer);
        std::thread::spawn(move || {
            for socket in listener.incoming() {
                let Ok(mut socket) = socket else { return };
                let (answer, seen) = (Arc::clone(&answer), Arc::clone(&seen));
                std::thread::spawn(move || {
                    let Some(request) = read_request(&mut socket) else { return };
                    seen.lock().unwrap().push(request.clone());
                    let answer = answer(&request);
                    if answer.status == 0 {
                        return;
                    }
                    if answer.status == 1 {
                        let _ = socket.set_read_timeout(Some(Duration::from_secs(10)));
                        if matches!(socket.read(&mut [0; 1]), Ok(0)) {
                            seen.lock().unwrap().push(Seen { method: "CLOSED".into(), ..request });
                        }
                        return;
                    }
                    let mut head = format!("HTTP/1.1 {} X\r\n", answer.status);
                    for (name, value) in &answer.headers {
                        head.push_str(&format!("{name}: {value}\r\n"));
                    }
                    head.push_str(&format!("Content-Length: {}\r\nConnection: close\r\n\r\n", answer.body.len()));
                    let _ = socket.write_all(head.as_bytes());
                    let _ = socket.write_all(answer.body.as_bytes());
                });
            }
        });
        (url, log)
    }

    fn read_request(socket: &mut TcpStream) -> Option<Seen> {
        let mut reader = BufReader::new(socket.try_clone().ok()?);
        let mut first = String::new();
        reader.read_line(&mut first).ok()?;
        let mut words = first.split_whitespace();
        let method = words.next()?.to_string();
        let path = words.next()?.to_string();
        let mut headers = HashMap::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).ok()?;
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
            }
        }
        let length = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
        let mut body = vec![0; length];
        reader.read_exact(&mut body).ok()?;
        Some(Seen { at: Instant::now(), method, path, headers, body: serde_json::from_slice(&body).unwrap_or(Value::Null) })
    }

    fn json_answer(request: &Seen, result: Value) -> Answer {
        Answer {
            status: 200,
            headers: vec![("Content-Type", "application/json".into())],
            body: json!({ "jsonrpc": "2.0", "id": request.body["id"], "result": result }).to_string(),
        }
    }

    fn stream(body: String) -> Answer {
        Answer { status: 200, headers: vec![("Content-Type", "text/event-stream".into())], body }
    }

    fn events(messages: &[Value]) -> Answer {
        stream(messages.iter().map(|m| format!("event: message\ndata: {m}\n\n")).collect())
    }

    fn status(status: u16, body: &str) -> Answer {
        Answer { status, headers: vec![], body: body.into() }
    }

    fn handshake(request: &Seen, session: Option<&str>) -> Answer {
        let mut answer = json_answer(request, json!({ "protocolVersion": "2025-06-18", "capabilities": { "tools": { "listChanged": true } },
            "serverInfo": { "name": "f", "version": "1" }, "instructions": "Search first." }));
        if let Some(session) = session {
            answer.headers.push(("Mcp-Session-Id", session.into()));
        }
        answer
    }

    /// A server that keeps a session: the handshake as JSON, the tool list
    /// as an event split over several `data:` lines, a call as a stream that
    /// pings the client before answering.
    fn well_behaved(request: &Seen) -> Answer {
        if request.method == "DELETE" {
            return status(200, "");
        }
        // The stream for what the server has to say unasked: not offered.
        if request.method == "GET" {
            return status(405, "");
        }
        let id = &request.body["id"];
        match request.body["method"].as_str() {
            // Of the `initialize` era: a request outside a session is refused.
            Some("server/discover") => status(400, "Bad Request: no session"),
            Some("initialize") => handshake(request, Some("s-1")),
            Some("tools/list") => stream(format!(
                ": keep-alive\r\nevent: message\r\ndata: {{\"jsonrpc\": \"2.0\", \"id\": {id},\r\ndata: \"result\": {{\"tools\": [{{\"name\": \"search\"}}]}}}}\r\n\r\n"
            )),
            // The server's ids are its own: its ping may carry the very
            // number of the call it answers.
            Some("tools/call") => events(&[
                json!({ "jsonrpc": "2.0", "id": id, "method": "ping" }),
                json!({ "jsonrpc": "2.0", "method": "notifications/progress", "params": {} }),
                json!({ "jsonrpc": "2.0", "method": "notifications/tools/list_changed" }),
                json!({ "jsonrpc": "2.0", "id": id, "result": { "content": [{ "type": "text", "text": "found it" }] } }),
                json!({ "jsonrpc": "2.0", "id": "srv-late", "method": "ping" }),
            ]),
            _ => status(202, ""),
        }
    }

    fn config(url: &str, timeout_secs: u64) -> McpServerConfig {
        McpServerConfig {
            url: Some(url.into()),
            headers: [("Authorization".to_string(), "Bearer t0k3n".to_string())].into(),
            timeout_secs: Some(timeout_secs),
            ..Default::default()
        }
    }

    /// Waits for what a background thread will do; a test does not sleep
    /// its way to it.
    fn eventually(log: &Log, found: impl Fn(&Seen) -> bool) -> Seen {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(seen) = log.lock().unwrap().iter().find(|s| found(s)) {
                return seen.clone();
            }
            assert!(Instant::now() < deadline, "never arrived: {:#?}", log.lock().unwrap());
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn position(log: &Log, method: &str) -> usize {
        log.lock().unwrap().iter().position(|s| s.body["method"] == method).unwrap()
    }

    #[test]
    fn a_session_is_started_used_and_ended() {
        let (url, log) = serve(well_behaved);
        let server = HttpServer::start(&config(&url, 5), &|| false).unwrap();
        assert_eq!(server.list_tools().unwrap()[0].name, "search", "an event over several data: lines");
        assert_eq!(server.instructions().as_deref(), Some("Search first."));
        assert!(!server.tools_stale());
        assert_eq!(server.call_tool("search", json!({ "q": "x" }), &|| false).unwrap().text, "found it");
        let heard = Instant::now() + Duration::from_secs(3);
        while !server.tools_stale() && Instant::now() < heard {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(server.tools_stale(), "the server said its tools changed, in the call's own stream");

        let seen = log.lock().unwrap().clone();
        let probe = &seen[0];
        assert_eq!(probe.body["method"], "server/discover", "asked first, in case the server has no handshake");
        assert_eq!(probe.headers["authorization"], "Bearer t0k3n");
        let at = position(&log, "initialize");
        let init = &seen[at];
        for accepted in ["application/json", "text/event-stream"] {
            assert!(init.headers["accept"].contains(accepted), "{init:?}");
        }
        assert_eq!(init.headers["content-type"], "application/json");
        assert_eq!(init.headers["authorization"], "Bearer t0k3n");
        assert!(!init.headers.contains_key("mcp-session-id") && !init.headers.contains_key("mcp-protocol-version"));
        for later in &seen[at + 1..] {
            assert_eq!(later.headers["mcp-session-id"], "s-1", "{later:?}");
            assert_eq!(later.headers["mcp-protocol-version"], "2025-06-18", "{later:?}");
            assert_eq!(later.headers["authorization"], "Bearer t0k3n");
        }
        assert!(position(&log, "notifications/initialized") < position(&log, "tools/list"), "acknowledged first");

        let call = eventually(&log, |s| s.body["method"] == "tools/call");
        let pong = eventually(&log, |s| s.body["id"] == call.body["id"] && s.body.get("method").is_none());
        assert_eq!(pong.body["result"], json!({}), "the server's ping is answered");
        assert_eq!(pong.headers["mcp-session-id"], "s-1");

        drop(server);
        let delete = eventually(&log, |s| s.method == "DELETE");
        assert_eq!((delete.headers["mcp-session-id"].as_str(), delete.headers["authorization"].as_str()), ("s-1", "Bearer t0k3n"));
    }

    /// The server may refuse what comes before the acknowledgement, so the
    /// next request waits for it to be taken — even when taking it is slow.
    #[test]
    fn nothing_is_sent_before_the_handshake_is_acknowledged() {
        let (url, log) = serve(|request| match request.body["method"].as_str() {
            Some("notifications/initialized") => {
                std::thread::sleep(Duration::from_millis(300));
                status(202, "")
            }
            _ => well_behaved(request),
        });
        let server = HttpServer::start(&config(&url, 5), &|| false).unwrap();
        server.list_tools().unwrap();
        let at = |method: &str| {
            let index = position(&log, method);
            log.lock().unwrap()[index].at
        };
        assert!(at("tools/list") >= at("notifications/initialized") + Duration::from_millis(300));
    }

    /// No session, nothing to end.
    #[test]
    fn a_server_without_sessions_is_sent_none_and_not_told_goodbye() {
        let (url, log) = serve(|request| match request.body["method"].as_str() {
            Some("initialize") => handshake(request, None),
            _ => well_behaved(request),
        });
        let server = HttpServer::start(&config(&url, 5), &|| false).unwrap();
        server.list_tools().unwrap();
        drop(server);
        std::thread::sleep(Duration::from_millis(200));
        let seen = log.lock().unwrap();
        assert!(seen.iter().all(|s| !s.headers.contains_key("mcp-session-id") && s.method == "POST"), "{seen:#?}");
    }

    #[test]
    fn a_server_that_wants_a_sign_in_does_not_start_and_says_why() {
        let (url, _log) = serve(|_| status(401, "missing bearer token"));
        let Err(err) = HttpServer::start(&config(&url, 5), &|| false) else { panic!("started") };
        assert_eq!(err, McpError::Http { status: 401, body: "missing bearer token".into() });
        assert!(err.to_string().contains("OAuth"));
    }

    /// A server's prompts and questions reach it over HTTP too — and not
    /// once its session is over.
    #[test]
    fn prompts_and_questions_go_over_http() {
        let (url, log) = serve(|request| match request.body["method"].as_str() {
            Some("initialize") => {
                let mut answer = json_answer(
                    request,
                    json!({ "protocolVersion": "2025-06-18", "capabilities": { "prompts": {} }, "serverInfo": { "name": "f", "version": "1" } }),
                );
                answer.headers.push(("Mcp-Session-Id", "s-1".into()));
                answer
            }
            Some("prompts/list") => json_answer(request, json!({ "prompts": [{ "name": "greet" }] })),
            Some("prompts/get") => json_answer(request, json!({ "messages": [{ "role": "user", "content": { "type": "text", "text": "Hello." } }] })),
            Some("tools/call") if request.body["params"]["name"] == "gone" => status(404, ""),
            Some("tools/call") if request.body["params"]["inputResponses"].is_null() => json_answer(
                request,
                json!({ "resultType": "input_required", "inputRequests": { "q": { "method": "elicitation/create",
                    "params": { "mode": "url", "message": "Sign in", "url": "https://a.example/in" } } } }),
            ),
            Some("tools/call") => json_answer(request, json!({ "content": [{ "type": "text", "text": "in" }] })),
            _ => well_behaved(request),
        });
        let server = HttpServer::start(&config(&url, 5), &|| false).unwrap();
        assert_eq!(server.list_prompts().unwrap()[0].name, "greet");
        assert_eq!(server.get_prompt("greet", &BTreeMap::new()).unwrap(), "Hello.");
        let agreed = |_: &McpQuestion| McpAnswer::Accept { content: Default::default() };
        let done = server.call_tool_asking("login", json!({}), &|| false, &agreed).unwrap();
        assert_eq!(done.text, "in");
        let retry = log.lock().unwrap().iter().filter(|s| s.body["method"] == "tools/call").nth(1).unwrap().body.clone();
        assert_eq!(retry["params"]["inputResponses"]["q"], json!({ "action": "accept" }), "the user's answer, not a default");

        server.call_tool("gone", json!({}), &|| false).unwrap_err();
        let over = |r: Result<(), McpError>| assert!(matches!(r, Err(McpError::Http { status: 404, .. })), "{r:?}");
        over(server.list_prompts().map(|_| ()));
        over(server.get_prompt("greet", &BTreeMap::new()).map(|_| ()));
        over(server.call_tool_asking("login", json!({}), &|| false, &|_| McpAnswer::Decline).map(|_| ()));
    }

    /// A server that challenges for a sign-in: the SDK keeps the challenge
    /// for the OAuth that is not built yet, and the row still says what is
    /// wanted.
    #[test]
    fn a_challenge_for_a_sign_in_or_for_more_rights_is_a_refusal_with_its_status() {
        for (code, challenge) in [(401, "Bearer resource_metadata=\"https://a.example/meta\""), (403, "Bearer error=\"insufficient_scope\"")] {
            let (url, _log) = serve(move |_| Answer { status: code, headers: vec![("WWW-Authenticate", challenge.into())], body: String::new() });
            let Err(err) = HttpServer::start(&config(&url, 5), &|| false) else { panic!("started") };
            assert_eq!(err, McpError::Http { status: code, body: String::new() });
        }
        let (url, _log) = serve(|_| Answer { status: 401, headers: vec![("WWW-Authenticate", "Bearer".into())], body: String::new() });
        assert!(HttpServer::start(&config(&url, 5), &|| false).err().unwrap().to_string().contains("OAuth"));
    }

    /// The entry's headers are a token, as a rule, and were given for this
    /// address: a redirect is an answer that is not success, never a second
    /// request carrying them somewhere else.
    #[test]
    fn a_redirect_is_not_followed() {
        let (url, log) = serve(|request| match request.path.as_str() {
            "/mcp" => Answer { status: 307, headers: vec![("Location", "/elsewhere".into())], body: String::new() },
            _ => well_behaved(request),
        });
        let Err(err) = HttpServer::start(&config(&url, 5), &|| false) else { panic!("started") };
        assert!(matches!(err, McpError::Http { status: 307, .. }), "{err}");
        assert!(log.lock().unwrap().iter().all(|s| s.path == "/mcp"), "{:#?}", log.lock().unwrap());
    }

    /// A header that cannot be sent is the entry's mistake, said before any
    /// connection is made.
    #[test]
    fn a_header_that_cannot_be_sent_does_not_start_the_server() {
        let mut entry = config("http://127.0.0.1:1/mcp", 5);
        entry.headers.insert("X Bad Name".into(), "v".into());
        assert!(matches!(HttpServer::start(&entry, &|| false), Err(McpError::NotStarted(m)) if m.contains("X Bad Name")));
        let mut entry = config("http://127.0.0.1:1/mcp", 5);
        entry.headers.insert("X-Token".into(), "line\nbreak".into());
        assert!(matches!(HttpServer::start(&entry, &|| false), Err(McpError::NotStarted(m)) if m.contains("x-token")));
    }

    /// One refused call is that call's failure, not the server's.
    #[test]
    fn an_error_status_fails_the_call_and_keeps_the_session() {
        let (url, _log) = serve(|request| match request.body["method"].as_str() {
            Some("tools/call") => status(500, &"x".repeat(5000)),
            _ => well_behaved(request),
        });
        let server = HttpServer::start(&config(&url, 5), &|| false).unwrap();
        let err = server.call_tool("search", json!({}), &|| false).unwrap_err();
        assert_eq!(err, McpError::Http { status: 500, body: "x".repeat(ERROR_BODY_CHARS) }, "the body is cut");
        assert!(server.is_alive());
        assert_eq!(server.list_tools().unwrap().len(), 1);
    }

    /// The server forgot the session: this call fails, the server counts as
    /// gone, and the restart in `services::mcp_servers` starts a new one.
    #[test]
    fn a_forgotten_session_ends_the_connection() {
        let (url, _log) = serve(|request| match request.body["method"].as_str() {
            Some("tools/call") => status(404, "unknown session"),
            _ => well_behaved(request),
        });
        let server = HttpServer::start(&config(&url, 30), &|| false).unwrap();
        // The SDK reads a 404 in a session as the session's end and keeps no body.
        let gone = McpError::Http { status: 404, body: String::new() };
        assert_eq!(server.call_tool("search", json!({}), &|| false).unwrap_err(), gone);
        assert!(!server.is_alive());
        let started = Instant::now();
        assert_eq!(server.list_tools().unwrap_err(), gone, "and every call after it");
        assert!(started.elapsed() < Duration::from_secs(5), "without waiting");
    }

    /// The SDK can replace a forgotten session by itself and send the call
    /// again; it is told not to. The rule is the app's — a failed call is
    /// never repeated — and a restart belongs to `services::mcp_servers`,
    /// once a turn.
    #[test]
    fn a_call_that_met_a_forgotten_session_is_not_sent_again() {
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let (url, log) = serve(move |request| match request.body["method"].as_str() {
            Some("tools/call") if calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 => status(404, "unknown session"),
            _ => well_behaved(request),
        });
        let server = HttpServer::start(&config(&url, 30), &|| false).unwrap();
        assert!(matches!(server.call_tool("search", json!({}), &|| false), Err(McpError::Http { status: 404, .. })));
        std::thread::sleep(Duration::from_millis(300));
        let count = |method: &str| log.lock().unwrap().iter().filter(|s| s.body["method"] == method).count();
        assert_eq!((count("tools/call"), count("initialize")), (1, 1));
    }

    /// Without a session, a 404 is the call's own — the tool, say, is
    /// behind a path that is not there.
    #[test]
    fn a_404_without_a_session_is_the_calls_failure() {
        let (url, _log) = serve(|request| match request.body["method"].as_str() {
            Some("initialize") => handshake(request, None),
            Some("tools/call") => status(404, "not here"),
            _ => well_behaved(request),
        });
        let server = HttpServer::start(&config(&url, 5), &|| false).unwrap();
        assert!(matches!(server.call_tool("search", json!({}), &|| false), Err(McpError::Http { status: 404, .. })));
        assert!(server.is_alive());
    }

    /// Gone mid-session: this call fails, and so does every one after it
    /// until the restart.
    #[test]
    fn a_server_that_hangs_up_ends_the_connection() {
        let (url, _log) = serve(|request| match request.body["method"].as_str() {
            Some("tools/call") => status(0, ""),
            _ => well_behaved(request),
        });
        let server = HttpServer::start(&config(&url, 30), &|| false).unwrap();
        let err = server.call_tool("search", json!({}), &|| false).unwrap_err();
        assert!(matches!(err, McpError::Unreachable(_)), "{err}");
        assert!(!server.is_alive());
        assert_eq!(server.list_tools().unwrap_err(), err);
    }

    #[test]
    fn a_server_nobody_listens_for_is_unreachable() {
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let result = HttpServer::start(&config(&format!("http://127.0.0.1:{port}/mcp"), 5), &|| false);
        assert!(matches!(result, Err(McpError::Unreachable(_))), "{:?}", result.err());
    }

    #[test]
    fn a_stop_returns_at_once_and_tells_the_server() {
        let (url, log) = serve(|request| match request.body["method"].as_str() {
            Some("tools/call") => {
                std::thread::sleep(Duration::from_secs(3));
                well_behaved(request)
            }
            _ => well_behaved(request),
        });
        let server = HttpServer::start(&config(&url, 30), &|| false).unwrap();
        let started = Instant::now();
        let stop_after = started + Duration::from_millis(100);
        assert_eq!(server.call_tool("slow", json!({}), &|| Instant::now() > stop_after), Err(McpError::Cancelled));
        assert!(started.elapsed() < Duration::from_secs(2), "waited for the server instead");

        let call = eventually(&log, |s| s.body["method"] == "tools/call");
        let cancel = eventually(&log, |s| s.body["method"] == "notifications/cancelled");
        assert_eq!(cancel.body["params"]["requestId"], call.body["id"]);
        assert_eq!(cancel.headers["mcp-session-id"], "s-1");
    }

    /// A server that never answers does not hold a thread and a connection
    /// here forever: the request gives up with the call.
    #[test]
    fn a_request_nobody_answers_is_given_up() {
        let (url, log) = serve(|request| match request.body["method"].as_str() {
            Some("tools/call") => status(1, ""),
            _ => well_behaved(request),
        });
        let server = HttpServer::start(&config(&url, 1), &|| false).unwrap();
        assert_eq!(server.call_tool("hang", json!({}), &|| false), Err(McpError::Timeout(1)));
        let closed = eventually(&log, |s| s.method == "CLOSED");
        assert_eq!(closed.body["method"], "tools/call");
    }

    /// A slow answer is a failed call, not a dead server.
    #[test]
    fn a_slow_server_times_out_and_stays() {
        let (url, _log) = serve(|request| match request.body["method"].as_str() {
            Some("tools/call") => {
                std::thread::sleep(Duration::from_secs(3));
                well_behaved(request)
            }
            _ => well_behaved(request),
        });
        let server = HttpServer::start(&config(&url, 1), &|| false).unwrap();
        let started = Instant::now();
        assert_eq!(server.call_tool("slow", json!({}), &|| false), Err(McpError::Timeout(1)));
        assert!(started.elapsed() < Duration::from_millis(2500));
        std::thread::sleep(Duration::from_millis(500));
        assert!(server.is_alive(), "the request's own timeout does not end the session");
    }

    /// A stream that ends without the answer took the call with it: reported
    /// at once, and the server counts as gone — the restart in
    /// `services::mcp_servers` starts a new conversation.
    #[test]
    fn a_stream_that_ends_without_the_answer_ends_the_connection() {
        let (url, _log) = serve(|request| match request.body["method"].as_str() {
            Some("tools/call") => events(&[json!({ "jsonrpc": "2.0", "method": "notifications/progress", "params": {} })]),
            _ => well_behaved(request),
        });
        let server = HttpServer::start(&config(&url, 30), &|| false).unwrap();
        let started = Instant::now();
        let err = server.call_tool("search", json!({}), &|| false).unwrap_err();
        assert!(matches!(err, McpError::Unreachable(_)), "{err}");
        assert!(started.elapsed() < Duration::from_secs(5), "waited out the timeout");
        assert!(!server.is_alive());
        assert_eq!(server.list_tools().unwrap_err(), err, "and every call after it");
    }

    /// A server that only accepts a request, or answers it with something
    /// else, never answers it: the SDK keeps waiting, so the call's own
    /// timeout is what ends it — and it is the call's failure alone.
    #[test]
    fn an_answer_that_never_comes_is_the_calls_timeout() {
        let (url, _log) = serve(|request| match request.body["method"].as_str() {
            Some("tools/call") => Answer {
                status: 200,
                headers: vec![("Content-Type", "application/json".into())],
                body: json!({ "jsonrpc": "2.0", "method": "notifications/message", "params": {} }).to_string(),
            },
            Some("tools/list") => status(202, ""),
            _ => well_behaved(request),
        });
        let server = HttpServer::start(&config(&url, 1), &|| false).unwrap();
        assert_eq!(server.list_tools().unwrap_err(), McpError::Timeout(1), "accepted, never answered");
        assert_eq!(server.call_tool("json", json!({}), &|| false).unwrap_err(), McpError::Timeout(1));
        assert!(server.is_alive());
    }

    /// An error answer in JSON is the server's own, with its code.
    #[test]
    fn an_error_answer_in_json_carries_the_servers_code() {
        let (url, _log) = serve(|request| match request.body["method"].as_str() {
            Some("tools/call") => Answer {
                status: 200,
                headers: vec![("Content-Type", "application/json".into())],
                body: json!({ "jsonrpc": "2.0", "id": request.body["id"], "error": { "code": -32602, "message": "unknown tool" } })
                    .to_string(),
            },
            _ => well_behaved(request),
        });
        let server = HttpServer::start(&config(&url, 5), &|| false).unwrap();
        assert_eq!(
            server.call_tool("nope", json!({}), &|| false),
            Err(McpError::Server { code: -32602, message: "unknown tool".into() })
        );
    }
}
