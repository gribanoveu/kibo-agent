//! What a search query is made of, before any index sees it.
//!
//! Two readings of the same string, for two different indexes:
//!
//! * [`fts5_query`] — every word, in any script, for the full-text tier. A
//!   Russian question about Russian docs has to work there, because nothing
//!   else will answer it.
//! * [`extract_search_tokens`] — only what looks like it came from code
//!   (`UserService`, `compact_if_needed`, `src/lib/chat.ts`), for matching
//!   symbol names and paths, which are ASCII.
//!
//! And one verdict on the result: [`weak_search_hint`], which tells the model
//! when a hit list is thin and what to change, so it refines the query rather
//! than re-running it.
//!
//! ## Not ported
//!
//! Upstream's RU→EN root dictionary (`уведомлен` → `Notification`, five
//! entries). It encoded one patent-office codebase's vocabulary; in a general
//! coding agent it would be either wrong or empty, and the semantic tier is
//! what bridges languages. Its Russian model-facing hints are English here,
//! like every other string the model reads.

use std::collections::HashSet;

/// Below this a plain word is noise in a symbol search (`a`, `to`, `id`).
const MIN_PLAIN_TOKEN_LEN: usize = 3;

/// A stem shorter than this matches half of all names.
const MIN_STEM_LEN: usize = 4;

/// A long question is not made more precise by its twenty-fifth word.
const MAX_FTS_TERMS: usize = 24;
const MIN_FTS_TERM_CHARS: usize = 2;

/// Terms of at least this many characters are searched as prefixes...
pub const FTS_PREFIX_MIN_CHARS: usize = 4;
/// ...cut to at most this many. `unicode61` case-folds Cyrillic but does not
/// stem it, so `уведомления` has to meet `уведомление` at a shared prefix.
/// The schema's `prefix = '4 5 6'` builds an index for exactly the lengths
/// these two produce, and a test in `infra::index_store` keeps them in step.
pub const FTS_STEM_PREFIX_CHARS: usize = 6;

/// The query as an FTS5 `MATCH` expression, or `None` when no word in it is
/// long enough to search for.
///
/// OR, not AND: BM25 ranks a chunk holding every term above one holding a
/// few, so OR only adds recall, and AND would return nothing for a question
/// with one word the code does not contain. Every term is quoted, so FTS5
/// syntax in the query (`NOT`, `*`, `:`, `"`) is text, not an operator.
///
/// Splits on anything that is not a letter or digit — which is also how the
/// index's tokenizer split the text, so `compact_if_needed` becomes the same
/// three words on both sides.
///
/// ## One name, two spellings
///
/// The tokenizer keeps `readSource` in a TypeScript file as one token and
/// splits `read_source` in a Rust file into two adjacent ones. A model asking
/// for `readSource` in a Rust repository — camelCase is how it names things by
/// default — found nothing but prose quoting the word. So a `camelCase` word
/// is searched as itself **and** as the phrase `"read source"`: exactly the
/// token sequence its `snake_case` spelling is stored as.
///
/// A phrase and not the parts as separate terms. That was tried first, and
/// measured on this repository it was worse than nothing: `read` and `source`
/// are everywhere, and as independent terms they outranked the definition of
/// `ContextUsage` with tests that merely mention context.
pub fn fts5_query(query: &str) -> Option<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut terms: Vec<String> = Vec::new();

    for word in query.split(|c: char| !c.is_alphanumeric()) {
        let parts = camel_parts(word);
        if parts.len() >= 2 && terms.len() < MAX_FTS_TERMS {
            let phrase = format!("\"{}\"", parts.join(" ").to_lowercase());
            if seen.insert(phrase.clone()) {
                terms.push(phrase);
            }
        }
        let raw = word;
        if terms.len() >= MAX_FTS_TERMS {
            break;
        }
        let lower = raw.to_lowercase();
        let chars = lower.chars().count();
        if chars < MIN_FTS_TERM_CHARS {
            continue;
        }
        let term = if chars >= FTS_PREFIX_MIN_CHARS {
            let stem: String = lower.chars().take(FTS_STEM_PREFIX_CHARS).collect();
            format!("\"{stem}\"*")
        } else {
            format!("\"{lower}\"")
        };
        if seen.insert(term.clone()) {
            terms.push(term);
        }
    }

    (!terms.is_empty()).then(|| terms.join(" OR "))
}

