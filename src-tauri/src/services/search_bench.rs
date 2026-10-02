//! The quality bench for code search (F-5.13c): questions with known answers,
//! asked of real repositories, under each embedding model.
//!
//! Ignored by default — it indexes whole repositories and loads models:
//!
//! ```text
//! cargo test --release search_bench -- --ignored --nocapture
//! ```
//!
//! `bench/search-queries.json` holds the questions; a repository missing from
//! disk is skipped. `SEARCH_BENCH_MODELS` adds models beside the bundled one,
//! as `name=dir,name=dir` — each an int8 Model2Vec directory of the bundled
//! width. Stores are kept under the temp dir between runs, so only the first
//! run of a model pays for embedding.
//!
//! What it measures is `code_search::search` itself, the ranking the tool
//! ships. The grid that chose its constants was a variant of the pipeline
//! beside it; its results are in `docs/06-port-plan.md` (F-5.13c, F-5.13c-2).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::domain::chunk_index::ChunkBuildOptions;
use crate::domain::code_search::SearchFilter;
use crate::domain::embeddings::{Embedding, EmbeddingError, EmbeddingProvider};
use crate::domain::workspace_index::IndexEventSink;
use crate::infra::local_embeddings::{DEFAULT_IDLE_UNLOAD, DIMENSIONS, LocalEmbeddings, bundled_model_dir};
use crate::services::code_search;
use crate::services::index_sync::RepoIndexer;

#[derive(Deserialize)]
struct Bench {
    repos: Vec<Repo>,
}

#[derive(Deserialize)]
struct Repo {
    name: String,
    root: String,
    queries: Vec<Query>,
}

#[derive(Deserialize)]
struct Query {
    lang: String,
    /// `"docs"` when the answer is documentation, `"caller"` when it is the
    /// use of a name the question gives rather than its declaration, `"none"`
    /// when the repository has no answer and the result should say it is weak.
    #[serde(default)]
    answer: String,
    q: String,
    expect: Vec<Expect>,
    /// How the agent itself put the question to `semanticSearch`, when it did.
    agent: Option<AgentAsk>,
}

/// One `semanticSearch` call as the agent made it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentAsk {
    query: String,
    #[serde(default)]
    queries: Vec<String>,
    fts: Option<Vec<String>>,
    #[serde(default)]
    include_docs: bool,
}

#[derive(Deserialize)]
struct Expect {
    path: String,
    name: Option<String>,
}

impl Expect {
    fn file(&self, path: &str) -> bool {
        path == self.path
    }
    fn declaration(&self, path: &str, name: Option<&str>) -> bool {
        self.file(path) && self.name.as_deref().is_none_or(|want| name.is_some_and(|n| n.contains(want)))
    }
}

/// "Words only": a model that never loads, as when the weights are missing.
struct NoModel;

impl EmbeddingProvider for NoModel {
    fn embed(&self, _: &[&str]) -> Result<Vec<Embedding>, EmbeddingError> {
        Err(EmbeddingError::Invalid("no model in this configuration".into()))
    }
    fn dimensions(&self) -> usize {
        DIMENSIONS
    }
}

/// The bench's own files quote every question; left in, they would be the
/// first word match of each one asked about this repository.
fn is_bench(path: &str) -> bool {
    path.starts_with("src-tauri/bench/") || path.ends_with("search_bench.rs")
}

/// 1-based rank of the first hit, if any.
type Rank = Option<usize>;

#[derive(Default)]
struct Tally {
    n: usize,
    at1: usize,
    at5: usize,
    at10: usize,
    rr: f64,
    decl5: usize,
}

impl Tally {
    fn add(&mut self, file: Rank, decl: Rank) {
        self.n += 1;
        self.at1 += usize::from(file == Some(1));
        self.at5 += usize::from(file.is_some_and(|r| r <= 5));
        self.at10 += usize::from(file.is_some_and(|r| r <= 10));
        self.rr += file.map_or(0.0, |r| 1.0 / r as f64);
        self.decl5 += usize::from(decl.is_some_and(|r| r <= 5));
    }
    fn row(&self, label: &str) -> String {
        format!(
            "{label:<28} n={:<3} file@1 {:>2}  @5 {:>2}  @10 {:>2}  MRR {:.3}  decl@5 {:>2}",
            self.n,
            self.at1,
            self.at5,
            self.at10,
            self.rr / self.n.max(1) as f64,
            self.decl5
        )
    }
}

fn models() -> Vec<(String, Arc<dyn EmbeddingProvider>)> {
    let mut models: Vec<(String, Arc<dyn EmbeddingProvider>)> = vec![
        ("bm25".into(), Arc::new(NoModel)),
        ("ru-en".into(), Arc::new(LocalEmbeddings::new(bundled_model_dir(None), DEFAULT_IDLE_UNLOAD))),
    ];
    for pair in std::env::var("SEARCH_BENCH_MODELS").unwrap_or_default().split(',').filter(|p| !p.is_empty()) {
        let (name, dir) = pair.split_once('=').expect("SEARCH_BENCH_MODELS is name=dir,name=dir");
        models.push((name.into(), Arc::new(LocalEmbeddings::new(PathBuf::from(dir), DEFAULT_IDLE_UNLOAD))));
    }
    models
}

