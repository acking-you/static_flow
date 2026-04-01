//! Kiro Gateway module — API key authentication, account management,
//! usage tracking, and admin CRUD for the Kiro provider backend.

pub(crate) mod auth_file;
mod local_import;
pub(crate) mod machine_id;
pub mod parser;
mod provider;
mod runtime;
mod scheduler;
mod status_cache;
mod token;
mod types;
mod wire;

pub mod anthropic;

use std::{collections::BTreeMap, time::Instant};

use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::Json,
};
use chrono::Utc;
pub(crate) use runtime::KiroGatewayRuntimeState;
use serde_json::json;
use sha2::{Digest, Sha256};
use static_flow_shared::llm_gateway_store::{
    now_ms, LlmGatewayKeyRecord, LlmGatewayUsageEventRecord, DEFAULT_KIRO_CHANNEL_MAX_CONCURRENCY,
    DEFAULT_KIRO_CHANNEL_MIN_START_INTERVAL_MS, LLM_GATEWAY_KEY_STATUS_ACTIVE,
    LLM_GATEWAY_KEY_STATUS_DISABLED, LLM_GATEWAY_PROTOCOL_ANTHROPIC, LLM_GATEWAY_PROVIDER_KIRO,
};
pub(crate) use status_cache::{refresh_cached_status, spawn_status_refresher};

use self::{
    anthropic::supported_model_ids,
    auth_file::KiroAuthRecord,
    status_cache::{
        refresh_cached_status_for_account, remove_cached_status_for_account,
        KiroCachedAccountStatus,
    },
    types::{
        AdminKiroAccountsResponse, AdminKiroKeyView, AdminKiroKeysResponse,
        AdminKiroUsageEventView, AdminKiroUsageEventsResponse, AdminKiroUsageQuery,
        CreateKiroKeyRequest, CreateManualKiroAccountRequest, ImportLocalKiroAccountRequest,
        KiroAccessResponse, KiroAccountView, KiroBalanceView, KiroCacheView, KiroPublicStatusView,
        PatchKiroAccountRequest, PatchKiroKeyRequest,
    },
};
use crate::{
    handlers::{ensure_admin_access, generate_task_id, ErrorResponse},
    public_submit_guard::extract_client_ip,
    state::AppState,
    upstream_proxy::parse_account_proxy_selection_patch,
};

const MIN_KIRO_CHANNEL_MAX_CONCURRENCY: u64 = 1;
const MAX_KIRO_CHANNEL_MAX_CONCURRENCY: u64 = 16;
const MIN_KIRO_CHANNEL_MIN_START_INTERVAL_MS: u64 = 0;
const MAX_KIRO_CHANNEL_MIN_START_INTERVAL_MS: u64 = 60_000;
type KiroAdminResult<T> = Result<T, (StatusCode, Json<ErrorResponse>)>;

/// Per-request context captured at authentication time and carried through
/// the proxy pipeline until the usage event is persisted.
#[derive(Clone)]
pub struct KiroEventContext {
    /// Kiro account name that handled the upstream request. Starts as `None`
    /// at authentication time and is filled in by the provider after account
    /// selection completes.
    pub account_name: Option<String>,
    /// HTTP method of the proxied request (typically `"POST"`).
    pub request_method: String,
    /// Full external URL of the gateway endpoint, including origin.
    pub request_url: String,
    /// Downstream API endpoint path (e.g. `"/v1/messages"`).
    pub endpoint: String,
    /// Model identifier extracted from the request body, if present.
    pub model: Option<String>,
    /// Client IP address derived from forwarding headers.
    pub client_ip: String,
    /// GeoIP-resolved region string for the client IP.
    pub ip_region: String,
    /// JSON-serialized snapshot of the incoming request headers.
    pub request_headers_json: String,
    /// Trailing user message content, kept for audit/logging.
    pub last_message_content: Option<String>,
    /// Wall-clock instant when authentication started; used for latency.
    pub started_at: Instant,
}

/// Aggregated token counts returned by the upstream Kiro provider after a
/// single request completes. Used to update key quotas and persist usage
/// events.
#[derive(Debug, Clone, Copy)]
pub struct KiroUsageSummary {
    /// Number of input tokens billed for this request.
    pub input_uncached_tokens: i32,
    /// Number of prompt-cache input tokens billed for this request.
    ///
    /// Kiro currently does not expose a cache split in the upstream event
    /// stream, so this remains `0` unless a future upstream signal becomes
    /// available.
    pub input_cached_tokens: i32,
    /// Number of output tokens billed for this request.
    pub output_tokens: i32,
    /// Exact Kiro credits consumed by this request when upstream metering is
    /// present.
    pub credit_usage: Option<f64>,
    /// Whether a Kiro request completed without an authoritative meteringEvent.
    pub credit_usage_missing: bool,
}

