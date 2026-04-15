# Admin Local Media Upload Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add resumable admin-only video uploads to `/admin/local-media`, storing files into the currently browsed directory, persisting task state on disk under the media root, and allowing browser-restart recovery by re-selecting the same local file.

**Architecture:** `static-flow-media` becomes the upload source of truth with disk-backed task metadata plus staged partial bytes under `<media_root>/.static-flow/uploads/<task_id>/`. `static-flow-backend` remains an authenticated reverse proxy, and the frontend becomes a queue plus chunk sender that observes durable server task state instead of inventing its own upload truth.

**Tech Stack:** Axum, Tokio async filesystem IO, reqwest proxying, Yew, `web-sys` file APIs, serde JSON, Rust unit/integration tests.

---

## File Map

- `media-types/src/lib.rs`
  Upload wire protocol shared by frontend, backend, and media-service.
- `media-service/src/state.rs`
  Upload path helpers and per-task write lock registry.
- `media-service/Cargo.toml`
  Upload-specific dependencies for typed errors and task IDs.
- `media-service/src/upload_store.rs`
  Disk-backed task metadata IO under `<media_root>/.static-flow/uploads`.
- `media-service/src/upload.rs`
  Create/resume, append, finalize, list/get/delete upload tasks.
- `media-service/src/handlers.rs`
  HTTP handlers for upload task creation, listing, chunk append, and delete.
- `media-service/src/routes.rs`
  Internal upload route registration.
- `backend/src/media_proxy/forward.rs`
  Backend-to-media-service upload forwarding helpers, including raw chunk body proxying.
- `backend/src/media_proxy/handlers.rs`
  Admin-authenticated upload proxy handlers.
- `backend/src/routes.rs`
  Public admin upload route registration.
- `frontend/src/api.rs`
  Admin local-media upload DTO helpers and HTTP calls.
- `frontend/src/components/admin_local_media_uploads.rs`
  Upload queue UI and chunk sender logic.
- `frontend/src/components/mod.rs`
  Export the new upload component.
- `frontend/src/pages/admin_local_media.rs`
  Compose the upload component above the directory cards and trigger list refresh after completion.
- `frontend/Cargo.toml`
  Add any missing `web-sys` features needed for browser file reads.

## Task 1: Lock Shared Upload Protocol Types

**Files:**
- Modify: `media-types/src/lib.rs`
- Test: `media-types/src/lib.rs`

- [ ] **Step 1: Write the failing protocol tests**

Add focused serde coverage near the end of `media-types/src/lib.rs`:

```rust
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p static-flow-media-types upload_type_tests -- --nocapture`  
Expected: FAIL because upload protocol types are missing or incomplete.

- [ ] **Step 3: Add the minimal shared protocol**

Extend `media-types/src/lib.rs` with the upload DTOs:

```rust
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
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p static-flow-media-types upload_type_tests -- --nocapture`  
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add media-types/src/lib.rs
git commit -m "feat: add local media upload protocol types"
```

## Task 2: Implement Media-Service Upload State And Persistence

**Files:**
- Modify: `media-service/Cargo.toml`
- Modify: `media-service/src/state.rs`
- Create: `media-service/src/upload_store.rs`
- Create: `media-service/src/upload.rs`
- Modify: `media-service/src/lib.rs`
- Test: `media-service/src/upload_store.rs`
- Test: `media-service/src/upload.rs`
- Test: `media-service/src/fs.rs`

- [ ] **Step 1: Write the failing media-service upload tests**

Create focused tests in `media-service/src/upload.rs` and `media-service/src/fs.rs`:

```rust
#[tokio::test]
async fn create_task_reuses_existing_partial_task() {
    let root = tempfile::tempdir().expect("root");
    let cache = tempfile::tempdir().expect("cache");
    let state =
        LocalMediaState::new_for_test(root.path().to_path_buf(), cache.path().to_path_buf());

    let first = create_or_resume_upload_task(
        state.clone(),
        CreateUploadTaskRequest {
            target_dir: String::new(),
            source_file_name: "clip.mp4".to_string(),
            file_size: 10,
            last_modified_ms: 123,
            mime_type: Some("video/mp4".to_string()),
        },
    )
    .await
    .expect("first task");

    let second = create_or_resume_upload_task(
        state.clone(),
        CreateUploadTaskRequest {
            target_dir: String::new(),
            source_file_name: "clip.mp4".to_string(),
            file_size: 10,
            last_modified_ms: 123,
            mime_type: Some("video/mp4".to_string()),
        },
    )
    .await
    .expect("second task");

    assert_eq!(first.task.task_id, second.task.task_id);
}

#[tokio::test]
async fn append_chunk_finalizes_into_target_directory() {
    let root = tempfile::tempdir().expect("root");
    let cache = tempfile::tempdir().expect("cache");
    let state =
        LocalMediaState::new_for_test(root.path().to_path_buf(), cache.path().to_path_buf());
    let created = create_or_resume_upload_task(
        state.clone(),
        CreateUploadTaskRequest {
            target_dir: String::new(),
            source_file_name: "clip.mp4".to_string(),
            file_size: 3,
            last_modified_ms: 1,
            mime_type: Some("video/mp4".to_string()),
        },
    )
    .await
    .expect("task");

    let updated = append_upload_chunk(
        state.clone(),
        &created.task.task_id,
        0,
        axum::body::Bytes::from_static(b"abc"),
    )
    .await
    .expect("append");

    assert_eq!(updated.status, UploadTaskStatus::Completed);
    assert_eq!(std::fs::read(root.path().join("clip.mp4")).expect("final file"), b"abc");
}

