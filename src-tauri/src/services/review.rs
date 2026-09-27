//! `/review`: the working tree's changes, reviewed group by group, one
//! request a group, side by side — the rules are `domain::review`'s.
//!
//! A worker is the ordinary turn loop in `ConversationMode::Review`, held to
//! one round, with its group's desk for `reportFinding`: retries,
//! cancellation and usage come with it. What it says goes nowhere; only its
//! findings and how it ended are kept. One group failing costs that group,
//! not the review.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::domain::command_exec::Shell;
use crate::domain::hooks::Hooks;
use crate::domain::mcp::McpTools;
use crate::domain::project_rules::RuleFile;
use crate::domain::review::{self, FailedGroup, FileDiff, Finding, GroupProgress, GroupState, GroupSummary, ReviewDesk, ReviewReport, ReviewSink, SummaryArgs};
use crate::domain::conversation_mode::ConversationMode;
use crate::domain::llm::LlmMessage;
use crate::domain::tools::{ApprovalPolicy, CodeSearchFn, ToolScope};
use crate::domain::turn::{ChatEventPayload, ChatEventSink, ChatStreamOutcome};
use crate::services::llm_chat::{self, Turn, TurnError};
use crate::services::llm_session::LlmSession;

/// Workers at once. More would mostly find the provider's rate limit.
pub const WORKERS: usize = 3;

/// What a review needs from the app around it.
pub struct Reviewer<'a> {
    pub session: &'a LlmSession,
    pub scope: &'a ToolScope,
    /// The project's instructions: its conventions are what a change is
    /// reviewed against.
    pub rules: &'a [RuleFile],
    pub search: Option<CodeSearchFn>,
    pub cancelled: &'a (dyn Fn() -> bool + Sync),
    /// Told every group's progress: each as waiting at the start, then its
    /// worker's steps as they happen, then how it ended.
    pub progress: ReviewSink,
}

/// Reviews `files` — what the working tree holds against `HEAD`.
pub fn review(reviewer: &Reviewer, files: Vec<FileDiff>) -> ReviewReport {
    let (selected, excluded) = review::select(files);
    let groups = review::group(selected);
    let total = groups.len();
    let everything: Vec<String> = groups.iter().flatten().map(|file| file.path.clone()).collect();
    let groups: Vec<(GroupProgress, Vec<FileDiff>)> = groups
        .into_iter()
        .enumerate()
        .map(|(i, group)| (GroupProgress::waiting(i, total, group.iter().map(|file| file.path.clone()).collect()), group))
        .collect();
    for (progress, _) in &groups {
        (reviewer.progress)(progress.clone());
    }

    let queue = Mutex::new(groups.into_iter().collect::<VecDeque<_>>());
    let results: Mutex<Vec<(Vec<String>, Worked)>> = Mutex::default();
    std::thread::scope(|threads| {
        for _ in 0..WORKERS.min(total) {
            threads.spawn(|| loop {
                let Some((progress, group)) = queue.lock().unwrap_or_else(|e| e.into_inner()).pop_front() else { break };
                let paths = progress.files.clone();
                // A stop is the turn loop's to notice: a stopped worker asks nothing.
                let worked = work(reviewer, progress, group, &everything);
                results.lock().unwrap_or_else(|e| e.into_inner()).push((paths, worked));
            });
        }
    });

    let mut reviewed = Vec::new();
    let mut findings = Vec::new();
    let mut failed = Vec::new();
    let mut summaries = Vec::new();
    for (files, worked) in results.into_inner().unwrap_or_else(|e| e.into_inner()) {
        // What a worker found stands however it ended; the group is only
        // called reviewed when it finished.
        findings.extend(worked.findings);
        if let Some(SummaryArgs { summary, checked, worth_a_look }) = worked.summary {
            summaries.push(GroupSummary { files: files.clone(), summary, checked, worth_a_look });
        }
        match worked.unfinished {
            None => reviewed.extend(files),
            Some(error) => failed.push(FailedGroup { files, error }),
        }
    }
    reviewed.sort();
    failed.sort_by(|a, b| a.files.cmp(&b.files));
    summaries.sort_by(|a, b| a.files.cmp(&b.files));
    ReviewReport { reviewed, excluded, findings: review::rank(findings), failed, summaries }
}

