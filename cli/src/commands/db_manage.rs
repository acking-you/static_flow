use std::{fs, path::Path};

use anyhow::{anyhow, bail, Context, Result};
use arrow::util::pretty::pretty_format_batches;
use arrow_array::{
    Array, BinaryArray, FixedSizeListArray, RecordBatch, StringArray, TimestampMillisecondArray,
};
use arrow_schema::{DataType, TimeUnit};
use chrono::Duration as ChronoDuration;
use futures::TryStreamExt;
use lancedb::{
    query::{ExecutableQuery, QueryBase, Select},
    table::{OptimizeAction, OptimizeOptions},
    Connection, Table,
};
use static_flow_shared::embedding::{
    embed_image_bytes, embed_text_with_language, TextEmbeddingLanguage,
};

use crate::{
    cli::QueryOutputFormat,
    db::{
        connect_db, ensure_fts_index, ensure_table, ensure_vector_index, upsert_articles,
        upsert_images,
    },
    schema::{article_schema, image_schema, taxonomy_schema, ArticleRecord, ImageRecord},
    utils::rasterize_svg_for_embedding,
};

const CLEANUP_TARGET_TABLES: [&str; 4] = ["articles", "images", "taxonomies", "article_views"];

#[derive(Debug, Clone)]
pub struct QueryRowsOptions {
    pub table: String,
    pub where_clause: Option<String>,
    pub columns: Vec<String>,
    pub limit: usize,
    pub offset: usize,
    pub format: QueryOutputFormat,
}

pub async fn list_tables(db_path: &Path, limit: u32) -> Result<()> {
    let db = connect_db(db_path).await?;
    let names = db.table_names().limit(limit).execute().await?;
    if names.is_empty() {
        tracing::info!("No tables found in {}", db_path.display());
        return Ok(());
    }

    tracing::info!("Tables ({}):", names.len());
    for name in names {
        tracing::info!("- {}", name);
    }
    Ok(())
}

pub async fn create_table(db_path: &Path, table: &str, replace: bool) -> Result<()> {
    let db = connect_db(db_path).await?;
    ensure_managed_table(&db, table, replace).await?;
    tracing::info!("Table `{}` is ready.", table);
    Ok(())
}

pub async fn drop_table(db_path: &Path, table: &str, yes: bool) -> Result<()> {
    if !yes {
        bail!("refusing to drop table without --yes")
    }

    let db = connect_db(db_path).await?;
    db.drop_table(table, &[])
        .await
        .with_context(|| format!("failed to drop table `{table}`"))?;
    tracing::info!("Dropped table `{}`.", table);
    Ok(())
}

pub async fn describe_table(db_path: &Path, table: &str) -> Result<()> {
    let db = connect_db(db_path).await?;
    let table = open_table(&db, table).await?;
    let schema = table.schema().await?;
    let row_count = table.count_rows(None).await?;

    tracing::info!("Table: {}", table.name());
    tracing::info!("Rows: {}", row_count);
    tracing::info!("Schema:");
    for field in schema.fields() {
        tracing::info!(
            "- {}: {}{}",
            field.name(),
            format_datatype(field.data_type()),
            if field.is_nullable() { " (nullable)" } else { "" }
        );
    }
    Ok(())
}

pub async fn count_rows(db_path: &Path, table: &str, where_clause: Option<String>) -> Result<()> {
    let db = connect_db(db_path).await?;
    let table = open_table(&db, table).await?;
    let count = match table.count_rows(where_clause).await {
        Ok(count) => count,
        Err(err) => {
            return Err(friendly_table_error(&table, "count rows", err.to_string()).await);
        },
    };
    tracing::info!("Row count: {}", count);
    Ok(())
}

pub async fn query_rows(db_path: &Path, options: QueryRowsOptions) -> Result<()> {
    let db = connect_db(db_path).await?;
    let table = open_table(&db, &options.table).await?;

    let projected_columns = normalize_columns(&options.columns);
    if !projected_columns.is_empty() {
        validate_columns(&table, &projected_columns, "query").await?;
    }

    let mut query = table.query().limit(options.limit).offset(options.offset);
    if let Some(filter) = options.where_clause {
        query = query.only_if(filter);
    }
    if !projected_columns.is_empty() {
        query = query.select(Select::columns(&projected_columns));
    }

    let stream = match query.execute().await {
        Ok(stream) => stream,
        Err(err) => {
            return Err(friendly_table_error(&table, "query rows", err.to_string()).await);
        },
    };
    let batches = stream
        .try_collect::<Vec<_>>()
        .await
        .map_err(|err| anyhow!("failed to read query result: {err}"))?;
    if batches.is_empty() {
        tracing::info!("No rows found.");
        return Ok(());
    }

    print_batches(&batches, options.format)?;
    Ok(())
}

