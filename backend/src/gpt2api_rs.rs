use std::{
    env,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result};
use axum::{
    body::{to_bytes, Body},
    extract::{Multipart, OriginalUri, Path as AxumPath, Query, State},
    http::{header, HeaderMap, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    Json,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use static_flow_shared::llm_gateway_store::{
    now_ms, Gpt2ApiAccountContributionRequestRecord, NewGpt2ApiAccountContributionRequestInput,
    LLM_GATEWAY_TOKEN_REQUEST_STATUS_FAILED, LLM_GATEWAY_TOKEN_REQUEST_STATUS_ISSUED,
    LLM_GATEWAY_TOKEN_REQUEST_STATUS_PENDING, LLM_GATEWAY_TOKEN_REQUEST_STATUS_REJECTED,
};

use crate::{
    email::{
        build_gpt2api_login_url, normalize_frontend_page_url_input, normalize_requester_email_input,
    },
    handlers::{ensure_admin_access, generate_task_id, AdminTaskActionRequest, ErrorResponse},
    public_submit_guard::{
        build_client_fingerprint, build_submit_rate_limit_key, enforce_public_submit_rate_limit,
        extract_client_ip,
    },
    state::AppState,
};

const DEFAULT_CONFIG_PATH: &str = "conf/gpt2api-rs.json";
const DEFAULT_TIMEOUT_SECONDS: u64 = 60;
const MAX_TIMEOUT_SECONDS: u64 = 300;
const MAX_PUBLIC_PROXY_BODY_BYTES: usize = 64 * 1024 * 1024;
const MAX_PUBLIC_GPT2API_CONTRIBUTION_MESSAGE_CHARS: usize = 4000;
const MAX_PUBLIC_GPT2API_CONTRIBUTION_LABEL_CHARS: usize = 80;
const MAX_PUBLIC_GPT2API_CONTRIBUTION_GITHUB_ID_CHARS: usize = 39;
const MAX_PUBLIC_GPT2API_SESSION_JSON_BYTES: usize = 256 * 1024;
const GPT2API_CONTRIBUTION_KEY_QUOTA_TOTAL_CALLS: i64 = 100_000_000_000;

type HandlerResult<T> = Result<T, (StatusCode, Json<ErrorResponse>)>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Gpt2ApiRsConfig {
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub admin_token: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
}

impl Default for Gpt2ApiRsConfig {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            admin_token: String::new(),
            api_key: String::new(),
            timeout_seconds: default_timeout_seconds(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminGpt2ApiRsConfigEnvelope {
    pub config_path: String,
    pub configured: bool,
    pub config: Gpt2ApiRsConfig,
}

#[derive(Debug, Deserialize)]
pub struct UsageLimitQuery {
    #[serde(default)]
    pub limit: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct UsageEventsQuery {
    #[serde(default)]
    pub key_id: Option<String>,
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub limit: Option<u64>,
    #[serde(default)]
    pub offset: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct AdminImageEditRequest {
    pub prompt: String,
    #[serde(default = "default_image_model")]
    pub model: String,
    #[serde(default = "default_image_count")]
    pub n: usize,
    #[serde(default = "default_image_size")]
    pub size: String,
    pub image_base64: String,
    #[serde(default)]
    pub file_name: Option<String>,
    #[serde(default)]
    pub mime_type: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SubmitGpt2ApiAccountContributionRequest {
    pub account_name: String,
    #[serde(default)]
    pub access_token: Option<String>,
    #[serde(default)]
    pub session_json: Option<String>,
    pub requester_email: String,
    pub contributor_message: String,
    #[serde(default)]
    pub github_id: Option<String>,
    #[serde(default)]
    pub frontend_page_url: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SubmitGpt2ApiAccountContributionRequestResponse {
    pub request_id: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminGpt2ApiAccountContributionRequestView {
    pub request_id: String,
    pub account_name: String,
    pub access_token: Option<String>,
    pub session_json: Option<String>,
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

#[derive(Debug, Serialize)]
pub struct AdminGpt2ApiAccountContributionRequestsResponse {
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
    pub has_more: bool,
    pub requests: Vec<AdminGpt2ApiAccountContributionRequestView>,
    pub generated_at: i64,
}

#[derive(Debug, Deserialize, Default)]
pub struct AdminGpt2ApiAccountContributionRequestQuery {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub offset: Option<usize>,
}

impl From<&Gpt2ApiAccountContributionRequestRecord> for AdminGpt2ApiAccountContributionRequestView {
    fn from(value: &Gpt2ApiAccountContributionRequestRecord) -> Self {
        Self {
            request_id: value.request_id.clone(),
            account_name: value.account_name.clone(),
            access_token: value.access_token.clone(),
            session_json: value.session_json.clone(),
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

#[derive(Debug, Clone)]
pub struct Gpt2ApiRsState {
    config_path: PathBuf,
    config: Arc<RwLock<Gpt2ApiRsConfig>>,
    client: reqwest::Client,
}

#[derive(Clone, Copy)]
enum TokenScope {
    Admin,
    Public,
}

impl Gpt2ApiRsState {
    pub async fn load_from_env() -> Result<Self> {
        let config_path = resolve_config_path();
        let config = load_config_or_default(&config_path).await?;
        let normalized = normalize_config(config)?;
        Ok(Self {
            config_path,
            config: Arc::new(RwLock::new(normalized)),
            client: reqwest::Client::builder()
                .build()
                .context("failed to build gpt2api-rs admin client")?,
        })
    }

    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    pub fn snapshot(&self) -> Gpt2ApiRsConfig {
        self.config.read().clone()
    }

    pub async fn replace(&self, next: Gpt2ApiRsConfig) -> Result<Gpt2ApiRsConfig> {
        let normalized = normalize_config(next)?;
        save_config(&self.config_path, &normalized).await?;
        *self.config.write() = normalized.clone();
        Ok(normalized)
    }
}

pub async fn get_admin_config(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> HandlerResult<Json<AdminGpt2ApiRsConfigEnvelope>> {
    ensure_admin_access(&state, &headers)?;
    Ok(Json(config_envelope(state.gpt2api_rs.as_ref())))
}

pub async fn update_admin_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<Gpt2ApiRsConfig>,
) -> HandlerResult<Json<AdminGpt2ApiRsConfigEnvelope>> {
    ensure_admin_access(&state, &headers)?;
    state
        .gpt2api_rs
        .replace(request)
        .await
        .map_err(internal_error)?;
    Ok(Json(config_envelope(state.gpt2api_rs.as_ref())))
}

pub async fn get_admin_status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    let config = state.gpt2api_rs.snapshot();
    if !is_configured(&config) {
        return Ok(Json(json!({
            "configured": false,
            "config_path": state.gpt2api_rs.config_path().display().to_string(),
        })));
    }
    let mut payload = proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Admin,
        Method::GET,
        "/admin/status",
        None,
        None,
    )
    .await?
    .0;
    if let Some(map) = payload.as_object_mut() {
        map.insert("configured".to_string(), Value::Bool(true));
        map.insert(
            "config_path".to_string(),
            Value::String(state.gpt2api_rs.config_path().display().to_string()),
        );
    }
    Ok(Json(payload))
}

pub async fn submit_public_account_contribution_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SubmitGpt2ApiAccountContributionRequest>,
) -> HandlerResult<Json<SubmitGpt2ApiAccountContributionRequestResponse>> {
    ensure_gpt2api_admin_online(&state).await?;

    let account_name = normalize_gpt2api_contribution_label(&request.account_name)
        .map_err(|err| bad_request(format!("invalid account_name: {err}")))?;
    let access_token = normalize_optional_gpt2api_secret(request.access_token.as_deref());
    let session_json = normalize_optional_gpt2api_secret(request.session_json.as_deref());
    if access_token.is_none() && session_json.is_none() {
        return Err(bad_request("access_token or session_json is required"));
    }
    if let Some(session_json) = session_json.as_deref() {
        if session_json.len() > MAX_PUBLIC_GPT2API_SESSION_JSON_BYTES {
            return Err(bad_request("session_json is too large"));
        }
    }
    let requester_email = normalize_requester_email_input(Some(request.requester_email))
        .map_err(|err| bad_request(format!("invalid requester_email: {err}")))?
        .ok_or_else(|| bad_request("requester_email is required"))?;
    let contributor_message = request.contributor_message.trim();
    if contributor_message.is_empty() {
        return Err(bad_request("contributor_message is required"));
    }
    if contributor_message.chars().count() > MAX_PUBLIC_GPT2API_CONTRIBUTION_MESSAGE_CHARS {
        return Err(bad_request("contributor_message is too long"));
    }
    let github_id = normalize_optional_gpt2api_github_id(request.github_id)
        .map_err(|err| bad_request(format!("invalid github_id: {err}")))?;
    let frontend_page_url = normalize_frontend_page_url_input(request.frontend_page_url)
        .map_err(|err| bad_request(format!("invalid frontend_page_url: {err}")))?;

    let client_ip = extract_client_ip(&headers);
    let fingerprint = build_client_fingerprint(&headers);
    let rate_limit_key = build_submit_rate_limit_key(&headers, &fingerprint);
    enforce_public_submit_rate_limit(
        state.llm_gateway_public_submit_guard.as_ref(),
        &rate_limit_key,
        now_ms(),
        60,
        "gpt2api public account contribution",
    )?;

    let request_id = generate_task_id("gptacct");
    let ip_region = state.geoip.resolve_region(&client_ip).await;
    let record = state
        .llm_gateway_store
        .create_gpt2api_account_contribution_request(NewGpt2ApiAccountContributionRequestInput {
            request_id: request_id.clone(),
            account_name,
            access_token,
            session_json,
            requester_email,
            contributor_message: contributor_message.to_string(),
            github_id,
            frontend_page_url,
            fingerprint,
            client_ip,
            ip_region,
        })
        .await
        .map_err(internal_error)?;

    if let Some(notifier) = state.email_notifier.clone() {
        let record_for_email = record.clone();
        tokio::spawn(async move {
            if let Err(err) = notifier
                .send_admin_new_gpt2api_account_contribution_request_notification(&record_for_email)
                .await
            {
                tracing::warn!(
                    "failed to send admin notification email for gpt2api account contribution {}: \
                     {}",
                    record_for_email.request_id,
                    err
                );
            }
        });
    }

    Ok(Json(SubmitGpt2ApiAccountContributionRequestResponse {
        request_id,
        status: LLM_GATEWAY_TOKEN_REQUEST_STATUS_PENDING.to_string(),
    }))
}

pub async fn list_admin_account_contribution_requests(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<AdminGpt2ApiAccountContributionRequestQuery>,
) -> HandlerResult<Json<AdminGpt2ApiAccountContributionRequestsResponse>> {
    ensure_admin_access(&state, &headers)?;
    let limit = query.limit.unwrap_or(25).clamp(1, 100);
    let offset = query.offset.unwrap_or(0);
    let total = state
        .llm_gateway_store
        .count_gpt2api_account_contribution_requests(query.status.as_deref())
        .await
        .map_err(internal_error)?;
    if total == 0 || offset >= total {
        return Ok(Json(AdminGpt2ApiAccountContributionRequestsResponse {
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
        .list_gpt2api_account_contribution_requests_page(query.status.as_deref(), limit, offset)
        .await
        .map_err(internal_error)?;
    Ok(Json(AdminGpt2ApiAccountContributionRequestsResponse {
        total,
        offset,
        limit,
        has_more: offset.saturating_add(requests.len()) < total,
        requests: requests
            .iter()
            .map(AdminGpt2ApiAccountContributionRequestView::from)
            .collect(),
        generated_at: now_ms(),
    }))
}

pub async fn approve_and_issue_account_contribution_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(request_id): AxumPath<String>,
    Json(request): Json<AdminTaskActionRequest>,
) -> HandlerResult<Json<AdminGpt2ApiAccountContributionRequestView>> {
    ensure_admin_access(&state, &headers)?;

    let mut contribution_request = state
        .llm_gateway_store
        .get_gpt2api_account_contribution_request(&request_id)
        .await
        .map_err(internal_error)?
        .ok_or_else(|| not_found("gpt2api account contribution request not found"))?;

    match contribution_request.status.as_str() {
        LLM_GATEWAY_TOKEN_REQUEST_STATUS_ISSUED | LLM_GATEWAY_TOKEN_REQUEST_STATUS_REJECTED => {
            return Err(conflict_error("gpt2api account contribution request is finalized"));
        },
        _ => {},
    }

    let Some(notifier) = state.email_notifier.clone() else {
        mark_gpt2api_contribution_failed(
            &state,
            &mut contribution_request,
            "email notifier is not configured".to_string(),
        )
        .await?;
        return Err(internal_error("email notifier is not configured"));
    };

    let access_token = resolve_gpt2api_contribution_access_token(&contribution_request)
        .map_err(|err| bad_request(format!("invalid contributed credential: {err}")))?;
    let imported_account_name = if let Some(name) = contribution_request
        .imported_account_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
    {
        name
    } else {
        let payload = import_gpt2api_contribution_account(&state, &contribution_request).await?;
        let account_name = find_gpt2api_account_name_for_access_token(&payload, &access_token)
            .ok_or_else(|| {
                internal_error("gpt2api-rs import response did not include the contributed account")
            })?;
        contribution_request.imported_account_name = Some(account_name.clone());
        account_name
    };

    let (key_id, key_name, api_key) = if let Some(existing_key_id) =
        contribution_request.issued_key_id.as_deref()
    {
        load_gpt2api_key_secret(&state, existing_key_id).await?
    } else {
        create_bound_gpt2api_contribution_key(&state, &contribution_request, &imported_account_name)
            .await?
    };

    let login_url = contribution_request
        .frontend_page_url
        .as_deref()
        .and_then(|url| build_gpt2api_login_url(url).ok())
        .or_else(|| {
            env::var("SITE_BASE_URL")
                .ok()
                .map(|base| format!("{}/gpt2api/login", base.trim_end_matches('/')))
        })
        .unwrap_or_else(|| "/gpt2api/login".to_string());

    let now = now_ms();
    contribution_request.admin_note = request.admin_note.clone();
    contribution_request.failure_reason = None;
    contribution_request.imported_account_name = Some(imported_account_name);
    contribution_request.issued_key_id = Some(key_id.clone());
    contribution_request.issued_key_name = Some(key_name.clone());
    contribution_request.updated_at = now;
    contribution_request.processed_at = Some(now);
    let mut issued_request = contribution_request.clone();
    issued_request.status = LLM_GATEWAY_TOKEN_REQUEST_STATUS_ISSUED.to_string();

    match notifier
        .send_user_gpt2api_account_contribution_issued_notification(
            &issued_request,
            &key_id,
            &key_name,
            &api_key,
            &login_url,
        )
        .await
    {
        Ok(()) => {
            contribution_request = issued_request;
            state
                .llm_gateway_store
                .upsert_gpt2api_account_contribution_request(&contribution_request)
                .await
                .map_err(internal_error)?;
            Ok(Json(AdminGpt2ApiAccountContributionRequestView::from(&contribution_request)))
        },
        Err(err) => {
            mark_gpt2api_contribution_failed(&state, &mut contribution_request, err.to_string())
                .await?;
            Err(internal_error("failed to send gpt2api contribution email"))
        },
    }
}

pub async fn reject_account_contribution_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(request_id): AxumPath<String>,
    Json(request): Json<AdminTaskActionRequest>,
) -> HandlerResult<Json<AdminGpt2ApiAccountContributionRequestView>> {
    ensure_admin_access(&state, &headers)?;
    let mut contribution_request = state
        .llm_gateway_store
        .get_gpt2api_account_contribution_request(&request_id)
        .await
        .map_err(internal_error)?
        .ok_or_else(|| not_found("gpt2api account contribution request not found"))?;

    if contribution_request.status == LLM_GATEWAY_TOKEN_REQUEST_STATUS_ISSUED {
        return Err(conflict_error(
            "issued gpt2api account contribution request cannot be rejected",
        ));
    }
    if contribution_request.status == LLM_GATEWAY_TOKEN_REQUEST_STATUS_REJECTED {
        return Err(conflict_error("gpt2api account contribution request is already rejected"));
    }

    if let Some(key_id) = contribution_request.issued_key_id.as_deref() {
        delete_gpt2api_key_if_present(&state, key_id).await?;
    }
    if contribution_request.imported_account_name.is_some() {
        if let Ok(access_token) = resolve_gpt2api_contribution_access_token(&contribution_request) {
            delete_gpt2api_account_if_present(&state, &access_token).await?;
        }
    }

    let now = now_ms();
    contribution_request.status = LLM_GATEWAY_TOKEN_REQUEST_STATUS_REJECTED.to_string();
    contribution_request.admin_note = request.admin_note.clone();
    contribution_request.failure_reason = None;
    contribution_request.updated_at = now;
    contribution_request.processed_at = Some(now);
    state
        .llm_gateway_store
        .upsert_gpt2api_account_contribution_request(&contribution_request)
        .await
        .map_err(internal_error)?;

    Ok(Json(AdminGpt2ApiAccountContributionRequestView::from(&contribution_request)))
}

pub async fn get_public_version(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Public,
        Method::GET,
        "/version",
        None,
        None,
    )
    .await
}

pub async fn get_public_models(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Public,
        Method::GET,
        "/v1/models",
        None,
        None,
    )
    .await
}

pub async fn post_public_login(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Public,
        Method::POST,
        "/auth/login",
        None,
        Some(Value::Object(serde_json::Map::new())),
    )
    .await
}

pub async fn list_admin_accounts(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Admin,
        Method::GET,
        "/admin/accounts",
        None,
        None,
    )
    .await
}

pub async fn list_admin_proxy_configs(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Admin,
        Method::GET,
        "/admin/proxy-configs",
        None,
        None,
    )
    .await
}

pub async fn create_admin_proxy_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Admin,
        Method::POST,
        "/admin/proxy-configs",
        None,
        Some(request),
    )
    .await
}

pub async fn update_admin_proxy_config(
    AxumPath(proxy_id): AxumPath<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Admin,
        Method::PATCH,
        &format!("/admin/proxy-configs/{proxy_id}"),
        None,
        Some(request),
    )
    .await
}

pub async fn delete_admin_proxy_config(
    AxumPath(proxy_id): AxumPath<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Admin,
        Method::DELETE,
        &format!("/admin/proxy-configs/{proxy_id}"),
        None,
        None,
    )
    .await
}

pub async fn check_admin_proxy_config(
    AxumPath(proxy_id): AxumPath<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Admin,
        Method::POST,
        &format!("/admin/proxy-configs/{proxy_id}/check"),
        None,
        Some(Value::Object(serde_json::Map::new())),
    )
    .await
}

pub async fn list_admin_account_groups(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Admin,
        Method::GET,
        "/admin/account-groups",
        None,
        None,
    )
    .await
}

pub async fn create_admin_account_group(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Admin,
        Method::POST,
        "/admin/account-groups",
        None,
        Some(request),
    )
    .await
}

