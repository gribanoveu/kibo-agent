use super::*;

// ------------------------------------------------------------------ MCP

/// A server that answers every call with its name and arguments.
pub(super) struct Echo;
impl crate::domain::mcp::McpClient for Echo {
    fn list_tools(&self) -> Result<Vec<crate::domain::mcp::McpTool>, crate::domain::mcp::McpError> {
        Ok(vec![])
    }
    fn call_tool(
        &self,
        name: &str,
        arguments: serde_json::Value,
        _: &dyn Fn() -> bool,
    ) -> Result<crate::domain::mcp::McpCallResult, crate::domain::mcp::McpError> {
        Ok(crate::domain::mcp::McpCallResult { text: format!("{name} got {arguments}"), is_error: false })
    }
}

pub(super) fn with_server(mut h: Harness, weight: u32) -> Harness {
    use crate::domain::mcp::{McpTool, McpToolHints};
    let tool = |name: &str, hints: McpToolHints| McpTool {
        name: name.into(),
        description: "Finds issues.".into(),
        input_schema: serde_json::json!({"type": "object"}),
        hints,
        ..Default::default()
    };
    h.mcp = McpTools::new(vec![crate::domain::mcp::ConnectedServer {
        name: "tracker".into(),
        config: crate::domain::mcp::McpServerConfig { weight: Some(weight), ..Default::default() },
        client: Arc::new(Echo),
        tools: vec![
            tool("find", McpToolHints::default()),
            tool("list", McpToolHints { read_only: true, destructive: false }),
            tool("purge", McpToolHints { read_only: false, destructive: true }),
        ],
        instructions: Some("Find before you purge.".into()),
    }]);
    h
}

/// What a server says about using its tools reaches the model, as the
/// server's own words.
#[test]
fn a_servers_instructions_are_in_the_prompt() {
    let h = with_server(harness("mcp-instructions", vec![text("hi")]), 3);
    h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");
    let sent = &h.provider.requests()[0].messages;
    let said = sent.iter().filter_map(|m| m.content.as_deref()).find(|c| c.contains("## MCP servers")).expect("said");
    assert!(said.contains("### tracker\n\nFind before you purge."), "{said}");
}

/// A tool its server calls destructive asks even under "Always allow",
/// and says why; Auto still means not asking.
#[test]
fn a_tool_its_server_calls_destructive_asks_despite_always_allow() {
    let script = || vec![asks(vec![wants("m1", "mcp__tracker__purge", "{}")]), text("done")];
    let mut h = with_server(harness("mcp-destructive", script()), 3);
    h.approval = asking();
    h.approval.allow_always("mcp__tracker__purge").unwrap();
    let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]));
    let ChatStreamOutcome::PendingApproval(pending) = outcome.expect("pauses") else { panic!("it ran") };
    assert!(pending.calls[0].reason.as_deref().unwrap().contains("destructive"), "{:?}", pending.calls[0].reason);

    let mut auto = with_server(harness("mcp-destructive-auto", script()), 3);
    auto.approval = asking();
    auto.approval.skip_all = true;
    let outcome = auto.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]));
    assert!(matches!(outcome, Ok(ChatStreamOutcome::Done(_))), "Auto asked");
}

/// "Only reads" is the server's word: it is put on the card and lifts
/// nothing — and a card that is not shown has nothing to say.
#[test]
fn a_tool_its_server_calls_read_only_still_asks_and_the_card_says_whose_word_it_is() {
    let script = || vec![asks(vec![wants("m1", "mcp__tracker__list", "{}")]), text("done")];
    let mut h = with_server(harness("mcp-read-only", script()), 3);
    h.approval = asking();
    let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]));
    let ChatStreamOutcome::PendingApproval(pending) = outcome.expect("pauses") else { panic!("it ran unasked") };
    assert!(pending.calls[0].requires_confirmation);
    assert!(pending.calls[0].reason.as_deref().unwrap().contains("not checked"), "{:?}", pending.calls[0].reason);

    let mut plain = with_server(harness("mcp-no-hint", vec![asks(vec![wants("m1", "mcp__tracker__find", "{}")])]), 3);
    plain.approval = asking();
    let outcome = plain.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]));
    let ChatStreamOutcome::PendingApproval(pending) = outcome.expect("pauses") else { panic!("it ran unasked") };
    assert_eq!(pending.calls[0].reason, None, "a tool its server said nothing about has no note");
}

