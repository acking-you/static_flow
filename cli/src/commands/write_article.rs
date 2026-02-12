use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use image::GenericImageView;
use regex::Regex;
use static_flow_shared::{
    embedding::{
        detect_language, embed_image_bytes, embed_text_with_language, TextEmbeddingLanguage,
        TEXT_VECTOR_DIM_EN, TEXT_VECTOR_DIM_ZH,
    },
    normalize_taxonomy_key,
};

use crate::{
    db::{
        connect_db, ensure_vector_index, optimize_table_indexes, upsert_articles, upsert_images,
        upsert_taxonomies,
    },
    schema::{ArticleRecord, ImageRecord, TaxonomyRecord},
    utils::{
        encode_thumbnail, estimate_read_time, hash_bytes, parse_markdown, parse_tags, parse_vector,
        rasterize_svg_for_embedding, Frontmatter,
    },
};

const IMAGE_LINK_PATTERN: &str = r#"!\[[^\]]*\]\(([^)\s]+)(?:\s+"[^"]*")?\)"#;
const OBSIDIAN_IMAGE_LINK_PATTERN: &str = r"!\[\[([^\]]+)\]\]";

pub struct WriteArticleOptions {
    pub id: Option<String>,
    pub summary: Option<String>,
    pub tags: Option<String>,
    pub category: Option<String>,
    pub category_description: Option<String>,
    pub date: Option<String>,
    pub import_local_images: bool,
    pub media_roots: Vec<PathBuf>,
    pub generate_thumbnail: bool,
    pub thumbnail_size: u32,
    pub vector: Option<String>,
    pub vector_en: Option<String>,
    pub vector_zh: Option<String>,
    pub language: Option<String>,
    pub auto_optimize: bool,
}

struct ImageImportConfig {
    generate_thumbnail: bool,
    thumbnail_size: u32,
    media_roots: Vec<PathBuf>,
}

struct ImageImportState {
    store: Vec<ImageRecord>,
    index_by_source: HashMap<PathBuf, String>,
    id_seen: HashSet<String>,
    unresolved: HashSet<String>,
}

