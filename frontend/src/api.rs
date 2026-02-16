#[cfg(not(feature = "mock"))]
use gloo_net::http::Request;
#[cfg(not(feature = "mock"))]
use js_sys::Date;
use serde::{Deserialize, Serialize};
use static_flow_shared::{Article, ArticleListItem};

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

#[cfg(not(feature = "mock"))]
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct ImageListResponse {
    images: Vec<ImageInfo>,
    total: usize,
}

#[cfg(not(feature = "mock"))]
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct ImageSearchResponse {
    images: Vec<ImageInfo>,
    total: usize,
    query_id: String,
}

#[cfg(not(feature = "mock"))]
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct ImageTextSearchResponse {
    images: Vec<ImageInfo>,
    total: usize,
    query: String,
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
pub async fn fetch_images() -> Result<Vec<ImageInfo>, String> {
    #[cfg(feature = "mock")]
    {
        return Ok(vec![]);
    }

    #[cfg(not(feature = "mock"))]
    {
        let url = format!("{}/images", API_BASE);

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

        Ok(json_response.images)
    }
}

/// Search images by an existing image id.
pub async fn search_images_by_id(
    image_id: &str,
    limit: Option<usize>,
    max_distance: Option<f32>,
) -> Result<Vec<ImageInfo>, String> {
    if image_id.trim().is_empty() {
        return Ok(vec![]);
    }

    #[cfg(feature = "mock")]
    {
        return Ok(vec![]);
    }

    #[cfg(not(feature = "mock"))]
    {
        let mut url = format!("{}/image-search?id={}", API_BASE, urlencoding::encode(image_id));
        if let Some(limit) = limit {
            url.push_str(&format!("&limit={limit}"));
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

        Ok(json_response.images)
    }
}

/// Search images with text query (text-to-image).
pub async fn search_images_by_text(
    keyword: &str,
    limit: Option<usize>,
    max_distance: Option<f32>,
) -> Result<Vec<ImageInfo>, String> {
    if keyword.trim().is_empty() {
        return Ok(vec![]);
    }

    #[cfg(feature = "mock")]
    {
        return Ok(vec![]);
    }

    #[cfg(not(feature = "mock"))]
    {
        let mut url = format!("{}/image-search-text?q={}", API_BASE, urlencoding::encode(keyword));
        if let Some(limit) = limit {
            url.push_str(&format!("&limit={limit}"));
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

        Ok(json_response.images)
    }
}
