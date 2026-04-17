use std::{
    collections::{BTreeMap, BTreeSet},
    net::IpAddr,
};

use axum::{
    body::{to_bytes, Body, Bytes},
    http::{header, HeaderMap, Method},
};
use serde_json::{json, Map, Value};
use static_flow_shared::llm_gateway_store::{
    LLM_GATEWAY_KEY_STATUS_ACTIVE, LLM_GATEWAY_KEY_STATUS_DISABLED,
};

use super::{
    bad_request, bad_request_with_detail, internal_error, method_not_allowed, not_found,
    types::{
        GatewayHandlerResult, GatewayResponseAdapter, OpenAiChatAdaptedRequest,
        PreparedGatewayRequest,
    },
    FAST_BILLABLE_MULTIPLIER, GPT53_CODEX_MODEL_ID, GPT53_CODEX_SPARK_MODEL_ID,
    MAX_OPENAI_TOOL_NAME_LEN,
};

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
) -> Result<
    PreparedGatewayRequest,
    (axum::http::StatusCode, axum::response::Json<crate::handlers::ErrorResponse>),
> {
    let body = read_gateway_request_body(body, max_request_body_bytes).await?;
    prepare_gateway_request_from_bytes(gateway_path, query, method, headers, body)
}

pub(crate) async fn read_gateway_request_body(
    body: Body,
    max_request_body_bytes: usize,
) -> Result<Bytes, (axum::http::StatusCode, axum::response::Json<crate::handlers::ErrorResponse>)> {
    to_bytes(body, max_request_body_bytes)
        .await
        .map_err(|err| internal_error("Failed to read llm gateway request body", err))
}

pub(crate) fn prepare_gateway_request_from_bytes(
    gateway_path: &str,
    query: &str,
    method: Method,
    headers: &HeaderMap,
    body: Bytes,
) -> Result<
    PreparedGatewayRequest,
    (axum::http::StatusCode, axum::response::Json<crate::handlers::ErrorResponse>),
