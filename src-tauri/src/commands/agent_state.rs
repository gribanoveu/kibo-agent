//! What the window's turns are reached through: each chat's `SessionHandle`,
//! the open folder, the approval policy and the mode — and who may start a turn.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};


use crate::domain::conversation_mode::{ConversationMode, PickedMode};
use crate::domain::tools::ApprovalPolicy;
use crate::domain::next_prompt::{FinishedTurn, TurnFeatures};
use crate::domain::turn::{ChatEventPayload, ChatStreamOutcome};
use crate::services::llm_chat::SteeringQueue;

/// What a running turn of one chat is reached through while it runs: its stop
/// flag, the notes typed at it, and its side of the next-prompt journal. One
/// per chat, so a chat's turn never shares any of this with another chat's.
#[derive(Default)]
pub(crate) struct SessionHandle {
    /// One in-flight turn per chat, so one flag needs no turn id to
    /// disambiguate. Reset by `chat_start` and deliberately *not* by
    /// `chat_resume` — a stop pressed while an approval card was showing has
    /// to survive until the resumed turn reads it.
    pub(super) cancel: AtomicBool,
    pub(super) steering: SteeringQueue,
    /// The running turn's signals for the next-prompt journal. Resident
    /// because a turn outlives one command: what came before a pause for
    /// approval still counts after the resume.
    pub(super) next_prompt: Mutex<TurnFeatures>,
    /// The last turn that ended, by id, until the window journals it. Apart
    /// from the running one, so a queued message that starts the next turn
    /// before that call arrives does not wipe it.
    pub(super) finished_turn: Mutex<Option<(String, FinishedTurn)>>,
    /// Its own checkout while the turn runs: the mark and the root cannot
    /// disagree because one lock holds both. Only turns in the same checkout
    /// share files and a git index.
    pub(super) running_root: Mutex<Option<PathBuf>>,
    /// A paused turn still owns the chat's history until its card is resumed.
    pub(super) paused: AtomicBool,
}

/// Counted while a chat's turn runs; clears itself on drop, however the turn
/// ended — finished, paused, stopped, refused or lost to a panicked thread.
pub(crate) struct RunningTurn(Arc<SessionHandle>);

impl std::fmt::Debug for RunningTurn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RunningTurn")
    }
}

impl Drop for RunningTurn {
    fn drop(&mut self) {
        *self.0.running_root.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }
}

impl SessionHandle {
    pub(super) fn is_running(&self) -> bool {
        self.running_root.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).is_some()
    }

    // The next-prompt journal's side of a turn. A poisoned lock costs the
    // journal one row, never the turn.

    /// A fresh turn, in the mode it starts in. A resume is not one.
    pub(super) fn begin_turn(&self, mode: ConversationMode) {
        if let Ok(mut features) = self.next_prompt.lock() {
            *features = TurnFeatures::new(mode);
        }
    }

    pub(super) fn observe(&self, event: &ChatEventPayload) {
        if let Ok(mut features) = self.next_prompt.lock() {
            features.observe(event);
        }
    }

    pub(super) fn end_turn(&self, turn_id: String, outcome: &ChatStreamOutcome) {
        let finished = self.next_prompt.lock().ok().and_then(|features| features.finish(outcome));
        if let (Some(finished), Ok(mut slot)) = (finished, self.finished_turn.lock()) {
            *slot = Some((turn_id, finished));
        }
    }

    /// The finished turn if it is `turn_id`, once; another turn's stays put.
    pub(super) fn take_finished(&self, turn_id: &str) -> Option<FinishedTurn> {
        let mut slot = self.finished_turn.lock().ok()?;
        if slot.as_ref().is_some_and(|(id, _)| id == turn_id) {
            slot.take().map(|(_, finished)| finished)
        } else {
            None
        }
    }
}

