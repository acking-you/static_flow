//! Codex public rate-limit status refresh loop for standalone `llm-access`.

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use anyhow::Context;
use futures_util::{stream, StreamExt};
use llm_access_core::store::{
    is_terminal_codex_auth_error, AdminCodexAccount, AdminCodexAccountStore, AdminConfigStore,
    AdminRuntimeConfig, CodexCredits, CodexPublicAccountStatus, CodexRateLimitBucket,
    CodexRateLimitStatus, CodexRateLimitWindow, CodexStatusRefreshTarget, ProviderCodexRoute,
    ProviderRouteStore, PublicStatusStore, KEY_STATUS_ACTIVE,
};
use rand::Rng;
use serde::Deserialize;

use crate::{codex_refresh, provider, runtime::LlmAccessRuntime};

const CODEX_STATUS_REFRESH_CONCURRENCY: usize = 4;

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

trait CodexStatusAccount {
    fn name(&self) -> &str;
    fn status(&self) -> &str;
    fn plan_type(&self) -> Option<&str> {
        None
    }
    fn primary_remaining_percent(&self) -> Option<f64> {
        None
    }
    fn secondary_remaining_percent(&self) -> Option<f64> {
        None
    }
    fn last_usage_success_at(&self) -> Option<i64> {
        None
    }
    fn usage_error_message(&self) -> Option<&str> {
        None
    }
}

impl CodexStatusAccount for AdminCodexAccount {
    fn name(&self) -> &str {
        &self.name
    }

    fn status(&self) -> &str {
        &self.status
    }

    fn plan_type(&self) -> Option<&str> {
        self.plan_type.as_deref()
    }

    fn primary_remaining_percent(&self) -> Option<f64> {
        self.primary_remaining_percent
    }

    fn secondary_remaining_percent(&self) -> Option<f64> {
        self.secondary_remaining_percent
    }

    fn last_usage_success_at(&self) -> Option<i64> {
        self.last_usage_success_at
    }

    fn usage_error_message(&self) -> Option<&str> {
        self.usage_error_message.as_deref()
    }
}

impl CodexStatusAccount for CodexStatusRefreshTarget {
    fn name(&self) -> &str {
        &self.name
    }

    fn status(&self) -> &str {
        &self.status
    }
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
    let source_url = compute_usage_url(&provider::codex_upstream_base_url());
    let existing = status_store.codex_rate_limit_status().await.ok();
    let accounts = prioritize_codex_refresh_targets(
        account_store
            .list_codex_status_refresh_targets()
            .await
            .context("list Codex status refresh targets")?,
        existing.as_ref(),
    );
    let mut refreshed = seed_full_refresh_statuses(&accounts, existing);
    status_store
        .save_codex_rate_limit_status(build_background_refresh_snapshot(
            &accounts,
            &refreshed,
            None,
            None,
            &source_url,
            config.codex_status_refresh_max_interval_seconds,
        ))
        .await
        .context("persist initial Codex public status snapshot")?;
    let refreshes = stream::iter(
        accounts
            .iter()
            .cloned()
            .enumerate()
            .map(|(index, account)| {
                let account_store = Arc::clone(account_store);
                let route_store = Arc::clone(route_store);
                let config = config.clone();
                async move {
                    if index > 0 {
                        let jitter = next_codex_account_jitter(&config);
                        if !jitter.is_zero() {
                            tokio::time::sleep(jitter).await;
                        }
                    }
                    let next = refresh_account_status(
                        &account,
                        account_store.as_ref(),
                        route_store.as_ref(),
                        &config,
                        false,
                    )
                    .await;
                    (index, account.name.clone(), next)
                }
            }),
    )
    .buffered(CODEX_STATUS_REFRESH_CONCURRENCY);
    tokio::pin!(refreshes);
    while let Some((index, account_name, next)) = refreshes.next().await {
        if let Ok(latest) = status_store.codex_rate_limit_status().await {
            refreshed =
                rebase_unprocessed_refresh_statuses(&accounts, refreshed, Some(latest), index);
        }
        let previous = refreshed[index].clone();
        refreshed[index] = merge_background_refresh_result(previous, next);
        let latest_snapshot = status_store.codex_rate_limit_status().await.ok();
        let latest_accounts = prioritize_codex_refresh_targets(
            account_store
                .list_codex_status_refresh_targets()
                .await
                .context("reload Codex status refresh targets during status refresh")?,
            latest_snapshot.as_ref(),
        );
        status_store
            .save_codex_rate_limit_status(build_background_refresh_snapshot(
                &latest_accounts,
                &refreshed,
                latest_snapshot,
                Some(&account_name),
                &source_url,
                config.codex_status_refresh_max_interval_seconds,
            ))
            .await
            .context("persist incremental Codex public status snapshot")?;
        crate::allocator::collect_process_allocator();
    }
    crate::allocator::collect_process_allocator();
    Ok(())
}

