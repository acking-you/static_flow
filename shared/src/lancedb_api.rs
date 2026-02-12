use std::{
    collections::{HashMap, HashSet},
    time::Instant,
};

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
    embedding::{
        detect_language, embed_text_with_language, embed_text_with_model, TextEmbeddingLanguage,
        TextEmbeddingModel,
    },
    normalize_taxonomy_key, Article, ArticleListItem, LocalizedText,
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
pub struct ImageTextSearchResponse {
    pub images: Vec<ImageInfo>,
    pub total: usize,
    pub query: String,
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

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct StatsResponse {
    pub total_articles: usize,
    pub total_tags: usize,
    pub total_categories: usize,
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
        let path = if tag.is_some() || category.is_some() { "filtered_scan" } else { "full_scan" };
        let reason =
            format!("tag_filter={}; category_filter={}", tag.is_some(), category.is_some());

        log_query_path("list_articles", path, path, &reason);
        let started = Instant::now();
        let articles = fetch_article_list(&table, tag, category).await?;
        log_query_result("list_articles", path, articles.len(), started.elapsed().as_millis());
        Ok(articles)
    }

    pub async fn get_article(&self, id: &str) -> Result<Option<Article>> {
        let table = self.articles_table().await?;
        let path = "id_filter_scan";

        log_query_path(
            "get_article",
            path,
            path,
            "id equality filter (no scalar index configured)",
        );
        let started = Instant::now();
        let article = fetch_article_detail(&table, id).await?;
        log_query_result(
            "get_article",
            path,
            usize::from(article.is_some()),
            started.elapsed().as_millis(),
        );
        Ok(article)
    }

    pub async fn list_tags(&self) -> Result<Vec<TagInfo>> {
        let path = "aggregate_from_articles_scan";
        log_query_path("list_tags", path, path, "aggregated from list_articles in-memory");

        let started = Instant::now();
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

        log_query_result("list_tags", path, tags.len(), started.elapsed().as_millis());
        Ok(tags)
    }

    pub async fn list_categories(&self) -> Result<Vec<CategoryInfo>> {
        let started = Instant::now();
        let articles = self.list_articles(None, None).await?;
        let mut category_counts: HashMap<String, usize> = HashMap::new();
        for article in articles {
            *category_counts.entry(article.category).or_insert(0) += 1;
        }

        let mut used_taxonomy_lookup = false;
        let mut description_map: HashMap<String, String> = HashMap::new();
        if let Some(table) = self.taxonomies_table().await? {
            used_taxonomy_lookup = true;
            description_map = fetch_category_descriptions(&table).await?;
        }

        let path = if used_taxonomy_lookup {
            "aggregate_scan_plus_taxonomy_lookup"
        } else {
            "aggregate_scan_only"
        };
        let reason = if used_taxonomy_lookup {
            "taxonomies table found"
        } else {
            "taxonomies table missing, fallback to category name as description"
        };
        log_query_path("list_categories", path, "aggregate_scan_plus_taxonomy_lookup", reason);

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

        log_query_result("list_categories", path, categories.len(), started.elapsed().as_millis());
        Ok(categories)
    }

    pub async fn fetch_stats(&self) -> Result<StatsResponse> {
        let table = self.articles_table().await?;

        let article_path = "count_rows";
        log_query_path(
            "fetch_stats.articles",
            article_path,
            article_path,
            "table.count_rows(None)",
        );
        let article_started = Instant::now();
        let total_articles = table.count_rows(None).await? as usize;
        log_query_result(
            "fetch_stats.articles",
            article_path,
            total_articles,
            article_started.elapsed().as_millis(),
        );

        let tags_path = "projection_scan";
        log_query_path(
            "fetch_stats.tags",
            tags_path,
            tags_path,
            "projection scan on tags column and count distinct values",
        );
        let tags_started = Instant::now();
        let total_tags = count_unique_tags(&table).await?;
        log_query_result(
            "fetch_stats.tags",
            tags_path,
            total_tags,
            tags_started.elapsed().as_millis(),
        );

        let categories_path = "projection_scan";
        log_query_path(
            "fetch_stats.categories",
            categories_path,
            categories_path,
            "projection scan on category column and count distinct values",
        );
        let categories_started = Instant::now();
        let total_categories = count_unique_categories(&table).await?;
        log_query_result(
            "fetch_stats.categories",
            categories_path,
            total_categories,
            categories_started.elapsed().as_millis(),
        );

        Ok(StatsResponse {
            total_articles,
            total_tags,
            total_categories,
        })
    }

