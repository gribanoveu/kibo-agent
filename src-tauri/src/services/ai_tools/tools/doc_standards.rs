//! `docStandards` — the documentation standard's check of API-method folders
//! (`services::doc_standards`), and the guide to fixing what it finds. Built
//! in from kibo-mcp-asciidoc-standarts, whose four tools are two `op`s here:
//! a check validates its path itself, and the rules' list is the check's own
//! output.
//!
//! Reads only, so it goes in every mode without a card; the feature
//! (`Feature::DocStandards`) keeps it out of sight until the user turns it on.

use std::fmt::Write as _;

use crate::domain::doc_standards::{ErrorLanguage, FolderReport, Skipped, StandardsRuleConfig};
use crate::domain::doc_standards_rules::RULES;
use crate::domain::llm::LlmToolDefinition;
use crate::domain::tools::{DocStandardsArgs, DocStandardsOp, ToolError, ToolResult, ToolScope};
use crate::services::doc_standards::{self, Checked};

use super::super::resolve::resolve_existing;

/// The guide, by topic: how to fix each rule, then the writing guide for a
/// method document and a sample that passes.
const GUIDE: &[(&str, &str)] = &[
    ("rules", include_str!("../../../../doc-standards/rules.md")),
    ("structure", include_str!("../../../../doc-standards/structure.md")),
    ("errors", include_str!("../../../../doc-standards/errors.md")),
    ("glossary", include_str!("../../../../doc-standards/glossary.md")),
    ("template", include_str!("../../../../doc-standards/template.adoc")),
];

/// Method folders shown in one answer, failing first, so a large docs tree
/// does not flood the context and still shows what needs fixing.
const MAX_FOLDERS: usize = 100;

pub fn doc_standards(scope: &ToolScope, args: &DocStandardsArgs) -> Result<ToolResult, ToolError> {
    match args.op {
        DocStandardsOp::Guide => guide(args.topic.as_deref()),
        DocStandardsOp::Check => check(scope, args),
    }
}

fn invalid(reason: String) -> ToolError {
    ToolError::InvalidArguments { tool: "docStandards".into(), reason }
}

fn guide(topic: Option<&str>) -> Result<ToolResult, ToolError> {
    let topics: Vec<&str> = GUIDE.iter().map(|(name, _)| *name).collect();
    let topic = topic.ok_or_else(|| invalid(format!("op «guide» needs `topic`: one of {}", topics.join(", "))))?;
    match GUIDE.iter().find(|(name, _)| *name == topic) {
        Some((name, text)) => Ok(ToolResult::DocStandards { text: text.to_string(), summary: format!("guide: {name}") }),
        None => Err(invalid(format!("there is no topic «{topic}» — there are: {}", topics.join(", ")))),
    }
}

fn check(scope: &ToolScope, args: &DocStandardsArgs) -> Result<ToolResult, ToolError> {
    let path = args.path.as_deref().filter(|p| !p.trim().is_empty()).ok_or_else(|| invalid("op «check» needs `path`".into()))?;
    let rules = args.rules.clone().unwrap_or_default();
    let unknown: Vec<&str> = rules.keys().map(String::as_str).filter(|id| !RULES.iter().any(|r| r.def.id == *id)).collect();
    if !unknown.is_empty() {
        let known: Vec<&str> = RULES.iter().map(|r| r.def.id).collect();
        return Err(invalid(format!("unknown rule id(s): {}; known: {}", unknown.join(", "), known.join(", "))));
    }
    let resolved = resolve_existing(scope, path)?;
    // The whole open folder is a repository, not documentation: its walk
    // would read every folder in it. The model names where the docs are.
    if resolved == scope.root() {
        return Err(invalid(
            "`path` is the open folder itself — name the documentation folder or a method folder in it \
             (find it with listFiles or grep), not the root"
                .into(),
        ));
    }
    let lang = args.language.unwrap_or_default();
    let config = StandardsRuleConfig { rules: rules.into_iter().collect() };
    let checked = doc_standards::check(scope.root(), &resolved, &config, lang).map_err(|why| {
        let head = match lang {
            ErrorLanguage::Ru => "путь не подходит для проверки",
            ErrorLanguage::En => "the path cannot be checked",
        };
        invalid(format!("{head}: {}", why.message(lang)))
    })?;
    Ok(summarize(checked, lang))
}

