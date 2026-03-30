//! Background token refresh and per-account usage polling.
//!
//! Runs a single tokio task that periodically:
//! 1. Refreshes `access_token` via `refresh_token` for accounts nearing expiry.
//! 2. Polls the upstream `/wham/usage` endpoint for each account to update the
//!    cached rate-limit snapshot used by the routing layer.

use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result};
use chrono::Utc;

use super::{
    accounts::{AccountPool, AccountRateLimitSnapshot, AccountStatus},
    runtime::CodexAuthSnapshot,
};
use crate::upstream_proxy::{standard_client_builder, UpstreamProxyRegistry};

const REFRESH_INTERVAL_SECONDS: u64 = 60;
const TOKEN_REFRESH_AHEAD_SECONDS: i64 = 600;
const REFRESH_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

/// Spawn the background refresh task. Returns a `JoinHandle` that runs until
/// the shutdown signal fires.
pub(crate) fn spawn_account_refresh_task(
    pool: Arc<AccountPool>,
    proxy_registry: Arc<UpstreamProxyRegistry>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(REFRESH_INTERVAL_SECONDS));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        ticker.tick().await; // first tick fires immediately

        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        tracing::info!("Account refresh task shutting down");
                        return;
                    }
                }
                _ = ticker.tick() => {
                    if let Err(err) = refresh_all_accounts(&pool, &proxy_registry).await {
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
) -> Result<()> {
    refresh_all_accounts(pool, proxy_registry).await
}

async fn refresh_all_accounts(
    pool: &AccountPool,
    proxy_registry: &UpstreamProxyRegistry,
) -> Result<()> {
    let entries = pool.all_entries().await;
    let now = Utc::now().timestamp();

    for (name, entry) in &entries {
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

        let needs_refresh = {
            let account = entry.read().await;
            if account.status != AccountStatus::Active {
                continue;
            }
            token_needs_refresh(&account.access_token, now)
        };

        if needs_refresh {
            match refresh_account_token(entry, proxy_registry).await {
                Ok(()) => {
                    if let Err(err) = pool.persist(name).await {
                        tracing::warn!(
                            account = name,
                            "Failed to persist refreshed tokens: {err:#}"
                        );
                    }
                    tracing::info!(account = name, "Refreshed access token");
                },
                Err(err) => {
                    tracing::warn!(
                        account = name,
                        "Token refresh failed, marking unavailable: {err:#}"
                    );
                    entry.write().await.status = AccountStatus::Unavailable;
                },
            }
        }

        // Always poll usage for active accounts.
        let snapshot = {
            let account = entry.read().await;
            if account.status != AccountStatus::Active {
                continue;
            }
            account.to_auth_snapshot()
        };
        match fetch_account_usage(proxy_registry, &snapshot).await {
            Ok(rl) => pool.update_rate_limit(name, rl).await,
            Err(err) => {
                pool.mark_usage_refresh_failure(name, format!("{err:#}"))
                    .await;
                tracing::warn!(account = name, "Usage poll failed: {err:#}");
            },
        }
    }
    Ok(())
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
    let refresh_token = {
        let account = entry.read().await;
        if account.refresh_token.is_empty() {
            anyhow::bail!("no refresh_token available");
        }
        account.refresh_token.clone()
    };

    let client = build_refresh_client(proxy_registry).await?;
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
    let client = build_refresh_client(proxy_registry).await?;
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
) -> Result<reqwest::Client> {
    let builder = standard_client_builder(60, 8, 60);
    let builder = proxy_registry
        .apply_provider_proxy(
            static_flow_shared::llm_gateway_store::LLM_GATEWAY_PROVIDER_CODEX,
            builder,
        )
        .await
        .context("failed to resolve codex refresh proxy")?;
    builder.build().context("failed to build refresh client")
}
