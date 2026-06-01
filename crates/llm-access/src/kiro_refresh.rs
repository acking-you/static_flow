//! Kiro credential refresh helpers for the standalone provider runtime.

use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
};

use anyhow::{anyhow, bail, Context};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, Duration, Utc};
use llm_access_core::store::{
    ProviderKiroAuthUpdate, ProviderKiroRoute, ProviderProxyConfig, ProviderRouteStore,
    KEY_STATUS_ACTIVE, KEY_STATUS_DISABLED,
};
use llm_access_kiro::{
    auth_file::{
        KiroAuthRecord, DEFAULT_KIRO_VERSION, DEFAULT_NODE_VERSION, DEFAULT_SYSTEM_VERSION,
    },
    machine_id,
    wire::{
        IdcRefreshRequest, IdcRefreshResponse, RefreshRequest, RefreshResponse, UsageLimitsResponse,
    },
};
use serde_json::Value;

use crate::kiro_headers;

const REFRESH_EARLY_MINUTES: i64 = 10;
const KIRO_USAGE_AWS_SDK_VERSION: &str = "1.0.0";
const KIRO_PROFILES_AWS_SDK_VERSION: &str = "1.0.0";
const KIRO_IDC_AWS_SDK_VERSION: &str = "3.980.0";
const KIRO_IDC_AMZ_SDK_REQUEST: &str = "attempt=1; max=4";
const KIRO_UPSTREAM_BASE_URL_ENV: &str = "KIRO_UPSTREAM_BASE_URL";
const KIRO_RUNTIME_UPSTREAM_BASE_URL_ENV: &str = "KIRO_RUNTIME_UPSTREAM_BASE_URL";
const KIRO_MANAGEMENT_UPSTREAM_BASE_URL_ENV: &str = "KIRO_MANAGEMENT_UPSTREAM_BASE_URL";
const KIRO_SOCIAL_SIGN_IN_PROFILE_ARN: &str =
    "arn:aws:codewhisperer:us-east-1:699475941385:profile/EHGA3GRVQMUK";
const KIRO_BUILDER_ID_PROFILE_ARN: &str =
    "arn:aws:codewhisperer:us-east-1:638616132270:profile/AAAACCCCXXXX";
const KIRO_STANDARD_PROFILE_REGIONS: &[&str] = &[
    "us-east-1",
    "eu-central-1",
    "us-gov-east-1",
    "us-gov-west-1",
    "us-iso-east-1",
    "us-isob-east-1",
    "us-isof-south-1",
    "us-isof-east-1",
];

static REFRESH_LOCKS: LazyLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Permanent refresh-token failure returned by Kiro OAuth/OIDC refresh APIs.
#[derive(Debug)]
struct RefreshTokenInvalidGrantError {
    message: String,
}

impl std::fmt::Display for RefreshTokenInvalidGrantError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for RefreshTokenInvalidGrantError {}

#[derive(Debug, Clone)]
pub(crate) struct KiroCallContext {
    pub auth: KiroAuthRecord,
    pub access_token: String,
}

pub(crate) async fn ensure_context_for_route(
    route: &ProviderKiroRoute,
    store: &dyn ProviderRouteStore,
    force_refresh: bool,
) -> anyhow::Result<KiroCallContext> {
    ensure_context_for_route_with_profile_requirement(
        route,
        store,
        force_refresh,
        ProfileArnRequirement::Optional,
    )
    .await
}

pub(crate) async fn ensure_context_for_route_requiring_profile(
    route: &ProviderKiroRoute,
    store: &dyn ProviderRouteStore,
    force_refresh: bool,
) -> anyhow::Result<KiroCallContext> {
    ensure_context_for_route_with_profile_requirement(
        route,
        store,
        force_refresh,
        ProfileArnRequirement::Required,
    )
    .await
}

