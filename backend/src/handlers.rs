use std::collections::HashMap;

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Json, Response},
};
use serde::{Deserialize, Serialize};
use static_flow_shared::{Article, ArticleListItem};
use tokio::fs;

use crate::{markdown, state::AppState};

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub q: String,
}

#[derive(Debug, Serialize)]
pub struct SearchResult {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub category: String,
    pub date: String,
    pub highlight: String,
    pub tags: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub results: Vec<SearchResult>,
    pub total: usize,
    pub query: String,
}

#[derive(Debug, Deserialize)]
pub struct ArticleQuery {
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ArticleListResponse {
    pub articles: Vec<ArticleListItem>,
    pub total: usize,
}

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
    pub code: u16,
}

#[derive(Debug, Serialize)]
pub struct TagInfo {
    pub name: String,
    pub count: usize,
}

#[derive(Debug, Serialize)]
pub struct CategoryInfo {
    pub name: String,
    pub count: usize,
    pub description: String,
}

#[derive(Debug, Serialize)]
pub struct TagsResponse {
    pub tags: Vec<TagInfo>,
}

#[derive(Debug, Serialize)]
pub struct CategoriesResponse {
    pub categories: Vec<CategoryInfo>,
}

// Category descriptions (matching frontend)
const CATEGORY_DESCRIPTIONS: &[(&str, &str)] = &[
    ("Rust", "静态类型、零成本抽象与 Wasm 生态的实战笔记。"),
    ("Web", "现代前端工程化与体验设计相关内容。"),
    ("DevOps", "自动化、流水线与交付体验的工程思考。"),
    ("Productivity", "效率、写作与自我管理的小实验与道具。"),
    ("AI", "Prompt、LLM 与智能体的落地探索。"),
];

pub async fn list_articles(
    State(state): State<AppState>,
    Query(query): Query<ArticleQuery>,
) -> Json<ArticleListResponse> {
    let articles = state.get_articles().await;

    // Filter by tag and/or category
    let filtered: Vec<ArticleListItem> = articles
        .into_iter()
        .filter(|article| {
            let mut matches = true;

            // Filter by tag (case insensitive)
            if let Some(ref tag) = query.tag {
                let tag_lower = tag.to_lowercase();
                matches = matches && article.tags.iter().any(|t| t.to_lowercase() == tag_lower);
            }

            // Filter by category (case insensitive)
            if let Some(ref category) = query.category {
                matches = matches && article.category.eq_ignore_ascii_case(category);
            }

            matches
        })
        .collect();

    let total = filtered.len();

    Json(ArticleListResponse {
        articles: filtered,
        total,
    })
}

pub async fn get_article(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Article>, (StatusCode, Json<ErrorResponse>)> {
    // Get file path for article ID
    let file_path = state.get_article_path(&id).await.ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Article not found".to_string(),
                code: 404,
            }),
        )
    })?;

    // Load article detail
    let article = markdown::load_article_detail(&file_path)
        .await
        .map_err(|e| {
            tracing::error!("Failed to load article {}: {}", id, e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Failed to load article".to_string(),
                    code: 500,
                }),
            )
        })?;

    Ok(Json(article))
}

pub async fn list_tags(State(state): State<AppState>) -> Json<TagsResponse> {
    let articles = state.get_articles().await;

    // Aggregate tag counts
    let mut tag_counts: HashMap<String, usize> = HashMap::new();
    for article in articles {
        for tag in article.tags {
            *tag_counts.entry(tag).or_insert(0) += 1;
        }
    }

    // Sort by name
    let mut tags: Vec<TagInfo> = tag_counts
        .into_iter()
        .map(|(name, count)| TagInfo {
            name,
            count,
        })
        .collect();
    tags.sort_by(|a, b| a.name.cmp(&b.name));

    Json(TagsResponse {
        tags,
    })
}

pub async fn list_categories(State(state): State<AppState>) -> Json<CategoriesResponse> {
    let articles = state.get_articles().await;

    // Aggregate category counts
    let mut category_counts: HashMap<String, usize> = HashMap::new();
    for article in articles {
        *category_counts.entry(article.category).or_insert(0) += 1;
    }

    // Build category info with descriptions
    let description_map: HashMap<&str, &str> = CATEGORY_DESCRIPTIONS.iter().copied().collect();

    let mut categories: Vec<CategoryInfo> = category_counts
        .into_iter()
        .map(|(name, count)| {
            let description = description_map
                .get(name.as_str())
                .unwrap_or(&"")
                .to_string();
            CategoryInfo {
                name,
                count,
                description,
            }
        })
        .collect();

    // Sort by name
    categories.sort_by(|a, b| a.name.cmp(&b.name));

    Json(CategoriesResponse {
        categories,
    })
}

