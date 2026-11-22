//! The indexes of the open folders that something reads (F-8.2, LZ-4 of
//! `docs/54-lazy-index.md`).
//!
//! One per app. It owns the embedding model — one [`EmbeddingProvider`] for
//! every folder, so opening another project does not load a second copy — and
//! an indexer and watcher per folder in use. A folder is in use while it is
//! the active one, whose panes the window draws from the index's signals, or
//! while a searcher of it lives: a turn's, and a helper's or a reranker's
//! clone of it. Any other open folder costs nothing — no store, no watcher, no
//! sync — and the next searcher opens it again: measured on this repository,
//! 25–55 ms to open the store and 45–75 ms for a sync that finds nothing new.
//!
//! A folder reopened while the first sync of its previous opening still runs
//! has two indexers syncing one store for a while: the store keeps that safe
//! (busy timeout, short transactions), at the cost of embedding a change twice.
//!
//! The store lives under `index_dir`, named by [`repository_id`], never inside
//! the folder: a store written under the watched tree would wake its own
//! watcher after every sync.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

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
    folders: Mutex<Folders>,
    /// Held from "is it open?" to "it is", across reading the store: two
    /// searchers of one folder starting together would otherwise open it
    /// twice and run two first syncs on one store.
    // ponytail: one lock for every folder, so a folder opening waits for
    // another's store and watcher: tens of milliseconds on macOS, but seconds
    // on Linux, where inotify puts a watch on every directory of a big tree.
    // Per-folder slots if a turn is seen waiting on that.
    opening: Mutex<()>,
}

/// Keyed by the folder's canonical spelling — the one the registry of open
/// folders and a chat's record resolve to.
#[derive(Default)]
struct Folders {
    open: HashMap<PathBuf, Open>,
    active: Option<PathBuf>,
    /// The searchers alive per folder, with the epoch they are counted in. A
    /// guard in case a searcher outlives a close (a turn holds its folder
    /// open, so none should): the folder opened again starts a new count,
    /// and the one left over does not take from it when it goes.
    held: HashMap<PathBuf, (u64, usize)>,
    epochs: u64,
}

impl Folders {
    fn in_use(&self, root: &Path) -> bool {
        self.active.as_deref() == Some(root) || self.held.get(root).is_some_and(|&(_, count)| count > 0)
    }
}

struct Open {
    indexer: Arc<RepoIndexer>,
    /// `None` when the folder could not be watched; the index still answers,
    /// from what the first sync found.
    _watcher: Option<FileWatcher>,
}

/// A searcher's claim on its folder, given back when the searcher is dropped.
struct Hold {
    index: Weak<WorkspaceIndex>,
    root: PathBuf,
    epoch: u64,
}

impl Drop for Hold {
    fn drop(&mut self) {
        if let Some(index) = self.index.upgrade() {
            index.release(&self.root, self.epoch);
        }
    }
}

impl WorkspaceIndex {
    pub fn new(index_dir: PathBuf, provider: Arc<dyn EmbeddingProvider>) -> Self {
        Self { index_dir, provider, folders: Mutex::default(), opening: Mutex::new(()) }
    }

    /// The bundled model, shared: a chat's web search picks a page's passages
    /// with the same weights the index loaded, not a second copy of them.
    pub fn embeddings(&self) -> Arc<dyn EmbeddingProvider> {
        Arc::clone(&self.provider)
    }

    fn folders(&self) -> MutexGuard<'_, Folders> {
        self.folders.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Makes `root` the active folder: indexed and followed from now on, and
    /// the folder active before closed unless a searcher still holds it.
    /// Returns once the store is open, which reads what was embedded before —
    /// it belongs off the UI thread.
    ///
    /// A failure is also reported through `sink` as [`IndexEvent::Failed`]:
    /// the caller opens the folder either way, and the window learns why
    /// search is off.
    pub fn activate(&self, root: &Path, sink: IndexEventSink) -> Result<(), WorkspaceIndexError> {
        let left = {
            let mut folders = self.folders();
            let previous = folders.active.replace(root.to_path_buf());
            previous.filter(|previous| !folders.in_use(previous)).and_then(|previous| folders.open.remove(&previous))
        };
        // Dropped outside the lock: stopping the OS watch is a system call.
        drop(left);
        self.open(root, &sink)
    }

