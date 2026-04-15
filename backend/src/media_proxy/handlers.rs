use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Json, Response},
};
use serde::de::DeserializeOwned;
use static_flow_media_types::{
    CreateUploadTaskRequest, CreateUploadTaskResponse, ListUploadTasksQuery,
    ListUploadTasksResponse, LocalMediaListQuery, LocalMediaListResponse, OpenPlaybackRequest,
    PlaybackJobStatusResponse, PlaybackOpenResponse, PosterQuery, RawPlaybackQuery,
    UploadChunkQuery, UploadTaskRecord,
};

use super::{
    forward::{
        forward_hls_request, forward_mp4_request, forward_poster_request, forward_raw_request,
        forward_upload_chunk_request,
    },
    MediaProxyState,
};
use crate::{
    handlers::{ensure_admin_access, ErrorResponse},
    state::AppState,
};

type HandlerResult<T> = Result<T, (StatusCode, Json<ErrorResponse>)>;

pub async fn list_local_media(
    State(state): State<AppState>,
    Query(query): Query<LocalMediaListQuery>,
    headers: HeaderMap,
) -> HandlerResult<Json<LocalMediaListResponse>> {
    ensure_admin_access(&state, &headers)?;
    let limit = query.limit.unwrap_or(120).clamp(1, 500);
    let offset = query.offset.unwrap_or(0);
    let Some(media_proxy) = state.media_proxy.clone() else {
        return Ok(Json(LocalMediaListResponse::unconfigured(limit, offset)));
    };

    let response: LocalMediaListResponse = send_json(
        media_proxy
            .client()
            .get(join_internal_url(media_proxy.as_ref(), "internal/local-media/list")?)
            .query(&query),
    )
    .await?;
    Ok(Json(response))
}

pub async fn open_local_media_playback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<OpenPlaybackRequest>,
) -> HandlerResult<Json<PlaybackOpenResponse>> {
    ensure_admin_access(&state, &headers)?;
    let media_proxy = configured_media_proxy(&state)?;
    let response: PlaybackOpenResponse = send_json(
        media_proxy
            .client()
            .post(join_internal_url(media_proxy.as_ref(), "internal/local-media/playback/open")?)
            .json(&request),
    )
    .await?;
    Ok(Json(response))
}

pub async fn list_upload_tasks(
    State(state): State<AppState>,
    Query(query): Query<ListUploadTasksQuery>,
    headers: HeaderMap,
) -> HandlerResult<Json<ListUploadTasksResponse>> {
    ensure_admin_access(&state, &headers)?;
    let media_proxy = configured_media_proxy(&state)?;
    let response = send_json(
        media_proxy
            .client()
            .get(join_internal_url(media_proxy.as_ref(), "internal/local-media/uploads/tasks")?)
            .query(&query),
    )
    .await?;
    Ok(Json(response))
}

pub async fn create_upload_task(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateUploadTaskRequest>,
) -> HandlerResult<Json<CreateUploadTaskResponse>> {
    ensure_admin_access(&state, &headers)?;
    let media_proxy = configured_media_proxy(&state)?;
    let response = send_json(
        media_proxy
            .client()
            .post(join_internal_url(media_proxy.as_ref(), "internal/local-media/uploads/tasks")?)
            .json(&request),
    )
    .await?;
    Ok(Json(response))
}

pub async fn get_upload_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    headers: HeaderMap,
) -> HandlerResult<Json<UploadTaskRecord>> {
    ensure_admin_access(&state, &headers)?;
    let media_proxy = configured_media_proxy(&state)?;
    let relative = format!("internal/local-media/uploads/tasks/{task_id}");
    let response = send_json(
        media_proxy
            .client()
            .get(join_internal_url(media_proxy.as_ref(), &relative)?),
    )
    .await?;
    Ok(Json(response))
}

pub async fn get_local_media_job_status(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
    headers: HeaderMap,
) -> HandlerResult<Json<PlaybackJobStatusResponse>> {
    ensure_admin_access(&state, &headers)?;
    let media_proxy = configured_media_proxy(&state)?;
    let relative = format!("internal/local-media/playback/jobs/{job_id}");
    let response: PlaybackJobStatusResponse = send_json(
        media_proxy
            .client()
            .get(join_internal_url(media_proxy.as_ref(), &relative)?),
    )
    .await?;
    Ok(Json(response))
}

pub async fn stream_local_media_raw(
    State(state): State<AppState>,
    Query(query): Query<RawPlaybackQuery>,
    headers: HeaderMap,
) -> HandlerResult<Response> {
    ensure_admin_access(&state, &headers)?;
    let media_proxy = configured_media_proxy(&state)?;
    forward_raw_request(media_proxy.client(), &media_proxy.config().base_url, &query.file, &headers)
        .await
        .map_err(bad_gateway)
}

