//! Public unauthenticated endpoints.

use std::time::Duration;

use axum::{
    body::Body,
    extract::{OriginalUri, Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use llm_access_core::store::{
    CodexPublicAccountStatus, CodexRateLimitStatus, ProviderCodexRoute, PublicAccessKey,
    PublicAccountContribution, PublicSponsor, PublicUsageLookupKey,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::{
    usage_query::{
        AdminUsageEventView, AdminUsageEventsResponse, AdminUsageTotalsView, UsageChartResponse,
    },
    HttpState,
};

const MAX_PUBLIC_ACCOUNT_CONTRIBUTIONS: usize = 24;
const MAX_PUBLIC_SPONSORS: usize = 36;
const PUBLIC_USAGE_LOOKUP_DEFAULT_LIMIT: usize = 20;
const PUBLIC_USAGE_LOOKUP_MAX_LIMIT: usize = 20;
const PUBLIC_USAGE_LOOKUP_CHART_BUCKETS: usize = 24;
const PUBLIC_USAGE_LOOKUP_BUCKET_MS: i64 = 60 * 60 * 1000;
const PUBLIC_USAGE_WORKER_QUERY_TIMEOUT: Duration = Duration::from_secs(30);
const PUBLIC_PAGE_CACHE_CONTROL: &str = "public, max-age=10, stale-while-revalidate=30";

type PublicEndpointResult<T> = Result<T, (StatusCode, &'static str)>;

#[derive(Debug, Serialize)]
struct LlmGatewayAccessResponse {
    base_url: String,
    gateway_path: String,
    model_catalog_path: String,
    auth_cache_ttl_seconds: u64,
    keys: Vec<LlmGatewayPublicKeyView>,
    generated_at: i64,
}

#[derive(Debug, Serialize)]
struct LlmGatewayPublicPageResponse {
    access: LlmGatewayAccessResponse,
    account_contributions: PublicLlmGatewayAccountContributionsResponse,
    support_config: LlmGatewaySupportConfigView,
    sponsors: PublicLlmGatewaySponsorsResponse,
    generated_at: i64,
}

#[derive(Debug, Serialize)]
struct LlmGatewayPublicKeyView {
    id: String,
    name: String,
    secret: String,
    quota_billable_limit: u64,
    usage_input_uncached_tokens: u64,
    usage_input_cached_tokens: u64,
    usage_output_tokens: u64,
    remaining_billable: i64,
    last_used_at: Option<i64>,
}

#[derive(Debug, Serialize)]
struct LlmGatewaySupportConfigView {
    sponsor_title: String,
    sponsor_intro: String,
    group_name: String,
    qq_group_number: String,
    group_invite_text: String,
    alipay_qr_url: String,
    wechat_qr_url: String,
    qq_group_qr_url: Option<String>,
    generated_at: i64,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PublicLlmGatewayUsageLookupRequest {
    api_key: String,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    start_ms: Option<i64>,
    #[serde(default)]
    end_ms: Option<i64>,
}

#[derive(Debug, Serialize)]
struct PublicLlmGatewayUsageLookupResponse {
    key: PublicLlmGatewayUsageKeyView,
    chart_points: Vec<PublicLlmGatewayUsageChartPointView>,
    total: usize,
    offset: usize,
    limit: usize,
    has_more: bool,
    totals: AdminUsageTotalsView,
    events: Vec<PublicLlmGatewayUsageEventView>,
    generated_at: i64,
}

#[derive(Debug, Serialize)]
struct PublicLlmGatewayUsageKeyView {
    name: String,
    provider_type: String,
    quota_billable_limit: u64,
    usage_input_uncached_tokens: u64,
    usage_input_cached_tokens: u64,
    usage_output_tokens: u64,
    usage_billable_tokens: u64,
    usage_credit_total: f64,
    usage_credit_missing_events: u64,
    remaining_billable: i64,
    last_used_at: Option<i64>,
}

#[derive(Debug, Serialize)]
struct PublicLlmGatewayUsageChartPointView {
    bucket_start_ms: i64,
    tokens: u64,
}

#[derive(Debug, Serialize)]
struct PublicLlmGatewayUsageEventView {
    id: String,
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
    created_at: i64,
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
    code: u16,
}

#[derive(Debug, Serialize)]
struct PublicLlmGatewayAccountContributionsResponse {
    contributions: Vec<PublicLlmGatewayAccountContributionView>,
    generated_at: i64,
}

#[derive(Debug, Serialize)]
struct PublicLlmGatewayAccountContributionView {
    request_id: String,
    account_name: String,
    contributor_message: String,
    github_id: Option<String>,
    processed_at: Option<i64>,
}

#[derive(Debug, Serialize)]
struct PublicLlmGatewaySponsorsResponse {
    sponsors: Vec<PublicLlmGatewaySponsorView>,
    generated_at: i64,
}

#[derive(Debug, Serialize)]
struct PublicLlmGatewaySponsorView {
    request_id: String,
    display_name: Option<String>,
    sponsor_message: String,
    github_id: Option<String>,
    processed_at: Option<i64>,
}

impl From<PublicAccessKey> for LlmGatewayPublicKeyView {
    fn from(value: PublicAccessKey) -> Self {
        let remaining_billable = value.remaining_billable();
        Self {
            id: value.key_id,
            name: value.key_name,
            secret: value.secret,
            quota_billable_limit: value.quota_billable_limit,
            usage_input_uncached_tokens: value.usage_input_uncached_tokens,
            usage_input_cached_tokens: value.usage_input_cached_tokens,
            usage_output_tokens: value.usage_output_tokens,
            remaining_billable,
            last_used_at: value.last_used_at_ms,
        }
    }
}

impl From<PublicAccountContribution> for PublicLlmGatewayAccountContributionView {
    fn from(value: PublicAccountContribution) -> Self {
        Self {
            request_id: value.request_id,
            account_name: value.account_name,
            contributor_message: value.contributor_message,
            github_id: value.github_id,
            processed_at: value.processed_at_ms,
        }
    }
}

impl From<PublicSponsor> for PublicLlmGatewaySponsorView {
    fn from(value: PublicSponsor) -> Self {
        Self {
            request_id: value.request_id,
            display_name: value.display_name,
            sponsor_message: value.sponsor_message,
            github_id: value.github_id,
            processed_at: value.processed_at_ms,
        }
    }
}

impl From<PublicUsageLookupKey> for PublicLlmGatewayUsageKeyView {
    fn from(value: PublicUsageLookupKey) -> Self {
        let remaining_billable = value.remaining_billable();
        Self {
            name: value.key_name,
            provider_type: value.provider_type,
            quota_billable_limit: value.quota_billable_limit,
            usage_input_uncached_tokens: value.usage_input_uncached_tokens,
            usage_input_cached_tokens: value.usage_input_cached_tokens,
            usage_output_tokens: value.usage_output_tokens,
            usage_billable_tokens: value.usage_billable_tokens,
            usage_credit_total: value.usage_credit_total,
            usage_credit_missing_events: value.usage_credit_missing_events,
            remaining_billable,
            last_used_at: value.last_used_at_ms,
        }
    }
}

#[derive(Debug, Serialize)]
struct KiroAccessResponse {
    base_url: String,
    gateway_path: String,
    auth_cache_ttl_seconds: u64,
    accounts: Vec<KiroPublicStatusView>,
    generated_at: i64,
}

#[derive(Debug, Serialize)]
struct KiroPublicStatusView {
    name: String,
    provider: Option<String>,
    disabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    disabled_reason: Option<String>,
    subscription_title: Option<String>,
    current_usage: Option<f64>,
    usage_limit: Option<f64>,
    remaining: Option<f64>,
    next_reset_at: Option<i64>,
    cache: KiroCacheView,
}

#[derive(Debug, Serialize)]
struct KiroCacheView {
    status: String,
    refresh_interval_seconds: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_checked_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_success_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_message: Option<String>,
}

pub(crate) async fn get_llm_gateway_access(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Response {
    match build_llm_gateway_access_response(&state, &headers).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => public_endpoint_error(err),
    }
}

pub(crate) async fn get_llm_gateway_public_page(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Response {
    let access = match build_llm_gateway_access_response(&state, &headers).await {
        Ok(response) => response,
        Err(err) => return public_endpoint_error(err),
    };
    let account_contributions = match build_llm_gateway_account_contributions_response(&state).await
    {
        Ok(response) => response,
        Err(err) => return public_endpoint_error(err),
    };
    let support_config = match build_llm_gateway_support_config_response() {
        Ok(response) => response,
        Err(err) => return public_endpoint_error(err),
    };
    let sponsors = match build_llm_gateway_sponsors_response(&state).await {
        Ok(response) => response,
        Err(err) => return public_endpoint_error(err),
    };
    let mut response = Json(LlmGatewayPublicPageResponse {
        access,
        account_contributions,
        support_config,
        sponsors,
        generated_at: now_ms(),
    })
    .into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static(PUBLIC_PAGE_CACHE_CONTROL));
    response
}

pub(crate) async fn get_llm_gateway_model_catalog(
    State(state): State<HttpState>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Response {
    let route = match select_public_codex_catalog_route(&state).await {
        Ok(Some(route)) => route,
        Ok(None) => return crate::provider::default_codex_public_model_catalog_response(),
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "public model catalog store error")
                .into_response()
        },
    };
    let codex_client_version = match state.admin_config_store.get_admin_runtime_config().await {
        Ok(config) => config.codex_client_version,
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "runtime config store error")
                .into_response()
        },
    };
    crate::provider::codex_public_model_catalog_response(
        route,
        state.provider_state.route_store(),
        &headers,
        uri.query().unwrap_or_default(),
        &crate::provider::codex_upstream_base_url(),
        &codex_client_version,
    )
    .await
}

async fn select_public_codex_catalog_route(
    state: &HttpState,
) -> anyhow::Result<Option<ProviderCodexRoute>> {
    let status = state.public_status_store.codex_rate_limit_status().await?;
    let Some(account_name) = preferred_public_codex_account_name(&status) else {
        return Ok(None);
    };
    state
        .admin_codex_account_store
        .resolve_admin_codex_account_route(&account_name)
        .await
}

fn preferred_public_codex_account_name(status: &CodexRateLimitStatus) -> Option<String> {
    let mut accounts = status
        .accounts
        .iter()
        .filter(|account| account.status == "active")
        .filter_map(public_codex_account_candidate)
        .collect::<Vec<_>>();
    accounts.sort_by(|left, right| {
        public_codex_account_cmp(left, right).then_with(|| left.0.cmp(&right.0))
    });
    accounts.into_iter().next().map(|account| account.0)
}

fn public_codex_account_candidate(
    account: &CodexPublicAccountStatus,
) -> Option<(String, f64, f64, bool)> {
    let (primary, primary_invalid) = sanitize_remaining_percent(account.primary_remaining_percent);
    let (secondary, secondary_invalid) =
        sanitize_remaining_percent(account.secondary_remaining_percent);
    if primary <= 0.0 || secondary <= 0.0 {
        return None;
    }
    Some((account.name.clone(), primary, secondary, primary_invalid || secondary_invalid))
}

fn sanitize_remaining_percent(value: Option<f64>) -> (f64, bool) {
    match value {
        Some(value) if value.is_finite() => (value, false),
        Some(_) => (100.0, true),
        None => (100.0, false),
    }
}

fn public_codex_account_cmp(
    left: &(String, f64, f64, bool),
    right: &(String, f64, f64, bool),
) -> std::cmp::Ordering {
    match (left.3, right.3) {
        (false, true) => return std::cmp::Ordering::Less,
        (true, false) => return std::cmp::Ordering::Greater,
        _ => {},
    }
    right
        .1
        .partial_cmp(&left.1)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| {
            right
                .2
                .partial_cmp(&left.2)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
}

pub(crate) async fn get_llm_gateway_status(State(state): State<HttpState>) -> Response {
    match state.public_status_store.codex_rate_limit_status().await {
        Ok(status) => Json(status).into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "public status store error").into_response(),
    }
}

pub(crate) async fn post_llm_gateway_public_usage_query(
    State(state): State<HttpState>,
    Json(request): Json<PublicLlmGatewayUsageLookupRequest>,
) -> Response {
    let presented_key = request.api_key.trim();
    if presented_key.is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "api_key is required");
    }
    let key = match state
        .public_usage_store
        .get_public_usage_key_by_secret(presented_key)
        .await
    {
        Ok(Some(key)) if key.status == "active" => key,
        Ok(_) => return public_usage_lookup_not_found(),
        Err(_) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, "public usage store error"),
    };
    let now = now_ms();
    let offset = request.offset.unwrap_or(0);
    let (start_ms, end_ms) = normalize_usage_time_range(request.start_ms, request.end_ms);
    let limit = request
        .limit
        .unwrap_or(PUBLIC_USAGE_LOOKUP_DEFAULT_LIMIT)
        .clamp(1, PUBLIC_USAGE_LOOKUP_MAX_LIMIT);
    let key_id = key.key_id.clone();
    let chart_start = public_usage_chart_window_start(now);
    let chart_params = vec![
        ("key_id", key_id.clone()),
        ("start_ms", chart_start.to_string()),
        ("bucket_ms", PUBLIC_USAGE_LOOKUP_BUCKET_MS.to_string()),
        ("bucket_count", PUBLIC_USAGE_LOOKUP_CHART_BUCKETS.to_string()),
    ];
    let chart = match fetch_usage_worker_json::<UsageChartResponse>(
        &state,
        "/admin/llm-access/usage/chart",
        &chart_params,
    )
    .await
    {
        Ok(chart) => chart,
        Err(response) => return response,
    };
    let chart_points = chart
        .chart_points
        .into_iter()
        .map(|point| PublicLlmGatewayUsageChartPointView {
            bucket_start_ms: point.bucket_start_ms,
            tokens: point.tokens,
        })
        .collect();
    let mut usage_params = vec![
        ("key_id", key_id),
        ("source", "all".to_string()),
        ("limit", limit.to_string()),
        ("offset", offset.to_string()),
    ];
    if let Some(start_ms) = start_ms {
        usage_params.push(("start_ms", start_ms.to_string()));
    }
    if let Some(end_ms) = end_ms {
        usage_params.push(("end_ms", end_ms.to_string()));
    }
    let page = match fetch_usage_worker_json::<AdminUsageEventsResponse>(
        &state,
        "/admin/llm-gateway/usage",
        &usage_params,
    )
    .await
    {
        Ok(page) => page,
        Err(response) => return response,
    };
    let mut response = Json(PublicLlmGatewayUsageLookupResponse {
        key: PublicLlmGatewayUsageKeyView::from(key),
        chart_points,
        total: page.total,
        offset: page.offset,
        limit: page.limit,
        has_more: page.has_more,
        totals: page.totals,
        events: page
            .events
            .iter()
            .map(PublicLlmGatewayUsageEventView::from)
            .collect(),
        generated_at: now,
    })
    .into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn fetch_usage_worker_json<T>(
    state: &HttpState,
    path: &str,
    query: &[(&str, String)],
) -> Result<T, Response>
where
    T: DeserializeOwned,
{
    let config = state
        .admin_config_store
        .get_admin_runtime_config()
        .await
        .map_err(|_| json_error(StatusCode::INTERNAL_SERVER_ERROR, "public usage store error"))?;
    let url = format!("{}{}", config.usage_query_base_url.trim_end_matches('/'), path);
    let client = reqwest::Client::builder()
        .timeout(PUBLIC_USAGE_WORKER_QUERY_TIMEOUT)
        .build()
        .map_err(|_| json_error(StatusCode::INTERNAL_SERVER_ERROR, "public usage store error"))?;
    let response = client.get(&url).query(query).send().await.map_err(|err| {
        tracing::warn!(url = %url, "public usage worker query failed: {err:#}");
        json_error(StatusCode::SERVICE_UNAVAILABLE, "usage worker is unavailable")
    })?;
    let status = response.status();
    if !status.is_success() {
        tracing::warn!(url = %url, status = %status, "public usage worker query returned non-success");
        return Err(json_error(StatusCode::SERVICE_UNAVAILABLE, "usage worker is unavailable"));
    }
    response.json::<T>().await.map_err(|err| {
        tracing::warn!(url = %url, "public usage worker query returned invalid JSON: {err:#}");
        json_error(StatusCode::SERVICE_UNAVAILABLE, "usage worker is unavailable")
    })
}

