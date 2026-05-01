//! Runtime startup validation for the standalone LLM access service.

use std::{
    sync::{Arc, RwLock},
    time::Duration,
};

use anyhow::{anyhow, Context};
#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
use async_trait::async_trait;
use llm_access_core::store::{
    AdminAccountGroupStore, AdminCodexAccountStore, AdminConfigStore, AdminKeyStore,
    AdminKiroAccountStore, AdminProxyStore, AdminReviewQueueStore, ControlStore,
    EmptyAdminAccountGroupStore, EmptyAdminCodexAccountStore, EmptyAdminConfigStore,
    EmptyAdminKeyStore, EmptyAdminKiroAccountStore, EmptyAdminProxyStore,
    EmptyAdminReviewQueueStore, EmptyProviderRouteStore, EmptyPublicAccessStore,
    EmptyPublicCommunityStore, EmptyPublicStatusStore, EmptyPublicSubmissionStore,
    EmptyPublicUsageStore, EmptyUsageAnalyticsStore, ProviderRouteStore, PublicAccessStore,
    PublicCommunityStore, PublicStatusStore, PublicSubmissionStore, PublicUsageStore,
    UsageAnalyticsStore,
};
#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
use llm_access_core::store::{AdminRuntimeConfig, UsageEventSink};
#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
use llm_access_core::usage::UsageEvent;
#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
use llm_access_store::duckdb::DuckDbUsageRepository;
use llm_access_store::repository::SqliteControlRepository;
#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
use tokio::{
    sync::{mpsc, watch, Mutex},
    task::JoinHandle,
    time,
};

use crate::config::StorageConfig;

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
const USAGE_EVENT_CHANNEL_CAPACITY: usize = 4_096;

/// Runtime dependencies shared by provider routes.
#[derive(Clone)]
pub struct LlmAccessRuntime {
    control_store: Arc<dyn ControlStore>,
    provider_route_store: Arc<dyn ProviderRouteStore>,
    admin_config_store: Arc<dyn AdminConfigStore>,
    admin_key_store: Arc<dyn AdminKeyStore>,
    admin_account_group_store: Arc<dyn AdminAccountGroupStore>,
    admin_proxy_store: Arc<dyn AdminProxyStore>,
    admin_codex_account_store: Arc<dyn AdminCodexAccountStore>,
    admin_kiro_account_store: Arc<dyn AdminKiroAccountStore>,
    admin_review_queue_store: Arc<dyn AdminReviewQueueStore>,
    public_access_store: Arc<dyn PublicAccessStore>,
    public_community_store: Arc<dyn PublicCommunityStore>,
    public_usage_store: Arc<dyn PublicUsageStore>,
    usage_analytics_store: Arc<dyn UsageAnalyticsStore>,
    public_submission_store: Arc<dyn PublicSubmissionStore>,
    public_status_store: Arc<dyn PublicStatusStore>,
    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    usage_event_flusher: Option<Arc<UsageEventFlusherHandle>>,
}

/// Runtime dependency bundle used to keep construction explicit as the
/// standalone service grows.
struct LlmAccessStores {
    control_store: Arc<dyn ControlStore>,
    provider_route_store: Arc<dyn ProviderRouteStore>,
    admin_config_store: Arc<dyn AdminConfigStore>,
    admin_key_store: Arc<dyn AdminKeyStore>,
    admin_account_group_store: Arc<dyn AdminAccountGroupStore>,
    admin_proxy_store: Arc<dyn AdminProxyStore>,
    admin_codex_account_store: Arc<dyn AdminCodexAccountStore>,
    admin_kiro_account_store: Arc<dyn AdminKiroAccountStore>,
    admin_review_queue_store: Arc<dyn AdminReviewQueueStore>,
    public_access_store: Arc<dyn PublicAccessStore>,
    public_community_store: Arc<dyn PublicCommunityStore>,
    public_usage_store: Arc<dyn PublicUsageStore>,
    usage_analytics_store: Arc<dyn UsageAnalyticsStore>,
    public_submission_store: Arc<dyn PublicSubmissionStore>,
    public_status_store: Arc<dyn PublicStatusStore>,
    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    usage_event_flusher: Option<Arc<UsageEventFlusherHandle>>,
}

