use std::{
    cmp::Ordering,
    collections::{BinaryHeap, HashMap},
    env,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering as AtomicOrdering},
        Arc, Weak,
    },
    time::{Duration, Instant, SystemTime},
};

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use dashmap::DashMap;
use parking_lot::{Mutex, RwLock};
use reqwest::header::HeaderValue as ReqwestHeaderValue;
use serde::Deserialize;
use static_flow_shared::llm_gateway_store::{
    LlmGatewayKeyRecord, LlmGatewayKeyUsageRollupRecord, LlmGatewayStore,
    LlmGatewayUsageEventRecord,
};
use tokio::{
    sync::{mpsc, watch, Notify, RwLock as AsyncRwLock},
    time::MissedTickBehavior,
};

use super::{
    accounts::AccountPool,
    activity::{RequestActivityGuard, RequestActivitySnapshot, RequestActivityTracker},
    types::LlmGatewayRateLimitStatusResponse,
};
use crate::{
    state::LlmGatewayRuntimeConfig,
    upstream_proxy::{
        AccountProxySelection, HttpClientProfile, ResolvedUpstreamProxy, UpstreamProxyRegistry,
    },
};

const CLEANER_TICK_SECONDS: u64 = 1;
const USAGE_EVENT_CHANNEL_CAPACITY: usize = 4_096;

#[derive(Debug, Clone, Default)]
pub(crate) struct UsageEventCountCache {
    pub total_event_count: usize,
    pub provider_event_counts: HashMap<String, usize>,
    pub key_event_counts: HashMap<String, usize>,
}

#[derive(Debug, Clone, Copy)]
struct UsageFlushConfig {
    batch_size: usize,
    flush_interval: Duration,
    max_buffer_bytes: usize,
}

fn usage_flush_config(runtime_config: &LlmGatewayRuntimeConfig) -> UsageFlushConfig {
    UsageFlushConfig {
        batch_size: runtime_config.usage_event_flush_batch_size.max(1) as usize,
        flush_interval: Duration::from_secs(
            runtime_config.usage_event_flush_interval_seconds.max(1),
        ),
        max_buffer_bytes: runtime_config.usage_event_flush_max_buffer_bytes.max(1) as usize,
    }
}

fn estimate_usage_event_bytes(event: &LlmGatewayUsageEventRecord) -> usize {
    event.id.len()
        + event.key_id.len()
        + event.key_name.len()
        + event.provider_type.len()
        + event.account_name.as_deref().map_or(0, str::len)
        + event.request_method.len()
        + event.request_url.len()
        + event.endpoint.len()
        + event.model.as_deref().map_or(0, str::len)
        + event.client_ip.len()
        + event.ip_region.len()
        + event.request_headers_json.len()
        + event.last_message_content.as_deref().map_or(0, str::len)
        + event
            .client_request_body_json
            .as_deref()
            .map_or(0, str::len)
        + event
            .upstream_request_body_json
            .as_deref()
            .map_or(0, str::len)
        + event.full_request_json.as_deref().map_or(0, str::len)
}

/// Long-lived runtime state shared by all gateway handlers.
#[derive(Clone)]
pub struct LlmGatewayRuntimeState {
    pub(crate) store: Arc<LlmGatewayStore>,
    pub(crate) runtime_config: Arc<RwLock<LlmGatewayRuntimeConfig>>,
    pub(crate) auth_source: Arc<CodexAuthSource>,
    pub(crate) account_pool: Arc<AccountPool>,
    pub(crate) upstream_proxy_registry: Arc<UpstreamProxyRegistry>,
    pub(crate) key_cache: Arc<LlmGatewayKeyCache>,
    pub(crate) account_request_scheduler: Arc<CodexAccountRequestScheduler>,
    pub(crate) request_scheduler: Arc<LlmGatewayKeyRequestScheduler>,
    pub(crate) rate_limit_status: Arc<RwLock<LlmGatewayRateLimitStatusResponse>>,
    /// In-memory per-key usage rollups aggregated from usage events.
    /// Rebuilt on startup and incrementally updated on each new event.
    pub(crate) usage_rollups: Arc<RwLock<HashMap<String, LlmGatewayKeyUsageRollupRecord>>>,
    pub(crate) usage_event_counts: Arc<RwLock<UsageEventCountCache>>,
    pub(crate) activity_tracker: Arc<RequestActivityTracker>,
    pub(crate) usage_event_tx: mpsc::Sender<LlmGatewayUsageEventRecord>,
}

impl LlmGatewayRuntimeState {
    /// Construct the shared runtime state used by all LLM gateway requests.
    pub fn new(
        store: Arc<LlmGatewayStore>,
        runtime_config: Arc<RwLock<LlmGatewayRuntimeConfig>>,
        account_pool: Arc<AccountPool>,
        upstream_proxy_registry: Arc<UpstreamProxyRegistry>,
        shutdown_rx: watch::Receiver<bool>,
    ) -> Result<Self> {
        let (usage_event_tx, usage_event_rx) =
            mpsc::channel::<LlmGatewayUsageEventRecord>(USAGE_EVENT_CHANNEL_CAPACITY);
        let refresh_interval_seconds = runtime_config
            .read()
            .codex_status_refresh_max_interval_seconds;
        let usage_event_counts = Arc::new(RwLock::new(UsageEventCountCache::default()));
        spawn_usage_event_flusher(
            store.clone(),
            runtime_config.clone(),
            usage_event_counts.clone(),
            usage_event_rx,
            shutdown_rx.clone(),
        );
        Ok(Self {
            store,
            runtime_config,
            auth_source: Arc::new(CodexAuthSource::new()),
            account_pool,
            upstream_proxy_registry,
            key_cache: Arc::new(LlmGatewayKeyCache::new()),
            account_request_scheduler: CodexAccountRequestScheduler::new(),
            request_scheduler: Arc::new(LlmGatewayKeyRequestScheduler::new()),
            rate_limit_status: Arc::new(RwLock::new(LlmGatewayRateLimitStatusResponse {
                status: "loading".to_string(),
                refresh_interval_seconds,
                last_checked_at: None,
                last_success_at: None,
                source_url: String::new(),
                error_message: None,
                accounts: Vec::new(),
                buckets: Vec::new(),
            })),
            usage_rollups: Arc::new(RwLock::new(HashMap::new())),
            usage_event_counts,
            activity_tracker: Arc::new(RequestActivityTracker::new()),
            usage_event_tx,
        })
    }

    pub(crate) async fn build_upstream_client(
        &self,
        auth_snapshot: &CodexAuthSnapshot,
    ) -> Result<(reqwest::Client, ResolvedUpstreamProxy)> {
        self.upstream_proxy_registry
            .client_for_selection(
                static_flow_shared::llm_gateway_store::LLM_GATEWAY_PROVIDER_CODEX,
                Some(&auth_snapshot.proxy_selection),
                codex_upstream_client_profile(),
            )
            .await
            .context("failed to resolve codex upstream proxy client")
    }

