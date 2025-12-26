use std::collections::HashMap;
use anyhow::{Context, Result};
use arrow_array::{
    Array, ArrayRef, BinaryArray, FixedSizeListArray, Float32Array, ListArray, RecordBatch,
    StringArray,
};
use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{Json, Response},
};
use futures::TryStreamExt;
use lancedb::index::scalar::FullTextSearchQuery;
use lancedb::query::{ExecutableQuery, QueryBase, Select};
use lancedb::Table;
use serde::{Deserialize, Serialize};
use static_flow_shared::embedding::embed_text;
use static_flow_shared::{Article, ArticleListItem};

use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub q: String,
}

#[derive(Debug, Deserialize)]
pub struct ImageSearchQuery {
    pub id: String,
}

#[derive(Debug, Deserialize)]
pub struct ImageRenderQuery {
    pub thumb: Option<bool>,
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

#[derive(Debug, Serialize)]
pub struct ImageInfo {
    pub id: String,
    pub filename: String,
}

#[derive(Debug, Serialize)]
pub struct ImageListResponse {
    pub images: Vec<ImageInfo>,
    pub total: usize,
}

#[derive(Debug, Serialize)]
pub struct ImageSearchResponse {
    pub images: Vec<ImageInfo>,
    pub total: usize,
    pub query_id: String,
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
) -> Result<Json<ArticleListResponse>, (StatusCode, Json<ErrorResponse>)> {
    let table = state
        .articles_table()
        .await
        .map_err(|e| internal_error("Failed to open articles table", e))?;
    let mut articles = fetch_article_list(&table)
        .await
        .map_err(|e| internal_error("Failed to fetch articles", e))?;

    // Filter by tag and/or category
    articles = articles
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

    let total = articles.len();

    Ok(Json(ArticleListResponse {
        articles,
        total,
    }))
}

pub async fn get_article(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Article>, (StatusCode, Json<ErrorResponse>)> {
    let table = state
        .articles_table()
        .await
        .map_err(|e| internal_error("Failed to open articles table", e))?;

    let article = fetch_article_detail(&table, &id)
        .await
        .map_err(|e| internal_error("Failed to fetch article", e))?;

    match article {
        Some(article) => Ok(Json(article)),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Article not found".to_string(),
                code: 404,
            }),
        )),
    }
}

pub async fn list_tags(
    State(state): State<AppState>,
) -> Result<Json<TagsResponse>, (StatusCode, Json<ErrorResponse>)> {
    let table = state
        .articles_table()
        .await
        .map_err(|e| internal_error("Failed to open articles table", e))?;
    let articles = fetch_article_list(&table)
        .await
        .map_err(|e| internal_error("Failed to fetch tags", e))?;

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
        .map(|(name, count)| TagInfo { name, count })
        .collect();
    tags.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(Json(TagsResponse { tags }))
}

pub async fn list_categories(
    State(state): State<AppState>,
) -> Result<Json<CategoriesResponse>, (StatusCode, Json<ErrorResponse>)> {
    let table = state
        .articles_table()
        .await
        .map_err(|e| internal_error("Failed to open articles table", e))?;
    let articles = fetch_article_list(&table)
        .await
        .map_err(|e| internal_error("Failed to fetch categories", e))?;

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

    Ok(Json(CategoriesResponse { categories }))
}

