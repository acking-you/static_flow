//! Codex upstream URL/version/session-header resolution + encrypted-reasoning
//! retry.


use axum::{
    body::Bytes,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use llm_access_codex::types::PreparedGatewayRequest;
use llm_access_core::{provider::ProtocolFamily, store::AdminConfigStore};
use serde_json::Value;

use super::{
    errors::extract_error_message_from_json_value, CodexAuthSnapshot, CodexDispatchRuntimeConfig,
    CodexTurnMetadataHeader, CodexUpstreamSessionHeaders, DEFAULT_WIRE_ORIGINATOR,
    MAX_CODEX_CLIENT_VERSION_LEN,
};

pub fn normalized_codex_gateway_path(path: &str) -> Option<&str> {
    if matches!(path, "/v1/models" | "/v1/messages") {
        return Some(path);
    }
    if path == "/v1/chat/completions"
        || path == "/v1/responses"
        || path.starts_with("/v1/responses/")
    {
        return Some(path);
    }
    let alias = path
        .strip_prefix("/api/llm-gateway")
        .or_else(|| path.strip_prefix("/api/codex-gateway"))?;
    match alias {
        "/models" | "/v1/models" => Some("/v1/models"),
        "/chat/completions" | "/v1/chat/completions" => Some("/v1/chat/completions"),
        "/responses" | "/v1/responses" => Some("/v1/responses"),
        "/responses/compact" | "/v1/responses/compact" => Some("/v1/responses/compact"),
        "/messages" | "/v1/messages" => Some("/v1/messages"),
        value if value.starts_with("/v1/responses/") => Some(value),
        _ => None,
    }
}
pub fn codex_protocol_family_for_endpoint(endpoint: &str) -> ProtocolFamily {
    if endpoint == "/v1/messages" || endpoint.starts_with("/v1/messages?") {
        ProtocolFamily::Anthropic
    } else {
        ProtocolFamily::OpenAi
    }
}
pub(crate) fn codex_upstream_base_url() -> String {
    std::env::var("CODEX_UPSTREAM_BASE_URL")
        .or_else(|_| std::env::var("STATICFLOW_LLM_GATEWAY_UPSTREAM_BASE_URL"))
        .map(|value| llm_access_codex::request::normalize_upstream_base_url(&value))
        .unwrap_or_else(|_| "https://chatgpt.com/backend-api/codex".to_string())
}
pub(crate) fn compute_codex_upstream_url(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    if base.contains("/backend-api/codex") && path.starts_with("/v1/") {
        format!("{}{}", base, path.trim_start_matches("/v1"))
    } else if base.ends_with("/v1") && path.starts_with("/v1") {
        format!("{}{}", base.trim_end_matches("/v1"), path)
    } else {
        format!("{base}{path}")
    }
}
pub fn normalize_codex_client_version(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_CODEX_CLIENT_VERSION_LEN {
        return None;
    }
    if !trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_'))
    {
        return None;
    }
    Some(trimmed.to_string())
}
pub(crate) fn resolve_codex_client_version(raw: Option<&str>) -> String {
    raw.and_then(normalize_codex_client_version)
        .unwrap_or_else(|| llm_access_core::store::DEFAULT_CODEX_CLIENT_VERSION.to_string())
}
fn resolve_codex_account_attempt_limit(raw: u64) -> usize {
    usize::try_from(raw).unwrap_or(usize::MAX).max(1)
}
pub async fn load_codex_dispatch_runtime_config(
    admin_config_store: &dyn AdminConfigStore,
) -> Result<CodexDispatchRuntimeConfig, Response> {
    match admin_config_store.get_admin_runtime_config().await {
        Ok(config) => Ok(CodexDispatchRuntimeConfig {
            client_version: resolve_codex_client_version(Some(&config.codex_client_version)),
            account_attempt_limit: resolve_codex_account_attempt_limit(
                config.account_failure_retry_limit,
            ),
        }),
        Err(_) => {
            Err((StatusCode::INTERNAL_SERVER_ERROR, "runtime config store error").into_response())
        },
    }
}
pub fn codex_user_agent(client_version: &str) -> String {
    format!("{DEFAULT_WIRE_ORIGINATOR}/{client_version}")
}
pub fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}
fn first_header_value(headers: &HeaderMap, names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| header_value(headers, name))
}
fn parse_codex_turn_metadata_header(headers: &HeaderMap) -> CodexTurnMetadataHeader {
    let Some(raw) = header_value(headers, "x-codex-turn-metadata") else {
        return CodexTurnMetadataHeader::default();
    };
    let Ok(value) = serde_json::from_str::<Value>(&raw) else {
        return CodexTurnMetadataHeader::default();
    };
    CodexTurnMetadataHeader {
        session_id: json_string_field(&value, "session_id"),
        thread_id: json_string_field(&value, "thread_id"),
    }
}
fn json_string_field(value: &Value, name: &str) -> Option<String> {
    value
        .get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}
