//! Kiro accounts: balance/cache views, account view, paged listing,
//! create/patch payloads, status-cache update, and refresh targets.

use serde::{Deserialize, Serialize};

use super::{
    codex_account::AdminAccountsSummary, DEFAULT_KIRO_STATUS_REFRESH_MAX_INTERVAL_SECONDS,
};

/// Admin-facing Kiro account balance snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AdminKiroBalanceView {
    /// Current upstream credit usage.
    pub current_usage: f64,
    /// Current upstream credit limit.
    pub usage_limit: f64,
    /// Remaining upstream credits.
    pub remaining: f64,
    /// Next reset timestamp in Unix milliseconds.
    pub next_reset_at: Option<i64>,
    /// Upstream subscription title.
    pub subscription_title: Option<String>,
    /// Upstream user id when the status API provides it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
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
