//! The Artifacts page's folders (`docs/38-artifacts.md`): the meetings and the
//! presented prototypes, each a collection of files. No store of its own —
//! each kind is read where it is kept, by a `match`, not a trait: there are
//! two, and a third is a variant the compiler points at every place for.

use std::fs;
use std::path::PathBuf;

use crate::domain::artifacts::{self, ArtifactError, ArtifactFile, ArtifactKind, Collection, FileView, AUDIO};
use crate::domain::features::{Feature, Features};
use crate::infra::{chat_store, meeting_store, prototype_store};

/// The most a file shown as text may be.
pub const MAX_TEXT_BYTES: u64 = 1024 * 1024;
/// A prototype shown before `prototype.json` whose chat is gone too.
const UNTITLED: &str = "Untitled prototype";
/// Its page when no manifest names one.
const PAGE: &str = "index.html";

/// Every collection, newest first. Meetings while their feature is on: off,
/// it shows its data nowhere. One that does not read is skipped, not the list.
// ponytail: every meeting.json is read in full, as `meeting_store::list` does; an index when there are hundreds.
pub fn list(features: &Features) -> Result<Vec<Collection>, ArtifactError> {
    let mut out = Vec::new();
    if features.is_on(Feature::MeetingNotes) {
        out.extend(meeting_store::list()?.into_iter().map(|m| Collection {
            kind: ArtifactKind::Meetings,
            id: m.id,
            title: m.title.unwrap_or(m.name),
            updated: m.created,
            bytes: m.bytes,
            page: None,
        }));
    }
    out.extend(prototype_store::presented()?.iter().filter_map(|id| prototype(id).ok()));
    artifacts::sort_collections(&mut out);
    Ok(out)
}

/// A collection's files, folders first.
pub fn files(kind: ArtifactKind, id: &str) -> Result<Vec<ArtifactFile>, ArtifactError> {
    let mut out: Vec<ArtifactFile> = match kind {
        ArtifactKind::Meetings => {
            let meeting = meeting_store::get(id)?;
            let mut out: Vec<ArtifactFile> =
                artifacts::meeting_texts(&meeting).into_iter().map(|(path, text)| file(path, text.len() as u64)).collect();
            if let Ok(audio) = fs::metadata(meeting_store::audio(id)?) {
                out.push(file(AUDIO.to_string(), audio.len()));
            }
            out
        }
        ArtifactKind::Prototypes => {
            prototype_store::listing(&presented(id)?)?.into_iter().map(|(path, bytes)| file(path, bytes)).collect()
        }
    };
    artifacts::sort_files(&mut out);
    Ok(out)
}

/// A collection's file as text, for the page to show: a meeting's written
/// from its record, a prototype's read from its working folder and nowhere
/// past it. Its recording, a picture, anything not UTF-8, is not text.
pub fn read(kind: ArtifactKind, id: &str, path: &str) -> Result<String, ArtifactError> {
    match kind {
        ArtifactKind::Meetings => {
            let meeting = meeting_store::get(id)?;
            if path == AUDIO {
                return Err(ArtifactError::NotText(path.to_string()));
            }
            artifacts::meeting_texts(&meeting)
                .into_iter()
                .find_map(|(name, text)| (name == path).then_some(text))
                .ok_or_else(|| ArtifactError::NotFound(path.to_string()))
        }
        ArtifactKind::Prototypes => {
            presented(id)?;
            let file = prototype_store::working_file(id, path)?.ok_or_else(|| ArtifactError::NotFound(path.to_string()))?;
            let io = |source| ArtifactError::Io { path: path.to_string(), source };
            if fs::metadata(&file).map_err(io)?.len() > MAX_TEXT_BYTES {
                return Err(ArtifactError::TooBig { path: path.to_string(), limit: MAX_TEXT_BYTES });
            }
            String::from_utf8(fs::read(&file).map_err(io)?).map_err(|_| ArtifactError::NotText(path.to_string()))
        }
    }
}

fn file(path: String, bytes: u64) -> ArtifactFile {
    ArtifactFile { view: FileView::of(&path), path, bytes }
}

