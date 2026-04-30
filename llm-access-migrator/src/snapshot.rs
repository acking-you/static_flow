//! Snapshot export/import contracts for seeding `llm-access` before CDC replay.

#[cfg(feature = "staticflow-source")]
use std::io::{BufWriter, Write};
use std::{
    fs,
    fs::File,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};
#[cfg(feature = "staticflow-source")]
use serde::Serialize;
#[cfg(feature = "staticflow-source")]
use static_flow_shared::llm_gateway_store::LlmGatewayStore;

const MANIFEST_FILE: &str = "manifest.json";
const KEYS_FILE: &str = "keys.jsonl";
const RUNTIME_CONFIG_FILE: &str = "runtime_config.jsonl";
const ACCOUNT_GROUPS_FILE: &str = "account_groups.jsonl";
const PROXY_CONFIGS_FILE: &str = "proxy_configs.jsonl";
const PROXY_BINDINGS_FILE: &str = "proxy_bindings.jsonl";
const TOKEN_REQUESTS_FILE: &str = "token_requests.jsonl";
const ACCOUNT_CONTRIBUTION_REQUESTS_FILE: &str = "account_contribution_requests.jsonl";
const GPT2API_ACCOUNT_CONTRIBUTION_REQUESTS_FILE: &str =
    "gpt2api_account_contribution_requests.jsonl";
const SPONSOR_REQUESTS_FILE: &str = "sponsor_requests.jsonl";
const USAGE_EVENTS_FILE: &str = "usage_events.jsonl";

const SNAPSHOT_DATA_FILES: &[&str] = &[
    KEYS_FILE,
    RUNTIME_CONFIG_FILE,
    ACCOUNT_GROUPS_FILE,
    PROXY_CONFIGS_FILE,
    PROXY_BINDINGS_FILE,
    TOKEN_REQUESTS_FILE,
    ACCOUNT_CONTRIBUTION_REQUESTS_FILE,
    GPT2API_ACCOUNT_CONTRIBUTION_REQUESTS_FILE,
    SPONSOR_REQUESTS_FILE,
    USAGE_EVENTS_FILE,
];

/// Stable manifest written beside one snapshot export.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SnapshotManifest {
    /// Source StaticFlow LanceDB content database path.
    pub source_db_path: String,
    /// Wall-clock export timestamp in Unix milliseconds.
    pub exported_at_ms: i64,
    /// Highest source CDC outbox sequence included at snapshot start.
    pub cdc_high_water_seq: i64,
    /// Number of key rows exported.
    pub keys: usize,
    /// Number of usage-event rows exported.
    pub usage_events: usize,
}

/// Options for exporting current LLM state from StaticFlow storage.
pub struct SnapshotExportOptions<'a> {
    /// Source StaticFlow LanceDB content database path.
    pub source_lancedb_path: &'a Path,
    /// Source SQLite CDC outbox path.
    pub source_cdc_sqlite_path: &'a Path,
    /// Directory that will receive manifest and snapshot data files.
    pub output_dir: &'a Path,
}

/// Options for importing one snapshot into `llm-access` storage files.
pub struct SnapshotImportOptions<'a> {
    /// Directory containing `manifest.json` and snapshot data files.
    pub snapshot_dir: &'a Path,
    /// Target SQLite control-plane database path.
    pub target_sqlite_path: &'a Path,
    /// Target DuckDB usage database path.
    pub target_duckdb_path: &'a Path,
}

/// Serialize a snapshot manifest in a human-reviewable format.
pub fn manifest_json(manifest: &SnapshotManifest) -> Result<String> {
    serde_json::to_string_pretty(manifest).context("serialize snapshot manifest")
}

/// Export a minimal, durable snapshot manifest.
///
/// Data-file population is added table-by-table; the manifest high-water mark
/// is intentionally available first so CDC replay can use one explicit
/// boundary.
pub fn export_snapshot(options: &SnapshotExportOptions<'_>) -> Result<SnapshotManifest> {
    fs::create_dir_all(options.output_dir).with_context(|| {
        format!("create snapshot output directory `{}`", options.output_dir.display())
    })?;
    let cdc_high_water_seq = read_source_cdc_high_water_seq(options.source_cdc_sqlite_path)?;
    create_empty_snapshot_files(options.output_dir)?;
    let manifest = SnapshotManifest {
        source_db_path: options.source_lancedb_path.display().to_string(),
        exported_at_ms: unix_ms(),
        cdc_high_water_seq,
        keys: 0,
        usage_events: 0,
    };
    fs::write(options.output_dir.join(MANIFEST_FILE), manifest_json(&manifest)?).with_context(
        || {
            format!(
                "write snapshot manifest to `{}`",
                options.output_dir.join(MANIFEST_FILE).display()
            )
        },
    )?;
    Ok(manifest)
}

