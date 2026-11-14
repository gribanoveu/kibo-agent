use super::*;

// ------------------------------------------------------ parallel explore

/// A provider for helpers running side by side: the calling turn gets
/// its script, each helper an answer made from its own task — whichever
/// order they arrive in — and every helper's request waits a moment for
/// others, so the most in flight at once is what the test reads.
pub(super) struct SideBySide {
    script: Mutex<VecDeque<ChatStreamResult>>,
    in_flight: Mutex<usize>,
    arrived: std::sync::Condvar,
    most: Mutex<usize>,
    helper_tasks: Mutex<Vec<String>>,
    /// A file each helper looks for as it starts: `(task, found)`.
    watch: Mutex<Option<PathBuf>>,
    saw: Mutex<Vec<(String, bool)>>,
}

impl SideBySide {
    fn new(script: Vec<ChatStreamResult>) -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::new(script.into()),
            in_flight: Mutex::new(0),
            arrived: std::sync::Condvar::new(),
            most: Mutex::new(0),
            helper_tasks: Mutex::new(Vec::new()),
            watch: Mutex::new(None),
            saw: Mutex::new(Vec::new()),
        })
    }
}

impl LlmProvider for SideBySide {
    fn chat(&self, _: ChatRequest) -> Result<ChatResponse, LlmError> {
        unreachable!("no compaction here")
    }

    fn chat_stream(
        &self,
        request: ChatRequest,
        _on_event: &dyn Fn(StreamEvent),
        _: &dyn Fn() -> bool,
    ) -> Result<ChatStreamResult, LlmError> {
        if request.tools.iter().any(|t| t.name == "explore") {
            return Ok(self.script.lock().unwrap().pop_front().expect("the calling turn's script ran out"));
        }
        let task = request.messages.iter().find(|m| m.role == LlmRole::User).and_then(|m| m.content.clone()).unwrap_or_default();
        self.helper_tasks.lock().unwrap().push(task.clone());
        if let Some(file) = self.watch.lock().unwrap().as_ref() {
            self.saw.lock().unwrap().push((task.clone(), file.exists()));
        }
        let mut in_flight = self.in_flight.lock().unwrap();
        *in_flight += 1;
        let mut most = self.most.lock().unwrap();
        *most = (*most).max(*in_flight);
        drop(most);
        self.arrived.notify_all();
        // Held for one more than the limit, so a helper past it is caught
        // in flight; the timeout is what lets a right-sized batch go.
        let (mut in_flight, _) = self
            .arrived
            .wait_timeout_while(in_flight, Duration::from_millis(400), |n| *n <= MAX_PARALLEL_CALLS)
            .unwrap();
        *in_flight -= 1;
        Ok(ChatStreamResult { text: format!("answer to {task}"), ..Default::default() })
    }

    fn list_models(&self) -> Result<Vec<LlmModelInfo>, LlmError> {
        unreachable!()
    }
}

pub(super) fn side_by_side(label: &str, asked: Vec<LlmToolCall>) -> (Harness, Arc<SideBySide>) {
    let provider = SideBySide::new(vec![
        ChatStreamResult { tool_calls: asked, ..Default::default() },
        ChatStreamResult { text: "done".into(), ..Default::default() },
    ]);
    let mut h = harness(label, vec![]);
    h.session.provider = provider.clone();
    (h, provider)
}

pub(super) fn explore(id: &str, task: &str) -> LlmToolCall {
    wants(id, "explore", &serde_json::json!({ "task": task }).to_string())
}

