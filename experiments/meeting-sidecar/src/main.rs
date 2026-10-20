//! MN-0 of `docs/36-meeting-notes-research.md`: the sidecar shape, tried for real.
//!
//! One process per recording. The app would start it, read JSON lines from its
//! stdout, and lose nothing but the line "failed" if a native library crashes
//! inside it — which is the reason it is a process and not a thread.
//!
//! ```text
//! meeting-sidecar <audio> --models <dir> [--asr punct|plain] [--embedding wespeaker|campplus]
//!                 [--speakers N] [--threshold F] [--threads N]
//! ```
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
use sherpa_onnx::{
    LinearResampler, OfflineRecognizer, OfflineRecognizerConfig, OfflineSpeakerDiarization,
    OfflineSpeakerDiarizationConfig, OfflineSpeakerDiarizationSegment,
};
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as AudioError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

const SAMPLE_RATE: u32 = 16_000;
/// Longest piece GigaAM is given at once. A turn longer than this is cut into
/// equal pieces, which can split a word at the seam.
// ponytail: equal cuts, VAD pauses inside the turn if seams cost WER on the bench.
const MAX_PIECE_SECONDS: f32 = 20.0;
/// Same speaker again after a pause shorter than this: one turn, not two.
const JOIN_GAP_SECONDS: f32 = 0.5;

struct Args {
    audio: PathBuf,
    models: PathBuf,
    asr: String,
    embedding: String,
    speakers: i32,
    threshold: f32,
    threads: i32,
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

    emit(progress("diarize", 0, 1));
    let segments = diarize(&args, &samples)?;
    let diarized = started.elapsed().as_secs_f32();
    emit(progress("diarize", 1, 1));

    let turns = turns(&segments);
    let recognizer = recognizer(&args)?;
    let mut utterances = Vec::with_capacity(turns.len());
    for (i, turn) in turns.iter().enumerate() {
        emit(progress("asr", i, turns.len()));
        let text = recognize(&recognizer, &samples, turn.start, turn.end);
        if !text.is_empty() {
            utterances.push(json!({
                "speaker": turn.speaker, "start": round(turn.start), "end": round(turn.end), "text": text,
            }));
        }
    }
    emit(progress("asr", turns.len(), turns.len()));
    let total = started.elapsed().as_secs_f32();

    let speakers = segments.iter().map(|s| s.speaker).max().map_or(0, |m| m + 1);
    emit(json!({
        "type": "result",
        "utterances": utterances,
        "stats": {
            "audio_seconds": round(audio_seconds),
            "speakers": speakers,
            "segments": segments.len(),
            "turns": turns.len(),
            "decode_seconds": round(decoded),
            "diarize_seconds": round(diarized - decoded),
            "asr_seconds": round(total - diarized),
            "rtf": (total / audio_seconds * 1000.0).round() / 1000.0,
        },
    }));
    Ok(())
}

fn parse_args(mut it: impl Iterator<Item = String>) -> Result<Args, String> {
    let usage = "usage: meeting-sidecar <audio> --models <dir> [--asr punct|plain] \
                 [--embedding wespeaker|campplus] [--speakers N] [--threshold F] [--threads N]";
    let audio = it.next().ok_or(usage)?;
    let mut args = Args {
        audio: PathBuf::from(audio),
        models: PathBuf::new(),
        asr: "punct".into(),
        embedding: "wespeaker".into(),
        speakers: -1,
        // wespeaker at 0.6 found the 4 people of a real call; sherpa's 0.5 split them more.
        threshold: 0.6,
        threads: 4,
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
    config.embedding.model = Some(match args.embedding.as_str() {
        "wespeaker" => file(&args.models, "wespeaker_en_voxceleb_resnet34_LM.onnx")?,
        "campplus" => file(&args.models, "3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx")?,
        other => return Err(format!("--embedding: no such model {other}")),
    });
    config.embedding.num_threads = args.threads;
    config.clustering.num_clusters = args.speakers;
    config.clustering.threshold = args.threshold;
    let diarizer = OfflineSpeakerDiarization::create(&config).ok_or("the diarization models did not load")?;
    let result = diarizer.process(samples).ok_or("diarization failed")?;
    Ok(result.sort_by_start_time())
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

/// The text of `[start, end)`, cut into pieces GigaAM takes whole.
fn recognize(recognizer: &OfflineRecognizer, samples: &[f32], start: f32, end: f32) -> String {
    let pieces = ((end - start) / MAX_PIECE_SECONDS).ceil().max(1.0) as usize;
    let step = (end - start) / pieces as f32;
    (0..pieces)
        .filter_map(|i| {
            let from = ((start + step * i as f32) * SAMPLE_RATE as f32) as usize;
            let to = (((start + step * (i + 1) as f32) * SAMPLE_RATE as f32) as usize).min(samples.len());
            let stream = recognizer.create_stream();
            stream.accept_waveform(SAMPLE_RATE as i32, samples.get(from..to)?);
            recognizer.decode(&stream);
            stream.get_result().map(|r| r.text.trim().to_string())
        })
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Debug, PartialEq)]
struct Turn {
    speaker: i32,
    start: f32,
    end: f32,
}

/// Diarization segments joined into turns: the same speaker after a short
/// pause is one turn. Overlapping speech stays two turns over the same audio,
/// each recognized — the louder voice wins both.
// ponytail: overlap transcribed twice; split by energy or drop the shorter if summaries suffer.
fn turns(segments: &[OfflineSpeakerDiarizationSegment]) -> Vec<Turn> {
    let mut turns: Vec<Turn> = Vec::new();
    for s in segments {
        match turns.last_mut() {
            Some(last) if last.speaker == s.speaker && s.start - last.end < JOIN_GAP_SECONDS => {
                last.end = last.end.max(s.end)
            }
            _ => turns.push(Turn { speaker: s.speaker, start: s.start, end: s.end }),
        }
    }
    turns
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

    #[test]
    fn a_short_pause_joins_one_speakers_segments() {
        let got = turns(&[seg(0, 0.0, 2.0), seg(0, 2.3, 4.0), seg(1, 4.1, 5.0), seg(0, 5.0, 6.0), seg(0, 7.0, 8.0)]);
        assert_eq!(
            got,
            [
                Turn { speaker: 0, start: 0.0, end: 4.0 },
                Turn { speaker: 1, start: 4.1, end: 5.0 },
                Turn { speaker: 0, start: 5.0, end: 6.0 },
                Turn { speaker: 0, start: 7.0, end: 8.0 },
            ]
        );
    }

    #[test]
    fn a_contained_segment_does_not_shorten_the_turn() {
        assert_eq!(turns(&[seg(0, 0.0, 5.0), seg(0, 1.0, 2.0)]), [Turn { speaker: 0, start: 0.0, end: 5.0 }]);
    }
}
