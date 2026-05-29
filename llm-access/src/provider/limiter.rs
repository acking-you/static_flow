//! Per-key/account request-limit permit acquisition, wait backoff, and
//! limit-rejection responses.

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

pub(crate) fn try_acquire_key_permit(
    limiter: &Arc<RequestLimiter>,
    key: &AuthenticatedKey,
    max_concurrency: Option<u64>,
    min_start_interval_ms: Option<u64>,
) -> Result<LimitPermit, LimitRejection> {
    limiter.try_acquire(format!("key:{}", key.key_id), max_concurrency, min_start_interval_ms)
}
pub(crate) async fn wait_for_limit(rejection: Option<&LimitRejection>) {
    tokio::time::sleep(
        rejection
            .and_then(|rejection| rejection.wait)
            .unwrap_or_else(|| Duration::from_millis(10)),
    )
    .await;
}
pub(crate) fn codex_key_limit_response(rejection: &LimitRejection) -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        format!(
            "key request limit reached: {} in_flight={} request_max_concurrency={} \
             request_min_start_interval_ms={} wait_ms={} elapsed_since_last_start_ms={}",
            rejection.reason,
            rejection.in_flight,
            rejection
                .max_concurrency
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unlimited".to_string()),
            rejection
                .min_start_interval_ms
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unlimited".to_string()),
            rejection
                .wait
                .map(|value| value.as_millis() as u64)
                .unwrap_or(0),
            rejection.elapsed_since_last_start_ms.unwrap_or(0),
        ),
    )
        .into_response()
}
pub(crate) fn kiro_key_limit_response(rejection: &LimitRejection) -> Response {
    kiro_json_error(
        StatusCode::TOO_MANY_REQUESTS,
        "rate_limit_error",
        &format!(
            "Kiro key request limit reached: {} in_flight={} request_max_concurrency={} \
             request_min_start_interval_ms={} wait_ms={}",
            rejection.reason,
            rejection.in_flight,
            rejection
                .max_concurrency
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unlimited".to_string()),
            rejection
                .min_start_interval_ms
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unlimited".to_string()),
            rejection
                .wait
                .map(|value| value.as_millis() as u64)
                .unwrap_or(0),
        ),
    )
}
