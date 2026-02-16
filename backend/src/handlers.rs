use std::{
    net::IpAddr,
    time::{Duration, Instant},
};

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{Json, Response},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use static_flow_shared::{
    lancedb_api::{
        ArticleListResponse, ArticleViewTrackResponse, ArticleViewTrendResponse,
        CategoriesResponse, ImageListResponse, ImageSearchResponse, ImageTextSearchResponse,
        SearchResponse, StatsResponse, TagsResponse,
    },
    Article,
};

use crate::state::{
    AppState, ViewAnalyticsRuntimeConfig, MAX_CONFIGURABLE_VIEW_DEDUPE_WINDOW_SECONDS,
    MAX_CONFIGURABLE_VIEW_TREND_DAYS,
};

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

#[derive(Debug, Deserialize)]
pub struct ViewTrendQuery {
    #[serde(default)]
    pub granularity: Option<String>,
    #[serde(default)]
    pub days: Option<usize>,
    #[serde(default)]
    pub day: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
    pub code: u16,
}

#[derive(Debug, Serialize)]
pub struct ViewAnalyticsConfigResponse {
    pub dedupe_window_seconds: u64,
    pub trend_default_days: usize,
    pub trend_max_days: usize,
}

impl From<ViewAnalyticsRuntimeConfig> for ViewAnalyticsConfigResponse {
    fn from(value: ViewAnalyticsRuntimeConfig) -> Self {
        Self {
            dedupe_window_seconds: value.dedupe_window_seconds,
            trend_default_days: value.trend_default_days,
            trend_max_days: value.trend_max_days,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct UpdateViewAnalyticsConfigRequest {
    #[serde(default)]
    pub dedupe_window_seconds: Option<u64>,
    #[serde(default)]
    pub trend_default_days: Option<usize>,
    #[serde(default)]
    pub trend_max_days: Option<usize>,
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

pub async fn track_article_view(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<ArticleViewTrackResponse>, (StatusCode, Json<ErrorResponse>)> {
    ensure_article_exists(&state, &id).await?;

    let config = state.view_analytics_config.read().await.clone();
    let fingerprint = build_client_fingerprint(&headers);
    let tracked = state
        .store
        .track_article_view(
            &id,
            &fingerprint,
            config.trend_default_days,
            config.dedupe_window_seconds,
            config.trend_max_days,
        )
        .await
        .map_err(|e| internal_error("Failed to track article view", e))?;

    Ok(Json(tracked))
}

pub async fn get_article_view_trend(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<ViewTrendQuery>,
) -> Result<Json<ArticleViewTrendResponse>, (StatusCode, Json<ErrorResponse>)> {
    ensure_article_exists(&state, &id).await?;
    let config = state.view_analytics_config.read().await.clone();

    let granularity = query
        .granularity
        .as_deref()
        .unwrap_or("day")
        .trim()
        .to_ascii_lowercase();

    match granularity.as_str() {
        "day" => {
            let response = state
                .store
                .fetch_article_view_trend_day(
                    &id,
                    query.days.unwrap_or(config.trend_default_days),
                    config.trend_max_days,
                )
                .await
                .map_err(|e| internal_error("Failed to fetch article view trend", e))?;
            Ok(Json(response))
        },
        "hour" => {
            let day = query.day.as_deref().map(str::trim).unwrap_or_default();
            if day.is_empty() {
                return Err(bad_request("`day` is required for hour granularity"));
            }
            if !is_valid_day_format(day) {
                return Err(bad_request("`day` must use YYYY-MM-DD format"));
            }

            let response = state
                .store
                .fetch_article_view_trend_hour(&id, day)
                .await
                .map_err(|e| internal_error("Failed to fetch article view trend", e))?;
            Ok(Json(response))
        },
        _ => Err(bad_request("`granularity` must be `day` or `hour`")),
    }
}

pub async fn get_view_analytics_config(
    State(state): State<AppState>,
) -> Json<ViewAnalyticsConfigResponse> {
    let config = state.view_analytics_config.read().await.clone();
    Json(config.into())
}

pub async fn update_view_analytics_config(
    State(state): State<AppState>,
    Json(request): Json<UpdateViewAnalyticsConfigRequest>,
) -> Result<Json<ViewAnalyticsConfigResponse>, (StatusCode, Json<ErrorResponse>)> {
    let current = state.view_analytics_config.read().await.clone();
    let next = apply_view_analytics_config_update(current, request)?;
    {
        let mut writer = state.view_analytics_config.write().await;
        *writer = next.clone();
    }
    Ok(Json(next.into()))
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

async fn ensure_article_exists(
    state: &AppState,
    id: &str,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    let article = state
        .store
        .get_article(id)
        .await
        .map_err(|e| internal_error("Failed to fetch article", e))?;
    if article.is_some() {
        Ok(())
    } else {
        Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Article not found".to_string(),
                code: 404,
            }),
        ))
    }
}

fn build_client_fingerprint(headers: &HeaderMap) -> String {
    let ip = extract_client_ip(headers);
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("unknown");
    let raw = format!("{ip}|{user_agent}");

    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn extract_client_ip(headers: &HeaderMap) -> String {
    // Trust X-Real-IP first (explicitly set by local reverse proxy), then
    // fall back to X-Forwarded-For chain.
    parse_first_ip_from_header(headers.get("x-real-ip"))
        .or_else(|| parse_first_ip_from_header(headers.get("x-forwarded-for")))
        .unwrap_or_else(|| "unknown".to_string())
}

fn parse_first_ip_from_header(value: Option<&axum::http::HeaderValue>) -> Option<String> {
    let raw = value?.to_str().ok()?;
    raw.split(',').find_map(normalize_ip_token)
}

fn normalize_ip_token(token: &str) -> Option<String> {
    let mut value = token.trim().trim_matches('"');
    if value.is_empty() || value.eq_ignore_ascii_case("unknown") {
        return None;
    }

    // Handle RFC7239 style token fragment: for=1.2.3.4
    if let Some(stripped) = value.strip_prefix("for=") {
        value = stripped.trim().trim_matches('"');
    }

    // [IPv6]:port
    if value.starts_with('[') {
        if let Some(end) = value.find(']') {
            let host = &value[1..end];
            let remain = value[end + 1..].trim();
            let valid_suffix = remain.is_empty()
                || (remain.starts_with(':') && remain[1..].chars().all(|ch| ch.is_ascii_digit()));
            if valid_suffix {
                if let Ok(ip) = host.parse::<IpAddr>() {
                    return Some(ip.to_string());
                }
            }
        }
    }

    // Plain IP literal (IPv4 or IPv6).
    if let Ok(ip) = value.parse::<IpAddr>() {
        return Some(ip.to_string());
    }

    // IPv4:port
    if let Some((host, port)) = value.rsplit_once(':') {
        if host.contains('.') && !port.is_empty() && port.chars().all(|ch| ch.is_ascii_digit()) {
            if let Ok(ip) = host.parse::<IpAddr>() {
                return Some(ip.to_string());
            }
        }
    }

    None
}

fn bad_request(message: &str) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::BAD_REQUEST,
        Json(ErrorResponse {
            error: message.to_string(),
            code: 400,
        }),
    )
}

fn is_valid_day_format(value: &str) -> bool {
    if value.len() != 10 {
        return false;
    }
    let bytes = value.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if index == 4 || index == 7 {
            if *byte != b'-' {
                return false;
            }
        } else if !byte.is_ascii_digit() {
            return false;
        }
    }
    true
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

fn apply_view_analytics_config_update(
    current: ViewAnalyticsRuntimeConfig,
    request: UpdateViewAnalyticsConfigRequest,
) -> Result<ViewAnalyticsRuntimeConfig, (StatusCode, Json<ErrorResponse>)> {
    let mut next = current;

    if let Some(value) = request.dedupe_window_seconds {
        if value == 0 || value > MAX_CONFIGURABLE_VIEW_DEDUPE_WINDOW_SECONDS {
            return Err(bad_request("`dedupe_window_seconds` must be between 1 and 3600"));
        }
        next.dedupe_window_seconds = value;
    }

    if let Some(value) = request.trend_max_days {
        if value == 0 || value > MAX_CONFIGURABLE_VIEW_TREND_DAYS {
            return Err(bad_request("`trend_max_days` must be between 1 and 365"));
        }
        next.trend_max_days = value;
    }

    if let Some(value) = request.trend_default_days {
        if value == 0 || value > MAX_CONFIGURABLE_VIEW_TREND_DAYS {
            return Err(bad_request("`trend_default_days` must be between 1 and 365"));
        }
        next.trend_default_days = value;
    }

    if next.trend_default_days > next.trend_max_days {
        return Err(bad_request(
            "`trend_default_days` must be less than or equal to `trend_max_days`",
        ));
    }

    Ok(next)
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue};

