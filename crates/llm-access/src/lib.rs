//! Standalone HTTP service shell for LLM access.

mod activity;
mod admin;
/// Process allocator tuning.
pub mod allocator;
pub mod cluster;
mod codex_refresh;
mod codex_status;
/// Command-line and environment configuration.
pub mod config;
mod email;
mod geoip;
/// Local Kiro endpoints.
pub mod kiro;
mod kiro_cache_snapshot;
mod kiro_headers;
mod kiro_latency;
mod kiro_refresh;
mod kiro_status;
mod process_memory;
/// Provider request entrypoints.
pub mod provider;
mod public;
mod request_context;
#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
mod rollup_backlog;
/// LLM-owned route classification.
pub mod routes;
/// Runtime startup validation.
pub mod runtime;
mod submission;
mod support;
/// Usage-event helpers.
pub mod usage;
mod usage_journal;
pub mod usage_query;
#[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
pub mod usage_worker;

use std::{
    path::PathBuf,
    sync::{Arc, RwLock},
};

use anyhow::Context;
use axum::{
    body::Body,
    extract::State,
    http::{HeaderValue, Request},
    middleware,
    response::Response,
    routing::{any, delete, get, post},
    Json, Router,
};
use config::{CliCommand, ServeConfig, StorageConfig};
use llm_access_core::store::{
    AdminAccountGroupStore, AdminCodexAccountStore, AdminConfigStore, AdminKeyStore,
    AdminKiroAccountStore, AdminProxyStore, AdminReviewQueueStore, PublicAccessStore,
    PublicCommunityStore, PublicStatusStore, PublicSubmissionStore, PublicUsageStore,
};
use serde::Serialize;
use tokio::sync::Semaphore;
use tower_http::cors::{Any, CorsLayer};

#[cfg(test)]
pub(crate) static CODEX_UPSTREAM_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) static KIRO_UPSTREAM_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Clone)]
struct HttpState {
    provider_state: provider::ProviderState,
    cluster_state: Option<Arc<crate::cluster::ClusterRuntimeState>>,
    geoip: geoip::GeoIpResolver,
    request_activity: Arc<activity::RequestActivityTracker>,
    admin_config_store: Arc<dyn AdminConfigStore>,
    admin_key_store: Arc<dyn AdminKeyStore>,
    admin_account_group_store: Arc<dyn AdminAccountGroupStore>,
    admin_proxy_store: Arc<dyn AdminProxyStore>,
    admin_codex_account_store: Arc<dyn AdminCodexAccountStore>,
    admin_kiro_account_store: Arc<dyn AdminKiroAccountStore>,
    admin_review_queue_store: Arc<dyn AdminReviewQueueStore>,
    public_access_store: Arc<dyn PublicAccessStore>,
    public_community_store: Arc<dyn PublicCommunityStore>,
    public_usage_store: Arc<dyn PublicUsageStore>,
    usage_journal_dir: Option<PathBuf>,
    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    usage_journal_sink: Option<Arc<usage_journal::JournalUsageEventSink>>,
    admin_usage_query_gate: Arc<Semaphore>,
    public_submission_store: Arc<dyn PublicSubmissionStore>,
    public_submit_guard: Arc<submission::PublicSubmitGuard>,
    public_status_store: Arc<dyn PublicStatusStore>,
    email_notifier: Option<Arc<email::EmailNotifier>>,
}

/// Run `llm-access` from process arguments.
pub fn run_from_env() -> anyhow::Result<()> {
    match CliCommand::parse(std::env::args_os())? {
        CliCommand::Init(storage) => bootstrap_storage(&storage),
        CliCommand::Serve(config) => {
            bootstrap_api_storage(&config.storage)?;
            let runtime =
                tokio::runtime::Runtime::new().context("failed to create tokio runtime")?;
            runtime.block_on(serve(config))
        },
    }
}

/// Initialize llm-access storage paths.
pub fn bootstrap_api_storage(config: &StorageConfig) -> anyhow::Result<()> {
    runtime::validate_state_root(config)?;
    let database_url =
        std::env::var(&config.control_store.database_url_env).with_context(|| {
            format!("missing control database env `{}`", config.control_store.database_url_env)
        })?;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to create bootstrap runtime for postgres control")?
        .block_on(llm_access_store::initialize_postgres_target(&database_url))?;
    Ok(())
}

/// Initialize only the storage paths required by the standalone usage worker.
pub fn bootstrap_usage_worker_storage(config: &StorageConfig) -> anyhow::Result<()> {
    runtime::validate_usage_worker_state_root(config)?;
    let database_url =
        std::env::var(&config.control_store.database_url_env).with_context(|| {
            format!("missing control database env `{}`", config.control_store.database_url_env)
        })?;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to create bootstrap runtime for postgres control")?
        .block_on(llm_access_store::initialize_postgres_target(&database_url))?;
    Ok(())
}

/// Initialize llm-access storage paths, including analytics storage.
pub fn bootstrap_storage(config: &StorageConfig) -> anyhow::Result<()> {
    bootstrap_api_storage(config)?;
    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    if let Some(tiered) = &config.duckdb_tiered {
        let database_url =
            std::env::var(&config.control_store.database_url_env).with_context(|| {
                format!("missing control database env `{}`", config.control_store.database_url_env)
            })?;
        let request_cache_config = crate::config::resolve_request_cache_config(config)?;
        llm_access_store::duckdb::DuckDbUsageRepository::open_tiered_with_postgres_catalog_with_connection_config(
            llm_access_store::duckdb::TieredDuckDbUsageConfig {
                active_dir: tiered.active_dir.clone(),
                archive_dir: tiered.archive_dir.clone(),
                rollover_bytes: tiered.rollover_bytes,
                details_dir: tiered.details_dir.clone(),
            },
            Arc::new(RwLock::new(
                llm_access_store::duckdb::DuckDbUsageConnectionConfig::default(),
            )),
            &database_url,
            request_cache_config,
        )?;
    } else {
        llm_access_store::initialize_duckdb_target_path(&config.duckdb)?;
    }
    llm_access_store::write_duckdb_schema_file(duckdb_schema_output_path(config))?;
    Ok(())
}

fn duckdb_schema_output_path(config: &StorageConfig) -> PathBuf {
    if let Some(tiered) = &config.duckdb_tiered {
        if let Some(parent) = tiered.archive_dir.parent() {
            return parent.join("usage.schema.sql");
        }
        return tiered.archive_dir.join("usage.schema.sql");
    }
    config.duckdb.with_extension("schema.sql")
}

