//! Per-key/account request limiter, permits, and Codex account cooldowns.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use llm_access_core::store::AuthenticatedKey;

use super::{
    kiro_error::kiro_json_error, ActiveCooldown, CodexAccountCooldowns, LimitPermit,
    LimitRejection, RequestLimiter,
};

impl Drop for LimitPermit {
    fn drop(&mut self) {
        let Ok(mut scopes) = self.limiter.scopes.lock() else {
            return;
        };
        if let Some(scope) = scopes.get_mut(&self.scope) {
            scope.in_flight = scope.in_flight.saturating_sub(1);
        }
    }
}
impl RequestLimiter {
    pub(super) fn try_acquire(
        self: &Arc<Self>,
        scope: String,
        max_concurrency: Option<u64>,
        min_start_interval_ms: Option<u64>,
    ) -> Result<LimitPermit, LimitRejection> {
        let max_concurrency = max_concurrency.filter(|value| *value > 0);
        let min_interval = min_start_interval_ms
            .filter(|value| *value > 0)
            .map(Duration::from_millis);
        let mut scopes = self.scopes.lock().expect("request limiter mutex poisoned");
        let state = scopes.entry(scope.clone()).or_default();
        let concurrency_ready = max_concurrency
            .map(|limit| state.in_flight < limit)
            .unwrap_or(true);
        let elapsed_since_last_start = state.last_start.map(|last_start| last_start.elapsed());
        let interval_wait = min_interval.and_then(|interval| {
            elapsed_since_last_start.and_then(|elapsed| interval.checked_sub(elapsed))
        });
        if concurrency_ready && interval_wait.is_none() {
            state.in_flight = state.in_flight.saturating_add(1);
            state.last_start = Some(Instant::now());
            return Ok(LimitPermit {
                limiter: Arc::clone(self),
                scope,
            });
        }
        let reason = if !concurrency_ready { "max_concurrency" } else { "min_start_interval" };
        Err(LimitRejection {
            reason,
            in_flight: state.in_flight,
            max_concurrency,
            min_start_interval_ms,
            wait: interval_wait.or_else(|| Some(Duration::from_millis(10))),
            elapsed_since_last_start_ms: elapsed_since_last_start
                .map(|elapsed| elapsed.as_millis().min(u128::from(u64::MAX)) as u64),
        })
    }
}
impl CodexAccountCooldowns {
    /// Return the remaining request-path cooldown for one Codex account.
    ///
    /// This state is intentionally local and ephemeral:
    /// - it is only used to keep request routing from hammering an account that
    ///   just failed in the request path;
    /// - it does not participate in background refresh or token refresh;
    /// - it lazily expires on read so we do not need a separate cleanup task.
    pub(super) fn cooldown_for_account(&self, account_name: &str) -> Option<ActiveCooldown> {
        let Ok(mut blocked_until) = self.blocked_until.lock() else {
            return None;
        };
        let blocked_until_at = blocked_until.get(account_name).copied()?;
        let remaining = blocked_until_at.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            blocked_until.remove(account_name);
            return None;
        }
        Some(ActiveCooldown {
            remaining,
        })
    }

    /// Mark one Codex account as temporarily unavailable for request routing.
    ///
    /// The write semantics are deliberately "single-flight-like": once one
    /// request has already established a cooldown window, concurrent failures
    /// do not shorten it by overwriting with a smaller randomly sampled
    /// TTL. A new write only takes effect when it extends the blocked-until
    /// instant.
    pub(super) fn mark_account_cooldown(&self, account_name: &str, cooldown: Duration) {
        if cooldown.is_zero() {
            return;
        }
        let Ok(mut blocked_until) = self.blocked_until.lock() else {
            return;
        };
        let next_until = Instant::now() + cooldown;
        match blocked_until.get_mut(account_name) {
            Some(existing_until) if *existing_until >= next_until => {},
            Some(existing_until) => *existing_until = next_until,
            None => {
                blocked_until.insert(account_name.to_string(), next_until);
            },
        }
    }
}
pub fn try_acquire_key_permit(
    limiter: &Arc<RequestLimiter>,
    key: &AuthenticatedKey,
    max_concurrency: Option<u64>,
    min_start_interval_ms: Option<u64>,
) -> Result<LimitPermit, LimitRejection> {
    limiter.try_acquire(format!("key:{}", key.key_id), max_concurrency, min_start_interval_ms)
}
pub async fn wait_for_limit(rejection: Option<&LimitRejection>) {
    tokio::time::sleep(
        rejection
            .and_then(|rejection| rejection.wait)
            .unwrap_or_else(|| Duration::from_millis(10)),
    )
    .await;
}
pub fn codex_key_limit_response(rejection: &LimitRejection) -> Response {
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
pub fn kiro_key_limit_response(rejection: &LimitRejection) -> Response {
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
