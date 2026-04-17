use std::{collections::BTreeMap, time::Instant};

use axum::{http::Method, response::Json};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use static_flow_shared::llm_gateway_store::{
    compute_billable_tokens, LlmGatewayAccountContributionRequestRecord,
    LlmGatewayAccountGroupRecord, LlmGatewayKeyRecord, LlmGatewayProxyConfigRecord,
    LlmGatewaySponsorRequestRecord, LlmGatewayTokenRequestRecord, LlmGatewayUsageEventRecord,
    LlmGatewayUsageEventSummaryRecord,
};

use crate::handlers::ErrorResponse;

/// Public read-only payload rendered by the `/llm-access` page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmGatewayAccessResponse {
    pub base_url: String,
    pub gateway_path: String,
    pub auth_cache_ttl_seconds: u64,
    pub keys: Vec<LlmGatewayPublicKeyView>,
    pub generated_at: i64,
}

/// Public support/community configuration rendered on `/llm-access`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmGatewaySupportConfigView {
    pub sponsor_title: String,
    pub sponsor_intro: String,
    pub group_name: String,
    pub qq_group_number: String,
    pub group_invite_text: String,
    pub alipay_qr_url: String,
    pub wechat_qr_url: String,
    pub qq_group_qr_url: Option<String>,
    pub generated_at: i64,
}

/// Public key summary safe to expose on the read-only access page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmGatewayPublicKeyView {
    pub id: String,
    pub name: String,
    pub secret: String,
    pub quota_billable_limit: u64,
    pub usage_input_uncached_tokens: u64,
    pub usage_input_cached_tokens: u64,
    pub usage_output_tokens: u64,
    pub remaining_billable: i64,
    pub last_used_at: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct PublicLlmGatewayUsageLookupRequest {
    pub api_key: String,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub offset: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PublicLlmGatewayUsageKeyView {
    pub name: String,
    pub provider_type: String,
    pub quota_billable_limit: u64,
    pub usage_input_uncached_tokens: u64,
    pub usage_input_cached_tokens: u64,
    pub usage_output_tokens: u64,
    pub usage_billable_tokens: u64,
    pub usage_credit_total: f64,
    pub usage_credit_missing_events: u64,
    pub remaining_billable: i64,
    pub last_used_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PublicLlmGatewayUsageEventView {
    pub id: String,
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
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PublicLlmGatewayUsageChartPointView {
    pub bucket_start_ms: i64,
    pub tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PublicLlmGatewayUsageLookupResponse {
    pub key: PublicLlmGatewayUsageKeyView,
    pub chart_points: Vec<PublicLlmGatewayUsageChartPointView>,
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
    pub has_more: bool,
    pub events: Vec<PublicLlmGatewayUsageEventView>,
    pub generated_at: i64,
}

/// Public read-only payload for the cached Codex rate-limit snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmGatewayRateLimitStatusResponse {
    pub status: String,
    pub refresh_interval_seconds: u64,
    pub last_checked_at: Option<i64>,
    pub last_success_at: Option<i64>,
    pub source_url: String,
    pub error_message: Option<String>,
    #[serde(default)]
    pub accounts: Vec<LlmGatewayPublicAccountStatusView>,
    pub buckets: Vec<LlmGatewayRateLimitBucketView>,
}

/// One public Codex account summary rendered on `/llm-access`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmGatewayPublicAccountStatusView {
    pub name: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_remaining_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secondary_remaining_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_usage_checked_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_usage_success_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_error_message: Option<String>,
}

/// One limit bucket rendered on the public status surface.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmGatewayRateLimitBucketView {
    pub limit_id: String,
    pub limit_name: Option<String>,
    pub display_name: String,
    pub is_primary: bool,
    pub plan_type: Option<String>,
    pub primary: Option<LlmGatewayRateLimitWindowView>,
    pub secondary: Option<LlmGatewayRateLimitWindowView>,
    pub credits: Option<LlmGatewayCreditsView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_name: Option<String>,
}

/// One usage window within a rate-limit bucket.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmGatewayRateLimitWindowView {
    pub used_percent: f64,
    pub remaining_percent: f64,
    pub window_duration_mins: Option<i64>,
    pub resets_at: Option<i64>,
}

