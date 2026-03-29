//! Multi-account Kiro upstream provider with retry, cooldown, and
//! quota-exhaustion failover.
//!
//! [`KiroProvider`] iterates over configured accounts in balance-descending
//! order (highest remaining quota first), skipping those that are disabled,
//! cooling down, or quota-exhausted. Each per-account attempt retries up to
//! 3 times (with forced token refresh on 401/403), and the outer loop sleeps
//! through cooldown windows before giving up.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context, Result};
use reqwest::header::{HeaderMap, HeaderValue};
use static_flow_shared::llm_gateway_store::LLM_GATEWAY_PROVIDER_KIRO;

use super::{
    auth_file::{
        KiroAuthRecord, DEFAULT_KIRO_VERSION, DEFAULT_NODE_VERSION, DEFAULT_SYSTEM_VERSION,
    },
    machine_id,
    runtime::{CallContext, KiroGatewayRuntimeState},
    scheduler::KiroRequestLease,
    status_cache::{
        account_is_request_eligible, mark_account_quota_exhausted, KiroStatusCacheSnapshot,
        STATUS_QUOTA_EXHAUSTED,
    },
    wire::{ConversationState, KiroRequest},
};
use crate::upstream_proxy::{ResolvedUpstreamProxy, UpstreamProxyRegistry};

/// Build a [`reqwest::Client`] configured for Kiro upstream calls together
/// with the resolved provider-level proxy metadata used for diagnostics.
pub async fn build_client(
    proxy_registry: &UpstreamProxyRegistry,
    timeout_secs: u64,
) -> Result<(reqwest::Client, ResolvedUpstreamProxy)> {
    let resolved = proxy_registry
        .resolve_provider_proxy(LLM_GATEWAY_PROVIDER_KIRO)
        .await
        .context("failed to resolve kiro upstream proxy")?;
    let builder = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(timeout_secs))
        .pool_max_idle_per_host(8)
        .pool_idle_timeout(Duration::from_secs(60))
        .tcp_keepalive(Duration::from_secs(30));
    let builder = proxy_registry
        .apply_provider_proxy(LLM_GATEWAY_PROVIDER_KIRO, builder)
        .await
        .context("failed to apply kiro upstream proxy")?;
    let client = builder.build().context("build kiro reqwest client")?;
    Ok((client, resolved))
}

/// Successful upstream response together with the account that served it
/// and the concurrency lease that must be held until the response body is
/// consumed.
pub struct ProviderCallResult {
    pub response: reqwest::Response,
    pub account_name: String,
    _channel_lease: KiroRequestLease,
}

/// Per-account attempt outcome that drives the retry/failover decision.
enum ProviderAttemptError {
    /// Transient failure; try the next account in the rotation.
    RetryNext(anyhow::Error),
    /// Unrecoverable error; abort immediately.
    Fatal(anyhow::Error),
    /// Monthly quota exhausted (HTTP 402); mark account and move on.
    QuotaExhausted(anyhow::Error),
    /// Upstream 5-minute credit window hit (HTTP 429); apply cooldown then move
    /// on.
    RateLimited { error: anyhow::Error, cooldown: Duration },
}

/// Multi-account upstream provider that routes requests through the
/// round-robin account pool with automatic failover.
pub struct KiroProvider {
    runtime: Arc<KiroGatewayRuntimeState>,
}

impl KiroProvider {
    pub fn new(runtime: Arc<KiroGatewayRuntimeState>) -> Self {
        Self {
            runtime,
        }
    }

    /// Send a `generateAssistantResponse` request, iterating accounts until
    /// one succeeds or all are exhausted.
    pub async fn call_api(
        &self,
        conversation_state: &ConversationState,
    ) -> Result<ProviderCallResult> {
        self.call_api_inner(conversation_state).await
    }

    /// Streaming variant of [`call_api`](Self::call_api). The response body
    /// should be consumed as an event stream.
    pub async fn call_api_stream(
        &self,
        conversation_state: &ConversationState,
    ) -> Result<ProviderCallResult> {
        self.call_api_inner(conversation_state).await
    }

    /// Send an MCP (Model Context Protocol) request through the account pool.
    pub async fn call_mcp(&self, request_body: &str) -> Result<ProviderCallResult> {
        self.call_mcp_inner(request_body).await
    }

