//! The questions MCP servers are waiting on the user for.
//!
//! A call that meets a question blocks on it here (`wait`) while the window
//! shows it; the window's answer comes back through a command (`answer`).
//! Keyed by an id the turn made up and sent with the question, so an answer
//! to a question already gone — the call stopped, the window late — finds
//! nothing and is said to have found nothing.

use std::collections::HashMap;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::time::Duration;

use crate::domain::mcp::McpAnswer;

/// How often a waiting question looks at the turn's stop flag.
const POLL: Duration = Duration::from_millis(50);

#[derive(Default)]
pub struct McpQuestions {
    open: Mutex<HashMap<String, Sender<McpAnswer>>>,
}

impl McpQuestions {
    /// Waits for the answer to question `id`. Stop is the user leaving it
    /// unanswered, and the server hears `cancel`.
    pub fn wait(&self, id: &str, cancelled: &dyn Fn() -> bool) -> McpAnswer {
        let (answer, answered) = mpsc::channel();
        self.lock().insert(id.to_string(), answer);
        let answer = loop {
            match answered.recv_timeout(POLL) {
                Ok(answer) => break answer,
                Err(RecvTimeoutError::Timeout) if !cancelled() => {}
                Err(_) => break McpAnswer::Cancel,
            }
        };
        self.lock().remove(id);
        answer
    }

    /// `false` when nothing is waiting on `id` any more.
    pub fn answer(&self, id: &str, answer: McpAnswer) -> bool {
        self.lock().remove(id).is_some_and(|waiting| waiting.send(answer).is_ok())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Sender<McpAnswer>>> {
        self.open.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Instant;

    fn open(desk: &McpQuestions, id: &str) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !desk.lock().contains_key(id) {
            assert!(Instant::now() < deadline, "never opened");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_waiting_question_gets_the_answer_sent_for_it_and_only_once() {
        let desk = Arc::new(McpQuestions::default());
        let waiting = {
            let desk = Arc::clone(&desk);
            std::thread::spawn(move || desk.wait("q1", &|| false))
        };
        open(&desk, "q1");
        assert!(!desk.answer("q2", McpAnswer::Decline), "another question's answer");
        assert!(desk.answer("q1", McpAnswer::Decline));
        assert_eq!(waiting.join().unwrap(), McpAnswer::Decline);
        assert!(!desk.answer("q1", McpAnswer::Decline), "answered already");
        assert!(desk.lock().is_empty());
    }

    #[test]
    fn stop_while_waiting_is_a_cancel_and_the_question_closes() {
        let desk = Arc::new(McpQuestions::default());
        let stop = Arc::new(AtomicBool::new(false));
        let waiting = {
            let (desk, stop) = (Arc::clone(&desk), Arc::clone(&stop));
            std::thread::spawn(move || desk.wait("q1", &|| stop.load(Ordering::SeqCst)))
        };
        open(&desk, "q1");
        stop.store(true, Ordering::SeqCst);
        assert_eq!(waiting.join().unwrap(), McpAnswer::Cancel);
        assert!(desk.lock().is_empty(), "a stopped question is not kept");
        assert!(!desk.answer("q1", McpAnswer::Decline), "still open after its call stopped");
    }
}