pub async fn update_admin_account_group(
    AxumPath(group_id): AxumPath<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Admin,
        Method::PATCH,
        &format!("/admin/account-groups/{group_id}"),
        None,
        Some(request),
    )
    .await
}

pub async fn delete_admin_account_group(
    AxumPath(group_id): AxumPath<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Admin,
        Method::DELETE,
        &format!("/admin/account-groups/{group_id}"),
        None,
        None,
    )
    .await
}

pub async fn import_admin_accounts(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Admin,
        Method::POST,
        "/admin/accounts/import",
        None,
        Some(request),
    )
    .await
}

pub async fn delete_admin_accounts(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Admin,
        Method::DELETE,
        "/admin/accounts",
        None,
        Some(request),
    )
    .await
}

pub async fn refresh_admin_accounts(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Admin,
        Method::POST,
        "/admin/accounts/refresh",
        None,
        Some(request),
    )
    .await
}

pub async fn update_admin_account(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Admin,
        Method::POST,
        "/admin/accounts/update",
        None,
        Some(request),
    )
    .await
}

pub async fn list_admin_keys(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Admin,
        Method::GET,
        "/admin/keys",
        None,
        None,
    )
    .await
}

pub async fn create_admin_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Admin,
        Method::POST,
        "/admin/keys",
        None,
        Some(request),
    )
    .await
}

