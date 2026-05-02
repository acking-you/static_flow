use std::collections::BTreeMap;

#[cfg(not(feature = "mock"))]
use gloo_net::http::{Request, RequestBuilder};
use js_sys::Date;
#[cfg(not(feature = "mock"))]
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use static_flow_shared::{Article, ArticleListItem};
#[cfg(not(feature = "mock"))]
use wasm_bindgen::JsValue;

#[cfg(feature = "mock")]
use crate::models;

pub const DEFAULT_LLM_GATEWAY_CODEX_CLIENT_VERSION: &str = "0.124.0";

fn default_codex_client_version() -> String {
    DEFAULT_LLM_GATEWAY_CODEX_CLIENT_VERSION.to_string()
}

// API base URL. Read at compile time from STATICFLOW_API_BASE and fall back
// to the local development backend when the variable is absent.
#[cfg(not(feature = "mock"))]
pub const API_BASE: &str = match option_env!("STATICFLOW_API_BASE") {
    Some(url) => url,
    None => "http://localhost:3000/api",
};

#[cfg(any(not(feature = "mock"), test))]
const LOCAL_MEDIA_API_BASE_OVERRIDE: Option<&str> = option_env!("STATICFLOW_LOCAL_MEDIA_API_BASE");

#[cfg(feature = "mock")]
pub const API_BASE: &str = "http://localhost:3000/api";

#[cfg(not(feature = "mock"))]
fn current_page_path() -> Option<String> {
    let window = web_sys::window()?;
    let location = window.location();
    let mut path = location.pathname().ok().unwrap_or_else(|| "/".to_string());
    if path.trim().is_empty() {
        path = "/".to_string();
    }
    let search = location.search().ok().unwrap_or_default();
    let hash = location.hash().ok().unwrap_or_default();
    if !search.is_empty() {
        path.push_str(&search);
    }
    if !hash.is_empty() {
        path.push_str(&hash);
    }
    Some(path)
}

#[cfg(not(feature = "mock"))]
fn with_behavior_headers(mut builder: RequestBuilder) -> RequestBuilder {
    builder = builder.header("x-sf-client", "web");
    if let Some(path) = current_page_path() {
        builder = builder.header("x-sf-page", &path);
    }
    builder
}

#[cfg(not(feature = "mock"))]
fn api_get(url: &str) -> RequestBuilder {
    with_behavior_headers(Request::get(url))
}

#[cfg(not(feature = "mock"))]
fn api_post(url: &str) -> RequestBuilder {
    with_behavior_headers(Request::post(url))
}

#[cfg(not(feature = "mock"))]
fn api_patch(url: &str) -> RequestBuilder {
    with_behavior_headers(Request::patch(url))
}

#[cfg(not(feature = "mock"))]
fn api_delete(url: &str) -> RequestBuilder {
    with_behavior_headers(Request::delete(url))
}

#[cfg(any(not(feature = "mock"), test))]
fn local_media_api_base() -> String {
    if let Some(override_base) = LOCAL_MEDIA_API_BASE_OVERRIDE {
        let trimmed = override_base.trim_end_matches('/');
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    derive_local_media_api_base_from_api_base(API_BASE)
}

#[cfg(any(not(feature = "mock"), test))]
fn derive_local_media_api_base_from_api_base(api_base: &str) -> String {
    let trimmed = api_base.trim();
    if trimmed.is_empty() {
        return "/admin/local-media/api".to_string();
    }

    let without_trailing = trimmed.trim_end_matches('/');
    if let Some(prefix) = without_trailing.strip_suffix("/api") {
        return format!("{prefix}/admin/local-media/api");
    }

    if without_trailing.starts_with("http://") || without_trailing.starts_with("https://") {
        return format!("{without_trailing}/admin/local-media/api");
    }

    format!("{}/admin/local-media/api", without_trailing.trim_end_matches('/'))
}

#[cfg(not(feature = "mock"))]
fn resolve_local_media_asset_url(url: String) -> String {
    resolve_local_media_asset_url_for_base(&local_media_api_base(), &url)
}

#[cfg(any(not(feature = "mock"), test))]
fn derive_local_media_origin(base: &str) -> Option<String> {
    if !(base.starts_with("http://") || base.starts_with("https://")) {
        return None;
    }
    let trimmed = base.trim_end_matches('/');
    let admin_suffix = "/admin/local-media/api";
    if let Some(origin) = trimmed.strip_suffix(admin_suffix) {
        return Some(origin.to_string());
    }
    let api_suffix = "/api";
    if let Some(origin) = trimmed.strip_suffix(api_suffix) {
        return Some(origin.to_string());
    }
    Some(trimmed.to_string())
}

#[cfg(any(not(feature = "mock"), test))]
fn resolve_local_media_asset_url_for_base(local_media_api_base: &str, url: &str) -> String {
    if url.starts_with("http://") || url.starts_with("https://") {
        return url.to_string();
    }
    if !url.starts_with('/') {
        return url.to_string();
    }
    match derive_local_media_origin(local_media_api_base) {
        Some(origin) => format!("{origin}{url}"),
        None => url.to_string(),
    }
}

#[cfg(not(feature = "mock"))]
fn normalize_local_media_list_response(
    mut response: LocalMediaListResponse,
) -> LocalMediaListResponse {
    for entry in &mut response.entries {
        if let Some(poster_url) = entry.poster_url.take() {
            entry.poster_url = Some(resolve_local_media_asset_url(poster_url));
        }
    }
    response
}

#[cfg(not(feature = "mock"))]
fn normalize_local_media_playback_response(
    mut response: LocalMediaPlaybackOpenResponse,
) -> LocalMediaPlaybackOpenResponse {
    if let Some(player_url) = response.player_url.take() {
        response.player_url = Some(resolve_local_media_asset_url(player_url));
    }
    response
}

#[cfg(not(feature = "mock"))]
fn normalize_local_media_job_response(
    mut response: LocalMediaPlaybackJobResponse,
) -> LocalMediaPlaybackJobResponse {
    if let Some(player_url) = response.player_url.take() {
        response.player_url = Some(resolve_local_media_asset_url(player_url));
    }
    response
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TagInfo {
    pub name: String,
    pub count: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CategoryInfo {
    pub name: String,
    pub count: usize,
    pub description: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct SiteStats {
    pub total_articles: usize,
    pub total_tags: usize,
    pub total_categories: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct ArticleViewPoint {
    pub key: String,
    pub views: u32,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ArticleViewTrackResponse {
    pub article_id: String,
    pub counted: bool,
    pub total_views: usize,
    pub timezone: String,
    pub today_views: u32,
    pub daily_points: Vec<ArticleViewPoint>,
    pub server_time_ms: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ArticleViewTrendResponse {
    pub article_id: String,
    pub timezone: String,
    pub granularity: String,
    pub day: Option<String>,
    pub total_views: usize,
    pub points: Vec<ArticleViewPoint>,
}

#[cfg(not(feature = "mock"))]
#[derive(Debug, Deserialize)]
#[allow(
    dead_code,
    reason = "The backend returns pagination metadata that some callers intentionally ignore."
)]
struct ArticleListResponse {
    articles: Vec<ArticleListItem>,
    total: usize,
    #[serde(default)]
    offset: usize,
    #[serde(default)]
    limit: usize,
    #[serde(default)]
    has_more: bool,
}

/// Public pagination result for article pages
#[derive(Debug, Clone)]
pub struct ArticlePage {
    pub articles: Vec<ArticleListItem>,
    pub total: usize,
    #[allow(
        dead_code,
        reason = "Some UI paths only need the article list and total count, but retaining \
                  has_more keeps the DTO aligned with backend pagination."
    )]
    pub has_more: bool,
}

#[cfg(not(feature = "mock"))]
#[derive(Debug, Deserialize)]
struct TagsResponse {
    tags: Vec<TagInfo>,
}

#[cfg(not(feature = "mock"))]
#[derive(Debug, Deserialize)]
struct CategoriesResponse {
    categories: Vec<CategoryInfo>,
}

/// 获取文章列表，支持按标签和分类过滤，支持分页
pub async fn fetch_articles(
    tag: Option<&str>,
    category: Option<&str>,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<ArticlePage, String> {
    #[cfg(feature = "mock")]
    {
        let mut articles = models::get_mock_articles();

        if let Some(t) = tag {
            articles.retain(|article| article.tags.iter().any(|tag| tag.eq_ignore_ascii_case(t)));
        }

        if let Some(c) = category {
            articles.retain(|article| article.category.eq_ignore_ascii_case(c));
        }

        let total = articles.len();
        let off = offset.unwrap_or(0);
        let articles = match limit {
            Some(l) => articles.into_iter().skip(off).take(l).collect(),
            None => articles,
        };
        let has_more = limit.is_some_and(|l| off + l < total);

        Ok(ArticlePage {
            articles,
            total,
            has_more,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let mut url = format!("{}/articles", API_BASE);
        let mut params = Vec::new();

        if let Some(t) = tag {
            params.push(format!("tag={}", t));
        }
        if let Some(c) = category {
            params.push(format!("category={}", c));
        }
        if let Some(l) = limit {
            params.push(format!("limit={}", l));
        }
        if let Some(o) = offset {
            params.push(format!("offset={}", o));
        }
        params.push(format!("_ts={}", Date::now() as u64));

        if !params.is_empty() {
            url.push('?');
            url.push_str(&params.join("&"));
        }

        let response = api_get(&url)
            .header("Cache-Control", "no-cache, no-store, max-age=0")
            .header("Pragma", "no-cache")
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;

        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }

        let json_response: ArticleListResponse = response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))?;

        Ok(ArticlePage {
            articles: json_response.articles,
            total: json_response.total,
            has_more: json_response.has_more,
        })
    }
}

/// Fetch all articles without pagination (for posts/archive pages)
pub async fn fetch_all_articles(
    tag: Option<&str>,
    category: Option<&str>,
) -> Result<Vec<ArticleListItem>, String> {
    let page = fetch_articles(tag, category, None, None).await?;
    Ok(page.articles)
}

/// 获取文章详情
pub async fn fetch_article_detail(id: &str) -> Result<Option<Article>, String> {
    #[cfg(feature = "mock")]
    {
        Ok(models::get_mock_article_detail(id))
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/articles/{}?_ts={}", API_BASE, id, Date::now() as u64);

        let response = api_get(&url)
            .header("Cache-Control", "no-cache, no-store, max-age=0")
            .header("Pragma", "no-cache")
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;

        if response.status() == 404 {
            return Ok(None);
        }

        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }

        let article: Article = response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))?;

        Ok(Some(article))
    }
}

/// Fetch raw markdown body for one article and language (`zh` or `en`).
pub async fn fetch_article_raw_markdown(id: &str, lang: &str) -> Result<String, String> {
    #[cfg(feature = "mock")]
    {
        let article =
            models::get_mock_article_detail(id).ok_or_else(|| "Article not found".to_string())?;
        let normalized_lang = lang.trim().to_ascii_lowercase();
        let content = match normalized_lang.as_str() {
            "zh" => article.content,
            "en" => article
                .content_en
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| "English article markdown not found".to_string())?,
            _ => return Err("`lang` must be `zh` or `en`".to_string()),
        };
        Ok(content)
    }

    #[cfg(not(feature = "mock"))]
    {
        let normalized_lang = lang.trim().to_ascii_lowercase();
        if normalized_lang != "zh" && normalized_lang != "en" {
            return Err("`lang` must be `zh` or `en`".to_string());
        }

        let url = format!(
            "{}/articles/{}/raw/{}?_ts={}",
            API_BASE,
            urlencoding::encode(id),
            urlencoding::encode(&normalized_lang),
            Date::now() as u64
        );

        let response = api_get(&url)
            .header("Cache-Control", "no-cache, no-store, max-age=0")
            .header("Pragma", "no-cache")
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;

        if response.status() == 404 {
            return Err("Raw article markdown not found".to_string());
        }
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }

        response
            .text()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

/// Track one article detail view with backend-side dedupe.
pub async fn track_article_view(id: &str) -> Result<ArticleViewTrackResponse, String> {
    #[cfg(feature = "mock")]
    {
        Ok(ArticleViewTrackResponse {
            article_id: id.to_string(),
            counted: true,
            total_views: 128,
            timezone: "Asia/Shanghai".to_string(),
            today_views: 12,
            daily_points: (0..30)
                .map(|offset| ArticleViewPoint {
                    key: format!("2026-02-{:02}", offset + 1),
                    views: ((offset * 7 + 11) % 42) as u32,
                })
                .collect(),
            server_time_ms: 0,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/articles/{}/view", API_BASE, urlencoding::encode(id));
        let response = api_post(&url)
            .header("Cache-Control", "no-cache, no-store, max-age=0")
            .header("Pragma", "no-cache")
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;

        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }

        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

/// Fetch article view trend points.
pub async fn fetch_article_view_trend(
    id: &str,
    granularity: &str,
    days: Option<usize>,
    day: Option<&str>,
) -> Result<ArticleViewTrendResponse, String> {
    #[cfg(feature = "mock")]
    {
        if granularity.eq_ignore_ascii_case("hour") {
            return Ok(ArticleViewTrendResponse {
                article_id: id.to_string(),
                timezone: "Asia/Shanghai".to_string(),
                granularity: "hour".to_string(),
                day: Some(day.unwrap_or("2026-02-15").to_string()),
                total_views: 128,
                points: (0..24)
                    .map(|hour| ArticleViewPoint {
                        key: format!("{hour:02}"),
                        views: ((hour * 3 + 5) % 18) as u32,
                    })
                    .collect(),
            });
        }

        let window = days.unwrap_or(30).max(1);
        Ok(ArticleViewTrendResponse {
            article_id: id.to_string(),
            timezone: "Asia/Shanghai".to_string(),
            granularity: "day".to_string(),
            day: None,
            total_views: 128,
            points: (0..window)
                .map(|offset| ArticleViewPoint {
                    key: format!("2026-02-{:02}", offset + 1),
                    views: ((offset * 5 + 9) % 40) as u32,
                })
                .collect(),
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let mut url = format!(
            "{}/articles/{}/view-trend?granularity={}",
            API_BASE,
            urlencoding::encode(id),
            urlencoding::encode(granularity),
        );
        if let Some(days) = days {
            url.push_str(&format!("&days={days}"));
        }
        if let Some(day) = day {
            url.push_str(&format!("&day={}", urlencoding::encode(day)));
        }

        let response = api_get(&url)
            .header("Cache-Control", "no-cache, no-store, max-age=0")
            .header("Pragma", "no-cache")
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;

        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }

        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

/// 获取所有标签及其文章数量
pub async fn fetch_tags() -> Result<Vec<TagInfo>, String> {
    #[cfg(feature = "mock")]
    {
        Ok(models::mock_tags())
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/tags", API_BASE);

        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;

        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }

        let json_response: TagsResponse = response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))?;

        Ok(json_response.tags)
    }
}

/// 获取所有分类及其文章数量和描述
pub async fn fetch_categories() -> Result<Vec<CategoryInfo>, String> {
    #[cfg(feature = "mock")]
    {
        Ok(models::mock_categories())
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/categories", API_BASE);

        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;

        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }

        let json_response: CategoriesResponse = response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))?;

        Ok(json_response.categories)
    }
}

#[cfg(feature = "local-media")]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LocalMediaEntryKind {
    Directory,
    Video,
}

#[cfg(feature = "local-media")]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LocalMediaEntry {
    pub kind: LocalMediaEntryKind,
    pub name: String,
    pub relative_path: String,
    pub size_bytes: Option<u64>,
    pub modified_at_ms: Option<i64>,
    pub extension: Option<String>,
    pub poster_url: Option<String>,
}

#[cfg(feature = "local-media")]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LocalMediaListResponse {
    pub configured: bool,
    pub current_dir: String,
    pub parent_dir: Option<String>,
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
    pub has_more: bool,
    pub entries: Vec<LocalMediaEntry>,
}

#[cfg(feature = "local-media")]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LocalMediaPlaybackStatus {
    Ready,
    Preparing,
    Failed,
}

#[cfg(feature = "local-media")]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LocalMediaPlaybackMode {
    Raw,
    Hls,
}

#[cfg(feature = "local-media")]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LocalMediaPlaybackOpenResponse {
    pub status: LocalMediaPlaybackStatus,
    pub mode: Option<LocalMediaPlaybackMode>,
    pub job_id: Option<String>,
    pub player_url: Option<String>,
    pub title: String,
    pub duration_seconds: Option<f64>,
    pub detail: Option<String>,
    pub error: Option<String>,
}

#[cfg(feature = "local-media")]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LocalMediaPlaybackJobResponse {
    pub job_id: String,
    pub status: LocalMediaPlaybackStatus,
    pub mode: Option<LocalMediaPlaybackMode>,
    pub player_url: Option<String>,
    pub duration_seconds: Option<f64>,
    pub detail: Option<String>,
    pub error: Option<String>,
}

#[cfg(all(feature = "local-media", not(feature = "mock")))]
#[derive(Debug, Serialize)]
struct LocalMediaPlaybackOpenRequest<'a> {
    file: &'a str,
}

#[cfg(all(feature = "local-media", not(feature = "mock")))]
pub fn build_admin_local_media_raw_playback(file: &str) -> LocalMediaPlaybackOpenResponse {
    let title = std::path::Path::new(file)
        .file_name()
        .and_then(|value| value.to_str())
        .map(ToString::to_string)
        .unwrap_or_else(|| file.to_string());
    let player_url =
        format!("{}/playback/raw?file={}", local_media_api_base(), urlencoding::encode(file));
    LocalMediaPlaybackOpenResponse {
        status: LocalMediaPlaybackStatus::Ready,
        mode: Some(LocalMediaPlaybackMode::Raw),
        job_id: None,
        player_url: Some(player_url),
        title,
        duration_seconds: None,
        detail: Some(
            "Streaming the original file directly. Switch to compatibility mode only if this \
             browser cannot play it."
                .to_string(),
        ),
        error: None,
    }
}

#[cfg(all(feature = "local-media", feature = "mock"))]
pub fn build_admin_local_media_raw_playback(file: &str) -> LocalMediaPlaybackOpenResponse {
    LocalMediaPlaybackOpenResponse {
        status: LocalMediaPlaybackStatus::Failed,
        mode: Some(LocalMediaPlaybackMode::Raw),
        job_id: None,
        player_url: None,
        title: file.to_string(),
        duration_seconds: None,
        detail: None,
        error: Some("Local media is unavailable in mock mode".to_string()),
    }
}

#[cfg(feature = "local-media")]
pub async fn fetch_admin_local_media_list(
    dir: Option<&str>,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<LocalMediaListResponse, String> {
    #[cfg(feature = "mock")]
    {
        let _ = (dir, limit, offset);
        Err("Local media is unavailable in mock mode".to_string())
    }

    #[cfg(not(feature = "mock"))]
    {
        let mut params = Vec::new();
        if let Some(dir) = dir.filter(|value| !value.trim().is_empty()) {
            params.push(format!("dir={}", urlencoding::encode(dir)));
        }
        if let Some(limit) = limit {
            params.push(format!("limit={limit}"));
        }
        if let Some(offset) = offset {
            params.push(format!("offset={offset}"));
        }
        let mut url = format!("{}/list", local_media_api_base());
        if !params.is_empty() {
            url.push('?');
            url.push_str(&params.join("&"));
        }
        let response = api_get(&url).send().await.map_err(|err| err.to_string())?;
        if !response.ok() {
            return Err(format!("Failed to load local media: HTTP {}", response.status()));
        }
        let response = response.json().await.map_err(|err| err.to_string())?;
        Ok(normalize_local_media_list_response(response))
    }
}

#[cfg(feature = "local-media")]
pub async fn open_admin_local_media_playback(
    file: &str,
) -> Result<LocalMediaPlaybackOpenResponse, String> {
    #[cfg(feature = "mock")]
    {
        let _ = file;
        Err("Local media is unavailable in mock mode".to_string())
    }

    #[cfg(not(feature = "mock"))]
    {
        let response = api_post(&format!("{}/playback/open", local_media_api_base()))
            .json(&LocalMediaPlaybackOpenRequest {
                file,
            })
            .map_err(|err| err.to_string())?
            .send()
            .await
            .map_err(|err| err.to_string())?;
        if !response.ok() {
            return Err(format!("Failed to open local media playback: HTTP {}", response.status()));
        }
        let response = response.json().await.map_err(|err| err.to_string())?;
        Ok(normalize_local_media_playback_response(response))
    }
}

#[cfg(feature = "local-media")]
pub async fn fetch_admin_local_media_job_status(
    job_id: &str,
) -> Result<LocalMediaPlaybackJobResponse, String> {
    #[cfg(feature = "mock")]
    {
        let _ = job_id;
        Err("Local media is unavailable in mock mode".to_string())
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/playback/jobs/{job_id}", local_media_api_base());
        let response = api_get(&url).send().await.map_err(|err| err.to_string())?;
        if !response.ok() {
            return Err(format!("Failed to fetch playback job status: HTTP {}", response.status()));
        }
        let response = response.json().await.map_err(|err| err.to_string())?;
        Ok(normalize_local_media_job_response(response))
    }
}

#[cfg(all(feature = "local-media", not(feature = "mock")))]
#[derive(Debug, Deserialize)]
struct LocalMediaApiErrorResponse {
    error: String,
}

#[cfg(all(feature = "local-media", not(feature = "mock")))]
async fn local_media_api_error(response: gloo_net::http::Response, fallback: &str) -> String {
    let status = response.status();
    match response.json::<LocalMediaApiErrorResponse>().await {
        Ok(payload) if !payload.error.trim().is_empty() => payload.error,
        _ => format!("{fallback}: HTTP {status}"),
    }
}

#[cfg(all(feature = "local-media", any(not(feature = "mock"), test)))]
pub fn build_admin_local_media_upload_tasks_url() -> String {
    format!("{}/uploads/tasks", local_media_api_base())
}

#[cfg(feature = "local-media")]
pub async fn create_admin_local_media_upload_task(
    request: &static_flow_media_types::CreateUploadTaskRequest,
) -> Result<static_flow_media_types::UploadTaskRecord, String> {
    #[cfg(feature = "mock")]
    {
        let _ = request;
        Err("Local media is unavailable in mock mode".to_string())
    }

    #[cfg(not(feature = "mock"))]
    {
        let response = api_post(&build_admin_local_media_upload_tasks_url())
            .json(request)
            .map_err(|err| err.to_string())?
            .send()
            .await
            .map_err(|err| err.to_string())?;
        if !response.ok() {
            return Err(local_media_api_error(response, "Failed to create upload task").await);
        }
        let payload: static_flow_media_types::CreateUploadTaskResponse =
            response.json().await.map_err(|err| err.to_string())?;
        Ok(payload.task)
    }
}

#[cfg(feature = "local-media")]
pub async fn fetch_admin_local_media_upload_tasks(
    dir: Option<&str>,
) -> Result<static_flow_media_types::ListUploadTasksResponse, String> {
    #[cfg(feature = "mock")]
    {
        let _ = dir;
        Err("Local media is unavailable in mock mode".to_string())
    }

    #[cfg(not(feature = "mock"))]
    {
        let mut url = build_admin_local_media_upload_tasks_url();
        if let Some(dir) = dir.filter(|value| !value.trim().is_empty()) {
            url.push_str(&format!("?dir={}", urlencoding::encode(dir)));
        }
        let response = api_get(&url).send().await.map_err(|err| err.to_string())?;
        if !response.ok() {
            return Err(local_media_api_error(response, "Failed to load upload tasks").await);
        }
        response.json().await.map_err(|err| err.to_string())
    }
}

#[cfg(feature = "local-media")]
pub async fn fetch_admin_local_media_upload_task(
    task_id: &str,
) -> Result<static_flow_media_types::UploadTaskRecord, String> {
    #[cfg(feature = "mock")]
    {
        let _ = task_id;
        Err("Local media is unavailable in mock mode".to_string())
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/{}", build_admin_local_media_upload_tasks_url(), task_id);
        let response = api_get(&url).send().await.map_err(|err| err.to_string())?;
        if !response.ok() {
            return Err(local_media_api_error(response, "Failed to load upload task").await);
        }
        response.json().await.map_err(|err| err.to_string())
    }
}

#[cfg(feature = "local-media")]
pub async fn append_admin_local_media_upload_chunk(
    task_id: &str,
    offset: u64,
    bytes: Vec<u8>,
) -> Result<static_flow_media_types::UploadTaskRecord, String> {
    #[cfg(feature = "mock")]
    {
        let _ = (task_id, offset, bytes);
        Err("Local media is unavailable in mock mode".to_string())
    }

    #[cfg(not(feature = "mock"))]
    {
        let url =
            format!("{}/uploads/tasks/{task_id}/chunks?offset={offset}", local_media_api_base());
        let response = gloo_net::http::Request::put(&url)
            .header("Content-Type", "application/octet-stream")
            .body(bytes)
            .map_err(|err| err.to_string())?
            .send()
            .await
            .map_err(|err| err.to_string())?;
        if !response.ok() {
            return Err(local_media_api_error(response, "Failed to append upload chunk").await);
        }
        let payload: static_flow_media_types::UploadChunkResponse =
            response.json().await.map_err(|err| err.to_string())?;
        Ok(payload.task)
    }
}

#[cfg(feature = "local-media")]
pub async fn delete_admin_local_media_upload_task(
    task_id: &str,
) -> Result<static_flow_media_types::UploadTaskRecord, String> {
    #[cfg(feature = "mock")]
    {
        let _ = task_id;
        Err("Local media is unavailable in mock mode".to_string())
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/{}", build_admin_local_media_upload_tasks_url(), task_id);
        let response = api_delete(&url)
            .send()
            .await
            .map_err(|err| err.to_string())?;
        if !response.ok() {
            return Err(local_media_api_error(response, "Failed to delete upload task").await);
        }
        response.json().await.map_err(|err| err.to_string())
    }
}

/// Fetch site-level counts for home page stats.
pub async fn fetch_site_stats() -> Result<SiteStats, String> {
    #[cfg(feature = "mock")]
    {
        use std::collections::HashSet;

        let articles = models::get_mock_articles();
        let mut tags = HashSet::new();
        let mut categories = HashSet::new();

        for article in &articles {
            for tag in &article.tags {
                let normalized = tag.trim().to_lowercase();
                if !normalized.is_empty() {
                    tags.insert(normalized);
                }
            }

            let normalized_category = article.category.trim().to_lowercase();
            if !normalized_category.is_empty() {
                categories.insert(normalized_category);
            }
        }

        Ok(SiteStats {
            total_articles: articles.len(),
            total_tags: tags.len(),
            total_categories: categories.len(),
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/stats", API_BASE);

        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;

        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }

        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct SearchResult {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub category: String,
    pub date: String,
    pub highlight: String,
    pub tags: Vec<String>,
}

#[cfg(not(feature = "mock"))]
#[allow(
    dead_code,
    reason = "The client currently consumes only the result list, but the backend response still \
              carries metadata useful for diagnostics."
)]
#[derive(Debug, Deserialize)]
struct SearchResponse {
    results: Vec<SearchResult>,
    total: usize,
    query: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ImageInfo {
    pub id: String,
    pub filename: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImagePage {
    pub images: Vec<ImageInfo>,
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
    pub has_more: bool,
}

#[cfg(not(feature = "mock"))]
#[allow(
    dead_code,
    reason = "The client often collapses image pagination to the fields needed by the current \
              screen."
)]
#[derive(Debug, Deserialize)]
struct ImageListResponse {
    images: Vec<ImageInfo>,
    total: usize,
    #[serde(default)]
    offset: usize,
    #[serde(default)]
    limit: usize,
    #[serde(default)]
    has_more: bool,
}

#[cfg(not(feature = "mock"))]
#[allow(
    dead_code,
    reason = "The client often collapses image pagination to the fields needed by the current \
              screen."
)]
#[derive(Debug, Deserialize)]
struct ImageSearchResponse {
    images: Vec<ImageInfo>,
    total: usize,
    query_id: String,
    #[serde(default)]
    offset: usize,
    #[serde(default)]
    limit: usize,
    #[serde(default)]
    has_more: bool,
}

