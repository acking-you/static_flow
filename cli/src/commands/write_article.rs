use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use static_flow_shared::embedding::{
    detect_language, embed_text_with_language, TextEmbeddingLanguage, TEXT_VECTOR_DIM_EN,
    TEXT_VECTOR_DIM_ZH,
};

use crate::db::{connect_db, ensure_vector_index, upsert_articles};
use crate::schema::ArticleRecord;
use crate::utils::{estimate_read_time, parse_markdown, parse_tags, parse_vector};

pub async fn run(
    db_path: &Path,
    file: &Path,
    summary: String,
    tags: String,
    category: String,
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
    if frontmatter.title.trim().is_empty() {
        anyhow::bail!("frontmatter title is required");
    }

    let id = file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    let tags = parse_tags(&tags);
    let read_time = frontmatter.read_time.unwrap_or_else(|| estimate_read_time(&body));
    let date = frontmatter
        .date
        .unwrap_or_else(|| chrono::Utc::now().format("%Y-%m-%d").to_string());
    let author = frontmatter.author.unwrap_or_else(|| "Unknown".to_string());

    let combined_text = format!("{} {} {}", frontmatter.title, summary, body);
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
        title: frontmatter.title,
        content: body,
        summary,
        tags,
        category,
        author,
        date,
        featured_image: frontmatter.featured_image,
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
