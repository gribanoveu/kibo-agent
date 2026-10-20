//! Meeting notes: a recording through the recognition process into a saved
//! transcript, and the transcript through the model into its summary
//! (`docs/36-meeting-notes-research.md`). One job at a time — the recognition
//! takes the cores, and the page has one Stop.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::domain::features::Feature;
use crate::domain::llm::{ChatRequest, LlmMessage, StreamEvent};
use crate::domain::meeting::{self, Meeting, MeetingError, MeetingEvent, MeetingEventSink, Stage};
use crate::domain::voices::VoiceInfo;
use crate::infra::meeting_process::{self, Running};
use crate::infra::{meeting_store, speech, voices_store};
use crate::services::features;
use crate::services::llm_session::{self, LlmSession};

#[derive(Default)]
pub struct MeetingNotes {
    busy: AtomicBool,
    stopped: AtomicBool,
    running: Running,
}

/// The job slot, freed when the job ends however it ends.
struct Job<'a>(&'a AtomicBool);

impl Drop for Job<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl MeetingNotes {
    fn begin(&self) -> Result<Job<'_>, MeetingError> {
        if self.busy.swap(true, Ordering::SeqCst) {
            return Err(MeetingError::Busy);
        }
        self.stopped.store(false, Ordering::SeqCst);
        Ok(Job(&self.busy))
    }

    /// Stops the job running now, recognition or summary.
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.running.stop();
    }

    /// `audio` recognized by `exe`, saved, then summarized; the meeting's id.
    /// The meeting keeps its own copy of the sound, so `audio` — an imported
    /// file, or a recording to be thrown away — is not needed after. A summary
    /// that fails leaves the meeting saved, to summarize again; a recognition
    /// that fails leaves nothing.
    pub fn process(&self, exe: &Path, models: &Path, audio: &Path, sink: &MeetingEventSink) -> Result<String, MeetingError> {
        let _job = self.begin()?;
        if !speech::has_models(models) {
            return Err(MeetingError::ModelsMissing);
        }
        let id = uuid::Uuid::new_v4().to_string();
        let name = audio.file_name().map_or_else(|| audio.display().to_string(), |n| n.to_string_lossy().into_owned());
        let meeting = match self.transcribe(exe, models, audio, &id, name, sink) {
            Ok(meeting) => meeting,
            Err(e) => {
                // The copy of the sound, written before the speech was found.
                let _ = delete_meeting(&id);
                return Err(e);
            }
        };
        sink(MeetingEvent::Transcribed { id: id.clone() });
        self.summarize_into(&meeting, sink)?;
        Ok(id)
    }

    fn transcribe(&self, exe: &Path, models: &Path, audio: &Path, id: &str, name: String, sink: &MeetingEventSink) -> Result<Meeting, MeetingError> {
        let keep = meeting_store::audio(id)?;
        let recognized = meeting_process::recognize(&self.running, exe, audio, models, &keep, &mut |stage, done, total| {
            sink(MeetingEvent::Progress { id: id.to_string(), name: name.clone(), stage, done, total })
        })?;
        if recognized.utterances.is_empty() {
            return Err(MeetingError::NoSpeech);
        }
        let mut meeting = Meeting {
            id: id.to_string(),
            name,
            created: chrono::Utc::now().to_rfc3339(),
            seconds: recognized.seconds,
            utterances: recognized.utterances,
            summary: None,
            names: Default::default(),
            guessed: Vec::new(),
            title: None,
        };
        if remembering() {
            let speech = meeting.speech();
            let known = voices_store::update(true, |book| {
                book.hear(id, &recognized.voices, &speech);
                book.recognize(id)
            })?;
            meeting.guess(known.unwrap_or_default());
        }
        meeting_store::save(&meeting)?;
        Ok(meeting)
    }

    /// The summary of meeting `id` written again.
    pub fn summarize(&self, id: &str, sink: &MeetingEventSink) -> Result<(), MeetingError> {
        let _job = self.begin()?;
        self.summarize_into(&meeting_store::get(id)?, sink)
    }

    fn summarize_into(&self, meeting: &Meeting, sink: &MeetingEventSink) -> Result<(), MeetingError> {
        let session = llm_session::resolve(None).map_err(|e| MeetingError::Summary(e.to_string()))?;
        let summary = write_summary(&session, meeting, sink, &|| self.stopped.load(Ordering::SeqCst))?;
        // Read again: a speaker renamed while the notes were written keeps the name.
        // ponytail: read, change, write without a lock; one per meeting if two writers ever race.
        let mut now = meeting_store::get(&meeting.id)?;
        now.take_notes(&summary, meeting);
        meeting_store::save(&now)
    }
}

