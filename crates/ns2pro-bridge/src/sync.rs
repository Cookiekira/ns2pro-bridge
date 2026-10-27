//! Best-effort recovery from poisoned locks. This is a policy, not a repair: the
//! value is returned exactly as the panicking holder left it and the poison flag
//! stays set. The current owners tolerate that: statistics are diagnostics, and
//! the pending-reply, reader-task and notification-sink slots are only replaced,
//! taken or dropped, never read for a cross-field invariant. Recovery lets a link
//! be torn down or resubscribed after a sink callback panicked; it does not make
//! that callback's own state consistent. Release builds abort on panic, so
//! recovery only matters in debug and test builds.
use std::sync::{Mutex, MutexGuard, PoisonError};

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
