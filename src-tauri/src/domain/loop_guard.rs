//! Noticing a turn that goes round in circles, and saying so to the model.
//!
//! Two shapes of the same trouble, each counted over **consecutive** rounds —
//! a round without the key ends its streak, so an edit between two runs of a
//! failing test is progress, not a loop:
//!
//! * **The same call, the same result** — a file re-read that has not changed,
//!   a test re-run that fails with the same output, a process polled that has
//!   written nothing. Repeating it cannot change the answer.
//! * **The same failure** — one tool refusing the same way with whatever
//!   arguments: an edit anchor that is never found, a write that was never
//!   preceded by a read. The route is wrong, not the details.
//!
//! At [`REMIND_AFTER`] the model is told once, as a note in the history. Once
//! per turn: a model that goes on after the note is stopped by the budget,
//! and a second note would only cost another round. The weighted budget stays
//! the ceiling; this is the nudge before it.
//!
//! The idea and the wording follow MiniMax Code's runaway guard
//! (`docs/19-minimax-code-ideas.md`, § 2).

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::mem::Discriminant;

use crate::domain::tools::ToolError;

/// How many rounds running make a loop. Three: twice is how a model checks
/// its own result, and waiting longer spends the rounds this exists to save.
pub const REMIND_AFTER: u32 = 3;

/// What kind of failure an error is — its variant, not its text, which names
/// the path or the anchor and so differs every time the model tries another.
pub type ErrorKind = Discriminant<ToolError>;

/// An edit that failed inside a batch fails for its own reason.
pub fn error_kind(error: &ToolError) -> ErrorKind {
    match error {
        ToolError::InEdit { reason, .. } => error_kind(reason),
        other => std::mem::discriminant(other),
    }
}

/// One settled call, as the guard reads it.
pub struct Settled {
    pub tool: String,
    pub arguments: String,
    /// What the model was given back, error text included.
    pub content: String,
    /// `None` for a call that ran — a command's non-zero exit is an answer,
    /// not a failure of the call. A call the user denied is not a loop and
    /// is not handed in at all.
    pub error: Option<ErrorKind>,
}

/// What the model is told about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Loop {
    SameCall { tool: String },
    SameError { tool: String },
}

impl Loop {
    pub fn tool(&self) -> &str {
        match self {
            Loop::SameCall { tool } | Loop::SameError { tool } => tool,
        }
    }

    /// The note added to the history. Says it is for this turn only: a model
    /// that keeps notes (rules, memory files) must not write it down as one.
    pub fn note(&self) -> String {
        let body = match self {
            Loop::SameCall { tool } => format!(
                "`{tool}` has now been called with the same arguments and come back with the same result \
                 {REMIND_AFTER} rounds running. Repeating it will not change the answer. Look at what the results \
                 already say, then either change approach with a concrete idea of what will be different, or — if \
                 something outside your reach is in the way — stop and say what it is. Repetition alone does not \
                 mean the task is done, nor that it is impossible."
            ),
            Loop::SameError { tool } => format!(
                "`{tool}` has now failed the same way {REMIND_AFTER} rounds running. Do not retry the same route \
                 unchanged: find the cause first — re-read the file, check the path, read the error in full — then \
                 change one thing, or take another route. This does not mean the whole task has failed."
            ),
        };
        format!("[Loop guard] {body} This note is about this turn only — not a rule to write down anywhere.")
    }
}

/// The counts one turn carries from round to round. Not part of the approval
/// checkpoint: a resume is the user's go-ahead, and starts the count over.
#[derive(Debug, Default)]
pub struct LoopGuard {
    calls: HashMap<u64, (u32, String)>,
    errors: HashMap<u64, (u32, String)>,
    reminded: bool,
}

impl LoopGuard {
    /// Takes one round's settled calls — empty for a round that made none,
    /// which ends every streak — and returns what to tell the model, at most
    /// once a turn.
    pub fn observe(&mut self, round: &[Settled]) -> Option<Loop> {
        self.calls = streaks(
            &self.calls,
            round.iter().map(|call| (key(&(&call.tool, &call.arguments, &call.content)), call.tool.as_str())),
        );
        self.errors = streaks(
            &self.errors,
            round.iter().filter_map(|call| call.error.map(|kind| (key(&(&call.tool, kind)), call.tool.as_str()))),
        );
        if self.reminded {
            return None;
        }
        // A failure first: its note says what to do about it.
        let found = looping(&self.errors)
            .map(|tool| Loop::SameError { tool })
            .or_else(|| looping(&self.calls).map(|tool| Loop::SameCall { tool }));
        self.reminded = found.is_some();
        found
    }
}

fn key(value: &impl Hash) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

/// Each key in this round, counted on from the streak it continues. Keys the
/// round did not have are dropped: their streak is over.
fn streaks<'a>(
    previous: &HashMap<u64, (u32, String)>,
    round: impl Iterator<Item = (u64, &'a str)>,
) -> HashMap<u64, (u32, String)> {
    let mut next: HashMap<u64, (u32, String)> = HashMap::new();
    for (key, tool) in round {
        let entry = next
            .entry(key)
            .or_insert_with(|| (previous.get(&key).map_or(0, |(count, _)| *count), tool.to_string()));
        entry.0 += 1;
    }
    next
}

