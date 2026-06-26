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
    /// Accumulated Codex image-generation tokens reported by upstream.
    #[serde(default)]
    pub codex_image_usage_tokens: u64,
    /// Number of successful Codex image responses missing upstream usage.
    #[serde(default)]
    pub codex_image_usage_missing_events: u64,
    /// Last successful Codex image usage timestamp.
    #[serde(default)]
    pub codex_image_last_used_at: Option<i64>,
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
    /// Preferred Kiro scheduler pool when route strategy is automatic.
    #[serde(default = "super::default_kiro_pool_strategy")]
    pub preferred_pool_strategy: String,
    /// Model name mapping.
    pub model_name_map: Option<BTreeMap<String, String>>,
    /// Per-key request concurrency cap.
    pub request_max_concurrency: Option<u64>,
    /// Per-key request pacing interval.
    pub request_min_start_interval_ms: Option<u64>,
    /// Whether Codex fast/priority requests are allowed for this key.
    pub codex_fast_enabled: bool,
    /// Whether repeated fatal Codex errors for the same session are rejected
    /// early for this key.
    #[serde(default)]
    pub codex_strict_session_rejection_enabled: bool,
    /// Whether Codex image generation/edit requests are enabled for this key.
    #[serde(default)]
    pub codex_image_generation_enabled: bool,
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
    /// Whether Kiro thinking signatures and encrypted content are validated.
    pub kiro_protected_content_validation_enabled: bool,
    /// Whether stable cctest text probes may bypass the normal Kiro path.
    pub kiro_cctest_text_handling_enabled: bool,
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
    /// Number of candidate accounts in the key's preferred scheduler pool.
    #[serde(default)]
    pub preferred_pool_candidate_count: usize,
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
    /// Sum of Codex image-generation tokens reported by upstream.
    pub codex_image_usage_tokens_sum: u64,
    /// Sum of successful Codex image responses missing upstream usage.
    pub codex_image_usage_missing_events: u64,
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
        summary.codex_image_usage_tokens_sum = summary
            .codex_image_usage_tokens_sum
            .saturating_add(key.codex_image_usage_tokens);
        summary.codex_image_usage_missing_events = summary
            .codex_image_usage_missing_events
            .saturating_add(key.codex_image_usage_missing_events);
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
    /// New preferred Kiro scheduler pool.
    pub preferred_pool_strategy: Option<String>,
    /// New model name map.
    pub model_name_map: Option<Option<BTreeMap<String, String>>>,
    /// New per-key request concurrency cap.
    pub request_max_concurrency: Option<Option<u64>>,
    /// New per-key request pacing interval.
    pub request_min_start_interval_ms: Option<Option<u64>>,
    /// New Codex fast toggle.
    pub codex_fast_enabled: Option<bool>,
    /// New Codex strict session-rejection toggle.
    pub codex_strict_session_rejection_enabled: Option<bool>,
    /// New Codex image generation/edit toggle.
    pub codex_image_generation_enabled: Option<bool>,
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
    /// New Kiro protected-content validation toggle.
    pub kiro_protected_content_validation_enabled: Option<bool>,
    /// New Kiro cctest text handling toggle.
    pub kiro_cctest_text_handling_enabled: Option<bool>,
    /// New Kiro cache policy override JSON.
    pub kiro_cache_policy_override_json: Option<Option<String>>,
    /// New Kiro billable model multiplier override JSON.
    pub kiro_billable_model_multipliers_override_json: Option<Option<String>>,
    /// Update timestamp.
    pub updated_at_ms: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn admin_key_with_codex_image_usage(id: &str, image_tokens: u64, missing: u64) -> AdminKey {
        AdminKey {
            id: id.to_string(),
            name: id.to_string(),
            secret: "secret".to_string(),
            key_hash: "hash".to_string(),
            status: KEY_STATUS_ACTIVE.to_string(),
            provider_type: "codex".to_string(),
            public_visible: true,
            quota_billable_limit: 1_000,
            usage_input_uncached_tokens: 0,
            usage_input_cached_tokens: 0,
            usage_output_tokens: 0,
            usage_credit_total: 0.0,
            usage_credit_missing_events: 0,
            codex_image_usage_tokens: image_tokens,
            codex_image_usage_missing_events: missing,
            codex_image_last_used_at: Some(1_700_000_000_000),
            remaining_billable: 1_000,
            last_used_at: None,
            created_at: 1,
            updated_at: 1,
            route_strategy: None,
            account_group_id: None,
            fixed_account_name: None,
            auto_account_names: None,
            preferred_pool_strategy: "balanced".to_string(),
            model_name_map: None,
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
            codex_fast_enabled: true,
            codex_strict_session_rejection_enabled: false,
            codex_image_generation_enabled: true,
            kiro_request_validation_enabled: false,
            kiro_cache_estimation_enabled: false,
            kiro_zero_cache_debug_enabled: false,
            kiro_full_request_logging_enabled: false,
            kiro_remote_media_resolution_enabled: false,
            kiro_latency_routing_enabled: true,
            kiro_protected_content_validation_enabled: false,
            kiro_cctest_text_handling_enabled: false,
            kiro_cache_policy_override_json: None,
            kiro_billable_model_multipliers_override_json: None,
            effective_kiro_cache_policy_json: "{}".to_string(),
            uses_global_kiro_cache_policy: true,
            effective_kiro_billable_model_multipliers_json: "{}".to_string(),
            uses_global_kiro_billable_model_multipliers: true,
            kiro_candidate_credit_summary: None,
        }
    }

    #[test]
    fn summarize_admin_keys_accumulates_codex_image_usage() {
        let keys = vec![
            admin_key_with_codex_image_usage("key-a", 10, 1),
            admin_key_with_codex_image_usage("key-b", 15, 2),
        ];

        let summary = summarize_admin_keys(&keys);

        assert_eq!(summary.codex_image_usage_tokens_sum, 25);
        assert_eq!(summary.codex_image_usage_missing_events, 3);
    }
}
