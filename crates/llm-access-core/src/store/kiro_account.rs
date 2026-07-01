//! Kiro accounts: balance/cache views, account view, paged listing,
//! create/patch payloads, status-cache update, and refresh targets.

use serde::{Deserialize, Serialize};

use super::{
    codex_account::AdminAccountsSummary, DEFAULT_KIRO_STATUS_REFRESH_MAX_INTERVAL_SECONDS,
    KEY_STATUS_ACTIVE,
};

/// Kiro account filter kind matching every non-normal account state.
pub const ADMIN_KIRO_ACCOUNT_ISSUE_ABNORMAL: &str = "abnormal";
/// Kiro account issue kind for expired or rejected upstream credentials.
pub const ADMIN_KIRO_ACCOUNT_ISSUE_AUTH_401: &str = "auth_401";
/// Kiro account issue kind for non-auth account or cache errors.
pub const ADMIN_KIRO_ACCOUNT_ISSUE_ERROR: &str = "error";
/// Kiro account issue kind for disabled or otherwise inactive accounts.
pub const ADMIN_KIRO_ACCOUNT_ISSUE_DISABLED: &str = "disabled";

/// Admin-facing Kiro account issue classification.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminKiroAccountIssue {
    /// Stable issue kind used by API filters and UI badges.
    pub kind: String,
    /// Human-readable source error.
    pub summary: String,
    /// Timestamp associated with the source error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at_ms: Option<i64>,
}

/// Page-level filters for admin Kiro account listings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdminKiroAccountPageQuery {
    /// Case-insensitive account-name prefix.
    pub prefix: Option<String>,
    /// Case-insensitive free-text search across account identity and errors.
    pub q: Option<String>,
    /// Stable issue kind filter.
    pub issue: Option<String>,
}

/// Classify operator-actionable Kiro account errors.
pub fn classify_admin_kiro_account_issue(
    status: &str,
    disabled: bool,
    disabled_reason: Option<&str>,
    account_error: Option<&str>,
    cache_error: Option<&str>,
    issue_at_ms: Option<i64>,
) -> Option<AdminKiroAccountIssue> {
    let messages = [account_error, cache_error, disabled_reason];
    if let Some(message) = messages
        .into_iter()
        .filter_map(non_empty_message)
        .find(|message| kiro_error_is_auth_401(message))
    {
        return Some(AdminKiroAccountIssue {
            kind: ADMIN_KIRO_ACCOUNT_ISSUE_AUTH_401.to_string(),
            summary: message.to_string(),
            at_ms: issue_at_ms,
        });
    }
    if let Some(message) = [account_error, cache_error]
        .into_iter()
        .filter_map(non_empty_message)
        .next()
    {
        return Some(AdminKiroAccountIssue {
            kind: ADMIN_KIRO_ACCOUNT_ISSUE_ERROR.to_string(),
            summary: message.to_string(),
            at_ms: issue_at_ms,
        });
    }
    if disabled || status != KEY_STATUS_ACTIVE {
        return Some(AdminKiroAccountIssue {
            kind: ADMIN_KIRO_ACCOUNT_ISSUE_DISABLED.to_string(),
            summary: non_empty_message(disabled_reason)
                .map(str::to_string)
                .unwrap_or_else(|| format!("account status is {status}")),
            at_ms: None,
        });
    }
    None
}

fn non_empty_message(message: Option<&str>) -> Option<&str> {
    message.map(str::trim).filter(|value| !value.is_empty())
}

fn kiro_error_is_auth_401(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("401") || lower.contains("unauthorized")
}

/// Admin-facing Kiro account balance snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AdminKiroBalanceView {
    /// Current upstream credit usage.
    pub current_usage: f64,
    /// Current upstream credit limit.
    pub usage_limit: f64,
    /// Remaining upstream credits.
    pub remaining: f64,
    /// Original upstream credit limit before any admin manual calibration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_usage_limit: Option<f64>,
    /// Admin-calibrated credit limit used for routing and display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_usage_limit: Option<f64>,
    /// Next reset timestamp in Unix milliseconds.
    pub next_reset_at: Option<i64>,
    /// Upstream subscription title.
    pub subscription_title: Option<String>,
    /// Upstream user id when the status API provides it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
}

