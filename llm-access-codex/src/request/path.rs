//! Gateway path classification (supported POST paths, file-finalize, models)
//! and key-field normalization (`name`, `status`).

use super::*;

/// Reject unsupported public gateway paths before any auth or upstream work
/// begins.
pub fn ensure_supported_gateway_path(path: &str) -> CodexGatewayResult<()> {
    if is_supported_codex_post_path(path) || is_models_path(path) {
        Ok(())
    } else {
        Err(not_found("Unsupported llm gateway endpoint"))
    }
}
pub(crate) fn is_supported_codex_post_path(path: &str) -> bool {
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
pub(crate) fn is_codex_file_finalize_path(path: &str) -> bool {
    let Some(file_id) = path
        .strip_prefix("/v1/files/")
        .and_then(|value| value.strip_suffix("/uploaded"))
    else {
        return false;
    };
    !file_id.is_empty() && !file_id.contains('/')
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
