//! DuckDB analytics writer helpers for LLM usage events.

#[cfg(feature = "duckdb-runtime")]
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[cfg(feature = "duckdb-runtime")]
use anyhow::{anyhow, Context};
#[cfg(feature = "duckdb-runtime")]
use async_trait::async_trait;
#[cfg(feature = "duckdb-runtime")]
use llm_access_core::{
    provider::{ProtocolFamily, ProviderType, RouteStrategy},
    store::{
        UsageAnalyticsStore, UsageChartPoint, UsageEventPage, UsageEventQuery, UsageEventSink,
        UsageEventSource,
    },
    usage::{UsageEvent, UsageTiming},
};
#[cfg(feature = "duckdb-runtime")]
use rusqlite::OptionalExtension;
#[cfg(feature = "duckdb-runtime")]
use tokio::task;

#[cfg(feature = "duckdb-runtime")]
use crate::KeyUsageRollupSummary;

#[cfg(feature = "duckdb-runtime")]
static TIERED_SEGMENT_SEALER_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// One row for the DuckDB `usage_events` wide fact table.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageEventRow {
    /// Source CDC sequence, or zero for native standalone events.
    pub source_seq: i64,
    /// Source CDC event id, or this event id for native standalone events.
    pub source_event_id: String,
    /// Stable usage event id.
    pub event_id: String,
    /// Event creation timestamp in Unix milliseconds.
    pub created_at_ms: i64,
    /// Provider type at event time.
    pub provider_type: String,
    /// Protocol family at event time.
    pub protocol_family: String,
    /// API key id at event time.
    pub key_id: String,
    /// API key display name at event time.
    pub key_name: String,
    /// Key status captured at event time.
    pub key_status_at_event: String,
    /// Upstream account name at event time.
    pub account_name: Option<String>,
    /// Account group id captured at event time.
    pub account_group_id_at_event: Option<String>,
    /// Route strategy captured at event time.
    pub route_strategy_at_event: Option<String>,
    /// Incoming HTTP method.
    pub request_method: String,
    /// Operator-facing request URL.
    pub request_url: String,
    /// Provider endpoint.
    pub endpoint: String,
    /// Requested model name.
    pub model: Option<String>,
    /// Mapped upstream model name.
    pub mapped_model: Option<String>,
    /// Final HTTP status code.
    pub status_code: i64,
    /// Overall latency in milliseconds.
    pub latency_ms: Option<i64>,
    /// Time waiting for local routing or scheduler.
    pub routing_wait_ms: Option<i64>,
    /// Time until upstream headers.
    pub upstream_headers_ms: Option<i64>,
    /// Time from upstream headers until body completion.
    pub post_headers_body_ms: Option<i64>,
    /// Time spent reading the incoming request body.
    pub request_body_read_ms: Option<i64>,
    /// Time spent parsing request JSON.
    pub request_json_parse_ms: Option<i64>,
    /// Time until provider handler parsed the request.
    pub pre_handler_ms: Option<i64>,
    /// Time until first downstream SSE write.
    pub first_sse_write_ms: Option<i64>,
    /// Time until stream finish.
    pub stream_finish_ms: Option<i64>,
    /// Request body size in bytes.
    pub request_body_bytes: Option<i64>,
    /// Number of route failovers.
    pub quota_failover_count: i64,
    /// Routing diagnostics JSON.
    pub routing_diagnostics_json: Option<String>,
    /// Uncached input tokens.
    pub input_uncached_tokens: i64,
    /// Cached input tokens.
    pub input_cached_tokens: i64,
    /// Output tokens.
    pub output_tokens: i64,
    /// Billable tokens.
    pub billable_tokens: i64,
    /// Credit usage when known.
    pub credit_usage: Option<String>,
    /// Whether token usage was unavailable.
    pub usage_missing: bool,
    /// Whether credit usage was unavailable.
    pub credit_usage_missing: bool,
    /// Client IP captured at event time.
    pub client_ip: Option<String>,
    /// IP region captured at event time.
    pub ip_region: Option<String>,
    /// Request headers JSON.
    pub request_headers_json: String,
    /// Last message content preview.
    pub last_message_content: Option<String>,
    /// Client request body JSON when captured.
    pub client_request_body_json: Option<String>,
    /// Upstream request body JSON when captured.
    pub upstream_request_body_json: Option<String>,
    /// Full request JSON when captured.
    pub full_request_json: Option<String>,
}

impl UsageEventRow {
    /// Build a DuckDB fact row from the provider-neutral event.
    pub fn from_usage_event(event: &llm_access_core::usage::UsageEvent) -> Self {
        let latency_ms = event.timing.latency_ms.or_else(|| {
            event.timing.stream_finish_ms.or_else(|| {
                match (event.timing.upstream_headers_ms, event.timing.post_headers_body_ms) {
                    (Some(headers), Some(body)) => Some(headers.saturating_add(body)),
                    _ => None,
                }
            })
        });
        Self {
            source_seq: 0,
            source_event_id: event.event_id.clone(),
            event_id: event.event_id.clone(),
            created_at_ms: event.created_at_ms,
            provider_type: event.provider_type.as_storage_str().to_string(),
            protocol_family: event.protocol_family.as_storage_str().to_string(),
            key_id: event.key_id.clone(),
            key_name: event.key_name.clone(),
            key_status_at_event: "active".to_string(),
            account_name: event.account_name.clone(),
            account_group_id_at_event: event.account_group_id_at_event.clone(),
            route_strategy_at_event: event
                .route_strategy_at_event
                .map(|strategy| strategy.as_storage_str().to_string()),
            request_method: event.request_method.clone(),
            request_url: event.request_url.clone(),
            endpoint: event.endpoint.clone(),
            model: event.model.clone(),
            mapped_model: event.mapped_model.clone(),
            status_code: event.status_code,
            latency_ms,
            routing_wait_ms: event.timing.routing_wait_ms,
            upstream_headers_ms: event.timing.upstream_headers_ms,
            post_headers_body_ms: event.timing.post_headers_body_ms,
            request_body_read_ms: event.timing.request_body_read_ms,
            request_json_parse_ms: event.timing.request_json_parse_ms,
            pre_handler_ms: event.timing.pre_handler_ms,
            first_sse_write_ms: event.timing.first_sse_write_ms,
            stream_finish_ms: event.timing.stream_finish_ms,
            request_body_bytes: event.request_body_bytes,
            quota_failover_count: event.quota_failover_count.min(i64::MAX as u64) as i64,
            routing_diagnostics_json: event.routing_diagnostics_json.clone(),
            input_uncached_tokens: event.input_uncached_tokens,
            input_cached_tokens: event.input_cached_tokens,
            output_tokens: event.output_tokens,
            billable_tokens: event.billable_tokens,
            credit_usage: event.credit_usage.clone(),
            usage_missing: event.usage_missing,
            credit_usage_missing: event.credit_usage_missing,
            client_ip: Some(event.client_ip.clone()),
            ip_region: Some(event.ip_region.clone()),
            request_headers_json: event.request_headers_json.clone(),
            last_message_content: event.last_message_content.clone(),
            client_request_body_json: event.client_request_body_json.clone(),
            upstream_request_body_json: event.upstream_request_body_json.clone(),
            full_request_json: event.full_request_json.clone(),
        }
    }
}

/// Return the insert statement for the DuckDB `usage_events` wide fact table.
pub fn insert_usage_event_sql() -> &'static str {
    "INSERT INTO usage_events (
        source_seq, source_event_id, event_id, created_at_ms, created_at,
        created_date, created_hour, provider_type, protocol_family, key_id,
        key_name, key_status_at_event, account_name, account_group_id_at_event,
        route_strategy_at_event, request_method, request_url, endpoint, model,
        mapped_model, status_code, latency_ms, routing_wait_ms,
        upstream_headers_ms, post_headers_body_ms, request_body_read_ms,
        request_json_parse_ms, pre_handler_ms, first_sse_write_ms,
        stream_finish_ms, request_body_bytes, quota_failover_count,
        routing_diagnostics_json,
        input_uncached_tokens, input_cached_tokens, output_tokens, billable_tokens,
        credit_usage, usage_missing, credit_usage_missing, client_ip, ip_region,
        request_headers_json, last_message_content, client_request_body_json,
        upstream_request_body_json, full_request_json
     ) VALUES (
        ?1, ?2, ?3, ?4, to_timestamp(?4 / 1000.0),
        CAST(to_timestamp(?4 / 1000.0) AS DATE),
        date_trunc('hour', to_timestamp(?4 / 1000.0)),
        ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18,
        ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31,
        ?32, ?33, ?34, ?35, ?36, ?37, ?38, ?39, ?40, ?41, ?42, ?43, ?44
     )"
}

#[cfg(feature = "duckdb-runtime")]
const USAGE_EVENT_ONLINE_MAX_LIMIT: usize = 20;
#[cfg(feature = "duckdb-runtime")]
const USAGE_EVENT_ONLINE_MAX_OFFSET: usize = 200;

#[cfg(feature = "duckdb-runtime")]
const COUNT_USAGE_EVENTS_SQL: &str = "SELECT count(*)
    FROM usage_events
    WHERE (?1 IS NULL OR key_id = ?1)
      AND (?2 IS NULL OR provider_type = ?2)
      AND (?3 IS NULL OR created_at_ms >= ?3)
      AND (?4 IS NULL OR created_at_ms < ?4)";

#[cfg(feature = "duckdb-runtime")]
const LIST_USAGE_EVENT_SUMMARIES_SQL: &str = "SELECT event_id, created_at_ms,
        provider_type, protocol_family, key_id, key_name, account_name,
        account_group_id_at_event, route_strategy_at_event, request_method,
        request_url, endpoint, model, mapped_model, status_code,
        request_body_bytes, quota_failover_count, NULL AS routing_diagnostics_json,
        input_uncached_tokens, input_cached_tokens, output_tokens,
        billable_tokens, CAST(credit_usage AS VARCHAR), usage_missing,
        credit_usage_missing, latency_ms, routing_wait_ms, upstream_headers_ms,
        post_headers_body_ms, request_body_read_ms, request_json_parse_ms,
        pre_handler_ms, first_sse_write_ms, stream_finish_ms, client_ip,
        ip_region, NULL AS last_message_content
    FROM usage_events
    WHERE (?1 IS NULL OR key_id = ?1)
      AND (?2 IS NULL OR provider_type = ?2)
      AND (?3 IS NULL OR created_at_ms >= ?3)
      AND (?4 IS NULL OR created_at_ms < ?4)
    LIMIT ?5 OFFSET ?6";

#[cfg(feature = "duckdb-runtime")]
const GET_USAGE_EVENT_DETAIL_SQL: &str = "SELECT event_id, created_at_ms,
        provider_type, protocol_family, key_id, key_name, account_name,
        account_group_id_at_event, route_strategy_at_event, request_method,
        request_url, endpoint, model, mapped_model, status_code,
        request_body_bytes, quota_failover_count, routing_diagnostics_json,
        input_uncached_tokens, input_cached_tokens, output_tokens,
        billable_tokens, CAST(credit_usage AS VARCHAR), usage_missing,
        credit_usage_missing, latency_ms, routing_wait_ms, upstream_headers_ms,
        post_headers_body_ms, request_body_read_ms, request_json_parse_ms,
        pre_handler_ms, first_sse_write_ms, stream_finish_ms, client_ip,
        ip_region, last_message_content, request_headers_json,
        client_request_body_json, upstream_request_body_json, full_request_json
    FROM usage_events
    WHERE event_id = ?1";

/// DuckDB usage writer.
#[cfg(feature = "duckdb-runtime")]
pub struct DuckDbUsageWriter {
    conn: duckdb::Connection,
}

#[cfg(feature = "duckdb-runtime")]
impl DuckDbUsageWriter {
    /// Create a writer from an opened DuckDB connection.
    pub fn new(conn: duckdb::Connection) -> anyhow::Result<Self> {
        crate::initialize_duckdb_target(&conn)?;
        Ok(Self {
            conn,
        })
    }

    /// Insert one usage event row.
    pub fn insert_usage_event(&mut self, row: &UsageEventRow) -> anyhow::Result<()> {
        self.insert_usage_events(std::slice::from_ref(row))
    }