/// `readSource` → `read`, `Source`; `HTTPServer` → `HTTP`, `Server`. Nothing
/// for a word with no case boundary inside it — the word itself is already a
/// term.
fn camel_parts(word: &str) -> Vec<&str> {
    let chars: Vec<(usize, char)> = word.char_indices().collect();
    let mut cuts = vec![0];
    for i in 1..chars.len() {
        let (at, c) = chars[i];
        let prev = chars[i - 1].1;
        let next_is_lower = chars.get(i + 1).is_some_and(|(_, n)| n.is_lowercase());
        if c.is_uppercase() && (prev.is_lowercase() || prev.is_ascii_digit() || (prev.is_uppercase() && next_is_lower)) {
            cuts.push(at);
        }
    }
    if cuts.len() < 2 {
        return Vec::new();
    }
    cuts.push(word.len());
    cuts.windows(2).map(|w| &word[w[0]..w[1]]).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum TokenKind {
    Plain = 0,
    PathLike = 1,
    Identifier = 2,
}

/// The code-shaped words of a query, most specific first: identifiers, then
/// paths, then plain words; longer before shorter within each. Deduplicated
/// ignoring case. ASCII only — symbol names and paths are.
pub fn extract_search_tokens(query: &str) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut scored: Vec<(TokenKind, String)> = Vec::new();
    let mut push = |kind: TokenKind, token: &str| {
        if seen.insert(token.to_ascii_lowercase()) {
            scored.push((kind, token.to_string()));
        }
    };

    for raw in split_raw_segments(query) {
        if looks_like_identifier(&raw) {
            push(TokenKind::Identifier, &raw);
        } else if is_path_like(&raw) {
            push(TokenKind::PathLike, &raw);
            // `src/lib/chat.ts` is also a search for `chat`.
            if let Some(stem) = path_stem(&raw) {
                if looks_like_identifier(stem) {
                    push(TokenKind::Identifier, stem);
                } else if stem.len() >= MIN_PLAIN_TOKEN_LEN {
                    push(TokenKind::Plain, stem);
                }
            }
        } else if raw.len() >= MIN_PLAIN_TOKEN_LEN && raw.chars().all(|c| c.is_ascii_alphanumeric()) {
            push(TokenKind::Plain, &raw);
        }
    }

    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.len().cmp(&a.1.len())));
    scored.into_iter().map(|(_, t)| t).collect()
}

/// Whether any token is a name as code spells it.
pub fn has_identifier_token(tokens: &[String]) -> bool {
    tokens.iter().any(|t| looks_like_identifier(t))
}

/// A trailing plural `s` dropped, lowercased: `notifications` meets
/// `Notification`. Deliberately that much and no more — a real stemmer on
/// identifiers mangles more names than it joins.
pub fn english_stem(token: &str) -> String {
    let lower = token.to_ascii_lowercase();
    if lower.len() >= 5 && lower.ends_with('s') && !lower.ends_with("ss") {
        return lower[..lower.len() - 1].to_string();
    }
    lower
}

/// Whether a directory or file name in `relative_path` is, or contains, the
/// token — `workspace_scanner` finds `src-tauri/src/infra/workspace_scanner.rs`.
pub fn path_segment_matches(relative_path: &str, token: &str) -> bool {
    let token_lower = token.to_ascii_lowercase();
    let stem = english_stem(token);
    relative_path.split('/').any(|segment| {
        let segment = segment.to_ascii_lowercase();
        let file_stem = segment.rsplit_once('.').map_or(segment.as_str(), |(stem, _)| stem);
        file_stem == token_lower
            || (token_lower.len() >= MIN_STEM_LEN && segment.contains(&token_lower))
            || (stem.len() >= MIN_STEM_LEN && segment.contains(&stem))
    })
}

