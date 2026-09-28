//! Chat mode: a conversation with the model outside any folder.
//!
//! Not a fourth [`ConversationMode`](super::conversation_mode::ConversationMode).
//! Those are narrower views of one agent working in the open folder; a chat
//! has no folder, so nothing of the agent's — its tools, its rules, its skills
//! — applies. What a chat has instead is a role: who the model is told to be,
//! and the tools that role is given. The tool set is the role's own, never
//! derived from the agent's: a DevOps role that works with Kubernetes gets
//! `kubectl`, not `readFile`.
//!
//! Adding a role is a variant here, its name, its prompt and its tools; the
//! window lists what [`ChatRole::ALL`] holds.

use serde::{Deserialize, Serialize};

use super::tools::ToolName;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChatRole {
    /// A general assistant that answers in text.
    #[default]
    Assistant,
}

impl ChatRole {
    pub const ALL: &'static [ChatRole] = &[ChatRole::Assistant];

    pub fn name(self) -> &'static str {
        match self {
            ChatRole::Assistant => "Assistant",
        }
    }

    /// What the model is told before the conversation.
    pub fn prompt(self) -> &'static str {
        match self {
            ChatRole::Assistant => {
                "You are a helpful assistant in a plain chat. You cannot read the user's files, run commands \
                 or change anything on their machine — answer from what the user writes here. When an answer \
                 depends on code or output you have not been shown, ask for it rather than guessing."
            }
        }
    }

    /// The tools this role may call. None yet: a plain chat only talks.
    pub fn tools(self) -> &'static [ToolName] {
        match self {
            ChatRole::Assistant => &[],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The promise of Chat mode as it ships: no role reaches the user's files.
    #[test]
    fn the_assistant_has_no_tools() {
        assert!(ChatRole::Assistant.tools().is_empty());
    }

    /// The window sends the role by this name; a rename is a role it can no longer pick.
    #[test]
    fn the_wire_name_is_the_one_the_window_sends() {
        assert_eq!(serde_json::to_string(&ChatRole::Assistant).unwrap(), "\"assistant\"");
        assert_eq!(ChatRole::default(), ChatRole::Assistant);
    }
}
