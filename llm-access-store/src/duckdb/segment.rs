//! Segment lifecycle: path layout, active-segment selection, async
//! sealing/compaction, archive finalization and catalog publish.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    thread,
    time::Duration,
};

use anyhow::{anyhow, Context};

use super::{
    append::publish_pending_segment_details_if_configured,
    connection::connection_config_snapshot,
    sql::{
        compact_copy_usage_events_sql, duckdb_compact_connection_sql, duckdb_relation_exists,
        duckdb_table_columns,
    },
    tiered_pending_dir,
    util::{duckdb_string_literal, i64_to_usize, now_ms, utc_date_parts},
    ArchivedSegmentPaths, ArchivedUsageSegment, DuckDbUsageConnectionConfig, DuckDbUsageRepository,
    SegmentFieldRollup, SegmentKeyRollup, SegmentStats, SharedDuckDbUsageConnectionConfig,
    TieredDuckDbUsageConfig, TieredUsageCatalogBackend, COMPACT_COPY_USAGE_ROLLUPS_DAILY_SQL,
    COMPACT_COPY_USAGE_ROLLUPS_HOURLY_SQL, TIERED_SEGMENT_SEALER_LOCK,
};
use crate::usage_catalog::{
    UsageCatalogFieldName, UsageCatalogFieldRollupRecord, UsageCatalogKeyRollupRecord,
    UsageCatalogSegmentRecord,
};

