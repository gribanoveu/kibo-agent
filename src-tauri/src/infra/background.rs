//! The background processes themselves — see `domain::background`.
//!
//! Each runs in a process group of its own, like a `runCommand` command, so
//! stopping it stops what it started too: `npm run dev` is a tree. Its two
//! streams are read by threads into one buffer, and a third thread notices
//! the exit. A process that exits on its own has its group killed as well,
//! for the same reason `process_runner` does it — something it left behind
//! would otherwise hold the pipes and run on unseen.

use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::domain::background::{
    BackgroundError, BackgroundProcesses, Feed, OutputBuffer, ProcessChanged, ProcessEventSink, ProcessInfo,
    ProcessOutput, ProcessState, MAX_FINISHED, MAX_RUNNING,
};
use crate::domain::command_exec::{truncate_output, Shell, MAX_OUTPUT_CHARS};
use crate::sync::lock;

use super::process_runner::{kill_tree, set_process_group, Utf8Decoder};

/// How often an exit is looked for — and new output, which is signalled at
/// most this often: a server writing a line per millisecond is one signal a
/// tick, not a thousand.
const POLL: Duration = Duration::from_millis(100);

pub struct Processes {
    registry: Arc<Mutex<Registry>>,
    /// Told of every start, output, end and stop.
    changed: ProcessEventSink,
}

/// Nobody listening — tests, and anything built before the window is.
impl Default for Processes {
    fn default() -> Self {
        Self::new(Arc::new(|_| {}))
    }
}

#[derive(Default)]
struct Registry {
    next_id: u32,
    entries: Vec<Entry>,
}

struct Entry {
    info: ProcessInfo,
    /// `Some` while it runs.
    child: Option<Child>,
    buffer: OutputBuffer,
    /// Pipes still being read. Output can arrive after the exit — the pipes
    /// drain once the process is gone — so its watch lasts until this is 0.
    streams: usize,
    /// Its end has been told to the model, or needs no telling.
    reported: bool,
}

impl Registry {
    fn entry(&mut self, id: u32) -> Result<&mut Entry, BackgroundError> {
        self.entries.iter_mut().find(|e| e.info.id == id).ok_or(BackgroundError::NotFound(id))
    }
}

impl Processes {
    pub fn new(changed: ProcessEventSink) -> Self {
        Self { registry: Arc::default(), changed }
    }

    /// Stops one the user asked to stop, from the Terminal tab. Unlike the
    /// model's own `stopProcess`, the model is told at its next round.
    pub fn stop_by_user(&self, id: u32) -> Result<ProcessInfo, BackgroundError> {
        self.stop_inner(id, false)
    }

    /// The last `max_bytes` of a process's output, for the tab — without
    /// moving the model's place.
    pub fn tail(&self, id: u32, max_bytes: usize) -> Result<String, BackgroundError> {
        Ok(lock(&self.registry).entry(id)?.buffer.tail(max_bytes).to_string())
    }

    /// The workspace changed or the app is quitting. Nothing is reported:
    /// there is no conversation left that these belong to.
    pub fn stop_all(&self) {
        let mut stopped = Vec::new();
        {
            let mut registry = lock(&self.registry);
            for entry in registry.entries.iter_mut() {
                if let Some(mut child) = entry.child.take() {
                    end(&mut child);
                    entry.info.state = ProcessState::Stopped;
                    stopped.push(entry.info.id);
                }
                entry.reported = true;
            }
        }
        for id in stopped {
            (self.changed)(ProcessChanged { id });
        }
    }

    fn stop_inner(&self, id: u32, reported: bool) -> Result<ProcessInfo, BackgroundError> {
        let (info, stopped) = {
            let mut registry = lock(&self.registry);
            let entry = registry.entry(id)?;
            let stopped = match entry.child.take() {
                Some(mut child) => {
                    end(&mut child);
                    entry.info.state = ProcessState::Stopped;
                    entry.reported = reported;
                    true
                }
                None => false,
            };
            (entry.info.clone(), stopped)
        };
        if stopped {
            (self.changed)(ProcessChanged { id });
        }
        Ok(info)
    }
}

