//! Admin API keys: the key view, paged listing/summary/query types, sort
//! modes, in-memory query filtering, and create/patch payloads.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{KEY_STATUS_ACTIVE, KEY_STATUS_DISABLED};

/// Admin-facing projection of one managed API key.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AdminKey {
    /// Key id.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Plaintext secret shown in admin UI.
    pub secret: String,
    /// SHA-256 secret hash.
    pub key_hash: String,
    /// Key status.
    pub status: String,
    /// Provider type.
    pub provider_type: String,
    /// Whether the key is visible on the public access page.
    pub public_visible: bool,
    /// Billable quota limit.
    pub quota_billable_limit: u64,
    /// Accumulated uncached input tokens.
    pub usage_input_uncached_tokens: u64,
    /// Accumulated cached input tokens.
    pub usage_input_cached_tokens: u64,
    /// Accumulated output tokens.
    pub usage_output_tokens: u64,
    /// Accumulated credit usage.
    pub usage_credit_total: f64,
    /// Number of events missing credit usage.
    pub usage_credit_missing_events: u64,
    /// Remaining billable tokens.
    pub remaining_billable: i64,
    /// Last usage timestamp.
    pub last_used_at: Option<i64>,
    /// Creation timestamp.
    pub created_at: i64,
    /// Update timestamp.
    pub updated_at: i64,
    /// Account route strategy.
    pub route_strategy: Option<String>,
    /// Account group id.
    pub account_group_id: Option<String>,
    /// Fixed account name.
    pub fixed_account_name: Option<String>,
    /// Auto account names.
    pub auto_account_names: Option<Vec<String>>,
    /// Model name mapping.
    pub model_name_map: Option<BTreeMap<String, String>>,
    /// Per-key request concurrency cap.
    pub request_max_concurrency: Option<u64>,
    /// Per-key request pacing interval.
    pub request_min_start_interval_ms: Option<u64>,
    /// Whether Codex fast/priority requests are allowed for this key.
    pub codex_fast_enabled: bool,
    /// Whether Kiro request validation is enabled.
    pub kiro_request_validation_enabled: bool,
    /// Whether Kiro cache estimation is enabled.
    pub kiro_cache_estimation_enabled: bool,
    /// Whether Kiro zero-cache diagnostics are enabled.
    pub kiro_zero_cache_debug_enabled: bool,
    /// Whether every Kiro request should retain full request payload
    /// diagnostics.
    pub kiro_full_request_logging_enabled: bool,
    /// Whether URL image/document sources should be fetched server-side and
    /// rewritten to inline Kiro media payloads.
    pub kiro_remote_media_resolution_enabled: bool,
    /// Whether recent first-token metrics may influence Kiro route ordering.
    pub kiro_latency_routing_enabled: bool,
    /// Kiro cache policy override JSON.
    pub kiro_cache_policy_override_json: Option<String>,
    /// Kiro billable multiplier override JSON.
    pub kiro_billable_model_multipliers_override_json: Option<String>,
    /// Effective Kiro cache policy JSON.
    pub effective_kiro_cache_policy_json: String,
    /// Whether the effective Kiro cache policy is global.
    pub uses_global_kiro_cache_policy: bool,
    /// Effective Kiro billable multiplier JSON.
    pub effective_kiro_billable_model_multipliers_json: String,
    /// Whether the effective billable multipliers are global.
    pub uses_global_kiro_billable_model_multipliers: bool,
    /// Admin-facing candidate-credit summary for Kiro routing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kiro_candidate_credit_summary: Option<AdminKiroKeyCandidateCreditSummary>,
}

/// Admin-facing candidate-credit summary for one Kiro key.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct AdminKiroKeyCandidateCreditSummary {
    /// Number of candidate accounts matched by the key route.
    pub candidate_count: usize,
    /// Number of candidate accounts with a loaded balance snapshot.
    pub loaded_balance_count: usize,
    /// Number of candidate accounts still missing a balance snapshot.
    pub missing_balance_count: usize,
    /// Sum of upstream credit limits across loaded candidate accounts.
    pub total_limit: f64,
    /// Sum of remaining upstream credits across loaded candidate accounts.
    pub total_remaining: f64,
}

