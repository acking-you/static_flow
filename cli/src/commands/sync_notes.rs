use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use image::GenericImageView;
use regex::Regex;
use static_flow_shared::{
    embedding::{detect_language, embed_text_with_language, TextEmbeddingLanguage},
    normalize_taxonomy_key,
};

use crate::{
    db::{connect_db, ensure_vector_index, upsert_articles, upsert_images, upsert_taxonomies},
    schema::{ArticleRecord, ImageRecord, TaxonomyRecord},
    utils::{
        collect_markdown_files, encode_thumbnail, estimate_read_time, hash_bytes,
        markdown_filename, normalize_markdown_path, parse_markdown,
    },
};

const IMAGE_LINK_PATTERN: &str = r#"!\[[^\]]*\]\(([^)\s]+)(?:\s+"[^"]*")?\)"#;

struct SyncConfig {
    recursive: bool,
    generate_thumbnail: bool,
    thumbnail_size: u32,
    language: Option<TextEmbeddingLanguage>,
    default_category: String,
    default_author: String,
}

struct SyncOutcome {
    articles: usize,
    images: usize,
}


pub struct SyncNotesOptions {
    pub recursive: bool,
    pub generate_thumbnail: bool,
    pub thumbnail_size: u32,
    pub language: Option<String>,
    pub default_category: String,
    pub default_author: String,
}

pub async fn run(db_path: &Path, dir: &Path, options: SyncNotesOptions) -> Result<()> {
    let SyncNotesOptions {
        recursive,
        generate_thumbnail,
        thumbnail_size,
        language,
        default_category,
        default_author,
    } = options;
    let db = connect_db(db_path).await?;
    let articles_table = db
        .open_table("articles")
        .execute()
        .await
        .context("articles table not found; run `sf-cli init` first")?;
    let images_table = db
        .open_table("images")
        .execute()
        .await
        .context("images table not found; run `sf-cli init` first")?;
    let taxonomies_table = db
        .open_table("taxonomies")
        .execute()
        .await
        .context("taxonomies table not found; run `sf-cli init` first")?;

    let language = match language.as_deref() {
        Some("en") => Some(TextEmbeddingLanguage::English),
        Some("zh") => Some(TextEmbeddingLanguage::Chinese),
        None => None,
        Some(value) => anyhow::bail!("unsupported language hint: {value}"),
    };

    let config = SyncConfig {
        recursive,
        generate_thumbnail,
        thumbnail_size,
        language,
        default_category,
        default_author,
    };

    let outcome =
        sync_notes(dir, &articles_table, &images_table, &taxonomies_table, &config).await?;

    if let Err(err) = ensure_vector_index(&articles_table, "vector_en").await {
        tracing::warn!("Failed to create vector index on articles (vector_en): {err}");
    }
    if let Err(err) = ensure_vector_index(&articles_table, "vector_zh").await {
        tracing::warn!("Failed to create vector index on articles (vector_zh): {err}");
    }
    if let Err(err) = ensure_vector_index(&images_table, "vector").await {
        tracing::warn!("Failed to create vector index on images: {err}");
    }

    tracing::info!("Synced notes: {} articles, {} images.", outcome.articles, outcome.images);
    Ok(())
}

