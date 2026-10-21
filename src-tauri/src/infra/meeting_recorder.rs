//! Recording a call: the default microphone and the default output's sound —
//! cpal's loopback, a Core Audio process tap on macOS 14.6+ and WASAPI loopback
//! on Windows — into one stereo WAV, L the microphone and R the call, 16 kHz
//! (`domain::recording`, `docs/36-meeting-notes-research.md`).
//!
//! The streams live on a thread of their own, which also writes: a second at a
//! time, each track brought to 16 kHz and the two put side by side. The header
//! says 0 until the recording stops; a file left so by a crash is sized by
//! [`repair`].

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use sherpa_onnx::LinearResampler;

use crate::domain::meeting::MeetingError;
use crate::domain::recording::{self, CHANNELS, HEADER_BYTES, RATE};

/// How often what the devices gave is written.
const TICK: Duration = Duration::from_secs(1);
/// A track this far behind the other is taken as silent (`recording::frames_ready`).
const MAX_LAG: usize = RATE as usize;

/// The recording under way, if one is. One at a time: there is one microphone.
#[derive(Default)]
pub struct Recorder(Mutex<Option<Session>>);

struct Session {
    id: String,
    stop: mpsc::Sender<()>,
    thread: JoinHandle<Result<(), MeetingError>>,
}

impl Recorder {
    /// Records into `path` as `id` from now on, once both devices are open.
    pub fn start(&self, id: &str, path: &Path) -> Result<(), MeetingError> {
        let mut slot = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_some() {
            return Err(MeetingError::AlreadyRecording);
        }
        let (stop, stopped) = mpsc::channel();
        let (ready, opened) = mpsc::sync_channel(1);
        let path = path.to_path_buf();
        let thread = std::thread::spawn(move || record(&path, &stopped, &ready));
        match opened.recv() {
            Ok(Ok(())) => {
                *slot = Some(Session { id: id.to_string(), stop, thread });
                Ok(())
            }
            Ok(Err(e)) => Err(e),
            Err(_) => Err(MeetingError::Device("the recording thread ended before it began".into())),
        }
    }

    /// The recording ended and its file whole; its id.
    pub fn stop(&self) -> Result<String, MeetingError> {
        let session = self.0.lock().unwrap_or_else(|e| e.into_inner()).take().ok_or(MeetingError::NotRecording)?;
        let _ = session.stop.send(());
        session.thread.join().map_err(|_| MeetingError::Device("the recording thread failed".into()))??;
        Ok(session.id)
    }
}

/// The recording thread: devices opened (said on `ready`), then a write each
/// [`TICK`] until `stop`, then the rest and the header's sizes.
fn record(path: &Path, stop: &Receiver<()>, ready: &SyncSender<Result<(), MeetingError>>) -> Result<(), MeetingError> {
    let opened = open_devices().and_then(|(streams, mic, call)| Ok((streams, WavWriter::create(path)?, mic, call)));
    let (streams, mut file, mut mic, mut call) = match opened {
        Ok(opened) => opened,
        Err(e) => {
            let _ = ready.send(Err(e));
            return Ok(());
        }
    };
    let _ = ready.send(Ok(()));
    let mut streams = Some(streams);
    loop {
        let flush = !matches!(stop.recv_timeout(TICK), Err(RecvTimeoutError::Timeout));
        if flush {
            // Closed first: nothing arrives after what is flushed.
            streams.take();
        }
        mic.pull(flush);
        call.pull(flush);
        let frames = recording::frames_ready(mic.pending.len(), call.pending.len(), MAX_LAG, flush);
        let full = file.append(&recording::interleave(&mic.pending, &call.pending, frames))?;
        mic.take(frames);
        call.take(frames);
        if flush || full {
            return file.finish();
        }
    }
}

/// One device's sound as it arrives — mono, at the device's rate — and as it
/// waits to be written, at 16 kHz.
struct Track {
    arrived: Arc<Mutex<Vec<f32>>>,
    /// `None` when the device is at 16 kHz already.
    resampler: Option<LinearResampler>,
    pending: Vec<f32>,
}

