use std::sync::Arc;

use anyhow::{Context, Result};
use arrow_array::{RecordBatch, RecordBatchIterator, RecordBatchReader};
use arrow_schema::{DataType, Field, Schema, TimeUnit};
use lancedb::{
    index::{scalar::BTreeIndexBuilder, Index},
    table::NewColumnTransform,
    Connection, Table,
};

use super::types::{
    LLM_GATEWAY_KEYS_TABLE, LLM_GATEWAY_RUNTIME_CONFIG_TABLE, LLM_GATEWAY_TOKEN_REQUESTS_TABLE,
    LLM_GATEWAY_USAGE_EVENTS_TABLE,
};

pub fn llm_gateway_keys_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("secret", DataType::Utf8, false),
        Field::new("key_hash", DataType::Utf8, false),
        Field::new("status", DataType::Utf8, false),
        Field::new("public_visible", DataType::Boolean, false),
        Field::new("quota_billable_limit", DataType::UInt64, false),
        Field::new("usage_input_uncached_tokens", DataType::UInt64, false),
        Field::new("usage_input_cached_tokens", DataType::UInt64, false),
        Field::new("usage_output_tokens", DataType::UInt64, false),
        Field::new("usage_billable_tokens", DataType::UInt64, false),
        Field::new("last_used_at", DataType::Timestamp(TimeUnit::Millisecond, None), true),
        Field::new("created_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
        Field::new("updated_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
        Field::new("route_strategy", DataType::Utf8, true),
        Field::new("fixed_account_name", DataType::Utf8, true),
    ]))
}

pub fn llm_gateway_usage_events_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("key_id", DataType::Utf8, false),
        Field::new("key_name", DataType::Utf8, true),
        Field::new("account_name", DataType::Utf8, true),
        Field::new("request_method", DataType::Utf8, true),
        Field::new("request_url", DataType::Utf8, true),
        Field::new("latency_ms", DataType::Int32, true),
        Field::new("endpoint", DataType::Utf8, false),
        Field::new("model", DataType::Utf8, true),
        Field::new("status_code", DataType::Int32, false),
        Field::new("input_uncached_tokens", DataType::UInt64, false),
        Field::new("input_cached_tokens", DataType::UInt64, false),
        Field::new("output_tokens", DataType::UInt64, false),
        Field::new("billable_tokens", DataType::UInt64, false),
        Field::new("usage_missing", DataType::Boolean, false),
        Field::new("client_ip", DataType::Utf8, true),
        Field::new("ip_region", DataType::Utf8, true),
        Field::new("request_headers_json", DataType::Utf8, true),
        Field::new("last_message_content", DataType::Utf8, true),
        Field::new("created_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
    ]))
}

pub fn llm_gateway_runtime_config_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("auth_cache_ttl_seconds", DataType::UInt64, false),
        Field::new("updated_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
    ]))
}

pub fn llm_gateway_token_requests_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("request_id", DataType::Utf8, false),
        Field::new("requester_email", DataType::Utf8, false),
        Field::new("requested_quota_billable_limit", DataType::UInt64, false),
        Field::new("request_reason", DataType::Utf8, false),
        Field::new("frontend_page_url", DataType::Utf8, true),
        Field::new("status", DataType::Utf8, false),
        Field::new("fingerprint", DataType::Utf8, false),
        Field::new("client_ip", DataType::Utf8, false),
        Field::new("ip_region", DataType::Utf8, false),
        Field::new("admin_note", DataType::Utf8, true),
        Field::new("failure_reason", DataType::Utf8, true),
        Field::new("issued_key_id", DataType::Utf8, true),
        Field::new("issued_key_name", DataType::Utf8, true),
        Field::new("created_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
        Field::new("updated_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
        Field::new("processed_at", DataType::Timestamp(TimeUnit::Millisecond, None), true),
    ]))
}

