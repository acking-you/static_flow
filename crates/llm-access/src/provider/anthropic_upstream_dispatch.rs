//! Direct standard Anthropic upstream dispatch for Kiro-owned keys.

use std::{
    collections::BTreeMap,
    io,
    sync::{LazyLock, Mutex},
    time::Instant,
};

use async_stream::stream;
use axum::{
    body::{to_bytes, Body, Bytes},
    http::{header, HeaderMap, Method, Request, Response, StatusCode, Uri, Version},
    response::IntoResponse,
};
use futures_util::StreamExt;
use llm_access_anthropic_pool::{
    build_messages_url, merge_usage, parse_usage_from_value, AnthropicUsageSummary,
    SmoothWeightedRoundRobin, WeightedChannel,
};
use llm_access_core::{
    provider::{ProtocolFamily, ProviderType},
    store::{
        self as core_store, AnthropicUpstreamChannelUsageDelta, AuthenticatedKey,
        ProviderAnthropicUpstreamRoute,
    },
    usage::UsageEvent,
};
use serde_json::Value;

use super::{
    client::anthropic_upstream_client,
    kiro_error::kiro_json_error,
    kiro_protocol::normalized_kiro_messages_path,
    limiter::{kiro_key_limit_response, try_acquire_key_permit},
    usage_meta::{
        capture_client_request_body_json, capture_error_bytes, capture_error_message,
        capture_upstream_request_body_json, captured_body_json,
    },
    util::{clamp_duration_ms, clamp_u64_to_i64, now_millis},
    ProviderDispatchDeps, ProviderUsageMetadata, MAX_PROVIDER_PROXY_BODY_BYTES,
};

static DIRECT_ANTHROPIC_SCHEDULER: LazyLock<Mutex<SmoothWeightedRoundRobin>> =
    LazyLock::new(|| Mutex::new(SmoothWeightedRoundRobin::default()));
const MAX_DIRECT_ANTHROPIC_RESPONSE_BYTES: usize = MAX_PROVIDER_PROXY_BODY_BYTES;

pub(super) enum AnthropicUpstreamDispatchOutcome {
    Handled(axum::response::Response),
    Fallback(Request<Body>),
}

#[derive(Clone)]
struct ReplayableRequest {
    method: Method,
    uri: Uri,
    version: Version,
    headers: HeaderMap,
    body: Bytes,
}

impl ReplayableRequest {
    fn rebuild(&self) -> Request<Body> {
        let mut request = Request::builder()
            .method(self.method.clone())
            .uri(self.uri.clone())
            .version(self.version)
            .body(Body::from(self.body.clone()))
            .expect("replayable provider request should build");
        *request.headers_mut() = self.headers.clone();
        request
    }
}

struct DirectAnthropicDispatchContext<'a> {
    key: &'a AuthenticatedKey,
    endpoint: &'a str,
    model: &'a str,
    request_headers: &'a HeaderMap,
    deps: &'a ProviderDispatchDeps,
}

#[derive(Clone)]
struct DirectAnthropicUsageContext {
    key: AuthenticatedKey,
    endpoint: String,
    model: String,
    mapped_model: Option<String>,
    deps: ProviderDispatchDeps,
}

impl DirectAnthropicUsageContext {
    fn from_dispatch(
        context: &DirectAnthropicDispatchContext<'_>,
        mapped_model: Option<String>,
    ) -> Self {
        Self {
            key: context.key.clone(),
            endpoint: context.endpoint.to_string(),
            model: context.model.to_string(),
            mapped_model,
            deps: context.deps.clone(),
        }
    }
}

