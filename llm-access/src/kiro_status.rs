//! Kiro account status refresh loop for the standalone LLM access service.

use std::{sync::Arc, time::Duration};

use anyhow::Context;
use llm_access_core::store::{
    AdminConfigStore, AdminKiroAccount, AdminKiroAccountStore, AdminKiroBalanceView,
    AdminKiroCacheView, AdminKiroStatusCacheUpdate, AdminRuntimeConfig, ProviderKiroRoute,
    ProviderRouteStore,
};
use llm_access_kiro::wire::UsageLimitsResponse;
use rand::Rng;

use crate::{kiro_refresh, runtime::LlmAccessRuntime};

/// Start the same Kiro status warmup and periodic refresh loop that the
/// monolithic backend used before request selection.
pub(crate) fn spawn_kiro_status_refresher(runtime: &LlmAccessRuntime) {
    let config_store = runtime.admin_config_store();
    let account_store = runtime.admin_kiro_account_store();
    let route_store = runtime.provider_route_store();
    tokio::spawn({
        let config_store = Arc::clone(&config_store);
        let account_store = Arc::clone(&account_store);
        let route_store = Arc::clone(&route_store);
        async move {
            if let Err(err) =
                refresh_all_kiro_statuses(&config_store, &account_store, &route_store).await
            {
                tracing::warn!("initial Kiro cached status refresh failed: {err:#}");
            }
        }
    });
    tokio::spawn(async move {
        loop {
            let delay = match config_store.get_admin_runtime_config().await {
                Ok(config) => next_kiro_refresh_delay(&config),
                Err(err) => {
                    tracing::warn!("failed to load Kiro status refresh config: {err:#}");
                    next_kiro_refresh_delay(&AdminRuntimeConfig::default())
                },
            };
            tokio::time::sleep(delay).await;
            if let Err(err) =
                refresh_all_kiro_statuses(&config_store, &account_store, &route_store).await
            {
                tracing::warn!("failed to refresh cached Kiro statuses: {err:#}");
            }
        }
    });
}

/// Refresh and persist status for routes missing a status-cache entry before
/// the hot request path tries to select an account.
pub(crate) async fn ensure_missing_route_statuses(
    routes: &[ProviderKiroRoute],
    route_store: &dyn ProviderRouteStore,
) {
    for route in routes.iter().filter(|route| route.cached_status.is_none()) {
        match refresh_and_persist_route_status(route, route_store, false).await {
            Ok(update) => {
                tracing::info!(
                    account_name = %route.account_name,
                    status = %update.cache.status,
                    "refreshed missing Kiro status before request selection"
                );
            },
            Err(err) => {
                tracing::warn!(
                    account_name = %route.account_name,
                    error = %err,
                    "failed to persist missing Kiro status before request selection"
                );
            },
        }
    }
}

async fn refresh_all_kiro_statuses(
    config_store: &Arc<dyn AdminConfigStore>,
    account_store: &Arc<dyn AdminKiroAccountStore>,
    route_store: &Arc<dyn ProviderRouteStore>,
) -> anyhow::Result<()> {
    let config = config_store
        .get_admin_runtime_config()
        .await
        .context("load Kiro status refresh config")?;
    let accounts = account_store
        .list_admin_kiro_accounts()
        .await
        .context("list Kiro accounts for status refresh")?;
    for (index, account) in accounts.into_iter().enumerate() {
        if index > 0 {
            let jitter = next_kiro_account_jitter(&config);
            if !jitter.is_zero() {
                tokio::time::sleep(jitter).await;
            }
        }
        if let Err(err) =
            refresh_account_status(&account, account_store.as_ref(), route_store.as_ref()).await
        {
            tracing::warn!(
                account_name = %account.name,
                error = %err,
                "failed to refresh Kiro account status"
            );
        }
    }
    Ok(())
}

async fn refresh_account_status(
    account: &AdminKiroAccount,
    account_store: &dyn AdminKiroAccountStore,
    route_store: &dyn ProviderRouteStore,
) -> anyhow::Result<()> {
    if account.disabled {
        let now = now_ms();
        let update = build_disabled_status_update(account, now);
        account_store.save_admin_kiro_status_cache(update).await?;
        return Ok(());
    }
    let Some(route) = account_store
        .resolve_admin_kiro_account_route(&account.name)
        .await?
    else {
        return Ok(());
    };
    let update = refresh_route_status_update(&route, route_store, false).await;
    account_store.save_admin_kiro_status_cache(update).await
}

pub(crate) async fn refresh_and_persist_route_status(
    route: &ProviderKiroRoute,
    route_store: &dyn ProviderRouteStore,
    force_refresh: bool,
) -> anyhow::Result<AdminKiroStatusCacheUpdate> {
    let update = refresh_route_status_update(route, route_store, force_refresh).await;
    route_store
        .save_kiro_status_cache_update(update.clone())
        .await?;
    Ok(update)
}

