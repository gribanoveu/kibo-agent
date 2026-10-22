//! The rule registry for the API-documentation standards checker.
//!
//! Each rule is a `RuleDef` (static metadata, serializable) paired with a
//! `check` function pointer in `RULES`. Adding a rule means adding one
//! `RuleImpl` entry plus its check function; removing one means deleting the
//! entry. `doc_standards::check` iterates `RULES`, skipping rules disabled via
//! `StandardsRuleConfig`.
//!
//! Ported from kibo-mcp-asciidoc-standarts (`rules.rs`), which took it from
//! Alfa Atlas. The message language comes from `MethodFolderCtx::lang`, and
//! К.1.3 asks `MethodFolderCtx::exists` rather than the file system.

use std::path::{Path, PathBuf};

use super::doc_standards_messages as msgs;
use super::doc_standards::{ErrorLanguage, RuleDef, RuleOutcome};

/// One file directly inside a method folder, with its text content already
/// read (empty string for unreadable/binary files).
#[derive(Debug, Clone)]
pub struct FileEntry {
    pub name: String,
    pub path: PathBuf,
    pub content: String,
}

/// Everything a rule check function needs about one `methodName` folder.
pub struct MethodFolderCtx {
    pub method_name: String,
    pub files: Vec<FileEntry>,
    /// Language of every rule's pass/fail message.
    pub lang: ErrorLanguage,
    /// Whether a link's target is on disk (К.1.3).
    pub exists: fn(&Path) -> bool,
}

impl MethodFolderCtx {
    pub fn new(method_name: String, files: Vec<FileEntry>, exists: fn(&Path) -> bool) -> Self {
        Self { method_name, files, lang: ErrorLanguage::default(), exists }
    }

    fn matches_ci(name: &str, candidate: &str) -> bool {
        name.eq_ignore_ascii_case(candidate)
    }

    /// Files matching the `diagram.puml` role's allowed names (К.1.1).
    fn diagram_matches(&self) -> Vec<&FileEntry> {
        let alt1 = format!("{}.puml", self.method_name);
        self.files
            .iter()
            .filter(|f| {
                Self::matches_ci(&f.name, "diagram.puml")
                    || Self::matches_ci(&f.name, "diagrams.puml")
                    || Self::matches_ci(&f.name, &alt1)
            })
            .collect()
    }

    /// Files matching the `methodName.adoc` role (no alternate names).
    fn main_doc_matches(&self) -> Vec<&FileEntry> {
        let expected = format!("{}.adoc", self.method_name);
        self.files
            .iter()
            .filter(|f| Self::matches_ci(&f.name, &expected))
            .collect()
    }

    /// Files matching the `request.adoc` role's allowed names (К.1.1).
    fn request_matches(&self) -> Vec<&FileEntry> {
        let alt1 = format!("request{}.adoc", self.method_name);
        let alt2 = format!("{}Request.adoc", self.method_name);
        self.files
            .iter()
            .filter(|f| {
                Self::matches_ci(&f.name, "request.adoc")
                    || Self::matches_ci(&f.name, &alt1)
                    || Self::matches_ci(&f.name, &alt2)
            })
            .collect()
    }

    /// Files matching the `response.adoc` role's allowed names (К.1.1).
    fn response_matches(&self) -> Vec<&FileEntry> {
        let alt1 = format!("response{}.adoc", self.method_name);
        let alt2 = format!("{}Response.adoc", self.method_name);
        self.files
            .iter()
            .filter(|f| {
                Self::matches_ci(&f.name, "response.adoc")
                    || Self::matches_ci(&f.name, &alt1)
                    || Self::matches_ci(&f.name, &alt2)
            })
            .collect()
    }

    /// The single unambiguous main doc, if exactly one file matches the role.
    fn main_doc(&self) -> Option<&FileEntry> {
        let matches = self.main_doc_matches();
        if matches.len() == 1 { Some(matches[0]) } else { None }
    }

    fn request_doc(&self) -> Option<&FileEntry> {
        let matches = self.request_matches();
        if matches.len() == 1 { Some(matches[0]) } else { None }
    }

    fn response_doc(&self) -> Option<&FileEntry> {
        let matches = self.response_matches();
        if matches.len() == 1 { Some(matches[0]) } else { None }
    }

    fn diagram_doc(&self) -> Option<&FileEntry> {
        let matches = self.diagram_matches();
        if matches.len() == 1 { Some(matches[0]) } else { None }
    }

    /// Main doc + request example — where К.4.x expects input-parameter tables.
    fn input_content(&self) -> String {
        let mut out = String::new();
        if let Some(f) = self.main_doc() {
            out.push_str(&f.content);
            out.push('\n');
        }
        if let Some(f) = self.request_doc() {
            out.push_str(&f.content);
        }
        out
    }

    /// Main doc + response example — where К.5.x expects output-parameter tables.
    fn output_content(&self) -> String {
        let mut out = String::new();
        if let Some(f) = self.main_doc() {
            out.push_str(&f.content);
            out.push('\n');
        }
        if let Some(f) = self.response_doc() {
            out.push_str(&f.content);
        }
        out
    }
}