pub async fn update_rows(
    db_path: &Path,
    table: &str,
    assignments: &[String],
    where_clause: Option<String>,
    all: bool,
) -> Result<()> {
    if assignments.is_empty() {
        bail!("at least one --set assignment is required")
    }
    if where_clause.is_none() && !all {
        bail!("update without --where is blocked; pass --all to update all rows")
    }

    let db = connect_db(db_path).await?;
    let table = open_table(&db, table).await?;

    let assignments = assignments
        .iter()
        .map(|item| parse_assignment(item))
        .collect::<Result<Vec<_>>>()?;
    let assignment_columns = assignments
        .iter()
        .map(|(column, _)| column.clone())
        .collect::<Vec<_>>();
    validate_columns(&table, &assignment_columns, "update").await?;

    let mut builder = table.update();
    if let Some(filter) = where_clause {
        builder = builder.only_if(filter);
    }

    for (column, expr) in assignments {
        builder = builder.column(column, expr);
    }

    let result = match builder.execute().await {
        Ok(result) => result,
        Err(err) => {
            return Err(friendly_table_error(&table, "update rows", err.to_string()).await);
        },
    };
    tracing::info!(
        "Update applied on `{}`: rows_updated={}, version={}",
        table.name(),
        result.rows_updated,
        result.version
    );
    Ok(())
}

pub async fn update_article_bilingual(
    db_path: &Path,
    id: &str,
    content_en_file: Option<&Path>,
    summary_zh_file: Option<&Path>,
    summary_en_file: Option<&Path>,
) -> Result<()> {
    if content_en_file.is_none() && summary_zh_file.is_none() && summary_en_file.is_none() {
        bail!(
            "nothing to update: provide --content-en-file and/or both --summary-zh-file \
             --summary-en-file"
        )
    }

    let summary_pair = match (summary_zh_file, summary_en_file) {
        (Some(zh), Some(en)) => Some((zh, en)),
        (None, None) => None,
        _ => bail!("summary update requires both --summary-zh-file and --summary-en-file"),
    };

    let content_en = match content_en_file {
        Some(path) => Some(
            fs::read_to_string(path)
                .with_context(|| format!("failed to read --content-en-file {}", path.display()))?,
        ),
        None => None,
    };

    let detailed_summary = match summary_pair {
        Some((zh_path, en_path)) => {
            let zh = fs::read_to_string(zh_path).with_context(|| {
                format!("failed to read --summary-zh-file {}", zh_path.display())
            })?;
            let en = fs::read_to_string(en_path).with_context(|| {
                format!("failed to read --summary-en-file {}", en_path.display())
            })?;
            Some(
                serde_json::json!({
                    "zh": zh,
                    "en": en,
                })
                .to_string(),
            )
        },
        None => None,
    };

    let db = connect_db(db_path).await?;
    let table = open_table(&db, "articles").await?;

    let mut builder = table
        .update()
        .only_if(format!("id = {}", sql_string_literal(id)));
    if let Some(value) = &content_en {
        builder = builder.column("content_en", sql_string_literal(value));
    }
    if let Some(value) = &detailed_summary {
        builder = builder.column("detailed_summary", sql_string_literal(value));
    }

    let result = match builder.execute().await {
        Ok(result) => result,
        Err(err) => {
            return Err(friendly_table_error(
                &table,
                "update article bilingual fields",
                err.to_string(),
            )
            .await);
        },
    };

    if result.rows_updated == 0 {
        bail!("article not found: `{id}`")
    }

    tracing::info!(
        "Article bilingual update applied: id=`{}`, rows_updated={}, version={}",
        id,
        result.rows_updated,
        result.version
    );
    Ok(())
}

