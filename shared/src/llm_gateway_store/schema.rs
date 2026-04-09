//! Arrow/LanceDB schema definitions and table bootstrap helpers for the LLM
//! gateway store.
//!
//! The functions here define the canonical table layouts and the incremental
//! migration logic needed to evolve existing production tables without manual
//! intervention.

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
    LLM_GATEWAY_ACCOUNT_CONTRIBUTION_REQUESTS_TABLE, LLM_GATEWAY_KEYS_TABLE,
    LLM_GATEWAY_PROXY_BINDINGS_TABLE, LLM_GATEWAY_PROXY_CONFIGS_TABLE,
    LLM_GATEWAY_RUNTIME_CONFIG_TABLE, LLM_GATEWAY_SPONSOR_REQUESTS_TABLE,
    LLM_GATEWAY_TOKEN_REQUESTS_TABLE, LLM_GATEWAY_USAGE_EVENTS_TABLE,
};

/// Canonical schema for the `llm_gateway_keys` table.
pub fn llm_gateway_keys_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("secret", DataType::Utf8, false),
        Field::new("key_hash", DataType::Utf8, false),
        Field::new("status", DataType::Utf8, false),
        // Upstream LLM provider identifier (e.g. "codex", "kiro"). Legacy
        // nullable rows are rewritten to this canonical non-null form during
        // startup migration.
        Field::new("provider_type", DataType::Utf8, false),
        // Wire protocol family this key speaks (e.g. "openai",
        // "anthropic"). Legacy nullable rows are rewritten to this
        // canonical non-null form during startup migration.
        Field::new("protocol_family", DataType::Utf8, false),
        Field::new("public_visible", DataType::Boolean, false),
        Field::new("quota_billable_limit", DataType::UInt64, false),
        Field::new("usage_input_uncached_tokens", DataType::UInt64, false),
        Field::new("usage_input_cached_tokens", DataType::UInt64, false),
        Field::new("usage_output_tokens", DataType::UInt64, false),
        Field::new("usage_billable_tokens", DataType::UInt64, false),
        Field::new("usage_credit_total", DataType::Float64, false),
        Field::new("usage_credit_missing_events", DataType::UInt64, false),
        Field::new("last_used_at", DataType::Timestamp(TimeUnit::Millisecond, None), true),
        Field::new("created_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
        Field::new("updated_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
        Field::new("route_strategy", DataType::Utf8, true),
        Field::new("fixed_account_name", DataType::Utf8, true),
        Field::new("auto_account_names_json", DataType::Utf8, true),
        Field::new("model_name_map_json", DataType::Utf8, true),
        Field::new("request_max_concurrency", DataType::UInt64, true),
        Field::new("request_min_start_interval_ms", DataType::UInt64, true),
        Field::new("kiro_request_validation_enabled", DataType::Boolean, true),
        Field::new("kiro_cache_estimation_enabled", DataType::Boolean, true),
    ]))
}

/// Canonical schema for the `llm_gateway_usage_events` table.
pub fn llm_gateway_usage_events_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("key_id", DataType::Utf8, false),
        Field::new("key_name", DataType::Utf8, true),
        // Upstream LLM provider for this event (e.g. "anthropic", "openai").
        // Nullable for events recorded before multi-provider routing.
        Field::new("provider_type", DataType::Utf8, true),
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
        Field::new("credit_usage", DataType::Float64, true),
        Field::new("credit_usage_missing", DataType::Boolean, false),
        Field::new("client_ip", DataType::Utf8, true),
        Field::new("ip_region", DataType::Utf8, true),
        Field::new("request_headers_json", DataType::Utf8, true),
        Field::new("last_message_content", DataType::Utf8, true),
        Field::new("client_request_body_json", DataType::Utf8, true),
        Field::new("upstream_request_body_json", DataType::Utf8, true),
        Field::new("full_request_json", DataType::Utf8, true),
        Field::new("created_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
    ]))
}

