use std::sync::Arc;

use anyhow::{Context, Result};
use axum::{
    body::{Body, Bytes},
    extract::{OriginalUri, State},
    http::{header, HeaderMap, Method, StatusCode},
    response::{Json, Response},
};

use crate::{
    handlers::{ensure_admin_access, ErrorResponse},
    state::AppState,
};

const DEFAULT_LLM_ACCESS_ADMIN_PROXY_BASE_URL: &str = "http://127.0.0.1:19182";
const LLM_ACCESS_ADMIN_PROXY_BASE_ENV: &str = "STATICFLOW_LLM_ACCESS_ADMIN_BASE";

type HandlerResult<T> = Result<T, (StatusCode, Json<ErrorResponse>)>;

#[derive(Clone)]
pub struct LlmAccessAdminProxyState {
    client: reqwest::Client,
    base_url: reqwest::Url,
}

impl LlmAccessAdminProxyState {
    pub fn from_env() -> Result<Arc<Self>> {
        let base_url = std::env::var(LLM_ACCESS_ADMIN_PROXY_BASE_ENV)
            .ok()
            .map(|value| value.trim().trim_end_matches('/').to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| DEFAULT_LLM_ACCESS_ADMIN_PROXY_BASE_URL.to_string());
        let base_url = reqwest::Url::parse(&base_url)
            .with_context(|| format!("invalid {LLM_ACCESS_ADMIN_PROXY_BASE_ENV}"))?;
        let client = reqwest::Client::builder().build()?;
        tracing::info!(base_url = %base_url, "llm-access admin proxy initialized");
        Ok(Arc::new(Self {
            client,
            base_url,
        }))
    }

    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    pub fn base_url(&self) -> &reqwest::Url {
        &self.base_url
    }
}

pub async fn proxy_admin_request(
    State(state): State<AppState>,
    method: Method,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    body: Bytes,
) -> HandlerResult<Response> {
    ensure_admin_access(&state, &headers)?;

    let normalized = normalize_llm_access_admin_path_and_query(
        uri.path_and_query()
            .map(|value| value.as_str())
            .unwrap_or_else(|| uri.path()),
    );
    let response = forward_llm_access_admin_request(
        state.llm_access_admin_proxy.client(),
        state.llm_access_admin_proxy.base_url(),
        method,
        &normalized,
        &headers,
        body,
    )
    .await
    .map_err(bad_gateway)?;

    Ok(response)
}

pub async fn proxy_public_request(
    State(state): State<AppState>,
    method: Method,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    body: Bytes,
) -> HandlerResult<Response> {
    let normalized = normalize_llm_access_admin_path_and_query(
        uri.path_and_query()
            .map(|value| value.as_str())
            .unwrap_or_else(|| uri.path()),
    );
    let response = forward_llm_access_admin_request(
        state.llm_access_admin_proxy.client(),
        state.llm_access_admin_proxy.base_url(),
        method,
        &normalized,
        &headers,
        body,
    )
    .await
    .map_err(bad_gateway)?;

    Ok(response)
}

fn join_proxy_url(base_url: &reqwest::Url, relative: &str) -> Result<reqwest::Url> {
    base_url
        .join(relative)
        .with_context(|| format!("failed to join llm-access admin proxy path {relative}"))
}

fn normalize_llm_access_admin_path_and_query(path_and_query: &str) -> String {
    if let Some(stripped) = path_and_query.strip_prefix("/static_flow") {
        stripped.to_string()
    } else {
        path_and_query.to_string()
    }
}

async fn forward_llm_access_admin_request(
    client: &reqwest::Client,
    base_url: &reqwest::Url,
    method: Method,
    path_and_query: &str,
    headers: &HeaderMap,
    body: Bytes,
) -> Result<Response> {
    let url = join_proxy_url(base_url, path_and_query)?;
    let mut request = client.request(method, url);
    if let Some(value) = headers.get(header::CONTENT_TYPE).cloned() {
        request = request.header(header::CONTENT_TYPE, value);
    }
    if let Some(value) = headers.get(header::ACCEPT).cloned() {
        request = request.header(header::ACCEPT, value);
    }
    if let Some(value) = headers.get("x-admin-token").cloned() {
        request = request.header("x-admin-token", value);
    }
    if let Some(value) = headers.get("x-request-id").cloned() {
        request = request.header("x-request-id", value);
    }
    if let Some(value) = headers.get("x-trace-id").cloned() {
        request = request.header("x-trace-id", value);
    }
    if !body.is_empty() {
        request = request.body(body);
    }

    let upstream = request
        .send()
        .await
        .context("failed to send llm-access admin proxy request")?;
    let status = StatusCode::from_u16(upstream.status().as_u16())
        .context("upstream returned invalid HTTP status code")?;
    let content_type = upstream.headers().get(header::CONTENT_TYPE).cloned();
    let cache_control = upstream.headers().get(header::CACHE_CONTROL).cloned();
    let content_disposition = upstream.headers().get(header::CONTENT_DISPOSITION).cloned();
    let bytes = upstream
        .bytes()
        .await
        .context("failed to read llm-access admin proxy response")?;

    let mut builder = Response::builder().status(status);
    if let Some(value) = content_type {
        builder = builder.header(header::CONTENT_TYPE, value);
    }
    if let Some(value) = cache_control {
        builder = builder.header(header::CACHE_CONTROL, value);
    }
    if let Some(value) = content_disposition {
        builder = builder.header(header::CONTENT_DISPOSITION, value);
    }
    builder
        .body(Body::from(bytes))
        .context("failed to build llm-access admin proxy response")
}

fn bad_gateway(err: anyhow::Error) -> (StatusCode, Json<ErrorResponse>) {
    tracing::warn!(error = %err, "llm-access admin proxy request failed");
    (
        StatusCode::BAD_GATEWAY,
        Json(ErrorResponse {
            error: format!("llm-access admin proxy failed: {err}"),
            code: StatusCode::BAD_GATEWAY.as_u16(),
        }),
    )
}

#[cfg(test)]
mod tests {
    use axum::{
        body::{to_bytes, Bytes},
        http::{header, HeaderMap, Method, StatusCode},
    };
    use serde_json::json;

    use super::{forward_llm_access_admin_request, normalize_llm_access_admin_path_and_query};

    #[test]
    fn normalize_llm_access_admin_path_strips_static_flow_prefix() {
        assert_eq!(
            normalize_llm_access_admin_path_and_query(
                "/static_flow/admin/llm-gateway/usage/metrics?window=1h"
            ),
            "/admin/llm-gateway/usage/metrics?window=1h"
        );
        assert_eq!(
            normalize_llm_access_admin_path_and_query("/admin/kiro-gateway/accounts"),
            "/admin/kiro-gateway/accounts"
        );
    }

    #[tokio::test]
    async fn forward_llm_access_admin_request_preserves_query_and_json_content_type() {
        let upstream = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/admin/llm-gateway/usage/metrics"))
            .and(wiremock::matchers::query_param("window", "15m"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_json(json!({"ok": true})),
            )
            .mount(&upstream)
            .await;

        let response = forward_llm_access_admin_request(
            &reqwest::Client::new(),
            &reqwest::Url::parse(&upstream.uri()).expect("base url"),
            Method::GET,
            "/admin/llm-gateway/usage/metrics?window=15m",
            &HeaderMap::new(),
            Bytes::new(),
        )
        .await
        .expect("forward response");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE),
            Some(&header::HeaderValue::from_static("application/json"))
        );
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body bytes");
        assert_eq!(body.as_ref(), br#"{"ok":true}"#);
    }
}