/// Build the HTTP router and hand back the shared Kiro cache simulator so the
/// serve loop can snapshot it to Valkey and restore it on startup.
pub fn router_with_simulator(
    runtime: runtime::LlmAccessRuntime,
) -> (Router, Arc<llm_access_kiro::cache_sim::KiroCacheSimulator>) {
    let request_activity = Arc::new(activity::RequestActivityTracker::new());
    let geoip = runtime.geoip();
    let provider_state = provider::ProviderState::new_with_config_store_activity_and_latency(
        runtime.control_store(),
        runtime.provider_route_store(),
        runtime.admin_config_store(),
        Arc::clone(&request_activity),
        geoip.clone(),
        runtime.kiro_latency_ranker(),
    );
    let kiro_cache_simulator = provider_state.kiro_cache_simulator();
    let state = HttpState {
        provider_state,
        cluster_state: runtime.cluster_state(),
        geoip,
        request_activity,
        admin_config_store: runtime.admin_config_store(),
        admin_key_store: runtime.admin_key_store(),
        admin_account_group_store: runtime.admin_account_group_store(),
        admin_proxy_store: runtime.admin_proxy_store(),
        admin_codex_account_store: runtime.admin_codex_account_store(),
        admin_kiro_account_store: runtime.admin_kiro_account_store(),
        admin_review_queue_store: runtime.admin_review_queue_store(),
        public_access_store: runtime.public_access_store(),
        public_community_store: runtime.public_community_store(),
        public_usage_store: runtime.public_usage_store(),
        usage_journal_dir: runtime.usage_journal_dir(),
        #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
        usage_journal_sink: runtime.usage_journal_sink(),
        admin_usage_query_gate: Arc::new(Semaphore::new(1)),
        public_submission_store: runtime.public_submission_store(),
        public_submit_guard: Arc::new(submission::PublicSubmitGuard::default()),
        public_status_store: runtime.public_status_store(),
        email_notifier: runtime.email_notifier(),
    };
    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/version", get(version))
        .route(
            "/admin/llm-gateway/config",
            get(admin::get_llm_gateway_config).post(admin::post_llm_gateway_config),
        )
        .route(
            "/admin/llm-gateway/keys",
            get(admin::list_llm_gateway_keys).post(admin::create_llm_gateway_key),
        )
        .route(
            "/admin/llm-gateway/keys/:key_id",
            axum::routing::patch(admin::patch_llm_gateway_key)
                .delete(admin::delete_llm_gateway_key),
        )
        .route(
            "/admin/llm-gateway/account-groups",
            get(admin::list_llm_gateway_account_groups)
                .post(admin::create_llm_gateway_account_group),
        )
        .route(
            "/admin/llm-gateway/account-group-options",
            get(admin::list_llm_gateway_account_group_options),
        )
        .route(
            "/admin/llm-gateway/account-groups/:group_id",
            axum::routing::patch(admin::patch_llm_gateway_account_group)
                .delete(admin::delete_llm_gateway_account_group),
        )
        .route(
            "/admin/llm-gateway/proxy-configs",
            get(admin::list_llm_gateway_proxy_configs).post(admin::create_llm_gateway_proxy_config),
        )
        .route(
            "/admin/llm-gateway/proxy-configs/:proxy_id",
            axum::routing::patch(admin::patch_llm_gateway_proxy_config)
                .delete(admin::delete_llm_gateway_proxy_config),
        )
        .route(
            "/admin/llm-gateway/proxy-configs/:proxy_id/check/:provider_type",
            post(admin::check_llm_gateway_proxy_config),
        )
        .route(
            "/admin/llm-gateway/proxy-configs/:proxy_id/override",
            delete(admin::reset_llm_gateway_proxy_config_override),
        )
        .route(
            "/admin/llm-gateway/proxy-configs/import-legacy-kiro",
            post(admin::import_legacy_kiro_proxy_configs),
        )
        .route("/admin/llm-gateway/proxy-bindings", get(admin::list_llm_gateway_proxy_bindings))
        .route(
            "/admin/llm-gateway/proxy-bindings/:provider_type",
            post(admin::update_llm_gateway_proxy_binding),
        )
        .route(
            "/admin/llm-gateway/accounts",
            get(admin::list_llm_gateway_accounts).post(admin::import_llm_gateway_account),
        )
        .route(
            "/admin/llm-gateway/accounts/import-jobs",
            get(admin::list_llm_gateway_account_import_jobs)
                .post(admin::create_llm_gateway_account_import_job),
        )
        .route(
            "/admin/llm-gateway/accounts/import-jobs/:job_id",
            get(admin::get_llm_gateway_account_import_job),
        )
        .route(
            "/admin/llm-gateway/accounts/:name",
            axum::routing::patch(admin::patch_llm_gateway_account)
                .delete(admin::delete_llm_gateway_account),
        )
        .route(
            "/admin/llm-gateway/accounts/:name/refresh",
            post(admin::refresh_llm_gateway_account),
        )
        .route(
            "/admin/llm-gateway/accounts/:name/refresh-auth",
            post(admin::refresh_llm_gateway_account_auth),
        )
        .route(
            "/admin/llm-gateway/accounts/:name/refresh-usage",
            post(admin::refresh_llm_gateway_account_usage),
        )
        .route(
            "/admin/llm-gateway/accounts/:name/probe-models",
            post(admin::probe_llm_gateway_account_models),
        )
        .route("/admin/llm-gateway/usage", get(admin::list_llm_gateway_usage_events))
        .route(
            "/admin/llm-gateway/usage/filter-options",
            get(admin::get_llm_gateway_usage_filter_options),
        )
        .route("/admin/llm-gateway/usage/metrics", get(admin::get_llm_gateway_usage_metrics))
        .route("/admin/llm-gateway/usage/:event_id", get(admin::get_llm_gateway_usage_event))
        .route("/admin/llm-access/usage-journal/status", get(admin::get_usage_journal_status))
        .route("/admin/llm-gateway/usage-journal/status", get(admin::get_usage_journal_status))
        .route("/admin/llm-access/usage-journal/preview", get(admin::get_usage_journal_preview))
        .route("/admin/llm-gateway/usage-journal/preview", get(admin::get_usage_journal_preview))
        .route("/admin/llm-gateway/token-requests", get(admin::list_llm_gateway_token_requests))
        .route(
            "/admin/llm-gateway/token-requests/:request_id/approve-and-issue",
            post(admin::approve_and_issue_llm_gateway_token_request),
        )
        .route(
            "/admin/llm-gateway/token-requests/:request_id/reject",
            post(admin::reject_llm_gateway_token_request),
        )
        .route(
            "/admin/llm-gateway/account-contribution-requests",
            get(admin::list_llm_gateway_account_contribution_requests),
        )
        .route(
            "/admin/llm-gateway/account-contribution-requests/:request_id/approve-and-issue",
            post(admin::approve_and_issue_llm_gateway_account_contribution_request),
        )
        .route(
            "/admin/llm-gateway/account-contribution-requests/:request_id/validate",
            post(admin::validate_llm_gateway_account_contribution_request),
        )
        .route(
            "/admin/llm-gateway/account-contribution-requests/:request_id/reject",
            post(admin::reject_llm_gateway_account_contribution_request),
        )
        .route("/admin/llm-gateway/sponsor-requests", get(admin::list_llm_gateway_sponsor_requests))
        .route(
            "/admin/llm-gateway/sponsor-requests/:request_id/approve",
            post(admin::approve_llm_gateway_sponsor_request),
        )
        .route(
            "/admin/llm-gateway/sponsor-requests/:request_id",
            axum::routing::delete(admin::delete_llm_gateway_sponsor_request),
        )
        .route(
            "/admin/kiro-gateway/account-groups",
            get(admin::list_admin_kiro_account_groups).post(admin::create_admin_kiro_account_group),
        )
        .route(
            "/admin/kiro-gateway/account-group-options",
            get(admin::list_admin_kiro_account_group_options),
        )
        .route(
            "/admin/kiro-gateway/account-groups/:group_id",
            axum::routing::patch(admin::patch_admin_kiro_account_group)
                .delete(admin::delete_admin_kiro_account_group),
        )
        .route(
            "/admin/kiro-gateway/keys",
            get(admin::list_admin_kiro_keys).post(admin::create_admin_kiro_key),
        )
        .route(
            "/admin/kiro-gateway/keys/:key_id",
            axum::routing::patch(admin::patch_admin_kiro_key).delete(admin::delete_admin_kiro_key),
        )
        .route("/admin/kiro-gateway/usage", get(admin::list_admin_kiro_usage_events))
        .route("/admin/kiro-gateway/usage/:event_id", get(admin::get_admin_kiro_usage_event))
        .route(
            "/admin/kiro-gateway/accounts/statuses",
            get(admin::list_admin_kiro_account_statuses),
        )
        .route("/admin/kiro-gateway/cache-stats", get(admin::get_admin_kiro_cache_stats))
        .route(
            "/admin/kiro-gateway/accounts",
            get(admin::list_admin_kiro_accounts).post(admin::create_admin_kiro_manual_account),
        )
        .route("/admin/kiro-gateway/accounts/import-local", post(admin::import_admin_kiro_account))
        .route(
            "/admin/kiro-gateway/accounts/:name",
            axum::routing::patch(admin::patch_admin_kiro_account)
                .delete(admin::delete_admin_kiro_account),
        )
        .route(
            "/admin/kiro-gateway/accounts/:name/balance",
            get(admin::get_admin_kiro_account_balance)
                .post(admin::refresh_admin_kiro_account_balance),
        )
        .route(
            "/admin/kiro-gateway/accounts/:name/probe-model",
            post(admin::probe_admin_kiro_account_model),
        )
        .route("/api/llm-gateway/access", get(public::get_llm_gateway_access))
        .route("/api/llm-gateway/public-page", get(public::get_llm_gateway_public_page))
        .route("/api/llm-gateway/model-catalog.json", get(public::get_llm_gateway_model_catalog))
        .route("/api/llm-gateway/status", get(public::get_llm_gateway_status))
        .route(
            "/api/llm-gateway/public-usage/query",
            post(public::post_llm_gateway_public_usage_query),
        )
        .route("/api/llm-gateway/support-config", get(public::get_llm_gateway_support_config))
        .route(
            "/api/llm-gateway/account-contributions",
            get(public::get_llm_gateway_account_contributions),
        )
        .route("/api/llm-gateway/sponsors", get(public::get_llm_gateway_sponsors))
        .route(
            "/api/llm-gateway/token-requests/submit",
            post(submission::submit_public_token_request),
        )
        .route(
            "/api/llm-gateway/account-contribution-requests/submit",
            post(submission::submit_public_account_contribution_request),
        )
        .route(
            "/api/llm-gateway/account-contribution-requests/batch-submit",
            post(submission::submit_public_account_contribution_batch_request),
        )
        .route(
            "/api/llm-gateway/sponsor-requests/submit",
            post(submission::submit_public_sponsor_request),
        )
        .route(
            "/api/llm-gateway/support-assets/:file_name",
            get(public::get_llm_gateway_support_asset),
        )
        .route("/api/kiro-gateway/access", get(public::get_kiro_gateway_access))
        .route("/v1/chat/completions", post(provider_entry_handler))
        .route("/v1/responses", post(provider_entry_handler))
        .route("/v1/models", get(provider_entry_handler))
        .route("/v1/messages", post(provider_entry_handler))
        .route("/v1/messages/count_tokens", post(kiro::count_tokens))
        .route("/cc/v1/messages", post(provider_entry_handler))
        .route("/api/kiro-gateway/v1/models", get(kiro::get_models))
        .route("/api/kiro-gateway/v1/messages/count_tokens", post(kiro::count_tokens))
        .route("/api/kiro-gateway/cc/v1/messages/count_tokens", post(kiro::count_tokens))
        .route("/api/llm-gateway/*path", any(provider_entry_handler))
        .route("/api/kiro-gateway/*path", any(provider_entry_handler))
        .route("/api/codex-gateway/*path", any(provider_entry_handler))
        .route("/api/llm-access/*path", any(provider_entry_handler))
        .layer(middleware::from_fn(request_context::request_context_middleware))
        .layer(cors_layer())
        .with_state(state);
    (app, kiro_cache_simulator)
}

