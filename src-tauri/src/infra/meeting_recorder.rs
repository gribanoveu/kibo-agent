//! Recording a call: the microphone chosen, else the default, and the default output's sound —
//! cpal's loopback, a Core Audio process tap on macOS 14.6+ and WASAPI loopback
//! on Windows — into one stereo WAV, L the microphone and R the call, 16 kHz
//! (`domain::recording`, `docs/36-meeting-notes-research.md`).
//!
//! The streams live on a thread of their own, which also writes: a tenth of a
//! second at a time, each track brought to 16 kHz and the two put side by
//! side, and how loud each was told to whoever started it. Each device's
//! sound goes where it was captured, so one that gives nothing for a while —
//! Windows' loopback while nothing plays — or is opened again after the
//! system's default moved leaves silence there, not a shift. The header
//! says 0 until the recording stops; a file left so by a crash is sized by
//! [`repair`].

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::domain::meeting::MeetingError;
use crate::domain::recording::{self, Levels, RecordingState, Source, CHANNELS, HEADER_BYTES, RATE};
use crate::infra::microphone::{parse_chosen, Input, CHECK};

/// How often what the devices gave is written, and its levels told: ten a
/// second at most, the throttle of a source that changes fast (AGENTS.md).
const TICK: Duration = Duration::from_millis(100);
/// A track whose sound ended this long before the other's is taken as silent (`recording::frames_ready`).
const MAX_LAG: usize = RATE as usize;

/// The recording under way, if one is. One at a time: there is one microphone.
#[derive(Default)]
pub struct Recorder(Mutex<Slot>);

/// What a recording tells as it goes.
pub type LevelSink = Box<dyn Fn(Levels) + Send>;
/// Told once when a recording ends without a Stop — the disk failed, or the
/// file holds no more — and why. Its file is whole as far as it was written.
pub type EndSink = Box<dyn FnOnce(MeetingError) + Send>;

#[derive(Default)]
enum Slot {
    #[default]
    Free,
    /// Its devices opening, off the lock: the window's own reads of the
    /// recording take it on the main thread. `true` once a stop came meanwhile
    /// — Meeting notes turned off — and it is not to begin.
    Starting(bool),
    Recording(Session),
    /// Stopped, its file being finished: still this recording's, not one to list.
    Stopping(String),
}

struct Session {
    id: String,
    stop: mpsc::Sender<()>,
    thread: JoinHandle<Result<(), MeetingError>>,
    /// Frames written by now.
    frames: Arc<AtomicU64>,
    /// The microphone written as silence while set.
    muted: Arc<AtomicBool>,
    /// Set once it ended by itself, before that is told: no recording any more.
    done: Arc<AtomicBool>,
}

impl Recorder {
    /// Records into `path` as `id` from now on, once a device is open, from
    /// `microphone` (a [`Microphone`]'s id) while it is plugged in and the
    /// default otherwise; how loud each side is goes to `levels` as it
    /// records, and an end without [`stop`](Self::stop) to `ended`.
    pub fn start(&self, id: &str, path: &Path, microphone: Option<&str>, levels: LevelSink, ended: EndSink) -> Result<(), MeetingError> {
        {
            let mut slot = self.slot();
            if !matches!(*slot, Slot::Free) {
                return Err(MeetingError::AlreadyRecording);
            }
            *slot = Slot::Starting(false);
        }
        let (stop, stopped) = mpsc::channel();
        let (ready, opened) = mpsc::sync_channel(1);
        let (path, path_kept) = (path.to_path_buf(), path.to_path_buf());
        let chosen = parse_chosen(microphone);
        let (frames, muted, done) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        let (written, silent, over) = (frames.clone(), muted.clone(), done.clone());
        let thread = std::thread::spawn(move || {
            let host = cpal::default_host();
            let (file, mic, call) = match open_all(&host, &path, chosen, silent) {
                Ok(opened) => opened,
                Err(e) => {
                    let _ = ready.send(Err(e));
                    return Ok(());
                }
            };
            let _ = ready.send(Ok(()));
            match record(&host, file, mic, call, &stopped, &written, &*levels) {
                Ended::Asked(result) => result,
                Ended::ByItself(e) => {
                    over.store(true, Ordering::Release);
                    ended(e);
                    Ok(())
                }
            }
        });
        let opened = opened.recv();
        let mut slot = self.slot();
        match opened {
            Ok(Ok(())) if matches!(*slot, Slot::Starting(true)) => {
                *slot = Slot::Free;
                drop(slot);
                let _ = stop.send(());
                let _ = thread.join();
                let _ = std::fs::remove_file(&path_kept);
                Err(MeetingError::Cancelled)
            }
            Ok(Ok(())) => {
                *slot = Slot::Recording(Session { id: id.to_string(), stop, thread, frames, muted, done });
                Ok(())
            }
            Ok(Err(e)) => {
                *slot = Slot::Free;
                Err(e)
            }
            Err(_) => {
                *slot = Slot::Free;
                Err(MeetingError::Device("the recording thread ended before it began".into()))
            }
        }
    }

