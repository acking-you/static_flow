//! Codex/OpenAI-compatible request normalization.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Cursor, Read},
    net::IpAddr,
};

use axum::body::{to_bytes, Body, Bytes};
use http::{header, HeaderMap, Method};
use serde_json::{json, Map, Value};

use crate::{
    anthropic_messages::adapt_anthropic_messages_request,
    conversation_normalizer::repair_responses_request,
    error::{
        bad_request, bad_request_with_detail, internal_error, method_not_allowed,
        CodexGatewayError, CodexGatewayResult,
    },
    instructions::codex_default_instructions,
    types::{GatewayResponseAdapter, OpenAiChatAdaptedRequest, PreparedGatewayRequest},
    FAST_BILLABLE_MULTIPLIER, GPT53_CODEX_MODEL_ID, GPT53_CODEX_SPARK_MODEL_ID,
    MAX_OPENAI_TOOL_NAME_LEN,
};

const LLM_GATEWAY_KEY_STATUS_ACTIVE: &str = "active";
const LLM_GATEWAY_KEY_STATUS_DISABLED: &str = "disabled";
const DEFAULT_PUBLIC_GPT_MODEL_ID: &str = "gpt-5.5";
const NATIVE_RESPONSES_UPSTREAM_UNSUPPORTED_FIELDS: &[&str] = &[
    "temperature",
    "top_p",
    "frequency_penalty",
    "presence_penalty",
    "user",
    "metadata",
    "prompt_cache_retention",
    "safety_identifier",
    "stream_options",
];
const NATIVE_RESPONSES_MESSAGE_ROLES: &[&str] = &["assistant", "system", "developer", "user"];

/// Normalize an incoming OpenAI-compatible request into the upstream Codex
/// shape.
///
/// `max_request_body_bytes` caps the body read to prevent oversized payloads
/// from exhausting backend memory.
#[cfg(test)]
pub(crate) async fn prepare_gateway_request(
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

/// Map the public `gpt-5.3-codex` id onto the current upstream Spark id.
pub fn apply_gpt53_codex_spark_mapping(
    prepared: &PreparedGatewayRequest,
    enabled: bool,
) -> CodexGatewayResult<PreparedGatewayRequest> {
    if !enabled || prepared.model.as_deref() != Some(GPT53_CODEX_MODEL_ID) {
        return Ok(prepared.clone());
    }

    let mut value = serde_json::from_slice::<Value>(&prepared.request_body)
        .map_err(|err| internal_error("Failed to parse mapped llm gateway request body", err))?;
    let Some(root) = value.as_object_mut() else {
        return Err(internal_error(
            "Failed to map llm gateway request model",
            "request body is not a JSON object",
        ));
    };
    root.insert("model".to_string(), Value::String(GPT53_CODEX_SPARK_MODEL_ID.to_string()));
    let request_body =
        Bytes::from(serde_json::to_vec(&value).map_err(|err| {
            internal_error("Failed to encode mapped llm gateway request body", err)
        })?);

    let mut mapped = prepared.clone();
    mapped.request_body = request_body;
    mapped.model = Some(GPT53_CODEX_SPARK_MODEL_ID.to_string());
    mapped.client_visible_model = Some(GPT53_CODEX_MODEL_ID.to_string());
    Ok(mapped)
}

/// Apply the per-key Codex fast policy to the upstream request body.
pub fn apply_codex_fast_policy(
    prepared: &PreparedGatewayRequest,
    enabled: bool,
) -> CodexGatewayResult<PreparedGatewayRequest> {
    let mut value = serde_json::from_slice::<Value>(&prepared.request_body)
        .map_err(|err| internal_error("Failed to parse llm gateway request body", err))?;
    let Some(root) = value.as_object_mut() else {
        return Err(internal_error(
            "Failed to apply llm gateway fast policy",
            "request body is not a JSON object",
        ));
    };

    let mut changed = false;
    if let Some(service_tier) = root.get_mut("service_tier") {
        let normalized_fast = service_tier
            .as_str()
            .is_some_and(|raw| raw.eq_ignore_ascii_case("fast"));
        let is_priority = service_tier
            .as_str()
            .is_some_and(|raw| raw.eq_ignore_ascii_case("priority"));
        if normalized_fast {
            *service_tier = Value::String("priority".to_string());
            changed = true;
        }
        if !enabled && (normalized_fast || is_priority) {
            root.remove("service_tier");
            changed = true;
        }
    }

    let billable_multiplier = resolve_billable_multiplier(Some(&value));
    if !changed && prepared.billable_multiplier == billable_multiplier {
        return Ok(prepared.clone());
    }

    let request_body = Bytes::from(serde_json::to_vec(&value).map_err(|err| {
        internal_error("Failed to encode llm gateway fast-policy request body", err)
    })?);

    let mut adjusted = prepared.clone();
    adjusted.request_body = request_body;
    adjusted.billable_multiplier = billable_multiplier;
    Ok(adjusted)
}

/// Align the outgoing native responses `store` field with the selected
/// upstream provider semantics.
pub fn align_responses_store_with_upstream(
    prepared: &PreparedGatewayRequest,
    upstream_base: &str,
) -> CodexGatewayResult<PreparedGatewayRequest> {
    if !prepared.upstream_path.starts_with("/v1/responses") {
        return Ok(prepared.clone());
    }

    let mut value = serde_json::from_slice::<Value>(&prepared.request_body).map_err(|err| {
        internal_error("Failed to parse llm gateway request body for store alignment", err)
    })?;
    let Some(root) = value.as_object_mut() else {
        return Err(internal_error(
            "Failed to align llm gateway request store field",
            "request body is not a JSON object",
        ));
    };

    let changed = if prepared.upstream_path.starts_with("/v1/responses/compact") {
        let removed_store = root.remove("store").is_some();
        let removed_previous_response_id = root.remove("previous_response_id").is_some();
        let removed_item_ids = strip_input_item_ids(root);
        removed_store || removed_previous_response_id || removed_item_ids
    } else {
        let is_azure = is_azure_responses_upstream_base(upstream_base);
        let store = is_azure;
        let mut changed = false;
        if root.get("store") != Some(&Value::Bool(store)) {
            root.insert("store".to_string(), Value::Bool(store));
            changed = true;
        }
        if !store && root.remove("previous_response_id").is_some() {
            changed = true;
        }
        if !(store && is_azure) && strip_input_item_ids(root) {
            changed = true;
        }
        changed
    };

    if !changed {
        return Ok(prepared.clone());
    }

    let request_body = Bytes::from(serde_json::to_vec(&value).map_err(|err| {
        internal_error("Failed to encode llm gateway request body after store alignment", err)
    })?);

    let mut aligned = prepared.clone();
    aligned.request_body = request_body;
    Ok(aligned)
}

/// Extract the last text-like message content from the request body.
///
/// This intentionally parses request-format structures (`messages` for chat
/// requests, `input` for responses requests) instead of storing the whole
/// body. Unsupported shapes return `Ok(None)` so the main request flow is not
/// blocked; malformed JSON returns `Err(...)` and the caller can record a
/// failure marker if desired.
/// Extract the last text-like message content from an OpenAI-compatible body.
pub fn extract_last_message_content(body: &Bytes) -> Result<Option<String>, String> {
    if body.is_empty() {
        return Ok(None);
    }
    let value: Value = serde_json::from_slice(body)
        .map_err(|err| format!("failed to parse request body: {err}"))?;
    if !value.is_object() {
        return Err("request body is not a JSON object".to_string());
    }
    Ok(extract_last_message_content_from_value(&value))
}

fn extract_last_message_content_from_value(value: &Value) -> Option<String> {
    let root = value.as_object()?;

    if let Some(messages) = root.get("messages").and_then(Value::as_array) {
        return extract_last_message_from_chat_messages(messages);
    }
    if let Some(input) = root.get("input") {
        return extract_last_text_from_responses_input(input);
    }
    None
}

/// Convert request-level service tier hints into a billing multiplier.
/// Resolve the billing multiplier implied by a request JSON object.
pub fn resolve_billable_multiplier(json_value: Option<&Value>) -> u64 {
    if request_uses_fast_service_tier(json_value) {
        FAST_BILLABLE_MULTIPLIER
    } else {
        1
    }
}

fn extract_last_message_from_chat_messages(messages: &[Value]) -> Option<String> {
    messages.iter().rev().find_map(|message| {
        message
            .as_object()
            .and_then(|obj| obj.get("content"))
            .and_then(extract_last_text_from_generic_content)
    })
}

fn extract_last_text_from_responses_input(input: &Value) -> Option<String> {
    match input {
        Value::String(text) => normalized_non_empty_text(text),
        Value::Array(items) => items
            .iter()
            .rev()
            .find_map(extract_last_text_from_responses_item),
        Value::Object(_) => extract_last_text_from_responses_item(input),
        _ => None,
    }
}

fn extract_last_text_from_responses_item(item: &Value) -> Option<String> {
    match item {
        Value::String(text) => normalized_non_empty_text(text),
        Value::Object(obj) => {
            let item_type = obj.get("type").and_then(Value::as_str).unwrap_or_default();
            match item_type {
                "message" => obj
                    .get("content")
                    .and_then(extract_last_text_from_generic_content),
                "function_call_output" | "custom_tool_call_output" => obj
                    .get("output")
                    .and_then(extract_last_text_from_generic_content),
                "text" | "input_text" | "output_text" => obj
                    .get("text")
                    .and_then(Value::as_str)
                    .and_then(normalized_non_empty_text),
                _ => obj
                    .get("content")
                    .and_then(extract_last_text_from_generic_content)
                    .or_else(|| {
                        obj.get("output")
                            .and_then(extract_last_text_from_generic_content)
                    })
                    .or_else(|| {
                        obj.get("text")
                            .and_then(Value::as_str)
                            .and_then(normalized_non_empty_text)
                    }),
            }
        },
        Value::Array(items) => items
            .iter()
            .rev()
            .find_map(extract_last_text_from_generic_content),
        _ => None,
    }
}

fn extract_last_text_from_generic_content(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => normalized_non_empty_text(text),
        Value::Array(items) => items
            .iter()
            .rev()
            .find_map(extract_last_text_from_generic_content),
        Value::Object(map) => map
            .get("content")
            .and_then(extract_last_text_from_generic_content)
            .or_else(|| {
                map.get("output")
                    .and_then(extract_last_text_from_generic_content)
            })
            .or_else(|| {
                map.get("text")
                    .and_then(Value::as_str)
                    .and_then(normalized_non_empty_text)
            }),
        _ => None,
    }
}

