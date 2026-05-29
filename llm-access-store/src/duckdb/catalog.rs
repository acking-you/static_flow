//! Tiered catalog lifecycle: segment sealing/publishing (sync + async),
//! catalog seeding/refresh from archives, pending-segment compaction, active
//! segment selection, and validation.

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

#[cfg(feature = "duckdb-runtime")]
pub(crate) fn tiered_compacting_dir(config: &TieredDuckDbUsageConfig) -> PathBuf {
    config.active_dir.join("compacting")
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn compacting_segment_path(
    config: &TieredDuckDbUsageConfig,
    segment_id: &str,
) -> PathBuf {
    tiered_compacting_dir(config).join(format!("{segment_id}.tmp.duckdb"))
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn clear_stale_compacting_files(config: &TieredDuckDbUsageConfig) -> anyhow::Result<()> {
    let compacting_dir = tiered_compacting_dir(config);
    for entry in fs::read_dir(&compacting_dir).with_context(|| {
        format!("failed to read compacting duckdb directory `{}`", compacting_dir.display())
    })? {
        let entry = entry?;
        let path = entry.path();
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if path.is_file()
            && (file_name.ends_with(".tmp.duckdb") || file_name.ends_with(".tmp.duckdb.wal"))
        {
            remove_file_if_exists(&path)?;
        }
    }
    Ok(())
}
#[cfg(all(test, feature = "duckdb-runtime"))]
pub(crate) fn initialize_tiered_catalog(config: &TieredDuckDbUsageConfig) -> anyhow::Result<()> {
    fs::create_dir_all(&config.active_dir).with_context(|| {
        format!("failed to create tiered active directory `{}`", config.active_dir.display())
    })?;
    fs::create_dir_all(&config.archive_dir).with_context(|| {
        format!("failed to create tiered archive directory `{}`", config.archive_dir.display())
    })?;
    Ok(())
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn choose_active_segment(
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
pub(crate) fn sealed_at_ms_for_segment(segment_id: &str) -> i64 {
    segment_id
        .split('-')
        .nth(1)
        .and_then(|raw| raw.parse::<i64>().ok())
        .unwrap_or_else(now_ms)
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) fn spawn_existing_pending_sealers(
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
pub(crate) fn spawn_segment_sealer(
    config: TieredDuckDbUsageConfig,
    catalog_backend: Arc<TieredUsageCatalogBackend>,
    pending_path: PathBuf,
    segment_id: String,
    connection_config: SharedDuckDbUsageConnectionConfig,
) {
    let _ = thread::Builder::new()
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
}
#[cfg(feature = "duckdb-runtime")]
pub(crate) async fn publish_pending_segment_async(
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
pub(crate) fn finalize_archived_segment(
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
pub(crate) fn seed_catalog_from_archives_if_empty(
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
pub(crate) fn refresh_catalog_from_archives_if_needed(
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
pub(crate) fn publish_archive_path_to_catalog(
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
pub(crate) fn compact_pending_segment_to_local_file(
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
pub(crate) fn validate_compacted_segment_matches_source(
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
pub(crate) fn publish_segment_catalog(
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
#[cfg(feature = "duckdb-runtime")]
pub(crate) async fn append_usage_events_to_tiered(
    config: &TieredDuckDbUsageConfig,
    state: &Mutex<TieredDuckDbUsageState>,
    connection_config: &SharedDuckDbUsageConnectionConfig,
    catalog_backend: &Arc<TieredUsageCatalogBackend>,
    rows: &[UsageEventRow],
) -> anyhow::Result<()> {
    let connection_config_snapshot = connection_config_snapshot(connection_config);
    let mut writer = {
        let mut state = state
            .lock()
            .map_err(|_| anyhow!("tiered duckdb state lock poisoned"))?;
        if state.active_has_rows
            && active_segment_disk_bytes(&state.active_path) >= config.rollover_bytes.max(1)
        {
            rollover_active_segment(
                config,
                &mut state,
                connection_config_snapshot,
                Arc::clone(catalog_backend),
            )?;
        }
        let should_reopen = state
            .active_writer
            .as_ref()
            .map(|writer| writer.connection_config != connection_config_snapshot)
            .unwrap_or(true);
        if should_reopen {
            state.active_writer = Some(PersistentUsageWriter::open(
                &state.active_path,
                connection_config_snapshot,
                state.detail_store.clone(),
            )?);
        }
        state
            .active_writer
            .take()
            .ok_or_else(|| anyhow!("tiered active writer missing after initialization"))?
    };
    writer.writer.insert_usage_events(rows).await?;
    let mut state = state
        .lock()
        .map_err(|_| anyhow!("tiered duckdb state lock poisoned"))?;
    state.active_has_rows = true;
    state.active_writer = Some(writer);
    if active_segment_disk_bytes(&state.active_path) >= config.rollover_bytes.max(1) {
        rollover_active_segment(
            config,
            &mut state,
            connection_config_snapshot,
            Arc::clone(catalog_backend),
        )?;
    }
    Ok(())
}
