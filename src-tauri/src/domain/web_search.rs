//! What a chat's `webSearch` and `webFetch` ask of the web, and what comes
//! back (`docs/24-web-search.md`). The search is `infra::tavily` or
//! `infra::open_web`, a page always `infra::open_web`; ports, because the
//! tools' tests answer with a stand-in, not the network.

use std::sync::Arc;

use crate::domain::embeddings::EmbeddingProvider;
use crate::domain::llm::{LlmMessage, LlmRole};
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
    #[error("the page answered {0} — try another address")]
    PageStatus(u16),
    #[error("the page is not answering: {0}")]
    PageUnavailable(String),
    #[error("the page has no text to read: {0}")]
    NotReadable(String),
    /// A private, loopback or non-web address: the user's own network is not
    /// the web, and a page must not be able to point the model at it.
    #[error("not fetched: {0}")]
    NotAllowed(String),
}

/// What the key has spent this billing cycle, and of how much — Settings'
/// line under the key. `limit` is `None` when Tavily sets none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebUsage {
    pub plan: Option<String>,
    pub used: u64,
    pub limit: Option<u64>,
}

pub type WebSearchFn = Arc<dyn Fn(&WebQuery) -> Result<Vec<WebHit>, WebSearchError> + Send + Sync>;

/// One page, asked for by address. `query` picks the passages of a page too
/// long to send whole; without it, its beginning is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageQuery<'a> {
    pub url: &'a str,
    pub query: Option<&'a str>,
    pub max_chars: usize,
}

/// A page read: its text as Markdown, whole or cut to what was asked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebPage {
    pub title: String,
    /// Where it ended up, after redirects.
    pub url: String,
    /// The page author's words, like [`WebHit::content`].
    pub content: String,
    /// Only part of the page: its passages about the query, or its start.
    pub truncated: bool,
}

pub type WebFetchFn = Arc<dyn Fn(&PageQuery) -> Result<WebPage, WebSearchError> + Send + Sync>;

/// What a chat reaches the web with: the search the user chose, and the page
/// reader, which needs no service.
#[derive(Clone)]
pub struct Web {
    pub search: WebSearchFn,
    pub fetch: WebFetchFn,
}

#[cfg(test)]
impl Web {
    /// A search, and a page reader that finds nothing.
    pub fn search_only(search: WebSearchFn) -> Self {
        Web { search, fetch: Arc::new(|_: &PageQuery| Err(WebSearchError::PageStatus(404))) }
    }
}

/// About a paragraph or two — what Tavily sends a passage as.
pub const PASSAGE_CHARS: usize = 600;

/// The parts of `text` closest to `query`, in the page's order, within
/// `budget` characters: Tavily's `content`, made here. A text that fits is
/// returned whole, without asking the model. `None` when the model could not
/// be used — the caller has a cruder answer: a search's snippet, a page's start.
///
/// Closest by the model alone. A share of the query's words added to the
/// cosine was tried (2026-10-09) on the same pages for seven queries — a
/// release, error messages, a Kubernetes state: it found the asked-for token in
/// no more passages anywhere, and in fewer once. The model already matches a
/// rare token like `ERR_OSSL_EVP_UNSUPPORTED`; what it cannot see is that
/// "latest version" means a number, and the engine's snippet, kept first by
/// the search, is what supplies that.
pub fn passages(text: &str, query: &str, model: &dyn EmbeddingProvider, budget: usize) -> Option<String> {
    let text = text.trim();
    if text.chars().count() <= budget {
        return Some(text.to_string());
    }
    let chunks = chunks(text, PASSAGE_CHARS);
    let texts: Vec<&str> = std::iter::once(query).chain(chunks.iter().map(String::as_str)).collect();
    let vectors = model.embed(&texts).ok()?;
    let (asked, found) = vectors.split_first()?;
    // Every vector of the bundled model is unit length: the dot is the cosine.
    let score = |v: &[f32]| v.iter().zip(&asked.0).map(|(a, b)| a * b).sum::<f32>();
    let mut ranked: Vec<(usize, f32)> = found.iter().enumerate().map(|(i, v)| (i, score(&v.0))).collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut chosen = Vec::new();
    let mut used = 0;
    for (i, _) in ranked {
        let len = chunks[i].chars().count();
        if used + len <= budget {
            chosen.push(i);
            used += len;
        }
    }
    chosen.sort_unstable();
    Some(chosen.iter().map(|&i| chunks[i].as_str()).collect::<Vec<_>>().join("\n…\n"))
}

