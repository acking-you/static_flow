use std::{
    collections::{BTreeMap, HashMap, HashSet},
    env,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{anyhow, Result};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use static_flow_shared::{
    article_request_store::{self, ArticleRequestStore},
    comments_store::{self, CommentDataStore},
    interactive_store::{self, InteractivePageStore},
    lancedb_api::{
        CategoryInfo, NewApiBehaviorEventInput, StaticFlowDataStore, StatsResponse, TagInfo,
    },
    llm_gateway_store::{
        self, default_kiro_cache_kmodels, default_kiro_cache_kmodels_json, LlmGatewayStore,
        DEFAULT_CODEX_STATUS_ACCOUNT_JITTER_MAX_SECONDS,
        DEFAULT_CODEX_STATUS_REFRESH_MAX_INTERVAL_SECONDS,
        DEFAULT_CODEX_STATUS_REFRESH_MIN_INTERVAL_SECONDS, DEFAULT_KIRO_CHANNEL_MAX_CONCURRENCY,
        DEFAULT_KIRO_CHANNEL_MIN_START_INTERVAL_MS, DEFAULT_KIRO_STATUS_ACCOUNT_JITTER_MAX_SECONDS,
        DEFAULT_KIRO_STATUS_REFRESH_MAX_INTERVAL_SECONDS,
        DEFAULT_KIRO_STATUS_REFRESH_MIN_INTERVAL_SECONDS,
        DEFAULT_LLM_GATEWAY_ACCOUNT_FAILURE_RETRY_LIMIT,
        DEFAULT_LLM_GATEWAY_AUTH_CACHE_TTL_SECONDS, DEFAULT_LLM_GATEWAY_MAX_REQUEST_BODY_BYTES,
        DEFAULT_LLM_GATEWAY_USAGE_EVENT_FLUSH_BATCH_SIZE,
        DEFAULT_LLM_GATEWAY_USAGE_EVENT_FLUSH_INTERVAL_SECONDS,
        DEFAULT_LLM_GATEWAY_USAGE_EVENT_FLUSH_MAX_BUFFER_BYTES,
    },
    music_store::{self, MusicDataStore},
    music_wish_store::{self, MusicWishStore},
    optimize::{scan_and_compact_tables, CompactConfig},
};
use tokio::sync::{mpsc, watch};

use crate::{
    article_request_worker::{self, ArticleRequestWorkerConfig},
    comment_worker::{self, CommentAiWorkerConfig},
    email::EmailNotifier,
    geoip::GeoIpResolver,
    kiro_gateway::KiroGatewayRuntimeState,
    llm_gateway::LlmGatewayRuntimeState,
    music_wish_worker::{self, MusicWishWorkerConfig},
    public_submit_guard::PublicSubmitGuard,
    upstream_proxy::UpstreamProxyRegistry,
};

type ListCacheEntry<T> = Option<(Vec<T>, Instant)>;
type SharedListCache<T> = Arc<RwLock<ListCacheEntry<T>>>;
type ValueCacheEntry<T> = Option<(T, Instant)>;
type SharedValueCache<T> = Arc<RwLock<ValueCacheEntry<T>>>;

pub const DEFAULT_VIEW_DEDUPE_WINDOW_SECONDS: u64 = 60;
pub const DEFAULT_VIEW_TREND_DAYS: usize = 30;
pub const DEFAULT_VIEW_TREND_MAX_DAYS: usize = 180;
pub const MAX_CONFIGURABLE_VIEW_DEDUPE_WINDOW_SECONDS: u64 = 3600;
pub const MAX_CONFIGURABLE_VIEW_TREND_DAYS: usize = 365;
pub const DEFAULT_COMMENT_SUBMIT_RATE_LIMIT_SECONDS: u64 = 60;
pub const MAX_CONFIGURABLE_COMMENT_RATE_LIMIT_SECONDS: u64 = 3600;
pub const DEFAULT_COMMENT_LIST_LIMIT: usize = 20;
pub const MAX_CONFIGURABLE_COMMENT_LIST_LIMIT: usize = 200;
pub const DEFAULT_COMMENT_CLEANUP_RETENTION_DAYS: i64 = -1;
pub const MAX_CONFIGURABLE_COMMENT_CLEANUP_RETENTION_DAYS: i64 = 3650;
pub const DEFAULT_API_BEHAVIOR_RETENTION_DAYS: i64 = 90;
pub const DEFAULT_API_BEHAVIOR_DEFAULT_DAYS: usize = 30;
pub const DEFAULT_API_BEHAVIOR_MAX_DAYS: usize = 180;
pub const MAX_CONFIGURABLE_API_BEHAVIOR_RETENTION_DAYS: i64 = 3650;
pub const MAX_CONFIGURABLE_API_BEHAVIOR_DAYS: usize = 365;
pub const DEFAULT_API_BEHAVIOR_FLUSH_BATCH_SIZE: usize = 256;
pub const DEFAULT_API_BEHAVIOR_FLUSH_INTERVAL_SECS: u64 = 15;
pub const DEFAULT_API_BEHAVIOR_FLUSH_MAX_BUFFER_BYTES: usize = 4 * 1024 * 1024;
pub const MIN_CONFIGURABLE_API_BEHAVIOR_FLUSH_BATCH_SIZE: usize = 1;
pub const MAX_CONFIGURABLE_API_BEHAVIOR_FLUSH_BATCH_SIZE: usize = 16_384;
pub const MIN_CONFIGURABLE_API_BEHAVIOR_FLUSH_INTERVAL_SECS: u64 = 1;
pub const MAX_CONFIGURABLE_API_BEHAVIOR_FLUSH_INTERVAL_SECS: u64 = 3_600;
pub const MIN_CONFIGURABLE_API_BEHAVIOR_FLUSH_MAX_BUFFER_BYTES: usize = 1_024;
pub const MAX_CONFIGURABLE_API_BEHAVIOR_FLUSH_MAX_BUFFER_BYTES: usize = 256 * 1024 * 1024;
pub const DEFAULT_TABLE_COMPACT_ENABLED: bool = true;
pub const DEFAULT_TABLE_COMPACT_SCAN_INTERVAL_SECS: u64 = 900;
pub const MIN_CONFIGURABLE_TABLE_COMPACT_SCAN_INTERVAL_SECS: u64 = 30;
pub const MAX_CONFIGURABLE_TABLE_COMPACT_SCAN_INTERVAL_SECS: u64 = 86_400;
pub const DEFAULT_TABLE_COMPACT_FRAGMENT_THRESHOLD: usize = 128;
pub const MIN_CONFIGURABLE_TABLE_COMPACT_FRAGMENT_THRESHOLD: usize = 2;
pub const MAX_CONFIGURABLE_TABLE_COMPACT_FRAGMENT_THRESHOLD: usize = 10_000;
pub const DEFAULT_TABLE_COMPACT_PRUNE_OLDER_THAN_HOURS: i64 = 1;
pub const MIN_CONFIGURABLE_TABLE_COMPACT_PRUNE_OLDER_THAN_HOURS: i64 = 1;
pub const MAX_CONFIGURABLE_TABLE_COMPACT_PRUNE_OLDER_THAN_HOURS: i64 = 8_760;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewAnalyticsRuntimeConfig {
    pub dedupe_window_seconds: u64,
    pub trend_default_days: usize,
    pub trend_max_days: usize,
}

impl Default for ViewAnalyticsRuntimeConfig {
    fn default() -> Self {
        Self {
            dedupe_window_seconds: DEFAULT_VIEW_DEDUPE_WINDOW_SECONDS,
            trend_default_days: DEFAULT_VIEW_TREND_DAYS,
            trend_max_days: DEFAULT_VIEW_TREND_MAX_DAYS,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommentRuntimeConfig {
    pub submit_rate_limit_seconds: u64,
    pub list_default_limit: usize,
    pub cleanup_retention_days: i64,
}

impl Default for CommentRuntimeConfig {
    fn default() -> Self {
        Self {
            submit_rate_limit_seconds: DEFAULT_COMMENT_SUBMIT_RATE_LIMIT_SECONDS,
            list_default_limit: DEFAULT_COMMENT_LIST_LIMIT,
            cleanup_retention_days: DEFAULT_COMMENT_CLEANUP_RETENTION_DAYS,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiBehaviorRuntimeConfig {
    pub retention_days: i64,
    pub default_days: usize,
    pub max_days: usize,
    pub flush_batch_size: usize,
    pub flush_interval_seconds: u64,
    pub flush_max_buffer_bytes: usize,
}

impl Default for ApiBehaviorRuntimeConfig {
    fn default() -> Self {
        Self {
            retention_days: DEFAULT_API_BEHAVIOR_RETENTION_DAYS,
            default_days: DEFAULT_API_BEHAVIOR_DEFAULT_DAYS,
            max_days: DEFAULT_API_BEHAVIOR_MAX_DAYS,
            flush_batch_size: DEFAULT_API_BEHAVIOR_FLUSH_BATCH_SIZE,
            flush_interval_seconds: DEFAULT_API_BEHAVIOR_FLUSH_INTERVAL_SECS,
            flush_max_buffer_bytes: DEFAULT_API_BEHAVIOR_FLUSH_MAX_BUFFER_BYTES,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MusicRuntimeConfig {
    pub play_dedupe_window_seconds: u64,
    pub comment_rate_limit_seconds: u64,
    pub list_default_limit: usize,
}

impl Default for MusicRuntimeConfig {
    fn default() -> Self {
        Self {
            play_dedupe_window_seconds: 60,
            comment_rate_limit_seconds: 60,
            list_default_limit: 20,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionRuntimeConfig {
    pub enabled: bool,
    pub scan_interval_seconds: u64,
    pub fragment_threshold: usize,
    pub prune_older_than_hours: i64,
}

impl Default for CompactionRuntimeConfig {
    fn default() -> Self {
        Self {
            enabled: DEFAULT_TABLE_COMPACT_ENABLED,
            scan_interval_seconds: DEFAULT_TABLE_COMPACT_SCAN_INTERVAL_SECS,
            fragment_threshold: DEFAULT_TABLE_COMPACT_FRAGMENT_THRESHOLD,
            prune_older_than_hours: DEFAULT_TABLE_COMPACT_PRUNE_OLDER_THAN_HOURS,
        }
    }
}

/// Runtime knobs for the public LLM gateway and its in-memory auth cache.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmGatewayRuntimeConfig {
    pub auth_cache_ttl_seconds: u64,
    /// Maximum allowed request body size in bytes for proxied gateway calls.
    pub max_request_body_bytes: u64,
    /// Number of consecutive Codex refresh failures tolerated before marking
    /// one account unavailable.
    pub account_failure_retry_limit: u64,
    /// Maximum number of concurrent Kiro upstream requests.
    pub kiro_channel_max_concurrency: u64,
    /// Minimum milliseconds between consecutive Kiro upstream request starts.
    pub kiro_channel_min_start_interval_ms: u64,
    pub codex_status_refresh_min_interval_seconds: u64,
    pub codex_status_refresh_max_interval_seconds: u64,
    pub codex_status_account_jitter_max_seconds: u64,
    pub kiro_status_refresh_min_interval_seconds: u64,
    pub kiro_status_refresh_max_interval_seconds: u64,
    pub kiro_status_account_jitter_max_seconds: u64,
    pub usage_event_flush_batch_size: u64,
    pub usage_event_flush_interval_seconds: u64,
    pub usage_event_flush_max_buffer_bytes: u64,
    pub kiro_cache_kmodels_json: String,
    pub kiro_cache_kmodels: BTreeMap<String, f64>,
}

impl Default for LlmGatewayRuntimeConfig {
    fn default() -> Self {
        Self {
            auth_cache_ttl_seconds: DEFAULT_LLM_GATEWAY_AUTH_CACHE_TTL_SECONDS,
            max_request_body_bytes: DEFAULT_LLM_GATEWAY_MAX_REQUEST_BODY_BYTES,
            account_failure_retry_limit: DEFAULT_LLM_GATEWAY_ACCOUNT_FAILURE_RETRY_LIMIT,
            kiro_channel_max_concurrency: DEFAULT_KIRO_CHANNEL_MAX_CONCURRENCY,
            kiro_channel_min_start_interval_ms: DEFAULT_KIRO_CHANNEL_MIN_START_INTERVAL_MS,
            codex_status_refresh_min_interval_seconds:
                DEFAULT_CODEX_STATUS_REFRESH_MIN_INTERVAL_SECONDS,
            codex_status_refresh_max_interval_seconds:
                DEFAULT_CODEX_STATUS_REFRESH_MAX_INTERVAL_SECONDS,
            codex_status_account_jitter_max_seconds:
                DEFAULT_CODEX_STATUS_ACCOUNT_JITTER_MAX_SECONDS,
            kiro_status_refresh_min_interval_seconds:
                DEFAULT_KIRO_STATUS_REFRESH_MIN_INTERVAL_SECONDS,
            kiro_status_refresh_max_interval_seconds:
                DEFAULT_KIRO_STATUS_REFRESH_MAX_INTERVAL_SECONDS,
            kiro_status_account_jitter_max_seconds: DEFAULT_KIRO_STATUS_ACCOUNT_JITTER_MAX_SECONDS,
            usage_event_flush_batch_size: DEFAULT_LLM_GATEWAY_USAGE_EVENT_FLUSH_BATCH_SIZE,
            usage_event_flush_interval_seconds:
                DEFAULT_LLM_GATEWAY_USAGE_EVENT_FLUSH_INTERVAL_SECONDS,
            usage_event_flush_max_buffer_bytes:
                DEFAULT_LLM_GATEWAY_USAGE_EVENT_FLUSH_MAX_BUFFER_BYTES,
            kiro_cache_kmodels_json: default_kiro_cache_kmodels_json(),
            kiro_cache_kmodels: default_kiro_cache_kmodels(),
        }
    }
}

pub fn parse_kiro_cache_kmodels_json(value: &str) -> Result<BTreeMap<String, f64>> {
    let map: BTreeMap<String, f64> =
        serde_json::from_str(value).map_err(|err| anyhow!("invalid json: {err}"))?;
    if map.is_empty() {
        return Err(anyhow!("kmodel map must not be empty"));
    }
    for (model, coeff) in &map {
        if model.trim().is_empty() {
            return Err(anyhow!("kmodel entry has empty model name"));
        }
        if !coeff.is_finite() || *coeff <= 0.0 {
            return Err(anyhow!("kmodel entry `{model}` must be a positive finite number"));
        }
    }
    Ok(map)
}

#[derive(Debug, Clone)]
pub struct AdminAccessConfig {
    pub local_only: bool,
    pub token: Option<String>,
}

/// Stores that participate in the periodic table compaction loop.
#[derive(Clone)]
struct TableCompactorStores {
    content_store: Arc<StaticFlowDataStore>,
    comment_store: Arc<CommentDataStore>,
    music_store: Arc<MusicDataStore>,
    music_wish_store: Arc<MusicWishStore>,
    article_request_store: Arc<ArticleRequestStore>,
    interactive_store: Arc<InteractivePageStore>,
    llm_gateway_store: Arc<LlmGatewayStore>,
}

#[derive(Clone)]
pub struct AppState {
    pub(crate) store: Arc<StaticFlowDataStore>,
    pub(crate) comment_store: Arc<CommentDataStore>,
    pub(crate) geoip: GeoIpResolver,
    pub(crate) tags_cache: SharedListCache<TagInfo>,
    pub(crate) categories_cache: SharedListCache<CategoryInfo>,
    pub(crate) stats_cache: SharedValueCache<StatsResponse>,
    pub(crate) view_analytics_config: Arc<RwLock<ViewAnalyticsRuntimeConfig>>,
    pub(crate) comment_runtime_config: Arc<RwLock<CommentRuntimeConfig>>,
    pub(crate) api_behavior_runtime_config: Arc<RwLock<ApiBehaviorRuntimeConfig>>,
    pub(crate) compaction_runtime_config: Arc<RwLock<CompactionRuntimeConfig>>,
    pub(crate) llm_gateway_runtime_config: Arc<RwLock<LlmGatewayRuntimeConfig>>,
    pub(crate) comment_submit_guard: Arc<PublicSubmitGuard>,
    pub(crate) comment_worker_tx: mpsc::Sender<String>,
    pub(crate) admin_access: AdminAccessConfig,
    pub(crate) music_store: Arc<MusicDataStore>,
    pub(crate) music_play_dedupe_guard: Arc<RwLock<HashMap<String, i64>>>,
    pub(crate) music_comment_guard: Arc<RwLock<HashMap<String, i64>>>,
    pub(crate) music_runtime_config: Arc<RwLock<MusicRuntimeConfig>>,
    pub(crate) music_wish_store: Arc<MusicWishStore>,
    pub(crate) music_wish_worker_tx: mpsc::Sender<String>,
    pub(crate) music_wish_submit_guard: Arc<PublicSubmitGuard>,
    pub(crate) article_request_store: Arc<ArticleRequestStore>,
    pub(crate) article_request_worker_tx: mpsc::Sender<String>,
    pub(crate) article_request_submit_guard: Arc<PublicSubmitGuard>,
    pub(crate) llm_gateway_public_submit_guard: Arc<PublicSubmitGuard>,
    pub(crate) interactive_store: Arc<InteractivePageStore>,
    pub(crate) llm_gateway_store: Arc<LlmGatewayStore>,
    pub(crate) upstream_proxy_registry: Arc<UpstreamProxyRegistry>,
    pub(crate) llm_gateway: Arc<LlmGatewayRuntimeState>,
    /// Kiro (Anthropic-protocol) gateway runtime state and request scheduler.
    pub(crate) kiro_gateway: Arc<KiroGatewayRuntimeState>,
    pub(crate) email_notifier: Option<Arc<EmailNotifier>>,
    pub(crate) behavior_event_tx: mpsc::Sender<NewApiBehaviorEventInput>,
    pub(crate) shutdown_tx: watch::Sender<bool>,
    pub(crate) shutdown_rx: watch::Receiver<bool>,
    pub(crate) index_html_template: Arc<String>,
}

impl AppState {
    pub async fn new(
        content_db_uri: &str,
        comments_db_uri: &str,
        music_db_uri: &str,
        index_html_template: String,
    ) -> Result<Self> {
        tracing::info!(
            content_db_uri,
            comments_db_uri,
            music_db_uri,
            "initializing application state"
        );
        let store = Arc::new(StaticFlowDataStore::connect(content_db_uri).await?);
        let comment_store = Arc::new(CommentDataStore::connect(comments_db_uri).await?);
        let music_store = Arc::new(MusicDataStore::connect(music_db_uri).await?);
        let music_wish_store = Arc::new(MusicWishStore::connect(music_db_uri).await?);
        let article_request_store = Arc::new(ArticleRequestStore::connect(content_db_uri).await?);
        let interactive_store = Arc::new(InteractivePageStore::connect(content_db_uri).await?);
        let llm_gateway_store = Arc::new(LlmGatewayStore::connect(content_db_uri).await?);
        let upstream_proxy_registry =
            Arc::new(UpstreamProxyRegistry::new(llm_gateway_store.clone()).await?);
        let geoip = GeoIpResolver::from_env()?;
        geoip.warmup().await;
        let email_notifier = EmailNotifier::from_env()?.map(Arc::new);

        let comment_runtime_config = Arc::new(RwLock::new(read_comment_runtime_config_from_env()));
        let api_behavior_runtime_config =
            Arc::new(RwLock::new(read_api_behavior_runtime_config_from_env()));
        let compaction_runtime_config =
            Arc::new(RwLock::new(read_compaction_runtime_config_from_env()));
        let llm_gateway_runtime_config_record =
            llm_gateway_store.get_runtime_config_or_default().await?;
        let llm_gateway_auth_cache_ttl_seconds =
            llm_gateway_runtime_config_record.auth_cache_ttl_seconds;
        let llm_gateway_max_request_body_bytes =
            llm_gateway_runtime_config_record.max_request_body_bytes;
        let llm_gateway_account_failure_retry_limit =
            llm_gateway_runtime_config_record.account_failure_retry_limit;
        let kiro_channel_max_concurrency =
            llm_gateway_runtime_config_record.kiro_channel_max_concurrency;
        let kiro_channel_min_start_interval_ms =
            llm_gateway_runtime_config_record.kiro_channel_min_start_interval_ms;
        let codex_status_refresh_min_interval_seconds =
            llm_gateway_runtime_config_record.codex_status_refresh_min_interval_seconds;
        let codex_status_refresh_max_interval_seconds =
            llm_gateway_runtime_config_record.codex_status_refresh_max_interval_seconds;
        let codex_status_account_jitter_max_seconds =
            llm_gateway_runtime_config_record.codex_status_account_jitter_max_seconds;
        let kiro_status_refresh_min_interval_seconds =
            llm_gateway_runtime_config_record.kiro_status_refresh_min_interval_seconds;
        let kiro_status_refresh_max_interval_seconds =
            llm_gateway_runtime_config_record.kiro_status_refresh_max_interval_seconds;
        let kiro_status_account_jitter_max_seconds =
            llm_gateway_runtime_config_record.kiro_status_account_jitter_max_seconds;
        let usage_event_flush_batch_size =
            llm_gateway_runtime_config_record.usage_event_flush_batch_size;
        let usage_event_flush_interval_seconds =
            llm_gateway_runtime_config_record.usage_event_flush_interval_seconds;
        let usage_event_flush_max_buffer_bytes =
            llm_gateway_runtime_config_record.usage_event_flush_max_buffer_bytes;
        let kiro_cache_kmodels_json = llm_gateway_runtime_config_record.kiro_cache_kmodels_json;
        let kiro_cache_kmodels = parse_kiro_cache_kmodels_json(&kiro_cache_kmodels_json)
            .unwrap_or_else(|err| {
                tracing::warn!(
                    error = %err,
                    "invalid kiro cache kmodels json in runtime config; falling back to defaults"
                );
                default_kiro_cache_kmodels()
            });
        tracing::info!(
            auth_cache_ttl_seconds = llm_gateway_auth_cache_ttl_seconds,
            max_request_body_bytes = llm_gateway_max_request_body_bytes,
            account_failure_retry_limit = llm_gateway_account_failure_retry_limit,
            kiro_channel_max_concurrency,
            kiro_channel_min_start_interval_ms,
            codex_status_refresh_min_interval_seconds,
            codex_status_refresh_max_interval_seconds,
            codex_status_account_jitter_max_seconds,
            kiro_status_refresh_min_interval_seconds,
            kiro_status_refresh_max_interval_seconds,
            kiro_status_account_jitter_max_seconds,
            usage_event_flush_batch_size,
            usage_event_flush_interval_seconds,
            usage_event_flush_max_buffer_bytes,
            kiro_cache_kmodels_json,
            "loaded llm gateway runtime config from storage"
        );
        let llm_gateway_runtime_config = Arc::new(RwLock::new(LlmGatewayRuntimeConfig {
            auth_cache_ttl_seconds: llm_gateway_auth_cache_ttl_seconds,
            max_request_body_bytes: llm_gateway_max_request_body_bytes,
            account_failure_retry_limit: llm_gateway_account_failure_retry_limit,
            kiro_channel_max_concurrency,
            kiro_channel_min_start_interval_ms,
            codex_status_refresh_min_interval_seconds,
            codex_status_refresh_max_interval_seconds,
            codex_status_account_jitter_max_seconds,
            kiro_status_refresh_min_interval_seconds,
            kiro_status_refresh_max_interval_seconds,
            kiro_status_account_jitter_max_seconds,
            usage_event_flush_batch_size,
            usage_event_flush_interval_seconds,
            usage_event_flush_max_buffer_bytes,
            kiro_cache_kmodels_json,
            kiro_cache_kmodels,
        }));
        let auths_dir = crate::llm_gateway::resolve_auths_dir();
        let account_pool = Arc::new(crate::llm_gateway::AccountPool::new(auths_dir.clone()));
        let loaded_accounts = account_pool.load_all().await.unwrap_or_else(|err| {
            tracing::warn!("failed to load codex accounts from {}: {err:#}", auths_dir.display());
            0
        });
        tracing::info!(
            auths_dir = %auths_dir.display(),
            loaded_accounts,
            "initialized codex account pool"
        );
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let llm_gateway = Arc::new(LlmGatewayRuntimeState::new(
            llm_gateway_store.clone(),
            llm_gateway_runtime_config.clone(),
            account_pool.clone(),
            upstream_proxy_registry.clone(),
            shutdown_rx.clone(),
        )?);
        llm_gateway.rebuild_usage_rollups().await?;
        llm_gateway.rebuild_usage_event_counts().await?;
        let kiro_gateway = Arc::new(
            KiroGatewayRuntimeState::new(
                llm_gateway_store.clone(),
                llm_gateway_runtime_config.clone(),
                upstream_proxy_registry.clone(),
            )
            .await?,
        );
        tracing::info!(
            auth_cache_ttl_seconds = llm_gateway_auth_cache_ttl_seconds,
            max_request_body_bytes = llm_gateway_max_request_body_bytes,
            account_failure_retry_limit = llm_gateway_account_failure_retry_limit,
            kiro_channel_max_concurrency,
            kiro_channel_min_start_interval_ms,
            codex_status_refresh_min_interval_seconds,
            codex_status_refresh_max_interval_seconds,
            codex_status_account_jitter_max_seconds,
            kiro_status_refresh_min_interval_seconds,
            kiro_status_refresh_max_interval_seconds,
            kiro_status_account_jitter_max_seconds,
            usage_event_flush_batch_size,
            usage_event_flush_interval_seconds,
            usage_event_flush_max_buffer_bytes,
            kiro_cache_kmodels_json = %llm_gateway_runtime_config.read().kiro_cache_kmodels_json,
            "initialized llm gateway runtime state"
        );
        let comment_worker_tx = comment_worker::spawn_comment_worker(
            comment_store.clone(),
            CommentAiWorkerConfig::from_env(content_db_uri.to_string()),
        );
        let music_wish_worker_tx = music_wish_worker::spawn_music_wish_worker(
            music_wish_store.clone(),
            MusicWishWorkerConfig::from_env(music_db_uri.to_string()),
            email_notifier.clone(),
        );
        let article_request_worker_tx = article_request_worker::spawn_article_request_worker(
            article_request_store.clone(),
            ArticleRequestWorkerConfig::from_env(content_db_uri.to_string()),
            email_notifier.clone(),
        );
        let admin_access = AdminAccessConfig {
            local_only: parse_bool_env("ADMIN_LOCAL_ONLY", true),
            token: env::var("ADMIN_TOKEN")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
        };
        tracing::info!(
            admin_local_only = admin_access.local_only,
            admin_token_configured = admin_access.token.is_some(),
            "resolved admin access configuration"
        );

        let behavior_event_tx = spawn_behavior_event_flusher(
            store.clone(),
            api_behavior_runtime_config.clone(),
            shutdown_rx.clone(),
        );
        let llm_gateway_warmup = llm_gateway.clone();
        let llm_gateway_warmup_runtime_config = llm_gateway_runtime_config.clone();
        let llm_gateway_warmup_proxy_registry = upstream_proxy_registry.clone();
        let mut llm_gateway_warmup_shutdown_rx = shutdown_rx.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = llm_gateway_warmup_shutdown_rx.changed() => {
                    if *llm_gateway_warmup_shutdown_rx.borrow() {
                        tracing::info!("Initial LLM gateway warmup cancelled during shutdown");
                    }
                }
                _ = async {
                    if let Err(err) = crate::llm_gateway::token_refresh::refresh_all_accounts_once(
                        llm_gateway_warmup.account_pool.as_ref(),
                        llm_gateway_warmup_proxy_registry.as_ref(),
                        llm_gateway_warmup_runtime_config.as_ref(),
                    )
                    .await
                    {
                        tracing::warn!("Initial Codex account usage refresh failed: {err:#}");
                    }
                    if let Err(err) =
                        crate::llm_gateway::refresh_public_rate_limit_status(&llm_gateway_warmup)
                            .await
                    {
                        tracing::warn!("Initial LLM gateway rate-limit refresh failed: {err:#}");
                    }
                } => {}
            }
        });
        crate::llm_gateway::spawn_public_rate_limit_refresher(
            llm_gateway.clone(),
            shutdown_rx.clone(),
        );
        let kiro_gateway_warmup = kiro_gateway.clone();
        let mut kiro_gateway_warmup_shutdown_rx = shutdown_rx.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = kiro_gateway_warmup_shutdown_rx.changed() => {
                    if *kiro_gateway_warmup_shutdown_rx.borrow() {
                        tracing::info!("Initial Kiro warmup cancelled during shutdown");
                    }
                }
                _ = async {
                    if let Err(err) =
                        crate::kiro_gateway::refresh_cached_status(&kiro_gateway_warmup).await
                    {
                        tracing::warn!("Initial Kiro cached status refresh failed: {err:#}");
                    }
                } => {}
            }
        });
        crate::kiro_gateway::spawn_status_refresher(kiro_gateway.clone(), shutdown_rx.clone());
        crate::llm_gateway::spawn_account_refresh_task(
            account_pool,
            upstream_proxy_registry.clone(),
            llm_gateway_runtime_config.clone(),
            shutdown_rx.clone(),
        );

        spawn_table_compactor(
            TableCompactorStores {
                content_store: store.clone(),
                comment_store: comment_store.clone(),
                music_store: music_store.clone(),
                music_wish_store: music_wish_store.clone(),
                article_request_store: article_request_store.clone(),
                interactive_store: interactive_store.clone(),
                llm_gateway_store: llm_gateway_store.clone(),
            },
            compaction_runtime_config.clone(),
            shutdown_rx,
        );
        let app_shutdown_rx = shutdown_tx.subscribe();
        tracing::info!("application state initialized successfully");

        Ok(Self {
            store,
            comment_store,
            geoip,
            tags_cache: Arc::new(RwLock::new(None)),
            categories_cache: Arc::new(RwLock::new(None)),
            stats_cache: Arc::new(RwLock::new(None)),
            view_analytics_config: Arc::new(RwLock::new(ViewAnalyticsRuntimeConfig::default())),
            comment_runtime_config,
            api_behavior_runtime_config,
            compaction_runtime_config,
            llm_gateway_runtime_config,
            comment_submit_guard: Arc::new(RwLock::new(HashMap::new())),
            comment_worker_tx,
            admin_access,
            music_store,
            music_play_dedupe_guard: Arc::new(RwLock::new(HashMap::new())),
            music_comment_guard: Arc::new(RwLock::new(HashMap::new())),
            music_runtime_config: Arc::new(RwLock::new(MusicRuntimeConfig::default())),
            music_wish_store,
            music_wish_worker_tx,
            music_wish_submit_guard: Arc::new(RwLock::new(HashMap::new())),
            article_request_store,
            article_request_worker_tx,
            article_request_submit_guard: Arc::new(RwLock::new(HashMap::new())),
            llm_gateway_public_submit_guard: Arc::new(RwLock::new(HashMap::new())),
            interactive_store,
            llm_gateway_store,
            upstream_proxy_registry,
            llm_gateway,
            kiro_gateway,
            email_notifier,
            behavior_event_tx,
            shutdown_tx,
            shutdown_rx: app_shutdown_rx,
            index_html_template: Arc::new(index_html_template),
        })
    }

    /// Signal all background tasks to shut down gracefully.
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }
}

/// Parse common boolean environment variable spellings with a fallback value.
fn parse_bool_env(key: &str, default_value: bool) -> bool {
    env::var(key)
        .ok()
        .map(|value| {
            matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")
        })
        .unwrap_or(default_value)
}

/// Read comment runtime settings from the environment with range validation.
fn read_comment_runtime_config_from_env() -> CommentRuntimeConfig {
    let submit_rate_limit_seconds = env::var("COMMENT_RATE_LIMIT_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0 && *value <= MAX_CONFIGURABLE_COMMENT_RATE_LIMIT_SECONDS)
        .unwrap_or(DEFAULT_COMMENT_SUBMIT_RATE_LIMIT_SECONDS);
    let list_default_limit = env::var("COMMENT_LIST_DEFAULT_LIMIT")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0 && *value <= MAX_CONFIGURABLE_COMMENT_LIST_LIMIT)
        .unwrap_or(DEFAULT_COMMENT_LIST_LIMIT);
    let cleanup_retention_days = env::var("COMMENT_CLEANUP_RETENTION_DAYS")
        .ok()
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| {
            *value == -1
                || (*value >= 1 && *value <= MAX_CONFIGURABLE_COMMENT_CLEANUP_RETENTION_DAYS)
        })
        .unwrap_or(DEFAULT_COMMENT_CLEANUP_RETENTION_DAYS);

    CommentRuntimeConfig {
        submit_rate_limit_seconds,
        list_default_limit,
        cleanup_retention_days,
    }
}

/// Read behavior analytics settings from the environment with range validation.
fn read_api_behavior_runtime_config_from_env() -> ApiBehaviorRuntimeConfig {
    let retention_days = env::var("API_BEHAVIOR_RETENTION_DAYS")
        .ok()
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| {
            *value == -1 || (*value >= 1 && *value <= MAX_CONFIGURABLE_API_BEHAVIOR_RETENTION_DAYS)
        })
        .unwrap_or(DEFAULT_API_BEHAVIOR_RETENTION_DAYS);
    let max_days = env::var("API_BEHAVIOR_MAX_DAYS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0 && *value <= MAX_CONFIGURABLE_API_BEHAVIOR_DAYS)
        .unwrap_or(DEFAULT_API_BEHAVIOR_MAX_DAYS);
    let default_days = env::var("API_BEHAVIOR_DEFAULT_DAYS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0 && *value <= MAX_CONFIGURABLE_API_BEHAVIOR_DAYS)
        .unwrap_or(DEFAULT_API_BEHAVIOR_DEFAULT_DAYS)
        .min(max_days);
    let flush_batch_size = env::var("API_BEHAVIOR_FLUSH_BATCH_SIZE")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| {
            (*value >= MIN_CONFIGURABLE_API_BEHAVIOR_FLUSH_BATCH_SIZE)
                && (*value <= MAX_CONFIGURABLE_API_BEHAVIOR_FLUSH_BATCH_SIZE)
        })
        .unwrap_or(DEFAULT_API_BEHAVIOR_FLUSH_BATCH_SIZE);
    let flush_interval_seconds = env::var("API_BEHAVIOR_FLUSH_INTERVAL_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| {
            (*value >= MIN_CONFIGURABLE_API_BEHAVIOR_FLUSH_INTERVAL_SECS)
                && (*value <= MAX_CONFIGURABLE_API_BEHAVIOR_FLUSH_INTERVAL_SECS)
        })
        .unwrap_or(DEFAULT_API_BEHAVIOR_FLUSH_INTERVAL_SECS);
    let flush_max_buffer_bytes = env::var("API_BEHAVIOR_FLUSH_MAX_BUFFER_BYTES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| {
            (*value >= MIN_CONFIGURABLE_API_BEHAVIOR_FLUSH_MAX_BUFFER_BYTES)
                && (*value <= MAX_CONFIGURABLE_API_BEHAVIOR_FLUSH_MAX_BUFFER_BYTES)
        })
        .unwrap_or(DEFAULT_API_BEHAVIOR_FLUSH_MAX_BUFFER_BYTES);

    ApiBehaviorRuntimeConfig {
        retention_days,
        default_days,
        max_days,
        flush_batch_size,
        flush_interval_seconds,
        flush_max_buffer_bytes,
    }
}

/// Read table compaction settings from the environment with range validation.
fn read_compaction_runtime_config_from_env() -> CompactionRuntimeConfig {
    let enabled = parse_bool_env("TABLE_COMPACT_ENABLED", DEFAULT_TABLE_COMPACT_ENABLED);
    let scan_interval_seconds = env::var("TABLE_COMPACT_SCAN_INTERVAL_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| {
            *value >= MIN_CONFIGURABLE_TABLE_COMPACT_SCAN_INTERVAL_SECS
                && *value <= MAX_CONFIGURABLE_TABLE_COMPACT_SCAN_INTERVAL_SECS
        })
        .unwrap_or(DEFAULT_TABLE_COMPACT_SCAN_INTERVAL_SECS);
    let fragment_threshold = env::var("TABLE_COMPACT_FRAGMENT_THRESHOLD")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| {
            *value >= MIN_CONFIGURABLE_TABLE_COMPACT_FRAGMENT_THRESHOLD
                && *value <= MAX_CONFIGURABLE_TABLE_COMPACT_FRAGMENT_THRESHOLD
        })
        .unwrap_or(DEFAULT_TABLE_COMPACT_FRAGMENT_THRESHOLD);
    let prune_older_than_hours = env::var("TABLE_COMPACT_PRUNE_OLDER_THAN_HOURS")
        .ok()
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| {
            *value >= MIN_CONFIGURABLE_TABLE_COMPACT_PRUNE_OLDER_THAN_HOURS
                && *value <= MAX_CONFIGURABLE_TABLE_COMPACT_PRUNE_OLDER_THAN_HOURS
        })
        .unwrap_or(DEFAULT_TABLE_COMPACT_PRUNE_OLDER_THAN_HOURS);

    CompactionRuntimeConfig {
        enabled,
        scan_interval_seconds,
        fragment_threshold,
        prune_older_than_hours,
    }
}

