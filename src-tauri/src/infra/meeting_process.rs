//! The recognition process from the app's side: start `<exe> --meeting-sidecar`,
//! read its lines, and kill it when asked to stop (`meeting_sidecar`).

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

use crate::domain::meeting::{MeetingError, Recognized, SidecarLine, Stage, SIDECAR_FLAG};

/// The process while one runs, for [`Running::stop`] to kill. One at a time is
/// the caller's to keep (`services::meeting_notes`).
#[derive(Default)]
pub struct Running(Mutex<Option<Child>>);

impl Running {
    pub fn stop(&self) {
        let child = self.0.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(mut child) = child {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// `audio` recognized by `exe` — the app's own, or a stand-in under test —
/// and the sound it heard written to `keep`.
pub fn recognize(
    running: &Running,
    exe: &Path,
    audio: &Path,
    models: &Path,
    keep: &Path,
    progress: &mut dyn FnMut(Stage, usize, usize),
) -> Result<Recognized, MeetingError> {
    let stdout = {
        let mut slot = running.0.lock().unwrap_or_else(|e| e.into_inner());
        let mut child = Command::new(exe)
            .arg(SIDECAR_FLAG)
            .arg(audio)
            .arg(models)
            .arg(keep)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| MeetingError::Recognition(format!("could not start {}: {e}", exe.display())))?;
        let stdout = child.stdout.take();
        *slot = Some(child);
        stdout
    };

    let mut result = None;
    let mut error = None;
    for line in stdout.into_iter().flat_map(|out| BufReader::new(out).lines().map_while(Result::ok)) {
        match serde_json::from_str::<SidecarLine>(&line) {
            Ok(SidecarLine::Progress { stage, done, total }) => progress(stage, done, total),
            Ok(SidecarLine::Result(recognized)) => result = Some(recognized),
            Ok(SidecarLine::Error { message }) => error = Some(message),
            Err(_) => eprintln!("meeting sidecar: {line}"),
        }
    }

    // Its output ended: it finished, crashed, or `stop` took it and killed it.
    let child = running.0.lock().unwrap_or_else(|e| e.into_inner()).take();
    let Some(mut child) = child else { return Err(MeetingError::Cancelled) };
    let status = child.wait().map_err(|e| MeetingError::Recognition(e.to_string()))?;
    match (result, error) {
        // A result is whole once written, even if the process fails on its way out.
        (Some(result), _) => Ok(result),
        (_, Some(message)) => Err(MeetingError::Recognition(message)),
        _ => Err(MeetingError::Recognition(format!("the recognition process ended without a result ({status})"))),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::domain::meeting::Utterance;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    /// A stand-in for the exe: a shell script that checks its arguments and runs `body`.
    fn sidecar(label: &str, body: &str) -> PathBuf {
        let path = crate::testing::temp_dir(label).join("sidecar");
        let script = format!("#!/bin/sh\n[ \"$1\" = {SIDECAR_FLAG} ] && [ \"$2\" = a.wav ] && [ \"$3\" = models ] && [ \"$4\" = a.kept.wav ] || exit 9\n{body}\n");
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn run(running: &Running, exe: &Path) -> (Result<Recognized, MeetingError>, Vec<(Stage, usize, usize)>) {
        let mut seen = Vec::new();
        let result = recognize(running, exe, Path::new("a.wav"), Path::new("models"), Path::new("a.kept.wav"), &mut |s, d, t| seen.push((s, d, t)));
        (result, seen)
    }

    #[test]
    fn its_progress_is_reported_and_its_result_returned() {
        let exe = sidecar(
            "sidecar-ok",
            r#"echo '{"type":"progress","stage":"speech","done":1,"total":2}'
echo 'sherpa says hello'
echo '{"type":"result","utterances":[{"speaker":0,"start":0.0,"end":1.0,"text":"да"}],"seconds":3.5}'"#,
        );
        let running = Running::default();
        let (result, seen) = run(&running, &exe);
        let recognized = result.unwrap();
        assert_eq!(recognized.utterances, [Utterance { speaker: 0, start: 0.0, end: 1.0, text: "да".into() }]);
        assert_eq!((recognized.seconds, recognized.voices), (3.5, vec![]), "a result without voices still reads");
        assert_eq!(seen, [(Stage::Speech, 1, 2)]);
        assert_eq!(run(&running, &exe).0.map(|r| r.seconds), Ok(3.5), "the slot is free again");
    }

    #[test]
    fn its_error_or_its_crash_is_the_failure() {
        let failed = sidecar("sidecar-error", r#"echo '{"type":"error","message":"no sound"}'; exit 1"#);
        assert_eq!(run(&Running::default(), &failed).0, Err(MeetingError::Recognition("no sound".into())));
        let crashed = sidecar("sidecar-crash", "kill -SEGV $$");
        let Err(MeetingError::Recognition(message)) = run(&Running::default(), &crashed).0 else { panic!() };
        assert!(message.contains("without a result"), "{message}");
    }

    #[test]
    fn a_stop_kills_it() {
        let exe = sidecar("sidecar-slow", "exec sleep 30");
        let running = Arc::new(Running::default());
        let worker = {
            let running = running.clone();
            std::thread::spawn(move || run(&running, &exe).0)
        };
        let started = Instant::now();
        while running.0.lock().unwrap().is_none() {
            std::thread::sleep(Duration::from_millis(10));
        }
        running.stop();
        assert_eq!(worker.join().unwrap(), Err(MeetingError::Cancelled));
        assert!(started.elapsed() < Duration::from_secs(10));
    }
}
