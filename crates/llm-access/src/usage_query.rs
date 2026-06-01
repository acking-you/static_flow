//! Reusable usage query JSON contract for API and worker routes.

use std::sync::{Arc, RwLock};

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use llm_access_core::{
    store::{
        KiroLatencyRankingQuery, UsageAnalyticsStore, UsageChartPoint, UsageEventPage,
        UsageEventQuery, UsageEventSource, UsageEventStatusKind, UsageMetricsQuery,
        DEFAULT_USAGE_ANALYTICS_RETENTION_DAYS,
    },
    usage::UsageEvent,
};
use serde::{Deserialize, Serialize};

const DEFAULT_ADMIN_USAGE_LIMIT: usize = 20;
const MAX_ADMIN_USAGE_LIMIT: usize = 200;
const DEFAULT_USAGE_CHART_BUCKET_MS: i64 = 60 * 60 * 1000;
const DEFAULT_USAGE_CHART_BUCKETS: usize = 24;
const MAX_USAGE_CHART_BUCKETS: usize = 168;
const DEFAULT_USAGE_METRICS_WINDOW: &str = "1h";
const DEFAULT_USAGE_METRICS_TOP_LIMIT: usize = 10;
const MAX_USAGE_METRICS_TOP_LIMIT: usize = 20;

/// Query options for usage list endpoints.
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct ListUsageEventsRequest {
    #[serde(default)]
    key_id: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    account_name: Option<String>,
    #[serde(default)]
    endpoint: Option<String>,
    #[serde(default)]
    status_code: Option<i32>,
    #[serde(default)]
    status_kind: Option<String>,
    #[serde(default)]
    start_ms: Option<i64>,
    #[serde(default)]
    end_ms: Option<i64>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    offset: Option<usize>,
}

/// Query options for recent usage monitoring metrics.
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct UsageMetricsRequest {
    #[serde(default)]
    provider_type: Option<String>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    window: Option<String>,
    #[serde(default)]
    top_limit: Option<usize>,
}

/// Query options for API-side Kiro latency routing weights.
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct KiroLatencyRankingRequest {
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    window: Option<String>,
}

/// Aggregate totals over the full filtered result set.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct AdminUsageTotalsView {
    pub(crate) event_count: usize,
    pub(crate) input_uncached_tokens: u64,
    pub(crate) input_cached_tokens: u64,
    pub(crate) output_tokens: u64,
    pub(crate) billable_tokens: u64,
}

/// Paginated usage response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AdminUsageEventsResponse {
    pub(crate) total: usize,
    pub(crate) offset: usize,
    pub(crate) limit: usize,
    pub(crate) has_more: bool,
    pub(crate) current_rpm: u32,
    pub(crate) current_in_flight: u32,
    #[serde(default = "default_usage_analytics_retention_days")]
    pub(crate) retention_days: u64,
    #[serde(default)]
    pub(crate) totals: AdminUsageTotalsView,
    pub(crate) events: Vec<AdminUsageEventView>,
    pub(crate) generated_at: i64,
}

fn default_usage_analytics_retention_days() -> u64 {
    DEFAULT_USAGE_ANALYTICS_RETENTION_DAYS
}

