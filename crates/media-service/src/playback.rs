use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result};
use axum::{
    body::Body,
    http::{header, HeaderMap, StatusCode},
    response::Response,
};
use tokio::{
    fs::{self, File},
    io::{AsyncReadExt, AsyncSeekExt, SeekFrom},
    process::{Child, ChildStderr},
};
use tokio_util::io::ReaderStream;

use crate::{
    cache::{
        build_cache_key, hls_cache_paths, mp4_cache_paths, source_modified_at_ms, CacheKeyInput,
        HlsCachePaths, Mp4CachePaths,
    },
    ffmpeg::{build_hls_command, build_mp4_remux_command, ensure_binary_paths},
    jobs::PlaybackJobHandle,
    path_guard::resolve_media_path,
    probe::{
        cache_profile_for_strategy, choose_playback_strategy, mode_for_strategy, probe_media,
        PlaybackStrategy,
    },
    types::{
        OpenPlaybackRequest, PlaybackJobStatusResponse, PlaybackMode, PlaybackOpenResponse,
        PlaybackStatus,
    },
    LocalMediaState,
};

const MAX_CHILD_STDERR_BYTES: usize = 64 * 1024;

pub async fn open_playback(
    state: Arc<LocalMediaState>,
    request: OpenPlaybackRequest,
) -> Result<PlaybackOpenResponse> {
    let source_path = resolve_media_path(state.root_dir(), &request.file)?;
    let source_metadata = fs::metadata(&source_path)
        .await
        .with_context(|| format!("failed to stat {}", source_path.display()))?;
    if !source_metadata.is_file() {
        anyhow::bail!("requested media path is not a file");
    }

    let bins = ensure_binary_paths(state.config()).await?;
    let probe = probe_media(&bins, &source_path).await?;
    let strategy = choose_playback_strategy(&source_path, &probe);
    let title = source_path
        .file_name()
        .and_then(|value| value.to_str())
        .map(ToString::to_string)
        .unwrap_or_else(|| request.file.clone());
    let duration_seconds = probe.duration_seconds;
    let detail = Some(initial_playback_detail(&state, strategy));

    tracing::info!(
        file = %request.file,
        title = %title,
        strategy = %strategy_name(strategy),
        duration_seconds = duration_seconds.unwrap_or_default(),
        "open local media playback"
    );

    if let PlaybackStrategy::Raw {
        ..
    } = strategy
    {
        let player_url = format!(
            "/admin/local-media/api/playback/raw?file={}",
            urlencoding::encode(&request.file)
        );
        return Ok(PlaybackOpenResponse {
            status: PlaybackStatus::Ready,
            mode: Some(PlaybackMode::Raw),
            job_id: None,
            player_url: Some(player_url),
            title,
            duration_seconds,
            detail: None,
            error: None,
        });
    }

    let modified_at_ms = source_modified_at_ms(&source_path).await?;
    let mode = mode_for_strategy(strategy);
    let cache_key = build_cache_key(&CacheKeyInput {
        relative_path: &request.file,
        file_size: source_metadata.len(),
        modified_at_ms,
        mode,
        profile: cache_profile_for_strategy(strategy),
    });
    if let PlaybackStrategy::Mp4Remux = strategy {
        let cache_paths = mp4_cache_paths(state.cache_dir(), &cache_key);
        if cached_mp4_is_ready(&cache_paths) {
            return Ok(ready_mp4_response(&cache_paths, title, duration_seconds));
        }

        if let Some(existing) = state.jobs().get(&cache_key) {
            let snapshot = existing.snapshot().await;
            return Ok(open_response_from_snapshot(snapshot, title));
        }

        let job =
            PlaybackJobHandle::new(cache_key.clone(), PlaybackMode::Raw, duration_seconds, detail);
        let job_snapshot = job.snapshot().await;
        state.jobs().insert(cache_key.clone(), job.clone());
        spawn_mp4_remux_job(state, job, bins, source_path, cache_paths, probe.has_audio());

        return Ok(open_response_from_snapshot(job_snapshot, title));
    }

    let cache_paths = hls_cache_paths(state.cache_dir(), &cache_key);
    if cached_hls_is_ready(&cache_paths) {
        return Ok(ready_hls_response(&cache_paths, title, duration_seconds, detail));
    }

    if let Some(existing) = state.jobs().get(&cache_key) {
        let snapshot = existing.snapshot().await;
        return Ok(open_response_from_snapshot(snapshot, title));
    }

    let job =
        PlaybackJobHandle::new(cache_key.clone(), PlaybackMode::Hls, duration_seconds, detail);
    let job_snapshot = job.snapshot().await;
    state.jobs().insert(cache_key.clone(), job.clone());
    spawn_hls_job(state, job, bins, source_path, cache_paths, strategy, probe.has_audio());

    Ok(open_response_from_snapshot(job_snapshot, title))
}

