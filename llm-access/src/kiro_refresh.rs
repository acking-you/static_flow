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

const REFRESH_EARLY_MINUTES: i64 = 10;
const KIRO_USAGE_AWS_SDK_VERSION: &str = "1.0.0";
const KIRO_IDC_AWS_SDK_VERSION: &str = "3.980.0";
const KIRO_IDC_AMZ_SDK_REQUEST: &str = "attempt=1; max=4";

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
    let auth = parse_route_auth(route)?;
    if !force_refresh && !needs_refresh(&auth) {
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
        let access_token = non_empty_access_token(&latest)?;
        return Ok(KiroCallContext {
            auth: latest,
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
    let access_token = non_empty_access_token(&refreshed)?;
    store
        .save_kiro_auth_update(ProviderKiroAuthUpdate {
            account_name: route.account_name.clone(),
            auth_json: refreshed_auth_json(&route.auth_json, &refreshed)
                .context("serialize refreshed kiro auth")?,
            auth_method: refreshed.auth_method().to_string(),
            account_id: account_id_from_auth_json(&route.auth_json),
            profile_arn: refreshed.profile_arn.clone(),
            user_id: user_id_from_auth_json(&route.auth_json),
            status: KEY_STATUS_ACTIVE.to_string(),
            last_error: None,
            refreshed_at_ms: now_ms(),
        })
        .await?;
    Ok(KiroCallContext {
        auth: refreshed,
        access_token,
    })
}

pub(crate) async fn fetch_usage_limits_for_route(
    route: &ProviderKiroRoute,
    store: &dyn ProviderRouteStore,
    force_refresh: bool,
) -> anyhow::Result<UsageLimitsResponse> {
    let ctx = ensure_context_for_route(route, store, force_refresh).await?;
    let region = ctx.auth.effective_api_region().to_string();
    let host = format!("q.{region}.amazonaws.com");
    let upstream_base = std::env::var("KIRO_UPSTREAM_BASE_URL")
        .map(|value| value.trim_end_matches('/').to_string())
        .unwrap_or_else(|_| format!("https://{host}"));
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
    let machine_id = machine_id::generate_from_auth(&ctx.auth)
        .ok_or_else(|| anyhow!("failed to derive kiro machine id"))?;
    let (amz_user_agent, user_agent) = usage_request_user_agents(&machine_id);
    let response = client
        .get(url)
        .header("x-amz-user-agent", amz_user_agent)
        .header("user-agent", user_agent)
        .header("host", host)
        .header("amz-sdk-invocation-id", uuid::Uuid::new_v4().to_string())
        .header("amz-sdk-request", "attempt=1; max=1")
        .header("authorization", format!("Bearer {}", ctx.access_token))
        .header("connection", "close")
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

fn usage_request_user_agents(machine_id: &str) -> (String, String) {
    (
        format!(
            "aws-sdk-js/{KIRO_USAGE_AWS_SDK_VERSION} KiroIDE-{DEFAULT_KIRO_VERSION}-{machine_id}"
        ),
        format!(
            "aws-sdk-js/{KIRO_USAGE_AWS_SDK_VERSION} ua/2.1 os/{DEFAULT_SYSTEM_VERSION} lang/js \
             md/nodejs#{DEFAULT_NODE_VERSION} \
             api/codewhispererruntime#{KIRO_USAGE_AWS_SDK_VERSION} m/N,E \
             KiroIDE-{DEFAULT_KIRO_VERSION}-{machine_id}"
        ),
    )
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
mod tests {
    #[test]
    fn invalid_refresh_grant_detection_matches_kiro_refresh_contract() {
        let body =
            r#"{"error":"invalid_grant","error_description":"Invalid refresh token provided"}"#;

        assert!(super::is_invalid_refresh_token_grant(400, body));
        assert!(!super::is_invalid_refresh_token_grant(401, body));
        assert!(!super::is_invalid_refresh_token_grant(400, r#"{"error":"invalid_client"}"#));
    }
}
