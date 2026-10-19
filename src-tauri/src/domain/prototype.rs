//! A Prototype chat's folder (`docs/31-prototype-role.md`): what can go wrong
//! with it.

use thiserror::Error;

use super::chat_record::ChatError;
use super::settings::AppDirError;

#[derive(Debug, Error)]
pub enum PrototypeError {
    /// The id came from the window and is not one plain path segment, so no
    /// folder is made for it.
    #[error("not a chat id: {0:?}")]
    NotAChatId(String),
    /// The app directory itself could not be made.
    #[error("{0}")]
    AppDir(String),
    #[error(transparent)]
    NoAppDir(#[from] AppDirError),
    #[error("{path}: {source}")]
    Io { path: String, source: std::io::Error },
    #[error(transparent)]
    Chat(#[from] ChatError),
    /// `present` outside a Prototype chat's working folder: there is no
    /// `versions/` beside it to keep one in.
    #[error("present works in a Prototype chat's folder only, and {0} is not one")]
    NotAChatFolder(String),
    /// Worded for the model: what to do about it.
    #[error("the folder is over {} MB, the most a version keeps — delete what the page does not use, or keep large files out of it", limit / (1024 * 1024))]
    TooBig { limit: u64 },
}
