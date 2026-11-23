//! What one request left behind, so a cache miss can be explained: a
//! fingerprint of the parts a provider's prompt cache keys on, and the
//! comparison with the chat's previous request that names what changed.
//!
//! The miss this exists for is measured, not hypothetical: in November,
//! about thirty misses had no cause anyone could name — the notice said
//! "Cache miss —" and the usage was gone with the round. A fingerprint is
//! hashes only: no prompt text, no tool arguments, no file content ever
//! reaches the row (`docs/08-data-policy.md`).
//!
//! Pure: the fingerprint is taken where the request is assembled, the
//! comparison is a function of two fingerprints, and where the previous one
//! lives and where the row goes is a port ([`RequestMarkers`]) — the chat's
//! own memory in the app, a list in a test.

use serde::{Deserialize, Serialize};

use crate::domain::llm::{ChatUsage, LlmMessage, LlmToolDefinition};

/// The parts of a request a prompt cache keys on, each as a blake3 hex hash.
///
/// `messages` is one hash per history message, in order: a change deep in
/// the history breaks the prefix from that message on, and the index is
/// what the notice names. Appending messages keeps the prefix — the normal
/// growth of a conversation is not a change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestFingerprint {
    /// The system part: the prompt's messages, ahead of the conversation.
    pub system: String,
    /// The tool definitions, in the order they are sent — `toolSearch`
    /// bringing a tool in changes this even when nothing else moves.
    pub tools: String,
    pub messages: Vec<String>,
}

/// Takes the fingerprint of a request's parts. Each part is hashed as its
/// JSON wire form — the bytes the provider actually receives, reasoning
/// blocks and images included, so a change anywhere in a message is seen.
pub fn fingerprint(system: &[LlmMessage], tools: &[LlmToolDefinition], history: &[LlmMessage]) -> RequestFingerprint {
    RequestFingerprint {
        system: hash_of(system),
        tools: hash_of(tools),
        messages: history.iter().map(hash_of).collect(),
    }
}

fn hash_of<T: Serialize + ?Sized>(value: &T) -> String {
    // These types are strings and JSON values: serialization cannot fail.
    // The fallback is an invariant's belt, not a path — an empty hash would
    // compare equal, so it must never be reached.
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    blake3::hash(&bytes).to_hex().to_string()
}

/// Why the shared prefix was paid for again, as far as our own request can
/// say. The order is the request's: the system part comes first, then the
/// tools, then the conversation — a change earlier invalidates everything
/// after it, so it is the one named.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MissCause {
    /// The system prompt changed — the whole conversation behind it is new
    /// to the cache.
    System,
    /// The tool set changed (`toolSearch` found a tool, a server connected).
    Tools,
    /// A message of the history changed, or the history got shorter (a fold,
    /// a cleared result): the prefix breaks at `from`, 0-based, of `of`
    /// messages the previous request had.
    History { from: usize, of: usize },
    /// Nothing we send changed — the prefix is the previous request's own,
    /// possibly grown. The cache was lost on the provider's side.
    Unchanged,
}

/// Compares the previous request's fingerprint with this one's.
pub fn compare(previous: &RequestFingerprint, current: &RequestFingerprint) -> MissCause {
    if previous.system != current.system {
        return MissCause::System;
    }
    if previous.tools != current.tools {
        return MissCause::Tools;
    }
    let shared = previous.messages.len().min(current.messages.len());
    if let Some(at) = (0..shared).find(|&i| previous.messages[i] != current.messages[i]) {
        return MissCause::History { from: at, of: previous.messages.len() };
    }
    if current.messages.len() < previous.messages.len() {
        return MissCause::History { from: current.messages.len(), of: previous.messages.len() };
    }
    MissCause::Unchanged
}

/// One request's row, as the service hands it to the port: what it cost,
/// what it carried, and what changed. The chat, the turn and the time are
/// the adapter's to add — the loop knows none of them.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestRecord {
    pub round: u32,
    pub provider_id: String,
    pub model: String,
    /// `None` when the provider reported no usage, or the request failed.
    pub usage: Option<ChatUsage>,
    pub fingerprint: RequestFingerprint,
    /// `None` for the chat's first request: nothing to compare with.
    pub cause: Option<MissCause>,
}