/// Speaker `speaker` of meeting `id` called `name` — blank for its number again.
/// The user naming a speaker is the voice memory's one lesson: while it is
/// on, that speaker's voice is now that name's.
pub fn rename_speaker(id: &str, speaker: usize, name: &str) -> Result<Meeting, MeetingError> {
    let mut meeting = meeting_store::get(id)?;
    meeting.rename(speaker, name)?;
    meeting_store::save(&meeting)?;
    if remembering() {
        let name = meeting.names.get(&speaker).cloned().unwrap_or_default();
        voices_store::update(false, |book| book.learn(id, speaker, &name))?;
    }
    Ok(meeting)
}

/// Meeting `id` gone — its recording too — and what the voice memory heard in it, on or off.
pub fn delete_meeting(id: &str) -> Result<(), MeetingError> {
    meeting_store::delete(id)?;
    voices_store::update(false, |book| book.forget_meeting(id))?;
    Ok(())
}

pub fn voices() -> Result<Vec<VoiceInfo>, MeetingError> {
    Ok(voices_store::read()?.list())
}

/// The person forgotten: their voice and every sample of it. The list after.
pub fn delete_voice(id: &str) -> Result<Vec<VoiceInfo>, MeetingError> {
    Ok(voices_store::update(false, |book| {
        book.delete(id);
        book.list()
    })?
    .unwrap_or_default())
}

fn remembering() -> bool {
    features::enabled().is_on(Feature::VoiceMemory)
}