pub async fn get_job_status(
    state: Arc<LocalMediaState>,
    job_id: &str,
) -> Result<Option<PlaybackJobStatusResponse>> {
    let mp4_paths = mp4_cache_paths(state.cache_dir(), job_id);
    if let Some(status) = cached_mp4_status(&mp4_paths).await {
        return Ok(Some(status));
    }
    let cache_paths = hls_cache_paths(state.cache_dir(), job_id);
    if let Some(status) = cached_hls_status(&cache_paths).await {
        return Ok(Some(status));
    }
    if let Some(existing) = state.jobs().get(job_id) {
        return Ok(Some(existing.snapshot().await));
    }
    Ok(None)
}

pub async fn stream_raw_file(
    state: Arc<LocalMediaState>,
    relative_path: &str,
    headers: &HeaderMap,
) -> Result<Response> {
    let source_path = resolve_media_path(state.root_dir(), relative_path)?;
    let mime = mime_guess2::from_path(&source_path)
        .first_raw()
        .unwrap_or("application/octet-stream");
    stream_file_with_range(&source_path, mime, headers).await
}

pub async fn stream_mp4_artifact(
    state: Arc<LocalMediaState>,
    job_id: &str,
    file_name: &str,
    headers: &HeaderMap,
) -> Result<Response> {
    let cache_paths = mp4_cache_paths(state.cache_dir(), job_id);
    let requested = cache_paths.dir.join(file_name);
    if !requested.starts_with(&cache_paths.dir) {
        anyhow::bail!("invalid MP4 artifact path");
    }
    if !requested.exists() {
        anyhow::bail!("requested MP4 artifact does not exist");
    }

    let mut response = stream_file_with_range(&requested, "video/mp4", headers).await?;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, header::HeaderValue::from_static("private, max-age=60"));
    Ok(response)
}

pub async fn stream_hls_artifact(
    state: Arc<LocalMediaState>,
    job_id: &str,
    file_name: &str,
    headers: &HeaderMap,
) -> Result<Response> {
    let cache_paths = hls_cache_paths(state.cache_dir(), job_id);
    let requested = cache_paths.dir.join(file_name);
    if !requested.starts_with(&cache_paths.dir) {
        anyhow::bail!("invalid HLS artifact path");
    }
    if !requested.exists() {
        anyhow::bail!("requested HLS artifact does not exist");
    }

    let content_type = match requested.extension().and_then(|value| value.to_str()) {
        Some("m3u8") => "application/vnd.apple.mpegurl",
        Some("ts") => "video/mp2t",
        _ => "application/octet-stream",
    };
    let mut response = stream_file_with_range(&requested, content_type, headers).await?;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, header::HeaderValue::from_static("private, max-age=60"));
    Ok(response)
}