impl AdminKiroBalanceView {
    /// Apply an optional admin-calibrated limit while preserving trusted
    /// upstream usage. Passing `None` restores a previously calibrated balance
    /// when the original upstream limit is known.
    pub fn with_manual_usage_limit(mut self, manual_usage_limit: Option<f64>) -> Self {
        let Some(limit) = manual_usage_limit
            .filter(|value| value.is_finite())
            .map(|value| value.max(0.0))
        else {
            if let Some(upstream_limit) = self.upstream_usage_limit.take() {
                self.usage_limit = upstream_limit;
                self.remaining = (upstream_limit - self.current_usage).max(0.0);
            }
            self.manual_usage_limit = None;
            return self;
        };
        let upstream_limit = self.upstream_usage_limit.unwrap_or(self.usage_limit);
        self.upstream_usage_limit = Some(upstream_limit);
        self.manual_usage_limit = Some(limit);
        self.usage_limit = limit;
        self.remaining = (limit - self.current_usage).max(0.0);
        self
    }
}

/// Admin-facing Kiro status-cache metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminKiroCacheView {
    /// Cache status label.
    pub status: String,
    /// Expected refresh interval in seconds.
    pub refresh_interval_seconds: u64,
    /// Last status-check attempt timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_checked_at: Option<i64>,
    /// Last successful status-check timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_success_at: Option<i64>,
    /// Last status-check error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

impl Default for AdminKiroCacheView {
    fn default() -> Self {
        Self {
            status: "loading".to_string(),
            refresh_interval_seconds: DEFAULT_KIRO_STATUS_REFRESH_MAX_INTERVAL_SECONDS,
            last_checked_at: None,
            last_success_at: None,
            error_message: None,
        }
    }
}

/// Admin-facing projection of one configured Kiro account.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AdminKiroAccount {
    /// Account display name.
    pub name: String,
    /// Kiro auth method.
    pub auth_method: String,
    /// Identity provider label.
    pub provider: Option<String>,
    /// Upstream user id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_user_id: Option<String>,
    /// Account email when known.
    pub email: Option<String>,
    /// Access token expiry string.
    pub expires_at: Option<String>,
    /// Kiro profile ARN.
    pub profile_arn: Option<String>,
    /// Whether a refresh token is available.
    pub has_refresh_token: bool,
    /// Whether this account is disabled.
    pub disabled: bool,
    /// Disable/error reason.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled_reason: Option<String>,
    /// Stable issue kind when the account has an actionable error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_kind: Option<String>,
    /// Source error for the actionable issue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_summary: Option<String>,
    /// Timestamp associated with the actionable issue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_at_ms: Option<i64>,
    /// Import source label.
    pub source: Option<String>,
    /// Import source DB path.
    pub source_db_path: Option<String>,
    /// Last import timestamp.
    pub last_imported_at: Option<i64>,
    /// Subscription title.
    pub subscription_title: Option<String>,
    /// Default region.
    pub region: Option<String>,
    /// Auth region.
    pub auth_region: Option<String>,
    /// API region.
    pub api_region: Option<String>,
    /// Machine id.
    pub machine_id: Option<String>,
    /// Per-account request concurrency cap.
    pub kiro_channel_max_concurrency: u64,
    /// Per-account request pacing interval.
    pub kiro_channel_min_start_interval_ms: u64,
    /// Cached-credit floor used before blocking the account locally.
    pub minimum_remaining_credits_before_block: f64,
    /// Optional admin-calibrated account credit limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_usage_limit: Option<f64>,
    /// Scheduler pool this account belongs to.
    #[serde(default = "super::default_kiro_pool_strategy")]
    pub pool_strategy: String,
    /// Account proxy mode.
    pub proxy_mode: String,
    /// Fixed proxy config id.
    pub proxy_config_id: Option<String>,
    /// Effective proxy source.
    pub effective_proxy_source: String,
    /// Effective proxy URL.
    pub effective_proxy_url: Option<String>,
    /// Effective proxy config name.
    pub effective_proxy_config_name: Option<String>,
    /// Legacy embedded proxy URL if present.
    pub proxy_url: Option<String>,
    /// Cached balance snapshot.
    pub balance: Option<AdminKiroBalanceView>,
    /// Cached status metadata.
    pub cache: AdminKiroCacheView,
}

