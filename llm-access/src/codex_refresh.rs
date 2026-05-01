//! Codex OAuth refresh helpers for the standalone provider runtime.

use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
};

use anyhow::{anyhow, bail, Context};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, Utc};
use llm_access_core::store::{
    ProviderCodexAuthUpdate, ProviderCodexRoute, ProviderProxyConfig, ProviderRouteStore,
    KEY_STATUS_ACTIVE,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const REFRESH_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

static REFRESH_LOCKS: LazyLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Debug, Clone)]
pub(crate) struct CodexCallContext {
    pub access_token: String,
    pub account_id: Option<String>,
    pub is_fedramp_account: bool,
}

#[derive(Debug, Clone)]
struct CodexAuthParts {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    account_id: Option<String>,
}

#[derive(Serialize)]
struct RefreshRequest<'a> {
    client_id: &'static str,
    grant_type: &'static str,
    refresh_token: &'a str,
}

#[derive(Deserialize)]
struct RefreshResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    id_token: Option<String>,
}

pub(crate) async fn ensure_context_for_route(
    route: &ProviderCodexRoute,
    store: &dyn ProviderRouteStore,
    force_refresh: bool,
) -> anyhow::Result<CodexCallContext> {
    let parts = parse_auth_parts(&route.auth_json)?;
    if !force_refresh && !access_token_is_expired(&parts.access_token) {
        return Ok(parts.into_context());
    }

    let refresh_lock = refresh_lock_for_account(&route.account_name)?;
    let _guard = refresh_lock.lock().await;
    let latest = parse_auth_parts(&route.auth_json)?;
    if !force_refresh && !access_token_is_expired(&latest.access_token) {
        return Ok(latest.into_context());
    }

    let refreshed = refresh_auth(route, &latest).await?;
    let auth_json = refreshed_auth_json(&route.auth_json, &latest, &refreshed)?;
    let next_parts = parse_auth_parts(&auth_json)?;
    store
        .save_codex_auth_update(ProviderCodexAuthUpdate {
            account_name: route.account_name.clone(),
            auth_json,
            account_id: next_parts.account_id.clone(),
            status: KEY_STATUS_ACTIVE.to_string(),
            last_error: None,
            refreshed_at_ms: now_ms(),
        })
        .await?;
    Ok(next_parts.into_context())
}

impl CodexAuthParts {
    fn into_context(self) -> CodexCallContext {
        CodexCallContext {
            access_token: self.access_token,
            account_id: self.account_id,
            is_fedramp_account: self
                .id_token
                .as_deref()
                .is_some_and(id_token_is_fedramp_account),
        }
    }
}

fn parse_auth_parts(auth_json: &str) -> anyhow::Result<CodexAuthParts> {
    let value: Value = serde_json::from_str(auth_json).context("parse codex auth json")?;
    let access_token = optional_string(&value, &["access_token", "accessToken"])
        .or_else(|| {
            value
                .get("tokens")
                .and_then(|tokens| optional_string(tokens, &["access_token", "accessToken"]))
        })
        .ok_or_else(|| anyhow!("codex auth missing access token"))?;
    let refresh_token = optional_string(&value, &["refresh_token", "refreshToken"]).or_else(|| {
        value
            .get("tokens")
            .and_then(|tokens| optional_string(tokens, &["refresh_token", "refreshToken"]))
    });
    let id_token = optional_string(&value, &["id_token", "idToken"]).or_else(|| {
        value
            .get("tokens")
            .and_then(|tokens| optional_string(tokens, &["id_token", "idToken"]))
    });
    let account_id = optional_string(&value, &["account_id", "accountId"]).or_else(|| {
        value
            .get("tokens")
            .and_then(|tokens| optional_string(tokens, &["account_id", "accountId"]))
    });
    Ok(CodexAuthParts {
        access_token,
        refresh_token,
        id_token,
        account_id,
    })
}

