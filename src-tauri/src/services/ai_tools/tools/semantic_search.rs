//! `semanticSearch` — the model's way into code it has not seen yet. The
//! ranking is `services::code_search`; this is the wrapper: the default size,
//! how much text each match carries, and the schema.

use crate::domain::llm::LlmToolDefinition;
use crate::domain::code_search::SearchFilter;
use crate::domain::search_query::PathFilter;
use crate::domain::tools::{SemanticSearchArgs, ToolDeps, ToolError, ToolResult};
use crate::services::code_search::{DEFAULT_TOP_K, MAX_QUERIES, MAX_TOP_K};

/// Text per match by default. The result lands in the model's context, and
/// ten whole chunks are thousands of tokens spent on text it re-reads with
/// `readFile` anyway. Path, lines and name decide which hit to open; the text
/// only has to be enough to recognise a wrong turn — and to be readable in the
/// transcript, where it is all a person sees.
const TEXT_CHARS: usize = 240;
/// With `preview`: most of a typical chunk (median ~470 bytes).
const PREVIEW_CHARS: usize = 1_200;

pub fn semantic_search(args: &SemanticSearchArgs, deps: &ToolDeps) -> Result<ToolResult, ToolError> {
    let search = deps
        .search
        .as_ref()
        .ok_or_else(|| ToolError::SearchUnavailable("this folder has no index".to_string()))?;
    let filter = SearchFilter {
        include_docs: args.include_docs == Some(true),
        paths: PathFilter::new(args.glob.as_deref(), args.exclude.as_deref()).map_err(ToolError::InvalidPattern)?,
    };
    let wordings: Vec<&str> =
        std::iter::once(args.query.as_str()).chain(args.queries.iter().flatten().map(String::as_str)).collect();
    let result = search(&wordings, args.fts.as_deref(), args.top_k.unwrap_or(DEFAULT_TOP_K), &filter)
        .map_err(ToolError::SearchUnavailable)?;
    let limit = if args.preview == Some(true) { PREVIEW_CHARS } else { TEXT_CHARS };
    let matches = result
        .matches
        .into_iter()
        .map(|mut m| {
            m.text = shorten(&m.text, limit);
            m
        })
        .collect();
    Ok(ToolResult::SearchResults { matches, meta: result.meta })
}

pub(super) fn shorten(text: &str, limit: usize) -> String {
    let Some((cut, _)) = text.char_indices().nth(limit) else {
        return text.to_string();
    };
    format!("{}…", &text[..cut])
}

