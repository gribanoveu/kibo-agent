//! MN-0 of `docs/36-meeting-notes-research.md`: the sidecar shape, tried for real.
//!
//! One process per recording. The app would start it, read JSON lines from its
//! stdout, and lose nothing but the line "failed" if a native library crashes
//! inside it — which is the reason it is a process and not a thread.
//!
//! ```text
//! meeting-sidecar <audio> --models <dir> [--asr punct|plain] [--embedding wespeaker|campplus]
//!                 [--speakers N] [--threshold F] [--threads N] [--clustering sherpa|spectral]
//! ```
//!
//! `--clustering spectral` replaces sherpa's diarization with windows over the
//! recognized words, one speaker embedding each, and spectral clustering whose
//! speaker count is the largest eigengap — no threshold to pick.
//!
//! `--speakers` is there to measure, not to use: given a count, sherpa's
//! clustering merged voices and left clusters empty on both real recordings.
//!
//! Lines on stdout, one JSON object each:
//! - `{"type":"progress","stage":"decode"|"diarize"|"asr","done":i,"total":n}`
//! - `{"type":"result","utterances":[{speaker,start,end,text}],"stats":{…}}`
//! - `{"type":"error","message":"…"}` — and exit code 1.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use serde_json::{json, Value};
use nalgebra::{DMatrix, SymmetricEigen};
use sherpa_onnx::{
    LinearResampler, OfflineRecognizer, OfflineRecognizerConfig, OfflineSpeakerDiarization,
    OfflineSpeakerDiarizationConfig, OfflineSpeakerDiarizationSegment, SpeakerEmbeddingExtractor,
    SpeakerEmbeddingExtractorConfig,
};
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as AudioError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

const SAMPLE_RATE: u32 = 16_000;
/// The whole recording goes through GigaAM in windows this long, each starting
/// [`OVERLAP_SECONDS`] before the last one ends. Cutting it by diarization turns
/// instead tripled WER: short pieces lose context and the turn edges clip words.
const WINDOW_SECONDS: f32 = 24.0;
/// A word in the overlap belongs to the window it is further inside of: the cut
/// is the overlap's middle, so each side has 2 s of context past it.
const OVERLAP_SECONDS: f32 = 4.0;
/// How long a token lasts when the model gives only where it starts: one GigaAM
/// frame is 40 ms, a token rarely less than two.
const TOKEN_SECONDS: f32 = 0.08;
/// The same speaker again after a pause shorter than this is one utterance.
const JOIN_GAP_SECONDS: f32 = 1.5;

struct Args {
    audio: PathBuf,
    models: PathBuf,
    asr: String,
    embedding: String,
    speakers: i32,
    threshold: f32,
    threads: i32,
    clustering: String,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            emit(json!({ "type": "error", "message": message }));
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let args = parse_args(std::env::args().skip(1))?;
    let started = Instant::now();

    emit(progress("decode", 0, 1));
    let (raw, rate) = decode(&args.audio)?;
    let samples = to_16k(&raw, rate)?;
    drop(raw);
    let audio_seconds = samples.len() as f32 / SAMPLE_RATE as f32;
    let decoded = started.elapsed().as_secs_f32();
    emit(progress("decode", 1, 1));

    let at = Instant::now();
    let recognizer = recognizer(&args)?;
    let words = transcribe(&recognizer, &samples);
    let asr_seconds = at.elapsed().as_secs_f32();

    emit(progress("diarize", 0, 1));
    let at = Instant::now();
    let segments = match args.clustering.as_str() {
        "sherpa" => diarize(&args, &samples)?,
        "spectral" => spectral_segments(&args, &samples, &words)?,
        other => return Err(format!("--clustering: no such method {other}")),
    };
    let diarize_seconds = at.elapsed().as_secs_f32();
    emit(progress("diarize", 1, 1));
    let utterances: Vec<Value> = utterances(&words, &segments)
        .iter()
        .map(|u| {
            let spans: Vec<[f32; 2]> = u.words.iter().map(|w| [round(w.start), round(w.end)]).collect();
            json!({ "speaker": u.speaker, "start": round(u.start), "end": round(u.end), "text": u.text, "words": spans })
        })
        .collect();
    let total = started.elapsed().as_secs_f32();

