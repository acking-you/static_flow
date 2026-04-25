//! Response and admin request types for the Kiro gateway surfaces.
//!
//! These structs back both the public `/api/kiro-gateway/*` endpoints and the
//! private `/admin/kiro-gateway/*` management API.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};
use static_flow_shared::llm_gateway_store::{
    KiroCachePolicy, LlmGatewayAccountGroupRecord, LlmGatewayKeyRecord, LlmGatewayUsageEventRecord,
    LlmGatewayUsageEventSummaryRecord,
};

use super::{auth_file::KiroAuthRecord, wire::UsageLimitsResponse};

/// Public access bundle returned by the Kiro gateway status endpoint.
///
/// Contains the gateway connection info, cache TTL, and a summary of all
/// registered accounts with their current balance/status.
#[derive(Debug, Serialize)]
pub struct KiroAccessResponse {
    /// Root URL of the Kiro gateway (e.g. `https://host:port`).
    pub base_url: String,
    /// URL path prefix for proxied requests (e.g. `/kiro/v1`).
    pub gateway_path: String,
    /// How long the auth cache is considered fresh, in seconds.
    pub auth_cache_ttl_seconds: u64,
    /// Per-account public status snapshots.
    pub accounts: Vec<KiroPublicStatusView>,
    /// Unix-epoch timestamp (seconds) when this response was generated.
    pub generated_at: i64,
}

/// Cache status view for a single Kiro account's balance/auth probe.
///
/// Tracks when the background refresh last ran, whether it succeeded,
/// and any error that occurred.
#[derive(Debug, Clone, Serialize, Default)]
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

/// Public-facing status snapshot for one Kiro account.
///
/// Exposed on the unauthenticated access endpoint so callers can see
/// account availability, remaining quota, and cache health.
#[derive(Debug, Serialize)]
pub struct KiroPublicStatusView {
    /// Display name of the account.
    pub name: String,
    /// Upstream provider identifier (e.g. `"anthropic"`, `"bedrock"`).
    pub provider: Option<String>,
    /// Admin-set kill switch; `true` means the account is disabled.
    pub disabled: bool,
    /// Optional structured explanation for why the account is disabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled_reason: Option<String>,
    /// Subscription tier label from the upstream balance API.
    pub subscription_title: Option<String>,
    /// Credits already consumed in the current billing period.
    pub current_usage: Option<f64>,
    /// Credit limit for the current billing period.
    pub usage_limit: Option<f64>,
    /// Remaining credits (`usage_limit - current_usage`, floored at 0).
    pub remaining: Option<f64>,
    /// Unix-epoch timestamp when the billing period resets.
    pub next_reset_at: Option<i64>,
    /// Live cache status for this account's balance probe.
    pub cache: KiroCacheView,
}

/// Admin-facing view of a single Kiro gateway API key.
///
/// Includes the full secret, token-level usage counters, remaining billable
/// quota, and the routing strategy that controls which upstream account
/// handles requests made with this key.
#[derive(Debug, Serialize)]
pub struct AdminKiroKeyView {
    /// Unique key identifier (UUID).
    pub id: String,
    /// Human-readable label for the key.
    pub name: String,
    /// Raw API key secret (shown only in admin responses).
    pub secret: String,
    /// SHA-256 hash of the secret, used for fast lookup on incoming requests.
    pub key_hash: String,
    /// Lifecycle status (e.g. `"active"`, `"revoked"`).
    pub status: String,
    /// Whether this key appears in the public access endpoint.
    pub public_visible: bool,
    /// Maximum billable tokens allowed before the key is throttled.
    pub quota_billable_limit: u64,
    /// Cumulative input tokens that were not served from cache.
    pub usage_input_uncached_tokens: u64,
    /// Cumulative input tokens served from prompt cache.
    pub usage_input_cached_tokens: u64,
    /// Cumulative output (completion) tokens.
    pub usage_output_tokens: u64,
    /// Exact cumulative Kiro credits consumed when upstream metering was
    /// present.
    pub usage_credit_total: f64,
    /// Number of requests for this key whose credit metering was unavailable.
    pub usage_credit_missing_events: u64,
    /// Remaining billable token budget (`quota - used`; can be negative).
    pub remaining_billable: i64,
    /// Unix-epoch timestamp of the last request through this key.
    pub last_used_at: Option<i64>,
    /// Unix-epoch timestamp when the key was created.
    pub created_at: i64,
    /// Unix-epoch timestamp of the last metadata update.
    pub updated_at: i64,
    /// Routing strategy: `"fixed"`, `"auto"`, or `None` (default round-robin).
    pub route_strategy: Option<String>,
    /// Reusable provider-scoped account-pool group selected by this key.
    pub account_group_id: Option<String>,
    /// Account name used when `route_strategy` is `"fixed"`.
    pub fixed_account_name: Option<String>,
    /// Candidate account names when `route_strategy` is `"auto"`.
    pub auto_account_names: Option<Vec<String>>,
    /// Optional per-key rewrite map from requested public model name to the
    /// actual model name forwarded upstream.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_name_map: Option<BTreeMap<String, String>>,
    pub kiro_request_validation_enabled: bool,
    pub kiro_cache_estimation_enabled: bool,
    pub kiro_zero_cache_debug_enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kiro_cache_policy_override_json: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kiro_billable_model_multipliers_override_json: Option<String>,
    pub effective_kiro_cache_policy_json: String,
    pub uses_global_kiro_cache_policy: bool,
    pub effective_kiro_billable_model_multipliers_json: String,
    pub uses_global_kiro_billable_model_multipliers: bool,
}

