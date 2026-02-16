use std::{
    collections::HashMap,
    convert::Infallible,
    net::IpAddr,
    time::{Duration, Instant},
};

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        Json, Response,
    },
};
use async_stream::stream;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use static_flow_shared::{
    comments_store::{
        CommentAiRunChunkRecord, CommentAiRunRecord, CommentAuditRecord, CommentDataStore,
        CommentTaskPatch, NewCommentAuditInput, NewCommentTaskInput, PublishedCommentPatch,
        COMMENT_AI_RUN_STATUS_RUNNING, COMMENT_STATUS_APPROVED, COMMENT_STATUS_DONE,
        COMMENT_STATUS_FAILED, COMMENT_STATUS_PENDING, COMMENT_STATUS_REJECTED,
        COMMENT_STATUS_RUNNING,
    },
    lancedb_api::{
        ArticleListResponse, ArticleViewTrackResponse, ArticleViewTrendResponse,
        CategoriesResponse, ImageListResponse, ImageSearchResponse, ImageTextSearchResponse,
        SearchResponse, StatsResponse, TagsResponse,
    },
    Article,
};
use tokio::time::sleep;

use crate::state::{
    AppState, CommentRuntimeConfig, ViewAnalyticsRuntimeConfig,
    MAX_CONFIGURABLE_COMMENT_CLEANUP_RETENTION_DAYS, MAX_CONFIGURABLE_COMMENT_LIST_LIMIT,
    MAX_CONFIGURABLE_COMMENT_RATE_LIMIT_SECONDS, MAX_CONFIGURABLE_VIEW_DEDUPE_WINDOW_SECONDS,
    MAX_CONFIGURABLE_VIEW_TREND_DAYS,
};

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub q: String,
    #[serde(default)]
    pub enhanced_highlight: bool,
    #[serde(default)]
    pub hybrid: bool,
    #[serde(default)]
    pub hybrid_rrf_k: Option<f32>,
    #[serde(default)]
    pub hybrid_vector_limit: Option<usize>,
    #[serde(default)]
    pub hybrid_fts_limit: Option<usize>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub max_distance: Option<f32>,
}

#[derive(Debug, Deserialize)]
pub struct ImageSearchQuery {
    pub id: String,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub max_distance: Option<f32>,
}

#[derive(Debug, Deserialize)]
pub struct ImageTextSearchQuery {
    pub q: String,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub max_distance: Option<f32>,
}

