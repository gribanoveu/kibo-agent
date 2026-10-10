//! The IPC boundary: thin `#[tauri::command]` functions, and the adapters that
//! turn the core's reports into Tauri events.
//!
//! Nothing here decides anything. A command validates what came across the
//! wire, calls one service, and flattens the error to a string — the one place
//! stringly-typed errors are the right answer.

pub mod chat;
pub mod chat_events;
pub mod plain_chat;
pub mod settings;
pub mod chat_history;
pub mod workspace_events;
pub mod skills;
pub mod folder_trust;
pub mod slash_commands;
pub mod tool_log;
pub mod metrics;
pub mod mcp;
pub mod hooks;
pub mod processes;
pub mod agents;
pub mod terminal;
pub mod git;
pub mod files;
pub mod images;
pub mod rewind;
pub mod viewer;