pub async fn ensure_keys_table(db: &Connection) -> Result<Table> {
    let table = ensure_table(db, LLM_GATEWAY_KEYS_TABLE, llm_gateway_keys_schema(), &[
        ("new_table_enable_stable_row_ids", "true"),
        ("new_table_enable_v2_manifest_paths", "true"),
    ])
    .await?;
    let schema = table.schema().await?;
    if schema.field_with_name("usage_billable_tokens").is_err() {
        tracing::info!(
            table = %table.name(),
            "Adding missing usage_billable_tokens column to llm gateway keys table"
        );
        table
            .add_columns(
                NewColumnTransform::AllNulls(Arc::new(Schema::new(vec![Field::new(
                    "usage_billable_tokens",
                    DataType::UInt64,
                    true,
                )]))),
                None,
            )
            .await
            .context("failed to add usage_billable_tokens to llm_gateway_keys")?;
    }
    ensure_scalar_index(&table, "id").await?;
    ensure_scalar_index(&table, "key_hash").await?;
    ensure_scalar_index(&table, "status").await?;
    ensure_scalar_index(&table, "public_visible").await?;
    ensure_nullable_utf8_column(&table, "route_strategy").await?;
    ensure_nullable_utf8_column(&table, "fixed_account_name").await?;
    Ok(table)
}

pub async fn ensure_usage_events_table(db: &Connection) -> Result<Table> {
    let table =
        ensure_table(db, LLM_GATEWAY_USAGE_EVENTS_TABLE, llm_gateway_usage_events_schema(), &[
            ("new_table_enable_stable_row_ids", "true"),
            ("new_table_enable_v2_manifest_paths", "true"),
        ])
        .await?;
    ensure_nullable_utf8_column(&table, "key_name").await?;
    ensure_nullable_utf8_column(&table, "request_method").await?;
    ensure_nullable_utf8_column(&table, "request_url").await?;
    ensure_nullable_i32_column(&table, "latency_ms").await?;
    ensure_nullable_utf8_column(&table, "client_ip").await?;
    ensure_nullable_utf8_column(&table, "ip_region").await?;
    ensure_nullable_utf8_column(&table, "request_headers_json").await?;
    ensure_nullable_utf8_column(&table, "account_name").await?;
    ensure_nullable_utf8_column(&table, "last_message_content").await?;
    ensure_scalar_index(&table, "id").await?;
    ensure_scalar_index(&table, "key_id").await?;
    ensure_scalar_index(&table, "created_at").await?;
    Ok(table)
}

pub async fn ensure_runtime_config_table(db: &Connection) -> Result<Table> {
    let table =
        ensure_table(db, LLM_GATEWAY_RUNTIME_CONFIG_TABLE, llm_gateway_runtime_config_schema(), &[
            ("new_table_enable_stable_row_ids", "true"),
            ("new_table_enable_v2_manifest_paths", "true"),
        ])
        .await?;
    ensure_scalar_index(&table, "id").await?;
    Ok(table)
}

pub async fn ensure_token_requests_table(db: &Connection) -> Result<Table> {
    let table =
        ensure_table(db, LLM_GATEWAY_TOKEN_REQUESTS_TABLE, llm_gateway_token_requests_schema(), &[
            ("new_table_enable_stable_row_ids", "true"),
            ("new_table_enable_v2_manifest_paths", "true"),
        ])
        .await?;
    ensure_nullable_utf8_column(&table, "frontend_page_url").await?;
    ensure_nullable_utf8_column(&table, "admin_note").await?;
    ensure_nullable_utf8_column(&table, "failure_reason").await?;
    ensure_nullable_utf8_column(&table, "issued_key_id").await?;
    ensure_nullable_utf8_column(&table, "issued_key_name").await?;
    ensure_nullable_ts_column(&table, "processed_at").await?;
    ensure_scalar_index(&table, "request_id").await?;
    ensure_scalar_index(&table, "requester_email").await?;
    ensure_scalar_index(&table, "status").await?;
    ensure_scalar_index(&table, "created_at").await?;
    Ok(table)
}

async fn ensure_table(
    db: &Connection,
    table_name: &str,
    schema: Arc<Schema>,
    storage_options: &[(&str, &str)],
) -> Result<Table> {
    match db.open_table(table_name).execute().await {
        Ok(table) => Ok(table),
        Err(_) => {
            let batch = RecordBatch::new_empty(schema.clone());
            let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema.clone());
            let mut builder =
                db.create_table(table_name, Box::new(batches) as Box<dyn RecordBatchReader + Send>);
            for &(key, value) in storage_options {
                builder = builder.storage_option(key, value);
            }
            builder
                .execute()
                .await
                .with_context(|| format!("failed to create table `{table_name}`"))?;
            db.open_table(table_name)
                .execute()
                .await
                .with_context(|| format!("failed to open table `{table_name}`"))
        },
    }
}

