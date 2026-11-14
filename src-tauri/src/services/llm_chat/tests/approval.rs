use super::*;

// ------------------------------------------------------------- approval

pub(super) fn asking() -> ApprovalPolicy {
    ApprovalPolicy::default()
}

/// "Always allow runCommand" lets the build through and still stops a
/// force push — which the card then explains. A command that only reads
/// needs nothing at all.
#[test]
fn a_command_past_undoing_asks_with_its_reason_and_a_read_does_not_ask() {
    let mut h = harness(
        "command-reason",
        vec![asks(vec![
            wants("r1", "runCommand", r#"{"command":"git status"}"#),
            wants("r2", "runCommand", r#"{"command":"git pf"}"#),
        ])],
    );
    h.approval = ApprovalPolicy::default();
    h.approval.allow_always("runCommand").unwrap();
    h.approval.git_aliases.insert("pf".into(), "push --force".into());

    let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]));
    let ChatStreamOutcome::PendingApproval(pending) = outcome.expect("pauses") else {
        panic!("expected a pause");
    };
    let asks: Vec<(bool, Option<&str>)> =
        pending.calls.iter().map(|c| (c.requires_confirmation, c.reason.as_deref())).collect();
    assert_eq!(asks, [(false, None), (true, Some("rewrites a remote (git push --force)"))]);
}

/// Nothing in the round runs — not even the calls that needed no decision.
/// A half-executed round is a state nobody could describe to whoever
/// resumes it.
#[test]
fn a_risky_call_pauses_the_whole_round_with_nothing_run() {
    let mut h = harness(
        "loop-pause",
        vec![asks(vec![
            wants("l1", "listFiles", "{}"),
            wants("w1", "writeFile", r#"{"path":"a.rs","content":"x"}"#),
        ])],
    );
    h.approval = asking();

    let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]));

    let ChatStreamOutcome::PendingApproval(pending) = outcome.expect("pauses") else {
        panic!("expected a pause");
    };
    assert_eq!(
        pending.calls.iter().map(|c| c.requires_confirmation).collect::<Vec<_>>(),
        [false, true],
        "both calls are carried, only the write needs an answer"
    );
    assert!(!h.root.join("a.rs").exists());
    assert!(
        !payloads(&h.events()).iter().any(|p| p.starts_with("toolCall:")),
        "the harmless call did not run either — nothing in the round did"
    );
    assert_eq!(pending.round, 1);
    assert!(pending.budget_used > 0, "a paused round is still charged");
    assert!(
        payloads(&h.events()).contains(&"roundCompleted".to_string()),
        "a round that pauses has still reported what it said"
    );
}

