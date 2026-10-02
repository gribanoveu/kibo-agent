//! Search of the open folder's code: names first, then words and meaning
//! fused into one ranking.
//!
//! A declaration named as the query spells it comes first — nothing a ranker
//! says outweighs a name the caller got right. Words (BM25) and meaning
//! (embeddings) are peers: `fuse_rrf` merges their rankings rather than
//! picking one, because "these words appear here" and "this passage means
//! that" are different evidence, and a chunk carrying both beats one carrying
//! either.
//!
//! The constants below were set by the quality bench (F-5.13c,
//! `services/search_bench.rs`, 67 questions over three repositories), not by
//! taste; `docs/06-port-plan.md` has the numbers. Upstream's stem tier is not
//! ported — the full-text index matches prefixes and weighs chunk names four
//! to one. Its path tier came back as [`PATH_BOOST`].

use std::collections::{HashMap, HashSet};

use crate::domain::chunk_index::{ChunkId, ChunkMetadata};
use crate::domain::code_search::{CodeMatch, CodeSearchResult, MatchSource, SearchFilter, SearchMeta};
use crate::domain::repo_index::Language;
use crate::domain::search_query::{
    SearchMetaInput, absent_from_workspace, extract_search_tokens, fts5_query, is_documentation,
    looks_like_identifier, path_segment_matches, query_words, weak_search_hint,
};
use crate::infra::index_store::IndexStoreError;
use crate::services::chunk_text::resolve_chunk;
use crate::services::index_sync::RepoIndexer;

pub const DEFAULT_TOP_K: usize = 10;
pub const MAX_TOP_K: usize = 50;
/// Wordings one search takes: the question and up to three rewordings.
pub const MAX_QUERIES: usize = 4;

/// How many candidates each ranking contributes per match wanted. A chunk
/// whose file changed since it was indexed is dropped when its text is read;
/// without slack, ten asked for could come back as seven.
const CANDIDATES_PER_MATCH: usize = 2;

/// Reciprocal-rank fusion's smoothing constant. Small: being first by one
/// measure outweighs being sixth by both. The published 60 did the opposite —
/// a long Russian design document, middling on words *and* on meaning,
/// outranked the function that was first by meaning, and MRR fell below words
/// alone (0.437 against 0.494).
const RRF_K: f32 = 3.0;

/// How much more a rank by words counts than the same rank by meaning. A
/// static model is a blunt instrument next to an exact word; it earns its
/// place on questions whose words the code does not use.
const LEXICAL_WEIGHT: f32 = 1.5;

/// The weight of the model's own `fts` words, ranked beside the query's words
/// rather than instead of them. Bench, 2026-10-02 (26 Russian questions as
/// deepseek-flash put them, 21 with `fts`), code answers in the top five: 17
/// of 22 with `fts` replacing the query's words, 20 beside them at 1.5, 21 at
/// 0.5, 22 at 0.25 and with `fts` ignored. Kept above nothing: `fts` is the
/// one way to ask for an exact error message or key.
const FTS_WEIGHT: f32 = 0.5;

/// Where documentation that outranks all the code goes when `includeDocs` is
/// not set: one passage, fifth, so the code above it keeps its places. Bench,
/// 2026-10-02, the 93 questions without `includeDocs`: answers in the docs in
/// the top five 1 → 7 of 15, code answers unchanged (31 first, 64 in the top
/// five of 78). First instead cost the code 4 of its first places; second or
/// third kept them but lowered its MRR (0.549, 0.553 against 0.557).
const UNASKED_DOC_SLOT: usize = 4;

/// Prose — Markdown, AsciiDoc, plain text — is long, and long text shares a
/// few words and a general drift with most questions. Scaled down, not out:
/// harder than this and questions answered by documentation lose.
const PROSE_FACTOR: f32 = 0.75;

/// A query word in a file's path (`controller` → `DocumentController.java`)
/// says what the file is about better than any one passage of it.
const PATH_BOOST: f32 = 1.5;

/// A file changed by a commit whose message is near the query in meaning.
/// Small on purpose: history is a hint about where to look, it says nothing
/// about which passage answers — and in a repository with a few vague
/// commits it says nothing at all, so it must not outvote the text.
///
/// Bench, 2026-09-21 (with `includeDocs` already in): MRR 0.564 at 1.0 and at
/// 1.2, 0.560 at 1.5. History fired on 15 of 39 questions here and named the
/// answer's file in 8; on docflow 10 of 20 and 3; on a 9-commit service 1 and
/// 0. Neutral at this weight — kept for repositories whose commits describe
/// the work; raise it only on a bench that shows a gain.
const HISTORY_BOOST: f32 = 1.2;

/// How many passages of one file a result lists before other files get their
/// turn. Five chunks of one long file can otherwise push the second file that
/// matters out of the top ten; the rest of them come back if there is room.
const MAX_PER_FILE: usize = 2;

/// With something filtered out — documentation, or paths — how many more
/// candidates each ranking brings in. In a repository whose docs outnumber
/// its code the plain pool can be mostly prose, and dropping it would leave
/// fewer matches than asked for.
const FILTERED_SLACK: usize = 2;

/// `fts`, when given, is ranked by words beside `query`'s own words, at
/// [`FTS_WEIGHT`]: a model that asked for more precision must not lose the
/// ranking for a wrong guess.
///
/// `filter.include_docs` false leaves documentation out (`is_documentation`),
/// all but one passage that outranks all the code, which comes fifth
/// ([`UNASKED_DOC_SLOT`]); the hint then says how many matches were left
/// out, so the model can ask again.
/// `filter.paths` keeps only the files it allows.
pub fn search(
    indexer: &RepoIndexer,
    query: &str,
    fts: Option<&[String]>,
    top_k: usize,
    filter: &SearchFilter,
) -> Result<CodeSearchResult, IndexStoreError> {
    search_many(indexer, &[query], fts, top_k, filter)
}

