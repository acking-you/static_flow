//! LLM gateway orchestration layer.
//!
//! The public gateway intentionally keeps request normalization, upstream
//! transport, response adaptation, and runtime cache management in separate
//! modules. This file owns the top-level handlers and the proxy control flow
//! so the routing layer only needs to depend on one coherent module.

mod models;
mod request;
mod response;
mod runtime;
mod support;
mod types;

pub(crate) mod accounts;
pub(crate) mod token_refresh;

use std::{
    env,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use async_stream::stream;
use axum::{
    body::{Body, Bytes},
    extract::{Request, State},
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
    now_ms, LlmGatewayKeyRecord, LlmGatewayRuntimeConfigRecord, LlmGatewayUsageEventRecord,
    NewLlmGatewayAccountContributionRequestInput, NewLlmGatewaySponsorRequestInput,
    NewLlmGatewayTokenRequestInput, LLM_GATEWAY_KEY_STATUS_ACTIVE, LLM_GATEWAY_KEY_STATUS_DISABLED,
    LLM_GATEWAY_SPONSOR_REQUEST_STATUS_APPROVED,
    LLM_GATEWAY_SPONSOR_REQUEST_STATUS_PAYMENT_EMAIL_SENT, LLM_GATEWAY_TOKEN_REQUEST_STATUS_FAILED,
    LLM_GATEWAY_TOKEN_REQUEST_STATUS_ISSUED, LLM_GATEWAY_TOKEN_REQUEST_STATUS_PENDING,
    LLM_GATEWAY_TOKEN_REQUEST_STATUS_REJECTED,
};

pub use self::runtime::LlmGatewayRuntimeState;
pub(crate) use self::{
    accounts::{resolve_auths_dir, AccountPool},
    support::{load_support_asset, load_support_config, render_payment_email_markdown},
    token_refresh::{build_refresh_client, spawn_account_refresh_task},
};
use self::{
    models::respond_local_models,
    request::{
        apply_gpt53_codex_spark_mapping, ensure_supported_gateway_path, external_origin,
        extract_last_message_content, extract_presented_key, normalize_name, normalize_status,
        normalize_upstream_base_url, prepare_gateway_request as normalize_gateway_request,
    },
    response::{
        adapt_completed_response_json, apply_upstream_response_headers,
        convert_json_response_to_chat_completion, convert_response_event_to_chat_chunk,
        encode_json_sse_chunk, encode_sse_event_with_model_alias, extract_usage_from_bytes,
        rewrite_json_response_model_alias, SseUsageCollector,
    },
    runtime::{bearer_header, gateway_auth_cache_ttl, CachedKeyLease, CodexAuthSnapshot},
    types::{
        AccountListResponse, AccountSummaryView, AdminLlmGatewayAccountContributionRequestQuery,
        AdminLlmGatewayAccountContributionRequestView,
        AdminLlmGatewayAccountContributionRequestsResponse, AdminLlmGatewayKeyView,
        AdminLlmGatewayKeysResponse, AdminLlmGatewaySponsorRequestQuery,
        AdminLlmGatewaySponsorRequestView, AdminLlmGatewaySponsorRequestsResponse,
        AdminLlmGatewayTokenRequestQuery, AdminLlmGatewayTokenRequestView,
        AdminLlmGatewayTokenRequestsResponse, AdminLlmGatewayUsageEventView,
        AdminLlmGatewayUsageEventsResponse, AdminLlmGatewayUsageQuery, CreateLlmGatewayKeyRequest,
        GatewayResponseAdapter, ImportAccountRequest, LlmGatewayAccessResponse,
        LlmGatewayCreditsView, LlmGatewayEventContext, LlmGatewayPublicKeyView,
        LlmGatewayRateLimitBucketView, LlmGatewayRateLimitStatusResponse,
        LlmGatewayRateLimitWindowView, LlmGatewayRuntimeConfigResponse,
        LlmGatewaySupportConfigView, PatchAccountSettingsRequest, PatchLlmGatewayKeyRequest,
        PreparedGatewayRequest, PublicLlmGatewayAccountContributionView,
        PublicLlmGatewayAccountContributionsResponse, PublicLlmGatewaySponsorView,
        PublicLlmGatewaySponsorsResponse, SubmitLlmGatewayAccountContributionRequest,
        SubmitLlmGatewayAccountContributionRequestResponse, SubmitLlmGatewaySponsorRequest,
        SubmitLlmGatewaySponsorRequestResponse, SubmitLlmGatewayTokenRequest,
        SubmitLlmGatewayTokenRequestResponse, UpdateLlmGatewayRuntimeConfigRequest, UsageBreakdown,
    },
};
use crate::{
    email::{
        build_llm_access_url, build_llm_gateway_base_url, normalize_frontend_page_url_input,
        normalize_requester_email_input,
    },
    handlers::{
        build_client_fingerprint, build_submit_rate_limit_key, enforce_comment_submit_rate_limit,
        ensure_admin_access, extract_client_ip, generate_task_id, AdminTaskActionRequest,
        ErrorResponse,
    },
    state::{AppState, LlmGatewayRuntimeConfig},
};

const DEFAULT_UPSTREAM_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
const DEFAULT_WIRE_ORIGINATOR: &str = "codex_cli_rs";
const DEFAULT_CODEX_CLI_VERSION: &str = "0.116.0";
const FAST_BILLABLE_MULTIPLIER: u64 = 2;
const MAX_GATEWAY_BODY_BYTES: usize = 8 * 1024 * 1024;
const MAX_RUNTIME_CACHE_TTL_SECONDS: u64 = 86_400;
const MIN_RUNTIME_CACHE_TTL_SECONDS: u64 = 1;
const MAX_OPENAI_TOOL_NAME_LEN: usize = 64;
const PUBLIC_RATE_LIMIT_REFRESH_SECONDS: u64 = 60;
const LAST_MESSAGE_CONTENT_EXTRACT_FAILED: &str = "[extract_failed]";
const MAX_PUBLIC_TOKEN_WISH_REASON_CHARS: usize = 4000;
const MAX_PUBLIC_TOKEN_WISH_QUOTA: u64 = 100_000_000_000;
const MAX_PUBLIC_ACCOUNT_CONTRIBUTION_MESSAGE_CHARS: usize = 4000;
const MAX_PUBLIC_ACCOUNT_CONTRIBUTION_GITHUB_ID_CHARS: usize = 39;
const MAX_PUBLIC_ACCOUNT_CONTRIBUTIONS: usize = 24;
const MAX_PUBLIC_SPONSOR_MESSAGE_CHARS: usize = 4000;
const MAX_PUBLIC_SPONSOR_DISPLAY_NAME_CHARS: usize = 80;
const MAX_PUBLIC_SPONSORS: usize = 36;
pub(super) const GPT53_CODEX_MODEL_ID: &str = "gpt-5.3-codex";
pub(super) const GPT53_CODEX_SPARK_MODEL_ID: &str = "gpt-5.3-codex-spark";

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
    let config = state.llm_gateway_runtime_config.read().await.clone();
    let keys = state
        .llm_gateway_store
        .list_public_keys()
        .await
        .map_err(|err| internal_error("Failed to list public gateway keys", err))?;
    let gateway_path = "/api/llm-gateway/v1".to_string();
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
        auth_cache_ttl_seconds: config.auth_cache_ttl_seconds,
        keys: keys.iter().map(LlmGatewayPublicKeyView::from).collect(),
        generated_at: now_ms(),
    }))
}

