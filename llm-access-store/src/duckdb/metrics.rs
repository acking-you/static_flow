//! Usage-metrics accumulation (summary/group/latency) and snapshot
//! assembly across tiers.

use std::{collections::BTreeMap, path::Path, sync::Mutex};

use anyhow::{anyhow, Context};
use llm_access_core::store::{
    KiroLatencyRankingQuery, KiroLatencyRankingRow, KiroLatencyRankingSnapshot, UsageEventQuery,
    UsageEventSource, UsageMetricsDimensionView, UsageMetricsQuery, UsageMetricsSnapshot,
    UsageMetricsStatusCodeView, UsageMetricsSummary, PROVIDER_KIRO,
};

use super::{
    query::archived_segments_for_query,
    sql::{duckdb_table_columns, usage_event_column_expr, usage_event_expr},
    util::now_ms,
    DuckDbUsageRepository, TieredDuckDbUsageState, TieredUsageCatalogBackend,
    UsageMetricsAccumulator, UsageMetricsGroupAccumulator, UsageMetricsObservedRow,
};

#[cfg(feature = "duckdb-runtime")]
fn normalize_metrics_optional_string(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}
#[cfg(feature = "duckdb-runtime")]
fn average_metric_ms(sum_ms: i64, samples: u64) -> Option<f64> {
    (samples > 0).then(|| sum_ms as f64 / samples as f64)
}
#[cfg(feature = "duckdb-runtime")]
fn error_rate(group: &UsageMetricsGroupAccumulator) -> Option<f64> {
    (group.request_count > 0).then(|| group.non_ok_count as f64 / group.request_count as f64)
}
#[cfg(feature = "duckdb-runtime")]
fn disconnect_rate(group: &UsageMetricsGroupAccumulator) -> Option<f64> {
    (group.request_count > 0)
        .then(|| group.downstream_disconnect_count as f64 / group.request_count as f64)
}
#[cfg(feature = "duckdb-runtime")]
fn cmp_option_f64_desc(left: Option<f64>, right: Option<f64>) -> std::cmp::Ordering {
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
fn cmp_option_i64_desc(left: Option<i64>, right: Option<i64>) -> std::cmp::Ordering {
    match (left, right) {
        (Some(left), Some(right)) => right.cmp(&left),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}
#[cfg(feature = "duckdb-runtime")]
fn metrics_account_key(account_name: Option<&str>) -> String {
    account_name
        .map(|value| format!("account:{value}"))
        .unwrap_or_else(|| "account:unknown".to_string())
}
#[cfg(feature = "duckdb-runtime")]
fn metrics_account_label(account_name: Option<&str>) -> String {
    account_name.unwrap_or("(unknown account)").to_string()
}
#[cfg(feature = "duckdb-runtime")]
fn metrics_proxy_key(
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
fn metrics_proxy_label(
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
fn update_usage_metrics_group(
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
impl UsageMetricsAccumulator {
    fn observe(&mut self, row: UsageMetricsObservedRow) {
        let normalized_account_name = normalize_metrics_optional_string(row.account_name.clone());
        let normalized_proxy_source = normalize_metrics_optional_string(row.proxy_source.clone());
        let normalized_proxy_config_id =
            normalize_metrics_optional_string(row.proxy_config_id.clone());
        let normalized_proxy_config_name =
            normalize_metrics_optional_string(row.proxy_config_name.clone());
        let normalized_proxy_url = normalize_metrics_optional_string(row.proxy_url.clone());
        let is_ok = row.status_code == 200;

        self.summary.total_requests = self.summary.total_requests.saturating_add(1);
        if is_ok {
            self.summary.ok_requests = self.summary.ok_requests.saturating_add(1);
        } else {
            self.summary.non_ok_requests = self.summary.non_ok_requests.saturating_add(1);
            self.non_ok_status_codes
                .entry(row.status_code)
                .and_modify(|count| *count = count.saturating_add(1))
                .or_insert(1);
        }
        if let Some(value) = row.first_sse_write_ms {
            self.summary.first_token_sum_ms = self.summary.first_token_sum_ms.saturating_add(value);
            self.summary.first_token_samples = self.summary.first_token_samples.saturating_add(1);
            self.summary.max_first_token_ms =
                Some(self.summary.max_first_token_ms.unwrap_or(value).max(value));
        }
        if let Some(value) = row.latency_ms {
            self.summary.latency_sum_ms = self.summary.latency_sum_ms.saturating_add(value);
            self.summary.latency_samples = self.summary.latency_samples.saturating_add(1);
        }
        if let Some(value) = row.routing_wait_ms {
            self.summary.routing_wait_sum_ms =
                self.summary.routing_wait_sum_ms.saturating_add(value);
            self.summary.routing_wait_samples = self.summary.routing_wait_samples.saturating_add(1);
        }
        if row.quota_failover_count > 0 {
            self.summary.failover_request_count =
                self.summary.failover_request_count.saturating_add(1);
            self.summary.total_quota_failovers = self
                .summary
                .total_quota_failovers
                .saturating_add(row.quota_failover_count);
        }
        if row.downstream_disconnect {
            self.summary.downstream_disconnect_count =
                self.summary.downstream_disconnect_count.saturating_add(1);
        }
        if row.usage_missing {
            self.summary.usage_missing_count = self.summary.usage_missing_count.saturating_add(1);
        }
        if row.credit_usage_missing {
            self.summary.credit_usage_missing_count =
                self.summary.credit_usage_missing_count.saturating_add(1);
        }

        let account_key = metrics_account_key(normalized_account_name.as_deref());
        let account_label = metrics_account_label(normalized_account_name.as_deref());
        self.distinct_accounts.insert(account_key.clone());
        let account_group = self.accounts.entry(account_key.clone()).or_insert_with(|| {
            UsageMetricsGroupAccumulator {
                key: account_key.clone(),
                label: account_label.clone(),
                account_name: normalized_account_name.clone(),
                ..UsageMetricsGroupAccumulator::default()
            }
        });
        update_usage_metrics_group(account_group, &row, is_ok);

        let proxy_key = metrics_proxy_key(
            normalized_proxy_config_id.as_deref(),
            normalized_proxy_url.as_deref(),
            normalized_proxy_source.as_deref(),
        );
        let proxy_label = metrics_proxy_label(
            normalized_proxy_config_name.as_deref(),
            normalized_proxy_url.as_deref(),
            normalized_proxy_source.as_deref(),
        );
        self.distinct_proxies.insert(proxy_key.clone());
        let proxy_group =
            self.proxies
                .entry(proxy_key.clone())
                .or_insert_with(|| UsageMetricsGroupAccumulator {
                    key: proxy_key.clone(),
                    label: proxy_label.clone(),
                    proxy_config_id: normalized_proxy_config_id.clone(),
                    proxy_config_name: normalized_proxy_config_name.clone(),
                    proxy_url: normalized_proxy_url.clone(),
                    proxy_source: normalized_proxy_source.clone(),
                    ..UsageMetricsGroupAccumulator::default()
                });
        if proxy_group.proxy_config_id.is_none() {
            proxy_group.proxy_config_id = normalized_proxy_config_id.clone();
        }
        if proxy_group.proxy_config_name.is_none() {
            proxy_group.proxy_config_name = normalized_proxy_config_name.clone();
        }
        if proxy_group.proxy_url.is_none() {
            proxy_group.proxy_url = normalized_proxy_url.clone();
        }
        if proxy_group.proxy_source.is_none() {
            proxy_group.proxy_source = normalized_proxy_source.clone();
        }
        update_usage_metrics_group(proxy_group, &row, is_ok);
    }

    fn into_snapshot(self, query: &UsageMetricsQuery) -> UsageMetricsSnapshot {
        let top_limit = query.top_limit.max(1);
        let non_ok_status_codes = {
            let mut rows = self
                .non_ok_status_codes
                .into_iter()
                .map(|(status_code, request_count)| UsageMetricsStatusCodeView {
                    status_code,
                    request_count,
                })
                .collect::<Vec<_>>();
            rows.sort_by(|left, right| {
                right
                    .request_count
                    .cmp(&left.request_count)
                    .then_with(|| left.status_code.cmp(&right.status_code))
            });
            rows.truncate(top_limit);
            rows
        };
        UsageMetricsSnapshot {
            generated_at_ms: now_ms(),
            start_ms: query.start_ms,
            end_ms: query.end_ms,
            provider_type: query.provider_type.clone(),
            source: query.source,
            summary: UsageMetricsSummary {
                total_requests: self.summary.total_requests,
                ok_requests: self.summary.ok_requests,
                non_ok_requests: self.summary.non_ok_requests,
                distinct_accounts: self.distinct_accounts.len(),
                distinct_proxies: self.distinct_proxies.len(),
                first_token_samples: self.summary.first_token_samples,
                avg_first_token_ms: average_metric_ms(
                    self.summary.first_token_sum_ms,
                    self.summary.first_token_samples,
                ),
                max_first_token_ms: self.summary.max_first_token_ms,
                avg_latency_ms: average_metric_ms(
                    self.summary.latency_sum_ms,
                    self.summary.latency_samples,
                ),
                avg_routing_wait_ms: average_metric_ms(
                    self.summary.routing_wait_sum_ms,
                    self.summary.routing_wait_samples,
                ),
                failover_request_count: self.summary.failover_request_count,
                total_quota_failovers: self.summary.total_quota_failovers,
                downstream_disconnect_count: self.summary.downstream_disconnect_count,
                usage_missing_count: self.summary.usage_missing_count,
                credit_usage_missing_count: self.summary.credit_usage_missing_count,
            },
            top_first_token_accounts: top_usage_metrics_groups(
                &self.accounts,
                top_limit,
                |left, right| {
                    cmp_option_f64_desc(
                        average_metric_ms(left.first_token_sum_ms, left.first_token_samples),
                        average_metric_ms(right.first_token_sum_ms, right.first_token_samples),
                    )
                    .then_with(|| {
                        cmp_option_i64_desc(left.max_first_token_ms, right.max_first_token_ms)
                    })
                },
            ),
            top_first_token_proxies: top_usage_metrics_groups(
                &self.proxies,
                top_limit,
                |left, right| {
                    cmp_option_f64_desc(
                        average_metric_ms(left.first_token_sum_ms, left.first_token_samples),
                        average_metric_ms(right.first_token_sum_ms, right.first_token_samples),
                    )
                    .then_with(|| {
                        cmp_option_i64_desc(left.max_first_token_ms, right.max_first_token_ms)
                    })
                },
            ),
            top_non_ok_accounts: top_usage_metrics_groups(
                &self.accounts,
                top_limit,
                |left, right| {
                    right
                        .non_ok_count
                        .cmp(&left.non_ok_count)
                        .then_with(|| cmp_option_f64_desc(error_rate(left), error_rate(right)))
                },
            ),
            top_non_ok_proxies: top_usage_metrics_groups(
                &self.proxies,
                top_limit,
                |left, right| {
                    right
                        .non_ok_count
                        .cmp(&left.non_ok_count)
                        .then_with(|| cmp_option_f64_desc(error_rate(left), error_rate(right)))
                },
            ),
            top_routing_wait_accounts: top_usage_metrics_groups(
                &self.accounts,
                top_limit,
                |left, right| {
                    cmp_option_f64_desc(
                        average_metric_ms(left.routing_wait_sum_ms, left.routing_wait_samples),
                        average_metric_ms(right.routing_wait_sum_ms, right.routing_wait_samples),
                    )
                    .then_with(|| {
                        cmp_option_i64_desc(left.max_routing_wait_ms, right.max_routing_wait_ms)
                    })
                },
            ),
            top_routing_wait_proxies: top_usage_metrics_groups(
                &self.proxies,
                top_limit,
                |left, right| {
                    cmp_option_f64_desc(
                        average_metric_ms(left.routing_wait_sum_ms, left.routing_wait_samples),
                        average_metric_ms(right.routing_wait_sum_ms, right.routing_wait_samples),
                    )
                    .then_with(|| {
                        cmp_option_i64_desc(left.max_routing_wait_ms, right.max_routing_wait_ms)
                    })
                },
            ),
            top_failover_accounts: top_usage_metrics_groups(
                &self.accounts,
                top_limit,
                |left, right| {
                    right
                        .failover_request_count
                        .cmp(&left.failover_request_count)
                        .then_with(|| right.total_quota_failovers.cmp(&left.total_quota_failovers))
                },
            ),
            top_failover_proxies: top_usage_metrics_groups(
                &self.proxies,
                top_limit,
                |left, right| {
                    right
                        .failover_request_count
                        .cmp(&left.failover_request_count)
                        .then_with(|| right.total_quota_failovers.cmp(&left.total_quota_failovers))
                },
            ),
            top_disconnect_accounts: top_usage_metrics_groups(
                &self.accounts,
                top_limit,
                |left, right| {
                    right
                        .downstream_disconnect_count
                        .cmp(&left.downstream_disconnect_count)
                        .then_with(|| {
                            cmp_option_f64_desc(disconnect_rate(left), disconnect_rate(right))
                        })
                },
            ),
            top_disconnect_proxies: top_usage_metrics_groups(
                &self.proxies,
                top_limit,
                |left, right| {
                    right
                        .downstream_disconnect_count
                        .cmp(&left.downstream_disconnect_count)
                        .then_with(|| {
                            cmp_option_f64_desc(disconnect_rate(left), disconnect_rate(right))
                        })
                },
            ),
            non_ok_status_codes,
        }
    }

    fn into_kiro_latency_ranking(
        self,
        query: &KiroLatencyRankingQuery,
    ) -> KiroLatencyRankingSnapshot {
        KiroLatencyRankingSnapshot {
            generated_at_ms: now_ms(),
            start_ms: query.start_ms,
            end_ms: query.end_ms,
            source: query.source,
            first_token_samples: self.summary.first_token_samples,
            avg_first_token_ms: average_metric_ms(
                self.summary.first_token_sum_ms,
                self.summary.first_token_samples,
            ),
            accounts: kiro_latency_account_rows(&self.accounts),
            proxies: kiro_latency_proxy_rows(&self.proxies),
        }
    }
}
#[cfg(feature = "duckdb-runtime")]
fn usage_metrics_group_view(group: &UsageMetricsGroupAccumulator) -> UsageMetricsDimensionView {
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
fn kiro_latency_row(group: &UsageMetricsGroupAccumulator) -> KiroLatencyRankingRow {
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
fn kiro_latency_account_rows(
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
fn kiro_latency_proxy_rows(
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
fn top_usage_metrics_groups<F>(
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
fn usage_metrics_sql(conn: &duckdb::Connection) -> anyhow::Result<String> {
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
fn accumulate_usage_metrics_from_conn(
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
fn usage_metrics_query_as_segment_filter(query: &UsageMetricsQuery) -> UsageEventQuery {
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
pub fn usage_metrics_snapshot_from_path(
    path: &Path,
    query: &UsageMetricsQuery,
) -> anyhow::Result<UsageMetricsSnapshot> {
    let conn = DuckDbUsageRepository::open_read_only_conn(path)?;
    let mut accumulator = UsageMetricsAccumulator::default();
    accumulate_usage_metrics_from_conn(&mut accumulator, &conn, query)?;
    Ok(accumulator.into_snapshot(query))
}
#[cfg(feature = "duckdb-runtime")]
pub fn usage_metrics_snapshot_from_tiered(
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
fn kiro_latency_metrics_query(query: &KiroLatencyRankingQuery) -> UsageMetricsQuery {
    UsageMetricsQuery {
        provider_type: Some(PROVIDER_KIRO.to_string()),
        source: query.source,
        start_ms: query.start_ms,
        end_ms: query.end_ms,
        top_limit: usize::MAX,
    }
}
#[cfg(feature = "duckdb-runtime")]
pub fn kiro_latency_ranking_snapshot_from_path(
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
pub fn kiro_latency_ranking_snapshot_from_tiered(
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