pub(super) async fn maybe_dispatch_anthropic_upstream_pool(
    key: AuthenticatedKey,
    request: Request<Body>,
    deps: ProviderDispatchDeps,
) -> AnthropicUpstreamDispatchOutcome {
    let resolution = match deps
        .route_store
        .resolve_anthropic_upstream_resolution(&key)
        .await
    {
        Ok(resolution) => resolution,
        Err(err) => {
            tracing::warn!(
                key_id = %key.key_id,
                error = %err,
                "direct anthropic upstream route resolution failed"
            );
            return AnthropicUpstreamDispatchOutcome::Handled(kiro_json_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "api_error",
                "direct Anthropic upstream route resolution failed",
            ));
        },
    };
    let mode = resolution.pool_mode;
    if mode == core_store::ANTHROPIC_UPSTREAM_POOL_MODE_DISABLED {
        return AnthropicUpstreamDispatchOutcome::Fallback(request);
    }
    let routes = resolution.routes;
    if routes.is_empty() {
        return if mode == core_store::ANTHROPIC_UPSTREAM_POOL_MODE_ONLY {
            AnthropicUpstreamDispatchOutcome::Handled(kiro_json_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "api_error",
                "direct Anthropic upstream route is not configured",
            ))
        } else {
            AnthropicUpstreamDispatchOutcome::Fallback(request)
        };
    }

    let method = request.method().clone();
    let uri = request.uri().clone();
    let version = request.version();
    let headers = request.headers().clone();
    let body_read_started = Instant::now();
    let body = match to_bytes(request.into_body(), MAX_PROVIDER_PROXY_BODY_BYTES).await {
        Ok(body) => body,
        Err(_) => {
            return AnthropicUpstreamDispatchOutcome::Handled(kiro_json_error(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "request body is too large",
            ))
        },
    };
    let replay = ReplayableRequest {
        method,
        uri,
        version,
        headers,
        body,
    };
    let mut usage_meta = ProviderUsageMetadata::from_request_parts(
        &replay.method,
        &replay.uri,
        &replay.headers,
        &deps.geoip,
    )
    .await
    .with_request_body(&replay.body, clamp_duration_ms(body_read_started.elapsed()));

    let Some(public_path) = normalized_kiro_messages_path(replay.uri.path()) else {
        return AnthropicUpstreamDispatchOutcome::Handled(kiro_json_error(
            StatusCode::NOT_FOUND,
            "invalid_request_error",
            "unsupported endpoint",
        ));
    };
    if replay.method != Method::POST {
        return AnthropicUpstreamDispatchOutcome::Handled(kiro_json_error(
            StatusCode::METHOD_NOT_ALLOWED,
            "invalid_request_error",
            "unsupported method",
        ));
    }
    capture_client_request_body_json(&mut usage_meta, &replay.body);

    let parse_started = Instant::now();
    let payload = match serde_json::from_slice::<Value>(&replay.body) {
        Ok(payload) => payload,
        Err(_) => {
            return AnthropicUpstreamDispatchOutcome::Handled(kiro_json_error(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "request body must be a valid Anthropic messages JSON payload",
            ))
        },
    };
    let original_model = match payload
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(model) => model.to_string(),
        None => {
            return AnthropicUpstreamDispatchOutcome::Handled(kiro_json_error(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "model is required",
            ))
        },
    };
    usage_meta.mark_pre_handler_done(clamp_duration_ms(parse_started.elapsed()));

    let context = DirectAnthropicDispatchContext {
        key: &key,
        endpoint: public_path,
        model: &original_model,
        request_headers: &replay.headers,
        deps: &deps,
    };
    let route_queue = order_routes_for_request(routes);
    let mut last_failure: Option<axum::response::Response> = None;
    for route in route_queue {
        let response = dispatch_one_route(&context, &route, &payload, &mut usage_meta).await;
        let retryable = is_retryable_direct_status(response.status());
        if !retryable {
            return AnthropicUpstreamDispatchOutcome::Handled(response);
        }
        last_failure = Some(response);
        usage_meta.mark_failover();
        usage_meta.error_message = None;
        usage_meta.error_class = None;
        usage_meta.error_body = None;
        usage_meta.response_body = None;
    }

    if mode == core_store::ANTHROPIC_UPSTREAM_POOL_MODE_PREFERRED_BEFORE_KIRO {
        AnthropicUpstreamDispatchOutcome::Fallback(replay.rebuild())
    } else {
        AnthropicUpstreamDispatchOutcome::Handled(last_failure.unwrap_or_else(|| {
            kiro_json_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "api_error",
                "direct Anthropic upstream route is not available",
            )
        }))
    }
}

