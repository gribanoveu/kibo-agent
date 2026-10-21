//! A recording into utterances: symphonia reads the sound, GigaAM v3 (through
//! sherpa-onnx) recognizes it, campplus embeds the voice, and
//! `domain::meeting` does the rest. Runs only inside the recognition process
//! (`meeting_sidecar`) — a native crash here must not take the window with it.

use std::fs::File;
use std::path::{Path, PathBuf};

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use sherpa_onnx::{
    LinearResampler, OfflineRecognizer, OfflineRecognizerConfig, OfflineSpeechDenoiser, OfflineSpeechDenoiserConfig, SpeakerEmbeddingExtractor,
    SpeakerEmbeddingExtractorConfig,
};
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as AudioError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

use crate::domain::meeting::{self, MeetingError, Recognized, Segment, Stage, Word};
use crate::domain::recording;
use crate::infra::app_dir;

const SAMPLE_RATE: u32 = recording::RATE;
/// The whole recording goes through GigaAM in windows this long, each starting
/// [`OVERLAP_SECONDS`] before the last one ends. Cutting it by speaker turns
/// instead tripled WER: short pieces lose context and the turn edges clip words.
const WINDOW_SECONDS: f32 = 24.0;
/// A word in the overlap belongs to the window it is further inside of: the cut
/// is the overlap's middle, so each side has 2 s of context past it.
const OVERLAP_SECONDS: f32 = 4.0;
/// More is slower on an M4: the rest are efficiency cores (docs/36).
const MAX_THREADS: usize = 4;
/// How far DPDFNet may take the noise down. Unlimited, its artifacts cost
/// GigaAM more words than the noise did: on loud renovation noise 37 % WER
/// against 23 % untouched; limited, 18 % — and on a quiet call no change (docs/36).
const DENOISE_LIMIT_DB: f32 = 12.0;
/// The microphone is denoised in pieces this long, on every thread at once,
/// each started [`DENOISE_PREROLL_SECONDS`] early so it begins warm.
const DENOISE_CHUNK_SECONDS: usize = 60;
const DENOISE_PREROLL_SECONDS: usize = 1;

/// GigaAM v3 with punctuation, the voice model, the denoiser — the files
/// `scripts/fetch-meeting-models.ts` puts in `models/meeting/`.
pub const MODEL_FILES: [&str; 6] = ["encoder.int8.onnx", "decoder.onnx", "joiner.onnx", "tokens.txt", "campplus.onnx", "dpdfnet2.onnx"];

/// The bundled models, or the source tree's in a dev build. A release has
/// only its bundle: a path on the machine that built it is no one's here, and
/// a bundle without the models says so rather than finding them elsewhere.
pub fn models_dir(resource_dir: Option<&Path>) -> PathBuf {
    let bundled = resource_dir.map(|dir| dir.join("models").join("meeting"));
    #[cfg(debug_assertions)]
    let bundled = bundled.filter(|dir| has_models(dir)).or_else(|| Some(Path::new(env!("CARGO_MANIFEST_DIR")).join("models").join("meeting")));
    bundled.unwrap_or_default()
}

pub fn has_models(dir: &Path) -> bool {
    MODEL_FILES.iter().all(|f| dir.join(f).is_file())
}

/// Every utterance of `audio`, speakers numbered by who spoke first, and their
/// voices. The sound the models heard is written to `keep` — the meeting's own
/// copy, so the original file is not needed after. `denoise_mic`: `audio` is a
/// call this app recorded, its left channel the microphone
/// (`domain::recording`), which is denoised before the two are put together.
pub fn recognize(audio: &Path, models: &Path, keep: &Path, denoise_mic: bool, progress: &mut dyn FnMut(Stage, usize, usize)) -> Result<Recognized, MeetingError> {
    if !has_models(models) {
        return Err(MeetingError::ModelsMissing);
    }
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(MAX_THREADS) as i32;
    progress(Stage::Decode, 0, 1);
    let (tracks, rate) = decode_tracks(audio, denoise_mic)?;
    let tracks = tracks.into_iter().map(|t| to_16k(t, rate)).collect::<Result<Vec<_>, _>>()?;
    let samples = heard(tracks, denoise_mic, |mic| denoise(models, mic, threads as usize))?;
    app_dir::write_private(keep, &wav(&samples)?).map_err(MeetingError::Store)?;
    progress(Stage::Decode, 1, 1);

    let words = transcribe(&recognizer(models, threads)?, &samples, progress);
    let (segments, voices) = speakers(models, threads, &samples, &words, progress)?;
    let (utterances, voices) = meeting::by_first_word(meeting::utterances(&words, &segments), voices);
    Ok(Recognized { utterances, seconds: samples.len() as f32 / SAMPLE_RATE as f32, voices })
}

