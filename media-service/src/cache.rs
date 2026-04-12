use std::{
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

use crate::types::PlaybackMode;

#[derive(Debug, Clone)]
pub struct CacheKeyInput<'a> {
    pub relative_path: &'a str,
    pub file_size: u64,
    pub modified_at_ms: i64,
    pub mode: PlaybackMode,
    pub profile: &'a str,
}

#[derive(Debug, Clone)]
pub struct HlsCachePaths {
    pub job_id: String,
    pub dir: PathBuf,
    pub playlist: PathBuf,
    pub ready_marker: PathBuf,
    pub error_marker: PathBuf,
}

#[derive(Debug, Clone)]
pub struct PosterCacheKeyInput<'a> {
    pub relative_path: &'a str,
    pub file_size: u64,
    pub modified_at_ms: i64,
    pub profile: &'a str,
}

#[derive(Debug, Clone)]
pub struct PosterCachePaths {
    pub dir: PathBuf,
    pub image: PathBuf,
    pub ready_marker: PathBuf,
    pub error_marker: PathBuf,
}

pub fn build_cache_key(input: &CacheKeyInput<'_>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.relative_path.as_bytes());
    hasher.update(b"\0");
    hasher.update(input.file_size.to_le_bytes());
    hasher.update(input.modified_at_ms.to_le_bytes());
    hasher.update(match input.mode {
        PlaybackMode::Raw => b"raw".as_slice(),
        PlaybackMode::Hls => b"hls".as_slice(),
    });
    hasher.update(b"\0");
    hasher.update(input.profile.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub fn hls_cache_paths(cache_root: &Path, job_id: &str) -> HlsCachePaths {
    let dir = cache_root.join(job_id);
    HlsCachePaths {
        job_id: job_id.to_string(),
        playlist: dir.join("index.m3u8"),
        ready_marker: dir.join(".ready"),
        error_marker: dir.join(".error.txt"),
        dir,
    }
}

pub fn build_poster_cache_key(input: &PosterCacheKeyInput<'_>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.relative_path.as_bytes());
    hasher.update(b"\0");
    hasher.update(input.file_size.to_le_bytes());
    hasher.update(input.modified_at_ms.to_le_bytes());
    hasher.update(b"poster");
    hasher.update(b"\0");
    hasher.update(input.profile.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub fn poster_cache_paths(cache_root: &Path, cache_key: &str) -> PosterCachePaths {
    let dir = cache_root.join(cache_key);
    PosterCachePaths {
        image: dir.join("poster.jpg"),
        ready_marker: dir.join(".ready"),
        error_marker: dir.join(".error.txt"),
        dir,
    }
}

pub async fn source_modified_at_ms(path: &Path) -> Result<i64> {
    let metadata = tokio::fs::metadata(path)
        .await
        .with_context(|| format!("failed to stat {}", path.display()))?;
    let modified = metadata
        .modified()
        .with_context(|| format!("failed to read modified time for {}", path.display()))?;
    let duration = modified
        .duration_since(UNIX_EPOCH)
        .with_context(|| format!("invalid modified time for {}", path.display()))?;
    Ok(duration.as_millis().try_into().unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::poster_cache_paths;

    #[test]
    fn poster_cache_paths_use_jpg_artifacts() {
        let cache_root = PathBuf::from("/tmp/local-media-cache");
        let paths = poster_cache_paths(&cache_root, "poster-key");
        assert_eq!(paths.dir, cache_root.join("poster-key"));
        assert_eq!(paths.image, cache_root.join("poster-key").join("poster.jpg"));
        assert_eq!(paths.ready_marker, cache_root.join("poster-key").join(".ready"));
        assert_eq!(paths.error_marker, cache_root.join("poster-key").join(".error.txt"));
    }
}