/// Extension trait on [`AppState`] for Kiro-specific API key authentication.
pub trait AppKiroStateExt {
    /// Authenticate an incoming request against the Kiro key store.
    ///
    /// Extracts the bearer/API key from headers, hashes it, looks up the
    /// matching [`LlmGatewayKeyRecord`] scoped to the Kiro provider, validates
    /// status and quota, and returns the key record together with a
    /// pre-populated [`KiroEventContext`] ready for downstream use.
    async fn authenticate_kiro_key(
        &self,
        headers: &HeaderMap,
    ) -> Result<(LlmGatewayKeyRecord, KiroEventContext), (StatusCode, Json<ErrorResponse>)>;
}

impl AppKiroStateExt for AppState {
    async fn authenticate_kiro_key(
        &self,
        headers: &HeaderMap,
    ) -> Result<(LlmGatewayKeyRecord, KiroEventContext), (StatusCode, Json<ErrorResponse>)> {
        let presented =
            extract_presented_key(headers).ok_or_else(|| unauthorized("Missing API key"))?;
        let key_hash = sha256_hex(presented.as_bytes());
        let key = self
            .llm_gateway_store
            .get_key_by_hash_for_provider(&key_hash, LLM_GATEWAY_PROVIDER_KIRO)
            .await
            .map_err(|err| internal_error("Failed to load Kiro API key", err))?
            .ok_or_else(|| unauthorized("Invalid API key"))?;
        let effective_key = self.llm_gateway.overlay_key_usage(&key).await;
        validate_key(&effective_key)?;
        let request_url = external_origin(headers)
            .map(|origin| format!("{origin}/api/kiro-gateway"))
            .unwrap_or_else(|| "/api/kiro-gateway".to_string());
        let client_ip = extract_client_ip(headers);
        let ip_region = self.geoip.resolve_region(&client_ip).await;
        let request_headers_json = serde_json::to_string(
            &headers
                .iter()
                .filter_map(|(key, value)| {
                    value
                        .to_str()
                        .ok()
                        .map(|value| (key.as_str().to_string(), value.to_string()))
                })
                .collect::<Vec<_>>(),
        )
        .unwrap_or_else(|_| "[]".to_string());
        Ok((effective_key, KiroEventContext {
            account_name: None,
            request_method: "POST".to_string(),
            request_url,
            endpoint: String::new(),
            model: None,
            client_ip,
            ip_region,
            request_headers_json,
            last_message_content: None,
            started_at: Instant::now(),
        }))
    }
}

/// Returns the public Kiro gateway access info (base URL, auth cache TTL,
/// and per-account availability statuses) without requiring admin credentials.
pub async fn get_public_access(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<KiroAccessResponse>, (StatusCode, Json<ErrorResponse>)> {
    let gateway_path = "/api/kiro-gateway".to_string();
    let base_url = external_origin(&headers)
        .map(|origin| format!("{origin}{gateway_path}"))
        .unwrap_or_else(|| gateway_path.clone());
    let auth_cache_ttl_seconds = state
        .llm_gateway_runtime_config
        .read()
        .auth_cache_ttl_seconds;
    let accounts = build_public_statuses(&state).await;
    Ok(Json(KiroAccessResponse {
        base_url,
        gateway_path,
        auth_cache_ttl_seconds,
        accounts,
        generated_at: now_ms(),
    }))
}

/// Lists all Kiro gateway API keys (admin-only).
pub async fn list_admin_keys(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<AdminKiroKeysResponse>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let base_keys = state
        .llm_gateway_store
        .list_keys_for_provider(LLM_GATEWAY_PROVIDER_KIRO)
        .await
        .map_err(|err| internal_error("Failed to list Kiro keys", err))?;
    let keys = state.llm_gateway.overlay_key_usage_batch(&base_keys).await;
    Ok(Json(AdminKiroKeysResponse {
        keys: keys.iter().map(AdminKiroKeyView::from).collect(),
        auth_cache_ttl_seconds: state
            .llm_gateway_runtime_config
            .read()
            .auth_cache_ttl_seconds,
        generated_at: now_ms(),
    }))
}

/// Create a new Kiro API key with the given name and billable token quota.
pub async fn create_admin_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateKiroKeyRequest>,
) -> Result<Json<AdminKiroKeyView>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let name = normalize_name(&request.name)?;
    let now = now_ms();
    let secret = generate_secret();
    let record = LlmGatewayKeyRecord {
        id: generate_task_id("kiro-key"),
        name,
        secret: secret.clone(),
        key_hash: sha256_hex(secret.as_bytes()),
        status: LLM_GATEWAY_KEY_STATUS_ACTIVE.to_string(),
        provider_type: LLM_GATEWAY_PROVIDER_KIRO.to_string(),
        protocol_family: LLM_GATEWAY_PROTOCOL_ANTHROPIC.to_string(),
        public_visible: false,
        quota_billable_limit: request.quota_billable_limit,
        usage_input_uncached_tokens: 0,
        usage_input_cached_tokens: 0,
        usage_output_tokens: 0,
        usage_billable_tokens: 0,
        usage_credit_total: 0.0,
        usage_credit_missing_events: 0,
        last_used_at: None,
        created_at: now,
        updated_at: now,
        route_strategy: None,
        fixed_account_name: None,
        auto_account_names: None,
        model_name_map: None,
        request_max_concurrency: None,
        request_min_start_interval_ms: None,
    };
    state
        .llm_gateway_store
        .create_key(&record)
        .await
        .map_err(|err| internal_error("Failed to create Kiro key", err))?;
    Ok(Json(AdminKiroKeyView::from(&record)))
}

