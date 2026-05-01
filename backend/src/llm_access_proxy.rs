//! Reverse proxy for running StaticFlow with an external `llm-access` service.

use std::{env, sync::OnceLock};

use anyhow::{anyhow, Context, Result};
use axum::{
    body::{to_bytes, Body},
    extract::Request,
    http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode, Uri},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use llm_access_core::routes::is_llm_access_path;
use serde_json::json;

const MODE_ENV: &str = "STATICFLOW_LLM_ACCESS_MODE";
const URL_ENV: &str = "STATICFLOW_LLM_ACCESS_URL";
const MAX_BODY_ENV: &str = "STATICFLOW_LLM_ACCESS_PROXY_MAX_BODY_BYTES";
const DEFAULT_EXTERNAL_URL: &str = "http://127.0.0.1:19182";
const DEFAULT_PROXY_MAX_BODY_BYTES: usize = 64 * 1024 * 1024;
const X_FORWARDED_HOST: &str = "x-forwarded-host";
const X_FORWARDED_PROTO: &str = "x-forwarded-proto";

#[derive(Debug, Clone)]
struct ExternalLlmAccessProxyConfig {
    base_url: reqwest::Url,
    max_body_bytes: usize,
}

/// Return whether this backend should delegate LLM routes to standalone
/// `llm-access` instead of running its own provider path.
pub(crate) fn is_external_mode_enabled() -> bool {
    env::var(MODE_ENV)
        .ok()
        .map(|value| value.trim().eq_ignore_ascii_case("external"))
        .unwrap_or(false)
}

/// Middleware entry point. In normal mode it is a no-op. In external mode it
/// proxies every LLM-owned path to the standalone service before Axum route
/// matching reaches the built-in handlers.
pub(crate) async fn proxy_middleware(request: Request, next: Next) -> Response {
    if !is_external_mode_enabled() || !is_llm_access_path(request.uri().path()) {
        return next.run(request).await;
    }

    match forward_to_external_llm_access(request).await {
        Ok(response) => response,
        Err(err) => {
            tracing::error!(error = %err, "failed to proxy request to external llm-access");
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "error": {
                        "code": "llm_access_proxy_error",
                        "message": "Failed to proxy request to external llm-access"
                    }
                })),
            )
                .into_response()
        },
    }
}

async fn forward_to_external_llm_access(request: Request) -> Result<Response> {
    let config = resolve_external_proxy_config()?;
    let (parts, body) = request.into_parts();
    let target_url = target_url_for(&config.base_url, parts.uri.path(), parts.uri.query());
    let method = reqwest::Method::from_bytes(parts.method.as_str().as_bytes())
        .context("failed to convert request method for llm-access proxy")?;
    let body_bytes = to_bytes(body, config.max_body_bytes)
        .await
        .context("failed to read request body for llm-access proxy")?;

    let mut upstream = proxy_client().request(method, target_url.clone());
    for (name, value) in &parts.headers {
        if should_forward_request_header(name) {
            upstream = upstream.header(name, value.clone());
        }
    }
    if !parts.headers.contains_key(X_FORWARDED_HOST) {
        if let Some(host) = forwarded_host_header(&parts.headers) {
            upstream = upstream.header(X_FORWARDED_HOST, host);
        }
    }
    if !parts.headers.contains_key(X_FORWARDED_PROTO) {
        upstream =
            upstream.header(X_FORWARDED_PROTO, forwarded_proto_header(&parts.headers, &parts.uri));
    }

    let upstream = upstream
        .body(body_bytes)
        .send()
        .await
        .with_context(|| format!("failed to send proxied request to {target_url}"))?;
    let status = StatusCode::from_u16(upstream.status().as_u16())
        .context("external llm-access returned invalid HTTP status")?;
    let mut builder = Response::builder().status(status);
    for (name, value) in upstream.headers() {
        if should_forward_response_header(name) {
            builder = builder.header(name, value.clone());
        }
    }

    builder
        .body(Body::from_stream(upstream.bytes_stream()))
        .context("failed to build external llm-access proxy response")
}

fn proxy_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new)
}