/// Build the HTTP router, discarding the cache simulator handle. Retained for
/// callers that do not need snapshot persistence (e.g. tests).
pub fn router(runtime: runtime::LlmAccessRuntime) -> Router {
    router_with_simulator(runtime).0
}

fn cors_layer() -> CorsLayer {
    let allowed_origins = std::env::var("LLM_ACCESS_ALLOWED_ORIGINS")
        .ok()
        .or_else(|| std::env::var("ALLOWED_ORIGINS").ok())
        .and_then(|value| parse_allowed_origins(&value));

    let layer = CorsLayer::new().allow_methods(Any).allow_headers(Any);
    match allowed_origins {
        Some(origins) => layer.allow_origin(origins),
        None => layer.allow_origin(Any),
    }
}

fn parse_allowed_origins(value: &str) -> Option<Vec<HeaderValue>> {
    let origins = value
        .split(',')
        .filter_map(|origin| {
            let origin = origin.trim();
            if origin.is_empty() {
                None
            } else {
                origin.parse::<HeaderValue>().ok()
            }
        })
        .collect::<Vec<_>>();

    if origins.is_empty() {
        None
    } else {
        Some(origins)
    }
}

async fn provider_entry_handler(
    State(state): State<HttpState>,
    request: Request<Body>,
) -> Response {
    provider::provider_entry(state.provider_state, request).await
}