async fn ensure_context_for_route_with_profile_requirement(
    route: &ProviderKiroRoute,
    store: &dyn ProviderRouteStore,
    force_refresh: bool,
    profile_requirement: ProfileArnRequirement,
) -> anyhow::Result<KiroCallContext> {
    let auth = parse_route_auth(route)?;
    if !force_refresh
        && !needs_refresh(&auth)
        && !profile_resolution_needed(&auth, profile_requirement)
    {
        let access_token = non_empty_access_token(&auth)?;
        return Ok(KiroCallContext {
            auth,
            access_token,
        });
    }

    let refresh_lock = refresh_lock_for_account(&route.account_name)?;
    let _guard = refresh_lock.lock().await;
    let latest = parse_route_auth(route)?;
    if !force_refresh && !needs_refresh(&latest) {
        let (resolved, changed) =
            resolve_profile_arn_for_auth(route, &latest, profile_requirement).await?;
        if changed {
            persist_active_kiro_auth_update(route, store, &resolved).await?;
        }
        let access_token = non_empty_access_token(&resolved)?;
        return Ok(KiroCallContext {
            auth: resolved,
            access_token,
        });
    }

    let refreshed = match refresh_auth(route, &latest).await {
        Ok(refreshed) => refreshed,
        Err(err) => {
            if let Some(invalid_refresh) = err.downcast_ref::<RefreshTokenInvalidGrantError>() {
                let mut disabled = latest.clone();
                disabled.disabled = true;
                disabled.disabled_reason = Some("invalid_refresh_token".to_string());
                store
                    .save_kiro_auth_update(ProviderKiroAuthUpdate {
                        account_name: route.account_name.clone(),
                        auth_json: refreshed_auth_json(&route.auth_json, &disabled)
                            .context("serialize disabled kiro auth")?,
                        auth_method: disabled.auth_method().to_string(),
                        account_id: account_id_from_auth_json(&route.auth_json),
                        profile_arn: disabled.profile_arn.clone(),
                        user_id: user_id_from_auth_json(&route.auth_json),
                        status: KEY_STATUS_DISABLED.to_string(),
                        last_error: Some(invalid_refresh.to_string()),
                        refreshed_at_ms: now_ms(),
                    })
                    .await?;
            }
            return Err(err);
        },
    };
    let (refreshed, _changed) =
        resolve_profile_arn_for_auth(route, &refreshed, profile_requirement).await?;
    let access_token = non_empty_access_token(&refreshed)?;
    persist_active_kiro_auth_update(route, store, &refreshed).await?;
    Ok(KiroCallContext {
        auth: refreshed,
        access_token,
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ProfileArnRequirement {
    Optional,
    Required,
}

fn profile_resolution_needed(auth: &KiroAuthRecord, requirement: ProfileArnRequirement) -> bool {
    current_profile_arn(auth).is_none()
        && (matches!(requirement, ProfileArnRequirement::Required) || supports_profiles(auth))
}

fn current_profile_arn(auth: &KiroAuthRecord) -> Option<&str> {
    auth.profile_arn
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

async fn resolve_profile_arn_for_auth(
    route: &ProviderKiroRoute,
    auth: &KiroAuthRecord,
    requirement: ProfileArnRequirement,
) -> anyhow::Result<(KiroAuthRecord, bool)> {
    if current_profile_arn(auth).is_some() {
        return Ok((auth.clone(), false));
    }
    if !supports_profiles(auth) {
        if matches!(requirement, ProfileArnRequirement::Required) {
            bail!("profileArn is required but could not be resolved");
        }
        return Ok((auth.clone(), false));
    }

    if let Some(profile_arn) = fixed_profile_arn(auth) {
        let mut resolved = auth.clone();
        resolved.profile_arn = Some(profile_arn.to_string());
        return Ok((resolved, true));
    }

    let fetched = match fetch_profile_arn_from_backend(route, auth).await {
        Ok(profile_arn) => profile_arn,
        Err(err) => {
            if matches!(requirement, ProfileArnRequirement::Optional) {
                return Ok((auth.clone(), false));
            }
            return Err(err.context("profileArn is required but could not be resolved"));
        },
    };

    let Some(profile_arn) = fetched else {
        if matches!(requirement, ProfileArnRequirement::Required) {
            bail!("profileArn is required but could not be resolved");
        }
        return Ok((auth.clone(), false));
    };

    let mut resolved = auth.clone();
    resolved.profile_arn = Some(profile_arn);
    Ok((resolved, true))
}

fn supports_profiles(auth: &KiroAuthRecord) -> bool {
    let provider = normalized_token_provider(auth);
    let is_idc_provider =
        matches!(provider.as_deref(), Some("Enterprise" | "Internal" | "BuilderId"));
    let is_external_idp =
        auth.auth_method() == "external_idp" || matches!(provider.as_deref(), Some("ExternalIdp"));
    let is_social = auth.auth_method() == "social";
    is_idc_provider || is_external_idp || is_social
}

fn fixed_profile_arn(auth: &KiroAuthRecord) -> Option<&'static str> {
    match normalized_token_provider(auth).as_deref() {
        Some("BuilderId") => Some(KIRO_BUILDER_ID_PROFILE_ARN),
        Some("Github" | "Google") => Some(KIRO_SOCIAL_SIGN_IN_PROFILE_ARN),
        _ => None,
    }
}

fn normalized_token_provider(auth: &KiroAuthRecord) -> Option<String> {
    let provider = auth
        .provider
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let normalized = if provider.eq_ignore_ascii_case("github") {
        "Github"
    } else if provider.eq_ignore_ascii_case("google") {
        "Google"
    } else if provider.eq_ignore_ascii_case("builderid")
        || provider.eq_ignore_ascii_case("builder-id")
        || provider.eq_ignore_ascii_case("aws")
    {
        "BuilderId"
    } else if provider.eq_ignore_ascii_case("enterprise") {
        "Enterprise"
    } else if provider.eq_ignore_ascii_case("internal") {
        "Internal"
    } else if provider.eq_ignore_ascii_case("externalidp")
        || provider.eq_ignore_ascii_case("external_idp")
    {
        "ExternalIdp"
    } else {
        provider
    };
    Some(normalized.to_string())
}

async fn fetch_profile_arn_from_backend(
    route: &ProviderKiroRoute,
    auth: &KiroAuthRecord,
) -> anyhow::Result<Option<String>> {
    let client = provider_client(route.proxy.as_ref())?;
    let access_token = non_empty_access_token(auth)?;
    let mut profiles = Vec::new();
    for region in profile_regions_to_query(auth) {
        let region_profiles =
            match fetch_profile_candidates_for_region(&client, auth, &access_token, region).await {
                Ok(region_profiles) => region_profiles,
                Err(_) => continue,
            };
        profiles.extend(region_profiles);
    }
    Ok(profiles
        .into_iter()
        .filter_map(|profile| profile.arn.map(|arn| arn.trim().to_string()))
        .find(|arn| !arn.is_empty()))
}

async fn fetch_profile_candidates_for_region(
    client: &reqwest::Client,
    auth: &KiroAuthRecord,
    access_token: &str,
    region: &str,
) -> anyhow::Result<Vec<ListAvailableProfile>> {
    let upstream_base = codewhisperer_profiles_base_url(region)?;
    let upstream_url = format!("{upstream_base}/ListAvailableProfiles");
    let host = upstream_host_header(&upstream_url)?;
    let mut next_token: Option<String> = None;
    let mut profiles = Vec::new();
    loop {
        let request_body = serde_json::to_vec(&ListAvailableProfilesRequest {
            next_token: next_token.clone(),
        })
        .context("encode ListAvailableProfiles request")?;
        let response = kiro_headers::add_kiro_headers(
            client.post(&upstream_url),
            auth,
            kiro_headers::KiroHeaderConfig {
                upstream_host: &host,
                access_token,
                service: kiro_headers::KiroAwsService::Runtime,
                client_version: KIRO_PROFILES_AWS_SDK_VERSION,
                sdk_request: "attempt=1; max=1",
                content_type: Some("application/json"),
                accept: Some("application/json"),
                connection_close: true,
                agent_mode: None,
                include_opt_out: false,
            },
        )?
        .body(request_body)
        .send()
        .await
        .with_context(|| format!("request kiro ListAvailableProfiles for region {region}"))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!("kiro ListAvailableProfiles failed for {region}: {status} {body}");
        }
        let payload: ListAvailableProfilesResponse = response
            .json()
            .await
            .with_context(|| format!("parse kiro ListAvailableProfiles response for {region}"))?;
        profiles.extend(payload.profiles.into_iter());
        if let Some(token) = payload
            .next_token
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            next_token = Some(token.to_string());
            continue;
        }
        return Ok(profiles);
    }
}

fn profile_regions_to_query(auth: &KiroAuthRecord) -> Vec<&'static str> {
    if auth.auth_method() == "external_idp" {
        return KIRO_STANDARD_PROFILE_REGIONS.to_vec();
    }
    let idc_region = auth.effective_auth_region();
    let idc_partition = aws_partition_for_region(idc_region);
    let filtered: Vec<&'static str> = KIRO_STANDARD_PROFILE_REGIONS
        .iter()
        .copied()
        .filter(|region| aws_partition_for_region(region) == idc_partition)
        .collect();
    if filtered.is_empty() {
        vec!["us-east-1", "eu-central-1"]
    } else {
        filtered
    }
}

