//! Sound coming in: a device opened, followed when it goes away or the system's
//! default moves, and what it gives brought to one 16 kHz track with its peak.
//! Recording a call reads two of these, `infra::meeting_recorder`; both sides
//! of it are here, the microphone and the output's loopback, because they
//! differ by one call.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{ErrorKind, FromSample, SampleFormat, SizedSample, StreamInstant};
use sherpa_onnx::LinearResampler;

use crate::domain::dictation::SoundIn;
use crate::domain::meeting::MeetingError;
use crate::domain::recording::{self, Microphone, Source, RATE};

/// How often, in tenth-of-a-second ticks, a side is checked for a device gone
/// or the system's default moved: once a second.
// ponytail: asked each second, not told — cpal says nothing when the default moves
// from a device still there (macOS' loopback, a microphone); a listener of our own
// on Core Audio and IMMNotificationClient if a second's delay is ever too long.
pub(super) const CHECK: u32 = 10;

/// Sound as a device gave it: mono at its rate, and when it was captured.
struct Chunk {
    at: StreamInstant,
    samples: Vec<f32>,
}

/// A device's `buffer` of `channels` captured `at`, as one track — silence
/// when `muted` then, whenever it is written.
fn chunk<T>(buffer: &[T], at: StreamInstant, channels: usize, muted: bool) -> Chunk
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let samples = if muted {
        vec![0.0; buffer.len() / channels.max(1)]
    } else {
        let samples: Vec<f32> = buffer.iter().map(|s| s.to_sample::<f32>()).collect();
        recording::mono(&samples, channels).collect()
    };
    Chunk { at, samples }
}

/// One device's sound as it arrives and as it waits to be written, at 16 kHz,
/// each piece where it was heard.
pub(super) struct Track {
    arrived: Arc<Mutex<Vec<Chunk>>>,
    /// The device's rate, which `arrived` is at.
    rate: u32,
    /// `None` when the device is at 16 kHz already.
    resampler: Option<LinearResampler>,
    pub(super) pending: Vec<f32>,
    /// Frames of it written by now, the silence written for it included.
    taken: u64,
    /// Where the last sound to arrive ended, nanoseconds into the recording.
    pub(super) end: Option<u64>,
}

impl Track {
    fn new(rate: u32) -> Result<Self, MeetingError> {
        let mut track = Track { arrived: Arc::default(), rate, resampler: None, pending: Vec::new(), taken: 0, end: None };
        track.tune(rate)?;
        Ok(track)
    }

    /// What arrives from now on comes at `rate`.
    fn tune(&mut self, rate: u32) -> Result<(), MeetingError> {
        self.resampler = match rate {
            RATE => None,
            rate => Some(LinearResampler::create(rate as i32, RATE as i32).ok_or(MeetingError::ModelFailed("resampler"))?),
        };
        self.rate = rate;
        Ok(())
    }

    /// What arrived since the last pull, at 16 kHz, onto what waits — after
    /// silence where the device gave nothing (`recording::silence_before`),
    /// timed from `origin`, the first sound of either side. Its peak. `flush`:
    /// the resampler's own last samples too.
    pub(super) fn pull(&mut self, origin: &mut Option<StreamInstant>, flush: bool) -> f32 {
        let arrived = std::mem::take(&mut *self.arrived.lock().unwrap_or_else(|e| e.into_inner()));
        let mut level = 0.0f32;
        for Chunk { at, samples } in arrived {
            let at = at.saturating_duration_since(*origin.get_or_insert(at)).as_nanos() as u64;
            let gap = recording::silence_before(at, self.end, self.taken + self.pending.len() as u64);
            self.pending.extend(std::iter::repeat_n(0.0, gap as usize));
            self.end = Some(at + samples.len() as u64 * 1_000_000_000 / self.rate as u64);
            level = level.max(recording::peak(&samples));
            match &self.resampler {
                Some(resampler) => self.pending.extend(resampler.resample(&samples, false)),
                None => self.pending.extend(samples),
            }
        }
        if let (true, Some(resampler)) = (flush, &self.resampler) {
            self.pending.extend(resampler.resample(&[], true));
        }
        level
    }

