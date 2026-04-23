use std::{
    collections::{hash_map::DefaultHasher, HashSet},
    fs::{self, File, OpenOptions},
    hash::{Hash, Hasher},
    path::PathBuf,
    time::Instant,
};

use fs2::FileExt;
use futures::stream::{self, StreamExt};
use lancedb::{
    table::{CompactionOptions, OptimizeAction, OptimizeOptions},
    Connection, Table,
};

const DEFAULT_FRAGMENT_THRESHOLD: usize = 10;
const MAINTENANCE_COMPACTION_BATCH_SIZE: usize = 1024;
const MAINTENANCE_COMPACTION_THREADS: usize = 1;
const SAFE_COMPACTION_BATCH_SIZE: usize = 8;
const SAFE_COMPACTION_MAX_ROWS_PER_GROUP: usize = 8;
const SAFE_COMPACTION_MAX_BYTES_PER_FILE: usize = 512 * 1024 * 1024;
const SMALL_FRAGMENT_ROW_THRESHOLD: usize = 100_000;
const FRAGMENT_SCAN_CONCURRENCY: usize = 32;

#[derive(Debug, Clone)]
pub struct CompactConfig {
    pub enabled: bool,
    pub fragment_threshold: usize,
    pub prune_older_than_hours: i64,
    pub optimize_dirty_indices: bool,
    /// Tables to skip during compaction.
    pub skip_tables: HashSet<String>,
}

impl Default for CompactConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            fragment_threshold: DEFAULT_FRAGMENT_THRESHOLD,
            prune_older_than_hours: 0,
            optimize_dirty_indices: true,
            skip_tables: HashSet::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactAction {
    CompactionDisabled,
    SkippedByConfig,
    SkippedBelowThreshold,
    CompactedMaintenance,
    CompactedSafeFallback,
    CompactedPruneFailed,
    OpenFailed,
    StatsFailed,
    CompactFailed,
}

impl CompactAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CompactionDisabled => "compaction_disabled",
            Self::SkippedByConfig => "skipped_by_config",
            Self::SkippedBelowThreshold => "skipped_below_threshold",
            Self::CompactedMaintenance => "compacted_maintenance",
            Self::CompactedSafeFallback => "compacted_safe_fallback",
            Self::CompactedPruneFailed => "compacted_prune_failed",
            Self::OpenFailed => "open_failed",
            Self::StatsFailed => "stats_failed",
            Self::CompactFailed => "compact_failed",
        }
    }
}

#[derive(Debug)]
pub struct CompactResult {
    pub table: String,
    pub small_fragments: usize,
    pub max_unindexed_rows: usize,
    pub action: CompactAction,
    pub elapsed_ms: u128,
    pub compacted: bool,
    pub pruned: bool,
    pub index_optimized: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableAccessMode {
    Shared,
    Exclusive,
}

#[derive(Debug)]
pub struct TableAccessFileGuard {
    file: File,
}

impl Drop for TableAccessFileGuard {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

pub async fn acquire_table_access_file_lock(
    lock_path: &std::path::Path,
    mode: TableAccessMode,
) -> Result<TableAccessFileGuard, String> {
    acquire_table_access_file_lock_inner(lock_path.to_path_buf(), mode).await
}

async fn acquire_table_access_file_lock_inner(
    lock_path: PathBuf,
    mode: TableAccessMode,
) -> Result<TableAccessFileGuard, String> {
    tokio::task::spawn_blocking(move || -> Result<TableAccessFileGuard, String> {
        if let Some(parent) = lock_path.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                format!("failed to create table lock dir `{}`: {err:#}", parent.display())
            })?;
        }
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|err| {
                format!("failed to open table lock file `{}`: {err:#}", lock_path.display())
            })?;
        let lock_result = match mode {
            TableAccessMode::Shared => FileExt::lock_shared(&file),
            TableAccessMode::Exclusive => FileExt::lock_exclusive(&file),
        };
        match lock_result {
            Ok(()) => Ok(TableAccessFileGuard {
                file,
            }),
            Err(err) => Err(format!(
                "failed to acquire {:?} lock for `{}`: {err:#}",
                mode,
                lock_path.display()
            )),
        }
    })
    .await
    .map_err(|err| format!("table lock task join failed: {err:#}"))?
}