/// The same server, deferred but for `find`, and `purge` hidden.
pub(super) fn with_deferred(h: Harness) -> Harness {
    use crate::domain::mcp::Exposure;
    let mut h = h;
    let tool = |name: &str| crate::domain::mcp::McpTool {
        name: name.into(),
        description: format!("{name}s issues."),
        input_schema: serde_json::json!({"type": "object"}),
        ..Default::default()
    };
    h.mcp = McpTools::new(vec![crate::domain::mcp::ConnectedServer {
        name: "tracker".into(),
        config: crate::domain::mcp::McpServerConfig {
            exposure: Some(Exposure::Deferred),
            tool_exposure: [("find".to_string(), Exposure::Direct), ("purge".to_string(), Exposure::Hidden)].into(),
            ..Default::default()
        },
        client: Arc::new(Echo),
        tools: vec![tool("find"), tool("list"), tool("purge")],
        instructions: None,
    }]);
    h
}

/// A deferred tool is not in the request until `toolSearch` has found
/// it, and is from the next round on — in this turn and the next.
#[test]
fn a_deferred_tool_is_declared_once_the_search_has_found_it() {
    let script = vec![
        asks(vec![wants("s1", "toolSearch", r#"{"query":"list"}"#)]),
        asks(vec![wants("m1", "mcp__tracker__list", "{}")]),
        text("done"),
    ];
    let h = with_deferred(harness("mcp-deferred", script));
    let history = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");
    let requests = h.provider.requests();
    let names = |i: usize| requests[i].tools.iter().map(|t| t.name.clone()).filter(|n| n.contains("tracker") || n == "toolSearch").collect::<Vec<_>>();
    assert_eq!(names(0), ["toolSearch", "mcp__tracker__find"]);
    assert_eq!(names(1), ["toolSearch", "mcp__tracker__find", "mcp__tracker__list"]);
    let ran = requests[2].messages.iter().find(|m| m.tool_call_id.as_deref() == Some("m1")).unwrap();
    assert!(ran.content.as_deref().unwrap().contains("list got"), "{:?}", ran.content);
    let said = requests[0].messages.iter().filter_map(|m| m.content.as_deref()).find(|c| c.contains("## MCP servers")).expect("said");
    assert!(said.contains("### tracker — 1 tool found with toolSearch"), "{said}");

    let ChatStreamOutcome::Done(done) = history else { panic!("paused") };
    let next = with_deferred(harness("mcp-deferred-next", vec![text("again")]));
    next.run(|turn| stream(turn, done.history.clone(), vec![])).expect("finishes");
    assert!(next.provider.requests()[0].tools.iter().any(|t| t.name == "mcp__tracker__list"), "the next turn keeps it");
}

/// A hidden tool is not offered, not found, and refused when called
/// from memory; without deferred tools there is no search.
#[test]
fn a_hidden_tool_is_nowhere_and_the_search_only_comes_with_something_to_find() {
    let script = vec![
        asks(vec![wants("s1", "toolSearch", r#"{"query":"purge"}"#), wants("m1", "mcp__tracker__purge", "{}")]),
        text("done"),
    ];
    let h = with_deferred(harness("mcp-hidden", script));
    h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");
    let requests = h.provider.requests();
    assert!(requests[0].tools.iter().all(|t| t.name != "mcp__tracker__purge"));
    let result = |id: &str| requests[1].messages.iter().find(|m| m.tool_call_id.as_deref() == Some(id)).unwrap().content.clone().unwrap();
    assert!(result("s1").starts_with("No tool matched"), "{}", result("s1"));
    assert!(result("m1").contains("mcp__tracker__purge") && !result("m1").contains("purge got"), "{}", result("m1"));

    // A server with nothing waiting: the search is the app's own tools', and says nothing of servers.
    let plain = with_server(harness("mcp-no-search", vec![text("hi")]), 3);
    plain.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");
    let search = plain.provider.requests()[0].tools.iter().find(|t| t.name == "toolSearch").cloned().expect("offered");
    assert!(!search.description.contains("MCP") && search.description.contains("`gitBlame`"), "{}", search.description);
}

/// The app's rarer tools wait for `toolSearch` in Agent mode and Analyst,
/// and one it found is declared from the next step on; where there is no
/// search — a review — they are declared as ever.
#[test]
fn a_waiting_tool_is_declared_once_the_search_found_it() {
    for mode in [ConversationMode::Agent, ConversationMode::Analyst] {
        let mut h = harness(
            "builtin-search",
            vec![asks(vec![wants("s1", "toolSearch", r#"{"query":"blame"}"#)]), text("done")],
        );
        h.mode = mode;
        h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");
        let requests = h.provider.requests();
        assert!(!tool_names(&requests[0]).contains(&"gitBlame".to_string()), "{mode:?}");
        assert!(tool_names(&requests[1]).contains(&"gitBlame".to_string()), "{mode:?}: {:?}", tool_names(&requests[1]));
        assert!(!tool_names(&requests[1]).contains(&"move".to_string()), "only what was found");
    }

    let mut review = harness("builtin-review", vec![text("hi")]);
    review.mode = ConversationMode::Review;
    review.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");
    let tools = tool_names(&review.provider.requests()[0]);
    assert!(tools.contains(&"gitBlame".to_string()) && !tools.contains(&"toolSearch".to_string()), "{tools:?}");
}

/// The agent searches only when Settings gave it a search: off, neither
/// web tool is offered; on, both are — in Analyst too, since they only
/// read — and a call reaches the search.
#[test]
fn the_web_tools_are_offered_to_the_agent_only_when_its_search_is_on() {
    let off = harness("web-off", vec![text("hi")]);
    off.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");
    assert!(off.provider.requests()[0].tools.iter().all(|t| t.name != "webSearch" && t.name != "webFetch"));

    for mode in [ConversationMode::Agent, ConversationMode::Analyst] {
        let mut on = harness("web-on", vec![asks(vec![wants("w1", "webSearch", r#"{"query":"tokio 2"}"#)]), text("ok")]);
        on.mode = mode;
        let searched = Arc::new(Mutex::new(Vec::new()));
        let seen = searched.clone();
        on.web = Some(Web::search_only(Arc::new(move |q: &crate::domain::web_search::WebQuery| {
            seen.lock().unwrap().push(q.query.to_string());
            Ok(Vec::new())
        })));
        on.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");
        let names: Vec<String> = on.provider.requests()[0].tools.iter().map(|t| t.name.clone()).collect();
        assert!(names.iter().any(|n| n == "webSearch") && names.iter().any(|n| n == "webFetch"), "{mode:?}: {names:?}");
        assert_eq!(*searched.lock().unwrap(), ["tokio 2"], "{mode:?}");
    }
}

/// Offered in Agent and Analyst beside the built-in tools; not in a review,
/// which has its own job and no user to ask.
#[test]
fn a_servers_tools_are_offered_where_the_mode_has_them() {
    for mode in [ConversationMode::Agent, ConversationMode::Analyst] {
        let mut h = with_server(harness("mcp-offered", vec![text("hi")]), 3);
        h.mode = mode;
        h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");
        let offered = &h.provider.requests()[0].tools;
        let tracker = offered.iter().find(|t| t.name == "mcp__tracker__find").expect("offered");
        assert_eq!(tracker.description, "[MCP server \"tracker\"] Finds issues.");
    }

    let mut review = with_server(harness("mcp-review", vec![asks(vec![wants("m1", "mcp__tracker__find", "{}")]), text("ok")]), 3);
    review.mode = ConversationMode::Review;
    review.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");
    let requests = review.provider.requests();
    assert!(requests[0].tools.iter().all(|t| !t.name.starts_with("mcp__")));
    let refused = requests[1].messages.iter().find(|m| m.tool_call_id.as_deref() == Some("m1")).unwrap();
    assert!(refused.content.as_deref().unwrap().contains("not available in this conversation mode"));
}

/// The one thing in Analyst that might change something: it asks, as in Agent.
#[test]
fn an_analysts_call_to_a_server_asks_first() {
    let mut h = with_server(harness("mcp-analyst-asks", vec![asks(vec![wants("m1", "mcp__tracker__find", "{}")])]), 3);
    h.mode = ConversationMode::Analyst;
    h.approval = asking();

    let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]));
    let ChatStreamOutcome::PendingApproval(pending) = outcome.expect("pauses") else {
        panic!("expected a pause");
    };
    assert!(pending.calls[0].requires_confirmation);
}

/// The same gate as a write: it asks, and the round is charged the
/// server's weight, not a built-in tool's.
#[test]
fn a_call_asks_first_and_costs_its_servers_weight() {
    let mut h = with_server(harness("mcp-asks", vec![asks(vec![wants("m1", "mcp__tracker__find", r#"{"q":"crash"}"#)])]), 7);
    h.approval = asking();

    let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]));
    let ChatStreamOutcome::PendingApproval(pending) = outcome.expect("pauses") else {
        panic!("expected a pause");
    };
    assert!(pending.calls[0].requires_confirmation);
    assert_eq!(pending.budget_used, 7);
}

/// "Always allow" for one server tool holds in the loop, not only in the
/// policy's own tests: the loop has to ask the MCP-aware gate.
#[test]
fn a_tool_allowed_always_runs_without_asking() {
    let mut h = with_server(harness("mcp-always", vec![asks(vec![wants("m1", "mcp__tracker__find", "{}")]), text("done")]), 3);
    h.approval = asking();
    h.approval.allow_always("mcp__tracker__find").unwrap();

    let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]));
    assert!(matches!(outcome, Ok(ChatStreamOutcome::Done(_))), "it paused");
}

/// A server that asks the user something mid-call: the window is told —
/// which call, which server, what — the answer it sends back reaches the
/// server, and the window hears the question closed. Nobody to ask is a
/// declined question.
#[test]
fn a_servers_question_goes_to_the_window_and_its_answer_back_to_the_server() {
    use crate::domain::mcp::{McpAnswer, McpQuestion};
    struct Asks;
    impl crate::domain::mcp::McpClient for Asks {
        fn list_tools(&self) -> Result<Vec<crate::domain::mcp::McpTool>, crate::domain::mcp::McpError> {
            Ok(vec![])
        }
        fn call_tool(&self, name: &str, arguments: serde_json::Value, cancelled: &dyn Fn() -> bool) -> Result<crate::domain::mcp::McpCallResult, crate::domain::mcp::McpError> {
            self.call_tool_asking(name, arguments, cancelled, &|_| McpAnswer::Decline)
        }
        fn call_tool_asking(
            &self,
            _: &str,
            _: serde_json::Value,
            _: &dyn Fn() -> bool,
            ask: &dyn Fn(&McpQuestion) -> McpAnswer,
        ) -> Result<crate::domain::mcp::McpCallResult, crate::domain::mcp::McpError> {
            let answer = ask(&McpQuestion::Url { message: "Sign in".into(), url: "https://a.example/in".into() });
            Ok(crate::domain::mcp::McpCallResult { text: format!("answered {}", answer.action()), is_error: false })
        }
    }
    let with_asking = |label: &str| {
        let mut h = harness(label, vec![asks(vec![wants("m1", "mcp__tracker__login", "{}")]), text("done")]);
        h.mcp = McpTools::new(vec![crate::domain::mcp::ConnectedServer {
            name: "tracker".into(),
            config: Default::default(),
            client: Arc::new(Asks),
            tools: vec![crate::domain::mcp::McpTool { name: "login".into(), input_schema: serde_json::json!({}), ..Default::default() }],
            instructions: None,
        }]);
        h
    };

    let mut h = with_asking("mcp-question");
    let desk = Arc::new(crate::services::mcp_questions::McpQuestions::default());
    h.questions = Some(Arc::clone(&desk));
    let log = h.log.clone();
    let window = std::thread::spawn(move || loop {
        let asked = log.lock().unwrap().iter().find_map(|e| match &e.event {
            ChatEventPayload::McpQuestion { id, call, server, question } => Some((id.clone(), call.clone(), server.clone(), question.clone())),
            _ => None,
        });
        if let Some((id, call, server, question)) = asked {
            assert!(desk.answer(&id, McpAnswer::Accept { content: Default::default() }));
            return (call, server, question);
        }
        std::thread::sleep(Duration::from_millis(5));
    });
    h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");
    let (call, server, question) = window.join().unwrap();
    assert_eq!((call.as_str(), server.as_str()), ("m1", "tracker"));
    assert!(matches!(question, McpQuestion::Url { url, .. } if url == "https://a.example/in"));
    let closed = h.events().into_iter().find_map(|e| match e.event {
        ChatEventPayload::McpQuestionClosed { action, .. } => Some(action),
        _ => None,
    });
    assert_eq!(closed.as_deref(), Some("accept"));
    let sent = &h.provider.requests()[1].messages;
    assert!(sent.iter().any(|m| m.content.as_deref().is_some_and(|c| c.contains("answered accept"))));

    let nobody = with_asking("mcp-question-nobody");
    nobody.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");
    assert!(nobody.events().iter().all(|e| !matches!(e.event, ChatEventPayload::McpQuestion { .. })));
    let sent = &nobody.provider.requests()[1].messages;
    assert!(sent.iter().any(|m| m.content.as_deref().is_some_and(|c| c.contains("answered decline"))));
}

/// Stop reaches a call that is waiting on a server, not only the loop
/// around it: the server is told and the call ends as cancelled.
#[test]
fn stopping_the_turn_reaches_a_server_call_in_flight() {
    struct WaitsForStop(Arc<Mutex<Option<usize>>>);
    impl crate::domain::mcp::McpClient for WaitsForStop {
        fn list_tools(&self) -> Result<Vec<crate::domain::mcp::McpTool>, crate::domain::mcp::McpError> {
            Ok(vec![])
        }
        fn call_tool(
            &self,
            _: &str,
            _: serde_json::Value,
            cancelled: &dyn Fn() -> bool,
        ) -> Result<crate::domain::mcp::McpCallResult, crate::domain::mcp::McpError> {
            // The user presses Stop while this call is running.
            *self.0.lock().unwrap() = Some(0);
            for _ in 0..200 {
                if cancelled() {
                    return Err(crate::domain::mcp::McpError::Cancelled);
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            Ok(crate::domain::mcp::McpCallResult { text: "never stopped".into(), is_error: false })
        }
    }
    let mut h = harness("mcp-stop", vec![asks(vec![wants("m1", "mcp__slow__wait", "{}")])]);
    h.mcp = McpTools::new(vec![crate::domain::mcp::ConnectedServer {
        name: "slow".into(),
        config: Default::default(),
        client: Arc::new(WaitsForStop(h.cancel_after.clone())),
        tools: vec![crate::domain::mcp::McpTool { name: "wait".into(), input_schema: serde_json::json!({}), ..Default::default() }],
        instructions: None,
    }]);

    let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]));
    assert!(matches!(outcome, Ok(ChatStreamOutcome::Cancelled(_))));
    let logged = h.logged.lock().unwrap();
    assert_eq!(logged[0].status, CallStatus::Error, "the call ran to its end instead: {:?}", logged[0]);
}

/// Run: the server's text reaches the model as it is, and the log line
/// keeps the tool and the shape of what was sent — not the values.
#[test]
fn a_call_runs_through_the_log_and_its_text_reaches_the_model() {
    let h = with_server(
        harness("mcp-runs", vec![asks(vec![wants("m1", "mcp__tracker__find", r#"{"q":"secret words"}"#)]), text("done")]),
        3,
    );
    h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");

    let second = &h.provider.requests()[1];
    let result = second.messages.iter().find(|m| m.tool_call_id.as_deref() == Some("m1")).unwrap();
    assert_eq!(result.content.as_deref(), Some(r#"find got {"q":"secret words"}"#));

    let logged = h.logged.lock().unwrap();
    assert_eq!(logged[0].tool, "mcp__tracker__find");
    assert_eq!(logged[0].status, CallStatus::Ok);
    let line = serde_json::to_string(&*logged).unwrap();
    assert!(!line.contains("secret words"), "{line}");
    assert!(line.contains("<string, 12 chars>"), "{line}");
}
