use std::{
    collections::{BTreeSet, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, bail, Context, Result};
use arrow::{compute::cast, util::pretty::pretty_format_batches};
use arrow_array::{
    Array, ArrayRef, BinaryArray, FixedSizeListArray, LargeBinaryArray, RecordBatch,
    RecordBatchIterator, RecordBatchReader, StringArray, TimestampMillisecondArray,
};
use arrow_schema::{DataType, Schema, TimeUnit};
use chrono::Duration as ChronoDuration;
use futures::TryStreamExt;
use lance::{dataset::ColumnAlteration, datatypes::BlobHandling, BlobArrayBuilder};
use lancedb::{
    query::{ExecutableQuery, QueryBase, Select},
    table::{OptimizeAction, OptimizeOptions},
    Connection, Table,
};
use static_flow_shared::{
    embedding::{embed_image_bytes, embed_text_with_language, TextEmbeddingLanguage},
    image_vector_maintenance::{
        reembed_image_vectors as reembed_image_vectors_in_table, ImageReembedOptions,
        ImageReembedScope,
    },
    optimize::{compact_table_with_fallback, prune_table_versions},
};

use crate::{
    cli::QueryOutputFormat,
    db::{
        connect_db, ensure_fts_index, ensure_scalar_index, ensure_table, ensure_vector_index,
        upsert_articles, upsert_images,
    },
    schema::{article_schema, image_schema, taxonomy_schema, ArticleRecord, ImageRecord},
    utils::rasterize_svg_for_embedding,
};

const CLEANUP_TARGET_TABLES: [&str; 4] = ["articles", "images", "taxonomies", "article_views"];
const DEFAULT_REBUILD_BATCH_SIZE: usize = 256;

#[derive(Debug, Clone, Copy)]
struct TablePolicy {
    scalar_indexes: &'static [&'static str],
    vector_indexes: &'static [&'static str],
    fts_indexes: &'static [&'static str],
    storage_options: &'static [(&'static str, &'static str)],
}

#[derive(Debug)]
struct TableStorageAudit {
    table: String,
    stable_row_ids: bool,
    row_count: usize,
    version: u64,
    fragments: usize,
    indexes: Vec<String>,
}

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

