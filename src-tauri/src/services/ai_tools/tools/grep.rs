//! `grep` — exhaustive regex line matching, for when the model needs every
//! occurrence rather than the best few ranked guesses.
//!
//! Alfa Atlas split this between `tools/grep.rs` and `services/docs_search.rs`
//! because a user-facing IPC shared the search. There is no second consumer
//! here, so it is one module; the split earns its keep again the day the UI
//! grows its own search.

use crate::domain::llm::LlmToolDefinition;
use std::fs;
use std::path::{Path, PathBuf};

use regex::RegexBuilder;

use crate::domain::search_query::PathFilter;
use crate::domain::tools::{GrepArgs, GrepMatch, ToolError, ToolResult, ToolScope};
use crate::infra::workspace_scanner;

use super::super::resolve::{relative_to_root, resolve_existing};

const DEFAULT_RESULTS: usize = 50;
const MAX_RESULTS: usize = 200;
/// Past this, a file is a build artefact, a bundle or a dump — never something
/// a person wrote and asked about.
const MAX_FILE_BYTES: u64 = 1_048_576;
/// A NUL byte in the first block means binary. Cheap, and wrong only for files
/// that are already unreadable as text.
const BINARY_SNIFF_BYTES: usize = 8_192;
/// One minified line can be the whole file. The hit's value is its location,
/// not its full text.
const LINE_MAX_CHARS: usize = 300;
/// Enough to see what a hit sits inside — a signature, the branch around it —
/// without turning a search into a file dump nobody asked for.
const MAX_CONTEXT_LINES: usize = 5;
/// Hits counted past the cap before counting stops: enough to tell "a few
/// more" from "narrow this down", without reading a vendored tree to the end.
const MAX_COUNTED: usize = 10_000;

pub fn grep(scope: &ToolScope, args: &GrepArgs) -> Result<ToolResult, ToolError> {
    let max_results = args
        .max_results
        .unwrap_or(DEFAULT_RESULTS)
        .clamp(1, MAX_RESULTS);
    let context_lines = args.context_lines.unwrap_or(0).min(MAX_CONTEXT_LINES);

    let pattern = RegexBuilder::new(&args.pattern)
        .case_insensitive(args.case_insensitive.unwrap_or(false))
        .build()
        .map_err(|e| ToolError::InvalidRegex(e.to_string()))?;

    let paths = PathFilter::new(args.glob.as_deref(), args.exclude.as_deref()).map_err(ToolError::InvalidPattern)?;

    let mut search = Search {
        pattern,
        paths,
        max_results,
        context_lines,
        files_only: args.files_only.unwrap_or(false),
        matches: Vec::new(),
        total: 0,
        files: std::collections::HashSet::new(),
        skipped: Vec::new(),
    };

    match target(scope, args.path.as_deref())? {
        Target::File(path) => {
            let rel = relative_to_root(scope, &path)?;
            search.file(&path, &rel);
        }
        Target::Dir(dir) => {
            for scanned in workspace_scanner::scan_files(&dir, None).map_err(ToolError::Io)? {
                let Ok(rel) = relative_to_root(scope, &scanned.path) else {
                    continue;
                };
                search.file(&scanned.path, &rel);
                if search.total >= MAX_COUNTED {
                    break;
                }
            }
        }
    }

    let shown = if search.files_only { search.files.len() } else { search.total };
    Ok(ToolResult::GrepResults {
        truncated: shown > search.matches.len(),
        total: search.total,
        total_files: search.files.len(),
        total_is_floor: search.total >= MAX_COUNTED,
        skipped: search.skipped,
        matches: search.matches,
    })
}

enum Target {
    Dir(PathBuf),
    File(PathBuf),
}

fn target(scope: &ToolScope, path: Option<&str>) -> Result<Target, ToolError> {
    let Some(path) = path.filter(|p| !p.is_empty() && *p != ".") else {
        return Ok(Target::Dir(scope.root().to_path_buf()));
    };
    let resolved = resolve_existing(scope, path)?;
    if resolved.is_file() {
        Ok(Target::File(resolved))
    } else {
        Ok(Target::Dir(resolved))
    }
}

struct Search {
    pattern: regex::Regex,
    paths: PathFilter,
    max_results: usize,
    context_lines: usize,
    files_only: bool,
    matches: Vec<GrepMatch>,
    /// Every hit, kept or only counted.
    total: usize,
    files: std::collections::HashSet<String>,
    skipped: Vec<String>,
}

