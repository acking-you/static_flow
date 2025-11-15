use serde::{Deserialize, Serialize};

// 完整文章数据模型
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Article {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub content: String, // Markdown 文本
    pub tags: Vec<String>,
    pub category: String,
    pub author: String,
    pub date: String, // 简化为 YYYY-MM-DD 字符串
    pub featured_image: Option<String>,
    pub read_time: u32, // 单位：分钟
}

// 列表项（精简版）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArticleListItem {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub tags: Vec<String>,
    pub category: String,
    pub author: String,
    pub date: String,
    pub featured_image: Option<String>,
    pub read_time: u32,
}

impl From<Article> for ArticleListItem {
    fn from(a: Article) -> Self {
        ArticleListItem {
            id: a.id,
            title: a.title,
            summary: a.summary,
            tags: a.tags,
            category: a.category,
            author: a.author,
            date: a.date,
            featured_image: a.featured_image,
            read_time: a.read_time,
        }
    }
}

// Tag & Category 结构体（方便未来扩展，如计数/描述）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tag {
    pub name: String,
    pub slug: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Category {
    pub name: String,
    pub slug: String,
}