impl AdminKiroKeyView {
    pub fn from_key_and_effective_policy(
        value: &LlmGatewayKeyRecord,
        effective_policy: &KiroCachePolicy,
        uses_global_kiro_cache_policy: bool,
        effective_billable_model_multipliers: &BTreeMap<String, f64>,
        uses_global_kiro_billable_model_multipliers: bool,
    ) -> Self {
        Self {
            id: value.id.clone(),
            name: value.name.clone(),
            secret: value.secret.clone(),
            key_hash: value.key_hash.clone(),
            status: value.status.clone(),
            public_visible: value.public_visible,
            quota_billable_limit: value.quota_billable_limit,
            usage_input_uncached_tokens: value.usage_input_uncached_tokens,
            usage_input_cached_tokens: value.usage_input_cached_tokens,
            usage_output_tokens: value.usage_output_tokens,
            usage_credit_total: value.usage_credit_total,
            usage_credit_missing_events: value.usage_credit_missing_events,
            remaining_billable: value.remaining_billable(),
            last_used_at: value.last_used_at,
            created_at: value.created_at,
            updated_at: value.updated_at,
            route_strategy: value.route_strategy.clone(),
            account_group_id: value.account_group_id.clone(),
            fixed_account_name: value.fixed_account_name.clone(),
            auto_account_names: value.auto_account_names.clone(),
            model_name_map: value.model_name_map.clone(),
            kiro_request_validation_enabled: value.kiro_request_validation_enabled,
            kiro_cache_estimation_enabled: value.kiro_cache_estimation_enabled,
            kiro_zero_cache_debug_enabled: value.kiro_zero_cache_debug_enabled,
            kiro_cache_policy_override_json: value.kiro_cache_policy_override_json.clone(),
            kiro_billable_model_multipliers_override_json: value
                .kiro_billable_model_multipliers_override_json
                .clone(),
            effective_kiro_cache_policy_json: serde_json::to_string_pretty(effective_policy)
                .expect("effective kiro cache policy should serialize"),
            uses_global_kiro_cache_policy,
            effective_kiro_billable_model_multipliers_json: serde_json::to_string_pretty(
                effective_billable_model_multipliers,
            )
            .expect("effective kiro billable multipliers should serialize"),
            uses_global_kiro_billable_model_multipliers,
        }
    }
}

fn deserialize_optional_nullable_string<'de, D>(
    deserializer: D,
) -> Result<Option<Option<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Some(Option::<String>::deserialize(deserializer)?))
}

