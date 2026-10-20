//! Meeting notes: a recording through the recognition process into a saved
//! transcript, and the transcript through the model into its summary
//! (`docs/36-meeting-notes-research.md`). One job at a time — the recognition
//! takes the cores, and the page has one Stop.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::domain::llm::{ChatRequest, LlmMessage, StreamEvent};
use crate::domain::meeting::{self, Meeting, MeetingError, MeetingEvent, MeetingEventSink};
use crate::infra::meeting_process::{self, Running};
use crate::infra::{meeting_store, speech};
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
    /// A summary that fails leaves the transcript saved, to summarize again.
    pub fn process(&self, exe: &Path, models: &Path, audio: &Path, sink: &MeetingEventSink) -> Result<String, MeetingError> {
        let _job = self.begin()?;
        if !speech::has_models(models) {
            return Err(MeetingError::ModelsMissing);
        }
        let id = uuid::Uuid::new_v4().to_string();
        let name = audio.file_name().map_or_else(|| audio.display().to_string(), |n| n.to_string_lossy().into_owned());
        let (utterances, seconds) = meeting_process::recognize(&self.running, exe, audio, models, &mut |stage, done, total| {
            sink(MeetingEvent::Progress { id: id.clone(), name: name.clone(), stage, done, total })
        })?;
        if utterances.is_empty() {
            return Err(MeetingError::NoSpeech);
        }
        let mut meeting = Meeting { id: id.clone(), name, created: chrono::Utc::now().to_rfc3339(), seconds, utterances, summary: None };
        meeting_store::save(&meeting)?;
        sink(MeetingEvent::Transcribed { id: id.clone() });
        self.summarize_into(&mut meeting, sink)?;
        Ok(id)
    }

    /// The summary of meeting `id` written again.
    pub fn summarize(&self, id: &str, sink: &MeetingEventSink) -> Result<(), MeetingError> {
        let _job = self.begin()?;
        self.summarize_into(&mut meeting_store::get(id)?, sink)
    }

    fn summarize_into(&self, meeting: &mut Meeting, sink: &MeetingEventSink) -> Result<(), MeetingError> {
        let session = llm_session::resolve(None).map_err(|e| MeetingError::Summary(e.to_string()))?;
        let summary = write_summary(&session, meeting, sink, &|| self.stopped.load(Ordering::SeqCst))?;
        meeting.summary = Some(summary);
        meeting_store::save(meeting)
    }
}

/// The notes of `meeting`, streamed to `sink` as they are written. A stopped
/// or empty answer is no summary: the one saved before stays.
fn write_summary(session: &LlmSession, meeting: &Meeting, sink: &MeetingEventSink, cancelled: &dyn Fn() -> bool) -> Result<String, MeetingError> {
    let transcript = meeting::render_transcript(&meeting.utterances);
    meeting::fits(&transcript, session.context_limit)?;
    let request = ChatRequest {
        messages: vec![LlmMessage::system(meeting::summary_instructions(session.reply_language)), LlmMessage::user(transcript)],
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
        fn chat(&self, _: ChatRequest) -> Result<ChatResponse, LlmError> {
            unreachable!("the summary streams")
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
    fn a_stopped_empty_or_too_long_summary_is_not_one() {
        let (sink, _) = recorder();
        let (stopped, _) = session(vec!["## Ре"], None);
        assert_eq!(write_summary(&stopped, &meeting(), &sink, &|| true), Err(MeetingError::Cancelled));
        let (empty, _) = session(vec!["  \n"], None);
        assert!(matches!(write_summary(&empty, &meeting(), &sink, &|| false), Err(MeetingError::Summary(_))));
        let (small, provider) = session(vec!["x"], Some(10));
        assert!(matches!(write_summary(&small, &meeting(), &sink, &|| false), Err(MeetingError::TooLong { .. })));
        assert!(provider.asked.lock().unwrap().is_empty(), "too long is known before asking");
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

    #[test]
    fn a_build_without_models_says_so_before_starting_anything() {
        let (sink, events) = recorder();
        let empty = crate::testing::temp_dir("meeting-no-models");
        let got = MeetingNotes::default().process(Path::new("/nonexistent"), &empty, Path::new("a.wav"), &sink);
        assert_eq!(got, Err(MeetingError::ModelsMissing));
        assert!(events.lock().unwrap().is_empty());
    }
}
