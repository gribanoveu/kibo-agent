//! What the index knows about one file: which language it is, what named
//! things are in it, and enough about its bytes to tell a rebuild from a
//! no-op.
//!
//! No content here. A repository of thousands of files would otherwise hold a
//! second copy of the working tree in memory before a single chunk exists. The
//! index describes the project; it does not duplicate the filesystem.
//!
//! ## One difference from Alfa Atlas, and it is the point of the layer
//!
//! Upstream's [`detect_language`] returns `Option<Language>`, and
//! `RepositoryIndex::update_file` **deletes the file from the index** when it
//! comes back `None`. With five languages — AsciiDoc, Markdown, Java, JSON,
//! YAML — that quietly drops `.txt`, `.puml` and `.mmd` from search while
//! leaving them visible to `grep`.
//!
//! Here it would drop almost everything: this repository is 56 `.rs`, 22
//! `.tsx`, 22 `.ts` and no `.java` at all. So the function returns a
//! `Language`, never an `Option`: a file nobody wrote a parser for is
//! [`Language::PlainText`] and gets whole-file chunks. The contract is one
//! sentence — **a file the scan accepted gets chunks** — and it is enforced by
//! the return type rather than by every caller remembering it.

use thiserror::Error;

/// Bumped when an indexer starts producing different symbols for the same
/// bytes. Nothing rebuilds on it yet; it exists so the on-disk index has a
/// cheap staleness signal from the first day rather than after the indexers
/// have already drifted.
pub const INDEX_VERSION: u32 = 1;

/// What the index knows how to read.
///
/// Three groups, and the difference between them is what a parser buys:
///
/// * [`PlainText`](Language::PlainText) — no parser. Whole-file chunks. Not a
///   failure mode: a real language of the index, and the one most files are.
/// * [`Json`](Language::Json) and [`Yaml`](Language::Yaml) — no parser either,
///   and chunked the same way, but named. "This is configuration, not prose"
///   is worth knowing at query time even when the bytes are handled alike.
/// * The rest — a tree-sitter grammar, so chunks follow functions and headings
///   instead of a byte count.
///
/// ## Where the grammars come from
///
/// The official grammar crates, one dependency each, read by a single generic
/// tree walk in `infra::language_indexers`. A language is a crate plus a row
/// naming the nodes worth keeping.
///
/// Upstream could not do that. Its set is Java, JSON, YAML, Markdown and
/// AsciiDoc, and it had to drop Kotlin: `tree-sitter` declared
/// `links = "tree-sitter"`, which allows one version of it in the entire build
/// graph, and `tree-sitter-asciidoc` needed a newer runtime than
/// `tree-sitter-kotlin` permitted. **That constraint is gone.** Current
/// grammars depend on the small `tree-sitter-language` crate and declare no
/// `links` at all — checked 2026-09-17 against `tree-sitter-rust` 0.24.2 and
/// `tree-sitter-typescript` 0.23.2, both of which do.
///
/// A bundle such as `ast-grep-language` was weighed and passed over. It exists
/// to solve exactly the conflict above, which no longer happens; what it costs
/// is `ast_grep_core` — a pattern matcher, an AST wrapper and meta-variables —
/// in the build graph to obtain a `tree_sitter::Language` that a grammar crate
/// hands over in one line, and a feature flag whose absence is
/// `unimplemented!()` at runtime rather than an error at compile time. Revisit
/// it if this list ever grows past a dozen languages, where curating versions
/// by hand becomes real work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Language {
    /// Anything without a parser of its own.
    PlainText,
    Json,
    Yaml,
    Markdown,
    Rust,
    TypeScript,
    /// Its own grammar rather than a flag on TypeScript: `.tsx` does not parse
    /// with the TypeScript grammar, and `tree-sitter-typescript` ships the two
    /// as separate languages for that reason.
    Tsx,
    JavaScript,
    Python,
    Go,
    /// Spring on the back, React on the front is what an enterprise repository
    /// usually is, and half of it would otherwise be chunked by byte count.
    Java,
}

impl Language {
    /// Every variant, for tests that need to visit each one. The two places
    /// that map a language to behaviour — `infra::language_indexers::indexer_for`
    /// and `chunk_index::spans_for` — are exhaustive `match`es, so a new
    /// variant cannot be forgotten there; this list can, which is what
    /// `all_is_complete` is for.
    pub const ALL: &'static [Language] = &[
        Language::PlainText,
        Language::Json,
        Language::Yaml,
        Language::Markdown,
        Language::Rust,
        Language::TypeScript,
        Language::Tsx,
        Language::JavaScript,
        Language::Python,
        Language::Go,
        Language::Java,
    ];
}

/// The file's extension, lowercased, with its dot — `""` when it has none, and
/// for a dotfile like `.gitignore`, whose leading dot is not an extension.
pub fn extension_of(path: &str) -> String {
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let Some(dot) = base.rfind('.') else {
        return String::new();
    };
    if dot == 0 {
        return String::new();
    }
    base[dot..].to_ascii_lowercase()
}

/// Which parser reads this file. Never "none": see the module docs.
pub fn detect_language(path: &str) -> Language {
    match extension_of(path).as_str() {
        ".md" | ".markdown" => Language::Markdown,
        ".json" => Language::Json,
        ".yaml" | ".yml" => Language::Yaml,
        ".rs" => Language::Rust,
        ".ts" | ".mts" | ".cts" => Language::TypeScript,
        ".tsx" => Language::Tsx,
        // `.jsx` too: the JavaScript grammar handles JSX, and a separate
        // variant for it would be a second name for the same parser.
        ".js" | ".mjs" | ".cjs" | ".jsx" => Language::JavaScript,
        ".py" | ".pyi" => Language::Python,
        ".go" => Language::Go,
        ".java" => Language::Java,
        _ => Language::PlainText,
    }
}

