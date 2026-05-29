//! Per-key usage rollup computation from a path, a connection, or the tiered
//! catalog, with rollup merging.

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

#[cfg(feature = "duckdb-runtime")]
pub(crate) fn test_key_rollup_matches_scope(
    rollup: &UsageCatalogKeyRollupRecord,
    query: &UsageCatalogQuery,
) -> bool {
    query
        .key_id
        .as_deref()
        .is_none_or(|key_id| rollup.key_id == key_id)
        && query
            .provider_type
            .as_deref()
            .is_none_or(|provider_type| rollup.provider_type == provider_type)
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn test_field_rollup_matches_scope(
    rollup: &UsageCatalogFieldRollupRecord,
    query: &UsageCatalogQuery,
) -> bool {
    match (query.key_id.as_deref(), query.provider_type.as_deref()) {
        (None, None) => rollup.key_id.is_none() && rollup.provider_type.is_none(),
        (key_id, provider_type) => {
            key_id.is_none_or(|key_id| rollup.key_id.as_deref() == Some(key_id))
                && provider_type.is_none_or(|provider_type| {
                    rollup.provider_type.as_deref() == Some(provider_type)
                })
        },
    }
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn test_rollup_matches_time(
    first_used_at_ms: Option<i64>,
    last_used_at_ms: Option<i64>,
    start_ms: Option<i64>,
    end_ms: Option<i64>,
) -> bool {
    (start_ms.is_none() || last_used_at_ms.is_none() || last_used_at_ms >= start_ms)
        && (end_ms.is_none() || first_used_at_ms.is_none() || first_used_at_ms < end_ms)
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn collect_segment_field_rollups(
    conn: &duckdb::Connection,
) -> anyhow::Result<Vec<SegmentFieldRollup>> {
    let mut rollups = Vec::new();
    rollups.extend(query_segment_field_rollups(conn, UsageCatalogFieldName::Model, "model")?);
    rollups.extend(query_segment_field_rollups(
        conn,
        UsageCatalogFieldName::AccountName,
        "account_name",
    )?);
    rollups.extend(query_segment_field_rollups(conn, UsageCatalogFieldName::Endpoint, "endpoint")?);
    rollups.extend(query_segment_field_rollups(
        conn,
        UsageCatalogFieldName::StatusCode,
        "CAST(status_code AS VARCHAR)",
    )?);
    rollups.extend(query_segment_field_rollups(
        conn,
        UsageCatalogFieldName::StatusKind,
        "CASE WHEN status_code = 200 THEN 'ok' ELSE 'non_ok' END",
    )?);
    Ok(rollups)
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn query_segment_field_rollups(
    conn: &duckdb::Connection,
    field_name: UsageCatalogFieldName,
    value_sql: &str,
) -> anyhow::Result<Vec<SegmentFieldRollup>> {
    let global_sql = format!(
        "SELECT
            CAST(NULL AS VARCHAR) AS key_id,
            CAST(NULL AS VARCHAR) AS provider_type,
            field_value,
            CAST(count(*) AS BIGINT),
            CAST(COALESCE(sum(input_uncached_tokens), 0) AS BIGINT),
            CAST(COALESCE(sum(input_cached_tokens), 0) AS BIGINT),
            CAST(COALESCE(sum(output_tokens), 0) AS BIGINT),
            CAST(COALESCE(sum(billable_tokens), 0) AS BIGINT),
            min(created_at_ms),
            max(created_at_ms)
         FROM (
            SELECT {value_sql} AS field_value, input_uncached_tokens, input_cached_tokens,
                   output_tokens, billable_tokens, created_at_ms
            FROM usage_events
         ) values_by_field
         WHERE field_value IS NOT NULL
           AND length(trim(field_value)) > 0
         GROUP BY field_value"
    );
    let scoped_sql = format!(
        "SELECT
            key_id,
            provider_type,
            field_value,
            CAST(count(*) AS BIGINT),
            CAST(COALESCE(sum(input_uncached_tokens), 0) AS BIGINT),
            CAST(COALESCE(sum(input_cached_tokens), 0) AS BIGINT),
            CAST(COALESCE(sum(output_tokens), 0) AS BIGINT),
            CAST(COALESCE(sum(billable_tokens), 0) AS BIGINT),
            min(created_at_ms),
            max(created_at_ms)
         FROM (
            SELECT key_id, provider_type, {value_sql} AS field_value,
                   input_uncached_tokens, input_cached_tokens, output_tokens,
                   billable_tokens, created_at_ms
            FROM usage_events
         ) values_by_field
         WHERE field_value IS NOT NULL
           AND length(trim(field_value)) > 0
         GROUP BY key_id, provider_type, field_value"
    );
    let mut rollups = query_segment_field_rollup_sql(conn, field_name, &global_sql, false)?;
    rollups.extend(query_segment_field_rollup_sql(conn, field_name, &scoped_sql, true)?);
    Ok(rollups)
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn key_usage_rollups_from_path(
    path: &Path,
) -> anyhow::Result<Vec<KeyUsageRollupSummary>> {
    let conn = DuckDbUsageRepository::open_read_only_conn(path)?;
    key_usage_rollups_from_conn(&conn)
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn key_usage_rollups_from_conn(
    conn: &duckdb::Connection,
) -> anyhow::Result<Vec<KeyUsageRollupSummary>> {
    let mut stmt = conn
        .prepare(
            "SELECT
                key_id,
                CAST(COALESCE(sum(input_uncached_tokens), 0) AS BIGINT),
                CAST(COALESCE(sum(input_cached_tokens), 0) AS BIGINT),
                CAST(COALESCE(sum(output_tokens), 0) AS BIGINT),
                CAST(COALESCE(sum(billable_tokens), 0) AS BIGINT),
                CAST(COALESCE(sum(COALESCE(try_cast(credit_usage AS DOUBLE), 0)), 0) AS VARCHAR),
                CAST(COALESCE(sum(CASE WHEN credit_usage_missing THEN 1 ELSE 0 END), 0) AS BIGINT),
                max(created_at_ms)
             FROM usage_events
             GROUP BY key_id",
        )
        .context("prepare duckdb key usage rollup query")?;
    let rows = stmt
        .query_map([], |row| {
            Ok(KeyUsageRollupSummary {
                key_id: row.get(0)?,
                input_uncached_tokens: row.get(1)?,
                input_cached_tokens: row.get(2)?,
                output_tokens: row.get(3)?,
                billable_tokens: row.get(4)?,
                credit_total: row.get(5)?,
                credit_missing_events: row.get(6)?,
                last_used_at_ms: row.get(7)?,
            })
        })
        .context("query duckdb key usage rollups")?;
    rows.collect::<Result<Vec<_>, _>>()
        .context("collect duckdb key usage rollups")
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn key_usage_rollups_from_tiered(
    _config: &TieredDuckDbUsageConfig,
    state: &Mutex<TieredDuckDbUsageState>,
    catalog_backend: &TieredUsageCatalogBackend,
) -> anyhow::Result<Vec<KeyUsageRollupSummary>> {
    let mut combined = BTreeMap::<String, KeyUsageRollupSummary>::new();
    {
        let state = state
            .lock()
            .map_err(|_| anyhow!("tiered duckdb state lock poisoned"))?;
        let conn = DuckDbUsageRepository::open_read_only_conn(&state.active_path)?;
        for rollup in key_usage_rollups_from_conn(&conn)? {
            merge_key_rollup(&mut combined, rollup);
        }
    }
    for rollup in catalog_backend.archived_key_usage_rollups()? {
        merge_key_rollup(&mut combined, rollup);
    }
    Ok(combined.into_values().collect())
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn merge_key_rollup(
    combined: &mut BTreeMap<String, KeyUsageRollupSummary>,
    rollup: KeyUsageRollupSummary,
) {
    let entry = combined
        .entry(rollup.key_id.clone())
        .or_insert_with(|| KeyUsageRollupSummary {
            key_id: rollup.key_id.clone(),
            input_uncached_tokens: 0,
            input_cached_tokens: 0,
            output_tokens: 0,
            billable_tokens: 0,
            credit_total: "0".to_string(),
            credit_missing_events: 0,
            last_used_at_ms: None,
        });
    entry.input_uncached_tokens = entry
        .input_uncached_tokens
        .saturating_add(rollup.input_uncached_tokens);
    entry.input_cached_tokens = entry
        .input_cached_tokens
        .saturating_add(rollup.input_cached_tokens);
    entry.output_tokens = entry.output_tokens.saturating_add(rollup.output_tokens);
    entry.billable_tokens = entry.billable_tokens.saturating_add(rollup.billable_tokens);
    let current_credit = entry.credit_total.parse::<f64>().unwrap_or(0.0);
    let added_credit = rollup.credit_total.parse::<f64>().unwrap_or(0.0);
    entry.credit_total = (current_credit + added_credit).to_string();
    entry.credit_missing_events = entry
        .credit_missing_events
        .saturating_add(rollup.credit_missing_events);
    entry.last_used_at_ms = match (entry.last_used_at_ms, rollup.last_used_at_ms) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (None, Some(right)) => Some(right),
        (left, None) => left,
    };
}
