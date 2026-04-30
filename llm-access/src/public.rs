//! Public unauthenticated compatibility endpoints.

use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use llm_access_core::{
    store::{
        PublicAccessKey, PublicAccountContribution, PublicSponsor, PublicUsageLookupKey,
        UsageEventQuery,
    },
    usage::UsageEvent,
};
use serde::{Deserialize, Serialize};

use crate::HttpState;

const MAX_PUBLIC_ACCOUNT_CONTRIBUTIONS: usize = 24;
const MAX_PUBLIC_SPONSORS: usize = 36;
const PUBLIC_USAGE_LOOKUP_DEFAULT_LIMIT: usize = 50;
const PUBLIC_USAGE_LOOKUP_MAX_LIMIT: usize = 200;
const PUBLIC_USAGE_LOOKUP_CHART_BUCKETS: usize = 24;
const PUBLIC_USAGE_LOOKUP_BUCKET_MS: i64 = 60 * 60 * 1000;

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
}

#[derive(Debug, Serialize)]
struct PublicLlmGatewayUsageLookupResponse {
    key: PublicLlmGatewayUsageKeyView,
    chart_points: Vec<PublicLlmGatewayUsageChartPointView>,
    total: usize,
    offset: usize,
    limit: usize,
    has_more: bool,
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
    let auth_cache_ttl_seconds = match state.public_access_store.auth_cache_ttl_seconds().await {
        Ok(value) => value,
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "public access store error").into_response()
        },
    };
    let keys = match state.public_access_store.list_public_access_keys().await {
        Ok(keys) => keys
            .into_iter()
            .map(LlmGatewayPublicKeyView::from)
            .collect(),
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "public access store error").into_response()
        },
    };
    let gateway_path = "/api/llm-gateway/v1".to_string();
    let model_catalog_path = "/api/llm-gateway/model-catalog.json".to_string();
    let base_url = external_origin(&headers)
        .map(|origin| format!("{origin}{gateway_path}"))
        .unwrap_or_else(|| gateway_path.clone());

    Json(LlmGatewayAccessResponse {
        base_url,
        gateway_path,
        model_catalog_path,
        auth_cache_ttl_seconds,
        keys,
        generated_at: now_ms(),
    })
    .into_response()
}

pub(crate) async fn get_llm_gateway_model_catalog() -> Response {
    let body = match llm_access_codex::models::default_public_model_catalog_json() {
        Ok(body) => body,
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "failed to build model catalog")
                .into_response()
        },
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::CONTENT_DISPOSITION, r#"inline; filename="model_catalog.json""#)
        .body(Body::from(body))
        .unwrap_or_else(|_| {
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to build model catalog response")
                .into_response()
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
    let limit = request
        .limit
        .unwrap_or(PUBLIC_USAGE_LOOKUP_DEFAULT_LIMIT)
        .clamp(1, PUBLIC_USAGE_LOOKUP_MAX_LIMIT);
    let key_id = key.key_id.clone();
    let chart_start = public_usage_chart_window_start(now);
    let chart_points = match state
        .usage_analytics_store
        .usage_chart_points(
            &key_id,
            chart_start,
            PUBLIC_USAGE_LOOKUP_BUCKET_MS,
            PUBLIC_USAGE_LOOKUP_CHART_BUCKETS,
        )
        .await
    {
        Ok(points) => points
            .into_iter()
            .map(|point| PublicLlmGatewayUsageChartPointView {
                bucket_start_ms: point.bucket_start_ms,
                tokens: point.tokens,
            })
            .collect(),
        Err(_) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, "public usage store error"),
    };
    let page = match state
        .usage_analytics_store
        .list_usage_events(UsageEventQuery {
            key_id: Some(key_id),
            limit,
            offset,
        })
        .await
    {
        Ok(page) => page,
        Err(_) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, "public usage store error"),
    };
    let mut response = Json(PublicLlmGatewayUsageLookupResponse {
        key: PublicLlmGatewayUsageKeyView::from(key),
        chart_points,
        total: page.total,
        offset,
        limit,
        has_more: page.has_more,
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

pub(crate) async fn get_llm_gateway_account_contributions(
    State(state): State<HttpState>,
) -> Response {
    let contributions = match state
        .public_community_store
        .list_public_account_contributions(MAX_PUBLIC_ACCOUNT_CONTRIBUTIONS)
        .await
    {
        Ok(contributions) => contributions
            .into_iter()
            .map(PublicLlmGatewayAccountContributionView::from)
            .collect(),
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "public community store error")
                .into_response()
        },
    };
    Json(PublicLlmGatewayAccountContributionsResponse {
        contributions,
        generated_at: now_ms(),
    })
    .into_response()
}

pub(crate) async fn get_llm_gateway_sponsors(State(state): State<HttpState>) -> Response {
    let sponsors = match state
        .public_community_store
        .list_public_sponsors(MAX_PUBLIC_SPONSORS)
        .await
    {
        Ok(sponsors) => sponsors
            .into_iter()
            .map(PublicLlmGatewaySponsorView::from)
            .collect(),
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "public community store error")
                .into_response()
        },
    };
    Json(PublicLlmGatewaySponsorsResponse {
        sponsors,
        generated_at: now_ms(),
    })
    .into_response()
}

pub(crate) async fn get_llm_gateway_support_config() -> Response {
    let config = match crate::support::load_support_config() {
        Ok(config) => config,
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "failed to load support config")
                .into_response()
        },
    };
    let qq_group_qr_url = config
        .has_group_qr()
        .then(|| format!("/api/llm-gateway/support-assets/{}", crate::support::QQ_GROUP_QR_FILE));
    Json(LlmGatewaySupportConfigView {
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
    .into_response()
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

impl From<&UsageEvent> for PublicLlmGatewayUsageEventView {
    fn from(value: &UsageEvent) -> Self {
        let latency_ms = usage_latency_ms(value);
        Self {
            id: value.event_id.clone(),
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

fn public_usage_chart_window_start(now_ms: i64) -> i64 {
    let current_bucket_start =
        now_ms.div_euclid(PUBLIC_USAGE_LOOKUP_BUCKET_MS) * PUBLIC_USAGE_LOOKUP_BUCKET_MS;
    current_bucket_start.saturating_sub(
        (PUBLIC_USAGE_LOOKUP_CHART_BUCKETS.saturating_sub(1) as i64)
            .saturating_mul(PUBLIC_USAGE_LOOKUP_BUCKET_MS),
    )
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