/// Credit metadata included in the upstream usage payload when available.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmGatewayCreditsView {
    pub has_credits: bool,
    pub unlimited: bool,
    pub balance: Option<String>,
}

/// Admin payload for the key inventory screen.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminLlmGatewayKeysResponse {
    pub keys: Vec<AdminLlmGatewayKeyView>,
    pub auth_cache_ttl_seconds: u64,
    pub generated_at: i64,
}

/// Admin-facing projection of one managed API key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminLlmGatewayKeyView {
    pub id: String,
    pub name: String,
    pub secret: String,
    pub key_hash: String,
    pub status: String,
    pub provider_type: String,
    pub public_visible: bool,
    pub quota_billable_limit: u64,
    pub usage_input_uncached_tokens: u64,
    pub usage_input_cached_tokens: u64,
    pub usage_output_tokens: u64,
    pub usage_credit_total: f64,
    pub usage_credit_missing_events: u64,
    pub remaining_billable: i64,
    pub last_used_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    pub route_strategy: Option<String>,
    pub account_group_id: Option<String>,
    pub fixed_account_name: Option<String>,
    pub auto_account_names: Option<Vec<String>>,
    pub model_name_map: Option<BTreeMap<String, String>>,
    pub request_max_concurrency: Option<u64>,
    pub request_min_start_interval_ms: Option<u64>,
}

/// Paginated admin response for settled usage events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminLlmGatewayUsageEventsResponse {
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
    pub has_more: bool,
    pub current_rpm: u32,
    pub current_in_flight: u32,
    pub events: Vec<AdminLlmGatewayUsageEventView>,
    pub generated_at: i64,
}

/// Full detail payload for one usage event requested on demand.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminLlmGatewayUsageEventDetailView {
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

/// Admin-facing reusable account-pool group shared by keys of one provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminAccountGroupView {
    pub id: String,
    pub provider_type: String,
    pub name: String,
    pub account_names: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl From<&LlmGatewayAccountGroupRecord> for AdminAccountGroupView {
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminAccountGroupsResponse {
    pub groups: Vec<AdminAccountGroupView>,
    pub generated_at: i64,
}

/// Public request body for asking an admin to grant a new token.
#[derive(Debug, Clone, Deserialize)]
pub struct SubmitLlmGatewayTokenRequest {
    pub requested_quota_billable_limit: u64,
    pub request_reason: String,
    pub requester_email: String,
    #[serde(default)]
    pub frontend_page_url: Option<String>,
}

/// Public acknowledgement returned after a token wish is queued.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitLlmGatewayTokenRequestResponse {
    pub request_id: String,
    pub status: String,
}

/// Public request body for contributing a Codex account into the shared pool.
#[derive(Debug, Clone, Deserialize)]
pub struct SubmitLlmGatewayAccountContributionRequest {
    pub account_name: String,
    #[serde(default)]
    pub account_id: Option<String>,
    pub id_token: String,
    pub access_token: String,
    pub refresh_token: String,
    pub requester_email: String,
    pub contributor_message: String,
    #[serde(default)]
    pub github_id: Option<String>,
    #[serde(default)]
    pub frontend_page_url: Option<String>,
}

/// Public acknowledgement returned after an account contribution is queued.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitLlmGatewayAccountContributionRequestResponse {
    pub request_id: String,
    pub status: String,
}

/// Public thank-you card payload rendered at the bottom of `/llm-access`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicLlmGatewayAccountContributionView {
    pub request_id: String,
    pub account_name: String,
    pub contributor_message: String,
    pub github_id: Option<String>,
    pub processed_at: Option<i64>,
}