#[derive(Debug, Deserialize)]
pub struct ImageRenderQuery {
    pub thumb: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct ArticleQuery {
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ViewTrendQuery {
    #[serde(default)]
    pub granularity: Option<String>,
    #[serde(default)]
    pub days: Option<usize>,
    #[serde(default)]
    pub day: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
    pub code: u16,
}

#[derive(Debug, Serialize)]
pub struct ViewAnalyticsConfigResponse {
    pub dedupe_window_seconds: u64,
    pub trend_default_days: usize,
    pub trend_max_days: usize,
}

impl From<ViewAnalyticsRuntimeConfig> for ViewAnalyticsConfigResponse {
    fn from(value: ViewAnalyticsRuntimeConfig) -> Self {
        Self {
            dedupe_window_seconds: value.dedupe_window_seconds,
            trend_default_days: value.trend_default_days,
            trend_max_days: value.trend_max_days,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct UpdateViewAnalyticsConfigRequest {
    #[serde(default)]
    pub dedupe_window_seconds: Option<u64>,
    #[serde(default)]
    pub trend_default_days: Option<usize>,
    #[serde(default)]
    pub trend_max_days: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct CommentRuntimeConfigResponse {
    pub submit_rate_limit_seconds: u64,
    pub list_default_limit: usize,
    pub cleanup_retention_days: i64,
}

impl From<CommentRuntimeConfig> for CommentRuntimeConfigResponse {
    fn from(value: CommentRuntimeConfig) -> Self {
        Self {
            submit_rate_limit_seconds: value.submit_rate_limit_seconds,
            list_default_limit: value.list_default_limit,
            cleanup_retention_days: value.cleanup_retention_days,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct UpdateCommentRuntimeConfigRequest {
    #[serde(default)]
    pub submit_rate_limit_seconds: Option<u64>,
    #[serde(default)]
    pub list_default_limit: Option<usize>,
    #[serde(default)]
    pub cleanup_retention_days: Option<i64>,
}

#[derive(Debug, Deserialize, Default)]
pub struct CommentClientMeta {
    #[serde(default)]
    pub ua: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub platform: Option<String>,
    #[serde(default)]
    pub viewport: Option<String>,
    #[serde(default)]
    pub timezone: Option<String>,
    #[serde(default)]
    pub referrer: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SubmitCommentRequest {
    pub article_id: String,
    pub entry_type: String,
    pub comment_text: String,
    #[serde(default)]
    pub selected_text: Option<String>,
    #[serde(default)]
    pub anchor_block_id: Option<String>,
    #[serde(default)]
    pub anchor_context_before: Option<String>,
    #[serde(default)]
    pub anchor_context_after: Option<String>,
    #[serde(default)]
    pub reply_to_comment_id: Option<String>,
    #[serde(default)]
    pub client_meta: Option<CommentClientMeta>,
}

#[derive(Debug, Serialize)]
pub struct SubmitCommentResponse {
    pub task_id: String,
    pub status: String,
}

#[derive(Debug, Deserialize)]
pub struct CommentListQuery {
    pub article_id: String,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct PublicCommentItem {
    pub comment_id: String,
    pub article_id: String,
    pub task_id: String,
    pub author_name: String,
    pub author_avatar_seed: String,
    pub comment_text: String,
    pub selected_text: Option<String>,
    pub anchor_block_id: Option<String>,
    pub anchor_context_before: Option<String>,
    pub anchor_context_after: Option<String>,
    pub reply_to_comment_id: Option<String>,
    pub reply_to_comment_text: Option<String>,
    pub reply_to_ai_reply_markdown: Option<String>,
    pub ai_reply_markdown: Option<String>,
    pub ip_region: String,
    pub published_at: i64,
}

#[derive(Debug, Serialize)]
pub struct CommentListResponse {
    pub comments: Vec<PublicCommentItem>,
    pub total: usize,
    pub article_id: String,
}

#[derive(Debug, Serialize)]
pub struct CommentStatsResponse {
    pub article_id: String,
    pub total: usize,
}

#[derive(Debug, Deserialize)]
pub struct AdminCommentTasksQuery {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct AdminCommentTaskListResponse {
    pub tasks: Vec<static_flow_shared::comments_store::CommentTaskRecord>,
    pub total: usize,
    pub status_counts: HashMap<String, usize>,
}

#[derive(Debug, Serialize)]
pub struct AdminCommentTaskGroup {
    pub article_id: String,
    pub total: usize,
    pub status_counts: HashMap<String, usize>,
    pub tasks: Vec<static_flow_shared::comments_store::CommentTaskRecord>,
}

#[derive(Debug, Serialize)]
pub struct AdminCommentTaskGroupedResponse {
    pub groups: Vec<AdminCommentTaskGroup>,
    pub total_tasks: usize,
    pub total_articles: usize,
    pub status_counts: HashMap<String, usize>,
}

#[derive(Debug, Deserialize)]
pub struct AdminCommentPublishedQuery {
    #[serde(default)]
    pub article_id: Option<String>,
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct AdminCommentPublishedResponse {
    pub comments: Vec<PublicCommentItem>,
    pub total: usize,
}

#[derive(Debug, Deserialize)]
pub struct AdminPatchPublishedCommentRequest {
    #[serde(default)]
    pub ai_reply_markdown: Option<String>,
    #[serde(default)]
    pub comment_text: Option<String>,
    #[serde(default)]
    pub operator: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AdminCommentAuditQuery {
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub action: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct AdminCommentAuditResponse {
    pub logs: Vec<CommentAuditRecord>,
    pub total: usize,
}

#[derive(Debug, Deserialize)]
pub struct AdminCommentAiRunsQuery {
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct AdminCommentAiOutputQuery {
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct AdminCommentAiOutputStreamQuery {
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub from_batch_index: Option<i32>,
    #[serde(default)]
    pub poll_ms: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct AdminCommentAiRunsResponse {
    pub runs: Vec<CommentAiRunRecord>,
    pub total: usize,
}

#[derive(Debug, Serialize)]
pub struct AdminCommentTaskAiOutputResponse {
    pub task_id: String,
    pub selected_run_id: Option<String>,
    pub runs: Vec<CommentAiRunRecord>,
    pub chunks: Vec<CommentAiRunChunkRecord>,
    pub merged_stdout: String,
    pub merged_stderr: String,
    pub merged_output: String,
}

#[derive(Debug, Serialize)]
pub struct AdminCommentAiStreamEvent {
    pub event_type: String,
    pub task_id: String,
    pub run_id: String,
    pub run_status: Option<String>,
    pub chunk: Option<CommentAiRunChunkRecord>,
}

#[derive(Debug, Deserialize)]
pub struct AdminPatchCommentTaskRequest {
    #[serde(default)]
    pub comment_text: Option<String>,
    #[serde(default)]
    pub selected_text: Option<String>,
    #[serde(default)]
    pub anchor_block_id: Option<String>,
    #[serde(default)]
    pub anchor_context_before: Option<String>,
    #[serde(default)]
    pub anchor_context_after: Option<String>,
    #[serde(default)]
    pub admin_note: Option<String>,
    #[serde(default)]
    pub operator: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AdminTaskActionRequest {
    #[serde(default)]
    pub operator: Option<String>,
    #[serde(default)]
    pub admin_note: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AdminCleanupRequest {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub retention_days: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct AdminCleanupResponse {
    pub deleted_tasks: usize,
    pub before_ms: Option<i64>,
}

const CACHE_TTL: Duration = Duration::from_secs(60);

pub async fn list_articles(
    State(state): State<AppState>,
    Query(query): Query<ArticleQuery>,
) -> Result<Json<ArticleListResponse>, (StatusCode, Json<ErrorResponse>)> {
    let articles = state
        .store
        .list_articles(query.tag.as_deref(), query.category.as_deref())
        .await
        .map_err(|e| internal_error("Failed to fetch articles", e))?;

    Ok(Json(ArticleListResponse {
        total: articles.len(),
        articles,
    }))
}

pub async fn get_article(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Article>, (StatusCode, Json<ErrorResponse>)> {
    let article = state
        .store
        .get_article(&id)
        .await
        .map_err(|e| internal_error("Failed to fetch article", e))?;

    match article {
        Some(article) => Ok(Json(article)),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Article not found".to_string(),
                code: 404,
            }),
        )),
    }
}

pub async fn track_article_view(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<ArticleViewTrackResponse>, (StatusCode, Json<ErrorResponse>)> {
    ensure_article_exists(&state, &id).await?;

    let config = state.view_analytics_config.read().await.clone();
    let fingerprint = build_client_fingerprint(&headers);
    let tracked = state
        .store
        .track_article_view(
            &id,
            &fingerprint,
            config.trend_default_days,
            config.dedupe_window_seconds,
            config.trend_max_days,
        )
        .await
        .map_err(|e| internal_error("Failed to track article view", e))?;

    Ok(Json(tracked))
}

pub async fn get_article_view_trend(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<ViewTrendQuery>,
) -> Result<Json<ArticleViewTrendResponse>, (StatusCode, Json<ErrorResponse>)> {
    ensure_article_exists(&state, &id).await?;
    let config = state.view_analytics_config.read().await.clone();

    let granularity = query
        .granularity
        .as_deref()
        .unwrap_or("day")
        .trim()
        .to_ascii_lowercase();

    match granularity.as_str() {
        "day" => {
            let response = state
                .store
                .fetch_article_view_trend_day(
                    &id,
                    query.days.unwrap_or(config.trend_default_days),
                    config.trend_max_days,
                )
                .await
                .map_err(|e| internal_error("Failed to fetch article view trend", e))?;
            Ok(Json(response))
        },
        "hour" => {
            let day = query.day.as_deref().map(str::trim).unwrap_or_default();
            if day.is_empty() {
                return Err(bad_request("`day` is required for hour granularity"));
            }
            if !is_valid_day_format(day) {
                return Err(bad_request("`day` must use YYYY-MM-DD format"));
            }

            let response = state
                .store
                .fetch_article_view_trend_hour(&id, day)
                .await
                .map_err(|e| internal_error("Failed to fetch article view trend", e))?;
            Ok(Json(response))
        },
        _ => Err(bad_request("`granularity` must be `day` or `hour`")),
    }
}

pub async fn get_view_analytics_config(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ViewAnalyticsConfigResponse>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let config = state.view_analytics_config.read().await.clone();
    Ok(Json(config.into()))
}

pub async fn update_view_analytics_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<UpdateViewAnalyticsConfigRequest>,
) -> Result<Json<ViewAnalyticsConfigResponse>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let current = state.view_analytics_config.read().await.clone();
    let next = apply_view_analytics_config_update(current, request)?;
    {
        let mut writer = state.view_analytics_config.write().await;
        *writer = next.clone();
    }
    Ok(Json(next.into()))
}

pub async fn get_comment_runtime_config(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<CommentRuntimeConfigResponse>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let config = state.comment_runtime_config.read().await.clone();
    Ok(Json(config.into()))
}

pub async fn update_comment_runtime_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<UpdateCommentRuntimeConfigRequest>,
) -> Result<Json<CommentRuntimeConfigResponse>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;

    let current = state.comment_runtime_config.read().await.clone();
    let next = apply_comment_runtime_config_update(current, request)?;
    {
        let mut writer = state.comment_runtime_config.write().await;
        *writer = next.clone();
    }

    Ok(Json(next.into()))
}

pub async fn submit_comment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SubmitCommentRequest>,
) -> Result<Json<SubmitCommentResponse>, (StatusCode, Json<ErrorResponse>)> {
    let article_id = request.article_id.trim();
    if article_id.is_empty() {
        return Err(bad_request("`article_id` is required"));
    }
    ensure_article_exists(&state, article_id).await?;

    let entry_type = request.entry_type.trim().to_ascii_lowercase();
    if entry_type != "selection" && entry_type != "footer" {
        return Err(bad_request("`entry_type` must be `selection` or `footer`"));
    }

    let comment_text = request.comment_text.trim();
    if comment_text.is_empty() {
        return Err(bad_request("`comment_text` is required"));
    }
    if comment_text.chars().count() > 5000 {
        return Err(bad_request("`comment_text` must be <= 5000 chars"));
    }

    let reply_context = resolve_reply_context(
        state.comment_store.as_ref(),
        article_id,
        request.reply_to_comment_id.as_deref(),
    )
    .await?;

    let ip = extract_client_ip(&headers);
    let fingerprint = build_client_fingerprint(&headers);
    let now_ms = chrono::Utc::now().timestamp_millis();
    let runtime_config = state.comment_runtime_config.read().await.clone();
    enforce_comment_submit_rate_limit(
        state.comment_submit_guard.as_ref(),
        &fingerprint,
        now_ms,
        runtime_config.submit_rate_limit_seconds,
    )
    .await?;

    let ip_region = state.geoip.resolve_region(&ip).await;
    let client_meta = request.client_meta.unwrap_or_default();
    let user_agent = client_meta.ua.or_else(|| {
        headers
            .get(header::USER_AGENT)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    });
    let task_id = generate_task_id("cmt");
    let task = state
        .comment_store
        .create_comment_task(NewCommentTaskInput {
            task_id: task_id.clone(),
            article_id: article_id.to_string(),
            entry_type,
            comment_text: comment_text.to_string(),
            selected_text: request.selected_text,
            anchor_block_id: request.anchor_block_id,
            anchor_context_before: request.anchor_context_before,
            anchor_context_after: request.anchor_context_after,
            reply_to_comment_id: reply_context.reply_to_comment_id,
            reply_to_comment_text: reply_context.reply_to_comment_text,
            reply_to_ai_reply_markdown: reply_context.reply_to_ai_reply_markdown,
            client_ip: ip,
            ip_region,
            fingerprint,
            ua: user_agent,
            language: client_meta.language,
            platform: client_meta.platform,
            timezone: client_meta.timezone,
            viewport: client_meta.viewport,
            referrer: client_meta.referrer,
        })
        .await
        .map_err(|e| internal_error("Failed to create comment task", e))?;

    let _ = state
        .comment_store
        .append_audit_log(NewCommentAuditInput {
            log_id: generate_task_id("audit"),
            task_id: task.task_id.clone(),
            action: "created".to_string(),
            operator: "system".to_string(),
            before_json: None,
            after_json: serde_json::to_string(&task).ok(),
        })
        .await;

    Ok(Json(SubmitCommentResponse {
        task_id,
        status: COMMENT_STATUS_PENDING.to_string(),
    }))
}

pub async fn list_comments(
    State(state): State<AppState>,
    Query(query): Query<CommentListQuery>,
) -> Result<Json<CommentListResponse>, (StatusCode, Json<ErrorResponse>)> {
    let article_id = query.article_id.trim();
    if article_id.is_empty() {
        return Err(bad_request("`article_id` is required"));
    }

    let runtime = state.comment_runtime_config.read().await.clone();
    let limit = normalize_comment_list_limit(query.limit, runtime.list_default_limit);
    let tasks = state
        .comment_store
        .list_comment_tasks_by_article(article_id, limit)
        .await
        .map_err(|e| internal_error("Failed to fetch comments", e))?;
    let published_rows = state
        .comment_store
        .list_published_comments(Some(article_id), limit.saturating_mul(3).max(limit))
        .await
        .map_err(|e| internal_error("Failed to fetch published comments", e))?;

    let mut published_by_task = HashMap::new();
    for row in published_rows {
        published_by_task.insert(row.task_id.clone(), row);
    }
    let comments = tasks
        .into_iter()
        .filter(|task| task.status != COMMENT_STATUS_REJECTED)
        .map(|task| {
            let published = published_by_task.remove(&task.task_id);
            public_comment_from_task(task, published)
        })
        .collect::<Vec<_>>();
    let total = comments.len();

    Ok(Json(CommentListResponse {
        comments,
        total,
        article_id: article_id.to_string(),
    }))
}

pub async fn get_comment_stats(
    State(state): State<AppState>,
    Query(query): Query<CommentListQuery>,
) -> Result<Json<CommentStatsResponse>, (StatusCode, Json<ErrorResponse>)> {
    let article_id = query.article_id.trim();
    if article_id.is_empty() {
        return Err(bad_request("`article_id` is required"));
    }

    let total = state
        .comment_store
        .count_comment_tasks_by_article(article_id, &[COMMENT_STATUS_REJECTED])
        .await
        .map_err(|e| internal_error("Failed to count comments", e))?;

    Ok(Json(CommentStatsResponse {
        article_id: article_id.to_string(),
        total,
    }))
}

pub async fn admin_list_comment_tasks(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<AdminCommentTasksQuery>,
) -> Result<Json<AdminCommentTaskListResponse>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;

    let runtime = state.comment_runtime_config.read().await.clone();
    let limit = normalize_comment_list_limit(query.limit, runtime.list_default_limit);
    let status_filter = query
        .status
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let tasks = state
        .comment_store
        .list_comment_tasks(status_filter, limit)
        .await
        .map_err(|e| internal_error("Failed to list comment tasks", e))?;
    let status_counts = state
        .comment_store
        .status_breakdown()
        .await
        .map_err(|e| internal_error("Failed to summarize comment statuses", e))?;

    let total = if let Some(status) = status_filter {
        status_counts.get(status).copied().unwrap_or(0)
    } else {
        status_counts.values().sum()
    };

    Ok(Json(AdminCommentTaskListResponse {
        tasks,
        total,
        status_counts,
    }))
}

pub async fn admin_list_comment_tasks_grouped(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<AdminCommentTasksQuery>,
) -> Result<Json<AdminCommentTaskGroupedResponse>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;

    let runtime = state.comment_runtime_config.read().await.clone();
    let limit = normalize_comment_list_limit(query.limit, runtime.list_default_limit);
    let status_filter = query
        .status
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let tasks = state
        .comment_store
        .list_comment_tasks(status_filter, limit)
        .await
        .map_err(|e| internal_error("Failed to list comment tasks", e))?;
    let status_counts = state
        .comment_store
        .status_breakdown()
        .await
        .map_err(|e| internal_error("Failed to summarize comment statuses", e))?;

    let mut by_article: HashMap<
        String,
        Vec<static_flow_shared::comments_store::CommentTaskRecord>,
    > = HashMap::new();
    for task in tasks {
        by_article
            .entry(task.article_id.clone())
            .or_default()
            .push(task);
    }

    let mut groups = by_article
        .into_iter()
        .map(|(article_id, mut tasks)| {
            tasks.sort_by(|left, right| right.created_at.cmp(&left.created_at));
            let mut counts = HashMap::new();
            for task in &tasks {
                *counts.entry(task.status.clone()).or_insert(0) += 1;
            }
            AdminCommentTaskGroup {
                article_id,
                total: tasks.len(),
                status_counts: counts,
                tasks,
            }
        })
        .collect::<Vec<_>>();
    groups.sort_by(|left, right| left.article_id.cmp(&right.article_id));

    let total_tasks = groups.iter().map(|group| group.total).sum::<usize>();
    let total_articles = groups.len();

    Ok(Json(AdminCommentTaskGroupedResponse {
        groups,
        total_tasks,
        total_articles,
        status_counts,
    }))
}

pub async fn admin_get_comment_task(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(task_id): Path<String>,
) -> Result<
    Json<static_flow_shared::comments_store::CommentTaskRecord>,
    (StatusCode, Json<ErrorResponse>),
> {
    ensure_admin_access(&state, &headers)?;
    let task = state
        .comment_store
        .get_comment_task(&task_id)
        .await
        .map_err(|e| internal_error("Failed to fetch comment task", e))?;

    match task {
        Some(task) => Ok(Json(task)),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Comment task not found".to_string(),
                code: 404,
            }),
        )),
    }
}

pub async fn admin_patch_comment_task(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(task_id): Path<String>,
    Json(request): Json<AdminPatchCommentTaskRequest>,
) -> Result<
    Json<static_flow_shared::comments_store::CommentTaskRecord>,
    (StatusCode, Json<ErrorResponse>),
> {
    ensure_admin_access(&state, &headers)?;

    let before = state
        .comment_store
        .get_comment_task(&task_id)
        .await
        .map_err(|e| internal_error("Failed to fetch comment task", e))?;
    let Some(before_task) = before else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Comment task not found".to_string(),
                code: 404,
            }),
        ));
    };

    let updated = state
        .comment_store
        .patch_comment_task(&task_id, CommentTaskPatch {
            comment_text: request.comment_text,
            selected_text: request.selected_text,
            anchor_block_id: request.anchor_block_id,
            anchor_context_before: request.anchor_context_before,
            anchor_context_after: request.anchor_context_after,
            admin_note: request.admin_note.clone(),
        })
        .await
        .map_err(|e| internal_error("Failed to patch comment task", e))?;
    let Some(task) = updated else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Comment task not found".to_string(),
                code: 404,
            }),
        ));
    };

    let operator = request.operator.unwrap_or_else(|| "admin".to_string());
    let _ = state
        .comment_store
        .append_audit_log(NewCommentAuditInput {
            log_id: generate_task_id("audit"),
            task_id: task_id.clone(),
            action: "patched".to_string(),
            operator,
            before_json: serde_json::to_string(&before_task).ok(),
            after_json: serde_json::to_string(&task).ok(),
        })
        .await;

    Ok(Json(task))
}

