//! What the agent is allowed to be this turn.
//!
//! A third axis, independent of the two that already exist. [`ToolScope`] says
//! *where* a tool may reach — a boundary, enforced against the filesystem.
//! [`ApprovalPolicy`] says *who has to agree* before a call runs. This says
//! which tools are offered at all, and it is the user's choice about the
//! conversation rather than a security control: someone thinking a change
//! through does not want the agent making it halfway through the thought.
//!
//! Picked once for the whole app and remembered (`AppSettings::conversation_mode`):
//! an analyst is a person, not a repository, so every folder they open starts
//! as the last one did. A restriction kept like that could be forgotten, which
//! is why the line under the message box always says the mode.
//!
//! The narrower modes are a real restriction, not advice. A tool the mode does
//! not offer is left out of the request *and* refused if the model asks for it
//! anyway: a model that has seen a tool earlier in a conversation will call it
//! again from memory, and "please don't" in a prompt is not a gate.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use super::tools::ToolName;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConversationMode {
    /// Everything: read, change, run.
    #[default]
    Agent,
    /// Questions about the system, asked by someone who is not its developer
    /// and answered from the code. Nothing that touches the working tree, and
    /// no commands — see [`tools`] for why running one is not a read.
    Analyst,
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
        ConversationMode::Analyst,
        ConversationMode::Review,
        ConversationMode::Explore,
    ];
}

/// The modes the user picks: all the window can send, so the others can be
/// neither chosen nor kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PickedMode {
    #[default]
    Agent,
    Analyst,
}

impl PickedMode {
    pub const ALL: &'static [PickedMode] = &[PickedMode::Agent, PickedMode::Analyst];

    /// As the user reads it.
    pub fn name(self) -> &'static str {
        match self {
            PickedMode::Agent => "Agent",
            PickedMode::Analyst => "Analyst",
        }
    }
}

impl From<PickedMode> for ConversationMode {
    fn from(mode: PickedMode) -> Self {
        match mode {
            PickedMode::Agent => ConversationMode::Agent,
            PickedMode::Analyst => ConversationMode::Analyst,
        }
    }
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
        // The same through DeepSeek, while the user has that feature on.
        ToolName::DeepseekSearch,
        ToolName::DeepseekFetch,
        // The MongoDB connections the user opened to the agent, read, while
        // the feature is on: a question about data is asked beside the code.
        ToolName::Mongo,
    ]
    .into_iter()
    .collect()
}

