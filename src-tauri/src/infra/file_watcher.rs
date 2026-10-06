//! Tells the index that the working tree changed.
//!
//! It does not say *what* changed. A sync already decides that for itself —
//! size and mtime first, so a repository where nothing changed costs one walk
//! (3 ms on this one) — and a whole-tree sync needs no path from here. That
//! also makes a lost or overflowed event harmless: the OS says "rescan" as an
//! error, and a rescan is all a change ever triggers anyway.
//!
//! Alfa Atlas debounced per path on the leading edge: the first event fired,
//! and events within the window after it were dropped. The last save of a
//! burst was then never indexed (`docs/07-upstream-findings.md`, B-12). Here
//! the call comes once the tree has been quiet for [`QUIET`] — or after
//! [`MAX_WAIT`], so a file rewritten without pause cannot hold the index back
//! for good. Changes made while `on_change` runs are queued and give one more
//! call after it.

use std::path::Path;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use ignore::gitignore::{Gitignore, GitignoreBuilder};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

/// How long the tree must stay still before `on_change` runs.
pub const QUIET: Duration = Duration::from_millis(300);
/// The longest a change waits, however busy the tree.
pub const MAX_WAIT: Duration = Duration::from_secs(3);

/// A running watch. Dropping it stops the watch and its thread.
pub struct FileWatcher {
    _watcher: RecommendedWatcher,
}

impl FileWatcher {
    /// Watches `root` recursively; `on_change` runs on the watcher's own
    /// thread, one call at a time.
    pub fn start(root: &Path, on_change: impl FnMut() + Send + 'static) -> notify::Result<Self> {
        Self::start_with(root, false, QUIET, MAX_WAIT, on_change)
    }

    /// The same for a working tree: a write to what its ignore rules leave out
    /// — `target/`, `node_modules/`, an editor's swap file — is not a change.
    /// The index never sees those files, and a build writing thousands of
    /// them would otherwise sync the index, and make the window re-read what
    /// listens to it, every [`MAX_WAIT`].
    pub fn start_tree(root: &Path, on_change: impl FnMut() + Send + 'static) -> notify::Result<Self> {
        Self::start_with(root, true, QUIET, MAX_WAIT, on_change)
    }

    fn start_with(
        root: &Path,
        follow_ignores: bool,
        quiet: Duration,
        max_wait: Duration,
        on_change: impl FnMut() + Send + 'static,
    ) -> notify::Result<Self> {
        // Events arrive with the path the OS resolved (`/var/...` on macOS
        // comes back as `/private/var/...`); the `.git` check is made on the
        // path below this prefix.
        let root = root.canonicalize().map_err(notify::Error::io)?;
        let (tx, rx) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(tx)?;
        watcher.watch(&root, RecursiveMode::Recursive)?;
        std::thread::Builder::new()
            .name("index-watcher".into())
            .spawn(move || {
                let ignored = follow_ignores.then(|| Ignored::read(&root));
                dispatch(&rx, &root, ignored, quiet, max_wait, on_change)
            })
            .map_err(notify::Error::io)?;
        Ok(Self { _watcher: watcher })
    }
}