/// The registry has to cross the pause. Without it the write the user just
/// approved is refused for never having read the file — the pause itself
/// would be what broke it.
#[test]
fn a_read_from_before_the_pause_still_counts_after_it() {
    std::fs::write(temp_dir("loop-seed").join("ignored"), "").ok();
    let mut h = harness(
        "loop-resume-reads",
        vec![
            asks(vec![wants("r1", "readFile", r#"{"path":"a.rs"}"#)]),
            asks(vec![wants(
                "w1",
                "writeFile",
                r#"{"path":"a.rs","content":"new"}"#,
            )]),
            text("written"),
        ],
    );
    h.approval = asking();
    std::fs::write(h.root.join("a.rs"), "old").unwrap();

    let paused = h.run(|turn| stream(turn, vec![LlmMessage::user("rewrite a.rs")], vec![]));
    let ChatStreamOutcome::PendingApproval(pending) = paused.expect("pauses") else {
        panic!("expected a pause");
    };

    let resumed = h.run(|turn| {
        resume(
            turn,
            pending,
            vec![ToolCallDecision {
                id: "w1".to_string(),
                approved: true,
                reason: None,
            }],
        )
    });

    assert!(matches!(resumed.expect("finishes"), ChatStreamOutcome::Done(_)));
    assert_eq!(std::fs::read_to_string(h.root.join("a.rs")).unwrap(), "new");
}

/// A refusal is not a failure of the turn, and the reason is what stops
/// the model from trying the same call again.
#[test]
fn a_denied_call_hands_the_model_the_reason_and_the_turn_continues() {
    let mut h = harness(
        "loop-denied",
        vec![
            asks(vec![wants("w1", "writeFile", r#"{"path":"a.rs","content":"x"}"#)]),
            text("understood"),
        ],
    );
    h.approval = asking();

    let paused = h.run(|turn| stream(turn, vec![LlmMessage::user("write it")], vec![]));
    let ChatStreamOutcome::PendingApproval(pending) = paused.expect("pauses") else {
        panic!("expected a pause");
    };
    let resumed = h.run(|turn| {
        resume(
            turn,
            pending,
            vec![ToolCallDecision {
                id: "w1".to_string(),
                approved: false,
                reason: Some("use the existing helper".to_string()),
            }],
        )
    });

    assert!(matches!(resumed.expect("finishes"), ChatStreamOutcome::Done(_)));
    assert!(!h.root.join("a.rs").exists(), "a denied call must not run");
    let told = tool_contents(h.provider.requests().last().unwrap());
    assert!(told[0].contains("use the existing helper"), "{}", told[0]);
    assert!(told[0].ends_with(AFTER_DENIAL), "what to do next arrives with the refusal: {}", told[0]);
}

/// Every settled call reaches the log, each with how it ended — and none
/// of the text the calls carried, whichever way they ended.
#[test]
fn every_call_is_logged_without_its_content() {
    const LEAK: &str = "LEAK-marker";
    let h = harness(
        "loop-log",
        vec![
            asks(vec![
                wants("w1", "writeFile", &format!(r#"{{"path":"a.rs","content":"{LEAK}"}}"#)),
                wants("r1", "readFile", r#"{"path":"a.rs"}"#),
                wants("e1", "editFile", &format!(r#"{{"path":"a.rs","edits":[{{"old":"absent {LEAK}","new":"x"}}]}}"#)),
                wants("b1", "writeFile", &format!(r#"{{"path":"b.rs","content":["{LEAK}"]}}"#)),
            ]),
            asks(vec![wants("w2", "writeFile", &format!(r#"{{"path":"c.rs","content":"{LEAK}"}}"#))]),
            text("done"),
        ],
    );
    h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");

    let logged = h.logged.lock().unwrap().clone();
    let summary: Vec<(&str, CallStatus)> = logged.iter().map(|e| (e.tool.as_str(), e.status)).collect();
    // In the order they settled: the broken one is refused before the
    // rest of its round runs.
    assert_eq!(
        summary,
        [
            ("writeFile", CallStatus::Error),
            ("writeFile", CallStatus::Ok),
            ("readFile", CallStatus::Ok),
            ("editFile", CallStatus::Error),
            ("writeFile", CallStatus::Ok),
        ]
    );
    for entry in &logged {
        let text = serde_json::to_string(entry).unwrap();
        assert!(!text.contains(LEAK), "{text}");
        assert_eq!((entry.provider_id.as_str(), entry.model.as_str()), ("test", "m"));
    }
    assert_eq!(logged[1].args["args"]["path"], "a.rs");
    assert_eq!(logged[0].args, serde_json::Value::Null, "unparsed arguments are not kept");
    assert!(logged[0].error.as_deref().is_some_and(|e| e.starts_with("invalid arguments for writeFile")));
    assert_eq!((logged[1].round, logged[4].round), (1, 2));
    assert_eq!(logged[3].error.as_deref(), Some("edit text not found"));
}

#[test]
fn a_denial_is_logged_as_one_with_the_users_reason() {
    let mut h = harness(
        "loop-log-denied",
        vec![asks(vec![wants("w1", "writeFile", r#"{"path":"a.rs","content":"x"}"#)]), text("ok")],
    );
    h.approval = asking();
    let ChatStreamOutcome::PendingApproval(pending) =
        h.run(|turn| stream(turn, vec![LlmMessage::user("write it")], vec![])).expect("pauses")
    else {
        panic!("expected a pause");
    };
    assert!(h.logged.lock().unwrap().is_empty(), "a paused call has not settled");

    h.run(|turn| {
        resume(turn, pending, vec![ToolCallDecision { id: "w1".into(), approved: false, reason: Some("not now".into()) }])
    })
    .expect("finishes");

    let logged = h.logged.lock().unwrap().clone();
    assert_eq!(logged.len(), 1);
    assert_eq!(logged[0].status, CallStatus::Denied);
    assert_eq!(logged[0].error.as_deref(), Some("Denied by the user: not now"));
    assert!(logged[0].result.is_none());
}

/// Pausing must not be a way to buy more budget: the resumed pass charges
/// the round again, exactly as `round` itself is counted twice.
#[test]
fn a_paused_round_is_charged_on_both_passes() {
    let write = |id: &str| wants(id, "writeFile", r#"{"path":"a.rs","content":"x"}"#);
    let mut h = harness(
        "loop-budget",
        vec![asks(vec![write("w1")]), asks(vec![write("w2")])],
    );
    h.approval = asking();
    let weight = ToolName::WriteFile.loop_weight();

    let ChatStreamOutcome::PendingApproval(first) = h
        .run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]))
        .expect("pauses")
    else {
        panic!("expected a pause");
    };
    assert_eq!(first.budget_used, weight);

    let approve = |id: &str| ToolCallDecision {
        id: id.to_string(),
        approved: true,
        reason: None,
    };
    let ChatStreamOutcome::PendingApproval(second) = h
        .run(|turn| resume(turn, first, vec![approve("w1")]))
        .expect("pauses again")
    else {
        panic!("expected a second pause");
    };

    assert_eq!(
        second.budget_used,
        weight * 3,
        "the resumed round is charged again, and then the new round"
    );
    assert_eq!(second.round, 3, "and the round counter moves the same way");
}

// ---------------------------------------------------------------- resume

#[test]
fn a_resume_missing_a_decision_is_refused() {
    let h = harness("loop-resume-missing", vec![]);
    let pending = PendingApproval {
        history: vec![LlmMessage {
            role: LlmRole::Assistant,
            content: None,
            tool_call_id: None,
            tool_calls: vec![wants("w1", "writeFile", "{}")],
            native_content: None,
            images: Vec::new(),
        }],
        round: 1,
        budget_used: 2,
        event_seq: 4,
        calls: vec![PendingToolCall {
            id: "w1".to_string(),
            name: "writeFile".to_string(),
            arguments: "{}".to_string(),
            requires_confirmation: true,
            reason: None,
        }],
        todos: vec![],
        reads: ReadFiles::default(),
    };

    let err = h.run(|turn| resume(turn, pending, vec![])).expect_err("refused");
    assert!(matches!(err, TurnError::Decision(_)), "{err}");
}

/// Tool results with no request in front of them are rejected by the
/// provider, far from the point where the mismatch could be explained.
#[test]
fn a_resume_whose_history_lost_the_tool_call_round_is_refused() {
    let h = harness("loop-resume-history", vec![]);
    let pending = PendingApproval {
        history: vec![LlmMessage::user("go")],
        round: 1,
        budget_used: 0,
        event_seq: 0,
        calls: vec![],
        todos: vec![],
        reads: ReadFiles::default(),
    };

    let err = h.run(|turn| resume(turn, pending, vec![])).expect_err("refused");
    assert!(matches!(err, TurnError::BadResume(_)), "{err}");

    // Only results may follow the round: anything else means it is over.
    let round = LlmMessage {
        role: LlmRole::Assistant,
        content: None,
        tool_call_id: None,
        tool_calls: vec![wants("w1", "writeFile", "{}")],
        native_content: None,
        images: Vec::new(),
    };
    let pending = PendingApproval {
        history: vec![round, LlmMessage::user("go")],
        round: 1,
        budget_used: 0,
        event_seq: 0,
        calls: vec![],
        todos: vec![],
        reads: ReadFiles::default(),
    };
    let err = h.run(|turn| resume(turn, pending, vec![])).expect_err("refused");
    assert!(matches!(err, TurnError::BadResume(_)), "{err}");
}

/// One turn, one stream of numbers: a listener that reconnects after the
/// pause cannot order anything if resuming starts again from zero.
#[test]
fn the_event_stream_continues_across_a_pause() {
    let mut h = harness(
        "loop-seq",
        vec![
            asks(vec![wants("w1", "writeFile", r#"{"path":"a.rs","content":"x"}"#)]),
            text("ok"),
        ],
    );
    h.approval = asking();

    let paused = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]));
    let ChatStreamOutcome::PendingApproval(pending) = paused.expect("pauses") else {
        panic!("expected a pause");
    };
    let before = h.events().last().expect("events").seq;
    assert_eq!(pending.event_seq, before);

    let decisions = vec![ToolCallDecision {
        id: "w1".to_string(),
        approved: true,
        reason: None,
    }];
    h.run(|turn| resume(turn, pending, decisions)).expect("finishes");

    let seqs: Vec<u64> = h.events().iter().map(|e| e.seq).collect();
    assert_eq!(seqs, (1..=seqs.len() as u64).collect::<Vec<_>>());
}
