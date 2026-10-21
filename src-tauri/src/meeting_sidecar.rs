//! The recognition process: this same exe, started by `infra::meeting_process`
//! as `<exe> --meeting-sidecar <audio> <models> <keep> [--call | --call-denoised]`. `main.rs` comes here before
//! any window or Tauri is up.
//!
//! A process and not a thread: a crash in the native speech library costs one
//! recording, stopping is a kill, and the gigabyte the models take goes back
//! to the system when it exits. It speaks one [`SidecarLine`] a line on stdout.

use std::io::Write;
use std::path::Path;

pub use crate::domain::meeting::SIDECAR_FLAG as FLAG;
use crate::domain::meeting::{SidecarLine, Sound, CALL_FLAG, DENOISED_CALL_FLAG};
use crate::infra::speech;

/// The process's exit code.
pub fn main(args: &[String]) -> i32 {
    let Some(([audio, models, keep], sound)) = parse(args) else {
        emit(&SidecarLine::Error { message: format!("usage: {FLAG} <audio> <models> <keep> [{CALL_FLAG} | {DENOISED_CALL_FLAG}]") });
        return 2;
    };
    // The app holds stdin open and writes nothing: its end is the app gone —
    // quit, crashed or killed — and an hour's recognition is then for no one.
    std::thread::spawn(|| {
        let _ = std::io::copy(&mut std::io::stdin().lock(), &mut std::io::sink());
        std::process::exit(3);
    });
    let mut progress = |stage, done, total| emit(&SidecarLine::Progress { stage, done, total });
    match speech::recognize(Path::new(audio), Path::new(models), Path::new(keep), sound, &mut progress) {
        Ok(recognized) => {
            emit(&SidecarLine::Result(recognized));
            0
        }
        Err(e) => {
            emit(&SidecarLine::Error { message: e.to_string() });
            1
        }
    }
}

/// The three paths, and what the sound is.
fn parse(args: &[String]) -> Option<([&String; 3], Sound)> {
    let [audio, models, keep, flags @ ..] = args else { return None };
    Some(([audio, models, keep], Sound::of_flags(flags)))
}

fn emit(line: &SidecarLine) {
    let Ok(text) = serde_json::to_string(line) else { return };
    let mut out = std::io::stdout().lock();
    // A closed pipe means the app is gone; nothing is left to tell.
    let _ = writeln!(out, "{text}").and_then(|()| out.flush());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn its_arguments_are_three_paths_and_maybe_what_the_sound_is() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let three = args(&["a.wav", "models", "keep.wav"]);
        assert_eq!(parse(&three).map(|(p, s)| (p.map(String::as_str), s)), Some((["a.wav", "models", "keep.wav"], Sound::Import)));
        let sound = |flag: &str| parse(&args(&["a.wav", "models", "keep.wav", flag])).map(|(_, s)| s);
        assert_eq!(sound(CALL_FLAG), Some(Sound::Call { denoise_mic: false }));
        assert_eq!(sound(DENOISED_CALL_FLAG), Some(Sound::Call { denoise_mic: true }));
        assert_eq!(sound("--other"), Some(Sound::Import));
        for s in [Sound::Import, Sound::Call { denoise_mic: false }, Sound::Call { denoise_mic: true }] {
            assert_eq!(Sound::of_flags(&s.flag().into_iter().map(String::from).collect::<Vec<_>>()), s, "back as it went");
        }
        assert!(parse(&args(&["a.wav", "models"])).is_none());
    }
}
