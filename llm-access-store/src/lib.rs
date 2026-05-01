//! Storage bootstrap helpers for the standalone LLM access service.

/// DuckDB analytics writer helpers.
pub mod duckdb;
/// Async repository adapters for runtime traits.
pub mod repository;
/// SQLite control-plane repository.
pub mod sqlite;

use std::path::Path;

use anyhow::Context;

/// Aggregated usage counters for one API key, derived from the DuckDB usage
/// fact table and written back into SQLite's operational rollup table.
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// Initialize a SQLite control-plane database at `path`.
pub fn initialize_sqlite_target_path(path: impl AsRef<Path>) -> anyhow::Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).with_context(|| {
            format!("failed to create sqlite parent directory `{}`", parent.display())
        })?;
    }
    let conn = rusqlite::Connection::open(path)
        .with_context(|| format!("failed to open sqlite database `{}`", path.display()))?;
    initialize_sqlite_target(&conn)
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

/// Initialize the SQLite control-plane schema for the standalone LLM access
/// service.
pub fn initialize_sqlite_target(conn: &rusqlite::Connection) -> anyhow::Result<()> {
    llm_access_migrations::run_sqlite_migrations(conn)
}

/// Return the DuckDB usage and audit schema SQL.
pub fn duckdb_schema_sql() -> String {
    llm_access_migrations::duckdb_schema_sql()
}

/// Return the SQLite control-plane schema SQL.
pub fn sqlite_schema_sql() -> String {
    llm_access_migrations::sqlite_migrations()
        .iter()
        .map(|migration| migration.sql)
        .collect::<Vec<_>>()
        .join("\n")
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
    use rusqlite::Connection as SqliteConnection;

    #[test]
    fn sqlite_target_schema_uses_operational_constraints_and_indexes() {
        let conn = SqliteConnection::open_in_memory().expect("open sqlite");

        super::initialize_sqlite_target(&conn).expect("initialize sqlite target");

        let journal_mode: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .expect("read sqlite journal mode");
        assert_eq!(journal_mode.to_ascii_lowercase(), "memory");

        let strict: i64 = conn
            .query_row("SELECT strict FROM pragma_table_list WHERE name = 'llm_keys'", [], |row| {
                row.get(0)
            })
            .expect("read strict flag");
        assert_eq!(strict, 1);

        conn.execute(
            "INSERT INTO llm_keys (
                key_id, name, secret, key_hash, status, provider_type, protocol_family,
                public_visible, quota_billable_limit, created_at_ms, updated_at_ms
            ) VALUES (
                'key-1', 'external', 'secret', 'hash', 'active', 'kiro', 'anthropic',
                1, 1000, 10, 10
            )",
            [],
        )
        .expect("insert valid key");

        let invalid_status = conn.execute(
            "INSERT INTO llm_keys (
                key_id, name, secret, key_hash, status, provider_type, protocol_family,
                public_visible, quota_billable_limit, created_at_ms, updated_at_ms
            ) VALUES (
                'key-2', 'bad', 'secret', 'hash2', 'deleted', 'kiro', 'anthropic',
                1, 1000, 10, 10
            )",
            [],
        );
        assert!(invalid_status.is_err());

        let route_config_valid = conn.execute(
            "INSERT INTO llm_key_route_config (
                key_id, route_strategy, auto_account_names_json
            ) VALUES (
                'key-1', 'auto', '[\"a\",\"b\"]'
            )",
            [],
        );
        assert!(route_config_valid.is_ok());

        conn.execute(
            "INSERT INTO llm_keys (
                key_id, name, secret, key_hash, status, provider_type, protocol_family,
                public_visible, quota_billable_limit, created_at_ms, updated_at_ms
            ) VALUES (
                'key-2', 'bad-json', 'secret', 'hash2', 'active', 'kiro', 'anthropic',
                1, 1000, 10, 10
            )",
            [],
        )
        .expect("insert second valid key");

        let route_config_invalid = conn.execute(
            "INSERT INTO llm_key_route_config (
                key_id, route_strategy, auto_account_names_json
            ) VALUES (
                'key-2', 'auto', 'not-json'
            )",
            [],
        );
        assert!(route_config_invalid.is_err());

        let key_provider_index_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master
                 WHERE type = 'index' AND name = 'idx_llm_keys_provider_status'",
                [],
                |row| row.get(0),
            )
            .expect("read key provider index count");
        assert_eq!(key_provider_index_count, 1);

        let request_tables = [
            "llm_token_requests",
            "llm_account_contribution_requests",
            "gpt2api_account_contribution_requests",
            "llm_sponsor_requests",
        ];
        for table in request_tables {
            let table_count: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [table],
                    |row| row.get(0),
                )
                .expect("read request table count");
            assert_eq!(table_count, 1, "missing request table {table}");
        }
    }

    #[test]
    fn duckdb_schema_keeps_usage_queries_on_wide_fact_table() {
        let schema = super::duckdb_schema_sql();

        assert!(schema.contains("CREATE TABLE IF NOT EXISTS usage_events"));
        assert!(schema.contains("key_name VARCHAR NOT NULL"));
        assert!(schema.contains("account_group_id_at_event VARCHAR"));
        assert!(schema.contains("route_strategy_at_event VARCHAR"));
        assert!(schema.contains("CREATE TABLE IF NOT EXISTS usage_event_details"));
        assert!(schema.contains("CREATE TABLE IF NOT EXISTS usage_rollups_hourly"));
        assert!(schema.contains("CREATE TABLE IF NOT EXISTS usage_rollups_daily"));
        assert!(!schema.contains("cdc_"));
        assert!(!schema.contains("CREATE INDEX IF NOT EXISTS idx_usage_events"));
        assert!(!schema.contains("CREATE UNIQUE INDEX IF NOT EXISTS idx_usage_events"));
        assert!(!schema.contains("CREATE TABLE IF NOT EXISTS dim_"));
        assert!(!schema.contains(" REFERENCES "));
    }

    #[test]
    fn path_helpers_create_sqlite_database_and_duckdb_schema_file() {
        let temp_root = std::env::temp_dir().join(format!(
            "llm-access-store-test-{}-{}",
            std::process::id(),
            "path-helpers"
        ));
        let _ = std::fs::remove_dir_all(&temp_root);
        let sqlite_path = temp_root.join("control").join("llm-access.sqlite3");
        let duckdb_schema_path = temp_root.join("analytics").join("duckdb-schema.sql");

        super::initialize_sqlite_target_path(&sqlite_path).expect("initialize sqlite path");
        super::write_duckdb_schema_file(&duckdb_schema_path).expect("write duckdb schema");

        assert!(sqlite_path.exists());
        let schema = std::fs::read_to_string(&duckdb_schema_path).expect("read duckdb schema file");
        assert!(schema.contains("CREATE TABLE IF NOT EXISTS usage_events"));

        std::fs::remove_dir_all(&temp_root).expect("cleanup temp root");
    }
}
