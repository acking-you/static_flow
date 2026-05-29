//! Codex upstream URL/base, client-version normalization, gateway-path
//! normalization, protocol-family mapping, and dispatch runtime config.

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

pub(crate) fn normalized_codex_gateway_path(path: &str) -> Option<&str> {
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
pub(crate) fn codex_protocol_family_for_endpoint(endpoint: &str) -> ProtocolFamily {
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
pub(crate) fn normalize_codex_client_version(raw: &str) -> Option<String> {
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
pub(crate) fn resolve_codex_account_attempt_limit(raw: u64) -> usize {
    usize::try_from(raw).unwrap_or(usize::MAX).max(1)
}
pub(crate) async fn load_codex_dispatch_runtime_config(
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
pub(crate) fn codex_user_agent(client_version: &str) -> String {
    format!("{DEFAULT_WIRE_ORIGINATOR}/{client_version}")
}
