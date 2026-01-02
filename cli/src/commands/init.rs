use std::path::Path;

use anyhow::Result;
use lancedb::{
    index::scalar::FullTextSearchQuery,
    query::{ExecutableQuery, QueryBase},
};

use crate::{
    db::{connect_db, ensure_fts_index, ensure_table, ensure_vector_index},
    schema::{article_schema, image_schema},
};

pub async fn run(db_path: &Path) -> Result<()> {
    let db = connect_db(db_path).await?;

    let articles_table = ensure_table(&db, "articles", article_schema()).await?;
    let images_table = ensure_table(&db, "images", image_schema()).await?;

    if let Err(err) = ensure_fts_index(&articles_table, "content").await {
        tracing::warn!("Failed to create FTS index on articles: {err}");
    }

    // Vector indices are created lazily to avoid empty-table training errors.
    if let Err(err) = ensure_vector_index(&articles_table, "vector_en").await {
        tracing::warn!("Failed to create vector index on articles (vector_en): {err}");
    }
    if let Err(err) = ensure_vector_index(&articles_table, "vector_zh").await {
        tracing::warn!("Failed to create vector index on articles (vector_zh): {err}");
    }
    if let Err(err) = ensure_vector_index(&images_table, "vector").await {
        tracing::warn!("Failed to create vector index on images: {err}");
    }

    let _ = articles_table
        .query()
        .full_text_search(FullTextSearchQuery::new("init".to_string()))
        .limit(1)
        .execute()
        .await;

    tracing::info!("LanceDB initialized at {}", db_path.display());
    Ok(())
}