    /// The recording ended and its file whole; its id.
    pub fn stop(&self) -> Result<String, MeetingError> {
        let session = {
            let mut slot = self.slot();
            match std::mem::take(&mut *slot) {
                Slot::Recording(session) => {
                    *slot = Slot::Stopping(session.id.clone());
                    session
                }
                Slot::Starting(_) => {
                    *slot = Slot::Starting(true);
                    return Err(MeetingError::NotRecording);
                }
                other => {
                    *slot = other;
                    return Err(MeetingError::NotRecording);
                }
            }
        };
        let _ = session.stop.send(());
        let result = session.thread.join().map_err(|_| MeetingError::Device("the recording thread failed".into())).and_then(|r| r);
        *self.slot() = Slot::Free;
        result.map(|()| session.id)
    }

    /// The slot, a recording that ended by itself let go — `ended` was told.
    fn slot(&self) -> MutexGuard<'_, Slot> {
        let mut slot = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(&*slot, Slot::Recording(s) if s.done.load(Ordering::Acquire)) {
            *slot = Slot::Free;
        }
        slot
    }

    /// A recording under way as `id`, with no devices behind it — for the
    /// tests of what must leave the one being recorded alone.
    #[cfg(test)]
    pub(crate) fn pretend(&self, id: &str) {
        self.pretend_with(id, |_| Ok(()));
    }

    /// The same, its thread `thread` given the stop's receiver.
    #[cfg(test)]
    fn pretend_with(&self, id: &str, thread: impl FnOnce(Receiver<()>) -> Result<(), MeetingError> + Send + 'static) -> Arc<AtomicBool> {
        let (stop, stopped) = mpsc::channel();
        let done = Arc::new(AtomicBool::new(false));
        let session = Session {
            id: id.to_string(),
            stop,
            thread: std::thread::spawn(move || thread(stopped)),
            frames: Arc::new(AtomicU64::new(RATE as u64 * 3)),
            muted: Arc::default(),
            done: done.clone(),
        };
        *self.0.lock().unwrap() = Slot::Recording(session);
        done
    }

    /// The recording under way, how long it is by now, and whether the microphone is muted.
    pub fn current(&self) -> Option<RecordingState> {
        match &*self.slot() {
            Slot::Recording(s) => Some(RecordingState {
                id: s.id.clone(),
                seconds: s.frames.load(Ordering::Relaxed) as f32 / RATE as f32,
                muted: s.muted.load(Ordering::Relaxed),
            }),
            _ => None,
        }
    }

    /// Whether `id`'s file is still being written — recorded now, or its
    /// stop finishing it — and so not to be read, listed or removed.
    pub fn holds(&self, id: &str) -> bool {
        match &*self.slot() {
            Slot::Recording(s) => s.id == id,
            Slot::Stopping(stopping) => stopping == id,
            Slot::Free | Slot::Starting(_) => false,
        }
    }

    /// The microphone written as silence from now on, or heard again. The
    /// stream stays open: the two sides stay in step, and unmuting is at once.
    pub fn mute(&self, on: bool) -> Result<(), MeetingError> {
        match &*self.slot() {
            Slot::Recording(s) => {
                s.muted.store(on, Ordering::Relaxed);
                Ok(())
            }
            _ => Err(MeetingError::NotRecording),
        }
    }
}

