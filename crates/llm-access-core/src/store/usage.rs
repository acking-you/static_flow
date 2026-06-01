//! Usage analytics contracts: event source/status enums, event query/page/
//! totals, chart points, the legacy Kiro-proxy migration record, and the
//! usage-metrics + Kiro-latency-ranking query/view/snapshot types.

use serde::{Deserialize, Serialize};

use super::proxy::AdminProxyConfig;
use crate::usage::UsageEvent;

/// Physical usage-event source queried by admin and public compatibility views.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum UsageEventSource {
    /// Query the currently writable active usage store.
    Hot,
    /// Query immutable archived usage segments.
    Archive,
    /// Query both active and archived usage data.
    #[default]
    All,
}

impl UsageEventSource {
    /// Parse a user-facing query value.
    pub fn from_query_value(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "hot" => Some(Self::Hot),
            "archive" | "archived" => Some(Self::Archive),
            "all" => Some(Self::All),
            _ => None,
        }
    }

    /// Whether active usage rows should be consulted.
    pub fn includes_hot(self) -> bool {
        matches!(self, Self::Hot | Self::All)
    }

    /// Whether archived usage rows should be consulted.
    pub fn includes_archive(self) -> bool {
        matches!(self, Self::Archive | Self::All)
    }
}

/// Paginated usage-event query used by admin and public compatibility views.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageEventStatusKind {
    /// Only requests that completed with HTTP 200.
    Ok,
    /// Any non-200 request, regardless of the exact status code.
    NonOk,
}

impl UsageEventStatusKind {
    /// Parse the public query-string value into a typed status bucket.
    pub fn from_query_value(value: &str) -> Option<Self> {
        match value {
            "ok" => Some(Self::Ok),
            "non_ok" => Some(Self::NonOk),
            _ => None,
        }
    }

    /// Stable query-string value used by HTTP handlers and tests.
    pub fn as_query_value(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::NonOk => "non_ok",
        }
    }
}

/// Paginated usage-event query used by admin and public compatibility views.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageEventQuery {
    /// Optional key filter.
    pub key_id: Option<String>,
    /// Optional provider filter.
    pub provider_type: Option<String>,
    /// Optional model filter.
    pub model: Option<String>,
    /// Optional account filter.
    pub account_name: Option<String>,
    /// Optional endpoint filter.
    pub endpoint: Option<String>,
    /// Optional status code filter.
    pub status_code: Option<i32>,
    /// Optional status bucket filter for common "200 vs non-200" analysis.
    pub status_kind: Option<UsageEventStatusKind>,
    /// Physical usage event source.
    pub source: UsageEventSource,
    /// Optional inclusive lower creation timestamp bound in Unix milliseconds.
    pub start_ms: Option<i64>,
    /// Optional exclusive upper creation timestamp bound in Unix milliseconds.
    pub end_ms: Option<i64>,
    /// Page limit.
    pub limit: usize,
    /// Page offset.
    pub offset: usize,
}

/// Aggregate totals over a filtered usage-event result set.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UsageEventTotals {
    /// Count of matching events.
    pub event_count: usize,
    /// Sum of uncached input tokens across all matches.
    pub input_uncached_tokens: u64,
    /// Sum of cached input tokens across all matches.
    pub input_cached_tokens: u64,
    /// Sum of output tokens across all matches.
    pub output_tokens: u64,
    /// Sum of billable tokens across all matches.
    pub billable_tokens: u64,
}

/// Usage-event page returned by the analytics store.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageEventPage {
    /// Total matching rows.
    pub total: usize,
    /// Page offset.
    pub offset: usize,
    /// Page limit.
    pub limit: usize,
    /// Whether more rows remain after this page.
    pub has_more: bool,
    /// Aggregate totals over the full filtered result set.
    pub totals: UsageEventTotals,
    /// Usage events in newest-first order.
    pub events: Vec<UsageEvent>,
}

/// One public usage chart bucket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageChartPoint {
    /// Bucket start timestamp in Unix milliseconds.
    pub bucket_start_ms: i64,
    /// Token total for the bucket.
    pub tokens: u64,
}

