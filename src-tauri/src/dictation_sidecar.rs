//! The dictation process: this same exe, started as `<exe> --dictation-sidecar
//! <models>`. `main.rs` comes here before any window or Tauri is up.
//!
//! A process for what `meeting_sidecar` is one for: a crash in the native
//! speech library costs a dictation, and the memory goes back when it exits.
//! This one stays between dictations, which is why `finish` and `cancel` both
//! end in `done`. Sound comes in as [`Frame`]s, read on a thread of their own
//! from the start, so what is said while the models load waits in the queue
//! and a long pass holds the pipe up for no one.
//!
//! GigaAM has no streaming form: VAD cuts phrases, a phrase ended is recognized
//! once, and the open one again from its start every [`PARTIAL_STEP`].
//!
//! [`PARTIAL_STEP`]: crate::domain::dictation::PARTIAL_STEP

use std::io::{ErrorKind, Read, Write};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

pub use crate::domain::dictation::SIDECAR_FLAG as FLAG;
use crate::domain::dictation::{spoken, DictationError, DictationLine, Frame, PartialPasses, PhraseDetector, PhraseRecognizer};
use crate::domain::recording::RATE;
use crate::infra::speech;

/// VAD says a phrase has begun a little after it has: this much sound before.
const LOOKBACK: usize = RATE as usize / 2;
/// An open phrase is ended here whatever VAD says: GigaAM was never fed longer
/// (docs/36), a pass over more takes too long, and past minutes it aborts.
const LONGEST_PHRASE: usize = 24 * RATE as usize;
/// The app closed stdin: quit, crashed or killed.
const APP_GONE: i32 = 3;

type Line<'a> = &'a mut dyn FnMut(DictationLine);

/// The process's exit code.
pub fn main(args: &[String]) -> i32 {
    let [models] = args else {
        emit(DictationLine::Error { message: format!("usage: {FLAG} <models>") });
        return 2;
    };
    let (sender, frames) = mpsc::channel();
    std::thread::spawn(move || read_frames(std::io::stdin().lock(), &sender));
    let (recognizer, detector) = match speech::dictation_models(Path::new(models)) {
        Ok(found) => found,
        Err(e) => {
            emit(DictationLine::Error { message: e.to_string() });
            return 1;
        }
    };
    emit(DictationLine::Ready);
    let started = Instant::now();
    run(&mut Listener::new(recognizer, detector, Box::new(move || started.elapsed())), &frames, &mut emit)
}

fn emit(line: DictationLine) {
    let Ok(text) = serde_json::to_string(&line) else { return };
    let mut out = std::io::stdout().lock();
    // A closed pipe means the app is gone; stdin's end says so too.
    let _ = writeln!(out, "{text}").and_then(|()| out.flush());
}

/// `input`'s frames onto `frames` until it ends, or until it cannot be trusted.
fn read_frames(mut input: impl Read, frames: &Sender<Result<Frame, DictationError>>) {
    let (mut stream, mut chunk) = (Vec::new(), [0u8; 8192]);
    loop {
        match input.read(&mut chunk) {
            Ok(0) => return,
            Ok(n) => stream.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(_) => return,
        }
        loop {
            match Frame::take(&mut stream) {
                Ok(None) => break,
                Ok(Some(frame)) => {
                    if frames.send(Ok(frame)).is_err() {
                        return;
                    }
                }
                Err(e) => {
                    let _ = frames.send(Err(e));
                    return;
                }
            }
        }
    }
}

/// Frames in, lines out, until the frames end. All the sound waiting goes in
/// before a pass, so the pass is over what was said by now.
fn run(listener: &mut Listener<impl PhraseRecognizer, impl PhraseDetector>, frames: &Receiver<Result<Frame, DictationError>>, say: Line) -> i32 {
    loop {
        let Ok(first) = frames.recv() else { return APP_GONE };
        for frame in std::iter::once(first).chain(frames.try_iter()) {
            match frame {
                Ok(frame) => listener.take(frame, say),
                Err(e) => {
                    say(DictationLine::Error { message: e.to_string() });
                    return 1;
                }
            }
        }
        listener.pass(say);
    }
}

