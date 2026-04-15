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
    let task = serde_json::from_slice(&bytes).context("failed to decode upload task")?;
    Ok(Some(task))
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
        // Ignore directories that have not yet persisted a valid `task.json`.
        // Callers only want durable task records when restoring UI state.
        if let Some(task) = load_task(&entry.path()).await? {
            tasks.push(task);
        }
    }

    tasks.sort_by(|left, right| right.updated_at_ms.cmp(&left.updated_at_ms));
    Ok(tasks)
}

#[cfg(test)]
mod tests {
    use static_flow_media_types::{UploadTaskRecord, UploadTaskStatus};

    use super::{load_task, save_task};

    #[tokio::test]
    async fn save_and_load_round_trip() {
        let temp = tempfile::tempdir().expect("tempdir");
        let task = UploadTaskRecord {
            task_id: "task-1".to_string(),
            resume_key: "resume".to_string(),
            status: UploadTaskStatus::Created,
            target_dir: String::new(),
            source_file_name: "clip.mp4".to_string(),
            target_file_name: "clip.mp4".to_string(),
            target_relative_path: "clip.mp4".to_string(),
            file_size: 10,
            uploaded_bytes: 0,
            last_modified_ms: 1,
            mime_type: Some("video/mp4".to_string()),
            error: None,
            created_at_ms: 1,
            updated_at_ms: 1,
        };

        save_task(temp.path(), &task).await.expect("save task");
        let loaded = load_task(temp.path())
            .await
            .expect("load task")
            .expect("task present");

        assert_eq!(loaded, task);
    }
}
