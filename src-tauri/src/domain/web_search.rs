//! What a chat's `webSearch` asks of the web, and what comes back
//! (`docs/24-web-search.md`). The search itself is `infra::tavily`; a port,
//! because the tool's tests answer with a stand-in, not the network.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// How far back a page may have been published or updated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TimeRange {
    Day,
    Week,
    Month,
    Year,
}

/// One search, already checked by the tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebQuery<'a> {
    pub query: &'a str,
    pub max_results: u32,
    pub time_range: Option<TimeRange>,
}

/// A page found: where it is, and the passages of it that match the query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebHit {
    pub title: String,
    pub url: String,
    /// Written by the page's author, not by anyone the user trusts. The log
    /// keeps the address and drops this (`tool_call_log::CONTENT_FIELDS`).
    pub content: String,
}

/// Why the search service gave nothing. Every message is for the model,
/// which tells the user or answers without it.
#[derive(Debug, Error)]
pub enum WebSearchError {
    #[error("the search service refused the API key — tell the user to check it in Settings → Web search, and answer without searching")]
    KeyRefused,
    /// Too many at once: the next turn may do better, this one should not.
    #[error("the search service is rate-limiting — do not search again in this turn")]
    RateLimited,
    /// The plan's credits are spent: no search works until the user acts.
    #[error("the search service's usage limit is reached ({0}) — tell the user, and answer without searching")]
    LimitReached(String),
    #[error("the search service refused the query: {0}")]
    Refused(String),
    #[error("the search service is not answering: {0}")]
    Unavailable(String),
    #[error("the search service's answer could not be read: {0}")]
    BadAnswer(String),
}

pub type WebSearchFn = Arc<dyn Fn(&WebQuery) -> Result<Vec<WebHit>, WebSearchError> + Send + Sync>;