impl Track {
    fn new(rate: u32) -> Result<Self, MeetingError> {
        let resampler = match rate {
            RATE => None,
            rate => Some(LinearResampler::create(rate as i32, RATE as i32).ok_or(MeetingError::ModelFailed("resampler"))?),
        };
        Ok(Track { arrived: Arc::default(), resampler, pending: Vec::new() })
    }

    /// What arrived since the last pull, at 16 kHz, onto what waits. `flush`:
    /// the resampler's own last samples too.
    fn pull(&mut self, flush: bool) {
        let arrived = std::mem::take(&mut *self.arrived.lock().unwrap_or_else(|e| e.into_inner()));
        match &self.resampler {
            Some(resampler) => self.pending.extend(resampler.resample(&arrived, flush)),
            None => self.pending.extend(arrived),
        }
    }

    fn take(&mut self, frames: usize) {
        self.pending.drain(..frames.min(self.pending.len()));
    }
}

/// The two streams, playing, and the tracks they fill.
fn open_devices() -> Result<([cpal::Stream; 2], Track, Track), MeetingError> {
    let host = cpal::default_host();
    let microphone = host.default_input_device().ok_or(MeetingError::NoDevice("microphone"))?;
    let speakers = host.default_output_device().ok_or(MeetingError::NoDevice("sound output"))?;
    let device = |e: cpal::Error| MeetingError::Device(e.to_string());
    let mic_config = microphone.default_input_config().map_err(device)?;
    // Loopback: the output device opened for input, in its output format.
    let call_config = speakers.default_output_config().map_err(device)?;
    let mic = Track::new(mic_config.sample_rate())?;
    let call = Track::new(call_config.sample_rate())?;
    let streams = [open(&microphone, &mic_config, &mic)?, open(&speakers, &call_config, &call)?];
    for stream in &streams {
        stream.play().map_err(device)?;
    }
    Ok((streams, mic, call))
}

fn open(device: &cpal::Device, config: &cpal::SupportedStreamConfig, track: &Track) -> Result<cpal::Stream, MeetingError> {
    let channels = config.channels() as usize;
    let arrived = track.arrived.clone();
    match config.sample_format() {
        SampleFormat::F32 => input::<f32>(device, config, channels, arrived),
        SampleFormat::I16 => input::<i16>(device, config, channels, arrived),
        SampleFormat::I32 => input::<i32>(device, config, channels, arrived),
        SampleFormat::U16 => input::<u16>(device, config, channels, arrived),
        other => Err(MeetingError::Device(format!("the device gives {other:?} samples, which this does not read"))),
    }
}

fn input<T>(device: &cpal::Device, config: &cpal::SupportedStreamConfig, channels: usize, arrived: Arc<Mutex<Vec<f32>>>) -> Result<cpal::Stream, MeetingError>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    device
        .build_input_stream(
            config.config(),
            move |buffer: &[T], _: &cpal::InputCallbackInfo| {
                let samples: Vec<f32> = buffer.iter().map(|s| s.to_sample::<f32>()).collect();
                arrived.lock().unwrap_or_else(|e| e.into_inner()).extend(recording::mono(&samples, channels));
            },
            // ponytail: a stream's error is only logged; R-3 opens it again (a device unplugged, the output changed).
            |e| eprintln!("meeting recorder: {e}"),
            None,
        )
        .map_err(|e| MeetingError::Device(e.to_string()))
}

/// The recording's file: a header saying 0, then frames as they come.
struct WavWriter {
    file: File,
    written: u32,
}

impl WavWriter {
    fn create(path: &Path) -> Result<Self, MeetingError> {
        let mut file = File::create(path).map_err(store)?;
        file.write_all(&recording::wav_header(CHANNELS, 0)).map_err(store)?;
        Ok(WavWriter { file, written: 0 })
    }

