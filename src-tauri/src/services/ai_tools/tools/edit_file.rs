//! `editFile` — anchored replacements inside a file that already exists.
//!
//! Exact and all-or-nothing — except that an anchor with no exact match is
//! tried once more ignoring line-end whitespace and typographic punctuation
//! ([`find_loosely`]), as pi does. Every anchor is looked up in the file's original
//! content, never in the output of an earlier edit in the same call, which is
//! what makes a batch independent of its own ordering. An anchor that matches
//! nothing, matches more than once, or overlaps another edit's region rejects
//! the entire call before anything reaches the disk.
//!
//! Alfa Atlas has a second path this does not port: when an exact match fails,
//! `fast_apply` hands the whole file and the edit's intent to the model and
//! asks it to make the change, then checks that everything outside the edited
//! region is byte-identical. It is ingenious and it turns a deterministic tool
//! into another model call, with that cost and that variance. A failed anchor
//! is already a precise, actionable error; the model can widen it and retry.

use crate::domain::file_write::{in_endings, line_ending};
use crate::domain::llm::LlmToolDefinition;
use std::borrow::Cow;
use std::fs;

use crate::domain::tools::{
    EditFileArgs, FileEdit, ReadFiles, ToolError, ToolResult, ToolScope, WriteBlocked,
};
use crate::services::text_diff;

use super::super::resolve::{relative_to_root, resolve_existing};

pub fn edit_file(
    scope: &ToolScope,
    args: &EditFileArgs,
    reads: &mut ReadFiles,
) -> Result<ToolResult, ToolError> {
    let path = resolve_existing(scope, &args.path)?;
    if !path.is_file() {
        return Err(ToolError::NotAFile(args.path.clone()));
    }
    let relative = relative_to_root(scope, &path)?;
    let content = fs::read_to_string(&path).map_err(ToolError::Io)?;

    // `false`: an anchored edit is content-addressed, so having read part of
    // the file is enough. The anchor has to be unique in the whole file, and
    // the hash proves nothing moved underneath — between them the parts the
    // agent never read are as safe as the parts it did.
    reads
        .check(&relative, &content, false)
        .map_err(|blocked| match blocked {
            WriteBlocked::NeverRead => ToolError::FileNotRead(relative.clone()),
            WriteBlocked::ReadInPart => ToolError::FileReadInPart(relative.clone()),
            WriteBlocked::ChangedSinceRead => ToolError::FileChangedSinceRead(relative.clone()),
        })?;

    let edited = apply_edits(&content, &args.edits)?;
    fs::write(&path, &edited).map_err(ToolError::Io)?;

    // `false`: editing taught the agent nothing about the parts it never read,
    // so this must not become a licence to replace the file wholesale.
    reads.record_write(&relative, &edited, false);

    Ok(ToolResult::FileEdited {
        path: relative,
        diff: text_diff::diff_stats(&content, &edited),
    })
}

/// Splices every edit into `content` at once. Nothing is applied unless all of
/// them resolve.
pub(in crate::services::ai_tools) fn apply_edits(content: &str, edits: &[FileEdit]) -> Result<String, ToolError> {
    let ranges = exact_match_ranges(content, edits)?;

    let mut result = String::with_capacity(content.len());
    let mut cursor = 0;
    for (start, end, new) in ranges {
        result.push_str(&content[cursor..start]);
        result.push_str(&new);
        cursor = end;
    }
    result.push_str(&content[cursor..]);
    Ok(result)
}

/// Resolves every anchor to a unique byte range in `content`, sorted by
/// position, having checked that none of them overlap.
fn exact_match_ranges<'a>(
    content: &str,
    edits: &'a [FileEdit],
) -> Result<Vec<(usize, usize, Cow<'a, str>)>, ToolError> {
    let ending = line_ending(content);
    let mut ranges: Vec<(usize, usize, Cow<str>)> = Vec::with_capacity(edits.len());
    for (index, edit) in edits.iter().enumerate() {
        let old = in_endings(&edit.old, ending);
        let (start, end) = find_unique(content, &old).map_err(|reason| match edits.len() {
            1 => reason,
            of => ToolError::InEdit { index: index + 1, of, reason: Box::new(reason) },
        })?;
        ranges.push((start, end, in_endings(&edit.new, ending)));
    }

    ranges.sort_by_key(|&(start, _, _)| start);
    for pair in ranges.windows(2) {
        if pair[1].0 < pair[0].1 {
            return Err(ToolError::EditsOverlap);
        }
    }
    Ok(ranges)
}

