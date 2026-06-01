//! Codex model-catalog fetch/parse + OpenAI-models responses.

use std::sync::Arc;

use axum::{
    body::Body,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use llm_access_core::store::{ProviderCodexRoute, ProviderRouteStore};
use serde_json::Value;

use super::{
    client::provider_client,
    codex_auth::{
        codex_user_agent, compute_codex_upstream_url, header_value, normalize_codex_client_version,
        resolve_codex_client_version,
    },
    util::now_seconds,
    CodexAuthSnapshot, DEFAULT_WIRE_ORIGINATOR,
};
use crate::codex_refresh;

pub(crate) async fn codex_openai_models_response(
    route: ProviderCodexRoute,
    route_store: Arc<dyn ProviderRouteStore>,
    request_headers: &HeaderMap,
    query: &str,
    upstream_base: &str,
    default_codex_client_version: &str,
) -> Response {
    let (payload, etag) = match fetch_codex_models_payload(
        &route,
        route_store,
        request_headers,
        query,
        upstream_base,
        default_codex_client_version,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    let body = match serde_json::to_vec(
        &llm_access_codex::models::openai_models_response_value_from_catalog(
            &payload,
            route.map_gpt53_codex_to_spark,
            now_seconds(),
        ),
    ) {
        Ok(body) => body,
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "failed to encode codex models response")
                .into_response()
        },
    };
    codex_models_json_response(
        body,
        "application/json",
        None,
        etag.as_deref(),
        "failed to build codex models response",
    )
}
pub(crate) async fn codex_public_model_catalog_response(
    route: ProviderCodexRoute,
    route_store: Arc<dyn ProviderRouteStore>,
    request_headers: &HeaderMap,
    query: &str,
    upstream_base: &str,
    default_codex_client_version: &str,
) -> Response {
    let (payload, etag) = match fetch_codex_models_payload(
        &route,
        route_store,
        request_headers,
        query,
        upstream_base,
        default_codex_client_version,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    let catalog = match llm_access_codex::models::normalize_public_model_catalog_value(
        payload,
        route.map_gpt53_codex_to_spark,
    ) {
        Ok(value) => value,
        Err(_) => {
            return (StatusCode::BAD_GATEWAY, "failed to normalize codex model catalog")
                .into_response()
        },
    };
    let body = match serde_json::to_vec(&catalog) {
        Ok(body) => body,
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "failed to encode codex model catalog")
                .into_response()
        },
    };
    codex_models_json_response(
        body,
        "application/json; charset=utf-8",
        Some(r#"inline; filename="model_catalog.json""#),
        etag.as_deref(),
        "failed to build codex model catalog response",
    )
}
pub(crate) fn default_codex_public_model_catalog_response() -> Response {
    let body = match llm_access_codex::models::default_public_model_catalog_json() {
        Ok(body) => body,
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "failed to build model catalog")
                .into_response()
        },
    };
    codex_models_json_response(
        body,
        "application/json; charset=utf-8",
        Some(r#"inline; filename="model_catalog.json""#),
        None,
        "failed to build model catalog response",
    )
}
fn codex_models_json_response(
    body: Vec<u8>,
    content_type: &'static str,
    content_disposition: Option<&'static str>,
    etag: Option<&str>,
    build_error: &'static str,
) -> Response {
    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "no-store");
    if let Some(value) = content_disposition {
        builder = builder.header(header::CONTENT_DISPOSITION, value);
    }
    if let Some(value) = etag {
        builder = builder.header(header::ETAG, value);
    }
    builder
        .body(Body::from(body))
        .unwrap_or_else(|_| (StatusCode::INTERNAL_SERVER_ERROR, build_error).into_response())
}
async fn fetch_codex_models_payload(
    route: &ProviderCodexRoute,
    route_store: Arc<dyn ProviderRouteStore>,
    request_headers: &HeaderMap,
    query: &str,
    upstream_base: &str,
    default_codex_client_version: &str,
) -> Result<(Value, Option<String>), Response> {
    let mut auth =
        match codex_refresh::ensure_context_for_route(route, route_store.as_ref(), false).await {
            Ok(ctx) => CodexAuthSnapshot {
                access_token: ctx.access_token,
                account_id: ctx.account_id,
                is_fedramp_account: ctx.is_fedramp_account,
            },
            Err(_) => {
                return Err((StatusCode::BAD_GATEWAY, "codex auth refresh failed").into_response())
            },
        };
    let client_version = codex_models_client_version(query, default_codex_client_version);
    let upstream_url = llm_access_codex::models::append_client_version_query(
        &compute_codex_upstream_url(upstream_base, "/v1/models"),
        &client_version,
    );
    let client = provider_client(route.proxy.as_ref()).map_err(|_| {
        (StatusCode::BAD_GATEWAY, "codex proxy configuration failed").into_response()
    })?;
    let mut response =
        send_codex_models_request(&client, &upstream_url, request_headers, &auth, &client_version)
            .await?;
    if matches!(response.status(), StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        auth = match codex_refresh::ensure_context_for_route(route, route_store.as_ref(), true)
            .await
        {
            Ok(ctx) => CodexAuthSnapshot {
                access_token: ctx.access_token,
                account_id: ctx.account_id,
                is_fedramp_account: ctx.is_fedramp_account,
            },
            Err(_) => {
                return Err((StatusCode::BAD_GATEWAY, "codex auth refresh failed").into_response())
            },
        };
        response = send_codex_models_request(
            &client,
            &upstream_url,
            request_headers,
            &auth,
            &client_version,
        )
        .await?;
    }
    parse_codex_models_payload(response).await
}
fn codex_models_client_version(query: &str, default_codex_client_version: &str) -> String {
    query
        .trim_start_matches('?')
        .split('&')
        .filter_map(|part| part.split_once('='))
        .find_map(|(name, value)| {
            (name == "client_version")
                .then_some(value)
                .and_then(normalize_codex_client_version)
        })
        .unwrap_or_else(|| resolve_codex_client_version(Some(default_codex_client_version)))
}
async fn send_codex_models_request(
    client: &reqwest::Client,
    upstream_url: &str,
    request_headers: &HeaderMap,
    auth: &CodexAuthSnapshot,
    client_version: &str,
) -> Result<reqwest::Response, Response> {
    let mut request = client
        .get(upstream_url)
        .bearer_auth(&auth.access_token)
        .header(reqwest::header::ACCEPT, "application/json")
        .header(
            reqwest::header::USER_AGENT,
            header_value(request_headers, header::USER_AGENT.as_str())
                .unwrap_or_else(|| codex_user_agent(client_version)),
        )
        .header(
            reqwest::header::HeaderName::from_static("originator"),
            header_value(request_headers, "originator")
                .unwrap_or_else(|| DEFAULT_WIRE_ORIGINATOR.to_string()),
        );
    if let Some(account_id) = auth.account_id.as_deref() {
        request = request.header("chatgpt-account-id", account_id);
    }
    if auth.is_fedramp_account {
        request = request.header("x-openai-fedramp", "true");
    }
    request.send().await.map_err(|_| {
        (StatusCode::BAD_GATEWAY, "codex models upstream request failed").into_response()
    })
}
async fn parse_codex_models_payload(
    response: reqwest::Response,
) -> Result<(Value, Option<String>), Response> {
    let status = response.status();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);
    let body = response.bytes().await.map_err(|_| {
        (StatusCode::BAD_GATEWAY, "codex models upstream response read failed").into_response()
    })?;
    if !status.is_success() {
        return Err((
            StatusCode::BAD_GATEWAY,
            format!(
                "codex models upstream returned status={} body={}",
                status,
                summarize_body_hint(body.as_ref())
            ),
        )
            .into_response());
    }
    if content_type.contains("text/html") {
        return Err((
            StatusCode::BAD_GATEWAY,
            format!(
                "codex models upstream returned html body={}",
                summarize_body_hint(body.as_ref())
            ),
        )
            .into_response());
    }
    let value = serde_json::from_slice::<Value>(body.as_ref()).map_err(|_| {
        (StatusCode::BAD_GATEWAY, "codex models upstream returned invalid json").into_response()
    })?;
    Ok((value, etag))
}
fn summarize_body_hint(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        "empty body".to_string()
    } else {
        trimmed.chars().take(200).collect()
    }
}