fn spawn_hls_job(
    state: Arc<LocalMediaState>,
    job: Arc<PlaybackJobHandle>,
    bins: crate::ffmpeg::BinaryPaths,
    source_path: PathBuf,
    cache_paths: HlsCachePaths,
    strategy: PlaybackStrategy,
    has_audio: bool,
) {
    tokio::spawn(async move {
        if state.transcode_limiter().available_permits() == 0 {
            tracing::info!(
                job_id = %cache_paths.job_id,
                source = %source_path.display(),
                "waiting for HLS transcode slot"
            );
        }
        tracing::info!(
            job_id = %cache_paths.job_id,
            source = %source_path.display(),
            strategy = %strategy_name(strategy),
            "starting HLS playback preparation"
        );
        let _permit = match state.transcode_limiter().clone().acquire_owned().await {
            Ok(permit) => permit,
            Err(err) => {
                let message = format!("failed to acquire HLS transcode permit: {err}");
                let _ = persist_error(&cache_paths, &message).await;
                job.mark_failed(message).await;
                state.jobs().remove(job.job_id());
                return;
            },
        };
        job.mark_preparing(active_playback_detail(strategy)).await;

        if let Err(err) = fs::create_dir_all(&cache_paths.dir).await {
            let message = format!("failed to create cache dir: {err}");
            let _ = persist_error(&cache_paths, &message).await;
            job.mark_failed(message).await;
            state.jobs().remove(job.job_id());
            return;
        }
        let _ = fs::remove_file(&cache_paths.ready_marker).await;
        let _ = fs::remove_file(&cache_paths.error_marker).await;

        let mut command =
            build_hls_command(&bins, &source_path, &cache_paths.dir, strategy, has_audio);
        match command.spawn() {
            Ok(child) => {
                match wait_for_child_with_bounded_stderr(child, MAX_CHILD_STDERR_BYTES).await {
                    Ok((status, _stderr)) if status.success() => {
                        if let Err(err) = fs::write(&cache_paths.ready_marker, b"ready").await {
                            let message = format!("failed to persist ready marker: {err}");
                            let _ = persist_error(&cache_paths, &message).await;
                            tracing::error!(job_id = %cache_paths.job_id, error = %message, "HLS playback preparation failed");
                            job.mark_failed(message).await;
                        } else {
                            job.mark_ready(hls_player_url(&cache_paths.job_id)).await;
                            tracing::info!(job_id = %cache_paths.job_id, "HLS playback is ready");
                        }
                    },
                    Ok((status, stderr)) => {
                        let message = if stderr.is_empty() {
                            format!("ffmpeg exited with status {status}")
                        } else {
                            format!("ffmpeg failed: {stderr}")
                        };
                        let _ = persist_error(&cache_paths, &message).await;
                        tracing::error!(job_id = %cache_paths.job_id, error = %message, "HLS playback preparation failed");
                        job.mark_failed(message).await;
                    },
                    Err(err) => {
                        let message = format!("failed to wait for ffmpeg: {err}");
                        let _ = persist_error(&cache_paths, &message).await;
                        tracing::error!(job_id = %cache_paths.job_id, error = %message, "HLS playback preparation failed");
                        job.mark_failed(message).await;
                    },
                }
            },
            Err(err) => {
                let message = format!("failed to spawn ffmpeg: {err}");
                let _ = persist_error(&cache_paths, &message).await;
                tracing::error!(job_id = %cache_paths.job_id, error = %message, "HLS playback preparation failed");
                job.mark_failed(message).await;
            },
        }

        state.jobs().remove(job.job_id());
    });
}

