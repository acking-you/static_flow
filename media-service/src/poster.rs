use std::{path::Path, sync::Arc};

use anyhow::{Context, Result};
use axum::{
    body::Body,
    http::{header, StatusCode},
    response::Response,
};
use tokio::fs::{self, File};
use tokio_util::io::ReaderStream;

use crate::{
    cache::{
        build_poster_cache_key, poster_cache_paths, source_modified_at_ms, PosterCacheKeyInput,
    },
    ffmpeg::{build_poster_command, ensure_binary_paths},
    path_guard::resolve_media_path,
    probe::probe_media,
    LocalMediaState,
};

const POSTER_PROFILE: &str = "poster-jpg-v1";
const POSTER_STDERR_LIMIT: usize = 32 * 1024;

pub async fn stream_or_generate_poster(
    state: Arc<LocalMediaState>,
    relative_path: &str,
) -> Result<Response> {
    let source_path = resolve_media_path(state.root_dir(), relative_path)?;
    let metadata = fs::metadata(&source_path)
        .await
        .with_context(|| format!("failed to stat {}", source_path.display()))?;
    if !metadata.is_file() {
        anyhow::bail!("requested media path is not a file");
    }

    let cache_key = build_poster_cache_key(&PosterCacheKeyInput {
        relative_path,
        file_size: metadata.len(),
        modified_at_ms: source_modified_at_ms(&source_path).await?,
        profile: POSTER_PROFILE,
    });
    let cache_paths = poster_cache_paths(state.cache_dir(), &cache_key);

    if poster_ready(&cache_paths) {
        return stream_jpeg(&cache_paths.image).await;
    }

    let _permit = state
        .poster_limiter()
        .clone()
        .acquire_owned()
        .await
        .context("failed to acquire poster generation permit")?;

    if poster_ready(&cache_paths) {
        return stream_jpeg(&cache_paths.image).await;
    }

    fs::create_dir_all(&cache_paths.dir)
        .await
        .with_context(|| format!("failed to create {}", cache_paths.dir.display()))?;
    let _ = fs::remove_file(&cache_paths.error_marker).await;
    let _ = fs::remove_file(&cache_paths.ready_marker).await;

    let bins = ensure_binary_paths(state.config()).await?;
    let probe = probe_media(&bins, &source_path).await?;
    let duration_seconds = probe.duration_seconds.unwrap_or(10.0);
    let seek_seconds = (duration_seconds * 0.1).max(0.0);
    let temp_output = cache_paths.dir.join("poster.jpg.part");
    let mut command = build_poster_command(&bins, &source_path, &temp_output, seek_seconds);

    match run_poster_command(
        &mut command,
        &temp_output,
        &cache_paths.image,
        &cache_paths.ready_marker,
    )
    .await
    {
        Ok(()) => stream_jpeg(&cache_paths.image).await,
        Err(err) => {
            let _ = persist_error(&cache_paths.error_marker, &err.to_string()).await;
            Err(err)
        },
    }
}

async fn run_poster_command(
    command: &mut tokio::process::Command,
    temp_output: &Path,
    final_output: &Path,
    ready_marker: &Path,
) -> Result<()> {
    let mut child = command
        .spawn()
        .context("failed to spawn ffmpeg for poster generation")?;
    let stderr = child.stderr.take();
    let stderr_task = stderr.map(|mut stderr| {
        tokio::spawn(async move {
            let mut output = Vec::new();
            let mut chunk = [0_u8; 4096];
            loop {
                let read = tokio::io::AsyncReadExt::read(&mut stderr, &mut chunk)
                    .await
                    .context("failed to read ffmpeg poster stderr")?;
                if read == 0 {
                    break;
                }
                output.extend_from_slice(&chunk[..read]);
                if output.len() > POSTER_STDERR_LIMIT {
                    let overflow = output.len() - POSTER_STDERR_LIMIT;
                    output.drain(..overflow);
                }
            }
            Ok::<String, anyhow::Error>(String::from_utf8_lossy(&output).into_owned())
        })
    });
    let status = child
        .wait()
        .await
        .context("failed to wait for ffmpeg poster generation")?;
    let stderr = match stderr_task {
        Some(task) => task
            .await
            .context("failed to join ffmpeg poster stderr task")??,
        None => String::new(),
    };
    if !status.success() {
        let stderr = stderr.trim().to_string();
        if stderr.is_empty() {
            anyhow::bail!("ffmpeg poster generation exited with status {}", status);
        }
        anyhow::bail!("ffmpeg poster generation failed: {stderr}");
    }

    fs::rename(temp_output, final_output)
        .await
        .with_context(|| format!("failed to finalize poster {}", final_output.display()))?;
    fs::write(ready_marker, b"ready")
        .await
        .with_context(|| format!("failed to write {}", ready_marker.display()))?;
    Ok(())
}

fn poster_ready(paths: &crate::cache::PosterCachePaths) -> bool {
    paths.image.exists() && paths.ready_marker.exists() && !paths.error_marker.exists()
}

async fn persist_error(path: &Path, message: &str) -> Result<()> {
    fs::write(path, message)
        .await
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

async fn stream_jpeg(path: &Path) -> Result<Response> {
    let metadata = fs::metadata(path)
        .await
        .with_context(|| format!("failed to stat {}", path.display()))?;
    let file = File::open(path)
        .await
        .with_context(|| format!("failed to open {}", path.display()))?;
    let stream = ReaderStream::new(file);
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "image/jpeg")
        .header(header::CONTENT_LENGTH, metadata.len().to_string())
        .header(header::CACHE_CONTROL, "private, max-age=86400")
        .body(Body::from_stream(stream))
        .expect("valid jpeg response"))
}