/// Update mutable fields on an existing Kiro key and return the view with
/// in-memory usage rollup applied.
pub async fn patch_admin_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key_id): Path<String>,
    Json(request): Json<PatchKiroKeyRequest>,
) -> Result<Json<AdminKiroKeyView>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let mut key = state
        .llm_gateway_store
        .get_key_by_id_for_provider(&key_id, LLM_GATEWAY_PROVIDER_KIRO)
        .await
        .map_err(|err| internal_error("Failed to load Kiro key", err))?
        .ok_or_else(|| not_found("Kiro key not found"))?;
    if let Some(name) = request.name {
        key.name = normalize_name(&name)?;
    }
    if let Some(status) = request.status {
        key.status = normalize_status(&status)?;
    }
    if let Some(limit) = request.quota_billable_limit {
        key.quota_billable_limit = limit;
    }
    if let Some(model_name_map) = request.model_name_map {
        key.model_name_map = normalize_model_name_map(model_name_map)?;
    }
    key.public_visible = false;
    key.updated_at = now_ms();
    state
        .llm_gateway_store
        .replace_key(&key)
        .await
        .map_err(|err| internal_error("Failed to update Kiro key", err))?;
    let effective_key = state.llm_gateway.overlay_key_usage(&key).await;
    Ok(Json(AdminKiroKeyView::from(&effective_key)))
}

/// Delete a Kiro API key by ID.
pub async fn delete_admin_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(key_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let key = state
        .llm_gateway_store
        .get_key_by_id_for_provider(&key_id, LLM_GATEWAY_PROVIDER_KIRO)
        .await
        .map_err(|err| internal_error("Failed to load Kiro key", err))?
        .ok_or_else(|| not_found("Kiro key not found"))?;
    state
        .llm_gateway_store
        .delete_key(&key_id)
        .await
        .map_err(|err| internal_error("Failed to delete Kiro key", err))?;
    Ok(Json(json!({"deleted": true, "id": key.id})))
}

/// List Kiro usage events with pagination (admin-only).
pub async fn list_admin_usage_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<AdminKiroUsageQuery>,
) -> Result<Json<AdminKiroUsageEventsResponse>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let total = state
        .llm_gateway_store
        .count_usage_events_for_provider(query.key_id.as_deref(), Some(LLM_GATEWAY_PROVIDER_KIRO))
        .await
        .map_err(|err| internal_error("Failed to count Kiro usage events", err))?;
    let offset = query.offset.unwrap_or(0);
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    if total == 0 || offset >= total {
        return Ok(Json(AdminKiroUsageEventsResponse {
            total,
            offset,
            limit,
            has_more: false,
            events: Vec::new(),
            generated_at: now_ms(),
        }));
    }
    let fetch_count = (total - offset).min(limit);
    let reverse_offset = total.saturating_sub(offset.saturating_add(fetch_count));
    let mut events = state
        .llm_gateway_store
        .query_usage_events(
            query.key_id.as_deref(),
            Some(LLM_GATEWAY_PROVIDER_KIRO),
            Some(fetch_count),
            Some(reverse_offset),
        )
        .await
        .map_err(|err| internal_error("Failed to query Kiro usage events", err))?;
    events.sort_by(|left, right| right.created_at.cmp(&left.created_at));
    let has_more = offset.saturating_add(events.len()) < total;
    Ok(Json(AdminKiroUsageEventsResponse {
        total,
        offset,
        limit,
        has_more,
        events: events.iter().map(AdminKiroUsageEventView::from).collect(),
        generated_at: now_ms(),
    }))
}

