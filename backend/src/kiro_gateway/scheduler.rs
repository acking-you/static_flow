//! Account-scoped request scheduler with per-account and proxy cooldown
//! tracking.
//!
//! [`KiroRequestScheduler`] keeps local concurrency and pacing state per Kiro
//! account instead of globally. The provider can therefore skip a throttled
//! account and immediately try the next one, only waiting when every eligible
//! account is locally blocked or cooling down.

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use parking_lot::Mutex;
use tokio::sync::Notify;

#[derive(Debug, Clone)]
struct AccountSchedulerState {
    in_flight: usize,
    next_start_at: Instant,
}

#[derive(Debug, Clone)]
struct AccountCooldownEntry {
    until: Instant,
    reason: String,
}

/// Active cooldown state for a single account, returned by
/// [`KiroRequestScheduler::cooldown_for_account`].
#[derive(Debug, Clone)]
pub(crate) struct AccountCooldown {
    pub remaining: Duration,
    pub reason: String,
}

/// Why the scheduler refused to start a request on an account right now.
#[derive(Debug, Clone)]
pub(crate) struct AccountLocalThrottle {
    pub wait: Option<Duration>,
    pub reason: &'static str,
    pub in_flight: usize,
    pub max_concurrency: usize,
    pub min_start_interval_ms: u64,
}

/// Per-account local limiter plus upstream cooldown tracker.
#[derive(Debug, Clone)]
pub(crate) struct KiroRequestScheduler {
    states: Arc<Mutex<HashMap<String, AccountSchedulerState>>>,
    cooldowns: Arc<Mutex<HashMap<String, AccountCooldownEntry>>>,
    proxy_cooldowns: Arc<Mutex<HashMap<String, AccountCooldownEntry>>>,
    last_started_at: Arc<Mutex<HashMap<String, Instant>>>,
    notify: Arc<Notify>,
}

/// RAII guard representing one in-flight request slot for a specific account.
#[derive(Debug)]
pub(crate) struct KiroRequestLease {
    scheduler: Arc<KiroRequestScheduler>,
    account_name: String,
    released: bool,
    waited_ms: u64,
}

impl KiroRequestScheduler {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            states: Arc::new(Mutex::new(HashMap::new())),
            cooldowns: Arc::new(Mutex::new(HashMap::new())),
            proxy_cooldowns: Arc::new(Mutex::new(HashMap::new())),
            last_started_at: Arc::new(Mutex::new(HashMap::new())),
            notify: Arc::new(Notify::new()),
        })
    }

    /// Attempt to reserve one local request slot for `account_name` without
    /// blocking. Callers can rotate to another account when this returns
    /// [`AccountLocalThrottle`].
    pub(crate) fn try_acquire(
        self: &Arc<Self>,
        account_name: &str,
        max_concurrency: u64,
        min_start_interval_ms: u64,
        queued_at: Instant,
    ) -> Result<KiroRequestLease, AccountLocalThrottle> {
        let max_concurrency = max_concurrency.max(1) as usize;
        let now = Instant::now();
        let mut states = self.states.lock();
        let state =
            states
                .entry(account_name.to_string())
                .or_insert_with(|| AccountSchedulerState {
                    in_flight: 0,
                    next_start_at: now,
                });

        if state.in_flight >= max_concurrency {
            return Err(AccountLocalThrottle {
                wait: None,
                reason: "local_concurrency_limit",
                in_flight: state.in_flight,
                max_concurrency,
                min_start_interval_ms,
            });
        }

        if now < state.next_start_at {
            return Err(AccountLocalThrottle {
                wait: Some(state.next_start_at.saturating_duration_since(now)),
                reason: "local_start_interval",
                in_flight: state.in_flight,
                max_concurrency,
                min_start_interval_ms,
            });
        }

        state.in_flight += 1;
        state.next_start_at = if min_start_interval_ms == 0 {
            now
        } else {
            now + Duration::from_millis(min_start_interval_ms)
        };
        self.last_started_at
            .lock()
            .insert(account_name.to_string(), now);

        Ok(KiroRequestLease {
            scheduler: self.clone(),
            account_name: account_name.to_string(),
            released: false,
            waited_ms: queued_at.elapsed().as_millis() as u64,
        })
    }

    /// Wait for either a local slot release notification or an optional
    /// timeout, whichever happens first.
    pub(crate) async fn wait_for_available(&self, wait: Option<Duration>) {
        match wait {
            Some(duration) => {
                tokio::select! {
                    _ = self.notify.notified() => {},
                    _ = tokio::time::sleep(duration) => {},
                }
            },
            None => self.notify.notified().await,
        }
    }

    /// Return the remaining upstream cooldown for `account_name`, or `None` if
    /// expired/absent.
    pub(crate) fn cooldown_for_account(&self, account_name: &str) -> Option<AccountCooldown> {
        let now = Instant::now();
        let mut cooldowns = self.cooldowns.lock();
        match cooldowns.get(account_name).cloned() {
            Some(entry) if entry.until > now => Some(AccountCooldown {
                remaining: entry.until.saturating_duration_since(now),
                reason: entry.reason,
            }),
            Some(_) => {
                cooldowns.remove(account_name);
                None
            },
            None => None,
        }
    }

    /// Record an upstream-imposed cooldown for `account_name` so it is skipped
    /// by the provider until the window expires.
    pub(crate) fn mark_account_cooldown(
        &self,
        account_name: &str,
        duration: Duration,
        reason: impl Into<String>,
    ) {
        let reason = reason.into();
        let until = Instant::now() + duration;
        self.cooldowns
            .lock()
            .insert(account_name.to_string(), AccountCooldownEntry {
                until,
                reason: reason.clone(),
            });
        tracing::warn!(
            account_name,
            cooldown_ms = duration.as_millis() as u64,
            reason,
            "marked kiro account in upstream cooldown window"
        );
        self.notify.notify_waiters();
    }

    /// Return the shortest remaining upstream cooldown across all accounts,
    /// pruning expired entries.
    pub(crate) fn shortest_cooldown(&self) -> Option<Duration> {
        let now = Instant::now();
        let mut cooldowns = self.cooldowns.lock();
        cooldowns.retain(|_, entry| entry.until > now);
        cooldowns
            .values()
            .map(|entry| entry.until.saturating_duration_since(now))
            .min()
    }

    /// Return all active proxy cooldowns, pruning expired entries first.
    pub(crate) fn proxy_cooldown_snapshot(&self) -> HashMap<String, AccountCooldown> {
        let now = Instant::now();
        let mut cooldowns = self.proxy_cooldowns.lock();
        cooldowns.retain(|_, entry| entry.until > now);
        cooldowns
            .iter()
            .map(|(proxy_key, entry)| {
                (proxy_key.clone(), AccountCooldown {
                    remaining: entry.until.saturating_duration_since(now),
                    reason: entry.reason.clone(),
                })
            })
            .collect()
    }

    /// Record an upstream-imposed cooldown for one proxy. Provider selection
    /// deprioritizes accounts using this proxy while still keeping them as a
    /// fallback if every better option is unavailable.
    pub(crate) fn mark_proxy_cooldown(
        &self,
        proxy_key: &str,
        duration: Duration,
        reason: impl Into<String>,
    ) {
        let reason = reason.into();
        let until = Instant::now() + duration;
        self.proxy_cooldowns
            .lock()
            .insert(proxy_key.to_string(), AccountCooldownEntry {
                until,
                reason: reason.clone(),
            });
        tracing::warn!(
            proxy_key,
            cooldown_ms = duration.as_millis() as u64,
            reason,
            "marked kiro upstream proxy in cooldown window"
        );
        self.notify.notify_waiters();
    }

    pub(crate) fn last_started_snapshot(&self) -> HashMap<String, Instant> {
        self.last_started_at.lock().clone()
    }

    pub(crate) fn notify_config_changed(&self) {
        self.notify.notify_waiters();
    }

    fn release(&self, account_name: &str) {
        {
            let now = Instant::now();
            let mut states = self.states.lock();
            let remove_entry = if let Some(state) = states.get_mut(account_name) {
                if state.in_flight > 0 {
                    state.in_flight -= 1;
                }
                state.in_flight == 0 && state.next_start_at <= now
            } else {
                false
            };
            if remove_entry {
                states.remove(account_name);
            }
        }
        self.notify.notify_waiters();
    }
}

