//! Dictation into the message box (`docs/52-voice-input.md`): the microphone
//! into the recognition process, and what it says out to the box. One at a
//! time. The process stays warm between dictations, and goes after [`IDLE`].
//!
//! A dictation is a thread of its own, for the microphone's stream lives where
//! it was opened. It wakes on each line of the process and each [`TICK`], so
//! whatever is asked of it from outside — [`Dictation::stop`], [`Dictation::cancel`]
//! — is only a note it reads on the tick.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::domain::dictation::{self, DictationError, DictationEvent, DictationEventSink, DictationLine, EndReason, Frame, Recognition, SoundIn, LONGEST_AUDIO};
use crate::domain::features::Feature;
use crate::infra::dictation_process::Process;
use crate::infra::microphone::Capture;
use crate::infra::settings_store;
use crate::services::features;
use crate::sync::lock;

/// How often the microphone is read and its level told: ten a second at most,
/// the throttle of a source that changes fast (AGENTS.md).
const TICK: Duration = Duration::from_millis(100);
/// How long the microphone goes on after Stop: the end of a word is said as
/// the key comes up.
const TAIL: Duration = Duration::from_millis(200);
const LONGEST: Duration = Duration::from_secs(10 * 60);
/// How long the process waits for the next dictation (decision 5).
const IDLE: Duration = Duration::from_secs(3 * 60);
/// How long a Stop waits for the process's `done`: a phrase takes it a second
/// or two, so past this it has hung.
const LONGEST_FINISH: Duration = Duration::from_secs(30);

#[derive(Clone, Copy)]
struct Limits {
    longest: Duration,
    idle: Duration,
    finish: Duration,
}

pub struct Dictation {
    state: Arc<Mutex<State>>,
    limits: Limits,
}

impl Default for Dictation {
    fn default() -> Self {
        Dictation { state: Arc::default(), limits: Limits { longest: LONGEST, idle: IDLE, finish: LONGEST_FINISH } }
    }
}

#[derive(Default)]
struct State {
    /// The process between dictations; the dictation has it meanwhile.
    warm: Option<Warm>,
    /// What the running dictation is asked, if one runs.
    running: Option<Arc<Mutex<Asked>>>,
    /// The running dictation's process goes with it, not back here.
    closing: bool,
    /// When the process last went idle.
    rested: Option<Instant>,
}

struct Warm {
    process: Box<dyn Recognition>,
    /// The `done`s still to come of dictations thrown away: all it says up to
    /// the last of them is theirs.
    owed: usize,
}

#[derive(Clone, Copy)]
enum Asked {
    Go,
    Stop { at: Instant, reason: EndReason },
    Cancel,
}

/// Stop, unless it was asked to end already.
fn ask_stop(asked: &Mutex<Asked>, reason: EndReason) {
    let mut asked = lock(asked);
    if matches!(*asked, Asked::Go) {
        *asked = Asked::Stop { at: Instant::now(), reason };
    }
}

impl Dictation {
    /// A dictation begun with the microphone chosen, told to `sink`. Refused
    /// here while the feature is off or one runs; a failure past that is
    /// `Failed` on `sink`, like any later one.
    pub fn start(&self, exe: PathBuf, models: PathBuf, sink: DictationEventSink) -> Result<(), DictationError> {
        if !features::enabled().is_on(Feature::VoiceInput) {
            return Err(DictationError::Off);
        }
        let microphone = settings_store::load().unwrap_or_default().meeting_microphone;
        self.begin(
            move || Ok(Box::new(Process::start(&exe, &models)?)),
            move || match Capture::open(microphone.as_deref()) {
                Ok(capture) => Ok(Box::new(capture)),
                Err(e) => Err(DictationError::Microphone(e.to_string())),
            },
            sink,
        )
    }

    fn begin<S, O>(&self, spawn: S, open: O, sink: DictationEventSink) -> Result<(), DictationError>
    where
        S: FnOnce() -> Result<Box<dyn Recognition>, DictationError> + Send + 'static,
        O: FnOnce() -> Result<Box<dyn SoundIn>, DictationError> + Send + 'static,
    {
        let asked = Arc::new(Mutex::new(Asked::Go));
        let warm = {
            let mut state = lock(&self.state);
            if state.running.is_some() {
                return Err(DictationError::Busy);
            }
            state.running = Some(asked.clone());
            state.warm.take()
        };
        sink(DictationEvent::Preparing);
        let (state, limits) = (self.state.clone(), self.limits);
        std::thread::spawn(move || {
            let end = |warm, event| {
                rest(&state, warm, limits.idle);
                sink(event);
            };
            let failed = |e: DictationError| DictationEvent::Failed { message: e.to_string() };
            // The process first: its models load while the microphone opens.
            let mut warm = warm;
            let alive = warm.as_mut().is_some_and(|w| w.process.alive());
            let warm = match warm.filter(|_| alive) {
                Some(warm) => warm,
                None => match spawn() {
                    Ok(process) => Warm { process, owed: 0 },
                    Err(e) => return end(None, failed(e)),
                },
            };
            let mic = match open() {
                Ok(mic) => mic,
                Err(e) => return end(Some(warm), failed(e)),
            };
            let mut session = Session { warm, mic: Some(mic), asked, sink: sink.clone(), limits, started: Instant::now(), heard: false, finishing: None };
            let ended = session.run();
            let Session { warm, mic, .. } = session;
            drop(mic);
            match ended {
                Ok(reason) => end(Some(warm), DictationEvent::Ended { reason }),
                Err(e) => {
                    drop(warm);
                    end(None, failed(e));
                }
            }
        });
        Ok(())
    }