pub(crate) async fn get_llm_gateway_account_contributions(
    State(state): State<HttpState>,
) -> Response {
    match build_llm_gateway_account_contributions_response(&state).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => public_endpoint_error(err),
    }
}

pub(crate) async fn get_llm_gateway_sponsors(State(state): State<HttpState>) -> Response {
    match build_llm_gateway_sponsors_response(&state).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => public_endpoint_error(err),
    }
}

pub(crate) async fn get_llm_gateway_support_config() -> Response {
    match build_llm_gateway_support_config_response() {
        Ok(response) => Json(response).into_response(),
        Err(err) => public_endpoint_error(err),
    }
}

fn public_endpoint_error((status, message): (StatusCode, &'static str)) -> Response {
    (status, message).into_response()
}

async fn build_llm_gateway_access_response(
    state: &HttpState,
    headers: &HeaderMap,
) -> PublicEndpointResult<LlmGatewayAccessResponse> {
    let auth_cache_ttl_seconds = state
        .public_access_store
        .auth_cache_ttl_seconds()
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "public access store error"))?;
    let keys = state
        .public_access_store
        .list_public_access_keys()
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "public access store error"))?
        .into_iter()
        .map(LlmGatewayPublicKeyView::from)
        .collect();
    let gateway_path = "/api/llm-gateway/v1".to_string();
    let model_catalog_path = "/api/llm-gateway/model-catalog.json".to_string();
    let base_url = external_origin(headers)
        .map(|origin| format!("{origin}{gateway_path}"))
        .unwrap_or_else(|| gateway_path.clone());

    Ok(LlmGatewayAccessResponse {
        base_url,
        gateway_path,
        model_catalog_path,
        auth_cache_ttl_seconds,
        keys,
        generated_at: now_ms(),
    })
}

