//! Runtime startup validation for the standalone LLM access service.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, RwLock},
    time::{Duration, Instant},
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
    EmptyPublicUsageStore, ProviderRouteStore, PublicAccessStore, PublicCommunityStore,
    PublicStatusStore, PublicSubmissionStore, PublicUsageStore,
};
#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
use llm_access_core::store::{
    AdminKey, AdminKeyPatch, AdminKeysPage, AdminPageRequest, AdminRuntimeConfig, AuthenticatedKey,
    KeyUsageRollupDelta, NewAdminKey, PublicAccessKey, PublicUsageLookupKey, UsageEventSink,
    UsageRollupBatch, UsageRollupBatchSink, UsageRollupDigestMismatch,
};
#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
use llm_access_core::usage::UsageEvent;
use llm_access_store::postgres::{PostgresControlRepository, ProxyConfigScope};
#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
use tokio::{
    sync::{mpsc, watch, Mutex},
    task::JoinHandle,
    time,
};

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
use crate::rollup_backlog::UsageRollupBacklog;
#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
use crate::usage_journal::JournalUsageEventSink;
use crate::{
    config::{resolve_request_cache_config, StorageConfig},
    geoip::GeoIpResolver,
    kiro_latency::KiroLatencyRanker,
};

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
const USAGE_EVENT_CHANNEL_CAPACITY: usize = 1_024;
#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
const USAGE_EVENT_ROLLUP_MAX_ATTEMPTS: usize = 3;
#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
const USAGE_EVENT_ROLLUP_RETRY_BACKOFF_MULTIPLIER: u32 = 3;
#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
const USAGE_ROLLUP_BATCH_MARKER_RETENTION: Duration = Duration::from_secs(90 * 24 * 60 * 60);
#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
const USAGE_ROLLUP_BATCH_MARKER_PRUNE_INTERVAL: Duration = Duration::from_secs(60 * 60);
#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
const USAGE_EVENT_LOG_SAMPLE_LIMIT: usize = 8;

/// Runtime dependencies shared by provider routes.
#[derive(Clone)]
pub struct LlmAccessRuntime {
    control_store: Arc<dyn ControlStore>,
    cluster_state: Option<Arc<crate::cluster::ClusterRuntimeState>>,
    geoip: GeoIpResolver,
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
    public_submission_store: Arc<dyn PublicSubmissionStore>,
    public_status_store: Arc<dyn PublicStatusStore>,
    email_notifier: Option<Arc<crate::email::EmailNotifier>>,
    kiro_latency_ranker: Arc<KiroLatencyRanker>,
    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    usage_journal_sink: Option<Arc<JournalUsageEventSink>>,
    usage_journal_dir: Option<PathBuf>,
    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    usage_event_flusher: Option<Arc<UsageEventFlusherHandle>>,
}

