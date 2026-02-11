use axum::{
    http::{HeaderValue, Method},
    middleware,
    routing::get,
    Router,
};
use tower_http::cors::{Any, CorsLayer};

use crate::{handlers, request_context, state::AppState};

pub fn create_router(state: AppState) -> Router {
    let allow_origin_env = std::env::var("ALLOWED_ORIGINS").ok();
    let allowed_origins = parse_allowed_origins(allow_origin_env.as_deref());

    // Configure CORS based on environment
    // Development: Allow all origins for local testing
    // Production: Restrict to GitHub Pages origin only
    let cors = match std::env::var("RUST_ENV").as_deref() {
        Ok("production") => {
            // Production: strict CORS, configurable via ALLOWED_ORIGINS
            if let Some(origins) = allowed_origins {
                CorsLayer::new()
                    .allow_origin(origins)
                    .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
                    .allow_headers(Any)
            } else {
                CorsLayer::new()
                    .allow_origin(
                        "https://acking-you.github.io"
                            .parse::<HeaderValue>()
                            .unwrap(),
                    )
                    .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
                    .allow_headers(Any)
            }
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
        .layer(middleware::from_fn(request_context::request_context_middleware))
        .layer(cors)
}

fn parse_allowed_origins(value: Option<&str>) -> Option<Vec<HeaderValue>> {
    let value = value?;
    let origins = value
        .split(',')
        .filter_map(|origin| {
            let origin = origin.trim();
            if origin.is_empty() {
                None
            } else {
                origin.parse::<HeaderValue>().ok()
            }
        })
        .collect::<Vec<_>>();

    if origins.is_empty() {
        None
    } else {
        Some(origins)
    }
}

#[cfg(test)]
mod tests {
    use super::parse_allowed_origins;

    #[test]
    fn parse_allowed_origins_returns_none_for_empty_input() {
        assert!(parse_allowed_origins(None).is_none());
        assert!(parse_allowed_origins(Some("  ,  ")).is_none());
    }

    #[test]
    fn parse_allowed_origins_parses_comma_separated_values() {
        let origins = parse_allowed_origins(Some("https://a.com, https://b.com")).unwrap();
        assert_eq!(origins.len(), 2);
    }
}
