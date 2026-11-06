//! The recognition process from the app's side: `<exe> --dictation-sidecar`
//! started, frames down its stdin and its lines read (`dictation_sidecar`).
//! Unlike a meeting's it lives between jobs, so whoever holds it decides how
//! long; dropped, it is killed.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Duration;

use crate::domain::dictation::{DictationError, DictationLine, Frame, Recognition, SIDECAR_FLAG};

pub struct Process {
    child: Child,
    stdin: ChildStdin,
    /// Its lines, read by a thread of their own; ended with its stdout.
    lines: Receiver<DictationLine>,
}

impl Process {
    /// `exe`, the app's own or a stand-in under test, loading from `models`.
    pub fn start(exe: &Path, models: &Path) -> Result<Self, DictationError> {
        let not_started = |e: String| DictationError::NotStarted(format!("{}: {e}", exe.display()));
        let mut child = Command::new(exe)
            .arg(SIDECAR_FLAG)
            .arg(models)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| not_started(e.to_string()))?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(not_started("no pipes to it".into()));
        };
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                match serde_json::from_str(&line) {
                    Ok(line) => {
                        if sender.send(line).is_err() {
                            return;
                        }
                    }
                    Err(_) => eprintln!("dictation sidecar: {line}"),
                }
            }
        });
        Ok(Process { child, stdin, lines })
    }

    /// Why it is gone, for a message.
    fn ended(&mut self) -> DictationError {
        DictationError::ProcessEnded(match self.child.try_wait() {
            Ok(Some(status)) => status.to_string(),
            _ => "it stopped talking".to_string(),
        })
    }
}

impl Recognition for Process {
    fn send(&mut self, frame: &Frame) -> Result<(), DictationError> {
        self.stdin.write_all(&frame.encode()).map_err(|_| self.ended())
    }

    fn line(&mut self, wait: Duration) -> Result<Option<DictationLine>, DictationError> {
        match self.lines.recv_timeout(wait) {
            Ok(line) => Ok(Some(line)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(self.ended()),
        }
    }

    fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::time::Instant;

    const WAIT: Duration = Duration::from_secs(5);

    /// A stand-in for the exe: a shell script that checks its arguments and runs `body`.
    fn sidecar(label: &str, body: &str) -> PathBuf {
        let path = crate::testing::temp_dir(label).join("sidecar");
        let script = format!("#!/bin/sh\n[ \"$1\" = {SIDECAR_FLAG} ] && [ \"$2\" = models ] || exit 9\n{body}\n");
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn start(exe: &Path) -> Process {
        Process::start(exe, Path::new("models")).unwrap()
    }

    fn until(mut done: impl FnMut() -> bool) {
        let started = Instant::now();
        while !done() {
            assert!(started.elapsed() < WAIT, "never happened");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn its_lines_are_read_and_what_is_not_one_is_passed_over() {
        let exe = sidecar("dictation-lines", r#"echo '{"type":"ready"}'
echo 'sherpa says hello'
echo '{"type":"phrase","text":"да"}'
exec sleep 30"#);
        let mut process = start(&exe);
        assert_eq!(process.line(WAIT), Ok(Some(DictationLine::Ready)));
        assert_eq!(process.line(WAIT), Ok(Some(DictationLine::Phrase { text: "да".into() })));
        assert_eq!(process.line(Duration::from_millis(20)), Ok(None), "nothing more said yet");
        assert!(process.alive());
    }

    #[test]
    fn a_frame_goes_down_its_stdin_as_it_is() {
        let written = crate::testing::temp_dir("dictation-stdin").join("heard");
        let exe = sidecar("dictation-stdin", &format!("exec cat > {}", written.display()));
        let mut process = start(&exe);
        let frames = [Frame::Audio(vec![0.5, -0.5]), Frame::Finish];
        frames.iter().for_each(|frame| process.send(frame).unwrap());
        let whole: Vec<u8> = frames.iter().flat_map(Frame::encode).collect();
        until(|| std::fs::read(&written).is_ok_and(|bytes| bytes == whole));
    }

    #[test]
    fn its_end_is_said_and_writing_to_it_fails() {
        let exe = sidecar("dictation-gone", "exit 4");
        let mut process = start(&exe);
        let Err(DictationError::ProcessEnded(_)) = process.line(WAIT) else { panic!("its output ended") };
        until(|| !process.alive());
        let Err(DictationError::ProcessEnded(status)) = process.line(WAIT) else { panic!() };
        assert!(status.contains('4'), "{status}");
        let sound = Frame::Audio(vec![0.0; 1 << 16]);
        assert!(matches!(process.send(&sound), Err(DictationError::ProcessEnded(_))), "nobody reads it");
    }

    #[test]
    fn one_that_does_not_start_says_what() {
        let Err(DictationError::NotStarted(message)) = Process::start(Path::new("/no/such/exe"), Path::new("models")) else { panic!() };
        assert!(message.contains("/no/such/exe"), "{message}");
    }

    #[test]
    fn dropped_it_is_killed() {
        let pid = crate::testing::temp_dir("dictation-kill").join("pid");
        let exe = sidecar("dictation-kill", &format!("echo $$ > {}\nexec sleep 30", pid.display()));
        let process = start(&exe);
        until(|| std::fs::read_to_string(&pid).is_ok_and(|p| p.ends_with('\n')));
        let pid = std::fs::read_to_string(&pid).unwrap();
        drop(process);
        let alive = Command::new("kill").args(["-0", pid.trim()]).stderr(Stdio::null()).status().unwrap().success();
        assert!(!alive, "dropped means gone by the time it returns");
    }
}
