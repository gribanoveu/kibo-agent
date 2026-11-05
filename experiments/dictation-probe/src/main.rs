//! VI-0 of `docs/52-voice-input.md`, the part one can try by voice:
//!
//! `dictation-probe <gigaam-dir> <silero_vad.onnx> <out-dir> [in.wav]`
//!
//! Starting the program is the press. The microphone opens at once and the
//! model loads while it records, so nothing said after the first buffer is
//! lost. A sound says "speak", open phrases print as they are re-recognized,
//! closed ones when VAD ends them, and Enter stops. Then the times, phrase by
//! phrase against the whole recording, and `<out-dir>/probe.wav`.
//!
//! With `in.wav` (16 kHz mono) the file plays in real time instead of the
//! microphone: that is how the probe checks itself without a voice.

use std::io::BufRead;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use sherpa_onnx::{LinearResampler, OfflineRecognizer, OfflineRecognizerConfig, SileroVadModelConfig, VadModelConfig, VoiceActivityDetector, Wave};

const RATE: usize = 16_000;
const THREADS: i32 = 4;
/// Silero's window, in samples.
const WINDOW: usize = 512;
/// What the plan proposes: a phrase ends on this pause, is cut at this length.
const PAUSE_SECONDS: f32 = 0.6;
const LONGEST_PHRASE_SECONDS: f32 = 20.0;
/// How often an open phrase is recognized again.
const PARTIAL_STEP: Duration = Duration::from_millis(500);
/// VAD says speech has begun a little after it has.
const LOOKBACK: usize = RATE / 2;
/// The microphone hears the sound that says "speak": this much goes past VAD.
const DEAF: usize = RATE * 3 / 10;
/// GigaAM was never fed longer pieces (docs/36).
const LONGEST_PASS: usize = 24 * RATE;
/// Above a digital zero: a microphone waking up delivers buffers of nothing.
const SOUND: f32 = 1e-4;

#[derive(Default)]
struct Capture {
    raw: Vec<f32>,
    first_buffer: Option<Duration>,
    first_sound: Option<Duration>,
}

impl Capture {
    fn push(&mut self, samples: impl Iterator<Item = f32>, since: Instant) {
        self.first_buffer.get_or_insert_with(|| since.elapsed());
        let before = self.raw.len();
        self.raw.extend(samples);
        if self.first_sound.is_none() && self.raw[before..].iter().any(|s| s.abs() > SOUND) {
            self.first_sound = Some(since.elapsed());
        }
    }
}

