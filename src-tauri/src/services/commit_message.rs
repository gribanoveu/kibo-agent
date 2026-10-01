//! "Generate description" in the Changes tab: one request to the model, from
//! what is staged, the branch's ticket key and the repository's recent
//! subjects. The rules for what it is shown are in `domain::commit_message`.

use std::path::Path;

use crate::domain::commit_message::{self, CommitContext, CommitMessageError, RECENT_SUBJECTS};
use crate::domain::git_changes::GitChangesError;
use crate::domain::llm::{ChatRequest, LlmMessage};
use crate::infra::{git_changes, git_head, llm_debug_log};
use crate::services::llm_session::LlmSession;
use crate::services::project_rules;

/// A message for what is staged in the repository `root` is in. `draft` is
/// what the box already holds; the model keeps its intent.
pub fn generate(session: &LlmSession, root: &Path, draft: &str) -> Result<String, CommitMessageError> {
    let staged = git_changes::staged_patches(root)?;
    if staged.is_empty() {
        return Err(GitChangesError::NothingStaged.into());
    }
    // No history is not a failure: a first commit has none to follow.
    let recent: Vec<String> = git_changes::history(root, RECENT_SUBJECTS)
        .map(|h| h.commits.into_iter().map(|c| c.summary).collect())
        .unwrap_or_default();
    let ticket = git_head::current_branch(root).and_then(|b| commit_message::ticket_from_branch(&b));

    let request = ChatRequest {
        messages: vec![
            LlmMessage::system(commit_message::system_prompt(&project_rules::load(root))),
            LlmMessage::user(commit_message::render_request(&CommitContext {
                staged: &staged,
                recent: &recent,
                ticket: ticket.as_deref(),
                draft,
            })),
        ],
        tools: Vec::new(),
        model: session.model.clone(),
    };

    llm_debug_log::log_request(session.debug_logging, &session.provider_id, 0, &request);
    let response = session.provider.chat(request);
    llm_debug_log::log_response(session.debug_logging, &session.provider_id, 0, &response);

    commit_message::clean_reply(&response?.content.unwrap_or_default()).ok_or(CommitMessageError::EmptyReply)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::llm::{ChatResponse, ChatStreamResult, LlmError, LlmModelInfo, LlmProvider};
    use crate::testing::temp_dir;
    use git2::Repository;
    use std::fs;
    use std::sync::{Arc, Mutex};

    struct Writer {
        answer: String,
        asked: Mutex<Vec<ChatRequest>>,
    }

    impl LlmProvider for Writer {
        fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
            self.asked.lock().unwrap().push(request);
            Ok(ChatResponse { content: Some(self.answer.clone()), tool_calls: Vec::new(), usage: None })
        }

        fn chat_stream(
            &self,
            _: ChatRequest,
            _: &dyn Fn(&str),
            _: &dyn Fn(&str),
            _: &dyn Fn(&str, &str, &str),
            _: &dyn Fn() -> bool,
        ) -> Result<ChatStreamResult, LlmError> {
            unreachable!("a commit message is never streamed")
        }

        fn list_models(&self) -> Result<Vec<LlmModelInfo>, LlmError> {
            unreachable!("a commit message never lists models")
        }
    }

    fn session(answer: &str) -> (LlmSession, Arc<Writer>) {
        let writer = Arc::new(Writer { answer: answer.to_string(), asked: Mutex::new(Vec::new()) });
        let session = LlmSession {
            provider: writer.clone(),
            provider_id: "test".to_string(),
            model: "m".to_string(),
            debug_logging: false,
            context_limit: None,
            reply_language: None,
            limits: Default::default(),
        };
        (session, writer)
    }

    /// A repository with one commit, on `branch`, with `a.txt` edited and staged.
    fn repo_on(branch: &str) -> std::path::PathBuf {
        let dir = temp_dir("commit-message");
        let repo = Repository::init(&dir).unwrap();
        let mut config = repo.config().unwrap();
        config.set_str("user.name", "Test").unwrap();
        config.set_str("user.email", "test@example.com").unwrap();
        fs::write(dir.join("a.txt"), "one\n").unwrap();
        git_changes::stage(&dir, &["a.txt".into()]).unwrap();
        git_changes::commit(&dir, "feat(OLD-1): first").unwrap();
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        repo.branch(branch, &head, false).unwrap();
        repo.set_head(&format!("refs/heads/{branch}")).unwrap();
        fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
        git_changes::stage(&dir, &["a.txt".into()]).unwrap();
        dir
    }

    #[test]
    fn the_model_sees_the_ticket_the_history_and_the_staged_diff() {
        let dir = repo_on("feature/KEY-42-login");
        let (session, writer) = session("```\nfeat(KEY-42): add two\n```");

        let message = generate(&session, &dir, "add two").unwrap();

        assert_eq!(message, "feat(KEY-42): add two");
        let asked = writer.asked.lock().unwrap();
        assert_eq!(asked.len(), 1);
        assert!(asked[0].tools.is_empty());
        let user = asked[0].messages[1].content.as_deref().unwrap();
        assert!(user.starts_with("Ticket key: KEY-42\n"));
        assert!(user.contains("- feat(OLD-1): first\n"));
        assert!(user.contains("keep its intent:\nadd two\n"));
        assert!(user.contains("--- a.txt\n@@ -1 +1,2 @@\n one\n+two\n"));
    }

    #[test]
    fn nothing_staged_is_refused_before_the_model_is_asked() {
        let dir = repo_on("main-work");
        git_changes::commit(&dir, "second").unwrap();
        let (session, writer) = session("feat: x");

        let err = generate(&session, &dir, "").unwrap_err();

        assert!(matches!(err, CommitMessageError::Git(GitChangesError::NothingStaged)));
        assert!(writer.asked.lock().unwrap().is_empty());
    }

    #[test]
    fn an_empty_reply_is_an_error_not_an_empty_message() {
        let dir = repo_on("main-work");
        let (session, _) = session("  \n");
        assert!(matches!(generate(&session, &dir, ""), Err(CommitMessageError::EmptyReply)));
    }
}
