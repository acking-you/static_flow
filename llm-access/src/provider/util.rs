//! Small time/clamp helpers.

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

pub(crate) fn now_millis() -> i64 {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    millis.min(i64::MAX as u128) as i64
}
pub(crate) fn clamp_u64_to_i64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}
pub(crate) fn clamp_usize_to_i64(value: usize) -> i64 {
    value.min(i64::MAX as usize) as i64
}
pub(crate) fn clamp_duration_ms(duration: Duration) -> i64 {
    duration.as_millis().min(i64::MAX as u128) as i64
}
pub(crate) fn now_seconds() -> i64 {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    seconds.min(i64::MAX as u64) as i64
}
