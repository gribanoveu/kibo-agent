//! Counters the app keeps per local calendar day — what the model spent, by
//! which model and at what hour, the chats started and where, the prompts
//! sent and the tools called — read by the Usage pane.
//!
//! One name per counter rather than a column each, so the store
//! (`infra/daily_metrics.rs`) holds a new one without a migration: a counter
//! is a variant here and a call to `record` where it happens. A counter kept
//! per something — tokens per model, chats per folder — says which in its
//! `key`; the rest leave it empty.

use serde::{Deserialize, Serialize};

use crate::domain::llm::ChatUsage;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Metric {
    /// Everything sent to the model, `CachedTokens` included.
    PromptTokens,
    /// The part of `PromptTokens` the provider read from its cache.
    CachedTokens,
    CompletionTokens,
    /// Prompt and completion together, keyed by the model that spent them.
    ModelTokens,
    /// Prompt and completion together, keyed by the local hour, `00`–`23`.
    HourTokens,
    /// Chats started, keyed by the folder — `chat_record::NO_FOLDER` for Chat mode.
    Sessions,
    /// Messages the user sent to start a turn.
    Prompts,
    /// Tool calls settled, keyed by the tool's name — a helper's included.
    ToolCalls,
}

impl Metric {
    pub const ALL: [Metric; 8] = [
        Metric::PromptTokens,
        Metric::CachedTokens,
        Metric::CompletionTokens,
        Metric::ModelTokens,
        Metric::HourTokens,
        Metric::Sessions,
        Metric::Prompts,
        Metric::ToolCalls,
    ];

    /// The name stored in the table. Never change one: the rows already
    /// written under it would stop being read.
    pub fn as_str(self) -> &'static str {
        match self {
            Metric::PromptTokens => "promptTokens",
            Metric::CachedTokens => "cachedTokens",
            Metric::CompletionTokens => "completionTokens",
            Metric::ModelTokens => "modelTokens",
            Metric::HourTokens => "hourTokens",
            Metric::Sessions => "sessions",
            Metric::Prompts => "prompts",
            Metric::ToolCalls => "toolCalls",
        }
    }

    pub fn parse(text: &str) -> Option<Metric> {
        Metric::ALL.into_iter().find(|m| m.as_str() == text)
    }
}

/// One counter on one day.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyMetric {
    /// `YYYY-MM-DD`, the user's local date.
    pub day: String,
    pub metric: Metric,
    /// What the counter is kept per — the model, for `ModelTokens`; empty otherwise.
    pub key: String,
    pub value: u64,
}

/// One addition to a counter: which, per what (empty for none), and how much.
pub type Count<'a> = (Metric, &'a str, u64);

/// What one answer from `model`, arriving in local `hour` (`00`–`23`), adds.
pub fn of_usage<'a>(model: &'a str, hour: &'a str, usage: &ChatUsage) -> [Count<'a>; 5] {
    let (prompt, completion) = (u64::from(usage.prompt_tokens), u64::from(usage.completion_tokens));
    [
        (Metric::PromptTokens, "", prompt),
        (Metric::CachedTokens, "", u64::from(usage.cached_tokens)),
        (Metric::CompletionTokens, "", completion),
        (Metric::ModelTokens, model, prompt + completion),
        (Metric::HourTokens, hour, prompt + completion),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_metric_parses_back_from_its_name() {
        for metric in Metric::ALL {
            assert_eq!(Metric::parse(metric.as_str()), Some(metric));
        }
        assert_eq!(Metric::parse("linesWritten"), None);
        // Stored names are forever: a rename here would orphan every row.
        assert_eq!(
            Metric::ALL.map(Metric::as_str),
            ["promptTokens", "cachedTokens", "completionTokens", "modelTokens", "hourTokens", "sessions", "prompts", "toolCalls"]
        );
    }

    #[test]
    fn a_round_adds_each_part_of_its_usage_to_its_own_counter() {
        let usage = ChatUsage { prompt_tokens: 1000, completion_tokens: 50, total_tokens: 1050, cached_tokens: 800 };
        assert_eq!(
            of_usage("qwen", "14", &usage),
            [
                (Metric::PromptTokens, "", 1000),
                (Metric::CachedTokens, "", 800),
                (Metric::CompletionTokens, "", 50),
                (Metric::ModelTokens, "qwen", 1050),
                (Metric::HourTokens, "14", 1050),
            ]
        );
    }
}
