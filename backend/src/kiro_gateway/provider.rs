//! Multi-account Kiro upstream provider with retry, cooldown, and
//! quota-exhaustion failover.
//!
//! [`KiroProvider`] iterates over configured accounts by readiness and
//! fairness: the least-recently-started eligible identity is tried first, with
//! cached remaining quota as a secondary tie-breaker. Disabled, cooling-down,
//! or quota-exhausted accounts are skipped. Each per-account attempt retries up
//! to 3 times (with forced token refresh on 401/403), and the outer loop
//! sleeps through cooldown windows before giving up.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context, Result};
use reqwest::{
    header::{HeaderMap, HeaderValue},
    StatusCode,
};
use static_flow_shared::llm_gateway_store::{LlmGatewayKeyRecord, LLM_GATEWAY_PROVIDER_KIRO};

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
use crate::upstream_proxy::{HttpClientProfile, ResolvedUpstreamProxy, UpstreamProxyRegistry};

const KIRO_PROVIDER_AWS_SDK_VERSION: &str = "1.0.34";
const KIRO_LOG_BODY_PREVIEW_CHARS: usize = 8_192;

pub(crate) const KIRO_API_CLIENT_PROFILE: HttpClientProfile =
    HttpClientProfile::new(Some(720), 8, 60);
pub(crate) const KIRO_MCP_CLIENT_PROFILE: HttpClientProfile =
    HttpClientProfile::new(Some(120), 8, 60);
pub(crate) const KIRO_AUX_CLIENT_PROFILE: HttpClientProfile =
    HttpClientProfile::new(Some(60), 8, 60);