/// [`search`] for several wordings of one question at once — a Russian
/// question and its English rewording, say. Each wording is ranked by meaning
/// and by words, and every ranking goes into one fusion: a passage only one
/// wording finds still gets in, one both find rises. Names from any wording
/// are looked up. `fts` stands in for the first wording's words, and git
/// history is asked with the first wording only. Past [`MAX_QUERIES`] the
/// rest are dropped.
pub fn search_many(
    indexer: &RepoIndexer,
    queries: &[&str],
    fts: Option<&[String]>,
    top_k: usize,
    filter: &SearchFilter,
) -> Result<CodeSearchResult, IndexStoreError> {
    let queries: Vec<&str> = queries.iter().copied().filter(|q| !q.trim().is_empty()).take(MAX_QUERIES).collect();
    let first = queries.first().copied().unwrap_or_default();
    let all = queries.join(" ");
    let include_docs = filter.include_docs;
    let top_k = top_k.clamp(1, MAX_TOP_K);
    let store = indexer.store();
    let tokens = extract_search_tokens(&all);
    let mut ranked: Vec<(ChunkMetadata, MatchSource)> = Vec::new();

    let mut names: Vec<String> = Vec::new();
    for query in &queries {
        for name in names_in(query, &extract_search_tokens(query)) {
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    for name in names {
        for id in store.chunks_declaring(&name, top_k)? {
            ranked.extend(store.load_chunk(&id)?.map(|chunk| (chunk, MatchSource::Symbol)));
        }
    }

    let filtered = !include_docs || !filter.paths.is_empty();
    let candidates = top_k * CANDIDATES_PER_MATCH * if filtered { FILTERED_SLACK } else { 1 };
    let mut tiers_used = vec![MatchSource::Symbol, MatchSource::Lexical];
    let mut unavailable = None;
    // Semantic first within each wording: a chunk both found is labelled by
    // the more telling one.
    let mut lists = Vec::new();
    for (i, query) in queries.iter().enumerate() {
        let semantic: Vec<ChunkId> = match indexer.search_meaning(query, candidates) {
            Ok(hits) => hits.into_iter().map(|(id, _)| id).collect(),
            Err(error) => {
                unavailable = Some(error.to_string());
                Vec::new()
            }
        };
        let own = if i == 0 { fts.and_then(|terms| fts5_query(&terms.join(" "))) } else { None };
        lists.push((semantic, MatchSource::Semantic, 1.0));
        for (words, weight) in [(own, FTS_WEIGHT), (fts5_query(query), LEXICAL_WEIGHT)] {
            let Some(words) = words else { continue };
            let lexical: Vec<ChunkId> = store.search_bm25(&words, candidates)?.into_iter().map(|(id, _)| id).collect();
            lists.push((lexical, MatchSource::Lexical, weight));
        }
    }
    if lists.iter().any(|(list, source, _)| *source == MatchSource::Semantic && !list.is_empty()) {
        tiers_used.push(MatchSource::Semantic);
    } else if unavailable.is_none() {
        // Nothing embedded, and not for want of time: the last sync could not
        // load the model. Worth the same warning as a failed search.
        unavailable = indexer.status().embedding_error;
    }
    let by_history = indexer.files_by_history(first);
    let mut fused = Vec::new();
    for (id, score, source) in fuse_rrf(lists) {
        if let Some(chunk) = store.load_chunk(&id)? {
            let score = score * weight(&chunk, &tokens, &by_history);
            fused.push((chunk, score, source));
        }
    }
    // Stable: equal scores keep the order fusion gave them.
    fused.sort_by(|a, b| b.1.total_cmp(&a.1));
    ranked.extend(fused.into_iter().map(|(chunk, _, source)| (chunk, source)));

    let mut seen = HashSet::new();
    let mut matches = Vec::with_capacity(top_k);
    let mut docs_left_out: Vec<(ChunkMetadata, MatchSource)> = Vec::new();
    let mut unasked_doc: Option<(ChunkMetadata, MatchSource)> = None;
    let mut paths_left_out = 0;
    let mut per_file: HashMap<String, usize> = HashMap::new();
    let mut later = Vec::new();
    for (chunk, source) in ranked {
        if matches.len() == top_k {
            break;
        }
        if !seen.insert(chunk.id.clone()) {
            continue;
        }
        if !filter.paths.allows(&chunk.file_id.0) {
            paths_left_out += 1;
            continue;
        }
        if !include_docs && is_documentation(&chunk.file_id.0) {
            if matches.is_empty() && unasked_doc.is_none() {
                unasked_doc = Some((chunk, source));
            } else {
                docs_left_out.push((chunk, source));
            }
            continue;
        }
        let Some(found) = resolved_match(indexer, chunk, source) else { continue };
        // A declaration the query named is exempt: it is the answer, however
        // many of its file's passages came before it.
        let count = per_file.entry(found.path.clone()).or_default();
        if *count >= MAX_PER_FILE && source != MatchSource::Symbol {
            later.push(found);
            continue;
        }
        *count += 1;
        matches.push(found);
    }
    let room = top_k - matches.len();
    matches.extend(later.into_iter().take(room));
    match unasked_doc {
        // No code at all: it heads the documentation given instead, below.
        Some(doc) if matches.is_empty() => docs_left_out.insert(0, doc),
        Some((chunk, source)) => {
            if let Some(doc) = resolved_match(indexer, chunk, source) {
                matches.insert(UNASKED_DOC_SLOT.min(matches.len()), doc);
                matches.truncate(top_k);
            }
        }
        None => {}
    }
    // No code at all — a folder of notes, say: the documentation left out is
    // the answer there is, and a second search to ask for it only costs a round.
    let docs_instead = matches.is_empty() && !docs_left_out.is_empty();
    if docs_instead {
        matches.extend(
            std::mem::take(&mut docs_left_out)
                .into_iter()
                .filter_map(|(chunk, source)| resolved_match(indexer, chunk, source))
                .take(top_k),
        );
    }

    let symbol_hits = matches.iter().filter(|m| m.source == MatchSource::Symbol).count() as u32;
    let (weak, hint) = weak_search_hint(SearchMetaInput {
        match_count: matches.len(),
        symbol_hits,
        has_semantic: matches.iter().any(|m| m.source == MatchSource::Semantic),
        only_lexical: matches.iter().all(|m| m.source == MatchSource::Lexical),
        extracted_tokens: &tokens,
    });
    // No Latin words in any wording: a question in another language, and
    // "add an identifier" alone does not say what went wrong. The bench
    // measured what fixes it — the English wording beside it.
    let foreign = tokens.is_empty() && all.chars().any(|c| c.is_alphabetic() && !c.is_ascii());
    let hint = if foreign {
        Some(
            "The query is in another language than most code, and ranking across languages is weak. \
             Search again with its English wording in queries — the question as the code would name it — \
             and any identifier you can guess.",
        )
    } else {
        hint
    };
    // A declaration the query named is found, whatever else it asks about.
    // Absent only when every wording is: a question in another language
    // lacks the code's words by itself, and its rewording has them.
    let mut unknown: Vec<&str> = Vec::new();
    // Not for a question in another language: its words are missing for
    // the language, and "may not be in this workspace" would be wrong.
    let mut absent = !matches.is_empty() && symbol_hits == 0 && !queries.is_empty() && !foreign;
    for query in &queries {
        if !absent {
            break;
        }
        let missing = unknown_words(indexer, query)?;
        absent = absent_from_workspace(missing.len(), query_words(query).len());
        for word in missing {
            if !unknown.contains(&word) {
                unknown.push(word);
            }
        }
    }
    let (weak, hint) = if absent {
        let words = unknown.iter().map(|w| format!("`{w}`")).collect::<Vec<_>>().join(", ");
        (
            true,
            Some(format!(
                "No indexed file has the words {words}: what the query asks about may not be in this workspace. \
                 These are only the nearest passages, not an answer — check with grep before relying on one."
            )),
        )
    } else {
        (weak, hint.map(str::to_string))
    };
    // Outranks the ordinary advice, which would say to wait for the semantic
    // index — wrong when waiting will not bring it back.
    let hint = match unavailable {
        Some(reason) => Some(format!(
            "Search by meaning is unavailable ({reason}); these results matched on names and words only."
        )),
        None => hint,
    };
    // Said even beside another hint: it is the one thing a second search can
    // change without rewording anything.
    let hint = match (docs_left_out.len(), hint) {
        _ if docs_instead => Some(
            "No code matched, so these are documentation matches — documentation is left out of a search \
             when anything else matches, unless includeDocs is set."
                .to_string(),
        ),
        (0, hint) => hint,
        (n, hint) => {
            let docs = format!(
                "{n} documentation {} {} left out ({}); search again with includeDocs if the answer may be in the docs.",
                if n == 1 { "match" } else { "matches" },
                if n == 1 { "was" } else { "were" },
                files_named(docs_left_out.iter().map(|(chunk, _)| chunk.file_id.0.as_str())),
            );
            Some(hint.map_or(docs.clone(), |hint| format!("{hint} {docs}")))
        }
    };
    // Nothing passed the path filter: without saying so this reads as
    // "nothing like it exists" rather than "not where you looked".
    let hint = if matches.is_empty() && paths_left_out > 0 {
        Some(format!(
            "None of the {paths_left_out} best matches passed glob/exclude; widen them or search without them."
        ))
    } else {
        hint
    };
    Ok(CodeSearchResult { matches, meta: SearchMeta { tiers_used, weak, hint } })
}

/// A passage as it stands on disk now, or `None` when its file changed since
/// it was indexed — the watcher is on its way.
fn resolved_match(indexer: &RepoIndexer, chunk: ChunkMetadata, source: MatchSource) -> Option<CodeMatch> {
    let resolved = resolve_chunk(indexer.root(), &chunk).ok()?;
    Some(CodeMatch {
        path: chunk.file_id.0,
        start_line: resolved.start_line,
        end_line: resolved.end_line,
        name: chunk.qualified_name,
        text: resolved.text,
        source,
    })
}

/// Up to three files by name, once each: `a.md, b.md and 4 more`.
fn files_named<'a>(paths: impl Iterator<Item = &'a str>) -> String {
    let mut unique: Vec<&str> = Vec::new();
    for path in paths {
        if !unique.contains(&path) {
            unique.push(path);
        }
    }
    let shown = unique.iter().take(3).copied().collect::<Vec<_>>().join(", ");
    match unique.len() {
        0..=3 => shown,
        n => format!("{shown} and {} more", n - 3),
    }
}

/// The words of the query ([`query_words`]) no indexed passage has, by the
/// prefix match the word ranking uses.
fn unknown_words<'q>(indexer: &RepoIndexer, query: &'q str) -> Result<Vec<&'q str>, IndexStoreError> {
    let mut unknown = Vec::new();
    for word in query_words(query) {
        let Some(fts) = fts5_query(word) else { continue };
        if indexer.store().search_bm25(&fts, 1)?.is_empty() {
            unknown.push(word);
        }
    }
    Ok(unknown)
}

