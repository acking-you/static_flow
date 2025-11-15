use crate::markdown;
use anyhow::Result;
use static_flow_shared::ArticleListItem;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Clone)]
pub struct AppState {
    /// Cached article list
    articles: Arc<RwLock<Vec<ArticleListItem>>>,
    /// Content directory path
    content_dir: String,
    /// Images directory path
    images_dir: String,
    /// Article ID to file path mapping
    id_to_path: Arc<RwLock<HashMap<String, String>>>,
}

impl AppState {
    pub async fn new(content_dir: &str, images_dir: &str) -> Result<Self> {
        let (articles, id_to_path) = markdown::scan_articles(content_dir).await?;

        Ok(Self {
            articles: Arc::new(RwLock::new(articles)),
            content_dir: content_dir.to_string(),
            images_dir: images_dir.to_string(),
            id_to_path: Arc::new(RwLock::new(id_to_path)),
        })
    }

    pub async fn get_articles(&self) -> Vec<ArticleListItem> {
        self.articles.read().await.clone()
    }

    pub async fn get_article_path(&self, id: &str) -> Option<String> {
        self.id_to_path.read().await.get(id).cloned()
    }

    pub fn article_count(&self) -> usize {
        // Using blocking read for initialization logging
        futures::executor::block_on(async { self.articles.read().await.len() })
    }

    pub fn content_dir(&self) -> &str {
        &self.content_dir
    }

    pub fn images_dir(&self) -> &str {
        &self.images_dir
    }
}