    pub async fn search_articles(
        &self,
        keyword: &str,
        limit: Option<usize>,
    ) -> Result<Vec<SearchResult>> {
        let table = self.articles_table().await?;
        let fts_index = inspect_index_for_column(&table, "content", true).await;
        let primary_path = if fts_index.is_some() { "fts_index" } else { "fts_without_index" };
        let primary_reason = format!(
            "{}; result_limit={}",
            index_reason("content", fts_index.as_ref()),
            limit
                .map(|value| value.to_string())
                .unwrap_or_else(|| "none".to_string())
        );

        log_query_path("search_articles.primary", primary_path, "fts_index", &primary_reason);

        let primary_started = Instant::now();
        match search_with_fts(&table, keyword, limit).await {
            Ok(results) if !results.is_empty() => {
                log_query_result(
                    "search_articles.primary",
                    primary_path,
                    results.len(),
                    primary_started.elapsed().as_millis(),
                );
                Ok(results)
            },
            Ok(_) => {
                log_query_result(
                    "search_articles.primary",
                    primary_path,
                    0,
                    primary_started.elapsed().as_millis(),
                );

                let fallback_path = "scan_fallback";
                log_query_path(
                    "search_articles.fallback",
                    fallback_path,
                    "fts_index",
                    "fts returned 0 rows; fallback to in-memory scan",
                );

                let fallback_started = Instant::now();
                let fallback_results = fallback_search(&table, keyword, limit).await?;
                log_query_result(
                    "search_articles.fallback",
                    fallback_path,
                    fallback_results.len(),
                    fallback_started.elapsed().as_millis(),
                );
                Ok(fallback_results)
            },
            Err(err) => {
                log_query_result(
                    "search_articles.primary",
                    primary_path,
                    0,
                    primary_started.elapsed().as_millis(),
                );

                let fallback_path = "scan_fallback";
                let fallback_reason = format!("fts query failed; error={err}");
                log_query_path(
                    "search_articles.fallback",
                    fallback_path,
                    "fts_index",
                    &fallback_reason,
                );

                let fallback_started = Instant::now();
                let fallback_results = fallback_search(&table, keyword, limit).await?;
                log_query_result(
                    "search_articles.fallback",
                    fallback_path,
                    fallback_results.len(),
                    fallback_started.elapsed().as_millis(),
                );
                Ok(fallback_results)
            },
        }
    }

