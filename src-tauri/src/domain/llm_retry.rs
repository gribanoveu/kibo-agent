//! Whether a failed round may be tried again, and how long to wait first.
//!
//! Pure decision, no sleeping and no I/O: the turn loop owns the waiting
//! because it is the only place that can keep the wait cancellable and tell
//! the UI why nothing is happening. What lives here is the part with rules
//! worth testing.
//!
//! This is *not* the rate-limit accounting Alfa Atlas has. That models one
//! corporate gateway's own window from the client side and answers "how much
//! budget is left"; this answers "the request was refused, now what" — which
//! is what a public API actually does to a long turn, and which Atlas does not
//! handle at all.

use std::time::Duration;

use crate::domain::llm::LlmError;

/// Retries in addition to the first request.
pub const MAX_ATTEMPTS: u32 = 5;

/// A connection dropped before the answer started is usually a proxy idling
/// out, and ten seconds is what was observed to clear it. Backing off further
/// only makes the pause longer for the same outcome.
const TRANSPORT_DELAY: Duration = Duration::from_secs(10);

/// Past this, waiting it out silently is worse than saying so: the turn would
/// sit apparently dead for minutes, and the user can no longer tell a long
/// rate-limit window from a hung app.
const MAX_WAIT: Duration = Duration::from_secs(120);

/// How long to wait before trying this round again, or `None` to give up and
/// report the error.
///
/// `produced_output` is the decisive argument and the easiest to get wrong.
/// Once any of the round reached the caller — a text delta, a tool call being
/// assembled — the round is no longer repeatable: a retry appends the text a
/// second time and re-runs whatever the first attempt already set in motion.
/// It applies to every retryable class, not just to dropped connections.
pub fn retry_delay(error: &LlmError, attempt: u32, produced_output: bool) -> Option<Duration> {
    if produced_output || attempt >= MAX_ATTEMPTS {
        return None;
    }
    match error {
        LlmError::RateLimited {
            retry_after_seconds,
            ..
        }
        | LlmError::Unavailable {
            retry_after_seconds,
            ..
        } => Some(rate_limit_delay(*retry_after_seconds, attempt)?),
        // Only a drop with nothing received. Any other HTTP failure is the
        // provider's considered answer, and sending the same request again
        // gets the same answer.
        LlmError::Http(message) if is_transport_drop(message) => Some(TRANSPORT_DELAY),
        _ => None,
    }
}

/// The server's own hint wins whenever it sent one — it is the only party that
/// knows when the window actually reopens, and guessing shorter buys another
/// refusal at the cost of one of the few attempts available.
fn rate_limit_delay(hint_seconds: Option<u64>, attempt: u32) -> Option<Duration> {
    match hint_seconds {
        Some(seconds) => {
            let wait = Duration::from_secs(seconds);
            (wait <= MAX_WAIT).then_some(wait)
        }
        // No hint: back off rather than hammer. 1, 2, 4, 8, 16 seconds.
        None => Some(Duration::from_secs(1 << attempt)),
    }
}

fn is_transport_drop(message: &str) -> bool {
    message.to_ascii_lowercase().contains("peer disconnected")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rate_limited(retry_after_seconds: Option<u64>) -> LlmError {
        LlmError::RateLimited {
            retry_after_seconds,
            message: "slow down".to_string(),
        }
    }

    fn dropped() -> LlmError {
        LlmError::Http("http error: Peer disconnected".to_string())
    }

    #[test]
    fn the_servers_own_hint_is_what_is_waited() {
        assert_eq!(
            retry_delay(&rate_limited(Some(20)), 0, false),
            Some(Duration::from_secs(20))
        );
    }

    /// Guessing shorter than the server said buys another refusal, and each
    /// one costs an attempt.
    #[test]
    fn a_hint_is_not_shortened_by_the_backoff() {
        let delay = retry_delay(&rate_limited(Some(45)), 3, false).unwrap();
        assert_eq!(delay, Duration::from_secs(45));
    }

    #[test]
    fn without_a_hint_the_wait_grows() {
        let delays: Vec<u64> = (0..MAX_ATTEMPTS)
            .map(|attempt| {
                retry_delay(&rate_limited(None), attempt, false)
                    .expect("retryable")
                    .as_secs()
            })
            .collect();
        assert_eq!(delays, [1, 2, 4, 8, 16]);
    }

    /// A window that reopens in ten minutes is reported, not slept through:
    /// the turn would otherwise sit apparently dead, and the message says how
    /// long the wait actually is.
    #[test]
    fn a_wait_longer_than_the_cap_is_reported_instead() {
        assert_eq!(retry_delay(&rate_limited(Some(600)), 0, false), None);
    }

    #[test]
    fn attempts_run_out() {
        assert!(retry_delay(&rate_limited(None), MAX_ATTEMPTS - 1, false).is_some());
        assert!(retry_delay(&rate_limited(None), MAX_ATTEMPTS, false).is_none());
    }

    /// The rule that makes retrying safe at all. Half a round has already
    /// reached the transcript and whatever tool call it carried is already in
    /// motion — sending the request again duplicates both.
    #[test]
    fn a_round_that_produced_output_is_never_retried() {
        for error in [rate_limited(Some(1)), rate_limited(None), dropped()] {
            assert_eq!(retry_delay(&error, 0, true), None, "{error}");
        }
    }

    /// A gateway's 502 is waited out like a rate limit, hint and backoff alike.
    #[test]
    fn a_provider_failing_on_its_side_is_retried() {
        let unavailable = |hint| LlmError::Unavailable { retry_after_seconds: hint, message: "http status 502".into() };
        assert_eq!(retry_delay(&unavailable(Some(7)), 0, false), Some(Duration::from_secs(7)));
        assert_eq!(retry_delay(&unavailable(None), 2, false), Some(Duration::from_secs(4)));
        assert_eq!(retry_delay(&unavailable(None), 0, true), None, "not once output got out");
    }

    #[test]
    fn a_dropped_connection_is_retried_at_a_fixed_delay() {
        assert_eq!(retry_delay(&dropped(), 0, false), Some(TRANSPORT_DELAY));
        assert_eq!(retry_delay(&dropped(), 3, false), Some(TRANSPORT_DELAY));
    }

    /// Everything the provider answered on purpose. Repeating the request
    /// produces the same answer, so a retry only delays the error.
    #[test]
    fn a_considered_refusal_is_not_retried() {
        for error in [
            LlmError::Http("http status 400: no such model".to_string()),
            LlmError::Http("http status 401: bad key".to_string()),
            LlmError::Http("http error: timeout".to_string()),
            LlmError::Provider("malformed chunk".to_string()),
            LlmError::Tls("unknown issuer".to_string()),
            LlmError::Message("no API key is stored".to_string()),
        ] {
            assert_eq!(retry_delay(&error, 0, false), None, "{error}");
        }
    }
}
