//! Heavy usage-event detail payload handling: detail row/blob encoding, pack
//! writes, object references, and the per-event insert execution.

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

pub(crate) fn has_external_detail_payloads(
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
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn insert_usage_event_detail_sql() -> &'static str {
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
pub(crate) fn compact_detail_object_payload_present_expr(columns: &HashSet<String>) -> String {
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
pub(crate) fn get_usage_event_detail_sql(conn: &duckdb::Connection) -> anyhow::Result<String> {
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
pub(crate) fn usage_event_detail_select_exprs(
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
pub(crate) fn usage_event_detail_payload_expr(
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
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn execute_usage_event_insert(
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
        row.stream_completed_cleanly,
        row.downstream_disconnect,
        row.final_event_type.as_deref(),
        row.bytes_streamed,
        row.request_body_bytes,
        row.quota_failover_count,
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
        row.routing_diagnostics_json.as_deref(),
        row.last_message_content.as_deref(),
        row.detail_object_payload_present,
        row.detail_object_path.as_deref(),
        row.detail_object_offset,
        row.detail_object_length,
        row.detail_object_sha256.as_deref(),
        row.proxy_source_at_event.as_deref(),
        row.proxy_config_id_at_event.as_deref(),
        row.proxy_config_name_at_event.as_deref(),
        row.proxy_url_at_event.as_deref(),
    ])?;
    Ok(())
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn execute_usage_event_detail_insert(
    stmt: &mut duckdb::Statement<'_>,
    row: &UsageEventRow,
) -> anyhow::Result<()> {
    stmt.execute(duckdb::params![
        &row.event_id,
        &row.request_headers_json,
        row.routing_diagnostics_json.as_deref(),
        row.last_message_content.as_deref(),
        row.client_request_body_json.as_deref(),
        row.upstream_request_body_json.as_deref(),
        row.full_request_json.as_deref(),
        row.error_message.as_deref(),
        row.error_body.as_deref(),
    ])?;
    Ok(())
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) async fn publish_pending_segment_details_if_configured(
    config: &TieredDuckDbUsageConfig,
    pending_path: &Path,
) -> anyhow::Result<()> {
    let _ = (config, pending_path);
    Ok(())
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn prune_expired_detail_day_buckets(
    config: &TieredDuckDbUsageConfig,
    cutoff_ms: i64,
) -> anyhow::Result<(usize, usize)> {
    let Some(details_root) = config.details_dir.as_ref() else {
        return Ok((0, 0));
    };
    let packs_root = details_root.join("packs");
    if !packs_root.exists() {
        return Ok((0, 0));
    }
    let cutoff_date = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(cutoff_ms)
        .unwrap_or_else(|| chrono::DateTime::<chrono::Utc>::from_timestamp(0, 0).expect("epoch"))
        .date_naive();
    let mut deleted_files = 0usize;
    let mut deleted_dirs = 0usize;
    for provider_entry in fs::read_dir(&packs_root).with_context(|| {
        format!("failed to read usage detail packs directory `{}`", packs_root.display())
    })? {
        let provider_entry = provider_entry?;
        let provider_path = provider_entry.path();
        if !provider_path.is_dir() {
            continue;
        }
        for year_entry in fs::read_dir(&provider_path).with_context(|| {
            format!("failed to read usage detail year directory `{}`", provider_path.display())
        })? {
            let year_entry = year_entry?;
            let year_path = year_entry.path();
            let Some(year) = year_path
                .file_name()
                .and_then(|value| value.to_str())
                .and_then(|value| value.parse::<i32>().ok())
            else {
                continue;
            };
            if !year_path.is_dir() {
                continue;
            }
            for month_entry in fs::read_dir(&year_path).with_context(|| {
                format!("failed to read usage detail month directory `{}`", year_path.display())
            })? {
                let month_entry = month_entry?;
                let month_path = month_entry.path();
                let Some(month) = month_path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .and_then(|value| value.parse::<u32>().ok())
                else {
                    continue;
                };
                if !month_path.is_dir() {
                    continue;
                }
                for day_entry in fs::read_dir(&month_path).with_context(|| {
                    format!("failed to read usage detail day directory `{}`", month_path.display())
                })? {
                    let day_entry = day_entry?;
                    let day_path = day_entry.path();
                    let Some(day) = day_path
                        .file_name()
                        .and_then(|value| value.to_str())
                        .and_then(|value| value.parse::<u32>().ok())
                    else {
                        continue;
                    };
                    if !day_path.is_dir() {
                        continue;
                    }
                    let Some(bucket_date) = chrono::NaiveDate::from_ymd_opt(year, month, day)
                    else {
                        continue;
                    };
                    if bucket_date >= cutoff_date {
                        continue;
                    }
                    let mut files = Vec::new();
                    collect_files_recursive(&day_path, &mut files)?;
                    deleted_files = deleted_files.saturating_add(files.len());
                    fs::remove_dir_all(&day_path).with_context(|| {
                        format!(
                            "failed to remove expired usage detail day directory `{}`",
                            day_path.display()
                        )
                    })?;
                    deleted_dirs = deleted_dirs.saturating_add(1);
                    deleted_dirs = deleted_dirs.saturating_add(prune_empty_directories_up_to(
                        &packs_root,
                        day_path.parent().unwrap_or(&packs_root),
                    )?);
                }
            }
        }
    }
    Ok((deleted_files, deleted_dirs))
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn usage_event_detail_object_ref(
    conn: &duckdb::Connection,
    event_id: &str,
) -> anyhow::Result<Option<UsageEventDetailObjectRef>> {
    let columns = duckdb_table_columns(conn, "usage_events")?;
    for column in [
        "detail_object_path",
        "detail_object_offset",
        "detail_object_length",
        "detail_object_sha256",
    ] {
        if !columns.contains(column) {
            return Ok(None);
        }
    }
    let row = conn
        .query_row(
            "SELECT detail_object_path, detail_object_offset, detail_object_length,
                    detail_object_sha256
             FROM usage_events
             WHERE event_id = ?1",
            duckdb::params![event_id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            },
        )
        .optional()
        .context("query duckdb usage event detail object ref")?;
    let Some((Some(relative_path), Some(offset), Some(length), Some(sha256))) = row else {
        return Ok(None);
    };
    if relative_path.trim().is_empty() || offset < 0 || length <= 0 || sha256.trim().is_empty() {
        return Ok(None);
    }
    let start = u64::try_from(offset).context("detail object offset exceeds u64")?;
    let length = u64::try_from(length).context("detail object length exceeds u64")?;
    let end = start
        .checked_add(length)
        .ok_or_else(|| anyhow!("detail object byte range overflows usize"))?;
    Ok(Some(UsageEventDetailObjectRef {
        relative_path,
        byte_range: start..end,
        sha256,
    }))
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn merge_usage_event_detail_payloads(
    event: &mut UsageEvent,
    detail: &UsageEventDetailRow,
) {
    event.request_headers_json = detail.request_headers_json.clone();
    event.routing_diagnostics_json = detail.routing_diagnostics_json.clone();
    event.last_message_content = detail.last_message_content.clone();
    event.client_request_body_json = detail.client_request_body_json.clone();
    event.upstream_request_body_json = detail.upstream_request_body_json.clone();
    event.full_request_json = detail.full_request_json.clone();
    event.error_message = detail.error_message.clone();
    event.error_body = detail.error_body.clone();
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn decode_usage_event_detail_row(row: &duckdb::Row<'_>) -> duckdb::Result<UsageEvent> {
    decode_usage_event_row(row, true)
}
