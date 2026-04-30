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
    /// Time until first downstream SSE write.
    pub first_sse_write_ms: Option<i64>,
    /// Time until stream finish.
    pub stream_finish_ms: Option<i64>,
    /// Request body size in bytes.
    pub request_body_bytes: Option<i64>,
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
}

impl UsageEventRow {
    /// Build a DuckDB fact row from the provider-neutral event.
    pub fn from_usage_event(event: &llm_access_core::usage::UsageEvent) -> Self {
        let latency_ms = event.timing.stream_finish_ms.or_else(|| {
            match (event.timing.upstream_headers_ms, event.timing.post_headers_body_ms) {
                (Some(headers), Some(body)) => Some(headers.saturating_add(body)),
                _ => None,
            }
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
            account_group_id_at_event: None,
            route_strategy_at_event: event
                .route_strategy_at_event
                .map(|strategy| strategy.as_storage_str().to_string()),
            endpoint: event.endpoint.clone(),
            model: event.model.clone(),
            mapped_model: event.mapped_model.clone(),
            status_code: event.status_code,
            latency_ms,
            routing_wait_ms: None,
            upstream_headers_ms: event.timing.upstream_headers_ms,
            post_headers_body_ms: event.timing.post_headers_body_ms,
            first_sse_write_ms: event.timing.first_sse_write_ms,
            stream_finish_ms: event.timing.stream_finish_ms,
            request_body_bytes: event.request_body_bytes,
            input_uncached_tokens: event.input_uncached_tokens,
            input_cached_tokens: event.input_cached_tokens,
            output_tokens: event.output_tokens,
            billable_tokens: event.billable_tokens,
            credit_usage: event.credit_usage.clone(),
            usage_missing: event.usage_missing,
            credit_usage_missing: event.credit_usage_missing,
            client_ip: None,
            ip_region: None,
        }
    }
}

/// Return the insert statement for the DuckDB `usage_events` wide fact table.
pub fn insert_usage_event_sql() -> &'static str {
    "INSERT INTO usage_events (
        source_seq, source_event_id, event_id, created_at_ms, created_at,
        created_date, created_hour, provider_type, protocol_family, key_id,
        key_name, key_status_at_event, account_name, account_group_id_at_event,
        route_strategy_at_event, endpoint, model, mapped_model, status_code,
        latency_ms, routing_wait_ms, upstream_headers_ms, post_headers_body_ms,
        first_sse_write_ms, stream_finish_ms, request_body_bytes,
        input_uncached_tokens, input_cached_tokens, output_tokens, billable_tokens,
        credit_usage, usage_missing, credit_usage_missing, client_ip, ip_region
     ) VALUES (
        ?1, ?2, ?3, ?4, to_timestamp(?4 / 1000.0),
        CAST(to_timestamp(?4 / 1000.0) AS DATE),
        date_trunc('hour', to_timestamp(?4 / 1000.0)),
        ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18,
        ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31, ?32
     )"
}

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
    pub fn insert_usage_event(&self, row: &UsageEventRow) -> anyhow::Result<()> {
        self.conn
            .execute(insert_usage_event_sql(), duckdb::params![
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
                &row.endpoint,
                row.model.as_deref(),
                row.mapped_model.as_deref(),
                row.status_code,
                row.latency_ms,
                row.routing_wait_ms,
                row.upstream_headers_ms,
                row.post_headers_body_ms,
                row.first_sse_write_ms,
                row.stream_finish_ms,
                row.request_body_bytes,
                row.input_uncached_tokens,
                row.input_cached_tokens,
                row.output_tokens,
                row.billable_tokens,
                row.credit_usage.as_deref(),
                row.usage_missing,
                row.credit_usage_missing,
                row.client_ip.as_deref(),
                row.ip_region.as_deref(),
            ])?;
        Ok(())
    }
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
}

#[cfg(feature = "duckdb-runtime")]
#[async_trait]
impl UsageEventSink for DuckDbUsageRepository {
    async fn append_usage_event(&self, event: &UsageEvent) -> anyhow::Result<()> {
        let path = self.path.clone();
        let row = UsageEventRow::from_usage_event(event);
        task::spawn_blocking(move || {
            let writer = DuckDbUsageWriter::new(Self::open_conn(&path)?)?;
            writer.insert_usage_event(&row)
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
            let total: i64 = conn
                .query_row(
                    "SELECT count(*) FROM usage_events WHERE (?1 IS NULL OR key_id = ?1)",
                    duckdb::params![key_filter],
                    |row| row.get(0),
                )
                .context("count duckdb usage events")?;
            let mut stmt = conn
                .prepare(
                    "SELECT event_id, created_at_ms, provider_type, protocol_family, key_id,
                            key_name, account_name, route_strategy_at_event, endpoint, model,
                            mapped_model, status_code, request_body_bytes,
                            input_uncached_tokens, input_cached_tokens, output_tokens,
                            billable_tokens, CAST(credit_usage AS VARCHAR), usage_missing,
                            credit_usage_missing, upstream_headers_ms, post_headers_body_ms,
                            first_sse_write_ms, stream_finish_ms
                     FROM usage_events
                     WHERE (?1 IS NULL OR key_id = ?1)
                     ORDER BY created_at_ms DESC
                     LIMIT ?2 OFFSET ?3",
                )
                .context("prepare duckdb usage event query")?;
            let rows = stmt
                .query_map(
                    duckdb::params![key_filter, query.limit as i64, query.offset as i64],
                    decode_usage_event_row,
                )
                .context("query duckdb usage events")?;
            let events = rows.collect::<Result<Vec<_>, _>>()?;
            let total = total.max(0) as usize;
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
                .prepare(
                    "SELECT event_id, created_at_ms, provider_type, protocol_family, key_id,
                            key_name, account_name, route_strategy_at_event, endpoint, model,
                            mapped_model, status_code, request_body_bytes,
                            input_uncached_tokens, input_cached_tokens, output_tokens,
                            billable_tokens, CAST(credit_usage AS VARCHAR), usage_missing,
                            credit_usage_missing, upstream_headers_ms, post_headers_body_ms,
                            first_sse_write_ms, stream_finish_ms
                     FROM usage_events
                     WHERE event_id = ?1",
                )
                .context("prepare duckdb usage event detail query")?;
            match stmt.query_row(duckdb::params![event_id], decode_usage_event_row) {
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
fn decode_usage_event_row(row: &duckdb::Row<'_>) -> duckdb::Result<UsageEvent> {
    let provider_type_raw: String = row.get(2)?;
    let protocol_family_raw: String = row.get(3)?;
    let route_strategy_raw: Option<String> = row.get(7)?;
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
                7,
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
        route_strategy_at_event,
        endpoint: row.get(8)?,
        model: row.get(9)?,
        mapped_model: row.get(10)?,
        status_code: row.get(11)?,
        request_body_bytes: row.get(12)?,
        input_uncached_tokens: row.get(13)?,
        input_cached_tokens: row.get(14)?,
        output_tokens: row.get(15)?,
        billable_tokens: row.get(16)?,
        credit_usage: row.get(17)?,
        usage_missing: row.get(18)?,
        credit_usage_missing: row.get(19)?,
        timing: UsageTiming {
            upstream_headers_ms: row.get(20)?,
            post_headers_body_ms: row.get(21)?,
            first_sse_write_ms: row.get(22)?,
            stream_finish_ms: row.get(23)?,
        },
    })
}

#[cfg(test)]
mod tests {
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
}
