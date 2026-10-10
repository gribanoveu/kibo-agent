//! `listFiles` — the directory listing, rendered for the model as an indented
//! tree rather than a flat array, so nesting is visible at a glance instead of
//! being reconstructed from N separate path strings.
//!
//! One branch, where Alfa Atlas had four: a documentation-only listing through
//! its own tree walker, a full-repository one, the virtual `@deps/` directory,
//! and a listing inside one external root. Three of those went with the
//! two-root model and `@deps`. `is_dependency_noise` — which hid `test/`,
//! `.min.js` and sourcemaps inside external roots — went with them, and it
//! deliberately never applied to the repository itself: a repo's own `test/`
//! directory is exactly what someone may be asking about.

use crate::domain::llm::LlmToolDefinition;
use crate::domain::tools::{ListFilesArgs, ToolError, ToolFileEntry, ToolResult, ToolScope};
use crate::infra::workspace_scanner;

use super::read_file::MAX_READ_BYTES;
use super::super::resolve::{basename, relative_to_root, resolve_existing};

/// How many entries one call may return.
///
/// Far above any listing a person would read, far below what costs real
/// context: a vendored `node_modules` is tens of thousands of entries, and
/// serializing that tree would spend more of the model's window than the whole
/// conversation around it.
const MAX_ENTRIES: usize = 1000;

pub fn list_files(scope: &ToolScope, args: &ListFilesArgs) -> Result<ToolResult, ToolError> {
    let dir = match args.path.as_deref().filter(|p| !p.is_empty() && *p != ".") {
        None => scope.root().to_path_buf(),
        Some(path) => {
            let resolved = resolve_existing(scope, path)?;
            if !resolved.is_dir() {
                return Err(ToolError::NotFound(path.to_string()));
            }
            resolved
        }
    };

    // `path` itself is depth 0, so this would list nothing and read as "no
    // files" — refused, with the depth that was meant.
    if args.depth == Some(0) {
        return Err(ToolError::InvalidArguments {
            tool: "listFiles".into(),
            reason: "depth 0 is the folder itself and lists nothing; use 1 for its entries, or omit depth".into(),
        });
    }
    let scanned = workspace_scanner::scan_entries(&dir, args.depth.map(|d| d as usize))
        .map_err(ToolError::Io)?;
    // A folder at the last level walked, with something in it, was not
    // opened: what is below it is unknown, not absent.
    let stopped_at = args.depth.filter(|&depth| {
        scanned.iter().any(|entry| {
            entry.is_dir
                && entry.path.strip_prefix(&dir).is_ok_and(|rel| rel.components().count() == depth as usize)
                && std::fs::read_dir(&entry.path).is_ok_and(|mut inside| inside.next().is_some())
        })
    });

    let mut entries: Vec<ToolFileEntry> = scanned
        .into_iter()
        .filter_map(|entry| {
            relative_to_root(scope, &entry.path)
                .ok()
                .map(|path| ToolFileEntry {
                    path,
                    is_dir: entry.is_dir,
                    size: None,
                })
        })
        .collect();

    if let Some(pattern) = args.pattern.as_deref().filter(|p| !p.is_empty()) {
        let matcher = globset::Glob::new(pattern)
            .map(|g| g.compile_matcher())
            .map_err(|e| ToolError::InvalidPattern(e.to_string()))?;
        entries.retain(|e| e.is_dir || matcher.is_match(basename(&e.path)));
        // Only the folders on the way to a match: the rest are empty
        // branches that read as if something were in them.
        let files: Vec<String> = entries.iter().filter(|e| !e.is_dir).map(|e| e.path.clone()).collect();
        entries.retain(|e| !e.is_dir || files.iter().any(|f| f.starts_with(&format!("{}/", e.path))));
    }

    // After the pattern, deliberately — narrowing the request is then a way
    // past the cap rather than a filter over an already-truncated list.
    let truncated = entries.len() > MAX_ENTRIES;
    entries.truncate(MAX_ENTRIES);
    // After the cap: at most one stat per entry shown.
    for entry in entries.iter_mut().filter(|e| !e.is_dir) {
        entry.size = std::fs::metadata(scope.root().join(&entry.path))
            .map(|meta| meta.len())
            .ok()
            .filter(|&len| len > MAX_READ_BYTES as u64);
    }

    Ok(ToolResult::FileList { entries, truncated, stopped_at })
}

