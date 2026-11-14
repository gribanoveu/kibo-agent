use super::*;

// ---------------------------------------------------------- loop guard

pub(super) fn loop_notes(request: &ChatRequest) -> Vec<String> {
    request
        .messages
        .iter()
        .filter(|m| m.role == LlmRole::User)
        .filter_map(|m| m.content.clone())
        .filter(|c| c.starts_with("[Loop guard]"))
        .collect()
}

pub(super) fn loop_events(h: &Harness) -> Vec<String> {
    payloads(&h.events()).into_iter().filter(|p| p.starts_with("loop:")).collect()
}

/// The same listing three rounds running: the fourth request carries the
/// note, once, and the reader is told too.
#[test]
fn the_same_call_three_rounds_running_is_pointed_out_once() {
    let steps = (1..=5).map(|n| asks(vec![wants(&format!("l{n}"), "listFiles", "{}")])).chain([text("done")]).collect();
    let h = harness("loop-guard-same", steps);

    let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("look")], vec![])).expect("turn");

    assert!(matches!(outcome, ChatStreamOutcome::Done(_)));
    let requests = h.provider.requests();
    assert!(loop_notes(&requests[2]).is_empty(), "two rounds are not a loop");
    assert_eq!(last_message(&requests[3]).content.as_deref(), Some(Loop::SameCall { tool: "listFiles".into() }.note().as_str()));
    assert_eq!(loop_notes(requests.last().unwrap()).len(), 1, "once a turn");
    assert_eq!(loop_events(&h), vec!["loop:listFiles:false"]);
}

/// The same failure with other arguments each time — a regex that never
/// compiles fails in the tool itself, a file never read is refused before
/// the call runs; both are the route, not the details.
#[test]
fn the_same_failure_three_rounds_running_is_pointed_out() {
    for (label, tool, args) in [
        ("loop-guard-regex", "grep", [r#"{"pattern":"("}"#, r#"{"pattern":"(a"}"#, r#"{"pattern":"(b"}"#]),
        ("loop-guard-unread", "writeFile", [
            r#"{"path":"a.rs","content":"x"}"#,
            r#"{"path":"a.rs","content":"y"}"#,
            r#"{"path":"a.rs","content":"z"}"#,
        ]),
    ] {
        let steps = args.iter().enumerate().map(|(n, a)| asks(vec![wants(&format!("c{n}"), tool, a)])).chain([text("done")]).collect();
        let h = harness(label, steps);
        if tool == "writeFile" {
            std::fs::write(h.root.join("a.rs"), "old").unwrap();
        }

        h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");

        assert_eq!(told_errors(&h).len(), 3, "{label}: every call failed");
        let last = h.provider.requests().last().unwrap().clone();
        assert_eq!(loop_notes(&last), vec![Loop::SameError { tool: tool.into() }.note()], "{label}");
        assert_eq!(loop_events(&h), vec![format!("loop:{tool}:true")], "{label}");
    }
}

/// Calls the user denied are their answer, not the model going round.
#[test]
fn denied_calls_are_not_counted() {
    let write = r#"{"path":"a.rs","content":"x"}"#;
    let mut h = harness(
        "loop-guard-denied",
        vec![asks(vec![wants("w1", "writeFile", write), wants("w2", "writeFile", write), wants("w3", "writeFile", write)]), text("ok")],
    );
    h.approval = asking();

    let paused = h.run(|turn| stream(turn, vec![LlmMessage::user("write")], vec![]));
    let Ok(ChatStreamOutcome::PendingApproval(pending)) = paused else { panic!("expected a pause") };
    let no = |id: &str| ToolCallDecision { id: id.to_string(), approved: false, reason: None };
    h.run(|turn| resume(turn, pending, vec![no("w1"), no("w2"), no("w3")])).expect("finishes");

    assert!(loop_notes(h.provider.requests().last().unwrap()).is_empty());
}

// ------------------------------------------------------------- the loop

#[test]
fn a_turn_with_no_tool_calls_answers_and_stops() {
    let h = harness("loop-plain", vec![text("done")]);

    let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("hi")], vec![]));

    let ChatStreamOutcome::Done(done) = outcome.expect("finishes") else {
        panic!("expected Done");
    };
    assert_eq!(done.result.text, "done");
    assert_eq!(h.provider.requests().len(), 1, "one round, one request");
}

