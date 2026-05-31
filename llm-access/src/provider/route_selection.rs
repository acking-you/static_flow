//! Route selection with account permits + the `DefaultProviderDispatcher`.

use std::{
    collections::HashSet,
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
};
use llm_access_core::{
    provider::ProviderType,
    store::{
        is_terminal_codex_auth_error, AuthenticatedKey, ProviderCodexRoute, ProviderKiroRoute,
        ProviderRouteStore,
    },
};
use llm_access_kiro::scheduler::{KiroRequestLease, KiroRequestScheduler};

use super::{
    codex_dispatch::dispatch_codex_proxy, errors::proxy_cooldown_key_for_route,
    kiro_dispatch::dispatch_kiro_proxy, kiro_error::kiro_json_error, limiter::wait_for_limit,
    util::now_millis, CodexAccountCooldowns, DefaultProviderDispatcher, LimitPermit,
    LimitRejection, ProviderDispatchDeps, ProviderDispatcher, RequestLimiter,
};
use crate::kiro_latency::KiroLatencyRanker;

pub async fn select_codex_route_with_account_permit(
    limiter: &Arc<RequestLimiter>,
    codex_account_cooldowns: &Arc<CodexAccountCooldowns>,
    routes: &[ProviderCodexRoute],
    failed_accounts: &HashSet<String>,
) -> Result<(ProviderCodexRoute, LimitPermit), Response> {
    if routes.is_empty() {
        return Err(
            (StatusCode::SERVICE_UNAVAILABLE, "codex route is not configured").into_response()
        );
    }
    loop {
        let mut saw_limit = false;
        let mut saw_account_cooldown = false;
        let mut saw_terminal_auth_error = false;
        let mut shortest_wait: Option<LimitRejection> = None;
        for route in routes {
            if failed_accounts.contains(&route.account_name) {
                continue;
            }
            if let Some(error) = route
                .cached_error_message
                .as_deref()
                .filter(|message| is_terminal_codex_auth_error(message))
            {
                saw_terminal_auth_error = true;
                tracing::warn!(
                    account = %route.account_name,
                    error,
                    "skipping codex account with terminal auth error"
                );
                continue;
            }
            if let Some(cooldown) =
                codex_account_cooldowns.cooldown_for_account(&route.account_name)
            {
                saw_account_cooldown = true;
                tracing::debug!(
                    account = %route.account_name,
                    cooldown_remaining_ms = cooldown.remaining.as_millis() as u64,
                    "skipping codex account on temporary request-path cooldown"
                );
                continue;
            }
            match limiter.try_acquire(
                format!("account:{}:{}", ProviderType::Codex.as_storage_str(), route.account_name),
                route.account_request_max_concurrency,
                route.account_request_min_start_interval_ms,
            ) {
                Ok(permit) => return Ok((route.clone(), permit)),
                Err(rejection) => {
                    saw_limit = true;
                    if shortest_wait
                        .as_ref()
                        .and_then(|current| current.wait)
                        .map(|current| rejection.wait.unwrap_or(current) < current)
                        .unwrap_or(true)
                    {
                        shortest_wait = Some(rejection);
                    }
                },
            }
        }
        if !failed_accounts.is_empty()
            && routes
                .iter()
                .all(|route| failed_accounts.contains(&route.account_name))
        {
            return Err((
                StatusCode::BAD_GATEWAY,
                "all eligible codex accounts failed for this request",
            )
                .into_response());
        }
        if saw_limit {
            wait_for_limit(shortest_wait.as_ref()).await;
            continue;
        }
        if saw_account_cooldown {
            return Err((StatusCode::TOO_MANY_REQUESTS, "quota_exceeded").into_response());
        }
        if saw_terminal_auth_error {
            return Err((
                StatusCode::BAD_GATEWAY,
                "all eligible codex accounts failed for this request",
            )
                .into_response());
        }
        return Err((StatusCode::SERVICE_UNAVAILABLE, "no usable codex account is configured")
            .into_response());
    }
}
pub async fn select_kiro_route_with_account_permit(
    scheduler: &Arc<KiroRequestScheduler>,
    routes: &[ProviderKiroRoute],
    failed_accounts: &HashSet<String>,
    latency_ranker: &KiroLatencyRanker,
    preferred_account_name: Option<&str>,
) -> Result<(ProviderKiroRoute, KiroRequestLease), Response> {
    if routes.is_empty() {
        return Err(kiro_json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "api_error",
            "kiro route is not configured",
        ));
    }
    let queued_at = Instant::now();
    loop {
        let mut saw_limit = false;
        let mut shortest_wait: Option<Duration> = None;
        let proxy_cooldowns = scheduler.proxy_cooldown_snapshot();
        if let Some(preferred_route) = preferred_account_name
            .and_then(|account_name| {
                routes.iter().find(|route| {
                    route.account_name == account_name
                        && !failed_accounts.contains(&route.account_name)
                })
            })
            .filter(|route| {
                proxy_cooldown_key_for_route(route)
                    .is_none_or(|key| !proxy_cooldowns.contains_key(&key))
            })
        {
            if scheduler
                .cooldown_for_account(&preferred_route.routing_identity)
                .is_none()
            {
                if let Ok(permit) = scheduler.try_acquire(
                    &preferred_route.routing_identity,
                    preferred_route
                        .account_request_max_concurrency
                        .unwrap_or(llm_access_core::store::DEFAULT_KIRO_CHANNEL_MAX_CONCURRENCY),
                    preferred_route
                        .account_request_min_start_interval_ms
                        .unwrap_or(
                            llm_access_core::store::DEFAULT_KIRO_CHANNEL_MIN_START_INTERVAL_MS,
                        ),
                    queued_at,
                ) {
                    return Ok((preferred_route.clone(), permit));
                }
            }
        }
        for route in selection_ordered_kiro_routes(routes, scheduler, latency_ranker, now_millis())
        {
            if failed_accounts.contains(&route.account_name) {
                continue;
            }
            if let Some(cooldown) = scheduler.cooldown_for_account(&route.routing_identity) {
                saw_limit = true;
                shortest_wait = Some(match shortest_wait {
                    Some(current) => current.min(cooldown.remaining),
                    None => cooldown.remaining,
                });
                continue;
            }
            match scheduler.try_acquire(
                &route.routing_identity,
                route
                    .account_request_max_concurrency
                    .unwrap_or(llm_access_core::store::DEFAULT_KIRO_CHANNEL_MAX_CONCURRENCY),
                route
                    .account_request_min_start_interval_ms
                    .unwrap_or(llm_access_core::store::DEFAULT_KIRO_CHANNEL_MIN_START_INTERVAL_MS),
                queued_at,
            ) {
                Ok(permit) => return Ok((route.clone(), permit)),
                Err(rejection) => {
                    saw_limit = true;
                    if let Some(wait) = rejection.wait {
                        shortest_wait = Some(match shortest_wait {
                            Some(current) => current.min(wait),
                            None => wait,
                        });
                    }
                },
            }
        }
        if !failed_accounts.is_empty()
            && routes
                .iter()
                .all(|route| failed_accounts.contains(&route.account_name))
        {
            return Err(kiro_json_error(
                StatusCode::BAD_GATEWAY,
                "api_error",
                "all eligible kiro accounts failed for this request",
            ));
        }
        if saw_limit {
            scheduler.wait_for_available(shortest_wait).await;
            continue;
        }
        return Err(kiro_json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "api_error",
            "no usable kiro account is configured",
        ));
    }
}
pub async fn hydrate_codex_route_for_dispatch(
    route: ProviderCodexRoute,
    route_store: &dyn ProviderRouteStore,
) -> Result<ProviderCodexRoute, Response> {
    if !route.auth_json.is_empty() {
        return Ok(route);
    }
    let account_name = route.account_name.clone();
    let loaded = route_store
        .resolve_codex_account_route(&account_name)
        .await
        .map_err(|_| {
            (StatusCode::INTERNAL_SERVER_ERROR, "codex route resolution failed").into_response()
        })?;
    let Some(loaded) = loaded else {
        return Err((
            StatusCode::BAD_GATEWAY,
            "all eligible codex accounts failed for this request",
        )
            .into_response());
    };
    let mut route = route;
    route.auth_json = loaded.auth_json;
    route.map_gpt53_codex_to_spark = loaded.map_gpt53_codex_to_spark;
    route.auth_refresh_enabled = loaded.auth_refresh_enabled;
    route.account_request_max_concurrency = loaded.account_request_max_concurrency;
    route.account_request_min_start_interval_ms = loaded.account_request_min_start_interval_ms;
    route.cached_error_message = loaded.cached_error_message;
    route.proxy = loaded.proxy;
    Ok(route)
}
pub async fn hydrate_kiro_route_for_dispatch(
    route: ProviderKiroRoute,
    route_store: &dyn ProviderRouteStore,
) -> Result<ProviderKiroRoute, Response> {
    if !route.auth_json.is_empty() {
        return Ok(route);
    }
    let account_name = route.account_name.clone();
    let loaded = route_store
        .resolve_kiro_account_route(&account_name)
        .await
        .map_err(|_| {
            kiro_json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "api_error",
                "kiro route resolution failed",
            )
        })?;
    let Some(loaded) = loaded else {
        return Err(kiro_json_error(
            StatusCode::BAD_GATEWAY,
            "api_error",
            "all eligible kiro accounts failed for this request",
        ));
    };
    let mut route = route;
    route.auth_json = loaded.auth_json;
    if route.profile_arn.is_none() {
        route.profile_arn = loaded.profile_arn;
    }
    if route.api_region.trim().is_empty() {
        route.api_region = loaded.api_region;
    }
    route.account_request_max_concurrency = loaded.account_request_max_concurrency;
    route.account_request_min_start_interval_ms = loaded.account_request_min_start_interval_ms;
    route.proxy = loaded.proxy;
    Ok(route)
}
pub fn selection_ordered_kiro_routes<'a>(
    routes: &'a [ProviderKiroRoute],
    scheduler: &KiroRequestScheduler,
    latency_ranker: &KiroLatencyRanker,
    now_ms: i64,
) -> Vec<&'a ProviderKiroRoute> {
    #[derive(Clone, Copy)]
    struct Candidate<'a> {
        route: &'a ProviderKiroRoute,
        proxy_in_cooldown: bool,
        last_started_at: Option<Instant>,
        latency_score_ms: Option<f64>,
        remaining: f64,
    }

    let last_started_snapshot = scheduler.last_started_snapshot();
    let proxy_cooldowns = scheduler.proxy_cooldown_snapshot();
    let mut sorted = routes
        .iter()
        .map(|route| {
            let proxy_key = proxy_cooldown_key_for_route(route);
            Candidate {
                route,
                proxy_in_cooldown: proxy_key
                    .as_deref()
                    .is_some_and(|key| proxy_cooldowns.contains_key(key)),
                last_started_at: last_started_snapshot.get(&route.routing_identity).copied(),
                latency_score_ms: latency_ranker.route_score_ms(route, now_ms),
                remaining: route.cached_remaining_credits.unwrap_or(-1.0),
            }
        })
        .collect::<Vec<_>>();
    sorted.sort_by(|left, right| {
        match (left.proxy_in_cooldown, right.proxy_in_cooldown) {
            (false, true) => return std::cmp::Ordering::Less,
            (true, false) => return std::cmp::Ordering::Greater,
            _ => {},
        }
        match (left.latency_score_ms, right.latency_score_ms) {
            (Some(left_score), Some(right_score)) => {
                let ordering = left_score.total_cmp(&right_score);
                if ordering != std::cmp::Ordering::Equal {
                    return ordering;
                }
            },
            (Some(_), None) => return std::cmp::Ordering::Less,
            (None, Some(_)) => return std::cmp::Ordering::Greater,
            (None, None) => {},
        }
        match (left.last_started_at, right.last_started_at) {
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(left_started), Some(right_started)) => {
                let ordering = left_started.cmp(&right_started);
                if ordering != std::cmp::Ordering::Equal {
                    return ordering;
                }
            },
            (None, None) => {},
        }
        right
            .remaining
            .total_cmp(&left.remaining)
            .then_with(|| left.route.account_name.cmp(&right.route.account_name))
    });
    sorted
        .into_iter()
        .map(|candidate| candidate.route)
        .collect()
}
#[async_trait]
impl ProviderDispatcher for DefaultProviderDispatcher {
    async fn dispatch(
        &self,
        key: AuthenticatedKey,
        request: Request<Body>,
        deps: ProviderDispatchDeps,
    ) -> Response {
        if ProviderType::from_storage_str(&key.provider_type) == Some(ProviderType::Codex) {
            return dispatch_codex_proxy(key, request, deps).await;
        }
        if ProviderType::from_storage_str(&key.provider_type) == Some(ProviderType::Kiro) {
            return dispatch_kiro_proxy(key, request, deps).await;
        }
        (StatusCode::NOT_IMPLEMENTED, "provider dispatch is not wired").into_response()
    }
}