impl KiroRequestLease {
    pub(crate) fn waited_ms(&self) -> u64 {
        self.waited_ms
    }
}

impl Drop for KiroRequestLease {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        self.scheduler.release(&self.account_name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheduler_enforces_per_account_concurrency() {
        let scheduler = KiroRequestScheduler::new();
        let started = Instant::now();
        let first = scheduler
            .try_acquire("alpha", 1, 0, started)
            .expect("first acquire should succeed");
        let blocked = scheduler
            .try_acquire("alpha", 1, 0, started)
            .expect_err("second acquire should be blocked");
        assert_eq!(blocked.reason, "local_concurrency_limit");
        assert_eq!(blocked.in_flight, 1);
        drop(first);
        scheduler
            .try_acquire("alpha", 1, 0, started)
            .expect("acquire should succeed after release");
    }

    #[test]
    fn scheduler_enforces_per_account_start_interval() {
        let scheduler = KiroRequestScheduler::new();
        let started = Instant::now();
        let first = scheduler
            .try_acquire("alpha", 2, 80, started)
            .expect("first acquire should succeed");
        drop(first);
        let blocked = scheduler
            .try_acquire("alpha", 2, 80, started)
            .expect_err("second acquire should be blocked by spacing");
        assert_eq!(blocked.reason, "local_start_interval");
        assert!(blocked.wait.expect("spacing wait should exist") >= Duration::from_millis(70));
    }

    #[test]
    fn scheduler_state_is_isolated_per_account() {
        let scheduler = KiroRequestScheduler::new();
        let started = Instant::now();
        let first = scheduler
            .try_acquire("alpha", 1, 0, started)
            .expect("alpha should acquire");
        scheduler
            .try_acquire("beta", 1, 0, started)
            .expect("beta should remain available");
        drop(first);
    }

    #[test]
    fn cooldown_entries_expire() {
        let scheduler = KiroRequestScheduler::new();
        scheduler.mark_account_cooldown("alpha", Duration::from_millis(20), "rate limit");
        let cooldown = scheduler
            .cooldown_for_account("alpha")
            .expect("cooldown should exist");
        assert_eq!(cooldown.reason, "rate limit");
        std::thread::sleep(Duration::from_millis(25));
        assert!(scheduler.cooldown_for_account("alpha").is_none());
    }
}
