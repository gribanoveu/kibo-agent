//! Sends the HTTP page's requests (`docs/45-http-client.md`, decision 3): the
//! one place they reach the network, over `http_agent`'s TLS, so a stand's own
//! certificate is trusted the way a provider's is.

use std::time::{Duration, Instant};

use ureq::http::{Method, Request, Response};
use ureq::{AsSendBody, Body};
use url::Url;

use super::http_agent::build_agent;
use crate::domain::http_auth::{digest_answer, is_digest};
use crate::domain::http_client::{CookieJar, Outcome, Outgoing};

/// The page waits this long for the whole of a response, redirects and body included.
pub const TIMEOUT: Duration = Duration::from_secs(30);
/// A larger body is refused with its size: the request keeps its response whole (decision 4).
pub const BODY_LIMIT: u64 = 10 << 20;
/// As Postman and `curl -L`: the page follows them and shows the way it went.
const MAX_REDIRECTS: u32 = 10;

/// What came back, and its body. A failure is an outcome too: a stand that
/// does not answer is what the user sent the request to find out.
pub fn send(request: &Outgoing, trusted_cert: Option<&str>, jar: &dyn CookieJar) -> (Outcome, Vec<u8>) {
    send_within(request, trusted_cert, jar, TIMEOUT)
}

/// For the agent: the one address it was given, and no session. Where a redirect
/// leads is for the model to read, not for a stand to take the request there.
pub fn send_once(request: &Outgoing, trusted_cert: Option<&str>) -> (Outcome, Vec<u8>) {
    settled(exchange(request, trusted_cert, &NoCookies, TIMEOUT, false))
}

fn send_within(request: &Outgoing, trusted_cert: Option<&str>, jar: &dyn CookieJar, timeout: Duration) -> (Outcome, Vec<u8>) {
    settled(exchange(request, trusted_cert, jar, timeout, true))
}

fn settled(exchanged: Result<(Outcome, Vec<u8>), Failed>) -> (Outcome, Vec<u8>) {
    exchanged.unwrap_or_else(|Failed(error)| (Outcome::Failed { error }, Vec::new()))
}

/// For what is not the user's session: a token request takes and leaves no cookies.
pub struct NoCookies;

impl CookieJar for NoCookies {
    fn header_for(&self, _url: &str) -> Option<String> {
        None
    }

    fn take(&self, _url: &str, _set_cookies: &[String]) {}
}

/// Why nothing came back, in the words `Outcome::Failed` shows.
struct Failed(String);

impl From<String> for Failed {
    fn from(why: String) -> Failed {
        Failed(why)
    }
}

/// One request on the way: redirects and a digest challenge each send the next.
struct Hop<'a> {
    method: Method,
    url: String,
    headers: Vec<(String, String)>,
    body: Option<&'a str>,
}

impl Hop<'_> {
    /// Cookies go per hop, so a login's redirect takes what the login just set.
    fn send(&self, agent: &ureq::Agent, jar: &dyn CookieJar, left: Duration, timeout: Duration) -> Result<Response<Body>, Failed> {
        let mut builder = Request::builder().method(self.method.clone()).uri(&self.url);
        let mut from_jar = jar.header_for(&self.url);
        for (name, value) in &self.headers {
            builder = match from_jar.take_if(|_| name.eq_ignore_ascii_case("cookie")) {
                Some(more) => builder.header(name, format!("{value}; {more}")),
                None => builder.header(name, value),
            };
        }
        if let Some(cookies) = from_jar {
            builder = builder.header("Cookie", cookies);
        }
        // No body is no body: an empty one would still send `Content-Length: 0` with a GET.
        match self.body {
            Some(body) => run(agent, builder.body(body.as_bytes()), left, timeout),
            None => run(agent, builder.body(()), left, timeout),
        }
    }

    /// On to `url`, as `curl -L` and Postman go: a 307 or 308 repeats the request, the rest
    /// turn it into a GET. `Authorization` and a `Cookie` written by hand stay on their origin.
    fn follow(&mut self, status: u16, url: String) {
        let moved = !same_origin(&self.url, &url);
        let drop_body = !matches!(status, 307 | 308) && self.method != Method::HEAD;
        if drop_body {
            self.method = Method::GET;
            self.body = None;
        }
        let gone = |name: &str| {
            (moved && (name.eq_ignore_ascii_case("authorization") || name.eq_ignore_ascii_case("cookie"))) || name.eq_ignore_ascii_case("content-length") || (drop_body && name.eq_ignore_ascii_case("content-type"))
        };
        self.headers.retain(|(name, _)| !gone(name));
        self.url = url;
    }
}