fn open(repo: &Repo, root: &Path, model: &str, provider: Arc<dyn EmbeddingProvider>) -> (RepoIndexer, Duration) {
    let store = std::env::temp_dir().join("kibo-search-bench").join(format!("{}-{model}", repo.name));
    let indexer = RepoIndexer::open(root, &store, provider, ChunkBuildOptions::default()).unwrap();
    let sink: IndexEventSink = Arc::new(|_| {});
    let started = Instant::now();
    indexer.sync(&sink).unwrap();
    (indexer, started.elapsed())
}

/// Where the first answer is in the tool's own result, and in the semantic
/// ranking alone (with the cosine of its first hit and of its top result),
/// how long the tool's search took, and whether it called its result weak.
fn ask(indexer: &RepoIndexer, query: &Query) -> (Rank, Rank, Option<(Rank, f32, Option<f32>)>, Duration, bool) {
    // Documentation is searched when the answer is documentation — what a
    // model asking that question should set `includeDocs` for.
    ask_as(indexer, query, &[&query.q], None, query.answer == "docs")
}

/// [`ask`] with the question put as `wordings` — several in one call, as a
/// model sending `queries` would — and the call's own `fts` and `includeDocs`.
fn ask_as(
    indexer: &RepoIndexer,
    query: &Query,
    wordings: &[&str],
    fts: Option<&[String]>,
    include_docs: bool,
) -> (Rank, Rank, Option<(Rank, f32, Option<f32>)>, Duration, bool) {
    // Timed as the tool calls it; ranked from a longer list, so that the
    // bench's own files, dropped below, do not take a place.
    let started = Instant::now();
    let filter = SearchFilter { include_docs, ..SearchFilter::default() };
    code_search::search_many(indexer, wordings, fts, code_search::DEFAULT_TOP_K, &filter).unwrap();
    let spent = started.elapsed();
    let mut result = code_search::search_many(indexer, wordings, fts, 20, &filter).unwrap();
    let weak = result.meta.weak;
    result.matches.retain(|m| !is_bench(&m.path));
    result.matches.truncate(10);
    if std::env::var_os("SEARCH_BENCH_VERBOSE").is_some() {
        for m in result.matches.iter().take(6) {
            println!("      {:?} {}:{} {}", m.source, m.path, m.start_line, m.name.as_deref().unwrap_or(""));
        }
    }
    let file = result.matches.iter().position(|m| query.expect.iter().any(|e| e.file(&m.path))).map(|i| i + 1);
    let decl = result
        .matches
        .iter()
        .position(|m| query.expect.iter().any(|e| e.declaration(&m.path, m.name.as_deref())))
        .map(|i| i + 1);

    let meaning = indexer.search_meaning(&query.q, 20).ok().filter(|hits| !hits.is_empty()).map(|hits| {
        let top = hits[0].1;
        let first = hits.iter().enumerate().find_map(|(i, (id, score))| {
            let chunk = indexer.store().load_chunk(id).unwrap().filter(|c| !is_bench(&c.file_id.0))?;
            query.expect.iter().any(|e| e.file(&chunk.file_id.0)).then_some((i + 1, *score))
        });
        (first.map(|(r, _)| r), top, first.map(|(_, s)| s))
    });
    (file, decl, meaning, spent, weak)
}

/// A question's answer as a key: its acceptable files, in order.
fn answer_of(query: &Query) -> Vec<&str> {
    let mut paths: Vec<&str> = query.expect.iter().map(|e| e.path.as_str()).collect();
    paths.sort_unstable();
    paths
}

