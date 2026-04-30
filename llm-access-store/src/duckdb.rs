//! DuckDB analytics writer helpers for LLM usage events.

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
