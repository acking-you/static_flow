#[cfg(not(feature = "mock"))]
use gloo_net::http::Request;
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
}

/// 获取文章详情
pub async fn fetch_article_detail(id: &str) -> Result<Option<Article>, String> {
    #[cfg(feature = "mock")]
    {
        return Ok(models::get_mock_article_detail(id));
    }

    #[cfg(not(feature = "mock"))]
    {
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

    #[cfg(feature = "mock")]
    {
        return Ok(models::mock_search(keyword));
    }

    #[cfg(not(feature = "mock"))]
    {
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
}