impl LlmAccessRuntime {
    /// Create runtime dependencies from explicit storage adapters.
    pub fn new(control_store: Arc<dyn ControlStore>) -> Self {
        Self::with_stores(LlmAccessStores {
            control_store,
            provider_route_store: Arc::new(EmptyProviderRouteStore),
            admin_config_store: Arc::new(EmptyAdminConfigStore),
            admin_key_store: Arc::new(EmptyAdminKeyStore),
            admin_account_group_store: Arc::new(EmptyAdminAccountGroupStore),
            admin_proxy_store: Arc::new(EmptyAdminProxyStore),
            admin_codex_account_store: Arc::new(EmptyAdminCodexAccountStore),
            admin_kiro_account_store: Arc::new(EmptyAdminKiroAccountStore),
            admin_review_queue_store: Arc::new(EmptyAdminReviewQueueStore),
            public_access_store: Arc::new(EmptyPublicAccessStore),
            public_community_store: Arc::new(EmptyPublicCommunityStore),
            public_usage_store: Arc::new(EmptyPublicUsageStore),
            usage_analytics_store: Arc::new(EmptyUsageAnalyticsStore),
            public_submission_store: Arc::new(EmptyPublicSubmissionStore),
            public_status_store: Arc::new(EmptyPublicStatusStore),
            #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
            usage_event_flusher: None,
        })
    }

    /// Create runtime dependencies from explicit storage adapters.
    fn with_stores(stores: LlmAccessStores) -> Self {
        Self {
            control_store: stores.control_store,
            provider_route_store: stores.provider_route_store,
            admin_config_store: stores.admin_config_store,
            admin_key_store: stores.admin_key_store,
            admin_account_group_store: stores.admin_account_group_store,
            admin_proxy_store: stores.admin_proxy_store,
            admin_codex_account_store: stores.admin_codex_account_store,
            admin_kiro_account_store: stores.admin_kiro_account_store,
            admin_review_queue_store: stores.admin_review_queue_store,
            public_access_store: stores.public_access_store,
            public_community_store: stores.public_community_store,
            public_usage_store: stores.public_usage_store,
            usage_analytics_store: stores.usage_analytics_store,
            public_submission_store: stores.public_submission_store,
            public_status_store: stores.public_status_store,
            #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
            usage_event_flusher: stores.usage_event_flusher,
        }
    }

    /// Open runtime dependencies from configured persistent storage.
    pub async fn from_storage_config(config: &StorageConfig) -> anyhow::Result<Self> {
        validate_state_root(config)?;
        let repository = Arc::new(SqliteControlRepository::open_path(&config.sqlite_control)?);
        #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
        let runtime_config = Arc::new(RwLock::new(repository.get_admin_runtime_config().await?));
        #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
        let duckdb_usage = Arc::new(DuckDbUsageRepository::open_path(&config.duckdb)?);
        #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
        rebuild_key_usage_rollups_from_duckdb(&repository, &duckdb_usage).await?;
        #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
        let (batched_usage_sink, usage_event_flusher) =
            BatchedUsageEventSink::new(duckdb_usage.clone(), runtime_config.clone());
        #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
        let usage_event_sink: Arc<dyn UsageEventSink> = Arc::new(batched_usage_sink);
        #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
        let control_store: Arc<dyn ControlStore> = Arc::new(RecordingControlStore {
            control_store: repository.clone(),
            usage_event_sink,
        });
        #[cfg(not(any(feature = "duckdb-runtime", feature = "duckdb-bundled")))]
        let control_store: Arc<dyn ControlStore> = repository.clone();
        let provider_route_store: Arc<dyn ProviderRouteStore> = repository.clone();
        #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
        let admin_config_store: Arc<dyn AdminConfigStore> = Arc::new(RecordingAdminConfigStore {
            admin_config_store: repository.clone(),
            runtime_config: runtime_config.clone(),
        });
        #[cfg(not(any(feature = "duckdb-runtime", feature = "duckdb-bundled")))]
        let admin_config_store: Arc<dyn AdminConfigStore> = repository.clone();
        let admin_key_store: Arc<dyn AdminKeyStore> = repository.clone();
        let admin_account_group_store: Arc<dyn AdminAccountGroupStore> = repository.clone();
        let admin_proxy_store: Arc<dyn AdminProxyStore> = repository.clone();
        let admin_codex_account_store: Arc<dyn AdminCodexAccountStore> = repository.clone();
        let admin_kiro_account_store: Arc<dyn AdminKiroAccountStore> = repository.clone();
        let admin_review_queue_store: Arc<dyn AdminReviewQueueStore> = repository.clone();
        let public_access_store: Arc<dyn PublicAccessStore> = repository.clone();
        let public_community_store: Arc<dyn PublicCommunityStore> = repository.clone();
        let public_usage_store: Arc<dyn PublicUsageStore> = repository.clone();
        #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
        let usage_analytics_store: Arc<dyn UsageAnalyticsStore> = duckdb_usage;
        #[cfg(not(any(feature = "duckdb-runtime", feature = "duckdb-bundled")))]
        let usage_analytics_store: Arc<dyn UsageAnalyticsStore> =
            Arc::new(EmptyUsageAnalyticsStore);
        let public_submission_store: Arc<dyn PublicSubmissionStore> = repository.clone();
        let public_status_store: Arc<dyn PublicStatusStore> = repository;
        Ok(Self::with_stores(LlmAccessStores {
            control_store,
            provider_route_store,
            admin_config_store,
            admin_key_store,
            admin_account_group_store,
            admin_proxy_store,
            admin_codex_account_store,
            admin_kiro_account_store,
            admin_review_queue_store,
            public_access_store,
            public_community_store,
            public_usage_store,
            usage_analytics_store,
            public_submission_store,
            public_status_store,
            #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
            usage_event_flusher: Some(usage_event_flusher),
        }))
    }