fn spawn_mp4_remux_job(
    state: Arc<LocalMediaState>,
    job: Arc<PlaybackJobHandle>,
    bins: crate::ffmpeg::BinaryPaths,
    source_path: PathBuf,
    cache_paths: Mp4CachePaths,
    has_audio: bool,
) {
    tokio::spawn(async move {
        if state.remux_limiter().available_permits() == 0 {
            tracing::info!(
                job_id = %cache_paths.job_id,
                source = %source_path.display(),
                "waiting for MP4 remux slot"
            );
        }
        tracing::info!(
            job_id = %cache_paths.job_id,
            source = %source_path.display(),
            "starting MP4 remux playback preparation"
        );
        let _permit = match state.remux_limiter().clone().acquire_owned().await {
            Ok(permit) => permit,
            Err(err) => {
                let message = format!("failed to acquire MP4 remux permit: {err}");
                let _ = persist_mp4_error(&cache_paths, &message).await;
                job.mark_failed(message).await;
                state.jobs().remove(job.job_id());
                return;
            },
        };
        job.mark_preparing(active_playback_detail(PlaybackStrategy::Mp4Remux))
            .await;

        if let Err(err) = fs::create_dir_all(&cache_paths.dir).await {
            let message = format!("failed to create cache dir: {err}");
            let _ = persist_mp4_error(&cache_paths, &message).await;
            job.mark_failed(message).await;
            state.jobs().remove(job.job_id());
            return;
        }
        let _ = fs::remove_file(&cache_paths.ready_marker).await;
        let _ = fs::remove_file(&cache_paths.error_marker).await;

        let mut command =
            build_mp4_remux_command(&bins, &source_path, &cache_paths.video, has_audio);
        match command.spawn() {
            Ok(child) => {
                match wait_for_child_with_bounded_stderr(child, MAX_CHILD_STDERR_BYTES).await {
                    Ok((status, _stderr)) if status.success() => {
                        if let Err(err) = fs::write(&cache_paths.ready_marker, b"ready").await {
                            let message = format!("failed to persist ready marker: {err}");
                            let _ = persist_mp4_error(&cache_paths, &message).await;
                            tracing::error!(job_id = %cache_paths.job_id, error = %message, "MP4 remux preparation failed");
                            job.mark_failed(message).await;
                        } else {
                            job.mark_ready(mp4_player_url(&cache_paths.job_id)).await;
                            tracing::info!(job_id = %cache_paths.job_id, "MP4 remux playback is ready");
                        }
                    },
                    Ok((status, stderr)) => {
                        let message = if stderr.is_empty() {
                            format!("ffmpeg exited with status {status}")
                        } else {
                            format!("ffmpeg failed: {stderr}")
                        };
                        let _ = persist_mp4_error(&cache_paths, &message).await;
                        tracing::error!(job_id = %cache_paths.job_id, error = %message, "MP4 remux preparation failed");
                        job.mark_failed(message).await;
                    },
                    Err(err) => {
                        let message = format!("failed to wait for ffmpeg: {err}");
                        let _ = persist_mp4_error(&cache_paths, &message).await;
                        tracing::error!(job_id = %cache_paths.job_id, error = %message, "MP4 remux preparation failed");
                        job.mark_failed(message).await;
                    },
                }
            },
            Err(err) => {
                let message = format!("failed to spawn ffmpeg: {err}");
                let _ = persist_mp4_error(&cache_paths, &message).await;
                tracing::error!(job_id = %cache_paths.job_id, error = %message, "MP4 remux preparation failed");
                job.mark_failed(message).await;
            },
        }

        state.jobs().remove(job.job_id());
    });
}

fn open_response_from_snapshot(
    snapshot: PlaybackJobStatusResponse,
    title: String,
) -> PlaybackOpenResponse {
    PlaybackOpenResponse {
        status: snapshot.status,
        mode: snapshot.mode,
        job_id: Some(snapshot.job_id),
        player_url: snapshot.player_url,
        title,
        duration_seconds: snapshot.duration_seconds,
        detail: snapshot.detail,
        error: snapshot.error,
    }
}

fn ready_hls_response(
    cache_paths: &HlsCachePaths,
    title: String,
    duration_seconds: Option<f64>,
    detail: Option<String>,
) -> PlaybackOpenResponse {
    PlaybackOpenResponse {
        status: PlaybackStatus::Ready,
        mode: Some(PlaybackMode::Hls),
        job_id: Some(cache_paths.job_id.clone()),
        player_url: Some(hls_player_url(&cache_paths.job_id)),
        title,
        duration_seconds,
        detail,
        error: None,
    }
}

fn ready_mp4_response(
    cache_paths: &Mp4CachePaths,
    title: String,
    duration_seconds: Option<f64>,
) -> PlaybackOpenResponse {
    PlaybackOpenResponse {
        status: PlaybackStatus::Ready,
        mode: Some(PlaybackMode::Raw),
        job_id: Some(cache_paths.job_id.clone()),
        player_url: Some(mp4_player_url(&cache_paths.job_id)),
        title,
        duration_seconds,
        detail: Some(playback_detail(PlaybackStrategy::Mp4Remux).to_string()),
        error: None,
    }
}

fn hls_player_url(job_id: &str) -> String {
    format!("/admin/local-media/api/playback/hls/{job_id}/index.m3u8")
}

fn mp4_player_url(job_id: &str) -> String {
    format!("/admin/local-media/api/playback/mp4/{job_id}/output.mp4")
}