fn contains_ci(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

/// Split `content` into AsciiDoc sections keyed by header title
/// (`= Title`, `== Title`, ...). Returns `(title, body)` pairs where `body`
/// is every line up to (but excluding) the next header of any level.
fn sections(content: &str) -> Vec<(String, String)> {
    let lines: Vec<&str> = content.lines().collect();
    let mut headers: Vec<(usize, String)> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        let eq_count = trimmed.chars().take_while(|c| *c == '=').count();
        if eq_count == 0 {
            continue;
        }
        let rest = &trimmed[eq_count..];
        if !rest.starts_with(' ') {
            continue;
        }
        let title = rest.trim();
        if !title.is_empty() {
            headers.push((i, title.to_string()));
        }
    }
    let mut out = Vec::new();
    for (idx, (line_no, title)) in headers.iter().enumerate() {
        let start = line_no + 1;
        let end = headers
            .get(idx + 1)
            .map(|(next_line, _)| *next_line)
            .unwrap_or(lines.len());
        let body = if start < end {
            lines[start..end].join("\n")
        } else {
            String::new()
        };
        out.push((title.clone(), body));
    }
    out
}

fn section_body_by_keywords(content: &str, keywords: &[&str]) -> Option<String> {
    let secs = sections(content);
    secs.into_iter()
        .find(|(title, _)| {
            let lower = title.to_lowercase();
            keywords.iter().any(|k| lower.contains(k))
        })
        .map(|(_, body)| body)
}

fn non_blank_len(text: &str) -> usize {
    text.chars().filter(|c| !c.is_whitespace()).count()
}

/// True when a parsed table cell counts as intentionally filled. A lone `-`
/// (or em dash) is the house placeholder for «nothing to put here».
fn table_cell_is_filled(cell: &str) -> bool {
    let trimmed = cell.trim();
    !trimmed.is_empty()
}

/// Extract cell text from the body of one `|===` block. Handles the two layouts
/// seen in real postanovki:
/// - compact rows: `| a | b | c | d`
/// - multi-line cells: a lone `|` line, then continuation lines without `|`
///   until the next row (common in long output-parameter tables).
fn extract_table_cells(body_lines: &[&str]) -> Vec<String> {
    let mut cells = Vec::new();
    let mut pending_multiline: Option<String> = None;

    for line in body_lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if let Some(pending) = pending_multiline.as_mut() {
                pending.push('\n');
            }
            continue;
        }

        if trimmed == "|" {
            if let Some(prev) = pending_multiline.take() {
                cells.push(prev);
            }
            pending_multiline = Some(String::new());
            continue;
        }

        if trimmed.starts_with('|') {
            if let Some(prev) = pending_multiline.take() {
                cells.push(prev);
            }
            for cell in trimmed.split('|').skip(1) {
                cells.push(cell.trim().to_string());
            }
        } else if let Some(pending) = pending_multiline.as_mut() {
            if !pending.is_empty() {
                pending.push('\n');
            }
            pending.push_str(trimmed);
        }
    }

    if let Some(prev) = pending_multiline.take() {
        cells.push(prev);
    }
    cells
}

