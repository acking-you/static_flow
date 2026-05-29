//! Usage-event read paths: list/get/fetch from a single path, from a live
//! connection, and across tiered segments, plus row decoding, page-fetch
//! planning, and detail-payload merging.

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

#[cfg(feature = "duckdb-runtime")]
pub(crate) fn dedupe_usage_events_owned(events: Vec<UsageEvent>) -> Vec<UsageEvent> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::with_capacity(events.len());
    for event in events {
        if seen.insert(event.event_id.clone()) {
            deduped.push(event);
        }
    }
    deduped
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn list_usage_events_from_path(
    path: &Path,
    query: &UsageEventQuery,
) -> anyhow::Result<UsageEventPage> {
    let conn = DuckDbUsageRepository::open_read_only_conn(path)?;
    list_usage_events_from_conn(&conn, query)
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn list_usage_events_from_conn(
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
pub(crate) fn fetch_usage_event_totals_from_conn(
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
pub(crate) fn fetch_usage_event_summaries_from_conn(
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
pub(crate) fn list_usage_events_from_tiered(
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
pub(crate) fn plan_tiered_usage_page_fetches<I>(
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
pub(crate) fn get_usage_event_from_path(
    path: &Path,
    event_id: &str,
) -> anyhow::Result<Option<UsageEvent>> {
    Ok(get_usage_event_from_active_paths(path, event_id)?.map(|(event, _)| event))
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn get_usage_event_from_conn(
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
pub(crate) fn get_usage_event_from_active_paths(
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
pub(crate) async fn get_usage_event_from_tiered(
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
pub(crate) fn get_usage_event_from_archived_paths(
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
pub(crate) fn merge_usage_event_totals(target: &mut UsageEventTotals, added: &UsageEventTotals) {
    target.event_count = target.event_count.saturating_add(added.event_count);
    target.input_uncached_tokens = target
        .input_uncached_tokens
        .saturating_add(added.input_uncached_tokens);
    target.input_cached_tokens = target
        .input_cached_tokens
        .saturating_add(added.input_cached_tokens);
    target.output_tokens = target.output_tokens.saturating_add(added.output_tokens);
    target.billable_tokens = target.billable_tokens.saturating_add(added.billable_tokens);
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn decode_usage_event_summary_row(row: &duckdb::Row<'_>) -> duckdb::Result<UsageEvent> {
    decode_usage_event_row(row, false)
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn decode_usage_event_row(
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