/// Summary usage event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AdminUsageEventView {
    pub(crate) id: String,
    pub(crate) key_id: String,
    pub(crate) key_name: String,
    pub(crate) account_name: Option<String>,
    pub(crate) request_method: String,
    pub(crate) request_url: String,
    pub(crate) latency_ms: i32,
    pub(crate) routing_wait_ms: Option<i32>,
    pub(crate) upstream_headers_ms: Option<i32>,
    pub(crate) post_headers_body_ms: Option<i32>,
    pub(crate) request_body_bytes: Option<u64>,
    pub(crate) request_body_read_ms: Option<i32>,
    pub(crate) request_json_parse_ms: Option<i32>,
    pub(crate) pre_handler_ms: Option<i32>,
    pub(crate) first_sse_write_ms: Option<i32>,
    pub(crate) stream_finish_ms: Option<i32>,
    pub(crate) stream_completed_cleanly: Option<bool>,
    pub(crate) downstream_disconnect: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) final_event_type: Option<String>,
    pub(crate) bytes_streamed: Option<u64>,
    pub(crate) other_latency_ms: Option<i32>,
    pub(crate) quota_failover_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) routing_diagnostics_json: Option<String>,
    pub(crate) endpoint: String,
    pub(crate) model: Option<String>,
    pub(crate) status_code: i32,
    pub(crate) input_uncached_tokens: u64,
    pub(crate) input_cached_tokens: u64,
    pub(crate) output_tokens: u64,
    pub(crate) billable_tokens: u64,
    pub(crate) usage_missing: bool,
    pub(crate) credit_usage: Option<f64>,
    pub(crate) credit_usage_missing: bool,
    pub(crate) client_ip: String,
    pub(crate) ip_region: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) last_message_content: Option<String>,
    pub(crate) created_at: i64,
}

/// Usage detail response.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct AdminUsageEventDetailView {
    #[serde(flatten)]
    event: AdminUsageEventView,
    request_headers_json: String,
    client_request_body_json: Option<String>,
    upstream_request_body_json: Option<String>,
    full_request_json: Option<String>,
    error_message: Option<String>,
    error_body: Option<String>,
}

/// Chart query options.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct UsageChartRequest {
    pub(crate) key_id: String,
    #[serde(default)]
    pub(crate) start_ms: Option<i64>,
    #[serde(default)]
    pub(crate) bucket_ms: Option<i64>,
    #[serde(default)]
    pub(crate) bucket_count: Option<usize>,
}

/// Usage chart response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct UsageChartResponse {
    pub(crate) chart_points: Vec<UsageChartPointView>,
}

/// One chart bucket.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct UsageChartPointView {
    pub(crate) bucket_start_ms: i64,
    pub(crate) tokens: u64,
}

/// Worker query route state.
#[derive(Clone)]
pub(crate) struct UsageQueryState {
    pub(crate) usage_analytics_store: Arc<dyn UsageAnalyticsStore>,
    pub(crate) retention_days: Arc<RwLock<u64>>,
}

/// List all LLM usage events.
pub(crate) async fn list_llm_usage_events(
    State(state): State<UsageQueryState>,
    Query(request): Query<ListUsageEventsRequest>,
) -> Response {
    list_usage_events(state, request, None).await
}

/// List Kiro usage events.
pub(crate) async fn list_kiro_usage_events(
    State(state): State<UsageQueryState>,
    Query(request): Query<ListUsageEventsRequest>,
) -> Response {
    list_usage_events(state, request, Some("kiro")).await
}

/// Return one LLM usage event.
pub(crate) async fn get_llm_usage_event(
    State(state): State<UsageQueryState>,
    Path(event_id): Path<String>,
) -> Response {
    get_usage_event(state, event_id, None).await
}

/// Return one Kiro usage event.
pub(crate) async fn get_kiro_usage_event(
    State(state): State<UsageQueryState>,
    Path(event_id): Path<String>,
) -> Response {
    get_usage_event(state, event_id, Some("kiro")).await
}

/// Return chart buckets for a public usage key.
pub(crate) async fn usage_chart_points(
    State(state): State<UsageQueryState>,
    Query(request): Query<UsageChartRequest>,
) -> Response {
    let key_id = request.key_id.trim();
    if key_id.is_empty() {
        tracing::warn!("invalid usage chart query: missing key_id");
        return (StatusCode::BAD_REQUEST, "key_id is required").into_response();
    }
    let start_ms = request.start_ms.unwrap_or(0).max(0);
    let bucket_ms = request
        .bucket_ms
        .unwrap_or(DEFAULT_USAGE_CHART_BUCKET_MS)
        .max(1);
    let bucket_count = request
        .bucket_count
        .unwrap_or(DEFAULT_USAGE_CHART_BUCKETS)
        .clamp(1, MAX_USAGE_CHART_BUCKETS);
    match state
        .usage_analytics_store
        .usage_chart_points(key_id, start_ms, bucket_ms, bucket_count)
        .await
    {
        Ok(points) => Json(UsageChartResponse {
            chart_points: points.into_iter().map(UsageChartPointView::from).collect(),
        })
        .into_response(),
        Err(err) => {
            tracing::error!(key_id, error = ?err, "failed to load usage chart");
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to load usage chart: {err:#}"),
            )
                .into_response()
        },
    }
}