pub async fn update_admin_key(
    AxumPath(key_id): AxumPath<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Admin,
        Method::PATCH,
        &format!("/admin/keys/{key_id}"),
        None,
        Some(request),
    )
    .await
}

pub async fn rotate_admin_key(
    AxumPath(key_id): AxumPath<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Admin,
        Method::POST,
        &format!("/admin/keys/{key_id}/rotate"),
        None,
        Some(Value::Object(serde_json::Map::new())),
    )
    .await
}

pub async fn delete_admin_key(
    AxumPath(key_id): AxumPath<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Admin,
        Method::DELETE,
        &format!("/admin/keys/{key_id}"),
        None,
        None,
    )
    .await
}

pub async fn list_admin_usage(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<UsageLimitQuery>,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    let limit = query.limit.unwrap_or(50);
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Admin,
        Method::GET,
        "/admin/usage",
        Some(&[("limit", limit.to_string())]),
        None,
    )
    .await
}

pub async fn list_admin_usage_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<UsageEventsQuery>,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    let mut params = Vec::new();
    if let Some(key_id) = query.key_id.filter(|value| !value.trim().is_empty()) {
        params.push(("key_id", key_id));
    }
    if let Some(q) = query.q.filter(|value| !value.trim().is_empty()) {
        params.push(("q", q));
    }
    if let Some(limit) = query.limit {
        params.push(("limit", limit.to_string()));
    }
    if let Some(offset) = query.offset {
        params.push(("offset", offset.to_string()));
    }
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Admin,
        Method::GET,
        "/admin/usage/events",
        Some(&params),
        None,
    )
    .await
}

