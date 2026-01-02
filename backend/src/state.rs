use std::{sync::Arc, time::Instant};

use anyhow::Result;
use lancedb::{connect, Connection, Table};
use tokio::sync::RwLock;

use crate::models::{CategoryInfo, TagInfo};

#[derive(Clone)]
pub struct AppState {
    /// LanceDB connection shared across handlers.
    db: Arc<Connection>,
    articles_table: String,
    images_table: String,
    pub(crate) tags_cache: Arc<RwLock<Option<(Vec<TagInfo>, Instant)>>>,
    pub(crate) categories_cache: Arc<RwLock<Option<(Vec<CategoryInfo>, Instant)>>>,
}

impl AppState {
    pub async fn new(db_uri: &str) -> Result<Self> {
        let db = connect(db_uri).execute().await?;
        Ok(Self {
            db: Arc::new(db),
            articles_table: "articles".to_string(),
            images_table: "images".to_string(),
            tags_cache: Arc::new(RwLock::new(None)),
            categories_cache: Arc::new(RwLock::new(None)),
        })
    }

    pub async fn articles_table(&self) -> Result<Table> {
        Ok(self.db.open_table(&self.articles_table).execute().await?)
    }

    pub async fn images_table(&self) -> Result<Table> {
        Ok(self.db.open_table(&self.images_table).execute().await?)
    }
}
