//! Small process-local helpers shared across the image gateway modules.

use std::{
    sync::{Mutex, MutexGuard},
    time::{SystemTime, UNIX_EPOCH},
};

/// Current Unix time in milliseconds, saturating to `0` before the epoch.
pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// Lock a mutex, recovering the guard if a previous holder panicked.
///
/// Every gateway lock guards plain bookkeeping (in-flight counters, the log
/// writer, the proxy-client cache), so a poisoned lock is safe to keep using
/// instead of propagating the original panic into every subsequent request.
pub(crate) fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
