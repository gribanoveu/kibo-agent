//! `meeting-recorder <seconds> <out-dir>`: records the default microphone and
//! the default output's sound (cpal's loopback: a Core Audio process tap on
//! macOS 14.6+, WASAPI loopback on Windows) side by side, then writes
//! `mic.wav`, `system.wav` and `mix.wav` (16 kHz mono 16-bit) and `log.txt`.

use std::fmt::Write as _;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

const RATE: u32 = 16_000;

struct Track {
    rate: u32,
    samples: Vec<f32>,
    /// When the first buffer came, since start.
    first: Option<Duration>,
    callbacks: usize,
    errors: Vec<String>,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let seconds: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(20);
    let out = Path::new(args.get(2).map(String::as_str).unwrap_or("."));
    let mut log = String::new();
    if let Err(e) = run(seconds, out, &mut log) {
        let _ = writeln!(log, "error: {e}");
    }
    print!("{log}");
    let _ = std::fs::write(out.join("log.txt"), log);
}

fn run(seconds: u64, out: &Path, log: &mut String) -> Result<(), String> {
    let host = cpal::default_host();
    let mic = host.default_input_device().ok_or("no input device")?;
    let speakers = host.default_output_device().ok_or("no output device")?;
    let mic_config = mic.default_input_config().map_err(|e| e.to_string())?;
    // Loopback: the output device opened for input, in its output format.
    let sys_config = speakers.default_output_config().map_err(|e| e.to_string())?;
    let _ = writeln!(log, "mic: {:?} {mic_config:?}", mic.description().map(|d| d.name().to_string()));
    let _ = writeln!(log, "system: {:?} {sys_config:?}", speakers.description().map(|d| d.name().to_string()));

    let start = Instant::now();
    let mic_track = track(mic_config.sample_rate());
    let sys_track = track(sys_config.sample_rate());
    let mic_stream = open(&mic, &mic_config, &mic_track, start).map_err(|e| format!("mic: {e}"))?;
    let sys_stream = open(&speakers, &sys_config, &sys_track, start).map_err(|e| format!("system: {e}"))?;
    mic_stream.play().map_err(|e| e.to_string())?;
    sys_stream.play().map_err(|e| e.to_string())?;
    std::thread::sleep(Duration::from_secs(seconds));
    drop(mic_stream);
    drop(sys_stream);

    let mic = report("mic", &mic_track, log);
    let sys = report("system", &sys_track, log);
    let len = mic.len().max(sys.len());
    let mix: Vec<f32> = (0..len)
        .map(|i| (mic.get(i).unwrap_or(&0.0) + sys.get(i).unwrap_or(&0.0)).clamp(-1.0, 1.0))
        .collect();
    for (name, samples) in [("mic.wav", &mic), ("system.wav", &sys), ("mix.wav", &mix)] {
        std::fs::write(out.join(name), wav(samples)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn track(rate: u32) -> Arc<Mutex<Track>> {
    Arc::new(Mutex::new(Track { rate, samples: Vec::new(), first: None, callbacks: 0, errors: Vec::new() }))
}

fn open(device: &cpal::Device, config: &cpal::SupportedStreamConfig, track: &Arc<Mutex<Track>>, start: Instant) -> Result<cpal::Stream, String> {
    if config.sample_format() != cpal::SampleFormat::F32 {
        return Err(format!("sample format {:?}, the spike takes f32", config.sample_format()));
    }
    let channels = config.channels() as usize;
    let (data, error) = (track.clone(), track.clone());
    device
        .build_input_stream(
            config.config(),
            move |buffer: &[f32], _: &cpal::InputCallbackInfo| {
                let mut t = data.lock().unwrap();
                t.first.get_or_insert_with(|| start.elapsed());
                t.callbacks += 1;
                t.samples.extend(buffer.chunks(channels).map(|frame| frame.iter().sum::<f32>() / channels as f32));
            },
            move |e| error.lock().unwrap().errors.push(e.to_string()),
            None,
        )
        .map_err(|e| e.to_string())
}

/// The track at 16 kHz, and a line on how it went.
fn report(name: &str, track: &Arc<Mutex<Track>>, log: &mut String) -> Vec<f32> {
    let t = track.lock().unwrap();
    let samples = resample(&t.samples, t.rate);
    let peak = samples.iter().fold(0f32, |m, s| m.max(s.abs()));
    let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len().max(1) as f32).sqrt();
    let silent = samples.iter().filter(|s| s.abs() < 1e-6).count() as f32 / samples.len().max(1) as f32;
    let _ = writeln!(
        log,
        "{name}: {:.1} s at {} Hz, {} callbacks, first after {:?}, peak {peak:.3}, rms {rms:.4}, exact zeros {:.0} %, errors {:?}",
        t.samples.len() as f32 / t.rate as f32,
        t.rate,
        t.callbacks,
        t.first,
        silent * 100.0,
        t.errors
    );
    samples
}

/// Linear: enough for a spike, the app has sherpa's resampler.
fn resample(samples: &[f32], rate: u32) -> Vec<f32> {
    let step = rate as f64 / RATE as f64;
    let n = (samples.len() as f64 / step) as usize;
    (0..n)
        .map(|i| {
            let at = i as f64 * step;
            let (j, frac) = (at as usize, (at.fract()) as f32);
            let a = samples[j];
            let b = *samples.get(j + 1).unwrap_or(&a);
            a + (b - a) * frac
        })
        .collect()
}

fn wav(samples: &[f32]) -> Vec<u8> {
    let data = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&((s * i16::MAX as f32) as i16).to_le_bytes());
    }
    out
}