pub async fn search_articles(
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
) -> Json<SearchResponse> {
    let keyword = query.q.trim().to_lowercase();

    if keyword.is_empty() {
        return Json(SearchResponse {
            results: vec![],
            total: 0,
            query: query.q,
        });
    }

    // Load all articles with content
    let article_list = state.get_articles().await;
    let mut scored_results: Vec<(SearchResult, usize)> = Vec::new();

    for article_item in article_list {
        // Load full article content
        if let Some(file_path) = state.get_article_path(&article_item.id).await {
            if let Ok(article) = markdown::load_article_detail(&file_path).await {
                let mut score = 0;
                let mut matched_snippets = Vec::new();

                // Search in title (highest priority, score = 10)
                if article.title.to_lowercase().contains(&keyword) {
                    score += 10;
                }

                // Search in summary (score = 5)
                if article.summary.to_lowercase().contains(&keyword) {
                    score += 5;
                    matched_snippets.push(extract_highlight(&article.summary, &keyword));
                }

                // Search in content (score = 1)
                if article.content.to_lowercase().contains(&keyword) {
                    score += 1;
                    matched_snippets.push(extract_highlight(&article.content, &keyword));
                }

                // Search in tags (score = 3)
                for tag in &article.tags {
                    if tag.to_lowercase().contains(&keyword) {
                        score += 3;
                    }
                }

                if score > 0 {
                    // 优先显示内容匹配的高亮片段，其次是摘要，最后是标题
                    let highlight = if matched_snippets.len() > 0 {
                        matched_snippets.last().cloned().unwrap()
                    } else {
                        extract_highlight(&article.title, &keyword)
                    };

                    scored_results.push((
                        SearchResult {
                            id: article.id,
                            title: article.title,
                            summary: article.summary,
                            category: article.category,
                            date: article.date,
                            highlight,
                            tags: article.tags,
                        },
                        score,
                    ));
                }
            }
        }
    }

    // Sort by score descending
    scored_results.sort_by(|a, b| b.1.cmp(&a.1));

    // Take top 10
    let results: Vec<SearchResult> = scored_results
        .into_iter()
        .take(10)
        .map(|(result, _)| result)
        .collect();

    let total = results.len();

    Json(SearchResponse {
        results,
        total,
        query: query.q,
    })
}

/// Extract a snippet around the keyword with highlighting
fn extract_highlight(text: &str, keyword: &str) -> String {
    let text_lower = text.to_lowercase();
    let keyword_lower = keyword.to_lowercase();

    if let Some(pos) = text_lower.find(&keyword_lower) {
        let start = pos.saturating_sub(40);
        let end = (pos + keyword.len() + 40).min(text.len());

        let mut snippet: String = text.chars().skip(start).take(end - start).collect();

        // Add ellipsis
        if start > 0 {
            snippet.insert_str(0, "...");
        }
        if end < text.len() {
            snippet.push_str("...");
        }

        // Highlight the keyword (simple marker, frontend will style it)
        let snippet_lower = snippet.to_lowercase();
        if let Some(keyword_pos) = snippet_lower.find(&keyword_lower) {
            let before = &snippet[..keyword_pos];
            let matched = &snippet[keyword_pos..keyword_pos + keyword.len()];
            let after = &snippet[keyword_pos + keyword.len()..];
            return format!("{}<mark>{}</mark>{}", before, matched, after);
        }

        snippet
    } else {
        text.chars().take(100).collect::<String>() + "..."
    }
}

/// Serve image files
pub async fn serve_image(
    State(state): State<AppState>,
    Path(filename): Path<String>,
) -> Result<Response, (StatusCode, Json<ErrorResponse>)> {
    // Sanitize filename to prevent directory traversal
    let filename = filename.replace("..", "").replace("/", "");

    let image_path = format!("{}/{}", state.images_dir(), filename);

    // Check if file exists
    if !tokio::fs::try_exists(&image_path).await.unwrap_or(false) {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Image not found".to_string(),
                code: 404,
            }),
        ));
    }

    // Read file
    let file_bytes = fs::read(&image_path).await.map_err(|e| {
        tracing::error!("Failed to read image {}: {}", image_path, e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "Failed to read image".to_string(),
                code: 500,
            }),
        )
    })?;

    // Determine MIME type from file extension
    let mime_type = match image_path.split('.').last() {
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        _ => "application/octet-stream",
    };

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime_type)
        .header(header::CACHE_CONTROL, "public, max-age=31536000") // Cache for 1 year
        .body(Body::from(file_bytes))
        .unwrap())
}