pub(super) fn too_long_error() -> LlmError {
    LlmError::Http(
        "http status 400: This model's maximum context length is 8192 tokens".to_string(),
    )
}

/// Forty messages of about a thousand tokens each.
pub(super) fn long_conversation() -> Vec<LlmMessage> {
    let filler = "x".repeat(4_000);
    (0..40)
        .map(|i| {
            if i % 2 == 0 {
                LlmMessage::user(format!("question {i} {filler}"))
            } else {
                LlmMessage::assistant(format!("answer {i} {filler}"))
            }
        })
        .collect()
}

/// A window the long conversation fits in with room to spare, so nothing
/// folds ahead of time — and a tenth of which, the tail after a refusal,
/// holds six of its messages.
pub(super) const WINDOW_FOR_LONG: u32 = 70_000;

/// The failure this exists for: the provider refuses because the
/// conversation no longer fits, and the turn ends. Now it makes room and
/// asks again, and the user never learns there was a problem.
#[test]
fn a_conversation_that_no_longer_fits_is_summarized_and_asked_again() {
    let mut h = harness(
        "loop-too-long",
        vec![Step::Fail(too_long_error()), text("done")],
    );
    h.session.context_limit = Some(WINDOW_FOR_LONG);

    let outcome = h.run(|turn| stream(turn, long_conversation(), vec![]));

    let ChatStreamOutcome::Done(done) = outcome.expect("finishes") else {
        panic!("expected Done");
    };
    assert_eq!(done.result.text, "done");

    let requests = h.provider.requests();
    assert_eq!(requests.len(), 2, "the refused round and the retry");
    assert!(
        requests[1].messages.len() < requests[0].messages.len(),
        "asked again with the same history"
    );
    assert!(
        conversation_of(&requests[1])[0]
            .content
            .as_deref()
            .unwrap()
            .contains("they were fixing the parser"),
        "the summary opens the shorter conversation"
    );
    assert_eq!(h.provider.summaries.lock().unwrap().len(), 1);
}

