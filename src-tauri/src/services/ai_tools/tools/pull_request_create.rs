//! `pullRequestCreate` — the agent opens a pull request from the folder's
//! branch on the user's Bitbucket (`docs/62-bitbucket-prs.md`, decision 10,
//! BB-7). The strip's own Create: the branch and the server are the folder's,
//! a branch not on the server is refused before anything is posted, and the
//! strip reads again after it. Mutating, so it goes through the approval
//! card, whose preview says where it goes, from which branch into which, and
//! under what title.
//!
//! The agent never merges or approves: those are the user's buttons.

use crate::domain::code_host::RemoteRepo;
use crate::domain::llm::LlmToolDefinition;
use crate::domain::tools::{PullRequestCreateArgs, ToolDeps, ToolError, ToolPreview, ToolResult, ToolScope};
use crate::infra::{git_head, repository_identity};

pub fn pull_request_create(scope: &ToolScope, args: &PullRequestCreateArgs, deps: &ToolDeps) -> Result<ToolResult, ToolError> {
    let prs = deps.pull_requests.as_deref().ok_or_else(|| ToolError::FeatureOff {
        tool: "pullRequestCreate".to_string(),
        feature: "Pull requests".to_string(),
    })?;
    let title = title(args)?;
    let description = args.body.as_deref().unwrap_or("");
    let pr = prs.create(scope.root(), title, description, target(args, deps.worktree_base), args.draft == Some(true))?;
    Ok(ToolResult::PullRequestCreated {
        id: pr.id,
        title: pr.title,
        from: pr.from_branch,
        to: pr.to_branch,
        url: pr.url,
        draft: pr.draft == Some(true),
    })
}

/// A title is the model's to write: an empty one would fall back to the
/// branch's name, which the user approving the card did not read as one.
fn title(args: &PullRequestCreateArgs) -> Result<&str, ToolError> {
    match args.title.trim() {
        "" => Err(ToolError::InvalidArguments {
            tool: "pullRequestCreate".into(),
            reason: "title is empty — write one line saying what the change does".into(),
        }),
        title => Ok(title),
    }
}

/// Where it goes: the branch named, or this chat's worktree's base — the
/// offer's "into its recorded base" — or, with neither, the server's default.
fn target<'a>(args: &'a PullRequestCreateArgs, worktree_base: Option<&'a str>) -> Option<&'a str> {
    args.target.as_deref().map(str::trim).filter(|t| !t.is_empty()).or(worktree_base)
}

/// The approval card: where it goes, from which branch into which, the title,
/// and whether it is a draft — all read here, nothing asked of the server.
/// What the card cannot know (the branch not pushed) the call itself refuses.
pub fn preview(scope: &ToolScope, args: &PullRequestCreateArgs, worktree_base: Option<&str>) -> ToolPreview {
    let title = match title(args) {
        Ok(title) => title,
        Err(e) => return ToolPreview::Failed { reason: e.to_string() },
    };
    let Some(remote) = repository_identity::remote_url(scope.root()).and_then(|url| RemoteRepo::parse(&url).ok()) else {
        return ToolPreview::Failed { reason: "this folder has no remote a server could be found by".to_string() };
    };
    let Some(branch) = git_head::checked_out_branch(scope.root()) else {
        return ToolPreview::Failed { reason: "no branch is checked out".to_string() };
    };
    let target = target(args, worktree_base).unwrap_or("the default branch");
    let mut notes = Vec::new();
    if args.draft == Some(true) {
        notes.push("Opened as a draft: reviewers see it, nothing merges it until it is marked ready.".to_string());
    }
    // Whole: what the user approves is what reviewers read. The card scrolls it.
    if let Some(description) = args.body.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
        notes.push(format!("Description:\n{description}"));
    }
    notes.push("Reviewers and watchers are notified by the server.".to_string());
    ToolPreview::Change {
        place: format!("{}/{} on {}", remote.project, remote.slug, remote.host),
        summary: format!("{branch} → {target}: {title}"),
        notes,
        production: false,
        diffs: Vec::new(),
    }
}

/// What the model reads after it.
pub fn created_text(id: u64, title: &str, from: &str, to: &str, url: &str, draft: bool) -> String {
    let draft = if draft { " as a draft" } else { "" };
    format!(
        "Created pull request #{id} \"{title}\" from {from} into {to}{draft}: {url}\n\
         The user sees it under the branch in Overview; merging and approving are theirs."
    )
}