fn summarize(checked: Checked, lang: ErrorLanguage) -> ToolResult {
    let Checked { mut report, skipped, .. } = checked;
    let ru = lang == ErrorLanguage::Ru;
    let total = report.folders.len();
    let passed = report.folders.iter().filter(|f| f.passed).count();
    let failed = total - passed;
    report.folders.sort_by_key(|f| f.passed); // stable: failing first, then by path
    report.folders.truncate(MAX_FOLDERS);

    let mut out = if ru {
        let verdict = if report.overall_passed { "пройдено" } else { "не пройдено" };
        format!("Папок метода: {total}, прошли: {passed}, не прошли: {failed}. Итог: {verdict}.\n")
    } else {
        let verdict = if report.overall_passed { "passed" } else { "failed" };
        format!("Method folders: {total}, passed: {passed}, failed: {failed}. Result: {verdict}.\n")
    };
    if total > report.folders.len() {
        let shown = report.folders.len();
        let _ = writeln!(out, "{}", if ru { format!("Показаны первые {shown} (сначала не прошедшие).") } else { format!("Showing the first {shown} (failing first).") });
    }
    for folder in &report.folders {
        out.push('\n');
        write_folder(&mut out, folder);
    }
    write_skipped(&mut out, &skipped, ru);
    if failed > 0 {
        let _ = writeln!(
            out,
            "\n{}",
            if ru {
                "Как исправлять: docStandards(op: \"guide\", topic: \"rules\"); образец документа — topic: \"template\"."
            } else {
                "How to fix: docStandards(op: \"guide\", topic: \"rules\"); a sample document — topic: \"template\"."
            }
        );
    }
    let summary = match failed {
        0 => format!("{total} passed"),
        _ => format!("{failed} of {total} failed"),
    };
    ToolResult::DocStandards { text: out, summary }
}

fn write_folder(out: &mut String, folder: &FolderReport) {
    let pct = (folder.score * 100).checked_div(folder.max_score).unwrap_or(0);
    let mark = if folder.passed { "✓" } else { "✗" };
    let _ = writeln!(out, "{mark} {} — {}/{} ({pct}%)", folder.folder, folder.score, folder.max_score);
    for f in folder.findings.iter().filter(|f| !f.passed) {
        let _ = writeln!(out, "  • {} {} (−{})", f.rule_id, f.title, f.weight);
        for line in f.message.lines() {
            let _ = writeln!(out, "    {line}");
        }
    }
}

fn write_skipped(out: &mut String, skipped: &[Skipped], ru: bool) {
    if skipped.is_empty() {
        return;
    }
    let _ = writeln!(out, "\n{}", if ru { "Пропущено:" } else { "Skipped:" });
    for s in skipped {
        let _ = writeln!(out, "- {} — {}", s.folder, s.reason);
    }
}

