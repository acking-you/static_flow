//! DuckDB analytics writer helpers for LLM usage events.

#[cfg(all(test, feature = "duckdb-runtime"))]
use std::fs;
#[cfg(feature = "duckdb-runtime")]
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
    path::PathBuf,
    sync::{Arc, Mutex, RwLock},
};

#[cfg(all(test, feature = "duckdb-runtime"))]
use anyhow::Context;
#[cfg(feature = "duckdb-runtime")]
use llm_access_core::store::UsageEventTotals;
#[cfg(feature = "duckdb-runtime")]
use serde::{Deserialize, Serialize};

#[cfg(feature = "duckdb-runtime")]
use crate::usage_catalog::{
    PostgresUsageCatalog, UsageCatalogFieldName, UsageCatalogFieldRollupRecord,
    UsageCatalogKeyRollupRecord, UsageCatalogSegmentRecord,
};

#[cfg(feature = "duckdb-runtime")]
mod append;
#[cfg(feature = "duckdb-runtime")]
mod catalog;
#[cfg(feature = "duckdb-runtime")]
mod connection;
#[cfg(feature = "duckdb-runtime")]
mod filter_options;
#[cfg(feature = "duckdb-runtime")]
mod metrics;
#[cfg(feature = "duckdb-runtime")]
mod query;
#[cfg(feature = "duckdb-runtime")]
mod repository;
#[cfg(feature = "duckdb-runtime")]
mod retention;
#[cfg(feature = "duckdb-runtime")]
mod segment;
#[cfg(feature = "duckdb-runtime")]
mod sql;
#[cfg(feature = "duckdb-runtime")]
mod util;
#[cfg(feature = "duckdb-runtime")]
mod writer;