fn aws_partition_for_region(region: &str) -> &'static str {
    if region.starts_with("cn-") {
        "aws-cn"
    } else if region.starts_with("us-gov-") {
        "aws-us-gov"
    } else if region.starts_with("us-isof-") {
        "aws-iso-f"
    } else if region.starts_with("us-isob-") {
        "aws-iso-b"
    } else if region.starts_with("us-iso-") {
        "aws-iso"
    } else {
        "aws"
    }
}

fn codewhisperer_profiles_base_url(region: &str) -> anyhow::Result<String> {
    if let Some(overridden) = read_upstream_env(KIRO_UPSTREAM_BASE_URL_ENV) {
        return Ok(overridden);
    }
    let url = match region {
        "us-east-1" => "https://q.us-east-1.amazonaws.com",
        "eu-central-1" => "https://q.eu-central-1.amazonaws.com",
        "us-gov-east-1" => "https://q-fips.us-gov-east-1.amazonaws.com",
        "us-gov-west-1" => "https://q-fips.us-gov-west-1.amazonaws.com",
        "us-iso-east-1" => "https://q.us-iso-east-1.c2s.ic.gov",
        "us-isob-east-1" => "https://q.us-isob-east-1.sc2s.sgov.gov",
        "us-isof-south-1" => "https://q.us-isof-south-1.csp.hci.ic.gov",
        "us-isof-east-1" => "https://q.us-isof-east-1.csp.hci.ic.gov",
        _ => bail!("unsupported CodeWhisperer profile region: {region}"),
    };
    Ok(url.to_string())
}