#[tokio::test]
async fn delete_task_marks_canceled_and_removes_staged_bytes() {
    let root = tempfile::tempdir().expect("root");
    let cache = tempfile::tempdir().expect("cache");
    let state =
        LocalMediaState::new_for_test(root.path().to_path_buf(), cache.path().to_path_buf());
    let created = create_or_resume_upload_task(
        state.clone(),
        CreateUploadTaskRequest {
            target_dir: String::new(),
            source_file_name: "clip.mp4".to_string(),
            file_size: 8,
            last_modified_ms: 1,
            mime_type: None,
        },
    )
    .await
    .expect("task");
    append_upload_chunk(
        state.clone(),
        &created.task.task_id,
        0,
        axum::body::Bytes::from_static(b"abc"),
    )
    .await
    .expect("append");

    let canceled = delete_upload_task(state.clone(), &created.task.task_id)
        .await
        .expect("delete task");

    assert_eq!(canceled.status, UploadTaskStatus::Canceled);
    assert!(
        tokio::fs::metadata(state.upload_task_dir(&created.task.task_id).join("blob.part"))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn get_task_reconciles_uploaded_bytes_from_part_file() {
    let root = tempfile::tempdir().expect("root");
    let cache = tempfile::tempdir().expect("cache");
    let state =
        LocalMediaState::new_for_test(root.path().to_path_buf(), cache.path().to_path_buf());
    let created = create_or_resume_upload_task(
        state.clone(),
        CreateUploadTaskRequest {
            target_dir: String::new(),
            source_file_name: "clip.mp4".to_string(),
            file_size: 10,
            last_modified_ms: 1,
            mime_type: None,
        },
    )
    .await
    .expect("task");
    let task_dir = state.upload_task_dir(&created.task.task_id);
    tokio::fs::write(task_dir.join("blob.part"), b"abcdef")
        .await
        .expect("write staged bytes");

    let task = get_upload_task(state, &created.task.task_id)
        .await
        .expect("get reconciled task");

    assert_eq!(task.uploaded_bytes, 6);
    assert_eq!(task.status, UploadTaskStatus::Partial);
}

#[test]
fn build_entry_from_dir_entry_skips_hidden_service_state() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join(".static-flow")).expect("service dir");
    let entry = std::fs::read_dir(dir.path())
        .expect("read dir")
        .find_map(Result::ok)
        .expect("entry");
    assert!(super::build_entry_from_dir_entry(&entry, "").is_none());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p static-flow-media upload -- --nocapture`  
Expected: FAIL because upload production code does not exist.

- [ ] **Step 3: Add upload dependencies, path helpers, and task locks**

Modify `media-service/Cargo.toml` and `media-service/src/state.rs`:

```rust
[dependencies]
thiserror = { workspace = true }
uuid = { version = "1.18", features = ["v4"] }

#[derive(Clone)]
pub struct LocalMediaState {
    // existing fields...
    upload_locks: Arc<DashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl LocalMediaState {
    pub fn upload_root(&self) -> PathBuf {
        self.root_dir.join(".static-flow").join("uploads")
    }

    pub fn upload_task_dir(&self, task_id: &str) -> PathBuf {
        self.upload_root().join(task_id)
    }

    pub fn upload_task_lock(&self, task_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.upload_locks
            .entry(task_id.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }
}
```

- [ ] **Step 4: Implement disk-backed upload store**

Create `media-service/src/upload_store.rs` with focused helpers:

```rust
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use static_flow_media_types::UploadTaskRecord;
use tokio::fs;

pub async fn ensure_task_dir(task_dir: &Path) -> Result<()> {
    fs::create_dir_all(task_dir)
        .await
        .with_context(|| format!("failed to create upload task dir {}", task_dir.display()))
}

pub fn task_json_path(task_dir: &Path) -> PathBuf {
    task_dir.join("task.json")
}

pub fn task_blob_path(task_dir: &Path) -> PathBuf {
    task_dir.join("blob.part")
}

pub async fn load_task(task_dir: &Path) -> Result<Option<UploadTaskRecord>> {
    let path = task_json_path(task_dir);
    if fs::metadata(&path).await.is_err() {
        return Ok(None);
    }
    let bytes = fs::read(&path)
        .await
        .with_context(|| format!("failed to read {}", path.display()))?;
    Ok(Some(serde_json::from_slice(&bytes).context("failed to decode upload task")?))
}

pub async fn save_task(task_dir: &Path, task: &UploadTaskRecord) -> Result<()> {
    ensure_task_dir(task_dir).await?;
    let path = task_json_path(task_dir);
    let bytes = serde_json::to_vec_pretty(task).context("failed to encode upload task")?;
    fs::write(&path, bytes)
        .await
        .with_context(|| format!("failed to write {}", path.display()))
}

pub async fn list_tasks(upload_root: &Path) -> Result<Vec<UploadTaskRecord>> {
    let mut tasks = Vec::new();
    let Ok(mut entries) = fs::read_dir(upload_root).await else {
        return Ok(tasks);
    };
    while let Some(entry) = entries.next_entry().await? {
        if let Some(task) = load_task(&entry.path()).await? {
            tasks.push(task);
        }
    }
    tasks.sort_by(|a, b| b.updated_at_ms.cmp(&a.updated_at_ms));
    Ok(tasks)
}
```

- [ ] **Step 5: Implement upload lifecycle**

Create `media-service/src/upload.rs` with concrete service functions:

```rust
use std::{path::Path, sync::Arc};

use axum::body::Bytes;
use static_flow_media_types::{
    CreateUploadTaskRequest, CreateUploadTaskResponse, ListUploadTasksQuery,
    ListUploadTasksResponse, UploadTaskRecord, UploadTaskStatus,
};

#[derive(Debug, thiserror::Error)]
pub enum UploadError {
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    NotFound(String),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

pub type UploadResult<T> = Result<T, UploadError>;

fn ensure_supported_upload_name(file_name: &str) -> UploadResult<()> {
    let ext = std::path::Path::new(file_name)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase());
    if matches!(
        ext.as_deref(),
        Some("mp4" | "m4v" | "mov" | "webm" | "mkv" | "avi" | "ts" | "mpeg" | "mpg")
    ) {
        Ok(())
    } else {
        Err(UploadError::BadRequest(
            "unsupported upload file extension".to_string(),
        ))
    }
}

fn build_resume_key(target_dir: &str, source_file_name: &str, file_size: u64, last_modified_ms: i64) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(target_dir.as_bytes());
    hasher.update(b"\n");
    hasher.update(source_file_name.as_bytes());
    hasher.update(b"\n");
    hasher.update(file_size.to_string().as_bytes());
    hasher.update(b"\n");
    hasher.update(last_modified_ms.to_string().as_bytes());
    format!("{:x}", hasher.finalize())
}

pub async fn create_or_resume_upload_task(
    state: Arc<LocalMediaState>,
    request: CreateUploadTaskRequest,
) -> UploadResult<CreateUploadTaskResponse> {
    let target_dir = normalize_relative_path(&request.target_dir)
        .map_err(|err| UploadError::BadRequest(err.to_string()))?;
    ensure_supported_upload_name(&request.source_file_name)?;
    crate::path_guard::resolve_media_path(state.root_dir(), &target_dir)
        .map_err(|err| UploadError::BadRequest(err.to_string()))?;
    let resume_key = build_resume_key(
        &target_dir,
        &request.source_file_name,
        request.file_size,
        request.last_modified_ms,
    );
    let upload_root = state.upload_root();
    tokio::fs::create_dir_all(&upload_root).await?;

    if let Some(existing) = find_resumable_task(&upload_root, &resume_key, &target_dir).await? {
        return Ok(CreateUploadTaskResponse {
            task: reconcile_task_with_disk(&state, existing).await?,
        });
    }

    let task_id = format!("upload-{}", uuid::Uuid::new_v4().simple());
    let target_file_name = resolve_available_file_name(
        state.root_dir(),
        &target_dir,
        &request.source_file_name,
    )
    .await?;
    let now = chrono::Utc::now().timestamp_millis();
    let task = UploadTaskRecord {
        task_id: task_id.clone(),
        resume_key,
        status: UploadTaskStatus::Created,
        target_dir: target_dir.clone(),
        source_file_name: request.source_file_name.clone(),
        target_file_name: target_file_name.clone(),
        target_relative_path: join_relative_path(&target_dir, &target_file_name),
        file_size: request.file_size,
        uploaded_bytes: 0,
        last_modified_ms: request.last_modified_ms,
        mime_type: request.mime_type.clone(),
        error: None,
        created_at_ms: now,
        updated_at_ms: now,
    };
    upload_store::save_task(&state.upload_task_dir(&task_id), &task).await?;
    Ok(CreateUploadTaskResponse { task })
}

pub async fn list_upload_tasks(
    state: Arc<LocalMediaState>,
    query: ListUploadTasksQuery,
) -> UploadResult<ListUploadTasksResponse> {
    let dir = query
        .dir
        .as_deref()
        .map(normalize_relative_path)
        .transpose()
        .map_err(|err| UploadError::BadRequest(err.to_string()))?;
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let offset = query.offset.unwrap_or(0);
    let mut tasks = upload_store::list_tasks(&state.upload_root()).await?;
    if let Some(dir) = dir.as_deref() {
        tasks.retain(|task| task.target_dir == dir);
    }
    for task in &mut tasks {
        *task = reconcile_task_with_disk(&state, task.clone()).await?;
    }
    let total = tasks.len();
    let page = tasks
        .into_iter()
        .skip(offset)
        .take(limit)
        .collect::<Vec<_>>();
    Ok(ListUploadTasksResponse {
        has_more: offset.saturating_add(page.len()) < total,
        tasks: page,
        total,
        limit,
        offset,
    })
}

pub async fn get_upload_task(state: Arc<LocalMediaState>, task_id: &str) -> UploadResult<UploadTaskRecord> {
    let task_dir = state.upload_task_dir(task_id);
    let task = load_existing_task(&task_dir)
        .await
        .map_err(|err| match err {
            UploadError::NotFound(_) => UploadError::NotFound("upload task not found".to_string()),
            other => other,
        })?;
    reconcile_task_with_disk(&state, task).await
}

async fn find_resumable_task(
    upload_root: &std::path::Path,
    resume_key: &str,
    target_dir: &str,
) -> UploadResult<Option<UploadTaskRecord>> {
    let tasks = upload_store::list_tasks(upload_root).await?;
    Ok(tasks.into_iter().find(|task| {
        task.resume_key == resume_key
            && task.target_dir == target_dir
            && matches!(task.status, UploadTaskStatus::Created | UploadTaskStatus::Partial)
    }))
}

async fn reconcile_task_with_disk(
    state: &Arc<LocalMediaState>,
    mut task: UploadTaskRecord,
) -> UploadResult<UploadTaskRecord> {
    let task_dir = state.upload_task_dir(&task.task_id);
    let blob_path = upload_store::task_blob_path(&task_dir);
    if let Ok(meta) = tokio::fs::metadata(&blob_path).await {
        let actual = meta.len();
        if actual != task.uploaded_bytes {
            task.uploaded_bytes = actual;
            task.status = if actual == 0 {
                UploadTaskStatus::Created
            } else if actual >= task.file_size {
                UploadTaskStatus::Completed
            } else {
                UploadTaskStatus::Partial
            };
            task.updated_at_ms = chrono::Utc::now().timestamp_millis();
            upload_store::save_task(&task_dir, &task).await?;
        }
    } else {
        let target_dir = crate::path_guard::resolve_media_path(state.root_dir(), &task.target_dir)?;
        let final_path = target_dir.join(&task.target_file_name);
        if tokio::fs::metadata(&final_path).await.is_ok() && task.uploaded_bytes >= task.file_size {
            task.status = UploadTaskStatus::Completed;
            task.updated_at_ms = chrono::Utc::now().timestamp_millis();
            upload_store::save_task(&task_dir, &task).await?;
        }
    }
    Ok(task)
}

async fn resolve_available_file_name(
    root_dir: &std::path::Path,
    target_dir: &str,
    source_file_name: &str,
) -> anyhow::Result<String> {
    let mut candidate = source_file_name.to_string();
    let stem = std::path::Path::new(source_file_name)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(source_file_name);
    let ext = std::path::Path::new(source_file_name)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| format!(".{value}"))
        .unwrap_or_default();
    let target_base = crate::path_guard::resolve_media_path(root_dir, target_dir)?;
    let mut suffix = 1usize;
    while tokio::fs::metadata(target_base.join(&candidate)).await.is_ok() {
        candidate = format!("{stem} ({suffix}){ext}");
        suffix += 1;
    }
    Ok(candidate)
}

fn join_relative_path(target_dir: &str, file_name: &str) -> String {
    if target_dir.is_empty() {
        file_name.to_string()
    } else {
        format!("{target_dir}/{file_name}")
    }
}

pub async fn append_upload_chunk(
    state: Arc<LocalMediaState>,
    task_id: &str,
    offset: u64,
    chunk: Bytes,
) -> UploadResult<UploadTaskRecord> {
    let _guard = state.upload_task_lock(task_id).lock().await;
    let task_dir = state.upload_task_dir(task_id);
    let mut task = load_existing_task(&task_dir).await?;
    if matches!(
        task.status,
        UploadTaskStatus::Completed | UploadTaskStatus::Canceled | UploadTaskStatus::Failed
    ) {
        return Err(UploadError::Conflict(
            "cannot append to terminal upload task".to_string(),
        ));
    }
    if offset != task.uploaded_bytes {
        return Err(UploadError::Conflict(format!(
            "offset mismatch: expected {}, got {}",
            task.uploaded_bytes, offset
        )));
    }
    if task.uploaded_bytes + chunk.len() as u64 > task.file_size {
        return Err(UploadError::BadRequest(
            "chunk exceeds declared file size".to_string(),
        ));
    }
    let blob_path = upload_store::task_blob_path(&task_dir);
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&blob_path)
        .await?;
    tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
    tokio::io::AsyncWriteExt::flush(&mut file).await?;
    file.sync_all().await?;
    task.uploaded_bytes += chunk.len() as u64;
    task.status = if task.uploaded_bytes >= task.file_size {
        UploadTaskStatus::Completed
    } else {
        UploadTaskStatus::Partial
    };
    task.updated_at_ms = chrono::Utc::now().timestamp_millis();
    if task.uploaded_bytes >= task.file_size {
        finalize_upload(&state, &mut task, &blob_path).await?;
    }
    upload_store::save_task(&task_dir, &task).await?;
    Ok(task)
}

