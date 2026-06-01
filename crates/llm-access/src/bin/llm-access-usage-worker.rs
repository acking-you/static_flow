//! Standalone usage journal consumer.

use better_mimalloc_rs::MiMalloc;

const DEFAULT_LOG_FILTER: &str =
    "warn,llm_access=info,llm_access_store=info,llm_usage_journal=info";

#[global_allocator]
static GLOBAL_MIMALLOC: MiMalloc = MiMalloc;

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
fn main() -> anyhow::Result<()> {
    llm_access::allocator::configure_process_allocator_for_low_rss();
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

    use anyhow::Context;
    use llm_access::{
        config::{resolve_request_cache_config, CliCommand},
        usage_worker::{router, ClusterUsageWorker, EdgeUsageWorker, UsageWorker},
    };
    use llm_access_core::store::AdminConfigStore;
    use llm_access_store::{
        duckdb::{DuckDbUsageConnectionConfig, DuckDbUsageRepository, TieredDuckDbUsageConfig},
        postgres::PostgresControlRepository,
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
    llm_access::bootstrap_usage_worker_storage(&storage)?;
    let runtime = tokio::runtime::Runtime::new().context("failed to create tokio runtime")?;
    let cluster_state = runtime
        .block_on(llm_access::cluster::ClusterRuntimeState::from_storage_config(&storage))?;
    let database_url =
        std::env::var(&storage.control_store.database_url_env).with_context(|| {
            format!("missing control database env `{}`", storage.control_store.database_url_env)
        })?;
    let request_cache_config = resolve_request_cache_config(&storage)?;
    let control_repo: Arc<PostgresControlRepository> = runtime.block_on(async {
        PostgresControlRepository::connect(&database_url, request_cache_config.clone())
            .await
            .map(Arc::new)
    })?;
    let control: Arc<dyn AdminConfigStore> = control_repo.clone();
    let runtime_config = runtime.block_on(control.get_admin_runtime_config())?;
    let bind_addr = if explicit_bind {
        bind_addr
    } else {
        runtime_config
            .usage_query_bind_addr
            .parse()
            .unwrap_or(bind_addr)
    };
    let role = match cluster_state.as_ref() {
        Some(cluster_state) => runtime.block_on(cluster_state.runtime_role()),
        None => llm_access::cluster::NodeRuntimeRole::Primary,
    };
    let connection_config = Arc::new(RwLock::new(
        DuckDbUsageConnectionConfig::from_admin_runtime_config(&runtime_config),
    ));
    let mut maintenance = None::<PrimaryMaintenanceHandles>;
    let worker = match role {
        llm_access::cluster::NodeRuntimeRole::Primary => {
            let duckdb = if let Some(tiered) = storage.duckdb_tiered.clone() {
                DuckDbUsageRepository::open_tiered_with_postgres_catalog_with_connection_config(
                    TieredDuckDbUsageConfig {
                        active_dir: tiered.active_dir,
                        archive_dir: tiered.archive_dir,
                        rollover_bytes: tiered.rollover_bytes,
                        details_dir: tiered.details_dir,
                    },
                    Arc::clone(&connection_config),
                    &database_url,
                    request_cache_config.clone(),
                )?
            } else {
                DuckDbUsageRepository::open_path_with_connection_config(
                    storage.duckdb.clone(),
                    Arc::clone(&connection_config),
                )?
            };
            let primary = UsageWorker::new_with_retention_days(
                storage.usage_journal_dir.clone(),
                Arc::new(duckdb),
                runtime_config.usage_journal_consumer_lease_ms,
                runtime_config.usage_analytics_retention_days,
            )?
            .with_attribution_resolver(Some(Arc::new(
                llm_access::usage_worker::UsageEventAttributionResolver::new(Arc::clone(
                    &control_repo,
                )),
            )))
            .with_cluster_state(cluster_state.clone());
            maintenance = Some(PrimaryMaintenanceHandles {
                duckdb_usage: primary.usage_repository(),
                retention_days: primary.retention_days_handle(),
            });
            ClusterUsageWorker::Primary(primary)
        },
        llm_access::cluster::NodeRuntimeRole::EdgeSecondary
        | llm_access::cluster::NodeRuntimeRole::Degraded => {
            let cluster_state = cluster_state
                .clone()
                .context("edge usage worker requires cluster state")?;
            let source_node_id = storage
                .node_identity
                .as_ref()
                .map(|identity| identity.node_id.clone())
                .context("edge usage worker requires node identity")?;
            ClusterUsageWorker::Edge(EdgeUsageWorker::new(
                storage.usage_journal_dir.clone(),
                cluster_state,
                source_node_id,
                runtime_config.usage_journal_consumer_lease_ms,
            )?)
        },
    };
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
        if let Err(err) = runtime.block_on(run_import_forever_with_runtime_config(
            worker,
            control,
            connection_config,
        )) {
            tracing::error!("llm access usage worker import loop stopped: {err:#}");
        }
    });
    if let Some(maintenance) = maintenance {
        std::thread::spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(err) => {
                    tracing::error!("failed to create usage worker maintenance runtime: {err:#}");
                    return;
                },
            };
            if let Err(err) = runtime.block_on(run_maintenance_forever(maintenance)) {
                tracing::error!("llm access usage worker maintenance loop stopped: {err:#}");
            }
        });
    }
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(bind_addr).await?;
        axum::serve(listener, app.into_make_service()).await?;
        Ok(())
    })
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
#[derive(Clone)]
struct PrimaryMaintenanceHandles {
    duckdb_usage: std::sync::Arc<llm_access_store::duckdb::DuckDbUsageRepository>,
    retention_days: std::sync::Arc<std::sync::RwLock<u64>>,
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
async fn run_import_forever_with_runtime_config(
    worker: llm_access::usage_worker::ClusterUsageWorker,
    control: std::sync::Arc<dyn llm_access_core::store::AdminConfigStore>,
    connection_config: std::sync::Arc<
        std::sync::RwLock<llm_access_store::duckdb::DuckDbUsageConnectionConfig>,
    >,
) -> anyhow::Result<()> {
    use std::time::{Duration, Instant};

    use llm_access_store::duckdb::DuckDbUsageConnectionConfig;

    const RUNTIME_CONFIG_REFRESH_INTERVAL: Duration = Duration::from_secs(60);

    let mut last_config_refresh = None::<Instant>;
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
                    if let llm_access::usage_worker::ClusterUsageWorker::Primary(primary) = &worker
                    {
                        primary.set_usage_analytics_retention_days(
                            runtime_config.usage_analytics_retention_days,
                        );
                    }
                },
                Err(err) => tracing::warn!(
                    "failed to refresh llm access usage worker runtime config: {err:#}"
                ),
            }
            last_config_refresh = Some(Instant::now());
        }
        if let Err(err) = worker.run_one_cycle().await {
            worker.record_error(&err);
            return Err(err);
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
async fn run_maintenance_forever(handles: PrimaryMaintenanceHandles) -> anyhow::Result<()> {
    use std::time::{Duration, Instant};

    const USAGE_ANALYTICS_MAINTENANCE_INTERVAL: Duration = Duration::from_secs(5 * 60);

    let mut last_maintenance = None::<Instant>;
    loop {
        if last_maintenance
            .map(|last| last.elapsed() >= USAGE_ANALYTICS_MAINTENANCE_INTERVAL)
            .unwrap_or(true)
        {
            let retention_days = handles
                .retention_days
                .read()
                .map(|days| (*days).max(1))
                .unwrap_or(1);
            if let Err(err) = handles
                .duckdb_usage
                .prune_usage_analytics(now_ms(), retention_days)
                .await
            {
                tracing::warn!("llm access usage analytics maintenance failed: {err:#}");
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

#[cfg(all(test, any(feature = "duckdb-runtime", feature = "duckdb-bundled")))]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    type BoxUnitFuture = std::pin::Pin<Box<dyn std::future::Future<Output = ()>>>;
    type BoxResultFuture = std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>>>>;

    use tokio::sync::Notify;

    async fn run_import_loop_with_hooks(
        refresh_interval: std::time::Duration,
        mut refresh_runtime_config: impl FnMut() -> BoxUnitFuture,
        mut run_cycle: impl FnMut() -> BoxResultFuture,
        mut sleep: impl FnMut(std::time::Duration) -> BoxUnitFuture,
    ) -> anyhow::Result<()> {
        let mut last_config_refresh = None::<std::time::Instant>;
        loop {
            if last_config_refresh
                .map(|last| last.elapsed() >= refresh_interval)
                .unwrap_or(true)
            {
                refresh_runtime_config().await;
                last_config_refresh = Some(std::time::Instant::now());
            }
            run_cycle().await?;
            sleep(std::time::Duration::from_secs(1)).await;
        }
    }

    async fn run_maintenance_loop_with_hooks(
        maintenance_interval: std::time::Duration,
        mut run_maintenance: impl FnMut() -> BoxResultFuture,
        mut sleep: impl FnMut(std::time::Duration) -> BoxUnitFuture,
    ) -> anyhow::Result<()> {
        let mut last_maintenance = None::<std::time::Instant>;
        loop {
            if last_maintenance
                .map(|last| last.elapsed() >= maintenance_interval)
                .unwrap_or(true)
            {
                run_maintenance().await?;
                last_maintenance = Some(std::time::Instant::now());
            }
            sleep(std::time::Duration::from_secs(1)).await;
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn import_loop_keeps_progress_when_maintenance_loop_blocks() {
        let import_iterations = Arc::new(AtomicUsize::new(0));
        let maintenance_started = Arc::new(Notify::new());
        let maintenance_block = Arc::new(Notify::new());
        let local = tokio::task::LocalSet::new();

        local
            .run_until(async move {
                let import_task = tokio::task::spawn_local({
                    let import_iterations = Arc::clone(&import_iterations);
                    run_import_loop_with_hooks(
                        std::time::Duration::from_secs(60),
                        || Box::pin(async {}),
                        move || {
                            let import_iterations = Arc::clone(&import_iterations);
                            Box::pin(async move {
                                let iteration =
                                    import_iterations.fetch_add(1, Ordering::SeqCst) + 1;
                                if iteration >= 3 {
                                    anyhow::bail!("stop import loop after three iterations");
                                }
                                Ok(())
                            })
                        },
                        |_| Box::pin(async {}),
                    )
                });

                let maintenance_task = tokio::task::spawn_local({
                    let maintenance_started = Arc::clone(&maintenance_started);
                    let maintenance_block = Arc::clone(&maintenance_block);
                    run_maintenance_loop_with_hooks(
                        std::time::Duration::from_secs(300),
                        move || {
                            let maintenance_started = Arc::clone(&maintenance_started);
                            let maintenance_block = Arc::clone(&maintenance_block);
                            Box::pin(async move {
                                maintenance_started.notify_waiters();
                                maintenance_block.notified().await;
                                Ok(())
                            })
                        },
                        |_| Box::pin(async {}),
                    )
                });

                maintenance_started.notified().await;
                let import_err = import_task
                    .await
                    .expect("import task join")
                    .expect_err("import loop should stop after three iterations");
                assert!(import_err
                    .to_string()
                    .contains("stop import loop after three iterations"));
                assert!(
                    import_iterations.load(Ordering::SeqCst) >= 3,
                    "import loop should keep making progress while maintenance is blocked"
                );

                maintenance_task.abort();
            })
            .await;
    }
}

#[cfg(not(any(feature = "duckdb-runtime", feature = "duckdb-bundled")))]
fn main() -> anyhow::Result<()> {
    anyhow::bail!("llm-access-usage-worker requires duckdb-runtime")
}
