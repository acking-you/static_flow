//! DuckDB analytics writer helpers for LLM usage events.
//! ## Module map
//!
//! `duckdb.rs` is the facade for the DuckDB usage-analytics writer. It keeps
//! the row/writer/repository/config types and their `impl` blocks (including
//! the `UsageEventSink` and `UsageAnalyticsStore` trait impls) and the
//! `DuckDbUsageRepository` itself; the pure free functions are grouped by
//! concern into descendant submodules. Every item stays `#[cfg(feature =
//! "duckdb-runtime")]`-gated as before.
//!
//! ```text
//!  UsageEvent(s)  --write-->  DuckDbUsageRepository (parent: types + impls)
//!    write path : [sql] [detail] [connection] [rollup]
//!    tiering    : [catalog] [segment] [prune]
//!    read path  : [query] [filter_options] [metrics] [latency]
//!    shared     : [util]
//! ```

#[cfg(feature = "duckdb-runtime")]
pub(crate) use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    fs,
    io::{Read, Seek, SeekFrom, Write},
    ops::Range,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[cfg(feature = "duckdb-runtime")]
pub(crate) use anyhow::{anyhow, Context};
#[cfg(feature = "duckdb-runtime")]
pub(crate) use async_trait::async_trait;
#[cfg(feature = "duckdb-runtime")]
pub(crate) use duckdb::OptionalExt;
#[cfg(feature = "duckdb-runtime")]
pub(crate) use flate2::{read::GzDecoder, write::GzEncoder, Compression};
#[cfg(feature = "duckdb-runtime")]
pub(crate) use llm_access_core::{
    provider::{ProtocolFamily, ProviderType, RouteStrategy},
    store::{
        AdminRuntimeConfig, KiroLatencyRankingQuery, KiroLatencyRankingRow,
        KiroLatencyRankingSnapshot, UsageAnalyticsStore, UsageChartPoint, UsageEventPage,
        UsageEventQuery, UsageEventSink, UsageEventSource, UsageEventStatusKind, UsageEventTotals,
        UsageFilterOptions, UsageMetricsDimensionView, UsageMetricsQuery, UsageMetricsSnapshot,
        UsageMetricsStatusCodeView, UsageMetricsSummary,
        DEFAULT_DUCKDB_USAGE_CHECKPOINT_THRESHOLD_MIB, DEFAULT_DUCKDB_USAGE_MEMORY_LIMIT_MIB,
        PROVIDER_KIRO,
    },
    usage::{UsageEvent, UsageStreamDetails, UsageTiming},
};
#[cfg(feature = "duckdb-runtime")]
pub(crate) use serde::{Deserialize, Serialize};
#[cfg(feature = "duckdb-runtime")]
pub(crate) use sha2::{Digest, Sha256};
#[cfg(feature = "duckdb-runtime")]
pub(crate) use tokio::task;

#[cfg(feature = "duckdb-runtime")]
pub(crate) use crate::{
    request_cache::RequestCacheConfig,
    usage_catalog::{
        PostgresUsageCatalog, UsageCatalogFieldFilter, UsageCatalogFieldName,
        UsageCatalogFieldRollupRecord, UsageCatalogKeyRollupRecord, UsageCatalogQuery,
        UsageCatalogRetentionSegment, UsageCatalogSegment, UsageCatalogSegmentMatch,
        UsageCatalogSegmentRecord, UsageCatalogSegmentTotals,
    },
    KeyUsageRollupSummary,
};

mod catalog;
mod connection;
mod detail;
mod filter_options;
mod latency;
mod metrics;
mod prune;
mod query;
mod rollup;
mod segment;
mod sql;
mod util;

pub(crate) use catalog::*;
pub use connection::*;
pub(crate) use detail::*;
pub(crate) use filter_options::*;
pub(crate) use latency::*;
pub(crate) use metrics::*;
pub(crate) use prune::*;
pub(crate) use query::*;
pub(crate) use rollup::*;
pub(crate) use segment::*;
pub use sql::*;
pub(crate) use util::*;

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
pub(crate) struct UsageEventDetailRow {
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
impl UsageEventDetailRow {
    fn from_usage_event_row(row: &UsageEventRow) -> Self {
        Self {
            event_id: row.event_id.clone(),
            request_headers_json: row.request_headers_json.clone(),
            routing_diagnostics_json: row.routing_diagnostics_json.clone(),
            last_message_content: row.last_message_content.clone(),
            client_request_body_json: row.client_request_body_json.clone(),
            upstream_request_body_json: row.upstream_request_body_json.clone(),
            full_request_json: row.full_request_json.clone(),
            error_message: row.error_message.clone(),
            error_body: row.error_body.clone(),
        }
    }

