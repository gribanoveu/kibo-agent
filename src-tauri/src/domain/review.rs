//! Reviewing the working tree's changes: which files, in which groups, and
//! what counts as a finding — every rule here is decided without a model.
//!
//! The shape follows alibaba/open-code-review (`docs/19-minimax-code-ideas.md`,
//! § 6): deterministic selection and grouping, a worker per group, and a
//! deterministic check of each finding. Unlike theirs, a worker reads nothing:
//! it gets its diffs with the code around them and answers in one reply
//! (`MAX_ROUNDS`). The check is the part that matters most. A model does not
//! give line numbers — it quotes the code it means, from the lines the change
//! added, and the quote is found in the diff here. A finding that quotes code
//! the change did not add, names no one place, or is on a file outside the
//! group is refused and counted, not placed somewhere wrong.

use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::domain::tools::ToolResult;
use crate::domain::turn::ChatEventPayload;

/// One changed file, as the review reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDiff {
    /// Root-relative, `/`-separated; the new path of a rename.
    pub path: String,
    pub status: FileStatus,
    pub binary: bool,
    /// The unified diff, as the worker reads it.
    pub patch: String,
    /// The lines on the new side of each hunk, in order.
    pub lines: Vec<NewLine>,
    /// The whole file is in `patch` — every line as context around the
    /// change — rather than only the code near it.
    pub whole: bool,
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

/// A file this short is shown whole: most of what a change breaks is further
/// than a few lines away but in the same file — a helper it calls, the
/// function beside it. Longer ones get the code near each change.
pub const WHOLE_FILE_LINES: usize = 600;

/// Past this a file's diff is not given to a worker: it would not fit one
/// request alone, and a diff that size is not read line by line anyway.
pub const MAX_PATCH_CHARS: usize = MAX_GROUP_CHARS;

/// Requests one group costs: one. The worker is given its diffs with the code
/// around each change and reports everything in one reply. A worker that read
/// on its own resent everything it had read with every request — a million
/// tokens for one file was the measured price.
pub const MAX_ROUNDS: u32 = 1;

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
    TooLarge,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Excluded {
    pub path: String,
    pub reason: Exclusion,
}

