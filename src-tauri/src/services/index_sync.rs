//! One repository's index, end to end: the keyword pass, then the vectors,
//! one sync at a time, reported as it goes.
//!
//! ## What upstream's 2 643 lines were, and why ~250 are here
//!
//! `embedding_sync` + `embedding_state` split a sync into tiers — files under
//! the docs root and open editor tabs inline, the rest into a background
//! backlog drained 25 files at a time — kept an in-memory repository index
//! and chunk index in step with the store, cooled down after a remote
//! provider's outage, repaired a model's dimensions, and adopted vectors from
//! a legacy layout. All of it served one of three things this port does not
//! have: embedding that took minutes (a remote model, then `bge-m3`), maps in
//! memory beside the store (F-5.6 made the store the only copy), and a remote
//! service to fail. Measured here: the whole of upstream's own repository
//! embeds in 0.6 s.
//!
//! What stays is what those tiers were *for*:
//!
//! * **The first sync does not hold anything up.** It runs on the caller's
//!   thread — the command layer puts it on a blocking pool — and search works
//!   throughout: keywords from the moment [`IndexEvent::KeywordsReady`] is
//!   sent, meaning from whatever is embedded so far (`EmbeddingIndex` takes
//!   its write lock one batch at a time).
//! * **One sync at a time.** A request during a sync does not start a second;
//!   it marks the running one to go round once more, so an edit made while
//!   syncing is never left out and a burst of saves costs two passes, not
//!   one per save.
//! * **A save does not load the model.** Nothing is embedded unless a chunk
//!   changed, and the bundled model loads on its first embed — so a sync that
//!   finds nothing new never touches the weights ("must not lose" item 2).
//! * **Meaning failing does not take keywords with it.** A missing model is
//!   reported in the finish event and the status; the keyword pass already
//!   landed.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use crate::domain::chunk_index::{ChunkBuildOptions, ChunkId};
use crate::domain::embeddings::{EmbeddingError, EmbeddingProvider, LOCAL_MODEL_ID};
use crate::domain::workspace_index::{IndexEvent, IndexEventSink};
use crate::infra::index_store::{IndexStore, IndexStoreError};
use crate::services::embedding_index::{EmbeddingIndex, EMBEDDED_TEXT_VERSION};
use crate::services::repo_index::{self, RepoSyncError};

/// What the last sync left behind, for a status line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IndexStatus {
    pub syncing: bool,
    /// Chunks with a vector — how much of the repository search by meaning
    /// covers.
    pub embedded: usize,
    /// `(path, reason)` for every file the last sync would not take. Strings,
    /// because this is what a person reads; the typed reason is in the sync's
    /// own report.
    pub skipped: Vec<(String, String)>,
    pub embedding_error: Option<String>,
}

pub struct RepoIndexer {
    root: PathBuf,
    store: IndexStore,
    embeddings: EmbeddingIndex,
    provider: Arc<dyn EmbeddingProvider>,
    options: ChunkBuildOptions,
    run: Mutex<RunState>,
    status: Mutex<IndexStatus>,
    history: Mutex<Option<CommitHistory>>,
}

/// Recent commits with their messages embedded, as of one HEAD.
struct CommitHistory {
    head: String,
    commits: Vec<(Vec<f32>, Vec<String>)>,
}

/// How far back history is read, and how many files a commit may touch
/// and still say something about each of them.
const HISTORY_COMMITS: usize = 300;
const HISTORY_MAX_FILES: usize = 20;
/// How many of the commits nearest a query count, and how near is near.
const HISTORY_MATCHES: usize = 3;
const HISTORY_MIN_SIMILARITY: f32 = 0.5;

#[derive(Default)]
struct RunState {
    running: bool,
    again: bool,
}

