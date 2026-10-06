//! Bringing the lexical index in line with the working tree: walk it, work
//! out what changed since the store last looked, and re-chunk exactly that.
//!
//! After this, keyword search works — `IndexStore::search_bm25` over every
//! text file in the repository — with not one vector computed.
//!
//! ## What upstream's `RepositoryIndex` was, and why it is not here
//!
//! Upstream keeps a `DashMap` of every file's metadata and symbols in memory,
//! rebuilds it on every open, and has the store beside it as a cache to
//! rebuild *from*. Two copies of one fact, and the code to keep them in step.
//! Here the store is the only copy: a sync reads what it recorded, compares,
//! and writes back what changed. What the in-memory map served — symbol
//! lookup, Java import resolution — has no caller in this port.
//!
//! ## The order a file is decided in
//!
//! 1. **Same size and mtime as recorded** → unchanged, not even opened. The
//!    one false negative this accepts — same size, same second, different
//!    bytes — needs an edit within the second of the last sync that keeps the
//!    length; a later edit of the file heals it.
//! 2. **Refused by `read_source`** → skipped, *with its reason*, and removed
//!    from the index if it was in it: a file that grew past the limit must
//!    not keep being found by its old text.
//! 3. **Same hash as recorded** → a `touch` or a checkout that restored the
//!    bytes. Only the row's mtime moves; chunks and their vectors stay.
//! 4. Otherwise → parsed, chunked and written, in one transaction.
//!
//! Recorded files the walk did not see are deleted. That covers deletion,
//! a rename (a delete plus a new file), and a newly `.gitignore`d path.

use std::collections::HashSet;
use std::io;
use std::path::{Component, Path};
use std::time::{SystemTime, UNIX_EPOCH};

use thiserror::Error;

use crate::domain::chunk_index::{build_chunks, ChunkBuildOptions};
use crate::domain::repo_index::{detect_language, FileId, FileMetadata};
use crate::infra::index_store::{IndexStore, IndexStoreError};
use crate::infra::language_indexers::indexer_for;
use crate::infra::workspace_scanner::scan_files;
use crate::services::chunk_text::{read_source, SourceSkip};

/// What one sync did — including, one by one, what it would not take.
///
/// `skipped` is the whole point of the struct. A file missing from search
/// with no reason anywhere is indistinguishable from a search that failed,
/// and that is the complaint an index draws most.
#[derive(Debug, Default)]
pub struct SyncReport {
    /// Parsed and re-chunked this run: new files, and files whose bytes changed.
    pub indexed: usize,
    /// Already current — by size and mtime, or by hash after a `touch`.
    pub unchanged: usize,
    /// Dropped from the index: gone from disk, now ignored, or now refused.
    pub removed: usize,
    pub skipped: Vec<SkippedFile>,
}

#[derive(Debug)]
pub struct SkippedFile {
    pub path: String,
    pub reason: SourceSkip,
}

