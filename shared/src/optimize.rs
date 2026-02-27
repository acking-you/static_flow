use lancedb::{
    table::{CompactionOptions, OptimizeAction, OptimizeOptions},
    Connection, Table,
};

const DEFAULT_FRAGMENT_THRESHOLD: usize = 10;
const SAFE_COMPACTION_BATCH_SIZE: usize = 8;
const SAFE_COMPACTION_MAX_ROWS_PER_GROUP: usize = 8;
const SAFE_COMPACTION_MAX_BYTES_PER_FILE: usize = 512 * 1024 * 1024;

pub struct CompactConfig {
    pub fragment_threshold: usize,
    pub prune_older_than_hours: i64,
}

impl Default for CompactConfig {
    fn default() -> Self {
        Self {
            fragment_threshold: DEFAULT_FRAGMENT_THRESHOLD,
            prune_older_than_hours: 2,
        }
    }
}

pub struct CompactResult {
    pub table: String,
    pub small_fragments: usize,
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
        results.push(check_and_compact(db, name, config).await);
    }
    results
}

async fn check_and_compact(db: &Connection, name: &str, config: &CompactConfig) -> CompactResult {
    let table = match db.open_table(name).execute().await {
        Ok(t) => t,
        Err(err) => {
            return CompactResult {
                table: name.to_string(),
                small_fragments: 0,
                compacted: false,
                error: Some(format!("open failed: {err:#}")),
            }
        },
    };

    let stats = match table.stats().await {
        Ok(s) => s,
        Err(err) => {
            return CompactResult {
                table: name.to_string(),
                small_fragments: 0,
                compacted: false,
                error: Some(format!("stats failed: {err:#}")),
            }
        },
    };

    let small = stats.fragment_stats.num_small_fragments;
    if small < config.fragment_threshold {
        return CompactResult {
            table: name.to_string(),
            small_fragments: small,
            compacted: false,
            error: None,
        };
    }

    if let Err(err) = optimize_all_with_fallback(&table).await {
        return CompactResult {
            table: name.to_string(),
            small_fragments: small,
            compacted: false,
            error: Some(err),
        };
    }

    if let Err(err) = table
        .optimize(OptimizeAction::Prune {
            older_than: Some(chrono::Duration::hours(config.prune_older_than_hours)),
            delete_unverified: Some(false),
            error_if_tagged_old_versions: Some(false),
        })
        .await
    {
        return CompactResult {
            table: name.to_string(),
            small_fragments: small,
            compacted: true,
            error: Some(format!("prune failed: {err:#}")),
        };
    }

    CompactResult {
        table: name.to_string(),
        small_fragments: small,
        compacted: true,
        error: None,
    }
}

async fn optimize_all_with_fallback(table: &Table) -> Result<(), String> {
    match table.optimize(OptimizeAction::All).await {
        Ok(_) => Ok(()),
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

            Ok(())
        },
    }
}

fn is_offset_overflow_error(err: &dyn std::error::Error) -> bool {
    err.to_string().contains("Offset overflow error")
}

#[cfg(test)]
mod tests {
    use super::is_offset_overflow_error;

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
}
