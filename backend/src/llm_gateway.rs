//! LLM gateway orchestration layer.
//!
//! The public gateway intentionally keeps request normalization, upstream
//! transport, response adaptation, and runtime cache management in separate
//! modules. This file owns the top-level handlers and the proxy control flow
//! so the routing layer only needs to depend on one coherent module.

mod activity;
mod instructions;
mod models;
mod request;
mod response;
mod runtime;
mod support;
mod types;

pub(crate) mod accounts;
pub(crate) mod token_refresh;
use std::{
    collections::{BTreeMap, HashSet},
    env,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context, Result};
use async_stream::stream;
use axum::{
    body::{Body, Bytes},
    extract::{OriginalUri, Request, State},
    http::{header, HeaderMap, StatusCode},
    middleware::Next,
    response::{Json, Response},
};
use eventsource_stream::Eventsource;
use futures_util::{StreamExt, TryStreamExt};
use reqwest::header::{HeaderMap as ReqwestHeaderMap, HeaderValue as ReqwestHeaderValue};
use serde_json::json;
use sha2::{Digest, Sha256};
use static_flow_shared::llm_gateway_store::{
    is_valid_kiro_prefix_cache_mode, now_ms, parse_kiro_cache_policy_json,
    LlmGatewayAccountGroupRecord, LlmGatewayKeyRecord, LlmGatewayProxyBindingRecord,
    LlmGatewayProxyConfigRecord, LlmGatewayRuntimeConfigRecord, LlmGatewayUsageEventRecord,
    NewLlmGatewayAccountContributionRequestInput, NewLlmGatewaySponsorRequestInput,
    NewLlmGatewayTokenRequestInput, DEFAULT_CODEX_CLIENT_VERSION, LLM_GATEWAY_KEY_STATUS_ACTIVE,
    LLM_GATEWAY_KEY_STATUS_DISABLED, LLM_GATEWAY_PROTOCOL_OPENAI, LLM_GATEWAY_PROVIDER_CODEX,
    LLM_GATEWAY_PROVIDER_KIRO, LLM_GATEWAY_SPONSOR_REQUEST_STATUS_APPROVED,
    LLM_GATEWAY_SPONSOR_REQUEST_STATUS_PAYMENT_EMAIL_SENT, LLM_GATEWAY_TOKEN_REQUEST_STATUS_FAILED,
    LLM_GATEWAY_TOKEN_REQUEST_STATUS_ISSUED, LLM_GATEWAY_TOKEN_REQUEST_STATUS_PENDING,
    LLM_GATEWAY_TOKEN_REQUEST_STATUS_REJECTED,
};
pub(crate) use types::compute_other_latency_ms;

pub use self::runtime::LlmGatewayRuntimeState;
pub(crate) use self::{
    accounts::{resolve_auths_dir, AccountPool},
    support::{load_support_asset, load_support_config, render_payment_email_markdown},
    token_refresh::spawn_account_refresh_task,
};
use self::{
    accounts::{AccountSettingsPatch, AccountStatus, AccountSummarySnapshot},
    models::{append_client_version_query, respond_local_models, respond_public_model_catalog},
    request::{
        apply_gpt53_codex_spark_mapping, ensure_supported_gateway_path, external_origin,
        extract_last_message_content, extract_presented_key, normalize_name, normalize_status,
        normalize_upstream_base_url,
        prepare_gateway_request_from_bytes as normalize_gateway_request_from_bytes,
        read_gateway_request_body,
    },
    response::{
        adapt_completed_response_json, apply_upstream_response_headers,
        convert_json_response_to_chat_completion, convert_response_event_to_chat_chunk,
        encode_json_sse_chunk, encode_sse_event_with_model_alias, extract_usage_from_bytes,
        rewrite_json_response_model_alias, SseUsageCollector,
    },
    runtime::{
        bearer_header, codex_upstream_client_profile, gateway_auth_cache_ttl, CachedKeyLease,
        CodexAccountRequestLease, CodexAuthSnapshot, CodexKeyRequestLease,
        CodexKeyRequestLimitRejection,
    },
    types::{
        AccountListResponse, AccountSummaryView, AdminAccountGroupView, AdminAccountGroupsResponse,
        AdminLegacyKiroProxyMigrationResponse, AdminLlmGatewayAccountContributionRequestQuery,
        AdminLlmGatewayAccountContributionRequestView,
        AdminLlmGatewayAccountContributionRequestsResponse, AdminLlmGatewayKeyView,
        AdminLlmGatewayKeysResponse, AdminLlmGatewaySponsorRequestQuery,
        AdminLlmGatewaySponsorRequestView, AdminLlmGatewaySponsorRequestsResponse,
        AdminLlmGatewayTokenRequestQuery, AdminLlmGatewayTokenRequestView,
        AdminLlmGatewayTokenRequestsResponse, AdminLlmGatewayUsageEventDetailView,
        AdminLlmGatewayUsageEventView, AdminLlmGatewayUsageEventsResponse,
        AdminLlmGatewayUsageQuery, AdminUpstreamProxyBindingView,
        AdminUpstreamProxyBindingsResponse, AdminUpstreamProxyCheckResponse,
        AdminUpstreamProxyCheckTargetView, AdminUpstreamProxyConfigView,
        AdminUpstreamProxyConfigsResponse, CreateAdminAccountGroupRequest,
        CreateAdminUpstreamProxyConfigRequest, CreateLlmGatewayKeyRequest, GatewayResponseAdapter,
        ImportAccountRequest, LlmGatewayAccessResponse, LlmGatewayCreditsView,
        LlmGatewayEventContext, LlmGatewayPublicAccountStatusView, LlmGatewayPublicKeyView,
        LlmGatewayRateLimitBucketView, LlmGatewayRateLimitStatusResponse,
        LlmGatewayRateLimitWindowView, LlmGatewayRuntimeConfigResponse,
        LlmGatewaySupportConfigView, PatchAccountSettingsRequest, PatchAdminAccountGroupRequest,
        PatchAdminUpstreamProxyConfigRequest, PatchLlmGatewayKeyRequest, PreparedGatewayRequest,
        PublicLlmGatewayAccountContributionView, PublicLlmGatewayAccountContributionsResponse,
        PublicLlmGatewaySponsorView, PublicLlmGatewaySponsorsResponse,
        PublicLlmGatewayUsageChartPointView, PublicLlmGatewayUsageEventView,
        PublicLlmGatewayUsageKeyView, PublicLlmGatewayUsageLookupRequest,
        PublicLlmGatewayUsageLookupResponse, SubmitLlmGatewayAccountContributionRequest,
        SubmitLlmGatewayAccountContributionRequestResponse, SubmitLlmGatewaySponsorRequest,
        SubmitLlmGatewaySponsorRequestResponse, SubmitLlmGatewayTokenRequest,
        SubmitLlmGatewayTokenRequestResponse, UpdateAdminUpstreamProxyBindingRequest,
        UpdateLlmGatewayRuntimeConfigRequest, UsageBreakdown,
    },
};
use crate::{
    email::{
        build_llm_access_url, build_llm_gateway_base_url, normalize_frontend_page_url_input,
        normalize_requester_email_input,
    },
    handlers::{ensure_admin_access, generate_task_id, AdminTaskActionRequest, ErrorResponse},
    public_submit_guard::{
        build_client_fingerprint, build_submit_rate_limit_key, enforce_public_submit_rate_limit,
        extract_client_ip,
    },
    state::{
        parse_kiro_billable_model_multipliers_json, parse_kiro_cache_kmodels_json, AppState,
        LlmGatewayRuntimeConfig,
    },
    upstream_proxy::{
        parse_account_proxy_selection_patch, validate_proxy_url, ResolvedUpstreamProxy,
    },
};

const DEFAULT_UPSTREAM_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
const DEFAULT_WIRE_ORIGINATOR: &str = "codex_cli_rs";
const MAX_CODEX_CLIENT_VERSION_LEN: usize = 64;
const FAST_BILLABLE_MULTIPLIER: u64 = 2;
const MAX_RUNTIME_CACHE_TTL_SECONDS: u64 = 86_400;
const MIN_RUNTIME_CACHE_TTL_SECONDS: u64 = 1;
/// Hard upper bound on the configurable request body size (256 MiB).
const MAX_RUNTIME_REQUEST_BODY_BYTES: u64 = 256 * 1024 * 1024;
/// Hard lower bound on the configurable request body size (1 KiB).
const MIN_RUNTIME_REQUEST_BODY_BYTES: u64 = 1024;
/// Hard upper bound on tolerated consecutive account refresh failures.
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
const MAX_OPENAI_TOOL_NAME_LEN: usize = 64;
const PUBLIC_RATE_LIMIT_REFRESH_SECONDS: u64 = 60;
const LAST_MESSAGE_CONTENT_EXTRACT_FAILED: &str = "[extract_failed]";
const PROXY_CONNECTIVITY_CHECK_TIMEOUT_SECONDS: u64 = 10;
const MAX_PUBLIC_TOKEN_WISH_REASON_CHARS: usize = 4000;
const MAX_PUBLIC_TOKEN_WISH_QUOTA: u64 = 100_000_000_000;
const MAX_PUBLIC_ACCOUNT_CONTRIBUTION_MESSAGE_CHARS: usize = 4000;
const MAX_PUBLIC_ACCOUNT_CONTRIBUTION_GITHUB_ID_CHARS: usize = 39;
const MAX_PUBLIC_ACCOUNT_CONTRIBUTIONS: usize = 24;
const MAX_PUBLIC_SPONSOR_MESSAGE_CHARS: usize = 4000;
const MAX_PUBLIC_SPONSOR_DISPLAY_NAME_CHARS: usize = 80;
const MAX_PUBLIC_SPONSORS: usize = 36;
const PUBLIC_USAGE_LOOKUP_DEFAULT_LIMIT: usize = 50;
const PUBLIC_USAGE_LOOKUP_MAX_LIMIT: usize = 200;
const PUBLIC_USAGE_LOOKUP_CHART_BUCKETS: usize = 24;
const PUBLIC_USAGE_LOOKUP_BUCKET_MS: i64 = 60 * 60 * 1000;
const CODEX_STREAM_FAILURE_STATUS_CODE: i32 = 599;
pub(super) const GPT53_CODEX_MODEL_ID: &str = "gpt-5.3-codex";
pub(super) const GPT53_CODEX_SPARK_MODEL_ID: &str = "gpt-5.3-codex-spark";

fn public_rate_limit_refresh_interval() -> tokio::time::Interval {
    let mut ticker = tokio::time::interval_at(
        tokio::time::Instant::now() + Duration::from_secs(PUBLIC_RATE_LIMIT_REFRESH_SECONDS),
        Duration::from_secs(PUBLIC_RATE_LIMIT_REFRESH_SECONDS),
    );
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    ticker
}

fn build_runtime_config_response(
    config: &LlmGatewayRuntimeConfig,
) -> LlmGatewayRuntimeConfigResponse {
    LlmGatewayRuntimeConfigResponse {
        auth_cache_ttl_seconds: config.auth_cache_ttl_seconds,
        max_request_body_bytes: config.max_request_body_bytes,
        account_failure_retry_limit: config.account_failure_retry_limit,
        codex_client_version: config.codex_client_version.clone(),
        codex_status_refresh_min_interval_seconds: config.codex_status_refresh_min_interval_seconds,
        codex_status_refresh_max_interval_seconds: config.codex_status_refresh_max_interval_seconds,
        codex_status_account_jitter_max_seconds: config.codex_status_account_jitter_max_seconds,
        kiro_status_refresh_min_interval_seconds: config.kiro_status_refresh_min_interval_seconds,
        kiro_status_refresh_max_interval_seconds: config.kiro_status_refresh_max_interval_seconds,
        kiro_status_account_jitter_max_seconds: config.kiro_status_account_jitter_max_seconds,
        usage_event_flush_batch_size: config.usage_event_flush_batch_size,
        usage_event_flush_interval_seconds: config.usage_event_flush_interval_seconds,
        usage_event_flush_max_buffer_bytes: config.usage_event_flush_max_buffer_bytes,
        kiro_cache_kmodels_json: config.kiro_cache_kmodels_json.clone(),
        kiro_billable_model_multipliers_json: config.kiro_billable_model_multipliers_json.clone(),
        kiro_cache_policy_json: config.kiro_cache_policy_json.clone(),
        kiro_prefix_cache_mode: config.kiro_prefix_cache_mode.clone(),
        kiro_prefix_cache_max_tokens: config.kiro_prefix_cache_max_tokens,
        kiro_prefix_cache_entry_ttl_seconds: config.kiro_prefix_cache_entry_ttl_seconds,
        kiro_conversation_anchor_max_entries: config.kiro_conversation_anchor_max_entries,
        kiro_conversation_anchor_ttl_seconds: config.kiro_conversation_anchor_ttl_seconds,
    }
}

fn validate_runtime_refresh_window(
    min_seconds: u64,
    max_seconds: u64,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
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

fn validate_kiro_prefix_cache_mode(mode: &str) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    if is_valid_kiro_prefix_cache_mode(mode) {
        Ok(())
    } else {
        Err(bad_request("kiro_prefix_cache_mode is invalid"))
    }
}

fn validate_positive_u64(
    field_name: &str,
    value: u64,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    if value == 0 {
        return Err(bad_request(&format!("{field_name} must be positive")));
    }
    Ok(())
}

pub(crate) fn normalize_codex_client_version(raw: &str) -> Option<String> {
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

pub(crate) fn resolve_codex_client_version(raw: Option<&str>) -> String {
    raw.and_then(normalize_codex_client_version)
        .unwrap_or_else(|| DEFAULT_CODEX_CLIENT_VERSION.to_string())
}

#[derive(Debug, Clone, serde::Deserialize)]
pub(super) struct UsageStatusPayload {
    #[serde(default)]
    plan_type: Option<String>,
    #[serde(default)]
    rate_limit: Option<UsageRateLimitDetails>,
    #[serde(default)]
    additional_rate_limits: Option<Vec<UsageAdditionalRateLimit>>,
    #[serde(default)]
    credits: Option<UsageCreditsDetails>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct UsageRateLimitDetails {
    #[serde(default)]
    primary_window: Option<UsageRateLimitWindow>,
    #[serde(default)]
    secondary_window: Option<UsageRateLimitWindow>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct UsageAdditionalRateLimit {
    #[serde(default)]
    metered_feature: Option<String>,
    #[serde(default)]
    limit_name: Option<String>,
    #[serde(default)]
    rate_limit: Option<UsageRateLimitDetails>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct UsageRateLimitWindow {
    used_percent: f64,
    #[serde(default)]
    limit_window_seconds: Option<i64>,
    #[serde(default)]
    reset_at: Option<i64>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct UsageCreditsDetails {
    #[serde(default)]
    has_credits: bool,
    #[serde(default)]
    unlimited: bool,
    #[serde(default)]
    balance: Option<UsageBalanceValue>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(untagged)]
enum UsageBalanceValue {
    String(String),
    Number(f64),
    Integer(i64),
}

// === Public access APIs ===

/// Serve the public read-only gateway access payload consumed by `/llm-access`.
pub async fn get_public_access(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<LlmGatewayAccessResponse>, (StatusCode, Json<ErrorResponse>)> {
    let config = state.llm_gateway_runtime_config.read().clone();
    let base_keys = state
        .llm_gateway_store
        .list_public_keys()
        .await
        .map_err(|err| internal_error("Failed to list public gateway keys", err))?;
    let keys = state.llm_gateway.overlay_key_usage_batch(&base_keys).await;
    let gateway_path = "/api/llm-gateway/v1".to_string();
    let model_catalog_path = "/api/llm-gateway/model-catalog.json".to_string();
    let base_url = external_origin(&headers)
        .map(|origin| format!("{origin}{gateway_path}"))
        .unwrap_or_else(|| gateway_path.clone());

    tracing::debug!(
        key_count = keys.len(),
        gateway_path,
        "Serving public LLM gateway access payload"
    );

    Ok(Json(LlmGatewayAccessResponse {
        base_url,
        gateway_path,
        model_catalog_path,
        auth_cache_ttl_seconds: config.auth_cache_ttl_seconds,
        keys: keys.iter().map(LlmGatewayPublicKeyView::from).collect(),
        generated_at: now_ms(),
    }))
}

/// Serve a raw `model_catalog.json` payload for Codex clients that want a
/// local catalog matching the gateway's currently available models.
pub async fn get_public_model_catalog(
    State(state): State<AppState>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<Response, (StatusCode, Json<ErrorResponse>)> {
    let (auth_snapshot, map_gpt53_codex_to_spark) = if let Some((_account_name, snapshot, mapped)) =
        state
            .llm_gateway
            .account_pool
            .select_best_account(None)
            .await
    {
        (snapshot, mapped)
    } else {
        (
            state
                .llm_gateway
                .auth_source
                .current()
                .await
                .map_err(|err| internal_error("Failed to load public model catalog auth", err))?,
            false,
        )
    };
    respond_public_model_catalog(
        &state,
        &auth_snapshot,
        &headers,
        uri.query().unwrap_or_default(),
        map_gpt53_codex_to_spark,
    )
    .await
}

/// Serve the cached Codex account rate-limit snapshot without hitting the
/// upstream backend on every request.
pub async fn get_public_rate_limit_status(
    State(state): State<AppState>,
) -> Result<Json<LlmGatewayRateLimitStatusResponse>, (StatusCode, Json<ErrorResponse>)> {
    let snapshot = state.llm_gateway.rate_limit_status.read().clone();
    tracing::debug!(
        status = %snapshot.status,
        bucket_count = snapshot.buckets.len(),
        "Serving cached public LLM gateway rate-limit status"
    );
    Ok(Json(snapshot))
}

// === Admin configuration APIs ===

/// Read the current runtime gateway configuration from the admin API.
pub async fn get_admin_runtime_config(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<LlmGatewayRuntimeConfigResponse>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let config = state.llm_gateway_runtime_config.read().clone();
    Ok(Json(build_runtime_config_response(&config)))
}

/// Persist admin-controlled runtime gateway configuration changes.
pub async fn update_admin_runtime_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<UpdateLlmGatewayRuntimeConfigRequest>,
) -> Result<Json<LlmGatewayRuntimeConfigResponse>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let current_record = state
        .llm_gateway_store
        .get_runtime_config_or_default()
        .await
        .map_err(|err| internal_error("Failed to load llm gateway config", err))?;
    let current = state.llm_gateway_runtime_config.read().clone();
    let ttl = request
        .auth_cache_ttl_seconds
        .unwrap_or(current.auth_cache_ttl_seconds);
    if !(MIN_RUNTIME_CACHE_TTL_SECONDS..=MAX_RUNTIME_CACHE_TTL_SECONDS).contains(&ttl) {
        return Err(bad_request("auth_cache_ttl_seconds is out of range"));
    }
    // Validate max_request_body_bytes within [1 KiB, 256 MiB].
    let max_request_body_bytes = request
        .max_request_body_bytes
        .unwrap_or(current.max_request_body_bytes);
    if !(MIN_RUNTIME_REQUEST_BODY_BYTES..=MAX_RUNTIME_REQUEST_BODY_BYTES)
        .contains(&max_request_body_bytes)
    {
        return Err(bad_request("max_request_body_bytes is out of range"));
    }
    let account_failure_retry_limit = request
        .account_failure_retry_limit
        .unwrap_or(current.account_failure_retry_limit);
    if !(MIN_RUNTIME_ACCOUNT_FAILURE_RETRY_LIMIT..=MAX_RUNTIME_ACCOUNT_FAILURE_RETRY_LIMIT)
        .contains(&account_failure_retry_limit)
    {
        return Err(bad_request("account_failure_retry_limit is out of range"));
    }
    let codex_client_version = match request.codex_client_version.as_deref() {
        Some(value) => normalize_codex_client_version(value)
            .ok_or_else(|| bad_request("codex_client_version is invalid"))?,
        None => resolve_codex_client_version(Some(&current.codex_client_version)),
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
    if codex_status_account_jitter_max_seconds > MAX_RUNTIME_STATUS_ACCOUNT_JITTER_SECONDS {
        return Err(bad_request("codex_status_account_jitter_max_seconds is out of range"));
    }
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
    if kiro_status_account_jitter_max_seconds > MAX_RUNTIME_STATUS_ACCOUNT_JITTER_SECONDS {
        return Err(bad_request("kiro_status_account_jitter_max_seconds is out of range"));
    }
    let usage_event_flush_batch_size = request
        .usage_event_flush_batch_size
        .unwrap_or(current.usage_event_flush_batch_size);
    if !(MIN_RUNTIME_USAGE_EVENT_FLUSH_BATCH_SIZE..=MAX_RUNTIME_USAGE_EVENT_FLUSH_BATCH_SIZE)
        .contains(&usage_event_flush_batch_size)
    {
        return Err(bad_request("usage_event_flush_batch_size is out of range"));
    }
    let usage_event_flush_interval_seconds = request
        .usage_event_flush_interval_seconds
        .unwrap_or(current.usage_event_flush_interval_seconds);
    if !(MIN_RUNTIME_USAGE_EVENT_FLUSH_INTERVAL_SECONDS
        ..=MAX_RUNTIME_USAGE_EVENT_FLUSH_INTERVAL_SECONDS)
        .contains(&usage_event_flush_interval_seconds)
    {
        return Err(bad_request("usage_event_flush_interval_seconds is out of range"));
    }
    let usage_event_flush_max_buffer_bytes = request
        .usage_event_flush_max_buffer_bytes
        .unwrap_or(current.usage_event_flush_max_buffer_bytes);
    if !(MIN_RUNTIME_USAGE_EVENT_FLUSH_MAX_BUFFER_BYTES
        ..=MAX_RUNTIME_USAGE_EVENT_FLUSH_MAX_BUFFER_BYTES)
        .contains(&usage_event_flush_max_buffer_bytes)
    {
        return Err(bad_request("usage_event_flush_max_buffer_bytes is out of range"));
    }
    let kiro_cache_kmodels_json = request
        .kiro_cache_kmodels_json
        .clone()
        .unwrap_or_else(|| current.kiro_cache_kmodels_json.clone());
    let kiro_cache_kmodels = parse_kiro_cache_kmodels_json(&kiro_cache_kmodels_json)
        .map_err(|_| bad_request("kiro_cache_kmodels_json is invalid"))?;
    let kiro_billable_model_multipliers_json = request
        .kiro_billable_model_multipliers_json
        .clone()
        .unwrap_or_else(|| current.kiro_billable_model_multipliers_json.clone());
    let kiro_billable_model_multipliers =
        parse_kiro_billable_model_multipliers_json(&kiro_billable_model_multipliers_json)
            .map_err(|_| bad_request("kiro_billable_model_multipliers_json is invalid"))?;
    let kiro_billable_model_multipliers_json =
        serde_json::to_string(&kiro_billable_model_multipliers).map_err(|err| {
            internal_error("Failed to normalize kiro billable multiplier config", err)
        })?;
    let kiro_cache_policy_json = request
        .kiro_cache_policy_json
        .clone()
        .unwrap_or_else(|| current.kiro_cache_policy_json.clone());
    let kiro_cache_policy = parse_kiro_cache_policy_json(&kiro_cache_policy_json)
        .map_err(|_| bad_request("kiro_cache_policy_json is invalid"))?;
    let kiro_prefix_cache_mode = request
        .kiro_prefix_cache_mode
        .clone()
        .unwrap_or_else(|| current.kiro_prefix_cache_mode.clone());
    validate_kiro_prefix_cache_mode(&kiro_prefix_cache_mode)?;
    let kiro_prefix_cache_max_tokens = request
        .kiro_prefix_cache_max_tokens
        .unwrap_or(current.kiro_prefix_cache_max_tokens);
    validate_positive_u64("kiro_prefix_cache_max_tokens", kiro_prefix_cache_max_tokens)?;
    let kiro_prefix_cache_entry_ttl_seconds = request
        .kiro_prefix_cache_entry_ttl_seconds
        .unwrap_or(current.kiro_prefix_cache_entry_ttl_seconds);
    validate_positive_u64(
        "kiro_prefix_cache_entry_ttl_seconds",
        kiro_prefix_cache_entry_ttl_seconds,
    )?;
    let kiro_conversation_anchor_max_entries = request
        .kiro_conversation_anchor_max_entries
        .unwrap_or(current.kiro_conversation_anchor_max_entries);
    validate_positive_u64(
        "kiro_conversation_anchor_max_entries",
        kiro_conversation_anchor_max_entries,
    )?;
    let kiro_conversation_anchor_ttl_seconds = request
        .kiro_conversation_anchor_ttl_seconds
        .unwrap_or(current.kiro_conversation_anchor_ttl_seconds);
    validate_positive_u64(
        "kiro_conversation_anchor_ttl_seconds",
        kiro_conversation_anchor_ttl_seconds,
    )?;
    let config = LlmGatewayRuntimeConfigRecord {
        id: "default".to_string(),
        auth_cache_ttl_seconds: ttl,
        max_request_body_bytes,
        account_failure_retry_limit,
        codex_client_version: codex_client_version.clone(),
        kiro_channel_max_concurrency: current.kiro_channel_max_concurrency,
        kiro_channel_min_start_interval_ms: current.kiro_channel_min_start_interval_ms,
        codex_status_refresh_min_interval_seconds,
        codex_status_refresh_max_interval_seconds,
        codex_status_account_jitter_max_seconds,
        kiro_status_refresh_min_interval_seconds,
        kiro_status_refresh_max_interval_seconds,
        kiro_status_account_jitter_max_seconds,
        usage_event_flush_batch_size,
        usage_event_flush_interval_seconds,
        usage_event_flush_max_buffer_bytes,
        usage_event_maintenance_enabled: current_record.usage_event_maintenance_enabled,
        usage_event_maintenance_interval_seconds: current_record
            .usage_event_maintenance_interval_seconds,
        usage_event_detail_retention_days: current_record.usage_event_detail_retention_days,
        kiro_cache_kmodels_json: kiro_cache_kmodels_json.clone(),
        kiro_billable_model_multipliers_json: kiro_billable_model_multipliers_json.clone(),
        kiro_cache_policy_json: kiro_cache_policy_json.clone(),
        kiro_prefix_cache_mode: kiro_prefix_cache_mode.clone(),
        kiro_prefix_cache_max_tokens,
        kiro_prefix_cache_entry_ttl_seconds,
        kiro_conversation_anchor_max_entries,
        kiro_conversation_anchor_ttl_seconds,
        updated_at: now_ms(),
    };
    state
        .llm_gateway_store
        .upsert_runtime_config(&config)
        .await
        .map_err(|err| internal_error("Failed to update llm gateway config", err))?;
    {
        let mut runtime = state.llm_gateway_runtime_config.write();
        *runtime = LlmGatewayRuntimeConfig {
            auth_cache_ttl_seconds: ttl,
            max_request_body_bytes,
            account_failure_retry_limit,
            codex_client_version: codex_client_version.clone(),
            kiro_channel_max_concurrency: current.kiro_channel_max_concurrency,
            kiro_channel_min_start_interval_ms: current.kiro_channel_min_start_interval_ms,
            codex_status_refresh_min_interval_seconds,
            codex_status_refresh_max_interval_seconds,
            codex_status_account_jitter_max_seconds,
            kiro_status_refresh_min_interval_seconds,
            kiro_status_refresh_max_interval_seconds,
            kiro_status_account_jitter_max_seconds,
            usage_event_flush_batch_size,
            usage_event_flush_interval_seconds,
            usage_event_flush_max_buffer_bytes,
            kiro_cache_kmodels_json: kiro_cache_kmodels_json.clone(),
            kiro_cache_kmodels,
            kiro_billable_model_multipliers_json: kiro_billable_model_multipliers_json.clone(),
            kiro_billable_model_multipliers,
            kiro_cache_policy_json: kiro_cache_policy_json.clone(),
            kiro_cache_policy,
            kiro_prefix_cache_mode: kiro_prefix_cache_mode.clone(),
            kiro_prefix_cache_max_tokens,
            kiro_prefix_cache_entry_ttl_seconds,
            kiro_conversation_anchor_max_entries,
            kiro_conversation_anchor_ttl_seconds,
        };
    }

    tracing::info!(
        auth_cache_ttl_seconds = ttl,
        max_request_body_bytes,
        account_failure_retry_limit,
        codex_client_version = %codex_client_version,
        codex_status_refresh_min_interval_seconds,
        codex_status_refresh_max_interval_seconds,
        codex_status_account_jitter_max_seconds,
        kiro_status_refresh_min_interval_seconds,
        kiro_status_refresh_max_interval_seconds,
        kiro_status_account_jitter_max_seconds,
        usage_event_flush_batch_size,
        usage_event_flush_interval_seconds,
        usage_event_flush_max_buffer_bytes,
        kiro_cache_kmodels_json = %kiro_cache_kmodels_json,
        kiro_billable_model_multipliers_json = %kiro_billable_model_multipliers_json,
        kiro_cache_policy_json = %kiro_cache_policy_json,
        kiro_prefix_cache_mode = %kiro_prefix_cache_mode,
        kiro_prefix_cache_max_tokens,
        kiro_prefix_cache_entry_ttl_seconds,
        kiro_conversation_anchor_max_entries,
        kiro_conversation_anchor_ttl_seconds,
        "Updated LLM gateway runtime config"
    );

    let updated = state.llm_gateway_runtime_config.read().clone();
    Ok(Json(build_runtime_config_response(&updated)))
}

/// List reusable upstream proxy configs managed from the admin UI.
pub async fn list_admin_proxy_configs(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<AdminUpstreamProxyConfigsResponse>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let proxy_configs = state
        .llm_gateway_store
        .list_proxy_configs()
        .await
        .map_err(|err| internal_error("Failed to list upstream proxy configs", err))?;
    Ok(Json(AdminUpstreamProxyConfigsResponse {
        proxy_configs: proxy_configs
            .iter()
            .map(AdminUpstreamProxyConfigView::from)
            .collect(),
        generated_at: now_ms(),
    }))
}

/// Create one reusable upstream proxy config.
pub async fn create_admin_proxy_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateAdminUpstreamProxyConfigRequest>,
) -> Result<Json<AdminUpstreamProxyConfigView>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let record = build_proxy_config_record(None, request)?;
    state
        .llm_gateway_store
        .create_proxy_config(&record)
        .await
        .map_err(|err| internal_error("Failed to create upstream proxy config", err))?;
    state
        .upstream_proxy_registry
        .refresh()
        .await
        .map_err(|err| internal_error("Failed to refresh upstream proxy registry", err))?;
    Ok(Json(AdminUpstreamProxyConfigView::from(&record)))
}

/// Patch one reusable upstream proxy config.
pub async fn patch_admin_proxy_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(proxy_id): axum::extract::Path<String>,
    Json(request): Json<PatchAdminUpstreamProxyConfigRequest>,
) -> Result<Json<AdminUpstreamProxyConfigView>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let mut record = state
        .llm_gateway_store
        .get_proxy_config_by_id(&proxy_id)
        .await
        .map_err(|err| internal_error("Failed to load upstream proxy config", err))?
        .ok_or_else(|| not_found("Upstream proxy config not found"))?;
    if let Some(name) = request.name.as_deref() {
        record.name = normalize_name(name)?;
    }
    if let Some(proxy_url) = request.proxy_url.as_deref() {
        record.proxy_url = normalize_required_proxy_url(proxy_url)
            .map_err(|err| bad_request_with_detail("invalid proxy_url", err))?;
    }
    if request.proxy_username.is_some() {
        record.proxy_username = normalize_optional_secret(request.proxy_username.as_deref());
    }
    if request.proxy_password.is_some() {
        record.proxy_password = normalize_optional_secret(request.proxy_password.as_deref());
    }
    if let Some(status) = request.status.as_deref() {
        record.status = normalize_status(status)?;
    }
    record.updated_at = now_ms();
    state
        .llm_gateway_store
        .upsert_proxy_config(&record)
        .await
        .map_err(|err| internal_error("Failed to update upstream proxy config", err))?;
    state
        .upstream_proxy_registry
        .refresh()
        .await
        .map_err(|err| internal_error("Failed to refresh upstream proxy registry", err))?;
    Ok(Json(AdminUpstreamProxyConfigView::from(&record)))
}

/// Delete one reusable upstream proxy config. Bound configs must be unbound
/// first to avoid ambiguous runtime behavior.
pub async fn delete_admin_proxy_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(proxy_id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let config = state
        .llm_gateway_store
        .get_proxy_config_by_id(&proxy_id)
        .await
        .map_err(|err| internal_error("Failed to load upstream proxy config", err))?
        .ok_or_else(|| not_found("Upstream proxy config not found"))?;
    let bindings = state
        .llm_gateway_store
        .list_proxy_bindings()
        .await
        .map_err(|err| internal_error("Failed to list upstream proxy bindings", err))?;
    if let Some(binding) = bindings
        .iter()
        .find(|binding| binding.proxy_config_id == proxy_id)
    {
        return Err(conflict_error(&format!(
            "proxy config is still bound to provider `{}`",
            binding.provider_type
        )));
    }
    state
        .llm_gateway_store
        .delete_proxy_config(&proxy_id)
        .await
        .map_err(|err| internal_error("Failed to delete upstream proxy config", err))?;
    state
        .upstream_proxy_registry
        .refresh()
        .await
        .map_err(|err| internal_error("Failed to refresh upstream proxy registry", err))?;
    Ok(Json(json!({ "deleted": true, "id": config.id })))
}

/// Probe a reusable upstream proxy config against the real upstream hostnames
/// used by Codex and Kiro so the admin UI can surface immediate diagnostics.
pub async fn check_admin_proxy_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path((proxy_id, provider_type)): axum::extract::Path<(String, String)>,
) -> Result<Json<AdminUpstreamProxyCheckResponse>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    validate_provider_type(&provider_type)?;
    let config = state
        .llm_gateway_store
        .get_proxy_config_by_id(&proxy_id)
        .await
        .map_err(|err| internal_error("Failed to load upstream proxy config", err))?
        .ok_or_else(|| not_found("Upstream proxy config not found"))?;
    let check = run_proxy_connectivity_check(&state, &config, &provider_type)
        .await
        .map_err(|err| internal_error("Failed to check upstream proxy config", err))?;
    Ok(Json(check))
}