pub async fn compact_table_with_fallback(table: &Table) -> Result<CompactAction, String> {
    table
        .repair_missing_frag_reuse_index()
        .await
        .map_err(|err| format!("failed to repair stale frag_reuse metadata: {err:#}"))?;
    let optimize_path = optimize_compaction_with_fallback(table).await?;
    Ok(match optimize_path {
        OptimizePath::Maintenance => CompactAction::CompactedMaintenance,
        OptimizePath::SafeFallback => CompactAction::CompactedSafeFallback,
    })
}

pub async fn prune_table_versions(
    table: &Table,
    older_than_hours: i64,
    delete_unverified: bool,
    error_if_tagged_old_versions: bool,
) -> Result<(), String> {
    table
        .optimize(OptimizeAction::Prune {
            older_than: Some(chrono::Duration::hours(older_than_hours)),
            delete_unverified: Some(delete_unverified),
            error_if_tagged_old_versions: Some(error_if_tagged_old_versions),
        })
        .await
        .map_err(|err| format!("prune failed: {err:#}"))?;
    Ok(())
}

/// Scan tables, compact those exceeding the fragment threshold, and prune old
/// versions on every enabled maintenance pass.
pub async fn scan_and_compact_tables(
    db: &Connection,
    table_names: &[&str],
    config: &CompactConfig,
) -> Vec<CompactResult> {
    let mut results = Vec::new();
    for &name in table_names {
        if config.skip_tables.contains(name) {
            results.push(CompactResult {
                table: name.to_string(),
                small_fragments: 0,
                max_unindexed_rows: 0,
                action: CompactAction::SkippedByConfig,
                elapsed_ms: 0,
                compacted: false,
                pruned: false,
                index_optimized: false,
                error: None,
            });
            continue;
        }
        results.push(check_and_compact(db, name, config).await);
    }
    results
}

async fn check_and_compact(db: &Connection, name: &str, config: &CompactConfig) -> CompactResult {
    let started = Instant::now();
    let finalize = |action: CompactAction,
                    small_fragments: usize,
                    max_unindexed_rows: usize,
                    compacted: bool,
                    pruned: bool,
                    index_optimized: bool,
                    error: Option<String>| CompactResult {
        table: name.to_string(),
        small_fragments,
        max_unindexed_rows,
        action,
        elapsed_ms: started.elapsed().as_millis(),
        compacted,
        pruned,
        index_optimized,
        error,
    };

    if !config.enabled {
        return finalize(CompactAction::CompactionDisabled, 0, 0, false, false, false, None);
    }

    let lock_path = local_table_rewrite_lock_path(db.uri(), name);
    let _file_guard =
        match acquire_table_access_file_lock(&lock_path, TableAccessMode::Exclusive).await {
            Ok(guard) => guard,
            Err(err) => {
                return finalize(
                    CompactAction::CompactFailed,
                    0,
                    0,
                    false,
                    false,
                    false,
                    Some(format!("table lock failed: {err}")),
                )
            },
        };

    let table = match db.open_table(name).execute().await {
        Ok(t) => t,
        Err(err) => {
            return finalize(
                CompactAction::OpenFailed,
                0,
                0,
                false,
                false,
                false,
                Some(format!("open failed: {err:#}")),
            )
        },
    };
    check_opened_table_and_compact(&table, config).await
}