fn initial_playback_detail(state: &LocalMediaState, strategy: PlaybackStrategy) -> String {
    match strategy {
        PlaybackStrategy::Raw {
            ..
        } => playback_detail(strategy).to_string(),
        PlaybackStrategy::Mp4Remux => {
            if state.remux_limiter().available_permits() == 0 {
                "Waiting for another MP4 copy job to finish before this file can start.".to_string()
            } else {
                "Starting an MP4 copy so duration and scrubbing work normally.".to_string()
            }
        },
        PlaybackStrategy::HlsCopy | PlaybackStrategy::HlsTranscode => {
            if state.transcode_limiter().available_permits() == 0 {
                "Waiting for another playback transcode job to finish before this file can start."
                    .to_string()
            } else {
                playback_detail(strategy).to_string()
            }
        },
    }
}

fn active_playback_detail(strategy: PlaybackStrategy) -> String {
    match strategy {
        PlaybackStrategy::Mp4Remux => "Copying the MKV streams into MP4 now. Playback will unlock \
                                       when the copy finishes."
            .to_string(),
        PlaybackStrategy::HlsCopy | PlaybackStrategy::HlsTranscode => {
            "Transcoding this file into HLS now. Playback will unlock as soon as the stream is \
             ready."
                .to_string()
        },
        PlaybackStrategy::Raw {
            ..
        } => playback_detail(strategy).to_string(),
    }
}

fn cached_hls_is_ready(cache_paths: &HlsCachePaths) -> bool {
    cache_paths.playlist.exists() && !cache_paths.error_marker.exists()
}

fn cached_mp4_is_ready(cache_paths: &Mp4CachePaths) -> bool {
    cache_paths.ready_marker.exists()
        && cache_paths.video.exists()
        && !cache_paths.error_marker.exists()
}

async fn cached_mp4_status(cache_paths: &Mp4CachePaths) -> Option<PlaybackJobStatusResponse> {
    if cache_paths.ready_marker.exists() && cache_paths.video.exists() {
        return Some(PlaybackJobStatusResponse {
            job_id: cache_paths.job_id.clone(),
            status: PlaybackStatus::Ready,
            mode: Some(PlaybackMode::Raw),
            player_url: Some(mp4_player_url(&cache_paths.job_id)),
            duration_seconds: None,
            detail: Some(playback_detail(PlaybackStrategy::Mp4Remux).to_string()),
            error: None,
        });
    }
    if cache_paths.error_marker.exists() {
        let error = fs::read_to_string(&cache_paths.error_marker)
            .await
            .unwrap_or_else(|_| "failed to read playback error".to_string());
        return Some(PlaybackJobStatusResponse {
            job_id: cache_paths.job_id.clone(),
            status: PlaybackStatus::Failed,
            mode: Some(PlaybackMode::Raw),
            player_url: None,
            duration_seconds: None,
            detail: Some(playback_detail(PlaybackStrategy::Mp4Remux).to_string()),
            error: Some(error.trim().to_string()),
        });
    }
    None
}

async fn cached_hls_status(cache_paths: &HlsCachePaths) -> Option<PlaybackJobStatusResponse> {
    if cache_paths.ready_marker.exists() && cache_paths.playlist.exists() {
        return Some(PlaybackJobStatusResponse {
            job_id: cache_paths.job_id.clone(),
            status: PlaybackStatus::Ready,
            mode: Some(PlaybackMode::Hls),
            player_url: Some(hls_player_url(&cache_paths.job_id)),
            duration_seconds: None,
            detail: Some(playback_detail(PlaybackStrategy::HlsTranscode).to_string()),
            error: None,
        });
    }
    if cache_paths.error_marker.exists() {
        let error = fs::read_to_string(&cache_paths.error_marker)
            .await
            .unwrap_or_else(|_| "failed to read playback error".to_string());
        return Some(PlaybackJobStatusResponse {
            job_id: cache_paths.job_id.clone(),
            status: PlaybackStatus::Failed,
            mode: Some(PlaybackMode::Hls),
            player_url: None,
            duration_seconds: None,
            detail: Some(playback_detail(PlaybackStrategy::HlsTranscode).to_string()),
            error: Some(error.trim().to_string()),
        });
    }
    if cache_paths.playlist.exists() {
        return Some(PlaybackJobStatusResponse {
            job_id: cache_paths.job_id.clone(),
            status: PlaybackStatus::Ready,
            mode: Some(PlaybackMode::Hls),
            player_url: Some(hls_player_url(&cache_paths.job_id)),
            duration_seconds: None,
            detail: Some(playback_detail(PlaybackStrategy::HlsTranscode).to_string()),
            error: None,
        });
    }
    None
}