fn order_routes_for_request(
    mut routes: Vec<ProviderAnthropicUpstreamRoute>,
) -> Vec<ProviderAnthropicUpstreamRoute> {
    let mut ordered = Vec::with_capacity(routes.len());
    if let Some(route) = select_weighted_route_global(&mut routes) {
        ordered.push(route);
    }
    let mut local_scheduler = SmoothWeightedRoundRobin::default();
    while let Some(route) = select_weighted_route(&mut local_scheduler, &mut routes) {
        ordered.push(route);
    }
    ordered
}

fn select_weighted_route_global(
    routes: &mut Vec<ProviderAnthropicUpstreamRoute>,
) -> Option<ProviderAnthropicUpstreamRoute> {
    let mut scheduler = DIRECT_ANTHROPIC_SCHEDULER
        .lock()
        .expect("direct anthropic scheduler lock");
    select_weighted_route(&mut scheduler, routes)
}

fn select_weighted_route(
    scheduler: &mut SmoothWeightedRoundRobin,
    routes: &mut Vec<ProviderAnthropicUpstreamRoute>,
) -> Option<ProviderAnthropicUpstreamRoute> {
    if routes.is_empty() {
        return None;
    }
    let weighted = routes
        .iter()
        .map(|route| WeightedChannel {
            name: route.channel_name.clone(),
            weight: route.weight,
        })
        .collect::<Vec<_>>();
    let selected_name = scheduler.select(&weighted).map(str::to_string);
    let index = selected_name
        .and_then(|name| routes.iter().position(|route| route.channel_name == name))
        .unwrap_or(0);
    Some(routes.remove(index))
}

fn apply_model_mapping_to_json(
    model_name_map_json: &str,
    payload: &mut Value,
) -> anyhow::Result<Option<String>> {
    let trimmed = model_name_map_json.trim();
    if trimmed.is_empty() || trimmed == "{}" {
        return Ok(None);
    }
    let map = serde_json::from_str::<BTreeMap<String, String>>(trimmed)?;
    let Some(model) = payload.get("model").and_then(Value::as_str) else {
        return Ok(None);
    };
    let Some(target) = map.get(model).cloned() else {
        return Ok(None);
    };
    if target == model {
        return Ok(None);
    }
    let Some(object) = payload.as_object_mut() else {
        return Ok(None);
    };
    object.insert("model".to_string(), Value::String(target.clone()));
    Ok(Some(target))
}

fn build_route_payload(
    model_name_map_json: &str,
    payload: &Value,
) -> anyhow::Result<(Value, Option<String>)> {
    let mut route_payload = payload.clone();
    let mapped_model = apply_model_mapping_to_json(model_name_map_json, &mut route_payload)?;
    Ok((route_payload, mapped_model))
}