fn prioritize_codex_refresh_targets(
    mut accounts: Vec<CodexStatusRefreshTarget>,
    existing: Option<&CodexRateLimitStatus>,
) -> Vec<CodexStatusRefreshTarget> {
    let status_by_name = existing
        .map(|status| {
            status
                .accounts
                .iter()
                .cloned()
                .map(|account| (account.name.clone(), account))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    accounts.retain(|account| account.status == KEY_STATUS_ACTIVE);
    accounts.sort_by(|left, right| {
        let left_status = status_by_name.get(&left.name);
        let right_status = status_by_name.get(&right.name);
        let left_success = left_status.and_then(|account| account.last_usage_success_at);
        let right_success = right_status.and_then(|account| account.last_usage_success_at);
        let left_checked = left_status.and_then(|account| account.last_usage_checked_at);
        let right_checked = right_status.and_then(|account| account.last_usage_checked_at);
        left_success
            .is_some()
            .cmp(&right_success.is_some())
            .then_with(|| {
                left_success
                    .unwrap_or(i64::MIN)
                    .cmp(&right_success.unwrap_or(i64::MIN))
            })
            .then_with(|| {
                left_checked
                    .unwrap_or(i64::MIN)
                    .cmp(&right_checked.unwrap_or(i64::MIN))
            })
            .then_with(|| left.name.cmp(&right.name))
    });
    accounts
}

pub(crate) async fn refresh_single_codex_account_status(
    config_store: &Arc<dyn AdminConfigStore>,
    account_store: &Arc<dyn AdminCodexAccountStore>,
    route_store: &Arc<dyn ProviderRouteStore>,
    status_store: &Arc<dyn PublicStatusStore>,
    account_name: &str,
) -> anyhow::Result<CodexPublicAccountStatus> {
    refresh_single_codex_account_status_with_mode(
        config_store,
        account_store,
        route_store,
        status_store,
        account_name,
        false,
    )
    .await
}

pub(crate) async fn refresh_single_codex_account_usage_only(
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
    let refresh_targets = account_store
        .list_codex_status_refresh_targets()
        .await
        .context("list Codex status refresh targets")?;
    let account = account_store
        .get_admin_codex_account(account_name)
        .await
        .context("load Codex account for status refresh")?
        .ok_or_else(|| anyhow::anyhow!("Codex account `{account_name}` not found"))?;
    let source_url = compute_usage_url(&provider::codex_upstream_base_url());
    let refreshed = refresh_account_status_with_current_access_token_only(
        &account,
        account_store.as_ref(),
        route_store.as_ref(),
        &config,
    )
    .await;
    let snapshot = merge_account_status_refresh(
        &refresh_targets,
        status_store.codex_rate_limit_status().await.ok(),
        account_name,
        refreshed.clone(),
        &source_url,
        config.codex_status_refresh_max_interval_seconds,
    );
    status_store
        .save_codex_rate_limit_status(snapshot)
        .await
        .context("persist manual Codex usage-only public status refresh")?;
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

pub(crate) async fn prime_single_codex_account_status(
    config_store: &Arc<dyn AdminConfigStore>,
    account_store: &Arc<dyn AdminCodexAccountStore>,
    route_store: &Arc<dyn ProviderRouteStore>,
    status_store: &Arc<dyn PublicStatusStore>,
    account_name: &str,
) -> anyhow::Result<CodexPublicAccountStatus> {
    refresh_single_codex_account_status_with_mode(
        config_store,
        account_store,
        route_store,
        status_store,
        account_name,
        false,
    )
    .await
}

async fn refresh_single_codex_account_status_with_mode(
    config_store: &Arc<dyn AdminConfigStore>,
    account_store: &Arc<dyn AdminCodexAccountStore>,
    route_store: &Arc<dyn ProviderRouteStore>,
    status_store: &Arc<dyn PublicStatusStore>,
    account_name: &str,
    force_refresh: bool,
) -> anyhow::Result<CodexPublicAccountStatus> {
    let config = config_store
        .get_admin_runtime_config()
        .await
        .context("load Codex status refresh config")?;
    let refresh_targets = account_store
        .list_codex_status_refresh_targets()
        .await
        .context("list Codex status refresh targets")?;
    let account = account_store
        .get_admin_codex_account(account_name)
        .await
        .context("load Codex account for status refresh")?
        .ok_or_else(|| anyhow::anyhow!("Codex account `{account_name}` not found"))?;
    let source_url = compute_usage_url(&provider::codex_upstream_base_url());
    let refreshed = refresh_account_status(
        &account,
        account_store.as_ref(),
        route_store.as_ref(),
        &config,
        force_refresh,
    )
    .await;
    let snapshot = merge_account_status_refresh(
        &refresh_targets,
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

fn seed_full_refresh_statuses<A: CodexStatusAccount>(
    accounts: &[A],
    existing: Option<CodexRateLimitStatus>,
) -> Vec<AccountStatusRefresh> {
    let Some(existing) = existing else {
        return accounts.iter().map(initial_account_status).collect();
    };
    let mut existing_accounts = existing
        .accounts
        .into_iter()
        .map(|account| (account.name.clone(), account))
        .collect::<BTreeMap<_, _>>();
    let mut existing_buckets = existing
        .buckets
        .into_iter()
        .filter_map(|bucket| bucket.account_name.clone().map(|name| (name, bucket)))
        .fold(
            BTreeMap::<String, Vec<CodexRateLimitBucket>>::new(),
            |mut grouped, (name, bucket)| {
                grouped.entry(name).or_default().push(bucket);
                grouped
            },
        );

    accounts
        .iter()
        .map(|account| {
            if account.status() != KEY_STATUS_ACTIVE {
                return initial_account_status(account);
            }
            let Some(public_account) = existing_accounts.remove(account.name()) else {
                return initial_account_status(account);
            };
            if public_account.status != account.status() {
                return initial_account_status(account);
            }
            let buckets = existing_buckets.remove(account.name()).unwrap_or_default();
            if !buckets.is_empty() {
                AccountStatusRefresh::Ready {
                    account: public_account,
                    buckets,
                }
            } else if public_account.usage_error_message.is_some() {
                AccountStatusRefresh::Error {
                    account: public_account,
                }
            } else {
                AccountStatusRefresh::Skipped {
                    account: public_account,
                }
            }
        })
        .collect()
}

fn account_status_refresh_from_cached_snapshot<A: CodexStatusAccount>(
    account: &A,
    existing_accounts: &mut BTreeMap<String, CodexPublicAccountStatus>,
    existing_buckets: &mut BTreeMap<String, Vec<CodexRateLimitBucket>>,
) -> Option<AccountStatusRefresh> {
    if account.status() != KEY_STATUS_ACTIVE {
        return None;
    }
    let public_account = existing_accounts.remove(account.name())?;
    if public_account.status != account.status() {
        return None;
    }
    let buckets = existing_buckets.remove(account.name()).unwrap_or_default();
    if !buckets.is_empty() {
        Some(AccountStatusRefresh::Ready {
            account: public_account,
            buckets,
        })
    } else if public_account.usage_error_message.is_some() {
        Some(AccountStatusRefresh::Error {
            account: public_account,
        })
    } else {
        Some(AccountStatusRefresh::Skipped {
            account: public_account,
        })
    }
}

fn rebase_unprocessed_refresh_statuses<A: CodexStatusAccount>(
    accounts: &[A],
    mut refreshed: Vec<AccountStatusRefresh>,
    latest: Option<CodexRateLimitStatus>,
    processed_until: usize,
) -> Vec<AccountStatusRefresh> {
    let Some(latest) = latest else {
        return refreshed;
    };
    let mut existing_accounts = latest
        .accounts
        .into_iter()
        .map(|account| (account.name.clone(), account))
        .collect::<BTreeMap<_, _>>();
    let mut existing_buckets = latest
        .buckets
        .into_iter()
        .filter_map(|bucket| bucket.account_name.clone().map(|name| (name, bucket)))
        .fold(
            BTreeMap::<String, Vec<CodexRateLimitBucket>>::new(),
            |mut grouped, (name, bucket)| {
                grouped.entry(name).or_default().push(bucket);
                grouped
            },
        );
    for (index, account) in accounts.iter().enumerate().skip(processed_until) {
        if let Some(rebased) = account_status_refresh_from_cached_snapshot(
            account,
            &mut existing_accounts,
            &mut existing_buckets,
        ) {
            refreshed[index] = rebased;
        }
    }
    refreshed
}

fn initial_account_status<A: CodexStatusAccount>(account: &A) -> AccountStatusRefresh {
    if account.status() == KEY_STATUS_ACTIVE {
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
                name: account.name().to_string(),
                status: account.status().to_string(),
                plan_type: account.plan_type().map(str::to_string),
                primary_remaining_percent: account.primary_remaining_percent(),
                secondary_remaining_percent: account.secondary_remaining_percent(),
                last_usage_checked_at: None,
                last_usage_success_at: account.last_usage_success_at(),
                usage_error_message: account.usage_error_message().map(str::to_string),
            },
        }
    }
}

async fn refresh_account_status<A: CodexStatusAccount>(
    account: &A,
    account_store: &dyn AdminCodexAccountStore,
    route_store: &dyn ProviderRouteStore,
    config: &AdminRuntimeConfig,
    force_refresh: bool,
) -> AccountStatusRefresh {
    let now = now_ms();
    if account.status() != KEY_STATUS_ACTIVE {
        return AccountStatusRefresh::Skipped {
            account: CodexPublicAccountStatus {
                name: account.name().to_string(),
                status: account.status().to_string(),
                plan_type: account.plan_type().map(str::to_string),
                primary_remaining_percent: account.primary_remaining_percent(),
                secondary_remaining_percent: account.secondary_remaining_percent(),
                last_usage_checked_at: Some(now),
                last_usage_success_at: account.last_usage_success_at(),
                usage_error_message: account.usage_error_message().map(str::to_string),
            },
        };
    }
    let Some(route) = account_store
        .resolve_admin_codex_account_route(account.name())
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
            account: account_error_status(account, now, &format!("{err:#}")),
        },
    }
}

async fn refresh_account_status_with_current_access_token_only<A: CodexStatusAccount>(
    account: &A,
    account_store: &dyn AdminCodexAccountStore,
    route_store: &dyn ProviderRouteStore,
    config: &AdminRuntimeConfig,
) -> AccountStatusRefresh {
    let now = now_ms();
    if account.status() != KEY_STATUS_ACTIVE {
        return AccountStatusRefresh::Skipped {
            account: CodexPublicAccountStatus {
                name: account.name().to_string(),
                status: account.status().to_string(),
                plan_type: account.plan_type().map(str::to_string),
                primary_remaining_percent: account.primary_remaining_percent(),
                secondary_remaining_percent: account.secondary_remaining_percent(),
                last_usage_checked_at: Some(now),
                last_usage_success_at: account.last_usage_success_at(),
                usage_error_message: account.usage_error_message().map(str::to_string),
            },
        };
    }
    let Some(route) = account_store
        .resolve_admin_codex_account_route(account.name())
        .await
        .ok()
        .flatten()
    else {
        return AccountStatusRefresh::Error {
            account: account_error_status(account, now, "active Codex route is not configured"),
        };
    };
    match fetch_route_usage_with_current_access_token_only(&route, route_store, config).await {
        Ok(buckets) => AccountStatusRefresh::Ready {
            account: account_ready_status(account, now, &buckets),
            buckets,
        },
        Err(err) => AccountStatusRefresh::Error {
            account: account_error_status(account, now, &format!("{err:#}")),
        },
    }
}

fn merge_background_refresh_result(
    previous: AccountStatusRefresh,
    refreshed: AccountStatusRefresh,
) -> AccountStatusRefresh {
    match (previous, refreshed) {
        (
            AccountStatusRefresh::Ready {
                mut account,
                buckets,
            },
            AccountStatusRefresh::Error {
                account: error_account,
            },
        ) => {
            if error_account
                .usage_error_message
                .as_deref()
                .is_some_and(is_terminal_codex_auth_error)
            {
                tracing::warn!(
                    account_name = %account.name,
                    last_usage_success_at = account.last_usage_success_at.unwrap_or(0),
                    error = %error_account
                        .usage_error_message
                        .as_deref()
                        .unwrap_or("unknown Codex usage refresh error"),
                    "background Codex usage refresh hit terminal auth error; marking account unavailable",
                );
                return AccountStatusRefresh::Error {
                    account: error_account,
                };
            }
            account.last_usage_checked_at = error_account.last_usage_checked_at;
            tracing::warn!(
                account_name = %account.name,
                last_usage_success_at = account.last_usage_success_at.unwrap_or(0),
                error = %error_account
                    .usage_error_message
                    .as_deref()
                    .unwrap_or("unknown Codex usage refresh error"),
                "background Codex usage refresh failed; preserving last known good snapshot",
            );
            AccountStatusRefresh::Ready {
                account,
                buckets,
            }
        },
        (_, refreshed) => refreshed,
    }
}

fn merge_account_status_refresh<A: CodexStatusAccount>(
    accounts: &[A],
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
        if account.name() == refreshed_name {
            merged.push(refreshed.clone());
            continue;
        }
        let Some(public_account) = existing_accounts.remove(account.name()) else {
            merged.push(initial_account_status(account));
            continue;
        };
        let buckets = existing_buckets.remove(account.name()).unwrap_or_default();
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

fn build_background_refresh_snapshot<A: CodexStatusAccount>(
    accounts: &[A],
    refreshed: &[AccountStatusRefresh],
    existing: Option<CodexRateLimitStatus>,
    preserved_name: Option<&str>,
    source_url: &str,
    refresh_interval_seconds: u64,
) -> CodexRateLimitStatus {
    let refreshed_by_name = refreshed
        .iter()
        .cloned()
        .map(|status| (account_status_refresh_name(&status).to_string(), status))
        .collect::<BTreeMap<_, _>>();
    let merged =
        merge_background_refresh_accounts(accounts, refreshed_by_name, existing, preserved_name);
    build_status_snapshot(merged, source_url, refresh_interval_seconds)
}

fn merge_background_refresh_accounts<A: CodexStatusAccount>(
    accounts: &[A],
    mut refreshed_by_name: BTreeMap<String, AccountStatusRefresh>,
    existing: Option<CodexRateLimitStatus>,
    preserved_name: Option<&str>,
) -> Vec<AccountStatusRefresh> {
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
    accounts
        .iter()
        .map(|account| {
            if preserved_name == Some(account.name()) {
                if let Some(status) = refreshed_by_name.remove(account.name()) {
                    return status;
                }
            }
            if let Some(status) = account_status_refresh_from_cached_snapshot(
                account,
                &mut existing_accounts,
                &mut existing_buckets,
            ) {
                return status;
            }
            refreshed_by_name
                .remove(account.name())
                .unwrap_or_else(|| initial_account_status(account))
        })
        .collect()
}

fn account_status_refresh_name(refresh: &AccountStatusRefresh) -> &str {
    match refresh {
        AccountStatusRefresh::Ready {
            account, ..
        }
        | AccountStatusRefresh::Error {
            account,
        }
        | AccountStatusRefresh::Skipped {
            account,
        } => &account.name,
    }
}

async fn fetch_route_usage(
    route: &ProviderCodexRoute,
    route_store: &dyn ProviderRouteStore,
    config: &AdminRuntimeConfig,
    force_refresh: bool,
) -> anyhow::Result<Vec<CodexRateLimitBucket>> {
    let mut force_refresh_attempt = force_refresh;
    loop {
        let context = match codex_refresh::ensure_context_for_route(
            route,
            route_store,
            force_refresh_attempt,
        )
        .await
        {
            Ok(context) => context,
            Err(err) => {
                if force_refresh_attempt && is_terminal_codex_auth_error(&format!("{err:#}")) {
                    if let Some(context) =
                        codex_refresh::current_unexpired_context_for_route(route, route_store)
                            .await?
                    {
                        match fetch_route_usage_once(route, config, &context).await? {
                            FetchRouteUsageOutcome::Ready(mut buckets) => {
                                if let Err(disable_err) =
                                    codex_refresh::disable_auto_refresh_for_route(
                                        route,
                                        route_store,
                                    )
                                    .await
                                {
                                    tracing::warn!(
                                        account_name = %route.account_name,
                                        error = ?disable_err,
                                        "failed to disable Codex auto refresh after direct access-token usage fallback",
                                    );
                                }
                                for bucket in &mut buckets {
                                    bucket.account_name = Some(route.account_name.clone());
                                }
                                return Ok(buckets);
                            },
                            FetchRouteUsageOutcome::Unauthorized {
                                body,
                            } => {
                                tracing::warn!(
                                    account_name = %route.account_name,
                                    body,
                                    "Codex access token remained unusable after terminal refresh failure",
                                );
                            },
                        }
                    }
                }
                return Err(err);
            },
        };
        match fetch_route_usage_once(route, config, &context).await? {
            FetchRouteUsageOutcome::Ready(mut buckets) => {
                for bucket in &mut buckets {
                    bucket.account_name = Some(route.account_name.clone());
                }
                return Ok(buckets);
            },
            FetchRouteUsageOutcome::Unauthorized {
                body,
            } if !force_refresh_attempt => {
                let _ = body;
                force_refresh_attempt = true;
            },
            FetchRouteUsageOutcome::Unauthorized {
                body,
            } => {
                anyhow::bail!("Codex usage status returned 401 Unauthorized: {body}");
            },
        }
    }
}

async fn fetch_route_usage_with_current_access_token_only(
    route: &ProviderCodexRoute,
    route_store: &dyn ProviderRouteStore,
    config: &AdminRuntimeConfig,
) -> anyhow::Result<Vec<CodexRateLimitBucket>> {
    let Some(context) = codex_refresh::current_unexpired_context_for_route(route, route_store)
        .await
        .context("load current Codex access token context")?
    else {
        anyhow::bail!("Codex current access token is missing or expired");
    };
    match fetch_route_usage_once(route, config, &context).await? {
        FetchRouteUsageOutcome::Ready(mut buckets) => {
            for bucket in &mut buckets {
                bucket.account_name = Some(route.account_name.clone());
            }
            Ok(buckets)
        },
        FetchRouteUsageOutcome::Unauthorized {
            body,
        } => anyhow::bail!("Codex usage status returned 401 Unauthorized: {body}"),
    }
}

enum FetchRouteUsageOutcome {
    Ready(Vec<CodexRateLimitBucket>),
    Unauthorized { body: String },
}

async fn fetch_route_usage_once(
    route: &ProviderCodexRoute,
    config: &AdminRuntimeConfig,
    context: &codex_refresh::CodexCallContext,
) -> anyhow::Result<FetchRouteUsageOutcome> {
    let source_url = compute_usage_url(&provider::codex_upstream_base_url());
    let client_version = provider::resolve_codex_client_version(Some(&config.codex_client_version));
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
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Ok(FetchRouteUsageOutcome::Unauthorized {
            body,
        });
    }
    if !status.is_success() {
        anyhow::bail!("Codex usage status returned {status}: {body}");
    }
    let payload =
        serde_json::from_str::<UsageStatusPayload>(&body).context("parse Codex usage status")?;
    Ok(FetchRouteUsageOutcome::Ready(map_rate_limit_status_payload(payload)))
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
    let mut configured_count = 0_usize;
    let mut active_count = 0_usize;
    let mut success_count = 0_usize;
    let mut errors = Vec::new();
    for item in refreshed {
        match item {
            AccountStatusRefresh::Ready {
                account,
                buckets: account_buckets,
            } => {
                configured_count += 1;
                if account.status != KEY_STATUS_ACTIVE {
                    continue;
                }
                active_count += 1;
                success_count += 1;
                buckets.extend(account_buckets);
                accounts.push(account);
            },
            AccountStatusRefresh::Error {
                account,
            } => {
                configured_count += 1;
                if account.status != KEY_STATUS_ACTIVE {
                    continue;
                }
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
            } => {
                configured_count += 1;
                if account.status == KEY_STATUS_ACTIVE {
                    accounts.push(account);
                }
            },
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
            configured_count
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

fn account_ready_status<A: CodexStatusAccount>(
    account: &A,
    checked_at: i64,
    buckets: &[CodexRateLimitBucket],
) -> CodexPublicAccountStatus {
    let primary = buckets.iter().find(|bucket| bucket.is_primary);
    CodexPublicAccountStatus {
        name: account.name().to_string(),
        status: account.status().to_string(),
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

fn account_error_status<A: CodexStatusAccount>(
    account: &A,
    checked_at: i64,
    message: &str,
) -> CodexPublicAccountStatus {
    CodexPublicAccountStatus {
        name: account.name().to_string(),
        status: account.status().to_string(),
        plan_type: account.plan_type().map(str::to_string),
        primary_remaining_percent: account.primary_remaining_percent(),
        secondary_remaining_percent: account.secondary_remaining_percent(),
        last_usage_checked_at: Some(checked_at),
        last_usage_success_at: account.last_usage_success_at(),
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
            route_weight_tier: "auto".to_string(),
            primary_remaining_percent: None,
            secondary_remaining_percent: None,
            map_gpt53_codex_to_spark: false,
            auto_refresh_enabled: true,
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
            proxy_mode: "inherit".to_string(),
            proxy_config_id: None,
            effective_proxy_source: "binding".to_string(),
            effective_proxy_url: None,
            effective_proxy_config_name: None,
            last_refresh: Some(100),
            access_token_expires_at: None,
            auth_refresh_error_message: None,
            last_usage_checked_at: None,
            last_usage_success_at: None,
            usage_error_message: None,
        }
    }

    fn sample_refresh_target(name: &str) -> CodexStatusRefreshTarget {
        CodexStatusRefreshTarget {
            name: name.to_string(),
            status: KEY_STATUS_ACTIVE.to_string(),
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

    #[test]
    fn manual_account_refresh_can_merge_against_lightweight_refresh_targets() {
        let targets = vec![sample_refresh_target("alpha"), sample_refresh_target("beta")];
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
            account: account_ready_status(
                &sample_admin_account("beta"),
                1200,
                std::slice::from_ref(&beta_bucket),
            ),
            buckets: vec![beta_bucket],
        };

        let snapshot = merge_account_status_refresh(
            &targets,
            Some(existing),
            "beta",
            refreshed,
            "https://chatgpt.com/backend-api/wham/usage",
            300,
        );

        assert_eq!(snapshot.accounts.len(), 2);
        assert_eq!(snapshot.accounts[0].name, "alpha");
        assert_eq!(snapshot.accounts[1].name, "beta");
        assert_eq!(snapshot.buckets.len(), 2);
    }

    #[test]
    fn full_refresh_seed_preserves_cached_account_statuses() {
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
            accounts: vec![
                CodexPublicAccountStatus {
                    name: "alpha".to_string(),
                    status: KEY_STATUS_ACTIVE.to_string(),
                    plan_type: Some("Pro".to_string()),
                    primary_remaining_percent: Some(70.0),
                    secondary_remaining_percent: Some(80.0),
                    last_usage_checked_at: Some(900),
                    last_usage_success_at: Some(900),
                    usage_error_message: None,
                },
                CodexPublicAccountStatus {
                    name: "beta".to_string(),
                    status: KEY_STATUS_ACTIVE.to_string(),
                    plan_type: Some("Pro".to_string()),
                    primary_remaining_percent: Some(62.0),
                    secondary_remaining_percent: Some(39.0),
                    last_usage_checked_at: Some(900),
                    last_usage_success_at: Some(900),
                    usage_error_message: None,
                },
            ],
            buckets: vec![alpha_bucket, beta_bucket],
        };

        let seeded = seed_full_refresh_statuses(&accounts, Some(existing));
        let snapshot =
            build_status_snapshot(seeded, "https://chatgpt.com/backend-api/wham/usage", 300);

        assert_eq!(snapshot.status, "ready");
        assert_eq!(snapshot.accounts.len(), 2);
        assert!(snapshot
            .accounts
            .iter()
            .all(|account| account.usage_error_message.is_none()));
        assert_eq!(snapshot.accounts[0].primary_remaining_percent, Some(70.0));
        assert_eq!(snapshot.accounts[1].secondary_remaining_percent, Some(39.0));
        assert_eq!(snapshot.buckets.len(), 2);
    }

    #[test]
    fn rebase_unprocessed_refresh_statuses_keeps_newer_manual_snapshot() {
        let accounts = vec![sample_admin_account("alpha"), sample_admin_account("beta")];
        let initial = vec![
            AccountStatusRefresh::Ready {
                account: account_ready_status(
                    &accounts[0],
                    900,
                    std::slice::from_ref(&sample_bucket("alpha", 70.0, 80.0)),
                ),
                buckets: vec![sample_bucket("alpha", 70.0, 80.0)],
            },
            AccountStatusRefresh::Error {
                account: account_error_status(
                    &accounts[1],
                    910,
                    "usage refresh pending for standalone llm-access",
                ),
            },
        ];
        let latest = CodexRateLimitStatus {
            status: "ready".to_string(),
            refresh_interval_seconds: 300,
            last_checked_at: Some(1200),
            last_success_at: Some(1200),
            source_url: "https://chatgpt.com/backend-api/wham/usage".to_string(),
            error_message: None,
            accounts: vec![
                CodexPublicAccountStatus {
                    name: "alpha".to_string(),
                    status: KEY_STATUS_ACTIVE.to_string(),
                    plan_type: Some("Pro".to_string()),
                    primary_remaining_percent: Some(70.0),
                    secondary_remaining_percent: Some(80.0),
                    last_usage_checked_at: Some(900),
                    last_usage_success_at: Some(900),
                    usage_error_message: None,
                },
                CodexPublicAccountStatus {
                    name: "beta".to_string(),
                    status: KEY_STATUS_ACTIVE.to_string(),
                    plan_type: Some("Plus".to_string()),
                    primary_remaining_percent: Some(99.0),
                    secondary_remaining_percent: Some(100.0),
                    last_usage_checked_at: Some(1200),
                    last_usage_success_at: Some(1200),
                    usage_error_message: None,
                },
            ],
            buckets: vec![sample_bucket("alpha", 70.0, 80.0), sample_bucket("beta", 99.0, 100.0)],
        };

        let rebased = rebase_unprocessed_refresh_statuses(&accounts, initial, Some(latest), 1);

        match &rebased[1] {
            AccountStatusRefresh::Ready {
                account,
                buckets,
            } => {
                assert_eq!(account.name, "beta");
                assert_eq!(account.plan_type.as_deref(), Some("Plus"));
                assert_eq!(account.primary_remaining_percent, Some(99.0));
                assert_eq!(account.secondary_remaining_percent, Some(100.0));
                assert_eq!(account.last_usage_success_at, Some(1200));
                assert_eq!(account.usage_error_message, None);
                assert_eq!(buckets.len(), 1);
                assert_eq!(buckets[0].account_name.as_deref(), Some("beta"));
            },
            _ => panic!("expected rebased ready status"),
        }
    }

    #[test]
    fn background_snapshot_preserves_new_accounts_from_latest_snapshot() {
        let original_accounts = [sample_admin_account("alpha"), sample_admin_account("beta")];
        let current_accounts = vec![
            sample_admin_account("alpha"),
            sample_admin_account("beta"),
            sample_admin_account("gamma"),
        ];
        let alpha_old_bucket = sample_bucket("alpha", 70.0, 80.0);
        let alpha_new_bucket = sample_bucket("alpha", 55.0, 66.0);
        let beta_bucket = sample_bucket("beta", 99.0, 100.0);
        let gamma_bucket = sample_bucket("gamma", 88.0, 77.0);
        let refreshed = vec![
            AccountStatusRefresh::Ready {
                account: account_ready_status(
                    &original_accounts[0],
                    900,
                    std::slice::from_ref(&alpha_old_bucket),
                ),
                buckets: vec![alpha_old_bucket],
            },
            AccountStatusRefresh::Error {
                account: account_error_status(
                    &original_accounts[1],
                    910,
                    "usage refresh pending for standalone llm-access",
                ),
            },
        ];
        let latest = CodexRateLimitStatus {
            status: "ready".to_string(),
            refresh_interval_seconds: 300,
            last_checked_at: Some(1200),
            last_success_at: Some(1200),
            source_url: "https://chatgpt.com/backend-api/wham/usage".to_string(),
            error_message: None,
            accounts: vec![
                CodexPublicAccountStatus {
                    name: "alpha".to_string(),
                    status: KEY_STATUS_ACTIVE.to_string(),
                    plan_type: Some("Pro".to_string()),
                    primary_remaining_percent: Some(55.0),
                    secondary_remaining_percent: Some(66.0),
                    last_usage_checked_at: Some(1200),
                    last_usage_success_at: Some(1200),
                    usage_error_message: None,
                },
                CodexPublicAccountStatus {
                    name: "beta".to_string(),
                    status: KEY_STATUS_ACTIVE.to_string(),
                    plan_type: Some("Plus".to_string()),
                    primary_remaining_percent: Some(99.0),
                    secondary_remaining_percent: Some(100.0),
                    last_usage_checked_at: Some(1200),
                    last_usage_success_at: Some(1200),
                    usage_error_message: None,
                },
                CodexPublicAccountStatus {
                    name: "gamma".to_string(),
                    status: KEY_STATUS_ACTIVE.to_string(),
                    plan_type: Some("Plus".to_string()),
                    primary_remaining_percent: Some(88.0),
                    secondary_remaining_percent: Some(77.0),
                    last_usage_checked_at: Some(1200),
                    last_usage_success_at: Some(1200),
                    usage_error_message: None,
                },
            ],
            buckets: vec![alpha_new_bucket, beta_bucket, gamma_bucket],
        };

        let snapshot = build_background_refresh_snapshot(
            &current_accounts,
            &refreshed,
            Some(latest),
            Some("alpha"),
            "https://chatgpt.com/backend-api/wham/usage",
            300,
        );

        assert_eq!(snapshot.accounts.len(), 3);
        assert_eq!(snapshot.accounts[0].name, "alpha");
        assert_eq!(snapshot.accounts[0].primary_remaining_percent, Some(70.0));
        assert_eq!(snapshot.accounts[1].name, "beta");
        assert_eq!(snapshot.accounts[1].primary_remaining_percent, Some(99.0));
        assert_eq!(snapshot.accounts[2].name, "gamma");
        assert_eq!(snapshot.accounts[2].primary_remaining_percent, Some(88.0));
        assert_eq!(snapshot.buckets.len(), 3);
        assert!(snapshot
            .buckets
            .iter()
            .any(|bucket| bucket.account_name.as_deref() == Some("gamma")));
    }

    #[test]
    fn background_refresh_preserves_last_good_account_status_after_transient_error() {
        let previous_bucket = sample_bucket("alpha", 70.0, 80.0);
        let previous = AccountStatusRefresh::Ready {
            account: account_ready_status(
                &sample_admin_account("alpha"),
                900,
                std::slice::from_ref(&previous_bucket),
            ),
            buckets: vec![previous_bucket.clone()],
        };
        let refreshed = AccountStatusRefresh::Error {
            account: account_error_status(
                &sample_admin_account("alpha"),
                1200,
                "request Codex usage status: deadline has elapsed",
            ),
        };

        let merged = merge_background_refresh_result(previous, refreshed);

        match merged {
            AccountStatusRefresh::Ready {
                account,
                buckets,
            } => {
                assert_eq!(account.primary_remaining_percent, Some(70.0));
                assert_eq!(account.secondary_remaining_percent, Some(80.0));
                assert_eq!(account.last_usage_checked_at, Some(1200));
                assert_eq!(account.last_usage_success_at, Some(900));
                assert_eq!(account.usage_error_message, None);
                assert_eq!(buckets.len(), 1);
                assert_eq!(buckets[0].account_name.as_deref(), Some("alpha"));
            },
            _ => panic!("expected ready snapshot"),
        }
    }

    #[test]
    fn background_refresh_keeps_error_when_no_last_good_snapshot_exists() {
        let previous = AccountStatusRefresh::Error {
            account: account_error_status(
                &sample_admin_account("alpha"),
                900,
                "usage refresh pending for standalone llm-access",
            ),
        };
        let refreshed = AccountStatusRefresh::Error {
            account: account_error_status(
                &sample_admin_account("alpha"),
                1200,
                "request Codex usage status: deadline has elapsed",
            ),
        };

        let merged = merge_background_refresh_result(previous, refreshed.clone());

        match merged {
            AccountStatusRefresh::Error {
                account,
            } => {
                assert_eq!(account.last_usage_checked_at, Some(1200));
                assert_eq!(
                    account.usage_error_message.as_deref(),
                    Some("request Codex usage status: deadline has elapsed")
                );
            },
            _ => panic!("expected error snapshot"),
        }
    }

    #[test]
    fn background_refresh_marks_terminal_auth_error_unavailable() {
        let previous_bucket = sample_bucket("alpha", 70.0, 80.0);
        let previous = AccountStatusRefresh::Ready {
            account: account_ready_status(
                &sample_admin_account("alpha"),
                900,
                std::slice::from_ref(&previous_bucket),
            ),
            buckets: vec![previous_bucket],
        };
        let refreshed = AccountStatusRefresh::Error {
            account: account_error_status(
                &sample_admin_account("alpha"),
                1200,
                "codex refresh token returned 401 Unauthorized: \
                 {\"error\":{\"code\":\"refresh_token_invalidated\"}}",
            ),
        };

        let merged = merge_background_refresh_result(previous, refreshed);

        match merged {
            AccountStatusRefresh::Error {
                account,
            } => {
                assert_eq!(account.last_usage_checked_at, Some(1200));
                assert_eq!(account.last_usage_success_at, None);
                assert_eq!(
                    account.usage_error_message.as_deref(),
                    Some(
                        "codex refresh token returned 401 Unauthorized: \
                         {\"error\":{\"code\":\"refresh_token_invalidated\"}}"
                    )
                );
            },
            _ => panic!("expected terminal auth error to mark account unavailable"),
        }
    }

    #[test]
    fn prioritize_codex_refresh_targets_keeps_only_active_and_oldest_first() {
        let existing = CodexRateLimitStatus {
            status: "ready".to_string(),
            refresh_interval_seconds: 300,
            last_checked_at: Some(1500),
            last_success_at: Some(1500),
            source_url: "https://chatgpt.com/backend-api/wham/usage".to_string(),
            error_message: None,
            accounts: vec![
                CodexPublicAccountStatus {
                    name: "fresh".to_string(),
                    status: KEY_STATUS_ACTIVE.to_string(),
                    plan_type: Some("Plus".to_string()),
                    primary_remaining_percent: Some(90.0),
                    secondary_remaining_percent: Some(80.0),
                    last_usage_checked_at: Some(1400),
                    last_usage_success_at: Some(1400),
                    usage_error_message: None,
                },
                CodexPublicAccountStatus {
                    name: "stale".to_string(),
                    status: KEY_STATUS_ACTIVE.to_string(),
                    plan_type: Some("Plus".to_string()),
                    primary_remaining_percent: Some(30.0),
                    secondary_remaining_percent: Some(20.0),
                    last_usage_checked_at: Some(600),
                    last_usage_success_at: Some(600),
                    usage_error_message: None,
                },
            ],
            buckets: vec![],
        };
        let accounts = vec![
            CodexStatusRefreshTarget {
                name: "disabled".to_string(),
                status: "disabled".to_string(),
            },
            CodexStatusRefreshTarget {
                name: "fresh".to_string(),
                status: KEY_STATUS_ACTIVE.to_string(),
            },
            CodexStatusRefreshTarget {
                name: "never".to_string(),
                status: KEY_STATUS_ACTIVE.to_string(),
            },
            CodexStatusRefreshTarget {
                name: "stale".to_string(),
                status: KEY_STATUS_ACTIVE.to_string(),
            },
        ];

        let prioritized = prioritize_codex_refresh_targets(accounts, Some(&existing));
        let names = prioritized
            .iter()
            .map(|account| account.name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(names, vec!["never", "stale", "fresh"]);
    }
}
