//! Direct standard Anthropic upstream dispatch for Kiro-owned keys.

use std::{
    collections::BTreeMap,
    sync::{LazyLock, Mutex},
    time::Instant,
};

use axum::{
    body::{to_bytes, Body, Bytes},
    http::{header, HeaderMap, Method, Request, Response, StatusCode, Uri, Version},
    response::IntoResponse,
};
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
    client::provider_client,
    kiro_error::kiro_json_error,
    kiro_protocol::normalized_kiro_messages_path,
    limiter::{kiro_key_limit_response, try_acquire_key_permit},
    usage_meta::{
        capture_client_request_body_json, capture_error_bytes, capture_error_message,
        capture_upstream_request_body_json,
    },
    util::{clamp_duration_ms, clamp_u64_to_i64, now_millis},
    ProviderDispatchDeps, ProviderUsageMetadata, MAX_PROVIDER_PROXY_BODY_BYTES,
};

static DIRECT_ANTHROPIC_SCHEDULER: LazyLock<Mutex<SmoothWeightedRoundRobin>> =
    LazyLock::new(|| Mutex::new(SmoothWeightedRoundRobin::default()));

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
    mapped_model: Option<&'a str>,
    request_headers: &'a HeaderMap,
    deps: &'a ProviderDispatchDeps,
}

