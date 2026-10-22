//! The Artifacts page (`docs/38-artifacts.md`): what the app made, as folders —
//! a kind, a collection of that kind (a meeting, a prototype), its files.
//!
//! The rules live here: how a file opens, the order things are listed in, and
//! the files a meeting is shown as, written from its record.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::meeting::{render_transcript, Meeting, MeetingError};
use super::prototype::PrototypeError;

/// The page's top folders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ArtifactKind {
    Meetings,
    Prototypes,
}

/// One meeting or one prototype.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Collection {
    pub kind: ArtifactKind,
    /// What its own store knows it by: the meeting's id, the chat's.
    pub id: String,
    pub title: String,
    /// RFC 3339: when the meeting was held, when the prototype was last presented.
    pub updated: String,
    /// The recording's size; the prototype's working folder's.
    pub bytes: u64,
    /// The prototype's page, relative to its folder; a meeting has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<String>,
}

/// A file in a collection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactFile {
    /// Inside the collection, with `/`: `assets/logo.png`.
    pub path: String,
    pub bytes: u64,
    pub view: FileView,
}

/// What a file opens in — decided here, by its name, so the page only draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FileView {
    Markdown,
    Code,
    Html,
    Image,
    Audio,
    Other,
}

impl FileView {
    pub fn of(path: &str) -> FileView {
        let name = path.rsplit('/').next().unwrap_or(path);
        let extension = name.rsplit_once('.').map(|(_, x)| x.to_ascii_lowercase()).unwrap_or_default();
        match extension.as_str() {
            "md" | "markdown" => FileView::Markdown,
            "html" | "htm" => FileView::Html,
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "avif" | "svg" | "ico" => FileView::Image,
            "wav" | "mp3" | "m4a" | "ogg" | "flac" => FileView::Audio,
            "txt" | "js" | "jsx" | "mjs" | "ts" | "tsx" | "css" | "json" | "csv" | "xml" | "yaml" | "yml" | "toml" => FileView::Code,
            _ => FileView::Other,
        }
    }
}

