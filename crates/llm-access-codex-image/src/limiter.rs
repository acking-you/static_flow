use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use llm_access_core::store::DEFAULT_CODEX_IMAGE_GENERATION_MAX_CONCURRENCY;

/// Process-local limiter for Codex image requests.
#[derive(Clone, Debug, Default)]
pub struct ImageAccountLimiter {
    states: Arc<Mutex<HashMap<String, u64>>>,
}

/// Permit held for the lifetime of one upstream image request.
#[derive(Debug)]
pub struct ImageAccountPermit {
    scope: String,
    states: Arc<Mutex<HashMap<String, u64>>>,
}

/// Returns the independent limiter scope used for image account concurrency.
pub fn image_account_limiter_scope(account_name: &str) -> String {
    format!("account:codex-image:{account_name}")
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
        let mut states = self.states.lock().expect("image limiter mutex poisoned");
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

impl Drop for ImageAccountPermit {
    fn drop(&mut self) {
        let mut states = self.states.lock().expect("image limiter mutex poisoned");
        let Some(in_flight) = states.get_mut(&self.scope) else {
            return;
        };
        *in_flight = in_flight.saturating_sub(1);
        if *in_flight == 0 {
            states.remove(&self.scope);
        }
    }
}
