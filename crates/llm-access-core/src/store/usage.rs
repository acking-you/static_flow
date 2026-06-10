//! Usage analytics contracts: event source/status enums, event query/page/
//! totals, chart points, the legacy Kiro-proxy migration record, and the
//! usage-metrics + Kiro-latency-ranking query/view/snapshot types.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::proxy::AdminProxyConfig;
use crate::usage::UsageEvent;

/// Aggregated control-plane usage delta for one key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeyUsageRollupDelta {
    /// Key receiving this rollup delta.
    pub key_id: String,
    /// Uncached input tokens to add.
    pub input_uncached_tokens: i64,
    /// Cached input tokens to add.
    pub input_cached_tokens: i64,
    /// Output tokens to add.
    pub output_tokens: i64,
    /// Billable tokens to add.
    pub billable_tokens: i64,
    /// Credit usage to add.
    pub credit_total: f64,
    /// Count of events whose credit usage was missing.
    pub credit_missing_events: i64,
    /// Latest usage timestamp represented by this delta.
    pub last_used_at_ms: Option<i64>,
}

impl KeyUsageRollupDelta {
    /// Build a single-key rollup delta from one raw usage event.
    pub fn from_usage_event(event: &UsageEvent) -> anyhow::Result<Self> {
        let (credit_total, credit_missing_events) = match event.credit_usage.as_deref() {
            Some(raw) => match raw.parse::<f64>() {
                Ok(value) if value.is_finite() => (value, event.credit_usage_missing as i64),
                _ => (0.0, 1),
            },
            None => (0.0, event.credit_usage_missing as i64),
        };
        Ok(Self {
            key_id: event.key_id.clone(),
            input_uncached_tokens: event.input_uncached_tokens.max(0),
            input_cached_tokens: event.input_cached_tokens.max(0),
            output_tokens: event.output_tokens.max(0),
            billable_tokens: event.billable_tokens.max(0),
            credit_total,
            credit_missing_events,
            last_used_at_ms: Some(event.created_at_ms),
        })
    }

    /// Add another delta for the same key.
    pub fn add_assign(&mut self, other: &Self) {
        self.input_uncached_tokens = self
            .input_uncached_tokens
            .saturating_add(other.input_uncached_tokens);
        self.input_cached_tokens = self
            .input_cached_tokens
            .saturating_add(other.input_cached_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.billable_tokens = self.billable_tokens.saturating_add(other.billable_tokens);
        self.credit_total += other.credit_total;
        self.credit_missing_events = self
            .credit_missing_events
            .saturating_add(other.credit_missing_events);
        self.last_used_at_ms = match (self.last_used_at_ms, other.last_used_at_ms) {
            (Some(current), Some(next)) => Some(current.max(next)),
            (None, next) => next,
            (current, None) => current,
        };
    }

    /// Subtract another delta after it has been durably applied.
    pub fn subtract_assign(&mut self, other: &Self) {
        self.input_uncached_tokens = self
            .input_uncached_tokens
            .saturating_sub(other.input_uncached_tokens);
        self.input_cached_tokens = self
            .input_cached_tokens
            .saturating_sub(other.input_cached_tokens);
        self.output_tokens = self.output_tokens.saturating_sub(other.output_tokens);
        self.billable_tokens = self.billable_tokens.saturating_sub(other.billable_tokens);
        self.credit_total = (self.credit_total - other.credit_total).max(0.0);
        self.credit_missing_events = self
            .credit_missing_events
            .saturating_sub(other.credit_missing_events);
        if self.last_used_at_ms == other.last_used_at_ms {
            self.last_used_at_ms = None;
        }
    }

    /// Whether all additive counters in this delta are zero.
    pub fn is_zero(&self) -> bool {
        self.input_uncached_tokens == 0
            && self.input_cached_tokens == 0
            && self.output_tokens == 0
            && self.billable_tokens == 0
            && self.credit_total == 0.0
            && self.credit_missing_events == 0
    }
}

/// Exact last-used timestamp cardinality represented by one rollup batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyUsageRollupLastUsedCount {
    /// Key receiving this timestamp contribution.
    pub key_id: String,
    /// Usage timestamp in Unix milliseconds.
    pub last_used_at_ms: i64,
    /// Number of raw events for this key at this timestamp.
    pub count: u64,
}

/// One durable, idempotently applied control-plane rollup batch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageRollupBatch {
    /// Stable id used by control stores for replay deduplication.
    pub batch_id: String,
    /// Optional source node id for diagnostics.
    pub source_node_id: Option<String>,
    /// Batch creation timestamp in Unix milliseconds.
    pub created_at_ms: i64,
    /// Number of raw usage events represented by this aggregated batch.
    pub source_event_count: u64,
    /// Per-key rollup deltas.
    pub deltas: Vec<KeyUsageRollupDelta>,
    /// Per-key timestamp cardinalities used to update in-memory quota overlays
    /// exactly when an aggregated batch is replayed or applied later.
    #[serde(default)]
    pub last_used_at_ms_counts: Vec<KeyUsageRollupLastUsedCount>,
}

