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
use serde::Serialize;
use static_flow_shared::llm_gateway_store::{LlmGatewayKeyRecord, LLM_GATEWAY_PROVIDER_KIRO};

use super::{
    auth_file::{
        KiroAuthRecord, DEFAULT_KIRO_VERSION, DEFAULT_NODE_VERSION, DEFAULT_SYSTEM_VERSION,
    },
    machine_id,
    runtime::{CallContext, KiroGatewayRuntimeState},
    scheduler::KiroRequestLease,
    status_cache::{
        account_is_request_eligible, account_request_block_reason,
        ensure_cached_status_for_account, mark_account_quota_exhausted, KiroCachedAccountStatus,
        KiroStatusCacheSnapshot, RequestEligibilityBlockReason,
    },
    wire::{ConversationState, KiroRequest},
};
use crate::upstream_proxy::{HttpClientProfile, ResolvedUpstreamProxy, UpstreamProxyRegistry};

const KIRO_PROVIDER_AWS_SDK_VERSION: &str = "1.0.34";
const KIRO_LOG_BODY_PREVIEW_CHARS: usize = 8_192;
// Observed Kiro upstream CONTENT_LENGTH_EXCEEDS_THRESHOLD failures occurred at
// 1,630,504 and 1,707,517 bytes. Keep a small local safety margin so we fail
// fast before sending requests that are already beyond the practical limit.
const KIRO_GENERATE_REQUEST_MAX_BODY_BYTES: usize = 1_600_000;

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
    pub request_body: String,
    pub routing_wait_ms: u64,
    pub upstream_headers_ms: u64,
    pub quota_failover_count: u64,
    pub routing_diagnostics_json: Option<String>,
    _channel_lease: KiroRequestLease,
}

#[derive(Debug, Clone, Serialize)]
struct KiroRoutingAttemptDiagnostic {
    account_name: String,
    routing_identity: String,
    outcome: &'static str,
    reason: Option<String>,
    wait_ms: Option<u64>,
    cache_status: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct KiroRoutingDiagnostics {
    request_kind: &'static str,
    route_total_ms: u64,
    selection_ms: u64,
    status_load_ms: u64,
    status_load_count: u64,
    local_queue_wait_ms: u64,
    upstream_cooldown_wait_ms: u64,
    account_attempt_count: u64,
    skipped_account_count: u64,
    local_throttle_count: u64,
    quota_failover_count: u64,
    rate_limit_failover_count: u64,
    retry_next_count: u64,
    selected_account: Option<String>,
    selected_routing_identity: Option<String>,
    attempts: Vec<KiroRoutingAttemptDiagnostic>,
}

impl KiroRoutingDiagnostics {
    fn new(request_kind: &'static str) -> Self {
        Self {
            request_kind,
            route_total_ms: 0,
            selection_ms: 0,
            status_load_ms: 0,
            status_load_count: 0,
            local_queue_wait_ms: 0,
            upstream_cooldown_wait_ms: 0,
            account_attempt_count: 0,
            skipped_account_count: 0,
            local_throttle_count: 0,
            quota_failover_count: 0,
            rate_limit_failover_count: 0,
            retry_next_count: 0,
            selected_account: None,
            selected_routing_identity: None,
            attempts: Vec::new(),
        }
    }

    fn add_status_load(&mut self, elapsed_ms: u64) {
        self.status_load_count = self.status_load_count.saturating_add(1);
        self.status_load_ms = self.status_load_ms.saturating_add(elapsed_ms);
    }

    fn add_local_wait(&mut self, elapsed_ms: u64) {
        self.local_queue_wait_ms = self.local_queue_wait_ms.saturating_add(elapsed_ms);
    }

    fn add_cooldown_wait(&mut self, elapsed_ms: u64) {
        self.upstream_cooldown_wait_ms = self.upstream_cooldown_wait_ms.saturating_add(elapsed_ms);
    }

    fn add_attempt(
        &mut self,
        account_name: &str,
        routing_identity: &str,
        outcome: &'static str,
        reason: Option<String>,
        wait_ms: Option<u64>,
        cache_status: Option<&str>,
    ) {
        if outcome == "skipped" {
            self.skipped_account_count = self.skipped_account_count.saturating_add(1);
        } else {
            self.account_attempt_count = self.account_attempt_count.saturating_add(1);
        }
        if self.attempts.len() >= 64 {
            return;
        }
        self.attempts.push(KiroRoutingAttemptDiagnostic {
            account_name: account_name.to_string(),
            routing_identity: routing_identity.to_string(),
            outcome,
            reason: reason.map(truncate_diagnostic_reason),
            wait_ms,
            cache_status: cache_status.map(str::to_string),
        });
    }

    fn mark_success(&mut self, account_name: &str, routing_identity: &str) {
        self.selected_account = Some(account_name.to_string());
        self.selected_routing_identity = Some(routing_identity.to_string());
    }

