//! What a small local model is given to guess the user's next message, and
//! how a finished turn is turned into it (`docs/20-next-prompt-suggestions.md`).
//!
//! The model is trained elsewhere, on pairs cut from Claude Code transcripts
//! by `scripts/extract_next_prompt.py` in the minimind repository. **The input
//! format is shared with that script**, field by field: a model does not
//! generalize to inputs it never saw, so one field computed differently here
//! costs the quality of all of them. Everything below mirrors a function of
//! the script under the same name, and the golden string in the tests is the
//! one the script's `selftest` pins.

use std::collections::{BTreeSet, HashMap};
use std::sync::LazyLock;

use regex::Regex;

use crate::domain::command_exec::CommandRequest;
use crate::domain::conversation_mode::ConversationMode;
use crate::domain::tools::{Task, TodoStatus, ToolName, ToolResult};
use crate::domain::turn::{ChatEventPayload, ChatStreamOutcome, ToolResultEvent};

/// Characters kept from the tail of each part, as in the script.
pub const USER_MAX: usize = 500;
pub const AGENT_MAX: usize = 1500;
pub const OFFER_MAX: usize = 200;

// The script's regexes as they are. Constant patterns, so a failure to compile
// is a typo caught by the first test, not a runtime condition.
fn rx(pattern: &str) -> Regex {
    Regex::new(pattern).expect("a constant pattern compiles")
}

static TEST_RX: LazyLock<Regex> = LazyLock::new(|| {
    rx(r"\b(pytest|cargo test|bun test|go test|dotnet test|ctest|jest|vitest|unittest|(npm|yarn|pnpm)( run)? test|mvnw? test|gradlew? test)\b")
});
/// Test output that failed. The exit code is not enough: `cargo test | tail`
/// exits 0.
static TEST_FAIL_RX: LazyLock<Regex> = LazyLock::new(|| rx(r"\b[1-9]\d* (?:fail|failed|failing|failures?)\b|\bFAILED\b"));
static COMMIT_RX: LazyLock<Regex> = LazyLock::new(|| rx(r"\bgit\b[^|;&]*\bcommit\b"));
static IDENT_RX: LazyLock<Regex> = LazyLock::new(|| rx(r"`([^`\n]+)`|([A-Za-z_][\w./-]*\w)"));
static NAME_LIKE_RX: LazyLock<Regex> = LazyLock::new(|| rx(r"[_./\d]|[a-z][A-Z]"));
/// Where a reply splits into sentences. The script's `(?<=[.!?…])\s+|\n+`
/// without the lookbehind, which this engine lacks: only where the last match
/// ends matters, and that is the same.
static SENTENCE_END_RX: LazyLock<Regex> = LazyLock::new(|| rx(r"[.!?…]\s+|\n+"));

static CLEANUPS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    vec![
        (rx(r"(?s)<system-reminder>.*?</system-reminder>"), ""),
        (rx(r"(?s)<pasted_content[^>]*>.*?</pasted_content>"), "[вставка]"),
        (rx(r"^<!-- \w+ -->\n"), ""),
        (rx(r"(?m)^> .*\n?"), ""),
    ]
});
static SECRETS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    vec![
        (rx(r"sk-[A-Za-z0-9_\-]{20,}"), "<KEY>"),
        (rx(r"(ghp|gho|ghs|github_pat)_[A-Za-z0-9_]{20,}"), "<KEY>"),
        (rx(r"AKIA[0-9A-Z]{16}"), "<KEY>"),
        (rx(r"xox[baprs]-[A-Za-z0-9\-]{10,}"), "<KEY>"),
        (rx(r"eyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]+"), "<JWT>"),
        (
            rx(r#"(?i)((?:api[_-]?key|token|secret|password|passwd)["']?\s*[:=]\s*["']?|bearer\s+)[^\s"'`]{8,}"#),
            "${1}<SECRET>",
        ),
    ]
});

/// The last `n` characters — characters, not bytes, as Python slices.
fn tail(text: &str, n: usize) -> &str {
    match text.char_indices().rev().nth(n.saturating_sub(1)) {
        Some((i, _)) if n > 0 => &text[i..],
        _ if n == 0 => "",
        _ => text,
    }
}