async fn build_llm_gateway_account_contributions_response(
    state: &HttpState,
) -> PublicEndpointResult<PublicLlmGatewayAccountContributionsResponse> {
    let contributions = state
        .public_community_store
        .list_public_account_contributions(MAX_PUBLIC_ACCOUNT_CONTRIBUTIONS)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "public community store error"))?
        .into_iter()
        .map(PublicLlmGatewayAccountContributionView::from)
        .collect();
    Ok(PublicLlmGatewayAccountContributionsResponse {
        contributions,
        generated_at: now_ms(),
    })
}

async fn build_llm_gateway_sponsors_response(
    state: &HttpState,
) -> PublicEndpointResult<PublicLlmGatewaySponsorsResponse> {
    let sponsors = state
        .public_community_store
        .list_public_sponsors(MAX_PUBLIC_SPONSORS)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "public community store error"))?
        .into_iter()
        .map(PublicLlmGatewaySponsorView::from)
        .collect();
    Ok(PublicLlmGatewaySponsorsResponse {
        sponsors,
        generated_at: now_ms(),
    })
}

fn build_llm_gateway_support_config_response() -> PublicEndpointResult<LlmGatewaySupportConfigView>
{
    let config = match crate::support::load_support_config() {
        Ok(config) => config,
        Err(_) => {
            return Err((StatusCode::INTERNAL_SERVER_ERROR, "failed to load support config"));
        },
    };
    let qq_group_qr_url = config
        .has_group_qr()
        .then(|| format!("/api/llm-gateway/support-assets/{}", crate::support::QQ_GROUP_QR_FILE));
    Ok(LlmGatewaySupportConfigView {
        sponsor_title: config.sponsor_title,
        sponsor_intro: config.sponsor_intro,
        group_name: config.group_name,
        qq_group_number: config.qq_group_number,
        group_invite_text: config.group_invite_text,
        alipay_qr_url: format!(
            "/api/llm-gateway/support-assets/{}",
            crate::support::ALIPAY_QR_FILE
        ),
        wechat_qr_url: format!(
            "/api/llm-gateway/support-assets/{}",
            crate::support::WECHAT_QR_FILE
        ),
        qq_group_qr_url,
        generated_at: now_ms(),
    })
}