fn is_standard_codex_responses_path(prepared: &PreparedGatewayRequest) -> bool {
    prepared
        .upstream_path
        .split('?')
        .next()
        .is_some_and(|path| path == "/v1/responses")
}
fn resolve_codex_upstream_session_headers(
    request_headers: &HeaderMap,
    prepared: &PreparedGatewayRequest,
) -> CodexUpstreamSessionHeaders {
    let metadata = parse_codex_turn_metadata_header(request_headers);
    let thread_anchor = prepared
        .thread_anchor
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let should_reconstruct = is_standard_codex_responses_path(prepared);
    let session_id =
        first_header_value(request_headers, &["session_id", "session-id"]).or_else(|| {
            if should_reconstruct {
                metadata
                    .session_id
                    .clone()
                    .or_else(|| thread_anchor.map(ToString::to_string))
            } else {
                None
            }
        });
    let thread_id =
        first_header_value(request_headers, &["thread_id", "thread-id"]).or_else(|| {
            if should_reconstruct {
                metadata
                    .thread_id
                    .clone()
                    .or_else(|| thread_anchor.map(ToString::to_string))
                    .or_else(|| session_id.clone())
            } else {
                None
            }
        });
    let conversation_id = header_value(request_headers, "conversation_id").or_else(|| {
        if should_reconstruct {
            thread_anchor
                .map(ToString::to_string)
                .or_else(|| metadata.thread_id.clone())
        } else {
            None
        }
    });
    let client_request_id = header_value(request_headers, "x-client-request-id").or_else(|| {
        if should_reconstruct {
            thread_id
                .clone()
                .or_else(|| thread_anchor.map(ToString::to_string))
        } else {
            None
        }
    });

    CodexUpstreamSessionHeaders {
        conversation_id,
        session_id,
        thread_id,
        client_request_id,
    }
}
pub fn is_codex_invalid_encrypted_content_response(status: StatusCode, bytes: &Bytes) -> bool {
    if status != StatusCode::BAD_REQUEST {
        return false;
    }
    if codex_error_code_from_bytes(bytes).as_deref() == Some("invalid_encrypted_content") {
        return true;
    }
    std::str::from_utf8(bytes.as_ref())
        .map(|body| body.contains("invalid_encrypted_content"))
        .unwrap_or(false)
}
pub fn is_codex_non_retryable_client_error_response(status: StatusCode, bytes: &Bytes) -> bool {
    if status != StatusCode::BAD_REQUEST
        || is_codex_invalid_encrypted_content_response(status, bytes)
    {
        return false;
    }

    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
        return false;
    };
    let error = value.get("error").unwrap_or(&value);
    if json_string_field(error, "code")
        .as_deref()
        .is_some_and(codex_error_code_is_request_shape_failure)
    {
        return true;
    }

    extract_error_message_from_json_value(&value)
        .as_deref()
        .is_some_and(codex_message_indicates_request_shape_failure)
}
fn codex_error_code_is_request_shape_failure(code: &str) -> bool {
    matches!(
        code,
        "invalid_value"
            | "unsupported_value"
            | "invalid_type"
            | "missing_required_parameter"
            | "unknown_parameter"
            | "unsupported_parameter"
    )
}
fn codex_message_indicates_request_shape_failure(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    (normalized.contains("invalid value") && normalized.contains("supported values"))
        || normalized.contains("invalid type")
        || normalized.contains("missing required parameter")
        || normalized.contains("unknown parameter")
        || normalized.contains("unsupported parameter")
}
fn codex_error_code_from_bytes(bytes: &Bytes) -> Option<String> {
    serde_json::from_slice::<Value>(bytes)
        .ok()
        .and_then(|value| codex_error_code_from_value(&value))
}
fn codex_error_code_from_value(value: &Value) -> Option<String> {
    let error = value.get("error").unwrap_or(value);
    if let Some(code) = json_string_field(error, "code") {
        return Some(code);
    }
    let message = json_string_field(error, "message")?;
    serde_json::from_str::<Value>(&message)
        .ok()
        .and_then(|nested| codex_error_code_from_value(&nested))
}
pub fn retry_codex_without_encrypted_reasoning(
    prepared: &PreparedGatewayRequest,
) -> Option<PreparedGatewayRequest> {
    let mut value = serde_json::from_slice::<Value>(&prepared.request_body).ok()?;
    let root = value.as_object_mut()?;
    if !strip_codex_encrypted_reasoning_items(root) {
        return None;
    }
    let request_body = Bytes::from(serde_json::to_vec(&value).ok()?);
    let mut retry = prepared.clone();
    retry.request_body = request_body;
    Some(retry)
}
fn strip_codex_encrypted_reasoning_items(root: &mut serde_json::Map<String, Value>) -> bool {
    let Some(input) = root.get_mut("input") else {
        return false;
    };
    let mut remove_input = false;
    let changed = match input {
        Value::Array(items) => {
            let mut changed = false;
            let mut filtered = Vec::with_capacity(items.len());
            for mut item in std::mem::take(items) {
                let keep = sanitize_codex_encrypted_reasoning_item(&mut item, &mut changed);
                if keep {
                    filtered.push(item);
                }
            }
            if changed {
                if filtered.is_empty() {
                    remove_input = true;
                } else {
                    *items = filtered;
                }
            }
            changed
        },
        Value::Object(_) => {
            let mut changed = false;
            let keep = sanitize_codex_encrypted_reasoning_item(input, &mut changed);
            if changed && !keep {
                remove_input = true;
            }
            changed
        },
        _ => false,
    };
    if remove_input {
        root.remove("input");
    }
    changed
}
fn sanitize_codex_encrypted_reasoning_item(item: &mut Value, changed: &mut bool) -> bool {
    let Some(obj) = item.as_object_mut() else {
        return true;
    };
    if obj.get("type").and_then(Value::as_str) != Some("reasoning") {
        return true;
    }
    if obj.remove("encrypted_content").is_none() {
        return true;
    }
    *changed = true;
    obj.len() > 1
}
pub fn add_codex_upstream_headers(
    mut upstream: reqwest::RequestBuilder,
    request_headers: &HeaderMap,
    prepared: &PreparedGatewayRequest,
    auth: &CodexAuthSnapshot,
    codex_client_version: &str,
) -> reqwest::RequestBuilder {
    let session_headers = resolve_codex_upstream_session_headers(request_headers, prepared);
    let incoming_turn_state = header_value(request_headers, "x-codex-turn-state");

    upstream = upstream
        .bearer_auth(&auth.access_token)
        .header(
            reqwest::header::ACCEPT,
            if prepared.wants_stream || prepared.force_upstream_stream {
                "text/event-stream"
            } else {
                "application/json"
            },
        )
        .header(
            reqwest::header::USER_AGENT,
            header_value(request_headers, header::USER_AGENT.as_str())
                .unwrap_or_else(|| codex_user_agent(codex_client_version)),
        )
        .header(
            reqwest::header::HeaderName::from_static("originator"),
            header_value(request_headers, "originator")
                .unwrap_or_else(|| DEFAULT_WIRE_ORIGINATOR.to_string()),
        );
    if !prepared.request_body.is_empty() {
        upstream = upstream
            .header(reqwest::header::CONTENT_TYPE, prepared.content_type.as_str())
            .body(prepared.request_body.clone());
    }
    if let Some(conversation_id) = session_headers.conversation_id.as_deref() {
        upstream = upstream.header("conversation_id", conversation_id);
    }
    if let Some(client_request_id) = session_headers.client_request_id.as_deref() {
        upstream = upstream.header("x-client-request-id", client_request_id);
    }
    if let Some(turn_state) = incoming_turn_state.as_deref() {
        upstream = upstream.header("x-codex-turn-state", turn_state);
    }
    for header_name in [
        "openai-beta",
        "x-openai-subagent",
        "x-codex-beta-features",
        "x-codex-turn-metadata",
        "x-codex-installation-id",
        "x-codex-parent-thread-id",
        "x-codex-window-id",
        "x-openai-memgen-request",
        "x-responsesapi-include-timing-metrics",
        "traceparent",
        "tracestate",
        "baggage",
    ] {
        if let Some(value) = header_value(request_headers, header_name) {
            upstream = upstream.header(header_name, value);
        }
    }
    if let Some(session_id) = session_headers.session_id.as_deref() {
        upstream = upstream
            .header("session_id", session_id)
            .header("session-id", session_id);
    }
    if let Some(thread_id) = session_headers.thread_id.as_deref() {
        upstream = upstream
            .header("thread_id", thread_id)
            .header("thread-id", thread_id);
    }
    if let Some(account_id) = auth.account_id.as_deref() {
        upstream = upstream.header("chatgpt-account-id", account_id);
    }
    if auth.is_fedramp_account {
        upstream = upstream.header("x-openai-fedramp", "true");
    }
    upstream
}
