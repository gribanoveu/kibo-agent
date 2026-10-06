//! The bundled Model2Vec model: `potion-multilingual-128M`, quantized to int8
//! and cut down to Russian and English (F-5.8b).
//!
//! A *static* model: a vector per token, averaged. No network, no ONNX
//! runtime, no GPU — embedding a chunk is a tokenizer pass and a lookup, which
//! is what makes indexing a whole repository on a laptop cheap enough to do on
//! open. Multilingual because questions here arrive in Russian about code
//! written in English.
//!
//! The weights ship with the app (`tauri.conf.json` → `bundle.resources`) and
//! live in the repository under Git LFS; `scripts/embedding-model.md` says how
//! they are built and checked.
//!
//! ## Only Russian and English — a deliberate limit
//!
//! The upstream model's vocabulary is 500 353 tokens in a hundred languages.
//! `scripts/prune-embedding-vocab.py` keeps the 271 538 whose letters are all
//! ASCII or Cyrillic, and the matching rows of the table. For text in
//! Russian, English and code **nothing changes**: a dropped token contains a
//! letter from another script, so it can never match such text, and the
//! tokenizer picks exactly the same pieces. Checked over two repositories,
//! 10 957 fragments: identical tokenization everywhere except the six that
//! contained `é`, `à`, `µ`, `Σ` or `世界`. Those characters now become
//! `[UNK]`, which is dropped before pooling — they embed as nothing. Going back to the full model is the build script without the
//! prune step, the directory name below, and a new `LOCAL_MODEL_ID`.
//!
//! ## Cost — measured, not estimated (release, macOS, 2026-09-18)
//!
//! | | full, `f32` table | ru + en, `f32` table | **ru + en, int8 table** |
//! |---|---|---|---|
//! | on disk | 146 MB | 80 MB | 80 MB |
//! | resident once loaded | ~1.07 GB | ~520 MB | **~320 MB** |
//! | load | 0.7 s | 0.42 s | 0.35 s |
//!
//! Resident is now mostly the tokenizer — a Unigram prefix tree with a node
//! per character of every token, ~250 MB — plus the table at one byte a
//! weight, ~70 MB. The `f32` columns are what `model2vec-rs` cost, which
//! widened the table on load; [`Int8Model`] replaced it (F-5.8c) and pools the
//! int8 rows directly, with the same result to the bit on every text of two
//! repositories that has no `[UNK]` in it. Upstream's notes said 512 MB for
//! the full model: that was the table alone.
//!
//! Embedding is cheap once loaded, and cheaper with int8 (a quarter of the
//! bytes to walk): 2 000 short texts in 13.6 ms against 19.7 ms; all 1 911
//! chunks of this repository took 150 ms on the `f32` table.
//!
//! So the model is **loaded on first use and unloaded when idle**: after
//! [`DEFAULT_IDLE_UNLOAD`] with no embedding, a background thread drops it and
//! hands the freed memory back to the OS. The next embed pays the load
//! again. A project nobody searches semantically never pays at all.
//!
//! Dropping is not enough on its own. The tokenizer is millions of small
//! allocations, and the allocator keeps freed small blocks for reuse: measured
//! on the full model, a dropped one still left **375 MB** resident.
//! [`release_freed_memory`] asks the allocator to return them — down to ~50 MB.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::thread;
use std::time::{Duration, Instant};

use tokenizers::Tokenizer;

use crate::domain::embeddings::{Embedding, EmbeddingError, EmbeddingProvider};

pub const DIMENSIONS: usize = 256;
const MODEL_DIR_NAME: &str = "potion-multilingual-128M-int8-ru-en";
const LFS_POINTER_PREFIX: &[u8] = b"version https://git-lfs.github.com/spec/v1";

