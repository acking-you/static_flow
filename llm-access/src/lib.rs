//! Standalone HTTP service shell for LLM access.

mod activity;
mod admin;
mod codex_refresh;
mod codex_status;
/// Command-line and environment configuration.
pub mod config;
mod email;
mod geoip;
/// Local Kiro endpoints.
pub mod kiro;
mod kiro_headers;
mod kiro_refresh;
mod kiro_status;
mod process_memory;
/// Provider request entrypoints.
pub mod provider;
mod public;
mod request_context;
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

use std::{path::PathBuf, sync::Arc};

use anyhow::Context;
use axum::{
    body::Body,
    extract::State,
    http::{HeaderValue, Request},
    middleware,
    response::Response,
    routing::{any, get, post},
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
    llm_access_store::initialize_sqlite_target_path(&config.sqlite_control)?;
    Ok(())
}

/// Initialize llm-access storage paths, including analytics storage.
pub fn bootstrap_storage(config: &StorageConfig) -> anyhow::Result<()> {
    bootstrap_api_storage(config)?;
    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    if let Some(tiered) = &config.duckdb_tiered {
        llm_access_store::duckdb::DuckDbUsageRepository::open_tiered(
            llm_access_store::duckdb::TieredDuckDbUsageConfig {
                active_dir: tiered.active_dir.clone(),
                archive_dir: tiered.archive_dir.clone(),
                catalog_dir: tiered.catalog_dir.clone(),
                rollover_bytes: tiered.rollover_bytes,
            },
        )?;
    } else {
        llm_access_store::initialize_duckdb_target_path(&config.duckdb)?;
    }
    llm_access_store::write_duckdb_schema_file(duckdb_schema_output_path(config))?;
    Ok(())
}

fn duckdb_schema_output_path(config: &StorageConfig) -> PathBuf {
    if let Some(tiered) = &config.duckdb_tiered {
        return tiered.catalog_dir.join("usage.schema.sql");
    }
    config.duckdb.with_extension("schema.sql")
}

