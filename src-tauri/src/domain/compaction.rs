//! When a conversation no longer fits, and what to fold away.
//!
//! The rules only — no summarizing happens here, and nothing is called. The
//! summary itself costs a request to the model, which belongs to a service;
//! what belongs here is the arithmetic that decides whether that request is
//! worth making and which messages it is about.
//!
//! Ported from Alfa Atlas's `src/lib/contextCompaction.ts`, which lived in the
//! frontend. Two things changed in the move, both because the history here is
//! the wire format rather than a transcript of blocks:
//!
//! * **No summary cache.** Upstream kept the whole conversation and rebuilt
//!   the wire history on every turn, so it cached the last summary to avoid
//!   re-summarizing from scratch. Here the compacted history *is* the history
//!   from then on — what the reader sees is a separate list of blocks — so the
//!   previous summary is simply the message the next pass starts from.
//! * **A cut cannot fall anywhere.** A tool result is a message that only
//!   makes sense after the assistant message that asked for it; a provider
//!   rejects a history whose first message is an answer to a question that is
//!   no longer there. Upstream had no such rule to break, because its
//!   messages carried their tool calls inside them.

use super::image::ImagePart;
use super::llm::{LlmMessage, LlmRole, LlmToolDefinition};
use super::tools::{ToolName, TOOL_DENIED_PREFIX, TOOL_ERROR_PREFIX, TOOL_NOT_RUN_PREFIX};
use std::collections::{BTreeSet, HashMap, HashSet};

/// Compaction starts once the estimate crosses this much of the context
/// window. Early enough that the rest of the turn — a tool-calling loop can
/// add a lot in one round — still fits, and early enough to absorb what the
/// estimate below is known to miss.
///
/// A percentage rather than a ratio because the comparison is then exact:
/// `10_000 * 0.8` is not 8_000 in either float width, so a threshold written
/// that way fires one token later than it reads, and says so only under a
/// test that lands exactly on it.
pub const TRIGGER_PERCENT: u64 = 90;

/// How much of the window the verbatim tail may take. The summary is for
/// what the conversation was about; the tail is for what it is doing right
/// now, and that part has to survive word for word.
///
/// Measured in tokens, not messages: one round of an agent is its call and up
/// to half a dozen results, so a tail of twelve messages kept a round or two
/// and the model went back to read again every file it had been working with.
pub const KEEP_TAIL_PERCENT: u64 = 25;

/// The tail after a real overflow, rather than a predicted one: the provider
/// has already refused, so a pass that compacts as gently as the one that
/// failed to prevent it would just fail again.
pub const RETRY_KEEP_TAIL_PERCENT: u64 = 10;

/// How much of the window the summarizer's request may fill: the folded part
/// is shown to it whole when it fits, and cut evenly when it does not.
pub const SUMMARY_INPUT_PERCENT: u64 = 50;

/// `percent` of the window, in tokens. A window that is not configured counts
/// as the default one: the tail and the summarizer's share need a size, and
/// no compaction runs on its own without a configured one.
pub fn share_of_window(context_limit: Option<u32>, percent: u64) -> usize {
    let limit = context_limit.filter(|limit| *limit > 0).unwrap_or(crate::domain::settings::DEFAULT_CONTEXT_LIMIT);
    (u64::from(limit) * percent / 100) as usize
}

/// What the summary message says it is. The model is told plainly that it is
/// reading a summary rather than a transcript — a summary presented as the
/// real conversation invites it to quote things nobody said.
pub const SUMMARY_PREFIX: &str = "[Compacted summary of earlier conversation]";

/// Around every tool call: its id, the `{"type":"function"…}` envelope, and
/// the `role`/`tool_call_id` of the message that answers it. Small on its own,
/// and there are dozens in a working turn.
const TOOL_CALL_OVERHEAD_CHARS: usize = 60;

/// Around every message: the role, the braces, the commas.
const MESSAGE_OVERHEAD_TOKENS: usize = 4;

/// The rule of thumb, and the reason this is called an estimate.
const CHARS_PER_TOKEN: usize = 4;

/// What the next request will cost, near enough to decide with.
///
/// Four characters to a token is the English rule of thumb. Cyrillic packs
/// more tokens into the same characters, so this **underestimates** exactly
/// where a long conversation is most likely to be — which is why the trigger
/// ratio leaves room rather than sitting at the edge.
pub fn estimate_tokens(messages: &[LlmMessage]) -> usize {
    messages.iter().map(estimate_message_tokens).sum()
}

/// The same rule of thumb, over a bare string.
///
/// Public because the two largest things in a request are not messages: the
/// system prompt and the tool schemas.
pub fn estimate_text_tokens(text: &str) -> usize {
    text.len().div_ceil(CHARS_PER_TOKEN)
}

/// What the tool schemas cost, on **every** request.
///
/// They are not messages and not part of the system prompt, so nothing else in
/// this file sees them — and they are not small. Upstream measured its 24
/// advertised tools at ~37 800 characters, about 9 500 tokens, resent verbatim
/// with every request; leaving them out had the estimate running a stable ~36%
/// under the provider's own `promptTokens`.
///
/// Serialized rather than stored as a number: the descriptions are edited
/// where the tools live, and a constant here would be wrong the first time one
/// of them grew a paragraph. Slightly under the wire form, which wraps each
/// entry in `{"type":"function","function":{…}}` — about 40 characters a tool,
/// inside the noise of the estimate itself.
pub fn estimate_tool_schema_tokens(tools: &[LlmToolDefinition]) -> usize {
    if tools.is_empty() {
        return 0;
    }
    match serde_json::to_string(tools) {
        Ok(json) => estimate_text_tokens(&json),
        // An estimate is allowed to be approximate; it is not allowed to take
        // down the turn it is estimating.
        Err(_) => 0,
    }
}

