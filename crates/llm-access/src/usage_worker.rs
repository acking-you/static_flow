//! Journal consumer worker for llm-access usage analytics.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Context};
use axum::{
    body::{Body, Bytes},
    extract::{OriginalUri, State},
    http::{header, HeaderMap, Response as HttpResponse, StatusCode, Uri},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use llm_access_core::{
    store::{UsageEventSink, DEFAULT_USAGE_ANALYTICS_RETENTION_DAYS},
    usage::UsageEvent,
};
use llm_access_store::{
    duckdb::{DuckDbUsageRepository, UsageEventRow},
    postgres::{PostgresControlRepository, UsageProxyAttribution},
};
use llm_usage_journal::{JournalConsumerState, JournalReader, WorkerProgressSnapshot};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    process_memory::{read_current_process_memory_stats, ProcessMemoryStats},
    usage_query::{
        get_kiro_usage_event, get_llm_usage_event, kiro_latency_ranking_snapshot,
        list_kiro_usage_events, list_llm_usage_events, usage_chart_points, usage_filter_options,
        usage_metrics_snapshot, UsageQueryState,
    },
};

const WORKER_PROGRESS_UPDATE_INTERVAL_MS: i64 = 1_000;
const RELAY_HEADER_SOURCE_NODE_ID: &str = "x-llm-access-source-node-id";
const RELAY_HEADER_FILE_SEQUENCE: &str = "x-llm-access-file-sequence";
const USAGE_SOURCE_HEADER: &str = "x-llm-access-usage-source";
const WORKER_ROLE_HEADER: &str = "x-llm-access-worker-role";
const PRIMARY_NODE_HEADER: &str = "x-llm-access-primary-node-id";
const CURRENT_NODE_HEADER: &str = "x-llm-access-node-id";

/// Clustered usage worker mode.
pub enum ClusterUsageWorker {
    /// Primary worker that owns local DuckDB/JuiceFS writes.
    Primary(UsageWorker),
    /// Edge worker that proxies usage queries and relays sealed journals.
    Edge(EdgeUsageWorker),
}

/// Edge-secondary usage worker.
pub struct EdgeUsageWorker {
    journal_root: PathBuf,
    state: JournalConsumerState,
    consumer_lease_ms: u64,
    cluster_state: Arc<crate::cluster::ClusterRuntimeState>,
    source_node_id: String,
    http_client: reqwest::Client,
}

/// Usage journal consumer.
pub struct UsageWorker {
    journal_root: PathBuf,
    state: JournalConsumerState,
    duckdb_usage: Arc<DuckDbUsageRepository>,
    attribution_resolver: Option<Arc<UsageEventAttributionResolver>>,
    consumer_lease_ms: u64,
    usage_analytics_retention_days: Arc<RwLock<u64>>,
    cluster_state: Option<Arc<crate::cluster::ClusterRuntimeState>>,
}

/// Resolve per-account proxy attribution while consuming usage events.
#[derive(Clone)]
pub struct UsageEventAttributionResolver {
    control: Arc<PostgresControlRepository>,
}

impl UsageEventAttributionResolver {
    /// Build one resolver backed by the shared Postgres control repository.
    pub fn new(control: Arc<PostgresControlRepository>) -> Self {
        Self {
            control,
        }
    }

    /// Convert freshly decoded usage events into persisted rows with proxy
    /// metadata.
    pub async fn build_usage_rows(
        &self,
        events: Vec<UsageEvent>,
    ) -> anyhow::Result<Vec<UsageEventRow>> {
        if events.is_empty() {
            return Ok(Vec::new());
        }
        let mut attribution_by_account =
            BTreeMap::<(String, String), Option<UsageProxyAttribution>>::new();
        for event in &events {
            let Some(account_name) = event
                .account_name
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                continue;
            };
            attribution_by_account
                .entry((event.provider_type.as_storage_str().to_string(), account_name.to_string()))
                .or_insert(None);
        }
        for ((provider_type, account_name), slot) in &mut attribution_by_account {
            *slot = self
                .control
                .resolve_usage_proxy_attribution(provider_type, account_name)
                .await?;
        }
        Ok(events
            .into_iter()
            .map(|event| {
                let attribution = event
                    .account_name
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .and_then(|account_name| {
                        attribution_by_account
                            .get(&(
                                event.provider_type.as_storage_str().to_string(),
                                account_name.to_string(),
                            ))
                            .and_then(|value| value.as_ref())
                    });
                UsageEventRow::from_usage_event(&event).with_proxy_attribution(attribution)
            })
            .collect())
    }
}

/// Build the usage worker HTTP router.
pub fn router(worker: &ClusterUsageWorker) -> Router {
    match worker {
        ClusterUsageWorker::Primary(worker) => primary_worker_router(worker),
        ClusterUsageWorker::Edge(worker) => edge_worker_router(worker),
    }
}

#[derive(Clone)]
struct PrimaryWorkerHttpState {
    journal_root: PathBuf,
    query: UsageQueryState,
    duckdb_usage: Arc<DuckDbUsageRepository>,
    attribution_resolver: Option<Arc<UsageEventAttributionResolver>>,
    cluster_state: Option<Arc<crate::cluster::ClusterRuntimeState>>,
}

#[derive(Clone)]
struct EdgeWorkerHttpState {
    journal_root: PathBuf,
    cluster_state: Arc<crate::cluster::ClusterRuntimeState>,
    http_client: reqwest::Client,
}

impl axum::extract::FromRef<PrimaryWorkerHttpState> for UsageQueryState {
    fn from_ref(input: &PrimaryWorkerHttpState) -> Self {
        input.query.clone()
    }
}

