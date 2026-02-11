use std::{fs, path::Path};

use anyhow::{Context, Result};
use static_flow_shared::{
    embedding::{
        detect_language, embed_text_with_language, TextEmbeddingLanguage, TEXT_VECTOR_DIM_EN,
        TEXT_VECTOR_DIM_ZH,
    },
    normalize_taxonomy_key,
};

use crate::{
    db::{
        connect_db, ensure_vector_index, optimize_table_indexes, upsert_articles, upsert_taxonomies,
    },
    schema::{ArticleRecord, TaxonomyRecord},
    utils::{estimate_read_time, parse_markdown, parse_tags, parse_vector, Frontmatter},
};

pub struct WriteArticleOptions {
    pub id: Option<String>,
    pub summary: Option<String>,
    pub tags: Option<String>,
    pub category: Option<String>,
    pub category_description: Option<String>,
    pub vector: Option<String>,
    pub vector_en: Option<String>,
    pub vector_zh: Option<String>,
    pub language: Option<String>,
    pub auto_optimize: bool,
}

pub async fn run(db_path: &Path, file: &Path, options: WriteArticleOptions) -> Result<()> {
    let WriteArticleOptions {
        id,
        summary,
        tags,
        category,
        category_description,
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

    let content = fs::read_to_string(file).context("failed to read markdown file")?;
    let (frontmatter, body) = parse_markdown(&content)?;
    let Frontmatter {
        title: frontmatter_title,
        summary: frontmatter_summary,
        tags: frontmatter_tags,
        category: frontmatter_category,
        category_description: frontmatter_category_description,
        author,
        date,
        featured_image,
        read_time,
    } = frontmatter;

    let title = resolve_title(file, &frontmatter_title, &body);

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
    let category = category
        .or(frontmatter_category)
        .filter(|value| !value.trim().is_empty())
        .context("category is required (pass --category or add category to frontmatter)")?;
    let category_description = category_description
        .or(frontmatter_category_description)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

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

    upsert_articles(&table, &[record]).await?;

    let mut taxonomies = Vec::new();
    push_taxonomy_record(
        &mut taxonomies,
        "category",
        &category,
        category_description.as_deref(),
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

    if auto_optimize {
        if let Err(err) = optimize_table_indexes(&table).await {
            tracing::warn!("Failed to optimize articles indexes after write-article: {err}");
        }
    }

    tracing::info!("Article written to LanceDB.");
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
        .map(str::to_string)
        .or_else(|| Some(name.trim().to_string()));
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{first_markdown_heading, resolve_title};

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
}