/// Show the effective provider-level proxy bindings for Codex and Kiro.
pub async fn list_admin_proxy_bindings(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<AdminUpstreamProxyBindingsResponse>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let views = load_proxy_binding_views(&state)
        .await
        .map_err(|err| internal_error("Failed to list upstream proxy bindings", err))?;
    Ok(Json(AdminUpstreamProxyBindingsResponse {
        bindings: views,
        generated_at: now_ms(),
    }))
}

/// Update or clear the proxy binding for one provider.
pub async fn update_admin_proxy_binding(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(provider_type): axum::extract::Path<String>,
    Json(request): Json<UpdateAdminUpstreamProxyBindingRequest>,
) -> Result<Json<AdminUpstreamProxyBindingView>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    validate_provider_type(&provider_type)?;
    if let Some(proxy_config_id) = request
        .proxy_config_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let config = state
            .llm_gateway_store
            .get_proxy_config_by_id(proxy_config_id)
            .await
            .map_err(|err| internal_error("Failed to load upstream proxy config", err))?
            .ok_or_else(|| not_found("Upstream proxy config not found"))?;
        if config.status != LLM_GATEWAY_KEY_STATUS_ACTIVE {
            return Err(bad_request("proxy config must be active before binding"));
        }
        let binding = LlmGatewayProxyBindingRecord {
            provider_type: provider_type.clone(),
            proxy_config_id: config.id.clone(),
            updated_at: now_ms(),
        };
        state
            .llm_gateway_store
            .upsert_proxy_binding(&binding)
            .await
            .map_err(|err| internal_error("Failed to update upstream proxy binding", err))?;
    } else {
        state
            .llm_gateway_store
            .delete_proxy_binding(&provider_type)
            .await
            .map_err(|err| internal_error("Failed to clear upstream proxy binding", err))?;
    }
    state
        .upstream_proxy_registry
        .refresh()
        .await
        .map_err(|err| internal_error("Failed to refresh upstream proxy registry", err))?;
    let view = load_proxy_binding_views(&state)
        .await
        .map_err(|err| internal_error("Failed to load updated upstream proxy binding", err))?
        .into_iter()
        .find(|view| view.provider_type == provider_type)
        .ok_or_else(|| {
            internal_error(
                "Updated upstream proxy binding disappeared",
                "binding missing after refresh",
            )
        })?;
    Ok(Json(view))
}

/// Import legacy Kiro account-level proxy settings into the shared proxy
/// registry and clear them from the per-account auth JSON files.
pub async fn import_legacy_kiro_proxy_configs(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<AdminLegacyKiroProxyMigrationResponse>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let result = state
        .upstream_proxy_registry
        .import_legacy_kiro_account_proxies()
        .await
        .map_err(|err| internal_error("Failed to import legacy Kiro proxy configs", err))?;
    Ok(Json(AdminLegacyKiroProxyMigrationResponse {
        created_configs: result
            .created_configs
            .iter()
            .map(AdminUpstreamProxyConfigView::from)
            .collect(),
        reused_configs: result
            .reused_configs
            .iter()
            .map(AdminUpstreamProxyConfigView::from)
            .collect(),
        migrated_account_names: result.migrated_account_names,
        generated_at: now_ms(),
    }))
}

/// List all managed keys for the admin inventory screen.
pub async fn list_admin_keys(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<AdminLlmGatewayKeysResponse>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let base_keys = state
        .llm_gateway_store
        .list_keys()
        .await
        .map_err(|err| internal_error("Failed to list llm gateway keys", err))?;
    let keys = state.llm_gateway.overlay_key_usage_batch(&base_keys).await;
    let config = state.llm_gateway_runtime_config.read().clone();

    tracing::debug!(key_count = keys.len(), "Listed admin LLM gateway keys");

    Ok(Json(AdminLlmGatewayKeysResponse {
        keys: keys.iter().map(AdminLlmGatewayKeyView::from).collect(),
        auth_cache_ttl_seconds: config.auth_cache_ttl_seconds,
        generated_at: now_ms(),
    }))
}

/// List reusable Codex account-pool groups for the admin UI.
pub async fn list_admin_account_groups(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<AdminAccountGroupsResponse>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let groups = state
        .llm_gateway_store
        .list_account_groups_for_provider(LLM_GATEWAY_PROVIDER_CODEX)
        .await
        .map_err(|err| internal_error("Failed to list Codex account groups", err))?;
    Ok(Json(AdminAccountGroupsResponse {
        groups: groups.iter().map(AdminAccountGroupView::from).collect(),
        generated_at: now_ms(),
    }))
}

/// Create a reusable Codex account-pool group.
pub async fn create_admin_account_group(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateAdminAccountGroupRequest>,
) -> Result<Json<AdminAccountGroupView>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let name = normalize_name(&request.name)?;
    let account_names =
        normalize_codex_account_group_members(&state, request.account_names).await?;
    let now = now_ms();
    let record = LlmGatewayAccountGroupRecord {
        id: generate_id("llm-group"),
        provider_type: LLM_GATEWAY_PROVIDER_CODEX.to_string(),
        name,
        account_names,
        created_at: now,
        updated_at: now,
    };
    state
        .llm_gateway_store
        .create_account_group(&record)
        .await
        .map_err(|err| internal_error("Failed to create Codex account group", err))?;
    Ok(Json(AdminAccountGroupView::from(&record)))
}

/// Update one reusable Codex account-pool group.
pub async fn patch_admin_account_group(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(group_id): axum::extract::Path<String>,
    Json(request): Json<PatchAdminAccountGroupRequest>,
) -> Result<Json<AdminAccountGroupView>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let mut group =
        load_account_group_for_provider(&state, LLM_GATEWAY_PROVIDER_CODEX, &group_id).await?;
    if let Some(name) = request.name.as_deref() {
        group.name = normalize_name(name)?;
    }
    if let Some(account_names) = request.account_names {
        group.account_names = normalize_codex_account_group_members(&state, account_names).await?;
    }
    group.updated_at = now_ms();
    state
        .llm_gateway_store
        .replace_account_group(&group)
        .await
        .map_err(|err| internal_error("Failed to update Codex account group", err))?;
    Ok(Json(AdminAccountGroupView::from(&group)))
}

/// Delete one reusable Codex account-pool group if no key still references it.
pub async fn delete_admin_account_group(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(group_id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let group =
        load_account_group_for_provider(&state, LLM_GATEWAY_PROVIDER_CODEX, &group_id).await?;
    let keys = state
        .llm_gateway_store
        .list_keys_for_provider(LLM_GATEWAY_PROVIDER_CODEX)
        .await
        .map_err(|err| internal_error("Failed to inspect Codex keys before group delete", err))?;
    if let Some(key) = keys
        .iter()
        .find(|key| key.account_group_id.as_deref() == Some(group_id.as_str()))
    {
        return Err(bad_request(&format!(
            "account group is still referenced by key `{}`",
            key.name
        )));
    }
    state
        .llm_gateway_store
        .delete_account_group(&group.id)
        .await
        .map_err(|err| internal_error("Failed to delete Codex account group", err))?;
    Ok(Json(json!({ "deleted": true, "id": group.id })))
}

/// Create a new admin-managed key and warm it into the in-memory auth cache.
pub async fn create_admin_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateLlmGatewayKeyRequest>,
) -> Result<Json<AdminLlmGatewayKeyView>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let name = normalize_name(&request.name)?;
    validate_codex_request_limit_inputs(
        request.request_max_concurrency,
        request.request_min_start_interval_ms,
    )?;
    let record = create_managed_key_record(&state, ManagedKeyCreateInput {
        name,
        quota_billable_limit: request.quota_billable_limit,
        public_visible: request.public_visible,
        route_strategy: None,
        account_group_id: None,
        fixed_account_name: None,
        auto_account_names: None,
        model_name_map: None,
        request_max_concurrency: request.request_max_concurrency,
        request_min_start_interval_ms: request.request_min_start_interval_ms,
    })
    .await?;

    tracing::info!(
        key_id = %record.id,
        key_name = %record.name,
        public_visible = record.public_visible,
        quota_billable_limit = record.quota_billable_limit,
        "Created LLM gateway key"
    );

    Ok(Json(AdminLlmGatewayKeyView::from(&record)))
}

struct ManagedKeyCreateInput {
    name: String,
    quota_billable_limit: u64,
    public_visible: bool,
    route_strategy: Option<String>,
    account_group_id: Option<String>,
    fixed_account_name: Option<String>,
    auto_account_names: Option<Vec<String>>,
    model_name_map: Option<std::collections::BTreeMap<String, String>>,
    request_max_concurrency: Option<u64>,
    request_min_start_interval_ms: Option<u64>,
}

async fn create_managed_key_record(
    state: &AppState,
    input: ManagedKeyCreateInput,
) -> Result<LlmGatewayKeyRecord, (StatusCode, Json<ErrorResponse>)> {
    let secret = generate_secret();
    let key_hash = sha256_hex(secret.as_bytes());
    let now = now_ms();
    let record = LlmGatewayKeyRecord {
        id: generate_id("llm-key"),
        name: input.name,
        secret,
        key_hash: key_hash.clone(),
        status: LLM_GATEWAY_KEY_STATUS_ACTIVE.to_string(),
        provider_type: LLM_GATEWAY_PROVIDER_CODEX.to_string(),
        protocol_family: LLM_GATEWAY_PROTOCOL_OPENAI.to_string(),
        public_visible: input.public_visible,
        quota_billable_limit: input.quota_billable_limit,
        usage_input_uncached_tokens: 0,
        usage_input_cached_tokens: 0,
        usage_output_tokens: 0,
        usage_billable_tokens: 0,
        usage_credit_total: 0.0,
        usage_credit_missing_events: 0,
        last_used_at: None,
        created_at: now,
        updated_at: now,
        route_strategy: input.route_strategy,
        account_group_id: input.account_group_id,
        fixed_account_name: input.fixed_account_name,
        auto_account_names: input.auto_account_names,
        model_name_map: input.model_name_map,
        request_max_concurrency: input.request_max_concurrency,
        request_min_start_interval_ms: input.request_min_start_interval_ms,
        kiro_request_validation_enabled: true,
        kiro_cache_estimation_enabled: true,
        kiro_zero_cache_debug_enabled: false,
        kiro_cache_policy_override_json: None,
        kiro_billable_model_multipliers_override_json: None,
    };
    state
        .llm_gateway_store
        .create_key(&record)
        .await
        .map_err(|err| internal_error("Failed to create llm gateway key", err))?;
    let ttl = current_cache_ttl(state).await;
    state
        .llm_gateway
        .key_cache
        .renew(record.clone(), Duration::from_secs(ttl));
    Ok(record)
}

/// Patch one managed key and refresh or invalidate its in-memory cache lease.
pub async fn patch_admin_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(key_id): axum::extract::Path<String>,
    Json(request): Json<PatchLlmGatewayKeyRequest>,
) -> Result<Json<AdminLlmGatewayKeyView>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let mut key = state
        .llm_gateway_store
        .get_key_by_id(&key_id)
        .await
        .map_err(|err| internal_error("Failed to load llm gateway key", err))?
        .ok_or_else(|| not_found("LLM gateway key not found"))?;

    if key.provider_type != LLM_GATEWAY_PROVIDER_CODEX {
        return Err(bad_request("Kiro keys must be managed from /admin/kiro-gateway"));
    }

    if let Some(name) = request.name.as_deref() {
        key.name = normalize_name(name)?;
    }
    if let Some(status) = request.status.as_deref() {
        key.status = normalize_status(status)?;
    }
    if let Some(public_visible) = request.public_visible {
        key.public_visible = public_visible;
    }
    if let Some(limit) = request.quota_billable_limit {
        key.quota_billable_limit = limit;
    }
    if let Some(strategy) = request.route_strategy.as_deref() {
        key.route_strategy = normalize_route_strategy_input(Some(strategy))
            .map_err(|err| bad_request_with_detail("invalid route_strategy", err))?;
    }
    if let Some(group_id) = request.account_group_id.as_deref() {
        key.account_group_id = normalize_optional_account_group_id_input(Some(group_id))
            .map_err(|err| bad_request_with_detail("invalid account_group_id", err))?;
    }
    if let Some(account_name) = request.fixed_account_name.as_deref() {
        key.fixed_account_name = normalize_optional_account_name_input(Some(account_name))
            .map_err(|err| bad_request_with_detail("invalid fixed_account_name", err))?;
    }
    if let Some(account_names) = request.auto_account_names {
        key.auto_account_names = normalize_auto_account_names_input(Some(account_names))
            .map_err(|err| bad_request_with_detail("invalid auto_account_names", err))?;
    }
    if let Some(model_name_map) = request.model_name_map {
        key.model_name_map = Some(model_name_map);
    }
    if request.request_max_concurrency_unlimited {
        key.request_max_concurrency = None;
    } else if let Some(value) = request.request_max_concurrency {
        key.request_max_concurrency = Some(value);
    }
    if request.request_min_start_interval_ms_unlimited {
        key.request_min_start_interval_ms = None;
    } else if let Some(value) = request.request_min_start_interval_ms {
        key.request_min_start_interval_ms = Some(value);
    }

    validate_codex_request_limit_inputs(
        key.request_max_concurrency,
        key.request_min_start_interval_ms,
    )?;

    materialize_legacy_codex_route_group_if_needed(&state, &mut key).await?;
    validate_codex_key_group_config(&state, &mut key).await?;
    key.updated_at = now_ms();
    state
        .llm_gateway_store
        .replace_key(&key)
        .await
        .map_err(|err| internal_error("Failed to update llm gateway key", err))?;

    if key.status == LLM_GATEWAY_KEY_STATUS_ACTIVE {
        let ttl = current_cache_ttl(&state).await;
        let effective_key = state.llm_gateway.overlay_key_usage(&key).await;
        state
            .llm_gateway
            .key_cache
            .renew(effective_key, Duration::from_secs(ttl));
    } else {
        state.llm_gateway.key_cache.invalidate(&key.key_hash);
    }

    tracing::info!(
        key_id = %key.id,
        key_name = %key.name,
        status = %key.status,
        public_visible = key.public_visible,
        quota_billable_limit = key.quota_billable_limit,
        "Updated LLM gateway key"
    );

    let effective_key = state.llm_gateway.overlay_key_usage(&key).await;
    Ok(Json(AdminLlmGatewayKeyView::from(&effective_key)))
}

/// Delete one managed key and evict it from the in-memory cache immediately.
pub async fn delete_admin_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(key_id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let key = state
        .llm_gateway_store
        .get_key_by_id(&key_id)
        .await
        .map_err(|err| internal_error("Failed to load llm gateway key", err))?
        .ok_or_else(|| not_found("LLM gateway key not found"))?;
    state
        .llm_gateway_store
        .delete_key(&key_id)
        .await
        .map_err(|err| internal_error("Failed to delete llm gateway key", err))?;
    state.llm_gateway.key_cache.invalidate(&key.key_hash);

    tracing::info!(key_id, key_name = %key.name, "Deleted LLM gateway key");

    Ok(Json(json!({ "deleted": true, "id": key_id })))
}

/// Return a paginated, reverse-chronological slice of usage diagnostics.
pub async fn list_admin_usage_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<AdminLlmGatewayUsageQuery>,
) -> Result<Json<AdminLlmGatewayUsageEventsResponse>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;

    let offset = query.offset.unwrap_or(0);
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    tracing::debug!(
        key_id = query.key_id.as_deref().unwrap_or("all"),
        offset,
        limit,
        "Listing admin LLM gateway usage events"
    );
    let total = state
        .llm_gateway_store
        .count_usage_events_for_provider(query.key_id.as_deref(), None)
        .await
        .map_err(|err| internal_error("Failed to count llm gateway usage events", err))?;
    let activity_snapshot = state
        .llm_gateway
        .request_activity_snapshot(query.key_id.as_deref());
    if total == 0 || offset >= total {
        tracing::debug!(
            key_id = query.key_id.as_deref().unwrap_or("all"),
            offset,
            limit,
            total,
            "LLM gateway usage event query resolved to an empty page"
        );
        return Ok(Json(AdminLlmGatewayUsageEventsResponse {
            total,
            offset,
            limit,
            has_more: false,
            current_rpm: activity_snapshot.rpm,
            current_in_flight: activity_snapshot.in_flight,
            events: vec![],
            generated_at: now_ms(),
        }));
    }

    let fetch_count = (total - offset).min(limit);
    let reverse_offset = total.saturating_sub(offset.saturating_add(fetch_count));
    let mut events = state
        .llm_gateway_store
        .query_usage_event_summaries(
            query.key_id.as_deref(),
            None,
            Some(fetch_count),
            Some(reverse_offset),
        )
        .await
        .map_err(|err| internal_error("Failed to query llm gateway usage events", err))?;
    events.sort_by(|left, right| right.created_at.cmp(&left.created_at));
    let has_more = offset.saturating_add(events.len()) < total;

    tracing::debug!(
        key_id = query.key_id.as_deref().unwrap_or("all"),
        total,
        offset,
        fetched = events.len(),
        has_more,
        "Admin LLM gateway usage event page ready"
    );

    Ok(Json(AdminLlmGatewayUsageEventsResponse {
        total,
        offset,
        limit,
        has_more,
        current_rpm: activity_snapshot.rpm,
        current_in_flight: activity_snapshot.in_flight,
        events: events
            .iter()
            .map(AdminLlmGatewayUsageEventView::from)
            .collect(),
        generated_at: now_ms(),
    }))
}

pub async fn get_admin_usage_event_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(event_id): axum::extract::Path<String>,
) -> Result<Json<AdminLlmGatewayUsageEventDetailView>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let event = state
        .llm_gateway_store
        .get_usage_event_detail_by_id(&event_id)
        .await
        .map_err(|err| internal_error("Failed to load llm gateway usage event detail", err))?
        .ok_or_else(|| not_found("LLM gateway usage event not found"))?;
    Ok(Json(AdminLlmGatewayUsageEventDetailView::from(&event)))
}

pub async fn lookup_public_usage(
    State(state): State<AppState>,
    Json(request): Json<PublicLlmGatewayUsageLookupRequest>,
) -> Result<Response, (StatusCode, Json<ErrorResponse>)> {
    let presented_key = request.api_key.trim();
    if presented_key.is_empty() {
        return Err(bad_request("api_key is required"));
    }

    let key_hash = sha256_hex(presented_key.as_bytes());
    let key = state
        .llm_gateway_store
        .get_key_by_hash(&key_hash)
        .await
        .map_err(|err| internal_error("Failed to look up public gateway key", err))?
        .ok_or_else(public_usage_lookup_not_found)?;
    let effective_key = state.llm_gateway.overlay_key_usage(&key).await;
    validate_public_usage_lookup_key(&effective_key)?;

    let offset = request.offset.unwrap_or(0);
    let limit = request
        .limit
        .unwrap_or(PUBLIC_USAGE_LOOKUP_DEFAULT_LIMIT)
        .clamp(1, PUBLIC_USAGE_LOOKUP_MAX_LIMIT);
    let total = state
        .llm_gateway
        .usage_event_count_for_key(&effective_key.id);

    let now_ms = now_ms();
    let chart_start_ms = public_usage_chart_window_start(now_ms);
    let chart_events = state
        .llm_gateway_store
        .query_usage_events_since(Some(&effective_key.id), None, Some(chart_start_ms), None, None)
        .await
        .map_err(|err| internal_error("Failed to query public gateway usage chart events", err))?;

    let (events, has_more) = if total == 0 || offset >= total {
        (Vec::new(), false)
    } else {
        let fetch_count = (total - offset).min(limit);
        let reverse_offset = total.saturating_sub(offset.saturating_add(fetch_count));
        let mut events = state
            .llm_gateway_store
            .query_usage_events(
                Some(&effective_key.id),
                None,
                Some(fetch_count),
                Some(reverse_offset),
            )
            .await
            .map_err(|err| internal_error("Failed to query public gateway usage events", err))?;
        events.sort_by(|left, right| right.created_at.cmp(&left.created_at));
        let has_more = offset.saturating_add(events.len()) < total;
        (events, has_more)
    };

    let payload = PublicLlmGatewayUsageLookupResponse {
        key: PublicLlmGatewayUsageKeyView::from(&effective_key),
        chart_points: build_public_usage_chart_points(&chart_events, now_ms),
        total,
        offset,
        limit,
        has_more,
        events: events
            .iter()
            .map(PublicLlmGatewayUsageEventView::from)
            .collect(),
        generated_at: now_ms,
    };

    json_no_store_response(
        &payload,
        "Failed to encode public gateway usage lookup response",
        "Failed to build public gateway usage lookup response",
    )
}

/// Accept a public token wish from `/llm-access`; actual key creation only
/// happens after an admin approves it.
pub async fn submit_public_token_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SubmitLlmGatewayTokenRequest>,
) -> Result<Json<SubmitLlmGatewayTokenRequestResponse>, (StatusCode, Json<ErrorResponse>)> {
    if request.requested_quota_billable_limit == 0 {
        return Err(bad_request("requested_quota_billable_limit must be > 0"));
    }
    if request.requested_quota_billable_limit > MAX_PUBLIC_TOKEN_WISH_QUOTA {
        return Err(bad_request("requested_quota_billable_limit is too large"));
    }
    let request_reason = request.request_reason.trim();
    if request_reason.is_empty() {
        return Err(bad_request("request_reason is required"));
    }
    if request_reason.chars().count() > MAX_PUBLIC_TOKEN_WISH_REASON_CHARS {
        return Err(bad_request("request_reason is too long"));
    }
    let requester_email = normalize_requester_email_input(Some(request.requester_email))
        .map_err(|err| bad_request_with_detail("invalid requester_email", err))?
        .ok_or_else(|| bad_request("requester_email is required"))?;
    let frontend_page_url = normalize_frontend_page_url_input(request.frontend_page_url)
        .map_err(|err| bad_request_with_detail("invalid frontend_page_url", err))?;

    let client_ip = extract_client_ip(&headers);
    let fingerprint = build_client_fingerprint(&headers);
    let rate_limit_key = build_submit_rate_limit_key(&headers, &fingerprint);
    enforce_public_submit_rate_limit(
        state.llm_gateway_public_submit_guard.as_ref(),
        &rate_limit_key,
        now_ms(),
        60,
        "llm-access public submission",
    )?;

    let request_id = generate_task_id("llmwish");
    let ip_region = state.geoip.resolve_region(&client_ip).await;
    let record = state
        .llm_gateway_store
        .create_token_request(NewLlmGatewayTokenRequestInput {
            request_id: request_id.clone(),
            requester_email,
            requested_quota_billable_limit: request.requested_quota_billable_limit,
            request_reason: request_reason.to_string(),
            frontend_page_url,
            fingerprint,
            client_ip,
            ip_region,
        })
        .await
        .map_err(|err| internal_error("Failed to create llm gateway token request", err))?;

    if let Some(notifier) = state.email_notifier.clone() {
        let record_for_email = record.clone();
        tokio::spawn(async move {
            if let Err(err) = notifier
                .send_admin_new_llm_token_request_notification(&record_for_email)
                .await
            {
                tracing::warn!(
                    "failed to send admin notification email for llm token request {}: {}",
                    record_for_email.request_id,
                    err
                );
            }
        });
    }

    Ok(Json(SubmitLlmGatewayTokenRequestResponse {
        request_id,
        status: LLM_GATEWAY_TOKEN_REQUEST_STATUS_PENDING.to_string(),
    }))
}

fn normalize_optional_github_id_input(value: Option<String>) -> Result<Option<String>> {
    let Some(trimmed) = value
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
    else {
        return Ok(None);
    };

    if trimmed.chars().count() > MAX_PUBLIC_ACCOUNT_CONTRIBUTION_GITHUB_ID_CHARS {
        anyhow::bail!("github_id is too long");
    }
    if trimmed.starts_with('-') || trimmed.ends_with('-') {
        anyhow::bail!("github_id cannot start or end with `-`");
    }
    if !trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
    {
        anyhow::bail!("github_id may contain only ASCII letters, digits, or `-`");
    }

    Ok(Some(trimmed))
}

fn normalize_optional_display_name_input(value: Option<String>) -> Result<Option<String>> {
    let Some(trimmed) = value
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
    else {
        return Ok(None);
    };

    if trimmed.chars().count() > MAX_PUBLIC_SPONSOR_DISPLAY_NAME_CHARS {
        anyhow::bail!("display_name is too long");
    }

    Ok(Some(trimmed))
}

fn normalize_route_strategy_input(value: Option<&str>) -> Result<Option<String>> {
    let Some(trimmed) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };

    match trimmed {
        "auto" | "fixed" => Ok(Some(trimmed.to_string())),
        _ => anyhow::bail!("route_strategy must be `auto` or `fixed`"),
    }
}

fn normalize_optional_account_name_input(value: Option<&str>) -> Result<Option<String>> {
    let Some(trimmed) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };

    accounts::validate_account_name(trimmed)
        .map(Some)
        .map_err(anyhow::Error::msg)
}

pub(crate) fn normalize_optional_account_group_id_input(
    value: Option<&str>,
) -> Result<Option<String>> {
    let Some(trimmed) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    Ok(Some(trimmed.to_string()))
}

fn normalize_auto_account_names_input(value: Option<Vec<String>>) -> Result<Option<Vec<String>>> {
    let Some(values) = value else {
        return Ok(None);
    };

    let mut names = values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(|value| accounts::validate_account_name(&value).map_err(anyhow::Error::msg))
        .collect::<Result<Vec<_>>>()?;
    names.sort();
    names.dedup();

    if names.is_empty() {
        return Ok(None);
    }
    Ok(Some(names))
}

