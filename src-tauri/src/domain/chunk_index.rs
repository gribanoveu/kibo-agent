//! Cutting an indexed file into addressable pieces.
//!
//! [`spans_for`] decides *where* the cuts go — a kind, a byte range, and which
//! symbol drove it — and [`build_chunks`] turns the ranges into [`Chunk`]s
//! (slicing, hashing, naming, numbering), once, for every language.
//!
//! Both are pure. Upstream kept the strategies in `infra/chunk_strategies/`
//! behind a trait and a `HashMap`, and the builder in a service because it
//! read the file through the repository index; neither needs anything but the
//! text and the symbols, so neither leaves the domain here. Reading the file —
//! the size limit, the binary check — is `services::chunk_text`.
//!
//! No embeddings here and no search. This layer answers "what are the pieces",
//! and nothing about what is done with them.
//!
//! Ported from Alfa Atlas with one defect fixed rather than carried: see
//! [`spans_from_forward_gap_symbols`].

use super::repo_index::{FileId, Language, Symbol};

/// Bumped when the chunking algorithm would reshuffle its output for the same
/// input. Mirrors `repo_index::INDEX_VERSION`, and feeds [`chunk_hash`] so a
/// change invalidates every stored chunk without a separate migration.
pub const CHUNK_VERSION: u32 = 1;

/// There is no tokenizer in this layer. A byte ceiling keeps it independent of
/// any model's tokenizer and is easy to swap for a token limit at the
/// embedding stage, where a tokenizer actually exists.
pub const DEFAULT_MAX_CHUNK_BYTES: usize = 16 * 1024;

/// Past this a file is not indexed at all, and says so. The same ceiling as
/// the `grep` tool's, on purpose: what the agent will not search by pattern
/// it should not find by meaning either. A minified bundle under it would
/// otherwise produce more full-text rows than every hand-written file in the
/// repository together.
pub const DEFAULT_MAX_FILE_BYTES: u64 = 1_048_576;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkBuildOptions {
    pub max_chunk_bytes: usize,
    /// Enforced by `services::chunk_text::read_source` **before** the file is
    /// read, not by the builder after — by then the megabytes are in memory.
    pub max_file_bytes: u64,
}

impl Default for ChunkBuildOptions {
    fn default() -> Self {
        Self {
            max_chunk_bytes: DEFAULT_MAX_CHUNK_BYTES,
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
        }
    }
}

/// `"{file_id}#{start}-{end}"` — stable across rebuilds while a file's byte
/// layout is unchanged, and readable in a log (`src/main.rs#512-983`) where a
/// hash would not be. Whether a chunk is *stale* is a different question,
/// answered by [`ChunkMetadata::hash`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ChunkId(pub String);

/// What a chunk is a piece of.
///
/// Three variants, not upstream's four: its `Method` and `Field` were Java's
/// words for what every code grammar here produces, and nothing downstream
/// tells a method from a function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChunkKind {
    /// Prose under a heading.
    Section,
    /// One innermost declaration in code — a function, a method, a type.
    Declaration,
    /// The whole file: no parser, or a parser that found nothing.
    File,
}

/// A strategy's whole output: a range, and what anchors it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkSpan {
    pub kind: ChunkKind,
    pub start_byte: u32,
    pub end_byte: u32,
    pub anchor_symbol: Option<Symbol>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkMetadata {
    pub id: ChunkId,
    pub file_id: FileId,
    pub language: Language,
    pub kind: ChunkKind,
    pub start_byte: u32,
    pub end_byte: u32,
    /// Which version of the file this came from. Kept here rather than looked
    /// up through the index, because a chunk outlives the index that built it
    /// — once it is a row in a store, "is this still current" has to be
    /// answerable from the chunk alone.
    pub file_hash: blake3::Hash,
    /// `BLAKE3(file_hash ‖ start ‖ end ‖ CHUNK_VERSION)` — deliberately not a
    /// hash of the text. It changes when the file changes *or* when this
    /// span moves, which is exactly when an embedding has to be recomputed,
    /// and it costs nothing to compute from a file hash already in hand.
    pub hash: blake3::Hash,
    /// The heading trail for a section (`Install > macOS`), the enclosing
    /// declarations for code (`UserService.find`); `None` for a whole file.
    pub qualified_name: Option<String>,
    /// 0-based position in this file's final chunk sequence, assigned after
    /// every split.
    pub ordinal: u32,
}

/// The unit search and the model actually work with — and unlike an indexed
/// file, it does carry its text. Keeping capped fragments instead of whole
/// files is the entire point of the layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub metadata: ChunkMetadata,
    pub text: String,
}

