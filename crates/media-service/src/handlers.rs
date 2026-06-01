//! HTTP handlers for the standalone media service.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Json, Response},
};
use serde::Serialize;

use crate::{
    fs::{list_directory, normalize_relative_path},
    playback::{
        get_job_status, open_playback, stream_hls_artifact, stream_mp4_artifact, stream_raw_file,
    },
    poster::stream_or_generate_poster,
    state::LocalMediaState,
    types::{
        CreateUploadTaskRequest, CreateUploadTaskResponse, ListUploadTasksQuery,
        ListUploadTasksResponse, LocalMediaListQuery, LocalMediaListResponse, OpenPlaybackRequest,
        PlaybackJobStatusResponse, PlaybackOpenResponse, PosterQuery, RawPlaybackQuery,
        UploadChunkQuery, UploadChunkResponse, UploadTaskRecord,
    },
    upload::UploadError,
};

type HandlerResult<T> = Result<T, (StatusCode, Json<ErrorResponse>)>;

#[derive(Debug, Clone, Serialize)]
pub struct ErrorResponse {
    pub error: String,
    pub code: u16,
}

pub async fn list_local_media(
    State(state): State<Arc<LocalMediaState>>,
    Query(query): Query<LocalMediaListQuery>,
) -> HandlerResult<Json<LocalMediaListResponse>> {
    let limit = query
        .limit
        .unwrap_or(state.config().list_page_size)
        .clamp(1, 500);
    let offset = query.offset.unwrap_or(0);
    let response = list_directory(state.as_ref(), query.dir.as_deref(), limit, offset)
        .await
        .map_err(internal_error)?;
    Ok(Json(response))
}

pub async fn open_local_media_playback(
    State(state): State<Arc<LocalMediaState>>,
    Json(request): Json<OpenPlaybackRequest>,
) -> HandlerResult<Json<PlaybackOpenResponse>> {
    let normalized_file = normalize_relative_path(&request.file).map_err(internal_error)?;
    let response = open_playback(state, OpenPlaybackRequest {
        file: normalized_file,
    })
    .await
    .map_err(internal_error)?;
    Ok(Json(response))
}

pub async fn get_local_media_job_status(
    State(state): State<Arc<LocalMediaState>>,
    Path(job_id): Path<String>,
) -> HandlerResult<Json<PlaybackJobStatusResponse>> {
    let status = get_job_status(state, &job_id)
        .await
        .map_err(internal_error)?;
    match status {
        Some(status) => Ok(Json(status)),
        None => Err(error_response(StatusCode::NOT_FOUND, "Playback job not found")),
    }
}

pub async fn stream_local_media_raw(
    State(state): State<Arc<LocalMediaState>>,
    Query(query): Query<RawPlaybackQuery>,
    headers: HeaderMap,
) -> HandlerResult<Response> {
    let normalized_file = normalize_relative_path(&query.file).map_err(internal_error)?;
    stream_raw_file(state, &normalized_file, &headers)
        .await
        .map_err(internal_error)
}

pub async fn stream_local_media_hls_artifact(
    State(state): State<Arc<LocalMediaState>>,
    Path((job_id, file_name)): Path<(String, String)>,
    headers: HeaderMap,
) -> HandlerResult<Response> {
    if file_name.contains('/') || file_name.contains('\\') {
        return Err(error_response(StatusCode::BAD_REQUEST, "Invalid HLS file name"));
    }
    stream_hls_artifact(state, &job_id, &file_name, &headers)
        .await
        .map_err(internal_error)
}

pub async fn stream_local_media_mp4_artifact(
    State(state): State<Arc<LocalMediaState>>,
    Path((job_id, file_name)): Path<(String, String)>,
    headers: HeaderMap,
) -> HandlerResult<Response> {
    if file_name.contains('/') || file_name.contains('\\') {
        return Err(error_response(StatusCode::BAD_REQUEST, "Invalid MP4 file name"));
    }
    stream_mp4_artifact(state, &job_id, &file_name, &headers)
        .await
        .map_err(internal_error)
}

pub async fn stream_local_media_poster(
    State(state): State<Arc<LocalMediaState>>,
    Query(query): Query<PosterQuery>,
) -> HandlerResult<Response> {
    let normalized_file = normalize_relative_path(&query.file).map_err(internal_error)?;
    stream_or_generate_poster(state, &normalized_file)
        .await
        .map_err(internal_error)
}

pub async fn create_upload_task(
    State(state): State<Arc<LocalMediaState>>,
    Json(request): Json<CreateUploadTaskRequest>,
) -> HandlerResult<Json<CreateUploadTaskResponse>> {
    let response = crate::upload::create_or_resume_upload_task(state, request)
        .await
        .map_err(upload_error)?;
    Ok(Json(response))
}

pub async fn list_upload_tasks(
    State(state): State<Arc<LocalMediaState>>,
    Query(query): Query<ListUploadTasksQuery>,
) -> HandlerResult<Json<ListUploadTasksResponse>> {
    let response = crate::upload::list_upload_tasks(state, query)
        .await
        .map_err(upload_error)?;
    Ok(Json(response))
}

pub async fn get_upload_task(
    State(state): State<Arc<LocalMediaState>>,
    Path(task_id): Path<String>,
) -> HandlerResult<Json<UploadTaskRecord>> {
    let task = crate::upload::get_upload_task(state, &task_id)
        .await
        .map_err(upload_error)?;
    Ok(Json(task))
}

pub async fn append_upload_chunk(
    State(state): State<Arc<LocalMediaState>>,
    Path(task_id): Path<String>,
    Query(query): Query<UploadChunkQuery>,
    body: axum::body::Bytes,
) -> HandlerResult<Json<UploadChunkResponse>> {
    let task = crate::upload::append_upload_chunk(state, &task_id, query.offset, body)
        .await
        .map_err(upload_error)?;
    Ok(Json(UploadChunkResponse {
        task,
    }))
}

pub async fn delete_upload_task(
    State(state): State<Arc<LocalMediaState>>,
    Path(task_id): Path<String>,
) -> HandlerResult<Json<UploadTaskRecord>> {
    let task = crate::upload::delete_upload_task(state, &task_id)
        .await
        .map_err(upload_error)?;
    Ok(Json(task))
}

fn internal_error(err: impl std::fmt::Display) -> (StatusCode, Json<ErrorResponse>) {
    tracing::error!("media service handler error: {err}");
    error_response(StatusCode::INTERNAL_SERVER_ERROR, "Local media request failed")
}

fn upload_error(err: UploadError) -> (StatusCode, Json<ErrorResponse>) {
    match err {
        UploadError::BadRequest(message) => error_response(StatusCode::BAD_REQUEST, message),
        UploadError::Conflict(message) => error_response(StatusCode::CONFLICT, message),
        UploadError::NotFound(message) => error_response(StatusCode::NOT_FOUND, message),
        UploadError::Internal(err) => internal_error(err),
    }
}

fn error_response(
    status: StatusCode,
    message: impl Into<String>,
) -> (StatusCode, Json<ErrorResponse>) {
    let code = status.as_u16();
    (
        status,
        Json(ErrorResponse {
            error: message.into(),
            code,
        }),
    )
}
