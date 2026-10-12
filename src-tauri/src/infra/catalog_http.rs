//! Reading a catalog over HTTP (`docs/32-catalog.md`, `docs/08-data-policy.md`):
//! a GET for its `index.json`, nothing sent but the address — no key, no
//! cookie, nothing of the user's.
//!
//! Redirects are followed, since nothing in the request would leak along
//! them, but only to `https` from an `https` source: what is read here is
//! installed later, and a hop to plain http would let it be rewritten on
//! the way.

use std::time::Duration;

use url::Url;

use crate::domain::catalog::{self, CatalogError};
use crate::infra::http_agent;

/// A static file answers in well under a second; this is a host that is not coming.
const TIMEOUT: Duration = Duration::from_secs(20);

/// What a conditional GET came back with.
#[derive(Debug, PartialEq, Eq)]
pub enum Fetched {
    Fresh { text: String, etag: Option<String> },
    /// The copy whose `ETag` was sent is still the current one.
    NotModified,
}

/// The index at `base`, unless `etag` says the copy already held is current.
pub fn fetch_index(base: &Url, etag: Option<&str>) -> Result<Fetched, CatalogError> {
    let agent = http_agent::build_agent(None).map_err(|e| CatalogError::Unavailable(e.to_string()))?;
    fetch_with(&agent, base, etag)
}

/// One of the source's files — `path` already checked by the index's parser —
/// as bytes, refused past [`catalog::MAX_FILE_BYTES`] rather than cut.
pub fn fetch_file(base: &Url, path: &str) -> Result<Vec<u8>, CatalogError> {
    let agent = http_agent::build_agent(None).map_err(|e| CatalogError::Unavailable(e.to_string()))?;
    fetch_file_with(&agent, base, path)
}

/// A GET of `url`, held to `https` when the source is — a redirect down to
/// plain http fails rather than being followed.
fn get(agent: &ureq::Agent, base: &Url, url: &Url, etag: Option<&str>) -> Result<ureq::http::Response<ureq::Body>, CatalogError> {
    let mut request = agent.get(url.as_str());
    if let Some(etag) = etag {
        request = request.header("If-None-Match", etag);
    }
    request
        .config()
        .timeout_global(Some(TIMEOUT))
        .https_only(base.scheme() == "https")
        .build()
        .call()
        .map_err(|e| CatalogError::Unavailable(e.to_string()))
}

fn fetch_with(agent: &ureq::Agent, base: &Url, etag: Option<&str>) -> Result<Fetched, CatalogError> {
    let mut response = get(agent, base, &catalog::file_url(base, catalog::INDEX_FILE)?, etag)?;
    let status = response.status().as_u16();
    match status {
        304 => return Ok(Fetched::NotModified),
        200..=299 => {}
        _ => return Err(CatalogError::Status(status)),
    }
    let etag = response.headers().get("etag").and_then(|v| v.to_str().ok()).map(str::to_string);
    let text = response
        .body_mut()
        .with_config()
        .limit(catalog::MAX_INDEX_BYTES)
        .read_to_string()
        .map_err(|e| CatalogError::Unavailable(e.to_string()))?;
    Ok(Fetched::Fresh { text, etag })
}

