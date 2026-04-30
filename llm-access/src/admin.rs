//! Local admin compatibility endpoints for the standalone LLM access service.

use std::{
    collections::BTreeMap,
    net::IpAddr,
    time::{Duration, Instant},
};

use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use llm_access_core::{
    store::{
        self as core_store, AdminAccountGroupPatch, AdminCodexAccountPatch, AdminKeyPatch,
        AdminProxyConfigPatch, AdminReviewQueueAction, AdminRuntimeConfig, NewAdminAccountGroup,
        NewAdminCodexAccount, NewAdminKey, NewAdminProxyConfig, UpdateAdminRuntimeConfig,
        UsageEventQuery, KEY_STATUS_ACTIVE, KEY_STATUS_DISABLED, KIRO_PREFIX_CACHE_MODE_FORMULA,
        PROVIDER_CODEX, PROVIDER_KIRO,
    },
    usage::UsageEvent,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::HttpState;

const MAX_CODEX_CLIENT_VERSION_LEN: usize = 64;
const MAX_RUNTIME_CACHE_TTL_SECONDS: u64 = 86_400;
const MIN_RUNTIME_CACHE_TTL_SECONDS: u64 = 1;
const MAX_RUNTIME_REQUEST_BODY_BYTES: u64 = 256 * 1024 * 1024;
const MIN_RUNTIME_REQUEST_BODY_BYTES: u64 = 1024;
const MAX_RUNTIME_ACCOUNT_FAILURE_RETRY_LIMIT: u64 = 100;
const MIN_RUNTIME_ACCOUNT_FAILURE_RETRY_LIMIT: u64 = 0;
const MIN_RUNTIME_STATUS_REFRESH_INTERVAL_SECONDS: u64 = 240;
const MAX_RUNTIME_STATUS_REFRESH_INTERVAL_SECONDS: u64 = 3_600;
const MAX_RUNTIME_STATUS_ACCOUNT_JITTER_SECONDS: u64 = 60;
const MIN_RUNTIME_USAGE_EVENT_FLUSH_BATCH_SIZE: u64 = 1;
const MAX_RUNTIME_USAGE_EVENT_FLUSH_BATCH_SIZE: u64 = 16_384;
const MIN_RUNTIME_USAGE_EVENT_FLUSH_INTERVAL_SECONDS: u64 = 1;
const MAX_RUNTIME_USAGE_EVENT_FLUSH_INTERVAL_SECONDS: u64 = 3_600;
const MIN_RUNTIME_USAGE_EVENT_FLUSH_MAX_BUFFER_BYTES: u64 = 1_024;
const MAX_RUNTIME_USAGE_EVENT_FLUSH_MAX_BUFFER_BYTES: u64 = 256 * 1024 * 1024;
const MAX_CODEX_KEY_REQUEST_MAX_CONCURRENCY: u64 = 1_024;
const MAX_CODEX_KEY_REQUEST_MIN_START_INTERVAL_MS: u64 = 300_000;
const DEFAULT_ADMIN_REVIEW_QUEUE_LIMIT: usize = 50;
const MAX_ADMIN_REVIEW_QUEUE_LIMIT: usize = 200;
const DEFAULT_ADMIN_USAGE_LIMIT: usize = 50;
const MAX_ADMIN_USAGE_LIMIT: usize = 500;
const PROXY_CONNECTIVITY_CHECK_TIMEOUT_SECONDS: u64 = 10;
const BAND_CONTIGUITY_TOLERANCE: f64 = 1e-12;

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
    code: u16,
}

#[derive(Debug, Serialize)]
struct AdminKeysResponse {
    keys: Vec<core_store::AdminKey>,
    auth_cache_ttl_seconds: u64,
    generated_at: i64,
}

#[derive(Debug, Serialize)]
struct DeleteResponse {
    deleted: bool,
    id: String,
}

#[derive(Debug, Serialize)]
struct AdminAccountGroupsResponse {
    groups: Vec<core_store::AdminAccountGroup>,
    generated_at: i64,
}

#[derive(Debug, Serialize)]
struct AdminProxyConfigsResponse {
    proxy_configs: Vec<core_store::AdminProxyConfig>,
    generated_at: i64,
}

#[derive(Debug, Serialize)]
struct AdminProxyBindingsResponse {
    bindings: Vec<core_store::AdminProxyBinding>,
    generated_at: i64,
}

#[derive(Debug, Serialize)]
struct AdminAccountsResponse {
    accounts: Vec<core_store::AdminCodexAccount>,
    generated_at: i64,
}

#[derive(Debug, Serialize)]
struct AdminTokenRequestsResponse {
    total: usize,
    offset: usize,
    limit: usize,
    has_more: bool,
    requests: Vec<core_store::AdminTokenRequest>,
    generated_at: i64,
}

#[derive(Debug, Serialize)]
struct AdminAccountContributionRequestsResponse {
    total: usize,
    offset: usize,
    limit: usize,
    has_more: bool,
    requests: Vec<core_store::AdminAccountContributionRequest>,
    generated_at: i64,
}

#[derive(Debug, Serialize)]
struct AdminSponsorRequestsResponse {
    total: usize,
    offset: usize,
    limit: usize,
    has_more: bool,
    requests: Vec<core_store::AdminSponsorRequest>,
    generated_at: i64,
}

#[derive(Debug, Serialize)]
struct AdminUsageEventsResponse {
    total: usize,
    offset: usize,
    limit: usize,
    has_more: bool,
    current_rpm: u32,
    current_in_flight: u32,
    events: Vec<AdminUsageEventView>,
    generated_at: i64,
}

#[derive(Debug, Serialize)]
struct AdminUsageEventView {
    id: String,
    key_id: String,
    key_name: String,
    account_name: Option<String>,
    request_method: String,
    request_url: String,
    latency_ms: i32,
    routing_wait_ms: Option<i32>,
    upstream_headers_ms: Option<i32>,
    post_headers_body_ms: Option<i32>,
    request_body_bytes: Option<u64>,
    request_body_read_ms: Option<i32>,
    request_json_parse_ms: Option<i32>,
    pre_handler_ms: Option<i32>,
    first_sse_write_ms: Option<i32>,
    stream_finish_ms: Option<i32>,
    other_latency_ms: Option<i32>,
    quota_failover_count: u64,
    routing_diagnostics_json: Option<String>,
    endpoint: String,
    model: Option<String>,
    status_code: i32,
    input_uncached_tokens: u64,
    input_cached_tokens: u64,
    output_tokens: u64,
    billable_tokens: u64,
    usage_missing: bool,
    credit_usage: Option<f64>,
    credit_usage_missing: bool,
    client_ip: String,
    ip_region: String,
    last_message_content: Option<String>,
    created_at: i64,
}

#[derive(Debug, Serialize)]
struct AdminUsageEventDetailView {
    #[serde(flatten)]
    event: AdminUsageEventView,
    request_headers_json: String,
    client_request_body_json: Option<String>,
    upstream_request_body_json: Option<String>,
    full_request_json: Option<String>,
}

#[derive(Debug, Serialize)]
struct AdminProxyCheckTargetView {
    target: String,
    url: String,
    reachable: bool,
    status_code: Option<u16>,
    latency_ms: i64,
    error_message: Option<String>,
}

#[derive(Debug, Serialize)]
struct AdminProxyCheckResponse {
    proxy_config_id: String,
    proxy_config_name: String,
    provider_type: String,
    auth_label: String,
    ok: bool,
    targets: Vec<AdminProxyCheckTargetView>,
    checked_at: i64,
}

