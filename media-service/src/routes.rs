//! Route assembly for the standalone media service.

use std::sync::Arc;

use axum::{
    routing::{get, post},
    Router,
};

use crate::{handlers, state::LocalMediaState};

pub fn create_router(state: Arc<LocalMediaState>) -> Router {
    Router::new()
        .route("/internal/local-media/list", get(handlers::list_local_media))
        .route("/internal/local-media/playback/open", post(handlers::open_local_media_playback))
        .route(
            "/internal/local-media/playback/jobs/:job_id",
            get(handlers::get_local_media_job_status),
        )
        .route("/internal/local-media/playback/raw", get(handlers::stream_local_media_raw))
        .route(
            "/internal/local-media/playback/hls/:job_id/:file_name",
            get(handlers::stream_local_media_hls_artifact),
        )
        .route("/internal/local-media/poster", get(handlers::stream_local_media_poster))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tempfile::tempdir;
    use tower::ServiceExt;

    use super::create_router;
    use crate::state::LocalMediaState;

    #[tokio::test]
    async fn media_router_registers_internal_list_route() {
        let root = tempdir().expect("root tempdir");
        let cache = tempdir().expect("cache tempdir");
        let response = create_router(LocalMediaState::new_for_test(
            root.path().to_path_buf(),
            cache.path().to_path_buf(),
        ))
        .oneshot(
            Request::builder()
                .uri("/internal/local-media/list")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("route response");
        assert_ne!(response.status(), StatusCode::NOT_FOUND);
    }
}