    /// Insert a batch of usage event rows in one transaction.
    pub fn insert_usage_events(&mut self, rows: &[UsageEventRow]) -> anyhow::Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(insert_usage_event_sql())?;
            for row in rows {
                execute_usage_event_insert(&mut stmt, row)?;
            }
        }
        tx.commit()?;
        Ok(())
    }
}

#[cfg(feature = "duckdb-runtime")]
fn execute_usage_event_insert(
    stmt: &mut duckdb::Statement<'_>,
    row: &UsageEventRow,
) -> anyhow::Result<()> {
    stmt.execute(duckdb::params![
        row.source_seq,
        &row.source_event_id,
        &row.event_id,
        row.created_at_ms,
        &row.provider_type,
        &row.protocol_family,
        &row.key_id,
        &row.key_name,
        &row.key_status_at_event,
        row.account_name.as_deref(),
        row.account_group_id_at_event.as_deref(),
        row.route_strategy_at_event.as_deref(),
        &row.request_method,
        &row.request_url,
        &row.endpoint,
        row.model.as_deref(),
        row.mapped_model.as_deref(),
        row.status_code,
        row.latency_ms,
        row.routing_wait_ms,
        row.upstream_headers_ms,
        row.post_headers_body_ms,
        row.request_body_read_ms,
        row.request_json_parse_ms,
        row.pre_handler_ms,
        row.first_sse_write_ms,
        row.stream_finish_ms,
        row.request_body_bytes,
        row.quota_failover_count,
        row.routing_diagnostics_json.as_deref(),
        row.input_uncached_tokens,
        row.input_cached_tokens,
        row.output_tokens,
        row.billable_tokens,
        row.credit_usage.as_deref(),
        row.usage_missing,
        row.credit_usage_missing,
        row.client_ip.as_deref(),
        row.ip_region.as_deref(),
        &row.request_headers_json,
        row.last_message_content.as_deref(),
        row.client_request_body_json.as_deref(),
        row.upstream_request_body_json.as_deref(),
        row.full_request_json.as_deref(),
    ])?;
    Ok(())
}

/// Initialize a DuckDB analytics database at `path`.
#[cfg(feature = "duckdb-runtime")]
pub fn initialize_duckdb_target_path(path: impl AsRef<Path>) -> anyhow::Result<()> {
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
    configure_duckdb_usage_connection(&conn)?;
    crate::initialize_duckdb_target(&conn)
}

/// File-backed DuckDB usage-event repository.
#[cfg(feature = "duckdb-runtime")]
#[derive(Debug, Clone)]
pub struct DuckDbUsageRepository {
    inner: Arc<DuckDbUsageRepositoryInner>,
}

#[cfg(feature = "duckdb-runtime")]
#[derive(Debug)]
enum DuckDbUsageRepositoryInner {
    Single { path: PathBuf },
    Tiered { config: TieredDuckDbUsageConfig, state: Mutex<TieredDuckDbUsageState> },
}

/// Tiered DuckDB usage storage configuration.
#[cfg(feature = "duckdb-runtime")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TieredDuckDbUsageConfig {
    /// Local directory for the current writable DuckDB file.
    pub active_dir: PathBuf,
    /// JuiceFS-backed directory for immutable archived DuckDB segments.
    pub archive_dir: PathBuf,
    /// JuiceFS-backed directory for the segment catalog SQLite database.
    pub catalog_dir: PathBuf,
    /// Rollover threshold in bytes for the active DuckDB file.
    pub rollover_bytes: u64,
}

#[cfg(feature = "duckdb-runtime")]
#[derive(Debug)]
struct TieredDuckDbUsageState {
    active_path: PathBuf,
    next_sequence: u64,
}

#[cfg(feature = "duckdb-runtime")]
#[derive(Debug, Clone)]
struct ArchivedUsageSegment {
    segment_id: String,
    archive_path: PathBuf,
    start_ms: Option<i64>,
    end_ms: Option<i64>,
    row_count: usize,
}

#[cfg(feature = "duckdb-runtime")]
#[derive(Debug, Clone)]
enum TieredUsagePartitionKind {
    Active,
    Archive,
}

#[cfg(feature = "duckdb-runtime")]
#[derive(Debug, Clone)]
struct TieredUsagePartition {
    path: PathBuf,
    count: usize,
    kind: TieredUsagePartitionKind,
}

#[cfg(feature = "duckdb-runtime")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TieredUsagePageFetch {
    partition_index: usize,
    local_newest_offset: usize,
    limit: usize,
}

#[cfg(feature = "duckdb-runtime")]
#[derive(Debug, Clone)]
struct SegmentKeyRollup {
    key_id: String,
    provider_type: String,
    row_count: usize,
    input_uncached_tokens: i64,
    input_cached_tokens: i64,
    output_tokens: i64,
    billable_tokens: i64,
    credit_total: String,
    credit_missing_events: i64,
    last_used_at_ms: Option<i64>,
}

#[cfg(feature = "duckdb-runtime")]
#[derive(Debug)]
struct SegmentStats {
    start_ms: Option<i64>,
    end_ms: Option<i64>,
    row_count: usize,
    event_id_count: usize,
    rollups: Vec<SegmentKeyRollup>,
}

#[cfg(feature = "duckdb-runtime")]
const DUCKDB_COMPACT_MEMORY_LIMIT: &str = "1536MB";

#[cfg(feature = "duckdb-runtime")]
const DUCKDB_COMPACT_MAX_TEMP_DIRECTORY_SIZE: &str = "8GB";

#[cfg(feature = "duckdb-runtime")]
const DUCKDB_USAGE_CONNECTION_MEMORY_LIMIT: &str = "512MB";

#[cfg(feature = "duckdb-runtime")]
const DUCKDB_USAGE_CONNECTION_MAX_TEMP_DIRECTORY_SIZE: &str = "2GB";

#[cfg(feature = "duckdb-runtime")]
impl DuckDbUsageRepository {
    /// Open a DuckDB usage repository and initialize the analytics schema.
    pub fn open_path(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref().to_path_buf();
        initialize_duckdb_target_path(&path)?;
        Ok(Self {
            inner: Arc::new(DuckDbUsageRepositoryInner::Single {
                path,
            }),
        })
    }

    /// Open a tiered DuckDB usage repository.
    pub fn open_tiered(config: TieredDuckDbUsageConfig) -> anyhow::Result<Self> {
        fs::create_dir_all(&config.active_dir).with_context(|| {
            format!("failed to create active duckdb directory `{}`", config.active_dir.display())
        })?;
        fs::create_dir_all(tiered_pending_dir(&config)).with_context(|| {
            format!(
                "failed to create pending duckdb directory `{}`",
                tiered_pending_dir(&config).display()
            )
        })?;
        fs::create_dir_all(tiered_compacting_dir(&config)).with_context(|| {
            format!(
                "failed to create compacting duckdb directory `{}`",
                tiered_compacting_dir(&config).display()
            )
        })?;
        fs::create_dir_all(&config.archive_dir).with_context(|| {
            format!("failed to create archive duckdb directory `{}`", config.archive_dir.display())
        })?;
        fs::create_dir_all(&config.catalog_dir).with_context(|| {
            format!("failed to create duckdb catalog directory `{}`", config.catalog_dir.display())
        })?;
        initialize_tiered_catalog(&config)?;
        clear_stale_compacting_files(&config)?;
        spawn_existing_pending_sealers(config.clone())?;

        let (active_path, next_sequence) = choose_active_segment(&config)?;
        initialize_duckdb_target_path(&active_path)?;
        Ok(Self {
            inner: Arc::new(DuckDbUsageRepositoryInner::Tiered {
                config,
                state: Mutex::new(TieredDuckDbUsageState {
                    active_path,
                    next_sequence,
                }),
            }),
        })
    }

    fn open_conn(path: &Path) -> anyhow::Result<duckdb::Connection> {
        let conn = duckdb::Connection::open(path)
            .with_context(|| format!("failed to open duckdb database `{}`", path.display()))?;
        configure_duckdb_usage_connection(&conn)?;
        Ok(conn)
    }

    fn open_read_only_conn(path: &Path) -> anyhow::Result<duckdb::Connection> {
        let config = duckdb::Config::default()
            .access_mode(duckdb::AccessMode::ReadOnly)
            .context("failed to configure duckdb read-only access")?;
        let conn = duckdb::Connection::open_with_flags(path, config).with_context(|| {
            format!("failed to open read-only duckdb database `{}`", path.display())
        })?;
        configure_duckdb_usage_connection(&conn)?;
        Ok(conn)
    }

    /// Aggregate all persisted usage events into per-key operational rollups.
    pub async fn key_usage_rollups(&self) -> anyhow::Result<Vec<KeyUsageRollupSummary>> {
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || match inner.as_ref() {
            DuckDbUsageRepositoryInner::Single {
                path,
            } => key_usage_rollups_from_path(path),
            DuckDbUsageRepositoryInner::Tiered {
                config,
                state,
            } => key_usage_rollups_from_tiered(config, state),
        })
        .await
        .context("duckdb key usage rollup task failed")?
    }
}

#[cfg(feature = "duckdb-runtime")]
fn tiered_catalog_path(config: &TieredDuckDbUsageConfig) -> PathBuf {
    config.catalog_dir.join("usage-segments.sqlite3")
}

#[cfg(feature = "duckdb-runtime")]
fn tiered_pending_dir(config: &TieredDuckDbUsageConfig) -> PathBuf {
    config.active_dir.join("pending")
}

#[cfg(feature = "duckdb-runtime")]
fn tiered_compacting_dir(config: &TieredDuckDbUsageConfig) -> PathBuf {
    config.active_dir.join("compacting")
}

#[cfg(feature = "duckdb-runtime")]
fn compacting_segment_path(config: &TieredDuckDbUsageConfig, segment_id: &str) -> PathBuf {
    tiered_compacting_dir(config).join(format!("{segment_id}.tmp.duckdb"))
}

#[cfg(feature = "duckdb-runtime")]
fn archive_segment_path(config: &TieredDuckDbUsageConfig, segment_id: &str) -> PathBuf {
    config.archive_dir.join(format!("{segment_id}.duckdb"))
}

#[cfg(feature = "duckdb-runtime")]
fn uploading_archive_segment_path(config: &TieredDuckDbUsageConfig, segment_id: &str) -> PathBuf {
    config
        .archive_dir
        .join(format!("{segment_id}.uploading.duckdb"))
}

#[cfg(feature = "duckdb-runtime")]
fn remove_file_if_exists(path: &Path) -> anyhow::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("failed to remove file `{}`", path.display())),
    }
}

#[cfg(feature = "duckdb-runtime")]
fn duckdb_string_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(feature = "duckdb-runtime")]
fn duckdb_usage_temp_dir() -> PathBuf {
    std::env::temp_dir().join("staticflow-llm-access-duckdb")
}

#[cfg(feature = "duckdb-runtime")]
fn configure_duckdb_usage_connection(conn: &duckdb::Connection) -> anyhow::Result<()> {
    let temp_dir = duckdb_usage_temp_dir();
    fs::create_dir_all(&temp_dir).with_context(|| {
        format!("failed to create duckdb usage temp directory `{}`", temp_dir.display())
    })?;
    let temp_dir_str = temp_dir
        .to_str()
        .ok_or_else(|| anyhow!("duckdb usage temp directory path is not valid UTF-8"))?;
    let sql = format!(
        "
        SET memory_limit={};
        SET threads=1;
        SET temp_directory={};
        SET max_temp_directory_size={};
        ",
        duckdb_string_literal(DUCKDB_USAGE_CONNECTION_MEMORY_LIMIT),
        duckdb_string_literal(temp_dir_str),
        duckdb_string_literal(DUCKDB_USAGE_CONNECTION_MAX_TEMP_DIRECTORY_SIZE),
    );
    conn.execute_batch(&sql)
        .context("failed to configure duckdb usage connection")
}

