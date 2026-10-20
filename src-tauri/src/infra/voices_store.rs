//! The voice memory on disk: `~/.kibo/voices.json`, every voice print in one
//! file — what the feature's "Delete data" removes.

use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::domain::meeting::MeetingError;
use crate::domain::voices::VoiceBook;
use crate::infra::app_dir;

/// Under the app directory; the feature's `data` in `domain::features`.
pub const FILE: &str = "voices.json";

/// From the read to the write: two changes at once would lose one.
static LOCK: Mutex<()> = Mutex::new(());

fn store(e: impl std::fmt::Display) -> MeetingError {
    MeetingError::Store(e.to_string())
}

fn path() -> Result<PathBuf, MeetingError> {
    Ok(app_dir::dir().map_err(store)?.join(FILE))
}

fn load() -> Result<Option<VoiceBook>, MeetingError> {
    match fs::read(path()?) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(store),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(store(e)),
    }
}

/// The book as it is now; empty when there is none.
pub fn read() -> Result<VoiceBook, MeetingError> {
    Ok(load()?.unwrap_or_default())
}

/// `change` applied and saved. With `create` false, no book is no change —
/// so forgetting never writes a file that was not there.
pub fn update<T>(create: bool, change: impl FnOnce(&mut VoiceBook) -> T) -> Result<Option<T>, MeetingError> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut book = match load()? {
        Some(book) => book,
        None if create => VoiceBook::default(),
        None => return Ok(None),
    };
    let out = change(&mut book);
    let bytes = serde_json::to_vec(&book).map_err(store)?;
    app_dir::write_private(&path()?, &bytes).map_err(store)?;
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::with_app_dir;

    #[test]
    fn a_change_is_saved_and_forgetting_makes_no_file() {
        with_app_dir("voices-store", || {
            assert_eq!(update(false, |b| b.forget_meeting("m")).unwrap(), None);
            assert!(!path().unwrap().exists());
            update(true, |b| b.hear("m", &[vec![1.0, 0.0]], &[60.0])).unwrap();
            assert_eq!(read().unwrap().heard.len(), 1);
            update(false, |b| b.forget_meeting("m")).unwrap();
            assert_eq!(read().unwrap(), VoiceBook::default());
        });
    }
}
