//! DeepSeek's own Search and Fetch (api-docs.deepseek.com/guides/search), as
//! the `Web` that `deepseekSearch` and `deepseekFetch` reach the web with.
//! The key is the DeepSeek provider's; a page is read on DeepSeek's side.

use std::sync::Arc;
use std::time::Duration;

use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::domain::embeddings::EmbeddingProvider;
use crate::domain::provider_balance::DEEPSEEK_API;
use crate::domain::web_search::{PageQuery, Web, WebHit, WebPage, WebQuery, WebSearchError};
use crate::infra::http_agent::{self, TlsError};
use crate::infra::open_web::shape;

/// A search answers in a second or two.
const SEARCH_TIMEOUT: Duration = Duration::from_secs(20);
/// A fetch waits on the page's own site too.
const FETCH_TIMEOUT: Duration = Duration::from_secs(40);
/// Of an error body, what the model is shown.
const ERROR_CHARS: usize = 300;

pub fn web(key: SecretString, trusted_cert_pem: Option<&str>, model: Arc<dyn EmbeddingProvider>) -> Result<Web, TlsError> {
    web_at(DEEPSEEK_API.to_string(), key, trusted_cert_pem, model)
}

fn web_at(base: String, key: SecretString, trusted_cert_pem: Option<&str>, model: Arc<dyn EmbeddingProvider>) -> Result<Web, TlsError> {
    let agent = http_agent::build_agent(trusted_cert_pem)?;
    let key = Arc::new(key);
    let (search_agent, search_key, search_base) = (agent.clone(), Arc::clone(&key), base.clone());
    Ok(Web {
        search: Arc::new(move |query: &WebQuery| {
            let body = json!({ "query": query.query, "count": query.max_results });
            let text = post(&search_agent, &format!("{search_base}/v1/tools/search"), &search_key, body, SEARCH_TIMEOUT)?;
            parse_hits(&text)
        }),
        fetch: Arc::new(move |query: &PageQuery| {
            let text = post(&agent, &format!("{base}/v1/tools/fetch"), &key, json!({ "url": query.url }), FETCH_TIMEOUT)?;
            let page: Fetched = serde_json::from_str(&text).map_err(|e| WebSearchError::BadAnswer(e.to_string()))?;
            let page = WebPage { title: page.title, url: query.url.to_string(), content: page.content, truncated: false };
            Ok(shape(page, query, model.as_ref()))
        }),
    })
}

fn post(agent: &ureq::Agent, url: &str, key: &SecretString, body: Value, timeout: Duration) -> Result<String, WebSearchError> {
    let mut response = agent
        .post(url)
        .header("Authorization", format!("Bearer {}", key.expose_secret()).as_str())
        .config()
        .timeout_global(Some(timeout))
        .build()
        .send_json(body)
        .map_err(|e| WebSearchError::Unavailable(e.to_string()))?;
    let status = response.status().as_u16();
    let text = response.body_mut().read_to_string().map_err(|e| WebSearchError::Unavailable(e.to_string()))?;
    match status {
        200..=299 => Ok(text),
        _ => Err(status_error(status, &text)),
    }
}

#[derive(Deserialize)]
struct Found {
    #[serde(default)]
    title: String,
    url: String,
    #[serde(default)]
    snippet: String,
    /// The long summary, "intended to be read by a model".
    #[serde(default)]
    summary: String,
}

#[derive(Deserialize)]
struct Fetched {
    #[serde(default)]
    title: String,
    content: String,
}

fn parse_hits(text: &str) -> Result<Vec<WebHit>, WebSearchError> {
    #[derive(Deserialize)]
    struct Answer {
        results: Vec<Found>,
    }
    let answer: Answer = serde_json::from_str(text).map_err(|e| WebSearchError::BadAnswer(e.to_string()))?;
    Ok(answer
        .results
        .into_iter()
        .map(|f| WebHit { title: f.title, url: f.url, content: if f.summary.trim().is_empty() { f.snippet } else { f.summary } })
        .collect())
}

/// What each status means, by DeepSeek's table: the model is told what to do
/// next, and where the user fixes it.
fn status_error(status: u16, body: &str) -> WebSearchError {
    let said = said(body);
    match status {
        401 => WebSearchError::Refused(format!(
            "DeepSeek refused the API key ({said}) — tell the user to check the DeepSeek provider's key in Settings → Models"
        )),
        402 => WebSearchError::LimitReached(format!("the DeepSeek account's balance is spent: {said}")),
        429 => WebSearchError::RateLimited,
        // Fetch's: the security policy refused the address, or the page has nothing to read.
        403 | 404 => WebSearchError::NotReadable(format!("{status}: {said}")),
        400..=499 => WebSearchError::Refused(format!("{status}: {said}")),
        _ => WebSearchError::Unavailable(format!("{status}: {said}")),
    }
}