/// Export current LLM state from the StaticFlow LanceDB store into JSONL files.
///
/// This is intentionally gated behind `staticflow-source` so the migrator's
/// core CDC replay code does not pay for LanceDB dependencies unless it is
/// being used as the one-time source exporter.
#[cfg(feature = "staticflow-source")]
pub async fn export_snapshot_from_staticflow_store(
    options: &SnapshotExportOptions<'_>,
    page_size: usize,
) -> Result<SnapshotManifest> {
    fs::create_dir_all(options.output_dir).with_context(|| {
        format!("create snapshot output directory `{}`", options.output_dir.display())
    })?;
    let cdc_high_water_seq = read_source_cdc_high_water_seq(options.source_cdc_sqlite_path)?;
    let store = LlmGatewayStore::connect(&options.source_lancedb_path.display().to_string())
        .await
        .with_context(|| {
            format!("connect StaticFlow LLM store `{}`", options.source_lancedb_path.display())
        })?;

    let keys = store.list_keys().await.context("export keys")?;
    write_jsonl_file(&options.output_dir.join(KEYS_FILE), &keys)?;

    let runtime_config = store
        .get_runtime_config_or_default()
        .await
        .context("export runtime config")?;
    write_jsonl_file(&options.output_dir.join(RUNTIME_CONFIG_FILE), &[runtime_config])?;

    let account_groups = store
        .list_account_groups()
        .await
        .context("export account groups")?;
    write_jsonl_file(&options.output_dir.join(ACCOUNT_GROUPS_FILE), &account_groups)?;

    let proxy_configs = store
        .list_proxy_configs()
        .await
        .context("export proxy configs")?;
    write_jsonl_file(&options.output_dir.join(PROXY_CONFIGS_FILE), &proxy_configs)?;

    let proxy_bindings = store
        .list_proxy_bindings()
        .await
        .context("export proxy bindings")?;
    write_jsonl_file(&options.output_dir.join(PROXY_BINDINGS_FILE), &proxy_bindings)?;

    let token_requests = collect_token_requests(&store, page_size).await?;
    write_jsonl_file(&options.output_dir.join(TOKEN_REQUESTS_FILE), &token_requests)?;

    let account_contribution_requests =
        collect_account_contribution_requests(&store, page_size).await?;
    write_jsonl_file(
        &options.output_dir.join(ACCOUNT_CONTRIBUTION_REQUESTS_FILE),
        &account_contribution_requests,
    )?;

    let gpt2api_account_contribution_requests =
        collect_gpt2api_account_contribution_requests(&store, page_size).await?;
    write_jsonl_file(
        &options
            .output_dir
            .join(GPT2API_ACCOUNT_CONTRIBUTION_REQUESTS_FILE),
        &gpt2api_account_contribution_requests,
    )?;

    let sponsor_requests = collect_sponsor_requests(&store, page_size).await?;
    write_jsonl_file(&options.output_dir.join(SPONSOR_REQUESTS_FILE), &sponsor_requests)?;

    let usage_events =
        write_usage_events(&store, page_size, &options.output_dir.join(USAGE_EVENTS_FILE)).await?;

    let manifest = SnapshotManifest {
        source_db_path: options.source_lancedb_path.display().to_string(),
        exported_at_ms: unix_ms(),
        cdc_high_water_seq,
        keys: keys.len(),
        usage_events,
    };
    fs::write(options.output_dir.join(MANIFEST_FILE), manifest_json(&manifest)?).with_context(
        || {
            format!(
                "write snapshot manifest to `{}`",
                options.output_dir.join(MANIFEST_FILE).display()
            )
        },
    )?;
    Ok(manifest)
}

/// Import a snapshot manifest and initialize the target storage containers.
pub fn import_snapshot(options: &SnapshotImportOptions<'_>) -> Result<SnapshotManifest> {
    let manifest_path = options.snapshot_dir.join(MANIFEST_FILE);
    let manifest: SnapshotManifest = serde_json::from_slice(
        &fs::read(&manifest_path)
            .with_context(|| format!("read snapshot manifest `{}`", manifest_path.display()))?,
    )
    .context("decode snapshot manifest")?;
    llm_access_store::initialize_sqlite_target_path(options.target_sqlite_path)
        .with_context(|| format!("initialize `{}`", options.target_sqlite_path.display()))?;
    let duckdb_schema_path = duckdb_schema_file_path(options.target_duckdb_path);
    llm_access_store::write_duckdb_schema_file(&duckdb_schema_path)
        .with_context(|| format!("write `{}`", duckdb_schema_path.display()))?;
    Ok(manifest)
}

