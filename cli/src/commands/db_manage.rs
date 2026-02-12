use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use arrow::util::pretty::pretty_format_batches;
use arrow_array::{Array, BinaryArray, RecordBatch, StringArray, TimestampMillisecondArray};
use arrow_schema::{DataType, TimeUnit};
use chrono::Duration as ChronoDuration;
use futures::TryStreamExt;
use lancedb::{
    query::{ExecutableQuery, QueryBase, Select},
    table::{OptimizeAction, OptimizeOptions},
    Connection, Table,
};
use static_flow_shared::embedding::embed_image_bytes;

use crate::{
    cli::QueryOutputFormat,
    db::{
        connect_db, ensure_fts_index, ensure_table, ensure_vector_index, upsert_articles,
        upsert_images,
    },
    schema::{article_schema, image_schema, taxonomy_schema, ArticleRecord, ImageRecord},
    utils::rasterize_svg_for_embedding,
};

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