pub async fn admin_approve_comment_task(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(task_id): Path<String>,
    Json(request): Json<AdminTaskActionRequest>,
) -> Result<
    Json<static_flow_shared::comments_store::CommentTaskRecord>,
    (StatusCode, Json<ErrorResponse>),
> {
    ensure_admin_access(&state, &headers)?;

    let before = state
        .comment_store
        .get_comment_task(&task_id)
        .await
        .map_err(|e| internal_error("Failed to fetch comment task", e))?;
    let Some(before_task) = before else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Comment task not found".to_string(),
                code: 404,
            }),
        ));
    };

    let task = state
        .comment_store
        .transition_comment_task(
            &task_id,
            COMMENT_STATUS_APPROVED,
            request.admin_note.clone(),
            None,
            false,
        )
        .await
        .map_err(|e| map_comment_action_error("Failed to approve comment task", e))?;
    let Some(task) = task else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Comment task not found".to_string(),
                code: 404,
            }),
        ));
    };

    let operator = request.operator.unwrap_or_else(|| "admin".to_string());
    let _ = state
        .comment_store
        .append_audit_log(NewCommentAuditInput {
            log_id: generate_task_id("audit"),
            task_id: task_id.clone(),
            action: "approved".to_string(),
            operator,
            before_json: serde_json::to_string(&before_task).ok(),
            after_json: serde_json::to_string(&task).ok(),
        })
        .await;

    Ok(Json(task))
}

pub async fn admin_approve_and_run_comment_task(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(task_id): Path<String>,
    Json(request): Json<AdminTaskActionRequest>,
) -> Result<
    Json<static_flow_shared::comments_store::CommentTaskRecord>,
    (StatusCode, Json<ErrorResponse>),