/// Result of migrating legacy embedded Kiro proxy fields into shared proxy
/// configs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminLegacyKiroProxyMigration {
    /// Proxy configs created during migration.
    pub created_configs: Vec<AdminProxyConfig>,
    /// Existing proxy configs reused by matching legacy tuples.
    pub reused_configs: Vec<AdminProxyConfig>,
    /// Kiro account names updated by migration.
    pub migrated_account_names: Vec<String>,
}

/// Distinct values available for usage filter autocomplete.
#[derive(Debug, Clone, Default)]
pub struct UsageFilterOptions {
    /// Distinct model names.
    pub models: Vec<String>,
    /// Distinct account names.
    pub accounts: Vec<String>,
    /// Distinct endpoint values.
    pub endpoints: Vec<String>,
}

/// Query for recent operational monitoring metrics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageMetricsQuery {
    /// Optional provider filter.
    pub provider_type: Option<String>,
    /// Physical usage-event source.
    pub source: UsageEventSource,
    /// Inclusive lower timestamp bound in Unix milliseconds.
    pub start_ms: i64,
    /// Exclusive upper timestamp bound in Unix milliseconds.
    pub end_ms: i64,
    /// Per-section top-N limit.
    pub top_limit: usize,
}

/// High-level aggregate over one monitoring window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct UsageMetricsSummary {
    /// Total requests in the window.
    pub total_requests: u64,
    /// Successful HTTP 200 requests.
    pub ok_requests: u64,
    /// Non-200 requests.
    pub non_ok_requests: u64,
    /// Distinct account count observed in the window.
    pub distinct_accounts: usize,
    /// Distinct proxy identities observed in the window.
    pub distinct_proxies: usize,
    /// Samples carrying first-token timing.
    pub first_token_samples: u64,
    /// Average first-token latency in milliseconds.
    pub avg_first_token_ms: Option<f64>,
    /// Maximum first-token latency in milliseconds.
    pub max_first_token_ms: Option<i64>,
    /// Average end-to-end latency in milliseconds.
    pub avg_latency_ms: Option<f64>,
    /// Average routing wait in milliseconds.
    pub avg_routing_wait_ms: Option<f64>,
    /// Requests that experienced at least one quota failover.
    pub failover_request_count: u64,
    /// Total quota-failover count across all requests.
    pub total_quota_failovers: u64,
    /// Downstream disconnect count.
    pub downstream_disconnect_count: u64,
    /// Requests missing regular token usage.
    pub usage_missing_count: u64,
    /// Requests missing credit usage.
    pub credit_usage_missing_count: u64,
}

/// One grouped monitoring row for accounts or proxies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct UsageMetricsDimensionView {
    /// Stable grouping key.
    pub key: String,
    /// Human-readable label.
    pub label: String,
    /// Account name when this row represents an account.
    pub account_name: Option<String>,
    /// Proxy config id when known.
    pub proxy_config_id: Option<String>,
    /// Proxy config name when known.
    pub proxy_config_name: Option<String>,
    /// Proxy URL when known.
    pub proxy_url: Option<String>,
    /// Proxy source (`fixed`, `binding`, `none`, ...).
    pub proxy_source: Option<String>,
    /// Total requests in this group.
    pub request_count: u64,
    /// Successful HTTP 200 requests.
    pub ok_count: u64,
    /// Non-200 requests.
    pub non_ok_count: u64,
    /// Samples carrying first-token timing.
    pub first_token_samples: u64,
    /// Average first-token latency in milliseconds.
    pub avg_first_token_ms: Option<f64>,
    /// Maximum first-token latency in milliseconds.
    pub max_first_token_ms: Option<i64>,
    /// Samples carrying routing-wait timing.
    pub routing_wait_samples: u64,
    /// Average routing-wait latency in milliseconds.
    pub avg_routing_wait_ms: Option<f64>,
    /// Maximum routing-wait latency in milliseconds.
    pub max_routing_wait_ms: Option<i64>,
    /// Requests that experienced quota failover.
    pub failover_request_count: u64,
    /// Sum of quota-failover counts.
    pub total_quota_failovers: u64,
    /// Downstream disconnect count.
    pub downstream_disconnect_count: u64,
    /// Requests missing regular token usage.
    pub usage_missing_count: u64,
    /// Requests missing credit usage.
    pub credit_usage_missing_count: u64,
}