async fn normalize_codex_account_group_members(
    state: &AppState,
    account_names: Vec<String>,
) -> Result<Vec<String>, (StatusCode, Json<ErrorResponse>)> {
    let names = normalize_auto_account_names_input(Some(account_names))
        .map_err(|err| bad_request_with_detail("invalid account_names", err))?
        .ok_or_else(|| bad_request("account_names must not be empty"))?;
    validate_account_names_exist(&state.llm_gateway.account_pool, &names).await?;
    Ok(names)
}

async fn load_account_group_for_provider(
    state: &AppState,
    provider_type: &str,
    group_id: &str,
) -> Result<LlmGatewayAccountGroupRecord, (StatusCode, Json<ErrorResponse>)> {
    let group = state
        .llm_gateway_store
        .get_account_group_by_id(group_id)
        .await
        .map_err(|err| internal_error("Failed to load account group", err))?
        .ok_or_else(|| bad_request("account_group_id does not exist"))?;
    if group.provider_type != provider_type {
        return Err(bad_request("account_group_id belongs to a different provider"));
    }
    Ok(group)
}

async fn validate_codex_key_group_config(
    state: &AppState,
    key: &mut LlmGatewayKeyRecord,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    match key.route_strategy.as_deref().unwrap_or("auto") {
        "fixed" => {
            let group_id = key
                .account_group_id
                .as_deref()
                .ok_or_else(|| bad_request("fixed route_strategy requires account_group_id"))?;
            let group =
                load_account_group_for_provider(state, LLM_GATEWAY_PROVIDER_CODEX, group_id)
                    .await?;
            if group.account_names.len() != 1 {
                return Err(bad_request(
                    "fixed route_strategy requires an account group with exactly one account",
                ));
            }
            key.fixed_account_name = None;
            key.auto_account_names = None;
            Ok(())
        },
        "auto" => {
            if let Some(group_id) = key.account_group_id.as_deref() {
                let _ =
                    load_account_group_for_provider(state, LLM_GATEWAY_PROVIDER_CODEX, group_id)
                        .await?;
            }
            key.fixed_account_name = None;
            key.auto_account_names = None;
            Ok(())
        },
        _ => Err(bad_request("route_strategy must be `auto` or `fixed`")),
    }
}

async fn create_single_account_group_for_key(
    state: &AppState,
    provider_type: &str,
    key_name: &str,
    key_id: &str,
    account_name: &str,
) -> Result<LlmGatewayAccountGroupRecord, (StatusCode, Json<ErrorResponse>)> {
    let now = now_ms();
    let record = LlmGatewayAccountGroupRecord {
        id: generate_id("llm-group"),
        provider_type: provider_type.to_string(),
        name: format!("Migrated {} {}", key_name, &key_id[..key_id.len().min(8)]),
        account_names: vec![account_name.to_string()],
        created_at: now,
        updated_at: now,
    };
    state
        .llm_gateway_store
        .create_account_group(&record)
        .await
        .map_err(|err| internal_error("Failed to create account group", err))?;
    Ok(record)
}

async fn create_account_group_for_key_subset(
    state: &AppState,
    provider_type: &str,
    key_name: &str,
    key_id: &str,
    account_names: Vec<String>,
) -> Result<LlmGatewayAccountGroupRecord, (StatusCode, Json<ErrorResponse>)> {
    let now = now_ms();
    let record = LlmGatewayAccountGroupRecord {
        id: generate_id("llm-group"),
        provider_type: provider_type.to_string(),
        name: format!("Migrated {} {}", key_name, &key_id[..key_id.len().min(8)]),
        account_names,
        created_at: now,
        updated_at: now,
    };
    state
        .llm_gateway_store
        .create_account_group(&record)
        .await
        .map_err(|err| internal_error("Failed to create account group", err))?;
    Ok(record)
}

async fn materialize_legacy_codex_route_group_if_needed(
    state: &AppState,
    key: &mut LlmGatewayKeyRecord,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    if key.account_group_id.is_some() {
        key.fixed_account_name = None;
        key.auto_account_names = None;
        return Ok(());
    }
    match key.route_strategy.as_deref().unwrap_or("auto") {
        "fixed" => {
            let Some(account_name) = key.fixed_account_name.clone() else {
                return Ok(());
            };
            validate_account_names_exist(
                &state.llm_gateway.account_pool,
                std::slice::from_ref(&account_name),
            )
            .await?;
            let group = create_single_account_group_for_key(
                state,
                LLM_GATEWAY_PROVIDER_CODEX,
                &key.name,
                &key.id,
                &account_name,
            )
            .await?;
            key.account_group_id = Some(group.id);
            key.fixed_account_name = None;
            key.auto_account_names = None;
            Ok(())
        },
        "auto" => {
            let Some(account_names) = key.auto_account_names.clone() else {
                return Ok(());
            };
            let account_names = normalize_codex_account_group_members(state, account_names).await?;
            let group = create_account_group_for_key_subset(
                state,
                LLM_GATEWAY_PROVIDER_CODEX,
                &key.name,
                &key.id,
                account_names,
            )
            .await?;
            key.account_group_id = Some(group.id);
            key.fixed_account_name = None;
            key.auto_account_names = None;
            Ok(())
        },
        _ => Ok(()),
    }
}

fn validate_codex_request_limit_inputs(
    request_max_concurrency: Option<u64>,
    request_min_start_interval_ms: Option<u64>,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
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

fn normalize_required_proxy_url(raw: &str) -> Result<String> {
    let value = raw.trim();
    if value.is_empty() {
        anyhow::bail!("proxy_url is required");
    }
    validate_proxy_url(value)?;
    Ok(value.to_string())
}

fn normalize_optional_secret(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn validate_provider_type(provider_type: &str) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    match provider_type {
        LLM_GATEWAY_PROVIDER_CODEX | LLM_GATEWAY_PROVIDER_KIRO => Ok(()),
        _ => Err(bad_request("provider_type must be `codex` or `kiro`")),
    }
}

fn build_proxy_config_record(
    existing: Option<&LlmGatewayProxyConfigRecord>,
    request: CreateAdminUpstreamProxyConfigRequest,
) -> Result<LlmGatewayProxyConfigRecord, (StatusCode, Json<ErrorResponse>)> {
    let now = now_ms();
    Ok(LlmGatewayProxyConfigRecord {
        id: existing
            .map(|value| value.id.clone())
            .unwrap_or_else(|| generate_id("llm-proxy")),
        name: normalize_name(&request.name)?,
        proxy_url: normalize_required_proxy_url(&request.proxy_url)
            .map_err(|err| bad_request_with_detail("invalid proxy_url", err))?,
        proxy_username: normalize_optional_secret(request.proxy_username.as_deref()),
        proxy_password: normalize_optional_secret(request.proxy_password.as_deref()),
        status: existing
            .map(|value| value.status.clone())
            .unwrap_or_else(|| LLM_GATEWAY_KEY_STATUS_ACTIVE.to_string()),
        created_at: existing.map(|value| value.created_at).unwrap_or(now),
        updated_at: now,
    })
}

async fn load_proxy_binding_views(state: &AppState) -> Result<Vec<AdminUpstreamProxyBindingView>> {
    let mut views = Vec::new();
    let configs = state.llm_gateway_store.list_proxy_configs().await?;
    let configs_by_id = configs
        .into_iter()
        .map(|record| (record.id.clone(), record))
        .collect::<std::collections::HashMap<_, _>>();
    let bindings = state
        .llm_gateway_store
        .list_proxy_bindings()
        .await?
        .into_iter()
        .map(|record| (record.provider_type.clone(), record))
        .collect::<std::collections::HashMap<_, _>>();

    for provider_type in [LLM_GATEWAY_PROVIDER_CODEX, LLM_GATEWAY_PROVIDER_KIRO] {
        let binding = bindings.get(provider_type);
        match state
            .upstream_proxy_registry
            .resolve_provider_proxy(provider_type)
            .await
        {
            Ok(resolved) => views.push(AdminUpstreamProxyBindingView {
                provider_type: provider_type.to_string(),
                effective_source: resolved.source.as_str().to_string(),
                bound_proxy_config_id: binding.map(|value| value.proxy_config_id.clone()),
                effective_proxy_config_name: resolved.proxy_config_name.clone(),
                effective_proxy_url: resolved.proxy_url.clone(),
                effective_proxy_username: resolved.proxy_username.clone(),
                effective_proxy_password: resolved.proxy_password.clone(),
                binding_updated_at: resolved.binding_updated_at,
                error_message: None,
            }),
            Err(err) => {
                let bound_proxy_config_id = binding.map(|value| value.proxy_config_id.clone());
                let effective_proxy_config_name = bound_proxy_config_id
                    .as_ref()
                    .and_then(|id| configs_by_id.get(id))
                    .map(|config| config.name.clone());
                views.push(AdminUpstreamProxyBindingView {
                    provider_type: provider_type.to_string(),
                    effective_source: "invalid".to_string(),
                    bound_proxy_config_id,
                    effective_proxy_config_name,
                    effective_proxy_url: None,
                    effective_proxy_username: None,
                    effective_proxy_password: None,
                    binding_updated_at: binding.map(|value| value.updated_at),
                    error_message: Some(err.to_string()),
                });
            },
        }
    }

    Ok(views)
}

async fn run_proxy_connectivity_check(
    state: &AppState,
    config: &LlmGatewayProxyConfigRecord,
    provider_type: &str,
) -> Result<AdminUpstreamProxyCheckResponse> {
    let (auth_label, target) = match provider_type {
        LLM_GATEWAY_PROVIDER_CODEX => run_codex_proxy_connectivity_check(state, config).await?,
        LLM_GATEWAY_PROVIDER_KIRO => run_kiro_proxy_connectivity_check(state, config).await?,
        _ => unreachable!("provider type must be validated before dispatch"),
    };

    Ok(AdminUpstreamProxyCheckResponse {
        proxy_config_id: config.id.clone(),
        proxy_config_name: config.name.clone(),
        provider_type: provider_type.to_string(),
        auth_label,
        ok: target.reachable,
        targets: vec![target],
        checked_at: now_ms(),
    })
}

async fn run_codex_proxy_connectivity_check(
    state: &AppState,
    config: &LlmGatewayProxyConfigRecord,
) -> Result<(String, AdminUpstreamProxyCheckTargetView)> {
    let (auth_snapshot, auth_label) = if let Some((account_name, snapshot, _)) = state
        .llm_gateway
        .account_pool
        .select_best_account(None)
        .await
    {
        (snapshot, format!("Codex account `{account_name}`"))
    } else {
        (
            state.llm_gateway.auth_source.current().await?,
            "legacy Codex auth `~/.codex/auth.json`".to_string(),
        )
    };
    let upstream_base = std::env::var("STATICFLOW_LLM_GATEWAY_UPSTREAM_BASE_URL")
        .ok()
        .map(|value| normalize_upstream_base_url(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_UPSTREAM_BASE_URL.to_string());
    let codex_client_version = resolve_codex_client_version(Some(
        &state.llm_gateway_runtime_config.read().codex_client_version,
    ));
    let url = append_client_version_query(
        &compute_upstream_url(&upstream_base, "/v1/models"),
        &codex_client_version,
    );
    let client = build_proxy_client(config, PROXY_CONNECTIVITY_CHECK_TIMEOUT_SECONDS)?;
    let mut headers = ReqwestHeaderMap::new();
    headers.insert(header::AUTHORIZATION, bearer_header(&auth_snapshot.access_token)?);
    headers.insert(header::ACCEPT, ReqwestHeaderValue::from_static("application/json"));
    headers.insert(
        header::USER_AGENT,
        ReqwestHeaderValue::from_str(&codex_user_agent(&codex_client_version))?,
    );
    headers.insert(
        reqwest::header::HeaderName::from_static("originator"),
        ReqwestHeaderValue::from_static(DEFAULT_WIRE_ORIGINATOR),
    );
    if let Some(account_id) = auth_snapshot.account_id.as_deref() {
        headers.insert(
            reqwest::header::HeaderName::from_static("chatgpt-account-id"),
            ReqwestHeaderValue::from_str(account_id)?,
        );
    }
    let started_at = Instant::now();
    let response = client
        .get(&url)
        .headers(headers)
        .send()
        .await
        .context("request codex proxy connectivity check")?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    Ok((auth_label, AdminUpstreamProxyCheckTargetView {
        target: LLM_GATEWAY_PROVIDER_CODEX.to_string(),
        url,
        reachable: status.is_success(),
        status_code: Some(status.as_u16()),
        latency_ms: started_at.elapsed().as_millis() as i64,
        error_message: (!status.is_success()).then(|| summarize_upstream_error_body(&body)),
    }))
}

/// Run a connectivity check against the Kiro upstream through the given proxy.
/// Picks the first non-disabled account for the probe.
async fn run_kiro_proxy_connectivity_check(
    state: &AppState,
    config: &LlmGatewayProxyConfigRecord,
) -> Result<(String, AdminUpstreamProxyCheckTargetView)> {
    let auths = state.kiro_gateway.token_manager.list_auths().await?;
    let account_name = auths
        .iter()
        .find(|item| !item.disabled)
        .map(|item| item.name.clone())
        .ok_or_else(|| anyhow!("no kiro account available for proxy check"))?;
    let ctx = state
        .kiro_gateway
        .token_manager
        .ensure_context_for_account(&account_name, false)
        .await?;
    let auth = ctx.auth.clone();
    let region = auth.effective_api_region().to_string();
    let host = format!("q.{region}.amazonaws.com");
    let url = if let Some(profile_arn) = auth.profile_arn.as_deref() {
        format!(
            "https://{host}/getUsageLimits?origin=AI_EDITOR&resourceType=AGENTIC_REQUEST&profileArn={}",
            urlencoding::encode(profile_arn)
        )
    } else {
        format!("https://{host}/getUsageLimits?origin=AI_EDITOR&resourceType=AGENTIC_REQUEST")
    };
    let machine_id =
        crate::kiro_gateway::machine_id::generate_from_auth(&auth).ok_or_else(|| {
            anyhow!("failed to derive machine_id from selected kiro auth `{account_name}`")
        })?;
    let user_agent = format!(
        "aws-sdk-js/1.0.0 ua/2.1 os/{} lang/js md/nodejs#{} api/codewhispererruntime#1.0.0 m/N,E \
         KiroIDE-{}-{}",
        crate::kiro_gateway::auth_file::DEFAULT_SYSTEM_VERSION,
        crate::kiro_gateway::auth_file::DEFAULT_NODE_VERSION,
        crate::kiro_gateway::auth_file::DEFAULT_KIRO_VERSION,
        machine_id
    );
    let amz_user_agent = format!(
        "aws-sdk-js/1.0.0 KiroIDE-{}-{}",
        crate::kiro_gateway::auth_file::DEFAULT_KIRO_VERSION,
        machine_id
    );
    let client = build_proxy_client(config, PROXY_CONNECTIVITY_CHECK_TIMEOUT_SECONDS)?;
    let started_at = Instant::now();
    let response = client
        .get(&url)
        .header("x-amz-user-agent", amz_user_agent)
        .header("user-agent", user_agent)
        .header("host", host)
        .header("amz-sdk-invocation-id", uuid::Uuid::new_v4().to_string())
        .header("amz-sdk-request", "attempt=1; max=1")
        .header("authorization", format!("Bearer {}", ctx.token))
        .header("connection", "close")
        .send()
        .await
        .context("request kiro proxy connectivity check")?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    Ok((format!("Kiro account `{account_name}`"), AdminUpstreamProxyCheckTargetView {
        target: LLM_GATEWAY_PROVIDER_KIRO.to_string(),
        url,
        reachable: status.is_success(),
        status_code: Some(status.as_u16()),
        latency_ms: started_at.elapsed().as_millis() as i64,
        error_message: (!status.is_success()).then(|| summarize_upstream_error_body(&body)),
    }))
}

fn build_proxy_client(
    config: &LlmGatewayProxyConfigRecord,
    timeout_secs: u64,
) -> Result<reqwest::Client> {
    validate_proxy_url(&config.proxy_url)?;
    let proxy = build_proxy_config(config)?;
    reqwest::Client::builder()
        .proxy(proxy)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(timeout_secs))
        .build()
        .context("build proxy connectivity client")
}

fn build_proxy_config(config: &LlmGatewayProxyConfigRecord) -> Result<reqwest::Proxy> {
    let mut proxy = reqwest::Proxy::all(&config.proxy_url)
        .with_context(|| format!("failed to build proxy `{}`", config.proxy_url))?;
    if let Some(username) = config.proxy_username.as_deref() {
        proxy = proxy.basic_auth(username, config.proxy_password.as_deref().unwrap_or(""));
    }
    Ok(proxy)
}

fn summarize_upstream_error_body(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        "empty body".to_string()
    } else {
        trimmed.chars().take(200).collect()
    }
}

fn maybe_parse_gateway_json_bytes(raw: &Bytes) -> serde_json::Value {
    if raw.is_empty() {
        return serde_json::Value::Null;
    }
    serde_json::from_slice::<serde_json::Value>(raw)
        .unwrap_or_else(|_| serde_json::Value::String(String::from_utf8_lossy(raw).to_string()))
}

fn maybe_raw_request_body_text(raw: &Bytes) -> Option<String> {
    (!raw.is_empty()).then(|| String::from_utf8_lossy(raw).to_string())
}

fn elapsed_ms_u64(started_at: Instant) -> u64 {
    started_at.elapsed().as_millis().min(u64::MAX as u128) as u64
}

fn elapsed_ms_i32(started_at: Instant) -> i32 {
    started_at.elapsed().as_millis().min(i32::MAX as u128) as i32
}

fn clamp_u64_ms_to_i32(value: u64) -> i32 {
    value.min(i32::MAX as u64) as i32
}

fn codex_routing_diagnostics_json(
    route_total_ms: u64,
    selected_account_name: Option<&str>,
    failover_count: u64,
    failed_account_names: &HashSet<String>,
) -> Option<String> {
    let mut failed_accounts = failed_account_names.iter().cloned().collect::<Vec<_>>();
    failed_accounts.sort();
    serde_json::to_string(&json!({
        "request_kind": "codex",
        "route_total_ms": route_total_ms,
        "account_attempt_count": failover_count.saturating_add(1),
        "failover_count": failover_count,
        "selected_account": selected_account_name,
        "failed_accounts": failed_accounts,
    }))
    .ok()
}

fn update_codex_route_metrics(
    event_context: &mut Option<LlmGatewayEventContext>,
    route_total_ms: u64,
    selected_account_name: Option<&str>,
    failover_count: u64,
    failed_account_names: &HashSet<String>,
) {
    let Some(context) = event_context.as_mut() else {
        return;
    };
    context.routing_wait_ms = Some(clamp_u64_ms_to_i32(route_total_ms));
    context.routing_diagnostics_json = codex_routing_diagnostics_json(
        route_total_ms,
        selected_account_name,
        failover_count,
        failed_account_names,
    );
}

fn default_gateway_event_context(prepared: &PreparedGatewayRequest) -> LlmGatewayEventContext {
    LlmGatewayEventContext {
        request_method: prepared.method.as_str().to_string(),
        request_url: prepared.original_path.clone(),
        client_ip: "unknown".to_string(),
        ip_region: "Unknown".to_string(),
        request_headers_json: "{}".to_string(),
        started_at: Instant::now(),
        routing_wait_ms: None,
        upstream_headers_ms: None,
        post_headers_body_ms: None,
        request_body_bytes: None,
        request_body_read_ms: None,
        request_json_parse_ms: None,
        pre_handler_ms: None,
        first_sse_write_ms: None,
        stream_finish_ms: None,
        routing_diagnostics_json: None,
        upstream_headers_at: None,
    }
}

struct GatewayUsageEventBuild<'a> {
    current: &'a LlmGatewayKeyRecord,
    prepared: &'a PreparedGatewayRequest,
    context: &'a LlmGatewayEventContext,
    latency_ms: i32,
    status_code: i32,
    usage: UsageBreakdown,
    last_message_content: Option<String>,
    selected_account_name: Option<&'a str>,
}

struct GatewayFailureUsageRequest<'a> {
    prepared: &'a PreparedGatewayRequest,
    status_code: i32,
    usage: UsageBreakdown,
    event_context: Option<&'a LlmGatewayEventContext>,
    selected_account_name: Option<&'a str>,
    failure_stage: &'a str,
    error: &'a str,
    details: Option<serde_json::Value>,
}

fn build_gateway_usage_event_record(
    args: GatewayUsageEventBuild<'_>,
) -> LlmGatewayUsageEventRecord {
    let capture_request_details = args.status_code >= 400;
    LlmGatewayUsageEventRecord {
        id: generate_id("llm-usage"),
        key_id: args.current.id.clone(),
        key_name: args.current.name.clone(),
        provider_type: LLM_GATEWAY_PROVIDER_CODEX.to_string(),
        account_name: args
            .selected_account_name
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        request_method: args.context.request_method.clone(),
        request_url: args.context.request_url.clone(),
        latency_ms: args.latency_ms,
        routing_wait_ms: args.context.routing_wait_ms,
        upstream_headers_ms: args.context.upstream_headers_ms,
        post_headers_body_ms: args
            .context
            .post_headers_body_ms
            .or_else(|| args.context.upstream_headers_at.map(elapsed_ms_i32)),
        request_body_bytes: args.context.request_body_bytes,
        request_body_read_ms: args.context.request_body_read_ms,
        request_json_parse_ms: args.context.request_json_parse_ms,
        pre_handler_ms: args.context.pre_handler_ms,
        first_sse_write_ms: args.context.first_sse_write_ms,
        stream_finish_ms: args.context.stream_finish_ms.or(Some(args.latency_ms)),
        quota_failover_count: 0,
        routing_diagnostics_json: args.context.routing_diagnostics_json.clone(),
        endpoint: args.prepared.upstream_path.clone(),
        model: args.prepared.model.clone(),
        status_code: args.status_code,
        input_uncached_tokens: args.usage.input_uncached_tokens,
        input_cached_tokens: args.usage.input_cached_tokens,
        output_tokens: args.usage.output_tokens,
        billable_tokens: args
            .usage
            .billable_tokens_with_multiplier(args.prepared.billable_multiplier),
        usage_missing: args.usage.usage_missing,
        credit_usage: None,
        credit_usage_missing: false,
        client_ip: args.context.client_ip.clone(),
        ip_region: args.context.ip_region.clone(),
        request_headers_json: args.context.request_headers_json.clone(),
        last_message_content: args.last_message_content,
        client_request_body_json: None,
        upstream_request_body_json: None,
        full_request_json: capture_request_details
            .then(|| maybe_raw_request_body_text(&args.prepared.client_request_body))
            .flatten(),
        created_at: now_ms(),
    }
}

fn build_codex_failure_diagnostic_payload(
    prepared: &PreparedGatewayRequest,
    event_context: Option<&LlmGatewayEventContext>,
    selected_account_name: Option<&str>,
    failure_stage: &str,
    status_code: i32,
    error: &str,
    details: Option<serde_json::Value>,
) -> String {
    serde_json::to_string_pretty(&serde_json::json!({
        "kind": "codex_failure_diagnostic",
        "failure_stage": failure_stage,
        "status_code": status_code,
        "request_method": event_context.map(|ctx| ctx.request_method.clone()),
        "request_url": event_context.map(|ctx| ctx.request_url.clone()),
        "endpoint": prepared.upstream_path,
        "model": prepared.model,
        "account_name": selected_account_name,
        "original_last_message_content": extract_last_message_content(&prepared.client_request_body).ok().flatten(),
        "client_request_body": maybe_parse_gateway_json_bytes(&prepared.client_request_body),
        "upstream_request_body": maybe_parse_gateway_json_bytes(&prepared.request_body),
        "error": error,
        "details": details.unwrap_or_else(|| json!({})),
    }))
    .unwrap_or_else(|serialize_err| {
        format!(
            "{{\"kind\":\"codex_failure_diagnostic\",\"failure_stage\":{:?},\"status_code\":{},\"error\":{:?},\"serialize_error\":{:?}}}",
            failure_stage,
            status_code,
            error,
            serialize_err.to_string()
        )
    })
}

fn missing_usage_breakdown() -> UsageBreakdown {
    UsageBreakdown {
        usage_missing: true,
        ..UsageBreakdown::default()
    }
}

fn extract_request_model_and_stream(raw_body: &Bytes) -> (Option<String>, bool) {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(raw_body) else {
        return (None, false);
    };
    let Some(root) = value.as_object() else {
        return (None, false);
    };
    let model = root
        .get("model")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let wants_stream = root
        .get("stream")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    (model, wants_stream)
}

fn build_failure_prepared_gateway_request(
    gateway_path: &str,
    query: &str,
    method: axum::http::Method,
    raw_body: Bytes,
    content_type: &str,
) -> PreparedGatewayRequest {
    let original_path = format!("{gateway_path}{query}");
    let (model, wants_stream) = extract_request_model_and_stream(&raw_body);
    PreparedGatewayRequest {
        original_path: original_path.clone(),
        upstream_path: original_path,
        method,
        client_request_body: raw_body,
        request_body: Bytes::new(),
        model,
        client_visible_model: None,
        wants_stream,
        force_upstream_stream: false,
        content_type: content_type.to_string(),
        response_adapter: if gateway_path == "/v1/chat/completions" {
            GatewayResponseAdapter::ChatCompletions
        } else {
            GatewayResponseAdapter::Responses
        },
        thread_anchor: None,
        tool_name_restore_map: BTreeMap::new(),
        billable_multiplier: 1,
    }
}

async fn validate_account_names_exist(
    pool: &AccountPool,
    names: &[String],
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    for name in names {
        if !pool.exists(name).await {
            return Err(bad_request(&format!("unknown account `{name}`")));
        }
    }
    Ok(())
}

async fn partition_existing_account_names(
    pool: &AccountPool,
    names: &[String],
) -> (Vec<String>, Vec<String>) {
    let mut existing = Vec::new();
    let mut missing = Vec::new();
    for name in names {
        if pool.exists(name).await {
            existing.push(name.clone());
        } else {
            missing.push(name.clone());
        }
    }
    (existing, missing)
}

/// Accept a public Codex account contribution request from `/llm-access`.
pub async fn submit_public_account_contribution_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SubmitLlmGatewayAccountContributionRequest>,
) -> Result<
    Json<SubmitLlmGatewayAccountContributionRequestResponse>,
    (StatusCode, Json<ErrorResponse>),
> {
    let account_name =
        accounts::validate_account_name(&request.account_name).map_err(|err| bad_request(&err))?;
    let account_id = request
        .account_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let id_token = request.id_token.trim().to_string();
    let access_token = request.access_token.trim().to_string();
    let refresh_token = request.refresh_token.trim().to_string();
    if id_token.is_empty() || access_token.is_empty() || refresh_token.is_empty() {
        return Err(bad_request("id_token, access_token, and refresh_token are required"));
    }
    let requester_email = normalize_requester_email_input(Some(request.requester_email))
        .map_err(|err| bad_request_with_detail("invalid requester_email", err))?
        .ok_or_else(|| bad_request("requester_email is required"))?;
    let contributor_message = request.contributor_message.trim();
    if contributor_message.is_empty() {
        return Err(bad_request("contributor_message is required"));
    }
    if contributor_message.chars().count() > MAX_PUBLIC_ACCOUNT_CONTRIBUTION_MESSAGE_CHARS {
        return Err(bad_request("contributor_message is too long"));
    }
    let github_id = normalize_optional_github_id_input(request.github_id)
        .map_err(|err| bad_request_with_detail("invalid github_id", err))?;
    let frontend_page_url = normalize_frontend_page_url_input(request.frontend_page_url)
        .map_err(|err| bad_request_with_detail("invalid frontend_page_url", err))?;

    let client_ip = extract_client_ip(&headers);
    let fingerprint = build_client_fingerprint(&headers);
    let rate_limit_key = build_submit_rate_limit_key(&headers, &fingerprint);
    enforce_public_submit_rate_limit(
        state.llm_gateway_public_submit_guard.as_ref(),
        &rate_limit_key,
        now_ms(),
        60,
        "llm-access public submission",
    )?;

    let request_id = generate_task_id("llmacct");
    let ip_region = state.geoip.resolve_region(&client_ip).await;
    let record = state
        .llm_gateway_store
        .create_account_contribution_request(NewLlmGatewayAccountContributionRequestInput {
            request_id: request_id.clone(),
            account_name,
            account_id,
            id_token,
            access_token,
            refresh_token,
            requester_email,
            contributor_message: contributor_message.to_string(),
            github_id,
            frontend_page_url,
            fingerprint,
            client_ip,
            ip_region,
        })
        .await
        .map_err(|err| {
            internal_error("Failed to create llm gateway account contribution request", err)
        })?;

    if let Some(notifier) = state.email_notifier.clone() {
        let record_for_email = record.clone();
        tokio::spawn(async move {
            if let Err(err) = notifier
                .send_admin_new_llm_account_contribution_request_notification(&record_for_email)
                .await
            {
                tracing::warn!(
                    "failed to send admin notification email for llm account contribution {}: {}",
                    record_for_email.request_id,
                    err
                );
            }
        });
    }

    Ok(Json(SubmitLlmGatewayAccountContributionRequestResponse {
        request_id,
        status: LLM_GATEWAY_TOKEN_REQUEST_STATUS_PENDING.to_string(),
    }))
}

/// List approved account contributions for the public thank-you wall.
pub async fn list_public_account_contributions(
    State(state): State<AppState>,
) -> Result<Json<PublicLlmGatewayAccountContributionsResponse>, (StatusCode, Json<ErrorResponse>)> {
    let contributions = state
        .llm_gateway_store
        .list_public_account_contributions(MAX_PUBLIC_ACCOUNT_CONTRIBUTIONS)
        .await
        .map_err(|err| {
            internal_error("Failed to list llm gateway public account contributions", err)
        })?;
    Ok(Json(PublicLlmGatewayAccountContributionsResponse {
        contributions: contributions
            .iter()
            .map(PublicLlmGatewayAccountContributionView::from)
            .collect(),
        generated_at: now_ms(),
    }))
}