async fn sync_notes(
    dir: &Path,
    articles_table: &lancedb::Table,
    images_table: &lancedb::Table,
    taxonomies_table: &lancedb::Table,
    config: &SyncConfig,
) -> Result<SyncOutcome> {
    let markdown_files = collect_markdown_files(dir, config.recursive)?;
    if markdown_files.is_empty() {
        tracing::info!("No markdown files found in {}", dir.display());
        return Ok(SyncOutcome {
            articles: 0,
            images: 0,
        });
    }

    let image_regex = Regex::new(IMAGE_LINK_PATTERN).context("invalid image regex")?;

    let mut image_store: Vec<ImageRecord> = Vec::new();
    let mut image_index_by_source: HashMap<PathBuf, String> = HashMap::new();
    let mut image_id_seen: HashSet<String> = HashSet::new();
    let mut article_store: Vec<ArticleRecord> = Vec::new();
    let mut taxonomy_store: HashMap<String, TaxonomyRecord> = HashMap::new();

    for markdown_path in markdown_files {
        let markdown_text = fs::read_to_string(&markdown_path)
            .with_context(|| format!("failed to read markdown {}", markdown_path.display()))?;
        let (frontmatter, body) = parse_markdown(&markdown_text)?;

        let featured_image_source = frontmatter.featured_image.clone();

        let title = frontmatter.title.trim().to_string();
        if title.is_empty() {
            anyhow::bail!("frontmatter title is required: {}", markdown_path.display());
        }

        let article_id = normalize_markdown_path(&relative_article_id(dir, &markdown_path));

        let summary = frontmatter
            .summary
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| build_summary(&body));
        let tags = frontmatter
            .tags
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| vec!["notes".to_string()]);
        let category = frontmatter
            .category
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| config.default_category.clone());
        let category_description = frontmatter
            .category_description
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let author = frontmatter
            .author
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| config.default_author.clone());
        let date = frontmatter
            .date
            .unwrap_or_else(|| chrono::Utc::now().format("%Y-%m-%d").to_string());
        let read_time = frontmatter
            .read_time
            .unwrap_or_else(|| estimate_read_time(&body));

        upsert_taxonomy_entry(
            &mut taxonomy_store,
            "category",
            &category,
            category_description.as_deref(),
        );
        for tag in &tags {
            upsert_taxonomy_entry(&mut taxonomy_store, "tag", tag, None);
        }

        let (rewritten_body, mapped_images) = rewrite_image_links(
            &body,
            &markdown_path,
            &image_regex,
            config,
            &mut image_store,
            &mut image_index_by_source,
            &mut image_id_seen,
        )?;

        let featured_image = resolve_featured_image(
            featured_image_source,
            &markdown_path,
            &mapped_images,
            config,
            &mut image_store,
            &mut image_index_by_source,
            &mut image_id_seen,
        );

        let combined_text = format!("{} {} {}", title, summary, rewritten_body);
        let embedding_language = config
            .language
            .unwrap_or_else(|| detect_language(&combined_text));
        let embedding = embed_text_with_language(&combined_text, embedding_language);
        let (vector_en, vector_zh) = match embedding_language {
            TextEmbeddingLanguage::English => (Some(embedding), None),
            TextEmbeddingLanguage::Chinese => (None, Some(embedding)),
        };

        let now_ms = chrono::Utc::now().timestamp_millis();
        article_store.push(ArticleRecord {
            id: article_id,
            title,
            content: rewritten_body,
            summary,
            tags,
            category,
            author,
            date,
            featured_image,
            read_time,
            vector_en,
            vector_zh,
            created_at: now_ms,
            updated_at: now_ms,
        });
    }

    for chunk in image_store.chunks(64) {
        upsert_images(images_table, chunk).await?;
    }
    for chunk in article_store.chunks(64) {
        upsert_articles(articles_table, chunk).await?;
    }
    let taxonomy_records = taxonomy_store.into_values().collect::<Vec<_>>();
    for chunk in taxonomy_records.chunks(64) {
        upsert_taxonomies(taxonomies_table, chunk).await?;
    }

    Ok(SyncOutcome {
        articles: article_store.len(),
        images: image_store.len(),
    })
}

fn upsert_taxonomy_entry(
    taxonomy_store: &mut HashMap<String, TaxonomyRecord>,
    kind: &str,
    name: &str,
    description: Option<&str>,
) {
    let key = normalize_taxonomy_key(name);
    if key.is_empty() {
        return;
    }

    let now_ms = chrono::Utc::now().timestamp_millis();
    let id = format!("{kind}:{key}");
    let next_description = description
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| Some(name.trim().to_string()));

    match taxonomy_store.get_mut(&id) {
        Some(existing) => {
            existing.name = name.trim().to_string();
            if next_description.is_some() {
                existing.description = next_description;
            }
            existing.updated_at = now_ms;
        },
        None => {
            taxonomy_store.insert(id.clone(), TaxonomyRecord {
                id,
                kind: kind.to_string(),
                key,
                name: name.trim().to_string(),
                description: next_description,
                created_at: now_ms,
                updated_at: now_ms,
            });
        },
    }
}

