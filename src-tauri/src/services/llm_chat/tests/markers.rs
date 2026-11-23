use super::*;

/// The chat's markers in a test: every record kept, the last one's
/// fingerprint the next request compares with — the shape `ChatMarkers`
/// has in the app, without the disk.
#[derive(Default)]
struct Markers(Mutex<Vec<RequestRecord>>);

impl RequestMarkers for Markers {
    fn previous(&self) -> Option<request_marker::RequestFingerprint> {
        self.0.lock().unwrap().last().map(|r| r.fingerprint.clone())
    }
    fn record(&self, record: RequestRecord) {
        self.0.lock().unwrap().push(record);
    }
    fn last_anchor(&self) -> Option<request_marker::Anchor> {
        self.0.lock().unwrap().iter().rev().find_map(request_marker::Anchor::of)
    }
}

fn usage(prompt: u32, cached: u32, completion: u32) -> ChatUsage {
    ChatUsage { prompt_tokens: prompt, completion_tokens: completion, total_tokens: prompt + completion, cached_tokens: cached }
}

fn with_markers(h: &mut Harness) -> Arc<Markers> {
    let markers = Arc::new(Markers::default());
    h.markers = Some(markers.clone());
    markers
}

/// TJ-3: every request leaves a marker — hashes and numbers, never content
/// — and the usage event carries what changed since the chat's previous
/// request. The first has nothing to compare with; the second, a history
/// grown by the round's own calls and results, is the provider's to keep.
#[test]
fn each_request_leaves_a_marker_and_the_usage_event_names_the_cause() {
    let mut h = harness(
        "loop-markers",
        vec![
            Step::Reply(ChatStreamResult {
                tool_calls: vec![wants("c1", "todo", r#"{"op":"write","tasks":["a"]}"#)],
                usage: Some(usage(1000, 900, 10)),
                ..Default::default()
            }),
            Step::Reply(ChatStreamResult { text: "done".into(), usage: Some(usage(1200, 0, 5)), ..Default::default() }),
        ],
    );
    let markers = with_markers(&mut h);

    let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]));
    assert!(matches!(outcome, Ok(ChatStreamOutcome::Done(_))), "{outcome:?}");

    let rows = markers.0.lock().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].round, 1);
    assert_eq!(rows[1].round, 2);
    assert_eq!(rows[0].cause, None, "the first request compares with nothing");
    assert_eq!(rows[1].cause, Some(MissCause::Unchanged), "a grown history keeps the prefix");
    assert_eq!(rows[1].usage, Some(usage(1200, 0, 5)));
    assert_eq!(rows[0].provider_id, "test");
    assert_eq!(rows[0].model, "m");
    // Hashes only: 64 hex characters, no message text anywhere in the row.
    assert!(rows[0].fingerprint.messages.iter().all(|m| m.len() == 64 && m.chars().all(|c| c.is_ascii_hexdigit())));
    assert_eq!(rows[0].fingerprint.messages.len(), 1, "the user's message");
    assert!(rows[1].fingerprint.messages.len() > 1, "the round's calls and results joined");

    let causes: Vec<Option<MissCause>> = h
        .events()
        .iter()
        .filter_map(|e| match &e.event {
            ChatEventPayload::ContextUsage { cause, .. } => Some(*cause),
            _ => None,
        })
        .collect();
    assert_eq!(causes, vec![None, Some(MissCause::Unchanged)]);
}