fn normalized_non_empty_text(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Detect whether the request explicitly opted into the fast/priority tier.
fn request_uses_fast_service_tier(json_value: Option<&Value>) -> bool {
    json_value
        .and_then(Value::as_object)
        .and_then(|root| root.get("service_tier"))
        .and_then(Value::as_str)
        .is_some_and(|tier| {
            tier.eq_ignore_ascii_case("fast") || tier.eq_ignore_ascii_case("priority")
        })
}

/// Read one trimmed header value as UTF-8 text.
/// Read one trimmed header value as UTF-8 text.
pub fn extract_header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

/// Serialize request headers into a stable JSON object for admin diagnostics.
///
/// These values are intentionally captured from the original inbound request so
/// operators can inspect what the reverse proxy and client actually sent. The
/// serialized JSON is stored in the usage ledger only; it is **not** reused as
/// an upstream header set when the gateway later calls the Codex backend.
pub fn serialize_headers_json(headers: &HeaderMap) -> String {
    let mut map = BTreeMap::<String, Vec<String>>::new();
    for name in headers.keys() {
        let key = name.as_str().to_string();
        let values = headers
            .get_all(name)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        if !values.is_empty() {
            map.insert(key, values);
        }
    }
    serde_json::to_string(&map).unwrap_or_else(|err| {
        tracing::warn!("Failed to serialize LLM gateway request headers to JSON: {err}");
        "{}".to_string()
    })
}

/// Builds the operator-facing absolute URL when proxy headers are available.
///
/// Reverse-proxy headers such as `x-forwarded-host` and
/// `x-forwarded-proto` are consumed here only to reconstruct the public URL
/// that the caller hit. They are not forwarded to the upstream Codex API.
pub fn resolve_request_url_from_headers(headers: &HeaderMap, uri: &http::Uri) -> String {
    let scheme = extract_header_value(headers, "x-forwarded-proto")
        .or_else(|| extract_header_value(headers, "x-scheme"))
        .unwrap_or_else(|| "http".to_string());
    let host = extract_header_value(headers, "x-forwarded-host")
        .or_else(|| extract_header_value(headers, header::HOST.as_str()));
    let path_and_query = uri
        .path_and_query()
        .map(|value| value.as_str().to_string())
        .unwrap_or_else(|| uri.path().to_string());
    match host {
        Some(host) => format!("{scheme}://{host}{path_and_query}"),
        None => path_and_query,
    }
}

/// Extracts the first trustworthy client IP from the reverse-proxy header
/// chain.
///
/// The gateway uses these proxy headers strictly for local diagnostics,
/// behavior analysis, and admin troubleshooting. Upstream Codex requests are
/// rebuilt from a narrow allowlist and therefore do not inherit
/// `x-forwarded-for`, `x-real-ip`, `forwarded`, or similar network-path
/// headers.
pub fn extract_client_ip_from_headers(headers: &HeaderMap) -> String {
    parse_first_ip_from_header(headers.get("x-forwarded-for"))
        .or_else(|| parse_first_ip_from_header(headers.get("x-real-ip")))
        .or_else(|| parse_first_ip_from_header(headers.get("cf-connecting-ip")))
        .or_else(|| parse_first_ip_from_header(headers.get("x-client-ip")))
        .or_else(|| parse_ip_from_forwarded_header(headers.get("forwarded")))
        .unwrap_or_else(|| "unknown".to_string())
}

/// Parse the first IP candidate from a comma-delimited proxy header.
fn parse_first_ip_from_header(value: Option<&http::HeaderValue>) -> Option<String> {
    let raw = value?.to_str().ok()?;
    raw.split(',').find_map(normalize_ip_token)
}

/// Parse the RFC 7239 `Forwarded` header and extract the first usable IP.
fn parse_ip_from_forwarded_header(value: Option<&http::HeaderValue>) -> Option<String> {
    let raw = value?.to_str().ok()?;
    raw.split(',').find_map(|entry| {
        entry.split(';').find_map(|segment| {
            let token = segment.trim();
            if token
                .get(..4)
                .map(|prefix| prefix.eq_ignore_ascii_case("for="))
                .unwrap_or(false)
            {
                normalize_ip_token(token)
            } else {
                None
            }
        })
    })
}

/// Normalize raw proxy IP tokens across IPv4, IPv6, and host:port forms.
fn normalize_ip_token(token: &str) -> Option<String> {
    let mut value = token.trim().trim_matches('"');
    if value.is_empty() || value.eq_ignore_ascii_case("unknown") {
        return None;
    }

    if value
        .get(..4)
        .map(|prefix| prefix.eq_ignore_ascii_case("for="))
        .unwrap_or(false)
    {
        value = value[4..].trim().trim_matches('"');
    }

    if value.starts_with('[') {
        if let Some(end) = value.find(']') {
            let host = &value[1..end];
            let remain = value[end + 1..].trim();
            let valid_suffix = remain.is_empty()
                || (remain.starts_with(':') && remain[1..].chars().all(|ch| ch.is_ascii_digit()));
            if valid_suffix {
                if let Ok(ip) = host.parse::<IpAddr>() {
                    return Some(ip.to_string());
                }
            }
        }
    }

    if let Ok(ip) = value.parse::<IpAddr>() {
        return Some(ip.to_string());
    }

    if let Some((host, port)) = value.rsplit_once(':') {
        if host.contains('.') && !port.is_empty() && port.chars().all(|ch| ch.is_ascii_digit()) {
            if let Ok(ip) = host.parse::<IpAddr>() {
                return Some(ip.to_string());
            }
        }
    }

    None
}

/// Return a non-empty trimmed JSON string field.
/// Return a non-empty trimmed JSON string field.
pub fn extract_non_empty_string(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn coerce_non_empty_scalar_to_string(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => {
            let trimmed = text.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        },
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(boolean) => Some(boolean.to_string()),
        _ => None,
    }
}

fn inject_default_instructions_when_missing(root: &mut Map<String, Value>) {
    let needs_default_instructions = match root.get("instructions") {
        None | Some(Value::Null) => true,
        Some(Value::String(value)) => value.trim().is_empty(),
        Some(_) => false,
    };
    if needs_default_instructions {
        root.insert(
            "instructions".to_string(),
            Value::String(codex_default_instructions().to_string()),
        );
    }
}

fn normalize_native_responses_request(path: &str, root: &mut Map<String, Value>) {
    root.remove("max_output_tokens");
    if path == "/v1/responses" {
        remove_native_responses_upstream_unsupported_fields(root);
        normalize_native_responses_input_for_upstream(root);
    }
    if path == "/v1/responses/compact" {
        retain_native_compact_fields(root);
    }
}

fn remove_native_responses_upstream_unsupported_fields(root: &mut Map<String, Value>) {
    for field in NATIVE_RESPONSES_UPSTREAM_UNSUPPORTED_FIELDS {
        root.remove(*field);
    }
}

fn normalize_native_responses_input_for_upstream(root: &mut Map<String, Value>) {
    let Some(input) = root.get_mut("input") else {
        return;
    };
    match input {
        Value::String(text) => {
            let mut content = Map::new();
            content.insert("type".to_string(), Value::String("input_text".to_string()));
            content.insert("text".to_string(), Value::String(text.clone()));

            let mut message = Map::new();
            message.insert("type".to_string(), Value::String("message".to_string()));
            message.insert("role".to_string(), Value::String("user".to_string()));
            message.insert("content".to_string(), Value::Array(vec![Value::Object(content)]));
            *input = Value::Array(vec![Value::Object(message)]);
        },
        Value::Object(_) => {
            let item = std::mem::take(input);
            *input = Value::Array(vec![item]);
        },
        _ => {},
    }
}

fn repair_native_responses_request(
    path: &str,
    root: &mut Map<String, Value>,
) -> CodexGatewayResult<()> {
    if path != "/v1/responses" {
        return Ok(());
    }
    repair_native_responses_tool_role_messages(root)
}

fn repair_native_responses_tool_role_messages(
    root: &mut Map<String, Value>,
) -> CodexGatewayResult<()> {
    let Some(Value::Array(items)) = root.get_mut("input") else {
        return Ok(());
    };

    for item in items {
        let Some(item_obj) = item.as_object() else {
            continue;
        };
        if item_obj.get("role").and_then(Value::as_str) != Some("tool") {
            continue;
        }

        let call_id = extract_non_empty_string(
            item_obj
                .get("call_id")
                .or_else(|| item_obj.get("tool_call_id"))
                .or_else(|| item_obj.get("id")),
        )
        .map(ToString::to_string);

        let repaired = if let Some(call_id) = call_id {
            let output = convert_tool_message_content_to_responses_output(
                item_obj.get("content").or_else(|| item_obj.get("output")),
            )
            .map_err(|err| bad_request_with_detail("Invalid tool content", err))?;
            json!({
                "type": "function_call_output",
                "call_id": call_id,
                "output": output
            })
        } else {
            let mut content_items = item_obj
                .get("content")
                .or_else(|| item_obj.get("output"))
                .map(convert_user_message_content_to_responses_items)
                .unwrap_or_default();
            if content_items.is_empty() {
                content_items.push(json!({
                    "type": "input_text",
                    "text": "(empty)",
                }));
            }
            json!({
                "type": "message",
                "role": "user",
                "content": content_items
            })
        };
        *item = repaired;
    }

    Ok(())
}

fn validate_native_responses_request(
    path: &str,
    root: &Map<String, Value>,
) -> CodexGatewayResult<()> {
    if path != "/v1/responses" {
        return Ok(());
    }
    validate_native_responses_input_roles(root.get("input"))
}

fn validate_native_responses_input_roles(input: Option<&Value>) -> CodexGatewayResult<()> {
    let Some(Value::Array(items)) = input else {
        return Ok(());
    };

    for (index, item) in items.iter().enumerate() {
        let Some(item_obj) = item.as_object() else {
            continue;
        };
        let Some(role) = item_obj.get("role").and_then(Value::as_str) else {
            continue;
        };
        if NATIVE_RESPONSES_MESSAGE_ROLES.contains(&role) {
            continue;
        }
        if role == "tool" {
            let message = format!(
                "responses input item {index} uses Chat Completions role `tool`; send tool \
                 outputs as `function_call_output` items with `call_id` and `output`"
            );
            return Err(bad_request(&message));
        }
        let message = format!(
            "responses input item {index} has unsupported role `{role}`; supported roles are \
             `assistant`, `system`, `developer`, and `user`"
        );
        return Err(bad_request(&message));
    }

    Ok(())
}

fn strip_input_item_ids(root: &mut Map<String, Value>) -> bool {
    let Some(Value::Array(items)) = root.get_mut("input") else {
        return false;
    };
    let mut removed_any = false;
    for item in items {
        let Some(item_obj) = item.as_object_mut() else {
            continue;
        };
        if item_obj.remove("id").is_some() {
            removed_any = true;
        }
    }
    removed_any
}

fn retain_native_compact_fields(root: &mut Map<String, Value>) {
    root.retain(|key, _| {
        matches!(
            key.as_str(),
            "model"
                | "instructions"
                | "input"
                | "tools"
                | "parallel_tool_calls"
                | "reasoning"
                | "service_tier"
                | "prompt_cache_key"
                | "text"
        )
    });
}

/// Rewrite chat/completions requests onto the upstream responses endpoint.
fn rewrite_responses_path(path: &str, query: &str) -> String {
    let rewritten = if let Some(suffix) = path.strip_prefix("/v1/chat/completions") {
        format!("/v1/responses{suffix}")
    } else {
        path.to_string()
    };
    format!("{rewritten}{query}")
}

/// Normalize configured upstream base URLs into the Codex backend form.
pub fn normalize_upstream_base_url(base: &str) -> String {
    let mut normalized = base.trim().trim_end_matches('/').to_string();
    let lower = normalized.to_ascii_lowercase();
    if (lower.starts_with("https://chatgpt.com") || lower.starts_with("https://chat.openai.com"))
        && !lower.contains("/backend-api")
    {
        normalized = format!("{normalized}/backend-api/codex");
    }
    normalized
}

fn is_azure_responses_upstream_base(base_url: &str) -> bool {
    let base_url = base_url.to_ascii_lowercase();
    const AZURE_MARKERS: [&str; 6] = [
        "openai.azure.",
        "cognitiveservices.azure.",
        "aoai.azure.",
        "azure-api.",
        "azurefd.",
        "windows.net/openai",
    ];
    AZURE_MARKERS.iter().any(|marker| base_url.contains(marker))
}

/// Collapse user-provided reasoning-effort aliases into supported values.
fn normalize_reasoning_effort(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "low" => Some("low"),
        "medium" => Some("medium"),
        "high" => Some("high"),
        "max" => Some("xhigh"),
        "xhigh" | "extra_high" => Some("xhigh"),
        _ => None,
    }
}

fn normalize_codex_public_model(
    request_body: Bytes,
    model: Option<String>,
) -> CodexGatewayResult<(Bytes, Option<String>)> {
    let Some(model) = model else {
        return Ok((request_body, None));
    };
    if model.starts_with("gpt-") {
        return Ok((request_body, Some(model)));
    }

    let mut value = serde_json::from_slice::<Value>(&request_body).map_err(|err| {
        internal_error("Failed to parse llm gateway request body for model fallback", err)
    })?;
    let Some(root) = value.as_object_mut() else {
        return Err(internal_error(
            "Failed to normalize llm gateway request model",
            "request body is not a JSON object",
        ));
    };
    root.insert("model".to_string(), Value::String(DEFAULT_PUBLIC_GPT_MODEL_ID.to_string()));
    let request_body = Bytes::from(serde_json::to_vec(&value).map_err(|err| {
        internal_error("Failed to encode llm gateway request body after model fallback", err)
    })?);
    Ok((request_body, Some(DEFAULT_PUBLIC_GPT_MODEL_ID.to_string())))
}