const BEHAVIOR_CHANNEL_CAPACITY: usize = 2048;

#[derive(Debug, Clone, Copy)]
struct BehaviorFlushConfig {
    batch_size: usize,
    flush_interval: Duration,
    max_buffer_bytes: usize,
}

fn behavior_flush_config(runtime_config: &ApiBehaviorRuntimeConfig) -> BehaviorFlushConfig {
    BehaviorFlushConfig {
        batch_size: runtime_config.flush_batch_size.max(1),
        flush_interval: Duration::from_secs(runtime_config.flush_interval_seconds.max(1)),
        max_buffer_bytes: runtime_config.flush_max_buffer_bytes.max(1),
    }
}

fn estimate_behavior_event_bytes(event: &NewApiBehaviorEventInput) -> usize {
    event.client_source.len()
        + event.method.len()
        + event.path.len()
        + event.query.len()
        + event.page_path.len()
        + event.referrer.as_deref().map_or(0, str::len)
        + event.client_ip.len()
        + event.ip_region.len()
        + event.ua_raw.as_deref().map_or(0, str::len)
        + event.device_type.len()
        + event.os_family.len()
        + event.browser_family.len()
        + event.request_id.len()
        + event.trace_id.len()
}

fn spawn_behavior_event_flusher(
    store: Arc<StaticFlowDataStore>,
    runtime_config: Arc<RwLock<ApiBehaviorRuntimeConfig>>,
    mut shutdown_rx: watch::Receiver<bool>,
) -> mpsc::Sender<NewApiBehaviorEventInput> {
    let (tx, mut rx) = mpsc::channel::<NewApiBehaviorEventInput>(BEHAVIOR_CHANNEL_CAPACITY);

    tokio::spawn(async move {
        let initial_config = behavior_flush_config(&runtime_config.read());
        let mut buffer = Vec::with_capacity(initial_config.batch_size);
        let mut buffered_bytes = 0usize;
        let mut flush_count: u64 = 0;

        loop {
            let flush_config = {
                let config = runtime_config.read().clone();
                behavior_flush_config(&config)
            };
            tokio::select! {
                biased;
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        if !buffer.is_empty() {
                            if let Err(err) = store
                                .append_api_behavior_events(std::mem::take(&mut buffer))
                                .await
                            {
                                tracing::warn!("final behavior event flush failed: {err:#}");
                            }
                        }
                        tracing::info!("behavior event flusher shutting down (shutdown signal)");
                        return;
                    }
                }
                maybe_event = rx.recv() => {
                    match maybe_event {
                        Some(input) => {
                            buffered_bytes =
                                buffered_bytes.saturating_add(estimate_behavior_event_bytes(&input));
                            buffer.push(input);
                            while buffer.len() < flush_config.batch_size
                                && buffered_bytes < flush_config.max_buffer_bytes
                            {
                                match rx.try_recv() {
                                    Ok(input) => {
                                        buffered_bytes = buffered_bytes
                                            .saturating_add(estimate_behavior_event_bytes(&input));
                                        buffer.push(input);
                                    },
                                    Err(_) => break,
                                }
                            }
                            if buffer.len() >= flush_config.batch_size
                                || buffered_bytes >= flush_config.max_buffer_bytes
                            {
                                let batch = std::mem::take(&mut buffer);
                                buffered_bytes = 0;
                                let count = batch.len();

                                if let Err(err) = store.append_api_behavior_events(batch).await {
                                    tracing::warn!("behavior event batch flush failed ({count} events): {err:#}");
                                    continue;
                                }

                                flush_count += 1;
                                tracing::debug!("flushed {count} behavior events (flush #{flush_count})");
                            }
                        },
                        None => {
                            if !buffer.is_empty() {
                                if let Err(err) = store
                                    .append_api_behavior_events(std::mem::take(&mut buffer))
                                    .await
                                {
                                    tracing::warn!("final behavior event flush failed: {err:#}");
                                }
                            }
                            tracing::info!("behavior event flusher shutting down");
                            return;
                        }
                    }
                }
                _ = tokio::time::sleep(flush_config.flush_interval) => {
                    if !buffer.is_empty() {
                        let batch = std::mem::take(&mut buffer);
                        buffered_bytes = 0;
                        let count = batch.len();

                        if let Err(err) = store.append_api_behavior_events(batch).await {
                            tracing::warn!("behavior event timed flush failed ({count} events): {err:#}");
                            continue;
                        }

                        flush_count += 1;
                        tracing::debug!("flushed {count} behavior events (flush #{flush_count})");
                    }
                }
            }
        }
    });

    tx
}

