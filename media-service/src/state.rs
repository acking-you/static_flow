//! Shared runtime state for the standalone media service.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result};
use dashmap::DashMap;
use tokio::{fs as tokio_fs, sync::Semaphore};

use crate::{
    config::{read_local_media_config_from_env, LocalMediaConfig},
    jobs::PlaybackJobHandle,
};

#[derive(Clone)]
pub struct LocalMediaState {
    config: LocalMediaConfig,
    root_dir: PathBuf,
    cache_dir: PathBuf,
    transcode_limiter: Arc<Semaphore>,
    poster_limiter: Arc<Semaphore>,
    jobs: Arc<DashMap<String, Arc<PlaybackJobHandle>>>,
}

impl LocalMediaState {
    pub async fn from_env() -> Result<Option<Arc<Self>>> {
        let config = read_local_media_config_from_env()?;
        if !config.enabled {
            tracing::info!("local media feature disabled by environment");
            return Ok(None);
        }

        let Some(root_dir) = config.root.clone() else {
            tracing::info!("local media root is not configured; feature stays inactive");
            return Ok(None);
        };

        let root_dir = tokio_fs::canonicalize(&root_dir).await.with_context(|| {
            format!("failed to canonicalize local media root {}", root_dir.display())
        })?;
        let metadata = tokio_fs::metadata(&root_dir)
            .await
            .with_context(|| format!("failed to stat local media root {}", root_dir.display()))?;
        if !metadata.is_dir() {
            anyhow::bail!("local media root is not a directory: {}", root_dir.display());
        }

        tokio_fs::create_dir_all(&config.cache_dir)
            .await
            .with_context(|| {
                format!(
                    "failed to create local media cache directory {}",
                    config.cache_dir.display()
                )
            })?;
        let cache_dir = tokio_fs::canonicalize(&config.cache_dir)
            .await
            .with_context(|| {
                format!(
                    "failed to canonicalize local media cache directory {}",
                    config.cache_dir.display()
                )
            })?;

        tracing::info!(
            root_dir = %root_dir.display(),
            cache_dir = %cache_dir.display(),
            max_transcode_jobs = config.max_transcode_jobs,
            max_poster_jobs = config.max_poster_jobs,
            auto_download_ffmpeg = config.auto_download_ffmpeg,
            "local media feature initialized"
        );

        Ok(Some(Arc::new(Self {
            transcode_limiter: Arc::new(Semaphore::new(config.max_transcode_jobs)),
            poster_limiter: Arc::new(Semaphore::new(config.max_poster_jobs)),
            jobs: Arc::new(DashMap::new()),
            config,
            root_dir,
            cache_dir,
        })))
    }

    pub fn config(&self) -> &LocalMediaConfig {
        &self.config
    }

    pub fn root_dir(&self) -> &Path {
        &self.root_dir
    }

    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    pub fn transcode_limiter(&self) -> &Arc<Semaphore> {
        &self.transcode_limiter
    }

    pub fn poster_limiter(&self) -> &Arc<Semaphore> {
        &self.poster_limiter
    }

    pub fn jobs(&self) -> &Arc<DashMap<String, Arc<PlaybackJobHandle>>> {
        &self.jobs
    }
}

#[cfg(test)]
impl LocalMediaState {
    pub fn new_for_test(root_dir: PathBuf, cache_dir: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            transcode_limiter: Arc::new(Semaphore::new(1)),
            poster_limiter: Arc::new(Semaphore::new(1)),
            jobs: Arc::new(DashMap::new()),
            config: LocalMediaConfig {
                enabled: true,
                root: Some(root_dir.clone()),
                cache_dir: cache_dir.clone(),
                auto_download_ffmpeg: false,
                max_transcode_jobs: 1,
                max_poster_jobs: 1,
                list_page_size: 120,
                ffmpeg_bin: None,
                ffprobe_bin: None,
            },
            root_dir,
            cache_dir,
        })
    }
}