fn rewrite_image_links(
    markdown: &str,
    markdown_path: &Path,
    image_regex: &Regex,
    config: &SyncConfig,
    image_store: &mut Vec<ImageRecord>,
    image_index_by_source: &mut HashMap<PathBuf, String>,
    image_id_seen: &mut HashSet<String>,
) -> Result<(String, HashMap<String, String>)> {
    let mut path_mapping: HashMap<String, String> = HashMap::new();
    let parent = markdown_path.parent().unwrap_or_else(|| Path::new("."));

    let captures = image_regex.captures_iter(markdown).collect::<Vec<_>>();

    for raw_path in captures {
        let Some(raw_path) = raw_path.get(1).map(|m| m.as_str().to_string()) else {
            continue;
        };

        if !is_local_image_path(&raw_path) {
            continue;
        }

        if path_mapping.contains_key(&raw_path) {
            continue;
        }

        let resolved = parent.join(raw_path.as_str());
        if !resolved.exists() {
            tracing::warn!(
                "Image not found when syncing notes: {} (from {})",
                resolved.display(),
                markdown_path.display()
            );
            continue;
        }

        let canonical = resolved.canonicalize().unwrap_or_else(|_| resolved.clone());
        let image_id = if let Some(existing_id) = image_index_by_source.get(&canonical) {
            existing_id.clone()
        } else {
            let record = build_image_record(&canonical, config)?;
            let id = record.id.clone();
            image_index_by_source.insert(canonical.clone(), id.clone());
            if image_id_seen.insert(id.clone()) {
                image_store.push(record);
            }
            id
        };

        let mapped = format!("images/{}", image_id);
        path_mapping.insert(raw_path.clone(), mapped.clone());
    }

    let rewritten = image_regex
        .replace_all(markdown, |captures: &regex::Captures<'_>| {
            let Some(raw_path) = captures.get(1).map(|m| m.as_str()) else {
                return captures
                    .get(0)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default();
            };
            if let Some(mapped) = path_mapping.get(raw_path) {
                // avoid changing already-rewritten links
                if raw_path == mapped {
                    return captures
                        .get(0)
                        .map(|m| m.as_str().to_string())
                        .unwrap_or_default();
                }
                return captures
                    .get(0)
                    .map(|m| m.as_str().replacen(raw_path, mapped, 1))
                    .unwrap_or_default();
            }

            captures
                .get(0)
                .map(|m| m.as_str().to_string())
                .unwrap_or_default()
        })
        .to_string();

    Ok((rewritten, path_mapping))
}

fn build_image_record(path: &Path, config: &SyncConfig) -> Result<ImageRecord> {
    let bytes =
        fs::read(path).with_context(|| format!("failed to read image {}", path.display()))?;
    let id = hash_bytes(&bytes);
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("image")
        .to_string();

    let mut metadata = serde_json::json!({
        "filename": filename,
        "bytes": bytes.len(),
        "source": path.display().to_string(),
    });

    let (vector, thumbnail) = match image::load_from_memory(&bytes) {
        Ok(img) => {
            let (w, h) = img.dimensions();
            metadata["width"] = serde_json::json!(w);
            metadata["height"] = serde_json::json!(h);
            let thumb = if config.generate_thumbnail {
                Some(encode_thumbnail(&img, config.thumbnail_size)?)
            } else {
                None
            };
            (static_flow_shared::embedding::embed_image_bytes(&bytes), thumb)
        },
        Err(_) => (static_flow_shared::embedding::embed_image_bytes(&bytes), None),
    };

    Ok(ImageRecord {
        id,
        filename,
        data: bytes,
        thumbnail,
        vector,
        metadata: metadata.to_string(),
        created_at: chrono::Utc::now().timestamp_millis(),
    })
}

