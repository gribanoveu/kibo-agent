//! `explore` — research handed to a helper turn with a context of its own.
//!
//! The helper reads and searches in `ConversationMode::Explore`, and only its
//! closing answer comes back: the files it went through never enter the
//! calling turn's history. Running it needs the model, which the tool layer
//! does not have — `ToolDeps::explore` is the loop's port for it.

use crate::domain::llm::LlmToolDefinition;
use crate::domain::tools::{ExploreArgs, ToolDeps, ToolError, ToolResult};

pub fn explore(args: &ExploreArgs, deps: &ToolDeps) -> Result<ToolResult, ToolError> {
    if args.task.trim().is_empty() {
        return Err(ToolError::Explore("the task is empty — say what to find out".to_string()));
    }
    let run = deps.explore.ok_or_else(|| ToolError::Explore("not available here".to_string()))?;
    run(&args.task)
}

pub(super) fn definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "explore".to_string(),
        description: "Hand a research task to a helper agent with a fresh context. It searches and reads the repository on its own and returns only its answer, so the reading does not fill this conversation. Use it for open questions that take many searches and reads: how a flow works across files, where something is decided, every place a pattern is used. Not for a file or a name you can find in one or two calls. The helper sees nothing of this conversation: the task must say what to find out, what you already know, and what to report (paths and line numbers, not whole files). It can only read; it cannot change files or run commands. Several explore calls in one reply run at the same time: when there are questions that do not depend on each other's answers, ask them together, one task each."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "task": { "type": "string", "description": "The whole brief: the question, what is already known, and what the answer should contain." }
            },
            "required": ["task"]
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(text: &str) -> ExploreArgs {
        ExploreArgs { task: text.to_string() }
    }

    fn answered(text: String) -> ToolResult {
        ToolResult::Explored { text, agent: 1, tokens: Default::default() }
    }

    #[test]
    fn the_task_goes_to_the_helper_and_its_answer_comes_back() {
        let run = |task: &str| Ok(answered(format!("answer to: {task}")));
        let deps = ToolDeps { explore: Some(&run), ..ToolDeps::default() };
        assert_eq!(explore(&task("where is X"), &deps).unwrap(), answered("answer to: where is X".to_string()));
    }

    #[test]
    fn an_empty_task_or_no_helper_is_an_error_the_model_can_read() {
        let run = |_: &str| -> Result<ToolResult, ToolError> { panic!("an empty task must not reach the helper") };
        let deps = ToolDeps { explore: Some(&run), ..ToolDeps::default() };
        assert!(explore(&task("  "), &deps).unwrap_err().to_string().contains("the task is empty"));
        assert!(explore(&task("where is X"), &ToolDeps::default()).unwrap_err().to_string().contains("not available"));
    }
}
