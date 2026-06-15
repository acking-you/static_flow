//! Storage bootstrap helpers for the standalone LLM access service.

use std::path::Path;

use anyhow::Context;
use serde::{Deserialize, Serialize};

/// DuckDB analytics writer helpers.
pub mod duckdb;
/// Postgres control-plane repository.
pub mod postgres;
/// Shared record types reused by active storage backends.
pub(crate) mod records;
/// Valkey-backed request-path cache primitives.
pub mod request_cache;
/// Postgres-backed archived usage catalog metadata.
pub(crate) mod usage_catalog;

/// Aggregated usage counters for one API key from analytics usage events.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KeyUsageRollupSummary {
    /// API key id.
    pub key_id: String,
    /// Total uncached input tokens.
    pub input_uncached_tokens: i64,
    /// Total cached input tokens.
    pub input_cached_tokens: i64,
    /// Total output tokens.
    pub output_tokens: i64,
    /// Total billable tokens.
    pub billable_tokens: i64,
    /// Total provider credit usage as a decimal string.
    pub credit_total: String,
    /// Number of usage events without provider credit usage.
    pub credit_missing_events: i64,
    /// Latest usage event timestamp.
    pub last_used_at_ms: Option<i64>,
}

/// Write the DuckDB schema SQL to `path`.
pub fn write_duckdb_schema_file(path: impl AsRef<Path>) -> anyhow::Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).with_context(|| {
            format!("failed to create duckdb schema parent directory `{}`", parent.display())
        })?;
    }
    std::fs::write(path, duckdb_schema_sql())
        .with_context(|| format!("failed to write duckdb schema SQL `{}`", path.display()))
}

/// Initialize a Postgres control-plane database at `database_url`.
pub async fn initialize_postgres_target(database_url: &str) -> anyhow::Result<()> {
    let pool = sqlx_postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(database_url)
        .await
        .context("connect postgres for initialization")?;
    llm_access_migrations::run_postgres_migrations(&pool).await?;
    pool.close().await;
    Ok(())
}

/// Return the DuckDB usage and audit schema SQL.
pub fn duckdb_schema_sql() -> String {
    llm_access_migrations::duckdb_schema_sql()
}

/// Initialize the DuckDB usage and audit schema for the standalone LLM access
/// service.
#[cfg(feature = "duckdb-runtime")]
pub fn initialize_duckdb_target(conn: &::duckdb::Connection) -> anyhow::Result<()> {
    conn.execute_batch(&duckdb_schema_sql())
        .context("failed to initialize duckdb llm access schema")?;
    Ok(())
}

/// Initialize a DuckDB usage database at `path`.
#[cfg(feature = "duckdb-runtime")]
pub fn initialize_duckdb_target_path(path: impl AsRef<Path>) -> anyhow::Result<()> {
    crate::duckdb::initialize_duckdb_target_path(path)
}

#[cfg(test)]
mod tests {
    #[test]
    fn duckdb_schema_keeps_usage_queries_on_wide_fact_table() {
        let schema = super::duckdb_schema_sql();

        assert!(schema.contains("CREATE TABLE IF NOT EXISTS usage_events"));
        assert!(schema.contains("key_name VARCHAR NOT NULL"));
        assert!(schema.contains("account_group_id_at_event VARCHAR"));
        assert!(schema.contains("route_strategy_at_event VARCHAR"));
        assert!(schema.contains("stream_completed_cleanly BOOLEAN"));
        assert!(schema.contains("downstream_disconnect BOOLEAN"));
        assert!(schema.contains("final_event_type VARCHAR"));
        assert!(schema.contains("bytes_streamed BIGINT"));
        assert!(schema.contains("proxy_source_at_event VARCHAR"));
        assert!(schema.contains("proxy_config_id_at_event VARCHAR"));
        assert!(schema.contains("proxy_config_name_at_event VARCHAR"));
        assert!(schema.contains("proxy_url_at_event VARCHAR"));
        assert!(schema.contains("CREATE TABLE IF NOT EXISTS usage_event_details"));
        assert!(schema.contains("CREATE TABLE IF NOT EXISTS usage_rollups_hourly"));
        assert!(schema.contains("CREATE TABLE IF NOT EXISTS usage_rollups_daily"));
        assert!(schema.contains("CREATE TABLE IF NOT EXISTS proxy_traffic_rollups_hourly"));
        assert!(!schema.contains("cdc_"));
        assert!(!schema.contains("CREATE INDEX IF NOT EXISTS idx_usage_events"));
        assert!(!schema.contains("CREATE UNIQUE INDEX IF NOT EXISTS idx_usage_events"));
        assert!(!schema.contains("CREATE TABLE IF NOT EXISTS dim_"));
        assert!(!schema.contains(" REFERENCES "));
    }

    #[test]
    fn path_helpers_write_duckdb_schema_file() {
        let temp_root = std::env::temp_dir().join(format!(
            "llm-access-store-test-{}-{}",
            std::process::id(),
            "path-helpers"
        ));
        let _ = std::fs::remove_dir_all(&temp_root);
        let duckdb_schema_path = temp_root.join("analytics").join("duckdb-schema.sql");

        super::write_duckdb_schema_file(&duckdb_schema_path).expect("write duckdb schema");

        let schema = std::fs::read_to_string(&duckdb_schema_path).expect("read duckdb schema file");
        assert!(schema.contains("CREATE TABLE IF NOT EXISTS usage_events"));

        std::fs::remove_dir_all(&temp_root).expect("cleanup temp root");
    }
}
