use super::*;

// ------------------------------------------------------------- steering

pub(super) fn user_messages(request: &ChatRequest) -> Vec<String> {
    request
        .messages
        .iter()
        .filter(|m| m.role == LlmRole::User)
        .filter_map(|m| m.content.clone())
        .collect()
}

/// The point of steering: what the user typed mid-turn reaches the model
/// on the next round, marked as a clarification rather than a new task.
#[test]
fn a_note_typed_mid_turn_reaches_the_next_round() {
    let h = harness(
        "steer-applied",
        vec![
            asks_while_typing(
                vec![wants("l1", "listFiles", "{}")],
                "use the existing helper",
            ),
            text("understood"),
        ],
    );

    h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");

    let second = user_messages(&h.provider.requests()[1]);
    assert!(
        second.iter().any(|m| m.contains("use the existing helper")),
        "{second:?}"
    );
    assert!(
        second.iter().any(|m| m.starts_with(STEERING_PREFIX)),
        "a clarification, not a new task: {second:?}"
    );
    let applied: Vec<String> = payloads(&h.events())
        .into_iter()
        .filter(|p| p.starts_with("steering:"))
        .collect();
    assert_eq!(applied.len(), 1, "the note is announced by id: {applied:?}");
}

/// Ending the turn here would silently drop what the user typed, with
/// nothing to tell them it was never seen.
#[test]
fn a_note_that_arrives_as_the_model_finishes_keeps_the_turn_going() {
    let h = harness(
        "steer-late",
        vec![
            text_while_typing("all done", "and rename it too"),
            text("also did the other thing"),
        ],
    );

    let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]));

    let ChatStreamOutcome::Done(done) = outcome.expect("finishes") else {
        panic!("expected Done");
    };
    assert_eq!(done.result.text, "also did the other thing");
    assert_eq!(h.provider.requests().len(), 2, "the turn ran another round");
    let second = user_messages(&h.provider.requests()[1]);
    assert!(second.iter().any(|m| m.contains("and rename it too")), "{second:?}");
    // The answer the model had already given stays in the conversation,
    // or the extra round reads as if it never spoke.
    assert!(h.provider.requests()[1]
        .messages
        .iter()
        .any(|m| m.role == LlmRole::Assistant && m.content.as_deref() == Some("all done")));
}

/// A note is handed over once. A round that is retried, or a turn that
/// pauses in between, must not deliver it a second time.
#[test]
fn a_note_is_delivered_once() {
    let h = harness(
        "steer-once",
        vec![
            asks_while_typing(vec![wants("l1", "listFiles", "{}")], "watch the indentation"),
            asks(vec![wants("l2", "listFiles", "{}")]),
            text("done"),
        ],
    );

    h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");

    let delivered = user_messages(h.provider.requests().last().unwrap())
        .iter()
        .filter(|m| m.contains("watch the indentation"))
        .count();
    assert_eq!(delivered, 1);
}

/// The user typed it at a conversation that had already finished. It is
/// their next message, not a clarification of a turn they cannot see.
#[test]
fn a_note_left_over_from_a_finished_turn_does_not_leak_into_the_next() {
    let h = harness("steer-leftover", vec![text("hi")]);
    h.steering.push(SteeringNote::user("from the previous turn"));

    h.run(|turn| stream(turn, vec![LlmMessage::user("a new question")], vec![]))
        .expect("finishes");

    let sent = user_messages(&h.provider.requests()[0]);
    assert_eq!(sent, ["a new question"]);
}