    /// `frames` written: what waited, and silence past it for a track behind.
    pub(super) fn take(&mut self, frames: usize) {
        self.pending.drain(..frames.min(self.pending.len()));
        self.taken = self.taken.saturating_add(frames as u64);
    }

    /// When the first sound waiting to be pulled was captured.
    pub(super) fn first_at(&self) -> Option<StreamInstant> {
        self.arrived.lock().unwrap_or_else(|e| e.into_inner()).first().map(|c| c.at)
    }
}

/// One source of sound: its device's stream as it is now, and its track.
pub(super) struct Input {
    source: Source,
    /// The device asked for, opened whenever it is plugged in; the default otherwise.
    pub(super) chosen: Option<cpal::DeviceId>,
    /// Set once `chosen` is there but would not open: the default is used until
    /// it is unplugged and back.
    refused: bool,
    /// `None` while no device of its kind opens; tried again by [`Input::follow`].
    pub(super) stream: Option<cpal::Stream>,
    /// The device it is open on, to tell when the system's default moves.
    device: Option<cpal::DeviceId>,
    /// Set by the stream once it gives no more sound.
    pub(super) broken: Arc<AtomicBool>,
    /// Its sound captured as silence while set, read as it is captured.
    muted: Arc<AtomicBool>,
    pub(super) track: Track,
}

impl Input {
    /// `source`'s side, no device open yet.
    pub(super) fn new(source: Source, muted: Arc<AtomicBool>) -> Result<Self, MeetingError> {
        Ok(Input { source, chosen: None, refused: false, stream: None, device: None, broken: Arc::default(), muted, track: Track::new(RATE)? })
    }

    /// `source`'s default device, open and playing.
    #[cfg(test)]
    pub(super) fn open(host: &cpal::Host, source: Source) -> Result<Self, MeetingError> {
        let mut input = Input::new(source, Arc::default())?;
        input.connect(host)?;
        Ok(input)
    }

    /// The chosen device if it opens, else the default.
    pub(super) fn connect(&mut self, host: &cpal::Host) -> Result<(), MeetingError> {
        if let Some(device) = self.chosen_device(host) {
            match self.open_on(&device) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    eprintln!("audio input: the chosen microphone did not open, the default records instead: {e}");
                    self.refused = true;
                }
            }
        }
        self.open_on(&default_device(host, self.source)?)
    }

    fn open_on(&mut self, device: &cpal::Device) -> Result<(), MeetingError> {
        let config = match self.source {
            Source::Microphone => device.default_input_config(),
            // Loopback: the output device opened for input, in its output format.
            Source::Call => device.default_output_config(),
        }
        .map_err(device_error)?;
        self.track.tune(config.sample_rate())?;
        let broken = Arc::new(AtomicBool::new(false));
        let stream = open(device, &config, self.track.arrived.clone(), broken.clone(), self.muted.clone())?;
        stream.play().map_err(device_error)?;
        (self.stream, self.device, self.broken) = (Some(stream), device.id().ok(), broken);
        Ok(())
    }

    /// The device chosen while it is plugged in and has not refused to open.
    /// One unplugged is forgiven: plugged back, it is tried again.
    fn chosen_device(&mut self, host: &cpal::Host) -> Option<cpal::Device> {
        let found = self.chosen.as_ref().and_then(|id| host.device_by_id(id));
        if found.is_none() {
            self.refused = false;
        }
        found.filter(|_| !self.refused)
    }

    /// Opened again when its stream broke, the device it should be on moved —
    /// the default, or the chosen one plugged in or out — or it is not open;
    /// what the old one gave goes first, at its own rate.
    pub(super) fn follow(&mut self, host: &cpal::Host, origin: &mut Option<StreamInstant>) {
        let wanted = match self.chosen_device(host) {
            Some(device) => device.id().ok(),
            None => default_device(host, self.source).ok().and_then(|d| d.id().ok()),
        };
        if self.stream.is_some() && wanted == self.device && !self.broken.load(Ordering::Relaxed) {
            return;
        }
        self.stream = None;
        self.device = None;
        self.track.pull(origin, true);
        // ponytail: a side that does not open is tried again each second, logged each time.
        if let Err(e) = self.connect(host) {
            eprintln!("audio input: {:?} not open again: {e}", self.source);
        }
    }
}