fn read_source_cdc_high_water_seq(path: &Path) -> Result<i64> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("open source CDC SQLite `{}` read-only", path.display()))?;
    conn.query_row("SELECT COALESCE(MAX(seq), 0) FROM cdc_outbox", [], |row| row.get(0))
        .context("read source CDC high-water sequence")
}

fn create_empty_snapshot_files(output_dir: &Path) -> Result<()> {
    for file_name in SNAPSHOT_DATA_FILES {
        File::create(output_dir.join(file_name))
            .with_context(|| format!("create snapshot data file `{file_name}`"))?;
    }
    Ok(())
}

#[cfg(feature = "staticflow-source")]
fn write_jsonl_file<T: Serialize>(path: &Path, rows: &[T]) -> Result<()> {
    let mut writer = create_jsonl_writer(path)?;
    write_jsonl_rows(path, &mut writer, rows)?;
    writer
        .flush()
        .with_context(|| format!("flush JSONL file `{}`", path.display()))
}

#[cfg(feature = "staticflow-source")]
fn create_jsonl_writer(path: &Path) -> Result<BufWriter<File>> {
    let file =
        File::create(path).with_context(|| format!("create JSONL file `{}`", path.display()))?;
    Ok(BufWriter::new(file))
}

#[cfg(feature = "staticflow-source")]
fn write_jsonl_rows<T: Serialize>(
    path: &Path,
    writer: &mut BufWriter<File>,
    rows: &[T],
) -> Result<usize> {
    for row in rows {
        serde_json::to_writer(&mut *writer, row)
            .with_context(|| format!("write JSONL row to `{}`", path.display()))?;
        writer
            .write_all(b"\n")
            .with_context(|| format!("write JSONL newline to `{}`", path.display()))?;
    }
    Ok(rows.len())
}

#[cfg(feature = "staticflow-source")]
async fn collect_token_requests(
    store: &LlmGatewayStore,
    page_size: usize,
) -> Result<Vec<static_flow_shared::llm_gateway_store::LlmGatewayTokenRequestRecord>> {
    let total = store.count_token_requests(None).await?;
    let mut rows = Vec::with_capacity(total);
    let batch_size = page_size.max(1);
    for offset in (0..total).step_by(batch_size) {
        rows.extend(store.query_token_requests(None, batch_size, offset).await?);
    }
    Ok(rows)
}

#[cfg(feature = "staticflow-source")]
async fn collect_account_contribution_requests(
    store: &LlmGatewayStore,
    page_size: usize,
) -> Result<Vec<static_flow_shared::llm_gateway_store::LlmGatewayAccountContributionRequestRecord>>
{
    let total = store.count_account_contribution_requests(None).await?;
    let mut rows = Vec::with_capacity(total);
    let batch_size = page_size.max(1);
    for offset in (0..total).step_by(batch_size) {
        rows.extend(
            store
                .query_account_contribution_requests(None, batch_size, offset)
                .await?,
        );
    }
    Ok(rows)
}

#[cfg(feature = "staticflow-source")]
async fn collect_gpt2api_account_contribution_requests(
    store: &LlmGatewayStore,
    page_size: usize,
) -> Result<Vec<static_flow_shared::llm_gateway_store::Gpt2ApiAccountContributionRequestRecord>> {
    let total = store
        .count_gpt2api_account_contribution_requests(None)
        .await?;
    let mut rows = Vec::with_capacity(total);
    let batch_size = page_size.max(1);
    for offset in (0..total).step_by(batch_size) {
        rows.extend(
            store
                .query_gpt2api_account_contribution_requests(None, batch_size, offset)
                .await?,
        );
    }
    Ok(rows)
}

#[cfg(feature = "staticflow-source")]
async fn collect_sponsor_requests(
    store: &LlmGatewayStore,
    page_size: usize,
) -> Result<Vec<static_flow_shared::llm_gateway_store::LlmGatewaySponsorRequestRecord>> {
    let total = store.count_sponsor_requests(None).await?;
    let mut rows = Vec::with_capacity(total);
    let batch_size = page_size.max(1);
    for offset in (0..total).step_by(batch_size) {
        rows.extend(
            store
                .query_sponsor_requests(None, batch_size, offset)
                .await?,
        );
    }
    Ok(rows)
}

