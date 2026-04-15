use std::{collections::HashSet, path::Path, sync::Arc};

use axum::body::Bytes;
use sha2::{Digest, Sha256};
use static_flow_media_types::{
    CreateUploadTaskRequest, CreateUploadTaskResponse, ListUploadTasksQuery,
    ListUploadTasksResponse, UploadTaskRecord, UploadTaskStatus,
};

use crate::{
    fs::normalize_relative_path, path_guard::resolve_media_path, state::LocalMediaState,
    upload_store,
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

impl From<std::io::Error> for UploadError {
    fn from(err: std::io::Error) -> Self {
        Self::Internal(err.into())
    }
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
        Err(UploadError::BadRequest("unsupported upload file extension".to_string()))
    }
}

fn build_resume_key(
    target_dir: &str,
    source_file_name: &str,
    file_size: u64,
    last_modified_ms: i64,
) -> String {
    // Browser task ids disappear on refresh. Derive resume identity from the
    // actual user-selected file so choosing the same file again reattaches to
    // the staged upload instead of creating a duplicate task.
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
    resolve_media_path(state.root_dir(), &target_dir)
        .map_err(|err| UploadError::BadRequest(err.to_string()))?;

    let resume_key = build_resume_key(
        &target_dir,
        &request.source_file_name,
        request.file_size,
        request.last_modified_ms,
    );
    let upload_root = state.upload_root();
    tokio::fs::create_dir_all(&upload_root).await?;
    let tasks = upload_store::list_tasks(&upload_root).await?;

    if let Some(existing) = tasks.iter().find(|task| {
        task.resume_key == resume_key
            && task.target_dir == target_dir
            && matches!(task.status, UploadTaskStatus::Created | UploadTaskStatus::Partial)
    }) {
        // Persisted metadata can drift after crashes or restarts. Reconcile the
        // staged blob length before returning the task so the browser resumes
        // from the authoritative on-disk offset.
        return Ok(CreateUploadTaskResponse {
            task: reconcile_task_with_disk(&state, existing.clone()).await?,
        });
    }

    let task_id = format!("upload-{}", uuid::Uuid::new_v4().simple());
    let target_file_name = resolve_available_file_name(
        state.root_dir(),
        &target_dir,
        &request.source_file_name,
        &tasks,
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
    Ok(CreateUploadTaskResponse {
        task,
    })
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

    let mut reconciled = Vec::with_capacity(tasks.len());
    for task in tasks {
        reconciled.push(reconcile_task_with_disk(&state, task).await?);
    }
    reconciled.sort_by(|left, right| right.updated_at_ms.cmp(&left.updated_at_ms));

    let total = reconciled.len();
    let page = reconciled
        .into_iter()
        .skip(offset)
        .take(limit)
        .collect::<Vec<_>>();

    Ok(ListUploadTasksResponse {
        tasks: page,
        total,
        limit,
        offset,
        has_more: offset.saturating_add(limit) < total,
    })
}

pub async fn get_upload_task(
    state: Arc<LocalMediaState>,
    task_id: &str,
) -> UploadResult<UploadTaskRecord> {
    let task_dir = state.upload_task_dir(task_id);
    let task = load_existing_task(&task_dir).await?;
    reconcile_task_with_disk(&state, task).await
}

pub async fn append_upload_chunk(
    state: Arc<LocalMediaState>,
    task_id: &str,
    offset: u64,
    chunk: Bytes,
) -> UploadResult<UploadTaskRecord> {
    let task_lock = state.upload_task_lock(task_id);
    let _guard = task_lock.lock().await;
    let task_dir = state.upload_task_dir(task_id);
    let mut task = load_existing_task(&task_dir).await?;
    task = reconcile_task_with_disk(&state, task).await?;

    if matches!(
        task.status,
        UploadTaskStatus::Completed | UploadTaskStatus::Canceled | UploadTaskStatus::Failed
    ) {
        return Err(UploadError::Conflict("cannot append to terminal upload task".to_string()));
    }
    if offset != task.uploaded_bytes {
        return Err(UploadError::Conflict(format!(
            "offset mismatch: expected {}, got {}",
            task.uploaded_bytes, offset
        )));
    }
    if task.uploaded_bytes + chunk.len() as u64 > task.file_size {
        return Err(UploadError::BadRequest("chunk exceeds declared file size".to_string()));
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
    task.status = if task.uploaded_bytes == 0 {
        UploadTaskStatus::Created
    } else {
        UploadTaskStatus::Partial
    };
    task.error = None;
    task.updated_at_ms = chrono::Utc::now().timestamp_millis();

    if task.uploaded_bytes == task.file_size {
        finalize_upload(&state, &mut task, &blob_path).await?;
    }

    upload_store::save_task(&task_dir, &task).await?;
    Ok(task)
}

pub async fn delete_upload_task(
    state: Arc<LocalMediaState>,
    task_id: &str,
) -> UploadResult<UploadTaskRecord> {
    let task_lock = state.upload_task_lock(task_id);
    let _guard = task_lock.lock().await;
    let task_dir = state.upload_task_dir(task_id);
    let mut task = load_existing_task(&task_dir).await?;

    if matches!(task.status, UploadTaskStatus::Completed) {
        return Err(UploadError::Conflict("completed uploads cannot be canceled".to_string()));
    }

    let blob_path = upload_store::task_blob_path(&task_dir);
    match tokio::fs::remove_file(&blob_path).await {
        Ok(()) => {},
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {},
        Err(err) => return Err(err.into()),
    }

    task.status = UploadTaskStatus::Canceled;
    task.error = None;
    task.updated_at_ms = chrono::Utc::now().timestamp_millis();
    upload_store::save_task(&task_dir, &task).await?;
    Ok(task)
}

async fn load_existing_task(task_dir: &Path) -> UploadResult<UploadTaskRecord> {
    upload_store::load_task(task_dir)
        .await?
        .ok_or_else(|| UploadError::NotFound("upload task not found".to_string()))
}

async fn reconcile_task_with_disk(
    state: &Arc<LocalMediaState>,
    mut task: UploadTaskRecord,
) -> UploadResult<UploadTaskRecord> {
    let task_dir = state.upload_task_dir(&task.task_id);
    let blob_path = upload_store::task_blob_path(&task_dir);
    match tokio::fs::metadata(&blob_path).await {
        Ok(meta) => {
            let actual = meta.len();
            if actual > task.file_size {
                task.status = UploadTaskStatus::Failed;
                task.error = Some("staged upload exceeds declared file size".to_string());
                task.updated_at_ms = chrono::Utc::now().timestamp_millis();
                upload_store::save_task(&task_dir, &task).await?;
                return Ok(task);
            }
            if actual != task.uploaded_bytes {
                // The staged blob is the source of truth after a restart. Trust
                // its actual length instead of the last saved JSON snapshot so
                // append offsets remain monotonic.
                task.uploaded_bytes = actual;
                task.status =
                    if actual == 0 { UploadTaskStatus::Created } else { UploadTaskStatus::Partial };
                task.updated_at_ms = chrono::Utc::now().timestamp_millis();
            }
            if actual == task.file_size {
                // Completion is inferred purely from bytes on disk. That makes
                // recovery idempotent: a finished `.part` file can be finalized
                // again without a separate commit marker.
                finalize_upload(state, &mut task, &blob_path).await?;
                task.updated_at_ms = chrono::Utc::now().timestamp_millis();
            }
            upload_store::save_task(&task_dir, &task).await?;
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let target_dir = resolve_media_path(state.root_dir(), &task.target_dir)
                .map_err(|err| UploadError::BadRequest(err.to_string()))?;
            let final_path = target_dir.join(&task.target_file_name);
            if tokio::fs::metadata(&final_path).await.is_ok()
                && matches!(task.status, UploadTaskStatus::Completed)
            {
                task.error = None;
            }
        },
        Err(err) => return Err(err.into()),
    }
    Ok(task)
}

async fn finalize_upload(
    state: &Arc<LocalMediaState>,
    task: &mut UploadTaskRecord,
    blob_path: &Path,
) -> UploadResult<()> {
    let target_dir = resolve_media_path(state.root_dir(), &task.target_dir)
        .map_err(|err| UploadError::BadRequest(err.to_string()))?;
    let mut final_path = target_dir.join(&task.target_file_name);

    if tokio::fs::metadata(&final_path).await.is_ok() {
        // Another upload may have claimed the original target name while this
        // task was still staging. Resolve that collision here so the final move
        // stays atomic and never overwrites an existing media file.
        let renamed = resolve_available_file_name(
            state.root_dir(),
            &task.target_dir,
            &task.source_file_name,
            &upload_store::list_tasks(&state.upload_root()).await?,
        )
        .await?;
        task.target_file_name = renamed.clone();
        task.target_relative_path = join_relative_path(&task.target_dir, &renamed);
        final_path = target_dir.join(renamed);
    }

    tokio::fs::rename(blob_path, &final_path).await?;
    task.uploaded_bytes = task.file_size;
    task.status = UploadTaskStatus::Completed;
    task.error = None;
    Ok(())
}

async fn resolve_available_file_name(
    root_dir: &Path,
    target_dir: &str,
    source_file_name: &str,
    existing_tasks: &[UploadTaskRecord],
) -> UploadResult<String> {
    let target_base = resolve_media_path(root_dir, target_dir)
        .map_err(|err| UploadError::BadRequest(err.to_string()))?;
    let reserved_names = existing_tasks
        .iter()
        .filter(|task| {
            task.target_dir == target_dir
                && matches!(task.status, UploadTaskStatus::Created | UploadTaskStatus::Partial)
        })
        .map(|task| task.target_file_name.clone())
        .collect::<HashSet<_>>();

    // Reserve names already claimed by in-flight uploads so two partial tasks
    // created in quick succession do not target the same final file.
    let stem = std::path::Path::new(source_file_name)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(source_file_name);
    let ext = std::path::Path::new(source_file_name)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| format!(".{value}"))
        .unwrap_or_default();

    let mut candidate = source_file_name.to_string();
    let mut suffix = 1usize;
    while reserved_names.contains(&candidate)
        || tokio::fs::metadata(target_base.join(&candidate))
            .await
            .is_ok()
    {
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

#[cfg(test)]
mod tests {
    use axum::body::Bytes;
    use static_flow_media_types::{CreateUploadTaskRequest, UploadTaskStatus};

    use super::{
        append_upload_chunk, create_or_resume_upload_task, delete_upload_task, get_upload_task,
    };
    use crate::state::LocalMediaState;

    #[tokio::test]
    async fn create_task_reuses_existing_partial_task() {
        let root = tempfile::tempdir().expect("root");
        let cache = tempfile::tempdir().expect("cache");
        let state =
            LocalMediaState::new_for_test(root.path().to_path_buf(), cache.path().to_path_buf());

        let first = create_or_resume_upload_task(state.clone(), CreateUploadTaskRequest {
            target_dir: String::new(),
            source_file_name: "clip.mp4".to_string(),
            file_size: 10,
            last_modified_ms: 123,
            mime_type: Some("video/mp4".to_string()),
        })
        .await
        .expect("first task");

        let second = create_or_resume_upload_task(state.clone(), CreateUploadTaskRequest {
            target_dir: String::new(),
            source_file_name: "clip.mp4".to_string(),
            file_size: 10,
            last_modified_ms: 123,
            mime_type: Some("video/mp4".to_string()),
        })
        .await
        .expect("second task");

        assert_eq!(first.task.task_id, second.task.task_id);
    }

    #[tokio::test]
    async fn create_task_auto_renames_when_destination_exists() {
        let root = tempfile::tempdir().expect("root");
        let cache = tempfile::tempdir().expect("cache");
        std::fs::write(root.path().join("clip.mp4"), b"existing").expect("existing file");
        let state =
            LocalMediaState::new_for_test(root.path().to_path_buf(), cache.path().to_path_buf());

        let response = create_or_resume_upload_task(state, CreateUploadTaskRequest {
            target_dir: String::new(),
            source_file_name: "clip.mp4".to_string(),
            file_size: 10,
            last_modified_ms: 123,
            mime_type: None,
        })
        .await
        .expect("task");

        assert_eq!(response.task.target_file_name, "clip (1).mp4");
    }

    #[tokio::test]
    async fn append_chunk_rejects_wrong_offset() {
        let root = tempfile::tempdir().expect("root");
        let cache = tempfile::tempdir().expect("cache");
        let state =
            LocalMediaState::new_for_test(root.path().to_path_buf(), cache.path().to_path_buf());
        let task = create_or_resume_upload_task(state.clone(), CreateUploadTaskRequest {
            target_dir: String::new(),
            source_file_name: "clip.mp4".to_string(),
            file_size: 10,
            last_modified_ms: 123,
            mime_type: None,
        })
        .await
        .expect("task");

        let err = append_upload_chunk(state, &task.task.task_id, 5, Bytes::from_static(b"abc"))
            .await
            .expect_err("offset mismatch must fail");

        assert!(err.to_string().contains("offset mismatch"));
    }

    #[tokio::test]
    async fn append_chunk_finalizes_into_target_directory() {
        let root = tempfile::tempdir().expect("root");
        let cache = tempfile::tempdir().expect("cache");
        let state =
            LocalMediaState::new_for_test(root.path().to_path_buf(), cache.path().to_path_buf());
        let created = create_or_resume_upload_task(state.clone(), CreateUploadTaskRequest {
            target_dir: String::new(),
            source_file_name: "clip.mp4".to_string(),
            file_size: 3,
            last_modified_ms: 1,
            mime_type: Some("video/mp4".to_string()),
        })
        .await
        .expect("task");

        let updated = append_upload_chunk(
            state.clone(),
            &created.task.task_id,
            0,
            Bytes::from_static(b"abc"),
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
        let created = create_or_resume_upload_task(state.clone(), CreateUploadTaskRequest {
            target_dir: String::new(),
            source_file_name: "clip.mp4".to_string(),
            file_size: 8,
            last_modified_ms: 1,
            mime_type: None,
        })
        .await
        .expect("task");
        append_upload_chunk(state.clone(), &created.task.task_id, 0, Bytes::from_static(b"abc"))
            .await
            .expect("append");

        let canceled = delete_upload_task(state.clone(), &created.task.task_id)
            .await
            .expect("delete task");

        assert_eq!(canceled.status, UploadTaskStatus::Canceled);
        assert!(tokio::fs::metadata(
            state
                .upload_task_dir(&created.task.task_id)
                .join("blob.part")
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn get_task_reconciles_uploaded_bytes_from_part_file() {
        let root = tempfile::tempdir().expect("root");
        let cache = tempfile::tempdir().expect("cache");
        let state =
            LocalMediaState::new_for_test(root.path().to_path_buf(), cache.path().to_path_buf());
        let created = create_or_resume_upload_task(state.clone(), CreateUploadTaskRequest {
            target_dir: String::new(),
            source_file_name: "clip.mp4".to_string(),
            file_size: 10,
            last_modified_ms: 1,
            mime_type: None,
        })
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
}