    /// What was said stays; the end of it comes on the sink.
    pub fn stop(&self) {
        if let Some(asked) = &lock(&self.state).running {
            ask_stop(asked, EndReason::Stopped);
        }
    }

    /// What was said is thrown away.
    pub fn cancel(&self) {
        if let Some(asked) = &lock(&self.state).running {
            *lock(asked) = Asked::Cancel;
        }
    }

    /// The running dictation thrown away and the process gone — the app is
    /// quitting, or the feature went off.
    pub fn close(&self) {
        let warm = {
            let mut state = lock(&self.state);
            state.closing = state.running.is_some();
            state.warm.take()
        };
        drop(warm);
        self.cancel();
    }

    /// Feature `id` turned on or off. Voice input off goes with its button.
    pub fn feature_set(&self, id: &str, on: bool) {
        if !on && Feature::from_id(id) == Some(Feature::VoiceInput) {
            self.close();
        }
    }
}

/// The slot free, and the process, if it is still good, kept for `idle`.
fn rest(shared: &Arc<Mutex<State>>, warm: Option<Warm>, idle: Duration) {
    let mut state = lock(shared);
    state.running = None;
    // The lock goes first, then `warm`: a kill and a wait are not for under
    // it, the window's Stop takes it on the main thread.
    if std::mem::take(&mut state.closing) {
        return;
    }
    let Some(warm) = warm else { return };
    let at = Instant::now();
    (state.warm, state.rested) = (Some(warm), Some(at));
    drop(state);
    let shared = shared.clone();
    // ponytail: a sleeping thread per dictation, each gone in `idle`; only the last one's
    // `at` still stands. A timer thread of its own if dictations ever come by the hundred.
    std::thread::spawn(move || {
        std::thread::sleep(idle);
        let mut state = lock(&shared);
        if state.rested == Some(at) {
            state.rested = None;
            let warm = state.warm.take();
            drop(state);
            drop(warm);
        }
    });
}

struct Session {
    warm: Warm,
    /// Gone once Stop's tail is over: the system's indicator goes out at once.
    mic: Option<Box<dyn SoundIn>>,
    asked: Arc<Mutex<Asked>>,
    sink: DictationEventSink,
    limits: Limits,
    started: Instant,
    /// Whether the microphone gave anything yet.
    heard: bool,
    /// `finish` sent: when, and how this dictation ends once `done` comes.
    finishing: Option<(Instant, EndReason)>,
}

impl Session {
    /// Until it ends; an error is the process lost.
    fn run(&mut self) -> Result<EndReason, DictationError> {
        let mut next = Instant::now();
        loop {
            match self.warm.process.line(next.saturating_duration_since(Instant::now()))? {
                Some(line) => {
                    if let Some(reason) = self.hear(line)? {
                        return Ok(reason);
                    }
                }
                None => {
                    next = Instant::now() + TICK;
                    if let Some(reason) = self.tick()? {
                        return Ok(reason);
                    }
                }
            }
        }
    }

    /// A line of the process. A reason when it ended the dictation.
    fn hear(&mut self, line: DictationLine) -> Result<Option<EndReason>, DictationError> {
        // An error is the process's, whichever dictation it came in.
        if self.warm.owed > 0 && !matches!(line, DictationLine::Error { .. }) {
            self.warm.owed -= (line == DictationLine::Done) as usize;
            return Ok(None);
        }
        match line {
            // Sound goes to it from the first buffer, whether it is ready or not.
            DictationLine::Ready => {}
            DictationLine::Partial { text } => (self.sink)(DictationEvent::Partial { text }),
            DictationLine::Phrase { text } => (self.sink)(DictationEvent::Phrase { text }),
            // A cancel may be waiting for the tick: it wins, what was said goes.
            DictationLine::Done => {
                let cancelled = matches!(*lock(&self.asked), Asked::Cancel);
                return Ok(self.finishing.map(|(_, reason)| if cancelled { EndReason::Cancelled } else { reason }));
            }
            DictationLine::Error { message } => return Err(DictationError::ProcessEnded(message)),
        }
        Ok(None)
    }