    fn has_external_payloads(&self) -> bool {
        has_external_detail_payloads(
            self.client_request_body_json.as_deref(),
            self.upstream_request_body_json.as_deref(),
            self.full_request_json.as_deref(),
            self.error_body.as_deref(),
        )
    }
}
#[cfg(feature = "duckdb-runtime")]
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct UsageEventDetailBlob {
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
impl UsageEventDetailBlob {
    fn from_detail_row(row: &UsageEventDetailRow) -> Self {
        Self {
            request_headers_json: row.request_headers_json.clone(),
            routing_diagnostics_json: row.routing_diagnostics_json.clone(),
            last_message_content: row.last_message_content.clone(),
            client_request_body_json: row.client_request_body_json.clone(),
            upstream_request_body_json: row.upstream_request_body_json.clone(),
            full_request_json: row.full_request_json.clone(),
            error_message: row.error_message.clone(),
            error_body: row.error_body.clone(),
        }
    }

    fn into_detail_row(self, event_id: String) -> UsageEventDetailRow {
        UsageEventDetailRow {
            event_id,
            request_headers_json: self.request_headers_json,
            routing_diagnostics_json: self.routing_diagnostics_json,
            last_message_content: self.last_message_content,
            client_request_body_json: self.client_request_body_json,
            upstream_request_body_json: self.upstream_request_body_json,
            full_request_json: self.full_request_json,
            error_message: self.error_message,
            error_body: self.error_body,
        }
    }
}
#[cfg(feature = "duckdb-runtime")]
#[derive(Debug)]
pub(crate) struct UsageEventDetailPackWrite {
    relative_path: String,
    bytes: Vec<u8>,
}
#[cfg(feature = "duckdb-runtime")]
#[derive(Debug, Clone)]
pub(crate) struct UsageEventDetailObjectRef {
    relative_path: String,
    byte_range: Range<u64>,
    sha256: String,
}
#[cfg(feature = "duckdb-runtime")]
#[derive(Debug, Clone)]
pub(crate) struct UsageEventDetailStore {
    root_dir: PathBuf,
}
#[cfg(feature = "duckdb-runtime")]
impl UsageEventDetailStore {
    fn from_dir(path: &Path) -> anyhow::Result<Option<Self>> {
        if path.as_os_str().is_empty() {
            return Ok(None);
        }
        if !path.is_absolute() {
            return Err(anyhow!(
                "usage details dir `{}` must be an absolute local filesystem path",
                path.display()
            ));
        }
        fs::create_dir_all(path).with_context(|| {
            format!("failed to create usage details directory `{}`", path.display())
        })?;
        Ok(Some(Self {
            root_dir: path.to_path_buf(),
        }))
    }

    fn pack_relative_path_for_rows(&self, rows: &[UsageEventRow], pack_bytes: &[u8]) -> String {
        let first = rows
            .iter()
            .find(|row| row.detail_object_payload_present)
            .or_else(|| rows.first())
            .expect("detail pack rows should not be empty");
        let (year, month, day) = utc_date_parts(first.created_at_ms);
        let pack_hash = sha256_hex(pack_bytes);
        format!(
            "packs/{}/{year:04}/{month:02}/{day:02}/{}-{}.detailpack-v1",
            first.provider_type,
            first.event_id,
            &pack_hash[..16]
        )
    }

    fn prepare_pack(
        &self,
        rows: &mut [UsageEventRow],
    ) -> anyhow::Result<Option<UsageEventDetailPackWrite>> {
        let mut pack_bytes = Vec::new();
        let mut packed = Vec::new();
        let mut seen = BTreeMap::<String, (i64, i64, String)>::new();
        for (index, row) in rows.iter_mut().enumerate() {
            let detail = UsageEventDetailRow::from_usage_event_row(row);
            let has_external_payloads = detail.has_external_payloads();
            row.detail_object_payload_present = has_external_payloads;
            if !has_external_payloads {
                row.detail_object_path = None;
                row.detail_object_offset = None;
                row.detail_object_length = None;
                row.detail_object_sha256 = None;
                continue;
            }
            let blob = UsageEventDetailBlob::from_detail_row(&detail);
            let encoded = gzip_json_bytes(&blob)
                .with_context(|| format!("failed to encode usage detail `{}`", row.event_id))?;
            let compressed_sha = sha256_hex(&encoded);
            let (offset, length, sha256) =
                if let Some((offset, length, sha256)) = seen.get(&compressed_sha).cloned() {
                    (offset, length, sha256)
                } else {
                    let offset = i64::try_from(pack_bytes.len())
                        .context("usage detail pack offset exceeds i64")?;
                    let length = i64::try_from(encoded.len())
                        .context("usage detail pack member length exceeds i64")?;
                    pack_bytes.extend_from_slice(&encoded);
                    seen.insert(compressed_sha.clone(), (offset, length, compressed_sha.clone()));
                    (offset, length, compressed_sha)
                };
            packed.push((index, offset, length, sha256));
        }
        if packed.is_empty() {
            return Ok(None);
        }
        let relative_path = self.pack_relative_path_for_rows(rows, &pack_bytes);
        for (index, offset, length, sha256) in packed {
            rows[index].detail_object_path = Some(relative_path.clone());
            rows[index].detail_object_offset = Some(offset);
            rows[index].detail_object_length = Some(length);
            rows[index].detail_object_sha256 = Some(sha256);
        }
        Ok(Some(UsageEventDetailPackWrite {
            relative_path,
            bytes: pack_bytes,
        }))
    }

    async fn put_pack(&self, pack: UsageEventDetailPackWrite) -> anyhow::Result<()> {
        let pack_path = self.root_dir.join(&pack.relative_path);
        if let Some(parent) = pack_path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "failed to create usage detail pack parent directory `{}`",
                    parent.display()
                )
            })?;
        }
        fs::write(&pack_path, pack.bytes).with_context(|| {
            format!("failed to write usage detail pack `{}`", pack_path.display())
        })?;
        Ok(())
    }

    async fn get_row_for_ref(
        &self,
        event_id: &str,
        detail_ref: &UsageEventDetailObjectRef,
    ) -> anyhow::Result<Option<UsageEventDetailRow>> {
        let pack_path = self.root_dir.join(&detail_ref.relative_path);
        let mut file = match fs::File::open(&pack_path) {
            Ok(file) => file,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => {
                return Err(err).with_context(|| {
                    format!("failed to open usage detail pack `{}`", pack_path.display())
                })
            },
        };
        let range_len = detail_ref
            .byte_range
            .end
            .checked_sub(detail_ref.byte_range.start)
            .ok_or_else(|| anyhow!("usage detail pack byte range is invalid"))?;
        let mut bytes =
            vec![0_u8; usize::try_from(range_len).context("detail byte range too large")?];
        file.seek(SeekFrom::Start(detail_ref.byte_range.start))
            .with_context(|| {
                format!("failed to seek usage detail pack `{}`", pack_path.display())
            })?;
        file.read_exact(&mut bytes).with_context(|| {
            format!("failed to read usage detail pack `{}`", pack_path.display())
        })?;
        let actual_sha = sha256_hex(&bytes);
        if actual_sha != detail_ref.sha256 {
            return Err(anyhow!(
                "usage detail pack member hash mismatch for event `{event_id}` in `{}`",
                pack_path.display()
            ));
        }
        let blob: UsageEventDetailBlob = gunzip_json_bytes(&bytes).with_context(|| {
            format!("failed to decode usage detail pack member `{}`", pack_path.display())
        })?;
        Ok(Some(blob.into_detail_row(event_id.to_string())))
    }
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

    fn insert_usage_event_summaries(&mut self, rows: &[UsageEventRow]) -> anyhow::Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        {
            let mut summary_stmt = tx.prepare(insert_usage_event_sql())?;
            for row in rows {
                execute_usage_event_insert(&mut summary_stmt, row)?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Insert a batch of usage event rows in one transaction.
    pub fn insert_usage_events(&mut self, rows: &[UsageEventRow]) -> anyhow::Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        {
            let mut summary_stmt = tx.prepare(insert_usage_event_sql())?;
            let mut detail_stmt = tx.prepare(insert_usage_event_detail_sql())?;
            for row in rows {
                execute_usage_event_insert(&mut summary_stmt, row)?;
                execute_usage_event_detail_insert(&mut detail_stmt, row)?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Insert only the summary projection for a batch of usage events.
    pub fn insert_usage_event_summaries_only(
        &mut self,
        rows: &[UsageEventRow],
    ) -> anyhow::Result<()> {
        self.insert_usage_event_summaries(rows)
    }
}
#[cfg(feature = "duckdb-runtime")]
#[derive(Debug)]
pub(crate) struct HotUsageWriter {
    summary: DuckDbUsageWriter,
    detail_store: Option<Arc<UsageEventDetailStore>>,
}
#[cfg(feature = "duckdb-runtime")]
impl HotUsageWriter {
    fn open(
        duckdb_path: &Path,
        connection_config: DuckDbUsageConnectionConfig,
        detail_store: Option<Arc<UsageEventDetailStore>>,
    ) -> anyhow::Result<Self> {
        let summary =
            DuckDbUsageWriter::new(DuckDbUsageRepository::open_conn_with_connection_config(
                duckdb_path,
                connection_config,
            )?)?;
        Ok(Self {
            summary,
            detail_store,
        })
    }

    async fn insert_usage_events(&mut self, rows: &[UsageEventRow]) -> anyhow::Result<()> {
        if let Some(detail_store) = &self.detail_store {
            let mut rows = rows.to_vec();
            let pack = detail_store.prepare_pack(&mut rows)?;
            self.summary.insert_usage_event_summaries(&rows)?;
            if let Some(pack) = pack {
                detail_store.put_pack(pack).await?;
            }
            return Ok(());
        }
        self.summary.insert_usage_event_summaries(rows)?;
        Ok(())
    }
}
#[cfg(feature = "duckdb-runtime")]
#[derive(Debug)]
pub(crate) struct PersistentUsageWriter {
    writer: HotUsageWriter,
    connection_config: DuckDbUsageConnectionConfig,
}
#[cfg(feature = "duckdb-runtime")]
impl PersistentUsageWriter {
    fn open(
        path: &Path,
        connection_config: DuckDbUsageConnectionConfig,
        detail_store: Option<Arc<UsageEventDetailStore>>,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            writer: HotUsageWriter::open(path, connection_config, detail_store)?,
            connection_config,
        })
    }
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
pub(crate) enum DuckDbUsageRepositoryInner {
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
pub(crate) type SharedDuckDbUsageConnectionConfig = Arc<RwLock<DuckDbUsageConnectionConfig>>;
/// Runtime-tunable DuckDB connection settings for usage analytics writes.
#[cfg(feature = "duckdb-runtime")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DuckDbUsageConnectionConfig {
    /// DuckDB buffer-manager memory limit in MiB.
    pub memory_limit_mib: u64,
    /// WAL size threshold for automatic checkpoints in MiB.
    pub checkpoint_threshold_mib: u64,
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
pub(crate) struct TieredDuckDbUsageState {
    active_path: PathBuf,
    next_sequence: u64,
    active_has_rows: bool,
    active_writer: Option<PersistentUsageWriter>,
    detail_store: Option<Arc<UsageEventDetailStore>>,
}
#[cfg(feature = "duckdb-runtime")]
#[derive(Debug)]
pub(crate) struct SingleDuckDbUsageState {
    path: PathBuf,
    writer: Option<PersistentUsageWriter>,
}
#[cfg(feature = "duckdb-runtime")]
#[derive(Debug, Clone)]
pub(crate) struct ArchivedUsageSegment {
    archive_path: PathBuf,
    start_ms: Option<i64>,
    end_ms: Option<i64>,
}
#[cfg(feature = "duckdb-runtime")]
#[derive(Debug, Clone)]
pub(crate) enum TieredUsagePartitionKind {
    Active,
    Archive,
}
#[cfg(feature = "duckdb-runtime")]
#[derive(Debug, Clone)]
pub(crate) struct TieredUsagePartition {
    path: PathBuf,
    count: usize,
    totals: UsageEventTotals,
    kind: TieredUsagePartitionKind,
}
#[cfg(feature = "duckdb-runtime")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TieredUsagePageFetch {
    partition_index: usize,
    local_newest_offset: usize,
    limit: usize,
}
#[cfg(feature = "duckdb-runtime")]
#[derive(Debug, Clone)]
pub(crate) struct SegmentKeyRollup {
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
pub(crate) struct SegmentFieldRollup {
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
pub(crate) struct SegmentStats {
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
pub(crate) struct ArchivedSegmentPaths {
    pending_duckdb: PathBuf,
    compact_duckdb: PathBuf,
    uploading_duckdb: PathBuf,
    archive_duckdb: PathBuf,
}
#[cfg(feature = "duckdb-runtime")]
#[derive(Debug)]
pub(crate) enum TieredUsageCatalogBackend {
    Postgres(Arc<PostgresUsageCatalog>),
    Test(Arc<TestTieredUsageCatalog>),
}
#[cfg(feature = "duckdb-runtime")]
#[derive(Debug)]
pub(crate) struct TestTieredUsageCatalog {
    path: PathBuf,
    state: Mutex<TestTieredUsageCatalogState>,
}
#[cfg(feature = "duckdb-runtime")]
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub(crate) struct TestTieredUsageCatalogState {
    segments: BTreeMap<String, UsageCatalogSegmentRecord>,
    segment_rollups: BTreeMap<String, Vec<UsageCatalogKeyRollupRecord>>,
    segment_field_rollups: BTreeMap<String, Vec<UsageCatalogFieldRollupRecord>>,
    event_locators: BTreeMap<String, String>,
}
#[cfg(feature = "duckdb-runtime")]
const DUCKDB_COMPACT_MAX_TEMP_DIRECTORY_SIZE: &str = "8GB";
#[cfg(feature = "duckdb-runtime")]
const DUCKDB_USAGE_CONNECTION_MAX_TEMP_DIRECTORY_SIZE: &str = "2GB";
#[cfg(feature = "duckdb-runtime")]
impl DuckDbUsageRepository {
    /// Open a DuckDB usage repository and initialize the analytics schema.
    pub fn open_path(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        Self::open_path_with_connection_config(
            path,
            Arc::new(RwLock::new(DuckDbUsageConnectionConfig::default())),
        )
    }

    /// Open a DuckDB usage repository with runtime-tunable connection settings.
    pub fn open_path_with_connection_config(
        path: impl AsRef<Path>,
        connection_config: SharedDuckDbUsageConnectionConfig,
    ) -> anyhow::Result<Self> {
        let path = path.as_ref().to_path_buf();
        initialize_duckdb_target_path_with_connection_config(
            &path,
            connection_config_snapshot(&connection_config),
        )?;
        Ok(Self {
            inner: Arc::new(DuckDbUsageRepositoryInner::Single {
                state: Box::new(Mutex::new(SingleDuckDbUsageState {
                    path,
                    writer: None,
                })),
                connection_config,
            }),
        })
    }

    /// Open a tiered DuckDB usage repository.
    pub fn open_tiered(config: TieredDuckDbUsageConfig) -> anyhow::Result<Self> {
        Self::open_tiered_with_connection_config(
            config,
            Arc::new(RwLock::new(DuckDbUsageConnectionConfig::default())),
        )
    }

    /// Open a tiered DuckDB usage repository with runtime-tunable settings.
    pub fn open_tiered_with_connection_config(
        config: TieredDuckDbUsageConfig,
        connection_config: SharedDuckDbUsageConnectionConfig,
    ) -> anyhow::Result<Self> {
        let catalog_backend = Arc::new(TieredUsageCatalogBackend::Test(Arc::new(
            TestTieredUsageCatalog::open(test_catalog_state_path(&config))?,
        )));
        Self::open_tiered_with_catalog_backend(config, connection_config, catalog_backend)
    }

    /// Open a tiered DuckDB usage repository with a Postgres-backed archive
    /// catalog and optional Valkey read cache.
    pub fn open_tiered_with_postgres_catalog_with_connection_config(
        config: TieredDuckDbUsageConfig,
        connection_config: SharedDuckDbUsageConnectionConfig,
        database_url: &str,
        request_cache_config: Option<RequestCacheConfig>,
    ) -> anyhow::Result<Self> {
        let catalog_backend = Arc::new(TieredUsageCatalogBackend::Postgres(Arc::new(
            PostgresUsageCatalog::new(database_url, request_cache_config)?,
        )));
        Self::open_tiered_with_catalog_backend(config, connection_config, catalog_backend)
    }

    fn open_tiered_with_catalog_backend(
        config: TieredDuckDbUsageConfig,
        connection_config: SharedDuckDbUsageConnectionConfig,
        catalog_backend: Arc<TieredUsageCatalogBackend>,
    ) -> anyhow::Result<Self> {
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
        clear_stale_compacting_files(&config)?;
        let detail_store = config
            .details_dir
            .as_deref()
            .map(UsageEventDetailStore::from_dir)
            .transpose()?
            .flatten()
            .map(Arc::new);

        seed_catalog_from_archives_if_empty(catalog_backend.as_ref(), &config)?;
        refresh_catalog_from_archives_if_needed(catalog_backend.as_ref())?;
        spawn_existing_pending_sealers(
            config.clone(),
            Arc::clone(&catalog_backend),
            Arc::clone(&connection_config),
        )?;
        let (active_path, next_sequence) =
            choose_active_segment(&config, catalog_backend.as_ref())?;
        let active_has_rows = active_path.exists();
        initialize_duckdb_target_path_with_connection_config(
            &active_path,
            connection_config_snapshot(&connection_config),
        )?;
        Ok(Self {
            inner: Arc::new(DuckDbUsageRepositoryInner::Tiered {
                config,
                state: Box::new(Mutex::new(TieredDuckDbUsageState {
                    active_path,
                    next_sequence,
                    active_has_rows,
                    active_writer: None,
                    detail_store,
                })),
                connection_config,
                catalog_backend,
            }),
        })
    }

    /// Prune tiered usage analytics outside the retained day window.
    pub async fn prune_usage_analytics(
        &self,
        now_ms: i64,
        retention_days: u64,
    ) -> anyhow::Result<UsageAnalyticsPruneReport> {
        match self.inner.as_ref() {
            DuckDbUsageRepositoryInner::Single {
                ..
            } => Ok(UsageAnalyticsPruneReport::default()),
            DuckDbUsageRepositoryInner::Tiered {
                config,
                state,
                connection_config,
                catalog_backend,
            } => {
                prune_tiered_usage_analytics(
                    config,
                    state,
                    connection_config,
                    catalog_backend.as_ref(),
                    now_ms,
                    retention_days,
                )
                .await
            },
        }
    }

    fn open_conn_with_connection_config(
        path: &Path,
        connection_config: DuckDbUsageConnectionConfig,
    ) -> anyhow::Result<duckdb::Connection> {
        let conn = Self::open_raw_conn(path)?;
        configure_duckdb_usage_connection(&conn, connection_config)?;
        Ok(conn)
    }

    fn open_raw_conn(path: &Path) -> anyhow::Result<duckdb::Connection> {
        duckdb::Connection::open(path)
            .with_context(|| format!("failed to open duckdb database `{}`", path.display()))
    }

    fn open_read_only_conn(path: &Path) -> anyhow::Result<duckdb::Connection> {
        let config = duckdb::Config::default()
            .access_mode(duckdb::AccessMode::ReadOnly)
            .context("failed to configure duckdb read-only access")?;
        let conn = duckdb::Connection::open_with_flags(path, config).with_context(|| {
            format!("failed to open read-only duckdb database `{}`", path.display())
        })?;
        configure_duckdb_usage_connection(&conn, DuckDbUsageConnectionConfig::default())?;
        Ok(conn)
    }

    fn open_checkpoint_conn(
        path: &Path,
        connection_config: DuckDbUsageConnectionConfig,
    ) -> anyhow::Result<duckdb::Connection> {
        let conn = Self::open_raw_conn(path)?;
        let temp_dir = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("checkpointing");
        configure_duckdb_compact_connection(&conn, &temp_dir, connection_config)?;
        Ok(conn)
    }

    /// Aggregate all persisted usage events into per-key operational rollups.
    pub async fn key_usage_rollups(&self) -> anyhow::Result<Vec<KeyUsageRollupSummary>> {
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || match inner.as_ref() {
            DuckDbUsageRepositoryInner::Single {
                state, ..
            } => {
                let path = {
                    let state = state
                        .lock()
                        .map_err(|_| anyhow!("single duckdb state lock poisoned"))?;
                    state.path.clone()
                };
                key_usage_rollups_from_path(&path)
            },
            DuckDbUsageRepositoryInner::Tiered {
                config,
                state,
                catalog_backend,
                ..
            } => key_usage_rollups_from_tiered(config, state, catalog_backend.as_ref()),
        })
        .await
        .context("duckdb key usage rollup task failed")?
    }

    /// Append a batch after removing only in-memory duplicates from the same
    /// call.
    pub async fn append_usage_events_if_new(&self, events: &[UsageEvent]) -> anyhow::Result<usize> {
        let deduped = dedupe_usage_events_owned(events.to_vec());
        if deduped.is_empty() {
            return Ok(0);
        }
        UsageEventSink::append_usage_events(self, &deduped).await?;
        Ok(deduped.len())
    }

    /// Append already-enriched fact rows after removing only in-memory
    /// duplicates from the same call.
    pub async fn append_usage_event_rows_owned(
        &self,
        rows: Vec<UsageEventRow>,
    ) -> anyhow::Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let inner = Arc::clone(&self.inner);
        let mut seen = HashSet::new();
        let deduped = rows
            .into_iter()
            .filter(|row| seen.insert(row.event_id.clone()))
            .collect::<Vec<_>>();
        if deduped.is_empty() {
            return Ok(());
        }
        match inner.as_ref() {
            DuckDbUsageRepositoryInner::Single {
                ..
            } => {
                let inner = Arc::clone(&inner);
                task::spawn_blocking(move || match inner.as_ref() {
                    DuckDbUsageRepositoryInner::Single {
                        state,
                        connection_config,
                    } => {
                        let mut state = state
                            .lock()
                            .map_err(|_| anyhow!("single duckdb state lock poisoned"))?;
                        let writer = ensure_single_writer(
                            &mut state,
                            connection_config_snapshot(connection_config),
                        )?;
                        writer.writer.summary.insert_usage_events(&deduped)
                    },
                    _ => unreachable!("single branch expected"),
                })
                .await
                .context("duckdb usage row insert task failed")?
            },
            DuckDbUsageRepositoryInner::Tiered {
                config,
                state,
                catalog_backend,
                connection_config,
            } => {
                append_usage_events_to_tiered(
                    config,
                    state,
                    connection_config,
                    catalog_backend,
                    &deduped,
                )
                .await
            },
        }
    }
}
#[cfg(feature = "duckdb-runtime")]
impl TieredUsageCatalogBackend {
    fn is_empty(&self) -> anyhow::Result<bool> {
        match self {
            Self::Postgres(catalog) => catalog.is_empty(),
            Self::Test(catalog) => catalog.is_empty(),
        }
    }

    fn next_sequence(&self) -> anyhow::Result<u64> {
        match self {
            Self::Postgres(catalog) => catalog.next_sequence(),
            Self::Test(catalog) => catalog.next_sequence(),
        }
    }

    fn archive_path_for_segment(&self, segment_id: &str) -> anyhow::Result<Option<PathBuf>> {
        match self {
            Self::Postgres(catalog) => catalog.archive_path_for_segment(segment_id),
            Self::Test(catalog) => catalog.archive_path_for_segment(segment_id),
        }
    }

    fn publish_segment(
        &self,
        segment: &UsageCatalogSegmentRecord,
        rollups: &[UsageCatalogKeyRollupRecord],
        field_rollups: &[UsageCatalogFieldRollupRecord],
        event_ids: &[String],
    ) -> anyhow::Result<()> {
        match self {
            Self::Postgres(catalog) => {
                catalog.publish_segment(segment, rollups, field_rollups, event_ids)
            },
            Self::Test(catalog) => {
                catalog.publish_segment(segment, rollups, field_rollups, event_ids)
            },
        }
    }

    fn archived_key_usage_rollups(&self) -> anyhow::Result<Vec<KeyUsageRollupSummary>> {
        match self {
            Self::Postgres(catalog) => catalog.archived_key_usage_rollups(),
            Self::Test(catalog) => catalog.archived_key_usage_rollups(),
        }
    }

    fn delete_expired_segments(
        &self,
        cutoff_ms: i64,
    ) -> anyhow::Result<Vec<UsageCatalogRetentionSegment>> {
        match self {
            Self::Postgres(catalog) => catalog.delete_expired_segments(cutoff_ms),
            Self::Test(catalog) => catalog.delete_expired_segments(cutoff_ms),
        }
    }

    fn archived_paths(&self) -> anyhow::Result<HashSet<PathBuf>> {
        match self {
            Self::Postgres(catalog) => catalog.archived_paths(),
            Self::Test(catalog) => catalog.archived_paths(),
        }
    }

    fn archived_paths_missing_field_rollups(&self) -> anyhow::Result<Vec<PathBuf>> {
        match self {
            Self::Postgres(catalog) => catalog.archived_paths_missing_field_rollups(),
            Self::Test(catalog) => catalog.archived_paths_missing_field_rollups(),
        }
    }

    fn archived_segments_for_query(
        &self,
        query: &UsageEventQuery,
    ) -> anyhow::Result<Vec<ArchivedUsageSegment>> {
        let catalog_query = catalog_query_from_usage_query(query);
        match self {
            Self::Postgres(catalog) => catalog
                .archived_segment_matches_for_query(&catalog_query)
                .map(|segments| {
                    segments
                        .into_iter()
                        .map(|segment| segment.segment.into())
                        .collect()
                }),
            Self::Test(catalog) => catalog.archived_segments_for_query(&catalog_query),
        }
    }

    fn archived_segment_matches_for_query(
        &self,
        query: &UsageEventQuery,
    ) -> anyhow::Result<Vec<UsageCatalogSegmentMatch>> {
        let catalog_query = catalog_query_from_usage_query(query);
        match self {
            Self::Postgres(catalog) => catalog.archived_segment_matches_for_query(&catalog_query),
            Self::Test(catalog) => catalog.archived_segment_matches_for_query(&catalog_query),
        }
    }

    fn archived_filter_option_values(
        &self,
        query: &UsageEventQuery,
        field_name: UsageCatalogFieldName,
    ) -> anyhow::Result<Option<Vec<String>>> {
        let catalog_query = catalog_filter_options_query_from_usage_query(query, field_name);
        match self {
            Self::Postgres(catalog) => {
                catalog.archived_filter_option_values(&catalog_query, field_name)
            },
            Self::Test(catalog) => {
                catalog.archived_filter_option_values(&catalog_query, field_name)
            },
        }
    }

    fn locate_archived_segment(
        &self,
        event_id: &str,
    ) -> anyhow::Result<Option<ArchivedUsageSegment>> {
        match self {
            Self::Postgres(catalog) => catalog
                .locate_archived_segment(event_id)
                .map(|segment| segment.map(Into::into)),
            Self::Test(catalog) => catalog.locate_archived_segment(event_id),
        }
    }
}
#[cfg(feature = "duckdb-runtime")]
impl TestTieredUsageCatalog {
    fn open(path: PathBuf) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "failed to create test usage catalog parent directory `{}`",
                    parent.display()
                )
            })?;
        }
        let state = if path.exists() {
            let bytes = fs::read(&path).with_context(|| {
                format!("failed to read test usage catalog state `{}`", path.display())
            })?;
            serde_json::from_slice::<TestTieredUsageCatalogState>(&bytes).with_context(|| {
                format!("failed to deserialize test usage catalog state `{}`", path.display())
            })?
        } else {
            TestTieredUsageCatalogState::default()
        };
        Ok(Self {
            path,
            state: Mutex::new(state),
        })
    }

    fn lock(&self) -> anyhow::Result<std::sync::MutexGuard<'_, TestTieredUsageCatalogState>> {
        self.state
            .lock()
            .map_err(|_| anyhow!("test tiered usage catalog lock poisoned"))
    }

    fn persist(&self, state: &TestTieredUsageCatalogState) -> anyhow::Result<()> {
        let bytes = serde_json::to_vec(state).context("serialize test usage catalog state")?;
        let temp_path = self.path.with_extension("json.tmp");
        fs::write(&temp_path, bytes).with_context(|| {
            format!("failed to write test usage catalog temp state `{}`", temp_path.display())
        })?;
        fs::rename(&temp_path, &self.path).with_context(|| {
            format!("failed to replace test usage catalog state `{}`", self.path.display())
        })?;
        Ok(())
    }

    fn is_empty(&self) -> anyhow::Result<bool> {
        Ok(self.lock()?.segments.is_empty())
    }

    fn next_sequence(&self) -> anyhow::Result<u64> {
        Ok(self
            .lock()?
            .segments
            .keys()
            .filter_map(|segment_id| parse_sequence_from_segment_id(segment_id))
            .max()
            .unwrap_or(0))
    }

    fn archive_path_for_segment(&self, segment_id: &str) -> anyhow::Result<Option<PathBuf>> {
        Ok(self
            .lock()?
            .segments
            .get(segment_id)
            .map(|segment| segment.archive_path.clone()))
    }

    fn publish_segment(
        &self,
        segment: &UsageCatalogSegmentRecord,
        rollups: &[UsageCatalogKeyRollupRecord],
        field_rollups: &[UsageCatalogFieldRollupRecord],
        event_ids: &[String],
    ) -> anyhow::Result<()> {
        let mut state = self.lock()?;
        state
            .segments
            .insert(segment.segment_id.clone(), segment.clone());
        state
            .segment_rollups
            .insert(segment.segment_id.clone(), rollups.to_vec());
        state
            .segment_field_rollups
            .insert(segment.segment_id.clone(), field_rollups.to_vec());
        state
            .event_locators
            .retain(|_, current_segment_id| current_segment_id != &segment.segment_id);
        for event_id in event_ids {
            state
                .event_locators
                .insert(event_id.clone(), segment.segment_id.clone());
        }
        self.persist(&state)?;
        Ok(())
    }

    fn archived_key_usage_rollups(&self) -> anyhow::Result<Vec<KeyUsageRollupSummary>> {
        let state = self.lock()?;
        let mut combined = BTreeMap::<String, KeyUsageRollupSummary>::new();
        for rollup in state.segment_rollups.values().flatten() {
            merge_key_rollup(&mut combined, KeyUsageRollupSummary {
                key_id: rollup.key_id.clone(),
                input_uncached_tokens: rollup.input_uncached_tokens,
                input_cached_tokens: rollup.input_cached_tokens,
                output_tokens: rollup.output_tokens,
                billable_tokens: rollup.billable_tokens,
                credit_total: rollup.credit_total.clone(),
                credit_missing_events: rollup.credit_missing_events,
                last_used_at_ms: rollup.last_used_at_ms,
            });
        }
        Ok(combined.into_values().collect())
    }

    fn delete_expired_segments(
        &self,
        cutoff_ms: i64,
    ) -> anyhow::Result<Vec<UsageCatalogRetentionSegment>> {
        let mut state = self.lock()?;
        let mut deleted = state
            .segments
            .iter()
            .filter(|(_, segment)| segment.end_ms.is_some_and(|end_ms| end_ms < cutoff_ms))
            .map(|(segment_id, segment)| UsageCatalogRetentionSegment {
                segment_id: segment_id.clone(),
                archive_path: segment.archive_path.clone(),
            })
            .collect::<Vec<_>>();
        deleted.sort_by(|left, right| left.segment_id.cmp(&right.segment_id));
        if deleted.is_empty() {
            return Ok(deleted);
        }
        let deleted_ids = deleted
            .iter()
            .map(|segment| segment.segment_id.as_str())
            .collect::<HashSet<_>>();
        state
            .segments
            .retain(|segment_id, _| !deleted_ids.contains(segment_id.as_str()));
        state
            .segment_rollups
            .retain(|segment_id, _| !deleted_ids.contains(segment_id.as_str()));
        state
            .segment_field_rollups
            .retain(|segment_id, _| !deleted_ids.contains(segment_id.as_str()));
        state
            .event_locators
            .retain(|_, segment_id| !deleted_ids.contains(segment_id.as_str()));
        self.persist(&state)?;
        Ok(deleted)
    }

    fn archived_paths(&self) -> anyhow::Result<HashSet<PathBuf>> {
        Ok(self
            .lock()?
            .segments
            .values()
            .map(|segment| segment.archive_path.clone())
            .collect())
    }

    fn archived_paths_missing_field_rollups(&self) -> anyhow::Result<Vec<PathBuf>> {
        let state = self.lock()?;
        let mut paths = state
            .segments
            .iter()
            .filter(|(segment_id, _)| {
                state
                    .segment_field_rollups
                    .get(*segment_id)
                    .is_none_or(|rollups| rollups.is_empty())
            })
            .map(|(_, segment)| segment.archive_path.clone())
            .collect::<Vec<_>>();
        paths.sort();
        Ok(paths)
    }

    fn archived_segments_for_query(
        &self,
        query: &UsageCatalogQuery,
    ) -> anyhow::Result<Vec<ArchivedUsageSegment>> {
        let state = self.lock()?;
        let mut segments = state
            .segments
            .values()
            .filter(|segment| segment_matches_time_window(segment, query.start_ms, query.end_ms))
            .filter(|segment| {
                test_catalog_segment_matches_query(&state, &segment.segment_id, query)
            })
            .map(archived_segment_from_record)
            .collect::<Vec<_>>();
        sort_archived_segments(&mut segments);
        Ok(segments)
    }

    fn archived_segment_matches_for_query(
        &self,
        query: &UsageCatalogQuery,
    ) -> anyhow::Result<Vec<UsageCatalogSegmentMatch>> {
        let state = self.lock()?;
        let mut segments = Vec::new();
        for (segment_id, segment) in state.segments.iter().filter(|(_, segment)| {
            segment_matches_time_window(segment, query.start_ms, query.end_ms)
        }) {
            if !test_catalog_segment_matches_query(&state, segment_id, query) {
                continue;
            }
            let matching_totals =
                test_catalog_segment_totals_for_query(&state, segment_id, segment, query);
            if query.field_filters.len() > 1 || matching_totals.is_some() {
                segments.push(UsageCatalogSegmentMatch {
                    segment: UsageCatalogSegment {
                        archive_path: segment.archive_path.clone(),
                        start_ms: segment.start_ms,
                        end_ms: segment.end_ms,
                        row_count: segment.row_count,
                    },
                    matching_totals,
                });
            }
        }
        segments.sort_by(|left, right| {
            right
                .segment
                .end_ms
                .unwrap_or_default()
                .cmp(&left.segment.end_ms.unwrap_or_default())
                .then_with(|| right.segment.archive_path.cmp(&left.segment.archive_path))
        });
        Ok(segments)
    }

    fn archived_filter_option_values(
        &self,
        query: &UsageCatalogQuery,
        field_name: UsageCatalogFieldName,
    ) -> anyhow::Result<Option<Vec<String>>> {
        if !query.field_filters.is_empty() {
            return Ok(None);
        }
        let state = self.lock()?;
        let mut values = BTreeSet::new();
        for (segment_id, _segment) in state.segments.iter().filter(|(_, segment)| {
            segment_matches_time_window(segment, query.start_ms, query.end_ms)
        }) {
            let Some(rollups) = state.segment_field_rollups.get(segment_id) else {
                continue;
            };
            for rollup in rollups {
                if rollup.field_name != field_name {
                    continue;
                }
                if !test_field_rollup_matches_scope(rollup, query) {
                    continue;
                }
                if !test_rollup_matches_time(
                    rollup.first_used_at_ms,
                    rollup.last_used_at_ms,
                    query.start_ms,
                    query.end_ms,
                ) {
                    continue;
                }
                values.insert(rollup.field_value.clone());
            }
        }
        Ok(Some(values.into_iter().collect()))
    }

    fn locate_archived_segment(
        &self,
        event_id: &str,
    ) -> anyhow::Result<Option<ArchivedUsageSegment>> {
        let state = self.lock()?;
        let Some(segment_id) = state.event_locators.get(event_id) else {
            return Ok(None);
        };
        Ok(state
            .segments
            .get(segment_id)
            .map(archived_segment_from_record))
    }
}
#[cfg(feature = "duckdb-runtime")]
impl From<UsageCatalogSegment> for ArchivedUsageSegment {
    fn from(value: UsageCatalogSegment) -> Self {
        Self {
            archive_path: value.archive_path,
            start_ms: value.start_ms,
            end_ms: value.end_ms,
        }
    }
}
#[cfg(feature = "duckdb-runtime")]
const USAGE_ANALYTICS_RETENTION_DAY_MS: i64 = 86_400_000;
#[cfg(feature = "duckdb-runtime")]
#[derive(Debug)]
pub(crate) struct RetentionSegmentCandidate {
    archive_path: PathBuf,
}
#[cfg(feature = "duckdb-runtime")]
#[async_trait]
impl UsageEventSink for DuckDbUsageRepository {
    async fn append_usage_events(&self, events: &[UsageEvent]) -> anyhow::Result<()> {
        self.append_usage_events_owned(events.to_vec()).await
    }