#[derive(Debug, Serialize)]
struct AdminLegacyKiroProxyMigrationResponse {
    created_configs: Vec<core_store::AdminProxyConfig>,
    reused_configs: Vec<core_store::AdminProxyConfig>,
    migrated_account_names: Vec<String>,
    generated_at: i64,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ListUsageEventsRequest {
    #[serde(default)]
    key_id: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    offset: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ListReviewQueueRequest {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    offset: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ReviewQueueActionRequest {
    #[serde(default)]
    admin_note: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreateLlmGatewayKeyRequest {
    name: String,
    quota_billable_limit: u64,
    #[serde(default)]
    public_visible: bool,
    #[serde(default)]
    request_max_concurrency: Option<u64>,
    #[serde(default)]
    request_min_start_interval_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PatchLlmGatewayKeyRequest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    public_visible: Option<bool>,
    #[serde(default)]
    quota_billable_limit: Option<u64>,
    #[serde(default)]
    route_strategy: Option<String>,
    #[serde(default)]
    account_group_id: Option<String>,
    #[serde(default)]
    fixed_account_name: Option<String>,
    #[serde(default)]
    auto_account_names: Option<Vec<String>>,
    #[serde(default)]
    model_name_map: Option<BTreeMap<String, String>>,
    #[serde(default)]
    request_max_concurrency: Option<u64>,
    #[serde(default)]
    request_min_start_interval_ms: Option<u64>,
    #[serde(default)]
    request_max_concurrency_unlimited: bool,
    #[serde(default)]
    request_min_start_interval_ms_unlimited: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreateLlmGatewayAccountGroupRequest {
    name: String,
    account_names: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PatchLlmGatewayAccountGroupRequest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    account_names: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreateLlmGatewayProxyConfigRequest {
    name: String,
    proxy_url: String,
    #[serde(default)]
    proxy_username: Option<String>,
    #[serde(default)]
    proxy_password: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PatchLlmGatewayProxyConfigRequest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    proxy_url: Option<String>,
    #[serde(default)]
    proxy_username: Option<String>,
    #[serde(default)]
    proxy_password: Option<String>,
    #[serde(default)]
    status: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct UpdateLlmGatewayProxyBindingRequest {
    #[serde(default)]
    proxy_config_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ImportLlmGatewayAccountRequest {
    name: String,
    tokens: ImportLlmGatewayAccountTokens,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ImportLlmGatewayAccountTokens {
    id_token: String,
    access_token: String,
    refresh_token: String,
    #[serde(default)]
    account_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PatchLlmGatewayAccountRequest {
    #[serde(default)]
    proxy_mode: Option<String>,
    #[serde(default)]
    proxy_config_id: Option<String>,
    #[serde(default)]
    map_gpt53_codex_to_spark: Option<bool>,
    #[serde(default)]
    request_max_concurrency: Option<u64>,
    #[serde(default)]
    request_min_start_interval_ms: Option<u64>,
    #[serde(default)]
    request_max_concurrency_unlimited: bool,
    #[serde(default)]
    request_min_start_interval_ms_unlimited: bool,
}

#[derive(Debug)]
struct AdminHttpError {
    status: StatusCode,
    message: String,
}

impl IntoResponse for AdminHttpError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorResponse {
                error: self.message,
                code: self.status.as_u16(),
            }),
        )
            .into_response()
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct KiroCachePolicy {
    small_input_high_credit_boost: KiroSmallInputHighCreditBoostPolicy,
    prefix_tree_credit_ratio_bands: Vec<KiroCreditRatioBand>,
    high_credit_diagnostic_threshold: f64,
    #[serde(default)]
    anthropic_cache_creation_input_ratio: f64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct KiroSmallInputHighCreditBoostPolicy {
    target_input_tokens: u64,
    credit_start: f64,
    credit_end: f64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct KiroCreditRatioBand {
    credit_start: f64,
    credit_end: f64,
    cache_ratio_start: f64,
    cache_ratio_end: f64,
}

pub(crate) async fn get_llm_gateway_config(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    match state.admin_config_store.get_admin_runtime_config().await {
        Ok(config) => Json(config).into_response(),
        Err(_) => internal_error("Failed to load llm gateway config").into_response(),
    }
}

pub(crate) async fn post_llm_gateway_config(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<UpdateAdminRuntimeConfig>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let current = match state.admin_config_store.get_admin_runtime_config().await {
        Ok(config) => config,
        Err(_) => return internal_error("Failed to load llm gateway config").into_response(),
    };
    let config = match apply_runtime_config_update(current, request) {
        Ok(config) => config,
        Err(response) => return response.into_response(),
    };
    match state
        .admin_config_store
        .update_admin_runtime_config(config)
        .await
    {
        Ok(config) => Json(config).into_response(),
        Err(_) => internal_error("Failed to update llm gateway config").into_response(),
    }
}

pub(crate) async fn list_llm_gateway_keys(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let keys = match state.admin_key_store.list_admin_keys().await {
        Ok(keys) => keys,
        Err(_) => return internal_error("Failed to list llm gateway keys").into_response(),
    };
    let auth_cache_ttl_seconds = match state.admin_config_store.get_admin_runtime_config().await {
        Ok(config) => config.auth_cache_ttl_seconds,
        Err(_) => return internal_error("Failed to load llm gateway config").into_response(),
    };
    Json(AdminKeysResponse {
        keys,
        auth_cache_ttl_seconds,
        generated_at: now_ms(),
    })
    .into_response()
}

pub(crate) async fn create_llm_gateway_key(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<CreateLlmGatewayKeyRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let name = match normalize_name(&request.name) {
        Ok(name) => name,
        Err(response) => return response.into_response(),
    };
    if let Err(response) =
        validate_i64_backed_u64("quota_billable_limit", request.quota_billable_limit)
    {
        return response.into_response();
    }
    if let Err(response) = validate_codex_request_limit_inputs(
        request.request_max_concurrency,
        request.request_min_start_interval_ms,
    ) {
        return response.into_response();
    }
    let secret = generate_secret();
    let key = NewAdminKey {
        id: generate_id("llm-key"),
        name,
        key_hash: sha256_hex(secret.as_bytes()),
        secret,
        public_visible: request.public_visible,
        quota_billable_limit: request.quota_billable_limit,
        request_max_concurrency: request.request_max_concurrency,
        request_min_start_interval_ms: request.request_min_start_interval_ms,
        created_at_ms: now_ms(),
    };
    match state.admin_key_store.create_admin_key(key).await {
        Ok(key) => Json(key).into_response(),
        Err(_) => internal_error("Failed to create llm gateway key").into_response(),
    }
}

pub(crate) async fn patch_llm_gateway_key(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(key_id): Path<String>,
    Json(request): Json<PatchLlmGatewayKeyRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let patch = match normalize_key_patch(request) {
        Ok(patch) => patch,
        Err(response) => return response.into_response(),
    };
    match state.admin_key_store.patch_admin_key(&key_id, patch).await {
        Ok(Some(key)) => Json(key).into_response(),
        Ok(None) => not_found("LLM gateway key not found").into_response(),
        Err(_) => internal_error("Failed to update llm gateway key").into_response(),
    }
}

pub(crate) async fn delete_llm_gateway_key(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(key_id): Path<String>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    match state.admin_key_store.delete_admin_key(&key_id).await {
        Ok(Some(key)) => Json(DeleteResponse {
            deleted: true,
            id: key.id,
        })
        .into_response(),
        Ok(None) => not_found("LLM gateway key not found").into_response(),
        Err(_) => internal_error("Failed to delete llm gateway key").into_response(),
    }
}

pub(crate) async fn list_llm_gateway_account_groups(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    match state
        .admin_account_group_store
        .list_admin_account_groups(PROVIDER_CODEX)
        .await
    {
        Ok(groups) => Json(AdminAccountGroupsResponse {
            groups,
            generated_at: now_ms(),
        })
        .into_response(),
        Err(_) => internal_error("Failed to list llm gateway account groups").into_response(),
    }
}

pub(crate) async fn create_llm_gateway_account_group(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<CreateLlmGatewayAccountGroupRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let name = match normalize_name(&request.name) {
        Ok(name) => name,
        Err(response) => return response.into_response(),
    };
    let account_names = match normalize_account_names(request.account_names) {
        Ok(Some(names)) => names,
        Ok(None) => return bad_request("account_names must not be empty").into_response(),
        Err(response) => return response.into_response(),
    };
    let group = NewAdminAccountGroup {
        id: generate_id("llm-group"),
        provider_type: PROVIDER_CODEX.to_string(),
        name,
        account_names,
        created_at_ms: now_ms(),
    };
    match state
        .admin_account_group_store
        .create_admin_account_group(group)
        .await
    {
        Ok(group) => Json(group).into_response(),
        Err(_) => internal_error("Failed to create llm gateway account group").into_response(),
    }
}

pub(crate) async fn patch_llm_gateway_account_group(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(group_id): Path<String>,
    Json(request): Json<PatchLlmGatewayAccountGroupRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let name = match request.name.as_deref().map(normalize_name).transpose() {
        Ok(name) => name,
        Err(response) => return response.into_response(),
    };
    let account_names = match request
        .account_names
        .map(normalize_account_names)
        .transpose()
    {
        Ok(value) => value.flatten(),
        Err(response) => return response.into_response(),
    };
    let patch = AdminAccountGroupPatch {
        name,
        account_names,
        updated_at_ms: now_ms(),
    };
    match state
        .admin_account_group_store
        .patch_admin_account_group(&group_id, patch)
        .await
    {
        Ok(Some(group)) => Json(group).into_response(),
        Ok(None) => not_found("LLM gateway account group not found").into_response(),
        Err(_) => internal_error("Failed to update llm gateway account group").into_response(),
    }
}

pub(crate) async fn delete_llm_gateway_account_group(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(group_id): Path<String>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let keys = match state.admin_key_store.list_admin_keys().await {
        Ok(keys) => keys,
        Err(_) => return internal_error("Failed to inspect llm gateway keys").into_response(),
    };
    if let Some(key) = keys
        .iter()
        .find(|key| key.account_group_id.as_deref() == Some(group_id.as_str()))
    {
        return bad_request(&format!("account group is still referenced by key `{}`", key.name))
            .into_response();
    }
    match state
        .admin_account_group_store
        .delete_admin_account_group(&group_id)
        .await
    {
        Ok(Some(group)) => Json(DeleteResponse {
            deleted: true,
            id: group.id,
        })
        .into_response(),
        Ok(None) => not_found("LLM gateway account group not found").into_response(),
        Err(_) => internal_error("Failed to delete llm gateway account group").into_response(),
    }
}

pub(crate) async fn list_llm_gateway_proxy_configs(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    match state.admin_proxy_store.list_admin_proxy_configs().await {
        Ok(proxy_configs) => Json(AdminProxyConfigsResponse {
            proxy_configs,
            generated_at: now_ms(),
        })
        .into_response(),
        Err(_) => internal_error("Failed to list llm gateway proxy configs").into_response(),
    }
}

pub(crate) async fn create_llm_gateway_proxy_config(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<CreateLlmGatewayProxyConfigRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let name = match normalize_name(&request.name) {
        Ok(name) => name,
        Err(response) => return response.into_response(),
    };
    let proxy_url = match normalize_required_proxy_url(&request.proxy_url) {
        Ok(proxy_url) => proxy_url,
        Err(response) => return response.into_response(),
    };
    let proxy = NewAdminProxyConfig {
        id: generate_id("llm-proxy"),
        name,
        proxy_url,
        proxy_username: normalize_optional_string_option(request.proxy_username.as_deref()),
        proxy_password: normalize_optional_string_option(request.proxy_password.as_deref()),
        created_at_ms: now_ms(),
    };
    match state
        .admin_proxy_store
        .create_admin_proxy_config(proxy)
        .await
    {
        Ok(proxy) => Json(proxy).into_response(),
        Err(_) => internal_error("Failed to create llm gateway proxy config").into_response(),
    }
}

pub(crate) async fn patch_llm_gateway_proxy_config(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(proxy_id): Path<String>,
    Json(request): Json<PatchLlmGatewayProxyConfigRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let patch = match normalize_proxy_config_patch(request) {
        Ok(patch) => patch,
        Err(response) => return response.into_response(),
    };
    match state
        .admin_proxy_store
        .patch_admin_proxy_config(&proxy_id, patch)
        .await
    {
        Ok(Some(proxy)) => Json(proxy).into_response(),
        Ok(None) => not_found("LLM gateway proxy config not found").into_response(),
        Err(_) => internal_error("Failed to update llm gateway proxy config").into_response(),
    }
}

pub(crate) async fn delete_llm_gateway_proxy_config(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(proxy_id): Path<String>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let bindings = match state.admin_proxy_store.list_admin_proxy_bindings().await {
        Ok(bindings) => bindings,
        Err(_) => {
            return internal_error("Failed to inspect llm gateway proxy bindings").into_response()
        },
    };
    if let Some(binding) = bindings
        .iter()
        .find(|binding| binding.bound_proxy_config_id.as_deref() == Some(proxy_id.as_str()))
    {
        return conflict(&format!(
            "proxy config is still bound to provider `{}`",
            binding.provider_type
        ))
        .into_response();
    }
    match state
        .admin_proxy_store
        .delete_admin_proxy_config(&proxy_id)
        .await
    {
        Ok(Some(proxy)) => Json(DeleteResponse {
            deleted: true,
            id: proxy.id,
        })
        .into_response(),
        Ok(None) => not_found("LLM gateway proxy config not found").into_response(),
        Err(_) => internal_error("Failed to delete llm gateway proxy config").into_response(),
    }
}

pub(crate) async fn list_llm_gateway_proxy_bindings(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    match state.admin_proxy_store.list_admin_proxy_bindings().await {
        Ok(bindings) => Json(AdminProxyBindingsResponse {
            bindings,
            generated_at: now_ms(),
        })
        .into_response(),
        Err(_) => internal_error("Failed to list llm gateway proxy bindings").into_response(),
    }
}

pub(crate) async fn update_llm_gateway_proxy_binding(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(provider_type): Path<String>,
    Json(request): Json<UpdateLlmGatewayProxyBindingRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    if let Err(response) = validate_provider_type(&provider_type) {
        return response.into_response();
    }
    let proxy_config_id = normalize_optional_string_option(request.proxy_config_id.as_deref());
    if let Some(proxy_id) = proxy_config_id.as_deref() {
        let proxy = match state
            .admin_proxy_store
            .get_admin_proxy_config(proxy_id)
            .await
        {
            Ok(Some(proxy)) => proxy,
            Ok(None) => return not_found("LLM gateway proxy config not found").into_response(),
            Err(_) => {
                return internal_error("Failed to load llm gateway proxy config").into_response()
            },
        };
        if proxy.status != KEY_STATUS_ACTIVE {
            return bad_request("proxy config must be active before binding").into_response();
        }
    }
    match state
        .admin_proxy_store
        .update_admin_proxy_binding(&provider_type, proxy_config_id)
        .await
    {
        Ok(binding) => Json(binding).into_response(),
        Err(_) => internal_error("Failed to update llm gateway proxy binding").into_response(),
    }
}

pub(crate) async fn check_llm_gateway_proxy_config(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((proxy_id, provider_type)): Path<(String, String)>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    if let Err(response) = validate_provider_type(&provider_type) {
        return response.into_response();
    }
    let proxy = match state
        .admin_proxy_store
        .get_admin_proxy_config(&proxy_id)
        .await
    {
        Ok(Some(proxy)) => proxy,
        Ok(None) => return not_found("LLM gateway proxy config not found").into_response(),
        Err(_) => return internal_error("Failed to load llm gateway proxy config").into_response(),
    };
    match run_proxy_connectivity_check(&proxy, &provider_type).await {
        Ok(result) => Json(result).into_response(),
        Err(_) => internal_error("Failed to check upstream proxy config").into_response(),
    }
}

pub(crate) async fn import_legacy_kiro_proxy_configs(
    State(_state): State<HttpState>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    Json(AdminLegacyKiroProxyMigrationResponse {
        created_configs: Vec::new(),
        reused_configs: Vec::new(),
        migrated_account_names: Vec::new(),
        generated_at: now_ms(),
    })
    .into_response()
}

pub(crate) async fn list_llm_gateway_usage_events(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(request): Query<ListUsageEventsRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let query = normalize_usage_query(request);
    match state.usage_analytics_store.list_usage_events(query).await {
        Ok(page) => Json(AdminUsageEventsResponse {
            total: page.total,
            offset: page.offset,
            limit: page.limit,
            has_more: page.has_more,
            current_rpm: 0,
            current_in_flight: 0,
            events: page.events.iter().map(AdminUsageEventView::from).collect(),
            generated_at: now_ms(),
        })
        .into_response(),
        Err(_) => internal_error("Failed to list llm gateway usage events").into_response(),
    }
}

pub(crate) async fn get_llm_gateway_usage_event(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(event_id): Path<String>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    match state.usage_analytics_store.get_usage_event(&event_id).await {
        Ok(Some(event)) => Json(AdminUsageEventDetailView {
            event: AdminUsageEventView::from(&event),
            request_headers_json: "{}".to_string(),
            client_request_body_json: None,
            upstream_request_body_json: None,
            full_request_json: None,
        })
        .into_response(),
        Ok(None) => not_found("LLM gateway usage event not found").into_response(),
        Err(_) => internal_error("Failed to load llm gateway usage event").into_response(),
    }
}

pub(crate) async fn list_llm_gateway_accounts(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    match state
        .admin_codex_account_store
        .list_admin_codex_accounts()
        .await
    {
        Ok(accounts) => Json(AdminAccountsResponse {
            accounts,
            generated_at: now_ms(),
        })
        .into_response(),
        Err(_) => internal_error("Failed to list llm gateway accounts").into_response(),
    }
}

pub(crate) async fn import_llm_gateway_account(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<ImportLlmGatewayAccountRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let name = match normalize_account_name(&request.name) {
        Ok(name) => name,
        Err(response) => return response.into_response(),
    };
    let id_token = request.tokens.id_token.trim().to_string();
    let access_token = request.tokens.access_token.trim().to_string();
    let refresh_token = request.tokens.refresh_token.trim().to_string();
    if access_token.is_empty() {
        return bad_request("access_token is required").into_response();
    }
    if refresh_token.is_empty() {
        return bad_request("refresh_token is required").into_response();
    }
    if id_token.is_empty() {
        return bad_request("id_token is required").into_response();
    }
    let account_id = normalize_optional_string_option(request.tokens.account_id.as_deref());
    let auth_json = match serde_json::to_string(&serde_json::json!({
        "id_token": id_token,
        "access_token": access_token,
        "refresh_token": refresh_token,
        "account_id": account_id,
    })) {
        Ok(value) => value,
        Err(_) => return internal_error("Failed to encode account auth").into_response(),
    };
    let account = NewAdminCodexAccount {
        name,
        account_id,
        auth_json,
        map_gpt53_codex_to_spark: false,
        created_at_ms: now_ms(),
    };
    match state
        .admin_codex_account_store
        .create_admin_codex_account(account)
        .await
    {
        Ok(account) => Json(account).into_response(),
        Err(_) => internal_error("Failed to import llm gateway account").into_response(),
    }
}

pub(crate) async fn patch_llm_gateway_account(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Json(request): Json<PatchLlmGatewayAccountRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let name = match normalize_account_name(&name) {
        Ok(name) => name,
        Err(response) => return response.into_response(),
    };
    let patch = match normalize_account_patch(request) {
        Ok(patch) => patch,
        Err(response) => return response.into_response(),
    };
    if let Some(Some(proxy_id)) = patch.proxy_config_id.as_ref() {
        let proxy = match state
            .admin_proxy_store
            .get_admin_proxy_config(proxy_id)
            .await
        {
            Ok(Some(proxy)) => proxy,
            Ok(None) => return not_found("LLM gateway proxy config not found").into_response(),
            Err(_) => {
                return internal_error("Failed to load llm gateway proxy config").into_response()
            },
        };
        if proxy.status != KEY_STATUS_ACTIVE {
            return bad_request("proxy config must be active before account binding")
                .into_response();
        }
    }
    match state
        .admin_codex_account_store
        .patch_admin_codex_account(&name, patch)
        .await
    {
        Ok(Some(account)) => Json(account).into_response(),
        Ok(None) => not_found("LLM gateway account not found").into_response(),
        Err(_) => internal_error("Failed to update llm gateway account").into_response(),
    }
}

pub(crate) async fn delete_llm_gateway_account(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let name = match normalize_account_name(&name) {
        Ok(name) => name,
        Err(response) => return response.into_response(),
    };
    match state
        .admin_codex_account_store
        .delete_admin_codex_account(&name)
        .await
    {
        Ok(Some(account)) => Json(DeleteResponse {
            deleted: true,
            id: account.name,
        })
        .into_response(),
        Ok(None) => not_found("LLM gateway account not found").into_response(),
        Err(_) => internal_error("Failed to delete llm gateway account").into_response(),
    }
}

pub(crate) async fn refresh_llm_gateway_account(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let name = match normalize_account_name(&name) {
        Ok(name) => name,
        Err(response) => return response.into_response(),
    };
    match state
        .admin_codex_account_store
        .refresh_admin_codex_account(&name, now_ms())
        .await
    {
        Ok(Some(account)) => Json(account).into_response(),
        Ok(None) => not_found("LLM gateway account not found").into_response(),
        Err(_) => internal_error("Failed to refresh llm gateway account").into_response(),
    }
}

pub(crate) async fn list_llm_gateway_token_requests(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(request): Query<ListReviewQueueRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let query = normalize_review_queue_query(request);
    match state
        .admin_review_queue_store
        .list_admin_token_requests(query)
        .await
    {
        Ok(page) => Json(AdminTokenRequestsResponse {
            total: page.total,
            offset: page.offset,
            limit: page.limit,
            has_more: page.has_more,
            requests: page.requests,
            generated_at: now_ms(),
        })
        .into_response(),
        Err(_) => internal_error("Failed to list llm gateway token requests").into_response(),
    }
}

pub(crate) async fn list_llm_gateway_account_contribution_requests(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(request): Query<ListReviewQueueRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let query = normalize_review_queue_query(request);
    match state
        .admin_review_queue_store
        .list_admin_account_contribution_requests(query)
        .await
    {
        Ok(page) => Json(AdminAccountContributionRequestsResponse {
            total: page.total,
            offset: page.offset,
            limit: page.limit,
            has_more: page.has_more,
            requests: page.requests,
            generated_at: now_ms(),
        })
        .into_response(),
        Err(_) => internal_error("Failed to list llm gateway account contribution requests")
            .into_response(),
    }
}

pub(crate) async fn list_llm_gateway_sponsor_requests(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(request): Query<ListReviewQueueRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let query = normalize_review_queue_query(request);
    match state
        .admin_review_queue_store
        .list_admin_sponsor_requests(query)
        .await
    {
        Ok(page) => Json(AdminSponsorRequestsResponse {
            total: page.total,
            offset: page.offset,
            limit: page.limit,
            has_more: page.has_more,
            requests: page.requests,
            generated_at: now_ms(),
        })
        .into_response(),
        Err(_) => internal_error("Failed to list llm gateway sponsor requests").into_response(),
    }
}

pub(crate) async fn approve_and_issue_llm_gateway_token_request(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
    Json(request): Json<ReviewQueueActionRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let current = match state
        .admin_review_queue_store
        .get_admin_token_request(&request_id)
        .await
    {
        Ok(Some(request)) => request,
        Ok(None) => return not_found("LLM gateway token request not found").into_response(),
        Err(_) => {
            return internal_error("Failed to load llm gateway token request").into_response()
        },
    };
    if matches!(current.status.as_str(), "issued" | "rejected") {
        return conflict("LLM gateway token request is finalized").into_response();
    }
    let key = if current.issued_key_id.is_none() {
        let secret = generate_secret();
        Some(NewAdminKey {
            id: generate_id("llm-key"),
            name: normalize_name(&format!("wish-{}", current.request_id))
                .unwrap_or_else(|_| format!("wish-{}", current.request_id)),
            key_hash: sha256_hex(secret.as_bytes()),
            secret,
            public_visible: false,
            quota_billable_limit: current.requested_quota_billable_limit,
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
            created_at_ms: now_ms(),
        })
    } else {
        None
    };
    match state
        .admin_review_queue_store
        .issue_admin_token_request(&request_id, key, review_queue_action(request))
        .await
    {
        Ok(Some(request)) => Json(request).into_response(),
        Ok(None) => not_found("LLM gateway token request not found").into_response(),
        Err(_) => internal_error("Failed to issue llm gateway token request").into_response(),
    }
}

pub(crate) async fn reject_llm_gateway_token_request(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
    Json(request): Json<ReviewQueueActionRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let current = match state
        .admin_review_queue_store
        .get_admin_token_request(&request_id)
        .await
    {
        Ok(Some(request)) => request,
        Ok(None) => return not_found("LLM gateway token request not found").into_response(),
        Err(_) => {
            return internal_error("Failed to load llm gateway token request").into_response()
        },
    };
    if current.status == "issued" {
        return conflict("Issued LLM gateway token request cannot be rejected").into_response();
    }
    if current.status == "rejected" {
        return conflict("LLM gateway token request is already rejected").into_response();
    }
    match state
        .admin_review_queue_store
        .reject_admin_token_request(&request_id, review_queue_action(request))
        .await
    {
        Ok(Some(request)) => Json(request).into_response(),
        Ok(None) => not_found("LLM gateway token request not found").into_response(),
        Err(_) => internal_error("Failed to reject llm gateway token request").into_response(),
    }
}

pub(crate) async fn approve_and_issue_llm_gateway_account_contribution_request(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
    Json(request): Json<ReviewQueueActionRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let current = match state
        .admin_review_queue_store
        .get_admin_account_contribution_request(&request_id)
        .await
    {
        Ok(Some(request)) => request,
        Ok(None) => {
            return not_found("LLM gateway account contribution request not found").into_response()
        },
        Err(_) => {
            return internal_error("Failed to load llm gateway account contribution request")
                .into_response()
        },
    };
    if matches!(current.status.as_str(), "issued" | "rejected") {
        return conflict("LLM gateway account contribution request is finalized").into_response();
    }
    let action = review_queue_action(request);
    let imported_account_name = current
        .imported_account_name
        .clone()
        .unwrap_or_else(|| current.account_name.clone());
    let account = if current.imported_account_name.is_none() {
        let auth_json = match serde_json::to_string(&serde_json::json!({
            "id_token": current.id_token,
            "access_token": current.access_token,
            "refresh_token": current.refresh_token,
            "account_id": current.account_id,
        })) {
            Ok(value) => value,
            Err(_) => return internal_error("Failed to encode account auth").into_response(),
        };
        Some(NewAdminCodexAccount {
            name: imported_account_name.clone(),
            account_id: current.account_id.clone(),
            auth_json,
            map_gpt53_codex_to_spark: false,
            created_at_ms: action.updated_at_ms,
        })
    } else {
        None
    };
    let (account_group, key) = if current.issued_key_id.is_none() {
        let group_id = generate_id("llm-group");
        let name = format!("contrib-{}", current.request_id);
        let secret = generate_secret();
        (
            Some(NewAdminAccountGroup {
                id: group_id,
                provider_type: PROVIDER_CODEX.to_string(),
                name: name.clone(),
                account_names: vec![imported_account_name],
                created_at_ms: action.updated_at_ms,
            }),
            Some(NewAdminKey {
                id: generate_id("llm-key"),
                name,
                key_hash: sha256_hex(secret.as_bytes()),
                secret,
                public_visible: false,
                quota_billable_limit: 100_000_000_000,
                request_max_concurrency: None,
                request_min_start_interval_ms: None,
                created_at_ms: action.updated_at_ms,
            }),
        )
    } else {
        (None, None)
    };
    match state
        .admin_review_queue_store
        .issue_admin_account_contribution_request(&request_id, account, account_group, key, action)
        .await
    {
        Ok(Some(request)) => Json(request).into_response(),
        Ok(None) => not_found("LLM gateway account contribution request not found").into_response(),
        Err(_) => internal_error("Failed to issue llm gateway account contribution request")
            .into_response(),
    }
}

pub(crate) async fn reject_llm_gateway_account_contribution_request(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
    Json(request): Json<ReviewQueueActionRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let current = match state
        .admin_review_queue_store
        .get_admin_account_contribution_request(&request_id)
        .await
    {
        Ok(Some(request)) => request,
        Ok(None) => {
            return not_found("LLM gateway account contribution request not found").into_response()
        },
        Err(_) => {
            return internal_error("Failed to load llm gateway account contribution request")
                .into_response()
        },
    };
    if current.status == "issued" {
        return conflict("Issued LLM gateway account contribution request cannot be rejected")
            .into_response();
    }
    if current.status == "rejected" {
        return conflict("LLM gateway account contribution request is already rejected")
            .into_response();
    }
    match state
        .admin_review_queue_store
        .reject_admin_account_contribution_request(&request_id, review_queue_action(request))
        .await
    {
        Ok(Some(request)) => Json(request).into_response(),
        Ok(None) => not_found("LLM gateway account contribution request not found").into_response(),
        Err(_) => internal_error("Failed to reject llm gateway account contribution request")
            .into_response(),
    }
}

pub(crate) async fn approve_llm_gateway_sponsor_request(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
    Json(request): Json<ReviewQueueActionRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let current = match state
        .admin_review_queue_store
        .get_admin_sponsor_request(&request_id)
        .await
    {
        Ok(Some(request)) => request,
        Ok(None) => return not_found("LLM gateway sponsor request not found").into_response(),
        Err(_) => {
            return internal_error("Failed to load llm gateway sponsor request").into_response()
        },
    };
    if current.status == "approved" {
        return conflict("LLM gateway sponsor request is already approved").into_response();
    }
    match state
        .admin_review_queue_store
        .approve_admin_sponsor_request(&request_id, review_queue_action(request))
        .await
    {
        Ok(Some(request)) => Json(request).into_response(),
        Ok(None) => not_found("LLM gateway sponsor request not found").into_response(),
        Err(_) => internal_error("Failed to approve llm gateway sponsor request").into_response(),
    }
}

pub(crate) async fn delete_llm_gateway_sponsor_request(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    match state
        .admin_review_queue_store
        .delete_admin_sponsor_request(&request_id)
        .await
    {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => not_found("LLM gateway sponsor request not found").into_response(),
        Err(_) => internal_error("Failed to delete llm gateway sponsor request").into_response(),
    }
}

fn apply_runtime_config_update(
    current: AdminRuntimeConfig,
    request: UpdateAdminRuntimeConfig,
) -> Result<AdminRuntimeConfig, AdminHttpError> {
    let auth_cache_ttl_seconds = request
        .auth_cache_ttl_seconds
        .unwrap_or(current.auth_cache_ttl_seconds);
    validate_range(
        "auth_cache_ttl_seconds",
        auth_cache_ttl_seconds,
        MIN_RUNTIME_CACHE_TTL_SECONDS,
        MAX_RUNTIME_CACHE_TTL_SECONDS,
    )?;

    let max_request_body_bytes = request
        .max_request_body_bytes
        .unwrap_or(current.max_request_body_bytes);
    validate_range(
        "max_request_body_bytes",
        max_request_body_bytes,
        MIN_RUNTIME_REQUEST_BODY_BYTES,
        MAX_RUNTIME_REQUEST_BODY_BYTES,
    )?;

    let account_failure_retry_limit = request
        .account_failure_retry_limit
        .unwrap_or(current.account_failure_retry_limit);
    validate_range(
        "account_failure_retry_limit",
        account_failure_retry_limit,
        MIN_RUNTIME_ACCOUNT_FAILURE_RETRY_LIMIT,
        MAX_RUNTIME_ACCOUNT_FAILURE_RETRY_LIMIT,
    )?;

    let codex_client_version = match request.codex_client_version.as_deref() {
        Some(value) => normalize_codex_client_version(value)
            .ok_or_else(|| bad_request("codex_client_version is invalid"))?,
        None => current.codex_client_version,
    };

    let codex_status_refresh_min_interval_seconds = request
        .codex_status_refresh_min_interval_seconds
        .unwrap_or(current.codex_status_refresh_min_interval_seconds);
    let codex_status_refresh_max_interval_seconds = request
        .codex_status_refresh_max_interval_seconds
        .unwrap_or(current.codex_status_refresh_max_interval_seconds);
    validate_runtime_refresh_window(
        codex_status_refresh_min_interval_seconds,
        codex_status_refresh_max_interval_seconds,
    )?;
    let codex_status_account_jitter_max_seconds = request
        .codex_status_account_jitter_max_seconds
        .unwrap_or(current.codex_status_account_jitter_max_seconds);
    validate_max(
        "codex_status_account_jitter_max_seconds",
        codex_status_account_jitter_max_seconds,
        MAX_RUNTIME_STATUS_ACCOUNT_JITTER_SECONDS,
    )?;

    let kiro_status_refresh_min_interval_seconds = request
        .kiro_status_refresh_min_interval_seconds
        .unwrap_or(current.kiro_status_refresh_min_interval_seconds);
    let kiro_status_refresh_max_interval_seconds = request
        .kiro_status_refresh_max_interval_seconds
        .unwrap_or(current.kiro_status_refresh_max_interval_seconds);
    validate_runtime_refresh_window(
        kiro_status_refresh_min_interval_seconds,
        kiro_status_refresh_max_interval_seconds,
    )?;
    let kiro_status_account_jitter_max_seconds = request
        .kiro_status_account_jitter_max_seconds
        .unwrap_or(current.kiro_status_account_jitter_max_seconds);
    validate_max(
        "kiro_status_account_jitter_max_seconds",
        kiro_status_account_jitter_max_seconds,
        MAX_RUNTIME_STATUS_ACCOUNT_JITTER_SECONDS,
    )?;

    let usage_event_flush_batch_size = request
        .usage_event_flush_batch_size
        .unwrap_or(current.usage_event_flush_batch_size);
    validate_range(
        "usage_event_flush_batch_size",
        usage_event_flush_batch_size,
        MIN_RUNTIME_USAGE_EVENT_FLUSH_BATCH_SIZE,
        MAX_RUNTIME_USAGE_EVENT_FLUSH_BATCH_SIZE,
    )?;
    let usage_event_flush_interval_seconds = request
        .usage_event_flush_interval_seconds
        .unwrap_or(current.usage_event_flush_interval_seconds);
    validate_range(
        "usage_event_flush_interval_seconds",
        usage_event_flush_interval_seconds,
        MIN_RUNTIME_USAGE_EVENT_FLUSH_INTERVAL_SECONDS,
        MAX_RUNTIME_USAGE_EVENT_FLUSH_INTERVAL_SECONDS,
    )?;
    let usage_event_flush_max_buffer_bytes = request
        .usage_event_flush_max_buffer_bytes
        .unwrap_or(current.usage_event_flush_max_buffer_bytes);
    validate_range(
        "usage_event_flush_max_buffer_bytes",
        usage_event_flush_max_buffer_bytes,
        MIN_RUNTIME_USAGE_EVENT_FLUSH_MAX_BUFFER_BYTES,
        MAX_RUNTIME_USAGE_EVENT_FLUSH_MAX_BUFFER_BYTES,
    )?;

    let kiro_cache_kmodels_json = request
        .kiro_cache_kmodels_json
        .unwrap_or(current.kiro_cache_kmodels_json);
    parse_kiro_cache_kmodels_json(&kiro_cache_kmodels_json)
        .map_err(|_| bad_request("kiro_cache_kmodels_json is invalid"))?;

    let kiro_billable_model_multipliers_json = match request.kiro_billable_model_multipliers_json {
        Some(value) => {
            let multipliers = parse_kiro_billable_model_multipliers_json(&value)
                .map_err(|_| bad_request("kiro_billable_model_multipliers_json is invalid"))?;
            serde_json::to_string(&multipliers).map_err(|_| {
                internal_error("Failed to normalize kiro billable multiplier config")
            })?
        },
        None => current.kiro_billable_model_multipliers_json,
    };

    let kiro_cache_policy_json = request
        .kiro_cache_policy_json
        .unwrap_or(current.kiro_cache_policy_json);
    parse_kiro_cache_policy_json(&kiro_cache_policy_json)
        .map_err(|_| bad_request("kiro_cache_policy_json is invalid"))?;

    let kiro_prefix_cache_mode = request
        .kiro_prefix_cache_mode
        .unwrap_or(current.kiro_prefix_cache_mode);
    validate_kiro_prefix_cache_mode(&kiro_prefix_cache_mode)?;

    let kiro_prefix_cache_max_tokens = request
        .kiro_prefix_cache_max_tokens
        .unwrap_or(current.kiro_prefix_cache_max_tokens);
    validate_positive("kiro_prefix_cache_max_tokens", kiro_prefix_cache_max_tokens)?;
    let kiro_prefix_cache_entry_ttl_seconds = request
        .kiro_prefix_cache_entry_ttl_seconds
        .unwrap_or(current.kiro_prefix_cache_entry_ttl_seconds);
    validate_positive("kiro_prefix_cache_entry_ttl_seconds", kiro_prefix_cache_entry_ttl_seconds)?;
    let kiro_conversation_anchor_max_entries = request
        .kiro_conversation_anchor_max_entries
        .unwrap_or(current.kiro_conversation_anchor_max_entries);
    validate_positive(
        "kiro_conversation_anchor_max_entries",
        kiro_conversation_anchor_max_entries,
    )?;
    let kiro_conversation_anchor_ttl_seconds = request
        .kiro_conversation_anchor_ttl_seconds
        .unwrap_or(current.kiro_conversation_anchor_ttl_seconds);
    validate_positive(
        "kiro_conversation_anchor_ttl_seconds",
        kiro_conversation_anchor_ttl_seconds,
    )?;

    Ok(AdminRuntimeConfig {
        auth_cache_ttl_seconds,
        max_request_body_bytes,
        account_failure_retry_limit,
        codex_client_version,
        codex_status_refresh_min_interval_seconds,
        codex_status_refresh_max_interval_seconds,
        codex_status_account_jitter_max_seconds,
        kiro_status_refresh_min_interval_seconds,
        kiro_status_refresh_max_interval_seconds,
        kiro_status_account_jitter_max_seconds,
        usage_event_flush_batch_size,
        usage_event_flush_interval_seconds,
        usage_event_flush_max_buffer_bytes,
        kiro_cache_kmodels_json,
        kiro_billable_model_multipliers_json,
        kiro_cache_policy_json,
        kiro_prefix_cache_mode,
        kiro_prefix_cache_max_tokens,
        kiro_prefix_cache_entry_ttl_seconds,
        kiro_conversation_anchor_max_entries,
        kiro_conversation_anchor_ttl_seconds,
    })
}

fn ensure_admin_access(headers: &HeaderMap) -> Result<(), AdminHttpError> {
    if let Some(expected_token) = admin_token() {
        let provided = headers
            .get("x-admin-token")
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .unwrap_or_default();
        if provided == expected_token {
            return Ok(());
        }
    }

    let ip = extract_client_ip(headers);
    if ip == "unknown" {
        if is_local_host_header(headers) {
            return Ok(());
        }
        return Err(forbidden("Admin endpoint is local-only"));
    }
    let ip = ip
        .parse::<IpAddr>()
        .map_err(|_| forbidden("Admin endpoint is local-only"))?;
    if is_private_or_loopback_ip(ip) {
        Ok(())
    } else {
        Err(forbidden("Admin endpoint is local-only"))
    }
}

fn admin_token() -> Option<String> {
    std::env::var("LLM_ACCESS_ADMIN_TOKEN")
        .ok()
        .or_else(|| std::env::var("ADMIN_TOKEN").ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

fn generate_id(prefix: &str) -> String {
    format!("{prefix}-{}", uuid::Uuid::new_v4().simple())
}

fn generate_secret() -> String {
    format!("sfk_{}", uuid::Uuid::new_v4().simple())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn normalize_codex_client_version(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_CODEX_CLIENT_VERSION_LEN {
        return None;
    }
    if !trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_'))
    {
        return None;
    }
    Some(trimmed.to_string())
}

fn normalize_review_queue_query(
    request: ListReviewQueueRequest,
) -> core_store::AdminReviewQueueQuery {
    core_store::AdminReviewQueueQuery {
        status: request
            .status
            .and_then(|status| normalize_optional_string(&status)),
        limit: request
            .limit
            .unwrap_or(DEFAULT_ADMIN_REVIEW_QUEUE_LIMIT)
            .clamp(1, MAX_ADMIN_REVIEW_QUEUE_LIMIT),
        offset: request.offset.unwrap_or(0),
    }
}

fn normalize_usage_query(request: ListUsageEventsRequest) -> UsageEventQuery {
    UsageEventQuery {
        key_id: request
            .key_id
            .and_then(|key_id| normalize_optional_string(&key_id)),
        limit: request
            .limit
            .unwrap_or(DEFAULT_ADMIN_USAGE_LIMIT)
            .clamp(1, MAX_ADMIN_USAGE_LIMIT),
        offset: request.offset.unwrap_or(0),
    }
}

fn review_queue_action(request: ReviewQueueActionRequest) -> AdminReviewQueueAction {
    AdminReviewQueueAction {
        admin_note: normalize_optional_string_option(request.admin_note.as_deref()),
        updated_at_ms: now_ms(),
    }
}

impl From<&UsageEvent> for AdminUsageEventView {
    fn from(value: &UsageEvent) -> Self {
        let latency_ms = usage_latency_ms(value);
        Self {
            id: value.event_id.clone(),
            key_id: value.key_id.clone(),
            key_name: value.key_name.clone(),
            account_name: value.account_name.clone(),
            request_method: usage_request_method(value),
            request_url: value.endpoint.clone(),
            latency_ms,
            routing_wait_ms: None,
            upstream_headers_ms: optional_i64_to_i32(value.timing.upstream_headers_ms),
            post_headers_body_ms: optional_i64_to_i32(value.timing.post_headers_body_ms),
            request_body_bytes: value.request_body_bytes.and_then(non_negative_i64_to_u64),
            request_body_read_ms: None,
            request_json_parse_ms: None,
            pre_handler_ms: None,
            first_sse_write_ms: optional_i64_to_i32(value.timing.first_sse_write_ms),
            stream_finish_ms: optional_i64_to_i32(value.timing.stream_finish_ms),
            other_latency_ms: compute_other_latency_ms(
                latency_ms,
                None,
                optional_i64_to_i32(value.timing.upstream_headers_ms),
                optional_i64_to_i32(value.timing.post_headers_body_ms),
            ),
            quota_failover_count: 0,
            routing_diagnostics_json: None,
            endpoint: value.endpoint.clone(),
            model: value.model.clone(),
            status_code: value.status_code.clamp(0, i64::from(i32::MAX)) as i32,
            input_uncached_tokens: non_negative_i64_to_u64(value.input_uncached_tokens)
                .unwrap_or(0),
            input_cached_tokens: non_negative_i64_to_u64(value.input_cached_tokens).unwrap_or(0),
            output_tokens: non_negative_i64_to_u64(value.output_tokens).unwrap_or(0),
            billable_tokens: non_negative_i64_to_u64(value.billable_tokens).unwrap_or(0),
            usage_missing: value.usage_missing,
            credit_usage: value
                .credit_usage
                .as_deref()
                .and_then(|raw| raw.parse::<f64>().ok()),
            credit_usage_missing: value.credit_usage_missing,
            client_ip: "unknown".to_string(),
            ip_region: "unknown".to_string(),
            last_message_content: None,
            created_at: value.created_at_ms,
        }
    }
}

fn usage_request_method(value: &UsageEvent) -> String {
    if value.endpoint.ends_with("/models") || value.endpoint == "/v1/models" {
        "GET"
    } else {
        "POST"
    }
    .to_string()
}

fn usage_latency_ms(value: &UsageEvent) -> i32 {
    let latency = value.timing.stream_finish_ms.or_else(|| {
        match (value.timing.upstream_headers_ms, value.timing.post_headers_body_ms) {
            (Some(headers), Some(body)) => Some(headers.saturating_add(body)),
            _ => None,
        }
    });
    optional_i64_to_i32(latency).unwrap_or(0)
}

fn optional_i64_to_i32(value: Option<i64>) -> Option<i32> {
    value.map(|value| value.clamp(0, i64::from(i32::MAX)) as i32)
}

fn non_negative_i64_to_u64(value: i64) -> Option<u64> {
    u64::try_from(value.max(0)).ok()
}

fn compute_other_latency_ms(
    latency_ms: i32,
    routing_wait_ms: Option<i32>,
    upstream_headers_ms: Option<i32>,
    post_headers_body_ms: Option<i32>,
) -> Option<i32> {
    if routing_wait_ms.is_none() && upstream_headers_ms.is_none() && post_headers_body_ms.is_none()
    {
        return None;
    }
    let measured_ms: i64 = [routing_wait_ms, upstream_headers_ms, post_headers_body_ms]
        .into_iter()
        .flatten()
        .map(|value| i64::from(value.max(0)))
        .sum();
    Some((i64::from(latency_ms.max(0)) - measured_ms).clamp(0, i64::from(i32::MAX)) as i32)
}

async fn run_proxy_connectivity_check(
    proxy: &core_store::AdminProxyConfig,
    provider_type: &str,
) -> anyhow::Result<AdminProxyCheckResponse> {
    let target_url = match provider_type {
        PROVIDER_CODEX => "https://chatgpt.com/backend-api/codex/v1/models".to_string(),
        PROVIDER_KIRO => {
            "https://q.us-east-1.amazonaws.com/getUsageLimits?origin=AI_EDITOR&resourceType=AGENTIC_REQUEST"
                .to_string()
        },
        _ => unreachable!("provider type must be validated before proxy check"),
    };
    let client = build_proxy_client(proxy)?;
    let started_at = Instant::now();
    let result = client.get(&target_url).send().await;
    let target = match result {
        Ok(response) => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            AdminProxyCheckTargetView {
                target: provider_type.to_string(),
                url: target_url,
                reachable: true,
                status_code: Some(status.as_u16()),
                latency_ms: started_at.elapsed().as_millis().min(i64::MAX as u128) as i64,
                error_message: (!status.is_success()).then(|| summarize_upstream_error_body(&body)),
            }
        },
        Err(err) => AdminProxyCheckTargetView {
            target: provider_type.to_string(),
            url: target_url,
            reachable: false,
            status_code: None,
            latency_ms: started_at.elapsed().as_millis().min(i64::MAX as u128) as i64,
            error_message: Some(err.to_string()),
        },
    };
    Ok(AdminProxyCheckResponse {
        proxy_config_id: proxy.id.clone(),
        proxy_config_name: proxy.name.clone(),
        provider_type: provider_type.to_string(),
        auth_label: "anonymous connectivity probe".to_string(),
        ok: target.reachable,
        targets: vec![target],
        checked_at: now_ms(),
    })
}

fn build_proxy_client(proxy: &core_store::AdminProxyConfig) -> anyhow::Result<reqwest::Client> {
    let mut proxy_config = reqwest::Proxy::all(&proxy.proxy_url)?;
    if let Some(username) = proxy.proxy_username.as_deref() {
        proxy_config =
            proxy_config.basic_auth(username, proxy.proxy_password.as_deref().unwrap_or(""));
    }
    reqwest::Client::builder()
        .proxy(proxy_config)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(PROXY_CONNECTIVITY_CHECK_TIMEOUT_SECONDS))
        .build()
        .map_err(Into::into)
}

fn summarize_upstream_error_body(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        "empty body".to_string()
    } else {
        trimmed.chars().take(200).collect()
    }
}

fn validate_range(field: &str, value: u64, min: u64, max: u64) -> Result<(), AdminHttpError> {
    if (min..=max).contains(&value) {
        Ok(())
    } else {
        Err(bad_request(&format!("{field} is out of range")))
    }
}

fn validate_max(field: &str, value: u64, max: u64) -> Result<(), AdminHttpError> {
    if value <= max {
        Ok(())
    } else {
        Err(bad_request(&format!("{field} is out of range")))
    }
}

fn validate_positive(field: &str, value: u64) -> Result<(), AdminHttpError> {
    if value > 0 {
        Ok(())
    } else {
        Err(bad_request(&format!("{field} must be positive")))
    }
}

fn validate_runtime_refresh_window(
    min_seconds: u64,
    max_seconds: u64,
) -> Result<(), AdminHttpError> {
    if !(MIN_RUNTIME_STATUS_REFRESH_INTERVAL_SECONDS..=MAX_RUNTIME_STATUS_REFRESH_INTERVAL_SECONDS)
        .contains(&min_seconds)
        || !(MIN_RUNTIME_STATUS_REFRESH_INTERVAL_SECONDS
            ..=MAX_RUNTIME_STATUS_REFRESH_INTERVAL_SECONDS)
            .contains(&max_seconds)
    {
        return Err(bad_request("refresh window seconds must be between 240 and 3600"));
    }
    if min_seconds > max_seconds {
        return Err(bad_request("refresh min interval must be less than or equal to max interval"));
    }
    Ok(())
}

fn validate_kiro_prefix_cache_mode(mode: &str) -> Result<(), AdminHttpError> {
    if matches!(mode, KIRO_PREFIX_CACHE_MODE_FORMULA | core_store::DEFAULT_KIRO_PREFIX_CACHE_MODE) {
        Ok(())
    } else {
        Err(bad_request("kiro_prefix_cache_mode is invalid"))
    }
}

fn normalize_key_patch(
    request: PatchLlmGatewayKeyRequest,
) -> Result<AdminKeyPatch, AdminHttpError> {
    let name = match request.name.as_deref() {
        Some(raw) => Some(normalize_name(raw)?),
        None => None,
    };
    let status = match request.status.as_deref() {
        Some(raw) => Some(normalize_status(raw)?),
        None => None,
    };
    if let Some(limit) = request.quota_billable_limit {
        validate_i64_backed_u64("quota_billable_limit", limit)?;
    }
    let route_strategy = match request.route_strategy.as_deref() {
        Some(raw) => Some(normalize_route_strategy_input(raw)?),
        None => None,
    };
    let account_group_id = request
        .account_group_id
        .as_deref()
        .map(normalize_optional_string);
    let fixed_account_name = request
        .fixed_account_name
        .as_deref()
        .map(normalize_optional_string);
    let auto_account_names = request.auto_account_names.map(normalize_auto_account_names);
    let request_max_concurrency = if request.request_max_concurrency_unlimited {
        Some(None)
    } else {
        request.request_max_concurrency.map(Some)
    };
    let request_min_start_interval_ms = if request.request_min_start_interval_ms_unlimited {
        Some(None)
    } else {
        request.request_min_start_interval_ms.map(Some)
    };
    validate_codex_request_limit_inputs(
        request_max_concurrency.flatten(),
        request_min_start_interval_ms.flatten(),
    )?;
    Ok(AdminKeyPatch {
        name,
        status,
        public_visible: request.public_visible,
        quota_billable_limit: request.quota_billable_limit,
        route_strategy,
        account_group_id,
        fixed_account_name,
        auto_account_names,
        model_name_map: request.model_name_map.map(Some),
        request_max_concurrency,
        request_min_start_interval_ms,
        updated_at_ms: now_ms(),
    })
}

fn normalize_name(raw: &str) -> Result<String, AdminHttpError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        Err(bad_request("name is required"))
    } else {
        Ok(trimmed.to_string())
    }
}

fn normalize_status(raw: &str) -> Result<String, AdminHttpError> {
    let trimmed = raw.trim();
    if matches!(trimmed, KEY_STATUS_ACTIVE | KEY_STATUS_DISABLED) {
        Ok(trimmed.to_string())
    } else {
        Err(bad_request("status must be `active` or `disabled`"))
    }
}

fn normalize_route_strategy_input(raw: &str) -> Result<Option<String>, AdminHttpError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    match trimmed {
        "auto" | "fixed" => Ok(Some(trimmed.to_string())),
        _ => Err(bad_request("route_strategy must be `auto` or `fixed`")),
    }
}

fn validate_provider_type(provider_type: &str) -> Result<(), AdminHttpError> {
    match provider_type {
        PROVIDER_CODEX | PROVIDER_KIRO => Ok(()),
        _ => Err(bad_request("provider_type must be `codex` or `kiro`")),
    }
}

fn normalize_optional_string(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn normalize_optional_string_option(raw: Option<&str>) -> Option<String> {
    raw.and_then(normalize_optional_string)
}

fn normalize_account_name(raw: &str) -> Result<String, AdminHttpError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(bad_request("account name is required"));
    }
    if trimmed.len() > 64 {
        return Err(bad_request("account name must be 64 characters or fewer"));
    }
    if !trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return Err(bad_request(
            "account name must contain only ASCII letters, digits, hyphens, or underscores",
        ));
    }
    Ok(trimmed.to_string())
}

fn normalize_account_names(values: Vec<String>) -> Result<Option<Vec<String>>, AdminHttpError> {
    let mut names = values
        .into_iter()
        .map(|value| normalize_account_name(&value))
        .collect::<Result<Vec<_>, _>>()?;
    names.sort();
    names.dedup();
    if names.is_empty() {
        Ok(None)
    } else {
        Ok(Some(names))
    }
}

fn normalize_auto_account_names(values: Vec<String>) -> Option<Vec<String>> {
    let mut names = values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    if names.is_empty() {
        None
    } else {
        Some(names)
    }
}

fn normalize_required_proxy_url(raw: &str) -> Result<String, AdminHttpError> {
    let value = raw.trim();
    if value.is_empty() {
        return Err(bad_request("proxy_url is required"));
    }
    let parsed =
        url::Url::parse(value).map_err(|_| bad_request("proxy_url must be a valid URL"))?;
    if !matches!(parsed.scheme(), "http" | "https" | "socks5" | "socks5h") {
        return Err(bad_request("proxy_url scheme must be http, https, socks5, or socks5h"));
    }
    if parsed.host_str().is_none() {
        return Err(bad_request("proxy_url must include a host"));
    }
    Ok(value.to_string())
}

fn normalize_proxy_config_patch(
    request: PatchLlmGatewayProxyConfigRequest,
) -> Result<AdminProxyConfigPatch, AdminHttpError> {
    let name = request.name.as_deref().map(normalize_name).transpose()?;
    let proxy_url = request
        .proxy_url
        .as_deref()
        .map(normalize_required_proxy_url)
        .transpose()?;
    let status = request
        .status
        .as_deref()
        .map(normalize_status)
        .transpose()?;
    Ok(AdminProxyConfigPatch {
        name,
        proxy_url,
        proxy_username: request
            .proxy_username
            .as_deref()
            .map(|value| normalize_optional_string_option(Some(value))),
        proxy_password: request
            .proxy_password
            .as_deref()
            .map(|value| normalize_optional_string_option(Some(value))),
        status,
        updated_at_ms: now_ms(),
    })
}

fn normalize_account_patch(
    request: PatchLlmGatewayAccountRequest,
) -> Result<AdminCodexAccountPatch, AdminHttpError> {
    let proxy_mode = request
        .proxy_mode
        .as_deref()
        .map(normalize_proxy_mode)
        .transpose()?;
    let proxy_config_id = request
        .proxy_config_id
        .as_deref()
        .map(|value| normalize_optional_string_option(Some(value)));
    if matches!(proxy_mode.as_deref(), Some("fixed"))
        && proxy_config_id
            .as_ref()
            .and_then(|value| value.as_ref())
            .is_none()
    {
        return Err(bad_request("fixed proxy_mode requires proxy_config_id"));
    }
    let request_max_concurrency = if request.request_max_concurrency_unlimited {
        Some(None)
    } else {
        request.request_max_concurrency.map(Some)
    };
    let request_min_start_interval_ms = if request.request_min_start_interval_ms_unlimited {
        Some(None)
    } else {
        request.request_min_start_interval_ms.map(Some)
    };
    validate_codex_request_limit_inputs(
        request_max_concurrency.flatten(),
        request_min_start_interval_ms.flatten(),
    )?;
    Ok(AdminCodexAccountPatch {
        map_gpt53_codex_to_spark: request.map_gpt53_codex_to_spark,
        proxy_mode,
        proxy_config_id,
        request_max_concurrency,
        request_min_start_interval_ms,
        updated_at_ms: now_ms(),
    })
}

fn normalize_proxy_mode(raw: &str) -> Result<String, AdminHttpError> {
    let trimmed = raw.trim();
    match trimmed {
        "inherit" | "fixed" | "none" => Ok(trimmed.to_string()),
        _ => Err(bad_request("proxy_mode must be `inherit`, `fixed`, or `none`")),
    }
}

fn validate_codex_request_limit_inputs(
    request_max_concurrency: Option<u64>,
    request_min_start_interval_ms: Option<u64>,
) -> Result<(), AdminHttpError> {
    if let Some(value) = request_max_concurrency {
        if value == 0 || value > MAX_CODEX_KEY_REQUEST_MAX_CONCURRENCY {
            return Err(bad_request("request_max_concurrency is out of range"));
        }
    }
    if let Some(value) = request_min_start_interval_ms {
        if value > MAX_CODEX_KEY_REQUEST_MIN_START_INTERVAL_MS {
            return Err(bad_request("request_min_start_interval_ms is out of range"));
        }
    }
    Ok(())
}

fn validate_i64_backed_u64(field: &str, value: u64) -> Result<(), AdminHttpError> {
    if value <= i64::MAX as u64 {
        Ok(())
    } else {
        Err(bad_request(&format!("{field} is out of range")))
    }
}

fn parse_kiro_cache_kmodels_json(value: &str) -> anyhow::Result<BTreeMap<String, f64>> {
    let map: BTreeMap<String, f64> = serde_json::from_str(value)?;
    anyhow::ensure!(!map.is_empty(), "kmodel map must not be empty");
    for (model, coeff) in &map {
        anyhow::ensure!(!model.trim().is_empty(), "kmodel entry has empty model name");
        anyhow::ensure!(
            coeff.is_finite() && *coeff > 0.0,
            "kmodel entry `{model}` must be a positive finite number"
        );
    }
    Ok(map)
}

fn parse_kiro_billable_model_multipliers_json(
    value: &str,
) -> anyhow::Result<BTreeMap<String, f64>> {
    let overrides: BTreeMap<String, f64> = serde_json::from_str(value)?;
    let mut merged = BTreeMap::from([
        ("haiku".to_string(), 1.0),
        ("opus".to_string(), 1.0),
        ("sonnet".to_string(), 1.0),
    ]);
    for (family, multiplier) in overrides {
        anyhow::ensure!(
            matches!(family.as_str(), "opus" | "sonnet" | "haiku"),
            "billable multiplier family `{family}` must be one of `opus`, `sonnet`, `haiku`"
        );
        anyhow::ensure!(
            multiplier.is_finite() && multiplier > 0.0,
            "billable multiplier `{family}` must be a positive finite number"
        );
        merged.insert(family, multiplier);
    }
    Ok(merged)
}

fn parse_kiro_cache_policy_json(value: &str) -> anyhow::Result<KiroCachePolicy> {
    let policy: KiroCachePolicy = serde_json::from_str(value)?;
    validate_kiro_cache_policy(&policy)?;
    Ok(policy)
}

fn validate_kiro_cache_policy(policy: &KiroCachePolicy) -> anyhow::Result<()> {
    let boost = &policy.small_input_high_credit_boost;
    anyhow::ensure!(
        boost.target_input_tokens > 0,
        "small_input_high_credit_boost.target_input_tokens must be positive"
    );
    anyhow::ensure!(
        boost.credit_start.is_finite()
            && boost.credit_end.is_finite()
            && boost.credit_start < boost.credit_end,
        "small_input_high_credit_boost credit range is invalid"
    );
    anyhow::ensure!(
        policy.high_credit_diagnostic_threshold.is_finite()
            && policy.high_credit_diagnostic_threshold >= 0.0,
        "high_credit_diagnostic_threshold must be finite and >= 0"
    );
    anyhow::ensure!(
        policy.anthropic_cache_creation_input_ratio.is_finite()
            && (0.0..=1.0).contains(&policy.anthropic_cache_creation_input_ratio),
        "anthropic_cache_creation_input_ratio must be finite and between 0 and 1"
    );
    anyhow::ensure!(
        !policy.prefix_tree_credit_ratio_bands.is_empty(),
        "prefix_tree_credit_ratio_bands must contain at least one band"
    );

    let mut previous_credit_end = None;
    let mut previous_ratio_end = None;
    for (index, band) in policy.prefix_tree_credit_ratio_bands.iter().enumerate() {
        anyhow::ensure!(
            band.credit_start.is_finite() && band.credit_end.is_finite(),
            "prefix_tree_credit_ratio_bands[{index}] credit bounds must be finite"
        );
        anyhow::ensure!(
            band.credit_start < band.credit_end,
            "prefix_tree_credit_ratio_bands[{index}] credit_start must be < credit_end"
        );
        anyhow::ensure!(
            band.cache_ratio_start.is_finite() && band.cache_ratio_end.is_finite(),
            "prefix_tree_credit_ratio_bands[{index}] cache ratios must be finite"
        );
        anyhow::ensure!(
            (0.0..=1.0).contains(&band.cache_ratio_start)
                && (0.0..=1.0).contains(&band.cache_ratio_end),
            "prefix_tree_credit_ratio_bands[{index}] cache ratios must be between 0 and 1"
        );
        anyhow::ensure!(
            band.cache_ratio_start >= band.cache_ratio_end,
            "prefix_tree_credit_ratio_bands[{index}] cache ratio must not increase within the band"
        );
        if let Some(prev_end) = previous_credit_end {
            anyhow::ensure!(
                band.credit_start >= prev_end - BAND_CONTIGUITY_TOLERANCE,
                "prefix_tree_credit_ratio_bands[{index}] overlaps previous band"
            );
            anyhow::ensure!(
                band.credit_start <= prev_end + BAND_CONTIGUITY_TOLERANCE,
                "prefix_tree_credit_ratio_bands[{index}] has a gap after previous band"
            );
        }
        if let Some(prev_ratio) = previous_ratio_end {
            anyhow::ensure!(
                band.cache_ratio_start <= prev_ratio,
                "prefix_tree_credit_ratio_bands[{index}] cache ratio increases between bands"
            );
        }
        previous_credit_end = Some(band.credit_end);
        previous_ratio_end = Some(band.cache_ratio_end);
    }
    Ok(())
}

fn extract_client_ip(headers: &HeaderMap) -> String {
    parse_first_ip_from_header(headers.get("x-forwarded-for"))
        .or_else(|| parse_first_ip_from_header(headers.get("x-real-ip")))
        .or_else(|| parse_first_ip_from_header(headers.get("cf-connecting-ip")))
        .or_else(|| parse_first_ip_from_header(headers.get("x-client-ip")))
        .or_else(|| parse_ip_from_forwarded_header(headers.get("forwarded")))
        .unwrap_or_else(|| "unknown".to_string())
}

fn parse_first_ip_from_header(value: Option<&header::HeaderValue>) -> Option<String> {
    let raw = value?.to_str().ok()?;
    raw.split(',')
        .find_map(|part| normalize_ip_token(part.trim()))
}

fn parse_ip_from_forwarded_header(value: Option<&header::HeaderValue>) -> Option<String> {
    let raw = value?.to_str().ok()?;
    for segment in raw.split(',') {
        for pair in segment.split(';') {
            let (key, value) = pair.split_once('=')?;
            if key.trim().eq_ignore_ascii_case("for") {
                if let Some(ip) = normalize_ip_token(value.trim().trim_matches('"')) {
                    return Some(ip);
                }
            }
        }
    }
    None
}

fn normalize_ip_token(token: &str) -> Option<String> {
    let token = token.trim();
    if token.is_empty() || token.eq_ignore_ascii_case("unknown") {
        return None;
    }
    if let Ok(ip) = token.parse::<IpAddr>() {
        return Some(ip.to_string());
    }
    if let Some(host) = token
        .strip_prefix('[')
        .and_then(|value| value.split_once(']').map(|parts| parts.0))
    {
        if let Ok(ip) = host.parse::<IpAddr>() {
            return Some(ip.to_string());
        }
    }
    if let Some((host, _port)) = token.rsplit_once(':') {
        if let Ok(ip) = host.parse::<IpAddr>() {
            return Some(ip.to_string());
        }
    }
    None
}

fn is_private_or_loopback_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.octets()[0] == 169 && v4.octets()[1] == 254
        },
        IpAddr::V6(v6) => v6.is_loopback() || v6.is_unique_local() || v6.is_unicast_link_local(),
    }
}

fn is_local_host_header(headers: &HeaderMap) -> bool {
    let Some(raw_host) = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let host = raw_host.trim();
    if host.eq_ignore_ascii_case("localhost") || host.eq_ignore_ascii_case("[::1]") {
        return true;
    }
    if let Some(host_only) = host
        .strip_prefix('[')
        .and_then(|value| value.split_once(']').map(|parts| parts.0))
    {
        if let Ok(ip) = host_only.parse::<IpAddr>() {
            return is_private_or_loopback_ip(ip);
        }
    }
    let host_only = host
        .split_once(':')
        .map(|parts| parts.0)
        .unwrap_or(host)
        .trim();
    if host_only.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host_only
        .parse::<IpAddr>()
        .map(is_private_or_loopback_ip)
        .unwrap_or(false)
}

fn bad_request(message: &str) -> AdminHttpError {
    AdminHttpError {
        status: StatusCode::BAD_REQUEST,
        message: message.to_string(),
    }
}

fn forbidden(message: &str) -> AdminHttpError {
    AdminHttpError {
        status: StatusCode::FORBIDDEN,
        message: message.to_string(),
    }
}

fn conflict(message: &str) -> AdminHttpError {
    AdminHttpError {
        status: StatusCode::CONFLICT,
        message: message.to_string(),
    }
}

fn not_found(message: &str) -> AdminHttpError {
    AdminHttpError {
        status: StatusCode::NOT_FOUND,
        message: message.to_string(),
    }
}

fn internal_error(message: &str) -> AdminHttpError {
    AdminHttpError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        message: message.to_string(),
    }
}