/// The whole file as mono f32 at its own rate — what the tests read back.
#[cfg(test)]
pub(crate) fn decode(path: &Path) -> Result<(Vec<f32>, u32), MeetingError> {
    let (mut tracks, rate) = decode_tracks(path, false)?;
    Ok((tracks.swap_remove(0), rate))
}

/// The whole file at its own rate: each channel on its own when `split`, else
/// one, their mean.
fn decode_tracks(path: &Path, split: bool) -> Result<(Vec<Vec<f32>>, u32), MeetingError> {
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

    let mut tracks: Vec<Vec<f32>> = vec![Vec::new()];
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
        if split {
            tracks.resize_with(tracks.len().max(channels), Vec::new);
            for frame in interleaved.chunks(channels) {
                frame.iter().zip(tracks.iter_mut()).for_each(|(s, track)| track.push(*s));
            }
        } else {
            tracks[0].extend(recording::mono(&interleaved, channels));
        }
    }
    match (tracks[0].is_empty(), damaged) {
        (true, Some(e)) => Err(unreadable(e.to_string())),
        (true, None) => Err(MeetingError::Silent),
        (false, _) => Ok((tracks, rate)),
    }
}

/// What the models hear: the tracks as one, the microphone's — a recording's
/// first of two — through `denoise` first when asked.
fn heard(
    mut tracks: Vec<Vec<f32>>,
    denoise_mic: bool,
    denoise: impl FnOnce(&[f32]) -> Result<Vec<f32>, MeetingError>,
) -> Result<Vec<f32>, MeetingError> {
    if denoise_mic && tracks.len() == recording::CHANNELS as usize {
        tracks[0] = denoise(&tracks[0])?;
    }
    Ok(mixed(tracks))
}

/// The tracks as one: their mean, the longest's length. One track is itself.
fn mixed(mut tracks: Vec<Vec<f32>>) -> Vec<f32> {
    if tracks.len() == 1 {
        return tracks.swap_remove(0);
    }
    let len = tracks.iter().map(Vec::len).max().unwrap_or(0);
    let n = tracks.len().max(1) as f32;
    (0..len).map(|i| tracks.iter().map(|t| t.get(i).copied().unwrap_or(0.0)).sum::<f32>() / n).collect()
}

/// `mic` at 16 kHz with its noise taken down by DPDFNet-2, as far as
/// [`DENOISE_LIMIT_DB`]; the same length.
fn denoise(models: &Path, mic: &[f32], threads: usize) -> Result<Vec<f32>, MeetingError> {
    let mut config = OfflineSpeechDenoiserConfig::default();
    config.model.dpdfnet.model = path(models, MODEL_FILES[5]);
    config.model.dpdfnet.attenuation_limit_db = DENOISE_LIMIT_DB;
    config.model.num_threads = 1;
    let rate = SAMPLE_RATE as usize;
    in_chunks(
        mic,
        (DENOISE_CHUNK_SECONDS * rate, DENOISE_PREROLL_SECONDS * rate),
        threads,
        || OfflineSpeechDenoiser::create(&config),
        |denoiser, piece| denoiser.run(piece, SAMPLE_RATE as i32).samples,
    )
    .ok_or(MeetingError::ModelFailed("denoiser"))
}