/// Prose: a section owns what *follows* its heading, so each span runs from
/// one heading to the next. The first span is pulled back to byte 0 (a
/// document's preamble belongs to its first section) and the last runs to the
/// end of the file. No headings at all means one whole-file span.
///
/// ## The fix this function carries
///
/// Upstream writes `end = anchors[i + 1].start_byte` and trusts that the
/// anchors arrive sorted and non-overlapping. Nothing enforces that: the
/// invariant is held by each indexer's own discipline, and its Java indexer
/// documents deliberately not descending into method bodies for exactly this
/// reason. When it breaks, `end` lands before `start`, and the consumer —
/// which slices `content[start..end]` — used to panic, taking the whole
/// sync worker with it. It now skips the span silently instead, which turned
/// a crash into a file that is indexed in part and searched as if the rest of
/// it were not there.
///
/// So the anchors are put in order here and an anchor that cannot start a span
/// is dropped, which makes a backwards span unconstructible rather than
/// merely unlikely. Discipline in a caller is not a guarantee, and a
/// `debug_assert!` is not one either — it is compiled out of exactly the build
/// where the damage would be silent.
pub fn spans_from_forward_gap_symbols(anchors: &[Symbol], content_len: usize) -> Vec<ChunkSpan> {
    let anchors = ordered_anchors(anchors, content_len);
    if anchors.is_empty() {
        return whole_file_span(content_len);
    }

    let last = anchors.len() - 1;
    anchors
        .iter()
        .enumerate()
        .map(|(i, sym)| ChunkSpan {
            kind: ChunkKind::Section,
            start_byte: if i == 0 { 0 } else { sym.start_byte },
            end_byte: if i == last {
                content_len as u32
            } else {
                anchors[i + 1].start_byte
            },
            anchor_symbol: Some((*sym).clone()),
        })
        .collect()
}

/// Anchors in document order, with anything that cannot start a span removed:
/// one that begins past the end of the content, and one that does not begin
/// strictly after the previous — a nested symbol, or an indexer that emitted
/// its findings out of order.
///
/// Dropping rather than repairing, because a span of zero length is a chunk
/// with no text, and a span that runs backwards is the defect above.
fn ordered_anchors(anchors: &[Symbol], content_len: usize) -> Vec<&Symbol> {
    let mut ordered: Vec<&Symbol> = anchors
        .iter()
        .filter(|sym| (sym.start_byte as usize) < content_len)
        .collect();
    ordered.sort_by_key(|sym| sym.start_byte);

    let mut kept: Vec<&Symbol> = Vec::with_capacity(ordered.len());
    for sym in ordered {
        match kept.last() {
            Some(previous) if sym.start_byte <= previous.start_byte => continue,
            _ => kept.push(sym),
        }
    }
    kept
}

/// Code: one chunk per **innermost** declaration, and each owns the gap
/// *before* it, so a doc comment, an attribute, an annotation or a decorator
/// travels with what it describes. The first chunk is pulled back to byte 0
/// (imports and a class header go with the first member) and the last runs to
/// the end of the file (closing braces go with the last).
///
/// Innermost, because the indexer reports containers too: a class *and* its
/// methods. Cutting at both would give the class a chunk covering its methods'
/// chunks. A container with no members is innermost itself and gets its own.
///
/// The B-1 guard, backward-gap edition: this is the function upstream's
/// `spans_from_backward_gap_symbols` was, and it crashed on a nested anchor.
/// Here anchors are sorted and a symbol is kept only if the next one starts at
/// or after its end. That one test drops containers *and* the earlier of two
/// symbols that cross without nesting (a parser recovering from broken syntax
/// can produce those) — and since everything after the next starts later
/// still, no kept anchor can overlap another. Whatever arrives, the spans are
/// contiguous and forward.
pub fn spans_from_declarations(symbols: &[Symbol], content_len: usize) -> Vec<ChunkSpan> {
    let mut ordered: Vec<&Symbol> = symbols
        .iter()
        .filter(|sym| sym.start_byte < sym.end_byte && (sym.end_byte as usize) <= content_len)
        .collect();
    // Containers before what they contain, so "the next one starts inside
    // me" is the test for "I am a container".
    ordered.sort_by_key(|sym| (sym.start_byte, std::cmp::Reverse(sym.end_byte)));

    let mut anchors: Vec<&Symbol> = Vec::with_capacity(ordered.len());
    for (i, sym) in ordered.iter().enumerate() {
        if ordered.get(i + 1).is_none_or(|next| next.start_byte >= sym.end_byte) {
            anchors.push(sym);
        }
    }
    if anchors.is_empty() {
        return whole_file_span(content_len);
    }

    let last = anchors.len() - 1;
    let mut start = 0;
    anchors
        .iter()
        .enumerate()
        .map(|(i, sym)| {
            let end = if i == last { content_len as u32 } else { sym.end_byte };
            let span = ChunkSpan {
                kind: ChunkKind::Declaration,
                start_byte: start,
                end_byte: end,
                anchor_symbol: Some((*sym).clone()),
            };
            start = end;
            span
        })
        .collect()
}

/// `UserService.find` — the anchor's name after every symbol that strictly
/// encloses it, outermost first. `.` in every language: it is a separator to
/// the full-text tokenizer, so `find` and `UserService` are both words, and a
/// Rust reader recognises `Turn.new` as readily as `Turn::new`.
pub fn qualified_name(anchor: &Symbol, symbols: &[Symbol]) -> String {
    let mut enclosing: Vec<&Symbol> = symbols
        .iter()
        .filter(|sym| {
            sym.start_byte <= anchor.start_byte
                && anchor.end_byte <= sym.end_byte
                && (sym.start_byte, sym.end_byte) != (anchor.start_byte, anchor.end_byte)
        })
        .collect();
    enclosing.sort_by_key(|sym| (sym.start_byte, std::cmp::Reverse(sym.end_byte)));
    enclosing
        .iter()
        .map(|sym| sym.name.as_str())
        .chain(std::iter::once(anchor.name.as_str()))
        .collect::<Vec<_>>()
        .join(".")
}

