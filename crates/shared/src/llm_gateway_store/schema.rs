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
    GPT2API_ACCOUNT_CONTRIBUTION_REQUESTS_TABLE, LLM_GATEWAY_ACCOUNT_CONTRIBUTION_REQUESTS_TABLE,
    LLM_GATEWAY_ACCOUNT_GROUPS_TABLE, LLM_GATEWAY_KEYS_TABLE, LLM_GATEWAY_PROXY_BINDINGS_TABLE,
    LLM_GATEWAY_PROXY_CONFIGS_TABLE, LLM_GATEWAY_RUNTIME_CONFIG_TABLE,
    LLM_GATEWAY_SPONSOR_REQUESTS_TABLE, LLM_GATEWAY_TOKEN_REQUESTS_TABLE,
    LLM_GATEWAY_USAGE_EVENTS_TABLE,
};
use crate::lance_schema_encoding::{compressed_utf8_field, low_cardinality_utf8_field};
#[cfg(test)]
use crate::lance_schema_encoding::{
    COMPRESSION_LEVEL_META_KEY, COMPRESSION_META_KEY, DICT_DIVISOR_META_KEY,
    DICT_SIZE_RATIO_META_KEY, DICT_VALUES_COMPRESSION_LEVEL_META_KEY,
    DICT_VALUES_COMPRESSION_META_KEY, HEAVY_TEXT_COMPRESSION_LEVEL, HEAVY_TEXT_COMPRESSION_SCHEME,
    LOW_CARDINALITY_DICT_DIVISOR, LOW_CARDINALITY_DICT_SIZE_RATIO,
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
        Field::new("account_group_id", DataType::Utf8, true),
        Field::new("model_name_map_json", DataType::Utf8, true),
        Field::new("request_max_concurrency", DataType::UInt64, true),
        Field::new("request_min_start_interval_ms", DataType::UInt64, true),
        Field::new("kiro_request_validation_enabled", DataType::Boolean, true),
        Field::new("kiro_cache_estimation_enabled", DataType::Boolean, true),
        Field::new("kiro_zero_cache_debug_enabled", DataType::Boolean, true),
        Field::new("kiro_cache_policy_override_json", DataType::Utf8, true),
        Field::new("kiro_billable_model_multipliers_override_json", DataType::Utf8, true),
    ]))
}

/// Canonical schema for reusable provider-scoped account groups.
pub fn llm_gateway_account_groups_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("provider_type", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("account_names_json", DataType::Utf8, false),
        Field::new("created_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
        Field::new("updated_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
    ]))
}

