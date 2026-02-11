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
            Ok(results) if !results.is_empty() => Ok(results),
            Ok(_) => {
                tracing::info!("FTS returned no rows, falling back to scan; keyword={keyword}");
                let fallback_results = fallback_search(&table, keyword).await?;
                tracing::info!(
                    "Scan fallback completed; keyword={keyword}; rows={}",
                    fallback_results.len()
                );
                Ok(fallback_results)
            },
            Err(err) => {
                tracing::warn!(
                    "FTS search failed, falling back to scan; keyword={keyword}; error={err}"
                );
                let fallback_results = fallback_search(&table, keyword).await?;
                tracing::info!(
                    "Scan fallback completed after FTS failure; keyword={keyword}; rows={}",
                    fallback_results.len()
                );
                Ok(fallback_results)
            },
        }
    }

    pub async fn semantic_search(&self, keyword: &str, limit: usize) -> Result<Vec<SearchResult>> {
        let table = self.articles_table().await?;

        let mut search_language = detect_language(keyword);
        let mut query_embedding = embed_text_with_language(keyword, search_language);
        let mut rows = run_semantic_vector_search(
            &table,
            vector_column_for_language(search_language),
            query_embedding.as_slice(),
            limit,
        )
        .await?;

        if rows.is_empty() {
            let primary_column = vector_column_for_language(search_language);
            let fallback_language = alternate_embedding_language(search_language);
            let fallback_column = vector_column_for_language(fallback_language);
            tracing::info!(
                "Semantic search primary returned no rows; keyword={keyword}; \
                 primary_column={primary_column}; trying fallback_column={fallback_column}"
            );

            let fallback_embedding = embed_text_with_language(keyword, fallback_language);
            let fallback_rows = run_semantic_vector_search(
                &table,
                fallback_column,
                fallback_embedding.as_slice(),
                limit,
            )
            .await?;

            if !fallback_rows.is_empty() {
                tracing::info!(
                    "Semantic fallback succeeded; keyword={keyword}; \
                     fallback_column={fallback_column}; rows={}",
                    fallback_rows.len()
                );
                search_language = fallback_language;
                query_embedding = fallback_embedding;
                rows = fallback_rows;
            } else {
                tracing::info!(
                    "Semantic fallback returned no rows; keyword={keyword}; \
                     fallback_column={fallback_column}"
                );
            }
        }

        Ok(rows
            .into_iter()
            .map(|row| SearchResult {
                id: row.id,
                title: row.title,
                summary: row.summary.clone(),
                category: row.category,
                date: row.date,
                highlight: extract_semantic_highlight(
                    &row.content,
                    &row.summary,
                    keyword,
                    query_embedding.as_slice(),
                    search_language,
                ),
                tags: row.tags,
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

fn vector_column_for_language(language: TextEmbeddingLanguage) -> &'static str {
    match language {
        TextEmbeddingLanguage::English => "vector_en",
        TextEmbeddingLanguage::Chinese => "vector_zh",
    }
}

fn alternate_embedding_language(language: TextEmbeddingLanguage) -> TextEmbeddingLanguage {
    match language {
        TextEmbeddingLanguage::English => TextEmbeddingLanguage::Chinese,
        TextEmbeddingLanguage::Chinese => TextEmbeddingLanguage::English,
    }
}

async fn run_semantic_vector_search(
    table: &Table,
    vector_column: &str,
    query_embedding: &[f32],
    limit: usize,
) -> Result<Vec<SearchArticleRow>> {
    let vector_query = table
        .query()
        .nearest_to(query_embedding)
        .context("failed to build semantic query")?;

    let batches = vector_query
        .column(vector_column)
        .only_if(format!("{vector_column} IS NOT NULL"))
        .limit(limit)
        .select(Select::columns(&[
            "id",
            "title",
            "summary",
            "content",
            "tags",
            "category",
            "date",
            "_distance",
        ]))
        .execute()
        .await?;

    let batch_list = batches.try_collect::<Vec<_>>().await?;
    batches_to_search_rows(&batch_list)
}

#[derive(Debug)]
struct SearchArticleRow {
    id: String,
    title: String,
    summary: String,
    content: String,
    tags: Vec<String>,
    category: String,
    date: String,
}

async fn search_with_fts(table: &Table, keyword: &str) -> Result<Vec<SearchResult>> {
    let batches = table
        .query()
        .full_text_search(FullTextSearchQuery::new(keyword.to_string()))
        .limit(10)
        .select(Select::columns(&[
            "id", "title", "summary", "content", "tags", "category", "date", "_score",
        ]))
        .execute()
        .await?;

    let batch_list = batches.try_collect::<Vec<_>>().await?;
    let rows = batches_to_search_rows(&batch_list)?;

    Ok(rows
        .into_iter()
        .map(|row| SearchResult {
            highlight: extract_highlight(&row.content, keyword),
            id: row.id,
            title: row.title,
            summary: row.summary,
            category: row.category,
            date: row.date,
            tags: row.tags,
        })
        .collect())
}

async fn fallback_search(table: &Table, keyword: &str) -> Result<Vec<SearchResult>> {
    let batches = table
        .query()
        .select(Select::columns(&["id", "title", "summary", "content", "tags", "category", "date"]))
        .execute()
        .await?;

    let batch_list = batches.try_collect::<Vec<_>>().await?;
    let rows = batches_to_search_rows(&batch_list)?;

    let keyword_lower = keyword.to_lowercase();
    let mut results = Vec::new();
    for row in rows {
        let mut score = 0;
        if row.title.to_lowercase().contains(&keyword_lower) {
            score += 10;
        }
        if row.summary.to_lowercase().contains(&keyword_lower) {
            score += 5;
        }
        if row.content.to_lowercase().contains(&keyword_lower) {
            score += 1;
        }
        for tag in &row.tags {
            if tag.to_lowercase().contains(&keyword_lower) {
                score += 3;
            }
        }

        if score > 0 {
            results.push((
                SearchResult {
                    highlight: extract_highlight(&row.content, keyword),
                    id: row.id,
                    title: row.title,
                    summary: row.summary,
                    category: row.category,
                    date: row.date,
                    tags: row.tags,
                },
                score,
            ));
        }
    }

    results.sort_by(|a, b| b.1.cmp(&a.1));
    Ok(results.into_iter().map(|(result, _)| result).collect())
}

fn batches_to_search_rows(batches: &[RecordBatch]) -> Result<Vec<SearchArticleRow>> {
    let mut rows = Vec::new();
    for batch in batches {
        let id = string_array(batch, "id")?;
        let title = string_array(batch, "title")?;
        let summary = string_array(batch, "summary")?;
        let content = string_array(batch, "content")?;
        let tags = list_array(batch, "tags")?;
        let category = string_array(batch, "category")?;
        let date = string_array(batch, "date")?;

        for row in 0..batch.num_rows() {
            rows.push(SearchArticleRow {
                id: value_string(id, row),
                title: value_string(title, row),
                summary: value_string(summary, row),
                content: value_string(content, row),
                tags: value_string_list(tags, row),
                category: value_string(category, row),
                date: value_string(date, row),
            });
        }
    }

    Ok(rows)
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
    const CONTEXT_CHARS: usize = 40;
    const FALLBACK_EXCERPT_CHARS: usize = 100;

    let keyword = keyword.trim();
    if keyword.is_empty() {
        return excerpt_with_ellipsis(text, FALLBACK_EXCERPT_CHARS);
    }

    let text_chars: Vec<char> = text.chars().collect();
    if text_chars.is_empty() {
        return String::new();
    }

    if let Some((match_start, match_end)) = find_case_insensitive_match_range(text, keyword) {
        if match_start >= match_end || match_start >= text_chars.len() {
            return excerpt_with_ellipsis(text, FALLBACK_EXCERPT_CHARS);
        }

        let match_end = match_end.min(text_chars.len());
        let snippet_start = match_start.saturating_sub(CONTEXT_CHARS);
        let snippet_end = (match_end + CONTEXT_CHARS).min(text_chars.len());

        let mut snippet = String::new();
        if snippet_start > 0 {
            snippet.push_str("...");
        }
        snippet.extend(text_chars[snippet_start..match_start].iter());
        snippet.push_str("<mark>");
        snippet.extend(text_chars[match_start..match_end].iter());
        snippet.push_str("</mark>");
        snippet.extend(text_chars[match_end..snippet_end].iter());
        if snippet_end < text_chars.len() {
            snippet.push_str("...");
        }

        return snippet;
    }

    excerpt_with_ellipsis(text, FALLBACK_EXCERPT_CHARS)
}

fn find_case_insensitive_match_range(text: &str, keyword: &str) -> Option<(usize, usize)> {
    let keyword_folded = keyword
        .chars()
        .flat_map(|value| value.to_lowercase())
        .collect::<Vec<_>>();
    if keyword_folded.is_empty() {
        return None;
    }

    let mut text_folded = Vec::new();
    let mut folded_to_original = Vec::new();

    for (char_index, value) in text.chars().enumerate() {
        for lowered in value.to_lowercase() {
            text_folded.push(lowered);
            folded_to_original.push(char_index);
        }
    }

    if text_folded.len() < keyword_folded.len() {
        return None;
    }

    for folded_start in 0..=(text_folded.len() - keyword_folded.len()) {
        if text_folded[folded_start..folded_start + keyword_folded.len()] == keyword_folded[..] {
            let original_start = folded_to_original[folded_start];
            let original_end = folded_to_original[folded_start + keyword_folded.len() - 1] + 1;
            return Some((original_start, original_end));
        }
    }

    None
}

fn excerpt_with_ellipsis(text: &str, max_chars: usize) -> String {
    let chars = text.chars().collect::<Vec<_>>();
    if chars.len() <= max_chars {
        return chars.into_iter().collect();
    }

    let mut excerpt = chars.into_iter().take(max_chars).collect::<String>();
    excerpt.push_str("...");
    excerpt
}

fn extract_semantic_highlight(
    content: &str,
    summary: &str,
    keyword: &str,
    query_embedding: &[f32],
    language: TextEmbeddingLanguage,
) -> String {
    const MAX_CANDIDATES: usize = 24;
    const MAX_SNIPPET_CHARS: usize = 180;

    let content = content.trim();
    if content.is_empty() {
        return excerpt_with_ellipsis(summary, MAX_SNIPPET_CHARS);
    }

    if find_case_insensitive_match_range(content, keyword).is_some() {
        return extract_highlight(content, keyword);
    }

    let candidates = semantic_snippet_candidates(content, MAX_SNIPPET_CHARS);
    let mut best_snippet: Option<&str> = None;
    let mut best_score = f32::NEG_INFINITY;

    for candidate in candidates.iter().take(MAX_CANDIDATES) {
        let candidate_embedding = embed_text_with_language(candidate, language);
        let semantic_score = cosine_similarity(query_embedding, candidate_embedding.as_slice());
        let lexical_score = semantic_keyword_overlap_score(candidate, keyword);
        let score = semantic_score + lexical_score * 0.15;

        if score > best_score {
            best_score = score;
            best_snippet = Some(candidate.as_str());
        }
    }

    if let Some(snippet) = best_snippet {
        if let Some(token) = first_overlapping_token(snippet, keyword) {
            return extract_highlight(snippet, &token);
        }
        return excerpt_with_ellipsis(snippet, MAX_SNIPPET_CHARS);
    }

    if !summary.trim().is_empty() {
        return excerpt_with_ellipsis(summary, MAX_SNIPPET_CHARS);
    }

    excerpt_with_ellipsis(content, MAX_SNIPPET_CHARS)
}

fn semantic_snippet_candidates(content: &str, max_chars: usize) -> Vec<String> {
    let mut candidates = Vec::new();
    let mut block_lines = Vec::new();

    let push_block = |lines: &mut Vec<String>, out: &mut Vec<String>| {
        if lines.is_empty() {
            return;
        }

        let block = lines.join(" ");
        lines.clear();

        let block = block.trim();
        if block.is_empty() {
            return;
        }

        out.extend(split_text_by_sentence_or_size(block, max_chars));
    };

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            push_block(&mut block_lines, &mut candidates);
            continue;
        }

        if trimmed.is_empty() {
            push_block(&mut block_lines, &mut candidates);
            continue;
        }

        block_lines.push(trimmed.to_string());
    }
    push_block(&mut block_lines, &mut candidates);

    if candidates.is_empty() {
        candidates.extend(split_text_by_sentence_or_size(content, max_chars));
    }

    candidates
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| value.chars().count() >= 12)
        .collect()
}