/// One dictation at a time, from its sound to its lines.
struct Listener<R, D> {
    recognizer: R,
    detector: D,
    clock: Box<dyn Fn() -> Duration>,
    /// Sound since the last phrase ended, or the last moments of it while
    /// nothing is open: nothing more is ever kept.
    heard: Vec<f32>,
    /// Where in `heard` the open phrase starts.
    open_at: Option<usize>,
    passes: PartialPasses,
    /// The window shows a `partial`, which only a line can take away.
    shown: bool,
}

impl<R: PhraseRecognizer, D: PhraseDetector> Listener<R, D> {
    fn new(recognizer: R, detector: D, clock: Box<dyn Fn() -> Duration>) -> Self {
        Listener { recognizer, detector, clock, heard: Vec::new(), open_at: None, passes: PartialPasses::default(), shown: false }
    }

    fn take(&mut self, frame: Frame, say: Line) {
        match frame {
            Frame::Audio(samples) => self.hear(&samples, say),
            Frame::Finish => {
                self.detector.flush();
                self.close_phrases(say);
                self.clear_shown(say);
                self.forget();
                say(DictationLine::Done);
            }
            Frame::Cancel => {
                self.forget();
                say(DictationLine::Done);
            }
        }
    }

    fn hear(&mut self, samples: &[f32], say: Line) {
        self.detector.accept(samples);
        let before = self.heard.len();
        self.heard.extend_from_slice(samples);
        if self.detector.speaking() {
            self.open_at.get_or_insert(before.saturating_sub(LOOKBACK));
        }
        if self.open_at.is_some_and(|at| self.heard.len() - at >= LONGEST_PHRASE) {
            self.detector.flush();
        }
        self.close_phrases(say);
        if self.open_at.is_none() {
            self.clear_shown(say);
            self.heard.drain(..self.heard.len().saturating_sub(LOOKBACK));
        }
    }

    /// Each phrase ended, recognized once. VAD ends none without saying so,
    /// so nothing is open after: speech going on past a cut opens the next
    /// one with the next sound.
    fn close_phrases(&mut self, say: Line) {
        while let Some(phrase) = self.detector.next_phrase() {
            if let Some(text) = spoken(&self.recognizer.recognize(&phrase)) {
                say(DictationLine::Phrase { text: text.to_string() });
                self.shown = false;
            }
            self.passes.phrase_closed();
            self.heard.clear();
            self.open_at = None;
        }
    }

    /// The open phrase recognized again, if it is time.
    fn pass(&mut self, say: Line) {
        let Some(open) = self.open_at.map(|at| &self.heard[at..]) else { return };
        if !self.passes.due(open.len(), (self.clock)()) {
            return;
        }
        let text = self.recognizer.recognize(open);
        self.passes.passed(open.len(), (self.clock)());
        if let Some(text) = spoken(&text) {
            say(DictationLine::Partial { text: text.to_string() });
            self.shown = true;
        }
    }

    /// Nothing is open, so nothing is to stay grey: a phrase of no words ended it.
    fn clear_shown(&mut self, say: Line) {
        if std::mem::take(&mut self.shown) {
            say(DictationLine::Partial { text: String::new() });
        }
    }

