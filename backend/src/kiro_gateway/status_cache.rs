//! Background status cache that periodically probes each Kiro account's usage
//! limits.
//!
//! A background task ([`spawn_status_refresher`]) polls every account on a
//! fixed interval, building a [`KiroStatusCacheSnapshot`] that the provider
//! consults to skip disabled or quota-exhausted accounts without making a
//! real upstream call.

use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::Arc,
    time::Duration,
};

use anyhow::{anyhow, Result};
use rand::Rng;
use serde::{Deserialize, Serialize};
use static_flow_shared::llm_gateway_store::now_ms;
use tokio::sync::watch;

use super::{
    auth_file::{resolve_auths_dir, KiroAuthRecord},
    runtime::KiroGatewayRuntimeState,
    types::{KiroBalanceView, KiroCacheView},
};

pub(crate) const STATUS_LOADING: &str = "loading";
pub(crate) const STATUS_READY: &str = "ready";
pub(crate) const STATUS_DEGRADED: &str = "degraded";
pub(crate) const STATUS_ERROR: &str = "error";
pub(crate) const STATUS_DISABLED: &str = "disabled";
pub(crate) const STATUS_EMPTY: &str = "empty";
pub(crate) const STATUS_QUOTA_EXHAUSTED: &str = "quota_exhausted";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestEligibilityBlockReason {
    MissingStatus,
    Disabled,
    QuotaExhausted,
    MinimumRemainingCreditsThreshold,
}

fn next_kiro_refresh_delay(config: &crate::state::LlmGatewayRuntimeConfig) -> Duration {
    let min_seconds = config
        .kiro_status_refresh_min_interval_seconds
        .min(config.kiro_status_refresh_max_interval_seconds);
    let max_seconds = config
        .kiro_status_refresh_min_interval_seconds
        .max(config.kiro_status_refresh_max_interval_seconds);
    let seconds = if min_seconds == max_seconds {
        min_seconds
    } else {
        rand::thread_rng().gen_range(min_seconds..=max_seconds)
    };
    Duration::from_secs(seconds)
}

fn next_kiro_account_jitter(config: &crate::state::LlmGatewayRuntimeConfig) -> Duration {
    let max_seconds = config.kiro_status_account_jitter_max_seconds;
    if max_seconds == 0 {
        Duration::ZERO
    } else {
        Duration::from_secs(rand::thread_rng().gen_range(0..=max_seconds))
    }
}

/// Cached status for a single Kiro account: last-known balance and cache
/// metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct KiroCachedAccountStatus {
    pub balance: Option<KiroBalanceView>,
    pub cache: KiroCacheView,
}

/// Point-in-time snapshot of all account statuses, with an aggregate health
/// indicator (`status`) derived from individual account states.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct KiroStatusCacheSnapshot {
    pub status: String,
    pub last_checked_at: Option<i64>,
    pub last_success_at: Option<i64>,
    pub error_message: Option<String>,
    pub accounts: HashMap<String, KiroCachedAccountStatus>,
}

impl Default for KiroStatusCacheSnapshot {
    fn default() -> Self {
        Self {
            status: STATUS_LOADING.to_string(),
            last_checked_at: None,
            last_success_at: None,
            error_message: None,
            accounts: HashMap::new(),
        }
    }
}

fn persisted_status_cache_path_from_dir(auths_dir: &Path) -> std::path::PathBuf {
    auths_dir.join(".status-cache").join("snapshot.json")
}

pub(crate) async fn load_persisted_status_cache() -> Result<KiroStatusCacheSnapshot> {
    load_persisted_status_cache_from_dir(&resolve_auths_dir()).await
}

async fn persist_status_cache(snapshot: &KiroStatusCacheSnapshot) -> Result<()> {
    persist_status_cache_to_dir(&resolve_auths_dir(), snapshot).await
}

pub(crate) async fn load_persisted_status_cache_from_dir(
    auths_dir: &Path,
) -> Result<KiroStatusCacheSnapshot> {
    let path = persisted_status_cache_path_from_dir(auths_dir);
    if !path.exists() {
        return Ok(KiroStatusCacheSnapshot::default());
    }
    let raw = tokio::fs::read_to_string(&path).await?;
    if raw.trim().is_empty() {
        return Ok(KiroStatusCacheSnapshot::default());
    }
    let snapshot = serde_json::from_str(&raw)?;
    Ok(snapshot)
}