/// Tokens per text before the rest is cut off — `model2vec`'s own default,
/// and the one `parity.json` was computed with. A static model has no
/// position limit, so this is about meaning, not capacity: the mean of four
/// thousand token vectors says less than the mean of the first five hundred.
/// A median chunk here is ~470 bytes and well inside it.
const MAX_TOKENS: usize = 512;
const BATCH_SIZE: usize = 512;

/// Where the bundled model is: under the app's resource directory in a built
/// app, and under this crate in development and tests.
///
/// The resource directory comes from the caller — Tauri knows it, `infra`
/// does not ask Tauri. Upstream guessed it from `current_exe()` with three
/// layouts in mind, none of them Linux's (`/usr/lib/<app>`).
pub fn bundled_model_dir(resource_dir: Option<&Path>) -> PathBuf {
    resource_dir
        .map(|dir| dir.join("models").join(MODEL_DIR_NAME))
        .filter(|dir| dir.join("model.safetensors").is_file())
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("models").join(MODEL_DIR_NAME))
}

/// How long the model stays loaded after its last use. Long enough that a
/// sync followed by a few searches loads it once; short enough that a window
/// left open overnight is not holding a gigabyte.
pub const DEFAULT_IDLE_UNLOAD: Duration = Duration::from_secs(10 * 60);

/// The bundled model behind a lazy, self-unloading slot.
///
/// **Construct one per process** — the workspace index owns it. Each instance
/// has its own slot, so two would be able to hold two copies.
pub struct LocalEmbeddings {
    dir: PathBuf,
    cell: IdleCell<Int8Model>,
}

impl LocalEmbeddings {
    /// Nothing is read until the first [`embed`](EmbeddingProvider::embed).
    /// Constructing this on every file save is free — which is what "must not
    /// lose" item 2 of stage 5 asks of the incremental tick.
    pub fn new(dir: PathBuf, idle: Duration) -> Self {
        Self { dir, cell: IdleCell::new(idle) }
    }

    /// Whether the weights are in memory right now.
    pub fn is_loaded(&self) -> bool {
        self.cell.is_loaded()
    }
}

/// A value loaded on first use and dropped after `idle` without use.
///
/// Generic only so its tests can run on a number instead of a gigabyte of
/// weights; the model is the one thing that lives in it.
struct IdleCell<T> {
    idle: Duration,
    slot: Arc<Mutex<Slot<T>>>,
}

struct Slot<T> {
    value: Option<Arc<T>>,
    last_used: Option<Instant>,
    reaper_running: bool,
}

impl<T: Send + Sync + 'static> IdleCell<T> {
    fn new(idle: Duration) -> Self {
        Self {
            idle,
            slot: Arc::new(Mutex::new(Slot { value: None, last_used: None, reaper_running: false })),
        }
    }

    fn is_loaded(&self) -> bool {
        self.slot.lock().is_ok_and(|slot| slot.value.is_some())
    }

    /// The value, loading it if needed and starting the reaper that will drop
    /// it. A failed load is not remembered: the next call tries again, so a
    /// missing model starts working after `git lfs pull` without a restart.
    ///
    /// The lock is held only to swap the `Arc`, never while the caller uses
    /// the value: an unload during an embed drops the slot's reference, and
    /// the embed finishes on its own.
    fn get_or_load(&self, load: impl FnOnce() -> Result<T, EmbeddingError>) -> Result<Arc<T>, EmbeddingError> {
        let mut slot = self.slot.lock().map_err(|_| EmbeddingError::Invalid("model slot poisoned".into()))?;
        let value = match &slot.value {
            Some(value) => Arc::clone(value),
            None => {
                let value = Arc::new(load()?);
                slot.value = Some(Arc::clone(&value));
                value
            }
        };
        slot.last_used = Some(Instant::now());
        if !slot.reaper_running {
            slot.reaper_running = true;
            spawn_reaper(Arc::downgrade(&self.slot), self.idle);
        }
        Ok(value)
    }
}

