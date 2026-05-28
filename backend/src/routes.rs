use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, OriginalUri, State},
    handler::Handler,
    http::{header, HeaderMap, HeaderValue, Method},
    middleware,
    response::{IntoResponse, Response},
    routing::{any, get, patch, post, put},
    Router,
};
use tower_http::{
    cors::{Any, CorsLayer},
    services::ServeDir,
};

use crate::{
    behavior_analytics, gpt2api_rs, handlers, health, request_context, seo, state::AppState,
};

#[cfg(feature = "local-media")]
fn local_media_upload_chunk_route<H, T, S>(handler: H) -> axum::routing::MethodRouter<S>
where
    H: Handler<T, S>,
    T: 'static,
    S: Clone + Send + Sync + 'static,
{
    put(handler)
        .layer(DefaultBodyLimit::max(static_flow_media_types::LOCAL_MEDIA_UPLOAD_CHUNK_BYTES))
}

fn request_prefers_html(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.contains("text/html") || value.contains("application/xhtml+xml"))
        .unwrap_or(false)
}

async fn admin_kiro_accounts_entry(
    state: State<AppState>,
    method: Method,
    headers: HeaderMap,
    uri: OriginalUri,
    body: Bytes,
) -> Response {
    if method == Method::GET && request_prefers_html(&headers) {
        return seo::seo_spa_shell(state, uri).await;
    }

    match crate::llm_access_admin_proxy::proxy_admin_request(state, method, headers, uri, body)
        .await
    {
        Ok(response) => response,
        Err(err) => err.into_response(),
    }
}