/// The script's `clean`: wrappers dropped, the home folder shown as `~`, keys
/// and tokens replaced. The model never sees a secret, and what it sees reads
/// like what it was trained on. `home` is passed in: this layer reads nothing.
pub fn clean(text: &str, home: &str) -> String {
    let mut text = text.to_string();
    for (pattern, with) in CLEANUPS.iter() {
        text = pattern.replace_all(&text, *with).into_owned();
    }
    if !home.is_empty() {
        text = text.replace(home, "~");
    }
    for (pattern, with) in SECRETS.iter() {
        text = pattern.replace_all(&text, *with).into_owned();
    }
    text.trim().to_string()
}

/// The reply's closing sentence when it is a question, else empty.
pub fn offer(reply: &str) -> String {
    let reply = reply.trim();
    let start = SENTENCE_END_RX.find_iter(reply).last().map_or(0, |m| m.end());
    let last = reply[start..].trim();
    if last.ends_with('?') { tail(last, OFFER_MAX).to_string() } else { String::new() }
}

/// The model's input. Fields in a fixed order, every one always present.
pub fn model_input(mode: &str, outcome: &str, commit: bool, tests: &str, todo: &str, user: &str, reply: &str) -> String {
    format!(
        "<mode>{mode}<outcome>{outcome}<commit>{}<tests>{tests}<todo>{todo}<offer>{}\n<user>{}\n<agent>{}",
        u8::from(commit),
        offer(reply),
        tail(user, USER_MAX),
        tail(reply, AGENT_MAX),
    )
}

/// Code-like names: `backticked`, or words with `_ . /`, digits or camelCase.
pub fn names(text: &str) -> BTreeSet<String> {
    IDENT_RX
        .captures_iter(text)
        .filter_map(|c| match (c.get(1), c.get(2)) {
            (Some(tick), _) => Some(tick.as_str().to_string()),
            (None, Some(word)) if NAME_LIKE_RX.is_match(word.as_str()) => Some(word.as_str().to_string()),
            _ => None,
        })
        .collect()
}

/// Names a suggestion uses that its input never mentions. Non-empty means it
/// is not shown: a small model cannot know them, it invents them.
pub fn novel_names(suggestion: &str, input: &str) -> BTreeSet<String> {
    names(suggestion).into_iter().filter(|n| !input.contains(n.as_str())).collect()
}

/// `done/total` of the checklist, cancelled items left out; empty without one.
pub fn todo_progress(todos: &[Task]) -> String {
    let live: Vec<_> = todos.iter().filter(|t| t.status != TodoStatus::Cancelled).collect();
    if live.is_empty() {
        return String::new();
    }
    let done = live.iter().filter(|t| t.status == TodoStatus::Completed).count();
    format!("{done}/{}", live.len())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tests {
    #[default]
    None,
    Pass,
    Fail,
}

impl Tests {
    fn as_str(self) -> &'static str {
        match self {
            Tests::None => "none",
            Tests::Pass => "pass",
            Tests::Fail => "fail",
        }
    }
}

/// What a command call was, as far as this cares. Decided from the call and
/// settled by its result.
#[derive(Debug, Clone, Copy)]
struct Watched {
    test: bool,
    commit: bool,
}

/// One turn's signals, gathered from its events as they go by.
///
/// A turn includes its pauses for approval: the value outlives the pause and
/// keeps adding up after the resume.
#[derive(Debug, Clone, Default)]
pub struct TurnFeatures {
    mode: ConversationMode,
    commit: bool,
    tests: Tests,
    replies: Vec<String>,
    pending: HashMap<String, Watched>,
}

impl TurnFeatures {
    /// `mode` as it was when the turn started.
    pub fn new(mode: ConversationMode) -> Self {
        Self { mode, ..Self::default() }
    }

    pub fn observe(&mut self, event: &ChatEventPayload) {
        match event {
            ChatEventPayload::RoundCompleted { text, .. } if !text.is_empty() => self.replies.push(text.clone()),
            ChatEventPayload::ToolCall(call) if call.name == ToolName::RunCommand.wire_name() => {
                let Ok(request) = serde_json::from_str::<CommandRequest>(&call.arguments) else {
                    return;
                };
                let watched = Watched {
                    test: TEST_RX.is_match(&request.command),
                    commit: COMMIT_RX.is_match(&request.command),
                };
                if watched.test || watched.commit {
                    self.pending.insert(call.id.clone(), watched);
                }
            }
            ChatEventPayload::ToolResult(result) => self.settle(result),
            _ => {}
        }
    }