/// Public response containing already-approved account contributions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicLlmGatewayAccountContributionsResponse {
    pub contributions: Vec<PublicLlmGatewayAccountContributionView>,
    pub generated_at: i64,
}

/// Public sponsor submission payload from `/llm-access`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitLlmGatewaySponsorRequest {
    pub requester_email: String,
    pub sponsor_message: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub github_id: Option<String>,
    #[serde(default)]
    pub frontend_page_url: Option<String>,
}

/// Public acknowledgement returned after a sponsor request is queued.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitLlmGatewaySponsorRequestResponse {
    pub request_id: String,
    pub status: String,
    pub payment_email_sent: bool,
}

/// Public sponsor thank-you card payload rendered at the bottom of
/// `/llm-access`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicLlmGatewaySponsorView {
    pub request_id: String,
    pub display_name: Option<String>,
    pub sponsor_message: String,
    pub github_id: Option<String>,
    pub processed_at: Option<i64>,
}

/// Public response containing already-approved sponsors.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicLlmGatewaySponsorsResponse {
    pub sponsors: Vec<PublicLlmGatewaySponsorView>,
    pub generated_at: i64,
}

/// Admin-facing projection of one token wish / issuance task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminLlmGatewayTokenRequestView {
    pub request_id: String,
    pub requester_email: String,
    pub requested_quota_billable_limit: u64,
    pub request_reason: String,
    pub frontend_page_url: Option<String>,
    pub status: String,
    pub client_ip: String,
    pub ip_region: String,
    pub admin_note: Option<String>,
    pub failure_reason: Option<String>,
    pub issued_key_id: Option<String>,
    pub issued_key_name: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub processed_at: Option<i64>,
}

/// Paginated admin response for token wishes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminLlmGatewayTokenRequestsResponse {
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
    pub has_more: bool,
    pub requests: Vec<AdminLlmGatewayTokenRequestView>,
    pub generated_at: i64,
}

/// Admin-facing projection of one account contribution request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminLlmGatewayAccountContributionRequestView {
    pub request_id: String,
    pub account_name: String,
    pub account_id: Option<String>,
    pub id_token: String,
    pub access_token: String,
    pub refresh_token: String,
    pub requester_email: String,
    pub contributor_message: String,
    pub github_id: Option<String>,
    pub frontend_page_url: Option<String>,
    pub status: String,
    pub client_ip: String,
    pub ip_region: String,
    pub admin_note: Option<String>,
    pub failure_reason: Option<String>,
    pub imported_account_name: Option<String>,
    pub issued_key_id: Option<String>,
    pub issued_key_name: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub processed_at: Option<i64>,
}

/// Paginated admin response for account contribution requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminLlmGatewayAccountContributionRequestsResponse {
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
    pub has_more: bool,
    pub requests: Vec<AdminLlmGatewayAccountContributionRequestView>,
    pub generated_at: i64,
}

/// Admin query parameters for account contribution request pagination.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct AdminLlmGatewayAccountContributionRequestQuery {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub offset: Option<usize>,
}

/// Admin-facing projection of one sponsor request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminLlmGatewaySponsorRequestView {
    pub request_id: String,
    pub requester_email: String,
    pub sponsor_message: String,
    pub display_name: Option<String>,
    pub github_id: Option<String>,
    pub frontend_page_url: Option<String>,
    pub status: String,
    pub client_ip: String,
    pub ip_region: String,
    pub admin_note: Option<String>,
    pub failure_reason: Option<String>,
    pub payment_email_sent_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    pub processed_at: Option<i64>,
}

/// Paginated admin response for sponsor requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminLlmGatewaySponsorRequestsResponse {
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
    pub has_more: bool,
    pub requests: Vec<AdminLlmGatewaySponsorRequestView>,
    pub generated_at: i64,
}

/// Admin query parameters for sponsor request pagination.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct AdminLlmGatewaySponsorRequestQuery {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub offset: Option<usize>,
}

