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
    let initial = parse_auth_parts(&route.auth_json)?;
    if !force_refresh && !access_token_is_expired(&initial.access_token) {
        return Ok(initial.into_context());
    }

    let refresh_lock = refresh_lock_for_account(&route.account_name)?;
    let _guard = refresh_lock.lock().await;
    let latest_route = store
        .resolve_codex_account_route(&route.account_name)
        .await?
        .ok_or_else(|| {
            anyhow!("active Codex account `{}` is not configured", route.account_name)
        })?;
    let latest = parse_auth_parts(&latest_route.auth_json)?;
    let latest_access_token_is_expired = access_token_is_expired(&latest.access_token);
    if !force_refresh && !latest_access_token_is_expired {
        return Ok(latest.into_context());
    }
    if force_refresh && auth_parts_changed(&initial, &latest) && !latest_access_token_is_expired {
        return Ok(latest.into_context());
    }

    let refreshed = refresh_auth(&latest_route, &latest).await?;
    let auth_json = refreshed_auth_json(&latest_route.auth_json, &latest, &refreshed)?;
    let next_parts = parse_auth_parts(&auth_json)?;
    store
        .save_codex_auth_update(ProviderCodexAuthUpdate {
            account_name: latest_route.account_name.clone(),
            auth_json,
            account_id: next_parts.account_id.clone(),
            status: KEY_STATUS_ACTIVE.to_string(),
            last_error: None,
            refreshed_at_ms: now_ms(),
        })
        .await?;
    Ok(next_parts.into_context())
}

fn auth_parts_changed(previous: &CodexAuthParts, latest: &CodexAuthParts) -> bool {
    previous.access_token != latest.access_token
        || previous.refresh_token != latest.refresh_token
        || previous.id_token != latest.id_token
        || previous.account_id != latest.account_id
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

pub(crate) fn provider_client(
    proxy: Option<&ProviderProxyConfig>,
) -> anyhow::Result<reqwest::Client> {
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

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use chrono::Duration;
    use llm_access_core::{
        provider::RouteStrategy,
        store::{AuthenticatedKey, ProviderKiroAuthUpdate, ProviderKiroRoute},
    };
    use serde_json::json;

    use super::*;

    #[derive(Clone)]
    struct TestRouteStore {
        latest_codex_route: Arc<Mutex<Option<ProviderCodexRoute>>>,
        codex_updates: Arc<Mutex<Vec<ProviderCodexAuthUpdate>>>,
    }

    #[async_trait]
    impl ProviderRouteStore for TestRouteStore {
        async fn resolve_codex_route(
            &self,
            _key: &AuthenticatedKey,
        ) -> anyhow::Result<Option<ProviderCodexRoute>> {
            Ok(self.latest_codex_route.lock().expect("route").clone())
        }

        async fn resolve_codex_account_route(
            &self,
            account_name: &str,
        ) -> anyhow::Result<Option<ProviderCodexRoute>> {
            let route = self.latest_codex_route.lock().expect("route").clone();
            Ok(route.filter(|route| route.account_name == account_name))
        }

        async fn resolve_kiro_route(
            &self,
            _key: &AuthenticatedKey,
        ) -> anyhow::Result<Option<ProviderKiroRoute>> {
            Ok(None)
        }

        async fn save_kiro_auth_update(
            &self,
            _update: ProviderKiroAuthUpdate,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        async fn save_codex_auth_update(
            &self,
            update: ProviderCodexAuthUpdate,
        ) -> anyhow::Result<()> {
            self.codex_updates.lock().expect("updates").push(update);
            Ok(())
        }
    }

    #[tokio::test]
    async fn refresh_uses_latest_stored_auth_after_guarded_reload() {
        let stale_route = codex_route_with_auth(auth_json(expired_token(), None));
        let latest_access_token = future_token();
        let latest_route = codex_route_with_auth(auth_json(
            latest_access_token.clone(),
            Some("latest-refresh-token"),
        ));
        let store = TestRouteStore {
            latest_codex_route: Arc::new(Mutex::new(Some(latest_route))),
            codex_updates: Arc::new(Mutex::new(Vec::new())),
        };

        let context = ensure_context_for_route(&stale_route, &store, false)
            .await
            .expect("refresh context should use latest stored auth");

        assert_eq!(context.access_token, latest_access_token);
        assert!(
            store.codex_updates.lock().expect("updates").is_empty(),
            "latest usable auth should not be rewritten"
        );
    }

    #[tokio::test]
    async fn forced_refresh_uses_latest_auth_when_stored_token_changed() {
        let stale_route = codex_route_with_auth(auth_json(future_token(), Some("stale-refresh")));
        let latest_access_token = future_token();
        let latest_route = codex_route_with_auth(auth_json(
            latest_access_token.clone(),
            Some("latest-refresh-token"),
        ));
        let store = TestRouteStore {
            latest_codex_route: Arc::new(Mutex::new(Some(latest_route))),
            codex_updates: Arc::new(Mutex::new(Vec::new())),
        };

        let context = ensure_context_for_route(&stale_route, &store, true)
            .await
            .expect("forced refresh should use changed stored auth");

        assert_eq!(context.access_token, latest_access_token);
        assert!(
            store.codex_updates.lock().expect("updates").is_empty(),
            "changed latest auth should avoid a second refresh request"
        );
    }

    fn codex_route_with_auth(auth_json: String) -> ProviderCodexRoute {
        ProviderCodexRoute {
            account_name: "codex-account".to_string(),
            account_group_id_at_event: None,
            route_strategy_at_event: RouteStrategy::Auto,
            auth_json,
            map_gpt53_codex_to_spark: false,
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
            account_request_max_concurrency: None,
            account_request_min_start_interval_ms: None,
            proxy: None,
        }
    }

    fn auth_json(access_token: String, refresh_token: Option<&str>) -> String {
        let mut value = json!({
            "tokens": {
                "access_token": access_token,
                "account_id": "account-id"
            }
        });
        if let Some(refresh_token) = refresh_token {
            value["tokens"]["refresh_token"] = json!(refresh_token);
        }
        value.to_string()
    }

    fn expired_token() -> String {
        jwt_with_exp((Utc::now() - Duration::minutes(5)).timestamp())
    }

    fn future_token() -> String {
        jwt_with_exp((Utc::now() + Duration::hours(1)).timestamp())
    }

    fn jwt_with_exp(exp: i64) -> String {
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"none"}"#);
        let payload = URL_SAFE_NO_PAD.encode(format!(r#"{{"exp":{exp}}}"#));
        format!("{header}.{payload}.signature")
    }
}
