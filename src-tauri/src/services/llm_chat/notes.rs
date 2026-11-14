//! Notes the history is owed before a request: the checklist, the plan, what changed.

use super::*;

/// Puts the checklist back into the history when the history no longer shows
/// it as it stands — a summary folded the last `todo` result away, or a
/// branched chat cut it off while the list carried on — and otherwise leaves
/// the history alone.
///
/// Into the history, once, rather than onto the end of every request: the
/// history is only ever appended to, so each request is the previous one plus
/// what happened since, which is what every provider's prompt cache matches.
/// A checklist re-sent at the end of each request was a different tail every
/// time, and OpenAI's cache — which reuses whole earlier requests — kept
/// nothing past the system prompt: 6k of a 40k history (`agent_bench`, GPT via
/// OpenRouter). Codex does the same with its plan tool, and never re-sends it.
pub(super) fn restore_checklist(history: &mut Vec<LlmMessage>, todos: &[Task]) {
    if todos.is_empty() || history_shows(history, todos) {
        return;
    }
    append_note(history, format!("{CHECKLIST_NOTE}\n{}", for_model(&ToolResult::Todo { tasks: todos.to_vec() })));
}

pub(super) const CHECKLIST_NOTE: &str = "[The checklist as it stands]";
pub(super) const PLAN_NOTE: &str = "[The plan as it stands]";

/// Puts the plan into the history when the history no longer shows it as it
/// stands — the user edited it in the Plan tab, a summary folded the last
/// `writePlan` away, or the chat was handed over with a plan and no call —
/// and otherwise leaves the history alone.
///
/// The history rather than the prompt, for the reason the checklist is
/// there ([`restore_checklist`]): a plan in the system prefix changed it on
/// every `writePlan` and every edit, and the whole conversation after it was
/// a cache miss again — at DeepSeek's prices, fifty times what a hit costs.
pub(super) fn restore_plan(history: &mut Vec<LlmMessage>, plan: Option<&str>) {
    let Some(plan) = plan.map(str::trim).filter(|p| !p.is_empty()) else { return };
    if latest_plan_is(history, plan) {
        return;
    }
    append_note(
        history,
        format!(
            "{PLAN_NOTE} The user may have edited it since it was written; where it differs from an earlier \
             version in the conversation, this one is right.\n<plan>\n{plan}\n</plan>"
        ),
    );
}

/// Whether the latest plan in the history — a `writePlan` call's content, or
/// one put back by [`restore_plan`] — is `plan`. The latest, not any: the
/// user's edit overrules the call it was made to.
pub(super) fn latest_plan_is(history: &[LlmMessage], plan: &str) -> bool {
    for message in history.iter().rev() {
        let written = message.tool_calls.iter().rev().filter(|c| c.name == ToolName::WritePlan.wire_name()).find_map(|call| {
            serde_json::from_str::<crate::domain::tools::WritePlanArgs>(&call.arguments).ok()
        });
        if let Some(written) = written {
            return written.content.trim() == plan;
        }
        if let Some(at) = message.content.as_deref().and_then(|c| c.rfind(PLAN_NOTE).map(|at| &c[at..])) {
            return at.contains(&format!("<plan>\n{plan}\n</plan>"));
        }
    }
    false
}

/// How a note on a changed part of the prompt begins; the part's name follows.
pub(super) const STANDING_NOTE: &str = "[Changed during this conversation:";

/// The note saying what `name` is now. It ends in a line of its own, since a
/// section separates its files and paragraphs with blank lines — and the next
/// note follows after one — so without it a section cut at a blank line would
/// read as the start of the note that said it whole.
pub(super) fn standing_note(name: &str, now: Option<&str>) -> String {
    format!(
        "{STANDING_NOTE} {name}. Where it differs from that section of the system prompt, this one is right.]\n{}\n[End of {name}.]",
        now.unwrap_or("Gone: the section no longer applies."),
    )
}

/// Puts into the history each part of the prompt that the files say
/// otherwise than the history last did — the prompt's own frozen version
/// ([`Frozen`]) until a note has followed it — and otherwise leaves it alone.
/// The latest note, not any: a file put back as it was is a change too.
pub(super) fn restore_standing(history: &mut Vec<LlmMessage>, turn: &Turn) {
    let (Some(frozen), Some(_mode)) = (turn.frozen, turn.place.mode()) else { return };
    let parallel = match turn.place {
        Place::Folder { parallel, .. } => prompt::parallel_block(&parallel()),
        Place::Chat { .. } => None,
    };
    let parts = [
        ("Project instructions", prompt::rules_block(&frozen.rules), prompt::rules_block(turn.rules)),
        ("Memory", prompt::memory_block(&frozen.rules), prompt::memory_block(turn.rules)),
        ("Parallel sessions", frozen.parallel.clone(), parallel),
    ];
    for (name, was, now) in parts {
        let note = standing_note(name, now.as_deref());
        let head = format!("{STANDING_NOTE} {name}.");
        let shown = history.iter().rev().filter_map(|m| m.content.as_deref()).find_map(|c| c.rfind(&head).map(|at| &c[at..]));
        let current = match shown {
            Some(shown) => shown.strip_prefix(note.as_str()).is_some_and(|rest| rest.is_empty() || rest.starts_with("\n\n")),
            None => was == now,
        };
        if !current {
            append_note(history, note);
        }
    }
}

/// Onto the last tool result when that is where the history ends — it has
/// not been sent yet — or onto the plan's note just added, which the
/// checklist's follows; otherwise a message of its own. A user's message is
/// left as they wrote it, since a branched chat finds it by its text.
pub(super) fn append_note(history: &mut Vec<LlmMessage>, note: String) {
    let is_note = |m: &LlmMessage| m.role == LlmRole::User && m.content.as_deref().is_some_and(|c| c.starts_with(PLAN_NOTE));
    match history.last_mut() {
        Some(last) if last.role == LlmRole::Tool || is_note(last) => {
            let content = last.content.get_or_insert_with(String::new);
            content.push_str("\n\n");
            content.push_str(&note);
        }
        _ => history.push(LlmMessage::user(note)),
    }
}

/// Whether the latest checklist in the history — a `todo` result, or one put
/// back by [`restore_checklist`] — is the list exactly as it stands.
///
/// The latest, not any: a stale list is worse than none. The list only
/// changes through `todo`, whose every result shows it whole.
pub(super) fn history_shows(history: &[LlmMessage], todos: &[Task]) -> bool {
    let current = for_model(&ToolResult::Todo { tasks: todos.to_vec() });
    history
        .iter()
        .rev()
        .filter_map(|m| m.content.as_deref())
        .find(|content| content.contains(CHECKLIST_LEGEND))
        .is_some_and(|shown| shows_exactly(shown, &current))
}

/// `list` appears in `content` whole: followed by nothing, or by a blank line
/// before whatever was added after it — never by another row, or a shorter
/// list would pass for the longer one it begins.
pub(super) fn shows_exactly(content: &str, list: &str) -> bool {
    content.match_indices(list).any(|(at, _)| {
        let rest = &content[at + list.len()..];
        rest.is_empty() || rest.starts_with("\n\n")
    })
}
