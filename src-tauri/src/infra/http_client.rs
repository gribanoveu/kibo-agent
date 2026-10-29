//! Sends the HTTP page's requests (`docs/45-http-client.md`, decision 3): the
//! one place they reach the network, over `http_agent`'s TLS, so a stand's own
//! certificate is trusted the way a provider's is.

use std::time::{Duration, Instant};

use ureq::http::{Method, Request, Response};
use ureq::{AsSendBody, Body, ResponseExt};
use url::Url;

use super::http_agent::build_agent;
use crate::domain::http_client::{Outcome, Outgoing};

/// The page waits this long for the whole of a response, redirects and body included.
pub const TIMEOUT: Duration = Duration::from_secs(30);
/// A larger body is refused with its size: the request keeps its response whole (decision 4).
pub const BODY_LIMIT: u64 = 10 << 20;
/// As Postman and `curl -L`: the page follows them and shows the way it went.
const MAX_REDIRECTS: u32 = 10;

/// What came back, and its body. A failure is an outcome too: a stand that
/// does not answer is what the user sent the request to find out.
pub fn send(request: &Outgoing, trusted_cert: Option<&str>) -> (Outcome, Vec<u8>) {
    send_within(request, trusted_cert, TIMEOUT)
}

fn send_within(request: &Outgoing, trusted_cert: Option<&str>, timeout: Duration) -> (Outcome, Vec<u8>) {
    exchange(request, trusted_cert, timeout).unwrap_or_else(|Failed(error)| (Outcome::Failed { error }, Vec::new()))
}

/// Why nothing came back, in the words `Outcome::Failed` shows.
struct Failed(String);

impl From<String> for Failed {
    fn from(why: String) -> Failed {
        Failed(why)
    }
}

fn exchange(request: &Outgoing, trusted_cert: Option<&str>, timeout: Duration) -> Result<(Outcome, Vec<u8>), Failed> {
    let started = Instant::now();
    let agent = build_agent(trusted_cert).map_err(|e| e.to_string())?;
    let method = Method::from_bytes(request.method.as_bytes()).map_err(|_| format!("{} is not an HTTP method", request.method))?;
    let mut builder = Request::builder().method(method).uri(address(&request.url)?);
    for (name, value) in &request.headers {
        builder = builder.header(name, value);
    }
    // No body is no body: an empty one would still send `Content-Length: 0` with a GET.
    let mut response = match &request.body {
        Some(body) => run(&agent, builder.body(body.as_bytes()), timeout),
        None => run(&agent, builder.body(()), timeout),
    }?;

    let status = response.status().as_u16();
    let headers = response.headers().iter().map(|(k, v)| (k.to_string(), String::from_utf8_lossy(v.as_bytes()).into_owned())).collect();
    let url = response.get_uri().to_string();
    // The history holds the request's own address, each hop, and the last — which is `url`.
    let mut redirects: Vec<String> = response.get_redirect_history().unwrap_or_default().iter().map(ToString::to_string).collect();
    redirects.pop();
    let length = response.headers().get("content-length").and_then(|v| v.to_str().ok()?.parse::<u64>().ok());
    // ureq's limit refuses the read after its last byte — the one that would find the end — so 10 MB whole needs one more.
    let body = response.body_mut().with_config().limit(BODY_LIMIT + 1).read_to_vec().map_err(|e| match e {
        ureq::Error::BodyExceedsLimit(_) => too_large(length),
        e => described(e, timeout),
    })?;
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    Ok((Outcome::Response { status, headers, url, redirects, elapsed_ms }, body))
}