/// The microphone alone, as a dictation reads it, a tick at a time. Its stream
/// stays on the thread that opened it, as a recording's does, and closes with it.
pub struct Capture {
    host: cpal::Host,
    input: Input,
    origin: Option<StreamInstant>,
    ticks: u32,
}

impl Capture {
    /// `microphone` as the settings keep it, else the default, open and playing.
    pub fn open(microphone: Option<&str>) -> Result<Self, MeetingError> {
        let mut capture = Capture::closed(microphone)?;
        capture.input.connect(&capture.host)?;
        Ok(capture)
    }

    fn closed(microphone: Option<&str>) -> Result<Self, MeetingError> {
        let mut input = Input::new(Source::Microphone, Arc::default())?;
        input.chosen = parse_chosen(microphone);
        Ok(Capture { host: cpal::default_host(), input, origin: None, ticks: 0 })
    }
}

impl SoundIn for Capture {
    fn pull(&mut self) -> Vec<f32> {
        self.ticks += 1;
        if self.ticks % CHECK == 0 {
            self.input.follow(&self.host, &mut self.origin);
        }
        let track = &mut self.input.track;
        track.pull(&mut self.origin, false);
        let sound = std::mem::take(&mut track.pending);
        track.take(sound.len());
        sound
    }
}

fn default_device(host: &cpal::Host, source: Source) -> Result<cpal::Device, MeetingError> {
    match source {
        Source::Microphone => host.default_input_device().ok_or(MeetingError::NoDevice("microphone")),
        Source::Call => host.default_output_device().ok_or(MeetingError::NoDevice("sound output")),
    }
}

/// The chosen microphone's id as the settings keep it; one that does not
/// parse — kept on another system — is none, and the default records.
pub(super) fn parse_chosen(microphone: Option<&str>) -> Option<cpal::DeviceId> {
    microphone.and_then(|m| m.parse().ok())
}

/// The microphones plugged in, by the system's name, to choose one to record from.
pub fn microphones() -> Result<Vec<Microphone>, MeetingError> {
    let devices = cpal::default_host().input_devices().map_err(device_error)?;
    Ok(devices
        .filter_map(|d| {
            let id = d.id().ok()?.to_string();
            let name = d.description().map(|n| n.name().to_string()).unwrap_or_else(|_| id.clone());
            Some(Microphone { id, name })
        })
        .collect())
}

fn device_error(e: cpal::Error) -> MeetingError {
    MeetingError::Device(e.to_string())
}

/// Whether a stream's error means it gives no more sound. Rerouted (cpal
/// followed the default itself), a glitch or no real-time priority, it goes on.
fn ends_stream(kind: ErrorKind) -> bool {
    !matches!(kind, ErrorKind::DeviceChanged | ErrorKind::Xrun | ErrorKind::RealtimeDenied)
}

fn open(device: &cpal::Device, config: &cpal::SupportedStreamConfig, arrived: Arc<Mutex<Vec<Chunk>>>, broken: Arc<AtomicBool>, muted: Arc<AtomicBool>) -> Result<cpal::Stream, MeetingError> {
    let channels = config.channels() as usize;
    match config.sample_format() {
        SampleFormat::F32 => input::<f32>(device, config, channels, arrived, broken, muted),
        SampleFormat::I16 => input::<i16>(device, config, channels, arrived, broken, muted),
        SampleFormat::I32 => input::<i32>(device, config, channels, arrived, broken, muted),
        SampleFormat::U16 => input::<u16>(device, config, channels, arrived, broken, muted),
        other => Err(MeetingError::Device(format!("the device gives {other:?} samples, which this does not read"))),
    }
}