/// Where the cuts go for `language` — the only place that mapping is written,
/// and a `match` so a new language cannot be forgotten.
pub fn spans_for(language: Language, symbols: &[Symbol], content_len: usize) -> Vec<ChunkSpan> {
    match language {
        Language::PlainText | Language::Json | Language::Yaml => whole_file_span(content_len),
        Language::Markdown => spans_from_forward_gap_symbols(symbols, content_len),
        Language::Rust
        | Language::TypeScript
        | Language::Tsx
        | Language::JavaScript
        | Language::Python
        | Language::Go
        | Language::Java => spans_from_declarations(symbols, content_len),
    }
}

/// A file's text and symbols, as the chunks search and the model work with.
///
/// Covers the file: every byte is in exactly one chunk, in order, whatever
/// the symbols were. A span that does not fall on character boundaries — a
/// symbol stored for other content — is dropped rather than sliced into a
/// panic; the hash check upstream of this makes that unreachable in practice,
/// and a missing piece is the lesser failure than a dead worker.
pub fn build_chunks(
    file_id: &FileId,
    language: Language,
    content: &str,
    symbols: &[Symbol],
    options: &ChunkBuildOptions,
) -> Vec<Chunk> {
    let file_hash = blake3::hash(content.as_bytes());
    let mut sorted = symbols.to_vec();
    sorted.sort_by_key(|sym| sym.start_byte);
    let headings = section_headings(&sorted, content, language);

    let chunks = spans_for(language, &sorted, content.len())
        .into_iter()
        .filter_map(|span| {
            let text = content.get(span.start_byte as usize..span.end_byte as usize)?;
            let qualified_name = span.anchor_symbol.as_ref().and_then(|anchor| match span.kind {
                ChunkKind::Section => section_breadcrumb(anchor, &headings),
                ChunkKind::Declaration => Some(qualified_name(anchor, &sorted)),
                ChunkKind::File => None,
            });
            Some(Chunk {
                metadata: ChunkMetadata {
                    id: ChunkId(format!("{}#{}-{}", file_id.0, span.start_byte, span.end_byte)),
                    file_id: file_id.clone(),
                    language,
                    kind: span.kind,
                    start_byte: span.start_byte,
                    end_byte: span.end_byte,
                    file_hash,
                    hash: chunk_hash(file_hash, span.start_byte, span.end_byte),
                    qualified_name,
                    ordinal: 0,
                },
                text: text.to_string(),
            })
        })
        .collect();

    finalize_ordinals(split_oversized_chunks(chunks, options.max_chunk_bytes))
}

/// One span over everything — for a language with no parser, and for a file
/// whose parser found nothing. An empty file produces no spans: there is
/// nothing to chunk.
pub fn whole_file_span(content_len: usize) -> Vec<ChunkSpan> {
    if content_len == 0 {
        return Vec::new();
    }
    vec![ChunkSpan {
        kind: ChunkKind::File,
        start_byte: 0,
        end_byte: content_len as u32,
        anchor_symbol: None,
    }]
}

/// One heading with the depth derived for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionHeading {
    pub start_byte: u32,
    pub level: u8,
    pub name: String,
}

/// Between breadcrumb components. Spaces on both sides so a full-text index
/// tokenizes the parts as words rather than as one opaque string.
const BREADCRUMB_SEPARATOR: &str = " > ";

/// How much breadcrumb is worth keeping. Over either limit the *outermost*
/// components go first: the file path already says which document a chunk is
/// in, so what is worth keeping is where inside it.
const MAX_BREADCRUMB_CHARS: usize = 200;
const MAX_BREADCRUMB_COMPONENTS: usize = 5;

/// The character a heading repeats to encode its depth, for languages whose
/// headings are line-prefixed that way. `None` means no breadcrumb rather
/// than a guessed one.
fn heading_marker(language: Language) -> Option<char> {
    match language {
        Language::Markdown => Some('#'),
        _ => None,
    }
}

/// Depth of the heading at `start_byte`, read off the source line: `### Errors`
/// is 3.
///
/// Derived from the text rather than stored on [`Symbol`], because a symbol is
/// persisted and reused whenever a file's hash is unchanged — a new field on
/// it would need a schema migration *and* a way to backfill every file already
/// indexed, while the depth is sitting in content the chunk builder has
/// already read.
///
/// `None` when the line is not a marker run followed by a space. A Markdown
/// setext heading (a title underlined with `===`) is the real case, and it is
/// left out of the ancestry rather than given an invented depth.
pub fn heading_level(content: &str, start_byte: u32, language: Language) -> Option<u8> {
    let marker = heading_marker(language)?;
    let line = content.get(start_byte as usize..)?.lines().next()?;
    // The marker is ASCII, so a count of leading marker characters is also a
    // byte offset: `line[depth..]` cannot split a character.
    let depth = line.chars().take_while(|c| *c == marker).count();
    if depth == 0 || depth > 6 || !line[depth..].starts_with(' ') {
        return None;
    }
    Some(depth as u8)
}

/// The file's headings that carry a derivable depth, in document order.
/// `symbols` must already be sorted by `start_byte`.
pub fn section_headings(
    symbols: &[Symbol],
    content: &str,
    language: Language,
) -> Vec<SectionHeading> {
    symbols
        .iter()
        .filter_map(|sym| {
            Some(SectionHeading {
                start_byte: sym.start_byte,
                level: heading_level(content, sym.start_byte, language)?,
                name: sym.name.clone(),
            })
        })
        .collect()
}