    /// `bytes` appended, as far as a WAV holds; `true` once it holds no more.
    fn append(&mut self, bytes: &[u8]) -> Result<bool, MeetingError> {
        let room = (recording::max_data_bytes(CHANNELS) - self.written) as usize;
        let bytes = &bytes[..bytes.len().min(room)];
        self.file.write_all(bytes).map_err(store)?;
        self.written += bytes.len() as u32;
        Ok(bytes.len() == room)
    }

    /// The header's sizes put in: the file is whole.
    fn finish(mut self) -> Result<(), MeetingError> {
        self.file.flush().map_err(store)?;
        write_sizes(&mut self.file, self.written)
    }
}

/// A recording's header sized by the file's length, if it says otherwise —
/// one left by a crash says 0 and would read as nothing. `true` when it was.
pub fn repair(path: &Path) -> Result<bool, MeetingError> {
    let mut file = OpenOptions::new().read(true).write(true).open(path).map_err(store)?;
    let mut header = [0u8; HEADER_BYTES as usize];
    file.read_exact(&mut header).map_err(store)?;
    if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" || &header[36..40] != b"data" {
        return Err(MeetingError::Unreadable(format!("{} is not a recording of this app's", path.display())));
    }
    let channels = u16::from_le_bytes([header[22], header[23]]);
    let len = file.metadata().map_err(store)?.len();
    let data = recording::data_bytes_of(len, channels);
    if header[40..44] == data.to_le_bytes() {
        return Ok(false);
    }
    write_sizes(&mut file, data)?;
    Ok(true)
}

fn write_sizes(file: &mut File, data: u32) -> Result<(), MeetingError> {
    let header = recording::wav_header(CHANNELS, data);
    file.seek(SeekFrom::Start(4)).and_then(|_| file.write_all(&header[4..8])).map_err(store)?;
    file.seek(SeekFrom::Start(40)).and_then(|_| file.write_all(&header[40..44])).map_err(store)?;
    file.sync_all().map_err(store)
}