    // Outer loop: retries the full account rotation when all accounts are
    // cooling down. Breaks out on success, fatal error, or full exhaustion.
    async fn call_api_inner(
        &self,
        conversation_state: &ConversationState,
    ) -> Result<ProviderCallResult> {
        let queued_at = Instant::now();
        loop {
            let auths = self.runtime.token_manager.list_auths().await?;
            if auths.is_empty() {
                return Err(anyhow!("no kiro account available for request"));
            }
            let snapshot = self.runtime.cached_status_snapshot().await;
            let mut last_error: Option<anyhow::Error> = None;
            let mut saw_quota_exhausted = false;
            let mut shortest_cooldown: Option<Duration> = None;
            let mut shortest_local_wait: Option<Duration> = None;
            let mut saw_local_limit = false;
            let mut blocked_accounts = Vec::new();

            // Iterate accounts in balance-descending order (highest remaining first).
            for auth in balance_ordered_accounts(&auths, &snapshot) {
                // Skip accounts still in an upstream cooldown window.
                if let Some(cooldown) = self
                    .runtime
                    .request_scheduler
                    .cooldown_for_account(&auth.name)
                {
                    shortest_cooldown = Some(match shortest_cooldown {
                        Some(current) => current.min(cooldown.remaining),
                        None => cooldown.remaining,
                    });
                    tracing::info!(
                        account_name = %auth.name,
                        cooldown_ms = cooldown.remaining.as_millis() as u64,
                        reason = %cooldown.reason,
                        "skipping kiro account before request because it is in upstream cooldown"
                    );
                    blocked_accounts.push(format!(
                        "{}: upstream_cooldown wait_ms={} reason={}",
                        auth.name,
                        cooldown.remaining.as_millis() as u64,
                        cooldown.reason
                    ));
                    continue;
                }

                // Skip accounts that the status cache marks as ineligible.
                let cache_entry = snapshot.accounts.get(&auth.name);
                if !account_is_request_eligible(&auth, cache_entry) {
                    let quota_exhausted = !auth.disabled
                        && cache_entry.is_some_and(|status| {
                            status.cache.status == STATUS_QUOTA_EXHAUSTED
                                || status
                                    .balance
                                    .as_ref()
                                    .is_some_and(|balance| balance.remaining <= 0.0)
                        });
                    if quota_exhausted {
                        saw_quota_exhausted = true;
                    } else {
                        last_error = Some(anyhow!("kiro account `{}` is disabled", auth.name));
                    }
                    tracing::info!(
                    account_name = %auth.name,
                    cache_status = cache_entry
                        .map(|status| status.cache.status.as_str())
                        .unwrap_or("unknown"),
                    reason = if quota_exhausted { "cached_quota_unavailable" } else { "disabled" },
                    "skipping kiro account before request"
                    );
                    blocked_accounts.push(format!(
                        "{}: {} cache_status={} remaining={}",
                        auth.name,
                        if quota_exhausted { "cached_quota_unavailable" } else { "disabled" },
                        cache_entry
                            .map(|status| status.cache.status.as_str())
                            .unwrap_or("unknown"),
                        cache_entry
                            .and_then(|status| status.balance.as_ref())
                            .map(|balance| format!("{:.4}", balance.remaining))
                            .unwrap_or_else(|| "unknown".to_string())
                    ));
                    continue;
                }

                let local_max_concurrency = auth.effective_kiro_channel_max_concurrency();
                let local_min_start_interval_ms =
                    auth.effective_kiro_channel_min_start_interval_ms();
                let channel_lease = match self.runtime.request_scheduler.try_acquire(
                    &auth.name,
                    local_max_concurrency,
                    local_min_start_interval_ms,
                    queued_at,
                ) {
                    Ok(lease) => lease,
                    Err(throttle) => {
                        saw_local_limit = true;
                        if let Some(wait) = throttle.wait {
                            shortest_local_wait = Some(match shortest_local_wait {
                                Some(current) => current.min(wait),
                                None => wait,
                            });
                        }
                        tracing::info!(
                            account_name = %auth.name,
                            request_kind = "messages",
                            reason = throttle.reason,
                            wait_ms = throttle.wait.map(|value| value.as_millis() as u64).unwrap_or(0),
                            in_flight = throttle.in_flight,
                            max_concurrency = throttle.max_concurrency,
                            min_start_interval_ms = throttle.min_start_interval_ms,
                            "skipping kiro account before request because it is locally throttled"
                        );
                        blocked_accounts.push(format!(
                            "{}: {} in_flight={} max_concurrency={} min_start_interval_ms={} \
                             wait_ms={}",
                            auth.name,
                            throttle.reason,
                            throttle.in_flight,
                            throttle.max_concurrency,
                            throttle.min_start_interval_ms,
                            throttle
                                .wait
                                .map(|value| value.as_millis() as u64)
                                .unwrap_or(0)
                        ));
                        continue;
                    },
                };
                tracing::info!(
                    account_name = %auth.name,
                    request_kind = "messages",
                    queue_wait_ms = channel_lease.waited_ms(),
                    max_concurrency = local_max_concurrency,
                    min_start_interval_ms = local_min_start_interval_ms,
                    "selected kiro account for upstream request"
                );
                match self
                    .call_api_for_account(&auth.name, conversation_state, channel_lease)
                    .await
                {
                    Ok(result) => return Ok(result),
                    Err(ProviderAttemptError::QuotaExhausted(err)) => {
                        saw_quota_exhausted = true;
                        tracing::warn!(
                            account_name = %auth.name,
                            error = %err,
                            "kiro account quota exhausted during request; moving to next account"
                        );
                        mark_account_quota_exhausted(&self.runtime, &auth.name, err.to_string())
                            .await;
                        last_error = Some(err);
                    },
                    Err(ProviderAttemptError::RateLimited {
                        error,
                        cooldown,
                    }) => {
                        shortest_cooldown = Some(match shortest_cooldown {
                            Some(current) => current.min(cooldown),
                            None => cooldown,
                        });
                        self.runtime.request_scheduler.mark_account_cooldown(
                            &auth.name,
                            cooldown,
                            error.to_string(),
                        );
                        tracing::warn!(
                            account_name = %auth.name,
                            error = %error,
                            cooldown_ms = cooldown.as_millis() as u64,
                            "kiro account hit upstream 5-minute credit window; moving to next account"
                        );
                        last_error = Some(error);
                    },
                    Err(ProviderAttemptError::RetryNext(err)) => {
                        tracing::warn!(
                            account_name = %auth.name,
                            error = %err,
                            "kiro account request failed; trying next account in sequence"
                        );
                        last_error = Some(err);
                    },
                    Err(ProviderAttemptError::Fatal(err)) => return Err(err),
                }
            }

            let upstream_wait = self
                .runtime
                .request_scheduler
                .shortest_cooldown()
                .or(shortest_cooldown);
            let combined_wait = match (upstream_wait, shortest_local_wait) {
                (Some(left), Some(right)) => Some(left.min(right)),
                (Some(left), None) => Some(left),
                (None, Some(right)) => Some(right),
                (None, None) => None,
            };
            if saw_local_limit {
                tracing::warn!(
                    request_kind = "messages",
                    wait_ms = combined_wait
                        .map(|value| value.as_millis() as u64)
                        .unwrap_or(0),
                    blocked_accounts = ?blocked_accounts,
                    "all currently eligible kiro accounts are locally throttled or cooling down; \
                     waiting before retrying request"
                );
                self.runtime
                    .request_scheduler
                    .wait_for_available(combined_wait)
                    .await;
                continue;
            }
            if let Some(wait) = upstream_wait {
                tracing::warn!(
                    request_kind = "messages",
                    wait_ms = wait.as_millis() as u64,
                    blocked_accounts = ?blocked_accounts,
                    "all currently eligible kiro accounts are cooling down; waiting before \
                     retrying request"
                );
                tokio::time::sleep(wait).await;
                continue;
            }

            if saw_quota_exhausted {
                return Err(anyhow!(
                    "all configured kiro accounts are quota exhausted; blocked_accounts={}",
                    blocked_accounts.join(" | ")
                ));
            }
            let base_error =
                last_error.unwrap_or_else(|| anyhow!("no kiro account available for request"));
            return Err(anyhow!("{base_error}; blocked_accounts={}", blocked_accounts.join(" | ")));
        }
    }

