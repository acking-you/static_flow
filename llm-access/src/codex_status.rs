//! Codex public rate-limit status refresh loop for standalone `llm-access`.

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use anyhow::Context;
use llm_access_core::store::{
    AdminCodexAccount, AdminCodexAccountStore, AdminConfigStore, AdminRuntimeConfig, CodexCredits,
    CodexPublicAccountStatus, CodexRateLimitBucket, CodexRateLimitStatus, CodexRateLimitWindow,
    ProviderCodexRoute, ProviderRouteStore, PublicStatusStore, KEY_STATUS_ACTIVE,
};
use rand::Rng;
use serde::Deserialize;

use crate::{codex_refresh, provider, runtime::LlmAccessRuntime};

#[derive(Debug, Clone, Deserialize)]
struct UsageStatusPayload {
    #[serde(default)]
    plan_type: Option<String>,
    #[serde(default)]
    rate_limit: Option<UsageRateLimitDetails>,
    #[serde(default)]
    additional_rate_limits: Option<Vec<UsageAdditionalRateLimit>>,
    #[serde(default)]
    credits: Option<UsageCreditsDetails>,
}

#[derive(Debug, Clone, Deserialize)]
struct UsageRateLimitDetails {
    #[serde(default)]
    primary_window: Option<UsageRateLimitWindow>,
    #[serde(default)]
    secondary_window: Option<UsageRateLimitWindow>,
}

#[derive(Debug, Clone, Deserialize)]
struct UsageAdditionalRateLimit {
    #[serde(default)]
    metered_feature: Option<String>,
    #[serde(default)]
    limit_name: Option<String>,
    #[serde(default)]
    rate_limit: Option<UsageRateLimitDetails>,
}