/// `samples` through `run` a `chunk` at a time, each begun `preroll` early and
/// that part dropped, on `threads` workers with one `make()` each; the pieces
/// back in order, as long as `samples` — a piece that came back short is
/// silence past its end. `None` if a worker could not `make` its own.
fn in_chunks<W>(
    samples: &[f32],
    (chunk, preroll): (usize, usize),
    threads: usize,
    make: impl Fn() -> Option<W> + Sync,
    run: impl Fn(&W, &[f32]) -> Vec<f32> + Sync,
) -> Option<Vec<f32>> {
    let starts: Vec<usize> = (0..samples.len()).step_by(chunk.max(1)).collect();
    let pieces: Mutex<Vec<Option<Vec<f32>>>> = Mutex::new(vec![None; starts.len()]);
    let next = AtomicUsize::new(0);
    let made = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads.clamp(1, starts.len().max(1)))
            .map(|_| {
                scope.spawn(|| {
                    let worker = make()?;
                    while let Some(&start) = starts.get(next.fetch_add(1, Ordering::Relaxed)) {
                        let (from, end) = (start.saturating_sub(preroll), (start + chunk).min(samples.len()));
                        let out = run(&worker, &samples[from..end]);
                        let mut own: Vec<f32> = out.get(start - from..).unwrap_or_default().iter().copied().take(end - start).collect();
                        own.resize(end - start, 0.0);
                        pieces.lock().unwrap_or_else(|e| e.into_inner())[start / chunk.max(1)] = Some(own);
                    }
                    Some(())
                })
            })
            .collect();
        workers.into_iter().all(|w| w.join().ok().flatten().is_some())
    });
    made.then(|| pieces.into_inner().unwrap_or_else(|e| e.into_inner()).into_iter().flatten().flatten().collect())
}

/// `samples` at 16 kHz; already there, the same ones, not a copy of an hour.
fn to_16k(samples: Vec<f32>, rate: u32) -> Result<Vec<f32>, MeetingError> {
    if rate == SAMPLE_RATE {
        return Ok(samples);
    }
    let resampler = LinearResampler::create(rate as i32, SAMPLE_RATE as i32).ok_or(MeetingError::ModelFailed("resampler"))?;
    Ok(resampler.resample(&samples, true))
}

/// 16 kHz mono 16-bit PCM WAV: 32 KB a second, 115 MB an hour.
fn wav(samples: &[f32]) -> Result<Vec<u8>, MeetingError> {
    let data = u32::try_from(samples.len() * 2)
        .ok()
        .filter(|data| *data <= recording::max_data_bytes(1))
        .ok_or_else(|| MeetingError::Unreadable("longer than a WAV file holds".into()))?;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(&recording::wav_header(1, data));
    for s in samples {
        // Past ±1 is clipped: `as` saturates.
        out.extend_from_slice(&((s * i16::MAX as f32) as i16).to_le_bytes());
    }
    Ok(out)
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
        let found = result.timestamps.as_ref().map(|times| meeting::words_of(&result.tokens, times, result.durations.as_deref(), offset));
        words.extend(own_words(found, &result.text, (offset, to as f32 / SAMPLE_RATE as f32), kept_from, cut));
        kept_from = cut;
    }
    progress(Stage::Speech, windows, windows);
    words
}

/// The words of a window that are its own: starting where the last window's
/// ended and before this one's `cut`. Without timestamps the window is one
/// "word", its speaker the window's, from where the last one's ended.
fn own_words(found: Option<Vec<Word>>, text: &str, (from, to): (f32, f32), kept_from: f32, cut: f32) -> Vec<Word> {
    let found = found.unwrap_or_else(|| vec![Word { start: from.max(kept_from), end: to, text: text.trim().into() }]);
    found.into_iter().filter(|w| w.start >= kept_from && w.start < cut && !w.text.is_empty()).collect()
}

