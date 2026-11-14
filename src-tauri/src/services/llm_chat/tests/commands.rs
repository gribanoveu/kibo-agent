use super::*;

// --------------------------------------------------- background processes

/// Reports one ended process, once.
pub(super) struct EndedOnce(Mutex<Vec<crate::domain::background::ProcessInfo>>);
impl BackgroundProcesses for EndedOnce {
    fn start(&self, _: &Shell, _: &str, _: &std::path::Path, _: &str) -> Result<crate::domain::background::ProcessInfo, crate::domain::background::BackgroundError> {
        unreachable!()
    }
    fn read(&self, _: u32) -> Result<crate::domain::background::ProcessOutput, crate::domain::background::BackgroundError> {
        unreachable!()
    }
    fn stop(&self, _: u32) -> Result<crate::domain::background::ProcessInfo, crate::domain::background::BackgroundError> {
        unreachable!()
    }
    fn list(&self) -> Vec<crate::domain::background::ProcessInfo> {
        vec![]
    }
    fn take_ended(&self) -> Vec<crate::domain::background::ProcessInfo> {
        std::mem::take(&mut self.0.lock().unwrap())
    }
    fn adopt(
        &self,
        _: std::process::Child,
        _: &str,
        _: &str,
        _: &str,
    ) -> Result<(crate::domain::background::ProcessInfo, [crate::domain::background::Feed; 2]), crate::domain::background::BackgroundError> {
        unreachable!()
    }
}