#[cfg(feature = "duckdb-runtime")]
fn clear_stale_compacting_files(config: &TieredDuckDbUsageConfig) -> anyhow::Result<()> {
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

#[cfg(feature = "duckdb-runtime")]
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

#[cfg(feature = "duckdb-runtime")]
fn initialize_tiered_catalog(config: &TieredDuckDbUsageConfig) -> anyhow::Result<()> {
    let path = tiered_catalog_path(config);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!("failed to create tiered catalog directory `{}`", parent.display())
        })?;
    }
    let conn = rusqlite::Connection::open(&path)
        .with_context(|| format!("failed to open tiered usage catalog `{}`", path.display()))?;
    conn.execute_batch(
        "
        PRAGMA journal_mode=DELETE;
        CREATE TABLE IF NOT EXISTS usage_segments (
            segment_id TEXT PRIMARY KEY,
            archive_path TEXT NOT NULL,
            state TEXT NOT NULL CHECK (state IN ('archived')),
            start_ms INTEGER,
            end_ms INTEGER,
            row_count INTEGER NOT NULL,
            size_bytes INTEGER NOT NULL,
            sealed_at_ms INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS usage_segment_events (
            event_id TEXT PRIMARY KEY,
            segment_id TEXT NOT NULL REFERENCES usage_segments(segment_id) ON DELETE CASCADE
        );
        CREATE TABLE IF NOT EXISTS usage_segment_key_rollups (
            segment_id TEXT NOT NULL REFERENCES usage_segments(segment_id) ON DELETE CASCADE,
            key_id TEXT NOT NULL,
            provider_type TEXT NOT NULL,
            row_count INTEGER NOT NULL,
            input_uncached_tokens INTEGER NOT NULL,
            input_cached_tokens INTEGER NOT NULL,
            output_tokens INTEGER NOT NULL,
            billable_tokens INTEGER NOT NULL,
            credit_total TEXT NOT NULL,
            credit_missing_events INTEGER NOT NULL,
            last_used_at_ms INTEGER,
            PRIMARY KEY (segment_id, key_id, provider_type)
        );
        CREATE INDEX IF NOT EXISTS idx_usage_segments_time
            ON usage_segments(end_ms, start_ms);
        CREATE INDEX IF NOT EXISTS idx_usage_segment_key_rollups_key
            ON usage_segment_key_rollups(key_id, provider_type);
        ",
    )
    .context("failed to initialize tiered usage catalog")?;
    Ok(())
}

#[cfg(feature = "duckdb-runtime")]
fn choose_active_segment(config: &TieredDuckDbUsageConfig) -> anyhow::Result<(PathBuf, u64)> {
    let mut active_files = Vec::new();
    for entry in fs::read_dir(&config.active_dir).with_context(|| {
        format!("failed to read active duckdb directory `{}`", config.active_dir.display())
    })? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file()
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("usage-active-") && name.ends_with(".duckdb"))
        {
            active_files.push(path);
        }
    }
    active_files.sort();
    if let Some(path) = active_files.pop() {
        let next = parse_segment_sequence(&path).unwrap_or(0).saturating_add(1);
        return Ok((path, next));
    }

    let next_sequence = next_catalog_sequence(config)?.saturating_add(1);
    Ok((active_segment_path(config, next_sequence), next_sequence.saturating_add(1)))
}

#[cfg(feature = "duckdb-runtime")]
fn next_catalog_sequence(config: &TieredDuckDbUsageConfig) -> anyhow::Result<u64> {
    let catalog = rusqlite::Connection::open(tiered_catalog_path(config))
        .context("failed to open tiered catalog for sequence lookup")?;
    let max_segment_id: Option<String> = catalog
        .query_row(
            "SELECT segment_id FROM usage_segments ORDER BY sealed_at_ms DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .context("failed to read latest usage segment id")?;
    Ok(max_segment_id
        .as_deref()
        .and_then(parse_sequence_from_segment_id)
        .unwrap_or(0))
}

#[cfg(feature = "duckdb-runtime")]
fn active_segment_path(config: &TieredDuckDbUsageConfig, sequence: u64) -> PathBuf {
    config
        .active_dir
        .join(format!("usage-active-{sequence:012}.duckdb"))
}

#[cfg(feature = "duckdb-runtime")]
fn parse_segment_sequence(path: &Path) -> Option<u64> {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .and_then(|stem| stem.rsplit('-').next())
        .and_then(|raw| raw.parse::<u64>().ok())
}

#[cfg(feature = "duckdb-runtime")]
fn parse_sequence_from_segment_id(segment_id: &str) -> Option<u64> {
    segment_id
        .rsplit('-')
        .next()
        .and_then(|raw| raw.parse::<u64>().ok())
}

#[cfg(feature = "duckdb-runtime")]
fn spawn_existing_pending_sealers(config: TieredDuckDbUsageConfig) -> anyhow::Result<()> {
    let pending_dir = tiered_pending_dir(&config);
    for entry in fs::read_dir(&pending_dir).with_context(|| {
        format!("failed to read pending duckdb directory `{}`", pending_dir.display())
    })? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("duckdb") {
            let segment_id = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("usage-recovered")
                .to_string();
            spawn_segment_sealer(config.clone(), path, segment_id);
        }
    }
    Ok(())
}

#[cfg(feature = "duckdb-runtime")]
fn spawn_segment_sealer(
    config: TieredDuckDbUsageConfig,
    pending_path: PathBuf,
    segment_id: String,
) {
    let _ = thread::Builder::new()
        .name("llm-access-duckdb-sealer".to_string())
        .spawn(move || {
            let Ok(_sealer_guard) = TIERED_SEGMENT_SEALER_LOCK.lock() else {
                eprintln!(
                    "failed to archive llm-access duckdb segment `{segment_id}` from `{}`: sealer \
                     lock poisoned",
                    pending_path.display()
                );
                return;
            };
            let mut last_err = None;
            for attempt in 0..5 {
                match publish_pending_segment(&config, &pending_path, &segment_id) {
                    Ok(()) => return,
                    Err(err) => {
                        last_err = Some(err);
                        thread::sleep(Duration::from_millis(250 * (attempt + 1)));
                    },
                }
            }
            if let Some(err) = last_err {
                eprintln!(
                    "failed to archive llm-access duckdb segment `{segment_id}` from `{}`: {err:#}",
                    pending_path.display()
                );
            }
        });
}

#[cfg(feature = "duckdb-runtime")]
fn publish_pending_segment(
    config: &TieredDuckDbUsageConfig,
    pending_path: &Path,
    segment_id: &str,
) -> anyhow::Result<()> {
    fs::create_dir_all(&config.archive_dir).with_context(|| {
        format!("failed to create archive directory `{}`", config.archive_dir.display())
    })?;
    let uploading_path = uploading_archive_segment_path(config, segment_id);
    let archive_path = archive_segment_path(config, segment_id);
    let compact_path = compacting_segment_path(config, segment_id);
    if archive_path.exists() {
        return finalize_archived_segment(
            config,
            pending_path,
            &compact_path,
            &uploading_path,
            &archive_path,
            segment_id,
        );
    }
    let compact_path = compact_pending_segment_to_local_file(config, pending_path, segment_id)?;
    let stats = validate_compacted_segment_matches_source(pending_path, &compact_path)?;
    remove_file_if_exists(&uploading_path)?;
    fs::copy(&compact_path, &uploading_path).with_context(|| {
        format!(
            "failed to copy compacted duckdb segment `{}` to uploading archive `{}`",
            compact_path.display(),
            uploading_path.display()
        )
    })?;
    fs::rename(&uploading_path, &archive_path).with_context(|| {
        format!(
            "failed to publish uploading archive `{}` to `{}`",
            uploading_path.display(),
            archive_path.display()
        )
    })?;
    let size_bytes = fs::metadata(&archive_path)
        .with_context(|| format!("failed to stat archived segment `{}`", archive_path.display()))?
        .len();
    publish_segment_catalog(config, segment_id, &archive_path, &stats, size_bytes)?;
    remove_file_if_exists(pending_path)?;
    remove_file_if_exists(&compact_path)?;
    Ok(())
}

#[cfg(feature = "duckdb-runtime")]
fn finalize_archived_segment(
    config: &TieredDuckDbUsageConfig,
    pending_path: &Path,
    compact_path: &Path,
    uploading_path: &Path,
    archive_path: &Path,
    segment_id: &str,
) -> anyhow::Result<()> {
    let stats = collect_segment_stats(archive_path)?;
    let size_bytes = fs::metadata(archive_path)
        .with_context(|| format!("failed to stat archived segment `{}`", archive_path.display()))?
        .len();
    publish_segment_catalog(config, segment_id, archive_path, &stats, size_bytes)?;
    remove_file_if_exists(uploading_path)?;
    remove_file_if_exists(pending_path)?;
    remove_file_if_exists(compact_path)?;
    Ok(())
}