#[cfg(not(feature = "mock"))]
#[allow(
    dead_code,
    reason = "The client often collapses image pagination to the fields needed by the current \
              screen."
)]
#[derive(Debug, Deserialize)]
struct ImageTextSearchResponse {
    images: Vec<ImageInfo>,
    total: usize,
    query: String,
    #[serde(default)]
    offset: usize,
    #[serde(default)]
    limit: usize,
    #[serde(default)]
    has_more: bool,
}

/// 搜索文章
pub async fn search_articles(
    keyword: &str,
    limit: Option<usize>,
) -> Result<Vec<SearchResult>, String> {
    if keyword.trim().is_empty() {
        return Ok(vec![]);
    }

    #[cfg(feature = "mock")]
    {
        let mut results = models::mock_search(keyword);
        if let Some(limit) = limit {
            results.truncate(limit);
        }
        Ok(results)
    }

    #[cfg(not(feature = "mock"))]
    {
        let mut url = format!("{}/search?q={}", API_BASE, urlencoding::encode(keyword));
        if let Some(limit) = limit {
            url.push_str(&format!("&limit={limit}"));
        }

        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;

        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }

        let json_response: SearchResponse = response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))?;

        Ok(json_response.results)
    }
}

/// Semantic search articles (vector search).
///
/// When `enhanced_highlight` is true, backend will run semantic snippet
/// reranking to improve highlight precision at extra latency cost.
#[allow(
    clippy::too_many_arguments,
    reason = "This function mirrors the backend search query shape, so bundling it into an extra \
              options struct would only add call-site churn."
)]
pub async fn semantic_search_articles(
    keyword: &str,
    enhanced_highlight: bool,
    limit: Option<usize>,
    max_distance: Option<f32>,
    hybrid: bool,
    hybrid_rrf_k: Option<f32>,
    hybrid_vector_limit: Option<usize>,
    hybrid_fts_limit: Option<usize>,
) -> Result<Vec<SearchResult>, String> {
    if keyword.trim().is_empty() {
        return Ok(vec![]);
    }

    #[cfg(feature = "mock")]
    {
        let _ = (
            enhanced_highlight,
            max_distance,
            hybrid,
            hybrid_rrf_k,
            hybrid_vector_limit,
            hybrid_fts_limit,
        );
        let mut results = models::mock_search(keyword);
        if let Some(limit) = limit {
            results.truncate(limit);
        }
        Ok(results)
    }

    #[cfg(not(feature = "mock"))]
    {
        let mut url = format!("{}/semantic-search?q={}", API_BASE, urlencoding::encode(keyword));
        if enhanced_highlight {
            url.push_str("&enhanced_highlight=true");
        }
        if let Some(limit) = limit {
            url.push_str(&format!("&limit={limit}"));
        }
        if let Some(max_distance) = max_distance {
            url.push_str(&format!("&max_distance={max_distance}"));
        }
        if hybrid {
            url.push_str("&hybrid=true");
        }
        if let Some(rrf_k) = hybrid_rrf_k {
            url.push_str(&format!("&hybrid_rrf_k={rrf_k}"));
        }
        if let Some(vector_limit) = hybrid_vector_limit {
            url.push_str(&format!("&hybrid_vector_limit={vector_limit}"));
        }
        if let Some(fts_limit) = hybrid_fts_limit {
            url.push_str(&format!("&hybrid_fts_limit={fts_limit}"));
        }

        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;

        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }

        let json_response: SearchResponse = response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))?;

        Ok(json_response.results)
    }
}

/// Fetch related articles for a given article id.
pub async fn fetch_related_articles(id: &str) -> Result<Vec<ArticleListItem>, String> {
    #[cfg(feature = "mock")]
    {
        let articles = models::get_mock_articles();
        Ok(articles
            .into_iter()
            .filter(|a| a.id != id)
            .take(3)
            .collect())
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/articles/{}/related", API_BASE, id);

        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;

        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }

        let json_response: ArticleListResponse = response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))?;

        Ok(json_response.articles)
    }
}

/// Fetch all images for image-to-image search.
#[allow(
    dead_code,
    reason = "Some pages use paginated image APIs directly, but keeping the convenience wrapper \
              avoids duplicating trivial call sites."
)]
pub async fn fetch_images() -> Result<Vec<ImageInfo>, String> {
    let page = fetch_images_page(None, None).await?;
    Ok(page.images)
}

/// Fetch one image catalog page.
pub async fn fetch_images_page(
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<ImagePage, String> {
    #[cfg(feature = "mock")]
    {
        Ok(ImagePage {
            images: vec![],
            total: 0,
            offset: offset.unwrap_or(0),
            limit: limit.unwrap_or(0),
            has_more: false,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let mut url = format!("{}/images", API_BASE);
        if let Some(limit) = limit {
            url.push_str(&format!("?limit={limit}"));
            if let Some(offset) = offset {
                url.push_str(&format!("&offset={offset}"));
            }
        } else if let Some(offset) = offset {
            url.push_str(&format!("?offset={offset}"));
        }

        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;

        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }

        let json_response: ImageListResponse = response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))?;

        Ok(ImagePage {
            images: json_response.images,
            total: json_response.total,
            offset: json_response.offset,
            limit: json_response.limit,
            has_more: json_response.has_more,
        })
    }
}

/// Fetch random image recommendations.
pub async fn fetch_random_images_page(limit: Option<usize>) -> Result<ImagePage, String> {
    #[cfg(feature = "mock")]
    {
        Ok(ImagePage {
            images: vec![],
            total: 0,
            offset: 0,
            limit: limit.unwrap_or(10),
            has_more: false,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let mut url = format!("{}/images/random", API_BASE);
        if let Some(limit) = limit {
            url.push_str(&format!("?limit={limit}"));
        }

        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;

        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }

        let json_response: ImageListResponse = response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))?;

        Ok(ImagePage {
            images: json_response.images,
            total: json_response.total,
            offset: json_response.offset,
            limit: json_response.limit,
            has_more: json_response.has_more,
        })
    }
}

/// Search images by an existing image id.
#[allow(
    dead_code,
    reason = "Some pages use paginated image APIs directly, but keeping the convenience wrapper \
              avoids duplicating trivial call sites."
)]
pub async fn search_images_by_id(
    image_id: &str,
    limit: Option<usize>,
    max_distance: Option<f32>,
) -> Result<Vec<ImageInfo>, String> {
    let page = search_images_by_id_page(image_id, limit, None, max_distance).await?;
    Ok(page.images)
}

/// Search one page of similar images by id.
pub async fn search_images_by_id_page(
    image_id: &str,
    limit: Option<usize>,
    offset: Option<usize>,
    max_distance: Option<f32>,
) -> Result<ImagePage, String> {
    if image_id.trim().is_empty() {
        return Ok(ImagePage {
            images: vec![],
            total: 0,
            offset: offset.unwrap_or(0),
            limit: limit.unwrap_or(0),
            has_more: false,
        });
    }

    #[cfg(feature = "mock")]
    {
        let _ = max_distance;
        Ok(ImagePage {
            images: vec![],
            total: 0,
            offset: offset.unwrap_or(0),
            limit: limit.unwrap_or(0),
            has_more: false,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let mut url = format!("{}/image-search?id={}", API_BASE, urlencoding::encode(image_id));
        if let Some(limit) = limit {
            url.push_str(&format!("&limit={limit}"));
        }
        if let Some(offset) = offset {
            url.push_str(&format!("&offset={offset}"));
        }
        if let Some(max_distance) = max_distance {
            url.push_str(&format!("&max_distance={max_distance}"));
        }

        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;

        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }

        let json_response: ImageSearchResponse = response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))?;

        Ok(ImagePage {
            images: json_response.images,
            total: json_response.total,
            offset: json_response.offset,
            limit: json_response.limit,
            has_more: json_response.has_more,
        })
    }
}

/// Search images with text query (text-to-image).
#[allow(
    dead_code,
    reason = "Some pages use paginated image APIs directly, but keeping the convenience wrapper \
              avoids duplicating trivial call sites."
)]
pub async fn search_images_by_text(
    keyword: &str,
    limit: Option<usize>,
    max_distance: Option<f32>,
) -> Result<Vec<ImageInfo>, String> {
    let page = search_images_by_text_page(keyword, limit, None, max_distance).await?;
    Ok(page.images)
}