impl UsageRollupBatch {
    /// Aggregate raw usage events into one idempotent rollup batch.
    pub fn from_usage_events(
        batch_id: String,
        source_node_id: Option<String>,
        created_at_ms: i64,
        events: &[UsageEvent],
    ) -> anyhow::Result<Self> {
        let mut deltas = BTreeMap::<String, KeyUsageRollupDelta>::new();
        let mut last_used_counts = BTreeMap::<(String, i64), u64>::new();
        for event in events {
            let delta = KeyUsageRollupDelta::from_usage_event(event)?;
            if let Some(last_used_at_ms) = delta.last_used_at_ms {
                let key = (delta.key_id.clone(), last_used_at_ms);
                let count = last_used_counts.entry(key).or_insert(0);
                *count = count.saturating_add(1);
            }
            deltas
                .entry(delta.key_id.clone())
                .and_modify(|current| current.add_assign(&delta))
                .or_insert(delta);
        }
        Ok(Self {
            batch_id,
            source_node_id,
            created_at_ms,
            source_event_count: events.len() as u64,
            deltas: deltas.into_values().collect(),
            last_used_at_ms_counts: last_used_counts
                .into_iter()
                .map(|((key_id, last_used_at_ms), count)| KeyUsageRollupLastUsedCount {
                    key_id,
                    last_used_at_ms,
                    count,
                })
                .collect(),
        })
    }

    /// Return true if this batch has no deltas to apply.
    pub fn is_empty(&self) -> bool {
        self.deltas.is_empty()
    }
}

/// Summary returned by idempotent rollup batch sinks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsageRollupApplyReport {
    /// Batches newly applied in this call.
    pub applied_batch_count: usize,
    /// Batches already recorded and therefore skipped.
    pub already_applied_batch_count: usize,
    /// Delta rows newly considered for application.
    pub delta_count: usize,
    /// Delta rows skipped because their key no longer exists.
    pub missing_key_delta_count: usize,
}

/// A rollup batch id was replayed with different content.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("usage rollup batch id `{batch_id}` was replayed with a different digest")]
pub struct UsageRollupDigestMismatch {
    /// Conflicting rollup batch id.
    pub batch_id: String,
}

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

#[cfg(test)]
mod tests {
    use super::UsageRollupBatch;
    use crate::{
        provider::{ProtocolFamily, ProviderType},
        usage::{UsageEvent, UsageStreamDetails, UsageTiming},
    };

    #[test]
    fn rollup_treats_malformed_credit_as_missing_without_dropping_batch() {
        let events = vec![
            test_usage_event("evt-bad-credit", 100, Some("not-a-number"), false),
            test_usage_event("evt-good-credit", 200, Some("0.5"), false),
        ];

        let batch = UsageRollupBatch::from_usage_events(
            "batch-credit".to_string(),
            Some("node-test".to_string()),
            300,
            &events,
        )
        .expect("aggregate usage rollup");

        assert_eq!(batch.deltas.len(), 1);
        let delta = &batch.deltas[0];
        assert_eq!(delta.credit_total, 0.5);
        assert_eq!(delta.credit_missing_events, 1);
        assert_eq!(delta.last_used_at_ms, Some(200));
        assert_eq!(batch.last_used_at_ms_counts.len(), 2);
        assert_eq!(batch.last_used_at_ms_counts[0].last_used_at_ms, 100);
        assert_eq!(batch.last_used_at_ms_counts[1].last_used_at_ms, 200);
    }

    #[test]
    fn rollup_treats_non_finite_credit_as_missing() {
        let event = test_usage_event("evt-nan-credit", 100, Some("NaN"), false);

        let delta = super::KeyUsageRollupDelta::from_usage_event(&event).expect("usage delta");

        assert_eq!(delta.credit_total, 0.0);
        assert_eq!(delta.credit_missing_events, 1);
    }

    fn test_usage_event(
        event_id: &str,
        created_at_ms: i64,
        credit_usage: Option<&str>,
        credit_usage_missing: bool,
    ) -> UsageEvent {
        UsageEvent {
            event_id: event_id.to_string(),
            created_at_ms,
            provider_type: ProviderType::Kiro,
            protocol_family: ProtocolFamily::Anthropic,
            key_id: "key-usage".to_string(),
            key_name: "runtime".to_string(),
            account_name: Some("acct-1".to_string()),
            account_group_id_at_event: None,
            route_strategy_at_event: None,
            request_method: "POST".to_string(),
            request_url: "/v1/messages".to_string(),
            endpoint: "/v1/messages".to_string(),
            model: Some("claude".to_string()),
            mapped_model: Some("claude".to_string()),
            status_code: 200,
            request_body_bytes: Some(1),
            quota_failover_count: 0,
            routing_diagnostics_json: None,
            input_uncached_tokens: 10,
            input_cached_tokens: 1,
            output_tokens: 2,
            billable_tokens: 12,
            credit_usage: credit_usage.map(str::to_string),
            usage_missing: false,
            credit_usage_missing,
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: "{}".to_string(),
            last_message_content: None,
            client_request_body_json: None,
            upstream_request_body_json: None,
            full_request_json: None,
            error_message: None,
            error_body: None,
            response_body: None,
            timing: UsageTiming::default(),
            stream: UsageStreamDetails::default(),
        }
    }
}