/// `Install > Requirements > macOS` — the trail of headings leading to
/// `anchor`, walking back through the nearest shallower heading at each step.
///
/// This is what a section chunk is called. It is the only way to tell two
/// chunks of one long document apart before reading them, and the column a
/// full-text search can weight above body text.
///
/// An anchor with no derivable depth still gets its own name back rather than
/// `None`: a title alone is worth indexing, it just cannot take part in the
/// ancestry.
pub fn section_breadcrumb(anchor: &Symbol, headings: &[SectionHeading]) -> Option<String> {
    if anchor.name.is_empty() {
        return None;
    }
    let Some(position) = headings.iter().position(|h| h.start_byte == anchor.start_byte) else {
        return Some(anchor.name.clone());
    };

    let mut trail: Vec<&str> = vec![headings[position].name.as_str()];
    let mut level = headings[position].level;
    for heading in headings[..position].iter().rev() {
        if heading.level < level {
            trail.push(heading.name.as_str());
            level = heading.level;
            if level == 1 {
                break;
            }
        }
    }
    trail.reverse();

    while trail.len() > MAX_BREADCRUMB_COMPONENTS
        || (trail.len() > 1 && joined_chars(&trail) > MAX_BREADCRUMB_CHARS)
    {
        trail.remove(0);
    }
    Some(trail.join(BREADCRUMB_SEPARATOR))
}

fn joined_chars(trail: &[&str]) -> usize {
    trail.iter().map(|part| part.chars().count()).sum::<usize>()
        + BREADCRUMB_SEPARATOR.len() * trail.len().saturating_sub(1)
}

pub fn chunk_hash(file_hash: blake3::Hash, start_byte: u32, end_byte: u32) -> blake3::Hash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(file_hash.as_bytes());
    hasher.update(&start_byte.to_le_bytes());
    hasher.update(&end_byte.to_le_bytes());
    hasher.update(&CHUNK_VERSION.to_le_bytes());
    hasher.finalize()
}

/// Sorts by `start_byte` and assigns `ordinal`. The one place chunk order is
/// decided, run after semantic splitting *and* after size splitting so the
/// numbers describe the final sequence either way.
pub fn finalize_ordinals(mut chunks: Vec<Chunk>) -> Vec<Chunk> {
    chunks.sort_by_key(|c| c.metadata.start_byte);
    for (i, chunk) in chunks.iter_mut().enumerate() {
        chunk.metadata.ordinal = i as u32;
    }
    chunks
}

/// How far back [`split_one_chunk`] looks for a readable boundary before
/// settling for an arbitrary one. An internal tuning constant, not a knob.
const SAFE_BOUNDARY_LOOKBACK: usize = 2048;

/// The size ceiling, applied once after semantic splitting and never
/// duplicated per language. A chunk within the limit passes through untouched;
/// an oversized one is cut near the limit, preferring a blank line, a
/// statement end or a closing brace over an arbitrary offset. The parts keep
/// the parent's kind, name, language and file, and get their own id and hash.
pub fn split_oversized_chunks(chunks: Vec<Chunk>, max_chunk_bytes: usize) -> Vec<Chunk> {
    let mut out = Vec::with_capacity(chunks.len());
    for chunk in chunks {
        if chunk.text.len() <= max_chunk_bytes {
            out.push(chunk);
            continue;
        }
        out.extend(split_one_chunk(chunk, max_chunk_bytes));
    }
    out
}

fn split_one_chunk(chunk: Chunk, max_chunk_bytes: usize) -> Vec<Chunk> {
    let Chunk { metadata, text } = chunk;
    let base_start = metadata.start_byte;

    let mut boundaries = Vec::new();
    let mut offset = 0usize;
    while offset < text.len() {
        if text.len() - offset <= max_chunk_bytes {
            boundaries.push((offset, text.len()));
            break;
        }
        let cut = find_safe_split(&text, offset, offset + max_chunk_bytes);
        boundaries.push((offset, cut));
        offset = cut;
    }

    boundaries
        .into_iter()
        .map(|(part_start, part_end)| {
            let start_byte = base_start + part_start as u32;
            let end_byte = base_start + part_end as u32;
            Chunk {
                metadata: ChunkMetadata {
                    id: ChunkId(format!("{}#{}-{}", metadata.file_id.0, start_byte, end_byte)),
                    file_id: metadata.file_id.clone(),
                    language: metadata.language,
                    kind: metadata.kind,
                    start_byte,
                    end_byte,
                    file_hash: metadata.file_hash,
                    hash: chunk_hash(metadata.file_hash, start_byte, end_byte),
                    qualified_name: metadata.qualified_name.clone(),
                    ordinal: 0,
                },
                text: text[part_start..part_end].to_string(),
            }
        })
        .collect()
}

/// A cut at or before `target` and after `window_start`, preferring a blank
/// line, a statement end or a closing brace within [`SAFE_BOUNDARY_LOOKBACK`].
/// Always a valid UTF-8 boundary. The window is at least `max_chunk_bytes`
/// wide — far more than the longest UTF-8 character — so the search can always
/// make progress and cannot collapse into a zero-length cut.
fn find_safe_split(text: &str, window_start: usize, target: usize) -> usize {
    let mut safe_target = target.min(text.len());
    while safe_target > window_start && !text.is_char_boundary(safe_target) {
        safe_target -= 1;
    }

    let mut lookback_start = safe_target
        .saturating_sub(SAFE_BOUNDARY_LOOKBACK)
        .max(window_start);
    while lookback_start < safe_target && !text.is_char_boundary(lookback_start) {
        lookback_start += 1;
    }

    let search_area = &text[lookback_start..safe_target];
    for separator in ["\n\n", ";\n", "}\n"] {
        if let Some(pos) = search_area.rfind(separator) {
            return lookback_start + pos + separator.len();
        }
    }
    safe_target
}