/// Canonical schema for the singleton runtime-config table.
pub fn llm_gateway_runtime_config_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("auth_cache_ttl_seconds", DataType::UInt64, false),
        // Upper bound on proxied request body size in bytes. Guards against
        // oversized payloads exhausting backend memory.
        Field::new("max_request_body_bytes", DataType::UInt64, false),
        // Number of consecutive Codex refresh failures tolerated before
        // marking one account unavailable.
        Field::new("account_failure_retry_limit", DataType::UInt64, false),
        // Maximum concurrent Kiro upstream requests allowed.
        Field::new("kiro_channel_max_concurrency", DataType::UInt64, false),
        // Minimum milliseconds between consecutive Kiro upstream request starts.
        Field::new("kiro_channel_min_start_interval_ms", DataType::UInt64, false),
        Field::new("codex_status_refresh_min_interval_seconds", DataType::UInt64, false),
        Field::new("codex_status_refresh_max_interval_seconds", DataType::UInt64, false),
        Field::new("codex_status_account_jitter_max_seconds", DataType::UInt64, false),
        Field::new("kiro_status_refresh_min_interval_seconds", DataType::UInt64, false),
        Field::new("kiro_status_refresh_max_interval_seconds", DataType::UInt64, false),
        Field::new("kiro_status_account_jitter_max_seconds", DataType::UInt64, false),
        Field::new("usage_event_flush_batch_size", DataType::UInt64, false),
        Field::new("usage_event_flush_interval_seconds", DataType::UInt64, false),
        Field::new("usage_event_flush_max_buffer_bytes", DataType::UInt64, false),
        Field::new("kiro_cache_kmodels_json", DataType::Utf8, false),
        Field::new("kiro_prefix_cache_mode", DataType::Utf8, false),
        Field::new("kiro_prefix_cache_max_tokens", DataType::UInt64, false),
        Field::new("kiro_prefix_cache_entry_ttl_seconds", DataType::UInt64, false),
        Field::new("kiro_conversation_anchor_max_entries", DataType::UInt64, false),
        Field::new("kiro_conversation_anchor_ttl_seconds", DataType::UInt64, false),
        Field::new("updated_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
    ]))
}

/// Canonical schema for persisted upstream proxy configs.
pub fn llm_gateway_proxy_configs_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("proxy_url", DataType::Utf8, false),
        Field::new("proxy_username", DataType::Utf8, true),
        Field::new("proxy_password", DataType::Utf8, true),
        Field::new("status", DataType::Utf8, false),
        Field::new("created_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
        Field::new("updated_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
    ]))
}

/// Canonical schema for provider-level upstream proxy bindings.
pub fn llm_gateway_proxy_bindings_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("provider_type", DataType::Utf8, false),
        Field::new("proxy_config_id", DataType::Utf8, false),
        Field::new("updated_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
    ]))
}

/// Canonical schema for public token-request submissions.
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

/// Canonical schema for public Codex account-contribution submissions.
pub fn llm_gateway_account_contribution_requests_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("request_id", DataType::Utf8, false),
        Field::new("account_name", DataType::Utf8, false),
        Field::new("account_id", DataType::Utf8, true),
        Field::new("id_token", DataType::Utf8, false),
        Field::new("access_token", DataType::Utf8, false),
        Field::new("refresh_token", DataType::Utf8, false),
        Field::new("requester_email", DataType::Utf8, false),
        Field::new("contributor_message", DataType::Utf8, false),
        Field::new("github_id", DataType::Utf8, true),
        Field::new("frontend_page_url", DataType::Utf8, true),
        Field::new("status", DataType::Utf8, false),
        Field::new("fingerprint", DataType::Utf8, false),
        Field::new("client_ip", DataType::Utf8, false),
        Field::new("ip_region", DataType::Utf8, false),
        Field::new("admin_note", DataType::Utf8, true),
        Field::new("failure_reason", DataType::Utf8, true),
        Field::new("imported_account_name", DataType::Utf8, true),
        Field::new("issued_key_id", DataType::Utf8, true),
        Field::new("issued_key_name", DataType::Utf8, true),
        Field::new("created_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
        Field::new("updated_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
        Field::new("processed_at", DataType::Timestamp(TimeUnit::Millisecond, None), true),
    ]))
}

/// Canonical schema for public sponsor submissions.
pub fn llm_gateway_sponsor_requests_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("request_id", DataType::Utf8, false),
        Field::new("requester_email", DataType::Utf8, false),
        Field::new("sponsor_message", DataType::Utf8, false),
        Field::new("display_name", DataType::Utf8, true),
        Field::new("github_id", DataType::Utf8, true),
        Field::new("frontend_page_url", DataType::Utf8, true),
        Field::new("status", DataType::Utf8, false),
        Field::new("fingerprint", DataType::Utf8, false),
        Field::new("client_ip", DataType::Utf8, false),
        Field::new("ip_region", DataType::Utf8, false),
        Field::new("admin_note", DataType::Utf8, true),
        Field::new("failure_reason", DataType::Utf8, true),
        Field::new("payment_email_sent_at", DataType::Timestamp(TimeUnit::Millisecond, None), true),
        Field::new("created_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
        Field::new("updated_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
        Field::new("processed_at", DataType::Timestamp(TimeUnit::Millisecond, None), true),
    ]))
}