/// Filter options response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct UsageFilterOptionsResponse {
    pub(crate) models: Vec<String>,
    pub(crate) accounts: Vec<String>,
    pub(crate) endpoints: Vec<String>,
}

/// Return distinct model/account/endpoint values for filter autocomplete.
pub(crate) async fn usage_filter_options(
    State(state): State<UsageQueryState>,
    Query(request): Query<ListUsageEventsRequest>,
) -> Response {
    let query = match normalize_usage_query(request, None) {
        Ok(query) => query,
        Err(message) => {
            tracing::warn!(message, "invalid usage filter options query");
            return (StatusCode::BAD_REQUEST, message).into_response();
        },
    };
    match state
        .usage_analytics_store
        .list_usage_filter_options(query)
        .await
    {
        Ok(options) => Json(UsageFilterOptionsResponse {
            models: options.models,
            accounts: options.accounts,
            endpoints: options.endpoints,
        })
        .into_response(),
        Err(err) => {
            tracing::error!(error = ?err, "failed to load usage filter options");
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to load usage filter options: {err:#}"),
            )
                .into_response()
        },
    }
}

/// Return one recent usage monitoring snapshot.
pub(crate) async fn usage_metrics_snapshot(
    State(state): State<UsageQueryState>,
    Query(request): Query<UsageMetricsRequest>,
) -> Response {
    let query = match normalize_usage_metrics_query(request) {
        Ok(query) => query,
        Err(message) => {
            tracing::warn!(message, "invalid usage metrics query");
            return (StatusCode::BAD_REQUEST, message).into_response();
        },
    };
    match state
        .usage_analytics_store
        .usage_metrics_snapshot(query)
        .await
    {
        Ok(snapshot) => Json(snapshot).into_response(),
        Err(err) => {
            tracing::error!(error = ?err, "failed to load usage metrics snapshot");
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to load usage metrics snapshot: {err:#}"),
            )
                .into_response()
        },
    }
}

/// Return the compact Kiro latency snapshot used by API-side route ordering.
pub(crate) async fn kiro_latency_ranking_snapshot(
    State(state): State<UsageQueryState>,
    Query(request): Query<KiroLatencyRankingRequest>,
) -> Response {
    let query = match normalize_kiro_latency_ranking_query(request) {
        Ok(query) => query,
        Err(message) => {
            tracing::warn!(message, "invalid kiro latency ranking query");
            return (StatusCode::BAD_REQUEST, message).into_response();
        },
    };
    match state
        .usage_analytics_store
        .kiro_latency_ranking_snapshot(query)
        .await
    {
        Ok(snapshot) => Json(snapshot).into_response(),
        Err(err) => {
            tracing::error!(error = ?err, "failed to load kiro latency ranking snapshot");
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to load Kiro latency ranking snapshot: {err:#}"),
            )
                .into_response()
        },
    }
}

async fn list_usage_events(
    state: UsageQueryState,
    request: ListUsageEventsRequest,
    provider_type: Option<&str>,
) -> Response {
    let query = match normalize_usage_query(request, provider_type) {
        Ok(query) => query,
        Err(message) => {
            tracing::warn!(provider_type, message, "invalid usage events query");
            return (StatusCode::BAD_REQUEST, message).into_response();
        },
    };
    match state.usage_analytics_store.list_usage_events(query).await {
        Ok(page) => Json(response_from_page(page, usage_retention_days(&state))).into_response(),
        Err(err) => {
            tracing::error!(provider_type, error = ?err, "failed to list usage events");
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to list usage events: {err:#}"),
            )
                .into_response()
        },
    }
}