/// Canonical schema for the `llm_gateway_usage_events` table.
pub fn llm_gateway_usage_events_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        low_cardinality_utf8_field("key_id", false),
        low_cardinality_utf8_field("key_name", true),
        // Upstream LLM provider for this event (e.g. "anthropic", "openai").
        // Nullable for events recorded before multi-provider routing.
        low_cardinality_utf8_field("provider_type", true),
        low_cardinality_utf8_field("account_name", true),
        low_cardinality_utf8_field("request_method", true),
        low_cardinality_utf8_field("request_url", true),
        Field::new("latency_ms", DataType::Int32, true),
        Field::new("routing_wait_ms", DataType::UInt32, true),
        Field::new("upstream_headers_ms", DataType::UInt32, true),
        Field::new("post_headers_body_ms", DataType::UInt32, true),
        Field::new("request_body_bytes", DataType::UInt64, true),
        Field::new("request_body_read_ms", DataType::UInt32, true),
        Field::new("request_json_parse_ms", DataType::UInt32, true),
        Field::new("pre_handler_ms", DataType::UInt32, true),
        Field::new("first_sse_write_ms", DataType::UInt32, true),
        Field::new("stream_finish_ms", DataType::UInt32, true),
        Field::new("quota_failover_count", DataType::UInt32, true),
        compressed_utf8_field("routing_diagnostics_json", true),
        low_cardinality_utf8_field("endpoint", false),
        low_cardinality_utf8_field("model", true),
        Field::new("status_code", DataType::Int32, false),
        Field::new("input_uncached_tokens", DataType::UInt64, false),
        Field::new("input_cached_tokens", DataType::UInt64, false),
        Field::new("output_tokens", DataType::UInt64, false),
        Field::new("billable_tokens", DataType::UInt64, false),
        Field::new("usage_missing", DataType::Boolean, false),
        Field::new("credit_usage", DataType::Float64, true),
        Field::new("credit_usage_missing", DataType::Boolean, false),
        low_cardinality_utf8_field("client_ip", true),
        low_cardinality_utf8_field("ip_region", true),
        compressed_utf8_field("request_headers_json", true),
        compressed_utf8_field("last_message_content", true),
        compressed_utf8_field("client_request_body_json", true),
        compressed_utf8_field("upstream_request_body_json", true),
        compressed_utf8_field("full_request_json", true),
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
        Field::new("codex_client_version", DataType::Utf8, false),
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
        Field::new("usage_event_maintenance_enabled", DataType::Boolean, false),
        Field::new("usage_event_maintenance_interval_seconds", DataType::UInt64, false),
        Field::new("usage_event_detail_retention_days", DataType::Int64, false),
        Field::new("kiro_cache_kmodels_json", DataType::Utf8, false),
        Field::new("kiro_billable_model_multipliers_json", DataType::Utf8, false),
        Field::new("kiro_cache_policy_json", DataType::Utf8, false),
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

/// Canonical schema for public gpt2api-rs account contribution submissions.
pub fn gpt2api_account_contribution_requests_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("request_id", DataType::Utf8, false),
        Field::new("account_name", DataType::Utf8, false),
        Field::new("access_token", DataType::Utf8, true),
        Field::new("session_json", DataType::Utf8, true),
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
    ensure_nullable_utf8_column(&table, "account_group_id").await?;
    ensure_nullable_utf8_column(&table, "model_name_map_json").await?;
    ensure_nullable_u64_column(&table, "request_max_concurrency").await?;
    ensure_nullable_u64_column(&table, "request_min_start_interval_ms").await?;
    ensure_nullable_bool_column(&table, "kiro_request_validation_enabled").await?;
    ensure_nullable_bool_column(&table, "kiro_cache_estimation_enabled").await?;
    ensure_nullable_bool_column(&table, "kiro_zero_cache_debug_enabled").await?;
    ensure_nullable_utf8_column(&table, "kiro_cache_policy_override_json").await?;
    ensure_nullable_utf8_column(&table, "kiro_billable_model_multipliers_override_json").await?;
    ensure_scalar_index(&table, "id").await?;
    ensure_scalar_index(&table, "key_hash").await?;
    ensure_scalar_index(&table, "status").await?;
    ensure_scalar_index(&table, "public_visible").await?;
    Ok(table)
}

