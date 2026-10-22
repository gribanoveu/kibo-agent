//! Finds the `methodName` folders under a path (К.1.1 of the standard), reads
//! their files and scores them with `domain::doc_standards` — what the
//! `docStandards` tool checks.
//!
//! Ported from kibo-mcp-asciidoc-standarts (`target.rs`). What changed: the
//! path arrives already resolved inside the open folder (`resolve.rs`), so the
//! server's own checks — absolute, `--allow-root` — are gone, and folders are
//! named from the open folder's root, as every other tool names files.

use std::fs;
use std::path::{Path, PathBuf};

use crate::domain::doc_standards::{
    self, excluded_dir_reason, exception_reason, ErrorLanguage, MethodFolder, NothingToCheck, Skipped, StandardsReport,
    StandardsRuleConfig,
};
use crate::domain::doc_standards_rules::FileEntry;

/// Individual files excluded from role-matching / content checks, but that
/// do not disqualify the folder they live in.
const EXCLUDED_FILE_NAMES: &[&str] = &[
    "index.adoc",
    "db.adoc",
    "config.adoc",
    "compositeexception.adoc",
    "systemparams.adoc",
    "_common.adoc",
    "_configs.adoc",
    "kafka.adoc",
    "mongodb.adoc",
];

/// Text file extensions relevant to the rules; everything else in a method
/// folder (images, binaries) is ignored.
const RELEVANT_EXTENSIONS: &[&str] = &[".adoc", ".asciidoc", ".puml", ".plantuml"];

/// Marker text (§6 of the standard) that excludes a whole folder from the
/// check. Matched case-insensitively as a substring.
const EXCEPTION_WHOLE_FILE: &str = "exceptions - файл исключен из проверки документации";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    /// The path is itself one method folder.
    Method,
    /// The path is a docs root (or any directory above method folders).
    Tree,
}

/// A path that could be checked, and what came of it.
#[derive(Debug)]
pub struct Checked {
    pub kind: TargetKind,
    pub report: StandardsReport,
    pub skipped: Vec<Skipped>,
}

/// Scores the method folders at or under `path`, an existing path inside
/// `base` (a file stands for its folder). Folders are named relative to
/// `base`.
pub fn check(base: &Path, path: &Path, config: &StandardsRuleConfig, lang: ErrorLanguage) -> Result<Checked, NothingToCheck> {
    let target = resolve(base, path, lang)?;
    let report = doc_standards::check(target.methods, config, lang, |p| p.exists());
    Ok(Checked { kind: target.kind, report, skipped: target.skipped })
}

struct Target {
    kind: TargetKind,
    methods: Vec<MethodFolder>,
    skipped: Vec<Skipped>,
}

fn is_excluded_file_name(name: &str) -> bool {
    EXCLUDED_FILE_NAMES.contains(&name.to_lowercase().as_str())
}

fn is_relevant_extension(name: &str) -> bool {
    let lower = name.to_lowercase();
    RELEVANT_EXTENSIONS.iter().any(|ext| lower.ends_with(ext))
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn resolve(base: &Path, path: &Path, lang: ErrorLanguage) -> Result<Target, NothingToCheck> {
    // The standard evaluates a method folder, not a file.
    let root = if path.is_file() { path.parent().map(Path::to_path_buf).unwrap_or_else(|| path.to_path_buf()) } else { path.to_path_buf() };
    if !root.is_dir() {
        return Err(NothingToCheck::NotAFolder);
    }
    let root_name = file_name(&root);
    if excluded_dir_reason(&root_name, lang).is_some() {
        return Err(NothingToCheck::ExcludedName(root_name));
    }

    let mut methods = Vec::new();
    let mut skipped = Vec::new();
    let Ok(listing) = list_dir(&root) else {
        return Err(NothingToCheck::Unreadable);
    };
    for sub in &listing.subdirs {
        walk(base, sub, lang, &mut methods, &mut skipped);
    }
    note_excluded(base, &root, &listing, lang, &mut skipped);

    // A folder with its own documents and no method folders beneath it is a
    // method folder; otherwise it is a container and its loose files (a
    // repo-wide overview, say) are not a method.
    let kind = if listing.has_own_adoc() && methods.is_empty() {
        let folder = relative(base, &root);
        match load_method(&root, folder.clone(), root_name) {
            Ok(m) => methods.push(m),
            Err(()) => skipped.push(Skipped { folder, reason: exception_reason(lang) }),
        }
        TargetKind::Method
    } else {
        TargetKind::Tree
    };

    skipped.sort_by(|a, b| a.folder.cmp(&b.folder));

    if methods.is_empty() {
        return Err(match kind {
            TargetKind::Method => NothingToCheck::MarkedExcluded,
            TargetKind::Tree if skipped.is_empty() => NothingToCheck::NoMethodFolder,
            TargetKind::Tree => NothingToCheck::AllExcluded(skipped),
        });
    }
    Ok(Target { kind, methods, skipped })
}

struct Listing {
    subdirs: Vec<PathBuf>,
    /// Names of subdirectories the standard excludes.
    excluded: Vec<String>,
    files: Vec<PathBuf>,
}

impl Listing {
    fn has_own_adoc(&self) -> bool {
        self.files.iter().any(|p| {
            let name = file_name(p);
            name.to_lowercase().ends_with(".adoc") && !is_excluded_file_name(&name)
        })
    }
}

/// Excluded subdirectories are remembered with their reason instead of being
/// silently dropped; hidden ones (`.git`) are not worth a mention. Symbolic
/// links are neither folders nor files here, so the walk stays inside.
///
/// ponytail: walks everything not hidden, `node_modules` and `target` too —
/// a check of a whole repository's root costs their walk. Skip the
/// gitignored (`infra::workspace_scanner`) if that is ever slow.
fn list_dir(dir: &Path) -> std::io::Result<Listing> {
    let mut listing = Listing { subdirs: Vec::new(), excluded: Vec::new(), files: Vec::new() };
    for entry in fs::read_dir(dir)?.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if file_type.is_dir() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            if excluded_dir_reason(&name, ErrorLanguage::default()).is_some() {
                listing.excluded.push(name);
            } else {
                listing.subdirs.push(path);
            }
        } else if file_type.is_file() {
            listing.files.push(path);
        }
    }
    Ok(listing)
}

