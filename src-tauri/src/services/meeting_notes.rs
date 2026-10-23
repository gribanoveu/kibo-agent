//! Meeting notes: a recording through the recognition process into a saved
//! transcript, and the transcript through the model into its summary
//! (`docs/36-meeting-notes-research.md`). One job at a time — the recognition
//! takes the cores, and the page has one Stop. A call is recorded beside it,
//! one at a time too, into a file of its own until it is made into a meeting.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use crate::domain::features::Feature;
use crate::domain::llm::{ChatRequest, LlmMessage, StreamEvent};
use crate::domain::meeting::{self, JobKind, JobState, Meeting, MeetingError, MeetingEvent, MeetingEventSink, Sound, Stage};
use crate::domain::recording::{self, RecordingInfo, RecordingState};
use crate::domain::voices::VoiceInfo;
use crate::infra::meeting_process::{self, Running};
use crate::infra::meeting_recorder::{self, Recorder};
use crate::infra::{meeting_store, settings_store, speech, voices_store};
use crate::services::features;
use crate::services::llm_session::{self, LlmSession};

#[derive(Default)]
pub struct MeetingNotes {
    /// The job running now, as its events have told it; `None` is a free slot.
    job: Mutex<Option<JobState>>,
    /// The recognition process, and whether the job was stopped.
    running: Running,
    /// The call being recorded, if one is.
    recorder: Recorder,
}

/// How long the recording whose file is `meta` is.
fn length(meta: &std::fs::Metadata) -> f32 {
    recording::seconds_of(recording::data_bytes_of(meta.len(), recording::CHANNELS) as u64)
}

/// When the recording whose file is `meta` began: its last write — the stop,
/// or a crash — less how long it is, which keeps time through every silence.
fn began(meta: &std::fs::Metadata) -> Option<std::time::SystemTime> {
    meta.modified().ok()?.checked_sub(std::time::Duration::from_secs_f32(length(meta)))
}

/// What a job recognizes: the file, what the meeting is called, and what it is;
/// when the call began, for a recording of ours — an import is dated when recognized.
struct Audio<'a> {
    path: &'a Path,
    name: String,
    sound: Sound,
    began: Option<std::time::SystemTime>,
}

/// The job slot, freed when the job ends however it ends — and the page told so.
struct Job<'a> {
    notes: &'a MeetingNotes,
    sink: &'a MeetingEventSink,
}

impl Job<'_> {
    /// `event` to the page, and into what a page opened later reads.
    fn tell(&self, event: MeetingEvent) {
        if let Some(job) = self.notes.slot().as_mut() {
            job.hear(&event);
        }
        (self.sink)(event);
    }
}

impl Drop for Job<'_> {
    fn drop(&mut self) {
        *self.notes.slot() = None;
        (self.sink)(MeetingEvent::Ended);
    }
}

impl MeetingNotes {
    fn slot(&self) -> MutexGuard<'_, Option<JobState>> {
        self.job.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn begin<'a>(&'a self, state: JobState, sink: &'a MeetingEventSink) -> Result<Job<'a>, MeetingError> {
        let mut slot = self.slot();
        if slot.is_some() {
            return Err(MeetingError::Busy);
        }
        *slot = Some(state);
        self.running.reset();
        Ok(Job { notes: self, sink })
    }

    /// The job running now, for a page opened while it runs.
    pub fn current(&self) -> Option<JobState> {
        self.slot().clone()
    }

    /// Meeting `id` deleted, unless a job works on it now: its notes or a
    /// document would be written into a meeting that is no more. The slot is
    /// held while it goes, so no job on it starts in between.
    pub fn delete_meeting(&self, id: &str) -> Result<(), MeetingError> {
        let slot = self.slot();
        if slot.as_ref().is_some_and(|job| job.id.as_deref() == Some(id)) {
            return Err(MeetingError::InUse);
        }
        remove_meeting(id)
    }

    /// Stops the job running now, recognition or summary.
    pub fn stop(&self) {
        self.running.stop();
    }

    /// The job stopped and the recording ended, its file whole — the app is
    /// quitting, or the feature went off.
    pub fn stop_all(&self) {
        self.stop();
        let _ = self.recorder.stop();
    }

    /// Feature `id` turned on or off. Meeting notes off stops its job and its
    /// recording: the page and its Stop are gone, and "Delete data" may follow.
    pub fn feature_set(&self, id: &str, on: bool) {
        if !on && Feature::from_id(id) == Some(Feature::MeetingNotes) {
            self.stop_all();
        }
    }

