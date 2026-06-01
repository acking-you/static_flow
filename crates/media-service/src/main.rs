//! Binary entrypoint for the standalone StaticFlow media service.

use std::env;

use anyhow::Result;
use static_flow_media::{routes, state::LocalMediaState};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().compact().init();
    let state = LocalMediaState::from_env()
        .await?
        .ok_or_else(|| anyhow::anyhow!("local media root is not configured"))?;
    let host = env::var("HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port = env::var("PORT").unwrap_or_else(|_| "39085".to_string());
    let bind_addr = format!("{host}:{port}");
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!(bind_addr = %bind_addr, "starting static-flow-media");
    axum::serve(listener, routes::create_router(state)).await?;
    Ok(())
}