/// Every tool reachable in `mode`.
///
/// `runCommand` is out of `Analyst`, and that is a judgement call.
/// Running the project's tests would be useful to an answer, but nothing in
/// this app can tell `cargo test` from `rm -rf` before the line runs — the
/// argument is an opaque string, and `domain::command_exec` says so. A mode
/// whose promise is "the code stays as it is" cannot keep it while offering a
/// tool that might. The cost is real and recorded: an answer cannot check
/// itself against a build.
///
/// `Mcp` is the other, and goes the other way. It is in `Analyst` though it is
/// mutating: an analyst's tracker and wiki are there. A server is the user's
/// own choice, not the repository's code, and a call to one asks as in `Agent`
/// — unless the user set Auto or always allowed it.
pub fn tools(mode: ConversationMode) -> HashSet<ToolName> {
    let mut tools = base_tools();
    match mode {
        ConversationMode::Agent => {
            tools.extend([
                ToolName::WriteFile,
                ToolName::EditFile,
                ToolName::Delete,
                ToolName::Move,
                ToolName::Todo,
                ToolName::RunCommand,
                ToolName::StopProcess,
                ToolName::WritePlan,
                // A next step put to the user, not taken (`docs/43-single-loop.md`, Ш6).
                ToolName::Offer,
                ToolName::Mcp,
                // Where the MCP tools are, so is the way to the deferred ones.
                ToolName::ToolSearch,
                ToolName::Explore,
                // A change reviewed inside the turn, transparent like explore.
                ToolName::Review,
                ToolName::Remember,
                // While the user has the feature on (`domain::features`).
                ToolName::Board,
                // Here and not in `base_tools`: with HC-8 it writes, and Analyst gets nothing that does.
                ToolName::HttpEnvironment,
                ToolName::HttpSend,
                ToolName::HttpReadRequest,
                ToolName::HttpSaveRequest,
                ToolName::HttpDraftScript,
            ]);
        }
        // Research handed off reads the same files, only somewhere else.
        // `remember` writes the agent's memory, not the tree: "remember
        // that" is something the user says in any mode they talk in — and
        // "remind me", which is the board's.
        ConversationMode::Analyst => {
            tools.extend([ToolName::Explore, ToolName::Review, ToolName::Remember, ToolName::Board, ToolName::Mcp, ToolName::ToolSearch]);
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
    /// this test. The one exception is MCP in `Analyst`: mutating because
    /// nothing is known about a foreign tool, offered because that is where an
    /// analyst's tracker and wiki are, and held by the approval gate instead.
    #[test]
    fn nothing_that_changes_the_tree_is_offered_outside_agent_mode() {
        for &tool in ToolName::ALL {
            if tool.is_mutating() {
                assert!(!offers(ConversationMode::Explore, tool), "a helper offers {}", tool.wire_name());
                if tool != ToolName::Mcp {
                    assert!(!offers(ConversationMode::Analyst, tool), "Analyst offers {}, which changes the tree", tool.wire_name());
                }
            }
        }
    }

    /// What Analyst has beyond every mode's reading, and nothing else: a
    /// helper's hand-offs (research, a review) and its own memory, board and
    /// servers.
    #[test]
    fn an_analyst_gets_the_helper_handoffs_memory_board_and_servers() {
        let mut expected = base_tools();
        expected.extend([ToolName::Explore, ToolName::Review, ToolName::Remember, ToolName::Board, ToolName::Mcp, ToolName::ToolSearch]);
        assert_eq!(tools(ConversationMode::Analyst), expected);
    }

    /// The modes the user picks. A review worker's set is its own, pinned by
    /// `a_review_reads_and_reports_and_changes_nothing`, and so is a helper's.
    fn picked() -> impl Iterator<Item = ConversationMode> {
        PickedMode::ALL.iter().map(|&mode| mode.into())
    }

    /// A helper reads what every mode reads, and nothing more: no checklist,
    /// and no `explore` — one helper sending another has no floor.
    #[test]
    fn a_helper_reads_and_cannot_hand_off_again() {
        assert_eq!(tools(ConversationMode::Explore), base_tools());
        assert!(!offers(ConversationMode::Explore, ToolName::Explore));
        for mode in picked() {
            assert!(offers(mode, ToolName::Explore), "{mode:?}");
        }
        assert!(!offers(ConversationMode::Review, ToolName::Explore));
    }

    /// A review runs in the modes the user talks in — and not inside a
    /// helper: no review of a review, none beside an explore.
    #[test]
    fn a_review_runs_in_picked_modes_and_not_inside_a_helper() {
        for mode in picked() {
            assert!(offers(mode, ToolName::Review), "{mode:?}");
        }
        assert!(!offers(ConversationMode::Explore, ToolName::Review));
        assert!(!offers(ConversationMode::Review, ToolName::Review));
    }

    /// Looking at the user's terminal is reading; typing into it is not.
    #[test]
    fn the_terminal_is_read_in_every_mode_and_typed_into_in_agent_alone() {
        for mode in picked() {
            assert!(offers(mode, ToolName::ReadTerminal), "{mode:?}");
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
            assert!(offers(mode, ToolName::ReadOutput), "{mode:?}");
        }
        assert!(offers(ConversationMode::Agent, ToolName::StopProcess));
        assert!(!offers(ConversationMode::Analyst, ToolName::StopProcess));
        assert!(!offers(ConversationMode::Analyst, ToolName::RunCommand));
    }

    /// A mode that cannot read the repository has nothing to be a mode about.
    #[test]
    fn every_mode_can_look_at_the_repository() {
        for mode in picked() {
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
    /// the Kubernetes role's cluster tools — the agent has no cluster pinned
    /// to it — and the Prototype role's `present`, which keeps a version of a
    /// chat's folder the agent does not have. The roles' web tools it shares.
    #[test]
    fn agent_mode_offers_every_tool_there_is() {
        let agent = tools(ConversationMode::Agent);
        let cluster: Vec<ToolName> =
            crate::domain::chat_role::ChatRole::Kubernetes.tools().iter().copied().filter(|tool| !tool.is_web()).collect();
        assert_eq!(agent.len(), ToolName::ALL.len() - 2 - cluster.len());
        assert!(!agent.contains(&ToolName::ReportFinding));
        assert!(!agent.contains(&ToolName::Present));
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
        let writes = [ToolName::WriteFile, ToolName::EditFile, ToolName::Delete, ToolName::Move];
        assert!(writes.iter().all(|tool| !review.contains(tool)));
        assert!(!review.contains(&ToolName::WritePlan) && !review.contains(&ToolName::Todo));
    }


    /// A plan is a document and a list of steps, and the button that starts
    /// it; a question needs none of them.
    #[test]
    fn a_plan_is_agents_and_a_question_has_no_use_for_one() {
        for tool in [ToolName::Todo, ToolName::WritePlan, ToolName::Offer] {
            assert!(offers(ConversationMode::Agent, tool), "{tool:?}");
            assert!(!offers(ConversationMode::Analyst, tool), "{tool:?}");
        }
    }

    /// "Remember that" is said in whatever mode the user talks in; a helper
    /// and a reviewer have no one to remember it for.
    #[test]
    fn memory_is_kept_where_the_user_talks_to_the_agent() {
        for mode in picked() {
            assert!(offers(mode, ToolName::Remember), "{mode:?}");
        }
        assert!(!offers(ConversationMode::Explore, ToolName::Remember));
        assert!(!offers(ConversationMode::Review, ToolName::Remember));
    }

    /// The wire name is what the window sends; a rename here is a mode the
    /// frontend can no longer select.
    #[test]
    fn the_wire_names_are_the_ones_the_window_sends() {
        for (mode, wire) in [(PickedMode::Agent, "\"agent\""), (PickedMode::Analyst, "\"analyst\"")] {
            assert_eq!(serde_json::to_string(&mode).unwrap(), wire);
            assert_eq!(serde_json::from_str::<PickedMode>(wire).unwrap(), mode);
        }
    }

    /// What the user cannot pick cannot be sent: a review, a helper, and the
    /// modes that are gone.
    #[test]
    fn the_window_cannot_send_any_other_mode() {
        for wire in ["\"review\"", "\"explore\"", "\"plan\"", "\"ask\""] {
            assert!(serde_json::from_str::<PickedMode>(wire).is_err(), "{wire}");
        }
    }

    /// Each picked mode is the mode of that name, not the first one.
    #[test]
    fn a_picked_mode_is_the_mode_it_names() {
        assert_eq!(ConversationMode::from(PickedMode::Agent), ConversationMode::Agent);
        assert_eq!(ConversationMode::from(PickedMode::Analyst), ConversationMode::Analyst);
    }

    /// Nothing selected is the full harness: this app is an agent, and a
    /// default that silently disarmed it would look like a broken tool loop.
    #[test]
    fn the_default_is_the_full_harness() {
        assert_eq!(ConversationMode::default(), ConversationMode::Agent);
        assert_eq!(PickedMode::default(), PickedMode::Agent);
    }
}
