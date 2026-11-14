//! What the agent has read, so a write is checked against what it saw.

use super::*;

/// What the agent has read, and what the file looked like when it did.
///
/// One registry answers two questions that would otherwise be separate
/// features. A write to a path with no entry is a write to a file the agent
/// never looked at; a write to a path whose content no longer hashes the same
/// is a write over somebody else's change. Both destroy work, and both are the
/// same lookup.
///
/// Owned by the caller and passed in, so the executor itself stays stateless —
/// the same arrangement the turn's todo list uses. It has to survive an
/// approval pause, or every approved write would come back as "the file
/// changed".
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadFiles {
    /// Keyed by root-relative path, the spelling every tool reports.
    seen: std::collections::HashMap<String, FileRead>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileRead {
    /// Of the whole file as it was on disk, not of the slice returned.
    /// A string, not a `u64`: the checkpoint crosses the webview during an
    /// approval, and a JSON number there is a JS double — it comes back rounded
    /// and every approved write reads as "changed on disk".
    hash: String,
    /// Whether the agent has seen all of it. A partial read is enough to place
    /// an anchored edit — the anchor's uniqueness and this hash cover the rest
    /// — but not to replace the file wholesale: overwriting 500 lines having
    /// read 50 destroys 450 nobody looked at.
    whole: bool,
}

/// Why a write was refused before it touched the disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteBlocked {
    NeverRead,
    /// Read, but only in part, where the whole file was needed.
    ReadInPart,
    ChangedSinceRead,
}

impl ReadFiles {
    /// Records what a read saw. `content` is the whole file, whatever slice of
    /// it was returned.
    ///
    /// A slice of a file already read whole, and unchanged since, leaves it
    /// read whole: the agent still knows all of it. Downgrading it stranded a
    /// delete — "read it in full" — behind a full read that repeating would
    /// not show again.
    pub fn record(&mut self, path: &str, content: &str, whole: bool) {
        self.keep(path.to_string(), FileRead { hash: hash(content), whole });
    }

    /// The reads of calls that ran beside each other, each with a record of
    /// its own, taken in the order they were asked — as if they had run one
    /// after another.
    pub fn merge(&mut self, other: ReadFiles) {
        for (path, read) in other.seen {
            self.keep(path, read);
        }
    }

    fn keep(&mut self, path: String, read: FileRead) {
        let known = self.seen.get(&path).is_some_and(|seen| seen.whole && seen.hash == read.hash);
        self.seen.insert(path, FileRead { whole: read.whole || known, ..read });
    }

    /// Whether the agent may write `current` at `path`, or why not.
    ///
    /// `require_whole` is the difference between an anchored edit and a
    /// wholesale replacement. An edit needs no read at all: its anchor has to
    /// match the file exactly and once, so text the agent never saw cannot be
    /// hit — and the read it would have to make first costs a round, most
    /// often after a grep that already showed it the line.
    pub fn check(&self, path: &str, current: &str, require_whole: bool) -> Result<(), WriteBlocked> {
        let Some(seen) = self.seen.get(path) else {
            return if require_whole { Err(WriteBlocked::NeverRead) } else { Ok(()) };
        };
        if seen.hash != hash(current) {
            return Err(WriteBlocked::ChangedSinceRead);
        }
        if require_whole && !seen.whole {
            return Err(WriteBlocked::ReadInPart);
        }
        Ok(())
    }

    /// After a successful write the agent knows what it just put there, so the
    /// next write to the same path must not be refused as stale. `whole` says
    /// whether it now knows the entire file: true after a replacement, and
    /// unchanged after an edit, which taught it nothing about the parts it had
    /// not read.
    pub fn record_write(&mut self, path: &str, content: &str, whole: bool) {
        let whole = whole || self.seen.get(path).is_some_and(|s| s.whole);
        self.record(path, content, whole);
    }

    /// The model no longer has the text in front of it — its read was cleared
    /// from the history to save context — so it no longer knows the whole
    /// file. An anchored edit still checks itself; a wholesale replacement
    /// would be written from memory.
    pub fn forget_whole(&mut self, path: &str) {
        if let Some(read) = self.seen.get_mut(path) {
            read.whole = false;
        }
    }

    /// A move changes where a file is, not what is in it, so what was read
    /// follows it — a file, or everything read under a directory. Otherwise
    /// the file has to be read again at its new path before it can be
    /// changed, which checks nothing.
    pub fn moved(&mut self, from: &str, to: &str) {
        let under = format!("{from}/");
        let paths: Vec<String> = self.seen.keys().filter(|p| *p == from || p.starts_with(&under)).cloned().collect();
        for path in paths {
            if let Some(read) = self.seen.remove(&path) {
                self.seen.insert(format!("{to}{}", &path[from.len()..]), read);
            }
        }
    }
}

/// Not persisted anywhere, so a non-cryptographic hash with no stability
/// guarantee across releases is enough. It detects an edit made behind the
/// agent's back, not a forgery.
fn hash(content: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    content.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}