async fn refresh_route_status_update(
    route: &ProviderKiroRoute,
    route_store: &dyn ProviderRouteStore,
    force_refresh: bool,
) -> AdminKiroStatusCacheUpdate {
    let now = now_ms();
    match kiro_refresh::fetch_usage_limits_for_route(route, route_store, force_refresh).await {
        Ok(usage) => build_ready_status_update(route, now, &usage),
        Err(err) => build_error_status_update(route, now, err.to_string()),
    }
}

fn build_ready_status_update(
    route: &ProviderKiroRoute,
    now: i64,
    usage: &UsageLimitsResponse,
) -> AdminKiroStatusCacheUpdate {
    let balance = admin_kiro_balance_from_usage(usage);
    let cache = AdminKiroCacheView {
        status: "ready".to_string(),
        refresh_interval_seconds: route.status_refresh_interval_seconds,
        last_checked_at: Some(now),
        last_success_at: Some(now),
        error_message: None,
    };
    AdminKiroStatusCacheUpdate {
        account_name: route.account_name.clone(),
        balance: Some(balance),
        refreshed_at_ms: now,
        expires_at_ms: status_expires_at(now, route.status_refresh_interval_seconds),
        cache,
        last_error: None,
    }
}

fn build_error_status_update(
    route: &ProviderKiroRoute,
    now: i64,
    error_message: String,
) -> AdminKiroStatusCacheUpdate {
    let has_prior_balance = route.cached_balance.is_some();
    let cache = AdminKiroCacheView {
        status: if has_prior_balance { "degraded" } else { "error" }.to_string(),
        refresh_interval_seconds: route.status_refresh_interval_seconds,
        last_checked_at: Some(now),
        last_success_at: route
            .cached_cache
            .as_ref()
            .and_then(|cache| cache.last_success_at),
        error_message: Some(error_message.clone()),
    };
    AdminKiroStatusCacheUpdate {
        account_name: route.account_name.clone(),
        balance: route.cached_balance.clone(),
        refreshed_at_ms: now,
        expires_at_ms: status_expires_at(now, route.status_refresh_interval_seconds),
        cache,
        last_error: Some(error_message),
    }
}

fn build_disabled_status_update(
    account: &AdminKiroAccount,
    now: i64,
) -> AdminKiroStatusCacheUpdate {
    let refresh_interval_seconds = account.cache.refresh_interval_seconds;
    let cache = AdminKiroCacheView {
        status: "disabled".to_string(),
        refresh_interval_seconds,
        last_checked_at: Some(now),
        last_success_at: account.cache.last_success_at,
        error_message: None,
    };
    AdminKiroStatusCacheUpdate {
        account_name: account.name.clone(),
        balance: account.balance.clone(),
        refreshed_at_ms: now,
        expires_at_ms: status_expires_at(now, refresh_interval_seconds),
        cache,
        last_error: None,
    }
}

fn admin_kiro_balance_from_usage(usage: &UsageLimitsResponse) -> AdminKiroBalanceView {
    let usage_limit = usage.usage_limit();
    let current_usage = usage.current_usage();
    AdminKiroBalanceView {
        current_usage,
        usage_limit,
        remaining: (usage_limit - current_usage).max(0.0),
        next_reset_at: usage
            .usage_breakdown_list
            .first()
            .and_then(|item| item.next_date_reset.or(usage.next_date_reset))
            .map(|value| value as i64),
        subscription_title: usage.subscription_title().map(ToString::to_string),
        user_id: usage.user_id().map(ToString::to_string),
    }
}

fn next_kiro_refresh_delay(config: &AdminRuntimeConfig) -> Duration {
    let min_seconds = config
        .kiro_status_refresh_min_interval_seconds
        .min(config.kiro_status_refresh_max_interval_seconds);
    let max_seconds = config
        .kiro_status_refresh_min_interval_seconds
        .max(config.kiro_status_refresh_max_interval_seconds);
    let seconds = if min_seconds == max_seconds {
        min_seconds
    } else {
        rand::thread_rng().gen_range(min_seconds..=max_seconds)
    };
    Duration::from_secs(seconds)
}

fn next_kiro_account_jitter(config: &AdminRuntimeConfig) -> Duration {
    if config.kiro_status_account_jitter_max_seconds == 0 {
        return Duration::ZERO;
    }
    Duration::from_secs(
        rand::thread_rng().gen_range(0..=config.kiro_status_account_jitter_max_seconds),
    )
}

