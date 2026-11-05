//! Dictation into the message box (`docs/52-voice-input.md`): what the window
//! is told, and how the app and the recognition process talk. The app writes
//! [`Frame`]s to the process's stdin, sound being too much for lines of JSON,
//! and the process answers with a [`DictationLine`] a line on stdout.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::domain::recording::RATE;

/// `<exe> --dictation-sidecar <models>` is the recognition process.
pub const SIDECAR_FLAG: &str = "--dictation-sidecar";

/// How often an open phrase is recognized again, counted from the end of the
/// last pass: a slow machine shows less often, it does not fall behind.
pub const PARTIAL_STEP: Duration = Duration::from_millis(500);
/// Less of a phrase than this, in samples, recognizes as noise.
const SHORTEST_PARTIAL: usize = RATE as usize / 2;

/// A frame's tag, then its data's length as a little-endian `u32`.
const HEADER_BYTES: usize = 5;
const TAG_AUDIO: u8 = 1;
const TAG_FINISH: u8 = 2;
const TAG_CANCEL: u8 = 3;
/// The most samples one [`Frame::Audio`] may carry, half a minute of sound. A
/// frame carries a tick's worth: past this a length is a stream out of step,
/// not to be believed.
const LONGEST_AUDIO: usize = 1 << 19;
const LONGEST_FRAME_BYTES: usize = LONGEST_AUDIO * 2;

/// What the message box hears of a dictation, on its channel.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum DictationEvent {
    /// The microphone is opening.
    Preparing,
    /// What is said from now on is kept: the sound that says "speak".
    Listening,
    /// How loud the last tick was, a peak for each of its quarters.
    Levels { peaks: [f32; 4] },
    /// The phrase being said, as far as it is recognized: it will change.
    Partial { text: String },
    /// A phrase ended, and this is its text.
    Phrase { text: String },
    Ended { reason: EndReason },
    Failed { message: String },
}

/// Why a dictation ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum EndReason {
    /// Stopped: what was said is in the box.
    Stopped,
    /// Thrown away: the box goes back to what it held.
    Cancelled,
    /// It ran as long as a dictation may, what was said kept.
    Limit,
}

/// Where a dictation's events go; `commands/` turns them into its channel.
pub type DictationEventSink = Arc<dyn Fn(DictationEvent) + Send + Sync>;

#[derive(Debug, Error, PartialEq)]
pub enum DictationError {
    #[error("the dictation stream has an unknown frame, tagged {0}")]
    UnknownFrame(u8),
    #[error("the dictation stream has a frame of {0} bytes, too long to be one")]
    FrameTooLong(usize),
    #[error("the dictation stream has sound of {0} bytes, half a sample short")]
    BrokenSound(usize),
    #[error("this build has no speech models in it — run `bun scripts/fetch-meeting-models.ts` and build again")]
    ModelsMissing,
    #[error("the {0} model did not load")]
    ModelFailed(&'static str),
}

/// What makes text of a phrase's sound.
pub trait PhraseRecognizer {
    fn recognize(&self, samples: &[f32]) -> String;
}

/// What finds where phrases begin and end in sound as it comes.
pub trait PhraseDetector {
    fn accept(&self, samples: &[f32]);
    /// Whether a phrase is open now.
    fn speaking(&self) -> bool;
    /// The oldest phrase ended and not yet taken, taken out.
    fn next_phrase(&self) -> Option<Vec<f32>>;
    /// What is open ends here, as a phrase.
    fn flush(&self);
    /// Everything heard is forgotten, phrases not yet taken too.
    fn reset(&self);
}

/// What the app tells the recognition process. Its stdin closing is the app gone.
#[derive(Debug, Clone, PartialEq)]
pub enum Frame {
    /// Sound at 16 kHz, mono.
    Audio(Vec<f32>),
    /// This dictation is over: recognize what is left and say `done`.
    Finish,
    /// This dictation is over: throw it away and say `done`.
    Cancel,
}

impl Frame {
    /// As it goes down the pipe. Sound goes as 16-bit samples, half of what
    /// the floats would take, which is all a microphone has anyway.
    pub fn encode(&self) -> Vec<u8> {
        let (tag, data) = match self {
            Frame::Audio(samples) => (TAG_AUDIO, samples.iter().flat_map(|s| to_sample(*s).to_le_bytes()).collect()),
            Frame::Finish => (TAG_FINISH, Vec::new()),
            Frame::Cancel => (TAG_CANCEL, Vec::new()),
        };
        let mut bytes = Vec::with_capacity(HEADER_BYTES + data.len());
        bytes.push(tag);
        bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&data);
        bytes
    }