async fn dispatch_one_route(
    context: &DirectAnthropicDispatchContext<'_>,
    route: &ProviderAnthropicUpstreamRoute,
    payload: &Value,
    usage_meta: &mut ProviderUsageMetadata,
) -> axum::response::Response {
    let _key_permit = match try_acquire_key_permit(
        &context.deps.request_limiter,
        context.key,
        route.request_max_concurrency,
        route.request_min_start_interval_ms,
    ) {
        Ok(permit) => permit,
        Err(rejection) => return kiro_key_limit_response(&rejection),
    };
    let _channel_permit = match context.deps.request_limiter.try_acquire(
        format!("anthropic-upstream-channel:{}", route.channel_name),
        Some(route.channel_max_concurrency),
        Some(route.channel_min_start_interval_ms),
    ) {
        Ok(permit) => permit,
        Err(rejection) => return kiro_key_limit_response(&rejection),
    };

    let upstream_url = match build_messages_url(&route.base_url) {
        Ok(url) => url,
        Err(err) => {
            tracing::warn!(
                channel = %route.channel_name,
                error = %err,
                "direct anthropic upstream url is invalid"
            );
            return kiro_json_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "api_error",
                "direct Anthropic upstream URL is invalid",
            );
        },
    };
    let client = match anthropic_upstream_client(route.proxy.as_ref()) {
        Ok(client) => client,
        Err(err) => {
            tracing::warn!(
                channel = %route.channel_name,
                error = %err,
                "direct anthropic upstream client build failed"
            );
            return kiro_json_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "api_error",
                "direct Anthropic upstream client build failed",
            );
        },
    };
    let (route_payload, mapped_model) =
        match build_route_payload(&route.model_name_map_json, payload) {
            Ok(output) => output,
            Err(err) => {
                tracing::warn!(
                    key_id = %context.key.key_id,
                    channel = %route.channel_name,
                    error = %err,
                    "direct anthropic model mapping failed"
                );
                return kiro_json_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "api_error",
                    "model mapping failed",
                );
            },
        };
    let upstream_body = match serde_json::to_vec(&route_payload) {
        Ok(body) => Bytes::from(body),
        Err(err) => {
            tracing::warn!(
                key_id = %context.key.key_id,
                channel = %route.channel_name,
                error = %err,
                "direct anthropic upstream body serialization failed"
            );
            return kiro_json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "api_error",
                "request serialization failed",
            );
        },
    };
    capture_upstream_request_body_json(usage_meta, &upstream_body);
    let mut request = client
        .post(upstream_url)
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-api-key", &route.api_key)
        .body(upstream_body.clone());
    let usage_context = DirectAnthropicUsageContext::from_dispatch(context, mapped_model.clone());
    let anthropic_version = context
        .request_headers
        .get("anthropic-version")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("2023-06-01");
    request = request.header("anthropic-version", anthropic_version);
    for header_name in ["anthropic-beta", "accept", "user-agent"] {
        if let Some(value) = context.request_headers.get(header_name) {
            request = request.header(header_name, value.clone());
        }
    }
    let response = match request.send().await {
        Ok(response) => response,
        Err(err) => {
            capture_error_message(usage_meta, &err.to_string());
            usage_meta.capture_error_class("upstream_transport_error");
            let status = StatusCode::BAD_GATEWAY;
            record_direct_usage(
                &usage_context,
                route,
                status,
                AnthropicUsageSummary::missing(),
                usage_meta,
                Some(upstream_body.clone()),
            )
            .await;
            return kiro_json_error(
                status,
                "api_error",
                "direct Anthropic upstream request failed",
            );
        },
    };
    usage_meta.mark_upstream_headers();
    let status = response.status();
    let headers = response.headers().clone();
    if status.is_success() && is_anthropic_event_stream(&headers) {
        return build_streaming_downstream_response(
            status,
            &headers,
            response,
            usage_context,
            route.clone(),
            usage_meta.clone(),
            upstream_body,
        );
    }

    let body = match read_limited_response_body(response, MAX_DIRECT_ANTHROPIC_RESPONSE_BYTES).await
    {
        Ok(body) => body,
        Err(err) => {
            capture_error_message(usage_meta, &err);
            usage_meta.capture_error_class("upstream_body_error");
            let status = StatusCode::BAD_GATEWAY;
            record_direct_usage(
                &usage_context,
                route,
                status,
                AnthropicUsageSummary::missing(),
                usage_meta,
                Some(upstream_body),
            )
            .await;
            return kiro_json_error(
                status,
                "api_error",
                "direct Anthropic upstream response read failed",
            );
        },
    };
    usage_meta.mark_post_headers_body();
    usage_meta.mark_stream_finish();
    if !status.is_success() {
        capture_error_bytes(usage_meta, &body);
        usage_meta.capture_error_class("upstream_error");
    }
    let usage = parse_anthropic_response_usage(&headers, &body);
    let usage = if !status.is_success() && is_retryable_direct_status(status) {
        AnthropicUsageSummary::missing()
    } else {
        usage
    };
    record_direct_usage(&usage_context, route, status, usage, usage_meta, Some(upstream_body))
        .await;
    build_downstream_response(status, &headers, body)
}

async fn read_limited_response_body(
    response: reqwest::Response,
    max_bytes: usize,
) -> Result<Bytes, String> {
    if response
        .content_length()
        .is_some_and(|len| len > max_bytes as u64)
    {
        return Err("direct Anthropic upstream response is too large".to_string());
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|err| format!("failed to read direct Anthropic upstream body: {err}"))?;
        if body.len().saturating_add(chunk.len()) > max_bytes {
            return Err("direct Anthropic upstream response is too large".to_string());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(Bytes::from(body))
}

fn is_anthropic_event_stream(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(';')
                .any(|part| part.trim().eq_ignore_ascii_case("text/event-stream"))
        })
}