/// List all configured Kiro accounts with their cached balance/status.
pub async fn list_admin_accounts(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<AdminKiroAccountsResponse>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    Ok(Json(AdminKiroAccountsResponse {
        accounts: build_account_views(&state).await,
        generated_at: now_ms(),
    }))
}

/// Create a Kiro account from a manually supplied JSON payload.
pub async fn create_manual_account(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateManualKiroAccountRequest>,
) -> Result<Json<KiroAccountView>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let auth = auth_record_from_manual_request(request)?;
    let saved = state
        .kiro_gateway
        .token_manager
        .upsert_auth(auth)
        .await
        .map_err(|err| internal_error("Failed to save Kiro account", err))?;
    if let Err(err) =
        refresh_cached_status_for_account(&state.kiro_gateway, &saved.name, false).await
    {
        tracing::warn!(
            account_name = %saved.name,
            "failed to prime cached Kiro status after manual save: {err:#}"
        );
    }
    build_account_view_by_name(&state, &saved.name)
        .await
        .ok_or_else(|| internal_error_message("Saved Kiro account but failed to reload it"))
        .map(Json)
}

/// Import a Kiro account from the local Kiro CLI SQLite store, optionally
/// overriding scheduler settings.
pub async fn import_local_account(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ImportLocalKiroAccountRequest>,
) -> Result<Json<KiroAccountView>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    if let Some(value) = request.kiro_channel_max_concurrency {
        if !(MIN_KIRO_CHANNEL_MAX_CONCURRENCY..=MAX_KIRO_CHANNEL_MAX_CONCURRENCY).contains(&value) {
            return Err(bad_request("kiro_channel_max_concurrency is out of range"));
        }
    }
    if let Some(value) = request.kiro_channel_min_start_interval_ms {
        if !(MIN_KIRO_CHANNEL_MIN_START_INTERVAL_MS..=MAX_KIRO_CHANNEL_MIN_START_INTERVAL_MS)
            .contains(&value)
        {
            return Err(bad_request("kiro_channel_min_start_interval_ms is out of range"));
        }
    }
    let imported = state
        .kiro_gateway
        .token_manager
        .import_local_account(request.name.as_deref(), request.sqlite_path.as_deref())
        .await
        .map_err(|err| internal_error("Failed to import local Kiro auth", err))?;
    let mut saved = imported;
    let override_max = request
        .kiro_channel_max_concurrency
        .unwrap_or_else(|| saved.effective_kiro_channel_max_concurrency());
    let override_min = request
        .kiro_channel_min_start_interval_ms
        .unwrap_or_else(|| saved.effective_kiro_channel_min_start_interval_ms());
    if request.kiro_channel_max_concurrency.is_some()
        || request.kiro_channel_min_start_interval_ms.is_some()
    {
        saved.kiro_channel_max_concurrency = Some(override_max);
        saved.kiro_channel_min_start_interval_ms = Some(override_min);
        saved = state
            .kiro_gateway
            .token_manager
            .upsert_auth(saved)
            .await
            .map_err(|err| internal_error("Failed to save imported Kiro scheduler config", err))?;
    }
    if let Err(err) =
        refresh_cached_status_for_account(&state.kiro_gateway, &saved.name, false).await
    {
        tracing::warn!(
            account_name = %saved.name,
            "failed to prime cached Kiro status after import: {err:#}"
        );
    }
    build_account_view_by_name(&state, &saved.name)
        .await
        .ok_or_else(|| internal_error_message("Imported Kiro account but failed to reload it"))
        .map(Json)
}