/// Returns when the watcher is dropped (the sender goes with it).
fn dispatch(
    rx: &Receiver<notify::Result<Event>>,
    root: &Path,
    mut ignored: Option<Ignored>,
    quiet: Duration,
    max_wait: Duration,
    mut on_change: impl FnMut(),
) {
    loop {
        loop {
            match rx.recv() {
                Ok(message) if counts(&message, root, &mut ignored) => break,
                Ok(_) => {}
                Err(_) => return,
            }
        }
        let deadline = Instant::now() + max_wait;
        let mut fire_at = Instant::now() + quiet;
        while let Some(wait) = fire_at.min(deadline).checked_duration_since(Instant::now()) {
            match rx.recv_timeout(wait) {
                Ok(message) if counts(&message, root, &mut ignored) => fire_at = Instant::now() + quiet,
                // The loop condition sees that the wait is over.
                Ok(_) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
        on_change();
    }
}

/// Reads are not changes — a sync reads every file it indexes, and on Linux
/// that would wake the watcher again. Nor is anything under `.git`, which git
/// rewrites on a mere `git status`, or what the tree's ignore rules leave out.
/// An error is the OS saying it lost track, so it counts.
///
/// A `.gitignore` written is a change whatever it says — it changes what the
/// index holds — and the rules are read again from it.
fn counts(message: &notify::Result<Event>, root: &Path, ignored: &mut Option<Ignored>) -> bool {
    let Ok(event) = message else { return true };
    if matches!(event.kind, EventKind::Access(_)) {
        return false;
    }
    if event.paths.is_empty() {
        return true;
    }
    if let Some(ignored) = ignored.as_mut() {
        if event.paths.iter().any(|path| path.file_name().is_some_and(|name| name == ".gitignore")) {
            *ignored = Ignored::read(root);
            return true;
        }
    }
    event
        .paths
        .iter()
        .any(|path| !under_git(path, root) && !ignored.as_ref().is_some_and(|ignored| ignored.covers(root, path)))
}

/// What the scan of the index leaves out by the root's `.gitignore`,
/// `.git/info/exclude` and the user's global ignore file — the root's rules
/// first, so a `!` there wins over the global file.
///
/// ponytail: the root's files only. What only a nested `.gitignore` leaves
/// out still wakes a sync that finds nothing, as before; a `!` in a nested
/// one re-including what the root's leave out is not seen, and a change to
/// that file waits for the next change that counts. Reading every nested
/// file means a matcher per directory, kept current as they change.
struct Ignored {
    tree: Gitignore,
    global: Gitignore,
}

impl Ignored {
    fn read(root: &Path) -> Self {
        let mut builder = GitignoreBuilder::new(root);
        // A file that is not there adds nothing, which is what it should.
        let _ = builder.add(root.join(".gitignore"));
        let _ = builder.add(root.join(".git").join("info").join("exclude"));
        Self { tree: builder.build().unwrap_or_else(|_| Gitignore::empty()), global: Gitignore::global().0 }
    }

    fn covers(&self, root: &Path, path: &Path) -> bool {
        // Relative, as both matchers take it: one outside their root panics.
        let Ok(relative) = path.strip_prefix(root) else { return false };
        let is_dir = path.is_dir();
        match self.tree.matched_path_or_any_parents(relative, is_dir) {
            ignore::Match::Ignore(_) => true,
            ignore::Match::Whitelist(_) => false,
            ignore::Match::None => self.global.matched_path_or_any_parents(relative, is_dir).is_ignore(),
        }
    }
}

fn under_git(path: &Path, root: &Path) -> bool {
    path.strip_prefix(root).unwrap_or(path).components().any(|part| part.as_os_str() == ".git")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::temp_dir;
    use notify::event::{AccessKind, ModifyKind};
    use std::path::PathBuf;
    use std::sync::mpsc::Sender;

    /// The quiet period the tests run with. Their sleeps are fractions of it
    /// and must not stretch past it: at 80 ms a macOS CI runner, busy with the
    /// rest of the suite, slept a 20 ms gap through it and fired mid-burst.
    const QUICK: Duration = Duration::from_millis(400);

    fn modified(path: &str) -> notify::Result<Event> {
        Ok(Event::new(EventKind::Modify(ModifyKind::Any)).add_path(PathBuf::from("/repo").join(path)))
    }

    /// Runs `dispatch` on a thread; every `on_change` call arrives on the
    /// returned receiver.
    fn dispatcher(max_wait: Duration) -> (Sender<notify::Result<Event>>, Receiver<()>, std::thread::JoinHandle<()>) {
        let (events, rx) = mpsc::channel();
        let (calls_tx, calls) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            dispatch(&rx, Path::new("/repo"), None, QUICK, max_wait, move || calls_tx.send(()).unwrap());
        });
        (events, calls, thread)
    }

    fn no_call(calls: &Receiver<()>) -> bool {
        calls.recv_timeout(QUICK * 4).is_err()
    }

    #[test]
    fn a_burst_of_saves_gives_one_call_after_the_last() {
        let (events, calls, _) = dispatcher(Duration::from_secs(10));
        for _ in 0..4 {
            events.send(modified("a.rs")).unwrap();
            std::thread::sleep(QUICK / 4);
        }
        events.send(modified("a.rs")).unwrap();
        let last = Instant::now();

        calls.recv_timeout(Duration::from_secs(5)).unwrap();

        assert!(last.elapsed() >= QUICK, "fired {:?} after the last save", last.elapsed());
        assert!(no_call(&calls), "one burst, one call");
    }

    #[test]
    fn reads_and_git_internals_are_not_changes() {
        let (events, calls, _) = dispatcher(Duration::from_secs(10));
        events
            .send(Ok(Event::new(EventKind::Access(AccessKind::Any)).add_path("/repo/a.rs".into())))
            .unwrap();
        events.send(modified(".git/index")).unwrap();
        events.send(modified("sub/.git/HEAD")).unwrap();
        assert!(no_call(&calls));

        events.send(modified(".github/workflows/ci.yml")).unwrap();
        calls.recv_timeout(Duration::from_secs(5)).unwrap();
    }

    /// Reads and `.git` writes do not push the call back either — an editor
    /// whose git integration polls `git status` must not starve a real save.
    #[test]
    fn what_does_not_count_does_not_delay_the_call() {
        let (events, calls, _) = dispatcher(Duration::from_secs(10));
        events.send(modified("a.rs")).unwrap();
        let sent = Instant::now();
        while sent.elapsed() < QUICK * 3 {
            events.send(modified(".git/index")).unwrap();
            std::thread::sleep(QUICK / 8);
        }
        assert!(calls.try_recv().is_ok(), "the save waited out the .git writes");
    }

    #[test]
    fn a_tree_that_never_goes_quiet_still_gets_a_call() {
        let (events, calls, _) = dispatcher(QUICK * 3);
        let started = Instant::now();
        while started.elapsed() < QUICK * 10 {
            events.send(modified("log.txt")).unwrap();
            std::thread::sleep(QUICK / 4);
            if calls.try_recv().is_ok() {
                return;
            }
        }
        panic!("no call in {:?} of steady changes", started.elapsed());
    }

    #[test]
    fn a_lost_track_error_counts_as_a_change() {
        let (events, calls, _) = dispatcher(Duration::from_secs(10));
        events.send(Err(notify::Error::generic("rescan"))).unwrap();
        calls.recv_timeout(Duration::from_secs(5)).unwrap();
    }

    /// An event that names no path (FSEvents' "rescan" arrives like this) may
    /// have been anything.
    #[test]
    fn an_event_without_paths_counts_as_a_change() {
        let (events, calls, _) = dispatcher(Duration::from_secs(10));
        events.send(Ok(Event::new(EventKind::Other))).unwrap();
        calls.recv_timeout(Duration::from_secs(5)).unwrap();
    }

    #[test]
    fn a_change_during_the_call_gives_one_more_call() {
        let (events, rx) = mpsc::channel();
        let (inside_tx, inside) = mpsc::channel();
        let (resume_tx, resume) = mpsc::channel::<()>();
        std::thread::spawn(move || {
            dispatch(&rx, Path::new("/repo"), None, QUICK, Duration::from_secs(10), move || {
                inside_tx.send(()).unwrap();
                let _ = resume.recv();
            });
        });
        events.send(modified("a.rs")).unwrap();
        inside.recv_timeout(Duration::from_secs(5)).unwrap();

        events.send(modified("b.rs")).unwrap();
        resume_tx.send(()).unwrap();

        inside.recv_timeout(Duration::from_secs(5)).expect("the save made mid-call was dropped");
        resume_tx.send(()).unwrap();
    }

    #[test]
    fn dropping_the_sender_ends_the_thread() {
        let (events, calls, thread) = dispatcher(Duration::from_secs(10));
        events.send(modified("a.rs")).unwrap();
        drop(events);
        thread.join().unwrap();
        assert!(calls.try_recv().is_err(), "a closed watch still called on_change");
    }

    // ------------------------------------------------------- ignore rules

    /// A tree with this `.gitignore`, and a dispatcher that follows it.
    fn tree(label: &str, gitignore: &str) -> (PathBuf, Sender<notify::Result<Event>>, Receiver<()>) {
        let root = temp_dir(label).canonicalize().unwrap();
        std::fs::write(root.join(".gitignore"), gitignore).unwrap();
        let (events, rx) = mpsc::channel();
        let (calls_tx, calls) = mpsc::channel();
        // Read here, not on the thread: a test that rewrites the file next
        // must not race the first reading.
        let ignored = Some(Ignored::read(&root));
        let at = root.clone();
        std::thread::spawn(move || {
            dispatch(&rx, &at, ignored, QUICK, Duration::from_secs(10), move || calls_tx.send(()).unwrap());
        });
        (root, events, calls)
    }

    fn written(root: &Path, path: &str) -> notify::Result<Event> {
        Ok(Event::new(EventKind::Modify(ModifyKind::Any)).add_path(root.join(path)))
    }

    #[test]
    fn what_the_tree_ignores_is_not_a_change() {
        let (root, events, calls) = tree("watcher-ignored", "target/\n*.tmp\n!keep.tmp\n");
        events.send(written(&root, "target/debug/build/x.o")).unwrap();
        events.send(written(&root, "src/scratch.tmp")).unwrap();
        assert!(no_call(&calls));

        events.send(written(&root, "keep.tmp")).unwrap();
        calls.recv_timeout(Duration::from_secs(5)).expect("a `!` in the root's file was not followed");
        events.send(written(&root, "src/a.rs")).unwrap();
        calls.recv_timeout(Duration::from_secs(5)).expect("a source file was taken for ignored");
    }

    /// The rules are read again when the file changes: a folder no longer
    /// ignored is heard from then on, and the edit itself is a change.
    #[test]
    fn a_rewritten_gitignore_counts_and_its_new_rules_apply() {
        let (root, events, calls) = tree("watcher-gitignore-edit", "target/\n");
        std::fs::write(root.join(".gitignore"), "").unwrap();
        events.send(written(&root, ".gitignore")).unwrap();
        calls.recv_timeout(Duration::from_secs(5)).expect("the edit to .gitignore was not a change");

        events.send(written(&root, "target/a.rs")).unwrap();
        calls.recv_timeout(Duration::from_secs(5)).expect("the old rules still applied");
    }

    /// A path the OS names outside the root is not matched — the matcher
    /// panics on one — and counts, as it did before there were rules.
    #[test]
    fn a_path_outside_the_root_counts() {
        let (_, events, calls) = tree("watcher-outside", "*\n");
        events.send(Ok(Event::new(EventKind::Modify(ModifyKind::Any)).add_path("/elsewhere/a.rs".into()))).unwrap();
        calls.recv_timeout(Duration::from_secs(5)).unwrap();
    }

    // ------------------------------------------------ the real filesystem

    /// The `.git` rule looks below the root only. With the root left as given,
    /// macOS reports `/private/var/...` for `/var/...`, the prefix does not
    /// strip, and a root that sits under any folder named `.git` hears nothing.
    #[test]
    fn a_root_below_a_folder_named_git_still_hears_its_files() {
        let root = temp_dir("watcher-under-git").join(".git").join("repo");
        std::fs::create_dir_all(&root).unwrap();
        let (calls_tx, calls) = mpsc::channel();
        let _watcher = FileWatcher::start_with(&root, false, QUICK, Duration::from_secs(10), move || {
            let _ = calls_tx.send(());
        })
        .unwrap();

        std::fs::write(root.join("a.rs"), "fn a() {}").unwrap();

        calls.recv_timeout(Duration::from_secs(10)).expect("the write was taken for a .git write");
    }

    /// On disk, through the OS's own paths: a build writing into an ignored
    /// folder is not heard, and a save next to it is.
    #[test]
    fn a_write_into_an_ignored_folder_on_disk_is_not_heard() {
        let root = temp_dir("watcher-disk-ignored");
        std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        let (calls_tx, calls) = mpsc::channel();
        let _watcher = FileWatcher::start_with(&root, true, QUICK, Duration::from_secs(10), move || {
            let _ = calls_tx.send(());
        })
        .unwrap();
        // FSEvents may report what happened just before the watch began.
        std::thread::sleep(QUICK * 4);
        while calls.try_recv().is_ok() {}

        std::fs::write(root.join("target").join("out.o"), "x").unwrap();
        assert!(calls.recv_timeout(QUICK * 8).is_err(), "a write into target/ woke the index");

        std::fs::write(root.join("a.rs"), "fn a() {}").unwrap();
        calls.recv_timeout(Duration::from_secs(10)).expect("the save was not seen");
    }

    #[test]
    fn a_write_on_disk_reaches_on_change_and_a_drop_stops_it() {
        let root = temp_dir("watcher-disk");
        let (calls_tx, calls) = mpsc::channel();
        let watcher = FileWatcher::start_with(&root, false, QUICK, Duration::from_secs(10), move || {
            let _ = calls_tx.send(());
        })
        .unwrap();

        std::fs::write(root.join("a.rs"), "fn a() {}").unwrap();
        calls.recv_timeout(Duration::from_secs(10)).expect("the write was not seen");

        drop(watcher);
        std::fs::write(root.join("b.rs"), "fn b() {}").unwrap();
        // The thread ends with the watcher, and the sender inside `on_change`
        // with it.
        assert!(matches!(calls.recv_timeout(Duration::from_secs(5)), Err(RecvTimeoutError::Disconnected)));
    }
}
