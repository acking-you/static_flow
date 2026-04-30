//! Standalone HTTP service shell for LLM access.

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
use llm_access_core::store::PublicAccessStore;
use serde::Serialize;

#[derive(Clone)]
struct HttpState {
    provider_state: provider::ProviderState,
    public_access_store: Arc<dyn PublicAccessStore>,
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
        public_access_store: runtime.public_access_store(),
    };
    Router::new()
        .route("/healthz", get(healthz))
        .route("/version", get(version))
        .route("/api/llm-gateway/access", get(public::get_llm_gateway_access))
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
    use std::sync::Arc;

    use async_trait::async_trait;
    use axum::{
        body::{to_bytes, Body},
        http::{header, Request, StatusCode},
    };
    use llm_access_core::store::{AuthenticatedKey, ControlStore};
    use tower::util::ServiceExt;

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
}
