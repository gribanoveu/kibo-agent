//! Putting files back as they were before a message: what the agent changed,
//! and the plan that undoes it.
//!
//! Every write the agent makes records, per file, the content before and the
//! content after — as hashes, the bytes kept in `infra::file_history`. A
//! rewind takes the changes made after a message, oldest first, and for each
//! file aims at the state before the first of them. It never writes over what
//! it did not write: a file whose chain of changes does not join up was
//! touched by someone else in between, and a file that is not now what the
//! agent left is the user's since. Both are left as they are and said so.
//!
//! What a command changed is not here: a shell line can write anywhere, and
//! guessing its targets is how a rewind comes to delete the user's work. The
//! preview says commands ran instead.
//!
//! The idea and the rules follow MiniMax Code's diff rewind
//! (`docs/19-minimax-code-ideas.md`, § 3).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A file's content at one moment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum FileState {
    Absent,
    /// Its content is kept under this hash.
    Stored { hash: String },
    /// It existed but was not kept — too big, or past the call's budget. A
    /// file whose way back needs this cannot be put back.
    Unstored,
}

/// What one call did to one file. `path` is root-relative, `/`-separated —
/// the spelling every tool reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    pub path: String,
    pub before: FileState,
    pub after: FileState,
}

/// What the agent did to a file over the part being undone — what the
/// preview names, not what the rewind will do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Action {
    Created,
    Modified,
    Deleted,
}

/// Why a file is left as it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Skip {
    /// Its earlier content was not kept.
    NotKept,
    /// Kept once, but the copy has since been cleared away.
    Expired,
    /// Something other than the agent changed it between two of its writes.
    ChangedBetween,
    /// It is not what the agent left: changed since, by the user or a command.
    ChangedSince,
    /// Not a path inside the folder.
    UnsafePath,
    /// Ready, but the write itself failed.
    WriteFailed,
}

/// One file's part of a rewind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileRewind {
    pub path: String,
    pub action: Action,
    /// What it should be now for the rewind to touch it.
    pub expected: FileState,
    /// What it becomes.
    pub target: FileState,
    /// `None` when it is ready.
    pub skip: Option<Skip>,
}

/// The rewind of `changes`, oldest first, one entry per file in path order.
/// `current` is what each file is now (`Unstored` where it could not be read)
/// and `kept` says whether content under a hash can still be had.
///
/// A file that ends as it began — created and deleted again — has nothing to
/// undo and is not listed.
pub fn plan(
    changes: &[FileChange],
    current: impl Fn(&str) -> FileState,
    kept: impl Fn(&str) -> bool,
) -> Vec<FileRewind> {
    let mut by_path: BTreeMap<&str, Vec<&FileChange>> = BTreeMap::new();
    for change in changes {
        by_path.entry(&change.path).or_default().push(change);
    }
    by_path
        .into_iter()
        .filter_map(|(path, chain)| {
            let (first, last) = (chain[0], chain[chain.len() - 1]);
            let (target, expected) = (first.before.clone(), last.after.clone());
            if target == expected && target != FileState::Unstored {
                return None;
            }
            let action = match (&target, &expected) {
                (FileState::Absent, _) => Action::Created,
                (_, FileState::Absent) => Action::Deleted,
                _ => Action::Modified,
            };
            let joined = chain.windows(2).all(|pair| pair[0].after == pair[1].before && pair[0].after != FileState::Unstored);
            let skip = if !safe(path) {
                Some(Skip::UnsafePath)
            } else if target == FileState::Unstored || expected == FileState::Unstored {
                Some(Skip::NotKept)
            } else if !joined {
                Some(Skip::ChangedBetween)
            } else if current(path) != expected {
                Some(Skip::ChangedSince)
            } else if matches!(&target, FileState::Stored { hash } if !kept(hash)) {
                Some(Skip::Expired)
            } else {
                None
            };
            Some(FileRewind { path: path.to_string(), action, expected, target, skip })
        })
        .collect()
}