> {
    let allows_get = is_models_path(gateway_path);
    let allows_post =
        gateway_path == "/v1/chat/completions" || gateway_path.starts_with("/v1/responses");
    let method_allowed =
        (method == Method::GET && allows_get) || (method == Method::POST && allows_post);
    if !method_allowed {
        return Err(method_not_allowed(
            "Unsupported method for the requested llm gateway endpoint",
        ));
    }
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("application/json")
        .to_string();
    let client_request_body = body.clone();
    let mut json_value = if content_type.starts_with("application/json") && !body.is_empty() {
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
    let billable_multiplier =
        if gateway_path == "/v1/chat/completions" || gateway_path == "/v1/responses" {
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
            normalize_responses_request(gateway_path, root, thread_anchor.as_deref());
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

    Ok(PreparedGatewayRequest {
        original_path,
        upstream_path,
        method,
        client_request_body,
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
    })
}

pub(crate) fn apply_gpt53_codex_spark_mapping(
    prepared: &PreparedGatewayRequest,
    enabled: bool,
) -> GatewayHandlerResult<PreparedGatewayRequest> {
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

/// Extract the last text-like message content from the request body.
///
/// This intentionally parses request-format structures (`messages` for chat
/// requests, `input` for responses requests) instead of storing the whole
/// body. Unsupported shapes return `Ok(None)` so the main request flow is not
/// blocked; malformed JSON returns `Err(...)` and the caller can record a
/// failure marker if desired.
pub(crate) fn extract_last_message_content(body: &Bytes) -> Result<Option<String>, String> {
    if body.is_empty() {
        return Ok(None);
    }
    let value: Value = serde_json::from_slice(body)
        .map_err(|err| format!("failed to parse request body: {err}"))?;
    let Some(root) = value.as_object() else {
        return Err("request body is not a JSON object".to_string());
    };

    if let Some(messages) = root.get("messages").and_then(Value::as_array) {
        return Ok(extract_last_message_from_chat_messages(messages));
    }
    if let Some(input) = root.get("input") {
        return Ok(extract_last_text_from_responses_input(input));
    }
    Ok(None)
}

/// Convert request-level service tier hints into a billing multiplier.
pub(crate) fn resolve_billable_multiplier(json_value: Option<&Value>) -> u64 {
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
pub(crate) fn extract_header_value(headers: &HeaderMap, name: &str) -> Option<String> {
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
pub(crate) fn serialize_headers_json(headers: &HeaderMap) -> String {
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
pub(crate) fn resolve_request_url_from_headers(
    headers: &HeaderMap,
    uri: &axum::http::Uri,
) -> String {
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
pub(crate) fn extract_client_ip_from_headers(headers: &HeaderMap) -> String {
    parse_first_ip_from_header(headers.get("x-forwarded-for"))
        .or_else(|| parse_first_ip_from_header(headers.get("x-real-ip")))
        .or_else(|| parse_first_ip_from_header(headers.get("cf-connecting-ip")))
        .or_else(|| parse_first_ip_from_header(headers.get("x-client-ip")))
        .or_else(|| parse_ip_from_forwarded_header(headers.get("forwarded")))
        .unwrap_or_else(|| "unknown".to_string())
}

/// Parse the first IP candidate from a comma-delimited proxy header.
fn parse_first_ip_from_header(value: Option<&axum::http::HeaderValue>) -> Option<String> {
    let raw = value?.to_str().ok()?;
    raw.split(',').find_map(normalize_ip_token)
}

/// Parse the RFC 7239 `Forwarded` header and extract the first usable IP.
fn parse_ip_from_forwarded_header(value: Option<&axum::http::HeaderValue>) -> Option<String> {
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
pub(crate) fn extract_non_empty_string(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
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
pub(crate) fn normalize_upstream_base_url(base: &str) -> String {
    let mut normalized = base.trim().trim_end_matches('/').to_string();
    let lower = normalized.to_ascii_lowercase();
    if (lower.starts_with("https://chatgpt.com") || lower.starts_with("https://chat.openai.com"))
        && !lower.contains("/backend-api")
    {
        normalized = format!("{normalized}/backend-api/codex");
    }
    normalized
}

/// Collapse user-provided reasoning-effort aliases into supported values.
fn normalize_reasoning_effort(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "low" => Some("low"),
        "medium" => Some("medium"),
        "high" => Some("high"),
        "xhigh" | "extra_high" => Some("xhigh"),
        _ => None,
    }
}

/// Map OpenAI chat roles into the role set accepted by the responses API.
fn normalize_openai_role_for_responses(role: &str) -> Option<&'static str> {
    match role {
        "system" | "developer" => Some("system"),
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
pub(crate) fn restore_openai_tool_name(
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

/// Collect every function/tool name referenced anywhere in the request.
fn collect_openai_tool_names(obj: &Map<String, Value>) -> Vec<String> {
    let mut names = Vec::new();

    if let Some(tools) = obj.get("tools").and_then(Value::as_array) {
        for tool in tools {
            let Some(tool_obj) = tool.as_object() else {
                continue;
            };
            let tool_type = tool_obj
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !tool_type.is_empty() && tool_type != "function" {
                continue;
            }
            let name = tool_obj
                .get("function")
                .and_then(|function| function.get("name"))
                .or_else(|| tool_obj.get("name"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            if let Some(name) = name {
                names.push(name.to_string());
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
                .unwrap_or_default();
            if tool_type != "function" {
                return None;
            }
            tool_choice
                .get("function")
                .and_then(|function| function.get("name"))
                .or_else(|| tool_choice.get("name"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
    {
        names.push(name.to_string());
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

fn chat_message_bad_request(
    index: usize,
    message: impl AsRef<str>,
) -> (axum::http::StatusCode, axum::response::Json<crate::handlers::ErrorResponse>) {
    bad_request(&format!("chat.completions message {index}: {}", message.as_ref()))
}

/// Adapt an OpenAI chat/completions request into the upstream responses format.
fn adapt_openai_chat_completions_request(
    obj: &Map<String, Value>,
) -> GatewayHandlerResult<OpenAiChatAdaptedRequest> {
    let source_messages = obj
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| bad_request("chat.completions messages field is required"))?;
    let tool_name_map = build_openai_tool_name_map(obj);
    let tool_name_restore_map = build_openai_tool_name_restore_map(&tool_name_map);

    let mut instructions_parts = Vec::new();
    let mut input_items = Vec::<Value>::new();

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
            "system" => {
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
                if text.trim().is_empty() {
                    return Err(chat_message_bad_request(index, "content is empty or unsupported"));
                }
                instructions_parts.push(text);
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
                    return Err(chat_message_bad_request(index, "content is empty or unsupported"));
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
                    for (index, tool_call) in tool_calls.iter().enumerate() {
                        let Some(tool_obj) = tool_call.as_object() else {
                            return Err(bad_request(&format!(
                                "chat.completions assistant tool_call {index} must be an object"
                            )));
                        };
                        let call_id = tool_obj
                            .get("id")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .unwrap_or_else(|| format!("call_{index}"));
                        let Some(function_name) = tool_obj
                            .get("function")
                            .and_then(|value| value.get("name"))
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                        else {
                            return Err(bad_request(&format!(
                                "chat.completions assistant tool_call {index} is missing \
                                 function.name"
                            )));
                        };
                        let function_name =
                            shorten_openai_tool_name_with_map(function_name, &tool_name_map);
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
                        emitted_tool_call = true;
                    }
                }
                if !emitted_content && !emitted_tool_call {
                    return Err(chat_message_bad_request(index, "content is empty or unsupported"));
                }
            },
            "tool" => {
                let call_id = message_obj
                    .get("tool_call_id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| bad_request("tool role message missing tool_call_id"))?;
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
    out.insert("instructions".to_string(), Value::String(instructions_parts.join("\n\n")));
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
        let mapped_tools = tools
            .iter()
            .filter_map(|tool| {
                let tool_obj = tool.as_object()?;
                let tool_type = tool_obj
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if tool_type != "function" {
                    return Some(tool.clone());
                }
                let function = tool_obj.get("function").and_then(Value::as_object)?;
                let name = function
                    .get("name")
                    .and_then(Value::as_str)
                    .map(|name| shorten_openai_tool_name_with_map(name, &tool_name_map))?;
                let mut mapped = Map::new();
                mapped.insert("type".to_string(), Value::String("function".to_string()));
                mapped.insert("name".to_string(), Value::String(name));
                if let Some(description) = function.get("description") {
                    mapped.insert("description".to_string(), description.clone());
                }
                if let Some(parameters) = function.get("parameters") {
                    mapped.insert("parameters".to_string(), parameters.clone());
                }
                if let Some(strict) = function.get("strict") {
                    mapped.insert("strict".to_string(), strict.clone());
                }
                Some(Value::Object(mapped))
            })
            .collect::<Vec<_>>();
        if !mapped_tools.is_empty() {
            out.insert("tools".to_string(), Value::Array(mapped_tools));
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
                .unwrap_or_default();
            if tool_type == "function" {
                if let Some(name) = tool_choice_obj
                    .get("function")
                    .and_then(|function| function.get("name"))
                    .or_else(|| tool_choice_obj.get("name"))
                    .and_then(Value::as_str)
                {
                    out.insert(
                        "tool_choice".to_string(),
                        json!({
                            "type": "function",
                            "name": shorten_openai_tool_name_with_map(name, &tool_name_map),
                        }),
                    );
                }
            } else {
                out.insert("tool_choice".to_string(), tool_choice.clone());
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
pub(crate) fn normalize_responses_request(
    path: &str,
    root: &mut serde_json::Map<String, Value>,
    thread_anchor: Option<&str>,
) {
    if root
        .get("instructions")
        .and_then(Value::as_str)
        .is_some_and(|value| value.trim().is_empty())
        || matches!(root.get("instructions"), Some(Value::Null))
    {
        root.remove("instructions");
    }
    if path == "/v1/responses" {
        root.insert("store".to_string(), Value::Bool(false));
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

    if path == "/v1/responses" {
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

/// Reject unsupported public gateway paths before any auth or upstream work
/// begins.
pub(crate) fn ensure_supported_gateway_path(
    path: &str,
) -> Result<(), (axum::http::StatusCode, axum::response::Json<crate::handlers::ErrorResponse>)> {
    if matches!(path, "/v1/responses" | "/v1/responses/compact" | "/v1/chat/completions")
        || is_models_path(path)
    {
        Ok(())
    } else {
        Err(not_found("Unsupported llm gateway endpoint"))
    }
}

/// Extract the presented API key from Authorization or x-api-key headers.
pub(crate) fn extract_presented_key(headers: &HeaderMap) -> Option<String> {
    if let Some(value) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(value.to_string());
    }
    headers
        .get("x-api-key")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

/// Reconstruct the externally visible origin from reverse-proxy headers.
pub(crate) fn external_origin(headers: &HeaderMap) -> Option<String> {
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

/// Read one query parameter from a raw query string.
pub(crate) fn extract_query_param(query: &str, key: &str) -> Option<String> {
    let raw = query.strip_prefix('?').unwrap_or(query);
    url::form_urlencoded::parse(raw.as_bytes())
        .find(|(candidate, _)| candidate == key)
        .map(|(_, value)| value.into_owned())
}

/// Return whether the path targets the supported `/v1/models` endpoint.
pub(crate) fn is_models_path(path: &str) -> bool {
    path == "/v1/models" || path.starts_with("/v1/models?")
}

/// Validate and normalize a human-facing key name.
pub(crate) fn normalize_name(
    raw: &str,
) -> Result<String, (axum::http::StatusCode, axum::response::Json<crate::handlers::ErrorResponse>)>
{
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(bad_request("name is required"));
    }
    Ok(trimmed.to_string())
}

/// Validate the small set of supported key status values.
pub(crate) fn normalize_status(
    raw: &str,
) -> Result<String, (axum::http::StatusCode, axum::response::Json<crate::handlers::ErrorResponse>)>
{
    let trimmed = raw.trim();
    match trimmed {
        LLM_GATEWAY_KEY_STATUS_ACTIVE | LLM_GATEWAY_KEY_STATUS_DISABLED => Ok(trimmed.to_string()),
        _ => Err(bad_request("status must be `active` or `disabled`")),
    }
}

#[cfg(test)]
mod tests {
    use axum::{body::Body, http::StatusCode};
    use serde_json::json;

    use super::{adapt_openai_chat_completions_request, prepare_gateway_request};

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

        let err = adapt_openai_chat_completions_request(obj.as_object().unwrap())
            .expect_err("request without message role should be rejected");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(err.1 .0.error.contains("role"));
    }

    #[test]
    fn adapt_openai_chat_completions_request_rejects_user_message_without_supported_content() {
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

        let err = adapt_openai_chat_completions_request(obj.as_object().unwrap())
            .expect_err("user message without supported content should be rejected");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(err.1 .0.error.contains("content"));
    }

    #[tokio::test]
    async fn prepare_gateway_request_preserves_raw_body_on_chat_validation_error() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{"model":"gpt-5.3-codex","messages":[{"role":"user","content":[{"type":"image_url"}]}]}"#,
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
        .expect_err("unsupported chat payload should fail");

        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(err.1 .0.error.contains("content"));
    }

    #[tokio::test]
    async fn prepare_gateway_request_keeps_raw_client_body_and_normalized_upstream_body() {
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
        .expect("responses request should normalize");

        let raw: serde_json::Value =
            serde_json::from_slice(&prepared.client_request_body).expect("raw body json");
        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");

        assert_eq!(raw["input"], "hello");
        assert_eq!(upstream["input"][0]["type"], "message");
        assert_eq!(upstream["input"][0]["role"], "user");
        assert_eq!(upstream["input"][0]["content"][0]["type"], "input_text");
        assert_eq!(upstream["input"][0]["content"][0]["text"], "hello");
        assert_eq!(upstream["stream"], true);
    }

    #[tokio::test]
    async fn prepare_gateway_request_compact_preserves_remote_compact_parameters() {
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
        .expect("compact request should normalize");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");

        assert_eq!(upstream["input"][0]["type"], "message");
        assert_eq!(upstream["input"][0]["role"], "user");
        assert_eq!(upstream["input"][0]["content"][0]["type"], "input_text");
        assert_eq!(upstream["input"][0]["content"][0]["text"], "hello compact");
        assert_eq!(upstream["tools"], json!([{ "type": "web_search" }]));
        assert_eq!(upstream["parallel_tool_calls"], json!(true));
        assert_eq!(upstream["reasoning"], json!({"effort":"high","summary":"auto"}));
        assert_eq!(upstream["text"], json!({"verbosity":"low"}));
        assert!(
            upstream.get("instructions").is_none(),
            "compact requests should omit empty instructions to match current codex payloads"
        );
        assert!(
            upstream.get("stream").is_none(),
            "compact requests should not inject stream control"
        );
    }
}