/// Find AsciiDoc table blocks (`|===` ... `|===`) in `content`, returning
/// `(column_count, cells)` for each. Column count is the pipe count on the
/// first non-empty row inside the block. This is a pragmatic heuristic, not
/// a full AsciiDoc table parser — it covers the simple `|===` tables used
/// throughout the standard's own examples, but won't understand `cols`
/// attribute shorthand like `4*`.
fn find_tables(content: &str) -> Vec<(usize, Vec<String>)> {
    let lines: Vec<&str> = content.lines().collect();
    let mut tables = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim() == "|===" {
            let mut j = i + 1;
            let mut body_lines = Vec::new();
            while j < lines.len() && lines[j].trim() != "|===" {
                body_lines.push(lines[j]);
                j += 1;
            }

            let cells = extract_table_cells(&body_lines);

            let column_count = body_lines
                .iter()
                .map(|l| l.matches('|').count())
                .find(|c| *c > 0)
                .unwrap_or(0);

            if !body_lines.is_empty() {
                tables.push((column_count, cells));
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }
    tables
}

fn wide_tables_filled(content: &str) -> bool {
    let tables = find_tables(content);
    let wide_tables: Vec<_> = tables.iter().filter(|(cols, _)| *cols >= 4).collect();
    !wide_tables.is_empty()
        && wide_tables
            .iter()
            .all(|(_, cells)| cells.iter().all(|c| table_cell_is_filled(c)))
}

// --- К.1.1 / К.1.2: required files present, unambiguously ---

fn check_k1_1(ctx: &MethodFolderCtx) -> RuleOutcome {
    let lang = ctx.lang;
    let mut missing = Vec::new();
    if ctx.diagram_matches().is_empty() {
        missing.push("diagram.puml");
    }
    if ctx.main_doc_matches().is_empty() {
        missing.push("methodName.adoc");
    }
    if ctx.request_matches().is_empty() {
        missing.push("request.adoc");
    }
    if ctx.response_matches().is_empty() {
        missing.push("response.adoc");
    }
    if missing.is_empty() {
        RuleOutcome { passed: true, message: msgs::passed(lang) }
    } else {
        RuleOutcome { passed: false, message: msgs::k1_1(lang, &missing) }
    }
}

fn check_k1_2(ctx: &MethodFolderCtx) -> RuleOutcome {
    let lang = ctx.lang;
    let mut ambiguous = Vec::new();
    if ctx.diagram_matches().len() > 1 {
        ambiguous.push("diagram.puml");
    }
    if ctx.main_doc_matches().len() > 1 {
        ambiguous.push("methodName.adoc");
    }
    if ctx.request_matches().len() > 1 {
        ambiguous.push("request.adoc");
    }
    if ctx.response_matches().len() > 1 {
        ambiguous.push("response.adoc");
    }
    if ambiguous.is_empty() {
        RuleOutcome { passed: true, message: msgs::passed(lang) }
    } else {
        RuleOutcome { passed: false, message: msgs::k1_2(lang, &ambiguous) }
    }
}

// --- К.1.3: links ---

/// Reference macros whose target is a file. `image:` also covers the block
/// form `image::`.
const LINK_MACROS: &[&str] = &["include::", "image:", "xref:", "link:"];

/// Local file targets referenced from `content`, as `(macro, path)` pairs.
/// External URLs, attribute references (`{...}`), bare anchors and
/// commented-out lines are skipped: none of them is a file to look for.
fn local_link_targets(content: &str) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    for line in content.lines() {
        if line.trim_start().starts_with("//") {
            continue;
        }
        for &mac in LINK_MACROS {
            let mut rest = line;
            while let Some(pos) = rest.find(mac) {
                let preceded_by_word = rest[..pos].chars().next_back().is_some_and(char::is_alphanumeric);
                let after = &rest[pos + mac.len()..];
                rest = after;
                if preceded_by_word {
                    continue;
                }
                let after = if mac == "image:" { after.strip_prefix(':').unwrap_or(after) } else { after };
                let Some(end) = after.find('[') else {
                    continue;
                };
                let target = &after[..end];
                let external = target.contains("://") || target.starts_with("mailto:");
                if target.is_empty()
                    || external
                    || target.starts_with('#')
                    || target.contains('{')
                    || target.contains(char::is_whitespace)
                {
                    continue;
                }
                let path = target.split('#').next().unwrap_or(target);
                // `xref:someId[]` names an anchor, not a file.
                if mac == "xref:" && !path.contains('.') {
                    continue;
                }
                out.push((mac, path.to_string()));
            }
        }
    }
    out
}

/// Value of a document attribute entry such as `:imagesdir: images`.
fn attribute_value<'a>(content: &'a str, name: &str) -> Option<&'a str> {
    let entry = format!(":{name}:");
    content
        .lines()
        .find_map(|l| l.trim().strip_prefix(entry.as_str()))
        .map(str::trim)
        .filter(|v| !v.is_empty())
}

/// К.1.3 as far as local files go: every include/image/xref/link target in
/// the folder's AsciiDoc files must exist. External links are not fetched —
/// the checker does no network access (the standard also counts 404s on
/// http links, which would need live requests, some only over VPN).
fn check_k1_3(ctx: &MethodFolderCtx) -> RuleOutcome {
    let lang = ctx.lang;
    let mut broken = Vec::new();
    for file in &ctx.files {
        let lower = file.name.to_lowercase();
        if !(lower.ends_with(".adoc") || lower.ends_with(".asciidoc")) {
            continue;
        }
        let dir = file.path.parent().unwrap_or(Path::new(""));
        let imagesdir = attribute_value(&file.content, "imagesdir").filter(|d| !d.contains('{') && !d.contains("://"));
        for (mac, target) in local_link_targets(&file.content) {
            let found = (ctx.exists)(&dir.join(&target))
                || (mac == "image:" && imagesdir.is_some_and(|d| (ctx.exists)(&dir.join(d).join(&target))));
            if !found {
                broken.push(format!("{}: {mac}{target}", file.name));
            }
        }
    }
    if broken.is_empty() {
        RuleOutcome { passed: true, message: msgs::passed(lang) }
    } else {
        RuleOutcome { passed: false, message: msgs::k1_3(lang, &broken) }
    }
}

// --- К.2.1: TOC macro ---

fn check_k2_1(ctx: &MethodFolderCtx) -> RuleOutcome {
    let lang = ctx.lang;
    let Some(main) = ctx.main_doc() else {
        return RuleOutcome { passed: false, message: msgs::main_doc_missing(lang) };
    };
    let passed = contains_ci(&main.content, "toc::[]") || contains_ci(&main.content, ":toc:");
    RuleOutcome {
        passed,
        message: if passed { msgs::passed(lang) } else { msgs::k2_1(lang) },
    }
}