impl Search {
    /// Keeps this file's hits up to the cap and counts the rest.
    fn file(&mut self, absolute: &Path, relative: &str) {
        if !self.paths.allows(relative) {
            return;
        }
        let Ok(meta) = fs::metadata(absolute) else {
            return;
        };
        if meta.len() > MAX_FILE_BYTES {
            self.skipped.push(relative.to_string());
            return;
        }
        let Ok(bytes) = fs::read(absolute) else {
            return;
        };
        if bytes[..BINARY_SNIFF_BYTES.min(bytes.len())].contains(&0) {
            return;
        }
        // Text in another encoding — cp1251 in an older Russian project —
        // could hold the very line asked for; said, not dropped.
        let Ok(content) = String::from_utf8(bytes) else {
            self.skipped.push(relative.to_string());
            return;
        };

        // Collected up front so context can look backwards. The no-context
        // case walks the same slice forwards and pays only for the borrows.
        let lines: Vec<&str> = content.lines().collect();
        for (idx, line) in lines.iter().enumerate() {
            if !self.pattern.is_match(line) {
                continue;
            }
            self.total += 1;
            self.files.insert(relative.to_string());
            if self.total >= MAX_COUNTED {
                return;
            }
            // The cap counts *hits*, not lines: context belongs to a hit, so
            // asking for context never silently returns fewer results.
            if self.matches.len() >= self.max_results
                || (self.files_only && self.matches.last().is_some_and(|m| m.path == relative))
            {
                continue;
            }
            let (before, after) = if self.context_lines == 0 {
                (Vec::new(), Vec::new())
            } else {
                (
                    lines[idx.saturating_sub(self.context_lines)..idx]
                        .iter()
                        .map(|l| truncate(l))
                        .collect(),
                    lines[(idx + 1).min(lines.len())
                        ..(idx + 1 + self.context_lines).min(lines.len())]
                        .iter()
                        .map(|l| truncate(l))
                        .collect(),
                )
            };
            self.matches.push(GrepMatch {
                path: relative.to_string(),
                line: (idx + 1) as u32,
                text: truncate(line),
                before,
                after,
            });
        }
    }
}

/// Counts characters, not bytes — this text is arbitrary UTF-8, and slicing it
/// by byte index would panic partway through a multi-byte character.
fn truncate(line: &str) -> String {
    if line.chars().count() <= LINE_MAX_CHARS {
        return line.to_string();
    }
    let kept: String = line.chars().take(LINE_MAX_CHARS).collect();
    format!("{kept}…")
}

