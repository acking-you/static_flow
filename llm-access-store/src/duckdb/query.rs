//! Usage-event read path: list/get/totals across active+archived tiers,
//! page planning, chart points, and row decoders.

use std::{path::Path, sync::Mutex};

use anyhow::{anyhow, Context};
use duckdb::OptionalExt;
use llm_access_core::{
    provider::{ProtocolFamily, ProviderType, RouteStrategy},
    store::{
        UsageChartPoint, UsageEventPage, UsageEventQuery, UsageEventSource, UsageEventStatusKind,
        UsageEventTotals, UsageFilterOptions,
    },
    usage::{UsageEvent, UsageStreamDetails, UsageTiming},
};

use super::{
    filter_options::{list_usage_filter_options_from_conn, merge_usage_event_totals},
    sql::{
        duckdb_relation_exists, duckdb_relation_has_rows, duckdb_table_columns,
        get_usage_event_detail_sql, list_usage_event_summaries_sql, usage_event_totals_sql,
    },
    util::{i64_to_usize, usize_to_i64},
    ArchivedUsageSegment, DuckDbUsageRepository, TieredDuckDbUsageConfig, TieredDuckDbUsageState,
    TieredUsageCatalogBackend, TieredUsagePageFetch, TieredUsagePartition,
    TieredUsagePartitionKind, UsageEventDetailObjectRef, UsageEventDetailRow,
    USAGE_EVENT_PAGE_MAX_LIMIT,
};
use crate::usage_catalog::UsageCatalogSegmentMatch;

