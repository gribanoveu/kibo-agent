//! `pullRequestStatus` — the pull request of the folder's branch, or of one
//! the model names, as the strip under the branch in Overview reads it
//! (`docs/62-bitbucket-prs.md`, decision 8, BB-6): reviewers and their
//! decisions, the builds of the branch's head commit, whether it can be merged
//! and what the server says stands in the way.
//!
//! Only a read: approve, merge and create are the user's buttons. It goes
//! through the strip's own service — one server, one token, the same refusal
//! when the remote names no server the user added — and the token never
//! reaches what the model reads.

use crate::domain::code_host::{BuildState, PrStrip, ReviewStatus, Snapshot};
use crate::domain::llm::LlmToolDefinition;
use crate::domain::tools::{PullRequestStatusArgs, ToolDeps, ToolError, ToolResult, ToolScope};

/// Most builds listed; a commit built by many jobs says how many more.
const BUILDS: usize = 10;

pub fn pull_request_status(scope: &ToolScope, args: &PullRequestStatusArgs, deps: &ToolDeps) -> Result<ToolResult, ToolError> {
    let reader = deps.pull_requests.as_deref().ok_or_else(|| ToolError::FeatureOff {
        tool: "pullRequestStatus".to_string(),
        feature: "Bitbucket".to_string(),
    })?;
    let strip = reader.status(scope.root(), args.branch.as_deref())?;
    let (text, summary) = describe(&strip)?;
    Ok(ToolResult::PullRequest { text, summary })
}

/// What the model reads, and the transcript's few words. A folder with nothing
/// to ask a server about is an error the model can pass on as it is.
fn describe(strip: &PrStrip) -> Result<(String, String), ToolError> {
    use crate::domain::code_host::CodeHostError;
    let (branch, snapshot, merge_blocked) = match strip {
        PrStrip::Ready { branch, snapshot, merge_blocked, .. } => (branch, snapshot, merge_blocked),
        PrStrip::NoRemote => return Err(CodeHostError::NoRemote.into()),
        PrStrip::NoBranch => return Err(CodeHostError::NoBranch.into()),
        PrStrip::NotConnected { remote } => return Err(CodeHostError::NoServer(remote.host.clone()).into()),
        // The strip's word for a server out of reach; this read says the error instead.
        PrStrip::Unreachable => return Err(CodeHostError::Unreachable("no answer".to_string()).into()),
    };
    if !snapshot.branch_on_server {
        return Ok((
            format!(
                "The branch {branch} is not on the server — either it was never pushed, or no branch is named so there. \
                 A pull request needs it pushed first."
            ),
            "not pushed".to_string(),
        ));
    }
    let Some(pr) = &snapshot.pr else {
        let into = snapshot.default_branch.as_deref().map(|to| format!(" (the default branch is {to})")).unwrap_or_default();
        let mut text = format!("The branch {branch} has no open pull request{into}.");
        builds(&mut text, snapshot);
        return Ok((text, "no pull request".to_string()));
    };
    let draft = if pr.draft == Some(true) { " — draft, not ready for review" } else { "" };
    let mut text = format!(
        "Pull request #{} \"{}\"{draft}\n{} → {}, by {}\n{}\n",
        pr.id, pr.title, pr.from_branch, pr.to_branch, pr.author_name, pr.url
    );
    if pr.reviewers.is_empty() {
        text.push_str("Reviewers: none\n");
    } else {
        let reviewers: Vec<String> = pr
            .reviewers
            .iter()
            .map(|r| {
                let decision = match r.status {
                    ReviewStatus::Approved => "approved",
                    ReviewStatus::NeedsWork => "needs work",
                    ReviewStatus::Unapproved => "not yet",
                };
                format!("{} — {decision}", r.display_name)
            })
            .collect();
        text.push_str(&format!("Reviewers: {}\n", reviewers.join("; ")));
    }
    let summary = match (merge_blocked, &snapshot.merge) {
        (None, Some(_)) => {
            text.push_str("Merge: can be merged\n");
            format!("#{} can be merged", pr.id)
        }
        (Some(reasons), _) => {
            text.push_str(&format!("Merge: blocked — {}\n", reasons.join("; ")));
            format!("#{} blocked", pr.id)
        }
        (None, None) => format!("#{}", pr.id),
    };
    comments(&mut text, snapshot);
    builds(&mut text, snapshot);
    Ok((text.trim_end().to_string(), summary))
}