async fn persist_active_kiro_auth_update(
    route: &ProviderKiroRoute,
    store: &dyn ProviderRouteStore,
    auth: &KiroAuthRecord,
) -> anyhow::Result<()> {
    store
        .save_kiro_auth_update(ProviderKiroAuthUpdate {
            account_name: route.account_name.clone(),
            auth_json: refreshed_auth_json(&route.auth_json, auth)
                .context("serialize refreshed kiro auth")?,
            auth_method: auth.auth_method().to_string(),
            account_id: account_id_from_auth_json(&route.auth_json),
            profile_arn: auth.profile_arn.clone(),
            user_id: user_id_from_auth_json(&route.auth_json),
            status: KEY_STATUS_ACTIVE.to_string(),
            last_error: None,
            refreshed_at_ms: now_ms(),
        })
        .await
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ListAvailableProfilesRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    next_token: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListAvailableProfilesResponse {
    #[serde(default)]
    profiles: Vec<ListAvailableProfile>,
    #[serde(default)]
    next_token: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListAvailableProfile {
    #[serde(default)]
    arn: Option<String>,
}

pub(crate) async fn fetch_usage_limits_for_route(
    route: &ProviderKiroRoute,
    store: &dyn ProviderRouteStore,
    force_refresh: bool,
) -> anyhow::Result<UsageLimitsResponse> {
    let ctx = ensure_context_for_route_requiring_profile(route, store, force_refresh).await?;
    let region = ctx.auth.effective_api_region().to_string();
    let upstream_base = management_upstream_base_url(&region);
    let mut url =
        format!("{upstream_base}/getUsageLimits?origin=AI_EDITOR&resourceType=AGENTIC_REQUEST");
    if let Some(profile_arn) = ctx
        .auth
        .profile_arn
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        let encoded =
            url::form_urlencoded::byte_serialize(profile_arn.as_bytes()).collect::<String>();
        url.push_str("&profileArn=");
        url.push_str(&encoded);
    }
    let client = provider_client(route.proxy.as_ref())?;
    let host = upstream_host_header(&url)?;
    let response = kiro_headers::add_kiro_headers(
        client.get(url),
        &ctx.auth,
        kiro_headers::KiroHeaderConfig {
            upstream_host: &host,
            access_token: &ctx.access_token,
            service: kiro_headers::KiroAwsService::Runtime,
            client_version: KIRO_USAGE_AWS_SDK_VERSION,
            sdk_request: "attempt=1; max=1",
            content_type: None,
            accept: None,
            connection_close: true,
            agent_mode: None,
            include_opt_out: false,
        },
    )?
    .send()
    .await
    .context("request kiro usage limits")?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        bail!("kiro usage limit request failed: {status} {body}");
    }
    response.json().await.context("parse kiro usage limits")
}

pub(crate) fn runtime_upstream_base_url(region: &str) -> String {
    configured_upstream_base_url(
        KIRO_RUNTIME_UPSTREAM_BASE_URL_ENV,
        format!("https://runtime.{region}.kiro.dev"),
    )
}

pub(crate) fn management_upstream_base_url(region: &str) -> String {
    configured_upstream_base_url(
        KIRO_MANAGEMENT_UPSTREAM_BASE_URL_ENV,
        format!("https://management.{region}.kiro.dev"),
    )
}

pub(crate) fn upstream_host_header(upstream_url: &str) -> anyhow::Result<String> {
    let parsed = reqwest::Url::parse(upstream_url)
        .with_context(|| format!("parse kiro upstream url: {upstream_url}"))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| anyhow!("kiro upstream url missing host: {upstream_url}"))?;
    Ok(match parsed.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    })
}

fn configured_upstream_base_url(split_env_var: &str, default: String) -> String {
    read_upstream_env(split_env_var)
        .or_else(|| read_upstream_env(KIRO_UPSTREAM_BASE_URL_ENV))
        .unwrap_or(default)
}

fn read_upstream_env(env_var: &str) -> Option<String> {
    std::env::var(env_var)
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
}

fn refreshed_auth_json(original_json: &str, refreshed: &KiroAuthRecord) -> anyhow::Result<String> {
    let mut original = serde_json::from_str::<Value>(original_json)
        .unwrap_or_else(|_| Value::Object(serde_json::Map::new()));
    let refreshed = serde_json::to_value(refreshed)?;
    let Some(original_object) = original.as_object_mut() else {
        return serde_json::to_string(&refreshed).context("serialize refreshed kiro auth object");
    };
    if let Some(refreshed_object) = refreshed.as_object() {
        for (key, value) in refreshed_object {
            original_object.insert(key.clone(), value.clone());
        }
    }
    serde_json::to_string(&original).context("serialize merged refreshed kiro auth object")
}

fn parse_route_auth(route: &ProviderKiroRoute) -> anyhow::Result<KiroAuthRecord> {
    let mut value: Value =
        serde_json::from_str(&route.auth_json).context("parse kiro auth json")?;
    if let Some(object) = value.as_object_mut() {
        object
            .entry("name".to_string())
            .or_insert_with(|| Value::String(route.account_name.clone()));
    }
    let mut auth: KiroAuthRecord =
        serde_json::from_value(value).context("parse kiro auth record")?;
    if auth.name.trim().is_empty() {
        auth.name = route.account_name.clone();
    }
    if auth.profile_arn.is_none() {
        auth.profile_arn = route.profile_arn.clone();
    }
    if auth.api_region.is_none() {
        auth.api_region = Some(route.api_region.clone());
    }
    Ok(auth.canonicalize())
}

fn non_empty_access_token(auth: &KiroAuthRecord) -> anyhow::Result<String> {
    auth.access_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| anyhow!("kiro access token missing"))
}