/// Ensure the key table exists, is migrated to the latest columns, and has the
/// required scalar indexes.
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
    // Backfill provider_type / protocol_family for tables created before
    // multi-provider support.
    ensure_nullable_utf8_column(&table, "provider_type").await?;
    ensure_nullable_utf8_column(&table, "protocol_family").await?;
    ensure_nullable_f64_column(&table, "usage_credit_total").await?;
    ensure_nullable_u64_column(&table, "usage_credit_missing_events").await?;
    ensure_nullable_utf8_column(&table, "route_strategy").await?;
    ensure_nullable_utf8_column(&table, "fixed_account_name").await?;
    ensure_nullable_utf8_column(&table, "auto_account_names_json").await?;
    ensure_nullable_utf8_column(&table, "model_name_map_json").await?;
    ensure_nullable_u64_column(&table, "request_max_concurrency").await?;
    ensure_nullable_u64_column(&table, "request_min_start_interval_ms").await?;
    ensure_nullable_bool_column(&table, "kiro_request_validation_enabled").await?;
    ensure_nullable_bool_column(&table, "kiro_cache_estimation_enabled").await?;
    ensure_scalar_index(&table, "id").await?;
    ensure_scalar_index(&table, "key_hash").await?;
    ensure_scalar_index(&table, "status").await?;
    ensure_scalar_index(&table, "public_visible").await?;
    Ok(table)
}

/// Ensure the usage-event table exists and has the latest columns/indexes.
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
    // Backfill provider_type for usage events recorded before multi-provider
    // routing.
    ensure_nullable_utf8_column(&table, "provider_type").await?;
    ensure_nullable_f64_column(&table, "credit_usage").await?;
    ensure_nullable_bool_column(&table, "credit_usage_missing").await?;
    ensure_nullable_utf8_column(&table, "last_message_content").await?;
    ensure_nullable_utf8_column(&table, "client_request_body_json").await?;
    ensure_nullable_utf8_column(&table, "upstream_request_body_json").await?;
    ensure_nullable_utf8_column(&table, "full_request_json").await?;
    ensure_scalar_index(&table, "id").await?;
    ensure_scalar_index(&table, "key_id").await?;
    ensure_scalar_index(&table, "provider_type").await?;
    ensure_scalar_index(&table, "created_at").await?;
    Ok(table)
}

/// Ensure the singleton runtime-config table exists.
pub async fn ensure_runtime_config_table(db: &Connection) -> Result<Table> {
    let table =
        ensure_table(db, LLM_GATEWAY_RUNTIME_CONFIG_TABLE, llm_gateway_runtime_config_schema(), &[
            ("new_table_enable_stable_row_ids", "true"),
            ("new_table_enable_v2_manifest_paths", "true"),
        ])
        .await?;
    // Backfill max_request_body_bytes for configs created before body-size
    // limiting.
    ensure_nullable_u64_column(&table, "max_request_body_bytes").await?;
    ensure_nullable_u64_column(&table, "account_failure_retry_limit").await?;
    ensure_nullable_u64_column(&table, "kiro_channel_max_concurrency").await?;
    ensure_nullable_u64_column(&table, "kiro_channel_min_start_interval_ms").await?;
    ensure_nullable_u64_column(&table, "codex_status_refresh_min_interval_seconds").await?;
    ensure_nullable_u64_column(&table, "codex_status_refresh_max_interval_seconds").await?;
    ensure_nullable_u64_column(&table, "codex_status_account_jitter_max_seconds").await?;
    ensure_nullable_u64_column(&table, "kiro_status_refresh_min_interval_seconds").await?;
    ensure_nullable_u64_column(&table, "kiro_status_refresh_max_interval_seconds").await?;
    ensure_nullable_u64_column(&table, "kiro_status_account_jitter_max_seconds").await?;
    ensure_nullable_u64_column(&table, "usage_event_flush_batch_size").await?;
    ensure_nullable_u64_column(&table, "usage_event_flush_interval_seconds").await?;
    ensure_nullable_u64_column(&table, "usage_event_flush_max_buffer_bytes").await?;
    ensure_nullable_utf8_column(&table, "kiro_cache_kmodels_json").await?;
    ensure_nullable_utf8_column(&table, "kiro_prefix_cache_mode").await?;
    ensure_nullable_u64_column(&table, "kiro_prefix_cache_max_tokens").await?;
    ensure_nullable_u64_column(&table, "kiro_prefix_cache_entry_ttl_seconds").await?;
    ensure_nullable_u64_column(&table, "kiro_conversation_anchor_max_entries").await?;
    ensure_nullable_u64_column(&table, "kiro_conversation_anchor_ttl_seconds").await?;
    ensure_scalar_index(&table, "id").await?;
    Ok(table)
}