/// Wakes a few times per idle period and unloads once the value has not been
/// used for a whole one. Holds the slot weakly, so dropping the cell ends the
/// thread at its next wake; exits after an unload, and the next load starts a
/// new one.
fn spawn_reaper<T: Send + Sync + 'static>(slot: Weak<Mutex<Slot<T>>>, idle: Duration) {
    let tick = (idle / 4).max(Duration::from_millis(10));
    thread::spawn(move || loop {
        thread::sleep(tick);
        let Some(slot) = slot.upgrade() else { return };
        let Ok(mut guard) = slot.lock() else { return };
        if guard.last_used.is_some_and(|at| at.elapsed() >= idle) {
            guard.value = None;
            guard.reaper_running = false;
            drop(guard);
            release_freed_memory();
            return;
        }
    });
}

/// Asks the allocator to hand freed memory back to the OS.
///
/// Without it, an unloaded model left 375 MB resident — the tokenizer's small
/// blocks, kept by the allocator for reuse. Best effort: a no-op where there
/// is no such call. On Windows the process heap has none that decommits, and
/// the unload returns only what was allocated in large blocks (the 512 MB
/// table), not the tokenizer.
fn release_freed_memory() {
    #[cfg(target_os = "macos")]
    {
        extern "C" {
            fn malloc_zone_pressure_relief(zone: *mut std::ffi::c_void, goal: usize) -> usize;
        }
        // SAFETY: a null zone means "all zones" and a zero goal means "as much
        // as possible"; the call only returns free pages and touches no live
        // allocation.
        unsafe {
            malloc_zone_pressure_relief(std::ptr::null_mut(), 0);
        }
    }
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        extern "C" {
            fn malloc_trim(pad: usize) -> std::ffi::c_int;
        }
        // SAFETY: glibc's own call; trims free memory from the heap top and
        // arenas, and touches no live allocation.
        unsafe {
            malloc_trim(0);
        }
    }
}

impl EmbeddingProvider for LocalEmbeddings {
    fn embed(&self, texts: &[&str]) -> Result<Vec<Embedding>, EmbeddingError> {
        let model = self.cell.get_or_load(|| load_model(&self.dir))?;
        Ok(model.encode(texts)?.into_iter().map(Embedding).collect())
    }

    fn dimensions(&self) -> usize {
        DIMENSIONS
    }
}

/// A static embedding model with its table kept as the int8 bytes it ships
/// as.
///
/// This replaces `model2vec-rs`, which widened the table to `f32` on load —
/// four bytes a weight where one holds the same number. The arithmetic is
/// identical: an int8 value converts to `f32` exactly, so summing converted
/// bytes gives the same vector as summing a pre-converted table. The parity
/// test against the Python vectors checks that; the saving is ~210 MB
/// resident (F-5.8c).
struct Int8Model {
    tokenizer: Tokenizer,
    /// The whole `model.safetensors` file, as read. Rows start at
    /// `table_start`, each `DIMENSIONS` bytes; kept as `u8` and reinterpreted
    /// per weight, so loading copies nothing.
    bytes: Vec<u8>,
    table_start: usize,
    rows: usize,
    /// `[UNK]`, dropped before pooling. `model2vec-rs` meant to do the same
    /// but looked the token up by a `unk_token` field that a Unigram
    /// tokenizer does not have, found nothing, and averaged `[UNK]` into
    /// every text containing a character outside the vocabulary.
    unk_id: Option<u32>,
    /// Texts are cut to `MAX_TOKENS × this` characters before tokenizing — the
    /// same pre-truncation `model2vec` does, so a megabyte of text is not
    /// tokenized to keep its first 512 tokens.
    median_token_chars: usize,
}