#[test]
#[ignore = "indexes real repositories; run with --ignored --nocapture"]
fn search_bench() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let bench: Bench =
        serde_json::from_str(&std::fs::read_to_string(manifest.join("bench/search-queries.json")).unwrap()).unwrap();

    for (model, provider) in models() {
        println!("\n=== {model}");
        let mut all = Tally::default();
        let mut by_lang: std::collections::BTreeMap<String, Tally> = Default::default();
        // Questions without an answer that were called weak, of how many;
        // and questions with one that were, wrongly.
        let (mut none_weak, mut none_total, mut false_weak) = (0, 0, 0);
        for repo in &bench.repos {
            let root = manifest.join(&repo.root);
            let Ok(root) = root.canonicalize() else {
                println!("{}: not on disk, skipped", repo.name);
                continue;
            };
            let (indexer, synced) = open(repo, &root, &model, Arc::clone(&provider));
            let status = indexer.status();
            println!(
                "{}: sync {:.1}s, {} embedded{}",
                repo.name,
                synced.as_secs_f32(),
                status.embedded,
                status.embedding_error.map(|e| format!(" ({e})")).unwrap_or_default()
            );
            let mut tally = Tally::default();
            let mut spent = Duration::ZERO;
            // How often history said anything, and how often it named the
            // answer's file — what `HISTORY_*` in `index_sync` are tuned by.
            let (mut history_fired, mut history_right) = (0, 0);
            // Answers asked both in Russian and in English: the gap between the
            // two is how much a Russian question loses to the language alone —
            // the ceiling of a model rewriting its queries into English.
            let paired: std::collections::HashSet<Vec<&str>> = repo
                .queries
                .iter()
                .filter(|q| q.lang == "ru")
                .map(answer_of)
                .filter(|answer| !answer.is_empty())
                .filter(|answer| repo.queries.iter().any(|q| q.lang == "en" && answer_of(q) == *answer))
                .collect();
            for query in &repo.queries {
                let near = indexer.files_by_history(&query.q);
                if !near.is_empty() {
                    history_fired += 1;
                    history_right += usize::from(query.expect.iter().any(|e| near.iter().any(|path| e.file(path))));
                }
                let (file, decl, meaning, took, weak) = ask(&indexer, query);
                spent += took;
                if query.answer == "none" {
                    none_total += 1;
                    none_weak += usize::from(weak);
                    println!("  none    weak {weak:<5} [{}] {}", query.lang, query.q);
                    continue;
                }
                false_weak += usize::from(weak);
                tally.add(file, decl);
                all.add(file, decl);
                by_lang.entry(query.lang.clone()).or_default().add(file, decl);
                if !query.answer.is_empty() {
                    by_lang.entry(format!("answer: {}", query.answer)).or_default().add(file, decl);
                }
                if paired.contains(&answer_of(query)) {
                    by_lang.entry(format!("pairs: {}", query.lang)).or_default().add(file, decl);
                    // The Russian question with its English counterpart in one
                    // call: what `queries` buys over the Russian alone.
                    if query.lang == "ru" {
                        let english = repo.queries.iter().find(|q| q.lang == "en" && answer_of(q) == answer_of(query));
                        if let Some(english) = english {
                            let (file, decl, ..) =
                                ask_as(&indexer, query, &[&query.q, &english.q], None, query.answer == "docs");
                            by_lang.entry("pairs: ru+en in one call".to_string()).or_default().add(file, decl);
                        }
                    }
                }
                // Without `includeDocs`, as a model that does not set it asks.
                let (bare, bare_decl, ..) = ask_as(&indexer, query, &[&query.q], None, false);
                let kind = if query.answer == "docs" { "docs" } else { "code" };
                by_lang.entry("no includeDocs: all".to_string()).or_default().add(bare, bare_decl);
                by_lang.entry(format!("no includeDocs: {kind}")).or_default().add(bare, bare_decl);
                // As the agent asked it: its wordings, and its `includeDocs`
                // rather than the one the answer calls for.
                if let Some(agent) = &query.agent {
                    let wordings: Vec<&str> =
                        std::iter::once(agent.query.as_str()).chain(agent.queries.iter().map(String::as_str)).collect();
                    let (asked, asked_decl, ..) =
                        ask_as(&indexer, query, &wordings, agent.fts.as_deref(), agent.include_docs);
                    let kind = if query.answer.is_empty() { "code" } else { query.answer.as_str() };
                    by_lang.entry(format!("agent: {} question alone", query.lang)).or_default().add(file, decl);
                    by_lang.entry(format!("agent: {} as asked", query.lang)).or_default().add(asked, asked_decl);
                    by_lang.entry(format!("agent: {} as asked, {kind}", query.lang)).or_default().add(asked, asked_decl);
                }
                let show = |r: Rank| r.map_or("-".to_string(), |r| r.to_string());
                let meaning = meaning.map_or(String::new(), |(rank, top, hit)| {
                    format!(
                        "  sem#{} cos {} top {top:.2}",
                        show(rank),
                        hit.map_or("-".to_string(), |s| format!("{s:.2}"))
                    )
                });
                let weak = if weak { " weak" } else { "" };
                println!("  file#{:<2} decl#{:<2}{meaning:<26}{weak} [{}] {}", show(file), show(decl), query.lang, query.q);
            }
            println!("{}", tally.row(&repo.name));
            println!(
                "  history: {history_fired} of {} questions, the answer's file in {history_right}",
                repo.queries.len()
            );
            println!("  {:.1} ms a query", spent.as_secs_f64() * 1000.0 / repo.queries.len().max(1) as f64);
        }
        for (lang, tally) in &by_lang {
            println!("{}", tally.row(lang));
        }
        println!("{}", all.row(&format!("ALL {model}")));
        println!("weak: {none_weak} of {none_total} without an answer, {false_weak} of {} with one", all.n);
    }
}