fn is_retryable_direct_status(status: StatusCode) -> bool {
    status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS || status.as_u16() == 529
}

fn build_streaming_downstream_response(
    status: StatusCode,
    headers: &HeaderMap,
    response: reqwest::Response,
    usage_context: DirectAnthropicUsageContext,
    route: ProviderAnthropicUpstreamRoute,
    mut usage_meta: ProviderUsageMetadata,
    upstream_body: Bytes,
) -> axum::response::Response {
    let mut body_stream = response.bytes_stream();
    let body = stream! {
        let mut usage = AnthropicUsageSummary::missing();
        let mut pending_line = String::new();
        while let Some(chunk_result) = body_stream.next().await {
            let chunk = match chunk_result {
                Ok(chunk) => chunk,
                Err(err) => {
                    capture_error_message(&mut usage_meta, &err.to_string());
                    usage_meta.capture_error_class("upstream_body_error");
                    usage_meta.mark_stream_internal_incomplete();
                    record_direct_usage(
                        &usage_context,
                        &route,
                        StatusCode::BAD_GATEWAY,
                        AnthropicUsageSummary::missing(),
                        &usage_meta,
                        Some(upstream_body.clone()),
                    )
                    .await;
                    yield Err(io::Error::other(format!(
                        "failed to read direct Anthropic upstream stream: {err}"
                    )));
                    return;
                },
            };
            usage_meta.observe_stream_write(chunk.len(), None);
            observe_anthropic_sse_chunk(&chunk, &mut pending_line, &mut usage);
            yield Ok::<Bytes, io::Error>(chunk);
        }
        observe_anthropic_sse_tail(&mut pending_line, &mut usage);
        usage_meta.mark_post_headers_body();
        usage_meta.mark_stream_completed_cleanly();
        record_direct_usage(
            &usage_context,
            &route,
            status,
            usage,
            &usage_meta,
            Some(upstream_body),
        )
        .await;
    };
    let mut builder = Response::builder().status(status);
    for (name, value) in headers {
        if is_hop_by_hop_header(name.as_str()) || *name == header::CONTENT_LENGTH {
            continue;
        }
        builder = builder.header(name, value);
    }
    builder
        .body(Body::from_stream(body))
        .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
}

fn observe_anthropic_sse_chunk(
    chunk: &Bytes,
    pending_line: &mut String,
    usage: &mut AnthropicUsageSummary,
) {
    pending_line.push_str(&String::from_utf8_lossy(chunk));
    while let Some(line_end) = pending_line.find('\n') {
        let mut line = pending_line[..line_end].to_string();
        if line.ends_with('\r') {
            line.pop();
        }
        observe_anthropic_sse_line(&line, usage);
        pending_line.drain(..=line_end);
    }
}

fn observe_anthropic_sse_tail(pending_line: &mut String, usage: &mut AnthropicUsageSummary) {
    if pending_line.is_empty() {
        return;
    }
    let line = std::mem::take(pending_line);
    observe_anthropic_sse_line(&line, usage);
}