pub async fn delete_upload_task(state: Arc<LocalMediaState>, task_id: &str) -> UploadResult<UploadTaskRecord> {
    let _guard = state.upload_task_lock(task_id).lock().await;
    let task_dir = state.upload_task_dir(task_id);
    let mut task = load_existing_task(&task_dir).await?;
    if matches!(task.status, UploadTaskStatus::Completed) {
        return Err(UploadError::Conflict(
            "completed uploads cannot be canceled".to_string(),
        ));
    }
    let blob_path = upload_store::task_blob_path(&task_dir);
    let _ = tokio::fs::remove_file(&blob_path).await;
    task.status = UploadTaskStatus::Canceled;
    task.error = None;
    task.updated_at_ms = chrono::Utc::now().timestamp_millis();
    upload_store::save_task(&task_dir, &task).await?;
    Ok(task)
}

async fn load_existing_task(task_dir: &std::path::Path) -> UploadResult<UploadTaskRecord> {
    upload_store::load_task(task_dir)
        .await?
        .ok_or_else(|| UploadError::NotFound("upload task not found".to_string()))
}

async fn finalize_upload(
    state: &Arc<LocalMediaState>,
    task: &mut UploadTaskRecord,
    blob_path: &std::path::Path,
) -> UploadResult<()> {
    let target_dir = crate::path_guard::resolve_media_path(state.root_dir(), &task.target_dir)?;
    let mut final_path = target_dir.join(&task.target_file_name);
    if tokio::fs::metadata(&final_path).await.is_ok() {
        let renamed = resolve_available_file_name(
            state.root_dir(),
            &task.target_dir,
            &task.source_file_name,
        )
        .await?;
        task.target_file_name = renamed.clone();
        task.target_relative_path = join_relative_path(&task.target_dir, &renamed);
        final_path = target_dir.join(renamed);
    }
    tokio::fs::rename(blob_path, &final_path).await?;
    Ok(())
}
```

- [ ] **Step 6: Run focused tests to verify they pass**

Run: `cargo test -p static-flow-media upload -- --nocapture`  
Expected: PASS for upload core tests.

- [ ] **Step 7: Commit**

```bash
git add media-service/Cargo.toml media-service/src/lib.rs media-service/src/state.rs media-service/src/upload_store.rs media-service/src/upload.rs media-service/src/fs.rs
git commit -m "feat: add media service upload persistence"
```

## Task 3: Expose Upload APIs From Media-Service HTTP Layer

**Files:**
- Modify: `media-service/src/handlers.rs`
- Modify: `media-service/src/routes.rs`
- Test: `media-service/src/routes.rs`

- [ ] **Step 1: Write the failing route test**

Add a route registration test in `media-service/src/routes.rs`:

```rust
#[tokio::test]
async fn media_router_registers_upload_routes() {
    let root = tempfile::tempdir().expect("root");
    let cache = tempfile::tempdir().expect("cache");
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p static-flow-media media_router_registers_upload_routes -- --nocapture`  
Expected: FAIL because upload routes are missing.

- [ ] **Step 3: Add upload handlers**

Extend `media-service/src/handlers.rs`:

```rust
use crate::upload::UploadError;

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
    Ok(Json(UploadChunkResponse { task }))
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

fn upload_error(err: UploadError) -> (StatusCode, Json<ErrorResponse>) {
    match err {
        UploadError::BadRequest(message) => error_response(StatusCode::BAD_REQUEST, message),
        UploadError::Conflict(message) => error_response(StatusCode::CONFLICT, message),
        UploadError::NotFound(message) => error_response(StatusCode::NOT_FOUND, message),
        UploadError::Internal(err) => internal_error(err),
    }
}
```

- [ ] **Step 4: Register upload routes**

Modify `media-service/src/routes.rs`:

```rust
use axum::routing::{get, post, put};

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
    put(handlers::append_upload_chunk),
)
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p static-flow-media media_router_registers_upload_routes -- --nocapture`  
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add media-service/src/handlers.rs media-service/src/routes.rs
git commit -m "feat: expose local media upload APIs"
```

