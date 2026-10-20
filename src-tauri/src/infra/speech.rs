//! A recording into utterances: symphonia reads the sound, GigaAM v3 (through
//! sherpa-onnx) recognizes it, campplus embeds the voice, and
//! `domain::meeting` does the rest. Runs only inside the recognition process
//! (`meeting_sidecar`) — a native crash here must not take the window with it.

use std::fs::File;
use std::path::{Path, PathBuf};

use sherpa_onnx::{LinearResampler, OfflineRecognizer, OfflineRecognizerConfig, SpeakerEmbeddingExtractor, SpeakerEmbeddingExtractorConfig};
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as AudioError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

use crate::domain::meeting::{self, MeetingError, Segment, Stage, Utterance, Word};

const SAMPLE_RATE: u32 = 16_000;
/// The whole recording goes through GigaAM in windows this long, each starting
/// [`OVERLAP_SECONDS`] before the last one ends. Cutting it by speaker turns
/// instead tripled WER: short pieces lose context and the turn edges clip words.
const WINDOW_SECONDS: f32 = 24.0;
/// A word in the overlap belongs to the window it is further inside of: the cut
/// is the overlap's middle, so each side has 2 s of context past it.
const OVERLAP_SECONDS: f32 = 4.0;
/// More is slower on an M4: the rest are efficiency cores (docs/36).
const MAX_THREADS: usize = 4;

/// GigaAM v3 with punctuation, then the voice model — the files
/// `scripts/fetch-meeting-models.ts` puts in `models/meeting/`.
pub const MODEL_FILES: [&str; 5] = ["encoder.int8.onnx", "decoder.onnx", "joiner.onnx", "tokens.txt", "campplus.onnx"];

/// The bundled models, or the source tree's in a dev build.
pub fn models_dir(resource_dir: Option<&Path>) -> PathBuf {
    resource_dir
        .map(|dir| dir.join("models").join("meeting"))
        .filter(|dir| has_models(dir))
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("models").join("meeting"))
}

pub fn has_models(dir: &Path) -> bool {
    MODEL_FILES.iter().all(|f| dir.join(f).is_file())
}

/// Every utterance of `audio`, speakers numbered by who spoke first.
pub fn recognize(audio: &Path, models: &Path, progress: &mut dyn FnMut(Stage, usize, usize)) -> Result<(Vec<Utterance>, f32), MeetingError> {
    if !has_models(models) {
        return Err(MeetingError::ModelsMissing);
    }
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(MAX_THREADS) as i32;
    progress(Stage::Decode, 0, 1);
    let (raw, rate) = decode(audio)?;
    let samples = to_16k(&raw, rate)?;
    drop(raw);
    progress(Stage::Decode, 1, 1);

    let words = transcribe(&recognizer(models, threads)?, &samples, progress);
    progress(Stage::Speakers, 0, 1);
    let segments = speakers(models, threads, &samples, &words)?;
    progress(Stage::Speakers, 1, 1);
    Ok((meeting::utterances(&words, &segments), samples.len() as f32 / SAMPLE_RATE as f32))
}

/// The whole file as mono f32 at its own rate.
fn decode(path: &Path) -> Result<(Vec<f32>, u32), MeetingError> {
    let unreadable = |e: String| MeetingError::Unreadable(e);
    let file = File::open(path).map_err(|e| unreadable(format!("{}: {e}", path.display())))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let mut format = symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .map_err(|e| unreadable(e.to_string()))?;
    let track = format.default_track(TrackType::Audio).ok_or_else(|| unreadable("no audio track".into()))?;
    let params = track.codec_params.as_ref().and_then(|p| p.audio()).ok_or_else(|| unreadable("no audio parameters".into()))?;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())
        .map_err(|e| unreadable(format!("no decoder for this codec: {e}")))?;
    let track_id = track.id;

    let mut mono = Vec::new();
    let mut interleaved: Vec<f32> = Vec::new();
    let mut rate = 0;
    let mut damaged = None;
    loop {
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            // A recording cut off mid-write — the recorder crashed, the disk filled —
            // is still the part that was written.
            Err(AudioError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(unreadable(e.to_string())),
        };
        if packet.track_id != track_id {
            continue;
        }
        let buf = match decoder.decode(&packet) {
            Ok(buf) => buf,
            // A damaged packet costs its few milliseconds, not the recording. If every
            // packet fails, the last reason is what is reported.
            Err(e) => {
                damaged = Some(e);
                continue;
            }
        };
        rate = buf.spec().rate();
        let channels = buf.spec().channels().count().max(1);
        interleaved.resize(buf.samples_interleaved(), 0.0);
        buf.copy_to_slice_interleaved(&mut interleaved);
        mono.extend(interleaved.chunks(channels).map(|frame| frame.iter().sum::<f32>() / channels as f32));
    }
    match (mono.is_empty(), damaged) {
        (true, Some(e)) => Err(unreadable(e.to_string())),
        (true, None) => Err(MeetingError::Silent),
        (false, _) => Ok((mono, rate)),
    }
}

