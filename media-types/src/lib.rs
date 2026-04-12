//! Shared request and response types for StaticFlow local-media APIs.
#![allow(
    missing_docs,
    reason = "shared protocol types are intentionally compact and self-describing"
)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LocalMediaEntryKind {
    Directory,
    Video,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LocalMediaEntry {
    pub kind: LocalMediaEntryKind,
    pub name: String,
    pub relative_path: String,
    pub size_bytes: Option<u64>,
    pub modified_at_ms: Option<i64>,
    pub extension: Option<String>,
    pub poster_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalMediaListResponse {
    pub configured: bool,
    pub current_dir: String,
    pub parent_dir: Option<String>,
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
    pub has_more: bool,
    pub entries: Vec<LocalMediaEntry>,
}

impl LocalMediaListResponse {
    pub fn unconfigured(limit: usize, offset: usize) -> Self {
        Self {
            configured: false,
            current_dir: String::new(),
            parent_dir: None,
            total: 0,
            offset,
            limit,
            has_more: false,
            entries: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalMediaListQuery {
    #[serde(default)]
    pub dir: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub offset: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenPlaybackRequest {
    pub file: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackStatus {
    Ready,
    Preparing,
    Failed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackMode {
    Raw,
    Hls,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaybackOpenResponse {
    pub status: PlaybackStatus,
    pub mode: Option<PlaybackMode>,
    pub job_id: Option<String>,
    pub player_url: Option<String>,
    pub title: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaybackJobStatusResponse {
    pub job_id: String,
    pub status: PlaybackStatus,
    pub mode: Option<PlaybackMode>,
    pub player_url: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawPlaybackQuery {
    pub file: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PosterQuery {
    pub file: String,
}