fn needs_refresh(auth: &KiroAuthRecord) -> bool {
    let Some(access_token) = auth.access_token.as_deref().map(str::trim) else {
        return true;
    };
    if access_token.is_empty() {
        return true;
    }
    let expires_at = auth
        .expires_at
        .as_deref()
        .and_then(parse_rfc3339_utc)
        .or_else(|| access_token_expiry(access_token));
    let Some(expires_at) = expires_at else {
        return false;
    };
    expires_at <= Utc::now() + Duration::minutes(REFRESH_EARLY_MINUTES)
}

async fn refresh_auth(
    route: &ProviderKiroRoute,
    auth: &KiroAuthRecord,
) -> anyhow::Result<KiroAuthRecord> {
    validate_refresh_token(auth)?;
    let method = auth.auth_method();
    if matches!(method, "idc" | "builder-id" | "iam") {
        refresh_idc(route, auth).await
    } else {
        refresh_social(route, auth).await
    }
}

fn validate_refresh_token(auth: &KiroAuthRecord) -> anyhow::Result<()> {
    let refresh_token = auth
        .refresh_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("missing kiro refresh token"))?;
    if refresh_token.len() < 100 || refresh_token.ends_with("...") || refresh_token.contains("...")
    {
        bail!("kiro refresh token appears truncated");
    }
    Ok(())
}

async fn refresh_social(
    route: &ProviderKiroRoute,
    auth: &KiroAuthRecord,
) -> anyhow::Result<KiroAuthRecord> {
    let refresh_token = auth
        .refresh_token
        .clone()
        .ok_or_else(|| anyhow!("missing refresh token"))?;
    let region = auth.effective_auth_region();
    let url = format!("https://prod.{region}.auth.desktop.kiro.dev/refreshToken");
    let host = format!("prod.{region}.auth.desktop.kiro.dev");
    let client = provider_client(route.proxy.as_ref())?;
    let machine_id = machine_id::generate_from_auth(auth)
        .ok_or_else(|| anyhow!("failed to derive kiro machine id"))?;
    let response = client
        .post(url)
        .header("accept", "application/json, text/plain, */*")
        .header("content-type", "application/json")
        .header("user-agent", social_refresh_user_agent(&machine_id))
        .header("accept-encoding", "gzip, compress, deflate, br")
        .header("host", host)
        .header("connection", "close")
        .json(&RefreshRequest {
            refresh_token,
        })
        .send()
        .await
        .context("refresh kiro social token")?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if is_invalid_refresh_token_grant(status.as_u16(), &body) {
            return Err(RefreshTokenInvalidGrantError {
                message: format!("kiro social refresh token is invalid: {status} {body}"),
            }
            .into());
        }
        bail!("kiro social token refresh failed: {status} {body}");
    }
    let payload: RefreshResponse = response.json().await.context("parse refresh response")?;
    let mut next_auth = auth.clone();
    next_auth.access_token = Some(payload.access_token);
    if let Some(refresh_token) = payload.refresh_token {
        next_auth.refresh_token = Some(refresh_token);
    }
    if let Some(profile_arn) = payload.profile_arn {
        next_auth.profile_arn = Some(profile_arn);
    }
    next_auth.expires_at =
        derive_refreshed_expires_at(next_auth.access_token.as_deref(), payload.expires_in);
    Ok(next_auth)
}

async fn refresh_idc(
    route: &ProviderKiroRoute,
    auth: &KiroAuthRecord,
) -> anyhow::Result<KiroAuthRecord> {
    let refresh_token = auth
        .refresh_token
        .clone()
        .ok_or_else(|| anyhow!("missing refresh token"))?;
    let client_id = auth
        .client_id
        .clone()
        .ok_or_else(|| anyhow!("missing kiro clientId"))?;
    let client_secret = auth
        .client_secret
        .clone()
        .ok_or_else(|| anyhow!("missing kiro clientSecret"))?;
    let region = auth.effective_auth_region();
    let client = provider_client(route.proxy.as_ref())?;
    let response = client
        .post(format!("https://oidc.{region}.amazonaws.com/token"))
        .header("content-type", "application/json")
        .header("host", format!("oidc.{region}.amazonaws.com"))
        .header("amz-sdk-invocation-id", uuid::Uuid::new_v4().to_string())
        .header("amz-sdk-request", KIRO_IDC_AMZ_SDK_REQUEST)
        .header("connection", "close")
        .header("x-amz-user-agent", idc_refresh_amz_user_agent())
        .header("accept", "*/*")
        .header("user-agent", idc_refresh_user_agent())
        .json(&IdcRefreshRequest {
            client_id,
            client_secret,
            refresh_token,
            grant_type: "refresh_token".to_string(),
        })
        .send()
        .await
        .context("refresh kiro idc token")?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if is_invalid_refresh_token_grant(status.as_u16(), &body) {
            return Err(RefreshTokenInvalidGrantError {
                message: format!("kiro idc refresh token is invalid: {status} {body}"),
            }
            .into());
        }
        bail!("kiro idc token refresh failed: {status} {body}");
    }
    let payload: IdcRefreshResponse = response.json().await.context("parse idc refresh")?;
    let mut next_auth = auth.clone();
    next_auth.access_token = Some(payload.access_token);
    if let Some(refresh_token) = payload.refresh_token {
        next_auth.refresh_token = Some(refresh_token);
    }
    if let Some(profile_arn) = payload.profile_arn {
        next_auth.profile_arn = Some(profile_arn);
    }
    next_auth.expires_at =
        derive_refreshed_expires_at(next_auth.access_token.as_deref(), payload.expires_in);
    Ok(next_auth)
}

