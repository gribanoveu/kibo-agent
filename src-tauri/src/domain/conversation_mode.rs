//! What the agent is allowed to be this turn.
//!
//! A third axis, independent of the two that already exist. [`ToolScope`] says
//! *where* a tool may reach — a boundary, enforced against the filesystem.
//! [`ApprovalPolicy`] says *who has to agree* before a call runs. This says
//! which tools are offered at all, and it is the user's choice about the
//! conversation rather than a security control: someone thinking a change
//! through does not want the agent making it halfway through the thought.
//!
//! Chosen per turn and never persisted. The frontend sends it with each
//! request, the way Alfa Atlas does — a mode that outlives the app is a
//! restriction switched on in March and wondered about in June.
//!
//! The narrower modes are a real restriction, not advice. A tool the mode does
//! not offer is left out of the request *and* refused if the model asks for it
//! anyway: a model that has seen a tool earlier in a conversation will call it
//! again from memory, and "please don't" in a prompt is not a gate.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use super::tools::ToolName;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConversationMode {
    /// Everything: read, change, run.
    #[default]
    Agent,
    /// Read and think. Nothing that touches the working tree, and no
    /// commands — see [`tools`] for why running one is not a read.
    Plan,
    /// Questions about the code, answered from the code. The leanest set:
    /// no checklist, nothing to change.
    Ask,
    /// `/review`: the uncommitted change read, checked and reported on.
    /// Not a mode the user picks: the command starts its turn in it.
    Review,
    /// The helper turn `explore` runs: reading only, and no `explore` of its
    /// own, so a helper never sends another. Not a mode the user picks.
    Explore,
}

impl ConversationMode {
    pub const ALL: &'static [ConversationMode] = &[
        ConversationMode::Agent,
        ConversationMode::Plan,
        ConversationMode::Ask,
        ConversationMode::Review,
        ConversationMode::Explore,
    ];
}

/// Offered in every mode: looking, searching, and reading git history. None of
/// it changes anything, and a mode that could not read would have nothing to
/// be a mode about.
fn base_tools() -> HashSet<ToolName> {
    [
        ToolName::ReadFile,
        ToolName::Grep,
        ToolName::ListFiles,
        ToolName::GitStatus,
        ToolName::GitDiff,
        ToolName::GitBlame,
        ToolName::GitLog,
        ToolName::SemanticSearch,
        ToolName::Skill,
        // A process's output is something to look at; one may still run
        // from an Agent turn before the mode changed.
        ToolName::ReadOutput,
        // The user's own screen, looked at.
        ToolName::ReadTerminal,
        // The web, read: offered only while Settings → Web search gives the
        // agent a search (`llm_chat::tool_definitions_for`).
        ToolName::WebSearch,
        ToolName::WebFetch,
    ]
    .into_iter()
    .collect()
}

/// Every tool reachable in `mode`.
///
/// `runCommand` is in `Agent` alone, and that is the one judgement call here.
/// Running the project's tests would be useful while planning, but nothing in
/// this app can tell `cargo test` from `rm -rf` before the line runs — the
/// argument is an opaque string, and `domain::command_exec` says so. A mode
/// whose whole promise is "nothing will change" cannot keep it while offering
/// a tool that might. The cost is real and recorded: a plan cannot check
/// itself against a build.
///
/// `todo` and `writePlan` are in `Plan` because a plan is a document and a
/// list of steps. Both are chat state, saved with the chat, and carry over
/// when the user hands the plan to Agent mode; neither touches the tree.
pub fn tools(mode: ConversationMode) -> HashSet<ToolName> {
    let mut tools = base_tools();
    match mode {
        ConversationMode::Agent => {
            tools.extend([
                ToolName::WriteFile,
                ToolName::EditFile,
                ToolName::DeleteFile,
                ToolName::CreateDirectory,
                ToolName::DeleteDirectory,
                ToolName::Move,
                ToolName::Todo,
                ToolName::RunCommand,
                // The same line, typed where the user can see it run.
                ToolName::RunInTerminal,
                ToolName::StopProcess,
                ToolName::WritePlan,
                // Agent only: nothing says a foreign tool changes nothing.
                ToolName::Mcp,
                // Where the MCP tools are, so is the way to the deferred ones.
                ToolName::ToolSearch,
                ToolName::Explore,
                ToolName::Remember,
            ]);
        }
        // `writePlan` is chat state, not the working tree: writing the plan
        // is the one thing Plan mode is for. Agent has it too, so a plan
        // that meets reality can be corrected where the user reads it.
        ConversationMode::Plan => {
            tools.extend([ToolName::Todo, ToolName::WritePlan, ToolName::Explore, ToolName::Remember]);
        }
        // Research handed off reads the same files, only somewhere else.
        // `remember` writes the agent's memory, not the tree: "remember
        // that" is something the user says in any mode they talk in.
        ConversationMode::Ask => {
            tools.extend([ToolName::Explore, ToolName::Remember]);
        }
        // Reading, running what proves something — a build, the tests — and
        // saying what is wrong. Nothing that writes a file: a review that
        // fixes as it goes is no longer a review of the change.
        ConversationMode::Review => {
            tools.extend([ToolName::RunCommand, ToolName::StopProcess, ToolName::ReportFinding]);
        }
        ConversationMode::Explore => {}
    }
    tools
}