pub(crate) async fn persist_status_cache_to_dir(
    auths_dir: &Path,
    snapshot: &KiroStatusCacheSnapshot,
) -> Result<()> {
    let path = persisted_status_cache_path_from_dir(auths_dir);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let content = serde_json::to_string_pretty(snapshot)?;
    tokio::fs::write(path, content).await?;
    Ok(())
}

async fn persist_status_cache_best_effort(snapshot: &KiroStatusCacheSnapshot) {
    if let Err(err) = persist_status_cache(snapshot).await {
        tracing::warn!("failed to persist kiro status cache snapshot: {err:#}");
    }
}

/// Spawn a background task that refreshes the status cache on a fixed interval,
/// stopping when `shutdown_rx` signals `true`.
pub(crate) fn spawn_status_refresher(
    runtime: Arc<KiroGatewayRuntimeState>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    tokio::spawn(async move {
        loop {
            let delay = {
                let config = runtime.runtime_config.read().clone();
                next_kiro_refresh_delay(&config)
            };
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        tracing::info!("kiro cached status refresher shutting down");
                        return;
                    }
                }
                _ = tokio::time::sleep(delay) => {
                    if let Err(err) = refresh_cached_status(&runtime).await {
                        tracing::warn!("failed to refresh cached kiro status: {err:#}");
                    }
                }
            }
        }
    });
}

/// Probe every known account's usage limits and rebuild the full snapshot.
pub(crate) async fn refresh_cached_status(runtime: &Arc<KiroGatewayRuntimeState>) -> Result<()> {
    let checked_at = now_ms();
    let auths = runtime.token_manager.list_auths().await?;
    let previous = runtime.status_cache.read().clone();
    let refresh_interval_seconds = runtime
        .runtime_config
        .read()
        .kiro_status_refresh_max_interval_seconds;
    let mut next = KiroStatusCacheSnapshot {
        status: STATUS_LOADING.to_string(),
        last_checked_at: Some(checked_at),
        last_success_at: previous.last_success_at,
        error_message: None,
        accounts: HashMap::with_capacity(auths.len()),
    };
    for (index, auth) in auths.into_iter().enumerate() {
        if index > 0 {
            let jitter = {
                let config = runtime.runtime_config.read().clone();
                next_kiro_account_jitter(&config)
            };
            if !jitter.is_zero() {
                tokio::time::sleep(jitter).await;
            }
        }
        let account_status =
            match refresh_cached_status_for_account(runtime, &auth.name, false).await {
                Ok(status) => status,
                Err(err) => {
                    tracing::warn!(
                        account_name = %auth.name,
                        error = %err,
                        "failed to refresh cached kiro status for account"
                    );
                    let prior = previous.accounts.get(&auth.name);
                    error_entry(prior, checked_at, err.to_string(), refresh_interval_seconds)
                },
            };
        next.accounts.insert(auth.name.clone(), account_status);
    }

    merge_newer_account_statuses(&mut next, &runtime.status_cache.read());
    match runtime.token_manager.list_auths().await {
        Ok(live_auths) => {
            let live_names = live_auths
                .into_iter()
                .map(|auth| auth.name)
                .collect::<HashSet<_>>();
            next.accounts
                .retain(|account_name, _| live_names.contains(account_name));
        },
        Err(err) => {
            tracing::warn!(
                "failed to reload kiro auth list before committing status cache: {err:#}"
            );
        },
    }
    let (ready_count, error_count) = refresh_snapshot_aggregate_metadata(&mut next);
    log_duplicate_upstream_identities(&next);

    tracing::info!(
        account_count = next.accounts.len(),
        ready_count,
        error_count,
        status = %next.status,
        "refreshed cached kiro status snapshot"
    );

    *runtime.status_cache.write() = next.clone();
    persist_status_cache_best_effort(&next).await;
    Ok(())
}

pub(crate) async fn ensure_cached_status_for_account(
    runtime: &Arc<KiroGatewayRuntimeState>,
    account_name: &str,
) -> Result<KiroCachedAccountStatus> {
    if let Some(entry) = runtime
        .status_cache
        .read()
        .accounts
        .get(account_name)
        .cloned()
    {
        return Ok(entry);
    }
    let refresh_lock = runtime.status_refresh_lock_for_account(account_name);
    let _guard = refresh_lock.lock().await;
    if let Some(entry) = runtime
        .status_cache
        .read()
        .accounts
        .get(account_name)
        .cloned()
    {
        return Ok(entry);
    }
    tracing::info!(
        account_name,
        "missing kiro account status cache entry; refreshing before request selection"
    );
    refresh_cached_status_for_account_locked(runtime, account_name, false).await
}

