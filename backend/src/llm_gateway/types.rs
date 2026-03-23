use std::{collections::BTreeMap, time::Instant};

use axum::{http::Method, response::Json};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use static_flow_shared::llm_gateway_store::{LlmGatewayKeyRecord, LlmGatewayUsageEventRecord};

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

/// Public read-only payload for the cached Codex rate-limit snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmGatewayRateLimitStatusResponse {
    pub status: String,
    pub refresh_interval_seconds: u64,
    pub last_checked_at: Option<i64>,
    pub last_success_at: Option<i64>,
    pub source_url: String,
    pub error_message: Option<String>,
    pub buckets: Vec<LlmGatewayRateLimitBucketView>,
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
    pub public_visible: bool,
    pub quota_billable_limit: u64,
    pub usage_input_uncached_tokens: u64,
    pub usage_input_cached_tokens: u64,
    pub usage_output_tokens: u64,
    pub remaining_billable: i64,
    pub last_used_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Paginated admin response for settled usage events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminLlmGatewayUsageEventsResponse {
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
    pub has_more: bool,
    pub events: Vec<AdminLlmGatewayUsageEventView>,
    pub generated_at: i64,
}

/// Admin-facing usage event enriched with request diagnostics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminLlmGatewayUsageEventView {
    pub id: String,
    pub key_id: String,
    pub key_name: String,
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
    pub client_ip: String,
    pub ip_region: String,
    pub request_headers_json: String,
    pub created_at: i64,
}

/// Lightweight admin response for runtime gateway configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmGatewayRuntimeConfigResponse {
    pub auth_cache_ttl_seconds: u64,
}

/// Admin request body for updating runtime cache configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct UpdateLlmGatewayRuntimeConfigRequest {
    pub auth_cache_ttl_seconds: Option<u64>,
}

/// Admin request body for creating a new externally visible gateway key.
#[derive(Debug, Clone, Deserialize)]
pub struct CreateLlmGatewayKeyRequest {
    pub name: String,
    pub quota_billable_limit: u64,
    #[serde(default)]
    pub public_visible: bool,
}

/// Admin request body for mutating one existing gateway key.
#[derive(Debug, Clone, Deserialize)]
pub struct PatchLlmGatewayKeyRequest {
    pub name: Option<String>,
    pub status: Option<String>,
    pub public_visible: Option<bool>,
    pub quota_billable_limit: Option<u64>,
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
        self.input_uncached_tokens
            .saturating_add(self.output_tokens)
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
    pub request_body: axum::body::Bytes,
    pub model: Option<String>,
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
            public_visible: value.public_visible,
            quota_billable_limit: value.quota_billable_limit,
            usage_input_uncached_tokens: value.usage_input_uncached_tokens,
            usage_input_cached_tokens: value.usage_input_cached_tokens,
            usage_output_tokens: value.usage_output_tokens,
            remaining_billable: value.remaining_billable(),
            last_used_at: value.last_used_at,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

impl From<&LlmGatewayUsageEventRecord> for AdminLlmGatewayUsageEventView {
    fn from(value: &LlmGatewayUsageEventRecord) -> Self {
        Self {
            id: value.id.clone(),
            key_id: value.key_id.clone(),
            key_name: value.key_name.clone(),
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
            client_ip: value.client_ip.clone(),
            ip_region: value.ip_region.clone(),
            request_headers_json: value.request_headers_json.clone(),
            created_at: value.created_at,
        }
    }
}