## Task 4: Add Backend Upload Proxy Endpoints

**Files:**
- Modify: `backend/src/media_proxy/forward.rs`
- Modify: `backend/src/media_proxy/handlers.rs`
- Modify: `backend/src/routes.rs`
- Test: `backend/src/media_proxy/forward.rs`

- [ ] **Step 1: Write the failing forwarding test**

Add a raw-body proxy test in `backend/src/media_proxy/forward.rs`:

```rust
#[tokio::test]
async fn forward_upload_chunk_preserves_body_and_content_type() {
    let upstream = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("PUT"))
        .and(wiremock::matchers::path(
            "/internal/local-media/uploads/tasks/task-1/chunks",
        ))
        .and(wiremock::matchers::query_param("offset", "4"))
        .and(wiremock::matchers::header("content-type", "application/octet-stream"))
        .and(wiremock::matchers::body_bytes(b"chunk".to_vec()))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_raw(
            r#"{"task":{"task_id":"task-1","resume_key":"k","status":"partial","target_dir":"","source_file_name":"clip.mp4","target_file_name":"clip.mp4","target_relative_path":"clip.mp4","file_size":10,"uploaded_bytes":9,"last_modified_ms":1,"mime_type":null,"error":null,"created_at_ms":1,"updated_at_ms":1}}"#,
            "application/json",
        ))
        .mount(&upstream)
        .await;

    let response = forward_upload_chunk_request(
        &reqwest::Client::new(),
        &reqwest::Url::parse(&upstream.uri()).expect("base"),
        "task-1",
        4,
        reqwest::header::HeaderValue::from_static("application/octet-stream"),
        bytes::Bytes::from_static(b"chunk"),
    )
    .await
    .expect("forward response");
    assert_eq!(response.status(), axum::http::StatusCode::OK);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p static-flow-backend forward_upload_chunk_preserves_body_and_content_type -- --nocapture`  