pub async fn check_opened_table_and_compact(
    table: &Table,
    config: &CompactConfig,
) -> CompactResult {
    let started = Instant::now();
    let finalize = |action: CompactAction,
                    small_fragments: usize,
                    max_unindexed_rows: usize,
                    compacted: bool,
                    pruned: bool,
                    index_optimized: bool,
                    error: Option<String>| CompactResult {
        table: table.name().to_string(),
        small_fragments,
        max_unindexed_rows,
        action,
        elapsed_ms: started.elapsed().as_millis(),
        compacted,
        pruned,
        index_optimized,
        error,
    };

    if !config.enabled {
        return finalize(CompactAction::CompactionDisabled, 0, 0, false, false, false, None);
    }

    let small = match count_small_fragments(table).await {
        Ok(count) => count,
        Err(err) => {
            return finalize(
                CompactAction::StatsFailed,
                0,
                0,
                false,
                false,
                false,
                Some(format!("fragment scan failed: {err}")),
            )
        },
    };
    let max_unindexed_rows = match max_unindexed_rows(table).await {
        Ok(count) => count,
        Err(err) => {
            return finalize(
                CompactAction::StatsFailed,
                small,
                0,
                false,
                false,
                false,
                Some(format!("index scan failed: {err}")),
            )
        },
    };
    if small < config.fragment_threshold {
        // Even when compaction is skipped, old versions should still be pruned
        // and lagging indices repaired through the same maintenance entrypoint.
        let pruned =
            match prune_table_versions_with_recovery(table, config.prune_older_than_hours).await {
                Ok(_) => true,
                Err(err) => {
                    return finalize(
                        CompactAction::SkippedBelowThreshold,
                        small,
                        max_unindexed_rows,
                        false,
                        false,
                        false,
                        Some(err),
                    );
                },
            };
        let index_optimized = match optimize_dirty_indices_if_needed(
            table,
            max_unindexed_rows,
            config.optimize_dirty_indices,
        )
        .await
        {
            Ok(changed) => changed,
            Err(err) => {
                return finalize(
                    CompactAction::SkippedBelowThreshold,
                    small,
                    max_unindexed_rows,
                    false,
                    pruned,
                    false,
                    Some(err),
                );
            },
        };
        return finalize(
            CompactAction::SkippedBelowThreshold,
            small,
            max_unindexed_rows,
            false,
            pruned,
            index_optimized,
            None,
        );
    }

    let action = match compact_table_with_fallback(table).await {
        Ok(action) => action,
        Err(err) => {
            return finalize(
                CompactAction::CompactFailed,
                small,
                max_unindexed_rows,
                false,
                false,
                false,
                Some(err),
            )
        },
    };

    if let Err(err) = prune_table_versions_with_recovery(table, config.prune_older_than_hours).await
    {
        return finalize(
            CompactAction::CompactedPruneFailed,
            small,
            max_unindexed_rows,
            true,
            false,
            false,
            Some(err),
        );
    }

    let index_optimized = match optimize_dirty_indices_if_needed(
        table,
        max_unindexed_rows,
        config.optimize_dirty_indices,
    )
    .await
    {
        Ok(changed) => changed,
        Err(err) => {
            return finalize(action, small, max_unindexed_rows, true, true, false, Some(err));
        },
    };

    finalize(action, small, max_unindexed_rows, true, true, index_optimized, None)
}

enum OptimizePath {
    Maintenance,
    SafeFallback,
}

async fn optimize_compaction_with_fallback(table: &Table) -> Result<OptimizePath, String> {
    let options = maintenance_compaction_options(table).await?;
    match table
        .optimize(OptimizeAction::Compact {
            options: options.clone(),
            remap_options: None,
        })
        .await
    {
        Ok(_) => Ok(OptimizePath::Maintenance),
        Err(err) => {
            if !is_offset_overflow_error(&err) {
                return Err(format!("compact failed: {err:#}"));
            }

            let options = CompactionOptions {
                num_threads: Some(MAINTENANCE_COMPACTION_THREADS),
                batch_size: Some(SAFE_COMPACTION_BATCH_SIZE),
                max_rows_per_group: SAFE_COMPACTION_MAX_ROWS_PER_GROUP,
                max_bytes_per_file: Some(SAFE_COMPACTION_MAX_BYTES_PER_FILE),
                defer_index_remap: options.defer_index_remap,
                ..CompactionOptions::default()
            };

            if let Err(fallback_err) = table
                .optimize(OptimizeAction::Compact {
                    options,
                    remap_options: None,
                })
                .await
            {
                return Err(format!(
                    "compact failed: {err:#}; safe compact fallback failed: {fallback_err:#}"
                ));
            }

            Ok(OptimizePath::SafeFallback)
        },
    }
}