/// Serve the cached Codex account rate-limit snapshot without hitting the
/// upstream backend on every request.
pub async fn get_public_rate_limit_status(
    State(state): State<AppState>,
) -> Result<Json<LlmGatewayRateLimitStatusResponse>, (StatusCode, Json<ErrorResponse>)> {
    let snapshot = state.llm_gateway.rate_limit_status.read().await.clone();
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
    let config = state.llm_gateway_runtime_config.read().await.clone();
    Ok(Json(LlmGatewayRuntimeConfigResponse {
        auth_cache_ttl_seconds: config.auth_cache_ttl_seconds,
    }))
}

/// Persist admin-controlled runtime gateway configuration changes.
pub async fn update_admin_runtime_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<UpdateLlmGatewayRuntimeConfigRequest>,
) -> Result<Json<LlmGatewayRuntimeConfigResponse>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let ttl = request
        .auth_cache_ttl_seconds
        .ok_or_else(|| bad_request("auth_cache_ttl_seconds is required"))?;
    if !(MIN_RUNTIME_CACHE_TTL_SECONDS..=MAX_RUNTIME_CACHE_TTL_SECONDS).contains(&ttl) {
        return Err(bad_request("auth_cache_ttl_seconds is out of range"));
    }

    let config = LlmGatewayRuntimeConfigRecord {
        id: "default".to_string(),
        auth_cache_ttl_seconds: ttl,
        updated_at: now_ms(),
    };
    state
        .llm_gateway_store
        .upsert_runtime_config(&config)
        .await
        .map_err(|err| internal_error("Failed to update llm gateway config", err))?;
    {
        let mut runtime = state.llm_gateway_runtime_config.write().await;
        *runtime = LlmGatewayRuntimeConfig {
            auth_cache_ttl_seconds: ttl,
        };
    }

    tracing::info!(auth_cache_ttl_seconds = ttl, "Updated LLM gateway runtime config");

    Ok(Json(LlmGatewayRuntimeConfigResponse {
        auth_cache_ttl_seconds: ttl,
    }))
}