/// The working folder of chat `id`'s prototype, if it has been presented: a
/// folder only worked in is no artifact yet, as in [`list`].
fn presented(id: &str) -> Result<PathBuf, ArtifactError> {
    let files = prototype_store::existing_files_dir(id)?.ok_or_else(|| ArtifactError::NotFound(id.to_string()))?;
    match prototype_store::latest(&files)? {
        Some(_) => Ok(files),
        None => Err(ArtifactError::NotFound(id.to_string())),
    }
}

fn prototype(id: &str) -> Result<Collection, ArtifactError> {
    let files = presented(id)?;
    let listed = prototype_store::listing(&files)?;
    let bytes = listed.iter().map(|(_, bytes)| bytes).sum();
    let (title, updated, page) = match prototype_store::manifest(&files)? {
        Some(manifest) => (manifest.title, manifest.presented, Some(manifest.page)),
        // Presented before `prototype.json`: named as its chat, dated by its
        // last version, and nothing written back — its next `present` does.
        // A chat store that does not answer leaves it unnamed, not unlisted.
        None => {
            let latest = prototype_store::latest(&files)?.ok_or_else(|| ArtifactError::NotFound(id.to_string()))?;
            let kept = chrono::DateTime::<chrono::Utc>::from(prototype_store::version_time(&files, latest)?).to_rfc3339();
            let title = chat_store::title(id).ok().flatten().unwrap_or_else(|| UNTITLED.to_string());
            (title, kept, listed.iter().any(|(path, _)| path == PAGE).then(|| PAGE.to_string()))
        }
    };
    Ok(Collection { kind: ArtifactKind::Prototypes, id: id.to_string(), title, updated, bytes, page })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::meeting::{Meeting, MeetingDocument, MeetingError, Utterance};
    use crate::domain::prototype::Manifest;
    use crate::testing::with_app_dir;

    const CHAT: &str = "3c5e7a9b-1d2f-4a6c-8e0b-2d4f6a8c0e1a";
    const OLD: &str = "0b9d6a0e-4f4e-4c8e-9a51-3c2f1d7e8a10";
    const ORPHAN: &str = "7f1e2d3c-4b5a-4968-8776-a5b4c3d2e1f0";

    fn on(meetings: bool) -> Features {
        Features::from_settings(&[("meeting-notes".to_string(), meetings)].into())
    }

    fn meeting(id: &str, created: &str) -> Meeting {
        Meeting {
            id: id.into(),
            name: "call.wav".into(),
            title: Some("Планёрка".into()),
            title_by_user: false,
            created: created.into(),
            seconds: 20.0,
            utterances: vec![Utterance { speaker: 0, start: 0.0, end: 1.0, text: "Привет.".into() }],
            summary: Some("## Итоги".into()),
            names: Default::default(),
            guessed: Vec::new(),
            topics: Vec::new(),
            documents: vec![MeetingDocument { topic: "Деплой".into(), text: "Шаги.".into() }],
        }
    }

    const PAGE_HTML: &str = "<h1>Кофейня</h1>";

    /// A chat's prototype, presented once: its page and a picture, kept as version 1.
    fn prototype_of(id: &str) -> PathBuf {
        let files = prototype_store::files_dir(id).unwrap();
        fs::create_dir_all(files.join("assets")).unwrap();
        fs::write(files.join("index.html"), PAGE_HTML).unwrap();
        fs::write(files.join("assets").join("logo.png"), [0x89, b'P', b'N', b'G']).unwrap();
        prototype_store::snapshot(&files).unwrap();
        files
    }

    #[test]
    fn the_list_holds_the_meetings_while_on_and_the_presented_prototypes_newest_first() {
        with_app_dir("artifacts-list", || {
            assert_eq!(list(&on(true)).unwrap(), [], "nothing yet is nothing");
            meeting_store::save(&meeting("aa", "2026-10-20T10:00:00+00:00")).unwrap();
            meeting_store::save(&Meeting { title: None, ..meeting("bb", "2026-10-23T10:00:00+00:00") }).unwrap();
            fs::write(meeting_store::audio("aa").unwrap(), "RIFF").unwrap();

            let files = prototype_of(CHAT);
            let manifest = Manifest { title: "Лендинг".into(), title_by_user: false, page: "index.html".into(), presented: "2026-10-22T10:00:00+00:00".into(), version: 1 };
            prototype_store::write_manifest(&files, &manifest).unwrap();
            prototype_store::files_dir("worked-in-only").unwrap();

            let all = list(&on(true)).unwrap();
            let rows: Vec<_> = all.iter().map(|c| (c.kind, c.id.as_str(), c.title.as_str(), c.bytes)).collect();
            assert_eq!(
                rows,
                [
                    (ArtifactKind::Meetings, "bb", "call.wav", 0),
                    (ArtifactKind::Prototypes, CHAT, "Лендинг", PAGE_HTML.len() as u64 + 4),
                    (ArtifactKind::Meetings, "aa", "Планёрка", 4),
                ],
                "named by its title, else its file; sized by its recording, by its folder"
            );
            assert_eq!((all[1].updated.as_str(), all[1].page.as_deref()), ("2026-10-22T10:00:00+00:00", Some("index.html")));
            assert_eq!(all[0].page, None);

            let off: Vec<_> = list(&on(false)).unwrap().into_iter().map(|c| c.kind).collect();
            assert_eq!(off, [ArtifactKind::Prototypes], "a feature that is off shows its data nowhere");
        });
    }

    /// Presented before it had a manifest: named as its chat, else untitled,
    /// dated by its version — and nothing written into its folder.
    #[test]
    fn a_prototype_without_a_manifest_is_named_as_its_chat() {
        with_app_dir("artifacts-old-prototype", || {
            let said = "Сделай лендинг кофейни";
            chat_store::save(OLD, "/repo", &[crate::domain::llm::LlmMessage::user(said)], &serde_json::json!([{ "kind": "user", "id": "user:0", "text": said }]), &[], None, None).unwrap();
            let old = prototype_of(OLD);
            let orphan = prototype_of(ORPHAN);
            fs::remove_file(orphan.join("index.html")).unwrap();

            let all = list(&on(false)).unwrap();
            let named = all.iter().find(|c| c.id == OLD).unwrap();
            assert_eq!((named.title.as_str(), named.page.as_deref()), (said, Some("index.html")));
            assert!(chrono::DateTime::parse_from_rfc3339(&named.updated).is_ok(), "{}", named.updated);
            let unnamed = all.iter().find(|c| c.id == ORPHAN).unwrap();
            assert_eq!((unnamed.title.as_str(), unnamed.page.as_deref()), (UNTITLED, None), "no chat, no index.html");
            assert_eq!(prototype_store::manifest(&old).unwrap(), None, "nothing written back");
        });
    }

    #[test]
    fn a_meeting_is_its_written_files_and_its_recording() {
        with_app_dir("artifacts-meeting-files", || {
            meeting_store::save(&meeting("aa", "2026-10-20T10:00:00+00:00")).unwrap();
            let paths = |id| files(ArtifactKind::Meetings, id).unwrap().into_iter().map(|f| (f.path, f.view)).collect::<Vec<_>>();
            assert_eq!(
                paths("aa"),
                [("notes.md".into(), FileView::Markdown), ("transcript.txt".into(), FileView::Code), ("Деплой.md".into(), FileView::Markdown)],
                "no recording kept, none listed"
            );
            fs::write(meeting_store::audio("aa").unwrap(), "RIFF").unwrap();
            let listed = files(ArtifactKind::Meetings, "aa").unwrap();
            assert_eq!((listed[0].path.as_str(), listed[0].bytes, listed[0].view), ("audio.wav", 4, FileView::Audio));
            let notes = listed.iter().find(|f| f.path == "notes.md").unwrap();
            assert_eq!(notes.bytes, "# Планёрка\n\n## Итоги".len() as u64, "the size of what is shown");

            assert_eq!(read(ArtifactKind::Meetings, "aa", "notes.md").unwrap(), "# Планёрка\n\n## Итоги");
            assert_eq!(read(ArtifactKind::Meetings, "aa", "Деплой.md").unwrap(), "# Деплой\n\nШаги.");
            assert_eq!(read(ArtifactKind::Meetings, "aa", "transcript.txt").unwrap(), "[00:00] Speaker 1: Привет.\n");
            assert!(matches!(read(ArtifactKind::Meetings, "aa", "audio.wav"), Err(ArtifactError::NotText(_))));
            for missing in ["meeting.json", "../aa/meeting.json", ""] {
                assert!(matches!(read(ArtifactKind::Meetings, "aa", missing), Err(ArtifactError::NotFound(_))), "{missing}");
            }
            assert!(matches!(files(ArtifactKind::Meetings, "zz"), Err(ArtifactError::Meeting(MeetingError::NotFound(_)))));
            assert!(matches!(files(ArtifactKind::Meetings, "../x"), Err(ArtifactError::Meeting(MeetingError::NotFound(_)))));
        });
    }

    #[test]
    fn a_prototype_is_its_working_folder_folders_first() {
        with_app_dir("artifacts-prototype-files", || {
            let files_dir = prototype_of(CHAT);
            fs::write(files_dir.join("app.jsx"), "export {}").unwrap();
            let listed: Vec<_> = files(ArtifactKind::Prototypes, CHAT).unwrap().into_iter().map(|f| (f.path, f.bytes, f.view)).collect();
            assert_eq!(
                listed,
                [
                    ("assets/logo.png".to_string(), 4, FileView::Image),
                    ("app.jsx".to_string(), 9, FileView::Code),
                    ("index.html".to_string(), PAGE_HTML.len() as u64, FileView::Html),
                ]
            );
            prototype_store::files_dir("worked-in-only").unwrap();
            for id in ["worked-in-only", "never-had-one", "../x", ""] {
                assert!(files(ArtifactKind::Prototypes, id).is_err(), "{id}");
            }
        });
    }

    /// A link in the folder is neither listed nor followed.
    #[cfg(unix)]
    #[test]
    fn a_link_in_a_prototype_is_neither_listed_nor_read() {
        with_app_dir("artifacts-prototype-link", || {
            let files_dir = prototype_of(CHAT);
            let outside = files_dir.parent().unwrap().parent().unwrap().join("secret.txt");
            fs::write(&outside, "key").unwrap();
            std::os::unix::fs::symlink(&outside, files_dir.join("link.txt")).unwrap();
            assert!(!files(ArtifactKind::Prototypes, CHAT).unwrap().iter().any(|f| f.path == "link.txt"));
            assert!(matches!(read(ArtifactKind::Prototypes, CHAT, "link.txt"), Err(ArtifactError::NotFound(_))));
        });
    }

    #[test]
    fn a_prototypes_file_is_read_from_its_working_folder_and_nowhere_past_it() {
        with_app_dir("artifacts-prototype-read", || {
            let files_dir = prototype_of(CHAT);
            prototype_store::write_manifest(&files_dir, &Manifest { title: "T".into(), title_by_user: false, page: "index.html".into(), presented: "2026-10-22T10:00:00+00:00".into(), version: 1 }).unwrap();
            fs::write(files_dir.parent().unwrap().parent().unwrap().join("next-door.txt"), "not its").unwrap();
            assert_eq!(read(ArtifactKind::Prototypes, CHAT, "index.html").unwrap(), PAGE_HTML);
            for outside in [
                "../prototype.json",
                "../versions/1/index.html",
                "assets/../../prototype.json",
                "../../next-door.txt",
                "/etc/hosts",
                "..",
                "assets",
                "missing.html",
            ] {
                assert!(matches!(read(ArtifactKind::Prototypes, CHAT, outside), Err(ArtifactError::NotFound(_))), "{outside}");
            }
            assert!(matches!(read(ArtifactKind::Prototypes, CHAT, "assets/logo.png"), Err(ArtifactError::NotText(_))));
            fs::write(files_dir.join("big.txt"), vec![b'a'; MAX_TEXT_BYTES as usize + 1]).unwrap();
            assert!(matches!(read(ArtifactKind::Prototypes, CHAT, "big.txt"), Err(ArtifactError::TooBig { .. })));
            fs::write(files_dir.join("big.txt"), vec![b'a'; MAX_TEXT_BYTES as usize]).unwrap();
            assert_eq!(read(ArtifactKind::Prototypes, CHAT, "big.txt").unwrap().len(), MAX_TEXT_BYTES as usize, "the limit itself is shown");

            prototype_store::files_dir(OLD).unwrap();
            fs::write(prototype_store::files_dir(OLD).unwrap().join("index.html"), "x").unwrap();
            assert!(matches!(read(ArtifactKind::Prototypes, OLD, "index.html"), Err(ArtifactError::NotFound(_))), "not presented, not an artifact");
        });
    }
}