/// Admin query parameters for token-wish filtering and pagination.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct AdminLlmGatewayTokenRequestQuery {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub offset: Option<usize>,
}

/// Admin-facing usage event summary used in paginated list views.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminLlmGatewayUsageEventView {
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

/// Lightweight admin response for runtime gateway configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmGatewayRuntimeConfigResponse {
    pub auth_cache_ttl_seconds: u64,
    /// Maximum allowed request body size in bytes for proxied calls.
    pub max_request_body_bytes: u64,
    /// Number of consecutive Codex refresh failures tolerated before an
    /// account becomes unavailable.
    pub account_failure_retry_limit: u64,
    pub codex_status_refresh_min_interval_seconds: u64,
    pub codex_status_refresh_max_interval_seconds: u64,
    pub codex_status_account_jitter_max_seconds: u64,
    pub kiro_status_refresh_min_interval_seconds: u64,
    pub kiro_status_refresh_max_interval_seconds: u64,
    pub kiro_status_account_jitter_max_seconds: u64,
    pub usage_event_flush_batch_size: u64,
    pub usage_event_flush_interval_seconds: u64,
    pub usage_event_flush_max_buffer_bytes: u64,
    pub kiro_cache_kmodels_json: String,
    pub kiro_billable_model_multipliers_json: String,
    pub kiro_cache_policy_json: String,
    pub kiro_prefix_cache_mode: String,
    pub kiro_prefix_cache_max_tokens: u64,
    pub kiro_prefix_cache_entry_ttl_seconds: u64,
    pub kiro_conversation_anchor_max_entries: u64,
    pub kiro_conversation_anchor_ttl_seconds: u64,
}

/// One reusable upstream proxy config managed from the admin UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminUpstreamProxyConfigView {
    pub id: String,
    pub name: String,
    pub proxy_url: String,
    pub proxy_username: Option<String>,
    pub proxy_password: Option<String>,
    pub status: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Inventory response for shared upstream proxy configs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminUpstreamProxyConfigsResponse {
    pub proxy_configs: Vec<AdminUpstreamProxyConfigView>,
    pub generated_at: i64,
}

/// One connectivity probe result for a reusable proxy config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminUpstreamProxyCheckTargetView {
    pub target: String,
    pub url: String,
    pub reachable: bool,
    pub status_code: Option<u16>,
    pub latency_ms: i64,
    pub error_message: Option<String>,
}

/// Admin response for checking whether a reusable proxy config can reach the
/// upstream hosts used by StaticFlow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminUpstreamProxyCheckResponse {
    pub proxy_config_id: String,
    pub proxy_config_name: String,
    pub provider_type: String,
    pub auth_label: String,
    pub ok: bool,
    pub targets: Vec<AdminUpstreamProxyCheckTargetView>,
    pub checked_at: i64,
}

/// Effective binding state for one upstream provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminUpstreamProxyBindingView {
    pub provider_type: String,
    pub effective_source: String,
    pub bound_proxy_config_id: Option<String>,
    pub effective_proxy_config_name: Option<String>,
    pub effective_proxy_url: Option<String>,
    pub effective_proxy_username: Option<String>,
    pub effective_proxy_password: Option<String>,
    pub binding_updated_at: Option<i64>,
    pub error_message: Option<String>,
}

/// Snapshot of provider-level proxy bindings shown in admin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminUpstreamProxyBindingsResponse {
    pub bindings: Vec<AdminUpstreamProxyBindingView>,
    pub generated_at: i64,
}

/// Create one reusable proxy config.
#[derive(Debug, Clone, Deserialize)]
pub struct CreateAdminUpstreamProxyConfigRequest {
    pub name: String,
    pub proxy_url: String,
    #[serde(default)]
    pub proxy_username: Option<String>,
    #[serde(default)]
    pub proxy_password: Option<String>,
}

