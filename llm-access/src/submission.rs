//! Public unauthenticated submission endpoints.

use std::{
    collections::HashMap,
    str::FromStr,
    sync::{Arc, RwLock},
};

use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use lettre::message::Mailbox;
use llm_access_core::store::{
    NewPublicAccountContributionRequest, NewPublicSponsorRequest, NewPublicTokenRequest,
    PUBLIC_SPONSOR_REQUEST_STATUS_SUBMITTED, PUBLIC_TOKEN_REQUEST_STATUS_PENDING,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

use crate::{geoip::GeoIpResolver, HttpState};

const MAX_PUBLIC_TOKEN_WISH_REASON_CHARS: usize = 4000;
const MAX_PUBLIC_TOKEN_WISH_QUOTA: u64 = 100_000_000_000;
const MAX_PUBLIC_ACCOUNT_CONTRIBUTION_MESSAGE_CHARS: usize = 4000;
const MAX_PUBLIC_ACCOUNT_CONTRIBUTION_GITHUB_ID_CHARS: usize = 39;
const MAX_PUBLIC_SPONSOR_MESSAGE_CHARS: usize = 4000;
const MAX_PUBLIC_SPONSOR_DISPLAY_NAME_CHARS: usize = 80;
const PUBLIC_SUBMIT_RATE_LIMIT_SECONDS: u64 = 60;

/// In-memory per-router public submission rate-limit state.
#[derive(Default)]
pub(crate) struct PublicSubmitGuard {
    entries: RwLock<HashMap<String, i64>>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SubmitLlmGatewayTokenRequest {
    requested_quota_billable_limit: u64,
    request_reason: String,
    requester_email: String,
    #[serde(default)]
    frontend_page_url: Option<String>,
}

#[derive(Debug, Serialize)]
struct SubmitLlmGatewayTokenRequestResponse {
    request_id: String,
    status: &'static str,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SubmitLlmGatewayAccountContributionRequest {
    account_name: String,
    #[serde(default)]
    account_id: Option<String>,
    id_token: String,
    access_token: String,
    refresh_token: String,
    requester_email: String,
    contributor_message: String,
    #[serde(default)]
    github_id: Option<String>,
    #[serde(default)]
    frontend_page_url: Option<String>,
}

#[derive(Debug, Serialize)]
struct SubmitLlmGatewayAccountContributionRequestResponse {
    request_id: String,
    status: &'static str,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SubmitLlmGatewaySponsorRequest {
    requester_email: String,
    sponsor_message: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    github_id: Option<String>,
    #[serde(default)]
    frontend_page_url: Option<String>,
}

#[derive(Debug, Serialize)]
struct SubmitLlmGatewaySponsorRequestResponse {
    request_id: String,
    status: &'static str,
    payment_email_sent: bool,
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
    code: u16,
}

pub(crate) async fn submit_public_token_request(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<SubmitLlmGatewayTokenRequest>,
) -> Response {
    let request =
        match normalize_token_request(request, &headers, &state.public_submit_guard, &state.geoip)
            .await
        {
            Ok(request) => request,
            Err(err) => return err.into_response(),
        };
    let request_id = request.request_id.clone();
    match state
        .public_submission_store
        .create_public_token_request(request)
        .await
    {
        Ok(()) => Json(SubmitLlmGatewayTokenRequestResponse {
            request_id,
            status: PUBLIC_TOKEN_REQUEST_STATUS_PENDING,
        })
        .into_response(),
        Err(_) => internal_error("public submission store error").into_response(),
    }
}

pub(crate) async fn submit_public_account_contribution_request(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<SubmitLlmGatewayAccountContributionRequest>,
) -> Response {
    let request = match normalize_account_contribution_request(
        request,
        &headers,
        &state.public_submit_guard,
        &state.geoip,
    )
    .await
    {
        Ok(request) => request,
        Err(err) => return err.into_response(),
    };
    let request_id = request.request_id.clone();
    match state
        .public_submission_store
        .create_public_account_contribution_request(request)
        .await
    {
        Ok(()) => Json(SubmitLlmGatewayAccountContributionRequestResponse {
            request_id,
            status: PUBLIC_TOKEN_REQUEST_STATUS_PENDING,
        })
        .into_response(),
        Err(_) => internal_error("public submission store error").into_response(),
    }
}

pub(crate) async fn submit_public_sponsor_request(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<SubmitLlmGatewaySponsorRequest>,
) -> Response {
    let request = match normalize_sponsor_request(
        request,
        &headers,
        &state.public_submit_guard,
        &state.geoip,
    )
    .await
    {
        Ok(request) => request,
        Err(err) => return err.into_response(),
    };
    let request_id = request.request_id.clone();
    match state
        .public_submission_store
        .create_public_sponsor_request(request)
        .await
    {
        Ok(()) => Json(SubmitLlmGatewaySponsorRequestResponse {
            request_id,
            status: PUBLIC_SPONSOR_REQUEST_STATUS_SUBMITTED,
            payment_email_sent: false,
        })
        .into_response(),
        Err(_) => internal_error("public submission store error").into_response(),
    }
}

async fn normalize_token_request(
    request: SubmitLlmGatewayTokenRequest,
    headers: &HeaderMap,
    guard: &Arc<PublicSubmitGuard>,
    geoip: &GeoIpResolver,
) -> Result<NewPublicTokenRequest, SubmitError> {
    if request.requested_quota_billable_limit == 0 {
        return Err(bad_request("requested_quota_billable_limit must be > 0"));
    }
    if request.requested_quota_billable_limit > MAX_PUBLIC_TOKEN_WISH_QUOTA {
        return Err(bad_request("requested_quota_billable_limit is too large"));
    }
    let request_reason = request.request_reason.trim();
    if request_reason.is_empty() {
        return Err(bad_request("request_reason is required"));
    }
    if request_reason.chars().count() > MAX_PUBLIC_TOKEN_WISH_REASON_CHARS {
        return Err(bad_request("request_reason is too long"));
    }
    let requester_email = normalize_requester_email_input(Some(request.requester_email))
        .map_err(|err| bad_request(&format!("invalid requester_email: {err}")))?
        .ok_or_else(|| bad_request("requester_email is required"))?;
    let frontend_page_url = normalize_frontend_page_url_input(request.frontend_page_url)
        .map_err(|err| bad_request(&format!("invalid frontend_page_url: {err}")))?;
    let submit_context = submit_context(headers, guard, geoip).await?;

    Ok(NewPublicTokenRequest {
        request_id: generate_task_id("llmwish"),
        requester_email,
        requested_quota_billable_limit: request.requested_quota_billable_limit,
        request_reason: request_reason.to_string(),
        frontend_page_url,
        fingerprint: submit_context.fingerprint,
        client_ip: submit_context.client_ip,
        ip_region: submit_context.ip_region,
        created_at_ms: submit_context.now_ms,
    })
}

async fn normalize_account_contribution_request(
    request: SubmitLlmGatewayAccountContributionRequest,
    headers: &HeaderMap,
    guard: &Arc<PublicSubmitGuard>,
    geoip: &GeoIpResolver,
) -> Result<NewPublicAccountContributionRequest, SubmitError> {
    let account_name =
        validate_account_name(&request.account_name).map_err(|err| bad_request(&err))?;
    let account_id = normalize_optional_string(request.account_id);
    let id_token = request.id_token.trim().to_string();
    let access_token = request.access_token.trim().to_string();
    let refresh_token = request.refresh_token.trim().to_string();
    if id_token.is_empty() || access_token.is_empty() || refresh_token.is_empty() {
        return Err(bad_request("id_token, access_token, and refresh_token are required"));
    }
    let requester_email = normalize_requester_email_input(Some(request.requester_email))
        .map_err(|err| bad_request(&format!("invalid requester_email: {err}")))?
        .ok_or_else(|| bad_request("requester_email is required"))?;
    let contributor_message = request.contributor_message.trim();
    if contributor_message.is_empty() {
        return Err(bad_request("contributor_message is required"));
    }
    if contributor_message.chars().count() > MAX_PUBLIC_ACCOUNT_CONTRIBUTION_MESSAGE_CHARS {
        return Err(bad_request("contributor_message is too long"));
    }
    let github_id = normalize_optional_github_id_input(request.github_id)
        .map_err(|err| bad_request(&format!("invalid github_id: {err}")))?;
    let frontend_page_url = normalize_frontend_page_url_input(request.frontend_page_url)
        .map_err(|err| bad_request(&format!("invalid frontend_page_url: {err}")))?;
    let submit_context = submit_context(headers, guard, geoip).await?;

    Ok(NewPublicAccountContributionRequest {
        request_id: generate_task_id("llmacct"),
        account_name,
        account_id,
        id_token,
        access_token,
        refresh_token,
        requester_email,
        contributor_message: contributor_message.to_string(),
        github_id,
        frontend_page_url,
        fingerprint: submit_context.fingerprint,
        client_ip: submit_context.client_ip,
        ip_region: submit_context.ip_region,
        created_at_ms: submit_context.now_ms,
    })
}

async fn normalize_sponsor_request(
    request: SubmitLlmGatewaySponsorRequest,
    headers: &HeaderMap,
    guard: &Arc<PublicSubmitGuard>,
    geoip: &GeoIpResolver,
) -> Result<NewPublicSponsorRequest, SubmitError> {
    let requester_email = normalize_requester_email_input(Some(request.requester_email))
        .map_err(|err| bad_request(&format!("invalid requester_email: {err}")))?
        .ok_or_else(|| bad_request("requester_email is required"))?;
    let sponsor_message = request.sponsor_message.trim();
    if sponsor_message.is_empty() {
        return Err(bad_request("sponsor_message is required"));
    }
    if sponsor_message.chars().count() > MAX_PUBLIC_SPONSOR_MESSAGE_CHARS {
        return Err(bad_request("sponsor_message is too long"));
    }
    let display_name = normalize_optional_display_name_input(request.display_name)
        .map_err(|err| bad_request(&format!("invalid display_name: {err}")))?;
    let github_id = normalize_optional_github_id_input(request.github_id)
        .map_err(|err| bad_request(&format!("invalid github_id: {err}")))?;
    let frontend_page_url = normalize_frontend_page_url_input(request.frontend_page_url)
        .map_err(|err| bad_request(&format!("invalid frontend_page_url: {err}")))?;
    let submit_context = submit_context(headers, guard, geoip).await?;

    Ok(NewPublicSponsorRequest {
        request_id: generate_task_id("llmsponsor"),
        requester_email,
        sponsor_message: sponsor_message.to_string(),
        display_name,
        github_id,
        frontend_page_url,
        fingerprint: submit_context.fingerprint,
        client_ip: submit_context.client_ip,
        ip_region: submit_context.ip_region,
        created_at_ms: submit_context.now_ms,
    })
}

struct SubmitContext {
    fingerprint: String,
    client_ip: String,
    ip_region: String,
    now_ms: i64,
}

async fn submit_context(
    headers: &HeaderMap,
    guard: &Arc<PublicSubmitGuard>,
    geoip: &GeoIpResolver,
) -> Result<SubmitContext, SubmitError> {
    let now_ms = now_ms();
    let client_ip = extract_client_ip(headers);
    let fingerprint = build_client_fingerprint(headers);
    let rate_limit_key = build_submit_rate_limit_key(headers, &fingerprint);
    enforce_public_submit_rate_limit(
        guard,
        &rate_limit_key,
        now_ms,
        PUBLIC_SUBMIT_RATE_LIMIT_SECONDS,
        "llm-access public submission",
    )?;
    let ip_region = geoip.resolve_region(&client_ip).await;
    Ok(SubmitContext {
        fingerprint,
        client_ip,
        ip_region,
        now_ms,
    })
}

fn normalize_requester_email_input(value: Option<String>) -> anyhow::Result<Option<String>> {
    match normalize_optional_string(value) {
        Some(raw) => {
            if raw.chars().count() > 254 {
                anyhow::bail!("`requester_email` must be <= 254 chars");
            }
            let trimmed = raw.trim();
            Mailbox::from_str(trimmed)
                .map_err(|err| anyhow::anyhow!("invalid email address: {trimmed}: {err}"))?;
            Ok(Some(trimmed.to_string()))
        },
        None => Ok(None),
    }
}

fn normalize_frontend_page_url_input(value: Option<String>) -> anyhow::Result<Option<String>> {
    match normalize_optional_string(value) {
        Some(raw) => {
            if raw.chars().count() > 2000 {
                anyhow::bail!("`frontend_page_url` must be <= 2000 chars");
            }
            let parsed = Url::parse(&raw).map_err(|err| anyhow::anyhow!("invalid URL: {err}"))?;
            match parsed.scheme() {
                "http" | "https" => {},
                _ => anyhow::bail!("`frontend_page_url` must use http or https"),
            }
            if parsed.host_str().is_none() {
                anyhow::bail!("`frontend_page_url` must include a host");
            }
            Ok(Some(raw))
        },
        None => Ok(None),
    }
}

fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
}

fn normalize_optional_github_id_input(value: Option<String>) -> anyhow::Result<Option<String>> {
    let Some(trimmed) = normalize_optional_string(value) else {
        return Ok(None);
    };
    if trimmed.chars().count() > MAX_PUBLIC_ACCOUNT_CONTRIBUTION_GITHUB_ID_CHARS {
        anyhow::bail!("github_id is too long");
    }
    if trimmed.starts_with('-') || trimmed.ends_with('-') {
        anyhow::bail!("github_id cannot start or end with `-`");
    }
    if !trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
    {
        anyhow::bail!("github_id may contain only ASCII letters, digits, or `-`");
    }
    Ok(Some(trimmed))
}

fn normalize_optional_display_name_input(value: Option<String>) -> anyhow::Result<Option<String>> {
    let Some(trimmed) = normalize_optional_string(value) else {
        return Ok(None);
    };
    if trimmed.chars().count() > MAX_PUBLIC_SPONSOR_DISPLAY_NAME_CHARS {
        anyhow::bail!("display_name is too long");
    }
    Ok(Some(trimmed))
}

fn validate_account_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("account name is required".to_string());
    }
    if trimmed.len() > 64 {
        return Err("account name must be 64 characters or fewer".to_string());
    }
    if !trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return Err("account name must contain only ASCII letters, digits, hyphens, or \
                    underscores"
            .to_string());
    }
    Ok(trimmed.to_string())
}