#[cfg(all(test, feature = "duckdb-runtime"))]
use connection::duckdb_usage_connection_sql;
#[cfg(feature = "duckdb-runtime")]
pub use connection::initialize_duckdb_target_path;
#[cfg(all(test, feature = "duckdb-runtime"))]
use query::plan_tiered_usage_page_fetches;
#[cfg(all(test, feature = "duckdb-runtime"))]
use retention::{
    duckdb_wal_path, prune_expired_detail_day_buckets, usage_analytics_retention_cutoff_ms,
};
#[cfg(all(test, feature = "duckdb-runtime"))]
use segment::{
    archive_segment_bucket_dir, archive_segment_path_for_timestamp, collect_files_recursive,
    collect_segment_event_ids, collect_segment_stats, compacting_segment_path,
    publish_pending_segment_async, publish_segment_catalog, test_catalog_state_path,
    uploading_archive_segment_path_from_archive_path,
};
#[cfg(all(test, feature = "duckdb-runtime"))]
use sql::duckdb_compact_connection_sql;

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
    /// Whether the downstream stream finished cleanly.
    pub stream_completed_cleanly: Option<bool>,
    /// Whether the downstream stream disconnected before completion.
    pub downstream_disconnect: Option<bool>,
    /// Last downstream SSE event type when known.
    pub final_event_type: Option<String>,
    /// Total downstream SSE bytes emitted by the gateway.
    pub bytes_streamed: Option<i64>,
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
    /// Effective proxy source captured at event time.
    pub proxy_source_at_event: Option<String>,
    /// Effective proxy config id captured at event time.
    pub proxy_config_id_at_event: Option<String>,
    /// Effective proxy config name captured at event time.
    pub proxy_config_name_at_event: Option<String>,
    /// Effective proxy URL captured at event time.
    pub proxy_url_at_event: Option<String>,
    /// Client request body JSON when captured.
    pub client_request_body_json: Option<String>,
    /// Upstream request body JSON when captured.
    pub upstream_request_body_json: Option<String>,
    /// Full request JSON when captured.
    pub full_request_json: Option<String>,
    /// Best-effort error message surfaced for failed requests.
    pub error_message: Option<String>,
    /// Raw error response body surfaced for failed requests.
    pub error_body: Option<String>,
    /// Whether heavyweight request payload details were externalized.
    pub detail_object_payload_present: bool,
    /// External detail pack object path relative to the configured detail root.
    pub detail_object_path: Option<String>,
    /// Byte offset inside the external detail pack.
    pub detail_object_offset: Option<i64>,
    /// Byte length inside the external detail pack.
    pub detail_object_length: Option<i64>,
    /// SHA-256 of the compressed detail member.
    pub detail_object_sha256: Option<String>,
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
            stream_completed_cleanly: event.stream.stream_completed_cleanly,
            downstream_disconnect: event.stream.downstream_disconnect,
            final_event_type: event.stream.final_event_type.clone(),
            bytes_streamed: event.stream.bytes_streamed,
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
            proxy_source_at_event: None,
            proxy_config_id_at_event: None,
            proxy_config_name_at_event: None,
            proxy_url_at_event: None,
            client_request_body_json: event.client_request_body_json.clone(),
            upstream_request_body_json: event.upstream_request_body_json.clone(),
            full_request_json: event.full_request_json.clone(),
            error_message: event.error_message.clone(),
            error_body: event.error_body.clone(),
            detail_object_payload_present: has_external_detail_payloads(
                event.client_request_body_json.as_deref(),
                event.upstream_request_body_json.as_deref(),
                event.full_request_json.as_deref(),
                event.error_body.as_deref(),
            ),
            detail_object_path: None,
            detail_object_offset: None,
            detail_object_length: None,
            detail_object_sha256: None,
        }
    }

    /// Apply worker-time proxy attribution metadata to one fact row.
    pub fn with_proxy_attribution(
        mut self,
        attribution: Option<&crate::postgres::UsageProxyAttribution>,
    ) -> Self {
        if let Some(attribution) = attribution {
            self.proxy_source_at_event = Some(attribution.proxy_source.clone());
            self.proxy_config_id_at_event = attribution.proxy_config_id.clone();
            self.proxy_config_name_at_event = attribution.proxy_config_name.clone();
            self.proxy_url_at_event = attribution.proxy_url.clone();
        }
        self
    }
}

fn has_external_detail_payloads(
    client_request_body_json: Option<&str>,
    upstream_request_body_json: Option<&str>,
    full_request_json: Option<&str>,
    error_body: Option<&str>,
) -> bool {
    [client_request_body_json, upstream_request_body_json, full_request_json, error_body]
        .into_iter()
        .flatten()
        .any(|value| !value.trim().is_empty())
}

/// Return the insert statement for the DuckDB `usage_events` fact table.
pub fn insert_usage_event_sql() -> &'static str {
    "INSERT INTO usage_events (
        source_seq, source_event_id, event_id, created_at_ms, created_at,
        created_date, created_hour, provider_type, protocol_family, key_id,
        key_name, key_status_at_event, account_name, account_group_id_at_event,
        route_strategy_at_event, request_method, request_url, endpoint, model,
        mapped_model, status_code, latency_ms, routing_wait_ms,
        upstream_headers_ms, post_headers_body_ms, request_body_read_ms,
        request_json_parse_ms, pre_handler_ms, first_sse_write_ms,
        stream_finish_ms, stream_completed_cleanly, downstream_disconnect,
        final_event_type, bytes_streamed, request_body_bytes,
        quota_failover_count, input_uncached_tokens, input_cached_tokens,
        output_tokens, billable_tokens, credit_usage, usage_missing,
        credit_usage_missing, client_ip, ip_region, request_headers_json,
        routing_diagnostics_json, last_message_content, detail_object_payload_present,
        detail_object_path, detail_object_offset, detail_object_length, detail_object_sha256,
        proxy_source_at_event, proxy_config_id_at_event, proxy_config_name_at_event,
        proxy_url_at_event
     ) VALUES (
        ?1, ?2, ?3, ?4, to_timestamp(?4 / 1000.0),
        CAST(to_timestamp(?4 / 1000.0) AS DATE),
        date_trunc('hour', to_timestamp(?4 / 1000.0)),
        ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18,
        ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31,
        ?32, ?33, ?34, ?35, ?36, ?37, ?38, ?39, ?40, ?41, ?42, ?43, ?44, ?45, ?46,
        ?47, ?48, ?49, ?50, ?51, ?52, ?53, ?54
     )
     ON CONFLICT DO NOTHING"
}

