//! Saving a file the user edited in the viewer: only over the version they
//! opened, and in the file's own line endings.

use std::borrow::Cow;

use serde::Serialize;
use thiserror::Error;

/// What a save did. A file changed on disk since it was opened — the agent
/// wrote it, or anything else did — is an answer, not a failure: nothing is
/// written, and the window asks whether to load the new one or keep the edit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum FileSave {
    Saved,
    /// Deleted counts too: what was opened is not what is there.
    ChangedOnDisk,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FileWriteError {
    #[error("not a file inside the open folder: {0}")]
    InvalidPath(String),
    /// Read back as text it would not be the same bytes: saved from here, the
    /// rest of the file would be lost.
    #[error("{0} is not UTF-8 text, so it is not saved from here")]
    NotText(String),
    #[error("could not write {0}: {1}")]
    Io(String, String),
}

/// The file's line ending, when every line break in it is the same one.
/// `None` for a file with no line breaks, or with both kinds.
pub fn line_ending(content: &str) -> Option<&'static str> {
    let crlf = content.matches("\r\n").count();
    match (crlf, content.matches('\n').count()) {
        (0, 0) => None,
        (0, _) => Some("\n"),
        (crlf, lf) if crlf == lf => Some("\r\n"),
        _ => None,
    }
}

/// `text` with every line break made `ending`, the file's own.
///
/// Neither writer of a file sends its line endings reliably. A model reads a
/// CRLF file with the `\r`s invisible and writes its anchor with `\n`; asked
/// to resend with CRLF, it went around `editFile` through a shell command
/// instead, at twice the rounds (`agent_bench`, `crlf-edit`). An editor hands
/// its text back with `\n` whatever the file had, and a CRLF file saved that
/// way would show every line changed. A file with a single line ending leaves
/// no doubt which one was meant; a file with a mix, or none, takes the text as
/// it is.
pub fn in_endings<'a>(text: &'a str, ending: Option<&str>) -> Cow<'a, str> {
    match ending {
        Some("\r\n") if text.contains('\n') => Cow::Owned(text.replace("\r\n", "\n").replace('\n', "\r\n")),
        Some("\n") if text.contains('\r') => Cow::Owned(text.replace("\r\n", "\n")),
        _ => Cow::Borrowed(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_ending_is_named_only_when_it_is_the_only_one() {
        assert_eq!(line_ending("a\nb\n"), Some("\n"));
        assert_eq!(line_ending("a\r\nb\r\n"), Some("\r\n"));
        assert_eq!(line_ending("a\r\nb\n"), None);
        assert_eq!(line_ending("one line"), None);
    }

    #[test]
    fn text_takes_the_files_one_line_ending_and_a_mixed_file_takes_it_as_it_is() {
        let saved = |disk: &str, text: &str| in_endings(text, line_ending(disk)).into_owned();
        assert_eq!(saved("a\r\nb\r\n", "a\nc\nb\n"), "a\r\nc\r\nb\r\n");
        // Already CRLF, or partly: not doubled.
        assert_eq!(saved("a\r\n", "a\r\nb\n"), "a\r\nb\r\n");
        assert_eq!(saved("a\nb\n", "a\r\nc\n"), "a\nc\n");
        assert_eq!(saved("a\r\nb\n", "x\r\ny\n"), "x\r\ny\n");
        assert_eq!(saved("", "new\n"), "new\n");
    }
}
