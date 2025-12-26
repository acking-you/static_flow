use axum::{
    http::{HeaderValue, Method},
    routing::get,
    Router,
};
use tower_http::cors::{Any, CorsLayer};

use crate::{handlers, state::AppState};

pub fn create_router(state: AppState) -> Router {
    // Configure CORS based on environment
    // Development: Allow all origins for local testing
    // Production: Restrict to GitHub Pages origin only
    let cors = match std::env::var("RUST_ENV").as_deref() {
        Ok("production") => {
            // Production: strict CORS
            CorsLayer::new()
                .allow_origin(
                    "https://acking-you.github.io"
                        .parse::<HeaderValue>()
                        .unwrap(),
                )
                .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
                .allow_headers(Any)
        },
        _ => {
            // Development: permissive CORS
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any)
        },
    };

    // Define routes
    Router::new()
        .route("/api/articles", get(handlers::list_articles))
        .route("/api/articles/:id", get(handlers::get_article))
        .route("/api/articles/:id/related", get(handlers::related_articles))
        .route("/api/tags", get(handlers::list_tags))
        .route("/api/categories", get(handlers::list_categories))
        .route("/api/search", get(handlers::search_articles))
        .route("/api/semantic-search", get(handlers::semantic_search))
        .route("/api/images/:filename", get(handlers::serve_image))
        .route("/api/images", get(handlers::list_images))
        .route("/api/image-search", get(handlers::search_images))
        .with_state(state)
        .layer(cors)
}
