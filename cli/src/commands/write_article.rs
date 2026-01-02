use std::{fs, path::Path};

use anyhow::{Context, Result};
use static_flow_shared::embedding::{
    detect_language, embed_text_with_language, TextEmbeddingLanguage, TEXT_VECTOR_DIM_EN,
    TEXT_VECTOR_DIM_ZH,
};

use crate::{
    db::{connect_db, ensure_vector_index, upsert_articles},
    schema::ArticleRecord,
    utils::{estimate_read_time, parse_markdown, parse_tags, parse_vector, Frontmatter},
};

pub async fn run(
    db_path: &Path,
    file: &Path,
    summary: Option<String>,
    tags: Option<String>,
    category: Option<String>,
    vector: Option<String>,
    vector_en: Option<String>,
    vector_zh: Option<String>,
    language: Option<String>,
) -> Result<()> {
    let db = connect_db(db_path).await?;
    let table = db
        .open_table("articles")
        .execute()
        .await
        .context("articles table not found; run `sf-cli init` first")?;

    let content = fs::read_to_string(file).context("failed to read markdown file")?;
    let (frontmatter, body) = parse_markdown(&content)?;
    let Frontmatter {
        title,
        summary: frontmatter_summary,
        tags: frontmatter_tags,
        category: frontmatter_category,
        author,
        date,
        featured_image,
        read_time,
    } = frontmatter;

    if title.trim().is_empty() {
        anyhow::bail!("frontmatter title is required");
    }

    let id = file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

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
    let category = category
        .or(frontmatter_category)
        .filter(|value| !value.trim().is_empty())
        .context("category is required (pass --category or add category to frontmatter)")?;
    let read_time = read_time.unwrap_or_else(|| estimate_read_time(&body));
    let date = date.unwrap_or_else(|| chrono::Utc::now().format("%Y-%m-%d").to_string());
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
    };

    upsert_articles(&table, &[record]).await?;

    if let Err(err) = ensure_vector_index(&table, "vector_en").await {
        tracing::warn!("Failed to create vector index on articles (vector_en): {err}");
    }
    if let Err(err) = ensure_vector_index(&table, "vector_zh").await {
        tracing::warn!("Failed to create vector index on articles (vector_zh): {err}");
    }

    tracing::info!("Article written to LanceDB.");
    Ok(())
}
