//! Cooldown classification + Anthropic/Codex error-response builders.

use std::time::Duration;

use axum::{
    body::{Body, Bytes},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use llm_access_core::store::ProviderKiroRoute;
use llm_access_kiro::{
    anthropic::converter::get_context_window_size, parser::decoder::EventStreamDecoder, wire::Event,
};
use rand::Rng;
use serde_json::{json, Value};

const KIRO_DEFAULT_RATE_LIMIT_COOLDOWN: Duration = Duration::from_secs(5 * 60);
const KIRO_MAX_RATE_LIMIT_COOLDOWN: Duration = Duration::from_secs(15 * 60);
const SAME_ACCOUNT_RETRY_MAX_DELAY: Duration = Duration::from_secs(30);
const TRANSPORT_RETRY_MIN_DELAY_MS: u64 = 200;
const TRANSPORT_RETRY_MAX_DELAY_MS: u64 = 2_000;
const EMPTY_STREAM_RETRY_MIN_DELAY_MS: u64 = 100;
const EMPTY_STREAM_RETRY_MAX_DELAY_MS: u64 = 1_000;
const AUTH_REFRESH_RETRY_MIN_DELAY_MS: u64 = 200;
const AUTH_REFRESH_RETRY_MAX_DELAY_MS: u64 = 2_000;
const RETRYABLE_STATUS_MIN_DELAY_MS: u64 = 500;
const RETRYABLE_STATUS_MAX_DELAY_MS: u64 = 3_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SameAccountRetryReason {
    Transport,
    AuthRefresh,
    RetryableStatus,
    EmptyStream,
    RetrySameAccount,
}

impl SameAccountRetryReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Transport => "transport",
            Self::AuthRefresh => "auth_refresh",
            Self::RetryableStatus => "retryable_status",
            Self::EmptyStream => "empty_stream",
            Self::RetrySameAccount => "retry_same_account",
        }
    }
}

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
pub fn kiro_rate_limit_cooldown(headers: &HeaderMap, body: &str) -> Option<Duration> {
    retry_after_header_duration(headers)
        .map(|duration| duration.min(KIRO_MAX_RATE_LIMIT_COOLDOWN))
        .or_else(|| daily_request_limit_cooldown(body))
        .or(Some(KIRO_DEFAULT_RATE_LIMIT_COOLDOWN))
}
pub fn retry_after_header_duration(headers: &HeaderMap) -> Option<Duration> {
    let seconds = headers
        .get(header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()?;
    Some(Duration::from_secs(seconds))
}
pub fn randomized_same_account_retry_delay(
    reason: SameAccountRetryReason,
    retry_after: Option<Duration>,
) -> Duration {
    if let Some(retry_after) = retry_after {
        let max_seconds = retry_after
            .min(SAME_ACCOUNT_RETRY_MAX_DELAY)
            .as_secs()
            .max(1);
        return Duration::from_secs(rand::thread_rng().gen_range(1..=max_seconds));
    }
    let (min_ms, max_ms) = match reason {
        SameAccountRetryReason::Transport => {
            (TRANSPORT_RETRY_MIN_DELAY_MS, TRANSPORT_RETRY_MAX_DELAY_MS)
        },
        SameAccountRetryReason::AuthRefresh => {
            (AUTH_REFRESH_RETRY_MIN_DELAY_MS, AUTH_REFRESH_RETRY_MAX_DELAY_MS)
        },
        SameAccountRetryReason::RetryableStatus => {
            (RETRYABLE_STATUS_MIN_DELAY_MS, RETRYABLE_STATUS_MAX_DELAY_MS)
        },
        SameAccountRetryReason::EmptyStream => {
            (EMPTY_STREAM_RETRY_MIN_DELAY_MS, EMPTY_STREAM_RETRY_MAX_DELAY_MS)
        },
        SameAccountRetryReason::RetrySameAccount => {
            (1_000, SAME_ACCOUNT_RETRY_MAX_DELAY.as_millis() as u64)
        },
    };
    Duration::from_millis(rand::thread_rng().gen_range(min_ms..=max_ms))
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
fn codex_json_error_body_with_code(
    status: StatusCode,
    message: &str,
    code: Option<&str>,
) -> String {
    json!({
        "error": {
            "message": message,
            "type": codex_error_type_for_status(status),
            "param": Value::Null,
            "code": code.map_or(Value::Null, Value::from),
        }
    })
    .to_string()
}
fn codex_json_error_with_code(status: StatusCode, message: &str, code: Option<&str>) -> Response {
    let body = codex_json_error_body_with_code(status, message, code);
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
    codex_surface_error_body_with_code(endpoint, status, message, None)
}
/// Like [`codex_surface_error_body`] but injects an OpenAI-style `error.code`
/// (e.g. `server_is_overloaded`) so the Codex client can classify the failure
/// instead of seeing a raw, untagged message. Anthropic-style endpoints keep
/// their type-based shape and ignore `code`.
pub fn codex_surface_error_body_with_code(
    endpoint: &str,
    status: StatusCode,
    message: &str,
    code: Option<&str>,
) -> String {
    if codex_endpoint_prefers_anthropic_errors(endpoint) {
        anthropic_json_error_body(codex_error_type_for_status(status), message)
    } else {
        codex_json_error_body_with_code(status, message, code)
    }
}
pub fn codex_surface_error_response(endpoint: &str, status: StatusCode, message: &str) -> Response {
    codex_surface_error_response_with_code(endpoint, status, message, None)
}
/// See [`codex_surface_error_body_with_code`]: surfaces a classified error with
/// an explicit OpenAI-style `error.code`.
pub fn codex_surface_error_response_with_code(
    endpoint: &str,
    status: StatusCode,
    message: &str,
    code: Option<&str>,
) -> Response {
    if codex_endpoint_prefers_anthropic_errors(endpoint) {
        anthropic_json_error(status, codex_error_type_for_status(status), message)
    } else {
        codex_json_error_with_code(status, message, code)
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
/// Formats the `Prompt is too long` message against an explicit limit. The
/// actual count is forced strictly above the limit so the client's `N > M`
/// overflow parser always fires.
fn kiro_too_long_message_with_limit(request_input_tokens: i32, limit_tokens: i32) -> String {
    let limit_tokens = limit_tokens.max(1);
    let actual_tokens = request_input_tokens.max(limit_tokens.saturating_add(1));
    format!(
        "Prompt is too long: {actual_tokens} tokens > {limit_tokens} tokens for the model context \
         window."
    )
}
pub fn kiro_prompt_too_long_message(model: &str, request_input_tokens: i32) -> String {
    kiro_too_long_message_with_limit(request_input_tokens, get_context_window_size(model))
}
/// The proactive-compaction `Prompt is too long` message for the configured
/// `trigger`. Exposed so the dispatch gate can record the same text into the
/// usage/error audit trail before returning the response.
pub fn kiro_proactive_compact_message(request_input_tokens: i32, trigger: i32) -> String {
    kiro_too_long_message_with_limit(request_input_tokens, trigger)
}
/// Builds the proactive `Prompt is too long` response that nudges the client
/// into reactive compaction at the configured `trigger`, before the request is
/// sent upstream. The reported limit is the trigger itself — an honest soft
/// ceiling that sits below the model's true window — so the client compacts
/// early, while the summary request it then issues still fits the real window.
pub fn kiro_proactive_compact_response(request_input_tokens: i32, trigger: i32) -> Response {
    let message = kiro_too_long_message_with_limit(request_input_tokens, trigger);
    anthropic_json_error(StatusCode::PAYLOAD_TOO_LARGE, "invalid_request_error", &message)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proactive_message_reports_trigger_as_limit_with_strict_overflow() {
        // real input above the trigger → reported verbatim, gap positive
        let message = kiro_too_long_message_with_limit(812_345, 780_000);
        assert!(message.contains("812345 tokens > 780000 tokens"), "got: {message}");
        assert!(message.starts_with("Prompt is too long:"), "got: {message}");
    }

    #[test]
    fn proactive_message_forces_actual_above_limit_at_boundary() {
        // real input == trigger → actual bumped to trigger+1 so N > M still holds
        let message = kiro_too_long_message_with_limit(780_000, 780_000);
        assert!(message.contains("780001 tokens > 780000 tokens"), "got: {message}");
    }

    #[test]
    fn proactive_response_is_413_invalid_request() {
        let response = kiro_proactive_compact_response(900_000, 780_000);
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }
}
