//! DuckDB connection setup and introspection: connection-config SQL,
//! relation/column probing, target initialization, WAL/checkpoint paths, and
//! single-writer enforcement.

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

#[cfg(feature = "duckdb-runtime")]
pub(crate) fn duckdb_table_columns(
    conn: &duckdb::Connection,
    table_name: &str,
) -> anyhow::Result<HashSet<String>> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({})", duckdb_string_literal(table_name)))
        .with_context(|| format!("prepare {table_name} schema lookup"))?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .with_context(|| format!("query {table_name} schema"))?;
    let mut columns = HashSet::new();
    for row in rows {
        columns.insert(row.with_context(|| format!("read {table_name} schema row"))?);
    }
    Ok(columns)
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn duckdb_relation_exists(conn: &duckdb::Connection, relation_name: &str) -> bool {
    let sql = format!("SELECT 1 FROM {relation_name} LIMIT 0");
    conn.prepare(&sql)
        .and_then(|mut stmt| stmt.exists([]))
        .is_ok()
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn duckdb_relation_has_rows(conn: &duckdb::Connection, relation_name: &str) -> bool {
    let sql = format!("SELECT 1 FROM {relation_name} LIMIT 1");
    conn.query_row(&sql, [], |_row| Ok(()))
        .optional()
        .map(|row| row.is_some())
        .unwrap_or(false)
}
/// Initialize a DuckDB analytics database at `path`.
#[cfg(feature = "duckdb-runtime")]
pub fn initialize_duckdb_target_path(path: impl AsRef<Path>) -> anyhow::Result<()> {
    initialize_duckdb_target_path_with_connection_config(
        path,
        DuckDbUsageConnectionConfig::default(),
    )
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn initialize_duckdb_target_path_with_connection_config(
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
pub(crate) fn duckdb_usage_temp_dir() -> PathBuf {
    std::env::temp_dir().join("staticflow-llm-access-duckdb")
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn connection_config_snapshot(
    connection_config: &SharedDuckDbUsageConnectionConfig,
) -> DuckDbUsageConnectionConfig {
    connection_config
        .read()
        .map(|config| *config)
        .unwrap_or_default()
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn duckdb_mib_setting(value_mib: u64) -> String {
    format!("{}MB", value_mib.max(1))
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn configure_duckdb_usage_connection(
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
pub(crate) fn configure_duckdb_compact_connection(
    conn: &duckdb::Connection,
    temp_dir: &Path,
    connection_config: DuckDbUsageConnectionConfig,
) -> anyhow::Result<()> {
    fs::create_dir_all(temp_dir).with_context(|| {
        format!("failed to create duckdb compact temp directory `{}`", temp_dir.display())
    })?;
    let temp_dir_str = temp_dir
        .to_str()
        .ok_or_else(|| anyhow!("duckdb compact temp directory path is not valid UTF-8"))?;
    let sql = duckdb_compact_connection_sql(connection_config, temp_dir_str);
    conn.execute_batch(&sql)
        .context("failed to configure duckdb compact connection")?;
    Ok(())
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn duckdb_wal_path(path: &Path) -> PathBuf {
    let mut path = path.as_os_str().to_os_string();
    path.push(".wal");
    PathBuf::from(path)
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn checkpoint_duckdb_path(
    path: &Path,
    connection_config: DuckDbUsageConnectionConfig,
) -> anyhow::Result<()> {
    let conn = DuckDbUsageRepository::open_checkpoint_conn(path, connection_config)?;
    conn.execute_batch("CHECKPOINT;")
        .with_context(|| format!("failed to checkpoint duckdb database `{}`", path.display()))?;
    Ok(())
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn ensure_single_writer(
    state: &mut SingleDuckDbUsageState,
    connection_config: DuckDbUsageConnectionConfig,
) -> anyhow::Result<&mut PersistentUsageWriter> {
    let should_reopen = state
        .writer
        .as_ref()
        .map(|writer| writer.connection_config != connection_config)
        .unwrap_or(true);
    if should_reopen {
        state.writer = Some(PersistentUsageWriter::open(&state.path, connection_config, None)?);
    }
    state
        .writer
        .as_mut()
        .ok_or_else(|| anyhow!("single usage writer missing after initialization"))
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn disconnect_rate(group: &UsageMetricsGroupAccumulator) -> Option<f64> {
    (group.request_count > 0)
        .then(|| group.downstream_disconnect_count as f64 / group.request_count as f64)
}