pub async fn run(db_path: &Path, file: &Path, options: WriteArticleOptions) -> Result<()> {
    let WriteArticleOptions {
        id,
        summary,
        tags,
        category,
        category_description,
        date: cli_date,
        import_local_images,
        media_roots,
        generate_thumbnail,
        thumbnail_size,
        vector,
        vector_en,
        vector_zh,
        language,
        auto_optimize,
    } = options;

    let db = connect_db(db_path).await?;
    let table = db
        .open_table("articles")
        .execute()
        .await
        .context("articles table not found; run `sf-cli init` first")?;
    let taxonomies_table = db
        .open_table("taxonomies")
        .execute()
        .await
        .context("taxonomies table not found; run `sf-cli init` first")?;
    let images_table = if import_local_images {
        Some(
            db.open_table("images")
                .execute()
                .await
                .context("images table not found; run `sf-cli init` first")?,
        )
    } else {
        None
    };

    let content = fs::read_to_string(file).context("failed to read markdown file")?;
    let (frontmatter, body) = parse_markdown(&content)?;
    let Frontmatter {
        title: frontmatter_title,
        summary: frontmatter_summary,
        tags: frontmatter_tags,
        category: frontmatter_category,
        category_description: frontmatter_category_description,
        author,
        date: frontmatter_date,
        featured_image,
        read_time,
    } = frontmatter;

    let title = resolve_title(file, frontmatter_title.as_deref().unwrap_or_default(), &body);

    let id = id.unwrap_or_else(|| {
        file.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string()
    });

    let summary = summary
        .or(frontmatter_summary)
        .filter(|value| !value.trim().is_empty())
        .context("summary is required (pass --summary or add summary to frontmatter)")?;
    let tags = if let Some(tags) = tags {
        parse_tags(&tags)
    } else if let Some(tags) = frontmatter_tags {
        tags
    } else {
        anyhow::bail!("tags are required (pass --tags or add tags to frontmatter)");
    };
    if tags.is_empty() {
        anyhow::bail!("tags are required (pass --tags or add tags to frontmatter)");
    }
    let category = category
        .or(frontmatter_category)
        .filter(|value| !value.trim().is_empty())
        .context("category is required (pass --category or add category to frontmatter)")?;
    let category_description = category_description
        .or(frontmatter_category_description)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .context(
            "category_description is required (pass --category-description or add \
             category_description to frontmatter)",
        )?;

    let image_import_config = ImageImportConfig {
        generate_thumbnail,
        thumbnail_size,
        media_roots,
    };

    let mut state = ImageImportState {
        store: Vec::new(),
        index_by_source: HashMap::new(),
        id_seen: HashSet::new(),
        unresolved: HashSet::new(),
    };

    let (body, mapped_images) = if import_local_images {
        rewrite_image_links(&body, file, &image_import_config, &mut state)?
    } else {
        (body, HashMap::new())
    };

    let featured_image = if import_local_images {
        resolve_featured_image(
            featured_image,
            file,
            &mapped_images,
            &image_import_config,
            &mut state,
        )
    } else {
        featured_image
    };

    // Fallback: pick a cover from the database when none is provided.
    let featured_image = if featured_image.is_none() {
        let fallback_img_table = match images_table.as_ref() {
            Some(t) => t.clone(),
            None => db
                .open_table("images")
                .execute()
                .await
                .context("images table not found for fallback cover")?,
        };
        match crate::db::query_fallback_cover(&fallback_img_table, &table).await {
            Ok(cover) => cover,
            Err(err) => {
                tracing::warn!("Failed to query fallback cover: {err}");
                None
            },
        }
    } else {
        featured_image
    };

    let read_time = read_time.unwrap_or_else(|| estimate_read_time(&body));
    let cli_date = normalize_cli_date(cli_date)?;
    let date = cli_date
        .or(frontmatter_date
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()))
        .unwrap_or_else(|| chrono::Utc::now().format("%Y-%m-%d").to_string());
    let author = author.unwrap_or_else(|| "Unknown".to_string());

    let combined_text = format!("{} {} {}", title, summary, body);
    let language = match language.as_deref() {
        Some("en") => TextEmbeddingLanguage::English,
        Some("zh") => TextEmbeddingLanguage::Chinese,
        _ => detect_language(&combined_text),
    };

    let mut vector_en = match vector_en {
        Some(json) => Some(parse_vector(&json, TEXT_VECTOR_DIM_EN)?),
        None => None,
    };
    let mut vector_zh = match vector_zh {
        Some(json) => Some(parse_vector(&json, TEXT_VECTOR_DIM_ZH)?),
        None => None,
    };

    if vector_en.is_none() && vector_zh.is_none() {
        if let Some(json) = vector {
            if let Ok(parsed) = parse_vector(&json, TEXT_VECTOR_DIM_EN) {
                vector_en = Some(parsed);
            } else if let Ok(parsed) = parse_vector(&json, TEXT_VECTOR_DIM_ZH) {
                vector_zh = Some(parsed);
            } else {
                anyhow::bail!("--vector does not match English or Chinese dimensions");
            }
        } else {
            match language {
                TextEmbeddingLanguage::English => {
                    vector_en = Some(embed_text_with_language(
                        &combined_text,
                        TextEmbeddingLanguage::English,
                    ));
                },
                TextEmbeddingLanguage::Chinese => {
                    vector_zh = Some(embed_text_with_language(
                        &combined_text,
                        TextEmbeddingLanguage::Chinese,
                    ));
                },
            }
        }
    }

    let now_ms = chrono::Utc::now().timestamp_millis();
    let record = ArticleRecord {
        id,
        title,
        content: body,
        summary,
        tags: tags.clone(),
        category: category.clone(),
        author,
        date,
        featured_image,
        read_time,
        vector_en,
        vector_zh,
        created_at: now_ms,
        updated_at: now_ms,
    };

    if let Some(images_table) = images_table.as_ref() {
        for chunk in state.store.chunks(64) {
            upsert_images(images_table, chunk).await?;
        }
    }

    if !state.unresolved.is_empty() {
        let mut missing = state.unresolved.into_iter().collect::<Vec<_>>();
        missing.sort();
        let preview = missing
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        tracing::warn!(
            "Some local images were not resolved ({} total). Please confirm media roots and rerun \
             with --media-root <path>. Examples: {}",
            missing.len(),
            preview
        );
    }

    upsert_articles(&table, &[record]).await?;

    let mut taxonomies = Vec::new();
    push_taxonomy_record(
        &mut taxonomies,
        "category",
        &category,
        Some(category_description.as_str()),
        now_ms,
    );
    for tag in &tags {
        push_taxonomy_record(&mut taxonomies, "tag", tag, None, now_ms);
    }
    upsert_taxonomies(&taxonomies_table, &taxonomies).await?;

    if let Err(err) = ensure_vector_index(&table, "vector_en").await {
        tracing::warn!("Failed to create vector index on articles (vector_en): {err}");
    }
    if let Err(err) = ensure_vector_index(&table, "vector_zh").await {
        tracing::warn!("Failed to create vector index on articles (vector_zh): {err}");
    }
    if let Some(images_table) = images_table.as_ref() {
        if let Err(err) = ensure_vector_index(images_table, "vector").await {
            tracing::warn!("Failed to create vector index on images: {err}");
        }
    }

    if auto_optimize {
        if let Err(err) = optimize_table_indexes(&table).await {
            tracing::warn!("Failed to optimize articles indexes after write-article: {err}");
        }
        if let Some(images_table) = images_table.as_ref() {
            if let Err(err) = optimize_table_indexes(images_table).await {
                tracing::warn!("Failed to optimize images indexes after write-article: {err}");
            }
        }
    }

    tracing::info!("Article written to LanceDB. Imported {} local images.", state.store.len());
    Ok(())
}