pub(crate) async fn get_llm_gateway_support_asset(Path(file_name): Path<String>) -> Response {
    let asset = match crate::support::load_support_asset(&file_name) {
        Ok(asset) => asset,
        Err(_) => return (StatusCode::NOT_FOUND, "support asset not found").into_response(),
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, asset.content_type)
        .body(Body::from(asset.bytes))
        .unwrap_or_else(|_| {
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to build support asset response")
                .into_response()
        })
}

pub(crate) async fn get_kiro_gateway_access(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Response {
    let auth_cache_ttl_seconds = match state.public_access_store.auth_cache_ttl_seconds().await {
        Ok(value) => value,
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "public access store error").into_response()
        },
    };
    let gateway_path = "/api/kiro-gateway".to_string();
    let base_url = external_origin(&headers)
        .map(|origin| format!("{origin}{gateway_path}"))
        .unwrap_or_else(|| gateway_path.clone());

    Json(KiroAccessResponse {
        base_url,
        gateway_path,
        auth_cache_ttl_seconds,
        accounts: Vec::new(),
        generated_at: now_ms(),
    })
    .into_response()
}

fn public_usage_lookup_not_found() -> Response {
    json_error(StatusCode::NOT_FOUND, "queryable key not found")
}

fn json_error(status: StatusCode, message: &str) -> Response {
    (
        status,
        Json(ErrorResponse {
            error: message.to_string(),
            code: status.as_u16(),
        }),
    )
        .into_response()
}