/// A plan written mid-turn leaves the system prompt alone — a prompt that
/// changed would cost the prompt cache the whole history — and reaches
/// the next request through its own call.
#[test]
fn a_plan_written_in_the_turn_leaves_the_prompt_alone() {
    let h = harness(
        "chat-plan-kept",
        vec![asks(vec![wants("p", "writePlan", r##"{"content":"# Notes\n\nEmployeeController: L13-L40"}"##)]), text("done")],
    );
    h.run(|turn| stream(turn, vec![LlmMessage::user("document it")], vec![])).expect("turn");

    let prompt_says = |request: &ChatRequest| {
        request.messages.iter().any(|m| m.role == LlmRole::System && m.content.as_deref().is_some_and(|c| c.contains("EmployeeController: L13-L40")))
    };
    let requests = h.provider.requests();
    assert!(!prompt_says(&requests[1]), "the prompt changed mid-turn");
    let lead = lead_of(&requests[0]);
    assert_eq!(requests[0].messages[..lead], requests[1].messages[..lead_of(&requests[1])], "the prefix moved");
    let in_call = conversation_of(&requests[1]).iter().flat_map(|m| &m.tool_calls).any(|c| c.arguments.contains("EmployeeController: L13-L40"));
    assert!(in_call, "the plan is in its call");
}

pub(super) fn summarized(tail: Vec<LlmMessage>) -> Vec<LlmMessage> {
    let mut history = vec![compaction::summary_message("they read the controllers")];
    history.extend(tail);
    history
}

pub(super) fn mentions_plan(message: &LlmMessage) -> bool {
    message.content.as_deref().is_some_and(|c| c.contains(PLAN_NOTE))
}

/// The plan goes into the history only when the latest one there — a
/// `writePlan` call or a note — is not it, and once: the second pass over
/// the same history adds nothing.
#[test]
fn a_plan_the_history_does_not_show_goes_into_it_once() {
    let written = |content: &str| LlmMessage {
        tool_calls: vec![wants("p", "writePlan", &serde_json::json!({ "content": content }).to_string())],
        ..LlmMessage::assistant("")
    };

    let mut folded = summarized(vec![LlmMessage::user("go on")]);
    restore_plan(&mut folded, Some("# Notes\n\nL13\n"));
    assert_eq!(folded.len(), 3, "a message of its own after the user's");
    assert!(folded[2].content.as_deref().unwrap().ends_with("<plan>\n# Notes\n\nL13\n</plan>"), "{:?}", folded[2]);
    let before = folded.clone();
    restore_plan(&mut folded, Some("# Notes\n\nL13"));
    assert_eq!(folded, before, "a plan already shown is not shown twice");

    let mut kept = vec![LlmMessage::user("go"), written("# Notes\n"), LlmMessage::tool_result("p", "ok")];
    let before = kept.clone();
    restore_plan(&mut kept, Some("# Notes"));
    assert_eq!(kept, before, "the call shows it");

    let mut edited = kept.clone();
    restore_plan(&mut edited, Some("# Notes, edited"));
    let last = edited.last().unwrap();
    assert_eq!((last.role, edited.len()), (LlmRole::Tool, 3), "onto the unsent result");
    assert!(last.content.as_deref().unwrap().starts_with("ok\n\n[The plan as it stands]"), "{last:?}");

    // A note from an earlier edit is overruled by a later call.
    let mut rewritten = edited.clone();
    rewritten.extend([written("# Other"), LlmMessage::tool_result("q", "ok")]);
    restore_plan(&mut rewritten, Some("# Notes, edited"));
    assert!(mentions_plan(rewritten.last().unwrap()), "the later call no longer showed the plan");

    // The latest counts, not any: a plan the user went back to after a
    // later call, a note overruled by a later one, the last of two calls.
    let mut reverted = vec![LlmMessage::user("go"), written("# A"), LlmMessage::tool_result("p", "ok"), written("# B"), LlmMessage::tool_result("q", "ok")];
    restore_plan(&mut reverted, Some("# A"));
    assert!(mentions_plan(reverted.last().unwrap()), "an earlier call showed it, the later one did not");
    let mut noted = vec![LlmMessage::user("go")];
    restore_plan(&mut noted, Some("# A"));
    restore_plan(&mut noted, Some("# B"));
    assert!(noted[1].content.as_deref().unwrap().ends_with("<plan>\n# B\n</plan>"), "{:?}", noted[1]);
    let twice = LlmMessage {
        tool_calls: vec![
            wants("p", "writePlan", r##"{"content":"# A"}"##),
            wants("q", "writePlan", r##"{"content":"# B"}"##),
        ],
        ..LlmMessage::assistant("")
    };
    let mut both = vec![LlmMessage::user("go"), twice, LlmMessage::tool_result("p", "ok"), LlmMessage::tool_result("q", "ok")];
    let before = both.clone();
    restore_plan(&mut both, Some("# B"));
    assert_eq!(both, before, "the second call of a round is the plan");

    for nothing in [None, Some("  \n ")] {
        let mut none = summarized(vec![LlmMessage::user("go on")]);
        restore_plan(&mut none, nothing);
        assert_eq!(none.len(), 2, "a blank plan is no plan");
    }
}

/// The plan and the checklist put back together share one message
/// rather than leaving two of theirs in a row after the user's.
#[test]
fn the_plan_and_the_checklist_share_a_note() {
    use crate::domain::tools::TodoStatus::InProgress;
    let mut history = vec![LlmMessage::user("go")];
    restore_plan(&mut history, Some("# Plan"));
    restore_checklist(&mut history, &[task("t1", "look", InProgress)]);
    assert_eq!(history.len(), 2);
    assert!(mentions_plan(&history[1]) && mentions_checklist(&history[1]), "{:?}", history[1]);
}

/// The whole way round: a plan written, a fold that takes its call
/// away, and the retry that follows reads the plan after the summary.
#[test]
fn a_plan_folded_away_with_its_call_comes_back_after_the_summary() {
    let mut h = harness(
        "chat-plan-folded",
        vec![
            asks(vec![wants("p", "writePlan", r##"{"content":"# Notes\n\nEmployeeController: L13-L40"}"##)]),
            asks(vec![wants("r", "readFile", r#"{"path":"big.txt"}"#)]),
            Step::Fail(too_long_error()),
            text("done"),
        ],
    );
    // A tenth of it, the tail after a refusal, holds the read and not
    // the plan's call before it.
    h.session.context_limit = Some(90_000);
    std::fs::write(h.root.join("big.txt"), "x".repeat(40_000)).unwrap();

    h.run(|turn| stream(turn, long_conversation(), vec![])).expect("turn");

    let requests = h.provider.requests();
    let retry = conversation_of(requests.last().unwrap());
    let noted: Vec<&LlmMessage> = retry.iter().filter(|m| mentions_plan(m)).collect();
    assert_eq!(noted.len(), 1, "{retry:?}");
    assert!(noted[0].content.as_deref().unwrap().contains("<plan>\n# Notes\n\nEmployeeController: L13-L40\n</plan>"));
    assert!(!retry.iter().flat_map(|m| &m.tool_calls).any(|c| c.name == "writePlan"), "the call was not folded — this proves nothing");
}

/// A turn that starts near the edge folds before it asks, rather than
/// waiting to be refused: the summary comes first, keeping a quarter of
/// the window word for word, and the window is told.
#[test]
fn a_turn_over_the_mark_is_summarized_before_its_first_request() {
    let mut h = harness("loop-fold-ahead", vec![text("done")]);
    // 40 messages of ~1 000 tokens in 45 000: over 90% with the prompt.
    h.session.context_limit = Some(45_000);

    h.run(|turn| stream(turn, long_conversation(), vec![])).expect("turn");

    let requests = h.provider.requests();
    assert_eq!(requests.len(), 1, "never refused, never retried");
    assert_eq!(h.provider.summaries.lock().unwrap().len(), 1);
    let conversation = conversation_of(&requests[0]);
    assert!(conversation[0].content.as_deref().unwrap().contains("they were fixing the parser"), "the summary opens it");
    // A quarter of 45 000 holds eleven of these messages.
    assert_eq!(conversation.len(), 1 + 11);
    let events: Vec<String> = payloads(&h.events()).into_iter().filter(|p| p.starts_with("compact")).collect();
    assert_eq!(events, ["compacting", "compacted:29"]);
}

/// With room to spare nothing is folded — the mark is 90%, not "long".
#[test]
fn a_turn_with_room_is_not_summarized() {
    let mut h = harness("loop-no-fold", vec![text("done")]);
    h.session.context_limit = Some(WINDOW_FOR_LONG);
    h.run(|turn| stream(turn, long_conversation(), vec![])).expect("turn");
    assert!(h.provider.summaries.lock().unwrap().is_empty());
}

/// History disappearing on its own is the thing to avoid: the model stops
/// remembering what it was told, and nothing in the window says why. The
/// summary takes seconds, so the start is said too, before it.
#[test]
fn the_transcript_is_told_that_history_was_folded_away() {
    let mut h = harness(
        "loop-too-long-event",
        vec![Step::Fail(too_long_error()), text("done")],
    );
    h.session.context_limit = Some(WINDOW_FOR_LONG);

    h.run(|turn| stream(turn, long_conversation(), vec![]))
        .expect("finishes");

    let compacted: Vec<String> = payloads(&h.events())
        .into_iter()
        .filter(|p| p.starts_with("compact"))
        .collect();
    assert_eq!(compacted, ["compacting", "compacted:34"]);
}

/// A second refusal after the history has already been summarized is not
/// about its length. Summarizing again would spend another request to lose
/// more of the conversation and fail anyway.
#[test]
fn a_conversation_is_summarized_once_and_then_the_refusal_stands() {
    let mut h = harness(
        "loop-too-long-twice",
        vec![Step::Fail(too_long_error()), Step::Fail(too_long_error())],
    );
    h.session.context_limit = Some(WINDOW_FOR_LONG);

    let outcome = h.run(|turn| stream(turn, long_conversation(), vec![]));

    assert!(matches!(outcome, Err(TurnError::Provider(_))), "{outcome:?}");
    assert_eq!(h.provider.summaries.lock().unwrap().len(), 1, "summarized twice");
}

/// Not every overflow is a long conversation: one enormous file read
/// fills the window on its own, and there is nothing to summarize. The
/// refusal is then the useful thing to report — and no request is spent
/// discovering that.
#[test]
fn a_short_conversation_that_does_not_fit_is_reported_rather_than_summarized() {
    let h = harness("loop-too-long-short", vec![Step::Fail(too_long_error())]);

    let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("x".repeat(9_000))], vec![]));

    assert!(matches!(outcome, Err(TurnError::Provider(_))), "{outcome:?}");
    assert!(h.provider.summaries.lock().unwrap().is_empty());
}

/// Any other refusal is reported as it always was — answering one with a
/// summarizing request costs money and loses history for nothing.
#[test]
fn another_kind_of_refusal_is_not_answered_by_summarizing() {
    let h = harness(
        "loop-other-error",
        vec![Step::Fail(LlmError::Http("http status 401: invalid api key".to_string()))],
    );

    let outcome = h.run(|turn| stream(turn, long_conversation(), vec![]));

    assert!(matches!(outcome, Err(TurnError::Provider(_))), "{outcome:?}");
    assert!(h.provider.summaries.lock().unwrap().is_empty());
}

/// The loop's actual job: a call runs, and what it produced goes back to
/// the model as the next request's history.
#[test]
fn a_tool_result_reaches_the_next_round() {
    std::fs::write("/dev/null", "").ok();
    let h = harness(
        "loop-tool",
        vec![
            asks(vec![wants("c1", "writeFile", r#"{"path":"src/a.txt","content":"x"}"#)]),
            text("made it"),
        ],
    );

    let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("make src")], vec![]));

    assert!(matches!(outcome.expect("finishes"), ChatStreamOutcome::Done(_)));
    assert!(h.root.join("src/a.txt").is_file(), "the tool actually ran");

    let second = &h.provider.requests()[1];
    let results = tool_contents(second);
    assert_eq!(results.len(), 1);
    assert!(results[0].starts_with("Wrote src/a.txt"), "{}", results[0]);
    // The assistant's own tool-call turn has to precede its results, or
    // the provider sees answers to a question it was never shown.
    let assistant = second
        .messages
        .iter()
        .find(|m| m.role == LlmRole::Assistant)
        .expect("the tool-call turn is in the history");
    assert_eq!(assistant.tool_calls.len(), 1);
}

/// The order is the contract: a listener pairs a call with its result by
/// id, and orders everything by `seq`.
#[test]
fn events_are_numbered_in_order_and_pair_by_id() {
    let h = harness(
        "loop-events",
        vec![
            asks(vec![wants("c1", "writeFile", r#"{"path":"a","content":"x"}"#)]),
            text("ok"),
        ],
    );

    h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");

    let events = h.events();
    assert_eq!(
        payloads(&events),
        [
            "roundStarted",
            "estimate",
            "roundCompleted",
            "toolCall:c1",
            "toolResult:c1",
            "roundStarted",
            "estimate",
            "delta",
            "roundCompleted",
        ]
    );
    let seqs: Vec<u64> = events.iter().map(|e| e.seq).collect();
    assert_eq!(seqs, (1..=events.len() as u64).collect::<Vec<_>>());
    assert_eq!(events[0].round, 1);
    assert_eq!(events.last().unwrap().round, 2);
}

/// The meter follows the turn: each round says what its request costs, and
/// the second carries the first's call and result on top.
#[test]
fn each_round_says_what_its_request_costs() {
    let h = harness(
        "loop-estimate",
        vec![asks(vec![wants("c1", "writeFile", r#"{"path":"a","content":"x"}"#)]), text("ok")],
    );
    h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");

    let totals: Vec<usize> = h
        .events()
        .iter()
        .filter_map(|e| match &e.event {
            ChatEventPayload::ContextEstimate(usage) => Some(usage.total),
            _ => None,
        })
        .collect();
    assert_eq!(totals.len(), 2, "{totals:?}");
    assert!(totals[0] > 0 && totals[1] > totals[0], "{totals:?}");
}
