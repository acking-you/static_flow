//! Standalone HTTP service shell for LLM access.

/// Command-line and environment configuration.
pub mod config;
/// Runtime startup validation.
pub mod runtime;

use anyhow::Context;
use axum::{
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use config::{CliCommand, ServeConfig, StorageConfig};
use serde::Serialize;

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
pub fn router() -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/version", get(version))
        .route("/v1/chat/completions", post(not_implemented))
        .route("/v1/messages", post(not_implemented))
}

/// Run the HTTP server until interrupted.
pub async fn serve(config: ServeConfig) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(config.bind_addr)
        .await
        .with_context(|| format!("failed to bind {}", config.bind_addr))?;
    axum::serve(listener, router())
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

async fn not_implemented() -> impl IntoResponse {
    (StatusCode::NOT_IMPLEMENTED, "llm-access provider runtime is not wired yet")
}