/// The notes of `meeting`, streamed to `sink` as they are written. A meeting
/// too long for one request is read in parts: notes of each, then the notes of
/// the whole from those. A stopped or empty answer is no summary.
fn write_summary(session: &LlmSession, meeting: &Meeting, sink: &MeetingEventSink, cancelled: &dyn Fn() -> bool) -> Result<String, MeetingError> {
    let lines = meeting::render_transcript(meeting);
    let transcript = lines.concat();
    let input = if meeting::fits(&transcript, session.context_limit).is_ok() {
        transcript
    } else {
        let parts = meeting::parts(&lines, meeting::budget(session.context_limit));
        let read = |done| sink(MeetingEvent::Progress { id: meeting.id.clone(), name: meeting.name.clone(), stage: Stage::Notes, done, total: parts.len() });
        let mut notes = Vec::with_capacity(parts.len());
        for (i, part) in parts.iter().enumerate() {
            read(i);
            let request = ChatRequest {
                messages: vec![LlmMessage::system(meeting::PART_INSTRUCTIONS), LlmMessage::user(part.clone())],
                tools: Vec::new(),
                model: session.model.clone(),
            };
            let response = session.provider.chat(request).map_err(|e| MeetingError::Summary(e.to_string()))?;
            if cancelled() {
                return Err(MeetingError::Cancelled);
            }
            notes.push(format!("### Part {} of {}\n\n{}", i + 1, parts.len(), response.content.unwrap_or_default().trim()));
        }
        // Every part read: what follows is the notes of the whole.
        read(parts.len());
        format!("{}\n\n{}", meeting::PARTS_PREFACE, notes.join("\n\n"))
    };
    meeting::fits(&input, session.context_limit)?;
    let request = ChatRequest {
        messages: vec![LlmMessage::system(meeting::summary_instructions(session.reply_language)), LlmMessage::user(input)],
        tools: Vec::new(),
        model: session.model.clone(),
    };
    let on_event = |event: StreamEvent| {
        if let StreamEvent::Text(text) = event {
            sink(MeetingEvent::SummaryDelta { id: meeting.id.clone(), text: text.to_string() });
        }
    };
    let result = session.provider.chat_stream(request, &on_event, cancelled).map_err(|e| MeetingError::Summary(e.to_string()))?;
    if cancelled() {
        return Err(MeetingError::Cancelled);
    }
    Some(result.text.trim().to_string())
        .filter(|text| !text.is_empty())
        .ok_or_else(|| MeetingError::Summary("the model answered with nothing".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::llm::{ChatResponse, ChatStreamResult, LlmError, LlmModelInfo, LlmProvider};
    use crate::domain::meeting::Utterance;
    use std::sync::{Arc, Mutex};

    struct Notes {
        chunks: Vec<&'static str>,
        asked: Mutex<Vec<ChatRequest>>,
    }

    impl LlmProvider for Notes {
        /// A part's notes: which part it was, by its first line's time.
        fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
            let part = request.messages[1].content.clone().unwrap_or_default();
            self.asked.lock().unwrap().push(request);
            Ok(ChatResponse { content: Some(format!("notes from {}", &part[..7])), tool_calls: Vec::new(), usage: None })
        }

        fn chat_stream(&self, request: ChatRequest, on_event: &dyn Fn(StreamEvent), _: &dyn Fn() -> bool) -> Result<ChatStreamResult, LlmError> {
            self.asked.lock().unwrap().push(request);
            self.chunks.iter().for_each(|c| on_event(StreamEvent::Text(c)));
            Ok(ChatStreamResult { text: self.chunks.concat(), ..Default::default() })
        }

        fn list_models(&self) -> Result<Vec<LlmModelInfo>, LlmError> {
            unreachable!()
        }
    }

    fn session(chunks: Vec<&'static str>, context_limit: Option<u32>) -> (LlmSession, Arc<Notes>) {
        let provider = Arc::new(Notes { chunks, asked: Mutex::new(Vec::new()) });
        let session = LlmSession {
            provider: provider.clone(),
            provider_id: "test".into(),
            model: "m".into(),
            debug_logging: false,
            context_limit,
            reply_language: Some("Russian"),
            limits: Default::default(),
        };
        (session, provider)
    }

    fn meeting() -> Meeting {
        Meeting {
            id: "m1".into(),
            name: "call.wav".into(),
            created: "2026-10-20T10:00:00Z".into(),
            seconds: 5.0,
            utterances: vec![Utterance { speaker: 0, start: 1.0, end: 2.0, text: "Решили выпустить в пятницу.".into() }],
            summary: None,
            names: Default::default(),
            guessed: Vec::new(),
            title: None,
        }
    }

    fn recorder() -> (MeetingEventSink, Arc<Mutex<Vec<MeetingEvent>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink_events = events.clone();
        (Arc::new(move |e| sink_events.lock().unwrap().push(e)), events)
    }

    #[test]
    fn the_model_is_given_the_transcript_and_its_notes_stream_to_the_page() {
        let (session, provider) = session(vec!["## Решения\n", "- Выпуск в пятницу.\n"], None);
        let (sink, events) = recorder();
        let notes = write_summary(&session, &meeting(), &sink, &|| false).unwrap();
        assert_eq!(notes, "## Решения\n- Выпуск в пятницу.");

        let asked = provider.asked.lock().unwrap();
        assert!(asked[0].tools.is_empty());
        assert_eq!(asked[0].messages[1], LlmMessage::user("[00:01] Speaker 1: Решили выпустить в пятницу.\n"));
        assert_eq!(asked[0].messages[0], LlmMessage::system(meeting::summary_instructions(Some("Russian"))));
        assert_eq!(
            *events.lock().unwrap(),
            [
                MeetingEvent::SummaryDelta { id: "m1".into(), text: "## Решения\n".into() },
                MeetingEvent::SummaryDelta { id: "m1".into(), text: "- Выпуск в пятницу.\n".into() },
            ]
        );
    }

    #[test]
    fn a_meeting_too_long_for_one_request_is_read_in_parts_then_as_a_whole() {
        let mut long = meeting();
        long.utterances = (0..12).map(|i| Utterance { speaker: 0, start: 60.0 * i as f32, end: 1.0, text: "Решили выпустить в пятницу.".into() }).collect();
        let (reading, provider) = session(vec!["## Итоги"], Some(150));
        let (sink, events) = recorder();
        assert_eq!(write_summary(&reading, &long, &sink, &|| false).unwrap(), "## Итоги");

        let asked = provider.asked.lock().unwrap();
        let parts = asked.len() - 1;
        assert!(parts >= 2, "{parts}");
        assert!(asked[..parts].iter().all(|r| r.messages[0] == LlmMessage::system(meeting::PART_INSTRUCTIONS)));
        let whole = asked[parts].messages[1].content.clone().unwrap();
        assert!(whole.starts_with(meeting::PARTS_PREFACE), "{whole}");
        assert!(whole.contains(&format!("### Part 1 of {parts}\n\nnotes from [00:00]")), "{whole}");
        assert!(whole.contains(&format!("### Part {parts} of {parts}")), "{whole}");
        drop(asked);

        // A stop between parts asks for no more of them.
        let (stopped, provider) = session(vec!["x"], Some(150));
        let (quiet, _) = recorder();
        assert_eq!(write_summary(&stopped, &long, &quiet, &|| true), Err(MeetingError::Cancelled));
        assert_eq!(provider.asked.lock().unwrap().len(), 1);

        let progress: Vec<_> = events.lock().unwrap().iter().filter_map(|e| match e {
            MeetingEvent::Progress { stage: Stage::Notes, done, total, .. } => Some((*done, *total)),
            _ => None,
        }).collect();
        assert_eq!(progress, (0..=parts).map(|i| (i, parts)).collect::<Vec<_>>());
    }

    #[test]
    fn a_renamed_speaker_is_saved_with_the_meeting() {
        crate::testing::with_app_dir("meeting-rename", || {
            meeting_store::save(&Meeting { id: "ab".into(), summary: Some("Speaker 1 decides.".into()), ..meeting() }).unwrap();
            let renamed = rename_speaker("ab", 0, "Оксана").unwrap();
            assert_eq!(meeting_store::get("ab").unwrap(), renamed);
            assert_eq!(renamed.summary.as_deref(), Some("Оксана decides."));
            assert_eq!(rename_speaker("ab", 5, "x"), Err(MeetingError::NoSpeaker(6)));
        });
    }

    #[test]
    fn a_name_teaches_the_voice_memory_only_while_it_is_on_and_a_deleted_meeting_is_forgotten() {
        crate::testing::with_app_dir("meeting-voices", || {
            let mut call = meeting();
            call.id = "a1".into();
            call.utterances[0].end = 61.0;
            call.utterances[0].text = "слово ".repeat(20);
            meeting_store::save(&call).unwrap();
            voices_store::update(true, |b| b.hear("a1", &[vec![1.0, 0.0]], &call.speech())).unwrap();

            rename_speaker("a1", 0, "Альбина").unwrap();
            assert_eq!(voices().unwrap(), [], "off: nothing learned");
            features::set("voice-memory", true).unwrap();
            rename_speaker("a1", 0, "Альбина").unwrap();
            assert_eq!(voices().unwrap().iter().map(|v| v.name.as_str()).collect::<Vec<_>>(), ["Альбина"]);

            delete_meeting("a1").unwrap();
            assert_eq!(voices().unwrap(), []);
            assert_eq!(voices_store::read().unwrap(), Default::default());
        });
    }

    #[test]
    fn a_deleted_voice_is_gone_from_the_list() {
        crate::testing::with_app_dir("meeting-voice-delete", || {
            assert_eq!(delete_voice("x").unwrap(), [], "no book is no voices");
            voices_store::update(true, |b| {
                b.hear("a1", &[vec![1.0, 0.0]], &[crate::domain::voices::Speech { seconds: 60.0, words: 100 }]);
                b.learn("a1", 0, "Илья");
            })
            .unwrap();
            let id = voices().unwrap()[0].id.clone();
            assert_eq!(delete_voice(&id).unwrap(), []);
        });
    }

    #[test]
    fn a_stopped_empty_or_too_long_summary_is_not_one() {
        let (sink, _) = recorder();
        let (stopped, _) = session(vec!["## Ре"], None);
        assert_eq!(write_summary(&stopped, &meeting(), &sink, &|| true), Err(MeetingError::Cancelled));
        let (empty, _) = session(vec!["  \n"], None);
        assert!(matches!(write_summary(&empty, &meeting(), &sink, &|| false), Err(MeetingError::Summary(_))));
        // A window too small even for the parts' notes.
        let (small, provider) = session(vec!["x"], Some(10));
        assert!(matches!(write_summary(&small, &meeting(), &sink, &|| false), Err(MeetingError::TooLong { .. })));
        assert!(provider.asked.lock().unwrap().iter().all(|r| r.messages[0] == LlmMessage::system(meeting::PART_INSTRUCTIONS)));
    }

    #[test]
    fn one_job_at_a_time_and_the_slot_frees_when_it_ends() {
        let notes = MeetingNotes::default();
        notes.stop();
        let job = notes.begin().unwrap();
        assert!(!notes.stopped.load(Ordering::SeqCst), "a stop before the job is not its stop");
        assert!(matches!(notes.begin(), Err(MeetingError::Busy)));
        drop(job);
        assert!(notes.begin().is_ok());
    }

    /// A stand-in for the exe that writes the copy of the sound it was given, then says `line`.
    #[cfg(unix)]
    fn sidecar(label: &str, line: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::testing::temp_dir(label);
        let exe = dir.join("sidecar");
        std::fs::write(&exe, format!("#!/bin/sh\nmkdir -p \"$(dirname \"$4\")\" && printf RIFF > \"$4\"\necho '{line}'\n")).unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let models = dir.join("models");
        std::fs::create_dir_all(&models).unwrap();
        speech::MODEL_FILES.iter().for_each(|f| std::fs::write(models.join(f), "").unwrap());
        (exe, models)
    }

    #[cfg(unix)]
    #[test]
    fn a_recognized_meeting_keeps_its_sound_and_a_failed_one_leaves_nothing() {
        crate::testing::with_app_dir("meeting-process", || {
            let (sink, events) = recorder();
            let (exe, models) = sidecar("meeting-process-ok", r#"{"type":"result","utterances":[{"speaker":0,"start":0.0,"end":1.0,"text":"да"}],"seconds":1.0}"#);
            // No model is set up here, so the notes fail — after the meeting is saved.
            assert!(matches!(MeetingNotes::default().process(&exe, &models, Path::new("/calls/call.m4a"), &sink), Err(MeetingError::Summary(_))));
            let [MeetingEvent::Transcribed { id }] = &events.lock().unwrap()[..] else { panic!() };
            let saved = meeting_store::get(id).unwrap();
            assert_eq!((saved.name.as_str(), saved.seconds), ("call.m4a", 1.0));
            assert_eq!(std::fs::read(meeting_store::audio(id).unwrap()).unwrap(), b"RIFF");

            for (label, line) in [("meeting-process-error", r#"{"type":"error","message":"no"}"#), ("meeting-process-silent", r#"{"type":"result","utterances":[],"seconds":1.0}"#)] {
                let (exe, models) = sidecar(label, line);
                assert!(MeetingNotes::default().process(&exe, &models, Path::new("a.wav"), &sink).is_err());
            }
            let left: Vec<_> = std::fs::read_dir(meeting_store::audio(id).unwrap().parent().unwrap().parent().unwrap()).unwrap().collect();
            assert_eq!(left.len(), 1, "only the recognized one");
        });
    }

    #[test]
    fn a_build_without_models_says_so_before_starting_anything() {
        let (sink, events) = recorder();
        let empty = crate::testing::temp_dir("meeting-no-models");
        let got = MeetingNotes::default().process(Path::new("/nonexistent"), &empty, Path::new("a.wav"), &sink);
        assert_eq!(got, Err(MeetingError::ModelsMissing));
        assert!(events.lock().unwrap().is_empty());
    }
}
