use lancedb::{table::OptimizeAction, Connection};

const DEFAULT_FRAGMENT_THRESHOLD: usize = 10;

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
        }
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
        }
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

    if let Err(err) = table.optimize(OptimizeAction::All).await {
        return CompactResult {
            table: name.to_string(),
            small_fragments: small,
            compacted: false,
            error: Some(format!("compact failed: {err:#}")),
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
