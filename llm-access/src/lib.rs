//! Standalone HTTP service shell for LLM access.

mod admin;
/// Command-line and environment configuration.
pub mod config;
/// Local Kiro compatibility endpoints.
pub mod kiro;
/// Provider request entrypoints.
pub mod provider;
mod public;
/// LLM-owned route classification.
pub mod routes;
/// Runtime startup validation.
pub mod runtime;
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
    AdminProxyStore, PublicAccessStore, PublicCommunityStore, PublicStatusStore,
    PublicSubmissionStore, PublicUsageStore,
};
use serde::Serialize;

#[derive(Clone)]
struct HttpState {
    provider_state: provider::ProviderState,
    admin_config_store: Arc<dyn AdminConfigStore>,
    admin_key_store: Arc<dyn AdminKeyStore>,
    admin_account_group_store: Arc<dyn AdminAccountGroupStore>,
    admin_proxy_store: Arc<dyn AdminProxyStore>,
    admin_codex_account_store: Arc<dyn AdminCodexAccountStore>,
    public_access_store: Arc<dyn PublicAccessStore>,
    public_community_store: Arc<dyn PublicCommunityStore>,
    public_usage_store: Arc<dyn PublicUsageStore>,
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
    }
}

/// Initialize llm-access storage paths.
pub fn bootstrap_storage(config: &StorageConfig) -> anyhow::Result<()> {
    runtime::validate_state_root(config)?;
    llm_access_store::initialize_sqlite_target_path(&config.sqlite_control)?;
    llm_access_store::write_duckdb_schema_file(config.duckdb.with_extension("schema.sql"))?;
    Ok(())
}

/// Build the HTTP router.
pub fn router(runtime: runtime::LlmAccessRuntime) -> Router {
    let provider_state = provider::ProviderState::new(runtime.control_store());
    let state = HttpState {
        provider_state,
        admin_config_store: runtime.admin_config_store(),
        admin_key_store: runtime.admin_key_store(),
        admin_account_group_store: runtime.admin_account_group_store(),
        admin_proxy_store: runtime.admin_proxy_store(),
        admin_codex_account_store: runtime.admin_codex_account_store(),
        public_access_store: runtime.public_access_store(),
        public_community_store: runtime.public_community_store(),
        public_usage_store: runtime.public_usage_store(),
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
    let service_runtime = runtime::LlmAccessRuntime::from_storage_config(&config.storage)?;
    let listener = tokio::net::TcpListener::bind(config.bind_addr)
        .await
        .with_context(|| format!("failed to bind {}", config.bind_addr))?;
    axum::serve(listener, router(service_runtime))
        .await
        .context("llm-access server failed")
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
