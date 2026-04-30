use anyhow::{anyhow, Context, Result};
use axum::{
    body::Body,
    http::{header, HeaderMap, Response, StatusCode},
};
pub(crate) use llm_access_codex::models::append_client_version_query;
use llm_access_codex::models::{
    apply_model_aliases, extract_gateway_model_descriptors, gateway_models_owner,
    normalize_public_model_catalog_value,
};
use reqwest::header::{HeaderMap as ReqwestHeaderMap, HeaderValue as ReqwestHeaderValue};
use serde_json::{json, Value};

use super::{
    codex_user_agent, compute_upstream_url, internal_error,
    request::{extract_header_value, extract_query_param, normalize_upstream_base_url},
    resolve_codex_client_version,
    runtime::{
        bearer_header, codex_upstream_client_profile, CodexAuthSnapshot, LlmGatewayRuntimeState,
    },
    DEFAULT_UPSTREAM_BASE_URL, DEFAULT_WIRE_ORIGINATOR,
};
use crate::state::AppState;

/// Serve `/v1/models` by querying the upstream Codex models endpoint.
pub(crate) async fn respond_local_models(
    state: &AppState,
    auth_snapshot: &CodexAuthSnapshot,
    incoming_headers: &HeaderMap,
    query: &str,
    map_gpt53_codex_to_spark: bool,
) -> Result<Response<Body>, (StatusCode, axum::response::Json<crate::handlers::ErrorResponse>)> {
    let (payload, etag) =
        fetch_codex_models_payload(&state.llm_gateway, auth_snapshot, incoming_headers, query)
            .await
            .map_err(|err| internal_error("Failed to fetch llm gateway models", err))?;
    let merged = apply_model_aliases(
        extract_gateway_model_descriptors(&payload, gateway_models_owner()),
        map_gpt53_codex_to_spark,
    );

    let created = static_flow_shared::llm_gateway_store::now_ms() / 1000;
    let data = merged
        .into_iter()
        .map(|item| {
            json!({
                "id": item.id,
                "object": "model",
                "created": created,
                "owned_by": item.owned_by,
            })
        })
        .collect::<Vec<_>>();
    let body = serde_json::to_vec(&json!({
        "object": "list",
        "data": data,
    }))
    .map_err(|err| internal_error("Failed to encode llm gateway models response", err))?;

    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, "no-store");
    if let Some(etag) = etag.as_deref() {
        builder = builder.header(header::ETAG, etag);
    }
    builder
        .body(Body::from(body))
        .map_err(|err| internal_error("Failed to build llm gateway models response", err))
}

/// Serve the raw `model_catalog.json` payload that Codex can load locally.
pub(crate) async fn respond_public_model_catalog(
    state: &AppState,
    auth_snapshot: &CodexAuthSnapshot,
    incoming_headers: &HeaderMap,
    query: &str,
    map_gpt53_codex_to_spark: bool,
) -> Result<Response<Body>, (StatusCode, axum::response::Json<crate::handlers::ErrorResponse>)> {
    let (payload, etag) =
        fetch_codex_models_payload(&state.llm_gateway, auth_snapshot, incoming_headers, query)
            .await
            .map_err(|err| internal_error("Failed to fetch llm gateway model catalog", err))?;
    let body = serde_json::to_vec(
        &normalize_public_model_catalog_value(payload, map_gpt53_codex_to_spark)
            .map_err(|err| internal_error("Failed to normalize public model catalog", err))?,
    )
    .map_err(|err| internal_error("Failed to encode public model catalog", err))?;

    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::CONTENT_DISPOSITION, r#"inline; filename="model_catalog.json""#);
    if let Some(etag) = etag.as_deref() {
        builder = builder.header(header::ETAG, etag);
    }
    builder
        .body(Body::from(body))
        .map_err(|err| internal_error("Failed to build public model catalog response", err))
}