fn store(e: impl std::fmt::Display) -> MeetingError {
    MeetingError::Store(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp(label: &str) -> PathBuf {
        crate::testing::temp_dir(label).join("call.wav")
    }

    /// A recording of `mic` and `call` as the recorder leaves one when stopped.
    fn written(path: PathBuf, mic: &[f32], call: &[f32]) -> PathBuf {
        let mut file = WavWriter::create(&path).unwrap();
        file.append(&recording::interleave(mic, call, mic.len().max(call.len()))).unwrap();
        file.finish().unwrap();
        path
    }

    #[test]
    fn a_finished_recording_reads_as_both_tracks() {
        let mic: Vec<f32> = (0..RATE).map(|i| (i as f32 / 30.0).sin() * 0.4).collect();
        let call: Vec<f32> = (0..RATE / 2).map(|i| (i as f32 / 11.0).sin() * 0.4).collect();
        let path = written(temp("recorder-whole"), &mic, &call);
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(bytes.len() as u64, HEADER_BYTES + 4 * RATE as u64);
        assert_eq!(bytes[40..44], (4 * RATE).to_le_bytes(), "the size is put in once it stops");
        assert!(!repair(&path).unwrap(), "a whole file needs nothing");

        // Read as the recognition reads it: its channels averaged.
        let (heard, rate) = crate::infra::speech::decode(&path).unwrap();
        assert_eq!((rate, heard.len()), (RATE, RATE as usize));
        let expected = |i: usize| (mic[i] + call.get(i).copied().unwrap_or(0.0)) / 2.0;
        let worst = (0..heard.len()).map(|i| (heard[i] - expected(i)).abs()).fold(0.0, f32::max);
        assert!(worst < 1e-3, "{worst}");
    }

    #[test]
    fn a_recording_cut_off_reads_as_what_was_written() {
        let path = temp("recorder-cut");
        let mut file = WavWriter::create(&path).unwrap();
        file.append(&recording::interleave(&[0.5; 1000], &[0.25; 1000], 1000)).unwrap();
        file.file.write_all(&[1, 2, 3]).unwrap(); // a frame cut off midway
        drop(file); // the app crashed: no sizes
        assert!(crate::infra::speech::decode(&path).is_err(), "unreadable as it was left");

        assert!(repair(&path).unwrap());
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!((&bytes[4..8], &bytes[40..44]), (4036u32.to_le_bytes().as_slice(), 4000u32.to_le_bytes().as_slice()));
        let (heard, _) = crate::infra::speech::decode(&path).unwrap();
        assert_eq!(heard.len(), 1000);
        assert!(!repair(&path).unwrap(), "once is enough");

        std::fs::write(&path, b"not a wav file, but long enough to have a header").unwrap();
        assert!(matches!(repair(&path), Err(MeetingError::Unreadable(_))));
    }

    #[test]
    fn a_full_file_takes_no_more() {
        let path = temp("recorder-full");
        let mut file = WavWriter::create(&path).unwrap();
        file.written = recording::max_data_bytes(CHANNELS) - 8;
        assert!(!file.append(&[0; 4]).unwrap());
        assert!(file.append(&[0; 12]).unwrap(), "past what it holds is cut off, and the file is full");
        assert_eq!(file.written, recording::max_data_bytes(CHANNELS));
        assert_eq!(std::fs::metadata(&path).unwrap().len(), HEADER_BYTES + 8);
    }

    #[test]
    fn a_track_at_another_rate_arrives_at_16_khz() {
        let mut track = Track::new(48_000).unwrap();
        track.arrived.lock().unwrap().extend(std::iter::repeat_n(0.5, 48_000));
        track.pull(false);
        assert!(track.pending.len() < RATE as usize, "the resampler keeps its last few for what comes next");
        track.pull(true);
        assert_eq!(track.pending.len(), RATE as usize, "a second is a second once flushed");
        track.take(10);
        track.take(usize::MAX);
        assert!(track.pending.is_empty());

        let mut same = Track::new(RATE).unwrap();
        same.arrived.lock().unwrap().extend([0.1, 0.2]);
        same.pull(false);
        assert_eq!(same.pending, [0.1, 0.2], "already at 16 kHz: as it came");
        assert!(same.arrived.lock().unwrap().is_empty(), "taken, not copied");
    }

    /// The real microphone and output for a few seconds, a voice playing:
    /// `cargo test record_live -- --ignored --nocapture`. Run from a terminal,
    /// macOS asks for the microphone for the terminal, and the call's track is
    /// likely silence — System Audio Recording is given to an app whose
    /// `Info.plist` asks for it, so the call's sound is checked from the `.app`.
    #[test]
    #[ignore]
    fn record_live() {
        let path = temp("recorder-live");
        let recorder = Recorder::default();
        recorder.start("live", &path).unwrap();
        assert_eq!(recorder.start("again", &path), Err(MeetingError::AlreadyRecording));
        let _ = std::process::Command::new("say").args(["-v", "Milena", "Проверка записи созвона, раз, два, три"]).status();
        std::thread::sleep(Duration::from_secs(1));
        assert_eq!(recorder.stop().unwrap(), "live");
        assert!(!repair(&path).unwrap(), "stopped, its sizes are in");

        let bytes = std::fs::read(&path).unwrap();
        let samples: Vec<i16> = bytes[HEADER_BYTES as usize..].chunks(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
        let peak = |channel: usize| samples.iter().skip(channel).step_by(2).map(|s| s.unsigned_abs()).max().unwrap_or(0) as f32 / i16::MAX as f32;
        let seconds = samples.len() as f32 / 2.0 / RATE as f32;
        eprintln!("{seconds:.1} s, peak: microphone {:.3}, call {:.3} — {}", peak(0), peak(1), path.display());
        assert!(seconds > 1.0, "{seconds}");
        assert_eq!(crate::infra::speech::decode(&path).unwrap().1, RATE);
    }

    #[test]
    fn stopping_with_nothing_recorded_says_so() {
        assert_eq!(Recorder::default().stop(), Err(MeetingError::NotRecording));
    }
}
