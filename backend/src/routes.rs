use axum::{
    http::{HeaderValue, Method},
    middleware,
    response::{Html, IntoResponse},
    routing::{get, patch, post},
    Router,
};
use tower_http::{
    cors::{Any, CorsLayer},
    services::ServeDir,
};

use crate::{behavior_analytics, handlers, request_context, seo, state::AppState};

pub fn create_router(state: AppState) -> Router {
    let behavior_state = state.clone();
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
                    .allow_methods([Method::GET, Method::POST, Method::PATCH, Method::OPTIONS])
                    .allow_headers(Any)
            } else {
                CorsLayer::new()
                    .allow_origin(
                        "https://acking-you.github.io"
                            .parse::<HeaderValue>()
                            .unwrap(),
                    )
                    .allow_methods([Method::GET, Method::POST, Method::PATCH, Method::OPTIONS])
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
    // 1) API + admin routes (highest priority)
    let api_router = Router::new()
        .route("/api/articles", get(handlers::list_articles))
        .route("/api/articles/:id", get(handlers::get_article))
        .route("/api/articles/:id/raw/:lang", get(handlers::get_article_raw_markdown))
        .route("/api/articles/:id/view", post(handlers::track_article_view))
        .route("/api/articles/:id/view-trend", get(handlers::get_article_view_trend))
        .route("/api/articles/:id/related", get(handlers::related_articles))
        .route("/api/comments/submit", post(handlers::submit_comment))
        .route("/api/comments/list", get(handlers::list_comments))
        .route("/api/comments/stats", get(handlers::get_comment_stats))
        .route("/api/tags", get(handlers::list_tags))
        .route("/api/categories", get(handlers::list_categories))
        .route("/api/stats", get(handlers::get_stats))
        .route("/api/search", get(handlers::search_articles))
        .route("/api/semantic-search", get(handlers::semantic_search))
        .route("/api/images/:filename", get(handlers::serve_image))
        .route("/api/images", get(handlers::list_images))
        .route("/api/image-search", get(handlers::search_images))
        .route("/api/image-search-text", get(handlers::search_images_by_text))
        // Music API (read-only)
        .route("/api/music", get(handlers::list_songs))
        .route(
            "/api/music/recommendations/random",
            get(handlers::random_recommended_songs),
        )
        .route("/api/music/search", get(handlers::search_songs))
        .route("/api/music/artists", get(handlers::list_music_artists))
        .route("/api/music/albums", get(handlers::list_music_albums))
        .route("/api/music/:id", get(handlers::get_song))
        .route("/api/music/:id/audio", get(handlers::stream_song_audio))
        .route("/api/music/:id/lyrics", get(handlers::get_song_lyrics))
        .route("/api/music/:id/related", get(handlers::related_songs))
        .route("/api/music/next", post(handlers::resolve_next_song))
        // Music API (write, rate-limited)
        .route("/api/music/:id/play", post(handlers::track_song_play))
        .route("/api/music/comments/submit", post(handlers::submit_music_comment))
        .route("/api/music/comments/list", get(handlers::list_music_comments))
        .route(
            "/admin/view-analytics-config",
            get(handlers::get_view_analytics_config).post(handlers::update_view_analytics_config),
        )
        .route(
            "/admin/comment-config",
            get(handlers::get_comment_runtime_config).post(handlers::update_comment_runtime_config),
        )
        .route(
            "/admin/api-behavior-config",
            get(handlers::get_api_behavior_config).post(handlers::update_api_behavior_config),
        )
        .route("/admin/api-behavior/overview", get(handlers::admin_api_behavior_overview))
        .route("/admin/api-behavior/events", get(handlers::admin_list_api_behavior_events))
        .route("/admin/api-behavior/cleanup", post(handlers::admin_cleanup_api_behavior))
        .route("/admin/api-behavior/compact", post(handlers::admin_compact_api_behavior))
        .route("/admin/geoip/status", get(handlers::get_geoip_status))
        .route("/admin/comments/tasks", get(handlers::admin_list_comment_tasks))
        .route("/admin/comments/tasks/grouped", get(handlers::admin_list_comment_tasks_grouped))
        .route(
            "/admin/comments/tasks/:task_id",
            get(handlers::admin_get_comment_task)
                .patch(handlers::admin_patch_comment_task)
                .delete(handlers::admin_delete_comment_task),
        )
        .route(
            "/admin/comments/tasks/:task_id/ai-output",
            get(handlers::admin_get_comment_task_ai_output),
        )
        .route(
            "/admin/comments/tasks/:task_id/ai-output/stream",
            get(handlers::admin_stream_comment_task_ai_output),
        )
        .route("/admin/comments/tasks/:task_id/approve", post(handlers::admin_approve_comment_task))
        .route(
            "/admin/comments/tasks/:task_id/approve-and-run",
            post(handlers::admin_approve_and_run_comment_task),
        )
        .route("/admin/comments/tasks/:task_id/reject", post(handlers::admin_reject_comment_task))
        .route("/admin/comments/tasks/:task_id/retry", post(handlers::admin_retry_comment_task))
        .route("/admin/comments/ai-runs", get(handlers::admin_list_comment_ai_runs))
        .route("/admin/comments/published", get(handlers::admin_list_published_comments))
        .route(
            "/admin/comments/published/:comment_id",
            patch(handlers::admin_patch_published_comment)
                .delete(handlers::admin_delete_published_comment),
        )
        .route("/admin/comments/audit-logs", get(handlers::admin_list_comment_audit_logs))
        .route("/admin/comments/cleanup", post(handlers::admin_cleanup_comments))
        .route(
            "/admin/music-config",
            get(handlers::get_music_config).post(handlers::update_music_config),
        )
        .route("/api/music/wishes/submit", post(handlers::submit_music_wish))
        .route("/api/music/wishes/list", get(handlers::list_music_wishes))
        .route("/admin/music-wishes/tasks", get(handlers::admin_list_music_wishes))
        .route(
            "/admin/music-wishes/tasks/:wish_id",
            get(handlers::admin_get_music_wish).delete(handlers::admin_delete_music_wish),
        )
        .route(
            "/admin/music-wishes/tasks/:wish_id/approve-and-run",
            post(handlers::admin_approve_and_run_music_wish),
        )
        .route(
            "/admin/music-wishes/tasks/:wish_id/reject",
            post(handlers::admin_reject_music_wish),
        )
        .route(
            "/admin/music-wishes/tasks/:wish_id/retry",
            post(handlers::admin_retry_music_wish),
        )
        .route(
            "/admin/music-wishes/tasks/:wish_id/ai-output",
            get(handlers::admin_music_wish_ai_output),
        )
        .route(
            "/admin/music-wishes/tasks/:wish_id/ai-output/stream",
            get(handlers::admin_music_wish_ai_stream),
        )
        // Article Request routes
        .route("/api/article-requests/submit", post(handlers::submit_article_request))
        .route("/api/article-requests/list", get(handlers::list_article_requests))
        .route("/admin/article-requests/tasks", get(handlers::admin_list_article_requests))
        .route(
            "/admin/article-requests/tasks/:request_id",
            get(handlers::admin_get_article_request).delete(handlers::admin_delete_article_request),
        )
        .route(
            "/admin/article-requests/tasks/:request_id/approve-and-run",
            post(handlers::admin_approve_and_run_article_request),
        )
        .route(
            "/admin/article-requests/tasks/:request_id/reject",
            post(handlers::admin_reject_article_request),
        )
        .route(
            "/admin/article-requests/tasks/:request_id/retry",
            post(handlers::admin_retry_article_request),
        )
        .route(
            "/admin/article-requests/tasks/:request_id/ai-output",
            get(handlers::admin_article_request_ai_output),
        )
        .route(
            "/admin/article-requests/tasks/:request_id/ai-output/stream",
            get(handlers::admin_article_request_ai_stream),
        )
        .with_state(state.clone());

    // 2) SEO routes — /, /posts/:id, /sitemap.xml, /robots.txt
    let spa_state = state.clone();
    let seo_router = Router::new()
        .route("/", get(seo::seo_homepage))
        .route("/posts/:id", get(seo::seo_article_page))
        .route("/sitemap.xml", get(seo::sitemap_xml))
        .route("/robots.txt", get(seo::robots_txt))
        .with_state(state);

    // 3) SPA fallback — serve frontend/dist/ static files; unknown routes get
    //    index.html (200)
    let frontend_dist_dir =
        std::env::var("FRONTEND_DIST_DIR").unwrap_or_else(|_| "../frontend/dist".to_string());
    let spa_fallback = ServeDir::new(&frontend_dist_dir);

    let spa_index_fallback =
        move || async move { Html(spa_state.index_html_template.as_ref().clone()).into_response() };

    // Merge: API first, then SEO, then static files, then SPA index fallback
    api_router
        .merge(seo_router)
        .fallback_service(spa_fallback.fallback(get(spa_index_fallback)))
        .layer(middleware::from_fn(request_context::request_context_middleware))
        .layer(middleware::from_fn_with_state(
            behavior_state,
            behavior_analytics::behavior_analytics_middleware,
        ))
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