/// Relative, and never above where it starts: no root, no `..`, no empty part.
fn safe(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && !path.contains(':')
        && path.split('/').all(|part| !part.is_empty() && part != "." && part != "..")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored(hash: &str) -> FileState {
        FileState::Stored { hash: hash.to_string() }
    }

    fn change(path: &str, before: FileState, after: FileState) -> FileChange {
        FileChange { path: path.to_string(), before, after }
    }

    /// Plans with the files as the agent left them and every copy kept.
    fn untouched(changes: &[FileChange]) -> Vec<FileRewind> {
        let last: BTreeMap<String, FileState> = changes.iter().map(|c| (c.path.clone(), c.after.clone())).collect();
        plan(changes, |path| last[path].clone(), |_| true)
    }

    #[test]
    fn each_file_goes_back_to_before_its_first_change() {
        let rewound = untouched(&[
            change("a.rs", stored("a0"), stored("a1")),
            change("new.rs", FileState::Absent, stored("n1")),
            change("a.rs", stored("a1"), stored("a2")),
            change("gone.rs", stored("g0"), FileState::Absent),
        ]);
        let summary: Vec<(&str, Action, &FileState, &FileState)> =
            rewound.iter().map(|f| (f.path.as_str(), f.action, &f.target, &f.expected)).collect();
        assert_eq!(
            summary,
            [
                ("a.rs", Action::Modified, &stored("a0"), &stored("a2")),
                ("gone.rs", Action::Deleted, &stored("g0"), &FileState::Absent),
                ("new.rs", Action::Created, &FileState::Absent, &stored("n1")),
            ]
        );
        assert!(rewound.iter().all(|f| f.skip.is_none()));
    }

    #[test]
    fn a_file_that_ends_as_it_began_is_not_listed() {
        let rewound = untouched(&[
            change("tmp.txt", FileState::Absent, stored("t")),
            change("tmp.txt", stored("t"), FileState::Absent),
            change("same.rs", stored("s0"), stored("s1")),
            change("same.rs", stored("s1"), stored("s0")),
        ]);
        assert!(rewound.is_empty(), "{rewound:?}");
    }

    /// The user's edit after the agent's is never written over.
    #[test]
    fn a_file_changed_since_is_left_alone() {
        let changes = [change("a.rs", stored("a0"), stored("a1"))];
        let rewound = plan(&changes, |_| stored("mine"), |_| true);
        assert_eq!(rewound[0].skip, Some(Skip::ChangedSince));
        let deleted = plan(&changes, |_| FileState::Absent, |_| true);
        assert_eq!(deleted[0].skip, Some(Skip::ChangedSince));
    }

    #[test]
    fn a_chain_that_does_not_join_up_was_touched_in_between() {
        let rewound = untouched(&[change("a.rs", stored("a0"), stored("a1")), change("a.rs", stored("x"), stored("a2"))]);
        assert_eq!(rewound[0].skip, Some(Skip::ChangedBetween));
    }

    #[test]
    fn content_not_kept_cannot_be_put_back() {
        let big = untouched(&[change("big.bin", FileState::Unstored, stored("b1"))]);
        assert_eq!((big.len(), big[0].skip), (1, Some(Skip::NotKept)));
        let after = untouched(&[change("a.rs", stored("a0"), FileState::Unstored)]);
        assert_eq!(after[0].skip, Some(Skip::NotKept));
        // Unstored in the middle cannot be proven to join up.
        let middle = untouched(&[
            change("a.rs", stored("a0"), FileState::Unstored),
            change("a.rs", FileState::Unstored, stored("a2")),
        ]);
        assert_eq!(middle[0].skip, Some(Skip::ChangedBetween));
    }

    #[test]
    fn a_copy_cleared_away_is_said_to_be_gone() {
        let changes = [change("a.rs", stored("a0"), stored("a1"))];
        let rewound = plan(&changes, |_| stored("a1"), |hash| hash != "a0");
        assert_eq!(rewound[0].skip, Some(Skip::Expired));
    }

    #[test]
    fn only_paths_inside_the_folder_are_planned() {
        for path in ["/etc/passwd", "../x", "a/../../x", "a//b", "./a", "C:\\x", "..\\x", ""] {
            let rewound = untouched(&[change(path, stored("0"), stored("1"))]);
            assert_eq!(rewound[0].skip, Some(Skip::UnsafePath), "{path}");
        }
        assert_eq!(untouched(&[change("src/a.rs", stored("0"), stored("1"))])[0].skip, None);
    }

    #[test]
    fn the_wire_shape_is_stable() {
        let json = serde_json::to_value(change("a.rs", FileState::Absent, stored("h"))).unwrap();
        assert_eq!(json, serde_json::json!({"path": "a.rs", "before": {"kind": "absent"}, "after": {"kind": "stored", "hash": "h"}}));
    }
}
