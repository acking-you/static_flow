//! Gateway path classification (supported POST paths, file-finalize, models).

/// Return whether `path` is a supported Codex POST endpoint.
///
/// Matches the fixed set of upstream POST paths (responses, chat-completions,
/// messages, memories, realtime, files) plus the dynamic per-file upload
/// finalize path (`/v1/files/<id>/uploaded`).
pub fn is_supported_codex_post_path(path: &str) -> bool {
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
fn is_codex_file_finalize_path(path: &str) -> bool {
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
