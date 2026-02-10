use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use gray_matter::{engine::YAML, Matter};
use image::{DynamicImage, ImageFormat};
use serde::Deserialize;
use sha2::{Digest, Sha256};

#[derive(Debug, Default, Deserialize)]
pub struct Frontmatter {
    pub title: String,
    pub summary: Option<String>,
    pub tags: Option<Vec<String>>,
    pub category: Option<String>,
    pub category_description: Option<String>,
    pub author: Option<String>,
    pub date: Option<String>,
    pub featured_image: Option<String>,
    pub read_time: Option<i32>,
}

pub fn parse_markdown(content: &str) -> Result<(Frontmatter, String)> {
    let matter = Matter::<YAML>::new();
    let parsed = matter.parse(content);

    let frontmatter = parsed
        .data
        .map(|data| data.deserialize::<Frontmatter>())
        .transpose()?
        .unwrap_or_default();

    Ok((frontmatter, parsed.content))
}

pub fn parse_tags(tags: &str) -> Vec<String> {
    tags.split(',')
        .map(|tag| tag.trim())
        .filter(|tag| !tag.is_empty())
        .map(|tag| tag.to_string())
        .collect()
}

pub fn estimate_read_time(content: &str) -> i32 {
    let words = content.split_whitespace().count();
    let minutes = (words as f32 / 200.0).ceil() as i32;
    minutes.max(1)
}

pub fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

pub fn relative_filename(root: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    relative
        .to_string_lossy()
        .replace('\\', "/")
        .trim_start_matches('/')
        .to_string()
}

pub fn parse_vector(json: &str, dim: usize) -> Result<Vec<f32>> {
    let vector: Vec<f32> = serde_json::from_str(json).context("invalid vector JSON")?;
    if vector.len() != dim {
        anyhow::bail!("vector length {} does not match {}", vector.len(), dim);
    }
    Ok(vector)
}

pub fn encode_thumbnail(image: &DynamicImage, size: u32) -> Result<Vec<u8>> {
    let thumbnail = image.thumbnail(size, size);
    let mut buffer = std::io::Cursor::new(Vec::new());
    thumbnail.write_to(&mut buffer, ImageFormat::Png)?;
    Ok(buffer.into_inner())
}

pub fn collect_image_files(dir: &Path, recursive: bool) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let exts = ["png", "jpg", "jpeg", "gif", "webp", "bmp"];

    if recursive {
        for entry in walkdir::WalkDir::new(dir)
            .into_iter()
            .filter_map(Result::ok)
        {
            if entry.file_type().is_file() {
                let path = entry.path();
                if has_image_extension(path, &exts) {
                    files.push(path.to_path_buf());
                }
            }
        }
    } else {
        for entry in fs::read_dir(dir).context("failed to read image directory")? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() && has_image_extension(&path, &exts) {
                files.push(path);
            }
        }
    }

    Ok(files)
}

pub fn collect_markdown_files(dir: &Path, recursive: bool) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();

    if recursive {
        for entry in walkdir::WalkDir::new(dir)
            .into_iter()
            .filter_map(Result::ok)
        {
            if entry.file_type().is_file() {
                let path = entry.path();
                if path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .map(|ext| ext.eq_ignore_ascii_case("md"))
                    .unwrap_or(false)
                {
                    files.push(path.to_path_buf());
                }
            }
        }
    } else {
        for entry in fs::read_dir(dir).context("failed to read notes directory")? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file()
                && path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .map(|ext| ext.eq_ignore_ascii_case("md"))
                    .unwrap_or(false)
            {
                files.push(path);
            }
        }
    }

    Ok(files)
}

pub fn normalize_markdown_path(path: &str) -> String {
    path.replace('\\', "/")
        .trim_start_matches("./")
        .trim_start_matches('/')
        .to_string()
}

pub fn markdown_filename(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_string()
}

fn has_image_extension(path: &Path, exts: &[&str]) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| exts.iter().any(|item| ext.eq_ignore_ascii_case(item)))
        .unwrap_or(false)
}