impl Int8Model {
    fn encode(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        let mut out = Vec::with_capacity(texts.len());
        for batch in texts.chunks(BATCH_SIZE) {
            let truncated: Vec<String> = batch
                .iter()
                .map(|text| {
                    let limit = MAX_TOKENS.saturating_mul(self.median_token_chars);
                    text.char_indices().nth(limit).map_or(*text, |(at, _)| &text[..at]).to_string()
                })
                .collect();
            let encodings = self
                .tokenizer
                .encode_batch_fast(truncated, false)
                .map_err(|e| EmbeddingError::Invalid(format!("tokenization failed: {e}")))?;
            for encoding in encodings {
                let ids = encoding.get_ids().iter().copied().filter(|id| Some(*id) != self.unk_id).take(MAX_TOKENS);
                out.push(self.pool(ids));
            }
        }
        Ok(out)
    }

    /// The mean of the rows at unit length — computed as the sum at unit
    /// length, which is the same vector: scaling by the count changes the
    /// length, and the length is then thrown away. The model's config says
    /// `"normalize": true`; were a future model's not to, the parity test
    /// would say so.
    fn pool(&self, ids: impl Iterator<Item = u32>) -> Vec<f32> {
        let mut sum = vec![0.0_f32; DIMENSIONS];
        for id in ids {
            let id = id as usize;
            if id >= self.rows {
                continue;
            }
            let row = &self.bytes[self.table_start + id * DIMENSIONS..][..DIMENSIONS];
            for (acc, &byte) in sum.iter_mut().zip(row) {
                *acc += f32::from(byte as i8);
            }
        }
        let norm = sum.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
        sum.iter_mut().for_each(|x| *x /= norm);
        sum
    }
}

fn load_model(dir: &Path) -> Result<Int8Model, EmbeddingError> {
    let tokenizer_bytes = read_model_file(&dir.join("tokenizer.json"))?;
    let bytes = read_model_file(&dir.join("model.safetensors"))?;
    let invalid = |why: String| EmbeddingError::Invalid(why);

    let tokenizer = Tokenizer::from_bytes(&tokenizer_bytes).map_err(|e| invalid(format!("tokenizer: {e}")))?;
    drop(tokenizer_bytes);

    let (table_start, rows, width) = int8_table(&bytes).map_err(invalid)?;
    // Every stored vector is `DIMENSIONS` wide. Weights from another model
    // are refused here, not discovered as a length mismatch in the store.
    if width != DIMENSIONS {
        return Err(invalid(format!("vectors are {width} wide, the index expects {DIMENSIONS}")));
    }
    if rows != tokenizer.get_vocab_size(false) {
        return Err(invalid(format!(
            "the table has {rows} rows for {} tokens",
            tokenizer.get_vocab_size(false)
        )));
    }

    let unk_id = tokenizer_unk_id(&tokenizer);
    let mut lengths: Vec<usize> = tokenizer.get_vocab(false).keys().map(String::len).collect();
    lengths.sort_unstable();
    let median_token_chars = lengths.get(lengths.len() / 2).copied().unwrap_or(1);

    Ok(Int8Model { tokenizer, bytes, table_start, rows, unk_id, median_token_chars })
}

/// Where the int8 `embeddings` tensor starts, and its shape. The safetensors
/// layout is an 8-byte little-endian header length, a JSON header, then raw
/// data at the offsets the header gives — small enough to read by hand rather
/// than take a crate for.
fn int8_table(bytes: &[u8]) -> Result<(usize, usize, usize), String> {
    let header_len = bytes
        .get(..8)
        .and_then(|b| b.try_into().ok())
        .map(u64::from_le_bytes)
        .ok_or("model.safetensors is too short")? as usize;
    let header: serde_json::Value = bytes
        .get(8..8 + header_len)
        .ok_or("model.safetensors header runs past the file")
        .and_then(|h| serde_json::from_slice(h).map_err(|_| "model.safetensors header is not JSON"))?;
    let tensor = &header["embeddings"];
    if tensor["dtype"] != "I8" {
        return Err(format!("expected an int8 `embeddings` tensor, found {}", tensor["dtype"]));
    }
    let dim = |i: usize| tensor["shape"][i].as_u64().map(|n| n as usize);
    let offset = |i: usize| tensor["data_offsets"][i].as_u64().map(|n| n as usize);
    let (Some(rows), Some(width), Some(begin), Some(end)) = (dim(0), dim(1), offset(0), offset(1)) else {
        return Err("`embeddings` has no shape or offsets".to_string());
    };
    let start = 8 + header_len + begin;
    if end - begin != rows * width || 8 + header_len + end > bytes.len() {
        return Err(format!("`embeddings` is {rows}×{width} but its data does not fit"));
    }
    Ok((start, rows, width))
}

