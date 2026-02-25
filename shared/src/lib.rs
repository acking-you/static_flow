use serde::{Deserialize, Serialize};

pub mod embedding;

#[cfg(not(target_arch = "wasm32"))]
pub mod comments_store;

#[cfg(not(target_arch = "wasm32"))]
pub mod lancedb_api;

#[cfg(not(target_arch = "wasm32"))]
pub mod music_store;

#[cfg(not(target_arch = "wasm32"))]
pub mod music_wish_store;

#[cfg(not(target_arch = "wasm32"))]
pub mod article_request_store;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalizedText {
    pub zh: Option<String>,
    pub en: Option<String>,
}

impl LocalizedText {
    pub fn normalized(self) -> Option<Self> {
        let zh = self
            .zh
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let en = self
            .en
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        if zh.is_none() && en.is_none() {
            None
        } else {
            Some(Self {
                zh,
                en,
            })
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Article {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub content: String,
    pub content_en: Option<String>,
    pub detailed_summary: Option<LocalizedText>,
    pub tags: Vec<String>,
    pub category: String,
    pub author: String,
    pub date: String,
    pub featured_image: Option<String>,
    pub read_time: u32,
}

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

pub fn normalize_taxonomy_key(name: &str) -> String {
    let mut normalized = String::new();
    let mut last_dash = false;

    for ch in name.trim().chars() {
        if ch.is_alphanumeric() {
            for lower in ch.to_lowercase() {
                normalized.push(lower);
            }
            last_dash = false;
            continue;
        }

        if !normalized.is_empty() && !last_dash {
            normalized.push('-');
            last_dash = true;
        }
    }

    normalized.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::normalize_taxonomy_key;

    #[test]
    fn normalize_taxonomy_key_compacts_symbols() {
        assert_eq!(normalize_taxonomy_key(" Rust / Web "), "rust-web");
        assert_eq!(normalize_taxonomy_key("AI---Ops"), "ai-ops");
    }
}
