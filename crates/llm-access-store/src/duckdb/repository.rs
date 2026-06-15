//! `DuckDbUsageRepository` inherent impl + the `UsageEventSink` /
//! `UsageAnalyticsStore` trait impls.

use std::{
    collections::HashSet,
    fs,
    path::Path,
    sync::{Arc, Mutex, RwLock},
};

use anyhow::{anyhow, Context};
use async_trait::async_trait;
use llm_access_core::{
    store::{
        KiroLatencyRankingQuery, KiroLatencyRankingSnapshot, ProxyTrafficQuery,
        ProxyTrafficSnapshot, UsageAnalyticsStore, UsageChartPoint, UsageEventPage,
        UsageEventQuery, UsageEventSink, UsageFilterOptions, UsageMetricsQuery,
        UsageMetricsSnapshot,
    },
    usage::UsageEvent,
};
use tokio::task;

use super::{
    append::{
        append_usage_events_to_tiered, dedupe_usage_events_owned, key_usage_rollups_from_path,
        key_usage_rollups_from_tiered,
    },
    connection::{
        clear_stale_compacting_files, configure_duckdb_usage_connection,
        connection_config_snapshot, initialize_duckdb_target_path_with_connection_config,
    },
    filter_options::list_usage_filter_options_from_tiered,
    metrics::{
        kiro_latency_ranking_snapshot_from_path, kiro_latency_ranking_snapshot_from_tiered,
        usage_metrics_snapshot_from_path, usage_metrics_snapshot_from_tiered,
    },
    proxy_traffic::{proxy_traffic_snapshot_from_path, proxy_traffic_snapshot_from_tiered},
    query::{
        get_usage_event_from_path, get_usage_event_from_tiered, list_usage_events_from_path,
        list_usage_events_from_tiered, list_usage_filter_options_from_path,
        usage_chart_points_from_single_path, usage_chart_points_from_tiered,
    },
    retention::{ensure_single_writer, prune_tiered_usage_analytics},
    segment::{
        choose_active_segment, configure_duckdb_compact_connection,
        refresh_catalog_from_archives_if_needed, seed_catalog_from_archives_if_empty,
        spawn_existing_pending_sealers, test_catalog_state_path, tiered_compacting_dir,
    },
    tiered_pending_dir, DuckDbUsageConnectionConfig, DuckDbUsageRepository,
    DuckDbUsageRepositoryInner, SharedDuckDbUsageConnectionConfig, SingleDuckDbUsageState,
    TestTieredUsageCatalog, TieredDuckDbUsageConfig, TieredDuckDbUsageState,
    TieredUsageCatalogBackend, UsageAnalyticsPruneReport, UsageEventDetailStore, UsageEventRow,
};
use crate::{
    request_cache::RequestCacheConfig, usage_catalog::PostgresUsageCatalog, KeyUsageRollupSummary,
};

#[cfg(feature = "duckdb-runtime")]
impl DuckDbUsageRepository {
    /// Open a DuckDB usage repository and initialize the analytics schema.
    pub fn open_path(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        Self::open_path_with_connection_config(
            path,
            Arc::new(RwLock::new(DuckDbUsageConnectionConfig::default())),
        )
    }

    /// Open a DuckDB usage repository with runtime-tunable connection settings.
    pub fn open_path_with_connection_config(
        path: impl AsRef<Path>,
        connection_config: SharedDuckDbUsageConnectionConfig,
    ) -> anyhow::Result<Self> {
        let path = path.as_ref().to_path_buf();
        initialize_duckdb_target_path_with_connection_config(
            &path,
            connection_config_snapshot(&connection_config),
        )?;
        Ok(Self {
            inner: Arc::new(DuckDbUsageRepositoryInner::Single {
                state: Box::new(Mutex::new(SingleDuckDbUsageState {
                    path,
                    writer: None,
                })),
                connection_config,
            }),
        })
    }

    /// Open a tiered DuckDB usage repository.
    pub fn open_tiered(config: TieredDuckDbUsageConfig) -> anyhow::Result<Self> {
        Self::open_tiered_with_connection_config(
            config,
            Arc::new(RwLock::new(DuckDbUsageConnectionConfig::default())),
        )
    }