/// `{"error": {"message": …}}`, OpenAI's shape; the body itself, cut, otherwise.
fn said(body: &str) -> String {
    let parsed: Option<Value> = serde_json::from_str(body).ok();
    let text = parsed.as_ref().and_then(|v| v["error"]["message"].as_str()).unwrap_or(body).trim();
    match text.char_indices().nth(ERROR_CHARS) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    struct NoModel;
    impl EmbeddingProvider for NoModel {
        fn embed(&self, _: &[&str]) -> Result<Vec<crate::domain::embeddings::Embedding>, crate::domain::embeddings::EmbeddingError> {
            Err(crate::domain::embeddings::EmbeddingError::Invalid("no model in this test".into()))
        }
        fn dimensions(&self) -> usize {
            2
        }
    }

    /// Answers one request with `status` and `body`, and hands back the request.
    fn serve(status: &str, body: &str) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let answer = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let handle = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0u8; 8192];
            while let Ok(n @ 1..) = socket.read(&mut buffer) {
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
            socket.write_all(answer.as_bytes()).unwrap();
            String::from_utf8_lossy(&request).into_owned()
        });
        (format!("http://127.0.0.1:{port}"), handle)
    }

    fn body_of(request: &str) -> Value {
        let (_, body) = request.split_once("\r\n\r\n").expect("a head and a body");
        serde_json::from_str(body).expect("a JSON body")
    }

    fn web_on(base: String) -> Web {
        web_at(base, SecretString::from("sk-ds"), None, Arc::new(NoModel)).unwrap()
    }

    #[test]
    fn a_search_asks_with_the_key_and_reads_the_summaries() {
        let (base, server) = serve(
            "200 OK",
            r#"{"results":[{"rerank_score":2.4,"title":"V4","url":"https://a.example/v4","snippet":"short","summary":"long one","content":""},
                {"rerank_score":1,"title":"B","url":"https://b.example","snippet":"only a snippet","summary":"","content":""}]}"#,
        );
        let hits = (web_on(base).search)(&WebQuery { query: "deepseek v4", max_results: 3, time_range: None }).unwrap();
        let sent = server.join().unwrap();
        assert!(sent.starts_with("POST /v1/tools/search HTTP/1.1"), "{sent}");
        assert!(sent.to_lowercase().contains("authorization: bearer sk-ds"), "{sent}");
        assert_eq!(body_of(&sent), json!({ "query": "deepseek v4", "count": 3 }));
        assert_eq!(
            hits,
            [
                WebHit { title: "V4".into(), url: "https://a.example/v4".into(), content: "long one".into() },
                WebHit { title: "B".into(), url: "https://b.example".into(), content: "only a snippet".into() },
            ]
        );
    }

    #[test]
    fn a_page_is_fetched_by_its_address_and_cut_to_the_limit() {
        let (base, server) = serve("200 OK", r##"{"title":"Notes","content":"# Notes\n\nabcdefghij"}"##);
        let page = (web_on(base).fetch)(&PageQuery { url: "https://a.example/notes", query: None, max_chars: 10 }).unwrap();
        let sent = server.join().unwrap();
        assert!(sent.starts_with("POST /v1/tools/fetch HTTP/1.1"), "{sent}");
        assert_eq!(body_of(&sent), json!({ "url": "https://a.example/notes" }));
        assert_eq!(
            page,
            WebPage { title: "Notes".into(), url: "https://a.example/notes".into(), content: "# Notes\n\na".into(), truncated: true }
        );
    }

    #[test]
    fn statuses_say_what_the_model_should_do() {
        let body = r#"{"error":{"message":"Insufficient Balance","type":"x","param":null,"code":"x"}}"#;
        assert!(matches!(status_error(401, body), WebSearchError::Refused(m) if m.contains("Settings → Models")));
        assert!(matches!(status_error(402, body), WebSearchError::LimitReached(m) if m.ends_with("Insufficient Balance")));
        assert!(matches!(status_error(429, ""), WebSearchError::RateLimited));
        assert!(matches!(status_error(404, "gone"), WebSearchError::NotReadable(m) if m == "404: gone"));
        assert!(matches!(status_error(400, body), WebSearchError::Refused(m) if m == "400: Insufficient Balance"));
        assert!(matches!(status_error(504, ""), WebSearchError::Unavailable(_)));
    }

    #[test]
    fn a_refusal_reaches_the_caller_with_its_reason() {
        let (base, server) = serve("402 Payment Required", r#"{"error":{"message":"Insufficient Balance"}}"#);
        let error = (web_on(base).search)(&WebQuery { query: "q", max_results: 1, time_range: None }).unwrap_err();
        server.join().unwrap();
        assert!(matches!(error, WebSearchError::LimitReached(_)), "{error}");
    }
}