fn spawn_table_compactor(
    stores: TableCompactorStores,
    compaction_runtime_config: Arc<RwLock<CompactionRuntimeConfig>>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    tokio::spawn(async move {
        // Startup delay to avoid racing with schema migrations
        tokio::select! {
            biased;
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    tracing::info!("table compactor cancelled during startup delay");
                    return;
                }
            }
            _ = tokio::time::sleep(Duration::from_secs(60)) => {}
        }

        let startup_config = compaction_runtime_config.read().clone();
        tracing::info!(
            "table compactor started (enabled={}, scan_interval={}s, threshold={}, \
             prune_older_than_hours={})",
            startup_config.enabled,
            startup_config.scan_interval_seconds,
            startup_config.fragment_threshold,
            startup_config.prune_older_than_hours
        );

        loop {
            let started = Instant::now();
            let runtime = compaction_runtime_config.read().clone();
            let config = CompactConfig {
                enabled: runtime.enabled,
                fragment_threshold: runtime.fragment_threshold,
                prune_older_than_hours: runtime.prune_older_than_hours,
                skip_tables: HashSet::new(),
            };

            let mut total_tables = 0usize;
            let mut total_compacted = 0usize;
            let mut total_failed = 0usize;

            // Scan each DB group sequentially
            for r in stores
                .content_store
                .scan_and_compact_managed_content_tables(&config)
                .await
            {
                total_tables += 1;
                if r.compacted {
                    total_compacted += 1;
                }

                if let Some(err) = r.error {
                    total_failed += 1;
                    tracing::warn!(
                        "compactor content/{} action={} compacted={} pruned={} small_fragments={} \
                         elapsed_ms={} error={}",
                        r.table,
                        r.action.as_str(),
                        r.compacted,
                        r.pruned,
                        r.small_fragments,
                        r.elapsed_ms,
                        err
                    );
                } else {
                    tracing::info!(
                        "compactor content/{} action={} compacted={} pruned={} small_fragments={} \
                         elapsed_ms={}",
                        r.table,
                        r.action.as_str(),
                        r.compacted,
                        r.pruned,
                        r.small_fragments,
                        r.elapsed_ms
                    );
                }
            }

            for (db_label, conn, tables) in [
                (
                    "content",
                    stores.article_request_store.connection(),
                    article_request_store::ARTICLE_REQUEST_TABLE_NAMES,
                ),
                (
                    "content",
                    stores.interactive_store.connection(),
                    interactive_store::INTERACTIVE_TABLE_NAMES,
                ),
                (
                    "content",
                    stores.llm_gateway_store.connection(),
                    llm_gateway_store::LLM_GATEWAY_TABLE_NAMES,
                ),
                (
                    "comments",
                    stores.comment_store.connection(),
                    comments_store::COMMENT_TABLE_NAMES,
                ),
                ("music", stores.music_store.connection(), music_store::MUSIC_TABLE_NAMES),
                (
                    "music",
                    stores.music_wish_store.connection(),
                    music_wish_store::MUSIC_WISH_TABLE_NAMES,
                ),
            ] {
                let results = scan_and_compact_tables(conn, tables, &config).await;
                for r in results {
                    total_tables += 1;
                    if r.compacted {
                        total_compacted += 1;
                    }

                    if let Some(err) = r.error {
                        total_failed += 1;
                        tracing::warn!(
                            "compactor {db_label}/{} action={} compacted={} pruned={} \
                             small_fragments={} elapsed_ms={} error={}",
                            r.table,
                            r.action.as_str(),
                            r.compacted,
                            r.pruned,
                            r.small_fragments,
                            r.elapsed_ms,
                            err
                        );
                    } else {
                        tracing::info!(
                            "compactor {db_label}/{} action={} compacted={} pruned={} \
                             small_fragments={} elapsed_ms={}",
                            r.table,
                            r.action.as_str(),
                            r.compacted,
                            r.pruned,
                            r.small_fragments,
                            r.elapsed_ms
                        );
                    }
                }
            }

            tracing::info!(
                "compactor cycle done: tables={} compacted={} failed={} elapsed_ms={} enabled={} \
                 scan_interval={}s threshold={} prune_older_than_hours={}",
                total_tables,
                total_compacted,
                total_failed,
                started.elapsed().as_millis(),
                runtime.enabled,
                runtime.scan_interval_seconds,
                runtime.fragment_threshold,
                runtime.prune_older_than_hours
            );

            if total_compacted > 0 {
                // SAFETY: forcing a mimalloc collection is an FFI call with no
                // borrowed Rust references involved, and it only touches the
                // allocator's own global state.
                unsafe {
                    better_mimalloc_sys::mi_collect(true);
                }
                tracing::info!(
                    "compactor forced mimalloc collection after {} compacted table(s)",
                    total_compacted
                );
            }

            // Wait for next cycle or shutdown
            tokio::select! {
                biased;
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        tracing::info!("table compactor shutting down");
                        return;
                    }
                }
                _ = tokio::time::sleep(Duration::from_secs(runtime.scan_interval_seconds)) => {}
            }
        }
    });
}