fn is_offset_overflow_error(err: &dyn std::error::Error) -> bool {
    err.to_string().contains("Offset overflow error")
}

pub fn local_table_access_lock_path(db_uri: &str, table: &str) -> PathBuf {
    local_table_lock_path(db_uri, table, "access")
}

pub fn local_table_rewrite_lock_path(db_uri: &str, table: &str) -> PathBuf {
    local_table_lock_path(db_uri, table, "rewrite")
}

fn local_table_lock_path(db_uri: &str, table: &str, scope: &str) -> PathBuf {
    let table_uri = format!("{}/{}.lance", db_uri.trim_end_matches('/'), table);
    table_access_lock_path(&format!("{scope}:{table_uri}"))
}

fn table_access_lock_path(lock_key: &str) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    lock_key.hash(&mut hasher);
    let hash = hasher.finish();
    let label = lock_key
        .rsplit('/')
        .next()
        .unwrap_or(lock_key)
        .trim_end_matches(".lance")
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') { ch } else { '_' })
        .collect::<String>();
    let label = if label.is_empty() { "table" } else { label.as_str() };
    std::env::temp_dir()
        .join("staticflow-table-locks")
        .join(format!("{label}-{hash:016x}.lock"))
}

async fn prune_table_versions_with_recovery(
    table: &Table,
    older_than_hours: i64,
) -> Result<bool, String> {
    match prune_table_versions(table, older_than_hours, false, false).await {
        Ok(()) => Ok(false),
        Err(err) if should_use_delete_unverified_prune_recovery(table.name(), &err) => {
            let table_uri = table.uri().await.map_err(|fallback_err| {
                format!(
                    "{err}; failed to resolve table uri before delete_unverified cleanup \
                     fallback: {fallback_err:#}"
                )
            })?;
            let lock_path = table_access_lock_path(&format!("access:{table_uri}"));
            let _file_guard =
                acquire_table_access_file_lock(&lock_path, TableAccessMode::Exclusive)
                    .await
                    .map_err(|fallback_err| {
                        format!(
                            "{err}; failed to acquire exclusive table access lock before \
                             delete_unverified cleanup fallback: {fallback_err}"
                        )
                    })?;
            tracing::warn!(
                table = table.name(),
                error = %err,
                "standard prune failed with missing manifest; retrying delete_unverified cleanup"
            );
            prune_table_versions(table, 0, true, false)
                .await
                .map(|()| true)
                .map_err(|fallback_err| {
                    format!("{err}; delete_unverified cleanup fallback failed: {fallback_err}")
                })
        },
        Err(err) => Err(err),
    }
}

fn should_use_delete_unverified_prune_recovery(table: &str, error: &str) -> bool {
    if table != "llm_gateway_usage_events" {
        return false;
    }
    let normalized = error.to_ascii_lowercase();
    normalized.contains("manifest") && normalized.contains("not found")
}

async fn count_small_fragments(table: &Table) -> Result<usize, String> {
    let ds_wrapper = table
        .dataset()
        .ok_or_else(|| "table has no native dataset".to_string())?;
    let dataset = ds_wrapper
        .get()
        .await
        .map_err(|err| format!("failed to load dataset: {err:#}"))?;
    let fragments = dataset.get_fragments();
    let small = stream::iter(fragments.into_iter().map(|fragment| async move {
        match fragment.fast_physical_rows() {
            Ok(rows) => rows,
            Err(_) => fragment.physical_rows().await.unwrap_or(0),
        }
    }))
    // Large fragmented tables can have thousands of fragments. Bound the scan
    // fan-out so maintenance does not create an avoidable memory spike while
    // it is merely counting small fragments.
    .buffer_unordered(FRAGMENT_SCAN_CONCURRENCY)
    .fold(0usize, |count, rows| async move {
        count + usize::from(rows < SMALL_FRAGMENT_ROW_THRESHOLD)
    })
    .await;

    Ok(small)
}

