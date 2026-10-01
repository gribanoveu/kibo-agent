//! Which old tool results to replace with a one-line stub, and when.
//!
//! A file read in round two rides along in every later round of the turn, and
//! in every turn after it: the history is resent whole each time. Replacing
//! what the model has long since used with a note saying it was there costs
//! one re-read if it turns out to be needed again.
//!
//! **Against a prompt cache.** Every provider this app talks to caches by
//! prefix — DeepSeek from token zero, Anthropic up to its cache points — so
//! changing an old message makes everything after it a cache miss, once.
//! Hence the shape of the rule:
//!
//! * **Rarely.** Nothing happens until the request fills [`TRIGGER_PERCENT`]
//!   of the model's window — the same estimate the context meter shows and
//!   compaction reads. Compaction folds a conversation at 80% before a turn
//!   starts; inside a turn nothing folds, and clearing is what keeps a long
//!   one inside the window. Below it a miss costs more than the
//!   tokens cleared would save: on DeepSeek a cached input token is ~25×
//!   cheaper than a missed one, so clearing pays back through the rounds that
//!   follow, not at once. A fixed trigger of 60k cleared a 260k window a
//!   quarter full, and a model documenting a repository read the same files
//!   three and four times over, each copy cleared before it was done.
//! * **In bulk.** When it does happen, everything eligible goes at once, and
//!   only if that is at least [`CLEAR_AT_LEAST_TOKENS`] — one miss per batch,
//!   not one per result per round.
//! * **Deterministically.** A stub is a function of the call alone, so the
//!   new prefix is cached from the next round on like any other.
//!
//! What stays: the last [`KEEP_RECENT_ROUNDS`] rounds' results, anything
//! under [`MIN_RESULT_TOKENS`], a loaded skill (instructions, not data), the
//! results of an MCP server the user marked as kept — the caller's `kept`, for
//! the same reason — and
//! everything the model itself said — its reasoning included, which DeepSeek
//! requires back in full once tools are in play.
//!
//! DeepSeek evaluates its own agents with exactly this move under context
//! pressure — dropping earlier tool-call history outright (V3.2 report,
//! "Discard-all") — so a model trained there is not thrown by a result that
//! is gone.
//!
//! The rules only; applying them, and what else has to forget the cleared
//! results, is `services::llm_chat`'s.

use super::compaction::estimate_text_tokens;
use super::llm::{LlmMessage, LlmRole};

/// How full the window the request must be before anything is cleared.
/// A percentage for the reason compaction's is: the comparison is exact.
// ponytail: clears everything eligible once over it; clearing oldest-first
// down to a target would keep more of what the model may still need, if the
// bench shows re-reads after a clearing.
pub const TRIGGER_PERCENT: u64 = 85;

/// The trigger, in estimated request tokens, for a model whose window is not
/// configured — where there is no fraction to take.
pub const FALLBACK_TRIGGER_TOKENS: usize = 60_000;

/// The least one clearing must free, or it waits: a batch this size is worth
/// the one cache miss it causes.
pub const CLEAR_AT_LEAST_TOKENS: usize = 20_000;

/// Results from this many of the latest tool-calling rounds are never
/// cleared: that is what the model is working with right now.
pub const KEEP_RECENT_ROUNDS: usize = 3;

/// A result smaller than this is left alone — its stub would save little.
pub const MIN_RESULT_TOKENS: usize = 1_000;

/// How a stub begins — how a cleared result is recognised, so it is never
/// cleared twice.
pub const STUB_PREFIX: &str = "[Result cleared to save context:";

/// Tools whose results are never cleared: a skill's result is instructions
/// the model is meant to keep following.
const NEVER_CLEARED: &[&str] = &["skill"];

/// One result to replace, and what with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cleared {
    /// Index into the history.
    pub index: usize,
    /// The call that produced it — name and raw arguments, as the model sent
    /// them.
    pub tool: String,
    pub arguments: String,
    pub stub: String,
}