    /// Rebuild the in-memory usage rollups by scanning all usage events.
    /// Called once at startup.
    pub(crate) async fn rebuild_usage_rollups(&self) -> Result<()> {
        let rows = self
            .store
            .aggregate_usage_rollups()
            .await
            .context("failed to aggregate gateway usage events for rollup rebuild")?;
        let key_count = rows.len();
        let rollups = rows
            .into_iter()
            .map(|row| (row.key_id.clone(), row))
            .collect::<HashMap<_, _>>();
        *self.usage_rollups.write() = rollups;
        tracing::info!(key_count, "rebuilt in-memory llm gateway usage rollups from usage events");
        Ok(())
    }

    pub(crate) async fn rebuild_usage_event_counts(&self) -> Result<()> {
        let counts = self
            .store
            .aggregate_usage_event_counts()
            .await
            .context("failed to aggregate gateway usage-event counts")?;
        let key_count = counts.key_event_counts.len();
        let provider_count = counts.provider_event_counts.len();
        *self.usage_event_counts.write() = UsageEventCountCache {
            total_event_count: counts.total_event_count,
            provider_event_counts: counts.provider_event_counts,
            key_event_counts: counts.key_event_counts,
        };
        tracing::info!(
            total_event_count = self.total_usage_event_count(),
            key_count,
            provider_count,
            "rebuilt in-memory llm gateway usage-event counts from usage events"
        );
        Ok(())
    }

    /// Return a copy of `key` with `usage_*` fields replaced by the
    /// in-memory rollup totals.
    pub(crate) async fn overlay_key_usage(&self, key: &LlmGatewayKeyRecord) -> LlmGatewayKeyRecord {
        let rollups = self.usage_rollups.read();
        apply_usage_rollup(key, rollups.get(&key.id))
    }

    /// Batch variant of [`overlay_key_usage`](Self::overlay_key_usage).
    pub(crate) async fn overlay_key_usage_batch(
        &self,
        keys: &[LlmGatewayKeyRecord],
    ) -> Vec<LlmGatewayKeyRecord> {
        let rollups = self.usage_rollups.read();
        keys.iter()
            .map(|key| apply_usage_rollup(key, rollups.get(&key.id)))
            .collect()
    }

    /// Queue one usage event for batched persistence and incrementally update
    /// the in-memory rollup. Returns the key record with refreshed usage
    /// totals.
    pub(crate) async fn append_usage_event(
        &self,
        base_key: &LlmGatewayKeyRecord,
        event: &LlmGatewayUsageEventRecord,
    ) -> Result<LlmGatewayKeyRecord> {
        self.usage_event_tx
            .send(event.clone())
            .await
            .context("failed to enqueue llm gateway usage event")?;
        let updated = {
            let mut rollups = self.usage_rollups.write();
            let rollup = rollups.entry(event.key_id.clone()).or_insert_with(|| {
                LlmGatewayKeyUsageRollupRecord {
                    key_id: event.key_id.clone(),
                    ..LlmGatewayKeyUsageRollupRecord::default()
                }
            });
            apply_event_to_rollup(rollup, event);
            apply_usage_rollup(base_key, Some(rollup))
        };
        Ok(updated)
    }

    pub(crate) fn total_usage_event_count(&self) -> usize {
        self.usage_event_counts.read().total_event_count
    }

    #[cfg(test)]
    pub(crate) fn usage_event_count_for_provider(&self, provider_type: &str) -> usize {
        self.usage_event_counts
            .read()
            .provider_event_counts
            .get(provider_type)
            .copied()
            .unwrap_or(0)
    }

    pub(crate) fn usage_event_count_for_key(&self, key_id: &str) -> usize {
        self.usage_event_counts
            .read()
            .key_event_counts
            .get(key_id)
            .copied()
            .unwrap_or(0)
    }

    pub(crate) fn start_request_activity(&self, key_id: &str) -> RequestActivityGuard {
        self.activity_tracker.start(key_id)
    }

    pub(crate) fn request_activity_snapshot(
        &self,
        key_id: Option<&str>,
    ) -> RequestActivitySnapshot {
        self.activity_tracker.snapshot(key_id)
    }
}

fn spawn_usage_event_flusher(
    store: Arc<LlmGatewayStore>,
    runtime_config: Arc<RwLock<LlmGatewayRuntimeConfig>>,
    usage_event_counts: Arc<RwLock<UsageEventCountCache>>,
    mut rx: mpsc::Receiver<LlmGatewayUsageEventRecord>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    tokio::spawn(async move {
        let initial_config = usage_flush_config(&runtime_config.read());
        let mut buffer = Vec::with_capacity(initial_config.batch_size);
        let mut buffered_bytes = 0usize;
        let mut flush_count: u64 = 0;
        let mut retry_failed_batch_on_timer = false;

        loop {
            let flush_config = {
                let config = runtime_config.read().clone();
                usage_flush_config(&config)
            };
            if retry_failed_batch_on_timer && !buffer.is_empty() {
                tokio::select! {
                    biased;
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            while let Ok(event) = rx.try_recv() {
                                buffered_bytes = buffered_bytes.saturating_add(estimate_usage_event_bytes(&event));
                                buffer.push(event);
                            }
                            let _ = flush_usage_event_buffer(
                                store.as_ref(),
                                usage_event_counts.as_ref(),
                                &mut buffer,
                                &mut buffered_bytes,
                                &mut flush_count,
                                "final usage event flush failed during shutdown",
                            )
                            .await;
                            tracing::info!("llm gateway usage event flusher shutting down (shutdown signal)");
                            return;
                        }
                    }
                    _ = tokio::time::sleep(flush_config.flush_interval) => {
                        retry_failed_batch_on_timer = flush_usage_event_buffer(
                            store.as_ref(),
                            usage_event_counts.as_ref(),
                            &mut buffer,
                            &mut buffered_bytes,
                            &mut flush_count,
                            "usage event retry flush failed",
                        )
                        .await;
                    }
                }
                continue;
            }
            tokio::select! {
                biased;
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        while let Ok(event) = rx.try_recv() {
                            buffered_bytes = buffered_bytes.saturating_add(estimate_usage_event_bytes(&event));
                            buffer.push(event);
                        }
                        let _ = flush_usage_event_buffer(
                            store.as_ref(),
                            usage_event_counts.as_ref(),
                            &mut buffer,
                            &mut buffered_bytes,
                            &mut flush_count,
                            "final usage event flush failed during shutdown",
                        )
                        .await;
                        tracing::info!("llm gateway usage event flusher shutting down (shutdown signal)");
                        return;
                    }
                }
                maybe_event = rx.recv() => {
                    match maybe_event {
                        Some(event) => {
                            buffered_bytes = buffered_bytes.saturating_add(estimate_usage_event_bytes(&event));
                            buffer.push(event);
                            while buffer.len() < flush_config.batch_size
                                && buffered_bytes < flush_config.max_buffer_bytes
                            {
                                match rx.try_recv() {
                                    Ok(event) => {
                                        buffered_bytes = buffered_bytes.saturating_add(
                                            estimate_usage_event_bytes(&event),
                                        );
                                        buffer.push(event);
                                    }
                                    Err(_) => break,
                                }
                            }
                            if buffer.len() >= flush_config.batch_size
                                || buffered_bytes >= flush_config.max_buffer_bytes
                            {
                                retry_failed_batch_on_timer = flush_usage_event_buffer(
                                    store.as_ref(),
                                    usage_event_counts.as_ref(),
                                    &mut buffer,
                                    &mut buffered_bytes,
                                    &mut flush_count,
                                    "usage event batch flush failed",
                                )
                                .await;
                            }
                        },
                        None => {
                            let _ = flush_usage_event_buffer(
                                store.as_ref(),
                                usage_event_counts.as_ref(),
                                &mut buffer,
                                &mut buffered_bytes,
                                &mut flush_count,
                                "final usage event flush failed",
                            )
                            .await;
                            tracing::info!("llm gateway usage event flusher shutting down");
                            return;
                        },
                    }
                }
                _ = tokio::time::sleep(flush_config.flush_interval) => {
                    if !buffer.is_empty() {
                        retry_failed_batch_on_timer = flush_usage_event_buffer(
                            store.as_ref(),
                            usage_event_counts.as_ref(),
                            &mut buffer,
                            &mut buffered_bytes,
                            &mut flush_count,
                            "usage event timed flush failed",
                        )
                        .await;
                    }
                }
            }
        }
    });
}