fn filter_responses_request_fields(path: &str, root: &mut Map<String, Value>) {
    root.retain(|key, _| match path {
        "/v1/responses" => matches!(
            key.as_str(),
            "model"
                | "instructions"
                | "previous_response_id"
                | "input"
                | "tools"
                | "tool_choice"
                | "parallel_tool_calls"
                | "reasoning"
                | "store"
                | "stream"
                | "include"
                | "service_tier"
                | "prompt_cache_key"
                | "text"
                | "client_metadata"
        ),
        "/v1/responses/compact" => matches!(
            key.as_str(),
            "model"
                | "instructions"
                | "input"
                | "tools"
                | "parallel_tool_calls"
                | "reasoning"
                | "service_tier"
                | "prompt_cache_key"
                | "text"
        ),
        _ => true,
    });
}

fn validate_responses_request(path: &str, root: &Map<String, Value>) -> CodexGatewayResult<()> {
    if !path.starts_with("/v1/responses") {
        return Ok(());
    }

    validate_json_object_input_messages(root)?;
    if path == "/v1/responses" {
        return Ok(());
    }
    validate_tool_call_history(root)?;
    Ok(())
}

fn normalize_codex_input_message_roles(root: &mut Map<String, Value>) {
    let Some(input) = root.get_mut("input") else {
        return;
    };
    normalize_codex_input_message_roles_value(input);
}

fn normalize_codex_input_message_roles_value(value: &mut Value) {
    match value {
        Value::Array(items) => {
            for item in items {
                normalize_codex_input_message_roles_value(item);
            }
        },
        Value::Object(obj) => {
            if obj.get("type").and_then(Value::as_str) == Some("message")
                && obj.get("role").and_then(Value::as_str) == Some("system")
            {
                obj.insert("role".to_string(), Value::String("developer".to_string()));
            }
        },
        _ => {},
    }
}

fn validate_json_object_input_messages(root: &Map<String, Value>) -> CodexGatewayResult<()> {
    let format_type = root
        .get("text")
        .and_then(Value::as_object)
        .and_then(|text| text.get("format"))
        .and_then(Value::as_object)
        .and_then(|format| format.get("type"))
        .and_then(Value::as_str);
    if format_type != Some("json_object") {
        return Ok(());
    }

    if responses_input_messages_contain_json_keyword(root.get("input")) {
        return Ok(());
    }

    Err(bad_request(
        "responses text.format.type `json_object` requires at least one input message containing \
         `json`",
    ))
}

fn responses_input_messages_contain_json_keyword(input: Option<&Value>) -> bool {
    let Some(input) = input else {
        return false;
    };
    match input {
        Value::String(text) => text_contains_json_keyword(text),
        Value::Array(items) => items.iter().any(response_input_item_contains_json_keyword),
        Value::Object(_) => response_input_item_contains_json_keyword(input),
        _ => false,
    }
}

fn response_input_item_contains_json_keyword(item: &Value) -> bool {
    let Some(obj) = item.as_object() else {
        return false;
    };
    if obj.get("type").and_then(Value::as_str) != Some("message") {
        return false;
    }
    obj.get("content")
        .is_some_and(response_content_contains_json_keyword)
}

fn response_content_contains_json_keyword(value: &Value) -> bool {
    match value {
        Value::String(text) => text_contains_json_keyword(text),
        Value::Array(items) => items.iter().any(response_content_contains_json_keyword),
        Value::Object(obj) => {
            obj.get("content")
                .is_some_and(response_content_contains_json_keyword)
                || obj
                    .get("text")
                    .and_then(Value::as_str)
                    .is_some_and(text_contains_json_keyword)
        },
        _ => false,
    }
}

fn text_contains_json_keyword(text: &str) -> bool {
    text.to_ascii_lowercase().contains("json")
}

fn validate_tool_call_history(root: &Map<String, Value>) -> CodexGatewayResult<()> {
    let Some(input) = root.get("input") else {
        return Ok(());
    };
    let items = match input {
        Value::Array(items) => items.iter().collect::<Vec<_>>(),
        Value::Object(_) => vec![input],
        _ => return Ok(()),
    };
    let mut seen_calls = BTreeSet::new();
    let mut pending_calls = BTreeSet::new();
    let mut seen_outputs = BTreeSet::new();

    for (index, item) in items.iter().enumerate() {
        let Some(obj) = item.as_object() else {
            continue;
        };
        let item_type = obj.get("type").and_then(Value::as_str).unwrap_or_default();
        match item_type {
            "function_call" | "custom_tool_call" => {
                let call_id =
                    extract_non_empty_string(obj.get("call_id").or_else(|| obj.get("id")))
                        .ok_or_else(|| {
                            bad_request(&format!(
                                "responses input item {index} ({item_type}) is missing call_id"
                            ))
                        })?;
                if !seen_calls.insert(call_id.to_string()) {
                    return Err(bad_request(&format!(
                        "responses input contains duplicate function call `{call_id}`"
                    )));
                }
                pending_calls.insert(call_id.to_string());
            },
            "function_call_output" | "custom_tool_call_output" => {
                let call_id = extract_non_empty_string(obj.get("call_id")).ok_or_else(|| {
                    bad_request(&format!(
                        "responses input item {index} ({item_type}) is missing call_id"
                    ))
                })?;
                if !seen_outputs.insert(call_id.to_string()) {
                    return Err(bad_request(&format!(
                        "responses input contains duplicate tool output for function call \
                         `{call_id}`"
                    )));
                }
                if pending_calls.remove(call_id) {
                    continue;
                }
                return Err(bad_request(&format!(
                    "responses input contains tool output for unknown function call `{call_id}`"
                )));
            },
            _ => {},
        }
    }

    if let Some(call_id) = pending_calls.into_iter().next() {
        return Err(bad_request(&format!("No tool output found for function call {call_id}")));
    }

    Ok(())
}

pub(crate) fn normalize_tool_parameters_schema(value: Value) -> Value {
    match value {
        Value::Object(mut obj) => {
            if let Some(array_ref) = obj
                .get("$ref")
                .and_then(Value::as_str)
                .map(str::trim)
                .and_then(|reference| reference.strip_suffix("[]"))
                .filter(|reference| !reference.is_empty())
            {
                let mut rewritten = Map::new();
                rewritten.insert("type".to_string(), Value::String("array".to_string()));
                rewritten.insert(
                    "items".to_string(),
                    normalize_tool_parameters_schema(json!({ "$ref": array_ref })),
                );
                for (key, child) in obj {
                    if key == "$ref" {
                        continue;
                    }
                    rewritten.insert(key, normalize_tool_parameters_schema(child));
                }
                return Value::Object(rewritten);
            }

            if obj
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|schema_type| schema_type == "object")
            {
                match obj.get_mut("properties") {
                    Some(Value::Object(properties)) => {
                        for child in properties.values_mut() {
                            *child = normalize_tool_parameters_schema(child.take());
                        }
                    },
                    Some(other) => {
                        *other = Value::Object(Map::new());
                    },
                    None => {
                        obj.insert("properties".to_string(), Value::Object(Map::new()));
                    },
                }
            }

            for key in ["items", "additionalProperties", "not"] {
                if let Some(child) = obj.get_mut(key) {
                    *child = normalize_tool_parameters_schema(child.take());
                }
            }
            for key in ["allOf", "anyOf", "oneOf", "prefixItems"] {
                if let Some(Value::Array(items)) = obj.get_mut(key) {
                    for item in items {
                        *item = normalize_tool_parameters_schema(item.take());
                    }
                }
            }
            for key in ["$defs", "definitions"] {
                if let Some(Value::Object(defs)) = obj.get_mut(key) {
                    for schema in defs.values_mut() {
                        *schema = normalize_tool_parameters_schema(schema.take());
                    }
                }
            }
            Value::Object(obj)
        },
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(normalize_tool_parameters_schema)
                .collect(),
        ),
        other => other,
    }
}

/// Map OpenAI chat roles into the role set accepted by the responses API.
fn normalize_openai_role_for_responses(role: &str) -> Option<&'static str> {
    match role {
        "system" | "developer" => Some("developer"),
        "user" => Some("user"),
        "assistant" => Some("assistant"),
        "tool" => Some("tool"),
        _ => None,
    }
}

/// Shorten tool names so they fit the upstream name length budget.
fn shorten_openai_tool_name_candidate(name: &str) -> String {
    if name.len() <= MAX_OPENAI_TOOL_NAME_LEN {
        return name.to_string();
    }
    if name.starts_with("mcp__") {
        if let Some(idx) = name.rfind("__") {
            if idx > 0 {
                let mut candidate = format!("mcp__{}", &name[idx + 2..]);
                if candidate.len() > MAX_OPENAI_TOOL_NAME_LEN {
                    candidate.truncate(MAX_OPENAI_TOOL_NAME_LEN);
                }
                return candidate;
            }
        }
    }
    name.chars().take(MAX_OPENAI_TOOL_NAME_LEN).collect()
}

/// Apply the stable shortening map for one OpenAI tool/function name.
fn shorten_openai_tool_name_with_map(
    name: &str,
    tool_name_map: &BTreeMap<String, String>,
) -> String {
    tool_name_map
        .get(name)
        .cloned()
        .unwrap_or_else(|| shorten_openai_tool_name_candidate(name))
}

/// Restore shortened tool names when adapting responses back to chat format.
pub fn restore_openai_tool_name(
    name: &str,
    tool_name_restore_map: Option<&BTreeMap<String, String>>,
) -> String {
    tool_name_restore_map
        .and_then(|map| map.get(name))
        .cloned()
        .unwrap_or_else(|| name.to_string())
}

/// Return the dynamic tools array regardless of the chosen field casing.
fn get_dynamic_tools_array(obj: &Map<String, Value>) -> Option<&Vec<Value>> {
    obj.get("dynamic_tools")
        .or_else(|| obj.get("dynamicTools"))
        .and_then(Value::as_array)
}

fn is_openai_chat_function_tool(tool_obj: &Map<String, Value>) -> bool {
    let tool_type = tool_obj
        .get("type")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    tool_type == "function"
        || (tool_type.is_empty()
            && (tool_obj.get("function").is_some() || tool_obj.get("name").is_some()))
}

fn openai_chat_tool_field<'a>(
    tool_obj: &'a Map<String, Value>,
    function: Option<&'a Map<String, Value>>,
    key: &str,
) -> Option<&'a Value> {
    tool_obj
        .get(key)
        .or_else(|| function.and_then(|function| function.get(key)))
}

fn openai_chat_tool_name_value<'a>(
    tool_obj: &'a Map<String, Value>,
    function: Option<&'a Map<String, Value>>,
) -> Option<&'a Value> {
    openai_chat_tool_field(tool_obj, function, "name")
}

fn legacy_openai_function_name_value(function_obj: &Map<String, Value>) -> Option<&Value> {
    function_obj.get("name")
}

fn map_openai_chat_function_tool(
    tool_obj: &Map<String, Value>,
    tool_name_map: &BTreeMap<String, String>,
) -> Option<Value> {
    let function = tool_obj.get("function").and_then(Value::as_object);
    let name = coerce_non_empty_scalar_to_string(openai_chat_tool_name_value(tool_obj, function))
        .map(|name| shorten_openai_tool_name_with_map(&name, tool_name_map))?;
    let mut mapped = Map::new();
    mapped.insert("type".to_string(), Value::String("function".to_string()));
    mapped.insert("name".to_string(), Value::String(name));
    if let Some(description) = openai_chat_tool_field(tool_obj, function, "description") {
        mapped.insert("description".to_string(), description.clone());
    }
    if let Some(parameters) = openai_chat_tool_field(tool_obj, function, "parameters") {
        mapped
            .insert("parameters".to_string(), normalize_tool_parameters_schema(parameters.clone()));
    }
    if let Some(strict) = openai_chat_tool_field(tool_obj, function, "strict") {
        mapped.insert("strict".to_string(), strict.clone());
    }
    Some(Value::Object(mapped))
}