    /// Shared control store used by request handlers.
    pub fn control_store(&self) -> Arc<dyn ControlStore> {
        Arc::clone(&self.control_store)
    }

    /// Provider route store used by data-plane dispatch.
    pub fn provider_route_store(&self) -> Arc<dyn ProviderRouteStore> {
        Arc::clone(&self.provider_route_store)
    }

    /// Admin config store used by local admin endpoints.
    pub fn admin_config_store(&self) -> Arc<dyn AdminConfigStore> {
        Arc::clone(&self.admin_config_store)
    }

    /// Admin key store used by local admin endpoints.
    pub fn admin_key_store(&self) -> Arc<dyn AdminKeyStore> {
        Arc::clone(&self.admin_key_store)
    }

    /// Admin account-group store used by local admin endpoints.
    pub fn admin_account_group_store(&self) -> Arc<dyn AdminAccountGroupStore> {
        Arc::clone(&self.admin_account_group_store)
    }

    /// Admin proxy store used by local admin endpoints.
    pub fn admin_proxy_store(&self) -> Arc<dyn AdminProxyStore> {
        Arc::clone(&self.admin_proxy_store)
    }

    /// Admin Codex account store used by local admin endpoints.
    pub fn admin_codex_account_store(&self) -> Arc<dyn AdminCodexAccountStore> {
        Arc::clone(&self.admin_codex_account_store)
    }

    /// Admin Kiro account store used by local admin endpoints.
    pub fn admin_kiro_account_store(&self) -> Arc<dyn AdminKiroAccountStore> {
        Arc::clone(&self.admin_kiro_account_store)
    }

    /// Admin review queue store used by local admin endpoints.
    pub fn admin_review_queue_store(&self) -> Arc<dyn AdminReviewQueueStore> {
        Arc::clone(&self.admin_review_queue_store)
    }

    /// Public access store used by unauthenticated public endpoints.
    pub fn public_access_store(&self) -> Arc<dyn PublicAccessStore> {
        Arc::clone(&self.public_access_store)
    }

    /// Public community store used by unauthenticated public endpoints.
    pub fn public_community_store(&self) -> Arc<dyn PublicCommunityStore> {
        Arc::clone(&self.public_community_store)
    }

    /// Public usage store used by unauthenticated public endpoints.
    pub fn public_usage_store(&self) -> Arc<dyn PublicUsageStore> {
        Arc::clone(&self.public_usage_store)
    }

    /// Usage analytics store used by admin and public usage views.
    pub fn usage_analytics_store(&self) -> Arc<dyn UsageAnalyticsStore> {
        Arc::clone(&self.usage_analytics_store)
    }