async fn flush_usage_event_buffer(
    store: &LlmGatewayStore,
    usage_event_counts: &RwLock<UsageEventCountCache>,
    buffer: &mut Vec<LlmGatewayUsageEventRecord>,
    buffered_bytes: &mut usize,
    flush_count: &mut u64,
    error_message: &'static str,
) -> bool {
    if buffer.is_empty() {
        return false;
    }

    let batch = std::mem::take(buffer);
    *buffered_bytes = 0;
    let count = batch.len();
    match store.append_usage_events(&batch).await {
        Ok(()) => {
            apply_persisted_usage_event_counts(usage_event_counts, &batch);
            *flush_count += 1;
            tracing::debug!("flushed {count} llm gateway usage events (flush #{flush_count})");
            false
        },
        Err(err) => {
            tracing::error!(count, "{}: {err:#}", error_message);
            *buffered_bytes = batch.iter().map(estimate_usage_event_bytes).sum();
            *buffer = batch;
            true
        },
    }
}

fn apply_persisted_usage_event_counts(
    usage_event_counts: &RwLock<UsageEventCountCache>,
    batch: &[LlmGatewayUsageEventRecord],
) {
    let mut counts = usage_event_counts.write();
    for event in batch {
        counts.total_event_count = counts.total_event_count.saturating_add(1);
        *counts
            .provider_event_counts
            .entry(event.provider_type.clone())
            .or_default() += 1;
        *counts
            .key_event_counts
            .entry(event.key_id.clone())
            .or_default() += 1;
    }
}

/// Stamp a key record's `usage_*` fields with the aggregated rollup values.
fn apply_usage_rollup(
    key: &LlmGatewayKeyRecord,
    rollup: Option<&LlmGatewayKeyUsageRollupRecord>,
) -> LlmGatewayKeyRecord {
    let mut effective = key.clone();
    let rollup = rollup
        .cloned()
        .unwrap_or_else(|| LlmGatewayKeyUsageRollupRecord {
            key_id: key.id.clone(),
            ..LlmGatewayKeyUsageRollupRecord::default()
        });
    effective.usage_input_uncached_tokens = rollup.input_uncached_tokens;
    effective.usage_input_cached_tokens = rollup.input_cached_tokens;
    effective.usage_output_tokens = rollup.output_tokens;
    effective.usage_billable_tokens = rollup.billable_tokens;
    effective.usage_credit_total = rollup.credit_total;
    effective.usage_credit_missing_events = rollup.credit_missing_events;
    effective.last_used_at = rollup.last_used_at;
    effective
}

/// Incrementally fold a single usage event into an existing rollup record.
fn apply_event_to_rollup(
    rollup: &mut LlmGatewayKeyUsageRollupRecord,
    event: &LlmGatewayUsageEventRecord,
) {
    rollup.input_uncached_tokens = rollup
        .input_uncached_tokens
        .saturating_add(event.input_uncached_tokens);
    rollup.input_cached_tokens = rollup
        .input_cached_tokens
        .saturating_add(event.input_cached_tokens);
    rollup.output_tokens = rollup.output_tokens.saturating_add(event.output_tokens);
    rollup.billable_tokens = rollup.billable_tokens.saturating_add(event.billable_tokens);
    rollup.credit_total += event.credit_usage.unwrap_or(0.0);
    if event.credit_usage_missing {
        rollup.credit_missing_events = rollup.credit_missing_events.saturating_add(1);
    }
    rollup.last_used_at = Some(
        rollup
            .last_used_at
            .map(|current| current.max(event.created_at))
            .unwrap_or(event.created_at),
    );
}

#[derive(Debug, Clone)]
struct AccountRequestState {
    in_flight: usize,
    next_start_at: Instant,
}

#[derive(Debug, Clone)]
pub(crate) struct CodexAccountRequestLimitRejection {
    pub reason: &'static str,
    pub in_flight: usize,
    pub max_concurrency: Option<u64>,
    pub min_start_interval_ms: Option<u64>,
    pub wait: Option<Duration>,
    pub elapsed_since_last_start_ms: Option<u64>,
}

#[derive(Debug, Clone)]
pub(crate) struct CodexAccountRequestScheduler {
    states: Arc<Mutex<HashMap<String, AccountRequestState>>>,
    notify: Arc<Notify>,
}

#[derive(Debug)]
pub(crate) struct CodexAccountRequestLease {
    scheduler: Option<Arc<CodexAccountRequestScheduler>>,
    account_name: String,
    released: bool,
    waited_ms: u64,
}