#[cfg(feature = "staticflow-source")]
async fn write_usage_events(
    store: &LlmGatewayStore,
    page_size: usize,
    path: &Path,
) -> Result<usize> {
    let total = store.count_usage_events(None).await?;
    let batch_size = page_size.max(1);
    let mut writer = create_jsonl_writer(path)?;
    let mut written = 0usize;
    for offset in (0..total).step_by(batch_size) {
        let rows = store
            .query_usage_event_rebuild_rows(None, None, Some(batch_size), Some(offset))
            .await?;
        if rows.is_empty() {
            break;
        }
        written += write_jsonl_rows(path, &mut writer, &rows)?;
    }
    writer
        .flush()
        .with_context(|| format!("flush JSONL file `{}`", path.display()))?;
    Ok(written)
}

fn duckdb_schema_file_path(path: &Path) -> PathBuf {
    path.with_extension("schema.sql")
}

fn unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_test_dir(name: &str) -> PathBuf {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        std::env::temp_dir()
            .join(format!("llm-access-migrator-{name}-{}-{now}", std::process::id()))
    }

    fn create_source_cdc(path: &Path, rows: usize) {
        let conn = Connection::open(path).expect("open source cdc");
        conn.execute_batch(
            "CREATE TABLE cdc_outbox (
                seq INTEGER PRIMARY KEY AUTOINCREMENT,
                event_id TEXT NOT NULL UNIQUE,
                source_instance TEXT NOT NULL,
                entity TEXT NOT NULL,
                op TEXT NOT NULL,
                primary_key TEXT NOT NULL,
                schema_version INTEGER NOT NULL,
                payload_json TEXT NOT NULL,
                created_at_ms INTEGER NOT NULL,
                committed_at_ms INTEGER NOT NULL
            ) STRICT;",
        )
        .expect("create cdc_outbox");
        for index in 0..rows {
            conn.execute(
                "INSERT INTO cdc_outbox (
                    event_id, source_instance, entity, op, primary_key, schema_version,
                    payload_json, created_at_ms, committed_at_ms
                 ) VALUES (?1, 'test', 'key', 'upsert', ?2, 1, '{}', 10, 10)",
                rusqlite::params![format!("event-{index}"), format!("key-{index}")],
            )
            .expect("insert cdc row");
        }
    }

    #[test]
    fn snapshot_manifest_records_cdc_high_water_mark() {
        let manifest = SnapshotManifest {
            source_db_path: "/data/lancedb".to_string(),
            exported_at_ms: 1000,
            cdc_high_water_seq: 42,
            keys: 3,
            usage_events: 9,
        };

        let json = manifest_json(&manifest).expect("manifest json");

        assert!(json.contains("\"cdc_high_water_seq\": 42"));
        assert!(json.contains("\"keys\": 3"));
        assert!(json.contains("\"usage_events\": 9"));
    }

    #[test]
    fn export_snapshot_writes_high_water_manifest_and_data_files() {
        let dir = unique_test_dir("export");
        fs::create_dir_all(&dir).expect("create test dir");
        let cdc_path = dir.join("source-cdc.sqlite");
        let snapshot_dir = dir.join("snapshot");
        create_source_cdc(&cdc_path, 2);

        let manifest = export_snapshot(&SnapshotExportOptions {
            source_lancedb_path: Path::new("/data/lancedb"),
            source_cdc_sqlite_path: &cdc_path,
            output_dir: &snapshot_dir,
        })
        .expect("export snapshot");

        assert_eq!(manifest.cdc_high_water_seq, 2);
        assert_eq!(manifest.keys, 0);
        assert_eq!(manifest.usage_events, 0);
        for file_name in SNAPSHOT_DATA_FILES {
            assert!(snapshot_dir.join(file_name).is_file(), "{file_name}");
        }
        fs::remove_dir_all(dir).expect("remove test dir");
    }

    #[test]
    fn import_snapshot_initializes_target_storage_files() {
        let dir = unique_test_dir("import");
        fs::create_dir_all(&dir).expect("create test dir");
        let snapshot_dir = dir.join("snapshot");
        fs::create_dir_all(&snapshot_dir).expect("create snapshot dir");
        let manifest = SnapshotManifest {
            source_db_path: "/data/lancedb".to_string(),
            exported_at_ms: 1000,
            cdc_high_water_seq: 2,
            keys: 0,
            usage_events: 0,
        };
        fs::write(
            snapshot_dir.join(MANIFEST_FILE),
            manifest_json(&manifest).expect("manifest json"),
        )
        .expect("write manifest");

        let loaded = import_snapshot(&SnapshotImportOptions {
            snapshot_dir: &snapshot_dir,
            target_sqlite_path: &dir.join("target.sqlite"),
            target_duckdb_path: &dir.join("usage.duckdb"),
        })
        .expect("import snapshot");

        assert_eq!(loaded, manifest);
        assert!(dir.join("target.sqlite").is_file());
        assert!(dir.join("usage.schema.sql").is_file());
        fs::remove_dir_all(dir).expect("remove test dir");
    }
}