#[cfg(test)]
mod tests {
    use super::*;

    fn symbol(name: &str, start: u32, end: u32) -> Symbol {
        Symbol {
            name: name.to_string(),
            start_line: 0,
            end_line: 0,
            start_byte: start,
            end_byte: end,
        }
    }

    fn chunk(text: &str, start: u32) -> Chunk {
        let file_hash = blake3::hash(b"file");
        let end = start + text.len() as u32;
        Chunk {
            metadata: ChunkMetadata {
                id: ChunkId(format!("a.md#{start}-{end}")),
                file_id: FileId("a.md".to_string()),
                language: Language::Markdown,
                kind: ChunkKind::Section,
                start_byte: start,
                end_byte: end,
                file_hash,
                hash: chunk_hash(file_hash, start, end),
                qualified_name: Some("Title".to_string()),
                ordinal: 0,
            },
            text: text.to_string(),
        }
    }

    /// Every byte in exactly one span, in order — the contract whatever the
    /// symbols were.
    fn assert_covers(spans: &[(u32, u32)], len: u32) {
        assert_eq!(spans.first().map(|s| s.0), Some(0), "{spans:?}");
        for pair in spans.windows(2) {
            assert_eq!(pair[0].1, pair[1].0, "gap or overlap in {spans:?}");
            assert!(pair[0].0 < pair[0].1, "empty or backwards span in {spans:?}");
        }
        assert_eq!(spans.last().map(|s| s.1), Some(len), "{spans:?}");
    }

    fn ranges(spans: &[ChunkSpan]) -> Vec<(u32, u32)> {
        spans.iter().map(|s| (s.start_byte, s.end_byte)).collect()
    }

    fn anchors(spans: &[ChunkSpan]) -> Vec<&str> {
        spans.iter().map(|s| s.anchor_symbol.as_ref().map_or("", |a| a.name.as_str())).collect()
    }

    // ------------------------------------------------------------ code

    /// A class and its methods both arrive. Cutting at the class too would
    /// give it a chunk covering its methods' chunks.
    #[test]
    fn code_is_cut_at_the_innermost_declarations() {
        let symbols = [symbol("Service", 10, 100), symbol("find", 30, 50), symbol("save", 60, 90)];
        let spans = spans_from_declarations(&symbols, 120);

        assert_eq!(anchors(&spans), ["find", "save"]);
        assert!(spans.iter().all(|s| s.kind == ChunkKind::Declaration));
        assert_eq!(ranges(&spans), [(0, 50), (50, 120)]);
    }

    /// Each declaration owns the gap before it: the doc comment and the
    /// attribute above `save` are in `save`'s chunk, not in `find`'s.
    #[test]
    fn a_declaration_takes_the_comment_above_it() {
        let content = "fn find() {}\n/// Saves.\n#[inline]\nfn save() {}\n";
        let save_at = content.find("fn save").unwrap() as u32;
        let symbols = [symbol("find", 0, 12), symbol("save", save_at, content.len() as u32 - 1)];
        let spans = spans_from_declarations(&symbols, content.len());

        let second = &content[spans[1].start_byte as usize..spans[1].end_byte as usize];
        assert!(second.starts_with("\n/// Saves.\n#[inline]"), "{second:?}");
    }

    #[test]
    fn a_container_with_no_members_is_its_own_chunk() {
        let symbols = [symbol("Empty", 0, 10), symbol("Full", 20, 80), symbol("run", 30, 70)];
        assert_eq!(anchors(&spans_from_declarations(&symbols, 80)), ["Empty", "run"]);
    }

    /// B-1 in its original habitat. Upstream's backward-gap builder took
    /// anchors as given, and a nested one sent `end` before `start`. Symbols
    /// out of order, nested, crossing, empty and past the end go in; spans
    /// that cover the file come out.
    #[test]
    fn no_arrangement_of_symbols_produces_a_bad_span() {
        let symbols = [
            symbol("late", 70, 90),
            symbol("outer", 0, 60),
            symbol("inner", 10, 20),
            symbol("crossing", 15, 40),
            symbol("empty", 45, 45),
            symbol("beyond", 95, 200),
        ];
        let spans = spans_from_declarations(&symbols, 100);

        assert_covers(&ranges(&spans), 100);
        // Which of two crossing symbols wins is arbitrary; that both cannot,
        // and that the untangled one after them survives, is not.
        let kept = anchors(&spans);
        assert_eq!(kept.len(), 2, "{kept:?}");
        assert_eq!(kept[1], "late");
    }

    #[test]
    fn code_with_no_declarations_is_one_chunk() {
        let spans = spans_from_declarations(&[], 40);
        assert_eq!(ranges(&spans), [(0, 40)]);
        assert_eq!(spans[0].kind, ChunkKind::File);
    }

    #[test]
    fn a_declaration_is_named_after_what_encloses_it() {
        let symbols = [symbol("Service", 0, 100), symbol("Listener", 10, 50), symbol("changed", 20, 40)];
        assert_eq!(qualified_name(&symbols[2], &symbols), "Service.Listener.changed");
        assert_eq!(qualified_name(&symbols[0], &symbols), "Service");
    }

