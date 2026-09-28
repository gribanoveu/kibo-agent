//! Reviewing the working tree's changes: which files are sent to the model,
//! how the change is put to it, what counts as a finding, and when it is told
//! to wrap up — every rule here is decided without a model.
//!
//! The review itself is an ordinary turn in `ConversationMode::Review`: the
//! agent reads what it needs and runs what proves something. Two things are
//! decided here rather than left to it. What it is sent: secrets, lockfiles,
//! generated code and prose are left out before anything reaches a model.
//! And where a finding lands: a model does not give line numbers — it quotes
//! the code it means, from the lines the change added, and the quote is found
//! in the diff here. A finding that quotes code the change did not add, or
//! names no one place, is refused back to the agent, which can correct it.

use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::domain::llm::LlmMessage;

/// One changed file, as the review reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDiff {
    /// Root-relative, `/`-separated; the new path of a rename.
    pub path: String,
    pub status: FileStatus,
    pub binary: bool,
    /// The unified diff, from the first hunk on.
    pub patch: String,
    /// The lines on the new side of each hunk, in order.
    pub lines: Vec<NewLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed { from: String },
}

/// A line of the new side of a hunk: added, or context around the change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewLine {
    pub hunk: u32,
    pub number: u32,
    pub added: bool,
    pub text: String,
}

/// Why a changed file is not reviewed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Exclusion {
    Binary,
    /// Nothing added to review: what was removed is context, not code.
    Deleted,
    /// Credentials and keys are not sent to a model.
    Secret,
    /// A lockfile, a build output, vendored or minified code.
    Generated,
    /// Prose: a review looks for bugs, and checking what a text claims
    /// against the code is a hunt through the whole repository.
    Documentation,
}