    /// The call recorded from now on — the microphone and what the computer
    /// plays — into `meetings/recordings/<id>.wav`; its id. Only while Meeting
    /// notes is on. How long and how loud it is goes to `sink` as it records,
    /// and an end without a Stop.
    pub fn record_start(&self, sink: &MeetingEventSink) -> Result<String, MeetingError> {
        still_on()?;
        let id = uuid::Uuid::new_v4().to_string();
        let (told, named) = (sink.clone(), id.clone());
        let levels = Box::new(move |l: recording::Levels| told(MeetingEvent::Recording { id: named.clone(), seconds: l.seconds, mic: l.mic, call: l.call }));
        let (told, named) = (sink.clone(), id.clone());
        let ended = Box::new(move |e: MeetingError| told(MeetingEvent::RecordingEnded { id: named, error: e.to_string() }));
        self.recorder.start(&id, &meeting_store::recording(&id)?, levels, ended)?;
        Ok(id)
    }

    /// The recording stopped and its file whole; its id.
    pub fn record_stop(&self) -> Result<String, MeetingError> {
        self.recorder.stop()
    }

    /// Your microphone muted in the recording, or heard again.
    pub fn record_mute(&self, on: bool) -> Result<(), MeetingError> {
        self.recorder.mute(on)
    }

    /// The call being recorded now, for a page opened while it records.
    pub fn recording(&self) -> Option<RecordingState> {
        self.recorder.current()
    }

    /// The calls recorded and not yet made into meetings, newest first. One a
    /// crash left without its sizes gets them here: the list is where it is
    /// next seen, and it reads as nothing without them.
    pub fn recordings(&self) -> Result<Vec<RecordingInfo>, MeetingError> {
        let mut rows: Vec<RecordingInfo> = meeting_store::recordings()?
            .into_iter()
            .filter(|(id, _)| !self.recorder.holds(id))
            .filter_map(|(id, path)| {
                // ponytail: a file that is not a WAV of ours is left out, not removed; Delete data takes it.
                meeting_recorder::repair(&path).ok()?;
                let meta = std::fs::metadata(&path).ok()?;
                let created = chrono::DateTime::<chrono::Utc>::from(began(&meta)?).to_rfc3339();
                Some(RecordingInfo { id, created, seconds: length(&meta) })
            })
            .collect();
        rows.sort_by(|a, b| b.created.cmp(&a.created));
        Ok(rows)
    }

    /// The call recorded as `id` made into a meeting, like an imported file;
    /// the recording goes once the meeting is saved — they are one call.
    pub fn process_recording(&self, exe: &Path, models: &Path, id: &str, sink: &MeetingEventSink) -> Result<String, MeetingError> {
        if self.recorder.holds(id) {
            return Err(MeetingError::StillRecording);
        }
        let path = meeting_store::recording(id)?;
        let began = std::fs::metadata(&path).ok().and_then(|m| began(&m)).ok_or_else(|| MeetingError::NotFound(id.to_string()))?;
        let name = chrono::DateTime::<chrono::Local>::from(began).format("Call %Y-%m-%d %H:%M").to_string();
        let sound = Sound::Call { denoise_mic: settings_store::load().unwrap_or_default().meeting_denoise() };
        self.process_as(exe, models, Audio { path: &path, name, sound, began: Some(began) }, sink, &|| {
            let _ = meeting_store::delete_recording(id);
        })
    }

    /// The recording `id` thrown away — not the one being recorded.
    pub fn delete_recording(&self, id: &str) -> Result<(), MeetingError> {
        if self.recorder.holds(id) {
            return Err(MeetingError::StillRecording);
        }
        meeting_store::delete_recording(id)
    }

    /// `audio` recognized by `exe`, saved, then summarized; the meeting's id.
    /// The meeting keeps its own copy of the sound, so `audio` — an imported
    /// file, or a recording to be thrown away — is not needed after. A summary
    /// that fails leaves the meeting saved, to summarize again; a recognition
    /// that fails leaves nothing.
    pub fn process(&self, exe: &Path, models: &Path, audio: &Path, sink: &MeetingEventSink) -> Result<String, MeetingError> {
        let name = audio.file_name().map_or_else(|| audio.display().to_string(), |n| n.to_string_lossy().into_owned());
        // An import has no microphone of ours: whatever it holds is one sound.
        self.process_as(exe, models, Audio { path: audio, name, sound: Sound::Import, began: None }, sink, &|| {})
    }