fn split_text_by_sentence_or_size(text: &str, max_chars: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();

    for ch in text.chars() {
        current.push(ch);
        let current_len = current.chars().count();
        let sentence_boundary = matches!(ch, '。' | '！' | '？' | ';' | '；' | '!' | '?' | '.');

        if current_len >= max_chars || (sentence_boundary && current_len >= max_chars / 2) {
            if !current.trim().is_empty() {
                chunks.push(current.trim().to_string());
            }
            current.clear();
        }
    }

    if !current.trim().is_empty() {
        chunks.push(current.trim().to_string());
    }

    let mut final_chunks = Vec::new();
    for chunk in chunks {
        let chars = chunk.chars().collect::<Vec<_>>();
        if chars.len() <= max_chars {
            final_chunks.push(chunk);
            continue;
        }

        let mut start = 0;
        while start < chars.len() {
            let end = (start + max_chars).min(chars.len());
            let part = chars[start..end]
                .iter()
                .collect::<String>()
                .trim()
                .to_string();
            if !part.is_empty() {
                final_chunks.push(part);
            }
            start = end;
        }
    }

    final_chunks
}

fn semantic_keyword_overlap_score(text: &str, keyword: &str) -> f32 {
    let tokens = semantic_query_tokens(keyword);
    if tokens.is_empty() {
        return 0.0;
    }

    let lowered = text.to_lowercase();
    let matched = tokens
        .iter()
        .filter(|token| lowered.contains(token.as_str()))
        .count();

    matched as f32 / tokens.len() as f32
}