#[cfg(feature = "duckdb-runtime")]
fn collect_segment_stats(path: &Path) -> anyhow::Result<SegmentStats> {
    let conn = DuckDbUsageRepository::open_read_only_conn(path)?;
    let (row_count, event_id_count, start_ms, end_ms): (i64, i64, Option<i64>, Option<i64>) = conn
        .query_row(
            "SELECT
                CAST(count(*) AS BIGINT),
                CAST(count(event_id) AS BIGINT),
                min(created_at_ms),
                max(created_at_ms)
             FROM usage_events",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .context("query duckdb segment stats")?;
    let mut stmt = conn
        .prepare(
            "SELECT
                key_id,
                provider_type,
                CAST(count(*) AS BIGINT),
                CAST(COALESCE(sum(input_uncached_tokens), 0) AS BIGINT),
                CAST(COALESCE(sum(input_cached_tokens), 0) AS BIGINT),
                CAST(COALESCE(sum(output_tokens), 0) AS BIGINT),
                CAST(COALESCE(sum(billable_tokens), 0) AS BIGINT),
                CAST(COALESCE(sum(COALESCE(try_cast(credit_usage AS DOUBLE), 0)), 0) AS VARCHAR),
                CAST(COALESCE(sum(CASE WHEN credit_usage_missing THEN 1 ELSE 0 END), 0) AS BIGINT),
                max(created_at_ms)
             FROM usage_events
             GROUP BY key_id, provider_type",
        )
        .context("prepare duckdb segment rollup query")?;
    let rollups = stmt
        .query_map([], |row| {
            Ok(SegmentKeyRollup {
                key_id: row.get(0)?,
                provider_type: row.get(1)?,
                row_count: i64_to_usize(row.get(2)?),
                input_uncached_tokens: row.get(3)?,
                input_cached_tokens: row.get(4)?,
                output_tokens: row.get(5)?,
                billable_tokens: row.get(6)?,
                credit_total: row.get(7)?,
                credit_missing_events: row.get(8)?,
                last_used_at_ms: row.get(9)?,
            })
        })
        .context("query duckdb segment rollups")?
        .collect::<Result<Vec<_>, _>>()
        .context("collect duckdb segment rollups")?;
    Ok(SegmentStats {
        start_ms,
        end_ms,
        row_count: i64_to_usize(row_count),
        event_id_count: i64_to_usize(event_id_count),
        rollups,
    })
}

#[cfg(feature = "duckdb-runtime")]
fn compact_pending_segment_to_local_file(
    config: &TieredDuckDbUsageConfig,
    pending_path: &Path,
    segment_id: &str,
) -> anyhow::Result<PathBuf> {
    fs::create_dir_all(tiered_compacting_dir(config)).with_context(|| {
        format!(
            "failed to create compacting duckdb directory `{}`",
            tiered_compacting_dir(config).display()
        )
    })?;
    let compact_path = compacting_segment_path(config, segment_id);
    remove_file_if_exists(&compact_path)?;

    let conn = DuckDbUsageRepository::open_conn(&compact_path)?;
    configure_duckdb_compact_connection(&conn, &tiered_compacting_dir(config))?;
    crate::initialize_duckdb_target(&conn)?;
    let pending_path_str = pending_path
        .to_str()
        .ok_or_else(|| anyhow!("pending duckdb segment path is not valid UTF-8"))?;
    let attach_sql = format!(
        "ATTACH DATABASE {} AS pending_segment (READ_ONLY);",
        duckdb_string_literal(pending_path_str)
    );
    conn.execute_batch(&attach_sql).with_context(|| {
        format!("failed to attach pending duckdb segment `{}`", pending_path.display())
    })?;
    conn.execute_batch(
        "
        INSERT INTO usage_events SELECT * FROM pending_segment.usage_events;
        INSERT INTO usage_event_details SELECT * FROM pending_segment.usage_event_details;
        INSERT INTO usage_rollups_hourly SELECT * FROM pending_segment.usage_rollups_hourly;
        INSERT INTO usage_rollups_daily SELECT * FROM pending_segment.usage_rollups_daily;
        DETACH pending_segment;
        CHECKPOINT;
        ",
    )
    .with_context(|| {
        format!(
            "failed to compact pending duckdb segment `{}` into `{}`",
            pending_path.display(),
            compact_path.display()
        )
    })?;
    Ok(compact_path)
}

#[cfg(feature = "duckdb-runtime")]
fn configure_duckdb_compact_connection(
    conn: &duckdb::Connection,
    temp_dir: &Path,
) -> anyhow::Result<()> {
    fs::create_dir_all(temp_dir).with_context(|| {
        format!("failed to create duckdb compact temp directory `{}`", temp_dir.display())
    })?;
    let temp_dir_str = temp_dir
        .to_str()
        .ok_or_else(|| anyhow!("duckdb compact temp directory path is not valid UTF-8"))?;
    let sql = format!(
        "
        SET memory_limit={};
        SET threads=1;
        SET preserve_insertion_order=false;
        SET temp_directory={};
        SET max_temp_directory_size={};
        ",
        duckdb_string_literal(DUCKDB_COMPACT_MEMORY_LIMIT),
        duckdb_string_literal(temp_dir_str),
        duckdb_string_literal(DUCKDB_COMPACT_MAX_TEMP_DIRECTORY_SIZE),
    );
    conn.execute_batch(&sql)
        .context("failed to configure duckdb compact connection")
}

#[cfg(feature = "duckdb-runtime")]
fn validate_compacted_segment_matches_source(
    source: &Path,
    compacted: &Path,
) -> anyhow::Result<SegmentStats> {
    let source_stats = collect_segment_stats(source)?;
    let compacted_stats = collect_segment_stats(compacted)?;
    if source_stats.row_count != compacted_stats.row_count
        || source_stats.event_id_count != compacted_stats.event_id_count
        || source_stats.start_ms != compacted_stats.start_ms
        || source_stats.end_ms != compacted_stats.end_ms
    {
        return Err(anyhow!(
            "compacted duckdb segment mismatch: source rows={} event_ids={} start={:?} end={:?}, \
             compacted rows={} event_ids={} start={:?} end={:?}",
            source_stats.row_count,
            source_stats.event_id_count,
            source_stats.start_ms,
            source_stats.end_ms,
            compacted_stats.row_count,
            compacted_stats.event_id_count,
            compacted_stats.start_ms,
            compacted_stats.end_ms
        ));
    }
    Ok(compacted_stats)
}

#[cfg(feature = "duckdb-runtime")]
fn publish_segment_catalog(
    config: &TieredDuckDbUsageConfig,
    segment_id: &str,
    archive_path: &Path,
    stats: &SegmentStats,
    size_bytes: u64,
) -> anyhow::Result<()> {
    let mut catalog = rusqlite::Connection::open(tiered_catalog_path(config))
        .context("failed to open tiered usage catalog for publication")?;
    let tx = catalog
        .transaction()
        .context("begin tiered catalog transaction")?;
    tx.execute(
        "INSERT OR REPLACE INTO usage_segments (
            segment_id, archive_path, state, start_ms, end_ms, row_count, size_bytes, sealed_at_ms
         ) VALUES (?1, ?2, 'archived', ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            segment_id,
            archive_path.to_string_lossy().as_ref(),
            stats.start_ms,
            stats.end_ms,
            usize_to_i64(stats.row_count),
            u64_to_i64(size_bytes),
            now_ms(),
        ],
    )
    .context("insert tiered usage segment catalog row")?;
    tx.execute("DELETE FROM usage_segment_events WHERE segment_id = ?1", rusqlite::params![
        segment_id
    ])
    .context("clear existing segment event locators")?;
    tx.execute("DELETE FROM usage_segment_key_rollups WHERE segment_id = ?1", rusqlite::params![
        segment_id
    ])
    .context("clear existing segment rollups")?;
    {
        let mut stmt = tx
            .prepare(
                "INSERT OR REPLACE INTO usage_segment_key_rollups (
                    segment_id, key_id, provider_type, row_count, input_uncached_tokens,
                    input_cached_tokens, output_tokens, billable_tokens, credit_total,
                    credit_missing_events, last_used_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            )
            .context("prepare tiered rollup insert")?;
        for rollup in &stats.rollups {
            stmt.execute(rusqlite::params![
                segment_id,
                rollup.key_id,
                rollup.provider_type,
                usize_to_i64(rollup.row_count),
                rollup.input_uncached_tokens,
                rollup.input_cached_tokens,
                rollup.output_tokens,
                rollup.billable_tokens,
                rollup.credit_total,
                rollup.credit_missing_events,
                rollup.last_used_at_ms,
            ])
            .context("insert tiered segment rollup")?;
        }
    }
    {
        let segment_conn = DuckDbUsageRepository::open_read_only_conn(archive_path)?;
        let mut event_query = segment_conn
            .prepare("SELECT event_id FROM usage_events")
            .context("prepare archived segment event locator query")?;
        let mut event_rows = event_query
            .query([])
            .context("query archived segment event locators")?;
        let mut insert_event = tx
            .prepare(
                "INSERT OR REPLACE INTO usage_segment_events (event_id, segment_id)
                 VALUES (?1, ?2)",
            )
            .context("prepare event locator insert")?;
        while let Some(row) = event_rows.next().context("read event locator row")? {
            let event_id: String = row.get(0)?;
            insert_event
                .execute(rusqlite::params![event_id, segment_id])
                .context("insert event locator")?;
        }
    }
    tx.commit()
        .context("commit tiered usage catalog transaction")?;
    Ok(())
}

#[cfg(feature = "duckdb-runtime")]
fn key_usage_rollups_from_path(path: &Path) -> anyhow::Result<Vec<KeyUsageRollupSummary>> {
    let conn = DuckDbUsageRepository::open_conn(path)?;
    key_usage_rollups_from_conn(&conn)
}

#[cfg(feature = "duckdb-runtime")]
fn key_usage_rollups_from_conn(
    conn: &duckdb::Connection,
) -> anyhow::Result<Vec<KeyUsageRollupSummary>> {
    let mut stmt = conn
        .prepare(
            "SELECT
                key_id,
                CAST(COALESCE(sum(input_uncached_tokens), 0) AS BIGINT),
                CAST(COALESCE(sum(input_cached_tokens), 0) AS BIGINT),
                CAST(COALESCE(sum(output_tokens), 0) AS BIGINT),
                CAST(COALESCE(sum(billable_tokens), 0) AS BIGINT),
                CAST(COALESCE(sum(COALESCE(try_cast(credit_usage AS DOUBLE), 0)), 0) AS VARCHAR),
                CAST(COALESCE(sum(CASE WHEN credit_usage_missing THEN 1 ELSE 0 END), 0) AS BIGINT),
                max(created_at_ms)
             FROM usage_events
             GROUP BY key_id",
        )
        .context("prepare duckdb key usage rollup query")?;
    let rows = stmt
        .query_map([], |row| {
            Ok(KeyUsageRollupSummary {
                key_id: row.get(0)?,
                input_uncached_tokens: row.get(1)?,
                input_cached_tokens: row.get(2)?,
                output_tokens: row.get(3)?,
                billable_tokens: row.get(4)?,
                credit_total: row.get(5)?,
                credit_missing_events: row.get(6)?,
                last_used_at_ms: row.get(7)?,
            })
        })
        .context("query duckdb key usage rollups")?;
    rows.collect::<Result<Vec<_>, _>>()
        .context("collect duckdb key usage rollups")
}

#[cfg(feature = "duckdb-runtime")]
fn key_usage_rollups_from_tiered(
    config: &TieredDuckDbUsageConfig,
    state: &Mutex<TieredDuckDbUsageState>,
) -> anyhow::Result<Vec<KeyUsageRollupSummary>> {
    let mut combined = BTreeMap::<String, KeyUsageRollupSummary>::new();
    {
        let state = state
            .lock()
            .map_err(|_| anyhow!("tiered duckdb state lock poisoned"))?;
        let conn = DuckDbUsageRepository::open_conn(&state.active_path)?;
        for rollup in key_usage_rollups_from_conn(&conn)? {
            merge_key_rollup(&mut combined, rollup);
        }
    }
    for rollup in archived_key_usage_rollups(config)? {
        merge_key_rollup(&mut combined, rollup);
    }
    Ok(combined.into_values().collect())
}

#[cfg(feature = "duckdb-runtime")]
fn merge_key_rollup(
    combined: &mut BTreeMap<String, KeyUsageRollupSummary>,
    rollup: KeyUsageRollupSummary,
) {
    let entry = combined
        .entry(rollup.key_id.clone())
        .or_insert_with(|| KeyUsageRollupSummary {
            key_id: rollup.key_id.clone(),
            input_uncached_tokens: 0,
            input_cached_tokens: 0,
            output_tokens: 0,
            billable_tokens: 0,
            credit_total: "0".to_string(),
            credit_missing_events: 0,
            last_used_at_ms: None,
        });
    entry.input_uncached_tokens = entry
        .input_uncached_tokens
        .saturating_add(rollup.input_uncached_tokens);
    entry.input_cached_tokens = entry
        .input_cached_tokens
        .saturating_add(rollup.input_cached_tokens);
    entry.output_tokens = entry.output_tokens.saturating_add(rollup.output_tokens);
    entry.billable_tokens = entry.billable_tokens.saturating_add(rollup.billable_tokens);
    let current_credit = entry.credit_total.parse::<f64>().unwrap_or(0.0);
    let added_credit = rollup.credit_total.parse::<f64>().unwrap_or(0.0);
    entry.credit_total = (current_credit + added_credit).to_string();
    entry.credit_missing_events = entry
        .credit_missing_events
        .saturating_add(rollup.credit_missing_events);
    entry.last_used_at_ms = match (entry.last_used_at_ms, rollup.last_used_at_ms) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (None, Some(right)) => Some(right),
        (left, None) => left,
    };
}

#[cfg(feature = "duckdb-runtime")]
fn archived_key_usage_rollups(
    config: &TieredDuckDbUsageConfig,
) -> anyhow::Result<Vec<KeyUsageRollupSummary>> {
    let catalog = rusqlite::Connection::open(tiered_catalog_path(config))
        .context("failed to open tiered catalog for archived rollups")?;
    let mut stmt = catalog
        .prepare(
            "SELECT
                key_id,
                COALESCE(sum(input_uncached_tokens), 0),
                COALESCE(sum(input_cached_tokens), 0),
                COALESCE(sum(output_tokens), 0),
                COALESCE(sum(billable_tokens), 0),
                COALESCE(sum(CAST(credit_total AS REAL)), 0),
                COALESCE(sum(credit_missing_events), 0),
                max(last_used_at_ms)
             FROM usage_segment_key_rollups
             GROUP BY key_id",
        )
        .context("prepare archived key usage rollup query")?;
    let rows = stmt
        .query_map([], |row| {
            let credit_total: f64 = row.get(5)?;
            Ok(KeyUsageRollupSummary {
                key_id: row.get(0)?,
                input_uncached_tokens: row.get(1)?,
                input_cached_tokens: row.get(2)?,
                output_tokens: row.get(3)?,
                billable_tokens: row.get(4)?,
                credit_total: credit_total.to_string(),
                credit_missing_events: row.get(6)?,
                last_used_at_ms: row.get(7)?,
            })
        })
        .context("query archived key usage rollups")?;
    rows.collect::<Result<Vec<_>, _>>()
        .context("collect archived key usage rollups")
}

#[cfg(feature = "duckdb-runtime")]
fn append_usage_events_to_tiered(
    config: &TieredDuckDbUsageConfig,
    state: &Mutex<TieredDuckDbUsageState>,
    rows: &[UsageEventRow],
) -> anyhow::Result<()> {
    let mut state = state
        .lock()
        .map_err(|_| anyhow!("tiered duckdb state lock poisoned"))?;
    {
        let mut writer =
            DuckDbUsageWriter::new(DuckDbUsageRepository::open_conn(&state.active_path)?)?;
        writer.insert_usage_events(rows)?;
    }
    let active_size = fs::metadata(&state.active_path)
        .map(|meta| meta.len())
        .unwrap_or(0);
    if active_size >= config.rollover_bytes.max(1) {
        rollover_active_segment(config, &mut state)?;
    }
    Ok(())
}