/// Admin response wrapper for the Kiro key inventory.
#[derive(Debug, Serialize)]
pub struct AdminKiroKeysResponse {
    pub keys: Vec<AdminKiroKeyView>,
    pub auth_cache_ttl_seconds: u64,
    pub generated_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AdminKiroAccountGroupView {
    pub id: String,
    pub provider_type: String,
    pub name: String,
    pub account_names: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl From<&LlmGatewayAccountGroupRecord> for AdminKiroAccountGroupView {
    fn from(value: &LlmGatewayAccountGroupRecord) -> Self {
        Self {
            id: value.id.clone(),
            provider_type: value.provider_type.clone(),
            name: value.name.clone(),
            account_names: value.account_names.clone(),
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AdminKiroAccountGroupsResponse {
    pub groups: Vec<AdminKiroAccountGroupView>,
    pub generated_at: i64,
}

/// Admin-facing projection of one persisted Kiro usage event summary.
#[derive(Debug, Serialize)]
pub struct AdminKiroUsageEventView {
    pub id: String,
    pub key_id: String,
    pub key_name: String,
    pub account_name: Option<String>,
    pub request_method: String,
    pub request_url: String,
    pub latency_ms: i32,
    pub endpoint: String,
    pub model: Option<String>,
    pub status_code: i32,
    pub input_uncached_tokens: u64,
    pub input_cached_tokens: u64,
    pub output_tokens: u64,
    pub billable_tokens: u64,
    pub usage_missing: bool,
    pub credit_usage: Option<f64>,
    pub credit_usage_missing: bool,
    pub client_ip: String,
    pub ip_region: String,
    pub last_message_content: Option<String>,
    pub created_at: i64,
}

/// Full Kiro usage-event detail payload returned on demand.
#[derive(Debug, Serialize)]
pub struct AdminKiroUsageEventDetailView {
    pub id: String,
    pub key_id: String,
    pub key_name: String,
    pub account_name: Option<String>,
    pub request_method: String,
    pub request_url: String,
    pub latency_ms: i32,
    pub endpoint: String,
    pub model: Option<String>,
    pub status_code: i32,
    pub input_uncached_tokens: u64,
    pub input_cached_tokens: u64,
    pub output_tokens: u64,
    pub billable_tokens: u64,
    pub usage_missing: bool,
    pub credit_usage: Option<f64>,
    pub credit_usage_missing: bool,
    pub client_ip: String,
    pub ip_region: String,
    pub request_headers_json: String,
    pub last_message_content: Option<String>,
    pub client_request_body_json: Option<String>,
    pub upstream_request_body_json: Option<String>,
    pub full_request_json: Option<String>,
    pub created_at: i64,
}

impl From<&LlmGatewayUsageEventSummaryRecord> for AdminKiroUsageEventView {
    fn from(value: &LlmGatewayUsageEventSummaryRecord) -> Self {
        Self {
            id: value.id.clone(),
            key_id: value.key_id.clone(),
            key_name: value.key_name.clone(),
            account_name: value.account_name.clone(),
            request_method: value.request_method.clone(),
            request_url: value.request_url.clone(),
            latency_ms: value.latency_ms,
            endpoint: value.endpoint.clone(),
            model: value.model.clone(),
            status_code: value.status_code,
            input_uncached_tokens: value.input_uncached_tokens,
            input_cached_tokens: value.input_cached_tokens,
            output_tokens: value.output_tokens,
            billable_tokens: value.billable_tokens,
            usage_missing: value.usage_missing,
            credit_usage: value.credit_usage,
            credit_usage_missing: value.credit_usage_missing,
            client_ip: value.client_ip.clone(),
            ip_region: value.ip_region.clone(),
            last_message_content: value.last_message_content.clone(),
            created_at: value.created_at,
        }
    }
}

impl From<&LlmGatewayUsageEventRecord> for AdminKiroUsageEventDetailView {
    fn from(value: &LlmGatewayUsageEventRecord) -> Self {
        Self {
            id: value.id.clone(),
            key_id: value.key_id.clone(),
            key_name: value.key_name.clone(),
            account_name: value.account_name.clone(),
            request_method: value.request_method.clone(),
            request_url: value.request_url.clone(),
            latency_ms: value.latency_ms,
            endpoint: value.endpoint.clone(),
            model: value.model.clone(),
            status_code: value.status_code,
            input_uncached_tokens: value.input_uncached_tokens,
            input_cached_tokens: value.input_cached_tokens,
            output_tokens: value.output_tokens,
            billable_tokens: value.billable_tokens,
            usage_missing: value.usage_missing,
            credit_usage: value.credit_usage,
            credit_usage_missing: value.credit_usage_missing,
            client_ip: value.client_ip.clone(),
            ip_region: value.ip_region.clone(),
            request_headers_json: value.request_headers_json.clone(),
            last_message_content: value.last_message_content.clone(),
            client_request_body_json: value.client_request_body_json.clone(),
            upstream_request_body_json: value.upstream_request_body_json.clone(),
            full_request_json: value.full_request_json.clone(),
            created_at: value.created_at,
        }
    }
}

/// Paginated admin response wrapper for Kiro usage events.
#[derive(Debug, Serialize)]
pub struct AdminKiroUsageEventsResponse {
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
    pub has_more: bool,
    pub events: Vec<AdminKiroUsageEventView>,
    pub generated_at: i64,
}

/// Query parameters accepted by the Kiro usage-events admin endpoint.
#[derive(Debug, Deserialize, Default)]
pub struct AdminKiroUsageQuery {
    pub key_id: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

/// Request body for creating a new private Kiro API key.
#[derive(Debug, Deserialize)]
pub struct CreateKiroKeyRequest {
    pub name: String,
    pub quota_billable_limit: u64,
}

/// Patch payload for mutating an existing private Kiro API key.
#[derive(Debug, Deserialize)]
pub struct PatchKiroKeyRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub quota_billable_limit: Option<u64>,
    #[serde(default)]
    pub route_strategy: Option<String>,
    #[serde(default)]
    pub account_group_id: Option<String>,
    #[serde(default)]
    pub fixed_account_name: Option<String>,
    #[serde(default)]
    pub auto_account_names: Option<Vec<String>>,
    #[serde(default)]
    pub model_name_map: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub kiro_request_validation_enabled: Option<bool>,
    #[serde(default)]
    pub kiro_cache_estimation_enabled: Option<bool>,
    #[serde(default)]
    pub kiro_zero_cache_debug_enabled: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_optional_nullable_string")]
    pub kiro_cache_policy_override_json: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_optional_nullable_string")]
    pub kiro_billable_model_multipliers_override_json: Option<Option<String>>,
}

#[derive(Debug, Deserialize)]
pub struct CreateKiroAccountGroupRequest {
    pub name: String,
    pub account_names: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct PatchKiroAccountGroupRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub account_names: Option<Vec<String>>,
}

/// Normalized account-balance snapshot derived from Kiro `getUsageLimits`.
#[derive(Debug, Clone, Serialize)]
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
    /// shape used by StaticFlow.
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

/// Admin-facing projection of one configured Kiro account.
///
/// Combines persisted auth metadata, effective scheduler settings, cached
/// balance, and cache-refresh state into one page-friendly record.
#[derive(Debug, Clone, Serialize)]
pub struct KiroAccountView {
    pub name: String,
    pub auth_method: String,
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_user_id: Option<String>,
    pub email: Option<String>,
    pub expires_at: Option<String>,
    pub profile_arn: Option<String>,
    pub has_refresh_token: bool,
    pub disabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled_reason: Option<String>,
    pub source: Option<String>,
    pub source_db_path: Option<String>,
    pub last_imported_at: Option<i64>,
    pub subscription_title: Option<String>,
    pub region: Option<String>,
    pub auth_region: Option<String>,
    pub api_region: Option<String>,
    pub machine_id: Option<String>,
    pub kiro_channel_max_concurrency: u64,
    pub kiro_channel_min_start_interval_ms: u64,
    pub minimum_remaining_credits_before_block: f64,
    pub proxy_mode: String,
    pub proxy_config_id: Option<String>,
    pub effective_proxy_source: String,
    pub effective_proxy_url: Option<String>,
    pub effective_proxy_config_name: Option<String>,
    pub proxy_url: Option<String>,
    pub balance: Option<KiroBalanceView>,
    pub cache: KiroCacheView,
}

impl KiroAccountView {
    /// Build a UI-friendly account view from the persisted auth record and the
    /// latest cached balance probe.
    pub fn from_auth(
        auth: &KiroAuthRecord,
        effective_proxy_source: String,
        effective_proxy_url: Option<String>,
        effective_proxy_config_name: Option<String>,
        balance: Option<KiroBalanceView>,
        cache: KiroCacheView,
    ) -> Self {
        let subscription_title = balance
            .as_ref()
            .and_then(|value| value.subscription_title.clone())
            .or_else(|| auth.subscription_title.clone());
        Self {
            name: auth.name.clone(),
            auth_method: auth.auth_method().to_string(),
            provider: auth.provider.clone(),
            upstream_user_id: balance.as_ref().and_then(|value| value.user_id.clone()),
            email: auth.email.clone(),
            expires_at: auth.expires_at.clone(),
            profile_arn: auth.profile_arn.clone(),
            has_refresh_token: auth.has_refresh_token(),
            disabled: auth.disabled,
            disabled_reason: auth.disabled_reason.clone(),
            source: auth.source.clone(),
            source_db_path: auth.source_db_path.clone(),
            last_imported_at: auth.last_imported_at,
            subscription_title,
            region: auth.region.clone(),
            auth_region: auth.auth_region.clone(),
            api_region: auth.api_region.clone(),
            machine_id: auth.machine_id.clone(),
            kiro_channel_max_concurrency: auth.effective_kiro_channel_max_concurrency(),
            kiro_channel_min_start_interval_ms: auth.effective_kiro_channel_min_start_interval_ms(),
            minimum_remaining_credits_before_block: auth
                .effective_minimum_remaining_credits_before_block(),
            proxy_mode: auth.proxy_selection().proxy_mode.as_str().to_string(),
            proxy_config_id: auth.proxy_selection().proxy_config_id,
            effective_proxy_source,
            effective_proxy_url,
            effective_proxy_config_name,
            proxy_url: auth.proxy_url.clone(),
            balance,
            cache,
        }
    }
}

/// Admin response wrapper for the full Kiro account inventory.
#[derive(Debug, Serialize)]
pub struct AdminKiroAccountsResponse {
    pub accounts: Vec<KiroAccountView>,
    pub generated_at: i64,
}

#[derive(Debug, Default, Deserialize)]
pub struct AdminKiroAccountStatusesQuery {
    #[serde(default)]
    pub prefix: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub offset: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct AdminKiroAccountStatusesResponse {
    pub accounts: Vec<KiroAccountView>,
    pub total: usize,
    pub limit: usize,
    pub offset: usize,
    pub generated_at: i64,
}

/// Request body for importing a Kiro account from a local CLI SQLite store.
#[derive(Debug, Deserialize)]
pub struct ImportLocalKiroAccountRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub sqlite_path: Option<String>,
    #[serde(default)]
    pub kiro_channel_max_concurrency: Option<u64>,
    #[serde(default)]
    pub kiro_channel_min_start_interval_ms: Option<u64>,
}

/// Request body for manually creating a persisted Kiro account JSON record.
#[derive(Debug, Deserialize)]
pub struct CreateManualKiroAccountRequest {
    pub name: String,
    #[serde(default)]
    pub access_token: Option<String>,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub profile_arn: Option<String>,
    #[serde(default)]
    pub expires_at: Option<String>,
    #[serde(default)]
    pub auth_method: Option<String>,
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
    pub client_secret: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub auth_region: Option<String>,
    #[serde(default)]
    pub api_region: Option<String>,
    #[serde(default)]
    pub machine_id: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub subscription_title: Option<String>,
    #[serde(default)]
    pub kiro_channel_max_concurrency: Option<u64>,
    #[serde(default)]
    pub kiro_channel_min_start_interval_ms: Option<u64>,
    #[serde(default)]
    pub minimum_remaining_credits_before_block: Option<f64>,
    #[serde(default)]
    pub disabled: bool,
}

/// Request body for editing only the mutable per-account scheduler settings.
#[derive(Debug, Deserialize)]
pub struct PatchKiroAccountRequest {
    #[serde(default)]
    pub kiro_channel_max_concurrency: Option<u64>,
    #[serde(default)]
    pub kiro_channel_min_start_interval_ms: Option<u64>,
    #[serde(default)]
    pub minimum_remaining_credits_before_block: Option<f64>,
    #[serde(default)]
    pub proxy_mode: Option<String>,
    #[serde(default)]
    pub proxy_config_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn account_view_surfaces_disabled_reason() {
        let auth = KiroAuthRecord {
            name: "alpha".to_string(),
            disabled: true,
            disabled_reason: Some("invalid_refresh_token".to_string()),
            ..KiroAuthRecord::default()
        };
        let view = KiroAccountView::from_auth(
            &auth,
            "direct".to_string(),
            None,
            None,
            None,
            KiroCacheView::default(),
        );
        assert_eq!(view.disabled_reason.as_deref(), Some("invalid_refresh_token"));
    }

    #[test]
    fn patch_kiro_key_request_distinguishes_absent_null_and_value_for_policy_override() {
        let absent: PatchKiroKeyRequest =
            serde_json::from_value(json!({})).expect("parse absent request");
        let clear: PatchKiroKeyRequest =
            serde_json::from_value(json!({"kiro_cache_policy_override_json": null}))
                .expect("parse clear request");
        let set: PatchKiroKeyRequest = serde_json::from_value(json!({
            "kiro_cache_policy_override_json": "{\"high_credit_diagnostic_threshold\":1.6}"
        }))
        .expect("parse set request");

        assert_eq!(absent.kiro_cache_policy_override_json, None);
        assert_eq!(clear.kiro_cache_policy_override_json, Some(None));
        assert_eq!(
            set.kiro_cache_policy_override_json,
            Some(Some("{\"high_credit_diagnostic_threshold\":1.6}".to_string()))
        );
    }

    #[test]
    fn patch_kiro_key_request_accepts_zero_cache_debug_toggle() {
        let absent: PatchKiroKeyRequest =
            serde_json::from_value(json!({})).expect("parse absent request");
        let enabled: PatchKiroKeyRequest =
            serde_json::from_value(json!({"kiro_zero_cache_debug_enabled": true}))
                .expect("parse enabled request");

        assert_eq!(absent.kiro_zero_cache_debug_enabled, None);
        assert_eq!(enabled.kiro_zero_cache_debug_enabled, Some(true));
    }

    #[test]
    fn patch_kiro_key_request_distinguishes_absent_null_and_value_for_billable_multiplier_override()
    {
        let absent: PatchKiroKeyRequest =
            serde_json::from_value(json!({})).expect("parse absent request");
        let clear: PatchKiroKeyRequest = serde_json::from_value(json!({
            "kiro_billable_model_multipliers_override_json": null
        }))
        .expect("parse clear request");
        let set: PatchKiroKeyRequest = serde_json::from_value(json!({
            "kiro_billable_model_multipliers_override_json": "{\"opus\":1.6}"
        }))
        .expect("parse set request");

        assert_eq!(absent.kiro_billable_model_multipliers_override_json, None);
        assert_eq!(clear.kiro_billable_model_multipliers_override_json, Some(None));
        assert_eq!(
            set.kiro_billable_model_multipliers_override_json,
            Some(Some("{\"opus\":1.6}".to_string()))
        );
    }

    #[test]
    fn admin_kiro_account_status_query_defaults_to_none() {
        let query: AdminKiroAccountStatusesQuery =
            serde_json::from_value(json!({})).expect("parse empty query");

        assert_eq!(query.prefix, None);
        assert_eq!(query.limit, None);
        assert_eq!(query.offset, None);
    }

    #[test]
    fn recent_kiro_usage_event_view_keeps_only_preview_fields() {
        let event = LlmGatewayUsageEventSummaryRecord {
            id: "evt-1".to_string(),
            key_id: "key-1".to_string(),
            key_name: "alpha".to_string(),
            provider_type: "kiro".to_string(),
            account_name: Some("acct-a".to_string()),
            request_method: "POST".to_string(),
            request_url: "https://example.com".to_string(),
            latency_ms: 42,
            endpoint: "/v1/messages".to_string(),
            model: Some("claude-sonnet-4-6".to_string()),
            status_code: 200,
            input_uncached_tokens: 10,
            input_cached_tokens: 20,
            output_tokens: 30,
            billable_tokens: 40,
            usage_missing: false,
            credit_usage: Some(1.25),
            credit_usage_missing: false,
            client_ip: "127.0.0.1".to_string(),
            ip_region: "Local".to_string(),
            last_message_content: Some("hello".to_string()),
            created_at: 123,
        };

        let view = AdminKiroUsageEventView::from(&event);

        assert_eq!(view.id, "evt-1");
        assert_eq!(view.key_name, "alpha");
        assert_eq!(view.model.as_deref(), Some("claude-sonnet-4-6"));
        assert_eq!(view.credit_usage, Some(1.25));
        assert_eq!(view.last_message_content.as_deref(), Some("hello"));
        assert_eq!(view.created_at, 123);
    }
}