fn provider_client(proxy: Option<&ProviderProxyConfig>) -> anyhow::Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder();
    if let Some(proxy_config) = proxy {
        let mut proxy = reqwest::Proxy::all(&proxy_config.proxy_url)?;
        if let Some(username) = proxy_config.proxy_username.as_deref() {
            proxy =
                proxy.basic_auth(username, proxy_config.proxy_password.as_deref().unwrap_or(""));
        }
        builder = builder.proxy(proxy);
    }
    Ok(builder.build()?)
}

fn derive_refreshed_expires_at(
    access_token: Option<&str>,
    expires_in: Option<i64>,
) -> Option<String> {
    if let Some(expires_in) = expires_in.filter(|value| *value > 0) {
        return Some((Utc::now() + Duration::seconds(expires_in)).to_rfc3339());
    }
    access_token.and_then(jwt_exp_to_rfc3339)
}

fn jwt_exp_to_rfc3339(token: &str) -> Option<String> {
    access_token_expiry(token).map(|value| value.to_rfc3339())
}

fn access_token_expiry(token: &str) -> Option<DateTime<Utc>> {
    let payload = token.split('.').nth(1)?;
    let decoded = URL_SAFE_NO_PAD.decode(payload.as_bytes()).ok()?;
    let value: Value = serde_json::from_slice(&decoded).ok()?;
    let exp = value.get("exp")?.as_i64()?;
    DateTime::from_timestamp(exp, 0)
}

