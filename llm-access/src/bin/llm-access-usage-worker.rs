//! Standalone usage journal consumer.

use better_mimalloc_rs::MiMalloc;

const DEFAULT_LOG_FILTER: &str =
    "warn,llm_access=info,llm_access_store=info,llm_usage_journal=info";

#[global_allocator]
static GLOBAL_MIMALLOC: MiMalloc = MiMalloc;

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
fn main() -> anyhow::Result<()> {
    MiMalloc::init();
    let _log_guards = static_flow_runtime::runtime_logging::init_runtime_logging(
        "llm-access-usage-worker",
        DEFAULT_LOG_FILTER,
    )?;
    run()
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
fn run() -> anyhow::Result<()> {
    use std::{
        ffi::OsString,
        sync::{Arc, RwLock},
    };

    use llm_access::{
        config::CliCommand,
        usage_worker::{router, run_forever, UsageWorker},
    };
    use llm_access_core::store::AdminConfigStore;
    use llm_access_store::{
        duckdb::{DuckDbUsageConnectionConfig, DuckDbUsageRepository, TieredDuckDbUsageConfig},
        repository::SqliteControlRepository,
    };

    let args = std::env::args_os().collect::<Vec<OsString>>();
    let explicit_bind = args.iter().any(|arg| arg == "--bind");
    let command = CliCommand::parse(args)?;
    let (bind_addr, storage) = match command {
        CliCommand::Init(storage) => {
            llm_access::bootstrap_storage(&storage)?;
            return Ok(());
        },
        CliCommand::Serve(config) => (config.bind_addr, config.storage),
    };
    llm_access::bootstrap_api_storage(&storage)?;
    let control = SqliteControlRepository::open_path(&storage.sqlite_control)?;
    let runtime = tokio::runtime::Runtime::new()?;
    let runtime_config = runtime.block_on(control.get_admin_runtime_config())?;
    let bind_addr = if explicit_bind {
        bind_addr
    } else {
        runtime_config
            .usage_query_bind_addr
            .parse()
            .unwrap_or(bind_addr)
    };
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
                details_object_store_url: tiered.details_object_store_url,
            },
            connection_config,
        )?
    } else {
        DuckDbUsageRepository::open_path_with_connection_config(storage.duckdb, connection_config)?
    };
    let worker = UsageWorker::new(
        storage.usage_journal_dir,
        Arc::new(duckdb),
        runtime_config.usage_journal_consumer_lease_ms,
    )?;
    let app = router(&worker);
    std::thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(err) => {
                tracing::error!("failed to create usage worker import runtime: {err:#}");
                return;
            },
        };
        if let Err(err) = runtime.block_on(run_forever(worker)) {
            tracing::error!("llm access usage worker import loop stopped: {err:#}");
        }
    });
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(bind_addr).await?;
        axum::serve(listener, app.into_make_service()).await?;
        Ok(())
    })
}

#[cfg(not(any(feature = "duckdb-runtime", feature = "duckdb-bundled")))]
fn main() -> anyhow::Result<()> {
    anyhow::bail!("llm-access-usage-worker requires duckdb-runtime")
}
