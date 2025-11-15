use anyhow::{Context, Result};
use gray_matter::engine::YAML;
use gray_matter::Matter;
use serde::{Deserialize, Serialize};
use static_flow_shared::{Article, ArticleListItem};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::fs;

#[derive(Debug, Deserialize, Serialize)]
struct Frontmatter {
    pub title: String,
    pub summary: String,
    pub tags: Vec<String>,
    pub category: String,
    pub author: String,
    pub date: String,
    #[serde(default)]
    pub featured_image: Option<String>,
    pub read_time: u32,
}

/// Scan content directory and return list of articles with ID to path mapping
pub async fn scan_articles(
    content_dir: &str,
) -> Result<(Vec<ArticleListItem>, HashMap<String, String>)> {
    let path = Path::new(content_dir);

    if !path.exists() {
        anyhow::bail!("Content directory does not exist: {}", content_dir);
    }

    let mut articles = Vec::new();
    let mut id_to_path = HashMap::new();

    let mut entries = fs::read_dir(path).await?;

    while let Some(entry) = entries.next_entry().await? {
        let file_path = entry.path();

        // Only process .md files
        if file_path.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }

        // Extract ID from filename (e.g., "post-001.md" -> "post-001")
        let id = file_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();

        match parse_article_metadata(&file_path, &id).await {
            Ok(article) => {
                articles.push(article);
                id_to_path.insert(id, file_path.to_string_lossy().to_string());
            }
            Err(e) => {
                tracing::warn!("Failed to parse {}: {}", file_path.display(), e);
            }
        }
    }

    // Sort by date descending (newest first)
    articles.sort_by(|a, b| b.date.cmp(&a.date));

    Ok((articles, id_to_path))
}

/// Parse article metadata (frontmatter) without loading full content
async fn parse_article_metadata(file_path: &PathBuf, id: &str) -> Result<ArticleListItem> {
    let content = fs::read_to_string(file_path)
        .await
        .context("Failed to read file")?;

    let matter = Matter::<YAML>::new();
    let parsed = matter.parse(&content);

    let frontmatter: Frontmatter = parsed
        .data
        .ok_or_else(|| anyhow::anyhow!("No frontmatter found"))?
        .deserialize()
        .context("Failed to deserialize frontmatter")?;

    Ok(ArticleListItem {
        id: id.to_string(),
        title: frontmatter.title,
        summary: frontmatter.summary,
        tags: frontmatter.tags,
        category: frontmatter.category,
        author: frontmatter.author,
        date: frontmatter.date,
        featured_image: frontmatter.featured_image,
        read_time: frontmatter.read_time,
    })
}

/// Load full article content including Markdown body
pub async fn load_article_detail(file_path: &str) -> Result<Article> {
    let content = fs::read_to_string(file_path)
        .await
        .context("Failed to read file")?;

    let matter = Matter::<YAML>::new();
    let parsed = matter.parse(&content);

    let frontmatter: Frontmatter = parsed
        .data
        .ok_or_else(|| anyhow::anyhow!("No frontmatter found"))?
        .deserialize()
        .context("Failed to deserialize frontmatter")?;

    // Extract ID from file path
    let id = Path::new(file_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    Ok(Article {
        id,
        title: frontmatter.title,
        summary: frontmatter.summary,
        content: parsed.content,
        tags: frontmatter.tags,
        category: frontmatter.category,
        author: frontmatter.author,
        date: frontmatter.date,
        featured_image: frontmatter.featured_image,
        read_time: frontmatter.read_time,
    })
}