/// Refresh the cached status for a single account, optionally forcing a
/// token refresh, and update the global snapshot in place.
pub(crate) async fn refresh_cached_status_for_account(
    runtime: &Arc<KiroGatewayRuntimeState>,
    account_name: &str,
    force_refresh: bool,
) -> Result<KiroCachedAccountStatus> {
    let refresh_lock = runtime.status_refresh_lock_for_account(account_name);
    let _guard = refresh_lock.lock().await;
    refresh_cached_status_for_account_locked(runtime, account_name, force_refresh).await
}

async fn refresh_cached_status_for_account_locked(
    runtime: &Arc<KiroGatewayRuntimeState>,
    account_name: &str,
    force_refresh: bool,
) -> Result<KiroCachedAccountStatus> {
    let auth = runtime
        .token_manager
        .auth_by_name(account_name)
        .await?
        .ok_or_else(|| anyhow!("kiro account `{account_name}` not found"))?;
    let checked_at = now_ms();
    let previous = runtime.status_cache.read().clone();
    let prior = previous.accounts.get(account_name);
    let refresh_interval_seconds = runtime
        .runtime_config
        .read()
        .kiro_status_refresh_max_interval_seconds;

    let entry = if auth.disabled {
        disabled_entry(prior, checked_at, refresh_interval_seconds)
    } else {
        match runtime
            .token_manager
            .fetch_usage_limits_for_account(account_name, force_refresh)
            .await
        {
            Ok(usage) => ready_entry(&usage, checked_at, refresh_interval_seconds),
            Err(err) => {
                tracing::warn!(
                    account_name,
                    error = %err,
                    force_refresh,
                    "failed to refresh cached kiro status for account"
                );
                error_entry(prior, checked_at, err.to_string(), refresh_interval_seconds)
            },
        }
    };

    let mut snapshot = previous;
    snapshot.last_checked_at = Some(checked_at);
    if entry.cache.status == STATUS_READY {
        snapshot.last_success_at = Some(checked_at);
    }
    snapshot
        .accounts
        .insert(account_name.to_string(), entry.clone());
    let ready_count = snapshot
        .accounts
        .values()
        .filter(|status| status.cache.status == STATUS_READY)
        .count();
    let error_count = snapshot
        .accounts
        .values()
        .filter(|status| status_counts_as_problem(&status.cache.status))
        .count();
    apply_snapshot_summary(&mut snapshot, error_count, ready_count);
    log_duplicate_upstream_identities(&snapshot);
    *runtime.status_cache.write() = snapshot.clone();
    persist_status_cache_best_effort(&snapshot).await;

    tracing::info!(
        account_name,
        cache_status = %entry.cache.status,
        upstream_user_id = entry
            .balance
            .as_ref()
            .and_then(|balance| balance.user_id.as_deref())
            .unwrap_or("unknown"),
        "updated cached kiro status for account"
    );

    Ok(entry)
}

/// Remove a deleted account from the snapshot and recompute the aggregate.
pub(crate) async fn remove_cached_status_for_account(
    runtime: &Arc<KiroGatewayRuntimeState>,
    account_name: &str,
) {
    let refresh_lock = runtime.status_refresh_lock_for_account(account_name);
    let _guard = refresh_lock.lock().await;
    let snapshot = {
        let mut snapshot = runtime.status_cache.write();
        snapshot.accounts.remove(account_name);
        let ready_count = snapshot
            .accounts
            .values()
            .filter(|status| status.cache.status == STATUS_READY)
            .count();
        let error_count = snapshot
            .accounts
            .values()
            .filter(|status| status_counts_as_problem(&status.cache.status))
            .count();
        apply_snapshot_summary(&mut snapshot, error_count, ready_count);
        snapshot.clone()
    };
    persist_status_cache_best_effort(&snapshot).await;
}

