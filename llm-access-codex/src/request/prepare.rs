//! Request body intake: read, size-guard, decode, and drive normalization.


use std::{
    collections::BTreeMap,
    io::{Cursor, Read},
};

#[cfg(test)]
use axum::body::{to_bytes, Body};
use axum::{
    body::Bytes,
    http::{header, Method},
};
use http::HeaderMap;
use serde_json::Value;

use super::{
    chat_completions::adapt_openai_chat_completions_request,
    extract_non_empty_string,
    headers::extract_header_value,
    last_message::extract_last_message_content_from_value,
    native_responses::{
        inject_default_instructions_when_missing, normalize_native_responses_request,
        repair_native_responses_request, validate_native_responses_request,
    },
    normalization::{
        filter_responses_request_fields, normalize_codex_public_model, normalize_responses_request,
        rewrite_responses_path, validate_responses_request,
    },
    path::{is_models_path, is_supported_codex_post_path},
    policy::resolve_billable_multiplier,
};
use crate::{
    anthropic_messages::adapt_anthropic_messages_request,
    conversation_normalizer::repair_responses_request,
    error::{
        bad_request, bad_request_with_detail, internal_error, method_not_allowed,
        CodexGatewayResult,
    },
    types::{GatewayResponseAdapter, PreparedGatewayRequest},
};
/// Normalize an incoming OpenAI-compatible request into the upstream Codex
/// shape.
///
/// `max_request_body_bytes` caps the body read to prevent oversized payloads
/// from exhausting backend memory.
#[cfg(test)]
pub async fn prepare_gateway_request(
    gateway_path: &str,
    query: &str,
    method: Method,
    headers: &HeaderMap,
    body: Body,
    max_request_body_bytes: usize,
) -> CodexGatewayResult<PreparedGatewayRequest> {
    let body = read_gateway_request_body(body, max_request_body_bytes).await?;
    prepare_gateway_request_from_bytes(
        gateway_path,
        query,
        method,
        headers,
        body,
        max_request_body_bytes,
    )
}
/// Read an Axum request body with the configured gateway byte limit.
#[cfg(test)]
pub async fn read_gateway_request_body(
    body: Body,
    max_request_body_bytes: usize,
) -> CodexGatewayResult<Bytes> {
    to_bytes(body, max_request_body_bytes)
        .await
        .map_err(|err| internal_error("Failed to read llm gateway request body", err))
}
/// Normalize an already-buffered OpenAI-compatible request body.
pub fn prepare_gateway_request_from_bytes(
    gateway_path: &str,
    query: &str,
    method: Method,
    headers: &HeaderMap,
    body: Bytes,
    max_request_body_bytes: usize,
) -> CodexGatewayResult<PreparedGatewayRequest> {
    let allows_get = is_models_path(gateway_path);
    let allows_post = is_supported_codex_post_path(gateway_path);
    let method_allowed =
        (method == Method::GET && allows_get) || (method == Method::POST && allows_post);
    if !method_allowed {
        return Err(method_not_allowed(
            "Unsupported method for the requested llm gateway endpoint",
        ));
    }
    let body = decode_gateway_request_body(headers, body, max_request_body_bytes)?;
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("application/json")
        .to_string();
    let is_json_content = content_type
        .to_ascii_lowercase()
        .starts_with("application/json");
    let mut json_value = if is_json_content && !body.is_empty() {
        serde_json::from_slice::<Value>(&body)
            .map(Some)
            .map_err(|err| bad_request_with_detail("Invalid JSON body", err))?
    } else {
        None
    };
    let model = json_value
        .as_ref()
        .and_then(|value| value.get("model"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    let last_message_content = json_value
        .as_ref()
        .and_then(extract_last_message_content_from_value);
    let billable_multiplier =
        if matches!(gateway_path, "/v1/chat/completions" | "/v1/responses" | "/v1/messages") {
            resolve_billable_multiplier(json_value.as_ref())
        } else {
            1
        };
    let original_wants_stream = json_value
        .as_ref()
        .and_then(|value| value.get("stream"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let original_path = format!("{gateway_path}{query}");
    let mut force_upstream_stream = false;
    let mut response_adapter = GatewayResponseAdapter::Responses;
    let mut upstream_path = original_path.clone();
    let mut thread_anchor = extract_header_value(headers, "conversation_id");
    let mut tool_name_restore_map = BTreeMap::new();

    if gateway_path == "/v1/chat/completions" {
        let Some(Value::Object(root)) = json_value.as_mut() else {
            return Err(bad_request("chat.completions requires a JSON object body"));
        };
        let (mut adapted, restore_map) = adapt_openai_chat_completions_request(root)?;
        tool_name_restore_map = restore_map;
        response_adapter = GatewayResponseAdapter::ChatCompletions;
        upstream_path = rewrite_responses_path(gateway_path, query);
        if let Some(prompt_cache_key) = extract_non_empty_string(adapted.get("prompt_cache_key")) {
            thread_anchor = Some(prompt_cache_key.to_string());
        }
        normalize_responses_request("/v1/responses", &mut adapted, thread_anchor.as_deref());
        repair_responses_request(&mut adapted)?;
        filter_responses_request_fields("/v1/responses", &mut adapted);
        validate_responses_request("/v1/responses", &adapted)?;
        if !original_wants_stream {
            adapted.insert("stream".to_string(), Value::Bool(true));
            force_upstream_stream = true;
        }
        json_value = Some(Value::Object(adapted));
    } else if gateway_path == "/v1/messages" {
        let Some(Value::Object(root)) = json_value.as_mut() else {
            return Err(bad_request("messages requires a JSON object body"));
        };
        let (mut adapted, restore_map) = adapt_anthropic_messages_request(root)?;
        tool_name_restore_map = restore_map;
        response_adapter = GatewayResponseAdapter::AnthropicMessages;
        upstream_path = "/v1/responses".to_string();
        if let Some(prompt_cache_key) = extract_non_empty_string(adapted.get("prompt_cache_key")) {
            thread_anchor = Some(prompt_cache_key.to_string());
        }
        normalize_responses_request("/v1/responses", &mut adapted, thread_anchor.as_deref());
        repair_responses_request(&mut adapted)?;
        filter_responses_request_fields("/v1/responses", &mut adapted);
        validate_responses_request("/v1/responses", &adapted)?;
        if !original_wants_stream {
            adapted.insert("stream".to_string(), Value::Bool(true));
            force_upstream_stream = true;
        }
        json_value = Some(Value::Object(adapted));
    } else if gateway_path.starts_with("/v1/responses") {
        response_adapter = GatewayResponseAdapter::Responses;
        if let Some(Value::Object(root)) = json_value.as_mut() {
            if let Some(prompt_cache_key) = extract_non_empty_string(root.get("prompt_cache_key")) {
                thread_anchor = Some(prompt_cache_key.to_string());
            }
            inject_default_instructions_when_missing(root);
            normalize_native_responses_request(gateway_path, root);
            repair_native_responses_request(gateway_path, root)?;
            validate_native_responses_request(gateway_path, root)?;
            if gateway_path == "/v1/responses" && !original_wants_stream {
                root.insert("stream".to_string(), Value::Bool(true));
                force_upstream_stream = true;
            }
        }
    }

    let request_body = match json_value {
        Some(value) => Bytes::from(
            serde_json::to_vec(&value)
                .map_err(|err| internal_error("Failed to encode gateway request body", err))?,
        ),
        None => body,
    };

    let (request_body, model) = normalize_codex_public_model(request_body, model)?;

    Ok(PreparedGatewayRequest {
        original_path,
        upstream_path,
        method,
        client_request_body: None,
        request_body,
        model,
        client_visible_model: None,
        wants_stream: original_wants_stream,
        force_upstream_stream,
        content_type,
        response_adapter,
        thread_anchor,
        tool_name_restore_map,
        billable_multiplier,
        last_message_content,
    })
}
fn decode_gateway_request_body(
    headers: &HeaderMap,
    body: Bytes,
    max_request_body_bytes: usize,
) -> CodexGatewayResult<Bytes> {
    let Some(content_encoding) = headers
        .get(header::CONTENT_ENCODING)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(body);
    };

    if content_encoding.eq_ignore_ascii_case("identity") {
        return Ok(body);
    }
    if !content_encoding.eq_ignore_ascii_case("zstd") {
        return Err(bad_request("Unsupported request content-encoding"));
    }

    let mut decoder = zstd::stream::Decoder::new(Cursor::new(body.as_ref()))
        .map_err(|err| bad_request_with_detail("Invalid zstd request body", err))?;
    let limit = u64::try_from(max_request_body_bytes)
        .unwrap_or(u64::MAX - 1)
        .saturating_add(1);
    let mut decoded = Vec::new();
    decoder
        .by_ref()
        .take(limit)
        .read_to_end(&mut decoded)
        .map_err(|err| bad_request_with_detail("Invalid zstd request body", err))?;
    if decoded.len() > max_request_body_bytes {
        return Err(bad_request("Decoded request body is too large"));
    }
    Ok(Bytes::from(decoded))
}
