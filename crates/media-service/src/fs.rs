use std::{
    cmp::Ordering,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use anyhow::{Context, Result};
use tokio::fs;

use crate::{
    path_guard::{resolve_media_path, sanitize_relative_media_path},
    types::{LocalMediaEntry, LocalMediaEntryKind, LocalMediaListResponse},
    LocalMediaState,
};

pub async fn list_directory(
    state: &LocalMediaState,
    dir: Option<&str>,
    limit: usize,
    offset: usize,
) -> Result<LocalMediaListResponse> {
    let current_dir = normalize_relative_path(dir.unwrap_or_default())?;
    let absolute_dir = resolve_media_path(state.root_dir(), &current_dir)?;
    let metadata = fs::metadata(&absolute_dir)
        .await
        .with_context(|| format!("failed to stat {}", absolute_dir.display()))?;
    if !metadata.is_dir() {
        anyhow::bail!("requested media path is not a directory: {}", current_dir);
    }

    let mut entries = collect_entries(&absolute_dir, &current_dir).await?;
    entries.sort_by(compare_entries);

    let total = entries.len();
    let paged = entries
        .into_iter()
        .skip(offset)
        .take(limit)
        .collect::<Vec<_>>();
    let parent_dir = parent_relative_dir(&current_dir);

    Ok(LocalMediaListResponse {
        configured: true,
        current_dir,
        parent_dir,
        total,
        offset,
        limit,
        has_more: offset.saturating_add(paged.len()) < total,
        entries: paged,
    })
}

pub fn normalize_relative_path(relative: &str) -> Result<String> {
    let path = sanitize_relative_media_path(relative)?;
    Ok(path_to_relative_string(&path))
}

async fn collect_entries(absolute_dir: &Path, relative_dir: &str) -> Result<Vec<LocalMediaEntry>> {
    let absolute_dir = absolute_dir.to_path_buf();
    let relative_dir = relative_dir.to_string();
    tokio::task::spawn_blocking(move || collect_entries_blocking(&absolute_dir, &relative_dir))
        .await
        .context("failed to join local media directory listing task")?
}

fn collect_entries_blocking(
    absolute_dir: &Path,
    relative_dir: &str,
) -> Result<Vec<LocalMediaEntry>> {
    let read_dir = std::fs::read_dir(absolute_dir)
        .with_context(|| format!("failed to read directory {}", absolute_dir.display()))?;
    let mut entries = Vec::new();

    for entry_result in read_dir {
        let entry = match entry_result {
            Ok(entry) => entry,
            Err(err) => {
                tracing::warn!(
                    dir = %absolute_dir.display(),
                    error = %err,
                    "skipping unreadable local media directory entry"
                );
                continue;
            },
        };
        if let Some(entry) = build_entry_from_dir_entry(&entry, relative_dir) {
            entries.push(entry);
        }
    }

    Ok(entries)
}

fn build_entry_from_dir_entry(
    entry: &std::fs::DirEntry,
    relative_dir: &str,
) -> Option<LocalMediaEntry> {
    let path = entry.path();
    let file_name = entry.file_name().to_string_lossy().to_string();
    if file_name.starts_with('.') {
        return None;
    }

    let file_type = match entry.file_type() {
        Ok(file_type) => file_type,
        Err(err) => {
            tracing::warn!(path = %path.display(), error = %err, "skipping local media entry with unreadable file type");
            return None;
        },
    };
    let relative_path = join_relative(relative_dir, &file_name);

    if file_type.is_dir() {
        let modified_at_ms = entry
            .metadata()
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .and_then(|duration| i64::try_from(duration.as_millis()).ok());
        return Some(LocalMediaEntry {
            kind: LocalMediaEntryKind::Directory,
            name: file_name,
            relative_path,
            size_bytes: None,
            modified_at_ms,
            extension: None,
            poster_url: None,
        });
    }

    if !file_type.is_file() || !is_video_name(&file_name) {
        return None;
    }

    let metadata = match entry.metadata() {
        Ok(metadata) => metadata,
        Err(err) => {
            tracing::warn!(path = %path.display(), error = %err, "skipping local media entry with unreadable metadata");
            return None;
        },
    };
    let modified_at_ms = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .and_then(|duration| i64::try_from(duration.as_millis()).ok());
    let poster_url = poster_url_for_relative_path(&relative_path);

    Some(LocalMediaEntry {
        kind: LocalMediaEntryKind::Video,
        name: file_name.clone(),
        relative_path,
        size_bytes: Some(metadata.len()),
        modified_at_ms,
        extension: PathBuf::from(&file_name)
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| value.to_ascii_lowercase()),
        poster_url: Some(poster_url),
    })
}

fn compare_entries(left: &LocalMediaEntry, right: &LocalMediaEntry) -> Ordering {
    match (left.kind, right.kind) {
        (LocalMediaEntryKind::Directory, LocalMediaEntryKind::Video) => Ordering::Less,
        (LocalMediaEntryKind::Video, LocalMediaEntryKind::Directory) => Ordering::Greater,
        _ => left
            .name
            .to_ascii_lowercase()
            .cmp(&right.name.to_ascii_lowercase()),
    }
}

fn parent_relative_dir(current: &str) -> Option<String> {
    let path = Path::new(current);
    let parent = path.parent()?;
    let normalized = path_to_relative_string(parent);
    if normalized.is_empty() {
        Some(String::new())
    } else {
        Some(normalized)
    }
}

fn join_relative(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}

fn path_to_relative_string(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            std::path::Component::Normal(part) => Some(part.to_string_lossy().to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn is_video_name(file_name: &str) -> bool {
    let ext = Path::new(file_name)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase());
    matches!(
        ext.as_deref(),
        Some("mp4" | "m4v" | "mov" | "webm" | "mkv" | "avi" | "ts" | "mpeg" | "mpg")
    )
}

pub fn poster_url_for_relative_path(relative_path: &str) -> String {
    format!("/admin/local-media/api/poster?file={}", urlencoding::encode(relative_path))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{build_entry_from_dir_entry, poster_url_for_relative_path};

    #[test]
    fn poster_url_uses_admin_endpoint_and_encodes_file_path() {
        assert_eq!(
            poster_url_for_relative_path("目录/clip 01.mkv"),
            "/admin/local-media/api/poster?file=%E7%9B%AE%E5%BD%95%2Fclip%2001.mkv"
        );
    }

    #[test]
    fn build_entry_from_dir_entry_skips_files_that_disappear_mid_scan() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("demo.mp4");
        fs::write(&path, b"demo").expect("write test file");
        let entry = fs::read_dir(dir.path())
            .expect("read dir")
            .next()
            .expect("entry result")
            .expect("dir entry");
        fs::remove_file(&path).expect("remove test file");

        let item = build_entry_from_dir_entry(&entry, "");
        assert!(item.is_none());
    }

    #[test]
    fn build_entry_from_dir_entry_skips_hidden_service_state() {
        let dir = tempdir().expect("tempdir");
        let hidden = dir.path().join(".static-flow");
        fs::create_dir_all(&hidden).expect("create hidden dir");
        let entry = fs::read_dir(dir.path())
            .expect("read dir")
            .find_map(Result::ok)
            .expect("entry");

        assert!(build_entry_from_dir_entry(&entry, "").is_none());
    }
}
