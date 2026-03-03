#[cfg(not(target_arch = "wasm32"))]
use std::sync::Arc;

#[cfg(not(target_arch = "wasm32"))]
use anyhow::{Context, Result};
#[cfg(not(target_arch = "wasm32"))]
use arrow_array::{
    builder::{FixedSizeListBuilder, Float32Builder, StringBuilder},
    Array, BinaryArray, RecordBatch, RecordBatchIterator, StringArray,
};
#[cfg(not(target_arch = "wasm32"))]
use arrow_schema::{DataType, Field, Schema};
#[cfg(not(target_arch = "wasm32"))]
use futures::TryStreamExt;
#[cfg(not(target_arch = "wasm32"))]
use lancedb::{
    query::{ExecutableQuery, QueryBase, Select},
    Table,
};

#[cfg(not(target_arch = "wasm32"))]
use crate::embedding::{embed_image_bytes, IMAGE_VECTOR_DIM};

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageReembedScope {
    MissingOnly,
    All,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone)]
pub struct ImageReembedOptions {
    pub scope: ImageReembedScope,
    pub limit: Option<usize>,
    pub dry_run: bool,
    pub batch_size: usize,
}

#[cfg(not(target_arch = "wasm32"))]
impl Default for ImageReembedOptions {
    fn default() -> Self {
        Self {
            scope: ImageReembedScope::MissingOnly,
            limit: None,
            dry_run: false,
            batch_size: 32,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, Default)]
pub struct ImageReembedStats {
    pub scanned_rows: usize,
    pub embedded_rows: usize,
    pub embedding_failed_rows: usize,
    pub update_candidates: usize,
    pub updated_rows: usize,
    pub skipped_failed_rows: usize,
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn reembed_image_vectors(
    table: &Table,
    mut options: ImageReembedOptions,
) -> Result<ImageReembedStats> {
    if options.limit == Some(0) {
        return Ok(ImageReembedStats::default());
    }
    if options.batch_size == 0 {
        options.batch_size = 1;
    }

    let mut query = table
        .query()
        .select(Select::columns(&["id", "filename", "data"]));
    if options.scope == ImageReembedScope::MissingOnly {
        query = query.only_if("vector IS NULL");
    }
    if let Some(limit) = options.limit {
        query = query.limit(limit);
    }

    let batches = query
        .execute()
        .await
        .context("failed to query image rows for vector re-embed")?
        .try_collect::<Vec<_>>()
        .await
        .context("failed to read image rows for vector re-embed")?;

    let mut updates: Vec<(String, Option<Vec<f32>>)> = Vec::new();
    let mut stats = ImageReembedStats::default();

    for batch in &batches {
        let ids = downcast_string(batch, "id")?;
        let filenames = downcast_string(batch, "filename")?;
        let data = downcast_binary(batch, "data")?;

        for row in 0..batch.num_rows() {
            stats.scanned_rows += 1;
            let id = ids.value(row).to_string();
            let filename = filenames.value(row);
            let image_bytes = data.value(row);

            match embed_image_bytes(image_bytes) {
                Ok(vector) => {
                    stats.embedded_rows += 1;
                    updates.push((id, Some(vector)));
                },
                Err(err) => {
                    stats.embedding_failed_rows += 1;
                    tracing::warn!(
                        "Image embedding failed; id={}; filename={}; bytes={}: {}",
                        id,
                        filename,
                        image_bytes.len(),
                        err
                    );
                    if options.scope == ImageReembedScope::All {
                        updates.push((id, None));
                    } else {
                        stats.skipped_failed_rows += 1;
                    }
                },
            }
        }
    }

    stats.update_candidates = updates.len();

    if options.dry_run || updates.is_empty() {
        return Ok(stats);
    }

    for chunk in updates.chunks(options.batch_size) {
        let batch = build_image_vector_update_batch(chunk)?;
        let schema = batch.schema();
        let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
        let mut merge = table.merge_insert(&["id"]);
        merge.when_matched_update_all(None);
        merge.execute(Box::new(batches)).await?;
        stats.updated_rows += chunk.len();
    }

    Ok(stats)
}

#[cfg(not(target_arch = "wasm32"))]
fn build_image_vector_update_batch(updates: &[(String, Option<Vec<f32>>)]) -> Result<RecordBatch> {
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new(
            "vector",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, false)),
                IMAGE_VECTOR_DIM as i32,
            ),
            true,
        ),
    ]));

    let mut id_builder = StringBuilder::new();
    let mut vector_builder =
        FixedSizeListBuilder::new(Float32Builder::new(), IMAGE_VECTOR_DIM as i32)
            .with_field(Field::new_list_field(DataType::Float32, false));

    for (id, vector) in updates {
        id_builder.append_value(id);
        match vector {
            Some(values) => {
                if values.len() != IMAGE_VECTOR_DIM {
                    anyhow::bail!(
                        "image vector length mismatch for id `{id}`: expected {}, got {}",
                        IMAGE_VECTOR_DIM,
                        values.len()
                    );
                }
                for value in values {
                    vector_builder.values().append_value(*value);
                }
                vector_builder.append(true);
            },
            None => {
                for _ in 0..IMAGE_VECTOR_DIM {
                    vector_builder.values().append_value(0.0);
                }
                vector_builder.append(false);
            },
        }
    }

    Ok(RecordBatch::try_new(schema, vec![
        Arc::new(id_builder.finish()),
        Arc::new(vector_builder.finish()),
    ])?)
}

#[cfg(not(target_arch = "wasm32"))]
fn downcast_string<'a>(batch: &'a RecordBatch, column: &str) -> Result<&'a StringArray> {
    let index = batch
        .schema()
        .index_of(column)
        .with_context(|| format!("missing column `{column}`"))?;
    batch
        .column(index)
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| anyhow::anyhow!("column `{column}` is not StringArray"))
}

#[cfg(not(target_arch = "wasm32"))]
fn downcast_binary<'a>(batch: &'a RecordBatch, column: &str) -> Result<&'a BinaryArray> {
    let index = batch
        .schema()
        .index_of(column)
        .with_context(|| format!("missing column `{column}`"))?;
    batch
        .column(index)
        .as_any()
        .downcast_ref::<BinaryArray>()
        .ok_or_else(|| anyhow::anyhow!("column `{column}` is not BinaryArray"))
}