impl RepoIndexer {
    /// Opens the store in `store_dir` and loads what it already embedded.
    /// Reads no file of the repository and loads no model.
    pub fn open(
        root: &Path,
        store_dir: &Path,
        provider: Arc<dyn EmbeddingProvider>,
        options: ChunkBuildOptions,
    ) -> Result<Self, IndexStoreError> {
        let store = IndexStore::open(store_dir)?;
        let model_id = format!("{LOCAL_MODEL_ID}{EMBEDDED_TEXT_VERSION}");
        let embeddings = EmbeddingIndex::load(&store, &model_id, provider.dimensions())?;
        let status = IndexStatus { embedded: embeddings.len(), ..IndexStatus::default() };
        Ok(Self {
            root: root.to_path_buf(),
            store,
            embeddings,
            provider,
            options,
            run: Mutex::new(RunState::default()),
            status: Mutex::new(status),
            history: Mutex::new(None),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn store(&self) -> &IndexStore {
        &self.store
    }

    pub fn status(&self) -> IndexStatus {
        self.status.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Brings the index in line with the working tree.
    ///
    /// `Ok(None)` when a sync was already running: this request was folded
    /// into it, and it will go round once more before it returns. `Err` only
    /// when the keyword pass failed — the walk or the store; a failure to
    /// embed is reported, not returned.
    pub fn sync(&self, sink: &IndexEventSink) -> Result<Option<IndexStatus>, RepoSyncError> {
        if !self.claim() {
            return Ok(None);
        }
        let mut release = Release { run: &self.run, armed: true };
        self.set_status(|status| status.syncing = true);
        loop {
            // The status settles before the event goes out: a listener re-reads
            // it on `SyncFinished`, and must not find this sync still running.
            match self.run_once(sink) {
                Ok((embedded, embedding_error)) => {
                    let again = self.go_again(&mut release);
                    sink(IndexEvent::SyncFinished { embedded, embedding_error });
                    if !again {
                        return Ok(Some(self.status()));
                    }
                }
                Err(error) => {
                    sink(IndexEvent::Failed { error: error.to_string() });
                    return Err(error);
                }
            }
        }
    }

    /// Search by meaning: the `top_k` chunks nearest `query`, best first.
    ///
    /// With nothing embedded yet this answers empty without touching the
    /// model — asking it to embed a query would load half a gigabyte to
    /// compare against nothing.
    pub fn search_meaning(&self, query: &str, top_k: usize) -> Result<Vec<(ChunkId, f32)>, EmbeddingError> {
        if self.embeddings.is_empty() {
            return Ok(Vec::new());
        }
        let vector = self.provider.embed(&[query])?.into_iter().next().map(|e| e.0).unwrap_or_default();
        Ok(self.embeddings.search(&vector, top_k))
    }

    /// Files touched by the commits whose messages are nearest `query` in
    /// meaning — at most a few commits, and only near ones. Empty outside a
    /// repository, with nothing embedded, or when the model cannot be asked.
    ///
    /// The commits are read and embedded on the first search after HEAD
    /// moves, not at sync.
    // ponytail: rebuilt in the search that notices a new HEAD (a few hundred
    // diffs); move it into `sync` if the first search after a commit shows it.
    pub fn files_by_history(&self, query: &str) -> HashSet<String> {
        if self.embeddings.is_empty() {
            return HashSet::new();
        }
        let Some(head) = crate::infra::git_history::head(&self.root) else {
            return HashSet::new();
        };
        let mut history = self.history.lock().unwrap_or_else(PoisonError::into_inner);
        if history.as_ref().is_none_or(|h| h.head != head) {
            let notes = crate::infra::git_history::recent_commits(&self.root, HISTORY_COMMITS, HISTORY_MAX_FILES);
            let messages: Vec<&str> = notes.iter().map(|n| n.message.as_str()).collect();
            let Ok(vectors) = self.provider.embed(&messages) else {
                return HashSet::new();
            };
            let commits = vectors.into_iter().zip(notes).map(|(v, n)| (v.0, n.files)).collect();
            *history = Some(CommitHistory { head, commits });
        }
        let Some(history) = history.as_ref() else {
            return HashSet::new();
        };
        let Ok(query) = self.provider.embed(&[query]) else {
            return HashSet::new();
        };
        let Some(query) = query.into_iter().next().map(|e| e.0) else {
            return HashSet::new();
        };
        let mut near: Vec<(f32, &Vec<String>)> = history
            .commits
            .iter()
            .map(|(vector, files)| (cosine(vector, &query), files))
            .filter(|(similarity, _)| *similarity >= HISTORY_MIN_SIMILARITY)
            .collect();
        near.sort_by(|a, b| b.0.total_cmp(&a.0));
        near.into_iter().take(HISTORY_MATCHES).flat_map(|(_, files)| files.iter().cloned()).collect()
    }

    /// One pass; what it embedded and why it could not, for `SyncFinished`.
    fn run_once(&self, sink: &IndexEventSink) -> Result<(usize, Option<String>), RepoSyncError> {
        sink(IndexEvent::SyncStarted);
        let report = repo_index::sync(&self.root, &self.store, &self.options)?;
        let skipped: Vec<(String, String)> =
            report.skipped.iter().map(|s| (s.path.clone(), s.reason.to_string())).collect();
        sink(IndexEvent::KeywordsReady {
            indexed: report.indexed,
            unchanged: report.unchanged,
            removed: report.removed,
            skipped: skipped.len(),
        });

        let embedded = self.embeddings.sync(&self.root, &self.store, self.provider.as_ref(), &mut |done, total| {
            sink(IndexEvent::EmbeddingProgress { done, total })
        });
        let (count, error) = match embedded {
            Ok(stats) => (stats.embedded, None),
            Err(error) => (0, Some(error.to_string())),
        };
        self.set_status(|status| {
            status.embedded = self.embeddings.len();
            status.skipped = skipped;
            status.embedding_error = error.clone();
        });
        Ok((count, error))
    }

    /// Takes the run, or — if a sync holds it — asks that sync to go round
    /// once more and reports `false`.
    fn claim(&self) -> bool {
        let mut run = self.run.lock().unwrap_or_else(PoisonError::into_inner);
        if run.running {
            run.again = true;
            false
        } else {
            run.running = true;
            true
        }
    }

    /// Whether another request arrived during this pass. Deciding "no",
    /// clearing `syncing` and giving the run up happen under one lock: a
    /// request arriving between them would otherwise see a sync that is
    /// running and about to stop, fold itself into it, and be lost — or claim
    /// the run and then have its `syncing` cleared.
    fn go_again(&self, release: &mut Release<'_>) -> bool {
        let mut run = self.run.lock().unwrap_or_else(PoisonError::into_inner);
        if run.again {
            run.again = false;
            true
        } else {
            self.set_status(|status| status.syncing = false);
            run.running = false;
            release.armed = false;
            false
        }
    }

    fn set_status(&self, change: impl FnOnce(&mut IndexStatus)) {
        change(&mut self.status.lock().unwrap_or_else(PoisonError::into_inner));
    }
}

/// Gives the run up if the sync leaves by an error or a panic, so one failed
/// sync does not make every later request fold into a sync that is gone.
struct Release<'a> {
    run: &'a Mutex<RunState>,
    armed: bool,
}

impl Drop for Release<'_> {
    fn drop(&mut self) {
        if self.armed {
            let mut run = self.run.lock().unwrap_or_else(PoisonError::into_inner);
            run.running = false;
            run.again = false;
        }
    }
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm = |v: &[f32]| v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
    dot / (norm(a) * norm(b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::embeddings::Embedding;
    use crate::testing::temp_dir;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

    const DIMS: usize = 16;

    /// Hashes words into buckets — texts sharing words point the same way.
    /// Counts calls; can fail, or stop at a given call until released.
    #[derive(Default)]
    struct FakeModel {
        calls: AtomicUsize,
        fail: bool,
        hold_at: Option<(usize, Mutex<mpsc::Receiver<()>>, mpsc::Sender<()>)>,
    }

    fn vector(text: &str) -> Vec<f32> {
        let mut v = vec![0.0_f32; DIMS];
        for word in text.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()) {
            v[blake3::hash(word.as_bytes()).as_bytes()[0] as usize % DIMS] += 1.0;
        }
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
        v.iter().map(|x| x / norm).collect()
    }

    impl EmbeddingProvider for FakeModel {
        fn embed(&self, texts: &[&str]) -> Result<Vec<Embedding>, EmbeddingError> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some((at, release, reached)) = &self.hold_at {
                if call == *at {
                    reached.send(()).unwrap();
                    release.lock().unwrap().recv().unwrap();
                }
            }
            if self.fail {
                return Err(EmbeddingError::NotFound(PathBuf::from("model.safetensors")));
            }
            Ok(texts.iter().map(|t| Embedding(vector(t))).collect())
        }
        fn dimensions(&self) -> usize {
            DIMS
        }
    }

    fn recorder() -> (IndexEventSink, Arc<Mutex<Vec<IndexEvent>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink_events = Arc::clone(&events);
        (Arc::new(move |event| sink_events.lock().unwrap().push(event)), events)
    }

    fn indexer(label: &str, model: Arc<FakeModel>) -> (RepoIndexer, PathBuf) {
        let root = temp_dir(&format!("{label}-repo"));
        let indexer = RepoIndexer::open(&root, &temp_dir(&format!("{label}-store")), model, ChunkBuildOptions::default()).unwrap();
        (indexer, root)
    }

    fn kinds(events: &[IndexEvent]) -> Vec<&'static str> {
        events
            .iter()
            .map(|e| match e {
                IndexEvent::SyncStarted => "started",
                IndexEvent::KeywordsReady { .. } => "keywords",
                IndexEvent::EmbeddingProgress { .. } => "progress",
                IndexEvent::SyncFinished { .. } => "finished",
                IndexEvent::Failed { .. } => "failed",
            })
            .collect()
    }

    // ---------------------------------------------------------- a sync

    #[test]
    fn a_sync_reports_keywords_then_vectors_then_the_finish() {
        let model = Arc::new(FakeModel::default());
        let (indexer, root) = indexer("indexer-events", Arc::clone(&model));
        fs::write(root.join("a.rs"), "fn alpha() {}\nfn beta() {}\n").unwrap();
        let (sink, events) = recorder();

        let status = indexer.sync(&sink).unwrap().expect("nothing else was running");

        let events = events.lock().unwrap();
        assert_eq!(kinds(&events), ["started", "keywords", "progress", "finished"]);
        assert_eq!(events[1], IndexEvent::KeywordsReady { indexed: 1, unchanged: 0, removed: 0, skipped: 0 });
        assert_eq!(events[3], IndexEvent::SyncFinished { embedded: 2, embedding_error: None });
        assert_eq!((status.embedded, status.syncing), (2, false));
        assert_eq!(indexer.search_meaning("alpha", 1).unwrap().len(), 1);
    }

    /// "Must not lose" item 2: a sync that finds nothing new does not ask the
    /// model for anything, so the bundled weights are never loaded for it.
    #[test]
    fn a_sync_with_nothing_new_never_asks_the_model() {
        let model = Arc::new(FakeModel::default());
        let (indexer, root) = indexer("indexer-noop", Arc::clone(&model));
        fs::write(root.join("a.rs"), "fn alpha() {}\n").unwrap();
        let (sink, _) = recorder();
        indexer.sync(&sink).unwrap();
        let calls = model.calls.load(Ordering::SeqCst);

        indexer.sync(&sink).unwrap();

        assert_eq!(model.calls.load(Ordering::SeqCst), calls);
    }

    /// Nothing embedded, nothing to compare with: the query is not embedded
    /// either.
    #[test]
    fn searching_an_empty_index_never_asks_the_model() {
        let model = Arc::new(FakeModel::default());
        let (indexer, _root) = indexer("indexer-empty-search", Arc::clone(&model));

        assert!(indexer.search_meaning("anything", 5).unwrap().is_empty());
        assert_eq!(model.calls.load(Ordering::SeqCst), 0);
    }

    /// A missing model is the realistic case — a clone without `git lfs pull`.
    /// The keyword index must still land, and the failure must be said.
    #[test]
    fn a_model_that_fails_leaves_keyword_search_working_and_says_why() {
        let model = Arc::new(FakeModel { fail: true, ..FakeModel::default() });
        let (indexer, root) = indexer("indexer-no-model", model);
        fs::write(root.join("a.rs"), "fn findable_by_keyword() {}\n").unwrap();
        let (sink, events) = recorder();

        let status = indexer.sync(&sink).unwrap().unwrap();

        assert!(status.embedding_error.as_deref().is_some_and(|e| e.contains("model.safetensors")), "{status:?}");
        let finished = events.lock().unwrap().last().cloned().unwrap();
        assert!(matches!(finished, IndexEvent::SyncFinished { embedding_error: Some(_), .. }));
        let query = crate::domain::search_query::fts5_query("findable_by_keyword").unwrap();
        assert_eq!(indexer.store().search_bm25(&query, 5).unwrap().len(), 1);
    }

    #[test]
    fn a_refused_file_is_in_the_status_with_its_reason() {
        let (indexer, root) = indexer("indexer-skipped", Arc::new(FakeModel::default()));
        fs::write(root.join("logo.png"), b"\x89PNG\0\0").unwrap();
        let (sink, _) = recorder();

        let status = indexer.sync(&sink).unwrap().unwrap();

        assert_eq!(status.skipped, [("logo.png".to_string(), "binary".to_string())]);
    }

    /// A sync that failed must give the run up, or every later request folds
    /// into a sync that no longer exists and nothing is ever indexed again.
    #[test]
    fn a_failed_sync_does_not_block_the_next_one() {
        let (indexer, root) = indexer("indexer-failed", Arc::new(FakeModel::default()));
        let (sink, events) = recorder();
        fs::remove_dir_all(&root).unwrap();
        assert!(indexer.sync(&sink).is_err());
        // A window showing "syncing" would otherwise show it for good.
        assert_eq!(kinds(&events.lock().unwrap()), ["started", "failed"]);

        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("a.rs"), "fn alpha() {}\n").unwrap();
        assert!(indexer.sync(&sink).unwrap().is_some(), "the run was never given up");
    }

