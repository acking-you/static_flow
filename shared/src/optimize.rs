use std::{collections::HashSet, time::Instant};

use futures::future::join_all;
use lancedb::{
    table::{CompactionOptions, OptimizeAction},
    Connection, Table,
};

const DEFAULT_FRAGMENT_THRESHOLD: usize = 10;
const MAINTENANCE_COMPACTION_BATCH_SIZE: usize = 1024;
const MAINTENANCE_COMPACTION_THREADS: usize = 1;
const SAFE_COMPACTION_BATCH_SIZE: usize = 8;
const SAFE_COMPACTION_MAX_ROWS_PER_GROUP: usize = 8;
const SAFE_COMPACTION_MAX_BYTES_PER_FILE: usize = 512 * 1024 * 1024;
const SMALL_FRAGMENT_ROW_THRESHOLD: usize = 100_000;

pub struct CompactConfig {
    pub enabled: bool,
    pub fragment_threshold: usize,
    pub prune_older_than_hours: i64,
    /// Tables to skip during compaction.
    pub skip_tables: HashSet<String>,
}

impl Default for CompactConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            fragment_threshold: DEFAULT_FRAGMENT_THRESHOLD,
            prune_older_than_hours: 2,
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

pub struct CompactResult {
    pub table: String,
    pub small_fragments: usize,
    pub action: CompactAction,
    pub elapsed_ms: u128,
    pub compacted: bool,
    pub error: Option<String>,
}

/// Scan tables, compact only those exceeding the fragment threshold.
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
                action: CompactAction::SkippedByConfig,
                elapsed_ms: 0,
                compacted: false,
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
                    compacted: bool,
                    error: Option<String>| CompactResult {
        table: name.to_string(),
        small_fragments,
        action,
        elapsed_ms: started.elapsed().as_millis(),
        compacted,
        error,
    };

    if !config.enabled {
        return finalize(CompactAction::CompactionDisabled, 0, false, None);
    }

    let table = match db.open_table(name).execute().await {
        Ok(t) => t,
        Err(err) => {
            return finalize(
                CompactAction::OpenFailed,
                0,
                false,
                Some(format!("open failed: {err:#}")),
            )
        },
    };

    let small = match count_small_fragments(&table).await {
        Ok(count) => count,
        Err(err) => {
            return finalize(
                CompactAction::StatsFailed,
                0,
                false,
                Some(format!("fragment scan failed: {err}")),
            )
        },
    };
    if small < config.fragment_threshold {
        return finalize(CompactAction::SkippedBelowThreshold, small, false, None);
    }

    let optimize_path = match optimize_compaction_with_fallback(&table).await {
        Ok(path) => path,
        Err(err) => return finalize(CompactAction::CompactFailed, small, false, Some(err)),
    };

    if let Err(err) = table
        .optimize(OptimizeAction::Prune {
            older_than: Some(chrono::Duration::hours(config.prune_older_than_hours)),
            delete_unverified: Some(false),
            error_if_tagged_old_versions: Some(false),
        })
        .await
    {
        return finalize(
            CompactAction::CompactedPruneFailed,
            small,
            true,
            Some(format!("prune failed: {err:#}")),
        );
    }

    let action = match optimize_path {
        OptimizePath::Maintenance => CompactAction::CompactedMaintenance,
        OptimizePath::SafeFallback => CompactAction::CompactedSafeFallback,
    };
    finalize(action, small, true, None)
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

async fn count_small_fragments(table: &Table) -> Result<usize, String> {
    let ds_wrapper = table
        .dataset()
        .ok_or_else(|| "table has no native dataset".to_string())?;
    let dataset = ds_wrapper
        .get()
        .await
        .map_err(|err| format!("failed to load dataset: {err:#}"))?;
    let fragments = dataset.get_fragments();
    let sizes = join_all(fragments.iter().map(|fragment| async move {
        match fragment.fast_physical_rows() {
            Ok(rows) => rows,
            Err(_) => fragment.physical_rows().await.unwrap_or(0),
        }
    }))
    .await;

    Ok(sizes
        .into_iter()
        .filter(|rows| *rows < SMALL_FRAGMENT_ROW_THRESHOLD)
        .count())
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
        path::PathBuf,
        sync::Arc,
        time::{SystemTime, UNIX_EPOCH},
    };

    use arrow_array::{Int32Array, RecordBatch, RecordBatchIterator, RecordBatchReader};
    use arrow_schema::{DataType, Field, Schema};
    use lancedb::connect;

    use super::{count_small_fragments, is_offset_overflow_error, CompactAction};

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

    fn temp_db_dir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        std::env::temp_dir().join(format!("staticflow-optimize-test-{nanos}"))
    }
}