/// The reviewers' open remarks, with the place each points at: what the
/// Address comments button sends the agent to work on. A resolved thread or
/// task is not open and is not here. All of them are listed: the button
/// carries the first twenty and sends the model here for the rest, so a cut
/// here would make that promise false.
fn comments(text: &mut String, snapshot: &Snapshot) {
    let open: Vec<_> = snapshot.comments.iter().filter(|comment| !comment.resolved).collect();
    if open.is_empty() {
        text.push_str("\nOpen review comments: none");
        return;
    }
    text.push_str(&format!("\nOpen review comments ({}):", open.len()));
    for comment in &open {
        let place = match (&comment.path, comment.line) {
            (Some(path), Some(line)) => format!("{path}:{line}"),
            (Some(path), None) => path.clone(),
            _ => "general".to_string(),
        };
        let kind = if comment.task { "task" } else { "comment" };
        text.push_str(&format!("\n- {kind} — {place} — {}: {}", comment.author_name, comment.text));
    }
}

/// The head commit's builds, newest first — what CI said of the code as it is
/// on the server. The logs are in CI, behind each link.
fn builds(text: &mut String, snapshot: &Snapshot) {
    if snapshot.builds.is_empty() {
        text.push_str("\nBuilds of the head commit: none reported");
        return;
    }
    let mut builds: Vec<_> = snapshot.builds.iter().collect();
    builds.sort_by_key(|build| std::cmp::Reverse(build.date_added));
    text.push_str("\nBuilds of the head commit, newest first:");
    for build in builds.iter().take(BUILDS) {
        let state = match build.state {
            BuildState::Successful => "successful",
            BuildState::Failed => "failed",
            BuildState::InProgress => "running",
            BuildState::Unknown => "unknown",
        };
        text.push_str(&format!("\n- {state} — {} — {}", build.name, build.url));
    }
    if builds.len() > BUILDS {
        text.push_str(&format!("\n… and {} more", builds.len() - BUILDS));
    }
}