    /// Search of `root`'s index, as the tools take it, opening the folder's
    /// index if nothing had it open. The folder stays indexed while the
    /// searcher, or any clone of it, lives. Bound to that folder's indexer: a
    /// turn keeps searching the folder it started in, and never another one's.
    ///
    /// `None` when the index will not open; why goes through `sink`.
    pub fn searcher(self: &Arc<Self>, root: &Path, sink: IndexEventSink) -> Option<CodeSearchFn> {
        let epoch = {
            let mut folders = self.folders();
            match folders.held.get_mut(root) {
                Some((epoch, count)) => {
                    *count += 1;
                    *epoch
                }
                None => {
                    folders.epochs += 1;
                    let epoch = folders.epochs;
                    folders.held.insert(root.to_path_buf(), (epoch, 1));
                    epoch
                }
            }
        };
        let hold = Hold { index: Arc::downgrade(self), root: root.to_path_buf(), epoch };
        // A folder that cannot be watched is still searched.
        let _ = self.open(root, &sink);
        let indexer = self.indexer(root)?;
        Some(Arc::new(move |queries, fts, top_k, filter| {
            let _held = &hold;
            code_search::search_many(&indexer, queries, fts, top_k, filter).map_err(|error| error.to_string())
        }))
    }

    /// A searcher of `root` gone: the folder closes when it was the last and
    /// the folder is not the active one.
    fn release(&self, root: &Path, epoch: u64) {
        let closed = {
            let mut folders = self.folders();
            match folders.held.get_mut(root) {
                Some((counted, count)) if *counted == epoch => {
                    *count -= 1;
                    if *count == 0 {
                        folders.held.remove(root);
                    }
                }
                _ => return,
            }
            if folders.in_use(root) {
                return;
            }
            folders.open.remove(root)
        };
        drop(closed);
    }

    /// Opens `root`'s store, starts watching it, and starts the first sync on
    /// a thread of its own — if it is not open and still in use once the store
    /// is read. Opening a folder that is open changes nothing.
    fn open(&self, root: &Path, sink: &IndexEventSink) -> Result<(), WorkspaceIndexError> {
        self.try_open(root, sink).inspect_err(|error| sink(IndexEvent::Failed { error: error.to_string() }))
    }

    fn try_open(&self, root: &Path, sink: &IndexEventSink) -> Result<(), WorkspaceIndexError> {
        // Checked before the lock too: a turn in an open folder does not wait
        // for another folder's store to be read.
        if self.indexer(root).is_some() {
            return Ok(());
        }
        let _opening = self.opening.lock().unwrap_or_else(PoisonError::into_inner);
        if self.indexer(root).is_some() {
            return Ok(());
        }
        // Read without the folders' lock: a large store takes a moment, and
        // the other folders' turns search meanwhile.
        let store_dir = self.index_dir.join(repository_id(root));
        let indexer =
            Arc::new(RepoIndexer::open(root, &store_dir, Arc::clone(&self.provider), ChunkBuildOptions::default())?);
        let watched = Arc::clone(&indexer);
        let watch_sink = Arc::clone(sink);
        // Errors reach the window through the sink; nothing here to do with them.
        let watcher = FileWatcher::start_tree(root, move || drop(watched.sync(&watch_sink)));
        let (watcher, failed) = match watcher {
            Ok(watcher) => (Some(watcher), None),
            Err(error) => (None, Some(error)),
        };

        // Released or switched away from while the store was read: nobody
        // wants it, and it is dropped unopened.
        let unwanted = {
            let mut folders = self.folders();
            if folders.in_use(root) {
                folders.open.insert(root.to_path_buf(), Open { indexer: Arc::clone(&indexer), _watcher: watcher });
                None
            } else {
                Some(watcher)
            }
        };
        if unwanted.is_some() {
            return Ok(());
        }

        let first_sink = Arc::clone(sink);
        std::thread::Builder::new()
            .name("index-first-sync".into())
            .spawn(move || drop(indexer.sync(&first_sink)))
            .map_err(IndexStoreError::Io)?;
        failed.map_or(Ok(()), |error| Err(error.into()))
    }

