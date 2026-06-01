//! Cooldown classification + Anthropic/Codex error-response builders.

use std::time::Duration;

use axum::{
    body::{Body, Bytes},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use llm_access_core::store::ProviderKiroRoute;
use llm_access_kiro::{
    anthropic::converter::get_context_window_size, parser::decoder::EventStreamDecoder, wire::Event,
};
use serde_json::{json, Value};


pub fn proxy_cooldown_key_for_route(route: &ProviderKiroRoute) -> Option<String> {
    route
        .proxy
        .as_ref()
        .map(|proxy| format!("url:{}", proxy.proxy_url))
}
pub fn is_monthly_request_limit(body: &str) -> bool {
    body.contains("MONTHLY_REQUEST_COUNT")
        || kiro_error_reason(body).as_deref() == Some("MONTHLY_REQUEST_COUNT")
}
pub fn daily_request_limit_cooldown(body: &str) -> Option<Duration> {
    if body.contains("5-minute credit limit exceeded") {
        return Some(Duration::from_secs(5 * 60));
    }
    if kiro_error_reason(body).as_deref() == Some("DAILY_REQUEST_COUNT") {
        return Some(Duration::from_secs(5 * 60));
    }
    None
}
pub fn transient_invalid_model_cooldown(body: &str) -> Option<Duration> {
    if !body.contains("Invalid model") {
        return None;
    }
    if kiro_error_reason(body).as_deref() == Some("INVALID_MODEL_ID") {
        return Some(Duration::from_secs(60));
    }
    None
}
fn kiro_error_reason(body: &str) -> Option<String> {
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
pub fn anthropic_json_error_body(error_type: &str, message: &str) -> String {
    serde_json::json!({
        "error": {
            "type": error_type,
            "message": message,
        }
    })
    .to_string()
}
pub fn anthropic_json_error(status: StatusCode, error_type: &str, message: &str) -> Response {
    let body = anthropic_json_error_body(error_type, message);
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .unwrap_or_else(|_| {
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to build error").into_response()
        })
}
pub fn codex_error_type_for_status(status: StatusCode) -> &'static str {
    if status.is_client_error() {
        "invalid_request_error"
    } else {
        "api_error"
    }
}
fn codex_json_error_body(status: StatusCode, message: &str) -> String {
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
fn codex_json_error(status: StatusCode, message: &str) -> Response {
    let body = codex_json_error_body(status, message);
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .unwrap_or_else(|_| {
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to build error").into_response()
        })
}
fn codex_endpoint_prefers_anthropic_errors(endpoint: &str) -> bool {
    endpoint == "/v1/messages" || endpoint.starts_with("/v1/messages?")
}
pub fn codex_surface_error_body(endpoint: &str, status: StatusCode, message: &str) -> String {
    if codex_endpoint_prefers_anthropic_errors(endpoint) {
        anthropic_json_error_body(codex_error_type_for_status(status), message)
    } else {
        codex_json_error_body(status, message)
    }
}
pub fn codex_surface_error_response(endpoint: &str, status: StatusCode, message: &str) -> Response {
    if codex_endpoint_prefers_anthropic_errors(endpoint) {
        anthropic_json_error(status, codex_error_type_for_status(status), message)
    } else {
        codex_json_error(status, message)
    }
}
pub fn extract_error_message_from_json_value(value: &Value) -> Option<String> {
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
pub fn summarize_error_bytes(bytes: &Bytes) -> String {
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
pub fn kiro_prompt_too_long_message(model: &str, request_input_tokens: i32) -> String {
    let limit_tokens = get_context_window_size(model).max(1);
    let actual_tokens = request_input_tokens.max(limit_tokens.saturating_add(1));
    format!(
        "Prompt is too long: {actual_tokens} tokens > {limit_tokens} tokens for the model context \
         window."
    )
}
pub fn kiro_prompt_too_long_response_for_body(
    status: StatusCode,
    bytes: &Bytes,
    model: &str,
    request_input_tokens: i32,
) -> Option<Response> {
    if status != StatusCode::PAYLOAD_TOO_LARGE && !kiro_body_is_content_length_exceeded(bytes) {
        return None;
    }
    let message = kiro_prompt_too_long_message(model, request_input_tokens);
    Some(anthropic_json_error(StatusCode::PAYLOAD_TOO_LARGE, "invalid_request_error", &message))
}
fn kiro_body_is_content_length_exceeded(bytes: &Bytes) -> bool {
    kiro_text_is_content_length_exceeded(&String::from_utf8_lossy(bytes.as_ref()))
}
pub fn kiro_events_contain_content_length_exceeded(events: &[Event]) -> bool {
    events.iter().any(kiro_event_is_content_length_exceeded)
}
pub fn kiro_chunk_contains_content_length_exceeded(chunk: &Bytes) -> bool {
    let mut decoder = EventStreamDecoder::new();
    let _ = decoder.feed(chunk);
    decoder.decode_iter().any(|result| {
        let Ok(frame) = result else {
            return false;
        };
        Event::from_frame(frame)
            .ok()
            .as_ref()
            .is_some_and(kiro_event_is_content_length_exceeded)
    })
}
fn kiro_event_is_content_length_exceeded(event: &Event) -> bool {
    match event {
        Event::Error {
            error_code,
            error_message,
        } => {
            kiro_text_is_content_length_exceeded(error_code)
                || kiro_text_is_content_length_exceeded(error_message)
        },
        Event::Exception {
            exception_type,
            message,
        } => {
            kiro_text_is_content_length_exceeded(exception_type)
                || kiro_text_is_content_length_exceeded(message)
        },
        _ => false,
    }
}
pub fn kiro_text_is_content_length_exceeded(text: &str) -> bool {
    let normalized = text.to_ascii_lowercase();
    normalized.contains("content_length_exceeds_threshold")
        || normalized.contains("contentlengthexceededexception")
        || normalized.contains("input content length exceeds threshold")
        || normalized.contains("input is too long")
}