> {
    ensure_admin_access(&state, &headers)?;

    let before = state
        .comment_store
        .get_comment_task(&task_id)
        .await
        .map_err(|e| internal_error("Failed to fetch comment task", e))?;
    let Some(before_task) = before else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Comment task not found".to_string(),
                code: 404,
            }),
        ));
    };

    if before_task.status == COMMENT_STATUS_RUNNING {
        return Err(conflict_error("Comment task is already running"));
    }
    if before_task.status == COMMENT_STATUS_DONE || before_task.status == COMMENT_STATUS_REJECTED {
        return Err(conflict_error("Comment task is finalized and cannot be processed"));
    }

    let transitioned = state
        .comment_store
        .transition_comment_task(
            &task_id,
            COMMENT_STATUS_RUNNING,
            request.admin_note.clone(),
            None,
            true,
        )
        .await
        .map_err(|e| map_comment_action_error("Failed to claim comment task for AI run", e))?;
    let task = transitioned.ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Comment task not found".to_string(),
                code: 404,
            }),
        )
    })?;

    if let Err(err) = state.comment_worker_tx.send(task_id.clone()).await {
        let reason = format!("failed to enqueue comment worker task: {err}");
        let _ = state
            .comment_store
            .transition_comment_task(&task_id, COMMENT_STATUS_FAILED, None, Some(reason), false)
            .await;
        return Err(internal_error("Failed to enqueue comment worker task", err));
    }

    let operator = request.operator.unwrap_or_else(|| "admin".to_string());
    let _ = state
        .comment_store
        .append_audit_log(NewCommentAuditInput {
            log_id: generate_task_id("audit"),
            task_id: task_id.clone(),
            action: "approved_and_run".to_string(),
            operator,
            before_json: serde_json::to_string(&before_task).ok(),
            after_json: serde_json::to_string(&task).ok(),
        })
        .await;

    Ok(Json(task))
}

pub async fn admin_reject_comment_task(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(task_id): Path<String>,
    Json(request): Json<AdminTaskActionRequest>,
) -> Result<
    Json<static_flow_shared::comments_store::CommentTaskRecord>,
    (StatusCode, Json<ErrorResponse>),
> {
    ensure_admin_access(&state, &headers)?;

    let before = state
        .comment_store
        .get_comment_task(&task_id)
        .await
        .map_err(|e| internal_error("Failed to fetch comment task", e))?;
    let Some(before_task) = before else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Comment task not found".to_string(),
                code: 404,
            }),
        ));
    };

    let task = state
        .comment_store
        .transition_comment_task(
            &task_id,
            COMMENT_STATUS_REJECTED,
            request.admin_note.clone(),
            None,
            false,
        )
        .await
        .map_err(|e| map_comment_action_error("Failed to reject comment task", e))?;
    let Some(task) = task else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Comment task not found".to_string(),
                code: 404,
            }),
        ));
    };

    let operator = request.operator.unwrap_or_else(|| "admin".to_string());
    let _ = state
        .comment_store
        .append_audit_log(NewCommentAuditInput {
            log_id: generate_task_id("audit"),
            task_id: task_id.clone(),
            action: "rejected".to_string(),
            operator,
            before_json: serde_json::to_string(&before_task).ok(),
            after_json: serde_json::to_string(&task).ok(),
        })
        .await;

    Ok(Json(task))
}

pub async fn admin_retry_comment_task(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(task_id): Path<String>,
    Json(request): Json<AdminTaskActionRequest>,
) -> Result<
    Json<static_flow_shared::comments_store::CommentTaskRecord>,
    (StatusCode, Json<ErrorResponse>),
> {
    ensure_admin_access(&state, &headers)?;

    let before = state
        .comment_store
        .get_comment_task(&task_id)
        .await
        .map_err(|e| internal_error("Failed to fetch comment task", e))?;
    let Some(before_task) = before else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Comment task not found".to_string(),
                code: 404,
            }),
        ));
    };
    if before_task.status != COMMENT_STATUS_FAILED {
        return Err(conflict_error("Only failed comment tasks can be retried"));
    }

    let task = state
        .comment_store
        .transition_comment_task(
            &task_id,
            COMMENT_STATUS_RUNNING,
            request.admin_note.clone(),
            None,
            true,
        )
        .await
        .map_err(|e| map_comment_action_error("Failed to retry comment task", e))?;
    let Some(task) = task else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Comment task not found".to_string(),
                code: 404,
            }),
        ));
    };

    if let Err(err) = state.comment_worker_tx.send(task_id.clone()).await {
        let reason = format!("failed to enqueue retry task: {err}");
        let _ = state
            .comment_store
            .transition_comment_task(&task_id, COMMENT_STATUS_FAILED, None, Some(reason), false)
            .await;
        return Err(internal_error("Failed to enqueue retry task", err));
    }

    let operator = request.operator.unwrap_or_else(|| "admin".to_string());
    let _ = state
        .comment_store
        .append_audit_log(NewCommentAuditInput {
            log_id: generate_task_id("audit"),
            task_id: task_id.clone(),
            action: "retried".to_string(),
            operator,
            before_json: serde_json::to_string(&before_task).ok(),
            after_json: serde_json::to_string(&task).ok(),
        })
        .await;

    Ok(Json(task))
}

pub async fn admin_list_published_comments(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<AdminCommentPublishedQuery>,
) -> Result<Json<AdminCommentPublishedResponse>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;

    let runtime = state.comment_runtime_config.read().await.clone();
    let limit = normalize_comment_list_limit(query.limit, runtime.list_default_limit);
    let mut rows = state
        .comment_store
        .list_published_comments(query.article_id.as_deref(), limit)
        .await
        .map_err(|e| internal_error("Failed to list published comments", e))?;
    if let Some(task_id) = query
        .task_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        rows.retain(|row| row.task_id == task_id);
    }

    Ok(Json(AdminCommentPublishedResponse {
        total: rows.len(),
        comments: rows
            .into_iter()
            .map(|row| public_comment_from_published(row, None))
            .collect(),
    }))
}

pub async fn admin_patch_published_comment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(comment_id): Path<String>,
    Json(request): Json<AdminPatchPublishedCommentRequest>,
) -> Result<Json<PublicCommentItem>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;

    let before = state
        .comment_store
        .get_published_comment_by_comment_id(&comment_id)
        .await
        .map_err(|e| internal_error("Failed to fetch published comment", e))?;
    let Some(before_record) = before else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Published comment not found".to_string(),
                code: 404,
            }),
        ));
    };

    let patched = state
        .comment_store
        .patch_published_comment(&comment_id, PublishedCommentPatch {
            ai_reply_markdown: request.ai_reply_markdown,
            comment_text: request.comment_text,
        })
        .await
        .map_err(|e| internal_error("Failed to patch published comment", e))?;
    let Some(after_record) = patched else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Published comment not found".to_string(),
                code: 404,
            }),
        ));
    };

    let operator = request.operator.unwrap_or_else(|| "admin".to_string());
    let _ = state
        .comment_store
        .append_audit_log(NewCommentAuditInput {
            log_id: generate_task_id("audit"),
            task_id: after_record.task_id.clone(),
            action: "published_patched".to_string(),
            operator,
            before_json: serde_json::to_string(&before_record).ok(),
            after_json: serde_json::to_string(&after_record).ok(),
        })
        .await;

    Ok(Json(public_comment_from_published(after_record, None)))
}

pub async fn admin_delete_published_comment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(comment_id): Path<String>,
    Json(request): Json<AdminTaskActionRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;

    let before = state
        .comment_store
        .get_published_comment_by_comment_id(&comment_id)
        .await
        .map_err(|e| internal_error("Failed to fetch published comment", e))?;
    let Some(before_record) = before else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Published comment not found".to_string(),
                code: 404,
            }),
        ));
    };

    state
        .comment_store
        .delete_published_comment(&comment_id)
        .await
        .map_err(|e| internal_error("Failed to delete published comment", e))?;

    let operator = request.operator.unwrap_or_else(|| "admin".to_string());
    let _ = state
        .comment_store
        .append_audit_log(NewCommentAuditInput {
            log_id: generate_task_id("audit"),
            task_id: before_record.task_id.clone(),
            action: "published_deleted".to_string(),
            operator,
            before_json: serde_json::to_string(&before_record).ok(),
            after_json: Some("{\"deleted\":true}".to_string()),
        })
        .await;

    Ok(Json(serde_json::json!({
        "comment_id": comment_id,
        "deleted": true
    })))
}

