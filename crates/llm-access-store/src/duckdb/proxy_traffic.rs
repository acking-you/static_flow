//! Proxy traffic aggregation over usage-event body byte counters.

use std::{collections::BTreeMap, path::Path, sync::Mutex};

use anyhow::{anyhow, Context};
use llm_access_core::store::{
    ProxyTrafficPoint, ProxyTrafficProxySummary, ProxyTrafficQuery, ProxyTrafficSnapshot,
    ProxyTrafficTotals, UsageEventQuery, UsageEventSource,
};

use super::{
    query::archived_segments_for_query,
    sql::{
        duckdb_relation_exists, duckdb_relation_has_rows, duckdb_table_columns,
        usage_event_filter_column_sql,
    },
    util::now_ms,
    DuckDbUsageRepository, TieredDuckDbUsageState, TieredUsageCatalogBackend,
};

const HOUR_MS: i64 = 60 * 60 * 1000;
const MAX_PROXY_TRAFFIC_BUCKETS: usize = 10_000;

#[cfg(feature = "duckdb-runtime")]
#[derive(Debug, Clone, Default)]
struct ProxyTrafficAccumulator {
    start_ms: i64,
    bucket_ms: i64,
    totals: ProxyTrafficTotals,
    points: Vec<ProxyTrafficPoint>,
    proxies: BTreeMap<String, ProxyTrafficProxySummary>,
}

#[cfg(feature = "duckdb-runtime")]
#[derive(Debug, Clone)]
struct ProxyTrafficObservedRow {
    bucket_index: i64,
    proxy_key: String,
    proxy_source: Option<String>,
    proxy_config_id: Option<String>,
    proxy_config_name: Option<String>,
    proxy_url: Option<String>,
    totals: ProxyTrafficTotals,
}

#[cfg(feature = "duckdb-runtime")]
impl ProxyTrafficAccumulator {
    fn new(query: &ProxyTrafficQuery) -> Self {
        let bucket_count =
            proxy_traffic_bucket_count(query.start_ms, query.end_ms, query.bucket_ms);
        let points = (0..bucket_count)
            .map(|index| ProxyTrafficPoint {
                bucket_start_ms: query
                    .start_ms
                    .saturating_add((index as i64).saturating_mul(query.bucket_ms)),
                event_count: 0,
                request_bytes: 0,
                response_bytes: 0,
                total_bytes: 0,
            })
            .collect();
        Self {
            start_ms: query.start_ms,
            bucket_ms: query.bucket_ms,
            totals: ProxyTrafficTotals::default(),
            points,
            proxies: BTreeMap::new(),
        }
    }

    fn observe(&mut self, row: ProxyTrafficObservedRow) {
        self.totals.add_assign(row.totals);
        if let Ok(index) = usize::try_from(row.bucket_index) {
            if let Some(point) = self.points.get_mut(index) {
                point.add_totals(row.totals);
            }
        }
        let entry = self
            .proxies
            .entry(row.proxy_key.clone())
            .or_insert_with(|| ProxyTrafficProxySummary {
                proxy_key: row.proxy_key.clone(),
                proxy_config_id: row.proxy_config_id.clone(),
                proxy_config_name: row.proxy_config_name.clone(),
                proxy_url: row.proxy_url.clone(),
                proxy_source: row.proxy_source.clone(),
                totals: ProxyTrafficTotals::default(),
            });
        if entry.proxy_config_id.is_none() {
            entry.proxy_config_id = row.proxy_config_id;
        }
        if entry.proxy_config_name.is_none() {
            entry.proxy_config_name = row.proxy_config_name;
        }
        if entry.proxy_url.is_none() {
            entry.proxy_url = row.proxy_url;
        }
        if entry.proxy_source.is_none() {
            entry.proxy_source = row.proxy_source;
        }
        entry.totals.add_assign(row.totals);
    }

