use std::{collections::HashSet, time::Instant};

use lancedb::{
    table::{CompactionOptions, OptimizeAction, OptimizeOptions},
    Connection, Table,
};

const DEFAULT_FRAGMENT_THRESHOLD: usize = 10;
const SAFE_COMPACTION_BATCH_SIZE: usize = 8;
const SAFE_COMPACTION_MAX_ROWS_PER_GROUP: usize = 8;
const SAFE_COMPACTION_MAX_BYTES_PER_FILE: usize = 512 * 1024 * 1024;

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
    CompactedAll,
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
            Self::CompactedAll => "compacted_all",
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

    let stats = match table.stats().await {
        Ok(s) => s,
        Err(err) => {
            return finalize(
                CompactAction::StatsFailed,
                0,
                false,
                Some(format!("stats failed: {err:#}")),
            )
        },
    };

    let small = stats.fragment_stats.num_small_fragments;
    if small < config.fragment_threshold {
        return finalize(CompactAction::SkippedBelowThreshold, small, false, None);
    }

    let optimize_path = match optimize_all_with_fallback(&table).await {
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
        OptimizePath::All => CompactAction::CompactedAll,
        OptimizePath::SafeFallback => CompactAction::CompactedSafeFallback,
    };
    finalize(action, small, true, None)
}

enum OptimizePath {
    All,
    SafeFallback,
}

async fn optimize_all_with_fallback(table: &Table) -> Result<OptimizePath, String> {
    match table.optimize(OptimizeAction::All).await {
        Ok(_) => Ok(OptimizePath::All),
        Err(err) => {
            if !is_offset_overflow_error(&err) {
                return Err(format!("compact failed: {err:#}"));
            }

            let options = CompactionOptions {
                batch_size: Some(SAFE_COMPACTION_BATCH_SIZE),
                max_rows_per_group: SAFE_COMPACTION_MAX_ROWS_PER_GROUP,
                max_bytes_per_file: Some(SAFE_COMPACTION_MAX_BYTES_PER_FILE),
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

            if let Err(index_err) = table
                .optimize(OptimizeAction::Index(OptimizeOptions::default()))
                .await
            {
                return Err(format!(
                    "compact hit offset overflow and safe compact succeeded, but index rebuild \
                     failed: {index_err:#}"
                ));
            }

            Ok(OptimizePath::SafeFallback)
        },
    }
}

fn is_offset_overflow_error(err: &dyn std::error::Error) -> bool {
    err.to_string().contains("Offset overflow error")
}

#[cfg(test)]
mod tests {
    use super::{is_offset_overflow_error, CompactAction};

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
        assert_eq!(CompactAction::CompactedSafeFallback.as_str(), "compacted_safe_fallback");
        assert_eq!(CompactAction::CompactFailed.as_str(), "compact_failed");
    }
}
