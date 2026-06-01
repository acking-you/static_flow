//! Shared request and trace id helpers.

use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

/// Request id header preserved across gateway and backend.
pub const REQUEST_ID_HEADER: &str = "x-request-id";

/// Trace id header preserved across gateway and backend.
pub const TRACE_ID_HEADER: &str = "x-trace-id";

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Keep a caller-provided id when it is non-empty, otherwise generate a new
/// one.
pub fn read_or_generate_id(raw_value: Option<&str>, prefix: &str) -> String {
    raw_value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| generate_id(prefix))
}

/// Generate a monotonic request or trace id with a stable prefix.
pub fn generate_id(prefix: &str) -> String {
    let now_ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or_default();
    let counter = REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{now_ns:032x}-{counter:016x}")
}

#[cfg(test)]
mod tests {
    use super::{generate_id, read_or_generate_id};

    #[test]
    fn read_or_generate_id_keeps_existing_value() {
        let value = read_or_generate_id(Some("req-existing"), "req");
        assert_eq!(value, "req-existing");
    }

    #[test]
    fn generate_id_uses_prefix() {
        let value = generate_id("trace");
        assert!(value.starts_with("trace-"));
    }
}