/// The listing as the model sees it: an indented tree in the style of
/// `tree(1)`.
///
/// The first line is always `./` — the scope root, never the on-disk folder
/// name, so the model does not mistake the directory it is working in for a
/// child to prepend onto every path.
///
/// Called when a result is serialized for the model, which arrives with the
/// chat loop; the structured `entries` stay the form the UI renders.
pub fn render_file_tree(entries: &[ToolFileEntry], truncated: bool) -> String {
    let mut root = Node::default();
    for entry in entries {
        let parts: Vec<&str> = entry.path.split('/').filter(|p| !p.is_empty()).collect();
        let Some((last, dirs)) = parts.split_last() else {
            continue;
        };
        let mut node = &mut root;
        for part in dirs {
            node = node.children.entry((*part).to_string()).or_default();
        }
        let leaf = node.children.entry((*last).to_string()).or_default();
        leaf.is_file = !entry.is_dir;
        leaf.size = entry.size;
    }

    let mut out = String::from("./\n");
    render_children(&root, "", &mut out);
    if truncated {
        out.push_str(&format!(
            "\n[showing the first {} entries — the listing is longer. Narrow it with path, pattern (e.g. \"*.rs\") or depth.]\n",
            entries.len()
        ));
    }
    out
}

/// One directory level. `BTreeMap` so the rendered tree is deterministic
/// whatever order the walk produced. `is_file` is meaningful only on a leaf: an
/// intermediate segment, inferred from some deeper entry's path and never
/// listed in its own right, always renders as a directory.
#[derive(Default)]
struct Node {
    children: std::collections::BTreeMap<String, Node>,
    is_file: bool,
    size: Option<u64>,
}

fn render_children(node: &Node, prefix: &str, out: &mut String) {
    let count = node.children.len();
    for (i, (name, child)) in node.children.iter().enumerate() {
        let is_last = i + 1 == count;
        let is_dir = !child.children.is_empty() || !child.is_file;
        out.push_str(prefix);
        out.push_str(if is_last { "└── " } else { "├── " });
        out.push_str(name);
        if is_dir {
            out.push('/');
        }
        if let Some(bytes) = child.size {
            out.push_str(&format!("  ({})", human_size(bytes)));
        }
        out.push('\n');
        if is_dir {
            let child_prefix = format!("{prefix}{}", if is_last { "    " } else { "│   " });
            render_children(child, &child_prefix, out);
        }
    }
}

/// `340 KB`, `52.1 MB` — a size to decide by, not to account with.
fn human_size(bytes: u64) -> String {
    match bytes {
        0..1_000_000 => format!("{} KB", bytes / 1000),
        1_000_000..1_000_000_000 => format!("{:.1} MB", bytes as f64 / 1e6),
        _ => format!("{:.1} GB", bytes as f64 / 1e9),
    }
}

