//! Standalone Codex image gateway executable.

use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use anyhow::{anyhow, Context};
use axum::{body::Body, extract::State, http::Request, response::Response, routing::get, Router};
use clap::{Parser, Subcommand};
use llm_access_codex_image::{
    dispatch::ImageGatewayMode,
    gateway::{CodexImageGateway, CodexImageGatewayConfig},
    logging::{ImageLogConfig, ImageLogWriter},
};
use llm_access_core::store::{
    AdminConfigStore, ControlStore, ProviderRouteStore, DEFAULT_CODEX_CLIENT_VERSION,
};
use llm_access_store::{postgres::PostgresControlRepository, request_cache::RequestCacheConfig};
use tokio::net::TcpListener;

const DEFAULT_BIND_ADDR: &str = "127.0.0.1:19082";
const DEFAULT_CONTROL_DATABASE_URL_ENV: &str = "LLM_ACCESS_CODEX_IMAGE_CONTROL_DATABASE_URL";
const DEFAULT_REQUEST_CACHE_URL_ENV: &str = "LLM_ACCESS_REQUEST_CACHE_URL";
const DEFAULT_REQUEST_CACHE_KEY_PREFIX: &str = "llma";

#[derive(Debug, Parser)]
#[command(name = "llm-access-codex-image")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Serve(ServeArgs),
}

#[derive(Debug, Parser)]
struct ServeArgs {
    #[arg(long, default_value = DEFAULT_BIND_ADDR)]
    bind: SocketAddr,
    #[arg(long)]
    state_root: PathBuf,
    #[arg(long, default_value = DEFAULT_CONTROL_DATABASE_URL_ENV)]
    postgres_control_database_url_env: String,
    #[arg(long, default_value = DEFAULT_REQUEST_CACHE_URL_ENV)]
    request_cache_url_env: String,
    #[arg(long, default_value = DEFAULT_REQUEST_CACHE_KEY_PREFIX)]
    request_cache_key_prefix: String,
    #[arg(long)]
    image_log_dir: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| {
            "warn,llm_access_codex_image=info,llm_access_store=info".to_string()
        }))
        .try_init();
    match Cli::parse().command {
        Command::Serve(args) => serve(args).await,
    }
}

async fn serve(args: ServeArgs) -> anyhow::Result<()> {
    let database_url =
        std::env::var(&args.postgres_control_database_url_env).with_context(|| {
            format!("missing control database env `{}`", args.postgres_control_database_url_env)
        })?;
    let request_cache_config = request_cache_config(&args)?;
    let control = Arc::new(
        PostgresControlRepository::connect_without_migrations(&database_url, request_cache_config)
            .await
            .context("connect postgres control repository without migrations")?,
    );
    control
        .verify_codex_image_gateway_schema()
        .await
        .context("verify codex image gateway control-plane schema")?;
    let runtime_config = control
        .get_admin_runtime_config()
        .await
        .context("load runtime config")?;
    let image_log_dir = args
        .image_log_dir
        .unwrap_or_else(|| args.state_root.join("codex-image-logs"));
    let image_log = ImageLogWriter::new(ImageLogConfig {
        log_dir: image_log_dir,
        max_file_bytes: runtime_config.usage_journal_max_file_bytes,
        max_file_age_ms: runtime_config.usage_journal_max_file_age_ms,
        max_files: usize::try_from(runtime_config.usage_journal_max_files.max(1))
            .unwrap_or(usize::MAX),
    })?;
    let control_store: Arc<dyn ControlStore> = control.clone();
    let route_store: Arc<dyn ProviderRouteStore> = control;
    let gateway = Arc::new(CodexImageGateway::new(CodexImageGatewayConfig {
        mode: ImageGatewayMode::StandaloneBinary,
        control_store,
        route_store,
        image_log,
        upstream_base: llm_access_codex::request::codex_upstream_base_url_from_env(),
        codex_client_version: llm_access_codex::request::normalize_codex_client_version(
            &runtime_config.codex_client_version,
        )
        .unwrap_or_else(|| DEFAULT_CODEX_CLIENT_VERSION.to_string()),
    })?);
    let app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .fallback(handle_image_request)
        .with_state(gateway);
    let listener = TcpListener::bind(args.bind)
        .await
        .with_context(|| format!("bind codex image gateway on {}", args.bind))?;
    tracing::info!(bind = %args.bind, "codex image gateway listening");
    axum::serve(listener, app)
        .await
        .context("serve codex image gateway")
}

fn request_cache_config(args: &ServeArgs) -> anyhow::Result<Option<RequestCacheConfig>> {
    let Some(url) = std::env::var(&args.request_cache_url_env)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    let key_prefix = args.request_cache_key_prefix.trim();
    if key_prefix.is_empty() {
        return Err(anyhow!("--request-cache-key-prefix cannot be empty"));
    }
    Ok(Some(RequestCacheConfig {
        url,
        key_prefix: key_prefix.to_string(),
    }))
}

async fn handle_image_request(
    State(gateway): State<Arc<CodexImageGateway>>,
    request: Request<Body>,
) -> Response {
    gateway.handle_request(request).await
}