#[cfg(feature = "duckdb-runtime")]
const COMPACT_COPY_USAGE_ROLLUPS_HOURLY_SQL: &str = "
    INSERT INTO usage_rollups_hourly (
        bucket_hour, provider_type, protocol_family, key_id, key_name,
        account_name, account_group_id_at_event, route_strategy_at_event,
        endpoint, model, mapped_model, status_code_class, request_count,
        input_uncached_tokens, input_cached_tokens, output_tokens,
        billable_tokens, credit_usage, credit_usage_missing_count,
        avg_latency_ms, max_latency_ms, p95_latency_ms
    )
    SELECT
        bucket_hour, provider_type, protocol_family, key_id, key_name,
        account_name, account_group_id_at_event, route_strategy_at_event,
        endpoint, model, mapped_model, status_code_class, request_count,
        input_uncached_tokens, input_cached_tokens, output_tokens,
        billable_tokens, credit_usage, credit_usage_missing_count,
        avg_latency_ms, max_latency_ms, p95_latency_ms
    FROM pending_segment.usage_rollups_hourly;
";

#[cfg(feature = "duckdb-runtime")]
const COMPACT_COPY_USAGE_ROLLUPS_DAILY_SQL: &str = "
    INSERT INTO usage_rollups_daily (
        bucket_date, provider_type, protocol_family, key_id, key_name,
        account_name, account_group_id_at_event, route_strategy_at_event,
        endpoint, model, mapped_model, status_code_class, request_count,
        input_uncached_tokens, input_cached_tokens, output_tokens,
        billable_tokens, credit_usage, credit_usage_missing_count,
        avg_latency_ms, max_latency_ms, p95_latency_ms
    )
    SELECT
        bucket_date, provider_type, protocol_family, key_id, key_name,
        account_name, account_group_id_at_event, route_strategy_at_event,
        endpoint, model, mapped_model, status_code_class, request_count,
        input_uncached_tokens, input_cached_tokens, output_tokens,
        billable_tokens, credit_usage, credit_usage_missing_count,
        avg_latency_ms, max_latency_ms, p95_latency_ms
    FROM pending_segment.usage_rollups_daily;
";

#[cfg(feature = "duckdb-runtime")]
const USAGE_EVENT_PAGE_MAX_LIMIT: usize = 200;

/// DuckDB usage writer.
#[cfg(feature = "duckdb-runtime")]
#[derive(Debug)]
pub struct DuckDbUsageWriter {
    conn: duckdb::Connection,
}

#[cfg(feature = "duckdb-runtime")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct UsageEventDetailRow {
    event_id: String,
    request_headers_json: String,
    routing_diagnostics_json: Option<String>,
    last_message_content: Option<String>,
    client_request_body_json: Option<String>,
    upstream_request_body_json: Option<String>,
    full_request_json: Option<String>,
    error_message: Option<String>,
    error_body: Option<String>,
}

#[cfg(feature = "duckdb-runtime")]
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct UsageEventDetailBlob {
    request_headers_json: String,
    routing_diagnostics_json: Option<String>,
    last_message_content: Option<String>,
    client_request_body_json: Option<String>,
    upstream_request_body_json: Option<String>,
    full_request_json: Option<String>,
    error_message: Option<String>,
    error_body: Option<String>,
}