#[derive(serde::Serialize)]
struct WorkerStatusResponse {
    #[serde(flatten)]
    progress: WorkerProgressSnapshot,
    process_memory: ProcessMemoryStats,
    #[serde(skip_serializing_if = "Option::is_none")]
    cluster: Option<WorkerClusterStatusView>,
}

#[derive(Clone, serde::Serialize)]
struct WorkerClusterStatusView {
    node_id: String,
    runtime_role: crate::cluster::NodeRuntimeRole,
    usage_query_mode: crate::cluster::UsageQueryMode,
    primary_node_id: Option<String>,
    primary_worker_base_url: Option<String>,
}

fn primary_worker_router(worker: &UsageWorker) -> Router {
    let query_state = UsageQueryState {
        usage_analytics_store: worker.duckdb_usage.clone(),
        retention_days: worker.usage_analytics_retention_days.clone(),
    };
    Router::new()
        .route("/admin/llm-gateway/usage", get(list_llm_usage_events))
        .route("/admin/llm-gateway/usage/:event_id", get(get_llm_usage_event))
        .route("/admin/kiro-gateway/usage", get(list_kiro_usage_events))
        .route("/admin/kiro-gateway/usage/:event_id", get(get_kiro_usage_event))
        .route("/admin/llm-access/usage/chart", get(usage_chart_points))
        .route("/admin/llm-gateway/usage/filter-options", get(usage_filter_options))
        .route("/admin/llm-gateway/usage/metrics", get(usage_metrics_snapshot))
        .route("/internal/kiro-gateway/latency-ranking", get(kiro_latency_ranking_snapshot))
        .route("/admin/llm-access/usage-worker/status", get(primary_worker_status))
        .route("/internal/usage-journal/import", post(primary_import_relay_file))
        .with_state(PrimaryWorkerHttpState {
            journal_root: worker.journal_root.clone(),
            query: query_state,
            duckdb_usage: worker.duckdb_usage.clone(),
            attribution_resolver: worker.attribution_resolver.clone(),
            cluster_state: worker.cluster_state.clone(),
        })
}

fn edge_worker_router(worker: &EdgeUsageWorker) -> Router {
    Router::new()
        .route("/admin/llm-gateway/usage", get(edge_proxy_usage_query))
        .route("/admin/llm-gateway/usage/:event_id", get(edge_proxy_usage_query))
        .route("/admin/kiro-gateway/usage", get(edge_proxy_usage_query))
        .route("/admin/kiro-gateway/usage/:event_id", get(edge_proxy_usage_query))
        .route("/admin/llm-access/usage/chart", get(edge_proxy_usage_query))
        .route("/admin/llm-gateway/usage/filter-options", get(edge_proxy_usage_query))
        .route("/admin/llm-gateway/usage/metrics", get(edge_proxy_usage_query))
        .route("/internal/kiro-gateway/latency-ranking", get(edge_proxy_usage_query))
        .route("/admin/llm-access/usage-worker/status", get(edge_worker_status))
        .route("/internal/usage-journal/import", post(edge_reject_import_relay_file))
        .with_state(EdgeWorkerHttpState {
            journal_root: worker.journal_root.clone(),
            cluster_state: worker.cluster_state.clone(),
            http_client: worker.http_client.clone(),
        })
}

async fn primary_worker_status(State(state): State<PrimaryWorkerHttpState>) -> impl IntoResponse {
    build_worker_status_response(&state.journal_root, state.cluster_state.as_ref()).await
}

async fn edge_worker_status(State(state): State<EdgeWorkerHttpState>) -> impl IntoResponse {
    build_worker_status_response(&state.journal_root, Some(&state.cluster_state)).await
}

async fn build_worker_status_response(
    journal_root: &Path,
    cluster_state: Option<&Arc<crate::cluster::ClusterRuntimeState>>,
) -> HttpResponse<Body> {
    match JournalConsumerState::open(journal_root).and_then(|state| state.progress_snapshot()) {
        Ok(progress) => Json(WorkerStatusResponse {
            progress,
            process_memory: read_current_process_memory_stats(),
            cluster: match cluster_state {
                Some(cluster_state) => Some(worker_cluster_status_view(cluster_state).await),
                None => None,
            },
        })
        .into_response(),
        Err(err) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to load worker status: {err:#}"),
        )
            .into_response(),
    }
}

async fn worker_cluster_status_view(
    cluster_state: &Arc<crate::cluster::ClusterRuntimeState>,
) -> WorkerClusterStatusView {
    let snapshot = cluster_state.snapshot().await;
    WorkerClusterStatusView {
        node_id: snapshot.node.node_id,
        runtime_role: snapshot.runtime_role,
        usage_query_mode: snapshot.usage_query_mode,
        primary_node_id: snapshot
            .primary
            .as_ref()
            .map(|primary| primary.node_id.clone()),
        primary_worker_base_url: snapshot
            .primary
            .as_ref()
            .and_then(|primary| primary.worker_base_url.clone()),
    }
}

async fn edge_proxy_usage_query(
    State(state): State<EdgeWorkerHttpState>,
    uri: OriginalUri,
) -> HttpResponse<Body> {
    proxy_worker_usage_query(&state.http_client, &state.cluster_state, &uri.0).await
}

async fn edge_reject_import_relay_file() -> HttpResponse<Body> {
    (StatusCode::SERVICE_UNAVAILABLE, "edge usage worker cannot ingest relayed journals")
        .into_response()
}

