//! Meetings on disk: `~/.kibo/meetings/<id>.json`, one file each — what the
//! Catalog's "Delete data" for meeting notes removes.

use std::fs;
use std::path::{Path, PathBuf};

use crate::domain::meeting::{Meeting, MeetingError, MeetingInfo};
use crate::infra::app_dir;

/// Under the app directory; the feature's `data` in `domain::features`.
pub const DIR: &str = "meetings";

fn store(e: impl std::fmt::Display) -> MeetingError {
    MeetingError::Store(e.to_string())
}

fn dir() -> Result<PathBuf, MeetingError> {
    Ok(app_dir::dir().map_err(store)?.join(DIR))
}

/// The id is the window's: only what [`save`] could have named.
fn path(dir: &Path, id: &str) -> Result<PathBuf, MeetingError> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return Err(MeetingError::NotFound(id.to_string()));
    }
    Ok(dir.join(format!("{id}.json")))
}

/// Newest first. A file that does not read is skipped, not the list.
// ponytail: every file is read in full for its row; keep an index when there are hundreds.
pub fn list() -> Result<Vec<MeetingInfo>, MeetingError> {
    let entries = match fs::read_dir(dir()?) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(store(e)),
    };
    let mut rows: Vec<MeetingInfo> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|p| fs::read(&p).ok().and_then(|bytes| serde_json::from_slice::<Meeting>(&bytes).ok()))
        .map(|m| m.info())
        .collect();
    rows.sort_by(|a, b| b.created.cmp(&a.created));
    Ok(rows)
}

pub fn get(id: &str) -> Result<Meeting, MeetingError> {
    let bytes = match fs::read(path(&dir()?, id)?) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(MeetingError::NotFound(id.to_string())),
        Err(e) => return Err(store(e)),
    };
    serde_json::from_slice(&bytes).map_err(store)
}

pub fn save(meeting: &Meeting) -> Result<(), MeetingError> {
    let bytes = serde_json::to_vec(meeting).map_err(store)?;
    app_dir::write_private(&path(&dir()?, &meeting.id)?, &bytes).map_err(store)
}

pub fn delete(id: &str) -> Result<(), MeetingError> {
    match fs::remove_file(path(&dir()?, id)?) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(store(e)),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::meeting::Utterance;
    use crate::testing::with_app_dir;

    fn meeting(id: &str, created: &str) -> Meeting {
        Meeting {
            id: id.into(),
            name: "call.m4a".into(),
            created: created.into(),
            seconds: 12.5,
            utterances: vec![Utterance { speaker: 1, start: 0.0, end: 1.0, text: "да".into() }],
            summary: None,
        }
    }

    #[test]
    fn a_saved_meeting_reads_back_and_lists_newest_first() {
        with_app_dir("meeting-store", || {
            assert_eq!(list().unwrap(), [], "no folder yet is no meetings");
            save(&meeting("aa", "2026-10-01T10:00:00Z")).unwrap();
            save(&meeting("bb", "2026-10-02T10:00:00Z")).unwrap();
            fs::write(dir().unwrap().join("cc.json"), "not json").unwrap();
            assert_eq!(get("aa").unwrap(), meeting("aa", "2026-10-01T10:00:00Z"));
            assert_eq!(list().unwrap().iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["bb", "aa"]);

            delete("aa").unwrap();
            assert_eq!(get("aa"), Err(MeetingError::NotFound("aa".into())));
            delete("aa").unwrap();
        });
    }

    #[test]
    fn an_id_that_is_not_one_of_ours_reaches_no_file() {
        with_app_dir("meeting-store-ids", || {
            for id in ["", "../settings", "a/b", "x.json"] {
                assert_eq!(get(id), Err(MeetingError::NotFound(id.into())), "{id}");
                assert!(delete(id).is_err(), "{id}");
            }
        });
    }
}
