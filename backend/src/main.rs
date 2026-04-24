//! StaticFlow backend server binary.

mod article_request_worker;
mod behavior_analytics;
mod comment_worker;
mod email;
mod geoip;
mod gpt2api_rs;
mod handlers;
mod health;
mod kiro_gateway;
mod llm_gateway;
#[cfg(feature = "local-media")]
mod media_proxy;
mod memory_profiler;
mod music_wish_worker;
mod public_submit_guard;
mod request_context;
mod routes;
mod seo;
mod state;
mod table_maintenance;
mod upstream_proxy;

use std::{env, net::SocketAddr, time::Duration};

use anyhow::Result;
use better_mimalloc_rs::MiMalloc;
use memory_profiler::ProfiledMiMalloc;
use static_flow_shared::runtime_logging::init_runtime_logging;

const DEFAULT_LOG_FILTER: &str =
    "warn,static_flow_backend=info,static_flow_shared::lancedb_api=info";
const GRACEFUL_SHUTDOWN_TIMEOUT_SECONDS: u64 = 10;

#[global_allocator]
static GLOBAL_MIMALLOC: ProfiledMiMalloc = ProfiledMiMalloc::new(MiMalloc);

#[tokio::main]
async fn main() -> Result<()> {
    MiMalloc::init();
    let _log_guards = init_runtime_logging("backend", DEFAULT_LOG_FILTER)?;
    // Initialize memory profiler as early as possible so all subsequent
    // allocations (tracing, LanceDB, etc.) are tracked from the start.
    let mem_profiler = memory_profiler::init_from_env();
    let mem_profiler_cfg = mem_profiler.config_snapshot();

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

    let frontend_dist_dir =
        env::var("FRONTEND_DIST_DIR").unwrap_or_else(|_| "../frontend/dist".to_string());
    tracing::info!("Frontend dist dir: {}", frontend_dist_dir);

    // Initialize application state
    let app_state =
        state::AppState::new(&db_uri, &comments_db_uri, &music_db_uri, frontend_dist_dir).await?;

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

    let (server_shutdown_tx, server_shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let mut server = tokio::spawn(async move {
        axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
            .with_graceful_shutdown(async move {
                let _ = server_shutdown_rx.await;
            })
            .await
    });

    tokio::select! {
        server_result = &mut server => {
            server_result??;
        }
        signal_result = tokio::signal::ctrl_c() => {
            signal_result?;
            tracing::info!("shutdown signal received, stopping background tasks...");
            app_state_ref.shutdown();
            let _ = server_shutdown_tx.send(());

            match tokio::time::timeout(
                Duration::from_secs(GRACEFUL_SHUTDOWN_TIMEOUT_SECONDS),
                &mut server,
            )
            .await
            {
                Ok(server_result) => {
                    server_result??;
                }
                Err(_) => {
                    tracing::warn!(
                        timeout_seconds = GRACEFUL_SHUTDOWN_TIMEOUT_SECONDS,
                        "backend graceful shutdown timed out; aborting remaining server connections"
                    );
                    server.abort();
                    match server.await {
                        Err(join_err) if join_err.is_cancelled() => {}
                        other => other??,
                    }
                }
            }
        }
    }

    Ok(())
}