#[cfg(feature = "duckdb-runtime")]
pub fn segment_matches_time_window(
    segment: &UsageCatalogSegmentRecord,
    start_ms: Option<i64>,
    end_ms: Option<i64>,
) -> bool {
    (start_ms.is_none() || segment.end_ms.is_none() || segment.end_ms >= start_ms)
        && (end_ms.is_none() || segment.start_ms.is_none() || segment.start_ms < end_ms)
}
#[cfg(feature = "duckdb-runtime")]
pub fn sort_archived_segments(segments: &mut [ArchivedUsageSegment]) {
    segments.sort_by(|left, right| {
        right
            .end_ms
            .unwrap_or(0)
            .cmp(&left.end_ms.unwrap_or(0))
            .then_with(|| right.archive_path.cmp(&left.archive_path))
    });
}
#[cfg(feature = "duckdb-runtime")]
pub fn tiered_compacting_dir(config: &TieredDuckDbUsageConfig) -> PathBuf {
    config.active_dir.join("compacting")
}
#[cfg(feature = "duckdb-runtime")]
pub fn test_catalog_state_path(config: &TieredDuckDbUsageConfig) -> PathBuf {
    config.archive_dir.join(".test-usage-catalog.json")
}
#[cfg(feature = "duckdb-runtime")]
pub fn compacting_segment_path(config: &TieredDuckDbUsageConfig, segment_id: &str) -> PathBuf {
    tiered_compacting_dir(config).join(format!("{segment_id}.tmp.duckdb"))
}
#[cfg(feature = "duckdb-runtime")]
fn archive_segment_file_name(segment_id: &str) -> String {
    format!("{segment_id}.duckdb")
}
#[cfg(feature = "duckdb-runtime")]
pub fn archive_segment_bucket_dir(timestamp_ms: i64) -> PathBuf {
    let (year, month, day) = utc_date_parts(timestamp_ms);
    PathBuf::from(format!("{year:04}/{month:02}/{day:02}"))
}
#[cfg(feature = "duckdb-runtime")]
pub fn archive_segment_path_for_timestamp(
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
pub fn uploading_archive_segment_path_from_archive_path(archive_path: &Path) -> PathBuf {
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
fn find_archived_segment_path_recursive(
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
fn existing_archived_segment_paths(
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
pub fn remove_file_if_exists(path: &Path) -> anyhow::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("failed to remove file `{}`", path.display())),
    }
}
#[cfg(feature = "duckdb-runtime")]
pub fn prune_empty_directories_up_to(root: &Path, start: &Path) -> anyhow::Result<usize> {
    let mut removed = 0usize;
    let mut current = Some(start);
    while let Some(dir) = current {
        if dir == root {
            break;
        }
        match fs::remove_dir(dir) {
            Ok(()) => {
                removed = removed.saturating_add(1);
                current = dir.parent();
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                current = dir.parent();
            },
            Err(err) if err.kind() == std::io::ErrorKind::DirectoryNotEmpty => break,
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("failed to remove directory `{}`", dir.display()))
            },
        }
    }
    Ok(removed)
}
#[cfg(feature = "duckdb-runtime")]
pub fn collect_files_recursive(root: &Path, files: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    if !root.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(root)
        .with_context(|| format!("failed to read directory `{}`", root.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files_recursive(&path, files)?;
        } else if path.is_file() {
            files.push(path);
        }
    }
    Ok(())
}
#[cfg(feature = "duckdb-runtime")]
pub fn choose_active_segment(
    config: &TieredDuckDbUsageConfig,
    catalog_backend: &TieredUsageCatalogBackend,
) -> anyhow::Result<(PathBuf, u64)> {
    let mut active_files = Vec::new();
    for entry in fs::read_dir(&config.active_dir).with_context(|| {
        format!("failed to read active duckdb directory `{}`", config.active_dir.display())
    })? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file()
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("usage-active-") && name.ends_with(".duckdb"))
        {
            active_files.push(path);
        }
    }
    active_files.sort();
    if let Some(path) = active_files.pop() {
        let next = parse_segment_sequence(&path).unwrap_or(0).saturating_add(1);
        return Ok((path, next));
    }

    let next_sequence = catalog_backend.next_sequence()?.saturating_add(1);
    Ok((active_segment_path(config, next_sequence), next_sequence.saturating_add(1)))
}
#[cfg(feature = "duckdb-runtime")]
pub fn active_segment_path(config: &TieredDuckDbUsageConfig, sequence: u64) -> PathBuf {
    config
        .active_dir
        .join(format!("usage-active-{sequence:012}.duckdb"))
}
#[cfg(feature = "duckdb-runtime")]
pub fn parse_segment_sequence(path: &Path) -> Option<u64> {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .and_then(|stem| stem.rsplit('-').next())
        .and_then(|raw| raw.parse::<u64>().ok())
}
#[cfg(feature = "duckdb-runtime")]
pub fn parse_sequence_from_segment_id(segment_id: &str) -> Option<u64> {
    segment_id
        .rsplit('-')
        .next()
        .and_then(|raw| raw.parse::<u64>().ok())
}
#[cfg(feature = "duckdb-runtime")]
fn sealed_at_ms_for_segment(segment_id: &str) -> i64 {
    segment_id
        .split('-')
        .nth(1)
        .and_then(|raw| raw.parse::<i64>().ok())
        .unwrap_or_else(now_ms)
}
#[cfg(feature = "duckdb-runtime")]
pub fn spawn_existing_pending_sealers(
    config: TieredDuckDbUsageConfig,
    catalog_backend: Arc<TieredUsageCatalogBackend>,
    connection_config: SharedDuckDbUsageConnectionConfig,
) -> anyhow::Result<()> {
    let pending_dir = tiered_pending_dir(&config);
    for entry in fs::read_dir(&pending_dir).with_context(|| {
        format!("failed to read pending duckdb directory `{}`", pending_dir.display())
    })? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("duckdb") {
            let segment_id = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("usage-recovered")
                .to_string();
            spawn_segment_sealer(
                config.clone(),
                Arc::clone(&catalog_backend),
                path,
                segment_id,
                Arc::clone(&connection_config),
            );
        }
    }
    Ok(())
}
#[cfg(feature = "duckdb-runtime")]
pub fn spawn_segment_sealer(
    config: TieredDuckDbUsageConfig,
    catalog_backend: Arc<TieredUsageCatalogBackend>,
    pending_path: PathBuf,
    segment_id: String,
    connection_config: SharedDuckDbUsageConnectionConfig,
) {
    let sealer_segment_id = segment_id.clone();
    let sealer_pending_path = pending_path.clone();
    let spawn_result = thread::Builder::new()
        .name("llm-access-duckdb-sealer".to_string())
        .spawn(move || {
            let Ok(_sealer_guard) = TIERED_SEGMENT_SEALER_LOCK.lock() else {
                eprintln!(
                    "failed to archive llm-access duckdb segment `{segment_id}` from `{}`: sealer \
                     lock poisoned",
                    pending_path.display()
                );
                return;
            };
            let mut last_err = None;
            for attempt in 0..5 {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("build tiered segment sealer runtime");
                match runtime.block_on(publish_pending_segment_async(
                    &config,
                    catalog_backend.as_ref(),
                    &pending_path,
                    &segment_id,
                    connection_config_snapshot(&connection_config),
                )) {
                    Ok(()) => return,
                    Err(err) => {
                        last_err = Some(err);
                        thread::sleep(Duration::from_millis(250 * (attempt + 1)));
                    },
                }
            }
            if let Some(err) = last_err {
                eprintln!(
                    "failed to archive llm-access duckdb segment `{segment_id}` from `{}`: {err:#}",
                    pending_path.display()
                );
            }
        });
    if let Err(err) = spawn_result {
        eprintln!(
            "failed to spawn llm-access duckdb segment sealer thread for `{sealer_segment_id}` \
             from `{}`: {err}",
            sealer_pending_path.display()
        );
    }
}
#[cfg(feature = "duckdb-runtime")]
pub async fn publish_pending_segment_async(
    config: &TieredDuckDbUsageConfig,
    catalog_backend: &TieredUsageCatalogBackend,
    pending_path: &Path,
    segment_id: &str,
    connection_config: DuckDbUsageConnectionConfig,
) -> anyhow::Result<()> {
    fs::create_dir_all(&config.archive_dir).with_context(|| {
        format!("failed to create archive directory `{}`", config.archive_dir.display())
    })?;
    if let Some(paths) =
        existing_archived_segment_paths(config, catalog_backend, pending_path, segment_id)?
    {
        if paths.archive_duckdb.exists() {
            return finalize_archived_segment(config, catalog_backend, &paths, segment_id);
        }
    }
    let compact_path =
        compact_pending_segment_to_local_file(config, pending_path, segment_id, connection_config)?;
    let stats = validate_compacted_segment_matches_source(pending_path, &compact_path)?;
    let bucket_timestamp_ms = stats.end_ms.or(stats.start_ms).unwrap_or_else(now_ms);
    let archive_path = archive_segment_path_for_timestamp(config, segment_id, bucket_timestamp_ms);
    let uploading_path = uploading_archive_segment_path_from_archive_path(&archive_path);
    if let Some(parent) = archive_path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!("failed to create archived segment bucket directory `{}`", parent.display())
        })?;
    }
    let paths = ArchivedSegmentPaths {
        pending_duckdb: pending_path.to_path_buf(),
        compact_duckdb: compact_path.clone(),
        uploading_duckdb: uploading_path.clone(),
        archive_duckdb: archive_path.clone(),
    };
    if archive_path.exists() {
        return finalize_archived_segment(config, catalog_backend, &paths, segment_id);
    }
    publish_pending_segment_details_if_configured(config, pending_path).await?;
    remove_file_if_exists(&uploading_path)?;
    fs::copy(&compact_path, &uploading_path).with_context(|| {
        format!(
            "failed to copy compacted duckdb segment `{}` to uploading archive `{}`",
            compact_path.display(),
            uploading_path.display()
        )
    })?;
    fs::rename(&uploading_path, &archive_path).with_context(|| {
        format!(
            "failed to publish uploading archive `{}` to `{}`",
            uploading_path.display(),
            archive_path.display()
        )
    })?;
    let size_bytes = fs::metadata(&archive_path)
        .with_context(|| format!("failed to stat archived segment `{}`", archive_path.display()))?
        .len();
    publish_segment_catalog(catalog_backend, segment_id, &archive_path, &stats, size_bytes)?;
    remove_file_if_exists(pending_path)?;
    remove_file_if_exists(&compact_path)?;
    Ok(())
}
#[cfg(feature = "duckdb-runtime")]
fn finalize_archived_segment(
    _config: &TieredDuckDbUsageConfig,
    catalog_backend: &TieredUsageCatalogBackend,
    paths: &ArchivedSegmentPaths,
    segment_id: &str,
) -> anyhow::Result<()> {
    let stats = collect_segment_stats(&paths.archive_duckdb)?;
    let size_bytes = fs::metadata(&paths.archive_duckdb)
        .with_context(|| {
            format!("failed to stat archived segment `{}`", paths.archive_duckdb.display())
        })?
        .len();
    publish_segment_catalog(
        catalog_backend,
        segment_id,
        &paths.archive_duckdb,
        &stats,
        size_bytes,
    )?;
    remove_file_if_exists(&paths.uploading_duckdb)?;
    remove_file_if_exists(&paths.pending_duckdb)?;
    remove_file_if_exists(&paths.compact_duckdb)?;
    Ok(())
}
#[cfg(feature = "duckdb-runtime")]
pub fn collect_segment_stats(path: &Path) -> anyhow::Result<SegmentStats> {
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
pub fn collect_segment_event_ids(path: &Path) -> anyhow::Result<Vec<String>> {
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
fn collect_segment_field_rollups(
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
fn query_segment_field_rollups(
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
fn query_segment_field_rollup_sql(
    conn: &duckdb::Connection,
    field_name: UsageCatalogFieldName,
    sql: &str,
    scoped: bool,
) -> anyhow::Result<Vec<SegmentFieldRollup>> {
    let mut stmt = conn
        .prepare(sql)
        .with_context(|| format!("prepare duckdb segment field rollup query `{sql}`"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(SegmentFieldRollup {
                key_id: if scoped { row.get(0)? } else { None },
                provider_type: if scoped { row.get(1)? } else { None },
                field_name,
                field_value: row.get(2)?,
                row_count: i64_to_usize(row.get(3)?),
                input_uncached_tokens: row.get(4)?,
                input_cached_tokens: row.get(5)?,
                output_tokens: row.get(6)?,
                billable_tokens: row.get(7)?,
                first_used_at_ms: row.get(8)?,
                last_used_at_ms: row.get(9)?,
            })
        })
        .with_context(|| format!("query duckdb segment field rollups `{sql}`"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .context("collect duckdb segment field rollups")
}
#[cfg(feature = "duckdb-runtime")]
pub fn seed_catalog_from_archives_if_empty(
    catalog_backend: &TieredUsageCatalogBackend,
    config: &TieredDuckDbUsageConfig,
) -> anyhow::Result<()> {
    if !catalog_backend.is_empty()? {
        return Ok(());
    }
    let mut archive_files = Vec::new();
    collect_files_recursive(&config.archive_dir, &mut archive_files)?;
    archive_files.retain(|path| {
        path.file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|name| name.ends_with(".duckdb") && !name.ends_with(".uploading.duckdb"))
    });
    archive_files.sort();
    for archive_path in archive_files {
        publish_archive_path_to_catalog(catalog_backend, &archive_path)?;
    }
    Ok(())
}
#[cfg(feature = "duckdb-runtime")]
pub fn refresh_catalog_from_archives_if_needed(
    catalog_backend: &TieredUsageCatalogBackend,
) -> anyhow::Result<()> {
    let missing_paths = catalog_backend.archived_paths_missing_field_rollups()?;
    for archive_path in missing_paths {
        if !archive_path.exists() {
            continue;
        }
        publish_archive_path_to_catalog(catalog_backend, &archive_path)?;
    }
    Ok(())
}
#[cfg(feature = "duckdb-runtime")]
fn publish_archive_path_to_catalog(
    catalog_backend: &TieredUsageCatalogBackend,
    archive_path: &Path,
) -> anyhow::Result<()> {
    let Some(segment_id) = archive_path
        .file_stem()
        .and_then(|value| value.to_str())
        .map(ToString::to_string)
    else {
        return Ok(());
    };
    let stats = collect_segment_stats(archive_path)?;
    let event_ids = collect_segment_event_ids(archive_path)?;
    let size_bytes = fs::metadata(archive_path)
        .with_context(|| format!("stat archived segment `{}`", archive_path.display()))?
        .len();
    let record = UsageCatalogSegmentRecord {
        segment_id: segment_id.clone(),
        archive_path: archive_path.to_path_buf(),
        start_ms: stats.start_ms,
        end_ms: stats.end_ms,
        row_count: stats.row_count,
        input_uncached_tokens: stats.input_uncached_tokens,
        input_cached_tokens: stats.input_cached_tokens,
        output_tokens: stats.output_tokens,
        billable_tokens: stats.billable_tokens,
        size_bytes,
        sealed_at_ms: sealed_at_ms_for_segment(&segment_id),
    };
    let rollups = stats
        .rollups
        .iter()
        .map(|rollup| UsageCatalogKeyRollupRecord {
            key_id: rollup.key_id.clone(),
            provider_type: rollup.provider_type.clone(),
            row_count: rollup.row_count,
            input_uncached_tokens: rollup.input_uncached_tokens,
            input_cached_tokens: rollup.input_cached_tokens,
            output_tokens: rollup.output_tokens,
            billable_tokens: rollup.billable_tokens,
            credit_total: rollup.credit_total.clone(),
            credit_missing_events: rollup.credit_missing_events,
            first_used_at_ms: rollup.first_used_at_ms,
            last_used_at_ms: rollup.last_used_at_ms,
        })
        .collect::<Vec<_>>();
    let field_rollups = stats
        .field_rollups
        .iter()
        .map(|rollup| UsageCatalogFieldRollupRecord {
            key_id: rollup.key_id.clone(),
            provider_type: rollup.provider_type.clone(),
            field_name: rollup.field_name,
            field_value: rollup.field_value.clone(),
            row_count: rollup.row_count,
            input_uncached_tokens: rollup.input_uncached_tokens,
            input_cached_tokens: rollup.input_cached_tokens,
            output_tokens: rollup.output_tokens,
            billable_tokens: rollup.billable_tokens,
            first_used_at_ms: rollup.first_used_at_ms,
            last_used_at_ms: rollup.last_used_at_ms,
        })
        .collect::<Vec<_>>();
    catalog_backend.publish_segment(&record, &rollups, &field_rollups, &event_ids)
}
#[cfg(feature = "duckdb-runtime")]
fn compact_pending_segment_to_local_file(
    config: &TieredDuckDbUsageConfig,
    pending_path: &Path,
    segment_id: &str,
    connection_config: DuckDbUsageConnectionConfig,
) -> anyhow::Result<PathBuf> {
    let pending_source_conn = DuckDbUsageRepository::open_read_only_conn(pending_path)?;
    let pending_event_columns = duckdb_table_columns(&pending_source_conn, "usage_events")?;
    let pending_has_hourly_rollups =
        duckdb_relation_exists(&pending_source_conn, "usage_rollups_hourly");
    let pending_has_daily_rollups =
        duckdb_relation_exists(&pending_source_conn, "usage_rollups_daily");

    fs::create_dir_all(tiered_compacting_dir(config)).with_context(|| {
        format!(
            "failed to create compacting duckdb directory `{}`",
            tiered_compacting_dir(config).display()
        )
    })?;
    let compact_path = compacting_segment_path(config, segment_id);
    remove_file_if_exists(&compact_path)?;

    let conn = DuckDbUsageRepository::open_raw_conn(&compact_path)?;
    configure_duckdb_compact_connection(&conn, &tiered_compacting_dir(config), connection_config)?;
    crate::initialize_duckdb_target(&conn)?;
    let pending_path_str = pending_path
        .to_str()
        .ok_or_else(|| anyhow!("pending duckdb segment path is not valid UTF-8"))?;
    let attach_sql = format!(
        "ATTACH DATABASE {} AS pending_segment (READ_ONLY);",
        duckdb_string_literal(pending_path_str)
    );
    conn.execute_batch(&attach_sql).with_context(|| {
        format!("failed to attach pending duckdb segment `{}`", pending_path.display())
    })?;
    let copy_usage_events_sql = compact_copy_usage_events_sql(&pending_event_columns);
    let mut compact_sql_parts = vec![copy_usage_events_sql.as_str()];
    if pending_has_hourly_rollups {
        compact_sql_parts.push(COMPACT_COPY_USAGE_ROLLUPS_HOURLY_SQL);
    }
    if pending_has_daily_rollups {
        compact_sql_parts.push(COMPACT_COPY_USAGE_ROLLUPS_DAILY_SQL);
    }
    compact_sql_parts.push("DETACH pending_segment;");
    compact_sql_parts.push("CHECKPOINT;");
    let compact_sql = compact_sql_parts.join("\n");
    conn.execute_batch(&compact_sql).with_context(|| {
        format!(
            "failed to compact pending duckdb segment `{}` into `{}`",
            pending_path.display(),
            compact_path.display()
        )
    })?;
    Ok(compact_path)
}
#[cfg(feature = "duckdb-runtime")]
pub fn configure_duckdb_compact_connection(
    conn: &duckdb::Connection,
    temp_dir: &Path,
    connection_config: DuckDbUsageConnectionConfig,
) -> anyhow::Result<()> {
    fs::create_dir_all(temp_dir).with_context(|| {
        format!("failed to create duckdb compact temp directory `{}`", temp_dir.display())
    })?;
    let temp_dir_str = temp_dir
        .to_str()
        .ok_or_else(|| anyhow!("duckdb compact temp directory path is not valid UTF-8"))?;
    let sql = duckdb_compact_connection_sql(connection_config, temp_dir_str);
    conn.execute_batch(&sql)
        .context("failed to configure duckdb compact connection")?;
    Ok(())
}
#[cfg(feature = "duckdb-runtime")]
fn validate_compacted_segment_matches_source(
    source: &Path,
    compacted: &Path,
) -> anyhow::Result<SegmentStats> {
    let source_stats = collect_segment_stats(source)?;
    let compacted_stats = collect_segment_stats(compacted)?;
    if source_stats.row_count != compacted_stats.row_count
        || source_stats.event_id_count != compacted_stats.event_id_count
        || source_stats.start_ms != compacted_stats.start_ms
        || source_stats.end_ms != compacted_stats.end_ms
    {
        return Err(anyhow!(
            "compacted duckdb segment mismatch: source rows={} event_ids={} start={:?} end={:?}, \
             compacted rows={} event_ids={} start={:?} end={:?}",
            source_stats.row_count,
            source_stats.event_id_count,
            source_stats.start_ms,
            source_stats.end_ms,
            compacted_stats.row_count,
            compacted_stats.event_id_count,
            compacted_stats.start_ms,
            compacted_stats.end_ms
        ));
    }
    Ok(compacted_stats)
}
#[cfg(feature = "duckdb-runtime")]
pub fn publish_segment_catalog(
    catalog_backend: &TieredUsageCatalogBackend,
    segment_id: &str,
    archive_path: &Path,
    stats: &SegmentStats,
    size_bytes: u64,
) -> anyhow::Result<()> {
    let event_ids = collect_segment_event_ids(archive_path)?;
    let record = UsageCatalogSegmentRecord {
        segment_id: segment_id.to_string(),
        archive_path: archive_path.to_path_buf(),
        start_ms: stats.start_ms,
        end_ms: stats.end_ms,
        row_count: stats.row_count,
        input_uncached_tokens: stats.input_uncached_tokens,
        input_cached_tokens: stats.input_cached_tokens,
        output_tokens: stats.output_tokens,
        billable_tokens: stats.billable_tokens,
        size_bytes,
        sealed_at_ms: sealed_at_ms_for_segment(segment_id),
    };
    let rollups = stats
        .rollups
        .iter()
        .map(|rollup| UsageCatalogKeyRollupRecord {
            key_id: rollup.key_id.clone(),
            provider_type: rollup.provider_type.clone(),
            row_count: rollup.row_count,
            input_uncached_tokens: rollup.input_uncached_tokens,
            input_cached_tokens: rollup.input_cached_tokens,
            output_tokens: rollup.output_tokens,
            billable_tokens: rollup.billable_tokens,
            credit_total: rollup.credit_total.clone(),
            credit_missing_events: rollup.credit_missing_events,
            first_used_at_ms: rollup.first_used_at_ms,
            last_used_at_ms: rollup.last_used_at_ms,
        })
        .collect::<Vec<_>>();
    let field_rollups = stats
        .field_rollups
        .iter()
        .map(|rollup| UsageCatalogFieldRollupRecord {
            key_id: rollup.key_id.clone(),
            provider_type: rollup.provider_type.clone(),
            field_name: rollup.field_name,
            field_value: rollup.field_value.clone(),
            row_count: rollup.row_count,
            input_uncached_tokens: rollup.input_uncached_tokens,
            input_cached_tokens: rollup.input_cached_tokens,
            output_tokens: rollup.output_tokens,
            billable_tokens: rollup.billable_tokens,
            first_used_at_ms: rollup.first_used_at_ms,
            last_used_at_ms: rollup.last_used_at_ms,
        })
        .collect::<Vec<_>>();
    catalog_backend.publish_segment(&record, &rollups, &field_rollups, &event_ids)
}
