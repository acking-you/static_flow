//! Usage-metrics aggregation: summary/group/observed accumulators feeding the
//! metrics snapshot, plus chart-point assembly and metric math helpers.

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

#[cfg(feature = "duckdb-runtime")]
pub(crate) fn usage_chart_points_from_tiered(
    _config: &TieredDuckDbUsageConfig,
    state: &Mutex<TieredDuckDbUsageState>,
    catalog_backend: &TieredUsageCatalogBackend,
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
        let conn = DuckDbUsageRepository::open_read_only_conn(&state.active_path)?;
        add_usage_chart_points_from_conn(&mut points, &conn, key_id, start_ms, bucket_ms)?;
    }
    let query = UsageEventQuery {
        key_id: Some(key_id.to_string()),
        provider_type: None,
        model: None,
        account_name: None,
        endpoint: None,
        status_code: None,
        status_kind: None,
        source: UsageEventSource::Archive,
        start_ms: Some(start_ms),
        end_ms: Some(start_ms.saturating_add((bucket_count as i64).saturating_mul(bucket_ms))),
        limit: USAGE_EVENT_PAGE_MAX_LIMIT,
        offset: 0,
    };
    for segment_match in archived_segment_matches_for_query(catalog_backend, &query)? {
        let segment = ArchivedUsageSegment::from(segment_match.segment);
        let conn = DuckDbUsageRepository::open_read_only_conn(&segment.archive_path)?;
        add_usage_chart_points_from_conn(&mut points, &conn, key_id, start_ms, bucket_ms)?;
    }
    Ok(points)
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn usage_chart_points_from_single_path(
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
    let conn = DuckDbUsageRepository::open_read_only_conn(path)?;
    add_usage_chart_points_from_conn(&mut points, &conn, key_id, start_ms, bucket_ms)?;
    Ok(points)
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn empty_usage_chart_points(
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
pub(crate) fn normalize_metrics_optional_string(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn average_metric_ms(sum_ms: i64, samples: u64) -> Option<f64> {
    (samples > 0).then(|| sum_ms as f64 / samples as f64)
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn error_rate(group: &UsageMetricsGroupAccumulator) -> Option<f64> {
    (group.request_count > 0).then(|| group.non_ok_count as f64 / group.request_count as f64)
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn metrics_account_key(account_name: Option<&str>) -> String {
    account_name
        .map(|value| format!("account:{value}"))
        .unwrap_or_else(|| "account:unknown".to_string())
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn metrics_account_label(account_name: Option<&str>) -> String {
    account_name.unwrap_or("(unknown account)").to_string()
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn metrics_proxy_key(
    proxy_config_id: Option<&str>,
    proxy_url: Option<&str>,
    proxy_source: Option<&str>,
) -> String {
    if let Some(value) = proxy_config_id {
        return format!("proxy:id:{value}");
    }
    if let Some(value) = proxy_url {
        return format!("proxy:url:{value}");
    }
    if let Some(value) = proxy_source {
        return format!("proxy:source:{value}");
    }
    "proxy:unknown".to_string()
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn metrics_proxy_label(
    proxy_config_name: Option<&str>,
    proxy_url: Option<&str>,
    proxy_source: Option<&str>,
) -> String {
    proxy_config_name
        .or(proxy_url)
        .or(proxy_source)
        .unwrap_or("(unknown proxy)")
        .to_string()
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn update_usage_metrics_group(
    group: &mut UsageMetricsGroupAccumulator,
    row: &UsageMetricsObservedRow,
    is_ok: bool,
) {
    group.request_count = group.request_count.saturating_add(1);
    if is_ok {
        group.ok_count = group.ok_count.saturating_add(1);
    } else {
        group.non_ok_count = group.non_ok_count.saturating_add(1);
    }
    if let Some(value) = row.first_sse_write_ms {
        group.first_token_sum_ms = group.first_token_sum_ms.saturating_add(value);
        group.first_token_samples = group.first_token_samples.saturating_add(1);
        group.max_first_token_ms = Some(group.max_first_token_ms.unwrap_or(value).max(value));
    }
    if let Some(value) = row.routing_wait_ms {
        group.routing_wait_sum_ms = group.routing_wait_sum_ms.saturating_add(value);
        group.routing_wait_samples = group.routing_wait_samples.saturating_add(1);
        group.max_routing_wait_ms = Some(group.max_routing_wait_ms.unwrap_or(value).max(value));
    }
    if row.quota_failover_count > 0 {
        group.failover_request_count = group.failover_request_count.saturating_add(1);
        group.total_quota_failovers = group
            .total_quota_failovers
            .saturating_add(row.quota_failover_count);
    }
    if row.downstream_disconnect {
        group.downstream_disconnect_count = group.downstream_disconnect_count.saturating_add(1);
    }
    if row.usage_missing {
        group.usage_missing_count = group.usage_missing_count.saturating_add(1);
    }
    if row.credit_usage_missing {
        group.credit_usage_missing_count = group.credit_usage_missing_count.saturating_add(1);
    }
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn usage_metrics_group_view(
    group: &UsageMetricsGroupAccumulator,
) -> UsageMetricsDimensionView {
    UsageMetricsDimensionView {
        key: group.key.clone(),
        label: group.label.clone(),
        account_name: group.account_name.clone(),
        proxy_config_id: group.proxy_config_id.clone(),
        proxy_config_name: group.proxy_config_name.clone(),
        proxy_url: group.proxy_url.clone(),
        proxy_source: group.proxy_source.clone(),
        request_count: group.request_count,
        ok_count: group.ok_count,
        non_ok_count: group.non_ok_count,
        first_token_samples: group.first_token_samples,
        avg_first_token_ms: average_metric_ms(group.first_token_sum_ms, group.first_token_samples),
        max_first_token_ms: group.max_first_token_ms,
        routing_wait_samples: group.routing_wait_samples,
        avg_routing_wait_ms: average_metric_ms(
            group.routing_wait_sum_ms,
            group.routing_wait_samples,
        ),
        max_routing_wait_ms: group.max_routing_wait_ms,
        failover_request_count: group.failover_request_count,
        total_quota_failovers: group.total_quota_failovers,
        downstream_disconnect_count: group.downstream_disconnect_count,
        usage_missing_count: group.usage_missing_count,
        credit_usage_missing_count: group.credit_usage_missing_count,
    }
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn top_usage_metrics_groups<F>(
    groups: &BTreeMap<String, UsageMetricsGroupAccumulator>,
    limit: usize,
    mut compare: F,
) -> Vec<UsageMetricsDimensionView>
where
    F: FnMut(&UsageMetricsGroupAccumulator, &UsageMetricsGroupAccumulator) -> std::cmp::Ordering,
{
    let mut groups = groups.values().collect::<Vec<_>>();
    groups.sort_by(|left, right| {
        compare(left, right)
            .then_with(|| right.request_count.cmp(&left.request_count))
            .then_with(|| left.label.cmp(&right.label))
            .then_with(|| left.key.cmp(&right.key))
    });
    groups
        .into_iter()
        .take(limit)
        .map(usage_metrics_group_view)
        .collect()
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn usage_metrics_sql(conn: &duckdb::Connection) -> anyhow::Result<String> {
    let columns = duckdb_table_columns(conn, "usage_events")?;
    let select = [
        usage_event_column_expr(&columns, "account_name", "CAST(NULL AS VARCHAR)"),
        usage_event_expr(
            &columns,
            "status_code",
            "CAST(e.status_code AS INTEGER)",
            "CAST(0 AS INTEGER)",
        ),
        usage_event_expr(
            &columns,
            "first_sse_write_ms",
            "CAST(e.first_sse_write_ms AS BIGINT)",
            "CAST(NULL AS BIGINT)",
        ),
        usage_event_expr(
            &columns,
            "latency_ms",
            "CAST(e.latency_ms AS BIGINT)",
            "CAST(NULL AS BIGINT)",
        ),
        usage_event_expr(
            &columns,
            "routing_wait_ms",
            "CAST(e.routing_wait_ms AS BIGINT)",
            "CAST(NULL AS BIGINT)",
        ),
        usage_event_expr(
            &columns,
            "quota_failover_count",
            "CAST(e.quota_failover_count AS BIGINT)",
            "CAST(0 AS BIGINT)",
        ),
        usage_event_expr(
            &columns,
            "downstream_disconnect",
            "COALESCE(e.downstream_disconnect, FALSE)",
            "FALSE",
        ),
        usage_event_expr(&columns, "usage_missing", "COALESCE(e.usage_missing, FALSE)", "FALSE"),
        usage_event_expr(
            &columns,
            "credit_usage_missing",
            "COALESCE(e.credit_usage_missing, FALSE)",
            "FALSE",
        ),
        usage_event_column_expr(&columns, "proxy_source_at_event", "CAST(NULL AS VARCHAR)"),
        usage_event_column_expr(&columns, "proxy_config_id_at_event", "CAST(NULL AS VARCHAR)"),
        usage_event_column_expr(&columns, "proxy_config_name_at_event", "CAST(NULL AS VARCHAR)"),
        usage_event_column_expr(&columns, "proxy_url_at_event", "CAST(NULL AS VARCHAR)"),
    ]
    .join(",\n            ");
    Ok(format!(
        "SELECT
            {select}
         FROM usage_events e
         WHERE (?1 IS NULL OR e.provider_type = ?1)
           AND e.created_at_ms >= ?2
           AND e.created_at_ms < ?3"
    ))
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn accumulate_usage_metrics_from_conn(
    accumulator: &mut UsageMetricsAccumulator,
    conn: &duckdb::Connection,
    query: &UsageMetricsQuery,
) -> anyhow::Result<()> {
    let sql = usage_metrics_sql(conn)?;
    let mut stmt = conn
        .prepare(&sql)
        .context("prepare duckdb usage metrics query")?;
    let rows = stmt
        .query_map(
            duckdb::params![query.provider_type.as_deref(), query.start_ms, query.end_ms],
            |row| {
                Ok(UsageMetricsObservedRow {
                    account_name: normalize_metrics_optional_string(
                        row.get::<_, Option<String>>(0)?,
                    ),
                    status_code: row.get::<_, i32>(1)?,
                    first_sse_write_ms: row.get::<_, Option<i64>>(2)?,
                    latency_ms: row.get::<_, Option<i64>>(3)?,
                    routing_wait_ms: row.get::<_, Option<i64>>(4)?,
                    quota_failover_count: row.get::<_, i64>(5)?.max(0) as u64,
                    downstream_disconnect: row.get::<_, bool>(6)?,
                    usage_missing: row.get::<_, bool>(7)?,
                    credit_usage_missing: row.get::<_, bool>(8)?,
                    proxy_source: normalize_metrics_optional_string(
                        row.get::<_, Option<String>>(9)?,
                    ),
                    proxy_config_id: normalize_metrics_optional_string(
                        row.get::<_, Option<String>>(10)?,
                    ),
                    proxy_config_name: normalize_metrics_optional_string(
                        row.get::<_, Option<String>>(11)?,
                    ),
                    proxy_url: normalize_metrics_optional_string(row.get::<_, Option<String>>(12)?),
                })
            },
        )
        .context("query duckdb usage metrics")?;
    for row in rows {
        accumulator.observe(row.context("read duckdb usage metrics row")?);
    }
    Ok(())
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn usage_metrics_query_as_segment_filter(query: &UsageMetricsQuery) -> UsageEventQuery {
    UsageEventQuery {
        key_id: None,
        provider_type: query.provider_type.clone(),
        model: None,
        account_name: None,
        endpoint: None,
        status_code: None,
        status_kind: None,
        source: UsageEventSource::Archive,
        start_ms: Some(query.start_ms),
        end_ms: Some(query.end_ms),
        limit: 1,
        offset: 0,
    }
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn usage_metrics_snapshot_from_path(
    path: &Path,
    query: &UsageMetricsQuery,
) -> anyhow::Result<UsageMetricsSnapshot> {
    let conn = DuckDbUsageRepository::open_read_only_conn(path)?;
    let mut accumulator = UsageMetricsAccumulator::default();
    accumulate_usage_metrics_from_conn(&mut accumulator, &conn, query)?;
    Ok(accumulator.into_snapshot(query))
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn usage_metrics_snapshot_from_tiered(
    state: &Mutex<TieredDuckDbUsageState>,
    catalog_backend: &TieredUsageCatalogBackend,
    query: &UsageMetricsQuery,
) -> anyhow::Result<UsageMetricsSnapshot> {
    let mut accumulator = UsageMetricsAccumulator::default();
    if query.source.includes_hot() {
        let active_path = {
            let state = state
                .lock()
                .map_err(|_| anyhow!("tiered duckdb state lock poisoned"))?;
            state.active_path.clone()
        };
        let conn = DuckDbUsageRepository::open_read_only_conn(&active_path)?;
        accumulate_usage_metrics_from_conn(&mut accumulator, &conn, query)?;
    }
    if query.source.includes_archive() {
        for segment in archived_segments_for_query(
            catalog_backend,
            &usage_metrics_query_as_segment_filter(query),
        )? {
            let conn = DuckDbUsageRepository::open_read_only_conn(&segment.archive_path)?;
            accumulate_usage_metrics_from_conn(&mut accumulator, &conn, query)?;
        }
    }
    Ok(accumulator.into_snapshot(query))
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn add_usage_chart_points_from_conn(
    points: &mut [UsageChartPoint],
    conn: &duckdb::Connection,
    key_id: &str,
    start_ms: i64,
    bucket_ms: i64,
) -> anyhow::Result<()> {
    if bucket_ms % 3_600_000 == 0
        && duckdb_relation_exists(conn, "usage_rollups_hourly")
        && duckdb_relation_has_rows(conn, "usage_rollups_hourly")
    {
        return add_usage_chart_points_from_hourly_rollups(
            points, conn, key_id, start_ms, bucket_ms,
        );
    }
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
pub(crate) fn add_usage_chart_points_from_hourly_rollups(
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
            "SELECT
                CAST(floor(((epoch(bucket_hour) * 1000)::BIGINT - ?2) / ?3) AS BIGINT) AS \
             bucket_index,
                CAST(sum(input_uncached_tokens + output_tokens) AS BIGINT) AS tokens
             FROM usage_rollups_hourly
             WHERE key_id = ?1
               AND (epoch(bucket_hour) * 1000)::BIGINT >= ?2
               AND (epoch(bucket_hour) * 1000)::BIGINT < ?4
             GROUP BY bucket_index",
        )
        .context("prepare duckdb hourly usage chart query")?;
    let mut rows = stmt
        .query(duckdb::params![key_id, start_ms, bucket_ms, end_ms])
        .context("query duckdb hourly usage chart")?;
    while let Some(row) = rows.next().context("read duckdb hourly usage chart row")? {
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
