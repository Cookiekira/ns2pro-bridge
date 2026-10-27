//! Poison-tolerant locking. Every guarded value (counters, pending replies,
//! notification sinks) is consistent between statements, and release builds abort
//! on panic, so a poisoned lock carries no torn state worth propagating.
use std::sync::{Mutex, MutexGuard, PoisonError};

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