impl CodexAccountRequestScheduler {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            states: Arc::new(Mutex::new(HashMap::new())),
            notify: Arc::new(Notify::new()),
        })
    }

    pub(crate) fn try_acquire(
        self: &Arc<Self>,
        account_name: &str,
        max_concurrency: Option<u64>,
        min_start_interval_ms: Option<u64>,
        queued_at: Instant,
    ) -> Result<CodexAccountRequestLease, CodexAccountRequestLimitRejection> {
        let max_concurrency = max_concurrency.filter(|value| *value > 0);
        if max_concurrency.is_none() && min_start_interval_ms.is_none() {
            return Ok(CodexAccountRequestLease {
                scheduler: None,
                account_name: account_name.to_string(),
                released: false,
                waited_ms: queued_at.elapsed().as_millis() as u64,
            });
        }

        let now = Instant::now();
        let mut states = self.states.lock();
        let state = states
            .entry(account_name.to_string())
            .or_insert_with(|| AccountRequestState {
                in_flight: 0,
                next_start_at: now,
            });

        if let Some(limit) = max_concurrency {
            if state.in_flight >= limit as usize {
                return Err(CodexAccountRequestLimitRejection {
                    reason: "local_concurrency_limit",
                    in_flight: state.in_flight,
                    max_concurrency,
                    min_start_interval_ms,
                    wait: None,
                    elapsed_since_last_start_ms: None,
                });
            }
        }

        if let Some(interval_ms) = min_start_interval_ms {
            if now < state.next_start_at {
                let wait = state.next_start_at.saturating_duration_since(now);
                let elapsed_since_last_start_ms =
                    interval_ms.saturating_sub(wait.as_millis() as u64);
                return Err(CodexAccountRequestLimitRejection {
                    reason: "local_start_interval",
                    in_flight: state.in_flight,
                    max_concurrency,
                    min_start_interval_ms,
                    wait: Some(wait),
                    elapsed_since_last_start_ms: Some(elapsed_since_last_start_ms),
                });
            }
        }

        state.in_flight += 1;
        state.next_start_at = min_start_interval_ms
            .map(|value| now + Duration::from_millis(value))
            .unwrap_or(now);

        Ok(CodexAccountRequestLease {
            scheduler: Some(self.clone()),
            account_name: account_name.to_string(),
            released: false,
            waited_ms: queued_at.elapsed().as_millis() as u64,
        })
    }

    pub(crate) async fn wait_for_available(&self, wait: Option<Duration>) {
        match wait {
            Some(duration) => {
                tokio::select! {
                    _ = self.notify.notified() => {},
                    _ = tokio::time::sleep(duration) => {},
                }
            },
            None => self.notify.notified().await,
        }
    }

    pub(crate) fn notify_config_changed(&self) {
        self.notify.notify_waiters();
    }

    fn release(&self, account_name: &str) {
        let now = Instant::now();
        let mut states = self.states.lock();
        let remove_entry = if let Some(state) = states.get_mut(account_name) {
            if state.in_flight > 0 {
                state.in_flight -= 1;
            }
            state.in_flight == 0 && state.next_start_at <= now
        } else {
            false
        };
        if remove_entry {
            states.remove(account_name);
        }
        self.notify.notify_waiters();
    }
}

impl CodexAccountRequestLease {
    pub(crate) fn untracked(account_name: impl Into<String>) -> Self {
        Self {
            scheduler: None,
            account_name: account_name.into(),
            released: false,
            waited_ms: 0,
        }
    }

    pub(crate) fn waited_ms(&self) -> u64 {
        self.waited_ms
    }
}

impl Drop for CodexAccountRequestLease {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        if let Some(scheduler) = self.scheduler.as_ref() {
            scheduler.release(&self.account_name);
        }
    }
}

#[derive(Debug, Clone)]
struct KeyRequestState {
    in_flight: usize,
    next_start_at: Instant,
}

#[derive(Debug, Clone)]
pub(crate) struct CodexKeyRequestLimitRejection {
    pub reason: &'static str,
    pub in_flight: usize,
    pub max_concurrency: Option<u64>,
    pub min_start_interval_ms: Option<u64>,
    pub wait: Option<Duration>,
    pub elapsed_since_last_start_ms: Option<u64>,
}

#[derive(Debug, Clone)]
pub(crate) struct LlmGatewayKeyRequestScheduler {
    states: Arc<Mutex<HashMap<String, KeyRequestState>>>,
}

#[derive(Debug)]
pub(crate) struct CodexKeyRequestLease {
    scheduler: Option<Arc<LlmGatewayKeyRequestScheduler>>,
    key_id: String,
    released: bool,
}

impl LlmGatewayKeyRequestScheduler {
    fn new() -> Self {
        Self {
            states: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub(crate) fn try_acquire(
        self: &Arc<Self>,
        key: &LlmGatewayKeyRecord,
    ) -> Result<CodexKeyRequestLease, CodexKeyRequestLimitRejection> {
        let max_concurrency = key.request_max_concurrency.filter(|value| *value > 0);
        let min_start_interval_ms = key.request_min_start_interval_ms;
        if max_concurrency.is_none() && min_start_interval_ms.is_none() {
            return Ok(CodexKeyRequestLease {
                scheduler: None,
                key_id: key.id.clone(),
                released: false,
            });
        }

        let now = Instant::now();
        let mut states = self.states.lock();
        let state = states
            .entry(key.id.clone())
            .or_insert_with(|| KeyRequestState {
                in_flight: 0,
                next_start_at: now,
            });

        if let Some(limit) = max_concurrency {
            if state.in_flight >= limit as usize {
                return Err(CodexKeyRequestLimitRejection {
                    reason: "local_concurrency_limit",
                    in_flight: state.in_flight,
                    max_concurrency,
                    min_start_interval_ms,
                    wait: None,
                    elapsed_since_last_start_ms: None,
                });
            }
        }

        if let Some(interval_ms) = min_start_interval_ms {
            if now < state.next_start_at {
                let wait = state.next_start_at.saturating_duration_since(now);
                let elapsed_since_last_start_ms =
                    interval_ms.saturating_sub(wait.as_millis() as u64);
                return Err(CodexKeyRequestLimitRejection {
                    reason: "local_start_interval",
                    in_flight: state.in_flight,
                    max_concurrency,
                    min_start_interval_ms,
                    wait: Some(wait),
                    elapsed_since_last_start_ms: Some(elapsed_since_last_start_ms),
                });
            }
        }

        state.in_flight += 1;
        state.next_start_at = min_start_interval_ms
            .map(|value| now + Duration::from_millis(value))
            .unwrap_or(now);

        Ok(CodexKeyRequestLease {
            scheduler: Some(self.clone()),
            key_id: key.id.clone(),
            released: false,
        })
    }

    fn release(&self, key_id: &str) {
        let now = Instant::now();
        let mut states = self.states.lock();
        let remove_entry = if let Some(state) = states.get_mut(key_id) {
            if state.in_flight > 0 {
                state.in_flight -= 1;
            }
            state.in_flight == 0 && state.next_start_at <= now
        } else {
            false
        };
        if remove_entry {
            states.remove(key_id);
        }
    }
}

impl Drop for CodexKeyRequestLease {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        if let Some(scheduler) = self.scheduler.as_ref() {
            scheduler.release(&self.key_id);
        }
    }
}

/// In-memory snapshot of the upstream Codex login state.
#[derive(Debug, Clone)]
pub(crate) struct CodexAuthSnapshot {
    pub access_token: String,
    pub account_id: Option<String>,
    pub is_fedramp_account: bool,
    pub proxy_selection: AccountProxySelection,
    modified_at: Option<SystemTime>,
}

impl CodexAuthSnapshot {
    /// Build a snapshot without filesystem mtime (used by the account pool).
    pub(crate) fn from_tokens(access_token: String, account_id: Option<String>) -> Self {
        Self::from_tokens_with_proxy(access_token, account_id, AccountProxySelection::default())
    }