async fn proxy_worker_usage_query(
    http_client: &reqwest::Client,
    cluster_state: &Arc<crate::cluster::ClusterRuntimeState>,
    uri: &Uri,
) -> HttpResponse<Body> {
    let snapshot = cluster_state.snapshot().await;
    let Some(base_url) = snapshot
        .primary
        .as_ref()
        .and_then(|primary| primary.worker_base_url.clone())
    else {
        return (StatusCode::SERVICE_UNAVAILABLE, "primary usage worker is unavailable")
            .into_response();
    };
    let path_and_query = uri
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or(uri.path());
    let url = format!("{}{}", base_url.trim_end_matches('/'), path_and_query);
    let response = match http_client.get(&url).send().await {
        Ok(response) => response,
        Err(err) => {
            tracing::warn!(url = %url, "edge usage worker proxy failed: {err:#}");
            return (StatusCode::SERVICE_UNAVAILABLE, "primary usage worker request failed")
                .into_response();
        },
    };
    let status = StatusCode::from_u16(response.status().as_u16())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let content_type = response.headers().get(header::CONTENT_TYPE).cloned();
    let bytes = match response.bytes().await {
        Ok(bytes) => bytes,
        Err(err) => {
            tracing::warn!(url = %url, "failed to read proxied usage worker response: {err:#}");
            return (StatusCode::BAD_GATEWAY, "failed to read primary usage worker response")
                .into_response();
        },
    };
    let mut builder = HttpResponse::builder().status(status);
    if let Some(content_type) = content_type {
        builder = builder.header(header::CONTENT_TYPE, content_type);
    }
    builder = builder
        .header(USAGE_SOURCE_HEADER, "proxied_primary")
        .header(WORKER_ROLE_HEADER, "edge_secondary")
        .header(CURRENT_NODE_HEADER, snapshot.node.node_id.as_str());
    if let Some(primary_node_id) = snapshot
        .primary
        .as_ref()
        .map(|primary| primary.node_id.as_str())
    {
        builder = builder.header(PRIMARY_NODE_HEADER, primary_node_id);
    }
    builder.body(Body::from(bytes)).unwrap_or_else(|_| {
        (StatusCode::INTERNAL_SERVER_ERROR, "failed to build proxied response").into_response()
    })
}

#[derive(serde::Serialize)]
struct RelayImportResponse {
    imported: bool,
    already_imported: bool,
    source_node_id: String,
    file_sequence: u64,
    event_count: u64,
}

async fn primary_import_relay_file(
    State(state): State<PrimaryWorkerHttpState>,
    headers: HeaderMap,
    body: Bytes,
) -> HttpResponse<Body> {
    let Some(source_node_id) = headers
        .get(RELAY_HEADER_SOURCE_NODE_ID)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return (StatusCode::BAD_REQUEST, "missing source node id").into_response();
    };
    let Some(file_sequence) = headers
        .get(RELAY_HEADER_FILE_SEQUENCE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
    else {
        return (StatusCode::BAD_REQUEST, "missing file sequence").into_response();
    };
    match import_relayed_journal_bytes(
        &state.journal_root,
        Arc::clone(&state.duckdb_usage),
        state.attribution_resolver.clone(),
        source_node_id,
        file_sequence,
        body,
    )
    .await
    {
        Ok(response) => Json(response).into_response(),
        Err(err) => {
            tracing::warn!(
                source_node_id,
                file_sequence,
                "failed to import relayed journal file: {err:#}"
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to import relayed journal: {err:#}"),
            )
                .into_response()
        },
    }
}

impl ClusterUsageWorker {
    /// Run one cluster-aware worker cycle.
    pub async fn run_one_cycle(&self) -> anyhow::Result<bool> {
        match self {
            Self::Primary(worker) => worker.run_one_import().await,
            Self::Edge(worker) => worker.run_one_relay().await,
        }
    }

    /// Persist a terminal worker error in the progress state.
    pub fn record_error(&self, err: &anyhow::Error) {
        match self {
            Self::Primary(worker) => worker.record_error(err),
            Self::Edge(worker) => worker.record_error(err),
        }
    }
}

impl EdgeUsageWorker {
    /// Create an edge-secondary relay worker.
    pub fn new(
        journal_root: PathBuf,
        cluster_state: Arc<crate::cluster::ClusterRuntimeState>,
        source_node_id: String,
        consumer_lease_ms: u64,
    ) -> anyhow::Result<Self> {
        for subdir in ["sealed", "consuming", "bad"] {
            fs::create_dir_all(journal_root.join(subdir)).with_context(|| {
                format!("failed to create journal dir `{}`", journal_root.join(subdir).display())
            })?;
        }
        Ok(Self {
            state: JournalConsumerState::open(&journal_root)?,
            journal_root,
            consumer_lease_ms: consumer_lease_ms.max(1),
            cluster_state,
            source_node_id,
            http_client: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .context("build edge usage relay http client")?,
        })
    }

    /// Return current relay worker progress.
    pub fn progress_snapshot(&self) -> anyhow::Result<WorkerProgressSnapshot> {
        self.state.progress_snapshot()
    }

    /// Persist a terminal worker error in the progress state.
    pub fn record_error(&self, err: &anyhow::Error) {
        let mut progress = self.progress_snapshot().unwrap_or_else(|_| idle_progress());
        progress.state = "error".to_string();
        progress.heartbeat_at_ms = Some(now_ms());
        progress.last_error = Some(format!("{err:#}"));
        progress.last_error_at_ms = Some(now_ms());
        if let Err(update_err) = self.state.update_progress(&progress, now_ms()) {
            tracing::error!("failed to persist edge usage worker error status: {update_err:#}");
        }
    }

    /// Relay one sealed journal file if one is available.
    pub async fn run_one_relay(&self) -> anyhow::Result<bool> {
        recover_orphan_consuming_files(&self.journal_root, &self.state, self.consumer_lease_ms)?;
        let Some(claim) = claim_oldest_sealed_file(&self.journal_root, &self.state)? else {
            self.state.update_progress(&idle_progress(), now_ms())?;
            return Ok(false);
        };
        self.state
            .update_progress(&relaying_progress(&claim), now_ms())?;
        let bytes = fs::read(&claim.path).with_context(|| {
            format!("failed to read relayed journal `{}`", claim.path.display())
        })?;
        let snapshot = self.cluster_state.snapshot().await;
        let Some(base_url) = snapshot
            .primary
            .as_ref()
            .and_then(|primary| primary.worker_base_url.clone())
        else {
            restore_claimed_file(&self.journal_root, &claim)?;
            anyhow::bail!("primary usage worker is unavailable")
        };
        let response = self
            .http_client
            .post(format!("{}/internal/usage-journal/import", base_url.trim_end_matches('/')))
            .header(RELAY_HEADER_SOURCE_NODE_ID, self.source_node_id.as_str())
            .header(RELAY_HEADER_FILE_SEQUENCE, claim.sequence.to_string())
            .body(bytes)
            .send()
            .await
            .context("send relayed journal file to primary usage worker")?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            restore_claimed_file(&self.journal_root, &claim)?;
            anyhow::bail!("primary usage worker rejected relay with {}: {}", status, body);
        }
        fs::remove_file(&claim.path).with_context(|| {
            format!(
                "failed to delete relayed journal after successful import `{}`",
                claim.path.display()
            )
        })?;
        let mut progress = idle_progress();
        progress.last_successful_file_sequence = Some(claim.sequence);
        progress.last_successful_import_at_ms = Some(now_ms());
        self.state.update_progress(&progress, now_ms())?;
        Ok(true)
    }
}

