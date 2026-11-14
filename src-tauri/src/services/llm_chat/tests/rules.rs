use super::*;

#[test]
fn parallel_git_guard_is_asked_on_every_command() {
    let mut h = harness("parallel-git-preflight", vec![]);
    let checkout = call("runCommand", r#"{"command":"git checkout main"}"#);
    h.run(|turn| preflight(turn, &ReadFiles::default(), &checkout)).expect("one turn may switch its tree");

    h.parallel = vec!["other".into()];
    let refused = h.run(|turn| preflight(turn, &ReadFiles::default(), &checkout)).expect_err("a second turn protects the shared tree");
    assert!(matches!(&refused, crate::domain::tools::ToolError::GitTreeDuringParallel(subcommand) if subcommand == "checkout"), "{refused}");
    for command in ["git status", "git add src/lib.rs"] {
        let call = call("runCommand", &format!(r#"{{"command":"{command}"}}"#));
        h.run(|turn| preflight(turn, &ReadFiles::default(), &call)).unwrap_or_else(|error| panic!("{command}: {error}"));
    }
    let before = *h.parallel_calls.lock().unwrap();
    let read = call("readFile", r#"{"path":"a.rs"}"#);
    h.run(|turn| preflight(turn, &ReadFiles::default(), &read)).expect("a read needs no neighbour lookup");
    assert_eq!(*h.parallel_calls.lock().unwrap(), before, "only runCommand asks about neighbours");
}

#[test]
fn an_approved_git_tree_change_is_guarded_again_on_resume() {
    let mut h = harness(
        "parallel-git-resume",
        vec![asks(vec![wants("g1", "runCommand", r#"{"command":"git checkout main"}"#)]), text("done")],
    );
    h.approval = asking();
    let paused = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("pauses");
    let ChatStreamOutcome::PendingApproval(pending) = paused else { panic!("expected approval") };

    h.parallel = vec!["other".into()];
    let decision = ToolCallDecision { id: "g1".into(), approved: true, reason: None };
    let outcome = h.run(|turn| resume(turn, pending, vec![decision])).expect("a refusal is a tool result");
    let ChatStreamOutcome::Done(done) = outcome else { panic!("expected the model to react") };
    let result = done.history.iter().find(|message| message.tool_call_id.as_deref() == Some("g1")).expect("tool result");
    assert!(
        result.content.as_deref().is_some_and(|text| text.contains("git checkout would change the shared working tree")),
        "{result:?}"
    );
}

#[test]
fn a_round_costs_the_weight_of_every_call_in_it() {
    let cost = round_cost(&[call("readFile", "{}"), call("grep", "{}")], &McpTools::default());
    assert_eq!(cost, ToolName::ReadFile.loop_weight() + ToolName::Grep.loop_weight());
}

#[test]
fn a_round_with_no_calls_is_free() {
    assert_eq!(round_cost(&[], &McpTools::default()), 0);
}

/// Otherwise a model that keeps inventing tool names spins against a
/// budget that never moves.
#[test]
fn an_invented_tool_name_still_costs_something() {
    assert_eq!(round_cost(&[call("summonDragon", "{}")], &McpTools::default()), 1);
}

#[test]
fn the_same_read_twice_is_replaced_by_a_note() {
    let mut seen = HashMap::new();
    let one = call("readFile", r#"{"path":"a.rs"}"#);

    let first = dedupe_repeat_result(&mut seen, &one, Some(&file("x")), "x".to_string());
    let second = dedupe_repeat_result(&mut seen, &one, Some(&file("x")), "x".to_string());

    assert_eq!(first, "x");
    assert_eq!(second, REPEAT_READ_NOTE);

    let many = call("readFile", r#"{"paths":["a.rs","b.rs"]}"#);
    let files = ToolResult::Files { files: vec![] };
    dedupe_repeat_result(&mut seen, &many, Some(&files), "ab".to_string());
    assert_eq!(dedupe_repeat_result(&mut seen, &many, Some(&files), "ab".to_string()), REPEAT_READ_NOTE);
}

/// The gate that makes this safe after a write: the file changed, so the
/// model must see it, however many times it has read that path before.
#[test]
fn a_changed_file_comes_back_in_full() {
    let mut seen = HashMap::new();
    let call = call("readFile", r#"{"path":"a.rs"}"#);

    dedupe_repeat_result(&mut seen, &call, Some(&file("before")), "before".to_string());
    let after =
        dedupe_repeat_result(&mut seen, &call, Some(&file("after")), "after".to_string());
    assert_eq!(after, "after");

    // And the next identical read is deduped against the *new* content,
    // not the one from before the write.
    let again =
        dedupe_repeat_result(&mut seen, &call, Some(&file("after")), "after".to_string());
    assert_eq!(again, REPEAT_READ_NOTE);
}

#[test]
fn a_different_range_or_query_is_a_different_call() {
    let mut seen = HashMap::new();
    let body = "x".to_string();

    let first = call("readFile", r#"{"path":"a.rs","startLine":1}"#);
    let second = call("readFile", r#"{"path":"a.rs","startLine":40}"#);
    dedupe_repeat_result(&mut seen, &first, Some(&file("x")), body.clone());
    let other = dedupe_repeat_result(&mut seen, &second, Some(&file("x")), body.clone());

    assert_eq!(other, "x", "same content, but the model asked for something else");
}

#[test]
fn a_repeated_search_is_replaced_by_its_own_note() {
    let mut seen = HashMap::new();
    let call = call("grep", r#"{"pattern":"fn main"}"#);

    dedupe_repeat_result(&mut seen, &call, Some(&grep()), "hits".to_string());
    let second = dedupe_repeat_result(&mut seen, &call, Some(&grep()), "hits".to_string());

    assert_eq!(second, REPEAT_SEARCH_NOTE);
}

pub(super) fn task(id: &str, title: &str, status: crate::domain::tools::TodoStatus) -> Task {
    Task { id: id.into(), title: title.into(), status, note: None }
}

pub(super) fn mentions_checklist(message: &LlmMessage) -> bool {
    message.content.as_deref().is_some_and(|c| c.contains(CHECKLIST_LEGEND))
}

/// Each request is the previous one plus what happened since — nothing
/// re-sent at the end, which is what every prompt cache matches. The list
/// the model works from is the last `todo` result.
#[test]
fn with_a_todo_result_in_the_history_nothing_is_added_and_each_request_extends_the_last() {
    let h = harness(
        "chat-checklist-prefix",
        vec![
            asks(vec![wants("t", "todo", r#"{"op":"write","tasks":["look","fix"]}"#)]),
            asks(vec![wants("l", "listFiles", "{}")]),
            asks(vec![wants("u", "todo", r#"{"op":"update","status":"completed"}"#)]),
            text("done"),
        ],
    );
    h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");
    let requests = h.provider.requests();
    for pair in requests.windows(2) {
        assert!(pair[1].messages.starts_with(&pair[0].messages), "a request is a prefix of the next");
    }
    let with_list: Vec<usize> = conversation_of(&requests[3])
        .iter()
        .enumerate()
        .filter(|(_, m)| mentions_checklist(m))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(with_list.len(), 2, "the two todo results, nothing else: {with_list:?}");
}

/// A list the history does not show — no result at all, as after a
/// summary — goes into the history once, beside the user's message, and
/// stays there: the next request extends it.
#[test]
fn a_list_the_history_does_not_show_goes_into_it_once() {
    use crate::domain::tools::TodoStatus::{InProgress, Pending};
    let todos = vec![task("t1", "look", InProgress), task("t2", "fix", Pending)];
    let h = harness("chat-checklist-restored", vec![asks(vec![wants("l", "listFiles", "{}")]), text("ok")]);

    let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], todos.clone())).expect("turn");

    let requests = h.provider.requests();
    let first = conversation_of(&requests[0]);
    assert_eq!(first[0].content.as_deref(), Some("go"), "the user's words untouched");
    assert!(first.last().is_some_and(|m| m.role == LlmRole::User && mentions_checklist(m)));
    assert!(requests[1].messages.starts_with(&requests[0].messages));
    assert_eq!(conversation_of(&requests[1]).iter().filter(|m| mentions_checklist(m)).count(), 1, "once");
    let ChatStreamOutcome::Done(done) = outcome else { panic!() };
    assert_eq!(done.history.iter().filter(|m| mentions_checklist(m)).count(), 1, "kept for the next turn");
}

/// The latest list in the history is an older one — a branched chat —
/// and the history ends with a result not yet sent: the list joins it.
#[test]
fn a_stale_list_is_superseded_on_the_unsent_tool_result() {
    use crate::domain::tools::TodoStatus::{Completed, InProgress};
    let todos = vec![task("t1", "look", Completed), task("t2", "fix", InProgress)];
    let history = vec![
        LlmMessage::user("go"),
        LlmMessage { tool_calls: vec![wants("old", "todo", "{}")], ..LlmMessage::assistant("") },
        LlmMessage::tool_result("old", for_model(&ToolResult::Todo { tasks: vec![task("t1", "look", InProgress)] })),
        LlmMessage { tool_calls: vec![wants("l", "listFiles", "{}")], ..LlmMessage::assistant("") },
        LlmMessage::tool_result("l", "./\n└── a.txt"),
    ];
    let h = harness("chat-checklist-stale", vec![text("ok")]);

    h.run(|turn| stream(turn, history, todos.clone())).expect("turn");

    let last = conversation_of(&h.provider.requests()[0]).last().unwrap().clone();
    assert_eq!(last.role, LlmRole::Tool);
    let content = last.content.unwrap();
    assert!(content.starts_with("./\n└── a.txt\n\n[The checklist as it stands]"), "{content}");
    assert!(content.contains(&for_model(&ToolResult::Todo { tasks: todos })));
}

/// The latest list is what counts, not whether the current one appears
/// somewhere: an earlier result showing it is overruled by a later one
/// that shows something else.
#[test]
fn an_earlier_copy_of_the_list_does_not_count_once_a_later_one_differs() {
    use crate::domain::tools::TodoStatus::{InProgress, Pending};
    let todos = vec![task("t1", "look", InProgress)];
    let other = vec![task("t1", "look", InProgress), task("t2", "fix", Pending)];
    let history = vec![
        LlmMessage::user("go"),
        LlmMessage { tool_calls: vec![wants("a", "todo", "{}")], ..LlmMessage::assistant("") },
        LlmMessage::tool_result("a", for_model(&ToolResult::Todo { tasks: todos.clone() })),
        LlmMessage { tool_calls: vec![wants("b", "todo", "{}")], ..LlmMessage::assistant("") },
        LlmMessage::tool_result("b", for_model(&ToolResult::Todo { tasks: other })),
        LlmMessage::user("go on"),
    ];
    let h = harness("chat-checklist-latest", vec![text("ok")]);
    h.run(|turn| stream(turn, history.clone(), todos)).expect("turn");
    assert_eq!(conversation_of(&h.provider.requests()[0]).len(), history.len() + 1, "put back");
}

/// A summary made to fit the window folds the last `todo` result away;
/// the retry carries the list, and the prompt's promise holds.
#[test]
fn a_list_folded_into_a_summary_comes_back_on_the_retry() {
    use crate::domain::tools::TodoStatus::{InProgress, Pending};
    let todos = vec![task("t1", "look", InProgress), task("t2", "fix", Pending)];
    let mut history = vec![
        LlmMessage::user("go"),
        LlmMessage { tool_calls: vec![wants("t", "todo", "{}")], ..LlmMessage::assistant("") },
        LlmMessage::tool_result("t", for_model(&ToolResult::Todo { tasks: todos.clone() })),
    ];
    history.extend(long_conversation());
    let mut h = harness("chat-checklist-summary", vec![Step::Fail(too_long_error()), text("done")]);
    h.session.context_limit = Some(WINDOW_FOR_LONG);

    h.run(|turn| stream(turn, history, todos)).expect("turn");

    let requests = h.provider.requests();
    assert_eq!(conversation_of(&requests[0]).iter().filter(|m| mentions_checklist(m)).count(), 1, "shown, nothing added");
    assert!(conversation_of(&requests[1]).iter().any(mentions_checklist), "back after the summary");
}

/// The latest list is current, a hook's word after it: nothing to add.
#[test]
fn a_current_list_with_a_hooks_note_after_it_is_enough() {
    use crate::domain::tools::TodoStatus::{InProgress, Pending};
    let todos = vec![task("t1", "look", InProgress), task("t2", "fix", Pending)];
    let history = vec![
        LlmMessage::user("go"),
        LlmMessage { tool_calls: vec![wants("now", "todo", "{}")], ..LlmMessage::assistant("") },
        LlmMessage::tool_result("now", format!("{}\n\n[A PostToolUse hook said:]\nfine", for_model(&ToolResult::Todo { tasks: todos.clone() }))),
        LlmMessage::user("go on"),
    ];
    let h = harness("chat-checklist-current", vec![text("ok")]);
    h.run(|turn| stream(turn, history.clone(), todos)).expect("turn");
    assert_eq!(conversation_of(&h.provider.requests()[0]).len(), history.len(), "nothing added");
}

pub(super) fn last_message(request: &ChatRequest) -> &LlmMessage {
    request.messages.iter().rev().find(|m| m.role != LlmRole::System).unwrap()
}

/// A blank reply after a round of reads is sent back once, not taken for
/// the end of the task.
#[test]
fn an_empty_reply_is_sent_back_once_instead_of_ending_the_turn() {
    let h = harness("chat-empty", vec![asks(vec![wants("r", "listFiles", "{}")]), text(""), text("done")]);

    let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");

    let ChatStreamOutcome::Done(done) = outcome else { panic!("{outcome:?}") };
    assert_eq!(done.result.text, "done");
    let requests = h.provider.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(last_message(&requests[2]).content.as_deref(), Some(EMPTY_REPLY_NOTE));
    assert_eq!(last_message(&requests[2]).role, LlmRole::User);
}

/// Once: a second blank reply in the same turn ends it.
#[test]
fn a_second_empty_reply_ends_the_turn() {
    let h = harness("chat-empty-twice", vec![text(""), text(""), text("never asked")]);

    let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");

    assert!(matches!(outcome, ChatStreamOutcome::Done(ref done) if done.result.text.is_empty()), "{outcome:?}");
    assert_eq!(h.provider.requests().len(), 2);
}

/// Blank because the length limit ran out first: the note says so, since
/// "carry on" alone invites the same overlong thinking again — and the
/// round reports that it was cut.
#[test]
fn an_empty_reply_cut_off_by_the_limit_says_so() {
    let cut = Step::Reply(ChatStreamResult { truncated: true, ..Default::default() });
    let h = harness("chat-empty-cut", vec![cut, text("done")]);

    h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");

    assert_eq!(last_message(&h.provider.requests()[1]).content.as_deref(), Some(EMPTY_TRUNCATED_NOTE));
    let cut_flags: Vec<bool> = h
        .events()
        .iter()
        .filter_map(|e| match &e.event {
            ChatEventPayload::RoundCompleted { truncated, .. } => Some(*truncated),
            _ => None,
        })
        .collect();
    assert_eq!(cut_flags, [true, false]);
}

/// Blank while the user typed: what they typed carries the turn on, with
/// no nudge and no empty assistant message for a provider to refuse.
#[test]
fn an_empty_reply_while_the_user_types_goes_on_with_what_they_typed() {
    let h = harness("chat-empty-typed", vec![text_while_typing("", "also check b"), text("done")]);

    h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");

    let second = &h.provider.requests()[1];
    let conversation = conversation_of(second);
    assert!(conversation.iter().all(|m| m.role != LlmRole::Assistant), "no empty answer kept: {conversation:?}");
    assert!(last_message(second).content.as_deref().unwrap_or("").contains("also check b"));
    assert!(conversation.iter().all(|m| m.content.as_deref() != Some(EMPTY_REPLY_NOTE)));
}

/// Whitespace is not an answer either; any real text is.
#[test]
fn only_a_reply_with_something_in_it_ends_the_turn() {
    let h = harness("chat-empty-space", vec![text(" \n "), text("ok")]);
    h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");
    assert_eq!(h.provider.requests().len(), 2);

    let h = harness("chat-not-empty", vec![text("ok"), text("never asked")]);
    h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");
    assert_eq!(h.provider.requests().len(), 1);
}

/// A long turn: four big reads, then the first one is cleared. From then
/// on the turn behaves as if the model had never seen it — the stub is
/// what the model reads, a wholesale write needs a fresh read, and that
/// read comes back in full rather than as "already above".
#[test]
fn a_cleared_read_is_gone_from_the_history_and_from_the_turn() {
    // The first read as one path, then as one of `paths`: either is forgotten.
    for (label, first) in [("chat-clearing", r#"{"path":"a.txt"}"#), ("chat-clearing-paths", r#"{"paths":["a.txt"]}"#)] {
        let read = |id: &str, path: &str| wants(id, "readFile", &format!(r#"{{"path":"{path}"}}"#));
        let h = harness(
            label,
            vec![
                asks(vec![wants("r1", "readFile", first)]),
                asks(vec![read("r2", "b.txt")]),
                asks(vec![read("r3", "c.txt")]),
                asks(vec![read("r4", "d.txt")]),
                asks(vec![wants("w", "writeFile", r#"{"path":"a.txt","content":"new"}"#)]),
                asks(vec![read("r5", "a.txt")]),
                text("done"),
            ],
        );
        for name in ["a.txt", "b.txt", "c.txt", "d.txt"] {
            std::fs::write(h.root.join(name), name.repeat(90_000 / name.len())).unwrap();
        }

        h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");
        let requests = h.provider.requests();

        // Round 5 is the first to start over the trigger: a.txt's result is
        // the one old enough to go.
        let fifth = tool_contents(&requests[4]);
        assert!(fifth[0].starts_with(result_clearing::STUB_PREFIX), "{}", &fifth[0][..80]);
        assert!(fifth[1..].iter().all(|c| c.starts_with("All 1 lines")), "the last three rounds stay");
        assert!(!tool_contents(&requests[3])[0].starts_with(result_clearing::STUB_PREFIX), "not before the trigger");

        let sixth = tool_contents(&requests[5]);
        assert!(sixth.last().unwrap().contains("only read part of a.txt"), "{}", sixth.last().unwrap());
        assert_eq!(std::fs::read_to_string(h.root.join("a.txt")).unwrap().len(), 90_000, "not written");

        let seventh = tool_contents(&requests[6]);
        assert!(seventh.last().unwrap().starts_with("All 1 lines"), "read again in full");
    }
}

/// Only results the model can re-derive from the transcript are worth
/// suppressing. A write confirmation is one line and has to be seen every
/// time, and an error is worth repeating as often as it happens.
#[test]
fn nothing_else_is_ever_suppressed() {
    let mut seen = HashMap::new();
    let written = ToolResult::Moved { from: "a".to_string(), to: "b".to_string(), files: None };
    let call = call("move", r#"{"path":"a","newPath":"b"}"#);

    for _ in 0..3 {
        let content =
            dedupe_repeat_result(&mut seen, &call, Some(&written), "moved".to_string());
        assert_eq!(content, "moved");
    }
    for _ in 0..3 {
        let content = dedupe_repeat_result(&mut seen, &call, None, "failed".to_string());
        assert_eq!(content, "failed");
    }
}

#[test]
fn a_failed_call_from_a_cut_off_round_is_told_why() {
    let note = truncated_round_note(true, true, "invalid arguments".to_string());
    assert!(note.starts_with("invalid arguments"), "{note}");
    assert!(note.contains("max_tokens"), "{note}");
}

/// A call that closed before the cut executed correctly; calling its
/// result damaged would be worse than saying nothing.
#[test]
fn a_call_that_succeeded_is_not_told_the_round_was_cut() {
    assert_eq!(truncated_round_note(true, false, "ok".to_string()), "ok");
}

#[test]
fn an_ordinary_failure_carries_no_note() {
    assert_eq!(
        truncated_round_note(false, true, "no such file".to_string()),
        "no such file"
    );
}