/// Create or migrate the reusable account-groups table.
pub async fn ensure_account_groups_table(db: &Connection) -> Result<Table> {
    let table =
        ensure_table(db, LLM_GATEWAY_ACCOUNT_GROUPS_TABLE, llm_gateway_account_groups_schema(), &[
            ("new_table_enable_stable_row_ids", "true"),
            ("new_table_enable_v2_manifest_paths", "true"),
        ])
        .await?;
    ensure_scalar_index(&table, "id").await?;
    ensure_scalar_index(&table, "provider_type").await?;
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
    ensure_nullable_u32_column(&table, "routing_wait_ms").await?;
    ensure_nullable_u32_column(&table, "upstream_headers_ms").await?;
    ensure_nullable_u32_column(&table, "post_headers_body_ms").await?;
    ensure_nullable_u64_column(&table, "request_body_bytes").await?;
    ensure_nullable_u32_column(&table, "request_body_read_ms").await?;
    ensure_nullable_u32_column(&table, "request_json_parse_ms").await?;
    ensure_nullable_u32_column(&table, "pre_handler_ms").await?;
    ensure_nullable_u32_column(&table, "first_sse_write_ms").await?;
    ensure_nullable_u32_column(&table, "stream_finish_ms").await?;
    ensure_nullable_u32_column(&table, "quota_failover_count").await?;
    ensure_nullable_utf8_column(&table, "routing_diagnostics_json").await?;
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
    ensure_nullable_utf8_column(&table, "codex_client_version").await?;
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
    ensure_nullable_bool_column(&table, "usage_event_maintenance_enabled").await?;
    ensure_nullable_u64_column(&table, "usage_event_maintenance_interval_seconds").await?;
    ensure_nullable_i64_column(&table, "usage_event_detail_retention_days").await?;
    ensure_nullable_utf8_column(&table, "kiro_cache_kmodels_json").await?;
    ensure_nullable_utf8_column(&table, "kiro_billable_model_multipliers_json").await?;
    ensure_nullable_utf8_column(&table, "kiro_cache_policy_json").await?;
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

/// Ensure the gpt2api-rs account-contribution queue table exists.
pub async fn ensure_gpt2api_account_contribution_requests_table(db: &Connection) -> Result<Table> {
    let table = ensure_table(
        db,
        GPT2API_ACCOUNT_CONTRIBUTION_REQUESTS_TABLE,
        gpt2api_account_contribution_requests_schema(),
        &[
            ("new_table_enable_stable_row_ids", "true"),
            ("new_table_enable_v2_manifest_paths", "true"),
        ],
    )
    .await?;
    ensure_nullable_utf8_column(&table, "access_token").await?;
    ensure_nullable_utf8_column(&table, "session_json").await?;
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

/// Adds a nullable UInt32 column to an existing table without rewriting old
/// rows.
async fn ensure_nullable_u32_column(table: &Table, column: &str) -> Result<()> {
    let schema = table.schema().await?;
    if schema.field_with_name(column).is_ok() {
        return Ok(());
    }
    tracing::info!(table = %table.name(), column, "Adding nullable UInt32 column to LLM gateway table");
    table
        .add_columns(
            NewColumnTransform::AllNulls(Arc::new(Schema::new(vec![Field::new(
                column,
                DataType::UInt32,
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

/// Adds a nullable Int64 column to an existing table without rewriting old
/// rows.
async fn ensure_nullable_i64_column(table: &Table, column: &str) -> Result<()> {
    let schema = table.schema().await?;
    if schema.field_with_name(column).is_ok() {
        return Ok(());
    }
    tracing::info!(table = %table.name(), column, "Adding nullable Int64 column to LLM gateway table");
    table
        .add_columns(
            NewColumnTransform::AllNulls(Arc::new(Schema::new(vec![Field::new(
                column,
                DataType::Int64,
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
pub fn key_columns() -> [&'static str; 30] {
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
        "account_group_id",
        "model_name_map_json",
        "request_max_concurrency",
        "request_min_start_interval_ms",
        "kiro_request_validation_enabled",
        "kiro_cache_estimation_enabled",
        "kiro_zero_cache_debug_enabled",
        "kiro_cache_policy_override_json",
        "kiro_billable_model_multipliers_override_json",
    ]
}

/// Ordered projection used when reading runtime-config rows back from LanceDB.
pub fn runtime_config_columns() -> [&'static str; 28] {
    [
        "id",
        "auth_cache_ttl_seconds",
        "max_request_body_bytes",
        "account_failure_retry_limit",
        "codex_client_version",
        "kiro_channel_max_concurrency",
        "kiro_channel_min_start_interval_ms",
        "codex_status_refresh_min_interval_seconds",
        "codex_status_refresh_max_interval_seconds",
        "codex_status_account_jitter_max_seconds",
        "kiro_status_refresh_min_interval_seconds",
        "kiro_status_refresh_max_interval_seconds",
        "kiro_status_account_jitter_max_seconds",
        "usage_event_flush_batch_size",
        "usage_event_flush_interval_seconds",
        "usage_event_flush_max_buffer_bytes",
        "usage_event_maintenance_enabled",
        "usage_event_maintenance_interval_seconds",
        "usage_event_detail_retention_days",
        "kiro_cache_kmodels_json",
        "kiro_billable_model_multipliers_json",
        "kiro_cache_policy_json",
        "kiro_prefix_cache_mode",
        "kiro_prefix_cache_max_tokens",
        "kiro_prefix_cache_entry_ttl_seconds",
        "kiro_conversation_anchor_max_entries",
        "kiro_conversation_anchor_ttl_seconds",
        "updated_at",
    ]
}

/// Ordered projection used when reading account-group rows back from LanceDB.
pub fn account_group_columns() -> [&'static str; 6] {
    ["id", "provider_type", "name", "account_names_json", "created_at", "updated_at"]
}

/// Ordered projection used when reading usage-event rows back from LanceDB.
pub fn usage_event_columns() -> [&'static str; 37] {
    [
        "id",
        "key_id",
        "key_name",
        "provider_type",
        "account_name",
        "request_method",
        "request_url",
        "latency_ms",
        "routing_wait_ms",
        "upstream_headers_ms",
        "post_headers_body_ms",
        "request_body_bytes",
        "request_body_read_ms",
        "request_json_parse_ms",
        "pre_handler_ms",
        "first_sse_write_ms",
        "stream_finish_ms",
        "quota_failover_count",
        "routing_diagnostics_json",
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

/// Ordered projection used when reading lightweight usage-event summaries.
pub fn usage_event_summary_columns() -> [&'static str; 33] {
    [
        "id",
        "key_id",
        "key_name",
        "provider_type",
        "account_name",
        "request_method",
        "request_url",
        "latency_ms",
        "routing_wait_ms",
        "upstream_headers_ms",
        "post_headers_body_ms",
        "request_body_bytes",
        "request_body_read_ms",
        "request_json_parse_ms",
        "pre_handler_ms",
        "first_sse_write_ms",
        "stream_finish_ms",
        "quota_failover_count",
        "routing_diagnostics_json",
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
        "last_message_content",
        "created_at",
    ]
}

/// Ordered projection used when rebuilding compact usage-event rows while
/// preserving headers but skipping large request bodies.
pub fn usage_event_rebuild_columns() -> [&'static str; 34] {
    [
        "id",
        "key_id",
        "key_name",
        "provider_type",
        "account_name",
        "request_method",
        "request_url",
        "latency_ms",
        "routing_wait_ms",
        "upstream_headers_ms",
        "post_headers_body_ms",
        "request_body_bytes",
        "request_body_read_ms",
        "request_json_parse_ms",
        "pre_handler_ms",
        "first_sse_write_ms",
        "stream_finish_ms",
        "quota_failover_count",
        "routing_diagnostics_json",
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

/// Ordered projection used when reading gpt2api account-contribution rows back
/// from LanceDB.
pub fn gpt2api_account_contribution_request_columns() -> [&'static str; 20] {
    [
        "request_id",
        "account_name",
        "access_token",
        "session_json",
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

#[cfg(test)]
mod tests {
    use super::{
        llm_gateway_usage_events_schema, COMPRESSION_LEVEL_META_KEY, COMPRESSION_META_KEY,
        DICT_DIVISOR_META_KEY, DICT_SIZE_RATIO_META_KEY, DICT_VALUES_COMPRESSION_LEVEL_META_KEY,
        DICT_VALUES_COMPRESSION_META_KEY, HEAVY_TEXT_COMPRESSION_LEVEL,
        HEAVY_TEXT_COMPRESSION_SCHEME, LOW_CARDINALITY_DICT_DIVISOR,
        LOW_CARDINALITY_DICT_SIZE_RATIO,
    };

    #[test]
    fn usage_event_heavy_text_columns_use_explicit_zstd_compression() {
        let schema = llm_gateway_usage_events_schema();
        let expected_level = HEAVY_TEXT_COMPRESSION_LEVEL.to_string();

        for column in [
            "routing_diagnostics_json",
            "request_headers_json",
            "last_message_content",
            "client_request_body_json",
            "upstream_request_body_json",
            "full_request_json",
        ] {
            let field = schema
                .field_with_name(column)
                .expect("usage-event text column exists");
            assert_eq!(
                field
                    .metadata()
                    .get(COMPRESSION_META_KEY)
                    .map(String::as_str),
                Some(HEAVY_TEXT_COMPRESSION_SCHEME),
                "{column} should force zstd compression",
            );
            assert_eq!(
                field
                    .metadata()
                    .get(COMPRESSION_LEVEL_META_KEY)
                    .map(String::as_str),
                Some(expected_level.as_str()),
                "{column} should pin a zstd compression level",
            );
        }
    }

    #[test]
    fn usage_event_routing_metric_columns_use_compact_non_negative_types() {
        let schema = llm_gateway_usage_events_schema();
        for column in [
            "routing_wait_ms",
            "upstream_headers_ms",
            "post_headers_body_ms",
            "request_body_read_ms",
            "request_json_parse_ms",
            "pre_handler_ms",
            "first_sse_write_ms",
            "stream_finish_ms",
            "quota_failover_count",
        ] {
            let field = schema
                .field_with_name(column)
                .expect("usage-event routing metric column exists");
            assert_eq!(
                field.data_type(),
                &arrow_schema::DataType::UInt32,
                "{column} should use compact unsigned storage"
            );
        }
        assert_eq!(
            schema
                .field_with_name("request_body_bytes")
                .expect("usage-event request body byte metric exists")
                .data_type(),
            &arrow_schema::DataType::UInt64,
            "request_body_bytes should preserve byte counts without lossy narrowing"
        );
        assert!(
            schema.field_with_name("upstream_response_ms").is_err(),
            "storage schema should use the semantically precise upstream_headers_ms name"
        );
        assert!(
            schema.field_with_name("upstream_body_ms").is_err(),
            "storage schema should use the semantically precise post_headers_body_ms name"
        );
    }

    #[test]
    fn usage_event_low_cardinality_columns_use_dictionary_friendly_metadata() {
        let schema = llm_gateway_usage_events_schema();
        let expected_dict_divisor = LOW_CARDINALITY_DICT_DIVISOR.to_string();
        let expected_dict_ratio = LOW_CARDINALITY_DICT_SIZE_RATIO.to_string();
        let expected_level = HEAVY_TEXT_COMPRESSION_LEVEL.to_string();

        for column in [
            "key_id",
            "key_name",
            "provider_type",
            "account_name",
            "request_method",
            "request_url",
            "endpoint",
            "model",
            "client_ip",
            "ip_region",
        ] {
            let field = schema
                .field_with_name(column)
                .expect("usage-event low-cardinality column exists");
            assert_eq!(
                field
                    .metadata()
                    .get(DICT_DIVISOR_META_KEY)
                    .map(String::as_str),
                Some(expected_dict_divisor.as_str()),
                "{column} should cap dictionary cardinality by divisor",
            );
            assert_eq!(
                field
                    .metadata()
                    .get(DICT_SIZE_RATIO_META_KEY)
                    .map(String::as_str),
                Some(expected_dict_ratio.as_str()),
                "{column} should allow dictionary encoding when it materially shrinks data",
            );
            assert_eq!(
                field
                    .metadata()
                    .get(DICT_VALUES_COMPRESSION_META_KEY)
                    .map(String::as_str),
                Some(HEAVY_TEXT_COMPRESSION_SCHEME),
                "{column} should compress dictionary values with zstd",
            );
            assert_eq!(
                field
                    .metadata()
                    .get(DICT_VALUES_COMPRESSION_LEVEL_META_KEY)
                    .map(String::as_str),
                Some(expected_level.as_str()),
                "{column} should pin dictionary-value zstd level",
            );
        }
    }
}
