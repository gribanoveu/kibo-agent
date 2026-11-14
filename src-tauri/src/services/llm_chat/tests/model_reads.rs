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

/// So does the HTTP client's, as a read: no card asks for it, and nothing is offered with the feature off.
#[test]
fn the_agent_is_offered_http_only_while_the_feature_is_on() {
    let offered = |features: Features| {
        let mut h = harness("loop-http", vec![text("hi")]);
        h.features = features;
        h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");
        let tools = &h.provider.requests()[0].tools;
        assert!(tools.iter().all(|t| !t.name.starts_with("http")), "not in every request");
        ["httpEnvironment", "httpSend", "httpReadRequest"].iter().all(|n| tools.iter().any(|t| t.name == "toolSearch" && t.description.contains(&format!("`{n}`"))))
    };
    assert!(!offered(Features::default()));
    assert!(offered(Features::all()));
}

/// Found with `toolSearch`, it runs with no card and the stand's answer reaches the model.
#[test]
fn the_agent_reads_a_stand_once_it_has_found_the_tool() {
    let script = vec![
        asks(vec![wants("s1", "toolSearch", r#"{"query":"http"}"#)]),
        asks(vec![wants("h1", "httpSend", r#"{"request":"r1"}"#)]),
        text("done"),
    ];
    let mut h = harness("loop-http-call", script);
    h.features = Features::all();
    h.http = Some(crate::services::ai_tools::tools::http_send::tests::answering());
    h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");
    let requests = h.provider.requests();
    let ran = requests[2].messages.iter().find(|m| m.tool_call_id.as_deref() == Some("h1")).unwrap();
    assert!(ran.content.as_deref().unwrap().starts_with("GET {{baseUrl}}/orders/1 → 200"), "{:?}", ran.content);
}

/// A write asks by the environment's mode, read when the call comes: Ask for writes
/// even in Auto and says why, Writes as other tools as any change. A read asks in none,
/// and a call to an environment not open asks nothing — the tool refuses it.
#[test]
fn a_write_to_a_stand_asks_by_its_mode() {
    use crate::domain::http_client::AgentAccess;
    use crate::services::ai_tools::tools::http_send::tests::opened_as;
    let run = |access: AgentAccess, approval: ApprovalPolicy, call: &str| {
        let script = vec![
            asks(vec![wants("s1", "toolSearch", r#"{"query":"http"}"#)]),
            asks(vec![wants("h1", "httpSend", call)]),
            text("done"),
        ];
        let mut h = harness("loop-http-write", script);
        h.features = Features::all();
        h.http = Some(opened_as(access));
        h.approval = approval;
        match h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes") {
            ChatStreamOutcome::PendingApproval(pending) => Some(pending.calls[0].reason.clone()),
            _ => None,
        }
    };
    let auto = ApprovalPolicy { skip_all: true, ..ApprovalPolicy::default() };
    let post = r#"{"request":"w1","body":"{}"}"#;
    let get = r#"{"request":"r1"}"#;
    let carded = |what: &str| Some(Some(format!("{what} — \"Stage\" asks for every write")));
    assert_eq!(run(AgentAccess::AskForWrites, auto.clone(), post), carded("POST {{baseUrl}}/orders, from \"Shop / Create\""), "the saved request's method");
    assert_eq!(run(AgentAccess::AskForWrites, asking(), get), None, "a read");
    let made = r#"{"request":"r1","method":"post"}"#;
    assert_eq!(run(AgentAccess::AskForWrites, asking(), made), carded("POST {{baseUrl}}/orders/1, from \"Shop / Get one\""), "the call's");
    let read = r#"{"request":"w1","method":"HEAD"}"#;
    assert_eq!(run(AgentAccess::AskForWrites, asking(), read), None, "a read of a POST's address");
    assert_eq!(run(AgentAccess::WritesAsOtherTools, asking(), post), Some(Some("POST {{baseUrl}}/orders, from \"Shop / Create\"".into())));
    assert_eq!(run(AgentAccess::WritesAsOtherTools, auto.clone(), post), None, "Auto");
    assert_eq!(run(AgentAccess::ReadOnly, asking(), post), None, "refused by the tool, not asked");
    let elsewhere = r#"{"request":"w1","environment":"Prod","method":"DELETE"}"#;
    assert_eq!(run(AgentAccess::AskForWrites, asking(), elsewhere), None, "not open");
    assert_eq!(run(AgentAccess::AskForWrites, asking(), r#"{"request":"nope","method":"DELETE"}"#), None, "no such request: refused by the tool");
    assert_eq!(run(AgentAccess::AskForWrites, asking(), r#"{"request":"w1","auth":"basic"}"#), None, "a call that cannot be read");
    let unread = Some(Some("the HTTP environments could not be read, so a write asks".to_string()));
    assert_eq!(run(AgentAccess::AskForWrites, auto.clone(), r#"{"request":"s1"}"#), unread, "a collection that could not be read: its method is unknown");
    assert_eq!(run(AgentAccess::AskForWrites, auto.clone(), r#"{"request":"s1","method":"GET"}"#), None, "but the call's own read");
    let overridden = r#"{"request":"r1","headers":{"X-HTTP-Method-Override":"DELETE"}}"#;
    assert_eq!(run(AgentAccess::AskForWrites, auto.clone(), overridden), carded("GET {{baseUrl}}/orders/1, from \"Shop / Get one\""), "a GET that deletes");

    // Environments that cannot be read: a write asks, a read does not.
    let unread = |call: &str| {
        let script = vec![asks(vec![wants("s1", "toolSearch", r#"{"query":"http"}"#)]), asks(vec![wants("h1", "httpSend", call)]), text("done")];
        let mut h = harness("loop-http-unread", script);
        h.features = Features::all();
        h.http = Some(crate::services::ai_tools::tools::http_send::tests::unreadable());
        h.approval = auto.clone();
        match h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes") {
            ChatStreamOutcome::PendingApproval(pending) => Some(pending.calls[0].reason.clone()),
            _ => None,
        }
    };
    assert_eq!(unread(post), Some(Some("the HTTP environments could not be read, so a write asks".into())));
    assert_eq!(unread(r#"{"request":"r1"}"#), Some(Some("the HTTP environments could not be read, so a write asks".into())), "its method is the saved request's, unknown");
    assert_eq!(unread(r#"{"request":"r1","method":"GET"}"#), None, "a read by the call's own method");
}

/// A case kept in the user's collections asks every time, in Auto and after "Always
/// allow" too, and says what it adds, whatever the environment's mode; with no
/// HTTP stands there is nothing to approve — the tool refuses.
#[test]
fn a_case_saved_always_asks() {
    use crate::domain::http_client::AgentAccess;
    use crate::services::ai_tools::tools::http_send::tests::opened_as;
    let run = |http: bool, approval: ApprovalPolicy| {
        let script = vec![
            asks(vec![wants("s1", "toolSearch", r#"{"query":"http"}"#)]),
            asks(vec![wants("h1", "httpSaveRequest", r#"{"from":"r1","name":" No auth ","auth":"none"}"#)]),
            text("done"),
        ];
        let mut h = harness("loop-http-save", script);
        h.features = Features::all();
        h.http = http.then(|| opened_as(AgentAccess::ReadOnly));
        h.approval = approval;
        match h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes") {
            ChatStreamOutcome::PendingApproval(pending) => Some(pending.calls[0].reason.clone()),
            _ => None,
        }
    };
    let told = Some(Some("it adds \"No auth\" to your HTTP collections — every save asks".to_string()));
    assert_eq!(run(true, ApprovalPolicy { skip_all: true, ..ApprovalPolicy::default() }), told, "Auto; a read-only environment");
    let mut allowed = ApprovalPolicy::default();
    allowed.allow_always("httpSaveRequest").unwrap();
    assert_eq!(run(true, allowed), told, "Always allow");
    assert_eq!(run(false, asking()), None, "no stands");
}

/// A write the round let through unasked, while another call's card waited, asks
/// once the mode has changed: it is refused and tells the model to call again.
/// With the mode as it was, it runs.
#[test]
fn an_http_write_let_through_while_a_card_waited_asks_if_the_mode_changed() {
    use crate::domain::http_client::AgentAccess;
    use crate::services::ai_tools::tools::http_send::tests::opened_as;
    let resumed = |now: AgentAccess| {
        let script = vec![
            asks(vec![wants("s1", "toolSearch", r#"{"query":"http"}"#)]),
            asks(vec![
                wants("w1", "writeFile", r#"{"path":"a.rs","content":"new"}"#),
                wants("h1", "httpSend", r#"{"request":"w1"}"#),
            ]),
            text("done"),
        ];
        let mut h = harness("loop-http-resumed", script);
        h.features = Features::all();
        h.http = Some(opened_as(AgentAccess::WritesAsOtherTools));
        h.approval = asking();
        h.approval.allow_always("httpSend").unwrap();
        let paused = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]));
        let ChatStreamOutcome::PendingApproval(pending) = paused.expect("pauses") else { panic!("ran unasked") };
        assert_eq!(pending.calls.iter().map(|c| c.requires_confirmation).collect::<Vec<_>>(), [true, false]);
        h.http = Some(opened_as(now));
        let decision = ToolCallDecision { id: "w1".into(), approved: true, reason: None };
        assert!(matches!(h.run(|turn| resume(turn, pending, vec![decision])).expect("finishes"), ChatStreamOutcome::Done(_)));
        let requests = h.provider.requests();
        let last = requests.last().unwrap();
        last.messages.iter().find(|m| m.tool_call_id.as_deref() == Some("h1")).unwrap().content.clone().unwrap()
    };
    let refused = resumed(AgentAccess::AskForWrites);
    assert!(refused.contains("needs the user's approval now"), "{refused}");
    let ran = resumed(AgentAccess::WritesAsOtherTools);
    assert!(ran.starts_with("POST {{baseUrl}}/orders → 200"), "{ran}");
}

#[test]
fn every_request_carries_the_tool_schemas() {
    let h = harness("loop-tools", vec![text("hi")]);

    h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");

    let requests = h.provider.requests();
    let offered: &[LlmToolDefinition] = &requests[0].tools;
    // Every built-in one but a review's own, the features' — the board,
    // DeepSeek's two, MongoDB's one, HTTP's five — the Kubernetes role's and the
    // Prototype role's `present`; MCP tools come from servers, and none
    // is connected. Those waiting for `toolSearch` are found with it.
    let waiting = waiting_tools(ConversationMode::Agent, false, &Features::default()).len();
    assert_eq!(waiting, 3, "move, gitBlame, readTerminal");
    // And `toolSearch`, which is there for them.
    assert_eq!(offered.len(), ToolName::ALL.len() - 13 - ChatRole::Kubernetes.tools().len() - waiting + 1);
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
