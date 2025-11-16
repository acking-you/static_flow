use axum::{routing::get, Router};
use tower_http::cors::{Any, CorsLayer};

use crate::{handlers, state::AppState};

pub fn create_router(state: AppState) -> Router {
    // Configure CORS
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    // Define routes
    Router::new()
        .route("/api/articles", get(handlers::list_articles))
        .route("/api/articles/:id", get(handlers::get_article))
        .route("/api/tags", get(handlers::list_tags))
        .route("/api/categories", get(handlers::list_categories))
        .route("/api/search", get(handlers::search_articles))
        .route("/api/images/:filename", get(handlers::serve_image))
        .with_state(state)
        .layer(cors)
}
