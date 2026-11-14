use super::*;

// ---------------------------------------------------------------- hooks

pub(super) type HookInputs = Arc<Mutex<Vec<(String, serde_json::Value)>>>;

/// Hooks answered by `answer(command, input) -> (exit code, stderr)`;
/// every run is kept, with the input it was given.
pub(super) fn hooked(
    mut h: Harness,
    config: serde_json::Value,
    answer: impl Fn(&str, &serde_json::Value) -> (i32, &'static str) + Send + Sync + 'static,
) -> (Harness, HookInputs) {
    let inputs: HookInputs = Arc::default();
    let seen = Arc::clone(&inputs);
    h.hooks = Hooks::new(
        serde_json::from_value(config).unwrap(),
        Arc::new(move |hook: &crate::domain::hooks::HookCommand, input: &str, _: &std::path::Path| {
            let input: serde_json::Value = serde_json::from_str(input).unwrap();
            let (code, stderr) = answer(&hook.command, &input);
            seen.lock().unwrap().push((hook.command.clone(), input));
            Ok(crate::domain::command_exec::CommandOutput {
                stdout: String::new(),
                stderr: stderr.into(),
                exit_code: Some(code),
                timed_out: false,
                stopped: false,
                truncated: false,
                duration_ms: 0,
                full_output: None,
            })
        }),
    );
    (h, inputs)
}

pub(super) fn hook(event: &str, matcher: &str, command: &str) -> serde_json::Value {
    serde_json::json!({"hooks": {event: [{"matcher": matcher, "hooks": [{"type": "command", "command": command}]}]}})
}

pub(super) fn feedback(h: &Harness) -> Vec<(String, String, bool)> {
    h.events()
        .into_iter()
        .filter_map(|e| match e.event {
            ChatEventPayload::HookFeedback { event, message, blocked } => Some((event, message, blocked)),
            _ => None,
        })
        .collect()
}

/// The approved call does not run; the model reads the hook's reason, the
/// log says only that a hook blocked it.
#[test]
fn a_pre_tool_use_hook_that_exits_two_refuses_the_call() {
    let (h, inputs) = hooked(
        harness("hook-pre-block", vec![asks(vec![wants("w1", "writeFile", r#"{"path":"a.rs","content":"x"}"#)]), text("ok")]),
        hook("PreToolUse", "writeFile|editFile", "guard"),
        |_, _| (2, "no writes on Fridays"),
    );
    h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");

    assert!(!h.root.join("a.rs").exists());
    let said = tool_contents(&h.provider.requests()[1]);
    assert_eq!(said, ["Error: a hook refused this call: no writes on Fridays"]);
    assert_eq!(feedback(&h), [("PreToolUse".to_string(), "no writes on Fridays".to_string(), true)]);
    let input = &inputs.lock().unwrap()[0].1;
    assert_eq!(input["tool_name"], "writeFile");
    assert_eq!(input["tool_input"]["path"], "a.rs");
    assert_eq!(input["tool_use_id"], "w1");
    assert_eq!(input["hook_event_name"], "PreToolUse");
    let logged = h.logged.lock().unwrap().clone();
    assert_eq!(logged[0].error.as_deref(), Some("blocked by a hook"));
}

#[test]
fn a_hook_that_fails_otherwise_only_warns_and_one_for_another_tool_does_not_run() {
    let config = serde_json::json!({"hooks": {"PreToolUse": [
        {"matcher": "writeFile", "hooks": [{"type": "command", "command": "broken"}]},
        {"matcher": "runCommand", "hooks": [{"type": "command", "command": "elsewhere"}]},
    ]}});
    let (h, inputs) = hooked(
        harness("hook-pre-warn", vec![asks(vec![wants("w1", "writeFile", r#"{"path":"a.rs","content":"x"}"#)]), text("ok")]),
        config,
        |_, _| (1, "jq: not found"),
    );
    h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");

    assert_eq!(std::fs::read_to_string(h.root.join("a.rs")).unwrap(), "x", "the call ran");
    assert_eq!(inputs.lock().unwrap().iter().map(|(c, _)| c.as_str()).collect::<Vec<_>>(), ["broken"]);
    assert_eq!(feedback(&h), [("PreToolUse".to_string(), "hook `broken` failed with code 1: jq: not found".to_string(), false)]);
}

/// A call the user denied never reaches the hooks: nothing is about to run.
#[test]
fn a_denied_call_does_not_ask_the_hooks() {
    let (mut h, inputs) = hooked(
        harness("hook-denied", vec![asks(vec![wants("w1", "writeFile", r#"{"path":"a.rs","content":"x"}"#)]), text("ok")]),
        hook("PreToolUse", "", "guard"),
        |_, _| (0, ""),
    );
    h.approval = asking();
    let Ok(ChatStreamOutcome::PendingApproval(pending)) = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])) else {
        panic!("expected a pause");
    };
    let no = vec![ToolCallDecision { id: "w1".into(), approved: false, reason: None }];
    h.run(|turn| resume(turn, pending, no)).expect("finishes");
    assert!(inputs.lock().unwrap().is_empty());
}

/// After the call it is too late to refuse; what the hook says goes to
/// the model beside the result.
#[test]
fn a_post_tool_use_hook_speaks_to_the_model_after_the_call() {
    let (h, inputs) = hooked(
        harness("hook-post", vec![asks(vec![wants("w1", "writeFile", r#"{"path":"a.rs","content":"x"}"#)]), text("ok")]),
        hook("PostToolUse", "writeFile", "lint"),
        |_, _| (2, "a.rs: missing semicolon"),
    );
    h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");

    assert!(h.root.join("a.rs").exists(), "it ran");
    let said = tool_contents(&h.provider.requests()[1]);
    assert!(said[0].ends_with("\n\n[A PostToolUse hook said:]\na.rs: missing semicolon"), "{said:?}");
    assert!(!inputs.lock().unwrap()[0].1["tool_response"].is_null());
}

#[test]
fn a_failed_call_does_not_reach_post_tool_use() {
    let (h, inputs) = hooked(
        harness("hook-post-failed", vec![asks(vec![wants("r1", "readFile", r#"{"path":"missing.rs"}"#)]), text("ok")]),
        hook("PostToolUse", "", "lint"),
        |_, _| (0, ""),
    );
    h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");
    assert!(inputs.lock().unwrap().is_empty());
}

/// "Run the tests before you stop": the hook sends the model back once,
/// and lets it go when told it already has.
#[test]
fn a_stop_hook_sends_the_model_back_until_it_lets_go() {
    let (h, inputs) = hooked(
        harness("hook-stop", vec![text("done"), text("tests pass, done")]),
        hook("Stop", "", "check"),
        |_, input| if input["stop_hook_active"] == true { (0, "") } else { (2, "run the tests first") },
    );
    let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");
    let ChatStreamOutcome::Done(done) = outcome else { panic!("expected done") };
    assert_eq!(done.result.text, "tests pass, done");

    let second = &h.provider.requests()[1];
    let tail: Vec<_> = second.messages.iter().rev().take(2).collect();
    assert_eq!(tail[1].content.as_deref(), Some("done"));
    assert_eq!(tail[0].role, LlmRole::User);
    assert!(tail[0].content.as_deref().unwrap().ends_with("It said:]\nrun the tests first"));
    assert_eq!(inputs.lock().unwrap().len(), 2);
}

#[test]
fn stop_hooks_keep_a_turn_going_only_so_many_times() {
    let steps = (0..=MAX_STOP_BLOCKS).map(|i| text(&format!("done {i}"))).collect();
    let (h, inputs) = hooked(harness("hook-stop-cap", steps), hook("Stop", "", "never"), |_, _| (2, "no"));
    h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");

    assert_eq!(h.provider.requests().len(), MAX_STOP_BLOCKS as usize + 1);
    assert_eq!(inputs.lock().unwrap().len(), MAX_STOP_BLOCKS as usize + 1, "the last one still runs");
    let last = feedback(&h).pop().unwrap();
    assert_eq!((last.1.contains("ends here anyway"), last.2), (true, false));
}