fn push_taxonomy_record(
    records: &mut Vec<TaxonomyRecord>,
    kind: &str,
    name: &str,
    description: Option<&str>,
    now_ms: i64,
) {
    let key = normalize_taxonomy_key(name);
    if key.is_empty() {
        return;
    }

    let description = description
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    records.push(TaxonomyRecord {
        id: format!("{kind}:{key}"),
        kind: kind.to_string(),
        key,
        name: name.trim().to_string(),
        description,
        created_at: now_ms,
        updated_at: now_ms,
    });
}

fn resolve_title(file: &Path, frontmatter_title: &str, body: &str) -> String {
    let frontmatter_title = frontmatter_title.trim();
    if !frontmatter_title.is_empty() {
        return frontmatter_title.to_string();
    }

    if let Some(heading) = first_markdown_heading(body) {
        return heading;
    }

    file.file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| "untitled".to_string())
}

fn first_markdown_heading(body: &str) -> Option<String> {
    for line in body.lines() {
        let line = line.trim();
        if !line.starts_with('#') {
            continue;
        }

        let heading = line.trim_start_matches('#').trim();
        let heading = heading.trim_end_matches('#').trim();
        if !heading.is_empty() {
            return Some(heading.to_string());
        }
    }
    None
}

fn rewrite_image_links(
    markdown: &str,
    markdown_path: &Path,
    config: &ImageImportConfig,
    state: &mut ImageImportState,
) -> Result<(String, HashMap<String, String>)> {
    let image_regex = Regex::new(IMAGE_LINK_PATTERN).context("invalid image regex")?;
    let obsidian_image_regex =
        Regex::new(OBSIDIAN_IMAGE_LINK_PATTERN).context("invalid obsidian image regex")?;
    let mut path_mapping: HashMap<String, String> = HashMap::new();

    for capture in image_regex.captures_iter(markdown) {
        let Some(raw_path) = capture.get(1).map(|m| m.as_str().to_string()) else {
            continue;
        };
        import_local_markdown_image(&raw_path, markdown_path, config, state, &mut path_mapping)?;
    }

    for capture in obsidian_image_regex.captures_iter(markdown) {
        let Some(inner) = capture.get(1).map(|m| m.as_str()) else {
            continue;
        };
        let (raw_path, _) = parse_obsidian_embed_target(inner);
        if raw_path.is_empty() {
            continue;
        }
        import_local_markdown_image(&raw_path, markdown_path, config, state, &mut path_mapping)?;
    }

    let rewritten_standard = image_regex
        .replace_all(markdown, |captures: &regex::Captures<'_>| {
            let Some(raw_path) = captures.get(1).map(|m| m.as_str()) else {
                return captures
                    .get(0)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default();
            };

            if let Some(mapped) = path_mapping.get(raw_path) {
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

    let rewritten = obsidian_image_regex
        .replace_all(rewritten_standard.as_str(), |captures: &regex::Captures<'_>| {
            let Some(inner) = captures.get(1).map(|m| m.as_str()) else {
                return captures
                    .get(0)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default();
            };
            let (raw_path, alt_hint) = parse_obsidian_embed_target(inner);
            if raw_path.is_empty() {
                return captures
                    .get(0)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default();
            }

            if let Some(mapped) = path_mapping.get(raw_path.as_str()) {
                let alt_text = alt_hint
                    .as_deref()
                    .filter(|hint| !looks_like_obsidian_size(hint))
                    .map(escape_markdown_alt_text)
                    .unwrap_or_default();
                return format!("![{alt_text}]({mapped})");
            }

            captures
                .get(0)
                .map(|m| m.as_str().to_string())
                .unwrap_or_default()
        })
        .to_string();

    Ok((rewritten, path_mapping))
}

fn import_local_markdown_image(
    raw_path: &str,
    markdown_path: &Path,
    config: &ImageImportConfig,
    state: &mut ImageImportState,
    path_mapping: &mut HashMap<String, String>,
) -> Result<()> {
    if !is_local_image_path(raw_path) {
        return Ok(());
    }
    if path_mapping.contains_key(raw_path) {
        return Ok(());
    }

    let Some(resolved) = resolve_local_asset_path(raw_path, markdown_path, &config.media_roots)
    else {
        state.unresolved.insert(raw_path.to_string());
        tracing::warn!(
            "Image not found when writing article: {} (from {}). Try --media-root <path> for \
             global Obsidian media directories.",
            raw_path,
            markdown_path.display(),
        );
        return Ok(());
    };
    let canonical = resolved.canonicalize().unwrap_or(resolved);
    let image_id = if let Some(existing_id) = state.index_by_source.get(&canonical) {
        existing_id.clone()
    } else {
        let record = build_image_record(&canonical, config)?;
        let id = record.id.clone();
        state.index_by_source.insert(canonical.clone(), id.clone());
        if state.id_seen.insert(id.clone()) {
            state.store.push(record);
        }
        id
    };

    path_mapping.insert(raw_path.to_string(), format!("images/{image_id}"));
    Ok(())
}

fn resolve_local_asset_path(
    raw_path: &str,
    markdown_path: &Path,
    media_roots: &[PathBuf],
) -> Option<PathBuf> {
    let trimmed = raw_path.trim();
    if trimmed.is_empty() {
        return None;
    }

    let source = PathBuf::from(trimmed);
    if source.is_absolute() && source.exists() {
        return Some(source);
    }

    let parent = markdown_path.parent().unwrap_or_else(|| Path::new("."));
    let direct = parent.join(trimmed);
    if direct.exists() {
        return Some(direct);
    }

    let normalized = trimmed.trim_start_matches("./").trim_start_matches('/');
    for root in media_roots {
        let by_raw = root.join(trimmed);
        if by_raw.exists() {
            return Some(by_raw);
        }
        let by_norm = root.join(normalized);
        if by_norm.exists() {
            return Some(by_norm);
        }
    }

    None
}

fn parse_obsidian_embed_target(embed: &str) -> (String, Option<String>) {
    let embed = embed.trim();
    if embed.is_empty() {
        return (String::new(), None);
    }

    let mut parts = embed.splitn(2, '|');
    let target = parts.next().unwrap_or_default().trim();
    let alt_hint = parts
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    // Obsidian links can append a heading like `file.png#section`; images only
    // need the underlying path.
    let target = target
        .split('#')
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    (target, alt_hint)
}

fn looks_like_obsidian_size(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty() {
        return false;
    }
    if value.chars().all(|ch| ch.is_ascii_digit()) {
        return true;
    }

    let lower = value.to_ascii_lowercase();
    if let Some((w, h)) = lower.split_once('x') {
        let valid_w = !w.is_empty() && w.chars().all(|ch| ch.is_ascii_digit());
        let valid_h = !h.is_empty() && h.chars().all(|ch| ch.is_ascii_digit());
        return valid_w && valid_h;
    }

    false
}

fn escape_markdown_alt_text(value: &str) -> String {
    value
        .replace('\\', r"\\")
        .replace('[', r"\[")
        .replace(']', r"\]")
}

fn resolve_featured_image(
    featured_image: Option<String>,
    markdown_path: &Path,
    mapped_images: &HashMap<String, String>,
    config: &ImageImportConfig,
    state: &mut ImageImportState,
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
    if !is_local_image_path(&featured) {
        return Some(featured);
    }

    let Some(resolved) = resolve_local_asset_path(&featured, markdown_path, &config.media_roots)
    else {
        state.unresolved.insert(featured.clone());
        tracing::warn!(
            "Featured image not found when writing article: {} (from {}). Try --media-root <path> \
             for global Obsidian media directories.",
            featured,
            markdown_path.display(),
        );
        return Some(featured);
    };
    let canonical = resolved.canonicalize().unwrap_or(resolved);

    if let Some(id) = state.index_by_source.get(&canonical) {
        return Some(format!("images/{id}"));
    }

    let record = match build_image_record(&canonical, config) {
        Ok(record) => record,
        Err(err) => {
            tracing::warn!("Failed to import featured image {}: {}", canonical.display(), err);
            return Some(featured);
        },
    };

    let id = record.id.clone();
    state.index_by_source.insert(canonical, id.clone());
    if state.id_seen.insert(id.clone()) {
        state.store.push(record);
    }

    Some(format!("images/{id}"))
}

fn build_image_record(path: &Path, config: &ImageImportConfig) -> Result<ImageRecord> {
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

    let rasterized_svg = rasterize_svg_for_embedding(path, &bytes)?;
    let (vector, thumbnail) = if let Some(rasterized) = rasterized_svg {
        metadata["width"] = serde_json::json!(rasterized.width);
        metadata["height"] = serde_json::json!(rasterized.height);
        metadata["embedding_input"] = serde_json::json!("svg_rasterized_png");
        let thumb = if config.generate_thumbnail {
            image::load_from_memory(&rasterized.png_bytes)
                .ok()
                .and_then(|img| encode_thumbnail(&img, config.thumbnail_size).ok())
        } else {
            None
        };
        (embed_image_bytes(&rasterized.png_bytes), thumb)
    } else {
        match image::load_from_memory(&bytes) {
            Ok(img) => {
                let (w, h) = img.dimensions();
                metadata["width"] = serde_json::json!(w);
                metadata["height"] = serde_json::json!(h);
                let thumb = if config.generate_thumbnail {
                    Some(encode_thumbnail(&img, config.thumbnail_size)?)
                } else {
                    None
                };
                (embed_image_bytes(&bytes), thumb)
            },
            Err(_) => (embed_image_bytes(&bytes), None),
        }
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

fn is_local_image_path(path: &str) -> bool {
    !(path.starts_with("http://")
        || path.starts_with("https://")
        || path.starts_with("data:")
        || path.starts_with("/"))
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.chars().all(|ch| ch.is_ascii_hexdigit())
}

fn normalize_cli_date(date: Option<String>) -> Result<Option<String>> {
    let Some(date) = date else {
        return Ok(None);
    };
    let date = date.trim();
    if date.is_empty() {
        return Ok(None);
    }

    chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .with_context(|| format!("invalid --date `{date}`; expected YYYY-MM-DD"))?;
    Ok(Some(date.to_string()))
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use super::{
        first_markdown_heading, is_local_image_path, looks_like_obsidian_size, normalize_cli_date,
        parse_obsidian_embed_target, resolve_local_asset_path, resolve_title,
    };

    #[test]
    fn resolve_title_prefers_frontmatter_title() {
        let title = resolve_title(Path::new("docs/demo.md"), "Frontmatter Title", "# Heading");
        assert_eq!(title, "Frontmatter Title");
    }

    #[test]
    fn resolve_title_falls_back_to_first_heading() {
        let title = resolve_title(Path::new("docs/demo.md"), "", "\n# Heading Title\n\nContent");
        assert_eq!(title, "Heading Title");
    }

    #[test]
    fn resolve_title_falls_back_to_file_stem() {
        let title = resolve_title(Path::new("docs/frontend-architecture.md"), "", "No heading");
        assert_eq!(title, "frontend-architecture");
    }

    #[test]
    fn first_markdown_heading_ignores_empty_heading_marks() {
        let heading = first_markdown_heading("###\n#    \n## Valid Title ##");
        assert_eq!(heading.as_deref(), Some("Valid Title"));
    }

    #[test]
    fn local_image_path_detection_handles_remote_and_absolute() {
        assert!(!is_local_image_path("https://example.com/a.png"));
        assert!(!is_local_image_path("http://example.com/a.png"));
        assert!(!is_local_image_path("data:image/png;base64,abc"));
        assert!(!is_local_image_path("/assets/a.png"));
        assert!(is_local_image_path("images/a.png"));
        assert!(is_local_image_path("../assets/a.png"));
    }

    #[test]
    fn parse_obsidian_embed_target_extracts_target_and_alias() {
        let (target, alias) = parse_obsidian_embed_target("assets/flow.svg|Execution Flow");
        assert_eq!(target, "assets/flow.svg");
        assert_eq!(alias.as_deref(), Some("Execution Flow"));
    }

    #[test]
    fn parse_obsidian_embed_target_strips_heading_fragment() {
        let (target, alias) = parse_obsidian_embed_target("assets/flow.svg#overview|320x240");
        assert_eq!(target, "assets/flow.svg");
        assert_eq!(alias.as_deref(), Some("320x240"));
    }

    #[test]
    fn obsidian_size_detection_handles_common_forms() {
        assert!(looks_like_obsidian_size("320"));
        assert!(looks_like_obsidian_size("320x240"));
        assert!(looks_like_obsidian_size("320X240"));
        assert!(!looks_like_obsidian_size("Execution Flow"));
    }

    #[test]
    fn resolve_local_asset_path_uses_media_root_fallback() {
        let unique = format!(
            "sf-cli-write-article-test-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        );
        let base = std::env::temp_dir().join(unique);
        let markdown_dir = base.join("notes");
        let media_root = base.join("learning");
        fs::create_dir_all(&markdown_dir).expect("create markdown dir");
        fs::create_dir_all(&media_root).expect("create media root");

        let markdown_path = markdown_dir.join("post.md");
        fs::write(&markdown_path, "# test").expect("write markdown");

        let raw_path = "_home_ts_user_claude_prompts_assets_diagram.svg";
        let media_file = media_root.join(raw_path);
        if let Some(parent) = media_file.parent() {
            fs::create_dir_all(parent).expect("create media file parent");
        }
        fs::write(&media_file, "svg").expect("write media file");

        let resolved = resolve_local_asset_path(raw_path, &markdown_path, &[media_root.clone()]);
        assert_eq!(resolved, Some(media_file));

        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn normalize_cli_date_accepts_valid_yyyy_mm_dd() {
        let date = normalize_cli_date(Some("2026-02-12".to_string())).expect("valid date");
        assert_eq!(date.as_deref(), Some("2026-02-12"));
    }

    #[test]
    fn normalize_cli_date_rejects_invalid_format() {
        let err = normalize_cli_date(Some("2026/02/12".to_string())).expect_err("invalid date");
        assert!(err.to_string().contains("expected YYYY-MM-DD"), "unexpected error: {err}");
    }
}