async fn get_usage_event(
    state: UsageQueryState,
    event_id: String,
    provider_type: Option<&str>,
) -> Response {
    match state.usage_analytics_store.get_usage_event(&event_id).await {
        Ok(Some(event))
            if provider_type.is_none()
                || provider_type == Some(event.provider_type.as_storage_str()) =>
        {
            Json(detail_from_event(&event)).into_response()
        },
        Ok(_) => axum::http::StatusCode::NOT_FOUND.into_response(),
        Err(err) => {
            tracing::error!(provider_type, event_id, error = ?err, "failed to load usage event");
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to load usage event: {err:#}"),
            )
                .into_response()
        },
    }
}

fn response_from_page(page: UsageEventPage, retention_days: u64) -> AdminUsageEventsResponse {
    AdminUsageEventsResponse {
        total: page.total,
        offset: page.offset,
        limit: page.limit,
        has_more: page.has_more,
        current_rpm: 0,
        current_in_flight: 0,
        retention_days,
        totals: AdminUsageTotalsView {
            event_count: page.totals.event_count,
            input_uncached_tokens: page.totals.input_uncached_tokens,
            input_cached_tokens: page.totals.input_cached_tokens,
            output_tokens: page.totals.output_tokens,
            billable_tokens: page.totals.billable_tokens,
        },
        events: page.events.iter().map(AdminUsageEventView::from).collect(),
        generated_at: now_ms(),
    }
}

fn usage_retention_days(state: &UsageQueryState) -> u64 {
    state
        .retention_days
        .read()
        .map(|value| *value)
        .unwrap_or(llm_access_core::store::DEFAULT_USAGE_ANALYTICS_RETENTION_DAYS)
        .max(1)
}

fn detail_from_event(event: &UsageEvent) -> AdminUsageEventDetailView {
    AdminUsageEventDetailView {
        event: AdminUsageEventView::from(event),
        request_headers_json: event.request_headers_json.clone(),
        client_request_body_json: event.client_request_body_json.clone(),
        upstream_request_body_json: event.upstream_request_body_json.clone(),
        full_request_json: event.full_request_json.clone(),
        error_message: event.error_message.clone(),
        error_body: event.error_body.clone(),
    }
}

fn normalize_usage_query(
    request: ListUsageEventsRequest,
    provider_type: Option<&str>,
) -> Result<UsageEventQuery, &'static str> {
    let (start_ms, end_ms) = normalize_usage_time_range(request.start_ms, request.end_ms);
    let source = match request
        .source
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(value) => UsageEventSource::from_query_value(value)
            .ok_or("source must be one of hot, archive, or all")?,
        None => UsageEventSource::Hot,
    };
    let status_kind = match request
        .status_kind
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(value) => Some(
            UsageEventStatusKind::from_query_value(value)
                .ok_or("status_kind must be one of ok or non_ok")?,
        ),
        None => None,
    };
    Ok(UsageEventQuery {
        key_id: request
            .key_id
            .and_then(|value| normalize_optional_string(&value)),
        provider_type: provider_type.map(str::to_string),
        model: request
            .model
            .and_then(|value| normalize_optional_string(&value)),
        account_name: request
            .account_name
            .and_then(|value| normalize_optional_string(&value)),
        endpoint: request
            .endpoint
            .and_then(|value| normalize_optional_string(&value)),
        status_code: request.status_code,
        status_kind,
        source,
        start_ms,
        end_ms,
        limit: request
            .limit
            .unwrap_or(DEFAULT_ADMIN_USAGE_LIMIT)
            .clamp(1, MAX_ADMIN_USAGE_LIMIT),
        offset: request.offset.unwrap_or(0),
    })
}