/// Patch one reusable proxy config.
#[derive(Debug, Clone, Deserialize)]
pub struct PatchAdminUpstreamProxyConfigRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub proxy_url: Option<String>,
    #[serde(default)]
    pub proxy_username: Option<String>,
    #[serde(default)]
    pub proxy_password: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
}

/// Update or clear a provider-level proxy binding.
#[derive(Debug, Clone, Deserialize)]
pub struct UpdateAdminUpstreamProxyBindingRequest {
    #[serde(default)]
    pub proxy_config_id: Option<String>,
}

/// Result payload after importing legacy Kiro account-level proxies into the
/// shared registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminLegacyKiroProxyMigrationResponse {
    pub created_configs: Vec<AdminUpstreamProxyConfigView>,
    pub reused_configs: Vec<AdminUpstreamProxyConfigView>,
    pub migrated_account_names: Vec<String>,
    pub generated_at: i64,
}

/// Admin request body for updating runtime cache configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct UpdateLlmGatewayRuntimeConfigRequest {
    pub auth_cache_ttl_seconds: Option<u64>,
    /// New maximum request body size in bytes, if changing.
    pub max_request_body_bytes: Option<u64>,
    /// New consecutive-failure threshold before one Codex account becomes
    /// unavailable.
    pub account_failure_retry_limit: Option<u64>,
    pub codex_status_refresh_min_interval_seconds: Option<u64>,
    pub codex_status_refresh_max_interval_seconds: Option<u64>,
    pub codex_status_account_jitter_max_seconds: Option<u64>,
    pub kiro_status_refresh_min_interval_seconds: Option<u64>,
    pub kiro_status_refresh_max_interval_seconds: Option<u64>,
    pub kiro_status_account_jitter_max_seconds: Option<u64>,
    pub usage_event_flush_batch_size: Option<u64>,
    pub usage_event_flush_interval_seconds: Option<u64>,
    pub usage_event_flush_max_buffer_bytes: Option<u64>,
    pub kiro_cache_kmodels_json: Option<String>,
    pub kiro_billable_model_multipliers_json: Option<String>,
    pub kiro_cache_policy_json: Option<String>,
    pub kiro_prefix_cache_mode: Option<String>,
    pub kiro_prefix_cache_max_tokens: Option<u64>,
    pub kiro_prefix_cache_entry_ttl_seconds: Option<u64>,
    pub kiro_conversation_anchor_max_entries: Option<u64>,
    pub kiro_conversation_anchor_ttl_seconds: Option<u64>,
}

/// Admin request body for creating a new externally visible gateway key.
#[derive(Debug, Clone, Deserialize)]
pub struct CreateLlmGatewayKeyRequest {
    pub name: String,
    pub quota_billable_limit: u64,
    #[serde(default)]
    pub public_visible: bool,
    #[serde(default)]
    pub request_max_concurrency: Option<u64>,
    #[serde(default)]
    pub request_min_start_interval_ms: Option<u64>,
}

/// Admin request body for mutating one existing gateway key.
#[derive(Debug, Clone, Deserialize)]
pub struct PatchLlmGatewayKeyRequest {
    pub name: Option<String>,
    pub status: Option<String>,
    pub public_visible: Option<bool>,
    pub quota_billable_limit: Option<u64>,
    pub route_strategy: Option<String>,
    pub account_group_id: Option<String>,
    pub fixed_account_name: Option<String>,
    pub auto_account_names: Option<Vec<String>>,
    pub model_name_map: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub request_max_concurrency: Option<u64>,
    #[serde(default)]
    pub request_min_start_interval_ms: Option<u64>,
    #[serde(default)]
    pub request_max_concurrency_unlimited: bool,
    #[serde(default)]
    pub request_min_start_interval_ms_unlimited: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateAdminAccountGroupRequest {
    pub name: String,
    pub account_names: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PatchAdminAccountGroupRequest {
    pub name: Option<String>,
    pub account_names: Option<Vec<String>>,
}

/// Admin query parameters for usage-event filtering and pagination.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct AdminLlmGatewayUsageQuery {
    #[serde(default)]
    pub key_id: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub offset: Option<usize>,
}

/// Token usage accounting in the billing model used by the gateway.
#[derive(Debug, Clone, Default)]
pub(crate) struct UsageBreakdown {
    pub input_uncached_tokens: u64,
    pub input_cached_tokens: u64,
    pub output_tokens: u64,
    pub usage_missing: bool,
}

impl UsageBreakdown {
    pub fn billable_tokens(&self) -> u64 {
        compute_billable_tokens(
            self.input_uncached_tokens,
            self.input_cached_tokens,
            self.output_tokens,
        )
    }

