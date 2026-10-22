//! The documentation standard's check (`Feature::DocStandards`): API-method
//! folders in AsciiDoc scored against criteria К.1.1–К.7.1, a folder passing
//! above 80% of its maximum. Types and scoring here; the rules in
//! `doc_standards_rules`, their texts in `doc_standards_messages`, the walk
//! that finds method folders in `services::doc_standards`.
//!
//! Ported from kibo-mcp-asciidoc-standarts (`model.rs`, `checker.rs`), which
//! took it from Alfa Atlas. The one change: К.1.3 asks `exists` whether a
//! link's target is there instead of the file system, so this stays free of
//! I/O.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::doc_standards_messages as messages;
use super::doc_standards_rules::{FileEntry, MethodFolderCtx, RULES};

/// Language of the rules' "what's wrong / how it should be" messages.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorLanguage {
    #[serde(rename = "ru")]
    #[default]
    Ru,
    #[serde(rename = "en")]
    En,
}

/// Static metadata for one standard rule (e.g. "К.1.1"). The check itself is
/// a separate `fn` paired with this in `RULES`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuleDef {
    pub id: &'static str,
    pub title: &'static str,
    pub weight: u32,
    pub default_enabled: bool,
}

/// Outcome of running one rule's check function against a method folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleOutcome {
    pub passed: bool,
    pub message: String,
}

/// One rule's result for one method folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub rule_id: String,
    pub title: String,
    pub passed: bool,
    pub weight: u32,
    pub message: String,
}

/// Result of checking one `methodName` folder against all enabled rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderReport {
    /// Relative to the checked path, `/`-separated (e.g. `users/getUser`).
    pub folder: String,
    pub method_name: String,
    pub score: u32,
    pub max_score: u32,
    /// `score / max_score > 0.8`, the standard's 80% threshold.
    pub passed: bool,
    pub findings: Vec<Finding>,
}

/// Result of checking every method folder found, sorted by folder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StandardsReport {
    pub folders: Vec<FolderReport>,
    /// True only if at least one folder was found and every folder passed.
    pub overall_passed: bool,
}

/// Enable/disable overrides, keyed by `RuleDef::id`. A rule absent from the
/// map falls back to `RuleDef::default_enabled`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StandardsRuleConfig {
    pub rules: HashMap<String, bool>,
}

impl StandardsRuleConfig {
    pub fn is_enabled(&self, def: &RuleDef) -> bool {
        self.rules.get(def.id).copied().unwrap_or(def.default_enabled)
    }
}

/// One discovered method folder with its relevant files already read.
#[derive(Debug, Clone)]
pub struct MethodFolder {
    /// Relative to the checked path, `/`-separated; the folder's own name
    /// when the checked path is the method folder itself.
    pub folder: String,
    pub method_name: String,
    pub files: Vec<FileEntry>,
}

/// Directory names excluded from the check, exactly as the standard's
/// "Исключения" table for К.1.1 lists them — case-sensitive, so `CONFIG` or
/// `EXTERNAL` are checked.
const EXCLUDED_DIR_NAMES: &[&str] =
    &["External", "_External", "external", "_external", "config", "Config", "Template", "template"];

/// A folder left out of the check, and why — in the check's language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    pub folder: String,
    pub reason: String,
}

/// Why a path has nothing to check. `Display` is for logs; what the user's
/// model reads is [`NothingToCheck::message`], in the check's language.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum NothingToCheck {
    #[error("path is neither a folder nor a file")]
    NotAFolder,
    #[error("folder name «{0}» is excluded by the standard")]
    ExcludedName(String),
    #[error("failed to read the folder")]
    Unreadable,
    #[error("the method folder is excluded by an Exceptions marker")]
    MarkedExcluded,
    #[error("no method folder found")]
    NoMethodFolder,
    #[error("every method folder found is excluded")]
    AllExcluded(Vec<Skipped>),
}