    fn forget(&mut self) {
        self.detector.reset();
        self.heard.clear();
        self.open_at = None;
        self.passes.phrase_closed();
        self.shown = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;
    use std::rc::Rc;

    use crate::domain::dictation::PARTIAL_STEP;

    const SECOND: usize = RATE as usize;
    const PAUSE: usize = SECOND * 6 / 10;

    /// Sound above zero is speech: a phrase ends on a pause, and only there.
    #[derive(Default)]
    struct Vad {
        open: RefCell<Vec<f32>>,
        quiet: Cell<usize>,
        ended: RefCell<VecDeque<Vec<f32>>>,
    }

    impl Vad {
        fn end(&self) {
            let phrase = std::mem::take(&mut *self.open.borrow_mut());
            self.ended.borrow_mut().push_back(phrase);
            self.quiet.set(0);
        }
    }

    impl PhraseDetector for Vad {
        fn accept(&self, samples: &[f32]) {
            for &s in samples {
                match s != 0.0 {
                    true => {
                        self.quiet.set(0);
                        self.open.borrow_mut().push(s);
                    }
                    false if !self.open.borrow().is_empty() => {
                        self.quiet.set(self.quiet.get() + 1);
                        if self.quiet.get() == PAUSE {
                            self.end();
                        }
                    }
                    false => {}
                }
            }
        }
        fn speaking(&self) -> bool {
            !self.open.borrow().is_empty()
        }
        fn next_phrase(&self) -> Option<Vec<f32>> {
            self.ended.borrow_mut().pop_front()
        }
        fn flush(&self) {
            if self.speaking() {
                self.end();
            }
        }
        fn reset(&self) {
            *self.open.borrow_mut() = Vec::new();
            self.ended.borrow_mut().clear();
        }
    }

    /// Text saying how much it heard, each pass taking `cost` on the clock;
    /// punctuation alone once it `hears_noise`.
    struct Words {
        now: Rc<Cell<Duration>>,
        cost: Duration,
        passes: Rc<Cell<usize>>,
        hears_noise: Rc<Cell<bool>>,
    }

    impl PhraseRecognizer for Words {
        fn recognize(&self, samples: &[f32]) -> String {
            self.now.set(self.now.get() + self.cost);
            self.passes.set(self.passes.get() + 1);
            match self.hears_noise.get() {
                false => format!("  слова {}  ", samples.len()),
                true => " . ".into(),
            }
        }
    }

    struct Rig {
        listener: Listener<Words, Vad>,
        now: Rc<Cell<Duration>>,
        passes: Rc<Cell<usize>>,
        hears_noise: Rc<Cell<bool>>,
        lines: Vec<DictationLine>,
    }

    fn rig(cost: Duration) -> Rig {
        let (now, passes, hears_noise) = (Rc::new(Cell::new(Duration::ZERO)), Rc::new(Cell::new(0)), Rc::new(Cell::new(false)));
        let clock = now.clone();
        let words = Words { now: now.clone(), cost, passes: passes.clone(), hears_noise: hears_noise.clone() };
        Rig { listener: Listener::new(words, Vad::default(), Box::new(move || clock.get())), now, passes, hears_noise, lines: Vec::new() }
    }

    impl Rig {
        fn take(&mut self, frame: Frame) {
            let lines = &mut self.lines;
            self.listener.take(frame, &mut |l| lines.push(l));
        }

        /// `n` samples of `level`, in tenths of a second that take their time, a pass after each.
        fn say(&mut self, level: f32, n: usize) {
            for piece in vec![level; n].chunks(SECOND / 10) {
                self.now.set(self.now.get() + Duration::from_millis(100));
                self.take(Frame::Audio(piece.to_vec()));
                let lines = &mut self.lines;
                self.listener.pass(&mut |l| lines.push(l));
            }
        }

        fn phrases(&self) -> Vec<&str> {
            self.lines.iter().filter_map(|l| if let DictationLine::Phrase { text } = l { Some(text.as_str()) } else { None }).collect()
        }

        fn partials(&self) -> Vec<&str> {
            self.lines.iter().filter_map(|l| if let DictationLine::Partial { text } = l { Some(text.as_str()) } else { None }).collect()
        }
    }

    #[test]
    fn a_phrase_ends_on_a_pause_and_is_recognized_once_whole() {
        let mut r = rig(Duration::ZERO);
        r.say(0.5, SECOND);
        r.say(0.0, PAUSE - 1600);
        assert!(r.phrases().is_empty(), "the pause is not over");
        r.say(0.0, 1600);
        assert_eq!(r.phrases(), ["слова 16000"], "the phrase alone, trimmed");
        let passes = r.passes.get();
        r.say(0.0, SECOND);
        assert_eq!(r.passes.get(), passes, "silence after it costs nothing");
    }

    #[test]
    fn a_phrase_cut_for_its_length_goes_on_as_the_next_one() {
        let mut r = rig(Duration::ZERO);
        r.say(0.5, LONGEST_PHRASE + SECOND);
        assert_eq!(r.phrases(), [format!("слова {LONGEST_PHRASE}")], "VAD saw no end to it");
        assert_eq!(r.partials().last(), Some(&"слова 16000"), "what is open starts at the cut");
        r.take(Frame::Finish);
        assert_eq!(r.phrases().last(), Some(&"слова 16000"));
    }

    #[test]
    fn the_open_phrase_starts_a_little_before_vad_said_so() {
        let mut r = rig(Duration::ZERO);
        r.say(0.0, SECOND);
        r.say(0.5, SECOND);
        // The first tenth of a second that speaks opens it, with the sound before.
        assert_eq!(r.partials().first(), Some(&format!("слова {}", LOOKBACK + SECOND / 10).as_str()));
    }

    #[test]
    fn the_step_counts_from_where_a_slow_pass_ended() {
        let mut slow = rig(PARTIAL_STEP * 2);
        slow.say(0.5, 5 * SECOND);
        // 50 tenths of a second, and five of them between passes however long a pass took.
        assert!((5..=10).contains(&slow.partials().len()), "{}", slow.partials().len());
    }

    #[test]
    fn nothing_new_is_not_recognized_again() {
        let mut r = rig(Duration::ZERO);
        r.say(0.5, SECOND);
        let passes = r.passes.get();
        r.now.set(r.now.get() + Duration::from_secs(5));
        let lines = &mut r.lines;
        r.listener.pass(&mut |l| lines.push(l));
        assert_eq!(r.passes.get(), passes);
    }

    #[test]
    fn finish_says_the_tail_then_done_and_the_next_dictation_starts_clean() {
        let mut r = rig(Duration::ZERO);
        r.say(0.5, SECOND);
        r.take(Frame::Finish);
        let tail = DictationLine::Phrase { text: format!("слова {SECOND}") };
        assert_eq!(r.lines[r.lines.len() - 2..], [tail, DictationLine::Done]);

        r.lines.clear();
        r.say(0.5, SECOND);
        assert_eq!(r.partials().first(), Some(&"слова 8000"), "from its own start, with no lookback into the last one");
    }

    #[test]
    fn cancel_says_only_done_and_forgets_what_was_open() {
        let mut r = rig(Duration::ZERO);
        r.say(0.5, 3 * SECOND);
        r.lines.clear();
        r.take(Frame::Cancel);
        assert_eq!(r.lines, [DictationLine::Done]);

        r.lines.clear();
        r.say(0.5, SECOND);
        assert_eq!(r.partials().first(), Some(&"слова 8000"), "a shorter phrase than the one thrown away still shows");
        r.take(Frame::Finish);
        assert_eq!(r.phrases(), [format!("слова {SECOND}")], "nothing of the old sound in it");
    }

    #[test]
    fn a_dictation_ending_on_silence_leaves_the_next_none_of_it() {
        let mut r = rig(Duration::ZERO);
        r.say(0.0, SECOND);
        r.take(Frame::Finish);
        r.say(0.5, SECOND);
        assert_eq!(r.partials().first(), Some(&"слова 8000"), "no lookback into the last one");
    }

    #[test]
    fn a_tick_right_after_cancel_finds_nothing_open() {
        let mut r = rig(Duration::ZERO);
        r.say(0.0, SECOND);
        r.say(0.5, SECOND);
        r.take(Frame::Cancel);
        r.lines.clear();
        r.now.set(r.now.get() + Duration::from_secs(5));
        let lines = &mut r.lines;
        r.listener.pass(&mut |l| lines.push(l));
        r.say(0.0, SECOND);
        assert!(r.lines.is_empty(), "the window took its grey text away itself");
    }

    #[test]
    fn the_open_phrase_shows_as_sound_comes_through_the_loop() {
        let (sender, frames) = mpsc::channel();
        sender.send(Ok(Frame::Audio(vec![0.5; SECOND]))).unwrap();
        drop(sender);
        let mut r = rig(Duration::ZERO);
        let lines = &mut r.lines;
        run(&mut r.listener, &frames, &mut |l| lines.push(l));
        assert_eq!(r.partials(), [format!("слова {SECOND}")]);
    }

    #[test]
    fn a_phrase_of_no_words_takes_its_grey_text_away() {
        let mut r = rig(Duration::ZERO);
        r.say(0.5, SECOND);
        assert!(!r.partials().is_empty(), "it looked like words");
        r.lines.clear();
        r.hears_noise.set(true);
        r.say(0.0, PAUSE);
        assert_eq!(r.lines, [DictationLine::Partial { text: String::new() }]);
        r.say(0.0, SECOND);
        assert_eq!(r.lines.len(), 1, "said once");
    }

    #[test]
    fn noise_that_never_showed_leaves_nothing_to_take_away() {
        let mut r = rig(Duration::ZERO);
        r.hears_noise.set(true);
        r.say(0.5, SECOND);
        r.say(0.0, PAUSE);
        assert!(r.lines.is_empty());
    }

    #[test]
    fn finish_on_a_phrase_of_no_words_clears_it_before_done() {
        let mut r = rig(Duration::ZERO);
        r.say(0.5, SECOND);
        r.lines.clear();
        r.hears_noise.set(true);
        r.take(Frame::Finish);
        assert_eq!(r.lines, [DictationLine::Partial { text: String::new() }, DictationLine::Done]);
    }

    #[test]
    fn little_of_what_was_heard_is_kept_while_nothing_is_open() {
        let mut r = rig(Duration::ZERO);
        r.say(0.0, 10 * SECOND);
        assert_eq!(r.listener.heard.len(), LOOKBACK);
    }

    /// What arrives a few bytes at a time, as a pipe gives it.
    struct Dripping(Vec<u8>);

    impl Read for Dripping {
        fn read(&mut self, into: &mut [u8]) -> std::io::Result<usize> {
            let n = self.0.len().min(3).min(into.len());
            into[..n].copy_from_slice(&self.0[..n]);
            self.0.drain(..n);
            Ok(n)
        }
    }

    #[test]
    fn frames_are_put_together_however_the_pipe_cuts_them() {
        let bytes = [Frame::Audio(vec![0.0; 5]), Frame::Finish].iter().flat_map(Frame::encode).collect();
        let (sender, frames) = mpsc::channel();
        read_frames(Dripping(bytes), &sender);
        drop(sender);
        assert_eq!(frames.iter().collect::<Vec<_>>(), [Ok(Frame::Audio(vec![0.0; 5])), Ok(Frame::Finish)]);
    }

    #[test]
    fn a_stream_out_of_step_ends_the_process_saying_why() {
        let mut bytes = Frame::Finish.encode();
        bytes.extend([9, 0, 0, 0, 0]);
        let (sender, frames) = mpsc::channel();
        read_frames(bytes.as_slice(), &sender);
        let mut r = rig(Duration::ZERO);
        let lines = &mut r.lines;
        assert_eq!(run(&mut r.listener, &frames, &mut |l| lines.push(l)), 1);
        assert_eq!(r.lines, [DictationLine::Done, DictationLine::Error { message: DictationError::UnknownFrame(9).to_string() }]);
    }

    #[test]
    fn stdin_ending_is_the_app_gone_after_what_it_sent() {
        let (sender, frames) = mpsc::channel();
        sender.send(Ok(Frame::Cancel)).unwrap();
        drop(sender);
        let mut r = rig(Duration::ZERO);
        let lines = &mut r.lines;
        assert_eq!(run(&mut r.listener, &frames, &mut |l| lines.push(l)), APP_GONE);
        assert_eq!(r.lines, [DictationLine::Done]);
    }

    #[test]
    fn it_takes_one_path_and_says_so_otherwise() {
        assert_eq!(main(&[]), 2);
        assert_eq!(main(&["a".into(), "b".into()]), 2);
    }

    #[test]
    fn models_missing_is_said_not_crashed_on() {
        let dir = std::env::temp_dir().join(format!("kibo-dictation-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(speech::dictation_models(&dir).err(), Some(DictationError::ModelsMissing));
        // GigaAM from before Silero was in the build.
        speech::DICTATION_FILES[..4].iter().for_each(|f| std::fs::write(dir.join(f), "").unwrap());
        assert_eq!(speech::dictation_models(&dir).err(), Some(DictationError::ModelsMissing));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn listener() -> Listener<speech::Phrases, speech::Silero> {
        let models = Path::new(env!("CARGO_MANIFEST_DIR")).join("models").join("meeting");
        let (recognizer, detector) = speech::dictation_models(&models).expect("the models: bun scripts/fetch-meeting-models.ts");
        let started = Instant::now();
        Listener::new(recognizer, detector, Box::new(move || started.elapsed()))
    }

    /// `sound` heard in tenths of a second, with the passes between.
    fn hear_all(listener: &mut Listener<speech::Phrases, speech::Silero>, sound: &[f32], lines: &mut Vec<DictationLine>) {
        for piece in sound.chunks(SECOND / 10) {
            listener.take(Frame::Audio(piece.to_vec()), &mut |l| lines.push(l));
            listener.pass(&mut |l| lines.push(l));
        }
    }

    /// A phrase, a pause and a phrase in a macOS voice.
    fn voice(label: &str) -> Vec<f32> {
        let dir = std::env::temp_dir().join(format!("kibo-dictation-live-{label}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let wav = dir.join("phrase.wav");
        let said = std::process::Command::new("say")
            .args(["-v", "Milena", "--data-format=LEI16@16000", "--file-format=WAVE", "-o"])
            .arg(&wav)
            .arg("Привет, это проверка голосового ввода. [[slnc 1000]] Сегодня хорошая погода.")
            .status()
            .unwrap();
        assert!(said.success());
        let samples = sherpa_onnx::Wave::read(wav.to_str().unwrap()).unwrap().samples().to_vec();
        let _ = std::fs::remove_dir_all(&dir);
        samples
    }

    fn phrases_of(lines: &[DictationLine]) -> Vec<&str> {
        lines.iter().filter_map(|l| if let DictationLine::Phrase { text } = l { Some(text.as_str()) } else { None }).collect()
    }

    /// Through GigaAM and Silero: `cargo test dictation_live -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn dictation_live() {
        let (mut listener, mut lines) = (listener(), Vec::new());
        hear_all(&mut listener, &voice("phrases"), &mut lines);
        listener.take(Frame::Finish, &mut |l| lines.push(l));
        lines.iter().for_each(|l| println!("{l:?}"));
        assert_eq!(listener.recognizer.recognize(&[]), "", "next to nothing is not a crash");
        let phrases = phrases_of(&lines);
        assert_eq!(phrases.len(), 2, "cut at the pause: {phrases:?}");
        assert!(phrases[0].contains("проверка") && phrases[1].contains("погода"), "{phrases:?}");
        assert_eq!(lines.last(), Some(&DictationLine::Done));
    }

    /// However the sound comes in pieces, the phrases are the same.
    #[test]
    #[ignore]
    fn dictation_live_one_frame() {
        let (mut listener, mut lines) = (listener(), Vec::new());
        listener.take(Frame::Audio(voice("frame")), &mut |l| lines.push(l));
        listener.take(Frame::Finish, &mut |l| lines.push(l));
        let phrases = phrases_of(&lines);
        assert_eq!(phrases.len(), 2, "{phrases:?}");
        assert!(phrases[0].starts_with("Привет"), "{phrases:?}");
    }

    /// What was open when a dictation was thrown away is not in the next one.
    #[test]
    #[ignore]
    fn dictation_live_cancelled() {
        let sound = voice("cancelled");
        let (mut listener, mut lines) = (listener(), Vec::new());
        hear_all(&mut listener, &sound[..SECOND], &mut lines);
        listener.take(Frame::Cancel, &mut |l| lines.push(l));
        lines.clear();
        hear_all(&mut listener, &sound, &mut lines);
        listener.take(Frame::Finish, &mut |l| lines.push(l));
        let phrases = phrases_of(&lines);
        assert_eq!(phrases.len(), 2, "{phrases:?}");
        assert_eq!(phrases[0].matches("Привет").count(), 1, "{phrases:?}");
    }

    /// The sound that says "speak" opens no phrase, heard as it is played.
    #[test]
    #[ignore]
    fn dictation_live_tone() {
        let tone: Vec<f32> = (0..SECOND / 10).map(|i| (i as f32 * 880.0 * std::f32::consts::TAU / SECOND as f32).sin() * 0.5).collect();
        let sound = [tone, vec![0.0; 2 * SECOND]].concat();
        let (mut listener, mut lines) = (listener(), Vec::new());
        hear_all(&mut listener, &sound, &mut lines);
        listener.take(Frame::Finish, &mut |l| lines.push(l));
        assert_eq!(lines, [DictationLine::Done]);
    }
}