    async fn call_mcp_inner(&self, request_body: &str) -> Result<ProviderCallResult> {
        let queued_at = Instant::now();
        loop {
            let auths = self.runtime.token_manager.list_auths().await?;
            if auths.is_empty() {
                return Err(anyhow!("no kiro account available for mcp request"));
            }
            let snapshot = self.runtime.cached_status_snapshot().await;
            let mut last_error: Option<anyhow::Error> = None;
            let mut saw_quota_exhausted = false;
            let mut shortest_cooldown: Option<Duration> = None;
            let mut shortest_local_wait: Option<Duration> = None;
            let mut saw_local_limit = false;
            let mut blocked_accounts = Vec::new();

            for auth in balance_ordered_accounts(&auths, &snapshot) {
                if let Some(cooldown) = self
                    .runtime
                    .request_scheduler
                    .cooldown_for_account(&auth.name)
                {
                    shortest_cooldown = Some(match shortest_cooldown {
                        Some(current) => current.min(cooldown.remaining),
                        None => cooldown.remaining,
                    });
                    tracing::info!(
                        account_name = %auth.name,
                        cooldown_ms = cooldown.remaining.as_millis() as u64,
                        reason = %cooldown.reason,
                        "skipping kiro account before mcp request because it is in upstream cooldown"
                    );
                    blocked_accounts.push(format!(
                        "{}: upstream_cooldown wait_ms={} reason={}",
                        auth.name,
                        cooldown.remaining.as_millis() as u64,
                        cooldown.reason
                    ));
                    continue;
                }

                let cache_entry = snapshot.accounts.get(&auth.name);
                if !account_is_request_eligible(&auth, cache_entry) {
                    let quota_exhausted = !auth.disabled
                        && cache_entry.is_some_and(|status| {
                            status.cache.status == STATUS_QUOTA_EXHAUSTED
                                || status
                                    .balance
                                    .as_ref()
                                    .is_some_and(|balance| balance.remaining <= 0.0)
                        });
                    if quota_exhausted {
                        saw_quota_exhausted = true;
                    } else {
                        last_error = Some(anyhow!("kiro account `{}` is disabled", auth.name));
                    }
                    tracing::info!(
                    account_name = %auth.name,
                    cache_status = cache_entry
                        .map(|status| status.cache.status.as_str())
                        .unwrap_or("unknown"),
                    reason = if quota_exhausted { "cached_quota_unavailable" } else { "disabled" },
                    "skipping kiro account before mcp request"
                    );
                    blocked_accounts.push(format!(
                        "{}: {} cache_status={} remaining={}",
                        auth.name,
                        if quota_exhausted { "cached_quota_unavailable" } else { "disabled" },
                        cache_entry
                            .map(|status| status.cache.status.as_str())
                            .unwrap_or("unknown"),
                        cache_entry
                            .and_then(|status| status.balance.as_ref())
                            .map(|balance| format!("{:.4}", balance.remaining))
                            .unwrap_or_else(|| "unknown".to_string())
                    ));
                    continue;
                }

                let local_max_concurrency = auth.effective_kiro_channel_max_concurrency();
                let local_min_start_interval_ms =
                    auth.effective_kiro_channel_min_start_interval_ms();
                let channel_lease = match self.runtime.request_scheduler.try_acquire(
                    &auth.name,
                    local_max_concurrency,
                    local_min_start_interval_ms,
                    queued_at,
                ) {
                    Ok(lease) => lease,
                    Err(throttle) => {
                        saw_local_limit = true;
                        if let Some(wait) = throttle.wait {
                            shortest_local_wait = Some(match shortest_local_wait {
                                Some(current) => current.min(wait),
                                None => wait,
                            });
                        }
                        tracing::info!(
                            account_name = %auth.name,
                            request_kind = "mcp",
                            reason = throttle.reason,
                            wait_ms = throttle.wait.map(|value| value.as_millis() as u64).unwrap_or(0),
                            in_flight = throttle.in_flight,
                            max_concurrency = throttle.max_concurrency,
                            min_start_interval_ms = throttle.min_start_interval_ms,
                            "skipping kiro account before mcp request because it is locally throttled"
                        );
                        blocked_accounts.push(format!(
                            "{}: {} in_flight={} max_concurrency={} min_start_interval_ms={} \
                             wait_ms={}",
                            auth.name,
                            throttle.reason,
                            throttle.in_flight,
                            throttle.max_concurrency,
                            throttle.min_start_interval_ms,
                            throttle
                                .wait
                                .map(|value| value.as_millis() as u64)
                                .unwrap_or(0)
                        ));
                        continue;
                    },
                };
                tracing::info!(
                    account_name = %auth.name,
                    request_kind = "mcp",
                    queue_wait_ms = channel_lease.waited_ms(),
                    max_concurrency = local_max_concurrency,
                    min_start_interval_ms = local_min_start_interval_ms,
                    "selected kiro account for upstream mcp request"
                );
                match self
                    .call_mcp_for_account(&auth.name, request_body, channel_lease)
                    .await
                {
                    Ok(result) => return Ok(result),
                    Err(ProviderAttemptError::QuotaExhausted(err)) => {
                        saw_quota_exhausted = true;
                        tracing::warn!(
                            account_name = %auth.name,
                            error = %err,
                            "kiro account quota exhausted during mcp request; moving to next account"
                        );
                        mark_account_quota_exhausted(&self.runtime, &auth.name, err.to_string())
                            .await;
                        last_error = Some(err);
                    },
                    Err(ProviderAttemptError::RateLimited {
                        error,
                        cooldown,
                    }) => {
                        shortest_cooldown = Some(match shortest_cooldown {
                            Some(current) => current.min(cooldown),
                            None => cooldown,
                        });
                        self.runtime.request_scheduler.mark_account_cooldown(
                            &auth.name,
                            cooldown,
                            error.to_string(),
                        );
                        tracing::warn!(
                            account_name = %auth.name,
                            error = %error,
                            cooldown_ms = cooldown.as_millis() as u64,
                            "kiro account hit upstream 5-minute credit window for mcp; moving to next account"
                        );
                        last_error = Some(error);
                    },
                    Err(ProviderAttemptError::RetryNext(err)) => {
                        tracing::warn!(
                            account_name = %auth.name,
                            error = %err,
                            "kiro mcp request failed for current account; trying next account in sequence"
                        );
                        last_error = Some(err);
                    },
                    Err(ProviderAttemptError::Fatal(err)) => return Err(err),
                }
            }

            let upstream_wait = self
                .runtime
                .request_scheduler
                .shortest_cooldown()
                .or(shortest_cooldown);
            let combined_wait = match (upstream_wait, shortest_local_wait) {
                (Some(left), Some(right)) => Some(left.min(right)),
                (Some(left), None) => Some(left),
                (None, Some(right)) => Some(right),
                (None, None) => None,
            };
            if saw_local_limit {
                tracing::warn!(
                    request_kind = "mcp",
                    wait_ms = combined_wait
                        .map(|value| value.as_millis() as u64)
                        .unwrap_or(0),
                    blocked_accounts = ?blocked_accounts,
                    "all currently eligible kiro accounts are locally throttled or cooling down; \
                     waiting before retrying mcp request"
                );
                self.runtime
                    .request_scheduler
                    .wait_for_available(combined_wait)
                    .await;
                continue;
            }
            if let Some(wait) = upstream_wait {
                tracing::warn!(
                    request_kind = "mcp",
                    wait_ms = wait.as_millis() as u64,
                    blocked_accounts = ?blocked_accounts,
                    "all currently eligible kiro accounts are cooling down; waiting before \
                     retrying mcp request"
                );
                tokio::time::sleep(wait).await;
                continue;
            }

            if saw_quota_exhausted {
                return Err(anyhow!(
                    "all configured kiro accounts are quota exhausted for mcp request; \
                     blocked_accounts={}",
                    blocked_accounts.join(" | ")
                ));
            }
            let base_error =
                last_error.unwrap_or_else(|| anyhow!("no kiro account available for mcp request"));
            return Err(anyhow!("{base_error}; blocked_accounts={}", blocked_accounts.join(" | ")));
        }
    }

