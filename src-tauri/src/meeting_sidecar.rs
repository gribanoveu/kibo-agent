//! The recognition process: this same exe, started by `infra::meeting_process`
//! as `<exe> --meeting-sidecar <audio> <models> <keep>`. `main.rs` comes here before
//! any window or Tauri is up.
//!
//! A process and not a thread: a crash in the native speech library costs one
//! recording, stopping is a kill, and the gigabyte the models take goes back
//! to the system when it exits. It speaks one [`SidecarLine`] a line on stdout.

use std::io::Write;
use std::path::Path;

pub use crate::domain::meeting::SIDECAR_FLAG as FLAG;
use crate::domain::meeting::SidecarLine;
use crate::infra::speech;

/// The process's exit code.
pub fn main(args: &[String]) -> i32 {
    let [audio, models, keep] = args else {
        emit(&SidecarLine::Error { message: format!("usage: {FLAG} <audio> <models> <keep>") });
        return 2;
    };
    // The app holds stdin open and writes nothing: its end is the app gone —
    // quit, crashed or killed — and an hour's recognition is then for no one.
    std::thread::spawn(|| {
        let _ = std::io::copy(&mut std::io::stdin().lock(), &mut std::io::sink());
        std::process::exit(3);
    });
    let mut progress = |stage, done, total| emit(&SidecarLine::Progress { stage, done, total });
    match speech::recognize(Path::new(audio), Path::new(models), Path::new(keep), &mut progress) {
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

fn emit(line: &SidecarLine) {
    let Ok(text) = serde_json::to_string(line) else { return };
    let mut out = std::io::stdout().lock();
    // A closed pipe means the app is gone; nothing is left to tell.
    let _ = writeln!(out, "{text}").and_then(|()| out.flush());
}
