//! Codex completed-response SSE accumulation and reconstruction from streamed
//! bytes.

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

pub(crate) fn completed_codex_sse_error_from_value(value: &Value) -> CompletedCodexSseError {
    let message = extract_error_message_from_json_value(value)
        .map(|message| message.trim().to_string())
        .filter(|message| !message.is_empty())
        .unwrap_or_else(|| "Unknown upstream error".to_string());
    let status = codex_status_from_error_json_value(value).unwrap_or(StatusCode::BAD_GATEWAY);
    CompletedCodexSseError {
        status,
        message,
        body: Some(value.to_string()),
    }
}
pub(crate) fn completed_response_from_sse_bytes(
    bytes: &[u8],
) -> Result<CompletedCodexSse, CompletedCodexSseError> {
    let mut accumulator = CompletedCodexSseAccumulator::default();
    for payload in sse_payloads(bytes) {
        let data = payload.data;
        if data.trim() == "[DONE]" {
            continue;
        }
        accumulator
            .observe_payload(payload.event.as_deref(), &data)
            .map_err(|message| CompletedCodexSseError {
                status: StatusCode::BAD_GATEWAY,
                message: message.to_string(),
                body: None,
            })?;
    }
    accumulator.finish()
}
pub(crate) fn sse_payloads(bytes: &[u8]) -> Vec<SsePayload> {
    let text = String::from_utf8_lossy(bytes).replace("\r\n", "\n");
    text.split("\n\n")
        .filter_map(|event| {
            let event_type = event.lines().find_map(|line| {
                line.strip_prefix("event:")
                    .map(|value| value.strip_prefix(' ').unwrap_or(value).trim())
                    .filter(|value| !value.is_empty())
                    .map(ToString::to_string)
            });
            let data = event
                .lines()
                .filter_map(|line| line.strip_prefix("data:"))
                .map(|line| line.strip_prefix(' ').unwrap_or(line))
                .collect::<Vec<_>>();
            if data.is_empty() {
                None
            } else {
                Some(SsePayload {
                    event: event_type,
                    data: data.join("\n"),
                })
            }
        })
        .collect()
}
