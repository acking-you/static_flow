//! Tiered segment primitives: archive path layout/lookup, segment id/sequence
//! parsing, stats/event-id/field-rollup collection, time-window matching, and
//! per-query partition selection.

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

#[cfg(feature = "duckdb-runtime")]
pub(crate) fn archived_segment_from_record(
    record: &UsageCatalogSegmentRecord,
) -> ArchivedUsageSegment {
    ArchivedUsageSegment {
        archive_path: record.archive_path.clone(),
        start_ms: record.start_ms,
        end_ms: record.end_ms,
    }
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn catalog_query_from_usage_query(query: &UsageEventQuery) -> UsageCatalogQuery {
    let mut field_filters = Vec::new();
    if let Some(model) = query.model.as_ref() {
        field_filters.push(UsageCatalogFieldFilter {
            field_name: UsageCatalogFieldName::Model,
            field_value: model.clone(),
        });
    }
    if let Some(account_name) = query.account_name.as_ref() {
        field_filters.push(UsageCatalogFieldFilter {
            field_name: UsageCatalogFieldName::AccountName,
            field_value: account_name.clone(),
        });
    }
    if let Some(endpoint) = query.endpoint.as_ref() {
        field_filters.push(UsageCatalogFieldFilter {
            field_name: UsageCatalogFieldName::Endpoint,
            field_value: endpoint.clone(),
        });
    }
    if let Some(status_code) = query.status_code {
        field_filters.push(UsageCatalogFieldFilter {
            field_name: UsageCatalogFieldName::StatusCode,
            field_value: status_code.to_string(),
        });
    }
    if let Some(status_kind) = query.status_kind {
        field_filters.push(UsageCatalogFieldFilter {
            field_name: UsageCatalogFieldName::StatusKind,
            field_value: status_kind.as_query_value().to_string(),
        });
    }
    UsageCatalogQuery {
        start_ms: query.start_ms,
        end_ms: query.end_ms,
        key_id: query.key_id.clone(),
        provider_type: query.provider_type.clone(),
        field_filters,
    }
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn catalog_query_has_exact_totals(query: &UsageCatalogQuery) -> bool {
    query.field_filters.len() <= 1
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn test_catalog_segment_matches_query(
    state: &TestTieredUsageCatalogState,
    segment_id: &str,
    query: &UsageCatalogQuery,
) -> bool {
    if query.field_filters.is_empty() {
        if query.key_id.is_none() && query.provider_type.is_none() {
            return true;
        }
        return state
            .segment_rollups
            .get(segment_id)
            .into_iter()
            .flatten()
            .any(|rollup| {
                test_key_rollup_matches_scope(rollup, query)
                    && test_rollup_matches_time(
                        rollup.first_used_at_ms,
                        rollup.last_used_at_ms,
                        query.start_ms,
                        query.end_ms,
                    )
            });
    }
    let Some(field_rollups) = state.segment_field_rollups.get(segment_id) else {
        return false;
    };
    query.field_filters.iter().all(|filter| {
        field_rollups.iter().any(|rollup| {
            rollup.field_name == filter.field_name
                && rollup.field_value == filter.field_value
                && test_field_rollup_matches_scope(rollup, query)
                && test_rollup_matches_time(
                    rollup.first_used_at_ms,
                    rollup.last_used_at_ms,
                    query.start_ms,
                    query.end_ms,
                )
        })
    })
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn test_catalog_segment_totals_for_query(
    state: &TestTieredUsageCatalogState,
    segment_id: &str,
    segment: &UsageCatalogSegmentRecord,
    query: &UsageCatalogQuery,
) -> Option<UsageCatalogSegmentTotals> {
    if !catalog_query_has_exact_totals(query) {
        return None;
    }
    if query.field_filters.is_empty() {
        if query.key_id.is_none() && query.provider_type.is_none() {
            return Some(UsageCatalogSegmentTotals {
                event_count: segment.row_count,
                input_uncached_tokens: i64_to_u64(segment.input_uncached_tokens),
                input_cached_tokens: i64_to_u64(segment.input_cached_tokens),
                output_tokens: i64_to_u64(segment.output_tokens),
                billable_tokens: i64_to_u64(segment.billable_tokens),
            });
        }
        let totals = state
            .segment_rollups
            .get(segment_id)
            .into_iter()
            .flatten()
            .filter(|rollup| test_key_rollup_matches_scope(rollup, query))
            .fold(
                UsageCatalogSegmentTotals {
                    event_count: 0,
                    input_uncached_tokens: 0,
                    input_cached_tokens: 0,
                    output_tokens: 0,
                    billable_tokens: 0,
                },
                |mut totals, rollup| {
                    totals.event_count = totals.event_count.saturating_add(rollup.row_count);
                    totals.input_uncached_tokens = totals
                        .input_uncached_tokens
                        .saturating_add(i64_to_u64(rollup.input_uncached_tokens));
                    totals.input_cached_tokens = totals
                        .input_cached_tokens
                        .saturating_add(i64_to_u64(rollup.input_cached_tokens));
                    totals.output_tokens = totals
                        .output_tokens
                        .saturating_add(i64_to_u64(rollup.output_tokens));
                    totals.billable_tokens = totals
                        .billable_tokens
                        .saturating_add(i64_to_u64(rollup.billable_tokens));
                    totals
                },
            );
        return Some(totals);
    }
    let filter = query.field_filters.first()?;
    let totals = state
        .segment_field_rollups
        .get(segment_id)
        .into_iter()
        .flatten()
        .filter(|rollup| {
            rollup.field_name == filter.field_name
                && rollup.field_value == filter.field_value
                && test_field_rollup_matches_scope(rollup, query)
        })
        .fold(
            UsageCatalogSegmentTotals {
                event_count: 0,
                input_uncached_tokens: 0,
                input_cached_tokens: 0,
                output_tokens: 0,
                billable_tokens: 0,
            },
            |mut totals, rollup| {
                totals.event_count = totals.event_count.saturating_add(rollup.row_count);
                totals.input_uncached_tokens = totals
                    .input_uncached_tokens
                    .saturating_add(i64_to_u64(rollup.input_uncached_tokens));
                totals.input_cached_tokens = totals
                    .input_cached_tokens
                    .saturating_add(i64_to_u64(rollup.input_cached_tokens));
                totals.output_tokens = totals
                    .output_tokens
                    .saturating_add(i64_to_u64(rollup.output_tokens));
                totals.billable_tokens = totals
                    .billable_tokens
                    .saturating_add(i64_to_u64(rollup.billable_tokens));
                totals
            },
        );
    Some(totals)
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn segment_matches_time_window(
    segment: &UsageCatalogSegmentRecord,
    start_ms: Option<i64>,
    end_ms: Option<i64>,
) -> bool {
    (start_ms.is_none() || segment.end_ms.is_none() || segment.end_ms >= start_ms)
        && (end_ms.is_none() || segment.start_ms.is_none() || segment.start_ms < end_ms)
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn sort_archived_segments(segments: &mut [ArchivedUsageSegment]) {
    segments.sort_by(|left, right| {
        right
            .end_ms
            .unwrap_or(0)
            .cmp(&left.end_ms.unwrap_or(0))
            .then_with(|| right.archive_path.cmp(&left.archive_path))
    });
}
pub(crate) fn tiered_pending_dir(config: &TieredDuckDbUsageConfig) -> PathBuf {
    config.active_dir.join("pending")
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn test_catalog_state_path(config: &TieredDuckDbUsageConfig) -> PathBuf {
    config.archive_dir.join(".test-usage-catalog.json")
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn archive_segment_file_name(segment_id: &str) -> String {
    format!("{segment_id}.duckdb")
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn archive_segment_bucket_dir(timestamp_ms: i64) -> PathBuf {
    let (year, month, day) = utc_date_parts(timestamp_ms);
    PathBuf::from(format!("{year:04}/{month:02}/{day:02}"))
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn archive_segment_path_for_timestamp(
    config: &TieredDuckDbUsageConfig,
    segment_id: &str,
    timestamp_ms: i64,
) -> PathBuf {
    config
        .archive_dir
        .join(archive_segment_bucket_dir(timestamp_ms))
        .join(archive_segment_file_name(segment_id))
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn uploading_archive_segment_path_from_archive_path(archive_path: &Path) -> PathBuf {
    let file_name = archive_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let uploading_name = file_name
        .strip_suffix(".duckdb")
        .map(|name| format!("{name}.uploading.duckdb"))
        .unwrap_or_else(|| format!("{file_name}.uploading"));
    archive_path.with_file_name(uploading_name)
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn find_archived_segment_path_recursive(
    root: &Path,
    expected_name: &str,
) -> anyhow::Result<Option<PathBuf>> {
    if !root.exists() {
        return Ok(None);
    }
    for entry in fs::read_dir(root)
        .with_context(|| format!("failed to read archive directory `{}`", root.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_archived_segment_path_recursive(&path, expected_name)? {
                return Ok(Some(found));
            }
            continue;
        }
        if path.is_file()
            && path
                .file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value == expected_name)
        {
            return Ok(Some(path));
        }
    }
    Ok(None)
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn existing_archived_segment_paths(
    config: &TieredDuckDbUsageConfig,
    catalog_backend: &TieredUsageCatalogBackend,
    pending_path: &Path,
    segment_id: &str,
) -> anyhow::Result<Option<ArchivedSegmentPaths>> {
    let archive_duckdb = if let Some(path) = catalog_backend.archive_path_for_segment(segment_id)? {
        Some(path)
    } else {
        find_archived_segment_path_recursive(
            &config.archive_dir,
            &archive_segment_file_name(segment_id),
        )?
    };
    Ok(archive_duckdb.map(|archive_duckdb| ArchivedSegmentPaths {
        pending_duckdb: pending_path.to_path_buf(),
        compact_duckdb: compacting_segment_path(config, segment_id),
        uploading_duckdb: uploading_archive_segment_path_from_archive_path(&archive_duckdb),
        archive_duckdb,
    }))
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn active_segment_path(config: &TieredDuckDbUsageConfig, sequence: u64) -> PathBuf {
    config
        .active_dir
        .join(format!("usage-active-{sequence:012}.duckdb"))
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn parse_segment_sequence(path: &Path) -> Option<u64> {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .and_then(|stem| stem.rsplit('-').next())
        .and_then(|raw| raw.parse::<u64>().ok())
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn parse_sequence_from_segment_id(segment_id: &str) -> Option<u64> {
    segment_id
        .rsplit('-')
        .next()
        .and_then(|raw| raw.parse::<u64>().ok())
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn collect_segment_stats(path: &Path) -> anyhow::Result<SegmentStats> {
    let conn = DuckDbUsageRepository::open_read_only_conn(path)?;
    let (
        row_count,
        event_id_count,
        start_ms,
        end_ms,
        input_uncached_tokens,
        input_cached_tokens,
        output_tokens,
        billable_tokens,
    ): (i64, i64, Option<i64>, Option<i64>, i64, i64, i64, i64) = conn
        .query_row(
            "SELECT
                CAST(count(*) AS BIGINT),
                CAST(count(event_id) AS BIGINT),
                min(created_at_ms),
                max(created_at_ms),
                CAST(COALESCE(sum(input_uncached_tokens), 0) AS BIGINT),
                CAST(COALESCE(sum(input_cached_tokens), 0) AS BIGINT),
                CAST(COALESCE(sum(output_tokens), 0) AS BIGINT),
                CAST(COALESCE(sum(billable_tokens), 0) AS BIGINT)
             FROM usage_events",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ))
            },
        )
        .context("query duckdb segment stats")?;
    let mut stmt = conn
        .prepare(
            "SELECT
                key_id,
                provider_type,
                CAST(count(*) AS BIGINT),
                CAST(COALESCE(sum(input_uncached_tokens), 0) AS BIGINT),
                CAST(COALESCE(sum(input_cached_tokens), 0) AS BIGINT),
                CAST(COALESCE(sum(output_tokens), 0) AS BIGINT),
                CAST(COALESCE(sum(billable_tokens), 0) AS BIGINT),
                CAST(COALESCE(sum(COALESCE(try_cast(credit_usage AS DOUBLE), 0)), 0) AS VARCHAR),
                CAST(COALESCE(sum(CASE WHEN credit_usage_missing THEN 1 ELSE 0 END), 0) AS BIGINT),
                min(created_at_ms),
                max(created_at_ms)
             FROM usage_events
             GROUP BY key_id, provider_type",
        )
        .context("prepare duckdb segment rollup query")?;
    let rollups = stmt
        .query_map([], |row| {
            Ok(SegmentKeyRollup {
                key_id: row.get(0)?,
                provider_type: row.get(1)?,
                row_count: i64_to_usize(row.get(2)?),
                input_uncached_tokens: row.get(3)?,
                input_cached_tokens: row.get(4)?,
                output_tokens: row.get(5)?,
                billable_tokens: row.get(6)?,
                credit_total: row.get(7)?,
                credit_missing_events: row.get(8)?,
                first_used_at_ms: row.get(9)?,
                last_used_at_ms: row.get(10)?,
            })
        })
        .context("query duckdb segment rollups")?
        .collect::<Result<Vec<_>, _>>()
        .context("collect duckdb segment rollups")?;
    let field_rollups = collect_segment_field_rollups(&conn)?;
    Ok(SegmentStats {
        start_ms,
        end_ms,
        row_count: i64_to_usize(row_count),
        event_id_count: i64_to_usize(event_id_count),
        input_uncached_tokens,
        input_cached_tokens,
        output_tokens,
        billable_tokens,
        rollups,
        field_rollups,
    })
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn collect_segment_event_ids(path: &Path) -> anyhow::Result<Vec<String>> {
    let conn = DuckDbUsageRepository::open_read_only_conn(path)?;
    let mut event_query = conn
        .prepare("SELECT event_id FROM usage_events")
        .context("prepare archived segment event locator query")?;
    let mut event_rows = event_query
        .query([])
        .context("query archived segment event locators")?;
    let mut event_ids = Vec::new();
    while let Some(row) = event_rows.next().context("read event locator row")? {
        event_ids.push(row.get(0)?);
    }
    Ok(event_ids)
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn active_segment_disk_bytes(path: &Path) -> u64 {
    fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
        + fs::metadata(duckdb_wal_path(path))
            .map(|meta| meta.len())
            .unwrap_or(0)
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn catalog_archived_duckdb_paths(
    catalog_backend: &TieredUsageCatalogBackend,
) -> anyhow::Result<HashSet<PathBuf>> {
    catalog_backend.archived_paths()
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn archived_usage_partitions_for_query(
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
pub(crate) fn archived_segment_matches_for_query(
    catalog_backend: &TieredUsageCatalogBackend,
    query: &UsageEventQuery,
) -> anyhow::Result<Vec<UsageCatalogSegmentMatch>> {
    catalog_backend.archived_segment_matches_for_query(query)
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn archived_segments_for_query(
    catalog_backend: &TieredUsageCatalogBackend,
    query: &UsageEventQuery,
) -> anyhow::Result<Vec<ArchivedUsageSegment>> {
    catalog_backend.archived_segments_for_query(query)
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn segment_fully_inside(
    segment: &ArchivedUsageSegment,
    query: &UsageEventQuery,
) -> bool {
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
pub(crate) fn locate_archived_segment(
    catalog_backend: &TieredUsageCatalogBackend,
    event_id: &str,
) -> anyhow::Result<Option<ArchivedUsageSegment>> {
    catalog_backend.locate_archived_segment(event_id)
}