fn first_overlapping_token(text: &str, keyword: &str) -> Option<String> {
    let mut tokens = semantic_query_tokens(keyword);
    if tokens.is_empty() {
        return None;
    }

    tokens.sort_by_key(|token| std::cmp::Reverse(token.chars().count()));
    let lowered = text.to_lowercase();

    tokens
        .into_iter()
        .find(|token| token.chars().count() >= 2 && lowered.contains(token.as_str()))
}

fn semantic_query_tokens(keyword: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();

    let flush = |buffer: &mut String, out: &mut Vec<String>| {
        if buffer.trim().is_empty() {
            buffer.clear();
            return;
        }

        let lowered = buffer.to_lowercase();
        out.push(lowered.clone());

        let chars = lowered.chars().collect::<Vec<_>>();
        if chars.iter().all(|ch| is_cjk_char(*ch)) && chars.len() >= 2 {
            for size in 2..=3 {
                if chars.len() < size {
                    continue;
                }
                for idx in 0..=(chars.len() - size) {
                    out.push(chars[idx..idx + size].iter().collect());
                }
            }
        }

        buffer.clear();
    };

    for ch in keyword.chars() {
        if ch.is_alphanumeric() || is_cjk_char(ch) {
            current.push(ch);
        } else {
            flush(&mut current, &mut tokens);
        }
    }
    flush(&mut current, &mut tokens);

    tokens.sort();
    tokens.dedup();
    tokens
}