pub async fn delete_rows(
    db_path: &Path,
    table: &str,
    where_clause: Option<String>,
    all: bool,
) -> Result<()> {
    let predicate = match where_clause {
        Some(predicate) => predicate,
        None if all => "true".to_string(),
        None => bail!("delete without --where is blocked; pass --all to delete all rows"),
    };

    let db = connect_db(db_path).await?;
    let table = open_table(&db, table).await?;
    let result = match table.delete(&predicate).await {
        Ok(result) => result,
        Err(err) => {
            return Err(friendly_table_error(&table, "delete rows", err.to_string()).await);
        },
    };
    tracing::info!("Delete applied on `{}`: version={}", table.name(), result.version);
    Ok(())
}

pub async fn ensure_indexes(db_path: &Path, table: Option<String>) -> Result<()> {
    let db = connect_db(db_path).await?;

    match table.as_deref() {
        Some("articles") => ensure_article_indexes(&db).await?,
        Some("images") => ensure_image_indexes(&db).await?,
        Some("taxonomies") => {
            tracing::info!("No indexes configured for `taxonomies` table.");
        },
        Some(other) => {
            bail!("unsupported table `{other}`, expected `articles`, `images`, or `taxonomies`")
        },
        None => {
            ensure_article_indexes(&db).await?;
            ensure_image_indexes(&db).await?;
        },
    }

    tracing::info!("Index ensure run completed.");
    Ok(())
}

pub async fn list_indexes(db_path: &Path, table: &str, with_stats: bool) -> Result<()> {
    let db = connect_db(db_path).await?;
    let table = open_table(&db, table).await?;
    let indexes = table.list_indices().await?;

    if indexes.is_empty() {
        tracing::info!("No indexes found for `{}`.", table.name());
        return Ok(());
    }

    tracing::info!("Indexes on `{}`:", table.name());
    for index in indexes {
        tracing::info!(
            "- {} | type={} | columns={}",
            index.name,
            index.index_type,
            index.columns.join(",")
        );

        if with_stats {
            if let Some(stats) = table.index_stats(&index.name).await? {
                tracing::info!(
                    "  indexed_rows={}, unindexed_rows={}, distance={:?}, parts={:?}",
                    stats.num_indexed_rows,
                    stats.num_unindexed_rows,
                    stats.distance_type,
                    stats.num_indices
                );
            }
        }
    }
    Ok(())
}

pub async fn drop_index(db_path: &Path, table: &str, name: &str) -> Result<()> {
    let db = connect_db(db_path).await?;
    let table = open_table(&db, table).await?;
    table.drop_index(name).await?;
    tracing::info!("Dropped index `{}` from `{}`.", name, table.name());
    Ok(())
}

pub async fn optimize_table(db_path: &Path, table: &str, all: bool, prune_now: bool) -> Result<()> {
    let db = connect_db(db_path).await?;
    let table = open_table(&db, table).await?;

    let action =
        if all { OptimizeAction::All } else { OptimizeAction::Index(OptimizeOptions::default()) };

    let _ = table.optimize(action).await?;

    if prune_now {
        let _ = table
            .optimize(OptimizeAction::Prune {
                older_than: Some(ChronoDuration::zero()),
                delete_unverified: Some(true),
                error_if_tagged_old_versions: Some(false),
            })
            .await?;
        tracing::info!(
            "Immediate prune completed for `{}` (older_than=0, delete_unverified=true).",
            table.name()
        );
    }

    tracing::info!(
        "Optimization completed for `{}` ({})",
        table.name(),
        if all { "all" } else { "index-only" }
    );
    Ok(())
}

pub async fn cleanup_orphans(db_path: &Path, table: Option<&str>) -> Result<()> {
    let db = connect_db(db_path).await?;
    let targets = resolve_cleanup_targets(table)?;
    let allow_missing = table.is_none();

    for target in targets {
        let table = match db.open_table(target).execute().await {
            Ok(table) => table,
            Err(err) => {
                if allow_missing {
                    tracing::warn!("Skip cleanup for missing table `{}`: {}", target, err);
                    continue;
                }
                return Err(anyhow::anyhow!("failed to open table `{}`: {}", target, err));
            },
        };
        let _ = table
            .optimize(OptimizeAction::Prune {
                older_than: Some(ChronoDuration::zero()),
                delete_unverified: Some(true),
                error_if_tagged_old_versions: Some(false),
            })
            .await?;
        tracing::info!(
            "Orphan cleanup completed for `{}` (older_than=0, delete_unverified=true).",
            table.name()
        );
    }

    Ok(())
}

