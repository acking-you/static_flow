//! Route assembly for the standalone media service.

use std::sync::Arc;

use axum::{
    extract::DefaultBodyLimit,
    handler::Handler,
    routing::{get, post, put},
    Router,
};

use crate::{handlers, state::LocalMediaState};

fn upload_chunk_route<H, T, S>(handler: H) -> axum::routing::MethodRouter<S>
where
    H: Handler<T, S>,
    T: 'static,
    S: Clone + Send + Sync + 'static,
{
    put(handler)
        .layer(DefaultBodyLimit::max(static_flow_media_types::LOCAL_MEDIA_UPLOAD_CHUNK_BYTES))
}

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
        .route(
            "/internal/local-media/playback/mp4/:job_id/:file_name",
            get(handlers::stream_local_media_mp4_artifact),
        )
        .route("/internal/local-media/poster", get(handlers::stream_local_media_poster))
        .route(
            "/internal/local-media/uploads/tasks",
            post(handlers::create_upload_task).get(handlers::list_upload_tasks),
        )
        .route(
            "/internal/local-media/uploads/tasks/:task_id",
            get(handlers::get_upload_task).delete(handlers::delete_upload_task),
        )
        .route(
            "/internal/local-media/uploads/tasks/:task_id/chunks",
            upload_chunk_route(handlers::append_upload_chunk),
        )
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{header, Request, StatusCode},
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

    #[tokio::test]
    async fn media_router_registers_upload_routes() {
        let root = tempdir().expect("root tempdir");
        let cache = tempdir().expect("cache tempdir");
        let response = create_router(LocalMediaState::new_for_test(
            root.path().to_path_buf(),
            cache.path().to_path_buf(),
        ))
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/internal/local-media/uploads/tasks")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("route response");
        assert_ne!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn upload_chunk_route_accepts_full_chunk_body_without_413() {
        let root = tempdir().expect("root tempdir");
        let cache = tempdir().expect("cache tempdir");
        let response = create_router(LocalMediaState::new_for_test(
            root.path().to_path_buf(),
            cache.path().to_path_buf(),
        ))
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/internal/local-media/uploads/tasks/task-1/chunks?offset=0")
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .body(Body::from(vec![
                    0_u8;
                    static_flow_media_types::LOCAL_MEDIA_UPLOAD_CHUNK_BYTES
                ]))
                .expect("request"),
        )
        .await
        .expect("route response");

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
