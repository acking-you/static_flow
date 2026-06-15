//! DuckDB connection/config helpers: runtime-limit formatting, temp-dir
//! resolution, and target initialization.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Context};
use llm_access_core::store::{
    AdminRuntimeConfig, DEFAULT_DUCKDB_USAGE_CHECKPOINT_THRESHOLD_MIB,
    DEFAULT_DUCKDB_USAGE_MEMORY_LIMIT_MIB,
};

use super::{
    segment::{remove_file_if_exists, tiered_compacting_dir},
    util::{duckdb_mib_setting, duckdb_string_literal},
    DuckDbUsageConnectionConfig, SharedDuckDbUsageConnectionConfig, TieredDuckDbUsageConfig,
    DUCKDB_USAGE_CONNECTION_MAX_TEMP_DIRECTORY_SIZE,
};

/// Initialize a DuckDB analytics database at `path`.
#[cfg(feature = "duckdb-runtime")]
pub fn initialize_duckdb_target_path(path: impl AsRef<Path>) -> anyhow::Result<()> {
    initialize_duckdb_target_path_with_connection_config(
        path,
        DuckDbUsageConnectionConfig::default(),
    )
}
#[cfg(feature = "duckdb-runtime")]
pub fn initialize_duckdb_target_path_with_connection_config(
    path: impl AsRef<Path>,
    connection_config: DuckDbUsageConnectionConfig,
) -> anyhow::Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).with_context(|| {
            format!("failed to create duckdb parent directory `{}`", parent.display())
        })?;
    }
    let conn = duckdb::Connection::open(path)
        .with_context(|| format!("failed to open duckdb database `{}`", path.display()))?;
    configure_duckdb_usage_connection(&conn, connection_config)?;
    crate::initialize_duckdb_target(&conn)
}
#[cfg(feature = "duckdb-runtime")]
impl Default for DuckDbUsageConnectionConfig {
    fn default() -> Self {
        Self {
            memory_limit_mib: DEFAULT_DUCKDB_USAGE_MEMORY_LIMIT_MIB,
            checkpoint_threshold_mib: DEFAULT_DUCKDB_USAGE_CHECKPOINT_THRESHOLD_MIB,
        }
    }
}
#[cfg(feature = "duckdb-runtime")]
impl DuckDbUsageConnectionConfig {
    /// Build DuckDB usage connection settings from admin runtime config.
    pub fn from_admin_runtime_config(config: &AdminRuntimeConfig) -> Self {
        Self {
            memory_limit_mib: config.duckdb_usage_memory_limit_mib.max(1),
            checkpoint_threshold_mib: config
                .duckdb_usage_checkpoint_threshold_mib
                .max(DEFAULT_DUCKDB_USAGE_CHECKPOINT_THRESHOLD_MIB),
        }
    }
}
#[cfg(feature = "duckdb-runtime")]
fn duckdb_usage_temp_dir() -> PathBuf {
    std::env::temp_dir().join("staticflow-llm-access-duckdb")
}
#[cfg(feature = "duckdb-runtime")]
pub fn connection_config_snapshot(
    connection_config: &SharedDuckDbUsageConnectionConfig,
) -> DuckDbUsageConnectionConfig {
    connection_config
        .read()
        .map(|config| *config)
        .unwrap_or_default()
}
#[cfg(feature = "duckdb-runtime")]
pub fn duckdb_usage_connection_sql(
    connection_config: &DuckDbUsageConnectionConfig,
    temp_dir_str: &str,
) -> String {
    format!(
        "
        SET memory_limit={};
        SET checkpoint_threshold={};
        SET threads=1;
        SET preserve_insertion_order=false;
        SET TimeZone='UTC';
        SET temp_directory={};
        SET max_temp_directory_size={};
        ",
        duckdb_string_literal(&duckdb_mib_setting(connection_config.memory_limit_mib)),
        duckdb_string_literal(&duckdb_mib_setting(connection_config.checkpoint_threshold_mib)),
        duckdb_string_literal(temp_dir_str),
        duckdb_string_literal(DUCKDB_USAGE_CONNECTION_MAX_TEMP_DIRECTORY_SIZE),
    )
}
#[cfg(feature = "duckdb-runtime")]
pub fn configure_duckdb_usage_connection(
    conn: &duckdb::Connection,
    connection_config: DuckDbUsageConnectionConfig,
) -> anyhow::Result<()> {
    let temp_dir = duckdb_usage_temp_dir();
    fs::create_dir_all(&temp_dir).with_context(|| {
        format!("failed to create duckdb usage temp directory `{}`", temp_dir.display())
    })?;
    let temp_dir_str = temp_dir
        .to_str()
        .ok_or_else(|| anyhow!("duckdb usage temp directory path is not valid UTF-8"))?;
    let sql = duckdb_usage_connection_sql(&connection_config, temp_dir_str);
    conn.execute_batch(&sql)
        .context("failed to configure duckdb usage connection")
}
#[cfg(feature = "duckdb-runtime")]
pub fn clear_stale_compacting_files(config: &TieredDuckDbUsageConfig) -> anyhow::Result<()> {
    let compacting_dir = tiered_compacting_dir(config);
    for entry in fs::read_dir(&compacting_dir).with_context(|| {
        format!("failed to read compacting duckdb directory `{}`", compacting_dir.display())
    })? {
        let entry = entry?;
        let path = entry.path();
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if path.is_file()
            && (file_name.ends_with(".tmp.duckdb") || file_name.ends_with(".tmp.duckdb.wal"))
        {
            remove_file_if_exists(&path)?;
        }
    }
    Ok(())
}