pub async fn reembed_svg_images(db_path: &Path, limit: Option<usize>, dry_run: bool) -> Result<()> {
    let db = connect_db(db_path).await?;
    let table = open_table(&db, "images").await?;

    let batches = table
        .query()
        .only_if("filename LIKE '%.svg' OR filename LIKE '%.SVG'")
        .select(Select::columns(&["id", "filename", "data", "thumbnail", "metadata", "created_at"]))
        .execute()
        .await?
        .try_collect::<Vec<_>>()
        .await?;

    if batches.is_empty() {
        tracing::info!("No SVG rows found in `images`.");
        return Ok(());
    }

    let mut updates = Vec::<ImageRecord>::new();
    let mut scanned = 0usize;
    let mut candidates = 0usize;
    let mut skipped_rasterize = 0usize;

    for batch in &batches {
        let ids = downcast_string(batch, "id")?;
        let filenames = downcast_string(batch, "filename")?;
        let data = downcast_binary(batch, "data")?;
        let thumbs = downcast_binary(batch, "thumbnail")?;
        let metadata = downcast_string(batch, "metadata")?;
        let created = downcast_timestamp_ms(batch, "created_at")?;

        for row in 0..batch.num_rows() {
            scanned += 1;
            if let Some(max) = limit {
                if candidates >= max {
                    break;
                }
            }

            let filename = filenames.value(row).to_string();
            let bytes = data.value(row).to_vec();
            let Some(rasterized) =
                rasterize_svg_for_embedding(Path::new(&filename), bytes.as_slice())?
            else {
                skipped_rasterize += 1;
                continue;
            };
            candidates += 1;

            let mut metadata_value = serde_json::from_str::<serde_json::Value>(metadata.value(row))
                .unwrap_or_else(|_| serde_json::json!({}));
            if !metadata_value.is_object() {
                metadata_value = serde_json::json!({
                    "raw_metadata": metadata_value,
                });
            }
            metadata_value["width"] = serde_json::json!(rasterized.width);
            metadata_value["height"] = serde_json::json!(rasterized.height);
            metadata_value["embedding_input"] = serde_json::json!("svg_rasterized_png");

            let thumbnail =
                if thumbs.is_null(row) { None } else { Some(thumbs.value(row).to_vec()) };

            updates.push(ImageRecord {
                id: ids.value(row).to_string(),
                filename,
                data: bytes,
                thumbnail,
                vector: embed_image_bytes(&rasterized.png_bytes),
                metadata: metadata_value.to_string(),
                created_at: created.value(row),
            });
        }
    }

    if candidates == 0 {
        tracing::info!(
            "No SVG rows were eligible for re-embedding (scanned={}, skipped_rasterize={}).",
            scanned,
            skipped_rasterize
        );
        return Ok(());
    }

    if dry_run {
        tracing::info!(
            "Dry run: {} SVG rows would be re-embedded (scanned={}, skipped_rasterize={}).",
            candidates,
            scanned,
            skipped_rasterize
        );
        return Ok(());
    }

    for chunk in updates.chunks(32) {
        upsert_images(&table, chunk).await?;
    }

    if let Err(err) = ensure_vector_index(&table, "vector").await {
        tracing::warn!("Failed to ensure vector index after SVG re-embed: {err}");
    }

    tracing::info!(
        "SVG re-embed completed: updated={}, scanned={}, skipped_rasterize={}",
        candidates,
        scanned,
        skipped_rasterize
    );
    Ok(())
}