pub(super) fn definition() -> LlmToolDefinition {
    LlmToolDefinition {
        name: "docStandards".to_string(),
        description: "Checks API-method documentation (AsciiDoc method folders: getUser/getUser.adoc, request.adoc, \
            response.adoc, a .puml diagram) against the corporate documentation standard, criteria К.1.1–К.7.1: weighted, \
            a folder passes above 80% of its maximum. op \"check\" (the default) takes `path` — a method folder, a file \
            in one (its folder is checked), or a documentation root with method folders under it, never the open folder itself — and answers with \
            failing folders first, each failed rule with what is wrong and how it should be; it refuses a path with \
            nothing to check, saying why. К.1.3 (links) checks local include/image/xref/link targets only. op \"guide\" \
            takes `topic`: \"rules\" maps every rule to its fix and to what must not be faked — read it before fixing; \
            \"structure\", \"errors\", \"glossary\" are the writing guide for a method document (Russian); \"template\" \
            is a whole sample document that passes. To fix failures: fix only what the findings name, check again, and \
            name the rules that cannot pass without data you do not have instead of inventing content."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "op": { "type": "string", "enum": ["check", "guide"] },
                "path": { "type": ["string", "null"], "description": "Only for op \"check\": a method folder, a file in one, or a documentation root — not `.`, the open folder itself." },
                "topic": {
                    "type": ["string", "null"],
                    "enum": ["rules", "structure", "errors", "glossary", "template", null],
                    "description": "Only for op \"guide\": which part of the guide."
                },
                "language": {
                    "type": ["string", "null"],
                    "enum": ["ru", "en", null],
                    "description": "Only for op \"check\": the language of the findings, `ru` unless said."
                },
                "rules": {
                    "type": ["object", "null"],
                    "additionalProperties": { "type": "boolean" },
                    "description": "Only for op \"check\": rules turned on or off by id, e.g. {\"K.7.1\": false}; every rule is on unless said."
                }
            },
            "required": ["op"],
            "additionalProperties": false
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::doc_standards_rules::{FileEntry, MethodFolderCtx};
    use crate::services::doc_standards::tests::{temp_dir, write_full_method};
    use std::fs;
    use std::path::PathBuf;

    fn args(path: &str) -> DocStandardsArgs {
        DocStandardsArgs { path: Some(path.into()), ..Default::default() }
    }

    fn text(result: ToolResult) -> (String, String) {
        let ToolResult::DocStandards { text, summary } = result else { panic!("{result:?}") };
        (text, summary)
    }

    fn reason(error: ToolError) -> String {
        let ToolError::InvalidArguments { reason, .. } = error else { panic!("{error:?}") };
        reason
    }

    #[test]
    fn a_check_names_failing_folders_first_from_the_open_folder_and_points_at_the_guide() {
        let root = temp_dir();
        write_full_method(&root.join("docs"), "aGood");
        let bad = root.join("docs/zBad");
        fs::create_dir_all(&bad).unwrap();
        fs::write(bad.join("zBad.adoc"), "= zBad").unwrap();
        fs::create_dir_all(root.join("docs/external")).unwrap();
        let scope = ToolScope::new(&root).unwrap();

        let (out, summary) = text(doc_standards(&scope, &DocStandardsArgs { language: Some(ErrorLanguage::En), ..args("docs") }).unwrap());
        assert!(out.starts_with("Method folders: 2, passed: 1, failed: 1. Result: failed."), "{out}");
        assert!(out.find("✗ docs/zBad").unwrap() < out.find("✓ docs/aGood — 100/100 (100%)").unwrap(), "{out}");
        assert!(out.contains("  • K.1.1 ") && out.contains("    What's wrong:"), "{out}");
        assert!(out.contains("Skipped:\n- docs/external — folder name «external» is excluded by the standard"), "{out}");
        assert!(out.contains("docStandards(op: \"guide\", topic: \"rules\")"), "{out}");
        assert_eq!(summary, "1 of 2 failed");

        let (out, summary) = text(doc_standards(&scope, &args("docs/aGood")).unwrap());
        assert_eq!(out, "Папок метода: 1, прошли: 1, не прошли: 0. Итог: пройдено.\n\n✓ docs/aGood — 100/100 (100%)\n");
        assert_eq!(summary, "1 passed");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_passed_folder_lists_no_findings_and_a_failed_one_only_its_failures() {
        let root = temp_dir();
        write_full_method(&root, "getUser");
        fs::remove_file(root.join("getUser/response.adoc")).unwrap();
        let scope = ToolScope::new(&root).unwrap();
        let (out, _) = text(doc_standards(&scope, &args("getUser")).unwrap());
        let bullets: Vec<&str> = out.lines().filter(|l| l.starts_with("  • ")).collect();
        assert!(bullets.iter().all(|l| l.starts_with("  • K.1.1 ") || l.starts_with("  • K.1.3 ") || l.starts_with("  • K.5")), "{out}");
        assert!(!bullets.is_empty());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rules_can_be_turned_off_and_an_unknown_id_is_refused_with_the_known_ones() {
        let root = temp_dir();
        write_full_method(&root, "getUser");
        let scope = ToolScope::new(&root).unwrap();
        let off = DocStandardsArgs { rules: Some([("K.7.1".to_string(), false)].into()), ..args("getUser") };
        let (out, _) = text(doc_standards(&scope, &off).unwrap());
        assert!(out.contains("97/97"), "{out}");

        let unknown = DocStandardsArgs { rules: Some([("K.9.9".to_string(), false)].into()), ..args("getUser") };
        let why = reason(doc_standards(&scope, &unknown).unwrap_err());
        assert!(why.starts_with("unknown rule id(s): K.9.9; known: K.1.1, K.1.2,"), "{why}");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_path_is_needed_inside_the_folder_and_one_with_nothing_to_check_says_why() {
        let root = temp_dir();
        fs::create_dir_all(root.join("images")).unwrap();
        let scope = ToolScope::new(&root).unwrap();
        assert_eq!(reason(doc_standards(&scope, &DocStandardsArgs::default()).unwrap_err()), "op «check» needs `path`");
        assert!(matches!(doc_standards(&scope, &args("../")), Err(ToolError::PathEscape(_))));
        assert!(matches!(doc_standards(&scope, &args("missing")), Err(ToolError::NotFound(_))));
        for root_itself in [".", "./"] {
            let why = reason(doc_standards(&scope, &args(root_itself)).unwrap_err());
            assert!(why.starts_with("`path` is the open folder itself"), "{root_itself}: {why}");
        }
        let why = reason(doc_standards(&scope, &args("images")).unwrap_err());
        assert!(why.starts_with("путь не подходит для проверки: не найдено ни одной папки метода"), "{why}");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_long_report_keeps_the_failing_folders() {
        let root = temp_dir();
        for i in 0..MAX_FOLDERS {
            write_full_method(&root.join("docs"), &format!("m{i:03}"));
        }
        let bad = root.join("docs/zBad");
        fs::create_dir_all(&bad).unwrap();
        fs::write(bad.join("zBad.adoc"), "= zBad").unwrap();
        let scope = ToolScope::new(&root).unwrap();
        let (out, _) = text(doc_standards(&scope, &args("docs")).unwrap());
        assert!(out.starts_with("Папок метода: 101, прошли: 100, не прошли: 1. Итог: не пройдено.\nПоказаны первые 100 (сначала не прошедшие).\n\n✗ docs/zBad"), "{out}");
        assert_eq!(out.matches("\n✓ ").count(), 99);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_guide_is_read_by_topic_and_a_wrong_one_is_told_the_right_ones() {
        let read = |topic: Option<&str>| doc_standards(&ToolScope::new(&std::env::temp_dir()).unwrap(), &DocStandardsArgs { op: DocStandardsOp::Guide, topic: topic.map(Into::into), ..Default::default() });
        let (out, summary) = text(read(Some("rules")).unwrap());
        assert!(out.starts_with("# Как исправлять непройденные правила стандарта"), "{out}");
        assert_eq!(summary, "guide: rules");
        assert_eq!(reason(read(Some("rule")).unwrap_err()), "there is no topic «rule» — there are: rules, structure, errors, glossary, template");
        assert!(reason(read(None).unwrap_err()).starts_with("op «guide» needs `topic`"));
    }

    /// The guide tells the model to copy the sample document, so the sample
    /// has to pass on its own.
    #[test]
    fn the_guides_template_passes_the_check() {
        let method = "createSignOperationV2";
        let template = GUIDE.iter().find(|(name, _)| *name == "template").unwrap().1;
        let file = |name: String, content: &str| FileEntry { path: PathBuf::from(&name), name, content: content.to_string() };
        let ctx = MethodFolderCtx::new(
            method.to_string(),
            vec![
                file(format!("{method}.adoc"), &template.replace("{имяРучки}", method)),
                file("request.adoc".into(), "[discrete#endpoint]\n=== Endpoint\n\n`POST /api/documents`\n"),
                file("response.adoc".into(), "[source,json]\n----\n{}\n----\n"),
                file(format!("{method}.puml"), "@startuml\nA -> B\n@enduml\n"),
            ],
            |_| true,
        );
        let failed: Vec<&str> = RULES.iter().filter(|r| !(r.check)(&ctx).passed).map(|r| r.def.id).collect();
        assert!(failed.is_empty(), "template fails rules: {failed:?}");
    }

    /// Every rule the check knows has a fix in the guide.
    #[test]
    fn the_guide_covers_every_rule() {
        let rules = GUIDE[0].1;
        for rule in RULES {
            let id = rule.def.id.replace('K', "К"); // the guide uses the standard's Cyrillic К
            assert!(rules.contains(&format!("| {id} |")), "doc-standards/rules.md has no row for {}", rule.def.id);
        }
    }
}
