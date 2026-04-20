//! Background token refresh and per-account usage polling.
//!
//! Runs a single tokio task that periodically:
//! 1. Refreshes `access_token` via `refresh_token` for accounts nearing expiry.
//! 2. Polls the upstream `/wham/usage` endpoint for each account to update the
//!    cached rate-limit snapshot used by the routing layer.

use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result};
use chrono::Utc;
use parking_lot::RwLock;
use rand::Rng;

use super::{
    accounts::{AccountPool, AccountRateLimitSnapshot, AccountStatus},
    runtime::CodexAuthSnapshot,
};
use crate::{
    state::LlmGatewayRuntimeConfig,
    upstream_proxy::{HttpClientProfile, UpstreamProxyRegistry},
};

const TOKEN_REFRESH_AHEAD_SECONDS: i64 = 600;
const REFRESH_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

fn next_codex_refresh_delay(config: &LlmGatewayRuntimeConfig) -> Duration {
    let min_seconds = config
        .codex_status_refresh_min_interval_seconds
        .min(config.codex_status_refresh_max_interval_seconds);
    let max_seconds = config
        .codex_status_refresh_min_interval_seconds
        .max(config.codex_status_refresh_max_interval_seconds);
    let seconds = if min_seconds == max_seconds {
        min_seconds
    } else {
        rand::thread_rng().gen_range(min_seconds..=max_seconds)
    };
    Duration::from_secs(seconds)
}

fn next_codex_account_jitter(config: &LlmGatewayRuntimeConfig) -> Duration {
    let max_seconds = config.codex_status_account_jitter_max_seconds;
    if max_seconds == 0 {
        Duration::ZERO
    } else {
        Duration::from_secs(rand::thread_rng().gen_range(0..=max_seconds))
    }
}

/// Spawn the background refresh task. Returns a `JoinHandle` that runs until
/// the shutdown signal fires.
pub(crate) fn spawn_account_refresh_task(
    pool: Arc<AccountPool>,
    proxy_registry: Arc<UpstreamProxyRegistry>,
    runtime_config: Arc<RwLock<LlmGatewayRuntimeConfig>>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) {
    tokio::spawn(async move {
        loop {
            let delay = {
                let config = runtime_config.read().clone();
                next_codex_refresh_delay(&config)
            };
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        tracing::info!("Account refresh task shutting down");
                        return;
                    }
                }
                _ = tokio::time::sleep(delay) => {
                    if let Err(err) =
                        refresh_all_accounts(&pool, &proxy_registry, runtime_config.as_ref()).await
                    {
                        tracing::warn!("Account refresh cycle failed: {err:#}");
                    }
                }
            }
        }
    });
}

pub(crate) async fn refresh_all_accounts_once(
    pool: &AccountPool,
    proxy_registry: &UpstreamProxyRegistry,
    runtime_config: &RwLock<LlmGatewayRuntimeConfig>,
) -> Result<()> {
    refresh_all_accounts(pool, proxy_registry, runtime_config).await
}

async fn refresh_all_accounts(
    pool: &AccountPool,
    proxy_registry: &UpstreamProxyRegistry,
    runtime_config: &RwLock<LlmGatewayRuntimeConfig>,
) -> Result<()> {
    let entries = pool.all_entries().await;

    for (index, (name, entry)) in entries.iter().enumerate() {
        if index > 0 {
            let jitter = {
                let config = runtime_config.read().clone();
                next_codex_account_jitter(&config)
            };
            if !jitter.is_zero() {
                tokio::time::sleep(jitter).await;
            }
        }
        if let Err(err) =
            refresh_account_entry(pool, proxy_registry, runtime_config, name, entry, false).await
        {
            tracing::warn!(account = name, "Account refresh cycle failed: {err:#}");
        }
    }
    Ok(())
}

pub(crate) async fn refresh_account_once(
    pool: &AccountPool,
    proxy_registry: &UpstreamProxyRegistry,
    runtime_config: &RwLock<LlmGatewayRuntimeConfig>,
    name: &str,
) -> Result<()> {
    let entry = pool
        .entry_by_name(name)
        .ok_or_else(|| anyhow::anyhow!("account `{name}` not found"))?;
    refresh_account_entry(pool, proxy_registry, runtime_config, name, &entry, true).await
}