async fn max_unindexed_rows(table: &Table) -> Result<usize, String> {
    let indices = table
        .list_indices()
        .await
        .map_err(|err| format!("failed to list indices: {err:#}"))?;
    let mut max_rows = 0usize;
    for index in indices {
        if let Some(stats) = table
            .index_stats(&index.name)
            .await
            .map_err(|err| format!("failed to inspect index `{}`: {err:#}", index.name))?
        {
            max_rows = max_rows.max(stats.num_unindexed_rows);
        }
    }
    Ok(max_rows)
}

async fn optimize_dirty_indices_if_needed(
    table: &Table,
    max_unindexed_rows: usize,
    enabled: bool,
) -> Result<bool, String> {
    if !enabled || max_unindexed_rows == 0 {
        return Ok(false);
    }
    // Only rebuild indices when the table reports real lag. Fragment count and
    // row count alone are not enough reason to rewrite every index.
    table
        .optimize(OptimizeAction::Index(OptimizeOptions::default()))
        .await
        .map_err(|err| format!("index optimize failed: {err:#}"))?;
    Ok(true)
}

async fn maintenance_compaction_options(table: &Table) -> Result<CompactionOptions, String> {
    let defer_index_remap = !table_uses_stable_row_ids(table).await?;
    Ok(CompactionOptions {
        num_threads: Some(MAINTENANCE_COMPACTION_THREADS),
        batch_size: Some(MAINTENANCE_COMPACTION_BATCH_SIZE),
        defer_index_remap,
        ..CompactionOptions::default()
    })
}