    use super::{
        apply_view_analytics_config_update, extract_client_ip, UpdateViewAnalyticsConfigRequest,
    };
    use crate::state::ViewAnalyticsRuntimeConfig;

    #[test]
    fn extract_client_ip_prefers_x_real_ip() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", HeaderValue::from_static("203.0.113.9"));
        headers.insert("x-forwarded-for", HeaderValue::from_static("198.51.100.1, 198.51.100.2"));

        assert_eq!(extract_client_ip(&headers), "203.0.113.9");
    }

    #[test]
    fn extract_client_ip_falls_back_to_x_forwarded_for() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", HeaderValue::from_static("198.51.100.1, 198.51.100.2"));

        assert_eq!(extract_client_ip(&headers), "198.51.100.1");
    }

    #[test]
    fn extract_client_ip_normalizes_ip_with_port() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", HeaderValue::from_static("198.51.100.1:4567"));
        assert_eq!(extract_client_ip(&headers), "198.51.100.1");
    }

    #[test]
    fn extract_client_ip_supports_rfc7239_for_token() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", HeaderValue::from_static("for=198.51.100.77"));
        assert_eq!(extract_client_ip(&headers), "198.51.100.77");
    }

    #[test]
    fn extract_client_ip_returns_unknown_when_no_valid_header() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", HeaderValue::from_static("not-an-ip"));
        headers.insert("x-forwarded-for", HeaderValue::from_static("unknown, bad-token"));

        assert_eq!(extract_client_ip(&headers), "unknown");
    }

    #[test]
    fn update_view_analytics_config_rejects_invalid_ranges() {
        let result = apply_view_analytics_config_update(
            ViewAnalyticsRuntimeConfig::default(),
            UpdateViewAnalyticsConfigRequest {
                dedupe_window_seconds: Some(0),
                trend_default_days: None,
                trend_max_days: None,
            },
        );
        assert!(result.is_err());

        let result = apply_view_analytics_config_update(
            ViewAnalyticsRuntimeConfig::default(),
            UpdateViewAnalyticsConfigRequest {
                dedupe_window_seconds: None,
                trend_default_days: Some(300),
                trend_max_days: Some(30),
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn update_view_analytics_config_applies_partial_update() {
        let config = apply_view_analytics_config_update(
            ViewAnalyticsRuntimeConfig::default(),
            UpdateViewAnalyticsConfigRequest {
                dedupe_window_seconds: Some(120),
                trend_default_days: None,
                trend_max_days: Some(240),
            },
        )
        .expect("should apply partial config update");

        assert_eq!(config.dedupe_window_seconds, 120);
        assert_eq!(config.trend_default_days, 30);
        assert_eq!(config.trend_max_days, 240);
    }
}
