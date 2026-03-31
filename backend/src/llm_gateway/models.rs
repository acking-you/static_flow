use anyhow::{anyhow, Context, Result};
use axum::{
    body::Body,
    http::{header, HeaderMap, Response, StatusCode},
};
use reqwest::header::{HeaderMap as ReqwestHeaderMap, HeaderValue as ReqwestHeaderValue};
use serde_json::{json, Value};

use super::{
    codex_user_agent, compute_upstream_url, internal_error,
    request::{extract_header_value, extract_query_param, normalize_upstream_base_url},
    runtime::{
        bearer_header, codex_upstream_client_profile, CodexAuthSnapshot, LlmGatewayRuntimeState,
    },
    types::GatewayModelDescriptor,
    DEFAULT_CODEX_CLI_VERSION, DEFAULT_UPSTREAM_BASE_URL, DEFAULT_WIRE_ORIGINATOR,
    GPT53_CODEX_MODEL_ID, GPT53_CODEX_SPARK_MODEL_ID,
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
    let (merged, etag) =
        fetch_codex_models(&state.llm_gateway, auth_snapshot, incoming_headers, query)
            .await
            .map_err(|err| internal_error("Failed to fetch llm gateway models", err))?;
    let merged = apply_model_aliases(merged, map_gpt53_codex_to_spark);

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

/// Fetch model descriptors from the upstream Codex backend.
async fn fetch_codex_models(
    gateway: &LlmGatewayRuntimeState,
    auth_snapshot: &CodexAuthSnapshot,
    incoming_headers: &HeaderMap,
    query: &str,
) -> Result<(Vec<GatewayModelDescriptor>, Option<String>)> {
    let upstream_base = std::env::var("STATICFLOW_LLM_GATEWAY_UPSTREAM_BASE_URL")
        .ok()
        .map(|value| normalize_upstream_base_url(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_UPSTREAM_BASE_URL.to_string());
    let client_version = extract_query_param(query, "client_version")
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_CODEX_CLI_VERSION.to_string());
    let url = append_client_version_query(
        &compute_upstream_url(&upstream_base, "/v1/models"),
        &client_version,
    );
    let mut headers = ReqwestHeaderMap::new();
    let incoming_user_agent = extract_header_value(incoming_headers, header::USER_AGENT.as_str());
    let incoming_originator = extract_header_value(incoming_headers, "originator");
    headers.insert(header::AUTHORIZATION, bearer_header(&auth_snapshot.access_token)?);
    headers.insert(header::ACCEPT, ReqwestHeaderValue::from_static("application/json"));
    let effective_user_agent = incoming_user_agent.unwrap_or_else(codex_user_agent);
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
    let (client, resolved_proxy) = gateway.build_upstream_client(auth_snapshot).await?;
    let response = client.get(url).headers(headers).send().await;
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
    parse_models_response(response, "codexmanager").await
}

/// Parse the upstream models response and return normalized descriptors plus
/// ETag.
async fn parse_models_response(
    response: reqwest::Response,
    owned_by: &'static str,
) -> Result<(Vec<GatewayModelDescriptor>, Option<String>)> {
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
    Ok((parse_gateway_model_descriptors(body.as_ref(), owned_by), etag))
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

/// Extract model ids from either ChatGPT-style or OpenAI-style model payloads.
fn parse_gateway_model_descriptors(
    body: &[u8],
    owned_by: &'static str,
) -> Vec<GatewayModelDescriptor> {
    let mut items = std::collections::BTreeSet::<GatewayModelDescriptor>::new();
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return Vec::new();
    };

    if let Some(models) = value.get("models").and_then(Value::as_array) {
        for item in models {
            let id = item
                .get("slug")
                .or_else(|| item.get("id"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            if let Some(id) = id {
                items.insert(GatewayModelDescriptor {
                    id: id.to_string(),
                    owned_by,
                });
            }
        }
    }
    if let Some(data) = value.get("data").and_then(Value::as_array) {
        for item in data {
            let id = item
                .get("id")
                .or_else(|| item.get("slug"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            if let Some(id) = id {
                items.insert(GatewayModelDescriptor {
                    id: id.to_string(),
                    owned_by,
                });
            }
        }
    }
    items.into_iter().collect()
}

fn apply_model_aliases(
    models: Vec<GatewayModelDescriptor>,
    map_gpt53_codex_to_spark: bool,
) -> Vec<GatewayModelDescriptor> {
    if !map_gpt53_codex_to_spark {
        return models;
    }

    models
        .into_iter()
        .map(|mut item| {
            if item.id == GPT53_CODEX_SPARK_MODEL_ID {
                item.id = GPT53_CODEX_MODEL_ID.to_string();
            }
            item
        })
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Ensure model requests carry a Codex client_version query parameter.
pub(crate) fn append_client_version_query(url: &str, client_version: &str) -> String {
    if url.contains("client_version=") {
        return url.to_string();
    }
    let separator = if url.contains('?') { '&' } else { '?' };
    format!("{url}{separator}client_version={client_version}")
}