Expected: FAIL because upload forward helper is missing.

- [ ] **Step 3: Add backend forwarding helpers**

Extend `backend/src/media_proxy/forward.rs`:

```rust
pub async fn forward_upload_chunk_request(
    client: &reqwest::Client,
    base_url: &reqwest::Url,
    task_id: &str,
    offset: u64,
    content_type: reqwest::header::HeaderValue,
    body: bytes::Bytes,
) -> Result<Response> {
    let relative = format!("internal/local-media/uploads/tasks/{task_id}/chunks");
    let upstream = client
        .put(join_internal_url(base_url, &relative)?)
        .query(&static_flow_media_types::UploadChunkQuery { offset })
        .header(reqwest::header::CONTENT_TYPE, content_type)
        .body(body);
    forward(upstream).await
}
```

- [ ] **Step 4: Add upload proxy handlers and routes**

Extend `backend/src/media_proxy/handlers.rs` and `backend/src/routes.rs`:

```rust
pub async fn list_upload_tasks(
    State(state): State<AppState>,
    Query(query): Query<static_flow_media_types::ListUploadTasksQuery>,
    headers: HeaderMap,
) -> HandlerResult<Json<static_flow_media_types::ListUploadTasksResponse>> {
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
    Json(request): Json<static_flow_media_types::CreateUploadTaskRequest>,
) -> HandlerResult<Json<static_flow_media_types::CreateUploadTaskResponse>> {
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
) -> HandlerResult<Json<static_flow_media_types::UploadTaskRecord>> {
    ensure_admin_access(&state, &headers)?;
    let media_proxy = configured_media_proxy(&state)?;
    let relative = format!("internal/local-media/uploads/tasks/{task_id}");
    let response = send_json(media_proxy.client().get(join_internal_url(media_proxy.as_ref(), &relative)?))
        .await?;
    Ok(Json(response))
}

pub async fn append_upload_chunk(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    Query(query): Query<static_flow_media_types::UploadChunkQuery>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> HandlerResult<Response> {
    ensure_admin_access(&state, &headers)?;
    let media_proxy = configured_media_proxy(&state)?;
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .cloned()
        .unwrap_or_else(|| axum::http::HeaderValue::from_static("application/octet-stream"));
    forward_upload_chunk_request(
        media_proxy.client(),
        &media_proxy.config().base_url,
        &task_id,
        query.offset,
        reqwest::header::HeaderValue::from_bytes(content_type.as_bytes())
            .expect("valid content type"),
        body,
    )
    .await
    .map_err(bad_gateway)
}

pub async fn delete_upload_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    headers: HeaderMap,
) -> HandlerResult<Json<static_flow_media_types::UploadTaskRecord>> {
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

use axum::routing::{delete, get, patch, post, put};

.route(
    "/admin/local-media/api/uploads/tasks",
    post(crate::media_proxy::handlers::create_upload_task)
        .get(crate::media_proxy::handlers::list_upload_tasks),
)
.route(
    "/admin/local-media/api/uploads/tasks/:task_id",
    get(crate::media_proxy::handlers::get_upload_task)
        .delete(crate::media_proxy::handlers::delete_upload_task),
)
.route(
    "/admin/local-media/api/uploads/tasks/:task_id/chunks",
    put(crate::media_proxy::handlers::append_upload_chunk),
)
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p static-flow-backend forward_upload_chunk_preserves_body_and_content_type -- --nocapture`  
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add backend/src/media_proxy/forward.rs backend/src/media_proxy/handlers.rs backend/src/routes.rs
git commit -m "feat: proxy local media upload endpoints"
```

## Task 5: Add Frontend Upload API Helpers And Upload Panel

**Files:**
- Modify: `frontend/Cargo.toml`
- Modify: `frontend/src/api.rs`
- Create: `frontend/src/components/admin_local_media_uploads.rs`
- Modify: `frontend/src/components/mod.rs`
- Modify: `frontend/src/pages/admin_local_media.rs`
- Test: `frontend/src/api.rs`
- Test: `frontend/src/components/admin_local_media_uploads.rs`

- [ ] **Step 1: Write the failing frontend API test**

Add a pure URL helper test in `frontend/src/api.rs`:

```rust
#[test]
fn build_admin_local_media_upload_tasks_url_uses_admin_prefix() {
    assert_eq!(
        build_admin_local_media_upload_tasks_url(),
        "/admin/local-media/api/uploads/tasks"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p static-flow-frontend build_admin_local_media_upload_tasks_url_uses_admin_prefix -- --nocapture`  
Expected: FAIL because upload API helpers do not exist.

- [ ] **Step 3: Add upload API helpers**

Extend `frontend/src/api.rs`:

```rust
pub fn build_admin_local_media_upload_tasks_url() -> String {
    format!("{}/uploads/tasks", local_media_api_base())
}

pub async fn create_admin_local_media_upload_task(
    request: &static_flow_media_types::CreateUploadTaskRequest,
) -> Result<static_flow_media_types::UploadTaskRecord, String> {
    let response = api_post(&build_admin_local_media_upload_tasks_url())
        .json(request)
        .map_err(|err| err.to_string())?
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if !response.ok() {
        return Err(format!("Failed: HTTP {}", response.status()));
    }
    let payload: static_flow_media_types::CreateUploadTaskResponse =
        response.json().await.map_err(|err| err.to_string())?;
    Ok(payload.task)
}

pub async fn fetch_admin_local_media_upload_tasks(
    dir: Option<&str>,
) -> Result<static_flow_media_types::ListUploadTasksResponse, String> {
    let mut url = build_admin_local_media_upload_tasks_url();
    if let Some(dir) = dir.filter(|value| !value.trim().is_empty()) {
        url.push_str(&format!("?dir={}", urlencoding::encode(dir)));
    }
    let response = api_get(&url).send().await.map_err(|err| err.to_string())?;
    if !response.ok() {
        return Err(format!("Failed: HTTP {}", response.status()));
    }
    response.json().await.map_err(|err| err.to_string())
}

pub async fn fetch_admin_local_media_upload_task(
    task_id: &str,
) -> Result<static_flow_media_types::UploadTaskRecord, String> {
    let url = format!("{}/{}", build_admin_local_media_upload_tasks_url(), task_id);
    let response = api_get(&url).send().await.map_err(|err| err.to_string())?;
    if !response.ok() {
        return Err(format!("Failed: HTTP {}", response.status()));
    }
    response.json().await.map_err(|err| err.to_string())
}

pub async fn append_admin_local_media_upload_chunk(
    task_id: &str,
    offset: u64,
    bytes: Vec<u8>,
) -> Result<static_flow_media_types::UploadTaskRecord, String> {
    let url = format!(
        "{}/uploads/tasks/{task_id}/chunks?offset={offset}",
        local_media_api_base()
    );
    let response = gloo_net::http::Request::put(&url)
        .header("Content-Type", "application/octet-stream")
        .body(bytes)
        .map_err(|err| err.to_string())?
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if !response.ok() {
        return Err(format!("Failed: HTTP {}", response.status()));
    }
    let payload: static_flow_media_types::UploadChunkResponse =
        response.json().await.map_err(|err| err.to_string())?;
    Ok(payload.task)
}

pub async fn delete_admin_local_media_upload_task(
    task_id: &str,
) -> Result<static_flow_media_types::UploadTaskRecord, String> {
    let url = format!("{}/{}", build_admin_local_media_upload_tasks_url(), task_id);
    let response = api_delete(&url).send().await.map_err(|err| err.to_string())?;
    if !response.ok() {
        return Err(format!("Failed: HTTP {}", response.status()));
    }
    response.json().await.map_err(|err| err.to_string())
}
```

Also extend `frontend/Cargo.toml` `web-sys` features with:

```toml
"File", "Blob", "FileList", "HtmlInputElement"
```

- [ ] **Step 4: Create the upload component**

Create `frontend/src/components/admin_local_media_uploads.rs`:

```rust
use std::collections::HashMap;

const CHUNK_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Properties, PartialEq, Clone)]
pub struct AdminLocalMediaUploadsProps {
    pub current_dir: String,
    pub on_refresh_dir: Callback<()>,
}

