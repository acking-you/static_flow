//! Provider entry handler + auth/route/quota gate helpers.


use axum::{
    body::Body,
    extract::State,
    http::{header, HeaderMap, Request, StatusCode},
    response::{IntoResponse, Response},
};
use llm_access_core::{
    provider::{ProtocolFamily, ProviderType},
    routes::provider_route_requirement,
    store::AuthenticatedKey,
};

use super::{
    codex_auth::normalized_codex_gateway_path, kiro_error::kiro_json_error, ProviderState,
};

/// Axum entrypoint for provider requests.
pub async fn provider_entry_handler(
    State(state): State<ProviderState>,
    request: Request<Body>,
) -> Response {
    provider_entry(state, request).await
}
/// Authenticate a provider request before handing it to provider dispatch.
pub async fn provider_entry(state: ProviderState, request: Request<Body>) -> Response {
    let Some(secret) = presented_secret(request.headers(), request.uri().path()).map(str::to_owned)
    else {
        return (StatusCode::UNAUTHORIZED, "missing bearer token").into_response();
    };
    let key = match state
        .control_store
        .authenticate_bearer_secret(&secret)
        .await
    {
        Ok(Some(key)) => key,
        Ok(None) => return (StatusCode::UNAUTHORIZED, "invalid bearer token").into_response(),
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "authentication backend error")
                .into_response();
        },
    };
    if !is_active_key(&key) {
        return (StatusCode::FORBIDDEN, "llm key is not active").into_response();
    }
    if !key_matches_route(&key, request.uri().path()) {
        return (StatusCode::FORBIDDEN, "llm key does not match provider route").into_response();
    }
    if is_quota_exhausted(&key) {
        return quota_exhausted_response(&key);
    }

    let _activity_guard = state.request_activity.start(&key.key_id);
    state
        .dispatcher
        .dispatch(key, request, state.dispatch_deps())
        .await
}
fn presented_secret<'a>(headers: &'a HeaderMap, path: &str) -> Option<&'a str> {
    if accepts_anthropic_api_key_header(path) {
        x_api_key_secret(headers).or_else(|| bearer_secret(headers))
    } else {
        bearer_secret(headers)
    }
}
fn accepts_anthropic_api_key_header(path: &str) -> bool {
    path == "/v1/models"
        || is_kiro_data_plane_route(path)
        || is_codex_anthropic_messages_route(path)
}
fn is_kiro_data_plane_route(path: &str) -> bool {
    provider_route_requirement(path)
        .map(|requirement| requirement.provider_type == ProviderType::Kiro)
        .unwrap_or(false)
}
fn is_codex_anthropic_messages_route(path: &str) -> bool {
    normalized_codex_gateway_path(path) == Some("/v1/messages")
}
fn x_api_key_secret(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get("x-api-key")?.to_str().ok()?.trim();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}
fn bearer_secret(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    if token.is_empty() {
        None
    } else {
        Some(token)
    }
}
pub fn is_active_key(key: &AuthenticatedKey) -> bool {
    key.status == "active"
}
pub fn key_matches_route(key: &AuthenticatedKey, path: &str) -> bool {
    if path == "/v1/models" {
        return true;
    }
    let Some(requirement) = provider_route_requirement(path) else {
        return true;
    };
    ProviderType::from_storage_str(&key.provider_type) == Some(requirement.provider_type)
        && ProtocolFamily::from_storage_str(&key.protocol_family)
            == Some(requirement.protocol_family)
}
pub fn is_quota_exhausted(key: &AuthenticatedKey) -> bool {
    key.remaining_billable() <= 0
}
pub fn quota_exhausted_response(key: &AuthenticatedKey) -> Response {
    if ProviderType::from_storage_str(&key.provider_type) == Some(ProviderType::Kiro) {
        kiro_json_error(StatusCode::PAYMENT_REQUIRED, "rate_limit_error", "key quota exhausted")
    } else {
        (StatusCode::TOO_MANY_REQUESTS, "quota_exceeded").into_response()
    }
}