#[cfg(feature = "duckdb-runtime")]
fn rollover_active_segment(
    config: &TieredDuckDbUsageConfig,
    state: &mut TieredDuckDbUsageState,
) -> anyhow::Result<()> {
    checkpoint_duckdb_path(&state.active_path)?;
    let sequence = parse_segment_sequence(&state.active_path).unwrap_or(state.next_sequence);
    let segment_id = format!("usage-{}-{sequence:012}", now_ms());
    let pending_path = tiered_pending_dir(config).join(format!("{segment_id}.duckdb"));
    fs::rename(&state.active_path, &pending_path).with_context(|| {
        format!(
            "failed to move active duckdb segment `{}` to pending `{}`",
            state.active_path.display(),
            pending_path.display()
        )
    })?;
    let new_active_path = active_segment_path(config, state.next_sequence);
    state.next_sequence = state.next_sequence.saturating_add(1);
    initialize_duckdb_target_path(&new_active_path)?;
    state.active_path = new_active_path;
    spawn_segment_sealer(config.clone(), pending_path, segment_id);
    Ok(())
}

#[cfg(feature = "duckdb-runtime")]
fn checkpoint_duckdb_path(path: &Path) -> anyhow::Result<()> {
    let conn = DuckDbUsageRepository::open_conn(path)?;
    conn.execute_batch("CHECKPOINT;")
        .with_context(|| format!("failed to checkpoint duckdb database `{}`", path.display()))?;
    Ok(())
}

#[cfg(feature = "duckdb-runtime")]
#[async_trait]
impl UsageEventSink for DuckDbUsageRepository {
    async fn append_usage_event(&self, event: &UsageEvent) -> anyhow::Result<()> {
        self.append_usage_events(std::slice::from_ref(event)).await
    }

    async fn append_usage_events(&self, events: &[UsageEvent]) -> anyhow::Result<()> {
        if events.is_empty() {
            return Ok(());
        }
        let inner = Arc::clone(&self.inner);
        let rows = events
            .iter()
            .map(UsageEventRow::from_usage_event)
            .collect::<Vec<_>>();
        task::spawn_blocking(move || match inner.as_ref() {
            DuckDbUsageRepositoryInner::Single {
                path,
            } => {
                let mut writer = DuckDbUsageWriter::new(Self::open_conn(path)?)?;
                writer.insert_usage_events(&rows)
            },
            DuckDbUsageRepositoryInner::Tiered {
                config,
                state,
            } => append_usage_events_to_tiered(config, state, &rows),
        })
        .await
        .context("duckdb usage insert task failed")?
    }
}

#[cfg(feature = "duckdb-runtime")]
#[async_trait]
impl UsageAnalyticsStore for DuckDbUsageRepository {
    async fn list_usage_events(&self, query: UsageEventQuery) -> anyhow::Result<UsageEventPage> {
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || match inner.as_ref() {
            DuckDbUsageRepositoryInner::Single {
                path,
            } => list_usage_events_from_path(path, &query),
            DuckDbUsageRepositoryInner::Tiered {
                config,
                state,
            } => list_usage_events_from_tiered(config, state, &query),
        })
        .await
        .context("duckdb usage event list task failed")?
    }

    async fn get_usage_event(&self, event_id: &str) -> anyhow::Result<Option<UsageEvent>> {
        let inner = Arc::clone(&self.inner);
        let event_id = event_id.to_string();
        task::spawn_blocking(move || match inner.as_ref() {
            DuckDbUsageRepositoryInner::Single {
                path,
            } => get_usage_event_from_path(path, &event_id),
            DuckDbUsageRepositoryInner::Tiered {
                config,
                state,
            } => get_usage_event_from_tiered(config, state, &event_id),
        })
        .await
        .context("duckdb usage event detail task failed")?
    }

    async fn usage_chart_points(
        &self,
        key_id: &str,
        start_ms: i64,
        bucket_ms: i64,
        bucket_count: usize,
    ) -> anyhow::Result<Vec<UsageChartPoint>> {
        let inner = Arc::clone(&self.inner);
        let key_id = key_id.to_string();
        task::spawn_blocking(move || match inner.as_ref() {
            DuckDbUsageRepositoryInner::Single {
                path,
            } => usage_chart_points_from_single_path(
                path,
                &key_id,
                start_ms,
                bucket_ms,
                bucket_count,
            ),
            DuckDbUsageRepositoryInner::Tiered {
                config,
                state,
            } => usage_chart_points_from_tiered(
                config,
                state,
                &key_id,
                start_ms,
                bucket_ms,
                bucket_count,
            ),
        })
        .await
        .context("duckdb usage chart task failed")?
    }
}

#[cfg(feature = "duckdb-runtime")]
fn list_usage_events_from_path(
    path: &Path,
    query: &UsageEventQuery,
) -> anyhow::Result<UsageEventPage> {
    let conn = DuckDbUsageRepository::open_conn(path)?;
    list_usage_events_from_conn(&conn, query)
}

#[cfg(feature = "duckdb-runtime")]
fn list_usage_events_from_conn(
    conn: &duckdb::Connection,
    query: &UsageEventQuery,
) -> anyhow::Result<UsageEventPage> {
    let total = count_usage_events_from_conn(conn, query)?;
    let safe_limit = query.limit.min(USAGE_EVENT_ONLINE_MAX_LIMIT);
    let safe_offset = query.offset.min(USAGE_EVENT_ONLINE_MAX_OFFSET);
    if safe_limit == 0 || safe_offset >= total {
        return Ok(UsageEventPage {
            total,
            offset: safe_offset,
            limit: safe_limit,
            has_more: false,
            events: Vec::new(),
        });
    }
    let fetch_count = total.saturating_sub(safe_offset).min(safe_limit);
    let reverse_offset = total.saturating_sub(safe_offset.saturating_add(fetch_count));
    let mut events =
        fetch_usage_event_summaries_from_conn(conn, query, fetch_count, reverse_offset)?;
    events.reverse();
    Ok(UsageEventPage {
        total,
        offset: safe_offset,
        limit: safe_limit,
        has_more: safe_offset.saturating_add(events.len()) < total,
        events,
    })
}

#[cfg(feature = "duckdb-runtime")]
fn count_usage_events_from_conn(
    conn: &duckdb::Connection,
    query: &UsageEventQuery,
) -> anyhow::Result<usize> {
    let total: i64 = conn
        .query_row(
            COUNT_USAGE_EVENTS_SQL,
            duckdb::params![
                query.key_id.as_deref(),
                query.provider_type.as_deref(),
                query.start_ms,
                query.end_ms
            ],
            |row| row.get(0),
        )
        .context("count duckdb usage events")?;
    Ok(i64_to_usize(total))
}

#[cfg(feature = "duckdb-runtime")]
fn fetch_usage_event_summaries_from_conn(
    conn: &duckdb::Connection,
    query: &UsageEventQuery,
    limit: usize,
    offset: usize,
) -> anyhow::Result<Vec<UsageEvent>> {
    let mut stmt = conn
        .prepare(LIST_USAGE_EVENT_SUMMARIES_SQL)
        .context("prepare duckdb usage event summary query")?;
    let rows = stmt
        .query_map(
            duckdb::params![
                query.key_id.as_deref(),
                query.provider_type.as_deref(),
                query.start_ms,
                query.end_ms,
                usize_to_i64(limit),
                usize_to_i64(offset)
            ],
            decode_usage_event_summary_row,
        )
        .context("query duckdb usage events")?;
    rows.collect::<Result<Vec<_>, _>>()
        .context("collect duckdb usage events")
}

#[cfg(feature = "duckdb-runtime")]
fn list_usage_events_from_tiered(
    config: &TieredDuckDbUsageConfig,
    state: &Mutex<TieredDuckDbUsageState>,
    query: &UsageEventQuery,
) -> anyhow::Result<UsageEventPage> {
    let safe_limit = query.limit.min(USAGE_EVENT_ONLINE_MAX_LIMIT);
    let safe_offset = query.offset.min(USAGE_EVENT_ONLINE_MAX_OFFSET);
    let mut total = 0usize;
    let mut partitions = Vec::new();
    let mut events = Vec::new();

    if query.source.includes_hot() {
        let active_path = {
            let state = state
                .lock()
                .map_err(|_| anyhow!("tiered duckdb state lock poisoned"))?;
            state.active_path.clone()
        };
        let conn = DuckDbUsageRepository::open_conn(&active_path)?;
        let count = count_usage_events_from_conn(&conn, query)?;
        total = total.saturating_add(count);
        if count > 0 {
            partitions.push(TieredUsagePartition {
                path: active_path,
                count,
                kind: TieredUsagePartitionKind::Active,
            });
        }
    }

    if query.source.includes_archive() {
        let segments = archived_segments_for_query(config, query)?;
        for segment in segments {
            let count = archived_segment_usage_count(config, query, &segment)?;
            total = total.saturating_add(count);
            if count > 0 {
                partitions.push(TieredUsagePartition {
                    path: segment.archive_path,
                    count,
                    kind: TieredUsagePartitionKind::Archive,
                });
            }
        }
    }

    if safe_limit > 0 && safe_offset < total {
        let plan = plan_tiered_usage_page_fetches(
            partitions.iter().map(|partition| partition.count),
            safe_offset,
            safe_limit,
        );
        for fetch in plan {
            let partition = &partitions[fetch.partition_index];
            let conn = match partition.kind {
                TieredUsagePartitionKind::Active => {
                    DuckDbUsageRepository::open_conn(&partition.path)?
                },
                TieredUsagePartitionKind::Archive => {
                    DuckDbUsageRepository::open_read_only_conn(&partition.path)?
                },
            };
            let reverse_offset = partition
                .count
                .saturating_sub(fetch.local_newest_offset.saturating_add(fetch.limit));
            let mut partition_events =
                fetch_usage_event_summaries_from_conn(&conn, query, fetch.limit, reverse_offset)?;
            partition_events.reverse();
            events.extend(partition_events);
        }
    }

    Ok(UsageEventPage {
        total,
        offset: safe_offset,
        limit: safe_limit,
        has_more: safe_offset.saturating_add(events.len()) < total,
        events,
    })
}

#[cfg(feature = "duckdb-runtime")]
fn plan_tiered_usage_page_fetches<I>(
    partition_counts: I,
    offset: usize,
    limit: usize,
) -> Vec<TieredUsagePageFetch>
where
    I: IntoIterator<Item = usize>,
{
    if limit == 0 {
        return Vec::new();
    }
    let mut remaining_offset = offset;
    let mut remaining_limit = limit;
    let mut fetches = Vec::new();

    for (partition_index, count) in partition_counts.into_iter().enumerate() {
        if count == 0 {
            continue;
        }
        if remaining_offset >= count {
            remaining_offset -= count;
            continue;
        }

        let available = count - remaining_offset;
        let fetch_limit = available.min(remaining_limit);
        fetches.push(TieredUsagePageFetch {
            partition_index,
            local_newest_offset: remaining_offset,
            limit: fetch_limit,
        });
        remaining_limit -= fetch_limit;
        remaining_offset = 0;
        if remaining_limit == 0 {
            break;
        }
    }

    fetches
}

#[cfg(feature = "duckdb-runtime")]
fn archived_segments_for_query(
    config: &TieredDuckDbUsageConfig,
    query: &UsageEventQuery,
) -> anyhow::Result<Vec<ArchivedUsageSegment>> {
    let catalog = rusqlite::Connection::open(tiered_catalog_path(config))
        .context("failed to open tiered usage catalog for segment lookup")?;
    let mut stmt = catalog
        .prepare(
            "SELECT segment_id, archive_path, start_ms, end_ms, row_count
             FROM usage_segments
             WHERE state = 'archived'
               AND (?1 IS NULL OR end_ms IS NULL OR end_ms >= ?1)
               AND (?2 IS NULL OR start_ms IS NULL OR start_ms < ?2)
             ORDER BY COALESCE(end_ms, 0) DESC, segment_id DESC",
        )
        .context("prepare archived segment lookup")?;
    let rows = stmt
        .query_map(rusqlite::params![query.start_ms, query.end_ms], |row| {
            Ok(ArchivedUsageSegment {
                segment_id: row.get(0)?,
                archive_path: PathBuf::from(row.get::<_, String>(1)?),
                start_ms: row.get(2)?,
                end_ms: row.get(3)?,
                row_count: i64_to_usize(row.get(4)?),
            })
        })
        .context("query archived segments")?;
    rows.collect::<Result<Vec<_>, _>>()
        .context("collect archived segments")
}

