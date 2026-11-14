use super::*;

// ------------------------------------------------------- what the model reads

/// A call refused before it runs still has to be reported and answered,
/// or the model is left waiting for a result that never comes.
#[test]
fn a_call_refused_by_the_preflight_is_reported_as_a_tool_error() {
    let h = harness(
        "loop-preflight",
        vec![
            asks(vec![wants(
                "w1",
                "writeFile",
                r#"{"path":"../outside.rs","content":"x"}"#,
            )]),
            text("understood"),
        ],
    );

    h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");

    let told = tool_contents(h.provider.requests().last().unwrap());
    assert_eq!(told.len(), 1);
    assert!(told[0].starts_with("Error:"), "{}", told[0]);
    let events = payloads(&h.events());
    assert!(events.contains(&"toolCall:w1".to_string()));
    assert!(events.contains(&"toolResult:w1".to_string()));
}

/// The preflight answers a refused call before the round pauses for the
/// rest, so the history then ends with that answer, not the request.
#[test]
fn a_round_with_a_refused_call_still_resumes_after_approval() {
    let mut h = harness(
        "loop-preflight-pause",
        vec![
            asks(vec![
                wants("w0", "writeFile", r#"{"path":"../outside.rs","content":"x"}"#),
                wants("w1", "writeFile", r#"{"path":"a.rs","content":"x"}"#),
            ]),
            text("done"),
        ],
    );
    h.approval = asking();
    let paused = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]));
    let ChatStreamOutcome::PendingApproval(pending) = paused.expect("pauses") else {
        panic!("expected a pause");
    };

    let approve = vec![ToolCallDecision { id: "w1".to_string(), approved: true, reason: None }];
    h.run(|turn| resume(turn, pending, approve)).expect("resumes");

    let told = tool_contents(h.provider.requests().last().unwrap());
    assert_eq!(told.len(), 2, "{told:?}");
    assert!(told[0].starts_with("Error:"), "{}", told[0]);
    assert!(h.root.join("a.rs").exists());
}

/// A listing goes to the model as a tree: a flat array of paths makes it
/// rebuild the directory structure from N separate strings.
#[test]
fn a_listing_reaches_the_model_as_a_tree() {
    let h = harness(
        "loop-listing",
        vec![asks(vec![wants("l1", "listFiles", "{}")]), text("seen")],
    );
    std::fs::create_dir(h.root.join("src")).unwrap();
    std::fs::write(h.root.join("src/main.rs"), "fn main() {}").unwrap();

    h.run(|turn| stream(turn, vec![LlmMessage::user("what is here")], vec![]))
        .expect("finishes");

    let told = tool_contents(h.provider.requests().last().unwrap());
    assert!(told[0].contains("main.rs"), "{}", told[0]);
    assert!(!told[0].contains("\"isDir\""), "raw JSON, not a tree: {}", told[0]);
}

/// Token usage is reported once per round, and it is the whole context —
/// every request resends the history.
#[test]
fn usage_is_reported_for_the_round_that_produced_it() {
    let h = harness(
        "loop-usage",
        vec![Step::Reply(ChatStreamResult {
            text: "done".to_string(),
            usage: Some(ChatUsage {
                prompt_tokens: 100,
                completion_tokens: 7,
                total_tokens: 107,
                cached_tokens: 90,
            }),
            ..Default::default()
        })],
    );

    h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");

    assert!(payloads(&h.events()).contains(&"contextUsage".to_string()));
}

/// The model is offered the tools this build actually has, every round.
/// "Remind me on Friday" needs the weekday as well as the date.
#[test]
fn today_names_the_weekday_then_the_date() {
    let now = Local::now();
    let day = today();
    assert!(day.starts_with(&now.format("%A, ").to_string()), "{day}");
    assert!(day.ends_with(&now.format(" %B %Y").to_string()) && !day.contains("  "), "{day}");
}

/// The board is offered in a folder too — once the user turns it on.
#[test]
fn the_agent_is_offered_the_board_only_while_the_feature_is_on() {
    let offered = |features: Features| {
        let mut h = harness("loop-board", vec![text("hi")]);
        h.features = features;
        h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");
        // Waiting for `toolSearch`, which says so.
        let tools = &h.provider.requests()[0].tools;
        tools.iter().any(|t| t.name == "board" || (t.name == "toolSearch" && t.description.contains("`board`")))
    };
    assert!(!offered(Features::default()));
    assert!(offered(Features::all()));
}

/// MongoDB's read waits for `toolSearch` in a folder — once the user turns it on.
#[test]
fn the_agent_is_offered_mongo_only_while_the_feature_is_on() {
    let offered = |features: Features| {
        let mut h = harness("loop-mongo", vec![text("hi")]);
        h.features = features;
        h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");
        let tools = &h.provider.requests()[0].tools;
        assert!(tools.iter().all(|t| t.name != "mongo"), "not in every request");
        tools.iter().any(|t| t.name == "toolSearch" && t.description.contains("`mongo`"))
    };
    assert!(!offered(Features::default()));
    assert!(offered(Features::all()));
}

#[test]
fn every_request_carries_the_tool_schemas() {
    let h = harness("loop-tools", vec![text("hi")]);

    h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");

    let requests = h.provider.requests();
    let offered: &[LlmToolDefinition] = &requests[0].tools;
    // Every built-in one but a review's own, the features' — the board,
    // DeepSeek's two, MongoDB's one — the Kubernetes role's and the
    // Prototype role's `present`; MCP tools come from servers, and none
    // is connected. Those waiting for `toolSearch` are found with it.
    let waiting = waiting_tools(ConversationMode::Agent, false, &Features::default()).len();
    assert_eq!(waiting, 3, "move, gitBlame, readTerminal");
    // And `toolSearch`, which is there for them.
    assert_eq!(offered.len(), ToolName::ALL.len() - 8 - ChatRole::Kubernetes.tools().len() - waiting + 1);
}

/// A write in the loop says what it did to the file, for a rewind; a
/// read says nothing.
#[test]
fn a_write_in_the_loop_reports_what_it_changed() {
    crate::testing::with_app_dir("chat-changes", || {
        let h = harness(
            "chat-changes",
            vec![
                asks(vec![wants("r", "readFile", r#"{"path":"a.rs"}"#)]),
                asks(vec![wants("w", "writeFile", r#"{"path":"a.rs","content":"new"}"#)]),
                text("done"),
            ],
        );
        std::fs::write(h.root.join("a.rs"), "old").unwrap();
        h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");

        let log = h.log.lock().unwrap();
        let changes = |id: &str| {
            log.iter()
                .find_map(|e| match &e.event {
                    ChatEventPayload::ToolResult(r) if r.id == id => Some(r.changes.clone()),
                    _ => None,
                })
                .unwrap()
        };
        assert!(changes("r").is_empty());
        let stored = |content: &[u8]| crate::domain::rewind::FileState::Stored { hash: crate::infra::file_history::hash(content) };
        assert_eq!(changes("w"), [FileChange { path: "a.rs".into(), before: stored(b"old"), after: stored(b"new") }]);
    });
}