/// Search one page of images with text query.
pub async fn search_images_by_text_page(
    keyword: &str,
    limit: Option<usize>,
    offset: Option<usize>,
    max_distance: Option<f32>,
) -> Result<ImagePage, String> {
    if keyword.trim().is_empty() {
        return Ok(ImagePage {
            images: vec![],
            total: 0,
            offset: offset.unwrap_or(0),
            limit: limit.unwrap_or(0),
            has_more: false,
        });
    }

    #[cfg(feature = "mock")]
    {
        let _ = max_distance;
        Ok(ImagePage {
            images: vec![],
            total: 0,
            offset: offset.unwrap_or(0),
            limit: limit.unwrap_or(0),
            has_more: false,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let mut url = format!("{}/image-search-text?q={}", API_BASE, urlencoding::encode(keyword));
        if let Some(limit) = limit {
            url.push_str(&format!("&limit={limit}"));
        }
        if let Some(offset) = offset {
            url.push_str(&format!("&offset={offset}"));
        }
        if let Some(max_distance) = max_distance {
            url.push_str(&format!("&max_distance={max_distance}"));
        }

        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;

        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }

        let json_response: ImageTextSearchResponse = response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))?;

        Ok(ImagePage {
            images: json_response.images,
            total: json_response.total,
            offset: json_response.offset,
            limit: json_response.limit,
            has_more: json_response.has_more,
        })
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct CommentClientMeta {
    pub ua: Option<String>,
    pub language: Option<String>,
    pub platform: Option<String>,
    pub viewport: Option<String>,
    pub timezone: Option<String>,
    pub referrer: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct SubmitCommentRequest {
    pub article_id: String,
    pub entry_type: String,
    pub comment_text: String,
    pub selected_text: Option<String>,
    pub anchor_block_id: Option<String>,
    pub anchor_context_before: Option<String>,
    pub anchor_context_after: Option<String>,
    pub reply_to_comment_id: Option<String>,
    pub client_meta: Option<CommentClientMeta>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct SubmitCommentResponse {
    pub task_id: String,
    pub status: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ArticleComment {
    pub comment_id: String,
    pub article_id: String,
    pub task_id: String,
    pub author_name: String,
    pub author_avatar_seed: String,
    pub comment_text: String,
    pub selected_text: Option<String>,
    pub anchor_block_id: Option<String>,
    pub anchor_context_before: Option<String>,
    pub anchor_context_after: Option<String>,
    pub reply_to_comment_id: Option<String>,
    pub reply_to_comment_text: Option<String>,
    pub reply_to_ai_reply_markdown: Option<String>,
    pub ai_reply_markdown: Option<String>,
    pub ip_region: String,
    pub published_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct CommentListResponse {
    pub comments: Vec<ArticleComment>,
    pub total: usize,
    pub article_id: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct CommentStatsResponse {
    pub article_id: String,
    pub total: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct CommentRuntimeConfig {
    pub submit_rate_limit_seconds: u64,
    pub list_default_limit: usize,
    pub cleanup_retention_days: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct MusicRuntimeConfig {
    pub play_dedupe_window_seconds: u64,
    pub comment_rate_limit_seconds: u64,
    pub list_default_limit: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ViewAnalyticsConfig {
    pub dedupe_window_seconds: u64,
    pub trend_default_days: usize,
    pub trend_max_days: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ApiBehaviorConfig {
    pub retention_days: i64,
    pub default_days: usize,
    pub max_days: usize,
    pub flush_batch_size: usize,
    pub flush_interval_seconds: u64,
    pub flush_max_buffer_bytes: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct CompactionRuntimeConfig {
    pub enabled: bool,
    pub scan_interval_seconds: u64,
    pub fragment_threshold: usize,
    pub prune_older_than_hours: i64,
    pub worker_count: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct ApiBehaviorBucket {
    pub key: String,
    pub count: u32,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct AdminApiBehaviorEvent {
    pub event_id: String,
    pub occurred_at: i64,
    pub client_source: String,
    pub method: String,
    pub path: String,
    pub query: String,
    pub page_path: String,
    pub referrer: Option<String>,
    pub status_code: i32,
    pub latency_ms: i32,
    pub client_ip: String,
    pub ip_region: String,
    pub ua_raw: Option<String>,
    pub device_type: String,
    pub os_family: String,
    pub browser_family: String,
    pub request_id: String,
    pub trace_id: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct AdminApiBehaviorOverviewResponse {
    pub timezone: String,
    pub days: usize,
    pub total_events: usize,
    pub unique_ips: usize,
    pub unique_pages: usize,
    pub avg_latency_ms: f64,
    pub timeseries: Vec<ApiBehaviorBucket>,
    pub top_endpoints: Vec<ApiBehaviorBucket>,
    pub top_pages: Vec<ApiBehaviorBucket>,
    pub device_distribution: Vec<ApiBehaviorBucket>,
    pub browser_distribution: Vec<ApiBehaviorBucket>,
    pub os_distribution: Vec<ApiBehaviorBucket>,
    pub region_distribution: Vec<ApiBehaviorBucket>,
    pub recent_events: Vec<AdminApiBehaviorEvent>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct AdminApiBehaviorEventsResponse {
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
    pub has_more: bool,
    pub events: Vec<AdminApiBehaviorEvent>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct AdminApiBehaviorEventsQuery {
    pub days: Option<usize>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub path_contains: Option<String>,
    pub page_contains: Option<String>,
    pub device_type: Option<String>,
    pub method: Option<String>,
    pub status_code: Option<i32>,
    pub ip: Option<String>,
    pub date: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct AdminApiBehaviorCleanupRequest {
    pub retention_days: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct AdminApiBehaviorCleanupResponse {
    pub deleted_events: usize,
    pub before_ms: i64,
    pub retention_days: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct MemoryProfilerConfigSnapshot {
    pub enabled: bool,
    pub sample_rate: u64,
    pub min_alloc_bytes: usize,
    pub max_tracked_allocations: usize,
    pub stack_skip: usize,
    pub max_stack_depth: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct MemoryProfilerConfigUpdate {
    pub enabled: Option<bool>,
    pub sample_rate: Option<u64>,
    pub min_alloc_bytes: Option<usize>,
    pub max_tracked_allocations: Option<usize>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct MiProcessMemoryInfo {
    pub elapsed_millis: u64,
    pub user_millis: u64,
    pub system_millis: u64,
    pub current_rss_bytes: u64,
    pub peak_rss_bytes: u64,
    pub current_commit_bytes: u64,
    pub peak_commit_bytes: u64,
    pub page_faults: u64,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct MemoryProfilerOverview {
    pub generated_at_ms: i64,
    pub config: MemoryProfilerConfigSnapshot,
    pub process_uptime_secs: u64,
    pub tracked_allocations: usize,
    pub distinct_stacks: usize,
    pub dropped_allocations: u64,
    pub sampled_alloc_events: u64,
    pub sampled_dealloc_events: u64,
    pub sampled_realloc_events: u64,
    pub total_live_bytes_estimate: u64,
    pub total_alloc_bytes_estimate: u64,
    pub process_rss_bytes: u64,
    pub process_virtual_bytes: u64,
    pub mimalloc: MiProcessMemoryInfo,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct MemoryStackEntry {
    pub stack_id: String,
    pub live_bytes_estimate: u64,
    pub alloc_bytes_total_estimate: u64,
    pub alloc_count: u64,
    pub free_count: u64,
    pub live_ratio_heap: f64,
    pub live_ratio_rss: f64,
    pub frames: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct MemoryStackReport {
    pub generated_at_ms: i64,
    pub top: usize,
    pub total_live_bytes_estimate: u64,
    pub process_rss_bytes: u64,
    pub entries: Vec<MemoryStackEntry>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct MemoryFunctionEntry {
    pub function: String,
    pub module: String,
    pub live_bytes_estimate: u64,
    pub alloc_bytes_total_estimate: u64,
    pub stack_count: usize,
    pub alloc_count: u64,
    pub free_count: u64,
    pub live_ratio_heap: f64,
    pub live_ratio_rss: f64,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct MemoryFunctionReport {
    pub generated_at_ms: i64,
    pub top: usize,
    pub total_live_bytes_estimate: u64,
    pub process_rss_bytes: u64,
    pub entries: Vec<MemoryFunctionEntry>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct MemoryModuleEntry {
    pub module: String,
    pub live_bytes_estimate: u64,
    pub alloc_bytes_total_estimate: u64,
    pub function_count: usize,
    pub stack_count: usize,
    pub alloc_count: u64,
    pub free_count: u64,
    pub live_ratio_heap: f64,
    pub live_ratio_rss: f64,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct MemoryModuleReport {
    pub generated_at_ms: i64,
    pub top: usize,
    pub total_live_bytes_estimate: u64,
    pub process_rss_bytes: u64,
    pub entries: Vec<MemoryModuleEntry>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct AdminCommentTask {
    pub task_id: String,
    pub article_id: String,
    pub entry_type: String,
    pub status: String,
    pub comment_text: String,
    pub selected_text: Option<String>,
    pub anchor_block_id: Option<String>,
    pub anchor_context_before: Option<String>,
    pub anchor_context_after: Option<String>,
    pub client_ip: String,
    pub ip_region: String,
    pub fingerprint: String,
    pub ua: Option<String>,
    pub language: Option<String>,
    pub platform: Option<String>,
    pub timezone: Option<String>,
    pub viewport: Option<String>,
    pub referrer: Option<String>,
    pub admin_note: Option<String>,
    pub failure_reason: Option<String>,
    pub attempt_count: i32,
    pub created_at: i64,
    pub updated_at: i64,
    pub approved_at: Option<i64>,
    pub completed_at: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct AdminCommentTaskGroup {
    pub article_id: String,
    pub total: usize,
    pub status_counts: std::collections::HashMap<String, usize>,
    pub tasks: Vec<AdminCommentTask>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct AdminCommentTaskGroupedResponse {
    pub groups: Vec<AdminCommentTaskGroup>,
    pub total_tasks: usize,
    pub total_articles: usize,
    pub status_counts: std::collections::HashMap<String, usize>,
    #[serde(default)]
    pub offset: usize,
    #[serde(default)]
    pub has_more: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct AdminCommentPublishedResponse {
    pub comments: Vec<ArticleComment>,
    pub total: usize,
    #[serde(default)]
    pub offset: usize,
    #[serde(default)]
    pub has_more: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct AdminCleanupResponse {
    pub deleted_tasks: usize,
    pub before_ms: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct AdminPatchCommentTaskRequest {
    pub comment_text: Option<String>,
    pub selected_text: Option<String>,
    pub anchor_block_id: Option<String>,
    pub anchor_context_before: Option<String>,
    pub anchor_context_after: Option<String>,
    pub admin_note: Option<String>,
    pub operator: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct AdminPatchPublishedCommentRequest {
    pub ai_reply_markdown: Option<String>,
    pub comment_text: Option<String>,
    pub operator: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct AdminTaskActionRequest {
    pub operator: Option<String>,
    pub admin_note: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct AdminCleanupRequest {
    pub status: Option<String>,
    pub retention_days: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct AdminCommentAuditLog {
    pub log_id: String,
    pub task_id: String,
    pub action: String,
    pub operator: String,
    pub before_json: Option<String>,
    pub after_json: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct AdminCommentAuditResponse {
    pub logs: Vec<AdminCommentAuditLog>,
    pub total: usize,
    #[serde(default)]
    pub offset: usize,
    #[serde(default)]
    pub has_more: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct AdminCommentAiRun {
    pub run_id: String,
    pub task_id: String,
    pub status: String,
    pub runner_program: String,
    pub runner_args_json: String,
    pub skill_path: String,
    pub exit_code: Option<i32>,
    pub final_reply_markdown: Option<String>,
    pub failure_reason: Option<String>,
    pub started_at: i64,
    pub updated_at: i64,
    pub completed_at: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct AdminCommentAiRunChunk {
    pub chunk_id: String,
    pub run_id: String,
    pub task_id: String,
    pub stream: String,
    pub batch_index: i32,
    pub content: String,
    pub created_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct AdminCommentTaskAiOutputResponse {
    pub task_id: String,
    pub selected_run_id: Option<String>,
    pub runs: Vec<AdminCommentAiRun>,
    pub chunks: Vec<AdminCommentAiRunChunk>,
    pub merged_stdout: String,
    pub merged_stderr: String,
    pub merged_output: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct AdminCommentAiStreamEvent {
    pub event_type: String,
    pub task_id: String,
    pub run_id: String,
    pub run_status: Option<String>,
    pub chunk: Option<AdminCommentAiRunChunk>,
}

#[cfg(any(not(feature = "mock"), test))]
fn admin_base() -> String {
    API_BASE
        .strip_suffix("/api")
        .map(str::to_string)
        .unwrap_or_else(|| API_BASE.to_string())
}

fn default_admin_gpt2api_rs_proxy_mode() -> String {
    "inherit".to_string()
}

fn default_admin_gpt2api_rs_key_role() -> String {
    "user".to_string()
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gpt2ApiRsConfig {
    pub base_url: String,
    pub admin_token: String,
    pub api_key: String,
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminGpt2ApiRsConfigEnvelope {
    pub config_path: String,
    pub configured: bool,
    pub config: Gpt2ApiRsConfig,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminGpt2ApiRsAccountView {
    pub name: String,
    pub access_token: String,
    pub source_kind: String,
    pub email: Option<String>,
    pub user_id: Option<String>,
    pub plan_type: Option<String>,
    pub default_model_slug: Option<String>,
    pub status: String,
    pub quota_remaining: i64,
    pub quota_known: bool,
    pub restore_at: Option<String>,
    pub last_refresh_at: Option<i64>,
    pub last_used_at: Option<i64>,
    pub last_error: Option<String>,
    pub success_count: i64,
    pub fail_count: i64,
    pub request_max_concurrency: Option<u64>,
    pub request_min_start_interval_ms: Option<u64>,
    #[serde(default = "default_admin_gpt2api_rs_proxy_mode")]
    pub proxy_mode: String,
    #[serde(default)]
    pub proxy_config_id: Option<String>,
    pub browser_profile_json: String,
    #[serde(default)]
    pub effective_proxy_source: String,
    #[serde(default)]
    pub effective_proxy_url: Option<String>,
    #[serde(default)]
    pub effective_proxy_config_name: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminGpt2ApiRsProxyConfigView {
    pub id: String,
    pub name: String,
    pub proxy_url: String,
    #[serde(default)]
    pub proxy_username: Option<String>,
    #[serde(default)]
    pub proxy_password: Option<String>,
    pub status: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminGpt2ApiRsCreateProxyConfigRequest {
    pub name: String,
    pub proxy_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_password: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminGpt2ApiRsUpdateProxyConfigRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_username: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_password: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminGpt2ApiRsProxyCheckResult {
    pub ok: bool,
    pub message: String,
    #[serde(default)]
    pub status_code: Option<u16>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminGpt2ApiRsKeyView {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub secret_hash: String,
    pub status: String,
    #[serde(default, alias = "quota_total_images")]
    pub quota_total_calls: i64,
    #[serde(default, alias = "quota_used_images")]
    pub quota_used_calls: i64,
    pub route_strategy: String,
    pub account_group_id: Option<String>,
    #[serde(default)]
    pub fixed_account_name: Option<String>,
    pub request_max_concurrency: Option<u64>,
    pub request_min_start_interval_ms: Option<u64>,
    #[serde(default = "default_admin_gpt2api_rs_key_role")]
    pub role: String,
    #[serde(default)]
    pub secret_plaintext: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminGpt2ApiRsAccountGroupView {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub account_names: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminGpt2ApiRsAccountGroupsResponse {
    #[serde(default)]
    pub groups: Vec<AdminGpt2ApiRsAccountGroupView>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminGpt2ApiRsUsageEventView {
    pub event_id: String,
    pub request_id: String,
    pub key_id: String,
    pub key_name: String,
    pub account_name: String,
    pub endpoint: String,
    #[serde(default)]
    pub request_method: String,
    #[serde(default)]
    pub request_url: String,
    pub requested_model: String,
    pub resolved_upstream_model: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub image_size: Option<String>,
    pub requested_n: i64,
    pub generated_n: i64,
    pub billable_images: i64,
    #[serde(default)]
    pub billable_credits: i64,
    #[serde(default)]
    pub size_credit_units: i64,
    #[serde(default)]
    pub context_text_count: i64,
    #[serde(default)]
    pub context_image_count: i64,
    #[serde(default)]
    pub context_credit_surcharge: i64,
    #[serde(default)]
    pub client_ip: String,
    #[serde(default)]
    pub request_headers_json: Option<String>,
    #[serde(default)]
    pub prompt_preview: Option<String>,
    #[serde(default)]
    pub last_message_content: Option<String>,
    #[serde(default)]
    pub request_body_json: Option<String>,
    #[serde(default)]
    pub prompt_chars: i64,
    #[serde(default)]
    pub effective_prompt_chars: i64,
    pub status_code: i64,
    pub latency_ms: i64,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub detail_ref: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminGpt2ApiRsUsageEventsQuery {
    pub key_id: Option<String>,
    pub q: Option<String>,
    pub limit: Option<u64>,
    pub offset: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminGpt2ApiRsUsageEventsResponse {
    pub total: u64,
    pub offset: u64,
    pub limit: u64,
    pub has_more: bool,
    #[serde(default)]
    pub current_rpm: u32,
    #[serde(default)]
    pub current_in_flight: u32,
    pub billable_credit_total: i64,
    pub events: Vec<AdminGpt2ApiRsUsageEventView>,
    #[serde(default)]
    pub generated_at: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminGpt2ApiRsImportAccountsRequest {
    #[serde(default)]
    pub access_tokens: Vec<String>,
    #[serde(default)]
    pub session_jsons: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminGpt2ApiRsDeleteAccountsRequest {
    #[serde(default)]
    pub access_tokens: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminGpt2ApiRsCreateAccountGroupRequest {
    pub name: String,
    #[serde(default)]
    pub account_names: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminGpt2ApiRsUpdateAccountGroupRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_names: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminGpt2ApiRsRefreshAccountsRequest {
    #[serde(default)]
    pub access_tokens: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminGpt2ApiRsUpdateAccountRequest {
    pub access_token: String,
    #[serde(default)]
    pub plan_type: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub quota_remaining: Option<i64>,
    #[serde(default)]
    pub restore_at: Option<String>,
    #[serde(default)]
    pub session_token: Option<String>,
    #[serde(default)]
    pub user_agent: Option<String>,
    #[serde(default)]
    pub impersonate_browser: Option<String>,
    #[serde(default)]
    pub request_max_concurrency: Option<u64>,
    #[serde(default)]
    pub request_min_start_interval_ms: Option<u64>,
    #[serde(default)]
    pub proxy_mode: Option<String>,
    #[serde(default)]
    pub proxy_config_id: Option<Option<String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminGpt2ApiRsCreateKeyRequest {
    pub name: String,
    pub quota_total_calls: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    pub route_strategy: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_group_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fixed_account_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_max_concurrency: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_min_start_interval_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminGpt2ApiRsUpdateKeyRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quota_total_calls: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route_strategy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_group_id: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fixed_account_name: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_max_concurrency: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_min_start_interval_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminGpt2ApiRsImageGenerationRequest {
    pub prompt: String,
    pub model: String,
    pub n: usize,
    pub size: String,
    pub response_format: String,
}

impl Default for AdminGpt2ApiRsImageGenerationRequest {
    fn default() -> Self {
        Self {
            prompt: String::new(),
            model: "gpt-image-2".to_string(),
            n: 1,
            size: "1024x1024".to_string(),
            response_format: "b64_json".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminGpt2ApiRsImageEditRequest {
    pub prompt: String,
    pub model: String,
    pub n: usize,
    pub size: String,
    pub image_base64: String,
    pub file_name: String,
    pub mime_type: String,
}

impl Default for AdminGpt2ApiRsImageEditRequest {
    fn default() -> Self {
        Self {
            prompt: String::new(),
            model: "gpt-image-2".to_string(),
            n: 1,
            size: "1024x1024".to_string(),
            image_base64: String::new(),
            file_name: "image.png".to_string(),
            mime_type: "image/png".to_string(),
        }
    }
}

#[cfg(not(feature = "mock"))]
async fn parse_admin_gpt2api_rs_response<T>(response: gloo_net::http::Response) -> Result<T, String>
where
    T: DeserializeOwned,
{
    if !response.ok() {
        let text = response.text().await.unwrap_or_default();
        return Err(format!("Failed: {text}"));
    }
    // Guard against SPA fallback returning index.html with a 200. Without this
    // check, `response.json()` on the HTML body surfaces as the cryptic
    // "Parse error: SerdeError(expected value)" banner. Surface a clearer
    // message so the operator knows the gpt2api-rs integration is missing on
    // the backend rather than chasing a phantom JSON bug.
    let content_type = response
        .headers()
        .get("content-type")
        .unwrap_or_default()
        .to_lowercase();
    if !content_type.contains("application/json") {
        return Err("gpt2api-rs admin endpoint is not available on this backend (got a non-JSON \
                    response). Ensure the gpt2api-rs integration is enabled on the server build."
            .to_string());
    }
    response
        .json()
        .await
        .map_err(|e| format!("Parse error: {:?}", e))
}

#[cfg(not(feature = "mock"))]
async fn get_admin_gpt2api_rs<T>(path: &str) -> Result<T, String>
where
    T: DeserializeOwned,
{
    let url = format!("{}/admin/gpt2api-rs{path}", admin_base());
    let response = api_get(&url)
        .send()
        .await
        .map_err(|e| format!("Network error: {:?}", e))?;
    parse_admin_gpt2api_rs_response(response).await
}

#[cfg(not(feature = "mock"))]
async fn post_admin_gpt2api_rs<B, T>(path: &str, body: &B) -> Result<T, String>
where
    B: Serialize,
    T: DeserializeOwned,
{
    let url = format!("{}/admin/gpt2api-rs{path}", admin_base());
    let response = api_post(&url)
        .json(body)
        .map_err(|e| format!("Serialize error: {:?}", e))?
        .send()
        .await
        .map_err(|e| format!("Network error: {:?}", e))?;
    parse_admin_gpt2api_rs_response(response).await
}

#[cfg(not(feature = "mock"))]
async fn delete_admin_gpt2api_rs<B, T>(path: &str, body: &B) -> Result<T, String>
where
    B: Serialize,
    T: DeserializeOwned,
{
    let url = format!("{}/admin/gpt2api-rs{path}", admin_base());
    let response = api_delete(&url)
        .json(body)
        .map_err(|e| format!("Serialize error: {:?}", e))?
        .send()
        .await
        .map_err(|e| format!("Network error: {:?}", e))?;
    parse_admin_gpt2api_rs_response(response).await
}

#[cfg(not(feature = "mock"))]
async fn patch_admin_gpt2api_rs<B, T>(path: &str, body: &B) -> Result<T, String>
where
    B: Serialize,
    T: DeserializeOwned,
{
    let url = format!("{}/admin/gpt2api-rs{path}", admin_base());
    let response = api_patch(&url)
        .json(body)
        .map_err(|e| format!("Serialize error: {:?}", e))?
        .send()
        .await
        .map_err(|e| format!("Network error: {:?}", e))?;
    parse_admin_gpt2api_rs_response(response).await
}

#[cfg(not(feature = "mock"))]
async fn delete_admin_gpt2api_rs_empty<T>(path: &str) -> Result<T, String>
where
    T: DeserializeOwned,
{
    let url = format!("{}/admin/gpt2api-rs{path}", admin_base());
    let response = api_delete(&url)
        .send()
        .await
        .map_err(|e| format!("Network error: {:?}", e))?;
    parse_admin_gpt2api_rs_response(response).await
}

pub async fn fetch_admin_gpt2api_rs_config() -> Result<AdminGpt2ApiRsConfigEnvelope, String> {
    #[cfg(feature = "mock")]
    {
        Ok(AdminGpt2ApiRsConfigEnvelope::default())
    }

    #[cfg(not(feature = "mock"))]
    {
        get_admin_gpt2api_rs("/config").await
    }
}

pub async fn update_admin_gpt2api_rs_config(
    config: &Gpt2ApiRsConfig,
) -> Result<AdminGpt2ApiRsConfigEnvelope, String> {
    #[cfg(feature = "mock")]
    {
        Ok(AdminGpt2ApiRsConfigEnvelope {
            config_path: "conf/gpt2api-rs.json".to_string(),
            configured: !config.base_url.trim().is_empty(),
            config: config.clone(),
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        post_admin_gpt2api_rs("/config", config).await
    }
}

pub async fn fetch_admin_gpt2api_rs_status() -> Result<serde_json::Value, String> {
    #[cfg(feature = "mock")]
    {
        Ok(serde_json::json!({ "configured": false }))
    }

    #[cfg(not(feature = "mock"))]
    {
        get_admin_gpt2api_rs("/status").await
    }
}

pub async fn fetch_admin_gpt2api_rs_version() -> Result<serde_json::Value, String> {
    #[cfg(feature = "mock")]
    {
        Ok(serde_json::json!({ "version": "0.1.0" }))
    }

    #[cfg(not(feature = "mock"))]
    {
        get_admin_gpt2api_rs("/version").await
    }
}

pub async fn fetch_admin_gpt2api_rs_models() -> Result<serde_json::Value, String> {
    #[cfg(feature = "mock")]
    {
        Ok(serde_json::json!({ "object": "list", "data": [] }))
    }

    #[cfg(not(feature = "mock"))]
    {
        get_admin_gpt2api_rs("/models").await
    }
}

pub async fn post_admin_gpt2api_rs_login() -> Result<serde_json::Value, String> {
    #[cfg(feature = "mock")]
    {
        Ok(serde_json::json!({ "ok": true, "version": "0.1.0" }))
    }

    #[cfg(not(feature = "mock"))]
    {
        post_admin_gpt2api_rs("/auth/login", &serde_json::json!({})).await
    }
}

pub async fn fetch_admin_gpt2api_rs_accounts() -> Result<Vec<AdminGpt2ApiRsAccountView>, String> {
    #[cfg(feature = "mock")]
    {
        Ok(Vec::new())
    }

    #[cfg(not(feature = "mock"))]
    {
        get_admin_gpt2api_rs("/accounts").await
    }
}

pub async fn fetch_admin_gpt2api_rs_proxy_configs(
) -> Result<Vec<AdminGpt2ApiRsProxyConfigView>, String> {
    #[cfg(feature = "mock")]
    {
        Ok(Vec::new())
    }

    #[cfg(not(feature = "mock"))]
    {
        get_admin_gpt2api_rs("/proxy-configs").await
    }
}

pub async fn create_admin_gpt2api_rs_proxy_config(
    request: &AdminGpt2ApiRsCreateProxyConfigRequest,
) -> Result<AdminGpt2ApiRsProxyConfigView, String> {
    #[cfg(feature = "mock")]
    {
        Ok(AdminGpt2ApiRsProxyConfigView {
            id: "mock-proxy".to_string(),
            name: request.name.clone(),
            proxy_url: request.proxy_url.clone(),
            proxy_username: request.proxy_username.clone(),
            proxy_password: request.proxy_password.clone(),
            status: request
                .status
                .clone()
                .unwrap_or_else(|| "active".to_string()),
            created_at: 0,
            updated_at: 0,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        post_admin_gpt2api_rs("/proxy-configs", request).await
    }
}

pub async fn update_admin_gpt2api_rs_proxy_config(
    proxy_id: &str,
    request: &AdminGpt2ApiRsUpdateProxyConfigRequest,
) -> Result<AdminGpt2ApiRsProxyConfigView, String> {
    #[cfg(feature = "mock")]
    {
        Ok(AdminGpt2ApiRsProxyConfigView {
            id: proxy_id.to_string(),
            name: request
                .name
                .clone()
                .unwrap_or_else(|| "mock-proxy".to_string()),
            proxy_url: request
                .proxy_url
                .clone()
                .unwrap_or_else(|| "http://127.0.0.1:11118".to_string()),
            proxy_username: request.proxy_username.clone().flatten(),
            proxy_password: request.proxy_password.clone().flatten(),
            status: request
                .status
                .clone()
                .unwrap_or_else(|| "active".to_string()),
            created_at: 0,
            updated_at: 0,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        patch_admin_gpt2api_rs(&format!("/proxy-configs/{proxy_id}"), request).await
    }
}

pub async fn delete_admin_gpt2api_rs_proxy_config(
    proxy_id: &str,
) -> Result<serde_json::Value, String> {
    #[cfg(feature = "mock")]
    {
        Ok(serde_json::json!({ "ok": true, "id": proxy_id }))
    }

    #[cfg(not(feature = "mock"))]
    {
        delete_admin_gpt2api_rs_empty(&format!("/proxy-configs/{proxy_id}")).await
    }
}

pub async fn check_admin_gpt2api_rs_proxy_config(
    proxy_id: &str,
) -> Result<AdminGpt2ApiRsProxyCheckResult, String> {
    #[cfg(feature = "mock")]
    {
        Ok(AdminGpt2ApiRsProxyCheckResult {
            ok: true,
            message: format!("proxy {proxy_id} ok"),
            status_code: Some(200),
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        post_admin_gpt2api_rs(&format!("/proxy-configs/{proxy_id}/check"), &serde_json::json!({}))
            .await
    }
}

pub async fn fetch_admin_gpt2api_rs_account_groups(
) -> Result<AdminGpt2ApiRsAccountGroupsResponse, String> {
    #[cfg(feature = "mock")]
    {
        Ok(AdminGpt2ApiRsAccountGroupsResponse::default())
    }

    #[cfg(not(feature = "mock"))]
    {
        get_admin_gpt2api_rs("/account-groups").await
    }
}

pub async fn create_admin_gpt2api_rs_account_group(
    request: &AdminGpt2ApiRsCreateAccountGroupRequest,
) -> Result<AdminGpt2ApiRsAccountGroupView, String> {
    #[cfg(feature = "mock")]
    {
        Ok(AdminGpt2ApiRsAccountGroupView {
            id: "mock-group".to_string(),
            name: request.name.clone(),
            account_names: request.account_names.clone(),
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        post_admin_gpt2api_rs("/account-groups", request).await
    }
}

pub async fn update_admin_gpt2api_rs_account_group(
    group_id: &str,
    request: &AdminGpt2ApiRsUpdateAccountGroupRequest,
) -> Result<AdminGpt2ApiRsAccountGroupView, String> {
    #[cfg(feature = "mock")]
    {
        Ok(AdminGpt2ApiRsAccountGroupView {
            id: group_id.to_string(),
            name: request.name.clone().unwrap_or_else(|| "mock".to_string()),
            account_names: request.account_names.clone().unwrap_or_default(),
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        patch_admin_gpt2api_rs(&format!("/account-groups/{group_id}"), request).await
    }
}

pub async fn delete_admin_gpt2api_rs_account_group(
    group_id: &str,
) -> Result<serde_json::Value, String> {
    #[cfg(feature = "mock")]
    {
        Ok(serde_json::json!({ "deleted": true, "id": group_id }))
    }

    #[cfg(not(feature = "mock"))]
    {
        delete_admin_gpt2api_rs_empty(&format!("/account-groups/{group_id}")).await
    }
}

pub async fn import_admin_gpt2api_rs_accounts(
    request: &AdminGpt2ApiRsImportAccountsRequest,
) -> Result<serde_json::Value, String> {
    #[cfg(feature = "mock")]
    {
        let _ = request;
        Ok(serde_json::json!({ "items": [] }))
    }

    #[cfg(not(feature = "mock"))]
    {
        post_admin_gpt2api_rs("/accounts/import", request).await
    }
}

pub async fn delete_admin_gpt2api_rs_accounts(
    request: &AdminGpt2ApiRsDeleteAccountsRequest,
) -> Result<serde_json::Value, String> {
    #[cfg(feature = "mock")]
    {
        let _ = request;
        Ok(serde_json::json!({ "items": [] }))
    }

    #[cfg(not(feature = "mock"))]
    {
        delete_admin_gpt2api_rs("/accounts", request).await
    }
}

pub async fn refresh_admin_gpt2api_rs_accounts(
    request: &AdminGpt2ApiRsRefreshAccountsRequest,
) -> Result<serde_json::Value, String> {
    #[cfg(feature = "mock")]
    {
        let _ = request;
        Ok(serde_json::json!({ "items": [] }))
    }

    #[cfg(not(feature = "mock"))]
    {
        post_admin_gpt2api_rs("/accounts/refresh", request).await
    }
}

pub async fn update_admin_gpt2api_rs_account(
    request: &AdminGpt2ApiRsUpdateAccountRequest,
) -> Result<serde_json::Value, String> {
    #[cfg(feature = "mock")]
    {
        let _ = request;
        Ok(serde_json::json!({ "item": null }))
    }

    #[cfg(not(feature = "mock"))]
    {
        post_admin_gpt2api_rs("/accounts/update", request).await
    }
}

pub async fn fetch_admin_gpt2api_rs_keys() -> Result<Vec<AdminGpt2ApiRsKeyView>, String> {
    #[cfg(feature = "mock")]
    {
        Ok(Vec::new())
    }

    #[cfg(not(feature = "mock"))]
    {
        get_admin_gpt2api_rs("/keys").await
    }
}

pub async fn fetch_admin_gpt2api_account_contribution_requests(
    query: &AdminGpt2ApiAccountContributionRequestsQuery,
) -> Result<AdminGpt2ApiAccountContributionRequestsResponse, String> {
    #[cfg(feature = "mock")]
    {
        let _ = query;
        Ok(AdminGpt2ApiAccountContributionRequestsResponse {
            total: 0,
            offset: 0,
            limit: 25,
            has_more: false,
            requests: vec![],
            generated_at: 0,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let mut params = Vec::new();
        if let Some(status) = query.status.as_deref() {
            params.push(format!("status={}", urlencoding::encode(status)));
        }
        if let Some(limit) = query.limit {
            params.push(format!("limit={limit}"));
        }
        if let Some(offset) = query.offset {
            params.push(format!("offset={offset}"));
        }
        let suffix =
            if params.is_empty() { String::new() } else { format!("?{}", params.join("&")) };
        get_admin_gpt2api_rs(&format!("/account-contribution-requests{suffix}")).await
    }
}

pub async fn approve_admin_gpt2api_account_contribution_request(
    request_id: &str,
    admin_note: Option<&str>,
) -> Result<AdminGpt2ApiAccountContributionRequestView, String> {
    #[cfg(feature = "mock")]
    {
        let _ = (request_id, admin_note);
        Err("mock not supported".to_string())
    }

    #[cfg(not(feature = "mock"))]
    {
        post_admin_gpt2api_rs(
            &format!("/account-contribution-requests/{}/approve", urlencoding::encode(request_id)),
            &serde_json::json!({ "admin_note": admin_note }),
        )
        .await
    }
}

pub async fn reject_admin_gpt2api_account_contribution_request(
    request_id: &str,
    admin_note: Option<&str>,
) -> Result<AdminGpt2ApiAccountContributionRequestView, String> {
    #[cfg(feature = "mock")]
    {
        let _ = (request_id, admin_note);
        Err("mock not supported".to_string())
    }

    #[cfg(not(feature = "mock"))]
    {
        post_admin_gpt2api_rs(
            &format!("/account-contribution-requests/{}/reject", urlencoding::encode(request_id)),
            &serde_json::json!({ "admin_note": admin_note }),
        )
        .await
    }
}

pub async fn create_admin_gpt2api_rs_key(
    request: &AdminGpt2ApiRsCreateKeyRequest,
) -> Result<AdminGpt2ApiRsKeyView, String> {
    #[cfg(feature = "mock")]
    {
        Ok(AdminGpt2ApiRsKeyView {
            id: "mock-key".to_string(),
            name: request.name.clone(),
            status: request
                .status
                .clone()
                .unwrap_or_else(|| "active".to_string()),
            quota_total_calls: request.quota_total_calls,
            route_strategy: request.route_strategy.clone(),
            role: request.role.clone().unwrap_or_else(|| "user".to_string()),
            secret_plaintext: Some("sk-mock-secret".to_string()),
            ..Default::default()
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        post_admin_gpt2api_rs("/keys", request).await
    }
}

pub async fn update_admin_gpt2api_rs_key(
    key_id: &str,
    request: &AdminGpt2ApiRsUpdateKeyRequest,
) -> Result<AdminGpt2ApiRsKeyView, String> {
    #[cfg(feature = "mock")]
    {
        Ok(AdminGpt2ApiRsKeyView {
            id: key_id.to_string(),
            name: request.name.clone().unwrap_or_default(),
            status: request
                .status
                .clone()
                .unwrap_or_else(|| "active".to_string()),
            quota_total_calls: request.quota_total_calls.unwrap_or_default(),
            route_strategy: request
                .route_strategy
                .clone()
                .unwrap_or_else(|| "auto".to_string()),
            role: request.role.clone().unwrap_or_else(|| "user".to_string()),
            ..Default::default()
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        patch_admin_gpt2api_rs(&format!("/keys/{key_id}"), request).await
    }
}

pub async fn rotate_admin_gpt2api_rs_key(key_id: &str) -> Result<AdminGpt2ApiRsKeyView, String> {
    #[cfg(feature = "mock")]
    {
        Ok(AdminGpt2ApiRsKeyView {
            id: key_id.to_string(),
            secret_plaintext: Some("sk-mock-rotated".to_string()),
            ..Default::default()
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        post_admin_gpt2api_rs(&format!("/keys/{key_id}/rotate"), &serde_json::json!({})).await
    }
}

pub async fn delete_admin_gpt2api_rs_key(key_id: &str) -> Result<serde_json::Value, String> {
    #[cfg(feature = "mock")]
    {
        let _ = key_id;
        Ok(serde_json::json!({ "ok": true }))
    }

    #[cfg(not(feature = "mock"))]
    {
        delete_admin_gpt2api_rs_empty(&format!("/keys/{key_id}")).await
    }
}

pub async fn fetch_admin_gpt2api_rs_usage_events(
    query: &AdminGpt2ApiRsUsageEventsQuery,
) -> Result<AdminGpt2ApiRsUsageEventsResponse, String> {
    #[cfg(feature = "mock")]
    {
        let _ = query;
        Ok(AdminGpt2ApiRsUsageEventsResponse::default())
    }

    #[cfg(not(feature = "mock"))]
    {
        let mut params = Vec::new();
        if let Some(key_id) = query
            .key_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            params.push(format!("key_id={}", urlencoding::encode(key_id)));
        }
        if let Some(q) = query
            .q
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            params.push(format!("q={}", urlencoding::encode(q)));
        }
        if let Some(limit) = query.limit {
            params.push(format!("limit={limit}"));
        }
        if let Some(offset) = query.offset {
            params.push(format!("offset={offset}"));
        }
        let suffix =
            if params.is_empty() { String::new() } else { format!("?{}", params.join("&")) };
        let url = format!("{}/admin/gpt2api-rs/usage/events{}", admin_base(), suffix);
        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        parse_admin_gpt2api_rs_response(response).await
    }
}

pub async fn admin_gpt2api_rs_generate_images(
    request: &AdminGpt2ApiRsImageGenerationRequest,
) -> Result<serde_json::Value, String> {
    #[cfg(feature = "mock")]
    {
        let _ = request;
        Ok(serde_json::json!({ "created": 0, "data": [] }))
    }

    #[cfg(not(feature = "mock"))]
    {
        post_admin_gpt2api_rs("/images/generations", request).await
    }
}

pub async fn admin_gpt2api_rs_edit_images(
    request: &AdminGpt2ApiRsImageEditRequest,
) -> Result<serde_json::Value, String> {
    #[cfg(feature = "mock")]
    {
        let _ = request;
        Ok(serde_json::json!({ "created": 0, "data": [] }))
    }

    #[cfg(not(feature = "mock"))]
    {
        post_admin_gpt2api_rs("/images/edits", request).await
    }
}

pub async fn admin_gpt2api_rs_chat_completions(
    request: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    #[cfg(feature = "mock")]
    {
        let _ = request;
        Ok(serde_json::json!({}))
    }

    #[cfg(not(feature = "mock"))]
    {
        post_admin_gpt2api_rs("/chat/completions", request).await
    }
}

pub async fn admin_gpt2api_rs_responses(
    request: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    #[cfg(feature = "mock")]
    {
        let _ = request;
        Ok(serde_json::json!({}))
    }

    #[cfg(not(feature = "mock"))]
    {
        post_admin_gpt2api_rs("/responses", request).await
    }
}

pub fn build_admin_comment_ai_stream_url(
    task_id: &str,
    run_id: Option<&str>,
    from_batch_index: Option<i32>,
) -> String {
    #[cfg(feature = "mock")]
    {
        let mut url = format!("/mock/admin/comments/tasks/{}/ai-output/stream", task_id);
        let mut params = Vec::new();
        if let Some(run_id) = run_id.map(str::trim).filter(|value| !value.is_empty()) {
            params.push(format!("run_id={}", urlencoding::encode(run_id)));
        }
        if let Some(from_batch_index) = from_batch_index {
            params.push(format!("from_batch_index={from_batch_index}"));
        }
        if !params.is_empty() {
            url.push('?');
            url.push_str(&params.join("&"));
        }
        url
    }

    #[cfg(not(feature = "mock"))]
    {
        let mut url = format!(
            "{}/admin/comments/tasks/{}/ai-output/stream",
            admin_base(),
            urlencoding::encode(task_id)
        );
        let mut params = Vec::new();
        if let Some(run_id) = run_id.map(str::trim).filter(|value| !value.is_empty()) {
            params.push(format!("run_id={}", urlencoding::encode(run_id)));
        }
        if let Some(from_batch_index) = from_batch_index {
            params.push(format!("from_batch_index={from_batch_index}"));
        }
        if !params.is_empty() {
            url.push('?');
            url.push_str(&params.join("&"));
        }
        url
    }
}

pub fn build_comment_client_meta() -> CommentClientMeta {
    #[cfg(feature = "mock")]
    {
        CommentClientMeta {
            ua: Some("mock-agent".to_string()),
            language: Some("zh-CN".to_string()),
            platform: Some("mock".to_string()),
            viewport: Some("1280x720".to_string()),
            timezone: Some("Asia/Shanghai".to_string()),
            referrer: None,
        }
    }

    #[cfg(not(feature = "mock"))]
    {
        let window = web_sys::window();
        let navigator = window.as_ref().map(|win| win.navigator());
        let ua = navigator.as_ref().and_then(|nav| nav.user_agent().ok());
        let language = navigator.as_ref().and_then(|nav| nav.language());
        let platform = navigator.as_ref().and_then(|nav| nav.platform().ok());
        let viewport = window.as_ref().and_then(|win| {
            let width = win.inner_width().ok()?.as_f64()?;
            let height = win.inner_height().ok()?.as_f64()?;
            Some(format!("{:.0}x{:.0}", width, height))
        });
        let timezone = {
            let options = js_sys::Object::new();
            let formatter = js_sys::Intl::DateTimeFormat::new(&js_sys::Array::new(), &options);
            js_sys::Reflect::get(&formatter.resolved_options(), &JsValue::from_str("timeZone"))
                .ok()
                .and_then(|value| value.as_string())
        };
        let referrer = window
            .as_ref()
            .and_then(|win| win.document())
            .map(|doc| doc.referrer())
            .filter(|value| !value.trim().is_empty());

        CommentClientMeta {
            ua,
            language,
            platform,
            viewport,
            timezone,
            referrer,
        }
    }
}

pub async fn submit_article_comment(
    mut request: SubmitCommentRequest,
) -> Result<SubmitCommentResponse, String> {
    if request.comment_text.trim().is_empty() {
        return Err("comment text is empty".to_string());
    }
    if request.client_meta.is_none() {
        request.client_meta = Some(build_comment_client_meta());
    }

    #[cfg(feature = "mock")]
    {
        Ok(SubmitCommentResponse {
            task_id: format!("mock-task-{}", Date::now() as u64),
            status: "pending".to_string(),
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/comments/submit", API_BASE);
        let response = api_post(&url)
            .header("Content-Type", "application/json")
            .json(&request)
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;

        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }

        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn fetch_article_comments(
    article_id: &str,
    limit: Option<usize>,
) -> Result<CommentListResponse, String> {
    if article_id.trim().is_empty() {
        return Err("article_id is empty".to_string());
    }

    #[cfg(feature = "mock")]
    {
        let _ = limit;
        Ok(CommentListResponse {
            comments: vec![],
            total: 0,
            article_id: article_id.to_string(),
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let mut url =
            format!("{}/comments/list?article_id={}", API_BASE, urlencoding::encode(article_id),);
        if let Some(limit) = limit {
            url.push_str(&format!("&limit={limit}"));
        }

        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }

        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn fetch_article_comment_stats(article_id: &str) -> Result<CommentStatsResponse, String> {
    if article_id.trim().is_empty() {
        return Err("article_id is empty".to_string());
    }

    #[cfg(feature = "mock")]
    {
        Ok(CommentStatsResponse {
            article_id: article_id.to_string(),
            total: 0,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url =
            format!("{}/comments/stats?article_id={}", API_BASE, urlencoding::encode(article_id),);
        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }

        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn fetch_admin_view_analytics_config() -> Result<ViewAnalyticsConfig, String> {
    #[cfg(feature = "mock")]
    {
        Ok(ViewAnalyticsConfig {
            dedupe_window_seconds: 60,
            trend_default_days: 30,
            trend_max_days: 180,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/admin/view-analytics-config", admin_base());
        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn update_admin_view_analytics_config(
    config: &ViewAnalyticsConfig,
) -> Result<ViewAnalyticsConfig, String> {
    #[cfg(feature = "mock")]
    {
        Ok(config.clone())
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/admin/view-analytics-config", admin_base());
        let response = api_post(&url)
            .header("Content-Type", "application/json")
            .json(config)
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn fetch_admin_api_behavior_config() -> Result<ApiBehaviorConfig, String> {
    #[cfg(feature = "mock")]
    {
        Ok(ApiBehaviorConfig {
            retention_days: 90,
            default_days: 30,
            max_days: 180,
            flush_batch_size: 256,
            flush_interval_seconds: 15,
            flush_max_buffer_bytes: 4 * 1024 * 1024,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/admin/api-behavior-config", admin_base());
        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn update_admin_api_behavior_config(
    config: &ApiBehaviorConfig,
) -> Result<ApiBehaviorConfig, String> {
    #[cfg(feature = "mock")]
    {
        Ok(config.clone())
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/admin/api-behavior-config", admin_base());
        let response = api_post(&url)
            .header("Content-Type", "application/json")
            .json(config)
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn fetch_admin_compaction_runtime_config() -> Result<CompactionRuntimeConfig, String> {
    #[cfg(feature = "mock")]
    {
        Ok(CompactionRuntimeConfig {
            enabled: true,
            scan_interval_seconds: 900,
            fragment_threshold: 128,
            prune_older_than_hours: 1,
            worker_count: 2,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/admin/compaction-config", admin_base());
        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn update_admin_compaction_runtime_config(
    config: &CompactionRuntimeConfig,
) -> Result<CompactionRuntimeConfig, String> {
    #[cfg(feature = "mock")]
    {
        Ok(config.clone())
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/admin/compaction-config", admin_base());
        let response = api_post(&url)
            .header("Content-Type", "application/json")
            .json(config)
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn fetch_admin_api_behavior_overview(
    days: Option<usize>,
    limit: Option<usize>,
) -> Result<AdminApiBehaviorOverviewResponse, String> {
    #[cfg(feature = "mock")]
    {
        let _ = limit;
        Ok(AdminApiBehaviorOverviewResponse {
            timezone: "Asia/Shanghai".to_string(),
            days: days.unwrap_or(30),
            total_events: 0,
            unique_ips: 0,
            unique_pages: 0,
            avg_latency_ms: 0.0,
            timeseries: vec![],
            top_endpoints: vec![],
            top_pages: vec![],
            device_distribution: vec![],
            browser_distribution: vec![],
            os_distribution: vec![],
            region_distribution: vec![],
            recent_events: vec![],
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let mut url = format!("{}/admin/api-behavior/overview", admin_base());
        let mut params = Vec::new();
        if let Some(days) = days {
            params.push(format!("days={days}"));
        }
        if let Some(limit) = limit {
            params.push(format!("limit={limit}"));
        }
        if !params.is_empty() {
            url.push('?');
            url.push_str(&params.join("&"));
        }

        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn fetch_admin_api_behavior_events(
    query: &AdminApiBehaviorEventsQuery,
) -> Result<AdminApiBehaviorEventsResponse, String> {
    #[cfg(feature = "mock")]
    {
        Ok(AdminApiBehaviorEventsResponse {
            total: 0,
            offset: query.offset.unwrap_or(0),
            limit: query.limit.unwrap_or(100),
            has_more: false,
            events: vec![],
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let mut url = format!("{}/admin/api-behavior/events", admin_base());
        let mut params = Vec::new();
        if let Some(days) = query.days {
            params.push(format!("days={days}"));
        }
        if let Some(value) = query
            .date
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            params.push(format!("date={}", urlencoding::encode(value)));
        }
        if let Some(limit) = query.limit {
            params.push(format!("limit={limit}"));
        }
        if let Some(offset) = query.offset {
            params.push(format!("offset={offset}"));
        }
        if let Some(value) = query
            .path_contains
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            params.push(format!("path_contains={}", urlencoding::encode(value)));
        }
        if let Some(value) = query
            .page_contains
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            params.push(format!("page_contains={}", urlencoding::encode(value)));
        }
        if let Some(value) = query
            .device_type
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            params.push(format!("device_type={}", urlencoding::encode(value)));
        }
        if let Some(value) = query
            .method
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            params.push(format!("method={}", urlencoding::encode(value)));
        }
        if let Some(value) = query.status_code {
            params.push(format!("status_code={value}"));
        }
        if let Some(value) = query.ip.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
            params.push(format!("ip={}", urlencoding::encode(value)));
        }
        if !params.is_empty() {
            url.push('?');
            url.push_str(&params.join("&"));
        }

        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn admin_cleanup_api_behavior(
    request: &AdminApiBehaviorCleanupRequest,
) -> Result<AdminApiBehaviorCleanupResponse, String> {
    #[cfg(feature = "mock")]
    {
        Ok(AdminApiBehaviorCleanupResponse {
            deleted_events: 0,
            before_ms: 0,
            retention_days: request.retention_days.unwrap_or(90),
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/admin/api-behavior/cleanup", admin_base());
        let response = api_post(&url)
            .header("Content-Type", "application/json")
            .json(request)
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn fetch_admin_memory_profiler_overview() -> Result<MemoryProfilerOverview, String> {
    #[cfg(feature = "mock")]
    {
        Ok(MemoryProfilerOverview {
            generated_at_ms: Date::now() as i64,
            config: MemoryProfilerConfigSnapshot {
                enabled: true,
                sample_rate: 128,
                min_alloc_bytes: 256,
                max_tracked_allocations: 100_000,
                stack_skip: 6,
                max_stack_depth: 24,
            },
            process_uptime_secs: 0,
            tracked_allocations: 0,
            distinct_stacks: 0,
            dropped_allocations: 0,
            sampled_alloc_events: 0,
            sampled_dealloc_events: 0,
            sampled_realloc_events: 0,
            total_live_bytes_estimate: 0,
            total_alloc_bytes_estimate: 0,
            process_rss_bytes: 0,
            process_virtual_bytes: 0,
            mimalloc: MiProcessMemoryInfo {
                elapsed_millis: 0,
                user_millis: 0,
                system_millis: 0,
                current_rss_bytes: 0,
                peak_rss_bytes: 0,
                current_commit_bytes: 0,
                peak_commit_bytes: 0,
                page_faults: 0,
            },
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/admin/runtime/memory/overview", admin_base());
        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn fetch_admin_memory_profiler_stacks(
    top: Option<usize>,
) -> Result<MemoryStackReport, String> {
    #[cfg(feature = "mock")]
    {
        Ok(MemoryStackReport {
            generated_at_ms: Date::now() as i64,
            top: top.unwrap_or(20),
            total_live_bytes_estimate: 0,
            process_rss_bytes: 0,
            entries: vec![],
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let mut url = format!("{}/admin/runtime/memory/stacks", admin_base());
        if let Some(top) = top {
            url.push_str(&format!("?top={top}"));
        }
        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn fetch_admin_memory_profiler_functions(
    top: Option<usize>,
) -> Result<MemoryFunctionReport, String> {
    #[cfg(feature = "mock")]
    {
        Ok(MemoryFunctionReport {
            generated_at_ms: Date::now() as i64,
            top: top.unwrap_or(20),
            total_live_bytes_estimate: 0,
            process_rss_bytes: 0,
            entries: vec![],
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let mut url = format!("{}/admin/runtime/memory/functions", admin_base());
        if let Some(top) = top {
            url.push_str(&format!("?top={top}"));
        }
        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn fetch_admin_memory_profiler_modules(
    top: Option<usize>,
) -> Result<MemoryModuleReport, String> {
    #[cfg(feature = "mock")]
    {
        Ok(MemoryModuleReport {
            generated_at_ms: Date::now() as i64,
            top: top.unwrap_or(20),
            total_live_bytes_estimate: 0,
            process_rss_bytes: 0,
            entries: vec![],
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let mut url = format!("{}/admin/runtime/memory/modules", admin_base());
        if let Some(top) = top {
            url.push_str(&format!("?top={top}"));
        }
        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn admin_reset_memory_profiler() -> Result<(), String> {
    #[cfg(feature = "mock")]
    {
        Ok(())
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/admin/runtime/memory/reset", admin_base());
        let response = api_post(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        Ok(())
    }
}

pub async fn admin_update_memory_profiler_config(
    config: &MemoryProfilerConfigUpdate,
) -> Result<MemoryProfilerConfigSnapshot, String> {
    #[cfg(feature = "mock")]
    {
        Ok(MemoryProfilerConfigSnapshot {
            enabled: config.enabled.unwrap_or(true),
            sample_rate: config.sample_rate.unwrap_or(128),
            min_alloc_bytes: config.min_alloc_bytes.unwrap_or(256),
            max_tracked_allocations: config.max_tracked_allocations.unwrap_or(100_000),
            stack_skip: 6,
            max_stack_depth: 24,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/admin/runtime/memory/config", admin_base());
        let response = api_post(&url)
            .header("Content-Type", "application/json")
            .json(config)
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn fetch_admin_comment_runtime_config() -> Result<CommentRuntimeConfig, String> {
    #[cfg(feature = "mock")]
    {
        Ok(CommentRuntimeConfig {
            submit_rate_limit_seconds: 60,
            list_default_limit: 20,
            cleanup_retention_days: -1,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/admin/comment-config", admin_base());
        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn update_admin_comment_runtime_config(
    config: &CommentRuntimeConfig,
) -> Result<CommentRuntimeConfig, String> {
    #[cfg(feature = "mock")]
    {
        Ok(config.clone())
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/admin/comment-config", admin_base());
        let response = api_post(&url)
            .header("Content-Type", "application/json")
            .json(config)
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn fetch_admin_comment_tasks_grouped(
    status: Option<&str>,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<AdminCommentTaskGroupedResponse, String> {
    #[cfg(feature = "mock")]
    {
        let _ = (status, limit, offset);
        Ok(AdminCommentTaskGroupedResponse {
            groups: vec![],
            total_tasks: 0,
            total_articles: 0,
            status_counts: std::collections::HashMap::new(),
            offset: 0,
            has_more: false,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let mut url = format!("{}/admin/comments/tasks/grouped", admin_base());
        let mut params = Vec::new();
        if let Some(status) = status.map(str::trim).filter(|value| !value.is_empty()) {
            params.push(format!("status={}", urlencoding::encode(status)));
        }
        if let Some(limit) = limit {
            params.push(format!("limit={limit}"));
        }
        if let Some(offset) = offset {
            params.push(format!("offset={offset}"));
        }
        if !params.is_empty() {
            url.push('?');
            url.push_str(&params.join("&"));
        }

        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn fetch_admin_comment_task(task_id: &str) -> Result<AdminCommentTask, String> {
    #[cfg(feature = "mock")]
    {
        let _ = task_id;
        Err("not found".to_string())
    }

    #[cfg(not(feature = "mock"))]
    {
        let url =
            format!("{}/admin/comments/tasks/{}", admin_base(), urlencoding::encode(task_id),);
        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn fetch_admin_comment_task_ai_output(
    task_id: &str,
    run_id: Option<&str>,
    limit: Option<usize>,
) -> Result<AdminCommentTaskAiOutputResponse, String> {
    #[cfg(feature = "mock")]
    {
        let _ = (run_id, limit);
        Ok(AdminCommentTaskAiOutputResponse {
            task_id: task_id.to_string(),
            selected_run_id: None,
            runs: vec![],
            chunks: vec![],
            merged_stdout: String::new(),
            merged_stderr: String::new(),
            merged_output: String::new(),
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let mut url = format!(
            "{}/admin/comments/tasks/{}/ai-output",
            admin_base(),
            urlencoding::encode(task_id),
        );
        let mut params = Vec::new();
        if let Some(run_id) = run_id.map(str::trim).filter(|value| !value.is_empty()) {
            params.push(format!("run_id={}", urlencoding::encode(run_id)));
        }
        if let Some(limit) = limit {
            params.push(format!("limit={limit}"));
        }
        if !params.is_empty() {
            url.push('?');
            url.push_str(&params.join("&"));
        }

        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn patch_admin_comment_task(
    task_id: &str,
    request: &AdminPatchCommentTaskRequest,
) -> Result<AdminCommentTask, String> {
    #[cfg(feature = "mock")]
    {
        let _ = (task_id, request);
        Err("not implemented in mock".to_string())
    }

    #[cfg(not(feature = "mock"))]
    {
        let url =
            format!("{}/admin/comments/tasks/{}", admin_base(), urlencoding::encode(task_id),);
        let response = api_patch(&url)
            .header("Content-Type", "application/json")
            .json(request)
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn admin_approve_comment_task(
    task_id: &str,
    request: &AdminTaskActionRequest,
) -> Result<AdminCommentTask, String> {
    admin_post_task_action(task_id, "approve", request).await
}

pub async fn admin_reject_comment_task(
    task_id: &str,
    request: &AdminTaskActionRequest,
) -> Result<AdminCommentTask, String> {
    admin_post_task_action(task_id, "reject", request).await
}

pub async fn admin_retry_comment_task(
    task_id: &str,
    request: &AdminTaskActionRequest,
) -> Result<AdminCommentTask, String> {
    admin_post_task_action(task_id, "retry", request).await
}

pub async fn admin_approve_and_run_comment_task(
    task_id: &str,
    request: &AdminTaskActionRequest,
) -> Result<AdminCommentTask, String> {
    admin_post_task_action(task_id, "approve-and-run", request).await
}

pub async fn admin_delete_comment_task(
    task_id: &str,
    request: &AdminTaskActionRequest,
) -> Result<serde_json::Value, String> {
    #[cfg(feature = "mock")]
    {
        let _ = request;
        Ok(serde_json::json!({ "task_id": task_id, "deleted": true }))
    }

    #[cfg(not(feature = "mock"))]
    {
        let url =
            format!("{}/admin/comments/tasks/{}", admin_base(), urlencoding::encode(task_id),);
        let response = api_delete(&url)
            .header("Content-Type", "application/json")
            .json(request)
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn fetch_admin_published_comments(
    article_id: Option<&str>,
    task_id: Option<&str>,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<AdminCommentPublishedResponse, String> {
    #[cfg(feature = "mock")]
    {
        let _ = (article_id, task_id, limit, offset);
        Ok(AdminCommentPublishedResponse {
            comments: vec![],
            total: 0,
            offset: 0,
            has_more: false,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let mut url = format!("{}/admin/comments/published", admin_base());
        let mut params = Vec::new();
        if let Some(article_id) = article_id.map(str::trim).filter(|value| !value.is_empty()) {
            params.push(format!("article_id={}", urlencoding::encode(article_id)));
        }
        if let Some(task_id) = task_id.map(str::trim).filter(|value| !value.is_empty()) {
            params.push(format!("task_id={}", urlencoding::encode(task_id)));
        }
        if let Some(limit) = limit {
            params.push(format!("limit={limit}"));
        }
        if let Some(offset) = offset {
            params.push(format!("offset={offset}"));
        }
        if !params.is_empty() {
            url.push('?');
            url.push_str(&params.join("&"));
        }

        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn patch_admin_published_comment(
    comment_id: &str,
    request: &AdminPatchPublishedCommentRequest,
) -> Result<ArticleComment, String> {
    #[cfg(feature = "mock")]
    {
        let _ = (comment_id, request);
        Err("not implemented in mock".to_string())
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!(
            "{}/admin/comments/published/{}",
            admin_base(),
            urlencoding::encode(comment_id),
        );
        let response = api_patch(&url)
            .header("Content-Type", "application/json")
            .json(request)
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn delete_admin_published_comment(
    comment_id: &str,
    request: &AdminTaskActionRequest,
) -> Result<serde_json::Value, String> {
    #[cfg(feature = "mock")]
    {
        let _ = request;
        Ok(serde_json::json!({ "comment_id": comment_id, "deleted": true }))
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!(
            "{}/admin/comments/published/{}",
            admin_base(),
            urlencoding::encode(comment_id),
        );
        let response = api_delete(&url)
            .header("Content-Type", "application/json")
            .json(request)
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn fetch_admin_comment_audit_logs(
    task_id: Option<&str>,
    action: Option<&str>,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<AdminCommentAuditResponse, String> {
    #[cfg(feature = "mock")]
    {
        let _ = (task_id, action, limit, offset);
        Ok(AdminCommentAuditResponse {
            logs: vec![],
            total: 0,
            offset: 0,
            has_more: false,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let mut url = format!("{}/admin/comments/audit-logs", admin_base());
        let mut params = Vec::new();
        if let Some(task_id) = task_id.map(str::trim).filter(|value| !value.is_empty()) {
            params.push(format!("task_id={}", urlencoding::encode(task_id)));
        }
        if let Some(action) = action.map(str::trim).filter(|value| !value.is_empty()) {
            params.push(format!("action={}", urlencoding::encode(action)));
        }
        if let Some(limit) = limit {
            params.push(format!("limit={limit}"));
        }
        if let Some(offset) = offset {
            params.push(format!("offset={offset}"));
        }
        if !params.is_empty() {
            url.push('?');
            url.push_str(&params.join("&"));
        }

        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn admin_cleanup_comments(
    request: &AdminCleanupRequest,
) -> Result<AdminCleanupResponse, String> {
    #[cfg(feature = "mock")]
    {
        let _ = request;
        Ok(AdminCleanupResponse {
            deleted_tasks: 0,
            before_ms: None,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/admin/comments/cleanup", admin_base());
        let response = api_post(&url)
            .header("Content-Type", "application/json")
            .json(request)
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

async fn admin_post_task_action(
    task_id: &str,
    action: &str,
    request: &AdminTaskActionRequest,
) -> Result<AdminCommentTask, String> {
    #[cfg(feature = "mock")]
    {
        let _ = (task_id, request);
        Err(format!("mock action not implemented: {}", action))
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!(
            "{}/admin/comments/tasks/{}/{}",
            admin_base(),
            urlencoding::encode(task_id),
            action
        );
        let response = api_post(&url)
            .header("Content-Type", "application/json")
            .json(request)
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

// ---------------------------------------------------------------------------
// Music API types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SongListItem {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub cover_image: Option<String>,
    pub duration_ms: u64,
    pub format: String,
    pub tags: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SongDetail {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub cover_image: Option<String>,
    pub duration_ms: u64,
    pub format: String,
    pub bitrate: u64,
    pub tags: String,
    pub source: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SongLyrics {
    pub song_id: String,
    pub lyrics_lrc: Option<String>,
    pub lyrics_translation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MusicCommentItem {
    pub id: String,
    pub song_id: String,
    pub nickname: String,
    pub comment_text: String,
    pub ip_region: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayTrackResponse {
    pub song_id: String,
    pub counted: bool,
    pub total_plays: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SongSearchResult {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub cover_image: Option<String>,
    pub score: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SongListResponse {
    pub songs: Vec<SongListItem>,
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
    pub has_more: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextSongResolveMode {
    Random,
    Semantic,
}

#[cfg(not(feature = "mock"))]
#[derive(Debug, Deserialize)]
struct NextSongApiResponse {
    song: Option<SongDetail>,
}

#[cfg(not(feature = "mock"))]
#[derive(Debug, Deserialize)]
struct MusicCommentListApiResponse {
    comments: Vec<MusicCommentItem>,
    #[allow(
        dead_code,
        reason = "The music comments UI currently renders the returned slice only, but total \
                  remains part of the stable backend payload."
    )]
    total: usize,
}

pub fn song_audio_url(id: &str) -> String {
    #[cfg(feature = "mock")]
    {
        format!("/mock/music/{}/audio", id)
    }
    #[cfg(not(feature = "mock"))]
    {
        format!("{}/music/{}/audio", API_BASE, urlencoding::encode(id))
    }
}

pub fn song_cover_url(cover: Option<&str>) -> String {
    match cover {
        Some(f) if !f.is_empty() => {
            // If it's already a full URL, use directly
            if f.starts_with("http://") || f.starts_with("https://") {
                return f.to_string();
            }
            #[cfg(feature = "mock")]
            {
                format!("/mock/images/{}", f)
            }
            #[cfg(not(feature = "mock"))]
            {
                format!("{}/images/{}", API_BASE, urlencoding::encode(f))
            }
        },
        _ => String::new(),
    }
}

pub async fn fetch_songs(
    limit: Option<usize>,
    offset: Option<usize>,
    artist: Option<&str>,
    album: Option<&str>,
    sort: Option<&str>,
) -> Result<SongListResponse, String> {
    #[cfg(feature = "mock")]
    {
        let _ = (limit, offset, artist, album, sort);
        Ok(SongListResponse {
            songs: vec![],
            total: 0,
            offset: 0,
            limit: 20,
            has_more: false,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let mut url = format!("{}/music", API_BASE);
        let mut params = Vec::new();
        if let Some(l) = limit {
            params.push(format!("limit={l}"));
        }
        if let Some(o) = offset {
            params.push(format!("offset={o}"));
        }
        if let Some(a) = artist {
            params.push(format!("artist={}", urlencoding::encode(a)));
        }
        if let Some(a) = album {
            params.push(format!("album={}", urlencoding::encode(a)));
        }
        if let Some(s) = sort {
            params.push(format!("sort={}", urlencoding::encode(s)));
        }
        if !params.is_empty() {
            url.push('?');
            url.push_str(&params.join("&"));
        }
        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        let r: SongListResponse = response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))?;
        Ok(r)
    }
}

pub async fn fetch_random_recommended_songs(
    limit: Option<usize>,
    exclude_ids: &[String],
) -> Result<Vec<SongListItem>, String> {
    #[cfg(feature = "mock")]
    {
        let _ = (limit, exclude_ids);
        Ok(vec![])
    }

    #[cfg(not(feature = "mock"))]
    {
        let mut url = format!("{}/music/recommendations/random", API_BASE);
        let mut params = Vec::new();
        if let Some(l) = limit {
            params.push(format!("limit={l}"));
        }

        let mut normalized_exclude = Vec::new();
        for id in exclude_ids {
            let trimmed = id.trim();
            if trimmed.is_empty() {
                continue;
            }
            normalized_exclude.push(trimmed.to_string());
            if normalized_exclude.len() >= 10 {
                break;
            }
        }
        if !normalized_exclude.is_empty() {
            params.push(format!(
                "exclude_ids={}",
                urlencoding::encode(&normalized_exclude.join(","))
            ));
        }

        if !params.is_empty() {
            url.push('?');
            url.push_str(&params.join("&"));
        }

        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn fetch_next_song(
    mode: NextSongResolveMode,
    current_song_id: Option<&str>,
    recent_song_ids: &[String],
) -> Result<Option<SongDetail>, String> {
    #[cfg(feature = "mock")]
    {
        let _ = (mode, current_song_id, recent_song_ids);
        Ok(None)
    }

    #[cfg(not(feature = "mock"))]
    {
        let mut normalized_recent = Vec::new();
        for id in recent_song_ids {
            let trimmed = id.trim();
            if trimmed.is_empty() {
                continue;
            }
            normalized_recent.push(trimmed.to_string());
            if normalized_recent.len() >= 10 {
                break;
            }
        }

        let current = current_song_id
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(|id| id.to_string());

        let body = serde_json::json!({
            "mode": match mode {
                NextSongResolveMode::Random => "random",
                NextSongResolveMode::Semantic => "semantic",
            },
            "current_song_id": current,
            "recent_song_ids": normalized_recent,
        });
        let url = format!("{}/music/next", API_BASE);
        let response = api_post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        let parsed: NextSongApiResponse = response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))?;
        Ok(parsed.song)
    }
}

pub async fn search_songs(
    q: &str,
    limit: Option<usize>,
    mode: Option<&str>,
) -> Result<Vec<SongSearchResult>, String> {
    #[cfg(feature = "mock")]
    {
        let _ = (q, limit, mode);
        Ok(vec![])
    }

    #[cfg(not(feature = "mock"))]
    {
        let mut url = format!("{}/music/search?q={}", API_BASE, urlencoding::encode(q));
        if let Some(l) = limit {
            url.push_str(&format!("&limit={l}"));
        }
        if let Some(m) = mode {
            url.push_str(&format!("&mode={}", urlencoding::encode(m)));
        }
        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        let r: Vec<SongSearchResult> = response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))?;
        Ok(r)
    }
}

pub async fn fetch_song_detail(id: &str) -> Result<Option<SongDetail>, String> {
    #[cfg(feature = "mock")]
    {
        let _ = id;
        Ok(None)
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/music/{}", API_BASE, urlencoding::encode(id));
        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if response.status() == 404 {
            return Ok(None);
        }
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        let d: SongDetail = response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))?;
        Ok(Some(d))
    }
}

pub async fn fetch_song_lyrics(id: &str) -> Result<Option<SongLyrics>, String> {
    #[cfg(feature = "mock")]
    {
        let _ = id;
        Ok(None)
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/music/{}/lyrics", API_BASE, urlencoding::encode(id));
        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if response.status() == 404 {
            return Ok(None);
        }
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        let l: SongLyrics = response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))?;
        Ok(Some(l))
    }
}

pub async fn fetch_related_songs(id: &str) -> Result<Vec<SongSearchResult>, String> {
    #[cfg(feature = "mock")]
    {
        let _ = id;
        Ok(vec![])
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/music/{}/related", API_BASE, urlencoding::encode(id));
        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn track_song_play(id: &str) -> Result<PlayTrackResponse, String> {
    #[cfg(feature = "mock")]
    {
        Ok(PlayTrackResponse {
            song_id: id.to_string(),
            counted: true,
            total_plays: 42,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/music/{}/play", API_BASE, urlencoding::encode(id));
        let response = api_post(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn submit_music_comment(
    song_id: &str,
    nickname: Option<&str>,
    text: &str,
) -> Result<MusicCommentItem, String> {
    #[cfg(feature = "mock")]
    {
        Ok(MusicCommentItem {
            id: "mock".to_string(),
            song_id: song_id.to_string(),
            nickname: nickname.unwrap_or("Reader").to_string(),
            comment_text: text.to_string(),
            ip_region: None,
            created_at: 0,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/music/comments/submit", API_BASE);
        let mut body = serde_json::json!({
            "song_id": song_id,
            "comment_text": text
        });
        if let Some(value) = nickname {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                body["nickname"] = serde_json::Value::String(trimmed.to_string());
            }
        }
        let response = api_post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn fetch_music_comments(
    song_id: &str,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<Vec<MusicCommentItem>, String> {
    #[cfg(feature = "mock")]
    {
        let _ = (song_id, limit, offset);
        Ok(vec![])
    }

    #[cfg(not(feature = "mock"))]
    {
        let mut url =
            format!("{}/music/comments/list?song_id={}", API_BASE, urlencoding::encode(song_id));
        if let Some(l) = limit {
            url.push_str(&format!("&limit={l}"));
        }
        if let Some(o) = offset {
            url.push_str(&format!("&offset={o}"));
        }
        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        let r: MusicCommentListApiResponse = response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))?;
        Ok(r.comments)
    }
}

// Music Wish types

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MusicWishItem {
    pub wish_id: String,
    pub song_name: String,
    pub artist_hint: Option<String>,
    pub wish_message: String,
    pub nickname: String,
    pub status: String,
    pub ip_region: String,
    pub ingested_song_id: Option<String>,
    pub ai_reply: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub admin_note: Option<String>,
    pub failure_reason: Option<String>,
    pub attempt_count: i32,
    pub fingerprint: String,
    pub client_ip: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MusicWishListResponse {
    pub wishes: Vec<MusicWishItem>,
    #[serde(default)]
    pub total: usize,
    #[serde(default)]
    pub offset: usize,
    #[serde(default)]
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AdminMusicWishListResponse {
    pub wishes: Vec<MusicWishItem>,
    pub total: usize,
    #[serde(default)]
    pub offset: usize,
    #[serde(default)]
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitMusicWishResponse {
    pub wish_id: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MusicWishAiRunRecord {
    pub run_id: String,
    pub wish_id: String,
    pub status: String,
    pub runner_program: String,
    pub exit_code: Option<i32>,
    pub final_reply_markdown: Option<String>,
    pub failure_reason: Option<String>,
    pub started_at: i64,
    pub updated_at: i64,
    pub completed_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MusicWishAiRunChunk {
    pub chunk_id: String,
    pub run_id: String,
    pub wish_id: String,
    pub stream: String,
    pub batch_index: i32,
    pub content: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminMusicWishAiOutputResponse {
    pub runs: Vec<MusicWishAiRunRecord>,
    pub chunks: Vec<MusicWishAiRunChunk>,
}

pub async fn submit_music_wish(
    song_name: &str,
    artist_hint: Option<&str>,
    wish_message: &str,
    nickname: Option<&str>,
    requester_email: Option<&str>,
    frontend_page_url: Option<&str>,
) -> Result<SubmitMusicWishResponse, String> {
    #[cfg(feature = "mock")]
    {
        let _ =
            (song_name, artist_hint, wish_message, nickname, requester_email, frontend_page_url);
        Ok(SubmitMusicWishResponse {
            wish_id: "mock-wish-1".to_string(),
            status: "pending".to_string(),
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/music/wishes/submit", API_BASE);
        let mut body = serde_json::json!({
            "song_name": song_name,
            "wish_message": wish_message,
        });
        if let Some(value) = nickname {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                body["nickname"] = serde_json::Value::String(trimmed.to_string());
            }
        }
        if let Some(hint) = artist_hint {
            body["artist_hint"] = serde_json::Value::String(hint.to_string());
        }
        if let Some(email) = requester_email {
            body["requester_email"] = serde_json::Value::String(email.to_string());
        }
        if let Some(page_url) = frontend_page_url {
            body["frontend_page_url"] = serde_json::Value::String(page_url.to_string());
        }
        let response = api_post(&url)
            .json(&body)
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let status = response.status();
            let text = response
                .text()
                .await
                .map_err(|e| format!("Read error: {:?}", e))?;
            return Err(format!("HTTP {}: {}", status, text));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn fetch_music_wishes(
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<MusicWishListResponse, String> {
    #[cfg(feature = "mock")]
    {
        let _ = (limit, offset);
        Ok(MusicWishListResponse {
            wishes: vec![],
            total: 0,
            offset: 0,
            has_more: false,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let mut url = format!("{}/music/wishes/list", API_BASE);
        let mut params = Vec::new();
        if let Some(l) = limit {
            params.push(format!("limit={l}"));
        }
        if let Some(o) = offset {
            params.push(format!("offset={o}"));
        }
        if !params.is_empty() {
            url.push('?');
            url.push_str(&params.join("&"));
        }
        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        let r: MusicWishListResponse = response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))?;
        Ok(r)
    }
}

pub async fn fetch_admin_music_wishes(
    status: Option<&str>,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<AdminMusicWishListResponse, String> {
    #[cfg(feature = "mock")]
    {
        let _ = (status, limit, offset);
        Ok(AdminMusicWishListResponse {
            wishes: vec![],
            total: 0,
            offset: 0,
            has_more: false,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let base = admin_base();
        let mut url = format!("{base}/admin/music-wishes/tasks?");
        if let Some(s) = status {
            url.push_str(&format!("status={}&", urlencoding::encode(s)));
        }
        if let Some(l) = limit {
            url.push_str(&format!("limit={l}&"));
        }
        if let Some(o) = offset {
            url.push_str(&format!("offset={o}&"));
        }
        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn admin_approve_and_run_music_wish(
    wish_id: &str,
    admin_note: Option<&str>,
) -> Result<MusicWishItem, String> {
    #[cfg(feature = "mock")]
    {
        let _ = (wish_id, admin_note);
        Err("mock not supported".to_string())
    }

    #[cfg(not(feature = "mock"))]
    {
        let base = admin_base();
        let url = format!(
            "{base}/admin/music-wishes/tasks/{}/approve-and-run",
            urlencoding::encode(wish_id)
        );
        let body = serde_json::json!({ "admin_note": admin_note });
        let response = api_post(&url)
            .json(&body)
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response
                .text()
                .await
                .map_err(|e| format!("Read error: {:?}", e))?;
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn admin_reject_music_wish(
    wish_id: &str,
    admin_note: Option<&str>,
) -> Result<MusicWishItem, String> {
    #[cfg(feature = "mock")]
    {
        let _ = (wish_id, admin_note);
        Err("mock not supported".to_string())
    }

    #[cfg(not(feature = "mock"))]
    {
        let base = admin_base();
        let url =
            format!("{base}/admin/music-wishes/tasks/{}/reject", urlencoding::encode(wish_id));
        let body = serde_json::json!({ "admin_note": admin_note });
        let response = api_post(&url)
            .json(&body)
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response
                .text()
                .await
                .map_err(|e| format!("Read error: {:?}", e))?;
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn admin_retry_music_wish(wish_id: &str) -> Result<MusicWishItem, String> {
    #[cfg(feature = "mock")]
    {
        let _ = wish_id;
        Err("mock not supported".to_string())
    }

    #[cfg(not(feature = "mock"))]
    {
        let base = admin_base();
        let url = format!("{base}/admin/music-wishes/tasks/{}/retry", urlencoding::encode(wish_id));
        let response = api_post(&url)
            .json(&serde_json::json!({}))
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response
                .text()
                .await
                .map_err(|e| format!("Read error: {:?}", e))?;
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn admin_delete_music_wish(wish_id: &str) -> Result<(), String> {
    #[cfg(feature = "mock")]
    {
        let _ = wish_id;
        Err("mock not supported".to_string())
    }

    #[cfg(not(feature = "mock"))]
    {
        let base = admin_base();
        let url = format!("{base}/admin/music-wishes/tasks/{}", urlencoding::encode(wish_id));
        let response = gloo_net::http::Request::delete(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        Ok(())
    }
}

pub async fn fetch_admin_music_wish_ai_output(
    wish_id: &str,
) -> Result<AdminMusicWishAiOutputResponse, String> {
    #[cfg(feature = "mock")]
    {
        let _ = wish_id;
        Ok(AdminMusicWishAiOutputResponse {
            runs: vec![],
            chunks: vec![],
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let base = admin_base();
        let url =
            format!("{base}/admin/music-wishes/tasks/{}/ai-output", urlencoding::encode(wish_id));
        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub fn build_admin_music_wish_ai_stream_url(wish_id: &str) -> String {
    #[cfg(feature = "mock")]
    {
        format!("/mock/admin/music-wishes/tasks/{}/ai-output/stream", wish_id)
    }

    #[cfg(not(feature = "mock"))]
    {
        let base = admin_base();
        format!("{base}/admin/music-wishes/tasks/{}/ai-output/stream", urlencoding::encode(wish_id))
    }
}

// Article Request types

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArticleRequestItem {
    pub request_id: String,
    pub article_url: String,
    pub title_hint: Option<String>,
    pub request_message: String,
    pub nickname: String,
    pub status: String,
    pub ip_region: String,
    pub ingested_article_id: Option<String>,
    pub ai_reply: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub admin_note: Option<String>,
    pub failure_reason: Option<String>,
    pub attempt_count: i32,
    pub fingerprint: String,
    pub client_ip: String,
    #[serde(default)]
    pub parent_request_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArticleRequestListResponse {
    pub requests: Vec<ArticleRequestItem>,
    #[serde(default)]
    pub total: usize,
    #[serde(default)]
    pub offset: usize,
    #[serde(default)]
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AdminArticleRequestListResponse {
    pub requests: Vec<ArticleRequestItem>,
    pub total: usize,
    #[serde(default)]
    pub offset: usize,
    #[serde(default)]
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitArticleRequestResponse {
    pub request_id: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArticleRequestAiRunRecord {
    pub run_id: String,
    pub request_id: String,
    pub status: String,
    pub runner_program: String,
    pub exit_code: Option<i32>,
    pub final_reply_markdown: Option<String>,
    pub failure_reason: Option<String>,
    pub started_at: i64,
    pub updated_at: i64,
    pub completed_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArticleRequestAiRunChunk {
    pub chunk_id: String,
    pub run_id: String,
    pub request_id: String,
    pub stream: String,
    pub batch_index: i32,
    pub content: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminArticleRequestAiOutputResponse {
    pub runs: Vec<ArticleRequestAiRunRecord>,
    pub chunks: Vec<ArticleRequestAiRunChunk>,
}

pub async fn submit_article_request(
    article_url: &str,
    title_hint: Option<&str>,
    request_message: &str,
    nickname: Option<&str>,
    requester_email: Option<&str>,
    frontend_page_url: Option<&str>,
    parent_request_id: Option<&str>,
) -> Result<SubmitArticleRequestResponse, String> {
    #[cfg(feature = "mock")]
    {
        let _ = (
            article_url,
            title_hint,
            request_message,
            nickname,
            requester_email,
            frontend_page_url,
            parent_request_id,
        );
        Ok(SubmitArticleRequestResponse {
            request_id: "mock-ar-1".to_string(),
            status: "pending".to_string(),
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/article-requests/submit", API_BASE);
        let mut body = serde_json::json!({
            "article_url": article_url,
            "request_message": request_message,
        });
        if let Some(value) = nickname {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                body["nickname"] = serde_json::Value::String(trimmed.to_string());
            }
        }
        if let Some(hint) = title_hint {
            let trimmed = hint.trim();
            if !trimmed.is_empty() {
                body["title_hint"] = serde_json::Value::String(trimmed.to_string());
            }
        }
        if let Some(email) = requester_email {
            body["requester_email"] = serde_json::Value::String(email.to_string());
        }
        if let Some(page_url) = frontend_page_url {
            body["frontend_page_url"] = serde_json::Value::String(page_url.to_string());
        }
        if let Some(pid) = parent_request_id {
            body["parent_request_id"] = serde_json::Value::String(pid.to_string());
        }
        let response = api_post(&url)
            .json(&body)
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let status = response.status();
            let text = response
                .text()
                .await
                .map_err(|e| format!("Read error: {:?}", e))?;
            return Err(format!("HTTP {}: {}", status, text));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn fetch_article_requests(
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<ArticleRequestListResponse, String> {
    #[cfg(feature = "mock")]
    {
        let _ = (limit, offset);
        Ok(ArticleRequestListResponse {
            requests: vec![],
            total: 0,
            offset: 0,
            has_more: false,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let mut url = format!("{}/article-requests/list", API_BASE);
        let mut params = Vec::new();
        if let Some(l) = limit {
            params.push(format!("limit={l}"));
        }
        if let Some(o) = offset {
            params.push(format!("offset={o}"));
        }
        if !params.is_empty() {
            url.push('?');
            url.push_str(&params.join("&"));
        }
        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        let r: ArticleRequestListResponse = response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))?;
        Ok(r)
    }
}

pub async fn fetch_admin_article_requests(
    status: Option<&str>,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<AdminArticleRequestListResponse, String> {
    #[cfg(feature = "mock")]
    {
        let _ = (status, limit, offset);
        Ok(AdminArticleRequestListResponse {
            requests: vec![],
            total: 0,
            offset: 0,
            has_more: false,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let base = admin_base();
        let mut url = format!("{base}/admin/article-requests/tasks?");
        if let Some(s) = status {
            url.push_str(&format!("status={}&", urlencoding::encode(s)));
        }
        if let Some(l) = limit {
            url.push_str(&format!("limit={l}&"));
        }
        if let Some(o) = offset {
            url.push_str(&format!("offset={o}&"));
        }
        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn admin_approve_and_run_article_request(
    request_id: &str,
    admin_note: Option<&str>,
) -> Result<ArticleRequestItem, String> {
    #[cfg(feature = "mock")]
    {
        let _ = (request_id, admin_note);
        Err("mock not supported".to_string())
    }

    #[cfg(not(feature = "mock"))]
    {
        let base = admin_base();
        let url = format!(
            "{base}/admin/article-requests/tasks/{}/approve-and-run",
            urlencoding::encode(request_id)
        );
        let body = serde_json::json!({ "admin_note": admin_note });
        let response = api_post(&url)
            .json(&body)
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response
                .text()
                .await
                .map_err(|e| format!("Read error: {:?}", e))?;
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn admin_reject_article_request(
    request_id: &str,
    admin_note: Option<&str>,
) -> Result<ArticleRequestItem, String> {
    #[cfg(feature = "mock")]
    {
        let _ = (request_id, admin_note);
        Err("mock not supported".to_string())
    }

    #[cfg(not(feature = "mock"))]
    {
        let base = admin_base();
        let url = format!(
            "{base}/admin/article-requests/tasks/{}/reject",
            urlencoding::encode(request_id)
        );
        let body = serde_json::json!({ "admin_note": admin_note });
        let response = api_post(&url)
            .json(&body)
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response
                .text()
                .await
                .map_err(|e| format!("Read error: {:?}", e))?;
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn admin_retry_article_request(request_id: &str) -> Result<ArticleRequestItem, String> {
    #[cfg(feature = "mock")]
    {
        let _ = request_id;
        Err("mock not supported".to_string())
    }

    #[cfg(not(feature = "mock"))]
    {
        let base = admin_base();
        let url = format!(
            "{base}/admin/article-requests/tasks/{}/retry",
            urlencoding::encode(request_id)
        );
        let response = api_post(&url)
            .json(&serde_json::json!({}))
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response
                .text()
                .await
                .map_err(|e| format!("Read error: {:?}", e))?;
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn admin_delete_article_request(request_id: &str) -> Result<(), String> {
    #[cfg(feature = "mock")]
    {
        let _ = request_id;
        Err("mock not supported".to_string())
    }

    #[cfg(not(feature = "mock"))]
    {
        let base = admin_base();
        let url =
            format!("{base}/admin/article-requests/tasks/{}", urlencoding::encode(request_id));
        let response = gloo_net::http::Request::delete(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        Ok(())
    }
}

pub async fn fetch_admin_article_request_ai_output(
    request_id: &str,
) -> Result<AdminArticleRequestAiOutputResponse, String> {
    #[cfg(feature = "mock")]
    {
        let _ = request_id;
        Ok(AdminArticleRequestAiOutputResponse {
            runs: vec![],
            chunks: vec![],
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let base = admin_base();
        let url = format!(
            "{base}/admin/article-requests/tasks/{}/ai-output",
            urlencoding::encode(request_id)
        );
        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub fn build_admin_article_request_ai_stream_url(request_id: &str) -> String {
    #[cfg(feature = "mock")]
    {
        format!("/mock/admin/article-requests/tasks/{}/ai-output/stream", request_id)
    }

    #[cfg(not(feature = "mock"))]
    {
        let base = admin_base();
        format!(
            "{base}/admin/article-requests/tasks/{}/ai-output/stream",
            urlencoding::encode(request_id)
        )
    }
}

/// Public key metadata exposed on the read-only LLM access page.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct LlmGatewayPublicKeyView {
    pub id: String,
    pub name: String,
    pub secret: String,
    pub quota_billable_limit: u64,
    pub usage_input_uncached_tokens: u64,
    pub usage_input_cached_tokens: u64,
    pub usage_output_tokens: u64,
    pub remaining_billable: i64,
    pub last_used_at: Option<i64>,
}

/// Public payload returned by `/api/llm-gateway/access`.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(default)]
pub struct LlmGatewayAccessResponse {
    pub base_url: String,
    pub gateway_path: String,
    pub model_catalog_path: String,
    pub auth_cache_ttl_seconds: u64,
    pub keys: Vec<LlmGatewayPublicKeyView>,
    pub generated_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(default)]
pub struct PublicLlmGatewayUsageLookupRequest {
    pub api_key: String,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(default)]
pub struct PublicLlmGatewayUsageKeyView {
    pub name: String,
    pub provider_type: String,
    pub quota_billable_limit: u64,
    pub usage_input_uncached_tokens: u64,
    pub usage_input_cached_tokens: u64,
    pub usage_output_tokens: u64,
    pub usage_billable_tokens: u64,
    pub usage_credit_total: f64,
    pub usage_credit_missing_events: u64,
    pub remaining_billable: i64,
    pub last_used_at: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(default)]
pub struct PublicLlmGatewayUsageEventView {
    pub id: String,
    pub key_name: String,
    pub account_name: Option<String>,
    pub request_method: String,
    pub request_url: String,
    pub latency_ms: i32,
    pub routing_wait_ms: Option<i32>,
    pub upstream_headers_ms: Option<i32>,
    pub post_headers_body_ms: Option<i32>,
    pub request_body_bytes: Option<u64>,
    pub request_body_read_ms: Option<i32>,
    pub request_json_parse_ms: Option<i32>,
    pub pre_handler_ms: Option<i32>,
    pub first_sse_write_ms: Option<i32>,
    pub stream_finish_ms: Option<i32>,
    pub other_latency_ms: Option<i32>,
    pub quota_failover_count: u64,
    pub endpoint: String,
    pub model: Option<String>,
    pub status_code: i32,
    pub input_uncached_tokens: u64,
    pub input_cached_tokens: u64,
    pub output_tokens: u64,
    pub billable_tokens: u64,
    pub usage_missing: bool,
    pub credit_usage: Option<f64>,
    pub credit_usage_missing: bool,
    pub client_ip: String,
    pub ip_region: String,
    pub created_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(default)]
pub struct PublicLlmGatewayUsageChartPointView {
    pub bucket_start_ms: i64,
    pub tokens: u64,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(default)]
pub struct PublicLlmGatewayUsageLookupResponse {
    pub key: PublicLlmGatewayUsageKeyView,
    pub chart_points: Vec<PublicLlmGatewayUsageChartPointView>,
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
    pub has_more: bool,
    pub events: Vec<PublicLlmGatewayUsageEventView>,
    pub generated_at: i64,
}

/// One public usage window from the cached Codex limit snapshot.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct LlmGatewayRateLimitWindowView {
    pub used_percent: f64,
    pub remaining_percent: f64,
    pub window_duration_mins: Option<i64>,
    pub resets_at: Option<i64>,
}

/// Optional credits metadata included in the cached status payload.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct LlmGatewayCreditsView {
    pub has_credits: bool,
    pub unlimited: bool,
    pub balance: Option<String>,
}

/// One public rate-limit bucket rendered on `/llm-access`.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct LlmGatewayRateLimitBucketView {
    pub limit_id: String,
    pub limit_name: Option<String>,
    pub display_name: String,
    pub is_primary: bool,
    pub plan_type: Option<String>,
    pub primary: Option<LlmGatewayRateLimitWindowView>,
    pub secondary: Option<LlmGatewayRateLimitWindowView>,
    pub credits: Option<LlmGatewayCreditsView>,
    pub account_name: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct LlmGatewayPublicAccountStatusView {
    pub name: String,
    pub status: String,
    pub plan_type: Option<String>,
    pub primary_remaining_percent: Option<f64>,
    pub secondary_remaining_percent: Option<f64>,
    pub last_usage_checked_at: Option<i64>,
    pub last_usage_success_at: Option<i64>,
    pub usage_error_message: Option<String>,
}

/// Cached public rate-limit status for the upstream Codex account.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct LlmGatewayRateLimitStatusResponse {
    pub status: String,
    pub refresh_interval_seconds: u64,
    pub last_checked_at: Option<i64>,
    pub last_success_at: Option<i64>,
    pub source_url: String,
    pub error_message: Option<String>,
    #[serde(default)]
    pub accounts: Vec<LlmGatewayPublicAccountStatusView>,
    pub buckets: Vec<LlmGatewayRateLimitBucketView>,
}

const fn default_true() -> bool {
    true
}

fn default_kiro_cache_policy_json() -> String {
    r#"{"small_input_high_credit_boost":{"target_input_tokens":100000,"credit_start":1.0,"credit_end":1.8},"prefix_tree_credit_ratio_bands":[{"credit_start":0.3,"credit_end":1.0,"cache_ratio_start":0.7,"cache_ratio_end":0.2},{"credit_start":1.0,"credit_end":2.5,"cache_ratio_start":0.2,"cache_ratio_end":0.0}],"high_credit_diagnostic_threshold":2.0}"#.to_string()
}

fn default_kiro_billable_model_multipliers_json() -> String {
    r#"{"haiku":1.0,"opus":1.0,"sonnet":1.0}"#.to_string()
}

/// Admin-only editable representation of a gateway key.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(default)]
pub struct AdminLlmGatewayKeyView {
    pub id: String,
    pub name: String,
    pub secret: String,
    pub key_hash: String,
    pub status: String,
    pub provider_type: String,
    pub public_visible: bool,
    pub quota_billable_limit: u64,
    pub usage_input_uncached_tokens: u64,
    pub usage_input_cached_tokens: u64,
    pub usage_output_tokens: u64,
    pub usage_credit_total: f64,
    pub usage_credit_missing_events: u64,
    pub remaining_billable: i64,
    pub last_used_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    pub route_strategy: Option<String>,
    pub account_group_id: Option<String>,
    pub fixed_account_name: Option<String>,
    pub auto_account_names: Option<Vec<String>>,
    pub model_name_map: Option<BTreeMap<String, String>>,
    pub request_max_concurrency: Option<u64>,
    pub request_min_start_interval_ms: Option<u64>,
    #[serde(default = "default_true")]
    pub kiro_request_validation_enabled: bool,
    #[serde(default = "default_true")]
    pub kiro_cache_estimation_enabled: bool,
    #[serde(default)]
    pub kiro_zero_cache_debug_enabled: bool,
    #[serde(default)]
    pub kiro_cache_policy_override_json: Option<String>,
    #[serde(default)]
    pub kiro_billable_model_multipliers_override_json: Option<String>,
    #[serde(default = "default_kiro_cache_policy_json")]
    pub effective_kiro_cache_policy_json: String,
    #[serde(default = "default_true")]
    pub uses_global_kiro_cache_policy: bool,
    #[serde(default = "default_kiro_billable_model_multipliers_json")]
    pub effective_kiro_billable_model_multipliers_json: String,
    #[serde(default = "default_true")]
    pub uses_global_kiro_billable_model_multipliers: bool,
}

/// Combined admin payload for the key inventory screen.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(default)]
pub struct AdminLlmGatewayKeysResponse {
    pub keys: Vec<AdminLlmGatewayKeyView>,
    pub auth_cache_ttl_seconds: u64,
    pub generated_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(default)]
pub struct AdminAccountGroupView {
    pub id: String,
    pub provider_type: String,
    pub name: String,
    pub account_names: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(default)]
pub struct AdminAccountGroupsResponse {
    pub groups: Vec<AdminAccountGroupView>,
    pub generated_at: i64,
}

/// Summary usage event used by admin paging and filtering views.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(default)]
pub struct AdminLlmGatewayUsageEventView {
    pub id: String,
    pub key_id: String,
    pub key_name: String,
    pub account_name: Option<String>,
    pub request_method: String,
    pub request_url: String,
    pub latency_ms: i32,
    pub routing_wait_ms: Option<i32>,
    pub upstream_headers_ms: Option<i32>,
    pub post_headers_body_ms: Option<i32>,
    pub request_body_bytes: Option<u64>,
    pub request_body_read_ms: Option<i32>,
    pub request_json_parse_ms: Option<i32>,
    pub pre_handler_ms: Option<i32>,
    pub first_sse_write_ms: Option<i32>,
    pub stream_finish_ms: Option<i32>,
    pub other_latency_ms: Option<i32>,
    pub quota_failover_count: u64,
    pub routing_diagnostics_json: Option<String>,
    pub endpoint: String,
    pub model: Option<String>,
    pub status_code: i32,
    pub input_uncached_tokens: u64,
    pub input_cached_tokens: u64,
    pub output_tokens: u64,
    pub billable_tokens: u64,
    pub usage_missing: bool,
    pub credit_usage: Option<f64>,
    pub credit_usage_missing: bool,
    pub client_ip: String,
    pub ip_region: String,
    pub last_message_content: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(default)]
pub struct AdminLlmGatewayUsageEventDetailView {
    pub id: String,
    pub key_id: String,
    pub key_name: String,
    pub account_name: Option<String>,
    pub request_method: String,
    pub request_url: String,
    pub latency_ms: i32,
    pub routing_wait_ms: Option<i32>,
    pub upstream_headers_ms: Option<i32>,
    pub post_headers_body_ms: Option<i32>,
    pub request_body_bytes: Option<u64>,
    pub request_body_read_ms: Option<i32>,
    pub request_json_parse_ms: Option<i32>,
    pub pre_handler_ms: Option<i32>,
    pub first_sse_write_ms: Option<i32>,
    pub stream_finish_ms: Option<i32>,
    pub other_latency_ms: Option<i32>,
    pub quota_failover_count: u64,
    pub routing_diagnostics_json: Option<String>,
    pub endpoint: String,
    pub model: Option<String>,
    pub status_code: i32,
    pub input_uncached_tokens: u64,
    pub input_cached_tokens: u64,
    pub output_tokens: u64,
    pub billable_tokens: u64,
    pub usage_missing: bool,
    pub credit_usage: Option<f64>,
    pub credit_usage_missing: bool,
    pub client_ip: String,
    pub ip_region: String,
    pub request_headers_json: String,
    pub last_message_content: Option<String>,
    pub client_request_body_json: Option<String>,
    pub upstream_request_body_json: Option<String>,
    pub full_request_json: Option<String>,
    pub created_at: i64,
}

/// Paginated usage-event response from the admin diagnostics endpoint.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(default)]
pub struct AdminLlmGatewayUsageEventsResponse {
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
    pub has_more: bool,
    pub current_rpm: u32,
    pub current_in_flight: u32,
    pub events: Vec<AdminLlmGatewayUsageEventView>,
    pub generated_at: i64,
}

/// Query options for paginating and filtering LLM gateway usage events.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
pub struct AdminLlmGatewayUsageEventsQuery {
    pub key_id: Option<String>,
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

/// Public acknowledgement returned after a token wish is queued.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct SubmitLlmGatewayTokenRequestResponse {
    pub request_id: String,
    pub status: String,
}

/// Public acknowledgement returned after an account contribution is queued.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct SubmitLlmGatewayAccountContributionRequestResponse {
    pub request_id: String,
    pub status: String,
}

/// Public thank-you card item for approved account contributions.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct PublicLlmGatewayAccountContributionView {
    pub request_id: String,
    pub account_name: String,
    pub contributor_message: String,
    pub github_id: Option<String>,
    pub processed_at: Option<i64>,
}

/// Public response for approved account contribution cards.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct PublicLlmGatewayAccountContributionsResponse {
    pub contributions: Vec<PublicLlmGatewayAccountContributionView>,
    pub generated_at: i64,
}

/// Public support/community config rendered on `/llm-access`.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct LlmGatewaySupportConfigView {
    pub sponsor_title: String,
    pub sponsor_intro: String,
    pub group_name: String,
    pub qq_group_number: String,
    pub group_invite_text: String,
    pub alipay_qr_url: String,
    pub wechat_qr_url: String,
    pub qq_group_qr_url: Option<String>,
    pub generated_at: i64,
}

/// Public form payload for contributing a Codex account.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct SubmitLlmGatewayAccountContributionInput {
    pub account_name: String,
    pub account_id: Option<String>,
    pub id_token: String,
    pub access_token: String,
    pub refresh_token: String,
    pub requester_email: String,
    pub contributor_message: String,
    pub github_id: Option<String>,
    pub frontend_page_url: Option<String>,
}

/// Public form payload for contributing a GPT image account.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct SubmitGpt2ApiAccountContributionInput {
    pub account_name: String,
    pub access_token: Option<String>,
    pub session_json: Option<String>,
    pub requester_email: String,
    pub contributor_message: String,
    pub github_id: Option<String>,
    pub frontend_page_url: Option<String>,
}

/// Public acknowledgement returned after a GPT contribution is queued.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct SubmitGpt2ApiAccountContributionRequestResponse {
    pub request_id: String,
    pub status: String,
}

/// Public form payload for requesting to become a sponsor.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct SubmitLlmGatewaySponsorInput {
    pub requester_email: String,
    pub sponsor_message: String,
    pub display_name: Option<String>,
    pub github_id: Option<String>,
    pub frontend_page_url: Option<String>,
}

/// Public acknowledgement returned after a sponsor request is queued.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct SubmitLlmGatewaySponsorRequestResponse {
    pub request_id: String,
    pub status: String,
    pub payment_email_sent: bool,
}

/// Public thank-you card item for approved sponsors.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct PublicLlmGatewaySponsorView {
    pub request_id: String,
    pub display_name: Option<String>,
    pub sponsor_message: String,
    pub github_id: Option<String>,
    pub processed_at: Option<i64>,
}

/// Public response for approved sponsor cards.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct PublicLlmGatewaySponsorsResponse {
    pub sponsors: Vec<PublicLlmGatewaySponsorView>,
    pub generated_at: i64,
}

/// Admin-only view of one token wish / issuance task.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct AdminLlmGatewayTokenRequestView {
    pub request_id: String,
    pub requester_email: String,
    pub requested_quota_billable_limit: u64,
    pub request_reason: String,
    pub frontend_page_url: Option<String>,
    pub status: String,
    pub client_ip: String,
    pub ip_region: String,
    pub admin_note: Option<String>,
    pub failure_reason: Option<String>,
    pub issued_key_id: Option<String>,
    pub issued_key_name: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub processed_at: Option<i64>,
}

/// Paginated admin response for token wishes.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct AdminLlmGatewayTokenRequestsResponse {
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
    pub has_more: bool,
    pub requests: Vec<AdminLlmGatewayTokenRequestView>,
    pub generated_at: i64,
}

/// Admin-only view of one Codex account contribution request.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct AdminLlmGatewayAccountContributionRequestView {
    pub request_id: String,
    pub account_name: String,
    pub account_id: Option<String>,
    pub id_token: String,
    pub access_token: String,
    pub refresh_token: String,
    pub requester_email: String,
    pub contributor_message: String,
    pub github_id: Option<String>,
    pub frontend_page_url: Option<String>,
    pub status: String,
    pub client_ip: String,
    pub ip_region: String,
    pub admin_note: Option<String>,
    pub failure_reason: Option<String>,
    pub imported_account_name: Option<String>,
    pub issued_key_id: Option<String>,
    pub issued_key_name: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub processed_at: Option<i64>,
}

/// Paginated admin response for account contribution requests.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct AdminLlmGatewayAccountContributionRequestsResponse {
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
    pub has_more: bool,
    pub requests: Vec<AdminLlmGatewayAccountContributionRequestView>,
    pub generated_at: i64,
}

/// Query options for admin account contribution request listing.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
pub struct AdminLlmGatewayAccountContributionRequestsQuery {
    pub status: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

/// Admin-only view of one GPT account contribution request.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct AdminGpt2ApiAccountContributionRequestView {
    pub request_id: String,
    pub account_name: String,
    pub access_token: Option<String>,
    pub session_json: Option<String>,
    pub requester_email: String,
    pub contributor_message: String,
    pub github_id: Option<String>,
    pub frontend_page_url: Option<String>,
    pub status: String,
    pub client_ip: String,
    pub ip_region: String,
    pub admin_note: Option<String>,
    pub failure_reason: Option<String>,
    pub imported_account_name: Option<String>,
    pub issued_key_id: Option<String>,
    pub issued_key_name: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub processed_at: Option<i64>,
}

/// Paginated admin response for GPT account contribution requests.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct AdminGpt2ApiAccountContributionRequestsResponse {
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
    pub has_more: bool,
    pub requests: Vec<AdminGpt2ApiAccountContributionRequestView>,
    pub generated_at: i64,
}

/// Query options for admin GPT account contribution request listing.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
pub struct AdminGpt2ApiAccountContributionRequestsQuery {
    pub status: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

/// Admin-only view of one sponsor request.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct AdminLlmGatewaySponsorRequestView {
    pub request_id: String,
    pub requester_email: String,
    pub sponsor_message: String,
    pub display_name: Option<String>,
    pub github_id: Option<String>,
    pub frontend_page_url: Option<String>,
    pub status: String,
    pub client_ip: String,
    pub ip_region: String,
    pub admin_note: Option<String>,
    pub failure_reason: Option<String>,
    pub payment_email_sent_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    pub processed_at: Option<i64>,
}

/// Paginated admin response for sponsor requests.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct AdminLlmGatewaySponsorRequestsResponse {
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
    pub has_more: bool,
    pub requests: Vec<AdminLlmGatewaySponsorRequestView>,
    pub generated_at: i64,
}

/// Query options for admin sponsor request listing.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
pub struct AdminLlmGatewaySponsorRequestsQuery {
    pub status: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

/// Query options for admin token-wish listing.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
pub struct AdminLlmGatewayTokenRequestsQuery {
    pub status: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

/// Editable LLM gateway runtime settings exposed to the admin UI.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct LlmGatewayRuntimeConfig {
    pub auth_cache_ttl_seconds: u64,
    pub max_request_body_bytes: u64,
    pub account_failure_retry_limit: u64,
    #[serde(default = "default_codex_client_version")]
    pub codex_client_version: String,
    pub codex_status_refresh_min_interval_seconds: u64,
    pub codex_status_refresh_max_interval_seconds: u64,
    pub codex_status_account_jitter_max_seconds: u64,
    pub kiro_status_refresh_min_interval_seconds: u64,
    pub kiro_status_refresh_max_interval_seconds: u64,
    pub kiro_status_account_jitter_max_seconds: u64,
    pub usage_event_flush_batch_size: u64,
    pub usage_event_flush_interval_seconds: u64,
    pub usage_event_flush_max_buffer_bytes: u64,
    pub kiro_cache_kmodels_json: String,
    #[serde(default = "default_kiro_billable_model_multipliers_json")]
    pub kiro_billable_model_multipliers_json: String,
    #[serde(default = "default_kiro_cache_policy_json")]
    pub kiro_cache_policy_json: String,
    pub kiro_prefix_cache_mode: String,
    pub kiro_prefix_cache_max_tokens: u64,
    pub kiro_prefix_cache_entry_ttl_seconds: u64,
    pub kiro_conversation_anchor_max_entries: u64,
    pub kiro_conversation_anchor_ttl_seconds: u64,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(default)]
pub struct AdminUpstreamProxyConfigView {
    pub id: String,
    pub name: String,
    pub proxy_url: String,
    pub proxy_username: Option<String>,
    pub proxy_password: Option<String>,
    pub status: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(default)]
pub struct AdminUpstreamProxyConfigsResponse {
    pub proxy_configs: Vec<AdminUpstreamProxyConfigView>,
    pub generated_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(default)]
pub struct AdminUpstreamProxyCheckTargetView {
    pub target: String,
    pub url: String,
    pub reachable: bool,
    pub status_code: Option<u16>,
    pub latency_ms: i64,
    pub error_message: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(default)]
pub struct AdminUpstreamProxyCheckResponse {
    pub proxy_config_id: String,
    pub proxy_config_name: String,
    pub provider_type: String,
    pub auth_label: String,
    pub ok: bool,
    pub targets: Vec<AdminUpstreamProxyCheckTargetView>,
    pub checked_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(default)]
pub struct AdminUpstreamProxyBindingView {
    pub provider_type: String,
    pub effective_source: String,
    pub bound_proxy_config_id: Option<String>,
    pub effective_proxy_config_name: Option<String>,
    pub effective_proxy_url: Option<String>,
    pub effective_proxy_username: Option<String>,
    pub effective_proxy_password: Option<String>,
    pub binding_updated_at: Option<i64>,
    pub error_message: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(default)]
pub struct AdminUpstreamProxyBindingsResponse {
    pub bindings: Vec<AdminUpstreamProxyBindingView>,
    pub generated_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(default)]
pub struct AdminLegacyKiroProxyMigrationResponse {
    pub created_configs: Vec<AdminUpstreamProxyConfigView>,
    pub reused_configs: Vec<AdminUpstreamProxyConfigView>,
    pub migrated_account_names: Vec<String>,
    pub generated_at: i64,
}

#[derive(Debug, Serialize, Clone, PartialEq, Default)]
pub struct CreateAdminUpstreamProxyConfigInput {
    pub name: String,
    pub proxy_url: String,
    pub proxy_username: Option<String>,
    pub proxy_password: Option<String>,
}

#[derive(Debug, Serialize, Clone, PartialEq, Default)]
pub struct PatchAdminUpstreamProxyConfigInput {
    pub name: Option<String>,
    pub proxy_url: Option<String>,
    pub proxy_username: Option<String>,
    pub proxy_password: Option<String>,
    pub status: Option<String>,
}

/// Fetch the read-only public gateway access bundle used by `/llm-access`.
pub async fn fetch_llm_gateway_access() -> Result<LlmGatewayAccessResponse, String> {
    #[cfg(feature = "mock")]
    {
        Ok(LlmGatewayAccessResponse {
            base_url: "http://localhost:3000/api/llm-gateway/v1".to_string(),
            gateway_path: "/api/llm-gateway/v1".to_string(),
            model_catalog_path: "/api/llm-gateway/model-catalog.json".to_string(),
            auth_cache_ttl_seconds: 60,
            keys: vec![],
            generated_at: 0,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/llm-gateway/access?_ts={}", API_BASE, Date::now() as u64);
        let response = api_get(&url)
            .header("Cache-Control", "no-cache, no-store, max-age=0")
            .header("Pragma", "no-cache")
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

#[cfg(any(not(feature = "mock"), test))]
fn build_llm_gateway_model_catalog_url_for_ts(path: Option<&str>, ts: u64) -> String {
    let path = path
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("/api/llm-gateway/model-catalog.json");
    if path.starts_with("http://") || path.starts_with("https://") || path.starts_with("/api/") {
        format!("{path}?_ts={ts}")
    } else {
        format!("{API_BASE}{path}?_ts={ts}")
    }
}

#[cfg(not(feature = "mock"))]
pub fn build_llm_gateway_model_catalog_url(path: Option<&str>) -> String {
    build_llm_gateway_model_catalog_url_for_ts(path, Date::now() as u64)
}

pub async fn fetch_llm_gateway_model_catalog_json(
    model_catalog_path: Option<&str>,
) -> Result<String, String> {
    #[cfg(feature = "mock")]
    {
        let _ = model_catalog_path;
        Ok(r#"{"models":[{"slug":"gpt-5.5","visibility":"list","supported_in_api":true}]}"#
            .to_string())
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = build_llm_gateway_model_catalog_url(model_catalog_path);
        let response = api_get(&url)
            .header("Cache-Control", "no-cache, no-store, max-age=0")
            .header("Pragma", "no-cache")
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .text()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn fetch_public_llm_gateway_usage(
    request: &PublicLlmGatewayUsageLookupRequest,
) -> Result<PublicLlmGatewayUsageLookupResponse, String> {
    #[cfg(feature = "mock")]
    {
        let _ = request;
        Ok(PublicLlmGatewayUsageLookupResponse {
            key: PublicLlmGatewayUsageKeyView {
                name: "mock-public-key".to_string(),
                provider_type: "codex".to_string(),
                quota_billable_limit: 10_000,
                usage_input_uncached_tokens: 2_500,
                usage_input_cached_tokens: 800,
                usage_output_tokens: 1_700,
                usage_billable_tokens: 4_200,
                usage_credit_total: 0.0,
                usage_credit_missing_events: 0,
                remaining_billable: 5_800,
                last_used_at: Some(1_775_000_000_000),
            },
            chart_points: (0..24)
                .map(|index| PublicLlmGatewayUsageChartPointView {
                    bucket_start_ms: 1_775_000_000_000 - ((23 - index) as i64 * 3_600_000),
                    tokens: if index % 4 == 0 { 480 } else { 120 + (index as u64 * 13) },
                })
                .collect(),
            total: 2,
            offset: request.offset.unwrap_or(0),
            limit: request.limit.unwrap_or(50),
            has_more: false,
            events: vec![
                PublicLlmGatewayUsageEventView {
                    id: "mock-usage-2".to_string(),
                    key_name: "mock-public-key".to_string(),
                    account_name: Some("default".to_string()),
                    request_method: "POST".to_string(),
                    request_url: "/api/llm-gateway/v1/responses".to_string(),
                    latency_ms: 842,
                    routing_wait_ms: None,
                    upstream_headers_ms: None,
                    post_headers_body_ms: None,
                    request_body_bytes: None,
                    request_body_read_ms: None,
                    request_json_parse_ms: None,
                    pre_handler_ms: None,
                    first_sse_write_ms: None,
                    stream_finish_ms: None,
                    other_latency_ms: None,
                    quota_failover_count: 0,
                    endpoint: "/responses".to_string(),
                    model: Some("gpt-5.3-codex".to_string()),
                    status_code: 200,
                    input_uncached_tokens: 420,
                    input_cached_tokens: 0,
                    output_tokens: 156,
                    billable_tokens: 576,
                    usage_missing: false,
                    credit_usage: None,
                    credit_usage_missing: false,
                    client_ip: "203.0.113.8".to_string(),
                    ip_region: "US".to_string(),
                    created_at: 1_775_000_000_000,
                },
                PublicLlmGatewayUsageEventView {
                    id: "mock-usage-1".to_string(),
                    key_name: "mock-public-key".to_string(),
                    account_name: Some("backup".to_string()),
                    request_method: "POST".to_string(),
                    request_url: "/api/llm-gateway/v1/responses".to_string(),
                    latency_ms: 1_204,
                    routing_wait_ms: None,
                    upstream_headers_ms: None,
                    post_headers_body_ms: None,
                    request_body_bytes: None,
                    request_body_read_ms: None,
                    request_json_parse_ms: None,
                    pre_handler_ms: None,
                    first_sse_write_ms: None,
                    stream_finish_ms: None,
                    other_latency_ms: None,
                    quota_failover_count: 0,
                    endpoint: "/responses".to_string(),
                    model: Some("gpt-5.3-codex".to_string()),
                    status_code: 200,
                    input_uncached_tokens: 310,
                    input_cached_tokens: 64,
                    output_tokens: 208,
                    billable_tokens: 518,
                    usage_missing: false,
                    credit_usage: None,
                    credit_usage_missing: false,
                    client_ip: "203.0.113.8".to_string(),
                    ip_region: "US".to_string(),
                    created_at: 1_774_996_400_000,
                },
            ],
            generated_at: 1_775_000_000_000,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/llm-gateway/public-usage/query", API_BASE);
        let response = api_post(&url)
            .header("Cache-Control", "no-cache, no-store, max-age=0")
            .json(request)
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

/// Fetch the cached public Codex rate-limit snapshot used by `/llm-access`.
pub async fn fetch_llm_gateway_status() -> Result<LlmGatewayRateLimitStatusResponse, String> {
    #[cfg(feature = "mock")]
    {
        Ok(LlmGatewayRateLimitStatusResponse {
            status: "ready".to_string(),
            refresh_interval_seconds: 60,
            last_checked_at: Some(0),
            last_success_at: Some(0),
            source_url: "https://chatgpt.com/backend-api/wham/usage".to_string(),
            error_message: None,
            accounts: vec![
                LlmGatewayPublicAccountStatusView {
                    name: "default".to_string(),
                    status: "active".to_string(),
                    plan_type: Some("Pro".to_string()),
                    primary_remaining_percent: Some(62.0),
                    secondary_remaining_percent: Some(39.0),
                    last_usage_checked_at: Some(0),
                    last_usage_success_at: Some(0),
                    usage_error_message: None,
                },
                LlmGatewayPublicAccountStatusView {
                    name: "backup".to_string(),
                    status: "unavailable".to_string(),
                    plan_type: Some("Pro".to_string()),
                    primary_remaining_percent: Some(17.0),
                    secondary_remaining_percent: Some(5.0),
                    last_usage_checked_at: Some(0),
                    last_usage_success_at: Some(0),
                    usage_error_message: Some("upstream 503".to_string()),
                },
            ],
            buckets: vec![LlmGatewayRateLimitBucketView {
                limit_id: "codex".to_string(),
                limit_name: None,
                display_name: "codex".to_string(),
                is_primary: true,
                plan_type: Some("Pro".to_string()),
                primary: Some(LlmGatewayRateLimitWindowView {
                    used_percent: 38.0,
                    remaining_percent: 62.0,
                    window_duration_mins: Some(300),
                    resets_at: Some(0),
                }),
                secondary: Some(LlmGatewayRateLimitWindowView {
                    used_percent: 61.0,
                    remaining_percent: 39.0,
                    window_duration_mins: Some(10080),
                    resets_at: Some(0),
                }),
                credits: Some(LlmGatewayCreditsView {
                    has_credits: true,
                    unlimited: false,
                    balance: Some("24".to_string()),
                }),
                account_name: Some("default".to_string()),
            }],
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/llm-gateway/status?_ts={}", API_BASE, Date::now() as u64);
        let response = api_get(&url)
            .header("Cache-Control", "no-cache, no-store, max-age=0")
            .header("Pragma", "no-cache")
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

/// Submit a public token wish from `/llm-access`.
pub async fn submit_llm_gateway_token_request(
    requested_quota_billable_limit: u64,
    request_reason: &str,
    requester_email: &str,
    frontend_page_url: Option<&str>,
) -> Result<SubmitLlmGatewayTokenRequestResponse, String> {
    #[cfg(feature = "mock")]
    {
        let _ =
            (requested_quota_billable_limit, request_reason, requester_email, frontend_page_url);
        Ok(SubmitLlmGatewayTokenRequestResponse {
            request_id: "mock-llm-wish-1".to_string(),
            status: "pending".to_string(),
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/llm-gateway/token-requests/submit", API_BASE);
        let mut body = serde_json::json!({
            "requested_quota_billable_limit": requested_quota_billable_limit,
            "request_reason": request_reason,
            "requester_email": requester_email,
        });
        if let Some(page_url) = frontend_page_url {
            body["frontend_page_url"] = serde_json::Value::String(page_url.to_string());
        }
        let response = api_post(&url)
            .json(&body)
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

/// Submit a public Codex account contribution request from `/llm-access`.
pub async fn submit_llm_gateway_account_contribution_request(
    input: &SubmitLlmGatewayAccountContributionInput,
) -> Result<SubmitLlmGatewayAccountContributionRequestResponse, String> {
    #[cfg(feature = "mock")]
    {
        let _ = input;
        Ok(SubmitLlmGatewayAccountContributionRequestResponse {
            request_id: "mock-llm-account-contribution-1".to_string(),
            status: "pending".to_string(),
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/llm-gateway/account-contribution-requests/submit", API_BASE);
        let mut body = serde_json::json!({
            "account_name": input.account_name,
            "id_token": input.id_token,
            "access_token": input.access_token,
            "refresh_token": input.refresh_token,
            "requester_email": input.requester_email,
            "contributor_message": input.contributor_message,
        });
        if let Some(account_id) = input
            .account_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            body["account_id"] = serde_json::Value::String(account_id.trim().to_string());
        }
        if let Some(github_id) = input
            .github_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            body["github_id"] = serde_json::Value::String(github_id.trim().to_string());
        }
        if let Some(page_url) = input.frontend_page_url.as_deref() {
            body["frontend_page_url"] = serde_json::Value::String(page_url.to_string());
        }
        let response = api_post(&url)
            .json(&body)
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

/// Submit a public GPT account contribution request from `/llm-access`.
pub async fn submit_gpt2api_account_contribution_request(
    input: &SubmitGpt2ApiAccountContributionInput,
) -> Result<SubmitGpt2ApiAccountContributionRequestResponse, String> {
    #[cfg(feature = "mock")]
    {
        let _ = input;
        Ok(SubmitGpt2ApiAccountContributionRequestResponse {
            request_id: "mock-gpt2api-account-contribution-1".to_string(),
            status: "pending".to_string(),
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/gpt2api/account-contribution-requests/submit", API_BASE);
        let mut body = serde_json::json!({
            "account_name": input.account_name,
            "requester_email": input.requester_email,
            "contributor_message": input.contributor_message,
        });
        if let Some(access_token) = input
            .access_token
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            body["access_token"] = serde_json::Value::String(access_token.trim().to_string());
        }
        if let Some(session_json) = input
            .session_json
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            body["session_json"] = serde_json::Value::String(session_json.trim().to_string());
        }
        if let Some(github_id) = input
            .github_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            body["github_id"] = serde_json::Value::String(github_id.trim().to_string());
        }
        if let Some(page_url) = input.frontend_page_url.as_deref() {
            body["frontend_page_url"] = serde_json::Value::String(page_url.to_string());
        }
        let response = api_post(&url)
            .json(&body)
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

/// Fetch approved public account contributions for the thank-you wall.
pub async fn fetch_llm_gateway_account_contributions(
) -> Result<PublicLlmGatewayAccountContributionsResponse, String> {
    #[cfg(feature = "mock")]
    {
        Ok(PublicLlmGatewayAccountContributionsResponse {
            contributions: vec![],
            generated_at: 0,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url =
            format!("{}/llm-gateway/account-contributions?_ts={}", API_BASE, Date::now() as u64);
        let response = api_get(&url)
            .header("Cache-Control", "no-cache, no-store, max-age=0")
            .header("Pragma", "no-cache")
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

/// Fetch public sponsor/community configuration for `/llm-access`.
pub async fn fetch_llm_gateway_support_config() -> Result<LlmGatewaySupportConfigView, String> {
    #[cfg(feature = "mock")]
    {
        Ok(LlmGatewaySupportConfigView {
            sponsor_title: "请作者喝杯咖啡".to_string(),
            sponsor_intro: "填写邮箱后，系统会把赞助说明和收款码发给你。".to_string(),
            group_name: "美区词元魔盗团".to_string(),
            qq_group_number: "1092356490".to_string(),
            group_invite_text: "遇到 token、贡献或使用问题都可以进群交流。".to_string(),
            alipay_qr_url: "/api/llm-gateway/support-assets/alipay_qr.png".to_string(),
            wechat_qr_url: "/api/llm-gateway/support-assets/wechat_qr.png".to_string(),
            qq_group_qr_url: Some("/api/llm-gateway/support-assets/qq_group_qr.png".to_string()),
            generated_at: 0,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/llm-gateway/support-config?_ts={}", API_BASE, Date::now() as u64);
        let response = api_get(&url)
            .header("Cache-Control", "no-cache, no-store, max-age=0")
            .header("Pragma", "no-cache")
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

/// Submit a public sponsor request from `/llm-access`.
pub async fn submit_llm_gateway_sponsor_request(
    input: &SubmitLlmGatewaySponsorInput,
) -> Result<SubmitLlmGatewaySponsorRequestResponse, String> {
    #[cfg(feature = "mock")]
    {
        let _ = input;
        Ok(SubmitLlmGatewaySponsorRequestResponse {
            request_id: "mock-llm-sponsor-1".to_string(),
            status: "payment_email_sent".to_string(),
            payment_email_sent: true,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/llm-gateway/sponsor-requests/submit", API_BASE);
        let mut body = serde_json::json!({
            "requester_email": input.requester_email,
            "sponsor_message": input.sponsor_message,
        });
        if let Some(display_name) = input
            .display_name
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            body["display_name"] = serde_json::Value::String(display_name.trim().to_string());
        }
        if let Some(github_id) = input
            .github_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            body["github_id"] = serde_json::Value::String(github_id.trim().to_string());
        }
        if let Some(page_url) = input.frontend_page_url.as_deref() {
            body["frontend_page_url"] = serde_json::Value::String(page_url.to_string());
        }
        let response = api_post(&url)
            .json(&body)
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

/// Fetch approved public sponsors for the thank-you wall.
pub async fn fetch_llm_gateway_sponsors() -> Result<PublicLlmGatewaySponsorsResponse, String> {
    #[cfg(feature = "mock")]
    {
        Ok(PublicLlmGatewaySponsorsResponse {
            sponsors: vec![],
            generated_at: 0,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/llm-gateway/sponsors?_ts={}", API_BASE, Date::now() as u64);
        let response = api_get(&url)
            .header("Cache-Control", "no-cache, no-store, max-age=0")
            .header("Pragma", "no-cache")
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn fetch_admin_music_runtime_config() -> Result<MusicRuntimeConfig, String> {
    #[cfg(feature = "mock")]
    {
        Ok(MusicRuntimeConfig {
            play_dedupe_window_seconds: 60,
            comment_rate_limit_seconds: 60,
            list_default_limit: 20,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/admin/music-config", admin_base());
        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn update_admin_music_runtime_config(
    config: &MusicRuntimeConfig,
) -> Result<MusicRuntimeConfig, String> {
    #[cfg(feature = "mock")]
    {
        Ok(config.clone())
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/admin/music-config", admin_base());
        let response = api_post(&url)
            .header("Content-Type", "application/json")
            .json(config)
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            return Err(format!("HTTP error: {}", response.status()));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

/// Fetch the current admin runtime configuration for the gateway cache.
pub async fn fetch_admin_llm_gateway_config() -> Result<LlmGatewayRuntimeConfig, String> {
    #[cfg(feature = "mock")]
    {
        Ok(LlmGatewayRuntimeConfig {
            auth_cache_ttl_seconds: 60,
            max_request_body_bytes: 8 * 1024 * 1024,
            account_failure_retry_limit: 3,
            codex_client_version: default_codex_client_version(),
            codex_status_refresh_min_interval_seconds: 240,
            codex_status_refresh_max_interval_seconds: 300,
            codex_status_account_jitter_max_seconds: 10,
            kiro_status_refresh_min_interval_seconds: 240,
            kiro_status_refresh_max_interval_seconds: 300,
            kiro_status_account_jitter_max_seconds: 10,
            usage_event_flush_batch_size: 256,
            usage_event_flush_interval_seconds: 15,
            usage_event_flush_max_buffer_bytes: 8 * 1024 * 1024,
            kiro_cache_kmodels_json: r#"{"claude-haiku-4-5-20251001":2.3681034438052206e-06,"claude-opus-4-6":8.061927916785985e-06,"claude-sonnet-4-6":5.055065250835128e-06}"#.to_string(),
            kiro_billable_model_multipliers_json: default_kiro_billable_model_multipliers_json(),
            kiro_cache_policy_json: r#"{"small_input_high_credit_boost":{"target_input_tokens":100000,"credit_start":1.0,"credit_end":1.8},"prefix_tree_credit_ratio_bands":[{"credit_start":0.3,"credit_end":1.0,"cache_ratio_start":0.7,"cache_ratio_end":0.2},{"credit_start":1.0,"credit_end":2.5,"cache_ratio_start":0.2,"cache_ratio_end":0.0}],"high_credit_diagnostic_threshold":2.0}"#.to_string(),
            kiro_prefix_cache_mode: "prefix_tree".to_string(),
            kiro_prefix_cache_max_tokens: 4_000_000,
            kiro_prefix_cache_entry_ttl_seconds: 21_600,
            kiro_conversation_anchor_max_entries: 20_000,
            kiro_conversation_anchor_ttl_seconds: 86_400,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/admin/llm-gateway/config", admin_base());
        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

/// Persist a new admin-selected auth cache TTL for gateway key validation.
pub async fn update_admin_llm_gateway_config(
    config: &LlmGatewayRuntimeConfig,
) -> Result<LlmGatewayRuntimeConfig, String> {
    #[cfg(feature = "mock")]
    {
        Ok(config.clone())
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/admin/llm-gateway/config", admin_base());
        let response = api_post(&url)
            .json(config)
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn fetch_admin_llm_gateway_proxy_configs(
) -> Result<AdminUpstreamProxyConfigsResponse, String> {
    #[cfg(feature = "mock")]
    {
        Ok(AdminUpstreamProxyConfigsResponse::default())
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/admin/llm-gateway/proxy-configs", admin_base());
        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn create_admin_llm_gateway_proxy_config(
    input: &CreateAdminUpstreamProxyConfigInput,
) -> Result<AdminUpstreamProxyConfigView, String> {
    #[cfg(feature = "mock")]
    {
        Ok(AdminUpstreamProxyConfigView {
            id: "mock-proxy".to_string(),
            name: input.name.clone(),
            proxy_url: input.proxy_url.clone(),
            proxy_username: input.proxy_username.clone(),
            proxy_password: input.proxy_password.clone(),
            status: "active".to_string(),
            created_at: 0,
            updated_at: 0,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/admin/llm-gateway/proxy-configs", admin_base());
        let response = api_post(&url)
            .json(input)
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn patch_admin_llm_gateway_proxy_config(
    proxy_id: &str,
    input: &PatchAdminUpstreamProxyConfigInput,
) -> Result<AdminUpstreamProxyConfigView, String> {
    #[cfg(feature = "mock")]
    {
        let _ = proxy_id;
        Ok(AdminUpstreamProxyConfigView {
            id: "mock-proxy".to_string(),
            name: input.name.clone().unwrap_or_else(|| "mock".to_string()),
            proxy_url: input
                .proxy_url
                .clone()
                .unwrap_or_else(|| "http://127.0.0.1:11111".to_string()),
            proxy_username: input.proxy_username.clone(),
            proxy_password: input.proxy_password.clone(),
            status: input.status.clone().unwrap_or_else(|| "active".to_string()),
            created_at: 0,
            updated_at: 0,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!(
            "{}/admin/llm-gateway/proxy-configs/{}",
            admin_base(),
            urlencoding::encode(proxy_id)
        );
        let response = api_patch(&url)
            .json(input)
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn delete_admin_llm_gateway_proxy_config(proxy_id: &str) -> Result<(), String> {
    #[cfg(feature = "mock")]
    {
        let _ = proxy_id;
        Ok(())
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!(
            "{}/admin/llm-gateway/proxy-configs/{}",
            admin_base(),
            urlencoding::encode(proxy_id)
        );
        let response = api_delete(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        Ok(())
    }
}

pub async fn check_admin_llm_gateway_proxy_config(
    proxy_id: &str,
    provider_type: &str,
) -> Result<AdminUpstreamProxyCheckResponse, String> {
    #[cfg(feature = "mock")]
    {
        Ok(AdminUpstreamProxyCheckResponse {
            proxy_config_id: proxy_id.to_string(),
            proxy_config_name: "mock-proxy".to_string(),
            provider_type: provider_type.to_string(),
            auth_label: format!("{provider_type} auth `mock`"),
            ok: true,
            targets: vec![AdminUpstreamProxyCheckTargetView {
                target: provider_type.to_string(),
                url: if provider_type == "kiro" {
                    "https://q.us-east-1.amazonaws.com/getUsageLimits".to_string()
                } else {
                    "https://chatgpt.com/backend-api/codex/v1/models".to_string()
                },
                reachable: true,
                status_code: Some(200),
                latency_ms: 120,
                error_message: None,
            }],
            checked_at: 0,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!(
            "{}/admin/llm-gateway/proxy-configs/{}/check/{}",
            admin_base(),
            urlencoding::encode(proxy_id),
            urlencoding::encode(provider_type)
        );
        let response = api_post(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn fetch_admin_llm_gateway_proxy_bindings(
) -> Result<AdminUpstreamProxyBindingsResponse, String> {
    #[cfg(feature = "mock")]
    {
        Ok(AdminUpstreamProxyBindingsResponse::default())
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/admin/llm-gateway/proxy-bindings", admin_base());
        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn update_admin_llm_gateway_proxy_binding(
    provider_type: &str,
    proxy_config_id: Option<&str>,
) -> Result<AdminUpstreamProxyBindingView, String> {
    #[cfg(feature = "mock")]
    {
        Ok(AdminUpstreamProxyBindingView {
            provider_type: provider_type.to_string(),
            effective_source: if proxy_config_id.is_some() {
                "binding".to_string()
            } else {
                "env_fallback".to_string()
            },
            bound_proxy_config_id: proxy_config_id.map(ToString::to_string),
            effective_proxy_config_name: Some("mock".to_string()),
            effective_proxy_url: Some("http://127.0.0.1:11111".to_string()),
            effective_proxy_username: None,
            effective_proxy_password: None,
            binding_updated_at: Some(0),
            error_message: None,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!(
            "{}/admin/llm-gateway/proxy-bindings/{}",
            admin_base(),
            urlencoding::encode(provider_type)
        );
        let response = api_post(&url)
            .json(&serde_json::json!({ "proxy_config_id": proxy_config_id }))
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn import_admin_legacy_kiro_proxy_configs(
) -> Result<AdminLegacyKiroProxyMigrationResponse, String> {
    #[cfg(feature = "mock")]
    {
        Ok(AdminLegacyKiroProxyMigrationResponse::default())
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/admin/llm-gateway/proxy-configs/import-legacy-kiro", admin_base());
        let response = api_post(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

/// Fetch the full admin key inventory, including secrets and current counters.
pub async fn fetch_admin_llm_gateway_keys() -> Result<AdminLlmGatewayKeysResponse, String> {
    #[cfg(feature = "mock")]
    {
        Ok(AdminLlmGatewayKeysResponse {
            keys: vec![],
            auth_cache_ttl_seconds: 60,
            generated_at: 0,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/admin/llm-gateway/keys", admin_base());
        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

#[derive(Debug, Clone, Default)]
pub struct CreateAdminAccountGroupInput<'a> {
    pub name: &'a str,
    pub account_names: &'a [String],
}

#[derive(Debug, Clone, Default)]
pub struct PatchAdminAccountGroupInput<'a> {
    pub name: Option<&'a str>,
    pub account_names: Option<&'a [String]>,
}

pub async fn fetch_admin_llm_gateway_account_groups() -> Result<AdminAccountGroupsResponse, String>
{
    #[cfg(feature = "mock")]
    {
        Ok(AdminAccountGroupsResponse::default())
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/admin/llm-gateway/account-groups", admin_base());
        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn create_admin_llm_gateway_account_group(
    input: CreateAdminAccountGroupInput<'_>,
) -> Result<AdminAccountGroupView, String> {
    #[cfg(feature = "mock")]
    {
        Ok(AdminAccountGroupView {
            id: "mock-group".to_string(),
            provider_type: "codex".to_string(),
            name: input.name.to_string(),
            account_names: input.account_names.to_vec(),
            created_at: 0,
            updated_at: 0,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/admin/llm-gateway/account-groups", admin_base());
        let response = api_post(&url)
            .json(&serde_json::json!({
                "name": input.name,
                "account_names": input.account_names,
            }))
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn patch_admin_llm_gateway_account_group(
    group_id: &str,
    input: PatchAdminAccountGroupInput<'_>,
) -> Result<AdminAccountGroupView, String> {
    #[cfg(feature = "mock")]
    {
        Ok(AdminAccountGroupView {
            id: group_id.to_string(),
            provider_type: "codex".to_string(),
            name: input.name.unwrap_or("mock").to_string(),
            account_names: input
                .account_names
                .map(|value| value.to_vec())
                .unwrap_or_default(),
            created_at: 0,
            updated_at: 0,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!(
            "{}/admin/llm-gateway/account-groups/{}",
            admin_base(),
            urlencoding::encode(group_id)
        );
        let mut body = serde_json::Map::new();
        if let Some(name) = input.name.map(str::trim).filter(|value| !value.is_empty()) {
            body.insert("name".to_string(), serde_json::Value::String(name.to_string()));
        }
        if let Some(account_names) = input.account_names {
            body.insert(
                "account_names".to_string(),
                serde_json::Value::Array(
                    account_names
                        .iter()
                        .map(|value| serde_json::Value::String(value.clone()))
                        .collect(),
                ),
            );
        }
        let response = api_patch(&url)
            .json(&serde_json::Value::Object(body))
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn delete_admin_llm_gateway_account_group(group_id: &str) -> Result<(), String> {
    #[cfg(feature = "mock")]
    {
        let _ = group_id;
        Ok(())
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!(
            "{}/admin/llm-gateway/account-groups/{}",
            admin_base(),
            urlencoding::encode(group_id)
        );
        let response = api_delete(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        Ok(())
    }
}

/// Fetch admin token wishes for review / issuance.
pub async fn fetch_admin_llm_gateway_token_requests(
    query: &AdminLlmGatewayTokenRequestsQuery,
) -> Result<AdminLlmGatewayTokenRequestsResponse, String> {
    #[cfg(feature = "mock")]
    {
        let _ = query;
        Ok(AdminLlmGatewayTokenRequestsResponse {
            total: 0,
            offset: 0,
            limit: 20,
            has_more: false,
            requests: vec![],
            generated_at: 0,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let mut url = format!("{}/admin/llm-gateway/token-requests", admin_base());
        let mut params = Vec::new();
        if let Some(status) = query.status.as_deref() {
            params.push(format!("status={}", urlencoding::encode(status)));
        }
        if let Some(limit) = query.limit {
            params.push(format!("limit={limit}"));
        }
        if let Some(offset) = query.offset {
            params.push(format!("offset={offset}"));
        }
        if !params.is_empty() {
            url.push('?');
            url.push_str(&params.join("&"));
        }
        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

/// Fetch admin account contribution requests for review / issuance.
pub async fn fetch_admin_llm_gateway_account_contribution_requests(
    query: &AdminLlmGatewayAccountContributionRequestsQuery,
) -> Result<AdminLlmGatewayAccountContributionRequestsResponse, String> {
    #[cfg(feature = "mock")]
    {
        let _ = query;
        Ok(AdminLlmGatewayAccountContributionRequestsResponse {
            total: 0,
            offset: 0,
            limit: 20,
            has_more: false,
            requests: vec![],
            generated_at: 0,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let mut url = format!("{}/admin/llm-gateway/account-contribution-requests", admin_base());
        let mut params = Vec::new();
        if let Some(status) = query.status.as_deref() {
            params.push(format!("status={}", urlencoding::encode(status)));
        }
        if let Some(limit) = query.limit {
            params.push(format!("limit={limit}"));
        }
        if let Some(offset) = query.offset {
            params.push(format!("offset={offset}"));
        }
        if !params.is_empty() {
            url.push('?');
            url.push_str(&params.join("&"));
        }
        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

/// Fetch admin sponsor requests for manual review.
pub async fn fetch_admin_llm_gateway_sponsor_requests(
    query: &AdminLlmGatewaySponsorRequestsQuery,
) -> Result<AdminLlmGatewaySponsorRequestsResponse, String> {
    #[cfg(feature = "mock")]
    {
        let _ = query;
        Ok(AdminLlmGatewaySponsorRequestsResponse {
            total: 0,
            offset: 0,
            limit: 20,
            has_more: false,
            requests: vec![],
            generated_at: 0,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let mut url = format!("{}/admin/llm-gateway/sponsor-requests", admin_base());
        let mut params = Vec::new();
        if let Some(status) = query.status.as_deref() {
            params.push(format!("status={}", urlencoding::encode(status)));
        }
        if let Some(limit) = query.limit {
            params.push(format!("limit={limit}"));
        }
        if let Some(offset) = query.offset {
            params.push(format!("offset={offset}"));
        }
        if !params.is_empty() {
            url.push('?');
            url.push_str(&params.join("&"));
        }
        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

/// Create a new gateway key that can later be exposed on the public page.
pub async fn create_admin_llm_gateway_key(
    name: &str,
    quota_billable_limit: u64,
    public_visible: bool,
    request_max_concurrency: Option<u64>,
    request_min_start_interval_ms: Option<u64>,
) -> Result<AdminLlmGatewayKeyView, String> {
    #[cfg(feature = "mock")]
    {
        Ok(AdminLlmGatewayKeyView {
            id: "mock".to_string(),
            name: name.to_string(),
            secret: "sfk_mock".to_string(),
            key_hash: "hash".to_string(),
            status: "active".to_string(),
            provider_type: "codex".to_string(),
            public_visible,
            quota_billable_limit,
            usage_input_uncached_tokens: 0,
            usage_input_cached_tokens: 0,
            usage_output_tokens: 0,
            usage_credit_total: 0.0,
            usage_credit_missing_events: 0,
            remaining_billable: quota_billable_limit as i64,
            last_used_at: None,
            created_at: 0,
            updated_at: 0,
            route_strategy: None,
            account_group_id: None,
            fixed_account_name: None,
            auto_account_names: None,
            model_name_map: None,
            request_max_concurrency,
            request_min_start_interval_ms,
            kiro_request_validation_enabled: true,
            kiro_cache_estimation_enabled: true,
            kiro_zero_cache_debug_enabled: false,
            kiro_cache_policy_override_json: None,
            kiro_billable_model_multipliers_override_json: None,
            effective_kiro_cache_policy_json: String::new(),
            uses_global_kiro_cache_policy: true,
            effective_kiro_billable_model_multipliers_json:
                default_kiro_billable_model_multipliers_json(),
            uses_global_kiro_billable_model_multipliers: true,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/admin/llm-gateway/keys", admin_base());
        let response = api_post(&url)
            .json(&serde_json::json!({
                "name": name,
                "quota_billable_limit": quota_billable_limit,
                "public_visible": public_visible,
                "request_max_concurrency": request_max_concurrency,
                "request_min_start_interval_ms": request_min_start_interval_ms
            }))
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

/// Patch editable fields on a gateway key from the admin UI.
#[derive(Clone, Debug, Default)]
pub struct PatchAdminLlmGatewayKeyRequest<'a> {
    pub name: Option<&'a str>,
    pub status: Option<&'a str>,
    pub public_visible: Option<bool>,
    pub quota_billable_limit: Option<u64>,
    pub route_strategy: Option<&'a str>,
    pub account_group_id: Option<&'a str>,
    pub fixed_account_name: Option<&'a str>,
    pub auto_account_names: Option<&'a [String]>,
    pub model_name_map: Option<&'a BTreeMap<String, String>>,
    pub request_max_concurrency: Option<u64>,
    pub request_min_start_interval_ms: Option<u64>,
    pub kiro_request_validation_enabled: Option<bool>,
    pub kiro_cache_estimation_enabled: Option<bool>,
    pub kiro_zero_cache_debug_enabled: Option<bool>,
    pub kiro_cache_policy_override_json: Option<Option<&'a str>>,
    pub kiro_billable_model_multipliers_override_json: Option<Option<&'a str>>,
    pub request_max_concurrency_unlimited: bool,
    pub request_min_start_interval_ms_unlimited: bool,
}

pub async fn patch_admin_llm_gateway_key(
    key_id: &str,
    request: PatchAdminLlmGatewayKeyRequest<'_>,
) -> Result<AdminLlmGatewayKeyView, String> {
    #[cfg(feature = "mock")]
    {
        let _ = (
            key_id,
            request.name,
            request.status,
            request.public_visible,
            request.quota_billable_limit,
            request.route_strategy,
            request.account_group_id,
            request.fixed_account_name,
            request.auto_account_names,
            request.model_name_map,
            request.request_max_concurrency,
            request.request_min_start_interval_ms,
            request.kiro_request_validation_enabled,
            request.kiro_cache_estimation_enabled,
            request.kiro_zero_cache_debug_enabled,
            request.kiro_cache_policy_override_json,
            request.kiro_billable_model_multipliers_override_json,
            request.request_max_concurrency_unlimited,
            request.request_min_start_interval_ms_unlimited,
        );
        Err("mock not supported".to_string())
    }

    #[cfg(not(feature = "mock"))]
    {
        let url =
            format!("{}/admin/llm-gateway/keys/{}", admin_base(), urlencoding::encode(key_id));
        let mut body = serde_json::Map::new();
        if let Some(name) = request
            .name
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            body.insert("name".to_string(), serde_json::Value::String(name.to_string()));
        }
        if let Some(status) = request
            .status
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            body.insert("status".to_string(), serde_json::Value::String(status.to_string()));
        }
        if let Some(public_visible) = request.public_visible {
            body.insert("public_visible".to_string(), serde_json::Value::Bool(public_visible));
        }
        if let Some(quota_billable_limit) = request.quota_billable_limit {
            body.insert(
                "quota_billable_limit".to_string(),
                serde_json::Value::Number(quota_billable_limit.into()),
            );
        }
        if let Some(strategy) = request.route_strategy {
            body.insert(
                "route_strategy".to_string(),
                serde_json::Value::String(strategy.to_string()),
            );
        }
        if let Some(group_id) = request.account_group_id {
            body.insert(
                "account_group_id".to_string(),
                serde_json::Value::String(group_id.to_string()),
            );
        }
        if let Some(account_name) = request.fixed_account_name {
            body.insert(
                "fixed_account_name".to_string(),
                serde_json::Value::String(account_name.to_string()),
            );
        }
        if let Some(account_names) = request.auto_account_names {
            body.insert(
                "auto_account_names".to_string(),
                serde_json::Value::Array(
                    account_names
                        .iter()
                        .map(|value| serde_json::Value::String(value.clone()))
                        .collect(),
                ),
            );
        }
        if let Some(model_name_map) = request.model_name_map {
            let value = serde_json::to_value(model_name_map)
                .map_err(|e| format!("Serialize error: {:?}", e))?;
            body.insert("model_name_map".to_string(), value);
        }
        if let Some(request_max_concurrency) = request.request_max_concurrency {
            body.insert(
                "request_max_concurrency".to_string(),
                serde_json::Value::Number(request_max_concurrency.into()),
            );
        }
        if let Some(request_min_start_interval_ms) = request.request_min_start_interval_ms {
            body.insert(
                "request_min_start_interval_ms".to_string(),
                serde_json::Value::Number(request_min_start_interval_ms.into()),
            );
        }
        if let Some(kiro_request_validation_enabled) = request.kiro_request_validation_enabled {
            body.insert(
                "kiro_request_validation_enabled".to_string(),
                serde_json::Value::Bool(kiro_request_validation_enabled),
            );
        }
        if let Some(kiro_cache_estimation_enabled) = request.kiro_cache_estimation_enabled {
            body.insert(
                "kiro_cache_estimation_enabled".to_string(),
                serde_json::Value::Bool(kiro_cache_estimation_enabled),
            );
        }
        if let Some(kiro_zero_cache_debug_enabled) = request.kiro_zero_cache_debug_enabled {
            body.insert(
                "kiro_zero_cache_debug_enabled".to_string(),
                serde_json::Value::Bool(kiro_zero_cache_debug_enabled),
            );
        }
        if let Some(kiro_cache_policy_override_json) = request.kiro_cache_policy_override_json {
            body.insert(
                "kiro_cache_policy_override_json".to_string(),
                kiro_cache_policy_override_json
                    .map(|raw| serde_json::Value::String(raw.to_string()))
                    .unwrap_or(serde_json::Value::Null),
            );
        }
        if let Some(kiro_billable_model_multipliers_override_json) =
            request.kiro_billable_model_multipliers_override_json
        {
            body.insert(
                "kiro_billable_model_multipliers_override_json".to_string(),
                kiro_billable_model_multipliers_override_json
                    .map(|raw| serde_json::Value::String(raw.to_string()))
                    .unwrap_or(serde_json::Value::Null),
            );
        }
        if request.request_max_concurrency_unlimited {
            body.insert(
                "request_max_concurrency_unlimited".to_string(),
                serde_json::Value::Bool(true),
            );
        }
        if request.request_min_start_interval_ms_unlimited {
            body.insert(
                "request_min_start_interval_ms_unlimited".to_string(),
                serde_json::Value::Bool(true),
            );
        }
        let response = api_patch(&url)
            .json(&serde_json::Value::Object(body))
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

/// Approve a token wish, issue the key, and email it to the requester.
pub async fn admin_approve_and_issue_llm_gateway_token_request(
    request_id: &str,
    admin_note: Option<&str>,
) -> Result<AdminLlmGatewayTokenRequestView, String> {
    #[cfg(feature = "mock")]
    {
        let _ = (request_id, admin_note);
        Err("mock not supported".to_string())
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!(
            "{}/admin/llm-gateway/token-requests/{}/approve-and-issue",
            admin_base(),
            urlencoding::encode(request_id)
        );
        let response = api_post(&url)
            .json(&serde_json::json!({ "admin_note": admin_note }))
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

/// Reject a token wish from the admin UI.
pub async fn admin_reject_llm_gateway_token_request(
    request_id: &str,
    admin_note: Option<&str>,
) -> Result<AdminLlmGatewayTokenRequestView, String> {
    #[cfg(feature = "mock")]
    {
        let _ = (request_id, admin_note);
        Err("mock not supported".to_string())
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!(
            "{}/admin/llm-gateway/token-requests/{}/reject",
            admin_base(),
            urlencoding::encode(request_id)
        );
        let response = api_post(&url)
            .json(&serde_json::json!({ "admin_note": admin_note }))
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

/// Approve an account contribution, import the account, issue a bound key,
/// and email it to the contributor.
pub async fn admin_approve_and_issue_llm_gateway_account_contribution_request(
    request_id: &str,
    admin_note: Option<&str>,
) -> Result<AdminLlmGatewayAccountContributionRequestView, String> {
    #[cfg(feature = "mock")]
    {
        let _ = (request_id, admin_note);
        Err("mock not supported".to_string())
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!(
            "{}/admin/llm-gateway/account-contribution-requests/{}/approve-and-issue",
            admin_base(),
            urlencoding::encode(request_id)
        );
        let response = api_post(&url)
            .json(&serde_json::json!({ "admin_note": admin_note }))
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

/// Reject an account contribution request from the admin UI.
pub async fn admin_reject_llm_gateway_account_contribution_request(
    request_id: &str,
    admin_note: Option<&str>,
) -> Result<AdminLlmGatewayAccountContributionRequestView, String> {
    #[cfg(feature = "mock")]
    {
        let _ = (request_id, admin_note);
        Err("mock not supported".to_string())
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!(
            "{}/admin/llm-gateway/account-contribution-requests/{}/reject",
            admin_base(),
            urlencoding::encode(request_id)
        );
        let response = api_post(&url)
            .json(&serde_json::json!({ "admin_note": admin_note }))
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

/// Mark a sponsor request as manually confirmed from the admin UI.
pub async fn admin_approve_llm_gateway_sponsor_request(
    request_id: &str,
    admin_note: Option<&str>,
) -> Result<AdminLlmGatewaySponsorRequestView, String> {
    #[cfg(feature = "mock")]
    {
        let _ = (request_id, admin_note);
        Err("mock not supported".to_string())
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!(
            "{}/admin/llm-gateway/sponsor-requests/{}/approve",
            admin_base(),
            urlencoding::encode(request_id)
        );
        let response = api_post(&url)
            .json(&serde_json::json!({ "admin_note": admin_note }))
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

/// Delete one sponsor request from the admin UI.
pub async fn delete_admin_llm_gateway_sponsor_request(request_id: &str) -> Result<(), String> {
    #[cfg(feature = "mock")]
    {
        let _ = request_id;
        Ok(())
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!(
            "{}/admin/llm-gateway/sponsor-requests/{}",
            admin_base(),
            urlencoding::encode(request_id)
        );
        let response = api_delete(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        Ok(())
    }
}

/// Delete a gateway key from the admin UI.
pub async fn delete_admin_llm_gateway_key(key_id: &str) -> Result<(), String> {
    #[cfg(feature = "mock")]
    {
        let _ = key_id;
        Ok(())
    }

    #[cfg(not(feature = "mock"))]
    {
        let url =
            format!("{}/admin/llm-gateway/keys/{}", admin_base(), urlencoding::encode(key_id));
        let response = api_delete(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        Ok(())
    }
}

/// Fetch a paginated slice of admin usage events with an optional key filter.
pub async fn fetch_admin_llm_gateway_usage_events(
    query: &AdminLlmGatewayUsageEventsQuery,
) -> Result<AdminLlmGatewayUsageEventsResponse, String> {
    #[cfg(feature = "mock")]
    {
        Ok(AdminLlmGatewayUsageEventsResponse {
            total: 0,
            offset: query.offset.unwrap_or(0),
            limit: query.limit.unwrap_or(50),
            has_more: false,
            current_rpm: 0,
            current_in_flight: 0,
            events: vec![],
            generated_at: 0,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let mut url = format!("{}/admin/llm-gateway/usage", admin_base());
        let mut params = Vec::new();
        if let Some(key_id) = query
            .key_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            params.push(format!("key_id={}", urlencoding::encode(key_id)));
        }
        if let Some(start_ms) = query.start_ms {
            params.push(format!("start_ms={start_ms}"));
        }
        if let Some(end_ms) = query.end_ms {
            params.push(format!("end_ms={end_ms}"));
        }
        if let Some(limit) = query.limit {
            params.push(format!("limit={limit}"));
        }
        if let Some(offset) = query.offset {
            params.push(format!("offset={offset}"));
        }
        if !params.is_empty() {
            url.push('?');
            url.push_str(&params.join("&"));
        }
        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn fetch_admin_llm_gateway_usage_event_detail(
    event_id: &str,
) -> Result<AdminLlmGatewayUsageEventDetailView, String> {
    #[cfg(feature = "mock")]
    {
        let _ = event_id;
        Ok(AdminLlmGatewayUsageEventDetailView::default())
    }

    #[cfg(not(feature = "mock"))]
    {
        let encoded = urlencoding::encode(event_id);
        let url = format!("{}/admin/llm-gateway/usage/{}", admin_base(), encoded);
        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

// === Account pool management ===

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct AccountSummaryView {
    pub name: String,
    pub status: String,
    pub account_id: Option<String>,
    pub plan_type: Option<String>,
    pub primary_remaining_percent: Option<f64>,
    pub secondary_remaining_percent: Option<f64>,
    pub map_gpt53_codex_to_spark: bool,
    pub request_max_concurrency: Option<u64>,
    pub request_min_start_interval_ms: Option<u64>,
    pub proxy_mode: String,
    pub proxy_config_id: Option<String>,
    pub effective_proxy_source: String,
    pub effective_proxy_url: Option<String>,
    pub effective_proxy_config_name: Option<String>,
    pub last_refresh: Option<i64>,
    pub last_usage_checked_at: Option<i64>,
    pub last_usage_success_at: Option<i64>,
    pub usage_error_message: Option<String>,
}

impl Default for AccountSummaryView {
    fn default() -> Self {
        Self {
            name: String::new(),
            status: String::new(),
            account_id: None,
            plan_type: None,
            primary_remaining_percent: None,
            secondary_remaining_percent: None,
            map_gpt53_codex_to_spark: false,
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
            proxy_mode: "inherit".to_string(),
            proxy_config_id: None,
            effective_proxy_source: "binding".to_string(),
            effective_proxy_url: None,
            effective_proxy_config_name: None,
            last_refresh: None,
            last_usage_checked_at: None,
            last_usage_success_at: None,
            usage_error_message: None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(default)]
pub struct AccountListResponse {
    pub accounts: Vec<AccountSummaryView>,
    pub generated_at: i64,
}

pub async fn fetch_admin_llm_gateway_accounts() -> Result<AccountListResponse, String> {
    #[cfg(feature = "mock")]
    {
        Ok(AccountListResponse {
            accounts: vec![],
            generated_at: 0,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/admin/llm-gateway/accounts", admin_base());
        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn import_admin_llm_gateway_account(
    name: &str,
    id_token: &str,
    access_token: &str,
    refresh_token: &str,
    account_id: Option<&str>,
) -> Result<AccountSummaryView, String> {
    #[cfg(feature = "mock")]
    {
        let _ = (id_token, access_token, refresh_token);
        Ok(AccountSummaryView {
            name: name.to_string(),
            status: "active".to_string(),
            account_id: account_id.map(str::to_string),
            plan_type: Some("Pro".to_string()),
            primary_remaining_percent: Some(100.0),
            secondary_remaining_percent: Some(100.0),
            map_gpt53_codex_to_spark: false,
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
            proxy_mode: "inherit".to_string(),
            proxy_config_id: None,
            effective_proxy_source: "binding".to_string(),
            effective_proxy_url: Some("http://127.0.0.1:11111".to_string()),
            effective_proxy_config_name: None,
            last_refresh: None,
            last_usage_checked_at: None,
            last_usage_success_at: None,
            usage_error_message: None,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/admin/llm-gateway/accounts", admin_base());
        let mut payload = serde_json::json!({
            "name": name,
            "tokens": {
                "id_token": id_token,
                "access_token": access_token,
                "refresh_token": refresh_token,
            }
        });
        if let Some(aid) = account_id {
            payload["tokens"]["account_id"] = serde_json::json!(aid);
        }
        let response = api_post(&url)
            .json(&payload)
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn delete_admin_llm_gateway_account(name: &str) -> Result<(), String> {
    #[cfg(feature = "mock")]
    {
        let _ = name;
        Ok(())
    }

    #[cfg(not(feature = "mock"))]
    {
        let url =
            format!("{}/admin/llm-gateway/accounts/{}", admin_base(), urlencoding::encode(name));
        let response = api_delete(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, Clone, PartialEq, Default)]
pub struct PatchAdminLlmGatewayAccountInput {
    pub map_gpt53_codex_to_spark: Option<bool>,
    pub proxy_mode: Option<String>,
    pub proxy_config_id: Option<String>,
    pub request_max_concurrency: Option<u64>,
    pub request_min_start_interval_ms: Option<u64>,
    pub request_max_concurrency_unlimited: bool,
    pub request_min_start_interval_ms_unlimited: bool,
}

pub async fn patch_admin_llm_gateway_account(
    name: &str,
    input: &PatchAdminLlmGatewayAccountInput,
) -> Result<AccountSummaryView, String> {
    #[cfg(feature = "mock")]
    {
        Ok(AccountSummaryView {
            name: name.to_string(),
            status: "active".to_string(),
            account_id: None,
            plan_type: Some("Pro".to_string()),
            primary_remaining_percent: Some(100.0),
            secondary_remaining_percent: Some(100.0),
            map_gpt53_codex_to_spark: input.map_gpt53_codex_to_spark.unwrap_or(false),
            request_max_concurrency: input.request_max_concurrency,
            request_min_start_interval_ms: input.request_min_start_interval_ms,
            proxy_mode: input
                .proxy_mode
                .clone()
                .unwrap_or_else(|| "inherit".to_string()),
            proxy_config_id: input.proxy_config_id.clone(),
            effective_proxy_source: "binding".to_string(),
            effective_proxy_url: Some("http://127.0.0.1:11111".to_string()),
            effective_proxy_config_name: None,
            last_refresh: None,
            last_usage_checked_at: None,
            last_usage_success_at: None,
            usage_error_message: None,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url =
            format!("{}/admin/llm-gateway/accounts/{}", admin_base(), urlencoding::encode(name));
        let response = api_patch(&url)
            .json(input)
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn refresh_admin_llm_gateway_account(name: &str) -> Result<AccountSummaryView, String> {
    #[cfg(feature = "mock")]
    {
        Ok(AccountSummaryView {
            name: name.to_string(),
            status: "active".to_string(),
            account_id: None,
            plan_type: Some("Pro".to_string()),
            primary_remaining_percent: Some(100.0),
            secondary_remaining_percent: Some(100.0),
            map_gpt53_codex_to_spark: false,
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
            proxy_mode: "inherit".to_string(),
            proxy_config_id: None,
            effective_proxy_source: "binding".to_string(),
            effective_proxy_url: Some("http://127.0.0.1:11111".to_string()),
            effective_proxy_config_name: None,
            last_refresh: None,
            last_usage_checked_at: None,
            last_usage_success_at: None,
            usage_error_message: None,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!(
            "{}/admin/llm-gateway/accounts/{}/refresh",
            admin_base(),
            urlencoding::encode(name)
        );
        let response = api_post(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(default)]
pub struct KiroBalanceView {
    pub current_usage: f64,
    pub usage_limit: f64,
    pub remaining: f64,
    pub next_reset_at: Option<i64>,
    pub subscription_title: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(default)]
pub struct KiroPublicStatusView {
    pub name: String,
    pub provider: Option<String>,
    pub disabled: bool,
    pub disabled_reason: Option<String>,
    pub subscription_title: Option<String>,
    pub current_usage: Option<f64>,
    pub usage_limit: Option<f64>,
    pub remaining: Option<f64>,
    pub next_reset_at: Option<i64>,
    pub cache: KiroCacheView,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(default)]
pub struct KiroModelView {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub owned_by: String,
    pub display_name: String,
    #[serde(rename = "type")]
    pub model_type: String,
    pub max_tokens: i32,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(default)]
pub struct KiroModelsResponse {
    pub object: String,
    pub data: Vec<KiroModelView>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(default)]
pub struct KiroAccessResponse {
    pub base_url: String,
    pub gateway_path: String,
    pub auth_cache_ttl_seconds: u64,
    pub accounts: Vec<KiroPublicStatusView>,
    pub generated_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(default)]
pub struct KiroCacheView {
    pub status: String,
    pub refresh_interval_seconds: u64,
    pub last_checked_at: Option<i64>,
    pub last_success_at: Option<i64>,
    pub error_message: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(default)]
pub struct KiroAccountView {
    pub name: String,
    pub auth_method: String,
    pub provider: Option<String>,
    pub email: Option<String>,
    pub expires_at: Option<String>,
    pub profile_arn: Option<String>,
    pub has_refresh_token: bool,
    pub disabled: bool,
    pub disabled_reason: Option<String>,
    pub source: Option<String>,
    pub source_db_path: Option<String>,
    pub last_imported_at: Option<i64>,
    pub subscription_title: Option<String>,
    pub region: Option<String>,
    pub auth_region: Option<String>,
    pub api_region: Option<String>,
    pub machine_id: Option<String>,
    pub kiro_channel_max_concurrency: u64,
    pub kiro_channel_min_start_interval_ms: u64,
    pub minimum_remaining_credits_before_block: f64,
    pub proxy_mode: String,
    pub proxy_config_id: Option<String>,
    pub effective_proxy_source: String,
    pub effective_proxy_url: Option<String>,
    pub effective_proxy_config_name: Option<String>,
    pub proxy_url: Option<String>,
    pub balance: Option<KiroBalanceView>,
    pub cache: KiroCacheView,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct CreateManualKiroAccountInput {
    pub name: String,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub profile_arn: Option<String>,
    pub expires_at: Option<String>,
    pub auth_method: Option<String>,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub region: Option<String>,
    pub auth_region: Option<String>,
    pub api_region: Option<String>,
    pub machine_id: Option<String>,
    pub provider: Option<String>,
    pub email: Option<String>,
    pub subscription_title: Option<String>,
    pub kiro_channel_max_concurrency: Option<u64>,
    pub kiro_channel_min_start_interval_ms: Option<u64>,
    pub minimum_remaining_credits_before_block: Option<f64>,
    pub disabled: bool,
}

#[derive(Debug, Serialize, Clone, PartialEq, Default)]
pub struct PatchKiroAccountInput {
    pub kiro_channel_max_concurrency: Option<u64>,
    pub kiro_channel_min_start_interval_ms: Option<u64>,
    pub minimum_remaining_credits_before_block: Option<f64>,
    pub proxy_mode: Option<String>,
    pub proxy_config_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(default)]
pub struct AdminKiroAccountsResponse {
    pub accounts: Vec<KiroAccountView>,
    pub generated_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(default)]
pub struct AdminKiroAccountStatusesResponse {
    pub accounts: Vec<KiroAccountView>,
    pub total: usize,
    pub limit: usize,
    pub offset: usize,
    pub generated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct AdminKiroAccountStatusesQuery {
    pub prefix: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

#[cfg(any(not(feature = "mock"), test))]
fn build_admin_kiro_account_statuses_url(query: &AdminKiroAccountStatusesQuery) -> String {
    let mut url = format!("{}/admin/kiro-gateway/accounts/statuses", admin_base());
    let mut params = Vec::new();
    if let Some(prefix) = query.prefix.as_deref() {
        params.push(format!("prefix={}", urlencoding::encode(prefix)));
    }
    if let Some(limit) = query.limit {
        params.push(format!("limit={limit}"));
    }
    if let Some(offset) = query.offset {
        params.push(format!("offset={offset}"));
    }
    if !params.is_empty() {
        url.push('?');
        url.push_str(&params.join("&"));
    }
    url
}

pub async fn fetch_kiro_access() -> Result<KiroAccessResponse, String> {
    #[cfg(feature = "mock")]
    {
        Ok(KiroAccessResponse {
            base_url: "http://localhost:3000/api/kiro-gateway".to_string(),
            gateway_path: "/api/kiro-gateway".to_string(),
            auth_cache_ttl_seconds: 60,
            accounts: vec![],
            generated_at: 0,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/kiro-gateway/access?_ts={}", API_BASE, Date::now() as u64);
        let response = api_get(&url)
            .header("Cache-Control", "no-cache, no-store, max-age=0")
            .header("Pragma", "no-cache")
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn fetch_kiro_models() -> Result<KiroModelsResponse, String> {
    #[cfg(feature = "mock")]
    {
        Ok(KiroModelsResponse {
            object: "list".to_string(),
            data: vec![
                KiroModelView {
                    id: "claude-sonnet-4-6".to_string(),
                    object: "model".to_string(),
                    created: 1_770_314_400,
                    owned_by: "anthropic".to_string(),
                    display_name: "Claude Sonnet 4.6".to_string(),
                    model_type: "chat".to_string(),
                    max_tokens: 32_000,
                },
                KiroModelView {
                    id: "claude-haiku-4-5-20251001".to_string(),
                    object: "model".to_string(),
                    created: 1_727_740_800,
                    owned_by: "anthropic".to_string(),
                    display_name: "Claude Haiku 4.5".to_string(),
                    model_type: "chat".to_string(),
                    max_tokens: 32_000,
                },
            ],
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/kiro-gateway/v1/models", API_BASE);
        let response = api_get(&url)
            .header("Cache-Control", "no-cache, no-store, max-age=0")
            .header("Pragma", "no-cache")
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn fetch_admin_kiro_keys() -> Result<AdminLlmGatewayKeysResponse, String> {
    #[cfg(feature = "mock")]
    {
        Ok(AdminLlmGatewayKeysResponse {
            keys: vec![],
            auth_cache_ttl_seconds: 60,
            generated_at: 0,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/admin/kiro-gateway/keys?_ts={}", admin_base(), Date::now() as u64);
        let response = api_get(&url)
            .header("Cache-Control", "no-cache, no-store, max-age=0")
            .header("Pragma", "no-cache")
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn fetch_admin_kiro_account_groups() -> Result<AdminAccountGroupsResponse, String> {
    #[cfg(feature = "mock")]
    {
        Ok(AdminAccountGroupsResponse::default())
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/admin/kiro-gateway/account-groups", admin_base());
        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn create_admin_kiro_account_group(
    input: CreateAdminAccountGroupInput<'_>,
) -> Result<AdminAccountGroupView, String> {
    #[cfg(feature = "mock")]
    {
        Ok(AdminAccountGroupView {
            id: "mock-kiro-group".to_string(),
            provider_type: "kiro".to_string(),
            name: input.name.to_string(),
            account_names: input.account_names.to_vec(),
            created_at: 0,
            updated_at: 0,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/admin/kiro-gateway/account-groups", admin_base());
        let response = api_post(&url)
            .json(&serde_json::json!({
                "name": input.name,
                "account_names": input.account_names,
            }))
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn patch_admin_kiro_account_group(
    group_id: &str,
    input: PatchAdminAccountGroupInput<'_>,
) -> Result<AdminAccountGroupView, String> {
    #[cfg(feature = "mock")]
    {
        Ok(AdminAccountGroupView {
            id: group_id.to_string(),
            provider_type: "kiro".to_string(),
            name: input.name.unwrap_or("mock").to_string(),
            account_names: input
                .account_names
                .map(|value| value.to_vec())
                .unwrap_or_default(),
            created_at: 0,
            updated_at: 0,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!(
            "{}/admin/kiro-gateway/account-groups/{}",
            admin_base(),
            urlencoding::encode(group_id)
        );
        let mut body = serde_json::Map::new();
        if let Some(name) = input.name.map(str::trim).filter(|value| !value.is_empty()) {
            body.insert("name".to_string(), serde_json::Value::String(name.to_string()));
        }
        if let Some(account_names) = input.account_names {
            body.insert(
                "account_names".to_string(),
                serde_json::Value::Array(
                    account_names
                        .iter()
                        .map(|value| serde_json::Value::String(value.clone()))
                        .collect(),
                ),
            );
        }
        let response = api_patch(&url)
            .json(&serde_json::Value::Object(body))
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn delete_admin_kiro_account_group(group_id: &str) -> Result<(), String> {
    #[cfg(feature = "mock")]
    {
        let _ = group_id;
        Ok(())
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!(
            "{}/admin/kiro-gateway/account-groups/{}",
            admin_base(),
            urlencoding::encode(group_id)
        );
        let response = api_delete(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        Ok(())
    }
}

pub async fn create_admin_kiro_key(
    name: &str,
    quota_billable_limit: u64,
) -> Result<AdminLlmGatewayKeyView, String> {
    #[cfg(feature = "mock")]
    {
        Ok(AdminLlmGatewayKeyView {
            id: "mock-kiro".to_string(),
            name: name.to_string(),
            secret: "sf-kiro-mock".to_string(),
            key_hash: "hash".to_string(),
            status: "active".to_string(),
            provider_type: "kiro".to_string(),
            public_visible: false,
            quota_billable_limit,
            usage_input_uncached_tokens: 0,
            usage_input_cached_tokens: 0,
            usage_output_tokens: 0,
            usage_credit_total: 0.0,
            usage_credit_missing_events: 0,
            remaining_billable: quota_billable_limit as i64,
            last_used_at: None,
            created_at: 0,
            updated_at: 0,
            route_strategy: None,
            account_group_id: None,
            fixed_account_name: None,
            auto_account_names: None,
            model_name_map: None,
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
            kiro_request_validation_enabled: true,
            kiro_cache_estimation_enabled: true,
            kiro_zero_cache_debug_enabled: false,
            kiro_cache_policy_override_json: None,
            kiro_billable_model_multipliers_override_json: None,
            effective_kiro_cache_policy_json: String::new(),
            uses_global_kiro_cache_policy: true,
            effective_kiro_billable_model_multipliers_json:
                default_kiro_billable_model_multipliers_json(),
            uses_global_kiro_billable_model_multipliers: true,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/admin/kiro-gateway/keys", admin_base());
        let response = api_post(&url)
            .json(&serde_json::json!({
                "name": name,
                "quota_billable_limit": quota_billable_limit
            }))
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn patch_admin_kiro_key(
    key_id: &str,
    request: PatchAdminLlmGatewayKeyRequest<'_>,
) -> Result<AdminLlmGatewayKeyView, String> {
    #[cfg(feature = "mock")]
    {
        let _ = (
            key_id,
            request.name,
            request.status,
            request.public_visible,
            request.quota_billable_limit,
            request.route_strategy,
            request.account_group_id,
            request.fixed_account_name,
            request.auto_account_names,
            request.model_name_map,
            request.request_max_concurrency,
            request.request_min_start_interval_ms,
            request.kiro_request_validation_enabled,
            request.kiro_cache_estimation_enabled,
            request.kiro_zero_cache_debug_enabled,
            request.kiro_cache_policy_override_json,
            request.kiro_billable_model_multipliers_override_json,
            request.request_max_concurrency_unlimited,
            request.request_min_start_interval_ms_unlimited,
        );
        Err("mock not supported".to_string())
    }

    #[cfg(not(feature = "mock"))]
    {
        let url =
            format!("{}/admin/kiro-gateway/keys/{}", admin_base(), urlencoding::encode(key_id));
        let mut body = serde_json::Map::new();
        if let Some(name) = request
            .name
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            body.insert("name".to_string(), serde_json::Value::String(name.to_string()));
        }
        if let Some(status) = request
            .status
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            body.insert("status".to_string(), serde_json::Value::String(status.to_string()));
        }
        if let Some(public_visible) = request.public_visible {
            body.insert("public_visible".to_string(), serde_json::Value::Bool(public_visible));
        }
        if let Some(quota_billable_limit) = request.quota_billable_limit {
            body.insert(
                "quota_billable_limit".to_string(),
                serde_json::Value::Number(quota_billable_limit.into()),
            );
        }
        if let Some(strategy) = request.route_strategy {
            body.insert(
                "route_strategy".to_string(),
                serde_json::Value::String(strategy.to_string()),
            );
        }
        if let Some(group_id) = request.account_group_id {
            body.insert(
                "account_group_id".to_string(),
                serde_json::Value::String(group_id.to_string()),
            );
        }
        if let Some(account_name) = request.fixed_account_name {
            body.insert(
                "fixed_account_name".to_string(),
                serde_json::Value::String(account_name.to_string()),
            );
        }
        if let Some(account_names) = request.auto_account_names {
            body.insert(
                "auto_account_names".to_string(),
                serde_json::Value::Array(
                    account_names
                        .iter()
                        .map(|value| serde_json::Value::String(value.clone()))
                        .collect(),
                ),
            );
        }
        if let Some(model_name_map) = request.model_name_map {
            let value = serde_json::to_value(model_name_map)
                .map_err(|e| format!("Serialize error: {:?}", e))?;
            body.insert("model_name_map".to_string(), value);
        }
        if let Some(kiro_request_validation_enabled) = request.kiro_request_validation_enabled {
            body.insert(
                "kiro_request_validation_enabled".to_string(),
                serde_json::Value::Bool(kiro_request_validation_enabled),
            );
        }
        if let Some(kiro_cache_estimation_enabled) = request.kiro_cache_estimation_enabled {
            body.insert(
                "kiro_cache_estimation_enabled".to_string(),
                serde_json::Value::Bool(kiro_cache_estimation_enabled),
            );
        }
        if let Some(kiro_zero_cache_debug_enabled) = request.kiro_zero_cache_debug_enabled {
            body.insert(
                "kiro_zero_cache_debug_enabled".to_string(),
                serde_json::Value::Bool(kiro_zero_cache_debug_enabled),
            );
        }
        if let Some(kiro_cache_policy_override_json) = request.kiro_cache_policy_override_json {
            body.insert(
                "kiro_cache_policy_override_json".to_string(),
                kiro_cache_policy_override_json
                    .map(|raw| serde_json::Value::String(raw.to_string()))
                    .unwrap_or(serde_json::Value::Null),
            );
        }
        if let Some(kiro_billable_model_multipliers_override_json) =
            request.kiro_billable_model_multipliers_override_json
        {
            body.insert(
                "kiro_billable_model_multipliers_override_json".to_string(),
                kiro_billable_model_multipliers_override_json
                    .map(|raw| serde_json::Value::String(raw.to_string()))
                    .unwrap_or(serde_json::Value::Null),
            );
        }
        let response = api_patch(&url)
            .json(&serde_json::Value::Object(body))
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn delete_admin_kiro_key(key_id: &str) -> Result<(), String> {
    #[cfg(feature = "mock")]
    {
        let _ = key_id;
        Ok(())
    }

    #[cfg(not(feature = "mock"))]
    {
        let url =
            format!("{}/admin/kiro-gateway/keys/{}", admin_base(), urlencoding::encode(key_id));
        let response = api_delete(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        Ok(())
    }
}

pub async fn fetch_admin_kiro_usage_events(
    query: &AdminLlmGatewayUsageEventsQuery,
) -> Result<AdminLlmGatewayUsageEventsResponse, String> {
    #[cfg(feature = "mock")]
    {
        let _ = query;
        Ok(AdminLlmGatewayUsageEventsResponse {
            total: 0,
            offset: 0,
            limit: 20,
            has_more: false,
            current_rpm: 0,
            current_in_flight: 0,
            events: vec![],
            generated_at: 0,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let mut url = format!("{}/admin/kiro-gateway/usage", admin_base());
        let mut params = Vec::new();
        if let Some(key_id) = query.key_id.as_deref() {
            params.push(format!("key_id={}", urlencoding::encode(key_id)));
        }
        if let Some(start_ms) = query.start_ms {
            params.push(format!("start_ms={start_ms}"));
        }
        if let Some(end_ms) = query.end_ms {
            params.push(format!("end_ms={end_ms}"));
        }
        if let Some(limit) = query.limit {
            params.push(format!("limit={limit}"));
        }
        if let Some(offset) = query.offset {
            params.push(format!("offset={offset}"));
        }
        if !params.is_empty() {
            url.push('?');
            url.push_str(&params.join("&"));
        }
        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn fetch_admin_kiro_accounts() -> Result<AdminKiroAccountsResponse, String> {
    #[cfg(feature = "mock")]
    {
        Ok(AdminKiroAccountsResponse {
            accounts: vec![],
            generated_at: 0,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/admin/kiro-gateway/accounts", admin_base());
        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn fetch_admin_kiro_account_statuses(
    query: &AdminKiroAccountStatusesQuery,
) -> Result<AdminKiroAccountStatusesResponse, String> {
    #[cfg(feature = "mock")]
    {
        Ok(AdminKiroAccountStatusesResponse {
            accounts: vec![],
            total: 0,
            limit: query.limit.unwrap_or(24),
            offset: query.offset.unwrap_or(0),
            generated_at: 0,
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = build_admin_kiro_account_statuses_url(query);
        let response = api_get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn import_admin_kiro_account(
    name: Option<&str>,
    sqlite_path: Option<&str>,
    kiro_channel_max_concurrency: Option<u64>,
    kiro_channel_min_start_interval_ms: Option<u64>,
) -> Result<KiroAccountView, String> {
    #[cfg(feature = "mock")]
    {
        let _ =
            (name, sqlite_path, kiro_channel_max_concurrency, kiro_channel_min_start_interval_ms);
        Err("mock not supported".to_string())
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/admin/kiro-gateway/accounts/import-local", admin_base());
        let mut body = serde_json::Map::new();
        if let Some(name) = name.map(str::trim).filter(|value| !value.is_empty()) {
            body.insert("name".to_string(), serde_json::Value::String(name.to_string()));
        }
        if let Some(path) = sqlite_path.map(str::trim).filter(|value| !value.is_empty()) {
            body.insert("sqlite_path".to_string(), serde_json::Value::String(path.to_string()));
        }
        if let Some(value) = kiro_channel_max_concurrency {
            body.insert(
                "kiro_channel_max_concurrency".to_string(),
                serde_json::Value::Number(serde_json::Number::from(value)),
            );
        }
        if let Some(value) = kiro_channel_min_start_interval_ms {
            body.insert(
                "kiro_channel_min_start_interval_ms".to_string(),
                serde_json::Value::Number(serde_json::Number::from(value)),
            );
        }
        let response = api_post(&url)
            .json(&serde_json::Value::Object(body))
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn create_admin_kiro_manual_account(
    input: &CreateManualKiroAccountInput,
) -> Result<KiroAccountView, String> {
    #[cfg(feature = "mock")]
    {
        let _ = input;
        Err("mock not supported".to_string())
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/admin/kiro-gateway/accounts", admin_base());
        let response = api_post(&url)
            .json(input)
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn patch_admin_kiro_account(
    name: &str,
    input: &PatchKiroAccountInput,
) -> Result<KiroAccountView, String> {
    #[cfg(feature = "mock")]
    {
        let _ = (name, input);
        Err("mock not supported".to_string())
    }

    #[cfg(not(feature = "mock"))]
    {
        let url =
            format!("{}/admin/kiro-gateway/accounts/{}", admin_base(), urlencoding::encode(name));
        let response = api_patch(&url)
            .json(input)
            .map_err(|e| format!("Serialize error: {:?}", e))?
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn refresh_admin_kiro_account_balance(name: &str) -> Result<KiroBalanceView, String> {
    #[cfg(feature = "mock")]
    {
        Ok(KiroBalanceView {
            current_usage: 0.0,
            usage_limit: 1_000.0,
            remaining: 1_000.0,
            next_reset_at: None,
            subscription_title: Some(format!("mock-{name}")),
        })
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!(
            "{}/admin/kiro-gateway/accounts/{}/balance",
            admin_base(),
            urlencoding::encode(name)
        );
        let response = api_post(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        response
            .json()
            .await
            .map_err(|e| format!("Parse error: {:?}", e))
    }
}

pub async fn delete_admin_kiro_account(name: &str) -> Result<(), String> {
    #[cfg(feature = "mock")]
    {
        let _ = name;
        Ok(())
    }

    #[cfg(not(feature = "mock"))]
    {
        let url =
            format!("{}/admin/kiro-gateway/accounts/{}", admin_base(), urlencoding::encode(name));
        let response = api_delete(&url)
            .send()
            .await
            .map_err(|e| format!("Network error: {:?}", e))?;
        if !response.ok() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed: {text}"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_generation_request_serializes_b64_response_format() {
        let value = serde_json::to_value(AdminGpt2ApiRsImageGenerationRequest::default())
            .expect("request should serialize");

        assert_eq!(value.get("response_format"), Some(&serde_json::json!("b64_json")));
    }

    #[test]
    fn admin_kiro_account_statuses_response_defaults_are_empty() {
        let response: AdminKiroAccountStatusesResponse =
            serde_json::from_str("{}").expect("response should parse");

        assert!(response.accounts.is_empty());
        assert_eq!(response.total, 0);
        assert_eq!(response.limit, 0);
        assert_eq!(response.offset, 0);
    }

    #[test]
    fn build_admin_kiro_account_statuses_url_encodes_prefix_and_window() {
        let url = build_admin_kiro_account_statuses_url(&AdminKiroAccountStatusesQuery {
            prefix: Some("alpha team".to_string()),
            limit: Some(24),
            offset: Some(48),
        });

        assert!(url.contains("/admin/kiro-gateway/accounts/statuses"));
        assert!(url.contains("prefix=alpha%20team"));
        assert!(url.contains("limit=24"));
        assert!(url.contains("offset=48"));
    }

    #[test]
    fn build_llm_gateway_model_catalog_url_uses_public_api_prefix() {
        let url = build_llm_gateway_model_catalog_url_for_ts(
            Some("/llm-gateway/model-catalog.json"),
            123,
        );

        assert!(url.contains("/api/llm-gateway/model-catalog.json"));
        assert!(url.contains("_ts=123"));
    }

    #[test]
    fn derive_local_media_api_base_from_http_api_base_uses_backend_origin() {
        let base = derive_local_media_api_base_from_api_base("http://127.0.0.1:39080/api");
        assert_eq!(base, "http://127.0.0.1:39080/admin/local-media/api");
    }

    #[test]
    fn derive_local_media_api_base_from_same_origin_falls_back_to_relative_admin_path() {
        let base = derive_local_media_api_base_from_api_base("");
        assert_eq!(base, "/admin/local-media/api");
    }

    #[test]
    fn resolve_local_media_asset_url_for_base_rewrites_relative_admin_asset_to_backend_origin() {
        let url = resolve_local_media_asset_url_for_base(
            "http://127.0.0.1:39080/admin/local-media/api",
            "/admin/local-media/api/poster?file=demo.mp4",
        );
        assert_eq!(url, "http://127.0.0.1:39080/admin/local-media/api/poster?file=demo.mp4");
    }

    #[test]
    fn resolve_local_media_asset_url_for_base_keeps_same_origin_relative_path_when_base_is_relative(
    ) {
        let url = resolve_local_media_asset_url_for_base(
            "/admin/local-media/api",
            "/admin/local-media/api/playback/raw?file=demo.mp4",
        );
        assert_eq!(url, "/admin/local-media/api/playback/raw?file=demo.mp4");
    }

    #[test]
    #[cfg(not(feature = "mock"))]
    fn build_admin_local_media_raw_playback_uses_raw_mode_and_encoded_url() {
        let response = build_admin_local_media_raw_playback("未归类/demo clip.mp4");
        assert_eq!(response.status, LocalMediaPlaybackStatus::Ready);
        assert_eq!(response.mode, Some(LocalMediaPlaybackMode::Raw));
        assert_eq!(response.title, "demo clip.mp4");
        assert!(response
            .player_url
            .as_deref()
            .unwrap_or_default()
            .contains("playback/raw?file=%E6%9C%AA%E5%BD%92%E7%B1%BB%2Fdemo%20clip.mp4"));
    }

    #[test]
    fn build_admin_local_media_upload_tasks_url_uses_admin_prefix() {
        assert!(build_admin_local_media_upload_tasks_url()
            .ends_with("/admin/local-media/api/uploads/tasks"));
    }

    #[test]
    fn compaction_runtime_config_deserializes_worker_count() {
        let config: CompactionRuntimeConfig = serde_json::from_str(
            r#"{
                "enabled": true,
                "scan_interval_seconds": 900,
                "fragment_threshold": 128,
                "prune_older_than_hours": 1,
                "worker_count": 4
            }"#,
        )
        .expect("compaction config should parse");

        assert_eq!(config.worker_count, 4);
    }

    #[test]
    fn music_runtime_config_deserializes_admin_payload() {
        let config: MusicRuntimeConfig = serde_json::from_str(
            r#"{
                "play_dedupe_window_seconds": 60,
                "comment_rate_limit_seconds": 90,
                "list_default_limit": 20
            }"#,
        )
        .expect("music config should parse");

        assert_eq!(config.play_dedupe_window_seconds, 60);
        assert_eq!(config.comment_rate_limit_seconds, 90);
        assert_eq!(config.list_default_limit, 20);
    }

    #[test]
    fn llm_gateway_runtime_config_ignores_legacy_usage_maintenance_fields() {
        let config: LlmGatewayRuntimeConfig = serde_json::from_str(
            r#"{
                "auth_cache_ttl_seconds": 60,
                "max_request_body_bytes": 8388608,
                "account_failure_retry_limit": 3,
                "kiro_channel_max_concurrency": 1,
                "kiro_channel_min_start_interval_ms": 0,
                "codex_status_refresh_min_interval_seconds": 240,
                "codex_status_refresh_max_interval_seconds": 300,
                "codex_status_account_jitter_max_seconds": 10,
                "kiro_status_refresh_min_interval_seconds": 240,
                "kiro_status_refresh_max_interval_seconds": 300,
                "kiro_status_account_jitter_max_seconds": 10,
                "usage_event_flush_batch_size": 256,
                "usage_event_flush_interval_seconds": 15,
                "usage_event_flush_max_buffer_bytes": 8388608,
                "usage_event_maintenance_enabled": true,
                "usage_event_maintenance_interval_seconds": 3600,
                "usage_event_detail_retention_days": 7,
                "kiro_cache_kmodels_json": "{}",
                "kiro_billable_model_multipliers_json": "{\"haiku\":1.0,\"opus\":1.0,\"sonnet\":1.0}",
                "kiro_cache_policy_json": "{}",
                "kiro_prefix_cache_mode": "prefix_tree",
                "kiro_prefix_cache_max_tokens": 4000000,
                "kiro_prefix_cache_entry_ttl_seconds": 21600,
                "kiro_conversation_anchor_max_entries": 20000,
                "kiro_conversation_anchor_ttl_seconds": 86400
            }"#,
        )
        .expect("llm gateway runtime config should parse");

        assert_eq!(config.usage_event_flush_interval_seconds, 15);
        assert_eq!(config.codex_client_version, DEFAULT_LLM_GATEWAY_CODEX_CLIENT_VERSION);
    }
}