fn estimate_message_tokens(message: &LlmMessage) -> usize {
    let mut chars = message.content.as_deref().map_or(0, str::len);
    chars += message.tool_call_id.as_deref().map_or(0, str::len);
    for call in &message.tool_calls {
        chars += call.name.len() + call.arguments.len() + TOOL_CALL_OVERHEAD_CHARS;
    }
    // A picture by its size, never by its base64: that would count a
    // screenshot as a quarter of a million tokens.
    let images: usize = message.images.iter().map(ImagePart::estimate_tokens).sum();
    MESSAGE_OVERHEAD_TOKENS + chars.div_ceil(CHARS_PER_TOKEN) + images
}

/// What the next request will cost, split the way the window shows it.
///
/// Split by what the reader can do about each part. Compacting shortens
/// `conversation` and nothing else; `instructions` and `tools` are paid
/// whatever the conversation looks like, which is why a nearly-empty chat is
/// not an empty window. `skills` and `mcp` are apart because each is the
/// user's to switch off, and each is both a fixed part — a list, the schemas —
/// and what it brought into the conversation: a loaded skill, a tool's result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextUsage {
    /// The system prompt but the skills list: instructions, the mode, the
    /// project's rules, the plan, this turn's facts.
    pub instructions: usize,
    /// The skills list in the prompt, and every skill loaded since.
    pub skills: usize,
    /// The built-in tools' schemas, as many as the mode offers.
    pub tools: usize,
    /// The MCP servers' tool schemas, and what their tools returned.
    pub mcp: usize,
    /// Everything else the two sides have said.
    pub conversation: usize,
    pub total: usize,
    /// `None` when the window is not configured — the meter then has a number
    /// and no scale, which is the honest picture rather than a guessed one.
    pub limit: Option<u32>,
    /// The total at which a pass starts happening on its own. `None` without a
    /// limit, for the same reason.
    pub compacts_at: Option<usize>,
}

/// What goes in front of the conversation on every request, by part — the
/// same split as [`ContextUsage`]'s fixed half.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RequestFrame {
    pub instructions: usize,
    pub skills: usize,
    pub tools: usize,
    pub mcp: usize,
}

impl RequestFrame {
    pub fn total(&self) -> usize {
        self.instructions + self.skills + self.tools + self.mcp
    }
}

impl ContextUsage {
    pub fn new(frame: RequestFrame, history: &[LlmMessage], limit: Option<u32>) -> Self {
        let limit = limit.filter(|limit| *limit > 0);
        let calls: HashMap<&str, &str> = history
            .iter()
            .flat_map(|m| &m.tool_calls)
            .map(|call| (call.id.as_str(), call.name.as_str()))
            .collect();
        let (mut skills, mut mcp, mut conversation) = (frame.skills, frame.mcp, 0);
        for message in history {
            let tokens = estimate_message_tokens(message);
            let tool = message
                .tool_call_id
                .as_deref()
                .filter(|_| message.role == LlmRole::Tool)
                .and_then(|id| calls.get(id))
                .and_then(|name| ToolName::from_wire_name(name));
            match tool {
                Some(ToolName::Skill) => skills += tokens,
                Some(ToolName::Mcp) => mcp += tokens,
                _ => conversation += tokens,
            }
        }
        Self {
            instructions: frame.instructions,
            skills,
            tools: frame.tools,
            mcp,
            conversation,
            total: frame.instructions + frame.tools + skills + mcp + conversation,
            limit,
            compacts_at: limit
                .map(|limit| (u64::from(limit) * TRIGGER_PERCENT / 100) as usize),
        }
    }
}

/// Whether a pass is worth making now.
///
/// No configured limit means no: the app talks to gateways it knows nothing
/// about, and compacting against a guessed window would throw away
/// conversation to solve a problem that may not exist.
///
/// Whether there is anything worth folding is [`plan_compaction`]'s to say.
pub fn should_compact(estimated_tokens: usize, context_limit: Option<u32>) -> bool {
    let Some(limit) = context_limit.filter(|limit| *limit > 0) else {
        return false;
    };
    estimated_tokens as u64 * 100 >= u64::from(limit) * TRIGGER_PERCENT
}

/// One pass, as positions in the history: `[..keep_head]` stays, the
/// `summarize` messages after it become one summary, and the rest is kept
/// word for word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactionPlan {
    /// Leading system messages. The instructions are not conversation and
    /// summarizing them would quietly rewrite how the agent behaves.
    pub keep_head: usize,
    pub summarize: usize,
}

impl CompactionPlan {
    /// Where the verbatim tail begins.
    pub fn tail_start(&self) -> usize {
        self.keep_head + self.summarize
    }
}

/// What to fold away, or `None` when there is nothing worth folding.
///
/// The cut is pulled back from the recency floor until it is a place a
/// history can legally be cut: never between an assistant message asking for
/// tool calls and the messages answering them. A provider refuses a history
/// that opens with an answer to a question it cannot see, so a cut in the
/// middle of a round does not shorten the conversation — it breaks it.
///
/// The tail is the latest messages that fit in `keep_tail_tokens`, and never
/// less than the last one.
pub fn plan_compaction(messages: &[LlmMessage], keep_tail_tokens: usize) -> Option<CompactionPlan> {
    let keep_head = messages
        .iter()
        .take_while(|message| message.role == LlmRole::System)
        .count();

    let mut floor = messages.len();
    let mut kept = 0;
    while floor > keep_head {
        let cost = estimate_message_tokens(&messages[floor - 1]);
        if floor < messages.len() && kept + cost > keep_tail_tokens {
            break;
        }
        kept += cost;
        floor -= 1;
    }
    let cut = safe_cut(messages, floor, keep_head)?;

    let summarize = cut - keep_head;
    (summarize > 0).then_some(CompactionPlan {
        keep_head,
        summarize,
    })
}