async fn refresh_account_entry(
    pool: &AccountPool,
    proxy_registry: &UpstreamProxyRegistry,
    runtime_config: &RwLock<LlmGatewayRuntimeConfig>,
    name: &str,
    entry: &Arc<tokio::sync::RwLock<super::accounts::CodexAccount>>,
    manual_refresh: bool,
) -> Result<()> {
    match pool.sync_account_from_disk_if_changed(name, entry).await {
        Ok(true) => {
            tracing::info!(
                account = name,
                "Reloaded auth file changes into in-memory account snapshot"
            );
        },
        Ok(false) => {},
        Err(err) => {
            tracing::warn!(
                account = name,
                "Failed to sync account from auth file, keeping in-memory state: {err:#}"
            );
        },
    }

    let now = Utc::now().timestamp();
    let (status, needs_refresh) = {
        let account = entry.read().await;
        (account.status, token_needs_refresh(&account.access_token, now))
    };

    if needs_refresh || (manual_refresh && status != AccountStatus::Active) {
        match refresh_account_token(entry, proxy_registry).await {
            Ok(()) => {
                pool.clear_consecutive_refresh_failures(name).await;
                if let Err(err) = pool.persist(name).await {
                    tracing::warn!(account = name, "Failed to persist refreshed tokens: {err:#}");
                }
                entry.write().await.status = AccountStatus::Active;
                tracing::info!(account = name, "Refreshed access token");
            },
            Err(err) => {
                let failure_count = pool
                    .mark_usage_refresh_failure(name, format!("{err:#}"))
                    .await;
                let retry_limit = runtime_config.read().account_failure_retry_limit;
                let next_status = status_after_refresh_failure(failure_count, retry_limit);
                tracing::warn!(
                    account = name,
                    failure_count,
                    retry_limit,
                    next_status = next_status.as_str(),
                    "Token refresh failed: {err:#}"
                );
                entry.write().await.status = next_status;
                return Err(err).context("token refresh failed");
            },
        }
    }

    let snapshot = {
        let account = entry.read().await;
        account.to_auth_snapshot()
    };
    match fetch_account_usage(proxy_registry, &snapshot).await {
        Ok(rl) => {
            pool.update_rate_limit(name, rl).await;
            entry.write().await.status = AccountStatus::Active;
            Ok(())
        },
        Err(err) => {
            let failure_count = pool
                .mark_usage_refresh_failure(name, format!("{err:#}"))
                .await;
            let retry_limit = runtime_config.read().account_failure_retry_limit;
            let next_status = status_after_refresh_failure(failure_count, retry_limit);
            tracing::warn!(
                account = name,
                failure_count,
                retry_limit,
                next_status = next_status.as_str(),
                "Usage poll failed: {err:#}"
            );
            entry.write().await.status = next_status;
            Err(err).context("usage poll failed")
        },
    }
}

fn status_after_refresh_failure(consecutive_failures: u64, retry_limit: u64) -> AccountStatus {
    if retry_limit == 0 || consecutive_failures >= retry_limit {
        AccountStatus::Unavailable
    } else {
        AccountStatus::Active
    }
}

fn token_needs_refresh(access_token: &str, now_epoch: i64) -> bool {
    let exp = extract_jwt_exp(access_token).unwrap_or(0);
    if exp == 0 {
        return false; // cannot determine expiry, skip
    }
    exp - now_epoch < TOKEN_REFRESH_AHEAD_SECONDS
}

fn extract_jwt_exp(jwt: &str) -> Option<i64> {
    let payload_b64 = jwt.split('.').nth(1)?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload_b64)
        .ok()
        .or_else(|| {
            base64::engine::general_purpose::URL_SAFE
                .decode(payload_b64)
                .ok()
        })?;
    let value: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    value.get("exp").and_then(|v| v.as_i64())
}

use base64::Engine;

async fn refresh_account_token(
    entry: &Arc<tokio::sync::RwLock<super::accounts::CodexAccount>>,
    proxy_registry: &UpstreamProxyRegistry,
) -> Result<()> {
    let (refresh_token, auth_snapshot) = {
        let account = entry.read().await;
        if account.refresh_token.is_empty() {
            anyhow::bail!("no refresh_token available");
        }
        (account.refresh_token.clone(), account.to_auth_snapshot())
    };

    let client = build_refresh_client(proxy_registry, &auth_snapshot).await?;
    let body = format!(
        "client_id={}&grant_type=refresh_token&refresh_token={}",
        urlencoding::encode(CODEX_CLIENT_ID),
        urlencoding::encode(&refresh_token),
    );

    let resp = client
        .post(REFRESH_TOKEN_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .context("refresh token request failed")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("refresh token returned {status}: {text}");
    }

    #[derive(serde::Deserialize)]
    struct RefreshResponse {
        access_token: Option<String>,
        refresh_token: Option<String>,
        id_token: Option<String>,
    }

    let refreshed: RefreshResponse = resp.json().await.context("parse refresh response")?;
    {
        let mut account = entry.write().await;
        if let Some(at) = refreshed.access_token {
            account.access_token = at;
        }
        if let Some(rt) = refreshed.refresh_token {
            account.refresh_token = rt;
        }
        if let Some(id) = refreshed.id_token {
            account.id_token = id;
        }
        account.last_refresh = Some(Utc::now());
    }
    Ok(())
}

