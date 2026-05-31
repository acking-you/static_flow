//! Codex public status contracts: rate-limit status/bucket/window views,
//! public account status, and credit balances.

use serde::{Deserialize, Serialize};

/// Public read-only payload for the cached Codex rate-limit snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CodexRateLimitStatus {
    /// Snapshot status label.
    pub status: String,
    /// Suggested client refresh interval in seconds.
    pub refresh_interval_seconds: u64,
    /// Last refresh attempt timestamp in Unix milliseconds.
    pub last_checked_at: Option<i64>,
    /// Last successful refresh timestamp in Unix milliseconds.
    pub last_success_at: Option<i64>,
    /// Upstream source URL used for the status refresh.
    pub source_url: String,
    /// Last refresh error, if any.
    pub error_message: Option<String>,
    /// Per-account public summaries.
    #[serde(default)]
    pub accounts: Vec<CodexPublicAccountStatus>,
    /// Flattened rate-limit buckets across active accounts.
    pub buckets: Vec<CodexRateLimitBucket>,
}

impl CodexRateLimitStatus {
    /// Construct the same empty loading state used before the status cache
    /// warms.
    pub fn loading(refresh_interval_seconds: u64) -> Self {
        Self {
            status: "loading".to_string(),
            refresh_interval_seconds,
            last_checked_at: None,
            last_success_at: None,
            source_url: String::new(),
            error_message: None,
            accounts: Vec::new(),
            buckets: Vec::new(),
        }
    }
}

/// One public Codex account summary rendered on `/llm-access`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CodexPublicAccountStatus {
    /// Account display name.
    pub name: String,
    /// Runtime status label.
    pub status: String,
    /// Upstream plan type when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_type: Option<String>,
    /// Primary bucket remaining percentage when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_remaining_percent: Option<f64>,
    /// Secondary bucket remaining percentage when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secondary_remaining_percent: Option<f64>,
    /// Last usage refresh attempt timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_usage_checked_at: Option<i64>,
    /// Last successful usage refresh timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_usage_success_at: Option<i64>,
    /// Last usage refresh error, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_error_message: Option<String>,
}

/// One limit bucket rendered on the public status surface.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CodexRateLimitBucket {
    /// Upstream limit id.
    pub limit_id: String,
    /// Upstream limit name when available.
    pub limit_name: Option<String>,
    /// Human-readable bucket name.
    pub display_name: String,
    /// Whether this is the primary request bucket.
    pub is_primary: bool,
    /// Plan type attached to this bucket when known.
    pub plan_type: Option<String>,
    /// Primary rolling window.
    pub primary: Option<CodexRateLimitWindow>,
    /// Secondary rolling window.
    pub secondary: Option<CodexRateLimitWindow>,
    /// Credit metadata when upstream provides it.
    pub credits: Option<CodexCredits>,
    /// Account that owns this bucket in multi-account mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_name: Option<String>,
}

/// One usage window within a rate-limit bucket.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CodexRateLimitWindow {
    /// Used percentage.
    pub used_percent: f64,
    /// Remaining percentage.
    pub remaining_percent: f64,
    /// Window duration in minutes.
    pub window_duration_mins: Option<i64>,
    /// Reset timestamp in Unix milliseconds.
    pub resets_at: Option<i64>,
}

/// Credit metadata included in upstream usage payloads when available.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CodexCredits {
    /// Whether this bucket carries credit data.
    pub has_credits: bool,
    /// Whether the account reports unlimited credits.
    pub unlimited: bool,
    /// Printable balance value.
    pub balance: Option<String>,
}
