//! Usage-analytics retention: tiered pruning, active-segment rollover/discard,
//! catalog segment deletion, orphan-file cleanup, and detail-bucket expiry.

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

#[cfg(feature = "duckdb-runtime")]
pub(crate) fn remove_file_if_exists(path: &Path) -> anyhow::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("failed to remove file `{}`", path.display())),
    }
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn prune_empty_directories_up_to(root: &Path, start: &Path) -> anyhow::Result<usize> {
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
pub(crate) fn collect_files_recursive(root: &Path, files: &mut Vec<PathBuf>) -> anyhow::Result<()> {
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
pub(crate) async fn prune_tiered_usage_analytics(
    config: &TieredDuckDbUsageConfig,
    state: &Mutex<TieredDuckDbUsageState>,
    connection_config: &SharedDuckDbUsageConnectionConfig,
    catalog_backend: &TieredUsageCatalogBackend,
    now_ms: i64,
    retention_days: u64,
) -> anyhow::Result<UsageAnalyticsPruneReport> {
    let cutoff_ms = usage_analytics_retention_cutoff_ms(now_ms, retention_days);
    let mut deleted_files = rollover_expired_active_segment(
        config,
        state,
        connection_config_snapshot(connection_config),
        cutoff_ms,
    )?;
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
pub(crate) fn usage_analytics_retention_cutoff_ms(now_ms: i64, retention_days: u64) -> i64 {
    let retention_days = i64::try_from(retention_days.max(1)).unwrap_or(i64::MAX);
    now_ms.saturating_sub(retention_days.saturating_mul(USAGE_ANALYTICS_RETENTION_DAY_MS))
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn rollover_expired_active_segment(
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
pub(crate) fn discard_expired_active_segment(
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
pub(crate) fn delete_expired_segments_from_catalog(
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
pub(crate) fn remove_duckdb_segment_files(path: &Path) -> anyhow::Result<usize> {
    let existed = path.exists();
    remove_file_if_exists(path)?;
    remove_file_if_exists(&duckdb_wal_path(path))?;
    Ok(usize::from(existed))
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn prune_orphan_archived_duckdb_files(
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
pub(crate) fn rollover_active_segment(
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
