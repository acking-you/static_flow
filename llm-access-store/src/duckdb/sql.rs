//! DuckDB SQL builders: insert/compact/select-expr/filter SQL for the
//! usage-event fact and detail tables.

use std::collections::HashSet;

use anyhow::Context;
use duckdb::OptionalExt;

use super::{
    util::duckdb_string_literal, DuckDbUsageConnectionConfig,
    DUCKDB_COMPACT_MAX_TEMP_DIRECTORY_SIZE,
};

#[cfg(feature = "duckdb-runtime")]
pub fn insert_usage_event_detail_sql() -> &'static str {
    "INSERT INTO usage_event_details (
        event_id, request_headers_json, routing_diagnostics_json,
        last_message_content, client_request_body_json,
        upstream_request_body_json, full_request_json, error_message,
        error_body
     ) VALUES (
        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9
     )
     ON CONFLICT DO NOTHING"
}
#[cfg(feature = "duckdb-runtime")]
pub fn duckdb_compact_connection_sql(
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
pub fn compact_copy_usage_events_sql(columns: &HashSet<String>) -> String {
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
fn compact_detail_object_payload_present_expr(columns: &HashSet<String>) -> String {
    if columns.contains("detail_object_payload_present") {
        return "COALESCE(e.detail_object_payload_present, false) AS detail_object_payload_present"
            .to_string();
    }
    let mut payload_checks = Vec::new();
    for column in ["client_request_body_json", "upstream_request_body_json", "full_request_json"] {
        if columns.contains(column) {
            payload_checks
                .push(format!("length(trim(COALESCE(CAST(e.{column} AS VARCHAR), ''))) > 0"));
        }
    }
    if payload_checks.is_empty() {
        "CAST(false AS BOOLEAN) AS detail_object_payload_present".to_string()
    } else {
        format!("({}) AS detail_object_payload_present", payload_checks.join(" OR "))
    }
}
#[cfg(feature = "duckdb-runtime")]
fn compact_source_required_expr(column: &'static str) -> String {
    format!("e.{column} AS {column}")
}
#[cfg(feature = "duckdb-runtime")]
fn compact_source_column_expr(
    columns: &HashSet<String>,
    column: &'static str,
    missing_sql: &'static str,
) -> String {
    compact_source_expr(columns, column, &format!("e.{column}"), missing_sql)
}
#[cfg(feature = "duckdb-runtime")]
fn compact_source_expr(
    columns: &HashSet<String>,
    column: &'static str,
    present_sql: &str,
    missing_sql: &'static str,
) -> String {
    let sql = if columns.contains(column) { present_sql } else { missing_sql };
    format!("{sql} AS {column}")
}
#[cfg(feature = "duckdb-runtime")]
pub fn usage_event_filter_column_sql(
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
fn usage_event_filter_where_sql(columns: &HashSet<String>, table_alias: &str) -> String {
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
pub fn list_usage_event_summaries_sql(conn: &duckdb::Connection) -> anyhow::Result<String> {
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
pub fn usage_event_totals_sql(conn: &duckdb::Connection) -> anyhow::Result<String> {
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
pub fn get_usage_event_detail_sql(conn: &duckdb::Connection) -> anyhow::Result<String> {
    let columns = duckdb_table_columns(conn, "usage_events")?;
    let detail_table_exists = duckdb_relation_exists(conn, "usage_event_details");
    let select = usage_event_detail_select_exprs(&columns, detail_table_exists).join(",\n        ");
    let from_sql = if detail_table_exists {
        "FROM usage_events e
    LEFT JOIN usage_event_details d ON d.event_id = e.event_id"
    } else {
        "FROM usage_events e"
    };
    Ok(format!("SELECT {select}\n    {from_sql}\n    WHERE e.event_id = ?1"))
}
#[cfg(feature = "duckdb-runtime")]
pub fn duckdb_table_columns(
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
pub fn duckdb_relation_exists(conn: &duckdb::Connection, relation_name: &str) -> bool {
    let sql = format!("SELECT 1 FROM {relation_name} LIMIT 0");
    conn.prepare(&sql)
        .and_then(|mut stmt| stmt.exists([]))
        .is_ok()
}
#[cfg(feature = "duckdb-runtime")]
pub fn duckdb_relation_has_rows(conn: &duckdb::Connection, relation_name: &str) -> bool {
    let sql = format!("SELECT 1 FROM {relation_name} LIMIT 1");
    conn.query_row(&sql, [], |_row| Ok(()))
        .optional()
        .map(|row| row.is_some())
        .unwrap_or(false)
}
#[cfg(feature = "duckdb-runtime")]
fn usage_event_summary_select_exprs(columns: &HashSet<String>) -> Vec<String> {
    let mut exprs = usage_event_base_select_exprs(columns, false, false);
    exprs.push("CAST(NULL AS VARCHAR) AS last_message_content".to_string());
    exprs
}
#[cfg(feature = "duckdb-runtime")]
fn usage_event_detail_select_exprs(
    columns: &HashSet<String>,
    detail_table_exists: bool,
) -> Vec<String> {
    let mut exprs = usage_event_base_select_exprs(columns, true, detail_table_exists);
    exprs.push(usage_event_detail_payload_expr(
        columns,
        detail_table_exists,
        "last_message_content",
        "CAST(NULL AS VARCHAR)",
    ));
    exprs.push(usage_event_detail_payload_expr(
        columns,
        detail_table_exists,
        "request_headers_json",
        "'{}'",
    ));
    exprs.push(usage_event_detail_payload_expr(
        columns,
        detail_table_exists,
        "client_request_body_json",
        "CAST(NULL AS VARCHAR)",
    ));
    exprs.push(usage_event_detail_payload_expr(
        columns,
        detail_table_exists,
        "upstream_request_body_json",
        "CAST(NULL AS VARCHAR)",
    ));
    exprs.push(usage_event_detail_payload_expr(
        columns,
        detail_table_exists,
        "full_request_json",
        "CAST(NULL AS VARCHAR)",
    ));
    exprs.push(usage_event_detail_payload_expr(
        columns,
        detail_table_exists,
        "error_message",
        "CAST(NULL AS VARCHAR)",
    ));
    exprs.push(usage_event_detail_payload_expr(
        columns,
        detail_table_exists,
        "error_body",
        "CAST(NULL AS VARCHAR)",
    ));
    exprs.push(usage_event_column_expr(columns, "detail_object_path", "CAST(NULL AS VARCHAR)"));
    exprs.push(usage_event_column_expr(columns, "detail_object_offset", "CAST(NULL AS BIGINT)"));
    exprs.push(usage_event_column_expr(columns, "detail_object_length", "CAST(NULL AS BIGINT)"));
    exprs.push(usage_event_column_expr(columns, "detail_object_sha256", "CAST(NULL AS VARCHAR)"));
    exprs
}
#[cfg(feature = "duckdb-runtime")]
fn usage_event_base_select_exprs(
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
fn usage_event_required_expr(column: &'static str) -> String {
    format!("e.{column} AS {column}")
}
#[cfg(feature = "duckdb-runtime")]
pub fn usage_event_column_expr(
    columns: &HashSet<String>,
    column: &'static str,
    missing_sql: &'static str,
) -> String {
    usage_event_expr(columns, column, &format!("e.{column}"), missing_sql)
}
#[cfg(feature = "duckdb-runtime")]
pub fn usage_event_expr(
    columns: &HashSet<String>,
    column: &'static str,
    present_sql: &str,
    missing_sql: &'static str,
) -> String {
    let sql = if columns.contains(column) { present_sql } else { missing_sql };
    format!("{sql} AS {column}")
}
#[cfg(feature = "duckdb-runtime")]
fn usage_event_detail_payload_expr(
    event_columns: &HashSet<String>,
    detail_table_exists: bool,
    column: &'static str,
    missing_sql: &'static str,
) -> String {
    let sql = match (detail_table_exists, event_columns.contains(column)) {
        (true, true) => format!("COALESCE(d.{column}, e.{column})"),
        (true, false) => format!("d.{column}"),
        (false, true) => format!("e.{column}"),
        (false, false) => missing_sql.to_string(),
    };
    format!("{sql} AS {column}")
}