/// The Unigram model's `unk_id`, read from its serialized form: the only
/// place a Unigram tokenizer keeps it.
fn tokenizer_unk_id(tokenizer: &Tokenizer) -> Option<u32> {
    let spec = serde_json::to_value(tokenizer).ok()?;
    spec["model"]["unk_id"].as_u64().map(|id| id as u32)
}

fn read_model_file(path: &Path) -> Result<Vec<u8>, EmbeddingError> {
    if !path.is_file() {
        return Err(EmbeddingError::NotFound(path.to_path_buf()));
    }
    let bytes = fs::read(path).map_err(|e| EmbeddingError::Unreadable {
        path: path.to_path_buf(),
        reason: e.to_string(),
    })?;
    if bytes.starts_with(LFS_POINTER_PREFIX) {
        return Err(EmbeddingError::LfsPointer(path.to_path_buf()));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::temp_dir;

    fn cosine(a: &[f32], b: &[f32]) -> f32 {
        let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
        let norm = |v: &[f32]| v.iter().map(|x| x * x).sum::<f32>().sqrt();
        dot / (norm(a) * norm(b)).max(1e-12)
    }

    /// One loaded model for the whole suite: a gigabyte per test would be the
    /// slowest thing in it.
    fn model() -> &'static LocalEmbeddings {
        static SHARED: std::sync::OnceLock<LocalEmbeddings> = std::sync::OnceLock::new();
        SHARED.get_or_init(|| LocalEmbeddings::new(bundled_model_dir(None), DEFAULT_IDLE_UNLOAD))
    }

    fn embed(provider: &LocalEmbeddings, text: &str) -> Vec<f32> {
        provider.embed(&[text]).unwrap().remove(0).0
    }

    /// Rust and Python must agree on the vectors, or an index built by one is
    /// searched with the other's idea of meaning. `parity.json` was written
    /// by `scripts/build-embedding-model.py` together with the weights.
    ///
    /// Upstream returned early — passing — when the weights were missing or
    /// an LFS pointer. Here that fails: a green run that checked nothing is
    /// how a clone without `git lfs pull` ships an app that cannot index.
    #[test]
    #[ignore = "loads the bundled model; run with: cargo test local_embeddings -- --ignored"]
    fn rust_matches_the_python_parity_fixture() {
        let raw = fs::read_to_string(bundled_model_dir(None).join("parity.json")).unwrap();
        let fixture: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let phrases: Vec<&str> = fixture["phrases"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        let expected: Vec<Vec<f32>> = fixture["vectors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row.as_array().unwrap().iter().map(|n| n.as_f64().unwrap() as f32).collect())
            .collect();

        let got = model().embed(&phrases).unwrap_or_else(|e| panic!("the bundled model must load in tests: {e}"));
        assert_eq!(got.len(), expected.len());
        for ((phrase, got), expected) in phrases.iter().zip(&got).zip(&expected) {
            assert_eq!(got.0.len(), DIMENSIONS);
            let similarity = cosine(&got.0, expected);
            assert!(similarity >= 0.999, "{phrase:?}: cosine {similarity}");
            // Cosine cannot see length, and the fixture's vectors are unit
            // length. A store that scores by dot product would rank by norm
            // if normalisation were lost.
            let norm = got.0.iter().map(|x| x * x).sum::<f32>().sqrt();
            assert!((norm - 1.0).abs() < 1e-3, "{phrase:?}: norm {norm}");
        }
    }

    /// The limit is part of the shipped model, not a property of the script
    /// that made it: a rebuild that forgot the prune step would ship twice the
    /// memory and nobody would notice until a user did.
    #[test]
    #[ignore = "loads the bundled model; run with: cargo test local_embeddings -- --ignored"]
    fn the_shipped_vocabulary_is_russian_and_english_only() {
        let raw = fs::read_to_string(bundled_model_dir(None).join("tokenizer.json")).unwrap();
        let tokenizer: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let vocab: Vec<&str> = tokenizer["model"]["vocab"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|entry| entry[0].as_str())
            .collect();

        // The full vocabulary is 500 353 tokens; the cut one 271 538.
        assert!((200_000..300_000).contains(&vocab.len()), "{} tokens: was the prune step skipped?", vocab.len());
        for foreign in ["世界", "é", "Σ", "ß"] {
            assert!(!vocab.iter().any(|piece| piece.contains(foreign)), "{foreign:?} is still in the vocabulary");
        }
        for kept in ["▁context", "▁контекст", "{"] {
            assert!(vocab.contains(&kept), "{kept:?} was cut");
        }
    }

    /// A character the vocabulary does not have becomes `[UNK]`, and `[UNK]`
    /// carries no meaning — so it is dropped, not averaged in. `model2vec-rs`
    /// averaged it: 17 of 10 970 fragments of two repositories came out
    /// different from their text without the stray character.
    #[test]
    #[ignore = "loads the bundled model; run with: cargo test local_embeddings -- --ignored"]
    fn a_character_outside_the_vocabulary_changes_nothing() {
        let provider = model();
        // No space before them: a space is a token of its own (`▁`), and a
        // real one.
        let plain = embed(provider, "hello world");
        assert_eq!(embed(provider, "hello世界 world"), plain);
    }

    /// What the model is for, in the direction this app needs: a Russian
    /// question lands nearer the English code it is about than English code
    /// it is not about.
    #[test]
    #[ignore = "loads the bundled model; run with: cargo test local_embeddings -- --ignored"]
    fn a_russian_question_is_nearest_the_code_it_is_about() {
        let provider = model();
        let question = embed(&provider, "как сжимается история, когда переполняется контекст");
        let about = embed(&provider, "compact the conversation history when the context window is full");
        let unrelated = embed(&provider, "render the settings dialog with a dropdown for the theme");

        assert!(cosine(&question, &about) > cosine(&question, &unrelated));
    }

    #[test]
    #[ignore = "loads the bundled model; run with: cargo test local_embeddings -- --ignored"]
    fn texts_come_back_in_order_one_vector_each() {
        let provider = model();
        let texts = ["alpha", "keyring fallback", ""];
        let vectors = provider.embed(&texts).unwrap();

        assert_eq!(vectors.len(), 3);
        assert_eq!(vectors[1].0, embed(&provider, "keyring fallback"));
    }

    // ------------------------------------------------------ the lifecycle

    /// Constructing is free: the incremental tick builds one on every save,
    /// and a project without semantic search must never read the weights.
    #[test]
    fn nothing_is_loaded_until_something_is_embedded() {
        let provider = LocalEmbeddings::new(temp_dir("model-lazy"), DEFAULT_IDLE_UNLOAD);
        assert!(!provider.is_loaded());
        // An empty directory: had construction loaded, it would have failed.
    }

    fn wait_until_unloaded<T: Send + Sync + 'static>(cell: &IdleCell<T>) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while cell.is_loaded() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// The decision this module carries: a gigabyte is held while it is being
    /// used, and let go once it is not. The next use loads it again.
    #[test]
    fn an_idle_value_is_dropped_and_comes_back_on_the_next_use() {
        let loads = std::sync::atomic::AtomicUsize::new(0);
        let load = || {
            loads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(42)
        };
        let cell = IdleCell::new(Duration::from_millis(50));

        assert_eq!(*cell.get_or_load(load).unwrap(), 42);
        assert!(cell.is_loaded());
        wait_until_unloaded(&cell);
        assert!(!cell.is_loaded(), "still loaded after the idle period");

        cell.get_or_load(load).unwrap();
        assert_eq!(loads.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert!(cell.is_loaded());

        // ...and the second load is dropped too: the reaper that ended with
        // the first unload must be replaced, or a reload is resident forever.
        wait_until_unloaded(&cell);
        assert!(!cell.is_loaded(), "the second load was never unloaded");
    }

    /// Use resets the clock: a value in steady use is loaded once and never
    /// dropped between two uses. The uses span more than one idle period, and
    /// each gap is a tenth of it: at 30 ms to 120 ms, a CI runner busy with
    /// the rest of the suite slept past the period and loaded it twice.
    #[test]
    fn a_value_in_use_is_loaded_once_and_stays() {
        let loads = std::sync::atomic::AtomicUsize::new(0);
        let cell = IdleCell::new(Duration::from_millis(600));
        for _ in 0..15 {
            cell.get_or_load(|| {
                loads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            })
            .unwrap();
            thread::sleep(Duration::from_millis(60));
        }
        assert_eq!(loads.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// A missing model is an error on every embed — not a remembered one
    /// that outlives `git lfs pull`.
    #[test]
    fn a_failed_load_is_tried_again() {
        let cell: IdleCell<u8> = IdleCell::new(DEFAULT_IDLE_UNLOAD);
        let missing = || Err(EmbeddingError::NotFound(PathBuf::from("tokenizer.json")));
        assert!(cell.get_or_load(missing).is_err());
        assert!(!cell.is_loaded());
        assert_eq!(*cell.get_or_load(|| Ok(7)).unwrap(), 7);
    }

    /// Dropping the cell ends its reaper instead of leaving a thread that
    /// holds the value alive.
    #[test]
    fn a_dropped_cell_takes_its_value_with_it() {
        let cell = IdleCell::new(Duration::from_secs(3600));
        let value = cell.get_or_load(|| Ok(vec![0u8; 16])).unwrap();
        let weak = Arc::downgrade(&value);
        drop(value);
        drop(cell);
        assert!(weak.upgrade().is_none());
    }

    // ------------------------------------------------------- the files

    /// The tests that load the weights are `#[ignore]`d — each run would
    /// otherwise pay seconds and most of a gigabyte for them, which a
    /// mutation run pays once per mutant. This stays in the default run, so
    /// a clone without `git lfs pull` still fails here rather than passing
    /// with a model that cannot load. It reads a few bytes of each file.
    #[test]
    fn the_bundled_files_are_the_real_ones() {
        use std::io::Read;
        for name in ["model.safetensors", "tokenizer.json"] {
            let path = bundled_model_dir(None).join(name);
            let mut head = [0u8; 64];
            let read = fs::File::open(&path)
                .and_then(|mut file| file.read(&mut head))
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            assert!(!head[..read].starts_with(LFS_POINTER_PREFIX), "{name} is an LFS pointer: run `git lfs pull`");
        }
    }

    /// The one-command fix gets its own message.
    #[test]
    fn an_lfs_pointer_is_named_as_one() {
        let dir = temp_dir("model-lfs");
        fs::write(dir.join("tokenizer.json"), "version https://git-lfs.github.com/spec/v1\noid sha256:abc\nsize 1\n").unwrap();

        let Err(error) = load_model(&dir) else { panic!("loaded a model from nothing") };
        assert!(matches!(error, EmbeddingError::LfsPointer(_)), "{error:?}");
        assert!(error.to_string().contains("git lfs pull"));
    }

    #[test]
    fn a_missing_model_says_which_file() {
        let dir = temp_dir("model-missing");
        let Err(error) = load_model(&dir) else { panic!("loaded a model from nothing") };
        assert!(matches!(error, EmbeddingError::NotFound(ref path) if path.ends_with("tokenizer.json")), "{error:?}");
    }

    #[test]
    fn files_that_are_not_a_model_are_refused() {
        let dir = temp_dir("model-garbage");
        for name in ["tokenizer.json", "model.safetensors"] {
            fs::write(dir.join(name), "not a model").unwrap();
        }
        assert!(matches!(load_model(&dir), Err(EmbeddingError::Invalid(_))));
    }

    fn safetensors(dtype: &str, rows: usize, width: usize, bytes_per: usize) -> Vec<u8> {
        let data = rows * width * bytes_per;
        let header = format!(
            r#"{{"embeddings":{{"dtype":"{dtype}","shape":[{rows},{width}],"data_offsets":[0,{data}]}}}}"#
        );
        let mut file = (header.len() as u64).to_le_bytes().to_vec();
        file.extend(header.as_bytes());
        file.resize(file.len() + data, 0);
        file
    }

    /// The real tokenizer with a table that does not belong to it. Each is a
    /// weight file from some other model, and each must be refused at load
    /// rather than embed with the wrong rows.
    #[test]
    #[ignore = "loads the bundled model; run with: cargo test local_embeddings -- --ignored"]
    fn a_table_that_does_not_fit_the_tokenizer_is_refused() {
        let dir = temp_dir("model-mismatch");
        fs::copy(bundled_model_dir(None).join("tokenizer.json"), dir.join("tokenizer.json")).unwrap();
        let vocab = 271_538;
        for (table, why) in [
            (safetensors("I8", 1_000, DIMENSIONS, 1), "rows"),
            (safetensors("F32", vocab, DIMENSIONS, 4), "int8"),
            (safetensors("I8", vocab, 128, 1), "wide"),
        ] {
            fs::write(dir.join("model.safetensors"), table).unwrap();
            let Err(EmbeddingError::Invalid(message)) = load_model(&dir) else {
                panic!("a table refused for {why} was accepted");
            };
            assert!(message.contains(why), "{message}");
        }
    }

    /// Past `MAX_TOKENS` a text is not read: two texts that agree on their
    /// first 512 tokens are the same vector.
    #[test]
    #[ignore = "loads the bundled model; run with: cargo test local_embeddings -- --ignored"]
    fn only_the_first_tokens_count() {
        let provider = model();
        let head = "compact the history ".repeat(200);
        assert_eq!(embed(provider, &format!("{head} and render the dialog")), embed(provider, &format!("{head} keyring fallback file")));
    }

    /// The text is cut before it is tokenized, not after: embedding a
    /// five-megabyte file must cost what embedding its first page does.
    #[test]
    #[ignore = "loads the bundled model; run with: cargo test local_embeddings -- --ignored"]
    fn a_huge_text_is_not_tokenized_whole() {
        let provider = model();
        embed(provider, "warm");
        let huge = "fn handler(request: Request) -> Response { compact(history) }\n".repeat(80_000);
        let started = Instant::now();
        embed(provider, &huge);
        assert!(started.elapsed() < Duration::from_secs(1), "took {:?}", started.elapsed());
    }

    /// A built app finds the weights under its resources; anything else —
    /// dev, tests, a resource directory without them — the crate's copy.
    #[test]
    fn the_model_is_looked_for_under_the_resources_first() {
        let resources = temp_dir("model-resources");
        let bundled = resources.join("models").join(MODEL_DIR_NAME);
        fs::create_dir_all(&bundled).unwrap();
        fs::write(bundled.join("model.safetensors"), "x").unwrap();

        assert_eq!(bundled_model_dir(Some(&resources)), bundled);
        let fallback = Path::new(env!("CARGO_MANIFEST_DIR")).join("models").join(MODEL_DIR_NAME);
        assert_eq!(bundled_model_dir(Some(&temp_dir("model-empty"))), fallback);
        assert_eq!(bundled_model_dir(None), fallback);
    }
}