    // Per-account retry loop (up to 3 attempts). Forces a token refresh on
    // 401/403, backs off on transient 408/429/5xx, and classifies 402 as
    // quota-exhausted for the outer failover loop.
    async fn call_api_for_account(
        &self,
        account_name: &str,
        conversation_state: &ConversationState,
        channel_lease: KiroRequestLease,
    ) -> Result<ProviderCallResult, ProviderAttemptError> {
        let mut force_refresh = false;
        let mut last_error: Option<anyhow::Error> = None;
        for attempt in 0..3 {
            let ctx = self
                .runtime
                .token_manager
                .ensure_context_for_account(account_name, force_refresh)
                .await
                .map_err(ProviderAttemptError::RetryNext)?;
            let request_body = serde_json::to_string(&KiroRequest {
                conversation_state: conversation_state.clone(),
                profile_arn: ctx.auth.profile_arn.clone(),
            })
            .map_err(|err| ProviderAttemptError::Fatal(anyhow!("serialize kiro request: {err}")))?;
            let (client, resolved_proxy) =
                build_client(self.runtime.upstream_proxy_registry.as_ref(), 720)
                    .await
                    .map_err(ProviderAttemptError::Fatal)?;
            let url = format!(
                "https://q.{}.amazonaws.com/generateAssistantResponse",
                ctx.auth.effective_api_region()
            );
            tracing::info!(
                account_name = %ctx.auth.name,
                attempt = attempt + 1,
                force_refresh,
                api_region = ctx.auth.effective_api_region(),
                proxy_url = %resolved_proxy.proxy_url,
                proxy_source = ?resolved_proxy.source,
                proxy_config_id = ?resolved_proxy.proxy_config_id,
                request_body_len = request_body.len(),
                has_profile_arn = ctx.auth.profile_arn.is_some(),
                queue_wait_ms = channel_lease.waited_ms(),
                "calling kiro upstream generateAssistantResponse"
            );
            let response = client
                .post(url)
                .headers(build_headers(&ctx).map_err(ProviderAttemptError::Fatal)?)
                .body(request_body)
                .send()
                .await;
            let response = match response {
                Ok(response) => response,
                Err(err) => {
                    last_error = Some(err.into());
                    continue;
                },
            };
            if response.status().is_success() {
                tracing::info!(
                    account_name = %ctx.auth.name,
                    attempt = attempt + 1,
                    status = %response.status(),
                    queue_wait_ms = channel_lease.waited_ms(),
                    "kiro upstream request succeeded"
                );
                return Ok(ProviderCallResult {
                    response,
                    account_name: ctx.auth.name,
                    _channel_lease: channel_lease,
                });
            }
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            tracing::warn!(
                account_name = %ctx.auth.name,
                attempt = attempt + 1,
                status = %status,
                body_len = body.len(),
                force_refresh,
                queue_wait_ms = channel_lease.waited_ms(),
                "kiro upstream request returned non-success status"
            );
            if status.as_u16() == 400 {
                return Err(ProviderAttemptError::Fatal(anyhow!(
                    "kiro upstream rejected request: {status} {body}"
                )));
            }
            if status.as_u16() == 402 && is_monthly_request_limit(&body) {
                return Err(ProviderAttemptError::QuotaExhausted(anyhow!(
                    "kiro account quota exhausted: {status} {body}"
                )));
            }
            if status.as_u16() == 429 {
                if let Some(cooldown) = daily_request_limit_cooldown(&body) {
                    return Err(ProviderAttemptError::RateLimited {
                        error: anyhow!("kiro upstream rate limit reached: {status} {body}"),
                        cooldown,
                    });
                }
            }
            if matches!(status.as_u16(), 401 | 403) && !force_refresh {
                tracing::info!(
                    account_name = %ctx.auth.name,
                    attempt = attempt + 1,
                    status = %status,
                    "kiro upstream auth failed; forcing token refresh before retry"
                );
                force_refresh = true;
                last_error = Some(anyhow!("kiro upstream auth failed: {status} {body}"));
                continue;
            }
            if matches!(status.as_u16(), 401 | 403) {
                return Err(ProviderAttemptError::RetryNext(anyhow!(
                    "kiro upstream auth failed after refresh: {status} {body}"
                )));
            }
            if matches!(status.as_u16(), 408 | 429) || status.is_server_error() {
                last_error = Some(anyhow!("kiro upstream transient failure: {status} {body}"));
                tokio::time::sleep(Duration::from_millis(350)).await;
                continue;
            }
            return Err(ProviderAttemptError::Fatal(anyhow!(
                "kiro upstream failure: {status} {body}"
            )));
        }
        drop(channel_lease);
        Err(ProviderAttemptError::RetryNext(
            last_error.unwrap_or_else(|| anyhow!("kiro upstream request failed")),
        ))
    }

