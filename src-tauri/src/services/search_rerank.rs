//! `semanticSearch`'s second pass: the active model reorders the first
//! [`POOL`] candidates the index ranked, in one request with no tools and no
//! history, as a chat's title is asked for. The index finds the right file
//! within 30 far more often than it puts it first (search bench: 84 of 93
//! against 37), and the model reading question and code together closes most
//! of that gap — MRR 0.546 → 0.786, `docs/13-reranker-research.md`. A failed or
//! empty reply leaves the index's order: the pass only ever reorders.

use std::sync::Arc;

use crate::domain::code_search::CodeMatch;
use crate::domain::llm::{ChatRequest, LlmMessage};
use crate::domain::tools::CodeSearchFn;
use crate::infra::llm_debug_log;
use crate::services::llm_session::LlmSession;

/// How many of the index's results the model is shown.
pub const POOL: usize = 30;
/// Text per candidate: enough to tell what the code does, not all of it.
const CANDIDATE_CHARS: usize = 400;

const PROMPT: &str = "You rank code search results for a question about a repository. \
Reply with the numbers of the candidates that best answer it, best first, comma-separated, at most 10. \
Prefer the code that does what the question asks over code that only mentions it. Nothing else.";

/// `search` with the model's pass on top: asks the index for at least
/// [`POOL`], reorders, and keeps as many as were asked for.
pub fn reranking(search: CodeSearchFn, session: Arc<LlmSession>) -> CodeSearchFn {
    Arc::new(move |wordings, fts, top_k, filter| {
        let mut result = search(wordings, fts, top_k.max(POOL), filter)?;
        result.matches = rerank(&session, wordings, std::mem::take(&mut result.matches));
        result.matches.truncate(top_k);
        Ok(result)
    })
}

/// `matches` with the first [`POOL`] reordered by the model: those it named,
/// in its order, then the rest as the index had them.
pub fn rerank(session: &LlmSession, wordings: &[&str], mut matches: Vec<CodeMatch>) -> Vec<CodeMatch> {
    let shown = matches.len().min(POOL);
    if shown < 2 {
        return matches;
    }
    let request = ChatRequest {
        messages: vec![LlmMessage::system(PROMPT), LlmMessage::user(render(wordings, &matches[..shown]))],
        tools: Vec::new(),
        model: session.model.clone(),
    };
    llm_debug_log::log_request(session.debug_logging, &session.provider_id, 0, &request);
    let response = session.provider.chat(request);
    llm_debug_log::log_response(session.debug_logging, &session.provider_id, 0, &response);
    // A failure costs the pass, not the search.
    let Ok(reply) = response else { return matches };
    let order = order(&reply.content.unwrap_or_default(), shown);
    let rest = matches.split_off(shown);
    let mut slots: Vec<Option<CodeMatch>> = matches.into_iter().map(Some).collect();
    let named: Vec<CodeMatch> = order.into_iter().filter_map(|i| slots[i].take()).collect();
    named.into_iter().chain(slots.into_iter().flatten()).chain(rest).collect()
}

fn render(wordings: &[&str], candidates: &[CodeMatch]) -> String {
    let mut text = format!("Question: {}\n\n", wordings.join(" / "));
    for (i, m) in candidates.iter().enumerate() {
        let body: String = m.text.chars().take(CANDIDATE_CHARS).collect();
        text.push_str(&format!(
            "[{}] {}:{}-{} {}\n{body}\n\n",
            i + 1,
            m.path,
            m.start_line,
            m.end_line,
            m.name.as_deref().unwrap_or("")
        ));
    }
    text
}