async fn refresh_auth(
    route: &ProviderCodexRoute,
    current: &CodexAuthParts,
) -> anyhow::Result<RefreshResponse> {
    let refresh_token = current
        .refresh_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("no codex refresh_token available"))?;
    let response = provider_client(route.proxy.as_ref())?
        .post(REFRESH_TOKEN_URL)
        .header("Content-Type", "application/json")
        .json(&RefreshRequest {
            client_id: CODEX_CLIENT_ID,
            grant_type: "refresh_token",
            refresh_token,
        })
        .send()
        .await
        .context("refresh codex token")?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        bail!("codex refresh token returned {status}: {body}");
    }
    response
        .json()
        .await
        .context("parse codex refresh response")
}

fn refreshed_auth_json(
    original_json: &str,
    current: &CodexAuthParts,
    refreshed: &RefreshResponse,
) -> anyhow::Result<String> {
    let mut value = serde_json::from_str::<Value>(original_json)
        .unwrap_or_else(|_| Value::Object(serde_json::Map::new()));
    let object = value
        .as_object_mut()
        .ok_or_else(|| anyhow!("codex auth json must be an object"))?;

    let access_token = refreshed
        .access_token
        .as_deref()
        .unwrap_or(current.access_token.as_str());
    let refresh_token = refreshed
        .refresh_token
        .as_deref()
        .or(current.refresh_token.as_deref());
    let id_token = refreshed
        .id_token
        .as_deref()
        .or(current.id_token.as_deref());

    if object.get("tokens").is_some() {
        let tokens = object
            .entry("tokens".to_string())
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
        let tokens = tokens
            .as_object_mut()
            .ok_or_else(|| anyhow!("codex auth tokens must be an object"))?;
        tokens.insert("access_token".to_string(), Value::String(access_token.to_string()));
        if let Some(refresh_token) = refresh_token {
            tokens.insert("refresh_token".to_string(), Value::String(refresh_token.to_string()));
        }
        if let Some(id_token) = id_token {
            tokens.insert("id_token".to_string(), Value::String(id_token.to_string()));
        }
    } else {
        object.insert("access_token".to_string(), Value::String(access_token.to_string()));
        if let Some(refresh_token) = refresh_token {
            object.insert("refresh_token".to_string(), Value::String(refresh_token.to_string()));
        }
        if let Some(id_token) = id_token {
            object.insert("id_token".to_string(), Value::String(id_token.to_string()));
        }
    }
    serde_json::to_string(&value).context("serialize refreshed codex auth")
}

fn access_token_is_expired(token: &str) -> bool {
    let Some(expires_at) = access_token_expiry(token) else {
        return false;
    };
    expires_at <= Utc::now()
}

fn access_token_expiry(token: &str) -> Option<DateTime<Utc>> {
    let payload = token.split('.').nth(1)?;
    let decoded = URL_SAFE_NO_PAD.decode(payload.as_bytes()).ok()?;
    let value: Value = serde_json::from_slice(&decoded).ok()?;
    let exp = value.get("exp")?.as_i64()?;
    DateTime::from_timestamp(exp, 0)
}

fn id_token_is_fedramp_account(id_token: &str) -> bool {
    let Some(payload_b64) = id_token.split('.').nth(1) else {
        return false;
    };
    let Ok(bytes) = URL_SAFE_NO_PAD.decode(payload_b64) else {
        return false;
    };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return false;
    };
    value
        .get("https://api.openai.com/auth")
        .or_else(|| value.get("https://chatgpt.com"))
        .and_then(|auth| auth.get("chatgpt_account_is_fedramp"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn optional_string(value: &Value, fields: &[&str]) -> Option<String> {
    fields
        .iter()
        .find_map(|field| value.get(*field).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
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

fn refresh_lock_for_account(account_name: &str) -> anyhow::Result<Arc<tokio::sync::Mutex<()>>> {
    let mut locks = REFRESH_LOCKS
        .lock()
        .map_err(|_| anyhow!("codex refresh lock registry poisoned"))?;
    Ok(locks
        .entry(account_name.to_string())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}
