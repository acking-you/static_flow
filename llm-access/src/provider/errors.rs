//! Provider error responses: Anthropic/Codex JSON error bodies,
//! request-limit/cooldown classification, and error-byte summarization.

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

pub(crate) fn capture_error_message(meta: &mut ProviderUsageMetadata, message: &str) {
    if meta.error_message.is_some() {
        return;
    }
    let trimmed = message.trim();
    if !trimmed.is_empty() {
        meta.error_message = Some(trimmed.to_string());
    }
}
pub(crate) fn capture_error_body(meta: &mut ProviderUsageMetadata, body: &str) {
    if meta.error_body.is_some() {
        return;
    }
    let trimmed = body.trim();
    if !trimmed.is_empty() {
        meta.error_body = Some(trimmed.to_string());
    }
}
pub(crate) fn capture_error_bytes(meta: &mut ProviderUsageMetadata, bytes: &Bytes) {
    capture_error_message(meta, &summarize_error_bytes(bytes));
    let body = String::from_utf8_lossy(bytes.as_ref());
    capture_error_body(meta, &body);
}
pub(crate) fn is_monthly_request_limit(body: &str) -> bool {
    body.contains("MONTHLY_REQUEST_COUNT")
        || kiro_error_reason(body).as_deref() == Some("MONTHLY_REQUEST_COUNT")
}
pub(crate) fn daily_request_limit_cooldown(body: &str) -> Option<Duration> {
    if body.contains("5-minute credit limit exceeded") {
        return Some(Duration::from_secs(5 * 60));
    }
    if kiro_error_reason(body).as_deref() == Some("DAILY_REQUEST_COUNT") {
        return Some(Duration::from_secs(5 * 60));
    }
    None
}
pub(crate) fn transient_invalid_model_cooldown(body: &str) -> Option<Duration> {
    if !body.contains("Invalid model") {
        return None;
    }
    if kiro_error_reason(body).as_deref() == Some("INVALID_MODEL_ID") {
        return Some(Duration::from_secs(60));
    }
    None
}
pub(crate) fn kiro_error_reason(body: &str) -> Option<String> {
    let value = serde_json::from_str::<serde_json::Value>(body).ok()?;
    value
        .get("reason")
        .and_then(|item| item.as_str())
        .or_else(|| {
            value
                .pointer("/error/reason")
                .and_then(|item| item.as_str())
        })
        .map(str::to_string)
}
pub(crate) fn anthropic_json_error_body(error_type: &str, message: &str) -> String {
    serde_json::json!({
        "error": {
            "type": error_type,
            "message": message,
        }
    })
    .to_string()
}
pub(crate) fn anthropic_json_error(
    status: StatusCode,
    error_type: &str,
    message: &str,
) -> Response {
    let body = anthropic_json_error_body(error_type, message);
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .unwrap_or_else(|_| {
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to build error").into_response()
        })
}
pub(crate) fn codex_error_type_for_status(status: StatusCode) -> &'static str {
    if status.is_client_error() {
        "invalid_request_error"
    } else {
        "api_error"
    }
}
pub(crate) fn codex_json_error_body(status: StatusCode, message: &str) -> String {
    json!({
        "error": {
            "message": message,
            "type": codex_error_type_for_status(status),
            "param": Value::Null,
            "code": Value::Null,
        }
    })
    .to_string()
}
pub(crate) fn codex_json_error(status: StatusCode, message: &str) -> Response {
    let body = codex_json_error_body(status, message);
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .unwrap_or_else(|_| {
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to build error").into_response()
        })
}
pub(crate) fn codex_endpoint_prefers_anthropic_errors(endpoint: &str) -> bool {
    endpoint == "/v1/messages" || endpoint.starts_with("/v1/messages?")
}
pub(crate) fn codex_surface_error_body(
    endpoint: &str,
    status: StatusCode,
    message: &str,
) -> String {
    if codex_endpoint_prefers_anthropic_errors(endpoint) {
        anthropic_json_error_body(codex_error_type_for_status(status), message)
    } else {
        codex_json_error_body(status, message)
    }
}
pub(crate) fn codex_surface_error_response(
    endpoint: &str,
    status: StatusCode,
    message: &str,
) -> Response {
    if codex_endpoint_prefers_anthropic_errors(endpoint) {
        anthropic_json_error(status, codex_error_type_for_status(status), message)
    } else {
        codex_json_error(status, message)
    }
}
pub(crate) fn extract_error_message_from_json_value(value: &Value) -> Option<String> {
    if let Some(message) = value.get("error").and_then(Value::as_str) {
        return Some(message.to_string());
    }
    if let Some(error) = value.get("error").and_then(Value::as_object) {
        if let Some(message) = error.get("message").and_then(Value::as_str) {
            return Some(message.to_string());
        }
    }
    if let Some(message) = value
        .pointer("/response/error/message")
        .and_then(Value::as_str)
        .map(ToString::to_string)
    {
        return Some(message);
    }
    value
        .get("message")
        .and_then(Value::as_str)
        .map(ToString::to_string)
}
pub(crate) fn summarize_error_bytes(bytes: &Bytes) -> String {
    if let Ok(value) = serde_json::from_slice::<Value>(bytes.as_ref()) {
        if let Some(message) = extract_error_message_from_json_value(&value)
            .map(|message| message.trim().to_string())
            .filter(|message| !message.is_empty())
        {
            return message;
        }
    }
    let body = String::from_utf8_lossy(bytes.as_ref()).trim().to_string();
    if body.is_empty() {
        "Unknown upstream error".to_string()
    } else {
        body
    }
}