pub async fn post_image_generation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Public,
        Method::POST,
        "/v1/images/generations",
        None,
        Some(request),
    )
    .await
}

pub async fn post_image_edit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<AdminImageEditRequest>,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    let config = state.gpt2api_rs.snapshot();
    let url = configured_url(&config, "/v1/images/edits").map_err(bad_request)?;
    let bearer = configured_token(&config, TokenScope::Public).map_err(bad_request)?;
    let image_bytes = BASE64
        .decode(request.image_base64.trim())
        .map_err(|err| bad_request(format!("invalid image_base64: {err}")))?;
    let file_name = request
        .file_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("image.png")
        .to_string();
    let mime_type = request
        .mime_type
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("image/png")
        .to_string();
    let form = reqwest::multipart::Form::new()
        .text("prompt", request.prompt.trim().to_string())
        .text("model", request.model.trim().to_string())
        .text("n", request.n.to_string())
        .text("size", request.size.trim().to_string())
        .part(
            "image",
            reqwest::multipart::Part::bytes(image_bytes)
                .file_name(file_name)
                .mime_str(&mime_type)
                .map_err(internal_error)?,
        );
    let response = state
        .gpt2api_rs
        .client
        .post(url)
        .timeout(Duration::from_secs(config.timeout_seconds))
        .bearer_auth(bearer)
        .multipart(form)
        .send()
        .await
        .map_err(bad_gateway)?;
    decode_json_response(response).await
}