pub async fn backfill_article_vectors(
    db_path: &Path,
    limit: Option<usize>,
    dry_run: bool,
) -> Result<()> {
    if limit == Some(0) {
        tracing::info!("Skip backfill: --limit=0.");
        return Ok(());
    }

    let db = connect_db(db_path).await?;
    let table = open_table(&db, "articles").await?;

    let filter = "(vector_zh IS NULL AND content IS NOT NULL AND content != '') OR (vector_en IS \
                  NULL AND content_en IS NOT NULL AND content_en != '')";
    let columns = ["id", "content", "content_en", "vector_en", "vector_zh"];

    let stream = match table
        .query()
        .only_if(filter.to_string())
        .select(Select::columns(&columns))
        .execute()
        .await
    {
        Ok(stream) => stream,
        Err(err) => {
            return Err(
                friendly_table_error(&table, "backfill article vectors", err.to_string()).await
            );
        },
    };

    let batches = stream
        .try_collect::<Vec<_>>()
        .await
        .map_err(|err| anyhow!("failed to read candidate rows for vector backfill: {err}"))?;

    if batches.is_empty() {
        tracing::info!("No article rows matched vector-backfill candidates.");
        return Ok(());
    }

    let now_ms = chrono::Utc::now().timestamp_millis();
    let mut scanned = 0usize;
    let mut candidates = 0usize;
    let mut filled_vector_zh = 0usize;
    let mut filled_vector_en = 0usize;
    let mut updates_vector_en = Vec::<(String, Vec<f32>)>::new();
    let mut updates_vector_zh = Vec::<(String, Vec<f32>)>::new();

    'scan: for batch in &batches {
        let ids = downcast_string(batch, "id")?;
        let contents = downcast_string(batch, "content")?;
        let contents_en = downcast_string(batch, "content_en")?;
        let vectors_en = downcast_fixed_size_list(batch, "vector_en")?;
        let vectors_zh = downcast_fixed_size_list(batch, "vector_zh")?;

        for row in 0..batch.num_rows() {
            scanned += 1;
            let id = ids.value(row).to_string();
            let content = contents.value(row);
            let content_en = nullable_string(contents_en, row);
            let should_fill_vector_zh = vectors_zh.is_null(row) && !content.trim().is_empty();
            let should_fill_vector_en = vectors_en.is_null(row)
                && content_en
                    .as_ref()
                    .map(|value| !value.trim().is_empty())
                    .unwrap_or(false);
            if !should_fill_vector_zh && !should_fill_vector_en {
                continue;
            }

            if let Some(max) = limit {
                if candidates >= max {
                    break 'scan;
                }
            }

            if should_fill_vector_zh {
                let vector = embed_text_with_language(content, TextEmbeddingLanguage::Chinese);
                updates_vector_zh.push((id.clone(), vector));
                filled_vector_zh += 1;
            }
            if should_fill_vector_en {
                if let Some(content_en) = &content_en {
                    let vector =
                        embed_text_with_language(content_en, TextEmbeddingLanguage::English);
                    updates_vector_en.push((id.clone(), vector));
                    filled_vector_en += 1;
                }
            }

            candidates += 1;
        }
    }

    if candidates == 0 {
        tracing::info!("No article rows need vector backfill after candidate scan.");
        return Ok(());
    }

    if dry_run {
        tracing::info!(
            "Dry run: {} article rows would be backfilled (scanned={}, fill_vector_zh={}, \
             fill_vector_en={}).",
            candidates,
            scanned,
            filled_vector_zh,
            filled_vector_en
        );
        return Ok(());
    }

    apply_article_vector_updates(
        &table,
        "vector_en",
        static_flow_shared::embedding::TEXT_VECTOR_DIM_EN,
        &updates_vector_en,
        now_ms,
    )
    .await?;
    apply_article_vector_updates(
        &table,
        "vector_zh",
        static_flow_shared::embedding::TEXT_VECTOR_DIM_ZH,
        &updates_vector_zh,
        now_ms,
    )
    .await?;

    if let Err(err) = ensure_vector_index(&table, "vector_en").await {
        tracing::warn!("Failed to ensure vector index on articles (vector_en): {err}");
    }
    if let Err(err) = ensure_vector_index(&table, "vector_zh").await {
        tracing::warn!("Failed to ensure vector index on articles (vector_zh): {err}");
    }

    tracing::info!(
        "Article vector backfill completed: updated={}, scanned={}, filled_vector_zh={}, \
         filled_vector_en={}",
        candidates,
        scanned,
        filled_vector_zh,
        filled_vector_en
    );
    Ok(())
}

async fn apply_article_vector_updates(
    table: &Table,
    vector_column: &str,
    vector_dim: usize,
    updates: &[(String, Vec<f32>)],
    updated_at_ms: i64,
) -> Result<()> {
    if updates.is_empty() {
        return Ok(());
    }

    for chunk in updates.chunks(32) {
        let batch =
            build_article_vector_update_batch(vector_column, vector_dim, chunk, updated_at_ms)?;
        let schema = batch.schema();
        let batches = arrow_array::RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
        let mut merge = table.merge_insert(&["id"]);
        merge.when_matched_update_all(None);
        merge.execute(Box::new(batches)).await?;
    }

    Ok(())
}