/// The first legal cut at or before `from`, or `None` if there is none above
/// `floor` — a single round of tool calls longer than the whole tail, which
/// is a history to leave alone rather than to break.
fn safe_cut(messages: &[LlmMessage], from: usize, floor: usize) -> Option<usize> {
    let mut cut = from;
    while cut > floor && messages.get(cut).is_some_and(|m| m.role == LlmRole::Tool) {
        cut -= 1;
    }
    (!messages.get(cut).is_some_and(|m| m.role == LlmRole::Tool)).then_some(cut)
}

/// The history as it will be sent from now on. The summary goes in as the
/// user's own recap rather than as a system instruction: it is a record of
/// what was said, and a model that reads it as an instruction starts
/// following the old conversation again instead of continuing it.
pub fn apply(messages: &[LlmMessage], plan: CompactionPlan, summary: &str) -> Vec<LlmMessage> {
    let mut compacted = messages[..plan.keep_head].to_vec();
    compacted.push(summary_message(summary));
    compacted.extend_from_slice(&messages[plan.tail_start()..]);
    compacted
}

pub fn summary_message(summary: &str) -> LlmMessage {
    LlmMessage::user(format!("{SUMMARY_PREFIX}\n\n{summary}"))
}

/// What a branch summary says it is: the part of the conversation the user
/// rewound away from (`docs/27-pi-ideas.md`, item 6). The files went back
/// with it, which the model is told too — the summary is what was tried,
/// not what is on disk.
pub const BRANCH_SUMMARY_PREFIX: &str =
    "[Summary of a conversation branch the user rewound away from; the files it changed were put back]";

pub fn branch_summary_message(summary: &str) -> LlmMessage {
    LlmMessage::user(format!("{BRANCH_SUMMARY_PREFIX}\n\n{summary}"))
}

/// The lists of files a summary ends with, in the order they are written.
const MODIFIED_FILES: &str = "modified-files";
const DELETED_FILES: &str = "deleted-files";
const READ_FILES: &str = "read-files";

/// The summary with the files the folded messages touched appended, as
/// `<modified-files>`, `<deleted-files>` and `<read-files>` blocks of one path
/// a line, each path as the model wrote it.
///
/// Built from the calls rather than asked of the summarizer: a model writing
/// prose drops a path now and then, and the next pass starts from this
/// summary, so a path dropped once would be gone for good. For the same reason
/// the lists are cumulative — an earlier summary among `folded` is where they
/// start.
///
/// Only calls that ran count: a failed or refused edit changed nothing. A file
/// both read and changed is listed once, as changed. `move` leaves its source
/// deleted and its destination modified. What a command or an MCP tool did to
/// files is not known, and not listed.
pub fn with_file_lists(summary: &str, folded: &[LlmMessage]) -> String {
    let ran: HashSet<&str> = folded
        .iter()
        .filter(|m| m.role == LlmRole::Tool && !m.content.as_deref().is_some_and(did_nothing))
        .filter_map(|m| m.tool_call_id.as_deref())
        .collect();

    let mut modified = BTreeSet::new();
    let mut deleted = BTreeSet::new();
    let mut read = BTreeSet::new();
    for message in folded {
        let earlier = (message.role == LlmRole::User)
            .then(|| message.content.as_deref()?.strip_prefix(SUMMARY_PREFIX))
            .flatten();
        if let Some(earlier) = earlier {
            modified.extend(paths_in(earlier, MODIFIED_FILES));
            deleted.extend(paths_in(earlier, DELETED_FILES));
            read.extend(paths_in(earlier, READ_FILES));
        }
        for call in message.tool_calls.iter().filter(|c| ran.contains(c.id.as_str())) {
            // The first JSON value, as the tool's own parsing takes it: a
            // complete object with noise after it still ran.
            let args = serde_json::Deserializer::from_str(&call.arguments)
                .into_iter::<serde_json::Value>()
                .next()
                .and_then(Result::ok)
                .unwrap_or_default();
            let field = |key: &str| args.get(key).and_then(serde_json::Value::as_str).map(str::to_string);
            let name = ToolName::from_wire_name(&call.name);
            if name == Some(ToolName::ReadFile) {
                read.extend(crate::domain::tools::read_paths(&args));
                continue;
            }
            let (Some(name), Some(path)) = (name, field("path")) else {
                continue;
            };
            match name {
                ToolName::WriteFile | ToolName::EditFile => {
                    deleted.remove(&path);
                    modified.insert(path);
                }
                ToolName::DeleteFile => {
                    modified.remove(&path);
                    deleted.insert(path);
                }
                ToolName::Move => {
                    if let Some(to) = field("newPath") {
                        modified.remove(&path);
                        deleted.insert(path);
                        deleted.remove(&to);
                        modified.insert(to);
                    }
                }
                _ => {}
            }
        }
    }
    read.retain(|path| !modified.contains(path) && !deleted.contains(path));

    // The summarizer was shown the earlier summary, lists and all, and may
    // copy them; only the lists built here go out.
    let mut out = without_file_lists(summary);
    for (tag, paths) in [(MODIFIED_FILES, &modified), (DELETED_FILES, &deleted), (READ_FILES, &read)] {
        if !paths.is_empty() {
            let lines: Vec<&str> = paths.iter().map(String::as_str).collect();
            out.push_str(&format!("\n\n<{tag}>\n{}\n</{tag}>", lines.join("\n")));
        }
    }
    out
}

fn did_nothing(result: &str) -> bool {
    [TOOL_ERROR_PREFIX, TOOL_DENIED_PREFIX, TOOL_NOT_RUN_PREFIX]
        .iter()
        .any(|prefix| result.starts_with(prefix))
}