/// List all managed keys for the admin inventory screen.
pub async fn list_admin_keys(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<AdminLlmGatewayKeysResponse>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let keys = state
        .llm_gateway_store
        .list_keys()
        .await
        .map_err(|err| internal_error("Failed to list llm gateway keys", err))?;
    let config = state.llm_gateway_runtime_config.read().await.clone();

    tracing::debug!(key_count = keys.len(), "Listed admin LLM gateway keys");

    Ok(Json(AdminLlmGatewayKeysResponse {
        keys: keys.iter().map(AdminLlmGatewayKeyView::from).collect(),
        auth_cache_ttl_seconds: config.auth_cache_ttl_seconds,
        generated_at: now_ms(),
    }))
}

/// Create a new admin-managed key and warm it into the in-memory auth cache.
pub async fn create_admin_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateLlmGatewayKeyRequest>,
) -> Result<Json<AdminLlmGatewayKeyView>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let name = normalize_name(&request.name)?;
    let record = create_managed_key_record(
        &state,
        name,
        request.quota_billable_limit,
        request.public_visible,
        None,
        None,
    )
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

async fn create_managed_key_record(
    state: &AppState,
    name: String,
    quota_billable_limit: u64,
    public_visible: bool,
    route_strategy: Option<String>,
    fixed_account_name: Option<String>,
) -> Result<LlmGatewayKeyRecord, (StatusCode, Json<ErrorResponse>)> {
    let secret = generate_secret();
    let key_hash = sha256_hex(secret.as_bytes());
    let now = now_ms();
    let record = LlmGatewayKeyRecord {
        id: generate_id("llm-key"),
        name,
        secret,
        key_hash: key_hash.clone(),
        status: LLM_GATEWAY_KEY_STATUS_ACTIVE.to_string(),
        public_visible,
        quota_billable_limit,
        usage_input_uncached_tokens: 0,
        usage_input_cached_tokens: 0,
        usage_output_tokens: 0,
        usage_billable_tokens: 0,
        last_used_at: None,
        created_at: now,
        updated_at: now,
        route_strategy,
        fixed_account_name,
    };
    state
        .llm_gateway_store
        .upsert_key(&record)
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
        key.route_strategy = if strategy.is_empty() { None } else { Some(strategy.to_string()) };
    }
    if let Some(account_name) = request.fixed_account_name.as_deref() {
        key.fixed_account_name =
            if account_name.is_empty() { None } else { Some(account_name.to_string()) };
    }
    key.updated_at = now_ms();
    state
        .llm_gateway_store
        .upsert_key(&key)
        .await
        .map_err(|err| internal_error("Failed to update llm gateway key", err))?;

    if key.status == LLM_GATEWAY_KEY_STATUS_ACTIVE {
        let ttl = current_cache_ttl(&state).await;
        state
            .llm_gateway
            .key_cache
            .renew(key.clone(), Duration::from_secs(ttl));
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

    Ok(Json(AdminLlmGatewayKeyView::from(&key)))
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
        .count_usage_events(query.key_id.as_deref())
        .await
        .map_err(|err| internal_error("Failed to count llm gateway usage events", err))?;
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
            events: vec![],
            generated_at: now_ms(),
        }));
    }

    let fetch_count = (total - offset).min(limit);
    let reverse_offset = total.saturating_sub(offset.saturating_add(fetch_count));
    let mut events = state
        .llm_gateway_store
        .query_usage_events(query.key_id.as_deref(), Some(fetch_count), Some(reverse_offset))
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
        events: events
            .iter()
            .map(AdminLlmGatewayUsageEventView::from)
            .collect(),
        generated_at: now_ms(),
    }))
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
    enforce_comment_submit_rate_limit(
        state.llm_gateway_token_request_submit_guard.as_ref(),
        &rate_limit_key,
        now_ms(),
        60,
    )
    .await?;

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
    enforce_comment_submit_rate_limit(
        state.llm_gateway_token_request_submit_guard.as_ref(),
        &rate_limit_key,
        now_ms(),
        60,
    )
    .await?;

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
    enforce_comment_submit_rate_limit(
        state.llm_gateway_token_request_submit_guard.as_ref(),
        &rate_limit_key,
        now_ms(),
        60,
    )
    .await?;

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
        create_managed_key_record(
            &state,
            key_name,
            token_request.requested_quota_billable_limit,
            false,
            None,
            None,
        )
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
    let usage = match token_refresh::validate_account_usage(&state.llm_gateway.client, &auth).await
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
                create_managed_key_record(
                    &state,
                    key_name,
                    100_000_000_000,
                    false,
                    Some("fixed".to_string()),
                    Some(imported_account_name.clone()),
                )
                .await?
            },
        }
    } else {
        let key_name = normalize_name(&format!("contrib-{}", contribution_request.request_id))?;
        create_managed_key_record(
            &state,
            key_name,
            100_000_000_000,
            false,
            Some("fixed".to_string()),
            Some(imported_account_name.clone()),
        )
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
        let mut ticker =
            tokio::time::interval(Duration::from_secs(PUBLIC_RATE_LIMIT_REFRESH_SECONDS));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        ticker.tick().await;

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
    let refresh_interval_seconds = PUBLIC_RATE_LIMIT_REFRESH_SECONDS;
    let source_url = compute_rate_limit_status_url();

    let pool_entries = runtime.account_pool.all_entries().await;

    let result: Result<(Vec<LlmGatewayRateLimitBucketView>, Option<String>)> =
        if pool_entries.is_empty() {
            // Legacy single-file path — one upstream request.
            fetch_rate_limit_status_snapshot(runtime, &source_url)
                .await
                .map(|buckets| (buckets, None::<String>))
        } else {
            // Multi-account: read the already-cached per-account snapshots kept
            // fresh by the background refresh task instead of hitting upstream.
            let summaries = runtime.account_pool.list_summaries().await;
            let mut all_buckets = Vec::new();
            for summary in &summaries {
                if summary.status.as_str() != "active" {
                    continue;
                }
                for mut bucket in summary.rate_limits.buckets.clone() {
                    bucket.account_name = Some(summary.name.clone());
                    all_buckets.push(bucket);
                }
            }
            Ok((all_buckets, None))
        };

    match result {
        Ok((buckets, partial_error)) => {
            let mut status = runtime.rate_limit_status.write().await;
            *status = LlmGatewayRateLimitStatusResponse {
                status: if partial_error.is_some() {
                    "degraded".to_string()
                } else {
                    "ready".to_string()
                },
                refresh_interval_seconds,
                last_checked_at: Some(checked_at),
                last_success_at: Some(checked_at),
                source_url,
                error_message: partial_error,
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
            let mut status = runtime.rate_limit_status.write().await;
            let had_snapshot = !status.buckets.is_empty();
            let previous_success_at = status.last_success_at;
            status.status = if had_snapshot { "degraded".to_string() } else { "error".to_string() };
            status.refresh_interval_seconds = refresh_interval_seconds;
            status.last_checked_at = Some(checked_at);
            status.last_success_at = previous_success_at;
            status.source_url = source_url;
            status.error_message = Some(err.to_string());
            tracing::warn!(
                had_snapshot,
                last_success_at = previous_success_at.unwrap_or_default(),
                "Failed to refresh cached public LLM gateway rate-limit status: {err:#}"
            );
            Err(err)
        },
    }
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
    let event_context = parts.extensions.get::<LlmGatewayEventContext>().cloned();
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

    let (auth_snapshot, selected_account_name, map_gpt53_codex_to_spark) =
        resolve_auth_for_key(&state, &key_lease.record).await?;

    if request::is_models_path(&gateway_path) {
        return respond_local_models(
            &state,
            &auth_snapshot,
            &parts.headers,
            &query,
            map_gpt53_codex_to_spark,
        )
        .await;
    }

    let prepared =
        normalize_gateway_request(&gateway_path, &query, parts.method, &parts.headers, body)
            .await?;
    let prepared = apply_gpt53_codex_spark_mapping(&prepared, map_gpt53_codex_to_spark)?;

    let response = send_upstream_with_retry(&state, &prepared, &parts.headers, &auth_snapshot)
        .await
        .map_err(|err| internal_error("Failed to proxy llm gateway request", err))?;

    forward_upstream_response(
        state,
        key_lease,
        prepared,
        response,
        event_context,
        selected_account_name,
    )
    .await
}

/// Validate the presented key via cache first, then fall back to LanceDB.
async fn validate_gateway_key(
    state: &AppState,
    key_hash: &str,
) -> Result<Arc<CachedKeyLease>, (StatusCode, Json<ErrorResponse>)> {
    if let Some(cached) = state.llm_gateway.key_cache.get(key_hash) {
        tracing::debug!(key_hash, "LLM gateway key cache hit");
        validate_cached_key(&cached.record)?;
        let ttl = current_cache_ttl(state).await;
        return Ok(state
            .llm_gateway
            .key_cache
            .renew(cached.record.clone(), Duration::from_secs(ttl)));
    }

    tracing::debug!(key_hash, "LLM gateway key cache miss");
    let key = state
        .llm_gateway_store
        .get_key_by_hash(key_hash)
        .await
        .map_err(|err| internal_error("Failed to validate llm gateway key", err))?
        .ok_or_else(|| auth_error(StatusCode::FORBIDDEN, "invalid api key"))?;
    validate_cached_key(&key)?;
    let ttl = current_cache_ttl(state).await;
    Ok(state
        .llm_gateway
        .key_cache
        .renew(key, Duration::from_secs(ttl)))
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
/// Routing order:
/// 1. `fixed` + `fixed_account_name` → use that specific account from the pool.
/// 2. `auto` (or unset) → pick the active account with the most remaining
///    quota.
/// 3. Fallback → if the pool is empty, use the legacy single-file
///    `CodexAuthSource`.
async fn resolve_auth_for_key(
    state: &AppState,
    key: &LlmGatewayKeyRecord,
) -> Result<(CodexAuthSnapshot, Option<String>, bool), (StatusCode, Json<ErrorResponse>)> {
    let pool = &state.llm_gateway.account_pool;
    let strategy = key.route_strategy.as_deref().unwrap_or("auto");

    match strategy {
        "fixed" => {
            let name = key.fixed_account_name.as_deref().unwrap_or("");
            if name.is_empty() {
                return Err(bad_request("fixed route_strategy requires fixed_account_name"));
            }
            pool.get_account(name)
                .await
                .map(|(snapshot, map_gpt53_codex_to_spark)| {
                    (snapshot, Some(name.to_string()), map_gpt53_codex_to_spark)
                })
                .ok_or_else(|| {
                    auth_error(
                        StatusCode::SERVICE_UNAVAILABLE,
                        &format!("bound account `{name}` is unavailable"),
                    )
                })
        },
        _ => {
            // auto: best available account by remaining quota, or legacy fallback.
            if let Some((name, snapshot, map_gpt53_codex_to_spark)) =
                pool.select_best_account().await
            {
                return Ok((snapshot, Some(name), map_gpt53_codex_to_spark));
            }
            // Fallback to single-file CodexAuthSource when pool is empty.
            state
                .llm_gateway
                .auth_source
                .current()
                .await
                .map(|snapshot| (snapshot, None, false))
                .map_err(|err| internal_error("no accounts available and legacy auth failed", err))
        },
    }
}

/// Read the live auth-cache TTL from the runtime config lock.
async fn current_cache_ttl(state: &AppState) -> u64 {
    state
        .llm_gateway_runtime_config
        .read()
        .await
        .auth_cache_ttl_seconds
}

// === Upstream transport ===

/// Retry once with a forced auth reload if the upstream rejects stale
/// credentials.
async fn send_upstream_with_retry(
    state: &AppState,
    prepared: &PreparedGatewayRequest,
    incoming_headers: &HeaderMap,
    auth_snapshot: &CodexAuthSnapshot,
) -> Result<reqwest::Response> {
    let first = send_upstream(state, prepared, incoming_headers, auth_snapshot).await?;
    if first.status() != StatusCode::UNAUTHORIZED {
        return Ok(first);
    }

    tracing::warn!(
        upstream_path = prepared.upstream_path,
        "Upstream returned 401, forcing Codex auth reload"
    );

    let refreshed = state.llm_gateway.auth_source.force_reload().await?;
    send_upstream(state, prepared, incoming_headers, &refreshed).await
}

/// Build the exact upstream HTTP request to the Codex backend.
async fn send_upstream(
    state: &AppState,
    prepared: &PreparedGatewayRequest,
    incoming_headers: &HeaderMap,
    auth_snapshot: &CodexAuthSnapshot,
) -> Result<reqwest::Response> {
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
    let effective_user_agent = incoming_user_agent.unwrap_or_else(codex_user_agent);
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
    if let Some(account_id) = auth_snapshot.account_id.as_deref() {
        headers.insert(
            reqwest::header::HeaderName::from_static("chatgpt-account-id"),
            ReqwestHeaderValue::from_str(account_id)?,
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

    let mut request_builder = state
        .llm_gateway
        .client
        .request(prepared.method.clone(), upstream_url)
        .headers(headers);
    if !prepared.request_body.is_empty() {
        request_builder = request_builder.body(prepared.request_body.clone());
    }

    request_builder
        .send()
        .await
        .context("upstream request failed")
}

// === Downstream response adaptation ===

/// Adapt the upstream response back into the caller's requested wire format.
async fn forward_upstream_response(
    state: AppState,
    key_lease: Arc<CachedKeyLease>,
    prepared: PreparedGatewayRequest,
    upstream: reqwest::Response,
    event_context: Option<LlmGatewayEventContext>,
    selected_account_name: Option<String>,
) -> Result<Response, (StatusCode, Json<ErrorResponse>)> {
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
            while let Some(event) = events.next().await {
                let event = event.map_err(|err| {
                    internal_error("Failed to parse llm gateway upstream SSE stream", err)
                })?;
                collector.observe_event(&event);
            }
            let usage = collector.usage.unwrap_or(UsageBreakdown {
                usage_missing: true,
                ..UsageBreakdown::default()
            });
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

            let response_json = collector.completed_response.ok_or_else(|| {
                internal_error(
                    "Failed to aggregate llm gateway response",
                    "response.completed event missing",
                )
            })?;
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
            let body = serde_json::to_vec(&adapted_json).map_err(|err| {
                internal_error("Failed to encode aggregated llm gateway response", err)
            })?;
            let builder = Response::builder()
                .status(status)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::CACHE_CONTROL, "no-store");
            return apply_upstream_response_headers(builder, &upstream_headers)
                .body(Body::from(body))
                .map_err(|err| {
                    internal_error("Failed to build aggregated llm gateway response", err)
                });
        }

        let gateway = state.llm_gateway.clone();
        let stream_key_lease = key_lease.clone();
        let stream_response_adapter = response_adapter;
        let body_stream = stream! {
            let mut collector = SseUsageCollector::default();
            let mut chat_metadata = types::ChatStreamMetadata::default();
            let mut events = upstream
                .bytes_stream()
                .map_err(std::io::Error::other)
                .eventsource();
            while let Some(event) = events.next().await {
                match event {
                    Ok(event) => {
                        collector.observe_event(&event);
                        match stream_response_adapter {
                            GatewayResponseAdapter::Responses => {
                                yield Ok::<Bytes, std::io::Error>(encode_sse_event_with_model_alias(
                                    &event,
                                    prepared.model.as_deref(),
                                    prepared.client_visible_model.as_deref(),
                                ));
                            }
                            GatewayResponseAdapter::ChatCompletions => {
                                if let Some(chunk) = convert_response_event_to_chat_chunk(
                                    &event,
                                    Some(&prepared.tool_name_restore_map),
                                    &mut chat_metadata,
                                    prepared.model.as_deref(),
                                    prepared.client_visible_model.as_deref(),
                                ) {
                                    yield Ok::<Bytes, std::io::Error>(encode_json_sse_chunk(&chunk));
                                }
                            }
                        }
                    }
                    Err(err) => {
                        yield Err(std::io::Error::other(format!(
                            "failed to parse upstream SSE event: {err}"
                        )));
                        return;
                    }
                }
            }
            let usage = collector.usage.unwrap_or(UsageBreakdown {
                usage_missing: true,
                ..UsageBreakdown::default()
            });
            if let Err(err) = persist_gateway_usage(
                gateway.as_ref(),
                stream_key_lease.as_ref(),
                &prepared,
                status.as_u16(),
                usage,
                event_context.clone(),
                selected_account_name.as_deref(),
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
        return apply_upstream_response_headers(builder, &upstream_headers)
            .body(Body::from_stream(body_stream))
            .map_err(|err| internal_error("Failed to build llm gateway stream response", err));
    }

    let body_bytes = upstream
        .bytes()
        .await
        .map_err(|err| internal_error("Failed to read llm gateway upstream response", err))?;
    let usage = if status.is_success() {
        extract_usage_from_bytes(&body_bytes).unwrap_or(UsageBreakdown {
            usage_missing: true,
            ..UsageBreakdown::default()
        })
    } else {
        UsageBreakdown {
            usage_missing: true,
            ..UsageBreakdown::default()
        }
    };

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

    let aliased_body_bytes = rewrite_json_response_model_alias(
        &body_bytes,
        prepared.model.as_deref(),
        prepared.client_visible_model.as_deref(),
    )
    .unwrap_or_else(|| body_bytes.to_vec());

    let response_bytes =
        if status.is_success() && response_adapter == GatewayResponseAdapter::ChatCompletions {
            convert_json_response_to_chat_completion(
                &body_bytes,
                Some(&prepared.tool_name_restore_map),
                prepared.model.as_deref(),
                prepared.client_visible_model.as_deref(),
            )
            .map_err(|err| {
                internal_error("Failed to adapt upstream response to chat.completions", err)
            })?
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
    apply_upstream_response_headers(builder, &upstream_headers)
        .body(Body::from(response_bytes))
        .map_err(|err| internal_error("Failed to build llm gateway response", err))
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
    let _guard = gateway.usage_write_lock.lock().await;
    let current = gateway
        .store
        .get_key_by_id(&cached_key.record.id)
        .await?
        .unwrap_or_else(|| cached_key.record.clone());
    let context = event_context.unwrap_or_else(|| LlmGatewayEventContext {
        request_method: prepared.method.as_str().to_string(),
        request_url: prepared.original_path.clone(),
        client_ip: "unknown".to_string(),
        ip_region: "Unknown".to_string(),
        request_headers_json: "{}".to_string(),
        started_at: Instant::now(),
    });
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
    let last_message_content = match extract_last_message_content(&prepared.request_body) {
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
    let event = LlmGatewayUsageEventRecord {
        id: generate_id("llm-usage"),
        key_id: current.id.clone(),
        key_name: current.name.clone(),
        account_name: selected_account_name
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        request_method: context.request_method,
        request_url: context.request_url,
        latency_ms,
        endpoint: prepared.upstream_path.clone(),
        model: prepared.model.clone(),
        status_code: status_code as i32,
        input_uncached_tokens: usage.input_uncached_tokens,
        input_cached_tokens: usage.input_cached_tokens,
        output_tokens: usage.output_tokens,
        billable_tokens: usage.billable_tokens_with_multiplier(prepared.billable_multiplier),
        usage_missing: usage.usage_missing,
        client_ip: context.client_ip,
        ip_region: context.ip_region,
        request_headers_json: context.request_headers_json,
        last_message_content,
        created_at: now_ms(),
    };
    let updated = gateway.store.apply_usage_event(&current, &event).await?;

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
    let mut request = runtime
        .client
        .get(source_url)
        .header(reqwest::header::USER_AGENT, codex_user_agent())
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
fn codex_user_agent() -> String {
    format!("{DEFAULT_WIRE_ORIGINATOR}/{DEFAULT_CODEX_CLI_VERSION}")
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
    let usage = token_refresh::validate_account_usage(&state.llm_gateway.client, &auth)
        .await
        .map_err(|err| bad_request(&format!("account verification failed: {err}")))?;

    let account = accounts::CodexAccount {
        name: name.clone(),
        access_token,
        account_id,
        refresh_token,
        id_token,
        map_gpt53_codex_to_spark: false,
        last_refresh: Some(chrono::Utc::now()),
        status: accounts::AccountStatus::Active,
    };
    pool.insert(account)
        .await
        .map_err(|err| internal_error("Failed to persist account", err))?;
    pool.update_rate_limit(&name, usage.clone()).await;

    tracing::info!(account = name, "Imported Codex account into gateway pool");

    Ok(Json(AccountSummaryView {
        name,
        status: "active".to_string(),
        account_id: request
            .tokens
            .account_id
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string),
        plan_type: usage.primary_plan_type(),
        primary_remaining_percent: usage.primary_remaining_percent(),
        secondary_remaining_percent: usage.secondary_remaining_percent(),
        map_gpt53_codex_to_spark: false,
        last_refresh: Some(now_ms()),
    }))
}

/// List all managed Codex accounts in the pool.
pub async fn list_accounts(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<AccountListResponse>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let summaries = state.llm_gateway.account_pool.list_summaries().await;
    let accounts = summaries
        .into_iter()
        .map(|summary| AccountSummaryView {
            name: summary.name,
            status: summary.status.as_str().to_string(),
            account_id: summary.account_id,
            plan_type: summary.rate_limits.primary_plan_type(),
            primary_remaining_percent: summary.rate_limits.primary_remaining_percent(),
            secondary_remaining_percent: summary.rate_limits.secondary_remaining_percent(),
            map_gpt53_codex_to_spark: summary.map_gpt53_codex_to_spark,
            last_refresh: summary
                .rate_limits
                .last_checked_at
                .or(summary.last_refresh_ms),
        })
        .collect();
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
    let Some(enabled) = request.map_gpt53_codex_to_spark else {
        return Err(bad_request("map_gpt53_codex_to_spark is required"));
    };

    let summaries = state.llm_gateway.account_pool.list_summaries().await;
    let current = summaries
        .iter()
        .find(|summary| summary.name == name)
        .ok_or_else(|| not_found("account not found"))?;
    if enabled && !current.rate_limits.is_gpt_pro() {
        return Err(bad_request("Spark mapping is only available for accounts with plan_type=Pro"));
    }

    let updated = state
        .llm_gateway
        .account_pool
        .set_map_gpt53_codex_to_spark(&name, enabled)
        .await
        .map_err(|err| internal_error("Failed to update account settings", err))?;
    if !updated {
        return Err(not_found("account not found"));
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
        "Updated Codex account settings"
    );

    Ok(Json(AccountSummaryView {
        name: summary.name,
        status: summary.status.as_str().to_string(),
        account_id: summary.account_id,
        plan_type: summary.rate_limits.primary_plan_type(),
        primary_remaining_percent: summary.rate_limits.primary_remaining_percent(),
        secondary_remaining_percent: summary.rate_limits.secondary_remaining_percent(),
        map_gpt53_codex_to_spark: summary.map_gpt53_codex_to_spark,
        last_refresh: summary
            .rate_limits
            .last_checked_at
            .or(summary.last_refresh_ms),
    }))
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