fn observe_anthropic_sse_line(line: &str, usage: &mut AnthropicUsageSummary) {
    let Some(data) = line.trim_start().strip_prefix("data:") else {
        return;
    };
    let data = data.trim();
    if data.is_empty() || data == "[DONE]" {
        return;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(data) else {
        return;
    };
    *usage = merge_usage(*usage, parse_usage_from_value(&value));
}

fn parse_anthropic_response_usage(headers: &HeaderMap, body: &Bytes) -> AnthropicUsageSummary {
    if is_anthropic_event_stream(headers) {
        return parse_anthropic_sse_usage(body);
    }
    serde_json::from_slice::<serde_json::Value>(body)
        .map(|value| parse_usage_from_value(&value))
        .unwrap_or_else(|_| AnthropicUsageSummary::missing())
}

fn parse_anthropic_sse_usage(body: &Bytes) -> AnthropicUsageSummary {
    let mut usage = AnthropicUsageSummary::missing();
    let mut pending_line = String::new();
    observe_anthropic_sse_chunk(body, &mut pending_line, &mut usage);
    observe_anthropic_sse_tail(&mut pending_line, &mut usage);
    usage
}

async fn record_direct_usage(
    context: &DirectAnthropicUsageContext,
    route: &ProviderAnthropicUpstreamRoute,
    status: StatusCode,
    usage: AnthropicUsageSummary,
    meta: &ProviderUsageMetadata,
    upstream_request_body_json: Option<Bytes>,
) {
    let billable_tokens = if usage.usage_missing {
        0
    } else {
        core_store::compute_kiro_billable_tokens(
            Some(context.mapped_model.as_deref().unwrap_or(&context.model)),
            usage.input_uncached_tokens.max(0) as u64,
            usage.input_cached_tokens.max(0) as u64,
            usage.output_tokens.max(0) as u64,
            &parse_billable_multipliers(&route.billable_model_multipliers_json),
        )
    };
    let used_at_ms = now_millis();
    let event = UsageEvent {
        event_id: format!("llm-usage-{}", uuid::Uuid::new_v4()),
        created_at_ms: used_at_ms,
        provider_type: ProviderType::Kiro,
        protocol_family: ProtocolFamily::Anthropic,
        key_id: context.key.key_id.clone(),
        key_name: context.key.key_name.clone(),
        account_name: Some(route.channel_name.clone()),
        account_group_id_at_event: route.account_group_id_at_event.clone(),
        route_strategy_at_event: Some(route.route_strategy_at_event),
        request_method: meta.request_method.clone(),
        request_url: meta.request_url.clone(),
        endpoint: context.endpoint.clone(),
        model: Some(context.model.clone()),
        mapped_model: context.mapped_model.clone(),
        status_code: status.as_u16() as i64,
        request_body_bytes: meta.request_body_bytes,
        quota_failover_count: meta.quota_failover_count,
        retry: meta.retry.clone(),
        routing_diagnostics_json: Some(
            serde_json::json!({
                "upstream_pool": "direct_anthropic",
                "channel_name": route.channel_name,
                "pool_mode": route.pool_mode_at_event,
            })
            .to_string(),
        ),
        input_uncached_tokens: usage.input_uncached_tokens.max(0),
        input_cached_tokens: usage.input_cached_tokens.max(0),
        output_tokens: usage.output_tokens.max(0),
        billable_tokens: clamp_u64_to_i64(billable_tokens),
        credit_usage: None,
        usage_missing: usage.usage_missing,
        credit_usage_missing: true,
        client_ip: meta.client_ip.clone(),
        ip_region: meta.ip_region.clone(),
        request_headers_json: meta.request_headers_json.clone(),
        last_message_content: meta.last_message_content.clone(),
        client_request_body_json: captured_body_json(&meta.client_request_body_json),
        upstream_request_body_json: captured_body_json(&upstream_request_body_json),
        full_request_json: None,
        error_message: meta.error_message.clone(),
        error_class: meta.error_class.clone(),
        session_blocked: meta.session_blocked,
        response_image_count: None,
        error_body: meta.error_body.clone(),
        response_body: meta.response_body.clone(),
        timing: meta.to_timing(),
        stream: meta.to_stream_details(),
    };
    if let Err(err) = context
        .deps
        .control_store
        .apply_usage_rollup_owned(event)
        .await
    {
        tracing::warn!(
            key_id = %context.key.key_id,
            channel = %route.channel_name,
            error = %err,
            "failed to record direct anthropic upstream usage event"
        );
    }
    let delta = AnthropicUpstreamChannelUsageDelta {
        input_uncached_tokens: usage.input_uncached_tokens.max(0) as u64,
        input_cached_tokens: usage.input_cached_tokens.max(0) as u64,
        output_tokens: usage.output_tokens.max(0) as u64,
        billable_tokens,
        usage_missing: usage.usage_missing,
        used_at_ms,
    };
    if let Err(err) = context
        .deps
        .control_store
        .record_anthropic_upstream_channel_usage(&route.channel_name, delta)
        .await
    {
        tracing::warn!(
            key_id = %context.key.key_id,
            channel = %route.channel_name,
            error = %err,
            "failed to record direct anthropic upstream channel usage"
        );
    }
}

fn parse_billable_multipliers(raw: &str) -> BTreeMap<String, f64> {
    serde_json::from_str(raw).unwrap_or_default()
}

fn build_downstream_response(
    status: StatusCode,
    headers: &HeaderMap,
    body: Bytes,
) -> axum::response::Response {
    let mut builder = Response::builder().status(status);
    for (name, value) in headers {
        if is_hop_by_hop_header(name.as_str()) {
            continue;
        }
        builder = builder.header(name, value);
    }
    builder
        .body(Body::from(body))
        .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
}

fn is_hop_by_hop_header(name: &str) -> bool {
    matches!(
        name,
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Bytes,
        http::{header, HeaderMap, HeaderValue, StatusCode},
    };

    use super::{
        build_route_payload, is_anthropic_event_stream, is_retryable_direct_status,
        observe_anthropic_sse_chunk, observe_anthropic_sse_tail, parse_anthropic_sse_usage,
    };

    #[test]
    fn parses_anthropic_sse_usage_without_double_counting_cache() {
        let body = Bytes::from_static(
            br#"event: message_start
data: {"type":"message_start","message":{"usage":{"input_tokens":10,"cache_creation_input_tokens":4,"cache_read_input_tokens":20,"output_tokens":1}}}

event: message_delta
data: {"type":"message_delta","usage":{"output_tokens":9}}

data: [DONE]
"#,
        );

        let usage = parse_anthropic_sse_usage(&body);

        assert!(!usage.usage_missing);
        assert_eq!(usage.input_uncached_tokens, 14);
        assert_eq!(usage.input_cached_tokens, 20);
        assert_eq!(usage.output_tokens, 9);
    }

    #[test]
    fn parses_anthropic_sse_usage_across_chunk_boundaries() {
        let mut usage = llm_access_anthropic_pool::AnthropicUsageSummary::missing();
        let mut pending_line = String::new();

        observe_anthropic_sse_chunk(
            &Bytes::from_static(br#"data: {"usage":{"input_tokens":7,"#),
            &mut pending_line,
            &mut usage,
        );
        observe_anthropic_sse_chunk(
            &Bytes::from_static(
                br#""cache_read_input_tokens":2,"output_tokens":4}}
data: [DONE]
"#,
            ),
            &mut pending_line,
            &mut usage,
        );
        observe_anthropic_sse_tail(&mut pending_line, &mut usage);

        assert!(!usage.usage_missing);
        assert_eq!(usage.input_uncached_tokens, 7);
        assert_eq!(usage.input_cached_tokens, 2);
        assert_eq!(usage.output_tokens, 4);
    }

    #[test]
    fn detects_event_stream_content_type_without_allocating_header_name() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("Text/Event-Stream; charset=utf-8"),
        );

        assert!(is_anthropic_event_stream(&headers));
    }

    #[test]
    fn classifies_direct_upstream_retryable_statuses() {
        assert!(is_retryable_direct_status(StatusCode::TOO_MANY_REQUESTS));
        assert!(is_retryable_direct_status(StatusCode::INTERNAL_SERVER_ERROR));
        assert!(is_retryable_direct_status(StatusCode::from_u16(529).expect("status")));
        assert!(!is_retryable_direct_status(StatusCode::BAD_REQUEST));
    }

    #[test]
    fn builds_route_payload_with_route_specific_model_mapping() {
        let payload = serde_json::json!({
            "model": "public-model",
            "messages": []
        });

        let (route_a_payload, route_a_model) =
            build_route_payload(r#"{"public-model":"upstream-a"}"#, &payload)
                .expect("route a payload");
        let (route_b_payload, route_b_model) =
            build_route_payload(r#"{"public-model":"upstream-b"}"#, &payload)
                .expect("route b payload");

        assert_eq!(route_a_model.as_deref(), Some("upstream-a"));
        assert_eq!(route_b_model.as_deref(), Some("upstream-b"));
        assert_eq!(route_a_payload["model"], "upstream-a");
        assert_eq!(route_b_payload["model"], "upstream-b");
        assert_eq!(payload["model"], "public-model");
    }
}