/// Return public sponsor/community configuration for `/llm-access`.
pub async fn get_public_support_config(
    State(_state): State<AppState>,
) -> Result<Json<LlmGatewaySupportConfigView>, (StatusCode, Json<ErrorResponse>)> {
    let config = load_support_config()
        .map_err(|err| internal_error("Failed to load llm access support config", err))?;
    Ok(Json(LlmGatewaySupportConfigView {
        sponsor_title: config.sponsor_title.clone(),
        sponsor_intro: config.sponsor_intro.clone(),
        group_name: config.group_name.clone(),
        qq_group_number: config.qq_group_number.clone(),
        group_invite_text: config.group_invite_text.clone(),
        alipay_qr_url: format!("/api/llm-gateway/support-assets/{}", support::ALIPAY_QR_FILE),
        wechat_qr_url: format!("/api/llm-gateway/support-assets/{}", support::WECHAT_QR_FILE),
        qq_group_qr_url: config
            .has_group_qr()
            .then(|| format!("/api/llm-gateway/support-assets/{}", support::QQ_GROUP_QR_FILE)),
        generated_at: now_ms(),
    }))
}

/// Serve public support assets such as QR code images.
pub async fn get_public_support_asset(
    axum::extract::Path(file_name): axum::extract::Path<String>,
) -> Result<Response, (StatusCode, Json<ErrorResponse>)> {
    let config = load_support_config()
        .map_err(|err| internal_error("Failed to load llm access support config", err))?;
    let asset = load_support_asset(&config, &file_name)
        .map_err(|err| not_found(&format!("support asset not found: {err}")))?;
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, asset.content_type)
        .body(Body::from(asset.bytes))
        .map_err(|err| internal_error("Failed to build llm support asset response", err))
}

/// Accept a public sponsor request from `/llm-access`, then try to send the
/// payment instructions email immediately.
pub async fn submit_public_sponsor_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SubmitLlmGatewaySponsorRequest>,
) -> Result<Json<SubmitLlmGatewaySponsorRequestResponse>, (StatusCode, Json<ErrorResponse>)> {
    let requester_email = normalize_requester_email_input(Some(request.requester_email))
        .map_err(|err| bad_request_with_detail("invalid requester_email", err))?
        .ok_or_else(|| bad_request("requester_email is required"))?;
    let sponsor_message = request.sponsor_message.trim();
    if sponsor_message.is_empty() {
        return Err(bad_request("sponsor_message is required"));
    }
    if sponsor_message.chars().count() > MAX_PUBLIC_SPONSOR_MESSAGE_CHARS {
        return Err(bad_request("sponsor_message is too long"));
    }
    let display_name = normalize_optional_display_name_input(request.display_name)
        .map_err(|err| bad_request_with_detail("invalid display_name", err))?;
    let github_id = normalize_optional_github_id_input(request.github_id)
        .map_err(|err| bad_request_with_detail("invalid github_id", err))?;
    let frontend_page_url = normalize_frontend_page_url_input(request.frontend_page_url)
        .map_err(|err| bad_request_with_detail("invalid frontend_page_url", err))?;

    let client_ip = extract_client_ip(&headers);
    let fingerprint = build_client_fingerprint(&headers);
    let rate_limit_key = build_submit_rate_limit_key(&headers, &fingerprint);
    enforce_public_submit_rate_limit(
        state.llm_gateway_public_submit_guard.as_ref(),
        &rate_limit_key,
        now_ms(),
        60,
        "llm-access public submission",
    )?;

    let request_id = generate_task_id("llmsponsor");
    let ip_region = state.geoip.resolve_region(&client_ip).await;
    let mut record = state
        .llm_gateway_store
        .create_sponsor_request(NewLlmGatewaySponsorRequestInput {
            request_id: request_id.clone(),
            requester_email,
            sponsor_message: sponsor_message.to_string(),
            display_name,
            github_id,
            frontend_page_url,
            fingerprint,
            client_ip,
            ip_region,
        })
        .await
        .map_err(|err| internal_error("Failed to create llm gateway sponsor request", err))?;

    let mut payment_email_sent = false;
    if let Some(notifier) = state.email_notifier.clone() {
        match load_support_config().and_then(|config| {
            let markdown = render_payment_email_markdown(&config)?;
            Ok((config, markdown))
        }) {
            Ok((config, markdown)) => {
                match notifier
                    .send_llm_sponsor_payment_instructions(
                        &record.requester_email,
                        &config.payment_email_subject,
                        &markdown,
                        &config.base_dir,
                        config.reply_to_email.as_deref(),
                    )
                    .await
                {
                    Ok(_) => {
                        payment_email_sent = true;
                        record.status =
                            LLM_GATEWAY_SPONSOR_REQUEST_STATUS_PAYMENT_EMAIL_SENT.to_string();
                        record.failure_reason = None;
                        record.payment_email_sent_at = Some(now_ms());
                        record.updated_at = now_ms();
                    },
                    Err(err) => {
                        record.failure_reason = Some(err.to_string());
                        record.updated_at = now_ms();
                    },
                }
            },
            Err(err) => {
                record.failure_reason = Some(err.to_string());
                record.updated_at = now_ms();
            },
        }
    } else {
        record.failure_reason = Some("email notifier is not configured".to_string());
        record.updated_at = now_ms();
    }
    state
        .llm_gateway_store
        .upsert_sponsor_request(&record)
        .await
        .map_err(|err| internal_error("Failed to persist llm gateway sponsor request", err))?;

    Ok(Json(SubmitLlmGatewaySponsorRequestResponse {
        request_id,
        status: record.status.clone(),
        payment_email_sent,
    }))
}

/// List approved sponsors for the public thank-you wall.
pub async fn list_public_sponsors(
    State(state): State<AppState>,
) -> Result<Json<PublicLlmGatewaySponsorsResponse>, (StatusCode, Json<ErrorResponse>)> {
    let sponsors = state
        .llm_gateway_store
        .list_public_sponsors(MAX_PUBLIC_SPONSORS)
        .await
        .map_err(|err| internal_error("Failed to list llm gateway public sponsors", err))?;
    Ok(Json(PublicLlmGatewaySponsorsResponse {
        sponsors: sponsors
            .iter()
            .map(PublicLlmGatewaySponsorView::from)
            .collect(),
        generated_at: now_ms(),
    }))
}

/// List sponsor requests for the admin audit surface.
pub async fn list_admin_sponsor_requests(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<AdminLlmGatewaySponsorRequestQuery>,
) -> Result<Json<AdminLlmGatewaySponsorRequestsResponse>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;

    let offset = query.offset.unwrap_or(0);
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let total = state
        .llm_gateway_store
        .count_sponsor_requests(query.status.as_deref())
        .await
        .map_err(|err| internal_error("Failed to count llm gateway sponsor requests", err))?;
    if total == 0 || offset >= total {
        return Ok(Json(AdminLlmGatewaySponsorRequestsResponse {
            total,
            offset,
            limit,
            has_more: false,
            requests: vec![],
            generated_at: now_ms(),
        }));
    }

    let requests = state
        .llm_gateway_store
        .list_sponsor_requests_page(query.status.as_deref(), limit, offset)
        .await
        .map_err(|err| internal_error("Failed to list llm gateway sponsor requests", err))?;
    let has_more = offset.saturating_add(requests.len()) < total;

    Ok(Json(AdminLlmGatewaySponsorRequestsResponse {
        total,
        offset,
        limit,
        has_more,
        requests: requests
            .iter()
            .map(AdminLlmGatewaySponsorRequestView::from)
            .collect(),
        generated_at: now_ms(),
    }))
}

/// Mark a sponsor request as manually confirmed so it appears on the public
/// sponsor wall.
pub async fn approve_sponsor_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(request_id): axum::extract::Path<String>,
    Json(request): Json<AdminTaskActionRequest>,
) -> Result<Json<AdminLlmGatewaySponsorRequestView>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;

    let mut sponsor_request = state
        .llm_gateway_store
        .get_sponsor_request(&request_id)
        .await
        .map_err(|err| internal_error("Failed to load llm gateway sponsor request", err))?
        .ok_or_else(|| not_found("LLM gateway sponsor request not found"))?;

    if sponsor_request.status == LLM_GATEWAY_SPONSOR_REQUEST_STATUS_APPROVED {
        return Err(conflict_error("LLM gateway sponsor request is already approved"));
    }

    let now = now_ms();
    sponsor_request.status = LLM_GATEWAY_SPONSOR_REQUEST_STATUS_APPROVED.to_string();
    sponsor_request.admin_note = request.admin_note.clone();
    sponsor_request.failure_reason = None;
    sponsor_request.updated_at = now;
    sponsor_request.processed_at = Some(now);
    state
        .llm_gateway_store
        .upsert_sponsor_request(&sponsor_request)
        .await
        .map_err(|err| internal_error("Failed to approve llm gateway sponsor request", err))?;

    Ok(Json(AdminLlmGatewaySponsorRequestView::from(&sponsor_request)))
}

/// Delete one sponsor request from admin review/history.
pub async fn delete_sponsor_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(request_id): axum::extract::Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;

    let existing = state
        .llm_gateway_store
        .get_sponsor_request(&request_id)
        .await
        .map_err(|err| internal_error("Failed to load llm gateway sponsor request", err))?;
    if existing.is_none() {
        return Err(not_found("LLM gateway sponsor request not found"));
    }

    state
        .llm_gateway_store
        .delete_sponsor_request(&request_id)
        .await
        .map_err(|err| internal_error("Failed to delete llm gateway sponsor request", err))?;
    Ok(StatusCode::NO_CONTENT)
}

/// List token wishes for the admin audit surface.
pub async fn list_admin_token_requests(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<AdminLlmGatewayTokenRequestQuery>,
) -> Result<Json<AdminLlmGatewayTokenRequestsResponse>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;

    let offset = query.offset.unwrap_or(0);
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let total = state
        .llm_gateway_store
        .count_token_requests(query.status.as_deref())
        .await
        .map_err(|err| internal_error("Failed to count llm gateway token requests", err))?;
    if total == 0 || offset >= total {
        return Ok(Json(AdminLlmGatewayTokenRequestsResponse {
            total,
            offset,
            limit,
            has_more: false,
            requests: vec![],
            generated_at: now_ms(),
        }));
    }

    let requests = state
        .llm_gateway_store
        .list_token_requests_page(query.status.as_deref(), limit, offset)
        .await
        .map_err(|err| internal_error("Failed to list llm gateway token requests", err))?;
    let has_more = offset.saturating_add(requests.len()) < total;

    Ok(Json(AdminLlmGatewayTokenRequestsResponse {
        total,
        offset,
        limit,
        has_more,
        requests: requests
            .iter()
            .map(AdminLlmGatewayTokenRequestView::from)
            .collect(),
        generated_at: now_ms(),
    }))
}

/// Approve a token wish, create the key if needed, and email it to the user.
pub async fn approve_and_issue_token_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(request_id): axum::extract::Path<String>,
    Json(request): Json<AdminTaskActionRequest>,
) -> Result<Json<AdminLlmGatewayTokenRequestView>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;

    let mut token_request = state
        .llm_gateway_store
        .get_token_request(&request_id)
        .await
        .map_err(|err| internal_error("Failed to load llm gateway token request", err))?
        .ok_or_else(|| not_found("LLM gateway token request not found"))?;

    match token_request.status.as_str() {
        LLM_GATEWAY_TOKEN_REQUEST_STATUS_ISSUED | LLM_GATEWAY_TOKEN_REQUEST_STATUS_REJECTED => {
            return Err(conflict_error("LLM gateway token request is finalized"));
        },
        _ => {},
    }

    let Some(notifier) = state.email_notifier.clone() else {
        token_request.status = LLM_GATEWAY_TOKEN_REQUEST_STATUS_FAILED.to_string();
        token_request.failure_reason = Some("email notifier is not configured".to_string());
        token_request.updated_at = now_ms();
        token_request.processed_at = Some(now_ms());
        state
            .llm_gateway_store
            .upsert_token_request(&token_request)
            .await
            .map_err(|err| {
                internal_error("Failed to persist llm gateway token request failure", err)
            })?;
        return Err(internal_error(
            "Failed to send llm gateway token email",
            "email notifier is not configured",
        ));
    };

    let key = if let Some(existing_key_id) = token_request.issued_key_id.as_deref() {
        state
            .llm_gateway_store
            .get_key_by_id(existing_key_id)
            .await
            .map_err(|err| internal_error("Failed to reload issued llm gateway key", err))?
            .ok_or_else(|| not_found("Previously issued LLM gateway key not found"))?
    } else {
        let key_name = normalize_name(&format!("wish-{}", token_request.request_id))?;
        create_managed_key_record(&state, ManagedKeyCreateInput {
            name: key_name,
            quota_billable_limit: token_request.requested_quota_billable_limit,
            public_visible: false,
            route_strategy: None,
            account_group_id: None,
            fixed_account_name: None,
            auto_account_names: None,
            model_name_map: None,
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
        })
        .await?
    };

    let gateway_base_url = token_request
        .frontend_page_url
        .as_deref()
        .and_then(|url| build_llm_gateway_base_url(url).ok())
        .or_else(|| {
            env::var("SITE_BASE_URL")
                .ok()
                .map(|base| format!("{}/api/llm-gateway/v1", base.trim_end_matches('/')))
        })
        .unwrap_or_else(|| "/api/llm-gateway/v1".to_string());
    let llm_access_url = token_request
        .frontend_page_url
        .as_deref()
        .and_then(|url| build_llm_access_url(url).ok());

    let now = now_ms();
    token_request.admin_note = request.admin_note.clone();
    token_request.failure_reason = None;
    token_request.issued_key_id = Some(key.id.clone());
    token_request.issued_key_name = Some(key.name.clone());
    token_request.updated_at = now;
    token_request.processed_at = Some(now);
    let mut issued_request = token_request.clone();
    issued_request.status = LLM_GATEWAY_TOKEN_REQUEST_STATUS_ISSUED.to_string();
    let email_result = notifier
        .send_user_llm_token_issued_notification(
            &issued_request,
            &key,
            &gateway_base_url,
            llm_access_url.as_deref(),
        )
        .await;

    match email_result {
        Ok(_) => {
            token_request = issued_request;
            state
                .llm_gateway_store
                .upsert_token_request(&token_request)
                .await
                .map_err(|err| {
                    internal_error("Failed to finalize llm gateway token request", err)
                })?;
            Ok(Json(AdminLlmGatewayTokenRequestView::from(&token_request)))
        },
        Err(err) => {
            token_request.status = LLM_GATEWAY_TOKEN_REQUEST_STATUS_FAILED.to_string();
            token_request.failure_reason = Some(err.to_string());
            state
                .llm_gateway_store
                .upsert_token_request(&token_request)
                .await
                .map_err(|upsert_err| {
                    internal_error(
                        "Failed to persist llm gateway token request failure",
                        upsert_err,
                    )
                })?;
            Err(internal_error("Failed to send llm gateway token email", err))
        },
    }
}

/// Reject a token wish without creating any key.
pub async fn reject_token_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(request_id): axum::extract::Path<String>,
    Json(request): Json<AdminTaskActionRequest>,
) -> Result<Json<AdminLlmGatewayTokenRequestView>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;

    let mut token_request = state
        .llm_gateway_store
        .get_token_request(&request_id)
        .await
        .map_err(|err| internal_error("Failed to load llm gateway token request", err))?
        .ok_or_else(|| not_found("LLM gateway token request not found"))?;

    if token_request.status == LLM_GATEWAY_TOKEN_REQUEST_STATUS_ISSUED {
        return Err(conflict_error("Issued LLM gateway token request cannot be rejected"));
    }
    if token_request.status == LLM_GATEWAY_TOKEN_REQUEST_STATUS_REJECTED {
        return Err(conflict_error("LLM gateway token request is already rejected"));
    }

    if token_request.status != LLM_GATEWAY_TOKEN_REQUEST_STATUS_ISSUED {
        if let Some(key_id) = token_request.issued_key_id.as_deref() {
            if let Some(mut key) = state
                .llm_gateway_store
                .get_key_by_id(key_id)
                .await
                .map_err(|err| {
                    internal_error("Failed to load partially issued llm gateway key", err)
                })?
            {
                if key.status == LLM_GATEWAY_KEY_STATUS_ACTIVE {
                    key.status = LLM_GATEWAY_KEY_STATUS_DISABLED.to_string();
                    key.updated_at = now_ms();
                    state
                        .llm_gateway_store
                        .upsert_key(&key)
                        .await
                        .map_err(|err| {
                            internal_error(
                                "Failed to disable partially issued llm gateway key",
                                err,
                            )
                        })?;
                    state.llm_gateway.key_cache.invalidate(&key.key_hash);
                }
            }
        }
    }

    let now = now_ms();
    token_request.status = LLM_GATEWAY_TOKEN_REQUEST_STATUS_REJECTED.to_string();
    token_request.admin_note = request.admin_note.clone();
    token_request.failure_reason = None;
    token_request.updated_at = now;
    token_request.processed_at = Some(now);
    state
        .llm_gateway_store
        .upsert_token_request(&token_request)
        .await
        .map_err(|err| internal_error("Failed to reject llm gateway token request", err))?;

    Ok(Json(AdminLlmGatewayTokenRequestView::from(&token_request)))
}

/// List Codex account contribution requests for admin review.
pub async fn list_admin_account_contribution_requests(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<
        AdminLlmGatewayAccountContributionRequestQuery,
    >,
) -> Result<
    Json<AdminLlmGatewayAccountContributionRequestsResponse>,
    (StatusCode, Json<ErrorResponse>),
> {
    ensure_admin_access(&state, &headers)?;

    let offset = query.offset.unwrap_or(0);
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let total = state
        .llm_gateway_store
        .count_account_contribution_requests(query.status.as_deref())
        .await
        .map_err(|err| {
            internal_error("Failed to count llm gateway account contribution requests", err)
        })?;
    if total == 0 || offset >= total {
        return Ok(Json(AdminLlmGatewayAccountContributionRequestsResponse {
            total,
            offset,
            limit,
            has_more: false,
            requests: vec![],
            generated_at: now_ms(),
        }));
    }

    let requests = state
        .llm_gateway_store
        .list_account_contribution_requests_page(query.status.as_deref(), limit, offset)
        .await
        .map_err(|err| {
            internal_error("Failed to list llm gateway account contribution requests", err)
        })?;
    let has_more = offset.saturating_add(requests.len()) < total;

    Ok(Json(AdminLlmGatewayAccountContributionRequestsResponse {
        total,
        offset,
        limit,
        has_more,
        requests: requests
            .iter()
            .map(AdminLlmGatewayAccountContributionRequestView::from)
            .collect(),
        generated_at: now_ms(),
    }))
}

/// Approve an account contribution, import the account, issue a bound key,
/// and email the contributor.
pub async fn approve_and_issue_account_contribution_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(request_id): axum::extract::Path<String>,
    Json(request): Json<AdminTaskActionRequest>,
) -> Result<Json<AdminLlmGatewayAccountContributionRequestView>, (StatusCode, Json<ErrorResponse>)>
{
    ensure_admin_access(&state, &headers)?;

    let mut contribution_request = state
        .llm_gateway_store
        .get_account_contribution_request(&request_id)
        .await
        .map_err(|err| {
            internal_error("Failed to load llm gateway account contribution request", err)
        })?
        .ok_or_else(|| not_found("LLM gateway account contribution request not found"))?;

    match contribution_request.status.as_str() {
        LLM_GATEWAY_TOKEN_REQUEST_STATUS_ISSUED | LLM_GATEWAY_TOKEN_REQUEST_STATUS_REJECTED => {
            return Err(conflict_error("LLM gateway account contribution request is finalized"));
        },
        _ => {},
    }

    let Some(notifier) = state.email_notifier.clone() else {
        contribution_request.status = LLM_GATEWAY_TOKEN_REQUEST_STATUS_FAILED.to_string();
        contribution_request.failure_reason = Some("email notifier is not configured".to_string());
        contribution_request.updated_at = now_ms();
        contribution_request.processed_at = Some(now_ms());
        state
            .llm_gateway_store
            .upsert_account_contribution_request(&contribution_request)
            .await
            .map_err(|err| {
                internal_error(
                    "Failed to persist llm gateway account contribution request failure",
                    err,
                )
            })?;
        return Err(internal_error(
            "Failed to send llm gateway contribution email",
            "email notifier is not configured",
        ));
    };

    let auth = runtime::CodexAuthSnapshot::from_tokens(
        contribution_request.access_token.clone(),
        contribution_request.account_id.clone(),
    );
    let codex_client_version = resolve_codex_client_version(Some(
        &state.llm_gateway_runtime_config.read().codex_client_version,
    ));
    let usage = match token_refresh::validate_account_usage(
        state.upstream_proxy_registry.as_ref(),
        &auth,
        &codex_client_version,
    )
    .await
    {
        Ok(usage) => usage,
        Err(err) => {
            contribution_request.status = LLM_GATEWAY_TOKEN_REQUEST_STATUS_FAILED.to_string();
            contribution_request.failure_reason = Some(err.to_string());
            contribution_request.updated_at = now_ms();
            contribution_request.processed_at = Some(now_ms());
            state
                .llm_gateway_store
                .upsert_account_contribution_request(&contribution_request)
                .await
                .map_err(|upsert_err| {
                    internal_error(
                        "Failed to persist llm gateway account contribution request failure",
                        upsert_err,
                    )
                })?;
            return Err(bad_request(&format!("account verification failed: {err}")));
        },
    };

    let imported_account_name = contribution_request
        .imported_account_name
        .clone()
        .unwrap_or_else(|| contribution_request.account_name.clone());
    let pool = &state.llm_gateway.account_pool;
    if contribution_request.imported_account_name.is_none()
        && pool.exists(&imported_account_name).await
    {
        contribution_request.status = LLM_GATEWAY_TOKEN_REQUEST_STATUS_FAILED.to_string();
        contribution_request.failure_reason =
            Some(format!("account `{imported_account_name}` already exists"));
        contribution_request.updated_at = now_ms();
        contribution_request.processed_at = Some(now_ms());
        state
            .llm_gateway_store
            .upsert_account_contribution_request(&contribution_request)
            .await
            .map_err(|err| {
                internal_error(
                    "Failed to persist llm gateway account contribution request failure",
                    err,
                )
            })?;
        return Err(conflict_error("LLM gateway account already exists"));
    }

    if !pool.exists(&imported_account_name).await {
        let account = accounts::CodexAccount {
            name: imported_account_name.clone(),
            access_token: contribution_request.access_token.clone(),
            account_id: contribution_request.account_id.clone(),
            refresh_token: contribution_request.refresh_token.clone(),
            id_token: contribution_request.id_token.clone(),
            map_gpt53_codex_to_spark: false,
            proxy_selection: Default::default(),
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
            last_refresh: Some(chrono::Utc::now()),
            status: accounts::AccountStatus::Active,
        };
        pool.insert(account)
            .await
            .map_err(|err| internal_error("Failed to persist contributed account", err))?;
    }
    pool.update_rate_limit(&imported_account_name, usage.clone())
        .await;
    contribution_request.imported_account_name = Some(imported_account_name.clone());

    let key = if let Some(existing_key_id) = contribution_request.issued_key_id.as_deref() {
        match state
            .llm_gateway_store
            .get_key_by_id(existing_key_id)
            .await
            .map_err(|err| {
                internal_error("Failed to reload issued llm gateway contribution key", err)
            })? {
            Some(existing) => existing,
            None => {
                let key_name =
                    normalize_name(&format!("contrib-{}", contribution_request.request_id))?;
                let issued_group = create_single_account_group_for_key(
                    &state,
                    LLM_GATEWAY_PROVIDER_CODEX,
                    &key_name,
                    existing_key_id,
                    &imported_account_name,
                )
                .await?;
                create_managed_key_record(&state, ManagedKeyCreateInput {
                    name: key_name,
                    quota_billable_limit: 100_000_000_000,
                    public_visible: false,
                    route_strategy: Some("fixed".to_string()),
                    account_group_id: Some(issued_group.id.clone()),
                    fixed_account_name: None,
                    auto_account_names: None,
                    model_name_map: None,
                    request_max_concurrency: None,
                    request_min_start_interval_ms: None,
                })
                .await?
            },
        }
    } else {
        let key_name = normalize_name(&format!("contrib-{}", contribution_request.request_id))?;
        let issued_group = create_single_account_group_for_key(
            &state,
            LLM_GATEWAY_PROVIDER_CODEX,
            &key_name,
            &contribution_request.request_id,
            &imported_account_name,
        )
        .await?;
        create_managed_key_record(&state, ManagedKeyCreateInput {
            name: key_name,
            quota_billable_limit: 100_000_000_000,
            public_visible: false,
            route_strategy: Some("fixed".to_string()),
            account_group_id: Some(issued_group.id.clone()),
            fixed_account_name: None,
            auto_account_names: None,
            model_name_map: None,
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
        })
        .await?
    };

    let gateway_base_url = contribution_request
        .frontend_page_url
        .as_deref()
        .and_then(|url| build_llm_gateway_base_url(url).ok())
        .or_else(|| {
            env::var("SITE_BASE_URL")
                .ok()
                .map(|base| format!("{}/api/llm-gateway/v1", base.trim_end_matches('/')))
        })
        .unwrap_or_else(|| "/api/llm-gateway/v1".to_string());
    let llm_access_url = contribution_request
        .frontend_page_url
        .as_deref()
        .and_then(|url| build_llm_access_url(url).ok());

    let now = now_ms();
    contribution_request.admin_note = request.admin_note.clone();
    contribution_request.failure_reason = None;
    contribution_request.issued_key_id = Some(key.id.clone());
    contribution_request.issued_key_name = Some(key.name.clone());
    contribution_request.updated_at = now;
    contribution_request.processed_at = Some(now);
    let mut issued_request = contribution_request.clone();
    issued_request.status = LLM_GATEWAY_TOKEN_REQUEST_STATUS_ISSUED.to_string();

    let email_result = notifier
        .send_user_llm_account_contribution_issued_notification(
            &issued_request,
            &key,
            &gateway_base_url,
            llm_access_url.as_deref(),
        )
        .await;

    match email_result {
        Ok(_) => {
            contribution_request = issued_request;
            state
                .llm_gateway_store
                .upsert_account_contribution_request(&contribution_request)
                .await
                .map_err(|err| {
                    internal_error(
                        "Failed to finalize llm gateway account contribution request",
                        err,
                    )
                })?;
            Ok(Json(AdminLlmGatewayAccountContributionRequestView::from(&contribution_request)))
        },
        Err(err) => {
            contribution_request.status = LLM_GATEWAY_TOKEN_REQUEST_STATUS_FAILED.to_string();
            contribution_request.failure_reason = Some(err.to_string());
            state
                .llm_gateway_store
                .upsert_account_contribution_request(&contribution_request)
                .await
                .map_err(|upsert_err| {
                    internal_error(
                        "Failed to persist llm gateway account contribution request failure",
                        upsert_err,
                    )
                })?;
            Err(internal_error("Failed to send llm gateway contribution email", err))
        },
    }
}

/// Reject an account contribution request and clean up any partial account/key.
pub async fn reject_account_contribution_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(request_id): axum::extract::Path<String>,
    Json(request): Json<AdminTaskActionRequest>,
) -> Result<Json<AdminLlmGatewayAccountContributionRequestView>, (StatusCode, Json<ErrorResponse>)>
{
    ensure_admin_access(&state, &headers)?;

    let mut contribution_request = state
        .llm_gateway_store
        .get_account_contribution_request(&request_id)
        .await
        .map_err(|err| {
            internal_error("Failed to load llm gateway account contribution request", err)
        })?
        .ok_or_else(|| not_found("LLM gateway account contribution request not found"))?;

    if contribution_request.status == LLM_GATEWAY_TOKEN_REQUEST_STATUS_ISSUED {
        return Err(conflict_error(
            "Issued LLM gateway account contribution request cannot be rejected",
        ));
    }
    if contribution_request.status == LLM_GATEWAY_TOKEN_REQUEST_STATUS_REJECTED {
        return Err(conflict_error("LLM gateway account contribution request is already rejected"));
    }

    if let Some(key_id) = contribution_request.issued_key_id.as_deref() {
        if let Some(mut key) = state
            .llm_gateway_store
            .get_key_by_id(key_id)
            .await
            .map_err(|err| {
                internal_error("Failed to load partially issued llm gateway contribution key", err)
            })?
        {
            if key.status == LLM_GATEWAY_KEY_STATUS_ACTIVE {
                key.status = LLM_GATEWAY_KEY_STATUS_DISABLED.to_string();
                key.updated_at = now_ms();
                state
                    .llm_gateway_store
                    .upsert_key(&key)
                    .await
                    .map_err(|err| {
                        internal_error(
                            "Failed to disable partially issued llm gateway contribution key",
                            err,
                        )
                    })?;
                state.llm_gateway.key_cache.invalidate(&key.key_hash);
            }
        }
    }

    if let Some(account_name) = contribution_request.imported_account_name.as_deref() {
        state
            .llm_gateway
            .account_pool
            .remove(account_name)
            .await
            .map_err(|err| {
                internal_error("Failed to remove partially imported contributed account", err)
            })?;
    }

    let now = now_ms();
    contribution_request.status = LLM_GATEWAY_TOKEN_REQUEST_STATUS_REJECTED.to_string();
    contribution_request.admin_note = request.admin_note.clone();
    contribution_request.failure_reason = None;
    contribution_request.updated_at = now;
    contribution_request.processed_at = Some(now);
    state
        .llm_gateway_store
        .upsert_account_contribution_request(&contribution_request)
        .await
        .map_err(|err| {
            internal_error("Failed to reject llm gateway account contribution request", err)
        })?;

    Ok(Json(AdminLlmGatewayAccountContributionRequestView::from(&contribution_request)))
}