    async fn call_mcp_for_account(
        &self,
        account_name: &str,
        request_body: &str,
        channel_lease: KiroRequestLease,
    ) -> Result<ProviderCallResult, ProviderAttemptError> {
        let mut force_refresh = false;
        let mut last_error: Option<anyhow::Error> = None;
        for attempt in 0..3 {
            let ctx = self
                .runtime
                .token_manager
                .ensure_context_for_account(account_name, force_refresh)
                .await
                .map_err(ProviderAttemptError::RetryNext)?;
            let (client, resolved_proxy) =
                build_client(self.runtime.upstream_proxy_registry.as_ref(), 120)
                    .await
                    .map_err(ProviderAttemptError::Fatal)?;
            let url = format!("https://q.{}.amazonaws.com/mcp", ctx.auth.effective_api_region());
            tracing::info!(
                account_name = %ctx.auth.name,
                attempt = attempt + 1,
                force_refresh,
                api_region = ctx.auth.effective_api_region(),
                proxy_url = %resolved_proxy.proxy_url,
                proxy_source = ?resolved_proxy.source,
                proxy_config_id = ?resolved_proxy.proxy_config_id,
                request_body_len = request_body.len(),
                queue_wait_ms = channel_lease.waited_ms(),
                "calling kiro upstream mcp"
            );
            let response = client
                .post(url)
                .headers(build_mcp_headers(&ctx).map_err(ProviderAttemptError::Fatal)?)
                .body(request_body.to_string())
                .send()
                .await;
            let response = match response {
                Ok(response) => response,
                Err(err) => {
                    last_error = Some(err.into());
                    continue;
                },
            };
            if response.status().is_success() {
                tracing::info!(
                    account_name = %ctx.auth.name,
                    attempt = attempt + 1,
                    status = %response.status(),
                    queue_wait_ms = channel_lease.waited_ms(),
                    "kiro upstream mcp request succeeded"
                );
                return Ok(ProviderCallResult {
                    response,
                    account_name: ctx.auth.name,
                    _channel_lease: channel_lease,
                });
            }
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            tracing::warn!(
                account_name = %ctx.auth.name,
                attempt = attempt + 1,
                status = %status,
                body_len = body.len(),
                force_refresh,
                queue_wait_ms = channel_lease.waited_ms(),
                "kiro upstream mcp request returned non-success status"
            );
            if status.as_u16() == 400 {
                return Err(ProviderAttemptError::Fatal(anyhow!(
                    "kiro upstream mcp rejected request: {status} {body}"
                )));
            }
            if status.as_u16() == 402 && is_monthly_request_limit(&body) {
                return Err(ProviderAttemptError::QuotaExhausted(anyhow!(
                    "kiro account quota exhausted for mcp request: {status} {body}"
                )));
            }
            if status.as_u16() == 429 {
                if let Some(cooldown) = daily_request_limit_cooldown(&body) {
                    return Err(ProviderAttemptError::RateLimited {
                        error: anyhow!("kiro upstream mcp rate limit reached: {status} {body}"),
                        cooldown,
                    });
                }
            }
            if matches!(status.as_u16(), 401 | 403) && !force_refresh {
                tracing::info!(
                    account_name = %ctx.auth.name,
                    attempt = attempt + 1,
                    status = %status,
                    "kiro upstream mcp auth failed; forcing token refresh before retry"
                );
                force_refresh = true;
                last_error = Some(anyhow!("kiro upstream mcp auth failed: {status} {body}"));
                continue;
            }
            if matches!(status.as_u16(), 401 | 403) {
                return Err(ProviderAttemptError::RetryNext(anyhow!(
                    "kiro upstream mcp auth failed after refresh: {status} {body}"
                )));
            }
            if matches!(status.as_u16(), 408 | 429) || status.is_server_error() {
                last_error = Some(anyhow!("kiro upstream mcp transient failure: {status} {body}"));
                tokio::time::sleep(Duration::from_millis(350)).await;
                continue;
            }
            return Err(ProviderAttemptError::Fatal(anyhow!(
                "kiro upstream mcp failure: {status} {body}"
            )));
        }
        drop(channel_lease);
        Err(ProviderAttemptError::RetryNext(
            last_error.unwrap_or_else(|| anyhow!("kiro upstream mcp request failed")),
        ))
    }
}

