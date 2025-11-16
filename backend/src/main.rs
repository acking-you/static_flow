mod handlers;
mod markdown;
mod routes;
mod state;

use std::env;

use anyhow::Result;
use tracing_subscriber;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt::init();

    // Load environment variables
    let port = env::var("PORT").unwrap_or_else(|_| "3000".to_string());
    let content_dir = env::var("CONTENT_DIR").unwrap_or_else(|_| "../content".to_string());
    let images_dir = env::var("IMAGES_DIR").unwrap_or_else(|_| "../content/images".to_string());

    tracing::info!("Starting StaticFlow backend server");
    tracing::info!("Content directory: {}", content_dir);
    tracing::info!("Images directory: {}", images_dir);

    // Initialize application state
    let app_state = state::AppState::new(&content_dir, &images_dir).await?;
    tracing::info!("Loaded {} articles", app_state.article_count());

    // Build router
    let app = routes::create_router(app_state);

    // Start server
    let addr = format!("0.0.0.0:{}", port);
    tracing::info!("Listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