/// Collect every function/tool name referenced anywhere in the request.
fn collect_openai_tool_names(obj: &Map<String, Value>) -> Vec<String> {
    let mut names = Vec::new();

    if let Some(tools) = obj.get("tools").and_then(Value::as_array) {
        for tool in tools {
            let Some(tool_obj) = tool.as_object() else {
                continue;
            };
            if !is_openai_chat_function_tool(tool_obj) {
                continue;
            }
            let function = tool_obj.get("function").and_then(Value::as_object);
            let name = openai_chat_tool_name_value(tool_obj, function)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            if let Some(name) = name {
                names.push(name.to_string());
            }
        }
    }

    if let Some(functions) = obj.get("functions").and_then(Value::as_array) {
        for function in functions {
            let Some(function_obj) = function.as_object() else {
                continue;
            };
            let name =
                coerce_non_empty_scalar_to_string(legacy_openai_function_name_value(function_obj));
            if let Some(name) = name {
                names.push(name);
            }
        }
    }

    if let Some(dynamic_tools) = get_dynamic_tools_array(obj) {
        for dynamic_tool in dynamic_tools {
            let Some(name) = dynamic_tool
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                continue;
            };
            names.push(name.to_string());
        }
    }

    if let Some(name) = obj
        .get("tool_choice")
        .and_then(Value::as_object)
        .and_then(|tool_choice| {
            let tool_type = tool_choice
                .get("type")
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or_default();
            if tool_type != "function" {
                return None;
            }
            let function = tool_choice.get("function").and_then(Value::as_object);
            openai_chat_tool_name_value(tool_choice, function)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
    {
        names.push(name.to_string());
    }

    if let Some(name) = obj
        .get("function_call")
        .and_then(Value::as_object)
        .and_then(legacy_openai_function_name_value)
        .and_then(|value| coerce_non_empty_scalar_to_string(Some(value)))
    {
        names.push(name);
    }

    if let Some(messages) = obj.get("messages").and_then(Value::as_array) {
        for message in messages {
            let Some(message_obj) = message.as_object() else {
                continue;
            };
            if message_obj.get("role").and_then(Value::as_str) != Some("assistant") {
                continue;
            }
            let Some(tool_calls) = message_obj.get("tool_calls").and_then(Value::as_array) else {
                continue;
            };
            for tool_call in tool_calls {
                let Some(name) = tool_call
                    .get("function")
                    .and_then(|function| function.get("name"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                else {
                    continue;
                };
                names.push(name.to_string());
            }
        }
    }

    names
}

/// Build a stable deduplicated shortening map for all tool names in a request.
fn build_openai_tool_name_map(obj: &Map<String, Value>) -> BTreeMap<String, String> {
    let mut unique_names = BTreeSet::new();
    for name in collect_openai_tool_names(obj) {
        unique_names.insert(name);
    }

    let mut used = BTreeSet::new();
    let mut out = BTreeMap::new();
    for name in unique_names {
        let base = shorten_openai_tool_name_candidate(name.as_str());
        let mut candidate = base.clone();
        let mut suffix = 1usize;
        while used.contains(&candidate) {
            let suffix_text = format!("_{suffix}");
            let mut truncated = base.clone();
            let limit = MAX_OPENAI_TOOL_NAME_LEN.saturating_sub(suffix_text.len());
            if truncated.len() > limit {
                truncated = truncated.chars().take(limit).collect();
            }
            candidate = format!("{truncated}{suffix_text}");
            suffix += 1;
        }
        used.insert(candidate.clone());
        out.insert(name, candidate);
    }
    out
}

/// Build the reverse map used when adapting responses back to chat format.
fn build_openai_tool_name_restore_map(
    tool_name_map: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut restore_map = BTreeMap::new();
    for (original, shortened) in tool_name_map {
        if original != shortened {
            restore_map.insert(shortened.clone(), original.clone());
        }
    }
    restore_map
}

/// Flatten mixed chat message content into plain text instructions.
fn extract_openai_message_content_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(items) => {
            let mut out = String::new();
            for item in items {
                if let Some(text) = item.as_str() {
                    out.push_str(text);
                    continue;
                }
                let Some(item_obj) = item.as_object() else {
                    continue;
                };
                let item_type = item_obj
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if matches!(item_type, "text" | "input_text" | "output_text") {
                    if let Some(text) = item_obj.get("text").and_then(Value::as_str) {
                        out.push_str(text);
                    }
                }
            }
            out
        },
        Value::Null => String::new(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

/// Convert one chat user content item into a responses input item.
fn map_openai_user_content_item_to_responses_item(item: &Value) -> Option<Value> {
    if let Some(text) = item.as_str() {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return None;
        }
        return Some(json!({
            "type": "input_text",
            "text": trimmed,
        }));
    }

    let obj = item.as_object()?;
    let item_type = obj.get("type").and_then(Value::as_str).unwrap_or_default();
    match item_type {
        "text" | "input_text" | "output_text" => obj
            .get("text")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|text| {
                json!({
                    "type": "input_text",
                    "text": text,
                })
            }),
        "input_image" => {
            let mut mapped = Map::new();
            mapped.insert("type".to_string(), Value::String("input_image".to_string()));
            if let Some(image_url) = obj.get("image_url").cloned() {
                mapped.insert("image_url".to_string(), image_url);
            } else if let Some(file_id) = obj.get("file_id").cloned() {
                mapped.insert("file_id".to_string(), file_id);
            } else {
                return None;
            }
            Some(Value::Object(mapped))
        },
        "image_url" => {
            let image_url = obj
                .get("image_url")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .or_else(|| {
                    obj.get("image_url")
                        .and_then(Value::as_object)
                        .and_then(|image_url| image_url.get("url"))
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(ToString::to_string)
                })?;
            Some(json!({
                "type": "input_image",
                "image_url": image_url,
            }))
        },
        _ => None,
    }
}

/// Convert chat-style user content into responses input content items.
fn convert_user_message_content_to_responses_items(content: &Value) -> Vec<Value> {
    match content {
        Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                Vec::new()
            } else {
                vec![json!({
                    "type": "input_text",
                    "text": trimmed,
                })]
            }
        },
        Value::Array(items) => items
            .iter()
            .filter_map(map_openai_user_content_item_to_responses_item)
            .collect(),
        Value::Null => Vec::new(),
        other => {
            let text = serde_json::to_string(other).unwrap_or_default();
            let trimmed = text.trim();
            if trimmed.is_empty() {
                Vec::new()
            } else {
                vec![json!({
                    "type": "input_text",
                    "text": trimmed,
                })]
            }
        },
    }
}

/// Convert a chat tool-output content item into a responses output item.
fn map_tool_result_content_item_to_responses_output_item(item: &Value) -> Option<Value> {
    if let Some(text) = item.as_str() {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return None;
        }
        return Some(json!({
            "type": "input_text",
            "text": trimmed,
        }));
    }

    let obj = item.as_object()?;
    let item_type = obj.get("type").and_then(Value::as_str).unwrap_or_default();
    match item_type {
        "text" | "input_text" => obj
            .get("text")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|text| {
                json!({
                    "type": "input_text",
                    "text": text,
                })
            }),
        "input_image" => {
            let mut mapped = Map::new();
            mapped.insert("type".to_string(), Value::String("input_image".to_string()));
            if let Some(image_url) = obj.get("image_url").cloned() {
                mapped.insert("image_url".to_string(), image_url);
            } else if let Some(file_id) = obj.get("file_id").cloned() {
                mapped.insert("file_id".to_string(), file_id);
            } else {
                return None;
            }
            Some(Value::Object(mapped))
        },
        _ => serde_json::to_string(item).ok().and_then(|text| {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(json!({
                    "type": "input_text",
                    "text": trimmed,
                }))
            }
        }),
    }
}

/// Convert a chat `tool` role payload into the responses function-call output
/// shape.
fn convert_tool_message_content_to_responses_output(
    value: Option<&Value>,
) -> Result<Value, String> {
    let Some(value) = value else {
        return Ok(Value::String(String::new()));
    };
    if value.is_null() {
        return Ok(Value::String(String::new()));
    }
    if let Some(text) = value.as_str() {
        return Ok(Value::String(text.to_string()));
    }
    if let Some(items) = value.as_array() {
        let mapped_items = items
            .iter()
            .filter_map(map_tool_result_content_item_to_responses_output_item)
            .collect::<Vec<_>>();
        if mapped_items.is_empty() {
            return Ok(Value::String(String::new()));
        }
        return Ok(Value::Array(mapped_items));
    }
    if let Some(item) = map_tool_result_content_item_to_responses_output_item(value) {
        return Ok(Value::Array(vec![item]));
    }
    serde_json::to_string(value)
        .map(Value::String)
        .map_err(|err| format!("serialize tool result content failed: {err}"))
}

/// Flush pending assistant text parts into a single responses assistant
/// message.
fn flush_assistant_output_parts(input_items: &mut Vec<Value>, pending_parts: &mut Vec<Value>) {
    if pending_parts.is_empty() {
        return;
    }
    input_items.push(json!({
        "type": "message",
        "role": "assistant",
        "content": pending_parts.clone(),
    }));
    pending_parts.clear();
}

/// Convert assistant chat history into responses message items.
fn append_assistant_content_to_responses_input(
    input_items: &mut Vec<Value>,
    content: &Value,
) -> Result<(), String> {
    if let Some(text) = content.as_str() {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            input_items.push(json!({
                "type": "message",
                "role": "assistant",
                "content": [{ "type": "output_text", "text": trimmed }]
            }));
        }
        return Ok(());
    }

    let items = if let Some(array) = content.as_array() {
        array.to_vec()
    } else if content.is_object() {
        vec![content.clone()]
    } else if content.is_null() {
        Vec::new()
    } else {
        return Err("unsupported assistant content".to_string());
    };

    let mut pending_parts = Vec::new();
    for item in items {
        let Some(item_obj) = item.as_object() else {
            continue;
        };
        let item_type = item_obj
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match item_type {
            "text" | "output_text" => {
                if let Some(text) = item_obj.get("text").and_then(Value::as_str) {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        pending_parts.push(json!({
                            "type": "output_text",
                            "text": trimmed,
                        }));
                    }
                }
            },
            _ => {},
        }
    }
    flush_assistant_output_parts(input_items, &mut pending_parts);
    Ok(())
}

fn chat_message_bad_request(index: usize, message: impl AsRef<str>) -> CodexGatewayError {
    bad_request(&format!("chat.completions message {index}: {}", message.as_ref()))
}