async fn persist_error(cache_paths: &HlsCachePaths, message: &str) -> Result<()> {
    fs::create_dir_all(&cache_paths.dir)
        .await
        .with_context(|| format!("failed to create {}", cache_paths.dir.display()))?;
    fs::write(&cache_paths.error_marker, message)
        .await
        .with_context(|| format!("failed to write {}", cache_paths.error_marker.display()))?;
    Ok(())
}

async fn persist_mp4_error(cache_paths: &Mp4CachePaths, message: &str) -> Result<()> {
    fs::create_dir_all(&cache_paths.dir)
        .await
        .with_context(|| format!("failed to create {}", cache_paths.dir.display()))?;
    fs::write(&cache_paths.error_marker, message)
        .await
        .with_context(|| format!("failed to write {}", cache_paths.error_marker.display()))?;
    Ok(())
}

fn strategy_name(strategy: PlaybackStrategy) -> &'static str {
    match strategy {
        PlaybackStrategy::Raw {
            ..
        } => "raw",
        PlaybackStrategy::Mp4Remux => "mp4-remux",
        PlaybackStrategy::HlsCopy => "hls-copy",
        PlaybackStrategy::HlsTranscode => "hls-transcode",
    }
}

fn playback_detail(strategy: PlaybackStrategy) -> &'static str {
    match strategy {
        PlaybackStrategy::Raw {
            ..
        } => "Direct playback is available.",
        PlaybackStrategy::Mp4Remux => {
            "Preparing a browser-seekable MP4 copy so duration and scrubbing work normally."
        },
        PlaybackStrategy::HlsCopy => {
            "Preparing an HLS stream from browser-friendly codecs so playback can start before the \
             cache fully completes."
        },
        PlaybackStrategy::HlsTranscode => {
            "Transcoding to browser-safe HLS. Playback becomes available before the cache fully \
             completes, but full seek freedom depends on generated segments."
        },
    }
}

async fn wait_for_child_with_bounded_stderr(
    mut child: Child,
    stderr_limit: usize,
) -> Result<(std::process::ExitStatus, String)> {
    let stderr_task = child.stderr.take().map(|stderr| {
        tokio::spawn(async move { collect_bounded_child_stderr(stderr, stderr_limit).await })
    });
    let status = child.wait().await.context("failed to wait for ffmpeg")?;
    let stderr = match stderr_task {
        Some(task) => task
            .await
            .context("failed to join stderr collection task")??,
        None => String::new(),
    };
    Ok((status, stderr.trim().to_string()))
}

async fn collect_bounded_child_stderr(mut stderr: ChildStderr, limit: usize) -> Result<String> {
    let mut output = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let read = stderr
            .read(&mut chunk)
            .await
            .context("failed to read ffmpeg stderr")?;
        if read == 0 {
            break;
        }
        output.extend_from_slice(&chunk[..read]);
        if output.len() > limit {
            let overflow = output.len() - limit;
            output.drain(..overflow);
        }
    }
    Ok(String::from_utf8_lossy(&output).into_owned())
}

async fn stream_file_with_range(
    path: &Path,
    content_type: &str,
    headers: &HeaderMap,
) -> Result<Response> {
    let metadata = fs::metadata(path)
        .await
        .with_context(|| format!("failed to stat {}", path.display()))?;
    let total_len = metadata.len();
    let file = File::open(path)
        .await
        .with_context(|| format!("failed to open {}", path.display()))?;

    if let Some(range_str) = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
    {
        if let Some((start, end)) = parse_range_header(range_str, total_len) {
            return build_partial_response(file, content_type, total_len, start, end).await;
        }
    }

    build_full_response(file, content_type, total_len).await
}

async fn build_partial_response(
    mut file: File,
    content_type: &str,
    total_len: u64,
    start: u64,
    end: u64,
) -> Result<Response> {
    file.seek(SeekFrom::Start(start))
        .await
        .context("failed to seek media file")?;
    let len = end.saturating_sub(start).saturating_add(1);
    let stream = ReaderStream::new(file.take(len));
    Ok(Response::builder()
        .status(StatusCode::PARTIAL_CONTENT)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, len.to_string())
        .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{total_len}"))
        .body(Body::from_stream(stream))
        .expect("valid partial response"))
}