/// One non-OK status-code aggregate row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct UsageMetricsStatusCodeView {
    /// HTTP status code.
    pub status_code: i32,
    /// Number of matching requests.
    pub request_count: u64,
}

/// Full monitoring snapshot for one recent window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct UsageMetricsSnapshot {
    /// Generation timestamp in Unix milliseconds.
    pub generated_at_ms: i64,
    /// Inclusive lower timestamp bound in Unix milliseconds.
    pub start_ms: i64,
    /// Exclusive upper timestamp bound in Unix milliseconds.
    pub end_ms: i64,
    /// Effective provider filter.
    pub provider_type: Option<String>,
    /// Effective usage source.
    pub source: UsageEventSource,
    /// Window-level summary.
    pub summary: UsageMetricsSummary,
    /// Highest first-token-latency accounts.
    pub top_first_token_accounts: Vec<UsageMetricsDimensionView>,
    /// Highest first-token-latency proxies.
    pub top_first_token_proxies: Vec<UsageMetricsDimensionView>,
    /// Largest non-OK account distributions.
    pub top_non_ok_accounts: Vec<UsageMetricsDimensionView>,
    /// Largest non-OK proxy distributions.
    pub top_non_ok_proxies: Vec<UsageMetricsDimensionView>,
    /// Highest routing-wait accounts.
    pub top_routing_wait_accounts: Vec<UsageMetricsDimensionView>,
    /// Highest routing-wait proxies.
    pub top_routing_wait_proxies: Vec<UsageMetricsDimensionView>,
    /// Largest quota-failover account distributions.
    pub top_failover_accounts: Vec<UsageMetricsDimensionView>,
    /// Largest quota-failover proxy distributions.
    pub top_failover_proxies: Vec<UsageMetricsDimensionView>,
    /// Largest downstream-disconnect account distributions.
    pub top_disconnect_accounts: Vec<UsageMetricsDimensionView>,
    /// Largest downstream-disconnect proxy distributions.
    pub top_disconnect_proxies: Vec<UsageMetricsDimensionView>,
    /// Non-OK status-code distribution.
    pub non_ok_status_codes: Vec<UsageMetricsStatusCodeView>,
}

/// Query for API-side Kiro latency routing weights.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KiroLatencyRankingQuery {
    /// Physical usage-event source.
    pub source: UsageEventSource,
    /// Inclusive lower timestamp bound in Unix milliseconds.
    pub start_ms: i64,
    /// Exclusive upper timestamp bound in Unix milliseconds.
    pub end_ms: i64,
}

/// One latency row grouped by Kiro account or proxy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct KiroLatencyRankingRow {
    /// Stable grouping key.
    pub key: String,
    /// Human-readable label.
    pub label: String,
    /// Account name when this row represents an account.
    pub account_name: Option<String>,
    /// Proxy config id when known.
    pub proxy_config_id: Option<String>,
    /// Proxy config name when known.
    pub proxy_config_name: Option<String>,
    /// Proxy URL when known.
    pub proxy_url: Option<String>,
    /// Proxy source (`fixed`, `binding`, `none`, ...).
    pub proxy_source: Option<String>,
    /// Samples carrying first-token timing.
    pub first_token_samples: u64,
    /// Average first-token latency in milliseconds.
    pub avg_first_token_ms: Option<f64>,
    /// Maximum first-token latency in milliseconds.
    pub max_first_token_ms: Option<i64>,
}

/// Compact Kiro latency routing snapshot for one recent window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct KiroLatencyRankingSnapshot {
    /// Generation timestamp in Unix milliseconds.
    pub generated_at_ms: i64,
    /// Inclusive lower timestamp bound in Unix milliseconds.
    pub start_ms: i64,
    /// Exclusive upper timestamp bound in Unix milliseconds.
    pub end_ms: i64,
    /// Effective usage source.
    pub source: UsageEventSource,
    /// Total first-token samples across the snapshot.
    pub first_token_samples: u64,
    /// Global average first-token latency in milliseconds.
    pub avg_first_token_ms: Option<f64>,
    /// Account latency rows. This is intentionally full, not top-N.
    pub accounts: Vec<KiroLatencyRankingRow>,
    /// Proxy latency rows. This is intentionally full, not top-N.
    pub proxies: Vec<KiroLatencyRankingRow>,
}