/// Update the per-account scheduler settings (concurrency and start interval).
pub async fn patch_account(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Json(request): Json<PatchKiroAccountRequest>,
) -> Result<Json<KiroAccountView>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let proxy_selection = parse_account_proxy_selection_patch(
        request.proxy_mode.as_deref(),
        request.proxy_config_id.as_deref(),
    )
    .map_err(|err| bad_request(&err.to_string()))?;
    let mut auth = state
        .kiro_gateway
        .token_manager
        .auth_by_name(&name)
        .await
        .map_err(|err| internal_error("Failed to load Kiro account", err))?
        .ok_or_else(|| not_found("Kiro account not found"))?;

    let max_concurrency = request
        .kiro_channel_max_concurrency
        .unwrap_or_else(|| auth.effective_kiro_channel_max_concurrency());
    if !(MIN_KIRO_CHANNEL_MAX_CONCURRENCY..=MAX_KIRO_CHANNEL_MAX_CONCURRENCY)
        .contains(&max_concurrency)
    {
        return Err(bad_request("kiro_channel_max_concurrency is out of range"));
    }

    let min_start_interval_ms = request
        .kiro_channel_min_start_interval_ms
        .unwrap_or_else(|| auth.effective_kiro_channel_min_start_interval_ms());
    if !(MIN_KIRO_CHANNEL_MIN_START_INTERVAL_MS..=MAX_KIRO_CHANNEL_MIN_START_INTERVAL_MS)
        .contains(&min_start_interval_ms)
    {
        return Err(bad_request("kiro_channel_min_start_interval_ms is out of range"));
    }
    if let Some(proxy_selection) = proxy_selection.as_ref() {
        state
            .upstream_proxy_registry
            .resolve_proxy_for_selection(LLM_GATEWAY_PROVIDER_KIRO, Some(proxy_selection))
            .await
            .map_err(|err| bad_request(&format!("invalid proxy selection: {err}")))?;
    }

    auth.kiro_channel_max_concurrency = Some(max_concurrency);
    auth.kiro_channel_min_start_interval_ms = Some(min_start_interval_ms);
    if let Some(proxy_selection) = proxy_selection {
        auth.proxy_mode = proxy_selection.proxy_mode;
        auth.proxy_config_id = proxy_selection.proxy_config_id;
    }
    let saved = state
        .kiro_gateway
        .token_manager
        .upsert_auth(auth)
        .await
        .map_err(|err| internal_error("Failed to update Kiro account", err))?;
    state.kiro_gateway.request_scheduler.notify_config_changed();
    tracing::info!(
        account_name = %saved.name,
        kiro_channel_max_concurrency = saved.effective_kiro_channel_max_concurrency(),
        kiro_channel_min_start_interval_ms = saved.effective_kiro_channel_min_start_interval_ms(),
        proxy_mode = %saved.proxy_selection().proxy_mode.as_str(),
        proxy_config_id = ?saved.proxy_selection().proxy_config_id,
        "updated Kiro account scheduler settings"
    );
    build_account_view_by_name(&state, &saved.name)
        .await
        .ok_or_else(|| internal_error_message("Updated Kiro account but failed to reload it"))
        .map(Json)
}

/// Return the cached balance for one Kiro account.
pub async fn get_account_balance(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Result<Json<KiroBalanceView>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let snapshot = state.kiro_gateway.cached_status_snapshot().await;
    let Some(entry) = snapshot.accounts.get(&name) else {
        return Err(not_found("Kiro balance cache not found"));
    };
    let Some(balance) = entry.balance.clone() else {
        let message = entry
            .cache
            .error_message
            .clone()
            .unwrap_or_else(|| "Kiro balance cache is not ready yet".to_string());
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorResponse {
                error: message,
                code: 503,
            }),
        ));
    };
    Ok(Json(balance))
}

/// Force-refresh the cached balance for one Kiro account and return it.
pub async fn refresh_account_balance_cache(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Result<Json<KiroBalanceView>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let entry = refresh_cached_status_for_account(&state.kiro_gateway, &name, false)
        .await
        .map_err(|err| internal_error("Failed to refresh cached Kiro balance", err))?;
    let Some(balance) = entry.balance else {
        let message = entry
            .cache
            .error_message
            .unwrap_or_else(|| "Kiro balance cache is not ready yet".to_string());
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorResponse {
                error: message,
                code: 503,
            }),
        ));
    };
    Ok(Json(balance))
}

/// Delete a Kiro account and evict its cached status entry.
pub async fn delete_account(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    state
        .kiro_gateway
        .token_manager
        .delete_auth(&name)
        .await
        .map_err(|err| internal_error("Failed to delete Kiro account", err))?;
    remove_cached_status_for_account(&state.kiro_gateway, &name).await;
    Ok(Json(json!({"status":"ok"})))
}