/// The results to clear now, or none. `request_tokens` is what the next
/// request is estimated to cost whole — instructions and tools included, as
/// `compaction::ContextUsage` counts it — and `context_limit` the window.
pub fn plan(
    history: &[LlmMessage],
    request_tokens: usize,
    context_limit: Option<u32>,
    kept: &dyn Fn(&str) -> bool,
) -> Vec<Cleared> {
    let full = match context_limit.filter(|limit| *limit > 0) {
        Some(limit) => request_tokens as u64 * 100 >= u64::from(limit) * TRIGGER_PERCENT,
        None => request_tokens >= FALLBACK_TRIGGER_TOKENS,
    };
    if !full {
        return Vec::new();
    }
    let recent_from = recent_rounds_start(history);
    let candidates: Vec<(Cleared, usize)> = history[..recent_from]
        .iter()
        .enumerate()
        .filter(|(_, m)| m.role == LlmRole::Tool)
        .filter_map(|(index, message)| {
            let content = message.content.as_deref()?;
            let tokens = estimate_text_tokens(content);
            if tokens < MIN_RESULT_TOKENS || content.starts_with(STUB_PREFIX) {
                return None;
            }
            let id = message.tool_call_id.as_deref()?;
            let call = history[..index].iter().rev().flat_map(|m| &m.tool_calls).find(|c| c.id == id)?;
            if NEVER_CLEARED.contains(&call.name.as_str()) || kept(&call.name) {
                return None;
            }
            let stub = stub(&call.name, &call.arguments, tokens);
            Some((Cleared { index, tool: call.name.clone(), arguments: call.arguments.clone(), stub }, tokens))
        })
        .collect();
    let freed: usize = candidates.iter().map(|(c, tokens)| tokens - estimate_text_tokens(&c.stub)).sum();
    if freed < CLEAR_AT_LEAST_TOKENS {
        return Vec::new();
    }
    candidates.into_iter().map(|(cleared, _)| cleared).collect()
}

/// Where the protected tail begins: the assistant message that opened the
/// [`KEEP_RECENT_ROUNDS`]-th latest tool-calling round, or the start.
fn recent_rounds_start(history: &[LlmMessage]) -> usize {
    history
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, m)| m.role == LlmRole::Assistant && !m.tool_calls.is_empty())
        .nth(KEEP_RECENT_ROUNDS - 1)
        .map_or(0, |(i, _)| i)
}