fn fetch_file_with(agent: &ureq::Agent, base: &Url, path: &str) -> Result<Vec<u8>, CatalogError> {
    let mut response = get(agent, base, &catalog::file_url(base, path)?, None)?;
    let status = response.status().as_u16();
    if !(200..=299).contains(&status) {
        return Err(CatalogError::Status(status));
    }
    response
        .body_mut()
        .with_config()
        // ureq refuses a body that reaches its limit; one at the limit is fine here.
        .limit(catalog::MAX_FILE_BYTES + 1)
        .read_to_vec()
        .map_err(|e| CatalogError::Unavailable(format!("{path}: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    /// One connection answered with `reply`; the request's head comes back
    /// on the channel.
    fn serve_once(reply: String) -> (Url, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = catalog::source_base(&format!("http://127.0.0.1:{}/cat/", listener.local_addr().unwrap().port())).unwrap();
        let (sent, heard) = mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut head = String::new();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                    break;
                }
                head.push_str(&line);
            }
            sent.send(head).unwrap();
            stream.write_all(reply.as_bytes()).unwrap();
        });
        (base, heard)
    }

    fn reply(status: &str, headers: &str, body: &str) -> String {
        format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}", body.len())
    }

    #[test]
    fn reads_the_index_and_its_etag() {
        let (base, heard) = serve_once(reply("200 OK", "ETag: \"abc\"\r\n", r#"{"version":1}"#));
        let fetched = fetch_index(&base, None).unwrap();
        assert_eq!(fetched, Fetched::Fresh { text: r#"{"version":1}"#.into(), etag: Some("\"abc\"".into()) });
        let head = heard.recv().unwrap();
        assert!(head.starts_with("GET /cat/index.json "), "{head}");
        assert!(!head.to_ascii_lowercase().contains("if-none-match"), "{head}");
    }

    #[test]
    fn sends_the_etag_held_and_hears_not_modified() {
        let (base, heard) = serve_once(reply("304 Not Modified", "", ""));
        assert_eq!(fetch_index(&base, Some("\"abc\"")).unwrap(), Fetched::NotModified);
        let head = heard.recv().unwrap().to_ascii_lowercase();
        assert!(head.contains("if-none-match: \"abc\""), "{head}");
    }

    #[test]
    fn another_status_is_an_error() {
        let (base, _heard) = serve_once(reply("404 Not Found", "", "nope"));
        assert_eq!(fetch_index(&base, None), Err(CatalogError::Status(404)));
    }

    #[test]
    fn an_index_past_the_limit_is_not_read() {
        let body = " ".repeat(catalog::MAX_INDEX_BYTES as usize + 1);
        let (base, _heard) = serve_once(reply("200 OK", "", &body));
        assert!(matches!(fetch_index(&base, None), Err(CatalogError::Unavailable(_))));
    }

    #[test]
    fn a_file_is_read_from_under_its_source() {
        let (base, heard) = serve_once(reply("200 OK", "", "# Review"));
        assert_eq!(fetch_file(&base, "skills/review/SKILL.md").unwrap(), b"# Review");
        assert!(heard.recv().unwrap().starts_with("GET /cat/skills/review/SKILL.md "));
        let (base, _heard) = serve_once(reply("404 Not Found", "", "nope"));
        assert_eq!(fetch_file(&base, "x.md"), Err(CatalogError::Status(404)));
    }

    #[test]
    fn a_file_past_the_limit_is_refused_not_cut() {
        let at_limit = "x".repeat(catalog::MAX_FILE_BYTES as usize);
        let (base, _heard) = serve_once(reply("200 OK", "", &at_limit));
        assert_eq!(fetch_file(&base, "x.md").unwrap().len(), at_limit.len());
        let (base, _heard) = serve_once(reply("200 OK", "", &format!("{at_limit}x")));
        assert!(matches!(fetch_file(&base, "x.md"), Err(CatalogError::Unavailable(why)) if why.starts_with("x.md: ")));
    }

    /// What is read gets installed: an https catalog that sends the client on
    /// to plain http is refused, not followed into a connection anyone on the
    /// way could rewrite.
    #[test]
    fn an_https_source_is_not_followed_to_plain_http() {
        use crate::infra::http_agent::tests::{serve_tls_with, CA, LEAF, LEAF_KEY};
        let (plain, _heard) = serve_once(reply("200 OK", "", r#"{"version":1}"#));
        let moved = reply("302 Found", &format!("Location: {plain}index.json\r\n"), "");
        let base = catalog::source_base(&format!("https://127.0.0.1:{}/", serve_tls_with(LEAF, LEAF_KEY, &moved))).unwrap();
        let agent = http_agent::build_agent(Some(CA)).unwrap();
        assert!(matches!(fetch_with(&agent, &base, None), Err(CatalogError::Unavailable(_))));
    }

    /// The app's own catalog, read and parsed as the page does — ignored: it
    /// needs the network. Run after changing the index format or the catalog:
    /// `cargo test live_catalog -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_catalog_reads_and_answers_not_modified() {
        let base = catalog::source_base(catalog::DEFAULT_SOURCE).unwrap();
        let Fetched::Fresh { text, etag } = fetch_index(&base, None).unwrap() else { panic!("no index") };
        let index = catalog::parse_index(&text).unwrap();
        println!("{} entries, problems {:?}, etag {etag:?}", index.items.len(), index.problems);
        assert!(!index.items.is_empty());
        assert_eq!(index.problems, Vec::<String>::new());
        assert_eq!(fetch_index(&base, etag.as_deref()).unwrap(), Fetched::NotModified);
    }

    #[test]
    fn nothing_listening_is_unavailable() {
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let base = catalog::source_base(&format!("http://127.0.0.1:{port}/")).unwrap();
        assert!(matches!(fetch_index(&base, None), Err(CatalogError::Unavailable(_))));
    }
}
