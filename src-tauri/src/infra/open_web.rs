//! The web without a key (`docs/24-web-search.md`, `docs/08-data-policy.md`):
//! a search through the user's own SearXNG, and any public page read as
//! Markdown. What leaves is the query, to the SearXNG at the address the user
//! gave — and from there to whichever engines it is set up to ask — and a
//! plain `GET` to each page's own host.

use std::net::{IpAddr, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use url::Url;

use crate::domain::embeddings::EmbeddingProvider;
use crate::domain::web_search::{
    passages, PageQuery, TimeRange, PASSAGE_CHARS, Web, WebFetchFn, WebHit, WebPage, WebQuery, WebSearchError, WebSearchFn,
};
use crate::infra::http_agent::{self, TlsError};

/// Sites answer a browser; some answer nothing else.
const USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15";
/// SearXNG asks its engines side by side and gives up on a slow one itself;
/// this is for a SearXNG that is not answering at all.
const SEARCH_TIMEOUT: Duration = Duration::from_secs(20);
/// A page asked for by `webFetch`: the model wants this one, so it waits.
const PAGE_TIMEOUT: Duration = Duration::from_secs(10);
/// A page a search found: one slower than this keeps its snippet, and the
/// turn does not wait on the slowest site of ten — a search waited 11.6 s on one.
const SEARCH_PAGE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_PAGE_BYTES: u64 = 3 * 1024 * 1024;
const MAX_REDIRECTS: usize = 5;
/// Of each page found, what a search sends: about Tavily's three passages.
const SEARCH_PASSAGES_CHARS: usize = 1_500;

/// The search the user chose, and the page reader beside it.
pub fn web(search: WebSearchFn, model: Arc<dyn EmbeddingProvider>) -> Result<Web, TlsError> {
    Ok(Web { search, fetch: fetcher(model)? })
}

/// The search through the SearXNG at `base`: its results, each page read and
/// cut to its passages about the query. `base` is the user's own — localhost
/// is allowed here, as it is not for a page.
pub fn searxng(base: Url, model: Arc<dyn EmbeddingProvider>) -> Result<WebSearchFn, TlsError> {
    let agent = http_agent::build_agent(None)?;
    Ok(Arc::new(move |query: &WebQuery| search(&agent, &base, model.as_ref(), query, &public)))
}

fn fetcher(model: Arc<dyn EmbeddingProvider>) -> Result<WebFetchFn, TlsError> {
    let agent = http_agent::build_agent(None)?;
    Ok(Arc::new(move |query: &PageQuery| fetch(&agent, model.as_ref(), query)))
}

/// `allowed` checks each found page before it is read — [`public`], but for a test.
fn search(
    agent: &ureq::Agent,
    base: &Url,
    model: &dyn EmbeddingProvider,
    query: &WebQuery,
    allowed: &(dyn Fn(&Url) -> Result<(), WebSearchError> + Sync),
) -> Result<Vec<WebHit>, WebSearchError> {
    let endpoint = base.join("search").map_err(|e| WebSearchError::Unavailable(format!("{base}: {e}")))?;
    let mut request = agent.get(endpoint.as_str()).query("q", query.query).query("format", "json");
    if let Some(range) = query.time_range {
        request = request.query(
            "time_range",
            match range {
                TimeRange::Day => "day",
                TimeRange::Week => "week",
                TimeRange::Month => "month",
                TimeRange::Year => "year",
            },
        );
    }
    let mut response = request
        .config()
        .timeout_global(Some(SEARCH_TIMEOUT))
        .build()
        .call()
        .map_err(|e| WebSearchError::Unavailable(format!("SearXNG at {base} is not answering ({e}) — is it running?")))?;
    let status = response.status().as_u16();
    let body = response.body_mut().read_to_string().map_err(|e| WebSearchError::Unavailable(e.to_string()))?;
    let mut hits = parse_results(status, &body)?;
    hits.truncate(query.max_results as usize);
    // Side by side: the turn waits for the slowest page, not for all of them.
    let read: Vec<Option<String>> = std::thread::scope(|scope| {
        let running: Vec<_> =
            hits.iter().map(|hit| scope.spawn(move || read_checked(agent, &hit.url, SEARCH_PAGE_TIMEOUT, allowed).ok().map(|page| page.content))).collect();
        running.into_iter().map(|handle| handle.join().ok().flatten()).collect()
    });
    with_passages(&mut hits, read, query.query, model);
    Ok(hits)
}

/// Each page's passages about `query` after its snippet. The snippet stays
/// first: the engine chose it for the query, and it often holds the very
/// words asked about. A page that would not be read keeps the snippet alone.
fn with_passages(hits: &mut [WebHit], pages: Vec<Option<String>>, query: &str, model: &dyn EmbeddingProvider) {
    for (hit, text) in hits.iter_mut().zip(pages) {
        let budget = SEARCH_PASSAGES_CHARS.saturating_sub(hit.content.chars().count()).max(PASSAGE_CHARS);
        if let Some(found) = text.and_then(|text| passages(&text, query, model, budget)) {
            if !found.is_empty() {
                hit.content = if hit.content.is_empty() { found } else { format!("{}\n…\n{found}", hit.content) };
            }
        }
    }
}

#[derive(Deserialize)]
struct Answer {
    results: Vec<Found>,
    /// `[engine, why]` for each engine that gave nothing: `["duckduckgo", "CAPTCHA"]`.
    #[serde(default)]
    unresponsive_engines: Vec<(String, String)>,
}

#[derive(Deserialize)]
struct Found {
    #[serde(default)]
    title: String,
    url: String,
    #[serde(default)]
    content: Option<String>,
}

/// SearXNG's results in its order, web addresses only. No results because
/// every engine failed is said as such — with which, and why — rather than as
/// "nothing found", which would send the model looking with other words.
fn parse_results(status: u16, body: &str) -> Result<Vec<WebHit>, WebSearchError> {
    match status {
        200..=299 => {}
        // `json` is not in `search.formats` of its settings.yml.
        403 => {
            return Err(WebSearchError::Refused(
                "SearXNG does not answer in JSON — tell the user to add `json` to `search.formats` in its settings.yml".into(),
            ))
        }
        429 => return Err(WebSearchError::RateLimited),
        _ => return Err(WebSearchError::Unavailable(format!("SearXNG answered {status}"))),
    }
    let answer: Answer = serde_json::from_str(body).map_err(|e| WebSearchError::BadAnswer(e.to_string()))?;
    let hits: Vec<WebHit> = answer
        .results
        .into_iter()
        .filter(|found| Url::parse(&found.url).is_ok_and(|u| matches!(u.scheme(), "http" | "https")))
        .map(|found| WebHit { title: squash(&found.title), url: found.url, content: squash(&found.content.unwrap_or_default()) })
        .collect();
    if hits.is_empty() && !answer.unresponsive_engines.is_empty() {
        let failed: Vec<String> = answer.unresponsive_engines.iter().map(|(engine, why)| format!("{engine} ({why})")).collect();
        return Err(WebSearchError::Unavailable(format!(
            "every engine of SearXNG failed: {} — do not search again in this turn",
            failed.join(", ")
        )));
    }
    Ok(hits)
}

fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn fetch(agent: &ureq::Agent, model: &dyn EmbeddingProvider, query: &PageQuery) -> Result<WebPage, WebSearchError> {
    Ok(shape(read(agent, query.url, PAGE_TIMEOUT)?, query, model))
}

/// The page whole when it fits; otherwise its passages about the question, or
/// without one — or without the model — its beginning.
fn shape(page: WebPage, query: &PageQuery, model: &dyn EmbeddingProvider) -> WebPage {
    if page.content.chars().count() <= query.max_chars {
        return page;
    }
    let content = query
        .query
        .and_then(|asked| passages(&page.content, asked, model, query.max_chars))
        .unwrap_or_else(|| page.content.chars().take(query.max_chars).collect());
    WebPage { content, truncated: true, ..page }
}

fn read(agent: &ureq::Agent, url: &str, timeout: Duration) -> Result<WebPage, WebSearchError> {
    read_checked(agent, url, timeout, &public)
}

/// A page's text, following redirects one at a time so each address is
/// checked by `allowed` before it is asked.
fn read_checked(
    agent: &ureq::Agent,
    url: &str,
    timeout: Duration,
    allowed: &dyn Fn(&Url) -> Result<(), WebSearchError>,
) -> Result<WebPage, WebSearchError> {
    let mut at = Url::parse(url.trim()).map_err(|e| WebSearchError::NotAllowed(format!("{url} is not an address: {e}")))?;
    for _ in 0..=MAX_REDIRECTS {
        allowed(&at)?;
        let mut response = agent
            .get(at.as_str())
            .header("User-Agent", USER_AGENT)
            .config()
            .timeout_global(Some(timeout))
            .max_redirects(0)
            .build()
            .call()
            .map_err(|e| WebSearchError::PageUnavailable(e.to_string()))?;
        let status = response.status().as_u16();
        let header = |name: &str| response.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
        if (300..400).contains(&status) {
            let location = header("location").ok_or(WebSearchError::PageStatus(status))?;
            at = at.join(&location).map_err(|e| WebSearchError::NotAllowed(format!("a redirect to {location}: {e}")))?;
            continue;
        }
        if !(200..300).contains(&status) {
            return Err(WebSearchError::PageStatus(status));
        }
        let kind = header("content-type").unwrap_or_default().to_ascii_lowercase();
        let html = kind.is_empty() || kind.contains("html");
        if !html && !kind.starts_with("text/") {
            return Err(WebSearchError::NotReadable(format!("it is {kind}")));
        }
        let bytes = response
            .body_mut()
            .with_config()
            .limit(MAX_PAGE_BYTES)
            .read_to_vec()
            .map_err(|e| WebSearchError::PageUnavailable(e.to_string()))?;
        // ponytail: a page in another encoding than UTF-8 reads with � in it; ureq's `charset` feature if that matters.
        let body = String::from_utf8_lossy(&bytes);
        let (title, content) = if html { readable(&body, at.as_str()) } else { (String::new(), body.trim().to_string()) };
        if content.is_empty() {
            return Err(WebSearchError::NotReadable("the page is empty, or drawn by scripts".into()));
        }
        return Ok(WebPage { title, url: at.into(), content, truncated: false });
    }
    Err(WebSearchError::NotAllowed(format!("more than {MAX_REDIRECTS} redirects")))
}

/// The article as Markdown, as Firefox's reader view finds it; the whole
/// page's text when it finds none — an index, a list of releases.
fn readable(html: &str, url: &str) -> (String, String) {
    let config = dom_smoothie::Config { text_mode: dom_smoothie::TextMode::Markdown, ..Default::default() };
    let article = dom_smoothie::Readability::new(html, Some(url), Some(config)).and_then(|mut r| r.parse());
    match article {
        Ok(article) if !article.text_content.trim().is_empty() => {
            (squash(&article.title), unescape(article.text_content.trim()))
        }
        _ => {
            let document = dom_query::Document::from(html);
            // A page drawn by scripts (dzen.ru) has little but them in its body:
            // their code is not the page's text.
            document.select("script, style, noscript, template").remove();
            let lines: Vec<String> =
                document.select("body").text().lines().map(squash).filter(|line| !line.is_empty()).collect();
            (squash(&document.select("title").text()), lines.join("\n"))
        }
    }
}

/// The Markdown without its escapes: `2\.0\.1` and `\#7006` are 2.0.1 and #7006
/// to a model, at a token more each, and a model quoting them copies the
/// backslashes. Code blocks are left as they are — a backslash there is code.
fn unescape(markdown: &str) -> String {
    let mut fenced = false;
    let lines: Vec<String> = markdown
        .lines()
        .map(|line| {
            if line.trim_start().starts_with("```") {
                fenced = !fenced;
                return line.to_string();
            }
            if fenced {
                return line.to_string();
            }
            let mut out = String::with_capacity(line.len());
            let mut chars = line.chars().peekable();
            while let Some(c) = chars.next() {
                if c == '\\' && chars.peek().is_some_and(char::is_ascii_punctuation) {
                    continue;
                }
                out.push(c);
            }
            out
        })
        .collect();
    lines.join("\n")
}

/// The web, not the user's own network: a page — or a search result, or a
/// redirect — cannot send the model to the router, a cluster or a service on
/// localhost.
// ponytail: checked at resolution, not at connection — a host that resolves
// differently a second time (DNS rebinding) gets past it; pin the address in
// ureq's resolver if that ever matters.
fn public(url: &Url) -> Result<(), WebSearchError> {
    let refuse = |why: &str| Err(WebSearchError::NotAllowed(format!("{url} {why}")));
    if !matches!(url.scheme(), "http" | "https") {
        return refuse("is not a web address");
    }
    let (Some(host), Some(port)) = (url.host_str(), url.port_or_known_default()) else {
        return refuse("has no host");
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let addresses: Vec<IpAddr> = (host, port)
        .to_socket_addrs()
        .map_err(|e| WebSearchError::PageUnavailable(format!("{host}: {e}")))?
        .map(|a| a.ip())
        .collect();
    if addresses.is_empty() || !addresses.into_iter().all(is_public) {
        return refuse("is on a private network or this machine");
    }
    Ok(())
}

fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => {
            let [first, second, ..] = a.octets();
            !(a.is_private()
                || a.is_loopback()
                || a.is_link_local()
                || a.is_unspecified()
                || a.is_broadcast()
                || a.is_documentation()
                || a.is_multicast()
                || first == 0
                // Carrier-grade NAT: Tailscale's addresses among them.
                || (first == 100 && (second & 0xc0) == 64))
        }
        IpAddr::V6(a) => match a.to_ipv4_mapped() {
            Some(v4) => is_public(IpAddr::V4(v4)),
            None => {
                let first = a.segments()[0];
                !(a.is_loopback()
                    || a.is_unspecified()
                    || a.is_multicast()
                    || (first & 0xfe00) == 0xfc00
                    || (first & 0xffc0) == 0xfe80)
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RESULTS: &str = r#"{"query":"tauri","number_of_results":0,"results":[
        {"url":"https://v2.tauri.app/release/","title":"Tauri  Ecosystem\nReleases","content":"Release notes   for every package","engine":"google cse","score":1.0},
        {"url":"https://v2.tauri.app/blog/tauri-20/","title":"Tauri 2.0","content":null},
        {"url":"javascript:alert(1)","title":"not a page"},
        {"url":"https://github.com/tauri-apps/tauri/releases","title":"Releases"}
    ],"answers":[],"infoboxes":[],"suggestions":[],"unresponsive_engines":[["duckduckgo","CAPTCHA"]]}"#;

    fn hit(title: &str, url: &str, content: &str) -> WebHit {
        WebHit { title: title.into(), url: url.into(), content: content.into() }
    }

    #[test]
    fn results_are_read_in_order_web_addresses_only() {
        assert_eq!(
            parse_results(200, RESULTS).unwrap(),
            vec![
                hit("Tauri Ecosystem Releases", "https://v2.tauri.app/release/", "Release notes for every package"),
                hit("Tauri 2.0", "https://v2.tauri.app/blog/tauri-20/", ""),
                hit("Releases", "https://github.com/tauri-apps/tauri/releases", ""),
            ]
        );
    }

    #[test]
    fn no_results_is_nothing_found_unless_every_engine_failed() {
        let none = r#"{"results":[],"unresponsive_engines":[]}"#;
        assert_eq!(parse_results(200, none).unwrap(), vec![]);
        let failed = r#"{"results":[],"unresponsive_engines":[["duckduckgo","CAPTCHA"],["brave","too many requests"]]}"#;
        let error = parse_results(200, failed).unwrap_err();
        assert!(
            matches!(&error, WebSearchError::Unavailable(why) if why.contains("duckduckgo (CAPTCHA), brave (too many requests)")),
            "{error:?}"
        );
    }

    #[test]
    fn statuses_say_what_to_fix() {
        assert!(matches!(parse_results(403, "Forbidden"), Err(WebSearchError::Refused(why)) if why.contains("search.formats")));
        assert!(matches!(parse_results(429, ""), Err(WebSearchError::RateLimited)));
        assert!(matches!(parse_results(502, ""), Err(WebSearchError::Unavailable(_))));
        assert!(matches!(parse_results(200, "<html>"), Err(WebSearchError::BadAnswer(_))));
    }

    /// Serves each request with the next of `answers`, as raw HTTP, on a port
    /// of this machine; the address to start from.
    fn serve(answers: Vec<String>) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for answer in answers {
                let Ok((mut stream, _)) = listener.accept() else { return };
                let mut request = [0u8; 4096];
                let _ = stream.read(&mut request);
                let _ = stream.write_all(answer.as_bytes());
            }
        });
        address
    }

    fn answer(head: &str, body: &str) -> String {
        format!("HTTP/1.1 {head}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
    }

    /// This machine stands in for the web; `/private` for the user's network.
    fn not_private(url: &Url) -> Result<(), WebSearchError> {
        match url.path() {
            "/private" => Err(WebSearchError::NotAllowed(url.to_string())),
            _ => Ok(()),
        }
    }

    fn agent() -> ureq::Agent {
        http_agent::build_agent(None).unwrap()
    }

    /// SearXNG itself may be on this machine — the user said so. A page it
    /// found there is not read: the search answers with its snippet.
    #[test]
    fn searxng_on_localhost_is_asked_but_a_page_on_this_machine_is_not_read() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let results = format!(r#"{{"results":[{{"url":"{address}/admin","title":"Router","content":"snippet"}}],"unresponsive_engines":[]}}"#);
        let answers = vec![
            answer("200 OK\r\nContent-Type: application/json", &results),
            answer("200 OK\r\nContent-Type: text/plain", "the router's admin page"),
        ];
        std::thread::spawn(move || {
            for answer in answers {
                let Ok((mut stream, _)) = listener.accept() else { return };
                let _ = stream.read(&mut [0u8; 4096]);
                let _ = stream.write_all(answer.as_bytes());
            }
        });
        let search = searxng(Url::parse(&format!("{address}/")).unwrap(), Arc::new(NoModel)).unwrap();
        let hits = search(&WebQuery { query: "router", max_results: 5, time_range: None }).unwrap();
        assert_eq!(hits, vec![hit("Router", &format!("{address}/admin"), "snippet")]);
    }

    #[test]
    fn the_query_and_its_period_reach_searxng_and_its_pages_are_read() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let base = Url::parse(&format!("{address}/")).unwrap();
        let results = format!(
            r#"{{"results":[{{"url":"{address}/page","title":"Read","content":"snippet"}},{{"url":"{address}/private","title":"Not read","content":"its snippet"}}],"unresponsive_engines":[]}}"#
        );
        let answers = vec![
            answer("200 OK\r\nContent-Type: application/json", &results),
            answer("200 OK\r\nContent-Type: text/plain", "the page's own words"),
        ];
        let asked = std::thread::spawn(move || {
            let mut first = String::new();
            for answer in answers {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0u8; 4096];
                let n = stream.read(&mut request).unwrap();
                if first.is_empty() {
                    first = String::from_utf8_lossy(&request[..n]).lines().next().unwrap_or_default().to_string();
                }
                let _ = stream.write_all(answer.as_bytes());
            }
            first
        });
        let query = WebQuery { query: "tauri 2", max_results: 5, time_range: Some(TimeRange::Month) };
        let hits = search(&agent(), &base, &NoModel, &query, &not_private).unwrap();
        assert_eq!(asked.join().unwrap(), "GET /search?q=tauri%202&format=json&time_range=month HTTP/1.1");
        assert_eq!(
            hits,
            vec![
                hit("Read", &format!("{address}/page"), "snippet\n…\nthe page's own words"),
                // Refused before it is asked: it keeps its snippet.
                hit("Not read", &format!("{address}/private"), "its snippet"),
            ]
        );
    }

    #[test]
    fn every_redirect_is_checked_before_it_is_followed() {
        let start = serve(vec![
            answer("302 Found\r\nLocation: /next", ""),
            answer("301 Moved\r\nLocation: /private", ""),
            answer("200 OK\r\nContent-Type: text/plain", "the router's admin page"),
        ]);
        let refused = read_checked(&agent(), &format!("{start}/start"), PAGE_TIMEOUT, &not_private);
        assert!(matches!(refused, Err(WebSearchError::NotAllowed(ref u)) if u.ends_with("/private")), "{refused:?}");
    }

    #[test]
    fn a_page_is_read_where_its_redirects_end() {
        let start = serve(vec![
            answer("302 Found\r\nLocation: /notes.txt", ""),
            answer("200 OK\r\nContent-Type: text/plain; charset=utf-8", "  release notes  "),
        ]);
        let page = read_checked(&agent(), &format!("{start}/start"), PAGE_TIMEOUT, &not_private).unwrap();
        assert_eq!((page.url, page.content.as_str()), (format!("{start}/notes.txt"), "release notes"));
    }

    #[test]
    fn what_is_not_text_or_not_found_is_not_read() {
        let start = serve(vec![answer("200 OK\r\nContent-Type: application/pdf", "%PDF-1.7")]);
        let pdf = read_checked(&agent(), &format!("{start}/a.pdf"), PAGE_TIMEOUT, &not_private);
        assert!(matches!(pdf, Err(WebSearchError::NotReadable(ref why)) if why.contains("application/pdf")), "{pdf:?}");
        let start = serve(vec![answer("404 Not Found", "")]);
        assert!(matches!(read_checked(&agent(), &start, PAGE_TIMEOUT, &not_private), Err(WebSearchError::PageStatus(404))));
        let start = serve(vec![answer("200 OK\r\nContent-Type: text/plain", "   ")]);
        assert!(matches!(read_checked(&agent(), &start, PAGE_TIMEOUT, &not_private), Err(WebSearchError::NotReadable(_))));
    }

    #[test]
    fn redirects_end_somewhere() {
        let start = serve((0..=MAX_REDIRECTS).map(|n| answer(&format!("302 Found\r\nLocation: /{n}"), "")).collect());
        let looped = read_checked(&agent(), &start, PAGE_TIMEOUT, &not_private);
        assert!(matches!(looped, Err(WebSearchError::NotAllowed(ref why)) if why.contains("redirects")), "{looped:?}");
    }

    struct NoModel;

    impl EmbeddingProvider for NoModel {
        fn embed(&self, _: &[&str]) -> Result<Vec<crate::domain::embeddings::Embedding>, crate::domain::embeddings::EmbeddingError> {
            Err(crate::domain::embeddings::EmbeddingError::Invalid("none".into()))
        }

        fn dimensions(&self) -> usize {
            1
        }
    }

    fn page(content: &str) -> WebPage {
        WebPage { title: "T".into(), url: "https://a.example".into(), content: content.into(), truncated: false }
    }

    #[test]
    fn a_long_page_is_cut_and_says_so_and_a_short_one_is_whole() {
        let asked = |query| PageQuery { url: "https://a.example", query, max_chars: 10 };
        assert_eq!(shape(page("short"), &asked(None), &NoModel), page("short"));
        let cut = shape(page("0123456789abcdef"), &asked(None), &NoModel);
        assert_eq!((cut.content.as_str(), cut.truncated), ("0123456789", true));
        // Without the model, a question still gets the beginning.
        assert_eq!(shape(page("0123456789abcdef"), &asked(Some("q")), &NoModel).content, "0123456789");
    }

    #[test]
    fn a_page_read_replaces_its_snippet_and_one_not_read_keeps_it() {
        let hit = |content: &str| WebHit { title: "T".into(), url: "https://a.example".into(), content: content.into() };
        let mut hits = vec![hit("snippet one"), hit("snippet two"), hit("snippet three"), hit("")];
        let pages = vec![Some("the page's own text".into()), None, Some("  ".into()), Some("text".into())];
        with_passages(&mut hits, pages, "q", &NoModel);
        assert_eq!(
            hits,
            vec![hit("snippet one\n…\nthe page's own text"), hit("snippet two"), hit("snippet three"), hit("text")]
        );
    }

    #[test]
    fn markdowns_escapes_go_and_code_keeps_its_backslashes() {
        let page = "Spring AI 2\\.0\\.1 \\(see \\#7006\\) C:\\Users\n```\nlet re = \"\\.\\d\";\n```\nend\\.";
        assert_eq!(unescape(page), "Spring AI 2.0.1 (see #7006) C:\\Users\n```\nlet re = \"\\.\\d\";\n```\nend.");
    }

    #[test]
    fn a_page_slower_than_its_timeout_is_given_up() {
        use std::io::Read;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = stream.read(&mut [0u8; 1024]);
            std::thread::sleep(Duration::from_secs(2));
        });
        let started = std::time::Instant::now();
        let slow = read_checked(&agent(), &address, Duration::from_millis(200), &not_private);
        assert!(matches!(slow, Err(WebSearchError::PageUnavailable(_))), "{slow:?}");
        assert!(started.elapsed() < Duration::from_secs(1), "waited {:?}", started.elapsed());
    }

    #[test]
    fn the_users_own_network_is_not_the_web() {
        for ip in [
            "127.0.0.1", "10.0.0.5", "172.16.3.4", "192.168.194.138", "169.254.169.254", "0.0.0.0", "100.100.1.1",
            "::1", "fd00::1", "fe80::1", "::ffff:127.0.0.1",
        ] {
            assert!(!is_public(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["93.184.216.34", "1.1.1.1", "100.128.0.1", "2606:4700::1111"] {
            assert!(is_public(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn a_private_or_non_web_address_is_refused_before_it_is_asked() {
        for url in ["http://localhost:1420/", "http://127.0.0.1/", "http://[::1]/", "file:///etc/passwd", "ftp://example.com/"] {
            assert!(matches!(public(&Url::parse(url).unwrap()), Err(WebSearchError::NotAllowed(_))), "{url}");
        }
    }

    /// The real thing, against the network: SearXNG answers, pages are read,
    /// and the bundled model picks their passages. Ignored — it needs the
    /// network and a SearXNG (`KIBO_SEARXNG`, `http://localhost:8080/` when
    /// unset). Run after touching this file:
    /// `cargo test open_web_live -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn open_web_live() {
        use crate::infra::local_embeddings::{bundled_model_dir, LocalEmbeddings, DEFAULT_IDLE_UNLOAD};
        let model: Arc<dyn EmbeddingProvider> = Arc::new(LocalEmbeddings::new(bundled_model_dir(None), DEFAULT_IDLE_UNLOAD));
        let base = std::env::var("KIBO_SEARXNG").unwrap_or_else(|_| "http://localhost:8080/".into());
        let web = web(searxng(Url::parse(&base).unwrap(), Arc::clone(&model)).unwrap(), model).unwrap();
        let started = std::time::Instant::now();
        let hits = (web.search)(&WebQuery { query: "Spring AI latest release version", max_results: 5, time_range: None }).unwrap();
        println!("search: {} pages in {:?}", hits.len(), started.elapsed());
        for hit in &hits {
            println!("\n[{}] {}\n{}", hit.title, hit.url, hit.content.chars().take(400).collect::<String>());
        }
        assert!(!hits.is_empty());
        assert!(hits.iter().any(|hit| hit.content.chars().count() > 300), "no page was read past its snippet");
        let versioned = regex::Regex::new(r"\b[12]\.\d+\.\d+").unwrap();
        println!("\npages naming a version: {} of {}", hits.iter().filter(|h| versioned.is_match(&h.content)).count(), hits.len());
        println!("backslashes: {}", hits.iter().map(|h| h.content.matches('\\').count()).sum::<usize>());

        let page = (web.fetch)(&PageQuery { url: &hits[0].url, query: Some("latest version"), max_chars: 4_000 }).unwrap();
        println!("\nfetch: {} ({} chars, truncated: {})\n{}", page.title, page.content.chars().count(), page.truncated, page.content.chars().take(600).collect::<String>());
        assert!(!page.content.is_empty());
        assert!(matches!((web.fetch)(&PageQuery { url: "http://localhost:1420/", query: None, max_chars: 100 }), Err(WebSearchError::NotAllowed(_))));
    }

    #[test]
    fn the_article_is_read_as_markdown_and_an_index_as_its_text() {
        let article = format!(
            "<html><head><title>Release 2.0</title></head><body><nav>Home | Docs</nav><article><h1>Release 2.0</h1>{}</article></body></html>",
            "<p>Tauri 2.0.1 (GA) brings mobile support and a new permission system for plugins and commands.</p>".repeat(8)
        );
        let (title, text) = readable(&article, "https://tauri.app/blog/");
        assert_eq!(title, "Release 2.0");
        assert!(text.contains("mobile support") && !text.contains("Home | Docs"), "{text}");
        assert!(text.contains("Tauri 2.0.1 (GA)") && !text.contains('\\'), "{text}");

        let (title, text) = readable(
            "<html><head><title>Index</title><style>li { color: red }</style></head><body><ul><li>v1</li><li>v2</li></ul>\
             <script>var it = {\"retpath\":\"https:\\u002F\\u002Fsso.example\"};</script><noscript>Enable JS</noscript></body></html>",
            "https://x.example/",
        );
        assert_eq!(title, "Index");
        assert!(text.contains("v1") && text.contains("v2"), "{text}");
        assert!(!text.contains("retpath") && !text.contains("Enable JS") && !text.contains("color"), "{text}");

        // As dzen.ru arrives: no article to find, and the body is the scripts
        // that would draw one. Their code is not text, so the page has none.
        let (_, text) = readable(
            "<html><head><title>Взносы ИП</title><style>body { margin: 0 }</style></head><body>\
             <script>var it = {\"retpath\":\"https:\\u002F\\u002Fdzen.example\"};</script>\
             <noscript>Включите JavaScript</noscript><template><p>card</p></template></body></html>",
            "https://dzen.example/a/1",
        );
        assert_eq!(text, "");
    }
}
