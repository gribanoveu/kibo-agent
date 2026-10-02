//! A lock that outlives a panic.

use std::sync::{Mutex, MutexGuard};

/// What `mutex` holds, poisoned or not: a panic elsewhere while holding it
/// leaves the value usable, and is not a reason to stop.
pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
