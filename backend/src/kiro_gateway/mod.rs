//! Kiro Gateway module — API key authentication, account management,
//! usage tracking, and admin CRUD for the Kiro provider backend.

pub(crate) mod auth_file;
mod billable_multipliers;
mod cache_policy;
pub(crate) mod cache_sim;
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

use std::{
    collections::{BTreeMap, BTreeSet},
    time::Instant,
};

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
    compute_kiro_billable_tokens, now_ms, parse_kiro_cache_policy_override_json,
    LlmGatewayAccountGroupRecord, LlmGatewayKeyRecord, LlmGatewayUsageEventRecord,
    DEFAULT_KIRO_CHANNEL_MAX_CONCURRENCY, DEFAULT_KIRO_CHANNEL_MIN_START_INTERVAL_MS,
    LLM_GATEWAY_KEY_STATUS_ACTIVE, LLM_GATEWAY_KEY_STATUS_DISABLED, LLM_GATEWAY_PROTOCOL_ANTHROPIC,
    LLM_GATEWAY_PROVIDER_KIRO,
};
pub(crate) use status_cache::{refresh_cached_status, spawn_status_refresher};

use self::{
    anthropic::supported_model_ids,
    auth_file::KiroAuthRecord,
    billable_multipliers::{
        canonicalize_kiro_billable_model_multipliers_override_json,
        resolve_effective_kiro_billable_model_multipliers,
        uses_global_kiro_billable_model_multipliers,
    },
    cache_policy::{resolve_effective_kiro_cache_policy, uses_global_kiro_cache_policy},
    status_cache::{
        refresh_cached_status_for_account, remove_cached_status_for_account,
        KiroCachedAccountStatus,
    },
    types::{
        AdminKiroAccountGroupView, AdminKiroAccountGroupsResponse, AdminKiroAccountStatusesQuery,
        AdminKiroAccountStatusesResponse, AdminKiroAccountsResponse, AdminKiroKeyView,
        AdminKiroKeysResponse, AdminKiroUsageEventDetailView, AdminKiroUsageEventView,
        AdminKiroUsageEventsResponse, AdminKiroUsageQuery, CreateKiroAccountGroupRequest,
        CreateKiroKeyRequest, CreateManualKiroAccountRequest, ImportLocalKiroAccountRequest,
        KiroAccessResponse, KiroAccountView, KiroBalanceView, KiroCacheView, KiroPublicStatusView,
        PatchKiroAccountGroupRequest, PatchKiroAccountRequest, PatchKiroKeyRequest,
    },
};
use crate::{
    handlers::{ensure_admin_access, generate_task_id, ErrorResponse},
    llm_gateway::normalize_optional_account_group_id_input,
    public_submit_guard::extract_client_ip,
    state::AppState,
    upstream_proxy::parse_account_proxy_selection_patch,
};

const MIN_KIRO_CHANNEL_MAX_CONCURRENCY: u64 = 1;
const MAX_KIRO_CHANNEL_MAX_CONCURRENCY: u64 = 16;
const MIN_KIRO_CHANNEL_MIN_START_INTERVAL_MS: u64 = 0;
const MAX_KIRO_CHANNEL_MIN_START_INTERVAL_MS: u64 = 60_000;
const DEFAULT_ADMIN_KIRO_ACCOUNT_STATUS_LIMIT: usize = 24;
const MAX_ADMIN_KIRO_ACCOUNT_STATUS_LIMIT: usize = 96;
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
    /// Full downstream client request body, serialized before local mutation.
    pub client_request_body_json: Option<String>,
    /// Full upstream Kiro request body prepared for diagnostics.
    pub upstream_request_body_json: Option<String>,
    /// Final conversation identifier sent to Kiro after request conversion.
    pub conversation_id: Option<String>,
    /// Human-readable session resolution path (e.g. `request_header`,
    /// `metadata_json`, `generated_fallback`).
    pub session_resolution: Option<String>,
    /// Header / metadata field name used to source the session identifier.
    pub session_source_name: Option<String>,
    /// Short preview of the session identifier source value for debugging.
    pub session_source_value_preview: Option<String>,
    /// Wall-clock instant when authentication started; used for latency.
    pub started_at: Instant,
}

/// Aggregated token counts returned by the upstream Kiro provider after a
/// single request completes. Used to update key quotas and persist usage
/// events.
#[derive(Debug, Clone, Copy)]
pub struct KiroUsageSummary {
    /// Reported input tokens for this request after splitting the
    /// authoritative upstream total into uncached and simulated cache-read
    /// portions.
    pub input_uncached_tokens: i32,
    /// Conservative lower-bound estimate of prompt-cache read tokens.
    ///
    /// Kiro does not expose cache-read token counts directly, so this value
    /// is derived from observed credit usage plus per-model calibration
    /// coefficients.
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
            client_request_body_json: None,
            upstream_request_body_json: None,
            conversation_id: None,
            session_resolution: None,
            session_source_name: None,
            session_source_value_preview: None,
            started_at: Instant::now(),
        }))
    }
}

fn build_admin_kiro_key_view(
    runtime_config: &crate::state::LlmGatewayRuntimeConfig,
    key: &LlmGatewayKeyRecord,
) -> KiroAdminResult<AdminKiroKeyView> {
    let effective_policy = resolve_effective_kiro_cache_policy(runtime_config, key)
        .map_err(|err| internal_error("Failed to resolve effective Kiro cache policy", err))?;
    let effective_billable_model_multipliers =
        resolve_effective_kiro_billable_model_multipliers(runtime_config, key).map_err(|err| {
            internal_error("Failed to resolve effective Kiro billable multiplier config", err)
        })?;
    Ok(AdminKiroKeyView::from_key_and_effective_policy(
        key,
        &effective_policy,
        uses_global_kiro_cache_policy(key),
        &effective_billable_model_multipliers,
        uses_global_kiro_billable_model_multipliers(key),
    ))
}

fn validate_kiro_cache_policy_override_update(
    runtime_config: &crate::state::LlmGatewayRuntimeConfig,
    key: &LlmGatewayKeyRecord,
    override_json: Option<&str>,
) -> KiroAdminResult<()> {
    let mut candidate = key.clone();
    candidate.kiro_cache_policy_override_json = override_json.map(ToString::to_string);
    resolve_effective_kiro_cache_policy(runtime_config, &candidate)
        .map_err(|_| bad_request("kiro_cache_policy_override_json is invalid"))?;
    Ok(())
}

fn validate_kiro_billable_model_multipliers_override_update(
    runtime_config: &crate::state::LlmGatewayRuntimeConfig,
    key: &LlmGatewayKeyRecord,
    override_json: Option<&str>,
) -> KiroAdminResult<()> {
    let mut candidate = key.clone();
    candidate.kiro_billable_model_multipliers_override_json =
        override_json.map(ToString::to_string);
    resolve_effective_kiro_billable_model_multipliers(runtime_config, &candidate)
        .map_err(|_| bad_request("kiro_billable_model_multipliers_override_json is invalid"))?;
    Ok(())
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
    Ok(Json(KiroAccessResponse {
        base_url,
        gateway_path,
        auth_cache_ttl_seconds,
        accounts: public_kiro_access_accounts(),
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
    let runtime_config = state.llm_gateway_runtime_config.read().clone();
    let key_views = keys
        .iter()
        .map(|key| build_admin_kiro_key_view(&runtime_config, key))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(AdminKiroKeysResponse {
        keys: key_views,
        auth_cache_ttl_seconds: state
            .llm_gateway_runtime_config
            .read()
            .auth_cache_ttl_seconds,
        generated_at: now_ms(),
    }))
}

/// List reusable Kiro account-pool groups for the admin UI.
pub async fn list_admin_account_groups(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<AdminKiroAccountGroupsResponse>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let groups = state
        .llm_gateway_store
        .list_account_groups_for_provider(LLM_GATEWAY_PROVIDER_KIRO)
        .await
        .map_err(|err| internal_error("Failed to list Kiro account groups", err))?;
    Ok(Json(AdminKiroAccountGroupsResponse {
        groups: groups.iter().map(AdminKiroAccountGroupView::from).collect(),
        generated_at: now_ms(),
    }))
}

/// Create a reusable Kiro account-pool group.
pub async fn create_admin_account_group(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateKiroAccountGroupRequest>,
) -> Result<Json<AdminKiroAccountGroupView>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let name = normalize_name(&request.name)?;
    let account_names = normalize_kiro_account_group_members(&state, request.account_names).await?;
    let now = now_ms();
    let record = LlmGatewayAccountGroupRecord {
        id: generate_task_id("kiro-group"),
        provider_type: LLM_GATEWAY_PROVIDER_KIRO.to_string(),
        name,
        account_names,
        created_at: now,
        updated_at: now,
    };
    state
        .llm_gateway_store
        .create_account_group(&record)
        .await
        .map_err(|err| internal_error("Failed to create Kiro account group", err))?;
    Ok(Json(AdminKiroAccountGroupView::from(&record)))
}

/// Update one reusable Kiro account-pool group.
pub async fn patch_admin_account_group(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(group_id): Path<String>,
    Json(request): Json<PatchKiroAccountGroupRequest>,
) -> Result<Json<AdminKiroAccountGroupView>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let mut group = load_kiro_account_group(&state, &group_id).await?;
    if let Some(name) = request.name.as_deref() {
        group.name = normalize_name(name)?;
    }
    if let Some(account_names) = request.account_names {
        group.account_names = normalize_kiro_account_group_members(&state, account_names).await?;
    }
    group.updated_at = now_ms();
    state
        .llm_gateway_store
        .replace_account_group(&group)
        .await
        .map_err(|err| internal_error("Failed to update Kiro account group", err))?;
    Ok(Json(AdminKiroAccountGroupView::from(&group)))
}

