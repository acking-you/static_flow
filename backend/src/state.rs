use std::{sync::Arc, time::Instant};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use static_flow_shared::lancedb_api::{CategoryInfo, StaticFlowDataStore, StatsResponse, TagInfo};
use tokio::sync::RwLock;

type ListCacheEntry<T> = Option<(Vec<T>, Instant)>;
type SharedListCache<T> = Arc<RwLock<ListCacheEntry<T>>>;
type ValueCacheEntry<T> = Option<(T, Instant)>;
type SharedValueCache<T> = Arc<RwLock<ValueCacheEntry<T>>>;

pub const DEFAULT_VIEW_DEDUPE_WINDOW_SECONDS: u64 = 60;
pub const DEFAULT_VIEW_TREND_DAYS: usize = 30;
pub const DEFAULT_VIEW_TREND_MAX_DAYS: usize = 180;
pub const MAX_CONFIGURABLE_VIEW_DEDUPE_WINDOW_SECONDS: u64 = 3600;
pub const MAX_CONFIGURABLE_VIEW_TREND_DAYS: usize = 365;

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

#[derive(Clone)]
pub struct AppState {
    pub(crate) store: Arc<StaticFlowDataStore>,
    pub(crate) tags_cache: SharedListCache<TagInfo>,
    pub(crate) categories_cache: SharedListCache<CategoryInfo>,
    pub(crate) stats_cache: SharedValueCache<StatsResponse>,
    pub(crate) view_analytics_config: Arc<RwLock<ViewAnalyticsRuntimeConfig>>,
}

impl AppState {
    pub async fn new(db_uri: &str) -> Result<Self> {
        let store = StaticFlowDataStore::connect(db_uri).await?;
        Ok(Self {
            store: Arc::new(store),
            tags_cache: Arc::new(RwLock::new(None)),
            categories_cache: Arc::new(RwLock::new(None)),
            stats_cache: Arc::new(RwLock::new(None)),
            view_analytics_config: Arc::new(RwLock::new(ViewAnalyticsRuntimeConfig::default())),
        })
    }
}