    fn into_json(mut self, route_total_ms: u64, quota_failover_count: u64) -> Option<String> {
        self.route_total_ms = route_total_ms;
        self.quota_failover_count = quota_failover_count;
        let attributed_wait = self
            .status_load_ms
            .saturating_add(self.local_queue_wait_ms)
            .saturating_add(self.upstream_cooldown_wait_ms);
        self.selection_ms = route_total_ms.saturating_sub(attributed_wait);
        serde_json::to_string(&self).ok()
    }
}

#[derive(Debug)]
pub struct ProviderCallError {
    pub error: anyhow::Error,
    pub request_body: Option<String>,
}

impl ProviderCallError {
    pub(crate) fn new(error: anyhow::Error, request_body: Option<String>) -> Self {
        Self {
            error,
            request_body,
        }
    }
}

fn validate_generate_request_size(request_body_len: usize) -> Result<()> {
    if request_body_len <= KIRO_GENERATE_REQUEST_MAX_BODY_BYTES {
        return Ok(());
    }

    let over_limit_bytes = request_body_len.saturating_sub(KIRO_GENERATE_REQUEST_MAX_BODY_BYTES);
    Err(anyhow!(
        "kiro local request rejected before upstream call: CONTENT_LENGTH_EXCEEDS_THRESHOLD \
         request_body_len={request_body_len} \
         request_body_limit={KIRO_GENERATE_REQUEST_MAX_BODY_BYTES} \
         over_limit_bytes={over_limit_bytes}"
    ))
}

fn elapsed_ms(started_at: Instant) -> u64 {
    started_at.elapsed().as_millis().min(u64::MAX as u128) as u64
}

fn truncate_diagnostic_reason(reason: String) -> String {
    const MAX_CHARS: usize = 512;
    if reason.chars().count() <= MAX_CHARS {
        return reason;
    }
    let mut truncated: String = reason.chars().take(MAX_CHARS).collect();
    truncated.push_str("...");
    truncated
}

impl std::fmt::Display for ProviderCallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(f)
    }
}

impl std::error::Error for ProviderCallError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.error.root_cause())
    }
}

impl From<anyhow::Error> for ProviderCallError {
    fn from(error: anyhow::Error) -> Self {
        Self::new(error, None)
    }
}