    /// The first frame in `stream`, taken out of it. `None` while it is not
    /// all there yet. An error means the stream cannot be trusted from here on.
    pub fn take(stream: &mut Vec<u8>) -> Result<Option<Frame>, DictationError> {
        let Some(header) = stream.get(..HEADER_BYTES) else { return Ok(None) };
        let (tag, length) = (header[0], u32::from_le_bytes([header[1], header[2], header[3], header[4]]) as usize);
        if !matches!(tag, TAG_AUDIO | TAG_FINISH | TAG_CANCEL) {
            return Err(DictationError::UnknownFrame(tag));
        }
        // Only sound has data: anything else with a length is out of step too.
        if length > if tag == TAG_AUDIO { LONGEST_FRAME_BYTES } else { 0 } {
            return Err(DictationError::FrameTooLong(length));
        }
        if tag == TAG_AUDIO && length % 2 != 0 {
            return Err(DictationError::BrokenSound(length));
        }
        let Some(data) = stream.get(HEADER_BYTES..HEADER_BYTES + length) else { return Ok(None) };
        let frame = match tag {
            TAG_AUDIO => Frame::Audio(data.chunks_exact(2).map(|b| from_sample(i16::from_le_bytes([b[0], b[1]]))).collect()),
            TAG_FINISH => Frame::Finish,
            _ => Frame::Cancel,
        };
        stream.drain(..HEADER_BYTES + length);
        Ok(Some(frame))
    }
}

fn to_sample(s: f32) -> i16 {
    (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
}

fn from_sample(s: i16) -> f32 {
    s as f32 / i16::MAX as f32
}

/// What the recognition process writes to stdout, one JSON object a line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum DictationLine {
    /// The models are loaded.
    Ready,
    Partial { text: String },
    Phrase { text: String },
    /// A `finish` or `cancel` is through: no more of that dictation comes,
    /// and what follows is the next one's. The process lives between
    /// dictations, so this is the only line that tells them apart.
    Done,
    Error { message: String },
}

/// What of a recognized phrase goes in the box: nothing for a phrase of no
/// words, as noise comes out — a lone full stop is not worth a space before it.
pub fn spoken(recognized: &str) -> Option<&str> {
    let text = recognized.trim();
    text.chars().any(char::is_alphanumeric).then_some(text)
}

/// When to recognize the open phrase again. Times are since any fixed moment.
#[derive(Debug, Default)]
pub struct PartialPasses {
    ended: Option<Duration>,
    /// How much of the open phrase the last pass heard.
    heard: usize,
}

impl PartialPasses {
    /// Whether a pass is worth it at `now`, `open` samples into the phrase.
    pub fn due(&self, open: usize, now: Duration) -> bool {
        open >= SHORTEST_PARTIAL && open > self.heard && self.ended.is_none_or(|ended| now >= ended + PARTIAL_STEP)
    }

    /// A pass over `open` samples ended at `now`.
    pub fn passed(&mut self, open: usize, now: Duration) {
        (self.ended, self.heard) = (Some(now), open);
    }