/// Page of admin Kiro accounts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AdminKiroAccountsPage {
    /// Page rows.
    pub accounts: Vec<AdminKiroAccount>,
    /// Full aggregate over all Kiro accounts.
    pub summary: AdminAccountsSummary,
    /// Total rows matching the query before pagination.
    pub total: usize,
    /// Page limit.
    pub limit: usize,
    /// Page offset.
    pub offset: usize,
    /// Whether another page is available.
    pub has_more: bool,
}

/// New persisted Kiro account row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAdminKiroAccount {
    /// Account display name.
    pub name: String,
    /// Kiro auth method.
    pub auth_method: String,
    /// Upstream account id when known.
    pub account_id: Option<String>,
    /// Kiro profile ARN when known.
    pub profile_arn: Option<String>,
    /// Upstream user id when known.
    pub user_id: Option<String>,
    /// Runtime account status.
    pub status: String,
    /// Persisted auth payload JSON.
    pub auth_json: String,
    /// Per-account request concurrency cap.
    pub max_concurrency: Option<u64>,
    /// Per-account request pacing interval.
    pub min_start_interval_ms: Option<u64>,
    /// Fixed proxy config id when configured.
    pub proxy_config_id: Option<String>,
    /// Creation timestamp.
    pub created_at_ms: i64,
}

/// Patch for mutable Kiro account routing/scheduler settings.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AdminKiroAccountPatch {
    /// New runtime status.
    pub status: Option<String>,
    /// New per-account request concurrency cap.
    pub max_concurrency: Option<u64>,
    /// New per-account request pacing interval.
    pub min_start_interval_ms: Option<u64>,
    /// New cached-credit floor.
    pub minimum_remaining_credits_before_block: Option<f64>,
    /// New admin-calibrated account credit limit; `Some(None)` clears it.
    pub manual_usage_limit: Option<Option<f64>>,
    /// New scheduler-pool strategy.
    pub pool_strategy: Option<String>,
    /// New account proxy mode.
    pub proxy_mode: Option<String>,
    /// New fixed proxy config id.
    pub proxy_config_id: Option<Option<String>>,
    /// Update timestamp.
    pub updated_at_ms: i64,
}

/// Cached Kiro account status update produced by a balance refresh.
#[derive(Debug, Clone, PartialEq)]
pub struct AdminKiroStatusCacheUpdate {
    /// Account name.
    pub account_name: String,
    /// Cached balance payload.
    pub balance: Option<AdminKiroBalanceView>,
    /// Cache metadata.
    pub cache: AdminKiroCacheView,
    /// Refresh timestamp.
    pub refreshed_at_ms: i64,
    /// Expiration timestamp.
    pub expires_at_ms: i64,
    /// Last refresh error.
    pub last_error: Option<String>,
}