    pub fn billable_tokens_with_multiplier(&self, multiplier: u64) -> u64 {
        self.billable_tokens().saturating_mul(multiplier.max(1))
    }
}

/// Normalized proxy request ready to send to the upstream Codex backend.
#[derive(Debug, Clone)]
pub(crate) struct PreparedGatewayRequest {
    pub original_path: String,
    pub upstream_path: String,
    pub method: Method,
    pub client_request_body: axum::body::Bytes,
    pub request_body: axum::body::Bytes,
    pub model: Option<String>,
    pub client_visible_model: Option<String>,
    pub wants_stream: bool,
    pub force_upstream_stream: bool,
    pub content_type: String,
    pub response_adapter: GatewayResponseAdapter,
    pub thread_anchor: Option<String>,
    pub tool_name_restore_map: BTreeMap<String, String>,
    pub billable_multiplier: u64,
}

/// Request diagnostics captured before the upstream call starts.
///
/// Usage accounting happens after the response completes, so the proxy stores
/// this request-scoped context in an extension and reuses it during
/// persistence.
#[derive(Debug, Clone)]
pub(crate) struct LlmGatewayEventContext {
    pub request_method: String,
    pub request_url: String,
    pub client_ip: String,
    pub ip_region: String,
    pub request_headers_json: String,
    pub started_at: Instant,
}

/// Response adaptation mode selected by the incoming OpenAI-compatible
/// endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GatewayResponseAdapter {
    Responses,
    ChatCompletions,
}

/// Internal normalized representation of one upstream model descriptor.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct GatewayModelDescriptor {
    pub id: String,
    pub owned_by: &'static str,
}

/// Stream-scoped metadata needed to fill chat chunk defaults consistently.
#[derive(Debug, Clone, Default)]
pub(crate) struct ChatStreamMetadata {
    pub response_id: Option<String>,
    pub model: Option<String>,
    pub created: Option<i64>,
}

pub(crate) type GatewayHandlerResult<T> = Result<T, (axum::http::StatusCode, Json<ErrorResponse>)>;
pub(crate) type OpenAiChatAdaptedRequest =
    (serde_json::Map<String, Value>, BTreeMap<String, String>);

// === Account pool types ===

/// Admin request body for importing a Codex account into the pool.
#[derive(Debug, Clone, Deserialize)]
pub struct ImportAccountRequest {
    pub name: String,
    pub tokens: ImportAccountTokens,
}

/// Token fields needed to import a Codex account.
#[derive(Debug, Clone, Deserialize)]
pub struct ImportAccountTokens {
    pub id_token: String,
    pub access_token: String,
    pub refresh_token: String,
    #[serde(default)]
    pub account_id: Option<String>,
}

/// Admin-facing summary of one managed Codex account.
#[derive(Debug, Clone, Serialize)]
pub struct AccountSummaryView {
    pub name: String,
    pub status: String,
    pub account_id: Option<String>,
    pub plan_type: Option<String>,
    pub primary_remaining_percent: Option<f64>,
    pub secondary_remaining_percent: Option<f64>,
    pub map_gpt53_codex_to_spark: bool,
    pub proxy_mode: String,
    pub proxy_config_id: Option<String>,
    pub effective_proxy_source: String,
    pub effective_proxy_url: Option<String>,
    pub effective_proxy_config_name: Option<String>,
    pub last_refresh: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_usage_checked_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_usage_success_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_error_message: Option<String>,
}