    /// The open phrase ended: the next one starts from nothing heard.
    pub fn phrase_closed(&mut self) {
        self.heard = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn taken(mut stream: Vec<u8>) -> (Result<Option<Frame>, DictationError>, Vec<u8>) {
        (Frame::take(&mut stream), stream)
    }

    #[test]
    fn a_frame_comes_back_as_it_went() {
        let mut stream = [Frame::Audio(vec![0.0, 1.0, -1.0, 0.5]), Frame::Finish, Frame::Cancel].iter().flat_map(Frame::encode).collect();
        let Ok(Some(Frame::Audio(sound))) = Frame::take(&mut stream) else { panic!("sound first") };
        assert_eq!(sound.len(), 4);
        assert_eq!(&sound[..3], [0.0, 1.0, -1.0], "the ends of the range stay the ends");
        assert!((sound[3] - 0.5).abs() < 0.001, "what is between within a sample's step");
        assert_eq!(Frame::take(&mut stream), Ok(Some(Frame::Finish)));
        assert_eq!(Frame::take(&mut stream), Ok(Some(Frame::Cancel)));
        assert!(stream.is_empty(), "each taken out");
        assert_eq!(Frame::take(&mut stream), Ok(None));
    }

    #[test]
    fn sound_past_full_scale_stays_at_the_ends() {
        let (frame, _) = taken(Frame::Audio(vec![1.5, -1.5]).encode());
        assert_eq!(frame, Ok(Some(Frame::Audio(vec![1.0, -1.0]))));
    }

    #[test]
    fn a_frame_not_all_there_waits_and_loses_nothing() {
        let whole = Frame::Audio(vec![0.25; 8]).encode();
        for cut in [0, 1, 4, 5, whole.len() - 1] {
            let (frame, left) = taken(whole[..cut].to_vec());
            assert_eq!(frame, Ok(None), "{cut} bytes");
            assert_eq!(left, whole[..cut], "{cut} bytes stay for the rest");
        }
        assert!(matches!(taken(whole).0, Ok(Some(Frame::Audio(_)))));
    }

    #[test]
    fn the_most_sound_a_frame_may_carry_is_read() {
        let (frame, left) = taken(Frame::Audio(vec![0.5; LONGEST_AUDIO]).encode());
        assert!(matches!(frame, Ok(Some(Frame::Audio(sound))) if sound.len() == LONGEST_AUDIO));
        assert!(left.is_empty());
    }

    #[test]
    fn a_stream_out_of_step_is_refused_at_once() {
        let header = |tag: u8, length: u32| {
            let mut bytes = vec![tag];
            bytes.extend_from_slice(&length.to_le_bytes());
            bytes
        };
        assert_eq!(taken(header(9, 0)).0, Err(DictationError::UnknownFrame(9)));
        assert_eq!(taken(header(TAG_AUDIO, 3)).0, Err(DictationError::BrokenSound(3)), "before the rest of it comes");
        assert_eq!(taken(header(TAG_AUDIO, u32::MAX)).0, Err(DictationError::FrameTooLong(u32::MAX as usize)), "not waited for");
        let longest = LONGEST_FRAME_BYTES as u32;
        assert_eq!(taken(header(TAG_AUDIO, longest)).0, Ok(None), "the longest there may be is still a frame");
        assert_eq!(taken(header(TAG_AUDIO, longest + 2)).0, Err(DictationError::FrameTooLong(LONGEST_FRAME_BYTES + 2)));
        assert_eq!(taken(header(TAG_FINISH, 1)).0, Err(DictationError::FrameTooLong(1)), "only sound has data");
        assert_eq!(taken(header(TAG_CANCEL, 2)).0, Err(DictationError::FrameTooLong(2)));
    }

    #[test]
    fn the_window_reads_an_event_by_its_kind() {
        let json = |e: DictationEvent| serde_json::to_value(e).unwrap();
        assert_eq!(json(DictationEvent::Listening), serde_json::json!({ "kind": "listening" }));
        assert_eq!(json(DictationEvent::Partial { text: "да".into() }), serde_json::json!({ "kind": "partial", "text": "да" }));
        assert_eq!(json(DictationEvent::Levels { peaks: [0.5; 4] }), serde_json::json!({ "kind": "levels", "peaks": [0.5, 0.5, 0.5, 0.5] }));
        assert_eq!(json(DictationEvent::Ended { reason: EndReason::Cancelled }), serde_json::json!({ "kind": "ended", "reason": "cancelled" }));
        assert_eq!(json(DictationEvent::Failed { message: "нет".into() }), serde_json::json!({ "kind": "failed", "message": "нет" }));
    }

    #[test]
    fn a_line_of_the_process_reads_back() {
        for line in [
            DictationLine::Ready,
            DictationLine::Partial { text: "при".into() },
            DictationLine::Phrase { text: "Привет.".into() },
            DictationLine::Done,
            DictationLine::Error { message: "no VAD".into() },
        ] {
            let text = serde_json::to_string(&line).unwrap();
            assert!(!text.contains('\n'), "one line");
            assert_eq!(serde_json::from_str::<DictationLine>(&text).unwrap(), line);
        }
        assert_eq!(serde_json::to_string(&DictationLine::Ready).unwrap(), r#"{"type":"ready"}"#);
    }

    #[test]
    fn a_phrase_is_its_words() {
        assert_eq!(spoken("  Привет, мир.\n"), Some("Привет, мир."), "trimmed");
        assert_eq!(spoken("42"), Some("42"), "a number is said too");
        assert_eq!(spoken(""), None);
        assert_eq!(spoken("  "), None);
        assert_eq!(spoken(" ... ?"), None, "punctuation alone is noise");
    }

    #[test]
    fn a_pass_waits_for_the_last_one_and_for_something_new() {
        let at = Duration::from_millis;
        let mut passes = PartialPasses::default();
        assert!(!passes.due(SHORTEST_PARTIAL - 1, at(0)), "too little to read");
        assert!(passes.due(SHORTEST_PARTIAL, at(0)), "the first as soon as there is enough");

        // A slow pass: the step counts from its end, so the next one does not follow at once.
        passes.passed(SHORTEST_PARTIAL, at(2000));
        assert!(!passes.due(40_000, at(2499)));
        assert!(passes.due(40_000, at(2500)));
        assert!(!passes.due(SHORTEST_PARTIAL, at(9000)), "nothing new since: the same text again");
    }

    #[test]
    fn a_phrase_closed_leaves_the_next_one_nothing_heard() {
        let at = Duration::from_millis;
        let mut passes = PartialPasses::default();
        passes.passed(40_000, at(1000));
        assert!(!passes.due(SHORTEST_PARTIAL, at(5000)), "shorter than what was heard");
        passes.phrase_closed();
        assert!(passes.due(SHORTEST_PARTIAL, at(5000)));
        assert!(!passes.due(SHORTEST_PARTIAL, at(1499)), "the step still holds across phrases");
    }
}
