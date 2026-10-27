//! An `explore` helper's answer, checked before the calling agent reads it.
//!
//! The agent gets the answer and nothing of how it was found, so a wrong
//! `path:line` would pass as a fact. Each one cited is checked against the
//! file — that it exists and has the line — and what fails is said under the
//! answer, with the files the helper read, for the agent to open itself.
//!
//! The answer is prose in the shape Explore mode asks for; nothing here
//! rejects it. One that cites no line is passed on with a note that nothing
//! in it could be checked.
//!
//! ponytail: a name with an extension and no folder that is not a file —
//! `db.internal:5432` — is taken for a host and not counted, so a made-up
//! file at the root goes unflagged. Names without an extension (`Makefile`)
//! and `#L12` links are not read as citations at all.

use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;

/// `path:12` or `path:12-20`: a path with an extension, relative or absolute,
/// with `/` or `\` between folders, not glued to a word, a scheme or another
/// path before it. The dash of a range touches both numbers: `a.rs:7 - 3
/// callers` is line 7.
static CITATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|[^\w./\\:-])(/?(?:[\w.-]+[/\\])*[\w.-]*\.\w+):(\d+)(?:[-–](\d+))?").expect("a valid pattern")
});

/// At most this many failed citations are named; the count covers the rest.
const MAX_NAMED: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Citation {
    pub path: String,
    pub start: usize,
    pub end: usize,
}

/// Every `path:line` in `text`, once each, in the order first cited.
pub fn citations(text: &str) -> Vec<Citation> {
    let mut found: Vec<Citation> = Vec::new();
    for caps in CITATION.captures_iter(text) {
        let start = caps[2].parse().unwrap_or(0);
        let end = caps.get(3).and_then(|m| m.as_str().parse().ok()).unwrap_or(start);
        let citation = Citation { path: caps[1].replace('\\', "/"), start, end };
        if !found.contains(&citation) {
            found.push(citation);
        }
    }
    found
}

