use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Instant,
};

use anyhow::{Context, Result};
use arrow_array::{
    builder::{Int32Builder, StringBuilder, TimestampMillisecondBuilder},
    Array, ArrayRef, BinaryArray, FixedSizeListArray, Float32Array, Int32Array, ListArray,
    RecordBatch, RecordBatchIterator, StringArray, TimestampMillisecondArray,
};
use arrow_schema::{DataType, Field, Schema, TimeUnit};
use chrono::{Duration as ChronoDuration, FixedOffset, NaiveDate, Utc};
use futures::TryStreamExt;
use lancedb::{
    connect,
    index::scalar::FullTextSearchQuery,
    query::{ExecutableQuery, QueryBase, Select},
    table::OptimizeAction,
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
    pub offset: usize,
    pub limit: usize,
    pub has_more: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ImageSearchResponse {
    pub images: Vec<ImageInfo>,
    pub total: usize,
    pub query_id: String,
    pub offset: usize,
    pub limit: usize,
    pub has_more: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ImageTextSearchResponse {
    pub images: Vec<ImageInfo>,
    pub total: usize,
    pub query: String,
    pub offset: usize,
    pub limit: usize,
    pub has_more: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ArticleListResponse {
    pub articles: Vec<ArticleListItem>,
    pub total: usize,
    #[serde(default)]
    pub offset: usize,
    #[serde(default)]
    pub limit: usize,
    #[serde(default)]
    pub has_more: bool,
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

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct ArticleViewPoint {
    pub key: String,
    pub views: u32,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ArticleViewTrackResponse {
    pub article_id: String,
    pub counted: bool,
    pub total_views: usize,
    pub timezone: String,
    pub today_views: u32,
    pub daily_points: Vec<ArticleViewPoint>,
    pub server_time_ms: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ArticleViewTrendResponse {
    pub article_id: String,
    pub timezone: String,
    pub granularity: String,
    pub day: Option<String>,
    pub total_views: usize,
    pub points: Vec<ArticleViewPoint>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ApiBehaviorEvent {
    pub event_id: String,
    pub occurred_at: i64,
    pub client_source: String,
    pub method: String,
    pub path: String,
    pub query: String,
    pub page_path: String,
    pub referrer: Option<String>,
    pub status_code: i32,
    pub latency_ms: i32,
    pub client_ip: String,
    pub ip_region: String,
    pub ua_raw: Option<String>,
    pub device_type: String,
    pub os_family: String,
    pub browser_family: String,
    pub request_id: String,
    pub trace_id: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct ApiBehaviorBucket {
    pub key: String,
    pub count: u32,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ApiBehaviorOverviewResponse {
    pub timezone: String,
    pub days: usize,
    pub total_events: usize,
    pub unique_ips: usize,
    pub unique_pages: usize,
    pub avg_latency_ms: f64,
    pub timeseries: Vec<ApiBehaviorBucket>,
    pub top_endpoints: Vec<ApiBehaviorBucket>,
    pub top_pages: Vec<ApiBehaviorBucket>,
    pub device_distribution: Vec<ApiBehaviorBucket>,
    pub browser_distribution: Vec<ApiBehaviorBucket>,
    pub os_distribution: Vec<ApiBehaviorBucket>,
    pub region_distribution: Vec<ApiBehaviorBucket>,
    pub recent_events: Vec<ApiBehaviorEvent>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct NewApiBehaviorEventInput {
    pub event_id: String,
    pub occurred_at: i64,
    pub client_source: String,
    pub method: String,
    pub path: String,
    pub query: String,
    pub page_path: String,
    pub referrer: Option<String>,
    pub status_code: i32,
    pub latency_ms: i32,
    pub client_ip: String,
    pub ip_region: String,
    pub ua_raw: Option<String>,
    pub device_type: String,
    pub os_family: String,
    pub browser_family: String,
    pub request_id: String,
    pub trace_id: String,
}

#[derive(Debug, Clone)]
pub struct ImageBlob {
    pub bytes: Vec<u8>,
    pub filename: String,
    pub mime_type: String,
}

pub const CONTENT_TABLE_NAMES: &[&str] = &[
    "articles",
    "images",
    "taxonomies",
    "article_views",
    "api_behavior_events",
];

pub struct StaticFlowDataStore {
    db: Connection,
    articles_table: String,
    images_table: String,
    taxonomies_table: String,
    article_views_table: String,
    api_behavior_table: String,
}

impl StaticFlowDataStore {
    pub fn connection(&self) -> &Connection {
        &self.db
    }

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
            article_views_table: "article_views".to_string(),
            api_behavior_table: "api_behavior_events".to_string(),
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

    async fn article_views_table(&self) -> Result<Table> {
        match self
            .db
            .open_table(&self.article_views_table)
            .execute()
            .await
        {
            Ok(table) => Ok(table),
            Err(_) => {
                let schema = article_view_schema();
                let batch = RecordBatch::new_empty(schema.clone());
                let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema.clone());
                self.db
                    .create_table(&self.article_views_table, Box::new(batches))
                    .execute()
                    .await
                    .context("failed to create article_views table")?;
                self.db
                    .open_table(&self.article_views_table)
                    .execute()
                    .await
                    .context("failed to open article_views table")
            },
        }
    }

    async fn api_behavior_table(&self) -> Result<Table> {
        match self.db.open_table(&self.api_behavior_table).execute().await {
            Ok(table) => Ok(table),
            Err(_) => {
                let schema = api_behavior_schema();
                let batch = RecordBatch::new_empty(schema.clone());
                let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema.clone());
                self.db
                    .create_table(&self.api_behavior_table, Box::new(batches))
                    .execute()
                    .await
                    .context("failed to create api_behavior_events table")?;
                self.db
                    .open_table(&self.api_behavior_table)
                    .execute()
                    .await
                    .context("failed to open api_behavior_events table")
            },
        }
    }

    pub async fn append_api_behavior_event(&self, input: NewApiBehaviorEventInput) -> Result<()> {
        self.append_api_behavior_events(vec![input]).await
    }

    pub async fn append_api_behavior_events(
        &self,
        inputs: Vec<NewApiBehaviorEventInput>,
    ) -> Result<()> {
        if inputs.is_empty() {
            return Ok(());
        }
        let table = self.api_behavior_table().await?;
        let now_ms = Utc::now().timestamp_millis();
        let records: Vec<ApiBehaviorRecord> = inputs
            .into_iter()
            .map(|input| ApiBehaviorRecord {
                event_id: normalize_required_text(input.event_id, 96, "evt"),
                occurred_at: input.occurred_at,
                client_source: normalize_required_text(input.client_source, 24, "unknown"),
                method: normalize_required_text(input.method, 16, "GET"),
                path: normalize_required_text(input.path, 512, "/"),
                query: normalize_text(input.query, 2048),
                page_path: normalize_required_text(input.page_path, 512, "unknown"),
                referrer: normalize_optional_text(input.referrer, 1024),
                status_code: input.status_code.max(0),
                latency_ms: input.latency_ms.max(0),
                client_ip: normalize_required_text(input.client_ip, 64, "unknown"),
                ip_region: normalize_required_text(input.ip_region, 128, "Unknown"),
                ua_raw: normalize_optional_text(input.ua_raw, 1024),
                device_type: normalize_required_text(input.device_type, 24, "unknown"),
                os_family: normalize_required_text(input.os_family, 48, "unknown"),
                browser_family: normalize_required_text(input.browser_family, 48, "unknown"),
                request_id: normalize_required_text(input.request_id, 128, "unknown"),
                trace_id: normalize_required_text(input.trace_id, 128, "unknown"),
                created_at: now_ms,
                updated_at: now_ms,
            })
            .collect();
        append_api_behavior_records(&table, &records).await
    }

    pub async fn list_api_behavior_events(
        &self,
        since_ms: Option<i64>,
        until_ms: Option<i64>,
        limit: Option<usize>,
    ) -> Result<Vec<ApiBehaviorEvent>> {
        let mut filters = Vec::new();
        if let Some(min) = since_ms {
            filters
                .push(format!("occurred_at >= arrow_cast({min}, 'Timestamp(Millisecond, None)')"));
        }
        if let Some(max) = until_ms {
            filters
                .push(format!("occurred_at < arrow_cast({max}, 'Timestamp(Millisecond, None)')"));
        }

        let filter = if filters.is_empty() {
            None
        } else {
            Some(filters.join(" AND "))
        };
        let mut events = self.query_api_behavior_events(filter, limit, None).await?;
        events.sort_by(|left, right| right.occurred_at.cmp(&left.occurred_at));
        Ok(events)
    }

    pub async fn count_api_behavior_events_with_filter(&self, filter: Option<String>) -> Result<usize> {
        let table = self.api_behavior_table().await?;
        let total = table
            .count_rows(filter)
            .await
            .context("failed to count api behavior events")?;
        Ok(total as usize)
    }

    pub async fn query_api_behavior_events(
        &self,
        filter: Option<String>,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Result<Vec<ApiBehaviorEvent>> {
        let table = self.api_behavior_table().await?;
        let mut q = table.query();
        if let Some(filter) = filter {
            q = q.only_if(filter);
        }
        if let Some(off) = offset {
            q = q.offset(off);
        }
        if let Some(lim) = limit {
            q = q.limit(lim.max(1));
        }
        let batches = q
            .select(Select::columns(&[
                "event_id",
                "occurred_at",
                "client_source",
                "method",
                "path",
                "query",
                "page_path",
                "referrer",
                "status_code",
                "latency_ms",
                "client_ip",
                "ip_region",
                "ua_raw",
                "device_type",
                "os_family",
                "browser_family",
                "request_id",
                "trace_id",
            ]))
            .execute()
            .await?
            .try_collect::<Vec<_>>()
            .await?;
        batches_to_api_behavior_events(&batches)
    }

    pub async fn cleanup_api_behavior_before(&self, before_ms: i64) -> Result<usize> {
        let table = self.api_behavior_table().await?;
        let filter =
            format!("occurred_at < arrow_cast({before_ms}, 'Timestamp(Millisecond, None)')");
        let deleted = table
            .count_rows(Some(filter.clone()))
            .await
            .context("failed to count api behavior rows before cleanup")?
            as usize;
        if deleted == 0 {
            return Ok(0);
        }

        table
            .delete(&filter)
            .await
            .context("failed to cleanup api behavior rows")?;
        Ok(deleted)
    }

    /// Compact the api_behavior_events table to merge small fragments.
    /// This reduces the number of open file descriptors and improves query
    /// performance.
    pub async fn compact_api_behavior_table(&self) -> Result<()> {
        let table = self.api_behavior_table().await?;
        table
            .optimize(OptimizeAction::All)
            .await
            .context("failed to compact api_behavior_events table")?;
        table
            .optimize(OptimizeAction::Prune {
                older_than: Some(ChronoDuration::hours(1)),
                delete_unverified: Some(false),
                error_if_tagged_old_versions: Some(false),
            })
            .await
            .context("failed to prune api_behavior_events table")?;
        Ok(())
    }

    pub async fn track_article_view(
        &self,
        article_id: &str,
        client_fingerprint: &str,
        daily_window_days: usize,
        dedupe_window_seconds: u64,
        max_daily_window_days: usize,
    ) -> Result<ArticleViewTrackResponse> {
        let table = self.article_views_table().await?;
        let now = Utc::now();
        let now_ms = now.timestamp_millis();
        let now_local = now.with_timezone(&shanghai_tz());
        let day_bucket = now_local.format("%Y-%m-%d").to_string();
        let hour_bucket = now_local.format("%Y-%m-%d %H").to_string();
        let dedupe_window_ms = (dedupe_window_seconds.max(1) as i64) * 1_000;
        let dedupe_bucket = now_ms / dedupe_window_ms;
        let record_id = format!("{article_id}:{client_fingerprint}:{dedupe_bucket}");
        let escaped_id = escape_literal(&record_id);
        let escaped_article_id = escape_literal(article_id);
        let escaped_day = escape_literal(&day_bucket);
        let counted = table
            .count_rows(Some(format!("id = '{escaped_id}'")))
            .await
            .context("failed to check view dedupe key")?
            == 0;

        let record = ArticleViewRecord {
            id: record_id,
            article_id: article_id.to_string(),
            viewed_at: now_ms,
            day_bucket: day_bucket.clone(),
            hour_bucket: hour_bucket.clone(),
            client_fingerprint: client_fingerprint.to_string(),
            created_at: now_ms,
            updated_at: now_ms,
        };
        upsert_article_view_record(&table, &record).await?;

        let window = normalize_daily_window(daily_window_days, max_daily_window_days);
        let since_date = now_local.date_naive() - ChronoDuration::days(window as i64);
        let since_day = since_date.format("%Y-%m-%d").to_string();

        let (total_views_result, today_views_result, day_counts_result) = futures::join!(
            async {
                table
                    .count_rows(Some(format!("article_id = '{escaped_article_id}'")))
                    .await
                    .context("failed to count total article views")
            },
            async {
                table
                    .count_rows(Some(format!(
                        "article_id = '{escaped_article_id}' AND day_bucket = '{escaped_day}'"
                    )))
                    .await
                    .context("failed to count today's views")
            },
            fetch_article_view_day_counts(&table, article_id, Some(&since_day)),
        );

        let total_views = total_views_result?;
        let today_views = today_views_result? as u32;
        let day_counts = day_counts_result?;
        let daily_points = build_recent_day_points(&day_counts, &day_bucket, window)?;

        Ok(ArticleViewTrackResponse {
            article_id: article_id.to_string(),
            counted,
            total_views,
            timezone: SHANGHAI_TIMEZONE.to_string(),
            today_views,
            daily_points,
            server_time_ms: now_ms,
        })
    }

    pub async fn fetch_article_view_trend_day(
        &self,
        article_id: &str,
        days: usize,
        max_days: usize,
    ) -> Result<ArticleViewTrendResponse> {
        let table = self.article_views_table().await?;
        let now_local = Utc::now().with_timezone(&shanghai_tz());
        let today_bucket = now_local.format("%Y-%m-%d").to_string();
        let window = normalize_daily_window(days, max_days);
        let since_date = now_local.date_naive() - ChronoDuration::days(window as i64);
        let since_day = since_date.format("%Y-%m-%d").to_string();
        let day_counts =
            fetch_article_view_day_counts(&table, article_id, Some(&since_day)).await?;
        let points = build_recent_day_points(&day_counts, &today_bucket, window)?;
        let total_views = table
            .count_rows(Some(format!("article_id = '{}'", escape_literal(article_id))))
            .await
            .context("failed to count total article views")? as usize;

        Ok(ArticleViewTrendResponse {
            article_id: article_id.to_string(),
            timezone: SHANGHAI_TIMEZONE.to_string(),
            granularity: "day".to_string(),
            day: None,
            total_views,
            points,
        })
    }

    pub async fn fetch_article_view_trend_hour(
        &self,
        article_id: &str,
        day: &str,
    ) -> Result<ArticleViewTrendResponse> {
        NaiveDate::parse_from_str(day, "%Y-%m-%d")
            .with_context(|| format!("invalid day format: {day}; expected YYYY-MM-DD"))?;

        let table = self.article_views_table().await?;
        let hour_counts = fetch_article_view_hour_counts_for_day(&table, article_id, day).await?;
        let points = (0..24)
            .map(|hour| {
                let key = format!("{hour:02}");
                ArticleViewPoint {
                    views: *hour_counts.get(&key).unwrap_or(&0),
                    key,
                }
            })
            .collect::<Vec<_>>();
        let total_views = table
            .count_rows(Some(format!("article_id = '{}'", escape_literal(article_id))))
            .await
            .context("failed to count total article views")? as usize;

        Ok(ArticleViewTrendResponse {
            article_id: article_id.to_string(),
            timezone: SHANGHAI_TIMEZONE.to_string(),
            granularity: "hour".to_string(),
            day: Some(day.to_string()),
            total_views,
            points,
        })
    }

    pub async fn list_articles(
        &self,
        tag: Option<&str>,
        category: Option<&str>,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Result<ArticleListResponse> {
        let table = self.articles_table().await?;
        let path = if tag.is_some() || category.is_some() { "filtered_scan" } else { "full_scan" };
        let reason =
            format!("tag_filter={}; category_filter={}", tag.is_some(), category.is_some());

        log_query_path("list_articles", path, path, &reason);
        let started = Instant::now();
        let all_articles = fetch_article_list(&table, tag, category).await?;
        let total = all_articles.len();
        log_query_result("list_articles", path, total, started.elapsed().as_millis());

        let off = offset.unwrap_or(0);
        let (articles, lim, has_more) = match limit {
            Some(l) => {
                let page: Vec<_> = all_articles.into_iter().skip(off).take(l).collect();
                let has_more = off + l < total;
                (page, l, has_more)
            },
            None => {
                let len = all_articles.len();
                (all_articles, len, false)
            },
        };

        Ok(ArticleListResponse {
            articles,
            total,
            offset: off,
            limit: lim,
            has_more,
        })
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

    pub async fn article_exists(&self, id: &str) -> Result<bool> {
        let table = self.articles_table().await?;
        let filter = format!("id = '{}'", escape_literal(id));
        let count = table
            .count_rows(Some(filter))
            .await
            .context("failed to check article existence")?;
        Ok(count > 0)
    }

    pub async fn get_article_raw_markdown(&self, id: &str, lang: &str) -> Result<Option<String>> {
        let table = self.articles_table().await?;
        let path = "id_filter_scan";
        let reason = format!("raw markdown query; lang={lang}");
        log_query_path("get_article_raw_markdown", path, path, &reason);

        let started = Instant::now();
        let raw = fetch_article_raw_markdown(&table, id, lang).await?;
        log_query_result(
            "get_article_raw_markdown",
            path,
            usize::from(raw.is_some()),
            started.elapsed().as_millis(),
        );
        Ok(raw)
    }

    pub async fn list_tags(&self) -> Result<Vec<TagInfo>> {
        let path = "aggregate_from_articles_scan";
        log_query_path("list_tags", path, path, "aggregated from list_articles in-memory");

        let started = Instant::now();
        let articles = self.list_articles(None, None, None, None).await?.articles;
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
        let articles = self.list_articles(None, None, None, None).await?.articles;
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

    #[allow(clippy::too_many_arguments)]
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
        let effective_vector_limit = if hybrid { hybrid_vector_limit.or(limit) } else { limit };
        let vector_selection = run_semantic_vector_search_with_fallback(
            &table,
            keyword,
            effective_vector_limit,
            max_distance,
            enhanced_highlight,
        )
        .await?;

        let search_language = vector_selection.search_language;
        let query_embedding = vector_selection.query_embedding;
        let mut rows = vector_selection.rows;
        let mut selected_column = vector_selection.selected_column;
        let mut selected_path = vector_selection.selected_path;

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
        let (images, _, _) = self.list_images_paged(None, 0).await?;
        Ok(images)
    }

    pub async fn list_images_paged(
        &self,
        limit: Option<usize>,
        offset: usize,
    ) -> Result<(Vec<ImageInfo>, usize, bool)> {
        let table = self.images_table().await?;
        let path = "projection_scan";
        let reason = format!(
            "projection scan on images table; result_limit={}; result_offset={offset}",
            limit
                .map(|value| value.to_string())
                .unwrap_or_else(|| "none".to_string())
        );
        log_query_path("list_images", path, path, &reason);

        let started = Instant::now();
        let total = table.count_rows(None).await? as usize;
        let effective_offset = offset.min(total);
        let max_fetchable = total.saturating_sub(effective_offset);
        if max_fetchable == 0 {
            log_query_result("list_images", path, 0, started.elapsed().as_millis());
            return Ok((vec![], total, false));
        }
        let effective_limit = limit.unwrap_or(max_fetchable).min(max_fetchable);
        if effective_limit == 0 {
            log_query_result("list_images", path, 0, started.elapsed().as_millis());
            return Ok((vec![], total, false));
        }
        let fetch_limit = effective_limit.saturating_add(1);
        let batches = table
            .query()
            .select(Select::columns(&["id", "filename"]))
            .offset(effective_offset)
            .limit(fetch_limit)
            .execute()
            .await?;

        let batch_list = batches.try_collect::<Vec<_>>().await?;
        let mut images = batches_to_images(&batch_list)?;
        let has_more = images.len() > effective_limit;
        if has_more {
            images.truncate(effective_limit);
        }
        log_query_result("list_images", path, images.len(), started.elapsed().as_millis());
        Ok((images, total, has_more))
    }

    pub async fn search_images_by_text(
        &self,
        query: &str,
        limit: Option<usize>,
        max_distance: Option<f32>,
    ) -> Result<Vec<ImageInfo>> {
        let (images, _, _) = self
            .search_images_by_text_paged(query, limit, 0, max_distance)
            .await?;
        Ok(images)
    }

    pub async fn search_images_by_text_paged(
        &self,
        query: &str,
        limit: Option<usize>,
        offset: usize,
        max_distance: Option<f32>,
    ) -> Result<(Vec<ImageInfo>, usize, bool)> {
        let table = self.images_table().await?;
        let total_started = Instant::now();

        let query_embedding = embed_text_with_model(query, TextEmbeddingModel::ClipVitB32);

        let index_diag = inspect_index_for_column(&table, "vector", false).await;
        let path = if index_diag.is_some() { "vector_index" } else { "vector_scan" };
        let reason = format!(
            "{}; query_model=clip_vit_b32_text; result_limit={}; result_offset={offset}; \
             max_distance={}",
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
            return Ok((vec![], 0, false));
        }
        let effective_offset = offset.min(candidate_count);
        let max_fetchable = candidate_count.saturating_sub(effective_offset);
        if max_fetchable == 0 {
            log_query_result("search_images_by_text", path, 0, total_started.elapsed().as_millis());
            return Ok((vec![], candidate_count, false));
        }
        let effective_limit = limit.unwrap_or(max_fetchable).min(max_fetchable);
        if effective_limit == 0 {
            log_query_result("search_images_by_text", path, 0, total_started.elapsed().as_millis());
            return Ok((vec![], candidate_count, false));
        }
        let fetch_limit = effective_limit.saturating_add(1);

        let vector_query = table
            .query()
            .nearest_to(query_embedding.as_slice())
            .context("failed to build text-image search query")?;

        let started = Instant::now();
        let mut vector_query = vector_query
            .only_if(filter)
            .offset(effective_offset)
            .limit(fetch_limit);
        if max_distance.is_some() {
            vector_query = vector_query.distance_range(None, max_distance);
        }
        let batches = vector_query
            .select(Select::columns(&["id", "filename", "_distance"]))
            .execute()
            .await?;

        let batch_list = batches.try_collect::<Vec<_>>().await?;
        let mut images = batches_to_images(&batch_list)?;
        let has_more = images.len() > effective_limit;
        if has_more {
            images.truncate(effective_limit);
        }
        log_query_result(
            "search_images_by_text",
            path,
            images.len(),
            started.elapsed().as_millis(),
        );
        Ok((images, candidate_count, has_more))
    }

    pub async fn search_images(
        &self,
        id: &str,
        limit: Option<usize>,
        max_distance: Option<f32>,
    ) -> Result<Vec<ImageInfo>> {
        let (images, _, _) = self.search_images_paged(id, limit, 0, max_distance).await?;
        Ok(images)
    }

    pub async fn search_images_paged(
        &self,
        id: &str,
        limit: Option<usize>,
        offset: usize,
        max_distance: Option<f32>,
    ) -> Result<(Vec<ImageInfo>, usize, bool)> {
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
                return Ok((vec![], 0, false));
            },
        };

        let index_diag = inspect_index_for_column(&table, "vector", false).await;
        let path = if index_diag.is_some() { "vector_index" } else { "vector_scan" };
        let reason = format!(
            "{}; result_limit={}; result_offset={offset}; max_distance={}",
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
            return Ok((vec![], 0, false));
        }
        let effective_offset = offset.min(candidate_count);
        let max_fetchable = candidate_count.saturating_sub(effective_offset);
        if max_fetchable == 0 {
            log_query_result("search_images", path, 0, total_started.elapsed().as_millis());
            return Ok((vec![], candidate_count, false));
        }
        let effective_limit = limit.unwrap_or(max_fetchable).min(max_fetchable);
        if effective_limit == 0 {
            log_query_result("search_images", path, 0, total_started.elapsed().as_millis());
            return Ok((vec![], candidate_count, false));
        }
        let fetch_limit = effective_limit.saturating_add(1);

        let vector_query = table
            .query()
            .nearest_to(vector.as_slice())
            .context("failed to build image search query")?;

        let started = Instant::now();
        let mut vector_query = vector_query
            .only_if(filter)
            .offset(effective_offset)
            .limit(fetch_limit);
        if max_distance.is_some() {
            vector_query = vector_query.distance_range(None, max_distance);
        }
        let batches = vector_query
            .select(Select::columns(&["id", "filename", "_distance"]))
            .execute()
            .await?;

        let batch_list = batches.try_collect::<Vec<_>>().await?;
        let mut images = batches_to_images(&batch_list)?;
        let has_more = images.len() > effective_limit;
        if has_more {
            images.truncate(effective_limit);
        }
        log_query_result("search_images", path, images.len(), started.elapsed().as_millis());
        Ok((images, candidate_count, has_more))
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

async fn fetch_article_raw_markdown(table: &Table, id: &str, lang: &str) -> Result<Option<String>> {
    let column = match lang {
        "zh" => "content",
        "en" => "content_en",
        _ => anyhow::bail!("unsupported article raw markdown language: {lang}"),
    };
    let filter = format!("id = '{}'", escape_literal(id));

    let batches = match table
        .query()
        .only_if(filter)
        .limit(1)
        .select(Select::columns(&[column]))
        .execute()
        .await
    {
        Ok(stream) => stream.try_collect::<Vec<_>>().await?,
        Err(err) => {
            let err_text = err.to_string();
            let missing_content_en_column = lang == "en"
                && err_text.contains("content_en")
                && err_text.contains("missing column");
            if missing_content_en_column {
                tracing::warn!(
                    "Article table appears to be on legacy schema (missing content_en). Falling \
                     back to None for raw en content: {err_text}"
                );
                return Ok(None);
            }
            return Err(err.into());
        },
    };

    let Some(batch) = batches.first() else {
        return Ok(None);
    };
    if batch.num_rows() == 0 {
        return Ok(None);
    }

    if lang == "zh" {
        let content = string_array(batch, "content")
            .map(|array| value_string(array, 0))
            .unwrap_or_default();
        let content = content.trim().to_string();
        return if content.is_empty() { Ok(None) } else { Ok(Some(content)) };
    }

    let content = optional_string_array(batch, "content_en")
        .and_then(|array| value_string_opt(array, 0))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    Ok(content)
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

#[derive(Debug)]
struct SemanticVectorSelection {
    rows: Vec<SearchArticleRow>,
    search_language: TextEmbeddingLanguage,
    query_embedding: Vec<f32>,
    selected_column: &'static str,
    selected_path: &'static str,
}

fn choose_primary_search_language(keyword: &str) -> TextEmbeddingLanguage {
    if is_pure_english_query(keyword) {
        TextEmbeddingLanguage::English
    } else {
        detect_language(keyword)
    }
}

fn is_pure_english_query(keyword: &str) -> bool {
    let mut has_ascii_alpha = false;
    for ch in keyword.chars() {
        if ch.is_ascii_alphabetic() {
            has_ascii_alpha = true;
            continue;
        }
        if ch.is_ascii_digit() || ch.is_ascii_whitespace() || ch.is_ascii_punctuation() {
            continue;
        }
        return false;
    }
    has_ascii_alpha
}

async fn run_semantic_vector_search_with_fallback(
    table: &Table,
    keyword: &str,
    limit: Option<usize>,
    max_distance: Option<f32>,
    enhanced_highlight: bool,
) -> Result<SemanticVectorSelection> {
    let mut search_language = choose_primary_search_language(keyword);
    let mut query_embedding = embed_text_with_language(keyword, search_language);
    let primary_column = vector_column_for_language(search_language);
    let primary_index = inspect_index_for_column(table, primary_column, false).await;
    let primary_path = if primary_index.is_some() { "vector_index" } else { "vector_scan" };
    let primary_reason = index_reason(primary_column, primary_index.as_ref());

    log_query_path(
        "semantic_search.primary",
        primary_path,
        "vector_index",
        &format!(
            "{primary_reason}; language={:?}; result_limit={}; max_distance={}; \
             enhanced_highlight={enhanced_highlight}",
            search_language,
            limit
                .map(|value| value.to_string())
                .unwrap_or_else(|| "none".to_string()),
            max_distance
                .map(|value| value.to_string())
                .unwrap_or_else(|| "none".to_string())
        ),
    );

    let primary_started = Instant::now();
    let mut rows = run_semantic_vector_search(
        table,
        primary_column,
        query_embedding.as_slice(),
        limit,
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
        let fallback_index = inspect_index_for_column(table, fallback_column, false).await;
        let fallback_path = if fallback_index.is_some() { "vector_index" } else { "vector_scan" };
        let fallback_reason = format!(
            "primary_rows=0; primary_language={:?}; fallback_language={:?}; {}",
            search_language,
            fallback_language,
            index_reason(fallback_column, fallback_index.as_ref())
        );
        log_query_path("semantic_search.fallback", fallback_path, "vector_index", &fallback_reason);

        let fallback_embedding = embed_text_with_language(keyword, fallback_language);
        let fallback_started = Instant::now();
        let fallback_rows = run_semantic_vector_search(
            table,
            fallback_column,
            fallback_embedding.as_slice(),
            limit,
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

    Ok(SemanticVectorSelection {
        rows,
        search_language,
        query_embedding,
        selected_column,
        selected_path,
    })
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

#[derive(Debug, Clone)]
struct ArticleViewRecord {
    id: String,
    article_id: String,
    viewed_at: i64,
    day_bucket: String,
    hour_bucket: String,
    client_fingerprint: String,
    created_at: i64,
    updated_at: i64,
}

#[derive(Debug, Clone)]
struct ApiBehaviorRecord {
    event_id: String,
    occurred_at: i64,
    client_source: String,
    method: String,
    path: String,
    query: String,
    page_path: String,
    referrer: Option<String>,
    status_code: i32,
    latency_ms: i32,
    client_ip: String,
    ip_region: String,
    ua_raw: Option<String>,
    device_type: String,
    os_family: String,
    browser_family: String,
    request_id: String,
    trace_id: String,
    created_at: i64,
    updated_at: i64,
}

const SHANGHAI_TIMEZONE: &str = "Asia/Shanghai";

fn shanghai_tz() -> FixedOffset {
    FixedOffset::east_opt(8 * 3600).expect("UTC+8 offset should be valid")
}

fn normalize_daily_window(days: usize, max_days: usize) -> usize {
    let upper = max_days.max(1);
    days.clamp(1, upper)
}

fn article_view_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("article_id", DataType::Utf8, false),
        Field::new("viewed_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
        Field::new("day_bucket", DataType::Utf8, false),
        Field::new("hour_bucket", DataType::Utf8, false),
        Field::new("client_fingerprint", DataType::Utf8, false),
        Field::new("created_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
        Field::new("updated_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
    ]))
}

fn api_behavior_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("event_id", DataType::Utf8, false),
        Field::new("occurred_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
        Field::new("client_source", DataType::Utf8, false),
        Field::new("method", DataType::Utf8, false),
        Field::new("path", DataType::Utf8, false),
        Field::new("query", DataType::Utf8, false),
        Field::new("page_path", DataType::Utf8, false),
        Field::new("referrer", DataType::Utf8, true),
        Field::new("status_code", DataType::Int32, false),
        Field::new("latency_ms", DataType::Int32, false),
        Field::new("client_ip", DataType::Utf8, false),
        Field::new("ip_region", DataType::Utf8, false),
        Field::new("ua_raw", DataType::Utf8, true),
        Field::new("device_type", DataType::Utf8, false),
        Field::new("os_family", DataType::Utf8, false),
        Field::new("browser_family", DataType::Utf8, false),
        Field::new("request_id", DataType::Utf8, false),
        Field::new("trace_id", DataType::Utf8, false),
        Field::new("created_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
        Field::new("updated_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
    ]))
}

fn build_article_view_batch(record: &ArticleViewRecord) -> Result<RecordBatch> {
    let mut id_builder = StringBuilder::new();
    let mut article_id_builder = StringBuilder::new();
    let mut viewed_at_builder = TimestampMillisecondBuilder::new();
    let mut day_bucket_builder = StringBuilder::new();
    let mut hour_bucket_builder = StringBuilder::new();
    let mut client_fingerprint_builder = StringBuilder::new();
    let mut created_at_builder = TimestampMillisecondBuilder::new();
    let mut updated_at_builder = TimestampMillisecondBuilder::new();

    id_builder.append_value(&record.id);
    article_id_builder.append_value(&record.article_id);
    viewed_at_builder.append_value(record.viewed_at);
    day_bucket_builder.append_value(&record.day_bucket);
    hour_bucket_builder.append_value(&record.hour_bucket);
    client_fingerprint_builder.append_value(&record.client_fingerprint);
    created_at_builder.append_value(record.created_at);
    updated_at_builder.append_value(record.updated_at);

    let schema = article_view_schema();
    let arrays: Vec<ArrayRef> = vec![
        Arc::new(id_builder.finish()),
        Arc::new(article_id_builder.finish()),
        Arc::new(viewed_at_builder.finish()),
        Arc::new(day_bucket_builder.finish()),
        Arc::new(hour_bucket_builder.finish()),
        Arc::new(client_fingerprint_builder.finish()),
        Arc::new(created_at_builder.finish()),
        Arc::new(updated_at_builder.finish()),
    ];

    Ok(RecordBatch::try_new(schema, arrays)?)
}

async fn upsert_article_view_record(table: &Table, record: &ArticleViewRecord) -> Result<()> {
    let batch = build_article_view_batch(record)?;
    let schema = batch.schema();
    let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);

    let mut merge = table.merge_insert(&["id"]);
    merge.when_matched_update_all(None);
    merge.when_not_matched_insert_all();
    merge.execute(Box::new(batches)).await?;
    Ok(())
}

fn build_api_behavior_batch_multi(records: &[ApiBehaviorRecord]) -> Result<RecordBatch> {
    let mut event_id_builder = StringBuilder::new();
    let mut occurred_at_builder = TimestampMillisecondBuilder::new();
    let mut client_source_builder = StringBuilder::new();
    let mut method_builder = StringBuilder::new();
    let mut path_builder = StringBuilder::new();
    let mut query_builder = StringBuilder::new();
    let mut page_path_builder = StringBuilder::new();
    let mut referrer_builder = StringBuilder::new();
    let mut status_code_builder = Int32Builder::new();
    let mut latency_ms_builder = Int32Builder::new();
    let mut client_ip_builder = StringBuilder::new();
    let mut ip_region_builder = StringBuilder::new();
    let mut ua_raw_builder = StringBuilder::new();
    let mut device_type_builder = StringBuilder::new();
    let mut os_family_builder = StringBuilder::new();
    let mut browser_family_builder = StringBuilder::new();
    let mut request_id_builder = StringBuilder::new();
    let mut trace_id_builder = StringBuilder::new();
    let mut created_at_builder = TimestampMillisecondBuilder::new();
    let mut updated_at_builder = TimestampMillisecondBuilder::new();

    for record in records {
        event_id_builder.append_value(&record.event_id);
        occurred_at_builder.append_value(record.occurred_at);
        client_source_builder.append_value(&record.client_source);
        method_builder.append_value(&record.method);
        path_builder.append_value(&record.path);
        query_builder.append_value(&record.query);
        page_path_builder.append_value(&record.page_path);
        referrer_builder.append_option(record.referrer.as_deref());
        status_code_builder.append_value(record.status_code);
        latency_ms_builder.append_value(record.latency_ms);
        client_ip_builder.append_value(&record.client_ip);
        ip_region_builder.append_value(&record.ip_region);
        ua_raw_builder.append_option(record.ua_raw.as_deref());
        device_type_builder.append_value(&record.device_type);
        os_family_builder.append_value(&record.os_family);
        browser_family_builder.append_value(&record.browser_family);
        request_id_builder.append_value(&record.request_id);
        trace_id_builder.append_value(&record.trace_id);
        created_at_builder.append_value(record.created_at);
        updated_at_builder.append_value(record.updated_at);
    }

    let schema = api_behavior_schema();
    let arrays: Vec<ArrayRef> = vec![
        Arc::new(event_id_builder.finish()),
        Arc::new(occurred_at_builder.finish()),
        Arc::new(client_source_builder.finish()),
        Arc::new(method_builder.finish()),
        Arc::new(path_builder.finish()),
        Arc::new(query_builder.finish()),
        Arc::new(page_path_builder.finish()),
        Arc::new(referrer_builder.finish()),
        Arc::new(status_code_builder.finish()),
        Arc::new(latency_ms_builder.finish()),
        Arc::new(client_ip_builder.finish()),
        Arc::new(ip_region_builder.finish()),
        Arc::new(ua_raw_builder.finish()),
        Arc::new(device_type_builder.finish()),
        Arc::new(os_family_builder.finish()),
        Arc::new(browser_family_builder.finish()),
        Arc::new(request_id_builder.finish()),
        Arc::new(trace_id_builder.finish()),
        Arc::new(created_at_builder.finish()),
        Arc::new(updated_at_builder.finish()),
    ];
    Ok(RecordBatch::try_new(schema, arrays)?)
}

async fn append_api_behavior_records(table: &Table, records: &[ApiBehaviorRecord]) -> Result<()> {
    if records.is_empty() {
        return Ok(());
    }
    let batch = build_api_behavior_batch_multi(records)?;
    let schema = batch.schema();
    let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
    table
        .add(Box::new(batches))
        .execute()
        .await
        .context("failed to append api behavior records")?;
    Ok(())
}

async fn fetch_article_view_day_counts(
    table: &Table,
    article_id: &str,
    since_day: Option<&str>,
) -> Result<HashMap<String, u32>> {
    let escaped_id = escape_literal(article_id);
    let filter = if let Some(day) = since_day {
        let escaped_day = escape_literal(day);
        format!("article_id = '{escaped_id}' AND day_bucket >= '{escaped_day}'")
    } else {
        format!("article_id = '{escaped_id}'")
    };
    let batches = table
        .query()
        .only_if(filter)
        .select(Select::columns(&["day_bucket"]))
        .execute()
        .await?
        .try_collect::<Vec<_>>()
        .await?;

    let mut counts: HashMap<String, u32> = HashMap::new();
    for batch in batches {
        let day_array = string_array(&batch, "day_bucket")?;
        for idx in 0..batch.num_rows() {
            if day_array.is_null(idx) {
                continue;
            }
            let day = day_array.value(idx).to_string();
            *counts.entry(day).or_insert(0) += 1;
        }
    }
    Ok(counts)
}

async fn fetch_article_view_hour_counts_for_day(
    table: &Table,
    article_id: &str,
    day: &str,
) -> Result<HashMap<String, u32>> {
    let filter = format!(
        "article_id = '{}' AND day_bucket = '{}'",
        escape_literal(article_id),
        escape_literal(day)
    );
    let batches = table
        .query()
        .only_if(filter)
        .select(Select::columns(&["hour_bucket"]))
        .execute()
        .await?
        .try_collect::<Vec<_>>()
        .await?;

    let mut counts: HashMap<String, u32> = HashMap::new();
    for batch in batches {
        let hour_array = string_array(&batch, "hour_bucket")?;
        for idx in 0..batch.num_rows() {
            if hour_array.is_null(idx) {
                continue;
            }
            let bucket = hour_array.value(idx);
            let hour = bucket.rsplit(' ').next().unwrap_or("").trim();
            if hour.len() != 2 || !hour.chars().all(|ch| ch.is_ascii_digit()) {
                continue;
            }
            *counts.entry(hour.to_string()).or_insert(0) += 1;
        }
    }
    Ok(counts)
}

fn build_recent_day_points(
    day_counts: &HashMap<String, u32>,
    end_day: &str,
    days: usize,
) -> Result<Vec<ArticleViewPoint>> {
    let end_date = NaiveDate::parse_from_str(end_day, "%Y-%m-%d")
        .with_context(|| format!("invalid day bucket format: {end_day}"))?;
    let mut points = Vec::with_capacity(days);
    for offset in (0..days).rev() {
        let day = end_date - ChronoDuration::days(offset as i64);
        let key = day.format("%Y-%m-%d").to_string();
        points.push(ArticleViewPoint {
            key: key.clone(),
            views: *day_counts.get(&key).unwrap_or(&0),
        });
    }
    Ok(points)
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

fn batches_to_api_behavior_events(batches: &[RecordBatch]) -> Result<Vec<ApiBehaviorEvent>> {
    let mut events = Vec::new();
    for batch in batches {
        if batch.num_rows() == 0 {
            continue;
        }

        let event_id = string_array(batch, "event_id")?;
        let occurred_at = timestamp_ms_array(batch, "occurred_at")?;
        let client_source = string_array(batch, "client_source")?;
        let method = string_array(batch, "method")?;
        let path = string_array(batch, "path")?;
        let query = string_array(batch, "query")?;
        let page_path = string_array(batch, "page_path")?;
        let referrer = optional_string_array(batch, "referrer");
        let status_code = int32_array(batch, "status_code")?;
        let latency_ms = int32_array(batch, "latency_ms")?;
        let client_ip = string_array(batch, "client_ip")?;
        let ip_region = string_array(batch, "ip_region")?;
        let ua_raw = optional_string_array(batch, "ua_raw");
        let device_type = string_array(batch, "device_type")?;
        let os_family = string_array(batch, "os_family")?;
        let browser_family = string_array(batch, "browser_family")?;
        let request_id = string_array(batch, "request_id")?;
        let trace_id = string_array(batch, "trace_id")?;

        for idx in 0..batch.num_rows() {
            if occurred_at.is_null(idx) {
                continue;
            }
            events.push(ApiBehaviorEvent {
                event_id: value_string(event_id, idx),
                occurred_at: occurred_at.value(idx),
                client_source: value_string(client_source, idx),
                method: value_string(method, idx),
                path: value_string(path, idx),
                query: value_string(query, idx),
                page_path: value_string(page_path, idx),
                referrer: referrer.and_then(|array| value_string_opt(array, idx)),
                status_code: status_code.value(idx),
                latency_ms: latency_ms.value(idx),
                client_ip: value_string(client_ip, idx),
                ip_region: value_string(ip_region, idx),
                ua_raw: ua_raw.and_then(|array| value_string_opt(array, idx)),
                device_type: value_string(device_type, idx),
                os_family: value_string(os_family, idx),
                browser_family: value_string(browser_family, idx),
                request_id: value_string(request_id, idx),
                trace_id: value_string(trace_id, idx),
            });
        }
    }
    Ok(events)
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
        .downcast_ref::<Int32Array>()
        .with_context(|| format!("column {name} is not Int32Array"))
}

fn timestamp_ms_array<'a>(
    batch: &'a RecordBatch,
    name: &str,
) -> Result<&'a TimestampMillisecondArray> {
    column(batch, name)?
        .as_any()
        .downcast_ref::<TimestampMillisecondArray>()
        .with_context(|| format!("column {name} is not TimestampMillisecondArray"))
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

fn normalize_text(value: String, max_chars: usize) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    trimmed.chars().take(max_chars.max(1)).collect::<String>()
}

fn normalize_optional_text(value: Option<String>, max_chars: usize) -> Option<String> {
    value
        .map(|item| normalize_text(item, max_chars))
        .filter(|item| !item.is_empty())
}

fn normalize_required_text(value: String, max_chars: usize, fallback: &str) -> String {
    let normalized = normalize_text(value, max_chars);
    if normalized.is_empty() {
        fallback.to_string()
    } else {
        normalized
    }
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
        alternate_embedding_language, choose_primary_search_language, cosine_similarity,
        extract_highlight, extract_semantic_highlight, find_case_insensitive_match_range,
        is_pure_english_query, semantic_query_tokens, split_text_by_sentence_or_size,
        vector_column_for_language, TextEmbeddingLanguage,
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

    #[test]
    fn pure_english_query_detection_is_strict() {
        assert!(is_pure_english_query("Rust async runtime 101"));
        assert!(is_pure_english_query("vector-en fallback? yes!"));
        assert!(!is_pure_english_query("Rust 异步"));
        assert!(!is_pure_english_query("仅中文"));
        assert!(!is_pure_english_query("12345"));
    }

    #[test]
    fn primary_search_language_prefers_vector_en_for_pure_english() {
        assert_eq!(
            choose_primary_search_language("How to optimize wasm bundle size"),
            TextEmbeddingLanguage::English
        );
        assert_eq!(choose_primary_search_language("Rust 异步编程"), TextEmbeddingLanguage::Chinese);
    }
}
