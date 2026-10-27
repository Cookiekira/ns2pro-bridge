//! Best-effort recovery from poisoned locks. This is a policy, not a repair: the
//! value is returned exactly as the panicking holder left it and the poison flag
//! stays set. Recovery only stops poison from blocking access and teardown:
//! statistics are diagnostics, pending-reply and reader-task slots are taken or
//! replaced, and notification paths may still invoke an existing sink. It neither
//! repairs a sink callback's state nor restarts a reader task that panicked. This
//! workspace's release and dist profiles abort on panic, so there recovery is moot.
use std::sync::{Mutex, MutexGuard, PoisonError};

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