    let speakers = segments.iter().map(|s| s.speaker).max().map_or(0, |m| m + 1);
    emit(json!({
        "type": "result",
        "utterances": utterances,
        "stats": {
            "audio_seconds": round(audio_seconds),
            "speakers": speakers,
            "segments": segments.len(),
            "words": words.len(),
            "decode_seconds": round(decoded),
            "diarize_seconds": round(diarize_seconds),
            "asr_seconds": round(asr_seconds),
            "rtf": (total / audio_seconds * 1000.0).round() / 1000.0,
        },
    }));
    Ok(())
}

fn parse_args(mut it: impl Iterator<Item = String>) -> Result<Args, String> {
    let usage = "usage: meeting-sidecar <audio> --models <dir> [--asr punct|plain] \
                 [--embedding wespeaker|campplus] [--speakers N] [--threshold F] [--threads N] \
                 [--clustering sherpa|spectral]";
    let audio = it.next().ok_or(usage)?;
    let mut args = Args {
        audio: PathBuf::from(audio),
        models: PathBuf::new(),
        asr: "punct".into(),
        // Spectral clustering on campplus found the 5 people of a real call and gave the
        // same WDER on two resamplings of it (16 %); sherpa's own swung from 10 % to 39 %.
        embedding: "campplus".into(),
        speakers: -1,
        // For `--clustering sherpa` with wespeaker: 0.6 was best on the real call.
        threshold: 0.6,
        threads: 4,
        clustering: "spectral".into(),
    };
    while let Some(flag) = it.next() {
        let value = it.next().ok_or_else(|| format!("{flag} needs a value"))?;
        let number = || format!("{flag}: not a number: {value}");
        match flag.as_str() {
            "--models" => args.models = PathBuf::from(&value),
            "--asr" => args.asr = value,
            "--embedding" => args.embedding = value,
            "--speakers" => args.speakers = value.parse().map_err(|_| number())?,
            "--threshold" => args.threshold = value.parse().map_err(|_| number())?,
            "--threads" => args.threads = value.parse().map_err(|_| number())?,
            "--clustering" => args.clustering = value,
            _ => return Err(format!("unknown flag {flag}\n{usage}")),
        }
    }
    if args.models.as_os_str().is_empty() {
        return Err(usage.into());
    }
    Ok(args)
}

/// The whole file as mono f32 at its own rate.
fn decode(path: &Path) -> Result<(Vec<f32>, u32), String> {
    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let mut format = symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .map_err(|e| format!("not a recording this can read: {e}"))?;
    let track = format.default_track(TrackType::Audio).ok_or("no audio track")?;
    let params = track.codec_params.as_ref().and_then(|p| p.audio()).ok_or("no audio parameters")?;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())
        .map_err(|e| format!("no decoder for this codec: {e}"))?;
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
            Err(e) => return Err(format!("reading the file: {e}")),
        };
        if packet.track_id != track_id {
            continue;
        }
        let buf = match decoder.decode(&packet) {
            Ok(buf) => buf,
            // A damaged packet costs its few milliseconds, not the recording: garbage
            // in AAC reads as anything, "unsupported feature" included. If every packet
            // fails, the last reason is what is reported.
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
        (true, Some(e)) => Err(format!("decoding: {e}")),
        (true, None) => Err("the recording has no sound in it".into()),
        (false, _) => Ok((mono, rate)),
    }
}

