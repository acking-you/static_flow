use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use llm_access_core::store::DEFAULT_CODEX_IMAGE_GENERATION_MAX_CONCURRENCY;

use crate::util::lock_unpoisoned;

/// Process-local in-flight limiter for per-account Codex image concurrency.
///
/// The cap is enforced only within this process: each running gateway (the
/// standalone binary and/or the integrated Codex API binary) keeps its own
/// counter, so the effective per-account concurrency is `cap * number of
/// gateway processes serving that account`. Treat the configured cap as a
/// per-process bound, not a global reservation.
#[derive(Clone, Debug, Default)]
pub struct ImageAccountLimiter {
    states: Arc<Mutex<HashMap<String, u64>>>,
}

/// Process-local limiter for per-key Codex image throttling.
///
/// Enforces two independent gates per key: a max in-flight concurrency and a
/// minimum interval between request *starts*. Like [`ImageAccountLimiter`] the
/// state is per-process (see its note on aggregate concurrency across
/// processes).
#[derive(Clone, Debug, Default)]
pub struct ImageKeyLimiter {
    states: Arc<Mutex<HashMap<String, ImageKeyLimitState>>>,
}

#[derive(Clone, Copy, Debug, Default)]
struct ImageKeyLimitState {
    in_flight: u64,
    last_start: Option<Instant>,
}

/// Permit held for the lifetime of one upstream image request.
#[derive(Debug)]
pub struct ImageAccountPermit {
    scope: String,
    states: Arc<Mutex<HashMap<String, u64>>>,
}

/// Permit held for one key-level image request.
#[derive(Debug)]
pub struct ImageKeyPermit {
    scope: String,
    states: Arc<Mutex<HashMap<String, ImageKeyLimitState>>>,
}

/// Rejection metadata for a key-level image request limit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageKeyLimitRejection {
    /// Stable reason for logging and downstream response text.
    pub reason: &'static str,
    /// Current in-flight requests for the scope.
    pub in_flight: u64,
    /// Configured max concurrency, when bounded.
    pub max_concurrency: Option<u64>,
    /// Configured min start interval, when bounded.
    pub min_start_interval_ms: Option<u64>,
}

/// Returns the independent limiter scope used for image account concurrency.
pub fn image_account_limiter_scope(account_name: &str) -> String {
    format!("account:codex-image:{account_name}")
}

/// Returns the independent limiter scope used for image key throttling.
pub fn image_key_limiter_scope(key_id: &str) -> String {
    format!("key:codex-image:{key_id}")
}

impl ImageAccountLimiter {
    /// Attempts to acquire one account image permit without waiting.
    pub fn try_acquire(
        &self,
        account_name: &str,
        limit: Option<u64>,
    ) -> Option<ImageAccountPermit> {
        let limit = limit
            .unwrap_or(DEFAULT_CODEX_IMAGE_GENERATION_MAX_CONCURRENCY)
            .max(1);
        let scope = image_account_limiter_scope(account_name);
        let mut states = lock_unpoisoned(&self.states);
        let in_flight = states.entry(scope.clone()).or_default();
        if *in_flight >= limit {
            return None;
        }
        *in_flight += 1;
        Some(ImageAccountPermit {
            scope,
            states: Arc::clone(&self.states),
        })
    }
}

impl ImageKeyLimiter {
    /// Attempt to admit one key-level image request without waiting.
    ///
    /// Both gates must pass: `in_flight` must be below `max_concurrency` (when
    /// set) and at least `min_start_interval_ms` must have elapsed since the
    /// last admitted *start* (when set). A `0` bound disables that gate.
    ///
    /// The interval clock is advanced only on a successful admit, so a
    /// rejected request never moves it. The clock gates request *starts*, so a
    /// request that is admitted but then fails on every upstream account still
    /// counts as a start — this intentionally rate-limits retries rather than
    /// letting a client hammer the gateway while all accounts are unavailable.
    pub fn try_acquire(
        &self,
        key_id: &str,
        max_concurrency: Option<u64>,
        min_start_interval_ms: Option<u64>,
    ) -> Result<ImageKeyPermit, ImageKeyLimitRejection> {
        let max_concurrency = max_concurrency.filter(|value| *value > 0);
        let min_interval = min_start_interval_ms
            .filter(|value| *value > 0)
            .map(Duration::from_millis);
        let scope = image_key_limiter_scope(key_id);
        let mut states = lock_unpoisoned(&self.states);
        let state = states.entry(scope.clone()).or_default();
        let concurrency_ready = max_concurrency
            .map(|limit| state.in_flight < limit)
            .unwrap_or(true);
        let interval_ready = min_interval
            .zip(state.last_start)
            .map(|(interval, last_start)| last_start.elapsed() >= interval)
            .unwrap_or(true);
        if concurrency_ready && interval_ready {
            state.in_flight = state.in_flight.saturating_add(1);
            // Stamp the interval clock only when an interval is configured.
            // Leaving `last_start == None` for unthrottled keys lets the permit
            // `Drop` reclaim the idle map entry instead of leaking it forever.
            if min_interval.is_some() {
                state.last_start = Some(Instant::now());
            }
            return Ok(ImageKeyPermit {
                scope,
                states: Arc::clone(&self.states),
            });
        }
        Err(ImageKeyLimitRejection {
            reason: if !concurrency_ready {
                "key_max_concurrency"
            } else {
                "key_min_start_interval"
            },
            in_flight: state.in_flight,
            max_concurrency,
            min_start_interval_ms,
        })
    }
}

impl Drop for ImageAccountPermit {
    fn drop(&mut self) {
        let mut states = lock_unpoisoned(&self.states);
        let Some(in_flight) = states.get_mut(&self.scope) else {
            return;
        };
        *in_flight = in_flight.saturating_sub(1);
        if *in_flight == 0 {
            states.remove(&self.scope);
        }
    }
}

impl Drop for ImageKeyPermit {
    fn drop(&mut self) {
        let mut states = lock_unpoisoned(&self.states);
        let Some(state) = states.get_mut(&self.scope) else {
            return;
        };
        state.in_flight = state.in_flight.saturating_sub(1);
        // Reclaim the entry once the key is idle and carries no interval clock
        // to preserve. Keys with a configured min-start-interval keep their
        // `last_start` so the next request stays throttled; that retained set
        // is bounded by the number of interval-configured keys.
        if state.in_flight == 0 && state.last_start.is_none() {
            states.remove(&self.scope);
        }
    }
}
