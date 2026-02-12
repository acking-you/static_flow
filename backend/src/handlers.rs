use std::time::{Duration, Instant};

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{Json, Response},
};
use serde::{Deserialize, Serialize};
use static_flow_shared::{
    lancedb_api::{
        ArticleListResponse, CategoriesResponse, ImageListResponse, ImageSearchResponse,
        ImageTextSearchResponse, SearchResponse, StatsResponse, TagsResponse,
    },
    Article,
};

use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub q: String,
    #[serde(default)]
    pub enhanced_highlight: bool,
    #[serde(default)]
    pub hybrid: bool,
    #[serde(default)]
    pub hybrid_rrf_k: Option<f32>,
    #[serde(default)]
    pub hybrid_vector_limit: Option<usize>,
    #[serde(default)]
    pub hybrid_fts_limit: Option<usize>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub max_distance: Option<f32>,
}

#[derive(Debug, Deserialize)]
pub struct ImageSearchQuery {
    pub id: String,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub max_distance: Option<f32>,
}

#[derive(Debug, Deserialize)]
pub struct ImageTextSearchQuery {
    pub q: String,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub max_distance: Option<f32>,
}

#[derive(Debug, Deserialize)]
pub struct ImageRenderQuery {
    pub thumb: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct ArticleQuery {
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
    pub code: u16,
}

const CACHE_TTL: Duration = Duration::from_secs(60);

pub async fn list_articles(
    State(state): State<AppState>,
    Query(query): Query<ArticleQuery>,
) -> Result<Json<ArticleListResponse>, (StatusCode, Json<ErrorResponse>)> {
    let articles = state
        .store
        .list_articles(query.tag.as_deref(), query.category.as_deref())
        .await
        .map_err(|e| internal_error("Failed to fetch articles", e))?;

    Ok(Json(ArticleListResponse {
        total: articles.len(),
        articles,
    }))
}

pub async fn get_article(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Article>, (StatusCode, Json<ErrorResponse>)> {
    let article = state
        .store
        .get_article(&id)
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
    if let Some(tags) = read_cache(state.tags_cache.as_ref()).await {
        return Ok(Json(TagsResponse {
            tags,
        }));
    }

    let tags = state
        .store
        .list_tags()
        .await
        .map_err(|e| internal_error("Failed to fetch tags", e))?;

    write_cache(state.tags_cache.as_ref(), tags.clone()).await;
    Ok(Json(TagsResponse {
        tags,
    }))
}

pub async fn list_categories(
    State(state): State<AppState>,
) -> Result<Json<CategoriesResponse>, (StatusCode, Json<ErrorResponse>)> {
    if let Some(categories) = read_cache(state.categories_cache.as_ref()).await {
        return Ok(Json(CategoriesResponse {
            categories,
        }));
    }

    let categories = state
        .store
        .list_categories()
        .await
        .map_err(|e| internal_error("Failed to fetch categories", e))?;

    write_cache(state.categories_cache.as_ref(), categories.clone()).await;
    Ok(Json(CategoriesResponse {
        categories,
    }))
}

pub async fn get_stats(
    State(state): State<AppState>,
) -> Result<Json<StatsResponse>, (StatusCode, Json<ErrorResponse>)> {
    if let Some(stats) = read_cache(state.stats_cache.as_ref()).await {
        return Ok(Json(stats));
    }

    let stats = state
        .store
        .fetch_stats()
        .await
        .map_err(|e| internal_error("Failed to fetch stats", e))?;

    write_cache(state.stats_cache.as_ref(), stats.clone()).await;
    Ok(Json(stats))
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

    let results = state
        .store
        .search_articles(keyword, normalize_limit(query.limit))
        .await
        .map_err(|e| internal_error("Failed to search articles", e))?;

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

    let results = state
        .store
        .semantic_search(
            keyword,
            normalize_limit(query.limit),
            normalize_max_distance(query.max_distance),
            query.enhanced_highlight,
            query.hybrid,
            normalize_positive_f32(query.hybrid_rrf_k),
            normalize_limit(query.hybrid_vector_limit),
            normalize_limit(query.hybrid_fts_limit),
        )
        .await
        .map_err(|e| internal_error("Failed to run semantic search", e))?;

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
    let articles = state
        .store
        .related_articles(&id, 4)
        .await
        .map_err(|e| internal_error("Failed to fetch related articles", e))?;

    Ok(Json(ArticleListResponse {
        total: articles.len(),
        articles,
    }))
}

pub async fn list_images(
    State(state): State<AppState>,
) -> Result<Json<ImageListResponse>, (StatusCode, Json<ErrorResponse>)> {
    let images = state
        .store
        .list_images()
        .await
        .map_err(|e| internal_error("Failed to fetch images", e))?;

    Ok(Json(ImageListResponse {
        total: images.len(),
        images,
    }))
}

pub async fn search_images(
    State(state): State<AppState>,
    Query(query): Query<ImageSearchQuery>,
) -> Result<Json<ImageSearchResponse>, (StatusCode, Json<ErrorResponse>)> {
    let images = state
        .store
        .search_images(
            &query.id,
            normalize_limit(query.limit),
            normalize_max_distance(query.max_distance),
        )
        .await
        .map_err(|e| internal_error("Failed to search images", e))?;

    Ok(Json(ImageSearchResponse {
        total: images.len(),
        images,
        query_id: query.id,
    }))
}

pub async fn search_images_by_text(
    State(state): State<AppState>,
    Query(query): Query<ImageTextSearchQuery>,
) -> Result<Json<ImageTextSearchResponse>, (StatusCode, Json<ErrorResponse>)> {
    let keyword = query.q.trim();
    if keyword.is_empty() {
        return Ok(Json(ImageTextSearchResponse {
            total: 0,
            images: vec![],
            query: query.q,
        }));
    }

    let images = state
        .store
        .search_images_by_text(
            keyword,
            normalize_limit(query.limit),
            normalize_max_distance(query.max_distance),
        )
        .await
        .map_err(|e| internal_error("Failed to search images by text", e))?;

    Ok(Json(ImageTextSearchResponse {
        total: images.len(),
        images,
        query: query.q,
    }))
}

pub async fn serve_image(
    State(state): State<AppState>,
    Path(filename): Path<String>,
    Query(query): Query<ImageRenderQuery>,
) -> Result<Response, (StatusCode, Json<ErrorResponse>)> {
    let image = state
        .store
        .get_image(&filename, query.thumb.unwrap_or(false))
        .await
        .map_err(|e| internal_error("Failed to fetch image", e))?;

    let image = match image {
        Some(image) => image,
        None => {
            return Err((
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: "Image not found".to_string(),
                    code: 404,
                }),
            ));
        },
    };

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, image.mime_type)
        .header(header::CACHE_CONTROL, "public, max-age=31536000")
        .body(Body::from(image.bytes))
        .unwrap())
}

async fn read_cache<T: Clone>(cache: &tokio::sync::RwLock<Option<(T, Instant)>>) -> Option<T> {
    let cache = cache.read().await;
    match cache.as_ref() {
        Some((items, cached_at)) if cached_at.elapsed() < CACHE_TTL => Some(items.clone()),
        _ => None,
    }
}

async fn write_cache<T>(cache: &tokio::sync::RwLock<Option<(T, Instant)>>, items: T) {
    let mut cache = cache.write().await;
    *cache = Some((items, Instant::now()));
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

fn normalize_limit(limit: Option<usize>) -> Option<usize> {
    limit.filter(|value| *value > 0)
}

fn normalize_max_distance(max_distance: Option<f32>) -> Option<f32> {
    max_distance.filter(|value| value.is_finite() && *value >= 0.0)
}

fn normalize_positive_f32(value: Option<f32>) -> Option<f32> {
    value.filter(|item| item.is_finite() && *item > 0.0)
}