async fn fetch_account_usage(
    proxy_registry: &UpstreamProxyRegistry,
    auth: &CodexAuthSnapshot,
) -> Result<AccountRateLimitSnapshot> {
    let client = build_refresh_client(proxy_registry, auth).await?;
    let upstream_base = std::env::var("STATICFLOW_LLM_GATEWAY_UPSTREAM_BASE_URL")
        .ok()
        .map(|v| v.trim().trim_end_matches('/').to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "https://chatgpt.com/backend-api/codex".to_string());

    let source_url = compute_usage_url(&upstream_base);
    let mut request = client
        .get(&source_url)
        .header(reqwest::header::USER_AGENT, "codex_cli_rs/0.116.0")
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {}", auth.access_token))
        .header(reqwest::header::ACCEPT, "application/json")
        .timeout(Duration::from_secs(20));

    if let Some(account_id) = auth.account_id.as_deref() {
        request = request.header("ChatGPT-Account-Id", account_id);
    }

    let response = request.send().await.context("usage request failed")?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("usage request returned {status}");
    }

    // Parse the upstream payload into the same struct used by the legacy
    // single-file path, then convert to display-ready buckets so the public
    // status page has all fields (resets_at, plan_type, credits, etc.).
    let payload: super::UsageStatusPayload =
        serde_json::from_str(&body).context("parse usage response")?;
    let buckets = super::map_rate_limit_status_payload(payload);
    let now_ms = static_flow_shared::llm_gateway_store::now_ms();

    Ok(AccountRateLimitSnapshot {
        buckets,
        last_checked_at: Some(now_ms),
    })
}

fn compute_usage_url(upstream_base: &str) -> String {
    let lower = upstream_base.to_ascii_lowercase();
    if lower.contains("/backend-api/codex") {
        format!("{}/wham/usage", upstream_base.trim_end_matches("/codex"))
    } else if lower.contains("/backend-api") {
        format!("{upstream_base}/wham/usage")
    } else {
        format!("{upstream_base}/api/codex/usage")
    }
}

/// Validate that an account can reach the upstream usage endpoint.
/// Used during import to verify the account tokens work before persisting.
pub(crate) async fn validate_account_usage(
    proxy_registry: &UpstreamProxyRegistry,
    auth: &CodexAuthSnapshot,
) -> Result<AccountRateLimitSnapshot> {
    fetch_account_usage(proxy_registry, auth).await
}

/// Build the HTTP client used for token refresh and usage polling.
/// Shares the same proxy configuration as the gateway upstream client.
pub(crate) async fn build_refresh_client(
    proxy_registry: &UpstreamProxyRegistry,
    auth: &CodexAuthSnapshot,
) -> Result<reqwest::Client> {
    let (client, _resolved_proxy) = proxy_registry
        .client_for_selection(
            static_flow_shared::llm_gateway_store::LLM_GATEWAY_PROVIDER_CODEX,
            Some(&auth.proxy_selection),
            codex_refresh_client_profile(),
        )
        .await
        .context("failed to resolve codex refresh proxy")?;
    Ok(client)
}

pub(crate) const fn codex_refresh_client_profile() -> HttpClientProfile {
    HttpClientProfile::new(Some(60), 8, 60)
}

#[cfg(test)]
mod tests {
    use std::{
        path::PathBuf,
        sync::{Arc, OnceLock},
        time::{Duration as StdDuration, SystemTime},
    };