/// Run the HTTP server until interrupted.
pub async fn serve(config: ServeConfig) -> anyhow::Result<()> {
    let service_runtime = runtime::LlmAccessRuntime::from_storage_config(&config.storage).await?;
    allocator::collect_process_allocator();
    let cluster_role = match service_runtime.cluster_state() {
        Some(cluster) => Some(cluster.runtime_role().await),
        None => None,
    };
    if background_status_refresh_enabled_with_role(
        config.storage.node_identity.as_ref(),
        cluster_role,
    ) {
        codex_status::spawn_codex_status_refresher(&service_runtime);
        kiro_status::spawn_kiro_status_refresher(&service_runtime);
        kiro_latency::spawn_kiro_latency_refresher(
            service_runtime.admin_config_store(),
            service_runtime.kiro_latency_ranker(),
        );
    } else {
        tracing::info!(
            "background provider status refresh is disabled by \
             LLM_ACCESS_BACKGROUND_STATUS_REFRESH_ENABLED"
        );
    }
    let shutdown_runtime = service_runtime.clone();
    let admin_config_store = service_runtime.admin_config_store();
    let listener = tokio::net::TcpListener::bind(config.bind_addr)
        .await
        .with_context(|| format!("failed to bind {}", config.bind_addr))?;
    spawn_allocator_collector();
    let (app, kiro_cache_simulator) = router_with_simulator(service_runtime);
    let snapshot_handle =
        setup_kiro_cache_snapshot(&config.storage, admin_config_store, kiro_cache_simulator).await;
    let app = app.into_make_service_with_connect_info::<std::net::SocketAddr>();
    allocator::collect_process_allocator();
    let result = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("llm-access server failed");
    if let Some(handle) = snapshot_handle {
        // Bound the final flush so a stuck Valkey cannot block process exit.
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle.shutdown()).await;
    }
    shutdown_runtime.shutdown_usage_events().await;
    result
}

/// Wire up cross-node Kiro cache snapshot persistence. Returns the periodic
/// task handle when a request cache is configured, restoring the simulator
/// before serving when the feature is enabled. Best-effort throughout: a
/// missing or broken cache simply yields `None`.
async fn setup_kiro_cache_snapshot(
    storage: &StorageConfig,
    admin_config_store: Arc<dyn AdminConfigStore>,
    simulator: Arc<llm_access_kiro::cache_sim::KiroCacheSimulator>,
) -> Option<kiro_cache_snapshot::KiroCacheSnapshotHandle> {
    let cache_config = match config::resolve_request_cache_config(storage) {
        Ok(Some(cache_config)) => cache_config,
        Ok(None) => return None,
        Err(error) => {
            tracing::warn!(%error, "failed to resolve request cache for kiro cache snapshot");
            return None;
        },
    };
    let node_id = storage
        .node_identity
        .as_ref()
        .map(|identity| identity.node_id.clone());
    let store = match kiro_cache_snapshot::KiroCacheSnapshotStore::new(&cache_config, node_id) {
        Ok(store) => store,
        Err(error) => {
            tracing::warn!(%error, "failed to open kiro cache snapshot store");
            return None;
        },
    };
    match admin_config_store.get_admin_runtime_config().await {
        Ok(config) if config.kiro_cache_snapshot_enabled => {
            // Bound the restore so a slow/black-holed Valkey cannot stall
            // startup and health checks: the listener is already bound but not
            // yet serving. On timeout we start empty and the periodic task
            // re-warms on the next tick.
            match tokio::time::timeout(
                std::time::Duration::from_secs(5),
                kiro_cache_snapshot::restore_simulator(&store, &simulator, &config),
            )
            .await
            {
                Ok(outcome) => {
                    tracing::info!(?outcome, "restored kiro cache snapshot from valkey");
                },
                Err(_) => {
                    tracing::warn!("kiro cache snapshot restore timed out; starting empty");
                },
            }
        },
        Ok(_) => {},
        Err(error) => {
            tracing::warn!(%error, "failed to read runtime config for kiro cache snapshot restore");
        },
    }
    Some(kiro_cache_snapshot::spawn(store, simulator, admin_config_store))
}

fn background_status_refresh_enabled_with_role(
    node_identity: Option<&crate::cluster::NodeIdentity>,
    runtime_role: Option<crate::cluster::NodeRuntimeRole>,
) -> bool {
    background_status_refresh_enabled_for_role(
        runtime_role.or_else(|| node_identity.and_then(background_status_refresh_runtime_role)),
        std::env::var("LLM_ACCESS_BACKGROUND_STATUS_REFRESH_ENABLED")
            .ok()
            .as_deref(),
    )
}

fn background_status_refresh_enabled_for_role(
    role: Option<crate::cluster::NodeRuntimeRole>,
    raw: Option<&str>,
) -> bool {
    if matches!(
        role,
        Some(
            crate::cluster::NodeRuntimeRole::EdgeSecondary
                | crate::cluster::NodeRuntimeRole::Degraded
        )
    ) {
        return false;
    }
    background_status_refresh_enabled_from_raw(raw)
}

fn background_status_refresh_enabled_from_raw(raw: Option<&str>) -> bool {
    let Some(raw) = raw else {
        return true;
    };
    !matches!(raw.trim().to_ascii_lowercase().as_str(), "0" | "false" | "no" | "off")
}

fn background_status_refresh_runtime_role(
    node_identity: &crate::cluster::NodeIdentity,
) -> Option<crate::cluster::NodeRuntimeRole> {
    match node_identity.node_class {
        crate::cluster::NodeClass::Core => Some(crate::cluster::NodeRuntimeRole::Primary),
        crate::cluster::NodeClass::Edge => Some(crate::cluster::NodeRuntimeRole::EdgeSecondary),
    }
}

fn spawn_allocator_collector() {
    tokio::spawn(async {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            allocator::collect_process_allocator();
        }
    });
}

async fn shutdown_signal() {
    if let Err(err) = tokio::signal::ctrl_c().await {
        tracing::warn!("failed to listen for shutdown signal: {err}");
    }
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    service: &'static str,
}

#[derive(Serialize)]
struct VersionResponse {
    service: &'static str,
    version: &'static str,
}