fn build_headers(ctx: &CallContext) -> Result<HeaderMap> {
    let machine_id = machine_id::generate_from_auth(&ctx.auth)
        .ok_or_else(|| anyhow!("failed to derive kiro machine id"))?;
    let x_amz_user_agent = format!("aws-sdk-js/1.0.27 KiroIDE-{DEFAULT_KIRO_VERSION}-{machine_id}");
    let user_agent = format!(
        "aws-sdk-js/1.0.27 ua/2.1 os/{DEFAULT_SYSTEM_VERSION} lang/js \
         md/nodejs#{DEFAULT_NODE_VERSION} api/codewhispererstreaming#1.0.27 m/E \
         KiroIDE-{DEFAULT_KIRO_VERSION}-{machine_id}"
    );
    let host = format!("q.{}.amazonaws.com", ctx.auth.effective_api_region());
    let mut headers = HeaderMap::new();
    headers.insert("content-type", HeaderValue::from_static("application/json"));
    headers.insert("x-amzn-codewhisperer-optout", HeaderValue::from_static("true"));
    headers.insert("x-amzn-kiro-agent-mode", HeaderValue::from_static("vibe"));
    headers.insert(
        "x-amz-user-agent",
        HeaderValue::from_str(&x_amz_user_agent).context("invalid x-amz-user-agent")?,
    );
    headers.insert("user-agent", HeaderValue::from_str(&user_agent).context("invalid user-agent")?);
    headers.insert("host", HeaderValue::from_str(&host).context("invalid host header")?);
    headers.insert(
        "amz-sdk-invocation-id",
        HeaderValue::from_str(&uuid::Uuid::new_v4().to_string())
            .context("invalid invocation id")?,
    );
    headers.insert("amz-sdk-request", HeaderValue::from_static("attempt=1; max=3"));
    headers.insert(
        "authorization",
        HeaderValue::from_str(&format!("Bearer {}", ctx.token)).context("invalid auth header")?,
    );
    headers.insert("connection", HeaderValue::from_static("close"));
    Ok(headers)
}