/// The tool of a streak that has reached the threshold, if any. The lowest
/// key when there are several, so the answer does not depend on map order.
fn looping(streaks: &HashMap<u64, (u32, String)>) -> Option<String> {
    streaks
        .iter()
        .filter(|(_, (count, _))| *count >= REMIND_AFTER)
        .min_by_key(|(key, _)| **key)
        .map(|(_, (_, tool))| tool.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settled(tool: &str, arguments: &str, content: &str, error: Option<ToolError>) -> Settled {
        Settled {
            tool: tool.into(),
            arguments: arguments.into(),
            content: content.into(),
            error: error.as_ref().map(error_kind),
        }
    }

    fn read(content: &str) -> Settled {
        settled("readFile", r#"{"path":"a.rs"}"#, content, None)
    }

    fn not_found(arguments: &str) -> Settled {
        let error = ToolError::EditTextNotFound { text: String::new(), nearest: None };
        settled("editFile", arguments, "Error: not found", Some(error))
    }

    #[test]
    fn the_same_call_with_the_same_result_three_rounds_running_is_a_loop() {
        let mut guard = LoopGuard::default();
        assert_eq!(guard.observe(&[read("fn a() {}")]), None);
        assert_eq!(guard.observe(&[read("fn a() {}")]), None);
        assert_eq!(guard.observe(&[read("fn a() {}")]), Some(Loop::SameCall { tool: "readFile".into() }));
    }

    #[test]
    fn a_result_that_changed_is_not_a_repeat() {
        let mut guard = LoopGuard::default();
        guard.observe(&[read("fn a() {}")]);
        guard.observe(&[read("fn a() { 1 }")]);
        assert_eq!(guard.observe(&[read("fn a() { 1 }")]), None);
    }

    #[test]
    fn a_round_without_the_call_ends_its_streak() {
        let mut guard = LoopGuard::default();
        guard.observe(&[read("x")]);
        guard.observe(&[read("x")]);
        guard.observe(&[]);
        assert_eq!(guard.observe(&[read("x")]), None);
    }

    #[test]
    fn the_same_call_three_times_in_one_round_counts_as_three() {
        let mut guard = LoopGuard::default();
        assert_eq!(guard.observe(&[read("x"), read("x"), read("x")]), Some(Loop::SameCall { tool: "readFile".into() }));
    }

    #[test]
    fn the_same_failure_with_other_arguments_is_a_loop_of_its_own() {
        let mut guard = LoopGuard::default();
        guard.observe(&[not_found(r#"{"old":"a"}"#)]);
        guard.observe(&[not_found(r#"{"old":"b"}"#)]);
        assert_eq!(guard.observe(&[not_found(r#"{"old":"c"}"#)]), Some(Loop::SameError { tool: "editFile".into() }));
    }

    #[test]
    fn different_failures_are_not_one_streak() {
        let mut guard = LoopGuard::default();
        let not_read = settled("editFile", "{}", "Error: not read", Some(ToolError::FileNotRead("a.rs".into())));
        guard.observe(&[not_found(r#"{"old":"a"}"#)]);
        guard.observe(&[not_read]);
        assert_eq!(guard.observe(&[not_found(r#"{"old":"c"}"#)]), None);
    }

    #[test]
    fn a_failure_inside_a_batch_counts_as_its_own_kind() {
        let inner = ToolError::EditTextNotFound { text: String::new(), nearest: None };
        let batch = ToolError::InEdit { index: 2, of: 3, reason: Box::new(ToolError::EditTextNotFound { text: String::new(), nearest: None }) };
        assert_eq!(error_kind(&batch), error_kind(&inner));
    }

    #[test]
    fn a_failure_is_named_before_a_repeat_when_both_are_there() {
        let mut guard = LoopGuard::default();
        for _ in 0..2 {
            assert_eq!(guard.observe(&[not_found("{}")]), None);
        }
        assert_eq!(guard.observe(&[not_found("{}")]), Some(Loop::SameError { tool: "editFile".into() }));
    }

    #[test]
    fn the_model_is_told_once_a_turn() {
        let mut guard = LoopGuard::default();
        for _ in 0..3 {
            guard.observe(&[read("x")]);
        }
        assert_eq!(guard.observe(&[read("x")]), None);
        for _ in 0..3 {
            assert_eq!(guard.observe(&[not_found("{}")]), None);
        }
    }

    #[test]
    fn the_note_names_the_tool_and_says_it_is_for_this_turn() {
        let note = Loop::SameError { tool: "editFile".into() }.note();
        assert!(note.contains("`editFile`") && note.contains(&REMIND_AFTER.to_string()), "{note}");
        assert!(note.contains("this turn only"), "{note}");
        assert!(Loop::SameCall { tool: "readFile".into() }.note().contains("same result"));
    }
}