    fn settle(&mut self, event: &ToolResultEvent) {
        let Some(watched) = self.pending.remove(&event.id) else {
            return;
        };
        let (failed, output) = match (&event.result, &event.error) {
            (Some(ToolResult::CommandRan(out)), _) => (!out.succeeded() || out.timed_out, format!("{}\n{}", out.stdout, out.stderr)),
            (_, Some(error)) => (true, error.clone()),
            // Started in the background: no result to judge yet, as in a
            // transcript where the command has not come back.
            _ => return,
        };
        if watched.test {
            self.tests = if failed || TEST_FAIL_RX.is_match(&output) { Tests::Fail } else { Tests::Pass };
        }
        if watched.commit && !failed {
            self.commit = true;
        }
    }

    /// The turn as it ended — `None` while it waits for an approval, which is
    /// not an end, and for a turn that said nothing, which the script never
    /// pairs either.
    pub fn finish(&self, outcome: &ChatStreamOutcome) -> Option<FinishedTurn> {
        let (done, cancelled) = match outcome {
            ChatStreamOutcome::Done(done) => (done, false),
            ChatStreamOutcome::Cancelled(done) => (done, true),
            ChatStreamOutcome::PendingApproval(_) => return None,
        };
        if self.replies.is_empty() {
            return None;
        }
        Some(FinishedTurn {
            mode: self.mode,
            cancelled,
            commit: self.commit,
            tests: self.tests,
            todo: todo_progress(&done.todos),
            reply: self.replies.join("\n"),
        })
    }
}

/// Everything the model input needs but the user's message, which only the
/// window has as the user saw it (`/init`, not the prompt it expands to).
#[derive(Debug, Clone)]
pub struct FinishedTurn {
    mode: ConversationMode,
    cancelled: bool,
    commit: bool,
    tests: Tests,
    todo: String,
    reply: String,
}