    pub(crate) fn from_tokens_with_proxy(
        access_token: String,
        account_id: Option<String>,
        proxy_selection: AccountProxySelection,
    ) -> Self {
        Self::from_tokens_with_proxy_and_fedramp(access_token, account_id, proxy_selection, false)
    }

    pub(crate) fn from_tokens_with_proxy_and_fedramp(
        access_token: String,
        account_id: Option<String>,
        proxy_selection: AccountProxySelection,
        is_fedramp_account: bool,
    ) -> Self {
        Self {
            access_token,
            account_id,
            is_fedramp_account,
            proxy_selection: proxy_selection.canonicalize(),
            modified_at: None,
        }
    }
}

/// Minimal shape read from `~/.codex/auth.json`.
#[derive(Debug, Clone, Deserialize)]
struct CodexAuthFile {
    #[serde(default)]
    tokens: CodexAuthTokens,
}

/// Token fields needed to authenticate against the upstream Codex backend.
#[derive(Debug, Clone, Default, Deserialize)]
struct CodexAuthTokens {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    account_id: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
}

/// File-backed auth source with mtime-based hot reload.
pub(crate) struct CodexAuthSource {
    path: PathBuf,
    cached: AsyncRwLock<Option<CodexAuthSnapshot>>,
}

impl CodexAuthSource {
    /// Create a new auth source bound to the resolved Codex auth.json path.
    pub(crate) fn new() -> Self {
        Self {
            path: codex_auth_path(),
            cached: AsyncRwLock::new(None),
        }
    }

    /// Return the current cached-or-reloaded upstream auth snapshot.
    pub(crate) async fn current(&self) -> Result<CodexAuthSnapshot> {
        self.load_if_needed(false).await
    }

    /// Force a reload from disk after an upstream authentication failure.
    pub(crate) async fn force_reload(&self) -> Result<CodexAuthSnapshot> {
        self.load_if_needed(true).await
    }

    /// Reload auth.json only when the caller requests it or the file changed.
    async fn load_if_needed(&self, force: bool) -> Result<CodexAuthSnapshot> {
        let metadata = tokio::fs::metadata(&self.path)
            .await
            .with_context(|| format!("failed to stat `{}`", self.path.display()))?;
        let modified_at = metadata.modified().ok();

        if !force {
            if let Some(cached) = self.cached.read().await.clone() {
                if modified_matches(cached.modified_at, modified_at) {
                    return Ok(cached);
                }
            }
        }

        let raw = tokio::fs::read_to_string(&self.path)
            .await
            .with_context(|| format!("failed to read `{}`", self.path.display()))?;
        let parsed: CodexAuthFile = serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse `{}`", self.path.display()))?;
        let access_token = parsed
            .tokens
            .access_token
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow!("`{}` missing tokens.access_token", self.path.display()))?;
        let snapshot = CodexAuthSnapshot {
            access_token,
            account_id: parsed
                .tokens
                .account_id
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
            is_fedramp_account: parsed
                .tokens
                .id_token
                .as_deref()
                .is_some_and(id_token_is_fedramp_account),
            proxy_selection: AccountProxySelection::default(),
            modified_at,
        };
        *self.cached.write().await = Some(snapshot.clone());
        Ok(snapshot)
    }
}

pub(crate) fn id_token_is_fedramp_account(id_token: &str) -> bool {
    let Some(payload_b64) = id_token.split('.').nth(1) else {
        return false;
    };
    let Some(decoded) = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload_b64)
        .ok()
        .or_else(|| {
            base64::engine::general_purpose::URL_SAFE
                .decode(payload_b64)
                .ok()
        })
    else {
        return false;
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&decoded) else {
        return false;
    };
    value
        .get("https://api.openai.com/auth")
        .and_then(|auth| auth.get("chatgpt_account_is_fedramp"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// Compare two optional mtimes without treating missing metadata as an error.
fn modified_matches(left: Option<SystemTime>, right: Option<SystemTime>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left == right,
        (None, None) => true,
        _ => false,
    }
}

/// Resolve the upstream Codex auth.json path, honoring explicit overrides
/// first.
fn codex_auth_path() -> PathBuf {
    if let Ok(path) = env::var("CODEX_AUTH_JSON_PATH") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    let home = env::var("HOME").unwrap_or_else(|_| "/home/ts_user".to_string());
    PathBuf::from(home).join(".codex").join("auth.json")
}

pub(crate) const fn codex_upstream_client_profile() -> HttpClientProfile {
    HttpClientProfile::new(None, 32, 90)
}

/// Renewable in-memory cache for validated API keys.
pub(crate) struct LlmGatewayKeyCache {
    index: Arc<DashMap<String, Weak<CachedKeyLease>>>,
    queue: Arc<Mutex<BinaryHeap<ExpiringLease>>>,
    next_seq: AtomicU64,
    cleanup_tx: mpsc::UnboundedSender<String>,
}

impl LlmGatewayKeyCache {
    pub(crate) fn new() -> Self {
        let index = Arc::new(DashMap::new());
        let queue = Arc::new(Mutex::new(BinaryHeap::new()));
        let (cleanup_tx, mut cleanup_rx) = mpsc::unbounded_channel::<String>();
        let cache = Self {
            index: index.clone(),
            queue: queue.clone(),
            next_seq: AtomicU64::new(1),
            cleanup_tx: cleanup_tx.clone(),
        };
        let cleanup_index = index;
        tokio::spawn(async move {
            while let Some(key_hash) = cleanup_rx.recv().await {
                let should_remove = cleanup_index
                    .get(&key_hash)
                    .map(|entry| entry.value().upgrade().is_none())
                    .unwrap_or(false);
                if should_remove {
                    cleanup_index.remove(&key_hash);
                }
            }
        });

        let cleaner_queue = queue;
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(CLEANER_TICK_SECONDS));
            ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                let mut expired = Vec::<ExpiringLease>::new();
                {
                    let mut queue = cleaner_queue.lock();
                    let now = Instant::now();
                    while queue.peek().is_some_and(|entry| entry.expires_at <= now) {
                        if let Some(entry) = queue.pop() {
                            expired.push(entry);
                        }
                    }
                }
                drop(expired);
            }
        });
        cache
    }

    pub(crate) fn get(&self, key_hash: &str) -> Option<Arc<CachedKeyLease>> {
        let lease = self
            .index
            .get(key_hash)
            .and_then(|entry| entry.value().upgrade());
        if let Some(lease) = lease {
            if lease.expires_at > Instant::now() {
                return Some(lease);
            }
        }
        self.index.remove(key_hash);
        None
    }

    pub(crate) fn renew(&self, record: LlmGatewayKeyRecord, ttl: Duration) -> Arc<CachedKeyLease> {
        let key_hash = record.key_hash.clone();
        let expires_at = Instant::now() + ttl;
        let lease = Arc::new(CachedKeyLease {
            key_hash: key_hash.clone(),
            record,
            expires_at,
            cleanup_tx: self.cleanup_tx.clone(),
        });
        self.index.insert(key_hash, Arc::downgrade(&lease));
        let seq = self.next_seq.fetch_add(1, AtomicOrdering::Relaxed);
        self.queue.lock().push(ExpiringLease {
            expires_at,
            seq,
            _lease: lease.clone(),
        });
        lease
    }

    pub(crate) fn invalidate(&self, key_hash: &str) {
        self.index.remove(key_hash);
    }
}

