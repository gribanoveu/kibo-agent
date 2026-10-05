//! Writes a file of the open folder that the user edited in the viewer.

use std::fs;
use std::path::{Component, Path};

use crate::domain::file_write::{in_endings, line_ending, FileSave, FileWriteError};

/// Writes `content` to `path`, relative to `root`, if the file still holds
/// `expected` — the text it was opened with; `None` writes whatever is there,
/// or recreates it, once the user chose to keep their edit. The file keeps its
/// line endings. A path out of the folder, through a symlink too, is refused,
/// as is a folder or a file that is not UTF-8.
// ponytail: the check and the write are not one step — a write landing
// between them is overwritten. The window it leaves is a disk read long.
pub fn write(root: &Path, path: &str, expected: Option<&str>, content: &str) -> Result<FileSave, FileWriteError> {
    let refused = || FileWriteError::InvalidPath(path.to_string());
    let io = |e: std::io::Error| FileWriteError::Io(path.to_string(), e.to_string());
    let rel = Path::new(path);
    if path.is_empty() || !rel.components().all(|c| matches!(c, Component::Normal(_))) {
        return Err(refused());
    }
    // Joined to the root as the folder was opened, not to its canonical form:
    // on Windows that is a `\\?\` path, where `/` from the frontend is not a
    // separator. The canonical one is only compared against.
    let full = root.join(rel);
    let root = root.canonicalize().map_err(|_| refused())?;
    // What is there decides where it is: an existing file by its own real
    // path, a missing one by its folder's.
    let inside = match full.canonicalize() {
        Ok(real) => real.starts_with(&root) && !real.is_dir(),
        Err(_) => full.parent().and_then(|dir| dir.canonicalize().ok()).is_some_and(|dir| dir.starts_with(&root)),
    };
    if !inside {
        return Err(refused());
    }

    let on_disk = match fs::read(&full) {
        Ok(bytes) => Some(String::from_utf8(bytes).map_err(|_| FileWriteError::NotText(path.to_string()))?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(io(e)),
    };
    if let Some(expected) = expected {
        if on_disk.as_deref() != Some(expected) {
            return Ok(FileSave::ChangedOnDisk);
        }
    }
    let ending = on_disk.as_deref().and_then(line_ending);
    fs::write(&full, in_endings(content, ending).as_bytes()).map_err(io)?;
    Ok(FileSave::Saved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::temp_dir;

    fn folder() -> std::path::PathBuf {
        let dir = temp_dir("file-write");
        fs::create_dir_all(dir.join("docs")).unwrap();
        fs::write(dir.join("docs/a.adoc"), "= Title\n\nText.\n").unwrap();
        dir
    }

    fn read(dir: &Path, path: &str) -> String {
        fs::read_to_string(dir.join(path)).unwrap()
    }

    #[test]
    fn writes_over_the_version_it_was_opened_with() {
        let dir = folder();
        let saved = write(&dir, "docs/a.adoc", Some("= Title\n\nText.\n"), "= Title\n\nMore text.\n");
        assert_eq!(saved, Ok(FileSave::Saved));
        assert_eq!(read(&dir, "docs/a.adoc"), "= Title\n\nMore text.\n");
    }

    /// The agent, or anything else, wrote the file after the editor read it:
    /// that write is kept, and the editor is told.
    #[test]
    fn a_file_changed_or_deleted_since_it_was_opened_is_left_alone() {
        let dir = folder();
        fs::write(dir.join("docs/a.adoc"), "= Title\n\nThe agent's text.\n").unwrap();
        let saved = write(&dir, "docs/a.adoc", Some("= Title\n\nText.\n"), "mine\n");
        assert_eq!(saved, Ok(FileSave::ChangedOnDisk));
        assert_eq!(read(&dir, "docs/a.adoc"), "= Title\n\nThe agent's text.\n");

        fs::remove_file(dir.join("docs/a.adoc")).unwrap();
        assert_eq!(write(&dir, "docs/a.adoc", Some("= Title\n\nText.\n"), "mine\n"), Ok(FileSave::ChangedOnDisk));
        assert!(!dir.join("docs/a.adoc").exists());
    }

    #[test]
    fn keeping_the_edit_writes_over_whatever_is_there_or_recreates_it() {
        let dir = folder();
        fs::write(dir.join("docs/a.adoc"), "theirs\n").unwrap();
        assert_eq!(write(&dir, "docs/a.adoc", None, "mine\n"), Ok(FileSave::Saved));
        assert_eq!(read(&dir, "docs/a.adoc"), "mine\n");

        fs::remove_file(dir.join("docs/a.adoc")).unwrap();
        assert_eq!(write(&dir, "docs/a.adoc", None, "mine\n"), Ok(FileSave::Saved));
        assert_eq!(read(&dir, "docs/a.adoc"), "mine\n");
    }

    #[test]
    fn a_crlf_file_stays_crlf() {
        let dir = folder();
        fs::write(dir.join("docs/w.adoc"), "a\r\nb\r\n").unwrap();
        assert_eq!(write(&dir, "docs/w.adoc", Some("a\r\nb\r\n"), "a\nc\nb\n"), Ok(FileSave::Saved));
        assert_eq!(read(&dir, "docs/w.adoc"), "a\r\nc\r\nb\r\n");
    }

    #[test]
    fn refuses_a_path_out_of_the_folder_a_folder_and_a_file_that_is_not_text() {
        let dir = folder();
        let outside = temp_dir("file-write-outside");
        fs::write(outside.join("secret.txt"), "s\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.join("secret.txt"), dir.join("link.txt")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, dir.join("away")).unwrap();
        let mut bad = vec!["", "..", "../x", "/etc/passwd", "./docs/a.adoc", "docs/../docs/a.adoc", "docs", "no/such/dir.txt"];
        if cfg!(unix) {
            bad.extend(["link.txt", "away/new.txt"]);
        }
        for path in bad {
            assert_eq!(write(&dir, path, None, "x"), Err(FileWriteError::InvalidPath(path.into())), "{path}");
        }
        assert_eq!(read(&outside, "secret.txt"), "s\n");
        assert!(!outside.join("new.txt").exists());

        fs::write(dir.join("docs/bin.dat"), [0xff, 0xfe, 0x00, 0x41]).unwrap();
        assert_eq!(write(&dir, "docs/bin.dat", None, "x"), Err(FileWriteError::NotText("docs/bin.dat".into())));
        assert_eq!(fs::read(dir.join("docs/bin.dat")).unwrap(), [0xff, 0xfe, 0x00, 0x41]);
    }
}
