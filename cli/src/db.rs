use std::{collections::HashSet, path::Path, sync::Arc};

use anyhow::{Context, Result};
use arrow_array::{RecordBatch, RecordBatchIterator};
use arrow_schema::Schema;
use lancedb::{
    connect,
    index::Index,
    table::{OptimizeAction, OptimizeOptions},
    Connection, Table,
};

use crate::schema::{
    build_article_batch, build_image_batch, build_taxonomy_batch, ArticleRecord, ImageRecord,
    TaxonomyRecord,
};

const MIN_VECTOR_INDEX_TRAIN_ROWS: usize = 256;

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
            db.create_table(name, Box::new(batches)).execute().await?;
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
    let indices = table.list_indices().await?;
    if indices.iter().any(|index| index.columns == [column]) {
        return Ok(());
    }

    let filter = format!("{column} IS NOT NULL");
    let row_count = table.count_rows(Some(filter)).await?;

    if row_count < MIN_VECTOR_INDEX_TRAIN_ROWS {
        tracing::debug!(
            "Skip creating vector index on {column}: rows={row_count}, need at least \
             {MIN_VECTOR_INDEX_TRAIN_ROWS}"
        );
        return Ok(());
    }

    match table.create_index(&[column], Index::Auto).execute().await {
        Ok(_) => Ok(()),
        Err(err) => {
            if err.to_string().contains("Not enough rows to train PQ") {
                tracing::debug!(
                    "Skip vector index on {column}: insufficient rows for PQ training ({err})"
                );
                Ok(())
            } else {
                Err(err.into())
            }
        },
    }
}

pub async fn optimize_table_indexes(table: &Table) -> Result<()> {
    let _ = table
        .optimize(OptimizeAction::Index(OptimizeOptions::default()))
        .await?;
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

    // NOTE:
    // LanceDB merge_insert on multi-row image batches (binary + vector columns)
    // may insert duplicate ids in some versions. Use per-row merge to guarantee
    // deterministic upsert semantics.
    let mut seen = HashSet::new();
    for record in records {
        if !seen.insert(record.id.clone()) {
            continue;
        }

        let batch = build_image_batch(std::slice::from_ref(record))?;
        let schema = batch.schema();
        let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);

        let mut merge = table.merge_insert(&["id"]);
        merge.when_matched_update_all(None);
        merge.when_not_matched_insert_all();
        merge.execute(Box::new(batches)).await?;
    }

    Ok(())
}

pub async fn upsert_taxonomies(table: &Table, records: &[TaxonomyRecord]) -> Result<()> {
    if records.is_empty() {
        return Ok(());
    }

    let batch = build_taxonomy_batch(records)?;
    let schema = batch.schema();
    let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);

    let mut merge = table.merge_insert(&["id"]);
    merge.when_matched_update_all(None);
    merge.when_not_matched_insert_all();
    merge.execute(Box::new(batches)).await?;
    Ok(())
}