async fn build_full_response(file: File, content_type: &str, total_len: u64) -> Result<Response> {
    let stream = ReaderStream::new(file);
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, total_len.to_string())
        .body(Body::from_stream(stream))
        .expect("valid full response"))
}

fn parse_range_header(range_str: &str, total: u64) -> Option<(u64, u64)> {
    let range_str = range_str.strip_prefix("bytes=")?;
    let mut parts = range_str.splitn(2, '-');
    let start_str = parts.next()?.trim();
    let end_str = parts.next().unwrap_or("").trim();
    if start_str.is_empty() {
        let suffix_len = end_str.parse::<u64>().ok()?;
        if suffix_len == 0 {
            return None;
        }
        let start = total.saturating_sub(suffix_len);
        return Some((start, total.saturating_sub(1)));
    }
    let start = start_str.parse::<u64>().ok()?;
    if start >= total {
        return None;
    }
    let end = if end_str.is_empty() {
        total.saturating_sub(1)
    } else {
        end_str.parse::<u64>().ok()?.min(total.saturating_sub(1))
    };
    if start > end {
        return None;
    }
    Some((start, end))
}

#[cfg(test)]
mod tests {
    use std::process::Stdio;

    use tempfile::tempdir;
    use tokio::{fs, process::Command};

    use super::{cached_hls_status, parse_range_header, wait_for_child_with_bounded_stderr};
    use crate::{
        cache::hls_cache_paths,
        types::{PlaybackMode, PlaybackStatus},
    };

    #[tokio::test]
    async fn cached_hls_status_treats_incremental_playlist_as_ready() {
        let tempdir = tempdir().expect("tempdir");
        let cache_paths = hls_cache_paths(tempdir.path(), "job-1");
        fs::create_dir_all(&cache_paths.dir)
            .await
            .expect("cache dir");
        fs::write(&cache_paths.playlist, "#EXTM3U\n")
            .await
            .expect("playlist");

        let status = cached_hls_status(&cache_paths).await.expect("status");
        assert_eq!(status.job_id, "job-1");
        assert_eq!(status.status, PlaybackStatus::Ready);
        assert_eq!(status.mode, Some(PlaybackMode::Hls));
        assert_eq!(
            status.player_url.as_deref(),
            Some("/admin/local-media/api/playback/hls/job-1/index.m3u8")
        );
    }

    #[tokio::test]
    async fn cached_hls_status_prefers_error_marker_over_playlist() {
        let tempdir = tempdir().expect("tempdir");
        let cache_paths = hls_cache_paths(tempdir.path(), "job-2");
        fs::create_dir_all(&cache_paths.dir)
            .await
            .expect("cache dir");
        fs::write(&cache_paths.playlist, "#EXTM3U\n")
            .await
            .expect("playlist");
        fs::write(&cache_paths.error_marker, "ffmpeg failed")
            .await
            .expect("error marker");

        let status = cached_hls_status(&cache_paths).await.expect("status");
        assert_eq!(status.job_id, "job-2");
        assert_eq!(status.status, PlaybackStatus::Failed);
        assert_eq!(status.mode, Some(PlaybackMode::Hls));
        assert_eq!(status.error.as_deref(), Some("ffmpeg failed"));
        assert!(status.player_url.is_none());
    }

    #[test]
    fn parse_range_header_supports_suffix_ranges() {
        assert_eq!(parse_range_header("bytes=-500", 1000), Some((500, 999)));
        assert_eq!(parse_range_header("bytes=-1500", 1000), Some((0, 999)));
    }

    #[tokio::test]
    async fn wait_for_child_with_bounded_stderr_keeps_only_recent_output() {
        let script = "python3 - <<'PY'\nimport sys\nsys.stderr.write('A' * 80000)\nPY";
        let child = Command::new("sh")
            .arg("-c")
            .arg(script)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn stderr writer");

        let (status, collected) = wait_for_child_with_bounded_stderr(child, 16 * 1024)
            .await
            .expect("wait child");
        assert!(status.success());
        assert_eq!(collected.len(), 16 * 1024);
        assert!(collected.chars().all(|ch| ch == 'A'));
    }
}
