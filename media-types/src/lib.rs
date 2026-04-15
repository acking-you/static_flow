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
    pub duration_seconds: Option<f64>,
    pub detail: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaybackJobStatusResponse {
    pub job_id: String,
    pub status: PlaybackStatus,
    pub mode: Option<PlaybackMode>,
    pub player_url: Option<String>,
    pub duration_seconds: Option<f64>,
    pub detail: Option<String>,
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UploadTaskStatus {
    Created,
    Partial,
    Completed,
    Failed,
    Canceled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateUploadTaskRequest {
    pub target_dir: String,
    pub source_file_name: String,
    pub file_size: u64,
    pub last_modified_ms: i64,
    pub mime_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UploadTaskRecord {
    pub task_id: String,
    pub resume_key: String,
    pub status: UploadTaskStatus,
    pub target_dir: String,
    pub source_file_name: String,
    pub target_file_name: String,
    pub target_relative_path: String,
    pub file_size: u64,
    pub uploaded_bytes: u64,
    pub last_modified_ms: i64,
    pub mime_type: Option<String>,
    pub error: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateUploadTaskResponse {
    pub task: UploadTaskRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ListUploadTasksQuery {
    #[serde(default)]
    pub dir: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub offset: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ListUploadTasksResponse {
    pub tasks: Vec<UploadTaskRecord>,
    pub total: usize,
    pub limit: usize,
    pub offset: usize,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UploadChunkQuery {
    pub offset: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UploadChunkResponse {
    pub task: UploadTaskRecord,
}

#[cfg(test)]
mod upload_type_tests {
    use super::*;

    #[test]
    fn upload_task_status_uses_snake_case_wire_values() {
        assert_eq!(
            serde_json::to_string(&UploadTaskStatus::Partial).expect("serialize"),
            "\"partial\""
        );
    }

    #[test]
    fn create_or_resume_request_round_trips() {
        let request = CreateUploadTaskRequest {
            target_dir: "movies/demo".to_string(),
            source_file_name: "clip.mp4".to_string(),
            file_size: 42,
            last_modified_ms: 1_713_000_000_000,
            mime_type: Some("video/mp4".to_string()),
        };
        let value = serde_json::to_value(&request).expect("serialize");
        let decoded: CreateUploadTaskRequest =
            serde_json::from_value(value).expect("deserialize");
        assert_eq!(decoded.target_dir, "movies/demo");
        assert_eq!(decoded.source_file_name, "clip.mp4");
        assert_eq!(decoded.file_size, 42);
    }
}
