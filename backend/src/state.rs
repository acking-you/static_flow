use std::{sync::Arc, time::Instant};

use anyhow::Result;
use static_flow_shared::lancedb_api::{CategoryInfo, StaticFlowDataStore, TagInfo};
use tokio::sync::RwLock;

type CacheEntry<T> = Option<(Vec<T>, Instant)>;
type SharedCache<T> = Arc<RwLock<CacheEntry<T>>>;

#[derive(Clone)]
pub struct AppState {
    pub(crate) store: Arc<StaticFlowDataStore>,
    pub(crate) tags_cache: SharedCache<TagInfo>,
    pub(crate) categories_cache: SharedCache<CategoryInfo>,
}

impl AppState {
    pub async fn new(db_uri: &str) -> Result<Self> {
        let store = StaticFlowDataStore::connect(db_uri).await?;
        Ok(Self {
            store: Arc::new(store),
            tags_cache: Arc::new(RwLock::new(None)),
            categories_cache: Arc::new(RwLock::new(None)),
        })
    }
}
