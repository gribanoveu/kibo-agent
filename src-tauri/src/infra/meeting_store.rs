//! Meetings on disk, a folder each — `~/.kibo/meetings/<id>/`, what the
//! Catalog's "Delete data" for meeting notes removes:
//!
//! - `meeting.json` — the transcript with its times, the names, the notes;
//! - `audio.wav` — the recording as the models heard it (16 kHz mono 16-bit),
//!   not the file it came from.
//!
//! A meeting is the whole folder: whatever lists, shows or deletes one takes it
//! all, by id.

use std::fs;
use std::path::{Path, PathBuf};

use crate::domain::meeting::{Meeting, MeetingError, MeetingInfo};
use crate::infra::app_dir;

/// Under the app directory; the feature's `data` in `domain::features`.
pub const DIR: &str = "meetings";
const MEETING: &str = "meeting.json";
const AUDIO: &str = "audio.wav";

fn store(e: impl std::fmt::Display) -> MeetingError {
    MeetingError::Store(e.to_string())
}

fn dir() -> Result<PathBuf, MeetingError> {
    Ok(app_dir::dir().map_err(store)?.join(DIR))
}

/// The id is the window's: only what [`save`] could have named.
fn folder(dir: &Path, id: &str) -> Result<PathBuf, MeetingError> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return Err(MeetingError::NotFound(id.to_string()));
    }
    Ok(dir.join(id))
}

/// Where meeting `id`'s recording is kept. A folder with it and no
/// `meeting.json` yet is a meeting still being recognized: not listed.
pub fn audio(id: &str) -> Result<PathBuf, MeetingError> {
    Ok(folder(&dir()?, id)?.join(AUDIO))
}

/// Newest first. A meeting that does not read is skipped, not the list.
// ponytail: every file is read in full for its row; keep an index when there are hundreds.
pub fn list() -> Result<Vec<MeetingInfo>, MeetingError> {
    let entries = match fs::read_dir(dir()?) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(store(e)),
    };
    let mut rows: Vec<MeetingInfo> = entries
        .flatten()
        .filter_map(|e| fs::read(e.path().join(MEETING)).ok())
        .filter_map(|bytes| serde_json::from_slice::<Meeting>(&bytes).ok())
        .map(|m| m.info())
        .collect();
    rows.sort_by(|a, b| b.created.cmp(&a.created));
    Ok(rows)
}

pub fn get(id: &str) -> Result<Meeting, MeetingError> {
    let bytes = match fs::read(folder(&dir()?, id)?.join(MEETING)) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(MeetingError::NotFound(id.to_string())),
        Err(e) => return Err(store(e)),
    };
    serde_json::from_slice(&bytes).map_err(store)
}

pub fn save(meeting: &Meeting) -> Result<(), MeetingError> {
    let bytes = serde_json::to_vec(meeting).map_err(store)?;
    app_dir::write_private(&folder(&dir()?, &meeting.id)?.join(MEETING), &bytes).map_err(store)
}

/// The meeting and its recording.
pub fn delete(id: &str) -> Result<(), MeetingError> {
    match fs::remove_dir_all(folder(&dir()?, id)?) {
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
            names: Default::default(),
            guessed: Vec::new(),
            title: None,
        }
    }

    #[test]
    fn a_saved_meeting_reads_back_and_lists_newest_first() {
        with_app_dir("meeting-store", || {
            assert_eq!(list().unwrap(), [], "no folder yet is no meetings");
            save(&meeting("aa", "2026-10-01T10:00:00Z")).unwrap();
            save(&meeting("bb", "2026-10-02T10:00:00Z")).unwrap();
            fs::create_dir_all(dir().unwrap().join("cc")).unwrap();
            fs::write(dir().unwrap().join("cc").join(MEETING), "not json").unwrap();
            fs::create_dir_all(dir().unwrap().join("dd")).unwrap(); // still being recognized
            assert_eq!(get("aa").unwrap(), meeting("aa", "2026-10-01T10:00:00Z"));
            assert_eq!(list().unwrap().iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["bb", "aa"]);

            fs::write(audio("aa").unwrap(), "RIFF").unwrap();
            delete("aa").unwrap();
            assert_eq!(get("aa"), Err(MeetingError::NotFound("aa".into())));
            assert!(!dir().unwrap().join("aa").exists(), "the recording went with it");
            delete("aa").unwrap();
        });
    }

    #[test]
    fn the_folder_is_the_one_delete_data_removes() {
        use crate::domain::features::Feature;
        assert_eq!(Feature::MeetingNotes.manifest().data, Some(DIR));
        assert_eq!(Feature::VoiceMemory.manifest().data, Some(crate::infra::voices_store::FILE));
    }

    #[test]
    fn an_id_that_is_not_one_of_ours_reaches_no_file() {
        with_app_dir("meeting-store-ids", || {
            for id in ["", "../settings", "a/b", "x.json"] {
                assert_eq!(get(id), Err(MeetingError::NotFound(id.into())), "{id}");
                assert!(delete(id).is_err(), "{id}");
                assert!(audio(id).is_err(), "{id}");
            }
        });
    }
}
