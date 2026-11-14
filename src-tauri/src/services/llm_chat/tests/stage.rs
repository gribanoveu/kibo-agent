use super::*;

// --------------------------------------------------- the stage's own bar

/// What stage one set out to produce: a turn that reads a file, changes
/// it, and hands the model back what actually landed on disk — not just
/// `{"path": "…"}`, which tells it nothing about whether the edit took.
#[test]
fn a_turn_reads_a_file_edits_it_and_is_told_what_changed() {
    let h = harness(
        "stage-one",
        vec![
            asks(vec![wants("r1", "readFile", r#"{"path":"lib.rs"}"#)]),
            asks(vec![wants(
                "e1",
                "editFile",
                r#"{"path":"lib.rs","edits":[{"old":"one","new":"two"}]}"#,
            )]),
            text("renamed it"),
        ],
    );
    std::fs::write(h.root.join("lib.rs"), "fn one() {}\n").unwrap();

    let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("rename one to two")], vec![]));

    let ChatStreamOutcome::Done(done) = outcome.expect("finishes") else {
        panic!("not done")
    };
    // What the next message is sent with: the calls and what they
    // returned, not only the answer.
    let shape: Vec<(LlmRole, usize)> = done.history.iter().map(|m| (m.role, m.tool_calls.len())).collect();
    assert_eq!(
        shape,
        [
            (LlmRole::User, 0),
            (LlmRole::Assistant, 1),
            (LlmRole::Tool, 0),
            (LlmRole::Assistant, 1),
            (LlmRole::Tool, 0),
            (LlmRole::Assistant, 0),
        ]
    );
    assert_eq!(done.history[2].content.as_deref().map(|c| c.starts_with("All 1 lines:")), Some(true));
    assert_eq!(done.history.last().and_then(|m| m.content.as_deref()), Some("renamed it"));
    assert_eq!(
        std::fs::read_to_string(h.root.join("lib.rs")).unwrap(),
        "fn two() {}\n"
    );
    let told = tool_contents(h.provider.requests().last().unwrap());
    let edit = told.last().expect("the edit was reported");
    // Plain text, not a JSON string of escapes: the counts, then the diff.
    assert!(edit.starts_with("Edited lib.rs (+1 -1 lines)\n```diff\n"), "{edit}");
    assert!(edit.contains("\n-fn one"), "no diff itself: {edit}");
}