#[derive(Debug, Error)]
pub enum RepoSyncError {
    #[error("could not walk the repository: {0}")]
    Scan(#[source] io::Error),
    #[error(transparent)]
    Store(#[from] IndexStoreError),
}

/// Brings `store` in line with the files under `root`.
///
/// Also carries out the store's cheap rebuild: when
/// [`IndexStore::derived_needs_backfill`] says the names or the full-text
/// rows were produced by an older tokenizer, every file is re-read and those
/// rewritten in place — the chunk rows, and the vectors hanging off them,
/// untouched.
pub fn sync(
    root: &Path,
    store: &IndexStore,
    options: &ChunkBuildOptions,
) -> Result<SyncReport, RepoSyncError> {
    let root = crate::domain::tools::canonicalize_plain(root).map_err(RepoSyncError::Scan)?;
    let scanned = scan_files(&root, None).map_err(RepoSyncError::Scan)?;
    let known = store.load_all_files()?;
    let backfill = store.derived_needs_backfill()?;

    let mut report = SyncReport::default();
    let mut present: HashSet<FileId> = HashSet::with_capacity(scanned.len());

    for file in scanned {
        let Some(relative_path) = relative_path(&root, &file.path) else { continue };
        let file_id = FileId(relative_path.clone());
        let prior = known.get(&file_id);

        if !backfill
            && prior.is_some_and(|p| p.size_bytes == file.size && same_second(p.modified_at, file.modified))
        {
            present.insert(file_id);
            report.unchanged += 1;
            continue;
        }

        let content = match read_source(&file.path, options.max_file_bytes) {
            Ok(content) => content,
            Err(reason) => {
                report.skipped.push(SkippedFile { path: relative_path, reason });
                continue;
            }
        };

        let language = detect_language(&relative_path);
        let metadata = FileMetadata {
            relative_path,
            size_bytes: content.len() as u64,
            modified_at: file.modified,
            hash: blake3::hash(content.as_bytes()),
            language,
        };
        // Parsed only on the paths that write what the parse produces: a
        // checkout that touched a thousand files and changed ten pays for ten.
        let chunk = || {
            let symbols = indexer_for(language).index(&content);
            let chunks = build_chunks(&file_id, language, &content, &symbols, options);
            (symbols, chunks)
        };

        if prior.is_some_and(|p| p.hash == metadata.hash && p.language == language) {
            if backfill {
                store.replace_derived_for_file(&file_id, &chunk().1)?;
            }
            store.upsert_files(std::slice::from_ref(&metadata))?;
            report.unchanged += 1;
        } else {
            let (symbols, chunks) = chunk();
            store.replace_file(&metadata, &symbols, &chunks)?;
            report.indexed += 1;
        }
        present.insert(file_id);
    }

    let gone: Vec<FileId> = known.into_keys().filter(|id| !present.contains(id)).collect();
    report.removed = gone.len();
    store.delete_files(&gone)?;

    if backfill {
        store.mark_derived_current()?;
    }
    Ok(report)
}

/// The store keeps whole seconds; a live `stat` has nanoseconds. Compared
/// exactly, no file read back from the store would ever match itself.
fn same_second(a: SystemTime, b: SystemTime) -> bool {
    let secs = |t: SystemTime| t.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    secs(a) == secs(b)
}

/// `/`-separated and relative to `root`, on every platform: the id a file is
/// stored under must not change with the machine that wrote it.
fn relative_path(root: &Path, path: &Path) -> Option<String> {
    let parts: Option<Vec<String>> = path
        .strip_prefix(root)
        .ok()?
        .components()
        .map(|c| match c {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    parts.filter(|p| !p.is_empty()).map(|p| p.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::temp_dir;
    use std::fs;
    use std::path::PathBuf;
    use std::time::Duration;

    struct Fixture {
        root: PathBuf,
        store: IndexStore,
    }

    impl Fixture {
        fn new(label: &str) -> Self {
            let root = temp_dir(&format!("{label}-repo"));
            let store = IndexStore::open(&temp_dir(&format!("{label}-store"))).unwrap();
            Fixture { root, store }
        }

        fn write(&self, path: &str, content: &str) {
            let full = self.root.join(path);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(full, content).unwrap();
        }

        fn sync(&self) -> SyncReport {
            sync(&self.root, &self.store, &ChunkBuildOptions::default()).unwrap()
        }

        fn found(&self, query: &str) -> Vec<String> {
            let mut files: Vec<String> = self
                .store
                .search_bm25(query, 50)
                .unwrap()
                .into_iter()
                .map(|(id, _)| id.0.split('#').next().unwrap().to_string())
                .collect();
            files.sort();
            files.dedup();
            files
        }

        /// Makes a file unopenable while leaving its size and mtime alone —
        /// so a sync that still calls it unchanged provably never opened it.
        #[cfg(unix)]
        fn seal(&self, path: &str) {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(self.root.join(path), fs::Permissions::from_mode(0o000)).unwrap();
        }

        /// Moves a file's mtime into the past, so the next write is a change
        /// of second whatever the clock did in between.
        fn age(&self, path: &str) {
            let past = SystemTime::now() - Duration::from_secs(3600);
            fs::File::options()
                .write(true)
                .open(self.root.join(path))
                .unwrap()
                .set_modified(past)
                .unwrap();
        }
    }

    // --------------------------------------------------------- the first sync

    /// The claim of this feature, end to end: a repository walked, and
    /// keyword search answering over code, prose and plain text alike — no
    /// vector anywhere.
    #[test]
    fn after_a_sync_every_text_file_is_searchable() {
        let f = Fixture::new("sync-first");
        f.write("src/turn.rs", "pub fn compact_history() {}\n");
        f.write("docs/guide.md", "# Compaction\n\nHistory is folded.\n");
        f.write("styles/tokens.css", ":root { --accent: teal; }\n");

        let report = f.sync();

        assert_eq!((report.indexed, report.unchanged, report.removed), (3, 0, 0));
        assert_eq!(f.found("compact_history"), ["src/turn.rs"]);
        assert_eq!(f.found("folded"), ["docs/guide.md"]);
        assert_eq!(f.found("accent"), ["styles/tokens.css"], "plain text is indexed too (B-8)");
    }

    #[test]
    fn a_declaration_is_found_by_its_qualified_name() {
        let f = Fixture::new("sync-names");
        f.write("src/store.rs", "struct Store;\nimpl Store {\n    fn open() {}\n}\n");
        f.sync();

        assert_eq!(f.found("qualified_name:open"), ["src/store.rs"]);
    }

    /// Ids are the same on every platform — `/`, relative, no `./`.
    #[test]
    fn files_are_stored_under_slash_separated_relative_ids() {
        let f = Fixture::new("sync-ids");
        f.write("src/deep/nested/mod.rs", "fn a() {}\n");
        f.sync();

        let ids: Vec<String> = f.store.load_all_files().unwrap().into_keys().map(|id| id.0).collect();
        assert_eq!(ids, ["src/deep/nested/mod.rs"]);
    }

    // -------------------------------------------------------------- skipping

    /// Every refused file is in the report with why. The list a counter in
    /// the UI will be built from.
    #[test]
    fn a_refused_file_is_reported_with_its_reason() {
        let f = Fixture::new("sync-skip");
        f.write("ok.rs", "fn a() {}\n");
        fs::write(f.root.join("logo.png"), b"\x89PNG\0\0").unwrap();
        f.write("bundle.js", &"x".repeat(2_000));

        let options = ChunkBuildOptions { max_file_bytes: 1_000, ..Default::default() };
        let report = sync(&f.root, &f.store, &options).unwrap();

        let mut skipped: Vec<(String, String)> =
            report.skipped.iter().map(|s| (s.path.clone(), s.reason.to_string())).collect();
        skipped.sort();
        assert_eq!(
            skipped,
            [
                ("bundle.js".to_string(), "too large: 2000 bytes, the limit is 1000".to_string()),
                ("logo.png".to_string(), "binary".to_string()),
            ]
        );
        assert_eq!(report.indexed, 1);
    }

    /// A file that grew past the limit must stop being found by its old text.
    #[test]
    fn a_file_that_becomes_refused_leaves_the_index() {
        let f = Fixture::new("sync-grew");
        f.write("gen.js", "const marker_word = 1;\n");
        f.sync();
        assert_eq!(f.found("marker_word"), ["gen.js"]);

        f.write("gen.js", &format!("const marker_word = 1;\n{}", "x".repeat(2_000)));
        let options = ChunkBuildOptions { max_file_bytes: 1_000, ..Default::default() };
        let report = sync(&f.root, &f.store, &options).unwrap();

        assert_eq!(report.removed, 1);
        assert_eq!(report.skipped.len(), 1);
        assert!(f.found("marker_word").is_empty());
    }

    // ----------------------------------------------------------- incremental

    #[test]
    fn a_second_sync_of_an_untouched_tree_changes_nothing() {
        let f = Fixture::new("sync-noop");
        f.write("a.rs", "fn a() {}\n");
        f.write("b.md", "# B\n");
        f.sync();

        let report = f.sync();
        assert_eq!((report.indexed, report.unchanged, report.removed), (0, 2, 0));
    }

    /// "Unchanged" has two routes and the counter cannot tell them apart; a
    /// file nobody may open can. Through the size-and-mtime shortcut it is
    /// never opened and stays. Through a read it would be refused and
    /// dropped — which is what every file in the repository would pay for on
    /// every sync if the shortcut broke, including a stored mtime compared at
    /// a precision the store does not keep.
    #[cfg(unix)]
    #[test]
    fn an_unchanged_file_is_not_even_opened() {
        let f = Fixture::new("sync-unopened");
        f.write("a.rs", "fn a() {}\n");
        f.sync();

        f.seal("a.rs");
        let report = f.sync();

        assert!(report.skipped.is_empty(), "{:?}", report.skipped);
        assert_eq!((report.unchanged, report.removed), (1, 0));
    }

    /// The mtime shortcut must not be the only way to "unchanged": a file
    /// whose mtime moved and whose bytes did not keeps its chunks — and the
    /// vectors they will carry.
    #[test]
    fn a_touched_file_is_not_rechunked() {
        let f = Fixture::new("sync-touch");
        f.write("a.rs", "fn a() {}\n");
        f.age("a.rs");
        f.sync();
        let chunks_before = f.store.load_all_chunks().unwrap();

        f.write("a.rs", "fn a() {}\n");
        let report = f.sync();

        assert_eq!((report.indexed, report.unchanged), (0, 1));
        assert_eq!(f.store.load_all_chunks().unwrap(), chunks_before);
    }

    /// A touch must move the recorded mtime, or the file is re-read and
    /// re-hashed on every sync from then on.
    #[cfg(unix)]
    #[test]
    fn after_a_touch_the_shortcut_applies_again() {
        let f = Fixture::new("sync-touch-row");
        f.write("a.rs", "fn a() {}\n");
        f.age("a.rs");
        f.sync();
        f.write("a.rs", "fn a() {}\n");
        f.sync();

        f.seal("a.rs");
        let report = f.sync();
        assert!(report.skipped.is_empty(), "the touched file was opened again: {:?}", report.skipped);
    }

    #[test]
    fn an_edited_file_is_rechunked_and_its_old_text_is_gone() {
        let f = Fixture::new("sync-edit");
        f.write("a.rs", "fn before_edit() {}\n");
        f.age("a.rs");
        f.sync();

        f.write("a.rs", "fn after_edit() {}\n");
        let report = f.sync();

        assert_eq!(report.indexed, 1);
        assert!(f.found("before_edit").is_empty(), "the full-text row outlived its chunk");
        assert_eq!(f.found("after_edit"), ["a.rs"]);
    }

    #[test]
    fn a_deleted_file_leaves_the_index() {
        let f = Fixture::new("sync-delete");
        f.write("keep.rs", "fn keep() {}\n");
        f.write("drop.rs", "fn dropped_one() {}\n");
        f.sync();

        fs::remove_file(f.root.join("drop.rs")).unwrap();
        let report = f.sync();

        assert_eq!(report.removed, 1);
        assert!(f.found("dropped_one").is_empty());
        assert_eq!(f.store.load_all_files().unwrap().len(), 1);
    }

    /// Ignoring a directory after the fact is how `target/` or a vendored
    /// tree gets out of the index; the walk no longer sees it, so it goes.
    #[test]
    fn a_newly_ignored_file_leaves_the_index() {
        let f = Fixture::new("sync-ignore");
        f.write("vendor/lib.js", "function vendored_fn() {}\n");
        f.sync();
        assert_eq!(f.found("vendored_fn"), ["vendor/lib.js"]);

        f.write(".gitignore", "vendor/\n");
        fs::create_dir_all(f.root.join(".git")).unwrap();
        f.sync();

        assert!(f.found("vendored_fn").is_empty());
    }

    // --------------------------------------------------------- the backfill

    /// The cheap rebuild the store was built for: names and full-text rows
    /// rewritten, chunk rows — and the vectors keyed on them — kept.
    #[test]
    fn a_stale_derived_version_is_backfilled_without_losing_vectors() {
        let f = Fixture::new("sync-backfill");
        f.write("a.rs", "fn backfilled() {}\n");
        f.sync();
        let chunk = f.store.load_all_chunks().unwrap().remove(0);
        f.store
            .upsert_embeddings("model", &[(chunk.id.clone(), chunk.hash, crate::domain::embeddings::QuantizedVector::quantize(&[1.0, 0.0]))])
            .unwrap();
        // What an older tokenizer left behind: the same chunk, other derived
        // text. Only a real rewrite gets rid of it.
        let outdated = crate::domain::chunk_index::Chunk {
            metadata: crate::domain::chunk_index::ChunkMetadata {
                qualified_name: Some("stale_name".into()),
                ..chunk.clone()
            },
            text: "outdated".into(),
        };
        f.store.replace_derived_for_file(&chunk.file_id, &[outdated]).unwrap();
        assert_eq!(f.found("outdated"), ["a.rs"]);

        f.store.write_meta("derived_version", "0").unwrap();
        assert!(f.store.derived_needs_backfill().unwrap());
        let report = f.sync();

        assert_eq!(report.indexed, 0, "the cheap rebuild re-chunked from scratch");
        assert!(!f.store.derived_needs_backfill().unwrap());
        assert_eq!(f.store.load_all_embeddings("model", 2).unwrap().len(), 1);
        assert_eq!(f.found("backfilled"), ["a.rs"]);
        assert!(f.found("outdated").is_empty(), "the derived rows were not rewritten");
        assert!(f.found("stale_name").is_empty());
    }
}