/// Runtime dependency bundle used to keep construction explicit as the
/// standalone service grows.
struct LlmAccessStores {
    control_store: Arc<dyn ControlStore>,
    cluster_state: Option<Arc<crate::cluster::ClusterRuntimeState>>,
    geoip: GeoIpResolver,
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
    public_submission_store: Arc<dyn PublicSubmissionStore>,
    public_status_store: Arc<dyn PublicStatusStore>,
    email_notifier: Option<Arc<crate::email::EmailNotifier>>,
    kiro_latency_ranker: Arc<KiroLatencyRanker>,
    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    usage_journal_sink: Option<Arc<JournalUsageEventSink>>,
    usage_journal_dir: Option<PathBuf>,
    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    usage_event_flusher: Option<Arc<UsageEventFlusherHandle>>,
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
trait RuntimeRepository:
    ControlStore
    + ProviderRouteStore
    + AdminConfigStore
    + AdminKeyStore
    + AdminAccountGroupStore
    + AdminProxyStore
    + AdminCodexAccountStore
    + AdminKiroAccountStore
    + AdminReviewQueueStore
    + PublicAccessStore
    + PublicCommunityStore
    + PublicUsageStore
    + PublicSubmissionStore
    + PublicStatusStore
    + UsageRollupBatchSink
    + Send
    + Sync
{
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
impl<T> RuntimeRepository for T where
    T: ControlStore
        + ProviderRouteStore
        + AdminConfigStore
        + AdminKeyStore
        + AdminAccountGroupStore
        + AdminProxyStore
        + AdminCodexAccountStore
        + AdminKiroAccountStore
        + AdminReviewQueueStore
        + PublicAccessStore
        + PublicCommunityStore
        + PublicUsageStore
        + PublicSubmissionStore
        + PublicStatusStore
        + UsageRollupBatchSink
        + Send
        + Sync
{
}

#[cfg(not(any(feature = "duckdb-runtime", feature = "duckdb-bundled")))]
trait RuntimeRepository:
    ControlStore
    + ProviderRouteStore
    + AdminConfigStore
    + AdminKeyStore
    + AdminAccountGroupStore
    + AdminProxyStore
    + AdminCodexAccountStore
    + AdminKiroAccountStore
    + AdminReviewQueueStore
    + PublicAccessStore
    + PublicCommunityStore
    + PublicUsageStore
    + PublicSubmissionStore
    + PublicStatusStore
    + Send
    + Sync
{
}

#[cfg(not(any(feature = "duckdb-runtime", feature = "duckdb-bundled")))]
impl<T> RuntimeRepository for T where
    T: ControlStore
        + ProviderRouteStore
        + AdminConfigStore
        + AdminKeyStore
        + AdminAccountGroupStore
        + AdminProxyStore
        + AdminCodexAccountStore
        + AdminKiroAccountStore
        + AdminReviewQueueStore
        + PublicAccessStore
        + PublicCommunityStore
        + PublicUsageStore
        + PublicSubmissionStore
        + PublicStatusStore
        + Send
        + Sync
{
}

impl LlmAccessRuntime {
    /// Create runtime dependencies from explicit storage adapters.
    pub fn new(control_store: Arc<dyn ControlStore>) -> Self {
        Self::with_stores(LlmAccessStores {
            control_store,
            cluster_state: None,
            geoip: GeoIpResolver::disabled(),
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
            public_submission_store: Arc::new(EmptyPublicSubmissionStore),
            public_status_store: Arc::new(EmptyPublicStatusStore),
            email_notifier: None,
            kiro_latency_ranker: Arc::new(KiroLatencyRanker::default()),
            #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
            usage_journal_sink: None,
            usage_journal_dir: None,
            #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
            usage_event_flusher: None,
        })
    }

    /// Create runtime dependencies from explicit storage adapters.
    fn with_stores(stores: LlmAccessStores) -> Self {
        Self {
            control_store: stores.control_store,
            cluster_state: stores.cluster_state,
            geoip: stores.geoip,
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
            public_submission_store: stores.public_submission_store,
            public_status_store: stores.public_status_store,
            email_notifier: stores.email_notifier,
            kiro_latency_ranker: stores.kiro_latency_ranker,
            #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
            usage_journal_sink: stores.usage_journal_sink,
            usage_journal_dir: stores.usage_journal_dir,
            #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
            usage_event_flusher: stores.usage_event_flusher,
        }
    }

    /// Open runtime dependencies from configured persistent storage.
    pub async fn from_storage_config(config: &StorageConfig) -> anyhow::Result<Self> {
        validate_state_root(config)?;
        let cluster_state =
            crate::cluster::ClusterRuntimeState::from_storage_config(config).await?;
        let geoip = GeoIpResolver::from_env()?;
        geoip.warmup().await;
        let email_notifier = crate::email::EmailNotifier::from_env()?.map(Arc::new);
        let database_url =
            std::env::var(&config.control_store.database_url_env).with_context(|| {
                format!("missing control database env `{}`", config.control_store.database_url_env)
            })?;
        let request_cache = resolve_request_cache_config(config)?;
        let proxy_scope = postgres_proxy_config_scope(config);
        let repository = Arc::new(
            PostgresControlRepository::connect_with_proxy_scope(
                &database_url,
                request_cache,
                proxy_scope,
            )
            .await?,
        );
        Self::from_open_repository(config, cluster_state, geoip, email_notifier, repository).await
    }

    async fn from_open_repository<R>(
        config: &StorageConfig,
        cluster_state: Option<Arc<crate::cluster::ClusterRuntimeState>>,
        geoip: GeoIpResolver,
        email_notifier: Option<Arc<crate::email::EmailNotifier>>,
        repository: Arc<R>,
    ) -> anyhow::Result<Self>
    where
        R: RuntimeRepository + 'static,
    {
        #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
        let initial_runtime_config = repository.get_admin_runtime_config().await?;
        #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
        let runtime_config = Arc::new(RwLock::new(initial_runtime_config.clone()));
        #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
        let journal_usage = Arc::new(JournalUsageEventSink::open(
            config.usage_journal_dir.clone(),
            &initial_runtime_config,
        )?);
        #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
        let journal_usage_for_status = journal_usage.clone();
        #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
        let rollup_backlog =
            UsageRollupBacklog::open(config.usage_journal_dir.clone(), &initial_runtime_config)?;
        #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
        let initial_pending_rollups = PendingUsageRollups::default();
        #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
        let initial_pending_rollup_report = rollup_backlog
            .for_each_pending_batch(|batch| initial_pending_rollups.add_batch(batch))?;
        #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
        if initial_pending_rollup_report.loaded_batch_count > 0
            || initial_pending_rollup_report.quarantined_file_count > 0
        {
            tracing::error!(
                pending_batch_count = initial_pending_rollup_report.loaded_batch_count,
                loaded_file_count = initial_pending_rollup_report.loaded_file_count,
                sealed_file_count = initial_pending_rollup_report.sealed_file_count,
                quarantined_file_count = initial_pending_rollup_report.quarantined_file_count,
                bad_file_samples = ?initial_pending_rollup_report.bad_file_samples,
                "loaded durable control rollup backlog into pending quota overlay"
            );
        }
        #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
        let source_node_id = config
            .node_identity
            .as_ref()
            .map(|identity| identity.node_id.clone());
        #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
        let (usage_accounting, usage_event_flusher) = UsageAccounting::new(
            repository.clone(),
            journal_usage.clone(),
            journal_usage,
            runtime_config.clone(),
            rollup_backlog,
            initial_pending_rollups,
            source_node_id,
        )?;
        #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
        let control_store: Arc<dyn ControlStore> = Arc::new(UsageAccountingControlStore::new(
            repository.clone(),
            usage_accounting.clone(),
        ));
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
        #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
        let admin_key_store: Arc<dyn AdminKeyStore> = Arc::new(UsageAccountingAdminKeyStore {
            admin_key_store: repository.clone(),
            usage_accounting: usage_accounting.clone(),
        });
        #[cfg(not(any(feature = "duckdb-runtime", feature = "duckdb-bundled")))]
        let admin_key_store: Arc<dyn AdminKeyStore> = repository.clone();
        let admin_account_group_store: Arc<dyn AdminAccountGroupStore> = repository.clone();
        let admin_proxy_store: Arc<dyn AdminProxyStore> = repository.clone();
        let admin_codex_account_store: Arc<dyn AdminCodexAccountStore> = repository.clone();
        let admin_kiro_account_store: Arc<dyn AdminKiroAccountStore> = repository.clone();
        let admin_review_queue_store: Arc<dyn AdminReviewQueueStore> = repository.clone();
        #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
        let public_access_store: Arc<dyn PublicAccessStore> =
            Arc::new(UsageAccountingPublicAccessStore {
                public_access_store: repository.clone(),
                usage_accounting: usage_accounting.clone(),
            });
        #[cfg(not(any(feature = "duckdb-runtime", feature = "duckdb-bundled")))]
        let public_access_store: Arc<dyn PublicAccessStore> = repository.clone();
        let public_community_store: Arc<dyn PublicCommunityStore> = repository.clone();
        #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
        let public_usage_store: Arc<dyn PublicUsageStore> =
            Arc::new(UsageAccountingPublicUsageStore {
                public_usage_store: repository.clone(),
                usage_accounting,
            });
        #[cfg(not(any(feature = "duckdb-runtime", feature = "duckdb-bundled")))]
        let public_usage_store: Arc<dyn PublicUsageStore> = repository.clone();
        let public_submission_store: Arc<dyn PublicSubmissionStore> = repository.clone();
        let public_status_store: Arc<dyn PublicStatusStore> = repository;
        Ok(Self::with_stores(LlmAccessStores {
            control_store,
            cluster_state,
            geoip,
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
            public_submission_store,
            public_status_store,
            email_notifier,
            kiro_latency_ranker: Arc::new(KiroLatencyRanker::default()),
            #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
            usage_journal_sink: Some(journal_usage_for_status),
            usage_journal_dir: Some(config.usage_journal_dir.clone()),
            #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
            usage_event_flusher: Some(usage_event_flusher),
        }))
    }

    /// Shared control store used by request handlers.
    pub fn control_store(&self) -> Arc<dyn ControlStore> {
        Arc::clone(&self.control_store)
    }

    /// Shared cluster state when this deployment participates in multi-node
    /// mode.
    pub fn cluster_state(&self) -> Option<Arc<crate::cluster::ClusterRuntimeState>> {
        self.cluster_state.clone()
    }

    pub(crate) fn geoip(&self) -> GeoIpResolver {
        self.geoip.clone()
    }

    /// Provider route store used by data-plane dispatch.
    pub fn provider_route_store(&self) -> Arc<dyn ProviderRouteStore> {
        Arc::clone(&self.provider_route_store)
    }

    /// Admin config store used by local admin endpoints.
    pub fn admin_config_store(&self) -> Arc<dyn AdminConfigStore> {
        Arc::clone(&self.admin_config_store)
    }

    /// In-memory Kiro latency ranking cache used by provider dispatch.
    pub(crate) fn kiro_latency_ranker(&self) -> Arc<KiroLatencyRanker> {
        Arc::clone(&self.kiro_latency_ranker)
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

    /// Journal root used by producer-side status views.
    pub(crate) fn usage_journal_dir(&self) -> Option<PathBuf> {
        self.usage_journal_dir.clone()
    }

    /// Producer-side journal sink used for live status views.
    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    pub(crate) fn usage_journal_sink(&self) -> Option<Arc<JournalUsageEventSink>> {
        self.usage_journal_sink.clone()
    }

    /// No producer journal sink is available without DuckDB runtime support.
    #[cfg(not(any(feature = "duckdb-runtime", feature = "duckdb-bundled")))]
    pub(crate) fn usage_journal_sink(
        &self,
    ) -> Option<Arc<crate::usage_journal::JournalUsageEventSink>> {
        None
    }

    /// Public submission store used by unauthenticated public endpoints.
    pub fn public_submission_store(&self) -> Arc<dyn PublicSubmissionStore> {
        Arc::clone(&self.public_submission_store)
    }

    /// Public status store used by unauthenticated public endpoints.
    pub fn public_status_store(&self) -> Arc<dyn PublicStatusStore> {
        Arc::clone(&self.public_status_store)
    }

    pub(crate) fn email_notifier(&self) -> Option<Arc<crate::email::EmailNotifier>> {
        self.email_notifier.clone()
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

fn postgres_proxy_config_scope(config: &StorageConfig) -> ProxyConfigScope {
    match config.node_identity.as_ref() {
        Some(identity) if identity.node_class == crate::cluster::NodeClass::Edge => {
            ProxyConfigScope::node(identity.node_id.clone())
        },
        _ => ProxyConfigScope::core(),
    }
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
#[derive(Default)]
struct PendingUsageRollups {
    inner: RwLock<PendingUsageRollupsInner>,
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
#[derive(Default)]
struct PendingUsageRollupsInner {
    rollups: HashMap<String, KeyUsageRollupDelta>,
    last_used_counts: HashMap<String, BTreeMap<i64, usize>>,
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
impl PendingUsageRollups {
    fn add_events(&self, events: &[UsageEvent]) -> anyhow::Result<()> {
        let batch = UsageRollupBatch::from_usage_events(
            "pending-events".to_string(),
            None,
            now_ms(),
            events,
        )
        .context("aggregate pending usage rollups")?;
        self.add_batch(&batch)
    }

    fn subtract_events(&self, events: &[UsageEvent]) -> anyhow::Result<()> {
        let batch = UsageRollupBatch::from_usage_events(
            "pending-events".to_string(),
            None,
            now_ms(),
            events,
        )
        .context("aggregate pending usage rollups")?;
        self.subtract_batch(&batch)
    }

    fn add_batch(&self, batch: &UsageRollupBatch) -> anyhow::Result<()> {
        let mut inner = self
            .inner
            .write()
            .map_err(|_| anyhow!("pending usage rollups lock poisoned"))?;
        increment_pending_last_used_batch(&mut inner.last_used_counts, batch);
        for delta in &batch.deltas {
            inner
                .rollups
                .entry(delta.key_id.clone())
                .and_modify(|current| current.add_assign(delta))
                .or_insert_with(|| delta.clone());
        }
        Ok(())
    }

    fn subtract_batch(&self, batch: &UsageRollupBatch) -> anyhow::Result<()> {
        let mut inner = self
            .inner
            .write()
            .map_err(|_| anyhow!("pending usage rollups lock poisoned"))?;
        subtract_pending_last_used_batch(&mut inner.last_used_counts, batch);
        for delta in &batch.deltas {
            let next_last_used = pending_last_used_for_key(&inner.last_used_counts, delta);
            if let Some(entry) = inner.rollups.get_mut(&delta.key_id) {
                entry.subtract_assign(delta);
                entry.last_used_at_ms = next_last_used;
                if entry.is_zero() {
                    inner.rollups.remove(&delta.key_id);
                    inner.last_used_counts.remove(&delta.key_id);
                }
            }
        }
        Ok(())
    }

    fn subtract_batches(&self, batches: &[UsageRollupBatch]) -> anyhow::Result<()> {
        for batch in batches {
            self.subtract_batch(batch)?;
        }
        Ok(())
    }

    fn delta_for_key(&self, key_id: &str) -> Option<KeyUsageRollupDelta> {
        self.inner
            .read()
            .ok()
            .and_then(|inner| inner.rollups.get(key_id).cloned())
    }
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
fn increment_pending_last_used_batch(
    counts_by_key: &mut HashMap<String, BTreeMap<i64, usize>>,
    batch: &UsageRollupBatch,
) {
    let mut represented_keys = HashSet::new();
    for entry in &batch.last_used_at_ms_counts {
        represented_keys.insert(entry.key_id.as_str());
        let counts = counts_by_key.entry(entry.key_id.clone()).or_default();
        let count = counts.entry(entry.last_used_at_ms).or_insert(0);
        *count = count.saturating_add(usize::try_from(entry.count).unwrap_or(usize::MAX));
    }
    for delta in &batch.deltas {
        if represented_keys.contains(delta.key_id.as_str()) {
            continue;
        }
        let Some(last_used_at_ms) = delta.last_used_at_ms else {
            continue;
        };
        let counts = counts_by_key.entry(delta.key_id.clone()).or_default();
        *counts.entry(last_used_at_ms).or_insert(0) += 1;
    }
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
fn subtract_pending_last_used_batch(
    counts_by_key: &mut HashMap<String, BTreeMap<i64, usize>>,
    batch: &UsageRollupBatch,
) {
    let mut represented_keys = HashSet::new();
    for entry in &batch.last_used_at_ms_counts {
        represented_keys.insert(entry.key_id.as_str());
        decrement_pending_last_used_count(
            counts_by_key,
            &entry.key_id,
            entry.last_used_at_ms,
            usize::try_from(entry.count).unwrap_or(usize::MAX),
        );
    }
    for delta in &batch.deltas {
        if represented_keys.contains(delta.key_id.as_str()) {
            continue;
        }
        let Some(last_used_at_ms) = delta.last_used_at_ms else {
            continue;
        };
        decrement_pending_last_used_count(counts_by_key, &delta.key_id, last_used_at_ms, 1);
    }
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
fn decrement_pending_last_used_count(
    counts_by_key: &mut HashMap<String, BTreeMap<i64, usize>>,
    key_id: &str,
    last_used_at_ms: i64,
    count: usize,
) {
    let Some(counts) = counts_by_key.get_mut(key_id) else {
        return;
    };
    if let Some(current) = counts.get_mut(&last_used_at_ms) {
        *current = current.saturating_sub(count);
        if *current == 0 {
            counts.remove(&last_used_at_ms);
        }
    }
    if counts.is_empty() {
        counts_by_key.remove(key_id);
    }
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
fn pending_last_used_for_key(
    counts_by_key: &HashMap<String, BTreeMap<i64, usize>>,
    delta: &KeyUsageRollupDelta,
) -> Option<i64> {
    counts_by_key
        .get(&delta.key_id)
        .and_then(|counts| counts.keys().next_back().copied())
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
#[derive(Debug, Default)]
struct UsageRollupRetryState {
    attempts: usize,
    suspended_until: Option<Instant>,
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
#[derive(Debug, Clone, Copy)]
struct UsageRollupFailureAttempt {
    attempt: usize,
    retry_suspended: bool,
    retry_after: Option<Duration>,
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
impl UsageRollupRetryState {
    fn retry_suspended(&self, now: Instant) -> bool {
        self.suspended_until.is_some_and(|until| now < until)
    }

    fn retry_after(&self, now: Instant) -> Option<Duration> {
        self.suspended_until
            .and_then(|until| until.checked_duration_since(now))
    }

    fn reenable_if_ready(&mut self, now: Instant) -> bool {
        if self.suspended_until.is_some_and(|until| now >= until) {
            self.suspended_until = None;
            self.attempts = 0;
            return true;
        }
        false
    }

    fn record_success(&mut self) -> bool {
        let had_retry_state = self.attempts > 0 || self.suspended_until.is_some();
        self.attempts = 0;
        self.suspended_until = None;
        had_retry_state
    }

    fn record_failure(
        &mut self,
        now: Instant,
        flush_interval: Duration,
    ) -> UsageRollupFailureAttempt {
        self.attempts = self.attempts.saturating_add(1);
        if self.attempts >= USAGE_EVENT_ROLLUP_MAX_ATTEMPTS {
            let retry_after = usage_rollup_retry_backoff(flush_interval);
            self.suspended_until = Some(now + retry_after);
            return UsageRollupFailureAttempt {
                attempt: self.attempts,
                retry_suspended: true,
                retry_after: Some(retry_after),
            };
        }
        UsageRollupFailureAttempt {
            attempt: self.attempts,
            retry_suspended: false,
            retry_after: None,
        }
    }
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
fn usage_rollup_retry_backoff(flush_interval: Duration) -> Duration {
    flush_interval
        .saturating_mul(USAGE_EVENT_ROLLUP_RETRY_BACKOFF_MULTIPLIER)
        .max(Duration::from_secs(3))
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
struct UsageAccounting {
    tx: mpsc::Sender<Vec<UsageEvent>>,
    pending_rollups: Arc<PendingUsageRollups>,
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
impl UsageAccounting {
    fn new(
        rollup_sink: Arc<dyn UsageRollupBatchSink>,
        journal_sink: Arc<JournalUsageEventSink>,
        analytics_sink: Arc<dyn UsageEventSink>,
        runtime_config: Arc<RwLock<AdminRuntimeConfig>>,
        rollup_backlog: UsageRollupBacklog,
        initial_pending_rollups: PendingUsageRollups,
        source_node_id: Option<String>,
    ) -> anyhow::Result<(Arc<Self>, Arc<UsageEventFlusherHandle>)> {
        let (tx, rx) = mpsc::channel::<Vec<UsageEvent>>(USAGE_EVENT_CHANNEL_CAPACITY);
        let pending_rollups = Arc::new(initial_pending_rollups);
        let handle = spawn_usage_event_flusher(UsageEventFlusherParts {
            rollup_sink,
            journal_sink,
            analytics_sink,
            pending_rollups: pending_rollups.clone(),
            runtime_config,
            rollup_backlog,
            source_node_id,
            rx,
        });
        Ok((
            Arc::new(Self {
                tx,
                pending_rollups,
            }),
            handle,
        ))
    }

    fn overlay_authenticated_key(&self, mut key: AuthenticatedKey) -> AuthenticatedKey {
        if let Some(delta) = self.pending_rollups.delta_for_key(&key.key_id) {
            key.billable_tokens_used = key
                .billable_tokens_used
                .saturating_add(delta.billable_tokens);
        }
        key
    }

    fn overlay_admin_key(&self, mut key: AdminKey) -> AdminKey {
        if let Some(delta) = self.pending_rollups.delta_for_key(&key.id) {
            key.usage_input_uncached_tokens =
                add_i64_to_u64(key.usage_input_uncached_tokens, delta.input_uncached_tokens);
            key.usage_input_cached_tokens =
                add_i64_to_u64(key.usage_input_cached_tokens, delta.input_cached_tokens);
            key.usage_output_tokens = add_i64_to_u64(key.usage_output_tokens, delta.output_tokens);
            key.usage_credit_total += delta.credit_total;
            key.usage_credit_missing_events =
                add_i64_to_u64(key.usage_credit_missing_events, delta.credit_missing_events);
            key.remaining_billable = key.remaining_billable.saturating_sub(delta.billable_tokens);
            key.last_used_at = max_optional_ms(key.last_used_at, delta.last_used_at_ms);
        }
        key
    }

    fn overlay_public_access_key(&self, mut key: PublicAccessKey) -> PublicAccessKey {
        if let Some(delta) = self.pending_rollups.delta_for_key(&key.key_id) {
            key.usage_input_uncached_tokens =
                add_i64_to_u64(key.usage_input_uncached_tokens, delta.input_uncached_tokens);
            key.usage_input_cached_tokens =
                add_i64_to_u64(key.usage_input_cached_tokens, delta.input_cached_tokens);
            key.usage_output_tokens = add_i64_to_u64(key.usage_output_tokens, delta.output_tokens);
            key.usage_billable_tokens =
                add_i64_to_u64(key.usage_billable_tokens, delta.billable_tokens);
            key.last_used_at_ms = max_optional_ms(key.last_used_at_ms, delta.last_used_at_ms);
        }
        key
    }

    fn overlay_public_usage_key(&self, mut key: PublicUsageLookupKey) -> PublicUsageLookupKey {
        if let Some(delta) = self.pending_rollups.delta_for_key(&key.key_id) {
            key.usage_input_uncached_tokens =
                add_i64_to_u64(key.usage_input_uncached_tokens, delta.input_uncached_tokens);
            key.usage_input_cached_tokens =
                add_i64_to_u64(key.usage_input_cached_tokens, delta.input_cached_tokens);
            key.usage_output_tokens = add_i64_to_u64(key.usage_output_tokens, delta.output_tokens);
            key.usage_billable_tokens =
                add_i64_to_u64(key.usage_billable_tokens, delta.billable_tokens);
            key.usage_credit_total += delta.credit_total;
            key.usage_credit_missing_events =
                add_i64_to_u64(key.usage_credit_missing_events, delta.credit_missing_events);
            key.last_used_at_ms = max_optional_ms(key.last_used_at_ms, delta.last_used_at_ms);
        }
        key
    }

    async fn enqueue_events(&self, events: Vec<UsageEvent>) -> anyhow::Result<()> {
        if events.is_empty() {
            return Ok(());
        }
        self.pending_rollups.add_events(&events)?;
        match self.tx.send(events).await {
            Ok(()) => Ok(()),
            Err(err) => {
                let failed_events = err.0;
                let _ = self.pending_rollups.subtract_events(&failed_events);
                Err(anyhow!("failed to enqueue llm access usage event batch"))
            },
        }
    }
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
#[async_trait]
impl UsageEventSink for UsageAccounting {
    async fn append_usage_events(&self, events: &[UsageEvent]) -> anyhow::Result<()> {
        self.enqueue_events(events.to_vec()).await
    }

    async fn append_usage_events_owned(&self, events: Vec<UsageEvent>) -> anyhow::Result<()> {
        self.enqueue_events(events).await
    }
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
fn add_i64_to_u64(value: u64, delta: i64) -> u64 {
    value.saturating_add(delta.max(0) as u64)
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
fn max_optional_ms(current: Option<i64>, next: Option<i64>) -> Option<i64> {
    match (current, next) {
        (Some(current), Some(next)) => Some(current.max(next)),
        (None, next) => next,
        (current, None) => current,
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
async fn prune_usage_rollup_batch_markers_if_due(
    rollup_sink: &dyn UsageRollupBatchSink,
    last_prune_at: &mut Option<Instant>,
    now: Instant,
    rollup_backlog: &UsageRollupBacklog,
) {
    if last_prune_at.is_some_and(|last| {
        now.saturating_duration_since(last) < USAGE_ROLLUP_BATCH_MARKER_PRUNE_INTERVAL
    }) {
        return;
    }
    match rollup_backlog.sealed_file_count() {
        Ok(count) if count > 0 => {
            tracing::debug!(
                sealed_file_count = count,
                "skipping usage rollup applied-batch marker prune until disk backlog is drained"
            );
            return;
        },
        Ok(_) => {},
        Err(err) => {
            tracing::warn!(
                "failed to count usage rollup disk backlog before marker prune; skipping prune: \
                 {err:#}"
            );
            return;
        },
    }
    *last_prune_at = Some(now);
    let retention_ms =
        i64::try_from(USAGE_ROLLUP_BATCH_MARKER_RETENTION.as_millis()).unwrap_or(i64::MAX);
    let cutoff_ms = now_ms().saturating_sub(retention_ms);
    match rollup_sink
        .prune_usage_rollup_batch_markers(cutoff_ms)
        .await
    {
        Ok(deleted) if deleted > 0 => tracing::info!(
            deleted_marker_count = deleted,
            cutoff_ms,
            retention_ms,
            "pruned old usage rollup applied-batch markers"
        ),
        Ok(_) => tracing::debug!(
            cutoff_ms,
            retention_ms,
            "usage rollup applied-batch marker prune found no old rows"
        ),
        Err(err) => tracing::warn!(
            cutoff_ms,
            retention_ms,
            "failed to prune usage rollup applied-batch markers: {err:#}"
        ),
    }
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
struct UsageEventFlusherParts {
    rollup_sink: Arc<dyn UsageRollupBatchSink>,
    journal_sink: Arc<JournalUsageEventSink>,
    analytics_sink: Arc<dyn UsageEventSink>,
    pending_rollups: Arc<PendingUsageRollups>,
    runtime_config: Arc<RwLock<AdminRuntimeConfig>>,
    rollup_backlog: UsageRollupBacklog,
    source_node_id: Option<String>,
    rx: mpsc::Receiver<Vec<UsageEvent>>,
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
fn spawn_usage_event_flusher(parts: UsageEventFlusherParts) -> Arc<UsageEventFlusherHandle> {
    let UsageEventFlusherParts {
        rollup_sink,
        journal_sink,
        analytics_sink,
        pending_rollups,
        runtime_config,
        mut rollup_backlog,
        source_node_id,
        mut rx,
    } = parts;
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    let join = tokio::spawn(async move {
        let initial_config = {
            let config = runtime_config
                .read()
                .expect("llm access runtime config lock poisoned");
            usage_flush_config(&config)
        };
        let mut buffer = Vec::with_capacity(initial_config.batch_size);
        let mut analytics_retry_buffer = Vec::new();
        let mut buffered_bytes = 0usize;
        let mut analytics_retry_bytes = 0usize;
        let mut flush_count: u64 = 0;
        let mut rollup_retry = UsageRollupRetryState::default();
        let mut last_rollup_marker_prune_at = None;

        loop {
            let flush_config = {
                let config = runtime_config
                    .read()
                    .expect("llm access runtime config lock poisoned");
                usage_flush_config(&config)
            };

            tokio::select! {
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        while let Ok(events) = rx.try_recv() {
                            append_usage_event_batch(
                                &mut buffer,
                                &mut buffered_bytes,
                                events,
                            );
                        }
                        flush_usage_event_buffer(
                            UsageEventFlushTargets {
                                rollup_sink: rollup_sink.as_ref(),
                                analytics_sink: analytics_sink.as_ref(),
                                pending_rollups: pending_rollups.as_ref(),
                                source_node_id: source_node_id.as_deref(),
                            },
                            UsageEventFlushState {
                                buffer: &mut buffer,
                                analytics_retry_buffer: &mut analytics_retry_buffer,
                                buffered_bytes: &mut buffered_bytes,
                                analytics_retry_bytes: &mut analytics_retry_bytes,
                                max_buffer_bytes: flush_config.max_buffer_bytes,
                                flush_count: &mut flush_count,
                                rollup_retry: &mut rollup_retry,
                                flush_interval: flush_config.flush_interval,
                                rollup_backlog: &mut rollup_backlog,
                            },
                            "final usage event flush failed during shutdown",
                        )
                        .await;
                        tracing::info!("llm access usage event flusher shutting down (shutdown signal)");
                        return;
                    }
                }
                maybe_events = rx.recv() => {
                    match maybe_events {
                        Some(events) => {
                            append_usage_event_batch(
                                &mut buffer,
                                &mut buffered_bytes,
                                events,
                            );
                            while buffer.len() < flush_config.batch_size
                                && buffered_bytes < flush_config.max_buffer_bytes
                            {
                                match rx.try_recv() {
                                    Ok(events) => {
                                        append_usage_event_batch(
                                            &mut buffer,
                                            &mut buffered_bytes,
                                            events,
                                        );
                                    },
                                    Err(_) => break,
                                }
                            }
                            if buffer.len() >= flush_config.batch_size
                                || buffered_bytes >= flush_config.max_buffer_bytes
                            {
                                flush_usage_event_buffer(
                                    UsageEventFlushTargets {
                                        rollup_sink: rollup_sink.as_ref(),
                                        analytics_sink: analytics_sink.as_ref(),
                                        pending_rollups: pending_rollups.as_ref(),
                                        source_node_id: source_node_id.as_deref(),
                                    },
                                    UsageEventFlushState {
                                        buffer: &mut buffer,
                                        analytics_retry_buffer: &mut analytics_retry_buffer,
                                        buffered_bytes: &mut buffered_bytes,
                                        analytics_retry_bytes: &mut analytics_retry_bytes,
                                        max_buffer_bytes: flush_config.max_buffer_bytes,
                                        flush_count: &mut flush_count,
                                        rollup_retry: &mut rollup_retry,
                                        flush_interval: flush_config.flush_interval,
                                        rollup_backlog: &mut rollup_backlog,
                                    },
                                    "usage event batch flush failed",
                                )
                                .await;
                            }
                        },
                        None => {
                            flush_usage_event_buffer(
                                UsageEventFlushTargets {
                                    rollup_sink: rollup_sink.as_ref(),
                                    analytics_sink: analytics_sink.as_ref(),
                                    pending_rollups: pending_rollups.as_ref(),
                                    source_node_id: source_node_id.as_deref(),
                                },
                                UsageEventFlushState {
                                    buffer: &mut buffer,
                                    analytics_retry_buffer: &mut analytics_retry_buffer,
                                    buffered_bytes: &mut buffered_bytes,
                                    analytics_retry_bytes: &mut analytics_retry_bytes,
                                    max_buffer_bytes: flush_config.max_buffer_bytes,
                                    flush_count: &mut flush_count,
                                    rollup_retry: &mut rollup_retry,
                                    flush_interval: flush_config.flush_interval,
                                    rollup_backlog: &mut rollup_backlog,
                                },
                                "final usage event flush failed",
                            )
                            .await;
                            tracing::info!("llm access usage event flusher shutting down");
                            return;
                        },
                    }
                }
                _ = time::sleep(flush_config.flush_interval) => {
                    journal_sink.maintain();
                    let now = Instant::now();
                    prune_usage_rollup_batch_markers_if_due(
                        rollup_sink.as_ref(),
                        &mut last_rollup_marker_prune_at,
                        now,
                        &rollup_backlog,
                    )
                    .await;
                    if rollup_retry.reenable_if_ready(now) {
                        tracing::warn!(
                            sealed_file_count = rollup_backlog.sealed_file_count().unwrap_or(0),
                            "usage control rollup retry window reopened; parked backlog will be retried"
                        );
                    }
                    let retry_suspended = rollup_retry.retry_suspended(now);
                    let sealed_file_count = rollup_backlog.sealed_file_count().unwrap_or(0);
                    let has_rollup_backlog = !retry_suspended && sealed_file_count > 0;
                    if retry_suspended && sealed_file_count > 0 {
                        tracing::debug!(
                            sealed_file_count,
                            retry_after_ms = rollup_retry
                                .retry_after(now)
                                .map(|duration| duration.as_millis()),
                            "usage control rollup backlog retry is temporarily suspended"
                        );
                    }
                    if !buffer.is_empty()
                        || has_rollup_backlog
                        || !analytics_retry_buffer.is_empty()
                    {
                        flush_usage_event_buffer(
                            UsageEventFlushTargets {
                                rollup_sink: rollup_sink.as_ref(),
                                analytics_sink: analytics_sink.as_ref(),
                                pending_rollups: pending_rollups.as_ref(),
                                source_node_id: source_node_id.as_deref(),
                            },
                            UsageEventFlushState {
                                buffer: &mut buffer,
                                analytics_retry_buffer: &mut analytics_retry_buffer,
                                buffered_bytes: &mut buffered_bytes,
                                analytics_retry_bytes: &mut analytics_retry_bytes,
                                max_buffer_bytes: flush_config.max_buffer_bytes,
                                flush_count: &mut flush_count,
                                rollup_retry: &mut rollup_retry,
                                flush_interval: flush_config.flush_interval,
                                rollup_backlog: &mut rollup_backlog,
                            },
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
fn append_usage_event_batch(
    buffer: &mut Vec<UsageEvent>,
    buffered_bytes: &mut usize,
    mut events: Vec<UsageEvent>,
) {
    *buffered_bytes =
        buffered_bytes.saturating_add(events.iter().map(estimate_usage_event_bytes).sum::<usize>());
    buffer.append(&mut events);
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
struct UsageEventFlushTargets<'a> {
    rollup_sink: &'a dyn UsageRollupBatchSink,
    analytics_sink: &'a dyn UsageEventSink,
    pending_rollups: &'a PendingUsageRollups,
    source_node_id: Option<&'a str>,
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
struct UsageEventFlushState<'a> {
    buffer: &'a mut Vec<UsageEvent>,
    analytics_retry_buffer: &'a mut Vec<UsageEvent>,
    buffered_bytes: &'a mut usize,
    analytics_retry_bytes: &'a mut usize,
    max_buffer_bytes: usize,
    flush_count: &'a mut u64,
    rollup_retry: &'a mut UsageRollupRetryState,
    flush_interval: Duration,
    rollup_backlog: &'a mut UsageRollupBacklog,
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
async fn flush_usage_event_buffer(
    targets: UsageEventFlushTargets<'_>,
    mut state: UsageEventFlushState<'_>,
    error_message: &'static str,
) {
    let now = Instant::now();
    if state.rollup_retry.reenable_if_ready(now) {
        tracing::warn!(
            sealed_file_count = state.rollup_backlog.sealed_file_count().unwrap_or(0),
            "usage control rollup retry window reopened during flush"
        );
    }
    flush_rollup_backlog(&targets, &mut state, error_message).await;
    if !state.buffer.is_empty() {
        let batch = std::mem::take(state.buffer);
        *state.buffered_bytes = 0;
        let count = batch.len();
        let buffered_bytes = batch.iter().map(estimate_usage_event_bytes).sum();
        let summary = usage_event_batch_log_summary(&batch);
        let rollup_batch = match usage_rollup_batch_from_events(targets.source_node_id, &batch) {
            Ok(batch) => batch,
            Err(err) => {
                tracing::error!(
                    count,
                    distinct_key_count = summary.distinct_key_count,
                    key_id_samples = ?summary.key_id_samples,
                    key_name_samples = ?summary.key_name_samples,
                    event_id_samples = ?summary.event_id_samples,
                    "failed to aggregate live usage rollup batch; keeping pending overlay: {err:#}"
                );
                append_analytics_retry_events(&mut state, batch);
                return;
            },
        };
        append_analytics_retry_events(&mut state, batch);
        if state.rollup_retry.retry_suspended(Instant::now()) {
            tracing::warn!(
                batch_id = %rollup_batch.batch_id,
                count,
                buffered_bytes,
                distinct_key_count = summary.distinct_key_count,
                key_id_samples = ?summary.key_id_samples,
                key_name_samples = ?summary.key_name_samples,
                event_id_samples = ?summary.event_id_samples,
                retry_after_ms = state
                    .rollup_retry
                    .retry_after(Instant::now())
                    .map(|duration| duration.as_millis()),
                "parking live usage rollup batch without control-store attempt because rollup retry \
                 window is suspended"
            );
            if let Err(backlog_err) = state
                .rollup_backlog
                .append_batches(std::slice::from_ref(&rollup_batch))
            {
                tracing::error!(
                    batch_id = %rollup_batch.batch_id,
                    count,
                    distinct_key_count = summary.distinct_key_count,
                    key_id_samples = ?summary.key_id_samples,
                    event_id_samples = ?summary.event_id_samples,
                    "failed to persist usage control rollup to disk backlog while retry window was \
                     suspended; pending overlay remains memory-only: {backlog_err:#}"
                );
            }
        } else {
            match targets
                .rollup_sink
                .apply_usage_rollup_batches(std::slice::from_ref(&rollup_batch))
                .await
            {
                Ok(report) => {
                    if state.rollup_retry.record_success() {
                        tracing::warn!(
                            batch_id = %rollup_batch.batch_id,
                            "live usage rollup write recovered; retry state reset"
                        );
                    }
                    if let Err(err) = targets.pending_rollups.subtract_batch(&rollup_batch) {
                        tracing::error!(
                            count,
                            distinct_key_count = summary.distinct_key_count,
                            key_id_samples = ?summary.key_id_samples,
                            key_name_samples = ?summary.key_name_samples,
                            event_id_samples = ?summary.event_id_samples,
                            "persisted usage rollups but failed to clear pending usage rollups: \
                             {err:#}"
                        );
                    }
                    if report.missing_key_delta_count > 0 || report.already_applied_batch_count > 0
                    {
                        tracing::warn!(
                            batch_id = %rollup_batch.batch_id,
                            applied_batch_count = report.applied_batch_count,
                            already_applied_batch_count = report.already_applied_batch_count,
                            delta_count = report.delta_count,
                            missing_key_delta_count = report.missing_key_delta_count,
                            "live usage rollup batch applied with replay or missing-key details"
                        );
                    }
                },
                Err(err) => {
                    let failure = state
                        .rollup_retry
                        .record_failure(Instant::now(), state.flush_interval);
                    let context = if failure.retry_suspended {
                        "live usage rollup write reached max attempts; parked control rollup \
                         backlog remains on disk and will retry after backoff"
                    } else {
                        "live usage rollup write failed; parked control rollup delta while \
                         forwarding events to analytics/journal"
                    };
                    log_usage_rollup_failure(UsageRollupFailureLog {
                        context,
                        flush_context: error_message,
                        count,
                        buffered_bytes,
                        summary: &summary,
                        attempt: failure.attempt,
                        retry_suspended: failure.retry_suspended,
                        retry_after: failure.retry_after,
                        err: &err,
                    });
                    if let Err(backlog_err) = state
                        .rollup_backlog
                        .append_batches(std::slice::from_ref(&rollup_batch))
                    {
                        tracing::error!(
                            batch_id = %rollup_batch.batch_id,
                            count,
                            distinct_key_count = summary.distinct_key_count,
                            key_id_samples = ?summary.key_id_samples,
                            event_id_samples = ?summary.event_id_samples,
                            "failed to persist usage control rollup to disk backlog; pending overlay \
                             remains memory-only: {backlog_err:#}"
                        );
                    }
                },
            }
        }
    }
    if !state.analytics_retry_buffer.is_empty() {
        let analytics_batch = std::mem::take(state.analytics_retry_buffer);
        *state.analytics_retry_bytes = 0;
        let count = analytics_batch.len();
        match targets
            .analytics_sink
            .append_usage_events_owned(analytics_batch)
            .await
        {
            Ok(()) => {
                *state.flush_count += 1;
                tracing::debug!(
                    "flushed {count} llm access usage events (flush #{})",
                    *state.flush_count
                );
            },
            Err(err) => {
                tracing::error!(count, "{}: {err:#}", error_message);
                tracing::warn!(
                    count,
                    "dropped llm access analytics events after control rollups were persisted or \
                     parked"
                );
            },
        }
    }
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
async fn flush_rollup_backlog(
    targets: &UsageEventFlushTargets<'_>,
    state: &mut UsageEventFlushState<'_>,
    error_message: &'static str,
) {
    let now = Instant::now();
    if state.rollup_retry.retry_suspended(now) {
        tracing::debug!(
            retry_after_ms = state
                .rollup_retry
                .retry_after(now)
                .map(|duration| duration.as_millis()),
            "skipping parked control rollup backlog while retry window is suspended"
        );
        return;
    }
    let claim = match state.rollup_backlog.claim_next() {
        Ok(Some(claim)) => claim,
        Ok(None) => return,
        Err(err) => {
            tracing::error!(
                flush_context = error_message,
                "failed to claim usage control rollup backlog: {err:#}"
            );
            return;
        },
    };
    let claim_path = claim.path().display().to_string();
    let batches = match state.rollup_backlog.read_claim(&claim) {
        Ok(batches) => batches,
        Err(err) => {
            match state.rollup_backlog.quarantine_claim(claim) {
                Ok(bad_path) => {
                    tracing::error!(
                        path = %claim_path,
                        bad_path = %bad_path.display(),
                        flush_context = error_message,
                        "quarantined unreadable usage control rollup backlog claim: {err:#}"
                    );
                },
                Err(quarantine_err) => {
                    tracing::error!(
                        path = %claim_path,
                        flush_context = error_message,
                        "failed to quarantine unreadable usage control rollup backlog claim after \
                         read failure: {quarantine_err:#}; original read error: {err:#}"
                    );
                },
            }
            return;
        },
    };
    let summary = usage_rollup_batches_log_summary(&batches);
    match targets
        .rollup_sink
        .apply_usage_rollup_batches(&batches)
        .await
    {
        Ok(report) => {
            if state.rollup_retry.record_success() {
                tracing::warn!(
                    path = %claim_path,
                    "parked usage rollup backlog write recovered; retry state reset"
                );
            }
            if let Err(err) = targets.pending_rollups.subtract_batches(&batches) {
                tracing::error!(
                    source_event_count = summary.source_event_count,
                    distinct_key_count = summary.distinct_key_count,
                    key_id_samples = ?summary.key_id_samples,
                    "persisted parked usage rollups but failed to clear pending usage rollups: \
                     {err:#}"
                );
            }
            if let Err(err) = state.rollup_backlog.complete_claim(claim) {
                tracing::error!(
                    path = %claim_path,
                    "applied usage control rollup backlog but failed to delete claim: {err:#}"
                );
            }
            tracing::warn!(
                path = %claim_path,
                applied_batch_count = report.applied_batch_count,
                already_applied_batch_count = report.already_applied_batch_count,
                delta_count = report.delta_count,
                missing_key_delta_count = report.missing_key_delta_count,
                source_event_count = summary.source_event_count,
                distinct_key_count = summary.distinct_key_count,
                key_id_samples = ?summary.key_id_samples,
                "parked control rollup backlog recovered and persisted"
            );
        },
        Err(err) => {
            if is_deterministic_rollup_apply_error(&err) {
                match state.rollup_backlog.quarantine_claim(claim) {
                    Ok(bad_path) => {
                        if let Err(clear_err) = targets.pending_rollups.subtract_batches(&batches) {
                            tracing::error!(
                                path = %claim_path,
                                bad_path = %bad_path.display(),
                                source_event_count = summary.source_event_count,
                                distinct_key_count = summary.distinct_key_count,
                                key_id_samples = ?summary.key_id_samples,
                                batch_id_samples = ?summary.batch_id_samples,
                                flush_context = error_message,
                                "quarantined poison control rollup backlog but failed to clear \
                                 pending usage rollups: {clear_err:#}"
                            );
                        }
                        tracing::error!(
                            path = %claim_path,
                            bad_path = %bad_path.display(),
                            source_event_count = summary.source_event_count,
                            distinct_key_count = summary.distinct_key_count,
                            key_id_samples = ?summary.key_id_samples,
                            batch_id_samples = ?summary.batch_id_samples,
                            flush_context = error_message,
                            "quarantined poison control rollup backlog after deterministic apply \
                             failure: {err:#}"
                        );
                    },
                    Err(quarantine_err) => {
                        tracing::error!(
                            path = %claim_path,
                            source_event_count = summary.source_event_count,
                            distinct_key_count = summary.distinct_key_count,
                            key_id_samples = ?summary.key_id_samples,
                            batch_id_samples = ?summary.batch_id_samples,
                            flush_context = error_message,
                            "failed to quarantine poison control rollup backlog after deterministic \
                             apply failure: {quarantine_err:#}; original apply error: {err:#}"
                        );
                    },
                }
                return;
            }
            let failure = state
                .rollup_retry
                .record_failure(Instant::now(), state.flush_interval);
            let context = if failure.retry_suspended {
                "parked control rollup retry reached max attempts; durable backlog remains on disk \
                 and will retry after backoff"
            } else {
                "parked control rollup retry failed"
            };
            tracing::error!(
                path = %claim_path,
                attempt = failure.attempt,
                max_attempts = USAGE_EVENT_ROLLUP_MAX_ATTEMPTS,
                retry_suspended = failure.retry_suspended,
                retry_after_ms = failure.retry_after.map(|duration| duration.as_millis()),
                source_event_count = summary.source_event_count,
                distinct_key_count = summary.distinct_key_count,
                key_id_samples = ?summary.key_id_samples,
                batch_id_samples = ?summary.batch_id_samples,
                flush_context = error_message,
                "usage control rollup failed: {}: {:#}",
                context,
                err
            );
            if let Err(restore_err) = state.rollup_backlog.restore_claim(claim) {
                tracing::error!(
                    path = %claim_path,
                    "failed to restore usage control rollup backlog claim after apply failure: \
                     {restore_err:#}"
                );
            }
        },
    }
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
fn is_deterministic_rollup_apply_error(err: &anyhow::Error) -> bool {
    err.downcast_ref::<UsageRollupDigestMismatch>().is_some()
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
struct UsageRollupFailureLog<'a> {
    context: &'static str,
    flush_context: &'static str,
    count: usize,
    buffered_bytes: usize,
    summary: &'a UsageEventBatchLogSummary,
    attempt: usize,
    retry_suspended: bool,
    retry_after: Option<Duration>,
    err: &'a anyhow::Error,
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
fn log_usage_rollup_failure(log: UsageRollupFailureLog<'_>) {
    tracing::error!(
        count = log.count,
        buffered_bytes = log.buffered_bytes,
        attempt = log.attempt,
        max_attempts = USAGE_EVENT_ROLLUP_MAX_ATTEMPTS,
        retry_suspended = log.retry_suspended,
        retry_after_ms = log.retry_after.map(|duration| duration.as_millis()),
        distinct_key_count = log.summary.distinct_key_count,
        key_id_samples = ?log.summary.key_id_samples,
        key_name_samples = ?log.summary.key_name_samples,
        account_name_samples = ?log.summary.account_name_samples,
        provider_samples = ?log.summary.provider_samples,
        endpoint_samples = ?log.summary.endpoint_samples,
        status_code_samples = ?log.summary.status_code_samples,
        event_id_samples = ?log.summary.event_id_samples,
        flush_context = log.flush_context,
        "usage control rollup failed: {}: {:#}",
        log.context,
        log.err
    );
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
#[derive(Debug, Default)]
struct UsageEventBatchLogSummary {
    distinct_key_count: usize,
    key_id_samples: Vec<String>,
    key_name_samples: Vec<String>,
    account_name_samples: Vec<String>,
    provider_samples: Vec<String>,
    endpoint_samples: Vec<String>,
    status_code_samples: Vec<String>,
    event_id_samples: Vec<String>,
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
fn usage_event_batch_log_summary(events: &[UsageEvent]) -> UsageEventBatchLogSummary {
    let mut summary = UsageEventBatchLogSummary::default();
    let mut distinct_key_ids = HashSet::<&str>::new();
    for event in events {
        distinct_key_ids.insert(event.key_id.as_str());
        push_unique_sample(&mut summary.key_id_samples, &event.key_id);
        push_unique_sample(&mut summary.key_name_samples, &event.key_name);
        if let Some(account_name) = &event.account_name {
            push_unique_sample(&mut summary.account_name_samples, account_name);
        }
        push_unique_sample(
            &mut summary.provider_samples,
            &format!("{:?}/{:?}", event.provider_type, event.protocol_family),
        );
        push_unique_sample(&mut summary.endpoint_samples, &event.endpoint);
        push_unique_sample(&mut summary.status_code_samples, &event.status_code.to_string());
        push_unique_sample(&mut summary.event_id_samples, &event.event_id);
    }
    summary.distinct_key_count = distinct_key_ids.len();
    summary
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
#[derive(Debug, Default)]
struct UsageRollupBatchLogSummary {
    source_event_count: u64,
    distinct_key_count: usize,
    key_id_samples: Vec<String>,
    batch_id_samples: Vec<String>,
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
fn usage_rollup_batches_log_summary(batches: &[UsageRollupBatch]) -> UsageRollupBatchLogSummary {
    let mut summary = UsageRollupBatchLogSummary::default();
    let mut distinct_key_ids = HashSet::<&str>::new();
    for batch in batches {
        summary.source_event_count = summary
            .source_event_count
            .saturating_add(batch.source_event_count);
        push_unique_sample(&mut summary.batch_id_samples, &batch.batch_id);
        for delta in &batch.deltas {
            distinct_key_ids.insert(delta.key_id.as_str());
            push_unique_sample(&mut summary.key_id_samples, &delta.key_id);
        }
    }
    summary.distinct_key_count = distinct_key_ids.len();
    summary
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
fn usage_rollup_batch_from_events(
    source_node_id: Option<&str>,
    events: &[UsageEvent],
) -> anyhow::Result<UsageRollupBatch> {
    UsageRollupBatch::from_usage_events(
        format!("usage-rollup-{}", uuid::Uuid::new_v4()),
        source_node_id.map(str::to_string),
        now_ms(),
        events,
    )
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
fn push_unique_sample(samples: &mut Vec<String>, value: &str) {
    if samples.len() >= USAGE_EVENT_LOG_SAMPLE_LIMIT || samples.iter().any(|sample| sample == value)
    {
        return;
    }
    samples.push(value.to_string());
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
fn append_analytics_retry_events(state: &mut UsageEventFlushState<'_>, events: Vec<UsageEvent>) {
    let mut dropped = 0usize;
    for event in events {
        let event_bytes = estimate_usage_event_bytes(&event);
        if state.analytics_retry_bytes.saturating_add(event_bytes) > state.max_buffer_bytes {
            dropped = dropped.saturating_add(1);
            continue;
        }
        *state.analytics_retry_bytes = state.analytics_retry_bytes.saturating_add(event_bytes);
        state.analytics_retry_buffer.push(event);
    }
    if dropped > 0 {
        tracing::warn!(
            dropped,
            max_buffer_bytes = state.max_buffer_bytes,
            "dropped llm access analytics retry events after control rollups were persisted or \
             parked"
        );
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
struct UsageAccountingControlStore {
    control_store: Arc<dyn ControlStore>,
    usage_accounting: Arc<UsageAccounting>,
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
impl UsageAccountingControlStore {
    fn new(control_store: Arc<dyn ControlStore>, usage_accounting: Arc<UsageAccounting>) -> Self {
        Self {
            control_store,
            usage_accounting,
        }
    }
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
#[async_trait]
impl ControlStore for UsageAccountingControlStore {
    async fn authenticate_bearer_secret(
        &self,
        secret: &str,
    ) -> anyhow::Result<Option<AuthenticatedKey>> {
        Ok(self
            .control_store
            .authenticate_bearer_secret(secret)
            .await?
            .map(|key| self.usage_accounting.overlay_authenticated_key(key)))
    }

    async fn apply_usage_rollup(&self, event: &UsageEvent) -> anyhow::Result<()> {
        self.usage_accounting.append_usage_event(event).await
    }

    async fn apply_usage_rollup_owned(&self, event: UsageEvent) -> anyhow::Result<()> {
        self.usage_accounting
            .append_usage_events_owned(vec![event])
            .await
    }
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
struct UsageAccountingAdminKeyStore {
    admin_key_store: Arc<dyn AdminKeyStore>,
    usage_accounting: Arc<UsageAccounting>,
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
#[async_trait]
impl AdminKeyStore for UsageAccountingAdminKeyStore {
    async fn list_admin_keys(&self) -> anyhow::Result<Vec<AdminKey>> {
        Ok(self
            .admin_key_store
            .list_admin_keys()
            .await?
            .into_iter()
            .map(|key| self.usage_accounting.overlay_admin_key(key))
            .collect())
    }

    async fn get_admin_key(&self, key_id: &str) -> anyhow::Result<Option<AdminKey>> {
        Ok(self
            .admin_key_store
            .get_admin_key(key_id)
            .await?
            .map(|key| self.usage_accounting.overlay_admin_key(key)))
    }

    async fn list_admin_keys_page(
        &self,
        provider_type: Option<&str>,
        page: AdminPageRequest,
    ) -> anyhow::Result<AdminKeysPage> {
        let mut page = self
            .admin_key_store
            .list_admin_keys_page(provider_type, page)
            .await?;
        page.keys = page
            .keys
            .into_iter()
            .map(|key| self.usage_accounting.overlay_admin_key(key))
            .collect();
        Ok(page)
    }

    async fn find_admin_key_referencing_account_group(
        &self,
        provider_type: &str,
        group_id: &str,
    ) -> anyhow::Result<Option<AdminKey>> {
        Ok(self
            .admin_key_store
            .find_admin_key_referencing_account_group(provider_type, group_id)
            .await?
            .map(|key| self.usage_accounting.overlay_admin_key(key)))
    }

    async fn create_admin_key(&self, key: NewAdminKey) -> anyhow::Result<AdminKey> {
        self.admin_key_store.create_admin_key(key).await
    }

    async fn patch_admin_key(
        &self,
        key_id: &str,
        patch: AdminKeyPatch,
    ) -> anyhow::Result<Option<AdminKey>> {
        Ok(self
            .admin_key_store
            .patch_admin_key(key_id, patch)
            .await?
            .map(|key| self.usage_accounting.overlay_admin_key(key)))
    }

    async fn delete_admin_key(&self, key_id: &str) -> anyhow::Result<Option<AdminKey>> {
        self.admin_key_store.delete_admin_key(key_id).await
    }
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
struct UsageAccountingPublicAccessStore {
    public_access_store: Arc<dyn PublicAccessStore>,
    usage_accounting: Arc<UsageAccounting>,
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
#[async_trait]
impl PublicAccessStore for UsageAccountingPublicAccessStore {
    async fn auth_cache_ttl_seconds(&self) -> anyhow::Result<u64> {
        self.public_access_store.auth_cache_ttl_seconds().await
    }

    async fn list_public_access_keys(&self) -> anyhow::Result<Vec<PublicAccessKey>> {
        Ok(self
            .public_access_store
            .list_public_access_keys()
            .await?
            .into_iter()
            .map(|key| self.usage_accounting.overlay_public_access_key(key))
            .collect())
    }
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
struct UsageAccountingPublicUsageStore {
    public_usage_store: Arc<dyn PublicUsageStore>,
    usage_accounting: Arc<UsageAccounting>,
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
#[async_trait]
impl PublicUsageStore for UsageAccountingPublicUsageStore {
    async fn get_public_usage_key_by_secret(
        &self,
        secret: &str,
    ) -> anyhow::Result<Option<PublicUsageLookupKey>> {
        Ok(self
            .public_usage_store
            .get_public_usage_key_by_secret(secret)
            .await?
            .map(|key| self.usage_accounting.overlay_public_usage_key(key)))
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
    validate_usage_journal_dir(&config.usage_journal_dir)?;
    for dir in [
        &config.kiro_auths_dir,
        &config.codex_auths_dir,
        &config.logs_dir,
        &config.usage_journal_dir,
    ] {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("failed to create `{}`", dir.display()))?;
    }
    Ok(())
}

/// Validate the minimal worker state used by the usage journal consumer.
pub fn validate_usage_worker_state_root(config: &StorageConfig) -> anyhow::Result<()> {
    let metadata = std::fs::metadata(&config.state_root).with_context(|| {
        format!("state root `{}` is not accessible", config.state_root.display())
    })?;
    if !metadata.is_dir() {
        return Err(anyhow!("state root `{}` is not a directory", config.state_root.display()));
    }
    validate_usage_journal_dir(&config.usage_journal_dir)?;
    std::fs::create_dir_all(&config.usage_journal_dir).with_context(|| {
        format!("failed to create usage journal dir `{}`", config.usage_journal_dir.display())
    })?;
    Ok(())
}

fn validate_usage_journal_dir(path: &std::path::Path) -> anyhow::Result<()> {
    for shared_root in
        [std::path::Path::new("/mnt/llm-access"), std::path::Path::new("/mnt/llm-access-usage")]
    {
        if path.starts_with(shared_root) {
            anyhow::bail!(
                "usage journal dir `{}` must stay on local disk, not under shared mount `{}`",
                path.display(),
                shared_root.display()
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    use std::{
        sync::{Arc, RwLock},
        time::{Duration, Instant},
    };

    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    use llm_access_core::{
        provider::{ProtocolFamily, ProviderType},
        store::{
            AdminRuntimeConfig, AuthenticatedKey, ControlStore, KeyUsageRollupDelta,
            UsageEventSink, UsageRollupApplyReport, UsageRollupBatch, UsageRollupBatchSink,
            UsageRollupDigestMismatch,
        },
        usage::UsageEvent,
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
            node_identity: None,
            control_store: crate::config::ControlStoreConfig {
                database_url_env: "LLM_ACCESS_CONTROL_DATABASE_URL".to_string(),
            },
            request_cache: None,
            duckdb: root.join("analytics/usage.duckdb"),
            usage_journal_dir: root.join("usage-journal"),
            duckdb_tiered: None,
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
        assert!(config.usage_journal_dir.is_dir());
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn validate_state_root_rejects_usage_journal_under_control_mount() {
        let mut config = temp_storage_config("state-root-shared-journal");
        let root = config.state_root.clone();
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create root");
        config.usage_journal_dir = std::path::PathBuf::from("/mnt/llm-access/usage-journal");

        let err = super::validate_state_root(&config).expect_err("shared journal must fail");
        assert!(err
            .to_string()
            .contains("usage journal dir `/mnt/llm-access/usage-journal` must stay on local disk"));

        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn validate_usage_worker_state_root_rejects_usage_journal_under_usage_mount() {
        let mut config = temp_storage_config("worker-state-root-shared-journal");
        let root = config.state_root.clone();
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create root");
        config.usage_journal_dir = std::path::PathBuf::from("/mnt/llm-access-usage/usage-journal");

        let err = super::validate_usage_worker_state_root(&config)
            .expect_err("shared usage journal must fail");
        assert!(err.to_string().contains(
            "usage journal dir `/mnt/llm-access-usage/usage-journal` must stay on local disk"
        ));

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
    #[derive(Default)]
    struct FailingUsageEventSink {
        attempts: Mutex<usize>,
    }

    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    #[async_trait::async_trait]
    impl UsageEventSink for FailingUsageEventSink {
        async fn append_usage_event(&self, event: &UsageEvent) -> anyhow::Result<()> {
            self.append_usage_events(std::slice::from_ref(event)).await
        }

        async fn append_usage_events(&self, _events: &[UsageEvent]) -> anyhow::Result<()> {
            *self.attempts.lock().await += 1;
            anyhow::bail!("analytics sink unavailable")
        }
    }

    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    #[derive(Default)]
    struct RecordingUsageRollupSink {
        batches: Mutex<Vec<Vec<String>>>,
        pruned_before_ms: Mutex<Vec<i64>>,
    }

    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    #[async_trait::async_trait]
    impl UsageRollupBatchSink for RecordingUsageRollupSink {
        async fn apply_usage_rollup_batches(
            &self,
            batches: &[UsageRollupBatch],
        ) -> anyhow::Result<UsageRollupApplyReport> {
            self.batches
                .lock()
                .await
                .push(batches.iter().map(|batch| batch.batch_id.clone()).collect());
            Ok(UsageRollupApplyReport {
                applied_batch_count: batches.len(),
                delta_count: batches.iter().map(|batch| batch.deltas.len()).sum(),
                ..UsageRollupApplyReport::default()
            })
        }

        async fn prune_usage_rollup_batch_markers(
            &self,
            applied_before_ms: i64,
        ) -> anyhow::Result<u64> {
            self.pruned_before_ms.lock().await.push(applied_before_ms);
            Ok(0)
        }
    }

    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    #[derive(Default)]
    struct FailingUsageRollupSink {
        attempts: Mutex<usize>,
    }

    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    #[async_trait::async_trait]
    impl UsageRollupBatchSink for FailingUsageRollupSink {
        async fn apply_usage_rollup_batches(
            &self,
            _batches: &[UsageRollupBatch],
        ) -> anyhow::Result<UsageRollupApplyReport> {
            *self.attempts.lock().await += 1;
            anyhow::bail!("rollup sink unavailable")
        }
    }

    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    #[derive(Default)]
    struct DigestMismatchUsageRollupSink {
        attempts: Mutex<usize>,
    }

    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    #[async_trait::async_trait]
    impl UsageRollupBatchSink for DigestMismatchUsageRollupSink {
        async fn apply_usage_rollup_batches(
            &self,
            batches: &[UsageRollupBatch],
        ) -> anyhow::Result<UsageRollupApplyReport> {
            *self.attempts.lock().await += 1;
            let batch_id = batches
                .first()
                .map(|batch| batch.batch_id.clone())
                .unwrap_or_else(|| "unknown".to_string());
            Err(UsageRollupDigestMismatch {
                batch_id,
            }
            .into())
        }
    }

    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    struct RecoveringUsageRollupSink {
        fail_remaining: Mutex<usize>,
        attempts: Mutex<usize>,
        batches: Mutex<Vec<Vec<String>>>,
    }

    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    impl RecoveringUsageRollupSink {
        fn new(failures: usize) -> Self {
            Self {
                fail_remaining: Mutex::new(failures),
                attempts: Mutex::new(0),
                batches: Mutex::new(Vec::new()),
            }
        }
    }

    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    #[async_trait::async_trait]
    impl UsageRollupBatchSink for RecoveringUsageRollupSink {
        async fn apply_usage_rollup_batches(
            &self,
            batches: &[UsageRollupBatch],
        ) -> anyhow::Result<UsageRollupApplyReport> {
            *self.attempts.lock().await += 1;
            let mut fail_remaining = self.fail_remaining.lock().await;
            if *fail_remaining > 0 {
                *fail_remaining -= 1;
                anyhow::bail!("rollup sink temporarily unavailable");
            }
            drop(fail_remaining);
            self.batches
                .lock()
                .await
                .push(batches.iter().map(|batch| batch.batch_id.clone()).collect());
            Ok(UsageRollupApplyReport {
                applied_batch_count: batches.len(),
                delta_count: batches.iter().map(|batch| batch.deltas.len()).sum(),
                ..UsageRollupApplyReport::default()
            })
        }
    }

    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    struct StaticControlStore {
        key: AuthenticatedKey,
    }

    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    #[async_trait::async_trait]
    impl ControlStore for StaticControlStore {
        async fn authenticate_bearer_secret(
            &self,
            secret: &str,
        ) -> anyhow::Result<Option<AuthenticatedKey>> {
            Ok((secret == "secret").then(|| self.key.clone()))
        }

        async fn apply_usage_rollup(&self, _event: &UsageEvent) -> anyhow::Result<()> {
            anyhow::bail!("usage rollups must be persisted by the accounting flusher")
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
            error_message: None,
            error_body: None,
            response_body: None,
            timing: llm_access_core::usage::UsageTiming {
                latency_ms: Some(20),
                ..llm_access_core::usage::UsageTiming::default()
            },
            stream: llm_access_core::usage::UsageStreamDetails::default(),
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
    async fn wait_for_recorded_rollup_batches(sink: &RecordingUsageRollupSink, expected: usize) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if sink.batches.lock().await.len() >= expected {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("usage rollup batch was not flushed");
    }

    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    fn test_journal_sink() -> (tempfile::TempDir, Arc<crate::usage_journal::JournalUsageEventSink>)
    {
        let root = tempfile::tempdir().expect("tempdir");
        let sink = Arc::new(
            crate::usage_journal::JournalUsageEventSink::open_for_tests(root.path().to_path_buf())
                .expect("journal sink"),
        );
        (root, sink)
    }

    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    fn test_rollup_backlog() -> (tempfile::TempDir, super::UsageRollupBacklog) {
        let root = tempfile::tempdir().expect("tempdir");
        let backlog = super::UsageRollupBacklog::open_for_tests(root.path().to_path_buf())
            .expect("rollup backlog");
        (root, backlog)
    }

    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    #[test]
    fn pending_rollups_restores_previous_last_used_after_newer_batch_subtract() {
        let pending_rollups = super::PendingUsageRollups::default();
        let older = UsageRollupBatch {
            batch_id: "older".to_string(),
            source_node_id: Some("node-test".to_string()),
            created_at_ms: 1_700_000_000_000,
            source_event_count: 1,
            deltas: vec![KeyUsageRollupDelta {
                key_id: "key-runtime".to_string(),
                input_uncached_tokens: 10,
                input_cached_tokens: 0,
                output_tokens: 2,
                billable_tokens: 12,
                credit_total: 0.0,
                credit_missing_events: 0,
                last_used_at_ms: Some(1_700_000_000_000),
            }],
            last_used_at_ms_counts: Vec::new(),
        };
        let newer = UsageRollupBatch {
            batch_id: "newer".to_string(),
            source_node_id: Some("node-test".to_string()),
            created_at_ms: 1_700_000_001_000,
            source_event_count: 1,
            deltas: vec![KeyUsageRollupDelta {
                key_id: "key-runtime".to_string(),
                input_uncached_tokens: 20,
                input_cached_tokens: 0,
                output_tokens: 4,
                billable_tokens: 24,
                credit_total: 0.0,
                credit_missing_events: 0,
                last_used_at_ms: Some(1_700_000_001_000),
            }],
            last_used_at_ms_counts: Vec::new(),
        };

        pending_rollups.add_batch(&older).expect("add older");
        pending_rollups.add_batch(&newer).expect("add newer");
        pending_rollups
            .subtract_batch(&newer)
            .expect("subtract newer");

        assert_eq!(
            pending_rollups
                .delta_for_key("key-runtime")
                .expect("pending delta")
                .last_used_at_ms,
            Some(1_700_000_000_000)
        );
    }

    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    #[test]
    fn pending_rollups_subtracts_all_last_used_timestamps_in_aggregated_batch() {
        let pending_rollups = super::PendingUsageRollups::default();
        let mut older = sample_usage_event("evt-last-used-older");
        older.created_at_ms = 1_700_000_000_000;
        older.input_uncached_tokens = 10;
        older.billable_tokens = 10;
        let mut newer = sample_usage_event("evt-last-used-newer");
        newer.created_at_ms = 1_700_000_001_000;
        newer.input_uncached_tokens = 20;
        newer.billable_tokens = 20;
        let events = vec![older, newer];
        let applied_batch = UsageRollupBatch::from_usage_events(
            "combined".to_string(),
            Some("node-test".to_string()),
            1_700_000_002_000,
            &events,
        )
        .expect("aggregate combined rollup batch");

        pending_rollups
            .add_events(std::slice::from_ref(&events[0]))
            .expect("add older event");
        pending_rollups
            .add_events(std::slice::from_ref(&events[1]))
            .expect("add newer event");
        pending_rollups
            .subtract_batch(&applied_batch)
            .expect("subtract combined batch");

        assert_eq!(pending_rollups.delta_for_key("key-runtime"), None);
    }

    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    fn sample_authenticated_key() -> AuthenticatedKey {
        AuthenticatedKey {
            key_id: "key-runtime".to_string(),
            key_name: "runtime".to_string(),
            provider_type: "kiro".to_string(),
            protocol_family: "anthropic".to_string(),
            status: "active".to_string(),
            quota_billable_limit: 100,
            billable_tokens_used: 5,
        }
    }

    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    #[tokio::test]
    async fn flush_drops_analytics_events_after_rollups_are_persisted() {
        let rollup_sink = RecordingUsageRollupSink::default();
        let analytics_sink = FailingUsageEventSink::default();
        let pending_rollups = super::PendingUsageRollups::default();
        let (_backlog_root, mut rollup_backlog) = test_rollup_backlog();
        let mut buffer = vec![sample_usage_event("evt-1"), sample_usage_event("evt-2")];
        pending_rollups
            .add_events(&buffer)
            .expect("add pending rollups");
        let mut analytics_retry_buffer = Vec::new();
        let mut buffered_bytes = buffer
            .iter()
            .map(super::estimate_usage_event_bytes)
            .sum::<usize>();
        let mut analytics_retry_bytes = 0usize;
        let mut flush_count = 0u64;
        let mut rollup_retry = super::UsageRollupRetryState::default();

        super::flush_usage_event_buffer(
            super::UsageEventFlushTargets {
                rollup_sink: &rollup_sink,
                analytics_sink: &analytics_sink,
                pending_rollups: &pending_rollups,
                source_node_id: Some("node-test"),
            },
            super::UsageEventFlushState {
                buffer: &mut buffer,
                analytics_retry_buffer: &mut analytics_retry_buffer,
                buffered_bytes: &mut buffered_bytes,
                analytics_retry_bytes: &mut analytics_retry_bytes,
                max_buffer_bytes: 8 * 1024 * 1024,
                flush_count: &mut flush_count,
                rollup_retry: &mut rollup_retry,
                flush_interval: Duration::from_secs(1),
                rollup_backlog: &mut rollup_backlog,
            },
            "usage event batch flush failed",
        )
        .await;

        assert!(buffer.is_empty());
        assert!(analytics_retry_buffer.is_empty());
        assert_eq!(buffered_bytes, 0);
        assert_eq!(analytics_retry_bytes, 0);
        assert_eq!(flush_count, 0);
        assert_eq!(*analytics_sink.attempts.lock().await, 1);
        assert_eq!(rollup_sink.batches.lock().await.len(), 1);
        assert!(pending_rollups.delta_for_key("key-runtime").is_none());
    }

    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    #[tokio::test]
    async fn usage_flusher_parks_rollup_after_bounded_failures() {
        let rollup_sink = Arc::new(FailingUsageRollupSink::default());
        let analytics_sink = Arc::new(RecordingUsageEventSink::default());
        let (_journal_root, journal_sink) = test_journal_sink();
        let (_backlog_root, rollup_backlog) = test_rollup_backlog();
        let runtime_config = Arc::new(RwLock::new(AdminRuntimeConfig {
            usage_event_flush_batch_size: 1,
            usage_event_flush_interval_seconds: 1,
            usage_event_flush_max_buffer_bytes: 8 * 1024 * 1024,
            ..AdminRuntimeConfig::default()
        }));
        let (sink, handle) = super::UsageAccounting::new(
            rollup_sink.clone(),
            journal_sink,
            analytics_sink.clone(),
            runtime_config,
            rollup_backlog,
            super::PendingUsageRollups::default(),
            Some("node-test".to_string()),
        )
        .expect("usage accounting");

        sink.append_usage_event(&sample_usage_event("evt-rollup-fail"))
            .await
            .expect("enqueue event");

        tokio::time::timeout(Duration::from_secs(4), async {
            loop {
                if *rollup_sink.attempts.lock().await >= super::USAGE_EVENT_ROLLUP_MAX_ATTEMPTS {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("usage rollup was not parked after bounded attempts");

        tokio::time::sleep(Duration::from_millis(1_100)).await;
        assert_eq!(*rollup_sink.attempts.lock().await, super::USAGE_EVENT_ROLLUP_MAX_ATTEMPTS);
        assert_eq!(analytics_sink.batches.lock().await.as_slice(), &[vec![
            "evt-rollup-fail".to_string()
        ]]);
        assert!(sink.pending_rollups.delta_for_key("key-runtime").is_some());
        handle.shutdown().await;
    }

    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    #[tokio::test]
    async fn usage_flusher_retries_parked_rollup_without_new_live_events() {
        let rollup_sink =
            Arc::new(RecoveringUsageRollupSink::new(super::USAGE_EVENT_ROLLUP_MAX_ATTEMPTS));
        let analytics_sink = Arc::new(RecordingUsageEventSink::default());
        let (_journal_root, journal_sink) = test_journal_sink();
        let (_backlog_root, rollup_backlog) = test_rollup_backlog();
        let runtime_config = Arc::new(RwLock::new(AdminRuntimeConfig {
            usage_event_flush_batch_size: 1,
            usage_event_flush_interval_seconds: 1,
            usage_event_flush_max_buffer_bytes: 8 * 1024 * 1024,
            ..AdminRuntimeConfig::default()
        }));
        let (sink, handle) = super::UsageAccounting::new(
            rollup_sink.clone(),
            journal_sink,
            analytics_sink,
            runtime_config,
            rollup_backlog,
            super::PendingUsageRollups::default(),
            Some("node-test".to_string()),
        )
        .expect("usage accounting");

        sink.append_usage_event(&sample_usage_event("evt-rollup-retry-window"))
            .await
            .expect("enqueue event");

        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                if *rollup_sink.attempts.lock().await > super::USAGE_EVENT_ROLLUP_MAX_ATTEMPTS
                    && !rollup_sink.batches.lock().await.is_empty()
                    && sink.pending_rollups.delta_for_key("key-runtime").is_none()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("parked rollup was not retried after retry window elapsed");

        handle.shutdown().await;
    }

    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    #[tokio::test]
    async fn rollup_marker_prune_waits_until_backlog_is_drained() {
        let rollup_sink = RecordingUsageRollupSink::default();
        let (_backlog_root, mut rollup_backlog) = test_rollup_backlog();
        let events = vec![sample_usage_event("evt-prune-wait")];
        let rollup_batch = UsageRollupBatch::from_usage_events(
            "prune-wait-rollup".to_string(),
            Some("node-test".to_string()),
            1_700_000_000_000,
            &events,
        )
        .expect("aggregate rollup batch");
        rollup_backlog
            .append_batches(std::slice::from_ref(&rollup_batch))
            .expect("append parked rollup");
        let now = Instant::now();
        let mut last_prune_at = None;

        super::prune_usage_rollup_batch_markers_if_due(
            &rollup_sink,
            &mut last_prune_at,
            now,
            &rollup_backlog,
        )
        .await;

        assert!(rollup_sink.pruned_before_ms.lock().await.is_empty());
        assert!(last_prune_at.is_none());
        let claim = rollup_backlog
            .claim_next()
            .expect("claim backlog")
            .expect("claim");
        rollup_backlog
            .complete_claim(claim)
            .expect("complete backlog claim");

        super::prune_usage_rollup_batch_markers_if_due(
            &rollup_sink,
            &mut last_prune_at,
            now,
            &rollup_backlog,
        )
        .await;

        assert_eq!(rollup_sink.pruned_before_ms.lock().await.len(), 1);
        assert_eq!(last_prune_at, Some(now));
    }

    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    #[tokio::test]
    async fn parked_rollup_backlog_quarantines_digest_mismatch() {
        let rollup_sink = DigestMismatchUsageRollupSink::default();
        let analytics_sink = RecordingUsageEventSink::default();
        let pending_rollups = super::PendingUsageRollups::default();
        let (_backlog_root, mut rollup_backlog) = test_rollup_backlog();
        let events = vec![sample_usage_event("evt-poison-rollup")];
        let rollup_batch = UsageRollupBatch::from_usage_events(
            "poison-rollup".to_string(),
            Some("node-test".to_string()),
            1_700_000_000_000,
            &events,
        )
        .expect("aggregate rollup batch");
        pending_rollups
            .add_batch(&rollup_batch)
            .expect("add pending rollup");
        rollup_backlog
            .append_batches(std::slice::from_ref(&rollup_batch))
            .expect("append parked rollup");
        let mut buffer = Vec::new();
        let mut analytics_retry_buffer = Vec::new();
        let mut buffered_bytes = 0usize;
        let mut analytics_retry_bytes = 0usize;
        let mut flush_count = 0u64;
        let mut rollup_retry = super::UsageRollupRetryState::default();

        super::flush_usage_event_buffer(
            super::UsageEventFlushTargets {
                rollup_sink: &rollup_sink,
                analytics_sink: &analytics_sink,
                pending_rollups: &pending_rollups,
                source_node_id: Some("node-test"),
            },
            super::UsageEventFlushState {
                buffer: &mut buffer,
                analytics_retry_buffer: &mut analytics_retry_buffer,
                buffered_bytes: &mut buffered_bytes,
                analytics_retry_bytes: &mut analytics_retry_bytes,
                max_buffer_bytes: 8 * 1024 * 1024,
                flush_count: &mut flush_count,
                rollup_retry: &mut rollup_retry,
                flush_interval: Duration::from_secs(1),
                rollup_backlog: &mut rollup_backlog,
            },
            "usage event batch flush failed",
        )
        .await;

        assert_eq!(*rollup_sink.attempts.lock().await, 1);
        assert_eq!(rollup_backlog.sealed_file_count().expect("sealed count"), 0);
        assert!(rollup_backlog
            .read_all_pending_batches()
            .expect("pending batches")
            .is_empty());
        assert!(pending_rollups.delta_for_key("key-runtime").is_none());
    }

    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    #[tokio::test]
    async fn usage_flusher_keeps_accepting_events_when_rollup_retries_fail() {
        let rollup_sink = Arc::new(FailingUsageRollupSink::default());
        let analytics_sink = Arc::new(RecordingUsageEventSink::default());
        let (_journal_root, journal_sink) = test_journal_sink();
        let (_backlog_root, rollup_backlog) = test_rollup_backlog();
        let runtime_config = Arc::new(RwLock::new(AdminRuntimeConfig {
            usage_event_flush_batch_size: 1,
            usage_event_flush_interval_seconds: 1,
            usage_event_flush_max_buffer_bytes: 8 * 1024 * 1024,
            ..AdminRuntimeConfig::default()
        }));
        let (sink, handle) = super::UsageAccounting::new(
            rollup_sink,
            journal_sink,
            analytics_sink.clone(),
            runtime_config,
            rollup_backlog,
            super::PendingUsageRollups::default(),
            Some("node-test".to_string()),
        )
        .expect("usage accounting");

        sink.append_usage_event(&sample_usage_event("evt-rollup-fail-1"))
            .await
            .expect("enqueue first event");
        sink.append_usage_event(&sample_usage_event("evt-rollup-fail-2"))
            .await
            .expect("enqueue second event");

        tokio::time::timeout(Duration::from_millis(1_500), async {
            loop {
                if analytics_sink.batches.lock().await.len() >= 2 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("rollup retry backlog blocked new usage events from analytics");

        let batches = analytics_sink.batches.lock().await;
        assert_eq!(batches.as_slice(), &[vec!["evt-rollup-fail-1".to_string()], vec![
            "evt-rollup-fail-2".to_string()
        ]]);
        assert!(sink.pending_rollups.delta_for_key("key-runtime").is_some());
        handle.shutdown().await;
    }

    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    #[tokio::test]
    async fn parked_rollup_backlog_recovers_after_live_rollup_succeeds() {
        let rollup_sink = RecoveringUsageRollupSink::new(1);
        let analytics_sink = RecordingUsageEventSink::default();
        let pending_rollups = super::PendingUsageRollups::default();
        let (_backlog_root, mut rollup_backlog) = test_rollup_backlog();
        let mut buffer = vec![sample_usage_event("evt-parked")];
        pending_rollups
            .add_events(&buffer)
            .expect("add first pending rollup");
        let mut analytics_retry_buffer = Vec::new();
        let mut buffered_bytes = buffer
            .iter()
            .map(super::estimate_usage_event_bytes)
            .sum::<usize>();
        let mut analytics_retry_bytes = 0usize;
        let mut flush_count = 0u64;
        let mut rollup_retry = super::UsageRollupRetryState::default();

        super::flush_usage_event_buffer(
            super::UsageEventFlushTargets {
                rollup_sink: &rollup_sink,
                analytics_sink: &analytics_sink,
                pending_rollups: &pending_rollups,
                source_node_id: Some("node-test"),
            },
            super::UsageEventFlushState {
                buffer: &mut buffer,
                analytics_retry_buffer: &mut analytics_retry_buffer,
                buffered_bytes: &mut buffered_bytes,
                analytics_retry_bytes: &mut analytics_retry_bytes,
                max_buffer_bytes: 8 * 1024 * 1024,
                flush_count: &mut flush_count,
                rollup_retry: &mut rollup_retry,
                flush_interval: Duration::from_secs(1),
                rollup_backlog: &mut rollup_backlog,
            },
            "usage event batch flush failed",
        )
        .await;

        assert!(!rollup_retry.retry_suspended(Instant::now()));
        assert_eq!(rollup_backlog.sealed_file_count().expect("sealed count"), 1);
        assert!(pending_rollups.delta_for_key("key-runtime").is_some());
        assert_eq!(analytics_sink.batches.lock().await.as_slice(), &[vec![
            "evt-parked".to_string()
        ]]);

        buffer = vec![sample_usage_event("evt-live-recovery")];
        buffered_bytes = buffer
            .iter()
            .map(super::estimate_usage_event_bytes)
            .sum::<usize>();
        pending_rollups
            .add_events(&buffer)
            .expect("add recovery pending rollup");
        super::flush_usage_event_buffer(
            super::UsageEventFlushTargets {
                rollup_sink: &rollup_sink,
                analytics_sink: &analytics_sink,
                pending_rollups: &pending_rollups,
                source_node_id: Some("node-test"),
            },
            super::UsageEventFlushState {
                buffer: &mut buffer,
                analytics_retry_buffer: &mut analytics_retry_buffer,
                buffered_bytes: &mut buffered_bytes,
                analytics_retry_bytes: &mut analytics_retry_bytes,
                max_buffer_bytes: 8 * 1024 * 1024,
                flush_count: &mut flush_count,
                rollup_retry: &mut rollup_retry,
                flush_interval: Duration::from_secs(1),
                rollup_backlog: &mut rollup_backlog,
            },
            "usage event batch flush failed",
        )
        .await;

        assert!(!rollup_retry.retry_suspended(Instant::now()));
        assert_eq!(rollup_backlog.sealed_file_count().expect("sealed count"), 0);
        assert!(pending_rollups.delta_for_key("key-runtime").is_none());
        assert_eq!(rollup_sink.batches.lock().await.len(), 2);
    }

    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    #[tokio::test]
    async fn usage_accounting_buffers_rollups_and_overlays_auth_until_flush() {
        let rollup_sink = Arc::new(RecordingUsageRollupSink::default());
        let analytics_sink = Arc::new(RecordingUsageEventSink::default());
        let (_journal_root, journal_sink) = test_journal_sink();
        let (_backlog_root, rollup_backlog) = test_rollup_backlog();
        let runtime_config = Arc::new(RwLock::new(AdminRuntimeConfig {
            usage_event_flush_batch_size: 2,
            usage_event_flush_interval_seconds: 3600,
            usage_event_flush_max_buffer_bytes: 8 * 1024 * 1024,
            ..AdminRuntimeConfig::default()
        }));
        let (accounting, _handle) = super::UsageAccounting::new(
            rollup_sink.clone(),
            journal_sink,
            analytics_sink.clone(),
            runtime_config,
            rollup_backlog,
            super::PendingUsageRollups::default(),
            Some("node-test".to_string()),
        )
        .expect("usage accounting");
        let control_store = super::UsageAccountingControlStore::new(
            Arc::new(StaticControlStore {
                key: sample_authenticated_key(),
            }),
            accounting.clone(),
        );

        accounting
            .append_usage_event(&sample_usage_event("evt-1"))
            .await
            .expect("enqueue first event");
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert!(rollup_sink.batches.lock().await.is_empty());
        assert!(analytics_sink.batches.lock().await.is_empty());
        let key = control_store
            .authenticate_bearer_secret("secret")
            .await
            .expect("authenticate")
            .expect("key exists");
        assert_eq!(key.billable_tokens_used, 17);

        accounting
            .append_usage_event(&sample_usage_event("evt-2"))
            .await
            .expect("enqueue second event");
        wait_for_recorded_rollup_batches(&rollup_sink, 1).await;
        wait_for_recorded_batches(&analytics_sink, 1).await;

        assert_eq!(rollup_sink.batches.lock().await.len(), 1);
        assert_eq!(analytics_sink.batches.lock().await.as_slice(), &[vec![
            "evt-1".to_string(),
            "evt-2".to_string()
        ]]);
    }

    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    #[tokio::test]
    async fn usage_accounting_loads_initial_rollup_backlog_into_overlay() {
        let rollup_sink = Arc::new(RecordingUsageRollupSink::default());
        let analytics_sink = Arc::new(RecordingUsageEventSink::default());
        let (_journal_root, journal_sink) = test_journal_sink();
        let (_backlog_root, rollup_backlog) = test_rollup_backlog();
        let runtime_config = Arc::new(RwLock::new(AdminRuntimeConfig::default()));
        let initial_batch = UsageRollupBatch {
            batch_id: "startup-backlog-1".to_string(),
            source_node_id: Some("node-test".to_string()),
            created_at_ms: 1_700_000_000_000,
            source_event_count: 1,
            deltas: vec![KeyUsageRollupDelta {
                key_id: "key-runtime".to_string(),
                input_uncached_tokens: 10,
                input_cached_tokens: 0,
                output_tokens: 2,
                billable_tokens: 12,
                credit_total: 0.0,
                credit_missing_events: 0,
                last_used_at_ms: Some(1_700_000_000_000),
            }],
            last_used_at_ms_counts: Vec::new(),
        };
        let initial_pending_rollups = super::PendingUsageRollups::default();
        initial_pending_rollups
            .add_batch(&initial_batch)
            .expect("add initial pending batch");
        let (accounting, _handle) = super::UsageAccounting::new(
            rollup_sink,
            journal_sink,
            analytics_sink,
            runtime_config,
            rollup_backlog,
            initial_pending_rollups,
            Some("node-test".to_string()),
        )
        .expect("usage accounting");
        let control_store = super::UsageAccountingControlStore::new(
            Arc::new(StaticControlStore {
                key: sample_authenticated_key(),
            }),
            accounting,
        );

        let key = control_store
            .authenticate_bearer_secret("secret")
            .await
            .expect("authenticate")
            .expect("key exists");

        assert_eq!(key.billable_tokens_used, 17);
    }

    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    #[tokio::test]
    async fn batched_usage_sink_flushes_when_batch_size_reached() {
        let rollup_sink = Arc::new(RecordingUsageRollupSink::default());
        let analytics_sink = Arc::new(RecordingUsageEventSink::default());
        let (_journal_root, journal_sink) = test_journal_sink();
        let (_backlog_root, rollup_backlog) = test_rollup_backlog();
        let runtime_config = Arc::new(RwLock::new(AdminRuntimeConfig {
            usage_event_flush_batch_size: 2,
            usage_event_flush_interval_seconds: 3600,
            usage_event_flush_max_buffer_bytes: 8 * 1024 * 1024,
            ..AdminRuntimeConfig::default()
        }));
        let (sink, _handle) = super::UsageAccounting::new(
            rollup_sink.clone(),
            journal_sink,
            analytics_sink.clone(),
            runtime_config,
            rollup_backlog,
            super::PendingUsageRollups::default(),
            Some("node-test".to_string()),
        )
        .expect("usage accounting");

        sink.append_usage_event(&sample_usage_event("evt-1"))
            .await
            .expect("enqueue first event");
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(rollup_sink.batches.lock().await.is_empty());
        assert!(analytics_sink.batches.lock().await.is_empty());

        sink.append_usage_event(&sample_usage_event("evt-2"))
            .await
            .expect("enqueue second event");
        wait_for_recorded_rollup_batches(&rollup_sink, 1).await;
        wait_for_recorded_batches(&analytics_sink, 1).await;

        let batches = analytics_sink.batches.lock().await;
        assert_eq!(batches.as_slice(), &[vec!["evt-1".to_string(), "evt-2".to_string()]]);
    }

    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    #[tokio::test]
    async fn batched_usage_sink_flushes_remaining_events_when_sender_closes() {
        let rollup_sink = Arc::new(RecordingUsageRollupSink::default());
        let analytics_sink = Arc::new(RecordingUsageEventSink::default());
        let (_journal_root, journal_sink) = test_journal_sink();
        let (_backlog_root, rollup_backlog) = test_rollup_backlog();
        let runtime_config = Arc::new(RwLock::new(AdminRuntimeConfig {
            usage_event_flush_batch_size: 256,
            usage_event_flush_interval_seconds: 3600,
            usage_event_flush_max_buffer_bytes: 8 * 1024 * 1024,
            ..AdminRuntimeConfig::default()
        }));
        let (sink, _handle) = super::UsageAccounting::new(
            rollup_sink,
            journal_sink,
            analytics_sink.clone(),
            runtime_config,
            rollup_backlog,
            super::PendingUsageRollups::default(),
            Some("node-test".to_string()),
        )
        .expect("usage accounting");

        sink.append_usage_event(&sample_usage_event("evt-1"))
            .await
            .expect("enqueue event");
        drop(sink);
        wait_for_recorded_batches(&analytics_sink, 1).await;

        let batches = analytics_sink.batches.lock().await;
        assert_eq!(batches.as_slice(), &[vec!["evt-1".to_string()]]);
    }

    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    #[tokio::test]
    async fn batched_usage_sink_flushes_remaining_events_on_shutdown_signal() {
        let rollup_sink = Arc::new(RecordingUsageRollupSink::default());
        let analytics_sink = Arc::new(RecordingUsageEventSink::default());
        let (_journal_root, journal_sink) = test_journal_sink();
        let (_backlog_root, rollup_backlog) = test_rollup_backlog();
        let runtime_config = Arc::new(RwLock::new(AdminRuntimeConfig {
            usage_event_flush_batch_size: 256,
            usage_event_flush_interval_seconds: 3600,
            usage_event_flush_max_buffer_bytes: 8 * 1024 * 1024,
            ..AdminRuntimeConfig::default()
        }));
        let (sink, handle) = super::UsageAccounting::new(
            rollup_sink,
            journal_sink,
            analytics_sink.clone(),
            runtime_config,
            rollup_backlog,
            super::PendingUsageRollups::default(),
            Some("node-test".to_string()),
        )
        .expect("usage accounting");

        sink.append_usage_event(&sample_usage_event("evt-1"))
            .await
            .expect("enqueue event");
        handle.shutdown().await;

        let batches = analytics_sink.batches.lock().await;
        assert_eq!(batches.as_slice(), &[vec!["evt-1".to_string()]]);
    }
}