    pub async fn semantic_search(
        &self,
        keyword: &str,
        limit: Option<usize>,
        max_distance: Option<f32>,
        enhanced_highlight: bool,
        hybrid: bool,
        hybrid_rrf_k: Option<f32>,
        hybrid_vector_limit: Option<usize>,
        hybrid_fts_limit: Option<usize>,
    ) -> Result<Vec<SearchResult>> {
        let table = self.articles_table().await?;
        let total_started = Instant::now();

        let mut search_language = detect_language(keyword);
        let mut query_embedding = embed_text_with_language(keyword, search_language);
        let primary_column = vector_column_for_language(search_language);
        let primary_index = inspect_index_for_column(&table, primary_column, false).await;
        let primary_path = if primary_index.is_some() { "vector_index" } else { "vector_scan" };
        let primary_reason = index_reason(primary_column, primary_index.as_ref());

        log_query_path(
            "semantic_search.primary",
            primary_path,
            "vector_index",
            &format!(
                "{primary_reason}; result_limit={}; max_distance={}; \
                 enhanced_highlight={enhanced_highlight}",
                limit
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "none".to_string()),
                max_distance
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "none".to_string())
            ),
        );

        let effective_vector_limit = if hybrid { hybrid_vector_limit.or(limit) } else { limit };

        let primary_started = Instant::now();
        let mut rows = run_semantic_vector_search(
            &table,
            primary_column,
            query_embedding.as_slice(),
            effective_vector_limit,
            max_distance,
        )
        .await?;
        log_query_result(
            "semantic_search.primary",
            primary_path,
            rows.len(),
            primary_started.elapsed().as_millis(),
        );

        let mut selected_column = primary_column;
        let mut selected_path = primary_path;

        if rows.is_empty() {
            let fallback_language = alternate_embedding_language(search_language);
            let fallback_column = vector_column_for_language(fallback_language);
            let fallback_index = inspect_index_for_column(&table, fallback_column, false).await;
            let fallback_path =
                if fallback_index.is_some() { "vector_index" } else { "vector_scan" };
            let fallback_reason = format!(
                "primary_rows=0; {}",
                index_reason(fallback_column, fallback_index.as_ref())
            );

            log_query_path(
                "semantic_search.fallback",
                fallback_path,
                "vector_index",
                &fallback_reason,
            );

            let fallback_embedding = embed_text_with_language(keyword, fallback_language);
            let fallback_started = Instant::now();
            let fallback_rows = run_semantic_vector_search(
                &table,
                fallback_column,
                fallback_embedding.as_slice(),
                effective_vector_limit,
                max_distance,
            )
            .await?;
            log_query_result(
                "semantic_search.fallback",
                fallback_path,
                fallback_rows.len(),
                fallback_started.elapsed().as_millis(),
            );

            if !fallback_rows.is_empty() {
                search_language = fallback_language;
                query_embedding = fallback_embedding;
                rows = fallback_rows;
                selected_column = fallback_column;
                selected_path = fallback_path;
            }
        }

        if hybrid {
            let lexical_limit = hybrid_fts_limit.or(limit);
            let fts_index = inspect_index_for_column(&table, "content", true).await;
            let lexical_primary_path =
                if fts_index.is_some() { "fts_index" } else { "fts_without_index" };
            let lexical_primary_reason = format!(
                "{}; result_limit={}",
                index_reason("content", fts_index.as_ref()),
                lexical_limit
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "none".to_string())
            );
            log_query_path(
                "semantic_search.hybrid.lexical_primary",
                lexical_primary_path,
                "fts_index",
                &lexical_primary_reason,
            );

            let lexical_started = Instant::now();
            let lexical_rows = match search_with_fts_rows(&table, keyword, lexical_limit).await {
                Ok(rows) => {
                    log_query_result(
                        "semantic_search.hybrid.lexical_primary",
                        lexical_primary_path,
                        rows.len(),
                        lexical_started.elapsed().as_millis(),
                    );
                    if rows.is_empty() {
                        let fallback_path = "scan_fallback";
                        log_query_path(
                            "semantic_search.hybrid.lexical_fallback",
                            fallback_path,
                            "fts_index",
                            "fts returned 0 rows in hybrid lexical path; fallback to scan",
                        );
                        let fallback_started = Instant::now();
                        let fallback_rows =
                            fallback_search_rows(&table, keyword, lexical_limit).await?;
                        log_query_result(
                            "semantic_search.hybrid.lexical_fallback",
                            fallback_path,
                            fallback_rows.len(),
                            fallback_started.elapsed().as_millis(),
                        );
                        fallback_rows
                    } else {
                        rows
                    }
                },
                Err(err) => {
                    log_query_result(
                        "semantic_search.hybrid.lexical_primary",
                        lexical_primary_path,
                        0,
                        lexical_started.elapsed().as_millis(),
                    );
                    let fallback_path = "scan_fallback";
                    log_query_path(
                        "semantic_search.hybrid.lexical_fallback",
                        fallback_path,
                        "fts_index",
                        &format!("fts query failed in hybrid lexical path; error={err}"),
                    );
                    let fallback_started = Instant::now();
                    let rows = fallback_search_rows(&table, keyword, lexical_limit).await?;
                    log_query_result(
                        "semantic_search.hybrid.lexical_fallback",
                        fallback_path,
                        rows.len(),
                        fallback_started.elapsed().as_millis(),
                    );
                    rows
                },
            };

            let rrf_k = hybrid_rrf_k
                .filter(|value| value.is_finite() && *value > 0.0)
                .unwrap_or(60.0);
            rows = fuse_hybrid_rrf(rows, lexical_rows, rrf_k);
            if let Some(limit) = limit {
                rows.truncate(limit);
            }
            selected_path = "hybrid_rrf";
            selected_column = "hybrid(vector_en/vector_zh + content_fts)";
            tracing::info!(
                "Hybrid semantic fusion applied; query=semantic_search; rrf_k={rrf_k}; \
                 vector_window={}; lexical_window={}; fused_rows={}",
                effective_vector_limit
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "none".to_string()),
                lexical_limit
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "none".to_string()),
                rows.len()
            );
        }

        let highlight_path =
            if enhanced_highlight { "semantic_snippet_rerank" } else { "fast_excerpt" };
        let highlight_reason =
            if enhanced_highlight { "enhanced_highlight=true" } else { "enhanced_highlight=false" };
        log_query_path(
            "semantic_search.highlight",
            highlight_path,
            "fast_excerpt",
            highlight_reason,
        );

        let highlight_started = Instant::now();
        let results = rows
            .into_iter()
            .map(|row| SearchResult {
                id: row.id,
                title: row.title,
                summary: row.summary.clone(),
                category: row.category,
                date: row.date,
                highlight: if enhanced_highlight {
                    extract_semantic_highlight(
                        &row.content,
                        &row.summary,
                        keyword,
                        query_embedding.as_slice(),
                        search_language,
                    )
                } else {
                    extract_fast_semantic_highlight(&row.content, &row.summary, keyword)
                },
                tags: row.tags,
            })
            .collect::<Vec<_>>();

        log_query_result(
            "semantic_search.highlight",
            highlight_path,
            results.len(),
            highlight_started.elapsed().as_millis(),
        );
        tracing::info!(
            "Semantic search final path; query=semantic_search; selected_path={selected_path}; \
             selected_column={selected_column}; highlight_path={highlight_path}; rows={}; \
             total_elapsed_ms={}",
            results.len(),
            total_started.elapsed().as_millis()
        );

        Ok(results)
    }

    pub async fn related_articles(&self, id: &str, limit: usize) -> Result<Vec<ArticleListItem>> {
        let table = self.articles_table().await?;
        let total_started = Instant::now();

        let vector = fetch_article_vector(&table, id).await?;
        let (vector, vector_column) = match vector {
            Some(value) => value,
            None => {
                log_query_path(
                    "related_articles",
                    "short_circuit_no_vector",
                    "vector_index",
                    "source article has no vector_en/vector_zh",
                );
                log_query_result(
                    "related_articles",
                    "short_circuit_no_vector",
                    0,
                    total_started.elapsed().as_millis(),
                );
                return Ok(vec![]);
            },
        };

        let index_diag = inspect_index_for_column(&table, vector_column, false).await;
        let path = if index_diag.is_some() { "vector_index" } else { "vector_scan" };
        let reason = format!(
            "source_column={vector_column}; {}; limit={limit}",
            index_reason(vector_column, index_diag.as_ref())
        );
        log_query_path("related_articles", path, "vector_index", &reason);

        let filter = format!("{vector_column} IS NOT NULL AND id != '{}'", escape_literal(id));
        let vector_query = table
            .query()
            .nearest_to(vector.as_slice())
            .context("failed to build related query")?;

        let started = Instant::now();
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
        let results = batches_to_article_list(&batch_list)?;
        log_query_result("related_articles", path, results.len(), started.elapsed().as_millis());

        Ok(results)
    }

    pub async fn list_images(&self) -> Result<Vec<ImageInfo>> {
        let table = self.images_table().await?;
        let path = "projection_scan";
        log_query_path("list_images", path, path, "projection scan on images table");

        let started = Instant::now();
        let batches = table
            .query()
            .select(Select::columns(&["id", "filename"]))
            .execute()
            .await?;

        let batch_list = batches.try_collect::<Vec<_>>().await?;
        let images = batches_to_images(&batch_list)?;
        log_query_result("list_images", path, images.len(), started.elapsed().as_millis());
        Ok(images)
    }

    pub async fn search_images_by_text(
        &self,
        query: &str,
        limit: Option<usize>,
        max_distance: Option<f32>,
    ) -> Result<Vec<ImageInfo>> {
        let table = self.images_table().await?;
        let total_started = Instant::now();

        let query_embedding = embed_text_with_model(query, TextEmbeddingModel::ClipVitB32);

        let index_diag = inspect_index_for_column(&table, "vector", false).await;
        let path = if index_diag.is_some() { "vector_index" } else { "vector_scan" };
        let reason = format!(
            "{}; query_model=clip_vit_b32_text; result_limit={}; max_distance={}",
            index_reason("vector", index_diag.as_ref()),
            limit
                .map(|value| value.to_string())
                .unwrap_or_else(|| "none".to_string()),
            max_distance
                .map(|value| value.to_string())
                .unwrap_or_else(|| "none".to_string())
        );
        log_query_path("search_images_by_text", path, "vector_index", &reason);

        let filter = "vector IS NOT NULL".to_string();
        let candidate_count = table.count_rows(Some(filter.clone())).await? as usize;
        if candidate_count == 0 {
            log_query_result("search_images_by_text", path, 0, total_started.elapsed().as_millis());
            return Ok(vec![]);
        }
        let effective_limit = limit.unwrap_or(candidate_count).min(candidate_count);
        if effective_limit == 0 {
            log_query_result("search_images_by_text", path, 0, total_started.elapsed().as_millis());
            return Ok(vec![]);
        }

        let vector_query = table
            .query()
            .nearest_to(query_embedding.as_slice())
            .context("failed to build text-image search query")?;

        let started = Instant::now();
        let mut vector_query = vector_query.only_if(filter).limit(effective_limit);
        if max_distance.is_some() {
            vector_query = vector_query.distance_range(None, max_distance);
        }
        let batches = vector_query
            .select(Select::columns(&["id", "filename", "_distance"]))
            .execute()
            .await?;

        let batch_list = batches.try_collect::<Vec<_>>().await?;
        let images = batches_to_images(&batch_list)?;
        log_query_result(
            "search_images_by_text",
            path,
            images.len(),
            started.elapsed().as_millis(),
        );
        Ok(images)
    }

    pub async fn search_images(
        &self,
        id: &str,
        limit: Option<usize>,
        max_distance: Option<f32>,
    ) -> Result<Vec<ImageInfo>> {
        let table = self.images_table().await?;
        let total_started = Instant::now();

        let vector = fetch_image_vector(&table, id).await?;
        let vector = match vector {
            Some(value) => value,
            None => {
                log_query_path(
                    "search_images",
                    "short_circuit_no_vector",
                    "vector_index",
                    "source image has no vector",
                );
                log_query_result(
                    "search_images",
                    "short_circuit_no_vector",
                    0,
                    total_started.elapsed().as_millis(),
                );
                return Ok(vec![]);
            },
        };

        let index_diag = inspect_index_for_column(&table, "vector", false).await;
        let path = if index_diag.is_some() { "vector_index" } else { "vector_scan" };
        let reason = format!(
            "{}; result_limit={}; max_distance={}",
            index_reason("vector", index_diag.as_ref()),
            limit
                .map(|value| value.to_string())
                .unwrap_or_else(|| "none".to_string()),
            max_distance
                .map(|value| value.to_string())
                .unwrap_or_else(|| "none".to_string())
        );
        log_query_path("search_images", path, "vector_index", &reason);

        let filter = format!("id != '{}' AND vector IS NOT NULL", escape_literal(id));
        let candidate_count = table.count_rows(Some(filter.clone())).await? as usize;
        if candidate_count == 0 {
            log_query_result("search_images", path, 0, total_started.elapsed().as_millis());
            return Ok(vec![]);
        }
        let effective_limit = limit.unwrap_or(candidate_count).min(candidate_count);
        if effective_limit == 0 {
            log_query_result("search_images", path, 0, total_started.elapsed().as_millis());
            return Ok(vec![]);
        }

        let vector_query = table
            .query()
            .nearest_to(vector.as_slice())
            .context("failed to build image search query")?;

        let started = Instant::now();
        let mut vector_query = vector_query.only_if(filter).limit(effective_limit);
        if max_distance.is_some() {
            vector_query = vector_query.distance_range(None, max_distance);
        }
        let batches = vector_query
            .select(Select::columns(&["id", "filename", "_distance"]))
            .execute()
            .await?;

        let batch_list = batches.try_collect::<Vec<_>>().await?;
        let images = batches_to_images(&batch_list)?;
        log_query_result("search_images", path, images.len(), started.elapsed().as_millis());
        Ok(images)
    }

    pub async fn get_image(
        &self,
        id_or_filename: &str,
        prefer_thumbnail: bool,
    ) -> Result<Option<ImageBlob>> {
        let table = self.images_table().await?;
        let path = "id_or_filename_filter_scan";
        let reason = format!("prefer_thumbnail={prefer_thumbnail}");
        log_query_path("get_image", path, path, &reason);

        let escaped = escape_literal(id_or_filename);
        let filter = format!("filename = '{}' OR id = '{}'", escaped, escaped);
        let started = Instant::now();
        let batches = table
            .query()
            .only_if(filter)
            .limit(1)
            .select(Select::columns(&["data", "thumbnail", "filename"]))
            .execute()
            .await?;

        let batch_list = batches.try_collect::<Vec<_>>().await?;
        let image = extract_image_bytes(&batch_list, prefer_thumbnail)?;
        log_query_result(
            "get_image",
            path,
            usize::from(image.is_some()),
            started.elapsed().as_millis(),
        );

        Ok(image.map(|(bytes, filename)| ImageBlob {
            mime_type: image_mime_type(&filename).to_string(),
            bytes,
            filename,
        }))
    }
}