fn build_article_vector_update_batch(
    vector_column: &str,
    vector_dim: usize,
    updates: &[(String, Vec<f32>)],
    updated_at_ms: i64,
) -> Result<RecordBatch> {
    let schema = std::sync::Arc::new(arrow_schema::Schema::new(vec![
        arrow_schema::Field::new("id", DataType::Utf8, false),
        arrow_schema::Field::new(
            vector_column,
            DataType::FixedSizeList(
                std::sync::Arc::new(arrow_schema::Field::new("item", DataType::Float32, true)),
                vector_dim as i32,
            ),
            true,
        ),
        arrow_schema::Field::new(
            "updated_at",
            DataType::Timestamp(TimeUnit::Millisecond, None),
            false,
        ),
    ]));

    let mut id_builder = arrow_array::builder::StringBuilder::new();
    let mut updated_at_builder = arrow_array::builder::TimestampMillisecondBuilder::new();
    let mut flat_vector_values = Vec::<f32>::with_capacity(updates.len() * vector_dim);

    for (id, vector) in updates {
        if vector.len() != vector_dim {
            bail!(
                "vector length mismatch for `{}`: expected {}, got {}",
                id,
                vector_dim,
                vector.len()
            );
        }
        id_builder.append_value(id);
        flat_vector_values.extend_from_slice(vector);
        updated_at_builder.append_value(updated_at_ms);
    }

    let value_array = std::sync::Arc::new(arrow_array::Float32Array::from(flat_vector_values))
        as arrow_array::ArrayRef;
    let vector_array = arrow_array::FixedSizeListArray::new(
        std::sync::Arc::new(arrow_schema::Field::new("item", DataType::Float32, true)),
        vector_dim as i32,
        value_array,
        None,
    );

    let arrays: Vec<arrow_array::ArrayRef> = vec![
        std::sync::Arc::new(id_builder.finish()),
        std::sync::Arc::new(vector_array),
        std::sync::Arc::new(updated_at_builder.finish()),
    ];

    Ok(RecordBatch::try_new(schema, arrays)?)
}

pub async fn upsert_article_json(db_path: &Path, json: &str) -> Result<()> {
    let mut record: ArticleRecord = serde_json::from_str(json).context("invalid article JSON")?;
    let now = chrono::Utc::now().timestamp_millis();
    if record.created_at == 0 {
        record.created_at = now;
    }
    if record.updated_at == 0 {
        record.updated_at = now;
    }

    let db = connect_db(db_path).await?;
    let table = open_table(&db, "articles").await?;
    upsert_articles(&table, &[record]).await?;
    tracing::info!("Upserted one article row.");
    Ok(())
}

pub async fn upsert_image_json(db_path: &Path, json: &str) -> Result<()> {
    let mut record: ImageRecord = serde_json::from_str(json).context("invalid image JSON")?;
    if record.created_at == 0 {
        record.created_at = chrono::Utc::now().timestamp_millis();
    }

    let db = connect_db(db_path).await?;
    let table = open_table(&db, "images").await?;
    upsert_images(&table, &[record]).await?;
    tracing::info!("Upserted one image row.");
    Ok(())
}

async fn ensure_article_indexes(db: &Connection) -> Result<()> {
    let table = open_table(db, "articles").await?;
    if let Err(err) = ensure_fts_index(&table, "content").await {
        tracing::warn!("Failed to create FTS index on articles: {err}");
    }
    if let Err(err) = ensure_vector_index(&table, "vector_en").await {
        tracing::warn!("Failed to create vector index on articles (vector_en): {err}");
    }
    if let Err(err) = ensure_vector_index(&table, "vector_zh").await {
        tracing::warn!("Failed to create vector index on articles (vector_zh): {err}");
    }
    Ok(())
}

async fn ensure_image_indexes(db: &Connection) -> Result<()> {
    let table = open_table(db, "images").await?;
    if let Err(err) = ensure_vector_index(&table, "vector").await {
        tracing::warn!("Failed to create vector index on images: {err}");
    }
    Ok(())
}

fn resolve_cleanup_targets(table: Option<&str>) -> Result<Vec<&'static str>> {
    match table {
        Some(name) => {
            if CLEANUP_TARGET_TABLES.contains(&name) {
                Ok(vec![CLEANUP_TARGET_TABLES
                    .iter()
                    .find(|&&candidate| candidate == name)
                    .copied()
                    .expect("managed table existence already checked")])
            } else {
                bail!(
                    "unsupported table `{name}`, expected one of: {}",
                    CLEANUP_TARGET_TABLES.join(", ")
                )
            }
        },
        None => Ok(CLEANUP_TARGET_TABLES.to_vec()),
    }
}

