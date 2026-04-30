//! DuckDB analytics writer helpers for LLM usage events.

/// One row for the DuckDB `usage_events` wide fact table.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageEventRow {
    /// Stable usage event id.
    pub event_id: String,
    /// Request id associated with the usage event.
    pub request_id: String,
    /// Event creation timestamp in Unix milliseconds.
    pub created_at_ms: i64,
    /// API key id at event time.
    pub key_id: String,
    /// API key display name at event time.
    pub key_name: String,
    /// Provider type at event time.
    pub provider_type: String,
    /// Protocol family at event time.
    pub protocol_family: String,
    /// Upstream account name at event time.
    pub account_name: Option<String>,
    /// Account group id captured at event time.
    pub account_group_id_at_event: Option<String>,
    /// Route strategy captured at event time.
    pub route_strategy_at_event: Option<String>,
    /// Provider endpoint.
    pub endpoint: String,
    /// Requested model name.
    pub model: String,
    /// Mapped upstream model name.
    pub mapped_model: Option<String>,
    /// Final HTTP status code.
    pub status_code: i64,
    /// Whether the request used streaming.
    pub stream: bool,
    /// Latency until upstream headers in milliseconds.
    pub upstream_headers_ms: Option<i64>,
    /// Latency until first SSE event in milliseconds.
    pub first_sse_ms: Option<i64>,
    /// Latency until stream completion in milliseconds.
    pub stream_finish_ms: Option<i64>,
    /// Uncached input tokens.
    pub input_uncached_tokens: i64,
    /// Cached input tokens.
    pub input_cached_tokens: i64,
    /// Output tokens.
    pub output_tokens: i64,
    /// Billable tokens.
    pub billable_tokens: i64,
    /// Total credit charged for this event.
    pub credit_total: Option<f64>,
}

/// Return the insert statement for the DuckDB `usage_events` wide fact table.
pub fn insert_usage_event_sql() -> &'static str {
    "INSERT INTO usage_events (
        event_id, request_id, created_at_ms, key_id, key_name, provider_type,
        protocol_family, account_name, account_group_id_at_event, route_strategy_at_event,
        endpoint, model, mapped_model, status_code, stream, upstream_headers_ms,
        first_sse_ms, stream_finish_ms, input_uncached_tokens, input_cached_tokens,
        output_tokens, billable_tokens, credit_total
     ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
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
                &row.event_id,
                &row.request_id,
                row.created_at_ms,
                &row.key_id,
                &row.key_name,
                &row.provider_type,
                &row.protocol_family,
                row.account_name.as_deref(),
                row.account_group_id_at_event.as_deref(),
                row.route_strategy_at_event.as_deref(),
                &row.endpoint,
                &row.model,
                row.mapped_model.as_deref(),
                row.status_code,
                row.stream,
                row.upstream_headers_ms,
                row.first_sse_ms,
                row.stream_finish_ms,
                row.input_uncached_tokens,
                row.input_cached_tokens,
                row.output_tokens,
                row.billable_tokens,
                row.credit_total.map(|value| value.to_string()),
            ])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn usage_insert_sql_targets_wide_fact_table_without_runtime_joins() {
        let sql = super::insert_usage_event_sql();

        assert!(sql.starts_with("INSERT INTO usage_events"));
        assert!(sql.contains("key_name"));
        assert!(sql.contains("account_group_id_at_event"));
        assert!(sql.contains("route_strategy_at_event"));
        assert!(!sql.to_ascii_lowercase().contains(" join "));
    }
}
