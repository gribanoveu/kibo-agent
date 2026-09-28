//! A saved conversation as a Markdown transcript, for reading and analysis
//! outside the app.
//!
//! What the *model* saw, not what the panel drew: `messages` is the record of
//! the conversation as it actually happened — every tool call, every result,
//! the system prompt included — and that is what a question like "why did it
//! do that" is answered from. `blocks` are the same turn arranged for the
//! reader, and are opaque to Rust anyway (see `chat_record`).
//!
//! Times are written in UTC rather than the local zone: an exported file
//! travels, and a timestamp that means something different on the machine
//! that reads it is worse than one that always means the same thing.

use super::chat_record::ChatRecord;
use super::llm::{LlmMessage, LlmRole};

/// The whole record as one Markdown document.
pub fn to_markdown(record: &ChatRecord) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {}\n\n", record.title));
    out.push_str(&format!("- Chat: `{}`\n", record.id));
    out.push_str(&format!("- Folder: `{}`\n", record.workspace));
    out.push_str(&format!("- Started: {}\n", time(record.created_at)));
    out.push_str(&format!("- Updated: {}\n", time(record.updated_at)));
    if let Some(from) = &record.branched_from {
        out.push_str(&format!("- Branched from: `{from}`\n"));
    }

    for message in &record.messages {
        out.push('\n');
        out.push_str(&section(message));
    }
    out
}

/// One message, heading and all.
fn section(message: &LlmMessage) -> String {
    let mut out = format!("## {}\n", role(message.role));
    if let Some(id) = &message.tool_call_id {
        out.push_str(&format!("\nIn answer to `{id}`.\n"));
    }
    match message.content.as_deref().map(str::trim) {
        // A tool result is data, not prose: fenced, so a diff or a stack
        // trace reads as what it is and cannot be mistaken for Markdown.
        Some(text) if !text.is_empty() && message.role == LlmRole::Tool => {
            out.push_str(&format!("\n{}\n", fenced(text, "")));
        }
        Some(text) if !text.is_empty() => out.push_str(&format!("\n{text}\n")),
        _ => {}
    }
    for call in &message.tool_calls {
        out.push_str(&format!("\n**Tool call** `{}` · `{}`\n", call.name, call.id));
        // Arguments as the model produced them — not reformatted, because
        // malformed JSON is itself something worth seeing.
        out.push_str(&format!("\n{}\n", fenced(call.arguments.trim(), "json")));
    }
    out
}

fn role(role: LlmRole) -> &'static str {
    match role {
        LlmRole::System => "System",
        LlmRole::User => "User",
        LlmRole::Assistant => "Assistant",
        LlmRole::Tool => "Tool result",
    }
}