    // --------------------------------------------------- per language

    #[test]
    fn each_language_is_cut_its_own_way() {
        let symbols = [symbol("A", 0, 5), symbol("B", 10, 15)];

        let json = spans_for(Language::Json, &symbols, 20);
        assert_eq!((json.len(), json[0].kind), (1, ChunkKind::File), "config is never cut at symbols");

        let markdown = spans_for(Language::Markdown, &symbols, 20);
        assert_eq!(ranges(&markdown), [(0, 10), (10, 20)], "a section owns what follows");

        let rust = spans_for(Language::Rust, &symbols, 20);
        assert_eq!(ranges(&rust), [(0, 5), (5, 20)], "a declaration owns what precedes");
    }

    // ---------------------------------------------------- the builder

    #[test]
    fn built_chunks_cover_the_file_and_carry_their_names() {
        let content = "use x;\n\nimpl Turn {\n    fn new() {}\n    fn run() {}\n}\n";
        let at = |needle: &str| content.find(needle).unwrap() as u32;
        let symbols = [
            symbol("run", at("fn run"), at("fn run") + 11),
            symbol("Turn", at("impl"), content.len() as u32 - 1),
            symbol("new", at("fn new"), at("fn new") + 11),
        ];
        let chunks = build_chunks(&FileId("src/turn.rs".into()), Language::Rust, content, &symbols, &ChunkBuildOptions::default());

        let names: Vec<_> = chunks.iter().map(|c| c.metadata.qualified_name.as_deref().unwrap_or("")).collect();
        assert_eq!(names, ["Turn.new", "Turn.run"]);
        assert_eq!(chunks.iter().map(|c| c.text.as_str()).collect::<String>(), content);
        assert_covers(&chunks.iter().map(|c| (c.metadata.start_byte, c.metadata.end_byte)).collect::<Vec<_>>(), content.len() as u32);
        assert_eq!(chunks.iter().map(|c| c.metadata.ordinal).collect::<Vec<_>>(), [0, 1]);
        assert_eq!(chunks[0].metadata.id.0, format!("src/turn.rs#0-{}", chunks[0].metadata.end_byte));
        assert_eq!(chunks[0].metadata.file_hash, blake3::hash(content.as_bytes()));
    }

    #[test]
    fn built_sections_are_named_by_their_breadcrumb() {
        let content = "# Install\n\n## macOS\n\nbrew\n";
        let symbols = [symbol("Install", 0, 9), symbol("macOS", 11, 19)];
        let chunks = build_chunks(&FileId("README.md".into()), Language::Markdown, content, &symbols, &ChunkBuildOptions::default());

        assert_eq!(chunks[1].metadata.qualified_name.as_deref(), Some("Install > macOS"));
    }

    #[test]
    fn a_huge_declaration_is_split_under_its_own_name() {
        let body = "    step();\n".repeat(3000);
        let content = format!("fn big() {{\n{body}}}\n");
        let symbols = [symbol("big", 0, content.len() as u32 - 1)];
        let options = ChunkBuildOptions::default();
        let chunks = build_chunks(&FileId("big.rs".into()), Language::Rust, &content, &symbols, &options);

        assert!(chunks.len() > 1);
        assert!(chunks.iter().all(|c| c.text.len() <= options.max_chunk_bytes));
        assert!(chunks.iter().all(|c| c.metadata.qualified_name.as_deref() == Some("big")));
    }

    /// A symbol stored for other content can land inside a character. Slicing
    /// there panics; the builder drops the span instead.
    #[test]
    fn a_span_inside_a_character_is_dropped_not_sliced() {
        // `é` is bytes 0..2; the first symbol ends, and so the second span
        // starts, at byte 1.
        let content = "é fn a() {}";
        let symbols = [symbol("x", 0, 1), symbol("a", 3, 11)];
        let chunks = build_chunks(&FileId("a.rs".into()), Language::Rust, content, &symbols, &ChunkBuildOptions::default());
        assert!(chunks.iter().all(|c| content.is_char_boundary(c.metadata.start_byte as usize)));
    }

    #[test]
    fn an_empty_file_builds_no_chunks() {
        assert!(build_chunks(&FileId("a.rs".into()), Language::Rust, "", &[], &ChunkBuildOptions::default()).is_empty());
    }

    // ------------------------------------------------------- the span builder

    #[test]
    fn a_section_owns_what_follows_its_heading() {
        let spans = spans_from_forward_gap_symbols(
            &[symbol("One", 10, 15), symbol("Two", 40, 45)],
            100,
        );

        assert_eq!(spans.len(), 2);
        // The preamble belongs to the first section rather than to nobody.
        assert_eq!((spans[0].start_byte, spans[0].end_byte), (0, 40));
        assert_eq!((spans[1].start_byte, spans[1].end_byte), (40, 100));
    }