fn build_client_fingerprint(headers: &HeaderMap) -> String {
    let ip = extract_client_ip(headers);
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("unknown");
    let raw = format!("{ip}|{user_agent}");

    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn build_submit_rate_limit_key(headers: &HeaderMap, fingerprint: &str) -> String {
    let ip = extract_client_ip(headers);
    if ip == "unknown" {
        format!("fp:{fingerprint}")
    } else {
        format!("ip:{ip}")
    }
}

fn enforce_public_submit_rate_limit(
    guard: &Arc<PublicSubmitGuard>,
    rate_limit_key: &str,
    now_ms: i64,
    rate_limit_seconds: u64,
    action_label: &str,
) -> Result<(), SubmitError> {
    let window_ms = (rate_limit_seconds.max(1) as i64) * 1_000;
    let mut writer = guard
        .entries
        .write()
        .map_err(|_| internal_error("public submit rate-limit state error"))?;
    if let Some(last) = writer.get(rate_limit_key) {
        let elapsed_ms = now_ms.saturating_sub(*last);
        if elapsed_ms < window_ms {
            let remaining_seconds = ((window_ms - elapsed_ms) + 999) / 1_000;
            return Err(SubmitError {
                status: StatusCode::TOO_MANY_REQUESTS,
                message: format!(
                    "{action_label} is rate-limited for this IP. Same IP may submit once every {} \
                     seconds. Retry in {} seconds.",
                    rate_limit_seconds.max(1),
                    remaining_seconds.max(1)
                ),
            });
        }
    }
    writer.insert(rate_limit_key.to_string(), now_ms);
    let stale_before = now_ms - window_ms * 6;
    writer.retain(|_, value| *value >= stale_before);
    Ok(())
}

fn extract_client_ip(headers: &HeaderMap) -> String {
    parse_first_ip_from_header(headers.get("x-forwarded-for"))
        .or_else(|| parse_first_ip_from_header(headers.get("x-real-ip")))
        .or_else(|| parse_first_ip_from_header(headers.get("cf-connecting-ip")))
        .or_else(|| parse_first_ip_from_header(headers.get("x-client-ip")))
        .or_else(|| parse_ip_from_forwarded_header(headers.get("forwarded")))
        .unwrap_or_else(|| "unknown".to_string())
}

fn parse_first_ip_from_header(value: Option<&axum::http::HeaderValue>) -> Option<String> {
    let raw = value?.to_str().ok()?;
    raw.split(',').find_map(normalize_ip_token)
}

fn parse_ip_from_forwarded_header(value: Option<&axum::http::HeaderValue>) -> Option<String> {
    let raw = value?.to_str().ok()?;
    raw.split(',').find_map(|entry| {
        entry.split(';').find_map(|segment| {
            let token = segment.trim();
            if token
                .get(..4)
                .map(|prefix| prefix.eq_ignore_ascii_case("for="))
                .unwrap_or(false)
            {
                normalize_ip_token(token)
            } else {
                None
            }
        })
    })
}

fn normalize_ip_token(token: &str) -> Option<String> {
    let mut value = token.trim().trim_matches('"');
    if value.is_empty() || value.eq_ignore_ascii_case("unknown") {
        return None;
    }

    if value
        .get(..4)
        .map(|prefix| prefix.eq_ignore_ascii_case("for="))
        .unwrap_or(false)
    {
        value = value[4..].trim().trim_matches('"');
    }

    if value.starts_with('[') {
        if let Some(end) = value.find(']') {
            let host = &value[1..end];
            let remain = value[end + 1..].trim();
            let valid_suffix = remain.is_empty()
                || (remain.starts_with(':') && remain[1..].chars().all(|ch| ch.is_ascii_digit()));
            if valid_suffix {
                if let Ok(ip) = host.parse::<std::net::IpAddr>() {
                    return Some(ip.to_string());
                }
            }
        }
    }

    if let Ok(ip) = value.parse::<std::net::IpAddr>() {
        return Some(ip.to_string());
    }

    if let Some((host, port)) = value.rsplit_once(':') {
        if host.contains('.') && !port.is_empty() && port.chars().all(|ch| ch.is_ascii_digit()) {
            if let Ok(ip) = host.parse::<std::net::IpAddr>() {
                return Some(ip.to_string());
            }
        }
    }

    None
}

fn generate_task_id(prefix: &str) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    format!("{prefix}-{now_ms}-{nanos}")
}

fn now_ms() -> i64 {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    millis.min(i64::MAX as u128) as i64
}

#[derive(Debug)]
struct SubmitError {
    status: StatusCode,
    message: String,
}

impl IntoResponse for SubmitError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorResponse {
                error: self.message,
                code: self.status.as_u16(),
            }),
        )
            .into_response()
    }
}

fn bad_request(message: &str) -> SubmitError {
    SubmitError {
        status: StatusCode::BAD_REQUEST,
        message: message.to_string(),
    }
}

fn internal_error(message: &str) -> SubmitError {
    SubmitError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        message: message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn submit_context_resolves_client_ip_region() {
        let resolver = crate::geoip::GeoIpResolver::fixed_for_tests("Singapore/Singapore");
        let headers = HeaderMap::from_iter([(
            "x-forwarded-for".parse().expect("header name"),
            "208.77.246.15".parse().expect("header value"),
        )]);
        let guard = Arc::new(PublicSubmitGuard::default());

        let context = submit_context(&headers, &guard, &resolver)
            .await
            .expect("submit context");

        assert_eq!(context.client_ip, "208.77.246.15");
        assert_eq!(context.ip_region, "Singapore/Singapore");
    }
}