// --- К.2.2: Назначение/Описание >= 50 non-blank chars ---

fn check_k2_2(ctx: &MethodFolderCtx) -> RuleOutcome {
    let lang = ctx.lang;
    let Some(main) = ctx.main_doc() else {
        return RuleOutcome { passed: false, message: msgs::main_doc_missing(lang) };
    };
    let passed = section_body_by_keywords(&main.content, &["назначение", "описание"])
        .map(|body| non_blank_len(&body) >= 50)
        .unwrap_or(false);
    RuleOutcome {
        passed,
        message: if passed { msgs::passed(lang) } else { msgs::k2_2(lang) },
    }
}

// --- К.3.1: diagram referenced from the main doc and non-empty ---

fn check_k3_1(ctx: &MethodFolderCtx) -> RuleOutcome {
    let lang = ctx.lang;
    let Some(main) = ctx.main_doc() else {
        return RuleOutcome { passed: false, message: msgs::main_doc_missing(lang) };
    };
    let passed = ctx
        .diagram_doc()
        .map(|diagram| {
            contains_ci(&main.content, &diagram.name) && !diagram.content.trim().is_empty()
        })
        .unwrap_or(false);
    RuleOutcome {
        passed,
        message: if passed { msgs::passed(lang) } else { msgs::k3_1(lang) },
    }
}

// --- К.4.1 / К.4.2: input parameters table ---

fn check_k4_1(ctx: &MethodFolderCtx) -> RuleOutcome {
    let lang = ctx.lang;
    let passed = find_tables(&ctx.input_content())
        .iter()
        .any(|(cols, _)| *cols >= 4);
    RuleOutcome {
        passed,
        message: if passed { msgs::passed(lang) } else { msgs::k4_1(lang) },
    }
}

fn check_k4_2(ctx: &MethodFolderCtx) -> RuleOutcome {
    let lang = ctx.lang;
    let passed = wide_tables_filled(&ctx.input_content());
    RuleOutcome {
        passed,
        message: if passed { msgs::passed(lang) } else { msgs::k4_2(lang) },
    }
}

// --- К.4.3: link to request.adoc ---

fn check_k4_3(ctx: &MethodFolderCtx) -> RuleOutcome {
    let lang = ctx.lang;
    let Some(main) = ctx.main_doc() else {
        return RuleOutcome { passed: false, message: msgs::main_doc_missing(lang) };
    };
    let passed = ctx
        .request_doc()
        .map(|request| contains_ci(&main.content, &request.name))
        .unwrap_or(false);
    RuleOutcome {
        passed,
        message: if passed { msgs::passed(lang) } else { msgs::k4_3(lang) },
    }
}

// --- К.4.4: request.adoc non-empty, has endpoint + example ---

/// True if `content` has a `/path` or `${HOST}/path` token anywhere, not just
/// as a whole line — real docs commonly write it inline (`` Endpoint:
/// `POST /api/foo` ``, `` `${HOST}/api/foo` ``), wrapped in backticks or
/// prefixed with an HTTP method. Tokens are split on whitespace and trimmed
/// of common markup punctuation before the `/`-prefix check.
fn looks_like_endpoint(content: &str) -> bool {
    if contains_ci(content, "${host}/") {
        return true;
    }
    content.split_whitespace().any(|raw| {
        let t = raw.trim_matches(|c: char| {
            matches!(c, '`' | '"' | '\'' | '(' | ')' | ',' | '.' | ';' | ':' | '[' | ']')
        });
        t.starts_with('/') && t.len() > 1 && !t.starts_with("//")
    })
}

fn check_k4_4(ctx: &MethodFolderCtx) -> RuleOutcome {
    let lang = ctx.lang;
    let content = ctx.request_doc().map(|f| f.content.trim()).unwrap_or_default();
    if content.is_empty() {
        return RuleOutcome { passed: false, message: msgs::k4_4(lang) };
    }
    if !looks_like_endpoint(content) {
        return RuleOutcome { passed: false, message: msgs::k4_4_no_endpoint(lang) };
    }
    RuleOutcome { passed: true, message: msgs::passed(lang) }
}

// --- К.5.1 / К.5.2: output parameters table ---
//
// §7.5 of the standard opens with «Если таблица присутствует», which reads as
// if the output table were optional, yet its weight table scores К.5.1 as
// «Таблица с выходными параметрами отсутствует». Decided: the table is
// required — a method without output parameters uses the К.5.* Exceptions
// marker instead.

fn check_k5_1(ctx: &MethodFolderCtx) -> RuleOutcome {
    let lang = ctx.lang;
    let passed = find_tables(&ctx.output_content())
        .iter()
        .any(|(cols, _)| *cols >= 4);
    RuleOutcome {
        passed,
        message: if passed { msgs::passed(lang) } else { msgs::k5_1(lang) },
    }
}