pub(super) async fn maybe_dispatch_anthropic_upstream_pool(
    key: AuthenticatedKey,
    request: Request<Body>,
    deps: ProviderDispatchDeps,
) -> AnthropicUpstreamDispatchOutcome {
    let mode = match deps
        .route_store
        .resolve_anthropic_upstream_pool_mode(&key)
        .await
    {
        Ok(mode) => mode,
        Err(err) => {
            tracing::warn!(
                key_id = %key.key_id,
                error = %err,
                "direct anthropic upstream pool mode resolution failed"
            );
            return AnthropicUpstreamDispatchOutcome::Fallback(request);
        },
    };
    if mode == core_store::ANTHROPIC_UPSTREAM_POOL_MODE_DISABLED {
        return AnthropicUpstreamDispatchOutcome::Fallback(request);
    }

    let routes = match deps
        .route_store
        .resolve_anthropic_upstream_route_candidates(&key)
        .await
    {
        Ok(routes) => routes,
        Err(err) => {
            tracing::warn!(
                key_id = %key.key_id,
                error = %err,
                "direct anthropic upstream route resolution failed"
            );
            return if mode == core_store::ANTHROPIC_UPSTREAM_POOL_MODE_ONLY {
                AnthropicUpstreamDispatchOutcome::Handled(kiro_json_error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "api_error",
                    "direct Anthropic upstream route resolution failed",
                ))
            } else {
                AnthropicUpstreamDispatchOutcome::Fallback(request)
            };
        },
    };
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
    let mut payload = match serde_json::from_slice::<Value>(&replay.body) {
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
    let mapped_model =
        match apply_model_mapping_to_json(&routes[0].model_name_map_json, &mut payload) {
            Ok(mapping) => mapping,
            Err(err) => {
                tracing::warn!(
                    key_id = %key.key_id,
                    error = %err,
                    "direct anthropic model mapping failed"
                );
                return AnthropicUpstreamDispatchOutcome::Handled(kiro_json_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "api_error",
                    "model mapping failed",
                ));
            },
        };
    usage_meta.mark_pre_handler_done(clamp_duration_ms(parse_started.elapsed()));
    let upstream_body = match serde_json::to_vec(&payload) {
        Ok(body) => Bytes::from(body),
        Err(err) => {
            tracing::warn!(
                key_id = %key.key_id,
                error = %err,
                "direct anthropic upstream body serialization failed"
            );
            return AnthropicUpstreamDispatchOutcome::Handled(kiro_json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "api_error",
                "request serialization failed",
            ));
        },
    };
    capture_upstream_request_body_json(&mut usage_meta, &upstream_body);

    let context = DirectAnthropicDispatchContext {
        key: &key,
        endpoint: public_path,
        model: &original_model,
        mapped_model: mapped_model.as_deref(),
        request_headers: &replay.headers,
        deps: &deps,
    };
    let mut remaining_routes = routes;
    let mut last_failure: Option<axum::response::Response> = None;
    while let Some(route) = select_weighted_route(&mut remaining_routes) {
        let response =
            dispatch_one_route(&context, &route, upstream_body.clone(), &mut usage_meta).await;
        let retryable = response.status().is_server_error()
            || response.status() == StatusCode::TOO_MANY_REQUESTS
            || response.status().as_u16() == 529;
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

fn select_weighted_route(
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
    let selected_name = DIRECT_ANTHROPIC_SCHEDULER
        .lock()
        .expect("direct anthropic scheduler lock")
        .select(&weighted)
        .map(str::to_string);
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
    let Some(model) = payload
        .get("model")
        .and_then(Value::as_str)
        .map(ToString::to_string)
    else {
        return Ok(None);
    };
    let Some(target) = map.get(&model).cloned() else {
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

async fn dispatch_one_route(
    context: &DirectAnthropicDispatchContext<'_>,
    route: &ProviderAnthropicUpstreamRoute,
    upstream_body: Bytes,
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
    let client = match provider_client(route.proxy.as_ref()) {
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
    let mut request = client
        .post(upstream_url)
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-api-key", &route.api_key)
        .body(upstream_body);
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
                context,
                route,
                status,
                AnthropicUsageSummary::missing(),
                usage_meta,
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
    let body = match response.bytes().await {
        Ok(body) => body,
        Err(err) => {
            capture_error_message(usage_meta, &err.to_string());
            usage_meta.capture_error_class("upstream_body_error");
            let status = StatusCode::BAD_GATEWAY;
            record_direct_usage(
                context,
                route,
                status,
                AnthropicUsageSummary::missing(),
                usage_meta,
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
    record_direct_usage(context, route, status, usage, usage_meta).await;
    build_downstream_response(status, &headers, body)
}

fn parse_anthropic_response_usage(headers: &HeaderMap, body: &Bytes) -> AnthropicUsageSummary {
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if content_type.contains("text/event-stream") {
        return parse_anthropic_sse_usage(body);
    }
    serde_json::from_slice::<serde_json::Value>(body)
        .map(|value| parse_usage_from_value(&value))
        .unwrap_or_else(|_| AnthropicUsageSummary::missing())
}

fn parse_anthropic_sse_usage(body: &Bytes) -> AnthropicUsageSummary {
    let text = String::from_utf8_lossy(body);
    let mut usage = AnthropicUsageSummary::missing();
    for line in text.lines() {
        let Some(data) = line.trim_start().strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(data) else {
            continue;
        };
        usage = merge_usage(usage, parse_usage_from_value(&value));
    }
    usage
}

async fn record_direct_usage(
    context: &DirectAnthropicDispatchContext<'_>,
    route: &ProviderAnthropicUpstreamRoute,
    status: StatusCode,
    usage: AnthropicUsageSummary,
    meta: &ProviderUsageMetadata,
) {
    let billable_tokens = if usage.usage_missing {
        0
    } else {
        core_store::compute_kiro_billable_tokens(
            Some(context.mapped_model.unwrap_or(context.model)),
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
        endpoint: context.endpoint.to_string(),
        model: Some(context.model.to_string()),
        mapped_model: context.mapped_model.map(ToString::to_string),
        status_code: status.as_u16() as i64,
        request_body_bytes: meta.request_body_bytes,
        quota_failover_count: meta.quota_failover_count,
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
        client_request_body_json: None,
        upstream_request_body_json: None,
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
        name.to_ascii_lowercase().as_str(),
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
    use axum::body::Bytes;

    use super::parse_anthropic_sse_usage;

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
}
