//! Standalone HTTP service shell for LLM access.

mod activity;
mod admin;
mod codex_refresh;
mod codex_status;
/// Command-line and environment configuration.
pub mod config;
/// Local Kiro endpoints.
pub mod kiro;
mod kiro_refresh;
mod kiro_status;
/// Provider request entrypoints.
pub mod provider;
mod public;
/// LLM-owned route classification.
pub mod routes;
/// Runtime startup validation.
pub mod runtime;
mod seed_staticflow;
mod submission;
mod support;
/// Usage-event helpers.
pub mod usage;

use std::sync::Arc;

use anyhow::Context;
use axum::{
    body::Body,
    extract::State,
    http::Request,
    response::Response,
    routing::{any, get, post},
    Json, Router,
};
use config::{CliCommand, ServeConfig, StorageConfig};
use llm_access_core::store::{
    AdminAccountGroupStore, AdminCodexAccountStore, AdminConfigStore, AdminKeyStore,
    AdminKiroAccountStore, AdminProxyStore, AdminReviewQueueStore, PublicAccessStore,
    PublicCommunityStore, PublicStatusStore, PublicSubmissionStore, PublicUsageStore,
    UsageAnalyticsStore,
};
use serde::Serialize;

#[cfg(test)]
pub(crate) static KIRO_UPSTREAM_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Clone)]
struct HttpState {
    provider_state: provider::ProviderState,
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
    usage_analytics_store: Arc<dyn UsageAnalyticsStore>,
    public_submission_store: Arc<dyn PublicSubmissionStore>,
    public_submit_guard: Arc<submission::PublicSubmitGuard>,
    public_status_store: Arc<dyn PublicStatusStore>,
}

/// Run `llm-access` from process arguments.
pub fn run_from_env() -> anyhow::Result<()> {
    match CliCommand::parse(std::env::args_os())? {
        CliCommand::Init(storage) => bootstrap_storage(&storage),
        CliCommand::Serve(config) => {
            bootstrap_storage(&config.storage)?;
            let runtime =
                tokio::runtime::Runtime::new().context("failed to create tokio runtime")?;
            runtime.block_on(serve(config))
        },
        CliCommand::SeedStaticFlow(config) => {
            bootstrap_storage(&config.storage)?;
            let runtime =
                tokio::runtime::Runtime::new().context("failed to create tokio runtime")?;
            runtime.block_on(seed_staticflow::seed_staticflow(config))
        },
    }
}

/// Initialize llm-access storage paths.
pub fn bootstrap_storage(config: &StorageConfig) -> anyhow::Result<()> {
    runtime::validate_state_root(config)?;
    llm_access_store::initialize_sqlite_target_path(&config.sqlite_control)?;
    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    llm_access_store::initialize_duckdb_target_path(&config.duckdb)?;
    llm_access_store::write_duckdb_schema_file(config.duckdb.with_extension("schema.sql"))?;
    Ok(())
}

/// Build the HTTP router.
pub fn router(runtime: runtime::LlmAccessRuntime) -> Router {
    let request_activity = Arc::new(activity::RequestActivityTracker::new());
    let provider_state = provider::ProviderState::new_with_config_store_and_activity(
        runtime.control_store(),
        runtime.provider_route_store(),
        runtime.admin_config_store(),
        Arc::clone(&request_activity),
    );
    let state = HttpState {
        provider_state,
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
        usage_analytics_store: runtime.usage_analytics_store(),
        public_submission_store: runtime.public_submission_store(),
        public_submit_guard: Arc::new(submission::PublicSubmitGuard::default()),
        public_status_store: runtime.public_status_store(),
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
        .route("/cc/v1/messages", post(provider_entry_handler))
        .route("/api/kiro-gateway/v1/models", get(kiro::get_models))
        .route("/api/kiro-gateway/v1/messages/count_tokens", post(kiro::count_tokens))
        .route("/api/kiro-gateway/cc/v1/messages/count_tokens", post(kiro::count_tokens))
        .route("/api/llm-gateway/*path", any(provider_entry_handler))
        .route("/api/kiro-gateway/*path", any(provider_entry_handler))
        .route("/api/codex-gateway/*path", any(provider_entry_handler))
        .route("/api/llm-access/*path", any(provider_entry_handler))
        .with_state(state)
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
    let result = axum::serve(listener, router(service_runtime))
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
mod tests {
    use std::{
        path::PathBuf,
        sync::{Arc, Mutex},
    };

    use async_trait::async_trait;
    use axum::{
        body::{to_bytes, Body},
        http::{header, Request, StatusCode},
        routing::get,
        Json, Router,
    };
    use llm_access_core::store::{AuthenticatedKey, ControlStore};
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
            kiro_auths_dir: root.join("auths/kiro"),
            codex_auths_dir: root.join("auths/codex"),
            logs_dir: root.join("logs"),
        };
        crate::bootstrap_storage(&config).expect("bootstrap storage");
        let runtime = crate::runtime::LlmAccessRuntime::from_storage_config(&config)
            .await
            .expect("open runtime");
        (super::router(runtime), root)
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
        let response = test_router()
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
        assert_eq!(value["total"], 0);
        assert_eq!(value["events"].as_array().expect("events array").len(), 0);
    }

    #[tokio::test]
    async fn router_routes_admin_llm_gateway_usage_detail_to_store() {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .uri("/admin/llm-gateway/usage/missing")
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
        assert!(body.contains("LLM gateway usage event not found"));
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