/// Refused past [`MAX_RUNNING`], naming what runs.
fn room(registry: &Registry) -> Result<(), BackgroundError> {
    let running: Vec<String> =
        registry.entries.iter().filter(|e| e.info.running()).map(|e| e.info.describe()).collect();
    if running.len() >= MAX_RUNNING {
        return Err(BackgroundError::TooMany(running.join("; ")));
    }
    Ok(())
}

impl Processes {
    /// Enters a running child under the next number, with `backlog` unread
    /// and two streams still to drain, and starts watching it. The feeds are
    /// the streams' way in.
    fn register(
        &self,
        registry: &mut Registry,
        child: Child,
        command: &str,
        shown_cwd: &str,
        backlog: &str,
    ) -> (ProcessInfo, [Feed; 2]) {
        registry.next_id += 1;
        let id = registry.next_id;
        let info = ProcessInfo { id, command: command.to_string(), cwd: shown_cwd.to_string(), state: ProcessState::Running };
        let mut buffer = OutputBuffer::default();
        buffer.push(backlog);
        registry.entries.push(Entry { info: info.clone(), child: Some(child), buffer, streams: 2, reported: false });
        watch(&self.registry, &self.changed, id);

        // Forget the oldest finished ones past the limit.
        let finished = registry.entries.iter().filter(|e| !e.info.running()).count();
        let mut excess = finished.saturating_sub(MAX_FINISHED);
        registry.entries.retain(|e| {
            let drop = excess > 0 && !e.info.running();
            if drop {
                excess -= 1;
            }
            !drop
        });
        (info, [feed(&self.registry, id), feed(&self.registry, id)])
    }
}

impl Drop for Processes {
    fn drop(&mut self) {
        self.stop_all();
    }
}

fn end(child: &mut Child) {
    kill_tree(child);
    let _ = child.kill();
    let _ = child.wait();
}

impl BackgroundProcesses for Processes {
    fn start(&self, shell: &Shell, command: &str, cwd: &Path, shown_cwd: &str) -> Result<ProcessInfo, BackgroundError> {
        let mut registry = lock(&self.registry);
        room(&registry)?;

        let mut process = Command::new(&shell.program);
        process
            .args(&shell.args)
            .arg(command)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        set_process_group(&mut process);
        super::login_path::apply(&mut process);
        let mut child = process.spawn().map_err(|e| BackgroundError::NotStarted(format!("{}: {e}", shell.program)))?;

        let (stdout, stderr) = (child.stdout.take(), child.stderr.take());
        let (info, [out, err]) = self.register(&mut registry, child, command, shown_cwd, "");
        // Under the lock still: a reader's first chunk waits for the entry.
        read_into(stdout, out);
        read_into(stderr, err);
        drop(registry);
        (self.changed)(ProcessChanged { id: info.id });
        Ok(info)
    }

    fn adopt(
        &self,
        mut child: Child,
        command: &str,
        shown_cwd: &str,
        backlog: &str,
    ) -> Result<(ProcessInfo, [Feed; 2]), BackgroundError> {
        let mut registry = lock(&self.registry);
        if let Err(full) = room(&registry) {
            end(&mut child);
            return Err(full);
        }
        let adopted = self.register(&mut registry, child, command, shown_cwd, backlog);
        drop(registry);
        (self.changed)(ProcessChanged { id: adopted.0.id });
        Ok(adopted)
    }

    fn read(&self, id: u32) -> Result<ProcessOutput, BackgroundError> {
        let mut registry = lock(&self.registry);
        let entry = registry.entry(id)?;
        let (unread, missed) = entry.buffer.take_unread();
        let (output, truncated) = truncate_output(&unread, MAX_OUTPUT_CHARS);
        Ok(ProcessOutput { process: entry.info.clone(), output, missed, truncated })
    }

    fn stop(&self, id: u32) -> Result<ProcessInfo, BackgroundError> {
        self.stop_inner(id, true)
    }

    fn list(&self) -> Vec<ProcessInfo> {
        lock(&self.registry).entries.iter().map(|e| e.info.clone()).collect()
    }

    fn take_ended(&self) -> Vec<ProcessInfo> {
        let mut registry = lock(&self.registry);
        registry
            .entries
            .iter_mut()
            .filter(|e| !e.info.running() && !e.reported)
            .map(|e| {
                e.reported = true;
                e.info.clone()
            })
            .collect()
    }
}