/// Both sides and the file, once at least one side is open: a call is worth
/// recording without a microphone, and the other way round. A side that did
/// not open is tried again each [`CHECK`].
fn open_all(host: &cpal::Host, path: &Path, chosen: Option<cpal::DeviceId>, muted: Arc<AtomicBool>) -> Result<(WavWriter, Input, Input), MeetingError> {
    let (mut mic, mut call) = (Input::new(Source::Microphone, muted)?, Input::new(Source::Call, Arc::default())?);
    mic.chosen = chosen;
    either(mic.connect(host), call.connect(host))?;
    Ok((WavWriter::create(path)?, mic, call))
}

/// Opening both sides as one: neither open is a failure, one is enough.
fn either(mic: Result<(), MeetingError>, call: Result<(), MeetingError>) -> Result<(), MeetingError> {
    match (mic, call) {
        (Err(e), Err(_)) => Err(e),
        (Err(e), Ok(())) | (Ok(()), Err(e)) => {
            eprintln!("meeting recorder: recording one side, the other tried again each second: {e}");
            Ok(())
        }
        (Ok(()), Ok(())) => Ok(()),
    }
}

/// How [`record`] came to an end.
enum Ended {
    /// Stopped: the file finished, or why it could not be.
    Asked(Result<(), MeetingError>),
    /// Without a Stop: why.
    ByItself(MeetingError),
}