/// Names the call so the model can make it again. Arguments are cut, not
/// dropped: `readFile` of which file is the useful half.
fn stub(tool: &str, arguments: &str, tokens: usize) -> String {
    let arguments: String = arguments.chars().take(200).collect();
    format!(
        "{STUB_PREFIX} {tool} {arguments} returned about {tokens} tokens here, several rounds ago. \
         Call it again if you need it — and read a file again before editing it.]"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::compaction::estimate_tokens;
    use crate::domain::llm::LlmToolCall;

    fn call(id: &str, name: &str, arguments: &str) -> LlmMessage {
        let mut message = LlmMessage::assistant("");
        message.content = None;
        message.tool_calls = vec![LlmToolCall { id: id.into(), name: name.into(), arguments: arguments.into() }];
        message
    }

    fn tokens(n: usize) -> String {
        "x".repeat(n * 4)
    }

    /// `rounds` tool-calling rounds, each reading `size` tokens, after a
    /// question.
    fn turn(rounds: usize, size: usize) -> Vec<LlmMessage> {
        let mut history = vec![LlmMessage::user("fix it")];
        for n in 0..rounds {
            let id = format!("c{n}");
            history.push(call(&id, "readFile", &format!(r#"{{"path":"f{n}.txt"}}"#)));
            history.push(LlmMessage::tool_result(&id, tokens(size)));
        }
        history
    }

    /// What the request costs: the history alone, in these tests.
    fn plan_of(history: &[LlmMessage], limit: Option<u32>) -> Vec<Cleared> {
        plan(history, estimate_tokens(history), limit, &|_| false)
    }

    /// Plenty that is old and big enough to go — but the request is under the
    /// trigger, and a cache miss would cost more than what clearing saves.
    #[test]
    fn nothing_is_cleared_below_the_trigger() {
        let history = turn(8, 10_000);
        let tokens = estimate_tokens(&history);
        let window = |percent: u64| Some((tokens as u64 * 100 / percent) as u32);
        assert_eq!(plan_of(&history, window(TRIGGER_PERCENT - 1)), vec![], "under the trigger of a larger window");
        assert!(!plan_of(&history, window(TRIGGER_PERCENT)).is_empty(), "at the trigger, the old results go");
    }

    /// The same history a 260k window holds easily is not cleared, where a
    /// fixed 60k trigger cleared it: the size that matters is the window's.
    #[test]
    fn a_large_window_keeps_what_a_small_one_clears() {
        let history = turn(8, 10_000);
        assert!(estimate_tokens(&history) >= FALLBACK_TRIGGER_TOKENS);
        assert_eq!(plan_of(&history, Some(260_000)), vec![]);
        assert!(!plan_of(&history, Some(90_000)).is_empty(), "nearly full");
    }

    /// Without a window there is no fraction: the fixed trigger stands in.
    #[test]
    fn without_a_window_the_fallback_trigger_decides() {
        let history = turn(8, 6_000);
        assert!(estimate_tokens(&history) < FALLBACK_TRIGGER_TOKENS);
        assert_eq!(plan_of(&history, None), vec![]);
        assert!(!plan(&history, FALLBACK_TRIGGER_TOKENS, None, &|_| false).is_empty(), "the instructions count too");
    }

    /// Over the trigger, every old result goes in one batch; the latest
    /// rounds' stay.
    #[test]
    fn over_the_trigger_old_results_go_together_and_recent_ones_stay() {
        let history = turn(8, 10_000);
        let cleared = plan_of(&history, None);
        let indices: Vec<usize> = cleared.iter().map(|c| c.index).collect();
        // Rounds 0..5 are old; 5, 6 and 7 are the last three.
        assert_eq!(indices, vec![2, 4, 6, 8, 10]);
        assert_eq!(cleared[0].tool, "readFile");
        assert_eq!(cleared[0].arguments, r#"{"path":"f0.txt"}"#);
        assert!(cleared[0].stub.starts_with(STUB_PREFIX));
        assert!(cleared[0].stub.contains(r#"readFile {"path":"f0.txt"}"#), "{}", cleared[0].stub);
    }

    /// Over the trigger, but too little to free: wait for a batch worth the
    /// cache miss.
    #[test]
    fn a_batch_too_small_to_be_worth_a_miss_waits() {
        let mut history = turn(3, 1_500);
        // Big, but recent: not eligible.
        for n in 10..13 {
            let id = format!("c{n}");
            history.push(call(&id, "readFile", "{}"));
            history.push(LlmMessage::tool_result(&id, tokens(20_000)));
        }
        assert!(estimate_tokens(&history) >= FALLBACK_TRIGGER_TOKENS);
        assert_eq!(plan_of(&history, None), vec![]);
    }

    #[test]
    fn small_results_skills_and_stubs_are_left_alone() {
        let mut history = turn(0, 0);
        history.push(call("s", "skill", "{}"));
        history.push(LlmMessage::tool_result("s", tokens(30_000)));
        history.push(call("small", "grep", "{}"));
        history.push(LlmMessage::tool_result("small", tokens(MIN_RESULT_TOKENS - 1)));
        history.push(call("done", "readFile", "{}"));
        history.push(LlmMessage::tool_result("done", format!("{STUB_PREFIX} {}", tokens(30_000))));
        history.push(call("big", "runCommand", "{}"));
        history.push(LlmMessage::tool_result("big", tokens(30_000)));
        history.extend(turn(3, 10).into_iter().skip(1));

        let cleared: Vec<String> = plan_of(&history, None).into_iter().map(|c| c.tool).collect();
        assert_eq!(cleared, vec!["runCommand"]);
    }

    /// A tool the caller keeps is left alone like a skill — an MCP server
    /// marked `keepResults`, whose answers are templates to follow.
    #[test]
    fn a_kept_tool_is_never_cleared() {
        let mut history = turn(0, 0);
        history.push(call("t", "mcp__kb__get_doc_template", "{}"));
        history.push(LlmMessage::tool_result("t", tokens(30_000)));
        history.push(call("big", "runCommand", "{}"));
        history.push(LlmMessage::tool_result("big", tokens(30_000)));
        history.extend(turn(3, 10).into_iter().skip(1));

        let kept = |name: &str| name.starts_with("mcp__kb__");
        let cleared: Vec<String> = plan(&history, estimate_tokens(&history), None, &kept).into_iter().map(|c| c.tool).collect();
        assert_eq!(cleared, vec!["runCommand"]);
    }

    /// A stub says the same thing every time for the same call — the prefix
    /// after a clearing caches like any other.
    #[test]
    fn a_stub_is_a_function_of_the_call() {
        let history = turn(8, 10_000);
        assert_eq!(plan_of(&history, None), plan_of(&history, None));
    }
}