fn to_16k(samples: &[f32], rate: u32) -> Result<Vec<f32>, String> {
    if rate == SAMPLE_RATE {
        return Ok(samples.to_vec());
    }
    let resampler = LinearResampler::create(rate as i32, SAMPLE_RATE as i32).ok_or("no resampler")?;
    Ok(resampler.resample(samples, true))
}

fn diarize(args: &Args, samples: &[f32]) -> Result<Vec<OfflineSpeakerDiarizationSegment>, String> {
    let mut config = OfflineSpeakerDiarizationConfig::default();
    config.segmentation.pyannote.model = Some(model(&args.models.join("sherpa-onnx-pyannote-segmentation-3-0"), "model")?);
    config.segmentation.num_threads = args.threads;
    config.embedding.model = Some(embedding_model(args)?);
    config.embedding.num_threads = args.threads;
    config.clustering.num_clusters = args.speakers;
    config.clustering.threshold = args.threshold;
    let diarizer = OfflineSpeakerDiarization::create(&config).ok_or("the diarization models did not load")?;
    let result = diarizer.process(samples).ok_or("diarization failed")?;
    Ok(result.sort_by_start_time())
}

fn embedding_model(args: &Args) -> Result<String, String> {
    match args.embedding.as_str() {
        "wespeaker" => file(&args.models, "wespeaker_en_voxceleb_resnet34_LM.onnx"),
        "campplus" => file(&args.models, "3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx"),
        other => Err(format!("--embedding: no such model {other}")),
    }
}

/// One speaker embedding per window of this long, every [`EMBED_HOP_SECONDS`],
/// over where the words are; shorter speech gets one window of its own length.
const EMBED_WINDOW_SECONDS: f32 = 1.5;
const EMBED_HOP_SECONDS: f32 = 0.75;
/// The eigengap is looked for among this many speakers at most.
const MAX_SPEAKERS: usize = 10;