/// Start the background worker that refreshes the public rate-limit cache on a
/// fixed cadence.
pub fn spawn_public_rate_limit_refresher(
    runtime: Arc<LlmGatewayRuntimeState>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) {
    tokio::spawn(async move {
        let mut ticker = public_rate_limit_refresh_interval();

        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        tracing::info!("LLM gateway public rate-limit refresher shutting down");
                        return;
                    }
                }
                _ = ticker.tick() => {
                    if let Err(err) = refresh_public_rate_limit_status(&runtime).await {
                        tracing::warn!("Failed to refresh cached public rate-limit status: {err:#}");
                    }
                }
            }
        }
    });
}

/// Refresh the cached Codex account rate-limit snapshot once.
///
/// When the account pool has entries the public status is assembled from the
/// per-account snapshots that the background `token_refresh` task already
/// maintains — no extra upstream requests are made here.  When the pool is
/// empty the legacy single-file `CodexAuthSource` path fires one upstream
/// request as before.
pub async fn refresh_public_rate_limit_status(runtime: &Arc<LlmGatewayRuntimeState>) -> Result<()> {
    let checked_at = now_ms();
    let refresh_interval_seconds = runtime
        .runtime_config
        .read()
        .codex_status_refresh_max_interval_seconds;
    let source_url = compute_rate_limit_status_url();

    let pool_entries = runtime.account_pool.all_entries().await;

    if pool_entries.is_empty() {
        // Legacy single-file path — one upstream request.
        match fetch_rate_limit_status_snapshot(runtime, &source_url).await {
            Ok(buckets) => {
                let mut status = runtime.rate_limit_status.write();
                *status = LlmGatewayRateLimitStatusResponse {
                    status: "ready".to_string(),
                    refresh_interval_seconds,
                    last_checked_at: Some(checked_at),
                    last_success_at: Some(checked_at),
                    source_url,
                    error_message: None,
                    accounts: Vec::new(),
                    buckets,
                };
                tracing::info!(
                    bucket_count = status.buckets.len(),
                    last_success_at = status.last_success_at.unwrap_or_default(),
                    "Refreshed cached public LLM gateway rate-limit status"
                );
                Ok(())
            },
            Err(err) => {
                let mut status = runtime.rate_limit_status.write();
                let had_snapshot = !status.buckets.is_empty();
                let previous_success_at = status.last_success_at;
                status.status =
                    if had_snapshot { "degraded".to_string() } else { "error".to_string() };
                status.refresh_interval_seconds = refresh_interval_seconds;
                status.last_checked_at = Some(checked_at);
                status.last_success_at = previous_success_at;
                status.source_url = source_url;
                status.error_message = Some(err.to_string());
                status.accounts.clear();
                tracing::warn!(
                    had_snapshot,
                    last_success_at = previous_success_at.unwrap_or_default(),
                    "Failed to refresh cached public LLM gateway rate-limit status: {err:#}"
                );
                Err(err)
            },
        }
    } else {
        // Multi-account: read the already-cached per-account snapshots kept
        // fresh by the background refresh task instead of hitting upstream.
        let summaries = runtime.account_pool.list_summaries().await;
        let (accounts, buckets, status_label, error_message) =
            summarize_public_multi_account_status(&summaries);
        {
            let mut status = runtime.rate_limit_status.write();
            *status = LlmGatewayRateLimitStatusResponse {
                status: status_label,
                refresh_interval_seconds,
                last_checked_at: Some(checked_at),
                last_success_at: Some(checked_at),
                source_url,
                error_message,
                accounts,
                buckets,
            };
            tracing::info!(
                account_count = status.accounts.len(),
                bucket_count = status.buckets.len(),
                status = status.status.as_str(),
                last_success_at = status.last_success_at.unwrap_or_default(),
                "Refreshed cached public LLM gateway rate-limit status"
            );
        }
        Ok(())
    }
}

fn summarize_public_multi_account_status(
    summaries: &[AccountSummarySnapshot],
) -> (
    Vec<LlmGatewayPublicAccountStatusView>,
    Vec<LlmGatewayRateLimitBucketView>,
    String,
    Option<String>,
) {
    let accounts = summaries
        .iter()
        .map(summarize_public_account_status)
        .collect::<Vec<_>>();
    let mut all_buckets = Vec::new();
    let mut refresh_errors = Vec::new();
    let mut missing_snapshots = Vec::new();
    let mut active_account_count = 0usize;

    for summary in summaries {
        if summary.status != AccountStatus::Active {
            continue;
        }
        active_account_count += 1;
        if summary.rate_limits.buckets.is_empty() {
            missing_snapshots.push(summary.name.clone());
        }
        if let Some(error_message) = summary.usage_refresh.error_message.as_deref() {
            refresh_errors.push(format!(
                "{}: {}",
                summary.name,
                summarize_account_usage_refresh_error(error_message)
            ));
        }
        for mut bucket in summary.rate_limits.buckets.clone() {
            bucket.account_name = Some(summary.name.clone());
            all_buckets.push(bucket);
        }
    }

    let error_message = summarize_public_multi_account_error(
        summaries.len(),
        active_account_count,
        &refresh_errors,
        &missing_snapshots,
    );
    let status = if active_account_count == 0 {
        "error"
    } else if error_message.is_some() {
        "degraded"
    } else {
        "ready"
    };
    (accounts, all_buckets, status.to_string(), error_message)
}

fn summarize_public_account_status(
    summary: &AccountSummarySnapshot,
) -> LlmGatewayPublicAccountStatusView {
    LlmGatewayPublicAccountStatusView {
        name: summary.name.clone(),
        status: summary.status.as_str().to_string(),
        plan_type: summary.rate_limits.primary_plan_type(),
        primary_remaining_percent: summary.rate_limits.primary_remaining_percent(),
        secondary_remaining_percent: summary.rate_limits.secondary_remaining_percent(),
        last_usage_checked_at: summary.usage_refresh.last_checked_at,
        last_usage_success_at: summary.usage_refresh.last_success_at,
        usage_error_message: summary.usage_refresh.error_message.clone(),
    }
}

fn summarize_public_multi_account_error(
    total_account_count: usize,
    active_account_count: usize,
    refresh_errors: &[String],
    missing_snapshots: &[String],
) -> Option<String> {
    if active_account_count == 0 {
        return Some(format!(
            "no active codex accounts available out of {} configured account(s)",
            total_account_count
        ));
    }

    let mut issues = Vec::new();
    if !refresh_errors.is_empty() {
        issues.push(format!(
            "usage refresh degraded for {} active account(s): {}",
            refresh_errors.len(),
            refresh_errors.join(" | ")
        ));
    }
    if !missing_snapshots.is_empty() {
        issues.push(format!(
            "usage snapshots not ready yet for {} active account(s): {}",
            missing_snapshots.len(),
            missing_snapshots.join(", ")
        ));
    }
    if issues.is_empty() {
        None
    } else {
        Some(format!("codex account status degraded: {}", issues.join(" | ")))
    }
}

fn summarize_account_usage_refresh_error(error_message: &str) -> String {
    let compact = error_message
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let mut out = compact.trim().to_string();
    if out.len() > 240 {
        out.truncate(237);
        out.push_str("...");
    }
    out
}

// === Request-context middleware ===

/// Captures request diagnostics once before the proxy mutates headers or body.
pub async fn capture_gateway_event_context_middleware(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let method = request.method().as_str().to_string();
    let uri = request.uri().clone();
    let headers = request.headers().clone();
    let client_ip = request::extract_client_ip_from_headers(&headers);
    let ip_region = state.geoip.resolve_region(&client_ip).await;
    let request_url = request::resolve_request_url_from_headers(&headers, &uri);
    let request_headers_json = request::serialize_headers_json(&headers);

    tracing::debug!(method, request_url, client_ip, "Captured LLM gateway request context");

    request.extensions_mut().insert(LlmGatewayEventContext {
        request_method: method,
        request_url,
        client_ip,
        ip_region,
        request_headers_json,
        started_at: Instant::now(),
        routing_wait_ms: None,
        upstream_headers_ms: None,
        post_headers_body_ms: None,
        request_body_bytes: None,
        request_body_read_ms: None,
        request_json_parse_ms: None,
        pre_handler_ms: None,
        first_sse_write_ms: None,
        stream_finish_ms: None,
        routing_diagnostics_json: None,
        upstream_headers_at: None,
    });

    next.run(request).await
}

// === Public proxy handler ===

/// Main public OpenAI-compatible gateway handler.
pub async fn proxy_gateway_request(
    State(state): State<AppState>,
    request: Request,
) -> Result<Response, (StatusCode, Json<ErrorResponse>)> {
    let (parts, body) = request.into_parts();
    let mut event_context = parts.extensions.get::<LlmGatewayEventContext>().cloned();
    if let Some(context) = event_context.as_mut() {
        context.pre_handler_ms = Some(elapsed_ms_i32(context.started_at));
    }
    let path = parts.uri.path().to_string();
    let query = parts
        .uri
        .query()
        .map(|value| format!("?{value}"))
        .unwrap_or_default();
    let gateway_path = path
        .strip_prefix("/api/llm-gateway")
        .unwrap_or(path.as_str())
        .to_string();
    ensure_supported_gateway_path(&gateway_path)?;

    let presented_key = extract_presented_key(&parts.headers)
        .ok_or_else(|| auth_error(StatusCode::UNAUTHORIZED, "missing api key"))?;
    let key_hash = sha256_hex(presented_key.as_bytes());
    let key_lease = validate_gateway_key(&state, &key_hash).await?;

    tracing::debug!(
        key_id = %key_lease.record.id,
        gateway_path,
        "Validated LLM gateway key and forwarding request"
    );

    let is_models_path = request::is_models_path(&gateway_path);
    let request_method = parts.method.clone();
    let content_type = parts
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("application/json")
        .to_string();
    let max_request_body_bytes = usize::try_from(
        state
            .llm_gateway_runtime_config
            .read()
            .max_request_body_bytes,
    )
    .map_err(|err| internal_error("Invalid llm gateway max request body size", err))?;
    let raw_request_body = if is_models_path {
        Bytes::new()
    } else {
        let body_read_started = Instant::now();
        match read_gateway_request_body(body, max_request_body_bytes).await {
            Ok(bytes) => {
                if let Some(context) = event_context.as_mut() {
                    context.request_body_read_ms = Some(elapsed_ms_i32(body_read_started));
                    context.request_body_bytes = Some(bytes.len() as u64);
                }
                bytes
            },
            Err(err) => {
                if let Some(context) = event_context.as_mut() {
                    context.request_body_read_ms = Some(elapsed_ms_i32(body_read_started));
                }
                let prepared = build_failure_prepared_gateway_request(
                    &gateway_path,
                    &query,
                    request_method.clone(),
                    Bytes::new(),
                    &content_type,
                );
                if let Err(persist_err) = persist_gateway_failure_usage(
                    state.llm_gateway.as_ref(),
                    key_lease.as_ref(),
                    GatewayFailureUsageRequest {
                        prepared: &prepared,
                        status_code: err.0.as_u16() as i32,
                        usage: missing_usage_breakdown(),
                        event_context: event_context.as_ref(),
                        selected_account_name: None,
                        failure_stage: "read_request_body",
                        error: &err.1 .0.error,
                        details: Some(json!({ "error_code": err.1.0.code })),
                    },
                )
                .await
                {
                    tracing::warn!(
                        key_id = %key_lease.record.id,
                        "failed to persist codex failure usage after request body read error: {persist_err:#}"
                    );
                }
                return Err(err);
            },
        }
    };
    let failure_prepared = build_failure_prepared_gateway_request(
        &gateway_path,
        &query,
        request_method.clone(),
        raw_request_body.clone(),
        &content_type,
    );

    let record_route_selection = !is_models_path;
    let mut route_wait_ms = 0_u64;
    let mut codex_failover_count = 0_u64;
    let mut failed_account_names = HashSet::new();
    let route_started = Instant::now();
    let mut resolved_route = match resolve_auth_for_key(
        &state,
        &key_lease.record,
        record_route_selection,
    )
    .await
    {
        Ok(resolved) => {
            route_wait_ms = route_wait_ms.saturating_add(elapsed_ms_u64(route_started));
            update_codex_route_metrics(
                &mut event_context,
                route_wait_ms,
                resolved.selected_account_name.as_deref(),
                codex_failover_count,
                &failed_account_names,
            );
            resolved
        },
        Err(err) => {
            route_wait_ms = route_wait_ms.saturating_add(elapsed_ms_u64(route_started));
            update_codex_route_metrics(
                &mut event_context,
                route_wait_ms,
                None,
                codex_failover_count,
                &failed_account_names,
            );
            if !is_models_path {
                if let Err(persist_err) = persist_gateway_failure_usage(
                    state.llm_gateway.as_ref(),
                    key_lease.as_ref(),
                    GatewayFailureUsageRequest {
                        prepared: &failure_prepared,
                        status_code: err.0.as_u16() as i32,
                        usage: missing_usage_breakdown(),
                        event_context: event_context.as_ref(),
                        selected_account_name: None,
                        failure_stage: "resolve_auth_for_key",
                        error: &err.1 .0.error,
                        details: Some(json!({ "error_code": err.1.0.code })),
                    },
                )
                .await
                {
                    tracing::warn!(
                        key_id = %key_lease.record.id,
                        "failed to persist codex failure usage after auth resolution error: {persist_err:#}"
                    );
                }
            }
            return Err(err);
        },
    };
    if is_models_path {
        return respond_local_models(
            &state,
            &resolved_route.auth_snapshot,
            &parts.headers,
            &query,
            resolved_route.map_gpt53_codex_to_spark,
        )
        .await;
    }

    let request_limit_lease = match state
        .llm_gateway
        .request_scheduler
        .try_acquire(&key_lease.record)
    {
        Ok(lease) => lease,
        Err(rejection) => {
            let response = codex_key_request_limit_error(&key_lease.record, rejection.clone());
            if let Err(persist_err) = persist_gateway_failure_usage(
                state.llm_gateway.as_ref(),
                key_lease.as_ref(),
                GatewayFailureUsageRequest {
                    prepared: &failure_prepared,
                    status_code: response.0.as_u16() as i32,
                    usage: missing_usage_breakdown(),
                    event_context: event_context.as_ref(),
                    selected_account_name: resolved_route.selected_account_name.as_deref(),
                    failure_stage: "key_request_limit",
                    error: &response.1 .0.error,
                    details: Some(json!({
                        "reason": rejection.reason,
                        "in_flight": rejection.in_flight,
                        "max_concurrency": rejection.max_concurrency,
                        "min_start_interval_ms": rejection.min_start_interval_ms,
                        "wait_ms": rejection.wait.map(|value| value.as_millis() as u64),
                        "elapsed_since_last_start_ms": rejection.elapsed_since_last_start_ms,
                    })),
                },
            )
            .await
            {
                tracing::warn!(
                    key_id = %key_lease.record.id,
                    "failed to persist codex failure usage after key request limit error: {persist_err:#}"
                );
            }
            return Err(response);
        },
    };
    update_codex_route_metrics(
        &mut event_context,
        route_wait_ms,
        resolved_route.selected_account_name.as_deref(),
        codex_failover_count,
        &failed_account_names,
    );

    let request_json_parse_started = Instant::now();
    let prepared = match normalize_gateway_request_from_bytes(
        &gateway_path,
        &query,
        request_method.clone(),
        &parts.headers,
        raw_request_body,
        max_request_body_bytes,
    ) {
        Ok(prepared) => {
            if let Some(context) = event_context.as_mut() {
                context.request_json_parse_ms = Some(elapsed_ms_i32(request_json_parse_started));
            }
            prepared
        },
        Err(err) => {
            if let Some(context) = event_context.as_mut() {
                context.request_json_parse_ms = Some(elapsed_ms_i32(request_json_parse_started));
            }
            tracing::error!(
                key_id = %key_lease.record.id,
                key_name = %key_lease.record.name,
                gateway_path,
                error = %err.1.0.error,
                "rejected malformed codex public request before upstream call"
            );
            if let Err(persist_err) = persist_gateway_failure_usage(
                state.llm_gateway.as_ref(),
                key_lease.as_ref(),
                GatewayFailureUsageRequest {
                    prepared: &failure_prepared,
                    status_code: err.0.as_u16() as i32,
                    usage: missing_usage_breakdown(),
                    event_context: event_context.as_ref(),
                    selected_account_name: resolved_route.selected_account_name.as_deref(),
                    failure_stage: "normalize_request",
                    error: &err.1 .0.error,
                    details: Some(json!({ "error_code": err.1.0.code })),
                },
            )
            .await
            {
                tracing::warn!(
                    key_id = %key_lease.record.id,
                    "failed to persist codex failure usage after request normalization error: {persist_err:#}"
                );
            }
            return Err(err);
        },
    };
    let prepared = match apply_gpt53_codex_spark_mapping(
        &prepared,
        resolved_route.map_gpt53_codex_to_spark,
    ) {
        Ok(prepared) => prepared,
        Err(err) => {
            if let Err(persist_err) = persist_gateway_failure_usage(
                state.llm_gateway.as_ref(),
                key_lease.as_ref(),
                GatewayFailureUsageRequest {
                    prepared: &prepared,
                    status_code: err.0.as_u16() as i32,
                    usage: missing_usage_breakdown(),
                    event_context: event_context.as_ref(),
                    selected_account_name: resolved_route.selected_account_name.as_deref(),
                    failure_stage: "apply_model_mapping",
                    error: &err.1 .0.error,
                    details: Some(json!({ "error_code": err.1.0.code })),
                },
            )
            .await
            {
                tracing::warn!(
                    key_id = %key_lease.record.id,
                    "failed to persist codex failure usage after model mapping error: {persist_err:#}"
                );
            }
            return Err(err);
        },
    };
    let _activity_guard = state
        .llm_gateway
        .start_request_activity(&key_lease.record.id);

    loop {
        let attempted_account_name = resolved_route.selected_account_name.clone();
        let response = match send_upstream_with_retry(
            &state,
            &prepared,
            &parts.headers,
            &resolved_route.auth_snapshot,
            attempted_account_name.as_deref(),
        )
        .await
        {
            Ok(response) => response,
            Err(err) => {
                if let Err(persist_err) = persist_gateway_failure_usage(
                    state.llm_gateway.as_ref(),
                    key_lease.as_ref(),
                    GatewayFailureUsageRequest {
                        prepared: &prepared,
                        status_code: StatusCode::INTERNAL_SERVER_ERROR.as_u16() as i32,
                        usage: missing_usage_breakdown(),
                        event_context: event_context.as_ref(),
                        selected_account_name: attempted_account_name.as_deref(),
                        failure_stage: "send_upstream",
                        error: &err.to_string(),
                        details: Some(json!({ "error_chain": format!("{err:#}") })),
                    },
                )
                .await
                {
                    tracing::warn!(
                        key_id = %key_lease.record.id,
                        "failed to persist codex failure usage after upstream send error: {persist_err:#}"
                    );
                }
                let Some(failed_account_name) = attempted_account_name else {
                    return Err(internal_error("Failed to proxy llm gateway request", err));
                };
                failed_account_names.insert(failed_account_name.clone());
                tracing::warn!(
                    key_id = %key_lease.record.id,
                    key_name = %key_lease.record.name,
                    failed_account = %failed_account_name,
                    failed_accounts = ?failed_account_names,
                    error = %err,
                    "codex upstream send failed; trying another account if available"
                );
                let ResolvedCodexRoute {
                    account_request_limit_lease, ..
                } = resolved_route;
                drop(account_request_limit_lease);
                let route_started = Instant::now();
                resolved_route = match resolve_auth_for_key_excluding(
                    &state,
                    &key_lease.record,
                    record_route_selection,
                    &failed_account_names,
                )
                .await
                {
                    Ok(next_route) => {
                        route_wait_ms = route_wait_ms.saturating_add(elapsed_ms_u64(route_started));
                        codex_failover_count = codex_failover_count.saturating_add(1);
                        update_codex_route_metrics(
                            &mut event_context,
                            route_wait_ms,
                            next_route.selected_account_name.as_deref(),
                            codex_failover_count,
                            &failed_account_names,
                        );
                        next_route
                    },
                    Err(_) => {
                        return Err(internal_error("Failed to proxy llm gateway request", err));
                    },
                };
                continue;
            },
        };
        if let Some(context) = event_context.as_mut() {
            context.upstream_headers_ms = Some(clamp_u64_ms_to_i32(response.upstream_headers_ms));
            context.upstream_headers_at = Some(response.upstream_headers_at);
        }
        let response = response.response;

        if let Some(failed_account_name) =
            retryable_codex_account_failure(response.status(), attempted_account_name.as_deref())
        {
            let failed_account_name = failed_account_name.to_string();
            let retry_response = match capture_upstream_non_success_response(
                &state,
                key_lease.as_ref(),
                &prepared,
                response,
                event_context.as_ref(),
                Some(&failed_account_name),
            )
            .await
            {
                Ok(response) => response,
                Err(err) => {
                    failed_account_names.insert(failed_account_name.clone());
                    tracing::warn!(
                        key_id = %key_lease.record.id,
                        key_name = %key_lease.record.name,
                        failed_account = %failed_account_name,
                        failed_accounts = ?failed_account_names,
                        "failed to capture codex non-success response; trying another account if available"
                    );
                    let ResolvedCodexRoute {
                        account_request_limit_lease, ..
                    } = resolved_route;
                    drop(account_request_limit_lease);
                    let route_started = Instant::now();
                    resolved_route = match resolve_auth_for_key_excluding(
                        &state,
                        &key_lease.record,
                        record_route_selection,
                        &failed_account_names,
                    )
                    .await
                    {
                        Ok(next_route) => {
                            route_wait_ms =
                                route_wait_ms.saturating_add(elapsed_ms_u64(route_started));
                            codex_failover_count = codex_failover_count.saturating_add(1);
                            update_codex_route_metrics(
                                &mut event_context,
                                route_wait_ms,
                                next_route.selected_account_name.as_deref(),
                                codex_failover_count,
                                &failed_account_names,
                            );
                            next_route
                        },
                        Err(_) => return Err(err),
                    };
                    continue;
                },
            };
            failed_account_names.insert(failed_account_name.clone());
            tracing::warn!(
                key_id = %key_lease.record.id,
                key_name = %key_lease.record.name,
                failed_account = %failed_account_name,
                failed_accounts = ?failed_account_names,
                "codex account returned non-success response; trying another account if available"
            );
            let ResolvedCodexRoute {
                account_request_limit_lease, ..
            } = resolved_route;
            drop(account_request_limit_lease);
            let route_started = Instant::now();
            resolved_route = match resolve_auth_for_key_excluding(
                &state,
                &key_lease.record,
                record_route_selection,
                &failed_account_names,
            )
            .await
            {
                Ok(next_route) => {
                    route_wait_ms = route_wait_ms.saturating_add(elapsed_ms_u64(route_started));
                    codex_failover_count = codex_failover_count.saturating_add(1);
                    update_codex_route_metrics(
                        &mut event_context,
                        route_wait_ms,
                        next_route.selected_account_name.as_deref(),
                        codex_failover_count,
                        &failed_account_names,
                    );
                    next_route
                },
                Err(_) => return Ok(retry_response),
            };
            continue;
        }

        let ResolvedCodexRoute {
            account_request_limit_lease,
            selected_account_name,
            ..
        } = resolved_route;
        update_codex_route_metrics(
            &mut event_context,
            route_wait_ms,
            selected_account_name.as_deref(),
            codex_failover_count,
            &failed_account_names,
        );
        return forward_upstream_response(ForwardUpstreamResponseArgs {
            state,
            key_lease,
            account_request_limit_lease,
            request_limit_lease,
            prepared,
            upstream: response,
            event_context,
            selected_account_name,
        })
        .await;
    }
}

/// Validate the presented key via cache first, then fall back to LanceDB.
async fn validate_gateway_key(
    state: &AppState,
    key_hash: &str,
) -> Result<Arc<CachedKeyLease>, (StatusCode, Json<ErrorResponse>)> {
    if let Some(cached) = state.llm_gateway.key_cache.get(key_hash) {
        tracing::debug!(key_hash, "LLM gateway key cache hit");
        let effective = state.llm_gateway.overlay_key_usage(&cached.record).await;
        validate_cached_key(&effective)?;
        let ttl = current_cache_ttl(state).await;
        return Ok(state
            .llm_gateway
            .key_cache
            .renew(effective, Duration::from_secs(ttl)));
    }

    tracing::debug!(key_hash, "LLM gateway key cache miss");
    let key = state
        .llm_gateway_store
        .get_key_by_hash(key_hash)
        .await
        .map_err(|err| internal_error("Failed to validate llm gateway key", err))?
        .ok_or_else(|| auth_error(StatusCode::FORBIDDEN, "invalid api key"))?;
    let effective_key = state.llm_gateway.overlay_key_usage(&key).await;
    validate_cached_key(&effective_key)?;
    let ttl = current_cache_ttl(state).await;
    Ok(state
        .llm_gateway
        .key_cache
        .renew(effective_key, Duration::from_secs(ttl)))
}

/// Enforce key status and quota invariants before any upstream request starts.
fn validate_cached_key(key: &LlmGatewayKeyRecord) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    if key.status != LLM_GATEWAY_KEY_STATUS_ACTIVE {
        return Err(auth_error(StatusCode::FORBIDDEN, "api key is disabled"));
    }
    if key.remaining_billable() <= 0 {
        return Err(auth_error(StatusCode::TOO_MANY_REQUESTS, "quota_exceeded"));
    }
    Ok(())
}

/// Select the upstream auth snapshot based on the key's routing strategy.
///
/// Group-based routing is the new source of truth. Legacy per-key account
/// fields remain as a compatibility input only until startup migration has
/// rewritten older rows.
struct ResolvedCodexRoute {
    auth_snapshot: CodexAuthSnapshot,
    selected_account_name: Option<String>,
    map_gpt53_codex_to_spark: bool,
    account_request_limit_lease: CodexAccountRequestLease,
}

async fn resolve_auth_for_key(
    state: &AppState,
    key: &LlmGatewayKeyRecord,
    record_route_selection: bool,
) -> Result<ResolvedCodexRoute, (StatusCode, Json<ErrorResponse>)> {
    resolve_auth_for_key_excluding(state, key, record_route_selection, &HashSet::new()).await
}

