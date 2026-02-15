use std::{collections::HashSet, path::Path, sync::Arc};

use anyhow::{Context, Result};
use arrow_array::{Array, RecordBatch, RecordBatchIterator, StringArray};
use arrow_schema::{DataType, Schema};
use futures::TryStreamExt;
use lancedb::{
    connect,
    index::Index,
    query::{ExecutableQuery, QueryBase, Select},
    table::{NewColumnTransform, OptimizeAction, OptimizeOptions},
    Connection, Table,
};
use rand::{rngs::OsRng, Rng};

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
    let table = match db.open_table(name).execute().await {
        Ok(table) => table,
        Err(_) => {
            let batch = RecordBatch::new_empty(schema.clone());
            let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema.clone());
            db.create_table(name, Box::new(batches)).execute().await?;
            db.open_table(name).execute().await?
        },
    };

    ensure_table_columns(&table, schema.as_ref()).await?;
    Ok(table)
}

async fn ensure_table_columns(table: &Table, expected_schema: &Schema) -> Result<()> {
    let existing_schema = table.schema().await?;
    let existing_columns = existing_schema
        .fields()
        .iter()
        .map(|field| field.name().to_string())
        .collect::<HashSet<_>>();

    let mut missing_columns = Vec::new();
    for field in expected_schema.fields() {
        if existing_columns.contains(field.name()) {
            continue;
        }
        if !field.is_nullable() {
            anyhow::bail!(
                "table `{}` missing required non-nullable column `{}`; manual migration needed",
                table.name(),
                field.name()
            );
        }

        let sql_type = sql_type_for_column_cast(field.data_type()).with_context(|| {
            format!(
                "unsupported nullable column type for auto-migration: `{}` ({}) on table `{}`",
                field.name(),
                field.data_type(),
                table.name()
            )
        })?;
        missing_columns.push((field.name().to_string(), format!("cast(NULL as {sql_type})")));
    }

    if missing_columns.is_empty() {
        return Ok(());
    }

    let preview = missing_columns
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    tracing::info!(
        "Auto-migrating table `{}` by adding missing nullable columns: {}",
        table.name(),
        preview
    );

    table
        .add_columns(NewColumnTransform::SqlExpressions(missing_columns), None)
        .await
        .with_context(|| format!("failed to add missing columns to table `{}`", table.name()))?;

    Ok(())
}

fn sql_type_for_column_cast(data_type: &DataType) -> Result<&'static str> {
    match data_type {
        DataType::Utf8 => Ok("string"),
        DataType::Int32 => Ok("int32"),
        DataType::Timestamp(_, _) => Ok("timestamp_ms"),
        DataType::Binary => Ok("binary"),
        _ => anyhow::bail!("unsupported data type: {data_type}"),
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

/// Filename prefix for dedicated fallback cover images.
pub const FALLBACK_COVER_PREFIX: &str = "cover-default-";

/// Two-tier fallback for articles without a featured image.
///
/// Tier 1: images whose filename starts with [`FALLBACK_COVER_PREFIX`].
/// Tier 2: `featured_image` values from existing articles.
pub async fn query_fallback_cover(
    images_table: &Table,
    articles_table: &Table,
) -> Result<Option<String>> {
    // Tier 1: dedicated fallback covers
    let filter = format!("filename LIKE '{FALLBACK_COVER_PREFIX}%'");
    let batches = images_table
        .query()
        .only_if(filter)
        .select(Select::columns(&["id"]))
        .execute()
        .await?
        .try_collect::<Vec<_>>()
        .await?;

    let mut candidates: Vec<String> = Vec::new();
    for batch in &batches {
        if let Some(arr) = batch
            .column_by_name("id")
            .and_then(|col| col.as_any().downcast_ref::<StringArray>())
        {
            for i in 0..arr.len() {
                if !arr.is_null(i) {
                    candidates.push(format!("images/{}", arr.value(i)));
                }
            }
        }
    }

    if !candidates.is_empty() {
        let pick = pick_random_cover(&candidates);
        tracing::info!("Fallback cover (dedicated): {pick}");
        return Ok(Some(pick));
    }

    // Tier 2: reuse existing article covers
    let batches = articles_table
        .query()
        .only_if("featured_image IS NOT NULL")
        .select(Select::columns(&["featured_image"]))
        .execute()
        .await?
        .try_collect::<Vec<_>>()
        .await?;

    let mut candidates: Vec<String> = Vec::new();
    for batch in &batches {
        if let Some(arr) = batch
            .column_by_name("featured_image")
            .and_then(|col| col.as_any().downcast_ref::<StringArray>())
        {
            for i in 0..arr.len() {
                if !arr.is_null(i) {
                    let val = arr.value(i);
                    if !val.is_empty() {
                        candidates.push(val.to_string());
                    }
                }
            }
        }
    }

    if !candidates.is_empty() {
        let pick = pick_random_cover(&candidates);
        tracing::info!("Fallback cover (existing article): {pick}");
        return Ok(Some(pick));
    }

    tracing::debug!("No fallback cover image available");
    Ok(None)
}

fn pick_random_cover(candidates: &[String]) -> String {
    let mut rng = OsRng;
    let index = rng.gen_range(0..candidates.len());
    candidates[index].clone()
}
