use gloo_net::http::Request;
use serde::{Deserialize, Serialize};
use static_flow_shared::{Article, ArticleListItem};

// API base URL - 开发环境直接连接后端
const API_BASE: &str = "http://localhost:3000/api";

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

#[derive(Debug, Deserialize)]
struct ArticleListResponse {
    articles: Vec<ArticleListItem>,
    #[allow(dead_code)]
    total: usize,
}

#[derive(Debug, Deserialize)]
struct TagsResponse {
    tags: Vec<TagInfo>,
}

#[derive(Debug, Deserialize)]
struct CategoriesResponse {
    categories: Vec<CategoryInfo>,
}

/// 获取文章列表，支持按标签和分类过滤
pub async fn fetch_articles(
    tag: Option<&str>,
    category: Option<&str>,
) -> Result<Vec<ArticleListItem>, String> {
    let mut url = format!("{}/articles", API_BASE);
    let mut params = Vec::new();

    if let Some(t) = tag {
        params.push(format!("tag={}", t));
    }
    if let Some(c) = category {
        params.push(format!("category={}", c));
    }

    if !params.is_empty() {
        url.push_str("?");
        url.push_str(&params.join("&"));
    }

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

/// 获取文章详情
pub async fn fetch_article_detail(id: &str) -> Result<Option<Article>, String> {
    let url = format!("{}/articles/{}", API_BASE, id);

    let response = Request::get(&url)
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

/// 获取所有标签及其文章数量
pub async fn fetch_tags() -> Result<Vec<TagInfo>, String> {
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

/// 获取所有分类及其文章数量和描述
pub async fn fetch_categories() -> Result<Vec<CategoryInfo>, String> {
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

#[derive(Debug, Deserialize)]
struct SearchResponse {
    results: Vec<SearchResult>,
    total: usize,
    query: String,
}

/// 搜索文章
pub async fn search_articles(keyword: &str) -> Result<Vec<SearchResult>, String> {
    if keyword.trim().is_empty() {
        return Ok(vec![]);
    }

    let url = format!("{}/search?q={}", API_BASE, urlencoding::encode(keyword));

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