    fn tick(&mut self) -> Result<Option<EndReason>, DictationError> {
        let asked = *lock(&self.asked);
        if let Asked::Cancel = asked {
            // After `finish` the `done` is on its way already. A process that
            // cannot be told is found dead by the next dictation.
            if self.finishing.is_some() || self.warm.process.send(&Frame::Cancel).is_ok() {
                self.warm.owed += 1;
            }
            return Ok(Some(EndReason::Cancelled));
        }
        if let Some((since, _)) = self.finishing {
            return match since.elapsed() >= self.limits.finish {
                true => Err(DictationError::ProcessEnded("it did not finish".into())),
                false => Ok(None),
            };
        }
        // Measured before the pull, so that the last one has the whole tail in it.
        let tail_over = matches!(asked, Asked::Stop { at, .. } if at.elapsed() >= TAIL);
        self.listen()?;
        match asked {
            Asked::Stop { reason, .. } if tail_over => {
                self.mic = None;
                self.warm.process.send(&Frame::Finish)?;
                self.finishing = Some((Instant::now(), reason));
            }
            Asked::Go if self.started.elapsed() >= self.limits.longest => ask_stop(&self.asked, EndReason::Limit),
            _ => {}
        }
        Ok(None)
    }

    /// What the microphone gave this tick, to the box and the process.
    fn listen(&mut self) -> Result<(), DictationError> {
        let sound = self.mic.as_mut().map(|mic| mic.pull()).unwrap_or_default();
        if sound.is_empty() {
            return Ok(());
        }
        if !std::mem::replace(&mut self.heard, true) {
            (self.sink)(DictationEvent::Listening);
        }
        (self.sink)(DictationEvent::Levels { peaks: dictation::quarters(&sound) });
        // More than a frame takes only after a long hole in the device's sound.
        for piece in sound.chunks(LONGEST_AUDIO) {
            self.warm.process.send(&Frame::Audio(piece.to_vec()))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};

    const WAIT: Duration = Duration::from_secs(5);

    fn until(what: &str, mut done: impl FnMut() -> bool) {
        let started = Instant::now();
        while !done() {
            assert!(started.elapsed() < WAIT, "never: {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// A recognition process in memory: what it was sent, and lines to say.
    #[derive(Clone)]
    struct Speech {
        sent: Arc<Mutex<Vec<Frame>>>,
        lines: Sender<DictationLine>,
        dead: Arc<AtomicBool>,
        dropped: Arc<AtomicBool>,
    }

    struct Fake {
        speech: Speech,
        lines: Receiver<DictationLine>,
    }

    impl Recognition for Fake {
        fn send(&mut self, frame: &Frame) -> Result<(), DictationError> {
            if self.speech.dead.load(Ordering::SeqCst) {
                return Err(DictationError::ProcessEnded("dead".into()));
            }
            self.speech.sent.lock().unwrap().push(frame.clone());
            Ok(())
        }

        fn line(&mut self, wait: Duration) -> Result<Option<DictationLine>, DictationError> {
            if self.speech.dead.load(Ordering::SeqCst) {
                return Err(DictationError::ProcessEnded("dead".into()));
            }
            match self.lines.recv_timeout(wait) {
                Ok(line) => Ok(Some(line)),
                Err(RecvTimeoutError::Timeout) => Ok(None),
                Err(RecvTimeoutError::Disconnected) => panic!("the test holds the other end"),
            }
        }

        fn alive(&mut self) -> bool {
            !self.speech.dead.load(Ordering::SeqCst)
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            self.speech.dropped.store(true, Ordering::SeqCst);
        }
    }

    fn speech() -> (Speech, Fake) {
        let (sender, lines) = mpsc::channel();
        let speech = Speech { sent: Arc::default(), lines: sender, dead: Arc::default(), dropped: Arc::default() };
        (speech.clone(), Fake { speech, lines })
    }

    impl Speech {
        fn say(&self, line: DictationLine) {
            self.lines.send(line).unwrap();
        }

        fn sent(&self) -> Vec<Frame> {
            self.sent.lock().unwrap().clone()
        }

        fn sound_sent(&self) -> usize {
            self.sent().iter().filter(|f| matches!(f, Frame::Audio(_))).count()
        }

        fn got(&self, frame: Frame) -> bool {
            self.sent().contains(&frame)
        }
    }

    /// A microphone in memory: what is queued, then `flowing` a sample a tick or nothing.
    #[derive(Clone, Default)]
    struct Mic {
        queued: Arc<Mutex<VecDeque<Vec<f32>>>>,
        flowing: Arc<AtomicBool>,
        pulls: Arc<AtomicUsize>,
        last_pull: Arc<Mutex<Option<Instant>>>,
        closed: Arc<AtomicBool>,
    }

    struct OpenMic(Mic);

    impl SoundIn for OpenMic {
        fn pull(&mut self) -> Vec<f32> {
            self.0.pulls.fetch_add(1, Ordering::SeqCst);
            *self.0.last_pull.lock().unwrap() = Some(Instant::now());
            match self.0.queued.lock().unwrap().pop_front() {
                Some(sound) => sound,
                None if self.0.flowing.load(Ordering::SeqCst) => vec![0.5; 4],
                None => Vec::new(),
            }
        }
    }

    impl Drop for OpenMic {
        fn drop(&mut self) {
            self.0.closed.store(true, Ordering::SeqCst);
        }
    }

    fn flowing() -> Mic {
        let mic = Mic::default();
        mic.flowing.store(true, Ordering::SeqCst);
        mic
    }

    #[derive(Clone, Default)]
    struct Told(Arc<Mutex<Vec<DictationEvent>>>);

    impl Told {
        fn sink(&self) -> DictationEventSink {
            let told = self.0.clone();
            Arc::new(move |event| told.lock().unwrap().push(event))
        }

        fn all(&self) -> Vec<DictationEvent> {
            self.0.lock().unwrap().clone()
        }

        /// What it heard, levels aside: they come as they come.
        fn words(&self) -> Vec<DictationEvent> {
            self.all().into_iter().filter(|e| !matches!(e, DictationEvent::Levels { .. })).collect()
        }

        fn has(&self, event: &DictationEvent) -> bool {
            self.all().contains(event)
        }

        fn wait(&self, event: DictationEvent) {
            until(&format!("{event:?}"), || self.has(&event));
        }

        fn wait_end(&self) {
            until("the end", || self.all().iter().any(|e| matches!(e, DictationEvent::Ended { .. } | DictationEvent::Failed { .. })));
        }
    }

    fn dictation() -> Dictation {
        dictation_with(LONGEST, IDLE, LONGEST_FINISH)
    }

    fn dictation_with(longest: Duration, idle: Duration, finish: Duration) -> Dictation {
        Dictation { state: Arc::default(), limits: Limits { longest, idle, finish } }
    }

    /// A dictation on `process`, which must not be asked for if one is warm.
    fn begin(dictation: &Dictation, process: Option<Fake>, mic: &Mic, told: &Told) -> Result<(), DictationError> {
        let mic = mic.clone();
        dictation.begin(
            move || Ok(Box::new(process.expect("the warm process is used"))),
            move || Ok(Box::new(OpenMic(mic))),
            told.sink(),
        )
    }

    /// Through to its end by Stop: a `done` for the `finish`.
    fn stopped(dictation: &Dictation, speech: &Speech, told: &Told) {
        dictation.stop();
        until("finish", || speech.got(Frame::Finish));
        speech.say(DictationLine::Done);
        told.wait(DictationEvent::Ended { reason: EndReason::Stopped });
    }

    fn free(dictation: &Dictation) {
        until("a free slot", || lock(&dictation.state).running.is_none());
    }

    #[test]
    fn it_listens_on_the_first_buffer_without_waiting_for_the_model() {
        let (speech, process) = speech();
        let (mic, told) = (Mic::default(), Told::default());
        let d = dictation();
        begin(&d, Some(process), &mic, &told).unwrap();
        until("the microphone read", || mic.pulls.load(Ordering::SeqCst) >= 2);
        assert_eq!(told.all(), [DictationEvent::Preparing], "nothing heard yet is nothing to say");
        mic.queued.lock().unwrap().extend([vec![0.5; 8], vec![0.25; 8]]);
        told.wait(DictationEvent::Listening);
        until("both buffers sent", || speech.sound_sent() == 2);
        assert_eq!(told.words(), [DictationEvent::Preparing, DictationEvent::Listening], "said once, no `ready` needed");
        assert!(told.has(&DictationEvent::Levels { peaks: [0.5; 4] }));
        d.cancel();
    }

    #[test]
    fn what_the_process_says_goes_to_the_box() {
        let (speech, process) = speech();
        let told = Told::default();
        let d = dictation();
        begin(&d, Some(process), &Mic::default(), &told).unwrap();
        speech.say(DictationLine::Ready);
        speech.say(DictationLine::Partial { text: "при".into() });
        speech.say(DictationLine::Phrase { text: "Привет.".into() });
        told.wait(DictationEvent::Phrase { text: "Привет.".into() });
        assert_eq!(told.words(), [DictationEvent::Preparing, DictationEvent::Partial { text: "при".into() }, DictationEvent::Phrase { text: "Привет.".into() }]);
        d.cancel();
    }

    #[test]
    fn levels_are_told_a_tick_at_a_time() {
        let (speech, process) = speech();
        let (mic, told) = (flowing(), Told::default());
        let d = dictation();
        begin(&d, Some(process), &mic, &told).unwrap();
        let started = Instant::now();
        (0..50).for_each(|_| speech.say(DictationLine::Partial { text: "да".into() }));
        std::thread::sleep(TICK * 4);
        let levels = told.all().iter().filter(|e| matches!(e, DictationEvent::Levels { .. })).count();
        let ticks = started.elapsed().as_millis() / TICK.as_millis() + 1;
        assert!(levels >= 1 && levels as u128 <= ticks, "{levels} in {ticks} ticks");
        d.cancel();
    }

    #[test]
    fn a_stop_keeps_the_microphone_for_the_tail_and_gives_the_last_phrase() {
        let (speech, process) = speech();
        let (mic, told) = (flowing(), Told::default());
        let d = dictation();
        begin(&d, Some(process), &mic, &told).unwrap();
        told.wait(DictationEvent::Listening);
        // Between two ticks: right after one, half a tail would look like a whole one.
        std::thread::sleep(TICK / 2);
        let (before, asked) = (speech.sound_sent(), Instant::now());
        d.stop();
        until("finish", || speech.got(Frame::Finish));
        assert!(asked.elapsed() >= TAIL, "the end of the word is in");
        // By the clock, not by counting ticks: a late tick pulls what a real
        // microphone gathered meanwhile, in fewer frames.
        assert!(speech.sound_sent() > before, "it went on");
        assert!(mic.last_pull.lock().unwrap().is_some_and(|at| at >= asked + TAIL), "to the end of the tail");
        assert_eq!(speech.sent().last(), Some(&Frame::Finish), "and all of it before the finish");
        until("the microphone let go", || mic.closed.load(Ordering::SeqCst));

        speech.say(DictationLine::Phrase { text: "Конец.".into() });
        told.wait(DictationEvent::Phrase { text: "Конец.".into() });
        assert!(!told.has(&DictationEvent::Ended { reason: EndReason::Stopped }), "not over until `done`");
        assert!(matches!(begin(&d, None, &Mic::default(), &told), Err(DictationError::Busy)), "still this one's");
        speech.say(DictationLine::Done);
        told.wait(DictationEvent::Ended { reason: EndReason::Stopped });
        free(&d);
    }

    #[test]
    fn stopping_twice_or_with_nothing_running_does_nothing() {
        let d = dictation();
        d.stop();
        d.cancel();
        let (speech, process) = speech();
        let (mic, told) = (Mic::default(), Told::default());
        begin(&d, Some(process), &mic, &told).unwrap();
        d.stop();
        d.stop();
        stopped(&d, &speech, &told);
        assert_eq!(speech.sent(), [Frame::Finish], "one finish, nothing heard before it");
    }

    #[test]
    fn a_cancel_ends_at_once_and_its_lines_stay_out_of_the_next_dictation() {
        let (speech, process) = speech();
        let (mic, told) = (flowing(), Told::default());
        let d = dictation();
        begin(&d, Some(process), &mic, &told).unwrap();
        told.wait(DictationEvent::Listening);
        d.cancel();
        told.wait(DictationEvent::Ended { reason: EndReason::Cancelled });
        assert!(speech.got(Frame::Cancel));
        assert!(!speech.got(Frame::Finish), "no tail, nothing to finish");
        assert!(mic.closed.load(Ordering::SeqCst), "the microphone goes before the box hears it ended");

        let (next_mic, next) = (flowing(), Told::default());
        begin(&d, None, &next_mic, &next).unwrap();
        next.wait(DictationEvent::Listening);
        speech.say(DictationLine::Phrase { text: "брошенная".into() });
        speech.say(DictationLine::Partial { text: "брошенная".into() });
        speech.say(DictationLine::Done);
        speech.say(DictationLine::Phrase { text: "новая".into() });
        next.wait(DictationEvent::Phrase { text: "новая".into() });
        assert_eq!(next.words(), [DictationEvent::Preparing, DictationEvent::Listening, DictationEvent::Phrase { text: "новая".into() }]);
        assert!(!told.has(&DictationEvent::Phrase { text: "брошенная".into() }));
        d.cancel();
    }

    #[test]
    fn a_cancel_in_the_finish_waits_for_the_done_already_coming() {
        let (speech, process) = speech();
        let told = Told::default();
        let d = dictation();
        begin(&d, Some(process), &Mic::default(), &told).unwrap();
        d.stop();
        until("finish", || speech.got(Frame::Finish));
        d.cancel();
        told.wait(DictationEvent::Ended { reason: EndReason::Cancelled });
        assert!(!speech.got(Frame::Cancel), "that would be a second `done`");

        let next = Told::default();
        begin(&d, None, &Mic::default(), &next).unwrap();
        speech.say(DictationLine::Phrase { text: "хвост".into() });
        speech.say(DictationLine::Done);
        speech.say(DictationLine::Phrase { text: "новая".into() });
        next.wait(DictationEvent::Phrase { text: "новая".into() });
        assert!(!next.has(&DictationEvent::Phrase { text: "хвост".into() }));
        d.cancel();
    }

    #[test]
    fn a_cancel_the_done_got_ahead_of_is_still_a_cancel() {
        let (speech, process) = speech();
        let told = Told::default();
        let d = dictation();
        begin(&d, Some(process), &Mic::default(), &told).unwrap();
        d.stop();
        until("finish", || speech.got(Frame::Finish));
        d.cancel();
        speech.say(DictationLine::Done);
        told.wait(DictationEvent::Ended { reason: EndReason::Cancelled });
    }

    #[test]
    fn an_error_is_told_even_while_a_cancelled_dictation_is_owed() {
        let (speech, process) = speech();
        let told = Told::default();
        let d = dictation();
        begin(&d, Some(process), &Mic::default(), &told).unwrap();
        d.cancel();
        told.wait_end();
        free(&d);

        let next = Told::default();
        begin(&d, None, &Mic::default(), &next).unwrap();
        speech.say(DictationLine::Error { message: "no models".into() });
        next.wait_end();
        assert!(matches!(next.words().last(), Some(DictationEvent::Failed { message }) if message.contains("no models")));
    }

    #[test]
    fn two_cancelled_in_a_row_are_both_waited_for() {
        let (speech, process) = speech();
        let told = Told::default();
        let d = dictation();
        begin(&d, Some(process), &Mic::default(), &told).unwrap();
        d.cancel();
        told.wait(DictationEvent::Ended { reason: EndReason::Cancelled });
        free(&d);
        let second = Told::default();
        begin(&d, None, &Mic::default(), &second).unwrap();
        d.cancel();
        second.wait(DictationEvent::Ended { reason: EndReason::Cancelled });
        free(&d);

        let third = Told::default();
        begin(&d, None, &Mic::default(), &third).unwrap();
        speech.say(DictationLine::Done);
        speech.say(DictationLine::Phrase { text: "вторая".into() });
        speech.say(DictationLine::Done);
        speech.say(DictationLine::Phrase { text: "третья".into() });
        third.wait(DictationEvent::Phrase { text: "третья".into() });
        assert!(!third.has(&DictationEvent::Phrase { text: "вторая".into() }));
        d.cancel();
    }

    #[test]
    fn one_at_a_time_and_the_slot_is_free_after() {
        let (speech, process) = speech();
        let told = Told::default();
        let d = dictation();
        begin(&d, Some(process), &Mic::default(), &told).unwrap();
        let second = Told::default();
        assert_eq!(begin(&d, None, &Mic::default(), &second), Err(DictationError::Busy));
        assert_eq!(second.all(), [], "refused says nothing on the channel");
        stopped(&d, &speech, &told);
        free(&d);
        let next = Told::default();
        begin(&d, None, &Mic::default(), &next).unwrap();
        d.cancel();
        next.wait_end();
        assert!(!speech.dropped.load(Ordering::SeqCst), "the same process all along");
    }

    #[test]
    fn the_slot_and_the_microphone_are_free_by_the_time_the_box_hears_the_end() {
        let (_speech, process) = speech();
        let mic = Mic::default();
        let d = Arc::new(dictation());
        let (was_free, seen, heard) = (Arc::new(AtomicBool::new(false)), d.clone(), mic.clone());
        let flag = was_free.clone();
        let sink: DictationEventSink = Arc::new(move |event| {
            if matches!(event, DictationEvent::Ended { .. }) {
                flag.store(lock(&seen.state).running.is_none() && heard.closed.load(Ordering::SeqCst), Ordering::SeqCst);
            }
        });
        let opened = mic.clone();
        d.begin(move || Ok(Box::new(process)), move || Ok(Box::new(OpenMic(opened))), sink).unwrap();
        d.cancel();
        until("the end", || was_free.load(Ordering::SeqCst));
    }

    #[test]
    fn with_the_feature_off_it_is_refused_and_on_it_goes_ahead() {
        crate::testing::with_app_dir("dictation-feature", || {
            let told = Told::default();
            let d = dictation();
            let start = || d.start("/no/such/exe".into(), "models".into(), told.sink());
            assert_eq!(start(), Err(DictationError::Off));
            assert_eq!(told.all(), []);
            features::set("voice-input", true).unwrap();
            assert_eq!(start(), Ok(()));
            told.wait_end();
            let [DictationEvent::Preparing, DictationEvent::Failed { message }] = &told.all()[..] else { panic!("{:?}", told.all()) };
            assert!(message.contains("/no/such/exe"), "{message}");
            free(&d);
        });
    }

    #[test]
    fn turning_it_off_throws_the_dictation_away_and_the_process_with_it() {
        let (speech, process) = speech();
        let told = Told::default();
        let d = dictation();
        begin(&d, Some(process), &flowing(), &told).unwrap();
        told.wait(DictationEvent::Listening);
        for (id, on) in [("voice-input", true), ("meeting-notes", false)] {
            d.feature_set(id, on);
        }
        std::thread::sleep(TICK * 2);
        assert!(!told.all().iter().any(|e| matches!(e, DictationEvent::Ended { .. })), "those are not its");
        d.feature_set("voice-input", false);
        told.wait(DictationEvent::Ended { reason: EndReason::Cancelled });
        until("the process gone", || speech.dropped.load(Ordering::SeqCst));
        free(&d);
        assert!(lock(&d.state).warm.is_none());

        let (again, process) = self::speech();
        let next = Told::default();
        begin(&d, Some(process), &Mic::default(), &next).unwrap();
        stopped(&d, &again, &next);
        free(&d);
        assert!(lock(&d.state).warm.is_some(), "that close was that dictation's");
    }

    #[test]
    fn closing_with_nothing_running_kills_the_warm_process_and_the_next_starts_a_new_one() {
        let (speech, process) = speech();
        let told = Told::default();
        let d = dictation();
        begin(&d, Some(process), &Mic::default(), &told).unwrap();
        stopped(&d, &speech, &told);
        free(&d);
        d.close();
        assert!(speech.dropped.load(Ordering::SeqCst));

        let (again, process) = self::speech();
        let next = Told::default();
        begin(&d, Some(process), &Mic::default(), &next).unwrap();
        stopped(&d, &again, &next);
        free(&d);
        assert!(lock(&d.state).warm.is_some(), "closing is not for ever");
    }

    #[test]
    fn the_process_goes_when_idle_counted_from_the_last_dictation() {
        let idle = Duration::from_millis(600);
        let (speech, process) = speech();
        let told = Told::default();
        let d = dictation_with(LONGEST, idle, LONGEST_FINISH);
        begin(&d, Some(process), &Mic::default(), &told).unwrap();
        stopped(&d, &speech, &told);
        free(&d);

        // Over before the first timer is up, which then finds the process idle — but not for long enough.
        let next = Told::default();
        begin(&d, None, &Mic::default(), &next).unwrap();
        std::thread::sleep(idle / 2);
        d.cancel();
        next.wait_end();
        free(&d);
        std::thread::sleep(idle * 2 / 3);
        assert!(!speech.dropped.load(Ordering::SeqCst), "idle only from the last one");
        until("idle for long enough", || speech.dropped.load(Ordering::SeqCst));
        assert!(lock(&d.state).warm.is_none());
    }

    #[test]
    fn a_process_dying_is_the_failure_and_it_is_not_kept() {
        let (speech, process) = speech();
        let told = Told::default();
        let d = dictation();
        begin(&d, Some(process), &flowing(), &told).unwrap();
        told.wait(DictationEvent::Listening);
        speech.dead.store(true, Ordering::SeqCst);
        told.wait_end();
        assert!(matches!(told.words().last(), Some(DictationEvent::Failed { .. })));
        free(&d);
        assert!(lock(&d.state).warm.is_none(), "not kept");
    }

    #[test]
    fn its_error_line_is_the_failure_and_says_why() {
        let (speech, process) = speech();
        let told = Told::default();
        let d = dictation();
        begin(&d, Some(process), &Mic::default(), &told).unwrap();
        speech.say(DictationLine::Error { message: "no models".into() });
        told.wait_end();
        let Some(DictationEvent::Failed { message }) = told.words().pop() else { panic!() };
        assert!(message.contains("no models"), "{message}");
        free(&d);
        assert!(speech.dropped.load(Ordering::SeqCst), "it is on its way out");
    }

    #[test]
    fn a_warm_process_that_died_is_replaced() {
        let (speech, process) = speech();
        let told = Told::default();
        let d = dictation();
        begin(&d, Some(process), &Mic::default(), &told).unwrap();
        stopped(&d, &speech, &told);
        free(&d);
        speech.dead.store(true, Ordering::SeqCst);

        let (again, process) = self::speech();
        let next = Told::default();
        begin(&d, Some(process), &flowing(), &next).unwrap();
        next.wait(DictationEvent::Listening);
        until("sound to the new one", || again.sound_sent() > 0);
        d.cancel();
    }

    #[test]
    fn a_microphone_that_will_not_open_fails_it_and_the_process_stays_warm() {
        let (speech, process) = speech();
        let told = Told::default();
        let d = dictation();
        d.begin(move || Ok(Box::new(process)), || Err(DictationError::Microphone("gone".into())), told.sink()).unwrap();
        told.wait_end();
        assert!(matches!(&told.all()[..], [DictationEvent::Preparing, DictationEvent::Failed { message }] if message.contains("gone")));
        free(&d);
        assert!(!speech.dropped.load(Ordering::SeqCst));
        assert!(lock(&d.state).warm.is_some());
    }

    #[test]
    fn at_the_limit_it_ends_as_a_stop_would() {
        let (speech, process) = speech();
        let (mic, told) = (flowing(), Told::default());
        let d = dictation_with(Duration::from_millis(150), IDLE, LONGEST_FINISH);
        begin(&d, Some(process), &mic, &told).unwrap();
        until("finish", || speech.got(Frame::Finish));
        speech.say(DictationLine::Phrase { text: "Успело.".into() });
        speech.say(DictationLine::Done);
        told.wait(DictationEvent::Ended { reason: EndReason::Limit });
        assert!(told.has(&DictationEvent::Phrase { text: "Успело.".into() }));
    }

    #[test]
    fn a_stop_after_the_limit_does_not_change_why_it_ended() {
        let (speech, process) = speech();
        let told = Told::default();
        let d = dictation_with(Duration::ZERO, IDLE, LONGEST_FINISH);
        begin(&d, Some(process), &Mic::default(), &told).unwrap();
        until("the limit asked", || lock(&d.state).running.as_ref().is_some_and(|a| matches!(*lock(a), Asked::Stop { .. })));
        d.stop();
        until("finish", || speech.got(Frame::Finish));
        speech.say(DictationLine::Done);
        told.wait(DictationEvent::Ended { reason: EndReason::Limit });
    }

    #[test]
    fn a_process_that_never_says_done_is_given_up_on() {
        let (speech, process) = speech();
        let told = Told::default();
        let d = dictation_with(LONGEST, IDLE, Duration::from_millis(200));
        begin(&d, Some(process), &Mic::default(), &told).unwrap();
        d.stop();
        told.wait_end();
        assert!(speech.got(Frame::Finish));
        assert!(matches!(told.words().last(), Some(DictationEvent::Failed { .. })));
        free(&d);
        assert!(speech.dropped.load(Ordering::SeqCst), "a hung process is not kept");
    }

    #[test]
    fn a_long_hole_goes_as_frames_the_process_will_read() {
        let (speech, process) = speech();
        let (mic, told) = (Mic::default(), Told::default());
        mic.queued.lock().unwrap().push_back(vec![0.0; LONGEST_AUDIO + 1]);
        let d = dictation();
        begin(&d, Some(process), &mic, &told).unwrap();
        until("both frames", || speech.sound_sent() == 2);
        let frames = speech.sent();
        let [Frame::Audio(first), Frame::Audio(second)] = &frames[..] else { panic!() };
        assert_eq!((first.len(), second.len()), (LONGEST_AUDIO, 1));
        d.cancel();
    }

    /// The real app's process, a voice from a file a tick at a time, twice: the
    /// second on the warm process. `cargo build` first, for the exe.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn dictation_live_app() {
        let exe = std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().join("kibo-agent");
        assert!(exe.exists(), "`cargo build` first: {}", exe.display());
        let models = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("models/meeting");
        let voice: Vec<f32> = crate::dictation_sidecar::tests::voice("app");
        let d = dictation();
        for round in 0..2 {
            let (told, mic) = (Told::default(), Mic::default());
            mic.queued.lock().unwrap().extend(voice.chunks(1600).map(<[f32]>::to_vec));
            let (exe, models, opened) = (exe.clone(), models.clone(), mic.clone());
            let started = Instant::now();
            d.begin(move || Ok(Box::new(Process::start(&exe, &models)?)), move || Ok(Box::new(OpenMic(opened))), told.sink()).unwrap();
            while !mic.queued.lock().unwrap().is_empty() {
                std::thread::sleep(TICK);
            }
            d.stop();
            until("the end", || told.all().iter().any(|e| matches!(e, DictationEvent::Ended { .. } | DictationEvent::Failed { .. })));
            let words = told.words();
            eprintln!("round {round}, {:?}: {words:?}", started.elapsed());
            assert!(words.iter().any(|e| matches!(e, DictationEvent::Partial { .. })), "text while it plays");
            let phrases: String = words.iter().filter_map(|e| if let DictationEvent::Phrase { text } = e { Some(text.as_str()) } else { None }).collect();
            assert!(phrases.to_lowercase().contains("провер") && phrases.to_lowercase().contains("погод"), "{words:?}");
            assert_eq!(words.last(), Some(&DictationEvent::Ended { reason: EndReason::Stopped }));
            free(&d);
        }
        d.close();
    }

    /// The real microphone at 16 kHz, whatever the room says: the sound of a
    /// second, near enough, and not silence. From a terminal macOS asks for it.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn dictation_live_microphone() {
        let mut mic = Capture::open(None).unwrap();
        std::thread::sleep(Duration::from_millis(500));
        mic.pull();
        std::thread::sleep(Duration::from_secs(1));
        let sound = mic.pull();
        assert!((14_000..18_000).contains(&sound.len()), "{}", sound.len());
        assert!(crate::domain::recording::peak(&sound) > 0.0);
    }
}