/// A code fence long enough to hold `text`, whatever backticks are in it.
///
/// Tool output routinely contains fenced Markdown — a file the agent read,
/// an answer it quoted — and a three-backtick fence around it ends at the
/// first one inside, silently turning the rest of the transcript into prose.
fn fenced(text: &str, lang: &str) -> String {
    let longest = text
        .split(|c| c != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    format!("{fence}{lang}\n{text}\n{fence}")
}

/// `1970-01-01 00:00 UTC`, or nothing at all for a timestamp that is not one.
fn time(millis: i64) -> String {
    match chrono::DateTime::from_timestamp_millis(millis) {
        Some(time) => time.format("%Y-%m-%d %H:%M UTC").to_string(),
        None => "unknown".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::chat_record::CHAT_SCHEMA_VERSION;
    use crate::domain::llm::LlmToolCall;

    fn record(messages: Vec<LlmMessage>) -> ChatRecord {
        ChatRecord {
            schema_version: CHAT_SCHEMA_VERSION,
            id: "abc".to_string(),
            workspace: "/repo".to_string(),
            title: "Fix the parser".to_string(),
            created_at: 0,
            updated_at: 86_400_000,
            messages,
            blocks: serde_json::json!([]),
            todos: Vec::new(),
            plan: None,
            branched_from: None,
            role: None,
        }
    }

    #[test]
    fn the_header_says_which_chat_and_which_folder() {
        let text = to_markdown(&record(vec![]));
        assert!(text.starts_with("# Fix the parser\n"), "{text}");
        assert!(text.contains("- Chat: `abc`"), "{text}");
        assert!(text.contains("- Folder: `/repo`"), "{text}");
        assert!(text.contains("- Started: 1970-01-01 00:00 UTC"), "{text}");
        assert!(text.contains("- Updated: 1970-01-02 00:00 UTC"), "{text}");
    }

    #[test]
    fn a_branch_says_what_it_came_from() {
        let mut branch = record(vec![]);
        assert!(!to_markdown(&branch).contains("Branched from"));

        branch.branched_from = Some("older".to_string());
        assert!(to_markdown(&branch).contains("- Branched from: `older`"));
    }

    /// Every speaker is named, the system prompt included: an answer is read
    /// against the instructions that produced it.
    #[test]
    fn each_message_is_under_a_heading_naming_who_said_it() {
        let text = to_markdown(&record(vec![
            LlmMessage::system("you are an agent"),
            LlmMessage::user("why?"),
            LlmMessage::assistant("because"),
            LlmMessage::tool_result("call-1", "ok"),
        ]));

        let headings: Vec<&str> = text.lines().filter(|line| line.starts_with("## ")).collect();
        assert_eq!(
            headings,
            ["## System", "## User", "## Assistant", "## Tool result"]
        );
        assert!(text.contains("you are an agent"), "{text}");
        assert!(text.contains("because"), "{text}");
    }

    /// The call and its result are two messages apart; the ids are what ties
    /// them back together while reading.
    #[test]
    fn a_call_is_written_with_its_arguments_and_its_result_points_back_at_it() {
        let text = to_markdown(&record(vec![
            LlmMessage::tool_requests(vec![LlmToolCall {
                id: "call-1".to_string(),
                name: "Read".to_string(),
                arguments: r#"{"path":"src/lib.rs"}"#.to_string(),
            }]),
            LlmMessage::tool_result("call-1", "fn main() {}"),
        ]));

        assert!(text.contains("**Tool call** `Read` · `call-1`"), "{text}");
        assert!(text.contains("```json\n{\"path\":\"src/lib.rs\"}\n```"), "{text}");
        assert!(text.contains("In answer to `call-1`."), "{text}");
        assert!(text.contains("```\nfn main() {}\n```"), "{text}");
    }

    /// Tool output is full of Markdown — a file the agent read, an answer it
    /// quoted. A fence that ends inside the output turns the rest of the
    /// transcript into prose.
    #[test]
    fn output_with_its_own_fences_in_it_stays_inside_one() {
        let output = "here:\n```rust\nfn main() {}\n```\nand that is all";
        let text = to_markdown(&record(vec![LlmMessage::tool_result("call-1", output)]));

        assert!(text.contains(&format!("````\n{output}\n````")), "{text}");
    }

    /// The fence is measured against the backticks in the text, not against
    /// the text: output that is *only* backticks — an empty fenced block a
    /// tool echoed back — still has to be held.
    #[test]
    fn output_that_is_nothing_but_backticks_is_still_held() {
        let text = to_markdown(&record(vec![LlmMessage::tool_result("call-1", "```")]));

        assert!(text.contains("````\n```\n````"), "{text}");
    }

    /// An assistant turn that only asked for tools has no text, and must not
    /// be written as if it had said something empty.
    #[test]
    fn a_message_with_nothing_said_is_just_its_heading() {
        let text = to_markdown(&record(vec![
            LlmMessage::assistant("   "),
            LlmMessage::user("next"),
        ]));

        assert!(text.contains("## Assistant\n\n## User"), "{text}");
    }

    #[test]
    fn a_timestamp_that_is_not_one_is_not_a_date() {
        let mut broken = record(vec![]);
        broken.created_at = i64::MAX;
        assert!(to_markdown(&broken).contains("- Started: unknown"), );
    }
}