/// Ensure the proxy-config inventory table exists.
pub async fn ensure_proxy_configs_table(db: &Connection) -> Result<Table> {
    let table =
        ensure_table(db, LLM_GATEWAY_PROXY_CONFIGS_TABLE, llm_gateway_proxy_configs_schema(), &[
            ("new_table_enable_stable_row_ids", "true"),
            ("new_table_enable_v2_manifest_paths", "true"),
        ])
        .await?;
    ensure_nullable_utf8_column(&table, "proxy_username").await?;
    ensure_nullable_utf8_column(&table, "proxy_password").await?;
    ensure_scalar_index(&table, "id").await?;
    ensure_scalar_index(&table, "status").await?;
    Ok(table)
}

/// Ensure the provider-binding table exists.
pub async fn ensure_proxy_bindings_table(db: &Connection) -> Result<Table> {
    let table =
        ensure_table(db, LLM_GATEWAY_PROXY_BINDINGS_TABLE, llm_gateway_proxy_bindings_schema(), &[
            ("new_table_enable_stable_row_ids", "true"),
            ("new_table_enable_v2_manifest_paths", "true"),
        ])
        .await?;
    ensure_scalar_index(&table, "provider_type").await?;
    ensure_scalar_index(&table, "proxy_config_id").await?;
    Ok(table)
}

/// Ensure the token-request queue table exists.
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

/// Ensure the account-contribution queue table exists.
pub async fn ensure_account_contribution_requests_table(db: &Connection) -> Result<Table> {
    let table = ensure_table(
        db,
        LLM_GATEWAY_ACCOUNT_CONTRIBUTION_REQUESTS_TABLE,
        llm_gateway_account_contribution_requests_schema(),
        &[
            ("new_table_enable_stable_row_ids", "true"),
            ("new_table_enable_v2_manifest_paths", "true"),
        ],
    )
    .await?;
    ensure_nullable_utf8_column(&table, "account_id").await?;
    ensure_nullable_utf8_column(&table, "github_id").await?;
    ensure_nullable_utf8_column(&table, "frontend_page_url").await?;
    ensure_nullable_utf8_column(&table, "admin_note").await?;
    ensure_nullable_utf8_column(&table, "failure_reason").await?;
    ensure_nullable_utf8_column(&table, "imported_account_name").await?;
    ensure_nullable_utf8_column(&table, "issued_key_id").await?;
    ensure_nullable_utf8_column(&table, "issued_key_name").await?;
    ensure_nullable_ts_column(&table, "processed_at").await?;
    ensure_scalar_index(&table, "request_id").await?;
    ensure_scalar_index(&table, "account_name").await?;
    ensure_scalar_index(&table, "requester_email").await?;
    ensure_scalar_index(&table, "status").await?;
    ensure_scalar_index(&table, "created_at").await?;
    Ok(table)
}