impl UsageWorker {
    /// Create a worker for one journal root and one analytics repository.
    pub fn new(
        journal_root: PathBuf,
        duckdb_usage: Arc<DuckDbUsageRepository>,
        consumer_lease_ms: u64,
    ) -> anyhow::Result<Self> {
        Self::new_with_retention_days(
            journal_root,
            duckdb_usage,
            consumer_lease_ms,
            DEFAULT_USAGE_ANALYTICS_RETENTION_DAYS,
        )
    }

    /// Create a worker with an explicit analytics retention horizon.
    pub fn new_with_retention_days(
        journal_root: PathBuf,
        duckdb_usage: Arc<DuckDbUsageRepository>,
        consumer_lease_ms: u64,
        usage_analytics_retention_days: u64,
    ) -> anyhow::Result<Self> {
        for subdir in ["sealed", "consuming", "bad"] {
            fs::create_dir_all(journal_root.join(subdir)).with_context(|| {
                format!("failed to create journal dir `{}`", journal_root.join(subdir).display())
            })?;
        }
        let state = JournalConsumerState::open(&journal_root)?;
        Ok(Self {
            journal_root,
            state,
            duckdb_usage,
            attribution_resolver: None,
            consumer_lease_ms: consumer_lease_ms.max(1),
            usage_analytics_retention_days: Arc::new(RwLock::new(
                usage_analytics_retention_days.max(1),
            )),
            cluster_state: None,
        })
    }

    /// Attach optional cluster state used by status surfaces.
    pub fn with_cluster_state(
        mut self,
        cluster_state: Option<Arc<crate::cluster::ClusterRuntimeState>>,
    ) -> Self {
        self.cluster_state = cluster_state;
        self
    }

    /// Attach an optional consumption-time proxy-attribution resolver.
    pub fn with_attribution_resolver(
        mut self,
        attribution_resolver: Option<Arc<UsageEventAttributionResolver>>,
    ) -> Self {
        self.attribution_resolver = attribution_resolver;
        self
    }

    /// Update the retention horizon used by query responses and maintenance.
    pub fn set_usage_analytics_retention_days(&self, days: u64) {
        if let Ok(mut current) = self.usage_analytics_retention_days.write() {
            *current = days.max(1);
        }
    }

    /// Return the current retained usage analytics horizon.
    pub fn usage_analytics_retention_days(&self) -> u64 {
        self.usage_analytics_retention_days
            .read()
            .map(|value| *value)
            .unwrap_or(DEFAULT_USAGE_ANALYTICS_RETENTION_DAYS)
            .max(1)
    }

    /// Return the shared retention-days handle used by the worker.
    pub fn retention_days_handle(&self) -> Arc<RwLock<u64>> {
        Arc::clone(&self.usage_analytics_retention_days)
    }

    /// Return the shared DuckDB usage repository used by the worker.
    pub fn usage_repository(&self) -> Arc<DuckDbUsageRepository> {
        Arc::clone(&self.duckdb_usage)
    }

    async fn append_events(&self, events: Vec<UsageEvent>) -> anyhow::Result<()> {
        if let Some(resolver) = &self.attribution_resolver {
            let rows = resolver.build_usage_rows(events).await?;
            return self.duckdb_usage.append_usage_event_rows_owned(rows).await;
        }
        self.duckdb_usage.append_usage_events_owned(events).await
    }

    /// Run storage maintenance for the current retention horizon.
    pub async fn run_maintenance(&self, now_ms: i64) -> anyhow::Result<()> {
        let report = self
            .duckdb_usage
            .prune_usage_analytics(now_ms, self.usage_analytics_retention_days())
            .await?;
        if report.deleted_segments > 0
            || report.deleted_files > 0
            || report.deleted_orphan_files > 0
            || report.deleted_detail_files > 0
            || report.deleted_detail_dirs > 0
        {
            tracing::info!(
                deleted_segments = report.deleted_segments,
                deleted_files = report.deleted_files,
                deleted_orphan_files = report.deleted_orphan_files,
                deleted_detail_files = report.deleted_detail_files,
                deleted_detail_dirs = report.deleted_detail_dirs,
                retention_days = self.usage_analytics_retention_days(),
                "pruned llm access usage analytics"
            );
        }
        Ok(())
    }

