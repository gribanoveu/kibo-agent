//! Opening and closing a folder of the window, once the command has checked
//! that it is one.

use std::path::Path;

use crate::domain::workspace_index::IndexEventSink;
use crate::infra::background::Processes;
use crate::infra::terminal::Terminals;
use crate::services::mcp_servers::McpServers;
use crate::services::workspace_index::WorkspaceIndex;

/// Makes `root` a folder the window works in: it goes on the recent list,
/// and its index opens beside the other open folders'. Nothing of another
/// folder stops (F-8.3). Blocking: the index reads what was embedded before.
/// An index that cannot open does not stop the folder opening: the reason
/// goes out on `sink`, and the agent works without search.
pub fn open(root: &Path, index: &WorkspaceIndex, sink: IndexEventSink) {
    // Only a convenience for the next launch: a folder that opened stays open
    // even if the list could not be written.
    if let Err(e) = crate::infra::recent_workspaces::record(&root.display().to_string()) {
        eprintln!("recent folders not saved: {e}");
    }
    // Reported through the sink; see above.
    let _ = index.open(root, sink);
}

/// Everything of `root` stops (F-8 invariant 4): its processes, its shells,
/// its MCP servers, its index's watch — and nothing of another folder's.
/// Blocking: a process is waited for once killed.
pub fn close(
    root: &Path,
    processes: Option<&Processes>,
    terminals: Option<&Terminals>,
    mcp: Option<&McpServers>,
    index: &WorkspaceIndex,
) {
    if let Some(processes) = processes {
        processes.stop_workspace(root);
    }
    if let Some(terminals) = terminals {
        terminals.close_workspace(root);
    }
    if let Some(mcp) = mcp {
        mcp.stop(root);
    }
    index.close(root);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::background::{BackgroundProcesses, ProcessState};
    use crate::domain::command_exec::Shell;
    use crate::domain::embeddings::{Embedding, EmbeddingError, EmbeddingProvider};
    use crate::domain::mcp::{McpCallResult, McpClient, McpConfig, McpError, McpServerConfig, McpTool};
    use crate::domain::terminal::{TerminalSize, TerminalState};
    use crate::testing::temp_dir;
    use std::sync::Arc;

    struct NoModel;
    impl EmbeddingProvider for NoModel {
        fn embed(&self, _: &[&str]) -> Result<Vec<Embedding>, EmbeddingError> {
            Err(EmbeddingError::Invalid("no model in this test".into()))
        }
        fn dimensions(&self) -> usize {
            2
        }
    }

    struct Server;
    impl McpClient for Server {
        fn list_tools(&self) -> Result<Vec<McpTool>, McpError> {
            Ok(vec![McpTool { name: "t".into(), ..Default::default() }])
        }
        fn call_tool(&self, _: &str, _: serde_json::Value, _: &dyn Fn() -> bool) -> Result<McpCallResult, McpError> {
            Err(McpError::Cancelled)
        }
    }

    /// F-8.3, F-8.4: opening B leaves A's processes running and A indexed;
    /// closing A stops A's processes, shells, MCP servers and index, and
    /// nothing of B's.
    #[cfg(unix)]
    #[test]
    fn closing_a_folder_stops_its_things_and_no_others() {
        crate::testing::with_app_dir("folder-close", || {
            let processes = Arc::new(Processes::default());
            let terminals = Terminals::with_shell("/bin/sh");
            let index = WorkspaceIndex::new(temp_dir("folder-close-index"), Arc::new(NoModel));
            let plain = |label| crate::infra::paths::canonicalize_plain(&temp_dir(label)).unwrap();
            let (a, b) = (plain("folder-close-a"), plain("folder-close-b"));
            let size = TerminalSize::new(80, 24).unwrap();
            let mcp = McpServers::new(Arc::new(|_, _, _| Ok(Arc::new(Server) as Arc<dyn McpClient>)), Arc::new(|_| {}));
            let mut config = McpConfig::default();
            config.mcp_servers.insert("s".into(), McpServerConfig { command: "x".into(), ..Default::default() });
            let in_folder = |root: &Path| {
                processes.for_chat("c", root).start(&Shell::default(), "sleep 30", root, ".").unwrap();
                terminals.open(root, size).unwrap();
                mcp.for_turn(&config, root, &|| false);
            };
            let serves = |root: &Path| mcp.running(root).get("mcp__s__t").is_some();
            open(&a, &index, Arc::new(|_| {}));
            in_folder(&a);
            open(&b, &index, Arc::new(|_| {}));
            in_folder(&b);
            let running = |root: &Path| processes.list_in(root).iter().filter(|p| p.info.state == ProcessState::Running).count();
            let shells = |root: &Path| terminals.list_in(root).iter().filter(|t| t.state == TerminalState::Running).count();
            assert_eq!((running(&a), shells(&a)), (1, 1), "opening B stopped nothing of A's");
            assert!(serves(&a), "a turn in B stopped A's servers");
            assert!(index.indexer(&a).is_some());

            close(&a, Some(&processes), Some(&terminals), Some(&mcp), &index);

            assert_eq!(running(&a), 0, "A's process stopped");
            assert!(terminals.list_in(&a).is_empty(), "A's shell closed");
            assert!(index.indexer(&a).is_none(), "A's index closed");
            assert!(!serves(&a), "A's servers stopped");
            assert_eq!((running(&b), shells(&b)), (1, 1), "B's untouched");
            assert!(serves(&b), "B's servers untouched");
            assert!(index.indexer(&b).is_some());
            processes.stop_all();
        });
    }
}