impl Exclusion {
    fn reason(self) -> &'static str {
        match self {
            Exclusion::Binary => "binary",
            Exclusion::Deleted => "deleted",
            Exclusion::Secret => "may hold secrets — do not read it",
            Exclusion::Generated => "generated or a lockfile",
            Exclusion::Documentation => "documentation",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Excluded {
    pub path: String,
    pub reason: Exclusion,
}

/// The files worth reviewing, and every other one with its reason.
pub fn select(files: Vec<FileDiff>) -> (Vec<FileDiff>, Vec<Excluded>) {
    let mut kept = Vec::new();
    let mut excluded = Vec::new();
    for file in files {
        match exclusion(&file) {
            Some(reason) => excluded.push(Excluded { path: file.path, reason }),
            None => kept.push(file),
        }
    }
    (kept, excluded)
}

fn exclusion(file: &FileDiff) -> Option<Exclusion> {
    let name = file.path.rsplit('/').next().unwrap_or(&file.path).to_ascii_lowercase();
    let segments: Vec<&str> = file.path.split('/').collect();
    let dirs = &segments[..segments.len().saturating_sub(1)];
    if file.binary {
        Some(Exclusion::Binary)
    } else if secret(&name) {
        Some(Exclusion::Secret)
    } else if file.status == FileStatus::Deleted {
        Some(Exclusion::Deleted)
    } else if generated(&name, dirs) {
        Some(Exclusion::Generated)
    } else if [".md", ".mdx", ".markdown", ".txt", ".rst", ".adoc"].iter().any(|ext| name.ends_with(ext)) {
        Some(Exclusion::Documentation)
    } else {
        None
    }
}

fn secret(name: &str) -> bool {
    name == ".env"
        || name.starts_with(".env.")
        || name.starts_with("id_rsa")
        || name.starts_with("id_ed25519")
        || name.starts_with("id_ecdsa")
        || [".pem", ".key", ".p12", ".pfx", ".jks", ".keystore"].iter().any(|ext| name.ends_with(ext))
}

const LOCKFILES: &[&str] = &[
    "cargo.lock",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "bun.lock",
    "bun.lockb",
    "poetry.lock",
    "uv.lock",
    "pipfile.lock",
    "gemfile.lock",
    "composer.lock",
    "go.sum",
    "flake.lock",
];

const GENERATED_DIRS: &[&str] = &[
    "node_modules",
    "vendor",
    "dist",
    "build",
    "target",
    "out",
    ".next",
    ".nuxt",
    "coverage",
    "__snapshots__",
    "__generated__",
];

fn generated(name: &str, dirs: &[&str]) -> bool {
    LOCKFILES.contains(&name)
        || [".min.js", ".min.css", ".map", ".snap", ".pb.go", "_pb2.py", ".g.dart"].iter().any(|ext| name.ends_with(ext))
        || dirs.iter().any(|dir| GENERATED_DIRS.contains(&dir.to_ascii_lowercase().as_str()))
}

/// How much diff the opening message carries, in characters (about 30k
/// tokens). It stays in the history and is resent with every request, so a
/// larger change is named past this, and the agent reads what it needs.
pub const MESSAGE_DIFF_CHARS: usize = 120_000;

/// The opening message of a review: the diffs that fit, in path order, the
/// rest of the change by name, and what was left out and why — and the reply
/// language, when the user chose one: nothing the user typed is in a review's
/// conversation to say it for them.
pub fn review_message(files: &[FileDiff], excluded: &[Excluded], language: Option<&str>) -> String {
    let mut out = String::from("Review the uncommitted changes of this repository — the working tree against HEAD.\n\n<changes>\n");
    let mut room = MESSAGE_DIFF_CHARS;
    let mut unshown = Vec::new();
    for file in files {
        if file.patch.len() > room {
            unshown.push(file.path.as_str());
            continue;
        }
        room -= file.patch.len();
        let status = match &file.status {
            FileStatus::Added => "new file".to_string(),
            FileStatus::Modified => "modified".to_string(),
            FileStatus::Deleted => "deleted".to_string(),
            FileStatus::Renamed { from } => format!("renamed from {from}"),
        };
        out.push_str(&format!("=== {} ({status}) ===\n{}\n", file.path, file.patch.trim_end()));
    }
    out.push_str("</changes>\n");
    if !unshown.is_empty() {
        out.push_str(&format!(
            "\nAlso changed, diff not included for its size — read it with gitDiff or readFile where it matters:\n{}\n",
            unshown.join("\n")
        ));
    }
    if !excluded.is_empty() {
        let lines: Vec<String> = excluded.iter().map(|e| format!("{} — {}", e.path, e.reason.reason())).collect();
        out.push_str(&format!("\nLeft out of the review:\n{}\n", lines.join("\n")));
    }
    crate::domain::prompt::with_language_reminder(out, language)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Severity {
    Critical,
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Category {
    Bug,
    Security,
    Performance,
    Maintainability,
    Other,
}

/// What `reportFinding` takes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FindingArgs {
    pub path: String,
    /// One or a few consecutive lines the change added, copied from the diff.
    pub existing_code: String,
    pub title: String,
    pub body: String,
    pub severity: Option<Severity>,
    pub category: Option<Category>,
    #[serde(default)]
    pub suggestion: Option<String>,
}

/// A finding kept, placed on the new side of the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub category: Category,
    pub title: String,
}

/// Why a finding was refused, worded for the agent that has to fix it.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum FindingError {
    #[error("{path} is not part of this change. reportFinding is for lines the change added; a problem it causes elsewhere goes in your answer, with the file and line")]
    NotChanged { path: String },
    #[error("existingCode was not found among the lines the change added to {path}. Copy one or a few consecutive `+` lines from its diff exactly, without the `+`")]
    NotInChange { path: String },
    #[error("existingCode matches {count} places in {path}. Add a neighbouring line or two so it names one")]
    Ambiguous { path: String, count: usize },
    #[error("{what} is empty")]
    Empty { what: &'static str },
    #[error("this is already reported at {path}:{line} as \"{title}\" — report each problem once")]
    Duplicate { path: String, line: u32, title: String },
}

/// Every place the snippet is on the new side of `file`: consecutive lines
/// of one hunk, compared without leading and trailing space, at least one of
/// them added. A `+` the model copied with the line is dropped; blank lines in
/// the snippet are skipped. Empty when it is not there; more than one when it
/// is too short to say which — a lone `}` or `return None;`.
pub fn locate(file: &FileDiff, code: &str) -> Vec<(u32, u32)> {
    let wanted: Vec<&str> = code
        .lines()
        .map(|line| line.strip_prefix('+').unwrap_or(line).trim())
        .filter(|line| !line.is_empty())
        .collect();
    if wanted.is_empty() {
        return Vec::new();
    }
    let lines: Vec<&NewLine> = file.lines.iter().filter(|line| !line.text.trim().is_empty()).collect();
    lines
        .windows(wanted.len())
        .filter(|window| {
            window.iter().all(|line| line.hunk == window[0].hunk)
                && window.iter().zip(&wanted).all(|(line, want)| line.text.trim() == *want)
                && window.iter().any(|line| line.added)
        })
        .map(|window| (window[0].number, window[window.len() - 1].number))
        .collect()
}

/// The change under review and the findings kept so far — what
/// `reportFinding` checks a finding against and adds it to.
#[derive(Debug, Default)]
pub struct ReviewDesk {
    files: Vec<FileDiff>,
    findings: Mutex<Vec<Finding>>,
}

impl ReviewDesk {
    pub fn new(files: Vec<FileDiff>) -> Self {
        Self { files, findings: Mutex::default() }
    }

    pub fn files(&self) -> &[FileDiff] {
        &self.files
    }

    /// Checks a finding and keeps it: its file is part of the change, its
    /// code is lines the change added, and it is not a second report of one
    /// already kept — the same file, overlapping lines, the same category.
    pub fn note(&self, args: &FindingArgs) -> Result<Finding, FindingError> {
        let path = args.path.trim().trim_start_matches("./");
        for (what, value) in [("path", path), ("title", args.title.trim()), ("body", args.body.trim()), ("existingCode", args.existing_code.trim())] {
            if value.is_empty() {
                return Err(FindingError::Empty { what });
            }
        }
        let file = self.files.iter().find(|file| file.path == path).ok_or_else(|| FindingError::NotChanged { path: path.to_string() })?;
        let (start_line, end_line) = match locate(file, &args.existing_code)[..] {
            [one] => one,
            [] => return Err(FindingError::NotInChange { path: path.to_string() }),
            ref many => return Err(FindingError::Ambiguous { path: path.to_string(), count: many.len() }),
        };
        let finding = Finding {
            path: path.to_string(),
            start_line,
            end_line,
            category: args.category.unwrap_or(Category::Bug),
            title: args.title.trim().to_string(),
        };
        let mut findings = self.findings.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(same) = findings.iter().find(|kept| {
            kept.path == finding.path
                && kept.category == finding.category
                && kept.start_line <= finding.end_line
                && finding.start_line <= kept.end_line
        }) {
            return Err(FindingError::Duplicate { path: same.path.clone(), line: same.start_line, title: same.title.clone() });
        }
        findings.push(finding.clone());
        Ok(finding)
    }
}

/// How much of the turn's budget a review uses before it is asked to wrap
/// up: the last fifth is for reporting what it verified, rather than for a
/// turn that runs out mid-read and reports nothing.
pub const WRAP_UP_AT_PERCENT: u32 = 80;

/// A turn's two ceilings — rounds, and calls weighted by cost — or how much
/// of them is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    pub rounds: u32,
    pub weight: u32,
}

/// What the note starts with — how it is recognised in the history, so it is
/// given once a turn, a pause and a resume included.
const WRAP_UP_MARK: &str = "[Review budget]";

/// The note asking a review to wrap up, once it has used
/// [`WRAP_UP_AT_PERCENT`] of either of the turn's ceilings and has not been
/// asked yet. The ceilings are an ordinary turn's: a review is not cut
/// shorter, only told in time that the end is near.
pub fn wrap_up(used: Budget, limit: Budget, history: &[LlmMessage]) -> Option<String> {
    let percent = |used: u32, limit: u32| (u64::from(used) * 100).checked_div(u64::from(limit)).unwrap_or(100);
    let most = percent(used.rounds, limit.rounds).max(percent(used.weight, limit.weight));
    if most < u64::from(WRAP_UP_AT_PERCENT) {
        return None;
    }
    if history.iter().any(|m| m.content.as_deref().is_some_and(|text| text.starts_with(WRAP_UP_MARK))) {
        return None;
    }
    Some(format!(
        "{WRAP_UP_MARK} This review has used {most}% of its budget ({} of {} rounds); at 100% the turn stops, whatever is unreported. Wrap up now: report with reportFinding what you have already verified, name what you could not settle in your closing answer as worth a look, and open nothing new unless it proves a finding you already have.",
        used.rounds, limit.rounds
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, lines: &[(u32, bool, &str)]) -> FileDiff {
        FileDiff {
            path: path.to_string(),
            status: FileStatus::Modified,
            binary: false,
            patch: String::new(),
            lines: lines.iter().map(|&(number, added, text)| NewLine { hunk: 0, number, added, text: text.to_string() }).collect(),
        }
    }

    /// A file whose diff is `chars` long.
    fn sized(path: &str, chars: usize) -> FileDiff {
        FileDiff { patch: "x".repeat(chars), ..file(path, &[(1, true, "x")]) }
    }

    #[test]
    fn what_is_left_out_and_why() {
        let mut deleted = sized("src/old.rs", 0);
        deleted.status = FileStatus::Deleted;
        let mut binary = sized("logo.png", 0);
        binary.binary = true;
        let files = vec![
            sized("src/main.rs", 3),
            deleted,
            binary,
            sized(".env.local", 1),
            sized("keys/server.pem", 1),
            sized("Cargo.lock", 9),
            sized("web/dist/app.js", 9),
            sized("web/app.min.js", 9),
            sized("src/distance.rs", 2),
            sized("CHANGELOG.md", 3),
            sized("docs/guide.rst", 3),
        ];
        let (kept, excluded) = select(files);
        assert_eq!(kept.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["src/main.rs", "src/distance.rs"]);
        let reasons: Vec<(&str, Exclusion)> = excluded.iter().map(|e| (e.path.as_str(), e.reason)).collect();
        assert_eq!(
            reasons,
            [
                ("src/old.rs", Exclusion::Deleted),
                ("logo.png", Exclusion::Binary),
                (".env.local", Exclusion::Secret),
                ("keys/server.pem", Exclusion::Secret),
                ("Cargo.lock", Exclusion::Generated),
                ("web/dist/app.js", Exclusion::Generated),
                ("web/app.min.js", Exclusion::Generated),
                ("CHANGELOG.md", Exclusion::Documentation),
                ("docs/guide.rst", Exclusion::Documentation),
            ]
        );
    }

    /// The agent starts from the diffs, is told what else changed, and is
    /// told what it must not read.
    #[test]
    fn the_opening_message_carries_the_diffs_that_fit_and_names_the_rest() {
        let mut renamed = file("src/new.rs", &[]);
        renamed.status = FileStatus::Renamed { from: "src/old.rs".into() };
        renamed.patch = "@@ -1 +1 @@\n-a\n+b\n".into();
        let big = sized("src/big.rs", MESSAGE_DIFF_CHARS);
        let excluded = [Excluded { path: ".env".into(), reason: Exclusion::Secret }];
        let text = review_message(&[renamed, big], &excluded, None);
        assert!(text.contains("<changes>\n=== src/new.rs (renamed from src/old.rs) ===\n@@ -1 +1 @@\n-a\n+b\n</changes>"), "{text}");
        assert!(text.contains("diff not included for its size — read it with gitDiff or readFile where it matters:\nsrc/big.rs\n"), "{text}");
        assert!(text.contains("Left out of the review:\n.env — may hold secrets — do not read it\n"), "{text}");
        let plain = review_message(&[sized("a.rs", 10)], &[], None);
        assert!(!plain.contains("Also changed") && !plain.contains("Left out"), "{plain}");
        assert!(!plain.contains("[Reply in"), "{plain}");
    }

    /// The last thing the opening message says is the language to reply in.
    #[test]
    fn the_message_ends_with_the_chosen_reply_language() {
        let text = review_message(&[sized("a.rs", 10)], &[], Some("Russian"));
        assert!(text.ends_with("</changes>\n\n\n[Reply in Russian.]"), "{text:?}");
    }

    /// The room is shared: a file that does not fit is named, and a smaller
    /// one after it still goes in.
    #[test]
    fn the_diffs_that_fit_go_in_whatever_their_order() {
        let text = review_message(&[sized("a.rs", MESSAGE_DIFF_CHARS - 10), sized("b.rs", 20), sized("c.rs", 5)], &[], None);
        assert!(text.contains("=== a.rs") && text.contains("=== c.rs"), "{text}");
        assert!(!text.contains("=== b.rs") && text.contains("where it matters:\nb.rs\n"), "{text}");
    }

    fn desk() -> ReviewDesk {
        ReviewDesk::new(vec![file(
            "src/pay.rs",
            &[(10, false, "fn pay(amount: u32) {"), (11, true, "    let fee = amount / 0;"), (12, true, ""), (13, true, "    charge(fee);"), (14, false, "}")],
        )])
    }

    fn args(path: &str, code: &str) -> FindingArgs {
        FindingArgs { path: path.into(), existing_code: code.into(), title: "Divides by zero".into(), body: "Panics on every call.".into(), ..Default::default() }
    }

    #[test]
    fn a_finding_is_placed_where_its_code_is() {
        let desk = desk();
        let one = desk.note(&args("src/pay.rs", "let fee = amount / 0;")).unwrap();
        assert_eq!((one.start_line, one.end_line, one.category), (11, 11, Category::Bug));
        // Several lines, a copied `+`, a blank line and context around added code.
        let file = &desk.files()[0];
        assert_eq!(locate(file, "+    let fee = amount / 0;\n\n+    charge(fee);"), [(11, 13)]);
        assert_eq!(locate(file, "fn pay(amount: u32) {\n    let fee = amount / 0;"), [(10, 11)]);
    }

    #[test]
    fn code_the_change_did_not_add_is_refused() {
        let desk = desk();
        assert!(locate(&desk.files()[0], "fn pay(amount: u32) {").is_empty(), "context alone");
        assert!(locate(&desk.files()[0], "charge(fee);\n}\nextra").is_empty());
        assert_eq!(desk.note(&args("src/pay.rs", "let fee = amount / 1;")), Err(FindingError::NotInChange { path: "src/pay.rs".into() }));
    }

    #[test]
    fn lines_of_two_hunks_are_not_one_snippet() {
        let mut file = file("a.rs", &[(1, true, "a();"), (40, true, "b();")]);
        file.lines[1].hunk = 1;
        assert!(locate(&file, "a();\nb();").is_empty());
        assert_eq!(locate(&file, "b();"), [(40, 40)]);
    }

    /// A line the change added twice names neither: the finding would land on
    /// whichever came first. The agent is asked for more.
    #[test]
    fn code_that_matches_twice_is_refused_until_it_names_one() {
        let desk = ReviewDesk::new(vec![file(
            "a.rs",
            &[(3, true, "let x = read();"), (4, true, "return None;"), (9, true, "let y = parse();"), (10, true, "return None;")],
        )]);
        assert_eq!(desk.note(&args("a.rs", "return None;")), Err(FindingError::Ambiguous { path: "a.rs".into(), count: 2 }));
        let named = desk.note(&args("a.rs", "let y = parse();\nreturn None;")).unwrap();
        assert_eq!((named.start_line, named.end_line), (9, 10));
    }

    /// A file the change did not touch is refused with where such a problem goes instead.
    #[test]
    fn a_file_outside_the_change_or_an_empty_field_is_refused() {
        let desk = desk();
        let outside = desk.note(&args("src/other.rs", "x")).unwrap_err();
        assert_eq!(outside, FindingError::NotChanged { path: "src/other.rs".into() });
        assert!(outside.to_string().contains("goes in your answer"), "{outside}");
        assert_eq!(desk.note(&FindingArgs { title: " ".into(), ..args("src/pay.rs", "charge(fee);") }), Err(FindingError::Empty { what: "title" }));
        assert!(desk.note(&args("./src/pay.rs", "charge(fee);")).is_ok(), "a leading ./ is the same file");
    }

    #[test]
    fn the_same_problem_is_kept_once() {
        let one = desk();
        one.note(&args("src/pay.rs", "let fee = amount / 0;\n    charge(fee);")).unwrap();
        let again = one.note(&args("src/pay.rs", "charge(fee);"));
        assert!(matches!(again, Err(FindingError::Duplicate { line: 11, .. })), "{again:?}");
        let other_kind = one.note(&FindingArgs { category: Some(Category::Security), ..args("src/pay.rs", "charge(fee);") });
        assert!(other_kind.is_ok(), "another kind of problem on the same line");

        let lines_apart = desk();
        lines_apart.note(&args("src/pay.rs", "let fee = amount / 0;")).unwrap();
        assert!(lines_apart.note(&args("src/pay.rs", "charge(fee);")).is_ok(), "the same kind on other lines");
    }

    /// Asked once, at 80% of whichever ceiling comes first; never before.
    #[test]
    fn a_review_is_asked_to_wrap_up_once_with_a_fifth_of_either_ceiling_left() {
        let limit = Budget { rounds: 60, weight: 250 };
        let used = |rounds, weight| Budget { rounds, weight };
        assert_eq!(wrap_up(used(47, 199), limit, &[]), None);
        let by_rounds = wrap_up(used(48, 10), limit, &[]).expect("at 80% of the rounds");
        assert!(by_rounds.starts_with("[Review budget] This review has used 80% of its budget (48 of 60 rounds)"), "{by_rounds}");
        let by_weight = wrap_up(used(5, 225), limit, &[]).expect("at 80% of the weighted budget");
        assert!(by_weight.contains("used 90% of its budget (5 of 60 rounds)") && by_weight.contains("reportFinding"), "{by_weight}");
        let asked = [LlmMessage::user("go"), LlmMessage::user(by_rounds)];
        assert_eq!(wrap_up(used(55, 240), limit, &asked), None, "once a turn");
    }
}