#[cfg(feature = "duckdb-runtime")]
#[derive(Debug)]
struct UsageEventDetailPackWrite {
    relative_path: String,
    bytes: Vec<u8>,
}

#[cfg(feature = "duckdb-runtime")]
#[derive(Debug, Clone)]
struct UsageEventDetailObjectRef {
    relative_path: String,
    byte_range: Range<u64>,
    sha256: String,
}

#[cfg(feature = "duckdb-runtime")]
#[derive(Debug, Clone)]
struct UsageEventDetailStore {
    root_dir: PathBuf,
}

#[cfg(feature = "duckdb-runtime")]
#[derive(Debug)]
struct HotUsageWriter {
    summary: DuckDbUsageWriter,
    detail_store: Option<Arc<UsageEventDetailStore>>,
}

#[cfg(feature = "duckdb-runtime")]
#[derive(Debug)]
struct PersistentUsageWriter {
    writer: HotUsageWriter,
    connection_config: DuckDbUsageConnectionConfig,
}

/// File-backed DuckDB usage-event repository.
#[cfg(feature = "duckdb-runtime")]
#[derive(Debug, Clone)]
pub struct DuckDbUsageRepository {
    inner: Arc<DuckDbUsageRepositoryInner>,
}

/// Summary of one usage analytics retention pass.
#[cfg(feature = "duckdb-runtime")]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UsageAnalyticsPruneReport {
    /// Archived segments removed from the catalog.
    pub deleted_segments: usize,
    /// Catalog-referenced DuckDB files removed from disk.
    pub deleted_files: usize,
    /// DuckDB files removed because no catalog row referenced them.
    pub deleted_orphan_files: usize,
    /// Detail pack files removed from expired day buckets.
    pub deleted_detail_files: usize,
    /// Detail directories removed from expired day buckets or empty archive
    /// buckets.
    pub deleted_detail_dirs: usize,
}

#[cfg(feature = "duckdb-runtime")]
#[derive(Debug)]
enum DuckDbUsageRepositoryInner {
    Single {
        state: Box<Mutex<SingleDuckDbUsageState>>,
        connection_config: SharedDuckDbUsageConnectionConfig,
    },
    Tiered {
        config: TieredDuckDbUsageConfig,
        state: Box<Mutex<TieredDuckDbUsageState>>,
        connection_config: SharedDuckDbUsageConnectionConfig,
        catalog_backend: Arc<TieredUsageCatalogBackend>,
    },
}

#[cfg(feature = "duckdb-runtime")]
type SharedDuckDbUsageConnectionConfig = Arc<RwLock<DuckDbUsageConnectionConfig>>;

/// Runtime-tunable DuckDB connection settings for usage analytics writes.
#[cfg(feature = "duckdb-runtime")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DuckDbUsageConnectionConfig {
    /// DuckDB buffer-manager memory limit in MiB.
    pub memory_limit_mib: u64,
    /// WAL size threshold for automatic checkpoints in MiB.
    pub checkpoint_threshold_mib: u64,
}

/// Tiered DuckDB usage storage configuration.
#[cfg(feature = "duckdb-runtime")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TieredDuckDbUsageConfig {
    /// Local directory for the current writable DuckDB file.
    pub active_dir: PathBuf,
    /// JuiceFS-backed directory for immutable archived DuckDB segments.
    pub archive_dir: PathBuf,
    /// Rollover threshold in bytes for the active DuckDB file.
    pub rollover_bytes: u64,
    /// Optional local root directory for packed detail payloads.
    pub details_dir: Option<PathBuf>,
}

