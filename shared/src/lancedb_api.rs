use std::collections::HashMap;

use anyhow::{Context, Result};
use arrow_array::{
    Array, ArrayRef, BinaryArray, FixedSizeListArray, Float32Array, ListArray, RecordBatch,
    StringArray,
};
use futures::TryStreamExt;
use lancedb::{
    connect,
    index::scalar::FullTextSearchQuery,
    query::{ExecutableQuery, QueryBase, Select},
    Connection, Table,
};
use serde::{Deserialize, Serialize};

use crate::{
    embedding::{detect_language, embed_text_with_language, TextEmbeddingLanguage},
    normalize_taxonomy_key, Article, ArticleListItem,
};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SearchResult {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub category: String,
    pub date: String,
    pub highlight: String,
    pub tags: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SearchResponse {
    pub results: Vec<SearchResult>,
    pub total: usize,
    pub query: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ImageInfo {
    pub id: String,
    pub filename: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ImageListResponse {
    pub images: Vec<ImageInfo>,
    pub total: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ImageSearchResponse {
    pub images: Vec<ImageInfo>,
    pub total: usize,
    pub query_id: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ArticleListResponse {
    pub articles: Vec<ArticleListItem>,
    pub total: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TagInfo {
    pub name: String,
    pub count: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TagsResponse {
    pub tags: Vec<TagInfo>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CategoryInfo {
    pub name: String,
    pub count: usize,
    pub description: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CategoriesResponse {
    pub categories: Vec<CategoryInfo>,
}

#[derive(Debug, Clone)]
pub struct ImageBlob {
    pub bytes: Vec<u8>,
    pub filename: String,
    pub mime_type: String,
}

pub struct StaticFlowDataStore {
    db: Connection,
    articles_table: String,
    images_table: String,
    taxonomies_table: String,
}

impl StaticFlowDataStore {
    pub async fn connect(db_uri: &str) -> Result<Self> {
        let db = connect(db_uri)
            .execute()
            .await
            .context("failed to connect to LanceDB")?;

        Ok(Self {
            db,
            articles_table: "articles".to_string(),
            images_table: "images".to_string(),
            taxonomies_table: "taxonomies".to_string(),
        })
    }

    pub async fn articles_table(&self) -> Result<Table> {
        self.db
            .open_table(&self.articles_table)
            .execute()
            .await
            .context("failed to open articles table")
    }

    pub async fn images_table(&self) -> Result<Table> {
        self.db
            .open_table(&self.images_table)
            .execute()
            .await
            .context("failed to open images table")
    }

    async fn taxonomies_table(&self) -> Result<Option<Table>> {
        match self.db.open_table(&self.taxonomies_table).execute().await {
            Ok(table) => Ok(Some(table)),
            Err(_) => Ok(None),
        }
    }

    pub async fn list_articles(
        &self,
        tag: Option<&str>,
        category: Option<&str>,
    ) -> Result<Vec<ArticleListItem>> {
        let table = self.articles_table().await?;
        fetch_article_list(&table, tag, category).await
    }

    pub async fn get_article(&self, id: &str) -> Result<Option<Article>> {
        let table = self.articles_table().await?;
        fetch_article_detail(&table, id).await
    }

    pub async fn list_tags(&self) -> Result<Vec<TagInfo>> {
        let articles = self.list_articles(None, None).await?;
        let mut tag_counts: HashMap<String, usize> = HashMap::new();
        for article in articles {
            for tag in article.tags {
                *tag_counts.entry(tag).or_insert(0) += 1;
            }
        }

        let mut tags = tag_counts
            .into_iter()
            .map(|(name, count)| TagInfo {
                name,
                count,
            })
            .collect::<Vec<_>>();
        tags.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(tags)
    }

    pub async fn list_categories(&self) -> Result<Vec<CategoryInfo>> {
        let articles = self.list_articles(None, None).await?;
        let mut category_counts: HashMap<String, usize> = HashMap::new();
        for article in articles {
            *category_counts.entry(article.category).or_insert(0) += 1;
        }

        let mut description_map: HashMap<String, String> = HashMap::new();
        if let Some(table) = self.taxonomies_table().await? {
            description_map = fetch_category_descriptions(&table).await?;
        }

        let mut categories = category_counts
            .into_iter()
            .map(|(name, count)| {
                let key = normalize_taxonomy_key(&name);
                let description = description_map
                    .get(&key)
                    .cloned()
                    .unwrap_or_else(|| name.clone());
                CategoryInfo {
                    name,
                    count,
                    description,
                }
            })
            .collect::<Vec<_>>();
        categories.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(categories)
    }

    pub async fn search_articles(&self, keyword: &str) -> Result<Vec<SearchResult>> {
        let table = self.articles_table().await?;

        match search_with_fts(&table, keyword).await {
            Ok(results) => Ok(results),
            Err(err) => {
                tracing::warn!("FTS search failed, falling back to scan: {}", err);
                fallback_search(&table, keyword).await
            },
        }
    }

    pub async fn semantic_search(&self, keyword: &str, limit: usize) -> Result<Vec<SearchResult>> {
        let table = self.articles_table().await?;

        let language = detect_language(keyword);
        let embedding = embed_text_with_language(keyword, language);
        let vector_column = match language {
            TextEmbeddingLanguage::English => "vector_en",
            TextEmbeddingLanguage::Chinese => "vector_zh",
        };

        let vector_query = table
            .query()
            .nearest_to(embedding.as_slice())
            .context("failed to build semantic query")?;

        let batches = vector_query
            .column(vector_column)
            .only_if(format!("{vector_column} IS NOT NULL"))
            .limit(limit)
            .select(Select::columns(&[
                "id",
                "title",
                "summary",
                "tags",
                "category",
                "author",
                "date",
                "featured_image",
                "read_time",
                "_distance",
            ]))
            .execute()
            .await?;

        let batch_list = batches.try_collect::<Vec<_>>().await?;
        let articles = batches_to_article_list(&batch_list)?;

        Ok(articles
            .into_iter()
            .map(|article| SearchResult {
                id: article.id,
                title: article.title,
                summary: article.summary.clone(),
                category: article.category,
                date: article.date,
                highlight: article.summary,
                tags: article.tags,
            })
            .collect())
    }

    pub async fn related_articles(&self, id: &str, limit: usize) -> Result<Vec<ArticleListItem>> {
        let table = self.articles_table().await?;

        let vector = fetch_article_vector(&table, id).await?;
        let (vector, vector_column) = match vector {
            Some(value) => value,
            None => return Ok(vec![]),
        };

        let filter = format!("{vector_column} IS NOT NULL AND id != '{}'", escape_literal(id));
        let vector_query = table
            .query()
            .nearest_to(vector.as_slice())
            .context("failed to build related query")?;

        let batches = vector_query
            .column(vector_column)
            .only_if(filter)
            .limit(limit)
            .select(Select::columns(&[
                "id",
                "title",
                "summary",
                "tags",
                "category",
                "author",
                "date",
                "featured_image",
                "read_time",
                "_distance",
            ]))
            .execute()
            .await?;

        let batch_list = batches.try_collect::<Vec<_>>().await?;
        batches_to_article_list(&batch_list)
    }

    pub async fn list_images(&self) -> Result<Vec<ImageInfo>> {
        let table = self.images_table().await?;

        let batches = table
            .query()
            .select(Select::columns(&["id", "filename"]))
            .execute()
            .await?;

        let batch_list = batches.try_collect::<Vec<_>>().await?;
        batches_to_images(&batch_list)
    }

    pub async fn search_images(&self, id: &str, limit: usize) -> Result<Vec<ImageInfo>> {
        let table = self.images_table().await?;

        let vector = fetch_image_vector(&table, id).await?;
        let vector = match vector {
            Some(value) => value,
            None => return Ok(vec![]),
        };

        let filter = format!("id != '{}'", escape_literal(id));
        let vector_query = table
            .query()
            .nearest_to(vector.as_slice())
            .context("failed to build image search query")?;

        let batches = vector_query
            .only_if(filter)
            .limit(limit)
            .select(Select::columns(&["id", "filename", "_distance"]))
            .execute()
            .await?;

        let batch_list = batches.try_collect::<Vec<_>>().await?;
        batches_to_images(&batch_list)
    }

    pub async fn get_image(
        &self,
        id_or_filename: &str,
        prefer_thumbnail: bool,
    ) -> Result<Option<ImageBlob>> {
        let table = self.images_table().await?;

        let escaped = escape_literal(id_or_filename);
        let filter = format!("filename = '{}' OR id = '{}'", escaped, escaped);
        let batches = table
            .query()
            .only_if(filter)
            .limit(1)
            .select(Select::columns(&["data", "thumbnail", "filename"]))
            .execute()
            .await?;

        let batch_list = batches.try_collect::<Vec<_>>().await?;
        let image = extract_image_bytes(&batch_list, prefer_thumbnail)?;

        Ok(image.map(|(bytes, filename)| ImageBlob {
            mime_type: image_mime_type(&filename).to_string(),
            bytes,
            filename,
        }))
    }
}

async fn fetch_category_descriptions(table: &Table) -> Result<HashMap<String, String>> {
    let batches = table
        .query()
        .only_if("kind = 'category'")
        .select(Select::columns(&["key", "description"]))
        .execute()
        .await?;

    let batch_list = batches.try_collect::<Vec<_>>().await?;
    let mut descriptions = HashMap::new();

    for batch in &batch_list {
        let key = string_array(batch, "key")?;
        let description = string_array(batch, "description")?;

        for row in 0..batch.num_rows() {
            if description.is_null(row) {
                continue;
            }

            let value = description.value(row).trim();
            if value.is_empty() {
                continue;
            }

            descriptions.insert(key.value(row).to_string(), value.to_string());
        }
    }

    Ok(descriptions)
}

async fn fetch_article_list(
    table: &Table,
    tag: Option<&str>,
    category: Option<&str>,
) -> Result<Vec<ArticleListItem>> {
    let mut filters = Vec::new();

    if let Some(tag) = tag {
        let tag_lower = tag.to_lowercase();
        let escaped_tag = escape_literal(tag);
        let escaped_lower = escape_literal(&tag_lower);
        let tag_filter = if escaped_tag == escaped_lower {
            format!("list_contains(tags, '{}')", escaped_tag)
        } else {
            format!(
                "(list_contains(tags, '{}') OR list_contains(tags, '{}'))",
                escaped_tag, escaped_lower
            )
        };
        filters.push(tag_filter);
    }

    if let Some(category) = category {
        let category_lower = category.to_lowercase();
        filters.push(format!("lower(category) = '{}'", escape_literal(&category_lower)));
    }

    let mut query = table.query();
    if !filters.is_empty() {
        query = query.only_if(filters.join(" AND "));
    }

    let batches = query
        .select(Select::columns(&[
            "id",
            "title",
            "summary",
            "tags",
            "category",
            "author",
            "date",
            "featured_image",
            "read_time",
        ]))
        .execute()
        .await?;

    let batch_list = batches.try_collect::<Vec<_>>().await?;
    let mut articles = batches_to_article_list(&batch_list)?;
    articles.sort_by(|a, b| b.date.cmp(&a.date));
    Ok(articles)
}

async fn fetch_article_detail(table: &Table, id: &str) -> Result<Option<Article>> {
    let filter = format!("id = '{}'", escape_literal(id));
    let batches = table
        .query()
        .only_if(filter)
        .limit(1)
        .select(Select::columns(&[
            "id",
            "title",
            "summary",
            "content",
            "tags",
            "category",
            "author",
            "date",
            "featured_image",
            "read_time",
        ]))
        .execute()
        .await?;

    let batch_list = batches.try_collect::<Vec<_>>().await?;
    batches_to_article_detail(&batch_list)
}

async fn fetch_article_vector(table: &Table, id: &str) -> Result<Option<(Vec<f32>, &'static str)>> {
    let filter = format!("id = '{}'", escape_literal(id));
    let batches = table
        .query()
        .only_if(filter)
        .limit(1)
        .select(Select::columns(&["vector_en", "vector_zh"]))
        .execute()
        .await?;

    let batch_list = batches.try_collect::<Vec<_>>().await?;
    if let Some(vector) = extract_vector(&batch_list, "vector_en") {
        return Ok(Some((vector, "vector_en")));
    }
    if let Some(vector) = extract_vector(&batch_list, "vector_zh") {
        return Ok(Some((vector, "vector_zh")));
    }
    Ok(None)
}

async fn fetch_image_vector(table: &Table, id: &str) -> Result<Option<Vec<f32>>> {
    let filter = format!("id = '{}'", escape_literal(id));
    let batches = table
        .query()
        .only_if(filter)
        .limit(1)
        .select(Select::columns(&["vector"]))
        .execute()
        .await?;

    let batch_list = batches.try_collect::<Vec<_>>().await?;
    Ok(extract_vector(&batch_list, "vector"))
}

async fn search_with_fts(table: &Table, keyword: &str) -> Result<Vec<SearchResult>> {
    let batches = table
        .query()
        .full_text_search(FullTextSearchQuery::new(keyword.to_string()))
        .limit(10)
        .select(Select::columns(&[
            "id",
            "title",
            "summary",
            "content",
            "tags",
            "category",
            "author",
            "date",
            "featured_image",
            "read_time",
            "_score",
        ]))
        .execute()
        .await?;

    let batch_list = batches.try_collect::<Vec<_>>().await?;
    let articles = batches_to_articles(&batch_list)?;

    Ok(articles
        .into_iter()
        .map(|article| SearchResult {
            highlight: extract_highlight(&article.content, keyword),
            id: article.id,
            title: article.title,
            summary: article.summary,
            category: article.category,
            date: article.date,
            tags: article.tags,
        })
        .collect())
}

async fn fallback_search(table: &Table, keyword: &str) -> Result<Vec<SearchResult>> {
    let batches = table
        .query()
        .select(Select::columns(&[
            "id",
            "title",
            "summary",
            "content",
            "tags",
            "category",
            "author",
            "date",
            "featured_image",
            "read_time",
        ]))
        .execute()
        .await?;

    let batch_list = batches.try_collect::<Vec<_>>().await?;
    let articles = batches_to_articles(&batch_list)?;

    let keyword_lower = keyword.to_lowercase();
    let mut results = Vec::new();
    for article in articles {
        let mut score = 0;
        if article.title.to_lowercase().contains(&keyword_lower) {
            score += 10;
        }
        if article.summary.to_lowercase().contains(&keyword_lower) {
            score += 5;
        }
        if article.content.to_lowercase().contains(&keyword_lower) {
            score += 1;
        }
        for tag in &article.tags {
            if tag.to_lowercase().contains(&keyword_lower) {
                score += 3;
            }
        }

        if score > 0 {
            results.push((
                SearchResult {
                    highlight: extract_highlight(&article.content, &keyword_lower),
                    id: article.id,
                    title: article.title,
                    summary: article.summary,
                    category: article.category,
                    date: article.date,
                    tags: article.tags,
                },
                score,
            ));
        }
    }

    results.sort_by(|a, b| b.1.cmp(&a.1));
    Ok(results.into_iter().map(|(result, _)| result).collect())
}

fn batches_to_article_list(batches: &[RecordBatch]) -> Result<Vec<ArticleListItem>> {
    let mut articles = Vec::new();
    for batch in batches {
        let id = string_array(batch, "id")?;
        let title = string_array(batch, "title")?;
        let summary = string_array(batch, "summary")?;
        let tags = list_array(batch, "tags")?;
        let category = string_array(batch, "category")?;
        let author = string_array(batch, "author")?;
        let date = string_array(batch, "date")?;
        let featured = string_array(batch, "featured_image")?;
        let read_time = int32_array(batch, "read_time")?;

        for row in 0..batch.num_rows() {
            articles.push(ArticleListItem {
                id: value_string(id, row),
                title: value_string(title, row),
                summary: value_string(summary, row),
                tags: value_string_list(tags, row),
                category: value_string(category, row),
                author: value_string(author, row),
                date: value_string(date, row),
                featured_image: value_string_opt(featured, row),
                read_time: read_time.value(row) as u32,
            });
        }
    }
    Ok(articles)
}

fn batches_to_articles(batches: &[RecordBatch]) -> Result<Vec<Article>> {
    let mut articles = Vec::new();
    for batch in batches {
        let id = string_array(batch, "id")?;
        let title = string_array(batch, "title")?;
        let summary = string_array(batch, "summary")?;
        let content = string_array(batch, "content")?;
        let tags = list_array(batch, "tags")?;
        let category = string_array(batch, "category")?;
        let author = string_array(batch, "author")?;
        let date = string_array(batch, "date")?;
        let featured = string_array(batch, "featured_image")?;
        let read_time = int32_array(batch, "read_time")?;

        for row in 0..batch.num_rows() {
            articles.push(Article {
                id: value_string(id, row),
                title: value_string(title, row),
                summary: value_string(summary, row),
                content: value_string(content, row),
                tags: value_string_list(tags, row),
                category: value_string(category, row),
                author: value_string(author, row),
                date: value_string(date, row),
                featured_image: value_string_opt(featured, row),
                read_time: read_time.value(row) as u32,
            });
        }
    }
    Ok(articles)
}

fn batches_to_article_detail(batches: &[RecordBatch]) -> Result<Option<Article>> {
    let articles = batches_to_articles(batches)?;
    Ok(articles.into_iter().next())
}

fn batches_to_images(batches: &[RecordBatch]) -> Result<Vec<ImageInfo>> {
    let mut images = Vec::new();
    for batch in batches {
        let id = string_array(batch, "id")?;
        let filename = string_array(batch, "filename")?;

        for row in 0..batch.num_rows() {
            images.push(ImageInfo {
                id: value_string(id, row),
                filename: value_string(filename, row),
            });
        }
    }
    Ok(images)
}

fn extract_vector(batches: &[RecordBatch], column: &str) -> Option<Vec<f32>> {
    for batch in batches {
        if batch.num_rows() == 0 {
            continue;
        }

        let vector_array = batch.schema().index_of(column).ok().and_then(|idx| {
            batch
                .column(idx)
                .as_any()
                .downcast_ref::<FixedSizeListArray>()
        })?;

        if vector_array.is_null(0) {
            return None;
        }
        return Some(value_vector(vector_array, 0));
    }
    None
}

fn extract_image_bytes(
    batches: &[RecordBatch],
    prefer_thumbnail: bool,
) -> Result<Option<(Vec<u8>, String)>> {
    for batch in batches {
        if batch.num_rows() == 0 {
            continue;
        }
        let data = binary_array(batch, "data")?;
        let thumb = binary_array(batch, "thumbnail")?;
        let filename = string_array(batch, "filename")?;
        let name = value_string(filename, 0);

        if prefer_thumbnail && !thumb.is_null(0) {
            return Ok(Some((thumb.value(0).to_vec(), name)));
        }
        return Ok(Some((data.value(0).to_vec(), name)));
    }
    Ok(None)
}

fn string_array<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a StringArray> {
    column(batch, name)?
        .as_any()
        .downcast_ref::<StringArray>()
        .with_context(|| format!("column {name} is not StringArray"))
}

fn list_array<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a ListArray> {
    column(batch, name)?
        .as_any()
        .downcast_ref::<ListArray>()
        .with_context(|| format!("column {name} is not ListArray"))
}

fn int32_array<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a arrow_array::Int32Array> {
    column(batch, name)?
        .as_any()
        .downcast_ref::<arrow_array::Int32Array>()
        .with_context(|| format!("column {name} is not Int32Array"))
}

fn binary_array<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a BinaryArray> {
    column(batch, name)?
        .as_any()
        .downcast_ref::<BinaryArray>()
        .with_context(|| format!("column {name} is not BinaryArray"))
}

fn column<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a ArrayRef> {
    let idx = batch
        .schema()
        .index_of(name)
        .with_context(|| format!("missing column {name}"))?;
    Ok(batch.column(idx))
}

fn value_string(array: &StringArray, row: usize) -> String {
    array.value(row).to_string()
}

fn value_string_opt(array: &StringArray, row: usize) -> Option<String> {
    if array.is_null(row) {
        None
    } else {
        Some(array.value(row).to_string())
    }
}

fn value_string_list(array: &ListArray, row: usize) -> Vec<String> {
    if array.is_null(row) {
        return vec![];
    }

    let value = array.value(row);
    let value = value
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap_or_else(|| panic!("tags list is not StringArray"));

    (0..value.len())
        .map(|idx| value.value(idx).to_string())
        .collect()
}

fn value_vector(array: &FixedSizeListArray, row: usize) -> Vec<f32> {
    let values = array.values();
    let values = values
        .as_any()
        .downcast_ref::<Float32Array>()
        .unwrap_or_else(|| panic!("vector values are not Float32Array"));

    let dim = array.value_length() as usize;
    let start = row * dim;
    let mut vector = Vec::with_capacity(dim);
    for idx in 0..dim {
        vector.push(values.value(start + idx));
    }
    vector
}

fn image_mime_type(filename: &str) -> &'static str {
    match filename.split('.').next_back() {
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

fn escape_literal(input: &str) -> String {
    input.replace('\'', "''")
}

fn extract_highlight(text: &str, keyword: &str) -> String {
    let text_lower = text.to_lowercase();
    let keyword_lower = keyword.to_lowercase();

    if let Some(pos) = text_lower.find(&keyword_lower) {
        let start = pos.saturating_sub(40);
        let end = (pos + keyword.len() + 40).min(text.len());

        let mut snippet: String = text.chars().skip(start).take(end - start).collect();

        if start > 0 {
            snippet.insert_str(0, "...");
        }
        if end < text.len() {
            snippet.push_str("...");
        }

        let snippet_lower = snippet.to_lowercase();
        if let Some(keyword_pos) = snippet_lower.find(&keyword_lower) {
            let before = &snippet[..keyword_pos];
            let matched = &snippet[keyword_pos..keyword_pos + keyword.len()];
            let after = &snippet[keyword_pos + keyword.len()..];
            return format!("{before}<mark>{matched}</mark>{after}");
        }

        snippet
    } else {
        text.chars().take(100).collect::<String>() + "..."
    }
}