/// What the model is told `listFiles` is for.
pub(super) fn definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "listFiles".to_string(),
        description: "List the files and directories under a path, as a tree. Ignored files (.gitignore, and .git itself) are never listed. Use it to learn a project's shape before reading; use grep when you already know what to look for. A file larger than one readFile returns (100 KB) shows its size — grep it, or read a range. At most 1000 entries; the result says when it was cut."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": [
                        "string",
                        "null"
                    ],
                    "description": "Subdirectory relative to the workspace root. Omit or \".\" lists the root."
                },
                "depth": {
                    "type": [
                        "integer",
                        "null"
                    ],
                    "minimum": 1,
                    "description": "Levels below `path`: its direct children are 1. Omit for unlimited. Start shallow on an unfamiliar repository. With a pattern, prefer no depth: files below it are not searched, and the result says when folders were left unopened."
                },
                "pattern": {
                    "type": [
                        "string",
                        "null"
                    ],
                    "description": "Glob over each entry's file *name*, never its full path, so \"*.rs\" matches at any depth; several types at once as \"*.{java,kt}\". Only the directories on the way to a matching file are kept."
                }
            },
            "required": []
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::tools::{ReadFiles, ToolCall, ToolDeps};
    use crate::services::ai_tools::tools::execute_tool;
    use crate::testing::temp_dir;
    use std::path::Path;

    fn write(root: &Path, relative: &str, body: &str) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().expect("has a parent")).expect("dirs are creatable");
        std::fs::write(path, body).expect("file is writable");
    }

    fn fixture(label: &str) -> (ToolScope, std::path::PathBuf) {
        let dir = temp_dir(label);
        let scope = ToolScope::new(&dir).expect("root resolves");
        let root = scope.root().to_path_buf();
        (scope, root)
    }

    fn run(scope: &ToolScope, args: &ListFilesArgs) -> (Vec<ToolFileEntry>, bool) {
        match list_files(scope, args).expect("listing runs") {
            ToolResult::FileList { entries, truncated, .. } => (entries, truncated),
            other => panic!("unexpected {other:?}"),
        }
    }

    fn paths(entries: &[ToolFileEntry]) -> Vec<&str> {
        entries.iter().map(|e| e.path.as_str()).collect()
    }

    /// A file one read would cut shows its size, in the tree the model reads;
    /// a file that fits, and a folder, show none.
    #[test]
    fn a_file_too_large_for_one_read_shows_its_size() {
        let (scope, root) = fixture("list-sizes");
        write(&root, "logs/app.log", &"x".repeat(150_000));
        write(&root, "logs/fits.log", &"x".repeat(MAX_READ_BYTES));
        write(&root, "src/a.rs", "fn a() {}\n");

        let (entries, _) = run(&scope, &ListFilesArgs::default());
        let size = |path: &str| entries.iter().find(|e| e.path == path).and_then(|e| e.size);
        assert_eq!(size("logs/app.log"), Some(150_000));
        assert_eq!((size("logs/fits.log"), size("src/a.rs"), size("logs")), (None, None, None));

        let tree = render_file_tree(&entries, false);
        assert!(tree.contains("app.log  (150 KB)\n"), "{tree}");
        assert!(tree.contains("fits.log\n"), "{tree}");
        assert_eq!((human_size(52_100_000), human_size(3_200_000_000)), ("52.1 MB".into(), "3.2 GB".into()));
    }

    #[test]
    fn lists_files_and_directories_relative_to_the_root() {
        let (scope, root) = fixture("list-basic");
        write(&root, "src/main.rs", "");
        write(&root, "README.md", "");

        let (entries, truncated) = run(&scope, &ListFilesArgs::default());

        assert_eq!(paths(&entries), ["README.md", "src", "src/main.rs"]);
        assert!(entries.iter().find(|e| e.path == "src").unwrap().is_dir);
        assert!(!truncated);
    }

    /// Paths stay relative to the scope root even when a subdirectory was
    /// asked for, so an entry can be handed straight back to `readFile`.
    #[test]
    fn a_subdirectory_listing_still_reports_root_relative_paths() {
        let (scope, root) = fixture("list-subdir");
        write(&root, "src/deep/a.rs", "");
        write(&root, "other/b.rs", "");

        let (entries, _) = run(
            &scope,
            &ListFilesArgs {
                path: Some("src".to_string()),
                ..ListFilesArgs::default()
            },
        );

        assert_eq!(paths(&entries), ["src/deep", "src/deep/a.rs"]);
    }

    #[test]
    fn depth_limits_how_far_the_listing_descends() {
        let (scope, root) = fixture("list-depth");
        write(&root, "top.txt", "");
        write(&root, "one/mid.txt", "");
        write(&root, "one/two/deep.txt", "");

        let at = |d: u32| {
            let args = ListFilesArgs {
                depth: Some(d),
                ..ListFilesArgs::default()
            };
            paths(&run(&scope, &args).0)
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        };

        assert_eq!(at(1), ["one", "top.txt"]);
        assert_eq!(at(2), ["one", "one/mid.txt", "one/two", "top.txt"]);
        let zero = list_files(&scope, &ListFilesArgs { depth: Some(0), ..ListFilesArgs::default() }).unwrap_err();
        assert!(zero.to_string().contains("use 1 for its entries"), "{zero}");
    }

    /// The pattern scopes which *files* come back. Dropping the directories
    /// they live in would leave the model unable to navigate to them.
    #[test]
    fn a_pattern_filters_files_but_keeps_directories() {
        let (scope, root) = fixture("list-pattern");
        write(&root, "src/main.rs", "");
        write(&root, "src/notes.md", "");

        let (entries, _) = run(
            &scope,
            &ListFilesArgs {
                pattern: Some("*.rs".to_string()),
                ..ListFilesArgs::default()
            },
        );

        assert_eq!(paths(&entries), ["src", "src/main.rs"]);
    }

    /// Matching on the name, not the path — otherwise `*.rs` would match
    /// nothing below the top level.
    #[test]
    fn a_pattern_nothing_matches_leaves_no_folders_behind() {
        let (scope, root) = fixture("list-pattern-none");
        write(&root, "src/main.rs", "");

        let (entries, _) = run(
            &scope,
            &ListFilesArgs {
                pattern: Some("*.xyz".to_string()),
                ..ListFilesArgs::default()
            },
        );

        assert!(entries.is_empty(), "{:?}", paths(&entries));
    }

    #[test]
    fn the_pattern_matches_at_any_depth() {
        let (scope, root) = fixture("list-pattern-depth");
        write(&root, "a/b/c/deep.rs", "");

        let (entries, _) = run(
            &scope,
            &ListFilesArgs {
                pattern: Some("*.rs".to_string()),
                ..ListFilesArgs::default()
            },
        );

        assert!(paths(&entries).contains(&"a/b/c/deep.rs"));
    }

    /// Several file types in one pattern — the description promises it.
    #[test]
    fn a_pattern_takes_alternatives() {
        let (scope, root) = fixture("list-pattern-braces");
        write(&root, "a/One.java", "");
        write(&root, "b/Two.kt", "");
        write(&root, "c/three.xml", "");

        let (entries, _) = run(&scope, &ListFilesArgs { pattern: Some("*.{java,kt}".to_string()), ..ListFilesArgs::default() });

        let listed = paths(&entries);
        assert!(listed.contains(&"a/One.java") && listed.contains(&"b/Two.kt"), "{listed:?}");
        assert!(!listed.iter().any(|p| p.starts_with('c')), "{listed:?}");
    }

    /// Folders left unopened at the depth limit are said, so an empty or
    /// short listing is not read as "there is nothing there"; branches with
    /// no match in them are left out.
    #[test]
    fn a_depth_limit_says_what_it_did_not_open_and_a_pattern_prunes_empty_branches() {
        let (scope, root) = fixture("list-depth-stop");
        write(&root, "src/main/java/App.java", "");
        write(&root, "src/test/notes.md", "");
        std::fs::create_dir_all(root.join("src/empty")).unwrap();
        // A folder whose name starts another's path, with no match inside.
        std::fs::create_dir_all(root.join("src/main/jav")).unwrap();
        std::fs::create_dir_all(root.join("src/test/hollow")).unwrap();

        let stopped = |depth: u32, pattern: Option<&str>| {
            let args = ListFilesArgs { depth: Some(depth), pattern: pattern.map(str::to_string), ..ListFilesArgs::default() };
            match list_files(&scope, &args).unwrap() {
                ToolResult::FileList { entries, stopped_at, .. } => (paths(&entries).iter().map(|p| p.to_string()).collect::<Vec<_>>(), stopped_at),
                other => panic!("{other:?}"),
            }
        };
        assert_eq!(stopped(2, Some("*.java")), (vec![], Some(2)), "not found because not looked");
        assert_eq!(stopped(4, Some("*.java")).1, None, "everything was opened");
        assert_eq!(stopped(1, None).1, Some(1));
        assert_eq!(stopped(2, None).1, Some(2), "src/main and src/test have more in them");
        let inside_test = ListFilesArgs { path: Some("src/test".into()), depth: Some(1), ..ListFilesArgs::default() };
        let ToolResult::FileList { stopped_at, .. } = list_files(&scope, &inside_test).unwrap() else { panic!() };
        assert_eq!(stopped_at, None, "an empty folder at the edge hides nothing");

        let (found, _) = run(&scope, &ListFilesArgs { pattern: Some("*.java".into()), ..ListFilesArgs::default() });
        assert_eq!(paths(&found), ["src", "src/main", "src/main/java", "src/main/java/App.java"]);
    }

    #[test]
    fn an_invalid_pattern_says_so() {
        let (scope, _) = fixture("list-badglob");
        let args = ListFilesArgs {
            pattern: Some("[".to_string()),
            ..ListFilesArgs::default()
        };
        assert!(matches!(
            list_files(&scope, &args),
            Err(ToolError::InvalidPattern(_))
        ));
    }

    #[test]
    fn a_path_leaving_the_root_is_refused() {
        let (scope, _) = fixture("list-escape");
        let args = ListFilesArgs {
            path: Some("..".to_string()),
            ..ListFilesArgs::default()
        };
        assert!(matches!(
            list_files(&scope, &args),
            Err(ToolError::PathEscape(_))
        ));
    }

    #[test]
    fn listing_a_file_rather_than_a_directory_is_refused() {
        let (scope, root) = fixture("list-file");
        write(&root, "a.txt", "");
        let args = ListFilesArgs {
            path: Some("a.txt".to_string()),
            ..ListFilesArgs::default()
        };
        assert!(matches!(
            list_files(&scope, &args),
            Err(ToolError::NotFound(_))
        ));
    }

    /// The cap runs after the pattern, so narrowing the request is a way past
    /// it. The other order would make `pattern` a filter over an
    /// already-truncated list, and the model could never reach the rest.
    #[test]
    fn the_cap_applies_after_the_pattern_not_before() {
        let (scope, root) = fixture("list-cap");
        for i in 0..(MAX_ENTRIES + 50) {
            write(&root, &format!("noise/f{i}.txt"), "");
        }
        write(&root, "needle.rs", "");

        let (all, truncated) = run(&scope, &ListFilesArgs::default());
        assert_eq!(all.len(), MAX_ENTRIES);
        assert!(truncated);

        let (filtered, truncated) = run(
            &scope,
            &ListFilesArgs {
                pattern: Some("*.rs".to_string()),
                ..ListFilesArgs::default()
            },
        );
        assert!(
            paths(&filtered).contains(&"needle.rs"),
            "narrowing must reach what the unfiltered cap cut off"
        );
        assert!(!truncated);
    }

    #[test]
    fn the_tree_shows_nesting() {
        let entries = vec![
            ToolFileEntry { path: "src".into(), is_dir: true, size: None },
            ToolFileEntry { path: "src/main.rs".into(), is_dir: false, size: None },
            ToolFileEntry { path: "src/lib.rs".into(), is_dir: false, size: None },
            ToolFileEntry { path: "README.md".into(), is_dir: false, size: None },
        ];

        assert_eq!(
            render_file_tree(&entries, false),
            "./\n\
             ├── README.md\n\
             └── src/\n    \
                 ├── lib.rs\n    \
                 └── main.rs\n"
        );
    }

    /// An empty directory is information — it must not vanish for having no
    /// children to infer it from.
    #[test]
    fn an_empty_directory_still_appears_in_the_tree() {
        let entries = vec![ToolFileEntry { path: "empty".into(), is_dir: true, size: None }];
        assert_eq!(render_file_tree(&entries, false), "./\n└── empty/\n");
    }

    /// A truncated listing must say so in the text the model reads, not only
    /// in a flag it never sees.
    #[test]
    fn a_truncated_tree_says_how_to_narrow_it() {
        let entries = vec![ToolFileEntry { path: "a.txt".into(), is_dir: false, size: None }];
        let rendered = render_file_tree(&entries, true);
        assert!(rendered.contains("the listing is longer"), "{rendered}");
        assert!(rendered.contains("pattern"), "{rendered}");
    }

    #[test]
    fn the_call_wire_shape_is_stable() {
        let json = r#"{"tool": "listFiles", "args": {"path": "src", "depth": "2"}}"#;
        let call: ToolCall = serde_json::from_str(json).expect("parses, quoted depth and all");
        let ToolCall::ListFiles(parsed) = &call else {
            panic!("wrong variant")
        };
        assert_eq!(parsed.depth, Some(2));
        assert!(!call.is_risky(), "listing changes nothing");

        let (scope, root) = fixture("list-dispatch");
        write(&root, "src/a.rs", "");
        assert!(matches!(
            execute_tool(&scope, &call, &mut ReadFiles::default(), &mut Vec::new(), &ToolDeps::default()),
            Ok(ToolResult::FileList { .. })
        ));
    }
}