/// `text` with what the app checked under it. `lines_of` is a file's line
/// count, `None` where there is no such file to read — asked once a file,
/// however many of its lines are cited; `read` is what the helper opened.
pub fn checked(text: &str, read: &[String], lines_of: impl Fn(&str) -> Option<usize>) -> String {
    let mut files: HashMap<String, Option<usize>> = HashMap::new();
    let cited: Vec<(Citation, Option<usize>)> = citations(text)
        .into_iter()
        .map(|c| {
            let lines = *files.entry(c.path.clone()).or_insert_with(|| lines_of(&c.path));
            (c, lines)
        })
        .filter(|(c, lines)| lines.is_some() || c.path.contains('/'))
        .collect();
    let broken: Vec<String> = cited
        .iter()
        .filter_map(|(c, lines)| {
            let at = if c.start == c.end { format!("{}:{}", c.path, c.start) } else { format!("{}:{}-{}", c.path, c.start, c.end) };
            let why = match *lines {
                None => "no such file".to_string(),
                Some(_) if c.start == 0 || c.end < c.start => "not a line range".to_string(),
                Some(lines) if c.end > lines => format!("the file has {lines} lines"),
                Some(_) => return None,
            };
            Some(format!("`{at}` ({why})"))
        })
        .collect();
    let check = if cited.is_empty() {
        "[Checked by the app: the answer cites no path:line, so none of it could be checked against the files. Treat it as a lead, not a fact.]".to_string()
    } else if broken.is_empty() {
        format!("[Checked by the app: all {} cited places exist.]", cited.len())
    } else {
        let more = broken.len().saturating_sub(MAX_NAMED);
        let named = broken.iter().take(MAX_NAMED).cloned().collect::<Vec<_>>().join(", ");
        let more = if more > 0 { format!(" and {more} more") } else { String::new() };
        format!(
            "[Checked by the app: {} of {} cited places are not there — {named}{more}. Read those yourself before relying on what the answer says about them.]",
            broken.len(),
            cited.len()
        )
    };
    let mut out = format!("{}\n\n{check}", text.trim_end());
    if !read.is_empty() {
        out.push_str(&format!(
            "\n[The helper read: {}. None of it is in this conversation; read again what you need.]",
            read.join(", ")
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(path: &str, start: usize, end: usize) -> Citation {
        Citation { path: path.into(), start, end }
    }

    #[test]
    fn a_citation_is_a_path_and_a_line_or_a_range() {
        let text = "The token is made in `src/auth/token.rs:42`, refreshed at src/auth/refresh.ts:10-25 and \
                    src/auth/refresh.ts:30–31; see also (lib.rs:7). Again: src/auth/token.rs:42.";
        assert_eq!(
            citations(text),
            vec![at("src/auth/token.rs", 42, 42), at("src/auth/refresh.ts", 10, 25), at("src/auth/refresh.ts", 30, 31), at("lib.rs", 7, 7)]
        );
    }

    /// A port, a time or a URL is not a place in a file.
    #[test]
    fn what_only_looks_like_one_is_not() {
        assert_eq!(citations("at 12:30, port :8080, localhost:8080, http://host.io:80/x"), Vec::new());
        assert_eq!(citations("in app.test.ts:4"), vec![at("app.test.ts", 4, 4)]);
        // A dash with space around it, or the next bullet, is not a range.
        assert_eq!(citations("- a.rs:40\n- 12 more callers, b.rs:7 - 3 use it"), vec![at("a.rs", 40, 40), at("b.rs", 7, 7)]);
    }

    #[test]
    fn absolute_and_windows_paths_are_citations_too() {
        assert_eq!(
            citations("see /repo/src/a.rs:12 and src\\auth\\token.rs:42-44"),
            vec![at("/repo/src/a.rs", 12, 12), at("src/auth/token.rs", 42, 44)]
        );
    }

    /// `db.internal:5432` reads like a file at the root; one that is not
    /// there is taken for a host, while a missing path in a folder is named.
    #[test]
    fn a_host_is_not_a_missing_file() {
        let out = checked("Postgres on db.internal:5432, set in conf/db.toml:3", &[], |_| None);
        assert!(out.contains("1 of 1 cited places") && out.contains("`conf/db.toml:3` (no such file)"), "{out}");
        assert!(!out.contains("`db.internal"), "{out}");
    }

    #[test]
    fn each_file_is_looked_up_once() {
        let asked = std::cell::Cell::new(0);
        let out = checked("a.rs:1, a.rs:2, a.rs:3-4", &[], |_| {
            asked.set(asked.get() + 1);
            Some(10)
        });
        assert_eq!(asked.get(), 1);
        assert!(out.contains("all 3 cited places exist"), "{out}");
    }

    #[test]
    fn places_that_are_not_there_are_named() {
        let lines = |path: &str| match path {
            "a.rs" => Some(120),
            _ => None,
        };
        let out = checked("X is in a.rs:7 and a.rs:118-130, Y in lib/b.rs:3, Z in a.rs:0.\n", &[], lines);
        assert!(out.starts_with("X is in a.rs:7"), "{out}");
        assert!(
            out.ends_with(
                "[Checked by the app: 3 of 4 cited places are not there — `a.rs:118-130` (the file has 120 lines), `lib/b.rs:3` (no such file), `a.rs:0` (not a line range). Read those yourself before relying on what the answer says about them.]"
            ),
            "{out}"
        );
    }

    #[test]
    fn an_answer_whose_places_all_exist_says_so_and_what_was_read() {
        let out = checked("X is in a.rs:120.", &["a.rs".into(), "b.rs".into()], |_| Some(120));
        assert_eq!(
            out,
            "X is in a.rs:120.\n\n[Checked by the app: all 1 cited places exist.]\n\
             [The helper read: a.rs, b.rs. None of it is in this conversation; read again what you need.]"
        );
    }

    #[test]
    fn an_answer_that_cites_nothing_is_passed_on_as_a_lead() {
        let out = checked("X is somewhere in the auth code.", &[], |_| panic!("nothing to look up"));
        assert!(out.starts_with("X is somewhere in the auth code.\n\n"), "{out}");
        assert!(out.contains("cites no path:line"), "{out}");
    }

    #[test]
    fn only_the_first_failures_are_named() {
        let text = (1..=12).map(|n| format!("src/f{n}.rs:1")).collect::<Vec<_>>().join(" ");
        let out = checked(&text, &[], |_| None);
        assert!(out.contains("12 of 12") && out.contains("`src/f10.rs:1`") && !out.contains("`src/f11.rs:1`"), "{out}");
        assert!(out.contains("and 2 more"), "{out}");
    }
}
