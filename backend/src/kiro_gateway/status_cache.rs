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
pub(crate) use llm_access_kiro::status::{
    account_is_request_eligible, account_request_block_reason, KiroCachedAccountStatus,
    KiroStatusCacheSnapshot, RequestEligibilityBlockReason, STATUS_LOADING, STATUS_READY,
};
use llm_access_kiro::{
    config::KiroRuntimeConfig,
    status::{
        apply_snapshot_summary, disabled_status_entry as disabled_entry,
        duplicate_upstream_identities, error_status_entry as error_entry,
        load_persisted_status_cache_from_dir as load_persisted_status_cache_snapshot_from_dir,
        merge_newer_account_statuses,
        persist_status_cache_to_dir as persist_status_cache_snapshot_to_dir,
        quota_exhausted_status_entry as quota_exhausted_entry, ready_status_entry as ready_entry,
        refresh_snapshot_aggregate_metadata, status_counts_as_problem,
    },
};
use rand::Rng;
use static_flow_shared::llm_gateway_store::now_ms;
use tokio::sync::watch;

use super::{auth_file::resolve_auths_dir, runtime::KiroGatewayRuntimeState};

fn next_kiro_refresh_delay(config: &KiroRuntimeConfig) -> Duration {
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

fn next_kiro_account_jitter(config: &KiroRuntimeConfig) -> Duration {
    let max_seconds = config.kiro_status_account_jitter_max_seconds;
    if max_seconds == 0 {
        Duration::ZERO
    } else {
        Duration::from_secs(rand::thread_rng().gen_range(0..=max_seconds))
    }
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
    load_persisted_status_cache_snapshot_from_dir(auths_dir).await
}

pub(crate) async fn persist_status_cache_to_dir(
    auths_dir: &Path,
    snapshot: &KiroStatusCacheSnapshot,
) -> Result<()> {
    persist_status_cache_snapshot_to_dir(auths_dir, snapshot).await
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
                let config = runtime.runtime_config.snapshot();
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
        .snapshot()
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
                let config = runtime.runtime_config.snapshot();
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
        .snapshot()
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
        .snapshot()
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

fn log_duplicate_upstream_identities(snapshot: &KiroStatusCacheSnapshot) {
    for (user_id, account_names) in duplicate_upstream_identities(snapshot) {
        tracing::warn!(
            upstream_user_id = %user_id,
            account_names = ?account_names,
            "multiple kiro auth records resolved to the same upstream user identity"
        );
    }
}

#[cfg(test)]
mod tests {
    use llm_access_kiro::status::{
        KiroBalanceView, KiroCacheView, STATUS_DEGRADED, STATUS_EMPTY, STATUS_QUOTA_EXHAUSTED,
    };

    use super::{super::auth_file::KiroAuthRecord, *};

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
        let config = KiroRuntimeConfig {
            kiro_status_refresh_min_interval_seconds: 240,
            kiro_status_refresh_max_interval_seconds: 300,
            ..KiroRuntimeConfig::default()
        };

        for _ in 0..64 {
            let value = next_kiro_refresh_delay(&config).as_secs();
            assert!((240..=300).contains(&value));
        }
    }
}