fn note_excluded(base: &Path, dir: &Path, listing: &Listing, lang: ErrorLanguage, skipped: &mut Vec<Skipped>) {
    for name in &listing.excluded {
        skipped.push(Skipped {
            folder: relative(base, &dir.join(name)),
            reason: excluded_dir_reason(name, lang).unwrap_or_default(),
        });
    }
}

fn walk(base: &Path, dir: &Path, lang: ErrorLanguage, methods: &mut Vec<MethodFolder>, skipped: &mut Vec<Skipped>) {
    let Ok(listing) = list_dir(dir) else {
        return;
    };
    note_excluded(base, dir, &listing, lang, skipped);
    if listing.has_own_adoc() {
        let folder = relative(base, dir);
        match load_method(dir, folder.clone(), file_name(dir)) {
            Ok(m) => methods.push(m),
            Err(()) => skipped.push(Skipped { folder, reason: exception_reason(lang) }),
        }
    }
    for sub in &listing.subdirs {
        walk(base, sub, lang, methods, skipped);
    }
}

/// Reads the folder's relevant files; `Err` when one of them carries the
/// whole-folder exception marker.
fn load_method(dir: &Path, folder: String, method_name: String) -> Result<MethodFolder, ()> {
    let mut files = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(MethodFolder { folder, method_name, files });
    };
    for entry in entries.flatten() {
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_excluded_file_name(&name) || !is_relevant_extension(&name) {
            continue;
        }
        let path = entry.path();
        let content = fs::read_to_string(&path).unwrap_or_default();
        if content.to_lowercase().contains(EXCEPTION_WHOLE_FILE) {
            return Err(());
        }
        files.push(FileEntry { name, path, content });
    }
    files.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(MethodFolder { folder, method_name, files })
}