    #[test]
    fn a_file_with_no_headings_is_one_chunk() {
        let spans = spans_from_forward_gap_symbols(&[], 100);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].kind, ChunkKind::File);
        assert_eq!(spans[0].anchor_symbol, None);
    }

    #[test]
    fn an_empty_file_has_nothing_to_chunk() {
        assert!(spans_from_forward_gap_symbols(&[], 0).is_empty());
        assert!(whole_file_span(0).is_empty());
    }

    /// B-1. Upstream trusts the caller to hand over sorted anchors; nothing
    /// makes that true. Out of order, the span runs backwards, and the
    /// consumer slices `content[start..end]` — a panic upstream once, a
    /// silently dropped chunk there now.
    #[test]
    fn anchors_out_of_order_do_not_produce_a_backwards_span() {
        let spans = spans_from_forward_gap_symbols(
            &[symbol("Third", 80, 85), symbol("First", 10, 15), symbol("Second", 40, 45)],
            100,
        );

        for span in &spans {
            assert!(
                span.start_byte <= span.end_byte,
                "span runs backwards: {span:?}"
            );
        }
        assert_eq!(
            spans.iter().map(|s| s.start_byte).collect::<Vec<_>>(),
            vec![0, 40, 80],
            "the sections are not in document order"
        );
    }

    /// A nested symbol, or two headings an indexer reported at the same
    /// offset: the second cannot start a span, because the span would have no
    /// text in it.
    #[test]
    fn an_anchor_that_does_not_advance_is_dropped() {
        let spans = spans_from_forward_gap_symbols(
            &[symbol("Outer", 10, 90), symbol("Inner", 10, 20), symbol("Next", 50, 55)],
            100,
        );

        assert_eq!(spans.len(), 2);
        assert!(spans.iter().all(|s| s.start_byte < s.end_byte));
    }

    /// An anchor past the end of the content would give the last span a
    /// start beyond its end. Real when an indexer reads one revision of a
    /// file and the chunker another.
    #[test]
    fn an_anchor_past_the_end_of_the_file_is_dropped() {
        let spans = spans_from_forward_gap_symbols(
            &[symbol("Real", 10, 15), symbol("Ghost", 500, 505)],
            100,
        );

        assert_eq!(spans.len(), 1);
        assert_eq!((spans[0].start_byte, spans[0].end_byte), (0, 100));
    }

    /// Every anchor gone leaves the file itself, not an empty index entry.
    #[test]
    fn a_file_whose_anchors_are_all_unusable_is_still_chunked() {
        let spans = spans_from_forward_gap_symbols(&[symbol("Ghost", 500, 505)], 100);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].kind, ChunkKind::File);
    }

    // ---------------------------------------------------------- breadcrumbs

    const DOC: &str = "# Guide\nintro\n## Install\n## Requirements\n### macOS\nbody\n";

    fn doc_headings() -> Vec<SectionHeading> {
        let symbols: Vec<Symbol> = [
            ("Guide", 0u32),
            ("Install", 14),
            ("Requirements", 25),
            ("macOS", 41),
        ]
        .into_iter()
        .map(|(name, at)| symbol(name, at, at + 1))
        .collect();
        section_headings(&symbols, DOC, Language::Markdown)
    }

    #[test]
    fn a_heading_level_is_read_off_the_line() {
        assert_eq!(heading_level(DOC, 0, Language::Markdown), Some(1));
        assert_eq!(heading_level(DOC, 41, Language::Markdown), Some(3));
    }

    /// A marker run with no space after it is not a heading — `#hashtag` and a
    /// Rust attribute both start that way.
    #[test]
    fn a_marker_without_a_space_is_not_a_heading() {
        assert_eq!(heading_level("#nope\n", 0, Language::Markdown), None);
        assert_eq!(heading_level("####### too deep\n", 0, Language::Markdown), None);
    }

    /// A language whose headings are not line-prefixed gets no breadcrumb
    /// rather than a depth invented from whatever the line starts with.
    #[test]
    fn a_language_without_markers_has_no_levels() {
        assert_eq!(heading_level("# not markdown\n", 0, Language::Rust), None);
        assert!(section_headings(&[symbol("x", 0, 1)], "# x\n", Language::Rust).is_empty());
    }

    #[test]
    fn a_breadcrumb_walks_back_through_shallower_headings() {
        let headings = doc_headings();
        assert_eq!(
            section_breadcrumb(&symbol("macOS", 41, 46), &headings).as_deref(),
            Some("Guide > Requirements > macOS")
        );
    }

    /// Only *shallower* headings are ancestors. `Install` is a sibling of
    /// `Requirements`, not its parent.
    #[test]
    fn a_sibling_is_not_an_ancestor() {
        let headings = doc_headings();
        let trail = section_breadcrumb(&symbol("macOS", 41, 46), &headings).unwrap();
        assert!(!trail.contains("Install"), "{trail}");
    }

    /// A heading the level rule could not read still names its own chunk.
    #[test]
    fn a_heading_with_no_derivable_level_still_names_itself() {
        assert_eq!(
            section_breadcrumb(&symbol("Setext", 999, 1000), &doc_headings()).as_deref(),
            Some("Setext")
        );
    }

    #[test]
    fn an_unnamed_anchor_has_no_breadcrumb() {
        assert_eq!(section_breadcrumb(&symbol("", 0, 1), &doc_headings()), None);
    }

    /// The path already says which document this is. What has to survive the
    /// limit is the innermost part — where inside it.
    #[test]
    fn a_long_trail_loses_its_outermost_parts_first() {
        let headings: Vec<SectionHeading> = (1..=8)
            .map(|level| SectionHeading {
                start_byte: level as u32 * 10,
                level: level.min(6),
                name: format!("level{level}"),
            })
            .collect();
        let anchor = symbol("level6", 60, 61);

        let trail = section_breadcrumb(&anchor, &headings).expect("a trail");
        assert!(trail.split(" > ").count() <= MAX_BREADCRUMB_COMPONENTS);
        assert!(trail.ends_with("level6"), "the innermost part was dropped: {trail}");
    }

    // ------------------------------------------------------------- the hash

    /// The trigger for recomputing an embedding: the file changed, or this
    /// span moved. Both have to change the hash, or a stale vector survives
    /// a rebuild.
    #[test]
    fn the_hash_follows_the_file_and_the_position() {
        let one = blake3::hash(b"one");
        let other = blake3::hash(b"other");

        assert_eq!(chunk_hash(one, 0, 10), chunk_hash(one, 0, 10));
        assert_ne!(chunk_hash(one, 0, 10), chunk_hash(other, 0, 10));
        assert_ne!(chunk_hash(one, 0, 10), chunk_hash(one, 5, 15));
        assert_ne!(chunk_hash(one, 0, 10), chunk_hash(one, 0, 11));
    }

    // ------------------------------------------------------- size splitting

    /// `CHUNK_VERSION` is in the digest so that bumping it invalidates every
    /// stored chunk without a migration. Nothing else proves it is in there:
    /// drop the line and every other test still passes, while a version bump
    /// silently rebuilds nothing.
    ///
    /// **This test is meant to fail when `CHUNK_VERSION` changes.** That is
    /// the acknowledgement — every chunk on every disk is now stale — and the
    /// fix is to paste the new digest in, deliberately.
    #[test]
    fn the_chunk_version_is_part_of_the_hash() {
        assert_eq!(
            chunk_hash(blake3::hash(b"file"), 100, 200).to_hex().as_str(),
            "3c3f518af93b6e87363e54473500b8126a61f15ccee2ba69cc71329bf3556214",
            "the chunk hash changed — if CHUNK_VERSION was bumped this is correct \
             and the new digest goes here; otherwise something fell out of the digest"
        );
    }

    #[test]
    fn a_chunk_within_the_limit_is_left_alone() {
        let before = chunk("small", 100);
        let after = split_oversized_chunks(vec![before.clone()], 16);
        assert_eq!(after, vec![before]);
    }

    #[test]
    fn an_oversized_chunk_is_cut_and_the_parts_cover_it_exactly() {
        let text = "a".repeat(50);
        let parts = split_oversized_chunks(vec![chunk(&text, 100)], 16);

        assert!(parts.len() > 1);
        assert_eq!(parts.iter().map(|p| p.text.as_str()).collect::<String>(), text);
        assert_eq!(parts[0].metadata.start_byte, 100);
        assert_eq!(parts.last().unwrap().metadata.end_byte, 150);
        for pair in parts.windows(2) {
            assert_eq!(pair[0].metadata.end_byte, pair[1].metadata.start_byte);
        }
    }

    /// Each part is its own chunk in the store, so it needs its own identity —
    /// parts sharing the parent's id would overwrite each other.
    #[test]
    fn the_parts_get_their_own_ids_and_hashes_but_keep_the_name() {
        let parts = split_oversized_chunks(vec![chunk(&"a".repeat(50), 0)], 16);

        let ids: std::collections::HashSet<_> = parts.iter().map(|p| &p.metadata.id).collect();
        assert_eq!(ids.len(), parts.len(), "two parts share an id");
        let hashes: std::collections::HashSet<_> = parts.iter().map(|p| p.metadata.hash).collect();
        assert_eq!(hashes.len(), parts.len(), "two parts share a hash");
        assert!(parts.iter().all(|p| p.metadata.qualified_name.as_deref() == Some("Title")));
        assert!(parts.iter().all(|p| p.metadata.kind == ChunkKind::Section));
    }

    /// Cutting mid-character would panic on the slice. A multibyte file is
    /// the ordinary case here, not an edge one.
    #[test]
    fn a_multibyte_chunk_is_cut_on_a_character_boundary() {
        let text = "рекомендация ".repeat(400);
        let parts = split_oversized_chunks(vec![chunk(&text, 0)], 1000);

        assert!(parts.len() > 1);
        assert_eq!(parts.iter().map(|p| p.text.as_str()).collect::<String>(), text);
    }

    /// A blank line is where a reader would have cut it too.
    #[test]
    fn the_cut_prefers_a_readable_boundary() {
        let text = format!("{}\n\n{}", "a".repeat(900), "b".repeat(900));
        let parts = split_oversized_chunks(vec![chunk(&text, 0)], 1000);

        assert_eq!(parts[0].text, format!("{}\n\n", "a".repeat(900)));
    }

    /// No boundary anywhere within the lookback: it still has to make
    /// progress rather than loop or emit an empty part.
    #[test]
    fn a_chunk_with_no_boundary_at_all_is_still_cut() {
        let parts = split_oversized_chunks(vec![chunk(&"a".repeat(5000), 0)], 1000);

        assert_eq!(parts.len(), 5);
        assert!(parts.iter().all(|p| !p.text.is_empty()));
    }

    // ----------------------------------------------------------- ordinals

    /// Assigned after every split, so the numbers describe the sequence that
    /// actually exists rather than the one before it was cut up.
    #[test]
    fn ordinals_number_the_final_sequence_in_document_order() {
        let chunks = vec![chunk("third", 200), chunk("first", 0), chunk("second", 100)];
        let numbered = finalize_ordinals(chunks);

        assert_eq!(
            numbered.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(),
            vec!["first", "second", "third"]
        );
        assert_eq!(
            numbered.iter().map(|c| c.metadata.ordinal).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
    }
}