/// Mark an account as quota-exhausted in the cache (e.g. after a 402 response),
/// zeroing its remaining balance so the provider skips it immediately.
pub(crate) async fn mark_account_quota_exhausted(
    runtime: &Arc<KiroGatewayRuntimeState>,
    account_name: &str,
    error_message: impl Into<String>,
) {
    let refresh_lock = runtime.status_refresh_lock_for_account(account_name);
    let _guard = refresh_lock.lock().await;
    let checked_at = now_ms();
    let error_message = error_message.into();
    let refresh_interval_seconds = runtime
        .runtime_config
        .read()
        .kiro_status_refresh_max_interval_seconds;
    let snapshot = {
        let mut snapshot = runtime.status_cache.write();
        let prior = snapshot.accounts.get(account_name).cloned();
        let entry = quota_exhausted_entry(
            prior.as_ref(),
            checked_at,
            error_message.clone(),
            refresh_interval_seconds,
        );
        snapshot.last_checked_at = Some(checked_at);
        snapshot.last_success_at = Some(checked_at);
        snapshot.accounts.insert(account_name.to_string(), entry);
        let ready_count = snapshot
            .accounts
            .values()
            .filter(|status| status.cache.status == STATUS_READY)
            .count();
        let error_count = snapshot
            .accounts
            .values()
            .filter(|status| status_counts_as_problem(&status.cache.status))
            .count();
        apply_snapshot_summary(&mut snapshot, error_count, ready_count);
        snapshot.clone()
    };
    persist_status_cache_best_effort(&snapshot).await;
    tracing::warn!(account_name, error_message, "marked cached kiro account as quota exhausted");
}

/// Determine whether an account should be considered for the next upstream
/// request based on its disabled flag, cached status, and remaining balance.
pub(crate) fn account_request_block_reason(
    auth: &KiroAuthRecord,
    entry: Option<&KiroCachedAccountStatus>,
) -> Option<RequestEligibilityBlockReason> {
    if auth.disabled {
        return Some(RequestEligibilityBlockReason::Disabled);
    }
    let Some(entry) = entry else {
        return Some(RequestEligibilityBlockReason::MissingStatus);
    };
    match entry.cache.status.as_str() {
        STATUS_DISABLED => return Some(RequestEligibilityBlockReason::Disabled),
        STATUS_QUOTA_EXHAUSTED => return Some(RequestEligibilityBlockReason::QuotaExhausted),
        _ => {},
    }
    let balance = entry.balance.as_ref()?;
    if balance.remaining <= 0.0 {
        return Some(RequestEligibilityBlockReason::QuotaExhausted);
    }
    if balance.remaining <= auth.effective_minimum_remaining_credits_before_block() {
        return Some(RequestEligibilityBlockReason::MinimumRemainingCreditsThreshold);
    }
    None
}

pub(crate) fn account_is_request_eligible(
    auth: &KiroAuthRecord,
    entry: Option<&KiroCachedAccountStatus>,
) -> bool {
    account_request_block_reason(auth, entry).is_none()
}

fn apply_snapshot_summary(
    snapshot: &mut KiroStatusCacheSnapshot,
    error_count: usize,
    ready_count: usize,
) {
    if snapshot.accounts.is_empty() {
        snapshot.status = STATUS_EMPTY.to_string();
        snapshot.error_message = None;
        return;
    }

    snapshot.status = if error_count == 0 {
        STATUS_READY.to_string()
    } else if ready_count > 0 {
        STATUS_DEGRADED.to_string()
    } else {
        STATUS_ERROR.to_string()
    };

    snapshot.error_message = if error_count == 0 { None } else { first_error_message(snapshot) };
}

fn merge_newer_account_statuses(
    snapshot: &mut KiroStatusCacheSnapshot,
    current: &KiroStatusCacheSnapshot,
) {
    for (account_name, current_status) in &current.accounts {
        let current_checked_at = current_status.cache.last_checked_at.unwrap_or(i64::MIN);
        let snapshot_checked_at = snapshot
            .accounts
            .get(account_name)
            .and_then(|status| status.cache.last_checked_at)
            .unwrap_or(i64::MIN);
        if current_checked_at >= snapshot_checked_at {
            snapshot
                .accounts
                .insert(account_name.clone(), current_status.clone());
        }
    }
}