fn parse_assignment(assignment: &str) -> Result<(String, String)> {
    let (column, expr) = assignment
        .split_once('=')
        .ok_or_else(|| anyhow!("invalid assignment `{assignment}`, expected column=expression"))?;
    let column = column.trim();
    let expr = expr.trim();

    if column.is_empty() || expr.is_empty() {
        bail!("invalid assignment `{assignment}`, empty column or expression")
    }

    Ok((column.to_string(), expr.to_string()))
}

fn sql_string_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn format_datatype(data_type: &DataType) -> String {
    match data_type {
        DataType::List(field) => format!("list<{}>", format_datatype(field.data_type())),
        DataType::FixedSizeList(field, size) => {
            format!("fixed_size_list<{}; {}>", format_datatype(field.data_type()), size)
        },
        DataType::Timestamp(TimeUnit::Millisecond, _) => "timestamp_ms".to_string(),
        other => other.to_string(),
    }
}

async fn ensure_managed_table(db: &Connection, table: &str, replace: bool) -> Result<()> {
    match table {
        "articles" => {
            if replace {
                let _ = db.drop_table("articles", &[]).await;
            }
            ensure_table(db, "articles", article_schema()).await?;
        },
        "images" => {
            if replace {
                let _ = db.drop_table("images", &[]).await;
            }
            ensure_table(db, "images", image_schema()).await?;
        },
        "taxonomies" => {
            if replace {
                let _ = db.drop_table("taxonomies", &[]).await;
            }
            ensure_table(db, "taxonomies", taxonomy_schema()).await?;
        },
        _ => bail!("unsupported table `{table}`, expected `articles`, `images`, or `taxonomies`"),
    }
    Ok(())
}

async fn open_table(db: &Connection, table: &str) -> Result<Table> {
    match db.open_table(table).execute().await {
        Ok(table) => Ok(table),
        Err(_) => {
            let available = db
                .table_names()
                .limit(200)
                .execute()
                .await
                .unwrap_or_default();
            if available.is_empty() {
                bail!("table `{table}` not found. No tables exist yet. Run `sf-cli init` first.");
            }

            let suggestions = suggest_names(table, &available);
            let mut message =
                format!("table `{table}` not found. Available tables: {}", available.join(", "));
            if !suggestions.is_empty() {
                message.push_str(&format!(". Did you mean: {}", suggestions.join(", ")));
            }
            bail!(message)
        },
    }
}

fn normalize_columns(columns: &[String]) -> Vec<String> {
    columns
        .iter()
        .map(|column| column.trim())
        .filter(|column| !column.is_empty())
        .map(|column| column.to_string())
        .collect()
}

async fn validate_columns(table: &Table, columns: &[String], operation: &str) -> Result<()> {
    if columns.is_empty() {
        return Ok(());
    }

    let schema = table
        .schema()
        .await
        .with_context(|| format!("failed to read schema for table `{}`", table.name()))?;
    let available = schema
        .fields()
        .iter()
        .map(|field| field.name().to_string())
        .collect::<Vec<_>>();

    let unknown = columns
        .iter()
        .filter(|column| !available.iter().any(|field| field == *column))
        .cloned()
        .collect::<Vec<_>>();

    if unknown.is_empty() {
        return Ok(());
    }

    let mut details = format!(
        "unknown column(s) for {} on table `{}`: {}. Schema columns: {}",
        operation,
        table.name(),
        unknown.join(", "),
        available.join(", ")
    );

    let mut suggestions = Vec::new();
    for column in &unknown {
        for suggestion in suggest_names(column, &available) {
            if !suggestions.iter().any(|item| item == &suggestion) {
                suggestions.push(suggestion);
            }
        }
    }
    if !suggestions.is_empty() {
        details.push_str(&format!(". Did you mean: {}", suggestions.join(", ")));
    }

    bail!(details)
}

