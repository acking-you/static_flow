//! Route selection with account permits + the `DefaultProviderDispatcher`.

use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
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
    preferred_account_name: Option<&str>,
    session_counts: Option<&HashMap<String, usize>>,
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
        if let Some(preferred_route) = preferred_account_name.and_then(|account_name| {
            routes.iter().find(|route| {
                route.account_name == account_name && !failed_accounts.contains(&route.account_name)
            })
        }) {
            let has_terminal_auth_error = preferred_route
                .cached_error_message
                .as_deref()
                .is_some_and(is_terminal_codex_auth_error);
            let has_cooldown = codex_account_cooldowns
                .cooldown_for_account(&preferred_route.account_name)
                .is_some();
            if !has_terminal_auth_error && !has_cooldown {
                if let Ok(permit) = limiter.try_acquire(
                    format!(
                        "account:{}:{}",
                        ProviderType::Codex.as_storage_str(),
                        preferred_route.account_name
                    ),
                    preferred_route.account_request_max_concurrency,
                    preferred_route.account_request_min_start_interval_ms,
                ) {
                    return Ok((preferred_route.clone(), permit));
                }
            }
        }
        let mut ordered_routes = routes.iter().enumerate().collect::<Vec<_>>();
        if let Some(session_counts) = session_counts {
            ordered_routes.sort_by(|(left_index, left), (right_index, right)| {
                session_counts
                    .get(&left.account_name)
                    .copied()
                    .unwrap_or_default()
                    .cmp(
                        &session_counts
                            .get(&right.account_name)
                            .copied()
                            .unwrap_or_default(),
                    )
                    .then_with(|| left_index.cmp(right_index))
            });
        }
        for (_, route) in ordered_routes {
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
    session_counts: Option<&HashMap<String, usize>>,
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
        for route in selection_ordered_kiro_routes(
            routes,
            scheduler,
            latency_ranker,
            now_millis(),
            session_counts,
        ) {
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
/// Order candidate routes for one authenticated Kiro key.
///
/// All routes in `routes` must come from the same key route config, so their
/// preferred pool strategy is expected to be identical.
///
/// Ordering contract, highest precedence first:
/// 1. proxy health: candidates whose proxy is in cooldown sort last (global, so
///    a throttled preferred-pool route never shadows a healthy fallback);
/// 2. fewest bound affinity sessions (global; only active when `session_counts`
///    is supplied for new-session balancing, inert otherwise);
/// 3. pool rank: the key's preferred pool first, then the remaining pools in
///    `KIRO_POOL_STRATEGIES` order;
/// 4. pool-specific tiebreakers — `balanced`: latency band, last-started,
///    remaining credits desc; `credit_first`: remaining credits desc, latency
///    band, last-started;
/// 5. account name, as the deterministic final tiebreaker.
pub fn selection_ordered_kiro_routes<'a>(
    routes: &'a [ProviderKiroRoute],
    scheduler: &KiroRequestScheduler,
    latency_ranker: &KiroLatencyRanker,
    now_ms: i64,
    session_counts: Option<&HashMap<String, usize>>,
) -> Vec<&'a ProviderKiroRoute> {
    if routes.len() <= 1 {
        return routes.iter().collect();
    }

    #[derive(Clone, Copy)]
    struct Candidate<'a> {
        route: &'a ProviderKiroRoute,
        pool_strategy: &'static str,
        proxy_in_cooldown: bool,
        session_count: Option<usize>,
        last_started_at: Option<Instant>,
        latency_band: Option<i64>,
        remaining_sort: f64,
    }

    #[derive(Clone, Copy)]
    enum CandidateSortKey {
        RemainingDesc,
        LatencyBand,
        LastStarted,
    }

    fn route_pool_strategy(route: &ProviderKiroRoute) -> &'static str {
        llm_access_core::store::normalize_kiro_pool_strategy(&route.pool_strategy)
            .unwrap_or(llm_access_core::store::KIRO_POOL_STRATEGY_BALANCED)
    }

    fn route_preferred_pool_strategy(route: &ProviderKiroRoute) -> &'static str {
        llm_access_core::store::normalize_kiro_pool_strategy(&route.preferred_pool_strategy)
            .unwrap_or(llm_access_core::store::KIRO_POOL_STRATEGY_BALANCED)
    }

    fn compare_proxy_cooldown(left: bool, right: bool) -> Option<std::cmp::Ordering> {
        match (left, right) {
            (false, true) => Some(std::cmp::Ordering::Less),
            (true, false) => Some(std::cmp::Ordering::Greater),
            _ => None,
        }
    }

    fn compare_session_count(
        left: Option<usize>,
        right: Option<usize>,
    ) -> Option<std::cmp::Ordering> {
        match (left, right) {
            (Some(left_count), Some(right_count)) => {
                let ordering = left_count.cmp(&right_count);
                (ordering != std::cmp::Ordering::Equal).then_some(ordering)
            },
            _ => None,
        }
    }

    fn compare_latency_band(left: Option<i64>, right: Option<i64>) -> Option<std::cmp::Ordering> {
        match (left, right) {
            (Some(left_band), Some(right_band)) => {
                let ordering = left_band.cmp(&right_band);
                (ordering != std::cmp::Ordering::Equal).then_some(ordering)
            },
            (Some(_), None) => Some(std::cmp::Ordering::Less),
            (None, Some(_)) => Some(std::cmp::Ordering::Greater),
            (None, None) => None,
        }
    }

    fn compare_last_started(
        left: Option<Instant>,
        right: Option<Instant>,
    ) -> Option<std::cmp::Ordering> {
        match (left, right) {
            (None, Some(_)) => Some(std::cmp::Ordering::Less),
            (Some(_), None) => Some(std::cmp::Ordering::Greater),
            (Some(left_started), Some(right_started)) => {
                let ordering = left_started.cmp(&right_started);
                (ordering != std::cmp::Ordering::Equal).then_some(ordering)
            },
            (None, None) => None,
        }
    }

    fn compare_remaining_desc(left: f64, right: f64) -> Option<std::cmp::Ordering> {
        let ordering = right.total_cmp(&left);
        (ordering != std::cmp::Ordering::Equal).then_some(ordering)
    }

    fn pool_rank(pool_strategy: &str, preferred_pool_strategy: &str) -> usize {
        if pool_strategy == preferred_pool_strategy {
            return 0;
        }
        llm_access_core::store::KIRO_POOL_STRATEGIES
            .iter()
            .position(|strategy| *strategy == pool_strategy)
            .map(|index| index + 1)
            .unwrap_or(llm_access_core::store::KIRO_POOL_STRATEGIES.len() + 1)
    }

    fn compare_pool_rank(
        left: &Candidate<'_>,
        right: &Candidate<'_>,
        preferred_pool_strategy: &str,
    ) -> Option<Ordering> {
        let ordering = pool_rank(left.pool_strategy, preferred_pool_strategy)
            .cmp(&pool_rank(right.pool_strategy, preferred_pool_strategy));
        (ordering != Ordering::Equal).then_some(ordering)
    }

    fn pool_sort_keys(pool_strategy: &str) -> &'static [CandidateSortKey] {
        const BALANCED_KEYS: [CandidateSortKey; 3] = [
            CandidateSortKey::LatencyBand,
            CandidateSortKey::LastStarted,
            CandidateSortKey::RemainingDesc,
        ];
        const CREDIT_FIRST_KEYS: [CandidateSortKey; 3] = [
            CandidateSortKey::RemainingDesc,
            CandidateSortKey::LatencyBand,
            CandidateSortKey::LastStarted,
        ];
        match pool_strategy {
            llm_access_core::store::KIRO_POOL_STRATEGY_CREDIT_FIRST => &CREDIT_FIRST_KEYS,
            _ => &BALANCED_KEYS,
        }
    }

    fn compare_by_key(
        left: &Candidate<'_>,
        right: &Candidate<'_>,
        key: CandidateSortKey,
    ) -> Option<Ordering> {
        match key {
            CandidateSortKey::RemainingDesc => {
                compare_remaining_desc(left.remaining_sort, right.remaining_sort)
            },
            CandidateSortKey::LatencyBand => {
                compare_latency_band(left.latency_band, right.latency_band)
            },
            CandidateSortKey::LastStarted => {
                compare_last_started(left.last_started_at, right.last_started_at)
            },
        }
    }

    fn compare_pool_tiebreakers(left: &Candidate<'_>, right: &Candidate<'_>) -> Option<Ordering> {
        for key in pool_sort_keys(left.pool_strategy) {
            if let Some(ordering) = compare_by_key(left, right, *key) {
                return Some(ordering);
            }
        }
        None
    }

    fn compare_candidate_order(
        left: &Candidate<'_>,
        right: &Candidate<'_>,
        preferred_pool_strategy: &str,
    ) -> Ordering {
        // New-session spread stays global: pool preference only applies after
        // proxy cooldown and active session count have been considered.
        compare_proxy_cooldown(left.proxy_in_cooldown, right.proxy_in_cooldown)
            .or_else(|| compare_session_count(left.session_count, right.session_count))
            .or_else(|| compare_pool_rank(left, right, preferred_pool_strategy))
            .or_else(|| compare_pool_tiebreakers(left, right))
            .unwrap_or_else(|| left.route.account_name.cmp(&right.route.account_name))
    }

    let last_started_snapshot = scheduler.last_started_snapshot();
    let proxy_cooldowns = scheduler.proxy_cooldown_snapshot();
    // Aggregate the affinity session counts (keyed by account name) onto routing
    // identities, so aliases of the same upstream Kiro account share one load
    // figure — matching how the scheduler, cooldowns, last-started, and quota
    // failover all group by `routing_identity`. Without this, sessions bound to
    // one alias would not deter new sessions from another alias of the same
    // upstream account. O(routes), keys borrow from `routes` (zero-alloc).
    let identity_session_counts: Option<HashMap<&str, usize>> = session_counts.map(|counts| {
        routes
            .iter()
            .fold(HashMap::new(), |mut by_identity, route| {
                let count = counts.get(&route.account_name).copied().unwrap_or(0);
                *by_identity
                    .entry(route.routing_identity.as_str())
                    .or_insert(0) += count;
                by_identity
            })
    });
    // Best known balance inside the credit_first pool. Routes without a
    // balance snapshot in that pool substitute this value so a fresh or
    // just-refreshed account ties with the best candidate instead of being
    // starved behind nearly-exhausted accounts with known balances.
    let credit_first_known_max = routes
        .iter()
        .filter(|route| {
            route_pool_strategy(route) == llm_access_core::store::KIRO_POOL_STRATEGY_CREDIT_FIRST
        })
        .filter_map(|route| {
            route
                .cached_remaining_credits
                .filter(|value| value.is_finite())
        })
        .fold(None, |max: Option<f64>, remaining| {
            Some(max.map_or(remaining, |current| current.max(remaining)))
        });
    let preferred_pool_strategy = route_preferred_pool_strategy(&routes[0]);
    debug_assert!(
        routes
            .iter()
            .all(|route| route_preferred_pool_strategy(route) == preferred_pool_strategy),
        "Kiro route selection expects one preferred_pool_strategy per key"
    );
    let mut sorted = routes
        .iter()
        .map(|route| {
            let proxy_key = proxy_cooldown_key_for_route(route);
            let pool_strategy = route_pool_strategy(route);
            // Unknown balances: credit_first ties with the pool's best known
            // balance (anti-starvation, see above); balanced keeps the legacy
            // -1.0 so routes without a snapshot stay last on this tiebreaker.
            let remaining_sort = route
                .cached_remaining_credits
                .filter(|value| value.is_finite())
                .unwrap_or_else(|| {
                    if pool_strategy == llm_access_core::store::KIRO_POOL_STRATEGY_CREDIT_FIRST {
                        credit_first_known_max.unwrap_or(0.0)
                    } else {
                        -1.0
                    }
                });
            Candidate {
                route,
                pool_strategy,
                proxy_in_cooldown: proxy_key
                    .as_deref()
                    .is_some_and(|key| proxy_cooldowns.contains_key(key)),
                session_count: identity_session_counts.as_ref().map(|counts| {
                    counts
                        .get(route.routing_identity.as_str())
                        .copied()
                        .unwrap_or(0)
                }),
                last_started_at: last_started_snapshot.get(&route.routing_identity).copied(),
                latency_band: latency_ranker.route_score_band(route, now_ms),
                remaining_sort,
            }
        })
        .collect::<Vec<_>>();
    sorted.sort_by(|left, right| compare_candidate_order(left, right, preferred_pool_strategy));
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