fn refresh_snapshot_aggregate_metadata(snapshot: &mut KiroStatusCacheSnapshot) -> (usize, usize) {
    snapshot.last_checked_at = snapshot
        .accounts
        .values()
        .filter_map(|status| status.cache.last_checked_at)
        .max()
        .or(snapshot.last_checked_at);
    snapshot.last_success_at = snapshot
        .accounts
        .values()
        .filter_map(|status| status.cache.last_success_at)
        .max()
        .or(snapshot.last_success_at);
    let ready_count = snapshot
        .accounts
        .values()
        .filter(|status| status.cache.status == STATUS_READY)
        .count();
    let error_count = snapshot
        .accounts
        .values()
        .filter(|status| status_counts_as_problem(&status.cache.status))
        .count();
    apply_snapshot_summary(snapshot, error_count, ready_count);
    (ready_count, error_count)
}

fn first_error_message(snapshot: &KiroStatusCacheSnapshot) -> Option<String> {
    snapshot.accounts.values().find_map(|status| {
        status
            .cache
            .error_message
            .as_ref()
            .filter(|_| status_counts_as_problem(&status.cache.status))
            .cloned()
    })
}

fn status_counts_as_problem(status: &str) -> bool {
    matches!(status, STATUS_ERROR | STATUS_DEGRADED | STATUS_QUOTA_EXHAUSTED)
}

fn log_duplicate_upstream_identities(snapshot: &KiroStatusCacheSnapshot) {
    let mut grouped = HashMap::<String, Vec<String>>::new();
    for (account_name, status) in &snapshot.accounts {
        let Some(user_id) = status
            .balance
            .as_ref()
            .and_then(|balance| balance.user_id.as_ref())
        else {
            continue;
        };
        grouped
            .entry(user_id.clone())
            .or_default()
            .push(account_name.clone());
    }
    for (user_id, mut account_names) in grouped {
        if account_names.len() < 2 {
            continue;
        }
        account_names.sort();
        tracing::warn!(
            upstream_user_id = %user_id,
            account_names = ?account_names,
            "multiple kiro auth records resolved to the same upstream user identity"
        );
    }
}

fn ready_entry(
    usage: &super::wire::UsageLimitsResponse,
    checked_at: i64,
    refresh_interval_seconds: u64,
) -> KiroCachedAccountStatus {
    KiroCachedAccountStatus {
        balance: Some(KiroBalanceView::from_usage(usage)),
        cache: KiroCacheView {
            status: STATUS_READY.to_string(),
            refresh_interval_seconds,
            last_checked_at: Some(checked_at),
            last_success_at: Some(checked_at),
            error_message: None,
        },
    }
}

fn error_entry(
    prior: Option<&KiroCachedAccountStatus>,
    checked_at: i64,
    error_message: String,
    refresh_interval_seconds: u64,
) -> KiroCachedAccountStatus {
    let previous_balance = prior.and_then(|status| status.balance.clone());
    let previous_success_at = prior.and_then(|status| status.cache.last_success_at);
    let status = if previous_balance.is_some() { STATUS_DEGRADED } else { STATUS_ERROR };
    KiroCachedAccountStatus {
        balance: previous_balance,
        cache: KiroCacheView {
            status: status.to_string(),
            refresh_interval_seconds,
            last_checked_at: Some(checked_at),
            last_success_at: previous_success_at,
            error_message: Some(error_message),
        },
    }
}

fn disabled_entry(
    prior: Option<&KiroCachedAccountStatus>,
    checked_at: i64,
    refresh_interval_seconds: u64,
) -> KiroCachedAccountStatus {
    KiroCachedAccountStatus {
        balance: prior.and_then(|status| status.balance.clone()),
        cache: KiroCacheView {
            status: STATUS_DISABLED.to_string(),
            refresh_interval_seconds,
            last_checked_at: Some(checked_at),
            last_success_at: prior.and_then(|status| status.cache.last_success_at),
            error_message: None,
        },
    }
}