#[derive(Default)]
pub struct AgentState {
    /// Each chat's handle, made on first ask. The workspace, the approval
    /// policy and the mode stay shared: the folder is the window's, and the
    /// policy and the chip are decisions about the conversation in it, not
    /// about one chat's turn.
    /// ponytail: handles are never pruned — the map grows by one entry per
    /// chat touched in a window's run. An LRU is the way past that if a long
    /// run ever shows it.
    pub(super) sessions: Mutex<HashMap<String, Arc<SessionHandle>>>,
    /// The folders open in the window (F-8): canonical paths, the last added
    /// first. `workspace` is always one of them — registering a folder makes
    /// it the active one, closing it moves the active mark.
    pub(super) open: Mutex<Vec<PathBuf>>,
    pub(super) workspace: Mutex<Option<PathBuf>>,
    /// Held for the whole of adding or closing a folder: the registry, the
    /// active mark and the folder's index change together. Without it React's
    /// StrictMode double launch opened one folder's index twice, two first
    /// syncs writing one store, and a close racing an add left an index open
    /// for a folder no longer in the registry, or the reverse. Async: held
    /// across the index's open, which reads the store.
    pub(super) folder_change: tokio::sync::Mutex<()>,
    /// Grows as the user answers "always allow"; not persisted, because a
    /// saved "never ask me" is a brake released a month ago and forgotten.
    pub(super) approval: Mutex<ApprovalPolicy>,
    /// What the composer's panel says right now. Resident rather than sent
    /// with each turn for the same reason as the approval policy: a paused
    /// turn is resumed from a checkpoint that predates the choice, and a mode
    /// threaded through both entry points would have to be re-sent correctly
    /// by the caller at the one moment it is easiest to forget. Kept in the
    /// settings as well: this is the copy turns read, that is the one that
    /// outlives the app.
    pub(super) mode: Mutex<PickedMode>,
}

impl AgentState {
    /// The state a window starts with: the mode picked last, read here so a
    /// turn does not depend on the window having asked. Settings that cannot
    /// be read leave the default, which still asks before it writes.
    pub fn restored() -> Self {
        let mode = crate::infra::settings_store::load().map(|s| s.conversation_mode).unwrap_or_default();
        Self { mode: Mutex::new(mode), ..Self::default() }
    }

    /// The handle `chat_id`'s turns are reached through, made on first ask.
    pub(crate) fn session(&self, chat_id: &str) -> Arc<SessionHandle> {
        let mut sessions = self.sessions.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        Arc::clone(sessions.entry(chat_id.to_string()).or_default())
    }

    /// Marks `chat_id`'s turn as running and hands back the guard that clears
    /// it. Refused when that chat's turn already runs or waits on its card —
    /// one chat, one owned history — and when `limit` turns run already.
    pub(crate) fn begin_running(&self, chat_id: &str, limit: u32, workspace: &std::path::Path) -> Result<RunningTurn, String> {
        self.begin_running_inner(chat_id, limit, workspace, false)
    }

    pub(super) fn begin_resume(&self, chat_id: &str, limit: u32, workspace: &std::path::Path) -> Result<RunningTurn, String> {
        self.begin_running_inner(chat_id, limit, workspace, true)
    }

    pub(super) fn begin_running_inner(&self, chat_id: &str, limit: u32, workspace: &std::path::Path, resume: bool) -> Result<RunningTurn, String> {
        // A hand-edited zero would refuse every turn; one always runs.
        let limit = limit.max(1);
        let _tree = crate::domain::command_risk::git_tree_lock();
        let mut sessions = self.sessions.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let handle = Arc::clone(sessions.entry(chat_id.to_string()).or_default());
        if handle.is_running() {
            return Err("this chat's turn is already running".to_string());
        }
        if !resume && handle.paused.load(Ordering::SeqCst) {
            return Err("this chat's turn is waiting for approval — resume or stop it first".to_string());
        }
        let running = sessions.values().filter(|s| s.is_running()).count() as u32;
        if running >= limit {
            return Err(format!(
                "{running} turns are already running — the parallel-turns limit. \
                 Wait for one to finish, or raise it in Settings"
            ));
        }
        *handle.running_root.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(workspace.to_path_buf());
        handle.paused.store(false, Ordering::SeqCst);
        Ok(RunningTurn(handle))
    }