fn parse_rfc3339_utc(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

fn is_invalid_refresh_token_grant(status: u16, body: &str) -> bool {
    status == 400
        && body.contains("\"invalid_grant\"")
        && body.contains("Invalid refresh token provided")
}

fn refresh_lock_for_account(account_name: &str) -> anyhow::Result<Arc<tokio::sync::Mutex<()>>> {
    let mut locks = REFRESH_LOCKS
        .lock()
        .map_err(|_| anyhow!("kiro refresh lock registry poisoned"))?;
    Ok(locks
        .entry(account_name.to_string())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone())
}

fn social_refresh_user_agent(machine_id: &str) -> String {
    format!("KiroIDE-{DEFAULT_KIRO_VERSION}-{machine_id}")
}

fn idc_refresh_amz_user_agent() -> String {
    format!("aws-sdk-js/{KIRO_IDC_AWS_SDK_VERSION} KiroIDE-{DEFAULT_KIRO_VERSION}")
}

fn idc_refresh_user_agent() -> String {
    format!(
        "aws-sdk-js/{KIRO_IDC_AWS_SDK_VERSION} ua/2.1 os/{DEFAULT_SYSTEM_VERSION} lang/js \
         md/nodejs#{DEFAULT_NODE_VERSION} m/E"
    )
}

fn account_id_from_auth_json(auth_json: &str) -> Option<String> {
    let value = serde_json::from_str::<Value>(auth_json).ok()?;
    optional_json_string(&value, &["accountId", "account_id"])
}

fn user_id_from_auth_json(auth_json: &str) -> Option<String> {
    let value = serde_json::from_str::<Value>(auth_json).ok()?;
    optional_json_string(&value, &["userId", "user_id"])
}

fn optional_json_string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

#[cfg(test)]
#[allow(
    clippy::await_holding_lock,
    reason = "Kiro refresh tests serialize process-wide upstream env var overrides across awaited \
              requests"
)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::{
        extract::{Query, State},
        response::IntoResponse,
        routing::{get, post},
        Json, Router,
    };
    use llm_access_core::store::{
        EmptyProviderRouteStore, ProviderKiroAuthUpdate, ProviderKiroRoute,
    };
    use serde_json::json;

    #[derive(Default)]
    struct CaptureStore {
        updates: Mutex<Vec<ProviderKiroAuthUpdate>>,
    }

    #[async_trait::async_trait]
    impl llm_access_core::store::ProviderRouteStore for CaptureStore {
        async fn resolve_codex_route(
            &self,
            _key: &llm_access_core::store::AuthenticatedKey,
        ) -> anyhow::Result<Option<llm_access_core::store::ProviderCodexRoute>> {
            Ok(None)
        }

        async fn resolve_codex_account_route(
            &self,
            _account_name: &str,
        ) -> anyhow::Result<Option<llm_access_core::store::ProviderCodexRoute>> {
            Ok(None)
        }

        async fn resolve_kiro_route(
            &self,
            _key: &llm_access_core::store::AuthenticatedKey,
        ) -> anyhow::Result<Option<ProviderKiroRoute>> {
            Ok(None)
        }

        async fn save_kiro_auth_update(
            &self,
            update: ProviderKiroAuthUpdate,
        ) -> anyhow::Result<()> {
            self.updates.lock().expect("updates").push(update);
            Ok(())
        }

        async fn save_codex_auth_update(
            &self,
            _update: llm_access_core::store::ProviderCodexAuthUpdate,
        ) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct CapturedProfileFallback {
        usage_queries: Mutex<Vec<Option<String>>>,
        list_profile_calls: Mutex<usize>,
    }

    fn static_kiro_route_with_auth_json(auth_json: &str) -> ProviderKiroRoute {
        ProviderKiroRoute {
            account_name: "kiro-a".to_string(),
            account_group_id_at_event: None,
            route_strategy_at_event: llm_access_core::provider::RouteStrategy::Auto,
            auth_json: auth_json.to_string(),
            profile_arn: None,
            api_region: "us-east-1".to_string(),
            request_validation_enabled: true,
            cache_estimation_enabled: true,
            zero_cache_debug_enabled: false,
            full_request_logging_enabled: false,
            remote_media_resolution_enabled: false,
            latency_routing_enabled: true,
            model_name_map_json: "{}".to_string(),
            cache_kmodels_json: llm_access_core::store::default_kiro_cache_kmodels_json(),
            cache_policy_json: llm_access_core::store::default_kiro_cache_policy_json(),
            context_usage_min_request_tokens:
                llm_access_core::store::DEFAULT_KIRO_CONTEXT_USAGE_MIN_REQUEST_TOKENS,
            prefix_cache_mode: llm_access_core::store::DEFAULT_KIRO_PREFIX_CACHE_MODE.to_string(),
            prefix_cache_max_tokens: llm_access_core::store::DEFAULT_KIRO_PREFIX_CACHE_MAX_TOKENS,
            prefix_cache_entry_ttl_seconds:
                llm_access_core::store::DEFAULT_KIRO_PREFIX_CACHE_ENTRY_TTL_SECONDS,
            conversation_anchor_max_entries:
                llm_access_core::store::DEFAULT_KIRO_CONVERSATION_ANCHOR_MAX_ENTRIES,
            conversation_anchor_ttl_seconds:
                llm_access_core::store::DEFAULT_KIRO_CONVERSATION_ANCHOR_TTL_SECONDS,
            billable_model_multipliers_json:
                llm_access_core::store::default_kiro_billable_model_multipliers_json(),
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
            account_request_max_concurrency: None,
            account_request_min_start_interval_ms: None,
            proxy: None,
            routing_identity: "kiro-a".to_string(),
            cached_status: None,
            cached_remaining_credits: None,
            cached_balance: None,
            cached_cache: None,
            status_refresh_interval_seconds: 300,
            minimum_remaining_credits_before_block: 0.0,
        }
    }

    async fn fake_usage_limits(
        State(captured): State<Arc<CapturedProfileFallback>>,
        Query(query): Query<std::collections::HashMap<String, String>>,
    ) -> impl IntoResponse {
        captured
            .usage_queries
            .lock()
            .expect("usage queries")
            .push(query.get("profileArn").cloned());
        Json(json!({
            "subscriptionInfo": {"subscriptionTitle": "Pro"},
            "usageBreakdownList": [{
                "currentUsageWithPrecision": 1.0,
                "usageLimitWithPrecision": 100.0,
                "bonuses": [],
                "nextDateReset": 900.0
            }],
            "userInfo": {"userId": "user-1"}
        }))
    }

    async fn fake_list_available_profiles(
        State(captured): State<Arc<CapturedProfileFallback>>,
    ) -> impl IntoResponse {
        *captured
            .list_profile_calls
            .lock()
            .expect("list profile calls") += 1;
        Json(json!({
            "profiles": [{
                "arn": "arn:aws:codewhisperer:us-east-1:123456789012:profile/PROFILE1",
                "name": "Default"
            }]
        }))
    }

    async fn spawn_profile_fallback_upstream(captured: Arc<CapturedProfileFallback>) -> String {
        let app = Router::new()
            .route("/getUsageLimits", get(fake_usage_limits))
            .route("/ListAvailableProfiles", post(fake_list_available_profiles))
            .with_state(captured);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind profile fallback upstream");
        let addr = listener.local_addr().expect("local addr");
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve profile fallback upstream");
        });
        format!("http://{}", addr)
    }

    fn clear_kiro_upstream_envs() {
        std::env::remove_var("KIRO_RUNTIME_UPSTREAM_BASE_URL");
        std::env::remove_var("KIRO_MANAGEMENT_UPSTREAM_BASE_URL");
        std::env::remove_var("KIRO_UPSTREAM_BASE_URL");
    }

    #[test]
    fn invalid_refresh_grant_detection_matches_kiro_refresh_contract() {
        let body =
            r#"{"error":"invalid_grant","error_description":"Invalid refresh token provided"}"#;

        assert!(super::is_invalid_refresh_token_grant(400, body));
        assert!(!super::is_invalid_refresh_token_grant(401, body));
        assert!(!super::is_invalid_refresh_token_grant(400, r#"{"error":"invalid_client"}"#));
    }

    #[test]
    fn kiro_upstream_defaults_split_runtime_and_management_domains() {
        let _guard = crate::KIRO_UPSTREAM_ENV_LOCK
            .lock()
            .expect("kiro upstream env lock");
        clear_kiro_upstream_envs();

        assert_eq!(
            super::runtime_upstream_base_url("us-east-1"),
            "https://runtime.us-east-1.kiro.dev"
        );
        assert_eq!(
            super::management_upstream_base_url("us-east-1"),
            "https://management.us-east-1.kiro.dev"
        );
    }

    #[test]
    fn kiro_legacy_upstream_env_still_overrides_both_endpoint_families() {
        let _guard = crate::KIRO_UPSTREAM_ENV_LOCK
            .lock()
            .expect("kiro upstream env lock");
        clear_kiro_upstream_envs();
        std::env::set_var("KIRO_UPSTREAM_BASE_URL", "http://127.0.0.1:19090/");

        assert_eq!(super::runtime_upstream_base_url("us-east-1"), "http://127.0.0.1:19090");
        assert_eq!(super::management_upstream_base_url("us-east-1"), "http://127.0.0.1:19090");

        clear_kiro_upstream_envs();
    }

    #[test]
    fn kiro_split_upstream_envs_override_legacy_and_drive_host_headers() {
        let _guard = crate::KIRO_UPSTREAM_ENV_LOCK
            .lock()
            .expect("kiro upstream env lock");
        clear_kiro_upstream_envs();
        std::env::set_var("KIRO_UPSTREAM_BASE_URL", "https://q.us-east-1.amazonaws.com");
        std::env::set_var("KIRO_RUNTIME_UPSTREAM_BASE_URL", "https://runtime.us-east-1.kiro.dev/");
        std::env::set_var("KIRO_MANAGEMENT_UPSTREAM_BASE_URL", "http://127.0.0.1:19091/");

        assert_eq!(
            super::runtime_upstream_base_url("us-east-1"),
            "https://runtime.us-east-1.kiro.dev"
        );
        assert_eq!(super::management_upstream_base_url("us-east-1"), "http://127.0.0.1:19091");
        assert_eq!(
            super::upstream_host_header("https://runtime.us-east-1.kiro.dev/mcp")
                .expect("runtime host"),
            "runtime.us-east-1.kiro.dev"
        );
        assert_eq!(
            super::upstream_host_header("http://127.0.0.1:19091/getUsageLimits")
                .expect("management host"),
            "127.0.0.1:19091"
        );

        clear_kiro_upstream_envs();
    }

    #[tokio::test]
    async fn usage_refresh_uses_fixed_builder_id_profile_arn_and_persists_it() {
        let _guard = crate::KIRO_UPSTREAM_ENV_LOCK
            .lock()
            .expect("kiro upstream env lock");
        clear_kiro_upstream_envs();
        let captured = Arc::new(CapturedProfileFallback::default());
        let upstream_base = spawn_profile_fallback_upstream(captured.clone()).await;
        std::env::set_var("KIRO_UPSTREAM_BASE_URL", &upstream_base);

        let route = static_kiro_route_with_auth_json(
            r#"{
                "accessToken":"kiro-upstream-token",
                "machineId":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "authMethod":"idc",
                "provider":"aws"
            }"#,
        );
        let store = Arc::new(CaptureStore::default());

        let _ = super::fetch_usage_limits_for_route(&route, store.as_ref(), false)
            .await
            .expect("usage refresh should succeed");

        let usage_queries = captured.usage_queries.lock().expect("usage queries");
        assert_eq!(usage_queries.as_slice(), &[Some(
            "arn:aws:codewhisperer:us-east-1:638616132270:profile/AAAACCCCXXXX".to_string()
        )]);
        drop(usage_queries);

        let updates = store.updates.lock().expect("updates");
        assert_eq!(updates.len(), 1);
        assert_eq!(
            updates[0].profile_arn.as_deref(),
            Some("arn:aws:codewhisperer:us-east-1:638616132270:profile/AAAACCCCXXXX")
        );
        assert!(updates[0].auth_json.contains("AAAACCCCXXXX"));
        drop(updates);
        clear_kiro_upstream_envs();
    }

    #[tokio::test]
    async fn usage_refresh_fetches_profile_from_list_available_profiles_for_external_idp() {
        let _guard = crate::KIRO_UPSTREAM_ENV_LOCK
            .lock()
            .expect("kiro upstream env lock");
        clear_kiro_upstream_envs();
        let captured = Arc::new(CapturedProfileFallback::default());
        let upstream_base = spawn_profile_fallback_upstream(captured.clone()).await;
        std::env::set_var("KIRO_UPSTREAM_BASE_URL", &upstream_base);

        let route = static_kiro_route_with_auth_json(
            r#"{
                "accessToken":"kiro-upstream-token",
                "machineId":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "authMethod":"external_idp",
                "provider":"Internal"
            }"#,
        );

        let _ = super::fetch_usage_limits_for_route(&route, &EmptyProviderRouteStore, false)
            .await
            .expect("usage refresh should succeed");

        assert_eq!(
            *captured
                .list_profile_calls
                .lock()
                .expect("list profile calls"),
            super::KIRO_STANDARD_PROFILE_REGIONS.len()
        );
        let usage_queries = captured.usage_queries.lock().expect("usage queries");
        assert_eq!(usage_queries.as_slice(), &[Some(
            "arn:aws:codewhisperer:us-east-1:123456789012:profile/PROFILE1".to_string()
        )]);
        drop(usage_queries);
        clear_kiro_upstream_envs();
    }
}