/// Offset pagination request shared by admin list endpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdminPageRequest {
    /// Maximum number of rows to return.
    pub limit: usize,
    /// Number of rows to skip.
    pub offset: usize,
}

impl AdminPageRequest {
    /// Return true when at least one row remains after this page.
    pub fn has_more(self, returned: usize, total: usize) -> bool {
        self.offset.saturating_add(returned) < total
    }
}

/// Page of admin-managed API keys.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AdminKeysPage {
    /// Page rows.
    pub keys: Vec<AdminKey>,
    /// Full aggregate over all rows matching this page filter.
    pub summary: AdminKeysSummary,
    /// Total rows matching the query before pagination.
    pub total: usize,
    /// Page limit.
    pub limit: usize,
    /// Page offset.
    pub offset: usize,
    /// Whether another page is available.
    pub has_more: bool,
}

/// Full aggregate for admin-managed API keys.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct AdminKeysSummary {
    /// Total rows matching the provider filter.
    pub total: usize,
    /// Public-visible key count.
    pub public_visible_count: usize,
    /// Active key count.
    pub active_count: usize,
    /// Disabled key count.
    pub disabled_count: usize,
    /// Sum of configured billable quotas.
    pub quota_billable_limit_sum: u64,
    /// Sum of remaining billable quotas.
    pub remaining_billable_sum: i64,
    /// Sum of uncached input tokens.
    pub usage_input_uncached_tokens_sum: u64,
    /// Sum of cached input tokens.
    pub usage_input_cached_tokens_sum: u64,
    /// Sum of output tokens.
    pub usage_output_tokens_sum: u64,
    /// Sum of billable tokens.
    pub usage_billable_tokens_sum: u64,
    /// Sum of recorded credit usage.
    pub usage_credit_total: f64,
    /// Sum of events missing credit usage.
    pub usage_credit_missing_events: u64,
}

/// Admin key list query shared by paginated inventory screens.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdminKeyPageQuery {
    /// Optional case-insensitive search query.
    pub search: Option<String>,
    /// Whether disabled rows should be excluded.
    pub active_only: bool,
    /// Sort mode applied before pagination.
    pub sort: AdminKeySortMode,
}

/// Supported admin key list sort modes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AdminKeySortMode {
    /// Default created-at descending order.
    #[default]
    Newest,
    /// Remaining quota ascending.
    QuotaAsc,
    /// Remaining quota descending.
    QuotaDesc,
    /// Recorded credit usage ascending.
    UsageAsc,
    /// Recorded credit usage descending.
    UsageDesc,
}

pub fn summarize_admin_keys(keys: &[AdminKey]) -> AdminKeysSummary {
    let mut summary = AdminKeysSummary::default();
    for key in keys {
        summary.total += 1;
        if key.public_visible {
            summary.public_visible_count += 1;
        }
        match key.status.as_str() {
            KEY_STATUS_ACTIVE => summary.active_count += 1,
            KEY_STATUS_DISABLED => summary.disabled_count += 1,
            _ => {},
        }
        summary.quota_billable_limit_sum = summary
            .quota_billable_limit_sum
            .saturating_add(key.quota_billable_limit);
        summary.remaining_billable_sum = summary
            .remaining_billable_sum
            .saturating_add(key.remaining_billable);
        summary.usage_input_uncached_tokens_sum = summary
            .usage_input_uncached_tokens_sum
            .saturating_add(key.usage_input_uncached_tokens);
        summary.usage_input_cached_tokens_sum = summary
            .usage_input_cached_tokens_sum
            .saturating_add(key.usage_input_cached_tokens);
        summary.usage_output_tokens_sum = summary
            .usage_output_tokens_sum
            .saturating_add(key.usage_output_tokens);
        summary.usage_billable_tokens_sum = summary.usage_billable_tokens_sum.saturating_add(
            key.quota_billable_limit
                .saturating_sub(key.remaining_billable.max(0) as u64),
        );
        summary.usage_credit_total += key.usage_credit_total;
        summary.usage_credit_missing_events = summary
            .usage_credit_missing_events
            .saturating_add(key.usage_credit_missing_events);
    }
    summary
}