/// Build the HTTP router.
pub fn router(runtime: runtime::LlmAccessRuntime) -> Router {
    let request_activity = Arc::new(activity::RequestActivityTracker::new());
    let geoip = runtime.geoip();
    let provider_state = provider::ProviderState::new_with_config_store_and_activity(
        runtime.control_store(),
        runtime.provider_route_store(),
        runtime.admin_config_store(),
        Arc::clone(&request_activity),
        geoip.clone(),
    );
    let state = HttpState {
        provider_state,
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
    Router::new()
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
        .route("/admin/llm-gateway/usage", get(admin::list_llm_gateway_usage_events))
        .route("/admin/llm-gateway/usage/:event_id", get(admin::get_llm_gateway_usage_event))
        .route("/admin/llm-access/usage-journal/status", get(admin::get_usage_journal_status))
        .route("/admin/llm-gateway/usage-journal/status", get(admin::get_usage_journal_status))
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
        .route("/api/llm-gateway/access", get(public::get_llm_gateway_access))
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
        .with_state(state)
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
    codex_status::spawn_codex_status_refresher(&service_runtime);
    kiro_status::spawn_kiro_status_refresher(&service_runtime);
    let shutdown_runtime = service_runtime.clone();
    let listener = tokio::net::TcpListener::bind(config.bind_addr)
        .await
        .with_context(|| format!("failed to bind {}", config.bind_addr))?;
    let result = axum::serve(
        listener,
        router(service_runtime).into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .context("llm-access server failed");
    shutdown_runtime.shutdown_usage_events().await;
    result
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
    use std::{
        path::PathBuf,
        sync::{Arc, Mutex},
    };

    use async_trait::async_trait;
    use axum::{
        body::{to_bytes, Body},
        extract::Path,
        http::{header, Request, StatusCode},
        routing::get,
        Json, Router,
    };
    use llm_access_core::store::{
        AdminCodexAccountStore, AdminKiroAccountStore, AdminReviewQueueAction,
        AdminReviewQueueStore, AuthenticatedKey, CodexPublicAccountStatus, CodexRateLimitStatus,
        ControlStore, NewAdminCodexAccount, NewAdminKiroAccount,
        NewPublicAccountContributionRequest, PublicStatusStore, PublicSubmissionStore,
    };
    use serde_json::json;
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

    async fn persistent_test_router(name: &str) -> (axum::Router, PathBuf) {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let root = std::env::temp_dir()
            .join(format!("llm-access-router-{name}-{}-{unique}", std::process::id()));
        std::fs::create_dir_all(&root).expect("create state root");
        let config = crate::config::StorageConfig {
            state_root: root.clone(),
            sqlite_control: root.join("control/llm-access.sqlite3"),
            duckdb: root.join("analytics/usage.duckdb"),
            usage_journal_dir: root.join("usage-journal"),
            duckdb_tiered: None,
            kiro_auths_dir: root.join("auths/kiro"),
            codex_auths_dir: root.join("auths/codex"),
            logs_dir: root.join("logs"),
        };
        crate::bootstrap_api_storage(&config).expect("bootstrap storage");
        let runtime = crate::runtime::LlmAccessRuntime::from_storage_config(&config)
            .await
            .expect("open runtime");
        (super::router(runtime), root)
    }

    async fn wait_for_codex_batch_import_job_terminal_state(
        router: axum::Router,
        job_id: &str,
    ) -> serde_json::Value {
        let started = std::time::Instant::now();
        loop {
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/admin/llm-gateway/accounts/import-jobs/{job_id}"))
                        .header(header::HOST, "localhost")
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("detail response");
            let body = to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("detail body");
            let value: serde_json::Value = serde_json::from_slice(&body).expect("detail json");
            let status = value["summary"]["status"].as_str().unwrap_or_default();
            if status == "completed" || status == "failed" {
                return value;
            }
            assert!(started.elapsed() < std::time::Duration::from_secs(2));
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }

    async fn fake_kiro_usage_limits() -> Json<serde_json::Value> {
        Json(json!({
            "subscriptionInfo": {"subscriptionTitle": "Pro"},
            "userInfo": {"userId": "kiro-import-user"},
            "usageBreakdownList": [{
                "currentUsageWithPrecision": 7.0,
                "usageLimitWithPrecision": 100.0,
                "nextDateReset": 1777777777000_i64
            }]
        }))
    }

    async fn spawn_fake_kiro_usage_upstream() -> String {
        let app = Router::new().route("/getUsageLimits", get(fake_kiro_usage_limits));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fake Kiro upstream");
        let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve fake Kiro upstream");
        });
        upstream_base
    }

    async fn fake_codex_usage_status() -> Json<serde_json::Value> {
        Json(json!({
            "plan_type": "pro",
            "rate_limit": {
                "primary_window": {
                    "used_percent": 25.0,
                    "limit_window_seconds": 18_000,
                    "reset_at": 1_777_777_777_i64
                },
                "secondary_window": {
                    "used_percent": 12.0,
                    "limit_window_seconds": 604_800,
                    "reset_at": 1_778_888_888_i64
                }
            }
        }))
    }

    async fn fake_codex_usage_status_failure() -> (StatusCode, Json<serde_json::Value>) {
        (
            StatusCode::BAD_GATEWAY,
            Json(json!({
                "error": "upstream unavailable"
            })),
        )
    }

    async fn spawn_fake_codex_usage_upstream(success: bool) -> String {
        let app = if success {
            Router::new()
                .route("/api/codex/usage", get(fake_codex_usage_status))
                .route("/backend-api/wham/usage", get(fake_codex_usage_status))
        } else {
            Router::new()
                .route("/api/codex/usage", get(fake_codex_usage_status_failure))
                .route("/backend-api/wham/usage", get(fake_codex_usage_status_failure))
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fake Codex upstream");
        let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve fake Codex upstream");
        });
        upstream_base
    }

    async fn seed_validated_account_contribution(
        root: &std::path::Path,
        request_id: &str,
        account_name: &str,
    ) {
        let repo = llm_access_store::repository::SqliteControlRepository::open_path(
            root.join("control/llm-access.sqlite3"),
        )
        .expect("open sqlite control repository");
        repo.create_public_account_contribution_request(NewPublicAccountContributionRequest {
            request_id: request_id.to_string(),
            account_name: account_name.to_string(),
            account_id: Some("acct-prime".to_string()),
            id_token: "id-seed".to_string(),
            access_token: "access-seed".to_string(),
            refresh_token: "refresh-seed".to_string(),
            requester_email: String::new(),
            contributor_message: "shared account".to_string(),
            github_id: None,
            frontend_page_url: None,
            show_on_public_wall: true,
            fingerprint: format!("fingerprint-{request_id}"),
            client_ip: "198.51.100.31".to_string(),
            ip_region: "unknown".to_string(),
            created_at_ms: 10,
        })
        .await
        .expect("create contribution request");
        repo.validate_admin_account_contribution_request(
            request_id,
            Some("acct-prime".to_string()),
            "id-valid".to_string(),
            "access-valid".to_string(),
            "refresh-valid".to_string(),
            AdminReviewQueueAction {
                admin_note: Some("validated".to_string()),
                updated_at_ms: 20,
            },
        )
        .await
        .expect("validate contribution request")
        .expect("validated contribution request exists");
    }

    async fn fake_usage_list() -> Json<serde_json::Value> {
        Json(json!({
            "total": 1,
            "offset": 0,
            "limit": 20,
            "has_more": false,
            "current_rpm": 0,
            "current_in_flight": 0,
            "events": [{
                "id": "evt-proxied",
                "key_id": "key-1",
                "key_name": "for-yangshu",
                "request_method": "POST",
                "request_url": "/v1/chat/completions",
                "latency_ms": 12,
                "quota_failover_count": 0,
                "endpoint": "/v1/chat/completions",
                "model": "claude-opus-4-7",
                "status_code": 200,
                "input_uncached_tokens": 1,
                "input_cached_tokens": 2,
                "output_tokens": 3,
                "billable_tokens": 4,
                "usage_missing": false,
                "credit_usage_missing": false,
                "client_ip": "127.0.0.1",
                "ip_region": "local",
                "created_at": 1_700_000_000_000_i64
            }],
            "generated_at": 1_700_000_000_000_i64
        }))
    }

    async fn fake_usage_detail(Path(event_id): Path<String>) -> Json<serde_json::Value> {
        Json(json!({
            "id": event_id,
            "key_id": "key-1",
            "key_name": "for-yangshu",
            "request_method": "POST",
            "request_url": "/v1/chat/completions",
            "latency_ms": 12,
            "quota_failover_count": 0,
            "endpoint": "/v1/chat/completions",
            "model": "claude-opus-4-7",
            "status_code": 200,
            "input_uncached_tokens": 1,
            "input_cached_tokens": 2,
            "output_tokens": 3,
            "billable_tokens": 4,
            "usage_missing": false,
            "credit_usage_missing": false,
            "client_ip": "127.0.0.1",
            "ip_region": "local",
            "created_at": 1_700_000_000_000_i64,
            "request_headers_json": "{}"
        }))
    }

    async fn fake_usage_worker_status() -> Json<serde_json::Value> {
        Json(json!({
            "state": "idle",
            "current_file_path": null,
            "current_file_sequence": null,
            "processed_blocks": 0,
            "total_blocks": 0,
            "processed_events": 0,
            "total_events": 0,
            "processed_compressed_bytes": 0,
            "total_compressed_bytes": 0,
            "progress_percent": 0.0,
            "import_rate_events_per_second": 0.0,
            "heartbeat_at_ms": 1_700_000_000_000_i64,
            "last_successful_file_sequence": null,
            "last_successful_import_at_ms": null,
            "last_error": null,
            "last_error_at_ms": null
        }))
    }

    async fn fake_usage_chart() -> Json<serde_json::Value> {
        Json(json!({
            "chart_points": [{
                "bucket_start_ms": 1_700_000_000_000_i64,
                "tokens": 42_u64
            }]
        }))
    }

    async fn spawn_fake_usage_worker() -> String {
        let app = Router::new()
            .route("/admin/llm-gateway/usage", get(fake_usage_list))
            .route("/admin/llm-gateway/usage/:event_id", get(fake_usage_detail))
            .route("/admin/kiro-gateway/usage", get(fake_usage_list))
            .route("/admin/kiro-gateway/usage/:event_id", get(fake_usage_detail))
            .route("/admin/llm-access/usage/chart", get(fake_usage_chart))
            .route("/admin/llm-access/usage-worker/status", get(fake_usage_worker_status));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fake usage worker");
        let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve fake usage worker");
        });
        upstream_base
    }

    async fn set_usage_query_base_url(router: &axum::Router, base_url: &str) {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/llm-gateway/config")
                    .header(header::HOST, "localhost")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(json!({ "usage_query_base_url": base_url }).to_string()))
                    .expect("request"),
            )
            .await
            .expect("config response");

        assert_eq!(response.status(), StatusCode::OK);
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

    #[tokio::test]
    async fn public_usage_query_reads_settled_events_from_usage_worker() {
        let usage_worker_base = spawn_fake_usage_worker().await;
        let (router, root) = persistent_test_router("public-usage-worker").await;
        set_usage_query_base_url(&router, &usage_worker_base).await;

        let create_response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/llm-gateway/keys")
                    .header(header::HOST, "localhost")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{
                            "name": "public usage test",
                            "quota_billable_limit": 1000,
                            "public_visible": true
                        }"#,
                    ))
                    .expect("request"),
            )
            .await
            .expect("create key response");
        assert_eq!(create_response.status(), StatusCode::OK);
        let body = to_bytes(create_response.into_body(), usize::MAX)
            .await
            .expect("create key body");
        let created: serde_json::Value = serde_json::from_slice(&body).expect("create key json");
        let secret = created["secret"].as_str().expect("secret");

        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/llm-gateway/public-usage/query")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(json!({ "api_key": secret }).to_string()))
                    .expect("request"),
            )
            .await
            .expect("response");

        std::fs::remove_dir_all(&root).expect("cleanup state root");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(value["events"][0]["id"], "evt-proxied");
        assert_eq!(value["chart_points"][0]["tokens"], 42);
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
    async fn router_primes_kiro_status_after_admin_account_create() {
        let _guard = crate::KIRO_UPSTREAM_ENV_LOCK
            .lock()
            .expect("Kiro upstream env lock");
        let upstream_base = spawn_fake_kiro_usage_upstream().await;
        std::env::set_var("KIRO_UPSTREAM_BASE_URL", upstream_base);
        let (router, root) = persistent_test_router("kiro-status-prime").await;
        let refresh_token = "r".repeat(96);
        let create_body = json!({
            "name": "imported-kiro",
            "access_token": "kiro-access-token",
            "refresh_token": refresh_token,
            "expires_at": "2035-01-01T00:00:00Z",
            "auth_method": "social",
            "profile_arn": "arn:aws:iam::123456789012:role/KiroProfile"
        });

        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/kiro-gateway/accounts")
                    .header(header::HOST, "localhost")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(create_body.to_string()))
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/admin/kiro-gateway/accounts/imported-kiro/balance")
                    .header(header::HOST, "localhost")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("balance response");

        std::env::remove_var("KIRO_UPSTREAM_BASE_URL");
        std::fs::remove_dir_all(&root).expect("cleanup state root");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(value["user_id"], "kiro-import-user");
        assert_eq!(value["remaining"], 93.0);
    }

    #[tokio::test]
    async fn router_primes_codex_status_after_account_contribution_issue() {
        let _guard = crate::CODEX_UPSTREAM_ENV_LOCK
            .lock()
            .expect("Codex upstream env lock");
        let upstream_base = spawn_fake_codex_usage_upstream(true).await;
        std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);
        let (router, root) = persistent_test_router("codex-status-prime-after-issue").await;
        seed_validated_account_contribution(&root, "llmacct-prime", "codex-prime").await;

        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(
                        "/admin/llm-gateway/account-contribution-requests/llmacct-prime/\
                         approve-and-issue",
                    )
                    .header(header::HOST, "localhost")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"admin_note":"issued"}"#))
                    .expect("request"),
            )
            .await
            .expect("issue response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("issue body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("issue json");
        assert_eq!(value["status"], "issued");
        assert_eq!(value["imported_account_name"], "codex-prime");

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/admin/llm-gateway/accounts")
                    .header(header::HOST, "localhost")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("accounts response");

        std::env::remove_var("CODEX_UPSTREAM_BASE_URL");
        std::fs::remove_dir_all(&root).expect("cleanup state root");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("accounts body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("accounts json");
        let account = value["accounts"]
            .as_array()
            .expect("accounts array")
            .iter()
            .find(|item| item["name"] == "codex-prime")
            .expect("issued account present");
        assert_eq!(account["plan_type"], "Pro");
        assert_eq!(account["primary_remaining_percent"], 75.0);
        assert_eq!(account["secondary_remaining_percent"], 88.0);
        assert!(account["usage_error_message"].is_null());
    }

    #[tokio::test]
    async fn router_keeps_issue_success_when_post_issue_codex_status_refresh_fails() {
        let _guard = crate::CODEX_UPSTREAM_ENV_LOCK
            .lock()
            .expect("Codex upstream env lock");
        let upstream_base = spawn_fake_codex_usage_upstream(false).await;
        std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);
        let (router, root) = persistent_test_router("codex-status-prime-after-issue-failure").await;
        seed_validated_account_contribution(&root, "llmacct-prime-fail", "codex-prime-fail").await;

        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(
                        "/admin/llm-gateway/account-contribution-requests/llmacct-prime-fail/\
                         approve-and-issue",
                    )
                    .header(header::HOST, "localhost")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"admin_note":"issued"}"#))
                    .expect("request"),
            )
            .await
            .expect("issue response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("issue body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("issue json");
        assert_eq!(value["status"], "issued");
        assert_eq!(value["imported_account_name"], "codex-prime-fail");

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/admin/llm-gateway/accounts")
                    .header(header::HOST, "localhost")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("accounts response");

        std::env::remove_var("CODEX_UPSTREAM_BASE_URL");
        std::fs::remove_dir_all(&root).expect("cleanup state root");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("accounts body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("accounts json");
        let account = value["accounts"]
            .as_array()
            .expect("accounts array")
            .iter()
            .find(|item| item["name"] == "codex-prime-fail")
            .expect("issued account present");
        assert!(account["plan_type"].is_null());
        assert!(account["primary_remaining_percent"].is_null());
        assert!(account["secondary_remaining_percent"].is_null());
        assert!(account["usage_error_message"]
            .as_str()
            .expect("usage error message")
            .contains("Codex usage status returned 502 Bad Gateway"));
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
                            "kiro_prefix_cache_mode": "formula"
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
    async fn router_rejects_codex_patch_for_persisted_kiro_key() {
        let (router, root) = persistent_test_router("codex-patch-kiro-key").await;

        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/kiro-gateway/keys")
                    .header(header::HOST, "localhost")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{
                            "name": "external kiro",
                            "quota_billable_limit": 1000
                        }"#,
                    ))
                    .expect("request"),
            )
            .await
            .expect("create response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("json body");
        let key_id = value["id"].as_str().expect("id").to_string();
        assert_eq!(value["provider_type"], "kiro");

        let response = router
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/admin/llm-gateway/keys/{key_id}"))
                    .header(header::HOST, "localhost")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"name":"wrong surface"}"#))
                    .expect("request"),
            )
            .await
            .expect("patch response");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let body = String::from_utf8(body.to_vec()).expect("utf8 body");
        assert!(body.contains("Kiro keys must be managed from /admin/kiro-gateway"));

        std::fs::remove_dir_all(&root).expect("cleanup state root");
    }

    #[tokio::test]
    async fn router_kiro_key_create_and_patch_ignore_codex_only_fields() {
        let (router, root) = persistent_test_router("kiro-key-codex-fields").await;

        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/kiro-gateway/keys")
                    .header(header::HOST, "localhost")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{
                            "name": "kiro api",
                            "quota_billable_limit": 1000,
                            "public_visible": true,
                            "request_max_concurrency": 9,
                            "request_min_start_interval_ms": 10
                        }"#,
                    ))
                    .expect("request"),
            )
            .await
            .expect("create response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("json body");
        let key_id = value["id"].as_str().expect("id").to_string();
        assert_eq!(value["provider_type"], "kiro");
        assert_eq!(value["public_visible"], false);
        assert!(value["request_max_concurrency"].is_null());
        assert!(value["request_min_start_interval_ms"].is_null());

        let response = router
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/admin/kiro-gateway/keys/{key_id}"))
                    .header(header::HOST, "localhost")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{
                            "name": "kiro api patched",
                            "public_visible": true,
                            "request_max_concurrency": 7,
                            "request_min_start_interval_ms": 30
                        }"#,
                    ))
                    .expect("request"),
            )
            .await
            .expect("patch response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(value["name"], "kiro api patched");
        assert_eq!(value["public_visible"], false);
        assert!(value["request_max_concurrency"].is_null());
        assert!(value["request_min_start_interval_ms"].is_null());

        std::fs::remove_dir_all(&root).expect("cleanup state root");
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
    }

    #[tokio::test]
    async fn router_serves_admin_llm_gateway_usage_for_local_request() {
        let usage_worker_base = spawn_fake_usage_worker().await;
        let (router, root) = persistent_test_router("usage-proxy").await;
        set_usage_query_base_url(&router, &usage_worker_base).await;

        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/admin/llm-gateway/usage")
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
        assert_eq!(value["total"], 1);
        assert_eq!(value["events"][0]["id"], "evt-proxied");

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/admin/llm-gateway/usage/evt-proxied")
                    .header(header::HOST, "localhost")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("detail response");

        std::fs::remove_dir_all(&root).expect("cleanup state root");

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn api_proxy_returns_503_when_usage_worker_is_unreachable() {
        let (router, root) = persistent_test_router("usage-proxy-unreachable").await;
        set_usage_query_base_url(&router, "http://127.0.0.1:9").await;

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/admin/llm-gateway/usage")
                    .header(header::HOST, "localhost")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        std::fs::remove_dir_all(&root).expect("cleanup state root");

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn combined_journal_status_returns_producer_state_when_worker_is_unreachable() {
        let (router, root) = persistent_test_router("usage-journal-status").await;
        set_usage_query_base_url(&router, "http://127.0.0.1:9").await;

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/admin/llm-access/usage-journal/status")
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
        assert_eq!(value["journal_enabled"], true);
        assert!(value["journal_root"]
            .as_str()
            .unwrap_or_default()
            .ends_with("usage-journal"));
        assert_eq!(value["worker"]["state"], "unreachable");

        std::fs::remove_dir_all(&root).expect("cleanup state root");
    }

    #[tokio::test]
    async fn combined_journal_status_includes_file_level_backlog_lists() {
        let usage_worker_base = spawn_fake_usage_worker().await;
        let (router, root) = persistent_test_router("usage-journal-file-lists").await;
        set_usage_query_base_url(&router, &usage_worker_base).await;

        let active_dir = root.join("usage-journal/active");
        let sealed_dir = root.join("usage-journal/sealed");
        let consuming_dir = root.join("usage-journal/consuming");
        let bad_dir = root.join("usage-journal/bad");
        std::fs::create_dir_all(&active_dir).expect("active dir");
        std::fs::create_dir_all(&sealed_dir).expect("sealed dir");
        std::fs::create_dir_all(&consuming_dir).expect("consuming dir");
        std::fs::create_dir_all(&bad_dir).expect("bad dir");
        std::fs::write(active_dir.join("usage-000000000001.open"), b"active").expect("active file");
        std::fs::write(sealed_dir.join("usage-000000000002.journal"), b"sealed")
            .expect("sealed file");
        std::fs::write(consuming_dir.join("usage-000000000003.journal"), b"consuming")
            .expect("consuming file");
        std::fs::write(bad_dir.join("usage-000000000004.journal"), b"bad").expect("bad file");

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/admin/llm-access/usage-journal/status")
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
        assert_eq!(value["sealed_files"].as_array().map(Vec::len), Some(1));
        assert_eq!(value["consuming_files"].as_array().map(Vec::len), Some(1));
        assert_eq!(value["bad_files"].as_array().map(Vec::len), Some(1));
        assert_eq!(value["sealed_files"][0]["sequence"].as_u64(), Some(2));
        assert_eq!(value["consuming_files"][0]["sequence"].as_u64(), Some(3));
        assert_eq!(value["bad_files"][0]["sequence"].as_u64(), Some(4));

        std::fs::remove_dir_all(&root).expect("cleanup state root");
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
    async fn router_patches_admin_llm_gateway_account_status_and_hides_disabled_route() {
        let (router, root) = persistent_test_router("codex-account-disable").await;
        let repo = llm_access_store::repository::SqliteControlRepository::open_path(
            root.join("control/llm-access.sqlite3"),
        )
        .expect("open sqlite control repository");
        repo.create_admin_codex_account(NewAdminCodexAccount {
            name: "codex_primary".to_string(),
            account_id: Some("acct-1".to_string()),
            auth_json: serde_json::json!({
                "id_token": "id",
                "access_token": "access",
                "refresh_token": "refresh",
                "account_id": "acct-1",
            })
            .to_string(),
            map_gpt53_codex_to_spark: false,
            created_at_ms: 100,
        })
        .await
        .expect("create codex account");
        repo.save_codex_rate_limit_status(CodexRateLimitStatus {
            status: "ready".to_string(),
            refresh_interval_seconds: 300,
            last_checked_at: Some(200),
            last_success_at: Some(200),
            source_url: "https://chatgpt.com/backend-api/wham/usage".to_string(),
            error_message: None,
            accounts: vec![CodexPublicAccountStatus {
                name: "codex_primary".to_string(),
                status: "active".to_string(),
                plan_type: Some("Plus".to_string()),
                primary_remaining_percent: Some(91.0),
                secondary_remaining_percent: Some(95.0),
                last_usage_checked_at: Some(200),
                last_usage_success_at: Some(200),
                usage_error_message: None,
            }],
            buckets: Vec::new(),
        })
        .await
        .expect("seed cached codex status");

        let disable_response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/admin/llm-gateway/accounts/codex_primary")
                    .header(header::HOST, "localhost")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"status":"disabled"}"#))
                    .expect("request"),
            )
            .await
            .expect("disable response");
        assert_eq!(disable_response.status(), StatusCode::OK);
        let disable_body = to_bytes(disable_response.into_body(), usize::MAX)
            .await
            .expect("disable body");
        let disable_value: serde_json::Value =
            serde_json::from_slice(&disable_body).expect("disable json");
        assert_eq!(disable_value["status"], "disabled");

        let list_response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/admin/llm-gateway/accounts")
                    .header(header::HOST, "localhost")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("accounts response");
        assert_eq!(list_response.status(), StatusCode::OK);
        let list_body = to_bytes(list_response.into_body(), usize::MAX)
            .await
            .expect("accounts body");
        let list_value: serde_json::Value =
            serde_json::from_slice(&list_body).expect("accounts json");
        let account = list_value["accounts"]
            .as_array()
            .expect("accounts array")
            .iter()
            .find(|item| item["name"] == "codex_primary")
            .expect("account present");
        assert_eq!(account["status"], "disabled");
        assert!(repo
            .resolve_admin_codex_account_route("codex_primary")
            .await
            .expect("resolve disabled codex route")
            .is_none());

        let enable_response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/admin/llm-gateway/accounts/codex_primary")
                    .header(header::HOST, "localhost")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"status":"active"}"#))
                    .expect("request"),
            )
            .await
            .expect("enable response");
        assert_eq!(enable_response.status(), StatusCode::OK);
        assert!(repo
            .resolve_admin_codex_account_route("codex_primary")
            .await
            .expect("resolve enabled codex route")
            .is_some());

        std::fs::remove_dir_all(&root).expect("cleanup state root");
    }

    #[tokio::test]
    async fn router_patches_admin_kiro_account_status_and_hides_disabled_route() {
        let (router, root) = persistent_test_router("kiro-account-disable").await;
        let repo = llm_access_store::repository::SqliteControlRepository::open_path(
            root.join("control/llm-access.sqlite3"),
        )
        .expect("open sqlite control repository");
        repo.create_admin_kiro_account(NewAdminKiroAccount {
            name: "kiro_primary".to_string(),
            auth_method: "social".to_string(),
            account_id: Some("kiro-account".to_string()),
            profile_arn: Some("arn:aws:iam::123456789012:role/KiroProfile".to_string()),
            user_id: Some("kiro-user".to_string()),
            status: "active".to_string(),
            auth_json: serde_json::json!({
                "accessToken": "kiro-access-token",
                "refreshToken": "r".repeat(96),
                "expiresAt": "2035-01-01T00:00:00Z",
                "profileArn": "arn:aws:iam::123456789012:role/KiroProfile",
                "apiRegion": "us-east-1",
            })
            .to_string(),
            max_concurrency: Some(1),
            min_start_interval_ms: Some(100),
            proxy_config_id: None,
            created_at_ms: 100,
        })
        .await
        .expect("create kiro account");

        let disable_response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/admin/kiro-gateway/accounts/kiro_primary")
                    .header(header::HOST, "localhost")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"status":"disabled"}"#))
                    .expect("request"),
            )
            .await
            .expect("disable response");
        assert_eq!(disable_response.status(), StatusCode::OK);
        let disable_body = to_bytes(disable_response.into_body(), usize::MAX)
            .await
            .expect("disable body");
        let disable_value: serde_json::Value =
            serde_json::from_slice(&disable_body).expect("disable json");
        assert_eq!(disable_value["disabled"], true);
        assert!(repo
            .resolve_admin_kiro_account_route("kiro_primary")
            .await
            .expect("resolve disabled kiro route")
            .is_none());

        let enable_response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/admin/kiro-gateway/accounts/kiro_primary")
                    .header(header::HOST, "localhost")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"status":"active"}"#))
                    .expect("request"),
            )
            .await
            .expect("enable response");
        assert_eq!(enable_response.status(), StatusCode::OK);
        assert!(repo
            .resolve_admin_kiro_account_route("kiro_primary")
            .await
            .expect("resolve enabled kiro route")
            .is_some());

        std::fs::remove_dir_all(&root).expect("cleanup state root");
    }

    #[tokio::test]
    async fn router_creates_and_completes_codex_batch_import_job() {
        let (router, root) = persistent_test_router("codex-batch-import-success").await;

        let create_response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/llm-gateway/accounts/import-jobs")
                    .header(header::HOST, "localhost")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{
                            "provider_type": "codex",
                            "source_type": "local_json",
                            "validate_before_import": false,
                            "items": [{
                                "name": "codex_primary",
                                "auth_json": {
                                    "refresh_token": "rt-1",
                                    "account_id": "acct-1"
                                }
                            }]
                        }"#,
                    ))
                    .expect("request"),
            )
            .await
            .expect("create response");
        assert_eq!(create_response.status(), StatusCode::OK);
        let body = to_bytes(create_response.into_body(), usize::MAX)
            .await
            .expect("create body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("create json");
        let job_id = value["summary"]["job_id"]
            .as_str()
            .expect("job id")
            .to_string();

        let detail = wait_for_codex_batch_import_job_terminal_state(router.clone(), &job_id).await;
        assert_eq!(detail["summary"]["succeeded_count"], 1);
        assert_eq!(detail["items"][0]["status"], "imported");

        let accounts_response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/admin/llm-gateway/accounts")
                    .header(header::HOST, "localhost")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("accounts response");
        assert_eq!(accounts_response.status(), StatusCode::OK);
        let accounts_body = to_bytes(accounts_response.into_body(), usize::MAX)
            .await
            .expect("accounts body");
        let accounts_value: serde_json::Value =
            serde_json::from_slice(&accounts_body).expect("accounts json");
        assert!(accounts_value["accounts"]
            .as_array()
            .expect("accounts array")
            .iter()
            .any(|item| item["name"] == "codex_primary"));

        std::fs::remove_dir_all(&root).expect("cleanup state root");
    }

    #[tokio::test]
    async fn router_marks_existing_account_name_as_conflict_in_codex_batch_import() {
        let (router, root) = persistent_test_router("codex-batch-import-conflict").await;

        let seed_response = router
            .clone()
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
                                "refresh_token": "seed-refresh",
                                "account_id": "acct-seed"
                            }
                        }"#,
                    ))
                    .expect("request"),
            )
            .await
            .expect("seed response");
        assert_eq!(seed_response.status(), StatusCode::OK);

        let create_response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/admin/llm-gateway/accounts/import-jobs")
                    .header(header::HOST, "localhost")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{
                            "provider_type": "codex",
                            "source_type": "local_json",
                            "validate_before_import": false,
                            "items": [{
                                "name": "codex_primary",
                                "auth_json": {
                                    "refresh_token": "rt-2",
                                    "account_id": "acct-2"
                                }
                            }]
                        }"#,
                    ))
                    .expect("request"),
            )
            .await
            .expect("create response");
        assert_eq!(create_response.status(), StatusCode::OK);
        let body = to_bytes(create_response.into_body(), usize::MAX)
            .await
            .expect("create body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("create json");
        let job_id = value["summary"]["job_id"]
            .as_str()
            .expect("job id")
            .to_string();

        let detail = wait_for_codex_batch_import_job_terminal_state(router.clone(), &job_id).await;
        assert_eq!(detail["items"][0]["status"], "conflict");
        assert!(detail["items"][0]["error_message"]
            .as_str()
            .is_some_and(|value| value.contains("account name already exists")));

        std::fs::remove_dir_all(&root).expect("cleanup state root");
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
    async fn router_hides_batch_imported_account_contributions_from_public_wall() {
        let (router, root) = persistent_test_router("public-account-contribution-visibility").await;

        let single_response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/llm-gateway/account-contribution-requests/submit")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header("x-real-ip", "198.51.100.21")
                    .body(Body::from(
                        r#"{
                            "account_name": "visible_account",
                            "refresh_token": "refresh-token-visible",
                            "contributor_message": "visible contribution"
                        }"#,
                    ))
                    .expect("request"),
            )
            .await
            .expect("single response");
        assert_eq!(single_response.status(), StatusCode::OK);
        let single_body = to_bytes(single_response.into_body(), usize::MAX)
            .await
            .expect("single body");
        let single_value: serde_json::Value =
            serde_json::from_slice(&single_body).expect("single json");
        let single_request_id = single_value["request_id"]
            .as_str()
            .expect("single request id")
            .to_string();

        let batch_response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/llm-gateway/account-contribution-requests/batch-submit")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header("x-real-ip", "198.51.100.22")
                    .body(Body::from(
                        r#"{
                            "contributor_message": "hidden contribution",
                            "items": [{
                                "account_name": "hidden_account",
                                "tokens": {
                                    "refresh_token": "refresh-token-hidden"
                                }
                            }]
                        }"#,
                    ))
                    .expect("request"),
            )
            .await
            .expect("batch response");
        assert_eq!(batch_response.status(), StatusCode::OK);
        let batch_body = to_bytes(batch_response.into_body(), usize::MAX)
            .await
            .expect("batch body");
        let batch_value: serde_json::Value =
            serde_json::from_slice(&batch_body).expect("batch json");
        let batch_request_id = batch_value["results"][0]["request_id"]
            .as_str()
            .expect("batch request id")
            .to_string();

        let repo = llm_access_store::repository::SqliteControlRepository::open_path(
            root.join("control/llm-access.sqlite3"),
        )
        .expect("open sqlite control repository");
        let updated_at_ms = 200;
        repo.issue_admin_account_contribution_request(
            &single_request_id,
            None,
            None,
            None,
            AdminReviewQueueAction {
                admin_note: Some("issued".to_string()),
                updated_at_ms,
            },
        )
        .await
        .expect("issue visible contribution")
        .expect("visible contribution exists");
        repo.issue_admin_account_contribution_request(
            &batch_request_id,
            None,
            None,
            None,
            AdminReviewQueueAction {
                admin_note: Some("issued".to_string()),
                updated_at_ms: updated_at_ms + 1,
            },
        )
        .await
        .expect("issue hidden contribution")
        .expect("hidden contribution exists");

        let wall_response = router
            .oneshot(
                Request::builder()
                    .uri("/api/llm-gateway/account-contributions")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("wall response");

        std::fs::remove_dir_all(&root).expect("cleanup state root");

        assert_eq!(wall_response.status(), StatusCode::OK);
        let wall_body = to_bytes(wall_response.into_body(), usize::MAX)
            .await
            .expect("wall body");
        let wall_value: serde_json::Value = serde_json::from_slice(&wall_body).expect("wall json");
        let contributions = wall_value["contributions"]
            .as_array()
            .expect("contributions array");
        assert_eq!(contributions.len(), 1);
        assert_eq!(contributions[0]["request_id"], single_request_id);
        assert_eq!(contributions[0]["account_name"], "visible_account");
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
