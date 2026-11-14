//! Opening a folder as the window's workspace, once the command has checked
//! that it is one.

use std::path::Path;

use crate::domain::workspace_index::IndexEventSink;
use crate::infra::background::Processes;
use crate::infra::terminal::Terminals;
use crate::services::workspace_index::WorkspaceIndex;

/// Makes `root` the folder the window works in: what ran in the folder
/// before stops — unless `keep_running`, a chat moved to its worktree, which
/// must not stop the neighbouring chat's processes — the folder goes on the
/// recent list, and its index opens. Blocking: the index reads what was
/// embedded before. An index that cannot open does not stop the folder
/// opening: the reason goes out on `sink`, and the agent works without search.
pub fn open(
    root: &Path,
    keep_running: bool,
    processes: Option<&Processes>,
    terminals: Option<&Terminals>,
    index: &WorkspaceIndex,
    sink: IndexEventSink,
) {
    if !keep_running {
        if let Some(processes) = processes {
            processes.stop_all();
        }
        if let Some(terminals) = terminals {
            terminals.close_all();
        }
    }
    // Only a convenience for the next launch: a folder that opened stays open
    // even if the list could not be written.
    if let Err(e) = crate::infra::recent_workspaces::record(&root.display().to_string()) {
        eprintln!("recent folders not saved: {e}");
    }
    // Reported through the sink; see above.
    let _ = index.open(root, sink);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::background::BackgroundProcesses;
    use crate::domain::embeddings::{Embedding, EmbeddingError, EmbeddingProvider};
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

    /// A folder switch stops what ran in the old one; a chat moving to its
    /// worktree leaves the neighbours' processes running. Either way the
    /// folder is remembered and indexed.
    #[cfg(unix)]
    #[test]
    fn a_switch_stops_the_old_folders_processes_and_a_worktree_move_does_not() {
        crate::testing::with_app_dir("folder-open", || {
            let processes = Processes::default();
            let index = WorkspaceIndex::new(temp_dir("folder-open-index"), Arc::new(NoModel));
            let root = crate::infra::paths::canonicalize_plain(&temp_dir("folder-open-root")).unwrap();
            processes.start(&crate::domain::command_exec::Shell::default(), "sleep 30", &root, ".").unwrap();

            open(&root, true, Some(&processes), None, &index, Arc::new(|_| {}));
            assert_eq!(processes.list().iter().filter(|p| p.state == crate::domain::background::ProcessState::Running).count(), 1, "a worktree move keeps them");
            assert_eq!(index.current().map(|i| i.root().to_path_buf()), Some(root.clone()));
            assert_eq!(crate::infra::recent_workspaces::load(), vec![root.display().to_string()]);

            open(&root, false, Some(&processes), None, &index, Arc::new(|_| {}));
            assert_eq!(processes.list().iter().filter(|p| p.state == crate::domain::background::ProcessState::Running).count(), 0, "a switch stops them");
        });
    }
}
