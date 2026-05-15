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
        usage_worker::{router, UsageWorker},
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
    let sqlite_control_path = storage.sqlite_control.clone();
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
            Arc::clone(&connection_config),
        )?
    } else {
        DuckDbUsageRepository::open_path_with_connection_config(
            storage.duckdb,
            Arc::clone(&connection_config),
        )?
    };
    let duckdb = Arc::new(duckdb);
    let worker = UsageWorker::new_with_retention_days(
        storage.usage_journal_dir,
        duckdb,
        runtime_config.usage_journal_consumer_lease_ms,
        runtime_config.usage_analytics_retention_days,
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
        if let Err(err) = runtime.block_on(run_forever_with_runtime_config(
            worker,
            sqlite_control_path,
            connection_config,
        )) {
            tracing::error!("llm access usage worker import loop stopped: {err:#}");
        }
    });
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(bind_addr).await?;
        axum::serve(listener, app.into_make_service()).await?;
        Ok(())
    })
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
async fn run_forever_with_runtime_config(
    worker: llm_access::usage_worker::UsageWorker,
    sqlite_control_path: std::path::PathBuf,
    connection_config: std::sync::Arc<
        std::sync::RwLock<llm_access_store::duckdb::DuckDbUsageConnectionConfig>,
    >,
) -> anyhow::Result<()> {
    use std::time::{Duration, Instant};

    use llm_access_core::store::AdminConfigStore;
    use llm_access_store::{
        duckdb::DuckDbUsageConnectionConfig, repository::SqliteControlRepository,
    };

    const RUNTIME_CONFIG_REFRESH_INTERVAL: Duration = Duration::from_secs(60);
    const USAGE_ANALYTICS_MAINTENANCE_INTERVAL: Duration = Duration::from_secs(60 * 60);

    let control = SqliteControlRepository::open_path(sqlite_control_path)?;
    let mut last_config_refresh = None::<Instant>;
    let mut last_maintenance = None::<Instant>;
    loop {
        if last_config_refresh
            .map(|last| last.elapsed() >= RUNTIME_CONFIG_REFRESH_INTERVAL)
            .unwrap_or(true)
        {
            match control.get_admin_runtime_config().await {
                Ok(runtime_config) => {
                    if let Ok(mut current) = connection_config.write() {
                        *current =
                            DuckDbUsageConnectionConfig::from_admin_runtime_config(&runtime_config);
                    }
                    worker.set_usage_analytics_retention_days(
                        runtime_config.usage_analytics_retention_days,
                    );
                },
                Err(err) => tracing::warn!(
                    "failed to refresh llm access usage worker runtime config: {err:#}"
                ),
            }
            last_config_refresh = Some(Instant::now());
        }
        if let Err(err) = worker.run_one_import().await {
            worker.record_error(&err);
            return Err(err);
        }
        if last_maintenance
            .map(|last| last.elapsed() >= USAGE_ANALYTICS_MAINTENANCE_INTERVAL)
            .unwrap_or(true)
        {
            if let Err(err) = worker.run_maintenance(now_ms()).await {
                worker.record_error(&err);
                return Err(err);
            }
            last_maintenance = Some(Instant::now());
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(not(any(feature = "duckdb-runtime", feature = "duckdb-bundled")))]
fn main() -> anyhow::Result<()> {
    anyhow::bail!("llm-access-usage-worker requires duckdb-runtime")
}