/// One renewable cache lease tracked by weak references and expiry timers.
pub(crate) struct CachedKeyLease {
    key_hash: String,
    pub record: LlmGatewayKeyRecord,
    expires_at: Instant,
    cleanup_tx: mpsc::UnboundedSender<String>,
}

impl Drop for CachedKeyLease {
    fn drop(&mut self) {
        let _ = self.cleanup_tx.send(self.key_hash.clone());
    }
}

/// Heap entry used by the background cleaner to retire expired cache leases.
struct ExpiringLease {
    expires_at: Instant,
    seq: u64,
    _lease: Arc<CachedKeyLease>,
}

impl PartialEq for ExpiringLease {
    fn eq(&self, other: &Self) -> bool {
        self.expires_at == other.expires_at && self.seq == other.seq
    }
}

impl Eq for ExpiringLease {}

impl PartialOrd for ExpiringLease {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ExpiringLease {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .expires_at
            .cmp(&self.expires_at)
            .then_with(|| other.seq.cmp(&self.seq))
    }
}

/// Read the live auth-cache TTL from the shared gateway runtime state.
pub(crate) async fn gateway_auth_cache_ttl(gateway: &LlmGatewayRuntimeState) -> u64 {
    gateway.runtime_config.read().auth_cache_ttl_seconds
}

