//! The two places the index touches file bytes: reading a file to be
//! chunked, and reading a chunk's text back out of it later.
//!
//! Both refuse rather than guess. A file the index will not take says why, in
//! a form the caller can count and show ("3 skipped: 2 too large, 1 binary"),
//! because a file that silently is not there is indistinguishable from a
//! search that failed.
//! A chunk whose file has changed since it was cut says so, rather than
//! serving whatever bytes now sit at its old offsets.
//!
//! Upstream put a 64 MiB `moka` cache in front of [`resolve_chunk`]. Not
//! ported: nothing reads chunk text in a loop yet. It goes back in when a
//! profile of search on a real repository says the disk read is the cost.

use std::fs;
use std::io;
use std::path::Path;

use thiserror::Error;

use crate::domain::chunk_index::ChunkMetadata;

/// A NUL in the first block means binary — the same sniff the `grep` tool
/// uses, so the two agree on what is text.
const BINARY_SNIFF_BYTES: usize = 8_192;

/// Why a file is not in the index. Each is an ordinary state of a working
/// tree, not a fault, and each has to reach the user as a reason.
#[derive(Debug, Error)]
pub enum SourceSkip {
    #[error("too large: {size} bytes, the limit is {limit}")]
    TooLarge { size: u64, limit: u64 },
    #[error("binary")]
    Binary,
    /// Text in some other encoding. Chunk offsets are byte offsets into the
    /// file; decoding it lossily would make them offsets into something else.
    #[error("not UTF-8")]
    NotUtf8,
    #[error("unreadable: {0}")]
    Io(#[source] io::Error),
}

/// The file's text, if the index should have it.
///
/// The size is checked **before** reading: a 200 MiB dump is refused by a
/// `stat`, not after it is in memory. And again after, because a file can grow
/// between the two calls and the limit is a promise about memory.
pub fn read_source(path: &Path, max_file_bytes: u64) -> Result<String, SourceSkip> {
    let size = fs::metadata(path).map_err(SourceSkip::Io)?.len();
    if size > max_file_bytes {
        return Err(SourceSkip::TooLarge { size, limit: max_file_bytes });
    }
    let bytes = fs::read(path).map_err(SourceSkip::Io)?;
    if bytes.len() as u64 > max_file_bytes {
        return Err(SourceSkip::TooLarge { size: bytes.len() as u64, limit: max_file_bytes });
    }
    if bytes[..BINARY_SNIFF_BYTES.min(bytes.len())].contains(&0) {
        return Err(SourceSkip::Binary);
    }
    String::from_utf8(bytes).map_err(|_| SourceSkip::NotUtf8)
}

#[derive(Debug, Error)]
pub enum ChunkTextError {
    #[error("unreadable: {0}")]
    Io(#[source] io::Error),
    /// The file no longer hashes to what the chunk was cut from. The caller
    /// decides — drop the hit, queue a resync; this module never trusts an
    /// offset against content it was not computed from.
    #[error("changed since it was indexed: {0}")]
    Stale(String),
    /// Unreachable while the hash matches; kept so a mismatch that slipped
    /// past it is an error and not a panic in a slice.
    #[error("chunk range out of bounds: {0}")]
    OutOfBounds(String),
}

/// A chunk's text as it is on disk, and the lines it spans — what a search
/// hands the model, which opens files by line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedChunk {
    pub text: String,
    /// 1-based, inclusive.
    pub start_line: u32,
    /// 1-based, inclusive: the line of the chunk's last character, not the
    /// empty one after its final newline.
    pub end_line: u32,
}

/// `[start_byte..end_byte)` of the chunk's file, under `repo_root`, provided
/// the file is still the one the chunk was cut from.
pub fn resolve_chunk(repo_root: &Path, metadata: &ChunkMetadata) -> Result<ResolvedChunk, ChunkTextError> {
    let content = fs::read(repo_root.join(&metadata.file_id.0)).map_err(ChunkTextError::Io)?;
    if blake3::hash(&content) != metadata.file_hash {
        return Err(ChunkTextError::Stale(metadata.file_id.0.clone()));
    }
    let (start, end) = (metadata.start_byte as usize, metadata.end_byte as usize);
    let text = content
        .get(start..end)
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .ok_or_else(|| ChunkTextError::OutOfBounds(metadata.id.0.clone()))?;
    let lines_in = |bytes: &[u8]| bytes.iter().filter(|&&b| b == b'\n').count() as u32;
    // A chunk cut after the previous declaration's `}` begins with the rest
    // of that line; its lines start where its own text does.
    let body = text.trim_start_matches(['\r', '\n']);
    let start_line = 1 + lines_in(&content[..end - body.len()]);
    let end_line = start_line + lines_in(body.strip_suffix('\n').unwrap_or(body).as_bytes());
    Ok(ResolvedChunk { text: text.to_string(), start_line, end_line })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::chunk_index::{build_chunks, ChunkBuildOptions};
    use crate::domain::repo_index::{detect_language, FileId};
    use crate::infra::language_indexers::indexer_for;
    use crate::testing::temp_dir;

    // ---------------------------------------------------------- read_source

    #[test]
    fn text_under_the_limit_is_read() {
        let dir = temp_dir("source-ok");
        fs::write(dir.join("a.rs"), "fn main() {}\n").unwrap();
        assert_eq!(read_source(&dir.join("a.rs"), 1024).unwrap(), "fn main() {}\n");
    }

    /// The reason the limit exists and the reason it says how big.
    #[test]
    fn a_file_over_the_limit_is_refused_with_its_size() {
        let dir = temp_dir("source-large");
        fs::write(dir.join("bundle.js"), "x".repeat(2_000)).unwrap();

        let skip = read_source(&dir.join("bundle.js"), 1_000).unwrap_err();
        assert!(matches!(skip, SourceSkip::TooLarge { size: 2_000, limit: 1_000 }), "{skip:?}");
        assert_eq!(skip.to_string(), "too large: 2000 bytes, the limit is 1000");
    }

    /// Exactly at the limit is in: the limit is the largest file taken.
    #[test]
    fn a_file_exactly_at_the_limit_is_read() {
        let dir = temp_dir("source-edge");
        fs::write(dir.join("a.txt"), "x".repeat(1_000)).unwrap();
        assert!(read_source(&dir.join("a.txt"), 1_000).is_ok());
    }

    /// Before, not only after: a file nobody may read but anyone may `stat`
    /// tells the two apart. Checked after reading, the refusal would be
    /// "unreadable" — and for a readable file, it would come with the whole
    /// file already in memory.
    #[cfg(unix)]
    #[test]
    fn the_limit_is_checked_before_the_file_is_read() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("source-before");
        let path = dir.join("dump.sql");
        fs::write(&path, "x".repeat(2_000)).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();

        let skip = read_source(&path, 1_000);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(skip, Err(SourceSkip::TooLarge { .. })), "{skip:?}");
    }

    #[test]
    fn a_binary_file_is_refused() {
        let dir = temp_dir("source-binary");
        fs::write(dir.join("logo.png"), b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR").unwrap();
        assert!(matches!(read_source(&dir.join("logo.png"), 1024), Err(SourceSkip::Binary)));
    }

    /// Latin-1 has no NUL and is not UTF-8. Decoded lossily, every offset
    /// after the first accented letter would point somewhere else.
    #[test]
    fn text_in_another_encoding_is_refused_not_mangled() {
        let dir = temp_dir("source-latin1");
        fs::write(dir.join("old.txt"), b"caf\xe9\n").unwrap();
        assert!(matches!(read_source(&dir.join("old.txt"), 1024), Err(SourceSkip::NotUtf8)));
    }

    #[test]
    fn a_missing_file_is_unreadable() {
        let dir = temp_dir("source-missing");
        assert!(matches!(read_source(&dir.join("gone.rs"), 1024), Err(SourceSkip::Io(_))));
    }

    // -------------------------------------------------------- resolve_chunk

    fn chunks_of(dir: &Path, path: &str, content: &str) -> Vec<crate::domain::chunk_index::Chunk> {
        fs::write(dir.join(path), content).unwrap();
        let language = detect_language(path);
        let symbols = indexer_for(language).index(content);
        build_chunks(&FileId(path.into()), language, content, &symbols, &ChunkBuildOptions::default())
    }

    #[test]
    fn a_chunk_reads_back_as_the_text_it_was_cut_with() {
        let dir = temp_dir("resolve-ok");
        let chunks = chunks_of(&dir, "a.rs", "fn one() {}\nfn two() {}\n");
        for chunk in &chunks {
            assert_eq!(resolve_chunk(&dir, &chunk.metadata).unwrap().text, chunk.text);
        }
    }

    #[test]
    fn a_chunk_knows_the_lines_it_spans() {
        let dir = temp_dir("resolve-lines");
        let chunks = chunks_of(&dir, "a.rs", "fn one() {}\n\nfn two() {\n    2\n}\n");
        let lines: Vec<_> = chunks
            .iter()
            .map(|c| resolve_chunk(&dir, &c.metadata).map(|r| (r.start_line, r.end_line)).unwrap())
            .collect();
        // `two`'s chunk begins with the newline after `one`, and the blank
        // line; its lines begin at `fn two`.
        assert_eq!(lines, [(1, 1), (3, 5)]);
    }

    /// Same length, different bytes: the offsets are all still in range, and
    /// only the hash knows the text under them is not the chunk's.
    #[test]
    fn a_changed_file_is_stale_even_when_the_offsets_still_fit() {
        let dir = temp_dir("resolve-stale");
        let chunks = chunks_of(&dir, "a.rs", "fn one() {}\n");
        fs::write(dir.join("a.rs"), "fn uno() {}\n").unwrap();

        assert!(matches!(resolve_chunk(&dir, &chunks[0].metadata), Err(ChunkTextError::Stale(_))));
    }

    #[test]
    fn a_deleted_file_is_unreadable() {
        let dir = temp_dir("resolve-gone");
        let chunks = chunks_of(&dir, "a.rs", "fn one() {}\n");
        fs::remove_file(dir.join("a.rs")).unwrap();

        assert!(matches!(resolve_chunk(&dir, &chunks[0].metadata), Err(ChunkTextError::Io(_))));
    }

    // ---------------------------------------------------------- end to end

    /// A real Spring bean through the real parser: the injected fields and the
    /// class header go with the first method, the annotations with theirs.
    #[test]
    fn a_java_service_is_chunked_by_method() {
        let dir = temp_dir("e2e-java");
        let content = r#"package com.acme;

@Service
public class UserService {
    @Autowired private UserRepository repository;

    @Transactional
    public User find(long id) {
        return repository.findById(id);
    }

    @Transactional
    public void delete(long id) {
        repository.deleteById(id);
    }
}
"#;
        let chunks = chunks_of(&dir, "UserService.java", content);

        let names: Vec<_> = chunks.iter().map(|c| c.metadata.qualified_name.clone().unwrap()).collect();
        assert_eq!(names, ["UserService.find", "UserService.delete"]);
        assert!(chunks[0].text.contains("private UserRepository repository"));
        assert!(chunks[1].text.trim_start().starts_with("@Transactional\n    public void delete"));
        assert_eq!(chunks.iter().map(|c| c.text.as_str()).collect::<String>(), content);
    }

    /// Five of this repository's TypeScript files declare nothing — a config,
    /// an entry point, a test made of `test(...)` calls. They are still found.
    #[test]
    fn a_code_file_with_no_declarations_is_still_one_chunk() {
        let dir = temp_dir("e2e-bare");
        let content = "import { test } from \"bun:test\";\ntest(\"works\", () => {});\n";
        let chunks = chunks_of(&dir, "a.test.ts", content);

        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, content);
    }
}