#[cfg(feature = "duckdb-runtime")]
fn archived_segment_usage_count(
    config: &TieredDuckDbUsageConfig,
    query: &UsageEventQuery,
    segment: &ArchivedUsageSegment,
) -> anyhow::Result<usize> {
    if query.start_ms.is_none() && query.end_ms.is_none() || segment_fully_inside(segment, query) {
        return archived_segment_count_from_catalog(config, query, segment);
    }
    let conn = DuckDbUsageRepository::open_read_only_conn(&segment.archive_path)?;
    count_usage_events_from_conn(&conn, query)
}

#[cfg(feature = "duckdb-runtime")]
fn archived_segment_count_from_catalog(
    config: &TieredDuckDbUsageConfig,
    query: &UsageEventQuery,
    segment: &ArchivedUsageSegment,
) -> anyhow::Result<usize> {
    if query.key_id.is_none() && query.provider_type.is_none() {
        return Ok(segment.row_count);
    }
    let catalog = rusqlite::Connection::open(tiered_catalog_path(config))
        .context("failed to open tiered catalog for segment count")?;
    let total: i64 = catalog
        .query_row(
            "SELECT COALESCE(sum(row_count), 0)
             FROM usage_segment_key_rollups
             WHERE segment_id = ?1
               AND (?2 IS NULL OR key_id = ?2)
               AND (?3 IS NULL OR provider_type = ?3)",
            rusqlite::params![
                segment.segment_id,
                query.key_id.as_deref(),
                query.provider_type.as_deref()
            ],
            |row| row.get(0),
        )
        .context("query archived segment catalog count")?;
    Ok(i64_to_usize(total))
}

#[cfg(feature = "duckdb-runtime")]
fn segment_fully_inside(segment: &ArchivedUsageSegment, query: &UsageEventQuery) -> bool {
    let lower_ok = match (query.start_ms, segment.start_ms) {
        (Some(start), Some(segment_start)) => segment_start >= start,
        (Some(_), None) => false,
        (None, _) => true,
    };
    let upper_ok = match (query.end_ms, segment.end_ms) {
        (Some(end), Some(segment_end)) => segment_end < end,
        (Some(_), None) => false,
        (None, _) => true,
    };
    lower_ok && upper_ok
}

#[cfg(feature = "duckdb-runtime")]
fn get_usage_event_from_path(path: &Path, event_id: &str) -> anyhow::Result<Option<UsageEvent>> {
    let conn = DuckDbUsageRepository::open_conn(path)?;
    get_usage_event_from_conn(&conn, event_id)
}

#[cfg(feature = "duckdb-runtime")]
fn get_usage_event_from_conn(
    conn: &duckdb::Connection,
    event_id: &str,
) -> anyhow::Result<Option<UsageEvent>> {
    let mut stmt = conn
        .prepare(GET_USAGE_EVENT_DETAIL_SQL)
        .context("prepare duckdb usage event detail query")?;
    match stmt.query_row(duckdb::params![event_id], decode_usage_event_detail_row) {
        Ok(event) => Ok(Some(event)),
        Err(duckdb::Error::QueryReturnedNoRows) => Ok(None),
        Err(err) => Err(err).context("query duckdb usage event detail"),
    }
}

#[cfg(feature = "duckdb-runtime")]
fn get_usage_event_from_tiered(
    config: &TieredDuckDbUsageConfig,
    state: &Mutex<TieredDuckDbUsageState>,
    event_id: &str,
) -> anyhow::Result<Option<UsageEvent>> {
    {
        let state = state
            .lock()
            .map_err(|_| anyhow!("tiered duckdb state lock poisoned"))?;
        let conn = DuckDbUsageRepository::open_conn(&state.active_path)?;
        if let Some(event) = get_usage_event_from_conn(&conn, event_id)? {
            return Ok(Some(event));
        }
    }
    let Some(segment) = locate_archived_segment(config, event_id)? else {
        return Ok(None);
    };
    let conn = DuckDbUsageRepository::open_read_only_conn(&segment.archive_path)?;
    get_usage_event_from_conn(&conn, event_id)
}

#[cfg(feature = "duckdb-runtime")]
fn locate_archived_segment(
    config: &TieredDuckDbUsageConfig,
    event_id: &str,
) -> anyhow::Result<Option<ArchivedUsageSegment>> {
    let catalog = rusqlite::Connection::open(tiered_catalog_path(config))
        .context("failed to open tiered catalog for event locator")?;
    let row = catalog
        .query_row(
            "SELECT s.segment_id, s.archive_path, s.start_ms, s.end_ms, s.row_count
             FROM usage_segment_events e
             JOIN usage_segments s ON s.segment_id = e.segment_id
             WHERE e.event_id = ?1 AND s.state = 'archived'",
            rusqlite::params![event_id],
            |row| {
                Ok(ArchivedUsageSegment {
                    segment_id: row.get(0)?,
                    archive_path: PathBuf::from(row.get::<_, String>(1)?),
                    start_ms: row.get(2)?,
                    end_ms: row.get(3)?,
                    row_count: i64_to_usize(row.get(4)?),
                })
            },
        )
        .optional()
        .context("query archived event locator")?;
    Ok(row)
}

#[cfg(feature = "duckdb-runtime")]
fn usage_chart_points_from_tiered(
    config: &TieredDuckDbUsageConfig,
    state: &Mutex<TieredDuckDbUsageState>,
    key_id: &str,
    start_ms: i64,
    bucket_ms: i64,
    bucket_count: usize,
) -> anyhow::Result<Vec<UsageChartPoint>> {
    let mut points = empty_usage_chart_points(start_ms, bucket_ms, bucket_count);
    if bucket_count == 0 {
        return Ok(points);
    }
    {
        let state = state
            .lock()
            .map_err(|_| anyhow!("tiered duckdb state lock poisoned"))?;
        let conn = DuckDbUsageRepository::open_conn(&state.active_path)?;
        add_usage_chart_points_from_conn(&mut points, &conn, key_id, start_ms, bucket_ms)?;
    }
    let query = UsageEventQuery {
        key_id: Some(key_id.to_string()),
        provider_type: None,
        source: UsageEventSource::Archive,
        start_ms: Some(start_ms),
        end_ms: Some(start_ms.saturating_add((bucket_count as i64).saturating_mul(bucket_ms))),
        limit: USAGE_EVENT_ONLINE_MAX_LIMIT,
        offset: 0,
    };
    for segment in archived_segments_for_query(config, &query)? {
        let conn = DuckDbUsageRepository::open_read_only_conn(&segment.archive_path)?;
        add_usage_chart_points_from_conn(&mut points, &conn, key_id, start_ms, bucket_ms)?;
    }
    Ok(points)
}

#[cfg(feature = "duckdb-runtime")]
fn usage_chart_points_from_single_path(
    path: &Path,
    key_id: &str,
    start_ms: i64,
    bucket_ms: i64,
    bucket_count: usize,
) -> anyhow::Result<Vec<UsageChartPoint>> {
    let mut points = empty_usage_chart_points(start_ms, bucket_ms, bucket_count);
    if bucket_count == 0 {
        return Ok(points);
    }
    let conn = DuckDbUsageRepository::open_conn(path)?;
    add_usage_chart_points_from_conn(&mut points, &conn, key_id, start_ms, bucket_ms)?;
    Ok(points)
}

#[cfg(feature = "duckdb-runtime")]
fn empty_usage_chart_points(
    start_ms: i64,
    bucket_ms: i64,
    bucket_count: usize,
) -> Vec<UsageChartPoint> {
    (0..bucket_count)
        .map(|index| UsageChartPoint {
            bucket_start_ms: start_ms.saturating_add((index as i64).saturating_mul(bucket_ms)),
            tokens: 0,
        })
        .collect()
}

#[cfg(feature = "duckdb-runtime")]
fn add_usage_chart_points_from_conn(
    points: &mut [UsageChartPoint],
    conn: &duckdb::Connection,
    key_id: &str,
    start_ms: i64,
    bucket_ms: i64,
) -> anyhow::Result<()> {
    let end_ms = points
        .last()
        .map(|point| point.bucket_start_ms.saturating_add(bucket_ms))
        .unwrap_or(start_ms);
    let mut stmt = conn
        .prepare(
            "SELECT CAST(floor((created_at_ms - ?2) / ?3) AS BIGINT) AS bucket_index,
                    CAST(sum(input_uncached_tokens + output_tokens) AS BIGINT) AS tokens
             FROM usage_events
             WHERE key_id = ?1 AND created_at_ms >= ?2 AND created_at_ms < ?4
             GROUP BY bucket_index",
        )
        .context("prepare duckdb usage chart query")?;
    let mut rows = stmt
        .query(duckdb::params![key_id, start_ms, bucket_ms, end_ms])
        .context("query duckdb usage chart")?;
    while let Some(row) = rows.next().context("read duckdb usage chart row")? {
        let bucket_index: i64 = row.get(0)?;
        let tokens: i64 = row.get(1)?;
        if let Ok(index) = usize::try_from(bucket_index) {
            if let Some(point) = points.get_mut(index) {
                point.tokens = point.tokens.saturating_add(tokens.max(0) as u64);
            }
        }
    }
    Ok(())
}

#[cfg(feature = "duckdb-runtime")]
fn i64_to_usize(value: i64) -> usize {
    usize::try_from(value.max(0)).unwrap_or(usize::MAX)
}