fn normalize_usage_metrics_query(
    request: UsageMetricsRequest,
) -> Result<UsageMetricsQuery, &'static str> {
    let source = match request
        .source
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(value) => UsageEventSource::from_query_value(value)
            .ok_or("source must be one of hot, archive, or all")?,
        None => UsageEventSource::All,
    };
    let window = request
        .window
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_USAGE_METRICS_WINDOW);
    let window_ms = match window {
        "15m" => 15 * 60 * 1000,
        "1h" => 60 * 60 * 1000,
        "6h" => 6 * 60 * 60 * 1000,
        "24h" => 24 * 60 * 60 * 1000,
        _ => return Err("window must be one of 15m, 1h, 6h, or 24h"),
    };
    let end_ms = now_ms();
    Ok(UsageMetricsQuery {
        provider_type: request
            .provider_type
            .and_then(|value| normalize_optional_string(&value)),
        source,
        start_ms: end_ms.saturating_sub(window_ms),
        end_ms,
        top_limit: request
            .top_limit
            .unwrap_or(DEFAULT_USAGE_METRICS_TOP_LIMIT)
            .clamp(1, MAX_USAGE_METRICS_TOP_LIMIT),
    })
}

fn normalize_kiro_latency_ranking_query(
    request: KiroLatencyRankingRequest,
) -> Result<KiroLatencyRankingQuery, &'static str> {
    let source = match request
        .source
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(value) => UsageEventSource::from_query_value(value)
            .ok_or("source must be one of hot, archive, or all")?,
        None => UsageEventSource::Hot,
    };
    let window = request
        .window
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("1h");
    let window_ms = match window {
        "15m" => 15 * 60 * 1000,
        "1h" => 60 * 60 * 1000,
        _ => return Err("window must be one of 15m or 1h"),
    };
    let end_ms = now_ms();
    Ok(KiroLatencyRankingQuery {
        source,
        start_ms: end_ms.saturating_sub(window_ms),
        end_ms,
    })
}

impl From<&UsageEvent> for AdminUsageEventView {
    fn from(value: &UsageEvent) -> Self {
        let latency_ms = usage_latency_ms(value);
        Self {
            id: value.event_id.clone(),
            key_id: value.key_id.clone(),
            key_name: value.key_name.clone(),
            account_name: value.account_name.clone(),
            request_method: value.request_method.clone(),
            request_url: value.request_url.clone(),
            latency_ms,
            routing_wait_ms: optional_i64_to_i32(value.timing.routing_wait_ms),
            upstream_headers_ms: optional_i64_to_i32(value.timing.upstream_headers_ms),
            post_headers_body_ms: optional_i64_to_i32(value.timing.post_headers_body_ms),
            request_body_bytes: value.request_body_bytes.and_then(non_negative_i64_to_u64),
            request_body_read_ms: optional_i64_to_i32(value.timing.request_body_read_ms),
            request_json_parse_ms: optional_i64_to_i32(value.timing.request_json_parse_ms),
            pre_handler_ms: optional_i64_to_i32(value.timing.pre_handler_ms),
            first_sse_write_ms: optional_i64_to_i32(value.timing.first_sse_write_ms),
            stream_finish_ms: optional_i64_to_i32(value.timing.stream_finish_ms),
            stream_completed_cleanly: value.stream.stream_completed_cleanly,
            downstream_disconnect: value.stream.downstream_disconnect,
            final_event_type: value.stream.final_event_type.clone(),
            bytes_streamed: value
                .stream
                .bytes_streamed
                .and_then(non_negative_i64_to_u64),
            other_latency_ms: compute_other_latency_ms(
                latency_ms,
                optional_i64_to_i32(value.timing.routing_wait_ms),
                optional_i64_to_i32(value.timing.upstream_headers_ms),
                optional_i64_to_i32(value.timing.post_headers_body_ms),
            ),
            quota_failover_count: value.quota_failover_count,
            routing_diagnostics_json: value.routing_diagnostics_json.clone(),
            endpoint: value.endpoint.clone(),
            model: value.model.clone(),
            status_code: value.status_code.clamp(0, i64::from(i32::MAX)) as i32,
            input_uncached_tokens: non_negative_i64_to_u64(value.input_uncached_tokens)
                .unwrap_or(0),
            input_cached_tokens: non_negative_i64_to_u64(value.input_cached_tokens).unwrap_or(0),
            output_tokens: non_negative_i64_to_u64(value.output_tokens).unwrap_or(0),
            billable_tokens: non_negative_i64_to_u64(value.billable_tokens).unwrap_or(0),
            usage_missing: value.usage_missing,
            credit_usage: value
                .credit_usage
                .as_deref()
                .and_then(|raw| raw.parse().ok()),
            credit_usage_missing: value.credit_usage_missing,
            client_ip: value.client_ip.clone(),
            ip_region: value.ip_region.clone(),
            last_message_content: value.last_message_content.clone(),
            created_at: value.created_at_ms,
        }
    }
}