/// A git diff reaches the model the way a write does: a line, then the
/// diff as a diff — not a JSON string of `\n` escapes.
#[test]
fn a_git_diff_reaches_the_model_as_a_diff() {
    let h = harness(
        "loop-git-diff",
        vec![asks(vec![wants("d1", "gitDiff", r#"{"path":"a.md"}"#)]), text("seen")],
    );
    std::fs::write(h.root.join("a.md"), "one\n").unwrap();
    let repo = git2::Repository::init(&h.root).unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(std::path::Path::new("a.md")).unwrap();
    index.write().unwrap();
    std::fs::write(h.root.join("a.md"), "uno\n").unwrap();

    h.run(|turn| stream(turn, vec![LlmMessage::user("what changed")], vec![]))
        .expect("finishes");

    let told = tool_contents(h.provider.requests().last().unwrap());
    assert!(told[0].starts_with("Diff (index → working tree): a.md (+1 -1 lines)\n```diff\n"), "{}", told[0]);
    assert!(told[0].contains("\n-one\n+uno\n"), "{}", told[0]);
}

/// The turn hands the tool the folder's search, with the model's
/// reordering on top (`search_rerank`); without it the model gets
/// "unavailable" and a pointer to grep.
#[test]
fn a_turn_searches_through_the_search_it_was_given() {
    use crate::domain::code_search::{CodeMatch, CodeSearchResult, MatchSource, SearchMeta};
    let mut h = harness(
        "turn-search",
        vec![asks(vec![wants("s1", "semanticSearch", r#"{"query":"where sync lives"}"#)]), text("found it")],
    );
    h.search = Some(Arc::new(|_: &[&str], _: Option<&[String]>, _: usize, _: &crate::domain::code_search::SearchFilter| {
        let at = |path: &str| CodeMatch {
            path: path.into(),
            start_line: 1,
            end_line: 2,
            name: None,
            text: "fn sync() {}".into(),
            source: MatchSource::Lexical,
        };
        Ok(CodeSearchResult {
            matches: vec![at("src/sync.rs"), at("src/other.rs")],
            meta: SearchMeta { tiers_used: vec![MatchSource::Lexical], weak: false, hint: None },
        })
    }));

    h.run(|turn| stream(turn, vec![LlmMessage::user("where is sync")], vec![])).expect("finishes");

    let told = tool_contents(h.provider.requests().last().unwrap());
    assert!(told[0].contains("src/sync.rs"), "{told:?}");
    let ranked = h.provider.summaries.lock().unwrap();
    let shown = ranked.iter().find_map(|r| r.messages.get(1)?.content.clone()).unwrap_or_default();
    assert!(shown.starts_with("Question: where sync lives") && shown.contains("[2] src/other.rs"), "{shown}");
}

/// The catalog the turn was given is what every request lists, and what
/// the tool loads from — the model can load only what it was told exists.
#[test]
fn every_request_lists_the_turns_skills_and_the_tool_loads_them() {
    crate::testing::with_app_dir("turn-skills", || {
        crate::infra::skills_store::test_support::write_skill("release", "Cuts a release.", "Bump the version.");
        let mut h = harness(
            "turn-skills",
            vec![asks(vec![wants("k1", "skill", r#"{"name":"release"}"#)]), text("done")],
        );
        h.skills = vec![Skill {
            meta: crate::domain::skills::SkillMeta { name: "release".into(), description: "Cuts a release.".into() },
            dir: crate::infra::skills_store::dir().unwrap().join("release"),
        }];

        h.run(|turn| stream(turn, vec![LlmMessage::user("ship it")], vec![])).expect("finishes");

        let requests = h.provider.requests();
        let told = tool_contents(requests.last().unwrap());
        assert!(told[0].contains("Bump the version."), "{told:?}");
        assert_requests_list_release(requests);
    });
}

/// A plan written in the turn is the current one from then on: the plan
/// the window held when the turn began is not put back after the call.
#[test]
fn a_plan_written_in_the_turn_is_not_overruled_by_the_one_it_began_with() {
    let mut h = harness(
        "turn-plan-rewritten",
        vec![
            asks(vec![wants("p", "writePlan", r##"{"content":"# New"}"##)]),
            asks(vec![wants("l", "listFiles", "{}")]),
            text("done"),
        ],
    );
    h.plan = Some("# Old".into());

    h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");

    let last = h.provider.requests().last().unwrap().clone();
    let noted: Vec<&LlmMessage> = conversation_of(&last).iter().filter(|m| mentions_plan(m)).collect();
    assert_eq!(noted.len(), 1, "only the plan the turn began with, before the call: {noted:?}");
    assert!(noted[0].content.as_deref().unwrap().contains("<plan>\n# Old\n</plan>"));
}

/// The plan as the window holds it reaches every request through the
/// history, never the prompt: an edit to it leaves the cached prefix alone.
#[test]
fn every_request_carries_the_plan_as_the_window_holds_it() {
    let mut h = harness("turn-plan", vec![asks(vec![wants("r1", "listFiles", r#"{"path":"."}"#)]), text("done")]);
    h.plan = Some("# Fix\n\n1. edited by the user".into());

    h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");

    let requests = h.provider.requests();
    for request in &requests {
        let carries = |m: &LlmMessage| m.content.as_deref().is_some_and(|c| c.contains("1. edited by the user"));
        assert_eq!(conversation_of(request).iter().filter(|m| carries(m)).count(), 1, "a request went out without the plan, or with two");
        assert!(!request.messages.iter().any(|m| m.role == LlmRole::System && carries(m)), "the plan is in the prompt");
    }
    assert!(requests[1].messages.starts_with(&requests[0].messages), "a request is a prefix of the next");
}

#[test]
fn every_request_in_a_worktree_names_the_checkout_to_keep_out_of() {
    let mut h = harness("turn-worktree", vec![asks(vec![wants("r1", "listFiles", r#"{"path":"."}"#)]), text("done")]);
    h.worktree_of = Some(PathBuf::from("/work/the-main-checkout"));

    h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes");

    for request in h.provider.requests() {
        assert!(
            request.messages.iter().any(|m| m.content.as_deref().is_some_and(|c| c.contains("git worktree of /work/the-main-checkout"))),
            "a request went out without saying it is a worktree"
        );
    }
}

#[test]
fn every_request_carries_the_project_instructions() {
    let mut h = harness("turn-rules", vec![asks(vec![wants("r1", "listFiles", r#"{"path":"."}"#)]), text("done")]);
    h.rules = vec![RuleFile::new("AGENTS.md", "Run cargo test before saying done.")];

    h.run(|turn| stream(turn, vec![LlmMessage::user("fix it")], vec![])).expect("finishes");

    let requests = h.provider.requests();
    assert_eq!(requests.len(), 2);
    for request in requests {
        assert!(
            request.messages.iter().any(|m| m.content.as_deref().is_some_and(|c| c.contains("Run cargo test before saying done."))),
            "a request went out without the project instructions"
        );
    }
}

pub(super) fn system_says(request: &ChatRequest, text: &str) -> bool {
    request.messages.iter().any(|m| m.role == LlmRole::System && m.content.as_deref().is_some_and(|c| c.contains(text)))
}

pub(super) fn notes_of(history: &[LlmMessage], name: &str) -> Vec<String> {
    let head = format!("{STANDING_NOTE} {name}.");
    history.iter().filter_map(|m| m.content.as_deref()).filter(|c| c.contains(&head)).map(str::to_string).collect()
}

pub(super) fn done_history(outcome: ChatStreamOutcome) -> Vec<LlmMessage> {
    match outcome {
        ChatStreamOutcome::Done(done) => done.history,
        other => panic!("not done: {other:?}"),
    }
}

/// A file edited mid-chat leaves the prompt as the chat began — the cache
/// holds it — and reaches the model once, as a note the next turn keeps.
#[test]
fn a_changed_file_is_a_note_once_and_the_prompt_stays_as_the_chat_began() {
    let mut h = harness("standing-changed", vec![asks(vec![wants("r1", "listFiles", r#"{"path":"."}"#)]), text("done"), text("again")]);
    h.rules = vec![RuleFile::new("AGENTS.md", "Run bun test.")];
    h.frozen = Some(Frozen { rules: vec![RuleFile::new("AGENTS.md", "Run cargo test.")], parallel: None });

    let first = done_history(h.run(|turn| stream(turn, vec![LlmMessage::user("fix it")], vec![])).expect("finishes"));
    let mut history = first.clone();
    history.push(LlmMessage::user("and the rest"));
    let second = done_history(h.run(|turn| stream(turn, history, vec![])).expect("finishes"));

    let requests = h.provider.requests();
    assert_eq!(requests.len(), 3);
    for request in &requests {
        assert!(system_says(request, "Run cargo test.") && !system_says(request, "Run bun test."), "the prompt moved");
    }
    let notes = notes_of(&second, "Project instructions");
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].contains("### AGENTS.md\n\nRun bun test."), "{}", notes[0]);
    assert!(notes_of(&second, "Memory").is_empty(), "only what changed is said");
    assert!(requests[0].messages.iter().any(|m| m.content.as_deref().is_some_and(|c| c.contains("Run bun test."))), "the first request is told");
    // Each request extends the last: the note sits where the first turn put it.
    assert_eq!(&second[..first.len()], &first[..]);
}

#[test]
fn parallel_sessions_change_in_notes_not_in_the_frozen_prompt() {
    let first = "first".to_string();
    let second = "second".to_string();
    let mut h = harness("standing-parallel", vec![text("one"), text("two"), text("three"), text("four")]);
    h.parallel = vec![first.clone()];
    h.frozen = Some(Frozen { rules: Vec::new(), parallel: prompt::parallel_block(&h.parallel) });

    let one = done_history(h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes"));
    h.parallel = vec![second.clone()];
    let mut history = one;
    history.push(LlmMessage::user("next"));
    let two = done_history(h.run(|turn| stream(turn, history, vec![])).expect("finishes"));
    let notes = notes_of(&two, "Parallel sessions");
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].contains("chat \"second\""), "{}", notes[0]);

    h.parallel.clear();
    let mut history = two;
    history.push(LlmMessage::user("next"));
    let three = done_history(h.run(|turn| stream(turn, history, vec![])).expect("finishes"));
    let notes = notes_of(&three, "Parallel sessions");
    assert_eq!(notes.len(), 2, "{notes:?}");
    assert!(notes[1].contains("Gone: the section no longer applies."), "{}", notes[1]);

    let mut history = three;
    history.push(LlmMessage::user("next"));
    let four = done_history(h.run(|turn| stream(turn, history, vec![])).expect("finishes"));
    assert_eq!(notes_of(&four, "Parallel sessions").len(), 2, "an unchanged absence is not repeated");
    for request in h.provider.requests() {
        assert!(system_says(&request, "chat \"first\""), "the frozen neighbour disappeared");
        assert!(!system_says(&request, "chat \"second\""), "the system prompt followed the live sessions");
    }
}

/// Files as the chat began say nothing; a file put back after a note
/// said otherwise is a change again.
#[test]
fn a_note_follows_the_files_back_and_nothing_is_said_without_a_change() {
    let rules = vec![RuleFile::new("AGENTS.md", "Run cargo test.")];
    let mut h = harness("standing-back", vec![text("done"), text("done"), text("done")]);
    h.rules = rules.clone();
    h.frozen = Some(Frozen { rules: rules.clone(), parallel: None });

    let quiet = done_history(h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes"));
    assert!(notes_of(&quiet, "Project instructions").is_empty(), "{quiet:?}");

    let said = standing_note("Project instructions", Some(&prompt::rules_block(&[RuleFile::new("AGENTS.md", "Run bun test.")]).unwrap()));
    let back = done_history(h.run(|turn| stream(turn, vec![LlmMessage::user("go"), LlmMessage::user(said), LlmMessage::user("on")], vec![])).expect("finishes"));
    let notes = notes_of(&back, "Project instructions");
    assert_eq!(notes.len(), 2, "{notes:?}");
    assert!(notes[1].contains("Run cargo test."), "{}", notes[1]);

    // The latest note is what the model believes, not the first one.
    let mut settled = back.clone();
    settled.push(LlmMessage::user("more"));
    let still = done_history(h.run(|turn| stream(turn, settled, vec![])).expect("finishes"));
    assert_eq!(notes_of(&still, "Project instructions").len(), 2, "said again with nothing changed");
}

/// A file cut short is a change, though what it says now begins what the
/// last note said.
#[test]
fn a_shortened_file_is_not_taken_for_the_longer_note() {
    let mut h = harness("standing-shortened", vec![text("done")]);
    h.rules = vec![RuleFile::new("AGENTS.md", "Run bun test.")];
    h.frozen = Some(Frozen { rules: vec![RuleFile::new("AGENTS.md", "Run cargo test.")], parallel: None });
    let longer = standing_note("Project instructions", Some(&prompt::rules_block(&[RuleFile::new("AGENTS.md", "Run bun test.\n\nThen lint.")]).unwrap()));

    let history = done_history(h.run(|turn| stream(turn, vec![LlmMessage::user(longer), LlmMessage::user("go")], vec![])).expect("finishes"));

    let notes = notes_of(&history, "Project instructions");
    assert_eq!(notes.len(), 2, "{notes:?}");
    assert!(!notes[1].contains("Then lint."), "{}", notes[1]);
}

/// A summary that folds the note away is followed by it again: the
/// prompt still says the files as the chat began.
#[test]
fn a_note_folded_away_comes_back_after_the_summary() {
    let mut h = harness(
        "standing-folded",
        vec![asks(vec![wants("r", "readFile", r#"{"path":"big.txt"}"#)]), Step::Fail(too_long_error()), text("done")],
    );
    h.session.context_limit = Some(90_000);
    h.rules = vec![RuleFile::new("AGENTS.md", "Run bun test.")];
    h.frozen = Some(Frozen { rules: vec![RuleFile::new("AGENTS.md", "Run cargo test.")], parallel: None });
    std::fs::write(h.root.join("big.txt"), "x".repeat(40_000)).unwrap();

    h.run(|turn| stream(turn, long_conversation(), vec![])).expect("turn");

    let requests = h.provider.requests();
    let says = |m: &LlmMessage| m.content.as_deref().is_some_and(|c| c.contains("Run bun test."));
    // With the project instructions the prompt is four messages, not the three `conversation_of` expects.
    let conversation = |request: &ChatRequest| request.messages.iter().filter(|m| m.role != LlmRole::System).cloned().collect::<Vec<_>>();
    let first = conversation(&requests[0]);
    let at = first.iter().position(says).expect("said before the fold");
    let retry = conversation(requests.last().unwrap());
    assert!(!retry.iter().any(|m| m.content == first[at - 1].content), "the note was not folded — this proves nothing");
    assert_eq!(retry.iter().filter(|m| says(m)).count(), 1, "{retry:?}");
}

/// A part of the prompt that is gone says so: a memory file removed
/// mid-chat is a note, not a prompt that changed silently.
#[test]
fn a_lost_memory_says_gone() {
    use crate::domain::project_rules::MEMORY_NAME;
    let mut h = harness("standing-gone", vec![text("done")]);
    h.frozen = Some(Frozen { rules: vec![RuleFile::new(MEMORY_NAME, "- k8s is OrbStack")], parallel: None });

    let history = done_history(h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("finishes"));

    let request = &h.provider.requests()[0];
    assert!(system_says(request, "k8s is OrbStack"), "the prompt moved");
    let memory = notes_of(&history, "Memory");
    assert!(memory.len() == 1 && memory[0].contains("Gone: the section no longer applies.\n[End of Memory.]"), "{memory:?}");
}

pub(super) fn assert_requests_list_release(requests: Vec<ChatRequest>) {
    assert_eq!(requests.len(), 2);
    for request in requests {
        assert!(
            request.messages.iter().any(|m| m.content.as_deref().is_some_and(|c| c.contains("- release: Cuts a release."))),
            "a request went out without the skills"
        );
    }
}

/// Every field of the checkpoint, exercised through a real pause rather
/// than by serializing a fixture: the checklist an earlier round built has
/// to come out the other side, along with the history, the ceilings, the
/// numbering and the read registry.
#[test]
fn the_whole_checkpoint_survives_a_real_pause() {
    let mut h = harness(
        "stage-one-checkpoint",
        vec![
            asks(vec![wants(
                "t1",
                "todo",
                r#"{"op":"write","tasks":["read it","rewrite it"]}"#,
            )]),
            asks(vec![wants("r1", "readFile", r#"{"path":"a.rs"}"#)]),
            asks(vec![wants(
                "w1",
                "writeFile",
                r#"{"path":"a.rs","content":"new"}"#,
            )]),
            text("done"),
        ],
    );
    h.approval = asking();
    std::fs::write(h.root.join("a.rs"), "old").unwrap();

    let ChatStreamOutcome::PendingApproval(pending) = h
        .run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![]))
        .expect("pauses")
    else {
        panic!("expected a pause");
    };

    assert_eq!(pending.round, 3, "the ceiling cannot be reset by pausing");
    assert!(pending.budget_used > 0);
    assert_eq!(pending.event_seq, h.events().last().unwrap().seq);
    assert_eq!(pending.calls.len(), 1);
    assert_eq!(
        pending.todos.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(),
        ["read it", "rewrite it"],
        "the checklist an earlier round built"
    );
    assert!(pending.history.len() > 1);
    assert!(pending.reads.check("a.rs", "old", true).is_ok());

    let outcome = h.run(|turn| {
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

    let ChatStreamOutcome::Done(done) = outcome.expect("finishes") else {
        panic!("expected Done");
    };
    assert_eq!(std::fs::read_to_string(h.root.join("a.rs")).unwrap(), "new");
    assert_eq!(done.todos.len(), 2, "and the checklist comes out with it");
}