/// Which files a search may return — `grep` and `semanticSearch` both, so the
/// two read a pattern the same way. `glob` matches the file's *name*
/// (`*.java`), or its path when it has a `/` in it (`src/main/**`); `exclude`
/// always matches the path (`src/docs/**`).
#[derive(Debug, Clone, Default)]
pub struct PathFilter {
    include: Option<(globset::GlobMatcher, bool)>,
    exclude: Option<globset::GlobMatcher>,
}

impl PathFilter {
    /// An empty or missing pattern filters nothing. `Err` carries the glob
    /// parser's message for a pattern that does not compile.
    pub fn new(glob: Option<&str>, exclude: Option<&str>) -> Result<Self, String> {
        let compile = |pattern: Option<&str>| match pattern {
            Some(p) if !p.is_empty() => {
                globset::Glob::new(p).map(|g| Some(g.compile_matcher())).map_err(|e| e.to_string())
            }
            _ => Ok(None),
        };
        let on_path = glob.is_some_and(|g| g.contains('/'));
        Ok(Self { include: compile(glob)?.map(|m| (m, on_path)), exclude: compile(exclude)? })
    }

    pub fn is_empty(&self) -> bool {
        self.include.is_none() && self.exclude.is_none()
    }

    /// Whether `relative_path` (`/`-separated, from the workspace root) passes.
    pub fn allows(&self, relative_path: &str) -> bool {
        if let Some((glob, on_path)) = &self.include {
            let name = relative_path.rsplit('/').next().unwrap_or(relative_path);
            if !glob.is_match(if *on_path { relative_path } else { name }) {
                return false;
            }
        }
        !self.exclude.as_ref().is_some_and(|exclude| exclude.is_match(relative_path))
    }
}

/// Folders whose files are documentation whatever they are written in —
/// `src/docs/asciidoc/…/x.puml` is a diagram of the docs, not code.
const DOC_DIRS: &[&str] = &["docs", "doc", "documentation", "wiki"];
/// Prose by extension, wherever it lies: a README, a skill, a design note.
const DOC_EXTENSIONS: &[&str] = &["md", "markdown", "mdx", "adoc", "asciidoc", "rst", "txt"];

/// Whether a file is documentation rather than code, for leaving it out of a
/// code search. By path, not by language: files without a parser of their
/// own (`build.gradle`, `pom.xml`) are plain text to the indexer and code to
/// everyone else.
pub fn is_documentation(relative_path: &str) -> bool {
    let lower = relative_path.to_ascii_lowercase();
    let mut segments: Vec<&str> = lower.split('/').collect();
    let file = segments.pop().unwrap_or_default();
    let extension = file.rsplit_once('.').map(|(_, ext)| ext).unwrap_or_default();
    DOC_EXTENSIONS.contains(&extension) || segments.iter().any(|dir| DOC_DIRS.contains(dir))
}

#[derive(Debug, Clone, Copy)]
pub struct SearchMetaInput<'a> {
    pub match_count: usize,
    pub symbol_hits: u32,
    pub has_semantic: bool,
    pub only_lexical: bool,
    pub extracted_tokens: &'a [String],
}

/// Whether the result is weak, and what the model should change about the
/// query. The hint names a *different query*, never "try again": re-running a
/// search that came back thin returns the same thin list.
pub fn weak_search_hint(input: SearchMetaInput<'_>) -> (bool, Option<&'static str>) {
    if input.match_count == 0 {
        return (
            true,
            Some("Nothing found. Search again with names as the code spells them (a function, a type, a file), or with other words for the same idea."),
        );
    }
    if input.extracted_tokens.is_empty() {
        return (
            true,
            Some("The query has no names from code in it. Add an identifier as it appears in the source (getUser, UserService, user_store) to narrow it."),
        );
    }
    if input.only_lexical && !input.has_semantic && input.symbol_hits == 0 {
        return (
            true,
            Some("These matched on text alone; no function, type or file name matched. Name one in the query, or search again once the semantic index has finished building."),
        );
    }
    if !has_identifier_token(input.extracted_tokens) {
        return (
            false,
            Some("To narrow the next search, add a function or type name taken from these results."),
        );
    }
    (false, None)
}

/// The words of a query worth asking the index whether it has them: long
/// enough for the prefix match of the word ranking ([`fts5_query`]) to be
/// specific. At three letters a prefix is in every index.
pub fn query_words(query: &str) -> Vec<&str> {
    query.split(|c: char| !c.is_alphanumeric()).filter(|w| w.chars().count() >= FTS_PREFIX_MIN_CHARS).collect()
}

