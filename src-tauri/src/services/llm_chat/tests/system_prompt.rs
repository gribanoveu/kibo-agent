use super::*;

// ----------------------------------------------------- the system prompt

/// Everything the two sides actually said, with what the app told the
/// model stripped off the front.
pub(super) fn conversation_of(request: &ChatRequest) -> &[LlmMessage] {
    &request.messages[lead_of(request)..]
}

pub(super) fn lead_of(request: &ChatRequest) -> usize {
    let lead = request
        .messages
        .iter()
        .take_while(|m| m.role == LlmRole::System)
        .count();
    assert_eq!(lead, 3, "the instructions, the mode, then this turn's facts");
    lead
}

/// The last of the leading system messages: the half that changes.
pub(super) fn facts_of(request: &ChatRequest) -> String {
    request.messages[lead_of(request) - 1]
        .content
        .clone()
        .expect("the facts are a message with content")
}

/// The model is told who it is and where it stands before it is asked
/// anything. Without this the agent goes into a repository with no
/// identity, no rules about tools, and no idea which folder is open.
#[test]
fn the_request_opens_with_the_prompt_and_then_the_conversation() {
    let h = harness("prompt-front", vec![text("done")]);

    h.run(|turn| stream(turn, vec![LlmMessage::user("hi")], vec![]))
        .expect("finishes");

    let requests = h.provider.requests();
    assert_eq!(
        requests[0].messages[0].content.as_deref(),
        Some(prompt::INSTRUCTIONS)
    );
    // As the scope has it, canonical: on Windows a temp folder's short
    // name (`RUNNER~1`) is not part of its long one.
    let root = crate::infra::paths::canonicalize_plain(&h.root).unwrap();
    assert!(
        facts_of(&requests[0]).contains(&root.display().to_string()),
        "the open folder is not in the prompt"
    );
    assert_eq!(conversation_of(&requests[0]).len(), 1);
}

/// A round that thought goes back as the provider sent it: the next
/// request, which answers its calls, is refused if the signed blocks are
/// missing or rebuilt.
#[test]
fn a_rounds_native_content_is_carried_into_the_next_request() {
    let native = serde_json::json!([{"type": "thinking", "thinking": "hm", "signature": "s"}]);
    let h = harness(
        "native-content",
        vec![
            Step::Reply(ChatStreamResult {
                tool_calls: vec![wants("t1", "listFiles", "{}")],
                native_content: Some(native.clone()),
                ..Default::default()
            }),
            text("done"),
        ],
    );

    h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]))
        .expect("finishes");

    let second = &h.provider.requests()[1];
    let asked = conversation_of(second)
        .iter()
        .find(|m| !m.tool_calls.is_empty())
        .expect("the round that called");
    assert_eq!(asked.native_content, Some(native));
}

/// The prompt is prepended at request time and belongs to no turn: a
/// folder and a checklist frozen into the stored conversation would be
/// saved to the chat file and resent, stale, for as long as it exists.
#[test]
fn the_prompt_never_enters_the_history_the_caller_keeps() {
    let mut h = harness(
        "prompt-not-history",
        vec![
            asks(vec![wants("w1", "writeFile", r#"{"path":"a.rs","content":"x"}"#)]),
            text("done"),
        ],
    );
    h.approval = asking();

    let ChatStreamOutcome::PendingApproval(pending) = h
        .run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]))
        .expect("pauses")
    else {
        panic!("expected a pause");
    };

    assert!(
        pending.history.iter().all(|m| m.role != LlmRole::System),
        "the prompt was stored with the conversation"
    );
}

/// The round after a `todo` call sees the list as it now is — in that
/// call's result, the last thing in the request — and not in the prompt,
/// where a list that changes would sit ahead of the history a cache reuses.
#[test]
fn the_checklist_follows_the_turn_in_the_todo_result() {
    let h = harness(
        "prompt-todo",
        vec![
            asks(vec![wants(
                "t1",
                "todo",
                r#"{"op":"write","tasks":["read it","rewrite it"]}"#,
            )]),
            text("done"),
        ],
    );

    h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]))
        .expect("finishes");

    let requests = h.provider.requests();
    let last = |request: &ChatRequest| request.messages.last().cloned().expect("a message");
    assert!(!facts_of(&requests[0]).contains("Checklist") && !facts_of(&requests[1]).contains("Checklist"));
    assert_eq!(last(&requests[0]), LlmMessage::user("go"), "a list nobody had written yet");
    let checklist = last(&requests[1]);
    assert_eq!(checklist.role, LlmRole::Tool, "nothing after the conversation");
    let text = checklist.content.expect("text");
    assert!(text.contains("read it") && text.contains("rewrite it"));
}