fn build_mcp_headers(ctx: &CallContext) -> Result<HeaderMap> {
    let machine_id = machine_id::generate_from_auth(&ctx.auth)
        .ok_or_else(|| anyhow!("failed to derive kiro machine id"))?;
    let x_amz_user_agent = format!("aws-sdk-js/1.0.27 KiroIDE-{DEFAULT_KIRO_VERSION}-{machine_id}");
    let user_agent = format!(
        "aws-sdk-js/1.0.27 ua/2.1 os/{DEFAULT_SYSTEM_VERSION} lang/js \
         md/nodejs#{DEFAULT_NODE_VERSION} api/codewhispererstreaming#1.0.27 m/E \
         KiroIDE-{DEFAULT_KIRO_VERSION}-{machine_id}"
    );
    let host = format!("q.{}.amazonaws.com", ctx.auth.effective_api_region());
    let mut headers = HeaderMap::new();
    headers.insert("content-type", HeaderValue::from_static("application/json"));
    headers.insert(
        "x-amz-user-agent",
        HeaderValue::from_str(&x_amz_user_agent).context("invalid x-amz-user-agent")?,
    );
    headers.insert("user-agent", HeaderValue::from_str(&user_agent).context("invalid user-agent")?);
    headers.insert("host", HeaderValue::from_str(&host).context("invalid host header")?);
    headers.insert(
        "amz-sdk-invocation-id",
        HeaderValue::from_str(&uuid::Uuid::new_v4().to_string())
            .context("invalid invocation id")?,
    );
    headers.insert("amz-sdk-request", HeaderValue::from_static("attempt=1; max=3"));
    headers.insert(
        "authorization",
        HeaderValue::from_str(&format!("Bearer {}", ctx.token)).context("invalid auth header")?,
    );
    headers.insert("connection", HeaderValue::from_static("close"));
    Ok(headers)
}