async fn healthz() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        service: "llm-access",
    })
}

async fn version() -> Json<VersionResponse> {
    Json(VersionResponse {
        service: "llm-access",
        version: env!("CARGO_PKG_VERSION"),
    })
}

#[cfg(test)]
#[allow(
    clippy::await_holding_lock,
    reason = "router tests serialize process-wide support and Kiro upstream env var overrides"
)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use axum::{
        body::{to_bytes, Body},
        http::{header, Request, StatusCode},
    };
    use llm_access_core::store::{AuthenticatedKey, ControlStore};
    use tower::util::ServiceExt;

    static SUPPORT_ENV_LOCK: Mutex<()> = Mutex::new(());

    #[derive(Default)]
    struct EmptyStore;

    #[async_trait]
    impl ControlStore for EmptyStore {
        async fn authenticate_bearer_secret(
            &self,
            _secret: &str,
        ) -> anyhow::Result<Option<AuthenticatedKey>> {
            Ok(None)
        }

        async fn apply_usage_rollup(
            &self,
            _event: &llm_access_core::usage::UsageEvent,
        ) -> anyhow::Result<()> {
            Ok(())
        }
    }

    fn test_router() -> axum::Router {
        let runtime = crate::runtime::LlmAccessRuntime::new(Arc::new(EmptyStore));
        super::router(runtime)
    }

    #[test]
    fn background_status_refresh_env_defaults_to_enabled() {
        assert!(super::background_status_refresh_enabled_for_role(None, None));
        assert!(super::background_status_refresh_enabled_for_role(None, Some("1")));
        assert!(super::background_status_refresh_enabled_for_role(None, Some("true")));
    }

    #[test]
    fn background_status_refresh_env_accepts_false_values() {
        assert!(!super::background_status_refresh_enabled_for_role(None, Some("0")));
        assert!(!super::background_status_refresh_enabled_for_role(None, Some("false")));
        assert!(!super::background_status_refresh_enabled_for_role(None, Some(" OFF ")));
    }

    #[test]
    fn background_status_refresh_requires_primary_runtime_role() {
        assert!(super::background_status_refresh_enabled_for_role(
            Some(crate::cluster::NodeRuntimeRole::Primary),
            Some("1")
        ));
        assert!(!super::background_status_refresh_enabled_for_role(
            Some(crate::cluster::NodeRuntimeRole::EdgeSecondary),
            Some("1")
        ));
        assert!(!super::background_status_refresh_enabled_for_role(
            Some(crate::cluster::NodeRuntimeRole::Degraded),
            Some("1")
        ));
    }

    #[tokio::test]
    async fn router_answers_llm_gateway_cors_preflight() {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri("/api/llm-gateway/access")
                    .header(header::ORIGIN, "https://acking-you.github.io")
                    .header(header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
                    .header(
                        header::ACCESS_CONTROL_REQUEST_HEADERS,
                        "x-sf-client,x-sf-page,cache-control,pragma",
                    )
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        assert!(response
            .headers()
            .contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN));
        assert!(response
            .headers()
            .contains_key(header::ACCESS_CONTROL_ALLOW_HEADERS));
    }

    #[tokio::test]
    async fn router_attaches_request_and_trace_headers() {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .header("x-request-id", "req-existing")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get("x-request-id"),
            Some(&header::HeaderValue::from_static("req-existing"))
        );
        let trace_id = response
            .headers()
            .get("x-trace-id")
            .and_then(|value| value.to_str().ok())
            .expect("trace header");
        assert!(trace_id.starts_with("trace-"));
    }

    #[tokio::test]
    async fn router_serves_kiro_models_without_provider_key() {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .uri("/api/kiro-gateway/v1/models")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let body = String::from_utf8(body.to_vec()).expect("utf8 body");
        assert!(body.contains(r#""object":"list""#));
        assert!(body.contains("claude-sonnet-4-6"));
    }

    #[tokio::test]
    async fn router_serves_kiro_count_tokens_without_provider_key() {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/kiro-gateway/v1/messages/count_tokens")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"model":"claude-sonnet-4-6","messages":[{"role":"user","content":"hello"}]}"#,
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let body = String::from_utf8(body.to_vec()).expect("utf8 body");
        assert!(body.contains(r#""input_tokens":"#));
    }

    #[tokio::test]
    async fn router_serves_kiro_public_access_without_provider_key() {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .uri("/api/kiro-gateway/access")
                    .header(header::HOST, "example.test")
                    .header("x-forwarded-proto", "https")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let body = String::from_utf8(body.to_vec()).expect("utf8 body");
        assert!(body.contains(r#""base_url":"https://example.test/api/kiro-gateway""#));
        assert!(body.contains(r#""accounts":[]"#));
    }

    #[tokio::test]
    async fn router_serves_llm_gateway_public_access_without_provider_key() {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .uri("/api/llm-gateway/access")
                    .header(header::HOST, "example.test")
                    .header("x-forwarded-proto", "https")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let body = String::from_utf8(body.to_vec()).expect("utf8 body");
        assert!(body.contains(r#""base_url":"https://example.test/api/llm-gateway/v1""#));
        assert!(body.contains(r#""model_catalog_path":"/api/llm-gateway/model-catalog.json""#));
        assert!(body.contains(r#""keys":[]"#));
    }

    #[tokio::test]
    async fn router_serves_llm_gateway_public_page_without_provider_key() {
        let _guard = SUPPORT_ENV_LOCK.lock().expect("support env lock");
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let root = std::env::temp_dir()
            .join(format!("llm-access-public-page-{}-{unique}", std::process::id()));
        std::fs::create_dir_all(&root).expect("create support dir");
        std::fs::write(
            root.join("config.json"),
            r#"{
                "owner_display_name":"StaticFlow",
                "sponsor_title":"Support StaticFlow",
                "sponsor_intro":"Keep the shared LLM pool healthy.",
                "group_name":"StaticFlow Group",
                "qq_group_number":"123456",
                "group_invite_text":"Join the group",
                "payment_email_subject":"Payment instructions",
                "payment_email_signature":"StaticFlow"
            }"#,
        )
        .expect("write support config");
        std::env::set_var("LLM_ACCESS_SUPPORT_DIR", &root);

        let response = test_router()
            .oneshot(
                Request::builder()
                    .uri("/api/llm-gateway/public-page")
                    .header(header::HOST, "example.test")
                    .header("x-forwarded-proto", "https")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        std::env::remove_var("LLM_ACCESS_SUPPORT_DIR");
        std::fs::remove_dir_all(&root).expect("cleanup support dir");

        assert_eq!(response.status(), StatusCode::OK);
        let cache_control = response
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(value["access"]["base_url"], "https://example.test/api/llm-gateway/v1");
        assert_eq!(value["access"]["model_catalog_path"], "/api/llm-gateway/model-catalog.json");
        assert!(value["access"]["keys"].as_array().expect("keys").is_empty());
        assert!(value["account_contributions"]["contributions"]
            .as_array()
            .expect("contributions")
            .is_empty());
        assert!(value["sponsors"]["sponsors"]
            .as_array()
            .expect("sponsors")
            .is_empty());
        assert_eq!(cache_control.as_deref(), Some("public, max-age=10, stale-while-revalidate=30"));
    }

    #[tokio::test]
    async fn router_serves_llm_gateway_model_catalog_without_provider_key() {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .uri("/api/llm-gateway/model-catalog.json")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/json; charset=utf-8")
        );
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let body = String::from_utf8(body.to_vec()).expect("utf8 body");
        assert!(body.contains(r#""models":["#));
        assert!(body.contains(r#""slug":"gpt-5.5""#));
        assert!(body.contains(r#""base_instructions":"#));
    }

    #[tokio::test]
    async fn router_serves_llm_gateway_status_without_provider_key() {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .uri("/api/llm-gateway/status")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let body = String::from_utf8(body.to_vec()).expect("utf8 body");
        assert!(body.contains(r#""status":"loading""#));
        assert!(body.contains(r#""accounts":[]"#));
        assert!(body.contains(r#""buckets":[]"#));
    }

    #[tokio::test]
    async fn router_serves_llm_gateway_support_config_without_provider_key() {
        let _guard = SUPPORT_ENV_LOCK.lock().expect("support env lock");
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let root = std::env::temp_dir()
            .join(format!("llm-access-support-config-{}-{unique}", std::process::id()));
        std::fs::create_dir_all(&root).expect("create support dir");
        std::fs::write(
            root.join("config.json"),
            r#"{
                "owner_display_name":"StaticFlow",
                "sponsor_title":"Support StaticFlow",
                "sponsor_intro":"Keep the shared LLM pool healthy.",
                "group_name":"StaticFlow Group",
                "qq_group_number":"123456",
                "group_invite_text":"Join the group",
                "payment_email_subject":"Payment instructions",
                "payment_email_signature":"StaticFlow"
            }"#,
        )
        .expect("write support config");
        std::env::set_var("LLM_ACCESS_SUPPORT_DIR", &root);

        let response = test_router()
            .oneshot(
                Request::builder()
                    .uri("/api/llm-gateway/support-config")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        std::env::remove_var("LLM_ACCESS_SUPPORT_DIR");
        std::fs::remove_dir_all(&root).expect("cleanup support dir");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let body = String::from_utf8(body.to_vec()).expect("utf8 body");
        assert!(body.contains(r#""sponsor_title":"Support StaticFlow""#));
        assert!(body.contains(r#""qq_group_number":"123456""#));
        assert!(body.contains(r#""alipay_qr_url":"/api/llm-gateway/support-assets/alipay_qr.png""#));
    }

    #[tokio::test]
    async fn router_serves_llm_gateway_account_contributions_without_provider_key() {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .uri("/api/llm-gateway/account-contributions")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let body = String::from_utf8(body.to_vec()).expect("utf8 body");
        assert!(body.contains(r#""contributions":[]"#));
    }

    #[tokio::test]
    async fn router_serves_llm_gateway_sponsors_without_provider_key() {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .uri("/api/llm-gateway/sponsors")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let body = String::from_utf8(body.to_vec()).expect("utf8 body");
        assert!(body.contains(r#""sponsors":[]"#));
    }

    #[tokio::test]
    async fn router_accepts_llm_gateway_token_request_without_provider_key() {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/llm-gateway/token-requests/submit")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header("x-real-ip", "198.51.100.10")
                    .body(Body::from(
                        r#"{
                            "requested_quota_billable_limit": 1000,
                            "request_reason": "please issue a test key",
                            "requester_email": "user@example.com",
                            "frontend_page_url": "https://example.test/llm-access"
                        }"#,
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let body = String::from_utf8(body.to_vec()).expect("utf8 body");
        assert!(body.contains(r#""request_id":"llmwish-"#));
        assert!(body.contains(r#""status":"pending""#));
    }

    #[tokio::test]
    async fn router_handles_llm_gateway_public_usage_query_without_provider_key() {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/llm-gateway/public-usage/query")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"api_key":"missing"}"#))
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let body = String::from_utf8(body.to_vec()).expect("utf8 body");
        assert!(body.contains("queryable key not found"));
    }

    #[test]
    fn bootstrap_usage_worker_storage_skips_api_auth_and_log_directories() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let root = std::env::temp_dir()
            .join(format!("llm-access-worker-bootstrap-{}-{unique}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create state root");

        let config = crate::config::StorageConfig {
            state_root: root.clone(),
            node_identity: None,
            control_store: crate::config::ControlStoreConfig {
                database_url_env: "LLM_ACCESS_CONTROL_DATABASE_URL".to_string(),
            },
            request_cache: None,
            duckdb: root.join("analytics/usage.duckdb"),
            usage_journal_dir: root.join("usage-journal"),
            duckdb_tiered: None,
            kiro_auths_dir: root.join("auths/kiro"),
            codex_auths_dir: root.join("auths/codex"),
            logs_dir: root.join("logs"),
        };

        crate::runtime::validate_usage_worker_state_root(&config)
            .expect("validate usage worker storage");

        assert!(config.usage_journal_dir.is_dir());
        assert!(!config.kiro_auths_dir.exists());
        assert!(!config.codex_auths_dir.exists());
        assert!(!config.logs_dir.exists());

        std::fs::remove_dir_all(&root).expect("cleanup state root");
    }

    #[tokio::test]
    async fn router_serves_admin_runtime_config_for_local_request() {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .uri("/admin/llm-gateway/config")
                    .header(header::HOST, "localhost")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(value["auth_cache_ttl_seconds"], 60);
        assert_eq!(value["max_request_body_bytes"], 8 * 1024 * 1024);
        assert_eq!(value["codex_client_version"], "0.124.0");
        assert_eq!(value["kiro_prefix_cache_mode"], "prefix_tree");
        assert_eq!(value["kiro_context_usage_min_request_tokens"], 15_000);
    }

    #[tokio::test]
    async fn router_serves_admin_kiro_cache_stats_for_local_request() {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .uri("/admin/kiro-gateway/cache-stats")
                    .header(header::HOST, "localhost")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(value["mode"], "prefix_tree");
        assert_eq!(value["page_size_tokens"], 64);
        assert_eq!(
            value["prefix_tree"]["max_tokens"],
            llm_access_core::store::DEFAULT_KIRO_PREFIX_CACHE_MAX_TOKENS
        );
        assert_eq!(value["prefix_tree"]["resident_tokens"], 0);
        assert_eq!(value["conversation_anchors"]["entries"], 0);
        assert!(value["generated_at"].as_i64().unwrap_or_default() > 0);
    }

    #[tokio::test]
    async fn router_rejects_remote_admin_runtime_config_without_token() {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .uri("/admin/llm-gateway/config")
                    .header(header::HOST, "ackingliu.top")
                    .header("x-forwarded-for", "198.51.100.10")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let body = String::from_utf8(body.to_vec()).expect("utf8 body");
        assert!(body.contains("Admin endpoint is local-only"));
    }

    #[tokio::test]
    async fn router_updates_admin_runtime_config_for_local_request() {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/llm-gateway/config")
                    .header(header::HOST, "localhost")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{
                            "auth_cache_ttl_seconds": 120,
                            "max_request_body_bytes": 2097152,
                            "codex_client_version": " 0.125.0 ",
                            "kiro_prefix_cache_mode": "formula",
                            "kiro_context_usage_min_request_tokens": 12345
                        }"#,
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(value["auth_cache_ttl_seconds"], 120);
        assert_eq!(value["max_request_body_bytes"], 2 * 1024 * 1024);
        assert_eq!(value["codex_client_version"], "0.125.0");
        assert_eq!(value["kiro_prefix_cache_mode"], "formula");
        assert_eq!(value["kiro_context_usage_min_request_tokens"], 12_345);
    }

    #[tokio::test]
    async fn router_lists_admin_llm_gateway_keys_for_local_request() {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .uri("/admin/llm-gateway/keys")
                    .header(header::HOST, "localhost")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(value["auth_cache_ttl_seconds"], 60);
        assert_eq!(value["keys"].as_array().expect("keys array").len(), 0);
        assert_eq!(value["total"], 0);
        assert_eq!(value["limit"], 50);
        assert_eq!(value["offset"], 0);
        assert_eq!(value["has_more"], false);
    }

    #[tokio::test]
    async fn router_creates_admin_llm_gateway_key_for_local_request() {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/llm-gateway/keys")
                    .header(header::HOST, "localhost")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{
                            "name": "external codex",
                            "quota_billable_limit": 1000,
                            "public_visible": true,
                            "request_max_concurrency": 2,
                            "request_min_start_interval_ms": 50
                        }"#,
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("json body");
        assert!(value["id"].as_str().expect("id").starts_with("llm-key-"));
        assert_eq!(value["name"], "external codex");
        assert!(value["secret"]
            .as_str()
            .expect("secret")
            .starts_with("sfk_"));
        assert_eq!(value["status"], "active");
        assert_eq!(value["provider_type"], "codex");
        assert_eq!(value["public_visible"], true);
        assert_eq!(value["quota_billable_limit"], 1000);
        assert_eq!(value["remaining_billable"], 1000);
        assert_eq!(value["request_max_concurrency"], 2);
        assert_eq!(value["request_min_start_interval_ms"], 50);
    }

    #[tokio::test]
    async fn router_routes_admin_llm_gateway_key_patch_to_store() {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/admin/llm-gateway/keys/missing")
                    .header(header::HOST, "localhost")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"name":"patched"}"#))
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let body = String::from_utf8(body.to_vec()).expect("utf8 body");
        assert!(body.contains("LLM gateway key not found"));
    }

    #[tokio::test]
    async fn router_routes_admin_llm_gateway_key_delete_to_store() {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/admin/llm-gateway/keys/missing")
                    .header(header::HOST, "localhost")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let body = String::from_utf8(body.to_vec()).expect("utf8 body");
        assert!(body.contains("LLM gateway key not found"));
    }

    #[tokio::test]
    async fn router_serves_admin_llm_gateway_account_groups_for_local_request() {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .uri("/admin/llm-gateway/account-groups")
                    .header(header::HOST, "localhost")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(value["groups"].as_array().expect("groups array").len(), 0);
    }

    #[tokio::test]
    async fn router_creates_admin_llm_gateway_account_group_for_local_request() {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/llm-gateway/account-groups")
                    .header(header::HOST, "localhost")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"name":"pool","account_names":["beta","alpha","alpha"]}"#))
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("json body");
        assert!(value["id"].as_str().expect("id").starts_with("llm-group-"));
        assert_eq!(value["provider_type"], "codex");
        assert_eq!(value["account_names"], serde_json::json!(["alpha", "beta"]));
    }

    #[tokio::test]
    async fn router_serves_admin_llm_gateway_proxy_configs_for_local_request() {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .uri("/admin/llm-gateway/proxy-configs")
                    .header(header::HOST, "localhost")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(
            value["proxy_configs"]
                .as_array()
                .expect("proxy configs array")
                .len(),
            0
        );
    }

    #[tokio::test]
    async fn router_creates_admin_llm_gateway_proxy_config_for_local_request() {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/llm-gateway/proxy-configs")
                    .header(header::HOST, "localhost")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"name":"hk","proxy_url":"http://127.0.0.1:11111","proxy_username":" u ","proxy_password":" p "}"#,
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("json body");
        assert!(value["id"].as_str().expect("id").starts_with("llm-proxy-"));
        assert_eq!(value["name"], "hk");
        assert_eq!(value["proxy_url"], "http://127.0.0.1:11111");
        assert_eq!(value["proxy_username"], "u");
        assert_eq!(value["proxy_password"], "p");
        assert_eq!(value["status"], "active");
    }

    #[tokio::test]
    async fn router_serves_admin_llm_gateway_proxy_bindings_for_local_request() {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .uri("/admin/llm-gateway/proxy-bindings")
                    .header(header::HOST, "localhost")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("json body");
        let bindings = value["bindings"].as_array().expect("bindings array");
        assert!(bindings
            .iter()
            .any(|binding| binding["provider_type"] == "codex"));
        assert!(bindings
            .iter()
            .any(|binding| binding["provider_type"] == "kiro"));
    }

    #[tokio::test]
    async fn router_serves_admin_llm_gateway_accounts_for_local_request() {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .uri("/admin/llm-gateway/accounts")
                    .header(header::HOST, "localhost")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(value["accounts"].as_array().expect("accounts array").len(), 0);
        assert_eq!(value["total"], 0);
        assert_eq!(value["limit"], 50);
        assert_eq!(value["offset"], 0);
        assert_eq!(value["has_more"], false);
    }

    #[tokio::test]
    async fn router_routes_admin_proxy_check_to_store() {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/llm-gateway/proxy-configs/missing/check/codex")
                    .header(header::HOST, "localhost")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let body = String::from_utf8(body.to_vec()).expect("utf8 body");
        assert!(body.contains("LLM gateway proxy config not found"));
    }

    #[tokio::test]
    async fn router_imports_admin_llm_gateway_account_for_local_request() {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/llm-gateway/accounts")
                    .header(header::HOST, "localhost")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{
                            "name": "codex_primary",
                            "tokens": {
                                "id_token": "id",
                                "access_token": "access",
                                "refresh_token": "refresh",
                                "account_id": "acct-1"
                            }
                        }"#,
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(value["name"], "codex_primary");
        assert_eq!(value["status"], "active");
        assert_eq!(value["account_id"], "acct-1");
        assert_eq!(value["proxy_mode"], "inherit");
    }

    #[tokio::test]
    async fn router_serves_admin_llm_gateway_review_queues_for_local_request() {
        for (path, field) in [
            ("/admin/llm-gateway/token-requests", "requests"),
            ("/admin/llm-gateway/account-contribution-requests", "requests"),
            ("/admin/llm-gateway/sponsor-requests", "requests"),
        ] {
            let response = test_router()
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .header(header::HOST, "localhost")
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("response");

            assert_eq!(response.status(), StatusCode::OK, "{path}");
            let body = to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body");
            let value: serde_json::Value = serde_json::from_slice(&body).expect("json body");
            assert_eq!(value["total"], 0, "{path}");
            assert_eq!(value[field].as_array().expect("requests array").len(), 0);
        }
    }

    #[tokio::test]
    async fn router_routes_admin_llm_gateway_review_queue_actions_for_local_request() {
        for path in [
            "/admin/llm-gateway/token-requests/missing/approve-and-issue",
            "/admin/llm-gateway/token-requests/missing/reject",
            "/admin/llm-gateway/account-contribution-requests/missing/approve-and-issue",
            "/admin/llm-gateway/account-contribution-requests/missing/reject",
            "/admin/llm-gateway/sponsor-requests/missing/approve",
        ] {
            let response = test_router()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(path)
                        .header(header::HOST, "localhost")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(r#"{"admin_note":"checked"}"#))
                        .expect("request"),
                )
                .await
                .expect("response");

            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
            let body = to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body");
            let value: serde_json::Value = serde_json::from_slice(&body).expect("json body");
            assert_eq!(value["code"], 404, "{path}");
        }

        let response = test_router()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/admin/llm-gateway/sponsor-requests/missing")
                    .header(header::HOST, "localhost")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(value["code"], 404);
    }

    #[tokio::test]
    async fn router_accepts_llm_gateway_account_contribution_without_provider_key() {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/llm-gateway/account-contribution-requests/submit")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header("x-real-ip", "198.51.100.11")
                    .body(Body::from(
                        r#"{
                            "account_name": "contributed_account",
                            "account_id": "acct-1",
                            "id_token": "id-token",
                            "access_token": "access-token",
                            "refresh_token": "refresh-token",
                            "requester_email": "user@example.com",
                            "contributor_message": "shared for testing",
                            "github_id": "acking-you",
                            "frontend_page_url": "https://example.test/llm-access"
                        }"#,
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let body = String::from_utf8(body.to_vec()).expect("utf8 body");
        assert!(body.contains(r#""request_id":"llmacct-"#));
        assert!(body.contains(r#""status":"pending""#));
    }

    #[tokio::test]
    async fn router_accepts_llm_gateway_account_contribution_batch_without_provider_key() {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/llm-gateway/account-contribution-requests/batch-submit")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header("x-real-ip", "198.51.100.15")
                    .body(Body::from(
                        r#"{
                            "requester_email": "user@example.com",
                            "contributor_message": "shared for testing",
                            "github_id": "acking-you",
                            "frontend_page_url": "https://example.test/llm-access",
                            "items": [
                                {
                                    "account_name": "contributed_account_a",
                                    "auth_json": {
                                        "account_id": "acct-1",
                                        "id_token": "id-token-a",
                                        "access_token": "access-token-a",
                                        "refresh_token": "refresh-token-a"
                                    }
                                },
                                {
                                    "account_name": "contributed_account_b",
                                    "tokens": {
                                        "refresh_token": "refresh-token-b"
                                    }
                                }
                            ]
                        }"#,
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(value["total"], 2);
        assert_eq!(value["created_count"], 2);
        assert_eq!(value["invalid_count"], 0);
        assert_eq!(value["conflict_count"], 0);
        assert_eq!(value["results"][0]["status"], "pending");
        assert_eq!(value["results"][1]["status"], "pending");
        assert!(value["results"][0]["request_id"]
            .as_str()
            .is_some_and(|value| value.starts_with("llmacct-")));
    }

    #[tokio::test]
    async fn router_reports_duplicate_names_in_public_account_contribution_batch() {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/llm-gateway/account-contribution-requests/batch-submit")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header("x-real-ip", "198.51.100.16")
                    .body(Body::from(
                        r#"{
                            "contributor_message": "shared for testing",
                            "items": [
                                {
                                    "account_name": "duplicate_account",
                                    "tokens": {
                                        "refresh_token": "refresh-token-a"
                                    }
                                },
                                {
                                    "account_name": "duplicate_account",
                                    "tokens": {
                                        "refresh_token": "refresh-token-b"
                                    }
                                }
                            ]
                        }"#,
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(value["total"], 2);
        assert_eq!(value["created_count"], 1);
        assert_eq!(value["invalid_count"], 0);
        assert_eq!(value["conflict_count"], 1);
        assert_eq!(value["results"][0]["status"], "pending");
        assert_eq!(value["results"][1]["status"], "conflict");
    }

    #[tokio::test]
    async fn router_accepts_llm_gateway_sponsor_request_without_provider_key() {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/llm-gateway/sponsor-requests/submit")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header("x-real-ip", "198.51.100.12")
                    .body(Body::from(
                        r#"{
                            "requester_email": "user@example.com",
                            "sponsor_message": "thanks",
                            "display_name": "Example Sponsor",
                            "github_id": "acking-you",
                            "frontend_page_url": "https://example.test/llm-access"
                        }"#,
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let body = String::from_utf8(body.to_vec()).expect("utf8 body");
        assert!(body.contains(r#""request_id":"llmsponsor-"#));
        assert!(body.contains(r#""status":"submitted""#));
        assert!(body.contains(r#""payment_email_sent":false"#));
    }
}
