//! Terminal SSE error rendering for Codex streams that fail after downstream
//! bytes have already been sent.
//!
//! This module deliberately does not classify upstream failures. The dispatcher
//! already owns that job; this file only translates a classified failure into
//! the terminal shape expected by the client-facing protocol.
//!
//! ```text
//! upstream SSE event
//!        |
//!        v
//! classify_codex_sse_event_failure(...)
//!        |
//!        v
//! CodexClassifiedUpstreamError + effective HTTP status
//!        |
//!        v
//! codex_stream_failure_chunks(adapter, path, status, error)
//!        |
//!        +-- Responses ---------> event: response.failed
//!        |                         data: {"type":"response.failed", ...}
//!        |
//!        +-- ChatCompletions ---> data: {"error":{...}}
//!        |                         data: [DONE]
//!        |
//!        `-- AnthropicMessages -> event: error
//!                                  data: {"type":"error","error":{...}}
//! ```
//!
//! The important boundary is "after downstream write started": at that point we
//! cannot change the HTTP status and we cannot safely fail over to another
//! account without mixing two upstream conversations in one client stream.
//! Returning a protocol-shaped terminal error is the least surprising contract:
//! clients see the real failure, usage still records the upstream failure, and
//! the stream does not look like a normal empty response.
//!
//! Error-body policy:
//! - Surface only the sanitized classified message/type/code to the client.
//! - Keep raw upstream error bodies in usage metadata, not in SSE frames.
//! - Leave preflight failures to the existing non-stream JSON error path, where
//!   the HTTP response has not been committed yet.

use axum::{body::Bytes, http::StatusCode};
use llm_access_codex::types::GatewayResponseAdapter;
use serde_json::{json, Value};

use super::{
    codex_upstream_error::CodexClassifiedUpstreamError,
    errors::{codex_error_type_for_status, codex_surface_error_body_with_code},
};

/// Build the final client-visible chunks for a stream that can no longer
/// return a regular HTTP error response.
///
/// The output is intentionally tiny: one SSE event for Responses/Anthropic, or
/// an OpenAI-style error data frame plus `[DONE]` for Chat Completions. Keeping
/// this as a `Vec<Bytes>` makes the dispatcher branch explicit without adding
/// another stream abstraction for at most two frames.
pub(super) fn codex_stream_failure_chunks(
    response_adapter: GatewayResponseAdapter,
    original_path: &str,
    effective_status: StatusCode,
    error: &CodexClassifiedUpstreamError,
) -> Vec<Bytes> {
    match response_adapter {
        GatewayResponseAdapter::Responses => vec![encode_sse_event(
            "response.failed",
            &responses_failed_payload(effective_status, error),
        )],
        GatewayResponseAdapter::ChatCompletions => vec![
            encode_sse_data(&codex_surface_error_body_with_code(
                original_path,
                effective_status,
                &error.message,
                error.class.surface_error_code(),
            )),
            Bytes::from_static(b"data: [DONE]\n\n"),
        ],
        GatewayResponseAdapter::AnthropicMessages => {
            vec![encode_sse_event("error", &anthropic_error_payload(effective_status, error))]
        },
    }
}

fn responses_failed_payload(
    effective_status: StatusCode,
    error: &CodexClassifiedUpstreamError,
) -> Value {
    json!({
        "type": "response.failed",
        "response": {
            "status": "failed",
            "error": stream_error_object(effective_status, error),
        }
    })
}

fn anthropic_error_payload(
    effective_status: StatusCode,
    error: &CodexClassifiedUpstreamError,
) -> Value {
    json!({
        "type": "error",
        "error": stream_error_object(effective_status, error),
    })
}

fn stream_error_object(
    effective_status: StatusCode,
    error: &CodexClassifiedUpstreamError,
) -> Value {
    json!({
        "type": codex_error_type_for_status(effective_status),
        "message": error.message,
        "code": error.class.surface_error_code().map_or(Value::Null, Value::from),
    })
}

fn encode_sse_event(event: &str, payload: &Value) -> Bytes {
    Bytes::from(format!("event: {event}\ndata: {}\n\n", encode_json(payload)))
}

fn encode_sse_data(data: &str) -> Bytes {
    Bytes::from(format!("data: {data}\n\n"))
}

fn encode_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string())
}
