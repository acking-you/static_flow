#[cfg(not(feature = "mock"))]
use gloo_net::http::Request;
#[cfg(not(feature = "mock"))]
use js_sys::Date;
use serde::{Deserialize, Serialize};
use static_flow_shared::{Article, ArticleListItem};
#[cfg(not(feature = "mock"))]
use wasm_bindgen::JsValue;

#[cfg(feature = "mock")]
use crate::models;

// API base URL - 编译时从环境变量读取，默认本地开发地址
// 生产环境通过 workflow 设置 STATICFLOW_API_BASE 环境变量
#[cfg(not(feature = "mock"))]
pub const API_BASE: &str = match option_env!("STATICFLOW_API_BASE") {
    Some(url) => url,
    None => "http://localhost:3000/api",
};

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
struct ArticleListResponse {
    articles: Vec<ArticleListItem>,
    #[allow(dead_code)]
    total: usize,
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

/// 获取文章列表，支持按标签和分类过滤
pub async fn fetch_articles(
    tag: Option<&str>,
    category: Option<&str>,
) -> Result<Vec<ArticleListItem>, String> {
    #[cfg(feature = "mock")]
    {
        let mut articles = models::get_mock_articles();

        if let Some(t) = tag {
            articles = articles
                .into_iter()
                .filter(|article| article.tags.iter().any(|tag| tag.eq_ignore_ascii_case(t)))
                .collect();
        }

        if let Some(c) = category {
            articles = articles
                .into_iter()
                .filter(|article| article.category.eq_ignore_ascii_case(c))
                .collect();
        }

        return Ok(articles);
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
        params.push(format!("_ts={}", Date::now() as u64));

        if !params.is_empty() {
            url.push('?');
            url.push_str(&params.join("&"));
        }

        let response = Request::get(&url)
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

        Ok(json_response.articles)
    }
}

/// 获取文章详情
pub async fn fetch_article_detail(id: &str) -> Result<Option<Article>, String> {
    #[cfg(feature = "mock")]
    {
        return Ok(models::get_mock_article_detail(id));
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/articles/{}?_ts={}", API_BASE, id, Date::now() as u64);

        let response = Request::get(&url)
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
        return Ok(content);
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

        let response = Request::get(&url)
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
        return Ok(ArticleViewTrackResponse {
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
        });
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/articles/{}/view", API_BASE, urlencoding::encode(id));
        let response = Request::post(&url)
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
        return Ok(ArticleViewTrendResponse {
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
        });
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

        let response = Request::get(&url)
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
        return Ok(models::mock_tags());
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/tags", API_BASE);

        let response = Request::get(&url)
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
        return Ok(models::mock_categories());
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/categories", API_BASE);

        let response = Request::get(&url)
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

        return Ok(SiteStats {
            total_articles: articles.len(),
            total_tags: tags.len(),
            total_categories: categories.len(),
        });
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/stats", API_BASE);

        let response = Request::get(&url)
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
#[allow(dead_code)]
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
#[allow(dead_code)]
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
#[allow(dead_code)]
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
#[allow(dead_code)]
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
        return Ok(results);
    }

    #[cfg(not(feature = "mock"))]
    {
        let mut url = format!("{}/search?q={}", API_BASE, urlencoding::encode(keyword));
        if let Some(limit) = limit {
            url.push_str(&format!("&limit={limit}"));
        }

        let response = Request::get(&url)
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
        let mut results = models::mock_search(keyword);
        if let Some(limit) = limit {
            results.truncate(limit);
        }
        return Ok(results);
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

        let response = Request::get(&url)
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
        return Ok(articles
            .into_iter()
            .filter(|a| a.id != id)
            .take(3)
            .collect());
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/articles/{}/related", API_BASE, id);

        let response = Request::get(&url)
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
#[allow(dead_code)]
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
        return Ok(ImagePage {
            images: vec![],
            total: 0,
            offset: offset.unwrap_or(0),
            limit: limit.unwrap_or(0),
            has_more: false,
        });
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

        let response = Request::get(&url)
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
#[allow(dead_code)]
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
        return Ok(ImagePage {
            images: vec![],
            total: 0,
            offset: offset.unwrap_or(0),
            limit: limit.unwrap_or(0),
            has_more: false,
        });
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

        let response = Request::get(&url)
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
#[allow(dead_code)]
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
        return Ok(ImagePage {
            images: vec![],
            total: 0,
            offset: offset.unwrap_or(0),
            limit: limit.unwrap_or(0),
            has_more: false,
        });
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

        let response = Request::get(&url)
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
pub struct ViewAnalyticsConfig {
    pub dedupe_window_seconds: u64,
    pub trend_default_days: usize,
    pub trend_max_days: usize,
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
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct AdminCommentPublishedResponse {
    pub comments: Vec<ArticleComment>,
    pub total: usize,
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

#[cfg(not(feature = "mock"))]
fn admin_base() -> String {
    API_BASE
        .strip_suffix("/api")
        .map(str::to_string)
        .unwrap_or_else(|| API_BASE.to_string())
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
        return url;
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
        return Ok(SubmitCommentResponse {
            task_id: format!("mock-task-{}", Date::now() as u64),
            status: "pending".to_string(),
        });
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/comments/submit", API_BASE);
        let response = Request::post(&url)
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
        return Ok(CommentListResponse {
            comments: vec![],
            total: 0,
            article_id: article_id.to_string(),
        });
    }

    #[cfg(not(feature = "mock"))]
    {
        let mut url =
            format!("{}/comments/list?article_id={}", API_BASE, urlencoding::encode(article_id),);
        if let Some(limit) = limit {
            url.push_str(&format!("&limit={limit}"));
        }

        let response = Request::get(&url)
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
        return Ok(CommentStatsResponse {
            article_id: article_id.to_string(),
            total: 0,
        });
    }

    #[cfg(not(feature = "mock"))]
    {
        let url =
            format!("{}/comments/stats?article_id={}", API_BASE, urlencoding::encode(article_id),);
        let response = Request::get(&url)
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
        return Ok(ViewAnalyticsConfig {
            dedupe_window_seconds: 60,
            trend_default_days: 30,
            trend_max_days: 180,
        });
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/admin/view-analytics-config", admin_base());
        let response = Request::get(&url)
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
        return Ok(config.clone());
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/admin/view-analytics-config", admin_base());
        let response = Request::post(&url)
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
        return Ok(CommentRuntimeConfig {
            submit_rate_limit_seconds: 60,
            list_default_limit: 20,
            cleanup_retention_days: -1,
        });
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/admin/comment-config", admin_base());
        let response = Request::get(&url)
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
        return Ok(config.clone());
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/admin/comment-config", admin_base());
        let response = Request::post(&url)
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
) -> Result<AdminCommentTaskGroupedResponse, String> {
    #[cfg(feature = "mock")]
    {
        return Ok(AdminCommentTaskGroupedResponse {
            groups: vec![],
            total_tasks: 0,
            total_articles: 0,
            status_counts: std::collections::HashMap::new(),
        });
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
        if !params.is_empty() {
            url.push('?');
            url.push_str(&params.join("&"));
        }

        let response = Request::get(&url)
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
        return Err("not found".to_string());
    }

    #[cfg(not(feature = "mock"))]
    {
        let url =
            format!("{}/admin/comments/tasks/{}", admin_base(), urlencoding::encode(task_id),);
        let response = Request::get(&url)
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
        return Ok(AdminCommentTaskAiOutputResponse {
            task_id: task_id.to_string(),
            selected_run_id: None,
            runs: vec![],
            chunks: vec![],
            merged_stdout: String::new(),
            merged_stderr: String::new(),
            merged_output: String::new(),
        });
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

        let response = Request::get(&url)
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
        return Err("not implemented in mock".to_string());
    }

    #[cfg(not(feature = "mock"))]
    {
        let url =
            format!("{}/admin/comments/tasks/{}", admin_base(), urlencoding::encode(task_id),);
        let response = Request::patch(&url)
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
        return Ok(serde_json::json!({ "task_id": task_id, "deleted": true }));
    }

    #[cfg(not(feature = "mock"))]
    {
        let url =
            format!("{}/admin/comments/tasks/{}", admin_base(), urlencoding::encode(task_id),);
        let response = Request::delete(&url)
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
) -> Result<AdminCommentPublishedResponse, String> {
    #[cfg(feature = "mock")]
    {
        return Ok(AdminCommentPublishedResponse {
            comments: vec![],
            total: 0,
        });
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
        if !params.is_empty() {
            url.push('?');
            url.push_str(&params.join("&"));
        }

        let response = Request::get(&url)
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
        return Err("not implemented in mock".to_string());
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!(
            "{}/admin/comments/published/{}",
            admin_base(),
            urlencoding::encode(comment_id),
        );
        let response = Request::patch(&url)
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
        return Ok(serde_json::json!({ "comment_id": comment_id, "deleted": true }));
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!(
            "{}/admin/comments/published/{}",
            admin_base(),
            urlencoding::encode(comment_id),
        );
        let response = Request::delete(&url)
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
) -> Result<AdminCommentAuditResponse, String> {
    #[cfg(feature = "mock")]
    {
        return Ok(AdminCommentAuditResponse {
            logs: vec![],
            total: 0,
        });
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
        if !params.is_empty() {
            url.push('?');
            url.push_str(&params.join("&"));
        }

        let response = Request::get(&url)
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
        return Ok(AdminCleanupResponse {
            deleted_tasks: 0,
            before_ms: None,
        });
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/admin/comments/cleanup", admin_base());
        let response = Request::post(&url)
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
        return Err(format!("mock action not implemented: {}", action));
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!(
            "{}/admin/comments/tasks/{}/{}",
            admin_base(),
            urlencoding::encode(task_id),
            action
        );
        let response = Request::post(&url)
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