/// Whether `old` would have matched with the file's line endings — reached
/// only in a file that mixes both, where which one was meant is a guess.
fn line_endings_differ(content: &str, old: &str) -> Option<ToolError> {
    if old.contains("\r\n") && content.contains(&old.replace("\r\n", "\n")) {
        return Some(ToolError::EditLineEndings { file: "LF", edit: "CRLF" });
    }
    if old.contains('\n') && !old.contains('\r') && content.contains(&old.replace('\n', "\r\n")) {
        return Some(ToolError::EditLineEndings { file: "CRLF", edit: "LF" });
    }
    None
}

/// The anchor's single occurrence.
///
/// Reporting the count on an ambiguous match is the actionable half: it tells
/// the model the anchor was too short, instead of leaving it to guess why an
/// exact match was refused.
fn find_unique(content: &str, old: &str) -> Result<(usize, usize), ToolError> {
    let mut occurrences = content.match_indices(old);
    let (start, end) = match occurrences.next() {
        Some((start, _)) => {
            let count = 1 + occurrences.count();
            if count > 1 {
                return Err(ToolError::EditTextAmbiguous(old.to_string(), count));
            }
            (start, start + old.len())
        }
        None => {
            if let Some(mismatch) = line_endings_differ(content, old) {
                return Err(mismatch);
            }
            find_loosely(content, old)?.ok_or_else(|| ToolError::EditTextNotFound {
                text: old.to_string(),
                nearest: closest_line(content, old),
            })?
        }
    };
    if let Some(word) = split_word(content, start, end) {
        return Err(ToolError::EditInsideWord(old.to_string(), word));
    }
    Ok((start, end))
}

/// The anchor's single occurrence once both sides are [`loosened`], as a range
/// of the original `content` — `None` when there is none.
///
/// What a model gets wrong without meaning anything by it: whitespace at the
/// end of a line it cannot see, and the typographic quotes, dashes and spaces
/// it writes back as ASCII. Only the matched range is replaced; the rest of the
/// file keeps its bytes, curly quotes and trailing spaces included. The match
/// still has to be unique, under the same error as an exact one.
fn find_loosely(content: &str, old: &str) -> Result<Option<(usize, usize)>, ToolError> {
    let (wanted, _) = loosened(old, false);
    let (text, origin) = loosened(content, true);
    let mut occurrences = text.match_indices(&wanted);
    let Some((start, _)) = occurrences.next() else {
        return Ok(None);
    };
    let count = 1 + occurrences.count();
    if count > 1 {
        return Err(ToolError::EditTextAmbiguous(old.to_string(), count));
    }
    // To where the next kept character begins: whitespace stripped from the
    // end of the anchor's last line goes with it.
    Ok(Some((origin[start], origin[start + wanted.len()])))
}

/// `text` without whitespace at the ends of its lines and with typographic
/// quotes, dashes and spaces made ASCII, plus where each of its bytes came
/// from in `text` — one more entry than bytes, the last being `text.len()`.
///
/// Line breaks stay as they are: which ending an edit meant is
/// [`in_endings`]'s business, and a guess here would undo its refusal in a
/// mixed file.
///
/// `file` says the end of `text` is the end of a line. An anchor's last line
/// with no break after it is not: `x – ` may stop before the ` 1` of `x - 1`,
/// and losing its space would double the one its replacement brings.
fn loosened(text: &str, file: bool) -> (String, Vec<usize>) {
    let mut out = String::with_capacity(text.len());
    let mut origin = Vec::with_capacity(text.len() + 1);
    let mut line_start = 0;
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches('\n').trim_end_matches('\r');
        let kept = if file || line.ends_with('\n') { body.trim_end_matches(char::is_whitespace) } else { body };
        let pieces = kept.char_indices().map(|(i, c)| (i, ascii(c))).chain(
            line[body.len()..].char_indices().map(|(i, c)| (body.len() + i, c)),
        );
        for (offset, c) in pieces {
            out.push(c);
            origin.extend(std::iter::repeat_n(line_start + offset, c.len_utf8()));
        }
        line_start += line.len();
    }
    origin.push(text.len());
    (out, origin)
}

fn ascii(c: char) -> char {
    match c {
        '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => '\'',
        '\u{201C}'..='\u{201F}' => '"',
        '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
        '\u{00A0}' | '\u{2002}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}' => ' ',
        c => c,
    }
}