    /// Import one sealed journal file if one is available.
    pub async fn run_one_import(&self) -> anyhow::Result<bool> {
        self.import_next(None, true).await
    }

    #[cfg(test)]
    async fn run_until_first_block_commit(&self) -> anyhow::Result<bool> {
        self.import_next(Some(1), false).await
    }

    /// Return current worker progress.
    pub fn progress_snapshot(&self) -> anyhow::Result<WorkerProgressSnapshot> {
        self.state.progress_snapshot()
    }

    /// Persist a terminal worker error in the progress state.
    pub fn record_error(&self, err: &anyhow::Error) {
        let mut progress = self.progress_snapshot().unwrap_or_else(|_| idle_progress());
        progress.state = "error".to_string();
        progress.heartbeat_at_ms = Some(now_ms());
        progress.last_error = Some(format!("{err:#}"));
        progress.last_error_at_ms = Some(now_ms());
        if let Err(update_err) = self.state.update_progress(&progress, now_ms()) {
            tracing::error!("failed to persist usage worker error status: {update_err:#}");
        }
    }

    async fn import_next(
        &self,
        max_blocks: Option<usize>,
        finalize_file: bool,
    ) -> anyhow::Result<bool> {
        recover_orphan_consuming_files(&self.journal_root, &self.state, self.consumer_lease_ms)?;
        let Some(claim) = claim_oldest_sealed_file(&self.journal_root, &self.state)? else {
            self.state.update_progress(&idle_progress(), now_ms())?;
            return Ok(false);
        };
        self.import_claimed_file(claim, max_blocks, finalize_file)
            .await
            .map(|()| true)
    }

    async fn import_claimed_file(
        &self,
        claim: ClaimedJournalFile,
        max_blocks: Option<usize>,
        finalize_file: bool,
    ) -> anyhow::Result<()> {
        let reader = JournalReader::open(&claim.path)?;
        let block_limit = max_blocks.unwrap_or(usize::MAX);
        let mut stream = reader.stream_batches()?;
        let total_compressed_bytes = stream.total_compressed_bytes();
        let mut processed_blocks = 0u64;
        let mut processed_events = 0u64;
        let mut last_progress_update_ms = None;

        for index in 0..block_limit {
            let Some(batch) = stream.next_batch()? else {
                break;
            };
            processed_blocks = processed_blocks.saturating_add(1);
            let events = batch
                .events
                .into_iter()
                .map(|event| event.into_usage_event())
                .collect::<Vec<_>>();
            let event_count = events.len() as u64;
            self.append_events(events).await?;
            processed_events = processed_events.saturating_add(event_count);
            let now = now_ms();
            let should_persist_progress = processed_blocks == 1
                || last_progress_update_ms
                    .map(|last| now.saturating_sub(last) >= WORKER_PROGRESS_UPDATE_INTERVAL_MS)
                    .unwrap_or(true)
                || index + 1 == block_limit;
            if should_persist_progress {
                let progress = importing_progress(
                    &claim,
                    processed_blocks,
                    processed_events,
                    stream.bytes_read(),
                    total_compressed_bytes,
                );
                self.state.update_progress(&progress, now)?;
                last_progress_update_ms = Some(now);
            }
        }

        if max_blocks.is_some() {
            return Ok(());
        }
        let report = stream.finish()?;

        self.state.record_consumed_file(
            claim.sequence,
            &report.file_digest_hex,
            report.footer.event_count,
            now_ms(),
        )?;
        if finalize_file {
            fs::remove_file(&claim.path).with_context(|| {
                format!("failed to delete consumed journal `{}`", claim.path.display())
            })?;
        }
        let mut progress = idle_progress();
        progress.last_successful_file_sequence = Some(claim.sequence);
        progress.last_successful_import_at_ms = Some(now_ms());
        self.state.update_progress(&progress, now_ms())?;
        Ok(())
    }
}

struct ClaimedJournalFile {
    sequence: u64,
    path: PathBuf,
}