/// The recording thread, its devices open: a write and its `levels` each
/// [`TICK`] until `stop`, then the rest and the header's sizes. Each [`CHECK`]
/// a side whose device went away, or whose system default moved (headphones
/// plugged in), is opened again on the default; its sound picks up where it is
/// heard, the time between silence.
fn record(
    host: &cpal::Host,
    mut file: WavWriter,
    mut mic: Input,
    mut call: Input,
    stop: &Receiver<()>,
    frames_written: &AtomicU64,
    levels: &dyn Fn(Levels),
) -> Ended {
    let mut origin = None;
    let mut ticks = 0u32;
    loop {
        let flush = !matches!(stop.recv_timeout(TICK), Err(RecvTimeoutError::Timeout));
        ticks += 1;
        if flush {
            // Closed first: nothing arrives after what is flushed.
            mic.stream = None;
            call.stream = None;
        } else if ticks % CHECK == 0 {
            mic.follow(host, &mut origin);
            call.follow(host, &mut origin);
        }
        // From the first sound of either side, whichever is pulled first.
        if origin.is_none() {
            origin = mic.track.first_at().into_iter().chain(call.track.first_at()).min();
        }
        let (mic_level, call_level) = (mic.track.pull(&mut origin, flush), call.track.pull(&mut origin, flush));
        let (m, c) = (&mut mic.track, &mut call.track);
        let frames = recording::frames_ready((m.pending.len(), m.end), (c.pending.len(), c.end), MAX_LAG, flush);
        let full = match file.append(&recording::interleave(&m.pending, &c.pending, frames)) {
            Ok(full) => full,
            Err(e) if flush => return Ended::Asked(Err(e)),
            Err(e) => return Ended::ByItself(e),
        };
        m.take(frames);
        c.take(frames);
        let written = file.frames();
        frames_written.store(written, Ordering::Relaxed);
        levels(Levels { seconds: written as f32 / RATE as f32, mic: mic_level, call: call_level });
        if flush {
            return Ended::Asked(file.finish());
        }
        if full {
            return Ended::ByItself(file.finish().err().unwrap_or(MeetingError::RecordingFull));
        }
    }
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

    fn frames(&self) -> u64 {
        self.written as u64 / (CHANNELS as u64 * 2)
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
    let channels = u16::from_le_bytes([header[22], header[23]]);
    if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" || &header[36..40] != b"data" || channels != CHANNELS {
        return Err(MeetingError::Unreadable(format!("{} is not a recording of this app's", path.display())));
    }
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
    use crate::infra::microphone::{give, microphones};
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
        // A header of ours in shape but with no channels: not ours, and no division by its frames.
        std::fs::write(&path, [recording::wav_header(0, 0).as_slice(), &[0; 8]].concat()).unwrap();
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
        let loudest = Arc::new(Mutex::new(Levels { seconds: 0.0, mic: 0.0, call: 0.0 }));
        let told = loudest.clone();
        // The first microphone listed, chosen by its id: the way the settings open one.
        let first = microphones().unwrap().into_iter().next().expect("a microphone");
        recorder
            .start(
                "live",
                &path,
                Some(&first.id),
                Box::new(move |l| {
                    let mut m = told.lock().unwrap();
                    *m = Levels { seconds: l.seconds, mic: m.mic.max(l.mic), call: m.call.max(l.call) };
                }),
                Box::new(|e| panic!("ended by itself: {e}")),
            )
            .unwrap();
        assert_eq!(recorder.start("again", &path, None, Box::new(|_| {}), Box::new(|_| {})), Err(MeetingError::AlreadyRecording));
        let _ = std::process::Command::new("say").args(["-v", "Milena", "Проверка записи созвона, раз, два, три"]).status();
        std::thread::sleep(Duration::from_millis(300));
        // Muted, the microphone's side is silence however loud it is around it.
        recorder.mute(true).unwrap();
        std::thread::sleep(Duration::from_millis(300)); // what arrived before the mute is written first
        let muted_from = recorder.current().unwrap().seconds;
        let _ = std::process::Command::new("say").args(["-v", "Milena", "Этого не слышно"]).status();
        let now = recorder.current().unwrap();
        assert!(now.muted);
        assert_eq!(now.id, "live");
        assert_eq!(recorder.stop().unwrap(), "live");
        assert_eq!(recorder.current(), None);
        assert!(!repair(&path).unwrap(), "stopped, its sizes are in");

        let bytes = std::fs::read(&path).unwrap();
        let samples: Vec<i16> = bytes[HEADER_BYTES as usize..].chunks(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
        let peak = |channel: usize| samples.iter().skip(channel).step_by(2).map(|s| s.unsigned_abs()).max().unwrap_or(0) as f32 / i16::MAX as f32;
        let seconds = samples.len() as f32 / 2.0 / RATE as f32;
        let told = *loudest.lock().unwrap();
        eprintln!("{seconds:.1} s, peak: microphone {:.3}, call {:.3}; told {told:?}; {:.1} s while running — {}", peak(0), peak(1), now.seconds, path.display());
        assert!(seconds > 1.0, "{seconds}");
        assert!(now.seconds > 1.0 && now.seconds <= seconds, "{} of {seconds}", now.seconds);
        assert!((told.seconds - seconds).abs() < 0.2, "the last told is the whole: {told:?}");
        assert!((told.mic - peak(0)).abs() < 0.05, "the meter saw what was written: {told:?}");
        let muted_tail = samples[(muted_from * RATE as f32) as usize * 2..].iter().step_by(2).map(|s| s.unsigned_abs()).max().unwrap_or(0);
        assert_eq!(muted_tail, 0, "from {muted_from:.1} s the microphone is silence");
        assert_eq!(crate::infra::speech::decode(&path).unwrap().1, RATE);
    }

    /// A side whose stream broke is opened again on the default, and its
    /// track goes on where the time is — run with `record_live`.
    #[test]
    #[ignore]
    fn record_live_reopened() {
        let host = cpal::default_host();
        let mut origin = None;
        let mut mic = Input::open(&host, Source::Microphone).unwrap();
        let begun = std::time::Instant::now();
        std::thread::sleep(Duration::from_millis(1500));
        mic.track.pull(&mut origin, false);
        let before = mic.track.pending.len();
        mic.broken.store(true, Ordering::Relaxed);
        mic.follow(&host, &mut origin);
        assert!(mic.stream.is_some() && !mic.broken.load(Ordering::Relaxed), "open again");
        mic.follow(&host, &mut origin);
        std::thread::sleep(Duration::from_millis(1500));
        mic.track.pull(&mut origin, true);
        let (frames, elapsed) = (mic.track.pending.len(), begun.elapsed().as_secs_f32());
        eprintln!("{before} frames before, {frames} after, {elapsed:.2} s");
        assert!(frames > before + RATE as usize, "it gave sound again");
        // Its first sound came some way into the wait: the track ends where the time is, not short of it.
        assert!((frames as f32 / RATE as f32 - elapsed).abs() < 0.5, "{frames} frames in {elapsed} s");
    }

    /// The recording thread over `mic` and `call` with no devices, into
    /// `file`; stopped first when `stopped`. How it ended.
    fn run(file: WavWriter, mic: Input, call: Input, stopped: bool) -> Ended {
        let (stop, asked) = mpsc::channel();
        if stopped {
            stop.send(()).unwrap();
        }
        record(&cpal::default_host(), file, mic, call, &asked, &AtomicU64::new(0), &|_| {})
    }

    fn sides() -> (Input, Input) {
        (Input::new(Source::Microphone, Arc::default()).unwrap(), Input::new(Source::Call, Arc::default()).unwrap())
    }

    #[test]
    fn each_side_starts_where_it_was_heard_from_the_first_sound_of_either() {
        let path = temp("recorder-origin");
        let (mic, call) = sides();
        // The call heard 60 ms before the microphone, though the microphone is pulled first.
        give(&mic.track, 60, std::iter::repeat_n(0.5, 160));
        give(&call.track, 0, std::iter::repeat_n(0.25, 160));
        assert!(matches!(run(WavWriter::create(&path).unwrap(), mic, call, true), Ended::Asked(Ok(()))));
        let bytes = std::fs::read(&path).unwrap();
        let samples: Vec<i16> = bytes[HEADER_BYTES as usize..].chunks(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
        let first = |channel: usize| samples.iter().skip(channel).step_by(2).position(|&s| s != 0);
        assert_eq!((first(0), first(1)), (Some(960), Some(0)), "the microphone 60 ms in, the call at the start");
    }

    #[test]
    fn a_recording_that_ends_without_a_stop_says_why() {
        // Full: the file finished, and that is the reason.
        let path = temp("recorder-self-full");
        let mut file = WavWriter::create(&path).unwrap();
        file.written = recording::max_data_bytes(CHANNELS) - 8;
        let (mic, call) = sides();
        give(&mic.track, 0, std::iter::repeat_n(0.5, 16));
        give(&call.track, 0, std::iter::repeat_n(0.5, 16));
        assert!(matches!(run(file, mic, call, false), Ended::ByItself(MeetingError::RecordingFull)));

        // A write that fails: by itself while it records, the Stop's own when stopping.
        let path = temp("recorder-self-failed");
        WavWriter::create(&path).unwrap();
        let unwritable = || WavWriter { file: File::open(&path).unwrap(), written: 0 };
        let with_sound = || {
            let (mic, call) = sides();
            give(&mic.track, 0, [0.5; 16]);
            give(&call.track, 0, [0.5; 16]);
            (mic, call)
        };
        let (mic, call) = with_sound();
        assert!(matches!(run(unwritable(), mic, call, false), Ended::ByItself(MeetingError::Store(_))));
        let (mic, call) = with_sound();
        assert!(matches!(run(unwritable(), mic, call, true), Ended::Asked(Err(MeetingError::Store(_)))));
    }

    #[test]
    fn one_side_open_is_a_recording_and_neither_is_not() {
        let no = |what| Err(MeetingError::NoDevice(what));
        assert_eq!(either(Ok(()), Ok(())), Ok(()));
        assert_eq!(either(no("microphone"), Ok(())), Ok(()), "a call with no microphone is still recorded");
        assert_eq!(either(Ok(()), no("sound output")), Ok(()));
        assert_eq!(either(no("microphone"), no("sound output")), Err(MeetingError::NoDevice("microphone")));
    }

    #[test]
    fn while_its_devices_open_the_recording_is_not_yet_anyones() {
        let recorder = Recorder::default();
        *recorder.0.lock().unwrap() = Slot::Starting(false);
        assert_eq!(recorder.current(), None);
        assert!(!recorder.holds("r"));
        assert_eq!(recorder.mute(true), Err(MeetingError::NotRecording));
        assert_eq!(recorder.start("again", &temp("recorder-starting"), None, Box::new(|_| {}), Box::new(|_| {})), Err(MeetingError::AlreadyRecording));
        // Turned off meanwhile: it is not to begin.
        assert_eq!(recorder.stop(), Err(MeetingError::NotRecording));
        assert!(matches!(*recorder.0.lock().unwrap(), Slot::Starting(true)));
    }

    #[test]
    fn a_stopping_recordings_file_is_still_its_own_until_finished() {
        let recorder = Arc::new(Recorder::default());
        let (finishing, finished) = mpsc::channel::<()>();
        recorder.pretend_with("r", move |stop| {
            stop.recv().unwrap();
            finished.recv().unwrap(); // the file being finished
            Ok(())
        });
        let stopping = {
            let recorder = recorder.clone();
            std::thread::spawn(move || recorder.stop())
        };
        let waited = std::time::Instant::now();
        while !matches!(*recorder.0.lock().unwrap(), Slot::Stopping(_)) {
            assert!(waited.elapsed() < Duration::from_secs(5), "the stop never said it was finishing");
            std::thread::yield_now();
        }
        assert!(recorder.holds("r"), "not listed, read or removed half-written");
        assert!(!recorder.holds("other"));
        assert_eq!(recorder.current(), None, "no longer recording");
        assert_eq!(recorder.stop(), Err(MeetingError::NotRecording), "one stop");
        finishing.send(()).unwrap();
        assert_eq!(stopping.join().unwrap(), Ok("r".to_string()));
        assert!(!recorder.holds("r"));
        assert!(matches!(*recorder.0.lock().unwrap(), Slot::Free));
    }

    #[test]
    fn a_recording_that_ended_by_itself_is_let_go() {
        let recorder = Recorder::default();
        let done = recorder.pretend_with("r", |_| Ok(()));
        assert!(recorder.holds("r"));
        done.store(true, Ordering::Release);
        assert_eq!(recorder.current(), None);
        assert!(!recorder.holds("r"), "its file is listed to transcribe");
        assert_eq!(recorder.mute(true), Err(MeetingError::NotRecording));
        assert_eq!(recorder.stop(), Err(MeetingError::NotRecording));
    }

    #[test]
    fn stopping_with_nothing_recorded_says_so() {
        assert_eq!(Recorder::default().stop(), Err(MeetingError::NotRecording));
        assert_eq!(Recorder::default().current(), None);
        assert_eq!(Recorder::default().mute(true), Err(MeetingError::NotRecording));
    }

    #[test]
    fn a_mute_is_the_recordings_until_undone() {
        let recorder = Recorder::default();
        recorder.pretend("r");
        assert_eq!(recorder.current().map(|r| r.muted), Some(false));
        recorder.mute(true).unwrap();
        assert_eq!(recorder.current().map(|r| r.muted), Some(true));
        recorder.mute(false).unwrap();
        assert_eq!(recorder.current().map(|r| r.muted), Some(false));
    }
}