/// Where one stream of process `id` goes: into its buffer, and its close
/// counted. Signalling is `watch`'s, on its tick. A process already
/// forgotten takes nothing.
fn feed(registry: &Arc<Mutex<Registry>>, id: u32) -> Feed {
    let registry = Arc::clone(registry);
    Box::new(move |text| {
        if let Ok(entry) = lock(&registry).entry(id) {
            match text {
                Some(text) => entry.buffer.push(text),
                None => entry.streams -= 1,
            }
        }
    })
}

/// Decoded like `process_runner`'s reader: a character split between two
/// reads arrives whole. No pipe is a stream already closed — on its thread
/// even then: `start` calls this holding the registry.
fn read_into(mut pipe: Option<impl Read + Send + 'static>, mut feed: Feed) {
    thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        let mut decoder = Utf8Decoder::default();
        let mut deliver = |text: String| {
            if !text.is_empty() {
                feed(Some(&text));
            }
        };
        while let Some(pipe) = pipe.as_mut() {
            match pipe.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => deliver(decoder.decode(&chunk[..n])),
            }
        }
        deliver(decoder.finish());
        feed(None);
    });
}

/// Each tick: has it exited, and has it written anything since the last one.
/// Either is one signal. After the exit it keeps ticking until the pipes are
/// drained, so the last of the output is signalled too. A stop ends it
/// without a signal — the stop sends its own.
fn watch(registry: &Arc<Mutex<Registry>>, changed: &ProcessEventSink, id: u32) {
    let registry = Arc::clone(registry);
    let changed = Arc::clone(changed);
    let mut seen = 0;
    thread::spawn(move || loop {
        thread::sleep(POLL);
        let (signal, done) = {
            let mut registry = lock(&registry);
            let Ok(entry) = registry.entry(id) else { return };
            if entry.info.state == ProcessState::Stopped {
                return;
            }
            let written = entry.buffer.written();
            let wrote = written != seen;
            seen = written;
            let mut exited = false;
            if let Some(child) = entry.child.as_mut() {
                match child.try_wait() {
                    Ok(None) => {}
                    Ok(Some(status)) => {
                        kill_tree(child);
                        entry.child = None;
                        entry.info.state = ProcessState::Exited { code: status.code() };
                        exited = true;
                    }
                    Err(_) => {
                        entry.child = None;
                        entry.info.state = ProcessState::Exited { code: None };
                        exited = true;
                    }
                }
            }
            (wrote || exited, entry.child.is_none() && entry.streams == 0)
        };
        if signal {
            changed(ProcessChanged { id });
        }
        if done {
            return;
        }
    });
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::testing::temp_dir;
    use std::time::Instant;

    fn start(processes: &Processes, command: &str) -> ProcessInfo {
        processes.start(&Shell::default(), command, &temp_dir("bg"), ".").expect("starts")
    }

    fn until(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn a_process_outlives_its_start_and_is_read_in_pieces() {
        let processes = Processes::default();
        let info = start(&processes, "echo ready; echo oops >&2; sleep 30");
        assert_eq!(info.state, ProcessState::Running);
        let mut seen = String::new();
        until("both streams", || {
            seen.push_str(&processes.read(info.id).unwrap().output);
            seen.contains("ready") && seen.contains("oops")
        });
        assert_eq!(processes.read(info.id).unwrap().output, "", "nothing new");
        assert!(processes.list()[0].running());
    }

    /// See `process_runner`'s test of the same name.
    #[test]
    fn cyrillic_output_longer_than_a_read_arrives_whole() {
        let processes = Processes::default();
        let letters = "д".repeat(10_000);
        let info = start(&processes, &format!("printf 'x{letters}\\320'"));
        let mut seen = String::new();
        until("the exit", || {
            seen.push_str(&processes.read(info.id).unwrap().output);
            !processes.list()[0].running()
        });
        seen.push_str(&processes.read(info.id).unwrap().output);
        assert_eq!(seen, format!("x{letters}\u{FFFD}"));
    }

    #[test]
    fn an_exit_is_noticed_and_told_once() {
        let processes = Processes::default();
        let info = start(&processes, "exit 3");
        until("the exit", || !processes.list()[0].running());
        assert_eq!(processes.list()[0].state, ProcessState::Exited { code: Some(3) });
        assert_eq!(processes.take_ended().len(), 1);
        assert!(processes.take_ended().is_empty(), "once");
        assert_eq!(processes.read(info.id).unwrap().process.state, ProcessState::Exited { code: Some(3) });
    }

    /// The model stopped it and knows; the user stopped it and the model
    /// has to be told.
    #[test]
    fn a_stop_ends_the_whole_tree_and_only_the_users_is_reported() {
        let processes = Processes::default();
        let dir = temp_dir("bg-tree");
        let pid_file = dir.join("child.pid");
        let own = processes
            .start(&Shell::default(), &format!("sleep 300 & echo $! > {}; wait", pid_file.display()), &dir, ".")
            .unwrap();
        until("the child", || pid_file.exists() && !std::fs::read_to_string(&pid_file).unwrap().trim().is_empty());
        let pid: i32 = std::fs::read_to_string(&pid_file).unwrap().trim().parse().unwrap();

        assert_eq!(processes.stop(own.id).unwrap().state, ProcessState::Stopped);
        until("the grandchild to die", || unsafe { libc::kill(pid, 0) } != 0);
        assert!(processes.take_ended().is_empty(), "the model asked for it");
        assert_eq!(processes.stop(own.id).unwrap().state, ProcessState::Stopped, "stopping again says how it ended");

        let theirs = start(&processes, "sleep 30");
        processes.stop_by_user(theirs.id).unwrap();
        assert_eq!(processes.take_ended().iter().map(|p| p.id).collect::<Vec<_>>(), [theirs.id]);
    }

    #[test]
    fn only_so_many_run_at_once() {
        let processes = Processes::default();
        for _ in 0..MAX_RUNNING {
            start(&processes, "sleep 30");
        }
        let refused = processes.start(&Shell::default(), "sleep 30", &temp_dir("bg-full"), ".").unwrap_err();
        assert!(matches!(&refused, BackgroundError::TooMany(list) if list.contains("#1 `sleep 30` is running")), "{refused}");
        processes.stop(1).unwrap();
        start(&processes, "sleep 30");
    }

    /// The oldest finished one goes — never one still running, however old.
    #[test]
    fn finished_ones_are_forgotten_past_the_limit() {
        let processes = Processes::default();
        start(&processes, "sleep 30");
        for _ in 0..=MAX_FINISHED {
            let info = start(&processes, "true");
            processes.stop(info.id).unwrap();
        }
        start(&processes, "sleep 30");
        let ids: Vec<u32> = processes.list().iter().map(|p| p.id).collect();
        assert_eq!(ids.len(), MAX_FINISHED + 2);
        assert_eq!(&ids[..2], [1, 3], "the oldest finished one went, the running one stayed");
        assert_eq!(processes.read(2), Err(BackgroundError::NotFound(2)));
    }

    /// A read carries at most what a tool result may: a chatty server's
    /// backlog is cut in the middle, and the result says so.
    #[test]
    fn a_long_backlog_is_cut_for_the_model() {
        let processes = Processes::default();
        let info = start(&processes, "seq 1 20000; echo done; sleep 30");
        until("the backlog", || processes.tail(info.id, 10).unwrap().contains("done"));
        let read = processes.read(info.id).unwrap();
        assert!(read.truncated);
        assert!(read.output.chars().count() <= MAX_OUTPUT_CHARS + 200, "{}", read.output.len());
        assert!(read.output.starts_with("1\n") && read.output.trim_end().ends_with("done"), "{}", &read.output[read.output.len() - 40..]);
    }

    #[test]
    fn stopping_everything_is_not_reported() {
        let processes = Processes::default();
        start(&processes, "sleep 30");
        processes.stop_all();
        assert_eq!(processes.list()[0].state, ProcessState::Stopped);
        assert!(processes.take_ended().is_empty());
    }

    #[test]
    fn the_tab_reads_the_tail_without_moving_the_models_place() {
        let processes = Processes::default();
        let info = start(&processes, "echo ready; sleep 30");
        until("output", || processes.tail(info.id, 100).unwrap().contains("ready"));
        assert_eq!(processes.tail(info.id, 3).unwrap(), "dy\n");
        assert!(processes.read(info.id).unwrap().output.contains("ready"));
    }

    /// A `Processes` whose signals land in the returned list.
    fn watched() -> (Processes, Arc<Mutex<Vec<u32>>>) {
        let heard = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&heard);
        (Processes::new(Arc::new(move |event: ProcessChanged| sink.lock().unwrap().push(event.id))), heard)
    }

    #[test]
    fn output_after_the_exit_is_signalled_until_the_pipes_are_drained() {
        // An exited process whose stdout is still being read.
        let registry = Arc::new(Mutex::new(Registry::default()));
        lock(&registry).entries.push(Entry {
            info: ProcessInfo { id: 1, command: "x".into(), cwd: ".".into(), state: ProcessState::Exited { code: Some(0) } },
            child: None,
            buffer: OutputBuffer::default(),
            streams: 1,
            reported: false,
        });
        let heard = Arc::new(Mutex::new(0));
        let count = Arc::clone(&heard);
        watch(&registry, &(Arc::new(move |_| *count.lock().unwrap() += 1) as ProcessEventSink), 1);

        thread::sleep(POLL * 2);
        assert_eq!(*heard.lock().unwrap(), 0, "nothing new, nothing said");
        lock(&registry).entry(1).unwrap().buffer.push("late\n");
        until("the late output", || *heard.lock().unwrap() == 1);

        {
            let mut registry = lock(&registry);
            let entry = registry.entry(1).unwrap();
            entry.buffer.push("last\n");
            entry.streams = 0;
        }
        until("the last of it", || *heard.lock().unwrap() == 2);
        lock(&registry).entry(1).unwrap().buffer.push("after the watch\n");
        thread::sleep(POLL * 3);
        assert_eq!(*heard.lock().unwrap(), 2, "drained, the watch is over");
    }

    #[test]
    fn a_silent_exit_is_signalled_and_ends_the_watch() {
        let (processes, heard) = watched();
        let info = start(&processes, "sleep 0.2");
        until("the exit's signal", || heard.lock().unwrap().len() == 2);
        until("the exit", || !processes.list()[0].running());
        // Drained and exited, nobody watches it any more: new text is not news.
        thread::sleep(POLL * 3);
        lock(&processes.registry).entry(info.id).unwrap().buffer.push("x");
        thread::sleep(POLL * 3);
        assert_eq!(heard.lock().unwrap().len(), 2);
    }

    #[test]
    fn a_start_output_and_exit_are_each_signalled() {
        let (processes, heard) = watched();
        let first = start(&processes, "sleep 0.3; echo done");
        assert_eq!(*heard.lock().unwrap(), [first.id], "the start, at once");
        until("the exit", || !processes.list()[0].running());
        until("the output and the exit", || heard.lock().unwrap().len() >= 2);
        assert!(heard.lock().unwrap().iter().all(|&id| id == first.id));
    }

    #[test]
    fn a_chatty_process_is_signalled_per_tick_not_per_line() {
        let (processes, heard) = watched();
        start(&processes, "i=0; while [ $i -lt 20000 ]; do echo line $i; i=$((i+1)); done; sleep 30");
        until("all the output", || processes.tail(1, 64).map(|t| t.contains("19999")).unwrap_or(false));
        thread::sleep(POLL * 3);
        let signals = heard.lock().unwrap().len();
        // One for the start, then at most one a tick while it wrote.
        assert!(signals < 60, "{signals} signals for 20000 lines");
        thread::sleep(POLL * 3);
        assert_eq!(heard.lock().unwrap().len(), signals, "and none once it is quiet");
    }

    #[test]
    fn a_stop_is_signalled_once_and_a_second_stop_not_at_all() {
        let (processes, heard) = watched();
        let info = start(&processes, "sleep 30");
        processes.stop(info.id).unwrap();
        processes.stop(info.id).unwrap();
        thread::sleep(POLL * 3);
        assert_eq!(*heard.lock().unwrap(), [info.id, info.id], "the start and the one stop");
    }

    #[test]
    fn stopping_everything_signals_each_that_ran() {
        let (processes, heard) = watched();
        let a = start(&processes, "sleep 30");
        let b = start(&processes, "sleep 30");
        heard.lock().unwrap().clear();
        processes.stop_all();
        thread::sleep(POLL * 3);
        assert_eq!(*heard.lock().unwrap(), [a.id, b.id]);
    }
}