/// Adapt an OpenAI chat/completions request into the upstream responses format.
fn adapt_openai_chat_completions_request(
    obj: &Map<String, Value>,
) -> CodexGatewayResult<OpenAiChatAdaptedRequest> {
    let source_messages = obj
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| bad_request("chat.completions messages field is required"))?;
    let tool_name_map = build_openai_tool_name_map(obj);
    let tool_name_restore_map = build_openai_tool_name_restore_map(&tool_name_map);

    let mut input_items = Vec::<Value>::new();
    let mut seen_tool_call_ids = BTreeSet::<String>::new();

    for (index, message) in source_messages.iter().enumerate() {
        let Some(message_obj) = message.as_object() else {
            return Err(chat_message_bad_request(index, "must be an object"));
        };
        let Some(role) = message_obj.get("role").and_then(Value::as_str) else {
            return Err(chat_message_bad_request(index, "is missing role"));
        };
        let Some(normalized_role) = normalize_openai_role_for_responses(role) else {
            return Err(chat_message_bad_request(index, format!("has unsupported role `{role}`")));
        };
        match normalized_role {
            "developer" => {
                let Some(content) = message_obj.get("content") else {
                    return Err(chat_message_bad_request(index, "is missing content"));
                };
                if !matches!(content, Value::String(_) | Value::Array(_)) {
                    return Err(chat_message_bad_request(
                        index,
                        "content must be a string or array",
                    ));
                }
                let text = extract_openai_message_content_text(content);
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    continue;
                }
                input_items.push(json!({
                    "type": "message",
                    "role": "developer",
                    "content": [{
                        "type": "input_text",
                        "text": trimmed,
                    }]
                }));
            },
            "user" => {
                let Some(content) = message_obj.get("content") else {
                    return Err(chat_message_bad_request(index, "is missing content"));
                };
                if !matches!(content, Value::String(_) | Value::Array(_)) {
                    return Err(chat_message_bad_request(
                        index,
                        "content must be a string or array",
                    ));
                }
                let content_items = convert_user_message_content_to_responses_items(content);
                if content_items.is_empty() {
                    continue;
                }
                input_items.push(json!({
                    "type": "message",
                    "role": "user",
                    "content": content_items
                }));
            },
            "assistant" => {
                let mut emitted_content = false;
                if let Some(content) = message_obj.get("content") {
                    if !matches!(
                        content,
                        Value::String(_) | Value::Array(_) | Value::Object(_) | Value::Null
                    ) {
                        return Err(chat_message_bad_request(
                            index,
                            "content has unsupported shape",
                        ));
                    }
                    let prior_len = input_items.len();
                    append_assistant_content_to_responses_input(&mut input_items, content)
                        .map_err(|err| bad_request_with_detail("Invalid assistant content", err))?;
                    emitted_content = input_items.len() > prior_len;
                }
                let mut emitted_tool_call = false;
                if let Some(raw_tool_calls) = message_obj.get("tool_calls") {
                    let Some(tool_calls) = raw_tool_calls.as_array() else {
                        return Err(chat_message_bad_request(index, "tool_calls must be an array"));
                    };
                    for (tool_call_index, tool_call) in tool_calls.iter().enumerate() {
                        let Some(tool_obj) = tool_call.as_object() else {
                            return Err(bad_request(&format!(
                                "chat.completions assistant tool_call {tool_call_index} must be \
                                 an object"
                            )));
                        };
                        let call_id = tool_obj
                            .get("id")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .ok_or_else(|| {
                                bad_request(&format!(
                                    "chat.completions assistant tool_call {tool_call_index} is \
                                     missing id"
                                ))
                            })?;
                        let Some(function_name) = coerce_non_empty_scalar_to_string(
                            tool_obj.get("function").and_then(|value| value.get("name")),
                        ) else {
                            continue;
                        };
                        let function_name =
                            shorten_openai_tool_name_with_map(&function_name, &tool_name_map);
                        let arguments = tool_obj
                            .get("function")
                            .and_then(|value| value.get("arguments"))
                            .map(|value| {
                                if let Some(text) = value.as_str() {
                                    text.to_string()
                                } else {
                                    serde_json::to_string(value)
                                        .unwrap_or_else(|_| "{}".to_string())
                                }
                            })
                            .unwrap_or_else(|| "{}".to_string());
                        input_items.push(json!({
                            "type": "function_call",
                            "call_id": call_id,
                            "name": function_name,
                            "arguments": arguments
                        }));
                        seen_tool_call_ids.insert(call_id.to_string());
                        emitted_tool_call = true;
                    }
                }
                if !emitted_content && !emitted_tool_call {
                    continue;
                }
            },
            "tool" => {
                let call_id = message_obj
                    .get("tool_call_id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| bad_request("tool role message missing tool_call_id"))?;
                if !seen_tool_call_ids.contains(call_id) {
                    continue;
                }
                let output =
                    convert_tool_message_content_to_responses_output(message_obj.get("content"))
                        .map_err(|err| bad_request_with_detail("Invalid tool content", err))?;
                input_items.push(json!({
                    "type": "function_call_output",
                    "call_id": call_id,
                    "output": output
                }));
            },
            _ => {},
        }
    }

    let mut out = Map::new();
    if let Some(model) = obj.get("model") {
        out.insert("model".to_string(), model.clone());
    }
    out.insert("input".to_string(), Value::Array(input_items));

    let stream = obj.get("stream").and_then(Value::as_bool).unwrap_or(false);
    out.insert("stream".to_string(), Value::Bool(stream));
    out.insert("store".to_string(), Value::Bool(false));

    let reasoning_effort = obj
        .get("reasoning_effort")
        .and_then(Value::as_str)
        .and_then(normalize_reasoning_effort)
        .or_else(|| {
            obj.get("reasoning")
                .and_then(|reasoning| reasoning.get("effort"))
                .and_then(Value::as_str)
                .and_then(normalize_reasoning_effort)
        })
        .unwrap_or("medium");
    out.insert(
        "reasoning".to_string(),
        json!({
            "effort": reasoning_effort
        }),
    );

    let parallel_tool_calls = obj
        .get("parallel_tool_calls")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    out.insert("parallel_tool_calls".to_string(), Value::Bool(parallel_tool_calls));
    out.insert(
        "include".to_string(),
        Value::Array(vec![Value::String("reasoning.encrypted_content".to_string())]),
    );

    if let Some(service_tier) = obj.get("service_tier") {
        out.insert("service_tier".to_string(), service_tier.clone());
    }

    if let Some(tools) = obj.get("tools").and_then(Value::as_array) {
        let mut mapped_tools = Vec::with_capacity(tools.len());
        for tool in tools {
            let Some(tool_obj) = tool.as_object() else {
                continue;
            };
            if !is_openai_chat_function_tool(tool_obj) {
                mapped_tools.push(tool.clone());
                continue;
            }
            if let Some(mapped) = map_openai_chat_function_tool(tool_obj, &tool_name_map) {
                mapped_tools.push(mapped);
            }
        }
        if !mapped_tools.is_empty() {
            out.insert("tools".to_string(), Value::Array(mapped_tools));
        }
    }

    if let Some(functions) = obj.get("functions").and_then(Value::as_array) {
        let mut mapped_functions = out
            .remove("tools")
            .and_then(|value| value.as_array().cloned())
            .unwrap_or_default();
        for function in functions {
            let Some(function_obj) = function.as_object() else {
                continue;
            };
            let mut wrapped = Map::new();
            wrapped.insert("type".to_string(), Value::String("function".to_string()));
            wrapped.insert("function".to_string(), Value::Object(function_obj.clone()));
            if let Some(mapped) = map_openai_chat_function_tool(&wrapped, &tool_name_map) {
                mapped_functions.push(mapped);
            }
        }
        if !mapped_functions.is_empty() {
            out.insert("tools".to_string(), Value::Array(mapped_functions));
        }
    }

    if let Some(dynamic_tools) = get_dynamic_tools_array(obj) {
        let mut mapped_dynamic_tools = out
            .remove("tools")
            .and_then(|value| value.as_array().cloned())
            .unwrap_or_default();
        for dynamic_tool in dynamic_tools {
            let Some(tool_obj) = dynamic_tool.as_object() else {
                continue;
            };
            let name = tool_obj
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|name| shorten_openai_tool_name_with_map(name, &tool_name_map));
            let Some(name) = name else {
                continue;
            };
            let description = tool_obj
                .get("description")
                .cloned()
                .unwrap_or_else(|| Value::String(String::new()));
            let parameters = tool_obj
                .get("input_schema")
                .or_else(|| tool_obj.get("inputSchema"))
                .or_else(|| tool_obj.get("parameters"))
                .cloned()
                .map(normalize_tool_parameters_schema)
                .unwrap_or_else(|| json!({ "type": "object", "properties": {} }));
            let mut mapped = Map::new();
            mapped.insert("type".to_string(), Value::String("function".to_string()));
            mapped.insert("name".to_string(), Value::String(name));
            mapped.insert("description".to_string(), description);
            mapped.insert("parameters".to_string(), parameters);
            if let Some(strict) = tool_obj.get("strict") {
                mapped.insert("strict".to_string(), strict.clone());
            }
            mapped_dynamic_tools.push(Value::Object(mapped));
        }
        if !mapped_dynamic_tools.is_empty() {
            out.insert("tools".to_string(), Value::Array(mapped_dynamic_tools));
        }
    }

    if let Some(tool_choice) = obj.get("tool_choice") {
        if let Some(tool_choice_str) = tool_choice.as_str() {
            out.insert("tool_choice".to_string(), Value::String(tool_choice_str.to_string()));
        } else if let Some(tool_choice_obj) = tool_choice.as_object() {
            let tool_type = tool_choice_obj
                .get("type")
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or_default();
            if tool_type == "function" {
                let function = tool_choice_obj.get("function").and_then(Value::as_object);
                if let Some(name) = coerce_non_empty_scalar_to_string(openai_chat_tool_name_value(
                    tool_choice_obj,
                    function,
                )) {
                    out.insert(
                        "tool_choice".to_string(),
                        json!({
                            "type": "function",
                            "name": shorten_openai_tool_name_with_map(&name, &tool_name_map),
                        }),
                    );
                }
            } else {
                out.insert("tool_choice".to_string(), tool_choice.clone());
            }
        }
    }

    if !out.contains_key("tool_choice") {
        if let Some(function_call) = obj.get("function_call") {
            if let Some(function_call_str) = function_call.as_str() {
                out.insert("tool_choice".to_string(), Value::String(function_call_str.to_string()));
            } else if let Some(function_call_obj) = function_call.as_object() {
                if let Some(name) = coerce_non_empty_scalar_to_string(
                    legacy_openai_function_name_value(function_call_obj),
                ) {
                    out.insert(
                        "tool_choice".to_string(),
                        json!({
                            "type": "function",
                            "name": shorten_openai_tool_name_with_map(&name, &tool_name_map),
                        }),
                    );
                }
            }
        }
    }

    let mut text = obj
        .get("text")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(verbosity) = obj
        .get("verbosity")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        text.insert("verbosity".to_string(), Value::String(verbosity.to_string()));
    }
    if let Some(format) = obj.get("response_format").cloned() {
        text.insert("format".to_string(), format);
    }
    if !text.is_empty() {
        out.insert("text".to_string(), Value::Object(text));
    }

    Ok((out, tool_name_restore_map))
}

/// Fill defaults and normalize request shape for upstream responses endpoints.
pub fn normalize_responses_request(
    path: &str,
    root: &mut serde_json::Map<String, Value>,
    thread_anchor: Option<&str>,
) {
    normalize_codex_input_message_roles(root);

    if root
        .get("instructions")
        .and_then(Value::as_str)
        .is_some_and(|value| value.trim().is_empty())
        || matches!(root.get("instructions"), Some(Value::Null))
    {
        root.remove("instructions");
    }
    if path == "/v1/responses/compact" {
        root.insert("store".to_string(), Value::Bool(false));
    }
    if matches!(path, "/v1/responses" | "/v1/responses/compact") {
        root.entry("instructions".to_string())
            .or_insert_with(|| Value::String(codex_default_instructions().to_string()));
    }
    root.entry("tools".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));

    if path == "/v1/responses" {
        root.entry("include".to_string())
            .or_insert_with(|| Value::Array(Vec::new()));
    }

    let has_non_empty_tools = root
        .get("tools")
        .and_then(Value::as_array)
        .is_some_and(|items| !items.is_empty());
    if !has_non_empty_tools {
        root.entry("parallel_tool_calls".to_string())
            .or_insert(Value::Bool(false));
    }

    if matches!(path, "/v1/responses" | "/v1/responses/compact") {
        if let Some(thread_anchor) = thread_anchor
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            let needs_prompt_cache_key = root
                .get("prompt_cache_key")
                .and_then(Value::as_str)
                .map(str::trim)
                .is_none_or(|value| value.is_empty());
            if needs_prompt_cache_key {
                root.insert(
                    "prompt_cache_key".to_string(),
                    Value::String(thread_anchor.to_string()),
                );
            }
        }
    }

    if let Some(input) = root.get_mut("input") {
        match input {
            Value::String(text) => {
                let mut content = serde_json::Map::new();
                content.insert("type".to_string(), Value::String("input_text".to_string()));
                content.insert("text".to_string(), Value::String(text.clone()));

                let mut message = serde_json::Map::new();
                message.insert("type".to_string(), Value::String("message".to_string()));
                message.insert("role".to_string(), Value::String("user".to_string()));
                message.insert("content".to_string(), Value::Array(vec![Value::Object(content)]));
                *input = Value::Array(vec![Value::Object(message)]);
            },
            Value::Object(_) => {
                *input = Value::Array(vec![input.clone()]);
            },
            _ => {},
        }
    }

    if path == "/v1/responses" {
        let has_reasoning = root.contains_key("reasoning");
        let include = root
            .entry("include".to_string())
            .or_insert_with(|| Value::Array(Vec::new()));
        if !include.is_array() {
            *include = Value::Array(Vec::new());
        }
        if has_reasoning {
            let include_array = include
                .as_array_mut()
                .expect("include array just initialized");
            if !include_array.iter().any(|value| {
                value
                    .as_str()
                    .map(|item| item == "reasoning.encrypted_content")
                    .unwrap_or(false)
            }) {
                include_array.push(Value::String("reasoning.encrypted_content".to_string()));
            }
        }
        if let Some(service_tier) = root.get_mut("service_tier") {
            if service_tier
                .as_str()
                .is_some_and(|raw| raw.eq_ignore_ascii_case("fast"))
            {
                *service_tier = Value::String("priority".to_string());
            }
        }
    }
}