#[cfg(feature = "duckdb-runtime")]
pub fn list_usage_events_from_path(
    path: &Path,
    query: &UsageEventQuery,
) -> anyhow::Result<UsageEventPage> {
    let conn = DuckDbUsageRepository::open_read_only_conn(path)?;
    list_usage_events_from_conn(&conn, query)
}
#[cfg(feature = "duckdb-runtime")]
fn list_usage_events_from_conn(
    conn: &duckdb::Connection,
    query: &UsageEventQuery,
) -> anyhow::Result<UsageEventPage> {
    let totals = fetch_usage_event_totals_from_conn(conn, query)?;
    let total = totals.event_count;
    let safe_limit = query.limit.min(USAGE_EVENT_PAGE_MAX_LIMIT);
    let safe_offset = query.offset;
    if safe_limit == 0 || safe_offset >= total {
        return Ok(UsageEventPage {
            total,
            offset: safe_offset,
            limit: safe_limit,
            has_more: false,
            totals,
            events: Vec::new(),
        });
    }
    let fetch_count = total.saturating_sub(safe_offset).min(safe_limit);
    let reverse_offset = total.saturating_sub(safe_offset.saturating_add(fetch_count));
    let mut events =
        fetch_usage_event_summaries_from_conn(conn, query, fetch_count, reverse_offset)?;
    events.reverse();
    Ok(UsageEventPage {
        total,
        offset: safe_offset,
        limit: safe_limit,
        has_more: safe_offset.saturating_add(events.len()) < total,
        totals,
        events,
    })
}
#[cfg(feature = "duckdb-runtime")]
fn fetch_usage_event_totals_from_conn(
    conn: &duckdb::Connection,
    query: &UsageEventQuery,
) -> anyhow::Result<UsageEventTotals> {
    let sql = usage_event_totals_sql(conn)?;
    conn.query_row(
        &sql,
        duckdb::params![
            query.key_id.as_deref(),
            query.provider_type.as_deref(),
            query.start_ms,
            query.end_ms,
            query.model.as_deref(),
            query.account_name.as_deref(),
            query.endpoint.as_deref(),
            query.status_code,
            query.status_kind.map(UsageEventStatusKind::as_query_value)
        ],
        |row| {
            Ok(UsageEventTotals {
                event_count: i64_to_usize(row.get(0)?),
                input_uncached_tokens: row.get::<_, i64>(1).map(|value| value.max(0) as u64)?,
                input_cached_tokens: row.get::<_, i64>(2).map(|value| value.max(0) as u64)?,
                output_tokens: row.get::<_, i64>(3).map(|value| value.max(0) as u64)?,
                billable_tokens: row.get::<_, i64>(4).map(|value| value.max(0) as u64)?,
            })
        },
    )
    .context("aggregate duckdb usage event totals")
}
#[cfg(feature = "duckdb-runtime")]
fn fetch_usage_event_summaries_from_conn(
    conn: &duckdb::Connection,
    query: &UsageEventQuery,
    limit: usize,
    offset: usize,
) -> anyhow::Result<Vec<UsageEvent>> {
    let sql = list_usage_event_summaries_sql(conn)?;
    let mut stmt = conn
        .prepare(&sql)
        .context("prepare duckdb usage event summary query")?;
    let rows = stmt
        .query_map(
            duckdb::params![
                query.key_id.as_deref(),
                query.provider_type.as_deref(),
                query.start_ms,
                query.end_ms,
                query.model.as_deref(),
                query.account_name.as_deref(),
                query.endpoint.as_deref(),
                query.status_code,
                query.status_kind.map(UsageEventStatusKind::as_query_value),
                usize_to_i64(limit),
                usize_to_i64(offset)
            ],
            decode_usage_event_summary_row,
        )
        .context("query duckdb usage events")?;
    rows.collect::<Result<Vec<_>, _>>()
        .context("collect duckdb usage events")
}
#[cfg(feature = "duckdb-runtime")]
pub fn list_usage_events_from_tiered(
    config: &TieredDuckDbUsageConfig,
    state: &Mutex<TieredDuckDbUsageState>,
    catalog_backend: &TieredUsageCatalogBackend,
    query: &UsageEventQuery,
) -> anyhow::Result<UsageEventPage> {
    let safe_limit = query.limit.min(USAGE_EVENT_PAGE_MAX_LIMIT);
    let safe_offset = query.offset;
    let mut total = 0usize;
    let mut totals = UsageEventTotals::default();
    let mut partitions = Vec::new();
    let mut events = Vec::new();

    if query.source.includes_hot() {
        let active_path = {
            let state = state
                .lock()
                .map_err(|_| anyhow!("tiered duckdb state lock poisoned"))?;
            state.active_path.clone()
        };
        let conn = DuckDbUsageRepository::open_read_only_conn(&active_path)?;
        let partition_totals = fetch_usage_event_totals_from_conn(&conn, query)?;
        let count = partition_totals.event_count;
        total = total.saturating_add(count);
        merge_usage_event_totals(&mut totals, &partition_totals);
        if count > 0 {
            partitions.push(TieredUsagePartition {
                path: active_path,
                count,
                totals: partition_totals,
                kind: TieredUsagePartitionKind::Active,
            });
        }
    }

    if query.source.includes_archive() {
        for partition in archived_usage_partitions_for_query(config, catalog_backend, query)? {
            let count = partition.count;
            total = total.saturating_add(count);
            merge_usage_event_totals(&mut totals, &partition.totals);
            partitions.push(partition);
        }
    }

    if safe_limit > 0 && safe_offset < total {
        let plan = plan_tiered_usage_page_fetches(
            partitions.iter().map(|partition| partition.count),
            safe_offset,
            safe_limit,
        );
        for fetch in plan {
            let partition = &partitions[fetch.partition_index];
            let conn = match partition.kind {
                TieredUsagePartitionKind::Active => {
                    DuckDbUsageRepository::open_read_only_conn(&partition.path)?
                },
                TieredUsagePartitionKind::Archive => {
                    DuckDbUsageRepository::open_read_only_conn(&partition.path)?
                },
            };
            let reverse_offset = partition
                .count
                .saturating_sub(fetch.local_newest_offset.saturating_add(fetch.limit));
            let mut partition_events =
                fetch_usage_event_summaries_from_conn(&conn, query, fetch.limit, reverse_offset)?;
            partition_events.reverse();
            events.extend(partition_events);
        }
    }

    Ok(UsageEventPage {
        total,
        offset: safe_offset,
        limit: safe_limit,
        has_more: safe_offset.saturating_add(events.len()) < total,
        totals,
        events,
    })
}
#[cfg(feature = "duckdb-runtime")]
pub fn plan_tiered_usage_page_fetches<I>(
    partition_counts: I,
    offset: usize,
    limit: usize,
) -> Vec<TieredUsagePageFetch>
where
    I: IntoIterator<Item = usize>,
{
    if limit == 0 {
        return Vec::new();
    }
    let mut remaining_offset = offset;
    let mut remaining_limit = limit;
    let mut fetches = Vec::new();

    for (partition_index, count) in partition_counts.into_iter().enumerate() {
        if count == 0 {
            continue;
        }
        if remaining_offset >= count {
            remaining_offset -= count;
            continue;
        }

        let available = count - remaining_offset;
        let fetch_limit = available.min(remaining_limit);
        fetches.push(TieredUsagePageFetch {
            partition_index,
            local_newest_offset: remaining_offset,
            limit: fetch_limit,
        });
        remaining_limit -= fetch_limit;
        remaining_offset = 0;
        if remaining_limit == 0 {
            break;
        }
    }

    fetches
}
#[cfg(feature = "duckdb-runtime")]
fn archived_usage_partitions_for_query(
    _config: &TieredDuckDbUsageConfig,
    catalog_backend: &TieredUsageCatalogBackend,
    query: &UsageEventQuery,
) -> anyhow::Result<Vec<TieredUsagePartition>> {
    let mut partitions = Vec::new();
    for segment_match in archived_segment_matches_for_query(catalog_backend, query)? {
        let segment = ArchivedUsageSegment::from(segment_match.segment.clone());
        let totals = if segment_fully_inside(&segment, query) {
            segment_match.matching_totals.clone().map(Into::into)
        } else {
            None
        };
        let totals = match totals {
            Some(totals) => totals,
            None => {
                let conn = DuckDbUsageRepository::open_read_only_conn(&segment.archive_path)?;
                fetch_usage_event_totals_from_conn(&conn, query)?
            },
        };
        let count = totals.event_count;
        if count > 0 {
            partitions.push(TieredUsagePartition {
                path: segment.archive_path,
                count,
                totals,
                kind: TieredUsagePartitionKind::Archive,
            });
        }
    }
    Ok(partitions)
}
#[cfg(feature = "duckdb-runtime")]
pub fn archived_segment_matches_for_query(
    catalog_backend: &TieredUsageCatalogBackend,
    query: &UsageEventQuery,
) -> anyhow::Result<Vec<UsageCatalogSegmentMatch>> {
    catalog_backend.archived_segment_matches_for_query(query)
}
#[cfg(feature = "duckdb-runtime")]
pub fn archived_segments_for_query(
    catalog_backend: &TieredUsageCatalogBackend,
    query: &UsageEventQuery,
) -> anyhow::Result<Vec<ArchivedUsageSegment>> {
    catalog_backend.archived_segments_for_query(query)
}
#[cfg(feature = "duckdb-runtime")]
fn segment_fully_inside(segment: &ArchivedUsageSegment, query: &UsageEventQuery) -> bool {
    let lower_ok = match (query.start_ms, segment.start_ms) {
        (Some(start), Some(segment_start)) => segment_start >= start,
        (Some(_), None) => false,
        (None, _) => true,
    };
    let upper_ok = match (query.end_ms, segment.end_ms) {
        (Some(end), Some(segment_end)) => segment_end < end,
        (Some(_), None) => false,
        (None, _) => true,
    };
    lower_ok && upper_ok
}
#[cfg(feature = "duckdb-runtime")]
pub fn get_usage_event_from_path(
    path: &Path,
    event_id: &str,
) -> anyhow::Result<Option<UsageEvent>> {
    Ok(get_usage_event_from_active_paths(path, event_id)?.map(|(event, _)| event))
}
#[cfg(feature = "duckdb-runtime")]
fn get_usage_event_from_conn(
    conn: &duckdb::Connection,
    event_id: &str,
) -> anyhow::Result<Option<UsageEvent>> {
    let sql = get_usage_event_detail_sql(conn)?;
    let mut stmt = conn
        .prepare(&sql)
        .context("prepare duckdb usage event detail query")?;
    match stmt.query_row(duckdb::params![event_id], decode_usage_event_detail_row) {
        Ok(event) => Ok(Some(event)),
        Err(duckdb::Error::QueryReturnedNoRows) => Ok(None),
        Err(err) => Err(err).context("query duckdb usage event detail"),
    }
}
#[cfg(feature = "duckdb-runtime")]
fn usage_event_detail_object_ref(
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
fn merge_usage_event_detail_payloads(event: &mut UsageEvent, detail: &UsageEventDetailRow) {
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
fn get_usage_event_from_active_paths(
    path: &Path,
    event_id: &str,
) -> anyhow::Result<Option<(UsageEvent, Option<UsageEventDetailObjectRef>)>> {
    let conn = DuckDbUsageRepository::open_read_only_conn(path)?;
    let event = match get_usage_event_from_conn(&conn, event_id)? {
        Some(event) => event,
        None => return Ok(None),
    };
    let detail_ref = usage_event_detail_object_ref(&conn, event_id)?;
    Ok(Some((event, detail_ref)))
}
#[cfg(feature = "duckdb-runtime")]
pub async fn get_usage_event_from_tiered(
    _config: &TieredDuckDbUsageConfig,
    state: &Mutex<TieredDuckDbUsageState>,
    catalog_backend: &TieredUsageCatalogBackend,
    event_id: &str,
) -> anyhow::Result<Option<UsageEvent>> {
    let (detail_store, active_path) = {
        let state = state
            .lock()
            .map_err(|_| anyhow!("tiered duckdb state lock poisoned"))?;
        (state.detail_store.clone(), state.active_path.clone())
    };
    if let Some((mut event, detail_ref)) =
        get_usage_event_from_active_paths(&active_path, event_id)?
    {
        if let Some(detail_ref) = detail_ref {
            if let Some(detail_store) = detail_store.as_ref() {
                if let Some(detail) = detail_store.get_row_for_ref(event_id, &detail_ref).await? {
                    merge_usage_event_detail_payloads(&mut event, &detail);
                }
            }
        }
        return Ok(Some(event));
    }
    let Some(segment) = locate_archived_segment(catalog_backend, event_id)? else {
        return Ok(None);
    };
    let (mut event, detail_ref) =
        match get_usage_event_from_archived_paths(&segment.archive_path, event_id)? {
            Some(event) => event,
            None => return Ok(None),
        };
    if let Some(detail_ref) = detail_ref {
        if let Some(detail_store) = detail_store.as_ref() {
            if let Some(detail) = detail_store.get_row_for_ref(event_id, &detail_ref).await? {
                merge_usage_event_detail_payloads(&mut event, &detail);
            }
        }
    }
    Ok(Some(event))
}
#[cfg(feature = "duckdb-runtime")]
fn get_usage_event_from_archived_paths(
    path: &Path,
    event_id: &str,
) -> anyhow::Result<Option<(UsageEvent, Option<UsageEventDetailObjectRef>)>> {
    let conn = DuckDbUsageRepository::open_read_only_conn(path)?;
    let event = match get_usage_event_from_conn(&conn, event_id)? {
        Some(event) => event,
        None => return Ok(None),
    };
    let detail_ref = usage_event_detail_object_ref(&conn, event_id)?;
    Ok(Some((event, detail_ref)))
}
#[cfg(feature = "duckdb-runtime")]
pub fn locate_archived_segment(
    catalog_backend: &TieredUsageCatalogBackend,
    event_id: &str,
) -> anyhow::Result<Option<ArchivedUsageSegment>> {
    catalog_backend.locate_archived_segment(event_id)
}
#[cfg(feature = "duckdb-runtime")]
pub fn usage_chart_points_from_tiered(
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
pub fn usage_chart_points_from_single_path(
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
fn empty_usage_chart_points(
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
pub fn list_usage_filter_options_from_path(
    path: &Path,
    query: &UsageEventQuery,
) -> anyhow::Result<UsageFilterOptions> {
    let conn = DuckDbUsageRepository::open_read_only_conn(path)?;
    list_usage_filter_options_from_conn(&conn, query)
}
#[cfg(feature = "duckdb-runtime")]
fn add_usage_chart_points_from_conn(
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
fn add_usage_chart_points_from_hourly_rollups(
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
        stream: UsageStreamDetails {
            stream_completed_cleanly: row.get(34)?,
            downstream_disconnect: row.get(35)?,
            final_event_type: row.get(36)?,
            bytes_streamed: row.get(37)?,
        },
        client_ip: row
            .get::<_, Option<String>>(38)?
            .unwrap_or_else(|| "unknown".to_string()),
        ip_region: row
            .get::<_, Option<String>>(39)?
            .unwrap_or_else(|| "unknown".to_string()),
        request_headers_json: if include_detail_payload {
            row.get::<_, Option<String>>(41)?
                .unwrap_or_else(|| "{}".to_string())
        } else {
            "{}".to_string()
        },
        last_message_content: row.get(40)?,
        client_request_body_json: if include_detail_payload { row.get(42)? } else { None },
        upstream_request_body_json: if include_detail_payload { row.get(43)? } else { None },
        full_request_json: if include_detail_payload { row.get(44)? } else { None },
        error_message: if include_detail_payload { row.get(45)? } else { None },
        error_body: if include_detail_payload { row.get(46)? } else { None },
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