async fn friendly_table_error(table: &Table, operation: &str, raw_error: String) -> anyhow::Error {
    if !is_schema_related_error(&raw_error) {
        return anyhow!("failed to {} on table `{}`: {}", operation, table.name(), raw_error);
    }

    let schema_columns = table
        .schema()
        .await
        .map(|schema| {
            schema
                .fields()
                .iter()
                .map(|field| field.name().to_string())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if schema_columns.is_empty() {
        anyhow!(
            "failed to {} on table `{}`: {}",
            operation,
            table.name(),
            compact_schema_error(&raw_error)
        )
    } else {
        anyhow!(
            "failed to {} on table `{}`: {}. Schema columns: {}",
            operation,
            table.name(),
            compact_schema_error(&raw_error),
            schema_columns.join(", ")
        )
    }
}

fn is_schema_related_error(raw_error: &str) -> bool {
    let lower = raw_error.to_lowercase();
    lower.contains("schema error") || lower.contains("no field named")
}

fn compact_schema_error(raw_error: &str) -> String {
    raw_error
        .split(',')
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(raw_error)
        .to_string()
}

fn suggest_names(input: &str, candidates: &[String]) -> Vec<String> {
    let needle = input.trim().to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }

    let mut scored = candidates
        .iter()
        .filter_map(|candidate| {
            let value = candidate.to_lowercase();
            let score = if value == needle {
                0
            } else if value.starts_with(&needle) || needle.starts_with(&value) {
                1
            } else if value.contains(&needle) || needle.contains(&value) {
                2
            } else {
                return None;
            };
            Some((score, candidate.clone()))
        })
        .collect::<Vec<_>>();

    scored.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    scored
        .into_iter()
        .map(|(_, candidate)| candidate)
        .take(3)
        .collect()
}

fn print_batches(batches: &[arrow_array::RecordBatch], format: QueryOutputFormat) -> Result<()> {
    match format {
        QueryOutputFormat::Table => {
            let formatted = pretty_format_batches(batches)?;
            tracing::info!("\n{formatted}");
        },
        QueryOutputFormat::Vertical => {
            let content = format_vertical_batches(batches)?;
            tracing::info!("\n{content}");
        },
    }
    Ok(())
}

fn format_vertical_batches(batches: &[arrow_array::RecordBatch]) -> Result<String> {
    let mut output = String::new();
    let mut row_no = 1usize;

    for batch in batches {
        let schema = batch.schema();
        for row_idx in 0..batch.num_rows() {
            output.push_str(&format!(
                "*************************** [{}] ***************************\n",
                row_no
            ));

            for (col_idx, field) in schema.fields().iter().enumerate() {
                let array = batch.column(col_idx);
                let value = arrow::util::display::array_value_to_string(array.as_ref(), row_idx)
                    .unwrap_or_else(|_| "<error>".to_string());
                output.push_str(&format!("{}: {}\n", field.name(), value));
            }
            output.push('\n');
            row_no += 1;
        }
    }

    if output.is_empty() {
        output.push_str("(no rows)\n");
    }

    Ok(output)
}

fn downcast_string<'a>(batch: &'a RecordBatch, column: &str) -> Result<&'a StringArray> {
    let index = batch
        .schema()
        .index_of(column)
        .with_context(|| format!("missing column `{column}`"))?;
    batch
        .column(index)
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| anyhow!("column `{column}` is not StringArray"))
}

fn downcast_fixed_size_list<'a>(
    batch: &'a RecordBatch,
    column: &str,
) -> Result<&'a FixedSizeListArray> {
    let index = batch
        .schema()
        .index_of(column)
        .with_context(|| format!("missing column `{column}`"))?;
    batch
        .column(index)
        .as_any()
        .downcast_ref::<FixedSizeListArray>()
        .ok_or_else(|| anyhow!("column `{column}` is not FixedSizeListArray"))
}

fn nullable_string(array: &StringArray, row: usize) -> Option<String> {
    if array.is_null(row) {
        None
    } else {
        Some(array.value(row).to_string())
    }
}

fn downcast_binary<'a>(batch: &'a RecordBatch, column: &str) -> Result<&'a BinaryArray> {
    let index = batch
        .schema()
        .index_of(column)
        .with_context(|| format!("missing column `{column}`"))?;
    batch
        .column(index)
        .as_any()
        .downcast_ref::<BinaryArray>()
        .ok_or_else(|| anyhow!("column `{column}` is not BinaryArray"))
}

fn downcast_timestamp_ms<'a>(
    batch: &'a RecordBatch,
    column: &str,
) -> Result<&'a TimestampMillisecondArray> {
    let index = batch
        .schema()
        .index_of(column)
        .with_context(|| format!("missing column `{column}`"))?;
    batch
        .column(index)
        .as_any()
        .downcast_ref::<TimestampMillisecondArray>()
        .ok_or_else(|| anyhow!("column `{column}` is not TimestampMillisecondArray"))
}