#[function_component(AdminLocalMediaUploads)]
pub fn admin_local_media_uploads(props: &AdminLocalMediaUploadsProps) -> Html {
    let tasks = use_state(Vec::<static_flow_media_types::UploadTaskRecord>::new);
    let attached_files = use_state(HashMap::<String, web_sys::File>::new);
    let active_task_id = use_state(|| None::<String>);
    let file_input = use_node_ref();
    let error = use_state(|| None::<String>);

    {
        let tasks = tasks.clone();
        let error = error.clone();
        use_effect_with(props.current_dir.clone(), move |dir| {
            let dir = dir.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match crate::api::fetch_admin_local_media_upload_tasks(Some(dir.as_str())).await {
                    Ok(response) => tasks.set(response.tasks),
                    Err(err) => error.set(Some(err)),
                }
            });
            || ()
        });
    }

    let on_change = {
        let current_dir = props.current_dir.clone();
        let tasks = tasks.clone();
        let attached_files = attached_files.clone();
        let active_task_id = active_task_id.clone();
        let error = error.clone();
        let on_refresh_dir = props.on_refresh_dir.clone();
        Callback::from(move |event: Event| {
            let Some(input) = event.target_dyn_into::<web_sys::HtmlInputElement>() else {
                return;
            };
            let Some(files) = input.files() else {
                return;
            };
            let selected = (0..files.length())
                .filter_map(|index| files.get(index))
                .collect::<Vec<_>>();
            let current_dir = current_dir.clone();
            let tasks = tasks.clone();
            let attached_files = attached_files.clone();
            let active_task_id = active_task_id.clone();
            let error = error.clone();
            let on_refresh_dir = on_refresh_dir.clone();
            wasm_bindgen_futures::spawn_local(async move {
                for file in selected {
                    let created = crate::api::create_admin_local_media_upload_task(
                        &static_flow_media_types::CreateUploadTaskRequest {
                            target_dir: current_dir.clone(),
                            source_file_name: file.name(),
                            file_size: file.size() as u64,
                            last_modified_ms: file.last_modified() as i64,
                            mime_type: Some(file.type_()),
                        },
                    )
                    .await;
                    match created {
                        Ok(task) => {
                            upsert_task(&tasks, task.clone());
                            let mut next_files = (*attached_files).clone();
                            next_files.insert(task.task_id.clone(), file.clone());
                            attached_files.set(next_files);
                            active_task_id.set(Some(task.task_id.clone()));
                            match run_single_upload(task, file).await {
                                Ok(updated) => {
                                    upsert_task(&tasks, updated.clone());
                                    if matches!(
                                        updated.status,
                                        static_flow_media_types::UploadTaskStatus::Completed
                                    ) {
                                        on_refresh_dir.emit(());
                                    }
                                },
                                Err(err) => error.set(Some(err)),
                            }
                            active_task_id.set(None);
                        },
                        Err(err) => error.set(Some(err)),
                    }
                }
            });
        })
    };

    let on_cancel = {
        let tasks = tasks.clone();
        let error = error.clone();
        Callback::from(move |task_id: String| {
            let tasks = tasks.clone();
            let error = error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match crate::api::delete_admin_local_media_upload_task(&task_id).await {
                    Ok(task) => upsert_task(&tasks, task),
                    Err(err) => error.set(Some(err)),
                }
            });
        })
    };

    html! {
        <section class="mb-5 rounded-[var(--radius)] border border-[var(--border)] bg-[var(--surface)] p-5 shadow-[var(--shadow)]">
            <h2 class="m-0 text-lg font-semibold text-[var(--text)]">{ "Uploads" }</h2>
            <p class="mt-2 text-sm text-[var(--muted)]">{ format!("Target directory: /{}", props.current_dir) }</p>
            if let Some(err) = (*error).clone() {
                <div class="mt-3 rounded-[var(--radius)] border border-red-400/40 bg-red-500/10 p-3 text-sm text-red-700">
                    { err }
                </div>
            }
            <input type="file" accept="video/*,.mkv,.mp4,.mov,.webm,.m4v" multiple=true ref={file_input} onchange={on_change} />
            <div class="mt-4 space-y-3">
                { for tasks.iter().map(|task| {
                    render_upload_task_card(task, (*active_task_id).as_deref(), on_cancel.clone())
                }) }
            </div>
        </section>
    }
}