/// The files worth a worker's time, and every other one with its reason.
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
    } else if file.patch.chars().count() > MAX_PATCH_CHARS {
        Some(Exclusion::TooLarge)
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

/// What one request is given at most, in characters of diff: about 100k
/// tokens, with room left in a 260k window for the prompt and the reply. A
/// change that fits is one group — splitting it only hides one part from the
/// worker reading the other (a type from the code using it, measured), and
/// each group pays for the prompt again.
pub const MAX_GROUP_CHARS: usize = 300_000;

/// The files in groups a worker each, by the size of what the worker reads:
/// a change that fits one request whole; a larger one by folder, neighbouring
/// folders together while they fit — files that sit together usually change
/// together. A file too big for any group is one on its own.
pub fn group(mut files: Vec<FileDiff>) -> Vec<Vec<FileDiff>> {
    if files.is_empty() {
        return Vec::new();
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let size = |file: &FileDiff| file.patch.len();
    if files.iter().map(size).sum::<usize>() <= MAX_GROUP_CHARS {
        return vec![files];
    }
    let folder = |file: &FileDiff| file.path.rsplit_once('/').map_or(String::new(), |(dir, _)| dir.to_string());
    let mut folders: Vec<Vec<FileDiff>> = Vec::new();
    for file in files {
        match folders.last_mut() {
            Some(last) if folder(&last[0]) == folder(&file) => last.push(file),
            _ => folders.push(vec![file]),
        }
    }
    let mut groups: Vec<Vec<FileDiff>> = Vec::new();
    let fits = |group: &[FileDiff], more: &[FileDiff]| group.iter().chain(more).map(size).sum::<usize>() <= MAX_GROUP_CHARS;
    for folder in folders {
        if let Some(last) = groups.last_mut().filter(|last| fits(last, &folder)) {
            last.extend(folder);
            continue;
        }
        // A folder too big for one group goes in pieces.
        let mut current: Vec<FileDiff> = Vec::new();
        for file in folder {
            if !current.is_empty() && !fits(&current, std::slice::from_ref(&file)) {
                groups.push(std::mem::take(&mut current));
            }
            current.push(file);
        }
        groups.push(current);
    }
    groups
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

/// What `finishReview` takes: a group's summary, the answer when nothing is
/// found and the context when something is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SummaryArgs {
    /// What the change does in these files.
    pub summary: String,
    /// The risks looked at and cleared.
    #[serde(default)]
    pub checked: Vec<String>,
    /// Doubts the diff cannot settle — they hang on code not shown.
    #[serde(default)]
    pub worth_a_look: Vec<Concern>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Concern {
    pub path: String,
    pub note: String,
}

/// One group's summary as the user reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupSummary {
    pub files: Vec<String>,
    pub summary: String,
    pub checked: Vec<String>,
    pub worth_a_look: Vec<Concern>,
}

/// A finding as the user reads it, placed on the new side of the file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    /// 1-based, in the order the report lists them; 0 until then.
    pub id: u32,
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub severity: Severity,
    pub category: Category,
    pub title: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
}

/// Why a finding was refused, worded for the worker that has to fix it.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum FindingError {
    #[error("{path} is not one of the files you are reviewing ({files}) — report only on those")]
    OutsideGroup { path: String, files: String },
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

/// One worker's files and what it has found so far — what `reportFinding`
/// checks a finding against and adds it to.
#[derive(Debug, Default)]
pub struct ReviewDesk {
    files: Vec<FileDiff>,
    findings: Mutex<Vec<Finding>>,
    summary: Mutex<Option<SummaryArgs>>,
}

impl ReviewDesk {
    pub fn new(files: Vec<FileDiff>) -> Self {
        Self { files, findings: Mutex::default(), summary: Mutex::default() }
    }

    pub fn files(&self) -> &[FileDiff] {
        &self.files
    }

    /// Checks a finding and keeps it: its file is one of this desk's, its
    /// code is lines the change added, and it is not a second report of one
    /// already kept — the same file, overlapping lines, the same category.
    pub fn note(&self, args: &FindingArgs) -> Result<Finding, FindingError> {
        let path = args.path.trim().trim_start_matches("./");
        for (what, value) in [("path", path), ("title", args.title.trim()), ("body", args.body.trim()), ("existingCode", args.existing_code.trim())] {
            if value.is_empty() {
                return Err(FindingError::Empty { what });
            }
        }
        let file = self.files.iter().find(|file| file.path == path).ok_or_else(|| FindingError::OutsideGroup {
            path: path.to_string(),
            files: self.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>().join(", "),
        })?;
        let (start_line, end_line) = match locate(file, &args.existing_code)[..] {
            [one] => one,
            [] => return Err(FindingError::NotInChange { path: path.to_string() }),
            ref many => return Err(FindingError::Ambiguous { path: path.to_string(), count: many.len() }),
        };
        let finding = Finding {
            id: 0,
            path: path.to_string(),
            start_line,
            end_line,
            severity: args.severity.unwrap_or(Severity::Medium),
            category: args.category.unwrap_or(Category::Bug),
            title: args.title.trim().to_string(),
            body: args.body.trim().to_string(),
            suggestion: args.suggestion.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(String::from),
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

    pub fn take_findings(&self) -> Vec<Finding> {
        std::mem::take(&mut *self.findings.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Keeps the group's summary, trimmed, blank items dropped; a second
    /// replaces the first.
    pub fn summarize(&self, args: &SummaryArgs) -> Result<(), FindingError> {
        let summary = args.summary.trim();
        if summary.is_empty() {
            return Err(FindingError::Empty { what: "summary" });
        }
        let kept = SummaryArgs {
            summary: summary.to_string(),
            checked: args.checked.iter().map(|c| c.trim()).filter(|c| !c.is_empty()).map(String::from).collect(),
            worth_a_look: args
                .worth_a_look
                .iter()
                .map(|c| Concern { path: c.path.trim().trim_start_matches("./").to_string(), note: c.note.trim().to_string() })
                .filter(|c| !c.note.is_empty())
                .collect(),
        };
        *self.summary.lock().unwrap_or_else(|e| e.into_inner()) = Some(kept);
        Ok(())
    }

    pub fn take_summary(&self) -> Option<SummaryArgs> {
        self.summary.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}

/// A group whose worker did not finish, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FailedGroup {
    pub files: Vec<String>,
    pub error: String,
}

/// Where the review sends each group's progress.
pub type ReviewSink = std::sync::Arc<dyn Fn(GroupProgress) + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GroupState {
    Waiting,
    Working,
    Done,
    Failed,
}

/// Where one group's worker has got to, as the review card draws it. Sent
/// whole on every change, so the window replaces rather than adds up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupProgress {
    /// 0-based, in the order the groups were made.
    pub group: u32,
    pub total: u32,
    pub files: Vec<String>,
    pub state: GroupState,
    /// Findings the check refused: code not in the change, or in more than
    /// one place.
    pub failed_calls: u32,
    pub findings: u32,
    /// What the provider counted, once it has: the request sent, and the
    /// reply — thinking included, which can be most of it.
    pub input: u64,
    pub output: u64,
    /// The reply stopped at the provider's length limit: what came after —
    /// the summary, usually — never arrived.
    #[serde(default)]
    pub truncated: bool,
    /// What its request weighs by our own count, known before it is sent —
    /// what the card shows until `tokens` arrives.
    pub estimate: u64,
    /// What is slowing it right now — a retry, a failed call, a loop note.
    /// Gone once a request is answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Why it failed, once it has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl GroupProgress {
    pub fn waiting(group: usize, total: usize, files: Vec<String>) -> Self {
        Self {
            group: group as u32,
            total: total as u32,
            files,
            state: GroupState::Waiting,
            failed_calls: 0,
            findings: 0,
            input: 0,
            output: 0,
            truncated: false,
            estimate: 0,
            note: None,
            error: None,
        }
    }

    /// Folds in one event of the worker's turn. `false` when nothing the card
    /// shows changed — a streamed word, say — so it is not sent.
    pub fn apply(&mut self, event: &ChatEventPayload) -> bool {
        match event {
            ChatEventPayload::ToolResult(result) => {
                if let Some(error) = &result.error {
                    self.failed_calls += 1;
                    let first: String = error.lines().next().unwrap_or_default().chars().take(160).collect();
                    self.note = Some(format!("A finding was not placed: {first}"));
                } else if matches!(result.result, Some(ToolResult::FindingNoted { .. })) {
                    self.findings += 1;
                }
            }
            ChatEventPayload::Retrying { attempt, max_attempts, delay_seconds } => {
                self.note = Some(format!("No answer from the provider — retry {attempt} of {max_attempts} in {delay_seconds}s"));
            }
            ChatEventPayload::LoopReminded { tool, .. } => {
                self.note = Some(format!("Repeating {tool} — told to change course"));
            }
            ChatEventPayload::ContextUsage(usage) => {
                self.input += u64::from(usage.prompt_tokens);
                self.output += u64::from(usage.completion_tokens);
                self.note = None;
            }
            ChatEventPayload::RoundCompleted { truncated: true, .. } => self.truncated = true,
            _ => return false,
        }
        true
    }
}

/// The review as the chat shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewReport {
    pub reviewed: Vec<String>,
    pub excluded: Vec<Excluded>,
    pub findings: Vec<Finding>,
    pub failed: Vec<FailedGroup>,
    /// A summary a group, in group order, for the groups that gave one.
    pub summaries: Vec<GroupSummary>,
}

/// What `/review` hands the window: the report to draw, and the same as text
/// for the conversation, so the next message can refer to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewDone {
    pub report: ReviewReport,
    pub for_model: String,
}

impl ReviewDone {
    pub fn new(report: ReviewReport) -> Self {
        let for_model = report_for_model(&report);
        Self { report, for_model }
    }
}

/// Most severe first, then by file and line, numbered from 1.
pub fn rank(mut findings: Vec<Finding>) -> Vec<Finding> {
    findings.sort_by(|a, b| (a.severity, &a.path, a.start_line).cmp(&(b.severity, &b.path, b.start_line)));
    for (i, finding) in findings.iter_mut().enumerate() {
        finding.id = i as u32 + 1;
    }
    findings
}

/// What a worker is told: its files' diffs, and the rest of the change by
/// name, to read for context and not to report on.
pub fn worker_prompt(group: &[FileDiff], others: &[String]) -> String {
    let mut out = String::from("Review the change to these files — only these:\n\n<review_files>\n");
    for file in group {
        let status = match &file.status {
            FileStatus::Added => "new file".to_string(),
            FileStatus::Modified => "modified".to_string(),
            FileStatus::Deleted => "deleted".to_string(),
            FileStatus::Renamed { from } => format!("renamed from {from}"),
        };
        let shown = if file.whole { ", whole file" } else { "" };
        out.push_str(&format!("=== {} ({status}{shown}) ===\n{}\n", file.path, file.patch.trim_end()));
    }
    out.push_str("</review_files>\n");
    if !others.is_empty() {
        out.push_str(&format!(
            "\nAlso changed, reviewed separately — not shown here, do not report on them:\n{}\n",
            others.join("\n")
        ));
    }
    out.push_str("\nThis is your one reply: report every problem now, each with its own reportFinding call, then end with the one finishReview call — required, also when you found nothing.");
    out
}

/// The report as the model reads it in the conversation afterwards, so the
/// user can say "fix 2 and 4".
pub fn report_for_model(report: &ReviewReport) -> String {
    let mut out = format!("[Code review of the working tree: {} files reviewed", report.reviewed.len());
    if !report.excluded.is_empty() {
        out.push_str(&format!(", {} left out", report.excluded.len()));
    }
    out.push_str("]\n");
    if report.findings.is_empty() {
        out.push_str("No findings.\n");
    }
    for f in &report.findings {
        let lines = if f.start_line == f.end_line { f.start_line.to_string() } else { format!("{}-{}", f.start_line, f.end_line) };
        out.push_str(&format!("{}. [{:?}] {}:{lines} — {}\n   {}\n", f.id, f.severity, f.path, f.title, f.body));
        if let Some(suggestion) = &f.suggestion {
            out.push_str(&format!("   Suggested: {suggestion}\n"));
        }
    }
    for failed in &report.failed {
        out.push_str(&format!("Not finished ({}): {}\n", failed.error, failed.files.join(", ")));
    }
    for group in &report.summaries {
        out.push_str(&format!("\n{}:\n{}\n", group.files.join(", "), group.summary));
        if !group.checked.is_empty() {
            out.push_str(&format!("Checked: {}\n", group.checked.join("; ")));
        }
        for concern in &group.worth_a_look {
            out.push_str(&format!("Worth a look: {} — {}\n", concern.path, concern.note));
        }
    }
    out
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
            whole: false,
        }
    }

    /// A file whose diff is `chars` long — what a group is measured in.
    fn sized(path: &str, chars: usize) -> FileDiff {
        FileDiff { patch: "x".repeat(chars), ..file(path, &[(1, true, "x")]) }
    }

    /// A tenth of a group.
    const U: usize = MAX_GROUP_CHARS / 10;

    #[test]
    fn what_is_left_out_and_why() {
        let mut deleted = sized("src/old.rs", 0);
        deleted.status = FileStatus::Deleted;
        let mut binary = sized("logo.png", 0);
        binary.binary = true;
        let mut huge = sized("src/big.rs", 1);
        huge.patch = "x".repeat(MAX_PATCH_CHARS + 1);
        let files = vec![
            sized("src/main.rs", 3),
            deleted,
            binary,
            huge,
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
                ("src/big.rs", Exclusion::TooLarge),
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

    #[test]
    fn a_change_that_fits_one_request_is_one_group() {
        // Twelve folders, each its own group if grouped by folder.
        let small = group((0..12).map(|i| sized(&format!("d{i:02}/f.rs"), U / 2)).collect());
        assert_eq!(small.len(), 1, "six tenths of a group");
        let whole = group((0..10).map(|i| sized(&format!("d{i:02}/f.rs"), U)).collect());
        assert_eq!(whole.len(), 1, "exactly a group");
        assert!(group(Vec::new()).is_empty());
        // Few files are no reason to keep together what does not fit.
        let few = group(vec![sized("a/x.rs", 6 * U), sized("b/y.rs", 6 * U), sized("c/z.rs", 6 * U)]);
        assert_eq!(few.len(), 3, "three files, each over half a group");
    }

    #[test]
    fn a_larger_change_goes_by_folder_neighbours_together_while_they_fit() {
        let groups = group(vec![
            sized("src/a/one.rs", 4 * U),
            sized("src/a/two.rs", 4 * U),
            sized("src/b/three.rs", 4 * U),
            sized("src/c/four.rs", 2 * U),
            sized("src/c/five.rs", 2 * U),
        ]);
        let paths: Vec<Vec<&str>> = groups.iter().map(|g| g.iter().map(|f| f.path.as_str()).collect()).collect();
        assert_eq!(paths, [vec!["src/a/one.rs", "src/a/two.rs"], vec!["src/b/three.rs", "src/c/five.rs", "src/c/four.rs"]]);
    }

    #[test]
    fn a_folder_too_big_for_a_group_is_split() {
        let groups = group((0..30).map(|i| sized(&format!("src/f{i:02}.rs"), U / 2)).collect());
        assert_eq!(groups.iter().map(Vec::len).collect::<Vec<_>>(), [20, 10]);
        let big = group(vec![sized("a/x.rs", 11 * U), sized("a/y.rs", U / 10), sized("a/z.rs", U / 10), sized("a/w.rs", U / 10)]);
        // In path order w, x, y, z: x is over a group alone and parts the others.
        assert_eq!(big.iter().map(Vec::len).collect::<Vec<_>>(), [1, 1, 2]);
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
        assert_eq!((one.start_line, one.end_line, one.severity, one.category), (11, 11, Severity::Medium, Category::Bug));
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
    /// whichever came first. The worker is asked for more.
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

    #[test]
    fn a_file_outside_the_group_or_an_empty_field_is_refused() {
        let desk = desk();
        assert!(matches!(desk.note(&args("src/other.rs", "x")), Err(FindingError::OutsideGroup { files, .. }) if files == "src/pay.rs"));
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
        assert_eq!(one.take_findings().len(), 2);

        let lines_apart = desk();
        lines_apart.note(&args("src/pay.rs", "let fee = amount / 0;")).unwrap();
        assert!(lines_apart.note(&args("src/pay.rs", "charge(fee);")).is_ok(), "the same kind on other lines");
    }

    /// The summary is kept tidy, needs its one sentence, and the last one stands.
    #[test]
    fn a_summary_is_kept_trimmed_and_needs_saying_something() {
        let desk = desk();
        assert_eq!(desk.summarize(&SummaryArgs { summary: "  ".into(), ..Default::default() }), Err(FindingError::Empty { what: "summary" }));
        desk.summarize(&SummaryArgs { summary: "first".into(), ..Default::default() }).unwrap();
        desk.summarize(&SummaryArgs {
            summary: " Adds a fee. ".into(),
            checked: vec![" rounding ".into(), " ".into()],
            worth_a_look: vec![Concern { path: "./src/pay.rs".into(), note: " zero amounts ".into() }, Concern { path: "x".into(), note: "".into() }],
        })
        .unwrap();
        assert_eq!(
            desk.take_summary(),
            Some(SummaryArgs {
                summary: "Adds a fee.".into(),
                checked: vec!["rounding".into()],
                worth_a_look: vec![Concern { path: "src/pay.rs".into(), note: "zero amounts".into() }],
            })
        );
        assert_eq!(desk.take_summary(), None);
    }

    #[test]
    fn findings_are_ranked_by_severity_then_place_and_numbered() {
        let at = |path: &str, line: u32, severity: Severity| Finding {
            id: 0,
            path: path.into(),
            start_line: line,
            end_line: line,
            severity,
            category: Category::Bug,
            title: String::new(),
            body: String::new(),
            suggestion: None,
        };
        let ranked = rank(vec![at("b.rs", 1, Severity::Low), at("b.rs", 9, Severity::Critical), at("a.rs", 5, Severity::Low), at("a.rs", 2, Severity::Low)]);
        let order: Vec<(u32, &str, u32)> = ranked.iter().map(|f| (f.id, f.path.as_str(), f.start_line)).collect();
        assert_eq!(order, [(1, "b.rs", 9), (2, "a.rs", 2), (3, "a.rs", 5), (4, "b.rs", 1)]);
    }

    #[test]
    fn the_worker_is_given_its_diffs_and_the_rest_by_name() {
        let mut renamed = file("src/new.rs", &[]);
        renamed.status = FileStatus::Renamed { from: "src/old.rs".into() };
        renamed.patch = "@@ -1 +1 @@\n-a\n+b\n".into();
        let whole = FileDiff { whole: true, ..file("src/small.rs", &[]) };
        assert!(worker_prompt(&[whole], &[]).contains("=== src/small.rs (modified, whole file) ==="));
        let prompt = worker_prompt(&[renamed], &["web/app.ts".into()]);
        assert!(prompt.contains("=== src/new.rs (renamed from src/old.rs) ===\n@@ -1 +1 @@\n-a\n+b\n</review_files>"), "{prompt}");
        assert!(prompt.contains("do not report on them:\nweb/app.ts"), "{prompt}");
        assert!(prompt.contains("This is your one reply") && prompt.contains("finishReview"), "{prompt}");
        assert!(!worker_prompt(&[], &[]).contains("Also changed"));
    }

    #[test]
    fn the_model_reads_the_report_numbered() {
        let report = ReviewReport {
            reviewed: vec!["a.rs".into()],
            excluded: vec![Excluded { path: "Cargo.lock".into(), reason: Exclusion::Generated }],
            findings: rank(vec![Finding {
                id: 0,
                path: "a.rs".into(),
                start_line: 3,
                end_line: 4,
                severity: Severity::High,
                category: Category::Bug,
                title: "Off by one".into(),
                body: "Skips the last item.".into(),
                suggestion: Some("use ..=".into()),
            }]),
            failed: vec![FailedGroup { files: vec!["b.rs".into()], error: "provider said 429".into() }],
            summaries: vec![GroupSummary {
                files: vec!["a.rs".into()],
                summary: "Adds paging.".into(),
                checked: vec!["empty pages".into(), "the last page".into()],
                worth_a_look: vec![Concern { path: "src/api.rs".into(), note: "callers may pass 0".into() }],
            }],
        };
        let text = report_for_model(&report);
        assert!(text.starts_with("[Code review of the working tree: 1 files reviewed, 1 left out]\n"), "{text}");
        assert!(text.contains("1. [High] a.rs:3-4 — Off by one\n   Skips the last item.\n   Suggested: use ..=\n"), "{text}");
        assert!(text.contains("Not finished (provider said 429): b.rs"), "{text}");
        assert!(text.contains("\na.rs:\nAdds paging.\nChecked: empty pages; the last page\nWorth a look: src/api.rs — callers may pass 0\n"), "{text}");
    }

    /// The card's line per group, folded from the worker's own turn events.
    #[test]
    fn a_group_s_progress_follows_its_worker() {
        use crate::domain::llm::ChatUsage;
        use crate::domain::turn::ToolResultEvent;
        let mut group = GroupProgress::waiting(1, 3, vec!["a.rs".into()]);
        let result = |result: Option<ToolResult>, error: Option<&str>| {
            ChatEventPayload::ToolResult(ToolResultEvent { id: "c".into(), result, error: error.map(String::from), changes: Vec::new() })
        };
        let usage = |prompt, completion| ChatEventPayload::ContextUsage(ChatUsage { prompt_tokens: prompt, completion_tokens: completion, total_tokens: 0, cached_tokens: 0 });

        assert!(!group.apply(&ChatEventPayload::Delta { delta: "hm".into() }), "a streamed word changes nothing shown");
        group.apply(&ChatEventPayload::Retrying { attempt: 2, max_attempts: 5, delay_seconds: 30 });
        assert_eq!(group.note.as_deref(), Some("No answer from the provider — retry 2 of 5 in 30s"));
        group.apply(&usage(1_000, 200));
        assert_eq!(group.note, None, "answered: the retry is over");
        group.apply(&result(None, Some("existingCode matches 2 places\nmore")));
        assert_eq!((group.failed_calls, group.note.as_deref()), (1, Some("A finding was not placed: existingCode matches 2 places")));
        group.apply(&result(Some(ToolResult::FindingNoted { path: "a.rs".into(), start_line: 1, end_line: 1 }), None));
        group.apply(&usage(3_000, 100));
        assert_eq!((group.findings, group.input, group.output), (1, 4_000, 300), "in and out add up apart");
        assert!(!group.apply(&ChatEventPayload::RoundCompleted { text: "ok".into(), reasoning: String::new(), truncated: false }));
        assert!(group.apply(&ChatEventPayload::RoundCompleted { text: "cut".into(), reasoning: String::new(), truncated: true }));
        assert!(group.truncated, "a reply cut at the length limit is said");
    }
}