/// Build a reqwest bearer Authorization header value.
pub(crate) fn bearer_header(token: &str) -> Result<ReqwestHeaderValue> {
    ReqwestHeaderValue::from_str(&format!("Bearer {token}"))
        .context("failed to build bearer header")
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use static_flow_shared::llm_gateway_store::{
        now_ms, LlmGatewayStore, LlmGatewayUsageEventRecord, LLM_GATEWAY_KEY_STATUS_ACTIVE,
        LLM_GATEWAY_PROTOCOL_OPENAI, LLM_GATEWAY_PROVIDER_CODEX, LLM_GATEWAY_USAGE_EVENTS_TABLE,
    };
    use tokio::{sync::watch, time::Duration};

    use super::*;
    use crate::{state::LlmGatewayRuntimeConfig, upstream_proxy::UpstreamProxyRegistry};

    fn id_token_with_fedramp_claim(value: bool) -> String {
        let payload = serde_json::json!({
            "https://api.openai.com/auth": {
                "chatgpt_account_is_fedramp": value
            }
        })
        .to_string();
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.as_bytes());
        format!("header.{encoded}.sig")
    }

    fn sample_key() -> LlmGatewayKeyRecord {
        LlmGatewayKeyRecord {
            id: "key-1".to_string(),
            name: "test-key".to_string(),
            secret: "sfk_test".to_string(),
            key_hash: "hash".to_string(),
            status: LLM_GATEWAY_KEY_STATUS_ACTIVE.to_string(),
            provider_type: LLM_GATEWAY_PROVIDER_CODEX.to_string(),
            protocol_family: LLM_GATEWAY_PROTOCOL_OPENAI.to_string(),
            public_visible: false,
            quota_billable_limit: 1_000_000,
            usage_input_uncached_tokens: 0,
            usage_input_cached_tokens: 0,
            usage_output_tokens: 0,
            usage_billable_tokens: 0,
            usage_credit_total: 0.0,
            usage_credit_missing_events: 0,
            last_used_at: None,
            created_at: 0,
            updated_at: 0,
            route_strategy: None,
            account_group_id: None,
            fixed_account_name: None,
            auto_account_names: None,
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

    #[test]
    fn id_token_is_fedramp_account_reads_codex_auth_claim() {
        assert!(id_token_is_fedramp_account(&id_token_with_fedramp_claim(true)));
        assert!(!id_token_is_fedramp_account(&id_token_with_fedramp_claim(false)));
        assert!(!id_token_is_fedramp_account("not-a-jwt"));
    }

    fn temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("staticflow-{prefix}-{nanos}"))
    }

    #[test]
    fn codex_request_scheduler_allows_unlimited_keys_without_tracking() {
        let scheduler = Arc::new(LlmGatewayKeyRequestScheduler::new());
        let lease = scheduler
            .try_acquire(&sample_key())
            .expect("unlimited keys should acquire immediately");
        assert_eq!(lease.key_id, "key-1");
        assert!(lease.scheduler.is_none());
    }

    #[test]
    fn codex_request_scheduler_enforces_concurrency_and_start_interval() {
        let scheduler = Arc::new(LlmGatewayKeyRequestScheduler::new());
        let mut key = sample_key();
        key.request_max_concurrency = Some(1);
        key.request_min_start_interval_ms = Some(250);

        let first_lease = scheduler
            .try_acquire(&key)
            .expect("first request should acquire");

        let concurrency_rejection = scheduler
            .try_acquire(&key)
            .expect_err("second in-flight request should be rejected");
        assert_eq!(concurrency_rejection.reason, "local_concurrency_limit");
        assert_eq!(concurrency_rejection.in_flight, 1);
        assert_eq!(concurrency_rejection.max_concurrency, Some(1));
        assert_eq!(concurrency_rejection.min_start_interval_ms, Some(250));

        drop(first_lease);

        let pacing_rejection = scheduler
            .try_acquire(&key)
            .expect_err("request restart should honor min start interval");
        assert_eq!(pacing_rejection.reason, "local_start_interval");
        assert_eq!(pacing_rejection.in_flight, 0);
        assert_eq!(pacing_rejection.max_concurrency, Some(1));
        assert_eq!(pacing_rejection.min_start_interval_ms, Some(250));
        assert!(pacing_rejection.wait.is_some());
        assert!(pacing_rejection.elapsed_since_last_start_ms.is_some());
    }

    #[test]
    fn codex_account_request_scheduler_allows_unlimited_accounts_without_tracking() {
        let scheduler = CodexAccountRequestScheduler::new();
        let queued_at = Instant::now();
        let lease = scheduler
            .try_acquire("alpha", None, None, queued_at)
            .expect("unlimited account should acquire immediately");
        assert_eq!(lease.account_name, "alpha");
        assert!(lease.scheduler.is_none());
    }

    #[test]
    fn codex_account_request_scheduler_enforces_per_account_concurrency_and_spacing() {
        let scheduler = CodexAccountRequestScheduler::new();
        let queued_at = Instant::now();
        let first = scheduler
            .try_acquire("alpha", Some(1), Some(200), queued_at)
            .expect("first request should acquire");

        let blocked = scheduler
            .try_acquire("alpha", Some(1), Some(200), queued_at)
            .expect_err("second in-flight request should be rejected");
        assert_eq!(blocked.reason, "local_concurrency_limit");
        assert_eq!(blocked.in_flight, 1);
        assert_eq!(blocked.max_concurrency, Some(1));
        assert_eq!(blocked.min_start_interval_ms, Some(200));

        drop(first);

        let pacing = scheduler
            .try_acquire("alpha", Some(1), Some(200), queued_at)
            .expect_err("account restart should honor min start interval");
        assert_eq!(pacing.reason, "local_start_interval");
        assert_eq!(pacing.in_flight, 0);
        assert_eq!(pacing.max_concurrency, Some(1));
        assert_eq!(pacing.min_start_interval_ms, Some(200));
        assert!(pacing.wait.is_some());
        assert!(pacing.elapsed_since_last_start_ms.is_some());
    }

    #[test]
    fn codex_account_request_scheduler_is_isolated_per_account() {
        let scheduler = CodexAccountRequestScheduler::new();
        let queued_at = Instant::now();
        let first = scheduler
            .try_acquire("alpha", Some(1), Some(0), queued_at)
            .expect("alpha should acquire");
        scheduler
            .try_acquire("beta", Some(1), Some(0), queued_at)
            .expect("beta should remain available");
        drop(first);
    }

    #[tokio::test]
    async fn usage_events_flush_buffer_on_shutdown_and_update_rollup_immediately() {
        let dir = temp_dir("llm-gateway-usage-runtime");
        let auths_dir = temp_dir("llm-gateway-auths");
        fs::create_dir_all(&auths_dir).expect("create auth dir");

        let store = Arc::new(
            LlmGatewayStore::connect(&dir.to_string_lossy())
                .await
                .expect("connect llm gateway store"),
        );
        let runtime_config = Arc::new(RwLock::new(LlmGatewayRuntimeConfig::default()));
        let account_pool = Arc::new(AccountPool::new(auths_dir.clone()));
        let upstream_proxy_registry = Arc::new(
            UpstreamProxyRegistry::new(store.clone())
                .await
                .expect("create upstream proxy registry"),
        );
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let runtime = LlmGatewayRuntimeState::new(
            store.clone(),
            runtime_config,
            account_pool,
            upstream_proxy_registry,
            shutdown_rx,
        )
        .expect("create runtime");
        let key = sample_key();
        let event = LlmGatewayUsageEventRecord {
            id: "evt-1".to_string(),
            key_id: key.id.clone(),
            key_name: key.name.clone(),
            provider_type: key.provider_type.clone(),
            account_name: Some("test-account".to_string()),
            request_method: "POST".to_string(),
            request_url: "/api/llm-gateway/v1/responses".to_string(),
            latency_ms: 10,
            endpoint: "/v1/responses".to_string(),
            model: Some("gpt-5".to_string()),
            status_code: 200,
            input_uncached_tokens: 2,
            input_cached_tokens: 0,
            output_tokens: 1,
            billable_tokens: 7,
            usage_missing: false,
            credit_usage: None,
            credit_usage_missing: false,
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: "{}".to_string(),
            last_message_content: Some("hello".to_string()),
            client_request_body_json: None,
            upstream_request_body_json: None,
            full_request_json: None,
            created_at: now_ms(),
        };

        let updated = runtime
            .append_usage_event(&key, &event)
            .await
            .expect("append usage event");
        assert_eq!(updated.usage_billable_tokens, 7);
        assert_eq!(
            store
                .count_usage_events(Some(&key.id))
                .await
                .expect("count queued usage events before flush"),
            0
        );
        assert_eq!(runtime.total_usage_event_count(), 0);
        assert_eq!(runtime.usage_event_count_for_provider(&key.provider_type), 0);
        assert_eq!(runtime.usage_event_count_for_key(&key.id), 0);

        shutdown_tx.send(true).expect("send shutdown");
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if store
                    .count_usage_events(Some(&key.id))
                    .await
                    .expect("count usage events after flush")
                    == 1
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("usage event flushed on shutdown");
        assert_eq!(runtime.total_usage_event_count(), 1);
        assert_eq!(runtime.usage_event_count_for_provider(&key.provider_type), 1);
        assert_eq!(runtime.usage_event_count_for_key(&key.id), 1);

        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&auths_dir);
    }

    #[tokio::test]
    async fn append_usage_event_keeps_persisted_counts_stable_until_flush() {
        let dir = temp_dir("llm-gateway-usage-counts");
        let auths_dir = temp_dir("llm-gateway-auths-counts");
        fs::create_dir_all(&auths_dir).expect("create auth dir");

        let store = Arc::new(
            LlmGatewayStore::connect(&dir.to_string_lossy())
                .await
                .expect("connect llm gateway store"),
        );
        let runtime_config = Arc::new(RwLock::new(LlmGatewayRuntimeConfig {
            usage_event_flush_batch_size: 256,
            usage_event_flush_interval_seconds: 60,
            usage_event_flush_max_buffer_bytes: 8 * 1024 * 1024,
            ..LlmGatewayRuntimeConfig::default()
        }));
        let account_pool = Arc::new(AccountPool::new(auths_dir.clone()));
        let upstream_proxy_registry = Arc::new(
            UpstreamProxyRegistry::new(store.clone())
                .await
                .expect("create upstream proxy registry"),
        );
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);
        let runtime = LlmGatewayRuntimeState::new(
            store,
            runtime_config,
            account_pool,
            upstream_proxy_registry,
            shutdown_rx,
        )
        .expect("create runtime");
        let key = sample_key();
        let event = LlmGatewayUsageEventRecord {
            id: "evt-1".to_string(),
            key_id: key.id.clone(),
            key_name: key.name.clone(),
            provider_type: key.provider_type.clone(),
            account_name: Some("test-account".to_string()),
            request_method: "POST".to_string(),
            request_url: "/api/llm-gateway/v1/responses".to_string(),
            latency_ms: 10,
            endpoint: "/v1/responses".to_string(),
            model: Some("gpt-5".to_string()),
            status_code: 200,
            input_uncached_tokens: 2,
            input_cached_tokens: 0,
            output_tokens: 1,
            billable_tokens: 7,
            usage_missing: false,
            credit_usage: None,
            credit_usage_missing: false,
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: "{}".to_string(),
            last_message_content: Some("hello".to_string()),
            client_request_body_json: None,
            upstream_request_body_json: None,
            full_request_json: None,
            created_at: now_ms(),
        };

        runtime
            .append_usage_event(&key, &event)
            .await
            .expect("append usage event");

        assert_eq!(runtime.total_usage_event_count(), 0);
        assert_eq!(runtime.usage_event_count_for_provider(&key.provider_type), 0);
        assert_eq!(runtime.usage_event_count_for_key(&key.id), 0);

        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&auths_dir);
    }

    #[tokio::test]
    async fn usage_event_flush_failure_stops_drain_until_timer_retry() {
        let dir = temp_dir("llm-gateway-usage-flush-failure");
        let auths_dir = temp_dir("llm-gateway-auths-flush-failure");
        fs::create_dir_all(&auths_dir).expect("create auth dir");

        let store = Arc::new(
            LlmGatewayStore::connect(&dir.to_string_lossy())
                .await
                .expect("connect llm gateway store"),
        );
        let runtime_config = Arc::new(RwLock::new(crate::state::LlmGatewayRuntimeConfig {
            usage_event_flush_batch_size: 1,
            usage_event_flush_interval_seconds: 3600,
            usage_event_flush_max_buffer_bytes: 8 * 1024 * 1024,
            ..crate::state::LlmGatewayRuntimeConfig::default()
        }));
        let account_pool = Arc::new(AccountPool::new(auths_dir.clone()));
        let upstream_proxy_registry = Arc::new(
            UpstreamProxyRegistry::new(store.clone())
                .await
                .expect("create upstream proxy registry"),
        );
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);
        let runtime = LlmGatewayRuntimeState::new(
            store,
            runtime_config,
            account_pool,
            upstream_proxy_registry,
            shutdown_rx,
        )
        .expect("create runtime");
        let key = sample_key();

        fs::remove_dir_all(dir.join(format!("{LLM_GATEWAY_USAGE_EVENTS_TABLE}.lance")))
            .expect("remove usage-events table to force append failures");

        let make_event = |index: usize| LlmGatewayUsageEventRecord {
            id: format!("evt-{index}"),
            key_id: key.id.clone(),
            key_name: key.name.clone(),
            provider_type: key.provider_type.clone(),
            account_name: Some("test-account".to_string()),
            request_method: "POST".to_string(),
            request_url: "/api/llm-gateway/v1/responses".to_string(),
            latency_ms: 10,
            endpoint: "/v1/responses".to_string(),
            model: Some("gpt-5".to_string()),
            status_code: 200,
            input_uncached_tokens: 2,
            input_cached_tokens: 0,
            output_tokens: 1,
            billable_tokens: 7,
            usage_missing: false,
            credit_usage: None,
            credit_usage_missing: false,
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: "{}".to_string(),
            last_message_content: Some("hello".to_string()),
            client_request_body_json: None,
            upstream_request_body_json: None,
            full_request_json: None,
            created_at: now_ms() + index as i64,
        };

        runtime
            .append_usage_event(&key, &make_event(0))
            .await
            .expect("enqueue first usage event");
        tokio::time::sleep(Duration::from_millis(150)).await;

        let mut accepted_after_failure = 0usize;
        for index in 1..=(USAGE_EVENT_CHANNEL_CAPACITY * 2) {
            let send_result = tokio::time::timeout(
                Duration::from_millis(10),
                runtime.append_usage_event(&key, &make_event(index)),
            )
            .await;
            if send_result.is_err() {
                break;
            }
            accepted_after_failure += 1;
        }

        assert!(
            accepted_after_failure <= USAGE_EVENT_CHANNEL_CAPACITY + 32,
            "receiver kept draining after failure; accepted_after_failure={accepted_after_failure}"
        );

        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&auths_dir);
    }

    #[tokio::test]
    async fn usage_events_flush_when_buffer_bytes_reach_limit() {
        let dir = temp_dir("llm-gateway-usage-byte-flush");
        let auths_dir = temp_dir("llm-gateway-auths-byte-flush");
        fs::create_dir_all(&auths_dir).expect("create auth dir");

        let store = Arc::new(
            LlmGatewayStore::connect(&dir.to_string_lossy())
                .await
                .expect("connect llm gateway store"),
        );
        let runtime_config = Arc::new(RwLock::new(crate::state::LlmGatewayRuntimeConfig {
            usage_event_flush_batch_size: 256,
            usage_event_flush_interval_seconds: 60,
            usage_event_flush_max_buffer_bytes: 64,
            ..crate::state::LlmGatewayRuntimeConfig::default()
        }));
        let account_pool = Arc::new(AccountPool::new(auths_dir.clone()));
        let upstream_proxy_registry = Arc::new(
            UpstreamProxyRegistry::new(store.clone())
                .await
                .expect("create upstream proxy registry"),
        );
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);
        let runtime = LlmGatewayRuntimeState::new(
            store.clone(),
            runtime_config,
            account_pool,
            upstream_proxy_registry,
            shutdown_rx,
        )
        .expect("create runtime");
        let key = sample_key();

        let first = LlmGatewayUsageEventRecord {
            id: "evt-1".to_string(),
            key_id: key.id.clone(),
            key_name: key.name.clone(),
            provider_type: key.provider_type.clone(),
            account_name: Some("test-account".to_string()),
            request_method: "POST".to_string(),
            request_url: "/api/llm-gateway/v1/responses".to_string(),
            latency_ms: 10,
            endpoint: "/v1/responses".to_string(),
            model: Some("gpt-5".to_string()),
            status_code: 200,
            input_uncached_tokens: 2,
            input_cached_tokens: 0,
            output_tokens: 1,
            billable_tokens: 7,
            usage_missing: false,
            credit_usage: None,
            credit_usage_missing: false,
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: "{}".to_string(),
            last_message_content: Some("1234567890".to_string()),
            client_request_body_json: None,
            upstream_request_body_json: None,
            full_request_json: None,
            created_at: now_ms(),
        };
        let second = LlmGatewayUsageEventRecord {
            id: "evt-2".to_string(),
            key_id: key.id.clone(),
            key_name: key.name.clone(),
            provider_type: key.provider_type.clone(),
            account_name: Some("test-account".to_string()),
            request_method: "POST".to_string(),
            request_url: "/api/llm-gateway/v1/responses".to_string(),
            latency_ms: 11,
            endpoint: "/v1/responses".to_string(),
            model: Some("gpt-5".to_string()),
            status_code: 200,
            input_uncached_tokens: 2,
            input_cached_tokens: 0,
            output_tokens: 1,
            billable_tokens: 7,
            usage_missing: false,
            credit_usage: None,
            credit_usage_missing: false,
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: "{}".to_string(),
            last_message_content: Some("abcdefghij".to_string()),
            client_request_body_json: None,
            upstream_request_body_json: None,
            full_request_json: None,
            created_at: now_ms() + 1,
        };

        runtime
            .append_usage_event(&key, &first)
            .await
            .expect("append first");
        runtime
            .append_usage_event(&key, &second)
            .await
            .expect("append second");

        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if store
                    .count_usage_events(Some(&key.id))
                    .await
                    .expect("count")
                    == 2
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("byte threshold should flush buffered events");

        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&auths_dir);
    }
}
