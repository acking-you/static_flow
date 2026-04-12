use std::{
    cmp::Ordering,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use anyhow::{Context, Result};
use tokio::fs::{self, ReadDir};

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
    let mut entries = Vec::new();
    let mut read_dir = fs::read_dir(absolute_dir)
        .await
        .with_context(|| format!("failed to read directory {}", absolute_dir.display()))?;

    while let Some(entry) = next_entry(&mut read_dir).await? {
        let file_name = entry.file_name().to_string_lossy().to_string();
        if file_name.starts_with('.') {
            continue;
        }

        let file_type = entry
            .file_type()
            .await
            .with_context(|| format!("failed to read file type for {}", entry.path().display()))?;
        let relative_path = join_relative(relative_dir, &file_name);

        if file_type.is_dir() {
            let modified_at_ms = entry
                .metadata()
                .await
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
                .and_then(|duration| i64::try_from(duration.as_millis()).ok());
            entries.push(LocalMediaEntry {
                kind: LocalMediaEntryKind::Directory,
                name: file_name,
                relative_path,
                size_bytes: None,
                modified_at_ms,
                extension: None,
                poster_url: None,
            });
            continue;
        }

        if !file_type.is_file() || !is_video_name(&file_name) {
            continue;
        }

        let metadata = entry
            .metadata()
            .await
            .with_context(|| format!("failed to stat {}", entry.path().display()))?;
        let modified_at_ms = metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .and_then(|duration| i64::try_from(duration.as_millis()).ok());
        let poster_url = poster_url_for_relative_path(&relative_path);

        entries.push(LocalMediaEntry {
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
        });
    }

    Ok(entries)
}

async fn next_entry(read_dir: &mut ReadDir) -> Result<Option<tokio::fs::DirEntry>> {
    read_dir
        .next_entry()
        .await
        .context("failed to advance local media directory iterator")
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
    use super::poster_url_for_relative_path;

    #[test]
    fn poster_url_uses_admin_endpoint_and_encodes_file_path() {
        assert_eq!(
            poster_url_for_relative_path("目录/clip 01.mkv"),
            "/admin/local-media/api/poster?file=%E7%9B%AE%E5%BD%95%2Fclip%2001.mkv"
        );
    }
}