pub async fn post_chat_completions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Public,
        Method::POST,
        "/v1/chat/completions",
        None,
        Some(request),
    )
    .await
}

pub async fn post_responses(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Public,
        Method::POST,
        "/v1/responses",
        None,
        Some(request),
    )
    .await
}

pub async fn post_public_auth_verify(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> HandlerResult<Json<Value>> {
    let bearer = extract_public_bearer(&headers)?;
    proxy_json_request_with_bearer(
        state.gpt2api_rs.as_ref(),
        bearer,
        Method::POST,
        "/auth/login",
        None,
        Some(Value::Object(serde_json::Map::new())),
    )
    .await
}

pub async fn public_image_generation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> HandlerResult<Json<Value>> {
    let bearer = extract_public_bearer(&headers)?;
    proxy_json_request_with_bearer(
        state.gpt2api_rs.as_ref(),
        bearer,
        Method::POST,
        "/v1/images/generations",
        None,
        Some(request),
    )
    .await
}

pub async fn public_image_edit(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> HandlerResult<Json<Value>> {
    let bearer = extract_public_bearer(&headers)?;
    let mut form = reqwest::multipart::Form::new();
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|err| bad_request(format!("invalid multipart body: {err}")))?
    {
        let name = field.name().unwrap_or_default().to_string();
        if name.trim().is_empty() {
            continue;
        }
        let file_name = field.file_name().map(ToString::to_string);
        let content_type = field.content_type().map(ToString::to_string);
        if let Some(file_name) = file_name {
            let bytes = field
                .bytes()
                .await
                .map_err(|err| bad_request(format!("invalid multipart file: {err}")))?;
            let mut part = reqwest::multipart::Part::bytes(bytes.to_vec()).file_name(file_name);
            if let Some(content_type) = content_type.as_deref() {
                part = part.mime_str(content_type).map_err(internal_error)?;
            }
            form = form.part(name, part);
        } else {
            let text = field
                .text()
                .await
                .map_err(|err| bad_request(format!("invalid multipart text: {err}")))?;
            form = form.text(name, text);
        }
    }
    proxy_multipart_request_with_bearer(state.gpt2api_rs.as_ref(), bearer, "/v1/images/edits", form)
        .await
}

pub async fn public_chat_completions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> HandlerResult<Response> {
    let bearer = extract_public_bearer(&headers)?;
    if request
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return proxy_stream_request_with_bearer(
            state.gpt2api_rs.as_ref(),
            bearer,
            Method::POST,
            "/v1/chat/completions",
            Some(request),
        )
        .await;
    }
    proxy_json_request_with_bearer(
        state.gpt2api_rs.as_ref(),
        bearer,
        Method::POST,
        "/v1/chat/completions",
        None,
        Some(request),
    )
    .await
    .map(|payload| payload.into_response())
}