/// Assemble admin-facing account views by joining persisted auth records with
/// the latest cached balance/status snapshot.
async fn build_account_views(state: &AppState) -> Vec<KiroAccountView> {
    let cached = state.kiro_gateway.cached_status_snapshot().await;
    let Ok(auths) = state.kiro_gateway.token_manager.list_auths().await else {
        return Vec::new();
    };
    let mut views = Vec::with_capacity(auths.len());
    for auth in auths {
        let (balance, cache) = cached_status_parts(cached.accounts.get(&auth.name));
        let (effective_proxy_source, effective_proxy_url, effective_proxy_config_name) =
            effective_account_proxy_parts(state, &auth).await;
        views.push(KiroAccountView::from_auth(
            &auth,
            effective_proxy_source,
            effective_proxy_url,
            effective_proxy_config_name,
            balance,
            cache,
        ));
    }
    views
}

/// Load a single account view by name, returning `None` if the account does
/// not exist on disk.
async fn build_account_view_by_name(state: &AppState, name: &str) -> Option<KiroAccountView> {
    let auth = state
        .kiro_gateway
        .token_manager
        .auth_by_name(name)
        .await
        .ok()
        .flatten()?;
    let cached = state.kiro_gateway.cached_status_snapshot().await;
    let (balance, cache) = cached_status_parts(cached.accounts.get(name));
    let (effective_proxy_source, effective_proxy_url, effective_proxy_config_name) =
        effective_account_proxy_parts(state, &auth).await;
    Some(KiroAccountView::from_auth(
        &auth,
        effective_proxy_source,
        effective_proxy_url,
        effective_proxy_config_name,
        balance,
        cache,
    ))
}

async fn effective_account_proxy_parts(
    state: &AppState,
    auth: &KiroAuthRecord,
) -> (String, Option<String>, Option<String>) {
    match state
        .upstream_proxy_registry
        .resolve_proxy_for_selection(LLM_GATEWAY_PROVIDER_KIRO, Some(&auth.proxy_selection()))
        .await
    {
        Ok(resolved) => (
            resolved.source.as_str().to_string(),
            resolved.proxy_url.clone(),
            resolved.proxy_config_name.clone(),
        ),
        Err(err) => (format!("invalid ({err})"), None, None),
    }
}

/// Build the unauthenticated public status list shown on the access endpoint.
async fn build_public_statuses(state: &AppState) -> Vec<KiroPublicStatusView> {
    let cached = state.kiro_gateway.cached_status_snapshot().await;
    let Ok(auths) = state.kiro_gateway.token_manager.list_auths().await else {
        return Vec::new();
    };
    let mut statuses = Vec::with_capacity(auths.len());
    for auth in auths {
        let (balance, cache) = cached_status_parts(cached.accounts.get(&auth.name));
        statuses.push(KiroPublicStatusView::from_auth_and_balance(&auth, balance.as_ref(), cache));
    }
    statuses
}

/// Split a cached account status entry into its balance and cache-health
/// components, returning sensible defaults when no entry exists yet.
fn cached_status_parts(
    entry: Option<&KiroCachedAccountStatus>,
) -> (Option<KiroBalanceView>, KiroCacheView) {
    entry
        .map(|status| (status.balance.clone(), status.cache.clone()))
        .unwrap_or_else(|| {
            (None, KiroCacheView {
                status: "loading".to_string(),
                refresh_interval_seconds: status_cache::KIRO_STATUS_REFRESH_SECONDS,
                last_checked_at: None,
                last_success_at: None,
                error_message: None,
            })
        })
}