    fn into_snapshot(self, query: &ProxyTrafficQuery) -> ProxyTrafficSnapshot {
        let mut proxies = self.proxies.into_values().collect::<Vec<_>>();
        proxies.sort_by(|left, right| {
            right
                .totals
                .total_bytes
                .cmp(&left.totals.total_bytes)
                .then_with(|| left.proxy_key.cmp(&right.proxy_key))
        });
        ProxyTrafficSnapshot {
            generated_at_ms: now_ms(),
            start_ms: self.start_ms,
            end_ms: query.end_ms,
            provider_type: query.provider_type.clone(),
            source: query.source,
            proxy_config_id: query.proxy_config_id.clone(),
            bucket_ms: self.bucket_ms,
            totals: self.totals,
            points: self.points,
            proxies,
        }
    }
}

#[cfg(feature = "duckdb-runtime")]
pub fn proxy_traffic_snapshot_from_path(
    path: &Path,
    query: &ProxyTrafficQuery,
) -> anyhow::Result<ProxyTrafficSnapshot> {
    let conn = DuckDbUsageRepository::open_read_only_conn(path)?;
    let mut accumulator = ProxyTrafficAccumulator::new(query);
    accumulate_proxy_traffic_from_conn(&mut accumulator, &conn, query)?;
    Ok(accumulator.into_snapshot(query))
}

#[cfg(feature = "duckdb-runtime")]
pub fn proxy_traffic_snapshot_from_tiered(
    state: &Mutex<TieredDuckDbUsageState>,
    catalog_backend: &TieredUsageCatalogBackend,
    query: &ProxyTrafficQuery,
) -> anyhow::Result<ProxyTrafficSnapshot> {
    let mut accumulator = ProxyTrafficAccumulator::new(query);
    if query.source.includes_hot() {
        let active_path = {
            let state = state
                .lock()
                .map_err(|_| anyhow!("tiered duckdb state lock poisoned"))?;
            state.active_path.clone()
        };
        let conn = DuckDbUsageRepository::open_read_only_conn(&active_path)?;
        accumulate_proxy_traffic_from_conn(&mut accumulator, &conn, query)?;
    }
    if query.source.includes_archive() {
        for segment in archived_segments_for_query(
            catalog_backend,
            &proxy_traffic_query_as_segment_filter(query),
        )? {
            let conn = DuckDbUsageRepository::open_read_only_conn(&segment.archive_path)?;
            accumulate_proxy_traffic_from_conn(&mut accumulator, &conn, query)?;
        }
    }
    Ok(accumulator.into_snapshot(query))
}

#[cfg(feature = "duckdb-runtime")]
fn accumulate_proxy_traffic_from_conn(
    accumulator: &mut ProxyTrafficAccumulator,
    conn: &duckdb::Connection,
    query: &ProxyTrafficQuery,
) -> anyhow::Result<()> {
    if can_use_hourly_proxy_traffic_rollups(conn, query) {
        accumulate_proxy_traffic_from_hourly_rollups(accumulator, conn, query)
    } else {
        accumulate_proxy_traffic_from_usage_events(accumulator, conn, query)
    }
}

#[cfg(feature = "duckdb-runtime")]
fn can_use_hourly_proxy_traffic_rollups(
    conn: &duckdb::Connection,
    query: &ProxyTrafficQuery,
) -> bool {
    query.bucket_ms % HOUR_MS == 0
        && query.start_ms % HOUR_MS == 0
        && query.end_ms % HOUR_MS == 0
        && duckdb_relation_exists(conn, "proxy_traffic_rollups_hourly")
        && duckdb_relation_has_rows(conn, "proxy_traffic_rollups_hourly")
}