impl FinishedTurn {
    pub fn model_input(&self, user: &str, home: &str) -> String {
        let mode = match self.mode {
            ConversationMode::Agent => "agent",
            ConversationMode::Plan => "plan",
            ConversationMode::Ask => "ask",
            ConversationMode::Review => "review",
        };
        let outcome = if self.cancelled { "cancelled" } else { "done" };
        model_input(mode, outcome, self.commit, self.tests.as_str(), &self.todo, &clean(user, home), &clean(&self.reply, home))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::command_exec::CommandOutput;
    use crate::domain::llm::ChatStreamResult;
    use crate::domain::turn::{ChatDone, PendingApproval, ToolCallEvent};

    /// The golden string, pinned by the extractor's `selftest` too. If this
    /// changes, the model was trained on something else.
    #[test]
    fn the_input_is_the_one_the_extractor_builds() {
        assert_eq!(
            model_input(
                "agent",
                "done",
                true,
                "pass",
                "2/5",
                "почини тест в auth.py",
                "Исправил проверку токена, 12/12 тестов проходят. Перейти к F-5.12?"
            ),
            "<mode>agent<outcome>done<commit>1<tests>pass<todo>2/5<offer>Перейти к F-5.12?\n\
             <user>почини тест в auth.py\n\
             <agent>Исправил проверку токена, 12/12 тестов проходят. Перейти к F-5.12?"
        );
    }

    #[test]
    fn parts_are_cut_from_the_tail_in_characters() {
        let input = model_input("agent", "done", false, "none", "", &"я".repeat(600), &"ё".repeat(1600));
        assert!(input.contains(&format!("<user>{}\n", "я".repeat(USER_MAX))), "{input}");
        assert!(input.ends_with(&format!("<agent>{}", "ё".repeat(AGENT_MAX))));
        assert!(!input.contains(&"я".repeat(USER_MAX + 1)));
        assert!(!input.contains(&"ё".repeat(AGENT_MAX + 1)));
        assert_eq!(tail("abc", 0), "");
        assert_eq!(tail("abc", 5), "abc");
    }

    #[test]
    fn an_offer_is_a_closing_question() {
        assert_eq!(offer("Готово.\nЗакоммитить?"), "Закоммитить?");
        assert_eq!(offer("Готово. Всё."), "");
        assert_eq!(offer("Сделал. Дальше F-5.12? "), "Дальше F-5.12?");
        assert_eq!(offer("Нужно ли? Да, нужно. Продолжать?"), "Продолжать?");
        assert_eq!(offer("Одно предложение?"), "Одно предложение?");
        assert_eq!(offer("Сделал:\n- парсер\nДальше лексер?"), "Дальше лексер?", "a line ends a sentence too");
        assert_eq!(offer("Это а?б?"), "Это а?б?", "a question mark inside a word does not split");
        assert_eq!(offer(""), "");
        let long = format!("Итог. {}?", "x".repeat(300));
        assert_eq!(offer(&long).chars().count(), OFFER_MAX);
        assert!(offer(&long).ends_with("x?"), "kept from the tail");
    }

    #[test]
    fn names_the_input_lacks_are_novel() {
        assert!(novel_names("поставь GitCompareArrows", "есть `GitCompareArrows`").is_empty());
        assert_eq!(novel_names("открой utils.py", "поправил main.py"), BTreeSet::from(["utils.py".to_string()]));
        assert!(novel_names("давай далее, сделай коммит", "").is_empty());
        assert_eq!(names("`x y` a_b fooBar v2 plain"), BTreeSet::from(["x y", "a_b", "fooBar", "v2"].map(String::from)));
    }

    #[test]
    fn cleaning_hides_the_home_folder_and_secrets() {
        let text = "ключ sk-abcdefghijklmnopqrstuvwx в /Users/me/a, token: abcdefgh123 \
                    ghp_abcdefghijklmnopqrstuvwxyz AKIAABCDEFGHIJKLMNOP xoxb-1234567890 \
                    eyJhbGciOiJIUzI1.eyJzdWIiOiIxMjM0.sig Bearer abcdefghijk";
        assert_eq!(
            clean(text, "/Users/me"),
            "ключ <KEY> в ~/a, token: <SECRET> <KEY> <KEY> <KEY> <JWT> Bearer <SECRET>"
        );
        assert_eq!(clean("  /x  ", ""), "/x", "no home, nothing replaced");
        assert_eq!(
            clean("<!-- reply -->\n> цитата\nа<system-reminder>\nx\n</system-reminder> <pasted_content id=1>\nx</pasted_content>", ""),
            "а [вставка]"
        );
    }

    fn task(status: TodoStatus) -> Task {
        Task { id: "1".into(), title: "t".into(), status, note: None }
    }

    #[test]
    fn progress_leaves_cancelled_items_out() {
        use TodoStatus::*;
        assert_eq!(todo_progress(&[]), "");
        assert_eq!(todo_progress(&[task(Cancelled)]), "");
        assert_eq!(todo_progress(&[task(Completed), task(Pending), task(InProgress), task(Cancelled)]), "1/3");
    }

    fn call(id: &str, command: &str) -> ChatEventPayload {
        ChatEventPayload::ToolCall(ToolCallEvent {
            id: id.into(),
            name: "runCommand".into(),
            arguments: serde_json::json!({ "command": command }).to_string(),
        })
    }

    fn ran(id: &str, exit_code: Option<i32>, stdout: &str) -> ChatEventPayload {
        ChatEventPayload::ToolResult(ToolResultEvent {
            id: id.into(),
            result: Some(ToolResult::CommandRan(CommandOutput {
                stdout: stdout.into(),
                exit_code,
                timed_out: exit_code.is_none(),
                ..CommandOutput::default()
            })),
            error: None,
            changes: Vec::new(),
        })
    }

    fn said(text: &str) -> ChatEventPayload {
        ChatEventPayload::RoundCompleted { text: text.into(), reasoning: String::new(), truncated: false }
    }

    fn done(todos: Vec<Task>) -> ChatStreamOutcome {
        ChatStreamOutcome::Done(ChatDone { result: ChatStreamResult::default(), todos, history: Vec::new() })
    }

    fn input_after(events: &[ChatEventPayload]) -> String {
        let mut turn = TurnFeatures::new(ConversationMode::Agent);
        events.iter().for_each(|e| turn.observe(e));
        turn.finish(&done(Vec::new())).expect("a turn that spoke").model_input("u", "")
    }

    fn head(input: &str) -> &str {
        input.split('\n').next().unwrap_or_default()
    }

    /// `cargo test 2>&1 | tail -30` exits 0 with tests failing — most failed
    /// runs in the transcripts looked like this.
    #[test]
    fn a_piped_test_run_that_failed_is_a_failure() {
        let input = input_after(&[call("a", "cargo test 2>&1 | tail -30"), ran("a", Some(0), "test result: FAILED. 3 passed; 1 failed"), said("x")]);
        assert!(head(&input).contains("<tests>fail"), "{input}");
    }

    /// `bun test` reports on stderr.
    #[test]
    fn a_failure_reported_on_stderr_is_a_failure() {
        let result = ChatEventPayload::ToolResult(ToolResultEvent {
            id: "a".into(),
            result: Some(ToolResult::CommandRan(CommandOutput {
                stderr: "290 pass\n1 fail".into(),
                exit_code: Some(0),
                ..CommandOutput::default()
            })),
            error: None,
            changes: Vec::new(),
        });
        let input = input_after(&[call("a", "bun test | tail"), result, said("x")]);
        assert!(head(&input).contains("<tests>fail"), "{input}");
    }

    #[test]
    fn the_last_test_run_decides() {
        let input = input_after(&[
            call("a", "bun test"),
            ran("a", Some(1), ""),
            call("b", "bun test"),
            ran("b", Some(0), "12 pass\n0 fail"),
            said("x"),
        ]);
        assert!(head(&input).contains("<tests>pass"), "`0 fail` is not a failure: {input}");
        let input = input_after(&[call("a", "pytest"), ran("a", None, ""), said("x")]);
        assert!(head(&input).contains("<tests>fail"), "timed out: {input}");
        let input = input_after(&[call("a", "pytest"), said("x")]);
        assert!(head(&input).contains("<tests>none"), "no result, nothing known: {input}");
    }

    #[test]
    fn a_commit_counts_only_when_it_succeeded() {
        let failed = input_after(&[call("a", "git commit -m x"), ran("a", Some(1), ""), said("x")]);
        assert!(head(&failed).contains("<commit>0"), "{failed}");
        let refused = ChatEventPayload::ToolResult(ToolResultEvent { id: "a".into(), result: None, error: Some("denied".into()), changes: Vec::new() });
        let denied = input_after(&[call("a", "git commit -m x"), refused, said("x")]);
        assert!(head(&denied).contains("<commit>0"), "{denied}");
        let made = input_after(&[call("a", "git add -A && git commit -m x"), ran("a", Some(0), ""), said("x")]);
        assert!(head(&made).contains("<commit>1<tests>none"), "{made}");
        let piped = input_after(&[call("a", "git log | grep commit"), ran("a", Some(0), ""), said("x")]);
        assert!(head(&piped).contains("<commit>0"), "not a commit: {piped}");
    }

    /// A pause for approval is part of the turn: what came before it is still
    /// counted after the resume, and the pause itself is not an end.
    #[test]
    fn a_turn_with_a_pause_adds_up_across_it() {
        let mut turn = TurnFeatures::new(ConversationMode::Plan);
        turn.observe(&call("a", "git commit -m x"));
        turn.observe(&ran("a", Some(0), ""));
        turn.observe(&said("Сначала закоммитил."));
        let paused = ChatStreamOutcome::PendingApproval(PendingApproval {
            history: Vec::new(),
            round: 1,
            budget_used: 1,
            event_seq: 1,
            calls: Vec::new(),
            todos: Vec::new(),
            reads: Default::default(),
        });
        assert!(turn.finish(&paused).is_none());

        turn.observe(&call("b", "cargo test"));
        turn.observe(&ran("b", Some(0), "ok"));
        turn.observe(&said("Тесты прошли. Дальше?"));
        let ended = ChatStreamOutcome::Cancelled(ChatDone {
            result: ChatStreamResult::default(),
            todos: vec![task(TodoStatus::Completed), task(TodoStatus::Pending)],
            history: Vec::new(),
        });
        assert_eq!(
            turn.finish(&ended).expect("ended").model_input("сделай в /home/me/x", "/home/me"),
            "<mode>plan<outcome>cancelled<commit>1<tests>pass<todo>1/2<offer>Дальше?\n\
             <user>сделай в ~/x\n\
             <agent>Сначала закоммитил.\nТесты прошли. Дальше?"
        );
    }

    #[test]
    fn a_turn_that_said_nothing_is_not_an_input() {
        let mut turn = TurnFeatures::new(ConversationMode::Ask);
        turn.observe(&said(""));
        assert!(turn.finish(&done(Vec::new())).is_none());
        turn.observe(&said("Ответ."));
        assert!(turn.finish(&done(Vec::new())).expect("spoke").model_input("?", "").starts_with("<mode>ask<outcome>done"));
    }
}