/// The paths of the `<tag>` block in `text`.
fn paths_in(text: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let block = text
        .find(&open)
        .map(|start| &text[start + open.len()..])
        .and_then(|rest| rest.find(&close).map(|end| &rest[..end]))
        .unwrap_or("");
    block.lines().map(str::trim).filter(|line| !line.is_empty()).map(str::to_string).collect()
}

fn without_file_lists(summary: &str) -> String {
    let mut text = summary.to_string();
    for tag in [MODIFIED_FILES, DELETED_FILES, READ_FILES] {
        let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
        while let Some(start) = text.find(&open) {
            let Some(end) = text[start..].find(&close) else { break };
            text.replace_range(start..start + end + close.len(), "");
        }
    }
    text.trim_end().to_string()
}

/// How the provider says "this conversation no longer fits".
///
/// Every one of these is someone's prose, which is why this is a list of
/// phrases rather than a status code: the OpenAI-compatible protocol has no
/// code for it, and each gateway phrases it its own way. Matching too
/// eagerly is the expensive mistake — an unrelated failure answered with a
/// summarizing request costs money and loses history — so these are phrases
/// specific enough that nothing else produces them.
const CONTEXT_LENGTH_PHRASES: [&str; 6] = [
    "context length",
    "context_length",
    "context window",
    "too many tokens",
    "prompt is too long",
    "maximum context",
];

pub fn is_context_length_error(message: &str) -> bool {
    let message = message.to_lowercase();
    CONTEXT_LENGTH_PHRASES
        .iter()
        .any(|phrase| message.contains(phrase))
}

/// What the summarizer is asked to do. Written for the next model reading
/// its own summary, not for a person: what matters is what would otherwise
/// have to be asked again.
pub const SUMMARY_INSTRUCTIONS: &str = "\
You are compacting the earlier part of a conversation between a user and a \
coding agent so it can continue in less context. Write a summary in English \
covering: what the user is trying to achieve, decisions already made and \
why, files touched by path and what changed in them, what has been tried \
and failed, and anything the agent must not forget to do. Be specific — \
names, paths, error messages. Do not add advice, do not speculate, and do \
not describe the conversation ('the user asked…'); write the state of the \
work. Plain prose and short lists only.";

/// The summarizer's instructions for a rewound branch. Unlike
/// [`SUMMARY_INSTRUCTIONS`] the work it describes is undone — what is worth
/// keeping is why it did not work and what was learned on the way. No file
/// lists either, for the same reason: every file in them was put back.
pub const BRANCH_SUMMARY_INSTRUCTIONS: &str = "\
You are summarizing part of a conversation between a user and a coding agent \
that the user abandoned: they rewound the chat to before it to try again, and \
every file the agent changed in it was put back as it was. Write a summary in \
English of what the next attempt should know: what was being attempted, what \
was tried and how it turned out — errors, dead ends, why it did not work —, \
what was learned about the code (names, paths, behaviour), and any decisions \
or preferences the user stated. Do not describe changes to files as if they \
were still there. Be specific — names, paths, error messages. Do not add \
advice, do not speculate, and do not describe the conversation ('the user \
asked…'). Plain prose and short lists only.";

/// The longest any one message is rendered at for the summarizer.
///
/// A file read is thirty thousand characters. Sending those verbatim to be
/// summarized would send the very context that just overflowed — the request
/// that is meant to make room would be the largest one of the session — so
/// when the whole does not fit in `budget_tokens`, the longest pieces are cut
/// to one length, never below this many bytes. What a summary needs from a
/// tool result is that it happened and roughly what came back, and that
/// survives the cut; what fits is shown whole, so a summary can keep the
/// names, paths and numbers a file had.
const MIN_RENDERED_BYTES: usize = 1_500;