fn input<T>(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    channels: usize,
    arrived: Arc<Mutex<Vec<Chunk>>>,
    broken: Arc<AtomicBool>,
    muted: Arc<AtomicBool>,
) -> Result<cpal::Stream, MeetingError>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    device
        .build_input_stream(
            config.config(),
            move |buffer: &[T], info: &cpal::InputCallbackInfo| {
                let chunk = chunk(buffer, info.timestamp().capture, channels, muted.load(Ordering::Relaxed));
                arrived.lock().unwrap_or_else(|e| e.into_inner()).push(chunk);
            },
            move |e: cpal::Error| {
                eprintln!("audio input: {e}");
                if ends_stream(e.kind()) {
                    broken.store(true, Ordering::Relaxed);
                }
            },
            None,
        )
        .map_err(device_error)
}

#[cfg(test)]
/// `samples` as a device gives them, captured `ms` into the recording.
pub(super) fn give(track: &Track, ms: u64, samples: impl IntoIterator<Item = f32>) {
    let chunk = Chunk { at: StreamInstant::from_millis(1000 + ms), samples: samples.into_iter().collect() };
    track.arrived.lock().unwrap().push(chunk);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_capture_gives_what_came_once() {
        let mut capture = Capture::closed(None).unwrap();
        give(&capture.input.track, 0, [0.25, 0.5]);
        assert_eq!(capture.pull(), [0.25, 0.5]);
        assert!(capture.pull().is_empty(), "given already");
        give(&capture.input.track, 1, [0.75]);
        assert_eq!(capture.pull(), [0.75], "no gap put before sound that goes straight on");
    }

    #[test]
    fn a_hole_in_a_captures_sound_is_as_long_as_it_was() {
        let mut capture = Capture::closed(None).unwrap();
        give(&capture.input.track, 0, std::iter::repeat_n(0.5, RATE as usize));
        assert_eq!(capture.pull().len(), RATE as usize);
        give(&capture.input.track, 2000, [0.5]);
        assert_eq!(capture.pull().len(), RATE as usize + 1, "a second of silence, the second given not counted twice");
    }

    #[test]
    fn a_capture_opens_the_microphone_chosen() {
        let id = cpal::DeviceId::new(cpal::default_host().id(), "Built-In:Mic:1");
        assert_eq!(Capture::closed(Some(&id.to_string())).unwrap().input.chosen, Some(id));
    }

    #[test]
    fn a_chosen_microphone_is_kept_as_its_id_and_one_from_elsewhere_is_the_default() {
        let id = cpal::DeviceId::new(cpal::default_host().id(), "Built-In:Mic:1");
        assert_eq!(parse_chosen(Some(&id.to_string())), Some(id));
        assert_eq!(parse_chosen(Some("nohost:device")), None);
        assert_eq!(parse_chosen(Some("no colon")), None);
        assert_eq!(parse_chosen(None), None);
    }

    #[test]
    fn a_track_at_another_rate_arrives_at_16_khz() {
        let mut origin = None;
        let mut track = Track::new(48_000).unwrap();
        give(&track, 0, std::iter::repeat_n(0.5, 48_000));
        assert_eq!(track.pull(&mut origin, false), 0.5, "its level");
        assert_eq!(track.pull(&mut origin, false), 0.0, "nothing new is silence");
        assert!(track.pending.len() < RATE as usize, "the resampler keeps its last few for what comes next");
        track.pull(&mut origin, true);
        assert_eq!(track.pending.len(), RATE as usize, "a second is a second once flushed");
        track.take(10);
        track.take(usize::MAX);
        assert!(track.pending.is_empty());

        let mut same = Track::new(RATE).unwrap();
        give(&same, 0, [0.1, 0.2]);
        same.pull(&mut origin, false);
        assert_eq!(same.pending, [0.1, 0.2], "already at 16 kHz: as it came");
        assert!(same.arrived.lock().unwrap().is_empty(), "taken, not copied");
    }

    #[test]
    fn a_pulls_level_is_the_loudest_of_what_arrived() {
        let mut origin = None;
        let mut track = Track::new(RATE).unwrap();
        give(&track, 0, [0.8]);
        give(&track, 10, [0.1]);
        assert_eq!(track.pull(&mut origin, false), 0.8, "a quiet piece last does not hide a loud one");
    }

    #[test]
    fn a_device_retuned_is_timed_at_its_own_rate() {
        let mut origin = None;
        let mut track = Track::new(RATE).unwrap();
        track.tune(48_000).unwrap();
        give(&track, 0, std::iter::repeat_n(0.5, 48_000));
        track.pull(&mut origin, true);
        assert_eq!(track.end, Some(1_000_000_000), "48,000 samples of it are a second");
    }

    #[test]
    fn sound_captured_muted_is_silence_whenever_it_is_written() {
        let at = StreamInstant::from_millis(1000);
        assert_eq!(chunk(&[0.5f32, 0.3, -0.2, 0.0], at, 2, false).samples, [0.4, -0.1], "heard: one track of its channels");
        let muted = chunk(&[0.5f32, 0.3, -0.2, 0.0], at, 2, true);
        assert_eq!(muted.samples, [0.0, 0.0], "muted: silence, as long as it was");
        assert_eq!(chunk(&[i16::MIN; 3], at, 1, false).samples, [-1.0; 3], "its format read as samples");

        // Unmuted before it is pulled, it stays as it was captured.
        let mut origin = None;
        let mut track = Track::new(RATE).unwrap();
        track.arrived.lock().unwrap().push(muted);
        assert_eq!(track.pull(&mut origin, false), 0.0, "silent on the meter");
        assert_eq!(track.pending, [0.0, 0.0], "and in the file");
    }

    #[test]
    fn a_device_silent_for_a_while_comes_back_where_it_is_heard() {
        let mut origin = None;
        let mut track = Track::new(RATE).unwrap();
        give(&track, 0, std::iter::repeat_n(0.5, 1600)); // 0.1 s
        give(&track, 101, std::iter::repeat_n(0.5, 1600)); // a millisecond late: it follows on
        track.pull(&mut origin, false);
        assert_eq!(track.pending.len(), 3200);

        // Nothing for a while: what was written past its end as it lagged counts.
        track.take(8000);
        give(&track, 1000, std::iter::repeat_n(0.25, 160));
        track.pull(&mut origin, false);
        assert_eq!((track.taken, track.pending.len()), (8000, 16_000 - 8000 + 160), "a second in, at a second");
        assert!(track.pending[..8000].iter().all(|&s| s == 0.0) && track.pending[8000] == 0.25);

        // A second device, opened later, starts where it was first heard, not at the start.
        let mut late = Track::new(RATE).unwrap();
        give(&late, 500, [0.5]);
        late.pull(&mut origin, false);
        assert_eq!(late.pending.len(), 8001);
    }

    #[test]
    fn a_stream_is_given_up_only_when_it_gives_no_more() {
        assert!(ends_stream(ErrorKind::DeviceNotAvailable), "unplugged");
        assert!(ends_stream(ErrorKind::StreamInvalidated), "Windows' default moved, or the rate");
        assert!(!ends_stream(ErrorKind::DeviceChanged), "cpal rerouted it itself");
        assert!(!ends_stream(ErrorKind::Xrun));
        assert!(!ends_stream(ErrorKind::RealtimeDenied));
    }
}