/// Ensure the sponsor-request queue table exists.
pub async fn ensure_sponsor_requests_table(db: &Connection) -> Result<Table> {
    let table = ensure_table(
        db,
        LLM_GATEWAY_SPONSOR_REQUESTS_TABLE,
        llm_gateway_sponsor_requests_schema(),
        &[
            ("new_table_enable_stable_row_ids", "true"),
            ("new_table_enable_v2_manifest_paths", "true"),
        ],
    )
    .await?;
    ensure_nullable_utf8_column(&table, "display_name").await?;
    ensure_nullable_utf8_column(&table, "github_id").await?;
    ensure_nullable_utf8_column(&table, "frontend_page_url").await?;
    ensure_nullable_utf8_column(&table, "admin_note").await?;
    ensure_nullable_utf8_column(&table, "failure_reason").await?;
    ensure_nullable_ts_column(&table, "payment_email_sent_at").await?;
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

/// Adds a nullable UInt64 column to an existing table without rewriting old
/// rows.
async fn ensure_nullable_u64_column(table: &Table, column: &str) -> Result<()> {
    let schema = table.schema().await?;
    if schema.field_with_name(column).is_ok() {
        return Ok(());
    }
    tracing::info!(table = %table.name(), column, "Adding nullable UInt64 column to LLM gateway table");
    table
        .add_columns(
            NewColumnTransform::AllNulls(Arc::new(Schema::new(vec![Field::new(
                column,
                DataType::UInt64,
                true,
            )]))),
            None,
        )
        .await
        .with_context(|| format!("failed to add `{column}` to `{}`", table.name()))?;
    Ok(())
}

/// Adds a nullable Float64 column to an existing table without rewriting old
/// rows.
async fn ensure_nullable_f64_column(table: &Table, column: &str) -> Result<()> {
    let schema = table.schema().await?;
    if schema.field_with_name(column).is_ok() {
        return Ok(());
    }
    tracing::info!(table = %table.name(), column, "Adding nullable Float64 column to LLM gateway table");
    table
        .add_columns(
            NewColumnTransform::AllNulls(Arc::new(Schema::new(vec![Field::new(
                column,
                DataType::Float64,
                true,
            )]))),
            None,
        )
        .await
        .with_context(|| format!("failed to add `{column}` to `{}`", table.name()))?;
    Ok(())
}

/// Adds a nullable Boolean column to an existing table without rewriting old
/// rows.
async fn ensure_nullable_bool_column(table: &Table, column: &str) -> Result<()> {
    let schema = table.schema().await?;
    if schema.field_with_name(column).is_ok() {
        return Ok(());
    }
    tracing::info!(table = %table.name(), column, "Adding nullable Boolean column to LLM gateway table");
    table
        .add_columns(
            NewColumnTransform::AllNulls(Arc::new(Schema::new(vec![Field::new(
                column,
                DataType::Boolean,
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

/// Ordered projection used when reading key rows back from LanceDB.
pub fn key_columns() -> [&'static str; 26] {
    [
        "id",
        "name",
        "secret",
        "key_hash",
        "status",
        "provider_type",
        "protocol_family",
        "public_visible",
        "quota_billable_limit",
        "usage_input_uncached_tokens",
        "usage_input_cached_tokens",
        "usage_output_tokens",
        "usage_billable_tokens",
        "usage_credit_total",
        "usage_credit_missing_events",
        "last_used_at",
        "created_at",
        "updated_at",
        "route_strategy",
        "fixed_account_name",
        "auto_account_names_json",
        "model_name_map_json",
        "request_max_concurrency",
        "request_min_start_interval_ms",
        "kiro_request_validation_enabled",
        "kiro_cache_estimation_enabled",
    ]
}

/// Ordered projection used when reading usage-event rows back from LanceDB.
pub fn usage_event_columns() -> [&'static str; 26] {
    [
        "id",
        "key_id",
        "key_name",
        "provider_type",
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
        "credit_usage",
        "credit_usage_missing",
        "client_ip",
        "ip_region",
        "request_headers_json",
        "last_message_content",
        "client_request_body_json",
        "upstream_request_body_json",
        "full_request_json",
        "created_at",
    ]
}

/// Ordered projection used when reading token-request rows back from LanceDB.
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

/// Ordered projection used when reading proxy-config rows back from LanceDB.
pub fn proxy_config_columns() -> [&'static str; 8] {
    [
        "id",
        "name",
        "proxy_url",
        "proxy_username",
        "proxy_password",
        "status",
        "created_at",
        "updated_at",
    ]
}

/// Ordered projection used when reading proxy-binding rows back from LanceDB.
pub fn proxy_binding_columns() -> [&'static str; 3] {
    ["provider_type", "proxy_config_id", "updated_at"]
}

/// Ordered projection used when reading account-contribution rows back from
/// LanceDB.
pub fn account_contribution_request_columns() -> [&'static str; 22] {
    [
        "request_id",
        "account_name",
        "account_id",
        "id_token",
        "access_token",
        "refresh_token",
        "requester_email",
        "contributor_message",
        "github_id",
        "frontend_page_url",
        "status",
        "fingerprint",
        "client_ip",
        "ip_region",
        "admin_note",
        "failure_reason",
        "imported_account_name",
        "issued_key_id",
        "issued_key_name",
        "created_at",
        "updated_at",
        "processed_at",
    ]
}

/// Ordered projection used when reading sponsor-request rows back from LanceDB.
pub fn sponsor_request_columns() -> [&'static str; 16] {
    [
        "request_id",
        "requester_email",
        "sponsor_message",
        "display_name",
        "github_id",
        "frontend_page_url",
        "status",
        "fingerprint",
        "client_ip",
        "ip_region",
        "admin_note",
        "failure_reason",
        "payment_email_sent_at",
        "created_at",
        "updated_at",
        "processed_at",
    ]
}

/// Escape a literal string for safe use inside a simple LanceDB SQL filter.
pub fn escape_literal(value: &str) -> String {
    value.replace('\'', "''")
}