#[cfg(feature = "duckdb-runtime")]
fn usize_to_i64(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

#[cfg(feature = "duckdb-runtime")]
fn u64_to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

#[cfg(feature = "duckdb-runtime")]
fn decode_usage_event_summary_row(row: &duckdb::Row<'_>) -> duckdb::Result<UsageEvent> {
    decode_usage_event_row(row, false)
}

#[cfg(feature = "duckdb-runtime")]
fn decode_usage_event_detail_row(row: &duckdb::Row<'_>) -> duckdb::Result<UsageEvent> {
    decode_usage_event_row(row, true)
}

#[cfg(feature = "duckdb-runtime")]
fn decode_usage_event_row(
    row: &duckdb::Row<'_>,
    include_detail_payload: bool,
) -> duckdb::Result<UsageEvent> {
    let provider_type_raw: String = row.get(2)?;
    let protocol_family_raw: String = row.get(3)?;
    let route_strategy_raw: Option<String> = row.get(8)?;
    let provider_type = ProviderType::from_storage_str(&provider_type_raw).ok_or_else(|| {
        duckdb::Error::FromSqlConversionFailure(
            2,
            duckdb::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid provider_type `{provider_type_raw}`"),
            )),
        )
    })?;
    let protocol_family =
        ProtocolFamily::from_storage_str(&protocol_family_raw).ok_or_else(|| {
            duckdb::Error::FromSqlConversionFailure(
                3,
                duckdb::types::Type::Text,
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("invalid protocol_family `{protocol_family_raw}`"),
                )),
            )
        })?;
    let route_strategy_at_event = match route_strategy_raw.as_deref() {
        Some(value) => Some(RouteStrategy::from_storage_str(value).ok_or_else(|| {
            duckdb::Error::FromSqlConversionFailure(
                8,
                duckdb::types::Type::Text,
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("invalid route_strategy_at_event `{value}`"),
                )),
            )
        })?),
        None => None,
    };
    Ok(UsageEvent {
        event_id: row.get(0)?,
        created_at_ms: row.get(1)?,
        provider_type,
        protocol_family,
        key_id: row.get(4)?,
        key_name: row.get(5)?,
        account_name: row.get(6)?,
        account_group_id_at_event: row.get(7)?,
        route_strategy_at_event,
        request_method: row.get(9)?,
        request_url: row.get(10)?,
        endpoint: row.get(11)?,
        model: row.get(12)?,
        mapped_model: row.get(13)?,
        status_code: row.get(14)?,
        request_body_bytes: row.get(15)?,
        quota_failover_count: u64::try_from(row.get::<_, i64>(16)?.max(0)).unwrap_or(u64::MAX),
        routing_diagnostics_json: row.get(17)?,
        input_uncached_tokens: row.get(18)?,
        input_cached_tokens: row.get(19)?,
        output_tokens: row.get(20)?,
        billable_tokens: row.get(21)?,
        credit_usage: row.get(22)?,
        usage_missing: row.get(23)?,
        credit_usage_missing: row.get(24)?,
        client_ip: row
            .get::<_, Option<String>>(34)?
            .unwrap_or_else(|| "unknown".to_string()),
        ip_region: row
            .get::<_, Option<String>>(35)?
            .unwrap_or_else(|| "unknown".to_string()),
        request_headers_json: if include_detail_payload {
            row.get::<_, Option<String>>(37)?
                .unwrap_or_else(|| "{}".to_string())
        } else {
            "{}".to_string()
        },
        last_message_content: row.get(36)?,
        client_request_body_json: if include_detail_payload { row.get(38)? } else { None },
        upstream_request_body_json: if include_detail_payload { row.get(39)? } else { None },
        full_request_json: if include_detail_payload { row.get(40)? } else { None },
        timing: UsageTiming {
            latency_ms: row.get(25)?,
            routing_wait_ms: row.get(26)?,
            upstream_headers_ms: row.get(27)?,
            post_headers_body_ms: row.get(28)?,
            request_body_read_ms: row.get(29)?,
            request_json_parse_ms: row.get(30)?,
            pre_handler_ms: row.get(31)?,
            first_sse_write_ms: row.get(32)?,
            stream_finish_ms: row.get(33)?,
        },
    })
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "duckdb-runtime")]
    use llm_access_core::{
        provider::{ProtocolFamily, ProviderType, RouteStrategy},
        store::{UsageAnalyticsStore, UsageEventQuery, UsageEventSink, UsageEventSource},
        usage::{UsageEvent, UsageTiming},
    };

    #[cfg(feature = "duckdb-runtime")]
    fn test_usage_event() -> UsageEvent {
        UsageEvent {
            event_id: "duckdb-test-event".to_string(),
            created_at_ms: 1_700_000_000_000,
            provider_type: ProviderType::Kiro,
            protocol_family: ProtocolFamily::Anthropic,
            key_id: "key-duckdb".to_string(),
            key_name: "DuckDB Key".to_string(),
            account_name: Some("kiro-account".to_string()),
            account_group_id_at_event: Some("group-duckdb".to_string()),
            route_strategy_at_event: Some(RouteStrategy::Auto),
            request_method: "POST".to_string(),
            request_url: "https://example.test/api/kiro-gateway/cc/v1/messages".to_string(),
            endpoint: "/cc/v1/messages".to_string(),
            model: Some("claude-sonnet-4-5".to_string()),
            mapped_model: Some("claude-sonnet-4-5".to_string()),
            status_code: 200,
            request_body_bytes: Some(1234),
            quota_failover_count: 2,
            routing_diagnostics_json: Some(r#"{"route":"auto"}"#.to_string()),
            input_uncached_tokens: 10,
            input_cached_tokens: 20,
            output_tokens: 30,
            billable_tokens: 40,
            credit_usage: Some("0.5".to_string()),
            usage_missing: false,
            credit_usage_missing: false,
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: r#"{"host":["example.test"]}"#.to_string(),
            last_message_content: Some("hello".to_string()),
            client_request_body_json: Some(r#"{"model":"claude-sonnet-4-5"}"#.to_string()),
            upstream_request_body_json: Some(r#"{"conversationState":{}}"#.to_string()),
            full_request_json: Some(r#"{"model":"claude-sonnet-4-5"}"#.to_string()),
            timing: UsageTiming {
                latency_ms: Some(55),
                routing_wait_ms: Some(5),
                upstream_headers_ms: Some(11),
                post_headers_body_ms: Some(22),
                request_body_read_ms: Some(3),
                request_json_parse_ms: Some(4),
                pre_handler_ms: Some(7),
                first_sse_write_ms: Some(33),
                stream_finish_ms: Some(44),
            },
        }
    }

    #[cfg(feature = "duckdb-runtime")]
    fn assert_usage_event_round_trips(actual: &UsageEvent, expected: &UsageEvent) {
        let actual_credit = actual
            .credit_usage
            .as_deref()
            .and_then(|value| value.parse::<f64>().ok());
        let expected_credit = expected
            .credit_usage
            .as_deref()
            .and_then(|value| value.parse::<f64>().ok());
        assert_eq!(actual_credit, expected_credit);

        let mut actual_without_decimal_format = actual.clone();
        actual_without_decimal_format.credit_usage = expected.credit_usage.clone();
        assert_eq!(actual_without_decimal_format, expected.clone());
    }

    #[cfg(feature = "duckdb-runtime")]
    fn assert_usage_event_summary_round_trips(actual: &UsageEvent, expected: &UsageEvent) {
        let mut expected_summary = expected.clone();
        expected_summary.request_headers_json = "{}".to_string();
        expected_summary.routing_diagnostics_json = None;
        expected_summary.last_message_content = None;
        expected_summary.client_request_body_json = None;
        expected_summary.upstream_request_body_json = None;
        expected_summary.full_request_json = None;
        assert_usage_event_round_trips(actual, &expected_summary);
    }

    #[test]
    fn usage_insert_sql_targets_all_fact_columns_without_runtime_joins() {
        let sql = super::insert_usage_event_sql();
        let lower = sql.to_ascii_lowercase();

        assert!(sql.starts_with("INSERT INTO usage_events"));
        for column in [
            "source_seq",
            "source_event_id",
            "event_id",
            "created_at_ms",
            "provider_type",
            "protocol_family",
            "key_id",
            "key_name",
            "key_status_at_event",
            "account_name",
            "account_group_id_at_event",
            "route_strategy_at_event",
            "endpoint",
            "status_code",
            "upstream_headers_ms",
            "post_headers_body_ms",
            "first_sse_write_ms",
            "stream_finish_ms",
            "input_uncached_tokens",
            "input_cached_tokens",
            "output_tokens",
            "billable_tokens",
            "credit_usage",
            "usage_missing",
            "credit_usage_missing",
        ] {
            assert!(sql.contains(column), "missing column {column}");
        }
        assert!(!lower.contains(" join "));
    }

    #[cfg(feature = "duckdb-runtime")]
    #[tokio::test]
    async fn duckdb_repository_persists_usage_events_with_default_feature() {
        let root = std::env::temp_dir()
            .join(format!("llm-access-duckdb-test-{}-duckdb-repository", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create duckdb test directory");
        let db_path = root.join("usage.duckdb");
        let repo = super::DuckDbUsageRepository::open_path(&db_path).expect("open duckdb usage db");
        let mut event = test_usage_event();
        event.routing_diagnostics_json = Some(r#"{"route":"diagnostic"}"#.to_string());
        event.last_message_content = Some("x".repeat(4096));

        repo.append_usage_event(&event)
            .await
            .expect("append duckdb usage event");

        let page = repo
            .list_usage_events(UsageEventQuery {
                key_id: Some(event.key_id.clone()),
                provider_type: None,
                source: UsageEventSource::All,
                start_ms: None,
                end_ms: None,
                limit: 10,
                offset: 0,
            })
            .await
            .expect("list duckdb usage events");
        assert_eq!(page.total, 1);
        assert_eq!(page.events.len(), 1);
        assert_usage_event_summary_round_trips(&page.events[0], &event);
        assert_eq!(page.events[0].request_headers_json, "{}");
        assert_eq!(page.events[0].routing_diagnostics_json, None);
        assert_eq!(page.events[0].last_message_content, None);
        assert_eq!(page.events[0].client_request_body_json, None);
        assert_eq!(page.events[0].upstream_request_body_json, None);
        assert_eq!(page.events[0].full_request_json, None);

        let detail = repo
            .get_usage_event(&event.event_id)
            .await
            .expect("get duckdb usage event")
            .expect("duckdb usage event exists");
        assert_usage_event_round_trips(&detail, &event);

        let chart = repo
            .usage_chart_points(&event.key_id, event.created_at_ms, 60_000, 1)
            .await
            .expect("query duckdb usage chart");
        assert_eq!(chart.len(), 1);
        assert_eq!(chart[0].bucket_start_ms, event.created_at_ms);
        assert_eq!(chart[0].tokens, 40);

        std::fs::remove_dir_all(&root).expect("cleanup duckdb test directory");
    }

    #[cfg(feature = "duckdb-runtime")]
    #[tokio::test]
    async fn duckdb_repository_persists_usage_event_batches() {
        let root = std::env::temp_dir()
            .join(format!("llm-access-duckdb-test-{}-duckdb-batch-repository", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create duckdb test directory");
        let db_path = root.join("usage.duckdb");
        let repo = super::DuckDbUsageRepository::open_path(&db_path).expect("open duckdb usage db");
        let mut first = test_usage_event();
        first.event_id = "batch-first".to_string();
        let mut second = test_usage_event();
        second.event_id = "batch-second".to_string();
        second.created_at_ms = second.created_at_ms.saturating_add(1);

        repo.append_usage_events(&[first.clone(), second.clone()])
            .await
            .expect("append duckdb usage event batch");

        let page = repo
            .list_usage_events(UsageEventQuery {
                key_id: Some(first.key_id.clone()),
                provider_type: None,
                source: UsageEventSource::All,
                start_ms: None,
                end_ms: None,
                limit: 10,
                offset: 0,
            })
            .await
            .expect("list duckdb usage events");
        assert_eq!(page.total, 2);
        assert_eq!(page.events.len(), 2);
        assert_usage_event_summary_round_trips(&page.events[0], &second);
        assert_usage_event_summary_round_trips(&page.events[1], &first);

        std::fs::remove_dir_all(&root).expect("cleanup duckdb test directory");
    }

    #[cfg(feature = "duckdb-runtime")]
    #[tokio::test]
    async fn duckdb_repository_summarizes_key_usage_rollups() {
        let root = std::env::temp_dir()
            .join(format!("llm-access-duckdb-test-{}-key-rollups", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create duckdb test directory");
        let db_path = root.join("usage.duckdb");
        let repo = super::DuckDbUsageRepository::open_path(&db_path).expect("open duckdb usage db");
        let mut first = test_usage_event();
        first.event_id = "rollup-first".to_string();
        first.created_at_ms = 1_700_000_000_000;
        first.credit_usage = Some("0.5".to_string());
        let mut second = test_usage_event();
        second.event_id = "rollup-second".to_string();
        second.created_at_ms = 1_700_000_060_000;
        second.credit_usage = Some("0.25".to_string());
        second.credit_usage_missing = true;

        repo.append_usage_events(&[first.clone(), second.clone()])
            .await
            .expect("append duckdb usage event batch");

        let rollups = repo
            .key_usage_rollups()
            .await
            .expect("summarize key usage rollups");

        assert_eq!(rollups.len(), 1);
        assert_eq!(rollups[0].key_id, first.key_id);
        assert_eq!(rollups[0].input_uncached_tokens, 20);
        assert_eq!(rollups[0].input_cached_tokens, 40);
        assert_eq!(rollups[0].output_tokens, 60);
        assert_eq!(rollups[0].billable_tokens, 80);
        assert_eq!(rollups[0].credit_total, "0.75");
        assert_eq!(rollups[0].credit_missing_events, 1);
        assert_eq!(rollups[0].last_used_at_ms, Some(second.created_at_ms));

        std::fs::remove_dir_all(&root).expect("cleanup duckdb test directory");
    }

    #[cfg(feature = "duckdb-runtime")]
    #[tokio::test]
    async fn duckdb_repository_lists_usage_events_newest_first_from_append_order() {
        let root = std::env::temp_dir()
            .join(format!("llm-access-duckdb-test-{}-append-order", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create duckdb test directory");
        let db_path = root.join("usage.duckdb");
        let repo = super::DuckDbUsageRepository::open_path(&db_path).expect("open duckdb usage db");
        let mut first = test_usage_event();
        first.event_id = "append-first".to_string();
        first.created_at_ms = 1_700_000_000_000;
        let mut second = test_usage_event();
        second.event_id = "append-second".to_string();
        second.created_at_ms = 1_700_000_060_000;

        repo.append_usage_event(&first)
            .await
            .expect("append first duckdb usage event");
        repo.append_usage_event(&second)
            .await
            .expect("append second duckdb usage event");

        let first_page = repo
            .list_usage_events(UsageEventQuery {
                key_id: Some(first.key_id.clone()),
                provider_type: None,
                source: UsageEventSource::All,
                start_ms: None,
                end_ms: None,
                limit: 1,
                offset: 0,
            })
            .await
            .expect("list first page");
        assert_eq!(first_page.total, 2);
        assert_eq!(first_page.offset, 0);
        assert_eq!(first_page.limit, 1);
        assert!(first_page.has_more);
        assert_eq!(first_page.events[0].event_id, second.event_id);

        let second_page = repo
            .list_usage_events(UsageEventQuery {
                key_id: Some(first.key_id.clone()),
                provider_type: None,
                source: UsageEventSource::All,
                start_ms: None,
                end_ms: None,
                limit: 1,
                offset: 1,
            })
            .await
            .expect("list second page");
        assert_eq!(second_page.total, 2);
        assert_eq!(second_page.offset, 1);
        assert_eq!(second_page.limit, 1);
        assert!(!second_page.has_more);
        assert_eq!(second_page.events[0].event_id, first.event_id);

        let time_filtered_page = repo
            .list_usage_events(UsageEventQuery {
                key_id: Some(first.key_id.clone()),
                provider_type: None,
                source: UsageEventSource::All,
                start_ms: Some(second.created_at_ms),
                end_ms: Some(second.created_at_ms.saturating_add(1)),
                limit: 10,
                offset: 0,
            })
            .await
            .expect("list time-filtered page");
        assert_eq!(time_filtered_page.total, 1);
        assert_eq!(time_filtered_page.events.len(), 1);
        assert_eq!(time_filtered_page.events[0].event_id, second.event_id);

        std::fs::remove_dir_all(&root).expect("cleanup duckdb test directory");
    }

    #[cfg(feature = "duckdb-runtime")]
    #[tokio::test]
    async fn duckdb_repository_clamps_online_usage_event_pages() {
        let root = std::env::temp_dir()
            .join(format!("llm-access-duckdb-test-{}-online-page-clamp", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create duckdb test directory");
        let db_path = root.join("usage.duckdb");
        let repo = super::DuckDbUsageRepository::open_path(&db_path).expect("open duckdb usage db");

        for index in 0..25 {
            let mut event = test_usage_event();
            event.event_id = format!("online-clamp-{index:02}");
            event.created_at_ms = 1_700_000_000_000 + i64::from(index);
            repo.append_usage_event(&event)
                .await
                .expect("append duckdb usage event");
        }

        let page = repo
            .list_usage_events(UsageEventQuery {
                key_id: None,
                provider_type: None,
                source: UsageEventSource::All,
                start_ms: None,
                end_ms: None,
                limit: 100,
                offset: 1_000,
            })
            .await
            .expect("list clamped page");

        assert_eq!(page.limit, super::USAGE_EVENT_ONLINE_MAX_LIMIT);
        assert_eq!(page.offset, super::USAGE_EVENT_ONLINE_MAX_OFFSET);
        assert_eq!(page.total, 25);
        assert!(page.events.is_empty());
        assert!(!page.has_more);

        let first_page = repo
            .list_usage_events(UsageEventQuery {
                key_id: None,
                provider_type: None,
                source: UsageEventSource::All,
                start_ms: None,
                end_ms: None,
                limit: 100,
                offset: 0,
            })
            .await
            .expect("list first clamped page");
        assert_eq!(first_page.limit, super::USAGE_EVENT_ONLINE_MAX_LIMIT);
        assert_eq!(first_page.total, 25);
        assert_eq!(first_page.events.len(), super::USAGE_EVENT_ONLINE_MAX_LIMIT);
        assert!(first_page.has_more);
        assert_eq!(first_page.events[0].event_id, "online-clamp-24");

        std::fs::remove_dir_all(&root).expect("cleanup duckdb test directory");
    }

    #[cfg(feature = "duckdb-runtime")]
    #[test]
    fn tiered_usage_page_plan_skips_whole_sources_and_fetches_only_page_rows() {
        let plan = super::plan_tiered_usage_page_fetches([50, 80, 80], 55, 20);

        assert_eq!(plan, vec![super::TieredUsagePageFetch {
            partition_index: 1,
            local_newest_offset: 5,
            limit: 20,
        }]);

        let cross_partition_plan = super::plan_tiered_usage_page_fetches([5, 10], 3, 10);
        assert_eq!(cross_partition_plan, vec![
            super::TieredUsagePageFetch {
                partition_index: 0,
                local_newest_offset: 3,
                limit: 2,
            },
            super::TieredUsagePageFetch {
                partition_index: 1,
                local_newest_offset: 0,
                limit: 8,
            },
        ]);
    }

    #[cfg(feature = "duckdb-runtime")]
    #[tokio::test]
    async fn duckdb_tiered_repository_rolls_over_without_blocking_active_appends() {
        let root = std::env::temp_dir()
            .join(format!("llm-access-duckdb-test-{}-tiered-rollover", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create tiered duckdb test directory");
        let repo = super::DuckDbUsageRepository::open_tiered(super::TieredDuckDbUsageConfig {
            active_dir: root.join("active"),
            archive_dir: root.join("archive"),
            catalog_dir: root.join("catalog"),
            rollover_bytes: 1,
        })
        .expect("open tiered duckdb usage db");
        let mut first = test_usage_event();
        first.event_id = "tiered-archived-first".to_string();
        first.created_at_ms = 1_700_000_000_000;
        first.last_message_content = Some("archived detail".repeat(128));
        let mut second = test_usage_event();
        second.event_id = "tiered-active-second".to_string();
        second.created_at_ms = 1_700_000_060_000;

        repo.append_usage_event(&first)
            .await
            .expect("append first tiered usage event");
        repo.append_usage_event(&second)
            .await
            .expect("append second tiered usage event after rollover");

        wait_for_archived_duckdb_file_count(&root.join("archive"), 2).await;

        let page = repo
            .list_usage_events(UsageEventQuery {
                key_id: Some(first.key_id.clone()),
                provider_type: None,
                source: UsageEventSource::All,
                start_ms: None,
                end_ms: None,
                limit: 10,
                offset: 0,
            })
            .await
            .expect("list tiered usage events");
        assert_eq!(page.total, 2);
        assert_eq!(page.events.len(), 2);
        assert_eq!(page.events[0].event_id, second.event_id);
        assert_eq!(page.events[1].event_id, first.event_id);

        let second_page = repo
            .list_usage_events(UsageEventQuery {
                key_id: Some(first.key_id.clone()),
                provider_type: None,
                source: UsageEventSource::All,
                start_ms: None,
                end_ms: None,
                limit: 1,
                offset: 1,
            })
            .await
            .expect("list second tiered usage page");
        assert_eq!(second_page.total, 2);
        assert_eq!(second_page.events.len(), 1);
        assert_eq!(second_page.events[0].event_id, first.event_id);
        assert!(!second_page.has_more);

        let archived_detail = repo
            .get_usage_event(&first.event_id)
            .await
            .expect("get archived tiered usage event")
            .expect("archived tiered event exists");
        assert_usage_event_round_trips(&archived_detail, &first);

        std::fs::remove_dir_all(&root).expect("cleanup tiered duckdb test directory");
    }

    #[cfg(feature = "duckdb-runtime")]
    #[test]
    fn duckdb_tiered_publish_rewrites_segment_with_current_schema() {
        let root = std::env::temp_dir()
            .join(format!("llm-access-duckdb-test-{}-compact-publish", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create compact publish test directory");
        let config = super::TieredDuckDbUsageConfig {
            active_dir: root.join("active"),
            archive_dir: root.join("archive"),
            catalog_dir: root.join("catalog"),
            rollover_bytes: 1,
        };
        super::initialize_tiered_catalog(&config).expect("initialize tiered catalog");

        let pending_path = root.join("pending-source.duckdb");
        {
            let conn = duckdb::Connection::open(&pending_path).expect("open pending source");
            crate::initialize_duckdb_target(&conn).expect("initialize pending source");
            let mut writer = super::DuckDbUsageWriter::new(conn).expect("open pending writer");
            let mut event = test_usage_event();
            event.event_id = "compact-publish-event".to_string();
            event.created_at_ms = 1_700_000_000_000;
            writer
                .insert_usage_events(&[super::UsageEventRow::from_usage_event(&event)])
                .expect("insert pending event");
        }
        {
            let conn = duckdb::Connection::open(&pending_path).expect("reopen pending source");
            conn.execute_batch(
                "
                CREATE INDEX IF NOT EXISTS idx_usage_events_created_date
                    ON usage_events(created_at_ms);
                CHECKPOINT;
                ",
            )
            .expect("create legacy source index");
        }

        super::publish_pending_segment(&config, &pending_path, "usage-compact-test-000001")
            .expect("publish compacted segment");

        let archive_path = config.archive_dir.join("usage-compact-test-000001.duckdb");
        assert!(archive_path.exists(), "archived compact segment should exist");
        assert!(
            !pending_path.exists(),
            "pending segment should be removed only after catalog publication"
        );
        assert!(
            !super::compacting_segment_path(&config, "usage-compact-test-000001").exists(),
            "local compact temp file should be removed after publication"
        );
        assert!(
            !config
                .archive_dir
                .join("usage-compact-test-000001.uploading.duckdb")
                .exists(),
            "uploading archive temp file should not remain after publication"
        );
        let stale_compact_path =
            super::compacting_segment_path(&config, "usage-compact-test-000001");
        std::fs::write(&stale_compact_path, b"stale compact retry")
            .expect("write stale compact retry file");
        super::publish_pending_segment(&config, &pending_path, "usage-compact-test-000001")
            .expect("published segment finalization is idempotent");
        assert!(
            !stale_compact_path.exists(),
            "idempotent finalization should remove stale compact retry files"
        );

        let archived = super::DuckDbUsageRepository::open_read_only_conn(&archive_path)
            .expect("open archived compact segment");
        let indexes = archived
            .prepare("SELECT index_name FROM duckdb_indexes() ORDER BY index_name")
            .expect("prepare index query")
            .query_map([], |row| row.get::<_, String>(0))
            .expect("query indexes")
            .collect::<Result<Vec<_>, _>>()
            .expect("read indexes");
        assert!(
            indexes.is_empty(),
            "archive should be rewritten with current schema and no legacy explicit indexes: \
             {indexes:?}"
        );

        let count: i64 = archived
            .query_row("SELECT CAST(count(*) AS BIGINT) FROM usage_events", [], |row| row.get(0))
            .expect("count archived rows");
        assert_eq!(count, 1);

        std::fs::remove_dir_all(&root).expect("cleanup compact publish test directory");
    }

    #[cfg(feature = "duckdb-runtime")]
    async fn wait_for_archived_duckdb_file_count(archive_dir: &std::path::Path, expected: usize) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let archived = std::fs::read_dir(archive_dir)
                .ok()
                .into_iter()
                .flat_map(|entries| entries.filter_map(Result::ok))
                .filter(|entry| {
                    entry.path().extension().and_then(|ext| ext.to_str()) == Some("duckdb")
                })
                .count();
            if archived >= expected {
                return;
            }
            if std::time::Instant::now() >= deadline {
                panic!("timed out waiting for archived duckdb segment");
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    }

    #[cfg(feature = "duckdb-runtime")]
    #[test]
    fn duckdb_initialization_drops_legacy_usage_art_indexes() {
        let root = std::env::temp_dir()
            .join(format!("llm-access-duckdb-test-{}-drop-indexes", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create duckdb test directory");
        let db_path = root.join("usage.duckdb");
        let conn = duckdb::Connection::open(&db_path).expect("open duckdb");

        crate::initialize_duckdb_target(&conn).expect("initialize duckdb");
        conn.execute_batch(
            "
            CREATE UNIQUE INDEX IF NOT EXISTS idx_usage_events_source_event_id
                ON usage_events(source_event_id);
            CREATE INDEX IF NOT EXISTS idx_usage_events_source_seq
                ON usage_events(source_seq);
            CREATE INDEX IF NOT EXISTS idx_usage_events_created_date
                ON usage_events(created_date);
            CREATE INDEX IF NOT EXISTS idx_usage_events_key_date
                ON usage_events(key_id, created_date);
            CREATE INDEX IF NOT EXISTS idx_usage_events_provider_date
                ON usage_events(provider_type, created_date);
            ",
        )
        .expect("create legacy indexes");

        crate::initialize_duckdb_target(&conn).expect("reinitialize duckdb");
        let mut stmt = conn
            .prepare("SELECT index_name FROM duckdb_indexes() ORDER BY index_name")
            .expect("prepare index query");
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .expect("query indexes");
        let indexes = rows.collect::<Result<Vec<_>, _>>().expect("read indexes");

        assert!(
            indexes.is_empty(),
            "only implicit primary key constraints should remain, found explicit indexes: \
             {indexes:?}"
        );

        std::fs::remove_dir_all(&root).expect("cleanup duckdb test directory");
    }
}