/// Scheme, host and port: how far what was written for an address goes.
fn same_origin(a: &str, b: &str) -> bool {
    matches!((Url::parse(a), Url::parse(b)), (Ok(a), Ok(b)) if a.origin() == b.origin())
}

/// Where a redirect goes, when `response` is one.
fn redirect_of(response: &Response<Body>, from: &str) -> Result<Option<String>, Failed> {
    if !matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
        return Ok(None);
    }
    let Some(location) = response.headers().get("location").and_then(|v| v.to_str().ok()) else { return Ok(None) };
    let base = Url::parse(from).map_err(|e| Failed(e.to_string()))?;
    let next = base.join(location).map_err(|e| Failed(format!("the redirect to {location} is not an address: {e}")))?;
    Ok(Some(next.into()))
}

/// `Authorization` for a 401 that asks for digest; only the address the user sent it to gets one.
fn digest_for(request: &Outgoing, hop: &Hop, first: &str, response: &Response<Body>) -> Option<String> {
    let (username, password) = request.digest.as_ref()?;
    if response.status().as_u16() != 401 {
        return None;
    }
    if !same_origin(&hop.url, first) {
        return None;
    }
    let here = Url::parse(&hop.url).ok()?;
    let challenge = response.headers().get_all("www-authenticate").iter().filter_map(|v| v.to_str().ok()).find(|v| is_digest(v))?;
    let uri = match here.query() {
        Some(query) => format!("{}?{query}", here.path()),
        None => here.path().to_string(),
    };
    digest_answer(challenge, username, password, hop.method.as_str(), &uri, &uuid::Uuid::new_v4().simple().to_string())
}

fn exchange(request: &Outgoing, trusted_cert: Option<&str>, jar: &dyn CookieJar, timeout: Duration, follow: bool) -> Result<(Outcome, Vec<u8>), Failed> {
    let started = Instant::now();
    let agent = build_agent(trusted_cert).map_err(|e| e.to_string())?;
    let method = Method::from_bytes(request.method.as_bytes()).map_err(|_| format!("{} is not an HTTP method", request.method))?;
    let first = address(&request.url)?;
    let mut hop = Hop { method, url: first.clone(), headers: request.headers.clone(), body: request.body.as_deref() };
    let mut redirects = Vec::new();
    let mut answered = false;
    // ureq would follow redirects itself, but its jar forgets a session cookie when
    // saved and names no cookie's domain, and the jar here must do both.
    let mut response = loop {
        let left = timeout.checked_sub(started.elapsed()).ok_or_else(|| no_response(timeout))?;
        let response = hop.send(&agent, jar, left, timeout)?;
        let set_cookies: Vec<String> = response.headers().get_all("set-cookie").iter().filter_map(|v| v.to_str().ok()).map(str::to_string).collect();
        if !set_cookies.is_empty() {
            jar.take(&hop.url, &set_cookies);
        }
        if let Some(answer) = digest_for(request, &hop, &first, &response).filter(|_| !answered) {
            // Once per address: a second 401 is the wrong password, and the user's to see.
            answered = true;
            // A redirect on the same host brought the last answer along, and it names another URI.
            hop.headers.retain(|(name, _)| !name.eq_ignore_ascii_case("authorization"));
            hop.headers.push(("Authorization".into(), answer));
            continue;
        }
        let next = if follow { redirect_of(&response, &hop.url)? } else { None };
        let Some(next) = next else { break response };
        let status = response.status().as_u16();
        // A body can hold a filled-in secret, and the stand names this address: the 307 is the user's to read.
        if hop.body.is_some() && matches!(status, 307 | 308) && !same_origin(&hop.url, &next) {
            break response;
        }
        if redirects.len() as u32 == MAX_REDIRECTS {
            return Err(Failed(format!("more than {MAX_REDIRECTS} redirects")));
        }
        redirects.push(hop.url.clone());
        hop.follow(status, next);
        answered = false;
    };

    let status = response.status().as_u16();
    let headers = response.headers().iter().map(|(k, v)| (k.to_string(), String::from_utf8_lossy(v.as_bytes()).into_owned())).collect();
    let url = hop.url;
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
    let url = crate::domain::http_client::with_scheme(url);
    Url::parse(&url).map(String::from).map_err(|e| Failed(format!("{url} is not an address: {e}")))
}

