//! Usage-analytics retention: prune/rollover/discard expired segments,
//! orphan cleanup, WAL/checkpoint helpers.

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
};

use anyhow::{anyhow, Context};

use super::{
    connection::{
        connection_config_snapshot, initialize_duckdb_target_path,
        initialize_duckdb_target_path_with_connection_config,
    },
    segment::{
        active_segment_path, collect_files_recursive, collect_segment_stats,
        parse_segment_sequence, prune_empty_directories_up_to, remove_file_if_exists,
        spawn_segment_sealer,
    },
    tiered_pending_dir,
    util::now_ms,
    DuckDbUsageConnectionConfig, DuckDbUsageRepository, PersistentUsageWriter,
    RetentionSegmentCandidate, SharedDuckDbUsageConnectionConfig, SingleDuckDbUsageState,
    TieredDuckDbUsageConfig, TieredDuckDbUsageState, TieredUsageCatalogBackend,
    UsageAnalyticsPruneReport, USAGE_ANALYTICS_RETENTION_DAY_MS,
};

#[cfg(feature = "duckdb-runtime")]
pub async fn prune_tiered_usage_analytics(
    config: &TieredDuckDbUsageConfig,
    state: &Mutex<TieredDuckDbUsageState>,
    connection_config: &SharedDuckDbUsageConnectionConfig,
    catalog_backend: &TieredUsageCatalogBackend,
    now_ms: i64,
    retention_days: u64,
) -> anyhow::Result<UsageAnalyticsPruneReport> {
    let cutoff_ms = usage_analytics_retention_cutoff_ms(now_ms, retention_days);
    let mut deleted_files = {
        // Serialize the active-segment rollover/discard against the append write
        // path (see `TieredDuckDbUsageState::write_gate`): an append must not
        // hold an in-flight writer for the active segment while this cycle
        // deletes/rolls it. Only this rollover touches the active segment; the
        // archived/detail cleanup below operates on other files and stays
        // ungated so it never stalls appends.
        let write_gate = {
            let state = state
                .lock()
                .map_err(|_| anyhow!("tiered duckdb state lock poisoned"))?;
            Arc::clone(&state.write_gate)
        };
        let _write_guard = write_gate.lock_owned().await;
        rollover_expired_active_segment(
            config,
            state,
            connection_config_snapshot(connection_config),
            cutoff_ms,
        )?
    };
    let expired_segments = delete_expired_segments_from_catalog(catalog_backend, cutoff_ms)?;
    for segment in &expired_segments {
        deleted_files =
            deleted_files.saturating_add(remove_duckdb_segment_files(&segment.archive_path)?);
        if let Some(parent) = segment.archive_path.parent() {
            let _ = prune_empty_directories_up_to(&config.archive_dir, parent);
        }
    }
    let deleted_orphan_files = prune_orphan_archived_duckdb_files(config, catalog_backend)?;
    let (deleted_detail_files, deleted_detail_dirs) =
        prune_expired_detail_day_buckets(config, cutoff_ms)?;
    Ok(UsageAnalyticsPruneReport {
        deleted_segments: expired_segments.len(),
        deleted_files,
        deleted_orphan_files,
        deleted_detail_files,
        deleted_detail_dirs,
    })
}
#[cfg(feature = "duckdb-runtime")]
pub fn usage_analytics_retention_cutoff_ms(now_ms: i64, retention_days: u64) -> i64 {
    let retention_days = i64::try_from(retention_days.max(1)).unwrap_or(i64::MAX);
    now_ms.saturating_sub(retention_days.saturating_mul(USAGE_ANALYTICS_RETENTION_DAY_MS))
}
#[cfg(feature = "duckdb-runtime")]
fn rollover_expired_active_segment(
    config: &TieredDuckDbUsageConfig,
    state: &Mutex<TieredDuckDbUsageState>,
    connection_config: DuckDbUsageConnectionConfig,
    cutoff_ms: i64,
) -> anyhow::Result<usize> {
    let mut state = state
        .lock()
        .map_err(|_| anyhow!("tiered duckdb state lock poisoned"))?;
    if !state.active_has_rows {
        return Ok(0);
    }
    state.active_writer = None;
    checkpoint_duckdb_path(&state.active_path, connection_config)?;
    let stats = collect_segment_stats(&state.active_path)?;
    if stats.row_count == 0 {
        state.active_has_rows = false;
        return Ok(0);
    }
    if stats.end_ms.is_some_and(|end_ms| end_ms < cutoff_ms) {
        return discard_expired_active_segment(config, &mut state, connection_config);
    }
    Ok(0)
}
#[cfg(feature = "duckdb-runtime")]
fn discard_expired_active_segment(
    config: &TieredDuckDbUsageConfig,
    state: &mut TieredDuckDbUsageState,
    connection_config: DuckDbUsageConnectionConfig,
) -> anyhow::Result<usize> {
    state.active_writer = None;
    let expired_path = state.active_path.clone();
    let new_active_path = active_segment_path(config, state.next_sequence);
    state.next_sequence = state.next_sequence.saturating_add(1);
    let deleted_files = remove_duckdb_segment_files(&expired_path)?;
    initialize_duckdb_target_path_with_connection_config(&new_active_path, connection_config)?;
    state.active_path = new_active_path;
    state.active_has_rows = false;
    state.active_writer = None;
    Ok(deleted_files)
}
#[cfg(feature = "duckdb-runtime")]
fn delete_expired_segments_from_catalog(
    catalog_backend: &TieredUsageCatalogBackend,
    cutoff_ms: i64,
) -> anyhow::Result<Vec<RetentionSegmentCandidate>> {
    catalog_backend
        .delete_expired_segments(cutoff_ms)
        .map(|segments| {
            segments
                .into_iter()
                .map(|segment| RetentionSegmentCandidate {
                    archive_path: segment.archive_path,
                })
                .collect()
        })
}
#[cfg(feature = "duckdb-runtime")]
fn remove_duckdb_segment_files(path: &Path) -> anyhow::Result<usize> {
    let existed = path.exists();
    remove_file_if_exists(path)?;
    remove_file_if_exists(&duckdb_wal_path(path))?;
    Ok(usize::from(existed))
}
#[cfg(feature = "duckdb-runtime")]
fn prune_orphan_archived_duckdb_files(
    config: &TieredDuckDbUsageConfig,
    catalog_backend: &TieredUsageCatalogBackend,
) -> anyhow::Result<usize> {
    let referenced = catalog_archived_duckdb_paths(catalog_backend)?;
    let mut deleted = 0usize;
    let mut candidates = Vec::new();
    collect_files_recursive(&config.archive_dir, &mut candidates)?;
    for path in candidates {
        let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if !file_name.ends_with(".duckdb") || file_name.ends_with(".uploading.duckdb") {
            continue;
        }
        if referenced.contains(&path) {
            continue;
        }
        deleted = deleted.saturating_add(remove_duckdb_segment_files(&path)?);
        if let Some(parent) = path.parent() {
            let _ = prune_empty_directories_up_to(&config.archive_dir, parent);
        }
    }
    Ok(deleted)
}
#[cfg(feature = "duckdb-runtime")]
fn catalog_archived_duckdb_paths(
    catalog_backend: &TieredUsageCatalogBackend,
) -> anyhow::Result<HashSet<PathBuf>> {
    catalog_backend.archived_paths()
}
#[cfg(feature = "duckdb-runtime")]
pub fn prune_expired_detail_day_buckets(
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
pub fn duckdb_wal_path(path: &Path) -> PathBuf {
    let mut path = path.as_os_str().to_os_string();
    path.push(".wal");
    PathBuf::from(path)
}
#[cfg(feature = "duckdb-runtime")]
pub fn rollover_active_segment(
    config: &TieredDuckDbUsageConfig,
    state: &mut TieredDuckDbUsageState,
    connection_config: DuckDbUsageConnectionConfig,
    catalog_backend: Arc<TieredUsageCatalogBackend>,
) -> anyhow::Result<()> {
    state.active_writer = None;
    checkpoint_duckdb_path(&state.active_path, connection_config)?;
    let sequence = parse_segment_sequence(&state.active_path).unwrap_or(state.next_sequence);
    let segment_id = format!("usage-{}-{sequence:012}", now_ms());
    let pending_path = tiered_pending_dir(config).join(format!("{segment_id}.duckdb"));
    fs::rename(&state.active_path, &pending_path).with_context(|| {
        format!(
            "failed to move active duckdb segment `{}` to pending `{}`",
            state.active_path.display(),
            pending_path.display()
        )
    })?;
    let new_active_path = active_segment_path(config, state.next_sequence);
    state.next_sequence = state.next_sequence.saturating_add(1);
    initialize_duckdb_target_path(&new_active_path)?;
    state.active_path = new_active_path;
    state.active_has_rows = false;
    state.active_writer = None;
    spawn_segment_sealer(
        config.clone(),
        catalog_backend,
        pending_path,
        segment_id,
        Arc::new(RwLock::new(connection_config)),
    );
    Ok(())
}
#[cfg(feature = "duckdb-runtime")]
fn checkpoint_duckdb_path(
    path: &Path,
    connection_config: DuckDbUsageConnectionConfig,
) -> anyhow::Result<()> {
    let conn = DuckDbUsageRepository::open_checkpoint_conn(path, connection_config)?;
    conn.execute_batch("CHECKPOINT;")
        .with_context(|| format!("failed to checkpoint duckdb database `{}`", path.display()))?;
    Ok(())
}
#[cfg(feature = "duckdb-runtime")]
pub fn ensure_single_writer(
    state: &mut SingleDuckDbUsageState,
    connection_config: DuckDbUsageConnectionConfig,
) -> anyhow::Result<&mut PersistentUsageWriter> {
    let should_reopen = state
        .writer
        .as_ref()
        .map(|writer| writer.connection_config != connection_config)
        .unwrap_or(true);
    if should_reopen {
        state.writer = Some(PersistentUsageWriter::open(&state.path, connection_config, None)?);
    }
    state
        .writer
        .as_mut()
        .ok_or_else(|| anyhow!("single usage writer missing after initialization"))
}
