mod article_request_worker;
mod behavior_analytics;
mod comment_worker;
mod email;
mod geoip;
mod handlers;
mod memory_profiler;
mod music_wish_worker;
mod request_context;
mod routes;
mod seo;
mod state;

use std::env;

use anyhow::Result;
use better_mimalloc_rs::MiMalloc;
use memory_profiler::ProfiledMiMalloc;
use tracing_subscriber::EnvFilter;

const DEFAULT_LOG_FILTER: &str =
    "warn,static_flow_backend=info,static_flow_shared::lancedb_api=info";

#[global_allocator]
static GLOBAL_MIMALLOC: ProfiledMiMalloc = ProfiledMiMalloc::new(MiMalloc);

#[tokio::main]
async fn main() -> Result<()> {
    MiMalloc::init();
    // Initialize memory profiler as early as possible so all subsequent
    // allocations (tracing, LanceDB, etc.) are tracked from the start.
    let mem_profiler = memory_profiler::init_from_env();
    let mem_profiler_cfg = mem_profiler.config_snapshot();

    // Default: suppress verbose dependency info logs.
    // Override with RUST_LOG for troubleshooting.
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_LOG_FILTER));
    tracing_subscriber::fmt()
        .compact()
        .with_env_filter(filter)
        .init();

    tracing::info!(
        "Memory profiler: enabled={}, sample_rate={}, min_alloc_bytes={}, \
         max_tracked_allocations={}",
        mem_profiler_cfg.enabled,
        mem_profiler_cfg.sample_rate,
        mem_profiler_cfg.min_alloc_bytes,
        mem_profiler_cfg.max_tracked_allocations,
    );

    // Load environment variables
    let port = env::var("PORT").unwrap_or_else(|_| "3000".to_string());
    let db_uri = env::var("LANCEDB_URI").unwrap_or_else(|_| "../data/lancedb".to_string());
    let comments_db_uri =
        env::var("COMMENTS_LANCEDB_URI").unwrap_or_else(|_| "../data/lancedb-comments".to_string());
    let music_db_uri =
        env::var("MUSIC_LANCEDB_URI").unwrap_or_else(|_| "../data/lancedb-music".to_string());

    tracing::info!("Starting StaticFlow backend server");
    tracing::info!("LanceDB URI: {}", db_uri);
    tracing::info!("Comments LanceDB URI: {}", comments_db_uri);
    tracing::info!("Music LanceDB URI: {}", music_db_uri);

    // Load frontend index.html template for SEO injection
    let frontend_dist_dir =
        env::var("FRONTEND_DIST_DIR").unwrap_or_else(|_| "../frontend/dist".to_string());
    let index_html_path = format!("{}/index.html", frontend_dist_dir);
    let index_html_template = match tokio::fs::read_to_string(&index_html_path).await {
        Ok(html) => {
            tracing::info!("Loaded SEO template from {}", index_html_path);
            html
        },
        Err(err) => {
            tracing::warn!(
                "Failed to load {}: {} — SEO pages will return fallback HTML",
                index_html_path,
                err
            );
            String::new()
        },
    };

    // Initialize application state
    let app_state =
        state::AppState::new(&db_uri, &comments_db_uri, &music_db_uri, index_html_template).await?;

    // Build router
    let app_state_ref = app_state.clone();
    let app = routes::create_router(app_state);

    // Start server
    // Development: 0.0.0.0 for direct access
    // Production: usually 127.0.0.1 behind local Nginx/pb-mapper
    let bind_addr = env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0".to_string());
    let addr = format!("{}:{}", bind_addr, port);
    tracing::info!("Listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await?;

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("shutdown signal received, stopping background tasks...");
            app_state_ref.shutdown();
        })
        .await?;

    Ok(())
}
