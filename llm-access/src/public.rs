//! Public unauthenticated compatibility endpoints.

use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use llm_access_core::store::{PublicAccessKey, PublicAccountContribution, PublicSponsor};
use serde::Serialize;

use crate::HttpState;

const MAX_PUBLIC_ACCOUNT_CONTRIBUTIONS: usize = 24;
const MAX_PUBLIC_SPONSORS: usize = 36;

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