fn to_16k(samples: &[f32], rate: u32) -> Result<Vec<f32>, MeetingError> {
    if rate == SAMPLE_RATE {
        return Ok(samples.to_vec());
    }
    let resampler = LinearResampler::create(rate as i32, SAMPLE_RATE as i32).ok_or(MeetingError::ModelFailed("resampler"))?;
    Ok(resampler.resample(samples, true))
}

fn path(dir: &Path, name: &str) -> Option<String> {
    Some(dir.join(name).to_string_lossy().into_owned())
}

fn recognizer(models: &Path, threads: i32) -> Result<OfflineRecognizer, MeetingError> {
    let mut config = OfflineRecognizerConfig::default();
    config.model_config.transducer.encoder = path(models, MODEL_FILES[0]);
    config.model_config.transducer.decoder = path(models, MODEL_FILES[1]);
    config.model_config.transducer.joiner = path(models, MODEL_FILES[2]);
    config.model_config.tokens = path(models, MODEL_FILES[3]);
    config.model_config.model_type = Some("nemo_transducer".into());
    config.model_config.num_threads = threads;
    OfflineRecognizer::create(&config).ok_or(MeetingError::ModelFailed("speech"))
}

/// Every word of the recording, in order, with where it is.
fn transcribe(recognizer: &OfflineRecognizer, samples: &[f32], progress: &mut dyn FnMut(Stage, usize, usize)) -> Vec<Word> {
    let window = (WINDOW_SECONDS * SAMPLE_RATE as f32) as usize;
    let hop = ((WINDOW_SECONDS - OVERLAP_SECONDS) * SAMPLE_RATE as f32) as usize;
    let windows = if samples.len() <= window { 1 } else { (samples.len() - window).div_ceil(hop) + 1 };
    let mut words = Vec::new();
    let mut kept_from = f32::NEG_INFINITY;
    for i in 0..windows {
        progress(Stage::Speech, i, windows);
        let from = i * hop;
        let to = (from + window).min(samples.len());
        let offset = from as f32 / SAMPLE_RATE as f32;
        let cut = if i + 1 == windows { f32::INFINITY } else { offset + WINDOW_SECONDS - OVERLAP_SECONDS / 2.0 };
        let stream = recognizer.create_stream();
        stream.accept_waveform(SAMPLE_RATE as i32, &samples[from..to]);
        recognizer.decode(&stream);
        let Some(result) = stream.get_result() else { continue };
        let found = match &result.timestamps {
            Some(times) => meeting::words_of(&result.tokens, times, result.durations.as_deref(), offset),
            // No timestamps: the window is one "word", its speaker the window's.
            None => vec![Word { start: offset, end: to as f32 / SAMPLE_RATE as f32, text: result.text.trim().into() }],
        };
        words.extend(found.into_iter().filter(|w| w.start >= kept_from && w.start < cut && !w.text.is_empty()));
        kept_from = cut;
    }
    progress(Stage::Speech, windows, windows);
    words
}

/// Windows over the words, one voice embedding each, labeled by spectral clustering.
fn speakers(models: &Path, threads: i32, samples: &[f32], words: &[Word]) -> Result<Vec<Segment>, MeetingError> {
    let config = SpeakerEmbeddingExtractorConfig { model: path(models, MODEL_FILES[4]), num_threads: threads, ..Default::default() };
    let extractor = SpeakerEmbeddingExtractor::create(&config).ok_or(MeetingError::ModelFailed("voice"))?;
    let mut windows = Vec::new();
    let mut embeddings = Vec::new();
    for (start, end) in meeting::windows_over(words) {
        let from = (start * SAMPLE_RATE as f32) as usize;
        let to = ((end * SAMPLE_RATE as f32) as usize).min(samples.len());
        let Some(stream) = extractor.create_stream() else { continue };
        stream.accept_waveform(SAMPLE_RATE as i32, &samples[from.min(to)..to]);
        stream.input_finished();
        if !extractor.is_ready(&stream) {
            continue;
        }
        if let Some(e) = extractor.compute(&stream) {
            windows.push((start, end));
            embeddings.push(e);
        }
    }
    let labels = meeting::spectral(&embeddings);
    Ok(windows.into_iter().zip(labels).map(|((start, end), speaker)| Segment { start, end, speaker }).collect())
}