fn quota_exhausted_entry(
    prior: Option<&KiroCachedAccountStatus>,
    checked_at: i64,
    error_message: String,
    refresh_interval_seconds: u64,
) -> KiroCachedAccountStatus {
    let previous_balance = prior.and_then(|status| status.balance.clone());
    let previous_success_at = prior
        .and_then(|status| status.cache.last_success_at)
        .or(Some(checked_at));
    let balance = previous_balance.map(|mut balance| {
        balance.current_usage = balance.current_usage.max(balance.usage_limit);
        balance.remaining = 0.0;
        balance
    });
    KiroCachedAccountStatus {
        balance,
        cache: KiroCacheView {
            status: STATUS_QUOTA_EXHAUSTED.to_string(),
            refresh_interval_seconds,
            last_checked_at: Some(checked_at),
            last_success_at: previous_success_at,
            error_message: Some(error_message),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_entry_preserves_previous_balance_as_degraded() {
        let prior = KiroCachedAccountStatus {
            balance: Some(KiroBalanceView {
                current_usage: 10.0,
                usage_limit: 100.0,
                remaining: 90.0,
                next_reset_at: Some(123),
                subscription_title: Some("plan".to_string()),
                user_id: Some("user-1".to_string()),
            }),
            cache: KiroCacheView {
                status: STATUS_READY.to_string(),
                refresh_interval_seconds: 300,
                last_checked_at: Some(100),
                last_success_at: Some(100),
                error_message: None,
            },
        };

        let next = error_entry(Some(&prior), 200, "boom".to_string(), 300);
        assert_eq!(next.cache.status, STATUS_DEGRADED);
        assert_eq!(next.cache.last_success_at, Some(100));
        assert!(next.balance.is_some());
    }

    #[test]
    fn apply_snapshot_summary_marks_empty_snapshot() {
        let mut snapshot = KiroStatusCacheSnapshot::default();
        apply_snapshot_summary(&mut snapshot, 0, 0);
        assert_eq!(snapshot.status, STATUS_EMPTY);
    }

    #[test]
    fn quota_exhausted_entry_zeroes_remaining_balance() {
        let prior = KiroCachedAccountStatus {
            balance: Some(KiroBalanceView {
                current_usage: 55.0,
                usage_limit: 100.0,
                remaining: 45.0,
                next_reset_at: Some(123),
                subscription_title: Some("plan".to_string()),
                user_id: Some("user-1".to_string()),
            }),
            cache: KiroCacheView {
                status: STATUS_READY.to_string(),
                refresh_interval_seconds: 300,
                last_checked_at: Some(100),
                last_success_at: Some(100),
                error_message: None,
            },
        };

        let next = quota_exhausted_entry(Some(&prior), 200, "quota exhausted".to_string(), 300);
        assert_eq!(next.cache.status, STATUS_QUOTA_EXHAUSTED);
        assert_eq!(next.cache.last_success_at, Some(100));
        assert_eq!(next.balance.as_ref().map(|value| value.remaining), Some(0.0));
        assert_eq!(next.balance.as_ref().map(|value| value.current_usage), Some(100.0));
    }

    #[test]
    fn request_eligibility_skips_zero_remaining_balance() {
        let auth = KiroAuthRecord {
            name: "alpha".to_string(),
            disabled: false,
            ..KiroAuthRecord::default()
        };
        let status = KiroCachedAccountStatus {
            balance: Some(KiroBalanceView {
                current_usage: 100.0,
                usage_limit: 100.0,
                remaining: 0.0,
                next_reset_at: None,
                subscription_title: None,
                user_id: Some("user-1".to_string()),
            }),
            cache: KiroCacheView {
                status: STATUS_READY.to_string(),
                refresh_interval_seconds: 300,
                last_checked_at: Some(100),
                last_success_at: Some(100),
                error_message: None,
            },
        };

        assert!(!account_is_request_eligible(&auth, Some(&status)));
    }

    #[test]
    fn request_eligibility_requires_cached_account_status() {
        let auth = KiroAuthRecord {
            name: "alpha".to_string(),
            disabled: false,
            ..KiroAuthRecord::default()
        };

        assert_eq!(
            account_request_block_reason(&auth, None),
            Some(RequestEligibilityBlockReason::MissingStatus)
        );
        assert!(!account_is_request_eligible(&auth, None));
    }

    #[tokio::test]
    async fn persisted_status_cache_round_trips_quota_exhausted_entries() {
        let root = std::env::temp_dir().join(format!(
            "kiro-status-cache-round-trip-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time should be monotonic")
                .as_nanos()
        ));
        let snapshot = KiroStatusCacheSnapshot {
            status: STATUS_QUOTA_EXHAUSTED.to_string(),
            last_checked_at: Some(123),
            last_success_at: Some(120),
            error_message: Some("quota exhausted".to_string()),
            accounts: [("alpha".to_string(), KiroCachedAccountStatus {
                balance: Some(KiroBalanceView {
                    current_usage: 100.0,
                    usage_limit: 100.0,
                    remaining: 0.0,
                    next_reset_at: Some(999),
                    subscription_title: Some("Pro".to_string()),
                    user_id: Some("user-alpha".to_string()),
                }),
                cache: KiroCacheView {
                    status: STATUS_QUOTA_EXHAUSTED.to_string(),
                    refresh_interval_seconds: 300,
                    last_checked_at: Some(123),
                    last_success_at: Some(120),
                    error_message: Some("quota exhausted".to_string()),
                },
            })]
            .into_iter()
            .collect(),
        };

        persist_status_cache_to_dir(&root, &snapshot)
            .await
            .expect("persist status cache");
        let loaded = load_persisted_status_cache_from_dir(&root)
            .await
            .expect("load persisted status cache");

        let entry = loaded.accounts.get("alpha").expect("alpha entry");
        assert_eq!(loaded.status, STATUS_QUOTA_EXHAUSTED);
        assert_eq!(entry.cache.status, STATUS_QUOTA_EXHAUSTED);
        assert_eq!(entry.balance.as_ref().map(|value| value.remaining), Some(0.0));
        assert_eq!(
            entry
                .balance
                .as_ref()
                .and_then(|value| value.user_id.as_deref()),
            Some("user-alpha")
        );

        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[test]
    fn request_eligibility_keeps_degraded_accounts_retryable() {
        let auth = KiroAuthRecord {
            name: "alpha".to_string(),
            disabled: false,
            ..KiroAuthRecord::default()
        };
        let status = KiroCachedAccountStatus {
            balance: Some(KiroBalanceView {
                current_usage: 55.0,
                usage_limit: 100.0,
                remaining: 45.0,
                next_reset_at: Some(123),
                subscription_title: Some("plan".to_string()),
                user_id: Some("user-1".to_string()),
            }),
            cache: KiroCacheView {
                status: STATUS_DEGRADED.to_string(),
                refresh_interval_seconds: 300,
                last_checked_at: Some(100),
                last_success_at: Some(90),
                error_message: Some("temporary upstream failure".to_string()),
            },
        };

        assert!(
            account_is_request_eligible(&auth, Some(&status)),
            "transient degraded cache entries must stay eligible so the next refresh/request can \
             recover the account"
        );
    }

    #[test]
    fn request_eligibility_skips_account_below_configured_remaining_threshold() {
        let auth = KiroAuthRecord {
            name: "alpha".to_string(),
            disabled: false,
            minimum_remaining_credits_before_block: Some(10.0),
            ..KiroAuthRecord::default()
        };
        let status = KiroCachedAccountStatus {
            balance: Some(KiroBalanceView {
                current_usage: 92.5,
                usage_limit: 100.0,
                remaining: 7.5,
                next_reset_at: None,
                subscription_title: None,
                user_id: Some("user-1".to_string()),
            }),
            cache: KiroCacheView {
                status: STATUS_READY.to_string(),
                refresh_interval_seconds: 300,
                last_checked_at: Some(100),
                last_success_at: Some(100),
                error_message: None,
            },
        };

        assert!(!account_is_request_eligible(&auth, Some(&status)));
    }

    #[test]
    fn request_eligibility_keeps_account_above_configured_remaining_threshold() {
        let auth = KiroAuthRecord {
            name: "alpha".to_string(),
            disabled: false,
            minimum_remaining_credits_before_block: Some(10.0),
            ..KiroAuthRecord::default()
        };
        let status = KiroCachedAccountStatus {
            balance: Some(KiroBalanceView {
                current_usage: 88.0,
                usage_limit: 100.0,
                remaining: 12.0,
                next_reset_at: None,
                subscription_title: None,
                user_id: Some("user-1".to_string()),
            }),
            cache: KiroCacheView {
                status: STATUS_READY.to_string(),
                refresh_interval_seconds: 300,
                last_checked_at: Some(100),
                last_success_at: Some(100),
                error_message: None,
            },
        };

        assert!(account_is_request_eligible(&auth, Some(&status)));
    }

    #[test]
    fn kiro_refresh_interval_draw_uses_configured_bounds() {
        let config = crate::state::LlmGatewayRuntimeConfig {
            kiro_status_refresh_min_interval_seconds: 240,
            kiro_status_refresh_max_interval_seconds: 300,
            ..crate::state::LlmGatewayRuntimeConfig::default()
        };

        for _ in 0..64 {
            let value = next_kiro_refresh_delay(&config).as_secs();
            assert!((240..=300).contains(&value));
        }
    }
}