/// Persist a Kiro usage event and update the in-memory rollup via the shared
/// LLM gateway runtime.
pub async fn record_messages_usage(
    state: &AppState,
    key: &LlmGatewayKeyRecord,
    event_context: &KiroEventContext,
    usage: KiroUsageSummary,
    usage_missing: bool,
) -> anyhow::Result<()> {
    let current = state
        .llm_gateway_store
        .get_key_by_id_for_provider(&key.id, LLM_GATEWAY_PROVIDER_KIRO)
        .await?
        .unwrap_or_else(|| key.clone());
    let latency_ms = event_context
        .started_at
        .elapsed()
        .as_millis()
        .min(i32::MAX as u128) as i32;
    let event = LlmGatewayUsageEventRecord {
        id: generate_task_id("kiro-usage"),
        key_id: current.id.clone(),
        key_name: current.name.clone(),
        provider_type: LLM_GATEWAY_PROVIDER_KIRO.to_string(),
        account_name: event_context.account_name.clone(),
        request_method: event_context.request_method.clone(),
        request_url: event_context.request_url.clone(),
        latency_ms,
        endpoint: event_context.endpoint.clone(),
        model: event_context.model.clone(),
        status_code: 200,
        input_uncached_tokens: usage.input_uncached_tokens.max(0) as u64,
        input_cached_tokens: usage.input_cached_tokens.max(0) as u64,
        output_tokens: usage.output_tokens.max(0) as u64,
        billable_tokens: (usage.input_uncached_tokens.max(0)
            + usage.input_cached_tokens.max(0)
            + usage.output_tokens.max(0)) as u64,
        usage_missing,
        credit_usage: usage.credit_usage,
        credit_usage_missing: usage.credit_usage_missing,
        client_ip: event_context.client_ip.clone(),
        ip_region: event_context.ip_region.clone(),
        request_headers_json: event_context.request_headers_json.clone(),
        last_message_content: event_context.last_message_content.clone(),
        created_at: now_ms(),
    };
    let _updated = state
        .llm_gateway
        .append_usage_event(&current, &event)
        .await?;
    tracing::info!(
        key_id = %current.id,
        key_name = %current.name,
        account_name = event.account_name.as_deref().unwrap_or("unknown"),
        endpoint = %event.endpoint,
        request_url = %event.request_url,
        latency_ms = event.latency_ms,
        billable_tokens = event.billable_tokens,
        credit_usage = event.credit_usage.unwrap_or_default(),
        credit_usage_missing = event.credit_usage_missing,
        "persisted kiro usage event"
    );
    Ok(())
}

/// Convert a manual account creation request into a canonicalized auth record,
/// applying scheduler defaults and validation.
fn auth_record_from_manual_request(
    request: CreateManualKiroAccountRequest,
) -> Result<KiroAuthRecord, (StatusCode, Json<ErrorResponse>)> {
    let name = normalize_name(&request.name)?;
    let kiro_channel_max_concurrency = request
        .kiro_channel_max_concurrency
        .unwrap_or(DEFAULT_KIRO_CHANNEL_MAX_CONCURRENCY);
    if !(MIN_KIRO_CHANNEL_MAX_CONCURRENCY..=MAX_KIRO_CHANNEL_MAX_CONCURRENCY)
        .contains(&kiro_channel_max_concurrency)
    {
        return Err(bad_request("kiro_channel_max_concurrency is out of range"));
    }
    let kiro_channel_min_start_interval_ms = request
        .kiro_channel_min_start_interval_ms
        .unwrap_or(DEFAULT_KIRO_CHANNEL_MIN_START_INTERVAL_MS);
    if !(MIN_KIRO_CHANNEL_MIN_START_INTERVAL_MS..=MAX_KIRO_CHANNEL_MIN_START_INTERVAL_MS)
        .contains(&kiro_channel_min_start_interval_ms)
    {
        return Err(bad_request("kiro_channel_min_start_interval_ms is out of range"));
    }
    Ok(KiroAuthRecord {
        name,
        access_token: normalize_optional_string(request.access_token),
        refresh_token: normalize_optional_string(request.refresh_token),
        profile_arn: normalize_optional_string(request.profile_arn),
        expires_at: normalize_optional_string(request.expires_at),
        auth_method: normalize_optional_string(request.auth_method),
        client_id: normalize_optional_string(request.client_id),
        client_secret: normalize_optional_string(request.client_secret),
        region: normalize_optional_string(request.region),
        auth_region: normalize_optional_string(request.auth_region),
        api_region: normalize_optional_string(request.api_region),
        machine_id: normalize_optional_string(request.machine_id),
        provider: normalize_optional_string(request.provider),
        email: normalize_optional_string(request.email),
        subscription_title: normalize_optional_string(request.subscription_title),
        kiro_channel_max_concurrency: Some(kiro_channel_max_concurrency),
        kiro_channel_min_start_interval_ms: Some(kiro_channel_min_start_interval_ms),
        proxy_mode: Default::default(),
        proxy_config_id: None,
        proxy_url: None,
        proxy_username: None,
        proxy_password: None,
        disabled: request.disabled,
        source: Some("manual".to_string()),
        source_db_path: None,
        last_imported_at: Some(Utc::now().timestamp_millis()),
    }
    .canonicalize())
}