/// Whether the query is about something the workspace does not have: two of
/// its words or more, and a third of them or more, are in no indexed passage.
///
/// The ranking returns its nearest passages whatever the query, and by
/// similarity alone "kubernetes helm chart" and a real question look the same
/// (the bench's unanswerable questions scored 0.23–0.54, real answers from
/// 0.32). Words the workspace lacks tell them apart: `kubernetes`, `helm`,
/// `stripe` are nowhere in a Java service. One missing word is not enough —
/// "which" or "stored" is missing from code without comments.
pub fn absent_from_workspace(unknown: usize, words: usize) -> bool {
    unknown >= 2 && unknown * 3 >= words
}

/// Runs of characters that can make up an identifier or a path.
fn split_raw_segments(query: &str) -> Vec<String> {
    query
        .split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '_' | '.')))
        .map(|s| s.trim_matches('.'))
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// `src/lib/chat.ts`, `tokens.css`, `workspace-scanner`. Not `snake_case`:
/// that is an identifier, and [`looks_like_identifier`] is asked first.
fn is_path_like(s: &str) -> bool {
    s.contains(['/', '.', '-'])
}

fn path_stem(s: &str) -> Option<&str> {
    let name = s.rsplit('/').next().unwrap_or(s);
    let (stem, ext) = name.rsplit_once('.')?;
    (!stem.is_empty() && !ext.is_empty() && ext.chars().all(|c| c.is_ascii_alphabetic())).then_some(stem)
}