fn resolve_featured_image(
    featured_image: Option<String>,
    markdown_path: &Path,
    mapped_images: &HashMap<String, String>,
    config: &SyncConfig,
    image_store: &mut Vec<ImageRecord>,
    source_index: &mut HashMap<PathBuf, String>,
    image_id_seen: &mut HashSet<String>,
) -> Option<String> {
    let featured = featured_image?.trim().to_string();
    if featured.is_empty() {
        return None;
    }
    if featured.starts_with("images/") {
        let id = featured.trim_start_matches("images/");
        if is_sha256_hex(id) {
            return Some(featured);
        }
    }
    if let Some(mapped) = mapped_images.get(&featured) {
        return Some(mapped.clone());
    }

    let parent = markdown_path.parent().unwrap_or_else(|| Path::new("."));
    let resolved = parent.join(&featured);
    if !resolved.exists() {
        tracing::warn!(
            "Featured image not found when syncing notes: {} (from {})",
            resolved.display(),
            markdown_path.display()
        );
        return None;
    }
    let canonical = resolved.canonicalize().unwrap_or(resolved);

    if let Some(id) = source_index.get(&canonical) {
        return Some(format!("images/{id}"));
    }

    let record = match build_image_record(&canonical, config) {
        Ok(record) => record,
        Err(err) => {
            tracing::warn!("Failed to import featured image {}: {}", canonical.display(), err);
            return None;
        },
    };

    let id = record.id.clone();
    source_index.insert(canonical, id.clone());
    if image_id_seen.insert(id.clone()) {
        image_store.push(record);
    }

    Some(format!("images/{id}"))
}

fn relative_article_id(root: &Path, markdown_path: &Path) -> String {
    let relative = markdown_path.strip_prefix(root).unwrap_or(markdown_path);
    let normalized = normalize_markdown_path(&relative.to_string_lossy());
    let raw_id = if normalized.to_lowercase().ends_with(".md") {
        normalized[..normalized.len() - 3].to_string()
    } else if normalized.is_empty() {
        markdown_filename(markdown_path)
    } else {
        normalized
    };

    sanitize_article_id(&raw_id)
}

fn build_summary(content: &str) -> String {
    let compact = content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let summary = compact.chars().take(180).collect::<String>();
    if summary.trim().is_empty() {
        "No summary available".to_string()
    } else {
        summary
    }
}

fn is_local_image_path(path: &str) -> bool {
    !(path.starts_with("http://")
        || path.starts_with("https://")
        || path.starts_with("data:")
        || path.starts_with("/"))
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.chars().all(|ch| ch.is_ascii_hexdigit())
}

fn sanitize_article_id(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut last_dash = false;

    for ch in value.chars() {
        let normalized = if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-') {
            ch.to_ascii_lowercase()
        } else {
            '-'
        };

        if normalized == '-' {
            if !last_dash {
                result.push('-');
            }
            last_dash = true;
        } else {
            result.push(normalized);
            last_dash = false;
        }
    }

    let trimmed = result.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "article".to_string()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_remote_image_paths_untouched() {
        assert!(!is_local_image_path("https://example.com/a.png"));
        assert!(!is_local_image_path("http://example.com/a.png"));
        assert!(!is_local_image_path("data:image/png;base64,abc"));
        assert!(!is_local_image_path("/assets/a.png"));
    }

    #[test]
    fn accepts_relative_image_paths() {
        assert!(is_local_image_path("images/a.png"));
        assert!(is_local_image_path("../assets/a.png"));
    }

    #[test]
    fn trims_article_id_suffix() {
        let id = relative_article_id(Path::new("notes"), Path::new("notes/rust/first-post.md"));
        assert_eq!(id, "rust-first-post");
    }

    #[test]
    fn sanitize_article_id_normalizes_symbols() {
        let id = sanitize_article_id("Rust/高级 + Notes.md");
        assert_eq!(id, "rust-notes-md");
    }

    #[test]
    fn detects_sha256_hash() {
        let hash = "1a31f145e050ecfdd6f6ec2a4dbf4f31f67187f65fcd4f95f5f6c68ca68cfb7b";
        assert!(is_sha256_hex(hash));
        assert!(!is_sha256_hex("short"));
        assert!(!is_sha256_hex("z123456789012345678901234567890123456789012345678901234567890123"));
    }
}