/// Lines gathered into pieces of at most `size` characters; a line longer
/// than that is cut between words.
fn chunks(text: &str, size: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut push = |piece: &str, current: &mut String| {
        if !current.is_empty() && current.chars().count() + 1 + piece.chars().count() > size {
            out.push(std::mem::take(current));
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(piece);
    };
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if line.chars().count() <= size {
            push(line, &mut current);
            continue;
        }
        let mut piece = String::new();
        for word in line.split_whitespace() {
            if !piece.is_empty() && piece.chars().count() + 1 + word.chars().count() > size {
                push(&piece, &mut current);
                piece.clear();
            }
            if !piece.is_empty() {
                piece.push(' ');
            }
            piece.push_str(word);
        }
        push(&piece, &mut current);
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// Whether `url` is in what the user wrote or a tool sent back — never only in
/// what the model wrote itself. What `webFetch` opens: a page that tells the
/// model to fetch `https://evil.example/?chat=<what the user said>` gets
/// nowhere, because that address is in no message — the model put it together.
/// Compared without the scheme and the fragment, as a user types `docs.rs/x`.
pub fn was_given(url: &str, history: &[LlmMessage]) -> bool {
    let bare = |u: &str| -> String {
        let u = u.split('#').next().unwrap_or(u);
        let u = u.strip_prefix("https://").or_else(|| u.strip_prefix("http://")).unwrap_or(u);
        u.trim_end_matches('/').to_string()
    };
    let wanted = bare(url.trim());
    !wanted.is_empty()
        && history
            .iter()
            .filter(|m| matches!(m.role, LlmRole::User | LlmRole::Tool))
            .filter_map(|m| m.content.as_deref())
            .any(|text| text.contains(&wanted))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::embeddings::{Embedding, EmbeddingError};

    /// A vector per text: 1 on the axis of the first of `words` it contains.
    struct Words(&'static [&'static str]);

    impl EmbeddingProvider for Words {
        fn embed(&self, texts: &[&str]) -> Result<Vec<Embedding>, EmbeddingError> {
            Ok(texts
                .iter()
                .map(|t| Embedding(self.0.iter().map(|w| if t.contains(w) { 1.0 } else { 0.0 }).collect()))
                .collect())
        }

        fn dimensions(&self) -> usize {
            self.0.len()
        }
    }

    struct Broken;

    impl EmbeddingProvider for Broken {
        fn embed(&self, _: &[&str]) -> Result<Vec<Embedding>, EmbeddingError> {
            Err(EmbeddingError::Invalid("no".into()))
        }

        fn dimensions(&self) -> usize {
            1
        }
    }

    #[test]
    fn a_text_that_fits_is_sent_whole_without_the_model() {
        assert_eq!(passages("  short page \n", "q", &Broken, 100).as_deref(), Some("short page"));
    }

    #[test]
    fn the_passages_about_the_query_are_kept_in_the_pages_order() {
        let filler = |w: &str| format!("{w} {}", "x".repeat(PASSAGE_CHARS - 10));
        let text = [filler("intro"), filler("tokio"), filler("other"), filler("tokio again")].join("\n");
        // "tokio again" ranks first; the page has it last, and so does the answer.
        let kept = passages(&text, "tokio again", &Words(&["tokio", "again"]), 2 * PASSAGE_CHARS).unwrap();
        assert!(kept.starts_with("tokio x") && kept.contains("\n…\ntokio again"), "{kept}");
        assert!(!kept.contains("intro") && !kept.contains("other"));
        assert!(kept.chars().count() <= 2 * PASSAGE_CHARS + 3);
    }

    #[test]
    fn without_the_model_there_are_no_passages() {
        assert_eq!(passages(&"word ".repeat(500), "q", &Broken, 100), None);
    }

    #[test]
    fn a_long_line_is_cut_between_words_and_short_lines_are_gathered() {
        let long = "word ".repeat(300);
        let pieces = chunks(&format!("a\nb\n{long}"), 100);
        assert!(pieces.iter().all(|p| p.chars().count() <= 100), "{pieces:?}");
        assert_eq!(pieces[0], "a\nb");
        assert_eq!(pieces.concat().matches("word").count(), 300);
    }

    #[test]
    fn an_address_counts_only_when_the_user_or_a_tool_gave_it() {
        let model = LlmMessage::assistant("see https://evil.example/?chat=secret");
        let history = vec![
            LlmMessage::user("read docs.rs/tokio please"),
            LlmMessage { role: LlmRole::Tool, content: Some("[1] Tokio\nhttps://tokio.rs/blog/".into()), ..LlmMessage::user("") },
            model,
        ];
        assert!(was_given("https://docs.rs/tokio", &history));
        assert!(was_given("https://tokio.rs/blog#latest", &history));
        assert!(was_given("http://tokio.rs/blog/", &history));
        assert!(!was_given("https://evil.example/?chat=secret", &history));
        assert!(!was_given("https://tokio.rs/blog/?leak=1", &history));
        assert!(!was_given("  ", &history));
    }
}
