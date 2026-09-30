//! `webSearch` — a chat's way to what the model could not know: a version out
//! since its training, an error message, a known issue (`docs/24-web-search.md`).
//! The search is `deps.web`; this is the checking, the size, and the schema.

use crate::domain::llm::LlmToolDefinition;
use crate::domain::tools::{ToolDeps, ToolError, ToolResult, WebSearchArgs};
use crate::domain::web_search::WebQuery;

use super::semantic_search::shorten;

pub const DEFAULT_RESULTS: u32 = 5;
pub const MAX_RESULTS: u32 = 10;
/// Tavily sends up to three passages of ~500 characters a page; this keeps
/// one page from taking the result over.
const CONTENT_CHARS: usize = 2_000;
/// A search query, not a document: the query leaves the machine, and a long
/// one is usually a paste that should not.
pub const MAX_QUERY_CHARS: usize = 400;

pub fn web_search(args: &WebSearchArgs, deps: &ToolDeps) -> Result<ToolResult, ToolError> {
    let search = deps.web.as_ref().ok_or(ToolError::NoWebSearch)?;
    let query = args.query.trim();
    let invalid = |reason: String| ToolError::InvalidArguments { tool: "webSearch".into(), reason };
    if query.is_empty() {
        return Err(invalid("`query` is empty — say what to search for".into()));
    }
    if query.chars().count() > MAX_QUERY_CHARS {
        return Err(invalid(format!(
            "`query` is longer than {MAX_QUERY_CHARS} characters — search with the words that matter, not a pasted text"
        )));
    }
    let max_results = args.max_results.unwrap_or(DEFAULT_RESULTS).clamp(1, MAX_RESULTS);
    let hits = search(&WebQuery { query, max_results, time_range: args.time_range })?;
    Ok(ToolResult::WebResults {
        hits: hits
            .into_iter()
            .map(|mut hit| {
                hit.content = shorten(&hit.content, CONTENT_CHARS);
                hit
            })
            .collect(),
    })
}

pub(super) fn definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "webSearch".to_string(),
        description: format!(
            "Search the web for what you cannot know from your training or this conversation: current versions, \
release notes, error messages, known issues, documentation. Returns pages with their address and the passages that match. \
Search when the answer depends on facts that change — a release, a chart's or an operator's notes, the known issue \
behind an error message, what a version deprecates; answer from what you know when it does not. \
The query goes to a search service outside the user's machine: search with the error text, the product and its version — \
never with secrets, credentials, or names from the user's own systems (hosts, clusters, namespaces, services). \
Write the query in the language the pages you want are written in — usually English for technical subjects. \
Say which address each fact you use comes from. Returns at most {MAX_RESULTS} pages."
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": format!("What to search for, as a search engine query (at most {MAX_QUERY_CHARS} characters).")
                },
                "maxResults": {
                    "type": ["integer", "null"],
                    "description": format!("How many pages, 1 to {MAX_RESULTS}; {DEFAULT_RESULTS} when absent.")
                },
                "timeRange": {
                    "type": ["string", "null"],
                    "enum": ["day", "week", "month", "year", null],
                    "description": "Only pages published or updated this recently — for news and releases."
                }
            },
            "required": ["query"],
            "additionalProperties": false
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::web_search::{TimeRange, WebHit, WebSearchError, WebSearchFn};
    use std::sync::{Arc, Mutex};

    type Asked = Arc<Mutex<Vec<(String, u32, Option<TimeRange>)>>>;

    /// Answers with `hits`, and records what it was asked.
    fn web(hits: Vec<WebHit>) -> (WebSearchFn, Asked) {
        let asked = Asked::default();
        let seen = asked.clone();
        let search: WebSearchFn = Arc::new(move |q: &WebQuery| {
            seen.lock().unwrap().push((q.query.to_string(), q.max_results, q.time_range));
            Ok(hits.clone())
        });
        (search, asked)
    }

    fn deps(web: Option<WebSearchFn>) -> ToolDeps<'static> {
        ToolDeps { web, ..ToolDeps::default() }
    }

    fn args(query: &str, max_results: Option<u32>) -> WebSearchArgs {
        WebSearchArgs { query: query.into(), max_results, time_range: None }
    }

    fn hit(content: &str) -> WebHit {
        WebHit { title: "T".into(), url: "https://a.example".into(), content: content.into() }
    }

    #[test]
    fn without_a_key_it_says_where_to_add_one() {
        let error = web_search(&args("q", None), &deps(None)).unwrap_err();
        assert!(matches!(error, ToolError::NoWebSearch));
        assert!(error.to_string().contains("Settings → Web search"));
    }

    #[test]
    fn the_query_is_trimmed_and_the_count_kept_in_range() {
        let (search, asked) = web(vec![]);
        let deps = deps(Some(search));
        for (count, sent) in [(None, DEFAULT_RESULTS), (Some(0), 1), (Some(3), 3), (Some(MAX_RESULTS), MAX_RESULTS), (Some(50), MAX_RESULTS)] {
            web_search(&WebSearchArgs { time_range: Some(TimeRange::Week), ..args("  rust 2027  ", count) }, &deps).unwrap();
            assert_eq!(asked.lock().unwrap().pop().unwrap(), ("rust 2027".to_string(), sent, Some(TimeRange::Week)));
        }
    }

    #[test]
    fn an_empty_or_pasted_query_is_refused_before_it_leaves() {
        let (search, asked) = web(vec![]);
        let deps = deps(Some(search));
        assert!(matches!(web_search(&args("   ", None), &deps), Err(ToolError::InvalidArguments { .. })));
        let long = "x".repeat(MAX_QUERY_CHARS + 1);
        assert!(matches!(web_search(&args(&long, None), &deps), Err(ToolError::InvalidArguments { .. })));
        assert!(asked.lock().unwrap().is_empty());
        web_search(&args(&"x".repeat(MAX_QUERY_CHARS), None), &deps).unwrap();
        assert_eq!(asked.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_long_page_is_cut_and_a_short_one_kept() {
        let (search, _) = web(vec![hit(&"y".repeat(CONTENT_CHARS + 10)), hit("short")]);
        let Ok(ToolResult::WebResults { hits }) = web_search(&args("q", None), &deps(Some(search))) else { panic!() };
        assert_eq!(hits[0].content, format!("{}…", "y".repeat(CONTENT_CHARS)));
        assert_eq!(hits[1], hit("short"));
    }

    #[test]
    fn the_services_refusal_reaches_the_model() {
        let search: WebSearchFn = Arc::new(|_: &WebQuery| Err(WebSearchError::KeyRefused));
        let error = web_search(&args("q", None), &deps(Some(search))).unwrap_err();
        assert!(matches!(error, ToolError::WebSearch(WebSearchError::KeyRefused)));
    }
}
