//! Wasm-safe types shared by the StaticFlow backend, frontend, and CLI
//! crates: content DTOs plus the task lifecycle status.
//!
//! Keep this crate free of host-only dependencies — the frontend compiles it
//! for wasm and tests it on the host, so anything heavier (LanceDB storage,
//! embedding models) belongs in `static-flow-store` / `static-flow-embedding`.

use serde::{Deserialize, Serialize};

/// Shared task lifecycle status for wish / request / comment workflows.
pub mod task_status;

/// Bilingual text payload used for localized summaries and similar fields.
#[allow(
    missing_docs,
    reason = "The type is a thin serde DTO; field names are self-descriptive and documented by \
              the type-level contract."
)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalizedText {
    pub zh: Option<String>,
    pub en: Option<String>,
}

impl LocalizedText {
    /// Trim both locales and return `None` when both become empty.
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

/// High-level article variants rendered by the product.
#[allow(
    missing_docs,
    reason = "Enum variants are intentionally compact and are explained by the type-level \
              documentation."
)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ArticleKind {
    #[default]
    Markdown,
    InteractiveRepost,
}

/// Full article payload returned by content APIs and persistence layers.
#[allow(
    missing_docs,
    reason = "The struct is primarily a data carrier; field-level docs would duplicate stable API \
              field names."
)]
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
    #[serde(default)]
    pub article_kind: ArticleKind,
    #[serde(default)]
    pub source_url: Option<String>,
    #[serde(default)]
    pub interactive_page_id: Option<String>,
}

/// Article summary payload used for list and feed endpoints.
#[allow(
    missing_docs,
    reason = "The list item mirrors serialized API fields, so repeating every field description \
              here would add noise."
)]
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
    #[serde(default)]
    pub article_kind: ArticleKind,
    #[serde(default)]
    pub interactive_page_id: Option<String>,
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
            article_kind: a.article_kind,
            interactive_page_id: a.interactive_page_id,
        }
    }
}

/// Tag descriptor used by taxonomy APIs.
#[allow(
    missing_docs,
    reason = "This DTO has two stable serialized fields whose meaning is evident from the \
              type-level docs."
)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tag {
    pub name: String,
    pub slug: String,
}

/// Category descriptor used by taxonomy APIs.
#[allow(
    missing_docs,
    reason = "This DTO has two stable serialized fields whose meaning is evident from the \
              type-level docs."
)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Category {
    pub name: String,
    pub slug: String,
}

/// Normalize a taxonomy label into a lowercase ASCII slug.
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
