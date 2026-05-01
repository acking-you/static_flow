//! DuckDB analytics writer helpers for LLM usage events.

#[cfg(feature = "duckdb-runtime")]
use std::path::{Path, PathBuf};

#[cfg(feature = "duckdb-runtime")]
use anyhow::Context;
#[cfg(feature = "duckdb-runtime")]
use async_trait::async_trait;
#[cfg(feature = "duckdb-runtime")]
use llm_access_core::{
    provider::{ProtocolFamily, ProviderType, RouteStrategy},
    store::{
        UsageAnalyticsStore, UsageChartPoint, UsageEventPage, UsageEventQuery, UsageEventSink,
    },
    usage::{UsageEvent, UsageTiming},
};
#[cfg(feature = "duckdb-runtime")]
use tokio::task;

#[cfg(feature = "duckdb-runtime")]
use crate::KeyUsageRollupSummary;

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
const USAGE_EVENT_SUMMARY_LAST_MESSAGE_MAX_CHARS: usize = 2_048;

#[cfg(feature = "duckdb-runtime")]
const LIST_USAGE_EVENT_SUMMARIES_SQL: &str = "SELECT event_id, created_at_ms,
        provider_type, protocol_family, key_id, key_name, account_name,
        account_group_id_at_event, route_strategy_at_event, request_method,
        request_url, endpoint, model, mapped_model, status_code,
        request_body_bytes, quota_failover_count, routing_diagnostics_json,
        input_uncached_tokens, input_cached_tokens, output_tokens,
        billable_tokens, CAST(credit_usage AS VARCHAR), usage_missing,
        credit_usage_missing, latency_ms, routing_wait_ms, upstream_headers_ms,
        post_headers_body_ms, request_body_read_ms, request_json_parse_ms,
        pre_handler_ms, first_sse_write_ms, stream_finish_ms, client_ip,
        ip_region,
        CASE
            WHEN last_message_content IS NULL THEN NULL
            ELSE left(last_message_content, ?5)
        END AS last_message_content
    FROM usage_events
    WHERE (?1 IS NULL OR key_id = ?1)
      AND (?2 IS NULL OR provider_type = ?2)
    LIMIT ?3 OFFSET ?4";

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
    crate::initialize_duckdb_target(&conn)
}

/// File-backed DuckDB usage-event repository.
#[cfg(feature = "duckdb-runtime")]
#[derive(Debug, Clone)]
pub struct DuckDbUsageRepository {
    path: PathBuf,
}

#[cfg(feature = "duckdb-runtime")]
impl DuckDbUsageRepository {
    /// Open a DuckDB usage repository and initialize the analytics schema.
    pub fn open_path(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref().to_path_buf();
        initialize_duckdb_target_path(&path)?;
        Ok(Self {
            path,
        })
    }

    fn open_conn(path: &Path) -> anyhow::Result<duckdb::Connection> {
        duckdb::Connection::open(path)
            .with_context(|| format!("failed to open duckdb database `{}`", path.display()))
    }

    /// Aggregate all persisted usage events into per-key operational rollups.
    pub async fn key_usage_rollups(&self) -> anyhow::Result<Vec<KeyUsageRollupSummary>> {
        let path = self.path.clone();
        task::spawn_blocking(move || {
            let conn = Self::open_conn(&path)?;
            let mut stmt = conn
                .prepare(
                    "SELECT
                        key_id,
                        CAST(COALESCE(sum(input_uncached_tokens), 0) AS BIGINT),
                        CAST(COALESCE(sum(input_cached_tokens), 0) AS BIGINT),
                        CAST(COALESCE(sum(output_tokens), 0) AS BIGINT),
                        CAST(COALESCE(sum(billable_tokens), 0) AS BIGINT),
                        CAST(COALESCE(sum(COALESCE(try_cast(credit_usage AS DOUBLE), 0)), 0) AS \
                     VARCHAR),
                        CAST(COALESCE(sum(CASE WHEN credit_usage_missing THEN 1 ELSE 0 END), 0) AS \
                     BIGINT),
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
        })
        .await
        .context("duckdb key usage rollup task failed")?
    }
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
        let path = self.path.clone();
        let rows = events
            .iter()
            .map(UsageEventRow::from_usage_event)
            .collect::<Vec<_>>();
        task::spawn_blocking(move || {
            let mut writer = DuckDbUsageWriter::new(Self::open_conn(&path)?)?;
            writer.insert_usage_events(&rows)
        })
        .await
        .context("duckdb usage insert task failed")?
    }
}

