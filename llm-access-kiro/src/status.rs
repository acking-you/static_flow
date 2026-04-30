//! Kiro status and account-balance view contracts.

use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::{auth_file::KiroAuthRecord, wire::UsageLimitsResponse};

pub const STATUS_LOADING: &str = "loading";
pub const STATUS_READY: &str = "ready";
pub const STATUS_DEGRADED: &str = "degraded";
pub const STATUS_ERROR: &str = "error";
pub const STATUS_DISABLED: &str = "disabled";
pub const STATUS_EMPTY: &str = "empty";
pub const STATUS_QUOTA_EXHAUSTED: &str = "quota_exhausted";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestEligibilityBlockReason {
    MissingStatus,
    Disabled,
    QuotaExhausted,
    MinimumRemainingCreditsThreshold,
}

/// Cache status view for a single Kiro account's balance/auth probe.
///
/// Tracks when the background refresh last ran, whether it succeeded,
/// and any error that occurred.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct KiroCacheView {
    /// Human-readable cache state (e.g. `"fresh"`, `"stale"`, `"error"`).
    pub status: String,
    /// How often the background task refreshes this account, in seconds.
    pub refresh_interval_seconds: u64,
    /// Unix-epoch timestamp of the most recent probe attempt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_checked_at: Option<i64>,
    /// Unix-epoch timestamp of the most recent successful probe.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_success_at: Option<i64>,
    /// Error message from the last failed probe, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

/// Normalized account-balance snapshot derived from Kiro `getUsageLimits`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KiroBalanceView {
    pub current_usage: f64,
    pub usage_limit: f64,
    pub remaining: f64,
    pub next_reset_at: Option<i64>,
    pub subscription_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
}

impl KiroBalanceView {
    /// Convert the raw upstream usage-limit payload into the admin/public view
    /// shape used by StaticFlow and standalone llm-access.
    pub fn from_usage(usage: &UsageLimitsResponse) -> Self {
        let usage_limit = usage.usage_limit();
        let current_usage = usage.current_usage();
        Self {
            current_usage,
            usage_limit,
            remaining: (usage_limit - current_usage).max(0.0),
            next_reset_at: usage
                .usage_breakdown_list
                .first()
                .and_then(|item| item.next_date_reset.or(usage.next_date_reset))
                .map(|value| value as i64),
            subscription_title: usage.subscription_title().map(ToString::to_string),
            user_id: usage.user_id().map(ToString::to_string),
        }
    }
}

/// Cached status for a single Kiro account: last-known balance and cache
/// metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KiroCachedAccountStatus {
    pub balance: Option<KiroBalanceView>,
    pub cache: KiroCacheView,
}

/// Point-in-time snapshot of all account statuses, with an aggregate health
/// indicator (`status`) derived from individual account states.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KiroStatusCacheSnapshot {
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

pub fn persisted_status_cache_path_from_dir(auths_dir: &Path) -> PathBuf {
    auths_dir.join(".status-cache").join("snapshot.json")
}