async fn resolve_auth_for_key_excluding(
    state: &AppState,
    key: &LlmGatewayKeyRecord,
    record_route_selection: bool,
    excluded_account_names: &HashSet<String>,
) -> Result<ResolvedCodexRoute, (StatusCode, Json<ErrorResponse>)> {
    let pool = &state.llm_gateway.account_pool;
    let strategy = key.route_strategy.as_deref().unwrap_or("auto");
    let queued_at = Instant::now();

    match strategy {
        "fixed" => {
            let resolved_name = if let Some(group_id) = key.account_group_id.as_deref() {
                let group =
                    load_account_group_for_provider(state, LLM_GATEWAY_PROVIDER_CODEX, group_id)
                        .await?;
                if group.account_names.len() != 1 {
                    return Err(bad_request(
                        "fixed route_strategy requires an account group with exactly one account",
                    ));
                }
                group.account_names[0].clone()
            } else {
                let name = key.fixed_account_name.as_deref().unwrap_or("");
                if name.is_empty() {
                    return Err(bad_request("fixed route_strategy requires account_group_id"));
                }
                name.to_string()
            };

            if excluded_account_names.contains(&resolved_name) {
                return Err(auth_error(
                    StatusCode::BAD_GATEWAY,
                    &format!("all configured codex accounts failed: {resolved_name}"),
                ));
            }

            loop {
                let candidate = pool
                    .account_candidate_by_name(&resolved_name)
                    .await
                    .ok_or_else(|| {
                        auth_error(
                            StatusCode::SERVICE_UNAVAILABLE,
                            &format!("bound account `{resolved_name}` is unavailable"),
                        )
                    })?;
                match state.llm_gateway.account_request_scheduler.try_acquire(
                    &candidate.name,
                    candidate.request_max_concurrency,
                    candidate.request_min_start_interval_ms,
                    queued_at,
                ) {
                    Ok(account_request_limit_lease) => {
                        if record_route_selection {
                            pool.record_route_selection(&resolved_name).await;
                        }
                        tracing::info!(
                            key_id = %key.id,
                            key_name = %key.name,
                            strategy = "fixed",
                            account = %resolved_name,
                            queue_wait_ms = account_request_limit_lease.waited_ms(),
                            request_max_concurrency = ?candidate.request_max_concurrency,
                            request_min_start_interval_ms = ?candidate.request_min_start_interval_ms,
                            "resolved codex upstream account for key"
                        );
                        return Ok(ResolvedCodexRoute {
                            auth_snapshot: candidate.snapshot,
                            selected_account_name: Some(resolved_name.clone()),
                            map_gpt53_codex_to_spark: candidate.map_gpt53_codex_to_spark,
                            account_request_limit_lease,
                        });
                    },
                    Err(rejection) => {
                        tracing::warn!(
                            key_id = %key.id,
                            key_name = %key.name,
                            strategy = "fixed",
                            account = %resolved_name,
                            reason = rejection.reason,
                            in_flight = rejection.in_flight,
                            request_max_concurrency = ?rejection.max_concurrency,
                            request_min_start_interval_ms = ?rejection.min_start_interval_ms,
                            wait_ms = rejection.wait.map(|value| value.as_millis() as u64),
                            elapsed_since_last_start_ms = rejection.elapsed_since_last_start_ms,
                            "waiting for fixed codex account to clear local scheduler limits"
                        );
                        state
                            .llm_gateway
                            .account_request_scheduler
                            .wait_for_available(rejection.wait)
                            .await;
                    },
                }
            }
        },
        "auto" => {
            let subset_resolution = if let Some(group_id) = key.account_group_id.as_deref() {
                let group =
                    load_account_group_for_provider(state, LLM_GATEWAY_PROVIDER_CODEX, group_id)
                        .await?;
                let configured_account_names = group.account_names.clone();
                let (existing_account_names, missing_account_names) =
                    partition_existing_account_names(pool, &configured_account_names).await;
                if !missing_account_names.is_empty() {
                    tracing::warn!(
                        key_id = %key.id,
                        key_name = %key.name,
                        account_group_id = %group.id,
                        missing_group_account_names = ?missing_account_names,
                        effective_group_account_names = ?existing_account_names,
                        "ignoring unknown codex group account names during request routing"
                    );
                }
                Some((existing_account_names, missing_account_names, configured_account_names))
            } else if let Some(names) = key.auto_account_names.as_ref() {
                let configured_account_names = names.clone();
                let (existing_account_names, missing_account_names) =
                    partition_existing_account_names(pool, &configured_account_names).await;
                if !missing_account_names.is_empty() {
                    tracing::warn!(
                        key_id = %key.id,
                        key_name = %key.name,
                        missing_auto_account_names = ?missing_account_names,
                        effective_auto_account_names = ?existing_account_names,
                        "ignoring unknown codex auto account names during request routing"
                    );
                }
                Some((existing_account_names, missing_account_names, configured_account_names))
            } else {
                None
            };

            if let Some((
                existing_account_names,
                configured_missing_names,
                configured_account_names,
            )) = subset_resolution.as_ref()
            {
                if existing_account_names.is_empty() {
                    tracing::warn!(
                        key_id = %key.id,
                        key_name = %key.name,
                        account_group_id = ?key.account_group_id,
                        auto_account_names = ?configured_account_names,
                        missing_auto_account_names = ?configured_missing_names,
                        "configured codex auto account subset has no existing accounts"
                    );
                    return Err(auth_error(
                        StatusCode::SERVICE_UNAVAILABLE,
                        &format!(
                            "configured account group has no existing accounts: {}",
                            configured_account_names.join(", ")
                        ),
                    ));
                }
            }

            let auto_account_filter = subset_resolution.as_ref().and_then(
                |(existing_account_names, _configured_missing_names, _configured_account_names)| {
                    (!existing_account_names.is_empty()).then(|| {
                        existing_account_names
                            .iter()
                            .cloned()
                            .collect::<HashSet<_>>()
                    })
                },
            );

            loop {
                let candidates_before_exclusion = if let Some(filter) = auto_account_filter.as_ref()
                {
                    pool.ranked_routable_accounts(Some(filter)).await
                } else {
                    pool.ranked_routable_accounts(None).await
                };
                let had_routable_accounts_before_exclusion =
                    !candidates_before_exclusion.is_empty();
                let candidates = candidates_before_exclusion
                    .into_iter()
                    .filter(|candidate| !excluded_account_names.contains(&candidate.name))
                    .collect::<Vec<_>>();

                if candidates.is_empty() {
                    if !excluded_account_names.is_empty() {
                        tracing::warn!(
                            key_id = %key.id,
                            key_name = %key.name,
                            had_routable_accounts_before_exclusion,
                            excluded_account_names = ?excluded_account_names,
                            "all eligible codex accounts have already failed this request"
                        );
                        return Err(auth_error(
                            StatusCode::BAD_GATEWAY,
                            "all eligible codex accounts failed for this request",
                        ));
                    }
                    if auto_account_filter.is_some() {
                        let configured_subset = subset_resolution
                            .as_ref()
                            .map(
                                |(
                                    _existing_account_names,
                                    _missing_names,
                                    configured_account_names,
                                )| {
                                    configured_account_names.clone()
                                },
                            )
                            .unwrap_or_default();
                        tracing::warn!(
                            key_id = %key.id,
                            key_name = %key.name,
                            account_group_id = ?key.account_group_id,
                            auto_account_names = ?configured_subset,
                            "configured codex account group had no usable accounts"
                        );
                        return Err(auth_error(
                            StatusCode::SERVICE_UNAVAILABLE,
                            &format!(
                                "configured account group has no usable accounts: {}",
                                configured_subset.join(", ")
                            ),
                        ));
                    }
                    return state
                        .llm_gateway
                        .auth_source
                        .current()
                        .await
                        .map(|snapshot| ResolvedCodexRoute {
                            auth_snapshot: snapshot,
                            selected_account_name: None,
                            map_gpt53_codex_to_spark: false,
                            account_request_limit_lease: CodexAccountRequestLease::untracked(
                                "legacy",
                            ),
                        })
                        .map_err(|err| {
                            internal_error("no accounts available and legacy auth failed", err)
                        });
                }

                let strategy_label = if key.account_group_id.is_some() {
                    "auto_group"
                } else if auto_account_filter.is_some() {
                    "auto_subset"
                } else {
                    "auto_global"
                };
                let mut saw_local_limit = false;
                let mut shortest_wait: Option<Duration> = None;
                let mut blocked_accounts = Vec::new();

                for candidate in candidates {
                    match state.llm_gateway.account_request_scheduler.try_acquire(
                        &candidate.name,
                        candidate.request_max_concurrency,
                        candidate.request_min_start_interval_ms,
                        queued_at,
                    ) {
                        Ok(account_request_limit_lease) => {
                            if record_route_selection {
                                pool.record_route_selection(&candidate.name).await;
                            }
                            tracing::info!(
                                key_id = %key.id,
                                key_name = %key.name,
                                strategy = strategy_label,
                                selected_account = %candidate.name,
                                queue_wait_ms = account_request_limit_lease.waited_ms(),
                                request_max_concurrency = ?candidate.request_max_concurrency,
                                request_min_start_interval_ms = ?candidate.request_min_start_interval_ms,
                                account_group_id = ?key.account_group_id,
                                auto_account_names = ?key.auto_account_names,
                                "resolved codex upstream account for key"
                            );
                            return Ok(ResolvedCodexRoute {
                                auth_snapshot: candidate.snapshot,
                                selected_account_name: Some(candidate.name),
                                map_gpt53_codex_to_spark: candidate.map_gpt53_codex_to_spark,
                                account_request_limit_lease,
                            });
                        },
                        Err(rejection) => {
                            saw_local_limit = true;
                            if let Some(wait) = rejection.wait {
                                shortest_wait = Some(match shortest_wait {
                                    Some(current) => current.min(wait),
                                    None => wait,
                                });
                            }
                            blocked_accounts.push(format!(
                                "{}: {} in_flight={} request_max_concurrency={} \
                                 request_min_start_interval_ms={} wait_ms={} \
                                 elapsed_since_last_start_ms={}",
                                candidate.name,
                                rejection.reason,
                                rejection.in_flight,
                                rejection
                                    .max_concurrency
                                    .map(|value| value.to_string())
                                    .unwrap_or_else(|| "unlimited".to_string()),
                                rejection
                                    .min_start_interval_ms
                                    .map(|value| value.to_string())
                                    .unwrap_or_else(|| "unlimited".to_string()),
                                rejection
                                    .wait
                                    .map(|value| value.as_millis() as u64)
                                    .unwrap_or(0),
                                rejection.elapsed_since_last_start_ms.unwrap_or(0)
                            ));
                        },
                    }
                }

                if saw_local_limit {
                    tracing::warn!(
                        key_id = %key.id,
                        key_name = %key.name,
                        strategy = strategy_label,
                        wait_ms = shortest_wait.map(|value| value.as_millis() as u64).unwrap_or(0),
                        blocked_accounts = ?blocked_accounts,
                        "all eligible codex accounts are locally throttled; waiting before retrying"
                    );
                    state
                        .llm_gateway
                        .account_request_scheduler
                        .wait_for_available(shortest_wait)
                        .await;
                    continue;
                }
            }
        },
        _ => Err(bad_request("route_strategy must be `auto` or `fixed`")),
    }
}

/// Read the live auth-cache TTL from the runtime config lock.
async fn current_cache_ttl(state: &AppState) -> u64 {
    state
        .llm_gateway_runtime_config
        .read()
        .auth_cache_ttl_seconds
}

// === Upstream transport ===

struct CodexUpstreamResponse {
    response: reqwest::Response,
    upstream_headers_ms: u64,
    upstream_headers_at: Instant,
}

/// Retry once with a forced auth reload if the upstream rejects stale
/// credentials.
async fn send_upstream_with_retry(
    state: &AppState,
    prepared: &PreparedGatewayRequest,
    incoming_headers: &HeaderMap,
    auth_snapshot: &CodexAuthSnapshot,
    selected_account_name: Option<&str>,
) -> Result<CodexUpstreamResponse> {
    let first =
        send_upstream(state, prepared, incoming_headers, auth_snapshot, selected_account_name)
            .await?;
    if first.response.status() != StatusCode::UNAUTHORIZED {
        return Ok(first);
    }

    tracing::warn!(
        upstream_path = prepared.upstream_path,
        "Upstream returned 401, forcing Codex auth reload"
    );

    let refreshed = reload_codex_auth_snapshot(state, selected_account_name).await?;
    send_upstream(state, prepared, incoming_headers, &refreshed, selected_account_name).await
}

fn retryable_codex_account_failure(
    status: StatusCode,
    selected_account_name: Option<&str>,
) -> Option<&str> {
    selected_account_name.filter(|_| !status.is_success())
}

/// Build the exact upstream HTTP request to the Codex backend.
async fn send_upstream(
    state: &AppState,
    prepared: &PreparedGatewayRequest,
    incoming_headers: &HeaderMap,
    auth_snapshot: &CodexAuthSnapshot,
    selected_account_name: Option<&str>,
) -> Result<CodexUpstreamResponse> {
    // Upstream headers are rebuilt from scratch instead of forwarding the
    // inbound request wholesale. This keeps reverse-proxy routing headers such
    // as `host`, `x-forwarded-for`, `x-forwarded-host`, `x-forwarded-proto`,
    // and `x-real-ip` inside StaticFlow for diagnostics only, while the Codex
    // backend receives just the protocol-level headers it actually needs.
    let mut headers = ReqwestHeaderMap::new();
    let incoming_user_agent =
        request::extract_header_value(incoming_headers, header::USER_AGENT.as_str());
    let incoming_originator = request::extract_header_value(incoming_headers, "originator");
    let incoming_openai_beta = request::extract_header_value(incoming_headers, "openai-beta");
    let default_codex_client_version = resolve_codex_client_version(Some(
        &state.llm_gateway_runtime_config.read().codex_client_version,
    ));
    let effective_user_agent =
        incoming_user_agent.unwrap_or_else(|| codex_user_agent(&default_codex_client_version));
    headers.insert(
        reqwest::header::ACCEPT,
        ReqwestHeaderValue::from_static(
            if prepared.wants_stream || prepared.force_upstream_stream {
                "text/event-stream"
            } else {
                "application/json"
            },
        ),
    );
    headers
        .insert(reqwest::header::USER_AGENT, ReqwestHeaderValue::from_str(&effective_user_agent)?);
    if !prepared.request_body.is_empty() {
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            ReqwestHeaderValue::from_str(&prepared.content_type)
                .unwrap_or_else(|_| ReqwestHeaderValue::from_static("application/json")),
        );
    }

    let upstream_base = env::var("STATICFLOW_LLM_GATEWAY_UPSTREAM_BASE_URL")
        .ok()
        .map(|value| normalize_upstream_base_url(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_UPSTREAM_BASE_URL.to_string());
    let upstream_url = compute_upstream_url(&upstream_base, &prepared.upstream_path);
    headers.insert(reqwest::header::AUTHORIZATION, bearer_header(&auth_snapshot.access_token)?);
    headers.insert(
        reqwest::header::HeaderName::from_static("originator"),
        ReqwestHeaderValue::from_str(
            incoming_originator
                .as_deref()
                .unwrap_or(DEFAULT_WIRE_ORIGINATOR),
        )?,
    );
    if let Some(openai_beta) = incoming_openai_beta.as_deref() {
        headers.insert(
            reqwest::header::HeaderName::from_static("openai-beta"),
            ReqwestHeaderValue::from_str(openai_beta)?,
        );
    }
    let incoming_session_id = request::extract_header_value(incoming_headers, "session_id");
    let incoming_client_request_id =
        request::extract_header_value(incoming_headers, "x-client-request-id");
    let incoming_subagent = request::extract_header_value(incoming_headers, "x-openai-subagent");
    let incoming_beta_features =
        request::extract_header_value(incoming_headers, "x-codex-beta-features");
    let incoming_turn_metadata =
        request::extract_header_value(incoming_headers, "x-codex-turn-metadata");
    let mut incoming_turn_state =
        request::extract_header_value(incoming_headers, "x-codex-turn-state");
    let thread_anchor = prepared.thread_anchor.as_deref();
    let is_compact_request = prepared.original_path.starts_with("/v1/responses/compact");
    let effective_client_request_id = if !is_compact_request {
        thread_anchor.or(incoming_client_request_id.as_deref())
    } else {
        incoming_client_request_id.as_deref()
    };
    if let (Some(anchor), Some(legacy_session_id)) = (thread_anchor, incoming_session_id.as_deref())
    {
        if legacy_session_id.trim() != anchor {
            incoming_turn_state = None;
        }
    } else if incoming_session_id.is_none() && thread_anchor.is_none() {
        incoming_turn_state = None;
    }
    let effective_session_id = thread_anchor.or(incoming_session_id.as_deref());
    if let Some(client_request_id) = effective_client_request_id {
        headers.insert(
            reqwest::header::HeaderName::from_static("x-client-request-id"),
            ReqwestHeaderValue::from_str(client_request_id)?,
        );
    }
    if let Some(subagent) = incoming_subagent.as_deref() {
        headers.insert(
            reqwest::header::HeaderName::from_static("x-openai-subagent"),
            ReqwestHeaderValue::from_str(subagent)?,
        );
    }
    if let Some(beta_features) = incoming_beta_features.as_deref() {
        headers.insert(
            reqwest::header::HeaderName::from_static("x-codex-beta-features"),
            ReqwestHeaderValue::from_str(beta_features)?,
        );
    }
    if let Some(turn_metadata) = incoming_turn_metadata.as_deref() {
        headers.insert(
            reqwest::header::HeaderName::from_static("x-codex-turn-metadata"),
            ReqwestHeaderValue::from_str(turn_metadata)?,
        );
    }
    if let Some(turn_state) = incoming_turn_state.as_deref() {
        headers.insert(
            reqwest::header::HeaderName::from_static("x-codex-turn-state"),
            ReqwestHeaderValue::from_str(turn_state)?,
        );
    }
    for header_name in [
        "x-codex-installation-id",
        "x-codex-parent-thread-id",
        "x-codex-window-id",
        "x-openai-memgen-request",
        "x-responsesapi-include-timing-metrics",
        "traceparent",
        "tracestate",
        "baggage",
    ] {
        if let Some(value) = request::extract_header_value(incoming_headers, header_name) {
            headers.insert(
                reqwest::header::HeaderName::from_static(header_name),
                ReqwestHeaderValue::from_str(&value)?,
            );
        }
    }
    if let Some(account_id) = auth_snapshot.account_id.as_deref() {
        headers.insert(
            reqwest::header::HeaderName::from_static("chatgpt-account-id"),
            ReqwestHeaderValue::from_str(account_id)?,
        );
    }
    if auth_snapshot.is_fedramp_account {
        headers.insert(
            reqwest::header::HeaderName::from_static("x-openai-fedramp"),
            ReqwestHeaderValue::from_static("true"),
        );
    }
    if let Some(session_id) = effective_session_id {
        headers.insert(
            reqwest::header::HeaderName::from_static("session_id"),
            ReqwestHeaderValue::from_str(session_id)?,
        );
    }

    tracing::debug!(
        upstream_url,
        method = %prepared.method,
        wants_stream = prepared.wants_stream,
        force_upstream_stream = prepared.force_upstream_stream,
        model = prepared.model.as_deref().unwrap_or("unknown"),
        "Sending LLM gateway request upstream"
    );

    let (client, resolved_proxy) = state
        .llm_gateway
        .build_upstream_client(auth_snapshot)
        .await
        .context("failed to build codex upstream client")?;
    let mut request_builder = client
        .request(prepared.method.clone(), upstream_url)
        .headers(headers);
    if !prepared.request_body.is_empty() {
        request_builder = request_builder.body(prepared.request_body.clone());
    }

    let upstream_started = Instant::now();
    match request_builder.send().await {
        Ok(response) => Ok(CodexUpstreamResponse {
            response,
            upstream_headers_ms: elapsed_ms_u64(upstream_started),
            upstream_headers_at: Instant::now(),
        }),
        Err(err) => {
            let invalidated = state
                .upstream_proxy_registry
                .invalidate_client_if_connect_error(
                    &resolved_proxy,
                    codex_upstream_client_profile(),
                    &err,
                )
                .await;
            tracing::warn!(
                account_name = selected_account_name.unwrap_or("legacy"),
                proxy_source = %resolved_proxy.source.as_str(),
                proxy_url = %resolved_proxy.proxy_url_label(),
                invalidated_client = invalidated,
                "codex upstream request failed: {err}"
            );
            Err(err).context("upstream request failed")
        },
    }
}

async fn reload_codex_auth_snapshot(
    state: &AppState,
    selected_account_name: Option<&str>,
) -> Result<CodexAuthSnapshot> {
    if let Some(account_name) = selected_account_name {
        return token_refresh::refresh_account_access_token_once(
            state.llm_gateway.account_pool.as_ref(),
            state.upstream_proxy_registry.as_ref(),
            account_name,
        )
        .await
        .with_context(|| {
            format!("failed to refresh selected Codex account `{account_name}` after upstream 401")
        });
    }
    state.llm_gateway.auth_source.force_reload().await
}

// === Downstream response adaptation ===

struct ForwardUpstreamResponseArgs {
    state: AppState,
    key_lease: Arc<CachedKeyLease>,
    account_request_limit_lease: CodexAccountRequestLease,
    request_limit_lease: CodexKeyRequestLease,
    prepared: PreparedGatewayRequest,
    upstream: reqwest::Response,
    event_context: Option<LlmGatewayEventContext>,
    selected_account_name: Option<String>,
}

async fn capture_upstream_non_success_response(
    state: &AppState,
    key_lease: &CachedKeyLease,
    prepared: &PreparedGatewayRequest,
    upstream: reqwest::Response,
    event_context: Option<&LlmGatewayEventContext>,
    selected_account_name: Option<&str>,
) -> Result<Response, (StatusCode, Json<ErrorResponse>)> {
    let status = upstream.status();
    let upstream_headers = upstream.headers().clone();
    let content_type = upstream
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/json")
        .to_string();
    let body_bytes = match upstream.bytes().await {
        Ok(body_bytes) => body_bytes,
        Err(err) => {
            if let Err(persist_err) = persist_gateway_failure_usage(
                state.llm_gateway.as_ref(),
                key_lease,
                GatewayFailureUsageRequest {
                    prepared,
                    status_code: StatusCode::INTERNAL_SERVER_ERROR.as_u16() as i32,
                    usage: missing_usage_breakdown(),
                    event_context,
                    selected_account_name,
                    failure_stage: "read_upstream_response",
                    error: "Failed to read llm gateway upstream response",
                    details: Some(json!({
                        "upstream_status": status.as_u16(),
                        "content_type": content_type,
                        "error": err.to_string(),
                    })),
                },
            )
            .await
            {
                tracing::warn!(
                    key_id = %key_lease.record.id,
                    "failed to persist codex failure usage after upstream body read error: {persist_err:#}"
                );
            }
            return Err(internal_error("Failed to read llm gateway upstream response", err));
        },
    };

    let body_text = String::from_utf8_lossy(&body_bytes);
    tracing::error!(
        key_id = %key_lease.record.id,
        key_name = %key_lease.record.name,
        account_name = selected_account_name.unwrap_or("legacy"),
        original_path = %prepared.original_path,
        upstream_path = %prepared.upstream_path,
        status = status.as_u16(),
        content_type = %content_type,
        model = prepared.model.as_deref().unwrap_or("unknown"),
        body_len = body_bytes.len(),
        body_preview = %summarize_upstream_error_body(&body_text),
        "codex public request returned non-success upstream response"
    );
    if let Err(persist_err) = persist_gateway_failure_usage(
        state.llm_gateway.as_ref(),
        key_lease,
        GatewayFailureUsageRequest {
            prepared,
            status_code: status.as_u16() as i32,
            usage: missing_usage_breakdown(),
            event_context,
            selected_account_name,
            failure_stage: "upstream_non_success",
            error: &format!("upstream returned non-success status {}", status.as_u16()),
            details: Some(json!({
                "content_type": content_type.clone(),
                "upstream_body": body_text.to_string(),
            })),
        },
    )
    .await
    {
        tracing::warn!(
            key_id = %key_lease.record.id,
            "failed to persist codex failure usage after non-success upstream response: {persist_err:#}"
        );
    }

    let response_bytes = rewrite_json_response_model_alias(
        &body_bytes,
        prepared.model.as_deref(),
        prepared.client_visible_model.as_deref(),
    )
    .unwrap_or_else(|| body_bytes.to_vec());
    let builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, &content_type)
        .header(header::CACHE_CONTROL, "no-store");
    apply_upstream_response_headers(builder, &upstream_headers)
        .body(Body::from(response_bytes))
        .map_err(|err| internal_error("Failed to build llm gateway response", err))
}