/// Order accounts by cached remaining balance (descending). Accounts without
/// cached balance information are placed last (still eligible, just unknown
/// priority).
fn balance_ordered_accounts(
    auths: &[KiroAuthRecord],
    snapshot: &KiroStatusCacheSnapshot,
) -> Vec<KiroAuthRecord> {
    let mut sorted = auths.to_vec();
    sorted.sort_by(|a, b| {
        let remaining_a = snapshot
            .accounts
            .get(&a.name)
            .and_then(|s| s.balance.as_ref())
            .map(|bal| bal.remaining)
            .unwrap_or(-1.0);
        let remaining_b = snapshot
            .accounts
            .get(&b.name)
            .and_then(|s| s.balance.as_ref())
            .map(|bal| bal.remaining)
            .unwrap_or(-1.0);
        remaining_b
            .partial_cmp(&remaining_a)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    sorted
}

fn is_monthly_request_limit(body: &str) -> bool {
    if body.contains("MONTHLY_REQUEST_COUNT") {
        return true;
    }
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("reason")
                .and_then(|item| item.as_str())
                .map(ToString::to_string)
                .or_else(|| {
                    value
                        .pointer("/error/reason")
                        .and_then(|item| item.as_str())
                        .map(ToString::to_string)
                })
        })
        .is_some_and(|value| value == "MONTHLY_REQUEST_COUNT")
}

fn daily_request_limit_cooldown(body: &str) -> Option<Duration> {
    if body.contains("5-minute credit limit exceeded") {
        return Some(Duration::from_secs(5 * 60));
    }
    let value = serde_json::from_str::<serde_json::Value>(body).ok()?;
    let reason = value
        .get("reason")
        .and_then(|item| item.as_str())
        .or_else(|| {
            value
                .pointer("/error/reason")
                .and_then(|item| item.as_str())
        });
    if reason == Some("DAILY_REQUEST_COUNT") {
        return Some(Duration::from_secs(5 * 60));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kiro_gateway::{
        status_cache::{KiroCachedAccountStatus, STATUS_QUOTA_EXHAUSTED},
        types::{KiroBalanceView, KiroCacheView},
    };

    fn auth(name: &str) -> KiroAuthRecord {
        KiroAuthRecord {
            name: name.to_string(),
            ..KiroAuthRecord::default()
        }
    }

    /// Build a test snapshot entry with a known remaining balance for one
    /// account.
    fn snapshot_with_balance(name: &str, remaining: f64) -> (String, KiroCachedAccountStatus) {
        (name.to_string(), KiroCachedAccountStatus {
            balance: Some(KiroBalanceView {
                current_usage: 100.0 - remaining,
                usage_limit: 100.0,
                remaining,
                next_reset_at: None,
                subscription_title: None,
            }),
            cache: KiroCacheView {
                status: "ready".to_string(),
                refresh_interval_seconds: 60,
                last_checked_at: Some(1),
                last_success_at: Some(1),
                error_message: None,
            },
        })
    }

    #[test]
    fn balance_ordered_sorts_by_remaining_descending() {
        let auths = vec![auth("alpha"), auth("beta"), auth("gamma")];
        let (na, sa) = snapshot_with_balance("alpha", 10.0);
        let (nb, sb) = snapshot_with_balance("beta", 80.0);
        let (nc, sc) = snapshot_with_balance("gamma", 40.0);
        let snapshot = KiroStatusCacheSnapshot {
            accounts: [(na, sa), (nb, sb), (nc, sc)].into_iter().collect(),
            ..Default::default()
        };

        let ordered = balance_ordered_accounts(&auths, &snapshot);
        let names: Vec<&str> = ordered.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["beta", "gamma", "alpha"]);
    }

    #[test]
    fn balance_ordered_unknown_accounts_sort_last() {
        let auths = vec![auth("alpha"), auth("beta")];
        let (na, sa) = snapshot_with_balance("alpha", 50.0);
        let snapshot = KiroStatusCacheSnapshot {
            accounts: [(na, sa)].into_iter().collect(),
            ..Default::default()
        };

        let ordered = balance_ordered_accounts(&auths, &snapshot);
        let names: Vec<&str> = ordered.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "beta"]);
    }

    #[test]
    fn cached_quota_exhausted_account_is_not_request_eligible() {
        let auth = auth("alpha");
        let status = KiroCachedAccountStatus {
            balance: Some(KiroBalanceView {
                current_usage: 100.0,
                usage_limit: 100.0,
                remaining: 0.0,
                next_reset_at: None,
                subscription_title: None,
            }),
            cache: KiroCacheView {
                status: STATUS_QUOTA_EXHAUSTED.to_string(),
                refresh_interval_seconds: 60,
                last_checked_at: Some(1),
                last_success_at: Some(1),
                error_message: Some("quota exhausted".to_string()),
            },
        };

        assert!(!account_is_request_eligible(&auth, Some(&status)));
    }

    #[test]
    fn daily_request_limit_detects_upstream_five_minute_window() {
        let body = r#"{"message":"5-minute credit limit exceeded","reason":"DAILY_REQUEST_COUNT"}"#;
        assert_eq!(daily_request_limit_cooldown(body), Some(Duration::from_secs(5 * 60)));
    }

    #[test]
    fn daily_request_limit_ignores_other_reasons() {
        let body = r#"{"message":"too many requests","reason":"OTHER_LIMIT"}"#;
        assert_eq!(daily_request_limit_cooldown(body), None);
    }
}
