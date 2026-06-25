//! Codex upstream error-envelope classification.

use std::time::Duration;

use axum::{
    body::Bytes,
    http::{header, HeaderMap, StatusCode},
};
use serde_json::Value;

use super::errors::extract_error_message_from_json_value;

pub(crate) const CODEX_RETRY_AFTER_MAX: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CodexUpstreamErrorClass {
    ContextWindowExceeded,
    QuotaExceeded,
    UsageNotIncluded,
    CyberPolicy,
    InvalidRequest,
    ServerOverloaded,
    Retryable,
    Stream,
    UnexpectedStatus,
}

impl CodexUpstreamErrorClass {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ContextWindowExceeded => "context_window_exceeded",
            Self::QuotaExceeded => "quota_exceeded",
            Self::UsageNotIncluded => "usage_not_included",
            Self::CyberPolicy => "cyber_policy",
            Self::InvalidRequest => "invalid_request",
            Self::ServerOverloaded => "server_overloaded",
            Self::Retryable => "retryable",
            Self::Stream => "stream",
            Self::UnexpectedStatus => "unexpected_status",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CodexClassifiedUpstreamError {
    pub class: CodexUpstreamErrorClass,
    pub status: StatusCode,
    pub message: String,
    pub body: Bytes,
    pub retry_after: Option<Duration>,
}

pub(crate) fn classify_codex_upstream_failure(
    status: StatusCode,
    headers: &HeaderMap,
    bytes: Bytes,
) -> CodexClassifiedUpstreamError {
    let value = serde_json::from_slice::<Value>(&bytes).ok();
    let message = value
        .as_ref()
        .and_then(extract_error_message_from_json_value)
        .map(|message| message.trim().to_string())
        .filter(|message| !message.is_empty())
        .unwrap_or_else(|| {
            let text = String::from_utf8_lossy(bytes.as_ref()).trim().to_string();
            if text.is_empty() {
                "Unknown upstream error".to_string()
            } else {
                text
            }
        });
    let class = value
        .as_ref()
        .map(|value| classify_codex_error_value(status, value, &message))
        .unwrap_or_else(|| classify_status_and_message(status, &message));
    CodexClassifiedUpstreamError {
        class,
        status,
        message,
        body: bytes,
        retry_after: retry_after(headers),
    }
}

pub(crate) fn classify_codex_success_error_body(
    status: StatusCode,
    headers: &HeaderMap,
    bytes: &Bytes,
) -> Option<CodexClassifiedUpstreamError> {
    let value = serde_json::from_slice::<Value>(bytes).ok()?;
    json_value_contains_error(&value)
        .then(|| classify_codex_upstream_failure(status, headers, bytes.clone()))
}

pub(crate) fn classify_codex_sse_event_failure(
    status: StatusCode,
    headers: &HeaderMap,
    event_type: Option<&str>,
    data: &str,
) -> Option<CodexClassifiedUpstreamError> {
    // Hot-path fast-out: `*.delta` events stream a single chunk of output text or
    // reasoning and never carry an error envelope. They dominate event volume on
    // a streaming turn (one per token), so skip the JSON parse for them entirely.
    // Real failures arrive as `error` / `response.error` / `response.failed` (or
    // an embedded `error` field) and still take the full path below.
    if event_type.is_some_and(|name| name.ends_with(".delta")) {
        return None;
    }
    if data.trim() == "[DONE]" {
        return None;
    }
    let mut value = serde_json::from_str::<Value>(data).ok()?;
    if let (Some(event_type), Some(object)) = (event_type, value.as_object_mut()) {
        object
            .entry("type")
            .or_insert_with(|| Value::String(event_type.to_string()));
    }
    let event_type = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let is_failure_event = matches!(event_type, "error" | "response.error" | "response.failed")
        || has_non_null_pointer(&value, "/response/error")
        || has_non_null_field(&value, "error");
    if !is_failure_event {
        return None;
    }
    let bytes = Bytes::from(value.to_string());
    Some(classify_codex_upstream_failure(status, headers, bytes))
}

fn classify_codex_error_value(
    status: StatusCode,
    value: &Value,
    message: &str,
) -> CodexUpstreamErrorClass {
    if any_error_code_matches(value, codex_code_is_context_window)
        || message_indicates_context_window(message)
    {
        return CodexUpstreamErrorClass::ContextWindowExceeded;
    }
    if any_error_code_matches(value, codex_code_is_usage_not_included) {
        return CodexUpstreamErrorClass::UsageNotIncluded;
    }
    if any_error_code_matches(value, codex_code_is_quota) || message_indicates_quota(message) {
        return CodexUpstreamErrorClass::QuotaExceeded;
    }
    if any_error_code_matches(value, codex_code_is_cyber_policy) {
        return CodexUpstreamErrorClass::CyberPolicy;
    }
    if any_error_code_matches(value, codex_code_is_server_overloaded)
        || message_indicates_capacity(message)
    {
        return CodexUpstreamErrorClass::ServerOverloaded;
    }
    if any_error_code_matches(value, codex_code_is_unexpected_status) {
        return CodexUpstreamErrorClass::UnexpectedStatus;
    }
    if any_error_code_matches(value, codex_code_is_invalid_request)
        || any_error_type_matches(value, codex_type_is_invalid_request)
    {
        return CodexUpstreamErrorClass::InvalidRequest;
    }
    if any_error_code_matches(value, codex_code_is_retryable)
        || any_error_type_matches(value, codex_type_is_retryable)
    {
        return CodexUpstreamErrorClass::Retryable;
    }
    classify_status_and_message(status, message)
}

fn classify_status_and_message(status: StatusCode, message: &str) -> CodexUpstreamErrorClass {
    if message_indicates_context_window(message) {
        return CodexUpstreamErrorClass::ContextWindowExceeded;
    }
    if message_indicates_quota(message) {
        return CodexUpstreamErrorClass::QuotaExceeded;
    }
    if message_indicates_capacity(message) {
        return CodexUpstreamErrorClass::ServerOverloaded;
    }
    // Request-shape failures are deterministic client errors. Return them to the
    // caller instead of failing over, which would cool every healthy account for
    // a client-side bug. Mirrors codex-rs request-shape message detection so a
    // 400 carrying only a message (no machine `code`) is still recognized.
    if message_indicates_request_shape(message) {
        return CodexUpstreamErrorClass::InvalidRequest;
    }
    if status == StatusCode::TOO_MANY_REQUESTS {
        return CodexUpstreamErrorClass::Retryable;
    }
    CodexUpstreamErrorClass::UnexpectedStatus
}

fn json_value_contains_error(value: &Value) -> bool {
    has_non_null_field(value, "error")
        || has_non_null_pointer(value, "/response/error")
        || matches!(
            value.get("type").and_then(Value::as_str),
            Some("error" | "response.error" | "response.failed")
        )
}

fn has_non_null_field(value: &Value, field: &str) -> bool {
    value.get(field).is_some_and(|value| !value.is_null())
}

fn has_non_null_pointer(value: &Value, pointer: &str) -> bool {
    value.pointer(pointer).is_some_and(|value| !value.is_null())
}

fn any_error_code_matches(value: &Value, predicate: fn(&str) -> bool) -> bool {
    string_at_any(value, &[
        "/error/code",
        "/code",
        "/response/error/code",
        "/response/incomplete_details/reason",
    ])
    .iter()
    .any(|code| predicate(code))
}

fn any_error_type_matches(value: &Value, predicate: fn(&str) -> bool) -> bool {
    string_at_any(value, &["/error/type", "/type", "/response/error/type"])
        .iter()
        .any(|value| predicate(value))
}

fn string_at_any(value: &Value, pointers: &[&str]) -> Vec<String> {
    pointers
        .iter()
        .filter_map(|pointer| value.pointer(pointer).and_then(Value::as_str))
        .flat_map(|raw| {
            let mut values = vec![raw.trim().to_string()];
            if let Ok(nested) = serde_json::from_str::<Value>(raw) {
                values.extend(string_at_any(&nested, pointers));
            }
            values
        })
        .filter(|value| !value.is_empty())
        .collect()
}

fn codex_code_is_context_window(code: &str) -> bool {
    matches!(
        normalized_code(code).as_str(),
        "context_length_exceeded"
            | "context_window_exceeded"
            | "input_too_long"
            | "max_context_length_exceeded"
    )
}

fn codex_code_is_quota(code: &str) -> bool {
    matches!(
        normalized_code(code).as_str(),
        "insufficient_quota" | "quota_exceeded" | "usage_limit_exceeded"
    )
}

fn codex_code_is_usage_not_included(code: &str) -> bool {
    normalized_code(code) == "usage_not_included"
}

fn codex_code_is_cyber_policy(code: &str) -> bool {
    matches!(
        normalized_code(code).as_str(),
        "cyber_policy" | "policy_violation" | "content_policy_violation"
    )
}

fn codex_code_is_server_overloaded(code: &str) -> bool {
    matches!(normalized_code(code).as_str(), "server_is_overloaded" | "slow_down")
}

fn codex_code_is_unexpected_status(code: &str) -> bool {
    matches!(normalized_code(code).as_str(), "bad_gateway" | "internal_server_error")
}

fn codex_code_is_invalid_request(code: &str) -> bool {
    matches!(
        normalized_code(code).as_str(),
        "invalid_prompt"
            | "invalid_request"
            | "invalid_value"
            | "unsupported_value"
            | "invalid_type"
            | "missing_required_parameter"
            | "unknown_parameter"
            | "unsupported_parameter"
            | "invalid_tool_choice"
            | "invalid_encrypted_content"
    )
}

fn codex_code_is_retryable(code: &str) -> bool {
    matches!(
        normalized_code(code).as_str(),
        "rate_limit_exceeded" | "timeout" | "temporarily_unavailable"
    )
}

fn codex_type_is_invalid_request(error_type: &str) -> bool {
    matches!(normalized_code(error_type).as_str(), "invalid_request_error")
}

fn codex_type_is_retryable(error_type: &str) -> bool {
    matches!(
        normalized_code(error_type).as_str(),
        "api_error" | "rate_limit_error" | "server_error"
    )
}

fn normalized_code(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn message_indicates_context_window(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    normalized.contains("context_length_exceeded")
        || normalized.contains("context length")
        || normalized.contains("context window")
        || normalized.contains("maximum context")
        || normalized.contains("input is too long")
}

fn message_indicates_quota(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    normalized.contains("usage limit")
        || normalized.contains("insufficient_quota")
        || normalized.contains("quota_exceeded")
        || normalized.contains("quota exceeded")
}

fn message_indicates_capacity(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    // `contains("overloaded")` also covers the structured `server_is_overloaded`
    // code text and the user-facing "Our servers are currently overloaded. Please
    // try again later." message the upstream backend returns under load.
    normalized.contains("overloaded")
        || normalized.contains("slow_down")
        || normalized.contains("high capacity")
        || normalized.contains("at capacity")
        || normalized.contains("over capacity")
}

/// Detects deterministic request-shape failures that some upstreams report with
/// only a human message (no machine `code`/`type`). Mirrors codex-rs'
/// `codex_message_indicates_request_shape_failure` so such errors are returned
/// to the caller as `InvalidRequest` rather than failing over — failing over
/// would retry every account and cool them down for a client-side bug.
fn message_indicates_request_shape(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    (normalized.contains("invalid value") && normalized.contains("supported values"))
        || normalized.contains("invalid type")
        || normalized.contains("missing required parameter")
        || normalized.contains("unknown parameter")
        || normalized.contains("unsupported parameter")
}

fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    headers
        .get(header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
        .map(|duration| duration.min(CODEX_RETRY_AFTER_MAX))
}
