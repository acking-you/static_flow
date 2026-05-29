//! Kiro latency-ranking assembly: per-account/per-proxy rows and ranking
//! snapshots from a path or the tiered catalog, with descending comparators.

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

#[cfg(feature = "duckdb-runtime")]
pub(crate) fn cmp_option_f64_desc(left: Option<f64>, right: Option<f64>) -> std::cmp::Ordering {
    match (left, right) {
        (Some(left), Some(right)) => right
            .partial_cmp(&left)
            .unwrap_or(std::cmp::Ordering::Equal),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn cmp_option_i64_desc(left: Option<i64>, right: Option<i64>) -> std::cmp::Ordering {
    match (left, right) {
        (Some(left), Some(right)) => right.cmp(&left),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn kiro_latency_row(group: &UsageMetricsGroupAccumulator) -> KiroLatencyRankingRow {
    KiroLatencyRankingRow {
        key: group.key.clone(),
        label: group.label.clone(),
        account_name: group.account_name.clone(),
        proxy_config_id: group.proxy_config_id.clone(),
        proxy_config_name: group.proxy_config_name.clone(),
        proxy_url: group.proxy_url.clone(),
        proxy_source: group.proxy_source.clone(),
        first_token_samples: group.first_token_samples,
        avg_first_token_ms: average_metric_ms(group.first_token_sum_ms, group.first_token_samples),
        max_first_token_ms: group.max_first_token_ms,
    }
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn kiro_latency_account_rows(
    groups: &BTreeMap<String, UsageMetricsGroupAccumulator>,
) -> Vec<KiroLatencyRankingRow> {
    let mut rows = groups
        .values()
        .filter(|group| group.account_name.is_some() && group.first_token_samples > 0)
        .map(kiro_latency_row)
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| {
        left.avg_first_token_ms
            .unwrap_or(f64::INFINITY)
            .total_cmp(&right.avg_first_token_ms.unwrap_or(f64::INFINITY))
            .then_with(|| right.first_token_samples.cmp(&left.first_token_samples))
            .then_with(|| left.label.cmp(&right.label))
    });
    rows
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn kiro_latency_proxy_rows(
    groups: &BTreeMap<String, UsageMetricsGroupAccumulator>,
) -> Vec<KiroLatencyRankingRow> {
    let mut rows = groups
        .values()
        .filter(|group| {
            group.first_token_samples > 0
                && (group.proxy_url.is_some() || group.proxy_config_id.is_some())
        })
        .map(kiro_latency_row)
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| {
        left.avg_first_token_ms
            .unwrap_or(f64::INFINITY)
            .total_cmp(&right.avg_first_token_ms.unwrap_or(f64::INFINITY))
            .then_with(|| right.first_token_samples.cmp(&left.first_token_samples))
            .then_with(|| left.label.cmp(&right.label))
    });
    rows
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn kiro_latency_metrics_query(query: &KiroLatencyRankingQuery) -> UsageMetricsQuery {
    UsageMetricsQuery {
        provider_type: Some(PROVIDER_KIRO.to_string()),
        source: query.source,
        start_ms: query.start_ms,
        end_ms: query.end_ms,
        top_limit: usize::MAX,
    }
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn kiro_latency_ranking_snapshot_from_path(
    path: &Path,
    query: &KiroLatencyRankingQuery,
) -> anyhow::Result<KiroLatencyRankingSnapshot> {
    let conn = DuckDbUsageRepository::open_read_only_conn(path)?;
    let metrics_query = kiro_latency_metrics_query(query);
    let mut accumulator = UsageMetricsAccumulator::default();
    accumulate_usage_metrics_from_conn(&mut accumulator, &conn, &metrics_query)?;
    Ok(accumulator.into_kiro_latency_ranking(query))
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn kiro_latency_ranking_snapshot_from_tiered(
    state: &Mutex<TieredDuckDbUsageState>,
    catalog_backend: &TieredUsageCatalogBackend,
    query: &KiroLatencyRankingQuery,
) -> anyhow::Result<KiroLatencyRankingSnapshot> {
    let metrics_query = kiro_latency_metrics_query(query);
    let mut accumulator = UsageMetricsAccumulator::default();
    if query.source.includes_hot() {
        let active_path = {
            let state = state
                .lock()
                .map_err(|_| anyhow!("tiered duckdb state lock poisoned"))?;
            state.active_path.clone()
        };
        let conn = DuckDbUsageRepository::open_read_only_conn(&active_path)?;
        accumulate_usage_metrics_from_conn(&mut accumulator, &conn, &metrics_query)?;
    }
    if query.source.includes_archive() {
        for segment in archived_segments_for_query(
            catalog_backend,
            &usage_metrics_query_as_segment_filter(&metrics_query),
        )? {
            let conn = DuckDbUsageRepository::open_read_only_conn(&segment.archive_path)?;
            accumulate_usage_metrics_from_conn(&mut accumulator, &conn, &metrics_query)?;
        }
    }
    Ok(accumulator.into_kiro_latency_ranking(query))
}