    /// [`process`](Self::process) of `audio`, and `saved` called once the
    /// meeting is saved — before its notes, which may fail.
    fn process_as(&self, exe: &Path, models: &Path, audio: Audio, sink: &MeetingEventSink, saved: &dyn Fn()) -> Result<String, MeetingError> {
        let job = self.begin(JobState::new(JobKind::Process, None), sink)?;
        if !speech::has_models(models) {
            return Err(MeetingError::ModelsMissing);
        }
        // With the slot held no other is being recognized: a folder without its
        // meeting is one the app quit or crashed in the middle of.
        for left in meeting_store::unfinished()? {
            let _ = remove_meeting(&left);
        }
        let id = uuid::Uuid::new_v4().to_string();
        let meeting = match self.transcribe(exe, models, audio, &id, &|e| job.tell(e)) {
            Ok(meeting) => meeting,
            Err(e) => {
                // The copy of the sound, written before the speech was found.
                let _ = remove_meeting(&id);
                return Err(e);
            }
        };
        saved();
        job.tell(MeetingEvent::Transcribed { id: id.clone() });
        self.summarize_into(&meeting, &|e| job.tell(e))?;
        Ok(id)
    }

    fn transcribe(&self, exe: &Path, models: &Path, audio: Audio, id: &str, sink: &dyn Fn(MeetingEvent)) -> Result<Meeting, MeetingError> {
        let keep = meeting_store::audio(id)?;
        let Audio { path, name, sound, began } = audio;
        let recognized = meeting_process::recognize(&self.running, exe, path, models, &keep, sound, &mut |stage, done, total| {
            sink(MeetingEvent::Progress { id: id.to_string(), name: name.clone(), stage, done, total })
        })?;
        if recognized.utterances.is_empty() {
            return Err(MeetingError::NoSpeech);
        }
        let mut meeting = Meeting {
            id: id.to_string(),
            name,
            created: began.map_or_else(chrono::Utc::now, chrono::DateTime::<chrono::Utc>::from).to_rfc3339(),
            seconds: recognized.seconds,
            utterances: recognized.utterances,
            summary: None,
            names: Default::default(),
            guessed: Vec::new(),
            title: None,
            title_by_user: false,
            topics: Vec::new(),
            documents: Vec::new(),
        };
        if remembering() {
            let speech = meeting.speech();
            let known = voices_store::update(true, |book| {
                book.hear(id, &recognized.voices, &speech);
                book.recognize(id)
            })?;
            meeting.guess(known.unwrap_or_default());
        }
        still_on()?;
        meeting_store::save(&meeting)?;
        Ok(meeting)
    }

    /// The summary of meeting `id` written again.
    pub fn summarize(&self, id: &str, sink: &MeetingEventSink) -> Result<(), MeetingError> {
        let job = self.begin(JobState::new(JobKind::Summary, Some(id.to_string())), sink)?;
        self.summarize_into(&meeting_store::get(id)?, &|e| job.tell(e))
    }

    fn summarize_into(&self, meeting: &Meeting, sink: &dyn Fn(MeetingEvent)) -> Result<(), MeetingError> {
        let session = llm_session::resolve(None).map_err(|e| MeetingError::Summary(e.to_string()))?;
        let summary = write_summary(&session, meeting, sink, &|| self.running.stopped())?;
        // Read again: a speaker renamed while the notes were written keeps the name.
        // ponytail: read, change, write without a lock; one per meeting if two writers ever race.
        let mut now = meeting_store::get(&meeting.id)?;
        now.take_notes(&summary, meeting);
        still_on()?;
        meeting_store::save(&now)
    }

    /// A document on `topic` written from meeting `id`, in place of the one it had.
    pub fn write_document(&self, id: &str, topic: &str, sink: &MeetingEventSink) -> Result<(), MeetingError> {
        let topic = meeting::clean_topic(topic)?;
        let state = JobState { topic: Some(topic.clone()), ..JobState::new(JobKind::Document, Some(id.to_string())) };
        let job = self.begin(state, sink)?;
        let session = llm_session::resolve(None).map_err(|e| MeetingError::Summary(e.to_string()))?;
        let meeting = meeting_store::get(id)?;
        let text = write_document(&session, &meeting, &topic, &|e| job.tell(e), &|| self.running.stopped())?;
        // Read again, as the notes are: a speaker renamed meanwhile keeps the name.
        let mut now = meeting_store::get(id)?;
        now.take_document(&topic, &text, &meeting);
        still_on()?;
        meeting_store::save(&now)
    }
}