#[derive(Debug, Clone)]
struct IndexDiagnostic {
    name: String,
    index_type: String,
    indexed_rows: Option<u64>,
    unindexed_rows: Option<u64>,
}

fn log_query_path(query: &str, path: &str, fastest_path: &str, reason: &str) {
    tracing::info!(
        "Query path selected; query={query}; path={path}; fastest_path={fastest_path};          \
         is_fastest={}; reason={reason}",
        path == fastest_path
    );
}

fn log_query_result(query: &str, path: &str, rows: usize, elapsed_ms: u128) {
    tracing::info!(
        "Query completed; query={query}; path={path}; rows={rows}; elapsed_ms={elapsed_ms}"
    );
}

fn index_reason(column: &str, index: Option<&IndexDiagnostic>) -> String {
    match index {
        Some(index) => format!(
            "column={column}; index={}; type={}; indexed_rows={}; unindexed_rows={}",
            index.name,
            index.index_type,
            optional_count_text(index.indexed_rows),
            optional_count_text(index.unindexed_rows)
        ),
        None => format!("column={column}; no_index_found"),
    }
}

fn optional_count_text(value: Option<u64>) -> String {
    match value {
        Some(value) => value.to_string(),
        None => "unknown".to_string(),
    }
}

fn is_fts_index_type(index_type: &lancedb::index::IndexType) -> bool {
    index_type.to_string().to_ascii_uppercase().contains("FTS")
}