/// Adapt the upstream response back into the caller's requested wire format.
async fn forward_upstream_response(
    args: ForwardUpstreamResponseArgs,
) -> Result<Response, (StatusCode, Json<ErrorResponse>)> {
    let ForwardUpstreamResponseArgs {
        state,
        key_lease,
        account_request_limit_lease,
        request_limit_lease,
        prepared,
        upstream,
        mut event_context,
        selected_account_name,
    } = args;
    let status = upstream.status();
    let response_adapter = prepared.response_adapter;
    let upstream_headers = upstream.headers().clone();
    let content_type = upstream
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/json")
        .to_string();

    let expects_sse = status.is_success()
        && (content_type.contains("text/event-stream")
            || prepared.wants_stream
            || prepared.force_upstream_stream);

    tracing::debug!(
        upstream_path = prepared.upstream_path,
        status = status.as_u16(),
        content_type,
        expects_sse,
        "Forwarding LLM gateway upstream response"
    );

    if expects_sse {
        if prepared.force_upstream_stream && !prepared.wants_stream {
            let mut collector = SseUsageCollector::default();
            let mut events = upstream
                .bytes_stream()
                .map_err(std::io::Error::other)
                .eventsource();
            let mut shutdown_rx = state.shutdown_rx.clone();
            loop {
                let maybe_event = tokio::select! {
                    biased;
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            tracing::info!(
                                upstream_path = prepared.upstream_path,
                                "stopping aggregated llm gateway SSE drain because backend is shutting down"
                            );
                            None
                        } else {
                            continue;
                        }
                    }
                    event = events.next() => event,
                };
                let Some(event) = maybe_event else {
                    break;
                };
                match event {
                    Ok(event) => collector.observe_event(&event),
                    Err(err) => {
                        if let Err(persist_err) = persist_gateway_failure_usage(
                            state.llm_gateway.as_ref(),
                            key_lease.as_ref(),
                            GatewayFailureUsageRequest {
                                prepared: &prepared,
                                status_code: StatusCode::INTERNAL_SERVER_ERROR.as_u16() as i32,
                                usage: collector.usage.unwrap_or_else(missing_usage_breakdown),
                                event_context: event_context.as_ref(),
                                selected_account_name: selected_account_name.as_deref(),
                                failure_stage: "stream_read",
                                error: &format!(
                                    "Failed to parse llm gateway upstream SSE stream: {err}"
                                ),
                                details: Some(json!({ "stream_kind": "aggregated_sse" })),
                            },
                        )
                        .await
                        {
                            tracing::warn!(
                                key_id = %key_lease.record.id,
                                "failed to persist codex failure usage after aggregated SSE parse error: {persist_err:#}"
                            );
                        }
                        return Err(internal_error(
                            "Failed to parse llm gateway upstream SSE stream",
                            err,
                        ));
                    },
                }
            }
            let usage = collector.usage.unwrap_or_else(missing_usage_breakdown);
            let response_json = match collector.completed_response {
                Some(response_json) => response_json,
                None => {
                    if let Err(persist_err) = persist_gateway_failure_usage(
                        state.llm_gateway.as_ref(),
                        key_lease.as_ref(),
                        GatewayFailureUsageRequest {
                            prepared: &prepared,
                            status_code: StatusCode::INTERNAL_SERVER_ERROR.as_u16() as i32,
                            usage,
                            event_context: event_context.as_ref(),
                            selected_account_name: selected_account_name.as_deref(),
                            failure_stage: "aggregate_response",
                            error: "response.completed event missing",
                            details: Some(json!({ "stream_kind": "aggregated_sse" })),
                        },
                    )
                    .await
                    {
                        tracing::warn!(
                            key_id = %key_lease.record.id,
                            "failed to persist codex failure usage after aggregated SSE completion error: {persist_err:#}"
                        );
                    }
                    return Err(internal_error(
                        "Failed to aggregate llm gateway response",
                        "response.completed event missing",
                    ));
                },
            };
            let response_json = if let (Some(model_from), Some(model_to)) =
                (prepared.model.as_deref(), prepared.client_visible_model.as_deref())
            {
                let aliased = response_json.clone();
                let aliased_bytes = rewrite_json_response_model_alias(
                    &serde_json::to_vec(&response_json).unwrap_or_default(),
                    Some(model_from),
                    Some(model_to),
                );
                aliased_bytes
                    .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                    .unwrap_or_else(|| {
                        if model_from != model_to {
                            tracing::debug!(
                                model_from,
                                model_to,
                                "Failed to alias aggregated llm gateway response model"
                            );
                        }
                        aliased
                    })
            } else {
                response_json
            };
            let adapted_json = adapt_completed_response_json(
                &response_json,
                response_adapter,
                Some(&prepared.tool_name_restore_map),
            );
            let body = match serde_json::to_vec(&adapted_json) {
                Ok(body) => body,
                Err(err) => {
                    if let Err(persist_err) = persist_gateway_failure_usage(
                        state.llm_gateway.as_ref(),
                        key_lease.as_ref(),
                        GatewayFailureUsageRequest {
                            prepared: &prepared,
                            status_code: StatusCode::INTERNAL_SERVER_ERROR.as_u16() as i32,
                            usage,
                            event_context: event_context.as_ref(),
                            selected_account_name: selected_account_name.as_deref(),
                            failure_stage: "adapt_response",
                            error: "Failed to encode aggregated llm gateway response",
                            details: Some(json!({ "error": err.to_string() })),
                        },
                    )
                    .await
                    {
                        tracing::warn!(
                            key_id = %key_lease.record.id,
                            "failed to persist codex failure usage after aggregated response encode error: {persist_err:#}"
                        );
                    }
                    return Err(internal_error(
                        "Failed to encode aggregated llm gateway response",
                        err,
                    ));
                },
            };
            let builder = Response::builder()
                .status(status)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::CACHE_CONTROL, "no-store");
            let response = match apply_upstream_response_headers(builder, &upstream_headers)
                .body(Body::from(body))
            {
                Ok(response) => response,
                Err(err) => {
                    if let Err(persist_err) = persist_gateway_failure_usage(
                        state.llm_gateway.as_ref(),
                        key_lease.as_ref(),
                        GatewayFailureUsageRequest {
                            prepared: &prepared,
                            status_code: StatusCode::INTERNAL_SERVER_ERROR.as_u16() as i32,
                            usage,
                            event_context: event_context.as_ref(),
                            selected_account_name: selected_account_name.as_deref(),
                            failure_stage: "build_response",
                            error: "Failed to build aggregated llm gateway response",
                            details: Some(json!({ "error": err.to_string() })),
                        },
                    )
                    .await
                    {
                        tracing::warn!(
                            key_id = %key_lease.record.id,
                            "failed to persist codex failure usage after aggregated response build error: {persist_err:#}"
                        );
                    }
                    return Err(internal_error(
                        "Failed to build aggregated llm gateway response",
                        err,
                    ));
                },
            };
            if let Some(context) = event_context.as_mut() {
                context.post_headers_body_ms = context.upstream_headers_at.map(elapsed_ms_i32);
                context.stream_finish_ms = Some(elapsed_ms_i32(context.started_at));
            }
            persist_gateway_usage(
                state.llm_gateway.as_ref(),
                key_lease.as_ref(),
                &prepared,
                status.as_u16(),
                usage,
                event_context.clone(),
                selected_account_name.as_deref(),
            )
            .await
            .map_err(|err| internal_error("Failed to persist llm gateway usage", err))?;
            return Ok(response);
        }

        let gateway = state.llm_gateway.clone();
        let mut shutdown_rx = state.shutdown_rx.clone();
        let stream_key_lease = key_lease.clone();
        let stream_account_request_limit_lease = account_request_limit_lease;
        let stream_request_limit_lease = request_limit_lease;
        let stream_response_adapter = response_adapter;
        let stream_event_context = event_context.clone();
        let stream_selected_account_name = selected_account_name.clone();
        let stream_prepared = prepared.clone();
        let body_stream = stream! {
            let _account_request_limit_lease = stream_account_request_limit_lease;
            let _request_limit_lease = stream_request_limit_lease;
            let mut stream_event_context = stream_event_context;
            let mut collector = SseUsageCollector::default();
            let mut chat_metadata = types::ChatStreamMetadata::default();
            let mut events = upstream
                .bytes_stream()
                .map_err(std::io::Error::other)
                .eventsource();
            loop {
                let maybe_event = tokio::select! {
                    biased;
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            tracing::info!(
                                upstream_path = stream_prepared.upstream_path,
                                "stopping llm gateway downstream stream because backend is shutting down"
                            );
                            None
                        } else {
                            continue;
                        }
                    }
                    event = events.next() => event,
                };
                let Some(event) = maybe_event else {
                    break;
                };
                match event {
                    Ok(event) => {
                        collector.observe_event(&event);
                        match stream_response_adapter {
                            GatewayResponseAdapter::Responses => {
                                if let Some(context) = stream_event_context.as_mut() {
                                    if context.first_sse_write_ms.is_none() {
                                        context.first_sse_write_ms =
                                            Some(elapsed_ms_i32(context.started_at));
                                    }
                                }
                                yield Ok::<Bytes, std::io::Error>(encode_sse_event_with_model_alias(
                                    &event,
                                    stream_prepared.model.as_deref(),
                                    stream_prepared.client_visible_model.as_deref(),
                                ));
                            }
                            GatewayResponseAdapter::ChatCompletions => {
                                if let Some(chunk) = convert_response_event_to_chat_chunk(
                                    &event,
                                    Some(&stream_prepared.tool_name_restore_map),
                                    &mut chat_metadata,
                                    stream_prepared.model.as_deref(),
                                    stream_prepared.client_visible_model.as_deref(),
                                ) {
                                    if let Some(context) = stream_event_context.as_mut() {
                                        if context.first_sse_write_ms.is_none() {
                                            context.first_sse_write_ms =
                                                Some(elapsed_ms_i32(context.started_at));
                                        }
                                    }
                                    yield Ok::<Bytes, std::io::Error>(encode_json_sse_chunk(&chunk));
                                }
                            }
                        }
                    }
                    Err(err) => {
                        if let Some(context) = stream_event_context.as_mut() {
                            context.post_headers_body_ms =
                                context.upstream_headers_at.map(elapsed_ms_i32);
                            context.stream_finish_ms = Some(elapsed_ms_i32(context.started_at));
                        }
                        if let Err(persist_err) = persist_gateway_failure_usage(
                            gateway.as_ref(),
                            stream_key_lease.as_ref(),
                            GatewayFailureUsageRequest {
                                prepared: &stream_prepared,
                                status_code: CODEX_STREAM_FAILURE_STATUS_CODE,
                                usage: collector.usage.unwrap_or_else(missing_usage_breakdown),
                                event_context: stream_event_context.as_ref(),
                                selected_account_name: stream_selected_account_name.as_deref(),
                                failure_stage: "stream_read",
                                error: &format!("failed to parse upstream SSE event: {err}"),
                                details: Some(json!({ "stream_kind": "sse" })),
                            },
                        ).await {
                            yield Err(std::io::Error::other(format!(
                                "failed to persist llm gateway failure usage: {persist_err}"
                            )));
                            return;
                        }
                        yield Err(std::io::Error::other(format!(
                            "failed to parse upstream SSE event: {err}"
                        )));
                        return;
                    }
                }
            }
            let usage = collector.usage.unwrap_or_else(missing_usage_breakdown);
            if let Some(context) = stream_event_context.as_mut() {
                context.post_headers_body_ms = context.upstream_headers_at.map(elapsed_ms_i32);
                context.stream_finish_ms = Some(elapsed_ms_i32(context.started_at));
            }
            if let Err(err) = persist_gateway_usage(
                gateway.as_ref(),
                stream_key_lease.as_ref(),
                &stream_prepared,
                status.as_u16(),
                usage,
                stream_event_context.clone(),
                stream_selected_account_name.as_deref(),
            ).await {
                yield Err(std::io::Error::other(format!(
                    "failed to persist llm gateway usage: {err}"
                )));
                return;
            }
            if stream_response_adapter == GatewayResponseAdapter::ChatCompletions {
                yield Ok::<Bytes, std::io::Error>(Bytes::from_static(b"data: [DONE]\n\n"));
            }
        };
        let builder = Response::builder()
            .status(status)
            .header(
                header::CONTENT_TYPE,
                if response_adapter == GatewayResponseAdapter::ChatCompletions {
                    "text/event-stream"
                } else {
                    &content_type
                },
            )
            .header(header::CACHE_CONTROL, "no-store");
        let response = match apply_upstream_response_headers(builder, &upstream_headers)
            .body(Body::from_stream(body_stream))
        {
            Ok(response) => response,
            Err(err) => {
                if let Err(persist_err) = persist_gateway_failure_usage(
                    state.llm_gateway.as_ref(),
                    key_lease.as_ref(),
                    GatewayFailureUsageRequest {
                        prepared: &prepared,
                        status_code: StatusCode::INTERNAL_SERVER_ERROR.as_u16() as i32,
                        usage: missing_usage_breakdown(),
                        event_context: event_context.as_ref(),
                        selected_account_name: selected_account_name.as_deref(),
                        failure_stage: "build_response",
                        error: "Failed to build llm gateway stream response",
                        details: Some(json!({ "error": err.to_string() })),
                    },
                )
                .await
                {
                    tracing::warn!(
                        key_id = %key_lease.record.id,
                        "failed to persist codex failure usage after stream response build error: {persist_err:#}"
                    );
                }
                return Err(internal_error("Failed to build llm gateway stream response", err));
            },
        };
        return Ok(response);
    }

    let body_bytes = match upstream.bytes().await {
        Ok(body_bytes) => {
            if let Some(context) = event_context.as_mut() {
                context.post_headers_body_ms = context.upstream_headers_at.map(elapsed_ms_i32);
                context.stream_finish_ms = Some(elapsed_ms_i32(context.started_at));
            }
            body_bytes
        },
        Err(err) => {
            if let Err(persist_err) = persist_gateway_failure_usage(
                state.llm_gateway.as_ref(),
                key_lease.as_ref(),
                GatewayFailureUsageRequest {
                    prepared: &prepared,
                    status_code: StatusCode::INTERNAL_SERVER_ERROR.as_u16() as i32,
                    usage: missing_usage_breakdown(),
                    event_context: event_context.as_ref(),
                    selected_account_name: selected_account_name.as_deref(),
                    failure_stage: "read_upstream_response",
                    error: "Failed to read llm gateway upstream response",
                    details: Some(json!({
                        "upstream_status": status.as_u16(),
                        "content_type": content_type,
                        "error": err.to_string(),
                    })),
                },
            )
            .await
            {
                tracing::warn!(
                    key_id = %key_lease.record.id,
                    "failed to persist codex failure usage after upstream body read error: {persist_err:#}"
                );
            }
            return Err(internal_error("Failed to read llm gateway upstream response", err));
        },
    };
    if !status.is_success() {
        let body_text = String::from_utf8_lossy(&body_bytes);
        tracing::error!(
            key_id = %key_lease.record.id,
            key_name = %key_lease.record.name,
            account_name = selected_account_name.as_deref().unwrap_or("legacy"),
            original_path = %prepared.original_path,
            upstream_path = %prepared.upstream_path,
            status = status.as_u16(),
            content_type = %content_type,
            model = prepared.model.as_deref().unwrap_or("unknown"),
            body_len = body_bytes.len(),
            body_preview = %summarize_upstream_error_body(&body_text),
            "codex public request returned non-success upstream response"
        );
        if let Err(persist_err) = persist_gateway_failure_usage(
            state.llm_gateway.as_ref(),
            key_lease.as_ref(),
            GatewayFailureUsageRequest {
                prepared: &prepared,
                status_code: status.as_u16() as i32,
                usage: missing_usage_breakdown(),
                event_context: event_context.as_ref(),
                selected_account_name: selected_account_name.as_deref(),
                failure_stage: "upstream_non_success",
                error: &format!("upstream returned non-success status {}", status.as_u16()),
                details: Some(json!({
                    "content_type": content_type,
                    "upstream_body": body_text.to_string(),
                })),
            },
        )
        .await
        {
            tracing::warn!(
                key_id = %key_lease.record.id,
                "failed to persist codex failure usage after non-success upstream response: {persist_err:#}"
            );
        }
    }
    let usage = extract_usage_from_bytes(&body_bytes).unwrap_or_else(missing_usage_breakdown);

    let aliased_body_bytes = rewrite_json_response_model_alias(
        &body_bytes,
        prepared.model.as_deref(),
        prepared.client_visible_model.as_deref(),
    )
    .unwrap_or_else(|| body_bytes.to_vec());

    let response_bytes = if status.is_success()
        && response_adapter == GatewayResponseAdapter::ChatCompletions
    {
        match convert_json_response_to_chat_completion(
            &body_bytes,
            Some(&prepared.tool_name_restore_map),
            prepared.model.as_deref(),
            prepared.client_visible_model.as_deref(),
        ) {
            Ok(bytes) => bytes,
            Err(err) => {
                if let Err(persist_err) = persist_gateway_failure_usage(
                    state.llm_gateway.as_ref(),
                    key_lease.as_ref(),
                    GatewayFailureUsageRequest {
                        prepared: &prepared,
                        status_code: StatusCode::INTERNAL_SERVER_ERROR.as_u16() as i32,
                        usage,
                        event_context: event_context.as_ref(),
                        selected_account_name: selected_account_name.as_deref(),
                        failure_stage: "adapt_response",
                        error: "Failed to adapt upstream response to chat.completions",
                        details: Some(json!({ "error": err })),
                    },
                )
                .await
                {
                    tracing::warn!(
                        key_id = %key_lease.record.id,
                        "failed to persist codex failure usage after response adaptation error: {persist_err:#}"
                    );
                }
                return Err(internal_error(
                    "Failed to adapt upstream response to chat.completions",
                    err,
                ));
            },
        }
    } else {
        aliased_body_bytes
    };

    let builder = Response::builder()
        .status(status)
        .header(
            header::CONTENT_TYPE,
            if status.is_success() && response_adapter == GatewayResponseAdapter::ChatCompletions {
                "application/json"
            } else {
                &content_type
            },
        )
        .header(header::CACHE_CONTROL, "no-store");
    let response = match apply_upstream_response_headers(builder, &upstream_headers)
        .body(Body::from(response_bytes))
    {
        Ok(response) => response,
        Err(err) => {
            if status.is_success() {
                if let Err(persist_err) = persist_gateway_failure_usage(
                    state.llm_gateway.as_ref(),
                    key_lease.as_ref(),
                    GatewayFailureUsageRequest {
                        prepared: &prepared,
                        status_code: StatusCode::INTERNAL_SERVER_ERROR.as_u16() as i32,
                        usage,
                        event_context: event_context.as_ref(),
                        selected_account_name: selected_account_name.as_deref(),
                        failure_stage: "build_response",
                        error: "Failed to build llm gateway response",
                        details: Some(json!({ "error": err.to_string() })),
                    },
                )
                .await
                {
                    tracing::warn!(
                        key_id = %key_lease.record.id,
                        "failed to persist codex failure usage after response build error: {persist_err:#}"
                    );
                }
            }
            return Err(internal_error("Failed to build llm gateway response", err));
        },
    };
    if status.is_success() {
        persist_gateway_usage(
            state.llm_gateway.as_ref(),
            key_lease.as_ref(),
            &prepared,
            status.as_u16(),
            usage,
            event_context,
            selected_account_name.as_deref(),
        )
        .await
        .map_err(|err| internal_error("Failed to persist llm gateway usage", err))?;
    }
    Ok(response)
}

/// Persist one settled usage event and refresh the key cache with new counters.
async fn persist_gateway_usage(
    gateway: &LlmGatewayRuntimeState,
    cached_key: &CachedKeyLease,
    prepared: &PreparedGatewayRequest,
    status_code: u16,
    usage: UsageBreakdown,
    event_context: Option<LlmGatewayEventContext>,
    selected_account_name: Option<&str>,
) -> Result<()> {
    let current = gateway
        .store
        .get_key_by_id(&cached_key.record.id)
        .await?
        .unwrap_or_else(|| cached_key.record.clone());
    let context = event_context.unwrap_or_else(|| default_gateway_event_context(prepared));
    let latency_ms = context
        .started_at
        .elapsed()
        .as_millis()
        .min(i32::MAX as u128) as i32;
    if usage.usage_missing {
        tracing::warn!(
            key_id = %current.id,
            upstream_path = prepared.upstream_path,
            status_code,
            latency_ms,
            "LLM gateway usage payload was missing and fell back to zeroed counters"
        );
    }
    let last_message_content = match extract_last_message_content(&prepared.client_request_body) {
        Ok(content) => content,
        Err(err) => {
            tracing::debug!(
                key_id = %current.id,
                upstream_path = prepared.upstream_path,
                "Failed to extract last message content from request body: {err}"
            );
            Some(LAST_MESSAGE_CONTENT_EXTRACT_FAILED.to_string())
        },
    };
    let event = build_gateway_usage_event_record(GatewayUsageEventBuild {
        current: &current,
        prepared,
        context: &context,
        latency_ms,
        status_code: status_code as i32,
        usage,
        last_message_content,
        selected_account_name,
    });
    let updated = gateway.append_usage_event(&current, &event).await?;

    tracing::info!(
        key_id = %updated.id,
        key_name = %updated.name,
        event_id = %event.id,
        account_name = event.account_name.as_deref().unwrap_or("legacy"),
        request_url = %event.request_url,
        status_code = event.status_code,
        latency_ms = event.latency_ms,
        billable_tokens = event.billable_tokens,
        "Persisted LLM gateway usage event"
    );

    let ttl = gateway_auth_cache_ttl(gateway).await;
    if updated.status == LLM_GATEWAY_KEY_STATUS_ACTIVE {
        gateway.key_cache.renew(updated, Duration::from_secs(ttl));
    } else {
        gateway.key_cache.invalidate(&cached_key.record.key_hash);
    }
    Ok(())
}

async fn persist_gateway_failure_usage(
    gateway: &LlmGatewayRuntimeState,
    cached_key: &CachedKeyLease,
    failure: GatewayFailureUsageRequest<'_>,
) -> Result<()> {
    let current = gateway
        .store
        .get_key_by_id(&cached_key.record.id)
        .await?
        .unwrap_or_else(|| cached_key.record.clone());
    let context = failure
        .event_context
        .cloned()
        .unwrap_or_else(|| default_gateway_event_context(failure.prepared));
    let latency_ms = context
        .started_at
        .elapsed()
        .as_millis()
        .min(i32::MAX as u128) as i32;
    let diagnostic_payload = build_codex_failure_diagnostic_payload(
        failure.prepared,
        Some(&context),
        failure.selected_account_name,
        failure.failure_stage,
        failure.status_code,
        failure.error,
        failure.details,
    );
    let event = build_gateway_usage_event_record(GatewayUsageEventBuild {
        current: &current,
        prepared: failure.prepared,
        context: &context,
        latency_ms,
        status_code: failure.status_code,
        usage: failure.usage,
        last_message_content: Some(diagnostic_payload),
        selected_account_name: failure.selected_account_name,
    });
    let updated = gateway.append_usage_event(&current, &event).await?;

    tracing::warn!(
        key_id = %updated.id,
        key_name = %updated.name,
        event_id = %event.id,
        account_name = event.account_name.as_deref().unwrap_or("legacy"),
        request_url = %event.request_url,
        status_code = event.status_code,
        latency_ms = event.latency_ms,
        "Persisted LLM gateway failure usage event"
    );

    let ttl = gateway_auth_cache_ttl(gateway).await;
    if updated.status == LLM_GATEWAY_KEY_STATUS_ACTIVE {
        gateway.key_cache.renew(updated, Duration::from_secs(ttl));
    } else {
        gateway.key_cache.invalidate(&cached_key.record.key_hash);
    }
    Ok(())
}

// === Shared helpers ===

/// Fetch one usage payload from the upstream Codex account endpoint and map it
/// into public-facing bucket rows.
async fn fetch_rate_limit_status_snapshot(
    runtime: &Arc<LlmGatewayRuntimeState>,
    source_url: &str,
) -> Result<Vec<LlmGatewayRateLimitBucketView>> {
    let auth_snapshot = runtime.auth_source.current().await?;
    match send_rate_limit_status_request(runtime, source_url, &auth_snapshot).await {
        Ok(payload) => Ok(map_rate_limit_status_payload(payload)),
        Err(first_err) if status_error_is_unauthorized(&first_err) => {
            tracing::info!(
                "Rate-limit status request hit unauthorized response, forcing auth reload"
            );
            let refreshed = runtime.auth_source.force_reload().await?;
            send_rate_limit_status_request(runtime, source_url, &refreshed)
                .await
                .map(map_rate_limit_status_payload)
        },
        Err(err) => Err(err),
    }
}

/// Issue the authenticated `GET /wham/usage` request.
async fn send_rate_limit_status_request(
    runtime: &Arc<LlmGatewayRuntimeState>,
    source_url: &str,
    auth_snapshot: &CodexAuthSnapshot,
) -> Result<UsageStatusPayload> {
    let (client, _) = runtime.build_upstream_client(auth_snapshot).await?;
    let codex_client_version =
        resolve_codex_client_version(Some(&runtime.runtime_config.read().codex_client_version));
    let mut request = client
        .get(source_url)
        .header(reqwest::header::USER_AGENT, codex_user_agent(&codex_client_version))
        .header(reqwest::header::AUTHORIZATION, bearer_header(&auth_snapshot.access_token)?)
        .header(reqwest::header::ACCEPT, "application/json")
        .timeout(Duration::from_secs(20));

    if let Some(account_id) = auth_snapshot.account_id.as_deref() {
        request = request.header("ChatGPT-Account-Id", account_id);
    }

    let response = request
        .send()
        .await
        .with_context(|| format!("failed to request `{source_url}`"))?;
    let status = response.status();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/json")
        .to_string();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!(
            "GET {source_url} failed: {status}; content-type={content_type}; body={body}"
        );
    }
    serde_json::from_str::<UsageStatusPayload>(&body)
        .with_context(|| format!("failed to decode rate-limit payload from `{source_url}`"))
}

/// Detect the common unauthorized shape from a reqwest/JSON decoding error
/// string so the caller can retry once after reloading auth.json.
fn status_error_is_unauthorized(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        let text = cause.to_string();
        text.contains(" 401 ") || text.contains("401 Unauthorized")
    })
}

/// Convert the raw upstream usage payload into display-ready public buckets.
pub(super) fn map_rate_limit_status_payload(
    payload: UsageStatusPayload,
) -> Vec<LlmGatewayRateLimitBucketView> {
    let plan_type = payload.plan_type.as_deref().map(normalize_plan_type_label);
    let mut buckets = Vec::new();
    buckets.push(LlmGatewayRateLimitBucketView {
        limit_id: "codex".to_string(),
        limit_name: None,
        display_name: "codex".to_string(),
        is_primary: true,
        plan_type: plan_type.clone(),
        primary: payload
            .rate_limit
            .as_ref()
            .and_then(|details| details.primary_window.as_ref())
            .map(map_rate_limit_window),
        secondary: payload
            .rate_limit
            .as_ref()
            .and_then(|details| details.secondary_window.as_ref())
            .map(map_rate_limit_window),
        credits: payload.credits.as_ref().map(map_credits_view),
        account_name: None,
    });
    buckets.extend(
        payload
            .additional_rate_limits
            .unwrap_or_default()
            .into_iter()
            .map(|details| {
                let limit_id = details
                    .metered_feature
                    .as_deref()
                    .map(normalize_limit_id)
                    .unwrap_or_else(|| "codex_other".to_string());
                let display_name = details
                    .limit_name
                    .clone()
                    .or_else(|| details.metered_feature.clone())
                    .unwrap_or_else(|| limit_id.clone());
                LlmGatewayRateLimitBucketView {
                    limit_id,
                    limit_name: details.limit_name.clone(),
                    display_name,
                    is_primary: false,
                    plan_type: plan_type.clone(),
                    primary: details
                        .rate_limit
                        .as_ref()
                        .and_then(|rate_limit| rate_limit.primary_window.as_ref())
                        .map(map_rate_limit_window),
                    secondary: details
                        .rate_limit
                        .as_ref()
                        .and_then(|rate_limit| rate_limit.secondary_window.as_ref())
                        .map(map_rate_limit_window),
                    credits: None,
                    account_name: None,
                }
            }),
    );
    buckets
}

/// Map one upstream usage window into a public view model with remaining
/// percentage precomputed.
fn map_rate_limit_window(window: &UsageRateLimitWindow) -> LlmGatewayRateLimitWindowView {
    let used_percent = window.used_percent.clamp(0.0, 100.0);
    LlmGatewayRateLimitWindowView {
        used_percent,
        remaining_percent: (100.0 - used_percent).clamp(0.0, 100.0),
        window_duration_mins: window.limit_window_seconds.map(seconds_to_window_minutes),
        resets_at: window.reset_at,
    }
}

/// Normalize the upstream credit payload into a stable public shape.
fn map_credits_view(credits: &UsageCreditsDetails) -> LlmGatewayCreditsView {
    LlmGatewayCreditsView {
        has_credits: credits.has_credits,
        unlimited: credits.unlimited,
        balance: credits.balance.as_ref().map(balance_value_to_string),
    }
}

/// Convert flexible numeric/string credit balances into one printable string.
fn balance_value_to_string(value: &UsageBalanceValue) -> String {
    match value {
        UsageBalanceValue::String(value) => value.trim().to_string(),
        UsageBalanceValue::Number(value) => format!("{value:.2}"),
        UsageBalanceValue::Integer(value) => value.to_string(),
    }
}

/// Derive the account usage endpoint from the configured upstream base URL.
fn compute_rate_limit_status_url() -> String {
    let upstream_base = env::var("STATICFLOW_LLM_GATEWAY_UPSTREAM_BASE_URL")
        .ok()
        .map(|value| normalize_upstream_base_url(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_UPSTREAM_BASE_URL.to_string());
    let normalized = upstream_base.trim_end_matches('/');
    let lower = normalized.to_ascii_lowercase();
    if lower.contains("/backend-api/codex") {
        format!("{}/wham/usage", normalized.trim_end_matches("/codex"))
    } else if lower.contains("/backend-api") {
        format!("{normalized}/wham/usage")
    } else {
        format!("{normalized}/api/codex/usage")
    }
}

/// Match Codex's duration bucketing for 5h / weekly / monthly labels.
fn seconds_to_window_minutes(seconds: i64) -> i64 {
    ((seconds.max(0)) + 59) / 60
}

/// Normalize upstream plan strings for presentation.
fn normalize_plan_type_label(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return "unknown".to_string();
    }
    let mut chars = trimmed.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
        None => "unknown".to_string(),
    }
}

/// Keep limit identifiers stable across the rate-limit cache.
fn normalize_limit_id(raw: &str) -> String {
    raw.trim().to_ascii_lowercase().replace('-', "_")
}

/// Join the configured upstream base URL with an OpenAI-style request path.
fn compute_upstream_url(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    if base.contains("/backend-api/codex") && path.starts_with("/v1/") {
        format!("{}{}", base, path.trim_start_matches("/v1"))
    } else if base.ends_with("/v1") && path.starts_with("/v1") {
        format!("{}{}", base.trim_end_matches("/v1"), path)
    } else {
        format!("{base}{path}")
    }
}

/// Default user agent used when callers do not provide their own.
fn codex_user_agent(client_version: &str) -> String {
    format!("{DEFAULT_WIRE_ORIGINATOR}/{client_version}")
}

/// Generate a user-facing API key secret with a stable prefix.
fn generate_secret() -> String {
    let raw = generate_id("sfk-seed");
    format!("sfk_{}", sha256_hex(raw.as_bytes()))
}

/// Generate a roughly time-ordered identifier for keys and usage events.
fn generate_id(prefix: &str) -> String {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    format!("{prefix}-{now_ms}-{nanos}")
}

/// Compute the lowercase hexadecimal SHA-256 digest for key lookup/storage.
fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

fn validate_public_usage_lookup_key(
    key: &LlmGatewayKeyRecord,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    if key.status != LLM_GATEWAY_KEY_STATUS_ACTIVE {
        return Err(public_usage_lookup_not_found());
    }
    Ok(())
}

fn public_usage_lookup_not_found() -> (StatusCode, Json<ErrorResponse>) {
    not_found("queryable key not found")
}

fn public_usage_chart_window_start(now_ms: i64) -> i64 {
    let aligned_now =
        now_ms.div_euclid(PUBLIC_USAGE_LOOKUP_BUCKET_MS) * PUBLIC_USAGE_LOOKUP_BUCKET_MS;
    aligned_now.saturating_sub(
        (PUBLIC_USAGE_LOOKUP_CHART_BUCKETS.saturating_sub(1) as i64)
            .saturating_mul(PUBLIC_USAGE_LOOKUP_BUCKET_MS),
    )
}

fn build_public_usage_chart_points(
    events: &[LlmGatewayUsageEventRecord],
    now_ms: i64,
) -> Vec<PublicLlmGatewayUsageChartPointView> {
    let start_ms = public_usage_chart_window_start(now_ms);
    let mut buckets = vec![0_u64; PUBLIC_USAGE_LOOKUP_CHART_BUCKETS];

    for event in events {
        if event.created_at < start_ms {
            continue;
        }
        let bucket_index = event
            .created_at
            .saturating_sub(start_ms)
            .div_euclid(PUBLIC_USAGE_LOOKUP_BUCKET_MS);
        if let Ok(bucket_index) = usize::try_from(bucket_index) {
            if bucket_index < PUBLIC_USAGE_LOOKUP_CHART_BUCKETS {
                buckets[bucket_index] = buckets[bucket_index]
                    .saturating_add(event.input_uncached_tokens)
                    .saturating_add(event.output_tokens);
            }
        }
    }

    buckets
        .into_iter()
        .enumerate()
        .map(|(index, tokens)| PublicLlmGatewayUsageChartPointView {
            bucket_start_ms: start_ms
                .saturating_add((index as i64).saturating_mul(PUBLIC_USAGE_LOOKUP_BUCKET_MS)),
            tokens,
        })
        .collect()
}

fn json_no_store_response<T: serde::Serialize>(
    payload: &T,
    encode_error_message: &str,
    build_error_message: &str,
) -> Result<Response, (StatusCode, Json<ErrorResponse>)> {
    let body =
        serde_json::to_vec(payload).map_err(|err| internal_error(encode_error_message, err))?;
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(body))
        .map_err(|err| internal_error(build_error_message, err))
}

/// Build a standardized 400 error payload.
fn bad_request(message: &str) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::BAD_REQUEST,
        Json(ErrorResponse {
            error: message.to_string(),
            code: 400,
        }),
    )
}

/// Build a standardized 400 error payload and log the underlying detail.
fn bad_request_with_detail(
    message: &str,
    err: impl std::fmt::Display,
) -> (StatusCode, Json<ErrorResponse>) {
    tracing::warn!("{message}: {err}");
    bad_request(message)
}

/// Build a standardized 405 error payload.
fn method_not_allowed(message: &str) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        Json(ErrorResponse {
            error: message.to_string(),
            code: 405,
        }),
    )
}

/// Build a standardized 404 error payload.
fn not_found(message: &str) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::NOT_FOUND,
        Json(ErrorResponse {
            error: message.to_string(),
            code: 404,
        }),
    )
}

/// Build a standardized 409 error payload.
fn conflict_error(message: &str) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::CONFLICT,
        Json(ErrorResponse {
            error: message.to_string(),
            code: 409,
        }),
    )
}

/// Build a standardized auth-related error payload.
fn auth_error(status: StatusCode, message: &str) -> (StatusCode, Json<ErrorResponse>) {
    (
        status,
        Json(ErrorResponse {
            error: message.to_string(),
            code: status.as_u16(),
        }),
    )
}

fn codex_key_request_limit_error(
    key: &LlmGatewayKeyRecord,
    rejection: CodexKeyRequestLimitRejection,
) -> (StatusCode, Json<ErrorResponse>) {
    let max_concurrency = rejection
        .max_concurrency
        .map(|value| value.to_string())
        .unwrap_or_else(|| "unlimited".to_string());
    let min_start_interval_ms = rejection
        .min_start_interval_ms
        .map(|value| value.to_string())
        .unwrap_or_else(|| "unlimited".to_string());
    let message = match rejection.reason {
        "local_concurrency_limit" => format!(
            "codex key request limit exceeded: key `{}` allows at most {} concurrent requests; \
             currently {} requests are already in flight (request_max_concurrency={}, \
             request_min_start_interval_ms={})",
            key.name,
            rejection.max_concurrency.unwrap_or(0),
            rejection.in_flight,
            max_concurrency,
            min_start_interval_ms,
        ),
        "local_start_interval" => format!(
            "codex key pacing limit exceeded: key `{}` requires at least {} ms between request \
             starts; elapsed={} ms, remaining_wait={} ms, in_flight={}, \
             request_max_concurrency={}, request_min_start_interval_ms={}",
            key.name,
            rejection.min_start_interval_ms.unwrap_or(0),
            rejection.elapsed_since_last_start_ms.unwrap_or(0),
            rejection
                .wait
                .map(|value| value.as_millis() as u64)
                .unwrap_or(0),
            rejection.in_flight,
            max_concurrency,
            min_start_interval_ms,
        ),
        other => format!(
            "codex key request limit exceeded for key `{}`: reason={}, in_flight={}, \
             request_max_concurrency={}, request_min_start_interval_ms={}",
            key.name, other, rejection.in_flight, max_concurrency, min_start_interval_ms,
        ),
    };
    tracing::warn!(
        key_id = %key.id,
        key_name = %key.name,
        reason = rejection.reason,
        in_flight = rejection.in_flight,
        max_concurrency = ?rejection.max_concurrency,
        min_start_interval_ms = ?rejection.min_start_interval_ms,
        wait_ms = rejection.wait.map(|value| value.as_millis() as u64),
        elapsed_since_last_start_ms = rejection.elapsed_since_last_start_ms,
        "rejected codex request because the key-level local request limit was exceeded"
    );
    auth_error(StatusCode::TOO_MANY_REQUESTS, &message)
}

/// Build a standardized 500 error payload and log the internal failure detail.
fn internal_error(message: &str, err: impl std::fmt::Display) -> (StatusCode, Json<ErrorResponse>) {
    tracing::error!("{message}: {err}");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorResponse {
            error: message.to_string(),
            code: 500,
        }),
    )
}

// === Admin account pool management ===