pub async fn public_responses(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> HandlerResult<Json<Value>> {
    let bearer = extract_public_bearer(&headers)?;
    proxy_json_request_with_bearer(
        state.gpt2api_rs.as_ref(),
        bearer,
        Method::POST,
        "/v1/responses",
        None,
        Some(request),
    )
    .await
}

pub async fn proxy_public_product_api(
    State(state): State<AppState>,
    method: Method,
    headers: HeaderMap,
    OriginalUri(original_uri): OriginalUri,
    body: Body,
) -> HandlerResult<Response> {
    let config = state.gpt2api_rs.snapshot();
    let url = configured_url_for_public_proxy(&config, &original_uri).map_err(bad_request)?;
    let body = to_bytes(body, MAX_PUBLIC_PROXY_BODY_BYTES)
        .await
        .map_err(|err| bad_request(format!("invalid gpt2api request body: {err}")))?;
    let mut request = state
        .gpt2api_rs
        .client
        .request(method, url)
        .timeout(Duration::from_secs(config.timeout_seconds))
        .body(body);
    for header_name in
        [header::ACCEPT, header::AUTHORIZATION, header::CONTENT_TYPE, header::CACHE_CONTROL]
    {
        if let Some(value) = headers.get(&header_name) {
            request = request.header(header_name, value.clone());
        }
    }
    let response = request.send().await.map_err(bad_gateway)?;
    proxy_raw_response(response).await
}

fn config_envelope(state: &Gpt2ApiRsState) -> AdminGpt2ApiRsConfigEnvelope {
    let config = state.snapshot();
    AdminGpt2ApiRsConfigEnvelope {
        config_path: state.config_path().display().to_string(),
        configured: is_configured(&config),
        config,
    }
}

async fn proxy_raw_response(response: reqwest::Response) -> HandlerResult<Response> {
    let status = StatusCode::from_u16(response.status().as_u16())
        .map_err(|err| internal_error(format!("invalid upstream status: {err}")))?;
    let mut builder = Response::builder().status(status);
    for header_name in [header::CONTENT_TYPE, header::CACHE_CONTROL, header::CONTENT_DISPOSITION] {
        if let Some(value) = response.headers().get(&header_name) {
            builder = builder.header(header_name, value.clone());
        }
    }
    builder
        .body(Body::from_stream(response.bytes_stream()))
        .map_err(internal_error)
}

async fn proxy_json_request(
    state: &Gpt2ApiRsState,
    scope: TokenScope,
    method: Method,
    path: &str,
    query: Option<&[(&str, String)]>,
    body: Option<Value>,
) -> HandlerResult<Json<Value>> {
    let config = state.snapshot();
    let bearer = configured_token(&config, scope).map_err(bad_request)?;
    proxy_json_request_with_bearer(state, bearer, method, path, query, body).await
}

async fn proxy_json_request_with_bearer(
    state: &Gpt2ApiRsState,
    bearer: String,
    method: Method,
    path: &str,
    query: Option<&[(&str, String)]>,
    body: Option<Value>,
) -> HandlerResult<Json<Value>> {
    let config = state.snapshot();
    let url = configured_url(&config, path).map_err(bad_request)?;
    let mut request = state.client.request(method, url);
    request = configure_timeout_and_auth(request, &config, &bearer);
    if let Some(query) = query {
        request = request.query(query);
    }
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await.map_err(bad_gateway)?;
    decode_json_response(response).await
}

async fn proxy_multipart_request_with_bearer(
    state: &Gpt2ApiRsState,
    bearer: String,
    path: &str,
    form: reqwest::multipart::Form,
) -> HandlerResult<Json<Value>> {
    let config = state.snapshot();
    let url = configured_url(&config, path).map_err(bad_request)?;
    let request =
        configure_timeout_and_auth(state.client.post(url), &config, &bearer).multipart(form);
    let response = request.send().await.map_err(bad_gateway)?;
    decode_json_response(response).await
}

async fn proxy_stream_request_with_bearer(
    state: &Gpt2ApiRsState,
    bearer: String,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> HandlerResult<Response> {
    let config = state.snapshot();
    let url = configured_url(&config, path).map_err(bad_request)?;
    let mut request =
        configure_timeout_and_auth(state.client.request(method, url), &config, &bearer);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await.map_err(bad_gateway)?;
    let status = StatusCode::from_u16(response.status().as_u16())
        .map_err(|err| internal_error(format!("invalid upstream status: {err}")))?;
    if !status.is_success() {
        let error = decode_error_response(status, response.bytes().await.map_err(bad_gateway)?);
        return Err(error);
    }
    let mut builder = Response::builder().status(status);
    for header_name in [header::CONTENT_TYPE, header::CACHE_CONTROL] {
        if let Some(value) = response.headers().get(&header_name) {
            builder = builder.header(header_name, value.clone());
        }
    }
    builder
        .body(Body::from_stream(response.bytes_stream()))
        .map_err(internal_error)
}

async fn decode_json_response(response: reqwest::Response) -> HandlerResult<Json<Value>> {
    let status = StatusCode::from_u16(response.status().as_u16())
        .map_err(|err| internal_error(format!("invalid upstream status: {err}")))?;
    let bytes = response.bytes().await.map_err(bad_gateway)?;
    if status.is_success() {
        let payload = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice::<Value>(&bytes)
                .map_err(|err| internal_error(format!("failed to decode upstream json: {err}")))?
        };
        return Ok(Json(payload));
    }
    Err(decode_error_response(status, bytes))
}

async fn ensure_gpt2api_admin_online(state: &AppState) -> HandlerResult<()> {
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Admin,
        Method::GET,
        "/admin/status",
        None,
        None,
    )
    .await
    .map(|_| ())
}

async fn import_gpt2api_contribution_account(
    state: &AppState,
    request: &Gpt2ApiAccountContributionRequestRecord,
) -> HandlerResult<Value> {
    let body = json!({
        "access_tokens": request
            .access_token
            .as_ref()
            .map(|token| vec![token.clone()])
            .unwrap_or_default(),
        "session_jsons": request
            .session_json
            .as_ref()
            .map(|session_json| vec![session_json.clone()])
            .unwrap_or_default(),
    });
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Admin,
        Method::POST,
        "/admin/accounts/import",
        None,
        Some(body),
    )
    .await
    .map(|payload| payload.0)
}

fn find_gpt2api_account_name_for_access_token(
    payload: &Value,
    access_token: &str,
) -> Option<String> {
    payload
        .get("items")
        .and_then(Value::as_array)?
        .iter()
        .find(|item| {
            item.get("access_token")
                .and_then(Value::as_str)
                .map(str::trim)
                == Some(access_token)
        })
        .and_then(|item| item.get("name").and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

async fn create_bound_gpt2api_contribution_key(
    state: &AppState,
    request: &Gpt2ApiAccountContributionRequestRecord,
    account_name: &str,
) -> HandlerResult<(String, String, String)> {
    let key_name = normalize_gpt2api_key_name(&format!("contrib-{}", request.request_id));
    let body = json!({
        "name": key_name,
        "quota_total_calls": GPT2API_CONTRIBUTION_KEY_QUOTA_TOTAL_CALLS,
        "status": "active",
        "route_strategy": "fixed",
        "fixed_account_name": account_name,
        "role": "user",
        "notification_email": request.requester_email,
        "notification_enabled": true,
    });
    let payload = proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Admin,
        Method::POST,
        "/admin/keys",
        None,
        Some(body),
    )
    .await?
    .0;
    extract_gpt2api_key_secret_tuple(&payload)
}

async fn load_gpt2api_key_secret(
    state: &AppState,
    key_id: &str,
) -> HandlerResult<(String, String, String)> {
    let payload = proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Admin,
        Method::GET,
        "/admin/keys",
        None,
        None,
    )
    .await?
    .0;
    let Some(keys) = payload.as_array() else {
        return Err(internal_error("gpt2api-rs keys response is not an array"));
    };
    let Some(key) = keys
        .iter()
        .find(|item| item.get("id").and_then(Value::as_str).map(str::trim) == Some(key_id))
    else {
        return Err(not_found("gpt2api contribution key not found"));
    };
    extract_gpt2api_key_secret_tuple(key)
}