#[cfg(feature = "duckdb-runtime")]
fn accumulate_proxy_traffic_from_usage_events(
    accumulator: &mut ProxyTrafficAccumulator,
    conn: &duckdb::Connection,
    query: &ProxyTrafficQuery,
) -> anyhow::Result<()> {
    let columns = duckdb_table_columns(conn, "usage_events")?;
    let proxy_source_sql = usage_event_filter_column_sql(
        &columns,
        "e",
        "proxy_source_at_event",
        "CAST(NULL AS VARCHAR)",
    );
    let proxy_config_id_sql = usage_event_filter_column_sql(
        &columns,
        "e",
        "proxy_config_id_at_event",
        "CAST(NULL AS VARCHAR)",
    );
    let proxy_config_name_sql = usage_event_filter_column_sql(
        &columns,
        "e",
        "proxy_config_name_at_event",
        "CAST(NULL AS VARCHAR)",
    );
    let proxy_url_sql =
        usage_event_filter_column_sql(&columns, "e", "proxy_url_at_event", "CAST(NULL AS VARCHAR)");
    let request_bytes_sql =
        usage_event_filter_column_sql(&columns, "e", "request_body_bytes", "CAST(NULL AS BIGINT)");
    let response_bytes_sql =
        usage_event_filter_column_sql(&columns, "e", "bytes_streamed", "CAST(NULL AS BIGINT)");
    let proxy_key_sql = proxy_key_sql(&proxy_config_id_sql, &proxy_url_sql, &proxy_source_sql);
    let sql = format!(
        "SELECT
            CAST(floor((e.created_at_ms - ?3) / ?5) AS BIGINT) AS bucket_index,
            {proxy_key_sql} AS proxy_key,
            nullif(trim({proxy_source_sql}), '') AS proxy_source,
            nullif(trim({proxy_config_id_sql}), '') AS proxy_config_id,
            nullif(trim({proxy_config_name_sql}), '') AS proxy_config_name,
            nullif(trim({proxy_url_sql}), '') AS proxy_url,
            CAST(count(*) AS BIGINT) AS request_count,
            CAST(COALESCE(sum(greatest(COALESCE({request_bytes_sql}, 0), 0)), 0) AS BIGINT)
                AS request_bytes,
            CAST(COALESCE(sum(greatest(COALESCE({response_bytes_sql}, 0), 0)), 0) AS BIGINT)
                AS response_bytes
         FROM usage_events e
         WHERE (?1 IS NULL OR e.provider_type = ?1)
           AND (?2 IS NULL OR {proxy_config_id_sql} = ?2)
           AND e.created_at_ms >= ?3
           AND e.created_at_ms < ?4
         GROUP BY
            bucket_index,
            proxy_key,
            proxy_source,
            proxy_config_id,
            proxy_config_name,
            proxy_url"
    );
    let mut stmt = conn
        .prepare(&sql)
        .context("prepare duckdb proxy traffic raw query")?;
    let rows = stmt
        .query_map(
            duckdb::params![
                query.provider_type.as_deref(),
                query.proxy_config_id.as_deref(),
                query.start_ms,
                query.end_ms,
                query.bucket_ms,
            ],
            decode_proxy_traffic_row,
        )
        .context("query duckdb proxy traffic raw events")?;
    for row in rows {
        accumulator.observe(row.context("read duckdb proxy traffic raw row")?);
    }
    Ok(())
}

#[cfg(feature = "duckdb-runtime")]
fn accumulate_proxy_traffic_from_hourly_rollups(
    accumulator: &mut ProxyTrafficAccumulator,
    conn: &duckdb::Connection,
    query: &ProxyTrafficQuery,
) -> anyhow::Result<()> {
    let mut stmt = conn
        .prepare(
            "SELECT
                CAST(floor(((epoch(bucket_hour) * 1000)::BIGINT - ?3) / ?5) AS BIGINT)
                    AS bucket_index,
                proxy_key,
                proxy_source,
                proxy_config_id,
                proxy_config_name,
                proxy_url,
                CAST(COALESCE(sum(request_count), 0) AS BIGINT) AS request_count,
                CAST(COALESCE(sum(request_bytes), 0) AS BIGINT) AS request_bytes,
                CAST(COALESCE(sum(response_bytes), 0) AS BIGINT) AS response_bytes
             FROM proxy_traffic_rollups_hourly
             WHERE (?1 IS NULL OR provider_type = ?1)
               AND (?2 IS NULL OR proxy_config_id = ?2)
               AND (epoch(bucket_hour) * 1000)::BIGINT >= ?3
               AND (epoch(bucket_hour) * 1000)::BIGINT < ?4
             GROUP BY
                bucket_index,
                proxy_key,
                proxy_source,
                proxy_config_id,
                proxy_config_name,
                proxy_url",
        )
        .context("prepare duckdb proxy traffic hourly rollup query")?;
    let rows = stmt
        .query_map(
            duckdb::params![
                query.provider_type.as_deref(),
                query.proxy_config_id.as_deref(),
                query.start_ms,
                query.end_ms,
                query.bucket_ms,
            ],
            decode_proxy_traffic_row,
        )
        .context("query duckdb proxy traffic hourly rollups")?;
    for row in rows {
        accumulator.observe(row.context("read duckdb proxy traffic rollup row")?);
    }
    Ok(())
}