fn resolve_external_proxy_config() -> Result<ExternalLlmAccessProxyConfig> {
    let raw_url = env::var(URL_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_EXTERNAL_URL.to_string());
    let base_url =
        reqwest::Url::parse(&raw_url).with_context(|| format!("invalid {URL_ENV}: {raw_url}"))?;
    let max_body_bytes = env::var(MAX_BODY_ENV)
        .ok()
        .map(|value| {
            value
                .trim()
                .parse::<usize>()
                .with_context(|| format!("invalid {MAX_BODY_ENV}: {value}"))
        })
        .transpose()?
        .unwrap_or(DEFAULT_PROXY_MAX_BODY_BYTES);
    if max_body_bytes == 0 {
        return Err(anyhow!("{MAX_BODY_ENV} must be positive"));
    }

    Ok(ExternalLlmAccessProxyConfig {
        base_url,
        max_body_bytes,
    })
}

fn target_url_for(base_url: &reqwest::Url, path: &str, query: Option<&str>) -> reqwest::Url {
    let mut target = base_url.clone();
    target.set_path(path.trim_start_matches('/'));
    target.set_query(query);
    target
}

fn should_forward_request_header(name: &HeaderName) -> bool {
    !is_hop_by_hop_header(name)
        && name != header::HOST
        && name != header::CONTENT_LENGTH
        && name != header::TRANSFER_ENCODING
}

fn forwarded_host_header(headers: &HeaderMap) -> Option<HeaderValue> {
    headers.get(header::HOST).cloned()
}

fn forwarded_proto_header(headers: &HeaderMap, uri: &Uri) -> HeaderValue {
    if let Some(value) = headers.get(X_FORWARDED_PROTO) {
        return value.clone();
    }
    match uri.scheme_str() {
        Some("https") => HeaderValue::from_static("https"),
        _ => HeaderValue::from_static("http"),
    }
}

fn should_forward_response_header(name: &HeaderName) -> bool {
    !is_hop_by_hop_header(name) && name != header::TRANSFER_ENCODING
}

fn is_hop_by_hop_header(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "upgrade"
    )
}

#[cfg(test)]
mod tests {
    use axum::http::{header, HeaderMap, HeaderValue, Uri};

    use super::{
        forwarded_host_header, forwarded_proto_header, should_forward_request_header,
        should_forward_response_header, target_url_for,
    };

    #[test]
    fn target_url_preserves_path_and_query_for_external_llm_access() {
        let base = reqwest::Url::parse("http://127.0.0.1:19182").expect("base url");
        let target = target_url_for(&base, "/api/kiro-gateway/cc/v1/messages", Some("stream=true"));

        assert_eq!(
            target.as_str(),
            "http://127.0.0.1:19182/api/kiro-gateway/cc/v1/messages?stream=true"
        );
    }

    #[test]
    fn proxy_header_filter_removes_hop_by_hop_headers() {
        assert!(!should_forward_request_header(&header::HOST));
        assert!(!should_forward_request_header(&header::CONNECTION));
        assert!(!should_forward_request_header(&header::TRANSFER_ENCODING));
        assert!(!should_forward_response_header(&header::TRANSFER_ENCODING));
        assert!(should_forward_request_header(&header::AUTHORIZATION));
        assert!(should_forward_response_header(&header::CONTENT_TYPE));
    }

    #[test]
    fn forwarded_identity_falls_back_to_original_host() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("127.0.0.1:39082"));
        let uri: Uri = "/api/llm-gateway/access".parse().expect("uri");

        assert_eq!(
            forwarded_host_header(&headers).as_ref(),
            Some(&HeaderValue::from_static("127.0.0.1:39082"))
        );
        assert_eq!(forwarded_proto_header(&headers, &uri), "http");
    }

    #[test]
    fn forwarded_proto_uses_https_absolute_uri() {
        let headers = HeaderMap::new();
        let uri: Uri = "https://ackingliu.top/api/llm-gateway/access"
            .parse()
            .expect("uri");

        assert_eq!(forwarded_proto_header(&headers, &uri), "https");
    }
}