/// `dir` relative to `base`, `/`-separated so reports read the same on every
/// OS; `.` for `base` itself.
fn relative(base: &Path, dir: &Path) -> String {
    let rel = dir.strip_prefix(base).unwrap_or(dir);
    let parts: Vec<_> = rel.iter().map(|c| c.to_string_lossy()).collect();
    if parts.is_empty() { ".".to_string() } else { parts.join("/") }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    pub(crate) fn temp_dir() -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!("kibo-doc-standards-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    pub(crate) fn write_full_method(root: &Path, method: &str) {
        let dir = root.join(method);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(format!("{method}.puml")), "@startuml\n@enduml").unwrap();
        let main = format!(
            "= {method}\n:toc:\n\n== Назначение\nЭто достаточно длинное описание метода для прохождения проверки на пятьдесят символов.\n\n== Схема работы\nimage::{method}.puml[]\n\n== Описание входных параметров\n|===\n| Имя | Тип | Обязательный | Описание\n| id | string | да | идентификатор\n|===\n\ninclude::./request.adoc[]\n\n== Описание выходных параметров\n|===\n| Имя | Тип | Обязательный | Описание\n| id | string | да | идентификатор\n|===\n\ninclude::./response.adoc[]\n\n== Алгоритм работы\nШаг 1.\n\n== Обработка ошибок\n404 - не найдено.\n"
        );
        fs::write(dir.join(format!("{method}.adoc")), main).unwrap();
        fs::write(dir.join("request.adoc"), "${HOST}/api/x\ncurl example").unwrap();
        fs::write(dir.join("response.adoc"), "{}").unwrap();
    }

    fn run(base: &Path, path: &Path) -> Result<Checked, String> {
        check(base, path, &StandardsRuleConfig::default(), ErrorLanguage::Ru).map_err(|e| e.message(ErrorLanguage::Ru))
    }

    fn folders(checked: &Checked) -> Vec<&str> {
        checked.report.folders.iter().map(|f| f.folder.as_str()).collect()
    }

    #[test]
    fn a_full_folder_on_disk_passes_with_its_links_found() {
        let root = temp_dir();
        write_full_method(&root, "getUser");
        let checked = run(&root, &root).unwrap();
        assert_eq!(checked.kind, TargetKind::Tree);
        let folder = &checked.report.folders[0];
        assert_eq!(folder.folder, "getUser");
        assert!(folder.passed && folder.findings.iter().all(|f| f.passed), "{folder:?}");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_method_folder_or_a_file_in_it_is_checked_alone_and_named_from_the_base() {
        let root = temp_dir();
        write_full_method(&root.join("users"), "getUser");
        write_full_method(&root.join("users"), "listUsers");
        for path in [root.join("users/getUser"), root.join("users/getUser/request.adoc")] {
            let checked = run(&root, &path).unwrap();
            assert_eq!(checked.kind, TargetKind::Method);
            assert_eq!(folders(&checked), ["users/getUser"]);
        }
        let checked = run(&root, &root.join("users/getUser")).unwrap();
        assert_eq!(checked.report.folders[0].method_name, "getUser");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_docs_root_is_a_tree_with_nested_methods_and_its_overview_is_no_method() {
        let root = temp_dir();
        write_full_method(&root.join("users"), "getUser");
        write_full_method(&root.join("users"), "listUsers");
        write_full_method(&root.join("external"), "vendorCall");
        fs::write(root.join("overview.adoc"), "= Обзор").unwrap();
        fs::create_dir_all(root.join(".git/x")).unwrap();
        fs::write(root.join(".git/x/x.adoc"), "= hidden").unwrap();
        let checked = run(&root, &root).unwrap();
        assert_eq!(checked.kind, TargetKind::Tree);
        assert_eq!(folders(&checked), ["users/getUser", "users/listUsers"]);
        assert_eq!(checked.skipped, [Skipped { folder: "external".into(), reason: "имя папки «external» исключено стандартом".into() }]);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_folder_without_method_folders_is_refused() {
        let root = temp_dir();
        fs::create_dir_all(root.join("images")).unwrap();
        fs::write(root.join("images/logo.png"), [0u8; 4]).unwrap();
        assert!(run(&root, &root).unwrap_err().starts_with("не найдено ни одной папки метода"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_excluded_folder_named_directly_is_refused() {
        let root = temp_dir();
        write_full_method(&root.join("lib_common"), "helper");
        let why = run(&root, &root.join("lib_common")).unwrap_err();
        assert!(why.contains("lib_common"), "{why}");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_tree_where_every_method_is_excluded_says_which_and_why() {
        let root = temp_dir();
        write_full_method(&root.join("Config"), "m");
        let why = run(&root, &root).unwrap_err();
        assert!(why.starts_with("все найденные папки методов исключены") && why.contains("Config — имя папки «Config»"), "{why}");
        fs::remove_dir_all(&root).ok();
    }

    /// The standard lists its exclusion names and masks in exact case:
    /// `*SOAP*` excludes `getSOAPData`, not `getSoapData`.
    #[test]
    fn folder_exclusions_are_case_sensitive_like_the_standard() {
        let root = temp_dir();
        for excluded in ["Config", "getSOAPData", "External_api", "external_x", "_External"] {
            write_full_method(&root.join(excluded), "m");
        }
        // Distinct beyond case from the excluded ones: macOS file systems
        // ignore case, so `Config` and `CONFIG` would be the same folder.
        for checked in ["EXTERNAL", "listSoapItems", "Lib_x"] {
            write_full_method(&root.join(checked), "m");
        }
        let checked = run(&root, &root).unwrap();
        assert_eq!(folders(&checked), ["EXTERNAL/m", "Lib_x/m", "listSoapItems/m"]);
        let skipped: Vec<_> = checked.skipped.iter().map(|s| s.folder.as_str()).collect();
        assert_eq!(skipped, ["Config", "External_api", "_External", "external_x", "getSOAPData"]);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_whole_file_exception_marker_skips_a_folder_or_refuses_it_alone() {
        let root = temp_dir();
        write_full_method(&root, "getUser");
        let dir = root.join("legacy");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("legacy.adoc"), "= legacy\n\nExceptions - Файл исключен из проверки документации").unwrap();
        let checked = run(&root, &root).unwrap();
        assert_eq!(folders(&checked), ["getUser"]);
        assert_eq!(checked.skipped[0].folder, "legacy");
        assert!(run(&root, &dir).unwrap_err().contains("маркером Exceptions"));
        fs::remove_dir_all(&root).ok();
    }

    /// Only the documents count: an image or an excluded file name neither
    /// makes a folder a method nor is read as one of its files.
    #[test]
    fn only_relevant_files_are_read_and_excluded_names_do_not_make_a_method() {
        let root = temp_dir();
        write_full_method(&root, "getUser");
        fs::write(root.join("getUser/logo.png"), [0u8; 4]).unwrap();
        fs::write(root.join("getUser/index.adoc"), "= index").unwrap();
        fs::create_dir_all(root.join("common")).unwrap();
        fs::write(root.join("common/Index.adoc"), "= index").unwrap();
        let target = resolve(&root, &root, ErrorLanguage::Ru).unwrap();
        let names: Vec<_> = target.methods[0].files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["getUser.adoc", "getUser.puml", "request.adoc", "response.adoc"]);
        assert_eq!(target.methods.len(), 1);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_missing_link_target_fails_k1_3_on_disk() {
        let root = temp_dir();
        write_full_method(&root, "getUser");
        fs::remove_file(root.join("getUser/response.adoc")).unwrap();
        let checked = run(&root, &root.join("getUser")).unwrap();
        let k13 = checked.report.folders[0].findings.iter().find(|f| f.rule_id == "K.1.3").unwrap();
        assert!(!k13.passed && k13.message.contains("include::./response.adoc"), "{}", k13.message);
        fs::remove_dir_all(&root).ok();
    }

    /// Thrift-style method folder with vertical input tables and multi-line /
    /// rowspan output tables — the layout that previously tripped K.4.2/K.5.2.
    fn write_thrift_method_with_multiline_tables(root: &Path) {
        const METHOD: &str = "fetchAusnTransactions";
        let dir = root.join(METHOD);
        fs::create_dir_all(&dir).unwrap();

        fs::write(dir.join(format!("{METHOD}.puml")), "@startuml\n@enduml").unwrap();

        fs::write(
            dir.join(format!("{METHOD}.adoc")),
            r#"= fetchAusnTransactions
:toc:

== Назначение метода

Данный метод предназначен для получения транзакций Клиента на режиме АУСН.

== Описание входных/выходных параметров

=== Входные параметры

include::request.adoc[]

=== Выходные параметры

include::response.adoc[]

== Схема работы

include::./fetchAusnTransactions.puml[]

== Алгоритм работы

1. Валидация входных параметров.

== Обработка ошибок

|===
|*Условие* |*Описание* |*Type* |*Code*

| Не прошла валидация userData
| Входные параметры не прошли валидацию
| ValidationException
| validationError
|="#,
        )
        .unwrap();

        fs::write(
            dir.join("request.adoc"),
            r#"[cols="4"]
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
|===

.Endpoint:${HOST}/wlbuh-ausn-api/tapi
{"organizationId":"UBV1IL"}
"#,
        )
        .unwrap();

        fs::write(
            dir.join("response.adoc"),
            r#"[cols="4"]
|===
|*Параметр* |*Тип данных* |*Обязательность* |*Описание*
|lastRequestDateTime|UNIX datetime|required|Дата и время последнего запроса
|transactions|object[]|required|Список транзакций
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
|OperationCategory |string |optional|Разметка 2
|===

[cols="5"]
|===
2+|*Поле* |*Тип данных*|*Обязательность* |*Описание*
.4+|page|number|i32|required |Номер страницы
|size|i32|required|Кол-во элементов на странице
|===

{"lastRequestDateTime":null}
"#,
        )
        .unwrap();
    }

    #[test]
    fn thrift_method_with_multiline_tables_passes_standards_check() {
        let root = temp_dir();
        write_thrift_method_with_multiline_tables(&root);
        let report = run(&root, &root).unwrap().report;
        assert_eq!(report.folders.len(), 1);
        let folder = &report.folders[0];
        let failures: Vec<_> = folder
            .findings
            .iter()
            .filter(|f| !f.passed)
            .map(|f| format!("{}: {}", f.rule_id, f.message))
            .collect();
        assert!(folder.passed, "score {}/{}; failures: {:?}", folder.score, folder.max_score, failures);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_base_itself_is_named_dot() {
        let root = temp_dir();
        write_full_method(&root, "getUser");
        let method = root.join("getUser");
        assert_eq!(folders(&run(&method, &method).unwrap()), ["."]);
        fs::remove_dir_all(&root).ok();
    }
}