    /// Open a tiered DuckDB usage repository with runtime-tunable settings.
    pub fn open_tiered_with_connection_config(
        config: TieredDuckDbUsageConfig,
        connection_config: SharedDuckDbUsageConnectionConfig,
    ) -> anyhow::Result<Self> {
        let catalog_backend = Arc::new(TieredUsageCatalogBackend::Test(Arc::new(
            TestTieredUsageCatalog::open(test_catalog_state_path(&config))?,
        )));
        Self::open_tiered_with_catalog_backend(config, connection_config, catalog_backend)
    }

    /// Open a tiered DuckDB usage repository with a Postgres-backed archive
    /// catalog and optional Valkey read cache.
    pub fn open_tiered_with_postgres_catalog_with_connection_config(
        config: TieredDuckDbUsageConfig,
        connection_config: SharedDuckDbUsageConnectionConfig,
        database_url: &str,
        request_cache_config: Option<RequestCacheConfig>,
    ) -> anyhow::Result<Self> {
        let catalog_backend = Arc::new(TieredUsageCatalogBackend::Postgres(Arc::new(
            PostgresUsageCatalog::new(database_url, request_cache_config)?,
        )));
        Self::open_tiered_with_catalog_backend(config, connection_config, catalog_backend)
    }

    fn open_tiered_with_catalog_backend(
        config: TieredDuckDbUsageConfig,
        connection_config: SharedDuckDbUsageConnectionConfig,
        catalog_backend: Arc<TieredUsageCatalogBackend>,
    ) -> anyhow::Result<Self> {
        fs::create_dir_all(&config.active_dir).with_context(|| {
            format!("failed to create active duckdb directory `{}`", config.active_dir.display())
        })?;
        fs::create_dir_all(tiered_pending_dir(&config)).with_context(|| {
            format!(
                "failed to create pending duckdb directory `{}`",
                tiered_pending_dir(&config).display()
            )
        })?;
        fs::create_dir_all(tiered_compacting_dir(&config)).with_context(|| {
            format!(
                "failed to create compacting duckdb directory `{}`",
                tiered_compacting_dir(&config).display()
            )
        })?;
        fs::create_dir_all(&config.archive_dir).with_context(|| {
            format!("failed to create archive duckdb directory `{}`", config.archive_dir.display())
        })?;
        clear_stale_compacting_files(&config)?;
        let detail_store = config
            .details_dir
            .as_deref()
            .map(UsageEventDetailStore::from_dir)
            .transpose()?
            .flatten()
            .map(Arc::new);

        seed_catalog_from_archives_if_empty(catalog_backend.as_ref(), &config)?;
        refresh_catalog_from_archives_if_needed(catalog_backend.as_ref())?;
        spawn_existing_pending_sealers(
            config.clone(),
            Arc::clone(&catalog_backend),
            Arc::clone(&connection_config),
        )?;
        let (active_path, next_sequence) =
            choose_active_segment(&config, catalog_backend.as_ref())?;
        let active_has_rows = active_path.exists();
        initialize_duckdb_target_path_with_connection_config(
            &active_path,
            connection_config_snapshot(&connection_config),
        )?;
        Ok(Self {
            inner: Arc::new(DuckDbUsageRepositoryInner::Tiered {
                config,
                state: Box::new(Mutex::new(TieredDuckDbUsageState {
                    active_path,
                    next_sequence,
                    active_has_rows,
                    active_writer: None,
                    detail_store,
                    write_gate: Arc::new(tokio::sync::Mutex::new(())),
                    #[cfg(test)]
                    append_seam: None,
                })),
                connection_config,
                catalog_backend,
            }),
        })
    }