impl NothingToCheck {
    pub fn message(&self, lang: ErrorLanguage) -> String {
        match self {
            NothingToCheck::NotAFolder => t(lang, "путь не является папкой или файлом", "path is neither a folder nor a file"),
            NothingToCheck::ExcludedName(name) => excluded_dir_reason(name, lang).unwrap_or_default(),
            NothingToCheck::Unreadable => t(lang, "не удалось прочитать папку", "failed to read the folder"),
            NothingToCheck::MarkedExcluded => t(
                lang,
                "папка метода целиком исключена из проверки маркером Exceptions",
                "the method folder is excluded from the check by an Exceptions marker",
            ),
            NothingToCheck::NoMethodFolder => t(
                lang,
                "не найдено ни одной папки метода: нужна папка с собственным .adoc-файлом (например getUser/getUser.adoc)",
                "no method folder found: expected a folder with its own .adoc file (e.g. getUser/getUser.adoc)",
            ),
            NothingToCheck::AllExcluded(skipped) => {
                let why: Vec<String> = skipped.iter().map(|s| format!("{} — {}", s.folder, s.reason)).collect();
                t(
                    lang,
                    &format!("все найденные папки методов исключены из проверки: {}", why.join("; ")),
                    &format!("every method folder found is excluded from the check: {}", why.join("; ")),
                )
            }
        }
    }
}

fn t(lang: ErrorLanguage, ru: &str, en: &str) -> String {
    match lang {
        ErrorLanguage::Ru => ru.to_string(),
        ErrorLanguage::En => en.to_string(),
    }
}

/// Why the standard excludes a directory by its name, or `None`.
pub fn excluded_dir_reason(name: &str, lang: ErrorLanguage) -> Option<String> {
    if EXCLUDED_DIR_NAMES.contains(&name) {
        return Some(t(
            lang,
            &format!("имя папки «{name}» исключено стандартом"),
            &format!("folder name «{name}» is excluded by the standard"),
        ));
    }
    // Case-sensitive, like the standard's list.
    if name.starts_with("External_") || name.starts_with("external_") || name.contains("SOAP") || name.starts_with("lib_") {
        return Some(t(
            lang,
            &format!("имя папки «{name}» подпадает под маску исключений (External_*, external_*, *SOAP*, lib_*)"),
            &format!("folder name «{name}» matches an excluded mask (External_*, external_*, *SOAP*, lib_*)"),
        ));
    }
    None
}

pub fn exception_reason(lang: ErrorLanguage) -> String {
    t(
        lang,
        "папка исключена маркером «Exceptions - Файл исключен из проверки документации»",
        "folder is excluded by the «Exceptions - Файл исключен из проверки документации» marker",
    )
}

/// Markers (§6 of the standard) that auto-pass the К.4 / К.5 groups.
/// Matched case-insensitively as a substring.
const EXCEPTION_INPUT_PARAMS: &str = "exceptions - описание входных параметров исключено из проверки документации";
const EXCEPTION_OUTPUT_PARAMS: &str = "exceptions - описание выходных параметров исключено из проверки документации";

/// Scores every folder. `exists` answers К.1.3: whether a link's target is on disk.
pub fn check(
    methods: Vec<MethodFolder>,
    config: &StandardsRuleConfig,
    lang: ErrorLanguage,
    exists: fn(&Path) -> bool,
) -> StandardsReport {
    let mut folders: Vec<FolderReport> = methods.into_iter().map(|m| evaluate(m, config, lang, exists)).collect();
    folders.sort_by(|a, b| a.folder.cmp(&b.folder));
    let overall_passed = !folders.is_empty() && folders.iter().all(|f| f.passed);
    StandardsReport { folders, overall_passed }
}