#[cfg(feature = "duckdb-runtime")]
fn decode_proxy_traffic_row(row: &duckdb::Row<'_>) -> duckdb::Result<ProxyTrafficObservedRow> {
    let request_bytes = row.get::<_, i64>(7)?.max(0) as u64;
    let response_bytes = row.get::<_, i64>(8)?.max(0) as u64;
    Ok(ProxyTrafficObservedRow {
        bucket_index: row.get(0)?,
        proxy_key: row.get(1)?,
        proxy_source: normalize_optional_string(row.get::<_, Option<String>>(2)?),
        proxy_config_id: normalize_optional_string(row.get::<_, Option<String>>(3)?),
        proxy_config_name: normalize_optional_string(row.get::<_, Option<String>>(4)?),
        proxy_url: normalize_optional_string(row.get::<_, Option<String>>(5)?),
        totals: ProxyTrafficTotals {
            event_count: row.get::<_, i64>(6)?.max(0) as u64,
            request_bytes,
            response_bytes,
            total_bytes: request_bytes.saturating_add(response_bytes),
        },
    })
}

#[cfg(feature = "duckdb-runtime")]
fn proxy_key_sql(proxy_config_sql: &str, proxy_url_sql: &str, proxy_source_sql: &str) -> String {
    format!(
        "CASE
            WHEN {proxy_config_sql} IS NOT NULL AND length(trim({proxy_config_sql})) > 0
                THEN 'proxy:id:' || trim({proxy_config_sql})
            WHEN {proxy_url_sql} IS NOT NULL AND length(trim({proxy_url_sql})) > 0
                THEN 'proxy:url:' || trim({proxy_url_sql})
            WHEN {proxy_source_sql} IS NOT NULL AND length(trim({proxy_source_sql})) > 0
                THEN 'proxy:source:' || trim({proxy_source_sql})
            ELSE 'proxy:unknown'
         END",
    )
}

#[cfg(feature = "duckdb-runtime")]
fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else if trimmed.len() == value.len() {
            Some(value)
        } else {
            Some(trimmed.to_string())
        }
    })
}

#[cfg(feature = "duckdb-runtime")]
fn proxy_traffic_bucket_count(start_ms: i64, end_ms: i64, bucket_ms: i64) -> usize {
    if end_ms <= start_ms || bucket_ms <= 0 {
        return 0;
    }
    let span = end_ms.saturating_sub(start_ms);
    let count = span.saturating_add(bucket_ms - 1).div_euclid(bucket_ms);
    usize::try_from(count)
        .unwrap_or(usize::MAX)
        .min(MAX_PROXY_TRAFFIC_BUCKETS)
}

#[cfg(feature = "duckdb-runtime")]
fn proxy_traffic_query_as_segment_filter(query: &ProxyTrafficQuery) -> UsageEventQuery {
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

#[cfg(all(test, feature = "duckdb-runtime"))]
mod tests {
    use llm_access_core::store::{ProxyTrafficQuery, UsageEventSource};

    use super::{proxy_traffic_bucket_count, ProxyTrafficAccumulator, MAX_PROXY_TRAFFIC_BUCKETS};

    #[test]
    fn proxy_traffic_bucket_count_limits_allocated_points() {
        let query = ProxyTrafficQuery {
            proxy_config_id: None,
            provider_type: None,
            source: UsageEventSource::Hot,
            start_ms: 0,
            end_ms: i64::MAX,
            bucket_ms: 1,
        };

        let accumulator = ProxyTrafficAccumulator::new(&query);

        assert_eq!(proxy_traffic_bucket_count(0, i64::MAX, 1), MAX_PROXY_TRAFFIC_BUCKETS);
        assert_eq!(accumulator.points.len(), MAX_PROXY_TRAFFIC_BUCKETS);
    }
}