fn extract_gpt2api_key_secret_tuple(payload: &Value) -> HandlerResult<(String, String, String)> {
    let id = payload
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| internal_error("gpt2api-rs key response missing id"))?
        .to_string();
    let name = payload
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| internal_error("gpt2api-rs key response missing name"))?
        .to_string();
    let secret = payload
        .get("secret_plaintext")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| internal_error("gpt2api-rs key response missing plaintext secret"))?
        .to_string();
    Ok((id, name, secret))
}

async fn delete_gpt2api_key_if_present(state: &AppState, key_id: &str) -> HandlerResult<()> {
    let path = format!("/admin/keys/{key_id}");
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Admin,
        Method::DELETE,
        &path,
        None,
        None,
    )
    .await
    .map(|_| ())
}

async fn delete_gpt2api_account_if_present(
    state: &AppState,
    access_token: &str,
) -> HandlerResult<()> {
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Admin,
        Method::DELETE,
        "/admin/accounts",
        None,
        Some(json!({ "access_tokens": [access_token] })),
    )
    .await
    .map(|_| ())
}

async fn mark_gpt2api_contribution_failed(
    state: &AppState,
    request: &mut Gpt2ApiAccountContributionRequestRecord,
    reason: String,
) -> HandlerResult<()> {
    request.status = LLM_GATEWAY_TOKEN_REQUEST_STATUS_FAILED.to_string();
    request.failure_reason = Some(reason);
    request.updated_at = now_ms();
    request.processed_at = Some(now_ms());
    state
        .llm_gateway_store
        .upsert_gpt2api_account_contribution_request(request)
        .await
        .map_err(internal_error)
}

fn resolve_gpt2api_contribution_access_token(
    request: &Gpt2ApiAccountContributionRequestRecord,
) -> Result<String> {
    if let Some(token) = request
        .access_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(token.to_string());
    }
    let session_json = request
        .session_json
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .context("access_token or session_json is required")?;
    let value: Value = serde_json::from_str(session_json).context("invalid session_json")?;
    value
        .get("accessToken")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .context("session_json missing accessToken")
}

fn normalize_gpt2api_contribution_label(raw: &str) -> Result<String> {
    let label = raw.trim();
    if label.is_empty() {
        anyhow::bail!("account_name is required");
    }
    if label.chars().count() > MAX_PUBLIC_GPT2API_CONTRIBUTION_LABEL_CHARS {
        anyhow::bail!("account_name is too long");
    }
    if label.chars().any(char::is_control) {
        anyhow::bail!("account_name cannot contain control characters");
    }
    Ok(label.to_string())
}