/// The words of the query that are names as code spells them — and the whole
/// query when it is one word, since a one-word search is a name more often
/// than not. Plain words stay out: "how does sync work" must not put every
/// function called `sync` above the answer.
fn names_in(query: &str, tokens: &[String]) -> Vec<String> {
    let mut names: Vec<String> = tokens.iter().filter(|t| looks_like_identifier(t)).cloned().collect();
    let whole = query.trim();
    let one_word = !whole.is_empty() && whole.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if one_word && !names.iter().any(|n| n.eq_ignore_ascii_case(whole)) {
        names.push(whole.to_string());
    }
    names
}

/// What a chunk's fused score is multiplied by, for what it is rather than
/// what it says.
fn weight(chunk: &ChunkMetadata, tokens: &[String], by_history: &HashSet<String>) -> f32 {
    let prose = matches!(chunk.language, Language::Markdown | Language::PlainText);
    let named = tokens.iter().any(|token| path_segment_matches(&chunk.file_id.0, token));
    let changed = by_history.contains(&chunk.file_id.0);
    (if prose { PROSE_FACTOR } else { 1.0 })
        * (if named { PATH_BOOST } else { 1.0 })
        * (if changed { HISTORY_BOOST } else { 1.0 })
}

/// Merges rankings by `Σ weight / (K + rank)` over the lists that hold a
/// chunk, best first.
///
/// Rank, not score: cosine similarity and BM25 measure incomparable things,
/// and no fixed factor makes them commensurable across queries. A chunk keeps
/// the source of the first list it is in; ties go to the earlier list.
fn fuse_rrf(lists: impl IntoIterator<Item = (Vec<ChunkId>, MatchSource, f32)>) -> Vec<(ChunkId, f32, MatchSource)> {
    let mut fused: HashMap<ChunkId, (f32, (usize, usize), MatchSource)> = HashMap::new();
    for (order, (list, source, weight)) in lists.into_iter().enumerate() {
        for (rank, id) in list.into_iter().enumerate() {
            let contribution = weight / (RRF_K + rank as f32 + 1.0);
            fused.entry(id).and_modify(|entry| entry.0 += contribution).or_insert((contribution, (order, rank), source));
        }
    }
    let mut out: Vec<_> = fused.into_iter().collect();
    out.sort_by(|(_, a), (_, b)| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    out.into_iter().map(|(id, (score, _, source))| (id, score, source)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::chunk_index::ChunkBuildOptions;
    use crate::domain::embeddings::{Embedding, EmbeddingError, EmbeddingProvider};
    use crate::domain::workspace_index::IndexEventSink;
    use crate::testing::temp_dir;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    const DIMS: usize = 64;

    /// Code and documentation, no path filter — the ranking as it was.
    fn all() -> SearchFilter {
        SearchFilter { include_docs: true, ..SearchFilter::default() }
    }

    /// Code only, as the model searches by default.
    fn code() -> SearchFilter {
        SearchFilter::default()
    }

    /// "Meaning" is shared words, with one synonym so that meaning and words
    /// can disagree: `automobile` means `car`, and no text says `automobile`.
    #[derive(Default)]
    struct FakeModel {
        fail: AtomicBool,
    }

    impl EmbeddingProvider for FakeModel {
        fn embed(&self, texts: &[&str]) -> Result<Vec<Embedding>, EmbeddingError> {
            if self.fail.load(Ordering::SeqCst) {
                return Err(EmbeddingError::NotFound(PathBuf::from("model.safetensors")));
            }
            Ok(texts
                .iter()
                .map(|text| {
                    let mut v = vec![0.0_f32; DIMS];
                    for word in text.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()) {
                        let word = if word == "automobile" { "car" } else { word };
                        v[blake3::hash(word.to_lowercase().as_bytes()).as_bytes()[0] as usize % DIMS] += 1.0;
                    }
                    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
                    Embedding(v.iter().map(|x| x / norm).collect())
                })
                .collect())
        }
        fn dimensions(&self) -> usize {
            DIMS
        }
    }

    fn indexed(label: &str, files: &[(&str, &str)], model: Arc<FakeModel>) -> RepoIndexer {
        let root = temp_dir(&format!("{label}-repo"));
        for (path, body) in files {
            fs::write(root.join(path), body).unwrap();
        }
        let indexer =
            RepoIndexer::open(&root, &temp_dir(&format!("{label}-store")), model, ChunkBuildOptions::default()).unwrap();
        let sink: IndexEventSink = Arc::new(|_| {});
        indexer.sync(&sink).unwrap();
        indexer
    }

    fn summary(result: &CodeSearchResult) -> Vec<(String, MatchSource)> {
        result.matches.iter().map(|m| (m.name.clone().unwrap_or_else(|| m.path.clone()), m.source)).collect()
    }

    // ------------------------------------------------------------ names

    /// Prose that repeats a name all over outranks its declaration on words;
    /// the declaration still comes first.
    #[test]
    fn a_name_the_query_spells_comes_first_with_its_lines() {
        let indexer = indexed(
            "search-name",
            &[
                ("notes.md", "# Notes\n\nparse_config parse_config parse_config is called on start.\n"),
                ("config.rs", "use std::fs;\n\nfn parse_config() {\n    todo!()\n}\n"),
            ],
            Arc::default(),
        );

        let result = search(&indexer, "where is parse_config", None, 5, &all()).unwrap();

        let first = &result.matches[0];
        assert_eq!((first.path.as_str(), first.source), ("config.rs", MatchSource::Symbol));
        assert_eq!((first.start_line, first.end_line), (1, 5));
        assert_eq!(result.matches.iter().filter(|m| m.path == "config.rs").count(), 1, "listed twice");
    }

    /// Documentation is out unless asked for, and the hint says what that
    /// cost — the model's way to know a second search with it is worth making.
    #[test]
    fn documentation_is_left_out_unless_asked_for_and_the_hint_says_so() {
        let indexer = indexed(
            "search-docs",
            &[
                ("notes.md", "# Notes\n\nparse_config parse_config parse_config is called on start.\n"),
                ("config.rs", "use std::fs;\n\nfn parse_config() {\n    todo!()\n}\n"),
            ],
            Arc::default(),
        );

        let code = search(&indexer, "where is parse_config called", None, 5, &code()).unwrap();
        assert!(code.matches.iter().all(|m| m.path == "config.rs"), "{:?}", summary(&code));
        let hint = code.meta.hint.unwrap_or_default();
        assert!(hint.contains("1 documentation match was left out (notes.md)") && hint.contains("includeDocs"), "{hint}");

        let all = search(&indexer, "where is parse_config called", None, 5, &all()).unwrap();
        assert!(all.matches.iter().any(|m| m.path == "notes.md"), "{:?}", summary(&all));
        assert!(!all.meta.hint.unwrap_or_default().contains("includeDocs"));
    }

    /// A folder of notes: with no code to show, the documentation is the
    /// answer, and the hint says it came in place of code rather than beside it.
    #[test]
    fn with_no_code_matched_the_documentation_is_the_result() {
        let indexer = indexed(
            "search-docs-only",
            &[
                ("a.txt", "The deploy runs every night at two and ships the parser.\n"),
                ("b.txt", "Holidays: the office closes in August.\n"),
            ],
            Arc::default(),
        );

        let found = search(&indexer, "when does the deploy run", None, 5, &code()).unwrap();
        assert!(found.matches.iter().any(|m| m.path == "a.txt"), "{:?}", summary(&found));
        let hint = found.meta.hint.unwrap_or_default();
        assert!(hint.starts_with("No code matched, so these are documentation matches"), "{hint}");
        assert!(!hint.contains("left out ("), "{hint}");
    }

    /// Prose that outranks the code on every measure fills the plain candidate
    /// pool; left out, it must not leave the search empty-handed.
    #[test]
    fn documentation_crowding_the_candidates_does_not_crowd_out_the_code() {
        let indexer = indexed(
            "search-docs-crowd",
            &[
                ("a.md", "gizmo gizmo gizmo stuff\n"),
                ("b.md", "gizmo gizmo gizmo stuff\n"),
                ("c.md", "gizmo gizmo gizmo stuff\n"),
                ("run.rs", "fn run() { gizmo }\n"),
            ],
            Arc::default(),
        );

        let result = search(&indexer, "gizmo stuff", None, 1, &code()).unwrap();

        assert_eq!(summary(&result).first().map(|(name, _)| name.as_str()), Some("run"), "{:?}", summary(&result));
        assert_eq!(result.matches.len(), 1, "{:?}", summary(&result));
    }

    /// The nearest commits count, and no more than a few of them: a query
    /// near everything is evidence about nothing.
    #[test]
    fn only_the_nearest_few_commits_count() {
        let files = [("f1.rs", "gear"), ("f2.rs", "gear gear alpha"), ("f3.rs", "gear alpha"), ("f4.rs", "gear alpha beta")];
        let indexer = indexed("search-history-near", &files.map(|(path, _)| (path, "fn x() {}\n")), Arc::default());
        let repo = git2::Repository::init(indexer.root()).unwrap();
        // Oldest first, so the newest commit is the least near.
        for (path, message) in files {
            commit_file(&repo, path, message);
        }

        let near = indexer.files_by_history("gear");

        assert_eq!(near, HashSet::from(["f1.rs".to_string(), "f2.rs".to_string(), "f3.rs".to_string()]));
    }

    /// The path filter keeps what it allows; when nothing passes, the hint
    /// says it was the filter, not an absence.
    #[test]
    fn a_path_filter_keeps_what_it_allows_and_says_when_nothing_passed() {
        use crate::domain::search_query::PathFilter;
        let indexer = indexed(
            "search-paths",
            &[("a.rs", "fn a() { gizmo }\n"), ("b.java", "class B { void gizmo() {} }\n")],
            Arc::default(),
        );
        let only = |glob: &str| SearchFilter { paths: PathFilter::new(Some(glob), None).unwrap(), ..all() };

        let java = search(&indexer, "gizmo please", None, 5, &only("*.java")).unwrap();
        assert!(!java.matches.is_empty() && java.matches.iter().all(|m| m.path == "b.java"), "{:?}", summary(&java));

        let none = search(&indexer, "gizmo please", None, 5, &only("*.py")).unwrap();
        assert!(none.matches.is_empty());
        assert!(none.meta.hint.unwrap_or_default().contains("passed glob/exclude"));
    }

    /// One file's passages all outrank another file's only one: the other
    /// file still gets a place, and the rest come back when there is room.
    #[test]
    fn one_file_does_not_take_every_place() {
        let indexer = indexed(
            "search-per-file",
            &[
                (
                    "long.rs",
                    "fn a() { gizmo(gizmo) }\n\nfn b() { gizmo(gizmo) }\n\nfn c() { gizmo(gizmo) }\n\nfn d() { gizmo(gizmo) }\n",
                ),
                ("other.rs", "fn e() { gizmo() }\n"),
            ],
            Arc::default(),
        );
        let files = |top_k| -> Vec<String> {
            search(&indexer, "gizmo please", None, top_k, &all()).unwrap().matches.into_iter().map(|m| m.path).collect()
        };

        assert_eq!(files(3), ["long.rs", "long.rs", "other.rs"]);
        assert_eq!(files(10).len(), 5, "held-back passages return when there is room");
    }

    /// Commits one file at a time, so each commit touches exactly `path`.
    fn commit_file(repo: &git2::Repository, path: &str, message: &str) {
        let mut index = repo.index().unwrap();
        index.add_path(std::path::Path::new(path)).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("Test", "test@example.com").unwrap();
        let parent: Vec<git2::Commit> = repo.head().ok().and_then(|h| h.peel_to_commit().ok()).into_iter().collect();
        let parents: Vec<&git2::Commit> = parent.iter().collect();
        repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parents).unwrap();
    }

    /// A commit whose message is near the query points at the files it
    /// touched, and those weigh a little more — only those, and only when
    /// some commit is near.
    #[test]
    fn a_file_a_near_commit_touched_weighs_a_little_more() {
        let indexer = indexed(
            "search-history",
            &[("a.rs", "fn a() { gear }\n"), ("b.rs", "fn b() { gear }\n")],
            Arc::default(),
        );
        let repo = git2::Repository::init(indexer.root()).unwrap();
        commit_file(&repo, "a.rs", "initial setup");
        commit_file(&repo, "b.rs", "gear handling");

        let near = indexer.files_by_history("gear");
        assert_eq!(near, HashSet::from(["b.rs".to_string()]));
        assert!(indexer.files_by_history("nothing like it").is_empty());

        let ids = indexer.store().search_bm25(&fts5_query("gear").unwrap(), 5).unwrap();
        let chunk_of = |path: &str| {
            ids.iter().find_map(|(id, _)| indexer.store().load_chunk(id).unwrap().filter(|c| c.file_id.0 == path)).unwrap()
        };
        assert_eq!(weight(&chunk_of("b.rs"), &[], &near), HISTORY_BOOST);
        assert_eq!(weight(&chunk_of("a.rs"), &[], &near), 1.0);

        // A new commit is seen by the next search: history is read per HEAD.
        fs::write(indexer.root().join("c.rs"), "fn c() {}\n").unwrap();
        commit_file(&repo, "c.rs", "gear");
        assert!(indexer.files_by_history("gear").contains("c.rs"));
    }

    #[test]
    fn a_plain_word_is_not_taken_for_a_name() {
        let indexer = indexed("search-plain", &[("a.rs", "fn sync() {}\n")], Arc::default());
        let result = search(&indexer, "how does sync work", None, 5, &all()).unwrap();
        assert!(result.matches.iter().all(|m| m.source != MatchSource::Symbol), "{:?}", summary(&result));
    }

    #[test]
    fn a_one_word_query_is_taken_for_a_name() {
        let indexer = indexed("search-one-word", &[("a.rs", "fn first() {}\n\nfn tokenize() {}\n")], Arc::default());
        let result = search(&indexer, "tokenize", None, 5, &all()).unwrap();
        // The chunk the declaration is in, not the file's first.
        let symbols: Vec<_> = summary(&result).into_iter().filter(|(_, s)| *s == MatchSource::Symbol).collect();
        assert_eq!(symbols, [("tokenize".to_string(), MatchSource::Symbol)]);
        assert_eq!((result.matches[0].start_line, result.matches[0].end_line), (3, 3));
        // In whatever case it was typed.
        assert_eq!(search(&indexer, "TOKENIZE", None, 5, &all()).unwrap().matches[0].source, MatchSource::Symbol);
    }

    // --------------------------------------------------- words and meaning

    /// Nearest passages come back for any query; words the workspace has
    /// nowhere say they are not an answer.
    #[test]
    fn a_query_about_what_the_workspace_lacks_is_weak_and_names_the_words() {
        let indexer =
            indexed("search-absent", &[("deploy.rs", "fn deploy_service() {\n    // push the build\n}\n")], Arc::default());

        let result = search(&indexer, "kubernetes helm deploy", None, 5, &all()).unwrap();

        assert!(!result.matches.is_empty(), "the nearest passages still come back");
        assert!(result.meta.weak);
        let hint = result.meta.hint.unwrap();
        assert!(hint.starts_with("No indexed file has the words `kubernetes`, `helm`:"), "{hint}");
        // A declaration the query names is found, whatever else it asks about.
        let named = search(&indexer, "kubernetes helm deploy_service", None, 5, &all()).unwrap();
        assert!(!named.meta.hint.unwrap_or_default().contains("No indexed file"));
        // Words another wording has count: a question in another language
        // with its rewording in the code's own is not about something absent.
        let reworded = search_many(&indexer, &["kubernetes helm", "deploy the service"], None, 5, &all()).unwrap();
        assert!(!reworded.meta.hint.unwrap_or_default().contains("No indexed file"));
        let both = search_many(&indexer, &["kubernetes helm", "terraform ansible"], None, 5, &all()).unwrap();
        let hint = both.meta.hint.unwrap_or_default();
        assert!(hint.starts_with("No indexed file has the words `kubernetes`, `helm`, `terraform`, `ansible`:"), "{hint}");
        // Not the verdict: the ordinary ones still come through.
        let wordless = search(&indexer, "как это работает", None, 5, &all()).unwrap();
        assert!(wordless.meta.weak, "{:?}", wordless.meta);
        assert!(wordless.meta.hint.unwrap().contains("English wording in queries"), "said what to do about the language");
    }

    /// What only a later wording finds still gets in — here a name only it
    /// spells; wordings past the limit are not searched at all.
    #[test]
    fn every_wording_is_searched_up_to_the_limit() {
        let indexer = indexed(
            "search-wordings",
            &[("drive.rs", "fn start_car() {}\n"), ("other.rs", "fn unrelated_fn() {}\n")],
            Arc::default(),
        );
        let symbols = |result: &CodeSearchResult| {
            result.matches.iter().filter(|m| m.source == MatchSource::Symbol).filter_map(|m| m.name.clone()).collect::<Vec<_>>()
        };

        let one = search_many(&indexer, &["машина", "start_car"], None, 5, &all()).unwrap();
        assert_eq!(symbols(&one), ["start_car"]);

        let mut many = vec!["машина"; MAX_QUERIES];
        many.push("unrelated_fn");
        let cut = search_many(&indexer, &many, None, 5, &all()).unwrap();
        assert!(symbols(&cut).is_empty(), "the one past the limit was dropped: {:?}", symbols(&cut));
        let blanks = search_many(&indexer, &["", " ", "", "", "start_car"], None, 5, &all()).unwrap();
        assert_eq!(symbols(&blanks), ["start_car"], "an empty wording takes no place under the limit");
    }

    /// `fts` is the first wording's words; a later one is ranked by its own.
    #[test]
    fn each_later_wording_is_ranked_by_its_own_words() {
        let model = Arc::new(FakeModel { fail: AtomicBool::new(true) });
        let indexer = indexed("search-wording-words", &[("a.rs", "fn a() { gizmo }\n"), ("b.rs", "fn b() { widget }\n")], model);

        let result = search_many(&indexer, &["nothing here", "the widget"], Some(&["gizmo".to_string()]), 5, &all()).unwrap();

        let mut paths: Vec<&str> = result.matches.iter().map(|m| m.path.as_str()).collect();
        paths.sort_unstable();
        assert_eq!(paths, ["a.rs", "b.rs"]);
    }

    /// A question in another language is told the remedy the bench measured,
    /// not only "add an identifier"; once reworded, it is not told again.
    #[test]
    fn a_question_in_another_language_is_told_to_add_its_english_wording() {
        let indexer = indexed("search-foreign", &[("send.rs", "fn send_pack() {}\n")], Arc::default());

        let alone = search(&indexer, "где отправляется пакет", None, 5, &all()).unwrap();
        let hint = alone.meta.hint.unwrap_or_default();
        assert!(hint.contains("English wording in queries") && !hint.contains("No indexed file"), "{hint}");
        assert!(alone.meta.weak);

        let reworded = search_many(&indexer, &["где отправляется пакет", "where the pack is sent"], None, 5, &all()).unwrap();
        assert!(!reworded.meta.hint.unwrap_or_default().contains("English wording"));
        let named = search(&indexer, "где вызывается send_pack", None, 5, &all()).unwrap();
        assert!(!named.meta.hint.unwrap_or_default().contains("English wording"), "a name from the code is in it");
        let short = search(&indexer, "is it on", None, 5, &all()).unwrap();
        assert!(!short.meta.hint.unwrap_or_default().contains("English wording"), "no word of it is foreign");
        let latin = search(&indexer, "where it is sent", None, 5, &all()).unwrap();
        assert!(!latin.meta.hint.unwrap_or_default().contains("English wording"));
    }

    #[test]
    fn meaning_finds_what_shares_no_word_with_the_query() {
        let indexer = indexed(
            "search-meaning",
            &[("drive.rs", "fn start_car() {\n    // turn the key\n}\n"), ("other.rs", "fn unrelated() {}\n")],
            Arc::default(),
        );

        let result = search(&indexer, "automobile", None, 1, &all()).unwrap();

        assert_eq!(summary(&result), [("start_car".to_string(), MatchSource::Semantic)]);
        assert!(result.meta.tiers_used.contains(&MatchSource::Semantic));
        // Found by words as well, it is still labelled by meaning.
        let both = search(&indexer, "turn the key", None, 1, &all()).unwrap();
        assert_eq!(summary(&both), [("start_car".to_string(), MatchSource::Semantic)]);
    }

    #[test]
    fn fusion_ranks_agreement_first_and_keeps_the_first_lists_label() {
        let id = |s: &str| ChunkId(s.into());
        let fused = fuse_rrf([
            (vec![id("a"), id("b")], MatchSource::Semantic, 1.0),
            (vec![id("b"), id("c")], MatchSource::Lexical, 1.0),
        ]);
        let order: Vec<_> = fused.into_iter().map(|(id, _, source)| (id, source)).collect();
        assert_eq!(
            order,
            [(id("b"), MatchSource::Semantic), (id("a"), MatchSource::Semantic), (id("c"), MatchSource::Lexical)]
        );
    }

    #[test]
    fn ties_go_to_the_earlier_list() {
        let id = |s: &str| ChunkId(s.into());
        for _ in 0..20 {
            let fused =
                fuse_rrf([(vec![id("z")], MatchSource::Semantic, 1.0), (vec![id("a")], MatchSource::Lexical, 1.0)]);
            assert_eq!(fused[0].0, id("z"));
        }
    }

    /// First by one measure beats sixth by both — the case the published
    /// K of 60 got wrong.
    #[test]
    fn a_first_place_outweighs_two_middling_ones() {
        let id = |s: &str| ChunkId(s.into());
        let filler = |p: &str| (0..5).map(|i| id(&format!("{p}{i}"))).collect::<Vec<_>>();
        let mut meaning = vec![id("top")];
        meaning.extend(filler("m"));
        meaning.push(id("both"));
        let mut words = filler("w");
        words.push(id("both"));

        let fused = fuse_rrf([(meaning, MatchSource::Semantic, 1.0), (words, MatchSource::Lexical, 1.0)]);

        assert_eq!(fused[0].0, id("top"));
    }

    /// Two files with the same text differ only by where they are; the vector
    /// knows it, because the file's name is embedded with the text.
    #[test]
    fn meaning_knows_which_file_a_chunk_is_in() {
        let indexer = indexed(
            "search-embedded-header",
            &[("gearbox.rs", "fn run() {}\n"), ("other.rs", "fn run() {}\n")],
            Arc::default(),
        );

        let hits = indexer.search_meaning("gearbox", 2).unwrap();

        assert_eq!(indexer.store().load_chunk(&hits[0].0).unwrap().unwrap().file_id.0, "gearbox.rs");
        assert!(hits[0].1 > hits[1].1, "by meaning, not by a tie: {hits:?}");
    }

    /// What is embedded is part of the vectors' identity: stored under the
    /// model's id alone, vectors of the text without its path would be taken
    /// for current ones after an update, and never rebuilt.
    #[test]
    fn vectors_are_stored_under_the_model_and_the_embedded_text() {
        use crate::domain::embeddings::LOCAL_MODEL_ID;
        use crate::services::embedding_index::EMBEDDED_TEXT_VERSION;
        let indexer = indexed("search-model-id", &[("a.rs", "fn a() {}\n")], Arc::default());
        let store = indexer.store();

        assert!(store.load_all_embeddings(LOCAL_MODEL_ID, DIMS).unwrap().is_empty());
        let versioned = format!("{LOCAL_MODEL_ID}{EMBEDDED_TEXT_VERSION}");
        assert!(!store.load_all_embeddings(&versioned, DIMS).unwrap().is_empty());
    }

    /// Declarations the query names are exempt from the per-file limit: the
    /// third one named is still an answer, not a repeat.
    #[test]
    fn declarations_the_query_names_are_not_held_back() {
        let indexer = indexed(
            "search-per-file-named",
            &[
                ("one.rs", "fn alpha_fn() {}\n\nfn beta_fn() {}\n\nfn gamma_fn() {}\n"),
                ("two.rs", "fn other() { alpha_fn(); beta_fn(); gamma_fn(); }\n"),
            ],
            Arc::default(),
        );

        let result = search(&indexer, "alpha_fn beta_fn gamma_fn", None, 3, &all()).unwrap();

        let mut names: Vec<_> = result.matches.iter().map(|m| m.name.clone().unwrap_or_default()).collect();
        names.sort();
        assert_eq!(names, ["alpha_fn", "beta_fn", "gamma_fn"], "{:?}", summary(&result));
    }

    /// A path filter that drops the whole plain candidate pool must not leave
    /// the search empty-handed: filtered, each ranking brings in more.
    #[test]
    fn a_path_filter_widens_the_pool_it_filters() {
        use crate::domain::search_query::PathFilter;
        let indexer = indexed(
            "search-paths-crowd",
            &[
                ("a.rs", "fn a() { gizmo(gizmo, gizmo) }\n"),
                ("b.rs", "fn b() { gizmo(gizmo, gizmo) }\n"),
                ("c.rs", "fn c() { gizmo(gizmo, gizmo) }\n"),
                ("D.java", "class D { void d() { gizmo(); } }\n"),
            ],
            Arc::default(),
        );
        let java = SearchFilter { paths: PathFilter::new(Some("*.java"), None).unwrap(), ..all() };

        let result = search(&indexer, "gizmo please", None, 1, &java).unwrap();

        assert_eq!(result.matches.first().map(|m| m.path.as_str()), Some("D.java"), "{:?}", summary(&result));
    }

    /// Each is first by one measure and second by the other: words win.
    #[test]
    fn a_first_place_by_words_outweighs_one_by_meaning() {
        let indexer = indexed(
            "search-words-weigh",
            &[("x.rs", "fn x() {\n    wheel(wheel, wheel);\n}\n"), ("y.rs", "fn y() {\n    car(wheel, car, car);\n}\n")],
            Arc::default(),
        );
        let meaning = indexer.search_meaning("automobile wheel", 2).unwrap();
        assert_eq!(indexer.store().load_chunk(&meaning[0].0).unwrap().unwrap().file_id.0, "y.rs", "set-up");

        let result = search(&indexer, "automobile wheel", None, 2, &all()).unwrap();

        assert_eq!(result.matches[0].path, "x.rs", "{:?}", summary(&result));
    }

    /// Prose — Markdown or anything read as plain text — that matches as well
    /// as the code, or a little better, is listed after it.
    #[test]
    fn prose_gives_way_to_code_that_matches_as_well() {
        for notes in ["notes.md", "notes.adoc"] {
            let indexer = indexed(
                "search-prose",
                &[(notes, "# Notes\n\nwidget gear widget gear\n"), ("turn.rs", "fn turn() {\n    widget(gear);\n}\n")],
                Arc::default(),
            );
            let result = search(&indexer, "widget gear", None, 2, &all()).unwrap();
            assert_eq!(result.matches[0].path, "turn.rs", "{notes}: {:?}", summary(&result));
        }
    }

    /// Scaled down, not out: prose that is the answer by every measure stays
    /// above code found only by a vague likeness.
    #[test]
    fn prose_that_matches_far_better_still_comes_first() {
        let indexer = indexed(
            "search-prose-wins",
            &[("notes.md", "# Notes\n\nwidget gear widget gear\n"), ("turn.rs", "fn turn() {\n    spin();\n}\n")],
            Arc::default(),
        );
        let result = search(&indexer, "widget gear", None, 2, &all()).unwrap();
        assert_eq!(result.matches[0].path, "notes.md", "{:?}", summary(&result));
    }

    /// A query word naming the file lifts it over a closer match elsewhere.
    #[test]
    fn a_query_word_in_the_path_lifts_the_file() {
        let indexer = indexed(
            "search-path",
            &[("other.rs", "fn run() {\n    gear(gear);\n}\n"), ("widget.rs", "fn run() {\n    gear();\n}\n")],
            Arc::default(),
        );
        let result = search(&indexer, "widget gear", None, 2, &all()).unwrap();
        assert_eq!(result.matches[0].path, "widget.rs", "{:?}", summary(&result));
    }

    // ------------------------------------------------------------ limits

    #[test]
    fn a_file_changed_since_it_was_indexed_is_left_out() {
        let indexer = indexed(
            "search-stale",
            &[("a.rs", "fn widget_one() {}\n"), ("b.rs", "fn widget_two() {}\n")],
            Arc::default(),
        );
        fs::write(indexer.root().join("a.rs"), "fn widget_ONE() {}\n").unwrap();

        let result = search(&indexer, "widget", None, 5, &all()).unwrap();

        assert_eq!(result.matches.iter().map(|m| m.path.as_str()).collect::<Vec<_>>(), ["b.rs"]);
    }

    /// The best candidate is stale; the next one fills its place, even when
    /// only one match was asked for.
    #[test]
    fn a_stale_best_match_is_replaced_by_the_next() {
        let indexer = indexed(
            "search-stale-slack",
            &[("a.rs", "fn widget() { widget(); widget(); }
"), ("b.rs", "fn other() { widget(); }
")],
            Arc::default(),
        );
        fs::write(indexer.root().join("a.rs"), "fn changed() {}
").unwrap();

        let result = search(&indexer, "calls to widget", None, 1, &all()).unwrap();

        assert_eq!(result.matches.iter().map(|m| m.path.as_str()).collect::<Vec<_>>(), ["b.rs"]);
    }

    #[test]
    fn the_number_asked_for_is_held_between_one_and_the_maximum() {
        let body: String = (0..60).map(|i| format!("fn gadget_{i}() {{ gadget }}\n")).collect();
        let indexer = indexed("search-limits", &[("a.rs", &body)], Arc::default());
        assert_eq!(search(&indexer, "gadget", None, 0, &all()).unwrap().matches.len(), 1);
        assert_eq!(search(&indexer, "gadget", None, 500, &all()).unwrap().matches.len(), MAX_TOP_K);
    }

    // ------------------------------------------------ meaning unavailable

    /// The model loaded for the sync and fails for the query: search still
    /// answers, on words, and says so.
    #[test]
    fn a_failing_model_leaves_words_and_says_why() {
        let model = Arc::new(FakeModel::default());
        let indexer = indexed("search-fails", &[("a.rs", "fn gizmo() {}\n")], Arc::clone(&model));
        model.fail.store(true, Ordering::SeqCst);

        // A plain word, so no name lookup: only words can find it.
        let result = search(&indexer, "what does gizmo do", None, 5, &all()).unwrap();

        assert_eq!(result.matches.len(), 1);
        assert!(!result.meta.tiers_used.contains(&MatchSource::Semantic));
        assert!(result.meta.hint.as_deref().is_some_and(|h| h.contains("model.safetensors")), "{:?}", result.meta);
    }

    /// With the model down, only the word ranking answers — which makes what
    /// it searched for visible.
    #[test]
    fn fts_is_what_the_words_are_matched_against() {
        let model = Arc::new(FakeModel::default());
        let indexer = indexed("search-fts", &[("a.rs", "fn gizmo() {}\n")], Arc::clone(&model));
        model.fail.store(true, Ordering::SeqCst);

        let terms = ["gizmo".to_string()];
        assert_eq!(search(&indexer, "the thing that starts up", Some(&terms), 5, &all()).unwrap().matches.len(), 1);
        // Nothing searchable in `fts`: the query's own words are used.
        let noise = ["!!".to_string()];
        assert_eq!(search(&indexer, "what does gizmo do", Some(&noise), 5, &all()).unwrap().matches.len(), 1);
    }

    /// A wrong word in `fts` does not cost the query's own words their match.
    #[test]
    fn fts_is_searched_beside_the_query_words() {
        let model = Arc::new(FakeModel::default());
        let indexer = indexed("search-fts-beside", &[("a.rs", "fn gizmo() {}\n")], Arc::clone(&model));
        model.fail.store(true, Ordering::SeqCst);

        let wrong = ["sprocket".to_string()];
        let found = search(&indexer, "what does gizmo do", Some(&wrong), 5, &all()).unwrap();
        assert_eq!(found.matches.len(), 1, "{:?}", summary(&found));
    }

    /// Without `includeDocs`, documentation that outranks all the code still
    /// comes — once, fifth, below the code it would otherwise displace.
    #[test]
    fn documentation_above_all_the_code_comes_fifth_unasked() {
        let mut files: Vec<(String, String)> =
            (0..6).map(|i| (format!("f{i}.rs"), format!("fn f{i}() {{ widget }}\n"))).collect();
        files.push(("notes.md".into(), "# Gizmo widget\n\ngizmo widget gizmo widget gizmo widget\n".into()));
        files.push(("more.md".into(), "# Other\n\nwidget\n".into()));
        let files: Vec<(&str, &str)> = files.iter().map(|(p, b)| (p.as_str(), b.as_str())).collect();
        let indexer = indexed("search-docs-unasked", &files, Arc::default());

        let everything = search(&indexer, "gizmo widget", None, 10, &all()).unwrap();
        assert_eq!(everything.matches[0].path, "notes.md", "{:?}", summary(&everything));

        let code = search(&indexer, "gizmo widget", None, 10, &code()).unwrap();
        let paths: Vec<&str> = code.matches.iter().map(|m| m.path.as_str()).collect();
        assert_eq!(paths.iter().filter(|p| p.ends_with(".md")).count(), 1, "{paths:?}");
        assert_eq!(paths[UNASKED_DOC_SLOT], "notes.md", "{paths:?}");
        assert!(code.meta.hint.unwrap_or_default().contains("1 documentation match was left out (more.md)"));
    }

    /// Never embedded because the model would not load: nothing to search by
    /// meaning, and the hint must not say "wait for it".
    #[test]
    fn a_model_that_never_loaded_is_named_rather_than_waited_for() {
        let model = Arc::new(FakeModel { fail: AtomicBool::new(true) });
        let indexer = indexed("search-no-model", &[("a.rs", "fn gizmo() {}\n")], model);

        let result = search(&indexer, "what does gizmo do", None, 5, &all()).unwrap();

        let hint = result.meta.hint.unwrap();
        assert!(hint.contains("unavailable") && !hint.contains("finished building"), "{hint}");
    }
}