pub(super) fn definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "semanticSearch".to_string(),
        description: format!(
            "Find code by what it does or what it is called, when you do not already know the file. \
Declarations named in the query come first; the rest is ranked by meaning and by shared words together. \
Each match gives the path, the line range readFile takes, the enclosing declaration's name, and the start of its text. \
Write the query as a sentence about the behaviour, and include any function, type or file names you know or can justify from the user's words. \
Write it in the language the code is written in — usually English — even when the user asked in another: \
a Russian question about English code matches far worse than the same question in English. \
When a question is not in the code's language, or you are unsure how the code puts it, add other wordings in queries: \
they are searched in the same call, which finds more than any one of them and costs less than a search each. \
Searching documentation written in another language, write the query in that language. \
Put the identifiers you expect the code to use in fts (sendNotification, RetryPolicy), not the words of the question. \
Documentation is left out unless you set includeDocs — but for one passage that outranks all the code, listed fifth; glob and exclude narrow the search to paths, as in grep. \
If meta.hint is present, follow it. Use grep instead when you need every occurrence of an exact string. \
Returns at most {MAX_TOP_K} matches."
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "What you are looking for, as a sentence in the language of the code (usually English), with any identifiers you know (parseConfig, RepoIndexer, read_source)."
                },
                "fts": {
                    "type": ["array", "null"],
                    "items": { "type": "string" },
                    "description": "Words that must match as text, when they are not just the query's own — identifiers, error messages, config keys. Leave out filler and guesses: a wrong word here costs ranking."
                },
                "queries": {
                    "type": ["array", "null"],
                    "items": { "type": "string" },
                    "maxItems": MAX_QUERIES - 1,
                    "description": format!("Up to {} more wordings of the same question, searched together with query — the English of a question asked in another language, or the way the code might phrase it.", MAX_QUERIES - 1)
                },
                "topK": {
                    "type": ["integer", "null"],
                    "minimum": 1,
                    "description": format!("How many matches, default {DEFAULT_TOP_K}, at most {MAX_TOP_K}.")
                },
                "preview": {
                    "type": ["boolean", "null"],
                    "description": "Longer text for every match. Leave unset unless you must compare several candidates verbatim; readFile reads one precisely."
                },
                "glob": {
                    "type": ["string", "null"],
                    "description": "Only these files, as in grep: without a `/` it matches the file name (\"*.java\"), with one the path from the workspace root (\"src/main/**\")."
                },
                "exclude": {
                    "type": ["string", "null"],
                    "description": "Files to leave out, as a glob over the path (\"**/generated/**\")."
                },
                "includeDocs": {
                    "type": ["boolean", "null"],
                    "description": "Search documentation too — docs folders, README, Markdown, AsciiDoc. Off by default, so code is not buried under prose that shares its words. Set it when the question is about documentation, design or how-to, or when meta.hint says documentation matches were left out. When no code matches at all, documentation comes back without it, and meta.hint says so."
                }
            },
            "required": ["query"]
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::code_search::{CodeMatch, CodeSearchResult, MatchSource, SearchMeta};
    use std::sync::{Arc, Mutex};

    type Asked = Arc<Mutex<Vec<(String, Option<Vec<String>>, usize, bool)>>>;

    /// Answers every search with one match carrying `text`, and records what
    /// it was asked.
    fn deps_answering(text: &str) -> (ToolDeps, Asked) {
        let asked: Asked = Arc::default();
        let record = Arc::clone(&asked);
        let text = text.to_string();
        let deps = ToolDeps {
            search: Some(Arc::new(move |queries: &[&str], fts: Option<&[String]>, top_k: usize, filter: &SearchFilter| {
                record.lock().unwrap().push((queries.join(" | "), fts.map(<[String]>::to_vec), top_k, filter.include_docs));
                Ok(CodeSearchResult {
                    matches: vec![CodeMatch {
                        path: "a.rs".into(),
                        start_line: 3,
                        end_line: 9,
                        name: Some("a".into()),
                        text: text.clone(),
                        source: MatchSource::Symbol,
                    }],
                    meta: SearchMeta { tiers_used: vec![MatchSource::Symbol], weak: false, hint: None },
                })
            })),
            ..ToolDeps::default()
        };
        (deps, asked)
    }

    fn args(query: &str) -> SemanticSearchArgs {
        SemanticSearchArgs { query: query.into(), ..SemanticSearchArgs::default() }
    }

    fn text_of(result: ToolResult) -> String {
        let ToolResult::SearchResults { matches, .. } = result else { panic!("{result:?}") };
        matches[0].text.clone()
    }

    #[test]
    fn without_an_index_the_model_is_pointed_at_grep() {
        let error = semantic_search(&args("x"), &ToolDeps::default()).unwrap_err();
        assert!(matches!(error, ToolError::SearchUnavailable(_)));
        assert!(error.to_string().contains("grep"), "{error}");
    }

    /// A Russian question about English code loses to the language alone
    /// (bench: 0.384 against 0.720 on the same answers); the model rewrites
    /// its query only if it is told to.
    #[test]
    fn the_model_is_told_to_search_in_the_language_of_the_code() {
        let description = definition().description;
        assert!(description.contains("in the language the code is written in"), "{description}");
        assert!(description.contains("identifiers you expect the code to use in fts"), "{description}");
    }

    /// The other wordings go to the search with the query, first in line.
    #[test]
    fn other_wordings_are_searched_in_the_same_call() {
        let (deps, asked) = deps_answering("fn a() {}");
        let args = SemanticSearchArgs { queries: Some(vec!["send the pack".into()]), ..args("отправить пакет") };
        semantic_search(&args, &deps).unwrap();
        assert_eq!(asked.lock().unwrap()[0].0, "отправить пакет | send the pack");

        let parsed: SemanticSearchArgs = serde_json::from_str(r#"{"query": "q", "queries": "one more"}"#).unwrap();
        assert_eq!(parsed.queries, Some(vec!["one more".to_string()]), "a lone string is one wording");
    }

    #[test]
    fn a_glob_that_does_not_compile_is_refused_before_searching() {
        let (deps, asked) = deps_answering("fn a() {}");
        let error = semantic_search(&SemanticSearchArgs { glob: Some("[".into()), ..args("x") }, &deps).unwrap_err();
        assert!(matches!(error, ToolError::InvalidPattern(_)), "{error}");
        assert!(asked.lock().unwrap().is_empty());
    }

    #[test]
    fn a_failed_search_is_reported_as_unavailable() {
        let deps = ToolDeps { search: Some(Arc::new(|_: &[&str], _: Option<&[String]>, _: usize, _: &SearchFilter| Err("disk".into()))), ..ToolDeps::default() };
        let error = semantic_search(&args("x"), &deps).unwrap_err();
        assert!(matches!(&error, ToolError::SearchUnavailable(reason) if reason == "disk"), "{error}");
    }

    #[test]
    fn the_query_fts_size_and_docs_reach_the_search() {
        let (deps, asked) = deps_answering("fn a() {}");
        let call = SemanticSearchArgs { fts: Some(vec!["a".into()]), ..args("find a") };
        semantic_search(&call, &deps).unwrap();
        semantic_search(&SemanticSearchArgs { top_k: Some(3), include_docs: Some(true), ..args("b") }, &deps).unwrap();
        assert_eq!(
            *asked.lock().unwrap(),
            [
                ("find a".to_string(), Some(vec!["a".to_string()]), DEFAULT_TOP_K, false),
                ("b".to_string(), None, 3, true),
            ]
        );
    }

    #[test]
    fn text_is_short_unless_a_preview_is_asked_for() {
        let long = "й".repeat(PREVIEW_CHARS * 2);
        let (deps, _) = deps_answering(&long);

        let short = text_of(semantic_search(&args("x"), &deps).unwrap());
        let preview = text_of(semantic_search(&SemanticSearchArgs { preview: Some(true), ..args("x") }, &deps).unwrap());

        assert_eq!(short.chars().count(), TEXT_CHARS + 1, "cut on a character, with an ellipsis");
        assert!(short.ends_with('…'));
        assert_eq!(preview.chars().count(), PREVIEW_CHARS + 1);
        let (deps, _) = deps_answering("fn a() {}");
        assert_eq!(text_of(semantic_search(&args("x"), &deps).unwrap()), "fn a() {}", "short text is left alone");
    }

    /// What the model reads back.
    #[test]
    fn the_result_is_flat_on_the_wire() {
        let (deps, _) = deps_answering("fn a() {}");
        let wire = serde_json::to_value(semantic_search(&args("x"), &deps).unwrap()).unwrap();
        assert_eq!(
            wire,
            serde_json::json!({
                "result": "searchResults",
                "matches": [{ "path": "a.rs", "startLine": 3, "endLine": 9, "name": "a", "text": "fn a() {}", "source": "symbol" }],
                "meta": { "tiersUsed": ["symbol"], "weak": false, "hint": null }
            })
        );
    }
}
