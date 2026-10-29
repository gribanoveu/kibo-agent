//! The IPC boundary: thin `#[tauri::command]` functions, and the adapters that
//! turn the core's reports into Tauri events.
//!
//! Nothing here decides anything. A command validates what came across the
//! wire, calls one service, and flattens the error to a string — the one place
//! stringly-typed errors are the right answer.

pub mod chat;
pub mod chat_events;
pub mod plain_chat;
pub mod prototype;
pub mod artifacts;
pub mod knowledge;
pub mod settings;
pub mod chat_history;
pub mod workspace_events;
pub mod skills;
pub mod catalog;
pub mod folder_trust;
pub mod slash_commands;
pub mod tool_log;
pub mod metrics;
pub mod mcp;
pub mod hooks;
pub mod processes;
pub mod agents;
pub mod board;
pub mod meeting;
pub mod mongo;
pub mod features;
pub mod terminal;
pub mod git;
pub mod files;
pub mod http;
pub mod images;
pub mod rewind;
pub mod viewer;

/// Runs `work` off the main thread, where a synchronous command runs: for one
/// that touches SQLite, git, the keychain, the user's files or the network
/// (AGENTS.md, IPC conventions).
pub(crate) async fn blocking<T, E>(work: impl FnOnce() -> Result<T, E> + Send + 'static) -> Result<T, String>
where
    T: Send + 'static,
    E: std::fmt::Display + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| format!("the command's thread failed: {e}"))?
        .map_err(|e| e.to_string())
}
