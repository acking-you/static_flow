use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use arrow_array::{RecordBatch, RecordBatchIterator};
use arrow_schema::Schema;
use lancedb::index::Index;
use lancedb::{connect, Connection, Table};

use crate::schema::{build_article_batch, build_image_batch, ArticleRecord, ImageRecord};

pub async fn connect_db(db_path: &Path) -> Result<Connection> {
    connect(db_path.to_string_lossy().as_ref())
        .execute()
        .await
        .context("failed to connect to LanceDB")
}

pub async fn ensure_table(db: &Connection, name: &str, schema: Arc<Schema>) -> Result<Table> {
    match db.open_table(name).execute().await {
        Ok(table) => Ok(table),
        Err(_) => {
            let batch = RecordBatch::new_empty(schema.clone());
            let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
            db.create_table(name, Box::new(batches))
                .execute()
                .await?;
            Ok(db.open_table(name).execute().await?)
        },
    }
}

pub async fn ensure_fts_index(table: &Table, column: &str) -> Result<()> {
    let indices = table.list_indices().await?;
    if indices.iter().any(|index| index.columns == [column]) {
        return Ok(());
    }

    table
        .create_index(&[column], Index::FTS(Default::default()))
        .execute()
        .await?;
    Ok(())
}

pub async fn ensure_vector_index(table: &Table, column: &str) -> Result<()> {
    let filter = format!("{column} IS NOT NULL");
    let row_count = table.count_rows(Some(filter)).await?;
    if row_count == 0 {
        // Lance needs existing rows to train a vector index.
        return Ok(());
    }

    let indices = table.list_indices().await?;
    if indices.iter().any(|index| index.columns == [column]) {
        return Ok(());
    }

    table.create_index(&[column], Index::Auto).execute().await?;
    Ok(())
}

pub async fn upsert_articles(table: &Table, records: &[ArticleRecord]) -> Result<()> {
    if records.is_empty() {
        return Ok(());
    }
    let batch = build_article_batch(records)?;
    let schema = batch.schema();
    let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);

    let mut merge = table.merge_insert(&["id"]);
    merge.when_matched_update_all(None);
    merge.when_not_matched_insert_all();
    merge.execute(Box::new(batches)).await?;
    Ok(())
}

pub async fn upsert_images(table: &Table, records: &[ImageRecord]) -> Result<()> {
    if records.is_empty() {
        return Ok(());
    }
    let batch = build_image_batch(records)?;
    let schema = batch.schema();
    let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);

    let mut merge = table.merge_insert(&["id"]);
    merge.when_matched_update_all(None);
    merge.when_not_matched_insert_all();
    merge.execute(Box::new(batches)).await?;
    Ok(())
}