pub async fn admin_delete_comment_task(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(task_id): Path<String>,
    Json(request): Json<AdminTaskActionRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;

    let before = state
        .comment_store
        .get_comment_task(&task_id)
        .await
        .map_err(|e| internal_error("Failed to fetch comment task", e))?;
    let Some(before_task) = before else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Comment task not found".to_string(),
                code: 404,
            }),
        ));
    };
    if before_task.status == COMMENT_STATUS_RUNNING {
        return Err(conflict_error("Running comment task cannot be deleted"));
    }

    state
        .comment_store
        .delete_comment_task(&task_id)
        .await
        .map_err(|e| internal_error("Failed to delete comment task", e))?;

    let operator = request.operator.unwrap_or_else(|| "admin".to_string());
    let _ = state
        .comment_store
        .append_audit_log(NewCommentAuditInput {
            log_id: generate_task_id("audit"),
            task_id: task_id.clone(),
            action: "task_deleted".to_string(),
            operator,
            before_json: serde_json::to_string(&before_task).ok(),
            after_json: Some("{\"deleted\":true}".to_string()),
        })
        .await;

    Ok(Json(serde_json::json!({
        "task_id": task_id,
        "deleted": true
    })))
}

pub async fn admin_list_comment_audit_logs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<AdminCommentAuditQuery>,
) -> Result<Json<AdminCommentAuditResponse>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;

    let runtime = state.comment_runtime_config.read().await.clone();
    let limit = normalize_comment_list_limit(query.limit, runtime.list_default_limit);
    let logs = state
        .comment_store
        .list_audit_logs(query.task_id.as_deref(), query.action.as_deref(), limit)
        .await
        .map_err(|e| internal_error("Failed to list comment audit logs", e))?;

    Ok(Json(AdminCommentAuditResponse {
        total: logs.len(),
        logs,
    }))
}

pub async fn admin_list_comment_ai_runs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<AdminCommentAiRunsQuery>,
) -> Result<Json<AdminCommentAiRunsResponse>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;

    let runtime = state.comment_runtime_config.read().await.clone();
    let limit = normalize_comment_list_limit(query.limit, runtime.list_default_limit);
    let runs = state
        .comment_store
        .list_ai_runs(query.task_id.as_deref(), query.status.as_deref(), limit)
        .await
        .map_err(|e| internal_error("Failed to list comment AI runs", e))?;

    Ok(Json(AdminCommentAiRunsResponse {
        total: runs.len(),
        runs,
    }))
}

pub async fn admin_get_comment_task_ai_output(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(task_id): Path<String>,
    Query(query): Query<AdminCommentAiOutputQuery>,
) -> Result<Json<AdminCommentTaskAiOutputResponse>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;

    let task = state
        .comment_store
        .get_comment_task(&task_id)
        .await
        .map_err(|e| internal_error("Failed to fetch comment task", e))?;
    if task.is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Comment task not found".to_string(),
                code: 404,
            }),
        ));
    }

    let runtime = state.comment_runtime_config.read().await.clone();
    let run_limit = normalize_comment_list_limit(Some(120), runtime.list_default_limit);
    let runs = state
        .comment_store
        .list_ai_runs(Some(&task_id), None, run_limit)
        .await
        .map_err(|e| internal_error("Failed to list comment AI runs", e))?;

    let selected_run_id = query
        .run_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| runs.first().map(|run| run.run_id.clone()));

    let chunk_limit = normalize_comment_list_limit(query.limit, runtime.list_default_limit)
        .saturating_mul(30)
        .max(300)
        .min(5000);
    let chunks = if let Some(run_id) = selected_run_id.as_deref() {
        state
            .comment_store
            .list_ai_run_chunks(run_id, chunk_limit)
            .await
            .map_err(|e| internal_error("Failed to list comment AI output chunks", e))?
    } else {
        vec![]
    };
    let (merged_stdout, merged_stderr, merged_output) = merge_ai_output_chunks(&chunks);

    Ok(Json(AdminCommentTaskAiOutputResponse {
        task_id,
        selected_run_id,
        runs,
        chunks,
        merged_stdout,
        merged_stderr,
        merged_output,
    }))
}

pub async fn admin_stream_comment_task_ai_output(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(task_id): Path<String>,
    Query(query): Query<AdminCommentAiOutputStreamQuery>,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>, (StatusCode, Json<ErrorResponse>)>
{
    ensure_admin_access(&state, &headers)?;

    let task_exists = state
        .comment_store
        .get_comment_task(&task_id)
        .await
        .map_err(|e| internal_error("Failed to fetch comment task", e))?
        .is_some();
    if !task_exists {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Comment task not found".to_string(),
                code: 404,
            }),
        ));
    }

    let runtime = state.comment_runtime_config.read().await.clone();
    let runs = state
        .comment_store
        .list_ai_runs(
            Some(&task_id),
            None,
            normalize_comment_list_limit(Some(120), runtime.list_default_limit),
        )
        .await
        .map_err(|e| internal_error("Failed to list comment AI runs", e))?;
    let selected_run_id = query
        .run_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| runs.first().map(|run| run.run_id.clone()));
    let Some(run_id) = selected_run_id else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "No AI run found for this task".to_string(),
                code: 404,
            }),
        ));
    };

    let mut cursor = query.from_batch_index.unwrap_or(-1);
    let poll_ms = query.poll_ms.unwrap_or(500).clamp(200, 5_000);
    let poll_interval = Duration::from_millis(poll_ms);
    let store = state.comment_store.clone();
    let task_id_for_stream = task_id.clone();
    let run_id_for_stream = run_id.clone();
    let stream = stream! {
        loop {
            let chunks_result = store.list_ai_run_chunks(&run_id_for_stream, 5000).await;
            match chunks_result {
                Ok(chunks) => {
                    for chunk in chunks {
                        if chunk.batch_index <= cursor {
                            continue;
                        }
                        cursor = chunk.batch_index;
                        let payload = AdminCommentAiStreamEvent {
                            event_type: "chunk".to_string(),
                            task_id: task_id_for_stream.clone(),
                            run_id: run_id_for_stream.clone(),
                            run_status: None,
                            chunk: Some(chunk),
                        };
                        if let Ok(data) = serde_json::to_string(&payload) {
                            yield Ok(Event::default().data(data));
                        }
                    }
                },
                Err(err) => {
                    let payload = AdminCommentAiStreamEvent {
                        event_type: "error".to_string(),
                        task_id: task_id_for_stream.clone(),
                        run_id: run_id_for_stream.clone(),
                        run_status: None,
                        chunk: None,
                    };
                    if let Ok(data) = serde_json::to_string(&payload) {
                        yield Ok(Event::default().data(data));
                    }
                    tracing::error!("failed to stream ai chunks task_id={} run_id={}: {}", task_id_for_stream, run_id_for_stream, err);
                    break;
                },
            }

            let run_result = store.get_ai_run(&run_id_for_stream).await;
            match run_result {
                Ok(Some(run)) => {
                    if run.status != COMMENT_AI_RUN_STATUS_RUNNING {
                        let payload = AdminCommentAiStreamEvent {
                            event_type: "done".to_string(),
                            task_id: task_id_for_stream.clone(),
                            run_id: run_id_for_stream.clone(),
                            run_status: Some(run.status),
                            chunk: None,
                        };
                        if let Ok(data) = serde_json::to_string(&payload) {
                            yield Ok(Event::default().data(data));
                        }
                        break;
                    }
                },
                Ok(None) => {
                    let payload = AdminCommentAiStreamEvent {
                        event_type: "done".to_string(),
                        task_id: task_id_for_stream.clone(),
                        run_id: run_id_for_stream.clone(),
                        run_status: Some("missing".to_string()),
                        chunk: None,
                    };
                    if let Ok(data) = serde_json::to_string(&payload) {
                        yield Ok(Event::default().data(data));
                    }
                    break;
                },
                Err(err) => {
                    tracing::error!("failed to poll ai run task_id={} run_id={}: {}", task_id_for_stream, run_id_for_stream, err);
                    break;
                },
            }

            sleep(poll_interval).await;
        }
    };

    Ok(
        Sse::new(stream)
            .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)).text("keepalive")),
    )
}

pub async fn admin_cleanup_comments(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<AdminCleanupRequest>,
) -> Result<Json<AdminCleanupResponse>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;

    let runtime = state.comment_runtime_config.read().await.clone();
    let retention_days = request
        .retention_days
        .unwrap_or(runtime.cleanup_retention_days);
    if retention_days != -1
        && (retention_days <= 0 || retention_days > MAX_CONFIGURABLE_COMMENT_CLEANUP_RETENTION_DAYS)
    {
        return Err(bad_request("`retention_days` must be -1 or within 1..3650"));
    }

    let before_ms = if retention_days > 0 {
        let now_ms = chrono::Utc::now().timestamp_millis();
        Some(now_ms - retention_days * 24 * 60 * 60 * 1000)
    } else {
        None
    };
    if before_ms.is_none()
        && request
            .status
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_none()
    {
        return Err(bad_request("cleanup requires `status` or positive `retention_days`"));
    }

    let deleted = state
        .comment_store
        .cleanup_comment_tasks(request.status.as_deref(), before_ms)
        .await
        .map_err(|e| internal_error("Failed to cleanup comment tasks", e))?;

    Ok(Json(AdminCleanupResponse {
        deleted_tasks: deleted,
        before_ms,
    }))
}