pub async fn stream_local_media_hls_artifact(
    State(state): State<AppState>,
    Path((job_id, file_name)): Path<(String, String)>,
    headers: HeaderMap,
) -> HandlerResult<Response> {
    ensure_admin_access(&state, &headers)?;
    let media_proxy = configured_media_proxy(&state)?;
    if file_name.contains('/') || file_name.contains('\\') {
        return Err(error_response(StatusCode::BAD_REQUEST, "Invalid HLS file name"));
    }
    forward_hls_request(
        media_proxy.client(),
        &media_proxy.config().base_url,
        &job_id,
        &file_name,
        &headers,
    )
    .await
    .map_err(bad_gateway)
}

pub async fn stream_local_media_mp4_artifact(
    State(state): State<AppState>,
    Path((job_id, file_name)): Path<(String, String)>,
    headers: HeaderMap,
) -> HandlerResult<Response> {
    ensure_admin_access(&state, &headers)?;
    let media_proxy = configured_media_proxy(&state)?;
    if file_name.contains('/') || file_name.contains('\\') {
        return Err(error_response(StatusCode::BAD_REQUEST, "Invalid MP4 file name"));
    }
    forward_mp4_request(
        media_proxy.client(),
        &media_proxy.config().base_url,
        &job_id,
        &file_name,
        &headers,
    )
    .await
    .map_err(bad_gateway)
}

pub async fn stream_local_media_poster(
    State(state): State<AppState>,
    Query(query): Query<PosterQuery>,
    headers: HeaderMap,
) -> HandlerResult<Response> {
    ensure_admin_access(&state, &headers)?;
    let media_proxy = configured_media_proxy(&state)?;
    forward_poster_request(media_proxy.client(), &media_proxy.config().base_url, &query.file)
        .await
        .map_err(bad_gateway)
}

pub async fn append_upload_chunk(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    Query(query): Query<UploadChunkQuery>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> HandlerResult<Response> {
    ensure_admin_access(&state, &headers)?;
    let media_proxy = configured_media_proxy(&state)?;
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .cloned()
        .unwrap_or_else(|| axum::http::HeaderValue::from_static("application/octet-stream"));
    let content_type = reqwest::header::HeaderValue::from_bytes(content_type.as_bytes())
        .map_err(|err| internal_error(format!("invalid content type header: {err}")))?;
    forward_upload_chunk_request(
        media_proxy.client(),
        &media_proxy.config().base_url,
        &task_id,
        query.offset,
        content_type,
        body,
    )
    .await
    .map_err(bad_gateway)
}

pub async fn delete_upload_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    headers: HeaderMap,
) -> HandlerResult<Json<UploadTaskRecord>> {
    ensure_admin_access(&state, &headers)?;
    let media_proxy = configured_media_proxy(&state)?;
    let relative = format!("internal/local-media/uploads/tasks/{task_id}");
    let response = send_json(
        media_proxy
            .client()
            .delete(join_internal_url(media_proxy.as_ref(), &relative)?),
    )
    .await?;
    Ok(Json(response))
}

fn configured_media_proxy(
    state: &AppState,
) -> Result<Arc<MediaProxyState>, (StatusCode, Json<ErrorResponse>)> {
    state.media_proxy.clone().ok_or_else(|| {
        error_response(StatusCode::SERVICE_UNAVAILABLE, "Local media is not configured")
    })
}

async fn send_json<T>(request: reqwest::RequestBuilder) -> HandlerResult<T>
where
    T: DeserializeOwned,
{
    let response = request.send().await.map_err(bad_gateway)?;
    let status = StatusCode::from_u16(response.status().as_u16())
        .map_err(|err| internal_error(format!("invalid upstream status: {err}")))?;
    let bytes = response.bytes().await.map_err(bad_gateway)?;

    if status.is_success() {
        let payload = serde_json::from_slice::<T>(&bytes)
            .map_err(|err| internal_error(format!("failed to decode upstream json: {err}")))?;
        return Ok(payload);
    }

    let payload = serde_json::from_slice::<ErrorResponse>(&bytes).unwrap_or(ErrorResponse {
        error: "Local media request failed".to_string(),
        code: status.as_u16(),
    });
    Err((status, Json(payload)))
}

fn join_internal_url(
    media_proxy: &MediaProxyState,
    relative: &str,
) -> Result<reqwest::Url, (StatusCode, Json<ErrorResponse>)> {
    media_proxy
        .config()
        .base_url
        .join(relative)
        .map_err(|err| internal_error(format!("invalid upstream path {relative}: {err}")))
}

fn bad_gateway(err: impl std::fmt::Display) -> (StatusCode, Json<ErrorResponse>) {
    tracing::error!("media proxy upstream error: {err}");
    error_response(StatusCode::BAD_GATEWAY, "Local media service unavailable")
}

fn internal_error(message: impl Into<String>) -> (StatusCode, Json<ErrorResponse>) {
    let message = message.into();
    tracing::error!("media proxy internal error: {message}");
    error_response(StatusCode::INTERNAL_SERVER_ERROR, "Local media proxy failed")
}

fn error_response(status: StatusCode, message: &str) -> (StatusCode, Json<ErrorResponse>) {
    (
        status,
        Json(ErrorResponse {
            error: message.to_string(),
            code: status.as_u16(),
        }),
    )
}