fn main() {
    let start = Instant::now();
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: dictation-probe <gigaam-dir> <silero_vad.onnx> <out-dir> [in.wav]");
        std::process::exit(2);
    }
    if let Err(e) = run(start, Path::new(&args[1]), &args[2], Path::new(&args[3]), args.get(4).map(String::as_str)) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run(start: Instant, models: &Path, vad_model: &str, out: &Path, file: Option<&str>) -> Result<(), String> {
    let capture = Arc::new(Mutex::new(Capture::default()));
    let stop = Arc::new(AtomicBool::new(false));

    // The stream runs on its own thread while the model loads on this one.
    let (stream, device_rate) = match file {
        Some(path) => (None, play_file(path, &capture, &stop, start)?),
        None => {
            let (stream, rate) = open_microphone(&capture, start)?;
            announce_first_buffer(&capture, start);
            wait_for_enter(&stop);
            (Some(stream), rate)
        }
    };

    let recognizer = recognizer(models)?;
    let vad = vad(vad_model)?;
    let loaded = start.elapsed();
    say(start, &format!("модель загружена ({} потока)", THREADS));

    let resampler = match device_rate as usize {
        RATE => None,
        rate => Some(LinearResampler::create(rate as i32, RATE as i32).ok_or("resampler")?),
    };

    let mut all: Vec<f32> = Vec::new();
    // A file has no sound saying "speak" to go deaf to.
    let deaf = if file.is_some() { 0 } else { DEAF };
    let mut fed = deaf;
    let mut open_from: Option<usize> = None;
    let mut last_partial = Instant::now();
    let mut phrases: Vec<String> = Vec::new();
    let mut passes: Vec<(f32, Duration)> = Vec::new();

    loop {
        let stopping = stop.load(Ordering::SeqCst);
        let raw = std::mem::take(&mut capture.lock().map_err(|e| e.to_string())?.raw);
        match &resampler {
            Some(r) => all.extend(r.resample(&raw, false)),
            None => all.extend(raw),
        }
        while fed + WINDOW <= all.len() {
            vad.accept_waveform(&all[fed..fed + WINDOW]);
            fed += WINDOW;
            match vad.detected() {
                true => _ = open_from.get_or_insert(fed.saturating_sub(LOOKBACK)),
                false => open_from = None,
            }
            close_phrases(&vad, &recognizer, start, deaf, &mut phrases, &mut passes, &mut open_from);
        }
        if stopping {
            break;
        }
        if let Some(from) = open_from {
            if last_partial.elapsed() >= PARTIAL_STEP {
                let (text, took) = recognize(&recognizer, &all[from..fed]);
                let seconds = (fed - from) as f32 / RATE as f32;
                passes.push((seconds, took));
                say(start, &format!("  … {text}   ({seconds:.1} с звука за {:.2} с)", took.as_secs_f32()));
                last_partial = Instant::now();
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    // The report holds the capture's lock through long passes: nothing may wait on it.
    drop(stream);
    vad.flush();
    close_phrases(&vad, &recognizer, start, deaf, &mut phrases, &mut passes, &mut open_from);

    report(&capture, loaded, &recognizer, &all, &phrases, &passes)?;
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    let path = out.join("probe.wav");
    match sherpa_onnx::write(path.to_string_lossy().as_ref(), &all, RATE as i32) {
        true => println!("запись: {}", path.display()),
        false => return Err(format!("could not write {}", path.display())),
    }
    Ok(())
}

fn close_phrases(
    vad: &VoiceActivityDetector,
    recognizer: &OfflineRecognizer,
    start: Instant,
    deaf: usize,
    phrases: &mut Vec<String>,
    passes: &mut Vec<(f32, Duration)>,
    open_from: &mut Option<usize>,
) {
    while let Some(segment) = vad.front() {
        let (text, took) = recognize(recognizer, segment.samples());
        let seconds = segment.n() as f32 / RATE as f32;
        passes.push((seconds, took));
        say(start, &format!("ФРАЗА: {text}   ({seconds:.1} с звука за {:.2} с)", took.as_secs_f32()));
        phrases.push(text);
        // A phrase cut for its length goes on: what is open starts after it.
        let end = deaf + (segment.start() + segment.n()) as usize;
        if let Some(from) = open_from {
            *from = end.max(*from);
        }
        vad.pop();
    }
}

fn report(
    capture: &Mutex<Capture>,
    loaded: Duration,
    recognizer: &OfflineRecognizer,
    all: &[f32],
    phrases: &[String],
    passes: &[(f32, Duration)],
) -> Result<(), String> {
    let c = capture.lock().map_err(|e| e.to_string())?;
    println!("\n--- от нажатия ---");
    println!("первый буфер микрофона: {}", seconds(c.first_buffer));
    println!("первый ненулевой звук:  {}", seconds(c.first_sound));
    println!("модель готова:          {:.2} с", loaded.as_secs_f32());
    if c.first_sound.is_none() {
        println!("микрофон отдал только нули: у этого терминала, похоже, нет разрешения на микрофон");
    }

    println!("\n--- текст ---");
    println!("по фразам VAD: {}", phrases.join(" "));
    let whole: Vec<String> = all.chunks(LONGEST_PASS).map(|piece| recognize(recognizer, piece).0).collect();
    println!("вся запись:    {}", whole.join(" "));

    println!("\n--- цена прохода ---");
    if let Some((_, first)) = passes.first() {
        let worst = passes.iter().map(|(_, t)| *t).max().unwrap_or_default();
        println!("проходов {}, первый {:.2} с, худший {:.2} с", passes.len(), first.as_secs_f32(), worst.as_secs_f32());
    }
    for length in [3, 8, 15] {
        if all.len() >= length * RATE {
            let (_, took) = recognize(recognizer, &all[..length * RATE]);
            println!("{length} с звука: {:.2} с", took.as_secs_f32());
        }
    }
    let rss = Command::new("ps").args(["-o", "rss=", "-p", &std::process::id().to_string()]).output().map_err(|e| e.to_string())?;
    let kb: f32 = String::from_utf8_lossy(&rss.stdout).trim().parse().unwrap_or(0.0);
    println!("память процесса: {:.0} МБ\n", kb / 1024.0);
    Ok(())
}

fn seconds(at: Option<Duration>) -> String {
    at.map_or("нет".into(), |d| format!("{:.2} с", d.as_secs_f32()))
}

fn say(start: Instant, line: &str) {
    println!("[{:6.2}] {line}", start.elapsed().as_secs_f32());
}

fn recognize(recognizer: &OfflineRecognizer, samples: &[f32]) -> (String, Duration) {
    let began = Instant::now();
    let stream = recognizer.create_stream();
    stream.accept_waveform(RATE as i32, samples);
    recognizer.decode(&stream);
    let text = stream.get_result().map(|r| r.text.trim().to_string()).unwrap_or_default();
    (text, began.elapsed())
}

fn recognizer(models: &Path) -> Result<OfflineRecognizer, String> {
    let path = |name: &str| Some(models.join(name).to_string_lossy().into_owned());
    let mut config = OfflineRecognizerConfig::default();
    config.model_config.transducer.encoder = path("encoder.int8.onnx");
    config.model_config.transducer.decoder = path("decoder.onnx");
    config.model_config.transducer.joiner = path("joiner.onnx");
    config.model_config.tokens = path("tokens.txt");
    config.model_config.model_type = Some("nemo_transducer".into());
    config.model_config.num_threads = THREADS;
    OfflineRecognizer::create(&config).ok_or_else(|| format!("no GigaAM in {}", models.display()))
}

fn vad(model: &str) -> Result<VoiceActivityDetector, String> {
    let config = VadModelConfig {
        silero_vad: SileroVadModelConfig {
            model: Some(model.into()),
            threshold: 0.5,
            min_silence_duration: PAUSE_SECONDS,
            min_speech_duration: 0.25,
            window_size: WINDOW as i32,
            max_speech_duration: LONGEST_PHRASE_SECONDS,
        },
        sample_rate: RATE as i32,
        num_threads: 1,
        ..Default::default()
    };
    VoiceActivityDetector::create(&config, 60.0).ok_or_else(|| format!("no VAD at {model}"))
}

fn open_microphone(capture: &Arc<Mutex<Capture>>, start: Instant) -> Result<(cpal::Stream, u32), String> {
    let device = cpal::default_host().default_input_device().ok_or("no input device")?;
    let config = device.default_input_config().map_err(|e| e.to_string())?;
    if config.sample_format() != cpal::SampleFormat::F32 {
        return Err(format!("sample format {:?}, the probe takes f32", config.sample_format()));
    }
    let name = device.description().map(|d| d.name().to_string()).unwrap_or_default();
    let channels = config.channels() as usize;
    let data = capture.clone();
    let stream = device
        .build_input_stream(
            config.config(),
            move |buffer: &[f32], _: &cpal::InputCallbackInfo| {
                if let Ok(mut c) = data.lock() {
                    c.push(buffer.chunks(channels).map(|frame| frame.iter().sum::<f32>() / channels as f32), start);
                }
            },
            |e| eprintln!("microphone: {e}"),
            None,
        )
        .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    say(start, &format!("микрофон открыт: {name}, {} Гц", config.sample_rate()));
    Ok((stream, config.sample_rate()))
}

/// The sound as soon as there is a buffer: not after the model, which the
/// buffered sound does not wait for.
fn announce_first_buffer(capture: &Arc<Mutex<Capture>>, start: Instant) {
    let capture = capture.clone();
    std::thread::spawn(move || loop {
        if capture.lock().map(|c| c.first_buffer.is_some()).unwrap_or(true) {
            say(start, ">>> ГОВОРИ. Enter — стоп");
            let _ = Command::new("afplay").arg("/System/Library/Sounds/Tink.aiff").spawn();
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
    });
}

fn wait_for_enter(stop: &Arc<AtomicBool>) {
    let stop = stop.clone();
    std::thread::spawn(move || {
        let _ = std::io::stdin().lock().lines().next();
        stop.store(true, Ordering::SeqCst);
    });
}

fn play_file(path: &str, capture: &Arc<Mutex<Capture>>, stop: &Arc<AtomicBool>, start: Instant) -> Result<u32, String> {
    let wave = Wave::read(path).ok_or_else(|| format!("cannot read {path}"))?;
    let rate = wave.sample_rate() as u32;
    let samples = wave.samples().to_vec();
    let (capture, stop) = (capture.clone(), stop.clone());
    std::thread::spawn(move || {
        for piece in samples.chunks(rate as usize / 10) {
            if let Ok(mut c) = capture.lock() {
                c.push(piece.iter().copied(), start);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        stop.store(true, Ordering::SeqCst);
    });
    Ok(rate)
}