/// The 0-based candidates a reply names, in its order, each once; numbers
/// outside `1..=shown` are dropped.
fn order(reply: &str, shown: usize) -> Vec<usize> {
    let mut picked = Vec::new();
    for n in reply.split(|c: char| !c.is_ascii_digit()).filter_map(|w| w.parse::<usize>().ok()) {
        if (1..=shown).contains(&n) && !picked.contains(&(n - 1)) {
            picked.push(n - 1);
        }
    }
    picked
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::code_search::{CodeSearchResult, MatchSource, SearchFilter, SearchMeta};
    use crate::domain::llm::{ChatResponse, ChatStreamResult, LlmError, LlmModelInfo, LlmProvider, StreamEvent};
    use std::sync::Mutex;

    /// Answers every request with `answer`, keeping what it was asked.
    struct Ranker {
        answer: Result<String, String>,
        asked: Mutex<Vec<ChatRequest>>,
    }

    impl LlmProvider for Ranker {
        fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
            self.asked.lock().unwrap().push(request);
            match &self.answer {
                Ok(text) => Ok(ChatResponse { content: Some(text.clone()), tool_calls: Vec::new(), usage: None }),
                Err(e) => Err(LlmError::Provider(e.clone())),
            }
        }
        fn chat_stream(&self, _: ChatRequest, _: &dyn Fn(StreamEvent), _: &dyn Fn() -> bool) -> Result<ChatStreamResult, LlmError> {
            unreachable!("a rerank is never streamed")
        }
        fn list_models(&self) -> Result<Vec<LlmModelInfo>, LlmError> {
            unreachable!("a rerank never lists models")
        }
    }

    fn session(answer: Result<&str, &str>) -> (Arc<LlmSession>, Arc<Ranker>) {
        let ranker = Arc::new(Ranker { answer: answer.map(str::to_string).map_err(str::to_string), asked: Mutex::default() });
        let session = LlmSession {
            provider: ranker.clone(),
            provider_id: "test".to_string(),
            model: "m".to_string(),
            debug_logging: false,
            context_limit: None,
            reply_language: None,
            limits: Default::default(),
        };
        (Arc::new(session), ranker)
    }

    fn found(n: usize) -> Vec<CodeMatch> {
        (1..=n)
            .map(|i| CodeMatch {
                path: format!("f{i}.rs"),
                start_line: 1,
                end_line: 2,
                name: None,
                text: format!("fn f{i}() {{}}"),
                source: MatchSource::Semantic,
            })
            .collect()
    }

    fn paths(matches: &[CodeMatch]) -> Vec<&str> {
        matches.iter().map(|m| m.path.as_str()).collect()
    }

    #[test]
    fn what_the_model_named_comes_first_and_the_rest_keeps_its_order() {
        let (session, ranker) = session(Ok("3, 1"));
        let ranked = rerank(&session, &["where is f3"], found(4));
        assert_eq!(paths(&ranked), ["f3.rs", "f1.rs", "f2.rs", "f4.rs"]);
        let asked = &ranker.asked.lock().unwrap()[0];
        assert!(asked.tools.is_empty());
        let shown = asked.messages[1].content.as_deref().unwrap_or_default();
        assert!(shown.starts_with("Question: where is f3") && shown.contains("[4] f4.rs:1-2"), "{shown}");
    }

    #[test]
    fn a_reply_is_read_for_numbers_in_range_each_once() {
        assert_eq!(order("[2], 2, 7, 0, 1.", 3), [1, 0]);
        assert_eq!(order("none of them", 3), Vec::<usize>::new());
    }

    /// Only the first [`POOL`] are shown; what lies past them stays behind them.
    #[test]
    fn past_the_pool_nothing_moves_and_nothing_is_shown() {
        let (session, ranker) = session(Ok(&POOL.to_string()));
        let ranked = rerank(&session, &["q"], found(POOL + 2));
        assert_eq!(ranked[0].path, format!("f{POOL}.rs"));
        assert_eq!(paths(&ranked[POOL..]), [format!("f{}.rs", POOL + 1), format!("f{}.rs", POOL + 2)]);
        let shown = ranker.asked.lock().unwrap()[0].messages[1].content.clone().unwrap_or_default();
        assert!(shown.contains(&format!("[{POOL}] ")) && !shown.contains(&format!("[{}] ", POOL + 1)), "{shown}");
    }

    #[test]
    fn a_failed_reply_or_a_single_match_keeps_the_index_order() {
        let (failing, _) = session(Err("down"));
        assert_eq!(paths(&rerank(&failing, &["q"], found(3))), ["f1.rs", "f2.rs", "f3.rs"]);
        let (session, ranker) = session(Ok("1"));
        assert_eq!(paths(&rerank(&session, &["q"], found(1))), ["f1.rs"]);
        assert!(ranker.asked.lock().unwrap().is_empty(), "one match is not worth a request");
    }

    /// The index is asked for the pool, and the tool gets what it asked for.
    #[test]
    fn the_search_asks_for_the_pool_and_returns_what_was_asked() {
        let asked_for = Arc::new(Mutex::new(0));
        let seen = asked_for.clone();
        let index: CodeSearchFn = Arc::new(move |_, _, top_k, _| {
            *seen.lock().unwrap() = top_k;
            Ok(CodeSearchResult {
                matches: found(top_k),
                meta: SearchMeta { tiers_used: vec![], weak: false, hint: None },
            })
        });
        let (session, _) = session(Ok("5"));
        let result = reranking(index, session)(&["q"], None, 3, &SearchFilter::default()).unwrap();
        assert_eq!(*asked_for.lock().unwrap(), POOL);
        assert_eq!(paths(&result.matches), ["f5.rs", "f1.rs", "f2.rs"]);
    }
}