pub async fn list_tags(
    State(state): State<AppState>,
) -> Result<Json<TagsResponse>, (StatusCode, Json<ErrorResponse>)> {
    if let Some(tags) = read_cache(state.tags_cache.as_ref()).await {
        return Ok(Json(TagsResponse {
            tags,
        }));
    }

    let tags = state
        .store
        .list_tags()
        .await
        .map_err(|e| internal_error("Failed to fetch tags", e))?;

    write_cache(state.tags_cache.as_ref(), tags.clone()).await;
    Ok(Json(TagsResponse {
        tags,
    }))
}

pub async fn list_categories(
    State(state): State<AppState>,
) -> Result<Json<CategoriesResponse>, (StatusCode, Json<ErrorResponse>)> {
    if let Some(categories) = read_cache(state.categories_cache.as_ref()).await {
        return Ok(Json(CategoriesResponse {
            categories,
        }));
    }

    let categories = state
        .store
        .list_categories()
        .await
        .map_err(|e| internal_error("Failed to fetch categories", e))?;

    write_cache(state.categories_cache.as_ref(), categories.clone()).await;
    Ok(Json(CategoriesResponse {
        categories,
    }))
}

pub async fn get_stats(
    State(state): State<AppState>,
) -> Result<Json<StatsResponse>, (StatusCode, Json<ErrorResponse>)> {
    if let Some(stats) = read_cache(state.stats_cache.as_ref()).await {
        return Ok(Json(stats));
    }

    let stats = state
        .store
        .fetch_stats()
        .await
        .map_err(|e| internal_error("Failed to fetch stats", e))?;

    write_cache(state.stats_cache.as_ref(), stats.clone()).await;
    Ok(Json(stats))
}

pub async fn search_articles(
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<SearchResponse>, (StatusCode, Json<ErrorResponse>)> {
    let keyword = query.q.trim();
    if keyword.is_empty() {
        return Ok(Json(SearchResponse {
            results: vec![],
            total: 0,
            query: query.q,
        }));
    }

    let results = state
        .store
        .search_articles(keyword, normalize_limit(query.limit))
        .await
        .map_err(|e| internal_error("Failed to search articles", e))?;

    Ok(Json(SearchResponse {
        total: results.len(),
        results,
        query: query.q,
    }))
}

pub async fn semantic_search(
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<SearchResponse>, (StatusCode, Json<ErrorResponse>)> {
    let keyword = query.q.trim();
    if keyword.is_empty() {
        return Ok(Json(SearchResponse {
            results: vec![],
            total: 0,
            query: query.q,
        }));
    }

    let results = state
        .store
        .semantic_search(
            keyword,
            normalize_limit(query.limit),
            normalize_max_distance(query.max_distance),
            query.enhanced_highlight,
            query.hybrid,
            normalize_positive_f32(query.hybrid_rrf_k),
            normalize_limit(query.hybrid_vector_limit),
            normalize_limit(query.hybrid_fts_limit),
        )
        .await
        .map_err(|e| internal_error("Failed to run semantic search", e))?;

    Ok(Json(SearchResponse {
        total: results.len(),
        results,
        query: query.q,
    }))
}

pub async fn related_articles(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ArticleListResponse>, (StatusCode, Json<ErrorResponse>)> {
    let articles = state
        .store
        .related_articles(&id, 4)
        .await
        .map_err(|e| internal_error("Failed to fetch related articles", e))?;

    Ok(Json(ArticleListResponse {
        total: articles.len(),
        articles,
    }))
}

pub async fn list_images(
    State(state): State<AppState>,
) -> Result<Json<ImageListResponse>, (StatusCode, Json<ErrorResponse>)> {
    let images = state
        .store
        .list_images()
        .await
        .map_err(|e| internal_error("Failed to fetch images", e))?;

    Ok(Json(ImageListResponse {
        total: images.len(),
        images,
    }))
}

pub async fn search_images(
    State(state): State<AppState>,
    Query(query): Query<ImageSearchQuery>,
) -> Result<Json<ImageSearchResponse>, (StatusCode, Json<ErrorResponse>)> {
    let images = state
        .store
        .search_images(
            &query.id,
            normalize_limit(query.limit),
            normalize_max_distance(query.max_distance),
        )
        .await
        .map_err(|e| internal_error("Failed to search images", e))?;

    Ok(Json(ImageSearchResponse {
        total: images.len(),
        images,
        query_id: query.id,
    }))
}

pub async fn search_images_by_text(
    State(state): State<AppState>,
    Query(query): Query<ImageTextSearchQuery>,
) -> Result<Json<ImageTextSearchResponse>, (StatusCode, Json<ErrorResponse>)> {
    let keyword = query.q.trim();
    if keyword.is_empty() {
        return Ok(Json(ImageTextSearchResponse {
            total: 0,
            images: vec![],
            query: query.q,
        }));
    }

    let images = state
        .store
        .search_images_by_text(
            keyword,
            normalize_limit(query.limit),
            normalize_max_distance(query.max_distance),
        )
        .await
        .map_err(|e| internal_error("Failed to search images by text", e))?;

    Ok(Json(ImageTextSearchResponse {
        total: images.len(),
        images,
        query: query.q,
    }))
}

pub async fn serve_image(
    State(state): State<AppState>,
    Path(filename): Path<String>,
    Query(query): Query<ImageRenderQuery>,
) -> Result<Response, (StatusCode, Json<ErrorResponse>)> {
    let image = state
        .store
        .get_image(&filename, query.thumb.unwrap_or(false))
        .await
        .map_err(|e| internal_error("Failed to fetch image", e))?;

    let image = match image {
        Some(image) => image,
        None => {
            return Err((
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: "Image not found".to_string(),
                    code: 404,
                }),
            ));
        },
    };

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, image.mime_type)
        .header(header::CACHE_CONTROL, "public, max-age=31536000")
        .body(Body::from(image.bytes))
        .unwrap())
}

async fn ensure_article_exists(
    state: &AppState,
    id: &str,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    let article = state
        .store
        .get_article(id)
        .await
        .map_err(|e| internal_error("Failed to fetch article", e))?;
    if article.is_some() {
        Ok(())
    } else {
        Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Article not found".to_string(),
                code: 404,
            }),
        ))
    }
}