/// The turn hands its processes to the tools: a background start in the
/// loop comes back as a number, not as a command that ran.
#[cfg(unix)]
#[test]
fn a_background_start_in_the_loop_reaches_the_registry() {
    let mut h = harness(
        "bg-loop",
        vec![asks(vec![wants("b1", "runCommand", r#"{"command":"sleep 30","background":true}"#)]), text("ok")],
    );
    let processes = Arc::new(crate::infra::background::Processes::default());
    h.processes = Some(processes.clone());
    h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");

    let said = tool_contents(&h.provider.requests()[1]);
    assert!(said[0].starts_with("Started background process #1 in "), "{said:?}");
    assert!(processes.list()[0].running());
}

/// "Is it going all right?" typed during a build is answered during it:
/// the build steps aside to the background and the note reaches the
/// next round, instead of both waiting for the build to end.
#[cfg(unix)]
#[test]
fn a_note_typed_during_a_command_is_answered_while_it_runs() {
    let mut h = harness(
        "steer-during-command",
        vec![
            asks(vec![wants("c1", "runCommand", r#"{"command":"echo building; sleep 30","timeoutSeconds":60}"#)]),
            text("all fine, still building"),
        ],
    );
    let processes = Arc::new(crate::infra::background::Processes::default());
    h.processes = Some(processes.clone());
    let steering = h.steering.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(1));
        steering.push(SteeringNote::user("is it going all right?"));
    });

    let started = Instant::now();
    h.run(|turn| stream(turn, vec![LlmMessage::user("build it")], vec![])).expect("finishes");

    assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
    let second = &h.provider.requests()[1];
    assert!(tool_contents(second)[0].starts_with("Still running"), "{:?}", tool_contents(second));
    assert!(user_messages(second).iter().any(|m| m.contains("is it going all right?")), "{:?}", user_messages(second));
    assert!(processes.list()[0].running());
    processes.stop(1).unwrap();
}

/// A dev server that died between turns is news the model gets before
/// its next round — once, and in the history, so a retry keeps it.
#[test]
fn a_process_that_ended_is_told_to_the_model_once() {
    let mut h = harness("bg-ended", vec![text("I see"), text("ok")]);
    h.processes = Some(Arc::new(EndedOnce(Mutex::new(vec![crate::domain::background::ProcessInfo {
        id: 2,
        command: "npm run dev".into(),
        cwd: ".".into(),
        state: crate::domain::background::ProcessState::Exited { code: Some(1) },
    }]))));
    h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");
    h.run(|turn| stream(turn, vec![LlmMessage::user("again")], vec![])).expect("finishes");

    let requests = h.provider.requests();
    let told = |request: &ChatRequest| {
        request.messages.iter().any(|m| {
            m.role == LlmRole::User && m.content.as_deref().is_some_and(|c| c.contains("#2 `npm run dev` exited with code 1"))
        })
    };
    assert!(told(&requests[0]));
    assert!(!told(&requests[1]), "said once");
    assert!(payloads(&h.events()).contains(&"ended:1".to_string()));
}

// ---------------------------------------------------- running a command

/// Scenario S-1, the reason stage two exists: the agent runs the tests,
/// reads the failure, fixes the code, and runs them again. Nothing about
/// it is mocked except the model's side of the conversation.
#[test]
fn the_agent_runs_a_failing_test_fixes_the_code_and_runs_it_again() {
    let h = harness(
        "s1",
        vec![
            asks(vec![wants("c1", "runCommand", r#"{"command":"sh check.sh"}"#)]),
            asks(vec![wants("r1", "readFile", r#"{"path":"answer.txt"}"#)]),
            asks(vec![wants(
                "e1",
                "editFile",
                r#"{"path":"answer.txt","edits":[{"old":"41","new":"42"}]}"#,
            )]),
            asks(vec![wants("c2", "runCommand", r#"{"command":"sh check.sh"}"#)]),
            text("fixed: the answer was 41, it is now 42"),
        ],
    );
    std::fs::write(
        h.root.join("check.sh"),
        "if [ \"$(cat answer.txt)\" = \"42\" ]; then echo PASS; else echo \"FAIL: expected 42, got $(cat answer.txt)\"; exit 1; fi\n",
    )
    .unwrap();
    std::fs::write(h.root.join("answer.txt"), "41").unwrap();

    let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("make the test pass")], vec![]));

    assert!(matches!(outcome.expect("finishes"), ChatStreamOutcome::Done(_)));
    assert_eq!(std::fs::read_to_string(h.root.join("answer.txt")).unwrap(), "42");

    let rounds = h.provider.requests();
    // What the model was told after the first run: the failure, in full.
    let first_run = tool_contents(&rounds[1]).pop().expect("the run was reported");
    assert!(first_run.contains("expected 42, got 41"), "{first_run}");
    assert!(first_run.starts_with("Exit code 1 after "), "{first_run}");
    // And after the second: the pass.
    let second_run = tool_contents(rounds.last().unwrap()).pop().expect("reported");
    assert!(second_run.contains("PASS"), "{second_run}");
    assert!(second_run.starts_with("Exit code 0 after "), "{second_run}");
}

/// Output reaches the UI while the command is still running, tagged with
/// the call it belongs to — otherwise a two-minute build is two minutes of
/// nothing.
#[test]
fn command_output_is_reported_as_it_is_produced() {
    let h = harness(
        "cmd-stream",
        vec![
            asks(vec![wants(
                "c1",
                "runCommand",
                r#"{"command":"echo working; echo trouble >&2"}"#,
            )]),
            text("done"),
        ],
    );

    h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");

    let chunks: Vec<(String, String)> = h
        .events()
        .into_iter()
        .filter_map(|e| match e.event {
            ChatEventPayload::CommandOutput { id, chunk, .. } => Some((id, chunk)),
            _ => None,
        })
        .collect();
    assert!(!chunks.is_empty(), "nothing was reported while it ran");
    assert!(chunks.iter().all(|(id, _)| id == "c1"), "tagged by call");
    let text: String = chunks.iter().map(|(_, chunk)| chunk.clone()).collect();
    assert!(text.contains("working") && text.contains("trouble"), "{text}");
}

/// An offer runs nothing, so it asks nothing: the turn ends with the
/// answer, not on an approval card — in any approval mode.
#[test]
fn an_offer_asks_nothing_and_the_turn_ends() {
    let mut h = harness(
        "offer-no-approval",
        vec![asks(vec![wants("o1", "offer", r#"{"kind":"commit","reason":"the counter"}"#)]), text("done")],
    );
    h.approval = asking();
    git2::Repository::init(&h.root).unwrap();
    std::fs::write(h.root.join("a.rs"), "x").unwrap();

    let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("write the counter")], vec![])).expect("turn");

    assert!(matches!(outcome, ChatStreamOutcome::Done(_)));
    assert!(tool_contents(&h.provider.requests()[1]).iter().any(|c| c.starts_with("Offered")));
}

/// Implement needs a plan, and a turn that wrote one has it for the
/// `offer` in the same round: the calls run in the order asked.
#[test]
fn a_plan_written_in_the_same_round_can_be_offered() {
    let implement = r#"{"kind":"implement","reason":"the parser"}"#;
    let h = harness(
        "offer-implement-same-round",
        vec![asks(vec![wants("p", "writePlan", r##"{"content":"# Plan"}"##), wants("o", "offer", implement)]), text("done")],
    );
    h.run(|turn| stream(turn, vec![LlmMessage::user("plan it")], vec![])).expect("turn");
    assert!(tool_contents(&h.provider.requests()[1]).iter().any(|c| c.starts_with("Offered")));
}

/// Reads run alongside each other with only the window's plan to go on;
/// an `offer` among them would not see the one this turn wrote.
#[test]
fn an_offer_after_a_read_sees_the_plan_this_turn_wrote() {
    let implement = r#"{"kind":"implement","reason":"the parser"}"#;
    let h = harness(
        "offer-implement-after-read",
        vec![
            asks(vec![wants("p", "writePlan", r##"{"content":"# Plan"}"##)]),
            asks(vec![wants("l", "listFiles", "{}"), wants("o", "offer", implement)]),
            text("done"),
        ],
    );
    h.run(|turn| stream(turn, vec![LlmMessage::user("plan it")], vec![])).expect("turn");
    assert!(tool_contents(&h.provider.requests()[2]).iter().any(|c| c.starts_with("Offered")));
}

/// The plan the window holds counts, but one emptied there does not.
#[test]
fn the_windows_plan_can_be_offered_unless_it_is_blank() {
    let implement = r#"{"kind":"implement","reason":"the parser"}"#;
    let said = |label: &str, plan: Option<&str>| {
        let mut h = harness(label, vec![asks(vec![wants("o", "offer", implement)]), text("done")]);
        h.plan = plan.map(str::to_string);
        h.run(|turn| stream(turn, vec![LlmMessage::user("go on")], vec![])).expect("turn");
        tool_contents(&h.provider.requests()[1]).concat()
    };
    assert!(said("offer-implement-window", Some("# Plan")).starts_with("Offered"));
    for (label, plan) in [("offer-implement-none", None), ("offer-implement-blank", Some(" \n"))] {
        assert!(said(label, plan).contains("no plan"), "{plan:?}");
    }
}

/// A command line is not a tool name: `ls` and `rm -rf /` are the same
/// call. Until the command itself is examined, every one of them asks.
#[test]
fn a_command_needs_approval_like_any_other_change() {
    let mut h = harness(
        "cmd-approval",
        vec![asks(vec![wants("c1", "runCommand", r#"{"command":"rm -rf ."}"#)])],
    );
    h.approval = asking();
    std::fs::write(h.root.join("keep.txt"), "x").unwrap();

    let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("clean up")], vec![]));

    let ChatStreamOutcome::PendingApproval(pending) = outcome.expect("pauses") else {
        panic!("expected a pause");
    };
    assert!(pending.calls[0].requires_confirmation);
    assert!(h.root.join("keep.txt").exists(), "nothing ran");
}

/// The language setting reaches the model with the session: every
/// request of the turn is told it.
#[test]
fn the_session_s_reply_language_is_in_the_request() {
    let mut h = harness("chat-language", vec![text("готово")]);
    h.session.reply_language = Some("Russian");
    h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");
    let told = h.provider.requests()[0]
        .messages
        .iter()
        .any(|m| m.content.as_deref().is_some_and(|c| c.contains("Write everything you say in Russian")));
    assert!(told, "the language block is in the request");
}

/// Every note the loop adds ends by saying the language again: the
/// model reads it just before it replies, where the system prompt at the
/// top of a long English history no longer held (`/review`, Chinese).
#[test]
fn each_note_of_the_loop_repeats_the_reply_language() {
    const SAID: &str = "\n\n[Reply in Russian.]";
    let russian = |mut h: Harness| {
        h.session.reply_language = Some("Russian");
        h
    };
    let notes_in = |h: &Harness, starts: &str| -> Vec<String> {
        h.provider.requests().last().unwrap().messages.iter()
            .filter(|m| m.role == LlmRole::User)
            .filter_map(|m| m.content.clone())
            .filter(|c| c.starts_with(starts))
            .collect()
    };

    let empty = russian(harness("lang-empty", vec![text(""), text("ok")]));
    empty.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");
    assert_eq!(notes_in(&empty, "[Your last reply was empty"), [format!("{EMPTY_REPLY_NOTE}{SAID}")]);

    let circling = (1..=5).map(|n| asks(vec![wants(&format!("l{n}"), "listFiles", "{}")])).chain([text("done")]).collect();
    let circling = russian(harness("lang-loop", circling));
    circling.run(|turn| stream(turn, vec![LlmMessage::user("look")], vec![])).expect("turn");
    let loop_notes = notes_in(&circling, "");
    assert!(loop_notes.iter().any(|c| c.ends_with(SAID) && c.contains("listFiles")), "{loop_notes:?}");

    let mut ended = russian(harness("lang-ended", vec![text("I see")]));
    ended.processes = Some(Arc::new(EndedOnce(Mutex::new(vec![crate::domain::background::ProcessInfo {
        id: 2,
        command: "npm run dev".into(),
        cwd: ".".into(),
        state: crate::domain::background::ProcessState::Exited { code: Some(1) },
    }]))));
    ended.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");
    assert!(notes_in(&ended, "[Background processes")[0].ends_with(SAID));

    let mut review = russian(weighing("lang-wrap-up", 2));
    review.mode = ConversationMode::Review;
    review.run(|turn| stream(turn, vec![LlmMessage::user("review")], vec![])).expect("turn");
    assert!(notes_in(&review, "[Review budget]")[0].ends_with(SAID));

    let (stopping, _) = hooked(
        russian(harness("lang-stop-hook", vec![text("done"), text("tests pass, done")])),
        hook("Stop", "", "check"),
        |_, input| if input["stop_hook_active"] == true { (0, "") } else { (2, "run the tests first") },
    );
    stopping.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");
    assert!(notes_in(&stopping, "[A Stop hook")[0].ends_with(SAID));

    // Auto says nothing more than the note itself.
    let auto = harness("lang-auto", vec![text(""), text("ok")]);
    auto.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");
    assert_eq!(notes_in(&auto, "[Your last reply was empty"), [EMPTY_REPLY_NOTE.to_string()]);
}

/// Rounds of distinct cheap reads, and then an answer.
pub(super) fn reading(label: &str, rounds: u32) -> Harness {
    let mut steps: Vec<Step> =
        (0..rounds).map(|n| asks(vec![wants(&format!("l{n}"), "listFiles", &format!(r#"{{"path":"d{n}"}}"#))])).collect();
    steps.push(text("done"));
    harness(label, steps)
}

/// Rounds whose calls weigh 102 each: two use four fifths of a budget of
/// 250, pinned here rather than read from the default so the arithmetic
/// holds when the default moves.
pub(super) fn weighing(label: &str, rounds: u32) -> Harness {
    let heavy = |n: u32| asks((0..34).map(|i| wants(&format!("g{n}-{i}"), "grep", &format!(r#"{{"pattern":"p{n}x{i}"}}"#))).collect());
    let mut steps: Vec<Step> = (0..rounds).map(heavy).collect();
    steps.push(text("done"));
    let mut h = harness(label, steps);
    h.session.limits = crate::domain::settings::TurnLimits { rounds: 60, budget: 250, parallel: 3 };
    h
}

pub(super) fn wrap_ups(h: &Harness) -> Vec<String> {
    payloads(&h.log.lock().unwrap()).into_iter().filter(|p| p.starts_with("wrap-up")).collect()
}

/// A review has an ordinary turn's ceilings, and is asked once to wrap up
/// with a fifth of the rounds left — after its 80th of 100 by default —
/// and an agent's turn never is.
#[test]
fn a_long_review_is_asked_once_to_wrap_up_and_an_agent_turn_never() {
    let limit = crate::domain::settings::TurnLimits::default().rounds * crate::domain::review::WRAP_UP_AT_PERCENT / 100;
    let mut review = reading("chat-wrap-up-review", limit + 2);
    review.mode = ConversationMode::Review;
    review.run(|turn| stream(turn, vec![LlmMessage::user("review")], vec![])).expect("turn");
    assert_eq!(wrap_ups(&review), [format!("wrap-up:{limit}")], "once, after the {limit}th round");
    let notes = review.provider.requests().last().unwrap().messages.iter().filter(|m| m.content.as_deref().is_some_and(|c| c.starts_with("[Review budget]"))).count();
    assert_eq!(notes, 1, "in the history the model reads");

    let agent = reading("chat-wrap-up-agent", limit + 2);
    agent.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");
    assert!(wrap_ups(&agent).is_empty());
}

/// The weighted budget counts as well as rounds: a review of expensive
/// calls is asked sooner — after its second round here, 204 of 250 used.
/// Tokens alone no longer do: an ordinary turn has no token ceiling.
#[test]
fn a_heavy_review_is_asked_to_wrap_up_by_its_budget() {
    let mut h = weighing("chat-wrap-up-weight", 2);
    h.mode = ConversationMode::Review;
    h.run(|turn| stream(turn, vec![LlmMessage::user("review")], vec![])).expect("turn");
    assert_eq!(wrap_ups(&h), ["wrap-up:2"]);

    let mut one = weighing("chat-wrap-up-weight-one", 1);
    one.mode = ConversationMode::Review;
    one.run(|turn| stream(turn, vec![LlmMessage::user("review")], vec![])).expect("turn");
    assert!(wrap_ups(&one).is_empty(), "102 of 250 is not yet");
}

/// The review card's output carries the whole story: the reviewer's
/// steps, the budget note when one was given, and a closing line saying
/// what was read and what came of it — no chat notice needed.
#[test]
fn the_review_card_output_tells_what_it_read_and_spent() {
    let heavy = |n: u32| asks((0..34).map(|i| wants(&format!("g{n}-{i}"), "grep", &format!(r#"{{"pattern":"p{n}x{i}"}}"#))).collect());
    let mut h = harness(
        "card-wrap-up",
        vec![asks(vec![wants("r1", "review", "{}")]), found("f1"), heavy(1), heavy(2), text("One real problem."), text("Will fix it.")],
    );
    git2::Repository::init(&h.root).unwrap();
    std::fs::write(h.root.join("a.rs"), "let x = y / 0;\n").unwrap();
    h.session.limits = crate::domain::settings::TurnLimits { rounds: 60, budget: 250, parallel: 3 };
    h.run(|turn| stream(turn, vec![], vec![])).expect("turn");

    let chunks: Vec<String> = h
        .events()
        .into_iter()
        .filter_map(|e| match e.event {
            ChatEventPayload::CommandOutput { id, chunk, .. } if id == "r1" => Some(chunk),
            _ => None,
        })
        .collect();
    let output = chunks.join("");
    assert!(output.contains("reportFinding a.rs\n"), "the reviewer's steps are the card's lines: {output}");
    assert!(output.contains("[Review budget] This review has used 82% of its budget"), "the budget note is a line of the card: {output}");
    assert!(output.contains("· HEAD → working tree · 1 file reviewed — 1 finding"), "and a closing line says what was read: {output}");
    assert!(!output.contains("ran out of rounds"), "the review finished inside its budget: {output}");
}

/// What a review runs with: the brief, the desk and the range, built from
/// a real change in `root` the way `run_review_tool` builds them.
pub(super) fn review_parts(root: &std::path::Path) -> (String, Arc<crate::domain::review::ReviewDesk>, crate::domain::review::ReviewRange) {
    let files = crate::infra::git_changes::review_diffs(root, None, None).unwrap();
    let (kept, excluded) = crate::domain::review::select(files);
    let range = crate::domain::review::ReviewRange {
        root: root.display().to_string(),
        from: "HEAD".into(),
        to: "working tree".into(),
        files: kept.len(),
        excluded,
    };
    let brief = crate::domain::review::review_message(&kept, &range, None, None);
    (brief, Arc::new(crate::domain::review::ReviewDesk::new(kept)), range)
}

/// A range the caller named with nothing between its ends is told apart
/// from a clean tree: the ends met, and only the caller can widen them.
#[test]
fn a_named_empty_range_says_so() {
    let h = harness("tool-review-empty-range", vec![]);
    git2::Repository::init(&h.root).unwrap();
    std::fs::write(h.root.join("a.rs"), "one\n").unwrap();
    let repo = git2::Repository::open(&h.root).unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(std::path::Path::new("a.rs")).unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let sig = git2::Signature::now("Test", "test@example.com").unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "first", &tree, &[]).unwrap();
    let err = h.run(|turn| {
        let progress = command_output_sink(turn.events, 0, "r1");
        run_review_tool(
            turn,
            &crate::domain::tools::ReviewArgs { from: Some("HEAD".into()), to: Some("HEAD".into()), focus: None },
            &progress,
        )
    })
    .unwrap_err();
    assert!(err.to_string().contains("the range is empty — its ends match"), "{err}");
    assert!(!err.to_string().contains("working tree matches HEAD"), "{err}");
}

/// Stopped mid-run, a review still reports what it had marked: the
/// findings are the result, the result says why there is no closing
/// answer, and the card's line says it was cut short. The cancel lands
/// at the second round's checkpoint: the first round's `reportFinding`
/// has run, the second has not.
#[test]
fn a_stopped_review_keeps_its_findings() {
    let h = harness("review-partial-stop", vec![found("f1"), text("Never mind.")]);
    h.cancel_at_poll(4);
    git2::Repository::init(&h.root).unwrap();
    std::fs::write(h.root.join("a.rs"), "let x = y / 0;\n").unwrap();
    let (brief, desk, range) = review_parts(&h.root);

    let result = h.run(|turn| {
        let progress = command_output_sink(turn.events, 0, "r1");
        run_review(turn, &brief, desk, range, &progress)
    });

    let result = result.expect("a review result, never an error now");
    let ToolResult::Reviewed { findings, text, .. } = result else { panic!("expected a review result") };
    assert_eq!(findings.len(), 1, "what was marked before the stop is kept: {findings:?}");
    assert_eq!(text, "(The review was stopped before it finished.)");
    let line: String = h
        .events()
        .into_iter()
        .filter_map(|e| match e.event {
            ChatEventPayload::CommandOutput { id, chunk, .. } if id == "r1" => Some(chunk),
            _ => None,
        })
        .collect();
    assert!(line.contains("— 1 finding — stopped before it finished"), "{line}");
}

/// The round ceiling cuts a review off the same way: the findings are
/// the result, marked with the reason — the cap only bites while the
/// review still has calls to make.
#[test]
fn a_review_out_of_rounds_keeps_its_findings() {
    let mut h = harness("review-partial-rounds", vec![found("f1"), text("Never answered.")]);
    h.session.limits = crate::domain::settings::TurnLimits { rounds: 1, budget: 250, parallel: 3 };
    git2::Repository::init(&h.root).unwrap();
    std::fs::write(h.root.join("a.rs"), "let x = y / 0;\n").unwrap();
    let (brief, desk, range) = review_parts(&h.root);

    let result = h.run(|turn| {
        let progress = command_output_sink(turn.events, 0, "r1");
        run_review(turn, &brief, desk, range, &progress)
    });

    let result = result.expect("a review result, never an error now");
    let ToolResult::Reviewed { findings, text, .. } = result else { panic!("expected a review result") };
    assert_eq!(findings.len(), 1, "the finding reported before the ceiling is kept: {findings:?}");
    assert_eq!(text, "(The review ran out of rounds before it finished.)");
    let line: String = h
        .events()
        .into_iter()
        .filter_map(|e| match e.event {
            ChatEventPayload::CommandOutput { id, chunk, .. } if id == "r1" => Some(chunk),
            _ => None,
        })
        .collect();
    assert!(line.contains("— 1 finding — ran out of rounds before it finished"), "{line}");
}