#[derive(Debug, Error)]
pub enum ArtifactError {
    #[error("there is no {0}")]
    NotFound(String),
    #[error("{path} is over {} MB, too big to show here", limit / (1024 * 1024))]
    TooBig { path: String, limit: u64 },
    /// A recording, a picture: what opens through its protocol, not as text.
    #[error("{0} is not text")]
    NotText(String),
    #[error("a name cannot be empty")]
    NoTitle,
    #[error("a name is {0} characters at most")]
    TitleTooLong(usize),
    /// "Save as" named a place it does not write to: not a file's full path,
    /// or inside the app's own folder, whose files are its state.
    #[error("cannot save to {0} — choose a file outside the app's own folder")]
    NotADestination(String),
    #[error("{path}: {source}")]
    Io { path: String, source: std::io::Error },
    #[error(transparent)]
    Meeting(#[from] MeetingError),
    #[error(transparent)]
    Prototype(#[from] PrototypeError),
}

/// The most a collection's name may be: a few words, as a `present` card's.
pub const MAX_TITLE_CHARS: usize = 80;

/// A name the user gave a collection: its words with one space between them,
/// neither empty nor long.
pub fn title(raw: &str) -> Result<String, ArtifactError> {
    let words = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    match words.chars().count() {
        0 => Err(ArtifactError::NoTitle),
        n if n > MAX_TITLE_CHARS => Err(ArtifactError::TitleTooLong(MAX_TITLE_CHARS)),
        _ => Ok(words),
    }
}

/// Newest first; one moment's in kind, then id order, so a list reads the same each time.
pub fn sort_collections(collections: &mut [Collection]) {
    collections.sort_by(|a, b| b.updated.cmp(&a.updated).then(a.kind.cmp(&b.kind)).then_with(|| a.id.cmp(&b.id)));
}

/// Paths in the order a folder lists them: at each level the folders first,
/// then by name, case aside.
pub fn file_order(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.split('/').peekable(), b.split('/').peekable());
    loop {
        match (a.next(), b.next()) {
            (Some(x), Some(y)) => {
                // A folder and a file of one name differ too: the folder first.
                let (x_folder, y_folder) = (a.peek().is_some(), b.peek().is_some());
                let order = y_folder.cmp(&x_folder).then_with(|| x.to_lowercase().cmp(&y.to_lowercase())).then_with(|| x.cmp(y));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (None, None) => return Ordering::Equal,
        }
    }
}

pub fn sort_files(files: &mut [ArtifactFile]) {
    files.sort_by(|a, b| file_order(&a.path, &b.path));
}

pub const NOTES: &str = "notes.md";
/// Plain text: Markdown would run its lines into one paragraph.
pub const TRANSCRIPT: &str = "transcript.txt";
/// The meeting's recording — the one file of a meeting that is a file.
pub const AUDIO: &str = "audio.wav";

/// The files a meeting is shown as, written from its record, each with its
/// text: the notes under its title, a document per subject, the transcript.
/// Not kept anywhere — `meeting.json` stays the one place they are true.
pub fn meeting_texts(meeting: &Meeting) -> Vec<(String, String)> {
    let title = meeting.title.as_deref().unwrap_or(&meeting.name);
    let mut out = Vec::new();
    if let Some(summary) = &meeting.summary {
        out.push((NOTES.to_string(), format!("# {title}\n\n{summary}")));
    }
    for document in &meeting.documents {
        let name = document_name(&document.topic, |name| {
            [NOTES, TRANSCRIPT, AUDIO].contains(&name) || out.iter().any(|(taken, _)| taken == name)
        });
        out.push((name, format!("# {}\n\n{}", document.topic, document.text)));
    }
    if !meeting.utterances.is_empty() {
        out.push((TRANSCRIPT.to_string(), render_transcript(meeting).concat()));
    }
    out
}

/// A document's file name, from its subject: what no file system refuses —
/// no separators, no characters Windows keeps for itself, no leading dot —
/// with a number after it while `taken` says the name is in use.
fn document_name(topic: &str, taken: impl Fn(&str) -> bool) -> String {
    let cleaned: String = topic.chars().map(|c| if c.is_control() || r#"/\:*?"<>|"#.contains(c) { ' ' } else { c }).collect();
    let words = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let stem: String = words.trim_start_matches(['.', ' ']).chars().take(80).collect();
    let stem = match stem.trim_end() {
        "" => "document",
        stem => stem,
    };
    let mut name = format!("{stem}.md");
    let mut n = 2;
    while taken(&name) {
        name = format!("{stem} {n}.md");
        n += 1;
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::meeting::{MeetingDocument, Utterance};

    #[test]
    fn a_file_opens_by_its_extension_whatever_its_case() {
        for (path, view) in [
            ("notes.md", FileView::Markdown),
            ("docs/README.MD", FileView::Markdown),
            ("index.html", FileView::Html),
            ("slides.HTM", FileView::Html),
            ("assets/logo.png", FileView::Image),
            ("assets/icon.svg", FileView::Image),
            ("audio.wav", FileView::Audio),
            ("app.jsx", FileView::Code),
            ("transcript.txt", FileView::Code),
            ("style.css", FileView::Code),
            ("Makefile", FileView::Other),
            ("archive.zip", FileView::Other),
            ("md", FileView::Other),
            ("folder.md/data", FileView::Other),
        ] {
            assert_eq!(FileView::of(path), view, "{path}");
        }
    }

    #[test]
    fn a_name_is_its_words_neither_empty_nor_long() {
        assert_eq!(title("  Планёрка \n по  релизу\t").unwrap(), "Планёрка по релизу");
        assert!(matches!(title(" \n\t"), Err(ArtifactError::NoTitle)));
        assert_eq!(title(&"я".repeat(MAX_TITLE_CHARS)).unwrap().chars().count(), MAX_TITLE_CHARS, "the limit itself is a name");
        assert!(matches!(title(&"я".repeat(MAX_TITLE_CHARS + 1)), Err(ArtifactError::TitleTooLong(MAX_TITLE_CHARS))));
    }

    #[test]
    fn a_folder_lists_its_folders_first_then_its_files_by_name() {
        let mut paths = ["index.html", "assets/logo.png", "App.jsx", "assets/fonts/a.woff", "assets/b.png", "b.css", "assets", "a/z"];
        paths.sort_by(|a, b| file_order(a, b));
        assert_eq!(paths, ["a/z", "assets/fonts/a.woff", "assets/b.png", "assets/logo.png", "App.jsx", "assets", "b.css", "index.html"]);
        assert_eq!(file_order("x", "x"), Ordering::Equal);
        assert_eq!(file_order("assets", "assets/logo.png"), Ordering::Greater, "a file after the folder of its name");
        assert_eq!(file_order("assets/logo.png", "assets"), Ordering::Less);
    }

    fn collection(kind: ArtifactKind, id: &str, updated: &str) -> Collection {
        Collection { kind, id: id.into(), title: id.into(), updated: updated.into(), bytes: 0, page: None }
    }

    #[test]
    fn collections_are_newest_first_and_the_same_each_time() {
        let mut all = vec![
            collection(ArtifactKind::Prototypes, "p", "2026-10-21T10:00:00+00:00"),
            collection(ArtifactKind::Meetings, "b", "2026-10-22T10:00:00+00:00"),
            collection(ArtifactKind::Prototypes, "a", "2026-10-22T10:00:00+00:00"),
            collection(ArtifactKind::Meetings, "a", "2026-10-22T10:00:00+00:00"),
        ];
        sort_collections(&mut all);
        let order: Vec<_> = all.iter().map(|c| (c.kind, c.id.as_str())).collect();
        assert_eq!(order, [(ArtifactKind::Meetings, "a"), (ArtifactKind::Meetings, "b"), (ArtifactKind::Prototypes, "a"), (ArtifactKind::Prototypes, "p")]);
        assert_eq!(serde_json::to_value(ArtifactKind::Meetings).unwrap(), "meetings");
        assert_eq!(serde_json::from_str::<ArtifactKind>("\"prototypes\"").unwrap(), ArtifactKind::Prototypes);
    }

    fn meeting(summary: Option<&str>, documents: &[(&str, &str)], said: &[&str]) -> Meeting {
        Meeting {
            id: "m".into(),
            name: "call.wav".into(),
            title: None,
            title_by_user: false,
            created: "2026-10-22T10:00:00Z".into(),
            seconds: 60.0,
            utterances: said.iter().enumerate().map(|(i, text)| Utterance { speaker: 0, start: i as f32 * 10.0, end: i as f32 * 10.0 + 1.0, text: text.to_string() }).collect(),
            summary: summary.map(str::to_string),
            names: Default::default(),
            guessed: Vec::new(),
            topics: Vec::new(),
            documents: documents.iter().map(|(topic, text)| MeetingDocument { topic: topic.to_string(), text: text.to_string() }).collect(),
        }
    }

    #[test]
    fn a_meeting_is_shown_as_its_notes_documents_and_transcript() {
        let mut m = meeting(Some("## Итоги\n- да"), &[("Деплой в прод", "Шаги.")], &["Привет.", "Итак."]);
        m.title = Some("Планёрка".into());
        let texts = meeting_texts(&m);
        let names: Vec<_> = texts.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["notes.md", "Деплой в прод.md", "transcript.txt"]);
        assert_eq!(texts[0].1, "# Планёрка\n\n## Итоги\n- да");
        assert_eq!(texts[1].1, "# Деплой в прод\n\nШаги.");
        assert_eq!(texts[2].1, "[00:00] Speaker 1: Привет.\n[00:10] Speaker 1: Итак.\n");

        let bare = meeting_texts(&meeting(Some("x"), &[], &[]));
        assert_eq!(bare, [("notes.md".to_string(), "# call.wav\n\nx".to_string())], "named by its file until it has a title; no words, no transcript");
        assert_eq!(meeting_texts(&meeting(None, &[], &["да"]))[0].0, "transcript.txt", "no notes yet, no notes file");
    }

    #[test]
    fn a_documents_name_is_its_subject_as_a_file_system_takes_it_and_its_own() {
        let m = meeting(
            None,
            &[("CI/CD: как и зачем?", ""), ("notes", ""), ("transcript.txt", ""), ("..скрытый", ""), (" \n\t", ""), ("Деплой", ""), ("Деплой", "")],
            &[],
        );
        let names: Vec<_> = meeting_texts(&m).into_iter().map(|(name, _)| name).collect();
        assert_eq!(names, ["CI CD как и зачем.md", "notes 2.md", "transcript.txt.md", "скрытый.md", "document.md", "Деплой.md", "Деплой 2.md"]);
        let long = document_name(&"слово ".repeat(30), |_| false);
        assert_eq!(long.chars().count(), 80 + ".md".len(), "80 characters at most: {long}");
        assert_eq!(document_name(&format!("{}x", "a".repeat(79) + " "), |_| false), format!("{}.md", "a".repeat(79)), "a space the cut leaves last goes");
    }
}