/// Build a [`reqwest::Client`] configured for Kiro upstream calls together
/// with the resolved provider-level proxy metadata used for diagnostics.
pub async fn build_client(
    proxy_registry: &UpstreamProxyRegistry,
    auth: &KiroAuthRecord,
    profile: HttpClientProfile,
) -> Result<(reqwest::Client, ResolvedUpstreamProxy)> {
    proxy_registry
        .client_for_selection(LLM_GATEWAY_PROVIDER_KIRO, Some(&auth.proxy_selection()), profile)
        .await
        .context("failed to resolve kiro upstream proxy")
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

#[derive(Clone, Copy)]
enum UpstreamLogLevel {
    Warn,
    Error,
}

struct UpstreamRequestLogContext<'a> {
    auth: &'a KiroAuthRecord,
    resolved_proxy: &'a ResolvedUpstreamProxy,
    endpoint: &'a str,
    attempt: usize,
    force_refresh: bool,
    queue_wait_ms: u64,
    request_body_len: usize,
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
        key_record: &LlmGatewayKeyRecord,
        conversation_state: &ConversationState,
    ) -> Result<ProviderCallResult> {
        self.call_api_inner(key_record, conversation_state).await
    }

    /// Streaming variant of [`call_api`](Self::call_api). The response body
    /// should be consumed as an event stream.
    pub async fn call_api_stream(
        &self,
        key_record: &LlmGatewayKeyRecord,
        conversation_state: &ConversationState,
    ) -> Result<ProviderCallResult> {
        self.call_api_inner(key_record, conversation_state).await
    }

    /// Send an MCP (Model Context Protocol) request through the account pool.
    pub async fn call_mcp(
        &self,
        key_record: &LlmGatewayKeyRecord,
        request_body: &str,
    ) -> Result<ProviderCallResult> {
        self.call_mcp_inner(key_record, request_body).await
    }

    // Outer loop: retries the full account rotation when all accounts are
    // cooling down. Breaks out on success, fatal error, or full exhaustion.
    async fn call_api_inner(
        &self,
        key_record: &LlmGatewayKeyRecord,
        conversation_state: &ConversationState,
    ) -> Result<ProviderCallResult> {
        let queued_at = Instant::now();
        loop {
            let auths = self.runtime.token_manager.list_auths().await?;
            if auths.is_empty() {
                return Err(anyhow!("no kiro account available for request"));
            }
            let auths = filter_auths_for_key_route(&auths, key_record)?;
            let snapshot = self.runtime.cached_status_snapshot().await;
            let mut last_error: Option<anyhow::Error> = None;
            let mut saw_quota_exhausted = false;
            let mut shortest_cooldown: Option<Duration> = None;
            let mut shortest_local_wait: Option<Duration> = None;
            let mut saw_local_limit = false;
            let mut blocked_accounts = Vec::new();

            let ordered_auths = selection_ordered_accounts(
                &auths,
                &snapshot,
                self.runtime.request_scheduler.as_ref(),
            );
            // Iterate accounts in fairness order so ready identities rotate
            // instead of repeatedly draining the current balance leader.
            for auth in ordered_auths {
                let routing_identity = routing_identity_for_account(&auth, &snapshot);
                // Skip accounts still in an upstream cooldown window.
                if let Some(cooldown) = self
                    .runtime
                    .request_scheduler
                    .cooldown_for_account(&routing_identity)
                {
                    shortest_cooldown = Some(match shortest_cooldown {
                        Some(current) => current.min(cooldown.remaining),
                        None => cooldown.remaining,
                    });
                    tracing::info!(
                        account_name = %auth.name,
                        routing_identity = %routing_identity,
                        cooldown_ms = cooldown.remaining.as_millis() as u64,
                        reason = %cooldown.reason,
                        "skipping kiro account before request because it is in upstream cooldown"
                    );
                    blocked_accounts.push(format!(
                        "{}[{}]: upstream_cooldown wait_ms={} reason={}",
                        auth.name,
                        routing_identity,
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
                    routing_identity = %routing_identity,
                    cache_status = cache_entry
                        .map(|status| status.cache.status.as_str())
                        .unwrap_or("unknown"),
                    reason = if quota_exhausted { "cached_quota_unavailable" } else { "disabled" },
                    "skipping kiro account before request"
                    );
                    blocked_accounts.push(format!(
                        "{}[{}]: {} cache_status={} remaining={}",
                        auth.name,
                        routing_identity,
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
                    &routing_identity,
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
                            routing_identity = %routing_identity,
                            request_kind = "messages",
                            reason = throttle.reason,
                            wait_ms = throttle.wait.map(|value| value.as_millis() as u64).unwrap_or(0),
                            in_flight = throttle.in_flight,
                            max_concurrency = throttle.max_concurrency,
                            min_start_interval_ms = throttle.min_start_interval_ms,
                            "skipping kiro account before request because it is locally throttled"
                        );
                        blocked_accounts.push(format!(
                            "{}[{}]: {} in_flight={} max_concurrency={} min_start_interval_ms={} \
                             wait_ms={}",
                            auth.name,
                            routing_identity,
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
                    routing_identity = %routing_identity,
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
                        let shared_accounts =
                            accounts_for_routing_identity(&auths, &snapshot, &routing_identity);
                        tracing::warn!(
                            account_name = %auth.name,
                            routing_identity = %routing_identity,
                            shared_accounts = ?shared_accounts,
                            error = %err,
                            "kiro account quota exhausted during request; moving to next account"
                        );
                        for account_name in shared_accounts {
                            mark_account_quota_exhausted(
                                &self.runtime,
                                &account_name,
                                err.to_string(),
                            )
                            .await;
                        }
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
                            &routing_identity,
                            cooldown,
                            error.to_string(),
                        );
                        tracing::warn!(
                            account_name = %auth.name,
                            routing_identity = %routing_identity,
                            error = %error,
                            cooldown_ms = cooldown.as_millis() as u64,
                            "kiro account hit upstream 5-minute credit window; moving to next account"
                        );
                        last_error = Some(error);
                    },
                    Err(ProviderAttemptError::RetryNext(err)) => {
                        tracing::warn!(
                            account_name = %auth.name,
                            routing_identity = %routing_identity,
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

    async fn call_mcp_inner(
        &self,
        key_record: &LlmGatewayKeyRecord,
        request_body: &str,
    ) -> Result<ProviderCallResult> {
        let queued_at = Instant::now();
        loop {
            let auths = self.runtime.token_manager.list_auths().await?;
            if auths.is_empty() {
                return Err(anyhow!("no kiro account available for mcp request"));
            }
            let auths = filter_auths_for_key_route(&auths, key_record)?;
            let snapshot = self.runtime.cached_status_snapshot().await;
            let mut last_error: Option<anyhow::Error> = None;
            let mut saw_quota_exhausted = false;
            let mut shortest_cooldown: Option<Duration> = None;
            let mut shortest_local_wait: Option<Duration> = None;
            let mut saw_local_limit = false;
            let mut blocked_accounts = Vec::new();

            let ordered_auths = selection_ordered_accounts(
                &auths,
                &snapshot,
                self.runtime.request_scheduler.as_ref(),
            );
            for auth in ordered_auths {
                let routing_identity = routing_identity_for_account(&auth, &snapshot);
                if let Some(cooldown) = self
                    .runtime
                    .request_scheduler
                    .cooldown_for_account(&routing_identity)
                {
                    shortest_cooldown = Some(match shortest_cooldown {
                        Some(current) => current.min(cooldown.remaining),
                        None => cooldown.remaining,
                    });
                    tracing::info!(
                        account_name = %auth.name,
                        routing_identity = %routing_identity,
                        cooldown_ms = cooldown.remaining.as_millis() as u64,
                        reason = %cooldown.reason,
                        "skipping kiro account before mcp request because it is in upstream cooldown"
                    );
                    blocked_accounts.push(format!(
                        "{}[{}]: upstream_cooldown wait_ms={} reason={}",
                        auth.name,
                        routing_identity,
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
                    routing_identity = %routing_identity,
                    cache_status = cache_entry
                        .map(|status| status.cache.status.as_str())
                        .unwrap_or("unknown"),
                    reason = if quota_exhausted { "cached_quota_unavailable" } else { "disabled" },
                    "skipping kiro account before mcp request"
                    );
                    blocked_accounts.push(format!(
                        "{}[{}]: {} cache_status={} remaining={}",
                        auth.name,
                        routing_identity,
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
                    &routing_identity,
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
                            routing_identity = %routing_identity,
                            request_kind = "mcp",
                            reason = throttle.reason,
                            wait_ms = throttle.wait.map(|value| value.as_millis() as u64).unwrap_or(0),
                            in_flight = throttle.in_flight,
                            max_concurrency = throttle.max_concurrency,
                            min_start_interval_ms = throttle.min_start_interval_ms,
                            "skipping kiro account before mcp request because it is locally throttled"
                        );
                        blocked_accounts.push(format!(
                            "{}[{}]: {} in_flight={} max_concurrency={} min_start_interval_ms={} \
                             wait_ms={}",
                            auth.name,
                            routing_identity,
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
                    routing_identity = %routing_identity,
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
                        let shared_accounts =
                            accounts_for_routing_identity(&auths, &snapshot, &routing_identity);
                        tracing::warn!(
                            account_name = %auth.name,
                            routing_identity = %routing_identity,
                            shared_accounts = ?shared_accounts,
                            error = %err,
                            "kiro account quota exhausted during mcp request; moving to next account"
                        );
                        for account_name in shared_accounts {
                            mark_account_quota_exhausted(
                                &self.runtime,
                                &account_name,
                                err.to_string(),
                            )
                            .await;
                        }
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
                            &routing_identity,
                            cooldown,
                            error.to_string(),
                        );
                        tracing::warn!(
                            account_name = %auth.name,
                            routing_identity = %routing_identity,
                            error = %error,
                            cooldown_ms = cooldown.as_millis() as u64,
                            "kiro account hit upstream 5-minute credit window for mcp; moving to next account"
                        );
                        last_error = Some(error);
                    },
                    Err(ProviderAttemptError::RetryNext(err)) => {
                        tracing::warn!(
                            account_name = %auth.name,
                            routing_identity = %routing_identity,
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
        let queue_wait_ms = channel_lease.waited_ms();
        for attempt in 0..3 {
            let attempt = attempt + 1;
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
            let (client, resolved_proxy) = build_client(
                self.runtime.upstream_proxy_registry.as_ref(),
                &ctx.auth,
                KIRO_API_CLIENT_PROFILE,
            )
            .await
            .map_err(ProviderAttemptError::Fatal)?;
            let url = format!(
                "https://q.{}.amazonaws.com/generateAssistantResponse",
                ctx.auth.effective_api_region()
            );
            tracing::info!(
                account_name = %ctx.auth.name,
                attempt,
                force_refresh,
                api_region = ctx.auth.effective_api_region(),
                proxy_url = %resolved_proxy.proxy_url_label(),
                proxy_source = ?resolved_proxy.source,
                proxy_config_id = ?resolved_proxy.proxy_config_id,
                proxy_config_name = ?resolved_proxy.proxy_config_name,
                request_body_len = request_body.len(),
                has_profile_arn = ctx.auth.profile_arn.is_some(),
                queue_wait_ms,
                "calling kiro upstream generateAssistantResponse"
            );
            let log_ctx = UpstreamRequestLogContext {
                auth: &ctx.auth,
                resolved_proxy: &resolved_proxy,
                endpoint: "/generateAssistantResponse",
                attempt,
                force_refresh,
                queue_wait_ms,
                request_body_len: request_body.len(),
            };
            let response = client
                .post(url)
                .headers(build_headers(&ctx).map_err(ProviderAttemptError::Fatal)?)
                .body(request_body)
                .send()
                .await;
            let response = match response {
                Ok(response) => response,
                Err(err) => {
                    log_upstream_send_error(&log_ctx, &err);
                    let invalidated = self
                        .runtime
                        .upstream_proxy_registry
                        .invalidate_client_if_connect_error(
                            &resolved_proxy,
                            KIRO_API_CLIENT_PROFILE,
                            &err,
                        )
                        .await;
                    tracing::warn!(
                        account_name = %ctx.auth.name,
                        proxy_source = %resolved_proxy.source.as_str(),
                        proxy_url = %resolved_proxy.proxy_url_label(),
                        invalidated_client = invalidated,
                        "kiro upstream client send failed: {err}"
                    );
                    last_error = Some(anyhow!(
                        "kiro upstream transport failure for {}: {err}",
                        log_ctx.endpoint
                    ));
                    continue;
                },
            };
            if response.status().is_success() {
                tracing::info!(
                    account_name = %ctx.auth.name,
                    attempt,
                    status = %response.status(),
                    endpoint = log_ctx.endpoint,
                    queue_wait_ms,
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
            if status.as_u16() == 402 && is_monthly_request_limit(&body) {
                log_upstream_response(
                    &log_ctx,
                    status,
                    &body,
                    UpstreamLogLevel::Warn,
                    "kiro upstream request returned quota-exhausted response",
                    None,
                );
                return Err(ProviderAttemptError::QuotaExhausted(anyhow!(
                    "kiro account quota exhausted: {status} {body}"
                )));
            }
            if status.as_u16() == 429 {
                if let Some(cooldown) = daily_request_limit_cooldown(&body) {
                    log_upstream_response(
                        &log_ctx,
                        status,
                        &body,
                        UpstreamLogLevel::Warn,
                        "kiro upstream request hit 5-minute credit-window limit",
                        Some(cooldown),
                    );
                    return Err(ProviderAttemptError::RateLimited {
                        error: anyhow!("kiro upstream rate limit reached: {status} {body}"),
                        cooldown,
                    });
                }
            }
            let response_message = match status.as_u16() {
                400 => "kiro upstream request returned fatal client error",
                401 | 403 => "kiro upstream request returned auth failure",
                408 | 429 => "kiro upstream request returned transient retryable status",
                _ if status.is_server_error() => {
                    "kiro upstream request returned upstream server error"
                },
                _ => "kiro upstream request returned fatal non-success status",
            };
            log_upstream_response(
                &log_ctx,
                status,
                &body,
                UpstreamLogLevel::Error,
                response_message,
                None,
            );
            if status.as_u16() == 400 {
                return Err(ProviderAttemptError::Fatal(anyhow!(
                    "kiro upstream rejected request: {status} {body}"
                )));
            }
            if matches!(status.as_u16(), 401 | 403) && !force_refresh {
                tracing::info!(
                    account_name = %ctx.auth.name,
                    attempt,
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
        let queue_wait_ms = channel_lease.waited_ms();
        for attempt in 0..3 {
            let attempt = attempt + 1;
            let ctx = self
                .runtime
                .token_manager
                .ensure_context_for_account(account_name, force_refresh)
                .await
                .map_err(ProviderAttemptError::RetryNext)?;
            let (client, resolved_proxy) = build_client(
                self.runtime.upstream_proxy_registry.as_ref(),
                &ctx.auth,
                KIRO_MCP_CLIENT_PROFILE,
            )
            .await
            .map_err(ProviderAttemptError::Fatal)?;
            let url = format!("https://q.{}.amazonaws.com/mcp", ctx.auth.effective_api_region());
            tracing::info!(
                account_name = %ctx.auth.name,
                attempt,
                force_refresh,
                api_region = ctx.auth.effective_api_region(),
                proxy_url = %resolved_proxy.proxy_url_label(),
                proxy_source = ?resolved_proxy.source,
                proxy_config_id = ?resolved_proxy.proxy_config_id,
                proxy_config_name = ?resolved_proxy.proxy_config_name,
                request_body_len = request_body.len(),
                has_profile_arn = ctx.auth.profile_arn.is_some(),
                queue_wait_ms,
                "calling kiro upstream mcp"
            );
            let log_ctx = UpstreamRequestLogContext {
                auth: &ctx.auth,
                resolved_proxy: &resolved_proxy,
                endpoint: "/mcp",
                attempt,
                force_refresh,
                queue_wait_ms,
                request_body_len: request_body.len(),
            };
            let response = client
                .post(url)
                .headers(build_mcp_headers(&ctx).map_err(ProviderAttemptError::Fatal)?)
                .body(request_body.to_string())
                .send()
                .await;
            let response = match response {
                Ok(response) => response,
                Err(err) => {
                    log_upstream_send_error(&log_ctx, &err);
                    let invalidated = self
                        .runtime
                        .upstream_proxy_registry
                        .invalidate_client_if_connect_error(
                            &resolved_proxy,
                            KIRO_MCP_CLIENT_PROFILE,
                            &err,
                        )
                        .await;
                    tracing::warn!(
                        account_name = %ctx.auth.name,
                        proxy_source = %resolved_proxy.source.as_str(),
                        proxy_url = %resolved_proxy.proxy_url_label(),
                        invalidated_client = invalidated,
                        "kiro upstream mcp client send failed: {err}"
                    );
                    last_error = Some(anyhow!(
                        "kiro upstream transport failure for {}: {err}",
                        log_ctx.endpoint
                    ));
                    continue;
                },
            };
            if response.status().is_success() {
                tracing::info!(
                    account_name = %ctx.auth.name,
                    attempt,
                    status = %response.status(),
                    endpoint = log_ctx.endpoint,
                    queue_wait_ms,
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
            if status.as_u16() == 402 && is_monthly_request_limit(&body) {
                log_upstream_response(
                    &log_ctx,
                    status,
                    &body,
                    UpstreamLogLevel::Warn,
                    "kiro upstream mcp request returned quota-exhausted response",
                    None,
                );
                return Err(ProviderAttemptError::QuotaExhausted(anyhow!(
                    "kiro account quota exhausted for mcp request: {status} {body}"
                )));
            }
            if status.as_u16() == 429 {
                if let Some(cooldown) = daily_request_limit_cooldown(&body) {
                    log_upstream_response(
                        &log_ctx,
                        status,
                        &body,
                        UpstreamLogLevel::Warn,
                        "kiro upstream mcp request hit 5-minute credit-window limit",
                        Some(cooldown),
                    );
                    return Err(ProviderAttemptError::RateLimited {
                        error: anyhow!("kiro upstream mcp rate limit reached: {status} {body}"),
                        cooldown,
                    });
                }
            }
            let response_message = match status.as_u16() {
                400 => "kiro upstream mcp request returned fatal client error",
                401 | 403 => "kiro upstream mcp request returned auth failure",
                408 | 429 => "kiro upstream mcp request returned transient retryable status",
                _ if status.is_server_error() => {
                    "kiro upstream mcp request returned upstream server error"
                },
                _ => "kiro upstream mcp request returned fatal non-success status",
            };
            log_upstream_response(
                &log_ctx,
                status,
                &body,
                UpstreamLogLevel::Error,
                response_message,
                None,
            );
            if status.as_u16() == 400 {
                return Err(ProviderAttemptError::Fatal(anyhow!(
                    "kiro upstream mcp rejected request: {status} {body}"
                )));
            }
            if matches!(status.as_u16(), 401 | 403) && !force_refresh {
                tracing::info!(
                    account_name = %ctx.auth.name,
                    attempt,
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

fn provider_user_agents(machine_id: &str) -> (String, String) {
    (
        format!(
            "aws-sdk-js/{KIRO_PROVIDER_AWS_SDK_VERSION} \
             KiroIDE-{DEFAULT_KIRO_VERSION}-{machine_id}"
        ),
        format!(
            "aws-sdk-js/{KIRO_PROVIDER_AWS_SDK_VERSION} ua/2.1 os/{DEFAULT_SYSTEM_VERSION} \
             lang/js md/nodejs#{DEFAULT_NODE_VERSION} \
             api/codewhispererstreaming#{KIRO_PROVIDER_AWS_SDK_VERSION} m/E \
             KiroIDE-{DEFAULT_KIRO_VERSION}-{machine_id}"
        ),
    )
}

fn summarize_logged_body(body: &str) -> String {
    let total_chars = body.chars().count();
    if total_chars <= KIRO_LOG_BODY_PREVIEW_CHARS {
        return body.to_string();
    }
    let preview = body
        .chars()
        .take(KIRO_LOG_BODY_PREVIEW_CHARS)
        .collect::<String>();
    format!("{preview}...[truncated,total_chars={total_chars}]")
}

fn log_upstream_send_error(log_ctx: &UpstreamRequestLogContext<'_>, err: &reqwest::Error) {
    tracing::error!(
        account_name = %log_ctx.auth.name,
        attempt = log_ctx.attempt,
        endpoint = log_ctx.endpoint,
        api_region = %log_ctx.auth.effective_api_region(),
        proxy_url = %log_ctx.resolved_proxy.proxy_url_label(),
        proxy_source = ?log_ctx.resolved_proxy.source,
        proxy_config_id = ?log_ctx.resolved_proxy.proxy_config_id,
        proxy_config_name = ?log_ctx.resolved_proxy.proxy_config_name,
        force_refresh = log_ctx.force_refresh,
        queue_wait_ms = log_ctx.queue_wait_ms,
        request_body_len = log_ctx.request_body_len,
        has_profile_arn = log_ctx.auth.profile_arn.is_some(),
        is_timeout = err.is_timeout(),
        is_connect = err.is_connect(),
        is_request = err.is_request(),
        upstream_url = ?err.url(),
        error = %err,
        "kiro upstream transport request failed"
    );
}

fn log_upstream_response(
    log_ctx: &UpstreamRequestLogContext<'_>,
    status: StatusCode,
    body: &str,
    level: UpstreamLogLevel,
    message: &str,
    cooldown: Option<Duration>,
) {
    let body_preview = summarize_logged_body(body);
    let cooldown_ms = cooldown.map(|value| value.as_millis() as u64);
    match level {
        UpstreamLogLevel::Warn => {
            tracing::warn!(
                account_name = %log_ctx.auth.name,
                attempt = log_ctx.attempt,
                endpoint = log_ctx.endpoint,
                status = %status,
                body_len = body.len(),
                body_preview = %body_preview,
                api_region = %log_ctx.auth.effective_api_region(),
                proxy_url = %log_ctx.resolved_proxy.proxy_url_label(),
                proxy_source = ?log_ctx.resolved_proxy.source,
                proxy_config_id = ?log_ctx.resolved_proxy.proxy_config_id,
                proxy_config_name = ?log_ctx.resolved_proxy.proxy_config_name,
                force_refresh = log_ctx.force_refresh,
                queue_wait_ms = log_ctx.queue_wait_ms,
                request_body_len = log_ctx.request_body_len,
                has_profile_arn = log_ctx.auth.profile_arn.is_some(),
                cooldown_ms,
                "{message}"
            );
        },
        UpstreamLogLevel::Error => {
            tracing::error!(
                account_name = %log_ctx.auth.name,
                attempt = log_ctx.attempt,
                endpoint = log_ctx.endpoint,
                status = %status,
                body_len = body.len(),
                body_preview = %body_preview,
                api_region = %log_ctx.auth.effective_api_region(),
                proxy_url = %log_ctx.resolved_proxy.proxy_url_label(),
                proxy_source = ?log_ctx.resolved_proxy.source,
                proxy_config_id = ?log_ctx.resolved_proxy.proxy_config_id,
                proxy_config_name = ?log_ctx.resolved_proxy.proxy_config_name,
                force_refresh = log_ctx.force_refresh,
                queue_wait_ms = log_ctx.queue_wait_ms,
                request_body_len = log_ctx.request_body_len,
                has_profile_arn = log_ctx.auth.profile_arn.is_some(),
                cooldown_ms,
                "{message}"
            );
        },
    }
}

fn build_headers(ctx: &CallContext) -> Result<HeaderMap> {
    let machine_id = machine_id::generate_from_auth(&ctx.auth)
        .ok_or_else(|| anyhow!("failed to derive kiro machine id"))?;
    let (x_amz_user_agent, user_agent) = provider_user_agents(&machine_id);
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
    let (x_amz_user_agent, user_agent) = provider_user_agents(&machine_id);
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
    if let Some(profile_arn) = ctx
        .auth
        .profile_arn
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        headers.insert(
            "x-amzn-kiro-profile-arn",
            HeaderValue::from_str(profile_arn).context("invalid profile arn header")?,
        );
    }
    headers.insert("connection", HeaderValue::from_static("close"));
    Ok(headers)
}

fn filter_auths_for_key_route(
    auths: &[KiroAuthRecord],
    key_record: &LlmGatewayKeyRecord,
) -> anyhow::Result<Vec<KiroAuthRecord>> {
    match key_record.route_strategy.as_deref() {
        None => Ok(auths.to_vec()),
        Some("fixed") => {
            let account_name = key_record
                .fixed_account_name
                .as_deref()
                .ok_or_else(|| anyhow!("fixed route_strategy requires fixed_account_name"))?;
            let matched = auths
                .iter()
                .filter(|auth| auth.name == account_name)
                .cloned()
                .collect::<Vec<_>>();
            if matched.is_empty() {
                anyhow::bail!("fixed route account `{account_name}` is not available");
            }
            Ok(matched)
        },
        Some("auto") => {
            let Some(auto_account_names) = key_record.auto_account_names.as_deref() else {
                return Ok(auths.to_vec());
            };
            let matched = auths
                .iter()
                .filter(|auth| auto_account_names.contains(&auth.name))
                .cloned()
                .collect::<Vec<_>>();
            if matched.is_empty() {
                anyhow::bail!("no configured auto accounts are available");
            }
            Ok(matched)
        },
        Some(other) => anyhow::bail!("unsupported route strategy `{other}`"),
    }
}

/// Order accounts by readiness/fairness first, then remaining balance.
///
/// Accounts that have never started a request in this process are tried first.
/// Once every identity has history, the least-recently-started identity wins so
/// one balance leader cannot absorb long consecutive request streaks. Cached
/// remaining balance stays as the secondary tie-breaker.
fn selection_ordered_accounts(
    auths: &[KiroAuthRecord],
    snapshot: &KiroStatusCacheSnapshot,
    scheduler: &super::scheduler::KiroRequestScheduler,
) -> Vec<KiroAuthRecord> {
    let mut sorted = auths.to_vec();
    sorted.sort_by(|a, b| {
        let identity_a = routing_identity_for_account(a, snapshot);
        let identity_b = routing_identity_for_account(b, snapshot);
        let last_started_a = scheduler.last_started_at(&identity_a);
        let last_started_b = scheduler.last_started_at(&identity_b);
        match (last_started_a, last_started_b) {
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(left), Some(right)) => {
                let ordering = left.cmp(&right);
                if ordering != std::cmp::Ordering::Equal {
                    return ordering;
                }
            },
            (None, None) => {},
        }
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
            .then_with(|| a.name.cmp(&b.name))
    });
    sorted
}

fn routing_identity_for_account(
    auth: &KiroAuthRecord,
    snapshot: &KiroStatusCacheSnapshot,
) -> String {
    snapshot
        .accounts
        .get(&auth.name)
        .and_then(|status| status.balance.as_ref())
        .and_then(|balance| balance.user_id.clone())
        .unwrap_or_else(|| auth.name.clone())
}

fn accounts_for_routing_identity(
    auths: &[KiroAuthRecord],
    snapshot: &KiroStatusCacheSnapshot,
    routing_identity: &str,
) -> Vec<String> {
    auths
        .iter()
        .filter(|auth| routing_identity_for_account(auth, snapshot) == routing_identity)
        .map(|auth| auth.name.clone())
        .collect()
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

    fn key(
        route_strategy: Option<&str>,
        fixed: Option<&str>,
        subset: Vec<&str>,
    ) -> LlmGatewayKeyRecord {
        LlmGatewayKeyRecord {
            id: "test".to_string(),
            name: "test-key".to_string(),
            route_strategy: route_strategy.map(str::to_string),
            fixed_account_name: fixed.map(str::to_string),
            auto_account_names: Some(subset.into_iter().map(str::to_string).collect()),
            secret: String::new(),
            key_hash: String::new(),
            status: "active".to_string(),
            provider_type: "kiro".to_string(),
            protocol_family: "anthropic".to_string(),
            public_visible: false,
            quota_billable_limit: 0,
            usage_input_uncached_tokens: 0,
            usage_input_cached_tokens: 0,
            usage_output_tokens: 0,
            usage_billable_tokens: 0,
            usage_credit_total: 0.0,
            usage_credit_missing_events: 0,
            last_used_at: None,
            created_at: 0,
            updated_at: 0,
            model_name_map: None,
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
        }
    }

    fn call_context(profile_arn: Option<&str>) -> CallContext {
        CallContext {
            auth: KiroAuthRecord {
                name: "alpha".to_string(),
                machine_id: Some("a".repeat(64)),
                profile_arn: profile_arn.map(str::to_string),
                api_region: Some("us-west-2".to_string()),
                ..KiroAuthRecord::default()
            },
            token: "token".to_string(),
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
                user_id: None,
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
    fn selection_ordered_prefers_higher_balance_without_history() {
        let auths = vec![auth("alpha"), auth("beta"), auth("gamma")];
        let (na, sa) = snapshot_with_balance("alpha", 10.0);
        let (nb, sb) = snapshot_with_balance("beta", 80.0);
        let (nc, sc) = snapshot_with_balance("gamma", 40.0);
        let snapshot = KiroStatusCacheSnapshot {
            accounts: [(na, sa), (nb, sb), (nc, sc)].into_iter().collect(),
            ..Default::default()
        };

        let scheduler = super::super::scheduler::KiroRequestScheduler::new();
        let ordered = selection_ordered_accounts(&auths, &snapshot, scheduler.as_ref());
        let names: Vec<&str> = ordered.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["beta", "gamma", "alpha"]);
    }

    #[test]
    fn selection_ordered_unknown_accounts_sort_last() {
        let auths = vec![auth("alpha"), auth("beta")];
        let (na, sa) = snapshot_with_balance("alpha", 50.0);
        let snapshot = KiroStatusCacheSnapshot {
            accounts: [(na, sa)].into_iter().collect(),
            ..Default::default()
        };

        let scheduler = super::super::scheduler::KiroRequestScheduler::new();
        let ordered = selection_ordered_accounts(&auths, &snapshot, scheduler.as_ref());
        let names: Vec<&str> = ordered.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "beta"]);
    }

    #[test]
    fn filter_auths_for_key_route_auto_no_subset_keeps_full_pool() {
        let auths = vec![auth("alpha"), auth("beta")];
        let auths = filter_auths_for_key_route(&auths, &key(None, None, vec![]))
            .expect("route should keep full pool");
        let names: Vec<&str> = auths.iter().map(|item| item.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "beta"]);
    }

    #[test]
    fn filter_auths_for_key_route_auto_strategy_without_subset_keeps_full_pool() {
        let auths = vec![auth("alpha"), auth("beta")];
        let mut key_record = key(Some("auto"), None, vec!["alpha"]);
        key_record.auto_account_names = None;
        let auths =
            filter_auths_for_key_route(&auths, &key_record).expect("route should keep full pool");
        let names: Vec<&str> = auths.iter().map(|item| item.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "beta"]);
    }

    #[test]
    fn filter_auths_for_key_route_auto_with_empty_subset_errors() {
        let auths = vec![auth("alpha"), auth("beta")];
        let err = filter_auths_for_key_route(&auths, &key(Some("auto"), None, vec![])).unwrap_err();
        assert_eq!(err.to_string(), "no configured auto accounts are available");
    }

    #[test]
    fn filter_auths_for_key_route_fixed_keeps_single_account() {
        let auths = vec![auth("alpha"), auth("beta")];
        let auths = filter_auths_for_key_route(&auths, &key(Some("fixed"), Some("beta"), vec![]))
            .expect("route should keep fixed account");
        let names: Vec<&str> = auths.iter().map(|item| item.name.as_str()).collect();
        assert_eq!(names, vec!["beta"]);
    }

    #[test]
    fn filter_auths_for_key_route_auto_with_subset_keeps_subset() {
        let auths = vec![auth("alpha"), auth("beta"), auth("gamma")];
        let auths =
            filter_auths_for_key_route(&auths, &key(Some("auto"), None, vec!["alpha", "gamma"]))
                .expect("route should keep auto subset");
        let names: Vec<&str> = auths.iter().map(|item| item.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "gamma"]);
    }

    #[test]
    fn filter_auths_for_key_route_fixed_missing_account_errors() {
        let auths = vec![auth("alpha"), auth("beta")];
        let err = filter_auths_for_key_route(&auths, &key(Some("fixed"), Some("missing"), vec![]))
            .unwrap_err();
        assert!(err
            .to_string()
            .contains("fixed route account `missing` is not available"));
    }

    #[test]
    fn selection_ordered_prefers_least_recently_started_identity() {
        let auths = vec![auth("alpha"), auth("beta"), auth("gamma")];
        let (na, sa) = snapshot_with_balance("alpha", 10.0);
        let (nb, sb) = snapshot_with_balance("beta", 90.0);
        let (nc, sc) = snapshot_with_balance("gamma", 70.0);
        let snapshot = KiroStatusCacheSnapshot {
            accounts: [(na, sa), (nb, sb), (nc, sc)].into_iter().collect(),
            ..Default::default()
        };
        let scheduler = super::super::scheduler::KiroRequestScheduler::new();
        let queued_at = Instant::now();

        let alpha = scheduler
            .try_acquire("alpha", 1, 0, queued_at)
            .expect("alpha should acquire");
        drop(alpha);
        std::thread::sleep(Duration::from_millis(2));
        let gamma = scheduler
            .try_acquire("gamma", 1, 0, queued_at)
            .expect("gamma should acquire");
        drop(gamma);

        let ordered = selection_ordered_accounts(&auths, &snapshot, scheduler.as_ref());
        let names: Vec<&str> = ordered.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["beta", "alpha", "gamma"]);
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
                user_id: None,
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

    #[test]
    fn routing_identity_prefers_cached_upstream_user_id() {
        let auth = auth("alias-alpha");
        let snapshot = KiroStatusCacheSnapshot {
            accounts: [("alias-alpha".to_string(), KiroCachedAccountStatus {
                balance: Some(KiroBalanceView {
                    current_usage: 10.0,
                    usage_limit: 100.0,
                    remaining: 90.0,
                    next_reset_at: None,
                    subscription_title: None,
                    user_id: Some("user-123".to_string()),
                }),
                cache: KiroCacheView {
                    status: "ready".to_string(),
                    refresh_interval_seconds: 60,
                    last_checked_at: Some(1),
                    last_success_at: Some(1),
                    error_message: None,
                },
            })]
            .into_iter()
            .collect(),
            ..Default::default()
        };

        assert_eq!(routing_identity_for_account(&auth, &snapshot), "user-123");
    }

    #[test]
    fn accounts_for_routing_identity_groups_aliases_of_same_user() {
        let auths = vec![auth("default"), auth("fhhfdss"), auth("hfdeeh")];
        let snapshot = KiroStatusCacheSnapshot {
            accounts: [
                ("default".to_string(), KiroCachedAccountStatus {
                    balance: Some(KiroBalanceView {
                        current_usage: 50.0,
                        usage_limit: 100.0,
                        remaining: 50.0,
                        next_reset_at: None,
                        subscription_title: None,
                        user_id: Some("user-a".to_string()),
                    }),
                    cache: KiroCacheView {
                        status: "ready".to_string(),
                        refresh_interval_seconds: 60,
                        last_checked_at: Some(1),
                        last_success_at: Some(1),
                        error_message: None,
                    },
                }),
                ("fhhfdss".to_string(), KiroCachedAccountStatus {
                    balance: Some(KiroBalanceView {
                        current_usage: 10.0,
                        usage_limit: 100.0,
                        remaining: 90.0,
                        next_reset_at: None,
                        subscription_title: None,
                        user_id: Some("user-b".to_string()),
                    }),
                    cache: KiroCacheView {
                        status: "ready".to_string(),
                        refresh_interval_seconds: 60,
                        last_checked_at: Some(1),
                        last_success_at: Some(1),
                        error_message: None,
                    },
                }),
                ("hfdeeh".to_string(), KiroCachedAccountStatus {
                    balance: Some(KiroBalanceView {
                        current_usage: 10.0,
                        usage_limit: 100.0,
                        remaining: 90.0,
                        next_reset_at: None,
                        subscription_title: None,
                        user_id: Some("user-b".to_string()),
                    }),
                    cache: KiroCacheView {
                        status: "ready".to_string(),
                        refresh_interval_seconds: 60,
                        last_checked_at: Some(1),
                        last_success_at: Some(1),
                        error_message: None,
                    },
                }),
            ]
            .into_iter()
            .collect(),
            ..Default::default()
        };

        assert_eq!(accounts_for_routing_identity(&auths, &snapshot, "user-b"), vec![
            "fhhfdss".to_string(),
            "hfdeeh".to_string()
        ]);
    }

    #[test]
    fn build_headers_use_latest_provider_signature() {
        let headers = build_headers(&call_context(None)).expect("provider headers");
        let expected_amz_user_agent = format!(
            "aws-sdk-js/{KIRO_PROVIDER_AWS_SDK_VERSION} KiroIDE-{DEFAULT_KIRO_VERSION}-{}",
            "a".repeat(64)
        );
        assert_eq!(
            headers
                .get("x-amz-user-agent")
                .and_then(|value| value.to_str().ok()),
            Some(expected_amz_user_agent.as_str())
        );
        let user_agent = headers
            .get("user-agent")
            .and_then(|value| value.to_str().ok())
            .expect("user-agent");
        assert!(user_agent.contains(&format!(
            "aws-sdk-js/{KIRO_PROVIDER_AWS_SDK_VERSION} ua/2.1 os/{DEFAULT_SYSTEM_VERSION} \
             lang/js md/nodejs#{DEFAULT_NODE_VERSION}"
        )));
        assert!(user_agent
            .contains(&format!("api/codewhispererstreaming#{KIRO_PROVIDER_AWS_SDK_VERSION}")));
    }

    #[test]
    fn build_mcp_headers_include_profile_arn_when_present() {
        let headers = build_mcp_headers(&call_context(Some("arn:aws:kiro:::profile/test")))
            .expect("mcp headers");
        assert_eq!(
            headers
                .get("x-amzn-kiro-profile-arn")
                .and_then(|value| value.to_str().ok()),
            Some("arn:aws:kiro:::profile/test")
        );
    }

    #[test]
    fn summarize_logged_body_truncates_large_payloads() {
        let input = "x".repeat(KIRO_LOG_BODY_PREVIEW_CHARS + 32);
        let summary = summarize_logged_body(&input);

        assert!(summary.starts_with(&"x".repeat(KIRO_LOG_BODY_PREVIEW_CHARS)));
        assert!(summary
            .contains(&format!("[truncated,total_chars={}]", KIRO_LOG_BODY_PREVIEW_CHARS + 32)));
        assert!(summary.len() < input.len() + 64);
    }
}
