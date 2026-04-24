use std::collections::HashMap;

use anyhow::{anyhow, Context, Result};
use axum::{
    body::Body,
    http::{header, HeaderMap, Response, StatusCode},
};
use reqwest::header::{HeaderMap as ReqwestHeaderMap, HeaderValue as ReqwestHeaderValue};
use serde_json::{json, Value};

use super::{
    codex_user_agent, compute_upstream_url,
    instructions::codex_default_instructions,
    internal_error,
    request::{extract_header_value, extract_query_param, normalize_upstream_base_url},
    resolve_codex_client_version,
    runtime::{
        bearer_header, codex_upstream_client_profile, CodexAuthSnapshot, LlmGatewayRuntimeState,
    },
    types::GatewayModelDescriptor,
    DEFAULT_UPSTREAM_BASE_URL, DEFAULT_WIRE_ORIGINATOR, GPT53_CODEX_MODEL_ID,
    GPT53_CODEX_SPARK_MODEL_ID,
};
use crate::state::AppState;

const GATEWAY_MODELS_OWNER: &str = "static-flow";

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

fn gateway_models_owner() -> &'static str {
    GATEWAY_MODELS_OWNER
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

/// Extract model ids from either ChatGPT-style or OpenAI-style model payloads.
fn extract_gateway_model_descriptors(
    value: &Value,
    owned_by: &'static str,
) -> Vec<GatewayModelDescriptor> {
    let mut items = std::collections::BTreeSet::<GatewayModelDescriptor>::new();
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

fn map_catalog_slug(slug: &str, map_gpt53_codex_to_spark: bool) -> (&str, bool) {
    if map_gpt53_codex_to_spark && slug == GPT53_CODEX_SPARK_MODEL_ID {
        (GPT53_CODEX_MODEL_ID, true)
    } else {
        (slug, false)
    }
}

fn normalize_public_model_catalog_value(
    mut value: Value,
    map_gpt53_codex_to_spark: bool,
) -> Result<Value> {
    let models = value
        .get_mut("models")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| anyhow!("upstream models payload is missing a models array"))?;
    let mut seen = HashMap::<String, usize>::new();
    let mut chosen = Vec::<(bool, Value)>::new();

    for mut item in std::mem::take(models) {
        let raw_slug = item
            .get("slug")
            .or_else(|| item.get("id"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);
        let Some(raw_slug) = raw_slug else {
            continue;
        };
        let (final_slug, was_alias) = map_catalog_slug(&raw_slug, map_gpt53_codex_to_spark);
        if let Some(object) = item.as_object_mut() {
            object.insert("slug".to_string(), Value::String(final_slug.to_string()));
            object.insert(
                "base_instructions".to_string(),
                Value::String(codex_default_instructions().to_string()),
            );
            object.remove("model_messages");
            if was_alias
                && object.get("display_name").and_then(Value::as_str) == Some(raw_slug.as_str())
            {
                object.insert("display_name".to_string(), Value::String(final_slug.to_string()));
            }
        }
        if let Some(index) = seen.get(final_slug).copied() {
            if chosen[index].0 && !was_alias {
                chosen[index] = (false, item);
            }
            continue;
        }
        seen.insert(final_slug.to_string(), chosen.len());
        chosen.push((was_alias, item));
    }

    if chosen.is_empty() {
        return Err(anyhow!("upstream model catalog contains no usable models"));
    }
    *models = chosen.into_iter().map(|(_, item)| item).collect();
    Ok(value)
}

#[cfg(test)]
fn parse_public_model_catalog_json(body: &[u8], map_gpt53_codex_to_spark: bool) -> Result<Value> {
    let value = serde_json::from_slice::<Value>(body).context("failed to parse catalog json")?;
    normalize_public_model_catalog_value(value, map_gpt53_codex_to_spark)
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
    format!("{url}{separator}client_version={}", urlencoding::encode(client_version))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        append_client_version_query, gateway_models_owner, parse_public_model_catalog_json,
    };
    use crate::llm_gateway::instructions::codex_default_instructions;

    #[test]
    fn codex_models_are_tagged_with_static_flow_owner() {
        assert_eq!(gateway_models_owner(), "static-flow");
    }

    #[test]
    fn append_client_version_query_url_encodes_value() {
        let url = append_client_version_query("https://example.com/v1/models", "0.124.0 beta");
        assert_eq!(url, "https://example.com/v1/models?client_version=0.124.0%20beta");
    }

    #[test]
    fn public_model_catalog_rewrites_alias_slug_and_dedupes() {
        let body = serde_json::to_vec(&json!({
            "models": [
                {
                    "slug": "gpt-5.3-codex-spark",
                    "display_name": "gpt-5.3-codex-spark",
                    "supported_in_api": false
                },
                {
                    "slug": "gpt-5.3-codex",
                    "display_name": "gpt-5.3-codex",
                    "supported_in_api": true
                },
                {
                    "slug": "gpt-5.5",
                    "display_name": "gpt-5.5",
                    "supported_in_api": true,
                    "base_instructions": "upstream instructions",
                    "model_messages": {
                        "instructions_template": "upstream template",
                        "instructions_variables": null
                    }
                }
            ]
        }))
        .expect("serialize sample models payload");

        let value =
            parse_public_model_catalog_json(&body, true).expect("catalog json should parse");
        let models = value["models"]
            .as_array()
            .expect("models should stay an array");

        assert_eq!(models.len(), 2);
        assert_eq!(models[0]["slug"], "gpt-5.3-codex");
        assert_eq!(models[0]["display_name"], "gpt-5.3-codex");
        assert_eq!(models[0]["supported_in_api"], true);
        assert_eq!(models[1]["slug"], "gpt-5.5");
        assert_eq!(models[1]["base_instructions"], json!(codex_default_instructions()));
        assert!(models[1].get("model_messages").is_none());
    }

    #[test]
    fn public_model_catalog_default_instructions_json_round_trips() {
        let body = serde_json::to_vec(&json!({
            "models": [
                {
                    "slug": "gpt-5.5",
                    "display_name": "gpt-5.5",
                    "supported_in_api": true
                }
            ]
        }))
        .expect("serialize sample models payload");

        let value =
            parse_public_model_catalog_json(&body, false).expect("catalog json should parse");
        let encoded = serde_json::to_vec(&value).expect("catalog json should encode");
        let raw_json = String::from_utf8(encoded.clone()).expect("catalog json is utf8");
        assert!(raw_json.contains("\\n# Personality\\n"));

        let decoded: serde_json::Value =
            serde_json::from_slice(&encoded).expect("encoded catalog should decode");
        assert_eq!(
            decoded["models"][0]["base_instructions"].as_str(),
            Some(codex_default_instructions())
        );
    }

    #[test]
    fn public_model_catalog_requires_models_array() {
        let body = serde_json::to_vec(&json!({
            "data": [
                { "id": "gpt-5.5" }
            ]
        }))
        .expect("serialize fallback list payload");

        let err = parse_public_model_catalog_json(&body, false)
            .expect_err("payload without models array should fail");

        assert!(err.to_string().contains("models array"));
    }
}