/// The line of `content` most like the anchor's first non-blank one: the one
/// sharing the most words with it — at least half, or it is a guess worth
/// less than no hint. The same line indented otherwise shares all of them.
// ponytail: the anchor's first line only; one that starts `}` finds nothing.
fn closest_line(content: &str, old: &str) -> Option<(u32, String)> {
    let first = old.lines().map(str::trim).find(|line| !line.is_empty())?;
    let words = |text: &str| -> std::collections::HashSet<String> {
        text.split(|c: char| !c.is_alphanumeric() && c != '_').filter(|w| !w.is_empty()).map(str::to_string).collect()
    };
    let wanted = words(first);
    let (index, line, score) = content
        .lines()
        .enumerate()
        .map(|(i, line)| (i, line, words(line).intersection(&wanted).count()))
        .max_by_key(|&(i, _, score)| (score, std::cmp::Reverse(i)))?;
    (score > 0 && score * 2 >= wanted.len())
        .then(|| (index as u32 + 1, line.trim_end().chars().take(200).collect()))
}

/// The word `start..end` cuts into, if an edge of it falls between two word
/// characters. `id` in `userId` and `line on` in `line one` do; `middle` at
/// the start of a line does not.
fn split_word(content: &str, start: usize, end: usize) -> Option<String> {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let before = content[..start].chars().next_back();
    let first = content[start..end].chars().next();
    let last = content[start..end].chars().next_back();
    let after = content[end..].chars().next();
    let cut_start = before.is_some_and(word) && first.is_some_and(word);
    let cut_end = last.is_some_and(word) && after.is_some_and(word);
    if !cut_start && !cut_end {
        return None;
    }
    // The text between the nearest non-word characters on either side.
    let from = content[..start].char_indices().rev().find(|(_, c)| !word(*c)).map_or(0, |(i, c)| i + c.len_utf8());
    let to = content[end..].find(|c: char| !word(c)).map_or(content.len(), |i| end + i);
    Some(content[from..to].chars().take(80).collect())
}