#[derive(Debug, Clone, Deserialize)]
struct UsageRateLimitWindow {
    used_percent: f64,
    #[serde(default)]
    limit_window_seconds: Option<i64>,
    #[serde(default)]
    reset_at: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
struct UsageCreditsDetails {
    #[serde(default)]
    has_credits: bool,
    #[serde(default)]
    unlimited: bool,
    #[serde(default)]
    balance: Option<UsageBalanceValue>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum UsageBalanceValue {
    String(String),
    Number(f64),
    Integer(i64),
}

/// Start Codex public status warmup and periodic refresh for `/llm-access`.
pub(crate) fn spawn_codex_status_refresher(runtime: &LlmAccessRuntime) {
    let config_store = runtime.admin_config_store();
    let account_store = runtime.admin_codex_account_store();
    let route_store = runtime.provider_route_store();
    let status_store = runtime.public_status_store();
    tokio::spawn({
        let config_store = Arc::clone(&config_store);
        let account_store = Arc::clone(&account_store);
        let route_store = Arc::clone(&route_store);
        let status_store = Arc::clone(&status_store);
        async move {
            if let Err(err) =
                refresh_codex_status(&config_store, &account_store, &route_store, &status_store)
                    .await
            {
                tracing::warn!("initial Codex public status refresh failed: {err:#}");
            }
        }
    });
    tokio::spawn(async move {
        loop {
            let delay = match config_store.get_admin_runtime_config().await {
                Ok(config) => next_codex_refresh_delay(&config),
                Err(err) => {
                    tracing::warn!("failed to load Codex status refresh config: {err:#}");
                    next_codex_refresh_delay(&AdminRuntimeConfig::default())
                },
            };
            tokio::time::sleep(delay).await;
            if let Err(err) =
                refresh_codex_status(&config_store, &account_store, &route_store, &status_store)
                    .await
            {
                tracing::warn!("failed to refresh cached Codex public status: {err:#}");
            }
        }
    });
}

async fn refresh_codex_status(
    config_store: &Arc<dyn AdminConfigStore>,
    account_store: &Arc<dyn AdminCodexAccountStore>,
    route_store: &Arc<dyn ProviderRouteStore>,
    status_store: &Arc<dyn PublicStatusStore>,
) -> anyhow::Result<()> {
    let config = config_store
        .get_admin_runtime_config()
        .await
        .context("load Codex status refresh config")?;
    let accounts = account_store
        .list_admin_codex_accounts()
        .await
        .context("list Codex accounts for status refresh")?;
    let source_url = compute_usage_url(&provider::codex_upstream_base_url());
    let mut refreshed = accounts
        .iter()
        .map(initial_account_status)
        .collect::<Vec<_>>();
    status_store
        .save_codex_rate_limit_status(build_status_snapshot(
            refreshed.clone(),
            &source_url,
            config.codex_status_refresh_max_interval_seconds,
        ))
        .await
        .context("persist initial Codex public status snapshot")?;
    for (index, account) in accounts.iter().enumerate() {
        if index > 0 {
            let jitter = next_codex_account_jitter(&config);
            if !jitter.is_zero() {
                tokio::time::sleep(jitter).await;
            }
        }
        refreshed[index] = refresh_account_status(
            account,
            account_store.as_ref(),
            route_store.as_ref(),
            &config,
            false,
        )
        .await;
        status_store
            .save_codex_rate_limit_status(build_status_snapshot(
                refreshed.clone(),
                &source_url,
                config.codex_status_refresh_max_interval_seconds,
            ))
            .await
            .context("persist incremental Codex public status snapshot")?;
    }
    Ok(())
}

pub(crate) async fn refresh_single_codex_account_status(
    config_store: &Arc<dyn AdminConfigStore>,
    account_store: &Arc<dyn AdminCodexAccountStore>,
    route_store: &Arc<dyn ProviderRouteStore>,
    status_store: &Arc<dyn PublicStatusStore>,
    account_name: &str,
) -> anyhow::Result<CodexPublicAccountStatus> {
    let config = config_store
        .get_admin_runtime_config()
        .await
        .context("load Codex status refresh config")?;
    let accounts = account_store
        .list_admin_codex_accounts()
        .await
        .context("list Codex accounts for status refresh")?;
    let account = accounts
        .iter()
        .find(|account| account.name == account_name)
        .ok_or_else(|| anyhow::anyhow!("Codex account `{account_name}` not found"))?;
    let source_url = compute_usage_url(&provider::codex_upstream_base_url());
    let refreshed = refresh_account_status(
        account,
        account_store.as_ref(),
        route_store.as_ref(),
        &config,
        true,
    )
    .await;
    let snapshot = merge_account_status_refresh(
        &accounts,
        status_store.codex_rate_limit_status().await.ok(),
        account_name,
        refreshed.clone(),
        &source_url,
        config.codex_status_refresh_max_interval_seconds,
    );
    status_store
        .save_codex_rate_limit_status(snapshot)
        .await
        .context("persist manual Codex public status refresh")?;
    match refreshed {
        AccountStatusRefresh::Ready {
            account, ..
        }
        | AccountStatusRefresh::Skipped {
            account,
        } => Ok(account),
        AccountStatusRefresh::Error {
            account,
        } => anyhow::bail!(
            "{}",
            account
                .usage_error_message
                .unwrap_or_else(|| "Codex usage refresh failed".to_string())
        ),
    }
}

fn initial_account_status(account: &AdminCodexAccount) -> AccountStatusRefresh {
    if account.status == KEY_STATUS_ACTIVE {
        AccountStatusRefresh::Error {
            account: account_error_status(
                account,
                now_ms(),
                "usage refresh pending for standalone llm-access",
            ),
        }
    } else {
        AccountStatusRefresh::Skipped {
            account: CodexPublicAccountStatus {
                name: account.name.clone(),
                status: account.status.clone(),
                plan_type: account.plan_type.clone(),
                primary_remaining_percent: account.primary_remaining_percent,
                secondary_remaining_percent: account.secondary_remaining_percent,
                last_usage_checked_at: None,
                last_usage_success_at: account.last_usage_success_at,
                usage_error_message: account.usage_error_message.clone(),
            },
        }
    }
}

async fn refresh_account_status(
    account: &AdminCodexAccount,
    account_store: &dyn AdminCodexAccountStore,
    route_store: &dyn ProviderRouteStore,
    config: &AdminRuntimeConfig,
    force_refresh: bool,
) -> AccountStatusRefresh {
    let now = now_ms();
    if account.status != KEY_STATUS_ACTIVE {
        return AccountStatusRefresh::Skipped {
            account: CodexPublicAccountStatus {
                name: account.name.clone(),
                status: account.status.clone(),
                plan_type: account.plan_type.clone(),
                primary_remaining_percent: account.primary_remaining_percent,
                secondary_remaining_percent: account.secondary_remaining_percent,
                last_usage_checked_at: Some(now),
                last_usage_success_at: account.last_usage_success_at,
                usage_error_message: account.usage_error_message.clone(),
            },
        };
    }
    let Some(route) = account_store
        .resolve_admin_codex_account_route(&account.name)
        .await
        .ok()
        .flatten()
    else {
        return AccountStatusRefresh::Error {
            account: account_error_status(account, now, "active Codex route is not configured"),
        };
    };
    match fetch_route_usage(&route, route_store, config, force_refresh).await {
        Ok(buckets) => AccountStatusRefresh::Ready {
            account: account_ready_status(account, now, &buckets),
            buckets,
        },
        Err(err) => AccountStatusRefresh::Error {
            account: account_error_status(account, now, &err.to_string()),
        },
    }
}

fn merge_account_status_refresh(
    accounts: &[AdminCodexAccount],
    existing: Option<CodexRateLimitStatus>,
    refreshed_name: &str,
    refreshed: AccountStatusRefresh,
    source_url: &str,
    refresh_interval_seconds: u64,
) -> CodexRateLimitStatus {
    let mut existing_accounts = existing
        .as_ref()
        .map(|status| {
            status
                .accounts
                .iter()
                .cloned()
                .map(|account| (account.name.clone(), account))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    let mut existing_buckets = existing
        .map(|status| {
            status
                .buckets
                .into_iter()
                .filter_map(|bucket| bucket.account_name.clone().map(|name| (name, bucket)))
                .fold(
                    BTreeMap::<String, Vec<CodexRateLimitBucket>>::new(),
                    |mut grouped, (name, bucket)| {
                        grouped.entry(name).or_default().push(bucket);
                        grouped
                    },
                )
        })
        .unwrap_or_default();
    let mut merged = Vec::with_capacity(accounts.len());
    for account in accounts {
        if account.name == refreshed_name {
            merged.push(refreshed.clone());
            continue;
        }
        let Some(public_account) = existing_accounts.remove(&account.name) else {
            merged.push(initial_account_status(account));
            continue;
        };
        let buckets = existing_buckets.remove(&account.name).unwrap_or_default();
        if !buckets.is_empty() {
            merged.push(AccountStatusRefresh::Ready {
                account: public_account,
                buckets,
            });
        } else if public_account.usage_error_message.is_some() {
            merged.push(AccountStatusRefresh::Error {
                account: public_account,
            });
        } else {
            merged.push(AccountStatusRefresh::Skipped {
                account: public_account,
            });
        }
    }
    build_status_snapshot(merged, source_url, refresh_interval_seconds)
}

async fn fetch_route_usage(
    route: &ProviderCodexRoute,
    route_store: &dyn ProviderRouteStore,
    config: &AdminRuntimeConfig,
    force_refresh: bool,
) -> anyhow::Result<Vec<CodexRateLimitBucket>> {
    let mut force_refresh_attempt = force_refresh;
    loop {
        let context =
            codex_refresh::ensure_context_for_route(route, route_store, force_refresh_attempt)
                .await?;
        let source_url = compute_usage_url(&provider::codex_upstream_base_url());
        let client_version =
            provider::resolve_codex_client_version(Some(&config.codex_client_version));
        let mut request = codex_refresh::provider_client(route.proxy.as_ref())?
            .get(&source_url)
            .header(reqwest::header::USER_AGENT, codex_user_agent(&client_version))
            .header(reqwest::header::AUTHORIZATION, format!("Bearer {}", context.access_token))
            .header(reqwest::header::ACCEPT, "application/json")
            .timeout(Duration::from_secs(20));
        if let Some(account_id) = context.account_id.as_deref() {
            request = request.header("ChatGPT-Account-Id", account_id);
        }
        if context.is_fedramp_account {
            request = request.header("X-OpenAI-Fedramp", "true");
        }

        let response = request.send().await.context("request Codex usage status")?;
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if status == reqwest::StatusCode::UNAUTHORIZED && !force_refresh_attempt {
            force_refresh_attempt = true;
            continue;
        }
        if !status.is_success() {
            anyhow::bail!("Codex usage status returned {status}: {body}");
        }
        let payload = serde_json::from_str::<UsageStatusPayload>(&body)
            .context("parse Codex usage status")?;
        let mut buckets = map_rate_limit_status_payload(payload);
        for bucket in &mut buckets {
            bucket.account_name = Some(route.account_name.clone());
        }
        return Ok(buckets);
    }
}

#[derive(Clone)]
enum AccountStatusRefresh {
    Ready { account: CodexPublicAccountStatus, buckets: Vec<CodexRateLimitBucket> },
    Error { account: CodexPublicAccountStatus },
    Skipped { account: CodexPublicAccountStatus },
}

fn build_status_snapshot(
    refreshed: Vec<AccountStatusRefresh>,
    source_url: &str,
    refresh_interval_seconds: u64,
) -> CodexRateLimitStatus {
    let checked_at = now_ms();
    let mut accounts = Vec::new();
    let mut buckets = Vec::new();
    let mut active_count = 0_usize;
    let mut success_count = 0_usize;
    let mut errors = Vec::new();
    for item in refreshed {
        match item {
            AccountStatusRefresh::Ready {
                account,
                buckets: account_buckets,
            } => {
                active_count += 1;
                success_count += 1;
                buckets.extend(account_buckets);
                accounts.push(account);
            },
            AccountStatusRefresh::Error {
                account,
            } => {
                if account.status == KEY_STATUS_ACTIVE {
                    active_count += 1;
                    if let Some(error) = account.usage_error_message.as_ref() {
                        errors.push(format!("{}: {}", account.name, error));
                    }
                }
                accounts.push(account);
            },
            AccountStatusRefresh::Skipped {
                account,
            } => accounts.push(account),
        }
    }

    let status = if active_count == 0 {
        "error"
    } else if !errors.is_empty() {
        "degraded"
    } else {
        "ready"
    };
    let error_message = if active_count == 0 {
        Some(format!(
            "no active codex accounts available out of {} configured account(s)",
            accounts.len()
        ))
    } else if errors.is_empty() {
        None
    } else {
        Some(format!(
            "usage refresh degraded for {} active account(s): {}",
            errors.len(),
            errors.join(" | ")
        ))
    };
    CodexRateLimitStatus {
        status: status.to_string(),
        refresh_interval_seconds,
        last_checked_at: Some(checked_at),
        last_success_at: if success_count > 0 { Some(checked_at) } else { None },
        source_url: source_url.to_string(),
        error_message,
        accounts,
        buckets,
    }
}

fn account_ready_status(
    account: &AdminCodexAccount,
    checked_at: i64,
    buckets: &[CodexRateLimitBucket],
) -> CodexPublicAccountStatus {
    let primary = buckets.iter().find(|bucket| bucket.is_primary);
    CodexPublicAccountStatus {
        name: account.name.clone(),
        status: account.status.clone(),
        plan_type: primary.and_then(|bucket| bucket.plan_type.clone()),
        primary_remaining_percent: primary
            .and_then(|bucket| bucket.primary.as_ref())
            .map(|window| window.remaining_percent),
        secondary_remaining_percent: primary
            .and_then(|bucket| bucket.secondary.as_ref())
            .map(|window| window.remaining_percent),
        last_usage_checked_at: Some(checked_at),
        last_usage_success_at: Some(checked_at),
        usage_error_message: None,
    }
}

fn account_error_status(
    account: &AdminCodexAccount,
    checked_at: i64,
    message: &str,
) -> CodexPublicAccountStatus {
    CodexPublicAccountStatus {
        name: account.name.clone(),
        status: account.status.clone(),
        plan_type: account.plan_type.clone(),
        primary_remaining_percent: account.primary_remaining_percent,
        secondary_remaining_percent: account.secondary_remaining_percent,
        last_usage_checked_at: Some(checked_at),
        last_usage_success_at: account.last_usage_success_at,
        usage_error_message: Some(message.to_string()),
    }
}

fn map_rate_limit_status_payload(payload: UsageStatusPayload) -> Vec<CodexRateLimitBucket> {
    let plan_type = payload.plan_type.as_deref().map(normalize_plan_type_label);
    let mut buckets = vec![CodexRateLimitBucket {
        limit_id: "codex".to_string(),
        limit_name: None,
        display_name: "codex".to_string(),
        is_primary: true,
        plan_type: plan_type.clone(),
        primary: payload
            .rate_limit
            .as_ref()
            .and_then(|details| details.primary_window.as_ref())
            .map(map_rate_limit_window),
        secondary: payload
            .rate_limit
            .as_ref()
            .and_then(|details| details.secondary_window.as_ref())
            .map(map_rate_limit_window),
        credits: payload.credits.as_ref().map(map_credits_view),
        account_name: None,
    }];
    buckets.extend(
        payload
            .additional_rate_limits
            .unwrap_or_default()
            .into_iter()
            .map(|details| {
                let limit_id = details
                    .metered_feature
                    .as_deref()
                    .map(normalize_limit_id)
                    .unwrap_or_else(|| "codex_other".to_string());
                let display_name = details
                    .limit_name
                    .clone()
                    .or_else(|| details.metered_feature.clone())
                    .unwrap_or_else(|| limit_id.clone());
                CodexRateLimitBucket {
                    limit_id,
                    limit_name: details.limit_name.clone(),
                    display_name,
                    is_primary: false,
                    plan_type: plan_type.clone(),
                    primary: details
                        .rate_limit
                        .as_ref()
                        .and_then(|rate_limit| rate_limit.primary_window.as_ref())
                        .map(map_rate_limit_window),
                    secondary: details
                        .rate_limit
                        .as_ref()
                        .and_then(|rate_limit| rate_limit.secondary_window.as_ref())
                        .map(map_rate_limit_window),
                    credits: None,
                    account_name: None,
                }
            }),
    );
    buckets
}

fn map_rate_limit_window(window: &UsageRateLimitWindow) -> CodexRateLimitWindow {
    let used_percent = window.used_percent.clamp(0.0, 100.0);
    CodexRateLimitWindow {
        used_percent,
        remaining_percent: (100.0 - used_percent).clamp(0.0, 100.0),
        window_duration_mins: window.limit_window_seconds.map(seconds_to_window_minutes),
        resets_at: window.reset_at,
    }
}

fn map_credits_view(credits: &UsageCreditsDetails) -> CodexCredits {
    CodexCredits {
        has_credits: credits.has_credits,
        unlimited: credits.unlimited,
        balance: credits.balance.as_ref().map(balance_value_to_string),
    }
}

fn balance_value_to_string(value: &UsageBalanceValue) -> String {
    match value {
        UsageBalanceValue::String(value) => value.trim().to_string(),
        UsageBalanceValue::Number(value) => format!("{value:.2}"),
        UsageBalanceValue::Integer(value) => value.to_string(),
    }
}

fn compute_usage_url(upstream_base: &str) -> String {
    let normalized = upstream_base.trim_end_matches('/');
    let lower = normalized.to_ascii_lowercase();
    if lower.contains("/backend-api/codex") {
        format!("{}/wham/usage", normalized.trim_end_matches("/codex"))
    } else if lower.contains("/backend-api") {
        format!("{normalized}/wham/usage")
    } else {
        format!("{normalized}/api/codex/usage")
    }
}

fn seconds_to_window_minutes(seconds: i64) -> i64 {
    (seconds.max(0) + 59) / 60
}

fn normalize_plan_type_label(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return "unknown".to_string();
    }
    let mut chars = trimmed.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
        None => "unknown".to_string(),
    }
}

fn normalize_limit_id(raw: &str) -> String {
    raw.trim().to_ascii_lowercase().replace('-', "_")
}

fn codex_user_agent(client_version: &str) -> String {
    format!("codex_cli_rs/{client_version}")
}

fn next_codex_refresh_delay(config: &AdminRuntimeConfig) -> Duration {
    let min = config.codex_status_refresh_min_interval_seconds;
    let max = config.codex_status_refresh_max_interval_seconds.max(min);
    let seconds = rand::thread_rng().gen_range(min..=max);
    Duration::from_secs(seconds)
}

fn next_codex_account_jitter(config: &AdminRuntimeConfig) -> Duration {
    let max = config.codex_status_account_jitter_max_seconds;
    if max == 0 {
        Duration::ZERO
    } else {
        Duration::from_secs(rand::thread_rng().gen_range(0..=max))
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_admin_account(name: &str) -> AdminCodexAccount {
        AdminCodexAccount {
            name: name.to_string(),
            status: KEY_STATUS_ACTIVE.to_string(),
            account_id: Some(format!("acct-{name}")),
            plan_type: None,
            primary_remaining_percent: None,
            secondary_remaining_percent: None,
            map_gpt53_codex_to_spark: false,
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
            proxy_mode: "inherit".to_string(),
            proxy_config_id: None,
            effective_proxy_source: "binding".to_string(),
            effective_proxy_url: None,
            effective_proxy_config_name: None,
            last_refresh: Some(100),
            last_usage_checked_at: None,
            last_usage_success_at: None,
            usage_error_message: None,
        }
    }

    fn sample_bucket(account_name: &str, primary: f64, secondary: f64) -> CodexRateLimitBucket {
        CodexRateLimitBucket {
            limit_id: "codex".to_string(),
            limit_name: None,
            display_name: "codex".to_string(),
            is_primary: true,
            plan_type: Some("Pro".to_string()),
            primary: Some(CodexRateLimitWindow {
                used_percent: 100.0 - primary,
                remaining_percent: primary,
                window_duration_mins: Some(300),
                resets_at: Some(2000),
            }),
            secondary: Some(CodexRateLimitWindow {
                used_percent: 100.0 - secondary,
                remaining_percent: secondary,
                window_duration_mins: Some(10080),
                resets_at: Some(3000),
            }),
            credits: None,
            account_name: Some(account_name.to_string()),
        }
    }

    #[test]
    fn maps_codex_usage_payload_into_public_buckets() {
        let payload: UsageStatusPayload = serde_json::from_value(serde_json::json!({
            "plan_type": "pro",
            "rate_limit": {
                "primary_window": {
                    "used_percent": 25.0,
                    "limit_window_seconds": 18000,
                    "reset_at": 123
                }
            },
            "credits": {
                "has_credits": true,
                "unlimited": false,
                "balance": 12.5
            },
            "additional_rate_limits": [{
                "metered_feature": "model-gpt-5.1",
                "limit_name": "GPT 5.1",
                "rate_limit": {
                    "primary_window": {
                        "used_percent": 50.0
                    }
                }
            }]
        }))
        .expect("usage payload");

        let buckets = map_rate_limit_status_payload(payload);

        assert_eq!(buckets.len(), 2);
        assert_eq!(buckets[0].plan_type.as_deref(), Some("Pro"));
        assert_eq!(
            buckets[0]
                .primary
                .as_ref()
                .map(|window| window.remaining_percent),
            Some(75.0)
        );
        assert_eq!(
            buckets[0]
                .credits
                .as_ref()
                .and_then(|credits| credits.balance.as_deref()),
            Some("12.50")
        );
        assert_eq!(buckets[1].limit_id, "model_gpt_5.1");
    }

    #[test]
    fn manual_account_refresh_merges_one_account_into_cached_snapshot() {
        let accounts = vec![sample_admin_account("alpha"), sample_admin_account("beta")];
        let alpha_bucket = sample_bucket("alpha", 70.0, 80.0);
        let beta_bucket = sample_bucket("beta", 62.0, 39.0);
        let existing = CodexRateLimitStatus {
            status: "ready".to_string(),
            refresh_interval_seconds: 300,
            last_checked_at: Some(900),
            last_success_at: Some(900),
            source_url: "https://chatgpt.com/backend-api/wham/usage".to_string(),
            error_message: None,
            accounts: vec![CodexPublicAccountStatus {
                name: "alpha".to_string(),
                status: KEY_STATUS_ACTIVE.to_string(),
                plan_type: Some("Pro".to_string()),
                primary_remaining_percent: Some(70.0),
                secondary_remaining_percent: Some(80.0),
                last_usage_checked_at: Some(900),
                last_usage_success_at: Some(900),
                usage_error_message: None,
            }],
            buckets: vec![alpha_bucket],
        };
        let refreshed = AccountStatusRefresh::Ready {
            account: account_ready_status(&accounts[1], 1200, std::slice::from_ref(&beta_bucket)),
            buckets: vec![beta_bucket],
        };

        let snapshot = merge_account_status_refresh(
            &accounts,
            Some(existing),
            "beta",
            refreshed,
            "https://chatgpt.com/backend-api/wham/usage",
            300,
        );

        assert_eq!(snapshot.accounts.len(), 2);
        assert_eq!(snapshot.accounts[0].name, "alpha");
        assert_eq!(snapshot.accounts[0].secondary_remaining_percent, Some(80.0));
        assert_eq!(snapshot.accounts[1].name, "beta");
        assert_eq!(snapshot.accounts[1].primary_remaining_percent, Some(62.0));
        assert_eq!(snapshot.accounts[1].secondary_remaining_percent, Some(39.0));
        assert_eq!(snapshot.buckets.len(), 2);
        assert!(snapshot
            .buckets
            .iter()
            .any(|bucket| bucket.account_name.as_deref() == Some("alpha")));
        assert!(snapshot
            .buckets
            .iter()
            .any(|bucket| bucket.account_name.as_deref() == Some("beta")));
    }
}