/// A named, ranged thing an indexer found in a file — a heading, a function, a
/// type, a method.
///
/// Carries both line and byte ranges because a parser hands over both anyway,
/// and throwing one away means deriving it again later for chunking, for
/// highlighting, or for "go to symbol".
///
/// There is no `kind` field. Upstream has one with six variants, and nothing
/// downstream branches on it: the chunker cuts at a range and the search
/// weights a name, whatever declared them. An enum nobody matches on is a
/// column to keep in step with every grammar for no reader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub start_line: u32,
    pub end_line: u32,
    pub start_byte: u32,
    pub end_byte: u32,
}

/// Repo-relative path, `/`-separated. A newtype so the key files are stored
/// under is not just another path-shaped `String`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FileId(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMetadata {
    pub relative_path: String,
    pub size_bytes: u64,
    pub modified_at: std::time::SystemTime,
    pub hash: blake3::Hash,
    pub language: Language,
}

/// One file's structural record — metadata and what was found in it, never its
/// content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedFile {
    pub metadata: FileMetadata,
    pub symbols: Vec<Symbol>,
}

#[derive(Debug, Error)]
pub enum RepoIndexError {
    #[error("io error: {0}")]
    Io(#[source] std::io::Error),
    #[error("{0}")]
    Message(String),
}

/// One language's reading of a file.
///
/// Infallible on purpose. A file that reads from disk but is malformed for its
/// language must still land in the index with real metadata and a real hash;
/// finding no symbols is the only failure mode there is. An `Err` here would
/// mean a file disappearing from search because its syntax was broken — which
/// is exactly when someone is most likely to search for it.
///
/// Implementors do not report which language they are: that mapping lives in
/// `infra::language_indexers` and nowhere else.
pub trait LanguageIndexer: Send + Sync {
    fn index(&self, content: &str) -> Vec<Symbol>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_extension_off_a_path() {
        assert_eq!(extension_of("src/main.rs"), ".rs");
        assert_eq!(extension_of("src\\main.RS"), ".rs");
        assert_eq!(extension_of("README"), "");
        assert_eq!(extension_of("archive.tar.gz"), ".gz");
    }

    /// A dotfile's leading dot is not an extension: `.gitignore` is a whole
    /// name, and reading it as one would make every dotfile a different
    /// "language".
    #[test]
    fn a_dotfile_has_no_extension() {
        assert_eq!(extension_of(".gitignore"), "");
        assert_eq!(extension_of("src/.env"), "");
        // ...but a dotfile that really does have one keeps it.
        assert_eq!(extension_of(".eslintrc.json"), ".json");
    }

    #[test]
    fn detects_the_languages_it_has_a_name_for() {
        for (path, expected) in [
            ("README.md", Language::Markdown),
            ("README.markdown", Language::Markdown),
            ("schema.json", Language::Json),
            ("config.yaml", Language::Yaml),
            ("config.yml", Language::Yaml),
            ("src/main.rs", Language::Rust),
            ("src/lib/chat.ts", Language::TypeScript),
            ("src/App.tsx", Language::Tsx),
            ("vite.config.js", Language::JavaScript),
            ("scripts/build.mjs", Language::JavaScript),
            ("components/Card.jsx", Language::JavaScript),
            ("scripts/build-embedding-model.py", Language::Python),
            ("cmd/main.go", Language::Go),
            ("src/main/java/com/acme/UserService.java", Language::Java),
        ] {
            assert_eq!(detect_language(path), expected, "{path}");
        }
    }

    /// `.tsx` does not parse with the TypeScript grammar. Folding the two
    /// together would give every React component a tree full of errors and no
    /// symbols — which looks exactly like a file with nothing in it.
    #[test]
    fn tsx_is_not_typescript() {
        assert_ne!(detect_language("App.tsx"), detect_language("chat.ts"));
    }

    /// The contract this module exists for. Upstream answers `None` here and
    /// deletes the file from the index; there is no `None` to answer with.
    #[test]
    fn everything_else_is_plain_text_rather_than_nothing() {
        for path in [
            "styles/tokens.css",
            "notes.txt",
            "flow.puml",
            "Cargo.toml",
            "Makefile",
            "Dockerfile",
            ".gitignore",
            "query.sql",
            "index.html",
        ] {
            assert_eq!(
                detect_language(path),
                Language::PlainText,
                "{path} must still be indexable"
            );
        }
    }

    /// A variant added without being listed leaves both registries'
    /// completeness tests checking a subset of the enum.
    #[test]
    fn all_is_complete() {
        assert_eq!(
            Language::ALL.len(),
            11,
            "a language was added or removed — update ALL and this count together"
        );
        let unique: std::collections::HashSet<_> = Language::ALL.iter().collect();
        assert_eq!(unique.len(), Language::ALL.len(), "duplicate entry in ALL");
    }

    /// Every language the detector can produce has to be one the registries
    /// will be asked about.
    #[test]
    fn every_detectable_language_is_listed() {
        for path in ["a.md", "a.json", "a.yaml", "a.rs", "a.ts", "a.tsx", "a.js", "a.py", "a.go", "a.java", "a.txt"] {
            assert!(Language::ALL.contains(&detect_language(path)));
        }
    }
}