/// Where a chat's request markers live: the previous fingerprint in memory
/// (it outlives a turn — the next turn's first request compares with the
/// last turn's last), and the row on disk. One per chat; a helper's turn and
/// a test have none.
pub trait RequestMarkers: Send + Sync {
    fn previous(&self) -> Option<RequestFingerprint>;
    /// Remembers the fingerprint and writes the row. Best-effort on disk:
    /// a marker lost is a line of analysis lost, never the turn.
    fn record(&self, record: RequestRecord);
    /// The last request that reported its cost, as the meter's anchor
    /// (docs/60). `None` until a request succeeds with usage — a helper's
    /// turn and a test have none, and their cost is the full estimate.
    fn last_anchor(&self) -> Option<Anchor>;
}

/// The last request's real cost and what it carried: the anchor the meter
/// and the compaction trigger trust while the conversation only grows
/// (docs/60). Hashes and a number, as the marker is — no content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    pub prompt_tokens: u32,
    pub fingerprint: RequestFingerprint,
    pub provider_id: String,
    pub model: String,
}

impl Anchor {
    /// The anchor of a recorded request, if the provider reported its cost.
    /// A failed request, or one with no usage, anchors nothing; nor does a
    /// usage of zeros — a gateway's empty usage object parses to them, and a
    /// zero anchor would read the whole conversation as free.
    pub fn of(record: &RequestRecord) -> Option<Anchor> {
        record.usage.filter(|usage| usage.prompt_tokens > 0).map(|usage| Anchor {
            prompt_tokens: usage.prompt_tokens,
            fingerprint: record.fingerprint.clone(),
            provider_id: record.provider_id.clone(),
            model: record.model.clone(),
        })
    }

    /// Whether the anchor still describes the request about to be sent: the
    /// same provider and model (another tokenizer counts differently), and a
    /// prefix that only grew. A rewritten history, a new system part or tool
    /// set invalidates it — a fold's stale count must not be shown, which is
    /// pi's post-compaction guard generalized.
    pub fn holds_for(&self, provider_id: &str, model: &str, current: &RequestFingerprint) -> bool {
        self.provider_id == provider_id
            && self.model == model
            && compare(&self.fingerprint, current) == MissCause::Unchanged
    }

