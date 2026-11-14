//! Which calls the user is asked about, and what "always allow" widens.

use super::*;

/// When the agent may act without asking.
///
/// Two independent relaxations, both narrowing to "ask about everything that
/// changes the tree" when left at their defaults.
#[derive(Debug, Clone, Default)]
pub struct ApprovalPolicy {
    /// Tools the user answered "always allow" for. Per tool, not per call.
    /// Never holds `ToolName::Mcp`: that would be every tool of every
    /// server — see [`ApprovalPolicy::allow_always`].
    pub always_allowed: HashSet<ToolName>,
    /// The same for MCP tools, by full name (`mcp__github__create_issue`).
    pub always_allowed_mcp: HashSet<String>,
    /// Run the whole turn unattended. Deliberately not part of persisted
    /// settings: a saved "never ask me" is a brake released a month ago and
    /// forgotten. The turn that ran under it has to say so in its transcript —
    /// that is the loop's job, not this struct's.
    pub skip_all: bool,
    /// The workspace's git aliases, for reading `git <alias>` as what it
    /// runs — loaded for each turn by the command layer.
    pub git_aliases: command_risk::GitAliases,
}

impl ApprovalPolicy {
    /// `risky` is the call's own verdict. Every tool here answers it with
    /// [`ToolName::is_mutating`]; `runCommand` will answer it by looking at the
    /// command line. Passing it in rather than deriving it keeps this gate the
    /// single place the three relaxations compose, and spares the call sites a
    /// rewrite when the first argument-dependent tool arrives.
    pub fn requires_approval(&self, name: ToolName, risky: bool) -> bool {
        risky && !self.skip_all && !self.always_allowed.contains(&name)
    }

    /// The gate for a parsed call — the one the loop asks. An MCP call is
    /// looked up by its own name, so "always allow" for one tool of a server
    /// leaves the rest of that server asking.
    pub fn requires_approval_for(&self, call: &ToolCall) -> bool {
        match call {
            // Off the machine, or past undoing, asks even under "always
            // allow runCommand" — CA-12.6, F-2.5. Auto still means not asking.
            ToolCall::RunCommand(request) => match command_risk::classify_with(&request.command, &self.git_aliases) {
                CommandRisk::ReadOnly => false,
                CommandRisk::AlwaysAsk(_) => !self.skip_all,
                CommandRisk::Ask => self.requires_approval(ToolName::RunCommand, true),
            },
            ToolCall::Mcp(args) => {
                call.is_risky() && !self.skip_all && !self.always_allowed_mcp.contains(&args.name)
            }
            _ => self.requires_approval(call.name(), call.is_risky()),
        }
    }

    /// Why this call asks when it asks regardless of "always allow" — shown
    /// on its card, so a card that appears despite the answer is explained.
    pub fn approval_reason(&self, call: &ToolCall) -> Option<String> {
        match call {
            ToolCall::RunCommand(crate::domain::command_exec::CommandRequest { command, .. }) => {
                match command_risk::classify_with(command, &self.git_aliases) {
                    CommandRisk::AlwaysAsk(why) => Some(why),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// "Always allow" from an approval card, by the name the call carried.
    pub fn allow_always(&mut self, wire_name: &str) -> Result<(), String> {
        if wire_name.starts_with(MCP_PREFIX) {
            self.always_allowed_mcp.insert(wire_name.to_string());
            return Ok(());
        }
        let name = ToolName::from_wire_name(wire_name).ok_or_else(|| format!("unknown tool: {wire_name}"))?;
        self.always_allowed.insert(name);
        Ok(())
    }
}
