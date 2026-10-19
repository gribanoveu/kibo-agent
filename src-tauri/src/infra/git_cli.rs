//! The `git` program, for what libgit2 cannot do.
//!
//! One thing so far: blaming a file whose history has a commit by an author
//! with no name or no email (`Imported <>`, as some importers wrote them).
//! libgit2's blame rebuilds every author through `git_signature_new`, which
//! refuses those, so the whole blame fails; `git` itself reads them.

use std::path::Path;
use std::process::Command;

/// One run of lines from one commit, as `git blame --porcelain` gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliBlameHunk {
    pub commit: String,
    pub start_line: u32,
    pub line_count: u32,
    pub author: String,
}

/// Lines `start..=end` of `path` (relative to `workdir`) as of `HEAD`.
pub fn blame(workdir: &Path, path: &str, start: u32, end: u32) -> Result<Vec<CliBlameHunk>, String> {
    let mut command = Command::new("git");
    command
        .args(["blame", "--porcelain", "-L", &format!("{start},{end}"), "HEAD", "--", path])
        .current_dir(workdir);
    super::login_path::apply(&mut command);
    let output = command.output().map_err(|e| format!("could not run git: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(parse_porcelain(&String::from_utf8_lossy(&output.stdout)))
}

/// A group starts with `<sha> <orig> <final> <count>`; a commit's `author`
/// line follows its first group only, so authors are kept by commit.
fn parse_porcelain(text: &str) -> Vec<CliBlameHunk> {
    let mut hunks: Vec<CliBlameHunk> = Vec::new();
    let mut authors = std::collections::HashMap::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        if line.starts_with('\t') {
            continue;
        }
        let fields: Vec<&str> = line.split(' ').collect();
        let is_header = fields.first().is_some_and(|sha| sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit()));
        if is_header {
            current = Some(fields[0].to_string());
            if let (Some(start), Some(count)) = (fields.get(2), fields.get(3)) {
                if let (Ok(start_line), Ok(line_count)) = (start.parse(), count.parse()) {
                    hunks.push(CliBlameHunk { commit: fields[0].to_string(), start_line, line_count, author: String::new() });
                }
            }
        } else if let (Some(sha), Some(name)) = (&current, line.strip_prefix("author ")) {
            authors.insert(sha.clone(), name.to_string());
        }
    }
    for hunk in &mut hunks {
        hunk.author = authors.get(&hunk.commit).cloned().unwrap_or_default();
    }
    hunks
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "1111111111111111111111111111111111111111";
    const B: &str = "2222222222222222222222222222222222222222";

    /// A commit's details come once; a later group of its lines has only
    /// the header, and still gets its author.
    #[test]
    fn groups_become_hunks_with_their_commits_author() {
        let text = format!(
            "{A} 1 1 2\nauthor Ann\nauthor-mail <>\nsummary first\nfilename a.txt\n\tone\n{A} 2 2\n\ttwo\n\
             {B} 3 3 1\nauthor \nauthor-mail <b@x>\nsummary second\nfilename a.txt\n\tthree\n\
             {A} 4 4 1\n\tfour\n"
        );
        assert_eq!(
            parse_porcelain(&text),
            [
                CliBlameHunk { commit: A.into(), start_line: 1, line_count: 2, author: "Ann".into() },
                CliBlameHunk { commit: B.into(), start_line: 3, line_count: 1, author: "".into() },
                CliBlameHunk { commit: A.into(), start_line: 4, line_count: 1, author: "Ann".into() },
            ]
        );
    }
}