/// Fetch the upstream Codex `/v1/models` payload without discarding metadata.
async fn fetch_codex_models_payload(
    gateway: &LlmGatewayRuntimeState,
    auth_snapshot: &CodexAuthSnapshot,
    incoming_headers: &HeaderMap,
    query: &str,
) -> Result<(Value, Option<String>)> {
    let upstream_base = std::env::var("STATICFLOW_LLM_GATEWAY_UPSTREAM_BASE_URL")
        .ok()
        .map(|value| normalize_upstream_base_url(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_UPSTREAM_BASE_URL.to_string());
    let default_client_version =
        resolve_codex_client_version(Some(&gateway.runtime_config.read().codex_client_version));
    let client_version = extract_query_param(query, "client_version")
        .and_then(|value| super::normalize_codex_client_version(&value))
        .unwrap_or_else(|| default_client_version.clone());
    let url = append_client_version_query(
        &compute_upstream_url(&upstream_base, "/v1/models"),
        &client_version,
    );
    let mut headers = ReqwestHeaderMap::new();
    let incoming_user_agent = extract_header_value(incoming_headers, header::USER_AGENT.as_str());
    let incoming_originator = extract_header_value(incoming_headers, "originator");
    headers.insert(header::AUTHORIZATION, bearer_header(&auth_snapshot.access_token)?);
    headers.insert(header::ACCEPT, ReqwestHeaderValue::from_static("application/json"));
    let effective_user_agent =
        incoming_user_agent.unwrap_or_else(|| codex_user_agent(&client_version));
    headers.insert(header::USER_AGENT, ReqwestHeaderValue::from_str(&effective_user_agent)?);
    headers.insert(
        reqwest::header::HeaderName::from_static("originator"),
        ReqwestHeaderValue::from_str(
            incoming_originator
                .as_deref()
                .unwrap_or(DEFAULT_WIRE_ORIGINATOR),
        )?,
    );
    if let Some(account_id) = auth_snapshot.account_id.as_deref() {
        headers.insert(
            reqwest::header::HeaderName::from_static("chatgpt-account-id"),
            ReqwestHeaderValue::from_str(account_id)?,
        );
    }
    if auth_snapshot.is_fedramp_account {
        headers.insert(
            reqwest::header::HeaderName::from_static("x-openai-fedramp"),
            ReqwestHeaderValue::from_static("true"),
        );
    }
    let (client, resolved_proxy) = gateway.build_upstream_client(auth_snapshot).await?;
    let response = client.get(&url).headers(headers).send().await;
    let response = match response {
        Ok(response) => response,
        Err(err) => {
            let invalidated = gateway
                .upstream_proxy_registry
                .invalidate_client_if_connect_error(
                    &resolved_proxy,
                    codex_upstream_client_profile(),
                    &err,
                )
                .await;
            tracing::warn!(
                proxy_source = %resolved_proxy.source.as_str(),
                proxy_url = %resolved_proxy.proxy_url_label(),
                invalidated_client = invalidated,
                "codex models upstream request failed: {err}"
            );
            return Err(err).context("codex models upstream request failed");
        },
    };
    match parse_models_payload(response).await {
        Ok(parsed) => Ok(parsed),
        Err(err) => {
            tracing::error!(
                upstream_url = %url,
                error = %err,
                "codex public models request failed while parsing upstream response"
            );
            Err(err)
        },
    }
}

/// Parse the upstream models response JSON and return the raw payload plus
/// ETag.
async fn parse_models_payload(response: reqwest::Response) -> Result<(Value, Option<String>)> {
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
    let body = response
        .bytes()
        .await
        .context("failed to read models response body")?;
    if !status.is_success() {
        return Err(anyhow!(
            "status={} body={}",
            status,
            summarize_models_body_hint(body.as_ref())
        ));
    }
    if content_type.contains("text/html") {
        return Err(anyhow!(
            "upstream returned html body={}",
            summarize_models_body_hint(body.as_ref())
        ));
    }
    let value = serde_json::from_slice::<Value>(body.as_ref())
        .context("failed to parse upstream models payload as json")?;
    Ok((value, etag))
}

/// Build a short body hint for model-fetch error messages.
fn summarize_models_body_hint(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        "empty body".to_string()
    } else {
        trimmed.chars().take(200).collect()
    }
}