/// Delete one Kiro account-pool group if no key still references it.
pub async fn delete_admin_account_group(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(group_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let group = load_kiro_account_group(&state, &group_id).await?;
    let keys = state
        .llm_gateway_store
        .list_keys_for_provider(LLM_GATEWAY_PROVIDER_KIRO)
        .await
        .map_err(|err| internal_error("Failed to inspect Kiro keys before group delete", err))?;
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
        .map_err(|err| internal_error("Failed to delete Kiro account group", err))?;
    Ok(Json(json!({"deleted": true, "id": group.id})))
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
    };
    state
        .llm_gateway_store
        .create_key(&record)
        .await
        .map_err(|err| internal_error("Failed to create Kiro key", err))?;
    let runtime_config = state.llm_gateway_runtime_config.read().clone();
    Ok(Json(build_admin_kiro_key_view(&runtime_config, &record)?))
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
    let runtime_config = state.llm_gateway_runtime_config.read().clone();
    if let Some(name) = request.name {
        key.name = normalize_name(&name)?;
    }
    if let Some(status) = request.status {
        key.status = normalize_status(&status)?;
    }
    if let Some(limit) = request.quota_billable_limit {
        key.quota_billable_limit = limit;
    }
    if let Some(group_id) = request.account_group_id.as_deref() {
        key.account_group_id = normalize_optional_account_group_id_input(Some(group_id))
            .map_err(|err| bad_request(&err.to_string()))?;
    }
    if request.route_strategy.is_some()
        || request.fixed_account_name.is_some()
        || request.auto_account_names.is_some()
    {
        let existing_account_names = state
            .kiro_gateway
            .token_manager
            .list_auths()
            .await
            .map_err(|err| internal_error("Failed to load Kiro accounts", err))?
            .into_iter()
            .map(|auth| auth.name)
            .collect::<BTreeSet<_>>();
        let (route_strategy, fixed_account_name, auto_account_names) =
            normalize_kiro_key_route_config_for_patch(
                key.route_strategy.clone(),
                key.fixed_account_name.clone(),
                key.auto_account_names.clone(),
                request.route_strategy.as_deref(),
                request.fixed_account_name.as_deref(),
                request.auto_account_names,
                &existing_account_names,
            )
            .map_err(|err| bad_request(&err.to_string()))?;
        key.route_strategy = route_strategy;
        key.fixed_account_name = fixed_account_name;
        key.auto_account_names = auto_account_names;
    }
    if let Some(model_name_map) = request.model_name_map {
        key.model_name_map = normalize_model_name_map(model_name_map)?;
    }
    if let Some(kiro_request_validation_enabled) = request.kiro_request_validation_enabled {
        key.kiro_request_validation_enabled = kiro_request_validation_enabled;
    }
    if let Some(kiro_cache_estimation_enabled) = request.kiro_cache_estimation_enabled {
        key.kiro_cache_estimation_enabled = kiro_cache_estimation_enabled;
    }
    if let Some(kiro_zero_cache_debug_enabled) = request.kiro_zero_cache_debug_enabled {
        key.kiro_zero_cache_debug_enabled = kiro_zero_cache_debug_enabled;
    }
    if let Some(kiro_cache_policy_override_json) = request.kiro_cache_policy_override_json {
        let override_json = match kiro_cache_policy_override_json {
            Some(value) => {
                parse_kiro_cache_policy_override_json(&value)
                    .map_err(|_| bad_request("kiro_cache_policy_override_json is invalid"))?;
                Some(value)
            },
            None => None,
        };
        validate_kiro_cache_policy_override_update(
            &runtime_config,
            &key,
            override_json.as_deref(),
        )?;
        key.kiro_cache_policy_override_json = override_json;
    }
    if let Some(kiro_billable_model_multipliers_override_json) =
        request.kiro_billable_model_multipliers_override_json
    {
        let override_json = canonicalize_kiro_billable_model_multipliers_override_json(
            kiro_billable_model_multipliers_override_json.as_deref(),
        )
        .map_err(|_| bad_request("kiro_billable_model_multipliers_override_json is invalid"))?;
        validate_kiro_billable_model_multipliers_override_update(
            &runtime_config,
            &key,
            override_json.as_deref(),
        )?;
        key.kiro_billable_model_multipliers_override_json = override_json;
    }
    key.public_visible = false;
    materialize_legacy_kiro_route_group_if_needed(&state, &mut key).await?;
    validate_kiro_key_group_config(&state, &mut key).await?;
    key.updated_at = now_ms();
    state
        .llm_gateway_store
        .replace_key(&key)
        .await
        .map_err(|err| internal_error("Failed to update Kiro key", err))?;
    let effective_key = state.llm_gateway.overlay_key_usage(&key).await;
    Ok(Json(build_admin_kiro_key_view(&runtime_config, &effective_key)?))
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
        .query_usage_event_summaries(
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

pub async fn get_admin_usage_event_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(event_id): Path<String>,
) -> Result<Json<AdminKiroUsageEventDetailView>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let event = state
        .llm_gateway_store
        .get_usage_event_detail_by_id(&event_id)
        .await
        .map_err(|err| internal_error("Failed to load Kiro usage event detail", err))?
        .filter(|event| event.provider_type == LLM_GATEWAY_PROVIDER_KIRO)
        .ok_or_else(|| not_found("Kiro usage event not found"))?;
    Ok(Json(AdminKiroUsageEventDetailView::from(&event)))
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

fn public_kiro_access_accounts() -> Vec<KiroPublicStatusView> {
    Vec::new()
}

fn normalize_admin_kiro_account_status_limit(raw: Option<usize>) -> usize {
    raw.unwrap_or(DEFAULT_ADMIN_KIRO_ACCOUNT_STATUS_LIMIT)
        .clamp(1, MAX_ADMIN_KIRO_ACCOUNT_STATUS_LIMIT)
}

fn normalized_kiro_account_status_prefix(raw: Option<&str>) -> Option<String> {
    let trimmed = raw.unwrap_or_default().trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_ascii_lowercase())
    }
}

fn filter_kiro_account_views_by_prefix(
    accounts: &[KiroAccountView],
    prefix: Option<&str>,
) -> Vec<KiroAccountView> {
    let Some(prefix) = normalized_kiro_account_status_prefix(prefix) else {
        return accounts.to_vec();
    };
    accounts
        .iter()
        .filter(|item| item.name.to_ascii_lowercase().starts_with(&prefix))
        .cloned()
        .collect()
}

struct PaginatedKiroAccountViews {
    accounts: Vec<KiroAccountView>,
    total: usize,
    limit: usize,
    offset: usize,
}

fn paginate_kiro_account_views(
    accounts: Vec<KiroAccountView>,
    offset: usize,
    limit: usize,
) -> PaginatedKiroAccountViews {
    let total = accounts.len();
    let accounts = accounts.into_iter().skip(offset).take(limit).collect();
    PaginatedKiroAccountViews {
        accounts,
        total,
        limit,
        offset,
    }
}

fn build_admin_kiro_account_statuses_response(
    accounts: &[KiroAccountView],
    query: &AdminKiroAccountStatusesQuery,
    generated_at: i64,
) -> AdminKiroAccountStatusesResponse {
    let limit = normalize_admin_kiro_account_status_limit(query.limit);
    let offset = query.offset.unwrap_or(0);
    let filtered = filter_kiro_account_views_by_prefix(accounts, query.prefix.as_deref());
    let page = paginate_kiro_account_views(filtered, offset, limit);
    AdminKiroAccountStatusesResponse {
        accounts: page.accounts,
        total: page.total,
        limit: page.limit,
        offset: page.offset,
        generated_at,
    }
}

