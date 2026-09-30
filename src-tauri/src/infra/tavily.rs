//! Tavily's search API — the one place `webSearch` reaches the network
//! (`docs/24-web-search.md`, `docs/08-data-policy.md`). What leaves is the
//! query and its limits; the key goes in a header, to this host only.

use std::sync::Arc;
use std::time::Duration;

use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::domain::web_search::{TimeRange, WebHit, WebQuery, WebSearchError, WebSearchFn};
use crate::infra::http_agent::{self, TlsError};

const ENDPOINT: &str = "https://api.tavily.com/search";
/// The key's name in the sealed credentials file, beside the providers'.
pub const KEY_ID: &str = "web-search:tavily";
/// A basic search answers in a second or two; a minute of nothing is a
/// service that is not coming back this turn.
const TIMEOUT: Duration = Duration::from_secs(20);
/// Of an error body, what the model is shown.
const ERROR_CHARS: usize = 300;

/// The search `webSearch` calls, with the user's key.
pub fn searcher(key: SecretString) -> Result<WebSearchFn, TlsError> {
    let agent = http_agent::build_agent(None)?;
    Ok(Arc::new(move |query: &WebQuery| search(&agent, &key, query)))
}

/// The search with the saved key; `None` while there is none.
pub fn saved() -> Option<WebSearchFn> {
    searcher(crate::infra::llm_credentials_store::get_api_key(KEY_ID)?).ok()
}

pub fn has_saved_key() -> bool {
    crate::infra::llm_credentials_store::has_api_key(KEY_ID)
}

fn search(agent: &ureq::Agent, key: &SecretString, query: &WebQuery) -> Result<Vec<WebHit>, WebSearchError> {
    let mut response = agent
        .post(ENDPOINT)
        .header("Authorization", &format!("Bearer {}", key.expose_secret()))
        .config()
        .timeout_global(Some(TIMEOUT))
        .build()
        .send_json(body(query))
        .map_err(|e| WebSearchError::Unavailable(e.to_string()))?;
    let status = response.status().as_u16();
    let text = response.body_mut().read_to_string().map_err(|e| WebSearchError::Unavailable(e.to_string()))?;
    match status {
        200..=299 => parse(&text),
        _ => Err(status_error(status, &text)),
    }
}

/// `include_answer` off: the model answers, not Tavily's. Nothing beyond the
/// matching passages — raw pages and images are tokens the model did not ask for.
fn body(query: &WebQuery) -> Value {
    let mut body = json!({
        "query": query.query,
        "max_results": query.max_results,
        "search_depth": "basic",
        "include_answer": false,
        "include_raw_content": false,
        "include_images": false,
    });
    if let Some(range) = query.time_range {
        body["time_range"] = json!(match range {
            TimeRange::Day => "day",
            TimeRange::Week => "week",
            TimeRange::Month => "month",
            TimeRange::Year => "year",
        });
    }
    body
}

#[derive(Deserialize)]
struct Answer {
    results: Vec<Found>,
}

#[derive(Deserialize)]
struct Found {
    #[serde(default)]
    title: String,
    url: String,
    #[serde(default)]
    content: String,
}

fn parse(text: &str) -> Result<Vec<WebHit>, WebSearchError> {
    let answer: Answer = serde_json::from_str(text).map_err(|e| WebSearchError::BadAnswer(e.to_string()))?;
    Ok(answer.results.into_iter().map(|f| WebHit { title: f.title, url: f.url, content: f.content }).collect())
}

/// 432 and 433 are Tavily's own: the plan's credits, and the pay-as-you-go
/// ceiling the user set.
fn status_error(status: u16, body: &str) -> WebSearchError {
    let said = said(body);
    match status {
        401 | 403 => WebSearchError::KeyRefused,
        429 => WebSearchError::RateLimited,
        432 | 433 => WebSearchError::LimitReached(said),
        400..=499 => WebSearchError::Refused(format!("{status}: {said}")),
        _ => WebSearchError::Unavailable(format!("{status}: {said}")),
    }
}

/// The error's own words: `{"detail": {"error": …}}` or `{"error": …}`, and
/// the body itself, cut, when it is neither.
fn said(body: &str) -> String {
    let parsed: Option<Value> = serde_json::from_str(body).ok();
    let message = parsed.as_ref().and_then(|v| v["detail"]["error"].as_str().or(v["error"].as_str()).or(v["detail"].as_str()));
    let text = message.unwrap_or(body).trim();
    match text.char_indices().nth(ERROR_CHARS) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(time_range: Option<TimeRange>) -> WebQuery<'static> {
        WebQuery { query: "tokio 2 release", max_results: 7, time_range }
    }

    #[test]
    fn the_body_asks_for_passages_and_nothing_else() {
        assert_eq!(
            body(&query(None)),
            json!({
                "query": "tokio 2 release",
                "max_results": 7,
                "search_depth": "basic",
                "include_answer": false,
                "include_raw_content": false,
                "include_images": false,
            })
        );
    }

    #[test]
    fn a_time_range_is_sent_by_its_api_name() {
        for (range, name) in
            [(TimeRange::Day, "day"), (TimeRange::Week, "week"), (TimeRange::Month, "month"), (TimeRange::Year, "year")]
        {
            assert_eq!(body(&query(Some(range)))["time_range"], name);
        }
    }

    #[test]
    fn results_are_read_and_missing_text_is_empty() {
        let text = r#"{"query":"q","answer":null,"images":[],"response_time":1.2,"results":[
            {"title":"Tokio","url":"https://tokio.rs","content":"a [...] b","score":0.9,"raw_content":null},
            {"url":"https://example.com"}
        ]}"#;
        assert_eq!(
            parse(text).unwrap(),
            vec![
                WebHit { title: "Tokio".into(), url: "https://tokio.rs".into(), content: "a [...] b".into() },
                WebHit { title: String::new(), url: "https://example.com".into(), content: String::new() },
            ]
        );
    }

    #[test]
    fn an_answer_without_results_is_unreadable() {
        assert!(matches!(parse(r#"{"query":"q"}"#), Err(WebSearchError::BadAnswer(_))));
        assert!(matches!(parse("<html>"), Err(WebSearchError::BadAnswer(_))));
    }

    #[test]
    fn statuses_say_what_the_model_should_do() {
        assert!(matches!(status_error(401, ""), WebSearchError::KeyRefused));
        assert!(matches!(status_error(403, ""), WebSearchError::KeyRefused));
        assert!(matches!(status_error(429, ""), WebSearchError::RateLimited));
        assert!(matches!(status_error(432, r#"{"detail":{"error":"plan limit"}}"#), WebSearchError::LimitReached(m) if m == "plan limit"));
        assert!(matches!(status_error(433, ""), WebSearchError::LimitReached(_)));
        assert!(matches!(status_error(400, r#"{"error":"bad query"}"#), WebSearchError::Refused(m) if m == "400: bad query"));
        assert!(matches!(status_error(502, "Bad Gateway"), WebSearchError::Unavailable(m) if m == "502: Bad Gateway"));
    }

    #[test]
    fn an_error_says_its_own_words_cut_short() {
        assert_eq!(said(r#"{"detail":{"error":"Unauthorized"}}"#), "Unauthorized");
        assert_eq!(said(r#"{"detail":"Not found"}"#), "Not found");
        assert_eq!(said("  plain text  "), "plain text");
        let long = "x".repeat(ERROR_CHARS + 5);
        assert_eq!(said(&long), format!("{}…", "x".repeat(ERROR_CHARS)));
        assert_eq!(said(&"x".repeat(ERROR_CHARS)), "x".repeat(ERROR_CHARS));
    }
}