fn check_k5_2(ctx: &MethodFolderCtx) -> RuleOutcome {
    let lang = ctx.lang;
    let passed = wide_tables_filled(&ctx.output_content());
    RuleOutcome {
        passed,
        message: if passed { msgs::passed(lang) } else { msgs::k5_2(lang) },
    }
}

// --- К.5.3: link to response.adoc ---

fn check_k5_3(ctx: &MethodFolderCtx) -> RuleOutcome {
    let lang = ctx.lang;
    let Some(main) = ctx.main_doc() else {
        return RuleOutcome { passed: false, message: msgs::main_doc_missing(lang) };
    };
    let passed = ctx
        .response_doc()
        .map(|response| contains_ci(&main.content, &response.name))
        .unwrap_or(false);
    RuleOutcome {
        passed,
        message: if passed { msgs::passed(lang) } else { msgs::k5_3(lang) },
    }
}

// --- К.5.4: response.adoc non-empty ---

fn check_k5_4(ctx: &MethodFolderCtx) -> RuleOutcome {
    let lang = ctx.lang;
    let passed = ctx
        .response_doc()
        .map(|f| !f.content.trim().is_empty())
        .unwrap_or(false);
    RuleOutcome {
        passed,
        message: if passed { msgs::passed(lang) } else { msgs::k5_4(lang) },
    }
}

// --- К.6.1: "Алгоритм работы" section ---

fn check_k6_1(ctx: &MethodFolderCtx) -> RuleOutcome {
    let lang = ctx.lang;
    let Some(main) = ctx.main_doc() else {
        return RuleOutcome { passed: false, message: msgs::main_doc_missing(lang) };
    };
    let passed = section_body_by_keywords(&main.content, &["алгоритм"])
        .map(|body| non_blank_len(&body) > 0)
        .unwrap_or(false);
    RuleOutcome {
        passed,
        message: if passed { msgs::passed(lang) } else { msgs::k6_1(lang) },
    }
}

// --- К.7.1: "Обработка ошибок" section ---

fn check_k7_1(ctx: &MethodFolderCtx) -> RuleOutcome {
    let lang = ctx.lang;
    let Some(main) = ctx.main_doc() else {
        return RuleOutcome { passed: false, message: msgs::main_doc_missing(lang) };
    };
    let passed = section_body_by_keywords(&main.content, &["обработка ошибок", "ошибки"])
        .map(|body| non_blank_len(&body) > 0)
        .unwrap_or(false);
    RuleOutcome {
        passed,
        message: if passed { msgs::passed(lang) } else { msgs::k7_1(lang) },
    }
}

/// Pairs a rule's static metadata with its check function.
pub struct RuleImpl {
    pub def: RuleDef,
    pub check: fn(&MethodFolderCtx) -> RuleOutcome,
}