/// Per-account attempt outcome that drives the retry/failover decision.
enum ProviderAttemptError {
    /// Transient failure; try the next account in the rotation.
    RetryNext(ProviderCallError),
    /// Unrecoverable error; abort immediately.
    Fatal(ProviderCallError),
    /// Monthly quota exhausted (HTTP 402); mark account and move on.
    QuotaExhausted(ProviderCallError),
    /// Upstream 5-minute credit window hit (HTTP 429); apply cooldown then move
    /// on.
    RateLimited { error: ProviderCallError, cooldown: Duration },
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
    ) -> std::result::Result<ProviderCallResult, ProviderCallError> {
        self.call_api_inner(key_record, conversation_state).await
    }

    /// Streaming variant of [`call_api`](Self::call_api). The response body
    /// should be consumed as an event stream.
    pub async fn call_api_stream(
        &self,
        key_record: &LlmGatewayKeyRecord,
        conversation_state: &ConversationState,
    ) -> std::result::Result<ProviderCallResult, ProviderCallError> {
        self.call_api_inner(key_record, conversation_state).await
    }

    /// Send an MCP (Model Context Protocol) request through the account pool.
    pub async fn call_mcp(
        &self,
        key_record: &LlmGatewayKeyRecord,
        request_body: &str,
    ) -> std::result::Result<ProviderCallResult, ProviderCallError> {
        self.call_mcp_inner(key_record, request_body).await
    }

    async fn ensure_status_for_selection(
        &self,
        snapshot: &mut KiroStatusCacheSnapshot,
        auth: &KiroAuthRecord,
        request_kind: &str,
        request_body: Option<&str>,
    ) -> std::result::Result<KiroCachedAccountStatus, ProviderCallError> {
        if let Some(entry) = snapshot.accounts.get(&auth.name).cloned() {
            return Ok(entry);
        }
        let entry = ensure_cached_status_for_account(&self.runtime, &auth.name)
            .await
            .map_err(|err| {
                ProviderCallError::new(
                    anyhow!(
                        "failed to load kiro account status for `{}` before {request_kind} \
                         selection: {err:#}",
                        auth.name
                    ),
                    request_body.map(str::to_string),
                )
            })?;
        snapshot.accounts.insert(auth.name.clone(), entry.clone());
        Ok(entry)
    }

    // Outer loop: retries the full account rotation when all accounts are
    // cooling down. Breaks out on success, fatal error, or full exhaustion.
    async fn call_api_inner(
        &self,
        key_record: &LlmGatewayKeyRecord,
        conversation_state: &ConversationState,
    ) -> std::result::Result<ProviderCallResult, ProviderCallError> {
        let queued_at = Instant::now();
        let mut quota_failover_count = 0u64;
        let mut diagnostics = KiroRoutingDiagnostics::new("messages");
        loop {
            let auths = self.runtime.token_manager.list_auths().await?;
            if auths.is_empty() {
                return Err(anyhow!("no kiro account available for request").into());
            }
            let auths = filter_auths_for_key_route(
                self.runtime.llm_gateway_store.as_ref(),
                &auths,
                key_record,
            )
            .await?;
            let mut snapshot = self.runtime.cached_status_snapshot().await;
            let mut last_error: Option<ProviderCallError> = None;
            let mut saw_quota_exhausted = false;
            let mut saw_minimum_remaining_threshold = false;
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
                let status_was_cached = snapshot.accounts.contains_key(&auth.name);
                let status_load_started = Instant::now();
                let cache_entry = match self
                    .ensure_status_for_selection(&mut snapshot, &auth, "messages", None)
                    .await
                {
                    Ok(entry) => {
                        if !status_was_cached {
                            diagnostics.add_status_load(elapsed_ms(status_load_started));
                        }
                        entry
                    },
                    Err(err) => {
                        diagnostics.add_attempt(
                            &auth.name,
                            &auth.name,
                            "skipped",
                            Some(format!("status_load_error: {err}")),
                            None,
                            None,
                        );
                        last_error = Some(err);
                        continue;
                    },
                };
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
                    diagnostics.add_attempt(
                        &auth.name,
                        &routing_identity,
                        "skipped",
                        Some(format!("upstream_cooldown: {}", cooldown.reason)),
                        Some(cooldown.remaining.as_millis() as u64),
                        Some(cache_entry.cache.status.as_str()),
                    );
                    continue;
                }

                // Skip accounts that the status cache marks as ineligible.
                if !account_is_request_eligible(&auth, Some(&cache_entry)) {
                    let block_reason = account_request_block_reason(&auth, Some(&cache_entry));
                    let threshold = auth.effective_minimum_remaining_credits_before_block();
                    match block_reason {
                        Some(RequestEligibilityBlockReason::QuotaExhausted) => {
                            saw_quota_exhausted = true;
                        },
                        Some(RequestEligibilityBlockReason::MinimumRemainingCreditsThreshold) => {
                            saw_minimum_remaining_threshold = true;
                        },
                        _ => {
                            last_error = Some(ProviderCallError::new(
                                anyhow!("kiro account `{}` is disabled", auth.name),
                                None,
                            ));
                        },
                    }
                    tracing::info!(
                    account_name = %auth.name,
                    routing_identity = %routing_identity,
                    cache_status = cache_entry.cache.status.as_str(),
                    reason = match block_reason {
                        Some(RequestEligibilityBlockReason::QuotaExhausted) => "cached_quota_unavailable",
                        Some(RequestEligibilityBlockReason::MinimumRemainingCreditsThreshold) => "minimum_remaining_credits_threshold",
                        _ => "disabled",
                    },
                    "skipping kiro account before request"
                    );
                    blocked_accounts.push(format!(
                        "{}[{}]: {} cache_status={} remaining={} threshold={:.4}",
                        auth.name,
                        routing_identity,
                        match block_reason {
                            Some(RequestEligibilityBlockReason::QuotaExhausted) =>
                                "cached_quota_unavailable",
                            Some(
                                RequestEligibilityBlockReason::MinimumRemainingCreditsThreshold,
                            ) => "minimum_remaining_credits_threshold",
                            _ => "disabled",
                        },
                        cache_entry.cache.status.as_str(),
                        cache_entry
                            .balance
                            .as_ref()
                            .map(|balance| format!("{:.4}", balance.remaining))
                            .unwrap_or_else(|| "unknown".to_string()),
                        threshold
                    ));
                    diagnostics.add_attempt(
                        &auth.name,
                        &routing_identity,
                        "skipped",
                        Some(match block_reason {
                            Some(RequestEligibilityBlockReason::QuotaExhausted) => {
                                "cached_quota_unavailable".to_string()
                            },
                            Some(
                                RequestEligibilityBlockReason::MinimumRemainingCreditsThreshold,
                            ) => "minimum_remaining_credits_threshold".to_string(),
                            _ => "disabled".to_string(),
                        }),
                        None,
                        Some(cache_entry.cache.status.as_str()),
                    );
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
                        diagnostics.local_throttle_count =
                            diagnostics.local_throttle_count.saturating_add(1);
                        diagnostics.add_attempt(
                            &auth.name,
                            &routing_identity,
                            "skipped",
                            Some(throttle.reason.to_string()),
                            throttle.wait.map(|value| value.as_millis() as u64),
                            Some(cache_entry.cache.status.as_str()),
                        );
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
                    Ok(mut result) => {
                        result.quota_failover_count = quota_failover_count;
                        diagnostics.mark_success(&auth.name, &routing_identity);
                        diagnostics.add_attempt(
                            &auth.name,
                            &routing_identity,
                            "success",
                            None,
                            None,
                            Some(cache_entry.cache.status.as_str()),
                        );
                        result.routing_diagnostics_json =
                            diagnostics.into_json(result.routing_wait_ms, quota_failover_count);
                        return Ok(result);
                    },
                    Err(ProviderAttemptError::QuotaExhausted(err)) => {
                        quota_failover_count = quota_failover_count.saturating_add(1);
                        saw_quota_exhausted = true;
                        diagnostics.add_attempt(
                            &auth.name,
                            &routing_identity,
                            "quota_exhausted",
                            Some(err.to_string()),
                            None,
                            Some(cache_entry.cache.status.as_str()),
                        );
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
                        diagnostics.rate_limit_failover_count =
                            diagnostics.rate_limit_failover_count.saturating_add(1);
                        diagnostics.add_attempt(
                            &auth.name,
                            &routing_identity,
                            "rate_limited",
                            Some(error.to_string()),
                            Some(cooldown.as_millis() as u64),
                            Some(cache_entry.cache.status.as_str()),
                        );
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
                        diagnostics.retry_next_count =
                            diagnostics.retry_next_count.saturating_add(1);
                        diagnostics.add_attempt(
                            &auth.name,
                            &routing_identity,
                            "retry_next",
                            Some(err.to_string()),
                            None,
                            Some(cache_entry.cache.status.as_str()),
                        );
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
                let wait_started = Instant::now();
                self.runtime
                    .request_scheduler
                    .wait_for_available(combined_wait)
                    .await;
                diagnostics.add_local_wait(elapsed_ms(wait_started));
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
                let wait_started = Instant::now();
                tokio::time::sleep(wait).await;
                diagnostics.add_cooldown_wait(elapsed_ms(wait_started));
                continue;
            }

            if saw_quota_exhausted {
                return Err(ProviderCallError::new(
                    anyhow!(
                        "all configured kiro accounts are quota exhausted; blocked_accounts={}",
                        blocked_accounts.join(" | ")
                    ),
                    None,
                ));
            }
            if saw_minimum_remaining_threshold {
                return Err(ProviderCallError::new(
                    anyhow!(
                        "all configured kiro accounts are below the configured minimum remaining \
                         credits threshold; blocked_accounts={}",
                        blocked_accounts.join(" | ")
                    ),
                    None,
                ));
            }
            let base_error = last_error.unwrap_or_else(|| {
                ProviderCallError::new(anyhow!("no kiro account available for request"), None)
            });
            let request_body = base_error.request_body.clone();
            return Err(ProviderCallError::new(
                anyhow!("{base_error}; blocked_accounts={}", blocked_accounts.join(" | ")),
                request_body,
            ));
        }
    }

    async fn call_mcp_inner(
        &self,
        key_record: &LlmGatewayKeyRecord,
        request_body: &str,
    ) -> std::result::Result<ProviderCallResult, ProviderCallError> {
        let queued_at = Instant::now();
        let mut quota_failover_count = 0u64;
        let mut diagnostics = KiroRoutingDiagnostics::new("mcp");
        loop {
            let auths = self.runtime.token_manager.list_auths().await?;
            if auths.is_empty() {
                return Err(anyhow!("no kiro account available for mcp request").into());
            }
            let auths = filter_auths_for_key_route(
                self.runtime.llm_gateway_store.as_ref(),
                &auths,
                key_record,
            )
            .await?;
            let mut snapshot = self.runtime.cached_status_snapshot().await;
            let mut last_error: Option<ProviderCallError> = None;
            let mut saw_quota_exhausted = false;
            let mut saw_minimum_remaining_threshold = false;
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
                let status_was_cached = snapshot.accounts.contains_key(&auth.name);
                let status_load_started = Instant::now();
                let cache_entry = match self
                    .ensure_status_for_selection(&mut snapshot, &auth, "mcp", Some(request_body))
                    .await
                {
                    Ok(entry) => {
                        if !status_was_cached {
                            diagnostics.add_status_load(elapsed_ms(status_load_started));
                        }
                        entry
                    },
                    Err(err) => {
                        diagnostics.add_attempt(
                            &auth.name,
                            &auth.name,
                            "skipped",
                            Some(format!("status_load_error: {err}")),
                            None,
                            None,
                        );
                        last_error = Some(err);
                        continue;
                    },
                };
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
                    diagnostics.add_attempt(
                        &auth.name,
                        &routing_identity,
                        "skipped",
                        Some(format!("upstream_cooldown: {}", cooldown.reason)),
                        Some(cooldown.remaining.as_millis() as u64),
                        Some(cache_entry.cache.status.as_str()),
                    );
                    continue;
                }

                if !account_is_request_eligible(&auth, Some(&cache_entry)) {
                    let block_reason = account_request_block_reason(&auth, Some(&cache_entry));
                    let threshold = auth.effective_minimum_remaining_credits_before_block();
                    match block_reason {
                        Some(RequestEligibilityBlockReason::QuotaExhausted) => {
                            saw_quota_exhausted = true;
                        },
                        Some(RequestEligibilityBlockReason::MinimumRemainingCreditsThreshold) => {
                            saw_minimum_remaining_threshold = true;
                        },
                        _ => {
                            last_error = Some(ProviderCallError::new(
                                anyhow!("kiro account `{}` is disabled", auth.name),
                                Some(request_body.to_string()),
                            ));
                        },
                    }
                    tracing::info!(
                    account_name = %auth.name,
                    routing_identity = %routing_identity,
                    cache_status = cache_entry.cache.status.as_str(),
                    reason = match block_reason {
                        Some(RequestEligibilityBlockReason::QuotaExhausted) => "cached_quota_unavailable",
                        Some(RequestEligibilityBlockReason::MinimumRemainingCreditsThreshold) => "minimum_remaining_credits_threshold",
                        _ => "disabled",
                    },
                    "skipping kiro account before mcp request"
                    );
                    blocked_accounts.push(format!(
                        "{}[{}]: {} cache_status={} remaining={} threshold={:.4}",
                        auth.name,
                        routing_identity,
                        match block_reason {
                            Some(RequestEligibilityBlockReason::QuotaExhausted) =>
                                "cached_quota_unavailable",
                            Some(
                                RequestEligibilityBlockReason::MinimumRemainingCreditsThreshold,
                            ) => "minimum_remaining_credits_threshold",
                            _ => "disabled",
                        },
                        cache_entry.cache.status.as_str(),
                        cache_entry
                            .balance
                            .as_ref()
                            .map(|balance| format!("{:.4}", balance.remaining))
                            .unwrap_or_else(|| "unknown".to_string()),
                        threshold
                    ));
                    diagnostics.add_attempt(
                        &auth.name,
                        &routing_identity,
                        "skipped",
                        Some(match block_reason {
                            Some(RequestEligibilityBlockReason::QuotaExhausted) => {
                                "cached_quota_unavailable".to_string()
                            },
                            Some(
                                RequestEligibilityBlockReason::MinimumRemainingCreditsThreshold,
                            ) => "minimum_remaining_credits_threshold".to_string(),
                            _ => "disabled".to_string(),
                        }),
                        None,
                        Some(cache_entry.cache.status.as_str()),
                    );
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
                        diagnostics.local_throttle_count =
                            diagnostics.local_throttle_count.saturating_add(1);
                        diagnostics.add_attempt(
                            &auth.name,
                            &routing_identity,
                            "skipped",
                            Some(throttle.reason.to_string()),
                            throttle.wait.map(|value| value.as_millis() as u64),
                            Some(cache_entry.cache.status.as_str()),
                        );
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
                    Ok(mut result) => {
                        result.quota_failover_count = quota_failover_count;
                        diagnostics.mark_success(&auth.name, &routing_identity);
                        diagnostics.add_attempt(
                            &auth.name,
                            &routing_identity,
                            "success",
                            None,
                            None,
                            Some(cache_entry.cache.status.as_str()),
                        );
                        result.routing_diagnostics_json =
                            diagnostics.into_json(result.routing_wait_ms, quota_failover_count);
                        return Ok(result);
                    },
                    Err(ProviderAttemptError::QuotaExhausted(err)) => {
                        quota_failover_count = quota_failover_count.saturating_add(1);
                        saw_quota_exhausted = true;
                        diagnostics.add_attempt(
                            &auth.name,
                            &routing_identity,
                            "quota_exhausted",
                            Some(err.to_string()),
                            None,
                            Some(cache_entry.cache.status.as_str()),
                        );
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
                        diagnostics.rate_limit_failover_count =
                            diagnostics.rate_limit_failover_count.saturating_add(1);
                        diagnostics.add_attempt(
                            &auth.name,
                            &routing_identity,
                            "rate_limited",
                            Some(error.to_string()),
                            Some(cooldown.as_millis() as u64),
                            Some(cache_entry.cache.status.as_str()),
                        );
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
                        diagnostics.retry_next_count =
                            diagnostics.retry_next_count.saturating_add(1);
                        diagnostics.add_attempt(
                            &auth.name,
                            &routing_identity,
                            "retry_next",
                            Some(err.to_string()),
                            None,
                            Some(cache_entry.cache.status.as_str()),
                        );
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
                let wait_started = Instant::now();
                self.runtime
                    .request_scheduler
                    .wait_for_available(combined_wait)
                    .await;
                diagnostics.add_local_wait(elapsed_ms(wait_started));
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
                let wait_started = Instant::now();
                tokio::time::sleep(wait).await;
                diagnostics.add_cooldown_wait(elapsed_ms(wait_started));
                continue;
            }

            if saw_quota_exhausted {
                return Err(ProviderCallError::new(
                    anyhow!(
                        "all configured kiro accounts are quota exhausted for mcp request; \
                         blocked_accounts={}",
                        blocked_accounts.join(" | ")
                    ),
                    Some(request_body.to_string()),
                ));
            }
            if saw_minimum_remaining_threshold {
                return Err(ProviderCallError::new(
                    anyhow!(
                        "all configured kiro accounts are below the configured minimum remaining \
                         credits threshold for mcp request; blocked_accounts={}",
                        blocked_accounts.join(" | ")
                    ),
                    Some(request_body.to_string()),
                ));
            }
            let base_error = last_error.unwrap_or_else(|| {
                ProviderCallError::new(
                    anyhow!("no kiro account available for mcp request"),
                    Some(request_body.to_string()),
                )
            });
            let request_body = base_error
                .request_body
                .clone()
                .or_else(|| Some(request_body.to_string()));
            return Err(ProviderCallError::new(
                anyhow!("{base_error}; blocked_accounts={}", blocked_accounts.join(" | ")),
                request_body,
            ));
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
        let mut last_error: Option<ProviderCallError> = None;
        let queue_wait_ms = channel_lease.waited_ms();
        for attempt in 0..3 {
            let attempt = attempt + 1;
            let ctx = self
                .runtime
                .token_manager
                .ensure_context_for_account(account_name, force_refresh)
                .await
                .map_err(|err| ProviderAttemptError::RetryNext(err.into()))?;
            let request_body = serde_json::to_string(&KiroRequest {
                conversation_state: conversation_state.clone(),
                profile_arn: ctx.auth.profile_arn.clone(),
            })
            .map_err(|err| {
                ProviderAttemptError::Fatal(ProviderCallError::new(
                    anyhow!("serialize kiro request: {err}"),
                    None,
                ))
            })?;
            if let Err(err) = validate_generate_request_size(request_body.len()) {
                tracing::warn!(
                    account_name = %ctx.auth.name,
                    attempt,
                    force_refresh,
                    api_region = ctx.auth.effective_api_region(),
                    request_body_len = request_body.len(),
                    request_body_limit = KIRO_GENERATE_REQUEST_MAX_BODY_BYTES,
                    over_limit_bytes = request_body
                        .len()
                        .saturating_sub(KIRO_GENERATE_REQUEST_MAX_BODY_BYTES),
                    has_profile_arn = ctx.auth.profile_arn.is_some(),
                    queue_wait_ms,
                    "kiro upstream generateAssistantResponse request body exceeds local safety limit; rejecting before upstream call"
                );
                return Err(ProviderAttemptError::Fatal(ProviderCallError::new(
                    err,
                    Some(request_body),
                )));
            }
            let (client, resolved_proxy) = build_client(
                self.runtime.upstream_proxy_registry.as_ref(),
                &ctx.auth,
                KIRO_API_CLIENT_PROFILE,
            )
            .await
            .map_err(|err| ProviderAttemptError::Fatal(err.into()))?;
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
            let upstream_started = Instant::now();
            let response = client
                .post(url)
                .headers(
                    build_headers(&ctx).map_err(|err| ProviderAttemptError::Fatal(err.into()))?,
                )
                .body(request_body.clone())
                .send()
                .await;
            let upstream_headers_ms = elapsed_ms(upstream_started);
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
                    last_error = Some(ProviderCallError::new(
                        anyhow!("kiro upstream transport failure for {}: {err}", log_ctx.endpoint),
                        Some(request_body.clone()),
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
                    upstream_headers_ms,
                    "kiro upstream request succeeded"
                );
                return Ok(ProviderCallResult {
                    response,
                    account_name: ctx.auth.name,
                    request_body,
                    routing_wait_ms: queue_wait_ms,
                    upstream_headers_ms,
                    quota_failover_count: 0,
                    routing_diagnostics_json: None,
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
                return Err(ProviderAttemptError::QuotaExhausted(ProviderCallError::new(
                    anyhow!("kiro account quota exhausted: {status} {body}"),
                    Some(request_body.clone()),
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
                        error: ProviderCallError::new(
                            anyhow!("kiro upstream rate limit reached: {status} {body}"),
                            Some(request_body.clone()),
                        ),
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
                return Err(ProviderAttemptError::Fatal(ProviderCallError::new(
                    anyhow!("kiro upstream rejected request: {status} {body}"),
                    Some(request_body.clone()),
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
                last_error = Some(ProviderCallError::new(
                    anyhow!("kiro upstream auth failed: {status} {body}"),
                    Some(request_body.clone()),
                ));
                continue;
            }
            if matches!(status.as_u16(), 401 | 403) {
                return Err(ProviderAttemptError::RetryNext(ProviderCallError::new(
                    anyhow!("kiro upstream auth failed after refresh: {status} {body}"),
                    Some(request_body.clone()),
                )));
            }
            if matches!(status.as_u16(), 408 | 429) || status.is_server_error() {
                last_error = Some(ProviderCallError::new(
                    anyhow!("kiro upstream transient failure: {status} {body}"),
                    Some(request_body.clone()),
                ));
                tokio::time::sleep(Duration::from_millis(350)).await;
                continue;
            }
            return Err(ProviderAttemptError::Fatal(ProviderCallError::new(
                anyhow!("kiro upstream failure: {status} {body}"),
                Some(request_body.clone()),
            )));
        }
        drop(channel_lease);
        Err(ProviderAttemptError::RetryNext(last_error.unwrap_or_else(|| {
            ProviderCallError::new(anyhow!("kiro upstream request failed"), None)
        })))
    }

    async fn call_mcp_for_account(
        &self,
        account_name: &str,
        request_body: &str,
        channel_lease: KiroRequestLease,
    ) -> Result<ProviderCallResult, ProviderAttemptError> {
        let mut force_refresh = false;
        let mut last_error: Option<ProviderCallError> = None;
        let queue_wait_ms = channel_lease.waited_ms();
        for attempt in 0..3 {
            let attempt = attempt + 1;
            let ctx = self
                .runtime
                .token_manager
                .ensure_context_for_account(account_name, force_refresh)
                .await
                .map_err(|err| ProviderAttemptError::RetryNext(err.into()))?;
            let (client, resolved_proxy) = build_client(
                self.runtime.upstream_proxy_registry.as_ref(),
                &ctx.auth,
                KIRO_MCP_CLIENT_PROFILE,
            )
            .await
            .map_err(|err| ProviderAttemptError::Fatal(err.into()))?;
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
            let upstream_started = Instant::now();
            let response = client
                .post(url)
                .headers(
                    build_mcp_headers(&ctx)
                        .map_err(|err| ProviderAttemptError::Fatal(err.into()))?,
                )
                .body(request_body.to_string())
                .send()
                .await;
            let upstream_headers_ms = elapsed_ms(upstream_started);
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
                    last_error = Some(ProviderCallError::new(
                        anyhow!("kiro upstream transport failure for {}: {err}", log_ctx.endpoint),
                        Some(request_body.to_string()),
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
                    upstream_headers_ms,
                    "kiro upstream mcp request succeeded"
                );
                return Ok(ProviderCallResult {
                    response,
                    account_name: ctx.auth.name,
                    request_body: request_body.to_string(),
                    routing_wait_ms: queue_wait_ms,
                    upstream_headers_ms,
                    quota_failover_count: 0,
                    routing_diagnostics_json: None,
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
                return Err(ProviderAttemptError::QuotaExhausted(ProviderCallError::new(
                    anyhow!("kiro account quota exhausted for mcp request: {status} {body}"),
                    Some(request_body.to_string()),
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
                        error: ProviderCallError::new(
                            anyhow!("kiro upstream mcp rate limit reached: {status} {body}"),
                            Some(request_body.to_string()),
                        ),
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
                return Err(ProviderAttemptError::Fatal(ProviderCallError::new(
                    anyhow!("kiro upstream mcp rejected request: {status} {body}"),
                    Some(request_body.to_string()),
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
                last_error = Some(ProviderCallError::new(
                    anyhow!("kiro upstream mcp auth failed: {status} {body}"),
                    Some(request_body.to_string()),
                ));
                continue;
            }
            if matches!(status.as_u16(), 401 | 403) {
                return Err(ProviderAttemptError::RetryNext(ProviderCallError::new(
                    anyhow!("kiro upstream mcp auth failed after refresh: {status} {body}"),
                    Some(request_body.to_string()),
                )));
            }
            if matches!(status.as_u16(), 408 | 429) || status.is_server_error() {
                last_error = Some(ProviderCallError::new(
                    anyhow!("kiro upstream mcp transient failure: {status} {body}"),
                    Some(request_body.to_string()),
                ));
                tokio::time::sleep(Duration::from_millis(350)).await;
                continue;
            }
            return Err(ProviderAttemptError::Fatal(ProviderCallError::new(
                anyhow!("kiro upstream mcp failure: {status} {body}"),
                Some(request_body.to_string()),
            )));
        }
        drop(channel_lease);
        Err(ProviderAttemptError::RetryNext(last_error.unwrap_or_else(|| {
            ProviderCallError::new(
                anyhow!("kiro upstream mcp request failed"),
                Some(request_body.to_string()),
            )
        })))
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

async fn filter_auths_for_key_route(
    store: &static_flow_shared::llm_gateway_store::LlmGatewayStore,
    auths: &[KiroAuthRecord],
    key_record: &LlmGatewayKeyRecord,
) -> anyhow::Result<Vec<KiroAuthRecord>> {
    match key_record.route_strategy.as_deref() {
        None => Ok(auths.to_vec()),
        Some("fixed") => {
            let account_name = if let Some(group_id) = key_record.account_group_id.as_deref() {
                let group = store
                    .get_account_group_by_id(group_id)
                    .await?
                    .ok_or_else(|| anyhow!("configured account_group_id does not exist"))?;
                if group.provider_type != LLM_GATEWAY_PROVIDER_KIRO {
                    anyhow::bail!("configured account_group_id belongs to a different provider");
                }
                if group.account_names.len() != 1 {
                    anyhow::bail!(
                        "fixed route_strategy requires an account group with exactly one account"
                    );
                }
                group.account_names[0].clone()
            } else {
                key_record
                    .fixed_account_name
                    .as_deref()
                    .ok_or_else(|| anyhow!("fixed route_strategy requires account_group_id"))?
                    .to_string()
            };
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
            let auto_account_names = if let Some(group_id) = key_record.account_group_id.as_deref()
            {
                let group = store
                    .get_account_group_by_id(group_id)
                    .await?
                    .ok_or_else(|| anyhow!("configured account_group_id does not exist"))?;
                if group.provider_type != LLM_GATEWAY_PROVIDER_KIRO {
                    anyhow::bail!("configured account_group_id belongs to a different provider");
                }
                Some(group.account_names)
            } else {
                key_record.auto_account_names.clone()
            };
            let Some(auto_account_names) = auto_account_names.as_deref() else {
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
    #[derive(Clone)]
    struct Candidate {
        auth: KiroAuthRecord,
        last_started_at: Option<Instant>,
        remaining: f64,
    }

    let last_started_snapshot = scheduler.last_started_snapshot();
    let mut sorted = auths
        .iter()
        .cloned()
        .map(|auth| {
            let routing_identity = routing_identity_for_account(&auth, snapshot);
            let remaining = snapshot
                .accounts
                .get(&auth.name)
                .and_then(|status| status.balance.as_ref())
                .map(|balance| balance.remaining)
                .unwrap_or(-1.0);
            Candidate {
                last_started_at: last_started_snapshot.get(&routing_identity).copied(),
                auth,
                remaining,
            }
        })
        .collect::<Vec<_>>();
    sorted.sort_by(|a, b| {
        // Sorting must only depend on immutable data. Reading scheduler state
        // inside the comparator makes the ordering change mid-sort under
        // concurrent traffic, which Rust correctly treats as undefined input.
        match (a.last_started_at, b.last_started_at) {
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
        b.remaining
            .total_cmp(&a.remaining)
            .then_with(|| a.auth.name.cmp(&b.auth.name))
    });
    sorted.into_iter().map(|candidate| candidate.auth).collect()
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
    use std::{
        fs,
        path::PathBuf,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        time::{SystemTime, UNIX_EPOCH},
    };

    use static_flow_shared::llm_gateway_store::LlmGatewayStore;

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
            account_group_id: None,
            model_name_map: None,
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
            kiro_request_validation_enabled: true,
            kiro_cache_estimation_enabled: true,
            kiro_zero_cache_debug_enabled: false,
            kiro_cache_policy_override_json: None,
            kiro_billable_model_multipliers_override_json: None,
        }
    }

    async fn temp_store() -> (PathBuf, LlmGatewayStore) {
        let dir = std::env::temp_dir().join(format!(
            "kiro-provider-tests-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time should be monotonic")
                .as_nanos()
        ));
        let store = LlmGatewayStore::connect(&dir.to_string_lossy())
            .await
            .expect("connect llm gateway store");
        (dir, store)
    }

    #[test]
    fn routing_diagnostics_json_breaks_down_waits_and_account_outcomes() {
        let mut diagnostics = KiroRoutingDiagnostics::new("messages");
        diagnostics.add_status_load(40);
        diagnostics.add_local_wait(100);
        diagnostics.add_cooldown_wait(200);
        diagnostics.local_throttle_count = 1;
        diagnostics.rate_limit_failover_count = 1;
        diagnostics.retry_next_count = 1;
        diagnostics.add_attempt(
            "acct-a",
            "identity-a",
            "skipped",
            Some("local_concurrency_limit".to_string()),
            Some(100),
            Some("ready"),
        );
        diagnostics.mark_success("acct-b", "identity-b");
        diagnostics.add_attempt("acct-b", "identity-b", "success", None, None, Some("ready"));

        let json = diagnostics
            .into_json(500, 2)
            .expect("routing diagnostics should serialize");
        let value: serde_json::Value =
            serde_json::from_str(&json).expect("routing diagnostics should be JSON");

        assert_eq!(value["request_kind"], "messages");
        assert_eq!(value["route_total_ms"], 500);
        assert_eq!(value["status_load_ms"], 40);
        assert_eq!(value["local_queue_wait_ms"], 100);
        assert_eq!(value["upstream_cooldown_wait_ms"], 200);
        assert_eq!(value["selection_ms"], 160);
        assert_eq!(value["quota_failover_count"], 2);
        assert_eq!(value["rate_limit_failover_count"], 1);
        assert_eq!(value["retry_next_count"], 1);
        assert_eq!(value["skipped_account_count"], 1);
        assert_eq!(value["account_attempt_count"], 1);
        assert_eq!(value["selected_account"], "acct-b");
        assert_eq!(value["attempts"].as_array().unwrap().len(), 2);
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
    fn validate_generate_request_size_accepts_payload_at_limit() {
        validate_generate_request_size(KIRO_GENERATE_REQUEST_MAX_BODY_BYTES)
            .expect("payload at the local safety limit should pass");
    }

    #[test]
    fn validate_generate_request_size_rejects_payload_over_limit() {
        let request_body_len = KIRO_GENERATE_REQUEST_MAX_BODY_BYTES + 1;
        let err = validate_generate_request_size(request_body_len)
            .expect_err("oversized payload should be rejected before upstream send");
        let text = err.to_string();

        assert!(text.contains("CONTENT_LENGTH_EXCEEDS_THRESHOLD"));
        assert!(text.contains(&request_body_len.to_string()));
        assert!(text.contains(&KIRO_GENERATE_REQUEST_MAX_BODY_BYTES.to_string()));
    }

    #[tokio::test]
    async fn filter_auths_for_key_route_auto_no_subset_keeps_full_pool() {
        let (dir, store) = temp_store().await;
        let auths = vec![auth("alpha"), auth("beta")];
        let auths = filter_auths_for_key_route(&store, &auths, &key(None, None, vec![]))
            .await
            .expect("route should keep full pool");
        let names: Vec<&str> = auths.iter().map(|item| item.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "beta"]);
        let _ = fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn filter_auths_for_key_route_auto_strategy_without_subset_keeps_full_pool() {
        let (dir, store) = temp_store().await;
        let auths = vec![auth("alpha"), auth("beta")];
        let mut key_record = key(Some("auto"), None, vec!["alpha"]);
        key_record.auto_account_names = None;
        let auths = filter_auths_for_key_route(&store, &auths, &key_record)
            .await
            .expect("route should keep full pool");
        let names: Vec<&str> = auths.iter().map(|item| item.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "beta"]);
        let _ = fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn filter_auths_for_key_route_auto_with_empty_subset_errors() {
        let (dir, store) = temp_store().await;
        let auths = vec![auth("alpha"), auth("beta")];
        let err = filter_auths_for_key_route(&store, &auths, &key(Some("auto"), None, vec![]))
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "no configured auto accounts are available");
        let _ = fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn filter_auths_for_key_route_fixed_keeps_single_account() {
        let (dir, store) = temp_store().await;
        let auths = vec![auth("alpha"), auth("beta")];
        let auths =
            filter_auths_for_key_route(&store, &auths, &key(Some("fixed"), Some("beta"), vec![]))
                .await
                .expect("route should keep fixed account");
        let names: Vec<&str> = auths.iter().map(|item| item.name.as_str()).collect();
        assert_eq!(names, vec!["beta"]);
        let _ = fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn filter_auths_for_key_route_auto_with_subset_keeps_subset() {
        let (dir, store) = temp_store().await;
        let auths = vec![auth("alpha"), auth("beta"), auth("gamma")];
        let auths = filter_auths_for_key_route(
            &store,
            &auths,
            &key(Some("auto"), None, vec!["alpha", "gamma"]),
        )
        .await
        .expect("route should keep auto subset");
        let names: Vec<&str> = auths.iter().map(|item| item.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "gamma"]);
        let _ = fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn filter_auths_for_key_route_fixed_missing_account_errors() {
        let (dir, store) = temp_store().await;
        let auths = vec![auth("alpha"), auth("beta")];
        let err = filter_auths_for_key_route(
            &store,
            &auths,
            &key(Some("fixed"), Some("missing"), vec![]),
        )
        .await
        .unwrap_err();
        assert!(err
            .to_string()
            .contains("fixed route account `missing` is not available"));
        let _ = fs::remove_dir_all(dir);
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
    fn selection_ordered_remains_stable_while_scheduler_updates_concurrently() {
        let auths = (0..32)
            .map(|idx| auth(&format!("acct-{idx:02}")))
            .collect::<Vec<_>>();
        let snapshot = KiroStatusCacheSnapshot {
            accounts: auths
                .iter()
                .enumerate()
                .map(|(idx, auth)| snapshot_with_balance(&auth.name, 1000.0 - idx as f64))
                .collect(),
            ..Default::default()
        };
        let scheduler = super::super::scheduler::KiroRequestScheduler::new();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker_scheduler = Arc::clone(&scheduler);
        let worker_names = auths
            .iter()
            .map(|auth| auth.name.clone())
            .collect::<Vec<_>>();

        let worker = std::thread::spawn(move || {
            let queued_at = Instant::now();
            while !worker_stop.load(Ordering::Relaxed) {
                for name in &worker_names {
                    let lease = worker_scheduler
                        .try_acquire(name, 64, 0, queued_at)
                        .expect("background acquire should succeed");
                    drop(lease);
                }
            }
        });

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            for _ in 0..512 {
                let ordered = selection_ordered_accounts(&auths, &snapshot, scheduler.as_ref());
                assert_eq!(ordered.len(), auths.len());
            }
        }));

        stop.store(true, Ordering::Relaxed);
        worker
            .join()
            .expect("background scheduler worker should join");

        assert!(
            result.is_ok(),
            "sorting should not panic while scheduler state changes concurrently"
        );
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
    fn threshold_blocked_account_is_not_request_eligible() {
        let auth = KiroAuthRecord {
            minimum_remaining_credits_before_block: Some(10.0),
            ..auth("alpha")
        };
        let status = KiroCachedAccountStatus {
            balance: Some(KiroBalanceView {
                current_usage: 93.0,
                usage_limit: 100.0,
                remaining: 7.0,
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
