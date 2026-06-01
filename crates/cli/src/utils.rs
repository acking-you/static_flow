use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use gray_matter::{engine::YAML, Matter};
use image::{DynamicImage, ImageFormat};
use resvg::{tiny_skia, usvg};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use static_flow_shared::LocalizedText;

const SVG_EMBED_MAX_SIDE: u32 = 1024;

#[derive(Debug, Default, Deserialize)]
pub struct Frontmatter {
    pub title: Option<String>,
    pub summary: Option<String>,
    pub content_en: Option<String>,
    pub detailed_summary: Option<LocalizedText>,
    pub detailed_summary_zh: Option<String>,
    pub detailed_summary_en: Option<String>,
    pub tags: Option<Vec<String>>,
    pub category: Option<String>,
    pub category_description: Option<String>,
    pub author: Option<String>,
    pub date: Option<String>,
    pub featured_image: Option<String>,
    pub read_time: Option<i32>,
}

impl Frontmatter {
    pub fn normalized_content_en(self_content_en: Option<String>) -> Option<String> {
        self_content_en
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    }

    pub fn normalized_detailed_summary(
        detailed_summary: Option<LocalizedText>,
        detailed_summary_zh: Option<String>,
        detailed_summary_en: Option<String>,
    ) -> Option<LocalizedText> {
        let mut merged = detailed_summary.unwrap_or(LocalizedText {
            zh: None,
            en: None,
        });
        if detailed_summary_zh.is_some() {
            merged.zh = detailed_summary_zh;
        }
        if detailed_summary_en.is_some() {
            merged.en = detailed_summary_en;
        }
        merged.normalized()
    }
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
    let exts = ["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg"];

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

#[derive(Debug, Clone)]
pub struct RasterizedSvgForEmbedding {
    pub png_bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

pub fn rasterize_svg_for_embedding(
    path: &Path,
    bytes: &[u8],
) -> Result<Option<RasterizedSvgForEmbedding>> {
    if !is_svg_extension(path) && !looks_like_svg(bytes) {
        return Ok(None);
    }

    match rasterize_svg_to_png(bytes, SVG_EMBED_MAX_SIDE) {
        Ok(rasterized) => Ok(Some(rasterized)),
        Err(err) => {
            tracing::warn!(
                "Failed to rasterize SVG for embedding (fallback to raw bytes): path={}, \
                 error={err}",
                path.display()
            );
            Ok(None)
        },
    }
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

fn is_svg_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("svg"))
        .unwrap_or(false)
}

fn looks_like_svg(bytes: &[u8]) -> bool {
    let sample_len = bytes.len().min(2048);
    let sample = std::str::from_utf8(&bytes[..sample_len]).unwrap_or_default();
    let lowered = sample.to_ascii_lowercase();
    lowered.contains("<svg")
}

fn rasterize_svg_to_png(bytes: &[u8], max_side: u32) -> Result<RasterizedSvgForEmbedding> {
    let options = usvg::Options::default();
    let tree = usvg::Tree::from_data(bytes, &options).context("failed to parse svg")?;

    let size = tree.size();
    let src_width = size.width().round().max(1.0);
    let src_height = size.height().round().max(1.0);
    let max_src_side = src_width.max(src_height);
    let scale = (max_side as f32 / max_src_side).min(1.0);
    let target_width = (src_width * scale).round().max(1.0) as u32;
    let target_height = (src_height * scale).round().max(1.0) as u32;

    let mut pixmap =
        tiny_skia::Pixmap::new(target_width, target_height).context("failed to allocate pixmap")?;
    let transform = tiny_skia::Transform::from_scale(scale, scale);
    let mut pixmap_mut = pixmap.as_mut();
    resvg::render(&tree, transform, &mut pixmap_mut);
    let png_bytes = pixmap
        .encode_png()
        .context("failed to encode rasterized svg")?;

    Ok(RasterizedSvgForEmbedding {
        png_bytes,
        width: target_width,
        height: target_height,
    })
}