fn admin_key_matches_query(key: &AdminKey, query: &AdminKeyPageQuery) -> bool {
    if query.active_only && key.status == KEY_STATUS_DISABLED {
        return false;
    }
    let Some(search) = query
        .search
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return true;
    };
    let search = search.to_ascii_lowercase();
    key.id.to_ascii_lowercase().contains(&search)
        || key.name.to_ascii_lowercase().contains(&search)
        || key.provider_type.to_ascii_lowercase().contains(&search)
        || key.status.to_ascii_lowercase().contains(&search)
}

pub fn apply_admin_key_query(keys: &mut Vec<AdminKey>, query: &AdminKeyPageQuery) {
    keys.retain(|key| admin_key_matches_query(key, query));
    match query.sort {
        AdminKeySortMode::Newest => keys.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.id.cmp(&a.id))
        }),
        AdminKeySortMode::QuotaAsc => keys.sort_by_key(|key| key.remaining_billable),
        AdminKeySortMode::QuotaDesc => {
            keys.sort_by_key(|key| std::cmp::Reverse(key.remaining_billable));
        },
        AdminKeySortMode::UsageAsc => keys.sort_by(|a, b| {
            a.usage_credit_total
                .partial_cmp(&b.usage_credit_total)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.created_at.cmp(&a.created_at))
        }),
        AdminKeySortMode::UsageDesc => keys.sort_by(|a, b| {
            b.usage_credit_total
                .partial_cmp(&a.usage_credit_total)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.created_at.cmp(&a.created_at))
        }),
    }
}

/// New admin key row after request validation and secret generation.
#[derive(Debug, Clone, PartialEq)]
pub struct NewAdminKey {
    /// Key id.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Plaintext secret.
    pub secret: String,
    /// SHA-256 secret hash.
    pub key_hash: String,
    /// Provider type.
    pub provider_type: String,
    /// Protocol family.
    pub protocol_family: String,
    /// Whether the key is public-visible.
    pub public_visible: bool,
    /// Billable quota limit.
    pub quota_billable_limit: u64,
    /// Per-key request concurrency cap.
    pub request_max_concurrency: Option<u64>,
    /// Per-key request pacing interval.
    pub request_min_start_interval_ms: Option<u64>,
    /// Creation timestamp.
    pub created_at_ms: i64,
}

/// Admin key patch after request normalization.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AdminKeyPatch {
    /// New name.
    pub name: Option<String>,
    /// New status.
    pub status: Option<String>,
    /// New public visibility.
    pub public_visible: Option<bool>,
    /// New quota limit.
    pub quota_billable_limit: Option<u64>,
    /// New route strategy.
    pub route_strategy: Option<Option<String>>,
    /// New account group id.
    pub account_group_id: Option<Option<String>>,
    /// New fixed account name.
    pub fixed_account_name: Option<Option<String>>,
    /// New auto account list.
    pub auto_account_names: Option<Option<Vec<String>>>,
    /// New model name map.
    pub model_name_map: Option<Option<BTreeMap<String, String>>>,
    /// New per-key request concurrency cap.
    pub request_max_concurrency: Option<Option<u64>>,
    /// New per-key request pacing interval.
    pub request_min_start_interval_ms: Option<Option<u64>>,
    /// New Codex fast toggle.
    pub codex_fast_enabled: Option<bool>,
    /// New Kiro request-validation toggle.
    pub kiro_request_validation_enabled: Option<bool>,
    /// New Kiro cache-estimation toggle.
    pub kiro_cache_estimation_enabled: Option<bool>,
    /// New Kiro zero-cache diagnostic toggle.
    pub kiro_zero_cache_debug_enabled: Option<bool>,
    /// New Kiro full request logging toggle.
    pub kiro_full_request_logging_enabled: Option<bool>,
    /// New Kiro remote-media resolution toggle.
    pub kiro_remote_media_resolution_enabled: Option<bool>,
    /// New Kiro latency-routing toggle.
    pub kiro_latency_routing_enabled: Option<bool>,
    /// New Kiro cache policy override JSON.
    pub kiro_cache_policy_override_json: Option<Option<String>>,
    /// New Kiro billable model multiplier override JSON.
    pub kiro_billable_model_multipliers_override_json: Option<Option<String>>,
    /// Update timestamp.
    pub updated_at_ms: i64,
}