fn status_expires_at(now: i64, refresh_interval_seconds: u64) -> i64 {
    now.saturating_add(refresh_interval_seconds.min(i64::MAX as u64 / 1000) as i64 * 1000)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use llm_access_core::{
        provider::RouteStrategy,
        store::{AdminKiroBalanceView, AdminKiroCacheView, ProviderKiroRoute},
    };
    use llm_access_kiro::wire::{
        Bonus, FreeTrialInfo, SubscriptionInfo, UsageBreakdown, UsageLimitsResponse, UserInfo,
    };

    fn route_with_prior_status() -> ProviderKiroRoute {
        ProviderKiroRoute {
            account_name: "kiro-a".to_string(),
            account_group_id_at_event: None,
            route_strategy_at_event: RouteStrategy::Auto,
            auth_json: r#"{"accessToken":"token"}"#.to_string(),
            profile_arn: None,
            api_region: "us-east-1".to_string(),
            request_validation_enabled: true,
            cache_estimation_enabled: true,
            zero_cache_debug_enabled: false,
            model_name_map_json: "{}".to_string(),
            cache_kmodels_json: llm_access_core::store::default_kiro_cache_kmodels_json(),
            cache_policy_json: llm_access_core::store::default_kiro_cache_policy_json(),
            prefix_cache_mode: "formula".to_string(),
            prefix_cache_max_tokens: 100_000,
            prefix_cache_entry_ttl_seconds: 3600,
            conversation_anchor_max_entries: 1024,
            conversation_anchor_ttl_seconds: 3600,
            billable_model_multipliers_json:
                llm_access_core::store::default_kiro_billable_model_multipliers_json(),
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
            account_request_max_concurrency: None,
            account_request_min_start_interval_ms: None,
            proxy: None,
            routing_identity: "user-1".to_string(),
            cached_status: Some("ready".to_string()),
            cached_remaining_credits: Some(42.0),
            cached_balance: Some(AdminKiroBalanceView {
                current_usage: 58.0,
                usage_limit: 100.0,
                remaining: 42.0,
                next_reset_at: Some(999),
                subscription_title: Some("Pro".to_string()),
                user_id: Some("user-1".to_string()),
            }),
            cached_cache: Some(AdminKiroCacheView {
                status: "ready".to_string(),
                refresh_interval_seconds: 180,
                last_checked_at: Some(1000),
                last_success_at: Some(900),
                error_message: None,
            }),
            status_refresh_interval_seconds: 180,
            minimum_remaining_credits_before_block: 0.0,
        }
    }

    #[test]
    fn error_status_update_preserves_prior_balance_and_marks_degraded() {
        let route = route_with_prior_status();

        let update = super::build_error_status_update(&route, 2_000, "boom".to_string());

        assert_eq!(update.account_name, "kiro-a");
        assert_eq!(update.balance, route.cached_balance);
        assert_eq!(update.cache.status, "degraded");
        assert_eq!(update.cache.last_checked_at, Some(2_000));
        assert_eq!(update.cache.last_success_at, Some(900));
        assert_eq!(update.cache.error_message.as_deref(), Some("boom"));
        assert_eq!(update.expires_at_ms, 182_000);
    }

    #[test]
    fn ready_status_update_converts_usage_limits_and_sets_interval() {
        let usage = UsageLimitsResponse {
            next_date_reset: Some(900.0),
            subscription_info: Some(SubscriptionInfo {
                subscription_title: Some("Pro".to_string()),
            }),
            usage_breakdown_list: vec![UsageBreakdown {
                current_usage_with_precision: 10.0,
                bonuses: vec![Bonus {
                    current_usage: 2.0,
                    usage_limit: 20.0,
                    status: Some("ACTIVE".to_string()),
                }],
                free_trial_info: Some(FreeTrialInfo {
                    current_usage_with_precision: 3.0,
                    free_trial_status: Some("ACTIVE".to_string()),
                    usage_limit_with_precision: 30.0,
                }),
                next_date_reset: Some(800.0),
                usage_limit_with_precision: 100.0,
            }],
            user_info: Some(UserInfo {
                user_id: Some("user-1".to_string()),
            }),
        };
        let route = route_with_prior_status();

        let update = super::build_ready_status_update(&route, 2_000, &usage);

        assert_eq!(update.cache.status, "ready");
        assert_eq!(update.cache.refresh_interval_seconds, 180);
        assert_eq!(update.cache.last_checked_at, Some(2_000));
        assert_eq!(update.cache.last_success_at, Some(2_000));
        assert_eq!(update.expires_at_ms, 182_000);
        let balance = update.balance.expect("balance");
        assert_eq!(balance.current_usage, 15.0);
        assert_eq!(balance.usage_limit, 150.0);
        assert_eq!(balance.remaining, 135.0);
        assert_eq!(balance.next_reset_at, Some(800));
        assert_eq!(balance.subscription_title.as_deref(), Some("Pro"));
        assert_eq!(balance.user_id.as_deref(), Some("user-1"));
    }
}