fn claim_oldest_sealed_file(
    journal_root: &Path,
    state: &JournalConsumerState,
) -> anyhow::Result<Option<ClaimedJournalFile>> {
    let sealed_dir = journal_root.join("sealed");
    if !sealed_dir.exists() {
        return Ok(None);
    }
    let mut candidates = Vec::new();
    for entry in fs::read_dir(&sealed_dir)
        .with_context(|| format!("failed to read sealed journal dir `{}`", sealed_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(sequence) = parse_journal_sequence(&path) else {
            continue;
        };
        candidates.push((sequence, path));
    }
    candidates.sort_by_key(|(sequence, _path)| *sequence);
    for (sequence, sealed_path) in candidates {
        if state.is_consumed(sequence)? {
            fs::remove_file(&sealed_path).with_context(|| {
                format!("failed to delete already consumed journal `{}`", sealed_path.display())
            })?;
            continue;
        }
        let consuming_path = journal_root.join("consuming").join(
            sealed_path
                .file_name()
                .ok_or_else(|| anyhow!("journal file has no name"))?,
        );
        fs::rename(&sealed_path, &consuming_path).with_context(|| {
            format!(
                "failed to claim journal `{}` as `{}`",
                sealed_path.display(),
                consuming_path.display()
            )
        })?;
        return Ok(Some(ClaimedJournalFile {
            sequence,
            path: consuming_path,
        }));
    }
    Ok(None)
}

fn recover_orphan_consuming_files(
    journal_root: &Path,
    state: &JournalConsumerState,
    consumer_lease_ms: u64,
) -> anyhow::Result<()> {
    let consuming_dir = journal_root.join("consuming");
    if !consuming_dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(&consuming_dir).with_context(|| {
        format!("failed to read consuming journal dir `{}`", consuming_dir.display())
    })? {
        let entry = entry?;
        let path = entry.path();
        let metadata = entry
            .metadata()
            .with_context(|| format!("failed to stat consuming journal `{}`", path.display()))?;
        if !metadata.is_file() {
            continue;
        }
        let Some(sequence) = parse_journal_sequence(&path) else {
            continue;
        };
        if state.is_consumed(sequence)? {
            fs::remove_file(&path).with_context(|| {
                format!("failed to delete orphan consuming journal `{}`", path.display())
            })?;
            continue;
        }
        if file_age_ms(&metadata) >= consumer_lease_ms as i64 {
            let sealed_path = journal_root.join("sealed").join(
                path.file_name()
                    .ok_or_else(|| anyhow!("consuming journal file has no name"))?,
            );
            fs::rename(&path, &sealed_path).with_context(|| {
                format!(
                    "failed to recover stale consuming journal `{}` back to `{}`",
                    path.display(),
                    sealed_path.display()
                )
            })?;
        }
    }
    Ok(())
}

fn restore_claimed_file(journal_root: &Path, claim: &ClaimedJournalFile) -> anyhow::Result<()> {
    let sealed_path = journal_root.join("sealed").join(
        claim
            .path
            .file_name()
            .ok_or_else(|| anyhow!("claimed journal file has no name"))?,
    );
    fs::rename(&claim.path, &sealed_path).with_context(|| {
        format!(
            "failed to restore claimed journal `{}` back to `{}`",
            claim.path.display(),
            sealed_path.display()
        )
    })?;
    Ok(())
}

fn importing_progress(
    claim: &ClaimedJournalFile,
    processed_blocks: u64,
    processed_events: u64,
    processed_compressed_bytes: u64,
    total_compressed_bytes: u64,
) -> WorkerProgressSnapshot {
    WorkerProgressSnapshot {
        state: "importing".to_string(),
        current_file_path: Some(claim.path.display().to_string()),
        current_file_sequence: Some(claim.sequence),
        processed_blocks,
        total_blocks: 0,
        processed_events,
        total_events: 0,
        processed_compressed_bytes,
        total_compressed_bytes,
        progress_percent: progress_percent(
            processed_events,
            0,
            processed_compressed_bytes,
            total_compressed_bytes,
        ),
        heartbeat_at_ms: Some(now_ms()),
        ..WorkerProgressSnapshot::default()
    }
}

fn relaying_progress(claim: &ClaimedJournalFile) -> WorkerProgressSnapshot {
    WorkerProgressSnapshot {
        state: "relaying".to_string(),
        current_file_path: Some(claim.path.display().to_string()),
        current_file_sequence: Some(claim.sequence),
        heartbeat_at_ms: Some(now_ms()),
        ..WorkerProgressSnapshot::default()
    }
}

async fn import_relayed_journal_bytes(
    journal_root: &Path,
    duckdb_usage: Arc<DuckDbUsageRepository>,
    attribution_resolver: Option<Arc<UsageEventAttributionResolver>>,
    source_node_id: &str,
    file_sequence: u64,
    body: Bytes,
) -> anyhow::Result<RelayImportResponse> {
    let digest = format!("{:x}", Sha256::digest(body.as_ref()));
    if JournalConsumerState::open(journal_root)?.is_relay_consumed(
        source_node_id,
        file_sequence,
        &digest,
    )? {
        return Ok(RelayImportResponse {
            imported: false,
            already_imported: true,
            source_node_id: source_node_id.to_string(),
            file_sequence,
            event_count: 0,
        });
    }
    let relay_tmp_dir = journal_root.join("relay-tmp");
    fs::create_dir_all(&relay_tmp_dir).with_context(|| {
        format!("failed to create relay temp dir `{}`", relay_tmp_dir.display())
    })?;
    let temp_path = relay_tmp_dir.join(format!(
        "relay-{}-{file_sequence:010}-{}.journal",
        sanitize_file_component(source_node_id),
        Uuid::new_v4()
    ));
    fs::write(&temp_path, body.as_ref())
        .with_context(|| format!("failed to write relay temp journal `{}`", temp_path.display()))?;
    let import_result = async {
        let reader = JournalReader::open(&temp_path)?;
        let mut stream = reader.stream_batches()?;
        let mut event_count = 0u64;
        while let Some(batch) = stream.next_batch()? {
            let events = batch
                .events
                .into_iter()
                .map(|event| event.into_usage_event())
                .collect::<Vec<_>>();
            event_count = event_count.saturating_add(events.len() as u64);
            if let Some(resolver) = &attribution_resolver {
                let rows = resolver.build_usage_rows(events).await?;
                duckdb_usage.append_usage_event_rows_owned(rows).await?;
            } else {
                duckdb_usage.append_usage_events_owned(events).await?;
            }
        }
        let report = stream.finish()?;
        JournalConsumerState::open(journal_root)?.record_relay_consumed_file(
            source_node_id,
            file_sequence,
            &report.file_digest_hex,
            report.footer.event_count,
            now_ms(),
        )?;
        Ok::<RelayImportResponse, anyhow::Error>(RelayImportResponse {
            imported: true,
            already_imported: false,
            source_node_id: source_node_id.to_string(),
            file_sequence,
            event_count,
        })
    }
    .await;
    let _ = fs::remove_file(&temp_path);
    import_result
}

fn sanitize_file_component(raw: &str) -> String {
    raw.chars()
        .map(
            |ch| {
                if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                    ch
                } else {
                    '-'
                }
            },
        )
        .collect()
}

fn idle_progress() -> WorkerProgressSnapshot {
    WorkerProgressSnapshot {
        state: "idle".to_string(),
        heartbeat_at_ms: Some(now_ms()),
        ..WorkerProgressSnapshot::default()
    }
}