/// Segments for [`utterances`] made without sherpa's clustering: windows over
/// speech, each labeled by [`spectral`].
fn spectral_segments(args: &Args, samples: &[f32], words: &[Word]) -> Result<Vec<OfflineSpeakerDiarizationSegment>, String> {
    let config = SpeakerEmbeddingExtractorConfig {
        model: Some(embedding_model(args)?),
        num_threads: args.threads,
        ..Default::default()
    };
    let extractor = SpeakerEmbeddingExtractor::create(&config).ok_or("the speaker model did not load")?;
    let mut windows = Vec::new();
    let mut embeddings = Vec::new();
    for (start, end) in windows_over(words) {
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
    let labels = spectral(&embeddings);
    Ok(windows
        .into_iter()
        .zip(labels)
        .map(|((start, end), speaker)| OfflineSpeakerDiarizationSegment { start, end, speaker: speaker as i32, confidence: 0.0 })
        .collect())
}

/// Speech is where words are, pauses under half a second closed; each stretch
/// is cut into overlapping windows.
fn windows_over(words: &[Word]) -> Vec<(f32, f32)> {
    let mut stretches: Vec<(f32, f32)> = Vec::new();
    for w in words {
        match stretches.last_mut() {
            Some(last) if w.start - last.1 < 0.5 => last.1 = last.1.max(w.end),
            _ => stretches.push((w.start, w.end)),
        }
    }
    let mut out = Vec::new();
    for (start, end) in stretches {
        let mut at = start;
        loop {
            let to = (at + EMBED_WINDOW_SECONDS).min(end);
            out.push((at, to));
            if to >= end {
                break;
            }
            at += EMBED_HOP_SECONDS;
        }
    }
    out
}

/// Spectral clustering of embeddings: cosine affinity, each row pruned to its
/// `p` nearest, the speaker count the largest gap among the Laplacian's
/// smallest eigenvalues, k-means on that many eigenvectors. `p` is the one that
/// makes the gap clearest (NME-SC, Park et al. 2019).
// ponytail: dense eigen-decomposition, O(n³) — ~4000 windows an hour is seconds; Lanczos if longer.
fn spectral(embeddings: &[Vec<f32>]) -> Vec<usize> {
    let n = embeddings.len();
    if n < 3 {
        return vec![0; n];
    }
    let unit: Vec<Vec<f64>> = embeddings
        .iter()
        .map(|e| {
            let norm = e.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt().max(1e-9);
            e.iter().map(|x| *x as f64 / norm).collect()
        })
        .collect();
    let cosine = DMatrix::from_fn(n, n, |i, j| unit[i].iter().zip(&unit[j]).map(|(a, b)| a * b).sum::<f64>());
    let max_k = MAX_SPEAKERS.min(n - 1);
    let mut best: Option<(f64, usize, DMatrix<f64>)> = None;
    for frac in [0.02, 0.04, 0.06, 0.08, 0.1, 0.15, 0.2, 0.25] {
        let p = ((n as f64 * frac).round() as usize).clamp(2, n - 1);
        let mut pruned = DMatrix::<f64>::zeros(n, n);
        for i in 0..n {
            let mut row: Vec<(usize, f64)> = (0..n).map(|j| (j, cosine[(i, j)])).collect();
            row.sort_by(|a, b| b.1.total_cmp(&a.1));
            for &(j, _) in row.iter().take(p) {
                pruned[(i, j)] = 1.0;
            }
        }
        let affinity = (&pruned + pruned.transpose()) * 0.5;
        let degree = DMatrix::from_diagonal(&affinity.column_sum());
        let eigen: SymmetricEigen<f64, nalgebra::Dyn> = SymmetricEigen::new(degree - &affinity);
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by(|&a, &b| eigen.eigenvalues[a].total_cmp(&eigen.eigenvalues[b]));
        let values: Vec<f64> = order.iter().map(|&i| eigen.eigenvalues[i]).collect();
        let (k, gap) = (1..=max_k).map(|k| (k, values[k] - values[k - 1])).max_by(|a, b| a.1.total_cmp(&b.1)).unwrap_or((1, 0.0));
        let ratio = (p as f64 / n as f64) / (gap / values[n - 1].max(1e-9) + 1e-10);
        if best.as_ref().is_none_or(|(r, _, _)| ratio < *r) {
            let vectors = DMatrix::from_fn(n, k, |i, c| eigen.eigenvectors[(i, order[c])]);
            best = Some((ratio, k, vectors));
        }
    }
    let Some((_, k, vectors)) = best else { return vec![0; n] };
    first_seen_order(&kmeans(&vectors, k))
}

/// K-means on the rows, started from the farthest-apart rows: the same input
/// gives the same labels.
fn kmeans(rows: &DMatrix<f64>, k: usize) -> Vec<usize> {
    let n = rows.nrows();
    let dist = |i: usize, c: &[f64]| (0..rows.ncols()).map(|d| (rows[(i, d)] - c[d]).powi(2)).sum::<f64>();
    let row = |i: usize| (0..rows.ncols()).map(|d| rows[(i, d)]).collect::<Vec<f64>>();
    let mut centers = vec![row(0)];
    while centers.len() < k {
        let far = (0..n)
            .max_by(|&a, &b| {
                let da = centers.iter().map(|c| dist(a, c)).fold(f64::MAX, f64::min);
                let db = centers.iter().map(|c| dist(b, c)).fold(f64::MAX, f64::min);
                da.total_cmp(&db)
            })
            .unwrap_or(0);
        centers.push(row(far));
    }
    let mut labels = vec![0; n];
    for _ in 0..50 {
        let next: Vec<usize> = (0..n)
            .map(|i| (0..k).min_by(|&a, &b| dist(i, &centers[a]).total_cmp(&dist(i, &centers[b]))).unwrap_or(0))
            .collect();
        let settled = next == labels;
        labels = next;
        for (c, center) in centers.iter_mut().enumerate() {
            let members: Vec<usize> = (0..n).filter(|&i| labels[i] == c).collect();
            if !members.is_empty() {
                *center = (0..rows.ncols()).map(|d| members.iter().map(|&i| rows[(i, d)]).sum::<f64>() / members.len() as f64).collect();
            }
        }
        if settled {
            break;
        }
    }
    labels
}

/// Labels renumbered by first appearance, so speaker 0 is whoever spoke first.
fn first_seen_order(labels: &[usize]) -> Vec<usize> {
    let mut seen: Vec<usize> = Vec::new();
    labels
        .iter()
        .map(|l| match seen.iter().position(|s| s == l) {
            Some(i) => i,
            None => {
                seen.push(*l);
                seen.len() - 1
            }
        })
        .collect()
}

fn recognizer(args: &Args) -> Result<OfflineRecognizer, String> {
    let dir = args.models.join(match args.asr.as_str() {
        "punct" => "sherpa-onnx-nemo-transducer-punct-giga-am-v3-russian-2025-12-16",
        "plain" => "sherpa-onnx-nemo-transducer-giga-am-v3-russian-2025-12-16",
        other => return Err(format!("--asr: no such model {other}")),
    });
    let mut config = OfflineRecognizerConfig::default();
    config.model_config.transducer.encoder = Some(model(&dir, "encoder")?);
    config.model_config.transducer.decoder = Some(model(&dir, "decoder")?);
    config.model_config.transducer.joiner = Some(model(&dir, "joiner")?);
    config.model_config.tokens = Some(file(&dir, "tokens.txt")?);
    config.model_config.model_type = Some("nemo_transducer".into());
    config.model_config.num_threads = args.threads;
    OfflineRecognizer::create(&config).ok_or_else(|| "the speech model did not load".into())
}

#[derive(Debug, Clone, PartialEq)]
struct Word {
    start: f32,
    end: f32,
    text: String,
}

/// Every word of the recording, in order, with where it is.
fn transcribe(recognizer: &OfflineRecognizer, samples: &[f32]) -> Vec<Word> {
    let window = (WINDOW_SECONDS * SAMPLE_RATE as f32) as usize;
    let hop = ((WINDOW_SECONDS - OVERLAP_SECONDS) * SAMPLE_RATE as f32) as usize;
    let windows = if samples.len() <= window { 1 } else { (samples.len() - window).div_ceil(hop) + 1 };
    let mut words = Vec::new();
    let mut kept_from = f32::NEG_INFINITY;
    for i in 0..windows {
        emit(progress("asr", i, windows));
        let from = i * hop;
        let to = (from + window).min(samples.len());
        let offset = from as f32 / SAMPLE_RATE as f32;
        let cut = if i + 1 == windows { f32::INFINITY } else { offset + WINDOW_SECONDS - OVERLAP_SECONDS / 2.0 };
        let stream = recognizer.create_stream();
        stream.accept_waveform(SAMPLE_RATE as i32, &samples[from..to]);
        recognizer.decode(&stream);
        let Some(result) = stream.get_result() else { continue };
        let found = match &result.timestamps {
            Some(times) => words_of(&result.tokens, times, result.durations.as_deref(), offset),
            // No timestamps: the window is one "word", its speaker the window's.
            None => vec![Word { start: offset, end: to as f32 / SAMPLE_RATE as f32, text: result.text.trim().into() }],
        };
        words.extend(found.into_iter().filter(|w| w.start >= kept_from && w.start < cut && !w.text.is_empty()));
        kept_from = cut;
    }
    emit(progress("asr", windows, windows));
    words
}

/// SentencePiece tokens joined into words: `▁` starts one — on the token, or
/// as a token of its own before it — and a token of only punctuation stays
/// with the word before it.
fn words_of(tokens: &[String], times: &[f32], durations: Option<&[f32]>, offset: f32) -> Vec<Word> {
    let mut words: Vec<Word> = Vec::new();
    let mut mark = false;
    for (i, (token, &at)) in tokens.iter().zip(times).enumerate() {
        let starts = mark || token.starts_with('▁') || token.starts_with(' ');
        let piece = token.trim_start_matches(['▁', ' ']);
        if piece.is_empty() {
            mark = true;
            continue;
        }
        mark = false;
        let end = offset + at + durations.and_then(|d| d.get(i)).copied().filter(|d| *d > 0.0).unwrap_or(TOKEN_SECONDS);
        let punctuation = !piece.chars().any(char::is_alphanumeric);
        match words.last_mut() {
            Some(last) if !starts || punctuation => {
                last.text.push_str(piece);
                last.end = end;
            }
            _ => words.push(Word { start: offset + at, end, text: piece.to_string() }),
        }
    }
    words
}

#[derive(Debug, PartialEq)]
struct Utterance {
    speaker: i32,
    start: f32,
    end: f32,
    text: String,
    /// Where speech actually is: the pauses inside an utterance are not speech.
    words: Vec<Word>,
}

/// Each word goes to the speaker whose segment it overlaps most — or, outside
/// every segment, the nearest one; consecutive words of one speaker, with no
/// long pause between, are one utterance.
fn utterances(words: &[Word], segments: &[OfflineSpeakerDiarizationSegment]) -> Vec<Utterance> {
    let mut out: Vec<Utterance> = Vec::new();
    for w in words {
        let speaker = speaker_of(w, segments);
        match out.last_mut() {
            Some(last) if last.speaker == speaker && w.start - last.end < JOIN_GAP_SECONDS => {
                last.text.push(' ');
                last.text.push_str(&w.text);
                last.end = w.end;
                last.words.push(w.clone());
            }
            _ => out.push(Utterance { speaker, start: w.start, end: w.end, text: w.text.clone(), words: vec![w.clone()] }),
        }
    }
    out
}

fn speaker_of(w: &Word, segments: &[OfflineSpeakerDiarizationSegment]) -> i32 {
    let overlap = |s: &OfflineSpeakerDiarizationSegment| w.end.min(s.end) - w.start.max(s.start);
    let distance = |s: &OfflineSpeakerDiarizationSegment| (s.start - w.end).max(w.start - s.end).max(0.0);
    segments
        .iter()
        .max_by(|a, b| overlap(a).total_cmp(&overlap(b)).then(distance(b).total_cmp(&distance(a))))
        .map_or(0, |s| s.speaker)
}

/// The one `.onnx` in `dir` named `<prefix>…`, int8 when there is a choice.
fn model(dir: &Path, prefix: &str) -> Result<String, String> {
    let mut found: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            name.starts_with(prefix) && name.ends_with(".onnx")
        })
        .collect();
    found.sort_by_key(|p| !p.to_string_lossy().contains("int8"));
    found
        .first()
        .map(|p| p.to_string_lossy().into_owned())
        .ok_or_else(|| format!("no {prefix}*.onnx in {}", dir.display()))
}