/// The messages to be folded away, as text for the summarizer, in about
/// `budget_tokens`.
pub fn render_for_summary(messages: &[LlmMessage], budget_tokens: usize) -> String {
    let cap = rendered_cap(messages, budget_tokens * CHARS_PER_TOKEN);
    let truncate = |text: &str| truncate(text, cap);
    let content_of = |message: &LlmMessage| truncate(message.content.as_deref().unwrap_or(""));
    let mut out = String::new();
    for message in messages {
        let line = match message.role {
            LlmRole::System => continue,
            // The picture's size, not its pixels: the summary is text, and the
            // model writing it may not see pictures at all.
            LlmRole::User => {
                let notes = message.images.iter().map(|image| format!("{} ", image.note()));
                format!("User: {}{}", notes.collect::<String>(), content_of(message))
            }
            LlmRole::Tool => format!("  [result] {}", content_of(message)),
            LlmRole::Assistant => {
                let mut parts = Vec::new();
                let text = content_of(message);
                if !text.is_empty() {
                    parts.push(format!("Assistant: {text}"));
                }
                for call in &message.tool_calls {
                    // The arguments, not only the name: the summary is asked
                    // for the files that were touched, and `writeFile` alone
                    // names none of them.
                    parts.push(format!("  [tool] {} {}", call.name, truncate(&call.arguments)));
                }
                parts.join("\n")
            }
        };
        if line.is_empty() {
            continue;
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// The length, in bytes, every piece of `messages` is cut to so that all of
/// them fit in `budget`: no cut when they fit, otherwise the one length that
/// leaves short pieces whole and shares what is left among the long ones.
fn rendered_cap(messages: &[LlmMessage], budget: usize) -> usize {
    let mut lengths: Vec<usize> = messages
        .iter()
        .filter(|m| m.role != LlmRole::System)
        .flat_map(|m| m.content.as_deref().map(str::len).into_iter().chain(m.tool_calls.iter().map(|c| c.arguments.len())))
        .collect();
    if lengths.iter().sum::<usize>() <= budget {
        return usize::MAX;
    }
    lengths.sort_unstable();
    let mut rest = budget;
    for (i, &length) in lengths.iter().enumerate() {
        let share = rest / (lengths.len() - i);
        if length > share {
            return share.max(MIN_RENDERED_BYTES);
        }
        rest -= length;
    }
    usize::MAX
}

fn truncate(text: &str, cap: usize) -> String {
    if text.len() <= cap {
        return text.to_string();
    }
    let mut end = cap;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… [{} characters omitted]", &text[..end], text[end..].chars().count())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::llm::LlmToolCall;

    fn user(text: &str) -> LlmMessage {
        LlmMessage::user(text)
    }

    fn definition(name: &str, description: &str) -> LlmToolDefinition {
        LlmToolDefinition {
            name: name.to_string(),
            description: description.to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        }
    }

    /// No tools advertised is no cost — not a floor, and not the JSON of an
    /// empty array.
    fn picture(width: u32, height: u32) -> ImagePart {
        use crate::domain::image::ImageMediaType;
        // Base64 the size of a real screenshot's, so counting it by its
        // characters would be unmistakable.
        ImagePart { media_type: ImageMediaType::Png, data: "A".repeat(400_000), width, height }
    }

    /// A picture costs what its size says, not what its base64 is long.
    #[test]
    fn a_picture_is_counted_by_its_size() {
        let words = estimate_tokens(&[LlmMessage::user("look")]);
        let with = estimate_tokens(&[LlmMessage::user_with_images("look", vec![picture(600, 400), picture(10, 10)])]);
        assert_eq!(with - words, 320 + 200);
    }

    /// The summary request names the picture and its size; its pixels would
    /// be noise to a model that may not see them, and its base64 a flood.
    #[test]
    fn a_picture_in_a_summary_is_its_size() {
        let rendered = render_for_summary(&[LlmMessage::user_with_images("why red?", vec![picture(30, 20)])], 10_000);
        assert_eq!(rendered, "User: [image 30×20] why red?\n");
    }

    #[test]
    fn no_schemas_cost_nothing() {
        assert_eq!(estimate_tool_schema_tokens(&[]), 0);
    }

    /// The description is the bulk of a schema and the part that grows: a
    /// count that only saw the names would stay flat while the real cost
    /// doubled.
    #[test]
    fn a_schema_costs_what_its_description_costs() {
        let short = [definition("readFile", "Reads a file.")];
        let long = [definition("readFile", &"Reads a file. ".repeat(100))];

        let grown = estimate_tool_schema_tokens(&long) - estimate_tool_schema_tokens(&short);
        assert!(
            grown > 300,
            "a description 1 400 characters longer added {grown} tokens"
        );
    }

    #[test]
    fn every_advertised_tool_is_paid_for() {
        let one = [definition("readFile", "Reads a file.")];
        let two = [
            definition("readFile", "Reads a file."),
            definition("grep", "Searches for a pattern."),
        ];

        assert!(estimate_tool_schema_tokens(&two) > estimate_tool_schema_tokens(&one));
    }

    fn assistant(text: &str) -> LlmMessage {
        LlmMessage::assistant(text)
    }

    fn call_to(id: &str, tool: &str) -> LlmMessage {
        LlmMessage { tool_calls: vec![LlmToolCall { id: id.into(), name: tool.into(), arguments: "{}".into() }], ..LlmMessage::assistant("") }
    }

    /// A loaded skill and an MCP tool's result are charged to their own rows,
    /// beside the list and the schemas that brought them; everything else,
    /// the calls included, is conversation. A mismatched id is conversation
    /// too — never lost.
    #[test]
    fn a_loaded_skill_and_an_mcp_result_are_counted_on_their_own_rows() {
        let frame = RequestFrame { instructions: 100, skills: 10, tools: 50, mcp: 20 };
        let skill = LlmMessage::tool_result("s", "x".repeat(400));
        let found = LlmMessage::tool_result("m", "y".repeat(800));
        let read = LlmMessage::tool_result("r", "z".repeat(1200));
        let stray = LlmMessage::tool_result("gone", "w".repeat(40));
        let calls = [call_to("s", "skill"), call_to("m", "mcp__tracker__find"), call_to("r", "readFile")];
        let mut history: Vec<LlmMessage> = vec![user("go")];
        history.extend(calls.iter().cloned());
        history.extend([skill.clone(), found.clone(), read.clone(), stray.clone()]);

        let usage = ContextUsage::new(frame, &history, None);

        assert_eq!(usage.skills, 10 + estimate_tokens(&[skill]));
        assert_eq!(usage.mcp, 20 + estimate_tokens(&[found]));
        assert_eq!(usage.conversation, estimate_tokens(&[user("go")]) + estimate_tokens(&calls) + estimate_tokens(&[read, stray]));
        assert_eq!((usage.instructions, usage.tools), (100, 50));
        assert_eq!(usage.total, frame.total() + estimate_tokens(&history));
    }

    fn calling(id: &str) -> LlmMessage {
        LlmMessage {
            role: LlmRole::Assistant,
            content: None,
            tool_call_id: None,
            tool_calls: vec![LlmToolCall {
                id: id.to_string(),
                name: "readFile".to_string(),
                arguments: r#"{"path":"a.rs"}"#.to_string(),
            }],
            native_content: None,
            images: Vec::new(),
        }
    }

    fn answered(id: &str) -> LlmMessage {
        LlmMessage {
            role: LlmRole::Tool,
            content: Some("fn main() {}".to_string()),
            tool_call_id: Some(id.to_string()),
            tool_calls: Vec::new(),
            native_content: None,
            images: Vec::new(),
        }
    }

    fn conversation(len: usize) -> Vec<LlmMessage> {
        (0..len)
            .map(|i| {
                if i % 2 == 0 {
                    user(&format!("question {i}"))
                } else {
                    assistant(&format!("answer {i}"))
                }
            })
            .collect()
    }

    #[test]
    fn an_empty_conversation_costs_nothing() {
        assert_eq!(estimate_tokens(&[]), 0);
    }

    #[test]
    fn what_a_tool_call_carries_is_counted_too() {
        let prose = estimate_tokens(&[assistant("")]);
        let call = estimate_tokens(&[calling("c1")]);

        // The arguments and the name are in the request whether or not the
        // message has any prose in it.
        assert!(call > prose, "{call} is not more than {prose}");
    }

    #[test]
    fn a_tool_result_costs_what_it_says() {
        let short = estimate_tokens(&[answered("c1")]);
        let long = estimate_tokens(&[LlmMessage {
            content: Some("x".repeat(4_000)),
            ..answered("c1")
        }]);

        // Four thousand characters, four characters to a token.
        assert!((950..=1_050).contains(&(long - short)), "{long} vs {short}");
    }

    #[test]
    fn nothing_is_compacted_without_a_known_window() {
        assert!(!should_compact(1_000_000, None));
        assert!(!should_compact(1_000_000, Some(0)));
    }

    #[test]
    fn compaction_starts_before_the_window_is_full() {
        let limit = 10_000;

        assert!(!should_compact(8_999, Some(limit)));
        assert!(should_compact(9_000, Some(limit)));
    }

    /// How many messages these tests keep, and the budget that keeps exactly
    /// that many of `messages`.
    const KEEP: usize = 12;
    fn tail_of(messages: &[LlmMessage], n: usize) -> usize {
        estimate_tokens(&messages[messages.len() - n..])
    }

    /// The tail is what fits in its tokens — however many messages that is.
    #[test]
    fn the_last_messages_stay_word_for_word() {
        let messages = conversation(30);
        let plan = plan_compaction(&messages, tail_of(&messages, KEEP)).expect("something to fold");

        assert_eq!(plan.keep_head, 0);
        assert_eq!(plan.summarize, 30 - KEEP);
        assert_eq!(messages.len() - plan.tail_start(), KEEP);
    }

    /// One big result counts for what it costs: a tail of the same tokens
    /// keeps fewer messages when one of them is large.
    #[test]
    fn the_tail_is_measured_in_tokens_not_messages() {
        let mut messages = conversation(30);
        let budget = tail_of(&messages, KEEP);
        messages[25] = user(&"x".repeat(budget * CHARS_PER_TOKEN / 2));
        let plan = plan_compaction(&messages, budget).expect("something to fold");
        assert!(messages.len() - plan.tail_start() < KEEP, "kept {}", messages.len() - plan.tail_start());
    }

    /// The last message stays whatever it costs: a tail with nothing in it
    /// leaves the model nothing to continue from.
    #[test]
    fn the_last_message_stays_however_big() {
        let mut messages = conversation(10);
        messages.push(user(&"x".repeat(100_000)));
        let plan = plan_compaction(&messages, 10).expect("something to fold");
        assert_eq!(plan.tail_start(), messages.len() - 1);
    }

    /// The instructions are not conversation: folding them into a summary
    /// rewrites how the agent behaves, quietly and permanently.
    #[test]
    fn the_system_prompt_is_never_summarized() {
        let mut messages = vec![LlmMessage::system("you are an agent")];
        messages.extend(conversation(30));

        let plan = plan_compaction(&messages, tail_of(&messages, KEEP)).expect("something to fold");
        assert_eq!(plan.keep_head, 1);

        let compacted = apply(&messages, plan, "they talked about the parser");
        assert_eq!(compacted[0], LlmMessage::system("you are an agent"));
        assert!(compacted[1].content.as_deref().unwrap().starts_with(SUMMARY_PREFIX));
    }

    #[test]
    fn a_conversation_with_nothing_to_fold_is_left_alone() {
        let short = conversation(KEEP);
        assert_eq!(plan_compaction(&short, tail_of(&short, KEEP)), None);
        assert_eq!(plan_compaction(&[], 1_000), None);
    }

    /// The rule the wire format adds: an answer whose question has been
    /// summarized away is a message the provider refuses outright.
    #[test]
    fn a_cut_never_separates_a_tool_call_from_its_answer() {
        // The floor lands inside the round: 20 messages, keep 3, so the cut
        // would fall on the second result.
        let mut messages = conversation(16);
        messages.push(calling("c1"));
        messages.push(answered("c1"));
        messages.push(answered("c1b"));
        messages.push(user("and now?"));

        let plan = plan_compaction(&messages, tail_of(&messages, 3)).expect("something to fold");
        let tail = &messages[plan.tail_start()..];

        assert_eq!(tail[0], calling("c1"), "the tail opens with an orphaned result");
        assert!(!tail.is_empty());
    }

    /// Where every message of the tail is part of one unfinished round,
    /// there is no legal cut. Leaving the history long is the lesser harm:
    /// the illegal one is refused by the provider outright.
    #[test]
    fn a_round_too_long_to_cut_is_left_whole() {
        let mut messages = vec![calling("c1")];
        messages.extend((0..30).map(|_| answered("c1")));

        assert_eq!(plan_compaction(&messages, tail_of(&messages, 2)), None);
    }

    #[test]
    fn the_compacted_history_is_the_head_the_summary_and_the_tail() {
        let messages = conversation(30);
        let plan = plan_compaction(&messages, tail_of(&messages, KEEP)).unwrap();

        let compacted = apply(&messages, plan, "they talked about the parser");

        assert_eq!(compacted.len(), 1 + KEEP);
        assert_eq!(compacted[0], summary_message("they talked about the parser"));
        assert_eq!(&compacted[1..], &messages[plan.tail_start()..]);
    }

    /// Two passes in a row: the second one folds the first one's summary in
    /// with everything since, which is what replaces the cache upstream kept.
    #[test]
    fn a_second_pass_starts_from_the_first_ones_summary() {
        let messages = conversation(40);
        let first = apply(
            &messages,
            plan_compaction(&messages, tail_of(&messages, KEEP)).unwrap(),
            "the parser",
        );
        let mut grown = first.clone();
        grown.extend(conversation(20));

        let plan = plan_compaction(&grown, tail_of(&grown, KEEP)).expect("something to fold");
        assert_eq!(plan.keep_head, 0, "the summary is a message like any other");
        assert!(plan.summarize >= 1);

        let second = apply(&grown, plan, "the parser and the lexer");
        assert_eq!(second.iter().filter(|m| is_summary(m)).count(), 1);
    }

    #[test]
    fn a_provider_saying_the_conversation_is_too_long_is_recognised() {
        for message in [
            "http status 400: This model's maximum context length is 8192 tokens",
            "provider error: context_length_exceeded",
            "Error: prompt is too long: 210000 tokens > 200000 maximum",
            "input exceeds the context window of this model",
            "too many tokens in the request",
        ] {
            assert!(is_context_length_error(message), "missed {message:?}");
        }
    }

    /// The expensive mistake is the other one: answering an unrelated failure
    /// with a summarizing request costs a call and folds away history for
    /// nothing.
    #[test]
    fn and_other_failures_are_not_mistaken_for_it() {
        for message in [
            "http status 401: invalid api key",
            "rate limited by the provider",
            "connection closed before message completed",
            "model not found: qwen3",
            "context deadline exceeded",
        ] {
            assert!(!is_context_length_error(message), "matched {message:?}");
        }
    }

    #[test]
    fn what_the_summarizer_reads_names_the_files_that_were_touched() {
        let messages = vec![
            user("fix the parser"),
            calling("c1"),
            answered("c1"),
            assistant("done"),
        ];

        let rendered = render_for_summary(&messages, 1_000);
        assert!(rendered.contains("User: fix the parser"), "{rendered}");
        assert!(rendered.contains("[tool] readFile"), "{rendered}");
        assert!(rendered.contains("a.rs"), "{rendered}");
        assert!(rendered.contains("Assistant: done"), "{rendered}");
    }

    /// The request that is supposed to make room must not be the largest one
    /// of the session: a file read is thirty thousand characters, and there
    /// are dozens of them in what is being folded away.
    #[test]
    fn a_huge_tool_result_is_cut_down_before_it_is_summarized() {
        let huge = LlmMessage {
            content: Some("x".repeat(30_000)),
            ..answered("c1")
        };

        let rendered = render_for_summary(&[huge], 0);
        assert!(rendered.len() < 3_000, "{} characters", rendered.len());
        assert!(rendered.contains("characters omitted"), "{rendered}");
    }

    /// What fits is shown whole: a summary of a file it saw only the first
    /// lines of cannot keep the names and numbers further down.
    #[test]
    fn what_fits_the_budget_is_summarized_from_the_whole_text() {
        let file = LlmMessage { content: Some("x".repeat(30_000)), ..answered("c1") };
        let rendered = render_for_summary(&[file], 10_000);
        assert!(!rendered.contains("omitted"), "cut although it fits");
        assert!(rendered.len() > 30_000);
    }

    /// Over the budget, the long pieces share what is left and the short ones
    /// stay whole.
    #[test]
    fn over_the_budget_the_long_pieces_are_cut_and_the_short_ones_kept() {
        let long = |id: &str| LlmMessage { content: Some("x".repeat(40_000)), ..answered(id) };
        let messages = [user("fix the parser"), long("a"), long("b")];
        let rendered = render_for_summary(&messages, 5_000);
        assert!(rendered.contains("User: fix the parser"), "{rendered}");
        assert_eq!(rendered.matches("characters omitted").count(), 2);
        assert!((18_000..=22_000).contains(&rendered.len()), "{} bytes for a 20 000-byte budget", rendered.len());
    }

    /// Cutting by characters on a multi-byte string is how this kind of code
    /// panics in production and nowhere else.
    #[test]
    fn cutting_a_long_message_does_not_split_a_character() {
        let cyrillic = LlmMessage {
            content: Some("я".repeat(30_000)),
            ..answered("c1")
        };

        let rendered = render_for_summary(&cyrillic_message(cyrillic), 0);
        assert!(rendered.contains("characters omitted"), "{rendered}");
    }

    fn cyrillic_message(message: LlmMessage) -> Vec<LlmMessage> {
        vec![message]
    }

    #[test]
    fn the_system_prompt_is_not_part_of_what_is_summarized() {
        let rendered = render_for_summary(&[LlmMessage::system("you are an agent"), user("hi")], 1_000);
        assert!(!rendered.contains("you are an agent"), "{rendered}");
        assert!(rendered.contains("User: hi"), "{rendered}");
    }

    fn is_summary(message: &LlmMessage) -> bool {
        message
            .content
            .as_deref()
            .is_some_and(|text| text.starts_with(SUMMARY_PREFIX))
    }

    /// One call and its result: the round as the history holds it.
    fn ran(id: &str, tool: &str, arguments: &str, result: &str) -> [LlmMessage; 2] {
        let call = LlmToolCall { id: id.into(), name: tool.into(), arguments: arguments.into() };
        [LlmMessage { tool_calls: vec![call], ..LlmMessage::assistant("") }, LlmMessage::tool_result(id, result)]
    }

    fn folded(rounds: &[[LlmMessage; 2]]) -> Vec<LlmMessage> {
        rounds.iter().flatten().cloned().collect()
    }

    #[test]
    fn the_summary_lists_the_files_its_calls_changed_deleted_and_read() {
        let history = folded(&[
            ran("1", "readFile", r#"{"path":"src/b.rs"}"#, "fn b() {}"),
            ran("2", "readFile", r#"{"path":"src/a.rs"}"#, "fn a() {}"),
            ran("3", "editFile", r#"{"path":"src/a.rs","edits":[]}"#, "{\"path\":\"src/a.rs\"}"),
            ran("4", "writeFile", r#"{"path":"src/new.rs","content":""}"#, "ok"),
            ran("5", "deleteFile", r#"{"path":"old.txt"}"#, "ok"),
            ran("6", "grep", r#"{"pattern":"x","path":"src"}"#, "nothing"),
            ran("7", "readFile", r#"{"paths":["src/c.rs","src/a.rs"]}"#, "==> src/c.rs"),
        ]);

        assert_eq!(
            with_file_lists("The work so far.", &history),
            "The work so far.\n\n\
             <modified-files>\nsrc/a.rs\nsrc/new.rs\n</modified-files>\n\n\
             <deleted-files>\nold.txt\n</deleted-files>\n\n\
             <read-files>\nsrc/b.rs\nsrc/c.rs\n</read-files>"
        );
    }

    /// A refused, failed or never-run write changed nothing, and a failed read
    /// read nothing.
    #[test]
    fn a_call_that_did_nothing_is_not_listed() {
        let history = folded(&[
            ran("1", "editFile", r#"{"path":"a.rs"}"#, "Error: the anchor was not found"),
            ran("2", "writeFile", r#"{"path":"b.rs"}"#, "Denied by the user: not that one"),
            ran("3", "deleteFile", r#"{"path":"c.rs"}"#, "Not run: the user stopped the turn before this call."),
            ran("4", "readFile", r#"{"path":"d.rs"}"#, "Error: no such file"),
        ]);

        assert_eq!(with_file_lists("Summary.", &history), "Summary.");
    }

    #[test]
    fn a_move_deletes_its_source_and_modifies_its_destination() {
        let history = folded(&[
            ran("1", "writeFile", r#"{"path":"draft.md"}"#, "ok"),
            ran("2", "move", r#"{"path":"draft.md","newPath":"docs/final.md"}"#, "ok"),
        ]);

        assert_eq!(
            with_file_lists("S.", &history),
            "S.\n\n<modified-files>\ndocs/final.md\n</modified-files>\n\n<deleted-files>\ndraft.md\n</deleted-files>"
        );
    }

    /// Each path is in one list, the one its last call put it in.
    #[test]
    fn a_file_is_listed_as_what_happened_to_it_last() {
        let history = folded(&[
            ran("1", "readFile", r#"{"path":"a.rs"}"#, "x"),
            ran("2", "writeFile", r#"{"path":"b.rs"}"#, "ok"),
            ran("3", "deleteFile", r#"{"path":"a.rs"}"#, "ok"),
            ran("4", "deleteFile", r#"{"path":"b.rs"}"#, "ok"),
            ran("5", "deleteFile", r#"{"path":"c.rs"}"#, "ok"),
            ran("6", "move", r#"{"path":"d.rs","newPath":"c.rs"}"#, "ok"),
        ]);

        assert_eq!(
            with_file_lists("S.", &history),
            "S.\n\n<modified-files>\nc.rs\n</modified-files>\n\n<deleted-files>\na.rs\nb.rs\nd.rs\n</deleted-files>"
        );
    }

    /// The point of building the lists: the second pass keeps a file from the
    /// first even though its summarizer never mentioned it — and a file
    /// deleted then and written now is modified, not both.
    #[test]
    fn the_lists_of_an_earlier_summary_carry_into_the_next() {
        let first = with_file_lists("First.", &folded(&[
            ran("1", "editFile", r#"{"path":"kept.rs"}"#, "ok"),
            ran("2", "deleteFile", r#"{"path":"back.rs"}"#, "ok"),
            ran("3", "readFile", r#"{"path":"seen.rs"}"#, "x"),
            ran("5", "deleteFile", r#"{"path":"gone.rs"}"#, "ok"),
        ]));
        let mut second = vec![summary_message(&first)];
        second.extend(folded(&[ran("4", "writeFile", r#"{"path":"back.rs"}"#, "ok")]));

        assert_eq!(
            with_file_lists("Second, about something else.", &second),
            "Second, about something else.\n\n\
             <modified-files>\nback.rs\nkept.rs\n</modified-files>\n\n\
             <deleted-files>\ngone.rs\n</deleted-files>\n\n\
             <read-files>\nseen.rs\n</read-files>"
        );
    }

    /// The summarizer was shown the earlier lists and may write them out
    /// again; the summary still ends with one set, built from the calls.
    #[test]
    fn lists_the_summarizer_copied_are_replaced_not_doubled() {
        let history = folded(&[ran("1", "editFile", r#"{"path":"a.rs"}"#, "ok")]);
        let copied = "Done.\n\n<modified-files>\nmade-up.rs\n</modified-files>\n\n<read-files>\nb.rs\n</read-files>";

        assert_eq!(with_file_lists(copied, &history), "Done.\n\n<modified-files>\na.rs\n</modified-files>");
    }

    /// The tool takes a complete object with noise after it; so do the lists.
    #[test]
    fn arguments_with_trailing_noise_still_name_their_file() {
        let history = folded(&[ran("1", "writeFile", "{\"path\":\"a.rs\",\"content\":\"\"}}", "ok")]);

        assert_eq!(with_file_lists("S.", &history), "S.\n\n<modified-files>\na.rs\n</modified-files>");
    }
}
