//! `kubeRunbook`: the text of one runbook the turn's prompt listed
//! (`domain::runbooks`). It reads no cluster.

use crate::domain::llm::LlmToolDefinition;
use crate::domain::tools::{KubeRunbookArgs, ToolDeps, ToolError, ToolResult};

pub fn kube_runbook(args: &KubeRunbookArgs, deps: &ToolDeps) -> Result<ToolResult, ToolError> {
    let name = args.name.trim();
    match deps.runbooks.iter().find(|runbook| runbook.name == name) {
        Some(runbook) => {
            let summary = if runbook.own { "the user's" } else { "built in" };
            Ok(ToolResult::Kube { text: runbook.text.clone(), summary: summary.to_string() })
        }
        None => {
            let names: Vec<&str> = deps.runbooks.iter().map(|runbook| runbook.name.as_str()).collect();
            Err(ToolError::InvalidArguments { tool: "kubeRunbook".to_string(), reason: format!("there is no runbook `{name}` — there are: {}", names.join(", ")) })
        }
    }
}

pub(super) fn definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "kubeRunbook".to_string(),
        description: "Reads one runbook from the list in your instructions: how that kind of failure is taken apart \
                      here — what to check, in which order, with which of your tools, and what not to do. Call it \
                      when you see a runbook's sign, before digging; once per conversation is enough."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": { "name": { "type": "string", "description": "A name from the list of runbooks." } },
            "required": ["name"]
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::runbooks::{merged, parse};

    #[test]
    fn a_runbook_is_read_by_name_and_a_wrong_name_is_told_the_right_ones() {
        let deps = ToolDeps { runbooks: merged(vec![parse("pending", "Sign: ours\nIt is the quota.", true).unwrap()]), ..ToolDeps::default() };
        let read = |name: &str| kube_runbook(&KubeRunbookArgs { name: name.into() }, &deps);
        assert_eq!(read(" pending ").unwrap(), ToolResult::Kube { text: "Sign: ours\nIt is the quota.".into(), summary: "the user's".into() });
        let ToolResult::Kube { text, summary } = read("oom-killed").unwrap() else { panic!() };
        assert!(text.starts_with("# OOMKilled") && summary == "built in", "{summary}: {text}");
        let Err(ToolError::InvalidArguments { reason, .. }) = read("oom") else { panic!() };
        assert!(reason.starts_with("there is no runbook `oom` — there are: crashloop, image-pull, pending, "), "{reason}");
    }
}
