use super::*;
use crate::domain::turn_journal::{JournalEntry, TurnJournal};

/// The turn's journal in a test: every entry kept, the count the adapter
/// would hold — the shape `ChatJournal` has in the app, without the disk.
#[derive(Default)]
struct Journal {
    entries: Mutex<Vec<JournalEntry>>,
    len: Mutex<usize>,
}

impl TurnJournal for Journal {
    fn len(&self) -> usize {
        *self.len.lock().unwrap()
    }
    fn append(&self, messages: &[LlmMessage]) {
        self.entries.lock().unwrap().push(JournalEntry::Append(messages.to_vec()));
        *self.len.lock().unwrap() += messages.len();
    }
    fn snapshot(&self, history: &[LlmMessage]) {
        self.entries.lock().unwrap().push(JournalEntry::Snapshot(history.to_vec()));
        *self.len.lock().unwrap() = history.len();
    }
}

fn with_journal(h: &mut Harness) -> Arc<Journal> {
    let journal = Arc::new(Journal::default());
    h.journal = Some(journal.clone());
    journal
}

/// TJ-4: the base is the history the turn was sent with; each completed
/// round appends what it added. The final answer is not journaled — the
/// turn is over, and its save folds the journal away.
#[test]
fn the_journal_keeps_the_base_and_each_completed_round() {
    let mut h = harness(
        "loop-journal",
        vec![
            asks(vec![wants("c1", "todo", r#"{"op":"write","tasks":["a"]}"#)]),
            text("done"),
        ],
    );
    let journal = with_journal(&mut h);

    let ChatStreamOutcome::Done(done) = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes") else {
        panic!("expected Done");
    };

    let entries = journal.entries.lock().unwrap();
    assert!(matches!(&entries[0], JournalEntry::Snapshot(base) if base == &vec![LlmMessage::user("go")]));
    assert!(matches!(&entries[1], JournalEntry::Append(_)));
    assert_eq!(entries.len(), 2, "the final answer is the save's, not the journal's");
    // Replayed, the journal is the history as of the last completed round —
    // exactly what the turn hands back minus its closing answer.
    let recovered = crate::domain::turn_journal::recover(&[], &serde_json::Value::Null, &[], None, &entries).unwrap();
    assert_eq!(recovered.messages, done.history[..done.history.len() - 1]);
}

/// A fold rewrote the history: the journal takes a snapshot, and the replay
/// is the folded history, not the base with the old tail.
#[test]
fn a_fold_puts_a_whole_snapshot_in_the_journal() {
    let mut h = harness(
        "loop-journal-fold",
        vec![Step::Fail(too_long_error()), text("done")],
    );
    h.session.context_limit = Some(WINDOW_FOR_LONG);
    let journal = with_journal(&mut h);

    let ChatStreamOutcome::Done(done) = h.run(|turn| stream(turn, long_conversation(), vec![])).expect("finishes") else {
        panic!("expected Done");
    };

    let entries = journal.entries.lock().unwrap();
    assert!(matches!(&entries[0], JournalEntry::Snapshot(_)));
    assert!(matches!(&entries[1], JournalEntry::Snapshot(_)), "the fold is a snapshot, not a tail");
    let recovered = crate::domain::turn_journal::recover(&[], &serde_json::Value::Null, &[], None, &entries).unwrap();
    assert_eq!(recovered.messages, done.history[..done.history.len() - 1]);
    assert!(recovered.messages.len() < long_conversation().len(), "the fold shortened it");
}

/// A pause journals nothing of its own round — the calls have not run. The
/// resume appends the round whole, counted from where the pause left the
/// journal, and the replay has no duplicate and no hole.
#[test]
fn a_resume_appends_the_paused_round_whole() {
    let mut h = harness(
        "loop-journal-resume",
        vec![
            asks(vec![wants("w1", "writeFile", r#"{"path":"a.rs","content":"x"}"#)]),
            text("done"),
        ],
    );
    h.approval = ApprovalPolicy::default();
    let journal = with_journal(&mut h);

    let paused = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]));
    let ChatStreamOutcome::PendingApproval(pending) = paused.expect("pauses") else {
        panic!("expected a pause");
    };
    assert_eq!(journal.entries.lock().unwrap().len(), 1, "the paused round is not journaled");

    let ChatStreamOutcome::Done(done) = h
        .run(|turn| resume(turn, pending, vec![ToolCallDecision { id: "w1".to_string(), approved: true, reason: None }]))
        .expect("finishes")
    else {
        panic!("expected Done");
    };

    let entries = journal.entries.lock().unwrap();
    assert_eq!(entries.len(), 2);
    let JournalEntry::Append(added) = &entries[1] else { panic!("expected an append") };
    assert!(added.iter().any(|m| m.role == LlmRole::Assistant && !m.tool_calls.is_empty()), "the round's request");
    assert!(added.iter().any(|m| m.tool_call_id.as_deref() == Some("w1")), "and its result");
    let recovered = crate::domain::turn_journal::recover(&[], &serde_json::Value::Null, &[], None, &entries).unwrap();
    assert_eq!(recovered.messages, done.history[..done.history.len() - 1]);
}

/// A stop between two calls of one round leaves requests unanswered: the
/// journal keeps the last whole round, not the dangling ones — a recovery
/// that replayed them would poison the next message.
#[test]
fn a_stop_mid_round_journals_no_dangling_call() {
    let mut h = harness(
        "loop-journal-cancel",
        vec![asks(vec![
            wants("w1", "writeFile", r#"{"path":"a.rs","content":"x"}"#),
            wants("w2", "writeFile", r#"{"path":"b.rs","content":"y"}"#),
        ])],
    );
    // Poll 4 is run_calls' first check, before either call runs.
    h.cancel_at_poll(4);
    let journal = with_journal(&mut h);

    let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]));
    assert!(matches!(outcome, Ok(ChatStreamOutcome::Cancelled(_))), "{outcome:?}");

    let entries = journal.entries.lock().unwrap();
    assert_eq!(entries.len(), 1, "only the base: the stopped round is not journaled");
    let recovered = crate::domain::turn_journal::recover(&[], &serde_json::Value::Null, &[], None, &entries).unwrap();
    assert_eq!(recovered.messages, vec![LlmMessage::user("go")]);
}

/// Old results cleared in place: the journal takes a whole snapshot, so the
/// replay carries the stubs, not the texts the model no longer has.
#[test]
fn cleared_results_are_snapshotted_not_appended() {
    let huge = "x".repeat(100_000);
    let round = |id: &str, content: &str| {
        vec![
            LlmMessage {
                role: LlmRole::Assistant,
                content: None,
                tool_call_id: None,
                tool_calls: vec![crate::domain::llm::LlmToolCall { id: id.into(), name: "readFile".into(), arguments: r#"{"path":"a.rs"}"#.into() }],
                native_content: None,
                images: Vec::new(),
            },
            LlmMessage { role: LlmRole::Tool, content: Some(content.into()), tool_call_id: Some(id.into()), tool_calls: vec![], native_content: None, images: Vec::new() },
        ]
    };
    let mut history = vec![LlmMessage::user("go")];
    history.extend(round("r1", &huge));
    history.extend(round("r2", "small"));
    history.extend(round("r3", "small"));
    history.extend(round("r4", "small"));
    history.push(LlmMessage::user("again"));

    let mut h = harness("loop-journal-clear", vec![text("done")]);
    h.session.context_limit = Some(25_000);
    let journal = with_journal(&mut h);

    let ChatStreamOutcome::Done(done) = h.run(|turn| stream(turn, history, vec![])).expect("finishes") else {
        panic!("expected Done");
    };

    let entries = journal.entries.lock().unwrap();
    assert_eq!(entries.len(), 2);
    assert!(matches!(&entries[0], JournalEntry::Snapshot(_)));
    assert!(matches!(&entries[1], JournalEntry::Snapshot(_)), "clearing rewrites in place: a snapshot, not a tail");
    let recovered = crate::domain::turn_journal::recover(&[], &serde_json::Value::Null, &[], None, &entries).unwrap();
    assert_eq!(recovered.messages, done.history[..done.history.len() - 1]);
    assert!(
        recovered.messages.iter().any(|m| m.content.as_deref().is_some_and(|c| c.starts_with("[Result cleared to save context:"))),
        "the cleared result is a stub in the replay"
    );
}

/// The crash this exists for: the turn dies mid-flight, and the journal
/// replays to exactly the history the failure hands back (TJ-2) — the
/// work is kept whichever way the turn ended.
#[test]
fn a_failed_turns_journal_replays_to_its_recovered_history() {
    let mut h = harness(
        "loop-journal-fail",
        vec![
            asks(vec![wants("c1", "todo", r#"{"op":"write","tasks":["a"]}"#)]),
            Step::Fail(LlmError::Provider("peer closed connection".to_string())),
        ],
    );
    let journal = with_journal(&mut h);

    let err = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect_err("round 2 fails");
    let TurnError::Failed { history, .. } = err else { panic!("expected Failed: {err}") };

    let entries = journal.entries.lock().unwrap();
    let recovered = crate::domain::turn_journal::recover(&[], &serde_json::Value::Null, &[], None, &entries).unwrap();
    assert_eq!(recovered.messages, history);
}
