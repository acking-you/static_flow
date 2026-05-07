//! Standalone usage journal consumer.

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
fn main() -> anyhow::Result<()> {
    use std::sync::{Arc, RwLock};

    use llm_access::{
        config::CliCommand,
        usage_worker::{run_forever, UsageWorker},
    };
    use llm_access_core::store::AdminRuntimeConfig;
    use llm_access_store::duckdb::{
        DuckDbUsageConnectionConfig, DuckDbUsageRepository, TieredDuckDbUsageConfig,
    };

    let command = CliCommand::parse(std::env::args_os())?;
    let storage = match command {
        CliCommand::Init(storage) => {
            llm_access::bootstrap_storage(&storage)?;
            return Ok(());
        },
        CliCommand::Serve(config) => config.storage,
    };
    llm_access::bootstrap_storage(&storage)?;
    let runtime_config = AdminRuntimeConfig::default();
    let connection_config = Arc::new(RwLock::new(
        DuckDbUsageConnectionConfig::from_admin_runtime_config(&runtime_config),
    ));
    let duckdb = if let Some(tiered) = storage.duckdb_tiered {
        DuckDbUsageRepository::open_tiered_with_connection_config(
            TieredDuckDbUsageConfig {
                active_dir: tiered.active_dir,
                archive_dir: tiered.archive_dir,
                catalog_dir: tiered.catalog_dir,
                rollover_bytes: tiered.rollover_bytes,
            },
            connection_config,
        )?
    } else {
        DuckDbUsageRepository::open_path_with_connection_config(storage.duckdb, connection_config)?
    };
    let worker = UsageWorker::new(storage.usage_journal_dir, Arc::new(duckdb))?;
    tokio::runtime::Runtime::new()?.block_on(run_forever(worker))
}

#[cfg(not(any(feature = "duckdb-runtime", feature = "duckdb-bundled")))]
fn main() -> anyhow::Result<()> {
    anyhow::bail!("llm-access-usage-worker requires duckdb-runtime")
}