pub async fn list_admin_account_statuses(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<AdminKiroAccountStatusesQuery>,
) -> Result<Json<AdminKiroAccountStatusesResponse>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let accounts = build_account_views(&state).await;
    Ok(Json(build_admin_kiro_account_statuses_response(&accounts, &query, now_ms())))
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
    let minimum_remaining_credits_before_block = request
        .minimum_remaining_credits_before_block
        .unwrap_or_else(|| auth.effective_minimum_remaining_credits_before_block());
    if !minimum_remaining_credits_before_block.is_finite()
        || minimum_remaining_credits_before_block < 0.0
    {
        return Err(bad_request("minimum_remaining_credits_before_block must be >= 0"));
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
    auth.minimum_remaining_credits_before_block = Some(minimum_remaining_credits_before_block);
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
        minimum_remaining_credits_before_block =
            saved.effective_minimum_remaining_credits_before_block(),
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
    let refresh_interval_seconds = state
        .llm_gateway_runtime_config
        .read()
        .kiro_status_refresh_max_interval_seconds;
    let mut views = Vec::with_capacity(auths.len());
    for auth in auths {
        let (balance, cache) =
            cached_status_parts(cached.accounts.get(&auth.name), refresh_interval_seconds);
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
    let refresh_interval_seconds = state
        .llm_gateway_runtime_config
        .read()
        .kiro_status_refresh_max_interval_seconds;
    let (balance, cache) = cached_status_parts(cached.accounts.get(name), refresh_interval_seconds);
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

/// Split a cached account status entry into its balance and cache-health
/// components, returning sensible defaults when no entry exists yet.
fn cached_status_parts(
    entry: Option<&KiroCachedAccountStatus>,
    refresh_interval_seconds: u64,
) -> (Option<KiroBalanceView>, KiroCacheView) {
    entry
        .map(|status| (status.balance.clone(), status.cache.clone()))
        .unwrap_or_else(|| {
            (None, KiroCacheView {
                status: "loading".to_string(),
                refresh_interval_seconds,
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
    _effective_policy: &static_flow_shared::llm_gateway_store::KiroCachePolicy,
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
    let should_log_zero_cache = should_log_kiro_zero_cache_usage(200, usage);
    if should_log_zero_cache {
        log_kiro_zero_cache_usage(
            &current,
            event_context,
            latency_ms,
            usage,
            usage_missing,
            current.kiro_zero_cache_debug_enabled,
        );
    }
    let runtime_config = state.llm_gateway_runtime_config.read().clone();
    let effective_billable_model_multipliers =
        resolve_effective_kiro_billable_model_multipliers(&runtime_config, &current)?;
    let event = build_kiro_usage_event_record(
        KiroUsageEventBuild {
            current: &current,
            event_context,
            effective_billable_model_multipliers: &effective_billable_model_multipliers,
        },
        latency_ms,
        200,
        usage,
        usage_missing,
        event_context.last_message_content.clone(),
    );
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
        captured_canonical_request = event.full_request_json.is_some(),
        captured_diagnostic_request_bodies = event.client_request_body_json.is_some()
            || event.upstream_request_body_json.is_some(),
        "persisted kiro usage event"
    );
    Ok(())
}

pub struct FailedKiroRequestEvent<'a> {
    pub _effective_policy: &'a static_flow_shared::llm_gateway_store::KiroCachePolicy,
    pub status_code: i32,
    pub diagnostic_payload: String,
    pub usage: KiroUsageSummary,
    pub usage_missing: bool,
}

pub async fn record_failed_request_event(
    state: &AppState,
    key: &LlmGatewayKeyRecord,
    event_context: &KiroEventContext,
    failure: FailedKiroRequestEvent<'_>,
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
    let runtime_config = state.llm_gateway_runtime_config.read().clone();
    let effective_billable_model_multipliers =
        resolve_effective_kiro_billable_model_multipliers(&runtime_config, &current)?;
    let event = build_kiro_usage_event_record(
        KiroUsageEventBuild {
            current: &current,
            event_context,
            effective_billable_model_multipliers: &effective_billable_model_multipliers,
        },
        latency_ms,
        failure.status_code,
        failure.usage,
        failure.usage_missing,
        Some(failure.diagnostic_payload),
    );
    let _updated = state
        .llm_gateway
        .append_usage_event(&current, &event)
        .await?;
    tracing::warn!(
        key_id = %current.id,
        key_name = %current.name,
        account_name = event.account_name.as_deref().unwrap_or("unknown"),
        endpoint = %event.endpoint,
        request_url = %event.request_url,
        status_code = event.status_code,
        latency_ms = event.latency_ms,
        captured_canonical_request = event.full_request_json.is_some(),
        captured_diagnostic_request_bodies = event.client_request_body_json.is_some()
            || event.upstream_request_body_json.is_some(),
        "persisted kiro failure usage event"
    );
    Ok(())
}

struct KiroUsageEventBuild<'a> {
    current: &'a LlmGatewayKeyRecord,
    event_context: &'a KiroEventContext,
    effective_billable_model_multipliers: &'a BTreeMap<String, f64>,
}

fn should_log_kiro_zero_cache_usage(status_code: i32, usage: KiroUsageSummary) -> bool {
    status_code < 400 && usage.input_cached_tokens <= 0
}

fn should_capture_kiro_request_details(
    key: &LlmGatewayKeyRecord,
    status_code: i32,
    usage: KiroUsageSummary,
) -> bool {
    status_code >= 400
        || (key.kiro_zero_cache_debug_enabled
            && should_log_kiro_zero_cache_usage(status_code, usage))
}

fn log_kiro_zero_cache_usage(
    key: &LlmGatewayKeyRecord,
    event_context: &KiroEventContext,
    latency_ms: i32,
    usage: KiroUsageSummary,
    usage_missing: bool,
    include_full_request_data: bool,
) {
    let last_message_content_len = event_context
        .last_message_content
        .as_ref()
        .map(|value| value.len())
        .unwrap_or(0);
    let client_request_body_len = event_context
        .client_request_body_json
        .as_ref()
        .map(|value| value.len())
        .unwrap_or(0);
    let upstream_request_body_len = event_context
        .upstream_request_body_json
        .as_ref()
        .map(|value| value.len())
        .unwrap_or(0);

    if include_full_request_data {
        tracing::warn!(
            key_id = %key.id,
            key_name = %key.name,
            account_name = event_context.account_name.as_deref().unwrap_or("unknown"),
            endpoint = %event_context.endpoint,
            request_url = %event_context.request_url,
            model = event_context.model.as_deref().unwrap_or("unknown"),
            latency_ms,
            input_uncached_tokens = usage.input_uncached_tokens,
            input_cached_tokens = usage.input_cached_tokens,
            output_tokens = usage.output_tokens,
            credit_usage = usage.credit_usage.unwrap_or_default(),
            credit_usage_missing = usage.credit_usage_missing,
            usage_missing,
            conversation_id = event_context.conversation_id.as_deref().unwrap_or("unknown"),
            session_resolution = event_context.session_resolution.as_deref().unwrap_or("unknown"),
            session_source_name = event_context.session_source_name.as_deref().unwrap_or("unknown"),
            session_source_value_preview =
                event_context.session_source_value_preview.as_deref().unwrap_or("unknown"),
            request_headers_json = %event_context.request_headers_json,
            client_request_body_json =
                %event_context.client_request_body_json.as_deref().unwrap_or("<missing>"),
            upstream_request_body_json =
                %event_context.upstream_request_body_json.as_deref().unwrap_or("<missing>"),
            "kiro zero-cache usage detected with full request diagnostics"
        );
    } else {
        tracing::warn!(
            key_id = %key.id,
            key_name = %key.name,
            account_name = event_context.account_name.as_deref().unwrap_or("unknown"),
            endpoint = %event_context.endpoint,
            request_url = %event_context.request_url,
            model = event_context.model.as_deref().unwrap_or("unknown"),
            latency_ms,
            input_uncached_tokens = usage.input_uncached_tokens,
            input_cached_tokens = usage.input_cached_tokens,
            output_tokens = usage.output_tokens,
            credit_usage = usage.credit_usage.unwrap_or_default(),
            credit_usage_missing = usage.credit_usage_missing,
            usage_missing,
            conversation_id = event_context.conversation_id.as_deref().unwrap_or("unknown"),
            session_resolution = event_context.session_resolution.as_deref().unwrap_or("unknown"),
            session_source_name = event_context.session_source_name.as_deref().unwrap_or("unknown"),
            session_source_value_preview =
                event_context.session_source_value_preview.as_deref().unwrap_or("unknown"),
            last_message_content_len,
            client_request_body_len,
            upstream_request_body_len,
            zero_cache_debug_enabled = false,
            "kiro zero-cache usage detected"
        );
    }
}

fn build_kiro_usage_event_record(
    build: KiroUsageEventBuild<'_>,
    latency_ms: i32,
    status_code: i32,
    usage: KiroUsageSummary,
    usage_missing: bool,
    last_message_content: Option<String>,
) -> LlmGatewayUsageEventRecord {
    let capture_request_details =
        should_capture_kiro_request_details(build.current, status_code, usage);
    LlmGatewayUsageEventRecord {
        id: generate_task_id("kiro-usage"),
        key_id: build.current.id.clone(),
        key_name: build.current.name.clone(),
        provider_type: LLM_GATEWAY_PROVIDER_KIRO.to_string(),
        account_name: build.event_context.account_name.clone(),
        request_method: build.event_context.request_method.clone(),
        request_url: build.event_context.request_url.clone(),
        latency_ms,
        endpoint: build.event_context.endpoint.clone(),
        model: build.event_context.model.clone(),
        status_code,
        input_uncached_tokens: usage.input_uncached_tokens.max(0) as u64,
        input_cached_tokens: usage.input_cached_tokens.max(0) as u64,
        output_tokens: usage.output_tokens.max(0) as u64,
        billable_tokens: compute_kiro_billable_tokens(
            build.event_context.model.as_deref(),
            usage.input_uncached_tokens.max(0) as u64,
            usage.input_cached_tokens.max(0) as u64,
            usage.output_tokens.max(0) as u64,
            build.effective_billable_model_multipliers,
        ),
        usage_missing,
        credit_usage: usage.credit_usage,
        credit_usage_missing: usage.credit_usage_missing,
        client_ip: build.event_context.client_ip.clone(),
        ip_region: build.event_context.ip_region.clone(),
        request_headers_json: build.event_context.request_headers_json.clone(),
        last_message_content,
        client_request_body_json: capture_request_details
            .then(|| build.event_context.client_request_body_json.clone())
            .flatten(),
        upstream_request_body_json: capture_request_details
            .then(|| build.event_context.upstream_request_body_json.clone())
            .flatten(),
        full_request_json: capture_request_details
            .then(|| build.event_context.client_request_body_json.clone())
            .flatten(),
        created_at: now_ms(),
    }
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
    let minimum_remaining_credits_before_block = request
        .minimum_remaining_credits_before_block
        .unwrap_or(0.0);
    if !minimum_remaining_credits_before_block.is_finite()
        || minimum_remaining_credits_before_block < 0.0
    {
        return Err(bad_request("minimum_remaining_credits_before_block must be >= 0"));
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
        minimum_remaining_credits_before_block: Some(minimum_remaining_credits_before_block),
        proxy_mode: Default::default(),
        proxy_config_id: None,
        proxy_url: None,
        proxy_username: None,
        proxy_password: None,
        disabled: request.disabled,
        disabled_reason: request.disabled.then(|| "manual".to_string()),
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

fn normalize_route_strategy_input(value: Option<&str>) -> anyhow::Result<Option<String>> {
    let Some(trimmed) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };

    match trimmed {
        "auto" | "fixed" => Ok(Some(trimmed.to_string())),
        _ => anyhow::bail!("route_strategy must be `auto` or `fixed`"),
    }
}

fn normalize_optional_account_name_input(value: Option<&str>) -> anyhow::Result<Option<String>> {
    let Some(trimmed) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };

    crate::llm_gateway::accounts::validate_account_name(trimmed)
        .map(Some)
        .map_err(anyhow::Error::msg)
}

async fn load_kiro_account_group(
    state: &AppState,
    group_id: &str,
) -> KiroAdminResult<LlmGatewayAccountGroupRecord> {
    let group = state
        .llm_gateway_store
        .get_account_group_by_id(group_id)
        .await
        .map_err(|err| internal_error("Failed to load Kiro account group", err))?
        .ok_or_else(|| bad_request("account_group_id does not exist"))?;
    if group.provider_type != LLM_GATEWAY_PROVIDER_KIRO {
        return Err(bad_request("account_group_id belongs to a different provider"));
    }
    Ok(group)
}

type NormalizedKiroKeyRouteConfig = (Option<String>, Option<String>, Option<Vec<String>>);

fn normalize_auto_account_names_input(
    value: Option<Vec<String>>,
) -> anyhow::Result<Option<Vec<String>>> {
    let Some(values) = value else {
        return Ok(None);
    };

    let mut names = values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(|value| {
            crate::llm_gateway::accounts::validate_account_name(&value).map_err(anyhow::Error::msg)
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    names.sort();
    names.dedup();
    if names.is_empty() {
        return Ok(None);
    }
    Ok(Some(names))
}

async fn normalize_kiro_account_group_members(
    state: &AppState,
    account_names: Vec<String>,
) -> KiroAdminResult<Vec<String>> {
    let names = normalize_auto_account_names_input(Some(account_names))
        .map_err(|err| bad_request(&err.to_string()))?
        .ok_or_else(|| bad_request("account_names must not be empty"))?;
    let existing_account_names = state
        .kiro_gateway
        .token_manager
        .list_auths()
        .await
        .map_err(|err| internal_error("Failed to load Kiro accounts", err))?
        .into_iter()
        .map(|auth| auth.name)
        .collect::<BTreeSet<_>>();
    for name in &names {
        if !existing_account_names.contains(name) {
            return Err(bad_request(&format!("unknown account `{name}`")));
        }
    }
    Ok(names)
}

async fn create_kiro_account_group_for_key_subset(
    state: &AppState,
    key_name: &str,
    key_id: &str,
    account_names: Vec<String>,
) -> KiroAdminResult<LlmGatewayAccountGroupRecord> {
    let now = now_ms();
    let record = LlmGatewayAccountGroupRecord {
        id: generate_task_id("kiro-group"),
        provider_type: LLM_GATEWAY_PROVIDER_KIRO.to_string(),
        name: format!("Migrated {} {}", key_name, &key_id[..key_id.len().min(8)]),
        account_names,
        created_at: now,
        updated_at: now,
    };
    state
        .llm_gateway_store
        .create_account_group(&record)
        .await
        .map_err(|err| internal_error("Failed to create Kiro account group", err))?;
    Ok(record)
}

async fn materialize_legacy_kiro_route_group_if_needed(
    state: &AppState,
    key: &mut LlmGatewayKeyRecord,
) -> KiroAdminResult<()> {
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
            let group = create_kiro_account_group_for_key_subset(state, &key.name, &key.id, vec![
                account_name,
            ])
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
            let account_names = normalize_kiro_account_group_members(state, account_names).await?;
            let group =
                create_kiro_account_group_for_key_subset(state, &key.name, &key.id, account_names)
                    .await?;
            key.account_group_id = Some(group.id);
            key.fixed_account_name = None;
            key.auto_account_names = None;
            Ok(())
        },
        _ => Ok(()),
    }
}

async fn validate_kiro_key_group_config(
    state: &AppState,
    key: &mut LlmGatewayKeyRecord,
) -> KiroAdminResult<()> {
    match key.route_strategy.as_deref().unwrap_or("auto") {
        "fixed" => {
            let group_id = key
                .account_group_id
                .as_deref()
                .ok_or_else(|| bad_request("fixed route_strategy requires account_group_id"))?;
            let group = load_kiro_account_group(state, group_id).await?;
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
                let _ = load_kiro_account_group(state, group_id).await?;
            }
            key.fixed_account_name = None;
            key.auto_account_names = None;
            Ok(())
        },
        _ => Err(bad_request("route_strategy must be `auto` or `fixed`")),
    }
}

fn filter_known_auto_account_names(
    auto_account_names: Option<Vec<String>>,
    existing_account_names: &BTreeSet<String>,
) -> anyhow::Result<Option<Vec<String>>> {
    let filtered_auto_account_names = auto_account_names.map(|names| {
        names
            .into_iter()
            .filter(|name| existing_account_names.contains(name))
            .collect::<Vec<_>>()
    });
    if filtered_auto_account_names
        .as_ref()
        .is_some_and(|names| names.is_empty())
    {
        anyhow::bail!("none of the configured auto accounts exist anymore");
    }
    Ok(filtered_auto_account_names.filter(|names| !names.is_empty()))
}

fn normalize_key_route_config(
    route_strategy: Option<&str>,
    fixed_account_name: Option<&str>,
    auto_account_names: Option<Vec<String>>,
    existing_account_names: &BTreeSet<String>,
) -> anyhow::Result<NormalizedKiroKeyRouteConfig> {
    let route_strategy = normalize_route_strategy_input(route_strategy)?;
    let fixed_account_name = normalize_optional_account_name_input(fixed_account_name)?;
    let auto_account_names = normalize_auto_account_names_input(auto_account_names)?;

    match route_strategy.as_deref() {
        Some("fixed") => {
            let fixed_account_name = fixed_account_name.ok_or_else(|| {
                anyhow::anyhow!("fixed route_strategy requires fixed_account_name")
            })?;
            if !existing_account_names.contains(&fixed_account_name) {
                anyhow::bail!("unknown account `{fixed_account_name}`");
            }
            Ok((Some("fixed".to_string()), Some(fixed_account_name), None))
        },
        Some("auto") => {
            let auto_account_names =
                filter_known_auto_account_names(auto_account_names, existing_account_names)?;
            Ok((Some("auto".to_string()), None, auto_account_names))
        },
        None => Ok((None, None, None)),
        _ => anyhow::bail!("route_strategy must be `auto` or `fixed`"),
    }
}

fn normalize_kiro_key_route_config_for_patch(
    current_route_strategy: Option<String>,
    current_fixed_account_name: Option<String>,
    current_auto_account_names: Option<Vec<String>>,
    request_route_strategy: Option<&str>,
    request_fixed_account_name: Option<&str>,
    request_auto_account_names: Option<Vec<String>>,
    existing_account_names: &BTreeSet<String>,
) -> anyhow::Result<NormalizedKiroKeyRouteConfig> {
    let request_route_strategy = request_route_strategy
        .map(|value| normalize_route_strategy_input(Some(value)))
        .transpose()?;

    if let Some(None) = request_route_strategy {
        return Ok((None, None, None));
    }

    let request_route_strategy = request_route_strategy.flatten();
    let route_strategy = request_route_strategy
        .as_deref()
        .or(current_route_strategy.as_deref());
    let fixed_account_name = if request_fixed_account_name.is_some() {
        request_fixed_account_name
    } else {
        current_fixed_account_name.as_deref()
    };
    let auto_account_names = if request_auto_account_names.is_some() {
        request_auto_account_names
    } else {
        current_auto_account_names
    };

    normalize_key_route_config(
        route_strategy,
        fixed_account_name,
        auto_account_names,
        existing_account_names,
    )
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
    use std::collections::{BTreeMap, BTreeSet};

    use axum::http::StatusCode;
    use static_flow_shared::llm_gateway_store::{
        default_kiro_cache_policy, default_kiro_cache_policy_json, LlmGatewayKeyRecord,
    };

    use super::{
        build_admin_kiro_account_statuses_response, build_kiro_usage_event_record,
        filter_kiro_account_views_by_prefix, normalize_key_route_config,
        normalize_kiro_key_route_config_for_patch, normalize_model_name_map,
        paginate_kiro_account_views, public_kiro_access_accounts,
        resolve_effective_kiro_billable_model_multipliers,
        validate_kiro_cache_policy_override_update, AdminKiroAccountStatusesQuery,
        KiroAccessResponse, KiroAccountView, KiroCacheView, KiroEventContext, KiroUsageEventBuild,
        KiroUsageSummary,
    };

    fn test_account_view(name: &str) -> KiroAccountView {
        KiroAccountView {
            name: name.to_string(),
            auth_method: "social".to_string(),
            provider: Some("github".to_string()),
            upstream_user_id: None,
            email: None,
            expires_at: None,
            profile_arn: None,
            has_refresh_token: true,
            disabled: false,
            disabled_reason: None,
            source: None,
            source_db_path: None,
            last_imported_at: None,
            subscription_title: None,
            region: Some("us-east-1".to_string()),
            auth_region: Some("us-east-1".to_string()),
            api_region: Some("us-east-1".to_string()),
            machine_id: None,
            kiro_channel_max_concurrency: 1,
            kiro_channel_min_start_interval_ms: 0,
            minimum_remaining_credits_before_block: 0.0,
            proxy_mode: "inherit".to_string(),
            proxy_config_id: None,
            effective_proxy_source: "direct".to_string(),
            effective_proxy_url: None,
            effective_proxy_config_name: None,
            proxy_url: None,
            balance: None,
            cache: KiroCacheView::default(),
        }
    }

    fn sample_kiro_key_for_usage_event(zero_cache_debug_enabled: bool) -> LlmGatewayKeyRecord {
        LlmGatewayKeyRecord {
            id: "key-1".to_string(),
            name: "test-key".to_string(),
            secret: "secret".to_string(),
            key_hash: "hash".to_string(),
            status: "active".to_string(),
            provider_type: "kiro".to_string(),
            protocol_family: "anthropic".to_string(),
            public_visible: false,
            quota_billable_limit: 100,
            usage_input_uncached_tokens: 0,
            usage_input_cached_tokens: 0,
            usage_output_tokens: 0,
            usage_billable_tokens: 0,
            usage_credit_total: 0.0,
            usage_credit_missing_events: 0,
            last_used_at: None,
            created_at: 0,
            updated_at: 0,
            route_strategy: None,
            account_group_id: None,
            fixed_account_name: None,
            auto_account_names: None,
            model_name_map: None,
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
            kiro_request_validation_enabled: true,
            kiro_cache_estimation_enabled: true,
            kiro_zero_cache_debug_enabled: zero_cache_debug_enabled,
            kiro_cache_policy_override_json: None,
            kiro_billable_model_multipliers_override_json: None,
        }
    }

    fn sample_kiro_event_context_for_usage_event() -> KiroEventContext {
        KiroEventContext {
            account_name: Some("acct-a".to_string()),
            request_method: "POST".to_string(),
            request_url: "/api/kiro-gateway/v1/messages".to_string(),
            endpoint: "/generateAssistantResponse".to_string(),
            model: Some("claude-opus-4-6".to_string()),
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: "[]".to_string(),
            last_message_content: Some("hello".to_string()),
            client_request_body_json: Some("{\"messages\":[]}".to_string()),
            upstream_request_body_json: Some(
                "{\"conversationState\":{\"conversationId\":\"conv-1\"}}".to_string(),
            ),
            conversation_id: Some("conv-1".to_string()),
            session_resolution: Some("metadata_json".to_string()),
            session_source_name: Some("session_id".to_string()),
            session_source_value_preview: Some("conv-1".to_string()),
            started_at: std::time::Instant::now(),
        }
    }

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

    #[test]
    fn filter_kiro_account_views_by_prefix_trims_and_matches_case_insensitively() {
        let accounts = vec![
            test_account_view("Alpha"),
            test_account_view("alpha-two"),
            test_account_view("beta"),
        ];

        let filtered = filter_kiro_account_views_by_prefix(&accounts, Some("  ALpHa "));

        assert_eq!(
            filtered
                .iter()
                .map(|item| item.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Alpha", "alpha-two"]
        );
    }

    #[test]
    fn paginate_kiro_account_views_returns_total_and_slice() {
        let accounts =
            vec![test_account_view("alpha"), test_account_view("beta"), test_account_view("gamma")];

        let page = paginate_kiro_account_views(accounts, 1, 1);

        assert_eq!(page.total, 3);
        assert_eq!(page.offset, 1);
        assert_eq!(page.limit, 1);
        assert_eq!(page.accounts.len(), 1);
        assert_eq!(page.accounts[0].name, "beta");
    }

    #[test]
    fn public_kiro_access_accounts_are_always_empty() {
        let response = KiroAccessResponse {
            base_url: "https://example.com/api/kiro-gateway".to_string(),
            gateway_path: "/api/kiro-gateway".to_string(),
            auth_cache_ttl_seconds: 60,
            accounts: public_kiro_access_accounts(),
            generated_at: 0,
        };

        assert!(response.accounts.is_empty());
    }

    #[test]
    fn build_admin_kiro_account_statuses_response_applies_prefix_and_window_metadata() {
        let accounts = vec![
            test_account_view("alpha"),
            test_account_view("beta"),
            test_account_view("gamma"),
            test_account_view("delta"),
        ];
        let response = build_admin_kiro_account_statuses_response(
            &accounts,
            &AdminKiroAccountStatusesQuery {
                prefix: Some("g".to_string()),
                limit: None,
                offset: Some(0),
            },
            0,
        );

        assert_eq!(response.total, 1);
        assert_eq!(response.limit, 24);
        assert_eq!(response.offset, 0);
        assert_eq!(response.accounts.len(), 1);
        assert_eq!(response.accounts[0].name, "gamma");
    }

    #[test]
    fn normalize_key_route_config_keeps_auto_without_subset_as_full_pool() {
        let existing = BTreeSet::from(["alpha".to_string(), "beta".to_string()]);

        let normalized = normalize_key_route_config(Some("auto"), None, Some(vec![]), &existing)
            .expect("normalize should succeed");

        assert_eq!(normalized, (Some("auto".to_string()), None, None));
    }

    #[test]
    fn normalize_key_route_config_requires_fixed_account_name() {
        let existing = BTreeSet::from(["alpha".to_string()]);

        let err = normalize_key_route_config(Some("fixed"), None, None, &existing)
            .expect_err("fixed without account should fail");

        assert!(err
            .to_string()
            .contains("fixed route_strategy requires fixed_account_name"));
    }

    #[test]
    fn normalize_key_route_config_filters_unknown_auto_accounts() {
        let existing = BTreeSet::from(["alpha".to_string(), "beta".to_string()]);

        let normalized = normalize_key_route_config(
            Some("auto"),
            None,
            Some(vec![
                "beta".to_string(),
                "missing".to_string(),
                "alpha".to_string(),
                "beta".to_string(),
            ]),
            &existing,
        )
        .expect("normalize should succeed");

        assert_eq!(
            normalized,
            (Some("auto".to_string()), None, Some(vec!["alpha".to_string(), "beta".to_string()]),)
        );
    }

    #[test]
    fn normalize_key_route_config_rejects_auto_subset_when_all_accounts_are_unknown() {
        let existing = BTreeSet::from(["alpha".to_string()]);

        let err = normalize_key_route_config(
            Some("auto"),
            None,
            Some(vec!["missing".to_string()]),
            &existing,
        )
        .expect_err("unknown subset should fail");

        assert!(err
            .to_string()
            .contains("none of the configured auto accounts exist anymore"));
    }

    #[test]
    fn normalize_key_route_config_empty_strategy_normalizes_to_none() {
        let existing = BTreeSet::from(["alpha".to_string()]);

        let normalized = normalize_key_route_config(Some(""), None, None, &existing)
            .expect("normalize should succeed");

        assert_eq!(normalized, (None, None, None));
    }

    #[test]
    fn normalize_kiro_key_route_config_for_patch_keeps_fixed_route_when_only_fixed_account_updates()
    {
        let existing = BTreeSet::from(["alpha".to_string(), "beta".to_string()]);

        let normalized = normalize_kiro_key_route_config_for_patch(
            Some("fixed".to_string()),
            Some("alpha".to_string()),
            None,
            None,
            Some("beta"),
            None,
            &existing,
        )
        .expect("normalize should succeed");

        assert_eq!(normalized, (Some("fixed".to_string()), Some("beta".to_string()), None));
    }

    #[test]
    fn normalize_kiro_key_route_config_for_patch_keeps_auto_route_when_only_subset_updates() {
        let existing =
            BTreeSet::from(["alpha".to_string(), "beta".to_string(), "gamma".to_string()]);

        let normalized = normalize_kiro_key_route_config_for_patch(
            Some("auto".to_string()),
            None,
            Some(vec!["alpha".to_string(), "beta".to_string()]),
            None,
            None,
            Some(vec!["beta".to_string(), "missing".to_string()]),
            &existing,
        )
        .expect("normalize should succeed");

        assert_eq!(normalized, (Some("auto".to_string()), None, Some(vec!["beta".to_string()])));
    }

    #[test]
    fn normalize_kiro_key_route_config_for_patch_empty_strategy_clears_fixed_to_default() {
        let existing = BTreeSet::from(["alpha".to_string()]);

        let normalized = normalize_kiro_key_route_config_for_patch(
            Some("fixed".to_string()),
            Some("alpha".to_string()),
            None,
            Some(""),
            None,
            None,
            &existing,
        )
        .expect("normalize should succeed");

        assert_eq!(normalized, (None, None, None));
    }

    #[test]
    fn normalize_kiro_key_route_config_for_patch_empty_strategy_clears_auto_subset_to_default() {
        let existing = BTreeSet::from(["alpha".to_string(), "beta".to_string()]);

        let normalized = normalize_kiro_key_route_config_for_patch(
            Some("auto".to_string()),
            None,
            Some(vec!["alpha".to_string(), "beta".to_string()]),
            Some(""),
            None,
            None,
            &existing,
        )
        .expect("normalize should succeed");

        assert_eq!(normalized, (None, None, None));
    }

    #[test]
    fn normalize_kiro_key_route_config_for_patch_empty_strategy_clears_auto_subset_with_stale_accounts(
    ) {
        let existing = BTreeSet::new();

        let normalized = normalize_kiro_key_route_config_for_patch(
            Some("auto".to_string()),
            None,
            Some(vec!["missing-alpha".to_string(), "missing-beta".to_string()]),
            Some(""),
            None,
            None,
            &existing,
        )
        .expect("normalize should succeed");

        assert_eq!(normalized, (None, None, None));
    }

    #[test]
    fn validate_kiro_cache_policy_override_update_rejects_invalid_effective_policy() {
        let runtime_config = crate::state::LlmGatewayRuntimeConfig {
            kiro_cache_policy_json: default_kiro_cache_policy_json(),
            kiro_cache_policy: default_kiro_cache_policy(),
            ..crate::state::LlmGatewayRuntimeConfig::default()
        };
        let key = LlmGatewayKeyRecord {
            id: "key-1".to_string(),
            name: "test-key".to_string(),
            secret: "secret".to_string(),
            key_hash: "hash".to_string(),
            status: "active".to_string(),
            provider_type: "kiro".to_string(),
            protocol_family: "anthropic".to_string(),
            public_visible: false,
            quota_billable_limit: 100,
            usage_input_uncached_tokens: 0,
            usage_input_cached_tokens: 0,
            usage_output_tokens: 0,
            usage_billable_tokens: 0,
            usage_credit_total: 0.0,
            usage_credit_missing_events: 0,
            last_used_at: None,
            created_at: 0,
            updated_at: 0,
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
        };

        let err = validate_kiro_cache_policy_override_update(
            &runtime_config,
            &key,
            Some(r#"{"small_input_high_credit_boost":{"credit_end":0.5}}"#),
        )
        .expect_err("merged invalid override should be rejected");

        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert_eq!(err.1 .0.error, "kiro_cache_policy_override_json is invalid");
    }

    #[test]
    fn normalize_kiro_key_route_config_for_patch_rejects_unknown_fixed_account() {
        let existing = BTreeSet::from(["alpha".to_string()]);

        let err = normalize_kiro_key_route_config_for_patch(
            Some("fixed".to_string()),
            Some("alpha".to_string()),
            None,
            None,
            Some("missing"),
            None,
            &existing,
        )
        .expect_err("unknown fixed account should fail");

        assert!(err.to_string().contains("unknown account `missing`"));
    }

    #[test]
    fn build_kiro_failure_usage_event_preserves_status_and_full_request_payloads() {
        let key = LlmGatewayKeyRecord {
            id: "key-1".to_string(),
            name: "test-key".to_string(),
            secret: "secret".to_string(),
            key_hash: "hash".to_string(),
            status: "active".to_string(),
            provider_type: "kiro".to_string(),
            protocol_family: "anthropic".to_string(),
            public_visible: false,
            quota_billable_limit: 100,
            usage_input_uncached_tokens: 0,
            usage_input_cached_tokens: 0,
            usage_output_tokens: 0,
            usage_billable_tokens: 0,
            usage_credit_total: 0.0,
            usage_credit_missing_events: 0,
            last_used_at: None,
            created_at: 0,
            updated_at: 0,
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
        };
        let event_context = KiroEventContext {
            account_name: Some("acct-a".to_string()),
            request_method: "POST".to_string(),
            request_url: "/api/kiro-gateway/v1/messages".to_string(),
            endpoint: "/generateAssistantResponse".to_string(),
            model: Some("claude-sonnet-4-6".to_string()),
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: "[]".to_string(),
            last_message_content: Some("hello".to_string()),
            client_request_body_json: Some("{\"messages\":[]}".to_string()),
            upstream_request_body_json: Some(
                "{\"conversationState\":{\"conversationId\":\"conv-1\"}}".to_string(),
            ),
            conversation_id: Some("conv-1".to_string()),
            session_resolution: Some("request_header".to_string()),
            session_source_name: Some("x-claude-code-session-id".to_string()),
            session_source_value_preview: Some("1234...".to_string()),
            started_at: std::time::Instant::now(),
        };
        let diagnostic = r#"{"kind":"kiro_failure_diagnostic","error":"boom"}"#.to_string();
        let runtime_config = crate::state::LlmGatewayRuntimeConfig::default();

        let record = build_kiro_usage_event_record(
            KiroUsageEventBuild {
                current: &key,
                event_context: &event_context,
                effective_billable_model_multipliers: &runtime_config
                    .kiro_billable_model_multipliers,
            },
            12,
            502,
            KiroUsageSummary {
                input_uncached_tokens: 0,
                input_cached_tokens: 0,
                output_tokens: 0,
                credit_usage: None,
                credit_usage_missing: false,
            },
            false,
            Some(diagnostic.clone()),
        );

        assert_eq!(record.status_code, 502);
        assert_eq!(record.last_message_content.as_deref(), Some(diagnostic.as_str()));
        assert_eq!(record.account_name.as_deref(), Some("acct-a"));
        assert_eq!(record.billable_tokens, 0);
        assert_eq!(record.request_headers_json, "[]");
        assert_eq!(record.client_request_body_json.as_deref(), Some("{\"messages\":[]}"));
        assert_eq!(
            record.upstream_request_body_json.as_deref(),
            Some("{\"conversationState\":{\"conversationId\":\"conv-1\"}}")
        );
        assert_eq!(record.full_request_json.as_deref(), Some("{\"messages\":[]}"));
    }

    #[test]
    fn build_kiro_success_usage_event_skips_full_request_payloads_for_high_credit() {
        let key = LlmGatewayKeyRecord {
            id: "key-1".to_string(),
            name: "test-key".to_string(),
            secret: "secret".to_string(),
            key_hash: "hash".to_string(),
            status: "active".to_string(),
            provider_type: "kiro".to_string(),
            protocol_family: "anthropic".to_string(),
            public_visible: false,
            quota_billable_limit: 100,
            usage_input_uncached_tokens: 0,
            usage_input_cached_tokens: 0,
            usage_output_tokens: 0,
            usage_billable_tokens: 0,
            usage_credit_total: 0.0,
            usage_credit_missing_events: 0,
            last_used_at: None,
            created_at: 0,
            updated_at: 0,
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
        };
        let event_context = KiroEventContext {
            account_name: Some("acct-a".to_string()),
            request_method: "POST".to_string(),
            request_url: "/api/kiro-gateway/v1/messages".to_string(),
            endpoint: "/generateAssistantResponse".to_string(),
            model: Some("claude-opus-4-6".to_string()),
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: "[]".to_string(),
            last_message_content: Some("hello".to_string()),
            client_request_body_json: Some("{\"messages\":[]}".to_string()),
            upstream_request_body_json: Some(
                "{\"conversationState\":{\"conversationId\":\"conv-1\"}}".to_string(),
            ),
            conversation_id: Some("conv-1".to_string()),
            session_resolution: Some("metadata_json".to_string()),
            session_source_name: Some("session_id".to_string()),
            session_source_value_preview: Some("conv-1".to_string()),
            started_at: std::time::Instant::now(),
        };
        let runtime_config = crate::state::LlmGatewayRuntimeConfig::default();

        let record = build_kiro_usage_event_record(
            KiroUsageEventBuild {
                current: &key,
                event_context: &event_context,
                effective_billable_model_multipliers: &runtime_config
                    .kiro_billable_model_multipliers,
            },
            42,
            200,
            KiroUsageSummary {
                input_uncached_tokens: 50_049,
                input_cached_tokens: 0,
                output_tokens: 926,
                credit_usage: Some(2.5598),
                credit_usage_missing: false,
            },
            false,
            event_context.last_message_content.clone(),
        );

        assert_eq!(record.request_headers_json, "[]");
        assert!(record.client_request_body_json.is_none());
        assert!(record.upstream_request_body_json.is_none());
        assert!(record.full_request_json.is_none());
        assert_eq!(record.billable_tokens, 54_679);
    }

    #[test]
    fn build_kiro_zero_cache_success_usage_event_captures_full_request_when_key_debug_is_enabled() {
        let key = sample_kiro_key_for_usage_event(true);
        let event_context = sample_kiro_event_context_for_usage_event();
        let runtime_config = crate::state::LlmGatewayRuntimeConfig::default();

        let record = build_kiro_usage_event_record(
            KiroUsageEventBuild {
                current: &key,
                event_context: &event_context,
                effective_billable_model_multipliers: &runtime_config
                    .kiro_billable_model_multipliers,
            },
            42,
            200,
            KiroUsageSummary {
                input_uncached_tokens: 50_049,
                input_cached_tokens: 0,
                output_tokens: 926,
                credit_usage: Some(1.9999),
                credit_usage_missing: false,
            },
            false,
            event_context.last_message_content.clone(),
        );

        assert_eq!(record.client_request_body_json.as_deref(), Some("{\"messages\":[]}"));
        assert_eq!(
            record.upstream_request_body_json.as_deref(),
            Some("{\"conversationState\":{\"conversationId\":\"conv-1\"}}")
        );
        assert_eq!(record.full_request_json.as_deref(), Some("{\"messages\":[]}"));
    }

    #[test]
    fn build_kiro_zero_cache_success_usage_event_skips_full_request_when_key_debug_is_disabled() {
        let key = sample_kiro_key_for_usage_event(false);
        let event_context = sample_kiro_event_context_for_usage_event();
        let runtime_config = crate::state::LlmGatewayRuntimeConfig::default();

        let record = build_kiro_usage_event_record(
            KiroUsageEventBuild {
                current: &key,
                event_context: &event_context,
                effective_billable_model_multipliers: &runtime_config
                    .kiro_billable_model_multipliers,
            },
            42,
            200,
            KiroUsageSummary {
                input_uncached_tokens: 50_049,
                input_cached_tokens: 0,
                output_tokens: 926,
                credit_usage: Some(1.9999),
                credit_usage_missing: false,
            },
            false,
            event_context.last_message_content.clone(),
        );

        assert!(record.client_request_body_json.is_none());
        assert!(record.upstream_request_body_json.is_none());
        assert!(record.full_request_json.is_none());
    }

    #[test]
    fn build_kiro_cached_success_usage_event_skips_full_request_when_key_debug_is_enabled() {
        let key = sample_kiro_key_for_usage_event(true);
        let event_context = sample_kiro_event_context_for_usage_event();
        let runtime_config = crate::state::LlmGatewayRuntimeConfig::default();

        let record = build_kiro_usage_event_record(
            KiroUsageEventBuild {
                current: &key,
                event_context: &event_context,
                effective_billable_model_multipliers: &runtime_config
                    .kiro_billable_model_multipliers,
            },
            42,
            200,
            KiroUsageSummary {
                input_uncached_tokens: 45_000,
                input_cached_tokens: 5_000,
                output_tokens: 926,
                credit_usage: Some(1.9999),
                credit_usage_missing: false,
            },
            false,
            event_context.last_message_content.clone(),
        );

        assert!(record.client_request_body_json.is_none());
        assert!(record.upstream_request_body_json.is_none());
        assert!(record.full_request_json.is_none());
    }

    #[test]
    fn build_kiro_success_usage_event_applies_configured_model_multiplier() {
        let key = LlmGatewayKeyRecord {
            id: "key-1".to_string(),
            name: "test-key".to_string(),
            secret: "secret".to_string(),
            key_hash: "hash".to_string(),
            status: "active".to_string(),
            provider_type: "kiro".to_string(),
            protocol_family: "anthropic".to_string(),
            public_visible: false,
            quota_billable_limit: 100,
            usage_input_uncached_tokens: 0,
            usage_input_cached_tokens: 0,
            usage_output_tokens: 0,
            usage_billable_tokens: 0,
            usage_credit_total: 0.0,
            usage_credit_missing_events: 0,
            last_used_at: None,
            created_at: 0,
            updated_at: 0,
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
        };
        let event_context = KiroEventContext {
            account_name: Some("acct-a".to_string()),
            request_method: "POST".to_string(),
            request_url: "/api/kiro-gateway/v1/messages".to_string(),
            endpoint: "/generateAssistantResponse".to_string(),
            model: Some("claude-opus-4-6".to_string()),
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: "[]".to_string(),
            last_message_content: Some("hello".to_string()),
            client_request_body_json: Some("{\"messages\":[]}".to_string()),
            upstream_request_body_json: Some(
                "{\"conversationState\":{\"conversationId\":\"conv-1\"}}".to_string(),
            ),
            conversation_id: Some("conv-1".to_string()),
            session_resolution: Some("metadata_json".to_string()),
            session_source_name: Some("session_id".to_string()),
            session_source_value_preview: Some("conv-1".to_string()),
            started_at: std::time::Instant::now(),
        };
        let mut runtime_config = crate::state::LlmGatewayRuntimeConfig::default();
        runtime_config
            .kiro_billable_model_multipliers
            .insert("opus".to_string(), 2.0);
        let effective_billable_model_multipliers =
            resolve_effective_kiro_billable_model_multipliers(&runtime_config, &key)
                .expect("key override should resolve");

        let record = build_kiro_usage_event_record(
            KiroUsageEventBuild {
                current: &key,
                event_context: &event_context,
                effective_billable_model_multipliers: &effective_billable_model_multipliers,
            },
            42,
            200,
            KiroUsageSummary {
                input_uncached_tokens: 50_049,
                input_cached_tokens: 0,
                output_tokens: 926,
                credit_usage: Some(2.5598),
                credit_usage_missing: false,
            },
            false,
            event_context.last_message_content.clone(),
        );

        assert_eq!(record.billable_tokens, 109_358);
    }

    #[test]
    fn build_kiro_success_usage_event_prefers_key_multiplier_override_over_global() {
        let mut key = LlmGatewayKeyRecord {
            id: "key-1".to_string(),
            name: "test-key".to_string(),
            secret: "secret".to_string(),
            key_hash: "hash".to_string(),
            status: "active".to_string(),
            provider_type: "kiro".to_string(),
            protocol_family: "anthropic".to_string(),
            public_visible: false,
            quota_billable_limit: 100,
            usage_input_uncached_tokens: 0,
            usage_input_cached_tokens: 0,
            usage_output_tokens: 0,
            usage_billable_tokens: 0,
            usage_credit_total: 0.0,
            usage_credit_missing_events: 0,
            last_used_at: None,
            created_at: 0,
            updated_at: 0,
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
        };
        key.kiro_billable_model_multipliers_override_json = Some(r#"{"opus":1.5}"#.to_string());
        let event_context = KiroEventContext {
            account_name: Some("acct-a".to_string()),
            request_method: "POST".to_string(),
            request_url: "/api/kiro-gateway/v1/messages".to_string(),
            endpoint: "/generateAssistantResponse".to_string(),
            model: Some("claude-opus-4-6".to_string()),
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: "[]".to_string(),
            last_message_content: Some("hello".to_string()),
            client_request_body_json: Some("{\"messages\":[]}".to_string()),
            upstream_request_body_json: Some(
                "{\"conversationState\":{\"conversationId\":\"conv-1\"}}".to_string(),
            ),
            conversation_id: Some("conv-1".to_string()),
            session_resolution: Some("metadata_json".to_string()),
            session_source_name: Some("session_id".to_string()),
            session_source_value_preview: Some("conv-1".to_string()),
            started_at: std::time::Instant::now(),
        };
        let mut runtime_config = crate::state::LlmGatewayRuntimeConfig::default();
        runtime_config
            .kiro_billable_model_multipliers
            .insert("opus".to_string(), 2.0);
        let effective_billable_model_multipliers =
            resolve_effective_kiro_billable_model_multipliers(&runtime_config, &key)
                .expect("key override should resolve");

        let record = build_kiro_usage_event_record(
            KiroUsageEventBuild {
                current: &key,
                event_context: &event_context,
                effective_billable_model_multipliers: &effective_billable_model_multipliers,
            },
            42,
            200,
            KiroUsageSummary {
                input_uncached_tokens: 50_049,
                input_cached_tokens: 0,
                output_tokens: 926,
                credit_usage: Some(2.5598),
                credit_usage_missing: false,
            },
            false,
            event_context.last_message_content.clone(),
        );

        assert_eq!(record.billable_tokens, 82_019);
    }

    #[test]
    fn build_kiro_success_usage_event_skips_full_request_payloads_for_normal_credit() {
        let key = LlmGatewayKeyRecord {
            id: "key-1".to_string(),
            name: "test-key".to_string(),
            secret: "secret".to_string(),
            key_hash: "hash".to_string(),
            status: "active".to_string(),
            provider_type: "kiro".to_string(),
            protocol_family: "anthropic".to_string(),
            public_visible: false,
            quota_billable_limit: 100,
            usage_input_uncached_tokens: 0,
            usage_input_cached_tokens: 0,
            usage_output_tokens: 0,
            usage_billable_tokens: 0,
            usage_credit_total: 0.0,
            usage_credit_missing_events: 0,
            last_used_at: None,
            created_at: 0,
            updated_at: 0,
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
        };
        let event_context = KiroEventContext {
            account_name: Some("acct-a".to_string()),
            request_method: "POST".to_string(),
            request_url: "/api/kiro-gateway/v1/messages".to_string(),
            endpoint: "/generateAssistantResponse".to_string(),
            model: Some("claude-opus-4-6".to_string()),
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: "[]".to_string(),
            last_message_content: Some("hello".to_string()),
            client_request_body_json: Some("{\"messages\":[]}".to_string()),
            upstream_request_body_json: Some(
                "{\"conversationState\":{\"conversationId\":\"conv-1\"}}".to_string(),
            ),
            conversation_id: Some("conv-1".to_string()),
            session_resolution: Some("metadata_json".to_string()),
            session_source_name: Some("session_id".to_string()),
            session_source_value_preview: Some("conv-1".to_string()),
            started_at: std::time::Instant::now(),
        };
        let runtime_config = crate::state::LlmGatewayRuntimeConfig::default();

        let record = build_kiro_usage_event_record(
            KiroUsageEventBuild {
                current: &key,
                event_context: &event_context,
                effective_billable_model_multipliers: &runtime_config
                    .kiro_billable_model_multipliers,
            },
            42,
            200,
            KiroUsageSummary {
                input_uncached_tokens: 50_049,
                input_cached_tokens: 0,
                output_tokens: 926,
                credit_usage: Some(1.9999),
                credit_usage_missing: false,
            },
            false,
            event_context.last_message_content.clone(),
        );

        assert_eq!(record.request_headers_json, "[]");
        assert!(record.client_request_body_json.is_none());
        assert!(record.upstream_request_body_json.is_none());
        assert!(record.full_request_json.is_none());
    }

    #[test]
    fn build_kiro_success_usage_event_override_settings_do_not_restore_full_request_payloads() {
        let key = LlmGatewayKeyRecord {
            id: "key-1".to_string(),
            name: "test-key".to_string(),
            secret: "secret".to_string(),
            key_hash: "hash".to_string(),
            status: "active".to_string(),
            provider_type: "kiro".to_string(),
            protocol_family: "anthropic".to_string(),
            public_visible: false,
            quota_billable_limit: 100,
            usage_input_uncached_tokens: 0,
            usage_input_cached_tokens: 0,
            usage_output_tokens: 0,
            usage_billable_tokens: 0,
            usage_credit_total: 0.0,
            usage_credit_missing_events: 0,
            last_used_at: None,
            created_at: 0,
            updated_at: 0,
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
        };
        let event_context = KiroEventContext {
            account_name: Some("acct-a".to_string()),
            request_method: "POST".to_string(),
            request_url: "/api/kiro-gateway/v1/messages".to_string(),
            endpoint: "/generateAssistantResponse".to_string(),
            model: Some("claude-opus-4-6".to_string()),
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: "[]".to_string(),
            last_message_content: Some("hello".to_string()),
            client_request_body_json: Some("{\"messages\":[]}".to_string()),
            upstream_request_body_json: Some(
                "{\"conversationState\":{\"conversationId\":\"conv-1\"}}".to_string(),
            ),
            conversation_id: Some("conv-1".to_string()),
            session_resolution: Some("metadata_json".to_string()),
            session_source_name: Some("session_id".to_string()),
            session_source_value_preview: Some("conv-1".to_string()),
            started_at: std::time::Instant::now(),
        };
        let runtime_config = crate::state::LlmGatewayRuntimeConfig::default();
        let record = build_kiro_usage_event_record(
            KiroUsageEventBuild {
                current: &key,
                event_context: &event_context,
                effective_billable_model_multipliers: &runtime_config
                    .kiro_billable_model_multipliers,
            },
            42,
            200,
            KiroUsageSummary {
                input_uncached_tokens: 50_049,
                input_cached_tokens: 0,
                output_tokens: 926,
                credit_usage: Some(1.3),
                credit_usage_missing: false,
            },
            false,
            event_context.last_message_content.clone(),
        );

        assert_eq!(record.request_headers_json, "[]");
        assert!(record.client_request_body_json.is_none());
        assert!(record.upstream_request_body_json.is_none());
        assert!(record.full_request_json.is_none());
    }
}
