//! Kiro upstream/MCP header construction and messages-path normalization.


use llm_access_kiro::auth_file::KiroAuthRecord;

use super::KIRO_PROVIDER_AWS_SDK_VERSION;
use crate::{kiro_headers, kiro_refresh};

pub fn normalized_kiro_messages_path(path: &str) -> Option<&'static str> {
    match path {
        "/cc/v1/messages" | "/api/kiro-gateway/cc/v1/messages" => Some("/cc/v1/messages"),
        "/v1/messages" | "/api/kiro-gateway/v1/messages" => Some("/v1/messages"),
        _ => None,
    }
}
pub fn add_kiro_upstream_headers(
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
pub fn add_kiro_mcp_headers(
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
