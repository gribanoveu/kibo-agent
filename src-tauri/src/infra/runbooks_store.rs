//! The user's runbooks on disk: `*.md` in `~/.kibo/runbooks/kubernetes`
//! (under the app directory) — a folder per role, so another role's notes
//! never reach this one. A chat has no folder, so there is no project's set. Read whole
//! each turn: a handful of small files, and one just written is there for
//! the next answer.

use std::fs;
use std::path::PathBuf;

use crate::domain::runbooks::{self, Runbook};
use crate::infra::app_dir;

/// Past this a file is not a note someone wrote by hand, and all of it would
/// go into the conversation.
const MAX_FILE_BYTES: u64 = 32 * 1024;

pub fn dir() -> Result<PathBuf, String> {
    app_dir::dir().map(|dir| dir.join("runbooks").join("kubernetes"))
}

/// Every runbook in the folder. A missing folder is none; a file that cannot
/// be read, is too large or is badly named is left out rather than failing
/// the rest.
pub fn own() -> Vec<Runbook> {
    let Ok(read) = dir().and_then(|dir| fs::read_dir(dir).map_err(|e| e.to_string())) else { return Vec::new() };
    read.filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .filter_map(|path| {
            let meta = fs::metadata(&path).ok()?;
            if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
                return None;
            }
            runbooks::parse(path.file_stem()?.to_str()?, &fs::read_to_string(&path).ok()?, true)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::with_app_dir;

    #[test]
    fn the_users_folder_is_read_and_what_is_not_a_runbook_is_left_out() {
        with_app_dir("runbooks-own", || {
            assert!(own().is_empty(), "no folder is no runbooks");
            let dir = dir().unwrap();
            fs::create_dir_all(dir.join("folder.md")).unwrap();
            // Beside the role's folder, not in it: someone else's.
            fs::write(dir.parent().unwrap().join("stray.md"), "Sign: not this role's").unwrap();
            fs::write(dir.join("quota.md"), "Sign: pods are not created\nAsk the platform team.").unwrap();
            fs::write(dir.join("notes.txt"), "Sign: not a runbook").unwrap();
            fs::write(dir.join("Bad Name.md"), "Sign: badly named").unwrap();
            fs::write(dir.join("huge.md"), "x".repeat(MAX_FILE_BYTES as usize + 1)).unwrap();
            let read = own();
            assert_eq!(read.len(), 1, "{read:?}");
            assert_eq!((read[0].name.as_str(), read[0].own, read[0].text.as_str()), ("quota", true, "Sign: pods are not created\nAsk the platform team."));
        });
    }
}