    /// Stops following `root` — closed in the window: its watcher is dropped,
    /// it is no longer active, and searchers left of it no longer count.
    /// Closing a folder that is not open does nothing.
    pub fn close(&self, root: &Path) {
        let closed = {
            let mut folders = self.folders();
            if folders.active.as_deref() == Some(root) {
                folders.active = None;
            }
            folders.held.remove(root);
            folders.open.remove(root)
        };
        drop(closed);
    }

    /// `root`'s indexer, if that folder's index is open.
    pub fn indexer(&self, root: &Path) -> Option<Arc<RepoIndexer>> {
        self.folders().open.get(root).map(|open| Arc::clone(&open.indexer))
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

    fn workspace_index(label: &str) -> Arc<WorkspaceIndex> {
        Arc::new(WorkspaceIndex::new(temp_dir(&format!("{label}-indexes")), Arc::new(FlatModel)))
    }

    fn channel_sink() -> (IndexEventSink, Receiver<IndexEvent>) {
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        (Arc::new(move |event| drop(tx.lock().unwrap().send(event))), rx)
    }

    fn quiet() -> IndexEventSink {
        Arc::new(|_| {})
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

    fn finds_in(indexer: &RepoIndexer, word: &str) -> usize {
        indexer.store().search_bm25(&fts5_query(word).unwrap(), 5).unwrap().len()
    }

    /// Waits until `word` reaches `root`'s index, as a save is seen.
    fn until_found(index: &WorkspaceIndex, root: &Path, word: &str) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while finds_in(&index.indexer(root).unwrap(), word) == 0 {
            assert!(std::time::Instant::now() < deadline, "{word} never reached the index of {}", root.display());
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn search(searcher: &CodeSearchFn, word: &str) -> Vec<String> {
        searcher(&[word], None, 5, &Default::default()).unwrap().matches.into_iter().map(|m| m.path).collect()
    }

    fn repo(label: &str, file: &str, text: &str) -> PathBuf {
        let root = temp_dir(label);
        fs::write(root.join(file), text).unwrap();
        root
    }

    #[test]
    fn the_active_folder_is_indexed_and_follows_its_changes() {
        let index = workspace_index("ws-open");
        let root = repo("ws-open-repo", "a.rs", "fn present_at_open() {}\n");
        let (sink, events) = channel_sink();

        index.activate(&root, sink).unwrap();

        assert!(matches!(next_finish(&events), IndexEvent::SyncFinished { embedding_error: None, .. }));
        assert_eq!(finds_in(&index.indexer(&root).unwrap(), "present_at_open"), 1);
        fs::write(root.join("b.rs"), "fn saved_after_open() {}\n").unwrap();
        // Not "the next finish": FSEvents may still report the folder's
        // creation, and that sync can finish before the save is seen.
        until_found(&index, &root, "saved_after_open");
    }

    #[test]
    fn activating_the_active_folder_again_keeps_its_index() {
        let index = workspace_index("ws-again");
        let root = temp_dir("ws-again-repo");
        let (sink, events) = channel_sink();
        index.activate(&root, sink).unwrap();
        next_finish(&events);
        let before = index.indexer(&root).unwrap();

        let (again, again_events) = channel_sink();
        index.activate(&root, again).unwrap();

        assert!(Arc::ptr_eq(&before, &index.indexer(&root).unwrap()));
        // Nor syncs it again: going back to an open folder is free.
        assert!(again_events.recv_timeout(Duration::from_millis(500)).is_err(), "an open folder was synced again");
    }

    /// LZ-4: only what is read is open. Another folder made active closes
    /// the one left, and stops following it.
    #[test]
    fn the_folder_left_is_closed() {
        let index = workspace_index("ws-left");
        let a = temp_dir("ws-left-a");
        let b = temp_dir("ws-left-b");
        let (sink, events) = channel_sink();
        index.activate(&a, sink).unwrap();
        next_finish(&events);
        let left = index.indexer(&a).unwrap();

        index.activate(&b, quiet()).unwrap();

        assert!(index.indexer(&a).is_none(), "the folder left is still open");
        assert!(index.indexer(&b).is_some());
        fs::write(a.join("late.rs"), "fn after_leaving() {}\n").unwrap();
        std::thread::sleep(Duration::from_secs(1));
        assert_eq!(finds_in(&left, "after_leaving"), 0, "the folder left is still watched");
    }

    /// A turn's folder in the background: its searcher opens it, the index is
    /// followed while any clone of the searcher lives — a helper's, the
    /// reranker's — and closes with the last of them.
    #[test]
    fn a_folder_is_open_while_a_searcher_of_it_lives() {
        let index = workspace_index("ws-held");
        let active = temp_dir("ws-held-active");
        index.activate(&active, quiet()).unwrap();
        let root = repo("ws-held-repo", "a.rs", "fn held_here() {}\n");
        assert!(index.indexer(&root).is_none());
        let (sink, events) = channel_sink();

        let searcher = index.searcher(&root, sink).unwrap();
        next_finish(&events);
        assert_eq!(search(&searcher, "held_here"), ["a.rs"]);
        fs::write(root.join("b.rs"), "fn saved_while_held() {}\n").unwrap();
        until_found(&index, &root, "saved_while_held");

        let helper = Arc::clone(&searcher);
        drop(searcher);
        assert!(index.indexer(&root).is_some(), "a clone no longer holds the folder");
        drop(helper);
        assert!(index.indexer(&root).is_none(), "the last searcher gone left the folder open");
        assert!(index.indexer(&active).is_some(), "the active folder closed with it");
    }

    #[test]
    fn the_active_folder_stays_open_when_its_searcher_goes() {
        let index = workspace_index("ws-active-held");
        let root = temp_dir("ws-active-held-repo");
        index.activate(&root, quiet()).unwrap();

        drop(index.searcher(&root, quiet()).unwrap());

        assert!(index.indexer(&root).is_some());
    }

    /// Left while a turn runs in it, a folder stays until the turn is done.
    #[test]
    fn a_folder_left_while_held_closes_with_its_last_searcher() {
        let index = workspace_index("ws-left-held");
        let a = temp_dir("ws-left-held-a");
        index.activate(&a, quiet()).unwrap();
        let searcher = index.searcher(&a, quiet()).unwrap();

        index.activate(&temp_dir("ws-left-held-b"), quiet()).unwrap();
        assert!(index.indexer(&a).is_some(), "closed under a running turn");

        drop(searcher);
        assert!(index.indexer(&a).is_none());
    }

    /// Searchers of one cold folder starting together: the folder is opened
    /// once, every searcher counts, and it closes only when all are gone.
    #[test]
    fn every_searcher_counts() {
        let index = workspace_index("ws-two-held");
        let root = temp_dir("ws-two-held-repo");
        let start = Arc::new(std::sync::Barrier::new(8));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let (index, root, start) = (Arc::clone(&index), root.clone(), Arc::clone(&start));
                std::thread::spawn(move || {
                    start.wait();
                    let searcher = index.searcher(&root, quiet()).unwrap();
                    (searcher, index.indexer(&root).unwrap())
                })
            })
            .collect();
        let (mut searchers, seen): (Vec<CodeSearchFn>, Vec<_>) = threads.into_iter().map(|t| t.join().unwrap()).unzip();
        let indexer = index.indexer(&root).unwrap();
        assert!(seen.iter().all(|one| Arc::ptr_eq(one, &indexer)), "a folder opened twice at once");

        while searchers.len() > 1 {
            searchers.pop();
            assert!(index.indexer(&root).is_some_and(|now| Arc::ptr_eq(&now, &indexer)), "closed or reopened with searchers left");
        }
        searchers.pop();
        assert!(index.indexer(&root).is_none());
    }

    /// A folder closed in the window and opened again is a new count: a
    /// searcher of before — a helper that outlived its turn — going does not
    /// close it under the new one's.
    #[test]
    fn a_searcher_from_before_a_close_does_not_close_the_folder_again() {
        let index = workspace_index("ws-epoch");
        let root = temp_dir("ws-epoch-repo");
        index.activate(&temp_dir("ws-epoch-active"), quiet()).unwrap();
        let old = index.searcher(&root, quiet()).unwrap();
        index.close(&root);
        assert!(index.indexer(&root).is_none());
        index.activate(&root, quiet()).unwrap();
        index.activate(&temp_dir("ws-epoch-active"), quiet()).unwrap();
        assert!(index.indexer(&root).is_none(), "a searcher of before the close still kept the folder open");

        let new = index.searcher(&root, quiet()).unwrap();
        drop(old);

        assert!(index.indexer(&root).is_some(), "an old searcher closed the folder under a new one");
        drop(new);
        assert!(index.indexer(&root).is_none());
    }

    /// Switched away from while its store is read: the folder is dropped
    /// unopened, not left open with nothing to close it.
    #[test]
    fn a_folder_left_while_its_store_opens_is_not_kept() {
        let index = workspace_index("ws-left-opening");
        let (root, other) = (temp_dir("ws-left-opening-a"), temp_dir("ws-left-opening-b"));
        let activating = |root: &Path| {
            let (opener, folder) = (Arc::clone(&index), root.to_path_buf());
            let thread = std::thread::spawn(move || opener.activate(&folder, quiet()));
            while index.folders().active.as_deref() != Some(root) {
                std::thread::yield_now();
            }
            thread
        };
        let opening = index.opening.lock().unwrap();
        let first = activating(&root);
        let second = activating(&other);
        drop(opening);
        first.join().unwrap().unwrap();
        second.join().unwrap().unwrap();

        assert!(index.indexer(&root).is_none(), "a folder nobody wanted any more was kept open");
        assert!(index.indexer(&other).is_some());
    }

    /// F-8.2: two folders indexed side by side, each searching only itself;
    /// closing one stops following it and leaves the other followed.
    #[test]
    fn open_folders_are_indexed_apart_and_closed_alone() {
        let index = workspace_index("ws-two");
        let a = repo("ws-two-a", "a.rs", "fn only_in_a() {}\n");
        let b = repo("ws-two-b", "b.rs", "fn only_in_b() {}\n");
        let (a_sink, a_events) = channel_sink();
        let (b_sink, b_events) = channel_sink();
        index.activate(&a, Arc::clone(&a_sink)).unwrap();
        let in_a = index.searcher(&a, a_sink).unwrap();
        let in_b = index.searcher(&b, b_sink).unwrap();
        next_finish(&a_events);
        next_finish(&b_events);

        // Paths, not counts: every text embeds alike here, so any query
        // matches every chunk of the folder searched — and only of it.
        assert_eq!(search(&in_a, "only_in_b"), ["a.rs"], "A finds its own and not B's");
        assert_eq!(search(&in_b, "only_in_a"), ["b.rs"], "B finds its own and not A's");

        // Opening B left A followed.
        fs::write(a.join("c.rs"), "fn saved_in_a() {}\n").unwrap();
        until_found(&index, &a, "saved_in_a");

        let left_behind = index.indexer(&a).unwrap();
        index.close(&a);
        assert!(index.indexer(&a).is_none(), "a closed folder is still open");
        fs::write(a.join("d.rs"), "fn after_close() {}\n").unwrap();
        fs::write(b.join("e.rs"), "fn saved_in_b() {}\n").unwrap();
        until_found(&index, &b, "saved_in_b");
        std::thread::sleep(Duration::from_secs(1));
        assert_eq!(finds_in(&left_behind, "after_close"), 0, "the closed folder is still watched");
        drop(in_a);
        assert!(index.indexer(&a).is_none(), "the closed folder's searcher reopened nothing");
    }

    /// The store is where the app keeps its data, not in the folder.
    #[test]
    fn the_store_is_not_written_into_the_folder() {
        let index = workspace_index("ws-clean");
        let root = repo("ws-clean-repo", "a.rs", "fn a() {}\n");
        let (sink, events) = channel_sink();
        index.activate(&root, sink).unwrap();
        next_finish(&events);

        let entries: Vec<_> = fs::read_dir(&root).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(entries, ["a.rs"]);
        assert_eq!(fs::read_dir(&index.index_dir).unwrap().count(), 1);
    }

    #[test]
    fn a_store_that_will_not_open_is_reported_and_leaves_nothing_open() {
        let blocked = temp_dir("ws-blocked").join("not-a-dir");
        fs::write(&blocked, "").unwrap();
        let index = Arc::new(WorkspaceIndex::new(blocked, Arc::new(FlatModel)));
        let (sink, events) = channel_sink();

        let repo = temp_dir("ws-blocked-repo");
        assert!(index.activate(&repo, Arc::clone(&sink)).is_err());
        assert!(matches!(events.try_recv(), Ok(IndexEvent::Failed { .. })));
        assert!(index.indexer(&repo).is_none());

        let other = temp_dir("ws-blocked-other");
        assert!(index.searcher(&other, sink).is_none(), "a searcher of a store that will not open");
        assert!(matches!(events.try_recv(), Ok(IndexEvent::Failed { .. })));
    }

    /// Opening the next folder fails: it is not open, and the folder open
    /// before it still is.
    #[test]
    fn a_failed_open_leaves_the_other_folders_open() {
        let index = workspace_index("ws-fail-after");
        let first = temp_dir("ws-fail-after-first");
        index.activate(&first, quiet()).unwrap();
        let second = temp_dir("ws-fail-after-second");
        fs::write(index.index_dir.join(repository_id(&second)), "").unwrap();

        assert!(index.searcher(&second, quiet()).is_none());

        assert!(index.indexer(&second).is_none());
        assert!(index.indexer(&first).is_some());
    }

    /// A folder that cannot be watched is still indexed once and searchable;
    /// the window hears why it will not follow changes.
    #[test]
    fn a_folder_that_cannot_be_watched_is_reported_but_kept() {
        let index = workspace_index("ws-unwatched");
        let missing = temp_dir("ws-unwatched-repo").join("gone");
        let (sink, events) = channel_sink();

        assert!(matches!(index.activate(&missing, sink), Err(WorkspaceIndexError::Watch(_))));

        assert!(index.indexer(&missing).is_some());
        let background = temp_dir("ws-unwatched-other").join("gone");
        assert!(index.searcher(&background, quiet()).is_some(), "a searcher of an unwatchable folder still searches");
        let said = std::iter::from_fn(|| events.recv_timeout(Duration::from_secs(5)).ok())
            .any(|event| matches!(&event, IndexEvent::Failed { error } if error.contains("cannot be watched")));
        assert!(said, "the window was not told");
    }

    /// Closing the active folder leaves no folder active: the next one made
    /// active does not keep the closed one's index alive.
    #[test]
    fn closing_the_active_folder_leaves_none_active() {
        let index = workspace_index("ws-close-active");
        let a = temp_dir("ws-close-active-a");
        index.activate(&a, quiet()).unwrap();

        index.close(&a);
        drop(index.searcher(&a, quiet()).unwrap());

        assert!(index.indexer(&a).is_none(), "a closed folder stayed active");
    }
}