/// A name as code spells it: `camelCase`, `PascalCase` or `snake_case`.
///
/// Upstream recognised only the first two, which was right for Java and left
/// every Rust and Python name — `compact_if_needed`, `read_source` — reading
/// as "no identifiers in the query", with a hint telling the model to add one.
/// `ALLCAPS` and `Title` alone are not identifiers: they are how prose
/// capitalises too.
pub fn looks_like_identifier(s: &str) -> bool {
    if s.len() < 2 || !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return false;
    }
    if s.contains('_') {
        let parts: Vec<&str> = s.split('_').filter(|p| !p.is_empty()).collect();
        return parts.len() >= 2 && parts.iter().all(|p| p.chars().any(|c| c.is_ascii_alphabetic()));
    }
    let mut chars = s.chars();
    let first = chars.next().unwrap_or(' ');
    let rest: Vec<char> = chars.collect();
    if first.is_ascii_lowercase() {
        return rest.iter().any(char::is_ascii_uppercase);
    }
    first.is_ascii_uppercase()
        && rest.iter().any(char::is_ascii_lowercase)
        && rest.iter().any(char::is_ascii_uppercase)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rules `grep` and `semanticSearch` share: a glob without `/` is a
    /// file name at any depth, with one it is a path; `exclude` is a path.
    #[test]
    fn a_path_filter_reads_names_paths_and_exclusions() {
        let only = |glob: Option<&str>, exclude: Option<&str>, paths: &[&str]| -> Vec<String> {
            let filter = PathFilter::new(glob, exclude).unwrap();
            paths.iter().filter(|p| filter.allows(p)).map(|p| p.to_string()).collect()
        };
        let paths = ["src/main/A.java", "src/test/B.java", "src/docs/c.adoc", "build.gradle"];
        assert_eq!(only(Some("*.java"), None, &paths), ["src/main/A.java", "src/test/B.java"]);
        assert_eq!(only(Some("src/main/**"), None, &paths), ["src/main/A.java"]);
        assert_eq!(only(None, Some("src/docs/**"), &paths), ["src/main/A.java", "src/test/B.java", "build.gradle"]);
        assert_eq!(only(Some("*.java"), Some("src/test/**"), &paths), ["src/main/A.java"]);
        assert!(PathFilter::new(Some(""), None).unwrap().is_empty());
        assert!(PathFilter::new(Some("["), None).is_err());
    }

    #[test]
    fn documentation_is_a_docs_folder_or_prose_anywhere() {
        for docs in [
            "docs/01-vision.md",
            "src/docs/asciidoc/sendToKalugaJob/sendToKalugaJob.puml",
            "doc/releasing.md",
            "README.md",
            ".agents/skills/controller-tests/SKILL.md",
            "notes/design.adoc",
        ] {
            assert!(is_documentation(docs), "{docs}");
        }
        // `scripts/docs` is a script called docs, not a folder of them.
        for code in ["src/main/java/A.java", "build.gradle", "src/docs.rs", "docker/Dockerfile", "src/doc_parser/mod.rs", "scripts/docs"] {
            assert!(!is_documentation(code), "{code}");
        }
    }

    // ---------------------------------------------------------- fts5_query

    #[test]
    fn every_word_becomes_a_quoted_term_joined_by_or() {
        assert_eq!(fts5_query("use the api").as_deref(), Some("\"use\" OR \"the\" OR \"api\""));
    }

    /// Long terms become prefixes of at most six characters, so an inflected
    /// Russian word meets its other forms.
    #[test]
    fn a_long_word_is_searched_by_its_prefix() {
        assert_eq!(fts5_query("уведомления").as_deref(), Some("\"уведом\"*"));
        assert_eq!(fts5_query("sync").as_deref(), Some("\"sync\"*"));
        assert_eq!(fts5_query("index").as_deref(), Some("\"index\"*"));
    }

    /// Split the way the index's tokenizer split the text, or an identifier
    /// in the query is one term the index never produced.
    #[test]
    fn an_identifier_splits_into_the_words_the_index_holds() {
        assert_eq!(
            fts5_query("compact_if_needed").as_deref(),
            Some("\"compac\"* OR \"if\" OR \"needed\"*")
        );
    }

    #[test]
    fn a_camel_case_word_is_also_the_phrase_its_snake_case_spelling_is() {
        assert_eq!(fts5_query("readSource").as_deref(), Some("\"read source\" OR \"readso\"*"));
        assert_eq!(camel_parts("HTTPServer"), ["HTTP", "Server"]);
        assert_eq!(camel_parts("parseV2Config"), ["parse", "V2", "Config"]);
        assert!(camel_parts("plain").is_empty() && camel_parts("ALLCAPS").is_empty());
    }

    /// FTS5 syntax typed into a query is text. Unquoted, `NOT` would be an
    /// operator and a stray `"` a syntax error from SQLite.
    #[test]
    fn query_syntax_is_neutralised() {
        let query = fts5_query("text:foo NOT \"bar* baz").unwrap();
        assert_eq!(query, "\"text\"* OR \"foo\" OR \"not\" OR \"bar\" OR \"baz\"");
    }

    #[test]
    fn a_query_of_nothing_searchable_is_none() {
        assert_eq!(fts5_query("a ? !"), None);
        assert_eq!(fts5_query(""), None);
    }

    #[test]
    fn repeated_and_excess_terms_are_dropped() {
        assert_eq!(fts5_query("Sync sync SYNC").as_deref(), Some("\"sync\"*"));
        let long: String = (0..40).map(|i| format!("w{i:02} ")).collect();
        assert_eq!(fts5_query(&long).unwrap().matches(" OR ").count(), MAX_FTS_TERMS - 1);
    }

    // ------------------------------------------------------ extract tokens

    #[test]
    fn identifiers_come_first_then_paths_then_words() {
        let tokens = extract_search_tokens("how does compact_if_needed use src/lib/chat.ts and ContextUsage");
        assert_eq!(tokens[..2], ["compact_if_needed", "ContextUsage"]);
        assert!(tokens.contains(&"src/lib/chat.ts".to_string()));
        assert!(tokens.contains(&"chat".to_string()), "the file's stem is searched too");
        assert!(tokens.iter().position(|t| t == "src/lib/chat.ts") < tokens.iter().position(|t| t == "how"));
    }

    /// The fix over upstream: a Rust or Python name is a name.
    #[test]
    fn snake_case_is_an_identifier() {
        for name in ["compact_if_needed", "read_source", "MAX_FILE_BYTES", "_private_fn"] {
            assert!(looks_like_identifier(name), "{name}");
        }
        for word in ["under_", "a_1", "Title", "HTTP", "plain"] {
            assert!(!looks_like_identifier(word), "{word}");
        }
        assert!(looks_like_identifier("getUser") && looks_like_identifier("UserService"));
    }

    #[test]
    fn non_ascii_and_short_words_are_not_code_tokens() {
        assert!(extract_search_tokens("как работает id").is_empty());
    }

    #[test]
    fn tokens_are_deduplicated_ignoring_case() {
        assert_eq!(extract_search_tokens("UserService userservice").len(), 1);
    }

    // ------------------------------------------------------------ matching

    #[test]
    fn a_plural_s_is_the_only_thing_stemmed() {
        assert_eq!(english_stem("notifications"), "notification");
        assert_eq!(english_stem("class"), "class");
        assert_eq!(english_stem("uses"), "uses", "too short to trust");
    }

    #[test]
    fn a_path_matches_by_directory_file_name_or_stem() {
        assert!(path_segment_matches("src-tauri/src/infra/workspace_scanner.rs", "workspace_scanner"));
        assert!(path_segment_matches("src-tauri/src/infra/workspace_scanner.rs", "infra"));
        assert!(path_segment_matches("src/main/java/CollectNotificationService.java", "notifications"));
        assert!(!path_segment_matches("src/foo/Bar.java", "xyz"));
        assert!(!path_segment_matches("src/lib/chat.ts", "at"), "a short token is not a substring search");
    }

    // ---------------------------------------------------------------- hint

    fn verdict(match_count: usize, symbol_hits: u32, has_semantic: bool, only_lexical: bool, tokens: &[&str]) -> (bool, Option<&'static str>) {
        let tokens: Vec<String> = tokens.iter().map(|t| t.to_string()).collect();
        weak_search_hint(SearchMetaInput { match_count, symbol_hits, has_semantic, only_lexical, extracted_tokens: &tokens })
    }

    #[test]
    fn nothing_found_is_weak() {
        let (weak, hint) = verdict(0, 0, true, false, &["UserService"]);
        assert!(weak);
        assert!(hint.unwrap().starts_with("Nothing found."));
    }

    #[test]
    fn a_query_with_no_code_words_is_weak() {
        let (weak, hint) = verdict(3, 0, true, false, &[]);
        assert!(weak);
        assert!(hint.unwrap().contains("no names from code"));
    }

    #[test]
    fn text_only_hits_with_no_semantic_tier_are_weak() {
        let (weak, hint) = verdict(3, 0, false, true, &["session"]);
        assert!(weak);
        assert!(hint.unwrap().contains("text alone"));
    }

    /// Not weak, just a nudge: the hits are real, the next query can be
    /// sharper.
    #[test]
    fn plain_words_that_hit_get_a_nudge_not_a_verdict() {
        let (weak, hint) = verdict(3, 1, true, false, &["session"]);
        assert!(!weak);
        assert!(hint.unwrap().contains("add a function or type name"));
    }

    #[test]
    fn a_named_query_that_hit_needs_no_hint() {
        assert_eq!(verdict(3, 1, true, false, &["read_source"]), (false, None));
    }

    #[test]
    fn words_worth_asking_about_are_four_letters_or_more_in_any_script() {
        assert_eq!(query_words("the kubernetes helm, пул соединений"), ["kubernetes", "helm", "соединений"]);
    }

    /// Two missing words and a third of the query; one is noise.
    #[test]
    fn a_workspace_lacks_the_topic_when_enough_of_its_words_are_missing() {
        assert!(absent_from_workspace(2, 6));
        assert!(!absent_from_workspace(2, 7), "under a third");
        assert!(!absent_from_workspace(1, 1), "one word is not enough");
    }

    /// Every string the model reads is English (port plan, "service strings").
    #[test]
    fn hints_are_english() {
        for input in [(0, 0, true, false, vec!["X"]), (3, 0, true, false, vec![]), (3, 0, false, true, vec!["x"]), (3, 1, true, false, vec!["x"])] {
            let tokens: Vec<&str> = input.4.clone();
            let (_, hint) = verdict(input.0, input.1, input.2, input.3, &tokens);
            assert!(hint.unwrap().is_ascii(), "{hint:?}");
        }
    }
}