fn is_supported_codex_post_path(path: &str) -> bool {
    matches!(
        path,
        "/v1/responses"
            | "/v1/responses/compact"
            | "/v1/chat/completions"
            | "/v1/messages"
            | "/v1/memories/trace_summarize"
            | "/v1/realtime/calls"
            | "/v1/files"
    ) || is_codex_file_finalize_path(path)
}

fn is_codex_file_finalize_path(path: &str) -> bool {
    let Some(file_id) = path
        .strip_prefix("/v1/files/")
        .and_then(|value| value.strip_suffix("/uploaded"))
    else {
        return false;
    };
    !file_id.is_empty() && !file_id.contains('/')
}


/// Reconstruct the externally visible origin from reverse-proxy headers.
pub fn external_origin(headers: &HeaderMap) -> Option<String> {
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


/// Return whether the path targets the supported `/v1/models` endpoint.
pub fn is_models_path(path: &str) -> bool {
    path == "/v1/models" || path.starts_with("/v1/models?")
}

/// Validate and normalize a human-facing key name.
pub fn normalize_name(raw: &str) -> CodexGatewayResult<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(bad_request("name is required"));
    }
    Ok(trimmed.to_string())
}

/// Validate the small set of supported key status values.
pub fn normalize_status(raw: &str) -> CodexGatewayResult<String> {
    let trimmed = raw.trim();
    match trimmed {
        LLM_GATEWAY_KEY_STATUS_ACTIVE | LLM_GATEWAY_KEY_STATUS_DISABLED => Ok(trimmed.to_string()),
        _ => Err(bad_request("status must be `active` or `disabled`")),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use axum::{
        body::{Body, Bytes},
        http::{header, HeaderValue, Method, StatusCode},
    };
    use serde_json::json;

    use super::{
        adapt_openai_chat_completions_request, align_responses_store_with_upstream,
        apply_codex_fast_policy, codex_default_instructions, prepare_gateway_request,
    };
    use crate::types::{GatewayResponseAdapter, PreparedGatewayRequest};

    #[test]
    fn adapt_openai_chat_completions_request_rejects_message_without_role() {
        let obj = json!({
            "model": "gpt-5.3-codex",
            "messages": [
                {
                    "content": "hello"
                }
            ]
        });

        let err = adapt_openai_chat_completions_request(
            obj.as_object().expect("sample request should be an object"),
        )
        .expect_err("request without message role should be rejected");
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        assert!(err.message.contains("role"));
    }

    #[test]
    fn adapt_openai_chat_completions_request_drops_user_message_without_supported_content() {
        let obj = json!({
            "model": "gpt-5.3-codex",
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "image_url"
                        }
                    ]
                }
            ]
        });

        let (adapted, _) = adapt_openai_chat_completions_request(
            obj.as_object().expect("sample request should be an object"),
        )
        .expect("unsupported user content should be dropped");
        assert_eq!(adapted["input"], json!([]));
    }

    fn prepared_responses_request(path: &str, body: serde_json::Value) -> PreparedGatewayRequest {
        PreparedGatewayRequest {
            original_path: path.to_string(),
            upstream_path: path.to_string(),
            method: Method::POST,
            client_request_body: None,
            request_body: Bytes::from(serde_json::to_vec(&body).expect("request body json")),
            model: body
                .get("model")
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string),
            client_visible_model: None,
            wants_stream: false,
            force_upstream_stream: false,
            content_type: "application/json".to_string(),
            response_adapter: GatewayResponseAdapter::Responses,
            thread_anchor: None,
            tool_name_restore_map: Default::default(),
            billable_multiplier: 1,
            last_message_content: None,
        }
    }

    #[test]
    fn align_responses_store_with_upstream_sets_false_for_non_azure() {
        let prepared = prepared_responses_request(
            "/v1/responses",
            json!({
                "model": "gpt-5.3-codex",
                "input": "hello",
                "previous_response_id": "resp_1"
            }),
        );

        let aligned =
            align_responses_store_with_upstream(&prepared, "https://chatgpt.com/backend-api/codex")
                .expect("store alignment should succeed");
        let body: serde_json::Value =
            serde_json::from_slice(&aligned.request_body).expect("aligned body json");

        assert_eq!(body.get("store"), Some(&json!(false)));
        assert_eq!(body.get("previous_response_id"), None);
    }

    #[test]
    fn align_responses_store_with_upstream_removes_input_item_ids_for_non_azure() {
        let prepared = prepared_responses_request(
            "/v1/responses",
            json!({
                "model": "gpt-5.3-codex",
                "input": [
                    {
                        "type": "message",
                        "id": "rs_item_1",
                        "role": "assistant",
                        "content": [{"type": "output_text", "text": "pong"}]
                    }
                ]
            }),
        );

        let aligned =
            align_responses_store_with_upstream(&prepared, "https://chatgpt.com/backend-api/codex")
                .expect("store alignment should succeed");
        let body: serde_json::Value =
            serde_json::from_slice(&aligned.request_body).expect("aligned body json");

        assert_eq!(body["input"][0].get("id"), None);
        assert_eq!(body.get("store"), Some(&json!(false)));
    }

    #[test]
    fn align_responses_store_with_upstream_sets_true_for_azure() {
        let prepared = prepared_responses_request(
            "/v1/responses",
            json!({
                "model": "gpt-5.3-codex",
                "input": "hello",
                "store": false,
                "previous_response_id": "resp_1"
            }),
        );

        let aligned = align_responses_store_with_upstream(
            &prepared,
            "https://foo.openai.azure.com/openai/deployments/bar",
        )
        .expect("store alignment should succeed");
        let body: serde_json::Value =
            serde_json::from_slice(&aligned.request_body).expect("aligned body json");

        assert_eq!(body.get("store"), Some(&json!(true)));
        assert_eq!(body.get("previous_response_id"), Some(&json!("resp_1")));
    }

    #[test]
    fn align_responses_store_with_upstream_keeps_input_item_ids_for_azure() {
        let prepared = prepared_responses_request(
            "/v1/responses",
            json!({
                "model": "gpt-5.3-codex",
                "input": [
                    {
                        "type": "message",
                        "id": "rs_item_1",
                        "role": "assistant",
                        "content": [{"type": "output_text", "text": "pong"}]
                    }
                ],
                "store": false
            }),
        );

        let aligned = align_responses_store_with_upstream(
            &prepared,
            "https://foo.openai.azure.com/openai/deployments/bar",
        )
        .expect("store alignment should succeed");
        let body: serde_json::Value =
            serde_json::from_slice(&aligned.request_body).expect("aligned body json");

        assert_eq!(body["input"][0]["id"], json!("rs_item_1"));
        assert_eq!(body.get("store"), Some(&json!(true)));
    }

    #[test]
    fn align_responses_store_with_upstream_removes_compact_store_field() {
        let prepared = prepared_responses_request(
            "/v1/responses/compact",
            json!({
                "model": "gpt-5.3-codex",
                "input": [{
                    "type": "message",
                    "id": "rs_item_1",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": "hello compact"}]
                }],
                "store": true,
                "previous_response_id": "resp_compact_1"
            }),
        );

        let aligned =
            align_responses_store_with_upstream(&prepared, "https://chatgpt.com/backend-api/codex")
                .expect("store alignment should succeed");
        let body: serde_json::Value =
            serde_json::from_slice(&aligned.request_body).expect("aligned body json");

        assert_eq!(body.get("store"), None);
        assert_eq!(body.get("previous_response_id"), None);
        assert_eq!(body["input"][0].get("id"), None);
    }

    #[test]
    fn apply_codex_fast_policy_keeps_priority_and_multiplier_when_enabled() {
        let prepared = prepared_responses_request(
            "/v1/responses",
            json!({
                "model": "gpt-5.3-codex",
                "input": "hello",
                "service_tier": "fast"
            }),
        );

        let adjusted =
            apply_codex_fast_policy(&prepared, true).expect("fast-enabled policy should succeed");
        let body: serde_json::Value =
            serde_json::from_slice(&adjusted.request_body).expect("adjusted body json");

        assert_eq!(body.get("service_tier"), Some(&json!("priority")));
        assert_eq!(adjusted.billable_multiplier, 2);
    }

    #[test]
    fn apply_codex_fast_policy_strips_priority_and_multiplier_when_disabled() {
        let prepared = prepared_responses_request(
            "/v1/responses",
            json!({
                "model": "gpt-5.3-codex",
                "input": "hello",
                "service_tier": "priority"
            }),
        );

        let adjusted =
            apply_codex_fast_policy(&prepared, false).expect("fast-disabled policy should succeed");
        let body: serde_json::Value =
            serde_json::from_slice(&adjusted.request_body).expect("adjusted body json");

        assert_eq!(body.get("service_tier"), None);
        assert_eq!(adjusted.billable_multiplier, 1);
    }

    #[test]
    fn apply_codex_fast_policy_leaves_flex_unchanged_when_disabled() {
        let prepared = prepared_responses_request(
            "/v1/responses",
            json!({
                "model": "gpt-5.3-codex",
                "input": "hello",
                "service_tier": "flex"
            }),
        );

        let adjusted =
            apply_codex_fast_policy(&prepared, false).expect("non-fast tier policy should succeed");
        let body: serde_json::Value =
            serde_json::from_slice(&adjusted.request_body).expect("adjusted body json");

        assert_eq!(body.get("service_tier"), Some(&json!("flex")));
        assert_eq!(adjusted.billable_multiplier, 1);
    }

    #[tokio::test]
    async fn prepare_gateway_request_rejects_chat_message_without_role() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(r#"{"model":"gpt-5.3-codex","messages":[{"content":"hello"}]}"#);

        let err = prepare_gateway_request(
            "/v1/chat/completions",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect_err("message without role should fail");

        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        assert!(err.message.contains("role"));
    }

    #[tokio::test]
    async fn prepare_gateway_request_keeps_native_responses_body_and_last_message_content() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(r#"{"model":"gpt-5.3-codex","input":"hello"}"#);

        let prepared = prepare_gateway_request(
            "/v1/responses",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("responses request should pass through");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");

        assert!(prepared.client_request_body.is_none());
        assert_eq!(prepared.last_message_content.as_deref(), Some("hello"));
        assert_eq!(prepared.wants_stream, false);
        assert_eq!(prepared.force_upstream_stream, true);
        assert_eq!(upstream["input"][0]["type"], json!("message"));
        assert_eq!(upstream["input"][0]["role"], json!("user"));
        assert_eq!(upstream["input"][0]["content"][0]["type"], json!("input_text"));
        assert_eq!(upstream["input"][0]["content"][0]["text"], json!("hello"));
        assert_eq!(upstream["stream"], json!(true));
    }

    #[tokio::test]
    async fn prepare_gateway_request_injects_default_instructions_for_native_responses_when_missing(
    ) {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(r#"{"model":"gpt-5.3-codex","input":"hello"}"#);

        let prepared = prepare_gateway_request(
            "/v1/responses",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("responses request should pass through");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");

        assert_eq!(upstream["instructions"].as_str(), Some(codex_default_instructions()));
    }

    #[tokio::test]
    async fn prepare_gateway_request_responses_preserves_native_codex_fields() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "input":"hello",
                "previous_response_id":"resp_123",
                "tools":[{"type":"function","name":"lookup","description":"Look up data.","parameters":{"type":"object","properties":{"q":{"type":"string"}}}}],
                "tool_choice":{"type":"function","name":"lookup"},
                "service_tier":"flex",
                "store":true,
                "client_metadata":{"source":"test"},
                "max_output_tokens":64,
                "max_completion_tokens":32,
                "max_tokens":16,
                "verbosity":"high"
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/responses",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("responses request should pass through");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");

        assert_eq!(upstream["tools"][0]["name"], json!("lookup"));
        assert_eq!(upstream["tool_choice"], json!({"type":"function","name":"lookup"}));
        assert_eq!(upstream["service_tier"], "flex");
        assert_eq!(upstream["stream"], json!(true));
        assert_eq!(upstream["store"], json!(true));
        assert_eq!(upstream["client_metadata"], json!({"source":"test"}));
        assert_eq!(upstream["previous_response_id"], json!("resp_123"));
        assert!(upstream.get("max_output_tokens").is_none());
        assert_eq!(upstream["max_completion_tokens"], json!(32));
        assert_eq!(upstream["max_tokens"], json!(16));
        assert_eq!(upstream["verbosity"], json!("high"));
    }

    #[tokio::test]
    async fn prepare_gateway_request_responses_strips_temperature_before_upstream() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "input":"hello",
                "stream":false,
                "temperature":0.7
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/responses",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("responses request should pass through");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");

        assert_eq!(prepared.force_upstream_stream, true);
        assert_eq!(upstream["stream"], json!(true));
        assert!(upstream.get("temperature").is_none());
    }

    #[tokio::test]
    async fn prepare_gateway_request_injects_default_instructions_for_bare_chat() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{"model":"gpt-5.3-codex","messages":[{"role":"user","content":"hello"}]}"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/chat/completions",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("chat request should normalize");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");

        assert_eq!(upstream["instructions"].as_str(), Some(codex_default_instructions()));
    }

    #[tokio::test]
    async fn prepare_gateway_request_chat_maps_system_message_to_developer_for_json_object() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "response_format":{"type":"json_object"},
                "messages":[
                    {"role":"system","content":"Return valid JSON only."},
                    {"role":"user","content":"hello"}
                ]
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/chat/completions",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("chat request with system json instruction should normalize");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");

        assert_eq!(upstream["text"]["format"]["type"], "json_object");
        assert_eq!(upstream["input"][0]["type"], "message");
        assert_eq!(upstream["input"][0]["role"], "developer");
        assert_eq!(upstream["input"][0]["content"][0]["type"], "input_text");
        assert_eq!(upstream["input"][0]["content"][0]["text"], "Return valid JSON only.");
        assert_eq!(upstream["instructions"].as_str(), Some(codex_default_instructions()));
    }

    #[tokio::test]
    async fn prepare_gateway_request_responses_preserves_system_message_role() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "input":[
                    {"type":"message","role":"system","content":[{"type":"input_text","text":"Reply with exactly PONG."}]},
                    {"type":"message","role":"user","content":[{"type":"input_text","text":"ping"}]}
                ]
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/responses",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("responses request with system message should pass through");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");

        assert_eq!(upstream["input"][0]["role"], "system");
        assert_eq!(upstream["input"][1]["role"], "user");
    }

    #[tokio::test]
    async fn prepare_gateway_request_repairs_native_responses_tool_role_message() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "input":[
                    {"role":"user","content":"call lookup"},
                    {"role":"tool","tool_call_id":"call_1","content":"{\"ok\":true}"}
                ]
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/responses",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("native responses tool role should be repaired before upstream dispatch");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");
        assert_eq!(upstream["input"][0]["role"], json!("user"));
        assert_eq!(upstream["input"][1]["type"], json!("function_call_output"));
        assert_eq!(upstream["input"][1]["call_id"], json!("call_1"));
        assert_eq!(upstream["input"][1]["output"], json!("{\"ok\":true}"));
        assert!(upstream["input"][1].get("role").is_none());
        assert!(upstream["input"][1].get("tool_call_id").is_none());
    }

    #[tokio::test]
    async fn prepare_gateway_request_rewrites_native_responses_tool_role_without_call_id_to_user_message(
    ) {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "input":[
                    {"role":"tool","content":"standalone tool output"}
                ]
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/responses",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("native responses tool role without call id should be repaired");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");
        assert_eq!(upstream["input"][0]["type"], json!("message"));
        assert_eq!(upstream["input"][0]["role"], json!("user"));
        assert_eq!(upstream["input"][0]["content"][0]["text"], json!("standalone tool output"));
        assert!(upstream["input"][0].get("tool_call_id").is_none());
    }

    #[tokio::test]
    async fn prepare_gateway_request_responses_skips_local_json_object_validation() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "text":{"format":{"type":"json_object"}},
                "input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"hello"}]}]
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/responses",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("native responses request should pass through");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");
        assert_eq!(upstream["text"]["format"]["type"], "json_object");
        assert_eq!(upstream["input"][0]["content"][0]["text"], "hello");
    }

    #[tokio::test]
    async fn prepare_gateway_request_repairs_chat_tool_call_without_output() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "messages":[
                    {"role":"user","content":"hello"},
                    {"role":"assistant","tool_calls":[{"id":"callauto12","type":"function","function":{"name":"lookup","arguments":"{}"}}]}
                ]
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/chat/completions",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("chat request with unmatched tool call should be repaired");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");
        assert_eq!(upstream["input"].as_array().map(Vec::len), Some(1));
        assert_eq!(upstream["input"][0]["role"], "user");
    }

    #[tokio::test]
    async fn prepare_gateway_request_rejects_chat_tool_call_without_id() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "messages":[
                    {"role":"user","content":"hello"},
                    {"role":"assistant","tool_calls":[{"type":"function","function":{"name":"lookup","arguments":"{}"}}]}
                ]
            }"#,
        );

        let err = prepare_gateway_request(
            "/v1/chat/completions",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect_err("chat request without tool call id should fail locally");

        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        assert!(err.message.contains("missing id"));
    }

    #[tokio::test]
    async fn prepare_gateway_request_repairs_chat_tool_call_without_string_function_name() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "messages":[
                    {"role":"user","content":"hello"},
                    {"role":"assistant","tool_calls":[{"id":"callauto12","type":"function","function":{"name":{"bad":true},"arguments":"{}"}}]}
                ]
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/chat/completions",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("chat request with malformed tool call should normalize");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");
        assert_eq!(upstream["input"].as_array().map(Vec::len), Some(1));
        assert_eq!(upstream["input"][0]["role"], "user");
    }

    #[tokio::test]
    async fn prepare_gateway_request_repairs_chat_orphan_tool_output() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "messages":[
                    {"role":"user","content":"hello"},
                    {"role":"tool","tool_call_id":"callauto12","content":"{\"ok\":true}"}
                ]
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/chat/completions",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("chat request with orphan tool output should be repaired");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");
        assert_eq!(upstream["input"].as_array().map(Vec::len), Some(1));
        assert_eq!(upstream["input"][0]["role"], "user");
    }

    #[tokio::test]
    async fn prepare_gateway_request_responses_preserves_previous_response_tool_output_delta() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "previous_response_id":"resp_1",
                "input":[
                    {"type":"function_call_output","call_id":"callauto12","output":"{\"ok\":true}"}
                ]
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/responses",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("responses request with previous_response_id should pass through");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");
        assert_eq!(upstream["previous_response_id"], json!("resp_1"));
        assert_eq!(upstream["input"][0]["type"], json!("function_call_output"));
    }

    #[tokio::test]
    async fn prepare_gateway_request_responses_preserves_function_call_namespace() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "input":[
                    {
                        "type":"function_call",
                        "call_id":"call_ns_1",
                        "name":"find_gameobjects",
                        "namespace":"game_tools",
                        "arguments":"{\"scene\":\"main\"}"
                    },
                    {
                        "type":"function_call_output",
                        "call_id":"call_ns_1",
                        "output":"[]"
                    }
                ]
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/responses",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("native responses request should preserve namespace");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");
        assert_eq!(upstream["input"][0]["type"], json!("function_call"));
        assert_eq!(upstream["input"][0]["namespace"], json!("game_tools"));
        assert_eq!(upstream["input"][0]["name"], json!("find_gameobjects"));
    }

    #[tokio::test]
    async fn prepare_gateway_request_responses_preserves_custom_tool_call_shape() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "input":[
                    {"type":"custom_tool_call","call_id":"callpatch1","name":"apply_patch","arguments":"*** Begin Patch"},
                    {"type":"custom_tool_call_output","call_id":"callpatch1","output":"ok"}
                ]
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/responses",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("native responses request should pass through");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");
        assert_eq!(upstream["input"][0]["type"], json!("custom_tool_call"));
        assert_eq!(upstream["input"][0]["call_id"], json!("callpatch1"));
        assert_eq!(upstream["input"][0]["arguments"], json!("*** Begin Patch"));
        assert!(upstream["input"][0].get("input").is_none());
        assert_eq!(upstream["input"][1]["type"], json!("custom_tool_call_output"));
    }

    #[tokio::test]
    async fn prepare_gateway_request_repairs_function_tool_schema_missing_properties() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "messages":[{"role":"user","content":"hello"}],
                "tools":[{
                    "type":"function",
                    "function":{
                        "name":"mcp__matlab__detect_matlab_toolboxes",
                        "description":"Detect installed MATLAB toolboxes.",
                        "parameters":{"type":"object"}
                    }
                }]
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/chat/completions",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("tool schema without properties should normalize");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");
        assert_eq!(upstream["tools"][0]["parameters"], json!({"type":"object","properties":{}}));
    }

    #[tokio::test]
    async fn prepare_gateway_request_coerces_chat_function_tool_scalar_name_to_string() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "messages":[{"role":"user","content":"hello"}],
                "tools":[{
                    "type":"function",
                    "function":{
                        "name":123,
                        "parameters":{"type":"object","properties":{}}
                    }
                }]
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/chat/completions",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("chat request with scalar function tool name should normalize");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");
        assert_eq!(upstream["tools"][0]["name"], json!("123"));
    }

    #[tokio::test]
    async fn prepare_gateway_request_accepts_chat_function_tool_with_top_level_name() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "messages":[{"role":"user","content":"hello"}],
                "tools":[{
                    "type":"function",
                    "name":"lookup",
                    "description":"Look up data.",
                    "parameters":{"type":"object","properties":{"q":{"type":"string"}}}
                }],
                "tool_choice":{"type":"function","name":"lookup"}
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/chat/completions",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("chat request with responses-style function tool should normalize");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");
        assert_eq!(upstream["tools"][0]["type"], json!("function"));
        assert_eq!(upstream["tools"][0]["name"], json!("lookup"));
        assert_eq!(
            upstream["tools"][0]["parameters"],
            json!({"type":"object","properties":{"q":{"type":"string"}}})
        );
        assert_eq!(upstream["tool_choice"], json!({"type":"function","name":"lookup"}));
    }

    #[tokio::test]
    async fn prepare_gateway_request_accepts_legacy_functions_and_function_call() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "messages":[{"role":"user","content":"hello"}],
                "functions":[{
                    "name":"lookup",
                    "description":"Look up data.",
                    "parameters":{"type":"object","properties":{"q":{"type":"string"}}}
                }],
                "function_call":{"name":"lookup"}
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/chat/completions",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("legacy chat function request should normalize");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");
        assert_eq!(upstream["tools"][0]["type"], json!("function"));
        assert_eq!(upstream["tools"][0]["name"], json!("lookup"));
        assert_eq!(
            upstream["tools"][0]["parameters"],
            json!({"type":"object","properties":{"q":{"type":"string"}}})
        );
        assert_eq!(upstream["tool_choice"], json!({"type":"function","name":"lookup"}));
    }

    #[tokio::test]
    async fn prepare_gateway_request_coerces_chat_function_tool_choice_scalar_name_to_string() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "messages":[{"role":"user","content":"hello"}],
                "tool_choice":{
                    "type":"function",
                    "function":{"name":123}
                }
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/chat/completions",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("chat request with scalar function tool_choice name should normalize");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");
        assert_eq!(upstream["tool_choice"], json!({"type":"function","name":"123"}));
    }

    #[tokio::test]
    async fn prepare_gateway_request_responses_preserves_unmatched_tool_calls() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "input":[
                    {"type":"function_call","call_id":"callauto12","name":"lookup","arguments":"{}"}
                ]
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/responses",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("native responses request should pass through");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");
        assert_eq!(upstream["input"][0]["type"], json!("function_call"));
        assert_eq!(upstream["input"][0]["call_id"], json!("callauto12"));
    }

    #[tokio::test]
    async fn prepare_gateway_request_responses_preserves_message_item_ids() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "input":[
                    {"type":"message","id":"item_bad","role":"assistant","content":[{"type":"output_text","text":"pong"}]}
                ]
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/responses",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("native responses request should pass through");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");
        assert_eq!(upstream["input"][0]["id"], json!("item_bad"));
    }

    #[tokio::test]
    async fn prepare_gateway_request_anthropic_messages_maps_to_responses() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "max_tokens":512,
                "stream":false,
                "system":"Return JSON only.",
                "tools":[{
                    "name":"lookup_weather",
                    "description":"Look up the weather.",
                    "input_schema":{
                        "type":"object",
                        "properties":{"city":{"type":"string"}},
                        "required":["city"]
                    }
                }],
                "tool_choice":{"type":"tool","name":"lookup_weather"},
                "thinking":{"type":"adaptive","budget_tokens":4096},
                "output_config":{
                    "effort":"high",
                    "format":{
                        "type":"json_schema",
                        "schema":{
                            "type":"object",
                            "properties":{"answer":{"type":"string"}},
                            "required":["answer"],
                            "additionalProperties":false
                        }
                    }
                },
                "messages":[
                    {"role":"user","content":"weather in tokyo"},
                    {"role":"assistant","content":[
                        {"type":"text","text":"Let me check."},
                        {"type":"tool_use","id":"toolu_1","name":"lookup_weather","input":{"city":"Tokyo"}}
                    ]},
                    {"role":"user","content":[
                        {"type":"tool_result","tool_use_id":"toolu_1","content":"{\"temp_c\":24}"}
                    ]}
                ]
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/messages",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("messages request should normalize");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");

        assert_eq!(prepared.upstream_path, "/v1/responses");
        assert_eq!(upstream["instructions"].as_str(), Some(codex_default_instructions()));
        assert_eq!(upstream["input"][0]["role"], "developer");
        assert_eq!(upstream["input"][0]["content"][0]["text"], "Return JSON only.");
        assert_eq!(upstream["input"][1]["role"], "user");
        assert_eq!(upstream["input"][1]["content"][0]["text"], "weather in tokyo");
        assert_eq!(upstream["input"][2]["role"], "assistant");
        assert_eq!(upstream["input"][2]["content"][0]["type"], "output_text");
        assert_eq!(upstream["input"][2]["content"][0]["text"], "Let me check.");
        assert_eq!(upstream["input"][3]["type"], "function_call");
        assert_eq!(upstream["input"][3]["call_id"], "toolu_1");
        assert_eq!(upstream["input"][3]["name"], "lookup_weather");
        assert_eq!(upstream["input"][4]["type"], "function_call_output");
        assert_eq!(upstream["input"][4]["call_id"], "toolu_1");
        assert_eq!(upstream["text"]["format"]["type"], "json_schema");
        assert_eq!(upstream["reasoning"]["effort"], "high");
        assert_eq!(upstream["tool_choice"], json!({"type":"function","name":"lookup_weather"}));
        assert_eq!(upstream["stream"], true);
    }

    #[tokio::test]
    async fn prepare_gateway_request_anthropic_messages_falls_back_non_gpt_model_to_latest_gpt() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"claude-sonnet-4-6",
                "messages":[{"role":"user","content":"hello"}]
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/messages",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("non-gpt anthropic model should fall back locally");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");

        assert_eq!(prepared.model.as_deref(), Some("gpt-5.5"));
        assert_eq!(prepared.client_visible_model, None);
        assert_eq!(upstream["model"], json!("gpt-5.5"));
    }

    #[tokio::test]
    async fn prepare_gateway_request_anthropic_messages_maps_enabled_thinking_budget_to_xhigh() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.5",
                "thinking":{"type":"enabled","budget_tokens":24576},
                "messages":[{"role":"user","content":"hello"}]
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/messages",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("enabled thinking request should normalize");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");

        assert_eq!(upstream["reasoning"]["effort"], "xhigh");
    }

    #[tokio::test]
    async fn prepare_gateway_request_anthropic_messages_normalizes_tool_input_schema() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.5",
                "tools":[{
                    "name":"inspect_file",
                    "input_schema":{
                        "type":"object",
                        "properties":{
                            "payload":{
                                "type":"object"
                            }
                        }
                    }
                }],
                "messages":[{"role":"user","content":"hello"}]
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/messages",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("tool schema request should normalize");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");

        assert_eq!(
            upstream["tools"][0]["parameters"]["properties"]["payload"]["properties"],
            json!({})
        );
    }

    #[tokio::test]
    async fn prepare_gateway_request_compact_keeps_native_body_without_local_normalization() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "input":"hello compact",
                "tools":[{"type":"web_search"}],
                "parallel_tool_calls":true,
                "reasoning":{"effort":"high","summary":"auto"},
                "text":{"verbosity":"low"}
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/responses/compact",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("compact request should pass through");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");

        assert_eq!(upstream["input"], json!("hello compact"));
        assert_eq!(upstream["tools"], json!([{ "type": "web_search" }]));
        assert_eq!(upstream["parallel_tool_calls"], json!(true));
        assert_eq!(upstream["reasoning"], json!({"effort":"high","summary":"auto"}));
        assert_eq!(upstream["text"], json!({"verbosity":"low"}));
        assert_eq!(upstream["instructions"].as_str(), Some(codex_default_instructions()));
        assert!(
            upstream.get("stream").is_none(),
            "compact requests should not inject stream control"
        );
    }

    #[tokio::test]
    async fn prepare_gateway_request_compact_preserves_native_fields_and_history() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "previous_response_id":"resp_compact_1",
                "input":[
                    {
                        "type":"function_call",
                        "call_id":"call_ns_1",
                        "name":"find_gameobjects",
                        "namespace":"game_tools",
                        "arguments":"{\"scene\":\"main\"}"
                    },
                    {
                        "type":"function_call_output",
                        "call_id":"call_ns_1",
                        "output":"[]"
                    }
                ],
                "tools":[{"type":"web_search"}],
                "parallel_tool_calls":true,
                "reasoning":{"effort":"high","summary":"auto"},
                "text":{"verbosity":"low"},
                "max_output_tokens":64,
                "store":true,
                "include":["reasoning.encrypted_content"],
                "client_metadata":{"source":"test"},
                "tool_choice":"required"
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/responses/compact",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("compact request should pass through");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");

        assert!(upstream.get("previous_response_id").is_none());
        assert_eq!(upstream["input"][0]["namespace"], json!("game_tools"));
        assert_eq!(upstream["tools"], json!([{ "type": "web_search" }]));
        assert_eq!(upstream["parallel_tool_calls"], json!(true));
        assert_eq!(upstream["reasoning"], json!({"effort":"high","summary":"auto"}));
        assert_eq!(upstream["text"], json!({"verbosity":"low"}));
        assert!(upstream.get("max_output_tokens").is_none());
        assert_eq!(upstream["instructions"].as_str(), Some(codex_default_instructions()));
        assert!(upstream.get("store").is_none());
        assert!(upstream.get("include").is_none());
        assert!(upstream.get("client_metadata").is_none());
        assert!(upstream.get("tool_choice").is_none());
    }

    #[tokio::test]
    async fn prepare_gateway_request_rewrites_array_style_local_schema_refs() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r##"{
                "model":"gpt-5.3-codex",
                "messages":[{"role":"user","content":"hello"}],
                "tools":[{
                    "type":"function",
                    "function":{
                        "name":"ai-game-developer_assets-create-folder",
                        "parameters":{
                            "type":"object",
                            "properties":{
                                "folders":{"$ref":"#/$defs/game_tools.CreateFolderInput[]"}
                            },
                            "$defs":{
                                "game_tools.CreateFolderInput":{
                                    "type":"object",
                                    "properties":{"path":{"type":"string"}}
                                }
                            }
                        }
                    }
                }]
            }"##,
        );

        let prepared = prepare_gateway_request(
            "/v1/chat/completions",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("array-style local refs should normalize");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");
        assert_eq!(
            upstream["tools"][0]["parameters"]["properties"]["folders"],
            json!({
                "type":"array",
                "items":{"$ref":"#/$defs/game_tools.CreateFolderInput"}
            })
        );
    }

    #[tokio::test]
    async fn prepare_gateway_request_accepts_memories_trace_summarize_without_responses_defaults() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(r#"{"model":"gpt-5.3-codex","raw_memories":["alpha"]}"#);

        let prepared = prepare_gateway_request(
            "/v1/memories/trace_summarize",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("memory summarize request should pass through");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");

        assert_eq!(prepared.upstream_path, "/v1/memories/trace_summarize");
        assert_eq!(upstream["raw_memories"], json!(["alpha"]));
        assert!(upstream.get("instructions").is_none());
        assert!(upstream.get("tools").is_none());
        assert!(upstream.get("stream").is_none());
    }

    #[tokio::test]
    async fn prepare_gateway_request_accepts_file_finalize_without_responses_defaults() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(r#"{}"#);

        let prepared = prepare_gateway_request(
            "/v1/files/file_abc123/uploaded",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("file finalize request should pass through");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");

        assert_eq!(prepared.upstream_path, "/v1/files/file_abc123/uploaded");
        assert_eq!(upstream, json!({}));
        assert!(upstream.get("stream").is_none());
    }

    #[tokio::test]
    async fn prepare_gateway_request_accepts_file_create_without_responses_defaults() {
        let headers = axum::http::HeaderMap::new();
        let body =
            Body::from(r#"{"file_name":"patch.txt","file_size":42,"use_case":"assistants"}"#);

        let prepared = prepare_gateway_request(
            "/v1/files",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("file create request should pass through");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");

        assert_eq!(prepared.upstream_path, "/v1/files");
        assert_eq!(upstream["file_name"], "patch.txt");
        assert_eq!(upstream["file_size"], 42);
        assert!(upstream.get("stream").is_none());
    }

    #[tokio::test]
    async fn prepare_gateway_request_rejects_nested_file_finalize_path() {
        let headers = axum::http::HeaderMap::new();
        let err = prepare_gateway_request(
            "/v1/files/a/b/uploaded",
            "",
            axum::http::Method::POST,
            &headers,
            Body::from(r#"{}"#),
            1024 * 1024,
        )
        .await
        .expect_err("nested file ids should not match the Codex finalize path");

        assert_eq!(err.status, StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn prepare_gateway_request_accepts_realtime_sdp_without_json_parsing() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/sdp"));
        let sdp = "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\n";

        let prepared = prepare_gateway_request(
            "/v1/realtime/calls",
            "",
            axum::http::Method::POST,
            &headers,
            Body::from(sdp),
            1024 * 1024,
        )
        .await
        .expect("realtime SDP request should pass through");

        assert_eq!(prepared.upstream_path, "/v1/realtime/calls");
        assert_eq!(prepared.content_type, "application/sdp");
        assert_eq!(prepared.model, None);
        assert_eq!(prepared.request_body.as_ref(), sdp.as_bytes());
    }

    #[tokio::test]
    async fn prepare_gateway_request_decodes_zstd_json_body_before_preserving_native_responses() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static("zstd"));
        let compressed = zstd::stream::encode_all(
            Cursor::new(br#"{"model":"gpt-5.3-codex","input":"compressed hello"}"#),
            3,
        )
        .expect("compress request body");

        let prepared = prepare_gateway_request(
            "/v1/responses",
            "",
            axum::http::Method::POST,
            &headers,
            Body::from(compressed),
            1024 * 1024,
        )
        .await
        .expect("compressed responses request should pass through");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");

        assert_eq!(upstream["input"][0]["type"], json!("message"));
        assert_eq!(upstream["input"][0]["content"][0]["text"], json!("compressed hello"));
        assert_eq!(upstream["stream"], json!(true));
        assert!(prepared.client_request_body.is_none());
        assert_eq!(prepared.last_message_content.as_deref(), Some("compressed hello"));
    }
}