fn extract_presented_key(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-api-key")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .or_else(|| {
            headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .map(str::trim)
                .and_then(|value| value.strip_prefix("Bearer "))
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
        })
}

fn validate_key(key: &LlmGatewayKeyRecord) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    if key.status != LLM_GATEWAY_KEY_STATUS_ACTIVE {
        return Err(forbidden("Kiro key is disabled"));
    }
    if key.remaining_billable() <= 0 {
        return Err((
            StatusCode::PAYMENT_REQUIRED,
            Json(ErrorResponse {
                error: "Kiro key quota exhausted".to_string(),
                code: 402,
            }),
        ));
    }
    Ok(())
}

fn generate_secret() -> String {
    format!("sf-kiro-{}", uuid::Uuid::new_v4().simple())
}

fn normalize_name(raw: &str) -> Result<String, (StatusCode, Json<ErrorResponse>)> {
    let value = raw.trim();
    if value.is_empty() {
        return Err(bad_request("name is required"));
    }
    Ok(value.to_string())
}

fn normalize_status(raw: &str) -> Result<String, (StatusCode, Json<ErrorResponse>)> {
    match raw.trim() {
        LLM_GATEWAY_KEY_STATUS_ACTIVE | LLM_GATEWAY_KEY_STATUS_DISABLED => {
            Ok(raw.trim().to_string())
        },
        _ => Err(bad_request("status must be `active` or `disabled`")),
    }
}

fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn normalize_model_name_map(
    raw: BTreeMap<String, String>,
) -> KiroAdminResult<Option<BTreeMap<String, String>>> {
    let supported = supported_model_ids();
    let mut normalized = BTreeMap::new();
    for (source_model, target_model) in raw {
        let source_model = source_model.trim().to_string();
        let target_model = target_model.trim().to_string();
        if source_model.is_empty() || target_model.is_empty() {
            return Err(bad_request("model_name_map entries must not be empty"));
        }
        if !supported.iter().any(|candidate| candidate == &source_model) {
            return Err(bad_request("model_name_map contains unsupported source model"));
        }
        if !supported.iter().any(|candidate| candidate == &target_model) {
            return Err(bad_request("model_name_map contains unsupported target model"));
        }
        if source_model != target_model {
            normalized.insert(source_model, target_model);
        }
    }
    Ok((!normalized.is_empty()).then_some(normalized))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn bad_request(message: &str) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::BAD_REQUEST,
        Json(ErrorResponse {
            error: message.to_string(),
            code: 400,
        }),
    )
}

fn unauthorized(message: &str) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::UNAUTHORIZED,
        Json(ErrorResponse {
            error: message.to_string(),
            code: 401,
        }),
    )
}

fn forbidden(message: &str) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::FORBIDDEN,
        Json(ErrorResponse {
            error: message.to_string(),
            code: 403,
        }),
    )
}

fn not_found(message: &str) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::NOT_FOUND,
        Json(ErrorResponse {
            error: message.to_string(),
            code: 404,
        }),
    )
}

fn internal_error_message(message: &str) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorResponse {
            error: message.to_string(),
            code: 500,
        }),
    )
}

fn internal_error(message: &str, err: impl std::fmt::Display) -> (StatusCode, Json<ErrorResponse>) {
    tracing::error!("{message}: {err}");
    internal_error_message(message)
}

fn external_origin(headers: &HeaderMap) -> Option<String> {
    let host = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get(header::HOST))
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("http");
    Some(format!("{scheme}://{host}"))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::normalize_model_name_map;

    #[test]
    fn normalize_model_name_map_drops_identity_entries() {
        let normalized = normalize_model_name_map(BTreeMap::from([
            ("claude-haiku-4-5-20251001".to_string(), "claude-haiku-4-5-20251001".to_string()),
            ("claude-haiku-4-5-20251001-thinking".to_string(), "claude-sonnet-4-6".to_string()),
        ]))
        .expect("normalize should succeed");

        assert_eq!(
            normalized,
            Some(BTreeMap::from([(
                "claude-haiku-4-5-20251001-thinking".to_string(),
                "claude-sonnet-4-6".to_string(),
            )]))
        );
    }

    #[test]
    fn normalize_model_name_map_rejects_unknown_models() {
        let result = normalize_model_name_map(BTreeMap::from([(
            "claude-unknown".to_string(),
            "claude-sonnet-4-6".to_string(),
        )]));

        assert!(result.is_err());
    }
}
