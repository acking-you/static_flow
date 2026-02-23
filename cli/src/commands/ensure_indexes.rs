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

    // Music DB: songs FTS index on searchable_text
    let music_db_path = db_path
        .parent()
        .unwrap_or(db_path)
        .join("lancedb-music");
    if music_db_path.exists() {
        tracing::info!("Found music DB at {}, ensuring songs FTS index...", music_db_path.display());
        let music_db = connect_db(&music_db_path).await?;
        if let Ok(songs_table) = music_db.open_table("songs").execute().await {
            if let Err(err) = ensure_fts_index(&songs_table, "searchable_text").await {
                tracing::warn!("Failed to create FTS index on songs.searchable_text: {err}");
            }
            if let Err(err) = ensure_vector_index(&songs_table, "vector_en").await {
                tracing::warn!("Failed to create vector index on songs.vector_en: {err}");
            }
            if let Err(err) = ensure_vector_index(&songs_table, "vector_zh").await {
                tracing::warn!("Failed to create vector index on songs.vector_zh: {err}");
            }
        } else {
            tracing::info!("songs table not found in music DB, skipping");
        }
    } else {
        tracing::info!("Music DB not found at {}, skipping", music_db_path.display());
    }

    tracing::info!("Index ensure run finished for {}", db_path.display());
    Ok(())
}