/// The measured case: `toolSearch` brings a tool into the set, the set goes
/// at the front of the request, and the next request's miss is the tool
/// set's — not a mystery.
#[test]
fn a_found_tool_changes_the_tool_set_and_the_marker_says_so() {
    let mut h = harness(
        "loop-markers-tools",
        vec![
            Step::Reply(ChatStreamResult {
                tool_calls: vec![wants("s1", "toolSearch", r#"{"query":"move a file"}"#)],
                usage: Some(usage(1000, 900, 10)),
                ..Default::default()
            }),
            Step::Reply(ChatStreamResult { text: "done".into(), usage: Some(usage(1200, 0, 5)), ..Default::default() }),
        ],
    );
    let markers = with_markers(&mut h);

    let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]));
    assert!(matches!(outcome, Ok(ChatStreamOutcome::Done(_))), "{outcome:?}");

    let rows = markers.0.lock().unwrap();
    assert_eq!(rows.len(), 2);
    assert_ne!(rows[0].fingerprint.tools, rows[1].fingerprint.tools, "the found tool joined the set");
    assert_eq!(rows[1].cause, Some(MissCause::Tools));
}

/// A refusal that folds the history: two requests of one round, two rows —
/// the second compares with the first and names the fold as the change.
#[test]
fn a_fold_between_attempts_is_the_second_rows_cause() {
    let mut h = harness(
        "loop-markers-fold",
        vec![
            Step::Fail(too_long_error()),
            Step::Reply(ChatStreamResult { text: "done".into(), usage: Some(usage(1200, 0, 5)), ..Default::default() }),
        ],
    );
    h.session.context_limit = Some(WINDOW_FOR_LONG);
    let markers = with_markers(&mut h);

    let outcome = h.run(|turn| stream(turn, long_conversation(), vec![]));
    assert!(matches!(outcome, Ok(ChatStreamOutcome::Done(_))), "{outcome:?}");

    let rows = markers.0.lock().unwrap();
    assert_eq!(rows.len(), 2, "the refused request and the folded one");
    assert_eq!(rows[0].round, 1);
    assert_eq!(rows[1].round, 1);
    assert_eq!(rows[0].usage, None);
    assert!(matches!(rows[1].cause, Some(MissCause::History { .. })), "{:?}", rows[1].cause);
    assert!(rows[1].fingerprint.messages.len() < rows[0].fingerprint.messages.len(), "the fold shortened the history");
}

/// CU-3: the meter during a turn is anchored: the second round's estimate is
/// the first round's real count plus the estimate of what the round added,
/// not a fresh estimate of the whole conversation.
#[test]
fn the_second_rounds_estimate_is_anchored_on_the_firsts_usage() {
    let mut h = harness(
        "loop-anchored-estimate",
        vec![
            Step::Reply(ChatStreamResult {
                tool_calls: vec![wants("c1", "todo", r#"{"op":"write","tasks":["a"]}"#)],
                usage: Some(usage(1000, 900, 10)),
                ..Default::default()
            }),
            Step::Reply(ChatStreamResult { text: "done".into(), usage: Some(usage(1200, 0, 5)), ..Default::default() }),
        ],
    );
    let _markers = with_markers(&mut h);

    let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]));
    let Ok(ChatStreamOutcome::Done(done)) = outcome else { panic!("{outcome:?}") };
    let estimates: Vec<usize> = h
        .events()
        .iter()
        .filter_map(|e| match &e.event {
            ChatEventPayload::ContextEstimate(u) => Some(u.total),
            _ => None,
        })
        .collect();
    assert_eq!(estimates.len(), 2);
    // The second round asked with the first round's calls and results joined;
    // the final assistant text is the round's own answer, not yet in history.
    let round_two_history = &done.history[..done.history.len() - 1];
    assert_eq!(estimates[1], 1000 + crate::domain::compaction::estimate_tokens(&round_two_history[1..]));
}

/// A failed request is a request too: its marker is written with no usage,
/// so a refusal can be found in the log by the turn that met it.
#[test]
fn a_failed_request_leaves_a_marker_too() {
    let mut h = harness(
        "loop-markers-fail",
        vec![Step::Fail(LlmError::Provider("peer closed connection".to_string()))],
    );
    let markers = with_markers(&mut h);

    let err = h
        .run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]))
        .expect_err("the request fails");
    assert!(matches!(err, TurnError::Failed { .. }), "{err}");

    let rows = markers.0.lock().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].usage, None);
    assert_eq!(rows[0].cause, None);
}
