pub mod ai_tools;
pub mod text_diff;
pub mod llm_session;
pub mod llm_chat;
pub mod plain_chat;
pub mod kubeconfigs;
pub mod kube_changes;
pub mod context_compaction;
pub mod commit_message;
pub mod chunk_text;
pub mod repo_index;
pub mod embedding_index;
pub mod index_sync;
pub mod workspace_index;
pub mod code_search;
#[cfg(test)]
mod search_bench;
#[cfg(test)]
mod agent_bench;
#[cfg(test)]
mod kube_bench;
pub mod skills;
pub mod project_rules;
pub mod mcp_questions;
pub mod mcp_servers;
pub mod rewind;