async fn run_single_upload(
    task: static_flow_media_types::UploadTaskRecord,
    file: web_sys::File,
) -> Result<static_flow_media_types::UploadTaskRecord, String> {
    let mut offset = task.uploaded_bytes;
    while offset < task.file_size {
        let end = (offset + CHUNK_BYTES).min(task.file_size);
        let blob = file
            .slice_with_f64_and_f64(offset as f64, end as f64)
            .map_err(|err| format!("{err:?}"))?;
        let js_value = wasm_bindgen_futures::JsFuture::from(blob.array_buffer())
            .await
            .map_err(|err| format!("{err:?}"))?;
        let bytes = js_sys::Uint8Array::new(&js_value).to_vec();
        let updated = crate::api::append_admin_local_media_upload_chunk(&task.task_id, offset, bytes).await?;
        offset = updated.uploaded_bytes;
        if matches!(updated.status, static_flow_media_types::UploadTaskStatus::Completed) {
            return Ok(updated);
        }
    }
    crate::api::fetch_admin_local_media_upload_task(&task.task_id).await
}

fn upsert_task(
    tasks: &UseStateHandle<Vec<static_flow_media_types::UploadTaskRecord>>,
    task: static_flow_media_types::UploadTaskRecord,
) {
    let mut next = (**tasks).clone();
    next.retain(|row| row.task_id != task.task_id);
    next.push(task);
    next.sort_by(|left, right| right.updated_at_ms.cmp(&left.updated_at_ms));
    tasks.set(next);
}