fn run(agent: &ureq::Agent, request: Result<Request<impl AsSendBody>, ureq::http::Error>, left: Duration, timeout: Duration) -> Result<Response<Body>, Failed> {
    let request = request.map_err(|e| format!("the request does not go out as written: {e}"))?;
    let request = agent
        .configure_request(request)
        .max_redirects(0)
        .max_redirects_will_error(false)
        .timeout_global(Some(left))
        // `PROPFIND`, `PURGE`: Postman sends whatever method is typed.
        .allow_non_standard_methods(true)
        .build();
    agent.run(request).map_err(|e| Failed(described(e, timeout)))
}

fn described(e: ureq::Error, timeout: Duration) -> String {
    match e {
        ureq::Error::Timeout(_) => no_response(timeout).0,
        e => e.to_string(),
    }
}

fn no_response(timeout: Duration) -> Failed {
    Failed(format!("no response in {} s", timeout.as_secs_f32()))
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
        Outgoing { method: method.into(), url, headers: vec![("X-Trace".into(), "t-1".into()), ("Content-Type".into(), "application/json".into())], body: body.map(String::from), digest: None }
    }

    /// Shadows the real one: most of these are about anything but cookies.
    fn send(request: &Outgoing, trusted_cert: Option<&str>) -> (Outcome, Vec<u8>) {
        super::send(request, trusted_cert, &NoCookies)
    }

    /// Remembers every `Set-Cookie` and hands `sends` back, as a jar does.
    #[derive(Default)]
    struct Jar {
        sends: Option<String>,
        taken: std::sync::Mutex<Vec<(String, Vec<String>)>>,
        asked: std::sync::Mutex<Vec<String>>,
    }

    impl CookieJar for Jar {
        fn header_for(&self, url: &str) -> Option<String> {
            self.asked.lock().unwrap().push(url.to_string());
            self.sends.clone()
        }

        fn take(&self, url: &str, set_cookies: &[String]) {
            self.taken.lock().unwrap().push((url.to_string(), set_cookies.to_vec()));
        }
    }

    const OK: &str = "HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok";

    fn redirect(status: &str, to: &str) -> String {
        format!("HTTP/1.1 {status}\r\nlocation: {to}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
    }

    #[test]
    fn a_login_s_redirect_takes_the_cookie_it_set_and_each_hop_is_asked() {
        let (port, seen) = serve(vec![
            reply("HTTP/1.1 302 Found\r\nlocation: /home\r\nset-cookie: sid=1; Path=/\r\nset-cookie: a=2\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"),
            reply(OK),
        ]);
        let jar = Jar { sends: Some("sid=1".into()), ..Jar::default() };
        let (outcome, _) = super::send(&outgoing("POST", format!("http://127.0.0.1:{port}/login"), Some("{}")), None, &jar);
        assert!(matches!(outcome, Outcome::Response { status: 200, .. }), "{outcome:?}");
        let login = format!("http://127.0.0.1:{port}/login");
        assert_eq!(*jar.taken.lock().unwrap(), vec![(login.clone(), vec!["sid=1; Path=/".to_string(), "a=2".to_string()])]);
        assert_eq!(*jar.asked.lock().unwrap(), vec![login, format!("http://127.0.0.1:{port}/home")]);
        assert!(seen.recv().unwrap().head.to_ascii_lowercase().contains("cookie: sid=1"));
        assert!(seen.recv().unwrap().head.to_ascii_lowercase().contains("cookie: sid=1"));
    }

    #[test]
    fn a_cookie_belongs_to_the_hop_that_set_it() {
        let (port, _seen) = serve(vec![reply(&redirect("302 Found", "/home")), reply("HTTP/1.1 200 OK\r\nset-cookie: sid=1\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")]);
        let jar = Jar::default();
        super::send(&outgoing("GET", format!("http://127.0.0.1:{port}/login"), None), None, &jar);
        let taken = jar.taken.lock().unwrap();
        assert_eq!(taken.last(), Some(&(format!("http://127.0.0.1:{port}/home"), vec!["sid=1".to_string()])));
    }

    #[test]
    fn what_was_written_by_hand_stays_on_its_origin() {
        let (other, elsewhere) = serve(vec![reply(OK)]);
        let (port, seen) = serve(vec![reply(&redirect("302 Found", "/next")), reply(&redirect("302 Found", &format!("http://127.0.0.1:{other}/")))]);
        let mut request = outgoing("GET", format!("http://127.0.0.1:{port}/"), None);
        request.headers.push(("Cookie".into(), "mine=1".into()));
        request.headers.push(("Authorization".into(), "Bearer t0k".into()));
        let jar = Jar { sends: Some("sid=2".into()), ..Jar::default() };
        super::send(&request, None, &jar);
        let head = |seen: &std::sync::mpsc::Receiver<Seen>| seen.recv().unwrap().head.to_ascii_lowercase();
        let (first, second, third) = (head(&seen), head(&seen), head(&elsewhere));
        assert!(first.contains("cookie: mine=1; sid=2") && first.matches("cookie:").count() == 1, "{first}");
        assert!(second.contains("cookie: mine=1; sid=2") && second.contains("authorization: bearer t0k"), "{second}");
        assert!(third.contains("cookie: sid=2") && !third.contains("mine") && !third.contains("authorization"), "{third}");
    }

    #[test]
    fn a_307_does_not_take_a_body_to_another_origin() {
        let (other, elsewhere) = serve(vec![reply(OK)]);
        let to_other = redirect("307 Temporary Redirect", &format!("http://127.0.0.1:{other}/"));
        let (port, _seen) = serve(vec![reply(&to_other), reply(&to_other)]);
        let (outcome, _) = send(&outgoing("POST", format!("http://127.0.0.1:{port}/"), Some("{\"password\": \"hunter2\"}")), None);
        assert!(matches!(&outcome, Outcome::Response { status: 307, redirects, .. } if redirects.is_empty()), "{outcome:?}");
        assert!(elsewhere.recv_timeout(Duration::from_millis(100)).is_err(), "the body stays home");
        let (outcome, _) = send(&outgoing("GET", format!("http://127.0.0.1:{port}/"), None), None);
        assert!(matches!(outcome, Outcome::Response { status: 200, .. }), "no body, nothing to keep: {outcome:?}");
    }

    #[test]
    fn a_length_written_by_hand_does_not_outlive_its_body() {
        let (port, seen) = serve(vec![reply(&redirect("302 Found", "/next")), reply(OK)]);
        let mut request = outgoing("POST", format!("http://127.0.0.1:{port}/"), Some("{}"));
        request.headers.push(("Content-Length".into(), "2".into()));
        send(&request, None);
        seen.recv().unwrap();
        let next = seen.recv().unwrap().head.to_ascii_lowercase();
        assert!(next.starts_with("get /next") && !next.contains("content-length"), "{next}");
    }

    #[test]
    fn a_redirect_turns_a_post_into_a_get_as_curl_does() {
        for status in ["301 Moved", "302 Found", "303 See Other"] {
            let (port, seen) = serve(vec![reply(&redirect(status, "/next")), reply(OK)]);
            send(&outgoing("POST", format!("http://127.0.0.1:{port}/"), Some("{\"a\":1}")), None);
            seen.recv().unwrap();
            let next = seen.recv().unwrap();
            assert!(next.head.starts_with("GET /next"), "{status}: {}", next.head);
            assert!(next.body.is_empty() && !next.head.to_ascii_lowercase().contains("content-type"), "{status}: {}", next.head);
            assert!(next.head.contains("X-Trace") || next.head.contains("x-trace"), "the rest of the headers stay: {}", next.head);
        }
    }

    #[test]
    fn a_307_and_a_308_repeat_the_request_body_and_all() {
        for status in ["307 Temporary Redirect", "308 Permanent Redirect"] {
            let (port, seen) = serve(vec![reply(&redirect(status, "/next")), reply(OK)]);
            let (outcome, _) = send(&outgoing("POST", format!("http://127.0.0.1:{port}/"), Some("{\"a\":1}")), None);
            assert!(matches!(outcome, Outcome::Response { status: 200, .. }), "{outcome:?}");
            seen.recv().unwrap();
            let next = seen.recv().unwrap();
            assert!(next.head.starts_with("POST /next") && next.head.to_ascii_lowercase().contains("content-type: application/json"), "{}", next.head);
            assert_eq!(next.body, b"{\"a\":1}");
        }
    }

    #[test]
    fn a_head_stays_a_head_through_a_redirect() {
        let (port, seen) = serve(vec![reply(&redirect("302 Found", "/next")), reply("HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")]);
        send(&outgoing("HEAD", format!("http://127.0.0.1:{port}/"), None), None);
        seen.recv().unwrap();
        assert!(seen.recv().unwrap().head.starts_with("HEAD /next"));
    }

    #[test]
    fn redirects_end_at_their_limit_and_a_3xx_with_nowhere_to_go_is_an_answer() {
        let loops: Vec<_> = (0..=MAX_REDIRECTS).map(|_| reply(&redirect("302 Found", "/again"))).collect();
        let (port, seen) = serve(loops);
        let (outcome, _) = send(&outgoing("GET", format!("http://127.0.0.1:{port}/"), None), None);
        assert_eq!(outcome, Outcome::Failed { error: "more than 10 redirects".into() });
        assert_eq!(seen.iter().count(), MAX_REDIRECTS as usize + 1, "the request and ten hops");

        let (port, _seen) = serve(vec![reply("HTTP/1.1 302 Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")]);
        let (outcome, _) = send(&outgoing("GET", format!("http://127.0.0.1:{port}/"), None), None);
        assert!(matches!(outcome, Outcome::Response { status: 302, .. }), "{outcome:?}");

        let (port, _seen) = serve(vec![reply(&redirect("304 Not Modified", "/never"))]);
        let (outcome, _) = send(&outgoing("GET", format!("http://127.0.0.1:{port}/"), None), None);
        assert!(matches!(outcome, Outcome::Response { status: 304, .. }), "{outcome:?}");
    }

    #[test]
    fn the_wait_covers_the_whole_way_not_each_hop() {
        let slow = |text: String| -> Box<dyn Fn(u16) -> Vec<u8> + Send> {
            Box::new(move |_| {
                std::thread::sleep(Duration::from_millis(250));
                text.clone().into_bytes()
            })
        };
        let (port, _seen) = serve(vec![slow(redirect("302 Found", "/2")), slow(redirect("302 Found", "/3")), slow(OK.to_string())]);
        let (outcome, _) = send_within(&outgoing("GET", format!("http://127.0.0.1:{port}/"), None), None, &NoCookies, Duration::from_millis(600));
        assert_eq!(outcome, Outcome::Failed { error: "no response in 0.6 s".into() });
    }

    const CHALLENGE: &str = "HTTP/1.1 401 Unauthorized\r\nwww-authenticate: Basic realm=\"x\"\r\nwww-authenticate: Digest realm=\"probe\", nonce=\"n1\", qop=\"auth\"\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";

    fn digest_request(url: String) -> Outgoing {
        Outgoing { digest: Some(("digestuser".into(), "digestpass".into())), ..outgoing("POST", url, Some("{}")) }
    }

    fn authorization(head: &str) -> Option<&str> {
        head.lines().find_map(|l| l.strip_prefix("authorization: ").or_else(|| l.strip_prefix("Authorization: "))).map(str::trim)
    }

    #[test]
    fn digest_answers_a_challenge_with_the_request_repeated() {
        let (port, seen) = serve(vec![reply(CHALLENGE), reply(OK)]);
        let (outcome, body) = send(&digest_request(format!("http://127.0.0.1:{port}/a/b?q=1")), None);
        assert!(matches!(outcome, Outcome::Response { status: 200, .. }), "{outcome:?}");
        assert_eq!(body, b"ok");
        let (first, second) = (seen.recv().unwrap(), seen.recv().unwrap());
        assert_eq!(authorization(&first.head), None, "as wire 08: the first goes without");
        let answer = authorization(&second.head).unwrap();
        assert!(answer.starts_with("Digest username=\"digestuser\", realm=\"probe\", nonce=\"n1\", uri=\"/a/b?q=1\""), "{answer}");
        // POST: the method the request has, in the hash.
        let cnonce = answer.split("cnonce=\"").nth(1).unwrap().split('"').next().unwrap();
        let again = digest_answer("Digest realm=\"probe\", nonce=\"n1\", qop=\"auth\"", "digestuser", "digestpass", "POST", "/a/b?q=1", cnonce).unwrap();
        assert_eq!(answer, again);
        assert_eq!(second.body, b"{}", "the body goes again");
    }

    #[test]
    fn a_wrong_password_is_answered_once_and_shown() {
        let (port, seen) = serve(vec![reply(CHALLENGE), reply(CHALLENGE), reply(OK)]);
        let (outcome, _) = send(&digest_request(format!("http://127.0.0.1:{port}/")), None);
        assert!(matches!(outcome, Outcome::Response { status: 401, .. }), "{outcome:?}");
        seen.recv().unwrap();
        seen.recv().unwrap();
        assert!(seen.recv_timeout(Duration::from_millis(100)).is_err(), "no third try");
    }

    #[test]
    fn a_401_is_answered_only_when_it_asks_for_digest_and_digest_is_set() {
        let (port, seen) = serve(vec![reply("HTTP/1.1 401 Unauthorized\r\nwww-authenticate: Basic realm=\"x\"\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"), reply(OK)]);
        let (outcome, _) = send(&digest_request(format!("http://127.0.0.1:{port}/")), None);
        assert!(matches!(outcome, Outcome::Response { status: 401, .. }), "{outcome:?}");
        seen.recv().unwrap();
        assert!(seen.recv_timeout(Duration::from_millis(100)).is_err());

        let (port, seen) = serve(vec![reply(CHALLENGE), reply(OK)]);
        let (outcome, _) = send(&outgoing("GET", format!("http://127.0.0.1:{port}/"), None), None);
        assert!(matches!(outcome, Outcome::Response { status: 401, .. }), "{outcome:?}");
        seen.recv().unwrap();
        assert!(seen.recv_timeout(Duration::from_millis(100)).is_err());

        let (port, seen) = serve(vec![reply("HTTP/1.1 403 Forbidden\r\nwww-authenticate: Digest realm=\"r\", nonce=\"n\"\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"), reply(OK)]);
        send(&digest_request(format!("http://127.0.0.1:{port}/")), None);
        seen.recv().unwrap();
        assert!(seen.recv_timeout(Duration::from_millis(100)).is_err(), "only a 401 asks");
    }

    #[test]
    fn another_host_gets_no_digest_answer() {
        let (other, other_seen) = serve(vec![reply(CHALLENGE), reply(OK)]);
        let (port, seen) = serve(vec![reply(&redirect("302 Found", &format!("http://127.0.0.1:{other}/trap")))]);
        let (outcome, _) = send(&digest_request(format!("http://127.0.0.1:{port}/")), None);
        assert!(matches!(outcome, Outcome::Response { status: 401, .. }), "{outcome:?}");
        seen.recv().unwrap();
        other_seen.recv().unwrap();
        assert!(other_seen.recv_timeout(Duration::from_millis(100)).is_err(), "the hash of the password stays home");
    }

    #[test]
    fn digest_answers_where_a_redirect_on_its_own_host_ended() {
        let (port, seen) = serve(vec![reply(&redirect("302 Found", "/in")), reply(CHALLENGE), reply(OK)]);
        let (outcome, _) = send(&digest_request(format!("http://127.0.0.1:{port}/")), None);
        assert!(matches!(outcome, Outcome::Response { status: 200, .. }), "{outcome:?}");
        seen.recv().unwrap();
        seen.recv().unwrap();
        let third = seen.recv().unwrap();
        assert!(third.head.starts_with("GET /in") && authorization(&third.head).unwrap().contains("uri=\"/in\""), "{}", third.head);
    }

    #[test]
    fn digest_answers_each_address_a_redirect_reaches() {
        let (port, seen) = serve(vec![reply(CHALLENGE), reply(&redirect("301 Moved Permanently", "/dir/")), reply(CHALLENGE), reply(OK)]);
        let (outcome, _) = send(&digest_request(format!("http://127.0.0.1:{port}/dir")), None);
        assert!(matches!(outcome, Outcome::Response { status: 200, .. }), "{outcome:?}");
        let heads: Vec<String> = (0..4).map(|_| seen.recv().unwrap().head).collect();
        assert!(authorization(&heads[3]).unwrap().contains("uri=\"/dir/\""), "{}", heads[3]);
        assert_eq!(heads[3].to_ascii_lowercase().matches("authorization:").count(), 1, "{}", heads[3]);
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
        let (outcome, _) = send(&Outgoing { method: "GET".into(), url: format!("127.0.0.1:{port}/health"), headers: vec![], body: None, digest: None }, None);
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
        // Its own origin: a stand that only adds a slash has not left it.
        assert!(second.starts_with("GET /new") && second.to_ascii_lowercase().contains("authorization: bearer t0k"), "{second}");
    }

    #[test]
    fn any_typed_method_goes_out_and_the_wait_is_timed() {
        let (port, seen) = serve(vec![Box::new(|_| {
            std::thread::sleep(Duration::from_millis(150));
            b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_vec()
        })]);
        let (outcome, _) = send(&Outgoing { method: "PURGE".into(), url: format!("http://127.0.0.1:{port}/cache"), headers: vec![], body: None, digest: None }, None);
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
        let (outcome, body) = send_within(&outgoing("GET", format!("http://127.0.0.1:{port}/"), None), None, &NoCookies, Duration::from_millis(300));
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
        let (outcome, _) = send(&Outgoing { method: "GE T".into(), url: "http://127.0.0.1:9/".into(), headers: vec![], body: None, digest: None }, None);
        assert_eq!(outcome, Outcome::Failed { error: "GE T is not an HTTP method".into() });
        let (outcome, _) = send(&Outgoing { method: "GET".into(), url: "http://[::1".into(), headers: vec![], body: None, digest: None }, None);
        assert!(matches!(&outcome, Outcome::Failed { error } if error.contains("is not an address")), "{outcome:?}");
        let (outcome, _) = send(&Outgoing { method: "GET".into(), url: "http://127.0.0.1:9/".into(), headers: vec![("X\nY".into(), "1".into())], body: None, digest: None }, None);
        assert!(matches!(&outcome, Outcome::Failed { error } if error.starts_with("the request does not go out as written")), "{outcome:?}");
    }

    #[test]
    fn the_agents_send_stays_where_it_was_sent_and_says_where_it_would_go() {
        let (port, seen) = serve(vec![reply("HTTP/1.1 302 Found\r\nlocation: /next\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"), reply("HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")]);
        let (outcome, _) = send_once(&outgoing("GET", format!("http://127.0.0.1:{port}/start"), None), None);
        let Outcome::Response { status, headers, redirects, .. } = outcome else { panic!("{outcome:?}") };
        assert_eq!(status, 302);
        assert!(headers.contains(&("location".to_string(), "/next".to_string())), "{headers:?}");
        assert!(redirects.is_empty());
        assert!(seen.recv().unwrap().head.starts_with("GET /start"));
        assert!(seen.recv_timeout(Duration::from_millis(200)).is_err(), "nothing went to /next");
    }

    /// The stand answered: a `Location` no address could be is for the model to read.
    #[test]
    fn a_bad_location_is_read_not_refused_when_nothing_follows_it() {
        let (port, _seen) = serve(vec![reply("HTTP/1.1 302 Found\r\nlocation: http://[bad\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")]);
        let (outcome, _) = send_once(&outgoing("GET", format!("http://127.0.0.1:{port}/"), None), None);
        assert!(matches!(outcome, Outcome::Response { status: 302, .. }), "{outcome:?}");
    }

    #[test]
    fn a_stand_down_is_an_outcome_for_the_agent_too() {
        let (outcome, _) = send_once(&outgoing("GET", format!("http://127.0.0.1:{}/", closed_port()), None), None);
        assert!(matches!(outcome, Outcome::Failed { .. }), "{outcome:?}");
    }
}