pub async fn audit_storage(db_path: &Path, table: Option<&str>) -> Result<()> {
    let db = connect_db(db_path).await?;
    let table_names = match table {
        Some(name) => vec![name.to_string()],
        None => db.table_names().limit(10_000).execute().await?,
    };

    if table_names.is_empty() {
        tracing::info!("No tables found in {}", db_path.display());
        return Ok(());
    }

    let mut audits = Vec::with_capacity(table_names.len());
    for table_name in table_names {
        let table = open_table(&db, &table_name).await?;
        audits.push(audit_one_table(&table).await?);
    }

    audits.sort_by(|left, right| left.table.cmp(&right.table));
    tracing::info!("Storage audit for `{}`:", db_path.display());
    for audit in audits {
        let indexes =
            if audit.indexes.is_empty() { "none".to_string() } else { audit.indexes.join(", ") };
        tracing::info!(
            "- {} | stable_row_ids={} | rows={} | version={} | fragments={} | indexes={}",
            audit.table,
            audit.stable_row_ids,
            audit.row_count,
            audit.version,
            audit.fragments,
            indexes
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
    let targets = resolve_index_targets(&db, table.as_deref()).await?;
    for target in targets {
        ensure_indexes_for_table(&db, &target).await?;
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

    if all {
        let action = compact_table_with_fallback(&table)
            .await
            .map_err(anyhow::Error::msg)?;
        let _ = table
            .optimize(OptimizeAction::Index(OptimizeOptions::default()))
            .await?;
        tracing::info!("Compaction completed for `{}` via {}", table.name(), action.as_str());
    } else {
        let _ = table
            .optimize(OptimizeAction::Index(OptimizeOptions::default()))
            .await?;
    }

    if prune_now {
        prune_table_versions(&table, 0, true, false)
            .await
            .map_err(anyhow::Error::msg)?;
        tracing::info!(
            "Immediate prune completed for `{}` (older_than=0, delete_unverified=true).",
            table.name()
        );
    }

    if table_policy(table.name()).is_some() {
        ensure_indexes_for_table(&db, table.name()).await?;
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
    let dataset = table
        .dataset()
        .ok_or_else(|| anyhow!("table `images` has no native dataset"))?
        .get()
        .await
        .context("failed to load images dataset for SVG re-embed")?;
    let mut scanner = dataset.scan();
    scanner.project(&["id", "filename", "data", "thumbnail", "metadata", "created_at"])?;
    scanner.filter("filename LIKE '%.svg' OR filename LIKE '%.SVG'")?;
    scanner.blob_handling(BlobHandling::AllBinary);
    let batches = scanner
        .try_into_stream()
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
            let bytes = binary_like_value(batch, "data", row)?.to_vec();
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

            let thumbnail = binary_like_value_opt(batch, "thumbnail", row)?;

            updates.push(ImageRecord {
                id: ids.value(row).to_string(),
                filename,
                data: bytes,
                thumbnail,
                vector: match embed_image_bytes(&rasterized.png_bytes) {
                    Ok(vector) => Some(vector),
                    Err(err) => {
                        tracing::warn!(
                            "Failed to embed rasterized SVG {}; writing NULL vector: {}",
                            ids.value(row),
                            err
                        );
                        None
                    },
                },
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

pub async fn migrate_images_vector_nullable(db_path: &Path, dry_run: bool) -> Result<()> {
    let db = connect_db(db_path).await?;
    let table = open_table(&db, "images").await?;
    let schema = table.schema().await?;
    let field = schema
        .field_with_name("vector")
        .context("`images` table missing required `vector` column")?;

    if !matches!(field.data_type(), DataType::FixedSizeList(_, _)) {
        bail!(
            "`images.vector` has unsupported type `{}` (expected fixed_size_list<float32, {}>)",
            field.data_type(),
            static_flow_shared::embedding::IMAGE_VECTOR_DIM
        );
    }

    if field.is_nullable() {
        tracing::info!("`images.vector` is already nullable; no migration needed.");
        return Ok(());
    }

    let before_version = table.version().await?;
    if dry_run {
        tracing::info!(
            "Dry run: `images.vector` would be migrated to nullable=true (current version={}).",
            before_version
        );
        return Ok(());
    }

    let _ = table
        .alter_columns(&[ColumnAlteration::new("vector".into()).set_nullable(true)])
        .await
        .context("failed to alter `images.vector` nullability")?;

    let after_version = table.version().await?;
    tracing::info!(
        "Migrated `images.vector` to nullable=true (version {} -> {}).",
        before_version,
        after_version
    );
    Ok(())
}

pub async fn reembed_image_vectors(
    db_path: &Path,
    limit: Option<usize>,
    dry_run: bool,
    all: bool,
    batch_size: usize,
) -> Result<()> {
    let db = connect_db(db_path).await?;
    let table = open_table(&db, "images").await?;

    let scope = if all { ImageReembedScope::All } else { ImageReembedScope::MissingOnly };
    let stats = reembed_image_vectors_in_table(&table, ImageReembedOptions {
        scope,
        limit,
        dry_run,
        batch_size,
    })
    .await?;

    if !dry_run && stats.updated_rows > 0 {
        if let Err(err) = ensure_vector_index(&table, "vector").await {
            tracing::warn!("Failed to ensure vector index after image re-embed: {err}");
        }
    }

    tracing::info!(
        "Image vector re-embed completed: scope={:?}, dry_run={}, scanned={}, embedded_ok={}, \
         embedded_failed={}, update_candidates={}, updated={}, skipped_failed={}",
        scope,
        dry_run,
        stats.scanned_rows,
        stats.embedded_rows,
        stats.embedding_failed_rows,
        stats.update_candidates,
        stats.updated_rows,
        stats.skipped_failed_rows
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
        .only_if(filter)
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
                match embed_text_with_language(content, TextEmbeddingLanguage::Chinese) {
                    Ok(vector) => {
                        updates_vector_zh.push((id.clone(), vector));
                        filled_vector_zh += 1;
                    },
                    Err(err) => {
                        tracing::warn!(
                            "Failed to embed article vector_zh for id `{}`; leaving NULL: {}",
                            id,
                            err
                        );
                    },
                }
            }
            if should_fill_vector_en {
                if let Some(content_en) = &content_en {
                    match embed_text_with_language(content_en, TextEmbeddingLanguage::English) {
                        Ok(vector) => {
                            updates_vector_en.push((id.clone(), vector));
                            filled_vector_en += 1;
                        },
                        Err(err) => {
                            tracing::warn!(
                                "Failed to embed article vector_en for id `{}`; leaving NULL: {}",
                                id,
                                err
                            );
                        },
                    }
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

pub async fn restore_table(db_path: &Path, table: &str, version: u64) -> Result<()> {
    let db = connect_db(db_path).await?;
    let table = open_table(&db, table).await?;
    let current = table.version().await?;
    tracing::info!(
        "Table `{}` current version: {}, restoring to version {}",
        table.name(),
        current,
        version
    );
    table.checkout(version).await.context("checkout failed")?;
    table.restore().await.context("restore failed")?;
    let new_ver = table.version().await?;
    tracing::info!("Restored `{}` to version {} (new version: {})", table.name(), version, new_ver);
    Ok(())
}

pub async fn rebuild_article_views_stable(db_path: &Path, force: bool) -> Result<()> {
    rebuild_table_stable(db_path, "article_views", force, DEFAULT_REBUILD_BATCH_SIZE).await
}

pub async fn migrate_images_blob_v2(db_path: &Path, force: bool, batch_size: usize) -> Result<()> {
    rebuild_table_with_target_schema(db_path, "images", image_schema(), force, batch_size).await
}

pub async fn repair_legacy_blob_filenames(
    db_path: &Path,
    table_name: &str,
    dry_run: bool,
) -> Result<()> {
    let db = connect_db(db_path).await?;
    let table = open_table(&db, table_name).await?;
    let schema = table.schema().await?;
    if !schema_requires_blob_v2_storage(schema.as_ref()) {
        bail!("table `{table_name}` does not contain blob v2 columns");
    }
    if !table_uses_blob_v2_storage(&table).await? {
        bail!("table `{table_name}` does not use blob v2 data storage version 2.2");
    }

    let data_dir = db_path.join(format!("{table_name}.lance")).join("data");
    if !data_dir.exists() {
        bail!("table data directory `{}` does not exist", data_dir.display());
    }

    let mut legacy_paths = Vec::new();
    collect_legacy_blob_paths(&data_dir, &mut legacy_paths)?;
    legacy_paths.sort();

    if legacy_paths.is_empty() {
        tracing::info!(
            "No legacy blob filenames found for `{table_name}` under `{}`.",
            data_dir.display()
        );
        return Ok(());
    }

    tracing::info!(
        "Found {} legacy blob sidecar files for `{table_name}` under `{}`.",
        legacy_paths.len(),
        data_dir.display()
    );

    let mut renamed = 0usize;
    for old_path in legacy_paths {
        let stem = old_path
            .file_stem()
            .and_then(|value| value.to_str())
            .ok_or_else(|| anyhow!("invalid blob file name `{}`", old_path.display()))?;
        let blob_id = u32::from_str_radix(stem, 16)
            .with_context(|| format!("failed to parse legacy blob id `{stem}`"))?;
        let new_name = format!("{:032b}.blob", blob_id.reverse_bits());
        let new_path = old_path.with_file_name(new_name);
        if new_path.exists() {
            bail!(
                "refusing to overwrite existing blob sidecar `{}` while repairing `{}`",
                new_path.display(),
                table_name
            );
        }

        tracing::info!("Repair blob sidecar: {} -> {}", old_path.display(), new_path.display());
        if !dry_run {
            fs::rename(&old_path, &new_path).with_context(|| {
                format!(
                    "failed to rename legacy blob sidecar `{}` -> `{}`",
                    old_path.display(),
                    new_path.display()
                )
            })?;
        }
        renamed += 1;
    }

    if dry_run {
        tracing::info!("Dry run complete. Planned {} blob sidecar renames.", renamed);
        return Ok(());
    }

    let mut remaining = Vec::new();
    collect_legacy_blob_paths(&data_dir, &mut remaining)?;
    if !remaining.is_empty() {
        bail!(
            "legacy blob sidecar filenames remain after repair for `{table_name}`: {} files",
            remaining.len()
        );
    }

    tracing::info!("Repaired {} legacy blob sidecar filenames for `{table_name}`.", renamed);
    Ok(())
}

pub async fn rebuild_table_stable(
    db_path: &Path,
    table_name: &str,
    force: bool,
    batch_size: usize,
) -> Result<()> {
    let db = connect_db(db_path).await?;
    let table = open_table(&db, table_name).await?;
    let current_schema = table.schema().await?;
    rebuild_table_with_target_schema(db_path, table_name, current_schema, force, batch_size).await
}

async fn rebuild_table_with_target_schema(
    db_path: &Path,
    table_name: &str,
    target_schema: Arc<Schema>,
    force: bool,
    batch_size: usize,
) -> Result<()> {
    let db = connect_db(db_path).await?;
    let table = open_table(&db, table_name).await?;
    let current_schema = table.schema().await?;
    let already_stable = table_uses_stable_row_ids(&table).await?;
    let schema_matches =
        schema_is_compatible_with_target(current_schema.as_ref(), target_schema.as_ref());
    if already_stable && schema_matches && !force {
        tracing::info!(
            "`{table_name}` already matches the requested stable schema; nothing to do."
        );
        return Ok(());
    }

    let row_count_before = table
        .count_rows(None)
        .await
        .with_context(|| format!("failed to count `{table_name}` rows before rebuild"))?
        as usize;
    let backup_dir = table_backup_path(db_path, table_name)?;
    let tmp_db_path = temp_rebuild_db_path(db_path, table_name)?;
    let original_dir = db_path.join(format!("{table_name}.lance"));

    if !original_dir.exists() {
        bail!("expected table directory at `{}`", original_dir.display());
    }
    if tmp_db_path.exists() {
        fs::remove_dir_all(&tmp_db_path).with_context(|| {
            format!("failed to remove stale rebuild temp dir `{}`", tmp_db_path.display())
        })?;
    }

    tracing::info!(
        "Rebuilding `{table_name}` with target schema. rows={} backup=`{}` tmp_db=`{}` \
         stable_before={} schema_matches_before={}",
        row_count_before,
        backup_dir.display(),
        tmp_db_path.display(),
        already_stable,
        schema_matches
    );

    let rebuild_result = async {
        rebuild_table_into_temp_db(&table, &target_schema, &tmp_db_path, table_name, batch_size)
            .await?;
        let tmp_db = connect_db(&tmp_db_path).await?;
        ensure_indexes_for_table(&tmp_db, table_name).await?;
        let tmp_table = open_table(&tmp_db, table_name).await?;
        let tmp_count = tmp_table
            .count_rows(None)
            .await
            .with_context(|| format!("failed to count rebuilt temp `{table_name}` rows"))?
            as usize;
        if tmp_count != row_count_before {
            bail!(
                "rebuilt temp `{table_name}` row count mismatch: before={} after={}",
                row_count_before,
                tmp_count
            );
        }
        if !table_uses_stable_row_ids(&tmp_table).await? {
            bail!("rebuilt temp `{table_name}` still does not use stable row ids");
        }
        validate_table_layout(&tmp_table, &target_schema, "rebuilt temp").await?;
        Ok::<(), anyhow::Error>(())
    }
    .await;

    if let Err(err) = rebuild_result {
        let _ = fs::remove_dir_all(&tmp_db_path);
        return Err(err);
    }

    fs::create_dir_all(
        backup_dir
            .parent()
            .ok_or_else(|| anyhow!("invalid backup path `{}`", backup_dir.display()))?,
    )
    .with_context(|| format!("failed to create backup parent for `{}`", backup_dir.display()))?;
    fs::rename(&original_dir, &backup_dir).with_context(|| {
        format!("failed to move `{}` to backup `{}`", original_dir.display(), backup_dir.display())
    })?;

    let tmp_table_dir = tmp_db_path.join(format!("{table_name}.lance"));
    let swap_result = fs::rename(&tmp_table_dir, &original_dir).with_context(|| {
        format!(
            "failed to move rebuilt `{}` into place from `{}`",
            table_name,
            tmp_table_dir.display()
        )
    });
    if let Err(err) = swap_result {
        let rollback_err = fs::rename(&backup_dir, &original_dir).with_context(|| {
            format!(
                "rebuild failed and rollback also failed; backup remains at `{}`",
                backup_dir.display()
            )
        });
        let _ = fs::remove_dir_all(&tmp_db_path);
        rollback_err?;
        return Err(err);
    }
    let _ = fs::remove_dir_all(&tmp_db_path);

    let db = connect_db(db_path).await?;
    let rebuilt = open_table(&db, table_name).await?;
    let row_count_after = rebuilt
        .count_rows(None)
        .await
        .with_context(|| format!("failed to count rebuilt `{table_name}` rows"))?
        as usize;
    if row_count_after != row_count_before {
        bail!(
            "rebuilt `{table_name}` row count mismatch after swap: before={} after={}",
            row_count_before,
            row_count_after
        );
    }
    if !table_uses_stable_row_ids(&rebuilt).await? {
        bail!("rebuilt `{table_name}` still does not use stable row ids after swap");
    }
    validate_table_layout(&rebuilt, &target_schema, "rebuilt").await?;

    tracing::info!(
        "Rebuilt `{table_name}` successfully. Backup preserved at `{}`.",
        backup_dir.display()
    );
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

async fn resolve_index_targets(db: &Connection, table: Option<&str>) -> Result<Vec<String>> {
    match table {
        Some(name) => {
            if table_policy(name).is_none() {
                bail!(
                    "unsupported table `{name}` for index management; supported tables: {}",
                    all_policy_table_names().join(", ")
                );
            }
            Ok(vec![name.to_string()])
        },
        None => {
            let existing = db.table_names().limit(10_000).execute().await?;
            let mut targets = existing
                .into_iter()
                .filter(|name| table_policy(name).is_some())
                .collect::<Vec<_>>();
            targets.sort();
            Ok(targets)
        },
    }
}

async fn ensure_indexes_for_table(db: &Connection, table_name: &str) -> Result<()> {
    let Some(policy) = table_policy(table_name) else {
        tracing::info!("No managed index policy for `{table_name}`, skipping.");
        return Ok(());
    };

    let mut table = open_table(db, table_name).await?;
    if drop_duplicate_indexes(&table).await? > 0 {
        table = open_table(db, table_name).await?;
    }
    for column in policy.scalar_indexes {
        if let Err(err) = ensure_scalar_index(&table, column).await {
            tracing::warn!("Failed to create scalar index on `{}` ({column}): {err}", table.name());
        }
    }
    for column in policy.fts_indexes {
        if let Err(err) = ensure_fts_index(&table, column).await {
            tracing::warn!("Failed to create FTS index on `{}` ({column}): {err}", table.name());
        }
    }
    for column in policy.vector_indexes {
        if let Err(err) = ensure_vector_index(&table, column).await {
            tracing::warn!("Failed to create vector index on `{}` ({column}): {err}", table.name());
        }
    }
    Ok(())
}

async fn drop_duplicate_indexes(table: &Table) -> Result<usize> {
    let indexes = table.list_indices().await?;
    let mut seen = HashSet::<(String, String)>::new();
    let mut duplicate_names = BTreeSet::<String>::new();

    for index in indexes {
        let key = (index.index_type.to_string(), index.columns.join(","));
        if !seen.insert(key) {
            duplicate_names.insert(index.name);
        }
    }

    let mut dropped = 0usize;
    for name in duplicate_names {
        table.drop_index(&name).await.with_context(|| {
            format!("failed to drop duplicate index `{name}` on `{}`", table.name())
        })?;
        dropped += 1;
    }
    Ok(dropped)
}

fn table_backup_path(db_path: &Path, table_name: &str) -> Result<PathBuf> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before UNIX_EPOCH")?
        .as_secs();
    let backup_root = db_path.parent().unwrap_or(db_path).join("lancedb-backups");
    Ok(backup_root.join(format!("{table_name}-legacy-{stamp}.lance")))
}

async fn table_uses_stable_row_ids(table: &Table) -> Result<bool> {
    let dataset = table
        .dataset()
        .ok_or_else(|| anyhow!("table `{}` has no native dataset", table.name()))?
        .get()
        .await
        .with_context(|| format!("failed to load dataset for `{}`", table.name()))?;
    Ok(dataset.manifest().uses_stable_row_ids())
}

async fn audit_one_table(table: &Table) -> Result<TableStorageAudit> {
    let dataset = table
        .dataset()
        .ok_or_else(|| anyhow!("table `{}` has no native dataset", table.name()))?
        .get()
        .await
        .with_context(|| format!("failed to load dataset for `{}`", table.name()))?;
    let row_count = table
        .count_rows(None)
        .await
        .with_context(|| format!("failed to count rows for `{}`", table.name()))?
        as usize;
    let version = table.version().await?;
    let fragments = dataset.get_fragments().len();
    let indexes = table
        .list_indices()
        .await?
        .into_iter()
        .map(|index| format!("{}:{}:[{}]", index.name, index.index_type, index.columns.join(",")))
        .collect::<Vec<_>>();
    Ok(TableStorageAudit {
        table: table.name().to_string(),
        stable_row_ids: dataset.manifest().uses_stable_row_ids(),
        row_count,
        version,
        fragments,
        indexes,
    })
}

fn temp_rebuild_db_path(db_path: &Path, table_name: &str) -> Result<PathBuf> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before UNIX_EPOCH")?
        .as_secs();
    let name = db_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| anyhow!("invalid db path `{}`", db_path.display()))?;
    let parent = db_path.parent().unwrap_or(db_path);
    Ok(parent.join(format!("{name}-rebuild-{table_name}-{stamp}")))
}

fn collect_legacy_blob_paths(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir)
        .with_context(|| format!("failed to read blob data directory `{}`", dir.display()))?
    {
        let entry =
            entry.with_context(|| format!("failed to iterate directory `{}`", dir.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to determine file type for `{}`", path.display()))?;
        if file_type.is_dir() {
            collect_legacy_blob_paths(&path, out)?;
            continue;
        }
        if !file_type.is_file() || path.extension().and_then(|ext| ext.to_str()) != Some("blob") {
            continue;
        }

        let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        if stem.len() == 8 && stem.as_bytes().iter().all(u8::is_ascii_hexdigit) {
            out.push(path);
        }
    }
    Ok(())
}

async fn rebuild_table_into_temp_db(
    table: &Table,
    schema: &Arc<Schema>,
    tmp_db_path: &Path,
    table_name: &str,
    batch_size: usize,
) -> Result<()> {
    let ds_wrapper = table
        .dataset()
        .ok_or_else(|| anyhow!("table `{}` has no native dataset", table.name()))?;
    let dataset = ds_wrapper
        .get()
        .await
        .with_context(|| format!("failed to load dataset for `{}`", table.name()))?;
    let total = table.count_rows(None).await? as usize;
    let tmp_db = connect_db(tmp_db_path).await?;
    let mut tmp_table: Option<Table> = None;
    let effective_batch_size = batch_size.max(1);
    let use_all_binary = schema_has_blob_like_field(schema.as_ref());

    if total == 0 {
        let empty = RecordBatch::new_empty(schema.clone());
        let reader = RecordBatchIterator::new(vec![Ok(empty)].into_iter(), schema.clone());
        let mut builder =
            tmp_db.create_table(table_name, Box::new(reader) as Box<dyn RecordBatchReader + Send>);
        for &(key, value) in storage_options_for_table(table_name, schema.as_ref()) {
            builder = builder.storage_option(key, value);
        }
        builder
            .execute()
            .await
            .with_context(|| format!("failed to create empty rebuilt `{table_name}`"))?;
        return Ok(());
    }

    let mut offset = 0usize;
    while offset < total {
        let mut scanner = dataset.scan();
        scanner.limit(Some(effective_batch_size as i64), Some(offset as i64))?;
        if use_all_binary {
            scanner.blob_handling(lance::datatypes::BlobHandling::AllBinary);
        }
        let stream = scanner.try_into_stream().await?;
        let source_batches = stream
            .try_collect::<Vec<_>>()
            .await
            .with_context(|| format!("failed to read `{table_name}` batch at offset={offset}"))?;
        if source_batches.is_empty() {
            break;
        }

        let aligned_batches = source_batches
            .into_iter()
            .map(|batch| align_batch_to_schema(schema.clone(), batch))
            .collect::<Result<Vec<_>>>()?;
        let written_rows: usize = aligned_batches.iter().map(RecordBatch::num_rows).sum();
        let reader = RecordBatchIterator::new(aligned_batches.into_iter().map(Ok), schema.clone());

        match &tmp_table {
            Some(existing) => {
                existing
                    .add(Box::new(reader) as Box<dyn RecordBatchReader + Send>)
                    .execute()
                    .await
                    .with_context(|| {
                        format!("failed to append rebuilt `{table_name}` batch at offset={offset}")
                    })?;
            },
            None => {
                let mut builder = tmp_db.create_table(
                    table_name,
                    Box::new(reader) as Box<dyn RecordBatchReader + Send>,
                );
                for &(key, value) in storage_options_for_table(table_name, schema.as_ref()) {
                    builder = builder.storage_option(key, value);
                }
                let created = builder.execute().await.with_context(|| {
                    format!("failed to create rebuilt temp table for `{table_name}`")
                })?;
                tmp_table = Some(created);
            },
        }

        if written_rows == 0 {
            break;
        }
        offset += written_rows;
    }

    Ok(())
}

fn align_batch_to_schema(schema: Arc<Schema>, batch: RecordBatch) -> Result<RecordBatch> {
    let mut arrays: Vec<ArrayRef> = Vec::with_capacity(schema.fields().len());
    let source_schema = batch.schema();
    for field in schema.fields() {
        let (idx, source_field) =
            source_schema
                .column_with_name(field.name())
                .ok_or_else(|| {
                    anyhow!("missing column `{}` while aligning rebuild batch", field.name())
                })?;
        let source = batch.column(idx).clone();
        let array = if source_field.data_type() == field.data_type() {
            source
        } else if is_blob_v2_field(field.as_ref()) {
            binary_like_array_to_blob_array(source.as_ref(), field.name())?
        } else {
            cast(source.as_ref(), field.data_type()).with_context(|| {
                format!(
                    "failed to cast column `{}` from `{}` to `{}` during rebuild",
                    field.name(),
                    source_field.data_type(),
                    field.data_type()
                )
            })?
        };
        arrays.push(array);
    }
    RecordBatch::try_new(schema, arrays).context("failed to build aligned rebuild batch")
}

async fn validate_table_layout(table: &Table, target_schema: &Schema, label: &str) -> Result<()> {
    let actual_schema = table.schema().await?;
    if !schema_is_compatible_with_target(actual_schema.as_ref(), target_schema) {
        bail!("{label} `{}` schema is not compatible with requested target layout", table.name());
    }
    if schema_requires_blob_v2_storage(target_schema) && !table_uses_blob_v2_storage(table).await? {
        bail!("{label} `{}` does not use blob v2 data storage version 2.2", table.name());
    }
    Ok(())
}

fn schema_is_compatible_with_target(actual: &Schema, target: &Schema) -> bool {
    target.fields().iter().all(|target_field| {
        let Ok(actual_field) = actual.field_with_name(target_field.name()) else {
            return false;
        };
        if actual_field.is_nullable() != target_field.is_nullable() {
            return false;
        }
        if is_blob_v2_field(target_field) {
            return !matches!(actual_field.data_type(), DataType::Binary | DataType::LargeBinary);
        }
        datatype_layout_matches(actual_field.data_type(), target_field.data_type())
    })
}

fn schema_has_blob_like_field(schema: &Schema) -> bool {
    schema.fields().iter().any(|field| {
        matches!(field.data_type(), DataType::LargeBinary) || is_blob_v2_field(field.as_ref())
    })
}

fn schema_requires_blob_v2_storage(schema: &Schema) -> bool {
    schema
        .fields()
        .iter()
        .any(|field| is_blob_v2_field(field.as_ref()))
}

fn datatype_layout_matches(actual: &DataType, target: &DataType) -> bool {
    match (actual, target) {
        (DataType::List(actual_field), DataType::List(target_field))
        | (DataType::LargeList(actual_field), DataType::LargeList(target_field)) => {
            datatype_layout_matches(actual_field.data_type(), target_field.data_type())
        },
        (
            DataType::FixedSizeList(actual_field, actual_size),
            DataType::FixedSizeList(target_field, target_size),
        ) => {
            actual_size == target_size
                && datatype_layout_matches(actual_field.data_type(), target_field.data_type())
        },
        (DataType::Struct(actual_fields), DataType::Struct(target_fields)) => {
            actual_fields.len() == target_fields.len()
                && actual_fields.iter().zip(target_fields.iter()).all(
                    |(actual_field, target_field)| {
                        actual_field.name() == target_field.name()
                            && datatype_layout_matches(
                                actual_field.data_type(),
                                target_field.data_type(),
                            )
                    },
                )
        },
        _ => actual == target,
    }
}

async fn table_uses_blob_v2_storage(table: &Table) -> Result<bool> {
    let dataset = table
        .dataset()
        .ok_or_else(|| anyhow!("table `{}` has no native dataset", table.name()))?
        .get()
        .await
        .with_context(|| format!("failed to load dataset for `{}`", table.name()))?;
    Ok(dataset.manifest().data_storage_format.version == "2.2")
}

fn binary_like_array_to_blob_array(array: &dyn Array, column: &str) -> Result<ArrayRef> {
    let mut builder = BlobArrayBuilder::new(array.len());
    if let Some(binary) = array.as_any().downcast_ref::<BinaryArray>() {
        for row in 0..binary.len() {
            if binary.is_null(row) {
                builder.push_null()?;
            } else {
                builder.push_bytes(binary.value(row))?;
            }
        }
        return builder
            .finish()
            .context("failed to finish blob v2 array conversion");
    }
    if let Some(binary) = array.as_any().downcast_ref::<LargeBinaryArray>() {
        for row in 0..binary.len() {
            if binary.is_null(row) {
                builder.push_null()?;
            } else {
                builder.push_bytes(binary.value(row))?;
            }
        }
        return builder
            .finish()
            .context("failed to finish blob v2 array conversion");
    }
    bail!("failed to convert column `{column}` from `{}` to blob v2 input", array.data_type());
}

fn is_blob_v2_field(field: &arrow_schema::Field) -> bool {
    field
        .metadata()
        .get("ARROW:extension:name")
        .map(|value| value == "lance.blob.v2")
        .unwrap_or(false)
}

fn storage_options_for_table(
    table_name: &str,
    _schema: &Schema,
) -> &'static [(&'static str, &'static str)] {
    table_policy(table_name)
        .map(|policy| policy.storage_options)
        .unwrap_or(DEFAULT_STORAGE_OPTIONS)
}

fn table_policy(table_name: &str) -> Option<TablePolicy> {
    match table_name {
        "articles" => Some(TablePolicy {
            scalar_indexes: &["id", "category"],
            vector_indexes: &["vector_en", "vector_zh"],
            fts_indexes: &["content"],
            storage_options: DEFAULT_STORAGE_OPTIONS,
        }),
        "images" => Some(TablePolicy {
            scalar_indexes: &["id", "filename"],
            vector_indexes: &["vector"],
            fts_indexes: &[],
            storage_options: BLOB_V2_STORAGE_OPTIONS,
        }),
        "taxonomies" => Some(TablePolicy {
            scalar_indexes: &["id", "kind", "key"],
            vector_indexes: &[],
            fts_indexes: &[],
            storage_options: DEFAULT_STORAGE_OPTIONS,
        }),
        "article_views" => Some(TablePolicy {
            scalar_indexes: &["id", "article_id", "day_bucket"],
            vector_indexes: &[],
            fts_indexes: &[],
            storage_options: DEFAULT_STORAGE_OPTIONS,
        }),
        "api_behavior_events" => Some(TablePolicy {
            scalar_indexes: &["event_id", "occurred_at", "method", "status_code", "device_type"],
            vector_indexes: &[],
            fts_indexes: &[],
            storage_options: DEFAULT_STORAGE_OPTIONS,
        }),
        "article_requests" => Some(TablePolicy {
            scalar_indexes: &["request_id", "status", "parent_request_id"],
            vector_indexes: &[],
            fts_indexes: &[],
            storage_options: DEFAULT_STORAGE_OPTIONS,
        }),
        "article_request_ai_runs" => Some(TablePolicy {
            scalar_indexes: &["run_id", "request_id"],
            vector_indexes: &[],
            fts_indexes: &[],
            storage_options: DEFAULT_STORAGE_OPTIONS,
        }),
        "article_request_ai_run_chunks" => Some(TablePolicy {
            scalar_indexes: &["chunk_id", "run_id", "request_id"],
            vector_indexes: &[],
            fts_indexes: &[],
            storage_options: DEFAULT_STORAGE_OPTIONS,
        }),
        "interactive_pages" => Some(TablePolicy {
            scalar_indexes: &["id", "article_id", "source_url"],
            vector_indexes: &[],
            fts_indexes: &[],
            storage_options: DEFAULT_STORAGE_OPTIONS,
        }),
        "interactive_page_locales" => Some(TablePolicy {
            scalar_indexes: &["id", "page_id", "locale"],
            vector_indexes: &[],
            fts_indexes: &[],
            storage_options: DEFAULT_STORAGE_OPTIONS,
        }),
        "interactive_assets" => Some(TablePolicy {
            scalar_indexes: &["id", "page_id", "logical_path"],
            vector_indexes: &[],
            fts_indexes: &[],
            storage_options: BLOB_V2_STORAGE_OPTIONS,
        }),
        "comment_tasks" => Some(TablePolicy {
            scalar_indexes: &["task_id", "article_id", "status"],
            vector_indexes: &[],
            fts_indexes: &[],
            storage_options: DEFAULT_STORAGE_OPTIONS,
        }),
        "comment_published" => Some(TablePolicy {
            scalar_indexes: &["comment_id", "task_id", "article_id"],
            vector_indexes: &[],
            fts_indexes: &[],
            storage_options: DEFAULT_STORAGE_OPTIONS,
        }),
        "comment_audit_logs" => Some(TablePolicy {
            scalar_indexes: &["log_id", "task_id", "action"],
            vector_indexes: &[],
            fts_indexes: &[],
            storage_options: DEFAULT_STORAGE_OPTIONS,
        }),
        "comment_ai_runs" => Some(TablePolicy {
            scalar_indexes: &["run_id", "task_id", "status"],
            vector_indexes: &[],
            fts_indexes: &[],
            storage_options: DEFAULT_STORAGE_OPTIONS,
        }),
        "comment_ai_run_chunks" => Some(TablePolicy {
            scalar_indexes: &["chunk_id", "run_id", "task_id"],
            vector_indexes: &[],
            fts_indexes: &[],
            storage_options: DEFAULT_STORAGE_OPTIONS,
        }),
        "songs" => Some(TablePolicy {
            scalar_indexes: &["id", "artist", "album"],
            vector_indexes: &["vector_en", "vector_zh"],
            fts_indexes: &["searchable_text"],
            storage_options: BLOB_V2_STORAGE_OPTIONS,
        }),
        "music_plays" => Some(TablePolicy {
            scalar_indexes: &["id", "song_id", "day_bucket"],
            vector_indexes: &[],
            fts_indexes: &[],
            storage_options: DEFAULT_STORAGE_OPTIONS,
        }),
        "music_comments" => Some(TablePolicy {
            scalar_indexes: &["id", "song_id"],
            vector_indexes: &[],
            fts_indexes: &[],
            storage_options: DEFAULT_STORAGE_OPTIONS,
        }),
        "music_wishes" => Some(TablePolicy {
            scalar_indexes: &["wish_id", "status"],
            vector_indexes: &[],
            fts_indexes: &[],
            storage_options: DEFAULT_STORAGE_OPTIONS,
        }),
        "music_wish_ai_runs" => Some(TablePolicy {
            scalar_indexes: &["run_id", "wish_id"],
            vector_indexes: &[],
            fts_indexes: &[],
            storage_options: DEFAULT_STORAGE_OPTIONS,
        }),
        "music_wish_ai_run_chunks" => Some(TablePolicy {
            scalar_indexes: &["chunk_id", "run_id", "wish_id"],
            vector_indexes: &[],
            fts_indexes: &[],
            storage_options: DEFAULT_STORAGE_OPTIONS,
        }),
        _ => None,
    }
}

fn all_policy_table_names() -> Vec<&'static str> {
    vec![
        "api_behavior_events",
        "article_request_ai_run_chunks",
        "article_request_ai_runs",
        "article_requests",
        "article_views",
        "articles",
        "comment_ai_run_chunks",
        "comment_ai_runs",
        "comment_audit_logs",
        "comment_published",
        "comment_tasks",
        "images",
        "interactive_assets",
        "interactive_page_locales",
        "interactive_pages",
        "music_comments",
        "music_plays",
        "music_wish_ai_run_chunks",
        "music_wish_ai_runs",
        "music_wishes",
        "songs",
        "taxonomies",
    ]
}

const DEFAULT_STORAGE_OPTIONS: &[(&str, &str)] =
    &[("new_table_enable_stable_row_ids", "true"), ("new_table_enable_v2_manifest_paths", "true")];

const BLOB_V2_STORAGE_OPTIONS: &[(&str, &str)] = &[
    ("new_table_data_storage_version", "2.2"),
    ("new_table_enable_stable_row_ids", "true"),
    ("new_table_enable_v2_manifest_paths", "true"),
];

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

fn binary_like_value<'a>(batch: &'a RecordBatch, column: &str, row: usize) -> Result<&'a [u8]> {
    let index = batch
        .schema()
        .index_of(column)
        .with_context(|| format!("missing column `{column}`"))?;
    let array = batch.column(index);
    if let Some(binary) = array.as_any().downcast_ref::<BinaryArray>() {
        return Ok(binary.value(row));
    }
    if let Some(binary) = array.as_any().downcast_ref::<LargeBinaryArray>() {
        return Ok(binary.value(row));
    }
    Err(anyhow!("column `{column}` is not BinaryArray/LargeBinaryArray"))
}

fn binary_like_value_opt(batch: &RecordBatch, column: &str, row: usize) -> Result<Option<Vec<u8>>> {
    let index = batch
        .schema()
        .index_of(column)
        .with_context(|| format!("missing column `{column}`"))?;
    let array = batch.column(index);
    if let Some(binary) = array.as_any().downcast_ref::<BinaryArray>() {
        return Ok((!binary.is_null(row)).then(|| binary.value(row).to_vec()));
    }
    if let Some(binary) = array.as_any().downcast_ref::<LargeBinaryArray>() {
        return Ok((!binary.is_null(row)).then(|| binary.value(row).to_vec()));
    }
    Err(anyhow!("column `{column}` is not BinaryArray/LargeBinaryArray"))
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

// ---------------------------------------------------------------------------
// Blob V2 Compaction E2E Test
// ---------------------------------------------------------------------------

pub async fn test_blob_compact(db_path: &Path, count: usize, blob_size: usize) -> Result<()> {
    use std::time::Instant;

    use static_flow_shared::music_store::{MusicDataStore, SongRecord};

    if count == 0 {
        bail!("count must be at least 1");
    }

    let test_dir = db_path.join("_test_blob_compact");
    if test_dir.exists() {
        std::fs::remove_dir_all(&test_dir)?;
    }
    std::fs::create_dir_all(&test_dir)?;
    let db_uri = test_dir.to_string_lossy().to_string();

    tracing::info!(
        "=== Blob V2 Compaction E2E Test ===\n  songs: {count}\n  blob_size: {} bytes\n  test_db: \
         {db_uri}",
        blob_size
    );

    let store = MusicDataStore::connect(&db_uri).await?;
    let now_ms = chrono::Utc::now().timestamp_millis();

    // Step 1: insert N songs (each as separate fragment)
    let start = Instant::now();
    for i in 0..count {
        let record = SongRecord {
            id: format!("test-song-{i:04}"),
            title: format!("Test Song {i}"),
            artist: "CompactionTest".into(),
            album: "BlobV2Test".into(),
            album_id: None,
            cover_image: None,
            duration_ms: 180_000,
            format: "mp3".into(),
            bitrate: 320,
            lyrics_lrc: None,
            lyrics_translation: None,
            audio_data: vec![(i as u8).wrapping_add(42); blob_size],
            source: "test".into(),
            source_id: None,
            tags: "test,compaction".into(),
            searchable_text: format!("Test Song {i} CompactionTest"),
            vector_en: None,
            vector_zh: None,
            created_at: now_ms,
            updated_at: now_ms,
        };
        store.upsert_song(&record).await?;
    }
    let insert_ms = start.elapsed().as_millis();
    tracing::info!("Inserted {count} songs in {insert_ms}ms");

    // Step 2: count fragments before compaction
    let table = store.connection().open_table("songs").execute().await?;
    let ds_before = table.dataset().context("no dataset")?;
    let frags_before = ds_before.get().await?.get_fragments().len();
    tracing::info!("Fragments before compact: {frags_before}");

    // Step 3: compact
    let compact_start = Instant::now();
    table.optimize(OptimizeAction::All).await?;
    let compact_ms = compact_start.elapsed().as_millis();
    tracing::info!("Compact completed in {compact_ms}ms");

    // Step 4: fragments after
    let ds_after = table.dataset().context("no dataset")?;
    let frags_after = ds_after.get().await?.get_fragments().len();
    tracing::info!("Fragments after compact: {frags_after}");

    // Step 5: prune
    table
        .optimize(OptimizeAction::Prune {
            older_than: Some(ChronoDuration::zero()),
            delete_unverified: Some(true),
            error_if_tagged_old_versions: Some(false),
        })
        .await?;
    tracing::info!("Prune completed");

    // Step 6: verify audio data integrity
    let mut pass = 0usize;
    let mut fail = 0usize;
    for i in 0..count {
        let id = format!("test-song-{i:04}");
        match store.get_song_audio(&id).await {
            Ok(Some((data, fmt))) => {
                let expected_byte = (i as u8).wrapping_add(42);
                let size_ok = data.len() == blob_size;
                let content_ok = data[0] == expected_byte && data[data.len() - 1] == expected_byte;
                let fmt_ok = fmt == "mp3";
                if size_ok && content_ok && fmt_ok {
                    pass += 1;
                } else {
                    fail += 1;
                    tracing::error!(
                        "FAIL {id}: size={}/{blob_size} first_byte={}/{expected_byte} format={fmt}",
                        data.len(),
                        data[0]
                    );
                }
            },
            Ok(None) => {
                fail += 1;
                tracing::error!("FAIL {id}: audio not found");
            },
            Err(err) => {
                fail += 1;
                tracing::error!("FAIL {id}: {err}");
            },
        }
    }

    // Step 7: verify metadata
    for i in 0..count {
        let id = format!("test-song-{i:04}");
        match store.get_song(&id).await {
            Ok(Some(detail)) => {
                if detail.title != format!("Test Song {i}") || detail.artist != "CompactionTest" {
                    fail += 1;
                    tracing::error!(
                        "FAIL {id}: metadata mismatch title={} artist={}",
                        detail.title,
                        detail.artist
                    );
                }
            },
            Ok(None) => {
                fail += 1;
                tracing::error!("FAIL {id}: metadata not found");
            },
            Err(err) => {
                fail += 1;
                tracing::error!("FAIL {id}: metadata error {err}");
            },
        }
    }

    // Cleanup
    if let Err(err) = std::fs::remove_dir_all(&test_dir) {
        tracing::warn!("Failed to cleanup test dir: {err}");
    }

    let total_ms = start.elapsed().as_millis();
    tracing::info!(
        "\n=== Result ===\n  pass: {pass}/{count}\n  fail: {fail}\n  fragments: {frags_before} -> \
         {frags_after}\n  total: {total_ms}ms"
    );

    if fail > 0 {
        bail!("{fail} verification(s) failed");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Verify Audio Data Retrieval
// ---------------------------------------------------------------------------

pub async fn verify_audio(db_path: &Path, ids: Option<String>, limit: Option<usize>) -> Result<()> {
    use static_flow_shared::music_store::MusicDataStore;

    let db_uri = db_path.to_string_lossy().to_string();
    let store = MusicDataStore::connect(&db_uri).await?;

    let target_ids: Vec<String> = match ids {
        Some(ref csv) => csv.split(',').map(|s| s.trim().to_string()).collect(),
        None => {
            // Query all song IDs from the table
            let db = connect_db(db_path).await?;
            let table = open_table(&db, "songs").await?;
            let mut query = table.query();
            if let Some(lim) = limit {
                query = query.limit(lim);
            }
            let stream = query
                .select(Select::columns(&["id"]))
                .execute()
                .await
                .map_err(|err| anyhow!("failed to query songs: {err}"))?;
            let batches = stream.try_collect::<Vec<_>>().await?;
            let mut all_ids = Vec::new();
            for batch in &batches {
                let id_col = downcast_string(batch, "id")?;
                for row in 0..batch.num_rows() {
                    all_ids.push(id_col.value(row).to_string());
                }
            }
            all_ids
        },
    };

    if target_ids.is_empty() {
        tracing::info!("No songs to verify.");
        return Ok(());
    }

    tracing::info!("Verifying audio for {} song(s)...", target_ids.len());

    let mut ok = 0usize;
    let mut err_count = 0usize;
    for id in &target_ids {
        match store.get_song_audio(id).await {
            Ok(Some((data, fmt))) => {
                tracing::info!("  ✓ {id}: {} bytes, format={fmt}", data.len());
                ok += 1;
            },
            Ok(None) => {
                tracing::error!("  ✗ {id}: audio not found");
                err_count += 1;
            },
            Err(err) => {
                tracing::error!("  ✗ {id}: {err}");
                err_count += 1;
            },
        }
    }

    tracing::info!("{ok}/{} songs verified OK", target_ids.len());
    if err_count > 0 {
        bail!("{err_count} song(s) failed audio verification");
    }
    Ok(())
}