/// Build the full application router, including public APIs, admin APIs, and
/// SPA fallbacks.
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
                            .expect("hardcoded CORS origin is valid"),
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

    // API and admin routes have the highest priority so they cannot be
    // shadowed by the SPA history fallback below.
    let api_router = Router::new()
        .route("/api/healthz", get(health::get_healthz))
        .route("/api/articles", get(handlers::list_articles))
        .route("/api/articles/:id", get(handlers::get_article))
        .route("/api/articles/:id/raw/:lang", get(handlers::get_article_raw_markdown))
        .route("/interactive-pages/:page_id", get(handlers::get_interactive_page_entry))
        .route(
            "/interactive-pages/:page_id/entry",
            get(handlers::get_interactive_page_embedded_entry),
        )
        .route(
            "/api/interactive-pages/*asset_path",
            get(handlers::get_interactive_page_asset),
        )
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
        .route("/api/images/random", get(handlers::random_images))
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
        .route(
            "/admin/compaction-config",
            get(handlers::get_compaction_runtime_config).post(handlers::update_compaction_runtime_config),
        )
        .route(
            "/admin/gpt2api-rs/config",
            get(gpt2api_rs::get_admin_config).post(gpt2api_rs::update_admin_config),
        )
        .route(
            "/admin/gpt2api-rs/status",
            get(gpt2api_rs::get_admin_status),
        )
        .route(
            "/admin/gpt2api-rs/version",
            get(gpt2api_rs::get_public_version),
        )
        .route(
            "/admin/gpt2api-rs/models",
            get(gpt2api_rs::get_public_models),
        )
        .route(
            "/admin/gpt2api-rs/auth/login",
            post(gpt2api_rs::post_public_login),
        )
        .route(
            "/admin/gpt2api-rs/accounts",
            get(gpt2api_rs::list_admin_accounts).delete(gpt2api_rs::delete_admin_accounts),
        )
        .route(
            "/admin/gpt2api-rs/proxy-configs",
            get(gpt2api_rs::list_admin_proxy_configs)
                .post(gpt2api_rs::create_admin_proxy_config),
        )
        .route(
            "/admin/gpt2api-rs/proxy-configs/:proxy_id",
            patch(gpt2api_rs::update_admin_proxy_config)
                .delete(gpt2api_rs::delete_admin_proxy_config),
        )
        .route(
            "/admin/gpt2api-rs/proxy-configs/:proxy_id/check",
            post(gpt2api_rs::check_admin_proxy_config),
        )
        .route(
            "/admin/gpt2api-rs/account-groups",
            get(gpt2api_rs::list_admin_account_groups)
                .post(gpt2api_rs::create_admin_account_group),
        )
        .route(
            "/admin/gpt2api-rs/account-groups/:group_id",
            patch(gpt2api_rs::update_admin_account_group)
                .delete(gpt2api_rs::delete_admin_account_group),
        )
        .route(
            "/admin/gpt2api-rs/accounts/import",
            post(gpt2api_rs::import_admin_accounts),
        )
        .route(
            "/admin/gpt2api-rs/accounts/refresh",
            post(gpt2api_rs::refresh_admin_accounts),
        )
        .route(
            "/admin/gpt2api-rs/accounts/update",
            post(gpt2api_rs::update_admin_account),
        )
        .route(
            "/admin/gpt2api-rs/keys",
            get(gpt2api_rs::list_admin_keys).post(gpt2api_rs::create_admin_key),
        )
        .route(
            "/admin/gpt2api-rs/keys/:key_id",
            patch(gpt2api_rs::update_admin_key).delete(gpt2api_rs::delete_admin_key),
        )
        .route(
            "/admin/gpt2api-rs/keys/:key_id/rotate",
            post(gpt2api_rs::rotate_admin_key),
        )
        .route(
            "/admin/gpt2api-rs/account-contribution-requests",
            get(gpt2api_rs::list_admin_account_contribution_requests),
        )
        .route(
            "/admin/gpt2api-rs/account-contribution-requests/:request_id/approve",
            post(gpt2api_rs::approve_and_issue_account_contribution_request),
        )
        .route(
            "/admin/gpt2api-rs/account-contribution-requests/:request_id/reject",
            post(gpt2api_rs::reject_account_contribution_request),
        )
        .route(
            "/admin/gpt2api-rs/usage",
            get(gpt2api_rs::list_admin_usage),
        )
        .route(
            "/admin/gpt2api-rs/usage/events",
            get(gpt2api_rs::list_admin_usage_events),
        )
        .route(
            "/admin/gpt2api-rs/images/generations",
            post(gpt2api_rs::post_image_generation),
        )
        .route(
            "/admin/gpt2api-rs/images/edits",
            post(gpt2api_rs::post_image_edit),
        )
        .route(
            "/admin/gpt2api-rs/chat/completions",
            post(gpt2api_rs::post_chat_completions),
        )
        .route(
            "/admin/gpt2api-rs/responses",
            post(gpt2api_rs::post_responses),
        )
        .route(
            "/api/gpt2api/auth/verify",
            post(gpt2api_rs::post_public_auth_verify),
        )
        .route(
            "/api/gpt2api/account-contribution-requests/submit",
            post(gpt2api_rs::submit_public_account_contribution_request),
        )
        .route(
            "/api/gpt2api/images/generations",
            post(gpt2api_rs::public_image_generation),
        )
        .route(
            "/api/gpt2api/images/edits",
            post(gpt2api_rs::public_image_edit),
        )
        .route(
            "/api/gpt2api/chat/completions",
            post(gpt2api_rs::public_chat_completions),
        )
        .route(
            "/api/gpt2api/responses",
            post(gpt2api_rs::public_responses),
        )
        .route(
            "/api/gpt2api/*path",
            any(gpt2api_rs::proxy_public_product_api),
        )
        .route("/admin/api-behavior/overview", get(handlers::admin_api_behavior_overview))
        .route("/admin/api-behavior/events", get(handlers::admin_list_api_behavior_events))
        .route("/admin/api-behavior/cleanup", post(handlers::admin_cleanup_api_behavior))
        .route("/admin/api-behavior/compact", post(handlers::admin_compact_api_behavior))
        .route("/admin/geoip/status", get(handlers::get_geoip_status))
        .route(
            "/admin/runtime/memory/overview",
            get(handlers::admin_memory_profiler_overview),
        )
        .route(
            "/admin/runtime/memory/stacks",
            get(handlers::admin_memory_profiler_stacks),
        )
        .route(
            "/admin/runtime/memory/functions",
            get(handlers::admin_memory_profiler_functions),
        )
        .route(
            "/admin/runtime/memory/modules",
            get(handlers::admin_memory_profiler_modules),
        )
        .route(
            "/admin/runtime/memory/reset",
            post(handlers::admin_reset_memory_profiler),
        )
        .route(
            "/admin/runtime/memory/config",
            post(handlers::admin_update_memory_profiler_config),
        )
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
        );

    #[cfg(feature = "local-media")]
    let api_router = api_router
        .route("/admin/local-media/api/list", get(crate::media_proxy::handlers::list_local_media))
        .route(
            "/admin/local-media/api/playback/open",
            post(crate::media_proxy::handlers::open_local_media_playback),
        )
        .route(
            "/admin/local-media/api/playback/jobs/:job_id",
            get(crate::media_proxy::handlers::get_local_media_job_status),
        )
        .route(
            "/admin/local-media/api/playback/raw",
            get(crate::media_proxy::handlers::stream_local_media_raw),
        )
        .route(
            "/admin/local-media/api/playback/hls/:job_id/:file_name",
            get(crate::media_proxy::handlers::stream_local_media_hls_artifact),
        )
        .route(
            "/admin/local-media/api/playback/mp4/:job_id/:file_name",
            get(crate::media_proxy::handlers::stream_local_media_mp4_artifact),
        )
        .route(
            "/admin/local-media/api/poster",
            get(crate::media_proxy::handlers::stream_local_media_poster),
        )
        .route(
            "/admin/local-media/api/uploads/tasks",
            post(crate::media_proxy::handlers::create_upload_task)
                .get(crate::media_proxy::handlers::list_upload_tasks),
        )
        .route(
            "/admin/local-media/api/uploads/tasks/:task_id",
            get(crate::media_proxy::handlers::get_upload_task)
                .delete(crate::media_proxy::handlers::delete_upload_task),
        )
        .route(
            "/admin/local-media/api/uploads/tasks/:task_id/chunks",
            local_media_upload_chunk_route(crate::media_proxy::handlers::append_upload_chunk),
        );

    #[cfg(not(feature = "local-media"))]
    let api_router = api_router
        .route("/admin/local-media/api", any(handlers::local_media_feature_disabled_api))
        .route("/admin/local-media/api/*path", any(handlers::local_media_feature_disabled_api));

    let api_router = api_router
        .route("/api/llm-gateway/access", any(crate::llm_access_admin_proxy::proxy_public_request))
        .route(
            "/api/llm-gateway/public-page",
            any(crate::llm_access_admin_proxy::proxy_public_request),
        )
        .route(
            "/api/llm-gateway/model-catalog.json",
            any(crate::llm_access_admin_proxy::proxy_public_request),
        )
        .route("/api/llm-gateway/status", any(crate::llm_access_admin_proxy::proxy_public_request))
        .route(
            "/api/llm-gateway/public-usage/query",
            any(crate::llm_access_admin_proxy::proxy_public_request),
        )
        .route(
            "/api/llm-gateway/support-config",
            any(crate::llm_access_admin_proxy::proxy_public_request),
        )
        .route(
            "/api/llm-gateway/account-contributions",
            any(crate::llm_access_admin_proxy::proxy_public_request),
        )
        .route(
            "/api/llm-gateway/sponsors",
            any(crate::llm_access_admin_proxy::proxy_public_request),
        )
        .route(
            "/api/llm-gateway/token-requests/submit",
            any(crate::llm_access_admin_proxy::proxy_public_request),
        )
        .route(
            "/api/llm-gateway/account-contribution-requests/submit",
            any(crate::llm_access_admin_proxy::proxy_public_request),
        )
        .route(
            "/api/llm-gateway/account-contribution-requests/batch-submit",
            any(crate::llm_access_admin_proxy::proxy_public_request),
        )
        .route(
            "/api/llm-gateway/sponsor-requests/submit",
            any(crate::llm_access_admin_proxy::proxy_public_request),
        )
        .route(
            "/api/llm-gateway/support-assets/:file_name",
            any(crate::llm_access_admin_proxy::proxy_public_request),
        )
        .route("/api/llm-gateway/*path", any(crate::llm_access_admin_proxy::proxy_public_request))
        .route("/api/kiro-gateway/*path", any(crate::llm_access_admin_proxy::proxy_public_request))
        .route("/api/codex-gateway/*path", any(crate::llm_access_admin_proxy::proxy_public_request))
        .route("/api/llm-access/*path", any(crate::llm_access_admin_proxy::proxy_public_request))
        .route("/admin/llm-gateway", get(seo::seo_spa_shell))
        .route("/admin/llm-gateway/monitor", get(seo::seo_spa_shell))
        .route("/static_flow/admin/llm-gateway", get(seo::seo_spa_shell))
        .route("/static_flow/admin/llm-gateway/monitor", get(seo::seo_spa_shell))
        .route("/admin/kiro-gateway", get(seo::seo_spa_shell))
        .route("/admin/kiro-gateway/accounts", any(admin_kiro_accounts_entry))
        .route("/static_flow/admin/kiro-gateway", get(seo::seo_spa_shell))
        .route("/static_flow/admin/kiro-gateway/accounts", any(admin_kiro_accounts_entry))
        .route("/admin/llm-gateway/*path", any(crate::llm_access_admin_proxy::proxy_admin_request))
        .route(
            "/static_flow/admin/llm-gateway/*path",
            any(crate::llm_access_admin_proxy::proxy_admin_request),
        )
        .route("/admin/kiro-gateway/*path", any(crate::llm_access_admin_proxy::proxy_admin_request))
        .route(
            "/static_flow/admin/kiro-gateway/*path",
            any(crate::llm_access_admin_proxy::proxy_admin_request),
        )
        .route("/admin/llm-access/*path", any(crate::llm_access_admin_proxy::proxy_admin_request))
        .route(
            "/static_flow/admin/llm-access/*path",
            any(crate::llm_access_admin_proxy::proxy_admin_request),
        );

    let api_router = api_router.with_state(state.clone());

    // 2) SEO routes — /, /posts/:id, /sitemap.xml, /robots.txt
    let spa_state = state.clone();
    let seo_router = Router::new()
        .route("/", get(seo::seo_homepage))
        .route("/posts/:id", get(seo::seo_article_page))
        .route("/sitemap.xml", get(seo::sitemap_xml))
        .route("/robots.txt", get(seo::robots_txt))
        .with_state(state);

    let gpt2api_frontend_router = Router::new()
        .route("/gpt2api", get(handlers::serve_gpt2api_frontend))
        .route("/gpt2api/*path", get(handlers::serve_gpt2api_frontend))
        .route("/static_flow/gpt2api", get(handlers::serve_gpt2api_frontend))
        .route("/static_flow/gpt2api/*path", get(handlers::serve_gpt2api_frontend))
        .with_state(spa_state.clone());

    // 3) SPA fallback — serve frontend/dist/ static files; unknown routes get
    //    index.html (200)
    let frontend_dist_dir = spa_state.frontend_dist_dir.as_ref().clone();
    let spa_fallback = ServeDir::new(frontend_dist_dir);

    // Merge: API first, then SEO, then static files, then SPA index fallback
    api_router
        .merge(seo_router)
        .merge(gpt2api_frontend_router)
        .fallback_service(spa_fallback.fallback(get(seo::seo_spa_shell).with_state(spa_state)))
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
    #[cfg(not(feature = "local-media"))]
    use axum::{
        body::{to_bytes, Body},
        extract::OriginalUri,
        http::{header, Request, StatusCode},
        response::{Html, Json},
        routing::any,
        routing::get,
        Router,
    };
    #[cfg(feature = "local-media")]
    use axum::{
        body::{to_bytes, Body},
        extract::OriginalUri,
        http::{header, Request, StatusCode},
        response::{Html, Json},
        routing::any,
        routing::get,
        Router,
    };
    use tower::Service;
    use tower_http::services::ServeDir;

    use super::parse_allowed_origins;

    #[cfg(feature = "local-media")]
    async fn accept_chunk(_: axum::body::Bytes) -> StatusCode {
        StatusCode::OK
    }

    #[test]
    fn parse_allowed_origins_returns_none_for_empty_input() {
        assert!(parse_allowed_origins(None).is_none());
        assert!(parse_allowed_origins(Some("  ,  ")).is_none());
    }

    #[test]
    fn parse_allowed_origins_parses_comma_separated_values() {
        let origins = parse_allowed_origins(Some("https://a.com, https://b.com"))
            .expect("valid comma-separated origins should parse");
        assert_eq!(origins.len(), 2);
    }

    #[cfg(feature = "local-media")]
    #[tokio::test]
    async fn local_media_upload_chunk_route_accepts_full_chunk_body() {
        let mut app = Router::new()
            .route("/chunks", super::local_media_upload_chunk_route(accept_chunk))
            .into_service();

        let response = app
            .call(
                Request::builder()
                    .method("PUT")
                    .uri("/chunks")
                    .header(header::CONTENT_TYPE, "application/octet-stream")
                    .body(Body::from(vec![
                        0_u8;
                        static_flow_media_types::LOCAL_MEDIA_UPLOAD_CHUNK_BYTES
                    ]))
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[cfg(not(feature = "local-media"))]
    #[tokio::test]
    async fn local_media_disabled_api_route_returns_json_404() {
        let mut router = axum::Router::new()
            .route("/admin/local-media/api", any(crate::handlers::local_media_feature_disabled_api))
            .route(
                "/admin/local-media/api/*path",
                any(crate::handlers::local_media_feature_disabled_api),
            )
            .into_service();

        let response = router
            .call(
                Request::builder()
                    .uri("/admin/local-media/api/list?limit=1")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE),
            Some(&header::HeaderValue::from_static("application/json"))
        );

        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let body_text = std::str::from_utf8(&body).expect("utf8 body");
        assert!(body_text.contains("Local media feature is disabled"));
    }

    #[tokio::test]
    async fn admin_llm_gateway_monitor_deeplink_prefers_spa_shell() {
        let fallback = move |OriginalUri(uri): OriginalUri| async move {
            Html(format!("fallback:{}", uri.path()))
        };
        let explicit = move |OriginalUri(uri): OriginalUri| async move {
            Html(format!("explicit:{}", uri.path()))
        };
        let proxy = move |OriginalUri(uri): OriginalUri| async move {
            Json(serde_json::json!({"proxied": uri.path()}))
        };

        let mut router = Router::new()
            .route("/admin/llm-gateway/monitor", get(explicit))
            .route("/admin/llm-gateway/*path", any(proxy))
            .fallback_service(ServeDir::new("frontend/dist").fallback(get(fallback)))
            .into_service();

        let response = router
            .call(
                Request::builder()
                    .uri("/admin/llm-gateway/monitor")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        assert_eq!(
            std::str::from_utf8(&body).expect("utf8 body"),
            "explicit:/admin/llm-gateway/monitor"
        );
    }

    #[tokio::test]
    async fn admin_llm_gateway_usage_metrics_prefers_proxy_route() {
        let fallback = move |OriginalUri(uri): OriginalUri| async move {
            Html(format!("fallback:{}", uri.path()))
        };
        let explicit = move |OriginalUri(uri): OriginalUri| async move {
            Html(format!("explicit:{}", uri.path()))
        };
        let proxy = move |OriginalUri(uri): OriginalUri| async move {
            Json(serde_json::json!({"proxied": uri.path()}))
        };

        let mut router = Router::new()
            .route("/admin/llm-gateway/monitor", get(explicit))
            .route("/admin/llm-gateway/*path", any(proxy))
            .fallback_service(ServeDir::new("frontend/dist").fallback(get(fallback)))
            .into_service();

        let response = router
            .call(
                Request::builder()
                    .uri("/admin/llm-gateway/usage/metrics")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE),
            Some(&header::HeaderValue::from_static("application/json"))
        );
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        assert_eq!(
            std::str::from_utf8(&body).expect("utf8 body"),
            r#"{"proxied":"/admin/llm-gateway/usage/metrics"}"#
        );
    }

    #[tokio::test]
    async fn public_llm_gateway_status_prefers_proxy_route() {
        let fallback = move |OriginalUri(uri): OriginalUri| async move {
            Html(format!("fallback:{}", uri.path()))
        };
        let proxy = move |OriginalUri(uri): OriginalUri| async move {
            Json(serde_json::json!({"proxied": uri.path()}))
        };

        let mut router = Router::new()
            .route("/api/llm-gateway/status", any(proxy))
            .fallback_service(ServeDir::new("frontend/dist").fallback(get(fallback)))
            .into_service();

        let response = router
            .call(
                Request::builder()
                    .uri("/api/llm-gateway/status")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE),
            Some(&header::HeaderValue::from_static("application/json"))
        );
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        assert_eq!(
            std::str::from_utf8(&body).expect("utf8 body"),
            r#"{"proxied":"/api/llm-gateway/status"}"#
        );
    }

    #[tokio::test]
    async fn public_kiro_gateway_access_prefers_proxy_route() {
        let fallback = move |OriginalUri(uri): OriginalUri| async move {
            Html(format!("fallback:{}", uri.path()))
        };
        let proxy = move |OriginalUri(uri): OriginalUri| async move {
            Json(serde_json::json!({"proxied": uri.path()}))
        };

        let mut router = Router::new()
            .route("/api/kiro-gateway/*path", any(proxy))
            .fallback_service(ServeDir::new("frontend/dist").fallback(get(fallback)))
            .into_service();

        let response = router
            .call(
                Request::builder()
                    .uri("/api/kiro-gateway/access")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE),
            Some(&header::HeaderValue::from_static("application/json"))
        );
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        assert_eq!(
            std::str::from_utf8(&body).expect("utf8 body"),
            r#"{"proxied":"/api/kiro-gateway/access"}"#
        );
    }

    #[tokio::test]
    async fn public_kiro_gateway_models_prefers_proxy_route() {
        let fallback = move |OriginalUri(uri): OriginalUri| async move {
            Html(format!("fallback:{}", uri.path()))
        };
        let proxy = move |OriginalUri(uri): OriginalUri| async move {
            Json(serde_json::json!({"proxied": uri.path()}))
        };

        let mut router = Router::new()
            .route("/api/kiro-gateway/*path", any(proxy))
            .fallback_service(ServeDir::new("frontend/dist").fallback(get(fallback)))
            .into_service();

        let response = router
            .call(
                Request::builder()
                    .uri("/api/kiro-gateway/v1/models")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE),
            Some(&header::HeaderValue::from_static("application/json"))
        );
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        assert_eq!(
            std::str::from_utf8(&body).expect("utf8 body"),
            r#"{"proxied":"/api/kiro-gateway/v1/models"}"#
        );
    }

    #[test]
    fn request_prefers_html_only_for_document_accepts() {
        let mut html_headers = header::HeaderMap::new();
        html_headers.insert(header::ACCEPT, header::HeaderValue::from_static("text/html"));
        assert!(super::request_prefers_html(&html_headers));

        let mut json_headers = header::HeaderMap::new();
        json_headers.insert(header::ACCEPT, header::HeaderValue::from_static("application/json"));
        assert!(!super::request_prefers_html(&json_headers));

        let mut wildcard_headers = header::HeaderMap::new();
        wildcard_headers.insert(header::ACCEPT, header::HeaderValue::from_static("*/*"));
        assert!(!super::request_prefers_html(&wildcard_headers));
    }
}