pub(super) fn definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "pullRequestCreate".to_string(),
        description: "Opens a pull request on the user's Bitbucket from the folder's checked-out branch — the user approves \
            it on a card first. The branch must already be pushed: push it with runCommand (`git push -u origin <branch>`) \
            before, or this is refused. `title`: one line saying what the change does, in the repository's style; \
            `body`: the description — what changed and why, and how it was checked; `target`: the branch to merge into — left out, the \
            branch this chat's worktree was cut from, or else the repository's default branch; `draft`: true when the work is not ready for review yet. Use this rather than \
            `gh`, `curl` or the server's API through runCommand. You cannot merge or approve — those are the user's."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "title": { "type": "string", "description": "One line: what the change does." },
                "body": { "type": ["string", "null"], "description": "The pull request's description: what changed and why, and how it was checked. Markdown." },
                "target": { "type": ["string", "null"], "description": "The branch to merge into. Left out: the worktree's base, or the default branch." },
                "draft": { "type": ["boolean", "null"], "description": "true: open as a draft, not ready for review." }
            },
            "required": ["title"],
            "additionalProperties": false
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::code_host::{AgentPullRequests, CodeHostError, PrStrip, PullRequest};
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    /// The service as the tool sees it: what it was asked to create, and its answer.
    struct Fake {
        answer: Result<PullRequest, CodeHostError>,
        asked: Mutex<Vec<(PathBuf, String, String, Option<String>, bool)>>,
    }

    impl AgentPullRequests for Fake {
        fn status(&self, _: &Path, _: Option<&str>) -> Result<PrStrip, CodeHostError> {
            Err(CodeHostError::NoRemote)
        }
        fn create(&self, root: &Path, title: &str, description: &str, to: Option<&str>, draft: bool) -> Result<PullRequest, CodeHostError> {
            self.asked.lock().unwrap().push((root.to_path_buf(), title.into(), description.into(), to.map(str::to_string), draft));
            self.answer.clone()
        }
    }

    fn created(draft: Option<bool>) -> PullRequest {
        PullRequest {
            id: 154,
            title: "Fix the tax profile".to_string(),
            version: 0,
            author: "u_me".to_string(),
            author_name: "Me".to_string(),
            from_branch: "feature/x".to_string(),
            to_branch: "master".to_string(),
            reviewers: Vec::new(),
            url: "https://git.example/projects/P/repos/r/pull-requests/154".to_string(),
            draft,
        }
    }

    fn args(title: &str, description: Option<&str>, target: Option<&str>, draft: Option<bool>) -> PullRequestCreateArgs {
        PullRequestCreateArgs {
            title: title.to_string(),
            body: description.map(str::to_string),
            target: target.map(str::to_string),
            draft,
        }
    }

    fn run(answer: Result<PullRequest, CodeHostError>, args: &PullRequestCreateArgs) -> (Result<ToolResult, ToolError>, Arc<Fake>) {
        run_in(answer, args, None)
    }

    fn run_in(answer: Result<PullRequest, CodeHostError>, args: &PullRequestCreateArgs, worktree_base: Option<&str>) -> (Result<ToolResult, ToolError>, Arc<Fake>) {
        let fake = Arc::new(Fake { answer, asked: Mutex::default() });
        let scope = ToolScope::new(&crate::testing::temp_dir("pr-create-tool")).unwrap();
        let deps = ToolDeps { pull_requests: Some(fake.clone()), worktree_base, ..ToolDeps::default() };
        (pull_request_create(&scope, args, &deps), fake)
    }

    /// A worktree's chat sends it into the branch the worktree was cut from —
    /// what the Create PR offer says — unless the model names another.
    #[test]
    fn a_worktree_s_pull_request_goes_into_its_base() {
        let (_, fake) = run_in(Ok(created(None)), &args("t", None, None, None), Some("develop"));
        assert_eq!(fake.asked.lock().unwrap()[0].3.as_deref(), Some("develop"));
        let (_, fake) = run_in(Ok(created(None)), &args("t", None, Some(" "), None), Some("develop"));
        assert_eq!(fake.asked.lock().unwrap()[0].3.as_deref(), Some("develop"), "a blank target is none");
        let (_, fake) = run_in(Ok(created(None)), &args("t", None, Some("release/1.2"), None), Some("develop"));
        assert_eq!(fake.asked.lock().unwrap()[0].3.as_deref(), Some("release/1.2"));
    }

    /// What the model wrote goes to the service as written — title trimmed,
    /// the description whole, the target and draft as asked.
    #[test]
    fn the_pull_request_asked_for_is_the_one_sent() {
        let (result, fake) = run(Ok(created(Some(true))), &args("  Fix the tax profile ", Some("Why:\n- because"), Some("release/1.2"), Some(true)));
        let asked = fake.asked.lock().unwrap().clone();
        assert_eq!(asked.len(), 1);
        let (_, title, description, to, draft) = &asked[0];
        assert_eq!((title.as_str(), description.as_str(), to.as_deref(), *draft), ("Fix the tax profile", "Why:\n- because", Some("release/1.2"), true));
        assert_eq!(
            result.unwrap(),
            ToolResult::PullRequestCreated {
                id: 154,
                title: "Fix the tax profile".into(),
                from: "feature/x".into(),
                to: "master".into(),
                url: "https://git.example/projects/P/repos/r/pull-requests/154".into(),
                draft: true,
            }
        );
        let (_, fake) = run(Ok(created(None)), &args("t", None, None, None));
        assert_eq!(fake.asked.lock().unwrap()[0].3, None, "no target is the default branch");
        assert_eq!(fake.asked.lock().unwrap()[0].4, false, "not a draft unless asked");
    }

    #[test]
    fn an_empty_title_is_refused_before_the_server() {
        let (result, fake) = run(Ok(created(None)), &args("  ", None, None, None));
        assert!(matches!(result, Err(ToolError::InvalidArguments { .. })), "{result:?}");
        assert!(fake.asked.lock().unwrap().is_empty());
    }

    /// The server's refusals reach the model as they are: push first, add a server.
    #[test]
    fn a_refusal_is_said() {
        let (result, _) = run(Err(CodeHostError::NotPushed("feature/x".into())), &args("t", None, None, None));
        assert!(result.unwrap_err().to_string().contains("not on the server — push it first"));
        let (result, _) = run(Err(CodeHostError::NoServer("git.corp.example".into())), &args("t", None, None, None));
        assert!(result.unwrap_err().to_string().contains("Catalog → Pull requests"));
    }

    #[test]
    fn without_the_feature_nothing_is_created() {
        let scope = ToolScope::new(&crate::testing::temp_dir("pr-create-off")).unwrap();
        let error = pull_request_create(&scope, &args("t", None, None, None), &ToolDeps::default()).unwrap_err();
        assert!(matches!(error, ToolError::FeatureOff { .. }), "{error}");
    }

    fn repo(label: &str, remote: Option<&str>) -> PathBuf {
        let dir = crate::testing::temp_dir(label);
        let git = git2::Repository::init(&dir).unwrap();
        let signature = git2::Signature::now("t", "t@example.com").unwrap();
        let tree = git.find_tree(git.index().unwrap().write_tree().unwrap()).unwrap();
        let head = git.commit(None, &signature, &signature, "first", &tree, &[]).unwrap();
        let commit = git.find_commit(head).unwrap();
        git.branch("feature/x", &commit, false).unwrap();
        git.set_head("refs/heads/feature/x").unwrap();
        if let Some(remote) = remote {
            git.remote("origin", remote).unwrap();
        }
        dir
    }

    /// The card names the server and repository, the branch into its target,
    /// the title, and a draft and a description when there are — read from
    /// the folder alone.
    #[test]
    fn the_card_says_where_from_where_into_and_what() {
        let dir = repo("pr-create-card", Some("ssh://git@git.corp.example/corp/tax-api.git"));
        let scope = ToolScope::new(&dir).unwrap();
        let ToolPreview::Change { place, summary, notes, production, .. } =
            preview(&scope, &args("Fix it", Some("Because."), Some(" release/1.2 "), Some(true)), Some("develop"))
        else {
            panic!("not a change")
        };
        assert_eq!(place, "corp/tax-api on git.corp.example");
        assert_eq!(summary, "feature/x → release/1.2: Fix it");
        assert!(notes.iter().any(|n| n.starts_with("Opened as a draft")), "{notes:?}");
        assert!(notes.iter().any(|n| n == "Description:\nBecause."), "{notes:?}");
        assert!(!production);

        let ToolPreview::Change { summary, notes, .. } = preview(&scope, &args("Fix it", None, None, None), None) else { panic!() };
        assert_eq!(summary, "feature/x → the default branch: Fix it");
        assert!(!notes.iter().any(|n| n.starts_with("Opened as a draft") || n.starts_with("Description")), "{notes:?}");

        let ToolPreview::Change { summary, .. } = preview(&scope, &args("Fix it", None, None, None), Some("develop")) else { panic!() };
        assert_eq!(summary, "feature/x → develop: Fix it", "a worktree's base");

        // What reviewers will read is what the user approves: all of it.
        let long = format!("{}end", "x".repeat(2000));
        let ToolPreview::Change { notes, .. } = preview(&scope, &args("Fix it", Some(&long), None, None), None) else { panic!() };
        assert!(notes.contains(&format!("Description:\n{long}")), "{notes:?}");
    }

    /// What would fail anyway is said on the card, not after the user agreed.
    #[test]
    fn the_card_refuses_what_cannot_be_sent() {
        let without = ToolScope::new(&repo("pr-create-card-none", None)).unwrap();
        assert!(matches!(preview(&without, &args("Fix it", None, None, None), None), ToolPreview::Failed { .. }));
        let dir = repo("pr-create-card-empty", Some("ssh://git@git.corp.example/corp/tax-api.git"));
        let with = ToolScope::new(&dir).unwrap();
        assert!(matches!(preview(&with, &args(" ", None, None, None), None), ToolPreview::Failed { .. }));

        let git = git2::Repository::open(&dir).unwrap();
        let head = git.head().unwrap().target().unwrap();
        git.set_head_detached(head).unwrap();
        let shown = preview(&with, &args("Fix it", None, None, None), None);
        assert!(matches!(&shown, ToolPreview::Failed { reason } if reason.contains("no branch")), "{shown:?}");
    }

    #[test]
    fn the_model_is_told_what_was_made_and_whose_the_rest_is() {
        let text = created_text(154, "Fix it", "feature/x", "master", "https://u", true);
        assert!(text.starts_with("Created pull request #154 \"Fix it\" from feature/x into master as a draft: https://u"), "{text}");
        assert!(text.contains("merging and approving are theirs"), "{text}");
        assert!(!created_text(1, "t", "a", "b", "u", false).contains("draft"));
    }
}