async fn table_uses_stable_row_ids(table: &Table) -> Result<bool, String> {
    let ds_wrapper = table
        .dataset()
        .ok_or_else(|| "table has no native dataset".to_string())?;
    let dataset = ds_wrapper
        .get()
        .await
        .map_err(|err| format!("failed to load dataset: {err:#}"))?;
    Ok(dataset.manifest().uses_stable_row_ids())
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashSet,
        fs::OpenOptions,
        path::PathBuf,
        sync::Arc,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    use arrow_array::{Int32Array, RecordBatch, RecordBatchIterator, RecordBatchReader};
    use arrow_schema::{DataType, Field, Schema};
    use fs2::FileExt;
    use lancedb::connect;

    use super::{
        check_and_compact, check_opened_table_and_compact, count_small_fragments,
        is_offset_overflow_error, local_table_rewrite_lock_path, CompactAction, CompactConfig,
    };

    #[derive(Debug)]
    struct MockErr(&'static str);

    impl std::fmt::Display for MockErr {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.0)
        }
    }

    impl std::error::Error for MockErr {}

    #[test]
    fn detects_offset_overflow_error() {
        let err = MockErr("LanceError(Arrow): Offset overflow error: 2149941176");
        assert!(is_offset_overflow_error(&err));
    }

    #[test]
    fn ignores_other_errors() {
        let err = MockErr("LanceError(IO): External error");
        assert!(!is_offset_overflow_error(&err));
    }

    #[test]
    fn compact_action_labels_are_stable() {
        assert_eq!(CompactAction::CompactionDisabled.as_str(), "compaction_disabled");
        assert_eq!(CompactAction::SkippedByConfig.as_str(), "skipped_by_config");
        assert_eq!(CompactAction::CompactedMaintenance.as_str(), "compacted_maintenance");
        assert_eq!(CompactAction::CompactedSafeFallback.as_str(), "compacted_safe_fallback");
        assert_eq!(CompactAction::CompactFailed.as_str(), "compact_failed");
    }

    #[test]
    fn compact_config_defaults_prune_to_zero_hours() {
        let config = CompactConfig::default();
        assert_eq!(config.prune_older_than_hours, 0);
    }

    #[tokio::test]
    async fn count_small_fragments_reads_fragment_metadata_without_stats() {
        let dir = temp_db_dir();
        std::fs::create_dir_all(&dir).expect("create temp db dir");
        let uri = dir.to_string_lossy().to_string();
        let db = connect(&uri).execute().await.expect("connect temp db");
        let schema = Arc::new(Schema::new(vec![Field::new("value", DataType::Int32, false)]));

        for chunk in [[1_i32, 2], [3, 4], [5, 6]] {
            let batch = RecordBatch::try_new(schema.clone(), vec![Arc::new(Int32Array::from(
                chunk.to_vec(),
            ))])
            .expect("batch");
            let reader: Box<dyn RecordBatchReader + Send> =
                Box::new(RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema.clone()));
            if db.open_table("fragments").execute().await.is_ok() {
                let table = db
                    .open_table("fragments")
                    .execute()
                    .await
                    .expect("open table");
                table.add(reader).execute().await.expect("append rows");
            } else {
                db.create_table("fragments", reader)
                    .execute()
                    .await
                    .expect("create table");
            }
        }

        let table = db
            .open_table("fragments")
            .execute()
            .await
            .expect("open fragments");
        let small = count_small_fragments(&table)
            .await
            .expect("count small fragments");
        assert_eq!(small, 3);

        std::fs::remove_dir_all(&dir).expect("cleanup temp db dir");
    }

    #[tokio::test]
    async fn prune_runs_even_when_compaction_is_skipped_below_threshold() {
        let dir = temp_db_dir();
        std::fs::create_dir_all(&dir).expect("create temp db dir");
        let uri = dir.to_string_lossy().to_string();
        let db = connect(&uri).execute().await.expect("connect temp db");
        let schema = Arc::new(Schema::new(vec![Field::new("value", DataType::Int32, false)]));

        for chunk in [[1_i32, 2], [3, 4], [5, 6]] {
            let batch = RecordBatch::try_new(schema.clone(), vec![Arc::new(Int32Array::from(
                chunk.to_vec(),
            ))])
            .expect("batch");
            let reader: Box<dyn RecordBatchReader + Send> =
                Box::new(RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema.clone()));
            if db.open_table("versions").execute().await.is_ok() {
                let table = db
                    .open_table("versions")
                    .execute()
                    .await
                    .expect("open table");
                table.add(reader).execute().await.expect("append rows");
            } else {
                db.create_table("versions", reader)
                    .execute()
                    .await
                    .expect("create table");
            }
        }

        let table = db
            .open_table("versions")
            .execute()
            .await
            .expect("open versions table");
        let version_count_before = table
            .list_versions()
            .await
            .expect("list versions before prune")
            .len();
        assert!(version_count_before > 1);

        let result = check_opened_table_and_compact(&table, &CompactConfig {
            enabled: true,
            fragment_threshold: usize::MAX,
            prune_older_than_hours: 0,
            optimize_dirty_indices: true,
            skip_tables: HashSet::new(),
        })
        .await;
        assert_eq!(result.action, CompactAction::SkippedBelowThreshold);
        assert!(!result.compacted);
        assert!(result.pruned);
        assert!(!result.index_optimized);
        assert!(result.error.is_none());

        let version_count_after = table
            .list_versions()
            .await
            .expect("list versions after prune")
            .len();
        assert_eq!(version_count_after, 1);

        std::fs::remove_dir_all(&dir).expect("cleanup temp db dir");
    }

    #[tokio::test]
    async fn check_opened_table_and_compact_optimizes_dirty_indices_when_needed() {
        use lancedb::index::{scalar::BTreeIndexBuilder, Index};

        let dir = temp_db_dir();
        std::fs::create_dir_all(&dir).expect("create temp db dir");
        let uri = dir.to_string_lossy().to_string();
        let db = connect(&uri).execute().await.expect("connect temp db");
        let schema = Arc::new(Schema::new(vec![Field::new("value", DataType::Int32, false)]));

        let initial_batch = RecordBatch::try_new(schema.clone(), vec![Arc::new(
            Int32Array::from_iter_values(0..100),
        )])
        .expect("initial batch");
        let table = db
            .create_table("dirty_index", initial_batch)
            .execute()
            .await
            .expect("create dirty_index table");

        table
            .create_index(&["value"], Index::BTree(BTreeIndexBuilder::default()))
            .execute()
            .await
            .expect("create btree index");

        let appended_batch =
            RecordBatch::try_new(schema, vec![Arc::new(Int32Array::from_iter_values(100..200))])
                .expect("appended batch");
        table
            .add(appended_batch)
            .execute()
            .await
            .expect("append unindexed rows");

        let index_name = table.list_indices().await.expect("list indices")[0]
            .name
            .clone();
        let stats_before = table
            .index_stats(&index_name)
            .await
            .expect("index stats before")
            .expect("index stats exist");
        assert_eq!(stats_before.num_unindexed_rows, 100);

        let result = check_opened_table_and_compact(&table, &CompactConfig {
            enabled: true,
            fragment_threshold: usize::MAX,
            prune_older_than_hours: 0,
            optimize_dirty_indices: true,
            skip_tables: HashSet::new(),
        })
        .await;

        assert_eq!(result.action, CompactAction::SkippedBelowThreshold);
        assert!(!result.compacted);
        assert!(result.pruned);
        assert!(result.index_optimized);
        assert_eq!(result.max_unindexed_rows, 100);
        assert!(result.error.is_none());

        let stats_after = table
            .index_stats(&index_name)
            .await
            .expect("index stats after")
            .expect("index stats exist");
        assert_eq!(stats_after.num_unindexed_rows, 0);
        assert_eq!(table.count_rows(None).await.expect("count rows"), 200);

        std::fs::remove_dir_all(&dir).expect("cleanup temp db dir");
    }

    #[tokio::test]
    async fn check_and_compact_waits_for_rewrite_lock_before_pruning_versions() {
        let dir = temp_db_dir();
        std::fs::create_dir_all(&dir).expect("create temp db dir");
        let uri = dir.to_string_lossy().to_string();
        let db = connect(&uri).execute().await.expect("connect temp db");
        let schema = Arc::new(Schema::new(vec![Field::new("value", DataType::Int32, false)]));

        for chunk in [[1_i32, 2], [3, 4], [5, 6]] {
            let batch = RecordBatch::try_new(schema.clone(), vec![Arc::new(Int32Array::from(
                chunk.to_vec(),
            ))])
            .expect("batch");
            let reader: Box<dyn RecordBatchReader + Send> =
                Box::new(RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema.clone()));
            if db.open_table("locked_versions").execute().await.is_ok() {
                let table = db
                    .open_table("locked_versions")
                    .execute()
                    .await
                    .expect("open table");
                table.add(reader).execute().await.expect("append rows");
            } else {
                db.create_table("locked_versions", reader)
                    .execute()
                    .await
                    .expect("create table");
            }
        }

        let table = db
            .open_table("locked_versions")
            .execute()
            .await
            .expect("open locked_versions table");
        let version_count_before = table
            .list_versions()
            .await
            .expect("list versions before locked maintenance")
            .len();
        assert!(version_count_before > 1);

        let lock_path = local_table_rewrite_lock_path(&uri, "locked_versions");
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent).expect("create table lock dir");
        }
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .expect("open table lock file");
        file.lock_exclusive().expect("acquire exclusive table lock");

        let db_for_task = db.clone();
        let handle = tokio::spawn(async move {
            check_and_compact(&db_for_task, "locked_versions", &CompactConfig {
                enabled: true,
                fragment_threshold: usize::MAX,
                prune_older_than_hours: 0,
                optimize_dirty_indices: true,
                skip_tables: HashSet::new(),
            })
            .await
        });

        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            !handle.is_finished(),
            "maintenance must wait for the table lock instead of racing past it"
        );

        file.unlock().expect("release exclusive table lock");
        let result = handle.await.expect("maintenance task join");
        let version_count_after = table
            .list_versions()
            .await
            .expect("list versions after maintenance")
            .len();
        assert!(
            version_count_after < version_count_before,
            "maintenance should prune versions after the lock is released"
        );
        assert_eq!(result.action, CompactAction::SkippedBelowThreshold);
        assert!(result.pruned, "maintenance should prune versions once it acquires the lock");
        assert!(!result.compacted, "locked maintenance must not compact table files");

        std::fs::remove_dir_all(&dir).expect("cleanup temp db dir");
    }

    fn temp_db_dir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        std::env::temp_dir().join(format!("staticflow-optimize-test-{nanos}"))
    }
}