fn is_cjk_char(ch: char) -> bool {
    matches!(
        ch as u32,
        0x4E00..=0x9FFF
            | 0x3400..=0x4DBF
            | 0x20000..=0x2A6DF
            | 0x2A700..=0x2B73F
            | 0x2B740..=0x2B81F
            | 0x2B820..=0x2CEAF
            | 0xF900..=0xFAFF
            | 0x2F800..=0x2FA1F
    )
}

fn cosine_similarity(left: &[f32], right: &[f32]) -> f32 {
    if left.len() != right.len() || left.is_empty() {
        return 0.0;
    }

    let mut dot = 0.0;
    let mut left_norm = 0.0;
    let mut right_norm = 0.0;

    for (l, r) in left.iter().zip(right.iter()) {
        dot += l * r;
        left_norm += l * l;
        right_norm += r * r;
    }

    if left_norm == 0.0 || right_norm == 0.0 {
        return 0.0;
    }

    dot / (left_norm.sqrt() * right_norm.sqrt())
}

#[cfg(test)]
mod tests {
    use super::{
        alternate_embedding_language, cosine_similarity, extract_highlight,
        extract_semantic_highlight, find_case_insensitive_match_range, semantic_query_tokens,
        split_text_by_sentence_or_size, vector_column_for_language, TextEmbeddingLanguage,
    };