pub(super) fn definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "pullRequestStatus".to_string(),
        description: "Reads the pull request of a branch on the user's Bitbucket — the folder's checked-out branch unless \
            `branch` names another: title, draft or not, source and target, author, link; each reviewer's decision; \
            whether it can be merged and, if not, the server's reasons (conflicts, missing approvals, failed builds); \
            the reviewers' open comments and tasks with the file and line each points at; \
            and the builds of the branch's head commit with their state and link. Nothing is changed: approving, \
            merging and creating are the user's buttons under the branch in Overview. Build logs are not here — they \
            are in CI, behind each build's link. A branch that is not pushed, or has no pull request, is said so."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "branch": { "type": ["string", "null"], "description": "A branch on the server. Left out: the folder's checked-out branch." }
            },
            "required": [],
            "additionalProperties": false
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::code_host::{
        Build, CodeHostError, MergeCheck, PullRequest, AgentPullRequests, RemoteRepo, Reviewer,
    };
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    /// The service as the tool sees it: one answer, and what it was asked.
    struct Fake {
        answer: Result<PrStrip, CodeHostError>,
        asked: Mutex<Vec<(std::path::PathBuf, Option<String>)>>,
    }

    impl AgentPullRequests for Fake {
        fn status(&self, root: &Path, branch: Option<&str>) -> Result<PrStrip, CodeHostError> {
            self.asked.lock().unwrap().push((root.to_path_buf(), branch.map(str::to_string)));
            self.answer.clone()
        }
        fn create(&self, _: &std::path::Path, _: &str, _: &str, _: Option<&str>, _: bool) -> Result<crate::domain::code_host::PullRequest, crate::domain::code_host::CodeHostError> {
            Err(crate::domain::code_host::CodeHostError::NoRemote)
        }
    }

    fn run(answer: Result<PrStrip, CodeHostError>, branch: Option<&str>) -> (Result<ToolResult, ToolError>, Arc<Fake>) {
        let fake = Arc::new(Fake { answer, asked: Mutex::default() });
        let dir = crate::testing::temp_dir("pr-status-tool");
        let scope = ToolScope::new(&dir).unwrap();
        let deps = ToolDeps { pull_requests: Some(fake.clone()), ..ToolDeps::default() };
        let result = pull_request_status(&scope, &PullRequestStatusArgs { branch: branch.map(str::to_string) }, &deps);
        (result, fake)
    }

    fn answer(result: Result<ToolResult, ToolError>) -> (String, String) {
        match result {
            Ok(ToolResult::PullRequest { text, summary }) => (text, summary),
            other => panic!("not a pull request: {other:?}"),
        }
    }

    fn build(name: &str, state: BuildState, at: u64) -> Build {
        Build { key: name.to_string(), name: name.to_string(), state, url: format!("https://ci/{name}"), date_added: Some(at) }
    }

    fn ready(pr: Option<PullRequest>, merge: Option<MergeCheck>, builds: Vec<Build>) -> PrStrip {
        let merge_blocked = merge.as_ref().and_then(crate::domain::code_host::merge_blocked);
        PrStrip::Ready {
            server_id: "s".to_string(),
            repo: "tax-api".to_string(),
            branch: "feature/x".to_string(),
            merge_blocked,
            latest_build: None,
            snapshot: Snapshot { pr, branch_on_server: true, default_branch: Some("master".to_string()), merge, builds, comments: Vec::new() },
            can_approve: false,
            approved: false,
            open_comments: 0,
        }
    }

    fn remark(id: u64, task: bool, resolved: bool, path: Option<&str>, line: Option<u64>) -> crate::domain::code_host::Comment {
        crate::domain::code_host::Comment {
            id,
            author: "u_reviewer".to_string(),
            author_name: "Rita Reviewer".to_string(),
            text: format!("remark {id}"),
            path: path.map(str::to_string),
            line,
            task,
            resolved,
        }
    }

    /// A strip with remarks, as the service hands it: the open count computed.
    fn remarked(comments: Vec<crate::domain::code_host::Comment>) -> PrStrip {
        let PrStrip::Ready { snapshot, .. } = ready(Some(pr(Some(false))), None, Vec::new()) else { unreachable!() };
        let open_comments = crate::domain::code_host::open_comments(&comments);
        PrStrip::Ready {
            server_id: "s".to_string(),
            repo: "tax-api".to_string(),
            branch: "feature/x".to_string(),
            merge_blocked: None,
            latest_build: None,
            snapshot: Snapshot { comments, ..snapshot },
            can_approve: false,
            approved: false,
            open_comments,
        }
    }

    fn pr(draft: Option<bool>) -> PullRequest {
        PullRequest {
            id: 153,
            title: "Fix the tax profile".to_string(),
            version: 7,
            author: "u_me".to_string(),
            author_name: "Евгений".to_string(),
            from_branch: "feature/x".to_string(),
            to_branch: "master".to_string(),
            reviewers: vec![
                Reviewer { slug: "anna".to_string(), display_name: "Anna Lead".to_string(), status: ReviewStatus::Approved },
                Reviewer { slug: "boris".to_string(), display_name: "Boris".to_string(), status: ReviewStatus::NeedsWork },
                Reviewer { slug: "gleb".to_string(), display_name: "Gleb".to_string(), status: ReviewStatus::Unapproved },
            ],
            url: "https://git.example/projects/P/repos/tax-api/pull-requests/153".to_string(),
            draft,
        }
    }

    /// Everything the strip knows, in words: the PR, each reviewer's decision,
    /// why it cannot merge, and the builds newest first with their links.
    #[test]
    fn a_pull_request_is_read_whole() {
        let merge = MergeCheck { can_merge: false, conflicted: true, vetoes: vec!["Requires 2 approvals".to_string()] };
        let builds = vec![build("old #1", BuildState::Successful, 1), build("new #2", BuildState::Failed, 2)];
        let (text, summary) = answer(run(Ok(ready(Some(pr(Some(false))), Some(merge), builds)), None).0);
        for line in [
            "Pull request #153 \"Fix the tax profile\"\n",
            "feature/x → master, by Евгений",
            "https://git.example/projects/P/repos/tax-api/pull-requests/153",
            "Reviewers: Anna Lead — approved; Boris — needs work; Gleb — not yet",
            "Merge: blocked — conflicts; Requires 2 approvals",
            "- failed — new #2 — https://ci/new #2\n- successful — old #1 — https://ci/old #1",
        ] {
            assert!(text.contains(line), "{line:?} missing from:\n{text}");
        }
        assert!(!text.contains("draft"), "{text}");
        assert_eq!(summary, "#153 blocked");
    }

    #[test]
    fn a_draft_and_a_mergeable_one_say_so() {
        let merge = MergeCheck { can_merge: true, conflicted: false, vetoes: Vec::new() };
        let (text, summary) = answer(run(Ok(ready(Some(pr(Some(true))), Some(merge), Vec::new())), None).0);
        assert!(text.contains("\"Fix the tax profile\" — draft, not ready for review"), "{text}");
        assert!(text.contains("Merge: can be merged"), "{text}");
        assert!(text.contains("Builds of the head commit: none reported"), "{text}");
        assert_eq!(summary, "#153 can be merged");
        // A server before drafts: neither word. And a merge check not asked
        // says nothing either way — no "can be merged" it never said.
        let (text, summary) = answer(run(Ok(ready(Some(pr(None)), None, Vec::new())), None).0);
        assert!(!text.contains("draft"), "{text}");
        assert!(!text.contains("Merge:"), "{text}");
        assert_eq!(summary, "#153");
    }

    /// The branch named goes to the service as it is; none is the folder's own.
    #[test]
    fn the_branch_named_is_the_one_read() {
        let (_, fake) = run(Ok(ready(None, None, Vec::new())), Some("release/1.2"));
        let (_, mine) = run(Ok(ready(None, None, Vec::new())), None);
        assert_eq!(fake.asked.lock().unwrap()[0].1.as_deref(), Some("release/1.2"));
        assert_eq!(mine.asked.lock().unwrap()[0].1, None);
    }

    #[test]
    fn no_pull_request_and_not_pushed_are_answers() {
        let builds = vec![build("ci #9", BuildState::InProgress, 9)];
        let (text, summary) = answer(run(Ok(ready(None, None, builds)), None).0);
        assert!(text.starts_with("The branch feature/x has no open pull request (the default branch is master)."), "{text}");
        assert!(text.contains("- running — ci #9"), "{text}");
        assert_eq!(summary, "no pull request");

        let PrStrip::Ready { snapshot, .. } = ready(None, None, Vec::new()) else { unreachable!() };
        let unpushed = PrStrip::Ready {
            server_id: "s".to_string(),
            repo: "r".to_string(),
            branch: "feature/x".to_string(),
            merge_blocked: None,
            latest_build: None,
            snapshot: Snapshot { branch_on_server: false, ..snapshot },
            can_approve: false,
            approved: false,
            open_comments: 0,
        };
        let (text, summary) = answer(run(Ok(unpushed), None).0);
        assert!(text.contains("is not on the server — either it was never pushed, or no branch is named so there"), "{text}");
        assert_eq!(summary, "not pushed");
    }

    /// A remote no server was added for is a refusal that says where to add
    /// one — not a network error; a server out of reach is said, not hidden.
    #[test]
    fn what_cannot_be_asked_is_said_plainly() {
        let remote = RemoteRepo::parse("ssh://git@git.corp.example/corp/tax-api.git").unwrap();
        let error = run(Ok(PrStrip::NotConnected { remote }), None).0.unwrap_err().to_string();
        assert!(error.contains("no server is configured for git.corp.example") && error.contains("Catalog → Bitbucket"), "{error}");
        let error = run(Err(CodeHostError::Unreachable("connection refused".to_string())), None).0.unwrap_err().to_string();
        assert!(error.contains("out of reach"), "{error}");
        assert!(run(Ok(PrStrip::NoRemote), None).0.is_err());
        assert!(run(Ok(PrStrip::NoBranch), None).0.is_err());
    }

    #[test]
    fn many_builds_are_cut_and_counted() {
        let builds = (0..13).map(|i| build(&format!("job{i}"), BuildState::Successful, i)).collect();
        let (text, _) = answer(run(Ok(ready(None, None, builds)), None).0);
        assert_eq!(text.matches("\n- successful").count(), BUILDS, "{text}");
        assert!(text.contains("job12") && !text.contains("job2 "), "newest kept:\n{text}");
        assert!(text.ends_with("… and 3 more"), "{text}");
    }

    /// The open remarks with their place and author; a resolved one is not
    /// open and is not listed; none is said, not hidden.
    #[test]
    fn open_remarks_are_read_with_their_place() {
        let comments = vec![
            remark(101, false, false, Some("src/main.rs"), Some(34)),
            remark(201, true, false, None, None),
            remark(103, false, true, Some("src/main.rs"), Some(10)),
        ];
        let (text, _) = answer(run(Ok(remarked(comments)), None).0);
        assert!(text.contains("Open review comments (2):"), "{text}");
        assert!(text.contains("- comment — src/main.rs:34 — Rita Reviewer: remark 101"), "{text}");
        assert!(text.contains("- task — general — Rita Reviewer: remark 201"), "{text}");
        assert!(!text.contains("remark 103"), "a resolved thread is not open:\n{text}");

        let (text, _) = answer(run(Ok(remarked(Vec::new())), None).0);
        assert!(text.contains("Open review comments: none"), "{text}");
        // A pull request with no remarks read says the same as one with none.
        let (text, _) = answer(run(Ok(ready(Some(pr(Some(false))), None, Vec::new())), None).0);
        assert!(text.contains("Open review comments: none"), "{text}");
    }

    #[test]
    fn every_open_remark_is_listed_for_the_button_s_promise() {
        // The Address comments button carries the first twenty and sends the
        // model here for the rest: a cut here would hide them for good.
        let comments = (0..23).map(|i| remark(i, false, false, Some("src/main.rs"), Some(i))).collect();
        let (text, _) = answer(run(Ok(remarked(comments)), None).0);
        assert_eq!(text.matches("\n- comment").count(), 23, "{text}");
        assert!(text.contains("remark 22"), "the tail the button promised is readable:\n{text}");
        assert!(!text.contains("… and"), "{text}");
    }

    #[test]
    fn without_the_feature_it_says_where_to_turn_it_on() {
        let dir = crate::testing::temp_dir("pr-status-off");
        let scope = ToolScope::new(&dir).unwrap();
        let error = pull_request_status(&scope, &PullRequestStatusArgs::default(), &ToolDeps::default()).unwrap_err();
        assert!(matches!(error, ToolError::FeatureOff { .. }), "{error}");
    }
}