/// The address as a browser would send it: spaces and the rest percent-encoded,
/// and `http://` when it names no scheme, as Postman takes `localhost:3000/x`.
fn address(url: &str) -> Result<String, Failed> {
    // A scheme is letters before the first `://` — not the `://` of an address in its query.
    let scheme = url.split_once("://").is_some_and(|(s, _)| s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')));
    let url = if scheme { url.to_string() } else { format!("http://{url}") };
    Url::parse(&url).map(String::from).map_err(|e| Failed(format!("{url} is not an address: {e}")))
}

fn run(agent: &ureq::Agent, request: Result<Request<impl AsSendBody>, ureq::http::Error>, timeout: Duration) -> Result<Response<Body>, Failed> {
    let request = request.map_err(|e| format!("the request does not go out as written: {e}"))?;
    let request = agent
        .configure_request(request)
        .max_redirects(MAX_REDIRECTS)
        .save_redirect_history(true)
        .timeout_global(Some(timeout))
        // `PROPFIND`, `PURGE`: Postman sends whatever method is typed.
        .allow_non_standard_methods(true)
        .build();
    agent.run(request).map_err(|e| Failed(described(e, timeout)))
}

fn described(e: ureq::Error, timeout: Duration) -> String {
    match e {
        ureq::Error::Timeout(_) => format!("no response in {} s", timeout.as_secs_f32()),
        e => e.to_string(),
    }
}

fn too_large(length: Option<u64>) -> String {
    // Rounded up: a byte over 10 MB reads 10.1, not the limit itself.
    let mb = |n: u64| (n.saturating_mul(10).div_ceil(1 << 20)) as f64 / 10.0;
    match length {
        Some(n) => format!("the response is {:.1} MB, more than the {} MB the page takes", mb(n), BODY_LIMIT >> 20),
        None => format!("the response is more than the {} MB the page takes", BODY_LIMIT >> 20),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    /// What reached the server: the request line and headers, then the body.
    pub(crate) struct Seen {
        pub(crate) head: String,
        pub(crate) body: Vec<u8>,
    }

    /// Answers each connection with the next of `replies`, a function of the
    /// path; reports what each request sent.
    pub(crate) fn serve(replies: Vec<Box<dyn Fn(u16) -> Vec<u8> + Send>>) -> (u16, mpsc::Receiver<Seen>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for (reply, socket) in replies.into_iter().zip(listener.incoming()) {
                let mut socket = socket.unwrap();
                let mut reader = BufReader::new(socket.try_clone().unwrap());
                let mut head = String::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                        break;
                    }
                    head.push_str(&line);
                }
                let length = head.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length: ").map(|n| n.trim().parse().unwrap())).unwrap_or(0);
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let _ = tx.send(Seen { head, body });
                let _ = socket.write_all(&reply(port));
            }
        });
        (port, rx)
    }

    pub(crate) fn reply(text: &str) -> Box<dyn Fn(u16) -> Vec<u8> + Send> {
        let text = text.to_string();
        Box::new(move |port| text.replace("{port}", &port.to_string()).into_bytes())
    }

    /// A port nothing listens on: a stand that is down.
    pub(crate) fn closed_port() -> u16 {
        TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
    }

    fn outgoing(method: &str, url: String, body: Option<&str>) -> Outgoing {
        Outgoing { method: method.into(), url, headers: vec![("X-Trace".into(), "t-1".into()), ("Content-Type".into(), "application/json".into())], body: body.map(String::from) }
    }

    #[test]
    fn the_method_headers_and_body_arrive_as_sent() {
        let (port, seen) = serve(vec![reply("HTTP/1.1 201 Created\r\ncontent-length: 7\r\nx-id: 42\r\nconnection: close\r\n\r\ncreated")]);
        let (outcome, body) = send(&outgoing("PATCH", format!("http://127.0.0.1:{port}/v1/orders?q=a b"), Some("{\"sum\": 0}")), None);
        // Before the server is asked: a request that never left would leave it waiting.
        let Outcome::Response { status, headers, url, redirects, .. } = outcome else { panic!("{outcome:?}") };
        let seen = seen.recv().unwrap();
        assert!(seen.head.starts_with("PATCH /v1/orders?q=a%20b HTTP/1.1\r\n"), "{}", seen.head);
        assert!(seen.head.contains("X-Trace: t-1\r\n") || seen.head.contains("x-trace: t-1\r\n"), "{}", seen.head);
        assert_eq!(seen.body, b"{\"sum\": 0}");
        assert_eq!((status, body.as_slice()), (201, &b"created"[..]));
        assert!(headers.contains(&("x-id".into(), "42".into())));
        assert_eq!(url, format!("http://127.0.0.1:{port}/v1/orders?q=a%20b"));
        assert!(redirects.is_empty());
    }

    #[test]
    fn a_get_without_a_body_sends_no_length_and_a_bare_host_is_http() {
        let (port, seen) = serve(vec![reply("HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")]);
        let (outcome, _) = send(&Outgoing { method: "GET".into(), url: format!("127.0.0.1:{port}/health"), headers: vec![], body: None }, None);
        assert!(matches!(outcome, Outcome::Response { status: 200, .. }), "{outcome:?}");
        let head = seen.recv().unwrap().head.to_ascii_lowercase();
        assert!(head.starts_with("get /health http/1.1"), "{head}");
        assert!(!head.contains("content-length"), "{head}");
    }

    #[test]
    fn an_address_without_a_scheme_is_http_even_with_one_in_its_query() {
        let taken = |url: &str| address(url).ok();
        assert_eq!(taken("localhost:3000/cb?next=http://x"), Some("http://localhost:3000/cb?next=http://x".into()));
        assert_eq!(taken("https://api/x"), Some("https://api/x".into()));
        assert_eq!(taken("svn+ssh://host/x"), Some("svn+ssh://host/x".into()));
    }

    #[test]
    fn a_redirect_is_followed_and_the_way_kept() {
        let (port, seen) = serve(vec![
            reply("HTTP/1.1 302 Found\r\nlocation: http://127.0.0.1:{port}/new\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"),
            reply("HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok"),
        ]);
        let mut request = outgoing("GET", format!("http://127.0.0.1:{port}/old"), None);
        request.headers.push(("Authorization".into(), "Bearer t0k".into()));
        let (outcome, body) = send(&request, None);
        let Outcome::Response { status, url, redirects, .. } = outcome else { panic!("{outcome:?}") };
        assert_eq!((status, body.as_slice()), (200, &b"ok"[..]));
        assert_eq!(url, format!("http://127.0.0.1:{port}/new"));
        assert_eq!(redirects, vec![format!("http://127.0.0.1:{port}/old")]);
        let (first, second) = (seen.recv().unwrap().head, seen.recv().unwrap().head);
        assert!(first.starts_with("GET /old") && first.to_ascii_lowercase().contains("authorization: bearer t0k"), "{first}");
        // The token stays with the address it was meant for.
        assert!(second.starts_with("GET /new") && !second.to_ascii_lowercase().contains("authorization"), "{second}");
    }

    #[test]
    fn any_typed_method_goes_out_and_the_wait_is_timed() {
        let (port, seen) = serve(vec![Box::new(|_| {
            std::thread::sleep(Duration::from_millis(150));
            b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_vec()
        })]);
        let (outcome, _) = send(&Outgoing { method: "PURGE".into(), url: format!("http://127.0.0.1:{port}/cache"), headers: vec![], body: None }, None);
        let Outcome::Response { elapsed_ms, .. } = outcome else { panic!("{outcome:?}") };
        assert!(seen.recv().unwrap().head.starts_with("PURGE /cache HTTP/1.1"));
        assert!((150..5000).contains(&elapsed_ms), "{elapsed_ms}");
    }

    #[test]
    fn a_500_is_a_response_not_an_error() {
        let (port, _seen) = serve(vec![reply("HTTP/1.1 500 Internal Server Error\r\ncontent-length: 4\r\nconnection: close\r\n\r\nboom")]);
        let (outcome, body) = send(&outgoing("GET", format!("http://127.0.0.1:{port}/"), None), None);
        assert!(matches!(outcome, Outcome::Response { status: 500, .. }), "{outcome:?}");
        assert_eq!(body, b"boom");
    }

    #[test]
    fn a_stand_that_does_not_answer_times_out() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        // Accepted and never answered.
        std::thread::spawn(move || {
            let held = listener.accept();
            std::thread::sleep(Duration::from_secs(2));
            drop(held);
        });
        let started = Instant::now();
        let (outcome, body) = send_within(&outgoing("GET", format!("http://127.0.0.1:{port}/"), None), None, Duration::from_millis(300));
        assert_eq!(outcome, Outcome::Failed { error: "no response in 0.3 s".into() });
        assert!(body.is_empty());
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn a_body_over_the_limit_is_an_error_with_its_size() {
        let big = BODY_LIMIT + 1;
        let (port, _seen) = serve(vec![Box::new(move |_| {
            let mut r = format!("HTTP/1.1 200 OK\r\ncontent-length: {big}\r\nconnection: close\r\n\r\n").into_bytes();
            r.resize(r.len() + big as usize, b'a');
            r
        })]);
        let (outcome, _) = send(&outgoing("GET", format!("http://127.0.0.1:{port}/"), None), None);
        assert_eq!(outcome, Outcome::Failed { error: "the response is 10.1 MB, more than the 10 MB the page takes".into() });
        assert_eq!(too_large(Some(52 << 20)), "the response is 52.0 MB, more than the 10 MB the page takes");
        assert_eq!(too_large(None), "the response is more than the 10 MB the page takes");
    }

    #[test]
    fn a_body_at_the_limit_is_taken() {
        let size = BODY_LIMIT as usize;
        let (port, _seen) = serve(vec![Box::new(move |_| {
            let mut r = format!("HTTP/1.1 200 OK\r\ncontent-length: {size}\r\nconnection: close\r\n\r\n").into_bytes();
            r.resize(r.len() + size, b'a');
            r
        })]);
        let (outcome, body) = send(&outgoing("GET", format!("http://127.0.0.1:{port}/"), None), None);
        assert!(matches!(outcome, Outcome::Response { status: 200, .. }), "{outcome:?}");
        assert_eq!(body.len(), size);
    }

    #[test]
    fn a_stands_own_certificate_is_trusted_and_no_other_is() {
        use crate::infra::http_agent::tests::{serve_tls_with, CA, LEAF, LEAF_KEY};
        let port = serve_tls_with(LEAF, LEAF_KEY, "HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok");
        let request = outgoing("GET", format!("https://127.0.0.1:{port}/"), None);
        let (outcome, body) = send(&request, Some(CA));
        assert!(matches!(outcome, Outcome::Response { status: 200, .. }), "{outcome:?}");
        assert_eq!(body, b"ok");
        let (outcome, _) = send(&request, None);
        assert!(matches!(outcome, Outcome::Failed { .. }), "{outcome:?}");
    }

    #[test]
    fn what_cannot_go_out_says_why() {
        let (outcome, _) = send(&Outgoing { method: "GE T".into(), url: "http://127.0.0.1:9/".into(), headers: vec![], body: None }, None);
        assert_eq!(outcome, Outcome::Failed { error: "GE T is not an HTTP method".into() });
        let (outcome, _) = send(&Outgoing { method: "GET".into(), url: "http://[::1".into(), headers: vec![], body: None }, None);
        assert!(matches!(&outcome, Outcome::Failed { error } if error.contains("is not an address")), "{outcome:?}");
        let (outcome, _) = send(&Outgoing { method: "GET".into(), url: "http://127.0.0.1:9/".into(), headers: vec![("X\nY".into(), "1".into())], body: None }, None);
        assert!(matches!(&outcome, Outcome::Failed { error } if error.starts_with("the request does not go out as written")), "{outcome:?}");
    }
}