/// Whether this mode offers this tool. The gate, asked in two places: once
/// when the request is built, once before a call runs.
pub fn offers(mode: ConversationMode, tool: ToolName) -> bool {
    tools(mode).contains(&tool)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The promise the mode makes. Every tool that changes the working tree,
    /// checked against the enum rather than a list written out here — a tool
    /// added later is in `ToolName::ALL` whether or not anybody remembers
    /// this test.
    #[test]
    fn nothing_that_changes_the_tree_is_offered_outside_agent_mode() {
        for mode in [ConversationMode::Plan, ConversationMode::Ask, ConversationMode::Explore] {
            for &tool in ToolName::ALL {
                if tool.is_mutating() {
                    assert!(
                        !offers(mode, tool),
                        "{mode:?} offers {}, which changes the tree",
                        tool.wire_name()
                    );
                }
            }
        }
    }

    /// The modes the user picks. A review worker's set is its own, pinned by
    /// `a_review_reads_and_reports_and_changes_nothing`, and so is a helper's.
    fn picked() -> impl Iterator<Item = &'static ConversationMode> {
        ConversationMode::ALL
            .iter()
            .filter(|mode| !matches!(mode, ConversationMode::Review | ConversationMode::Explore))
    }

    /// A helper reads what every mode reads, and nothing more: no checklist,
    /// and no `explore` — one helper sending another has no floor.
    #[test]
    fn a_helper_reads_and_cannot_hand_off_again() {
        assert_eq!(tools(ConversationMode::Explore), base_tools());
        assert!(!offers(ConversationMode::Explore, ToolName::Explore));
        for mode in picked() {
            assert!(offers(*mode, ToolName::Explore), "{mode:?}");
        }
        assert!(!offers(ConversationMode::Review, ToolName::Explore));
    }

    /// Looking at the user's terminal is reading; typing into it is not.
    #[test]
    fn the_terminal_is_read_in_every_mode_and_typed_into_in_agent_alone() {
        for mode in picked() {
            assert!(offers(*mode, ToolName::ReadTerminal), "{mode:?}");
            assert_eq!(offers(*mode, ToolName::RunInTerminal), *mode == ConversationMode::Agent, "{mode:?}");
        }
    }

    /// The judgement call, written down so changing it is deliberate: a
    /// command line is an opaque string, so a mode that promises to change
    /// nothing cannot offer one.
    #[test]
    fn no_mode_but_agent_can_run_a_command() {
        assert!(offers(ConversationMode::Agent, ToolName::RunCommand));
        // Reading a process's output changes nothing; stopping one does.
        for mode in picked() {
            assert!(offers(*mode, ToolName::ReadOutput), "{mode:?}");
        }
        assert!(offers(ConversationMode::Agent, ToolName::StopProcess));
        assert!(!offers(ConversationMode::Plan, ToolName::StopProcess));
        assert!(!offers(ConversationMode::Ask, ToolName::StopProcess));
        assert!(!offers(ConversationMode::Plan, ToolName::RunCommand));
        assert!(!offers(ConversationMode::Ask, ToolName::RunCommand));
    }

    /// A mode that cannot read the repository has nothing to be a mode about.
    #[test]
    fn every_mode_can_look_at_the_repository() {
        for &mode in picked() {
            for tool in [
                ToolName::ReadFile,
                ToolName::Grep,
                ToolName::ListFiles,
                ToolName::GitStatus,
                ToolName::GitDiff,
                ToolName::GitBlame,
                ToolName::GitLog,
                ToolName::SemanticSearch,
            ] {
                assert!(offers(mode, tool), "{mode:?} cannot use {}", tool.wire_name());
            }
        }
    }

    /// The web tools only read, so every folder mode may have them; whether
    /// it does is Settings' — the agent's search, on or off.
    #[test]
    fn every_folder_mode_may_search_the_web() {
        for &mode in ConversationMode::ALL {
            assert!(offers(mode, ToolName::WebSearch) && offers(mode, ToolName::WebFetch), "{mode:?}");
        }
    }

    /// Every tool but a review's own — a finding needs a review to be about —
    /// and the Kubernetes role's cluster tools: the agent has no cluster
    /// pinned to it. The role's web tools it shares.
    #[test]
    fn agent_mode_offers_every_tool_there_is() {
        let agent = tools(ConversationMode::Agent);
        let cluster: Vec<ToolName> =
            crate::domain::chat_role::ChatRole::Kubernetes.tools().iter().copied().filter(|tool| !tool.is_web()).collect();
        assert_eq!(agent.len(), ToolName::ALL.len() - 1 - cluster.len());
        assert!(!agent.contains(&ToolName::ReportFinding));
        assert!(cluster.iter().all(|tool| !agent.contains(tool)));
    }

    /// A reviewer reads everything, runs commands to prove a point, and
    /// reports; nothing it has writes a file.
    #[test]
    fn a_review_reads_runs_and_reports_but_writes_nothing() {
        let review = tools(ConversationMode::Review);
        for tool in [ToolName::ReadFile, ToolName::Grep, ToolName::SemanticSearch, ToolName::GitDiff, ToolName::RunCommand, ToolName::ReportFinding] {
            assert!(review.contains(&tool), "{tool:?}");
        }
        let writes = [ToolName::WriteFile, ToolName::EditFile, ToolName::DeleteFile, ToolName::CreateDirectory, ToolName::DeleteDirectory, ToolName::Move];
        assert!(writes.iter().all(|tool| !review.contains(tool)));
        assert!(!review.contains(&ToolName::WritePlan) && !review.contains(&ToolName::Todo));
    }


    /// A plan is a document and a list of steps; a question needs neither.
    #[test]
    fn a_plan_can_keep_a_checklist_and_a_question_has_no_use_for_one() {
        assert!(offers(ConversationMode::Plan, ToolName::Todo));
        assert!(offers(ConversationMode::Plan, ToolName::WritePlan));
        assert!(!offers(ConversationMode::Ask, ToolName::WritePlan));
        assert!(!offers(ConversationMode::Ask, ToolName::Todo));
    }

    /// "Remember that" is said in whatever mode the user talks in; a helper
    /// and a reviewer have no one to remember it for.
    #[test]
    fn memory_is_kept_where_the_user_talks_to_the_agent() {
        for mode in [ConversationMode::Agent, ConversationMode::Plan, ConversationMode::Ask] {
            assert!(offers(mode, ToolName::Remember), "{mode:?}");
        }
        assert!(!offers(ConversationMode::Explore, ToolName::Remember));
        assert!(!offers(ConversationMode::Review, ToolName::Remember));
    }

    /// The narrower modes are narrower. Without this, a set that quietly grew
    /// back to everything would pass every test above.
    #[test]
    fn each_mode_is_strictly_smaller_than_the_last() {
        assert!(
            tools(ConversationMode::Ask).len() < tools(ConversationMode::Plan).len(),
            "ask is not leaner than plan"
        );
        assert!(
            tools(ConversationMode::Plan).len() < tools(ConversationMode::Agent).len(),
            "plan is not leaner than agent"
        );
    }

    /// The wire name is what the window sends; a rename here is a mode the
    /// frontend can no longer select.
    #[test]
    fn the_wire_names_are_the_ones_the_window_sends() {
        for (mode, wire) in [
            (ConversationMode::Agent, "\"agent\""),
            (ConversationMode::Plan, "\"plan\""),
            (ConversationMode::Ask, "\"ask\""),
        ] {
            assert_eq!(serde_json::to_string(&mode).unwrap(), wire);
        }
    }

    /// Nothing selected is the full harness: this app is an agent, and a
    /// default that silently disarmed it would look like a broken tool loop.
    #[test]
    fn the_default_is_the_full_harness() {
        assert_eq!(ConversationMode::default(), ConversationMode::Agent);
    }
}