fn evaluate(method: MethodFolder, config: &StandardsRuleConfig, lang: ErrorLanguage, exists: fn(&Path) -> bool) -> FolderReport {
    let has_marker = |marker: &str| method.files.iter().any(|f| f.content.to_lowercase().contains(marker));
    let has_input_exception = has_marker(EXCEPTION_INPUT_PARAMS);
    let has_output_exception = has_marker(EXCEPTION_OUTPUT_PARAMS);

    let mut ctx = MethodFolderCtx::new(method.method_name.clone(), method.files, exists);
    ctx.lang = lang;

    let mut findings = Vec::new();
    let mut score = 0u32;
    let mut max_score = 0u32;

    for rule in RULES {
        if !config.is_enabled(&rule.def) {
            continue;
        }
        max_score += rule.def.weight;

        let auto_pass = (rule.def.id.starts_with("K.4") && has_input_exception)
            || (rule.def.id.starts_with("K.5") && has_output_exception);

        let outcome = if auto_pass {
            RuleOutcome { passed: true, message: messages::passed(lang) }
        } else {
            (rule.check)(&ctx)
        };

        if outcome.passed {
            score += rule.def.weight;
        }
        findings.push(Finding {
            rule_id: rule.def.id.to_string(),
            title: rule.def.title.to_string(),
            passed: outcome.passed,
            weight: rule.def.weight,
            message: outcome.message,
        });
    }

    FolderReport {
        folder: method.folder,
        method_name: method.method_name,
        score,
        max_score,
        passed: max_score > 0 && score * 100 > max_score * 80,
        findings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn config_falls_back_to_default_enabled() {
        let def = RuleDef { id: "K.1.1", title: "test", weight: 20, default_enabled: true };
        let config = StandardsRuleConfig::default();
        assert!(config.is_enabled(&def));

        let mut config = StandardsRuleConfig::default();
        config.rules.insert("K.1.1".to_string(), false);
        assert!(!config.is_enabled(&def));
    }

    fn file(name: &str, content: &str) -> FileEntry {
        FileEntry { name: name.into(), path: PathBuf::from(name), content: content.into() }
    }

    fn folder(name: &str, files: Vec<FileEntry>) -> MethodFolder {
        MethodFolder { folder: name.into(), method_name: name.into(), files }
    }

    /// Every link target is there: К.1.3 is about the rest of the rules here.
    fn all_exist(_: &Path) -> bool {
        true
    }

    fn full_method(method: &str) -> MethodFolder {
        let main = format!(
            "= {method}\n:toc:\n\n== Назначение\nЭто достаточно длинное описание метода для прохождения проверки на пятьдесят символов.\n\n== Схема работы\nimage::{method}.puml[]\n\n== Описание входных параметров\n|===\n| Имя | Тип | Обязательный | Описание\n| id | string | да | идентификатор\n|===\n\ninclude::./request.adoc[]\n\n== Описание выходных параметров\n|===\n| Имя | Тип | Обязательный | Описание\n| id | string | да | идентификатор\n|===\n\ninclude::./response.adoc[]\n\n== Алгоритм работы\nШаг 1.\n\n== Обработка ошибок\n404 - не найдено.\n"
        );
        folder(
            method,
            vec![
                file(&format!("{method}.puml"), "@startuml\n@enduml"),
                file(&format!("{method}.adoc"), &main),
                file("request.adoc", "${HOST}/api/x\ncurl example"),
                file("response.adoc", "{}"),
            ],
        )
    }

    #[test]
    fn a_full_folder_scores_everything_and_the_report_passes() {
        let report = check(vec![full_method("getUser")], &StandardsRuleConfig::default(), ErrorLanguage::Ru, all_exist);
        let folder = &report.folders[0];
        assert!(folder.passed, "{folder:?}");
        assert_eq!((folder.score, folder.max_score), (100, 100));
        assert_eq!(folder.findings.len(), RULES.len());
        assert!(report.overall_passed);
    }

    #[test]
    fn missing_files_fail_the_folder_and_the_report() {
        let report = check(
            vec![full_method("aGood"), folder("zBad", vec![file("zBad.adoc", "= zBad")])],
            &StandardsRuleConfig::default(),
            ErrorLanguage::Ru,
            all_exist,
        );
        let names: Vec<_> = report.folders.iter().map(|f| (f.folder.as_str(), f.passed)).collect();
        assert_eq!(names, [("aGood", true), ("zBad", false)]);
        assert!(!report.overall_passed);
    }

    #[test]
    fn nothing_found_is_not_a_pass() {
        assert!(!check(vec![], &StandardsRuleConfig::default(), ErrorLanguage::Ru, all_exist).overall_passed);
    }

    /// The threshold is strict: exactly 80% fails, as the standard says "above".
    #[test]
    fn eighty_percent_exactly_fails_and_one_point_more_passes() {
        // An empty request example fails К.4.4 (weight 3) and nothing else.
        let mut method = full_method("getUser");
        method.files[2].content = String::new();
        let report = check(vec![method.clone()], &StandardsRuleConfig::default(), ErrorLanguage::Ru, all_exist);
        let failed: Vec<_> = report.folders[0].findings.iter().filter(|f| !f.passed).map(|f| f.rule_id.as_str()).collect();
        assert_eq!(failed, ["K.4.4"]);

        // Only rules of weight 3 on: with five of them the failed one is 20%.
        let score = |ids: &[&str]| {
            let mut config = StandardsRuleConfig { rules: RULES.iter().map(|r| (r.def.id.to_string(), false)).collect() };
            for &id in ids {
                config.rules.insert(id.into(), true);
            }
            let folder = check(vec![method.clone()], &config, ErrorLanguage::Ru, all_exist).folders.remove(0);
            (folder.score, folder.max_score, folder.passed)
        };
        assert_eq!(score(&["K.4.4", "K.1.3", "K.2.1", "K.2.2", "K.3.1"]), (12, 15, false), "exactly 80% must fail");
        assert_eq!(score(&["K.4.4", "K.1.3", "K.2.1", "K.2.2", "K.3.1", "K.6.1"]), (15, 18, true));
    }

    #[test]
    fn input_exception_marker_autopasses_k4_rules_only() {
        let mut method = full_method("getUser");
        method.files[1].content = method.files[1].content.replace(
            "|===\n| Имя | Тип | Обязательный | Описание\n| id | string | да | идентификатор\n|===\n\ninclude::./request.adoc[]",
            "Exceptions - Описание входных параметров исключено из проверки документации",
        );
        method.files[2].content = String::new();
        let report = check(vec![method], &StandardsRuleConfig::default(), ErrorLanguage::Ru, all_exist);
        for finding in &report.folders[0].findings {
            assert!(finding.passed, "{} should pass: {}", finding.rule_id, finding.message);
        }
    }

    #[test]
    fn output_exception_marker_autopasses_k5_rules() {
        let mut method = full_method("getUser");
        method.files.retain(|f| f.name != "response.adoc");
        method.files[1].content.push_str("\nExceptions - Описание выходных параметров исключено из проверки документации\n");
        let report = check(vec![method], &StandardsRuleConfig::default(), ErrorLanguage::Ru, all_exist);
        let failed: Vec<_> = report.folders[0].findings.iter().filter(|f| !f.passed).map(|f| f.rule_id.as_str()).collect();
        // The response file is still missing for К.1.1; nothing of К.5 fails.
        assert_eq!(failed, ["K.1.1"]);
    }

    #[test]
    fn disabling_a_rule_reduces_max_score() {
        let mut config = StandardsRuleConfig::default();
        config.rules.insert("K.7.1".to_string(), false);
        let report = check(vec![full_method("getUser")], &config, ErrorLanguage::Ru, all_exist);
        assert_eq!(report.folders[0].max_score, 97);
        assert!(report.folders[0].findings.iter().all(|f| f.rule_id != "K.7.1"));
    }

    /// With every rule off there is nothing to score, and nothing is a pass.
    #[test]
    fn a_folder_with_every_rule_off_does_not_pass() {
        let config = StandardsRuleConfig { rules: RULES.iter().map(|r| (r.def.id.to_string(), false)).collect() };
        let folder = check(vec![full_method("getUser")], &config, ErrorLanguage::Ru, all_exist).folders.remove(0);
        assert_eq!((folder.score, folder.max_score, folder.passed), (0, 0, false));
    }

    #[test]
    fn link_targets_are_asked_of_exists() {
        fn none_exist(_: &Path) -> bool {
            false
        }
        let report = check(vec![full_method("getUser")], &StandardsRuleConfig::default(), ErrorLanguage::En, none_exist);
        let k13 = report.folders[0].findings.iter().find(|f| f.rule_id == "K.1.3").unwrap();
        assert!(!k13.passed);
        assert!(k13.message.starts_with("What's wrong"), "{}", k13.message);
    }
}