    /// The messages added after the anchored request — the only part the
    /// estimate still measures. Clamped: a shorter history cannot hold the
    /// anchor anyway ([`Anchor::holds_for`] refuses it), but the slice must
    /// not panic if asked alone.
    pub fn trailing<'a>(&self, history: &'a [LlmMessage]) -> &'a [LlmMessage] {
        &history[self.fingerprint.messages.len().min(history.len())..]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(system: &str, tools: &str, messages: &[&str]) -> RequestFingerprint {
        RequestFingerprint {
            system: system.into(),
            tools: tools.into(),
            messages: messages.iter().map(|m| m.to_string()).collect(),
        }
    }

    /// Each cause, on its own: the earliest changed part is the one named.
    #[test]
    fn the_cause_is_the_earliest_changed_part() {
        let base = fp("s", "t", &["a", "b", "c"]);
        assert_eq!(compare(&base, &fp("s2", "t", &["a", "b", "c"])), MissCause::System);
        assert_eq!(compare(&base, &fp("s", "t2", &["a", "b", "c"])), MissCause::Tools);
        assert_eq!(compare(&base, &fp("s", "t", &["a", "X", "c"])), MissCause::History { from: 1, of: 3 });
        // A change deep in the history names its own index, not the first.
        assert_eq!(compare(&base, &fp("s", "t", &["a", "b", "X"])), MissCause::History { from: 2, of: 3 });
        // System wins over everything behind it.
        assert_eq!(compare(&base, &fp("s2", "t2", &["X"])) , MissCause::System);
    }

    /// Growing the conversation is what every round does: the prefix is
    /// intact, and a miss with it is the provider's, not ours.
    #[test]
    fn an_appended_message_is_not_a_change() {
        let base = fp("s", "t", &["a", "b"]);
        assert_eq!(compare(&base, &fp("s", "t", &["a", "b", "c"])), MissCause::Unchanged);
        assert_eq!(compare(&base, &base), MissCause::Unchanged);
    }

    /// A fold or a cleared result shortens the history: the prefix breaks
    /// where the cut is.
    #[test]
    fn a_shorter_history_breaks_at_the_cut() {
        let base = fp("s", "t", &["a", "b", "c"]);
        assert_eq!(compare(&base, &fp("s", "t", &["a", "b"])), MissCause::History { from: 2, of: 3 });
        assert_eq!(compare(&base, &fp("s", "t", &[])), MissCause::History { from: 0, of: 3 });
    }

    /// The fingerprint is of the wire form: a message's reasoning blocks and
    /// a tool's schema are part of what the provider receives, and a change
    /// in either is a change.
    #[test]
    fn the_fingerprint_sees_the_whole_wire_form() {
        let mut a = LlmMessage::user("go");
        let mut b = LlmMessage::user("go");
        let base = fingerprint(&[], &[], &[a.clone()]);
        assert_eq!(base, fingerprint(&[], &[], &[b.clone()]));
        b.native_content = Some(serde_json::json!({"thinking": "hm"}));
        assert_ne!(base.messages, fingerprint(&[], &[], &[b]).messages);
        a.content = Some("go!".into());
        assert_ne!(base.messages, fingerprint(&[], &[], &[a]).messages);
        // The system part and the tools are hashed apart from the history.
        let sys = fingerprint(&[LlmMessage::user("prompt")], &[], &[]);
        assert_ne!(sys.system, fingerprint(&[], &[], &[]).system);
        assert_eq!(sys.messages, Vec::<String>::new());
    }

    fn record_with_usage(prompt: u32) -> RequestRecord {
        RequestRecord {
            round: 1,
            provider_id: "p".into(),
            model: "m".into(),
            usage: Some(ChatUsage { prompt_tokens: prompt, completion_tokens: 2, total_tokens: prompt + 2, cached_tokens: 0 }),
            fingerprint: fp("s", "t", &["a", "b"]),
            cause: None,
        }
    }

    /// A request that reported no cost anchors nothing: there is no truth to
    /// hold on to, and the estimate stays the whole number.
    #[test]
    fn a_record_without_usage_anchors_nothing() {
        assert!(Anchor::of(&RequestRecord { usage: None, ..record_with_usage(500) }).is_none());
        assert_eq!(Anchor::of(&record_with_usage(500)).map(|a| a.prompt_tokens), Some(500));
        // A gateway's empty usage object parses to zeros: not a cost, not an anchor.
        assert!(Anchor::of(&record_with_usage(0)).is_none());
    }

    /// The anchor holds while the conversation only grows under the same
    /// model; every other change — system, tools, a rewritten history, another
    /// tokenizer — invalidates it.
    #[test]
    fn the_anchor_holds_only_for_a_grown_prefix_under_the_same_model() {
        let anchor = Anchor::of(&record_with_usage(500)).unwrap();
        assert!(anchor.holds_for("p", "m", &fp("s", "t", &["a", "b", "c"])));
        assert!(anchor.holds_for("p", "m", &fp("s", "t", &["a", "b"])));
        assert!(!anchor.holds_for("p", "m", &fp("s2", "t", &["a", "b", "c"])));
        assert!(!anchor.holds_for("p", "m", &fp("s", "t2", &["a", "b", "c"])));
        assert!(!anchor.holds_for("p", "m", &fp("s", "t", &["a", "X", "c"])));
        assert!(!anchor.holds_for("p", "m", &fp("s", "t", &["a"])));
        assert!(!anchor.holds_for("p", "m2", &fp("s", "t", &["a", "b", "c"])));
        assert!(!anchor.holds_for("p2", "m", &fp("s", "t", &["a", "b", "c"])));
    }

    /// The trailing part is what the estimate measures: the messages after
    /// the anchored request, and nothing when nothing was added.
    #[test]
    fn the_trailing_part_is_what_was_added_after_the_anchor() {
        let anchor = Anchor::of(&record_with_usage(500)).unwrap();
        let history = vec![LlmMessage::user("a"), LlmMessage::user("b"), LlmMessage::user("c")];
        assert_eq!(anchor.trailing(&history), &history[2..]);
        assert!(anchor.trailing(&history[..2]).is_empty());
        assert!(anchor.trailing(&history[..1]).is_empty(), "a shorter history clamps, not panics");
    }
}