/// What the model is told `editFile` is for.
pub(super) fn definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "editFile".to_string(),
        description: "Replace exact passages in an existing file. The preferred way to change code: it touches only what you name. Each edit's `old` must appear **exactly once** in the file as it is now — include the surrounding lines needed to make it unique — and must begin and end on whole words: an anchor that starts or ends inside a name is refused. If any anchor is missing, ambiguous or overlaps another edit, the whole call is refused and nothing is written, so a failed edit never leaves the file half-changed. All edits are matched against the file's original content, so one edit's replacement can never become another's anchor. The file must already exist. It needs no readFile first: an anchor seen in a grep result is enough, since it has to match exactly. A file that changed on disk since you read it is refused."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path relative to the workspace root."
                },
                "edits": {
                    "type": "array",
                    "description": "Applied together, all or nothing.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "old": {
                                "type": "string",
                                "description": "The exact text to replace, including indentation and line breaks, unique within the file."
                            },
                            "new": {
                                "type": "string",
                                "description": "What to put in its place. Empty string deletes the passage."
                            }
                        },
                        "required": [
                            "old",
                            "new"
                        ]
                    }
                }
            },
            "required": [
                "path",
                "edits"
            ]
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::tools::{ReadFileArgs, ToolCall, ToolDeps};
    use crate::services::ai_tools::tools::{execute_tool, read_file::read_file};
    use crate::testing::temp_dir;
    use std::path::{Path, PathBuf};

    fn fixture(label: &str, body: &str) -> (ToolScope, PathBuf, ReadFiles) {
        let dir = temp_dir(label);
        std::fs::write(dir.join("a.txt"), body).expect("file is writable");
        let scope = ToolScope::new(&dir).expect("root resolves");
        let root = scope.root().to_path_buf();
        let mut reads = ReadFiles::default();
        agent_reads(&scope, &mut reads, "a.txt", None);
        (scope, root, reads)
    }

    fn agent_reads(scope: &ToolScope, reads: &mut ReadFiles, path: &str, range: Option<(u32, u32)>) {
        let args = ReadFileArgs {
            path: path.to_string(),
            paths: None,
            start_line: range.map(|(s, _)| s),
            end_line: range.map(|(_, e)| e),
            outline: None,
        };
        read_file(scope, &args, reads).expect("read succeeds");
    }

    fn edits(pairs: &[(&str, &str)]) -> EditFileArgs {
        EditFileArgs {
            path: "a.txt".to_string(),
            edits: pairs
                .iter()
                .map(|(old, new)| FileEdit {
                    old: (*old).to_string(),
                    new: (*new).to_string(),
                })
                .collect(),
        }
    }

    fn on_disk(root: &Path) -> String {
        std::fs::read_to_string(root.join("a.txt")).expect("readable")
    }

    #[test]
    fn replaces_the_anchored_text() {
        let (scope, root, mut reads) = fixture("edit-one", "let x = 1;\n");

        let result = edit_file(&scope, &edits(&[("= 1", "= 2")]), &mut reads).expect("edits");

        assert_eq!(on_disk(&root), "let x = 2;\n");
        let ToolResult::FileEdited { path, diff } = result else {
            panic!("wrong result")
        };
        assert_eq!(path, "a.txt");
        assert_eq!((diff.lines_added, diff.lines_removed), (1, 1));
    }

    /// Anchors resolve against the original content, never against an earlier
    /// edit's output — so a batch means the same thing whatever order it
    /// arrives in. Without that, two edits could silently fight.
    #[test]
    fn edits_are_independent_of_their_order() {
        let body = "alpha\nbeta\ngamma\n";
        let forwards = {
            let (scope, root, mut reads) = fixture("edit-order-fwd", body);
            edit_file(&scope, &edits(&[("alpha", "A"), ("gamma", "G")]), &mut reads).unwrap();
            on_disk(&root)
        };
        let backwards = {
            let (scope, root, mut reads) = fixture("edit-order-back", body);
            edit_file(&scope, &edits(&[("gamma", "G"), ("alpha", "A")]), &mut reads).unwrap();
            on_disk(&root)
        };

        assert_eq!(forwards, "A\nbeta\nG\n");
        assert_eq!(forwards, backwards);
    }

    /// The sharp case: one edit's replacement text *is* another edit's anchor.
    /// Applied against the original content this is unambiguous. Applied one
    /// after another it would not be — the first edit would manufacture a
    /// second occurrence of `B` and the next anchor would become ambiguous.
    /// The order-independence test above cannot tell the two apart, because on
    /// non-interfering edits they agree.
    #[test]
    fn an_anchor_never_matches_what_an_earlier_edit_produced() {
        let (scope, root, mut reads) = fixture("edit-no-cascade", "A\nB\n");

        edit_file(&scope, &edits(&[("A", "B"), ("B", "C")]), &mut reads)
            .expect("both anchors are unique in the original");

        assert_eq!(on_disk(&root), "B\nC\n");
    }

    #[test]
    fn a_missing_anchor_rejects_the_call() {
        let (scope, root, mut reads) = fixture("edit-missing", "let x = 1;\n");

        let err = edit_file(&scope, &edits(&[("= 9", "= 2")]), &mut reads).expect_err("no match");

        assert!(matches!(err, ToolError::EditTextNotFound { .. }));
        assert_eq!(on_disk(&root), "let x = 1;\n", "left untouched");
    }

    /// A miss says which edit of several it was and which line was probably
    /// meant — indented otherwise here — so the retry fixes one anchor, by
    /// copying.
    #[test]
    fn a_missing_anchor_names_its_edit_and_the_closest_line() {
        let body = "fn a() {\n    let total = price * qty;\n    total\n}\n";
        let (scope, _, mut reads) = fixture("edit-closest", body);

        let err = edit_file(&scope, &edits(&[("fn a()", "fn b()"), ("let total = price * qty;\n  total", "x")]), &mut reads)
            .expect_err("indented otherwise");
        let ToolError::InEdit { index: 2, of: 2, reason } = &err else { panic!("{err}") };
        assert!(
            matches!(&**reason, ToolError::EditTextNotFound { nearest: Some((2, line)), .. } if line == "    let total = price * qty;"),
            "{err}"
        );
        assert!(err.to_string().starts_with("edit 2 of 2: edit text not found"), "{err}");

        // One word changed is still the line; one word in common of four is not.
        let err = edit_file(&scope, &edits(&[("let total = cost * qty;", "x")]), &mut reads).expect_err("a word differs");
        assert!(matches!(err, ToolError::EditTextNotFound { nearest: Some((2, _)), .. }), "{err}");
        let err = edit_file(&scope, &edits(&[("let sum = a + b;", "x")]), &mut reads).expect_err("unrelated");
        assert!(matches!(err, ToolError::EditTextNotFound { nearest: None, .. }), "{err}");
    }

    /// A model writes `\n` whatever the file uses: the edit takes the file's
    /// line ending, in the anchor and in the replacement alike.
    #[test]
    fn an_edit_takes_the_line_ending_of_the_file() {
        let (scope, root, mut reads) = fixture("edit-to-crlf", "a: 1\r\nb: 2\r\nc: 3\r\n");
        edit_file(&scope, &edits(&[("a: 1\nb: 2", "a: 1\nb: 20\nb2: 21")]), &mut reads).expect("an LF edit in a CRLF file");
        assert_eq!(on_disk(&root), "a: 1\r\nb: 20\r\nb2: 21\r\nc: 3\r\n");

        let (scope, root, mut reads) = fixture("edit-to-lf", "a: 1\nb: 2\n");
        edit_file(&scope, &edits(&[("a: 1\r\nb: 2", "x\r\ny")]), &mut reads).expect("a CRLF edit in an LF file");
        assert_eq!(on_disk(&root), "x\ny\n");

        // Already right stays right: no `\r\r\n`.
        let (scope, root, mut reads) = fixture("edit-crlf-to-crlf", "a: 1\r\nb: 2\r\n");
        edit_file(&scope, &edits(&[("a: 1\r\nb: 2", "x\r\ny")]), &mut reads).expect("a CRLF edit in a CRLF file");
        assert_eq!(on_disk(&root), "x\r\ny\r\n");
    }

    /// A single-line edit in a CRLF file keeps its multi-line replacement in
    /// CRLF too, not a mix.
    #[test]
    fn a_one_line_anchor_in_a_crlf_file_gets_a_crlf_replacement() {
        let (scope, root, mut reads) = fixture("edit-one-line-crlf", "a: 1\r\nb: 2\r\n");
        edit_file(&scope, &edits(&[("a: 1", "a: 1\nz: 0")]), &mut reads).expect("one-line anchor");
        assert_eq!(on_disk(&root), "a: 1\r\nz: 0\r\nb: 2\r\n");
    }

    /// A file with both endings leaves which one was meant unknowable: the
    /// mismatch is named rather than guessed, and a `\r` does not show in an
    /// error, so it has to be.
    #[test]
    fn a_line_ending_mismatch_in_a_mixed_file_is_named_not_guessed() {
        let (scope, root, mut reads) = fixture("edit-mixed", "a: 1\r\nb: 2\r\nc: 3\n");
        let err = edit_file(&scope, &edits(&[("a: 1\nb: 2", "x")]), &mut reads).expect_err("LF anchor");
        assert!(matches!(err, ToolError::EditLineEndings { file: "CRLF", edit: "LF" }), "{err}");
        assert_eq!(on_disk(&root), "a: 1\r\nb: 2\r\nc: 3\n", "left untouched");

        let (scope, _, mut reads) = fixture("edit-mixed-lf", "a: 1\nb: 2\nc: 3\r\n");
        let err = edit_file(&scope, &edits(&[("a: 1\r\nb: 2", "x")]), &mut reads).expect_err("CRLF anchor");
        assert!(matches!(err, ToolError::EditLineEndings { file: "LF", edit: "CRLF" }), "{err}");

        let (scope, _, mut reads) = fixture("edit-absent", "a: 1\nb: 2\n");
        let err = edit_file(&scope, &edits(&[("a: 9\r\nb: 2", "x")]), &mut reads).expect_err("absent");
        assert!(matches!(err, ToolError::EditTextNotFound { .. }), "{err}");
    }

    /// Whitespace at the end of a line is invisible in a read: the anchor
    /// still matches without it, and only the matched lines lose theirs.
    #[test]
    fn an_anchor_matches_past_trailing_whitespace_it_could_not_see() {
        let (scope, root, mut reads) = fixture("edit-trailing", "fn a() {  \n    x();\t\n}\nkeep  \n");

        edit_file(&scope, &edits(&[("fn a() {\n    x();", "fn b() {\n    y();")]), &mut reads).expect("a loose match");

        assert_eq!(on_disk(&root), "fn b() {\n    y();\n}\nkeep  \n");
    }

    /// The model writes ASCII; the file keeps its typography everywhere the
    /// edit does not reach — and the other way round.
    #[test]
    fn typographic_quotes_dashes_and_spaces_match_their_ascii() {
        let body = "say(\u{201C}hi\u{201D}); // a \u{2013} b\nother \u{2018}q\u{2019}\u{00A0}x\n";
        let (scope, root, mut reads) = fixture("edit-typography", body);
        edit_file(&scope, &edits(&[("say(\"hi\");", "say(\"bye\");")]), &mut reads).expect("curly quotes");
        assert_eq!(on_disk(&root), "say(\"bye\"); // a \u{2013} b\nother \u{2018}q\u{2019}\u{00A0}x\n");

        edit_file(&scope, &edits(&[("'q' x", "'r' x")]), &mut reads).expect("curly quotes and a no-break space");
        assert_eq!(on_disk(&root), "say(\"bye\"); // a \u{2013} b\nother 'r' x\n");

        let (scope, root, mut reads) = fixture("edit-typography-back", "a - b\n");
        edit_file(&scope, &edits(&[("a \u{2014} b", "c")]), &mut reads).expect("an em dash in the anchor");
        assert_eq!(on_disk(&root), "c\n");
    }

    /// Space at the end of an anchor that ends mid-line is the anchor's own:
    /// dropping it would double the space its replacement brings.
    #[test]
    fn an_anchor_ending_mid_line_keeps_its_last_space() {
        let (scope, root, mut reads) = fixture("edit-mid-line", "x - 1\n");

        edit_file(&scope, &edits(&[("x \u{2013} ", "y - ")]), &mut reads).expect("a loose match");

        assert_eq!(on_disk(&root), "y - 1\n");
    }

    /// The map back to the file is by byte: text in another script before the
    /// match moves every offset after it.
    #[test]
    fn a_loose_match_after_multibyte_text_lands_where_it_was_found() {
        let (scope, root, mut reads) = fixture("edit-loose-multibyte", "привет \u{201C}мир\u{201D}\n");

        edit_file(&scope, &edits(&[("\"мир\"", "x")]), &mut reads).expect("curly quotes after Cyrillic");

        assert_eq!(on_disk(&root), "привет x\n");
    }

    /// The end of the file ends a line too.
    #[test]
    fn a_last_line_without_a_break_loses_its_trailing_space_with_the_match() {
        let (scope, root, mut reads) = fixture("edit-loose-eof", "a\nsay(\u{201C}x\u{201D})  ");

        edit_file(&scope, &edits(&[("say(\"x\")", "y")]), &mut reads).expect("at the end of the file");

        assert_eq!(on_disk(&root), "a\ny");
    }

    #[test]
    fn a_loose_match_in_a_crlf_file_keeps_crlf() {
        let (scope, root, mut reads) = fixture("edit-loose-crlf", "a  \r\nb\r\nc\r\n");

        edit_file(&scope, &edits(&[("a\nb", "x\ny")]), &mut reads).expect("trailing spaces before CRLF");

        assert_eq!(on_disk(&root), "x\r\ny\r\nc\r\n");
    }

    /// Loose or not, an anchor names one place and whole words.
    #[test]
    fn a_loose_match_is_held_to_the_same_rules() {
        let (scope, root, mut reads) = fixture("edit-loose-rules", "\"x\"\n\"x\"\nlet total = a \u{2014} b;\n");

        let err = edit_file(&scope, &edits(&[("\u{201C}x\u{201D}", "y")]), &mut reads).expect_err("twice");
        assert!(matches!(err, ToolError::EditTextAmbiguous(_, 2)), "{err}");
        let err = edit_file(&scope, &edits(&[("otal = a - b", "y")]), &mut reads).expect_err("inside a word");
        assert!(matches!(&err, ToolError::EditInsideWord(_, w) if w == "total = a \u{2014} b"), "{err}");
        let err = edit_file(&scope, &edits(&[("a + b", "y")]), &mut reads).expect_err("absent");
        assert!(matches!(err, ToolError::EditTextNotFound { .. }), "{err}");
        assert_eq!(on_disk(&root), "\"x\"\n\"x\"\nlet total = a \u{2014} b;\n", "left untouched");
    }

    /// A unique anchor inside a word would rewrite part of a name: `line on`
    /// in `line one` made `ONEe`.
    #[test]
    fn an_anchor_cutting_into_a_word_is_refused_and_names_the_word() {
        let (scope, root, mut reads) = fixture("edit-word", "line one\nlet aUserId = get();\nmiddle\nпривет мир\nmax_retry_count\n");

        for (old, word) in [("line on", "line one"), ("serId", "aUserId"), ("aUser", "aUserId"), ("ивет", "привет"), ("retry_count", "max_retry_count")] {
            let err = edit_file(&scope, &edits(&[(old, "X")]), &mut reads).expect_err(old);
            assert!(matches!(&err, ToolError::EditInsideWord(o, w) if o == old && w == word), "{old}: {err}");
        }
        assert_eq!(on_disk(&root), "line one\nlet aUserId = get();\nmiddle\nпривет мир\nmax_retry_count\n", "left untouched");

        // Whole words, and edges on punctuation or space, are fine.
        let text = on_disk(&root);
        for old in ["middle", "aUserId", "= get()", "();\nmiddle", "мир"] {
            let start = text.find(old).unwrap();
            assert_eq!(split_word(&text, start, start + old.len()), None, "{old}");
        }
        edit_file(&scope, &edits(&[("middle", "MIDDLE")]), &mut reads).expect("a whole line");
    }

    /// The count is what tells the model the anchor was too short.
    #[test]
    fn an_ambiguous_anchor_reports_how_many_times_it_matched() {
        let (scope, root, mut reads) = fixture("edit-ambiguous", "x = 1;\nx = 1;\nx = 1;\n");

        let err = edit_file(&scope, &edits(&[("x = 1;", "x = 2;")]), &mut reads)
            .expect_err("three candidates");

        assert!(matches!(err, ToolError::EditTextAmbiguous(_, 3)));
        assert!(err.to_string().contains("matched 3 times"), "{err}");
        assert_eq!(on_disk(&root), "x = 1;\nx = 1;\nx = 1;\n", "left untouched");
    }

    /// Two anchors covering the same bytes would make the result depend on
    /// which was applied first, or corrupt one of them outright.
    #[test]
    fn overlapping_edits_reject_the_call() {
        let (scope, root, mut reads) = fixture("edit-overlap", "ab cd ef\n");

        let err = edit_file(&scope, &edits(&[("ab cd", "X"), ("cd ef", "Y")]), &mut reads)
            .expect_err("regions overlap");

        assert!(matches!(err, ToolError::EditsOverlap));
        assert_eq!(on_disk(&root), "ab cd ef\n", "left untouched");
    }

    /// All or nothing: one unusable edit in a batch must not leave the file
    /// half-changed, which is worse than either outcome.
    #[test]
    fn one_bad_edit_in_a_batch_writes_nothing() {
        let (scope, root, mut reads) = fixture("edit-atomic", "alpha\nbeta\n");

        edit_file(&scope, &edits(&[("alpha", "A"), ("nope", "N")]), &mut reads)
            .expect_err("second anchor is missing");

        assert_eq!(on_disk(&root), "alpha\nbeta\n", "the good edit did not land either");
    }

    /// Straight from a grep hit: the anchor is the check, and a read first
    /// would only cost a round. It does not unlock a wholesale write after.
    #[test]
    fn an_unread_file_can_be_edited_but_not_then_replaced() {
        let dir = temp_dir("edit-unread");
        std::fs::write(dir.join("a.txt"), "precious\n").expect("writable");
        let scope = ToolScope::new(&dir).expect("root resolves");
        let mut reads = ReadFiles::default();

        edit_file(&scope, &edits(&[("precious", "kept")]), &mut reads).expect("an anchor is enough");

        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "kept\n");
        assert_eq!(reads.check("a.txt", "kept\n", true), Err(WriteBlocked::ReadInPart));
        let err = edit_file(&scope, &edits(&[("nowhere", "x")]), &mut reads).expect_err("still anchored");
        assert!(matches!(err, ToolError::EditTextNotFound { .. }), "{err}");
    }

    /// The difference from `writeFile`. An anchored edit is content-addressed:
    /// the anchor must be unique across the *whole* file and the hash proves
    /// nothing moved, so the unread parts are as safe as the read ones.
    #[test]
    fn a_partial_read_is_enough_to_edit() {
        let dir = temp_dir("edit-partial");
        let body: String = (1..=100).map(|i| format!("line {i}\n")).collect();
        std::fs::write(dir.join("a.txt"), &body).expect("writable");
        let scope = ToolScope::new(&dir).expect("root resolves");
        let root = scope.root().to_path_buf();
        let mut reads = ReadFiles::default();
        agent_reads(&scope, &mut reads, "a.txt", Some((1, 5)));

        edit_file(&scope, &edits(&[("line 3\n", "LINE 3\n")]), &mut reads)
            .expect("a partial read is enough for an anchored edit");

        assert!(on_disk(&root).contains("LINE 3\n"));
    }

    /// Editing does not teach the agent what the rest of the file says, so it
    /// must not unlock a wholesale replacement afterwards.
    #[test]
    fn editing_after_a_partial_read_does_not_unlock_replacing() {
        let dir = temp_dir("edit-no-upgrade");
        let body: String = (1..=100).map(|i| format!("line {i}\n")).collect();
        std::fs::write(dir.join("a.txt"), &body).expect("writable");
        let scope = ToolScope::new(&dir).expect("root resolves");
        let mut reads = ReadFiles::default();
        agent_reads(&scope, &mut reads, "a.txt", Some((1, 5)));
        edit_file(&scope, &edits(&[("line 3\n", "LINE 3\n")]), &mut reads).expect("edits");

        let err = super::super::write_file::write_file(
            &scope,
            &crate::domain::tools::WriteFileArgs {
                path: "a.txt".into(),
                content: "short\n".into(),
            },
            &mut reads,
        )
        .expect_err("still only ever saw five lines");

        assert!(matches!(err, ToolError::FileReadInPart(_)));
    }

    #[test]
    fn editing_over_someone_elses_change_is_refused() {
        let (scope, root, mut reads) = fixture("edit-stale", "original\n");
        std::fs::write(root.join("a.txt"), "edited by a human\n").expect("writable");

        let err = edit_file(&scope, &edits(&[("human", "robot")]), &mut reads)
            .expect_err("changed under it");

        assert!(matches!(err, ToolError::FileChangedSinceRead(_)));
        assert_eq!(on_disk(&root), "edited by a human\n", "left untouched");
    }

    /// The registry is refreshed by the write, so a follow-up edit is not
    /// mistaken for one made against a stale read.
    #[test]
    fn consecutive_edits_are_allowed() {
        let (scope, root, mut reads) = fixture("edit-twice", "one\ntwo\n");

        edit_file(&scope, &edits(&[("one", "ONE")]), &mut reads).expect("first");
        edit_file(&scope, &edits(&[("two", "TWO")]), &mut reads).expect("second");

        assert_eq!(on_disk(&root), "ONE\nTWO\n");
    }

    #[test]
    fn editing_a_file_that_does_not_exist_is_not_a_way_to_create_one() {
        let dir = temp_dir("edit-absent");
        let scope = ToolScope::new(&dir).expect("root resolves");
        assert!(matches!(
            edit_file(&scope, &edits(&[("a", "b")]), &mut ReadFiles::default()),
            Err(ToolError::NotFound(_))
        ));
    }

    #[test]
    fn a_path_leaving_the_root_is_refused() {
        let (scope, _root, mut reads) = fixture("edit-escape", "body\n");
        let outside = EditFileArgs {
            path: "../a.txt".to_string(),
            ..edits(&[("a", "b")])
        };
        assert!(matches!(
            edit_file(&scope, &outside, &mut reads),
            Err(ToolError::PathEscape(_))
        ));
    }

    /// Byte ranges come from `match_indices`, so they always land on character
    /// boundaries — splicing around a multi-byte anchor must not panic.
    #[test]
    fn a_multibyte_anchor_splices_cleanly() {
        let (scope, root, mut reads) = fixture("edit-multibyte", "привет мир\n");

        edit_file(&scope, &edits(&[("мир", "世界")]), &mut reads).expect("edits");

        assert_eq!(on_disk(&root), "привет 世界\n");
    }

    #[test]
    fn the_call_wire_shape_is_stable_and_counts_as_risky() {
        let json =
            r#"{"tool": "editFile", "args": {"path": "a.txt", "edits": [{"old": "x", "new": "y"}]}}"#;
        let call: ToolCall = serde_json::from_str(json).expect("parses");
        assert!(call.is_risky(), "an edit needs a human");

        let (scope, root, mut reads) = fixture("edit-dispatch", "x\n");
        assert!(matches!(
            execute_tool(&scope, &call, &mut reads, &mut Vec::new(), &ToolDeps::default()),
            Ok(ToolResult::FileEdited { .. })
        ));
        assert_eq!(on_disk(&root), "y\n");
    }
}