/// Minimal Kiro account projection used by background status refresh.
#[derive(Debug, Clone, PartialEq)]
pub struct KiroStatusRefreshTarget {
    /// Account display name.
    pub name: String,
    /// Whether refresh should be skipped and persisted as disabled.
    pub disabled: bool,
    /// Cached status metadata used when preserving disabled state.
    pub cache: AdminKiroCacheView,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn balance(current_usage: f64, usage_limit: f64) -> AdminKiroBalanceView {
        AdminKiroBalanceView {
            current_usage,
            usage_limit,
            remaining: (usage_limit - current_usage).max(0.0),
            next_reset_at: None,
            subscription_title: Some("KIRO PRO".to_string()),
            user_id: Some("user-1".to_string()),
            upstream_usage_limit: None,
            manual_usage_limit: None,
        }
    }

    #[test]
    fn manual_usage_limit_recomputes_remaining_from_trusted_current_usage() {
        let calibrated = balance(120.0, 1_000.0).with_manual_usage_limit(Some(500.0));

        assert_eq!(calibrated.current_usage, 120.0);
        assert_eq!(calibrated.usage_limit, 500.0);
        assert_eq!(calibrated.remaining, 380.0);
        assert_eq!(calibrated.upstream_usage_limit, Some(1_000.0));
        assert_eq!(calibrated.manual_usage_limit, Some(500.0));
    }

    #[test]
    fn manual_usage_limit_clamps_remaining_when_current_usage_exceeds_limit() {
        let calibrated = balance(700.0, 1_000.0).with_manual_usage_limit(Some(500.0));

        assert_eq!(calibrated.current_usage, 700.0);
        assert_eq!(calibrated.usage_limit, 500.0);
        assert_eq!(calibrated.remaining, 0.0);
        assert_eq!(calibrated.upstream_usage_limit, Some(1_000.0));
        assert_eq!(calibrated.manual_usage_limit, Some(500.0));
    }

    #[test]
    fn absent_manual_usage_limit_preserves_upstream_balance() {
        let calibrated = balance(120.0, 1_000.0).with_manual_usage_limit(None);

        assert_eq!(calibrated.current_usage, 120.0);
        assert_eq!(calibrated.usage_limit, 1_000.0);
        assert_eq!(calibrated.remaining, 880.0);
        assert_eq!(calibrated.upstream_usage_limit, None);
        assert_eq!(calibrated.manual_usage_limit, None);
    }

    #[test]
    fn classifies_kiro_unauthorized_errors_as_auth_401_issue() {
        let issue = classify_admin_kiro_account_issue(
            "active",
            false,
            None,
            Some("upstream status refresh failed"),
            Some("Kiro status API returned 401 Unauthorized"),
            Some(123_456),
        )
        .expect("401 error should be classified");

        assert_eq!(issue.kind, ADMIN_KIRO_ACCOUNT_ISSUE_AUTH_401);
        assert_eq!(issue.summary, "Kiro status API returned 401 Unauthorized");
        assert_eq!(issue.at_ms, Some(123_456));
    }

    #[test]
    fn classifies_non_auth_errors_as_error_issue() {
        let issue = classify_admin_kiro_account_issue(
            "active",
            false,
            None,
            Some("upstream status refresh failed"),
            Some("temporary upstream timeout"),
            Some(123_456),
        )
        .expect("non-auth error should be classified");

        assert_eq!(issue.kind, ADMIN_KIRO_ACCOUNT_ISSUE_ERROR);
        assert_eq!(issue.summary, "upstream status refresh failed");
        assert_eq!(issue.at_ms, Some(123_456));
    }

    #[test]
    fn classifies_disabled_accounts_as_disabled_issue() {
        let issue = classify_admin_kiro_account_issue(
            "disabled",
            true,
            Some("manually disabled"),
            None,
            None,
            None,
        )
        .expect("disabled account should be classified");

        assert_eq!(issue.kind, ADMIN_KIRO_ACCOUNT_ISSUE_DISABLED);
        assert_eq!(issue.summary, "manually disabled");
        assert_eq!(issue.at_ms, None);
    }

    #[test]
    fn leaves_active_clean_accounts_unclassified() {
        assert_eq!(
            classify_admin_kiro_account_issue("active", false, None, None, None, Some(123_456),),
            None
        );
    }
}
