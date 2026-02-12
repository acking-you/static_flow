use std::{sync::Arc, time::Instant};

use anyhow::Result;
use static_flow_shared::lancedb_api::{CategoryInfo, StaticFlowDataStore, StatsResponse, TagInfo};
use tokio::sync::RwLock;

type ListCacheEntry<T> = Option<(Vec<T>, Instant)>;
type SharedListCache<T> = Arc<RwLock<ListCacheEntry<T>>>;
type ValueCacheEntry<T> = Option<(T, Instant)>;
type SharedValueCache<T> = Arc<RwLock<ValueCacheEntry<T>>>;

#[derive(Clone)]
pub struct AppState {
    pub(crate) store: Arc<StaticFlowDataStore>,
    pub(crate) tags_cache: SharedListCache<TagInfo>,
    pub(crate) categories_cache: SharedListCache<CategoryInfo>,
    pub(crate) stats_cache: SharedValueCache<StatsResponse>,
}

impl AppState {
    pub async fn new(db_uri: &str) -> Result<Self> {
        let store = StaticFlowDataStore::connect(db_uri).await?;
        Ok(Self {
            store: Arc::new(store),
            tags_cache: Arc::new(RwLock::new(None)),
            categories_cache: Arc::new(RwLock::new(None)),
            stats_cache: Arc::new(RwLock::new(None)),
        })
    }
}
