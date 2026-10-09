//! The index of whichever folder is open, kept current while it stays open.
//!
//! One per app. It owns the embedding model — one [`EmbeddingProvider`] for
//! every folder, so opening another project does not load a second copy — and
//! the indexer and watcher of the open folder. Opening another folder drops the
//! previous watcher; a sync of the previous folder still running finishes on
//! its own thread and is then dropped with it.
//!
//! The store lives under `index_dir`, named by [`repository_id`], never inside
//! the folder: a store written under the watched tree would wake its own
//! watcher after every sync.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use crate::domain::chunk_index::ChunkBuildOptions;
use crate::domain::embeddings::EmbeddingProvider;
use crate::domain::tools::CodeSearchFn;
use crate::domain::workspace_index::{IndexEvent, IndexEventSink};
use crate::infra::file_watcher::FileWatcher;
use crate::infra::index_store::IndexStoreError;
use crate::infra::repository_identity::repository_id;
use crate::services::code_search;
use crate::services::index_sync::RepoIndexer;

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceIndexError {
    #[error("could not open the index: {0}")]
    Store(#[from] IndexStoreError),
    #[error("the folder cannot be watched, so the index will not follow changes: {0}")]
    Watch(#[from] notify::Error),
}

pub struct WorkspaceIndex {
    index_dir: PathBuf,
    provider: Arc<dyn EmbeddingProvider>,
    open: Mutex<Option<Open>>,
}

struct Open {
    root: PathBuf,
    indexer: Arc<RepoIndexer>,
    /// `None` when the folder could not be watched; the index still answers,
    /// from what the first sync found.
    _watcher: Option<FileWatcher>,
}

impl WorkspaceIndex {
    pub fn new(index_dir: PathBuf, provider: Arc<dyn EmbeddingProvider>) -> Self {
        Self { index_dir, provider, open: Mutex::new(None) }
    }

    /// The bundled model, shared: a chat's web search picks a page's passages
    /// with the same weights the index loaded, not a second copy of them.
    pub fn embeddings(&self) -> Arc<dyn EmbeddingProvider> {
        Arc::clone(&self.provider)
    }

    /// Makes `root` the indexed folder: opens its store, starts watching it,
    /// and starts the first sync on a thread of its own. Returns once the
    /// store is open — which reads what was embedded before, so it blocks for
    /// that long and belongs off the UI thread.
    ///
    /// Opening the folder that is already open changes nothing. A failure is
    /// also reported through `sink` as [`IndexEvent::Failed`]: the caller
    /// opens the folder either way, and the window learns why search is off.
    pub fn open(&self, root: &Path, sink: IndexEventSink) -> Result<(), WorkspaceIndexError> {
        self.try_open(root, &sink).inspect_err(|error| sink(IndexEvent::Failed { error: error.to_string() }))
    }

    fn try_open(&self, root: &Path, sink: &IndexEventSink) -> Result<(), WorkspaceIndexError> {
        let mut open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        if open.as_ref().is_some_and(|current| current.root == root) {
            return Ok(());
        }
        // Stop watching the old folder before anything below can fail.
        *open = None;

        let store_dir = self.index_dir.join(repository_id(root));
        let indexer =
            Arc::new(RepoIndexer::open(root, &store_dir, Arc::clone(&self.provider), ChunkBuildOptions::default())?);
        let watched = Arc::clone(&indexer);
        let watch_sink = Arc::clone(sink);
        // Errors reach the window through the sink; nothing here to do with them.
        let watcher = FileWatcher::start_tree(root, move || drop(watched.sync(&watch_sink)));

        let first = Arc::clone(&indexer);
        let first_sink = Arc::clone(sink);
        std::thread::Builder::new()
            .name("index-first-sync".into())
            .spawn(move || drop(first.sync(&first_sink)))
            .map_err(IndexStoreError::Io)?;

        let (watcher, failed) = match watcher {
            Ok(watcher) => (Some(watcher), None),
            Err(error) => (None, Some(error)),
        };
        *open = Some(Open { root: root.to_path_buf(), indexer, _watcher: watcher });
        failed.map_or(Ok(()), |error| Err(error.into()))
    }

    /// Search of the open folder, as the tools take it. Bound to that folder's
    /// indexer: a turn keeps searching the folder it started in.
    pub fn searcher(&self) -> Option<CodeSearchFn> {
        let indexer = self.current()?;
        Some(Arc::new(move |queries, fts, top_k, filter| {
            code_search::search_many(&indexer, queries, fts, top_k, filter).map_err(|error| error.to_string())
        }))
    }

    /// The open folder's indexer, for search.
    pub fn current(&self) -> Option<Arc<RepoIndexer>> {
        self.open.lock().unwrap_or_else(PoisonError::into_inner).as_ref().map(|open| Arc::clone(&open.indexer))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::embeddings::{Embedding, EmbeddingError};
    use crate::domain::search_query::fts5_query;
    use crate::testing::temp_dir;
    use std::fs;
    use std::sync::mpsc::{self, Receiver};
    use std::time::Duration;

    /// Every text gets the same vector; these tests are about who owns what.
    struct FlatModel;

    impl EmbeddingProvider for FlatModel {
        fn embed(&self, texts: &[&str]) -> Result<Vec<Embedding>, EmbeddingError> {
            Ok(texts.iter().map(|_| Embedding(vec![1.0, 0.0])).collect())
        }
        fn dimensions(&self) -> usize {
            2
        }
    }

    fn workspace_index(label: &str) -> WorkspaceIndex {
        WorkspaceIndex::new(temp_dir(&format!("{label}-indexes")), Arc::new(FlatModel))
    }

    fn channel_sink() -> (IndexEventSink, Receiver<IndexEvent>) {
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        (Arc::new(move |event| drop(tx.lock().unwrap().send(event))), rx)
    }

    /// Waits for the next `SyncFinished`, skipping what comes before it.
    fn next_finish(events: &Receiver<IndexEvent>) -> IndexEvent {
        loop {
            match events.recv_timeout(Duration::from_secs(10)).expect("no sync finished") {
                event @ (IndexEvent::SyncFinished { .. } | IndexEvent::Failed { .. }) => return event,
                _ => {}
            }
        }
    }

    fn finds(index: &WorkspaceIndex, word: &str) -> usize {
        let query = fts5_query(word).unwrap();
        index.current().unwrap().store().search_bm25(&query, 5).unwrap().len()
    }

    #[test]
    fn opening_a_folder_indexes_it_and_follows_its_changes() {
        let index = workspace_index("ws-open");
        let root = temp_dir("ws-open-repo");
        fs::write(root.join("a.rs"), "fn present_at_open() {}\n").unwrap();
        let (sink, events) = channel_sink();

        index.open(&root, sink).unwrap();

        assert!(matches!(next_finish(&events), IndexEvent::SyncFinished { embedding_error: None, .. }));
        assert_eq!(finds(&index, "present_at_open"), 1);
        fs::write(root.join("b.rs"), "fn saved_after_open() {}\n").unwrap();
        // Not "the next finish": FSEvents may still report the folder's
        // creation, and that sync can finish before the save is seen.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while finds(&index, "saved_after_open") == 0 {
            assert!(std::time::Instant::now() < deadline, "the save never reached the index");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    #[test]
    fn the_searcher_searches_the_open_folder_and_is_absent_without_one() {
        let index = workspace_index("ws-searcher");
        assert!(index.searcher().is_none());
        let root = temp_dir("ws-searcher-repo");
        fs::write(root.join("a.rs"), "fn find_me_here() {}\n").unwrap();
        let (sink, events) = channel_sink();
        index.open(&root, sink).unwrap();
        next_finish(&events);

        let found = index.searcher().unwrap()(&["find_me_here"], None, 5, &Default::default()).unwrap();

        assert_eq!(found.matches[0].path, "a.rs");
    }

    #[test]
    fn opening_the_open_folder_again_keeps_its_index() {
        let index = workspace_index("ws-again");
        let root = temp_dir("ws-again-repo");
        let (sink, events) = channel_sink();
        index.open(&root, Arc::clone(&sink)).unwrap();
        next_finish(&events);
        let before = index.current().unwrap();

        index.open(&root, sink).unwrap();

        assert!(Arc::ptr_eq(&before, &index.current().unwrap()));
    }

    #[test]
    fn opening_another_folder_stops_following_the_first() {
        let index = workspace_index("ws-switch");
        let first = temp_dir("ws-switch-first");
        let (first_sink, first_events) = channel_sink();
        index.open(&first, first_sink).unwrap();
        next_finish(&first_events);
        let left_behind = index.current().unwrap();

        let second = temp_dir("ws-switch-second");
        let (second_sink, second_events) = channel_sink();
        index.open(&second, second_sink).unwrap();
        next_finish(&second_events);
        fs::write(first.join("a.rs"), "fn after_switch() {}\n").unwrap();
        std::thread::sleep(Duration::from_secs(1));

        let query = fts5_query("after_switch").unwrap();
        assert!(left_behind.store().search_bm25(&query, 5).unwrap().is_empty(), "the folder left behind is still watched");
    }

    /// The store is where the app keeps its data, not in the folder.
    #[test]
    fn the_store_is_not_written_into_the_folder() {
        let index = workspace_index("ws-clean");
        let root = temp_dir("ws-clean-repo");
        fs::write(root.join("a.rs"), "fn a() {}\n").unwrap();
        let (sink, events) = channel_sink();
        index.open(&root, sink).unwrap();
        next_finish(&events);

        let entries: Vec<_> = fs::read_dir(&root).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(entries, ["a.rs"]);
        assert_eq!(fs::read_dir(&index.index_dir).unwrap().count(), 1);
    }

    #[test]
    fn a_store_that_will_not_open_is_reported_and_leaves_nothing_open() {
        let blocked = temp_dir("ws-blocked").join("not-a-dir");
        fs::write(&blocked, "").unwrap();
        let index = WorkspaceIndex::new(blocked, Arc::new(FlatModel));
        let (sink, events) = channel_sink();

        assert!(index.open(&temp_dir("ws-blocked-repo"), sink).is_err());

        assert!(matches!(events.try_recv(), Ok(IndexEvent::Failed { .. })));
        assert!(index.current().is_none());
    }

    /// Opening the next folder fails: the previous one is not left watched
    /// and answering as if it were the open folder.
    #[test]
    fn a_failed_open_leaves_the_previous_folder_closed() {
        let index = workspace_index("ws-fail-after");
        let first = temp_dir("ws-fail-after-first");
        let (sink, events) = channel_sink();
        index.open(&first, Arc::clone(&sink)).unwrap();
        next_finish(&events);
        let second = temp_dir("ws-fail-after-second");
        fs::write(index.index_dir.join(repository_id(&second)), "").unwrap();

        assert!(matches!(index.open(&second, sink), Err(WorkspaceIndexError::Store(_))));

        assert!(index.current().is_none());
    }

    /// A folder that cannot be watched is still indexed once and searchable;
    /// the window hears why it will not follow changes.
    #[test]
    fn a_folder_that_cannot_be_watched_is_reported_but_kept() {
        let index = workspace_index("ws-unwatched");
        let missing = temp_dir("ws-unwatched-repo").join("gone");
        let (sink, events) = channel_sink();

        assert!(matches!(index.open(&missing, sink), Err(WorkspaceIndexError::Watch(_))));

        assert!(index.current().is_some());
        let said = std::iter::from_fn(|| events.recv_timeout(Duration::from_secs(5)).ok())
            .any(|event| matches!(&event, IndexEvent::Failed { error } if error.contains("cannot be watched")));
        assert!(said, "the window was not told");
    }
}