/// The full rule registry, in the same order and with the same weights as
/// the standard's "Вес критериев" table (sums to 100 for the default-enabled
/// set). К.1.3 ("Корректность ссылок") covers local link targets only: 404s
/// on http links would need live requests (internal Confluence links only
/// over VPN), and this checker performs no network access at all.
pub const RULES: &[RuleImpl] = &[
    RuleImpl {
        def: RuleDef {
            id: "K.1.1",
            title: "Папка с документацией присутствует",
            weight: 20,
            default_enabled: true,
        },
        check: check_k1_1,
    },
    RuleImpl {
        def: RuleDef {
            id: "K.1.2",
            title: "Папка документации соответствует структуре",
            weight: 20,
            default_enabled: true,
        },
        check: check_k1_2,
    },
    RuleImpl {
        def: RuleDef {
            id: "K.1.3",
            title: "Корректность ссылок",
            weight: 3,
            default_enabled: true,
        },
        check: check_k1_3,
    },
    RuleImpl {
        def: RuleDef {
            id: "K.2.1",
            title: "Файл метода содержит Оглавление",
            weight: 3,
            default_enabled: true,
        },
        check: check_k2_1,
    },
    RuleImpl {
        def: RuleDef {
            id: "K.2.2",
            title: "Файл метода содержит поле Назначение",
            weight: 3,
            default_enabled: true,
        },
        check: check_k2_2,
    },
    RuleImpl {
        def: RuleDef {
            id: "K.3.1",
            title: "Диаграмма корректна",
            weight: 3,
            default_enabled: true,
        },
        check: check_k3_1,
    },
    RuleImpl {
        def: RuleDef {
            id: "K.4.1",
            title: "Таблица с входными данными присутствует",
            weight: 10,
            default_enabled: true,
        },
        check: check_k4_1,
    },
    RuleImpl {
        def: RuleDef {
            id: "K.4.2",
            title: "Таблица с входными данными корректна",
            weight: 3,
            default_enabled: true,
        },
        check: check_k4_2,
    },
    RuleImpl {
        def: RuleDef {
            id: "K.4.3",
            title: "Пример входных данных присутствует",
            weight: 10,
            default_enabled: true,
        },
        check: check_k4_3,
    },
    RuleImpl {
        def: RuleDef {
            id: "K.4.4",
            title: "Пример входных данных корректен",
            weight: 3,
            default_enabled: true,
        },
        check: check_k4_4,
    },
    RuleImpl {
        def: RuleDef {
            id: "K.5.1",
            title: "Таблица с выходными данными присутствует",
            weight: 5,
            default_enabled: true,
        },
        check: check_k5_1,
    },
    RuleImpl {
        def: RuleDef {
            id: "K.5.2",
            title: "Таблица с выходными данными корректна",
            weight: 3,
            default_enabled: true,
        },
        check: check_k5_2,
    },
    RuleImpl {
        def: RuleDef {
            id: "K.5.3",
            title: "Пример выходных данных присутствует",
            weight: 5,
            default_enabled: true,
        },
        check: check_k5_3,
    },
    RuleImpl {
        def: RuleDef {
            id: "K.5.4",
            title: "Пример выходных данных корректен",
            weight: 3,
            default_enabled: true,
        },
        check: check_k5_4,
    },
    RuleImpl {
        def: RuleDef {
            id: "K.6.1",
            title: "Раздел с алгоритмами корректен",
            weight: 3,
            default_enabled: true,
        },
        check: check_k6_1,
    },
    RuleImpl {
        def: RuleDef {
            id: "K.7.1",
            title: "Обработка ошибок",
            weight: 3,
            default_enabled: true,
        },
        check: check_k7_1,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(method_name: &str, files: &[(&str, &str)]) -> MethodFolderCtx {
        MethodFolderCtx::new(
            method_name.to_string(),
            files
                .iter()
                .map(|(name, content)| FileEntry {
                    name: name.to_string(),
                    path: PathBuf::from(name),
                    content: content.to_string(),
                })
                .collect(),
            |_| true,
        )
    }

    fn full_main_doc(method: &str) -> String {
        format!(
            "= {method}\n:toc:\n\n== Назначение\nЭто описание метода, которое должно быть длиннее пятидесяти символов для прохождения проверки.\n\n== Схема работы\nimage::{method}.puml[]\n\n== Описание входных параметров\n[cols=\"4\"]\n|===\n| Имя | Тип | Обязательный | Описание\n| id | string | да | идентификатор\n|===\n\ninclude::./request.adoc[]\n\n== Описание выходных параметров\n[cols=\"4\"]\n|===\n| Имя | Тип | Обязательный | Описание\n| id | string | да | идентификатор\n|===\n\ninclude::./response.adoc[]\n\n== Алгоритм работы\nШаг 1. Сделать что-то.\n\n== Обработка ошибок\n404 - не найдено.\n"
        )
    }

    #[test]
    fn k1_1_passes_with_all_roles_present() {
        let c = ctx(
            "getUser",
            &[
                ("getUser.puml", "@startuml\n@enduml"),
                ("getUser.adoc", "= getUser"),
                ("request.adoc", "content"),
                ("response.adoc", "content"),
            ],
        );
        assert!(check_k1_1(&c).passed);
    }

    #[test]
    fn k1_1_fails_when_request_missing() {
        let c = ctx(
            "getUser",
            &[
                ("getUser.puml", "@startuml\n@enduml"),
                ("getUser.adoc", "= getUser"),
                ("response.adoc", "content"),
            ],
        );
        assert!(!check_k1_1(&c).passed);
    }

    #[test]
    fn main_doc_dependent_rules_point_back_to_k1_1_when_filename_mismatches_folder() {
        // Folder is "sendToKalugaJob" but the actual file is "sendToKaluga.adoc"
        // (missing the "Job" suffix) — a real-world naming drift. The file does
        // contain a valid :toc: line, but since it isn't recognized as the main
        // doc (per К.1.1, no alternate names allowed), every rule that reads the
        // main doc should explain *that*, not restate its own unrelated symptom.
        let c = ctx(
            "sendToKalugaJob",
            &[
                ("sendToKaluga.adoc", ":toc: left\n\n== Назначение\nЭто описание длиннее пятидесяти символов для проверки текста.\n\n== Алгоритм работы\nШаг 1.\n\n== Обработка ошибок\n404.\n"),
                ("sendToKaluga.puml", "@startuml\n@enduml"),
            ],
        );
        assert!(ctx_main_doc_is_none(&c));
        for outcome in [
            check_k2_1(&c),
            check_k2_2(&c),
            check_k3_1(&c),
            check_k4_3(&c),
            check_k5_3(&c),
            check_k6_1(&c),
            check_k7_1(&c),
        ] {
            assert!(!outcome.passed);
            assert!(
                outcome.message.contains("К.1.1") || outcome.message.contains("K.1.1"),
                "expected message to point back to K.1.1, got: {}",
                outcome.message
            );
        }
    }

    fn ctx_main_doc_is_none(ctx: &MethodFolderCtx) -> bool {
        ctx.main_doc().is_none()
    }

    #[test]
    fn k1_2_fails_on_ambiguous_request() {
        let c = ctx(
            "getUser",
            &[
                ("getUser.puml", "@startuml\n@enduml"),
                ("getUser.adoc", "= getUser"),
                ("request.adoc", "content"),
                ("getUserRequest.adoc", "content"),
                ("response.adoc", "content"),
            ],
        );
        assert!(check_k1_1(&c).passed);
        assert!(!check_k1_2(&c).passed);
    }

    #[test]
    fn full_folder_passes_content_rules() {
        let c = ctx(
            "getUser",
            &[
                ("getUser.puml", "@startuml\n@enduml"),
                ("getUser.adoc", &full_main_doc("getUser")),
                ("request.adoc", "${HOST}/api/getUser\ncurl example"),
                ("response.adoc", "{ \"id\": 1 }"),
            ],
        );
        assert!(check_k2_1(&c).passed, "toc");
        assert!(check_k2_2(&c).passed, "purpose");
        assert!(check_k3_1(&c).passed, "diagram");
        assert!(check_k4_1(&c).passed, "input table");
        assert!(check_k4_2(&c).passed, "input table filled");
        assert!(check_k4_3(&c).passed, "input link");
        assert!(check_k4_4(&c).passed, "request example");
        assert!(check_k5_1(&c).passed, "output table");
        assert!(check_k5_2(&c).passed, "output table filled");
        assert!(check_k5_3(&c).passed, "output link");
        assert!(check_k5_4(&c).passed, "response example");
        assert!(check_k6_1(&c).passed, "algorithm");
        assert!(check_k7_1(&c).passed, "error handling");
    }

    #[test]
    fn k2_2_fails_on_short_purpose() {
        let c = ctx("m", &[("m.adoc", "= m\n== Назначение\nкоротко\n")]);
        assert!(!check_k2_2(&c).passed);
    }

    #[test]
    fn k4_2_fails_on_empty_cell() {
        let content = "== Описание входных параметров\n[cols=\"4\"]\n|===\n| Имя | Тип | Обязательный | Описание\n| id | | да | идентификатор\n|===\n";
        let c = ctx("m", &[("m.adoc", content)]);
        assert!(check_k4_1(&c).passed);
        assert!(!check_k4_2(&c).passed);
    }

    #[test]
    fn k4_2_ignores_unfilled_output_tables_in_response_adoc() {
        let c = ctx(
            "m",
            &[
                (
                    "m.adoc",
                    "== Описание входных параметров\ninclude::request.adoc[]\n== Описание выходных параметров\ninclude::response.adoc[]\n",
                ),
                (
                    "request.adoc",
                    "|===\n| Имя | Тип | Обязательный | Описание\n| id | string | да | идентификатор\n|===\n",
                ),
                (
                    "response.adoc",
                    "|===\n| Имя | Тип | Обязательный | Описание\n| id | | да | идентификатор\n|===\n",
                ),
            ],
        );
        assert!(check_k4_2(&c).passed, "{:?}", check_k4_2(&c));
        assert!(!check_k5_2(&c).passed);
    }

    #[test]
    fn k5_2_passes_multiline_output_table_cells() {
        let response_table = r#"[cols="4"]
|===
|*Параметр* |*Тип данных* |*Обязательность* |*Описание*

|
1: transaction[].id +
2: transaction[].organizationId
|
1:  string  +
2:  string
|
1: required +
2: required
|
1: Ключ полупроводки. +
2: `CUS` Клиента
|==="#;
        let c = ctx("fetchAusnTransactions", &[("response.adoc", response_table)]);
        assert!(check_k5_1(&c).passed);
        assert!(check_k5_2(&c).passed, "{:?}", check_k5_2(&c));
    }

    #[test]
    fn k5_2_passes_rowspan_tables() {
        let response_table = r#"|===
3+| *Поле* | *Тип* | *Обязательность* |*Описание*
.15+|specifics .5+|single[] | OperationTaxbaseCode | String | required | Разметка 1
|OperationCategory |string |optional|Разметка 2
|===

[cols="5"]
|===
2+|*Поле* |*Тип данных*|*Обязательность* |*Описание*
.4+|page|number|i32|required |Номер страницы
|size|i32|required|Кол-во элементов на странице
|==="#;
        let c = ctx("fetchAusnTransactions", &[("response.adoc", response_table)]);
        assert!(check_k5_2(&c).passed, "{:?}", check_k5_2(&c));
    }

    #[test]
    fn k4_2_and_k5_2_pass_realistic_thrift_method_tables() {
        const REQUEST: &str = r#"[cols="4"]
|===
|*Параметр* |*Тип данных* |*Обязательность* |*Описание*
|userData.id
|string
|required
|Мнемоника пользователя
|transactionFilter.organizationId
|string
|required
|Идентификатор организации
|="#;

        const RESPONSE: &str = r#"[cols="4"]
|===
|*Параметр* |*Тип данных* |*Обязательность* |*Описание*
|lastRequestDateTime|UNIX datetime|required|Дата и время последнего запроса
|===

[cols="4"]
|===
|*Параметр* |*Тип данных* |*Обязательность* |*Описание*

|
1: transaction[].id +
2: transaction[].organizationId
|
1: string +
2: string
|
1: required +
2: required
|
1: Ключ полупроводки. +
2: CUS Клиента
|===

|===
3+| *Поле* | *Тип* | *Обязательность* |*Описание*
.15+|specifics .5+|single[] | OperationTaxbaseCode | String | required | Разметка 1
|="#;

        let c = ctx(
            "fetchAusnTransactions",
            &[
                (
                    "fetchAusnTransactions.adoc",
                    "== Описание входных параметров\ninclude::request.adoc[]\n== Описание выходных параметров\ninclude::response.adoc[]\n",
                ),
                ("request.adoc", REQUEST),
                ("response.adoc", RESPONSE),
            ],
        );
        assert!(check_k4_2(&c).passed, "{:?}", check_k4_2(&c));
        assert!(check_k5_2(&c).passed, "{:?}", check_k5_2(&c));
    }

    #[test]
    fn k4_4_recognizes_inline_endpoint_with_method_and_backticks() {
        let content = "Endpoint: `POST /api/patent-notifications`\n\nПример запроса:\n{\"fnsId\": \"7701\"}\n";
        let c = ctx("m", &[("request.adoc", content)]);
        assert!(check_k4_4(&c).passed, "{:?}", check_k4_4(&c));
    }

    #[test]
    fn k4_4_recognizes_backtick_wrapped_host_endpoint() {
        let content = "`${HOST}/corp-users-api/tapi/getUser`\n";
        let c = ctx("m", &[("request.adoc", content)]);
        assert!(check_k4_4(&c).passed);
    }

    /// A real folder on disk: К.1.3 resolves link targets against the file
    /// system, so an in-memory `ctx` cannot exercise it.
    fn k1_3_folder(files: &[(&str, &str)]) -> (PathBuf, MethodFolderCtx) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "k1_3-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(dir.join("img")).unwrap();
        let entries = files
            .iter()
            .map(|(name, content)| {
                let path = dir.join(name);
                std::fs::write(&path, content).unwrap();
                FileEntry { name: name.to_string(), path, content: content.to_string() }
            })
            .collect();
        (dir.clone(), MethodFolderCtx::new("m".to_string(), entries, |p| p.exists()))
    }

    #[test]
    fn k1_3_passes_when_local_targets_exist_and_skips_what_is_not_a_file() {
        let main = "= m\n:imagesdir: img\ninclude::./request.adoc[]\nimage::m.puml[]\n\
            xref:request.adoc#top[запрос]\nxref:someAnchor[якорь]\n\
            link:https://example.com/missing[внешняя]\ninclude::{partials}/x.adoc[]\n\
            // include::commented-out.adoc[]\n";
        let (dir, c) = k1_3_folder(&[("m.adoc", main), ("request.adoc", "POST /x"), ("m.puml", "@startuml")]);
        let outcome = check_k1_3(&c);
        std::fs::remove_dir_all(&dir).ok();
        assert!(outcome.passed, "{}", outcome.message);
    }

    #[test]
    fn k1_3_names_each_missing_target() {
        let main = "include::../CompositeException.adoc[]\nimage::diagram.png[]\nsee xref:other.adoc[]\n";
        let (dir, c) = k1_3_folder(&[("m.adoc", main)]);
        let outcome = check_k1_3(&c);
        std::fs::remove_dir_all(&dir).ok();
        assert!(!outcome.passed);
        for missing in ["include::../CompositeException.adoc", "image:diagram.png", "xref:other.adoc"] {
            assert!(outcome.message.contains(missing), "{missing} not in {}", outcome.message);
        }
    }

    #[test]
    fn k4_4_fails_without_any_path_token() {
        let content = "Просто текстовое описание без явного пути.\n";
        let c = ctx("m", &[("request.adoc", content)]);
        assert!(!check_k4_4(&c).passed);
    }

    /// A filled-in example whose endpoint the rule cannot read must say so,
    /// not claim the file is empty — that sends the author looking for a
    /// problem that is not there.
    #[test]
    fn k4_4_names_an_unrecognized_endpoint_rather_than_an_empty_file() {
        let lang = ErrorLanguage::default();
        for content in [
            "Endpoint: POST https://demo.example.ru/files\n",
            "Endpoint: {host}/corp-api/process-data\n",
        ] {
            let c = ctx("m", &[("request.adoc", content)]);
            let outcome = check_k4_4(&c);
            assert!(!outcome.passed, "{content}");
            assert_eq!(outcome.message, msgs::k4_4_no_endpoint(lang), "{content}");
        }
        let empty = ctx("m", &[("request.adoc", "  \n")]);
        assert_eq!(check_k4_4(&empty).message, msgs::k4_4(lang));
    }
}