fn build_client_fingerprint(headers: &HeaderMap) -> String {
    let ip = extract_client_ip(headers);
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("unknown");
    let raw = format!("{ip}|{user_agent}");

    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn extract_client_ip(headers: &HeaderMap) -> String {
    // Prefer X-Forwarded-For chain first (proxy chain source-of-truth), then
    // fall back to X-Real-IP.
    parse_first_ip_from_header(headers.get("x-forwarded-for"))
        .or_else(|| parse_first_ip_from_header(headers.get("x-real-ip")))
        .unwrap_or_else(|| "unknown".to_string())
}

fn parse_first_ip_from_header(value: Option<&axum::http::HeaderValue>) -> Option<String> {
    let raw = value?.to_str().ok()?;
    raw.split(',').find_map(normalize_ip_token)
}

fn normalize_ip_token(token: &str) -> Option<String> {
    let mut value = token.trim().trim_matches('"');
    if value.is_empty() || value.eq_ignore_ascii_case("unknown") {
        return None;
    }

    // Handle RFC7239 style token fragment: for=1.2.3.4
    if let Some(stripped) = value.strip_prefix("for=") {
        value = stripped.trim().trim_matches('"');
    }

    // [IPv6]:port
    if value.starts_with('[') {
        if let Some(end) = value.find(']') {
            let host = &value[1..end];
            let remain = value[end + 1..].trim();
            let valid_suffix = remain.is_empty()
                || (remain.starts_with(':') && remain[1..].chars().all(|ch| ch.is_ascii_digit()));
            if valid_suffix {
                if let Ok(ip) = host.parse::<IpAddr>() {
                    return Some(ip.to_string());
                }
            }
        }
    }

    // Plain IP literal (IPv4 or IPv6).
    if let Ok(ip) = value.parse::<IpAddr>() {
        return Some(ip.to_string());
    }

    // IPv4:port
    if let Some((host, port)) = value.rsplit_once(':') {
        if host.contains('.') && !port.is_empty() && port.chars().all(|ch| ch.is_ascii_digit()) {
            if let Ok(ip) = host.parse::<IpAddr>() {
                return Some(ip.to_string());
            }
        }
    }

    None
}

async fn enforce_comment_submit_rate_limit(
    guard: &tokio::sync::RwLock<HashMap<String, i64>>,
    fingerprint: &str,
    now_ms: i64,
    rate_limit_seconds: u64,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    let window_ms = (rate_limit_seconds.max(1) as i64) * 1_000;
    let mut writer = guard.write().await;
    if let Some(last) = writer.get(fingerprint) {
        if now_ms - *last < window_ms {
            return Err((
                StatusCode::TOO_MANY_REQUESTS,
                Json(ErrorResponse {
                    error: format!(
                        "Comment submit rate-limited. Retry in {} seconds.",
                        rate_limit_seconds
                    ),
                    code: 429,
                }),
            ));
        }
    }
    writer.insert(fingerprint.to_string(), now_ms);
    let stale_before = now_ms - window_ms * 6;
    writer.retain(|_, value| *value >= stale_before);
    Ok(())
}

fn ensure_admin_access(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    if let Some(expected_token) = state.admin_access.token.as_deref() {
        let provided = headers
            .get("x-admin-token")
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .unwrap_or_default();
        if provided == expected_token {
            return Ok(());
        }
    }

    if !state.admin_access.local_only {
        return Ok(());
    }

    let ip = extract_client_ip(headers);
    if ip == "unknown" {
        if is_local_host_header(headers) {
            return Ok(());
        }
        return Err((
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: "Admin endpoint is local-only".to_string(),
                code: 403,
            }),
        ));
    }

    let ip = ip.parse::<IpAddr>().map_err(|_| {
        (
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: "Admin endpoint is local-only".to_string(),
                code: 403,
            }),
        )
    })?;

    if is_private_or_loopback_ip(ip) {
        Ok(())
    } else {
        Err((
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: "Admin endpoint is local-only".to_string(),
                code: 403,
            }),
        ))
    }
}

fn is_private_or_loopback_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.octets()[0] == 169 && v4.octets()[1] == 254
        },
        IpAddr::V6(v6) => v6.is_loopback() || v6.is_unique_local() || v6.is_unicast_link_local(),
    }
}

fn is_local_host_header(headers: &HeaderMap) -> bool {
    let Some(raw_host) = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let host = raw_host.trim();
    if host.eq_ignore_ascii_case("localhost") || host.eq_ignore_ascii_case("[::1]") {
        return true;
    }

    if let Some(host_only) = host
        .strip_prefix('[')
        .and_then(|value| value.split_once(']').map(|parts| parts.0))
    {
        if let Ok(ip) = host_only.parse::<IpAddr>() {
            return is_private_or_loopback_ip(ip);
        }
    }

    let host_only = host
        .split_once(':')
        .map(|parts| parts.0)
        .unwrap_or(host)
        .trim();
    if host_only.eq_ignore_ascii_case("localhost") {
        return true;
    }

    host_only
        .parse::<IpAddr>()
        .map(is_private_or_loopback_ip)
        .unwrap_or(false)
}

#[derive(Default)]
struct ReplyContext {
    reply_to_comment_id: Option<String>,
    reply_to_comment_text: Option<String>,
    reply_to_ai_reply_markdown: Option<String>,
}

async fn resolve_reply_context(
    store: &CommentDataStore,
    article_id: &str,
    reply_to_comment_id: Option<&str>,
) -> Result<ReplyContext, (StatusCode, Json<ErrorResponse>)> {
    let Some(reply_to_comment_id) = reply_to_comment_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(ReplyContext::default());
    };

    let published = store
        .get_published_comment_by_comment_id(reply_to_comment_id)
        .await
        .map_err(|e| internal_error("Failed to resolve reply target", e))?;
    if let Some(comment) = published {
        if comment.article_id != article_id {
            return Err(bad_request("`reply_to_comment_id` does not belong to this article"));
        }
        return Ok(ReplyContext {
            reply_to_comment_id: Some(comment.comment_id),
            reply_to_comment_text: Some(comment.comment_text),
            reply_to_ai_reply_markdown: normalize_optional_markdown(Some(comment.ai_reply_markdown)),
        });
    }

    let task = store
        .get_comment_task(reply_to_comment_id)
        .await
        .map_err(|e| internal_error("Failed to resolve reply task target", e))?;
    if let Some(task) = task {
        if task.article_id != article_id {
            return Err(bad_request("`reply_to_comment_id` does not belong to this article"));
        }
        return Ok(ReplyContext {
            reply_to_comment_id: Some(reply_to_comment_id.to_string()),
            reply_to_comment_text: Some(task.comment_text),
            reply_to_ai_reply_markdown: None,
        });
    }

    Err(bad_request("`reply_to_comment_id` is invalid"))
}

fn public_comment_from_published(
    row: static_flow_shared::comments_store::PublishedCommentRecord,
    override_time: Option<i64>,
) -> PublicCommentItem {
    PublicCommentItem {
        comment_id: row.comment_id,
        article_id: row.article_id,
        task_id: row.task_id,
        author_name: row.author_name,
        author_avatar_seed: row.author_avatar_seed,
        comment_text: row.comment_text,
        selected_text: row.selected_text,
        anchor_block_id: row.anchor_block_id,
        anchor_context_before: row.anchor_context_before,
        anchor_context_after: row.anchor_context_after,
        reply_to_comment_id: row.reply_to_comment_id,
        reply_to_comment_text: row.reply_to_comment_text,
        reply_to_ai_reply_markdown: row.reply_to_ai_reply_markdown,
        ai_reply_markdown: normalize_optional_markdown(Some(row.ai_reply_markdown)),
        ip_region: row.ip_region,
        published_at: override_time.unwrap_or(row.published_at),
    }
}

fn public_comment_from_task(
    task: static_flow_shared::comments_store::CommentTaskRecord,
    published: Option<static_flow_shared::comments_store::PublishedCommentRecord>,
) -> PublicCommentItem {
    if let Some(row) = published {
        return public_comment_from_published(row, Some(task.created_at));
    }

    let (author_name, author_avatar_seed) = derive_author_identity_for_public(&task.fingerprint);
    PublicCommentItem {
        comment_id: task.task_id.clone(),
        article_id: task.article_id,
        task_id: task.task_id,
        author_name,
        author_avatar_seed,
        comment_text: task.comment_text,
        selected_text: task.selected_text,
        anchor_block_id: task.anchor_block_id,
        anchor_context_before: task.anchor_context_before,
        anchor_context_after: task.anchor_context_after,
        reply_to_comment_id: task.reply_to_comment_id,
        reply_to_comment_text: task.reply_to_comment_text,
        reply_to_ai_reply_markdown: task.reply_to_ai_reply_markdown,
        ai_reply_markdown: None,
        ip_region: task.ip_region,
        published_at: task.created_at,
    }
}

fn merge_ai_output_chunks(chunks: &[CommentAiRunChunkRecord]) -> (String, String, String) {
    let mut ordered = chunks.to_vec();
    ordered.sort_by(|left, right| left.batch_index.cmp(&right.batch_index));

    let mut merged_stdout = String::new();
    let mut merged_stderr = String::new();
    let mut merged_output = String::new();

    for chunk in ordered {
        match chunk.stream.as_str() {
            "stderr" => append_merged_chunk(&mut merged_stderr, &chunk.content),
            _ => append_merged_chunk(&mut merged_stdout, &chunk.content),
        }
        append_merged_chunk(&mut merged_output, &chunk.content);
    }

    (merged_stdout, merged_stderr, merged_output)
}

fn append_merged_chunk(buffer: &mut String, chunk: &str) {
    if chunk.is_empty() {
        return;
    }
    if !buffer.is_empty() {
        buffer.push('\n');
    }
    buffer.push_str(chunk);
}

fn derive_author_identity_for_public(fingerprint: &str) -> (String, String) {
    let salt = std::env::var("COMMENT_AUTHOR_SALT")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "static-flow-comment".to_string());
    let raw = format!("{fingerprint}:{salt}");
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    let digest = format!("{:x}", hasher.finalize());
    let short = &digest[..10];
    (format!("Reader-{}", &short[..6]), short.to_string())
}

fn normalize_optional_markdown(value: Option<String>) -> Option<String> {
    value
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
}