    async fn append_usage_events_owned(&self, events: Vec<UsageEvent>) -> anyhow::Result<()> {
        if events.is_empty() {
            return Ok(());
        }
        let deduped = dedupe_usage_events_owned(events);
        if deduped.is_empty() {
            return Ok(());
        }
        let rows = deduped
            .iter()
            .map(UsageEventRow::from_usage_event)
            .collect::<Vec<_>>();
        self.append_usage_event_rows_owned(rows).await
    }
}
#[cfg(feature = "duckdb-runtime")]
#[async_trait]
impl UsageAnalyticsStore for DuckDbUsageRepository {
    async fn list_usage_events(&self, query: UsageEventQuery) -> anyhow::Result<UsageEventPage> {
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || match inner.as_ref() {
            DuckDbUsageRepositoryInner::Single {
                state, ..
            } => {
                let path = {
                    let state = state
                        .lock()
                        .map_err(|_| anyhow!("single duckdb state lock poisoned"))?;
                    state.path.clone()
                };
                list_usage_events_from_path(&path, &query)
            },
            DuckDbUsageRepositoryInner::Tiered {
                config,
                state,
                catalog_backend,
                ..
            } => list_usage_events_from_tiered(config, state, catalog_backend.as_ref(), &query),
        })
        .await
        .context("duckdb usage event list task failed")?
    }

    async fn get_usage_event(&self, event_id: &str) -> anyhow::Result<Option<UsageEvent>> {
        let inner = Arc::clone(&self.inner);
        let event_id = event_id.to_string();
        match inner.as_ref() {
            DuckDbUsageRepositoryInner::Single {
                ..
            } => {
                let inner = Arc::clone(&inner);
                task::spawn_blocking(move || match inner.as_ref() {
                    DuckDbUsageRepositoryInner::Single {
                        state, ..
                    } => {
                        let path = {
                            let state = state
                                .lock()
                                .map_err(|_| anyhow!("single duckdb state lock poisoned"))?;
                            state.path.clone()
                        };
                        get_usage_event_from_path(&path, &event_id)
                    },
                    _ => unreachable!("single branch expected"),
                })
                .await
                .context("duckdb usage event detail task failed")?
            },
            DuckDbUsageRepositoryInner::Tiered {
                config,
                state,
                catalog_backend,
                ..
            } => {
                get_usage_event_from_tiered(config, state, catalog_backend.as_ref(), &event_id)
                    .await
            },
        }
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
                state, ..
            } => {
                let path = {
                    let state = state
                        .lock()
                        .map_err(|_| anyhow!("single duckdb state lock poisoned"))?;
                    state.path.clone()
                };
                usage_chart_points_from_single_path(
                    &path,
                    &key_id,
                    start_ms,
                    bucket_ms,
                    bucket_count,
                )
            },
            DuckDbUsageRepositoryInner::Tiered {
                config,
                state,
                catalog_backend,
                ..
            } => usage_chart_points_from_tiered(
                config,
                state,
                catalog_backend.as_ref(),
                &key_id,
                start_ms,
                bucket_ms,
                bucket_count,
            ),
        })
        .await
        .context("duckdb usage chart task failed")?
    }

    async fn list_usage_filter_options(
        &self,
        query: UsageEventQuery,
    ) -> anyhow::Result<UsageFilterOptions> {
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || match inner.as_ref() {
            DuckDbUsageRepositoryInner::Single {
                state, ..
            } => {
                let path = {
                    let state = state
                        .lock()
                        .map_err(|_| anyhow!("single duckdb state lock poisoned"))?;
                    state.path.clone()
                };
                list_usage_filter_options_from_path(&path, &query)
            },
            DuckDbUsageRepositoryInner::Tiered {
                config,
                state,
                catalog_backend,
                ..
            } => {
                let active_path = {
                    let state = state
                        .lock()
                        .map_err(|_| anyhow!("tiered duckdb state lock poisoned"))?;
                    state.active_path.clone()
                };
                list_usage_filter_options_from_tiered(
                    config,
                    catalog_backend.as_ref(),
                    &active_path,
                    &query,
                )
            },
        })
        .await
        .context("duckdb usage filter options task failed")?
    }

    async fn usage_metrics_snapshot(
        &self,
        query: UsageMetricsQuery,
    ) -> anyhow::Result<UsageMetricsSnapshot> {
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || match inner.as_ref() {
            DuckDbUsageRepositoryInner::Single {
                state, ..
            } => {
                let path = {
                    let state = state
                        .lock()
                        .map_err(|_| anyhow!("single duckdb state lock poisoned"))?;
                    state.path.clone()
                };
                usage_metrics_snapshot_from_path(&path, &query)
            },
            DuckDbUsageRepositoryInner::Tiered {
                state,
                catalog_backend,
                ..
            } => usage_metrics_snapshot_from_tiered(state, catalog_backend.as_ref(), &query),
        })
        .await
        .context("duckdb usage metrics task failed")?
    }

    async fn kiro_latency_ranking_snapshot(
        &self,
        query: KiroLatencyRankingQuery,
    ) -> anyhow::Result<KiroLatencyRankingSnapshot> {
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || match inner.as_ref() {
            DuckDbUsageRepositoryInner::Single {
                state, ..
            } => {
                let path = {
                    let state = state
                        .lock()
                        .map_err(|_| anyhow!("single duckdb state lock poisoned"))?;
                    state.path.clone()
                };
                kiro_latency_ranking_snapshot_from_path(&path, &query)
            },
            DuckDbUsageRepositoryInner::Tiered {
                state,
                catalog_backend,
                ..
            } => kiro_latency_ranking_snapshot_from_tiered(state, catalog_backend.as_ref(), &query),
        })
        .await
        .context("duckdb kiro latency ranking task failed")?
    }
}
#[cfg(feature = "duckdb-runtime")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UsageFilterOptionField {
    Model,
    Account,
    Endpoint,
}
#[cfg(feature = "duckdb-runtime")]
impl UsageFilterOptionField {
    fn catalog_field_name(self) -> UsageCatalogFieldName {
        match self {
            Self::Model => UsageCatalogFieldName::Model,
            Self::Account => UsageCatalogFieldName::AccountName,
            Self::Endpoint => UsageCatalogFieldName::Endpoint,
        }
    }
}
#[cfg(feature = "duckdb-runtime")]
#[derive(Default)]
pub(crate) struct UsageMetricsSummaryAccumulator {
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
pub(crate) struct UsageMetricsGroupAccumulator {
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
pub(crate) struct UsageMetricsAccumulator {
    summary: UsageMetricsSummaryAccumulator,
    distinct_accounts: BTreeSet<String>,
    distinct_proxies: BTreeSet<String>,
    accounts: BTreeMap<String, UsageMetricsGroupAccumulator>,
    proxies: BTreeMap<String, UsageMetricsGroupAccumulator>,
    non_ok_status_codes: BTreeMap<i32, u64>,
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) struct UsageMetricsObservedRow {
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
#[cfg(feature = "duckdb-runtime")]
impl UsageMetricsAccumulator {
    fn observe(&mut self, row: UsageMetricsObservedRow) {
        let normalized_account_name = normalize_metrics_optional_string(row.account_name.clone());
        let normalized_proxy_source = normalize_metrics_optional_string(row.proxy_source.clone());
        let normalized_proxy_config_id =
            normalize_metrics_optional_string(row.proxy_config_id.clone());
        let normalized_proxy_config_name =
            normalize_metrics_optional_string(row.proxy_config_name.clone());
        let normalized_proxy_url = normalize_metrics_optional_string(row.proxy_url.clone());
        let is_ok = row.status_code == 200;

        self.summary.total_requests = self.summary.total_requests.saturating_add(1);
        if is_ok {
            self.summary.ok_requests = self.summary.ok_requests.saturating_add(1);
        } else {
            self.summary.non_ok_requests = self.summary.non_ok_requests.saturating_add(1);
            self.non_ok_status_codes
                .entry(row.status_code)
                .and_modify(|count| *count = count.saturating_add(1))
                .or_insert(1);
        }
        if let Some(value) = row.first_sse_write_ms {
            self.summary.first_token_sum_ms = self.summary.first_token_sum_ms.saturating_add(value);
            self.summary.first_token_samples = self.summary.first_token_samples.saturating_add(1);
            self.summary.max_first_token_ms =
                Some(self.summary.max_first_token_ms.unwrap_or(value).max(value));
        }
        if let Some(value) = row.latency_ms {
            self.summary.latency_sum_ms = self.summary.latency_sum_ms.saturating_add(value);
            self.summary.latency_samples = self.summary.latency_samples.saturating_add(1);
        }
        if let Some(value) = row.routing_wait_ms {
            self.summary.routing_wait_sum_ms =
                self.summary.routing_wait_sum_ms.saturating_add(value);
            self.summary.routing_wait_samples = self.summary.routing_wait_samples.saturating_add(1);
        }
        if row.quota_failover_count > 0 {
            self.summary.failover_request_count =
                self.summary.failover_request_count.saturating_add(1);
            self.summary.total_quota_failovers = self
                .summary
                .total_quota_failovers
                .saturating_add(row.quota_failover_count);
        }
        if row.downstream_disconnect {
            self.summary.downstream_disconnect_count =
                self.summary.downstream_disconnect_count.saturating_add(1);
        }
        if row.usage_missing {
            self.summary.usage_missing_count = self.summary.usage_missing_count.saturating_add(1);
        }
        if row.credit_usage_missing {
            self.summary.credit_usage_missing_count =
                self.summary.credit_usage_missing_count.saturating_add(1);
        }

        let account_key = metrics_account_key(normalized_account_name.as_deref());
        let account_label = metrics_account_label(normalized_account_name.as_deref());
        self.distinct_accounts.insert(account_key.clone());
        let account_group = self.accounts.entry(account_key.clone()).or_insert_with(|| {
            UsageMetricsGroupAccumulator {
                key: account_key.clone(),
                label: account_label.clone(),
                account_name: normalized_account_name.clone(),
                ..UsageMetricsGroupAccumulator::default()
            }
        });
        update_usage_metrics_group(account_group, &row, is_ok);

        let proxy_key = metrics_proxy_key(
            normalized_proxy_config_id.as_deref(),
            normalized_proxy_url.as_deref(),
            normalized_proxy_source.as_deref(),
        );
        let proxy_label = metrics_proxy_label(
            normalized_proxy_config_name.as_deref(),
            normalized_proxy_url.as_deref(),
            normalized_proxy_source.as_deref(),
        );
        self.distinct_proxies.insert(proxy_key.clone());
        let proxy_group =
            self.proxies
                .entry(proxy_key.clone())
                .or_insert_with(|| UsageMetricsGroupAccumulator {
                    key: proxy_key.clone(),
                    label: proxy_label.clone(),
                    proxy_config_id: normalized_proxy_config_id.clone(),
                    proxy_config_name: normalized_proxy_config_name.clone(),
                    proxy_url: normalized_proxy_url.clone(),
                    proxy_source: normalized_proxy_source.clone(),
                    ..UsageMetricsGroupAccumulator::default()
                });
        if proxy_group.proxy_config_id.is_none() {
            proxy_group.proxy_config_id = normalized_proxy_config_id.clone();
        }
        if proxy_group.proxy_config_name.is_none() {
            proxy_group.proxy_config_name = normalized_proxy_config_name.clone();
        }
        if proxy_group.proxy_url.is_none() {
            proxy_group.proxy_url = normalized_proxy_url.clone();
        }
        if proxy_group.proxy_source.is_none() {
            proxy_group.proxy_source = normalized_proxy_source.clone();
        }
        update_usage_metrics_group(proxy_group, &row, is_ok);
    }

    fn into_snapshot(self, query: &UsageMetricsQuery) -> UsageMetricsSnapshot {
        let top_limit = query.top_limit.max(1);
        let non_ok_status_codes = {
            let mut rows = self
                .non_ok_status_codes
                .into_iter()
                .map(|(status_code, request_count)| UsageMetricsStatusCodeView {
                    status_code,
                    request_count,
                })
                .collect::<Vec<_>>();
            rows.sort_by(|left, right| {
                right
                    .request_count
                    .cmp(&left.request_count)
                    .then_with(|| left.status_code.cmp(&right.status_code))
            });
            rows.truncate(top_limit);
            rows
        };
        UsageMetricsSnapshot {
            generated_at_ms: now_ms(),
            start_ms: query.start_ms,
            end_ms: query.end_ms,
            provider_type: query.provider_type.clone(),
            source: query.source,
            summary: UsageMetricsSummary {
                total_requests: self.summary.total_requests,
                ok_requests: self.summary.ok_requests,
                non_ok_requests: self.summary.non_ok_requests,
                distinct_accounts: self.distinct_accounts.len(),
                distinct_proxies: self.distinct_proxies.len(),
                first_token_samples: self.summary.first_token_samples,
                avg_first_token_ms: average_metric_ms(
                    self.summary.first_token_sum_ms,
                    self.summary.first_token_samples,
                ),
                max_first_token_ms: self.summary.max_first_token_ms,
                avg_latency_ms: average_metric_ms(
                    self.summary.latency_sum_ms,
                    self.summary.latency_samples,
                ),
                avg_routing_wait_ms: average_metric_ms(
                    self.summary.routing_wait_sum_ms,
                    self.summary.routing_wait_samples,
                ),
                failover_request_count: self.summary.failover_request_count,
                total_quota_failovers: self.summary.total_quota_failovers,
                downstream_disconnect_count: self.summary.downstream_disconnect_count,
                usage_missing_count: self.summary.usage_missing_count,
                credit_usage_missing_count: self.summary.credit_usage_missing_count,
            },
            top_first_token_accounts: top_usage_metrics_groups(
                &self.accounts,
                top_limit,
                |left, right| {
                    cmp_option_f64_desc(
                        average_metric_ms(left.first_token_sum_ms, left.first_token_samples),
                        average_metric_ms(right.first_token_sum_ms, right.first_token_samples),
                    )
                    .then_with(|| {
                        cmp_option_i64_desc(left.max_first_token_ms, right.max_first_token_ms)
                    })
                },
            ),
            top_first_token_proxies: top_usage_metrics_groups(
                &self.proxies,
                top_limit,
                |left, right| {
                    cmp_option_f64_desc(
                        average_metric_ms(left.first_token_sum_ms, left.first_token_samples),
                        average_metric_ms(right.first_token_sum_ms, right.first_token_samples),
                    )
                    .then_with(|| {
                        cmp_option_i64_desc(left.max_first_token_ms, right.max_first_token_ms)
                    })
                },
            ),
            top_non_ok_accounts: top_usage_metrics_groups(
                &self.accounts,
                top_limit,
                |left, right| {
                    right
                        .non_ok_count
                        .cmp(&left.non_ok_count)
                        .then_with(|| cmp_option_f64_desc(error_rate(left), error_rate(right)))
                },
            ),
            top_non_ok_proxies: top_usage_metrics_groups(
                &self.proxies,
                top_limit,
                |left, right| {
                    right
                        .non_ok_count
                        .cmp(&left.non_ok_count)
                        .then_with(|| cmp_option_f64_desc(error_rate(left), error_rate(right)))
                },
            ),
            top_routing_wait_accounts: top_usage_metrics_groups(
                &self.accounts,
                top_limit,
                |left, right| {
                    cmp_option_f64_desc(
                        average_metric_ms(left.routing_wait_sum_ms, left.routing_wait_samples),
                        average_metric_ms(right.routing_wait_sum_ms, right.routing_wait_samples),
                    )
                    .then_with(|| {
                        cmp_option_i64_desc(left.max_routing_wait_ms, right.max_routing_wait_ms)
                    })
                },
            ),
            top_routing_wait_proxies: top_usage_metrics_groups(
                &self.proxies,
                top_limit,
                |left, right| {
                    cmp_option_f64_desc(
                        average_metric_ms(left.routing_wait_sum_ms, left.routing_wait_samples),
                        average_metric_ms(right.routing_wait_sum_ms, right.routing_wait_samples),
                    )
                    .then_with(|| {
                        cmp_option_i64_desc(left.max_routing_wait_ms, right.max_routing_wait_ms)
                    })
                },
            ),
            top_failover_accounts: top_usage_metrics_groups(
                &self.accounts,
                top_limit,
                |left, right| {
                    right
                        .failover_request_count
                        .cmp(&left.failover_request_count)
                        .then_with(|| right.total_quota_failovers.cmp(&left.total_quota_failovers))
                },
            ),
            top_failover_proxies: top_usage_metrics_groups(
                &self.proxies,
                top_limit,
                |left, right| {
                    right
                        .failover_request_count
                        .cmp(&left.failover_request_count)
                        .then_with(|| right.total_quota_failovers.cmp(&left.total_quota_failovers))
                },
            ),
            top_disconnect_accounts: top_usage_metrics_groups(
                &self.accounts,
                top_limit,
                |left, right| {
                    right
                        .downstream_disconnect_count
                        .cmp(&left.downstream_disconnect_count)
                        .then_with(|| {
                            cmp_option_f64_desc(disconnect_rate(left), disconnect_rate(right))
                        })
                },
            ),
            top_disconnect_proxies: top_usage_metrics_groups(
                &self.proxies,
                top_limit,
                |left, right| {
                    right
                        .downstream_disconnect_count
                        .cmp(&left.downstream_disconnect_count)
                        .then_with(|| {
                            cmp_option_f64_desc(disconnect_rate(left), disconnect_rate(right))
                        })
                },
            ),
            non_ok_status_codes,
        }
    }

    fn into_kiro_latency_ranking(
        self,
        query: &KiroLatencyRankingQuery,
    ) -> KiroLatencyRankingSnapshot {
        KiroLatencyRankingSnapshot {
            generated_at_ms: now_ms(),
            start_ms: query.start_ms,
            end_ms: query.end_ms,
            source: query.source,
            first_token_samples: self.summary.first_token_samples,
            avg_first_token_ms: average_metric_ms(
                self.summary.first_token_sum_ms,
                self.summary.first_token_samples,
            ),
            accounts: kiro_latency_account_rows(&self.accounts),
            proxies: kiro_latency_proxy_rows(&self.proxies),
        }
    }
}

#[cfg(test)]
mod tests;