impl From<&AdminUsageEventView> for PublicLlmGatewayUsageEventView {
    fn from(value: &AdminUsageEventView) -> Self {
        Self {
            id: value.id.clone(),
            key_name: value.key_name.clone(),
            account_name: value.account_name.clone(),
            request_method: value.request_method.clone(),
            request_url: value.request_url.clone(),
            latency_ms: value.latency_ms,
            routing_wait_ms: value.routing_wait_ms,
            upstream_headers_ms: value.upstream_headers_ms,
            post_headers_body_ms: value.post_headers_body_ms,
            request_body_bytes: value.request_body_bytes,
            request_body_read_ms: value.request_body_read_ms,
            request_json_parse_ms: value.request_json_parse_ms,
            pre_handler_ms: value.pre_handler_ms,
            first_sse_write_ms: value.first_sse_write_ms,
            stream_finish_ms: value.stream_finish_ms,
            other_latency_ms: value.other_latency_ms,
            quota_failover_count: value.quota_failover_count,
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

fn public_usage_chart_window_start(now_ms: i64) -> i64 {
    let current_bucket_start =
        now_ms.div_euclid(PUBLIC_USAGE_LOOKUP_BUCKET_MS) * PUBLIC_USAGE_LOOKUP_BUCKET_MS;
    current_bucket_start.saturating_sub(
        (PUBLIC_USAGE_LOOKUP_CHART_BUCKETS.saturating_sub(1) as i64)
            .saturating_mul(PUBLIC_USAGE_LOOKUP_BUCKET_MS),
    )
}

fn normalize_usage_time_range(
    start_ms: Option<i64>,
    end_ms: Option<i64>,
) -> (Option<i64>, Option<i64>) {
    let start_ms = start_ms.filter(|value| *value > 0);
    let end_ms = end_ms.filter(|value| *value > 0);
    match (start_ms, end_ms) {
        (Some(start), Some(end)) if start >= end => (Some(start), Some(start.saturating_add(1))),
        other => other,
    }
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

fn now_ms() -> i64 {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    millis.min(i64::MAX as u128) as i64
}

#[cfg(test)]
mod tests {
    use llm_access_core::store::KEY_STATUS_ACTIVE;

    use super::*;

    fn sample_public_account(
        name: &str,
        primary_remaining_percent: Option<f64>,
        secondary_remaining_percent: Option<f64>,
        usage_error_message: Option<&str>,
    ) -> CodexPublicAccountStatus {
        CodexPublicAccountStatus {
            name: name.to_string(),
            status: KEY_STATUS_ACTIVE.to_string(),
            plan_type: Some("Pro".to_string()),
            primary_remaining_percent,
            secondary_remaining_percent,
            last_usage_checked_at: Some(100),
            last_usage_success_at: Some(100),
            usage_error_message: usage_error_message.map(str::to_string),
        }
    }

    #[test]
    fn preferred_public_codex_account_name_uses_cached_status_snapshot() {
        let status = CodexRateLimitStatus {
            status: "ready".to_string(),
            refresh_interval_seconds: 300,
            last_checked_at: Some(100),
            last_success_at: Some(100),
            source_url: "https://chatgpt.com/backend-api/wham/usage".to_string(),
            error_message: None,
            accounts: vec![
                sample_public_account("beta", Some(85.0), Some(91.0), Some("bad refresh value")),
                sample_public_account("alpha", Some(93.0), Some(94.0), None),
                sample_public_account("gamma", Some(0.0), Some(80.0), None),
            ],
            buckets: Vec::new(),
        };

        let selected = preferred_public_codex_account_name(&status);

        assert_eq!(selected.as_deref(), Some("alpha"));
    }

    #[test]
    fn preferred_public_codex_account_name_skips_exhausted_accounts() {
        let status = CodexRateLimitStatus {
            status: "ready".to_string(),
            refresh_interval_seconds: 300,
            last_checked_at: Some(100),
            last_success_at: Some(100),
            source_url: "https://chatgpt.com/backend-api/wham/usage".to_string(),
            error_message: None,
            accounts: vec![sample_public_account("alpha", Some(0.0), Some(100.0), None)],
            buckets: Vec::new(),
        };

        assert_eq!(preferred_public_codex_account_name(&status), None);
    }
}