#[cfg(feature = "duckdb-runtime")]
#[async_trait]
impl UsageAnalyticsStore for DuckDbUsageRepository {
    async fn list_usage_events(&self, query: UsageEventQuery) -> anyhow::Result<UsageEventPage> {
        let path = self.path.clone();
        task::spawn_blocking(move || {
            let conn = Self::open_conn(&path)?;
            let key_filter = query.key_id.as_deref();
            let provider_filter = query.provider_type.as_deref();
            let total: i64 = conn
                .query_row(
                    "SELECT count(*) FROM usage_events
                     WHERE (?1 IS NULL OR key_id = ?1)
                       AND (?2 IS NULL OR provider_type = ?2)",
                    duckdb::params![key_filter, provider_filter],
                    |row| row.get(0),
                )
                .context("count duckdb usage events")?;
            let total = total.max(0) as usize;
            if query.limit == 0 || query.offset >= total {
                return Ok(UsageEventPage {
                    total,
                    offset: query.offset,
                    limit: query.limit,
                    has_more: false,
                    events: Vec::new(),
                });
            }
            let fetch_count = total.saturating_sub(query.offset).min(query.limit);
            let reverse_offset = total.saturating_sub(query.offset.saturating_add(fetch_count));
            let mut stmt = conn
                .prepare(LIST_USAGE_EVENT_SUMMARIES_SQL)
                .context("prepare duckdb usage event summary query")?;
            let rows = stmt
                .query_map(
                    duckdb::params![
                        key_filter,
                        provider_filter,
                        fetch_count as i64,
                        reverse_offset as i64,
                        USAGE_EVENT_SUMMARY_LAST_MESSAGE_MAX_CHARS as i64
                    ],
                    decode_usage_event_summary_row,
                )
                .context("query duckdb usage events")?;
            let mut events = rows.collect::<Result<Vec<_>, _>>()?;
            events.sort_by(|left, right| right.created_at_ms.cmp(&left.created_at_ms));
            Ok(UsageEventPage {
                total,
                offset: query.offset,
                limit: query.limit,
                has_more: query.offset.saturating_add(events.len()) < total,
                events,
            })
        })
        .await
        .context("duckdb usage event list task failed")?
    }

    async fn get_usage_event(&self, event_id: &str) -> anyhow::Result<Option<UsageEvent>> {
        let path = self.path.clone();
        let event_id = event_id.to_string();
        task::spawn_blocking(move || {
            let conn = Self::open_conn(&path)?;
            let mut stmt = conn
                .prepare(GET_USAGE_EVENT_DETAIL_SQL)
                .context("prepare duckdb usage event detail query")?;
            match stmt.query_row(duckdb::params![event_id], decode_usage_event_detail_row) {
                Ok(event) => Ok(Some(event)),
                Err(duckdb::Error::QueryReturnedNoRows) => Ok(None),
                Err(err) => Err(err).context("query duckdb usage event detail"),
            }
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
        let path = self.path.clone();
        let key_id = key_id.to_string();
        task::spawn_blocking(move || {
            let mut points = (0..bucket_count)
                .map(|index| UsageChartPoint {
                    bucket_start_ms: start_ms
                        .saturating_add((index as i64).saturating_mul(bucket_ms)),
                    tokens: 0,
                })
                .collect::<Vec<_>>();
            if bucket_count == 0 {
                return Ok(points);
            }
            let end_ms = start_ms.saturating_add((bucket_count as i64).saturating_mul(bucket_ms));
            let conn = Self::open_conn(&path)?;
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
                        point.tokens = tokens.max(0) as u64;
                    }
                }
            }
            Ok(points)
        })
        .await
        .context("duckdb usage chart task failed")?
    }
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
        store::{UsageAnalyticsStore, UsageEventQuery, UsageEventSink},
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
        expected_summary.last_message_content =
            expected_summary.last_message_content.map(|value| {
                value
                    .chars()
                    .take(super::USAGE_EVENT_SUMMARY_LAST_MESSAGE_MAX_CHARS)
                    .collect()
            });
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
        event.last_message_content =
            Some("x".repeat(super::USAGE_EVENT_SUMMARY_LAST_MESSAGE_MAX_CHARS.saturating_add(10)));

        repo.append_usage_event(&event)
            .await
            .expect("append duckdb usage event");

        let page = repo
            .list_usage_events(UsageEventQuery {
                key_id: Some(event.key_id.clone()),
                provider_type: None,
                limit: 10,
                offset: 0,
            })
            .await
            .expect("list duckdb usage events");
        assert_eq!(page.total, 1);
        assert_eq!(page.events.len(), 1);
        assert_usage_event_summary_round_trips(&page.events[0], &event);
        assert_eq!(page.events[0].request_headers_json, "{}");
        assert_eq!(
            page.events[0]
                .last_message_content
                .as_ref()
                .map(String::len),
            Some(super::USAGE_EVENT_SUMMARY_LAST_MESSAGE_MAX_CHARS)
        );
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

        std::fs::remove_dir_all(&root).expect("cleanup duckdb test directory");
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