/// Approval is the harness's business: the prompt says nothing about it,
/// whether the turn asks the user or not.
#[test]
fn the_prompt_says_nothing_about_approval() {
    let unattended = harness("prompt-unattended", vec![text("done")]);
    unattended
        .run(|turn| stream(turn, vec![LlmMessage::user("hi")], vec![]))
        .expect("finishes");

    let mut attended = harness("prompt-attended", vec![text("done")]);
    attended.approval = asking();
    attended
        .run(|turn| stream(turn, vec![LlmMessage::user("hi")], vec![]))
        .expect("finishes");

    let speaks_of_approval = |h: &Harness| {
        h.provider.requests()[0].messages.iter().filter(|m| m.role == LlmRole::System).any(|m| {
            m.content.as_deref().is_some_and(|c| c.to_lowercase().contains("approv"))
        })
    };
    assert!(!speaks_of_approval(&unattended));
    assert!(!speaks_of_approval(&attended));
}

/// What the shell really is, as the command layer found it, is what the
/// model reads — not the bare path.
#[test]
fn the_shell_reaches_the_prompt_as_described() {
    let mut h = harness("prompt-shell", vec![text("done")]);
    h.shell_described = "/bin/sh — really bash 3.2.57 in sh mode".to_string();
    h.run(|turn| stream(turn, vec![LlmMessage::user("hi")], vec![])).expect("finishes");
    assert!(facts_of(&h.provider.requests()[0]).contains("Shell for commands: /bin/sh — really bash 3.2.57 in sh mode"));
}

/// Half the gate: a tool the mode does not offer is not in the request.
#[test]
fn a_narrower_mode_advertises_fewer_tools() {
    let mut analyst = harness("mode-advertised", vec![text("here is how it works")]);
    analyst.mode = ConversationMode::Analyst;

    analyst
        .run(|turn| stream(turn, vec![LlmMessage::user("how does it work?")], vec![]))
        .expect("finishes");

    let advertised: Vec<String> = analyst.provider.requests()[0]
        .tools
        .iter()
        .map(|t| t.name.clone())
        .collect();
    assert!(advertised.contains(&"readFile".to_string()));
    assert!(!advertised.contains(&"writeFile".to_string()));
    assert!(!advertised.contains(&"runCommand".to_string()));
}

/// The other half, and the one that matters: a model that used `writeFile`
/// earlier in the conversation calls it again from memory. Not advertising
/// it does not stop that — refusing it does, and the refusal has to reach
/// the model as a result it can act on rather than ending the turn.
#[test]
fn a_tool_the_mode_does_not_offer_is_refused_even_when_asked_for() {
    let mut analyst = harness(
        "mode-refused",
        vec![
            asks(vec![wants(
                "w1",
                "writeFile",
                r#"{"path":"a.rs","content":"new"}"#,
            )]),
            text("right — here is what I would change"),
        ],
    );
    analyst.mode = ConversationMode::Analyst;
    std::fs::write(analyst.root.join("a.rs"), "old").unwrap();

    let outcome = analyst
        .run(|turn| stream(turn, vec![LlmMessage::user("fix it")], vec![]))
        .expect("finishes rather than failing");

    let ChatStreamOutcome::Done(done) = outcome else {
        panic!("a refused tool must not pause or stop the turn");
    };
    assert_eq!(done.result.text, "right — here is what I would change");
    assert_eq!(
        std::fs::read_to_string(analyst.root.join("a.rs")).unwrap(),
        "old",
        "the file was written in a mode that cannot write"
    );

    let told = told_errors(&analyst);
    assert!(
        told.iter().any(|r| r.contains("not available in this conversation mode")),
        "the model was not told why: {told:?}"
    );
}

/// Every failure the model was handed back this turn.
pub(super) fn told_errors(h: &Harness) -> Vec<String> {
    h.events()
        .into_iter()
        .filter_map(|event| match event.event {
            ChatEventPayload::ToolResult(result) => result.error,
            _ => None,
        })
        .collect()
}