pub async fn load_persisted_status_cache_from_dir(
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

pub async fn persist_status_cache_to_dir(
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

pub fn account_request_block_reason(
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

pub fn account_is_request_eligible(
    auth: &KiroAuthRecord,
    entry: Option<&KiroCachedAccountStatus>,
) -> bool {
    account_request_block_reason(auth, entry).is_none()
}

pub fn apply_snapshot_summary(
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

pub fn merge_newer_account_statuses(
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

pub fn refresh_snapshot_aggregate_metadata(
    snapshot: &mut KiroStatusCacheSnapshot,
) -> (usize, usize) {
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

pub fn status_counts_as_problem(status: &str) -> bool {
    matches!(status, STATUS_ERROR | STATUS_DEGRADED | STATUS_QUOTA_EXHAUSTED)
}

pub fn duplicate_upstream_identities(
    snapshot: &KiroStatusCacheSnapshot,
) -> Vec<(String, Vec<String>)> {
    let mut grouped = BTreeMap::<String, Vec<String>>::new();
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
    grouped
        .into_iter()
        .filter_map(|(user_id, mut account_names)| {
            if account_names.len() < 2 {
                return None;
            }
            account_names.sort();
            Some((user_id, account_names))
        })
        .collect()
}

pub fn ready_status_entry(
    usage: &UsageLimitsResponse,
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

pub fn error_status_entry(
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

pub fn disabled_status_entry(
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

pub fn quota_exhausted_status_entry(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        auth_file::KiroAuthRecord,
        wire::{
            Bonus, FreeTrialInfo, SubscriptionInfo, UsageBreakdown, UsageLimitsResponse, UserInfo,
        },
    };

    #[test]
    fn balance_view_converts_usage_limits_with_active_bonuses() {
        let usage = UsageLimitsResponse {
            next_date_reset: Some(900.0),
            subscription_info: Some(SubscriptionInfo {
                subscription_title: Some("Pro".to_string()),
            }),
            usage_breakdown_list: vec![UsageBreakdown {
                current_usage_with_precision: 10.0,
                bonuses: vec![
                    Bonus {
                        current_usage: 2.0,
                        usage_limit: 20.0,
                        status: Some("ACTIVE".to_string()),
                    },
                    Bonus {
                        current_usage: 99.0,
                        usage_limit: 999.0,
                        status: Some("EXPIRED".to_string()),
                    },
                ],
                free_trial_info: Some(FreeTrialInfo {
                    current_usage_with_precision: 3.0,
                    free_trial_status: Some("ACTIVE".to_string()),
                    usage_limit_with_precision: 30.0,
                }),
                next_date_reset: Some(800.0),
                usage_limit_with_precision: 100.0,
            }],
            user_info: Some(UserInfo {
                user_id: Some("user-1".to_string()),
            }),
        };

        let balance = KiroBalanceView::from_usage(&usage);

        assert_eq!(balance.current_usage, 15.0);
        assert_eq!(balance.usage_limit, 150.0);
        assert_eq!(balance.remaining, 135.0);
        assert_eq!(balance.next_reset_at, Some(800));
        assert_eq!(balance.subscription_title.as_deref(), Some("Pro"));
        assert_eq!(balance.user_id.as_deref(), Some("user-1"));
    }

    #[test]
    fn cache_view_defaults_to_empty_status() {
        let cache = KiroCacheView::default();

        assert!(cache.status.is_empty());
        assert_eq!(cache.refresh_interval_seconds, 0);
        assert_eq!(cache.last_checked_at, None);
        assert_eq!(cache.last_success_at, None);
        assert_eq!(cache.error_message, None);
    }

    #[test]
    fn quota_exhausted_status_entry_zeroes_remaining_balance() {
        let prior = cached_account_with_balance(55.0, 100.0, 45.0, STATUS_READY);

        let next =
            quota_exhausted_status_entry(Some(&prior), 200, "quota exhausted".to_string(), 300);

        assert_eq!(next.cache.status, STATUS_QUOTA_EXHAUSTED);
        assert_eq!(next.cache.last_success_at, Some(100));
        assert_eq!(next.balance.as_ref().map(|value| value.remaining), Some(0.0));
        assert_eq!(next.balance.as_ref().map(|value| value.current_usage), Some(100.0));
    }

    #[test]
    fn request_block_reason_requires_cached_account_status() {
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

    #[test]
    fn request_block_reason_respects_remaining_threshold() {
        let auth = KiroAuthRecord {
            name: "alpha".to_string(),
            disabled: false,
            minimum_remaining_credits_before_block: Some(10.0),
            ..KiroAuthRecord::default()
        };
        let status = cached_account_with_balance(92.5, 100.0, 7.5, STATUS_READY);

        assert_eq!(
            account_request_block_reason(&auth, Some(&status)),
            Some(RequestEligibilityBlockReason::MinimumRemainingCreditsThreshold)
        );
    }

    #[test]
    fn snapshot_metadata_marks_mixed_ready_and_error_as_degraded() {
        let mut snapshot = KiroStatusCacheSnapshot {
            accounts: [
                ("alpha".to_string(), cached_account_with_balance(1.0, 100.0, 99.0, STATUS_READY)),
                ("beta".to_string(), cached_error_account("boom")),
            ]
            .into_iter()
            .collect(),
            ..KiroStatusCacheSnapshot::default()
        };

        let counts = refresh_snapshot_aggregate_metadata(&mut snapshot);

        assert_eq!(counts, (1, 1));
        assert_eq!(snapshot.status, STATUS_DEGRADED);
        assert_eq!(snapshot.error_message.as_deref(), Some("boom"));
    }

    #[test]
    fn duplicate_upstream_identities_groups_account_names() {
        let mut first = cached_account_with_balance(1.0, 100.0, 99.0, STATUS_READY);
        first.balance.as_mut().expect("balance").user_id = Some("same-user".to_string());
        let mut second = cached_account_with_balance(2.0, 100.0, 98.0, STATUS_READY);
        second.balance.as_mut().expect("balance").user_id = Some("same-user".to_string());
        let snapshot = KiroStatusCacheSnapshot {
            accounts: [("beta".to_string(), second), ("alpha".to_string(), first)]
                .into_iter()
                .collect(),
            ..KiroStatusCacheSnapshot::default()
        };

        assert_eq!(duplicate_upstream_identities(&snapshot), vec![(
            "same-user".to_string(),
            vec!["alpha".to_string(), "beta".to_string()]
        )]);
    }

    fn cached_account_with_balance(
        current_usage: f64,
        usage_limit: f64,
        remaining: f64,
        status: &str,
    ) -> KiroCachedAccountStatus {
        KiroCachedAccountStatus {
            balance: Some(KiroBalanceView {
                current_usage,
                usage_limit,
                remaining,
                next_reset_at: Some(123),
                subscription_title: Some("plan".to_string()),
                user_id: Some("user-1".to_string()),
            }),
            cache: KiroCacheView {
                status: status.to_string(),
                refresh_interval_seconds: 300,
                last_checked_at: Some(100),
                last_success_at: Some(100),
                error_message: None,
            },
        }
    }

    fn cached_error_account(message: &str) -> KiroCachedAccountStatus {
        KiroCachedAccountStatus {
            balance: None,
            cache: KiroCacheView {
                status: STATUS_ERROR.to_string(),
                refresh_interval_seconds: 300,
                last_checked_at: Some(100),
                last_success_at: None,
                error_message: Some(message.to_string()),
            },
        }
    }
}