async fn ensure_scalar_index(table: &Table, column: &str) -> Result<()> {
    let indexes = table.list_indices().await.unwrap_or_default();
    if indexes.iter().any(|idx| idx.columns == [column]) {
        return Ok(());
    }
    tracing::info!(table = %table.name(), column, "Creating scalar index for LLM gateway table");
    table
        .create_index(&[column], Index::BTree(BTreeIndexBuilder::default()))
        .execute()
        .await
        .with_context(|| format!("failed to create scalar index `{column}` on `{}`", table.name()))
}

/// Adds a nullable UTF-8 column to an existing table without rewriting old
/// rows.
async fn ensure_nullable_utf8_column(table: &Table, column: &str) -> Result<()> {
    let schema = table.schema().await?;
    if schema.field_with_name(column).is_ok() {
        return Ok(());
    }
    tracing::info!(table = %table.name(), column, "Adding nullable UTF-8 column to LLM gateway table");
    table
        .add_columns(
            NewColumnTransform::AllNulls(Arc::new(Schema::new(vec![Field::new(
                column,
                DataType::Utf8,
                true,
            )]))),
            None,
        )
        .await
        .with_context(|| format!("failed to add `{column}` to `{}`", table.name()))?;
    Ok(())
}

/// Adds a nullable Int32 column to an existing table without rewriting old
/// rows.
async fn ensure_nullable_i32_column(table: &Table, column: &str) -> Result<()> {
    let schema = table.schema().await?;
    if schema.field_with_name(column).is_ok() {
        return Ok(());
    }
    tracing::info!(table = %table.name(), column, "Adding nullable Int32 column to LLM gateway table");
    table
        .add_columns(
            NewColumnTransform::AllNulls(Arc::new(Schema::new(vec![Field::new(
                column,
                DataType::Int32,
                true,
            )]))),
            None,
        )
        .await
        .with_context(|| format!("failed to add `{column}` to `{}`", table.name()))?;
    Ok(())
}

/// Adds a nullable timestamp column to an existing table without rewriting old
/// rows.
async fn ensure_nullable_ts_column(table: &Table, column: &str) -> Result<()> {
    let schema = table.schema().await?;
    if schema.field_with_name(column).is_ok() {
        return Ok(());
    }
    tracing::info!(table = %table.name(), column, "Adding nullable timestamp column to LLM gateway table");
    table
        .add_columns(
            NewColumnTransform::AllNulls(Arc::new(Schema::new(vec![Field::new(
                column,
                DataType::Timestamp(TimeUnit::Millisecond, None),
                true,
            )]))),
            None,
        )
        .await
        .with_context(|| format!("failed to add `{column}` to `{}`", table.name()))?;
    Ok(())
}

pub fn key_columns() -> [&'static str; 16] {
    [
        "id",
        "name",
        "secret",
        "key_hash",
        "status",
        "public_visible",
        "quota_billable_limit",
        "usage_input_uncached_tokens",
        "usage_input_cached_tokens",
        "usage_output_tokens",
        "usage_billable_tokens",
        "last_used_at",
        "created_at",
        "updated_at",
        "route_strategy",
        "fixed_account_name",
    ]
}

pub fn usage_event_columns() -> [&'static str; 20] {
    [
        "id",
        "key_id",
        "key_name",
        "account_name",
        "request_method",
        "request_url",
        "latency_ms",
        "endpoint",
        "model",
        "status_code",
        "input_uncached_tokens",
        "input_cached_tokens",
        "output_tokens",
        "billable_tokens",
        "usage_missing",
        "client_ip",
        "ip_region",
        "request_headers_json",
        "last_message_content",
        "created_at",
    ]
}

pub fn token_request_columns() -> [&'static str; 16] {
    [
        "request_id",
        "requester_email",
        "requested_quota_billable_limit",
        "request_reason",
        "frontend_page_url",
        "status",
        "fingerprint",
        "client_ip",
        "ip_region",
        "admin_note",
        "failure_reason",
        "issued_key_id",
        "issued_key_name",
        "created_at",
        "updated_at",
        "processed_at",
    ]
}

pub fn escape_literal(value: &str) -> String {
    value.replace('\'', "''")
}