    /// Prune tiered usage analytics outside the retained day window.
    pub async fn prune_usage_analytics(
        &self,
        now_ms: i64,
        retention_days: u64,
    ) -> anyhow::Result<UsageAnalyticsPruneReport> {
        match self.inner.as_ref() {
            DuckDbUsageRepositoryInner::Single {
                ..
            } => Ok(UsageAnalyticsPruneReport::default()),
            DuckDbUsageRepositoryInner::Tiered {
                config,
                state,
                connection_config,
                catalog_backend,
            } => {
                prune_tiered_usage_analytics(
                    config,
                    state,
                    connection_config,
                    catalog_backend.as_ref(),
                    now_ms,
                    retention_days,
                )
                .await
            },
        }
    }

    pub(super) fn open_conn_with_connection_config(
        path: &Path,
        connection_config: DuckDbUsageConnectionConfig,
    ) -> anyhow::Result<duckdb::Connection> {
        let conn = Self::open_raw_conn(path)?;
        configure_duckdb_usage_connection(&conn, connection_config)?;
        Ok(conn)
    }

    pub(super) fn open_raw_conn(path: &Path) -> anyhow::Result<duckdb::Connection> {
        duckdb::Connection::open(path)
            .with_context(|| format!("failed to open duckdb database `{}`", path.display()))
    }

    pub(super) fn open_read_only_conn(path: &Path) -> anyhow::Result<duckdb::Connection> {
        let config = duckdb::Config::default()
            .access_mode(duckdb::AccessMode::ReadOnly)
            .context("failed to configure duckdb read-only access")?;
        let conn = duckdb::Connection::open_with_flags(path, config).with_context(|| {
            format!("failed to open read-only duckdb database `{}`", path.display())
        })?;
        configure_duckdb_usage_connection(&conn, DuckDbUsageConnectionConfig::default())?;
        Ok(conn)
    }

    pub(super) fn open_checkpoint_conn(
        path: &Path,
        connection_config: DuckDbUsageConnectionConfig,
    ) -> anyhow::Result<duckdb::Connection> {
        let conn = Self::open_raw_conn(path)?;
        let temp_dir = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("checkpointing");
        configure_duckdb_compact_connection(&conn, &temp_dir, connection_config)?;
        Ok(conn)
    }

    /// Aggregate all persisted usage events into per-key operational rollups.
    pub async fn key_usage_rollups(&self) -> anyhow::Result<Vec<KeyUsageRollupSummary>> {
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || match inner.as_ref() {
            DuckDbUsageRepositoryInner::Single {
                state, ..
            } => {
                let path = {
                    let state = state
                        .lock()
                        .map_err(|_| anyhow!("single duckdb state lock poisoned"))?;
                    state.path.clone()
                };
                key_usage_rollups_from_path(&path)
            },
            DuckDbUsageRepositoryInner::Tiered {
                config,
                state,
                catalog_backend,
                ..
            } => key_usage_rollups_from_tiered(config, state, catalog_backend.as_ref()),
        })
        .await
        .context("duckdb key usage rollup task failed")?
    }

    /// Append a batch after removing only in-memory duplicates from the same
    /// call.
    pub async fn append_usage_events_if_new(&self, events: &[UsageEvent]) -> anyhow::Result<usize> {
        let deduped = dedupe_usage_events_owned(events.to_vec());
        let len = deduped.len();
        if len == 0 {
            return Ok(0);
        }
        UsageEventSink::append_usage_events_owned(self, deduped).await?;
        Ok(len)
    }

    /// Append already-enriched fact rows after removing only in-memory
    /// duplicates from the same call.
    pub async fn append_usage_event_rows_owned(
        &self,
        rows: Vec<UsageEventRow>,
    ) -> anyhow::Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let inner = Arc::clone(&self.inner);
        let mut seen = HashSet::new();
        let deduped = rows
            .into_iter()
            .filter(|row| seen.insert(row.event_id.clone()))
            .collect::<Vec<_>>();
        if deduped.is_empty() {
            return Ok(());
        }
        match inner.as_ref() {
            DuckDbUsageRepositoryInner::Single {
                ..
            } => {
                let inner = Arc::clone(&inner);
                task::spawn_blocking(move || match inner.as_ref() {
                    DuckDbUsageRepositoryInner::Single {
                        state,
                        connection_config,
                    } => {
                        let mut state = state
                            .lock()
                            .map_err(|_| anyhow!("single duckdb state lock poisoned"))?;
                        let writer = ensure_single_writer(
                            &mut state,
                            connection_config_snapshot(connection_config),
                        )?;
                        writer
                            .writer
                            .summary
                            .insert_usage_events(&deduped)
                            .map(|_| ())
                    },
                    _ => unreachable!("single branch expected"),
                })
                .await
                .context("duckdb usage row insert task failed")?
            },
            DuckDbUsageRepositoryInner::Tiered {
                config,
                state,
                catalog_backend,
                connection_config,
            } => {
                append_usage_events_to_tiered(
                    config,
                    state,
                    connection_config,
                    catalog_backend,
                    &deduped,
                )
                .await
            },
        }
    }
}
#[cfg(feature = "duckdb-runtime")]
#[async_trait]
impl UsageEventSink for DuckDbUsageRepository {
    async fn append_usage_events(&self, events: &[UsageEvent]) -> anyhow::Result<()> {
        self.append_usage_events_owned(events.to_vec()).await
    }