pub async fn search_articles(
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<SearchResponse>, (StatusCode, Json<ErrorResponse>)> {
    let keyword = query.q.trim();
    if keyword.is_empty() {
        return Ok(Json(SearchResponse {
            results: vec![],
            total: 0,
            query: query.q,
        }));
    }

    let table = state
        .articles_table()
        .await
        .map_err(|e| internal_error("Failed to open articles table", e))?;

    let results = match search_with_fts(&table, keyword).await {
        Ok(results) => results,
        Err(err) => {
            tracing::warn!("FTS search failed, falling back to scan: {}", err);
            fallback_search(&table, keyword)
                .await
                .map_err(|e| internal_error("Failed to search articles", e))?
        },
    };

    Ok(Json(SearchResponse {
        total: results.len(),
        results,
        query: query.q,
    }))
}

pub async fn semantic_search(
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<SearchResponse>, (StatusCode, Json<ErrorResponse>)> {
    let keyword = query.q.trim();
    if keyword.is_empty() {
        return Ok(Json(SearchResponse {
            results: vec![],
            total: 0,
            query: query.q,
        }));
    }

    let table = state
        .articles_table()
        .await
        .map_err(|e| internal_error("Failed to open articles table", e))?;

    let embedding = embed_text(keyword);
    let vector_query = table
        .query()
        .nearest_to(embedding.as_slice())
        .map_err(|e| internal_error("Failed to build semantic query", e))?;
    let batches = vector_query
        .limit(10)
        .select(Select::columns(&[
            "id",
            "title",
            "summary",
            "tags",
            "category",
            "author",
            "date",
            "featured_image",
            "read_time",
        ]))
        .execute()
        .await
        .map_err(|e| internal_error("Failed to run semantic search", e))?;

    let batch_list = batches
        .try_collect::<Vec<_>>()
        .await
        .map_err(|e| internal_error("Failed to read semantic batches", e))?;
    let articles = batches_to_article_list(&batch_list)
        .map_err(|e| internal_error("Failed to parse semantic results", e))?;

    let results = articles
        .into_iter()
        .map(|article| {
            let summary = article.summary.clone();
            SearchResult {
                id: article.id,
                title: article.title,
                summary,
                category: article.category,
                date: article.date,
                highlight: article.summary,
                tags: article.tags,
            }
        })
        .collect::<Vec<_>>();

    Ok(Json(SearchResponse {
        total: results.len(),
        results,
        query: query.q,
    }))
}

pub async fn related_articles(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ArticleListResponse>, (StatusCode, Json<ErrorResponse>)> {
    let table = state
        .articles_table()
        .await
        .map_err(|e| internal_error("Failed to open articles table", e))?;

    let vector = fetch_article_vector(&table, &id)
        .await
        .map_err(|e| internal_error("Failed to fetch article vector", e))?;

    let vector = match vector {
        Some(vector) => vector,
        None => {
            return Ok(Json(ArticleListResponse {
                articles: vec![],
                total: 0,
            }));
        },
    };

    let filter = format!("id != '{}'", escape_literal(&id));
    let vector_query = table
        .query()
        .nearest_to(vector.as_slice())
        .map_err(|e| internal_error("Failed to build related query", e))?;
    let batches = vector_query
        .only_if(filter)
        .limit(4)
        .select(Select::columns(&[
            "id",
            "title",
            "summary",
            "tags",
            "category",
            "author",
            "date",
            "featured_image",
            "read_time",
        ]))
        .execute()
        .await
        .map_err(|e| internal_error("Failed to fetch related articles", e))?;

    let batch_list = batches
        .try_collect::<Vec<_>>()
        .await
        .map_err(|e| internal_error("Failed to read related batches", e))?;
    let articles = batches_to_article_list(&batch_list)
        .map_err(|e| internal_error("Failed to parse related articles", e))?;

    Ok(Json(ArticleListResponse {
        total: articles.len(),
        articles,
    }))
}

pub async fn list_images(
    State(state): State<AppState>,
) -> Result<Json<ImageListResponse>, (StatusCode, Json<ErrorResponse>)> {
    let table = state
        .images_table()
        .await
        .map_err(|e| internal_error("Failed to open images table", e))?;

    let batches = table
        .query()
        .select(Select::columns(&["id", "filename"]))
        .execute()
        .await
        .map_err(|e| internal_error("Failed to fetch images", e))?;

    let batch_list = batches
        .try_collect::<Vec<_>>()
        .await
        .map_err(|e| internal_error("Failed to read image batches", e))?;
    let images = batches_to_images(&batch_list)
        .map_err(|e| internal_error("Failed to parse images", e))?;

    Ok(Json(ImageListResponse {
        total: images.len(),
        images,
    }))
}

pub async fn search_images(
    State(state): State<AppState>,
    Query(query): Query<ImageSearchQuery>,
) -> Result<Json<ImageSearchResponse>, (StatusCode, Json<ErrorResponse>)> {
    let table = state
        .images_table()
        .await
        .map_err(|e| internal_error("Failed to open images table", e))?;

    let vector = fetch_image_vector(&table, &query.id)
        .await
        .map_err(|e| internal_error("Failed to fetch image vector", e))?;

    let vector = match vector {
        Some(vector) => vector,
        None => {
            return Ok(Json(ImageSearchResponse {
                images: vec![],
                total: 0,
                query_id: query.id,
            }));
        },
    };

    let filter = format!("id != '{}'", escape_literal(&query.id));
    let vector_query = table
        .query()
        .nearest_to(vector.as_slice())
        .map_err(|e| internal_error("Failed to build image search query", e))?;
    let batches = vector_query
        .only_if(filter)
        .limit(12)
        .select(Select::columns(&["id", "filename"]))
        .execute()
        .await
        .map_err(|e| internal_error("Failed to search images", e))?;

    let batch_list = batches
        .try_collect::<Vec<_>>()
        .await
        .map_err(|e| internal_error("Failed to read image search batches", e))?;
    let images = batches_to_images(&batch_list)
        .map_err(|e| internal_error("Failed to parse image search results", e))?;

    Ok(Json(ImageSearchResponse {
        total: images.len(),
        images,
        query_id: query.id,
    }))
}

/// Serve image files stored in LanceDB.
pub async fn serve_image(
    State(state): State<AppState>,
    Path(filename): Path<String>,
    Query(query): Query<ImageRenderQuery>,
) -> Result<Response, (StatusCode, Json<ErrorResponse>)> {
    let table = state
        .images_table()
        .await
        .map_err(|e| internal_error("Failed to open images table", e))?;

    let escaped = escape_literal(&filename);
    let filter = format!("filename = '{}' OR id = '{}'", escaped, escaped);
    let batches = table
        .query()
        .only_if(filter)
        .limit(1)
        .select(Select::columns(&["data", "thumbnail", "filename"]))
        .execute()
        .await
        .map_err(|e| internal_error("Failed to fetch image", e))?;

    let batch_list = batches
        .try_collect::<Vec<_>>()
        .await
        .map_err(|e| internal_error("Failed to parse image batch", e))?;

    let (bytes, name) = match extract_image_bytes(&batch_list, query.thumb.unwrap_or(false)) {
        Ok(Some(result)) => result,
        Ok(None) => {
            return Err((
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: "Image not found".to_string(),
                    code: 404,
                }),
            ));
        },
        Err(e) => return Err(internal_error("Failed to decode image", e)),
    };

    let mime_type = match name.split('.').last() {
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
        .header(header::CACHE_CONTROL, "public, max-age=31536000")
        .body(Body::from(bytes))
        .unwrap())
}

async fn fetch_article_list(table: &Table) -> Result<Vec<ArticleListItem>> {
    let batches = table
        .query()
        .select(Select::columns(&[
            "id",
            "title",
            "summary",
            "tags",
            "category",
            "author",
            "date",
            "featured_image",
            "read_time",
        ]))
        .execute()
        .await?;

    let batch_list = batches.try_collect::<Vec<_>>().await?;
    let mut articles = batches_to_article_list(&batch_list)?;
    // Sort by date descending.
    articles.sort_by(|a, b| b.date.cmp(&a.date));
    Ok(articles)
}

async fn fetch_article_detail(table: &Table, id: &str) -> Result<Option<Article>> {
    let filter = format!("id = '{}'", escape_literal(id));
    let batches = table
        .query()
        .only_if(filter)
        .limit(1)
        .select(Select::columns(&[
            "id",
            "title",
            "summary",
            "content",
            "tags",
            "category",
            "author",
            "date",
            "featured_image",
            "read_time",
        ]))
        .execute()
        .await?;

    let batch_list = batches.try_collect::<Vec<_>>().await?;
    Ok(batches_to_article_detail(&batch_list)?)
}

async fn fetch_article_vector(table: &Table, id: &str) -> Result<Option<Vec<f32>>> {
    let filter = format!("id = '{}'", escape_literal(id));
    let batches = table
        .query()
        .only_if(filter)
        .limit(1)
        .select(Select::columns(&["vector"]))
        .execute()
        .await?;

    let batch_list = batches.try_collect::<Vec<_>>().await?;
    Ok(extract_vector(&batch_list))
}

async fn fetch_image_vector(table: &Table, id: &str) -> Result<Option<Vec<f32>>> {
    let filter = format!("id = '{}'", escape_literal(id));
    let batches = table
        .query()
        .only_if(filter)
        .limit(1)
        .select(Select::columns(&["vector"]))
        .execute()
        .await?;

    let batch_list = batches.try_collect::<Vec<_>>().await?;
    Ok(extract_vector(&batch_list))
}

async fn search_with_fts(table: &Table, keyword: &str) -> Result<Vec<SearchResult>> {
    let batches = table
        .query()
        .full_text_search(FullTextSearchQuery::new(keyword.to_string()))
        .limit(10)
        .select(Select::columns(&[
            "id",
            "title",
            "summary",
            "content",
            "tags",
            "category",
            "date",
        ]))
        .execute()
        .await?;

    let batch_list = batches.try_collect::<Vec<_>>().await?;
    let articles = batches_to_articles(&batch_list)?;

    Ok(articles
        .into_iter()
        .map(|article| SearchResult {
            highlight: extract_highlight(&article.content, keyword),
            id: article.id,
            title: article.title,
            summary: article.summary,
            category: article.category,
            date: article.date,
            tags: article.tags,
        })
        .collect())
}

async fn fallback_search(table: &Table, keyword: &str) -> Result<Vec<SearchResult>> {
    let batches = table
        .query()
        .select(Select::columns(&[
            "id",
            "title",
            "summary",
            "content",
            "tags",
            "category",
            "date",
        ]))
        .execute()
        .await?;

    let batch_list = batches.try_collect::<Vec<_>>().await?;
    let articles = batches_to_articles(&batch_list)?;

    let keyword_lower = keyword.to_lowercase();
    let mut results = Vec::new();
    for article in articles {
        let mut score = 0;
        if article.title.to_lowercase().contains(&keyword_lower) {
            score += 10;
        }
        if article.summary.to_lowercase().contains(&keyword_lower) {
            score += 5;
        }
        if article.content.to_lowercase().contains(&keyword_lower) {
            score += 1;
        }
        for tag in &article.tags {
            if tag.to_lowercase().contains(&keyword_lower) {
                score += 3;
            }
        }

        if score > 0 {
            results.push((
                SearchResult {
                    highlight: extract_highlight(&article.content, &keyword_lower),
                    id: article.id,
                    title: article.title,
                    summary: article.summary,
                    category: article.category,
                    date: article.date,
                    tags: article.tags,
                },
                score,
            ));
        }
    }

    results.sort_by(|a, b| b.1.cmp(&a.1));
    Ok(results.into_iter().map(|(r, _)| r).collect())
}

fn batches_to_article_list(batches: &[RecordBatch]) -> Result<Vec<ArticleListItem>> {
    let mut articles = Vec::new();
    for batch in batches {
        let id = string_array(batch, "id")?;
        let title = string_array(batch, "title")?;
        let summary = string_array(batch, "summary")?;
        let tags = list_array(batch, "tags")?;
        let category = string_array(batch, "category")?;
        let author = string_array(batch, "author")?;
        let date = string_array(batch, "date")?;
        let featured = string_array(batch, "featured_image")?;
        let read_time = int32_array(batch, "read_time")?;

        for row in 0..batch.num_rows() {
            articles.push(ArticleListItem {
                id: value_string(id, row),
                title: value_string(title, row),
                summary: value_string(summary, row),
                tags: value_string_list(tags, row),
                category: value_string(category, row),
                author: value_string(author, row),
                date: value_string(date, row),
                featured_image: value_string_opt(featured, row),
                read_time: read_time.value(row) as u32,
            });
        }
    }
    Ok(articles)
}

fn batches_to_articles(batches: &[RecordBatch]) -> Result<Vec<Article>> {
    let mut articles = Vec::new();
    for batch in batches {
        let id = string_array(batch, "id")?;
        let title = string_array(batch, "title")?;
        let summary = string_array(batch, "summary")?;
        let content = string_array(batch, "content")?;
        let tags = list_array(batch, "tags")?;
        let category = string_array(batch, "category")?;
        let author = string_array(batch, "author")?;
        let date = string_array(batch, "date")?;
        let featured = string_array(batch, "featured_image")?;
        let read_time = int32_array(batch, "read_time")?;

        for row in 0..batch.num_rows() {
            articles.push(Article {
                id: value_string(id, row),
                title: value_string(title, row),
                summary: value_string(summary, row),
                content: value_string(content, row),
                tags: value_string_list(tags, row),
                category: value_string(category, row),
                author: value_string(author, row),
                date: value_string(date, row),
                featured_image: value_string_opt(featured, row),
                read_time: read_time.value(row) as u32,
            });
        }
    }
    Ok(articles)
}

fn batches_to_article_detail(batches: &[RecordBatch]) -> Result<Option<Article>> {
    let articles = batches_to_articles(batches)?;
    Ok(articles.into_iter().next())
}

fn batches_to_images(batches: &[RecordBatch]) -> Result<Vec<ImageInfo>> {
    let mut images = Vec::new();
    for batch in batches {
        let id = string_array(batch, "id")?;
        let filename = string_array(batch, "filename")?;

        for row in 0..batch.num_rows() {
            images.push(ImageInfo {
                id: value_string(id, row),
                filename: value_string(filename, row),
            });
        }
    }
    Ok(images)
}

fn extract_vector(batches: &[RecordBatch]) -> Option<Vec<f32>> {
    for batch in batches {
        if batch.num_rows() == 0 {
            continue;
        }
        let vector_array = batch
            .schema()
            .index_of("vector")
            .ok()
            .and_then(|idx| batch.column(idx).as_any().downcast_ref::<FixedSizeListArray>())?;
        return Some(value_vector(vector_array, 0));
    }
    None
}

fn extract_image_bytes(
    batches: &[RecordBatch],
    prefer_thumbnail: bool,
) -> Result<Option<(Vec<u8>, String)>> {
    for batch in batches {
        if batch.num_rows() == 0 {
            continue;
        }
        let data = binary_array(batch, "data")?;
        let thumb = binary_array(batch, "thumbnail")?;
        let filename = string_array(batch, "filename")?;
        let name = value_string(filename, 0);

        if prefer_thumbnail && !thumb.is_null(0) {
            return Ok(Some((thumb.value(0).to_vec(), name)));
        }
        return Ok(Some((data.value(0).to_vec(), name)));
    }
    Ok(None)
}

fn string_array<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a StringArray> {
    column(batch, name)?.as_any().downcast_ref::<StringArray>().with_context(|| {
        format!("column {} is not StringArray", name)
    })
}

fn list_array<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a ListArray> {
    column(batch, name)?.as_any().downcast_ref::<ListArray>().with_context(|| {
        format!("column {} is not ListArray", name)
    })
}

fn int32_array<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a arrow_array::Int32Array> {
    column(batch, name)?
        .as_any()
        .downcast_ref::<arrow_array::Int32Array>()
        .with_context(|| format!("column {} is not Int32Array", name))
}

fn binary_array<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a BinaryArray> {
    column(batch, name)?.as_any().downcast_ref::<BinaryArray>().with_context(|| {
        format!("column {} is not BinaryArray", name)
    })
}

fn column<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a ArrayRef> {
    let idx = batch
        .schema()
        .index_of(name)
        .with_context(|| format!("missing column {}", name))?;
    Ok(batch.column(idx))
}

fn value_string(array: &StringArray, row: usize) -> String {
    array.value(row).to_string()
}

fn value_string_opt(array: &StringArray, row: usize) -> Option<String> {
    if array.is_null(row) {
        None
    } else {
        Some(array.value(row).to_string())
    }
}

fn value_string_list(array: &ListArray, row: usize) -> Vec<String> {
    if array.is_null(row) {
        return vec![];
    }
    let value = array.value(row);
    let value = value
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap_or_else(|| panic!("tags list is not StringArray"));
    (0..value.len())
        .map(|idx| value.value(idx).to_string())
        .collect()
}

fn value_vector(array: &FixedSizeListArray, row: usize) -> Vec<f32> {
    let values = array.values();
    let values = values
        .as_any()
        .downcast_ref::<Float32Array>()
        .unwrap_or_else(|| panic!("vector values are not Float32Array"));
    let dim = array.value_length() as usize;
    let start = row * dim;
    let mut vector = Vec::with_capacity(dim);
    for idx in 0..dim {
        vector.push(values.value(start + idx));
    }
    vector
}

fn escape_literal(input: &str) -> String {
    input.replace('\'', "''")
}

fn internal_error(message: &str, err: impl std::fmt::Display) -> (StatusCode, Json<ErrorResponse>) {
    tracing::error!("{}: {}", message, err);
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorResponse {
            error: message.to_string(),
            code: 500,
        }),
    )
}

/// Extract a snippet around the keyword with highlighting.
fn extract_highlight(text: &str, keyword: &str) -> String {
    let text_lower = text.to_lowercase();
    let keyword_lower = keyword.to_lowercase();

    if let Some(pos) = text_lower.find(&keyword_lower) {
        let start = pos.saturating_sub(40);
        let end = (pos + keyword.len() + 40).min(text.len());

        let mut snippet: String = text.chars().skip(start).take(end - start).collect();

        if start > 0 {
            snippet.insert_str(0, "...");
        }
        if end < text.len() {
            snippet.push_str("...");
        }

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