/// What the model is told `grep` is for.
pub(super) fn definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "grep".to_string(),
        description: "Search file contents by regular expression, line by line: a pattern cannot span lines. Git-ignored files (build output, dependencies) are not searched. Use it when you know what the code says; use listFiles when you know where it lives. Results carry the path and line number in the spelling readFile takes, so a hit can be read without editing the path. Past maxResults the rest are still counted, so the result says how many there are in all; files over 1 MB or not UTF-8 text are not searched and are named."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Rust `regex` syntax: no backreferences and no lookaround. Escape regex metacharacters to search for them literally."
                },
                "path": {
                    "type": [
                        "string",
                        "null"
                    ],
                    "description": "A file or subdirectory to search under, relative to the workspace root. Omit, \".\" or \"\" searches everything."
                },
                "glob": {
                    "type": [
                        "string",
                        "null"
                    ],
                    "description": "Which files to search. Without a `/` it matches the file *name* at any depth — \"*.rs\"; with one it matches the path from the workspace root — \"src/main/**/*.java\"."
                },
                "exclude": {
                    "type": [
                        "string",
                        "null"
                    ],
                    "description": "Files to leave out, as a glob over the path from the workspace root — \"src/docs/**\" drops documentation from a code search."
                },
                "caseInsensitive": {
                    "type": [
                        "boolean",
                        "null"
                    ],
                    "description": "Default false: an exact, case-sensitive match."
                },
                "maxResults": {
                    "type": [
                        "integer",
                        "null"
                    ],
                    "minimum": 1,
                    "description": "Cap on the number of matches: 50 when absent, at most 200. The result says whether it was reached, so a capped search is never mistaken for an exhaustive one."
                },
                "contextLines": {
                    "type": [
                        "integer",
                        "null"
                    ],
                    "minimum": 0,
                    "description": "Lines of context around each hit, at most 5. Omit or 0 returns the matching line alone; 2-3 usually answers \"what does this line do\" without a follow-up readFile."
                },
                "filesOnly": {
                    "type": [
                        "boolean",
                        "null"
                    ],
                    "description": "Only the first hit of each file — which files use a name, not every use. maxResults then counts files."
                }
            },
            "required": [
                "pattern"
            ]
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::tools::{ReadFiles, ToolCall, ToolDeps};
    use crate::services::ai_tools::tools::execute_tool;
    use crate::testing::temp_dir;

    fn write(root: &Path, relative: &str, body: &str) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().expect("has a parent")).expect("dirs are creatable");
        std::fs::write(path, body).expect("file is writable");
    }

    fn args(pattern: &str) -> GrepArgs {
        GrepArgs {
            pattern: pattern.to_string(),
            ..GrepArgs::default()
        }
    }

    fn run(scope: &ToolScope, args: &GrepArgs) -> (Vec<GrepMatch>, bool) {
        match grep(scope, args).expect("search runs") {
            ToolResult::GrepResults { matches, truncated, .. } => (matches, truncated),
            other => panic!("unexpected {other:?}"),
        }
    }

    fn fixture(label: &str) -> (ToolScope, PathBuf) {
        let dir = temp_dir(label);
        let scope = ToolScope::new(&dir).expect("root resolves");
        let root = scope.root().to_path_buf();
        (scope, root)
    }

    #[test]
    fn finds_hits_with_line_numbers_and_round_trippable_paths() {
        let (scope, root) = fixture("grep-basic");
        write(&root, "src/main.rs", "fn main() {\n    todo!()\n}\n");
        write(&root, "src/lib.rs", "// nothing here\n");

        let (matches, truncated) = run(&scope, &args(r"todo!"));

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].path, "src/main.rs");
        assert_eq!(matches[0].line, 2);
        assert_eq!(matches[0].text, "    todo!()");
        assert!(!truncated);
    }

    #[test]
    fn the_pattern_is_a_regex_not_a_literal() {
        let (scope, root) = fixture("grep-regex");
        write(&root, "a.rs", "fn alpha() {}\nfn beta() {}\nlet alpha = 1;\n");

        let (matches, _) = run(&scope, &args(r"^fn \w+\(\)"));

        assert_eq!(matches.len(), 2);
        assert_eq!(matches.iter().map(|m| m.line).collect::<Vec<_>>(), [1, 2]);
    }

    #[test]
    fn case_sensitivity_is_opt_out() {
        let (scope, root) = fixture("grep-case");
        write(&root, "a.txt", "Alpha\nalpha\n");

        assert_eq!(run(&scope, &args("alpha")).0.len(), 1);

        let insensitive = GrepArgs {
            case_insensitive: Some(true),
            ..args("alpha")
        };
        assert_eq!(run(&scope, &insensitive).0.len(), 2);
    }

    /// The glob matches the file *name*. A pattern like `*.rs` would match no
    /// path at all if it were applied to `src/main.rs`.
    #[test]
    fn the_glob_filters_on_the_file_name() {
        let (scope, root) = fixture("grep-glob");
        write(&root, "src/main.rs", "needle\n");
        write(&root, "src/notes.md", "needle\n");

        let scoped = GrepArgs {
            glob: Some("*.rs".to_string()),
            ..args("needle")
        };
        let (matches, _) = run(&scope, &scoped);

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].path, "src/main.rs");
    }

    /// A glob with a `/` names a place; `exclude` takes one away. Together
    /// they say "the Java under main, not the docs".
    #[test]
    fn a_glob_with_a_slash_and_an_exclude_work_on_the_path() {
        let (scope, root) = fixture("grep-glob-path");
        write(&root, "src/main/A.java", "needle\n");
        write(&root, "src/test/B.java", "needle\n");
        write(&root, "src/docs/c.adoc", "needle\n");

        let paths = |args: GrepArgs| -> Vec<String> { run(&scope, &args).0.into_iter().map(|m| m.path).collect() };

        let under_main = GrepArgs { glob: Some("src/main/**".to_string()), ..args("needle") };
        assert_eq!(paths(under_main), ["src/main/A.java"]);

        let not_docs = GrepArgs { exclude: Some("src/docs/**".to_string()), ..args("needle") };
        let mut kept = paths(not_docs);
        kept.sort();
        assert_eq!(kept, ["src/main/A.java", "src/test/B.java"]);
    }

    #[test]
    fn context_lines_come_back_around_the_hit() {
        let (scope, root) = fixture("grep-context");
        write(&root, "a.txt", "one\ntwo\nNEEDLE\nfour\nfive\n");

        let with_context = GrepArgs {
            context_lines: Some(1),
            ..args("NEEDLE")
        };
        let (matches, _) = run(&scope, &with_context);

        assert_eq!(matches[0].before, ["two"]);
        assert_eq!(matches[0].after, ["four"]);
    }

    /// A hit at line 1 or at EOF must not reach outside the file.
    #[test]
    fn context_at_the_edges_does_not_run_off_the_file() {
        let (scope, root) = fixture("grep-context-edge");
        write(&root, "a.txt", "NEEDLE\nmiddle\nNEEDLE\n");

        let with_context = GrepArgs {
            context_lines: Some(5),
            ..args("NEEDLE")
        };
        let (matches, _) = run(&scope, &with_context);

        assert!(matches[0].before.is_empty(), "nothing precedes line 1");
        assert_eq!(matches[0].after, ["middle", "NEEDLE"]);
        assert_eq!(matches[1].before, ["NEEDLE", "middle"]);
        assert!(matches[1].after.is_empty(), "nothing follows the last line");
    }

    /// The cap counts hits, not lines — so asking for context never silently
    /// costs results. With 3 context lines per hit, a line-counting cap would
    /// return a quarter of what was asked for.
    #[test]
    fn the_cap_counts_hits_not_lines() {
        let (scope, root) = fixture("grep-cap");
        let body: String = (0..20).map(|i| format!("NEEDLE {i}\n")).collect();
        write(&root, "a.txt", &body);

        let capped = GrepArgs {
            max_results: Some(5),
            context_lines: Some(3),
            ..args("NEEDLE")
        };
        let (matches, truncated) = run(&scope, &capped);

        assert_eq!(matches.len(), 5);
        assert!(truncated, "there were more hits than the cap");
    }

    /// Past the cap hits are still counted, with their files; text that
    /// could not be searched is named, binary is not.
    #[test]
    fn a_cut_says_how_much_it_cut_and_what_was_not_searched() {
        let (scope, root) = fixture("grep-total");
        write(&root, "a.txt", &"NEEDLE\n".repeat(20));
        write(&root, "b.txt", "NEEDLE\n");
        write(&root, "big.txt", &"NEEDLE\n".repeat(200_000));
        std::fs::write(root.join("cp1251.txt"), [0xCF, 0xF0, 0xE8, b'\n']).unwrap();
        std::fs::write(root.join("bin.dat"), [0u8, 1, 2]).unwrap();

        let capped = GrepArgs { max_results: Some(5), ..args("NEEDLE") };
        let ToolResult::GrepResults { matches, truncated, total, total_files, total_is_floor, mut skipped } =
            grep(&scope, &capped).unwrap()
        else {
            panic!()
        };

        assert_eq!((matches.len(), truncated, total, total_files, total_is_floor), (5, true, 21, 2, false));
        skipped.sort();
        assert_eq!(skipped, ["big.txt", "cp1251.txt"]);
    }

    /// filesOnly: a file once, by its first hit, while every hit is still
    /// counted; the cap counts files, and a cut is a cut of files.
    #[test]
    fn files_only_shows_each_file_once_and_caps_files() {
        let (scope, root) = fixture("grep-files-only");
        write(&root, "a.txt", "x\nNEEDLE 1\nNEEDLE 2\n");
        write(&root, "b.txt", "NEEDLE 3\n");
        write(&root, "c.txt", "NEEDLE 4\nNEEDLE 5\n");

        let all = GrepArgs { files_only: Some(true), ..args("NEEDLE") };
        let ToolResult::GrepResults { matches, truncated, total, total_files, .. } = grep(&scope, &all).unwrap() else { panic!() };
        let mut seen: Vec<_> = matches.iter().map(|m| (m.path.as_str(), m.line)).collect();
        seen.sort();
        assert_eq!(seen, [("a.txt", 2), ("b.txt", 1), ("c.txt", 1)]);
        assert_eq!((truncated, total, total_files), (false, 5, 3), "every file shown, every hit counted");

        let capped = GrepArgs { max_results: Some(2), ..all };
        let (matches, truncated) = run(&scope, &capped);
        assert_eq!(matches.len(), 2);
        assert!(truncated, "a third file was left out");
    }

    /// Counting stops somewhere, and says so.
    #[test]
    fn counting_stops_at_its_limit_and_says_the_total_is_a_floor() {
        let (scope, root) = fixture("grep-floor");
        write(&root, "a.txt", &"NEEDLE\n".repeat(MAX_COUNTED / 2));
        write(&root, "b.txt", &"NEEDLE\n".repeat(MAX_COUNTED));
        // Past the limit, the rest of the tree is not read.
        write(&root, "c.txt", "NEEDLE\n");

        let ToolResult::GrepResults { total, total_is_floor, .. } = grep(&scope, &args("NEEDLE")).unwrap() else { panic!() };

        assert_eq!((total, total_is_floor), (MAX_COUNTED, true));
    }

    /// `truncated` must be false when the cap happens to equal the hit count —
    /// "exactly this many" is not "there may be more".
    #[test]
    fn an_exact_fit_is_not_reported_as_truncated() {
        let (scope, root) = fixture("grep-exact");
        write(&root, "a.txt", "NEEDLE\nNEEDLE\n");

        let capped = GrepArgs {
            max_results: Some(2),
            ..args("NEEDLE")
        };
        let (matches, truncated) = run(&scope, &capped);

        assert_eq!(matches.len(), 2);
        assert!(!truncated);
    }

    #[test]
    fn binary_and_oversized_files_are_skipped() {
        let (scope, root) = fixture("grep-skip");
        write(&root, "text.txt", "NEEDLE\n");
        std::fs::write(root.join("blob.bin"), b"NEEDLE\x00\x01\x02").expect("writable");
        let huge = format!("NEEDLE\n{}", "x".repeat(MAX_FILE_BYTES as usize));
        std::fs::write(root.join("huge.txt"), huge).expect("writable");

        let (matches, _) = run(&scope, &args("NEEDLE"));

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].path, "text.txt");
    }

    #[test]
    fn a_path_narrows_the_search_to_one_file_or_subtree() {
        let (scope, root) = fixture("grep-path");
        write(&root, "src/a.rs", "NEEDLE\n");
        write(&root, "tests/b.rs", "NEEDLE\n");

        let in_dir = GrepArgs {
            path: Some("src".to_string()),
            ..args("NEEDLE")
        };
        assert_eq!(run(&scope, &in_dir).0.len(), 1);

        let in_file = GrepArgs {
            path: Some("tests/b.rs".to_string()),
            ..args("NEEDLE")
        };
        let (matches, _) = run(&scope, &in_file);
        assert_eq!(matches[0].path, "tests/b.rs");
    }

    #[test]
    fn a_path_leaving_the_root_is_refused() {
        let (scope, _) = fixture("grep-escape");
        let outside = GrepArgs {
            path: Some("../elsewhere".to_string()),
            ..args("NEEDLE")
        };
        assert!(matches!(grep(&scope, &outside), Err(ToolError::PathEscape(_))));
    }

    #[test]
    fn an_invalid_pattern_says_so_instead_of_matching_nothing() {
        let (scope, _) = fixture("grep-badre");
        assert!(matches!(
            grep(&scope, &args("(unclosed")),
            Err(ToolError::InvalidRegex(_))
        ));
    }

    /// Truncation counts characters. Cutting this line at byte 300 would land
    /// mid-character and panic.
    #[test]
    fn a_long_line_is_cut_on_a_character_boundary() {
        let (scope, root) = fixture("grep-multibyte");
        let long = format!("NEEDLE {}", "ы".repeat(400));
        write(&root, "a.txt", &format!("{long}\n"));

        let (matches, _) = run(&scope, &args("NEEDLE"));

        assert_eq!(matches[0].text.chars().count(), LINE_MAX_CHARS + 1);
        assert!(matches[0].text.ends_with('…'));
    }

    /// A plain search's wire shape must not grow empty arrays just because the
    /// context feature exists.
    #[test]
    fn empty_context_is_absent_from_the_wire_form() {
        let hit = GrepMatch {
            path: "a.txt".into(),
            line: 1,
            text: "x".into(),
            before: Vec::new(),
            after: Vec::new(),
        };
        let json = serde_json::to_string(&hit).expect("serializes");
        assert_eq!(json, r#"{"path":"a.txt","line":1,"text":"x"}"#);
    }

    #[test]
    fn the_call_wire_shape_is_stable() {
        let json = r#"{"tool": "grep", "args": {"pattern": "todo", "maxResults": "10"}}"#;
        let call: ToolCall = serde_json::from_str(json).expect("parses, quoted cap and all");
        let ToolCall::Grep(parsed) = &call else {
            panic!("wrong variant")
        };
        assert_eq!(parsed.max_results, Some(10));
        assert!(!call.is_risky(), "searching changes nothing");

        let (scope, root) = fixture("grep-dispatch");
        write(&root, "a.txt", "todo\n");
        assert!(matches!(
            execute_tool(&scope, &call, &mut ReadFiles::default(), &mut Vec::new(), &ToolDeps::default()),
            Ok(ToolResult::GrepResults { .. })
        ));
    }
}