/// Import a Codex account into the pool after verifying it can reach the
/// upstream usage endpoint.
pub async fn import_account(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ImportAccountRequest>,
) -> Result<Json<AccountSummaryView>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let name = accounts::validate_account_name(&request.name).map_err(|err| bad_request(&err))?;
    let pool = &state.llm_gateway.account_pool;
    if pool.exists(&name).await {
        return Err(bad_request(&format!("account `{name}` already exists")));
    }

    let access_token = request.tokens.access_token.trim().to_string();
    let refresh_token = request.tokens.refresh_token.trim().to_string();
    let id_token = request.tokens.id_token.trim().to_string();
    let account_id = request
        .tokens
        .account_id
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string);

    if access_token.is_empty() {
        return Err(bad_request("access_token is required"));
    }

    // Validate by fetching usage through the existing proxy.
    let auth = runtime::CodexAuthSnapshot::from_tokens(access_token.clone(), account_id.clone());
    let codex_client_version = resolve_codex_client_version(Some(
        &state.llm_gateway_runtime_config.read().codex_client_version,
    ));
    let usage = token_refresh::validate_account_usage(
        state.upstream_proxy_registry.as_ref(),
        &auth,
        &codex_client_version,
    )
    .await
    .map_err(|err| bad_request(&format!("account verification failed: {err}")))?;

    let account = accounts::CodexAccount {
        name: name.clone(),
        access_token,
        account_id,
        refresh_token,
        id_token,
        map_gpt53_codex_to_spark: false,
        proxy_selection: Default::default(),
        request_max_concurrency: None,
        request_min_start_interval_ms: None,
        last_refresh: Some(chrono::Utc::now()),
        status: accounts::AccountStatus::Active,
    };
    pool.insert(account)
        .await
        .map_err(|err| internal_error("Failed to persist account", err))?;
    pool.update_rate_limit(&name, usage.clone()).await;

    tracing::info!(account = name, "Imported Codex account into gateway pool");

    let summary = state
        .llm_gateway
        .account_pool
        .list_summaries()
        .await
        .into_iter()
        .find(|summary| summary.name == name)
        .ok_or_else(|| not_found("account not found after import"))?;
    Ok(Json(build_codex_account_summary_view(&state, summary).await))
}

/// List all managed Codex accounts in the pool.
pub async fn list_accounts(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<AccountListResponse>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let summaries = state.llm_gateway.account_pool.list_summaries().await;
    let mut accounts = Vec::with_capacity(summaries.len());
    for summary in summaries {
        accounts.push(build_codex_account_summary_view(&state, summary).await);
    }
    Ok(Json(AccountListResponse {
        accounts,
        generated_at: now_ms(),
    }))
}

pub async fn patch_account_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(name): axum::extract::Path<String>,
    Json(request): Json<PatchAccountSettingsRequest>,
) -> Result<Json<AccountSummaryView>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let name = accounts::validate_account_name(&name).map_err(|err| bad_request(&err))?;
    let proxy_selection = parse_account_proxy_selection_patch(
        request.proxy_mode.as_deref(),
        request.proxy_config_id.as_deref(),
    )
    .map_err(|err| bad_request(&err.to_string()))?;
    let scheduler_settings_requested = request.request_max_concurrency.is_some()
        || request.request_min_start_interval_ms.is_some()
        || request.request_max_concurrency_unlimited
        || request.request_min_start_interval_ms_unlimited;
    if request.map_gpt53_codex_to_spark.is_none()
        && proxy_selection.is_none()
        && !scheduler_settings_requested
    {
        return Err(bad_request("at least one account setting field must be provided"));
    }
    if let Some(proxy_selection) = proxy_selection.as_ref() {
        state
            .upstream_proxy_registry
            .resolve_proxy_for_selection(LLM_GATEWAY_PROVIDER_CODEX, Some(proxy_selection))
            .await
            .map_err(|err| bad_request(&format!("invalid proxy selection: {err}")))?;
    }

    let summaries = state.llm_gateway.account_pool.list_summaries().await;
    let current = summaries
        .iter()
        .find(|summary| summary.name == name)
        .ok_or_else(|| not_found("account not found"))?;
    if request.map_gpt53_codex_to_spark == Some(true) && !current.rate_limits.is_gpt_pro() {
        return Err(bad_request("Spark mapping is only available for accounts with plan_type=Pro"));
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

    let updated = state
        .llm_gateway
        .account_pool
        .update_settings(&name, AccountSettingsPatch {
            map_gpt53_codex_to_spark: request.map_gpt53_codex_to_spark,
            proxy_selection,
            request_max_concurrency,
            request_min_start_interval_ms,
        })
        .await
        .map_err(|err| internal_error("Failed to update account settings", err))?;
    if !updated {
        return Err(not_found("account not found"));
    }
    if scheduler_settings_requested {
        state
            .llm_gateway
            .account_request_scheduler
            .notify_config_changed();
    }

    let summary = state
        .llm_gateway
        .account_pool
        .list_summaries()
        .await
        .into_iter()
        .find(|summary| summary.name == name)
        .ok_or_else(|| not_found("account not found"))?;

    tracing::info!(
        account = summary.name,
        map_gpt53_codex_to_spark = summary.map_gpt53_codex_to_spark,
        request_max_concurrency = ?summary.request_max_concurrency,
        request_min_start_interval_ms = ?summary.request_min_start_interval_ms,
        proxy_mode = %summary.proxy_selection.proxy_mode.as_str(),
        proxy_config_id = ?summary.proxy_selection.proxy_config_id,
        "Updated Codex account settings"
    );

    Ok(Json(build_codex_account_summary_view(&state, summary).await))
}

/// Force-refresh one managed Codex account and return its latest summary.
pub async fn refresh_account(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Result<Json<AccountSummaryView>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let name = accounts::validate_account_name(&name).map_err(|err| bad_request(&err))?;
    token_refresh::refresh_account_once(
        state.llm_gateway.account_pool.as_ref(),
        state.upstream_proxy_registry.as_ref(),
        state.llm_gateway_runtime_config.as_ref(),
        &name,
    )
    .await
    .map_err(|err| internal_error("Failed to refresh account status", err))?;

    let summary = state
        .llm_gateway
        .account_pool
        .list_summaries()
        .await
        .into_iter()
        .find(|summary| summary.name == name)
        .ok_or_else(|| not_found("account not found"))?;
    Ok(Json(build_codex_account_summary_view(&state, summary).await))
}

async fn build_codex_account_summary_view(
    state: &AppState,
    summary: accounts::AccountSummarySnapshot,
) -> AccountSummaryView {
    match state
        .upstream_proxy_registry
        .resolve_proxy_for_selection(LLM_GATEWAY_PROVIDER_CODEX, Some(&summary.proxy_selection))
        .await
    {
        Ok(resolved_proxy) => {
            account_summary_view_from_summary(summary, Some(&resolved_proxy), None)
        },
        Err(err) => account_summary_view_from_summary(summary, None, Some(err.to_string())),
    }
}

fn account_summary_view_from_summary(
    summary: accounts::AccountSummarySnapshot,
    resolved_proxy: Option<&ResolvedUpstreamProxy>,
    invalid_proxy_message: Option<String>,
) -> AccountSummaryView {
    let (effective_proxy_source, effective_proxy_url, effective_proxy_config_name) =
        if let Some(resolved_proxy) = resolved_proxy {
            (
                resolved_proxy.source.as_str().to_string(),
                resolved_proxy.proxy_url.clone(),
                resolved_proxy.proxy_config_name.clone(),
            )
        } else {
            (
                format!(
                    "invalid ({})",
                    invalid_proxy_message.unwrap_or_else(|| "unknown".to_string())
                ),
                None,
                None,
            )
        };
    AccountSummaryView {
        name: summary.name,
        status: summary.status.as_str().to_string(),
        account_id: summary.account_id,
        plan_type: summary.rate_limits.primary_plan_type(),
        primary_remaining_percent: summary.rate_limits.primary_remaining_percent(),
        secondary_remaining_percent: summary.rate_limits.secondary_remaining_percent(),
        map_gpt53_codex_to_spark: summary.map_gpt53_codex_to_spark,
        request_max_concurrency: summary.request_max_concurrency,
        request_min_start_interval_ms: summary.request_min_start_interval_ms,
        proxy_mode: summary.proxy_selection.proxy_mode.as_str().to_string(),
        proxy_config_id: summary.proxy_selection.proxy_config_id,
        effective_proxy_source,
        effective_proxy_url,
        effective_proxy_config_name,
        last_refresh: summary
            .rate_limits
            .last_checked_at
            .or(summary.last_refresh_ms),
        last_usage_checked_at: summary.usage_refresh.last_checked_at,
        last_usage_success_at: summary.usage_refresh.last_success_at,
        usage_error_message: summary.usage_refresh.error_message,
    }
}

/// Remove a Codex account from the pool and delete its auth file.
pub async fn remove_account(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let removed = state
        .llm_gateway
        .account_pool
        .remove(&name)
        .await
        .map_err(|err| internal_error("Failed to remove account", err))?;
    if !removed {
        return Err(not_found("account not found"));
    }
    tracing::info!(account = name, "Removed Codex account from gateway pool");
    Ok(Json(json!({ "deleted": true, "name": name })))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use static_flow_shared::llm_gateway_store::{
        now_ms, LLM_GATEWAY_PROTOCOL_OPENAI, LLM_GATEWAY_PROVIDER_CODEX,
    };

    use super::*;
    use crate::{
        llm_gateway::accounts::{AccountRateLimitSnapshot, AccountUsageRefreshHealth},
        upstream_proxy::AccountProxySelection,
    };

    fn sample_public_lookup_key() -> LlmGatewayKeyRecord {
        let now = now_ms();
        LlmGatewayKeyRecord {
            id: "key-public-lookup".to_string(),
            name: "Public Lookup Key".to_string(),
            secret: "sfk_example".to_string(),
            key_hash: "hash".to_string(),
            status: LLM_GATEWAY_KEY_STATUS_ACTIVE.to_string(),
            provider_type: LLM_GATEWAY_PROVIDER_CODEX.to_string(),
            protocol_family: LLM_GATEWAY_PROTOCOL_OPENAI.to_string(),
            public_visible: true,
            quota_billable_limit: 10_000,
            usage_input_uncached_tokens: 1_000,
            usage_input_cached_tokens: 500,
            usage_output_tokens: 700,
            usage_billable_tokens: 1_700,
            usage_credit_total: 0.0,
            usage_credit_missing_events: 0,
            last_used_at: Some(now),
            created_at: now,
            updated_at: now,
            route_strategy: None,
            account_group_id: None,
            fixed_account_name: None,
            auto_account_names: None,
            model_name_map: None,
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
            kiro_request_validation_enabled: true,
            kiro_cache_estimation_enabled: true,
            kiro_zero_cache_debug_enabled: false,
            kiro_cache_policy_override_json: None,
            kiro_billable_model_multipliers_override_json: None,
        }
    }

    fn sample_usage_event(id: &str, created_at: i64) -> LlmGatewayUsageEventRecord {
        LlmGatewayUsageEventRecord {
            id: id.to_string(),
            key_id: "key-public-lookup".to_string(),
            key_name: "Public Lookup Key".to_string(),
            provider_type: LLM_GATEWAY_PROVIDER_CODEX.to_string(),
            account_name: Some("default".to_string()),
            request_method: "POST".to_string(),
            request_url: "/api/llm-gateway/v1/responses".to_string(),
            latency_ms: 120,
            routing_wait_ms: None,
            upstream_headers_ms: None,
            post_headers_body_ms: None,
            request_body_bytes: None,
            request_body_read_ms: None,
            request_json_parse_ms: None,
            pre_handler_ms: None,
            first_sse_write_ms: None,
            stream_finish_ms: None,
            quota_failover_count: 0,
            routing_diagnostics_json: None,
            endpoint: "/responses".to_string(),
            model: Some("gpt-5.3-codex".to_string()),
            status_code: 200,
            input_uncached_tokens: 0,
            input_cached_tokens: 0,
            output_tokens: 0,
            billable_tokens: 0,
            usage_missing: false,
            credit_usage: None,
            credit_usage_missing: false,
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: "{}".to_string(),
            last_message_content: Some("secret".to_string()),
            client_request_body_json: None,
            upstream_request_body_json: None,
            full_request_json: None,
            created_at,
        }
    }

    fn sample_prepared_gateway_request() -> PreparedGatewayRequest {
        PreparedGatewayRequest {
            original_path: "/v1/responses".to_string(),
            upstream_path: "/v1/responses".to_string(),
            method: axum::http::Method::POST,
            client_request_body: Bytes::from_static(br#"{"input":"hello"}"#),
            request_body: Bytes::from_static(br#"{"input":"hello","stream":true}"#),
            model: Some("gpt-5.3-codex".to_string()),
            client_visible_model: None,
            wants_stream: false,
            force_upstream_stream: true,
            content_type: "application/json".to_string(),
            response_adapter: GatewayResponseAdapter::Responses,
            thread_anchor: None,
            tool_name_restore_map: BTreeMap::new(),
            billable_multiplier: 1,
        }
    }

    fn sample_gateway_event_context() -> LlmGatewayEventContext {
        LlmGatewayEventContext {
            request_method: "POST".to_string(),
            request_url: "/api/llm-gateway/v1/responses".to_string(),
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: "{}".to_string(),
            started_at: Instant::now(),
            routing_wait_ms: None,
            upstream_headers_ms: None,
            post_headers_body_ms: None,
            request_body_bytes: None,
            request_body_read_ms: None,
            request_json_parse_ms: None,
            pre_handler_ms: None,
            first_sse_write_ms: None,
            stream_finish_ms: None,
            routing_diagnostics_json: None,
            upstream_headers_at: None,
        }
    }

    fn sample_account_summary(
        name: &str,
        status: AccountStatus,
        buckets: Vec<LlmGatewayRateLimitBucketView>,
        usage_error_message: Option<&str>,
    ) -> AccountSummarySnapshot {
        AccountSummarySnapshot {
            name: name.to_string(),
            status,
            account_id: None,
            rate_limits: AccountRateLimitSnapshot {
                buckets,
                last_checked_at: Some(1_710_000_123_000),
            },
            usage_refresh: AccountUsageRefreshHealth {
                last_checked_at: Some(1_710_000_123_000),
                last_success_at: Some(1_710_000_120_000),
                error_message: usage_error_message.map(str::to_string),
            },
            last_refresh_ms: Some(1_710_000_110_000),
            map_gpt53_codex_to_spark: false,
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
            proxy_selection: AccountProxySelection::default(),
        }
    }

    fn sample_bucket(
        account_name: Option<&str>,
        primary_remaining_percent: f64,
        secondary_remaining_percent: f64,
    ) -> LlmGatewayRateLimitBucketView {
        LlmGatewayRateLimitBucketView {
            limit_id: "codex".to_string(),
            limit_name: None,
            display_name: "codex".to_string(),
            is_primary: true,
            plan_type: Some("Pro".to_string()),
            primary: Some(LlmGatewayRateLimitWindowView {
                used_percent: 100.0 - primary_remaining_percent,
                remaining_percent: primary_remaining_percent,
                window_duration_mins: Some(300),
                resets_at: Some(1_710_000_500_000),
            }),
            secondary: Some(LlmGatewayRateLimitWindowView {
                used_percent: 100.0 - secondary_remaining_percent,
                remaining_percent: secondary_remaining_percent,
                window_duration_mins: Some(10_080),
                resets_at: Some(1_710_600_500_000),
            }),
            credits: Some(LlmGatewayCreditsView {
                has_credits: true,
                unlimited: false,
                balance: Some("24".to_string()),
            }),
            account_name: account_name.map(str::to_string),
        }
    }

    #[test]
    fn build_public_usage_chart_points_uses_uncached_input_and_output_only() {
        let now_ms = 1_710_004_500_000;
        let start_ms = public_usage_chart_window_start(now_ms);

        let mut first_bucket = sample_usage_event("evt-1", start_ms + 10_000);
        first_bucket.input_uncached_tokens = 100;
        first_bucket.input_cached_tokens = 999;
        first_bucket.output_tokens = 40;
        first_bucket.billable_tokens = 1_139;

        let mut last_bucket = sample_usage_event(
            "evt-2",
            start_ms
                + ((PUBLIC_USAGE_LOOKUP_CHART_BUCKETS - 1) as i64 * PUBLIC_USAGE_LOOKUP_BUCKET_MS)
                + 30_000,
        );
        last_bucket.input_uncached_tokens = 25;
        last_bucket.input_cached_tokens = 500;
        last_bucket.output_tokens = 5;
        last_bucket.billable_tokens = 530;

        let mut ignored_old = sample_usage_event("evt-3", start_ms - 1);
        ignored_old.input_uncached_tokens = 10_000;
        ignored_old.output_tokens = 10_000;

        let points =
            build_public_usage_chart_points(&[first_bucket, last_bucket, ignored_old], now_ms);

        assert_eq!(points.len(), PUBLIC_USAGE_LOOKUP_CHART_BUCKETS);
        assert_eq!(points[0].bucket_start_ms, start_ms);
        assert_eq!(points[0].tokens, 140);
        assert_eq!(points[1].tokens, 0);
        assert_eq!(points[PUBLIC_USAGE_LOOKUP_CHART_BUCKETS - 1].tokens, 30);
    }

    #[test]
    fn validate_public_usage_lookup_key_allows_private_active_keys() {
        let active_public = sample_public_lookup_key();
        assert!(validate_public_usage_lookup_key(&active_public).is_ok());

        let mut disabled = active_public.clone();
        disabled.status = LLM_GATEWAY_KEY_STATUS_DISABLED.to_string();
        let disabled_err = validate_public_usage_lookup_key(&disabled)
            .expect_err("disabled keys must not be queryable");
        assert_eq!(disabled_err.0, StatusCode::NOT_FOUND);
        assert_eq!(disabled_err.1.error, "queryable key not found");

        let mut hidden = active_public;
        hidden.public_visible = false;
        assert!(
            validate_public_usage_lookup_key(&hidden).is_ok(),
            "private active keys should still be queryable by secret"
        );
    }

    #[test]
    fn summarize_public_multi_account_status_keeps_non_active_accounts_visible() {
        let active = sample_account_summary(
            "alpha",
            AccountStatus::Active,
            vec![sample_bucket(None, 62.0, 39.0)],
            None,
        );
        let unavailable = sample_account_summary(
            "beta",
            AccountStatus::Unavailable,
            vec![sample_bucket(None, 17.0, 5.0)],
            Some("upstream 503"),
        );

        let (accounts, buckets, status, error_message) =
            summarize_public_multi_account_status(&[active, unavailable]);

        assert_eq!(status, "ready");
        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts[0].name, "alpha");
        assert_eq!(accounts[0].status, "active");
        assert_eq!(accounts[1].name, "beta");
        assert_eq!(accounts[1].status, "unavailable");
        assert_eq!(accounts[1].usage_error_message.as_deref(), Some("upstream 503"));
        assert_eq!(buckets.len(), 1);
        assert_eq!(buckets[0].account_name.as_deref(), Some("alpha"));
        assert!(error_message.is_none());
    }

    #[test]
    fn summarize_public_multi_account_status_marks_all_non_active_accounts_as_error() {
        let unavailable = sample_account_summary(
            "alpha",
            AccountStatus::Unavailable,
            vec![sample_bucket(None, 17.0, 5.0)],
            Some("upstream 503"),
        );

        let (accounts, buckets, status, error_message) =
            summarize_public_multi_account_status(&[unavailable]);

        assert_eq!(status, "error");
        assert_eq!(accounts.len(), 1);
        assert!(buckets.is_empty());
        assert_eq!(
            error_message.as_deref(),
            Some("no active codex accounts available out of 1 configured account(s)")
        );
    }

    #[test]
    fn build_codex_failure_usage_event_preserves_status_and_diagnostic_payload() {
        let key = sample_public_lookup_key();
        let prepared = sample_prepared_gateway_request();
        let context = sample_gateway_event_context();

        let diagnostic = build_codex_failure_diagnostic_payload(
            &prepared,
            Some(&context),
            Some("acct-a"),
            "send_upstream",
            502,
            "upstream request failed",
            None,
        );
        let event = build_gateway_usage_event_record(GatewayUsageEventBuild {
            current: &key,
            prepared: &prepared,
            context: &context,
            latency_ms: 12,
            status_code: 502,
            usage: UsageBreakdown {
                usage_missing: true,
                ..UsageBreakdown::default()
            },
            last_message_content: Some(diagnostic.clone()),
            selected_account_name: Some("acct-a"),
        });

        assert_eq!(event.status_code, 502);
        assert_eq!(event.last_message_content.as_deref(), Some(diagnostic.as_str()));
        assert_eq!(event.request_headers_json, context.request_headers_json);
        assert_eq!(
            event.full_request_json,
            maybe_raw_request_body_text(&prepared.client_request_body)
        );
    }

    #[test]
    fn codex_failure_diagnostic_payload_contains_client_and_upstream_bodies() {
        let prepared = sample_prepared_gateway_request();

        let payload = build_codex_failure_diagnostic_payload(
            &prepared,
            None,
            Some("acct-a"),
            "request_validation",
            400,
            "Invalid JSON body",
            Some(json!({ "detail": "bad field" })),
        );
        let parsed: serde_json::Value = serde_json::from_str(&payload).expect("diagnostic json");

        assert_eq!(parsed["kind"], "codex_failure_diagnostic");
        assert_eq!(parsed["client_request_body"]["input"], "hello");
        assert_eq!(parsed["upstream_request_body"]["stream"], true);
        assert_eq!(parsed["account_name"], "acct-a");
        assert_eq!(parsed["status_code"], 400);
    }

    #[test]
    fn build_codex_failure_usage_event_uses_diagnostic_payload_for_429() {
        let key = sample_public_lookup_key();
        let prepared = sample_prepared_gateway_request();
        let context = sample_gateway_event_context();

        let payload = build_codex_failure_diagnostic_payload(
            &prepared,
            Some(&context),
            Some("acct-a"),
            "key_request_limit",
            429,
            "local_start_interval",
            Some(json!({ "wait_ms": 123 })),
        );
        let event = build_gateway_usage_event_record(GatewayUsageEventBuild {
            current: &key,
            prepared: &prepared,
            context: &context,
            latency_ms: 10,
            status_code: 429,
            usage: UsageBreakdown {
                usage_missing: true,
                ..UsageBreakdown::default()
            },
            last_message_content: Some(payload.clone()),
            selected_account_name: Some("acct-a"),
        });

        assert_eq!(event.status_code, 429);
        assert_eq!(event.last_message_content.as_deref(), Some(payload.as_str()));
    }

    #[test]
    fn codex_account_failover_retries_only_account_bound_non_success_responses() {
        assert_eq!(
            retryable_codex_account_failure(StatusCode::TOO_MANY_REQUESTS, Some("acct-a")),
            Some("acct-a")
        );
        assert_eq!(
            retryable_codex_account_failure(StatusCode::UNAUTHORIZED, Some("acct-a")),
            Some("acct-a")
        );
        assert_eq!(
            retryable_codex_account_failure(StatusCode::INTERNAL_SERVER_ERROR, Some("acct-a")),
            Some("acct-a")
        );
        assert_eq!(retryable_codex_account_failure(StatusCode::OK, Some("acct-a")), None);
        assert_eq!(retryable_codex_account_failure(StatusCode::TOO_MANY_REQUESTS, None), None);
    }

    #[test]
    fn build_codex_success_usage_event_uses_weighted_billable_formula_without_full_request_payloads(
    ) {
        let key = sample_public_lookup_key();
        let prepared = sample_prepared_gateway_request();
        let context = sample_gateway_event_context();

        let event = build_gateway_usage_event_record(GatewayUsageEventBuild {
            current: &key,
            prepared: &prepared,
            context: &context,
            latency_ms: 10,
            status_code: 200,
            usage: UsageBreakdown {
                input_uncached_tokens: 100,
                input_cached_tokens: 35,
                output_tokens: 7,
                usage_missing: false,
            },
            last_message_content: Some("hello".to_string()),
            selected_account_name: Some("acct-a"),
        });

        assert_eq!(event.billable_tokens, 138);
        assert_eq!(event.request_headers_json, context.request_headers_json);
        assert_eq!(event.client_request_body_json, None);
        assert_eq!(event.upstream_request_body_json, None);
        assert_eq!(event.full_request_json, None);
    }

    #[test]
    fn build_codex_usage_event_preserves_gateway_latency_metrics() {
        let key = sample_public_lookup_key();
        let prepared = sample_prepared_gateway_request();
        let mut context = sample_gateway_event_context();
        context.routing_wait_ms = Some(11);
        context.upstream_headers_ms = Some(29);
        context.post_headers_body_ms = Some(41);
        context.request_body_bytes = Some(512);
        context.request_body_read_ms = Some(3);
        context.request_json_parse_ms = Some(5);
        context.pre_handler_ms = Some(2);
        context.first_sse_write_ms = Some(37);
        context.stream_finish_ms = Some(83);
        context.routing_diagnostics_json = Some(r#"{"route_total_ms":11}"#.to_string());

        let event = build_gateway_usage_event_record(GatewayUsageEventBuild {
            current: &key,
            prepared: &prepared,
            context: &context,
            latency_ms: 90,
            status_code: 200,
            usage: UsageBreakdown::default(),
            last_message_content: Some("hello".to_string()),
            selected_account_name: Some("acct-a"),
        });

        assert_eq!(event.routing_wait_ms, Some(11));
        assert_eq!(event.upstream_headers_ms, Some(29));
        assert_eq!(event.post_headers_body_ms, Some(41));
        assert_eq!(event.request_body_bytes, Some(512));
        assert_eq!(event.request_body_read_ms, Some(3));
        assert_eq!(event.request_json_parse_ms, Some(5));
        assert_eq!(event.pre_handler_ms, Some(2));
        assert_eq!(event.first_sse_write_ms, Some(37));
        assert_eq!(event.stream_finish_ms, Some(83));
        assert_eq!(event.routing_diagnostics_json.as_deref(), Some(r#"{"route_total_ms":11}"#));
    }

    #[test]
    fn build_codex_failure_diagnostic_payload_preserves_stream_failure_status() {
        let mut prepared = sample_prepared_gateway_request();
        prepared.wants_stream = true;
        prepared.force_upstream_stream = false;

        let payload = build_codex_failure_diagnostic_payload(
            &prepared,
            None,
            Some("acct-a"),
            "stream_read",
            599,
            "failed to parse upstream SSE event",
            Some(json!({ "stream_kind": "sse" })),
        );
        let parsed: serde_json::Value = serde_json::from_str(&payload).expect("diagnostic json");

        assert_eq!(parsed["status_code"], 599);
        assert_eq!(parsed["failure_stage"], "stream_read");
    }

    #[test]
    fn update_runtime_config_rejects_invalid_refresh_ranges() {
        let err = validate_runtime_refresh_window(301, 300).expect_err("min > max should fail");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);

        let err = validate_runtime_refresh_window(239, 300).expect_err("too-small min should fail");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn parse_kiro_cache_kmodels_json_rejects_non_positive_entries() {
        let err = parse_kiro_cache_kmodels_json(
            r#"{"claude-opus-4-6":0,"claude-sonnet-4-6":5.055065250835128e-06}"#,
        )
        .expect_err("zero coefficient should fail");

        assert!(err.to_string().contains("claude-opus-4-6"));
    }

    #[test]
    fn update_runtime_config_rejects_invalid_kiro_cache_policy_json() {
        let request: UpdateLlmGatewayRuntimeConfigRequest = serde_json::from_value(json!({
            "kiro_cache_policy_json": "{\"prefix_tree_credit_ratio_bands\":[{\"credit_start\":1.0,\"credit_end\":0.5,\"cache_ratio_start\":0.2,\"cache_ratio_end\":0.1}]}"
        }))
        .expect("parse runtime config request");

        let err = parse_kiro_cache_policy_json(
            request
                .kiro_cache_policy_json
                .as_deref()
                .expect("request should contain policy json"),
        )
        .map_err(|_| bad_request("kiro_cache_policy_json is invalid"))
        .expect_err("invalid policy json should fail");

        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn update_runtime_config_request_parses_new_kiro_prefix_cache_fields() {
        let request: UpdateLlmGatewayRuntimeConfigRequest = serde_json::from_value(json!({
            "kiro_prefix_cache_mode": "prefix_tree",
            "kiro_prefix_cache_max_tokens": 4096,
            "kiro_prefix_cache_entry_ttl_seconds": 1800,
            "kiro_conversation_anchor_max_entries": 128,
            "kiro_conversation_anchor_ttl_seconds": 3600
        }))
        .expect("parse runtime config request");

        assert_eq!(request.kiro_prefix_cache_mode.as_deref(), Some("prefix_tree"));
        assert_eq!(request.kiro_prefix_cache_max_tokens, Some(4096));
        assert_eq!(request.kiro_prefix_cache_entry_ttl_seconds, Some(1800));
        assert_eq!(request.kiro_conversation_anchor_max_entries, Some(128));
        assert_eq!(request.kiro_conversation_anchor_ttl_seconds, Some(3600));
    }

    #[test]
    fn codex_client_version_normalization_accepts_trimmed_semver() {
        assert_eq!(normalize_codex_client_version(" 0.124.0 "), Some("0.124.0".to_string()));
        assert_eq!(
            resolve_codex_client_version(Some("invalid version?")),
            DEFAULT_CODEX_CLIENT_VERSION
        );
    }

    #[test]
    fn validate_runtime_config_rejects_invalid_kiro_prefix_cache_mode() {
        let err = validate_kiro_prefix_cache_mode("invalid").expect_err("mode should be rejected");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn validate_runtime_config_rejects_non_positive_kiro_limits() {
        let err = validate_positive_u64("kiro_prefix_cache_max_tokens", 0)
            .expect_err("zero max tokens should fail");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);

        let err = validate_positive_u64("kiro_conversation_anchor_max_entries", 0)
            .expect_err("zero max entries should fail");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test(start_paused = true)]
    async fn public_rate_limit_refresh_interval_waits_full_period_before_first_tick() {
        let mut ticker = public_rate_limit_refresh_interval();
        let mut tick = std::pin::pin!(ticker.tick());

        assert!(matches!(futures_util::poll!(tick.as_mut()), std::task::Poll::Pending));
        tokio::time::advance(Duration::from_secs(PUBLIC_RATE_LIMIT_REFRESH_SECONDS - 1)).await;
        assert!(matches!(futures_util::poll!(tick.as_mut()), std::task::Poll::Pending));
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(matches!(futures_util::poll!(tick.as_mut()), std::task::Poll::Ready(_)));
    }
}
