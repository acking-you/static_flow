//! Kiro upstream/MCP request header assembly and request-session resolution.

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

pub(crate) fn normalized_kiro_messages_path(path: &str) -> Option<&'static str> {
    match path {
        "/cc/v1/messages" | "/api/kiro-gateway/cc/v1/messages" => Some("/cc/v1/messages"),
        "/v1/messages" | "/api/kiro-gateway/v1/messages" => Some("/v1/messages"),
        _ => None,
    }
}
pub(crate) fn add_kiro_upstream_headers(
    upstream: reqwest::RequestBuilder,
    upstream_url: &str,
    access_token: &str,
    auth_record: Option<&KiroAuthRecord>,
) -> anyhow::Result<reqwest::RequestBuilder> {
    let auth = auth_record.ok_or_else(|| anyhow::anyhow!("invalid kiro auth record"))?;
    let host = kiro_refresh::upstream_host_header(upstream_url)?;
    kiro_headers::add_kiro_headers(upstream, auth, kiro_headers::KiroHeaderConfig {
        upstream_host: &host,
        access_token,
        service: kiro_headers::KiroAwsService::Streaming,
        client_version: KIRO_PROVIDER_AWS_SDK_VERSION,
        sdk_request: "attempt=1; max=3",
        content_type: Some("application/json"),
        accept: Some("application/vnd.amazon.eventstream"),
        connection_close: false,
        agent_mode: Some("vibe"),
        include_opt_out: true,
    })
}
pub(crate) fn add_kiro_mcp_headers(
    mut upstream: reqwest::RequestBuilder,
    upstream_url: &str,
    profile_arn: Option<&str>,
    access_token: &str,
    auth_record: Option<&KiroAuthRecord>,
) -> anyhow::Result<reqwest::RequestBuilder> {
    let auth = auth_record.ok_or_else(|| anyhow::anyhow!("invalid kiro auth record"))?;
    let host = kiro_refresh::upstream_host_header(upstream_url)?;
    upstream = kiro_headers::add_kiro_headers(upstream, auth, kiro_headers::KiroHeaderConfig {
        upstream_host: &host,
        access_token,
        service: kiro_headers::KiroAwsService::Streaming,
        client_version: KIRO_PROVIDER_AWS_SDK_VERSION,
        sdk_request: "attempt=1; max=3",
        content_type: Some("application/json"),
        accept: None,
        connection_close: false,
        agent_mode: None,
        include_opt_out: false,
    })?;
    if let Some(profile_arn) = profile_arn.map(str::trim).filter(|value| !value.is_empty()) {
        upstream = upstream.header("x-amzn-kiro-profile-arn", profile_arn);
    }
    Ok(upstream)
}
pub(crate) fn resolve_kiro_request_session(
    headers: &HeaderMap,
    metadata: Option<&llm_access_kiro::anthropic::types::Metadata>,
) -> ResolvedConversationId {
    let mut first_invalid_header: Option<(&'static str, String)> = None;
    for header_name in KIRO_REQUEST_SESSION_ID_HEADERS {
        let Some(raw_value) = headers
            .get(header_name)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
        else {
            continue;
        };
        if uuid::Uuid::try_parse(&raw_value).is_ok() {
            return ResolvedConversationId {
                conversation_id: raw_value.clone(),
                session_tracking: SessionTracking {
                    source: SessionIdSource::RequestHeader,
                    source_name: Some(header_name),
                    source_value_preview: Some(preview_session_value(&raw_value)),
                },
            };
        }
        if first_invalid_header.is_none() {
            first_invalid_header = Some((header_name, preview_session_value(&raw_value)));
        }
    }

    let mut resolved = resolve_conversation_id_from_metadata(metadata);
    if matches!(resolved.session_tracking.source, SessionIdSource::GeneratedFallback(_)) {
        if let Some((header_name, preview)) = first_invalid_header {
            resolved.session_tracking = SessionTracking {
                source: SessionIdSource::GeneratedFallback(
                    SessionFallbackReason::InvalidHeaderSessionId,
                ),
                source_name: Some(header_name),
                source_value_preview: Some(preview),
            };
        }
    }
    resolved
}