fn file(dir: &Path, name: &str) -> Result<String, String> {
    let path = dir.join(name);
    if path.is_file() {
        Ok(path.to_string_lossy().into_owned())
    } else {
        Err(format!("missing {}", path.display()))
    }
}

fn progress(stage: &str, done: usize, total: usize) -> Value {
    json!({ "type": "progress", "stage": stage, "done": done, "total": total })
}

fn emit(line: Value) {
    let mut out = std::io::stdout().lock();
    // A closed pipe means the app is gone; nothing is left to tell.
    let _ = writeln!(out, "{line}").and_then(|()| out.flush());
}

fn round(x: f32) -> f32 {
    (x * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(speaker: i32, start: f32, end: f32) -> OfflineSpeakerDiarizationSegment {
        OfflineSpeakerDiarizationSegment { start, end, speaker, confidence: 0.0 }
    }

    fn word(start: f32, end: f32, text: &str) -> Word {
        Word { start, end, text: text.into() }
    }

    fn tokens(t: &[&str]) -> Vec<String> {
        t.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn spectral_finds_three_speakers_without_being_told() {
        // Three directions in 8 dimensions, ten noisy windows each, interleaved.
        let mut embeddings = Vec::new();
        for i in 0..30 {
            let mut e = vec![0.0f32; 8];
            e[i % 3] = 1.0;
            e[3 + i % 5] = 0.15;
            embeddings.push(e);
        }
        let labels = spectral(&embeddings);
        assert_eq!(labels.iter().max(), Some(&2));
        assert!((0..30).all(|i| labels[i] == labels[i % 3]), "{labels:?}");
    }

    #[test]
    fn speech_stretches_are_cut_into_overlapping_windows() {
        let words = [word(0.0, 1.0, "а"), word(1.2, 3.0, "б"), word(10.0, 10.4, "в")];
        assert_eq!(windows_over(&words), [(0.0, 1.5), (0.75, 2.25), (1.5, 3.0), (10.0, 10.4)]);
    }

    #[test]
    fn tokens_join_into_words_at_the_word_mark() {
        let got = words_of(&tokens(&["▁При", "вет", ",", "▁мир", "▁..."]), &[0.0, 0.2, 0.4, 1.0, 1.4], None, 10.0);
        assert_eq!(got, [word(10.0, 10.48, "Привет,"), word(11.0, 11.48, "мир...")]);
    }

    #[test]
    fn a_word_mark_of_its_own_starts_the_next_word() {
        let times = [0.0, 0.25, 0.25, 0.5, 1.0, 1.0];
        let got = words_of(&tokens(&["▁У", "▁", "лу", "ком", "▁", "дуб"]), &times, Some(&[0.25; 6]), 0.0);
        assert_eq!(got, [word(0.0, 0.25, "У"), word(0.25, 0.75, "луком"), word(1.0, 1.25, "дуб")]);
    }

    #[test]
    fn a_tokens_duration_is_its_end_when_the_model_gives_one() {
        let got = words_of(&tokens(&["▁да"]), &[1.0], Some(&[0.3]), 0.0);
        assert_eq!(got, [word(1.0, 1.3, "да")]);
    }

    #[test]
    fn a_word_goes_to_the_speaker_it_overlaps_most() {
        let segments = [seg(0, 0.0, 1.2), seg(1, 1.0, 3.0)];
        assert_eq!(speaker_of(&word(0.9, 1.5, "x"), &segments), 1);
        assert_eq!(speaker_of(&word(0.1, 1.1, "x"), &segments), 0);
    }

    #[test]
    fn a_word_outside_every_segment_goes_to_the_nearest() {
        let segments = [seg(0, 0.0, 1.0), seg(1, 5.0, 6.0)];
        assert_eq!(speaker_of(&word(4.0, 4.5, "x"), &segments), 1);
        assert_eq!(speaker_of(&word(1.2, 1.5, "x"), &segments), 0);
        assert_eq!(speaker_of(&word(1.2, 1.5, "x"), &[]), 0);
    }

    #[test]
    fn one_speakers_words_join_until_a_long_pause_or_another_speaker() {
        let segments = [seg(0, 0.0, 10.0), seg(1, 10.0, 12.0)];
        let words = [word(0.0, 0.5, "а"), word(1.0, 1.5, "б"), word(5.0, 5.5, "в"), word(10.5, 11.0, "г")];
        let got: Vec<_> = utterances(&words, &segments).into_iter().map(|u| (u.speaker, u.start, u.end, u.text, u.words.len())).collect();
        assert_eq!(
            got,
            [(0, 0.0, 1.5, "а б".to_string(), 2), (0, 5.0, 5.5, "в".to_string(), 1), (1, 10.5, 11.0, "г".to_string(), 1)]
        );
    }
}
