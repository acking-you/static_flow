use axum::{body::Bytes, http::StatusCode};
use llm_access_codex::types::GatewayResponseAdapter;
use serde_json::{json, Value};

use super::{
    codex_upstream_error::CodexClassifiedUpstreamError,
    errors::{codex_error_type_for_status, codex_surface_error_body_with_code},
};

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
