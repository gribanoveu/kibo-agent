use super::*;

// ----------------------------------------------------------- cancelling

#[test]
fn a_stop_before_the_first_round_runs_nothing() {
    let h = harness("loop-cancel-early", vec![]);
    h.cancel_at_poll(1);

    let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]));

    assert!(matches!(outcome.expect("stops"), ChatStreamOutcome::Cancelled(_)));
    assert!(h.provider.requests().is_empty(), "the model was never asked");
}

/// The point of the second checkpoint: a stop that lands as the round
/// finishes pre-empts the write that round asked for, not merely the
/// model's next sentence.
#[test]
fn a_stop_as_the_round_finishes_pre_empts_its_tool_calls() {
    let h = harness(
        "loop-cancel-mid",
        vec![Step::Reply(ChatStreamResult {
            text: "making it".to_string(),
            tool_calls: vec![wants("w1", "writeFile", r#"{"path":"never","content":"x"}"#)],
            ..Default::default()
        })],
    );
    // Poll 1 is the top of the round; poll 2 is the provider's own
    // cancellation callback; poll 3 is the checkpoint after it returns.
    h.cancel_at_poll(3);

    let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]));

    let ChatStreamOutcome::Cancelled(done) = outcome.expect("stops") else {
        panic!("not cancelled")
    };
    assert!(!h.root.join("never").exists(), "the call was pre-empted");
    // A request for tools with no results after it is refused by the
    // provider on the next message: the unrun call gets one saying so.
    let shape: Vec<(LlmRole, Option<&str>)> =
        done.history.iter().map(|m| (m.role, m.tool_call_id.as_deref())).collect();
    assert_eq!(shape, [(LlmRole::User, None), (LlmRole::Assistant, None), (LlmRole::Tool, Some("w1"))]);
    assert!(done.history[2].content.as_deref().is_some_and(|c| c.starts_with("Not run")));
    assert_eq!(done.history[1].content.as_deref(), Some("making it"), "what the round said is kept, once");
    assert!(
        !payloads(&h.events()).iter().any(|p| p.starts_with("toolCall:")),
        "and was never even announced"
    );
}

// ------------------------------------------------------------- retrying

pub(super) fn rate_limited(seconds: u64) -> LlmError {
    LlmError::RateLimited {
        retry_after_seconds: Some(seconds),
        message: "slow down".to_string(),
    }
}

#[test]
fn a_rate_limited_round_waits_and_runs_again() {
    let h = harness(
        "loop-retry",
        vec![Step::Fail(rate_limited(3)), text("second time lucky")],
    );

    let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]));

    let ChatStreamOutcome::Done(done) = outcome.expect("finishes") else {
        panic!("expected Done");
    };
    assert_eq!(done.result.text, "second time lucky");
    assert_eq!(h.provider.requests().len(), 2, "the same round, twice");
    assert_eq!(
        h.slept.lock().unwrap().iter().sum::<Duration>(),
        Duration::from_secs(3),
        "waited exactly as long as the server asked"
    );
    assert!(payloads(&h.events()).contains(&"retrying".to_string()));
}

/// The rule that makes retrying safe at all: half the round has already
/// reached the transcript, and sending the request again would append the
/// text twice.
#[test]
fn a_round_that_already_streamed_is_not_retried() {
    let h = harness(
        "loop-retry-unsafe",
        vec![Step::StreamThenFail("half an answer", rate_limited(1))],
    );

    let err = h
        .run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]))
        .expect_err("gives up");

    assert!(matches!(err, TurnError::Provider(LlmError::RateLimited { .. })), "{err}");
    assert_eq!(h.provider.requests().len(), 1, "asked once, never repeated");
    assert!(h.slept.lock().unwrap().is_empty());
}

/// A refusal the provider meant is not retried at all.
#[test]
fn a_considered_refusal_ends_the_turn() {
    let h = harness(
        "loop-refusal",
        vec![Step::Fail(LlmError::Http("http status 400: no such model".to_string()))],
    );

    let err = h
        .run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]))
        .expect_err("fails");

    assert!(matches!(err, TurnError::Provider(_)), "{err}");
    assert_eq!(h.provider.requests().len(), 1);
}

#[test]
fn a_stop_during_a_retry_wait_takes_effect_inside_it() {
    let h = harness("loop-retry-cancel", vec![Step::Fail(rate_limited(60))]);
    // Past the round's own checkpoints, so the stop lands in the wait.
    h.cancel_at_poll(4);

    let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]));

    assert!(matches!(outcome.expect("stops"), ChatStreamOutcome::Cancelled(_)));
    assert!(
        h.slept.lock().unwrap().iter().sum::<Duration>() < Duration::from_secs(60),
        "the stop was not made to wait out the whole window"
    );
}

// -------------------------------------------------------------- ceilings

/// A model that never stops asking for tools must not hold the turn open
/// forever — and it is the user's ceiling that stops it.
#[test]
fn a_turn_that_never_finishes_is_cut_off_at_the_set_rounds() {
    let steps = (0..10)
        .map(|i| {
            asks(vec![wants(
                &format!("c{i}"),
                "writeFile",
                &format!(r#"{{"path":"d{i}","content":"x"}}"#),
            )])
        })
        .collect();
    let mut h = harness("loop-ceiling", steps);
    h.session.limits = crate::domain::settings::TurnLimits { rounds: 3, budget: 250, parallel: 3 };

    let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("loop forever")], vec![])).expect("ends");

    let ChatStreamOutcome::Done(done) = outcome else { panic!("expected done, got {outcome:?}") };
    assert_eq!(done.limit_reached, Some(3));
    assert_eq!(h.provider.requests().len(), 3);
    // The work survives the stop: every round's call and its result are
    // in the history "continue" is sent with, not only the first message.
    let calls: Vec<&str> = done.history.iter().flat_map(|m| m.tool_calls.iter().map(|c| c.id.as_str())).collect();
    let results: Vec<&str> = done.history.iter().filter_map(|m| m.tool_call_id.as_deref()).collect();
    assert_eq!(calls, ["c0", "c1", "c2"]);
    assert_eq!(results, ["c0", "c1", "c2"]);
}

/// The weighted budget is the user's too: two writes a round, at two
/// each, spend a budget of five within two rounds.
#[test]
fn a_turn_is_cut_off_at_the_set_budget() {
    let steps = (0..10)
        .map(|i| asks((0..2).map(|j| wants(&format!("c{i}-{j}"), "writeFile", &format!(r#"{{"path":"d{i}-{j}","content":"x"}}"#))).collect()))
        .collect();
    let mut h = harness("budget-ceiling", steps);
    h.session.limits = crate::domain::settings::TurnLimits { rounds: 60, budget: 5, parallel: 3 };

    let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("loop forever")], vec![])).expect("ends");

    let ChatStreamOutcome::Done(done) = outcome else { panic!("expected done, got {outcome:?}") };
    assert_eq!(done.limit_reached, Some(2));
}

/// A turn that ends on its own says nothing about limits — the window
/// would otherwise tell the user to continue a finished task.
#[test]
fn a_finished_turn_reached_no_limit() {
    let h = harness("no-ceiling", vec![text("done")]);
    let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("ends");
    let ChatStreamOutcome::Done(done) = outcome else { panic!("expected done, got {outcome:?}") };
    assert_eq!(done.limit_reached, None);
}