/// How one group's worker ended.
struct Worked {
    findings: Vec<Finding>,
    summary: Option<SummaryArgs>,
    /// Why it did not finish, when it did not.
    unfinished: Option<String>,
}

/// One group, reviewed, its progress told as it goes: what it found, and why
/// it did not finish when it did not. What was found before a stop, a failure
/// or the round ceiling is kept.
fn work(reviewer: &Reviewer, progress: GroupProgress, group: Vec<FileDiff>, everything: &[String]) -> Worked {
    let others: Vec<String> = everything.iter().filter(|path| !group.iter().any(|file| &file.path == *path)).cloned().collect();
    let prompt = review::worker_prompt(&group, &others);
    let desk = Arc::new(ReviewDesk::new(group));

    let progress = Arc::new(Mutex::new(GroupProgress { state: GroupState::Working, ..progress }));
    let tell = reviewer.progress.clone();
    tell(progress.lock().unwrap_or_else(|e| e.into_inner()).clone());
    // What the worker wrote as text: its summary when it sent none with
    // finishReview — some models answer in prose instead of the call.
    let said = Arc::new(Mutex::new(String::new()));
    let events: ChatEventSink = {
        let (progress, tell, said) = (Arc::clone(&progress), tell.clone(), Arc::clone(&said));
        Arc::new(move |event| {
            if let ChatEventPayload::RoundCompleted { text, .. } = &event.event {
                *said.lock().unwrap_or_else(|e| e.into_inner()) = text.clone();
            }
            let mut progress = progress.lock().unwrap_or_else(|e| e.into_inner());
            if progress.apply(&event.event) {
                tell(progress.clone());
            }
        })
    };
    let approval = ApprovalPolicy { skip_all: true, ..ApprovalPolicy::default() };
    let shell = Shell::default();
    let sleep = |d: Duration| std::thread::sleep(d);
    let take_steering = Vec::new;
    let log_call = |_| {};
    let (hooks, mcp) = (Hooks::default(), McpTools::default());
    let turn = Turn {
        events: &events,
        session: reviewer.session,
        scope: reviewer.scope,
        approval: &approval,
        mode: ConversationMode::Review,
        cancelled: reviewer.cancelled,
        sleep: &sleep,
        take_steering: &take_steering,
        shell: &shell,
        shell_described: "",
        search: reviewer.search.clone(),
        skills: &[],
        rules: reviewer.rules,
        log_call: &log_call,
        plan: None,
        worktree_of: None,
        mcp: &mcp,
        hooks: &hooks,
        processes: None,
        terminals: None,
        review: Some(Arc::clone(&desk)),
    };
    let messages = vec![LlmMessage::user(prompt)];
    {
        let mut progress = progress.lock().unwrap_or_else(|e| e.into_inner());
        progress.estimate = llm_chat::estimate_request(&turn, &messages) as u64;
        tell(progress.clone());
    }
    let unfinished = match llm_chat::stream(&turn, messages, Vec::new()) {
        Ok(ChatStreamOutcome::Done(_)) => None,
        Ok(ChatStreamOutcome::Cancelled(_)) => Some("stopped".to_string()),
        // Nothing a reviewer has asks; a card here is a bug, not a pause.
        Ok(ChatStreamOutcome::PendingApproval(_)) => Some("asked for approval".to_string()),
        // The one reply was given and its findings noted: that is the end
        // of a worker, not its ceiling.
        Err(TurnError::Exhausted { .. }) => None,
        Err(e) => Some(e.to_string()),
    };
    let mut last = progress.lock().unwrap_or_else(|e| e.into_inner()).clone();
    last.note = None;
    match &unfinished {
        None => last.state = GroupState::Done,
        Some(error) => {
            last.state = GroupState::Failed;
            last.error = Some(error.clone());
        }
    }
    tell(last);
    let summary = desk.take_summary().or_else(|| {
        let said = said.lock().unwrap_or_else(|e| e.into_inner());
        let said = said.trim();
        (!said.is_empty()).then(|| SummaryArgs { summary: said.to_string(), ..SummaryArgs::default() })
    });
    Worked { findings: desk.take_findings(), summary, unfinished }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::llm::{
        ChatRequest, ChatResponse, ChatStreamResult, LlmError, LlmModelInfo, LlmProvider, LlmRole, LlmToolCall,
    };
    use crate::domain::review::{Exclusion, FileStatus, NewLine, Severity};
    use crate::testing::temp_dir;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A reviewer that reports on whatever it was given with a `bad` line in
    /// it and closes with a summary naming its first file — and fails
    /// outright on a file named `boom`.
    struct Reviewing {
        requests: AtomicUsize,
        at_once: AtomicUsize,
        most: AtomicUsize,
    }

    impl LlmProvider for Reviewing {
        fn chat(&self, _: ChatRequest) -> Result<ChatResponse, LlmError> {
            unreachable!("a review does not summarize")
        }

        fn chat_stream(
            &self,
            request: ChatRequest,
            _: &dyn Fn(&str),
            _: &dyn Fn(&str),
            _: &dyn Fn(&str, &str, &str),
            _: &dyn Fn() -> bool,
        ) -> Result<ChatStreamResult, LlmError> {
            self.requests.fetch_add(1, Ordering::SeqCst);
            let now = self.at_once.fetch_add(1, Ordering::SeqCst) + 1;
            self.most.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(30));
            self.at_once.fetch_sub(1, Ordering::SeqCst);

            let prompt = request.messages.iter().find(|m| m.role == LlmRole::User).and_then(|m| m.content.clone()).unwrap_or_default();
            // The group with it answers last.
            if prompt.contains("=== a/slow.rs") {
                std::thread::sleep(Duration::from_millis(200));
            }
            if prompt.contains("=== boom.rs") {
                return Err(LlmError::Provider("said 500".into()));
            }
            let answered = request.messages.last().is_some_and(|m| m.role == LlmRole::Tool);
            if answered {
                return Ok(ChatStreamResult { text: "checked".into(), ..Default::default() });
            }
            let first = prompt.split("=== ").nth(1).and_then(|h| h.split(' ').next()).unwrap_or_default().to_string();
            // Answers in prose, no calls: the text is its summary.
            if prompt.contains("=== prose.rs") {
                return Ok(ChatStreamResult { text: "Adds prose; nothing wrong.".into(), ..Default::default() });
            }
            let summary = LlmToolCall {
                id: format!("s-{first}"),
                name: "finishReview".into(),
                arguments: serde_json::json!({ "summary": format!("Reviewed {first}."), "checked": ["the bad line"] }).to_string(),
            };
            if !prompt.contains("+bad") {
                return Ok(ChatStreamResult { tool_calls: vec![summary], ..Default::default() });
            }
            // The file whose diff has the bad line: the header just before it.
            let before = &prompt[..prompt.find("+bad").unwrap_or(0)];
            let header = before.rfind("=== ").map_or("", |at| &before[at + 4..]);
            let path = header.split(' ').next().unwrap_or_default().to_string();
            let arguments = serde_json::json!({
                "path": path, "existingCode": "bad", "title": "Bad", "body": "It is bad.", "severity": "high", "category": "bug"
            });
            Ok(ChatStreamResult {
                tool_calls: vec![LlmToolCall { id: format!("f-{path}"), name: "reportFinding".into(), arguments: arguments.to_string() }, summary],
                ..Default::default()
            })
        }

        fn list_models(&self) -> Result<Vec<LlmModelInfo>, LlmError> {
            Ok(Vec::new())
        }
    }

    /// A file whose first added line is `text`, its diff about `chars` long —
    /// what a group is measured in.
    fn diff(path: &str, text: &str, chars: usize) -> FileDiff {
        FileDiff {
            path: path.into(),
            status: FileStatus::Modified,
            binary: false,
            patch: format!("@@ -0,0 +1 @@\n+{text}\n{}", " ok\n".repeat(chars / 4)),
            lines: vec![NewLine { hunk: 0, number: 1, added: true, text: text.into() }],
            whole: false,
        }
    }

    /// Six tenths of a group: no two fit one.
    const BIG: usize = review::MAX_GROUP_CHARS * 6 / 10;

    fn run(files: Vec<FileDiff>, cancelled: &(dyn Fn() -> bool + Sync)) -> (ReviewReport, Arc<Reviewing>, Vec<GroupProgress>) {
        let provider = Arc::new(Reviewing { requests: AtomicUsize::new(0), at_once: AtomicUsize::new(0), most: AtomicUsize::new(0) });
        let session = LlmSession {
            provider: provider.clone(),
            provider_id: "test".into(),
            model: "m".into(),
            debug_logging: false,
            context_limit: None,
            reply_language: None,
        };
        let scope = ToolScope::new(&temp_dir("review-run")).unwrap();
        let told = Arc::new(Mutex::new(Vec::new()));
        let progress: ReviewSink = {
            let told = Arc::clone(&told);
            Arc::new(move |group| told.lock().unwrap().push(group))
        };
        let reviewer = Reviewer { session: &session, scope: &scope, rules: &[], search: None, cancelled, progress };
        let report = review(&reviewer, files);
        let told = told.lock().unwrap().clone();
        (report, provider, told)
    }

    /// Eight folders, no two of which fit one group: so each group
    /// is a worker of its own, and they run side by side.
    #[test]
    fn groups_are_reviewed_side_by_side_and_their_findings_ranked_together() {
        let mut files: Vec<FileDiff> = (0..8).map(|i| diff(&format!("d{i}/f.rs"), if i % 3 == 0 { "bad" } else { "fine" }, BIG)).collect();
        files.push(FileDiff { binary: true, ..diff("logo.png", "x", 1) });
        let (report, provider, told) = run(files, &|| false);

        assert_eq!(report.reviewed.len(), 8);
        assert_eq!(report.excluded.iter().map(|e| (e.path.as_str(), e.reason)).collect::<Vec<_>>(), [("logo.png", Exclusion::Binary)]);
        let found: Vec<(u32, &str, u32, Severity)> = report.findings.iter().map(|f| (f.id, f.path.as_str(), f.start_line, f.severity)).collect();
        assert_eq!(found, [(1, "d0/f.rs", 1, Severity::High), (2, "d3/f.rs", 1, Severity::High), (3, "d6/f.rs", 1, Severity::High)]);
        assert!(report.failed.is_empty());
        // Every group closes with its summary, found something or not, in group order.
        let summaries: Vec<(&str, &str)> = report.summaries.iter().map(|s| (s.files[0].as_str(), s.summary.as_str())).collect();
        assert_eq!(summaries.len(), 8);
        assert_eq!(summaries[1], ("d1/f.rs", "Reviewed d1/f.rs."));
        assert_eq!(report.summaries[0].checked, ["the bad line"]);
        let most = provider.most.load(Ordering::SeqCst);
        assert!(most > 1 && most <= WORKERS, "{most} at once");
        let ended: Vec<&GroupProgress> = told.iter().filter(|g| g.state == GroupState::Done).collect();
        assert_eq!(ended.len(), 8, "each group says it is done");
        assert!(told[..8].iter().enumerate().all(|(i, g)| g.state == GroupState::Waiting && g.group as usize == i && g.total == 8), "every group waiting first");
    }

    /// A small change is one worker, which sees every file.
    #[test]
    fn a_small_change_is_one_worker() {
        let (report, provider, _) = run(vec![diff("a.rs", "bad", 1), diff("b.rs", "fine", 1)], &|| false);
        assert_eq!(report.findings.len(), 1);
        assert_eq!(provider.requests.load(Ordering::SeqCst), 1, "one reply with its report");
    }

    #[test]
    fn a_group_that_fails_costs_that_group_alone() {
        let files = vec![diff("a/boom.rs", "fine", BIG), diff("b/x.rs", "bad", BIG), diff("c/y.rs", "fine", BIG), diff("d/z.rs", "fine", BIG)];
        let mut boom = files;
        boom[0].path = "boom.rs".into();
        let (report, _, told) = run(boom, &|| false);
        let boom_end = told.iter().rfind(|g| g.files == ["boom.rs"]).unwrap();
        assert_eq!(boom_end.state, GroupState::Failed);
        assert!(boom_end.error.as_deref().is_some_and(|e| e.contains("500")), "{boom_end:?}");
        let found = told.iter().rfind(|g| g.files == ["b/x.rs"]).unwrap();
        assert_eq!((found.state, found.findings), (GroupState::Done, 1));
        assert!(found.estimate > 0 && found.input + found.output == 0, "the mock reports no usage: the estimate stands in, {found:?}");
        let first_working = told.iter().find(|g| g.files == ["b/x.rs"] && g.state == GroupState::Working && g.estimate > 0).unwrap();
        assert_eq!(first_working.findings, 0, "the estimate is told before the request is sent");
        let mid = told.iter().find(|g| g.files == ["b/x.rs"] && g.state == GroupState::Working && g.findings == 1);
        assert!(mid.is_some(), "a finding is told as it is noted, before the group ends");
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.failed[0].files, ["boom.rs"]);
        assert!(report.failed[0].error.contains("500"), "{}", report.failed[0].error);
        assert_eq!(report.findings.len(), 1, "the others still count");
        assert!(!report.reviewed.contains(&"boom.rs".to_string()));
    }

    /// Workers end in any order; the summaries come in the groups' own.
    #[test]
    fn summaries_follow_the_groups_not_who_answered_first() {
        let (report, _, _) = run(vec![diff("a/slow.rs", "fine", BIG), diff("b/x.rs", "fine", BIG), diff("c/y.rs", "fine", BIG), diff("d/z.rs", "fine", BIG)], &|| false);
        let order: Vec<&str> = report.summaries.iter().map(|s| s.files[0].as_str()).collect();
        assert_eq!(order, ["a/slow.rs", "b/x.rs", "c/y.rs", "d/z.rs"]);
    }

    /// A worker that writes its summary as text rather than in finishReview
    /// still has one.
    #[test]
    fn a_summary_written_as_text_is_kept() {
        let (report, _, _) = run(vec![diff("prose.rs", "fine", 10)], &|| false);
        assert_eq!(report.summaries.len(), 1);
        assert_eq!(report.summaries[0].summary, "Adds prose; nothing wrong.");
        assert!(report.summaries[0].checked.is_empty());
    }

    #[test]
    fn a_stopped_review_reviews_nothing_more() {
        let (report, provider, _) = run((0..8).map(|i| diff(&format!("d{i}/f.rs"), "bad", BIG)).collect(), &|| true);
        assert_eq!(provider.requests.load(Ordering::SeqCst), 0);
        assert_eq!(report.failed.len(), 8);
        assert!(report.failed.iter().all(|f| f.error == "stopped"));
    }

    #[test]
    fn nothing_changed_is_nothing_to_do() {
        let (report, provider, told) = run(Vec::new(), &|| false);
        assert!(report.reviewed.is_empty() && report.findings.is_empty());
        assert_eq!(provider.requests.load(Ordering::SeqCst), 0);
        assert!(told.is_empty());
    }
}