/// Two helpers asked one after another run at once, and their answers
/// come back in the order they were asked, before the call after them.
#[test]
fn explores_in_one_reply_run_at_once_and_answer_in_order() {
    let (h, provider) = side_by_side(
        "explore-parallel",
        vec![explore("e1", "A"), explore("e2", "B"), wants("r1", "readFile", r#"{"path":"a.rs"}"#)],
    );
    std::fs::write(h.root.join("a.rs"), "fn a() {}").unwrap();

    let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");
    let ChatStreamOutcome::Done(done) = outcome else { panic!("expected done") };

    assert_eq!(*provider.most.lock().unwrap(), 2, "both helpers were in flight together");
    let results: Vec<(String, String)> = done
        .history
        .iter()
        .filter(|m| m.role == LlmRole::Tool)
        .map(|m| (m.tool_call_id.clone().unwrap(), m.content.clone().unwrap()))
        .collect();
    assert_eq!(results.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(), ["e1", "e2", "r1"]);
    assert!(results[0].1.starts_with("answer to A\n\n"), "{:?}", results[0]);
    assert!(results[1].1.starts_with("answer to B\n\n"), "{:?}", results[1]);
    assert!(results[2].1.contains("fn a() {}"), "{:?}", results[2]);
    // Each card started once, not again when the loop took its result.
    let shown = payloads(&h.events());
    for id in ["e1", "e2", "r1"] {
        assert_eq!(shown.iter().filter(|p| **p == format!("toolCall:{id}")).count(), 1, "{id}: {shown:?}");
    }
}

/// The round keeps its order: helpers asked after a write find it
/// written, one asked before it does not, and a write between two
/// helpers keeps them apart.
#[test]
fn helpers_run_where_the_round_asked_for_them() {
    let write = |id: &str| wants(id, "writeFile", r#"{"path":"a.txt","content":"x"}"#);

    let (after, provider) = side_by_side("explore-after-write", vec![write("w1"), explore("e1", "A"), explore("e2", "B")]);
    *provider.watch.lock().unwrap() = Some(after.root.join("a.txt"));
    after.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");
    let mut saw = provider.saw.lock().unwrap().clone();
    saw.sort();
    assert_eq!(saw, [("A".to_string(), true), ("B".to_string(), true)]);
    assert_eq!(*provider.most.lock().unwrap(), 2);

    let (around, provider) = side_by_side("explore-around-write", vec![explore("e1", "A"), write("w1"), explore("e2", "B")]);
    *provider.watch.lock().unwrap() = Some(around.root.join("a.txt"));
    around.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");
    assert_eq!(*provider.saw.lock().unwrap(), [("A".to_string(), false), ("B".to_string(), true)]);
    assert_eq!(*provider.most.lock().unwrap(), 1, "not in a row, so one at a time");
}

/// Past the limit they go in batches: never more at once than that.
#[test]
fn no_more_helpers_run_at_once_than_the_limit() {
    let asked = (0..=MAX_PARALLEL_CALLS).map(|n| explore(&format!("e{n}"), &format!("T{n}"))).collect();
    let (h, provider) = side_by_side("explore-batches", asked);
    h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");
    assert_eq!(*provider.most.lock().unwrap(), MAX_PARALLEL_CALLS);
    assert_eq!(provider.helper_tasks.lock().unwrap().len(), MAX_PARALLEL_CALLS + 1, "every one ran");
}

/// The hooks are asked once for each, before any runs; the one refused
/// never starts, and the other still answers.
#[test]
fn a_hook_refusing_one_of_them_stops_that_one_only() {
    let (h, provider) = side_by_side("explore-parallel-hook", vec![explore("e1", "A"), explore("e2", "B")]);
    let (h, inputs) = hooked(h, hook("PreToolUse", "explore", "guard"), |_, input| {
        if input["tool_input"]["task"] == "B" { (2, "not that one") } else { (0, "") }
    });
    let outcome = h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");
    let ChatStreamOutcome::Done(done) = outcome else { panic!("expected done") };

    assert_eq!(inputs.lock().unwrap().len(), 2, "asked once each");
    assert_eq!(*provider.helper_tasks.lock().unwrap(), ["A"]);
    let results: Vec<String> = done.history.iter().filter(|m| m.role == LlmRole::Tool).filter_map(|m| m.content.clone()).collect();
    assert!(results[0].starts_with("answer to A\n\n"), "{results:?}");
    assert!(results[1].contains("not that one"), "{results:?}");
}

// ------------------------------------------------------ parallel reads

/// Reads in a row are all started before any answers, and answer in the
/// order they were asked; a write ends the batch, and a read after it
/// waits for it.
#[test]
fn reads_in_a_row_start_together_and_answer_in_order() {
    let h = harness(
        "reads-parallel",
        vec![
            asks(vec![
                wants("r1", "readFile", r#"{"path":"a.rs"}"#),
                wants("g1", "grep", r#"{"pattern":"fn"}"#),
                wants("r2", "readFile", r#"{"path":"b.rs"}"#),
                wants("w1", "writeFile", r#"{"path":"c.rs","content":"fn c() {}"}"#),
                wants("r3", "readFile", r#"{"path":"c.rs"}"#),
            ]),
            text("done"),
        ],
    );
    std::fs::write(h.root.join("a.rs"), "fn a() {}").unwrap();
    std::fs::write(h.root.join("b.rs"), "fn b() {}").unwrap();
    h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");

    let shown = payloads(&h.events());
    let at = |p: &str| shown.iter().position(|s| s == p).unwrap_or_else(|| panic!("{p}: {shown:?}"));
    for id in ["r1", "g1", "r2"] {
        assert!(at(&format!("toolCall:{id}")) < at("toolResult:r1"), "{id} started with the batch: {shown:?}");
    }
    assert!(at("toolResult:r2") < at("toolCall:w1"), "the write waits for the batch: {shown:?}");
    assert!(at("toolResult:w1") < at("toolCall:r3"), "a read after the write waits for it: {shown:?}");
    let results: Vec<&String> = shown.iter().filter(|s| s.starts_with("toolResult:")).collect();
    assert_eq!(results, ["toolResult:r1", "toolResult:g1", "toolResult:r2", "toolResult:w1", "toolResult:r3"]);

    let answered = tool_contents(&h.provider.requests()[1]);
    assert!(answered[0].contains("fn a() {}") && answered[2].contains("fn b() {}"), "{answered:?}");
    assert!(answered[4].contains("fn c() {}"), "the read after the write sees it: {answered:?}");
}

/// A file read in a batch counts as read for a write in a later round,
/// as it would had the reads run one after another.
#[test]
fn reads_beside_each_other_count_for_a_later_write() {
    let h = harness(
        "reads-parallel-then-write",
        vec![
            asks(vec![wants("r1", "readFile", r#"{"path":"a.txt"}"#), wants("r2", "readFile", r#"{"path":"b.txt"}"#)]),
            asks(vec![wants("w1", "writeFile", r#"{"path":"a.txt","content":"new\n"}"#)]),
            text("done"),
        ],
    );
    std::fs::write(h.root.join("a.txt"), "old\n").unwrap();
    std::fs::write(h.root.join("b.txt"), "b\n").unwrap();
    h.run(|turn| stream(turn, vec![LlmMessage::user("go")], vec![])).expect("turn");

    let answered = tool_contents(&h.provider.requests()[2]);
    assert!(!answered.last().unwrap().starts_with(TOOL_ERROR_PREFIX), "{answered:?}");
    assert_eq!(std::fs::read_to_string(h.root.join("a.txt")).unwrap(), "new\n");
}

/// Pages go at once, and each is checked against the conversation as it
/// stood: an address the user gave is one it may open.
#[test]
fn pages_are_fetched_at_once_from_the_conversation() {
    let mut h = harness(
        "fetch-parallel",
        vec![
            asks(vec![
                wants("f1", "webFetch", r#"{"url":"https://a.example/x"}"#),
                wants("f2", "webFetch", r#"{"url":"https://b.example/y"}"#),
            ]),
            text("done"),
        ],
    );
    let gate = Arc::new((Mutex::new((0usize, 0usize)), std::sync::Condvar::new()));
    let seen = Arc::clone(&gate);
    h.web = Some(Web {
        search: Arc::new(|_: &crate::domain::web_search::WebQuery| Ok(Vec::new())),
        fetch: Arc::new(move |q: &crate::domain::web_search::PageQuery| {
            let (lock, arrived) = &*seen;
            let mut counts = lock.lock().unwrap();
            counts.0 += 1;
            counts.1 = counts.1.max(counts.0);
            arrived.notify_all();
            let (mut counts, _) = arrived.wait_timeout_while(counts, Duration::from_millis(400), |c| c.0 < 2).unwrap();
            counts.0 -= 1;
            Ok(crate::domain::web_search::WebPage {
                title: String::new(),
                url: q.url.to_string(),
                content: format!("page {}", q.url),
                truncated: false,
            })
        }),
    });
    h.run(|turn| stream(turn, vec![LlmMessage::user("read https://a.example/x and https://b.example/y")], vec![]))
        .expect("turn");

    assert_eq!(gate.0.lock().unwrap().1, 2, "both pages were being read at once");
    let answered = tool_contents(&h.provider.requests()[1]);
    assert!(answered[0].contains("page https://a.example/x"), "{answered:?}");
    assert!(answered[1].contains("page https://b.example/y"), "{answered:?}");
}