/// Admin response listing all managed Codex accounts.
#[derive(Debug, Clone, Serialize)]
pub struct AccountListResponse {
    pub accounts: Vec<AccountSummaryView>,
    pub generated_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PatchAccountSettingsRequest {
    pub proxy_mode: Option<String>,
    pub proxy_config_id: Option<String>,
    pub map_gpt53_codex_to_spark: Option<bool>,
}

impl From<&LlmGatewayKeyRecord> for LlmGatewayPublicKeyView {
    fn from(value: &LlmGatewayKeyRecord) -> Self {
        Self {
            id: value.id.clone(),
            name: value.name.clone(),
            secret: value.secret.clone(),
            quota_billable_limit: value.quota_billable_limit,
            usage_input_uncached_tokens: value.usage_input_uncached_tokens,
            usage_input_cached_tokens: value.usage_input_cached_tokens,
            usage_output_tokens: value.usage_output_tokens,
            remaining_billable: value.remaining_billable(),
            last_used_at: value.last_used_at,
        }
    }
}

impl From<&LlmGatewayKeyRecord> for AdminLlmGatewayKeyView {
    fn from(value: &LlmGatewayKeyRecord) -> Self {
        Self {
            id: value.id.clone(),
            name: value.name.clone(),
            secret: value.secret.clone(),
            key_hash: value.key_hash.clone(),
            status: value.status.clone(),
            provider_type: value.provider_type.clone(),
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
            request_max_concurrency: value.request_max_concurrency,
            request_min_start_interval_ms: value.request_min_start_interval_ms,
        }
    }
}

impl From<&LlmGatewayKeyRecord> for PublicLlmGatewayUsageKeyView {
    fn from(value: &LlmGatewayKeyRecord) -> Self {
        Self {
            name: value.name.clone(),
            provider_type: value.provider_type.clone(),
            quota_billable_limit: value.quota_billable_limit,
            usage_input_uncached_tokens: value.usage_input_uncached_tokens,
            usage_input_cached_tokens: value.usage_input_cached_tokens,
            usage_output_tokens: value.usage_output_tokens,
            usage_billable_tokens: value.usage_billable_tokens,
            usage_credit_total: value.usage_credit_total,
            usage_credit_missing_events: value.usage_credit_missing_events,
            remaining_billable: value.remaining_billable().max(0),
            last_used_at: value.last_used_at,
        }
    }
}

impl From<&LlmGatewayUsageEventSummaryRecord> for AdminLlmGatewayUsageEventView {
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

impl From<&LlmGatewayUsageEventRecord> for AdminLlmGatewayUsageEventDetailView {
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

impl From<&LlmGatewayUsageEventRecord> for PublicLlmGatewayUsageEventView {
    fn from(value: &LlmGatewayUsageEventRecord) -> Self {
        Self {
            id: value.id.clone(),
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
            created_at: value.created_at,
        }
    }
}

impl From<&LlmGatewayProxyConfigRecord> for AdminUpstreamProxyConfigView {
    fn from(value: &LlmGatewayProxyConfigRecord) -> Self {
        Self {
            id: value.id.clone(),
            name: value.name.clone(),
            proxy_url: value.proxy_url.clone(),
            proxy_username: value.proxy_username.clone(),
            proxy_password: value.proxy_password.clone(),
            status: value.status.clone(),
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

impl From<&LlmGatewayTokenRequestRecord> for AdminLlmGatewayTokenRequestView {
    fn from(value: &LlmGatewayTokenRequestRecord) -> Self {
        Self {
            request_id: value.request_id.clone(),
            requester_email: value.requester_email.clone(),
            requested_quota_billable_limit: value.requested_quota_billable_limit,
            request_reason: value.request_reason.clone(),
            frontend_page_url: value.frontend_page_url.clone(),
            status: value.status.clone(),
            client_ip: value.client_ip.clone(),
            ip_region: value.ip_region.clone(),
            admin_note: value.admin_note.clone(),
            failure_reason: value.failure_reason.clone(),
            issued_key_id: value.issued_key_id.clone(),
            issued_key_name: value.issued_key_name.clone(),
            created_at: value.created_at,
            updated_at: value.updated_at,
            processed_at: value.processed_at,
        }
    }
}

impl From<&LlmGatewayAccountContributionRequestRecord>
    for AdminLlmGatewayAccountContributionRequestView
{
    fn from(value: &LlmGatewayAccountContributionRequestRecord) -> Self {
        Self {
            request_id: value.request_id.clone(),
            account_name: value.account_name.clone(),
            account_id: value.account_id.clone(),
            id_token: value.id_token.clone(),
            access_token: value.access_token.clone(),
            refresh_token: value.refresh_token.clone(),
            requester_email: value.requester_email.clone(),
            contributor_message: value.contributor_message.clone(),
            github_id: value.github_id.clone(),
            frontend_page_url: value.frontend_page_url.clone(),
            status: value.status.clone(),
            client_ip: value.client_ip.clone(),
            ip_region: value.ip_region.clone(),
            admin_note: value.admin_note.clone(),
            failure_reason: value.failure_reason.clone(),
            imported_account_name: value.imported_account_name.clone(),
            issued_key_id: value.issued_key_id.clone(),
            issued_key_name: value.issued_key_name.clone(),
            created_at: value.created_at,
            updated_at: value.updated_at,
            processed_at: value.processed_at,
        }
    }
}

impl From<&LlmGatewayAccountContributionRequestRecord> for PublicLlmGatewayAccountContributionView {
    fn from(value: &LlmGatewayAccountContributionRequestRecord) -> Self {
        Self {
            request_id: value.request_id.clone(),
            account_name: value
                .imported_account_name
                .clone()
                .unwrap_or_else(|| value.account_name.clone()),
            contributor_message: value.contributor_message.clone(),
            github_id: value.github_id.clone(),
            processed_at: value.processed_at,
        }
    }
}

impl From<&LlmGatewaySponsorRequestRecord> for AdminLlmGatewaySponsorRequestView {
    fn from(value: &LlmGatewaySponsorRequestRecord) -> Self {
        Self {
            request_id: value.request_id.clone(),
            requester_email: value.requester_email.clone(),
            sponsor_message: value.sponsor_message.clone(),
            display_name: value.display_name.clone(),
            github_id: value.github_id.clone(),
            frontend_page_url: value.frontend_page_url.clone(),
            status: value.status.clone(),
            client_ip: value.client_ip.clone(),
            ip_region: value.ip_region.clone(),
            admin_note: value.admin_note.clone(),
            failure_reason: value.failure_reason.clone(),
            payment_email_sent_at: value.payment_email_sent_at,
            created_at: value.created_at,
            updated_at: value.updated_at,
            processed_at: value.processed_at,
        }
    }
}

impl From<&LlmGatewaySponsorRequestRecord> for PublicLlmGatewaySponsorView {
    fn from(value: &LlmGatewaySponsorRequestRecord) -> Self {
        Self {
            request_id: value.request_id.clone(),
            display_name: value.display_name.clone(),
            sponsor_message: value.sponsor_message.clone(),
            github_id: value.github_id.clone(),
            processed_at: value.processed_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use static_flow_shared::llm_gateway_store::LlmGatewayUsageEventSummaryRecord;

    use super::*;

    #[test]
    fn admin_usage_event_view_preserves_last_message_preview() {
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

        let view = AdminLlmGatewayUsageEventView::from(&event);

        assert_eq!(view.id, "evt-1");
        assert_eq!(view.key_name, "alpha");
        assert_eq!(view.last_message_content.as_deref(), Some("hello"));
        assert_eq!(view.created_at, 123);
    }
}