    /// A listener re-reads the status on `SyncFinished` or `Failed`; by then it
    /// must no longer say "syncing", or a window that read it keeps saying so.
    #[test]
    fn the_status_has_settled_when_the_end_is_reported() {
        let (indexer, root) = indexer("indexer-settled", Arc::new(FakeModel::default()));
        fs::write(root.join("a.rs"), "fn alpha() {}\n").unwrap();
        let indexer = Arc::new(indexer);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink: IndexEventSink = {
            let (indexer, seen) = (Arc::clone(&indexer), Arc::clone(&seen));
            Arc::new(move |event| {
                if matches!(event, IndexEvent::SyncFinished { .. } | IndexEvent::Failed { .. }) {
                    seen.lock().unwrap().push(indexer.status().syncing);
                }
            })
        };

        indexer.sync(&sink).unwrap();
        fs::remove_dir_all(&root).unwrap();
        assert!(indexer.sync(&sink).is_err());

        assert_eq!(*seen.lock().unwrap(), [false, false]);
    }

    // ------------------------------------------------------ concurrency

    /// Held in its second batch: the first 64 chunks are embedded, the rest
    /// are not. A request now folds into the running sync, and a search
    /// answers from the part that is done.
    #[test]
    fn during_a_sync_requests_fold_in_and_search_answers_from_what_is_done() {
        let (release_tx, release_rx) = mpsc::channel();
        let (reached_tx, reached_rx) = mpsc::channel();
        let model = Arc::new(FakeModel { hold_at: Some((1, Mutex::new(release_rx), reached_tx)), ..FakeModel::default() });
        let (indexer, root) = indexer("indexer-concurrent", Arc::clone(&model));
        let body: String = (0..100).map(|i| format!("fn handler_{i}() {{}}\n")).collect();
        fs::write(root.join("many.rs"), body).unwrap();
        let indexer = Arc::new(indexer);
        let (sink, events) = recorder();

        let running = {
            let (indexer, sink) = (Arc::clone(&indexer), Arc::clone(&sink));
            std::thread::spawn(move || indexer.sync(&sink))
        };
        reached_rx.recv().unwrap();

        // Bounded, and releasing the sync on the way out: a search that waits
        // for the sync would otherwise wait for this test to release it, and
        // the test would hang instead of failing.
        let (answer_tx, answer_rx) = mpsc::channel();
        {
            let indexer = Arc::clone(&indexer);
            std::thread::spawn(move || answer_tx.send(indexer.search_meaning("handler_3", 200).map(|hits| hits.len())));
        }
        let Ok(found) = answer_rx.recv_timeout(std::time::Duration::from_secs(5)) else {
            release_tx.send(()).unwrap();
            panic!("search waited for the whole sync");
        };
        assert_eq!(found.unwrap(), 64, "search did not answer from the batch already embedded");
        assert!(indexer.status().syncing);

        fs::write(root.join("late.rs"), "fn written_during_the_sync() {}\n").unwrap();
        assert!(indexer.sync(&sink).unwrap().is_none(), "a second sync started beside the first");

        release_tx.send(()).unwrap();
        let status = running.join().unwrap().unwrap().unwrap();

        let started = kinds(&events.lock().unwrap()).iter().filter(|k| **k == "started").count();
        assert_eq!(started, 2, "the folded request did not make the sync go round again");
        assert_eq!(status.embedded, 101, "the file written during the sync was left out");
        assert!(!indexer.status().syncing);
    }

    // ------------------------------------------------------ with the watcher

    /// The wiring F-5.12 will do, end to end: a file saved on disk becomes
    /// findable with nobody calling `sync`.
    #[test]
    fn a_saved_file_becomes_findable_through_the_watcher() {
        let (indexer, root) = indexer("indexer-watched", Arc::new(FakeModel::default()));
        let indexer = Arc::new(indexer);
        let (sink, _) = recorder();
        let (synced_tx, synced) = mpsc::channel();
        let watched = Arc::clone(&indexer);
        let _watcher = crate::infra::file_watcher::FileWatcher::start(&root, move || {
            let _ = synced_tx.send(watched.sync(&sink).map(|status| status.is_some()));
        })
        .unwrap();

        fs::write(root.join("a.rs"), "fn saved_while_watched() {}\n").unwrap();

        assert!(matches!(synced.recv_timeout(std::time::Duration::from_secs(10)), Ok(Ok(true))));
        let query = crate::domain::search_query::fts5_query("saved_while_watched").unwrap();
        assert_eq!(indexer.store().search_bm25(&query, 5).unwrap().len(), 1);
    }
}