fn progress_percent(
    processed_events: u64,
    total_events: u64,
    processed_compressed_bytes: u64,
    total_compressed_bytes: u64,
) -> f64 {
    if total_events > 0 {
        return (processed_events as f64 / total_events as f64) * 100.0;
    }
    if total_compressed_bytes == 0 {
        0.0
    } else {
        (processed_compressed_bytes as f64 / total_compressed_bytes as f64) * 100.0
    }
}

fn parse_journal_sequence(path: &Path) -> Option<u64> {
    let file_name = path.file_name()?.to_string_lossy();
    let suffix = file_name.strip_prefix("usage-")?;
    let digits = suffix
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    digits.parse().ok()
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

fn file_age_ms(metadata: &fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| now_ms().saturating_sub(duration.as_millis() as i64))
        .unwrap_or(i64::MAX)
}

/// Run the import/relay loop until the process is stopped.
pub async fn run_forever(worker: ClusterUsageWorker) -> anyhow::Result<()> {
    const USAGE_ANALYTICS_MAINTENANCE_INTERVAL: Duration = Duration::from_secs(60 * 60);
    let mut last_maintenance = None::<std::time::Instant>;
    loop {
        if let Err(err) = worker.run_one_cycle().await {
            worker.record_error(&err);
            return Err(err);
        }
        if let ClusterUsageWorker::Primary(primary) = &worker {
            if last_maintenance
                .map(|last| last.elapsed() >= USAGE_ANALYTICS_MAINTENANCE_INTERVAL)
                .unwrap_or(true)
            {
                if let Err(err) = primary.run_maintenance(now_ms()).await {
                    tracing::warn!("llm access usage analytics maintenance failed: {err:#}");
                }
                last_maintenance = Some(std::time::Instant::now());
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, sync::Arc, time::Duration};

    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
    };
    use llm_access_core::{
        provider::{ProtocolFamily, ProviderType},
        store::{UsageAnalyticsStore, UsageEventSink},
        usage::{UsageEvent, UsageStreamDetails, UsageTiming},
    };
    use llm_access_store::duckdb::{DuckDbUsageRepository, TieredDuckDbUsageConfig};
    use llm_usage_journal::{JournalConfig, JournalReader, JournalWriter};
    use tower::ServiceExt;

    use super::UsageWorker;

    #[tokio::test]
    async fn worker_imports_sealed_journal_and_deletes_file() {
        let fixture = UsageWorkerFixture::new();
        fixture.write_sealed_event("evt-worker-1");

        fixture.run_one_import().await.expect("import");

        assert!(!fixture.sealed_path(0).exists());
        assert!(fixture.duckdb_event_exists("evt-worker-1").await);
    }

    #[tokio::test]
    async fn worker_progress_updates_after_each_committed_block() {
        let fixture = UsageWorkerFixture::new();
        fixture.write_sealed_events_in_two_blocks(["evt-progress-1", "evt-progress-2"]);

        fixture
            .run_until_first_block_commit()
            .await
            .expect("first block");
        let progress = fixture.progress_snapshot();

        assert_eq!(progress.state, "importing");
        assert_eq!(progress.processed_blocks, 1);
        assert_eq!(progress.total_blocks, 0);
        assert_eq!(progress.total_events, 0);
        assert!(progress.processed_compressed_bytes > 0);
        assert!(progress.progress_percent > 0.0);
    }

    #[tokio::test]
    async fn worker_retry_does_not_duplicate_event_id() {
        let fixture = UsageWorkerFixture::new();
        fixture.write_sealed_event("evt-idempotent");
        fixture
            .simulate_commit_before_delete()
            .await
            .expect("commit");

        fixture.run_one_import().await.expect("retry");

        assert_eq!(fixture.count_duckdb_event("evt-idempotent").await, 1);
    }

    #[tokio::test]
    async fn worker_recovers_stale_consuming_file_before_importing_next_sealed_file() {
        let fixture = UsageWorkerFixture::new_with_consumer_lease_ms(1);
        fixture.write_stale_consuming_event("evt-stale-consuming");
        fixture.write_sealed_event("evt-fresh-sealed");
        tokio::time::sleep(Duration::from_millis(10)).await;

        fixture.run_one_import().await.expect("import");

        assert!(!fixture.consuming_path(0).exists());
        assert!(fixture.duckdb_event_exists("evt-stale-consuming").await);
        assert!(!fixture.duckdb_event_exists("evt-fresh-sealed").await);

        fixture.run_one_import().await.expect("second import");
        assert!(fixture.duckdb_event_exists("evt-fresh-sealed").await);
    }

    #[tokio::test]
    async fn usage_worker_serves_legacy_llm_usage_paths() {
        let fixture = UsageWorkerFixture::new();
        fixture.write_sealed_event("evt-worker-http");
        fixture.run_one_import().await.expect("import");

        let app = super::primary_worker_router(&fixture.worker);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/admin/llm-gateway/usage?limit=1")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("list response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("list body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("list json");
        assert_eq!(value["events"][0]["id"], "evt-worker-http");

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/admin/llm-gateway/usage/evt-worker-http")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("detail response");

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn usage_worker_serves_public_usage_chart_path() {
        let fixture = UsageWorkerFixture::new();
        fixture.write_sealed_event("evt-worker-chart");
        fixture.run_one_import().await.expect("import");

        let app = super::primary_worker_router(&fixture.worker);
        let response = app
            .oneshot(
                Request::builder()
                    .uri(
                        "/admin/llm-access/usage/chart?key_id=key-1&start_ms=1700000000000&\
                         bucket_ms=3600000&bucket_count=1",
                    )
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("chart response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("chart body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("chart json");
        assert_eq!(value["chart_points"][0]["tokens"], 40);
    }

    #[tokio::test]
    async fn usage_worker_status_includes_process_memory() {
        let fixture = UsageWorkerFixture::new();
        let app = super::primary_worker_router(&fixture.worker);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/admin/llm-access/usage-worker/status")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("status response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("status body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("status json");
        assert!(value["process_memory"].is_object());
    }

    struct UsageWorkerFixture {
        _temp_dir: tempfile::TempDir,
        journal_root: PathBuf,
        worker: UsageWorker,
        duckdb: Arc<DuckDbUsageRepository>,
    }

    impl UsageWorkerFixture {
        fn new() -> Self {
            Self::new_with_consumer_lease_ms(300_000)
        }

        fn new_with_consumer_lease_ms(consumer_lease_ms: u64) -> Self {
            let temp_dir = tempfile::tempdir().expect("tempdir");
            let journal_root = temp_dir.path().join("journal");
            let duckdb = Arc::new(
                DuckDbUsageRepository::open_tiered(TieredDuckDbUsageConfig {
                    active_dir: temp_dir.path().join("duckdb-active"),
                    archive_dir: temp_dir.path().join("duckdb-archive"),
                    rollover_bytes: 1024 * 1024 * 1024,
                    details_dir: None,
                })
                .expect("open duckdb"),
            );
            let worker = UsageWorker::new(journal_root.clone(), duckdb.clone(), consumer_lease_ms)
                .expect("worker");
            Self {
                _temp_dir: temp_dir,
                journal_root,
                worker,
                duckdb,
            }
        }

        fn write_sealed_event(&self, event_id: &str) {
            self.write_sealed_events([event_id], 1024);
        }

        fn write_sealed_events_in_two_blocks<const N: usize>(&self, event_ids: [&str; N]) {
            self.write_sealed_events(event_ids, 1);
        }

        fn write_sealed_events<const N: usize>(
            &self,
            event_ids: [&str; N],
            block_max_events: usize,
        ) {
            let config = JournalConfig {
                block_max_events,
                ..JournalConfig::new(self.journal_root.clone())
            };
            let mut writer = JournalWriter::open(config).expect("writer");
            let events = event_ids
                .into_iter()
                .map(test_usage_event)
                .collect::<Vec<_>>();
            writer.append_events(&events).expect("append");
            writer.seal_current_file().expect("seal");
        }

        fn write_stale_consuming_event(&self, event_id: &str) {
            self.write_sealed_event(event_id);
            std::fs::create_dir_all(self.journal_root.join("consuming")).expect("consuming dir");
            std::fs::rename(self.sealed_path(0), self.consuming_path(0))
                .expect("move to consuming");
        }

        async fn run_one_import(&self) -> anyhow::Result<bool> {
            self.worker.run_one_import().await
        }

        async fn run_until_first_block_commit(&self) -> anyhow::Result<bool> {
            self.worker.run_until_first_block_commit().await
        }

        fn progress_snapshot(&self) -> llm_usage_journal::WorkerProgressSnapshot {
            self.worker.progress_snapshot().expect("progress")
        }

        fn sealed_path(&self, sequence: u64) -> PathBuf {
            self.journal_root
                .join("sealed")
                .join(format!("usage-{sequence:012}.journal"))
        }

        fn consuming_path(&self, sequence: u64) -> PathBuf {
            self.journal_root
                .join("consuming")
                .join(format!("usage-{sequence:012}.journal"))
        }

        async fn duckdb_event_exists(&self, event_id: &str) -> bool {
            self.duckdb
                .get_usage_event(event_id)
                .await
                .expect("query event")
                .is_some()
        }

        async fn count_duckdb_event(&self, event_id: &str) -> usize {
            usize::from(self.duckdb_event_exists(event_id).await)
        }

        async fn simulate_commit_before_delete(&self) -> anyhow::Result<()> {
            let batches = JournalReader::open(&self.sealed_path(0))?.read_all_batches()?;
            for batch in batches {
                let events = batch
                    .events
                    .into_iter()
                    .map(|event| event.into_usage_event())
                    .collect::<Vec<_>>();
                self.duckdb.append_usage_events_owned(events).await?;
            }
            Ok(())
        }
    }

    fn test_usage_event(event_id: &str) -> UsageEvent {
        UsageEvent {
            event_id: event_id.to_string(),
            created_at_ms: 1_700_000_000_000,
            provider_type: ProviderType::Kiro,
            protocol_family: ProtocolFamily::Anthropic,
            key_id: "key-1".to_string(),
            key_name: "for-yangshu".to_string(),
            account_name: Some("acct-1".to_string()),
            account_group_id_at_event: Some("group-1".to_string()),
            route_strategy_at_event: None,
            request_method: "POST".to_string(),
            request_url: "/v1/messages".to_string(),
            endpoint: "/v1/messages".to_string(),
            model: Some("claude-opus-4-7".to_string()),
            mapped_model: Some("claude-opus-4-7".to_string()),
            status_code: 200,
            request_body_bytes: Some(17),
            quota_failover_count: 0,
            routing_diagnostics_json: Some("{\"route\":\"fixed\"}".to_string()),
            input_uncached_tokens: 10,
            input_cached_tokens: 20,
            output_tokens: 30,
            billable_tokens: 40,
            credit_usage: Some("0.12".to_string()),
            usage_missing: false,
            credit_usage_missing: false,
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: "{\"user-agent\":\"test\"}".to_string(),
            last_message_content: Some("hello".to_string()),
            client_request_body_json: Some("{\"model\":\"m\"}".to_string()),
            upstream_request_body_json: Some("{\"upstream\":true}".to_string()),
            full_request_json: Some("{\"model\":\"m\"}".to_string()),
            error_message: None,
            error_body: None,
            timing: UsageTiming::default(),
            stream: UsageStreamDetails::default(),
        }
    }
}