fn normalize_comment_list_limit(limit: Option<usize>, default_limit: usize) -> usize {
    let fallback = default_limit
        .max(1)
        .min(MAX_CONFIGURABLE_COMMENT_LIST_LIMIT);
    limit
        .filter(|value| *value > 0)
        .map(|value| value.min(MAX_CONFIGURABLE_COMMENT_LIST_LIMIT))
        .unwrap_or(fallback)
}

fn generate_task_id(prefix: &str) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    format!("{prefix}-{now_ms}-{nanos}")
}

fn bad_request(message: &str) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::BAD_REQUEST,
        Json(ErrorResponse {
            error: message.to_string(),
            code: 400,
        }),
    )
}

fn conflict_error(message: &str) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::CONFLICT,
        Json(ErrorResponse {
            error: message.to_string(),
            code: 409,
        }),
    )
}

fn map_comment_action_error(
    fallback_message: &str,
    err: impl std::fmt::Display,
) -> (StatusCode, Json<ErrorResponse>) {
    let text = err.to_string();
    if text.contains("invalid comment task transition") {
        return conflict_error(&text);
    }
    internal_error(fallback_message, text)
}

fn is_valid_day_format(value: &str) -> bool {
    if value.len() != 10 {
        return false;
    }
    let bytes = value.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if index == 4 || index == 7 {
            if *byte != b'-' {
                return false;
            }
        } else if !byte.is_ascii_digit() {
            return false;
        }
    }
    true
}

async fn read_cache<T: Clone>(cache: &tokio::sync::RwLock<Option<(T, Instant)>>) -> Option<T> {
    let cache = cache.read().await;
    match cache.as_ref() {
        Some((items, cached_at)) if cached_at.elapsed() < CACHE_TTL => Some(items.clone()),
        _ => None,
    }
}

async fn write_cache<T>(cache: &tokio::sync::RwLock<Option<(T, Instant)>>, items: T) {
    let mut cache = cache.write().await;
    *cache = Some((items, Instant::now()));
}

fn internal_error(message: &str, err: impl std::fmt::Display) -> (StatusCode, Json<ErrorResponse>) {
    tracing::error!("{}: {}", message, err);
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorResponse {
            error: message.to_string(),
            code: 500,
        }),
    )
}

fn normalize_limit(limit: Option<usize>) -> Option<usize> {
    limit.filter(|value| *value > 0)
}

fn normalize_max_distance(max_distance: Option<f32>) -> Option<f32> {
    max_distance.filter(|value| value.is_finite() && *value >= 0.0)
}

fn normalize_positive_f32(value: Option<f32>) -> Option<f32> {
    value.filter(|item| item.is_finite() && *item > 0.0)
}

fn apply_view_analytics_config_update(
    current: ViewAnalyticsRuntimeConfig,
    request: UpdateViewAnalyticsConfigRequest,
) -> Result<ViewAnalyticsRuntimeConfig, (StatusCode, Json<ErrorResponse>)> {
    let mut next = current;

    if let Some(value) = request.dedupe_window_seconds {
        if value == 0 || value > MAX_CONFIGURABLE_VIEW_DEDUPE_WINDOW_SECONDS {
            return Err(bad_request("`dedupe_window_seconds` must be between 1 and 3600"));
        }
        next.dedupe_window_seconds = value;
    }

    if let Some(value) = request.trend_max_days {
        if value == 0 || value > MAX_CONFIGURABLE_VIEW_TREND_DAYS {
            return Err(bad_request("`trend_max_days` must be between 1 and 365"));
        }
        next.trend_max_days = value;
    }

    if let Some(value) = request.trend_default_days {
        if value == 0 || value > MAX_CONFIGURABLE_VIEW_TREND_DAYS {
            return Err(bad_request("`trend_default_days` must be between 1 and 365"));
        }
        next.trend_default_days = value;
    }

    if next.trend_default_days > next.trend_max_days {
        return Err(bad_request(
            "`trend_default_days` must be less than or equal to `trend_max_days`",
        ));
    }

    Ok(next)
}

fn apply_comment_runtime_config_update(
    current: CommentRuntimeConfig,
    request: UpdateCommentRuntimeConfigRequest,
) -> Result<CommentRuntimeConfig, (StatusCode, Json<ErrorResponse>)> {
    let mut next = current;

    if let Some(value) = request.submit_rate_limit_seconds {
        if value == 0 || value > MAX_CONFIGURABLE_COMMENT_RATE_LIMIT_SECONDS {
            return Err(bad_request("`submit_rate_limit_seconds` must be between 1 and 3600"));
        }
        next.submit_rate_limit_seconds = value;
    }

    if let Some(value) = request.list_default_limit {
        if value == 0 || value > MAX_CONFIGURABLE_COMMENT_LIST_LIMIT {
            return Err(bad_request("`list_default_limit` must be between 1 and 200"));
        }
        next.list_default_limit = value;
    }

    if let Some(value) = request.cleanup_retention_days {
        if value != -1 && (value <= 0 || value > MAX_CONFIGURABLE_COMMENT_CLEANUP_RETENTION_DAYS) {
            return Err(bad_request("`cleanup_retention_days` must be -1 or between 1 and 3650"));
        }
        next.cleanup_retention_days = value;
    }

    Ok(next)
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue};

    use super::{
        apply_view_analytics_config_update, extract_client_ip, is_local_host_header,
        UpdateViewAnalyticsConfigRequest,
    };
    use crate::state::ViewAnalyticsRuntimeConfig;

    #[test]
    fn extract_client_ip_prefers_x_forwarded_for() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", HeaderValue::from_static("203.0.113.9"));
        headers.insert("x-forwarded-for", HeaderValue::from_static("198.51.100.1, 198.51.100.2"));

        assert_eq!(extract_client_ip(&headers), "198.51.100.1");
    }

    #[test]
    fn extract_client_ip_falls_back_to_x_forwarded_for() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", HeaderValue::from_static("198.51.100.1, 198.51.100.2"));

        assert_eq!(extract_client_ip(&headers), "198.51.100.1");
    }

    #[test]
    fn extract_client_ip_normalizes_ip_with_port() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", HeaderValue::from_static("198.51.100.1:4567"));
        assert_eq!(extract_client_ip(&headers), "198.51.100.1");
    }

    #[test]
    fn extract_client_ip_supports_rfc7239_for_token() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", HeaderValue::from_static("for=198.51.100.77"));
        assert_eq!(extract_client_ip(&headers), "198.51.100.77");
    }

    #[test]
    fn extract_client_ip_returns_unknown_when_no_valid_header() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", HeaderValue::from_static("not-an-ip"));
        headers.insert("x-forwarded-for", HeaderValue::from_static("unknown, bad-token"));

        assert_eq!(extract_client_ip(&headers), "unknown");
    }

    #[test]
    fn local_host_header_is_accepted_for_local_only_admin() {
        let mut headers = HeaderMap::new();
        headers.insert("host", HeaderValue::from_static("127.0.0.1:39080"));
        assert!(is_local_host_header(&headers));

        headers.insert("host", HeaderValue::from_static("localhost:39080"));
        assert!(is_local_host_header(&headers));

        headers.insert("host", HeaderValue::from_static("[::1]:39080"));
        assert!(is_local_host_header(&headers));
    }

    #[test]
    fn non_local_host_header_is_rejected_for_local_only_admin() {
        let mut headers = HeaderMap::new();
        headers.insert("host", HeaderValue::from_static("ackingliu.top"));
        assert!(!is_local_host_header(&headers));
    }

    #[test]
    fn update_view_analytics_config_rejects_invalid_ranges() {
        let result = apply_view_analytics_config_update(
            ViewAnalyticsRuntimeConfig::default(),
            UpdateViewAnalyticsConfigRequest {
                dedupe_window_seconds: Some(0),
                trend_default_days: None,
                trend_max_days: None,
            },
        );
        assert!(result.is_err());

        let result = apply_view_analytics_config_update(
            ViewAnalyticsRuntimeConfig::default(),
            UpdateViewAnalyticsConfigRequest {
                dedupe_window_seconds: None,
                trend_default_days: Some(300),
                trend_max_days: Some(30),
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn update_view_analytics_config_applies_partial_update() {
        let config = apply_view_analytics_config_update(
            ViewAnalyticsRuntimeConfig::default(),
            UpdateViewAnalyticsConfigRequest {
                dedupe_window_seconds: Some(120),
                trend_default_days: None,
                trend_max_days: Some(240),
            },
        )
        .expect("should apply partial config update");

        assert_eq!(config.dedupe_window_seconds, 120);
        assert_eq!(config.trend_default_days, 30);
        assert_eq!(config.trend_max_days, 240);
    }
}