    use static_flow_shared::llm_gateway_store::LlmGatewayStore;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        sync::Mutex,
        time::timeout,
    };

    use super::*;
    use crate::{
        llm_gateway::accounts::AccountStatus,
        state::LlmGatewayRuntimeConfig,
        upstream_proxy::{AccountProxyMode, AccountProxySelection},
    };

    fn temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("system time before epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("staticflow-{prefix}-{nanos}"));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn sample_account(name: &str, status: AccountStatus) -> super::super::accounts::CodexAccount {
        super::super::accounts::CodexAccount {
            name: name.to_string(),
            access_token: "not-a-jwt-access-token".to_string(),
            account_id: Some(format!("{name}-acct")),
            refresh_token: String::new(),
            id_token: String::new(),
            map_gpt53_codex_to_spark: false,
            proxy_selection: AccountProxySelection {
                proxy_mode: AccountProxyMode::Direct,
                proxy_config_id: None,
            },
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
            last_refresh: None,
            status,
        }
    }

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    async fn spawn_usage_server() -> (String, tokio::task::JoinHandle<bool>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local usage server");
        let addr = listener.local_addr().expect("usage server local addr");
        let handle = tokio::spawn(async move {
            let Ok(Ok((mut stream, _peer))) =
                timeout(StdDuration::from_millis(500), listener.accept()).await
            else {
                return false;
            };
            let mut request = vec![0_u8; 4096];
            let read = stream.read(&mut request).await.expect("read request");
            let request = String::from_utf8_lossy(&request[..read]);
            assert!(
                request.starts_with("GET /api/codex/usage HTTP/1.1"),
                "unexpected request line: {request}"
            );
            let body = serde_json::json!({
                "plan_type": "Pro",
                "rate_limit": {
                    "primary_window": {
                        "used_percent": 11.0,
                        "limit_window_seconds": 18_000,
                        "reset_at": 1_777_777_777_i64
                    },
                    "secondary_window": {
                        "used_percent": 17.0,
                        "limit_window_seconds": 604_800,
                        "reset_at": 1_778_888_888_i64
                    }
                }
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: \
                 {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("write response");
            true
        });
        (format!("http://{addr}"), handle)
    }

    #[tokio::test]
    async fn refresh_cycle_recovers_unavailable_account_when_usage_poll_succeeds() {
        let _env_guard = env_lock().lock().await;
        let original_upstream_base = std::env::var("STATICFLOW_LLM_GATEWAY_UPSTREAM_BASE_URL").ok();
        let auths_dir = temp_dir("codex-auths");
        let store_dir = temp_dir("codex-store");
        let pool = AccountPool::new(auths_dir.clone());
        pool.insert(sample_account("alpha", AccountStatus::Unavailable))
            .await
            .expect("insert unavailable account");

        let store = Arc::new(
            LlmGatewayStore::connect(&store_dir.to_string_lossy())
                .await
                .expect("connect llm gateway store"),
        );
        let proxy_registry = Arc::new(
            UpstreamProxyRegistry::new(store)
                .await
                .expect("create upstream proxy registry"),
        );
        let runtime_config = parking_lot::RwLock::new(LlmGatewayRuntimeConfig::default());
        let (upstream_base, server) = spawn_usage_server().await;
        std::env::set_var("STATICFLOW_LLM_GATEWAY_UPSTREAM_BASE_URL", &upstream_base);

        let refresh_result =
            refresh_all_accounts_once(&pool, proxy_registry.as_ref(), &runtime_config).await;
        let server_requested = timeout(StdDuration::from_secs(2), server)
            .await
            .expect("usage server task should finish")
            .expect("usage server join");

        if let Some(value) = original_upstream_base.as_deref() {
            std::env::set_var("STATICFLOW_LLM_GATEWAY_UPSTREAM_BASE_URL", value);
        } else {
            std::env::remove_var("STATICFLOW_LLM_GATEWAY_UPSTREAM_BASE_URL");
        }

        refresh_result.expect("refresh cycle should succeed");
        assert!(
            server_requested,
            "automatic refresh should still poll usage for unavailable accounts"
        );

        let summary = pool
            .list_summaries()
            .await
            .into_iter()
            .find(|summary| summary.name == "alpha")
            .expect("alpha summary");
        assert_eq!(summary.status, AccountStatus::Active);
        assert_eq!(summary.rate_limits.primary_remaining_percent(), Some(89.0));
        assert_eq!(summary.rate_limits.secondary_remaining_percent(), Some(83.0));
        assert_eq!(summary.usage_refresh.error_message, None);

        let _ = std::fs::remove_dir_all(&auths_dir);
        let _ = std::fs::remove_dir_all(&store_dir);
    }

    #[test]
    fn failure_retry_limit_keeps_account_active_until_threshold_is_reached() {
        assert_eq!(status_after_refresh_failure(1, 3), AccountStatus::Active);
        assert_eq!(status_after_refresh_failure(2, 3), AccountStatus::Active);
        assert_eq!(status_after_refresh_failure(3, 3), AccountStatus::Unavailable);
    }

    #[test]
    fn failure_retry_limit_zero_marks_account_unavailable_immediately() {
        assert_eq!(status_after_refresh_failure(1, 0), AccountStatus::Unavailable);
    }

    #[test]
    fn codex_refresh_interval_draw_uses_configured_bounds() {
        let config = crate::state::LlmGatewayRuntimeConfig {
            codex_status_refresh_min_interval_seconds: 240,
            codex_status_refresh_max_interval_seconds: 300,
            ..crate::state::LlmGatewayRuntimeConfig::default()
        };

        for _ in 0..64 {
            let value = next_codex_refresh_delay(&config).as_secs();
            assert!((240..=300).contains(&value));
        }
    }

    #[test]
    fn codex_per_account_jitter_stays_within_configured_limit() {
        let config = crate::state::LlmGatewayRuntimeConfig {
            codex_status_account_jitter_max_seconds: 10,
            ..crate::state::LlmGatewayRuntimeConfig::default()
        };

        for _ in 0..64 {
            let value = next_codex_account_jitter(&config).as_secs();
            assert!(value <= 10);
        }
    }
}
