use std::{
    collections::HashMap,
    env,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Result;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use static_flow_shared::{
    article_request_store::ArticleRequestStore,
    comments_store::CommentDataStore,
    interactive_store::InteractivePageStore,
    lancedb_api::{
        CategoryInfo, NewApiBehaviorEventInput, StaticFlowDataStore, StatsResponse, TagInfo,
    },
    llm_gateway_store::LlmGatewayStore as Gpt2ApiContributionStore,
    music_store::MusicDataStore,
    music_wish_store::MusicWishStore,
};
use tokio::sync::{mpsc, watch};

#[cfg(feature = "local-media")]
use crate::media_proxy::MediaProxyState;
use crate::{
    article_request_worker::{self, ArticleRequestWorkerConfig},
    comment_worker::{self, CommentAiWorkerConfig},
    email::EmailNotifier,
    geoip::GeoIpResolver,
    gpt2api_rs::Gpt2ApiRsState,
    llm_access_admin_proxy::LlmAccessAdminProxyState,
    music_wish_worker::{self, MusicWishWorkerConfig},
    public_submit_guard::PublicSubmitGuard,
    table_maintenance,
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
pub const DEFAULT_TABLE_COMPACT_PRUNE_OLDER_THAN_HOURS: i64 = 0;
pub const MIN_CONFIGURABLE_TABLE_COMPACT_PRUNE_OLDER_THAN_HOURS: i64 = 0;
pub const MAX_CONFIGURABLE_TABLE_COMPACT_PRUNE_OLDER_THAN_HOURS: i64 = 8_760;
pub const DEFAULT_TABLE_COMPACT_WORKER_COUNT: usize = 4;
pub const MIN_CONFIGURABLE_TABLE_COMPACT_WORKER_COUNT: usize = 1;
pub const MAX_CONFIGURABLE_TABLE_COMPACT_WORKER_COUNT: usize = 32;

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
    pub worker_count: usize,
}

impl Default for CompactionRuntimeConfig {
    fn default() -> Self {
        Self {
            enabled: DEFAULT_TABLE_COMPACT_ENABLED,
            scan_interval_seconds: DEFAULT_TABLE_COMPACT_SCAN_INTERVAL_SECS,
            fragment_threshold: DEFAULT_TABLE_COMPACT_FRAGMENT_THRESHOLD,
            prune_older_than_hours: DEFAULT_TABLE_COMPACT_PRUNE_OLDER_THAN_HOURS,
            worker_count: DEFAULT_TABLE_COMPACT_WORKER_COUNT,
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
pub(crate) struct TableCompactorStores {
    pub(crate) content_store: Arc<StaticFlowDataStore>,
    pub(crate) comment_store: Arc<CommentDataStore>,
    pub(crate) music_store: Arc<MusicDataStore>,
    pub(crate) music_wish_store: Arc<MusicWishStore>,
    pub(crate) article_request_store: Arc<ArticleRequestStore>,
    pub(crate) interactive_store: Arc<InteractivePageStore>,
    pub(crate) gpt2api_contribution_store: Arc<Gpt2ApiContributionStore>,
}

/// Immutable runtime metadata exposed by health and diagnostics endpoints.
#[derive(Debug, Clone)]
pub struct RuntimeMetadata {
    /// Unix timestamp in milliseconds for process startup.
    pub started_at_ms: i64,
    /// Build identifier exposed by `/api/healthz`.
    pub build_id: String,
}

#[derive(Clone)]
pub struct AppState {
    pub(crate) store: Arc<StaticFlowDataStore>,
    pub(crate) comment_store: Arc<CommentDataStore>,
    pub(crate) geoip: GeoIpResolver,
    pub(crate) gpt2api_rs: Arc<Gpt2ApiRsState>,
    pub(crate) tags_cache: SharedListCache<TagInfo>,
    pub(crate) categories_cache: SharedListCache<CategoryInfo>,
    pub(crate) stats_cache: SharedValueCache<StatsResponse>,
    pub(crate) view_analytics_config: Arc<RwLock<ViewAnalyticsRuntimeConfig>>,
    pub(crate) comment_runtime_config: Arc<RwLock<CommentRuntimeConfig>>,
    pub(crate) api_behavior_runtime_config: Arc<RwLock<ApiBehaviorRuntimeConfig>>,
    pub(crate) compaction_runtime_config: Arc<RwLock<CompactionRuntimeConfig>>,
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
    pub(crate) gpt2api_public_submit_guard: Arc<PublicSubmitGuard>,
    pub(crate) interactive_store: Arc<InteractivePageStore>,
    pub(crate) gpt2api_contribution_store: Arc<Gpt2ApiContributionStore>,
    pub(crate) email_notifier: Option<Arc<EmailNotifier>>,
    pub(crate) behavior_event_tx: mpsc::Sender<NewApiBehaviorEventInput>,
    pub(crate) shutdown_tx: watch::Sender<bool>,
    pub(crate) shutdown_rx: watch::Receiver<bool>,
    pub(crate) frontend_dist_dir: Arc<PathBuf>,
    pub(crate) runtime_metadata: Arc<RuntimeMetadata>,
    pub(crate) llm_access_admin_proxy: Arc<LlmAccessAdminProxyState>,
    #[cfg(feature = "local-media")]
    pub(crate) media_proxy: Option<Arc<MediaProxyState>>,
}

impl AppState {
    pub async fn new(
        content_db_uri: &str,
        comments_db_uri: &str,
        music_db_uri: &str,
        frontend_dist_dir: impl Into<PathBuf>,
    ) -> Result<Self> {
        tracing::info!(
            content_db_uri,
            comments_db_uri,
            music_db_uri,
            "initializing application state"
        );
        let frontend_dist_dir = frontend_dist_dir.into();
        let store = Arc::new(StaticFlowDataStore::connect(content_db_uri).await?);
        let comment_store = Arc::new(CommentDataStore::connect(comments_db_uri).await?);
        let music_store = Arc::new(MusicDataStore::connect(music_db_uri).await?);
        let music_wish_store = Arc::new(MusicWishStore::connect(music_db_uri).await?);
        let article_request_store = Arc::new(ArticleRequestStore::connect(content_db_uri).await?);
        let interactive_store = Arc::new(InteractivePageStore::connect(content_db_uri).await?);
        let gpt2api_contribution_store =
            Arc::new(Gpt2ApiContributionStore::connect(content_db_uri).await?);
        let geoip = GeoIpResolver::from_env()?;
        geoip.warmup().await;
        let gpt2api_rs = Arc::new(Gpt2ApiRsState::load_from_env().await?);
        let llm_access_admin_proxy = LlmAccessAdminProxyState::from_env()?;
        let email_notifier = EmailNotifier::from_env()?.map(Arc::new);
        let runtime_metadata = Arc::new(RuntimeMetadata {
            started_at_ms: chrono::Utc::now().timestamp_millis(),
            build_id: option_env!("STATICFLOW_BUILD_ID")
                .unwrap_or(env!("CARGO_PKG_VERSION"))
                .to_string(),
        });

        let comment_runtime_config = Arc::new(RwLock::new(read_comment_runtime_config_from_env()));
        let api_behavior_runtime_config =
            Arc::new(RwLock::new(read_api_behavior_runtime_config_from_env()));
        let compaction_runtime_config =
            Arc::new(RwLock::new(read_compaction_runtime_config_from_env()));
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
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
        #[cfg(feature = "local-media")]
        let media_proxy = MediaProxyState::from_env()?;

        table_maintenance::spawn_table_maintenance_loop(
            TableCompactorStores {
                content_store: store.clone(),
                comment_store: comment_store.clone(),
                music_store: music_store.clone(),
                music_wish_store: music_wish_store.clone(),
                article_request_store: article_request_store.clone(),
                interactive_store: interactive_store.clone(),
                gpt2api_contribution_store: gpt2api_contribution_store.clone(),
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
            gpt2api_rs,
            tags_cache: Arc::new(RwLock::new(None)),
            categories_cache: Arc::new(RwLock::new(None)),
            stats_cache: Arc::new(RwLock::new(None)),
            view_analytics_config: Arc::new(RwLock::new(ViewAnalyticsRuntimeConfig::default())),
            comment_runtime_config,
            api_behavior_runtime_config,
            compaction_runtime_config,
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
            gpt2api_public_submit_guard: Arc::new(RwLock::new(HashMap::new())),
            interactive_store,
            gpt2api_contribution_store,
            email_notifier,
            behavior_event_tx,
            shutdown_tx,
            shutdown_rx: app_shutdown_rx,
            frontend_dist_dir: Arc::new(frontend_dist_dir),
            runtime_metadata,
            llm_access_admin_proxy,
            #[cfg(feature = "local-media")]
            media_proxy,
        })
    }

    pub(crate) async fn load_index_html_template(&self) -> String {
        load_frontend_index_html(self.frontend_dist_dir.as_ref()).await
    }

    /// Signal all background tasks to shut down gracefully.
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }
}

pub(crate) async fn load_frontend_index_html(frontend_dist_dir: &Path) -> String {
    let index_html_path = frontend_dist_dir.join("index.html");
    match tokio::fs::read_to_string(&index_html_path).await {
        Ok(html) => html,
        Err(err) => {
            tracing::warn!(
                path = %index_html_path.display(),
                "failed to load frontend index.html: {err}"
            );
            String::new()
        },
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
    let worker_count = env::var("TABLE_COMPACT_WORKER_COUNT")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| {
            *value >= MIN_CONFIGURABLE_TABLE_COMPACT_WORKER_COUNT
                && *value <= MAX_CONFIGURABLE_TABLE_COMPACT_WORKER_COUNT
        })
        .unwrap_or(DEFAULT_TABLE_COMPACT_WORKER_COUNT);

    CompactionRuntimeConfig {
        enabled,
        scan_interval_seconds,
        fragment_threshold,
        prune_older_than_hours,
        worker_count,
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn compaction_runtime_config_defaults_prune_to_zero_hours() {
        let config = CompactionRuntimeConfig::default();
        assert_eq!(config.prune_older_than_hours, 0);
    }

    #[tokio::test]
    async fn load_frontend_index_html_reads_latest_file_contents() {
        let dir = tempfile::tempdir().expect("tempdir");
        let index_html_path = dir.path().join("index.html");
        tokio::fs::write(&index_html_path, "old html")
            .await
            .expect("write old html");

        let first = load_frontend_index_html(dir.path()).await;

        tokio::fs::write(&index_html_path, "new html")
            .await
            .expect("write new html");
        let second = load_frontend_index_html(dir.path()).await;

        assert_eq!(first, "old html");
        assert_eq!(second, "new html");
    }

    #[tokio::test]
    async fn load_frontend_index_html_returns_empty_when_file_is_missing() {
        let dir = tempfile::tempdir().expect("tempdir");

        let html = load_frontend_index_html(Path::new(dir.path())).await;

        assert!(html.is_empty());
    }
}
