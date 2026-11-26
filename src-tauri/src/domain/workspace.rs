//! Why a folder could not be opened, closed or acted on — the open set's
//! refusals (`docs/65-interface-language.md`, LC-2). The command layer builds
//! these and flattens them to its `String` errors unchanged; the codes are
//! what the window will translate in LC-5.

use thiserror::Error;

use super::ui_text::UiText;

#[derive(Debug, Error)]
pub enum WorkspaceError {
    #[error("cannot open {path}: {reason}")]
    CannotOpen { path: String, reason: String },
    #[error("{path} is not a folder")]
    NotAFolder { path: String },
    #[error("{path} is not an open folder")]
    NotOpen { path: String },
    #[error("{path} is not an open folder — open it to work in this chat")]
    NotOpenForChat { path: String },
    #[error("cannot use {path}: {reason}")]
    CannotUse { path: String, reason: String },
    #[error("no folder is open")]
    NoFolderOpen,
    #[error("an agent turn is running in this folder — stop it before closing the folder")]
    TurnRunning,
    #[error("an agent turn is running in this folder — wait for it to finish, or start a worktree for the new chat")]
    FolderBusy,
    #[error("{what} lock poisoned")]
    LockPoisoned { what: String },
}

impl UiText for WorkspaceError {
    fn code(&self) -> &'static str {
        match self {
            Self::CannotOpen { .. } => "workspace.cannot_open",
            Self::NotAFolder { .. } => "workspace.not_a_folder",
            Self::NotOpen { .. } => "workspace.not_open",
            Self::NotOpenForChat { .. } => "workspace.not_open_for_chat",
            Self::CannotUse { .. } => "workspace.cannot_use",
            Self::NoFolderOpen => "workspace.no_folder_open",
            Self::TurnRunning => "workspace.turn_running",
            Self::FolderBusy => "workspace.folder_busy",
            Self::LockPoisoned { .. } => "workspace.lock_poisoned",
        }
    }

    fn params(&self) -> Vec<(&'static str, String)> {
        match self {
            Self::CannotOpen { path, reason } | Self::CannotUse { path, reason } => {
                vec![("path", path.clone()), ("reason", reason.clone())]
            }
            Self::NotAFolder { path } | Self::NotOpen { path } | Self::NotOpenForChat { path } => {
                vec![("path", path.clone())]
            }
            Self::LockPoisoned { what } => vec![("what", what.clone())],
            Self::NoFolderOpen | Self::TurnRunning | Self::FolderBusy => Vec::new(),
        }
    }
}

#[cfg(test)]
impl WorkspaceError {
    /// One per variant — the catalog test checks each, `architecture.rs`
    /// holds the list complete.
    pub fn ui_samples() -> Vec<Self> {
        vec![
            Self::CannotOpen { path: "/x".into(), reason: "no such file".into() },
            Self::NotAFolder { path: "/x".into() },
            Self::NotOpen { path: "/x".into() },
            Self::NotOpenForChat { path: "/x".into() },
            Self::CannotUse { path: "/x".into(), reason: "no such file".into() },
            Self::NoFolderOpen,
            Self::TurnRunning,
            Self::FolderBusy,
            Self::LockPoisoned { what: "workspace".into() },
        ]
    }
}