    /// Titles of the other chats running in this checkout, live for the prompt's
    /// standing note and the command guard. A linked worktree is another checkout.
    pub(super) fn parallel_chats(&self, chat_id: &str, workspace: &std::path::Path) -> Vec<String> {
        let sessions = self.sessions.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let running: Vec<String> = sessions
            .iter()
            .filter(|(id, session)| {
                id.as_str() != chat_id
                    && session.is_running()
                    && session.running_root.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).as_deref() == Some(workspace)
            })
            .map(|(id, _)| id.clone())
            .collect();
        drop(sessions);
        running.into_iter().map(|id| crate::infra::chat_store::title(&id).ok().flatten().unwrap_or(id)).collect()
    }

    pub(crate) fn ensure_chat_idle(&self, chat_id: &str) -> Result<(), String> {
        if self.session(chat_id).is_running() {
            Err("this chat's turn is running — wait for it to finish before moving or deleting it".into())
        } else {
            Ok(())
        }
    }

    /// Automatic cleanup never removes a checkout another turn is still using.
    pub(crate) fn root_is_idle(&self, workspace: &std::path::Path) -> bool {
        let sessions = self.sessions.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        !sessions.values().any(|session| {
            session.is_running()
                && session.running_root.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).as_deref() == Some(workspace)
        })
    }

    /// The folder's tree may change only when no turn runs in it (F-8): only
    /// turns in the same checkout share files and a git index — a turn in
    /// another folder does not hold this one.
    pub(crate) fn ensure_folder_idle(&self, root: &std::path::Path) -> Result<(), String> {
        if self.root_is_idle(root) {
            Ok(())
        } else {
            Err("an agent turn is running in this folder — wait for it to finish, or start a worktree for the new chat".to_string())
        }
    }

    pub(crate) fn workspace(&self) -> Result<PathBuf, String> {
        self.workspace
            .lock()
            .map_err(|_| "workspace lock poisoned".to_string())?
            .clone()
            .ok_or_else(|| "no folder is open".to_string())
    }

    /// The open folders (F-8), the last added first.
    pub(crate) fn open_folders(&self) -> Vec<PathBuf> {
        self.open.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    pub(crate) fn is_open(&self, root: &std::path::Path) -> bool {
        self.open.lock().unwrap_or_else(PoisonError::into_inner).iter().any(|open| open == root)
    }

    /// Makes `root` an open folder: first in recency, exactly once. Does not
    /// touch the active mark — that is `activate` in `commands::workspace`.
    pub(crate) fn register_open(&self, root: PathBuf) {
        let mut open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        open.retain(|known| *known != root);
        open.insert(0, root);
    }

    /// Drops `root` from the open set unless a turn runs in it — checked and
    /// dropped under the registry's lock. A turn starting meanwhile marks
    /// itself running first and asks `is_open` after (`chat_start`), so one
    /// of the two sees the other: either this refuses, or the turn does —
    /// never a turn running in a folder already closed, its processes
    /// unstoppable. `false` when a turn holds it.
    pub(crate) fn unregister_if_idle(&self, root: &std::path::Path) -> bool {
        let mut open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        if !self.root_is_idle(root) {
            return false;
        }
        open.retain(|known| known != root);
        true
    }

    /// A turn just admitted in `root` may run only while `root` is open:
    /// checked after `begin_running`, see `unregister_if_idle`.
    pub(crate) fn still_open(&self, root: &std::path::Path) -> Result<(), String> {
        match self.is_open(root) {
            true => Ok(()),
            false => Err(format!("{} is not an open folder — open it to work in this chat", root.display())),
        }
    }

    /// The one door a folder path from the window goes through before the
    /// backend acts on it (F-8, invariant 1): canonicalized — `..` and
    /// symlinks resolve to what they really point at — and accepted only as
    /// one of the open folders.
    pub(crate) fn require_open(&self, path: &str) -> Result<PathBuf, String> {
        let asked = crate::infra::paths::canonicalize_plain(std::path::Path::new(path))
            .map_err(|e| format!("cannot use {path}: {e}"))?;
        if self.is_open(&asked) {
            Ok(asked)
        } else {
            Err(format!("{path} is not an open folder"))
        }
    }

    /// The folder a window-scoped command acts on (F-8): `Some(path)` is
    /// canonicalized and accepted only as an open folder (invariant 1);
    /// `None` is Chat mode's "no folder".
    pub(crate) fn ui_workspace(&self, workspace: &Option<String>) -> Result<Option<PathBuf>, String> {
        workspace.as_ref().map(|path| self.require_open(path)).transpose()
    }

    #[cfg(test)]
    pub(crate) fn set_workspace_for_test(&self, workspace: PathBuf) {
        // The active folder is always one of the open ones (F-8).
        self.register_open(workspace.clone());
        *self.workspace.lock().unwrap() = Some(workspace);
    }

    /// The checkout this chat belongs to, not whichever folder is open now.
    /// An unsaved chat has no record yet and starts in the open folder.
    pub(crate) fn chat_workspace(&self, chat_id: &str) -> Result<(PathBuf, Option<String>), String> {
        let record = match crate::infra::chat_store::load(chat_id) {
            Ok(record) => Some(record),
            Err(crate::domain::chat_record::ChatError::NotFound(_)) => None,
            Err(e) => return Err(e.to_string()),
        };
        let Some(record) = record else { return Ok((self.workspace()?, None)) };
        let root = crate::infra::paths::canonicalize_plain(std::path::Path::new(&record.workspace))
            .map_err(|e| format!("cannot open the chat's folder {}: {e}", record.workspace))?;
        if !root.is_dir() {
            return Err(format!("the chat's folder {} is not a folder", record.workspace));
        }
        Ok((root, record.worktree_base))
    }

    /// Prompt measuring also works before any folder is open, as it did for
    /// an unsaved chat; a saved chat still reads only its own checkout.
    pub(super) fn chat_prompt_workspace(&self, chat_id: &str) -> Result<Option<PathBuf>, String> {
        match crate::infra::chat_store::load(chat_id) {
            Ok(_) => self.chat_workspace(chat_id).map(|(root, _)| Some(root)),
            Err(crate::domain::chat_record::ChatError::NotFound(_)) => Ok(self.workspace().ok()),
            Err(e) => Err(e.to_string()),
        }
    }

    pub(super) fn picked(&self) -> PickedMode {
        // A poisoned lock loses the choice, not the turn.
        self.mode.lock().map_or(PickedMode::default(), |mode| *mode)
    }

    pub(super) fn mode(&self) -> ConversationMode {
        self.picked().into()
    }

    pub(super) fn approval(&self) -> Result<ApprovalPolicy, String> {
        Ok(self
            .approval
            .lock()
            .map_err(|_| "approval lock poisoned".to_string())?
            .clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::turn::SteeringNote;
    
    use crate::testing::temp_dir;
    use std::path::Path;

    /// The state is reachable without a running app, which is what keeps these
    /// checks about behaviour rather than about Tauri.
    fn state() -> Arc<AgentState> {
        Arc::new(AgentState::default())
    }

    fn next_prompt_turn(session: &SessionHandle, id: &str, events: &[ChatEventPayload]) {
        events.iter().for_each(|event| session.observe(event));
        let done = crate::domain::turn::ChatDone { result: Default::default(), todos: Vec::new(), history: Vec::new(), limit_reached: None };
        session.end_turn(id.to_string(), &ChatStreamOutcome::Done(done));
    }

    fn said(text: &str) -> ChatEventPayload {
        ChatEventPayload::RoundCompleted { text: text.into(), reasoning: String::new(), truncated: false }
    }

    /// A late call for an older turn must not take the newer one's place in
    /// the journal, and a turn is journaled once.
    #[test]
    fn the_next_prompt_journal_takes_a_finished_turn_once_and_only_by_its_id() {
        let state = state();
        let session = state.session("c1");
        session.begin_turn(state.mode());
        next_prompt_turn(&session, "turn-2", &[said("ok")]);

        assert!(session.take_finished("turn-1").is_none());
        assert!(session.take_finished("turn-2").is_some(), "another turn's is left where it was");
        assert!(session.take_finished("turn-2").is_none());
    }

    /// A new turn starts from nothing, in the mode it starts in — and does
    /// not wipe the last one before the window has journaled it.
    #[test]
    fn a_new_turn_starts_the_next_prompt_signals_afresh() {
        let state = state();
        let session = state.session("c1");
        session.begin_turn(state.mode());
        next_prompt_turn(&session, "turn-1", &[said("first")]);
        *state.mode.lock().unwrap() = PickedMode::Analyst;
        session.begin_turn(state.mode());
        next_prompt_turn(&session, "turn-2", &[said("second")]);

        let input = session.take_finished("turn-2").expect("journaled").model_input("u", "");
        assert!(input.starts_with("<mode>ask"), "{input}");
        assert!(input.ends_with("<agent>second"), "{input}");
    }

    /// Each chat is reached through its own handle: one chat's stop says
    /// nothing to another's turn, a note waits in the queue it was typed at,
    /// and each chat journals its own finished turn.
    #[test]
    fn one_chats_turn_is_reached_without_touching_anothers() {
        let state = state();
        let one = state.session("c1");
        let other = state.session("c2");

        one.cancel.store(true, Ordering::SeqCst);
        assert!(one.cancel.load(Ordering::SeqCst));
        assert!(!other.cancel.load(Ordering::SeqCst), "the other chat's turn runs on");

        one.steering.push(SteeringNote::user("use the helper"));
        assert!(other.steering.take().is_empty(), "not the other chat's note");
        assert_eq!(one.steering.take().len(), 1);

        other.begin_turn(state.mode());
        next_prompt_turn(&other, "turn-1", &[said("done")]);
        assert!(one.take_finished("turn-1").is_none(), "not the one chat's turn to journal");
        assert!(other.take_finished("turn-1").is_some());
    }

    /// One chat, one turn; the limit counts across chats; a guard's drop
    /// frees its slot.
    #[test]
    fn a_chat_runs_one_turn_and_the_limit_counts_across_chats() {
        let state = state();
        let root = Path::new("/repo");
        let one = state.begin_running("c1", 3, root).expect("the first turn runs");
        let refused = state.begin_running("c1", 3, root).expect_err("the same chat refuses");
        assert!(refused.contains("already running"), "{refused}");

        let two = state.begin_running("c2", 2, root).expect("another chat's turn runs beside it");
        let over = state.begin_running("c3", 2, root).expect_err("the limit binds across chats");
        assert!(over.contains("limit"), "{over}");

        drop(one);
        state.begin_running("c3", 2, root).expect("a finished turn frees its slot");
        drop(two);

        // A hand-edited zero in the settings still lets one turn run.
        state.begin_running("c4", 0, root).expect("a zero limit still runs one");
    }

    #[test]
    fn turn_admission_waits_for_a_tree_mutation() {
        let state = state();
        let tree = crate::domain::command_risk::git_tree_lock();
        let (admitted, wait) = std::sync::mpsc::channel();
        std::thread::scope(|threads| {
            threads.spawn(|| {
                let guard = state.begin_running("c1", 3, Path::new("/repo")).expect("turn runs after the mutation");
                admitted.send(()).unwrap();
                drop(guard);
            });
            assert!(wait.recv_timeout(std::time::Duration::from_millis(50)).is_err(), "admission crossed the tree lock");
            drop(tree);
            wait.recv_timeout(std::time::Duration::from_secs(1)).expect("admission resumes with the lock");
        });
    }

    #[test]
    fn parallel_chats_are_scoped_to_their_checkout() {
        let state = state();
        let root = Path::new("/repo");
        let linked = Path::new("/repo-worktree");
        let one = state.begin_running("c1", 3, root).expect("the first turn runs");
        let two = state.begin_running("c2", 3, root).expect("the second turn runs");
        let three = state.begin_running("c3", 3, linked).expect("a worktree turn runs");
        assert_eq!(state.parallel_chats("c1", root), vec!["c2"]);
        assert_eq!(state.parallel_chats("c3", linked), Vec::<String>::new());
        drop(two);
        assert_eq!(state.parallel_chats("c1", root), Vec::<String>::new());
        drop(one);
        drop(three);
    }

    #[test]
    fn a_folder_waits_only_for_its_own_turns() {
        let state = state();
        let here = Path::new("/repo");
        let there = Path::new("/other");
        state.ensure_folder_idle(here).expect("an idle folder can be switched");
        let running = state.begin_running("c1", 2, here).expect("the turn runs");
        let refused = state.ensure_folder_idle(here).expect_err("a running turn holds its folder");
        assert!(refused.contains("worktree"), "{refused}");
        state.ensure_folder_idle(there).expect("another folder is not held");
        drop(running);
        state.ensure_folder_idle(here).expect("the finished turn releases the folder");
    }

    /// The turn's thread dying with the guard frees the mark all the same —
    /// the Drop, not the happy path, is what the next start relies on.
    #[test]
    fn a_panicked_turn_frees_its_slot() {
        let state = state();
        let root = Path::new("/repo");
        let guard = state.begin_running("c1", 3, root).expect("the turn runs");
        std::thread::spawn(move || {
            let _guard = guard;
            panic!("the turn's thread died");
        })
        .join()
        .expect_err("the thread panicked");
        state.begin_running("c1", 3, root).expect("a panicked turn frees its slot");
    }

    #[test]
    fn a_turn_needs_a_folder_before_it_can_start() {
        let err = state().workspace().expect_err("nothing is open");
        assert!(err.contains("no folder"), "{err}");
    }

    #[test]
    fn opening_a_folder_canonicalizes_it_once() {
        let state = state();
        let root = temp_dir("cmd-workspace");
        std::fs::create_dir(root.join("inner")).unwrap();

        let spelled = format!("{}/inner/..", root.display());
        *state.workspace.lock().unwrap() = Some(
            PathBuf::from(&spelled)
                .canonicalize()
                .expect("canonicalizes"),
        );

        let opened = state.workspace().unwrap();
        assert!(!opened.to_string_lossy().contains(".."), "{opened:?}");
        assert!(opened.is_dir());
    }

    #[test]
    fn a_note_can_be_withdrawn_until_a_round_takes_it() {
        let state = state();
        let session = state.session("c1");
        let note = SteeringNote::user("actually, use the helper");
        let id = note.id.clone();
        session.steering.push(note);

        assert!(session.steering.cancel(&id));
        assert!(session.steering.take().is_empty());
        assert!(!session.steering.cancel(&id), "already gone");
    }

    /// A stop is remembered until a fresh turn clears it — that is what makes
    /// "stop while the approval card is showing" work, since the resumed turn
    /// reads the flag rather than being told again.
    #[test]
    fn a_stop_survives_until_the_next_fresh_turn() {
        let state = state();
        let session = state.session("c1");
        session.cancel.store(true, Ordering::SeqCst);
        assert!(session.cancel.load(Ordering::SeqCst));

        session.cancel.store(false, Ordering::SeqCst);
        assert!(!session.cancel.load(Ordering::SeqCst));
    }

    /// The registry holds each folder once, the last added first; closing
    /// removes only the named folder.
    #[test]
    fn open_folders_register_once_most_recent_first() {
        let state = state();
        let a = PathBuf::from("/a");
        let b = PathBuf::from("/b");
        state.register_open(a.clone());
        state.register_open(b.clone());
        state.register_open(a.clone());
        assert_eq!(state.open_folders(), vec![a.clone(), b.clone()]);

        assert!(state.unregister_if_idle(&b));
        assert!(state.unregister_if_idle(&b), "a folder that is not open is not an error");
        assert_eq!(state.open_folders(), vec![a]);
    }

    /// Invariant 1: a path from the window is canonicalized — over-qualified
    /// spellings, `..` and symlinks resolve to what they really point at —
    /// and accepted only as one of the open folders.
    #[test]
    fn require_open_resolves_spellings_and_rejects_what_is_not_open() {
        let state = state();
        let root = crate::infra::paths::canonicalize_plain(&temp_dir("require-open")).unwrap();
        state.register_open(root.clone());
        let shown = root.display().to_string();

        // Over-qualified spellings of the same folder: the same folder.
        assert_eq!(state.require_open(&format!("{shown}/.")).unwrap(), root);
        std::fs::create_dir(root.join("inner")).unwrap();
        assert_eq!(state.require_open(&format!("{shown}/inner/..")).unwrap(), root);

        // A symlink at the window's spelling resolves to the folder itself.
        #[cfg(unix)]
        {
            let links = temp_dir("require-open-links");
            std::os::unix::fs::symlink(&root, links.join("link")).unwrap();
            let spelled = links.join("link").display().to_string();
            assert_eq!(state.require_open(&spelled).unwrap(), root);
        }

        // What resolves outside the open set — a parent, another existing
        // folder, a path that does not exist — is refused, not acted on.
        let parent = root.parent().unwrap().to_path_buf();
        assert!(state.require_open(&parent.display().to_string()).is_err(), "the parent is not open");
        let other = crate::infra::paths::canonicalize_plain(&temp_dir("require-open-other")).unwrap();
        assert!(state.require_open(&other.display().to_string()).is_err(), "another existing folder is not open");
        assert!(state.require_open(&root.join("gone").display().to_string()).is_err(), "a missing path resolves nowhere");
    }

    /// `ui_workspace` is `require_open` for a window that may be in Chat
    /// mode: `Some` answers the folder or an error, `None` answers no folder.
    #[test]
    fn ui_workspace_accepts_only_open_folders_and_passes_none_through() {
        let state = state();
        assert_eq!(state.ui_workspace(&None), Ok(None));

        let root = crate::infra::paths::canonicalize_plain(&temp_dir("ui-workspace")).unwrap();
        state.register_open(root.clone());
        assert_eq!(state.ui_workspace(&Some(root.display().to_string())).unwrap(), Some(root.clone()));
        assert!(state.ui_workspace(&Some(root.join("elsewhere").display().to_string())).is_err());
    }
}
