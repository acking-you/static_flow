use std::path::Path;

use anyhow::{Context, Result};

use crate::db::{connect_db, ensure_fts_index, ensure_vector_index};

pub async fn run(db_path: &Path) -> Result<()> {
    let db = connect_db(db_path).await?;

    let articles_table = db
        .open_table("articles")
        .execute()
        .await
        .context("articles table not found; run `sf-cli init` first")?;
    let images_table = db
        .open_table("images")
        .execute()
        .await
        .context("images table not found; run `sf-cli init` first")?;

    if let Err(err) = ensure_fts_index(&articles_table, "content").await {
        tracing::warn!("Failed to create FTS index on articles: {err}");
    }

    if let Err(err) = ensure_vector_index(&articles_table, "vector_en").await {
        tracing::warn!("Failed to create vector index on articles (vector_en): {err}");
    }
    if let Err(err) = ensure_vector_index(&articles_table, "vector_zh").await {
        tracing::warn!("Failed to create vector index on articles (vector_zh): {err}");
    }
    if let Err(err) = ensure_vector_index(&images_table, "vector").await {
        tracing::warn!("Failed to create vector index on images: {err}");
    }

    tracing::info!("Index ensure run finished for {}", db_path.display());
    Ok(())
}