    #[test]
    fn highlight_marks_ascii_case_insensitive_keyword() {
        let text = "Alpha beta TEST gamma";
        let highlight = extract_highlight(text, "test");
        assert!(highlight.contains("<mark>TEST</mark>"));
    }

    #[test]
    fn highlight_marks_chinese_keyword_without_utf8_offset_bug() {
        let text = "这里是渲染功能测试内容。";
        let highlight = extract_highlight(text, "渲染");
        assert!(highlight.contains("<mark>渲染</mark>"));
    }

    #[test]
    fn highlight_returns_excerpt_when_keyword_missing() {
        let text = "no matched keyword here";
        let highlight = extract_highlight(text, "missing");
        assert!(!highlight.contains("<mark>"));
    }

    #[test]
    fn match_range_handles_multibyte_characters() {
        let range = find_case_insensitive_match_range("你好，渲染世界", "渲染");
        assert_eq!(range, Some((3, 5)));
    }

    #[test]
    fn semantic_highlight_uses_keyword_hit_when_available() {
        let content = "这段文本介绍前端渲染与性能优化。";
        let highlight = extract_semantic_highlight(
            content,
            "summary",
            "渲染",
            &[],
            TextEmbeddingLanguage::Chinese,
        );
        assert!(highlight.contains("<mark>渲染</mark>"));
    }

    #[test]
    fn semantic_highlight_uses_summary_when_content_empty() {
        let highlight = extract_semantic_highlight(
            "",
            "summary content",
            "query",
            &[],
            TextEmbeddingLanguage::English,
        );
        assert!(highlight.contains("summary"));
    }

    #[test]
    fn semantic_tokens_expand_cjk_ngrams() {
        let tokens = semantic_query_tokens("页面渲染性能");
        assert!(tokens.iter().any(|token| token == "渲染"));
    }

    #[test]
    fn cosine_similarity_returns_one_for_identical_vectors() {
        let left = vec![1.0, 2.0, 3.0];
        let right = vec![1.0, 2.0, 3.0];
        let score = cosine_similarity(&left, &right);
        assert!((score - 1.0).abs() < 1e-6);
    }

    #[test]
    fn split_text_breaks_long_snippets() {
        let text = "a".repeat(500);
        let parts = split_text_by_sentence_or_size(&text, 120);
        assert!(parts.len() >= 4);
        assert!(parts.iter().all(|part| part.chars().count() <= 120));
    }

    #[test]
    fn alternate_embedding_language_switches_between_en_and_zh() {
        assert_eq!(
            alternate_embedding_language(TextEmbeddingLanguage::English),
            TextEmbeddingLanguage::Chinese
        );
        assert_eq!(
            alternate_embedding_language(TextEmbeddingLanguage::Chinese),
            TextEmbeddingLanguage::English
        );
    }

    #[test]
    fn vector_column_mapping_is_stable() {
        assert_eq!(vector_column_for_language(TextEmbeddingLanguage::English), "vector_en");
        assert_eq!(vector_column_for_language(TextEmbeddingLanguage::Chinese), "vector_zh");
    }
}