    async fn append_usage_events_owned(&self, events: Vec<UsageEvent>) -> anyhow::Result<()> {
        if events.is_empty() {
            return Ok(());
        }
        let deduped = dedupe_usage_events_owned(events);
        if deduped.is_empty() {
            return Ok(());
        }
        let rows = deduped
            .iter()
            .map(UsageEventRow::from_usage_event)
            .collect::<Vec<_>>();
        self.append_usage_event_rows_owned(rows).await
    }
}
#[cfg(feature = "duckdb-runtime")]
#[async_trait]
impl UsageAnalyticsStore for DuckDbUsageRepository {
    async fn list_usage_events(&self, query: UsageEventQuery) -> anyhow::Result<UsageEventPage> {
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || match inner.as_ref() {
            DuckDbUsageRepositoryInner::Single {
                state, ..
            } => {
                let path = {
                    let state = state
                        .lock()
                        .map_err(|_| anyhow!("single duckdb state lock poisoned"))?;
                    state.path.clone()
                };
                list_usage_events_from_path(&path, &query)
            },
            DuckDbUsageRepositoryInner::Tiered {
                config,
                state,
                catalog_backend,
                ..
            } => list_usage_events_from_tiered(config, state, catalog_backend.as_ref(), &query),
        })
        .await
        .context("duckdb usage event list task failed")?
    }

    async fn get_usage_event(&self, event_id: &str) -> anyhow::Result<Option<UsageEvent>> {
        let inner = Arc::clone(&self.inner);
        let event_id = event_id.to_string();
        match inner.as_ref() {
            DuckDbUsageRepositoryInner::Single {
                ..
            } => {
                let inner = Arc::clone(&inner);
                task::spawn_blocking(move || match inner.as_ref() {
                    DuckDbUsageRepositoryInner::Single {
                        state, ..
                    } => {
                        let path = {
                            let state = state
                                .lock()
                                .map_err(|_| anyhow!("single duckdb state lock poisoned"))?;
                            state.path.clone()
                        };
                        get_usage_event_from_path(&path, &event_id)
                    },
                    _ => unreachable!("single branch expected"),
                })
                .await
                .context("duckdb usage event detail task failed")?
            },
            DuckDbUsageRepositoryInner::Tiered {
                config,
                state,
                catalog_backend,
                ..
            } => {
                get_usage_event_from_tiered(config, state, catalog_backend.as_ref(), &event_id)
                    .await
            },
        }
    }

    async fn usage_chart_points(
        &self,
        key_id: &str,
        start_ms: i64,
        bucket_ms: i64,
        bucket_count: usize,
    ) -> anyhow::Result<Vec<UsageChartPoint>> {
        let inner = Arc::clone(&self.inner);
        let key_id = key_id.to_string();
        task::spawn_blocking(move || match inner.as_ref() {
            DuckDbUsageRepositoryInner::Single {
                state, ..
            } => {
                let path = {
                    let state = state
                        .lock()
                        .map_err(|_| anyhow!("single duckdb state lock poisoned"))?;
                    state.path.clone()
                };
                usage_chart_points_from_single_path(
                    &path,
                    &key_id,
                    start_ms,
                    bucket_ms,
                    bucket_count,
                )
            },
            DuckDbUsageRepositoryInner::Tiered {
                config,
                state,
                catalog_backend,
                ..
            } => usage_chart_points_from_tiered(
                config,
                state,
                catalog_backend.as_ref(),
                &key_id,
                start_ms,
                bucket_ms,
                bucket_count,
            ),
        })
        .await
        .context("duckdb usage chart task failed")?
    }

    async fn list_usage_filter_options(
        &self,
        query: UsageEventQuery,
    ) -> anyhow::Result<UsageFilterOptions> {
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || match inner.as_ref() {
            DuckDbUsageRepositoryInner::Single {
                state, ..
            } => {
                let path = {
                    let state = state
                        .lock()
                        .map_err(|_| anyhow!("single duckdb state lock poisoned"))?;
                    state.path.clone()
                };
                list_usage_filter_options_from_path(&path, &query)
            },
            DuckDbUsageRepositoryInner::Tiered {
                config,
                state,
                catalog_backend,
                ..
            } => {
                let active_path = {
                    let state = state
                        .lock()
                        .map_err(|_| anyhow!("tiered duckdb state lock poisoned"))?;
                    state.active_path.clone()
                };
                list_usage_filter_options_from_tiered(
                    config,
                    catalog_backend.as_ref(),
                    &active_path,
                    &query,
                )
            },
        })
        .await
        .context("duckdb usage filter options task failed")?
    }

    async fn usage_metrics_snapshot(
        &self,
        query: UsageMetricsQuery,
    ) -> anyhow::Result<UsageMetricsSnapshot> {
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || match inner.as_ref() {
            DuckDbUsageRepositoryInner::Single {
                state, ..
            } => {
                let path = {
                    let state = state
                        .lock()
                        .map_err(|_| anyhow!("single duckdb state lock poisoned"))?;
                    state.path.clone()
                };
                usage_metrics_snapshot_from_path(&path, &query)
            },
            DuckDbUsageRepositoryInner::Tiered {
                state,
                catalog_backend,
                ..
            } => usage_metrics_snapshot_from_tiered(state, catalog_backend.as_ref(), &query),
        })
        .await
        .context("duckdb usage metrics task failed")?
    }

    async fn proxy_traffic_snapshot(
        &self,
        query: ProxyTrafficQuery,
    ) -> anyhow::Result<ProxyTrafficSnapshot> {
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || match inner.as_ref() {
            DuckDbUsageRepositoryInner::Single {
                state, ..
            } => {
                let path = {
                    let state = state
                        .lock()
                        .map_err(|_| anyhow!("single duckdb state lock poisoned"))?;
                    state.path.clone()
                };
                proxy_traffic_snapshot_from_path(&path, &query)
            },
            DuckDbUsageRepositoryInner::Tiered {
                state,
                catalog_backend,
                ..
            } => proxy_traffic_snapshot_from_tiered(state, catalog_backend.as_ref(), &query),
        })
        .await
        .context("duckdb proxy traffic task failed")?
    }

    async fn kiro_latency_ranking_snapshot(
        &self,
        query: KiroLatencyRankingQuery,
    ) -> anyhow::Result<KiroLatencyRankingSnapshot> {
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || match inner.as_ref() {
            DuckDbUsageRepositoryInner::Single {
                state, ..
            } => {
                let path = {
                    let state = state
                        .lock()
                        .map_err(|_| anyhow!("single duckdb state lock poisoned"))?;
                    state.path.clone()
                };
                kiro_latency_ranking_snapshot_from_path(&path, &query)
            },
            DuckDbUsageRepositoryInner::Tiered {
                state,
                catalog_backend,
                ..
            } => kiro_latency_ranking_snapshot_from_tiered(state, catalog_backend.as_ref(), &query),
        })
        .await
        .context("duckdb kiro latency ranking task failed")?
    }
}