async fn inspect_index_for_column(
    table: &Table,
    column: &str,
    require_fts: bool,
) -> Option<IndexDiagnostic> {
    if !tracing::enabled!(tracing::Level::INFO) {
        return None;
    }

    let indexes = match table.list_indices().await {
        Ok(indexes) => indexes,
        Err(err) => {
            tracing::warn!(
                "Failed to inspect indices; table={}; column={column}; error={err}",
                table.name()
            );
            return None;
        },
    };

    let index = indexes.into_iter().find(|index| {
        index.columns.len() == 1
            && index.columns[0] == column
            && (!require_fts || is_fts_index_type(&index.index_type))
    })?;

    let (indexed_rows, unindexed_rows) = match table.index_stats(&index.name).await {
        Ok(Some(stats)) => {
            (Some(stats.num_indexed_rows as u64), Some(stats.num_unindexed_rows as u64))
        },
        Ok(None) => (None, None),
        Err(err) => {
            tracing::warn!(
                "Failed to inspect index stats; table={}; index={}; column={column}; error={err}",
                table.name(),
                index.name
            );
            (None, None)
        },
    };

    Some(IndexDiagnostic {
        name: index.name,
        index_type: index.index_type.to_string(),
        indexed_rows,
        unindexed_rows,
    })
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

async fn count_unique_tags(table: &Table) -> Result<usize> {
    let batches = table
        .query()
        .select(Select::columns(&["tags"]))
        .execute()
        .await?;

    let batch_list = batches.try_collect::<Vec<_>>().await?;
    let mut unique_tags: HashSet<String> = HashSet::new();

    for batch in &batch_list {
        let tags = list_array(batch, "tags")?;
        for row in 0..batch.num_rows() {
            for tag in value_string_list(tags, row) {
                let normalized = tag.trim().to_lowercase();
                if !normalized.is_empty() {
                    unique_tags.insert(normalized);
                }
            }
        }
    }

    Ok(unique_tags.len())
}

async fn count_unique_categories(table: &Table) -> Result<usize> {
    let batches = table
        .query()
        .select(Select::columns(&["category"]))
        .execute()
        .await?;

    let batch_list = batches.try_collect::<Vec<_>>().await?;
    let mut unique_categories: HashSet<String> = HashSet::new();

    for batch in &batch_list {
        let categories = string_array(batch, "category")?;
        for row in 0..batch.num_rows() {
            let normalized = normalize_taxonomy_key(&value_string(categories, row));
            if !normalized.is_empty() {
                unique_categories.insert(normalized);
            }
        }
    }

    Ok(unique_categories.len())
}

async fn fetch_article_detail(table: &Table, id: &str) -> Result<Option<Article>> {
    let filter = format!("id = '{}'", escape_literal(id));
    let full_columns = [
        "id",
        "title",
        "summary",
        "content",
        "content_en",
        "detailed_summary",
        "tags",
        "category",
        "author",
        "date",
        "featured_image",
        "read_time",
    ];
    let base_columns = [
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
    ];

    let query = table
        .query()
        .only_if(filter.clone())
        .limit(1)
        .select(Select::columns(&full_columns));
    let batch_list = match query.execute().await {
        Ok(batches) => batches.try_collect::<Vec<_>>().await?,
        Err(err) => {
            let err_text = err.to_string();
            let has_missing_new_columns = err_text.contains("content_en")
                || err_text.contains("detailed_summary")
                || err_text.contains("missing column");
            if !has_missing_new_columns {
                return Err(err.into());
            }

            tracing::warn!(
                "Article table appears to be on legacy schema (missing \
                 content_en/detailed_summary). Falling back to base detail query: {err_text}"
            );
            table
                .query()
                .only_if(filter)
                .limit(1)
                .select(Select::columns(&base_columns))
                .execute()
                .await?
                .try_collect::<Vec<_>>()
                .await?
        },
    };
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
    limit: Option<usize>,
    max_distance: Option<f32>,
) -> Result<Vec<SearchArticleRow>> {
    let filter = format!("{vector_column} IS NOT NULL");
    let candidate_count = table.count_rows(Some(filter.clone())).await? as usize;
    if candidate_count == 0 {
        return Ok(vec![]);
    }
    let effective_limit = limit.unwrap_or(candidate_count).min(candidate_count);
    if effective_limit == 0 {
        return Ok(vec![]);
    }

    let vector_query = table
        .query()
        .nearest_to(query_embedding)
        .context("failed to build semantic query")?;

    let mut vector_query = vector_query
        .column(vector_column)
        .only_if(filter)
        .limit(effective_limit);
    if max_distance.is_some() {
        vector_query = vector_query.distance_range(None, max_distance);
    }

    let batches = vector_query
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

#[derive(Debug, Clone)]
struct SearchArticleRow {
    id: String,
    title: String,
    summary: String,
    content: String,
    tags: Vec<String>,
    category: String,
    date: String,
}

async fn search_with_fts_rows(
    table: &Table,
    keyword: &str,
    limit: Option<usize>,
) -> Result<Vec<SearchArticleRow>> {
    if limit == Some(0) {
        return Ok(vec![]);
    }

    let mut query = table
        .query()
        .full_text_search(FullTextSearchQuery::new(keyword.to_string()));
    if let Some(limit) = limit {
        query = query.limit(limit);
    }

    let batches = query
        .select(Select::columns(&[
            "id", "title", "summary", "content", "tags", "category", "date", "_score",
        ]))
        .execute()
        .await?;

    let batch_list = batches.try_collect::<Vec<_>>().await?;
    batches_to_search_rows(&batch_list)
}

async fn search_with_fts(
    table: &Table,
    keyword: &str,
    limit: Option<usize>,
) -> Result<Vec<SearchResult>> {
    let rows = search_with_fts_rows(table, keyword, limit).await?;

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

async fn fallback_search_rows(
    table: &Table,
    keyword: &str,
    limit: Option<usize>,
) -> Result<Vec<SearchArticleRow>> {
    let batches = table
        .query()
        .select(Select::columns(&["id", "title", "summary", "content", "tags", "category", "date"]))
        .execute()
        .await?;

    let batch_list = batches.try_collect::<Vec<_>>().await?;
    let rows = batches_to_search_rows(&batch_list)?;

    let keyword_lower = keyword.to_lowercase();
    let mut scored = Vec::new();
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
            scored.push((row, score));
        }
    }

    scored.sort_by(|a, b| b.1.cmp(&a.1));
    let mut rows = scored.into_iter().map(|(row, _)| row).collect::<Vec<_>>();
    if let Some(limit) = limit {
        rows.truncate(limit);
    }
    Ok(rows)
}

async fn fallback_search(
    table: &Table,
    keyword: &str,
    limit: Option<usize>,
) -> Result<Vec<SearchResult>> {
    let rows = fallback_search_rows(table, keyword, limit).await?;
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

fn fuse_hybrid_rrf(
    vector_rows: Vec<SearchArticleRow>,
    lexical_rows: Vec<SearchArticleRow>,
    rrf_k: f32,
) -> Vec<SearchArticleRow> {
    #[derive(Debug)]
    struct RrfAccum {
        score: f32,
        best_rank: usize,
        row: SearchArticleRow,
    }

    let mut fused: HashMap<String, RrfAccum> = HashMap::new();
    let rrf_score = |rank: usize| -> f32 { 1.0 / (rrf_k + rank as f32 + 1.0) };

    for (rank, row) in vector_rows.into_iter().enumerate() {
        let row_id = row.id.clone();
        let boost = rrf_score(rank);
        let entry = fused.entry(row_id).or_insert_with(|| RrfAccum {
            score: 0.0,
            best_rank: rank,
            row: row.clone(),
        });
        entry.score += boost;
        if rank < entry.best_rank {
            entry.best_rank = rank;
            entry.row = row;
        }
    }

    for (rank, row) in lexical_rows.into_iter().enumerate() {
        let row_id = row.id.clone();
        let boost = rrf_score(rank);
        let entry = fused.entry(row_id).or_insert_with(|| RrfAccum {
            score: 0.0,
            best_rank: rank,
            row: row.clone(),
        });
        entry.score += boost;
        if rank < entry.best_rank {
            entry.best_rank = rank;
            entry.row = row;
        }
    }

    let mut merged = fused.into_values().collect::<Vec<_>>();
    merged.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.row.id.cmp(&b.row.id))
    });
    merged.into_iter().map(|entry| entry.row).collect()
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
        let content_en = optional_string_array(batch, "content_en");
        let detailed_summary = optional_string_array(batch, "detailed_summary");
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
                content_en: content_en.and_then(|array| value_string_opt(array, row)),
                detailed_summary: detailed_summary
                    .and_then(|array| value_string_opt(array, row))
                    .and_then(parse_localized_text),
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

