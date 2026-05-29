//! SQL builders for the DuckDB usage fact/detail/rollup tables: insert
//! statements, compaction copy SQL, and dynamic select/filter/column
//! expression generation gated on the live table schema.

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

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
pub(crate) fn duckdb_compact_connection_sql(
    connection_config: DuckDbUsageConnectionConfig,
    temp_dir: &str,
) -> String {
    format!(
        "
        SET memory_limit={};
        SET threads=1;
        SET preserve_insertion_order=false;
        SET temp_directory={};
        SET max_temp_directory_size={};
        ",
        duckdb_string_literal(&format!("{}MB", connection_config.memory_limit_mib.max(1))),
        duckdb_string_literal(temp_dir),
        duckdb_string_literal(DUCKDB_COMPACT_MAX_TEMP_DIRECTORY_SIZE),
    )
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn compact_copy_usage_events_sql(columns: &HashSet<String>) -> String {
    let select = vec![
        compact_source_required_expr("source_seq"),
        compact_source_required_expr("source_event_id"),
        compact_source_required_expr("event_id"),
        compact_source_required_expr("created_at_ms"),
        compact_source_required_expr("created_at"),
        compact_source_required_expr("created_date"),
        compact_source_required_expr("created_hour"),
        compact_source_required_expr("provider_type"),
        compact_source_required_expr("protocol_family"),
        compact_source_required_expr("key_id"),
        compact_source_required_expr("key_name"),
        compact_source_required_expr("key_status_at_event"),
        compact_source_column_expr(columns, "account_name", "CAST(NULL AS VARCHAR)"),
        compact_source_column_expr(columns, "account_group_id_at_event", "CAST(NULL AS VARCHAR)"),
        compact_source_column_expr(columns, "route_strategy_at_event", "CAST(NULL AS VARCHAR)"),
        compact_source_column_expr(columns, "request_method", "'POST'"),
        compact_source_column_expr(columns, "request_url", "''"),
        compact_source_required_expr("endpoint"),
        compact_source_column_expr(columns, "model", "CAST(NULL AS VARCHAR)"),
        compact_source_column_expr(columns, "mapped_model", "CAST(NULL AS VARCHAR)"),
        compact_source_required_expr("status_code"),
        compact_source_column_expr(columns, "latency_ms", "CAST(NULL AS INTEGER)"),
        compact_source_column_expr(columns, "routing_wait_ms", "CAST(NULL AS INTEGER)"),
        compact_source_column_expr(columns, "upstream_headers_ms", "CAST(NULL AS INTEGER)"),
        compact_source_column_expr(columns, "post_headers_body_ms", "CAST(NULL AS INTEGER)"),
        compact_source_column_expr(columns, "request_body_read_ms", "CAST(NULL AS INTEGER)"),
        compact_source_column_expr(columns, "request_json_parse_ms", "CAST(NULL AS INTEGER)"),
        compact_source_column_expr(columns, "pre_handler_ms", "CAST(NULL AS INTEGER)"),
        compact_source_column_expr(columns, "first_sse_write_ms", "CAST(NULL AS INTEGER)"),
        compact_source_column_expr(columns, "stream_finish_ms", "CAST(NULL AS INTEGER)"),
        compact_source_column_expr(columns, "stream_completed_cleanly", "CAST(NULL AS BOOLEAN)"),
        compact_source_column_expr(columns, "downstream_disconnect", "CAST(NULL AS BOOLEAN)"),
        compact_source_column_expr(columns, "final_event_type", "CAST(NULL AS VARCHAR)"),
        compact_source_column_expr(columns, "bytes_streamed", "CAST(NULL AS BIGINT)"),
        compact_source_column_expr(columns, "request_body_bytes", "CAST(NULL AS BIGINT)"),
        compact_source_column_expr(columns, "quota_failover_count", "CAST(0 AS BIGINT)"),
        compact_source_required_expr("input_uncached_tokens"),
        compact_source_required_expr("input_cached_tokens"),
        compact_source_required_expr("output_tokens"),
        compact_source_required_expr("billable_tokens"),
        compact_source_expr(
            columns,
            "credit_usage",
            "CAST(e.credit_usage AS VARCHAR)",
            "CAST(NULL AS VARCHAR)",
        ),
        compact_source_column_expr(columns, "usage_missing", "false"),
        compact_source_column_expr(columns, "credit_usage_missing", "true"),
        compact_source_column_expr(columns, "client_ip", "CAST(NULL AS VARCHAR)"),
        compact_source_column_expr(columns, "ip_region", "CAST(NULL AS VARCHAR)"),
        compact_source_column_expr(columns, "request_headers_json", "'{}'"),
        compact_source_column_expr(columns, "routing_diagnostics_json", "CAST(NULL AS VARCHAR)"),
        compact_source_column_expr(columns, "last_message_content", "CAST(NULL AS VARCHAR)"),
        compact_detail_object_payload_present_expr(columns),
        compact_source_column_expr(columns, "detail_object_path", "CAST(NULL AS VARCHAR)"),
        compact_source_column_expr(columns, "detail_object_offset", "CAST(NULL AS BIGINT)"),
        compact_source_column_expr(columns, "detail_object_length", "CAST(NULL AS BIGINT)"),
        compact_source_column_expr(columns, "detail_object_sha256", "CAST(NULL AS VARCHAR)"),
        compact_source_column_expr(columns, "proxy_source_at_event", "CAST(NULL AS VARCHAR)"),
        compact_source_column_expr(columns, "proxy_config_id_at_event", "CAST(NULL AS VARCHAR)"),
        compact_source_column_expr(columns, "proxy_config_name_at_event", "CAST(NULL AS VARCHAR)"),
        compact_source_column_expr(columns, "proxy_url_at_event", "CAST(NULL AS VARCHAR)"),
    ]
    .join(",\n        ");

    format!(
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
    )
    SELECT
        {select}
    FROM pending_segment.usage_events e;"
    )
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn compact_source_required_expr(column: &'static str) -> String {
    format!("e.{column} AS {column}")
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn compact_source_column_expr(
    columns: &HashSet<String>,
    column: &'static str,
    missing_sql: &'static str,
) -> String {
    compact_source_expr(columns, column, &format!("e.{column}"), missing_sql)
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn compact_source_expr(
    columns: &HashSet<String>,
    column: &'static str,
    present_sql: &str,
    missing_sql: &'static str,
) -> String {
    let sql = if columns.contains(column) { present_sql } else { missing_sql };
    format!("{sql} AS {column}")
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn usage_event_filter_column_sql(
    columns: &HashSet<String>,
    table_alias: &str,
    column: &'static str,
    missing_sql: &'static str,
) -> String {
    if columns.contains(column) {
        format!("{table_alias}.{column}")
    } else {
        missing_sql.to_string()
    }
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn usage_event_filter_where_sql(columns: &HashSet<String>, table_alias: &str) -> String {
    let model_sql =
        usage_event_filter_column_sql(columns, table_alias, "model", "CAST(NULL AS VARCHAR)");
    let account_name_sql = usage_event_filter_column_sql(
        columns,
        table_alias,
        "account_name",
        "CAST(NULL AS VARCHAR)",
    );
    let endpoint_sql =
        usage_event_filter_column_sql(columns, table_alias, "endpoint", "CAST(NULL AS VARCHAR)");
    let status_code_sql =
        usage_event_filter_column_sql(columns, table_alias, "status_code", "CAST(NULL AS INTEGER)");
    format!(
        "WHERE (?1 IS NULL OR {table_alias}.key_id = ?1)
      AND (?2 IS NULL OR {table_alias}.provider_type = ?2)
      AND (?3 IS NULL OR {table_alias}.created_at_ms >= ?3)
      AND (?4 IS NULL OR {table_alias}.created_at_ms < ?4)
      AND (?5 IS NULL OR {model_sql} = ?5)
      AND (?6 IS NULL OR {account_name_sql} = ?6)
      AND (?7 IS NULL OR {endpoint_sql} = ?7)
      AND (?8 IS NULL OR {status_code_sql} = ?8)
      AND (?9 IS NULL
           OR (?9 = 'ok' AND {status_code_sql} = 200)
           OR (?9 = 'non_ok' AND {status_code_sql} <> 200))"
    )
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn list_usage_event_summaries_sql(conn: &duckdb::Connection) -> anyhow::Result<String> {
    let columns = duckdb_table_columns(conn, "usage_events")?;
    let select = usage_event_summary_select_exprs(&columns).join(",\n        ");
    let where_sql = usage_event_filter_where_sql(&columns, "e");
    Ok(format!(
        "SELECT {select}
    FROM usage_events e
    {where_sql}
    LIMIT ?10 OFFSET ?11"
    ))
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn usage_event_totals_sql(conn: &duckdb::Connection) -> anyhow::Result<String> {
    let columns = duckdb_table_columns(conn, "usage_events")?;
    let where_sql = usage_event_filter_where_sql(&columns, "e");
    Ok(format!(
        "SELECT
            count(*) AS event_count,
            COALESCE(sum(e.input_uncached_tokens), 0) AS input_uncached_tokens,
            COALESCE(sum(e.input_cached_tokens), 0) AS input_cached_tokens,
            COALESCE(sum(e.output_tokens), 0) AS output_tokens,
            COALESCE(sum(e.billable_tokens), 0) AS billable_tokens
         FROM usage_events e
         {where_sql}"
    ))
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn usage_event_summary_select_exprs(columns: &HashSet<String>) -> Vec<String> {
    let mut exprs = usage_event_base_select_exprs(columns, false, false);
    exprs.push("CAST(NULL AS VARCHAR) AS last_message_content".to_string());
    exprs
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn usage_event_base_select_exprs(
    columns: &HashSet<String>,
    include_detail_payload: bool,
    detail_table_exists: bool,
) -> Vec<String> {
    vec![
        usage_event_required_expr("event_id"),
        usage_event_required_expr("created_at_ms"),
        usage_event_required_expr("provider_type"),
        usage_event_required_expr("protocol_family"),
        usage_event_required_expr("key_id"),
        usage_event_column_expr(columns, "key_name", "e.key_id"),
        usage_event_column_expr(columns, "account_name", "CAST(NULL AS VARCHAR)"),
        usage_event_column_expr(columns, "account_group_id_at_event", "CAST(NULL AS VARCHAR)"),
        usage_event_column_expr(columns, "route_strategy_at_event", "CAST(NULL AS VARCHAR)"),
        usage_event_column_expr(columns, "request_method", "'POST'"),
        usage_event_column_expr(columns, "request_url", "''"),
        usage_event_required_expr("endpoint"),
        usage_event_column_expr(columns, "model", "CAST(NULL AS VARCHAR)"),
        usage_event_column_expr(columns, "mapped_model", "CAST(NULL AS VARCHAR)"),
        usage_event_required_expr("status_code"),
        usage_event_column_expr(columns, "request_body_bytes", "CAST(NULL AS BIGINT)"),
        usage_event_column_expr(columns, "quota_failover_count", "CAST(0 AS BIGINT)"),
        if include_detail_payload {
            usage_event_detail_payload_expr(
                columns,
                detail_table_exists,
                "routing_diagnostics_json",
                "CAST(NULL AS VARCHAR)",
            )
        } else {
            "CAST(NULL AS VARCHAR) AS routing_diagnostics_json".to_string()
        },
        usage_event_required_expr("input_uncached_tokens"),
        usage_event_required_expr("input_cached_tokens"),
        usage_event_required_expr("output_tokens"),
        usage_event_required_expr("billable_tokens"),
        usage_event_expr(
            columns,
            "credit_usage",
            "CAST(credit_usage AS VARCHAR)",
            "CAST(NULL AS VARCHAR)",
        ),
        usage_event_column_expr(columns, "usage_missing", "false"),
        usage_event_column_expr(columns, "credit_usage_missing", "true"),
        usage_event_column_expr(columns, "latency_ms", "CAST(NULL AS INTEGER)"),
        usage_event_column_expr(columns, "routing_wait_ms", "CAST(NULL AS INTEGER)"),
        usage_event_column_expr(columns, "upstream_headers_ms", "CAST(NULL AS INTEGER)"),
        usage_event_column_expr(columns, "post_headers_body_ms", "CAST(NULL AS INTEGER)"),
        usage_event_column_expr(columns, "request_body_read_ms", "CAST(NULL AS INTEGER)"),
        usage_event_column_expr(columns, "request_json_parse_ms", "CAST(NULL AS INTEGER)"),
        usage_event_column_expr(columns, "pre_handler_ms", "CAST(NULL AS INTEGER)"),
        usage_event_column_expr(columns, "first_sse_write_ms", "CAST(NULL AS INTEGER)"),
        usage_event_column_expr(columns, "stream_finish_ms", "CAST(NULL AS INTEGER)"),
        usage_event_column_expr(columns, "stream_completed_cleanly", "CAST(NULL AS BOOLEAN)"),
        usage_event_column_expr(columns, "downstream_disconnect", "CAST(NULL AS BOOLEAN)"),
        usage_event_column_expr(columns, "final_event_type", "CAST(NULL AS VARCHAR)"),
        usage_event_column_expr(columns, "bytes_streamed", "CAST(NULL AS BIGINT)"),
        usage_event_column_expr(columns, "client_ip", "CAST(NULL AS VARCHAR)"),
        usage_event_column_expr(columns, "ip_region", "CAST(NULL AS VARCHAR)"),
    ]
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn usage_event_required_expr(column: &'static str) -> String {
    format!("e.{column} AS {column}")
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn usage_event_column_expr(
    columns: &HashSet<String>,
    column: &'static str,
    missing_sql: &'static str,
) -> String {
    usage_event_expr(columns, column, &format!("e.{column}"), missing_sql)
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn usage_event_expr(
    columns: &HashSet<String>,
    column: &'static str,
    present_sql: &str,
    missing_sql: &'static str,
) -> String {
    let sql = if columns.contains(column) { present_sql } else { missing_sql };
    format!("{sql} AS {column}")
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn duckdb_usage_connection_sql(
    connection_config: &DuckDbUsageConnectionConfig,
    temp_dir_str: &str,
) -> String {
    format!(
        "
        SET memory_limit={};
        SET checkpoint_threshold={};
        SET threads=1;
        SET preserve_insertion_order=false;
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
pub(crate) fn query_segment_field_rollup_sql(
    conn: &duckdb::Connection,
    field_name: UsageCatalogFieldName,
    sql: &str,
    scoped: bool,
) -> anyhow::Result<Vec<SegmentFieldRollup>> {
    let mut stmt = conn
        .prepare(sql)
        .with_context(|| format!("prepare duckdb segment field rollup query `{sql}`"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(SegmentFieldRollup {
                key_id: if scoped { row.get(0)? } else { None },
                provider_type: if scoped { row.get(1)? } else { None },
                field_name,
                field_value: row.get(2)?,
                row_count: i64_to_usize(row.get(3)?),
                input_uncached_tokens: row.get(4)?,
                input_cached_tokens: row.get(5)?,
                output_tokens: row.get(6)?,
                billable_tokens: row.get(7)?,
                first_used_at_ms: row.get(8)?,
                last_used_at_ms: row.get(9)?,
            })
        })
        .with_context(|| format!("query duckdb segment field rollups `{sql}`"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .context("collect duckdb segment field rollups")
}
