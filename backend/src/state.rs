use std::{
    collections::{HashMap, HashSet},
    env,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use static_flow_shared::{
    article_request_store::{self, ArticleRequestStore},
    comments_store::{self, CommentDataStore},
    interactive_store::{self, InteractivePageStore},
    lancedb_api::{
        CategoryInfo, NewApiBehaviorEventInput, StaticFlowDataStore, StatsResponse, TagInfo,
    },
    llm_gateway_store::{
        self, LlmGatewayStore, DEFAULT_KIRO_CHANNEL_MAX_CONCURRENCY,
        DEFAULT_KIRO_CHANNEL_MIN_START_INTERVAL_MS, DEFAULT_LLM_GATEWAY_AUTH_CACHE_TTL_SECONDS,
        DEFAULT_LLM_GATEWAY_MAX_REQUEST_BODY_BYTES,
    },
    music_store::{self, MusicDataStore},
    music_wish_store::{self, MusicWishStore},
    optimize::{scan_and_compact_tables, CompactConfig},
};
use tokio::sync::{mpsc, watch, RwLock};

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
pub const DEFAULT_TABLE_COMPACT_ENABLED: bool = true;
pub const DEFAULT_TABLE_COMPACT_SCAN_INTERVAL_SECS: u64 = 180;
pub const MIN_CONFIGURABLE_TABLE_COMPACT_SCAN_INTERVAL_SECS: u64 = 30;
pub const MAX_CONFIGURABLE_TABLE_COMPACT_SCAN_INTERVAL_SECS: u64 = 86_400;
pub const DEFAULT_TABLE_COMPACT_FRAGMENT_THRESHOLD: usize = 10;
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
}

impl Default for ApiBehaviorRuntimeConfig {
    fn default() -> Self {
        Self {
            retention_days: DEFAULT_API_BEHAVIOR_RETENTION_DAYS,
            default_days: DEFAULT_API_BEHAVIOR_DEFAULT_DAYS,
            max_days: DEFAULT_API_BEHAVIOR_MAX_DAYS,
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
    /// Maximum number of concurrent Kiro upstream requests.
    pub kiro_channel_max_concurrency: u64,
    /// Minimum milliseconds between consecutive Kiro upstream request starts.
    pub kiro_channel_min_start_interval_ms: u64,
}

impl Default for LlmGatewayRuntimeConfig {
    fn default() -> Self {
        Self {
            auth_cache_ttl_seconds: DEFAULT_LLM_GATEWAY_AUTH_CACHE_TTL_SECONDS,
            max_request_body_bytes: DEFAULT_LLM_GATEWAY_MAX_REQUEST_BODY_BYTES,
            kiro_channel_max_concurrency: DEFAULT_KIRO_CHANNEL_MAX_CONCURRENCY,
            kiro_channel_min_start_interval_ms: DEFAULT_KIRO_CHANNEL_MIN_START_INTERVAL_MS,
        }
    }
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
        let kiro_channel_max_concurrency =
            llm_gateway_runtime_config_record.kiro_channel_max_concurrency;
        let kiro_channel_min_start_interval_ms =
            llm_gateway_runtime_config_record.kiro_channel_min_start_interval_ms;
        tracing::info!(
            auth_cache_ttl_seconds = llm_gateway_auth_cache_ttl_seconds,
            max_request_body_bytes = llm_gateway_max_request_body_bytes,
            kiro_channel_max_concurrency,
            kiro_channel_min_start_interval_ms,
            "loaded llm gateway runtime config from storage"
        );
        let llm_gateway_runtime_config = Arc::new(RwLock::new(LlmGatewayRuntimeConfig {
            auth_cache_ttl_seconds: llm_gateway_auth_cache_ttl_seconds,
            max_request_body_bytes: llm_gateway_max_request_body_bytes,
            kiro_channel_max_concurrency,
            kiro_channel_min_start_interval_ms,
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
            kiro_channel_max_concurrency,
            kiro_channel_min_start_interval_ms,
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

        let behavior_event_tx = spawn_behavior_event_flusher(store.clone(), shutdown_rx.clone());
        if let Err(err) = crate::llm_gateway::token_refresh::refresh_all_accounts_once(
            llm_gateway.account_pool.as_ref(),
            upstream_proxy_registry.as_ref(),
        )
        .await
        {
            tracing::warn!("Initial Codex account usage refresh failed: {err:#}");
        }
        if let Err(err) = crate::llm_gateway::refresh_public_rate_limit_status(&llm_gateway).await {
            tracing::warn!("Initial LLM gateway rate-limit refresh failed: {err:#}");
        }
        crate::llm_gateway::spawn_public_rate_limit_refresher(
            llm_gateway.clone(),
            shutdown_rx.clone(),
        );
        if let Err(err) = crate::kiro_gateway::refresh_cached_status(&kiro_gateway).await {
            tracing::warn!("Initial Kiro cached status refresh failed: {err:#}");
        }
        crate::kiro_gateway::spawn_status_refresher(kiro_gateway.clone(), shutdown_rx.clone());
        crate::llm_gateway::spawn_account_refresh_task(
            account_pool,
            upstream_proxy_registry.clone(),
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

    ApiBehaviorRuntimeConfig {
        retention_days,
        default_days,
        max_days,
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

/// Buffered writer for api_behavior_events.
///
/// Events are collected via an mpsc channel and flushed as a single batch
/// every `FLUSH_INTERVAL` or when the buffer reaches `FLUSH_BATCH_SIZE`,
/// whichever comes first.
const BEHAVIOR_FLUSH_BATCH_SIZE: usize = 50;
const BEHAVIOR_CHANNEL_CAPACITY: usize = 2048;

fn spawn_behavior_event_flusher(
    store: Arc<StaticFlowDataStore>,
    mut shutdown_rx: watch::Receiver<bool>,
) -> mpsc::Sender<NewApiBehaviorEventInput> {
    let (tx, mut rx) = mpsc::channel::<NewApiBehaviorEventInput>(BEHAVIOR_CHANNEL_CAPACITY);

    tokio::spawn(async move {
        let flush_interval = tokio::time::Duration::from_secs(5);
        let mut buffer = Vec::with_capacity(BEHAVIOR_FLUSH_BATCH_SIZE);
        let mut flush_count: u64 = 0;

        loop {
            let event = tokio::select! {
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
                    continue;
                }
                result = tokio::time::timeout(flush_interval, rx.recv()) => result,
            };

            match event {
                Ok(Some(input)) => {
                    buffer.push(input);
                    while buffer.len() < BEHAVIOR_FLUSH_BATCH_SIZE {
                        match rx.try_recv() {
                            Ok(input) => buffer.push(input),
                            Err(_) => break,
                        }
                    }
                },
                Ok(None) => {
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
                },
                Err(_) => {},
            }

            if buffer.is_empty() {
                continue;
            }

            let batch = std::mem::take(&mut buffer);
            let count = batch.len();

            if let Err(err) = store.append_api_behavior_events(batch).await {
                tracing::warn!("behavior event batch flush failed ({count} events): {err:#}");
                continue;
            }

            flush_count += 1;
            tracing::debug!("flushed {count} behavior events (flush #{flush_count})");
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

        let startup_config = compaction_runtime_config.read().await.clone();
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
            let runtime = compaction_runtime_config.read().await.clone();
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