fn optional_string_array<'a>(batch: &'a RecordBatch, name: &str) -> Option<&'a StringArray> {
    batch
        .schema()
        .index_of(name)
        .ok()
        .and_then(|idx| batch.column(idx).as_any().downcast_ref::<StringArray>())
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

fn parse_localized_text(raw: String) -> Option<LocalizedText> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    match serde_json::from_str::<LocalizedText>(trimmed) {
        Ok(parsed) => parsed.normalized(),
        Err(err) => {
            tracing::warn!(
                "Failed to parse detailed_summary as JSON; fallback to zh-only text: {err}"
            );
            LocalizedText {
                zh: Some(trimmed.to_string()),
                en: None,
            }
            .normalized()
        },
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

/// Build a low-cost semantic-search highlight without running snippet
/// reranking.
///
/// This is the default path when `enhanced_highlight=false`.
///
/// Strategy:
/// - Prefer lexical `<mark>` on `content` when possible.
/// - If `content` has no lexical hit, try lexical `<mark>` on `summary`.
/// - If there is still no lexical hit, return a short excerpt from `summary`.
/// - If `summary` is empty, return a short excerpt from `content`.
fn extract_fast_semantic_highlight(content: &str, summary: &str, keyword: &str) -> String {
    const MAX_SNIPPET_CHARS: usize = 180;

    let content = content.trim();
    let summary = summary.trim();
    let keyword = keyword.trim();

    if !keyword.is_empty() {
        if !content.is_empty() && find_case_insensitive_match_range(content, keyword).is_some() {
            return extract_highlight(content, keyword);
        }

        if !summary.is_empty() && find_case_insensitive_match_range(summary, keyword).is_some() {
            return extract_highlight(summary, keyword);
        }
    }

    if !summary.is_empty() {
        return excerpt_with_ellipsis(summary, MAX_SNIPPET_CHARS);
    }

    excerpt_with_ellipsis(content, MAX_SNIPPET_CHARS)
}

/// Build a semantic-search highlight snippet with optional lexical emphasis.
///
/// This function is intentionally more expensive than the fast path because it
/// reranks candidate snippets using embeddings.
///
/// Flow (high precision mode):
///
/// ```text
/// Query + Article Content
///          |
///          v
/// [1] Lexical hit in full content?
///      | yes --------------------------> return extract_highlight(content, keyword)
///      | no
///      v
/// [2] Split content into snippet candidates
///      (paragraph / sentence chunks)
///          |
///          v
/// [3] For each candidate:
///      - embed candidate
///      - compute cosine(query_embedding, candidate_embedding)
///      - compute lexical overlap score
///      - final_score = semantic_score + lexical_score * 0.15
///          |
///          v
/// [4] Pick best-scoring snippet
///      | lexical overlap token found --> return extract_highlight(best_snippet, token)
///      | no overlap                  --> return excerpt(best_snippet)
///          |
///          v
/// [5] If no candidate exists: fallback to summary/content excerpt
/// ```
///
/// Why this exists:
/// - Vector retrieval answers "which article is relevant".
/// - This stage answers "which fragment of that article should be shown".
/// - The result improves UX, especially when query terms are paraphrased.
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