/// Meeting `id` without its document on `topic`; the meeting after.
pub fn delete_document(id: &str, topic: &str) -> Result<Meeting, MeetingError> {
    let mut meeting = meeting_store::get(id)?;
    meeting.documents.retain(|d| d.topic != topic);
    meeting_store::save(&meeting)?;
    Ok(meeting)
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

/// Meeting `id` called `title` by the user: notes written again keep it
/// (`docs/38-artifacts.md`). The title comes checked (`domain::artifacts::title`).
pub fn rename_meeting(id: &str, title: &str) -> Result<Meeting, MeetingError> {
    let mut meeting = meeting_store::get(id)?;
    meeting.title = Some(title.to_string());
    meeting.title_by_user = true;
    meeting_store::save(&meeting)?;
    Ok(meeting)
}

/// Meeting `id` gone — its recording too — and what the voice memory heard in it, on or off.
fn remove_meeting(id: &str) -> Result<(), MeetingError> {
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

/// A job keeps nothing once the feature is off: its "Delete data" may have run.
fn still_on() -> Result<(), MeetingError> {
    if features::enabled().is_on(Feature::MeetingNotes) { Ok(()) } else { Err(MeetingError::Off) }
}

/// The notes of `meeting`, streamed to `sink` as they are written.
fn write_summary(session: &LlmSession, meeting: &Meeting, sink: &dyn Fn(MeetingEvent), cancelled: &dyn Fn() -> bool) -> Result<String, MeetingError> {
    let asked = Asked { part: meeting::PART_INSTRUCTIONS.to_string(), preface: meeting::PARTS_PREFACE, whole: meeting::summary_instructions(session.reply_language) };
    let text = |text: &str| sink(MeetingEvent::SummaryDelta { id: meeting.id.clone(), text: text.to_string() });
    write_over(session, meeting, &asked, sink, &text, cancelled)
}

/// The document on `topic` from `meeting`, streamed to `sink` as it is written.
fn write_document(session: &LlmSession, meeting: &Meeting, topic: &str, sink: &dyn Fn(MeetingEvent), cancelled: &dyn Fn() -> bool) -> Result<String, MeetingError> {
    let asked = Asked {
        part: meeting::document_part_instructions(topic),
        preface: meeting::DOCUMENT_PARTS_PREFACE,
        whole: meeting::document_instructions(topic, session.reply_language),
    };
    let text = |text: &str| sink(MeetingEvent::DocumentDelta { id: meeting.id.clone(), topic: topic.to_string(), text: text.to_string() });
    write_over(session, meeting, &asked, sink, &text, cancelled)
}

/// What the model is told over a whole meeting: for each part of one too long
/// to read at once, before what the parts gave, and for the whole.
struct Asked<'a> {
    part: String,
    preface: &'a str,
    whole: String,
}

/// What `asked` makes of `meeting`, its pieces to `text` as they are written. A
/// meeting too long for one request is read in parts: each part as `asked.part`
/// says, then the whole from those. A stopped or empty answer is nothing written.
fn write_over(
    session: &LlmSession,
    meeting: &Meeting,
    asked: &Asked,
    sink: &dyn Fn(MeetingEvent),
    text: &dyn Fn(&str),
    cancelled: &dyn Fn() -> bool,
) -> Result<String, MeetingError> {
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
                messages: vec![LlmMessage::system(&asked.part), LlmMessage::user(part.clone())],
                tools: Vec::new(),
                model: session.model.clone(),
            };
            // Streamed only to be stoppable mid-part; its pieces are not the page's.
            let part_notes = session.provider.chat_stream(request, &|_| {}, cancelled).map_err(|e| MeetingError::Summary(e.to_string()))?;
            if cancelled() {
                return Err(MeetingError::Cancelled);
            }
            notes.push(format!("### Part {} of {}\n\n{}", i + 1, parts.len(), part_notes.text.trim()));
        }
        // Every part read: what follows is the notes of the whole.
        read(parts.len());
        format!("{}\n\n{}", asked.preface, notes.join("\n\n"))
    };
    meeting::fits(&input, session.context_limit)?;
    let request = ChatRequest {
        messages: vec![LlmMessage::system(&asked.whole), LlmMessage::user(input)],
        tools: Vec::new(),
        model: session.model.clone(),
    };
    let on_event = |event: StreamEvent| {
        if let StreamEvent::Text(piece) = event {
            text(piece);
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
        /// Whether each request was stopped, as the provider was told.
        stopped: Mutex<Vec<bool>>,
    }

    impl LlmProvider for Notes {
        fn chat(&self, _: ChatRequest) -> Result<ChatResponse, LlmError> {
            unreachable!("every request streams, to be stoppable")
        }

        /// A part's notes: which part it was, by its first line's time. The
        /// notes of the whole: `chunks`.
        fn chat_stream(&self, request: ChatRequest, on_event: &dyn Fn(StreamEvent), cancel: &dyn Fn() -> bool) -> Result<ChatStreamResult, LlmError> {
            self.stopped.lock().unwrap().push(cancel());
            // The notes' and a document's both say so.
            let part = request.messages[0].content.as_deref().is_some_and(|c| c.contains("one part of a long meeting's transcript"));
            let text = request.messages[1].content.clone().unwrap_or_default();
            self.asked.lock().unwrap().push(request);
            if part {
                let notes = format!("notes from {}", &text[..7]);
                on_event(StreamEvent::Text(&notes));
                return Ok(ChatStreamResult { text: notes, ..Default::default() });
            }
            self.chunks.iter().for_each(|c| on_event(StreamEvent::Text(c)));
            Ok(ChatStreamResult { text: self.chunks.concat(), ..Default::default() })
        }

        fn list_models(&self) -> Result<Vec<LlmModelInfo>, LlmError> {
            unreachable!()
        }
    }

    fn session(chunks: Vec<&'static str>, context_limit: Option<u32>) -> (LlmSession, Arc<Notes>) {
        let provider = Arc::new(Notes { chunks, asked: Mutex::new(Vec::new()), stopped: Mutex::new(Vec::new()) });
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

    /// Not while a job works on it — the job would write into a meeting that
    /// is no more; another meeting's job is no reason, and neither is none.
    #[test]
    fn a_meeting_a_job_works_on_is_not_deleted() {
        crate::testing::with_app_dir("meeting-delete-in-use", || {
            meeting_store::save(&Meeting { id: "ab".into(), ..meeting() }).unwrap();
            meeting_store::save(&Meeting { id: "cd".into(), ..meeting() }).unwrap();
            let notes = MeetingNotes::default();
            *notes.slot() = Some(JobState::new(JobKind::Summary, Some("ab".into())));
            assert_eq!(notes.delete_meeting("ab"), Err(MeetingError::InUse));
            assert!(meeting_store::get("ab").is_ok(), "still there");
            notes.delete_meeting("cd").unwrap();
            *notes.slot() = Some(JobState::new(JobKind::Process, None));
            notes.delete_meeting("ab").unwrap();
            assert_eq!(meeting_store::get("ab"), Err(MeetingError::NotFound("ab".into())));
        });
    }

    #[test]
    fn a_meeting_renamed_keeps_its_title_through_new_notes() {
        crate::testing::with_app_dir("meeting-rename", || {
            meeting_store::save(&Meeting { id: "ab".into(), title: Some("Выпуск".into()), ..meeting() }).unwrap();
            let renamed = rename_meeting("ab", "Планёрка").unwrap();
            assert_eq!((renamed.title.as_deref(), renamed.title_by_user), (Some("Планёрка"), true));
            assert_eq!(meeting_store::get("ab").unwrap(), renamed, "saved");
            assert_eq!(rename_meeting("zz", "x"), Err(MeetingError::NotFound("zz".into())));
        });
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
            title_by_user: false,
            topics: Vec::new(),
            documents: Vec::new(),
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
        let notes = write_summary(&session, &meeting(), &*sink, &|| false).unwrap();
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
        assert_eq!(write_summary(&reading, &long, &*sink, &|| false).unwrap(), "## Итоги");

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
        assert_eq!(write_summary(&stopped, &long, &*quiet, &|| true), Err(MeetingError::Cancelled));
        assert_eq!(provider.asked.lock().unwrap().len(), 1);
        assert_eq!(*provider.stopped.lock().unwrap(), [true], "a part's request is told of the stop as it runs");

        assert!(!events.lock().unwrap().iter().any(|e| matches!(e, MeetingEvent::SummaryDelta { text, .. } if text.starts_with("notes from"))), "a part's notes do not stream to the page");
        let progress: Vec<_> = events.lock().unwrap().iter().filter_map(|e| match e {
            MeetingEvent::Progress { stage: Stage::Notes, done, total, .. } => Some((*done, *total)),
            _ => None,
        }).collect();
        assert_eq!(progress, (0..=parts).map(|i| (i, parts)).collect::<Vec<_>>());
    }

    #[test]
    fn a_document_is_written_on_its_subject_from_the_meeting_or_from_its_parts() {
        let (short, provider) = session(vec!["## Как\n", "Так."], None);
        let (sink, events) = recorder();
        assert_eq!(write_document(&short, &meeting(), "Выпуск", &*sink, &|| false).unwrap(), "## Как\nТак.");
        let asked = provider.asked.lock().unwrap();
        assert_eq!(asked[0].messages[0], LlmMessage::system(meeting::document_instructions("Выпуск", Some("Russian"))));
        assert_eq!(asked[0].messages[1], LlmMessage::user("[00:01] Speaker 1: Решили выпустить в пятницу.\n"));
        assert_eq!(
            *events.lock().unwrap(),
            [
                MeetingEvent::DocumentDelta { id: "m1".into(), topic: "Выпуск".into(), text: "## Как\n".into() },
                MeetingEvent::DocumentDelta { id: "m1".into(), topic: "Выпуск".into(), text: "Так.".into() },
            ],
            "its pieces are the document's, not the notes'"
        );
        drop(asked);

        let mut long = meeting();
        long.utterances = (0..12).map(|i| Utterance { speaker: 0, start: 60.0 * i as f32, end: 1.0, text: "Решили выпустить в пятницу.".into() }).collect();
        let (reading, provider) = session(vec!["## Как"], Some(150));
        let (sink, _) = recorder();
        write_document(&reading, &long, "Выпуск", &*sink, &|| false).unwrap();
        let asked = provider.asked.lock().unwrap();
        let parts = asked.len() - 1;
        assert!(parts >= 2, "{parts}");
        assert!(asked[..parts].iter().all(|r| r.messages[0] == LlmMessage::system(meeting::document_part_instructions("Выпуск"))));
        let whole = asked[parts].messages[1].content.clone().unwrap();
        assert!(whole.starts_with(meeting::DOCUMENT_PARTS_PREFACE), "{whole}");
        assert_eq!(asked[parts].messages[0], LlmMessage::system(meeting::document_instructions("Выпуск", Some("Russian"))));
    }

    #[test]
    fn a_deleted_document_is_gone_from_the_saved_meeting_and_the_others_stay() {
        crate::testing::with_app_dir("meeting-documents", || {
            let docs = ["Выпуск", "Лицензии"].map(|t| meeting::MeetingDocument { topic: t.into(), text: "x".into() }).to_vec();
            meeting_store::save(&Meeting { id: "ab".into(), documents: docs, ..meeting() }).unwrap();
            let after = delete_document("ab", "Выпуск").unwrap();
            assert_eq!(meeting_store::get("ab").unwrap(), after);
            assert_eq!(after.documents.iter().map(|d| d.topic.as_str()).collect::<Vec<_>>(), ["Лицензии"]);
        });
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

            MeetingNotes::default().delete_meeting("a1").unwrap();
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
        assert_eq!(write_summary(&stopped, &meeting(), &*sink, &|| true), Err(MeetingError::Cancelled));
        let (empty, _) = session(vec!["  \n"], None);
        assert!(matches!(write_summary(&empty, &meeting(), &*sink, &|| false), Err(MeetingError::Summary(_))));
        // A window too small even for the parts' notes.
        let (small, provider) = session(vec!["x"], Some(10));
        assert!(matches!(write_summary(&small, &meeting(), &*sink, &|| false), Err(MeetingError::TooLong { .. })));
        assert!(provider.asked.lock().unwrap().iter().all(|r| r.messages[0] == LlmMessage::system(meeting::PART_INSTRUCTIONS)));
    }

    #[test]
    fn one_job_at_a_time_and_the_slot_frees_when_it_ends() {
        let (sink, events) = recorder();
        let notes = MeetingNotes::default();
        notes.stop();
        let job = notes.begin(JobState::new(JobKind::Summary, Some("m".into())), &sink).unwrap();
        assert!(!notes.running.stopped(), "a stop before the job is not its stop");
        assert!(matches!(notes.begin(JobState::new(JobKind::Process, None), &sink), Err(MeetingError::Busy)));
        job.tell(MeetingEvent::Progress { id: "m".into(), name: "call.wav".into(), stage: Stage::Notes, done: 1, total: 3 });
        assert_eq!(notes.current().map(|j| (j.kind, j.done, j.total)), Some((JobKind::Summary, 1, 3)), "a page opened now sees it");
        drop(job);
        assert_eq!(notes.current(), None);
        assert_eq!(events.lock().unwrap().last(), Some(&MeetingEvent::Ended), "the page is told it ended");
        assert!(notes.begin(JobState::new(JobKind::Process, None), &sink).is_ok());
    }

    #[test]
    fn a_call_is_recorded_only_while_meeting_notes_is_on() {
        crate::testing::with_app_dir("meeting-record-off", || {
            let notes = MeetingNotes::default();
            assert_eq!(notes.record_start(&recorder().0), Err(MeetingError::Off));
            assert!(!meeting_store::audio("0").unwrap().parent().unwrap().parent().unwrap().join("recordings").exists(), "no folder made for it");
            assert_eq!(notes.record_stop(), Err(MeetingError::NotRecording));
            notes.stop_all(); // nothing to stop is no failure
        });
    }

    #[test]
    fn turning_meeting_notes_off_stops_its_job() {
        let notes = MeetingNotes::default();
        for (id, on) in [("meeting-notes", true), ("voice-memory", false), ("board", false)] {
            notes.feature_set(id, on);
            assert!(!notes.running.stopped(), "{id} {on}");
        }
        notes.feature_set("meeting-notes", false);
        assert!(notes.running.stopped());
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
            features::set("meeting-notes", true).unwrap();
            // A meeting the app quit in the middle of: its sound and no meeting.
            let quit = meeting_store::audio("0dd").unwrap();
            std::fs::create_dir_all(quit.parent().unwrap()).unwrap();
            std::fs::write(&quit, "RIFF").unwrap();
            let (sink, events) = recorder();
            let (exe, models) = sidecar("meeting-process-ok", r#"{"type":"result","utterances":[{"speaker":0,"start":0.0,"end":1.0,"text":"да"}],"seconds":1.0}"#);
            // No model is set up here, so the notes fail — after the meeting is saved.
            assert!(matches!(MeetingNotes::default().process(&exe, &models, Path::new("/calls/call.m4a"), &sink), Err(MeetingError::Summary(_))));
            // Cloned out: the lock is let go before the next job's events.
            let id = &match &events.lock().unwrap()[..] {
                [MeetingEvent::Transcribed { id }, MeetingEvent::Ended] => id.clone(),
                other => panic!("{other:?}"),
            };
            assert!(!quit.parent().unwrap().exists(), "the next import clears what the last one left");
            let saved = meeting_store::get(id).unwrap();
            assert_eq!((saved.name.as_str(), saved.seconds), ("call.m4a", 1.0));
            assert_eq!(std::fs::read(meeting_store::audio(id).unwrap()).unwrap(), b"RIFF");

            for (label, line) in [("meeting-process-error", r#"{"type":"error","message":"no"}"#), ("meeting-process-silent", r#"{"type":"result","utterances":[],"seconds":1.0}"#)] {
                let (exe, models) = sidecar(label, line);
                assert!(MeetingNotes::default().process(&exe, &models, Path::new("a.wav"), &sink).is_err());
            }
            let left: Vec<_> = std::fs::read_dir(meeting_store::audio(id).unwrap().parent().unwrap().parent().unwrap()).unwrap().collect();
            assert_eq!(left.len(), 1, "only the recognized one");

            // Turned off mid-job: its "Delete data" may have run, so nothing is kept.
            features::set("meeting-notes", false).unwrap();
            let (exe, models) = sidecar("meeting-process-off", r#"{"type":"result","utterances":[{"speaker":0,"start":0.0,"end":1.0,"text":"да"}],"seconds":1.0}"#);
            assert_eq!(MeetingNotes::default().process(&exe, &models, Path::new("a.wav"), &sink), Err(MeetingError::Off));
            assert_eq!(meeting_store::list().unwrap().len(), 1);
            let left: Vec<_> = std::fs::read_dir(meeting_store::audio(id).unwrap().parent().unwrap().parent().unwrap()).unwrap().collect();
            assert_eq!(left.len(), 1, "not even its sound");
        });
    }

    /// A recording as the recorder leaves one, `frames` long; or as a crash
    /// does, its sizes 0.
    fn recorded(id: &str, frames: usize, whole: bool) -> std::path::PathBuf {
        let path = meeting_store::recording(id).unwrap();
        let data = (frames * 4) as u32;
        let mut bytes = recording::wav_header(recording::CHANNELS, if whole { data } else { 0 }).to_vec();
        bytes.extend(recording::interleave(&vec![0.1; frames], &vec![0.2; frames], frames));
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn recordings_are_listed_newest_first_with_a_crashed_one_made_whole_and_the_one_recording_left_out() {
        crate::testing::with_app_dir("meeting-recordings", || {
            let notes = MeetingNotes::default();
            assert_eq!(notes.recordings().unwrap(), [], "none recorded yet");
            recorded("aa", 16_000, true);
            std::thread::sleep(std::time::Duration::from_millis(20));
            let crashed = recorded("bb", 32_000, false);
            recorded("cc", 8_000, true);
            notes.recorder.pretend("cc");
            assert_eq!(notes.recording().map(|r| r.id), Some("cc".into()));

            let rows = notes.recordings().unwrap();
            // bb stopped last but is a second longer: it began first, and the newest begun leads.
            assert_eq!(rows.iter().map(|r| (r.id.as_str(), r.seconds)).collect::<Vec<_>>(), [("aa", 1.0), ("bb", 2.0)]);
            let stopped = |id: &str| chrono::DateTime::<chrono::Utc>::from(std::fs::metadata(meeting_store::recording(id).unwrap()).unwrap().modified().unwrap());
            assert_eq!(rows[1].created, (stopped("bb") - chrono::Duration::seconds(2)).to_rfc3339(), "when it began, not when it stopped");
            assert_eq!(std::fs::read(&crashed).unwrap()[40..44], (32_000u32 * 4).to_le_bytes(), "its sizes put in");

            assert_eq!(notes.delete_recording("cc"), Err(MeetingError::StillRecording));
            assert_eq!(notes.process_recording(Path::new("x"), Path::new("y"), "cc", &recorder().0), Err(MeetingError::StillRecording));
            notes.delete_recording("aa").unwrap();
            assert_eq!(notes.recordings().unwrap().len(), 1);
        });
    }

    #[cfg(unix)]
    #[test]
    fn a_recording_made_into_a_meeting_is_gone_and_one_that_failed_stays() {
        crate::testing::with_app_dir("meeting-process-recording", || {
            features::set("meeting-notes", true).unwrap();
            let notes = MeetingNotes::default();
            let (sink, _) = recorder();
            let (failing, models) = sidecar("recording-fails", r#"{"type":"error","message":"no"}"#);
            use chrono::TimeZone;
            // Two minutes, stopped at 14:30: the meeting is named by its start.
            let kept = recorded("aa", 16_000 * 120, true);
            let stopped = chrono::Local.with_ymd_and_hms(2026, 10, 21, 14, 30, 0).unwrap();
            std::fs::File::options().write(true).open(&kept).unwrap().set_modified(stopped.into()).unwrap();
            assert!(notes.process_recording(&failing, &models, "aa", &sink).is_err());
            assert!(kept.exists(), "a recognition that failed leaves the call to try again");

            let (exe, models) = sidecar("recording-ok", r#"{"type":"result","utterances":[{"speaker":0,"start":0.0,"end":1.0,"text":"да"}],"seconds":1.0}"#);
            // No model is set up here, so the notes fail — after the meeting is saved.
            assert!(matches!(notes.process_recording(&exe, &models, "aa", &sink), Err(MeetingError::Summary(_))));
            assert!(!kept.exists(), "the meeting is the call now");
            let meetings = meeting_store::list().unwrap();
            assert_eq!(meetings.len(), 1);
            assert_eq!(meetings[0].name, "Call 2026-10-21 14:28");
            assert_eq!(meetings[0].created, (stopped - chrono::Duration::minutes(2)).with_timezone(&chrono::Utc).to_rfc3339(), "dated when the call began, not when recognized");
            assert_eq!(notes.process_recording(&exe, &models, "aa", &sink), Err(MeetingError::NotFound("aa".into())));
        });
    }

    /// A stand-in for the exe that says what it was told the sound is, as the
    /// meeting's one word.
    #[cfg(unix)]
    fn telling_sidecar(label: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let said = |word: &str| format!(r#"{{"type":"result","utterances":[{{"speaker":0,"start":0.0,"end":1.0,"text":"{word}"}}],"seconds":1.0}}"#);
        let (exe, models) = sidecar(label, "");
        let script = std::fs::read_to_string(&exe).unwrap().replace(
            "echo ''\n",
            &format!(
                "case \"$5\" in {}) echo '{}';; {}) echo '{}';; *) echo '{}';; esac\n",
                meeting::DENOISED_CALL_FLAG,
                said("denoised"),
                meeting::CALL_FLAG,
                said("call"),
                said("as-is")
            ),
        );
        std::fs::write(&exe, script).unwrap();
        (exe, models)
    }

    #[cfg(unix)]
    #[test]
    fn a_recordings_microphone_is_denoised_unless_turned_off_and_an_import_never() {
        crate::testing::with_app_dir("meeting-denoise", || {
            features::set("meeting-notes", true).unwrap();
            let (exe, models) = telling_sidecar("denoise-telling");
            let (sink, _) = recorder();
            let notes = MeetingNotes::default();
            let heard = || meeting_store::get(&meeting_store::list().unwrap()[0].id).unwrap().utterances[0].text.clone();

            recorded("aa", 16_000, true);
            let _ = notes.process_recording(&exe, &models, "aa", &sink);
            assert_eq!(heard(), "denoised", "on by default");
            notes.delete_meeting(&meeting_store::list().unwrap()[0].id).unwrap();

            settings_store::update(|s| s.meeting_denoise = Some(false)).unwrap();
            recorded("bb", 16_000, true);
            let _ = notes.process_recording(&exe, &models, "bb", &sink);
            assert_eq!(heard(), "call", "turned off: still a call of ours");
            notes.delete_meeting(&meeting_store::list().unwrap()[0].id).unwrap();

            settings_store::update(|s| s.meeting_denoise = Some(true)).unwrap();
            let _ = notes.process(&exe, &models, Path::new("/calls/call.m4a"), &sink);
            assert_eq!(heard(), "as-is", "an import has no microphone of ours");
        });
    }

    #[test]
    fn a_build_without_models_says_so_before_starting_anything() {
        let (sink, events) = recorder();
        let empty = crate::testing::temp_dir("meeting-no-models");
        let got = MeetingNotes::default().process(Path::new("/nonexistent"), &empty, Path::new("a.wav"), &sink);
        assert_eq!(got, Err(MeetingError::ModelsMissing));
        assert_eq!(*events.lock().unwrap(), [MeetingEvent::Ended]);
    }
}