fn render_upload_task_card(
    task: &static_flow_media_types::UploadTaskRecord,
    active_task_id: Option<&str>,
    on_cancel: Callback<String>,
) -> Html {
    let progress = if task.file_size == 0 {
        0.0
    } else {
        (task.uploaded_bytes as f64 / task.file_size as f64) * 100.0
    };
    let is_active = active_task_id == Some(task.task_id.as_str());
    let cancel = {
        let task_id = task.task_id.clone();
        let on_cancel = on_cancel.clone();
        Callback::from(move |_| on_cancel.emit(task_id.clone()))
    };
    html! {
        <div class="rounded-[var(--radius)] border border-[var(--border)] bg-[var(--surface-alt)] p-3">
            <div class="text-sm font-semibold text-[var(--text)] break-all">{ task.target_relative_path.clone() }</div>
            <div class="mt-2 h-2 overflow-hidden rounded-full bg-[var(--surface)]">
                <div class="h-full bg-sky-500" style={format!("width: {:.2}%;", progress)}></div>
            </div>
            <div class="mt-2 flex items-center justify-between gap-3 text-xs text-[var(--muted)]">
                { format!("{} / {} bytes ({:.1}%)", task.uploaded_bytes, task.file_size, progress) }
                if is_active {
                    <span>{ "Sending" }</span>
                } else {
                    <span>{ format!("{:?}", task.status) }</span>
                }
            </div>
            if !matches!(task.status, static_flow_media_types::UploadTaskStatus::Completed | static_flow_media_types::UploadTaskStatus::Canceled) {
                <button type="button" class="btn-fluent-secondary mt-3" onclick={cancel}>
                    { "Cancel" }
                </button>
            }
        </div>
    }
}
```

- [ ] **Step 5: Compose the upload panel into the page**

Modify `frontend/src/components/mod.rs` and `frontend/src/pages/admin_local_media.rs`:

```rust
pub mod admin_local_media_uploads;
```

```rust
<crate::components::admin_local_media_uploads::AdminLocalMediaUploads
    current_dir={current_dir.clone()}
    on_refresh_dir={{
        let open_dir = open_dir.clone();
        let current_dir = current_dir.clone();
        Callback::from(move |_| open_dir.emit(current_dir.clone()))
    }}
/>
```

- [ ] **Step 6: Run focused tests to verify they pass**

Run:

```bash
cargo test -p static-flow-frontend build_admin_local_media_upload_tasks_url_uses_admin_prefix -- --nocapture
cargo test -p static-flow-frontend admin_local_media -- --nocapture
```

Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add frontend/Cargo.toml frontend/src/api.rs frontend/src/components/admin_local_media_uploads.rs frontend/src/components/mod.rs frontend/src/pages/admin_local_media.rs
git commit -m "feat: add admin local media upload UI"
```

## Task 6: Full Verification And Cleanup

**Files:**
- Modify: any files from previous tasks that fail verification

- [ ] **Step 1: Run targeted formatter**

Run:

```bash
rustfmt media-types/src/lib.rs media-service/src/state.rs media-service/src/upload_store.rs media-service/src/upload.rs media-service/src/handlers.rs media-service/src/routes.rs backend/src/media_proxy/forward.rs backend/src/media_proxy/handlers.rs backend/src/routes.rs frontend/src/api.rs frontend/src/components/admin_local_media_uploads.rs frontend/src/components/mod.rs frontend/src/pages/admin_local_media.rs
```

Expected: files are reformatted in place with no errors.

- [ ] **Step 2: Run upload-focused tests**

Run:

```bash
cargo test -p static-flow-media-types upload_type_tests -- --nocapture
cargo test -p static-flow-media upload -- --nocapture
cargo test -p static-flow-media media_router_registers_upload_routes -- --nocapture
cargo test -p static-flow-backend forward_upload_chunk_preserves_body_and_content_type -- --nocapture
cargo test -p static-flow-frontend build_admin_local_media_upload_tasks_url_uses_admin_prefix -- --nocapture
cargo test -p static-flow-frontend admin_local_media -- --nocapture
```

Expected: all commands pass.

- [ ] **Step 3: Run clippy on affected crates**

Run:

```bash
cargo clippy -p static-flow-media-types -p static-flow-media -p static-flow-backend -p static-flow-frontend --tests -- -D warnings
```

Expected: exits `0` with no warnings.

- [ ] **Step 4: Smoke-check the full user flow**

Run:

```bash
cargo check -p static-flow-media -p static-flow-backend -p static-flow-frontend
```

Then manually verify:

- `/admin/local-media?dir=<dir>` shows the upload panel
- selecting a video creates a task and starts uploading
- refreshing the page shows the same task with persisted progress
- re-selecting the same local file resumes from server offset
- on completion the file appears in the current directory listing
- the uploaded file opens in the existing player flow

- [ ] **Step 5: Commit**

```bash
git add media-types/src/lib.rs media-service/Cargo.toml media-service/src/state.rs media-service/src/upload_store.rs media-service/src/upload.rs media-service/src/handlers.rs media-service/src/routes.rs backend/src/media_proxy/forward.rs backend/src/media_proxy/handlers.rs backend/src/routes.rs frontend/Cargo.toml frontend/src/api.rs frontend/src/components/admin_local_media_uploads.rs frontend/src/components/mod.rs frontend/src/pages/admin_local_media.rs
git commit -m "feat: add resumable admin local media uploads"
```

## Self-Review

- Spec coverage:
  - current-directory upload target: Task 2 + Task 5
  - durable resume state after reload/restart: Task 2 + Task 5
  - auto-rename: Task 2
  - backend auth/proxy boundary: Task 4
  - immediate directory visibility and playback reuse: Task 5 + Task 6
- Placeholder scan:
  - no `TODO`, `TBD`, or “similar to previous task” markers remain
- Type consistency:
  - `UploadTaskStatus`, `CreateUploadTaskRequest`, `UploadTaskRecord`,
    `CreateUploadTaskResponse`, `ListUploadTasksResponse`, and
    `UploadChunkResponse` are introduced in Task 1 and reused consistently later