#[cfg(feature = "duckdb-runtime")]
#[derive(Debug)]
struct TieredDuckDbUsageState {
    active_path: PathBuf,
    next_sequence: u64,
    active_has_rows: bool,
    active_writer: Option<PersistentUsageWriter>,
    detail_store: Option<Arc<UsageEventDetailStore>>,
    /// Serializes the append write path against retention's active-segment
    /// rollover/discard. A retention cycle must not delete or roll the active
    /// segment while an append holds its writer across the insert `.await` —
    /// otherwise the in-flight writer is orphaned onto a deleted/rolled segment
    /// and its rows (and all subsequent appends via the stale writer) are lost.
    write_gate: Arc<tokio::sync::Mutex<()>>,
    /// Test-only deterministic interleaving hook: when set, an append parks
    /// (still holding `write_gate`) right after acquiring the gate, letting a
    /// test observe whether retention is serialized behind it.
    #[cfg(test)]
    append_seam: Option<AppendSeam>,
}

/// Test-only one-shot handshake used to park an in-flight append at a known
/// point while it holds the tiered `write_gate`.
#[cfg(test)]
#[derive(Debug)]
struct AppendSeam {
    reached: tokio::sync::oneshot::Sender<()>,
    proceed: tokio::sync::oneshot::Receiver<()>,
}

#[cfg(feature = "duckdb-runtime")]
#[derive(Debug)]
struct SingleDuckDbUsageState {
    path: PathBuf,
    writer: Option<PersistentUsageWriter>,
}

#[cfg(feature = "duckdb-runtime")]
#[derive(Debug, Clone)]
struct ArchivedUsageSegment {
    archive_path: PathBuf,
    start_ms: Option<i64>,
    end_ms: Option<i64>,
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
    totals: UsageEventTotals,
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
    first_used_at_ms: Option<i64>,
    last_used_at_ms: Option<i64>,
}

#[cfg(feature = "duckdb-runtime")]
#[derive(Debug, Clone)]
struct SegmentFieldRollup {
    key_id: Option<String>,
    provider_type: Option<String>,
    field_name: UsageCatalogFieldName,
    field_value: String,
    row_count: usize,
    input_uncached_tokens: i64,
    input_cached_tokens: i64,
    output_tokens: i64,
    billable_tokens: i64,
    first_used_at_ms: Option<i64>,
    last_used_at_ms: Option<i64>,
}

#[cfg(feature = "duckdb-runtime")]
#[derive(Debug)]
struct SegmentStats {
    start_ms: Option<i64>,
    end_ms: Option<i64>,
    row_count: usize,
    event_id_count: usize,
    input_uncached_tokens: i64,
    input_cached_tokens: i64,
    output_tokens: i64,
    billable_tokens: i64,
    rollups: Vec<SegmentKeyRollup>,
    field_rollups: Vec<SegmentFieldRollup>,
}

#[cfg(feature = "duckdb-runtime")]
#[derive(Debug, Clone)]
struct ArchivedSegmentPaths {
    pending_duckdb: PathBuf,
    compact_duckdb: PathBuf,
    uploading_duckdb: PathBuf,
    archive_duckdb: PathBuf,
}

#[cfg(feature = "duckdb-runtime")]
#[derive(Debug)]
enum TieredUsageCatalogBackend {
    Postgres(Arc<PostgresUsageCatalog>),
    Test(Arc<TestTieredUsageCatalog>),
}

#[cfg(feature = "duckdb-runtime")]
#[derive(Debug)]
struct TestTieredUsageCatalog {
    path: PathBuf,
    state: Mutex<TestTieredUsageCatalogState>,
}

#[cfg(feature = "duckdb-runtime")]
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct TestTieredUsageCatalogState {
    segments: BTreeMap<String, UsageCatalogSegmentRecord>,
    segment_rollups: BTreeMap<String, Vec<UsageCatalogKeyRollupRecord>>,
    segment_field_rollups: BTreeMap<String, Vec<UsageCatalogFieldRollupRecord>>,
    event_locators: BTreeMap<String, String>,
}

#[cfg(feature = "duckdb-runtime")]
const DUCKDB_COMPACT_MAX_TEMP_DIRECTORY_SIZE: &str = "8GB";