/// Windows over the words, one voice embedding each, labeled by spectral
/// clustering; and each label's mean voice.
fn speakers(
    models: &Path,
    threads: i32,
    samples: &[f32],
    words: &[Word],
    progress: &mut dyn FnMut(Stage, usize, usize),
) -> Result<(Vec<Segment>, Vec<Vec<f32>>), MeetingError> {
    let config = SpeakerEmbeddingExtractorConfig { model: path(models, MODEL_FILES[4]), num_threads: threads, ..Default::default() };
    let extractor = SpeakerEmbeddingExtractor::create(&config).ok_or(MeetingError::ModelFailed("voice"))?;
    let over = meeting::windows_over(words);
    let total = over.len();
    // An hour is thousands of windows: told a hundredth at a time, not each.
    let step = (total / 100).max(1);
    let mut windows = Vec::new();
    let mut embeddings = Vec::new();
    for (i, (start, end)) in over.into_iter().enumerate() {
        if i % step == 0 {
            progress(Stage::Speakers, i, total);
        }
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
    progress(Stage::Speakers, total, total);
    let labels = meeting::spectral(&embeddings);
    let voices = meeting::centroids(&embeddings, &labels);
    Ok((windows.into_iter().zip(labels).map(|((start, end), speaker)| Segment { start, end, speaker }).collect(), voices))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundle_is_where_the_models_are_and_only_a_dev_build_looks_further() {
        let bundle = crate::testing::temp_dir("speech-bundle");
        let models = bundle.join("models").join("meeting");
        std::fs::create_dir_all(&models).unwrap();
        MODEL_FILES.iter().for_each(|f| std::fs::write(models.join(f), "").unwrap());
        assert_eq!(models_dir(Some(&bundle)), models, "the bundle's, when it has them");
        let empty = crate::testing::temp_dir("speech-bundle-empty");
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("models").join("meeting");
        let fallback = if cfg!(debug_assertions) { source } else { empty.join("models").join("meeting") };
        assert_eq!(models_dir(Some(&empty)), fallback);
    }

    #[test]
    fn each_window_keeps_its_own_words_with_or_without_timestamps() {
        let word = |start: f32, text: &str| Word { start, end: start + 0.5, text: text.into() };
        // The second window, 20–40 s: the first kept up to 22 s, this one to 42 s.
        let timed = own_words(Some(vec![word(21.0, "уже"), word(30.0, "своё"), word(43.0, "дальше")]), "", (20.0, 44.0), 22.0, 42.0);
        assert_eq!(timed, [word(30.0, "своё")]);
        let untimed = own_words(None, " всё окно ", (20.0, 44.0), 22.0, 42.0);
        assert_eq!(untimed, [Word { start: 22.0, end: 44.0, text: "всё окно".into() }], "not dropped for starting in the overlap");
        assert_eq!(own_words(None, "", (20.0, 44.0), 22.0, 42.0), []);
        assert_eq!(own_words(None, "начало", (0.0, 24.0), f32::NEG_INFINITY, 22.0)[0].start, 0.0);
    }

    #[test]
    fn a_track_in_pieces_comes_back_whole_and_in_order() {
        let samples: Vec<f32> = (0..1000).map(|i| i as f32).collect();
        // Each piece marked by its worker's run: what is kept is the piece's own part.
        let doubled = in_chunks(&samples, (300, 50), 3, || Some(()), |_, piece| piece.iter().map(|s| s * 2.0).collect()).unwrap();
        assert_eq!(doubled, samples.iter().map(|s| s * 2.0).collect::<Vec<_>>(), "the preroll is dropped, nothing doubled or lost");

        // A piece begun early starts at its preroll: what `run` saw first is that.
        let firsts = Mutex::new(Vec::new());
        in_chunks(&samples, (300, 50), 2, || Some(()), |_, piece| {
            firsts.lock().unwrap().push((piece[0], piece.len()));
            piece.to_vec()
        })
        .unwrap();
        let mut firsts = firsts.into_inner().unwrap();
        firsts.sort_by(|a, b| a.0.total_cmp(&b.0));
        assert_eq!(firsts, [(0.0, 300), (250.0, 350), (550.0, 350), (850.0, 150)]);

        let short = in_chunks(&samples, (300, 0), 2, || Some(()), |_, piece| piece[..piece.len() / 2].to_vec()).unwrap();
        assert_eq!(short.len(), 1000, "a piece back short is silence past its end");
        assert_eq!((short[149], short[150], short[300]), (149.0, 0.0, 300.0));
        assert!(in_chunks(&samples, (300, 0), 2, || None::<()>, |_, p| p.to_vec()).is_none(), "no worker, no result");
        assert_eq!(in_chunks(&[], (300, 0), 4, || Some(()), |_, p| p.to_vec()), Some(vec![]));
    }

    #[test]
    fn a_recordings_channels_read_apart_and_put_together() {
        let path = crate::testing::temp_dir("speech-split").join("call.wav");
        let mut bytes = recording::wav_header(2, 4 * 3).to_vec();
        bytes.extend(recording::interleave(&[0.5, 0.25, 0.0], &[-0.5, 0.25, 1.0], 3));
        std::fs::write(&path, bytes).unwrap();
        let (tracks, rate) = decode_tracks(&path, true).unwrap();
        let near = |a: &[f32], b: &[f32]| a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-3);
        assert_eq!((tracks.len(), rate), (2, SAMPLE_RATE));
        assert!(near(&tracks[0], &[0.5, 0.25, 0.0]) && near(&tracks[1], &[-0.5, 0.25, 1.0]), "{tracks:?}");
        let (one, _) = decode_tracks(&path, false).unwrap();
        assert!(one.len() == 1 && near(&one[0], &[0.0, 0.25, 0.5]), "{one:?}");
        assert!(near(&mixed(tracks), &one[0]), "put together as unsplit");
        assert_eq!(mixed(vec![vec![1.0, 1.0], vec![0.0]]), [0.5, 0.5], "the shorter is silence past its end");

        let louder = |mic: &[f32]| Ok(mic.iter().map(|s| s * 3.0).collect());
        assert_eq!(heard(vec![vec![0.2], vec![0.4]], true, louder).unwrap(), [0.5], "the microphone, and only it, goes through");
        assert_eq!(heard(vec![vec![0.2], vec![0.4]], false, louder).unwrap(), [0.3f32], "not asked: as it is");
        assert_eq!(heard(vec![vec![0.2]], true, louder).unwrap(), [0.2], "one track is no recording of ours");
        assert!(heard(vec![vec![0.2], vec![0.4]], true, |_| Err(MeetingError::ModelFailed("denoiser"))).is_err());
    }

    /// The bundled denoiser on speech under noise: `cargo test denoise_live -- --ignored`.
    /// A tone stands in for a voice, white noise for the room; with the model's
    /// files in `models/meeting/` (`bun scripts/fetch-meeting-models.ts`).
    #[test]
    #[ignore]
    fn denoise_live() {
        let models = Path::new(env!("CARGO_MANIFEST_DIR")).join("models").join("meeting");
        let seconds = 130; // three pieces, one short
        let mut seed = 1u32;
        let mut noise = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1 << 24) as f32 - 0.5
        };
        let noisy: Vec<f32> = (0..seconds * SAMPLE_RATE as usize).map(|i| 0.3 * (i as f32 / 16.0).sin() * ((i / 8000) % 2) as f32 + 0.2 * noise()).collect();
        let started = std::time::Instant::now();
        let clean = denoise(&models, &noisy, MAX_THREADS).unwrap();
        let rms = |s: &[f32]| (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt();
        // The gaps between "words" — every other half second, the first one left
        // out while the model warms up: noise only, before and after.
        let gaps = |s: &[f32]| s.chunks(8000).step_by(2).skip(1).flatten().copied().collect::<Vec<_>>();
        eprintln!("{seconds} s in {:.1} s; noise in the gaps {:.4} → {:.4}", started.elapsed().as_secs_f32(), rms(&gaps(&noisy)), rms(&gaps(&clean)));
        assert_eq!(clean.len(), noisy.len());
        assert!(rms(&gaps(&clean)) < rms(&gaps(&noisy)) / 2.0, "the noise is taken down");
        assert!(rms(&gaps(&clean)) > rms(&gaps(&noisy)) / 8.0, "but not out: 12 dB is a quarter");
    }

    #[test]
    fn the_kept_copy_reads_back_as_the_sound_the_models_heard() {
        let samples: Vec<f32> = (0..SAMPLE_RATE).map(|i| (i as f32 / 20.0).sin() * 0.5).chain([2.0, -2.0]).collect();
        let path = crate::testing::temp_dir("speech-wav").join("audio.wav");
        let bytes = wav(&samples).unwrap();
        assert_eq!(bytes.len(), 44 + 2 * samples.len());
        // What symphonia does not read and a player may: 32000 bytes a second, 2 a frame.
        assert_eq!(bytes[28..34], [0x00, 0x7d, 0, 0, 2, 0]);
        std::fs::write(&path, bytes).unwrap();

        let (back, rate) = decode(&path).unwrap();
        assert_eq!((rate, back.len()), (SAMPLE_RATE, samples.len()));
        let worst = samples.iter().zip(&back).map(|(a, b)| (a.clamp(-1.0, 1.0) - b).abs()).fold(0.0, f32::max);
        assert!(worst < 1e-3, "{worst}");
    }
}