impl From<UsageChartPoint> for UsageChartPointView {
    fn from(value: UsageChartPoint) -> Self {
        Self {
            bucket_start_ms: value.bucket_start_ms,
            tokens: value.tokens,
        }
    }
}

fn normalize_usage_time_range(
    start_ms: Option<i64>,
    end_ms: Option<i64>,
) -> (Option<i64>, Option<i64>) {
    match (start_ms, end_ms) {
        (Some(start), Some(end)) if end < start => (Some(end), Some(start)),
        values => values,
    }
}

fn normalize_optional_string(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn usage_latency_ms(value: &UsageEvent) -> i32 {
    let latency = value.timing.latency_ms.or(value.timing.stream_finish_ms);
    optional_i64_to_i32(latency).unwrap_or(0)
}

fn optional_i64_to_i32(value: Option<i64>) -> Option<i32> {
    value.map(|value| value.clamp(0, i64::from(i32::MAX)) as i32)
}

fn non_negative_i64_to_u64(value: i64) -> Option<u64> {
    (value >= 0).then_some(value as u64)
}

fn compute_other_latency_ms(
    latency_ms: i32,
    routing_wait_ms: Option<i32>,
    upstream_headers_ms: Option<i32>,
    post_headers_body_ms: Option<i32>,
) -> Option<i32> {
    let measured_ms: i64 = [routing_wait_ms, upstream_headers_ms, post_headers_body_ms]
        .into_iter()
        .flatten()
        .map(|value| i64::from(value.max(0)))
        .sum();
    Some((i64::from(latency_ms.max(0)) - measured_ms).clamp(0, i64::from(i32::MAX)) as i32)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use llm_access_core::store::{UsageEventPage, UsageEventSource, UsageEventStatusKind};

    use super::{
        normalize_usage_metrics_query, normalize_usage_query, response_from_page,
        ListUsageEventsRequest, UsageMetricsRequest,
    };

    #[test]
    fn normalize_usage_query_accepts_explicit_archive_source() {
        let query = normalize_usage_query(
            ListUsageEventsRequest {
                source: Some("archive".to_string()),
                limit: Some(20),
                offset: Some(0),
                ..ListUsageEventsRequest::default()
            },
            None,
        )
        .expect("archive source should be valid");

        assert_eq!(query.source, UsageEventSource::Archive);
    }

    #[test]
    fn normalize_usage_query_rejects_unknown_source() {
        let err = normalize_usage_query(
            ListUsageEventsRequest {
                source: Some("broad-scan".to_string()),
                limit: Some(20),
                offset: Some(0),
                ..ListUsageEventsRequest::default()
            },
            None,
        )
        .expect_err("unknown usage source should fail");

        assert!(err.contains("source must be one of"));
    }

    #[test]
    fn normalize_usage_query_keeps_large_offsets() {
        let query = normalize_usage_query(
            ListUsageEventsRequest {
                limit: Some(500),
                offset: Some(1_000),
                ..ListUsageEventsRequest::default()
            },
            None,
        )
        .expect("large offset should remain valid");

        assert_eq!(query.limit, 200);
        assert_eq!(query.offset, 1_000);
    }

    #[test]
    fn normalize_usage_query_preserves_exact_filters() {
        let query = normalize_usage_query(
            ListUsageEventsRequest {
                model: Some(" gpt-5.4 ".to_string()),
                account_name: Some(" account-a ".to_string()),
                endpoint: Some(" /v1/responses ".to_string()),
                status_code: Some(524),
                ..ListUsageEventsRequest::default()
            },
            Some("codex"),
        )
        .expect("filters should normalize");

        assert_eq!(query.provider_type.as_deref(), Some("codex"));
        assert_eq!(query.model.as_deref(), Some("gpt-5.4"));
        assert_eq!(query.account_name.as_deref(), Some("account-a"));
        assert_eq!(query.endpoint.as_deref(), Some("/v1/responses"));
        assert_eq!(query.status_code, Some(524));
        assert_eq!(query.status_kind, None);
    }

    #[test]
    fn normalize_usage_query_maps_status_kind_bucket() {
        let query = normalize_usage_query(
            ListUsageEventsRequest {
                status_kind: Some("non_ok".to_string()),
                ..ListUsageEventsRequest::default()
            },
            None,
        )
        .expect("status bucket should normalize");

        assert_eq!(query.status_kind, Some(UsageEventStatusKind::NonOk));
    }

    #[test]
    fn normalize_usage_query_rejects_unknown_status_kind() {
        let err = normalize_usage_query(
            ListUsageEventsRequest {
                status_kind: Some("500-class".to_string()),
                ..ListUsageEventsRequest::default()
            },
            None,
        )
        .expect_err("unknown status bucket should fail");

        assert!(err.contains("status_kind must be one of ok or non_ok"));
    }

    #[test]
    fn usage_events_response_declares_retention_days() {
        let response = response_from_page(
            UsageEventPage {
                total: 0,
                offset: 0,
                limit: 20,
                has_more: false,
                totals: llm_access_core::store::UsageEventTotals::default(),
                events: Vec::new(),
            },
            7,
        );

        assert_eq!(response.retention_days, 7);
    }

    #[test]
    fn usage_events_response_defaults_missing_retention_days() {
        let response: super::AdminUsageEventsResponse = serde_json::from_value(serde_json::json!({
            "total": 0,
            "offset": 0,
            "limit": 20,
            "has_more": false,
            "current_rpm": 0,
            "current_in_flight": 0,
            "totals": {
                "event_count": 0,
                "input_uncached_tokens": 0,
                "input_cached_tokens": 0,
                "output_tokens": 0,
                "billable_tokens": 0
            },
            "events": [],
            "generated_at": 1_700_000_000_000_i64
        }))
        .expect("usage response without retention_days");

        assert_eq!(
            response.retention_days,
            llm_access_core::store::DEFAULT_USAGE_ANALYTICS_RETENTION_DAYS
        );
        assert_eq!(response.totals.event_count, 0);
    }

    #[test]
    fn normalize_usage_metrics_query_defaults_to_recent_all_sources() {
        let query = normalize_usage_metrics_query(UsageMetricsRequest::default())
            .expect("default metrics query should normalize");

        assert_eq!(query.source, UsageEventSource::All);
        assert_eq!(query.top_limit, 10);
        assert_eq!(query.end_ms.saturating_sub(query.start_ms), 60 * 60 * 1000);
    }

    #[test]
    fn normalize_usage_metrics_query_rejects_unknown_window() {
        let err = normalize_usage_metrics_query(UsageMetricsRequest {
            window: Some("3d".to_string()),
            ..UsageMetricsRequest::default()
        })
        .expect_err("unknown metrics window should fail");

        assert!(err.contains("window must be one of"));
    }
}
