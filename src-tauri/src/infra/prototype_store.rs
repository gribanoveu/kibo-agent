//! `<app dir>/prototypes/<chat id>/`: a Prototype chat's folder
//! (`docs/31-prototype-role.md`). `files/` is what its tools work in; the
//! presented versions will sit beside it, out of their reach. Made on the
//! chat's first turn, gone with the chat.

use std::fs;
use std::path::{Path, PathBuf};

use crate::infra::app_dir;

const DIR: &str = "prototypes";
const FILES: &str = "files";

/// The chat's working folder, made if missing.
pub fn files_dir(chat_id: &str) -> Result<PathBuf, String> {
    files_dir_in(&app_dir::ensure()?, chat_id)
}

/// Removes the chat's folder with everything in it. A chat that never had
/// one — any but a Prototype chat — has nothing to remove.
pub fn remove(chat_id: &str) -> Result<(), String> {
    remove_in(&app_dir::dir()?, chat_id)
}

fn files_dir_in(base: &Path, chat_id: &str) -> Result<PathBuf, String> {
    let dir = chat_dir(base, chat_id).ok_or_else(|| format!("not a chat id: {chat_id:?}"))?.join(FILES);
    fs::create_dir_all(&dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    Ok(dir)
}

fn remove_in(base: &Path, chat_id: &str) -> Result<(), String> {
    // An id no folder could have been made for has none.
    let Some(dir) = chat_dir(base, chat_id) else { return Ok(()) };
    match fs::remove_dir_all(&dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("could not remove {}: {e}", dir.display())),
    }
}

/// The id comes from the window and becomes a path: one segment of plain
/// characters — what `crypto.randomUUID` makes — or no folder at all.
fn chat_dir(base: &Path, chat_id: &str) -> Option<PathBuf> {
    let plain = !chat_id.is_empty()
        && chat_id.len() <= 64
        && chat_id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    plain.then(|| base.join(DIR).join(chat_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::temp_dir;

    const ID: &str = "0b9d6a0e-4f4e-4c8e-9a51-3c2f1d7e8a10";

    #[test]
    fn a_chat_gets_a_folder_of_its_own_under_the_app_dir() {
        let base = temp_dir("prototype-store");
        let dir = files_dir_in(base.as_path(), ID).unwrap();
        assert_eq!(dir, base.as_path().join("prototypes").join(ID).join("files"));
        assert!(dir.is_dir());
        // Asked again, the same folder, its files kept.
        fs::write(dir.join("index.html"), "<p>hi</p>").unwrap();
        assert_eq!(files_dir_in(base.as_path(), ID).unwrap(), dir);
        assert!(dir.join("index.html").is_file());
    }

    /// An id is a path segment: anything that could climb out, nest or name
    /// nothing gets no folder, and nothing is made for it.
    #[test]
    fn an_id_that_is_not_one_plain_segment_gets_no_folder() {
        let base = temp_dir("prototype-store");
        let long = "a".repeat(65);
        for id in ["", ".", "..", "../x", "a/b", "a\\b", "a b", "a.b", "é", long.as_str()] {
            assert!(files_dir_in(base.as_path(), id).is_err(), "{id:?}");
        }
        assert!(!base.join("prototypes").exists(), "nothing made for a refused id");
    }

    #[test]
    fn removing_a_chat_takes_its_whole_folder() {
        let base = temp_dir("prototype-store");
        let dir = files_dir_in(base.as_path(), ID).unwrap();
        fs::create_dir_all(dir.join("assets")).unwrap();
        fs::write(dir.join("assets").join("a.svg"), "<svg/>").unwrap();
        let other = files_dir_in(base.as_path(), "other-chat").unwrap();

        remove_in(base.as_path(), ID).unwrap();
        assert!(!base.as_path().join("prototypes").join(ID).exists());
        assert!(other.is_dir(), "another chat's folder stays");
    }

    /// Every chat is deleted through here, and most never had a folder.
    #[test]
    fn removing_a_chat_without_a_folder_or_with_an_odd_id_is_nothing() {
        let base = temp_dir("prototype-store");
        let kept = files_dir_in(base.as_path(), ID).unwrap();
        remove_in(base.as_path(), "never-had-one").unwrap();
        remove_in(base.as_path(), "..").unwrap();
        remove_in(base.as_path(), "").unwrap();
        assert!(kept.is_dir());
        assert!(base.as_path().exists());
    }
}