#[cfg(feature = "duckdb-runtime")]
const DUCKDB_USAGE_CONNECTION_MAX_TEMP_DIRECTORY_SIZE: &str = "2GB";

fn tiered_pending_dir(config: &TieredDuckDbUsageConfig) -> PathBuf {
    config.active_dir.join("pending")
}

#[cfg(all(test, feature = "duckdb-runtime"))]
fn initialize_tiered_catalog(config: &TieredDuckDbUsageConfig) -> anyhow::Result<()> {
    fs::create_dir_all(&config.active_dir).with_context(|| {
        format!("failed to create tiered active directory `{}`", config.active_dir.display())
    })?;
    fs::create_dir_all(&config.archive_dir).with_context(|| {
        format!("failed to create tiered archive directory `{}`", config.archive_dir.display())
    })?;
    Ok(())
}

#[cfg(feature = "duckdb-runtime")]
const USAGE_ANALYTICS_RETENTION_DAY_MS: i64 = 86_400_000;

#[cfg(feature = "duckdb-runtime")]
#[derive(Debug)]
struct RetentionSegmentCandidate {
    archive_path: PathBuf,
}

#[cfg(feature = "duckdb-runtime")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UsageFilterOptionField {
    Model,
    Account,
    Endpoint,
}

#[cfg(feature = "duckdb-runtime")]
#[derive(Default)]
struct UsageMetricsSummaryAccumulator {
    total_requests: u64,
    ok_requests: u64,
    non_ok_requests: u64,
    first_token_sum_ms: i64,
    first_token_samples: u64,
    max_first_token_ms: Option<i64>,
    latency_sum_ms: i64,
    latency_samples: u64,
    routing_wait_sum_ms: i64,
    routing_wait_samples: u64,
    failover_request_count: u64,
    total_quota_failovers: u64,
    downstream_disconnect_count: u64,
    usage_missing_count: u64,
    credit_usage_missing_count: u64,
}

#[cfg(feature = "duckdb-runtime")]
#[derive(Clone, Default)]
struct UsageMetricsGroupAccumulator {
    key: String,
    label: String,
    account_name: Option<String>,
    proxy_config_id: Option<String>,
    proxy_config_name: Option<String>,
    proxy_url: Option<String>,
    proxy_source: Option<String>,
    request_count: u64,
    ok_count: u64,
    non_ok_count: u64,
    first_token_sum_ms: i64,
    first_token_samples: u64,
    max_first_token_ms: Option<i64>,
    routing_wait_sum_ms: i64,
    routing_wait_samples: u64,
    max_routing_wait_ms: Option<i64>,
    failover_request_count: u64,
    total_quota_failovers: u64,
    downstream_disconnect_count: u64,
    usage_missing_count: u64,
    credit_usage_missing_count: u64,
}

#[cfg(feature = "duckdb-runtime")]
#[derive(Default)]
struct UsageMetricsAccumulator {
    summary: UsageMetricsSummaryAccumulator,
    distinct_accounts: BTreeSet<String>,
    distinct_proxies: BTreeSet<String>,
    accounts: BTreeMap<String, UsageMetricsGroupAccumulator>,
    proxies: BTreeMap<String, UsageMetricsGroupAccumulator>,
    non_ok_status_codes: BTreeMap<i32, u64>,
}

#[cfg(feature = "duckdb-runtime")]
struct UsageMetricsObservedRow {
    account_name: Option<String>,
    status_code: i32,
    first_sse_write_ms: Option<i64>,
    latency_ms: Option<i64>,
    routing_wait_ms: Option<i64>,
    quota_failover_count: u64,
    downstream_disconnect: bool,
    usage_missing: bool,
    credit_usage_missing: bool,
    proxy_source: Option<String>,
    proxy_config_id: Option<String>,
    proxy_config_name: Option<String>,
    proxy_url: Option<String>,
}

#[cfg(test)]
mod tests;