fn normalize_optional_gpt2api_secret(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn normalize_optional_gpt2api_github_id(value: Option<String>) -> Result<Option<String>> {
    let Some(trimmed) = value
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
    else {
        return Ok(None);
    };
    if trimmed.chars().count() > MAX_PUBLIC_GPT2API_CONTRIBUTION_GITHUB_ID_CHARS {
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

fn normalize_gpt2api_key_name(raw: &str) -> String {
    raw.chars()
        .map(|ch| if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' { ch } else { '-' })
        .collect()
}

fn decode_error_response(
    status: StatusCode,
    bytes: bytes::Bytes,
) -> (StatusCode, Json<ErrorResponse>) {
    if let Ok(payload) = serde_json::from_slice::<ErrorResponse>(&bytes) {
        return (status, Json(payload));
    }
    if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
        if let Some(message) = extract_error_message(&value) {
            return error_response(status, message);
        }
    }
    let message = String::from_utf8_lossy(&bytes).trim().to_string();
    error_response(
        status,
        if message.is_empty() { "gpt2api-rs request failed".to_string() } else { message },
    )
}

fn configured_url(config: &Gpt2ApiRsConfig, path: &str) -> Result<String> {
    let base = config.base_url.trim();
    if base.is_empty() {
        anyhow::bail!("gpt2api-rs base_url is empty");
    }
    let path = path.trim();
    if !path.starts_with('/') {
        anyhow::bail!("gpt2api-rs relative path must start with `/`");
    }
    Ok(format!("{base}{path}"))
}

fn configured_url_for_public_proxy(config: &Gpt2ApiRsConfig, original_uri: &Uri) -> Result<String> {
    let raw = original_uri
        .path_and_query()
        .map(|path| path.as_str())
        .unwrap_or("/api/gpt2api");
    let Some(upstream) = raw.strip_prefix("/api/gpt2api") else {
        anyhow::bail!("gpt2api proxy path must start with /api/gpt2api");
    };
    let upstream = if upstream.is_empty() { "/" } else { upstream };
    configured_url(config, upstream)
}

fn configured_token(config: &Gpt2ApiRsConfig, scope: TokenScope) -> Result<String> {
    let value = match scope {
        TokenScope::Admin => config.admin_token.trim(),
        TokenScope::Public => config.api_key.trim(),
    };
    if value.is_empty() {
        let label = match scope {
            TokenScope::Admin => "admin_token",
            TokenScope::Public => "api_key",
        };
        anyhow::bail!("gpt2api-rs {label} is empty");
    }
    Ok(value.to_string())
}

fn configure_timeout_and_auth(
    request: reqwest::RequestBuilder,
    config: &Gpt2ApiRsConfig,
    bearer: &str,
) -> reqwest::RequestBuilder {
    request
        .timeout(Duration::from_secs(config.timeout_seconds))
        .bearer_auth(bearer)
}

fn extract_public_bearer(headers: &HeaderMap) -> HandlerResult<String> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| error_response(StatusCode::UNAUTHORIZED, "invalid_key"))
}

fn extract_error_message(value: &Value) -> Option<String> {
    if let Some(message) = value.get("error").and_then(Value::as_str) {
        return Some(message.to_string());
    }
    if let Some(error) = value.get("error").and_then(Value::as_object) {
        if let Some(message) = error.get("message").and_then(Value::as_str) {
            return Some(message.to_string());
        }
    }
    value
        .get("message")
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn is_configured(config: &Gpt2ApiRsConfig) -> bool {
    !config.base_url.trim().is_empty()
        && !config.admin_token.trim().is_empty()
        && !config.api_key.trim().is_empty()
}

fn default_timeout_seconds() -> u64 {
    DEFAULT_TIMEOUT_SECONDS
}

fn default_image_model() -> String {
    "gpt-image-2".to_string()
}

const fn default_image_count() -> usize {
    1
}

fn default_image_size() -> String {
    "1024x1024".to_string()
}

fn resolve_config_path() -> PathBuf {
    if let Ok(raw) = env::var("GPT2API_RS_CONFIG") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    PathBuf::from(DEFAULT_CONFIG_PATH)
}

async fn load_config_or_default(path: &Path) -> Result<Gpt2ApiRsConfig> {
    match tokio::fs::read_to_string(path).await {
        Ok(raw) => serde_json::from_str::<Gpt2ApiRsConfig>(&raw)
            .with_context(|| format!("invalid gpt2api-rs config json: {}", path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Gpt2ApiRsConfig::default()),
        Err(err) => Err(err).with_context(|| format!("failed to read {}", path.display())),
    }
}

async fn save_config(path: &Path, config: &Gpt2ApiRsConfig) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create config dir {}", parent.display()))?;
    }
    let content = serde_json::to_string_pretty(config)?;
    tokio::fs::write(path, format!("{content}\n"))
        .await
        .with_context(|| format!("failed to write {}", path.display()))
}

fn normalize_config(mut config: Gpt2ApiRsConfig) -> Result<Gpt2ApiRsConfig> {
    config.base_url = config.base_url.trim().trim_end_matches('/').to_string();
    config.admin_token = config.admin_token.trim().to_string();
    config.api_key = config.api_key.trim().to_string();
    if !config.base_url.is_empty() {
        let parsed = reqwest::Url::parse(&config.base_url)
            .with_context(|| format!("invalid gpt2api-rs base_url: {}", config.base_url))?;
        match parsed.scheme() {
            "http" | "https" => {},
            scheme => {
                anyhow::bail!("gpt2api-rs base_url scheme must be http/https, got `{scheme}`")
            },
        }
    }
    config.timeout_seconds = config.timeout_seconds.clamp(1, MAX_TIMEOUT_SECONDS);
    Ok(config)
}

fn bad_request(err: impl std::fmt::Display) -> (StatusCode, Json<ErrorResponse>) {
    error_response(StatusCode::BAD_REQUEST, err.to_string())
}

fn bad_gateway(err: impl std::fmt::Display) -> (StatusCode, Json<ErrorResponse>) {
    tracing::error!("gpt2api-rs upstream error: {err}");
    error_response(StatusCode::BAD_GATEWAY, "gpt2api-rs service is unavailable".to_string())
}

fn not_found(message: impl Into<String>) -> (StatusCode, Json<ErrorResponse>) {
    error_response(StatusCode::NOT_FOUND, message)
}

fn conflict_error(message: impl Into<String>) -> (StatusCode, Json<ErrorResponse>) {
    error_response(StatusCode::CONFLICT, message)
}

fn internal_error(err: impl std::fmt::Display) -> (StatusCode, Json<ErrorResponse>) {
    tracing::error!("gpt2api-rs internal error: {err}");
    error_response(StatusCode::INTERNAL_SERVER_ERROR, "gpt2api-rs proxy failed".to_string())
}

fn error_response(
    status: StatusCode,
    message: impl Into<String>,
) -> (StatusCode, Json<ErrorResponse>) {
    (
        status,
        Json(ErrorResponse {
            error: message.into(),
            code: status.as_u16(),
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_config_trims_and_clamps_timeout() {
        let config = normalize_config(Gpt2ApiRsConfig {
            base_url: " https://example.com/root/ ".to_string(),
            admin_token: " admin ".to_string(),
            api_key: " key ".to_string(),
            timeout_seconds: 999,
        })
        .expect("config should normalize");
        assert_eq!(config.base_url, "https://example.com/root");
        assert_eq!(config.admin_token, "admin");
        assert_eq!(config.api_key, "key");
        assert_eq!(config.timeout_seconds, MAX_TIMEOUT_SECONDS);
    }

    #[test]
    fn configured_url_for_public_proxy_strips_staticflow_api_prefix() {
        let config = Gpt2ApiRsConfig {
            base_url: "http://127.0.0.1:18787".to_string(),
            ..Gpt2ApiRsConfig::default()
        };
        let original_uri = OriginalUri(
            "/api/gpt2api/sessions/session-1?limit=20"
                .parse()
                .expect("valid uri"),
        );

        let url =
            configured_url_for_public_proxy(&config, &original_uri).expect("url should be built");

        assert_eq!(url, "http://127.0.0.1:18787/sessions/session-1?limit=20");
    }

    #[tokio::test]
    async fn save_and_load_round_trip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("gpt2api-rs.json");
        let config = Gpt2ApiRsConfig {
            base_url: "http://127.0.0.1:8787".to_string(),
            admin_token: "admin-token".to_string(),
            api_key: "public-key".to_string(),
            timeout_seconds: 42,
        };
        save_config(&path, &config).await.expect("save");
        let loaded = load_config_or_default(&path).await.expect("load");
        assert_eq!(loaded, config);
    }
}