/// A `/` command typed mid-turn: the model gets its prompt, and the
/// transcript the command as typed.
#[test]
fn a_note_shown_as_a_command_tells_the_model_its_prompt() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let seen = log.clone();
    let sink: ChatEventSink = Arc::new(move |event| seen.lock().unwrap().push(event));
    let events = Events::new(&sink, 0);
    let mut history = Vec::new();
    let note = SteeringNote { shown: Some("/review a.rs".to_string()), ..SteeringNote::user("Review a.rs line by line") };

    apply_steering(&events, 1, &mut history, vec![note, SteeringNote::user("and b.rs")]);

    assert_eq!(
        history.iter().map(|m| m.content.clone().unwrap_or_default()).collect::<Vec<_>>(),
        [format!("{STEERING_PREFIX}Review a.rs line by line"), format!("{STEERING_PREFIX}and b.rs")]
    );
    let shown: Vec<String> = log
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match &e.event {
            ChatEventPayload::SteeringApplied { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(shown, ["/review a.rs", "and b.rs"]);
}

/// A screenshot attached to the note is part of the note: the model is
/// sent it with the clarification, and the bubble is told to draw it.
#[test]
fn a_note_with_a_picture_sends_it_to_the_model() {
    use crate::domain::image::{ImageMediaType, ImagePart};

    let picture = ImagePart {
        media_type: ImageMediaType::Jpeg,
        data: "AA".into(),
        width: 30,
        height: 20,
    };
    let log = Arc::new(Mutex::new(Vec::new()));
    let seen = log.clone();
    let sink: ChatEventSink = Arc::new(move |event| seen.lock().unwrap().push(event));
    let events = Events::new(&sink, 0);
    let mut history = Vec::new();
    let note = SteeringNote { images: vec![picture.clone()], ..SteeringNote::user("why is it red?") };

    apply_steering(&events, 2, &mut history, vec![note]);

    assert_eq!(history.len(), 1, "one message, with both");
    assert_eq!(history[0].images, vec![picture.clone()]);
    assert_eq!(
        history[0].content.as_deref().unwrap_or_default(),
        format!("{STEERING_PREFIX}why is it red?")
    );
    let applied: Vec<(String, Vec<ImagePart>)> = log
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match &e.event {
            ChatEventPayload::SteeringApplied { text, images, .. } => Some((text.clone(), images.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(applied, [("why is it red?".to_string(), vec![picture])]);
}

/// The prefix alone ends in a colon and nothing: a note that is only a
/// picture says what to do with it.
#[test]
fn a_note_that_is_only_a_picture_says_what_it_is() {
    use crate::domain::image::{ImageMediaType, ImagePart};

    let picture = |n: u32| ImagePart {
        media_type: ImageMediaType::Png,
        data: "AA".into(),
        width: n,
        height: n,
    };
    let only_one = SteeringNote { images: vec![picture(1)], ..SteeringNote::user("  ") };
    let two = SteeringNote { images: vec![picture(1), picture(2)], ..SteeringNote::user("") };

    assert_eq!(only_one.prefixed(), format!("{STEERING_PREFIX}look at the picture attached"));
    assert_eq!(two.prefixed(), format!("{STEERING_PREFIX}look at the pictures attached"));
}

/// A note with neither words nor pictures says nothing at all: no message
/// made of the prefix and a dangling colon, and no bubble to draw.
#[test]
fn a_note_with_nothing_in_it_is_not_sent() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let seen = log.clone();
    let sink: ChatEventSink = Arc::new(move |event| seen.lock().unwrap().push(event));
    let events = Events::new(&sink, 0);
    let mut history = Vec::new();

    apply_steering(&events, 1, &mut history, vec![SteeringNote::user("  ")]);

    assert!(history.is_empty(), "{history:?}");
    assert!(log.lock().unwrap().is_empty(), "nothing to announce either");
}

/// The queue between the command and the round is part of the way: a note
/// with a picture, typed mid-turn, reaches the model with it — not only
/// when `apply_steering` is handed the note directly.
#[test]
fn a_note_with_a_picture_reaches_the_model_with_it() {
    use crate::domain::image::{ImageMediaType, ImagePart};

    let picture = ImagePart {
        media_type: ImageMediaType::Png,
        data: "AA".into(),
        width: 8,
        height: 6,
    };
    let h = harness(
        "steer-picture",
        vec![
            Step::ReplyWhileTheUserTypes(
                ChatStreamResult { text: "one".into(), ..Default::default() },
                SteeringNote { images: vec![picture.clone()], ..SteeringNote::user("look here") },
            ),
            text("two"),
        ],
    );

    h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");

    let second = &h.provider.requests()[1];
    let note = second
        .messages
        .iter()
        .find(|m| m.role == LlmRole::User && m.content.as_deref().unwrap_or_default().starts_with(STEERING_PREFIX))
        .expect("the note is sent to the model");
    assert_eq!(note.images, vec![picture]);
}

/// A call's arguments reach the window once each: what is new since the
/// last chunk, never the whole again, and nothing for a call that did not
/// grow. A name that comes after the first chunk still gets through.
#[test]
fn streamed_call_arguments_are_reported_as_what_is_new() {
    let chunks = vec![
        vec![("a", "read", "")],
        vec![("a", "read", r#"{"pa"#)],
        vec![("a", "read", r#"{"path":"x"}"#), ("b", "grep", "")],
        vec![("a", "read", r#"{"path":"x"}"#), ("b", "grep", "{}")],
        vec![("c", "", "{")],
        vec![("c", "list", "{")],
        vec![("c", "list", "{")],
    ];
    let h = harness("call-deltas", vec![Step::ReplyStreamingCalls(chunks, ChatStreamResult { text: "ok".into(), ..Default::default() })]);

    h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");

    let reported: Vec<(String, String, String)> = h
        .events()
        .into_iter()
        .filter_map(|e| match e.event {
            ChatEventPayload::ToolCallDelta(call) => Some((call.id, call.name, call.arguments)),
            _ => None,
        })
        .collect();
    let expected = [
        ("a", "read", ""),
        ("a", "read", r#"{"pa"#),
        ("a", "read", r#"th":"x"}"#),
        ("b", "grep", ""),
        ("b", "grep", "{}"),
        ("c", "", "{"),
        ("c", "list", ""),
    ];
    assert_eq!(reported, expected.map(|(i, n, a)| (i.to_string(), n.to_string(), a.to_string())));
}

/// Thinking reaches the window as thinking, under its own target, and the
/// answer as text — the two are drawn apart.
#[test]
fn reasoning_and_text_stream_as_their_own_kinds() {
    let h = harness("reasoning", vec![Step::ReplyThinking("weighing it", ChatStreamResult { text: "done".into(), ..Default::default() })]);
    h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");
    let streamed: Vec<(String, String)> = h
        .events()
        .into_iter()
        .filter_map(|e| match e.event {
            ChatEventPayload::Reasoning { delta } => Some((e.target_id.unwrap_or_default(), format!("reasoning:{delta}"))),
            ChatEventPayload::Delta { delta } => Some((e.target_id.unwrap_or_default(), format!("text:{delta}"))),
            _ => None,
        })
        .collect();
    assert_eq!(
        streamed,
        [
            ("round:1:reasoning".to_string(), "reasoning:weighing it".to_string()),
            ("round:1:text".to_string(), "text:done".to_string()),
        ]
    );
}

#[test]
fn a_queued_note_can_be_cancelled_until_a_round_takes_it() {
    let queue = SteeringQueue::default();
    let note = SteeringNote::user("never mind");
    let id = note.id.clone();
    queue.push(note);

    assert!(queue.cancel(&id), "still queued");
    assert!(queue.take().is_empty());
    // Cancelling what a round already picked up answers `false`: what has
    // been said to the model cannot be unsaid.
    assert!(!queue.cancel(&id));
}

/// Two identical clarifications are indistinguishable by text, which is
/// why cancelling works by id.
#[test]
fn identical_notes_are_still_separate() {
    let queue = SteeringQueue::default();
    let first = SteeringNote::user("check the locale");
    let second = SteeringNote::user("check the locale");
    assert_ne!(first.id, second.id);
    let first_id = first.id.clone();
    queue.push(first);
    queue.push(second);

    assert!(queue.cancel(&first_id));
    assert_eq!(queue.take().len(), 1);
}