    /// Public submission store used by unauthenticated public endpoints.
    pub fn public_submission_store(&self) -> Arc<dyn PublicSubmissionStore> {
        Arc::clone(&self.public_submission_store)
    }

    /// Public status store used by unauthenticated public endpoints.
    pub fn public_status_store(&self) -> Arc<dyn PublicStatusStore> {
        Arc::clone(&self.public_status_store)
    }

    /// Flush queued usage events before shutdown.
    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    pub async fn shutdown_usage_events(&self) {
        if let Some(flusher) = &self.usage_event_flusher {
            flusher.shutdown().await;
        }
    }

    /// No-op when DuckDB usage persistence is not compiled in.
    #[cfg(not(any(feature = "duckdb-runtime", feature = "duckdb-bundled")))]
    pub async fn shutdown_usage_events(&self) {}
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
#[derive(Debug, Clone, Copy)]
struct UsageFlushConfig {
    batch_size: usize,
    flush_interval: Duration,
    max_buffer_bytes: usize,
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
fn usage_flush_config(runtime_config: &AdminRuntimeConfig) -> UsageFlushConfig {
    UsageFlushConfig {
        batch_size: runtime_config.usage_event_flush_batch_size.max(1) as usize,
        flush_interval: Duration::from_secs(
            runtime_config.usage_event_flush_interval_seconds.max(1),
        ),
        max_buffer_bytes: runtime_config.usage_event_flush_max_buffer_bytes.max(1) as usize,
    }
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
async fn rebuild_key_usage_rollups_from_duckdb(
    repository: &Arc<SqliteControlRepository>,
    duckdb_usage: &Arc<DuckDbUsageRepository>,
) -> anyhow::Result<()> {
    let rollups = duckdb_usage.key_usage_rollups().await?;
    let rollup_count = rollups.len();
    repository
        .replace_key_usage_rollups(rollups, now_ms())
        .await?;
    tracing::info!(rollup_count, "rebuilt SQLite key usage rollups from DuckDB usage events");
    Ok(())
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
fn estimate_usage_event_bytes(event: &UsageEvent) -> usize {
    event.event_id.len()
        + event.provider_type.as_storage_str().len()
        + event.protocol_family.as_storage_str().len()
        + event.key_id.len()
        + event.key_name.len()
        + event.account_name.as_deref().map_or(0, str::len)
        + event
            .account_group_id_at_event
            .as_deref()
            .map_or(0, str::len)
        + event
            .route_strategy_at_event
            .map_or(0, |strategy| strategy.as_storage_str().len())
        + event.request_method.len()
        + event.request_url.len()
        + event.endpoint.len()
        + event.model.as_deref().map_or(0, str::len)
        + event.mapped_model.as_deref().map_or(0, str::len)
        + event
            .routing_diagnostics_json
            .as_deref()
            .map_or(0, str::len)
        + event.credit_usage.as_deref().map_or(0, str::len)
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

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
struct BatchedUsageEventSink {
    tx: mpsc::Sender<UsageEvent>,
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
impl BatchedUsageEventSink {
    fn new(
        inner: Arc<dyn UsageEventSink>,
        runtime_config: Arc<RwLock<AdminRuntimeConfig>>,
    ) -> (Self, Arc<UsageEventFlusherHandle>) {
        let (tx, rx) = mpsc::channel::<UsageEvent>(USAGE_EVENT_CHANNEL_CAPACITY);
        let handle = spawn_usage_event_flusher(inner, runtime_config, rx);
        (
            Self {
                tx,
            },
            handle,
        )
    }
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
#[async_trait]
impl UsageEventSink for BatchedUsageEventSink {
    async fn append_usage_event(&self, event: &UsageEvent) -> anyhow::Result<()> {
        self.tx
            .send(event.clone())
            .await
            .context("failed to enqueue llm access usage event")
    }

    async fn append_usage_events(&self, events: &[UsageEvent]) -> anyhow::Result<()> {
        for event in events {
            self.append_usage_event(event).await?;
        }
        Ok(())
    }
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
fn spawn_usage_event_flusher(
    inner: Arc<dyn UsageEventSink>,
    runtime_config: Arc<RwLock<AdminRuntimeConfig>>,
    mut rx: mpsc::Receiver<UsageEvent>,
) -> Arc<UsageEventFlusherHandle> {
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    let join = tokio::spawn(async move {
        let initial_config = {
            let config = runtime_config
                .read()
                .expect("llm access runtime config lock poisoned");
            usage_flush_config(&config)
        };
        let mut buffer = Vec::with_capacity(initial_config.batch_size);
        let mut buffered_bytes = 0usize;
        let mut flush_count: u64 = 0;
        let mut retry_failed_batch_on_timer = false;

        loop {
            let flush_config = {
                let config = runtime_config
                    .read()
                    .expect("llm access runtime config lock poisoned");
                usage_flush_config(&config)
            };

            if retry_failed_batch_on_timer && !buffer.is_empty() {
                tokio::select! {
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            while let Ok(event) = rx.try_recv() {
                                buffered_bytes = buffered_bytes.saturating_add(estimate_usage_event_bytes(&event));
                                buffer.push(event);
                            }
                            let _ = flush_usage_event_buffer(
                                inner.as_ref(),
                                &mut buffer,
                                &mut buffered_bytes,
                                &mut flush_count,
                                "final usage event flush failed during shutdown",
                            )
                            .await;
                            tracing::info!("llm access usage event flusher shutting down (shutdown signal)");
                            return;
                        }
                    }
                    _ = time::sleep(flush_config.flush_interval) => {
                        retry_failed_batch_on_timer = flush_usage_event_buffer(
                            inner.as_ref(),
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
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        while let Ok(event) = rx.try_recv() {
                            buffered_bytes = buffered_bytes.saturating_add(estimate_usage_event_bytes(&event));
                            buffer.push(event);
                        }
                        let _ = flush_usage_event_buffer(
                            inner.as_ref(),
                            &mut buffer,
                            &mut buffered_bytes,
                            &mut flush_count,
                            "final usage event flush failed during shutdown",
                        )
                        .await;
                        tracing::info!("llm access usage event flusher shutting down (shutdown signal)");
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
                                    },
                                    Err(_) => break,
                                }
                            }
                            if buffer.len() >= flush_config.batch_size
                                || buffered_bytes >= flush_config.max_buffer_bytes
                            {
                                retry_failed_batch_on_timer = flush_usage_event_buffer(
                                    inner.as_ref(),
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
                                inner.as_ref(),
                                &mut buffer,
                                &mut buffered_bytes,
                                &mut flush_count,
                                "final usage event flush failed",
                            )
                            .await;
                            tracing::info!("llm access usage event flusher shutting down");
                            return;
                        },
                    }
                }
                _ = time::sleep(flush_config.flush_interval) => {
                    if !buffer.is_empty() {
                        retry_failed_batch_on_timer = flush_usage_event_buffer(
                            inner.as_ref(),
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
    Arc::new(UsageEventFlusherHandle {
        shutdown_tx,
        join: Mutex::new(Some(join)),
    })
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
struct UsageEventFlusherHandle {
    shutdown_tx: watch::Sender<bool>,
    join: Mutex<Option<JoinHandle<()>>>,
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
impl UsageEventFlusherHandle {
    async fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
        if let Some(join) = self.join.lock().await.take() {
            if let Err(err) = join.await {
                tracing::error!(
                    "llm access usage event flusher task failed during shutdown: {err}"
                );
            }
        }
    }
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
async fn flush_usage_event_buffer(
    inner: &dyn UsageEventSink,
    buffer: &mut Vec<UsageEvent>,
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
    match inner.append_usage_events(&batch).await {
        Ok(()) => {
            *flush_count += 1;
            tracing::debug!("flushed {count} llm access usage events (flush #{flush_count})");
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

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
struct RecordingAdminConfigStore {
    admin_config_store: Arc<dyn AdminConfigStore>,
    runtime_config: Arc<RwLock<AdminRuntimeConfig>>,
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
#[async_trait]
impl AdminConfigStore for RecordingAdminConfigStore {
    async fn get_admin_runtime_config(&self) -> anyhow::Result<AdminRuntimeConfig> {
        Ok(self
            .runtime_config
            .read()
            .expect("llm access runtime config lock poisoned")
            .clone())
    }

    async fn update_admin_runtime_config(
        &self,
        config: AdminRuntimeConfig,
    ) -> anyhow::Result<AdminRuntimeConfig> {
        let updated = self
            .admin_config_store
            .update_admin_runtime_config(config)
            .await?;
        *self
            .runtime_config
            .write()
            .expect("llm access runtime config lock poisoned") = updated.clone();
        Ok(updated)
    }
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
struct RecordingControlStore {
    control_store: Arc<dyn ControlStore>,
    usage_event_sink: Arc<dyn UsageEventSink>,
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
#[async_trait]
impl ControlStore for RecordingControlStore {
    async fn authenticate_bearer_secret(
        &self,
        secret: &str,
    ) -> anyhow::Result<Option<llm_access_core::store::AuthenticatedKey>> {
        self.control_store.authenticate_bearer_secret(secret).await
    }

    async fn apply_usage_rollup(&self, event: &UsageEvent) -> anyhow::Result<()> {
        self.control_store.apply_usage_rollup(event).await?;
        self.usage_event_sink.append_usage_event(event).await
    }
}

/// Validate and prepare the persistent state root before storage is opened.
pub fn validate_state_root(config: &StorageConfig) -> anyhow::Result<()> {
    let metadata = std::fs::metadata(&config.state_root).with_context(|| {
        format!("state root `{}` is not accessible", config.state_root.display())
    })?;
    if !metadata.is_dir() {
        return Err(anyhow!("state root `{}` is not a directory", config.state_root.display()));
    }
    for dir in [&config.kiro_auths_dir, &config.codex_auths_dir, &config.logs_dir] {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("failed to create `{}`", dir.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    use std::{
        sync::{Arc, RwLock},
        time::Duration,
    };

    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    use llm_access_core::{
        provider::{ProtocolFamily, ProviderType},
        store::{AdminRuntimeConfig, UsageEventSink},
        usage::{UsageEvent, UsageTiming},
    };
    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    use tokio::sync::Mutex;

    fn temp_storage_config(name: &str) -> crate::config::StorageConfig {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("llm-access-{name}-{}-{unique}", std::process::id()));
        crate::config::StorageConfig {
            state_root: root.clone(),
            sqlite_control: root.join("control/llm-access.sqlite3"),
            duckdb: root.join("analytics/usage.duckdb"),
            kiro_auths_dir: root.join("auths/kiro"),
            codex_auths_dir: root.join("auths/codex"),
            logs_dir: root.join("logs"),
        }
    }

    #[test]
    fn validate_state_root_creates_expected_subdirectories() {
        let config = temp_storage_config("state-root");
        let root = config.state_root.clone();
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create root");

        super::validate_state_root(&config).expect("validate root");

        assert!(config.kiro_auths_dir.is_dir());
        assert!(config.codex_auths_dir.is_dir());
        assert!(config.logs_dir.is_dir());
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    #[tokio::test]
    async fn opens_runtime_control_store_from_sqlite_path() {
        let config = temp_storage_config("runtime-open");
        let root = config.state_root.clone();
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create root");
        llm_access_store::initialize_sqlite_target_path(&config.sqlite_control)
            .expect("initialize sqlite");

        let runtime = super::LlmAccessRuntime::from_storage_config(&config)
            .await
            .expect("open runtime storage");
        let key = runtime
            .control_store()
            .authenticate_bearer_secret("missing")
            .await
            .expect("query store");

        assert!(key.is_none());
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    #[derive(Default)]
    struct RecordingUsageEventSink {
        batches: Mutex<Vec<Vec<String>>>,
    }

    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    #[async_trait::async_trait]
    impl UsageEventSink for RecordingUsageEventSink {
        async fn append_usage_event(&self, event: &UsageEvent) -> anyhow::Result<()> {
            self.append_usage_events(std::slice::from_ref(event)).await
        }

        async fn append_usage_events(&self, events: &[UsageEvent]) -> anyhow::Result<()> {
            self.batches
                .lock()
                .await
                .push(events.iter().map(|event| event.event_id.clone()).collect());
            Ok(())
        }
    }

    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    fn sample_usage_event(event_id: &str) -> UsageEvent {
        UsageEvent {
            event_id: event_id.to_string(),
            created_at_ms: 1_700_000_000_000,
            provider_type: ProviderType::Kiro,
            protocol_family: ProtocolFamily::Anthropic,
            key_id: "key-runtime".to_string(),
            key_name: "runtime".to_string(),
            account_name: Some("account".to_string()),
            account_group_id_at_event: Some("group".to_string()),
            route_strategy_at_event: None,
            request_method: "POST".to_string(),
            request_url: "/cc/v1/messages".to_string(),
            endpoint: "/cc/v1/messages".to_string(),
            model: Some("claude-sonnet-4-5".to_string()),
            mapped_model: Some("claude-sonnet-4-5".to_string()),
            status_code: 200,
            request_body_bytes: Some(128),
            quota_failover_count: 0,
            routing_diagnostics_json: None,
            input_uncached_tokens: 10,
            input_cached_tokens: 0,
            output_tokens: 2,
            billable_tokens: 12,
            credit_usage: None,
            usage_missing: false,
            credit_usage_missing: false,
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: "{}".to_string(),
            last_message_content: Some("hello".to_string()),
            client_request_body_json: None,
            upstream_request_body_json: None,
            full_request_json: None,
            timing: UsageTiming {
                latency_ms: Some(20),
                ..UsageTiming::default()
            },
        }
    }

    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    async fn wait_for_recorded_batches(sink: &RecordingUsageEventSink, expected: usize) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if sink.batches.lock().await.len() >= expected {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("usage event batch was not flushed");
    }

    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    #[tokio::test]
    async fn batched_usage_sink_flushes_when_batch_size_reached() {
        let inner = Arc::new(RecordingUsageEventSink::default());
        let runtime_config = Arc::new(RwLock::new(AdminRuntimeConfig {
            usage_event_flush_batch_size: 2,
            usage_event_flush_interval_seconds: 3600,
            usage_event_flush_max_buffer_bytes: 8 * 1024 * 1024,
            ..AdminRuntimeConfig::default()
        }));
        let (sink, _handle) = super::BatchedUsageEventSink::new(inner.clone(), runtime_config);

        sink.append_usage_event(&sample_usage_event("evt-1"))
            .await
            .expect("enqueue first event");
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(inner.batches.lock().await.is_empty());

        sink.append_usage_event(&sample_usage_event("evt-2"))
            .await
            .expect("enqueue second event");
        wait_for_recorded_batches(&inner, 1).await;

        let batches = inner.batches.lock().await;
        assert_eq!(batches.as_slice(), &[vec!["evt-1".to_string(), "evt-2".to_string()]]);
    }

    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    #[tokio::test]
    async fn batched_usage_sink_flushes_remaining_events_when_sender_closes() {
        let inner = Arc::new(RecordingUsageEventSink::default());
        let runtime_config = Arc::new(RwLock::new(AdminRuntimeConfig {
            usage_event_flush_batch_size: 256,
            usage_event_flush_interval_seconds: 3600,
            usage_event_flush_max_buffer_bytes: 8 * 1024 * 1024,
            ..AdminRuntimeConfig::default()
        }));
        let (sink, _handle) = super::BatchedUsageEventSink::new(inner.clone(), runtime_config);

        sink.append_usage_event(&sample_usage_event("evt-1"))
            .await
            .expect("enqueue event");
        drop(sink);
        wait_for_recorded_batches(&inner, 1).await;

        let batches = inner.batches.lock().await;
        assert_eq!(batches.as_slice(), &[vec!["evt-1".to_string()]]);
    }

    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    #[tokio::test]
    async fn batched_usage_sink_flushes_remaining_events_on_shutdown_signal() {
        let inner = Arc::new(RecordingUsageEventSink::default());
        let runtime_config = Arc::new(RwLock::new(AdminRuntimeConfig {
            usage_event_flush_batch_size: 256,
            usage_event_flush_interval_seconds: 3600,
            usage_event_flush_max_buffer_bytes: 8 * 1024 * 1024,
            ..AdminRuntimeConfig::default()
        }));
        let (sink, handle) = super::BatchedUsageEventSink::new(inner.clone(), runtime_config);

        sink.append_usage_event(&sample_usage_event("evt-1"))
            .await
            .expect("enqueue event");
        handle.shutdown().await;

        let batches = inner.batches.lock().await;
        assert_eq!(batches.as_slice(), &[vec!["evt-1".to_string()]]);
    }
}
