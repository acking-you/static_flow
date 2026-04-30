//! Provider-facing HTTP entrypoints for `llm-access`.

use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use async_stream::stream;
use async_trait::async_trait;
use axum::{
    body::{to_bytes, Body, Bytes},
    extract::State,
    http::{header, HeaderMap, Method, Request, StatusCode},
    response::{IntoResponse, Response},
};
use eventsource_stream::Eventsource;
use futures_util::{StreamExt, TryStreamExt};
use llm_access_codex::{
    request::{apply_gpt53_codex_spark_mapping, prepare_gateway_request_from_bytes},
    response::{
        adapt_completed_response_json, apply_upstream_response_headers,
        convert_json_response_to_chat_completion, convert_response_event_to_chat_chunk,
        encode_json_sse_chunk, encode_sse_event_with_model_alias, extract_usage_from_bytes,
        rewrite_json_response_model_alias, SseUsageCollector,
    },
    types::{ChatStreamMetadata, GatewayResponseAdapter, PreparedGatewayRequest, UsageBreakdown},
};
use llm_access_core::{
    provider::{ProtocolFamily, ProviderType},
    routes::provider_route_requirement,
    store::{
        AuthenticatedKey, ControlStore, ProviderKiroRoute, ProviderProxyConfig, ProviderRouteStore,
    },
    usage::{UsageEvent, UsageTiming},
};
use llm_access_kiro::{
    anthropic::{
        converter::{
            convert_normalized_request_with_resolved_session, normalize_request,
            preview_session_value, resolve_conversation_id_from_metadata, ConversionError,
            ResolvedConversationId, SessionFallbackReason, SessionIdSource, SessionTracking,
        },
        stream::{
            anthropic_usage_json, build_inline_thinking_content_blocks, resolve_input_tokens,
            BufferedStreamContext, StreamContext,
        },
        types::{MessagesRequest, OutputConfig, Thinking},
    },
    cache_policy::{
        adjust_input_tokens_for_cache_creation_cost_with_policy, default_kiro_cache_policy,
        prefix_tree_credit_ratio_cap_basis_points_with_policy, validate_kiro_cache_policy,
        KiroCachePolicy,
    },
    cache_sim::{
        KiroCacheSimulationConfig, KiroCacheSimulationMode, KiroCacheSimulator, PromptProjection,
    },
    parser::decoder::EventStreamDecoder,
    token,
    wire::{Event, KiroRequest},
};
use serde_json::Value;

const MAX_PROVIDER_PROXY_BODY_BYTES: usize = 32 * 1024 * 1024;

/// Shared provider request state.
#[derive(Clone)]
pub struct ProviderState {
    control_store: Arc<dyn ControlStore>,
    route_store: Arc<dyn ProviderRouteStore>,
    dispatcher: Arc<dyn ProviderDispatcher>,
    kiro_cache_simulator: Arc<KiroCacheSimulator>,
    request_limiter: Arc<RequestLimiter>,
}

impl ProviderState {
    /// Create provider request state.
    pub fn new(
        control_store: Arc<dyn ControlStore>,
        route_store: Arc<dyn ProviderRouteStore>,
    ) -> Self {
        Self::with_dispatcher(control_store, route_store, Arc::new(DefaultProviderDispatcher))
    }

    /// Create provider request state with an explicit dispatcher.
    pub fn with_dispatcher(
        control_store: Arc<dyn ControlStore>,
        route_store: Arc<dyn ProviderRouteStore>,
        dispatcher: Arc<dyn ProviderDispatcher>,
    ) -> Self {
        Self {
            control_store,
            route_store,
            dispatcher,
            kiro_cache_simulator: Arc::new(KiroCacheSimulator::default()),
            request_limiter: Arc::new(RequestLimiter::default()),
        }
    }
}

/// In-process request limiter for authenticated provider requests.
#[derive(Default)]
pub struct RequestLimiter {
    scopes: Mutex<HashMap<String, LimitScope>>,
}

#[derive(Default)]
struct LimitScope {
    in_flight: u64,
    last_start: Option<Instant>,
}

struct LimitPermit {
    limiter: Arc<RequestLimiter>,
    scope: String,
}

#[derive(Debug, Clone, Copy, Default)]
struct RouteLimitSpec {
    key_max_concurrency: Option<u64>,
    key_min_start_interval_ms: Option<u64>,
    account_max_concurrency: Option<u64>,
    account_min_start_interval_ms: Option<u64>,
}

impl Drop for LimitPermit {
    fn drop(&mut self) {
        let Ok(mut scopes) = self.limiter.scopes.lock() else {
            return;
        };
        if let Some(scope) = scopes.get_mut(&self.scope) {
            scope.in_flight = scope.in_flight.saturating_sub(1);
        }
    }
}

impl RequestLimiter {
    async fn acquire(
        self: &Arc<Self>,
        scope: String,
        max_concurrency: Option<u64>,
        min_start_interval_ms: Option<u64>,
    ) -> LimitPermit {
        let max_concurrency = max_concurrency.filter(|value| *value > 0);
        let min_interval = min_start_interval_ms
            .filter(|value| *value > 0)
            .map(Duration::from_millis);
        loop {
            let wait = {
                let mut scopes = self.scopes.lock().expect("request limiter mutex poisoned");
                let state = scopes.entry(scope.clone()).or_default();
                let concurrency_ready = max_concurrency
                    .map(|limit| state.in_flight < limit)
                    .unwrap_or(true);
                let interval_wait = min_interval.and_then(|interval| {
                    state
                        .last_start
                        .and_then(|last_start| interval.checked_sub(last_start.elapsed()))
                });
                if concurrency_ready && interval_wait.is_none() {
                    state.in_flight = state.in_flight.saturating_add(1);
                    state.last_start = Some(Instant::now());
                    None
                } else {
                    Some(interval_wait.unwrap_or_else(|| Duration::from_millis(10)))
                }
            };
            if let Some(wait) = wait {
                tokio::time::sleep(wait).await;
            } else {
                return LimitPermit {
                    limiter: Arc::clone(self),
                    scope,
                };
            }
        }
    }
}

async fn acquire_route_permits(
    limiter: Arc<RequestLimiter>,
    provider: ProviderType,
    key: &AuthenticatedKey,
    account_name: &str,
    limits: RouteLimitSpec,
) -> Vec<LimitPermit> {
    let key_scope = format!("key:{}", key.key_id);
    let account_scope = format!("account:{}:{account_name}", provider.as_storage_str());
    let key_permit = limiter
        .acquire(key_scope, limits.key_max_concurrency, limits.key_min_start_interval_ms)
        .await;
    let account_permit = limiter
        .acquire(
            account_scope,
            limits.account_max_concurrency,
            limits.account_min_start_interval_ms,
        )
        .await;
    vec![key_permit, account_permit]
}

/// Provider runtime dispatch after key authentication succeeds.
#[async_trait]
pub trait ProviderDispatcher: Send + Sync {
    /// Dispatch an authenticated request to the selected provider runtime.
    async fn dispatch(
        &self,
        key: AuthenticatedKey,
        request: Request<Body>,
        route_store: Arc<dyn ProviderRouteStore>,
        control_store: Arc<dyn ControlStore>,
        kiro_cache_simulator: Arc<KiroCacheSimulator>,
        request_limiter: Arc<RequestLimiter>,
    ) -> Response;
}

struct DefaultProviderDispatcher;

#[async_trait]
impl ProviderDispatcher for DefaultProviderDispatcher {
    async fn dispatch(
        &self,
        key: AuthenticatedKey,
        request: Request<Body>,
        route_store: Arc<dyn ProviderRouteStore>,
        control_store: Arc<dyn ControlStore>,
        kiro_cache_simulator: Arc<KiroCacheSimulator>,
        request_limiter: Arc<RequestLimiter>,
    ) -> Response {
        if should_serve_local_codex_models(&key, &request) {
            return local_codex_models_response();
        }
        if ProviderType::from_storage_str(&key.provider_type) == Some(ProviderType::Codex) {
            return dispatch_codex_proxy(key, request, route_store, control_store, request_limiter)
                .await;
        }
        if ProviderType::from_storage_str(&key.provider_type) == Some(ProviderType::Kiro) {
            return dispatch_kiro_proxy(
                key,
                request,
                route_store,
                control_store,
                kiro_cache_simulator,
                request_limiter,
            )
            .await;
        }
        (StatusCode::NOT_IMPLEMENTED, "provider dispatch is not wired").into_response()
    }
}

async fn dispatch_codex_proxy(
    key: AuthenticatedKey,
    request: Request<Body>,
    route_store: Arc<dyn ProviderRouteStore>,
    control_store: Arc<dyn ControlStore>,
    request_limiter: Arc<RequestLimiter>,
) -> Response {
    let route = match route_store.resolve_codex_route(&key).await {
        Ok(Some(route)) => route,
        Ok(None) => {
            return (StatusCode::SERVICE_UNAVAILABLE, "codex route is not configured")
                .into_response()
        },
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "codex route resolution failed")
                .into_response()
        },
    };
    let Some(access_token) = codex_access_token_from_auth_json(&route.auth_json) else {
        return (StatusCode::SERVICE_UNAVAILABLE, "codex account auth is missing access_token")
            .into_response();
    };
    let Some(gateway_path) =
        normalized_codex_gateway_path(request.uri().path()).map(str::to_string)
    else {
        return (StatusCode::NOT_FOUND, "unsupported codex gateway endpoint").into_response();
    };
    let query = request
        .uri()
        .query()
        .map(|query| format!("?{query}"))
        .unwrap_or_default();
    let upstream_base = std::env::var("CODEX_UPSTREAM_BASE_URL")
        .map(|value| llm_access_codex::request::normalize_upstream_base_url(&value))
        .unwrap_or_else(|_| "https://chatgpt.com/backend-api/codex".to_string());
    let method = request.method().clone();
    let request_headers = request.headers().clone();
    let body = match to_bytes(request.into_body(), MAX_PROVIDER_PROXY_BODY_BYTES).await {
        Ok(body) => body,
        Err(_) => return (StatusCode::BAD_REQUEST, "request body is too large").into_response(),
    };
    let prepared = match prepare_gateway_request_from_bytes(
        &gateway_path,
        &query,
        method,
        &request_headers,
        body,
        MAX_PROVIDER_PROXY_BODY_BYTES,
    ) {
        Ok(prepared) => prepared,
        Err(err) => return (err.status, err.message).into_response(),
    };
    let prepared = match apply_gpt53_codex_spark_mapping(&prepared, route.map_gpt53_codex_to_spark)
    {
        Ok(prepared) => prepared,
        Err(err) => return (err.status, err.message).into_response(),
    };
    let upstream_url = format!("{}{}", upstream_base.trim_end_matches('/'), prepared.upstream_path);
    let method = match reqwest::Method::from_bytes(prepared.method.as_str().as_bytes()) {
        Ok(method) => method,
        Err(_) => return (StatusCode::METHOD_NOT_ALLOWED, "unsupported method").into_response(),
    };
    let permits = acquire_route_permits(
        request_limiter,
        ProviderType::Codex,
        &key,
        &route.account_name,
        RouteLimitSpec {
            key_max_concurrency: route.request_max_concurrency,
            key_min_start_interval_ms: route.request_min_start_interval_ms,
            account_max_concurrency: route.account_request_max_concurrency,
            account_min_start_interval_ms: route.account_request_min_start_interval_ms,
        },
    )
    .await;
    let client = match provider_client(route.proxy.as_ref()) {
        Ok(client) => client,
        Err(_) => {
            return (StatusCode::SERVICE_UNAVAILABLE, "failed to build upstream HTTP client")
                .into_response()
        },
    };
    let mut upstream = client
        .request(method, upstream_url)
        .bearer_auth(access_token)
        .header(
            reqwest::header::ACCEPT,
            if prepared.wants_stream || prepared.force_upstream_stream {
                "text/event-stream"
            } else {
                "application/json"
            },
        );
    if !prepared.request_body.is_empty() {
        upstream = upstream
            .header(reqwest::header::CONTENT_TYPE, prepared.content_type.as_str())
            .body(prepared.request_body.clone());
    }
    for name in [header::ACCEPT_LANGUAGE, header::USER_AGENT] {
        if let Some(value) = request_headers.get(&name) {
            upstream = upstream.header(name.as_str(), value.as_bytes());
        }
    }
    let response = match upstream.send().await {
        Ok(response) => response,
        Err(_) => {
            return (StatusCode::BAD_GATEWAY, "codex upstream request failed").into_response()
        },
    };
    adapt_codex_upstream_response(
        prepared,
        response,
        key,
        route.account_name,
        control_store,
        permits,
    )
    .await
}

async fn adapt_codex_upstream_response(
    prepared: PreparedGatewayRequest,
    response: reqwest::Response,
    key: AuthenticatedKey,
    account_name: String,
    control_store: Arc<dyn ControlStore>,
    permits: Vec<LimitPermit>,
) -> Response {
    let status = response.status();
    let upstream_headers = response.headers().clone();
    let content_type = upstream_headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/json")
        .to_string();
    let expects_sse = status.is_success()
        && (content_type.contains("text/event-stream")
            || prepared.wants_stream
            || prepared.force_upstream_stream);

    if expects_sse && prepared.force_upstream_stream && !prepared.wants_stream {
        let bytes = match response.bytes().await {
            Ok(bytes) => bytes,
            Err(_) => {
                return (StatusCode::BAD_GATEWAY, "codex upstream response read failed")
                    .into_response()
            },
        };
        let completed = match completed_response_from_sse_bytes(&bytes) {
            Ok(value) => value,
            Err(err) => return (StatusCode::BAD_GATEWAY, err).into_response(),
        };
        let completed_response = rewrite_json_value_model_alias(
            completed.response,
            prepared.model.as_deref(),
            prepared.client_visible_model.as_deref(),
        );
        let adapted = adapt_completed_response_json(
            &completed_response,
            prepared.response_adapter,
            Some(&prepared.tool_name_restore_map),
        );
        let body = match serde_json::to_vec(&adapted) {
            Ok(body) => body,
            Err(_) => {
                return (StatusCode::BAD_GATEWAY, "codex upstream response adaptation failed")
                    .into_response()
            },
        };
        if let Err(err) = record_codex_usage(
            control_store.as_ref(),
            &key,
            &prepared,
            status,
            &account_name,
            completed.usage.unwrap_or_else(missing_codex_usage),
        )
        .await
        {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to record codex usage: {err}"),
            )
                .into_response();
        }
        let builder = Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::CACHE_CONTROL, "no-store");
        return apply_upstream_response_headers(builder, &upstream_headers)
            .body(Body::from(body))
            .unwrap_or_else(|_| {
                (StatusCode::BAD_GATEWAY, "codex upstream response build failed").into_response()
            });
    }

    if expects_sse {
        return stream_codex_upstream_response(
            response,
            status,
            upstream_headers,
            content_type,
            CodexStreamContext {
                prepared,
                key,
                account_name,
                control_store,
                permits,
            },
        );
    }

    let bytes = match response.bytes().await {
        Ok(bytes) => bytes,
        Err(_) => {
            return (StatusCode::BAD_GATEWAY, "codex upstream response read failed").into_response()
        },
    };
    let response_body = if status.is_success()
        && prepared.response_adapter == GatewayResponseAdapter::ChatCompletions
    {
        match convert_json_response_to_chat_completion(
            &bytes,
            Some(&prepared.tool_name_restore_map),
            prepared.model.as_deref(),
            prepared.client_visible_model.as_deref(),
        ) {
            Ok(body) => body,
            Err(err) => return (StatusCode::BAD_GATEWAY, err).into_response(),
        }
    } else {
        rewrite_json_response_model_alias(
            &bytes,
            prepared.model.as_deref(),
            prepared.client_visible_model.as_deref(),
        )
        .unwrap_or_else(|| bytes.to_vec())
    };
    if status.is_success() {
        if let Err(err) = record_codex_usage(
            control_store.as_ref(),
            &key,
            &prepared,
            status,
            &account_name,
            extract_usage_from_bytes(&bytes).unwrap_or_else(missing_codex_usage),
        )
        .await
        {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to record codex usage: {err}"),
            )
                .into_response();
        }
    }
    let response_content_type = if status.is_success()
        && prepared.response_adapter == GatewayResponseAdapter::ChatCompletions
    {
        "application/json"
    } else {
        &content_type
    };
    let builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, response_content_type)
        .header(header::CACHE_CONTROL, "no-store");
    apply_upstream_response_headers(builder, &upstream_headers)
        .body(Body::from(response_body))
        .unwrap_or_else(|_| {
            (StatusCode::BAD_GATEWAY, "codex upstream response build failed").into_response()
        })
}

struct CodexStreamContext {
    prepared: PreparedGatewayRequest,
    key: AuthenticatedKey,
    account_name: String,
    control_store: Arc<dyn ControlStore>,
    permits: Vec<LimitPermit>,
}

fn stream_codex_upstream_response(
    response: reqwest::Response,
    status: StatusCode,
    upstream_headers: reqwest::header::HeaderMap,
    content_type: String,
    ctx: CodexStreamContext,
) -> Response {
    let response_adapter = ctx.prepared.response_adapter;
    let body_stream = stream! {
        let CodexStreamContext {
            prepared,
            key,
            account_name,
            control_store,
            permits,
        } = ctx;
        let _permits = permits;
        let mut events = response
            .bytes_stream()
            .map_err(std::io::Error::other)
            .eventsource();
        let mut chat_metadata = ChatStreamMetadata::default();
        let mut usage_collector = SseUsageCollector::default();
        while let Some(event) = events.next().await {
            match event {
                Ok(event) => {
                    usage_collector.observe_event(&event);
                    match response_adapter {
                        GatewayResponseAdapter::Responses => {
                            yield Ok::<Bytes, std::io::Error>(encode_sse_event_with_model_alias(
                                &event,
                                prepared.model.as_deref(),
                                prepared.client_visible_model.as_deref(),
                            ));
                        },
                        GatewayResponseAdapter::ChatCompletions => {
                            if let Some(chunk) = convert_response_event_to_chat_chunk(
                                &event,
                                Some(&prepared.tool_name_restore_map),
                                &mut chat_metadata,
                                prepared.model.as_deref(),
                                prepared.client_visible_model.as_deref(),
                            ) {
                                yield Ok::<Bytes, std::io::Error>(encode_json_sse_chunk(&chunk));
                            }
                        },
                    }
                },
                Err(err) => {
                    yield Err(std::io::Error::other(format!(
                        "failed to parse codex upstream SSE event: {err}"
                    )));
                    return;
                },
            }
        }
        if response_adapter == GatewayResponseAdapter::ChatCompletions {
            yield Ok::<Bytes, std::io::Error>(Bytes::from_static(b"data: [DONE]\n\n"));
        }
        if let Err(err) = record_codex_usage(
            control_store.as_ref(),
            &key,
            &prepared,
            status,
            &account_name,
            usage_collector.usage.unwrap_or_else(missing_codex_usage),
        ).await {
            yield Err(std::io::Error::other(format!("failed to record codex usage: {err}")));
        }
    };
    let response_content_type = if response_adapter == GatewayResponseAdapter::ChatCompletions {
        "text/event-stream"
    } else {
        content_type.as_str()
    };
    let builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, response_content_type)
        .header(header::CACHE_CONTROL, "no-store");
    apply_upstream_response_headers(builder, &upstream_headers)
        .body(Body::from_stream(body_stream))
        .unwrap_or_else(|_| {
            (StatusCode::BAD_GATEWAY, "codex upstream stream response build failed").into_response()
        })
}

async fn dispatch_kiro_proxy(
    key: AuthenticatedKey,
    request: Request<Body>,
    route_store: Arc<dyn ProviderRouteStore>,
    control_store: Arc<dyn ControlStore>,
    kiro_cache_simulator: Arc<KiroCacheSimulator>,
    request_limiter: Arc<RequestLimiter>,
) -> Response {
    let route = match route_store.resolve_kiro_route(&key).await {
        Ok(Some(route)) => route,
        Ok(None) => {
            return (StatusCode::SERVICE_UNAVAILABLE, "kiro route is not configured")
                .into_response()
        },
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "kiro route resolution failed")
                .into_response()
        },
    };
    let Some(access_token) = kiro_access_token_from_auth_json(&route.auth_json) else {
        return (StatusCode::SERVICE_UNAVAILABLE, "kiro account auth is missing access token")
            .into_response();
    };
    let Some((public_path, buffered_for_cc)) = normalized_kiro_messages_path(request.uri().path())
    else {
        return (StatusCode::NOT_FOUND, "unsupported kiro gateway endpoint").into_response();
    };
    if request.method() != Method::POST {
        return (StatusCode::METHOD_NOT_ALLOWED, "unsupported kiro method").into_response();
    }

    let request_headers = request.headers().clone();
    let body = match to_bytes(request.into_body(), MAX_PROVIDER_PROXY_BODY_BYTES).await {
        Ok(body) => body,
        Err(_) => {
            return kiro_json_error(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "request body is too large",
            )
        },
    };
    let mut payload = match serde_json::from_slice::<MessagesRequest>(&body) {
        Ok(payload) => payload,
        Err(err) => {
            return kiro_json_error(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                &format!("failed to parse request JSON: {err}"),
            )
        },
    };
    let request_input_tokens = token::count_all_tokens(
        payload.model.clone(),
        payload.system.clone(),
        payload.messages.clone(),
        payload.tools.clone(),
    ) as i32;
    override_kiro_thinking_from_model_name(&mut payload);
    let requested_model = payload.model.clone();
    let normalized = match normalize_request(&payload) {
        Ok(normalized) => normalized,
        Err(err) => return kiro_conversion_error_response(err),
    };
    let resolved_session =
        resolve_kiro_request_session(&request_headers, payload.metadata.as_ref());
    let conversion = match convert_normalized_request_with_resolved_session(
        normalized,
        route.request_validation_enabled,
        resolved_session,
    ) {
        Ok(conversion) => conversion,
        Err(err) => return kiro_conversion_error_response(err),
    };
    let thinking_enabled = payload
        .thinking
        .as_ref()
        .is_some_and(|thinking| thinking.is_enabled());
    let mut conversation_state = conversion.conversation_state;
    let mut cache_ctx =
        match build_kiro_cache_context(&route, &conversation_state, &kiro_cache_simulator) {
            Ok(context) => context,
            Err(err) => {
                return kiro_json_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "api_error",
                    &format!("Kiro cache configuration is invalid: {err}"),
                )
            },
        };
    if matches!(conversion.session_tracking.source, SessionIdSource::GeneratedFallback(_)) {
        if let Some(recovered) = kiro_cache_simulator.recover_conversation_id(
            &cache_ctx.projection,
            cache_ctx.simulation_config,
            Instant::now(),
        ) {
            conversation_state.conversation_id = recovered.clone();
            cache_ctx.conversation_id = recovered;
        }
    }

    let request_body = match serde_json::to_vec(&KiroRequest {
        conversation_state,
        profile_arn: route.profile_arn.clone(),
    }) {
        Ok(body) => body,
        Err(_) => {
            return kiro_json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "api_error",
                "failed to encode kiro request",
            )
        },
    };
    let upstream_url = format!(
        "{}/generateAssistantResponse",
        std::env::var("KIRO_UPSTREAM_BASE_URL")
            .map(|value| value.trim_end_matches('/').to_string())
            .unwrap_or_else(|_| format!("https://q.{}.amazonaws.com", route.api_region))
    );
    let permits = acquire_route_permits(
        request_limiter,
        ProviderType::Kiro,
        &key,
        &route.account_name,
        RouteLimitSpec {
            key_max_concurrency: route.request_max_concurrency,
            key_min_start_interval_ms: route.request_min_start_interval_ms,
            account_max_concurrency: route.account_request_max_concurrency,
            account_min_start_interval_ms: route.account_request_min_start_interval_ms,
        },
    )
    .await;
    let client = match provider_client(route.proxy.as_ref()) {
        Ok(client) => client,
        Err(_) => {
            return kiro_json_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "api_error",
                "failed to build upstream HTTP client",
            )
        },
    };
    let response = match client
        .post(upstream_url)
        .bearer_auth(access_token)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(reqwest::header::ACCEPT, "application/vnd.amazon.eventstream")
        .header("x-amzn-codewhisperer-optout", "true")
        .header("x-amzn-kiro-agent-mode", "vibe")
        .header("amz-sdk-invocation-id", uuid::Uuid::new_v4().to_string())
        .header("amz-sdk-request", "attempt=1; max=3")
        .body(request_body)
        .send()
        .await
    {
        Ok(response) => response,
        Err(_) => {
            return kiro_json_error(
                StatusCode::BAD_GATEWAY,
                "api_error",
                "kiro upstream request failed",
            )
        },
    };

    if !response.status().is_success() {
        return pass_through_kiro_error_response(response).await;
    }
    let response_ctx = KiroResponseContext {
        key,
        route,
        public_path: public_path.to_string(),
        model: requested_model,
        request_input_tokens,
        thinking_enabled,
        tool_name_map: conversion.tool_name_map,
        structured_output_tool_name: conversion.structured_output_tool_name,
        cache_ctx,
        control_store,
        kiro_cache_simulator,
        _permits: permits,
    };
    if payload.stream {
        if buffered_for_cc {
            return buffered_kiro_stream_response(response, response_ctx).await;
        }
        return stream_kiro_upstream_response(response, response_ctx);
    }

    non_stream_kiro_response(response, response_ctx).await
}

struct KiroResponseContext {
    key: AuthenticatedKey,
    route: ProviderKiroRoute,
    public_path: String,
    model: String,
    request_input_tokens: i32,
    thinking_enabled: bool,
    tool_name_map: std::collections::HashMap<String, String>,
    structured_output_tool_name: Option<String>,
    cache_ctx: KiroCacheContext,
    control_store: Arc<dyn ControlStore>,
    kiro_cache_simulator: Arc<KiroCacheSimulator>,
    _permits: Vec<LimitPermit>,
}

#[derive(Clone)]
struct KiroCacheContext {
    policy: KiroCachePolicy,
    simulation_config: KiroCacheSimulationConfig,
    projection: PromptProjection,
    prefix_cache_match: llm_access_kiro::cache_sim::PrefixCacheMatch,
    conversation_id: String,
    cache_kmodels: BTreeMap<String, f64>,
    billable_model_multipliers: BTreeMap<String, f64>,
}

fn stream_kiro_upstream_response(
    response: reqwest::Response,
    ctx: KiroResponseContext,
) -> Response {
    let status = response.status();
    let body_stream = stream! {
        let mut stream_ctx = StreamContext::new_with_thinking(
            &ctx.model,
            ctx.request_input_tokens,
            ctx.thinking_enabled,
            ctx.tool_name_map,
            ctx.structured_output_tool_name.clone(),
        );
        for event in stream_ctx.generate_initial_events() {
            yield Ok::<Bytes, std::io::Error>(Bytes::from(event.to_sse_string()));
        }
        let mut body_stream = response.bytes_stream();
        let mut decoder = EventStreamDecoder::new();
        while let Some(chunk_result) = body_stream.next().await {
            let chunk = match chunk_result {
                Ok(chunk) => chunk,
                Err(err) => {
                    yield Err(std::io::Error::other(format!("failed to read kiro upstream stream: {err}")));
                    return;
                },
            };
            let _ = decoder.feed(&chunk);
            for frame in decoder.decode_iter() {
                let frame = match frame {
                    Ok(frame) => frame,
                    Err(err) => {
                        yield Err(std::io::Error::other(format!("failed to decode kiro event frame: {err}")));
                        return;
                    },
                };
                let event = match Event::from_frame(frame) {
                    Ok(event) => event,
                    Err(err) => {
                        yield Err(std::io::Error::other(format!("failed to parse kiro event: {err}")));
                        return;
                    },
                };
                for sse_event in stream_ctx.process_kiro_event(&event) {
                    yield Ok::<Bytes, std::io::Error>(Bytes::from(sse_event.to_sse_string()));
                }
            }
        }
        let (input_tokens, output_tokens) = stream_ctx.final_usage();
        let (credit_usage, credit_usage_missing) = stream_ctx.final_credit_usage();
        let usage = build_kiro_usage_summary(
            &ctx.model,
            KiroUsageInputs {
                request_input_tokens: ctx.request_input_tokens,
                context_input_tokens: Some(input_tokens),
                output_tokens,
                credit_usage,
                credit_usage_missing,
                cache_estimation_enabled: ctx.route.cache_estimation_enabled,
            },
            &ctx.cache_ctx,
        );
        let mut final_events = stream_ctx.generate_final_events();
        let anthropic_usage = anthropic_usage_json(
            usage.input_uncached_tokens + usage.input_cached_tokens,
            usage.output_tokens,
            usage.input_cached_tokens,
        );
        for event in &mut final_events {
            if event.event == "message_delta" {
                if let Some(value) = event.data.get_mut("usage") {
                    *value = anthropic_usage.clone();
                }
            }
        }
        let assistant_message = stream_ctx.final_assistant_message();
        ctx.kiro_cache_simulator.record_success(
            &ctx.cache_ctx.projection,
            &assistant_message,
            &ctx.cache_ctx.conversation_id,
            ctx.route.cache_estimation_enabled,
            ctx.cache_ctx.simulation_config,
            Instant::now(),
        );
        if let Err(err) = record_kiro_usage(KiroUsageRecord {
            control_store: ctx.control_store.as_ref(),
            key: &ctx.key,
            route: &ctx.route,
            endpoint: &ctx.public_path,
            model: &ctx.model,
            status,
            usage,
            cache_ctx: &ctx.cache_ctx,
        }).await {
            yield Err(std::io::Error::other(format!("failed to record kiro usage: {err}")));
            return;
        }
        for event in final_events {
            yield Ok::<Bytes, std::io::Error>(Bytes::from(event.to_sse_string()));
        }
    };
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONNECTION, "keep-alive")
        .body(Body::from_stream(body_stream))
        .unwrap_or_else(|_| {
            (StatusCode::BAD_GATEWAY, "kiro stream response build failed").into_response()
        })
}

async fn buffered_kiro_stream_response(
    response: reqwest::Response,
    ctx: KiroResponseContext,
) -> Response {
    let status = response.status();
    let bytes = match response.bytes().await {
        Ok(bytes) => bytes,
        Err(_) => {
            return kiro_json_error(
                StatusCode::BAD_GATEWAY,
                "api_error",
                "failed to read kiro upstream response",
            )
        },
    };
    let events = match decode_kiro_events_from_bytes(&bytes) {
        Ok(events) => events,
        Err(err) => return kiro_json_error(StatusCode::BAD_GATEWAY, "api_error", &err),
    };
    let mut stream_ctx = BufferedStreamContext::new(
        &ctx.model,
        ctx.request_input_tokens,
        ctx.thinking_enabled,
        ctx.tool_name_map,
        ctx.structured_output_tool_name.clone(),
    );
    for event in &events {
        stream_ctx.process_and_buffer(event);
    }
    let (input_tokens, output_tokens) = stream_ctx.final_usage();
    let (credit_usage, credit_usage_missing) = stream_ctx.final_credit_usage();
    let usage = build_kiro_usage_summary(
        &ctx.model,
        KiroUsageInputs {
            request_input_tokens: ctx.request_input_tokens,
            context_input_tokens: Some(input_tokens),
            output_tokens,
            credit_usage,
            credit_usage_missing,
            cache_estimation_enabled: ctx.route.cache_estimation_enabled,
        },
        &ctx.cache_ctx,
    );
    let mut sse_events = stream_ctx.finish_and_get_all_events();
    let anthropic_usage = anthropic_usage_json(
        usage.input_uncached_tokens + usage.input_cached_tokens,
        usage.output_tokens,
        usage.input_cached_tokens,
    );
    for event in &mut sse_events {
        if event.event == "message_delta" {
            if let Some(value) = event.data.get_mut("usage") {
                *value = anthropic_usage.clone();
            }
        }
    }
    let assistant_message = stream_ctx.final_assistant_message();
    ctx.kiro_cache_simulator.record_success(
        &ctx.cache_ctx.projection,
        &assistant_message,
        &ctx.cache_ctx.conversation_id,
        ctx.route.cache_estimation_enabled,
        ctx.cache_ctx.simulation_config,
        Instant::now(),
    );
    if let Err(err) = record_kiro_usage(KiroUsageRecord {
        control_store: ctx.control_store.as_ref(),
        key: &ctx.key,
        route: &ctx.route,
        endpoint: &ctx.public_path,
        model: &ctx.model,
        status,
        usage,
        cache_ctx: &ctx.cache_ctx,
    })
    .await
    {
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("failed to record kiro usage: {err}"))
            .into_response();
    }
    let body = sse_events
        .into_iter()
        .map(|event| event.to_sse_string())
        .collect::<String>();
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONNECTION, "keep-alive")
        .body(Body::from(body))
        .unwrap_or_else(|_| {
            (StatusCode::BAD_GATEWAY, "kiro buffered response build failed").into_response()
        })
}

async fn non_stream_kiro_response(
    response: reqwest::Response,
    ctx: KiroResponseContext,
) -> Response {
    let status = response.status();
    let bytes = match response.bytes().await {
        Ok(bytes) => bytes,
        Err(_) => {
            return kiro_json_error(
                StatusCode::BAD_GATEWAY,
                "api_error",
                "failed to read kiro upstream response",
            )
        },
    };
    let events = match decode_kiro_events_from_bytes(&bytes) {
        Ok(events) => events,
        Err(err) => return kiro_json_error(StatusCode::BAD_GATEWAY, "api_error", &err),
    };
    let mut stream_ctx = StreamContext::new_with_thinking(
        &ctx.model,
        ctx.request_input_tokens,
        ctx.thinking_enabled,
        ctx.tool_name_map,
        ctx.structured_output_tool_name.clone(),
    );
    for event in &events {
        let _ = stream_ctx.process_kiro_event(event);
    }
    let _ = stream_ctx.generate_final_events();
    let (input_tokens, output_tokens) = stream_ctx.final_usage();
    let (credit_usage, credit_usage_missing) = stream_ctx.final_credit_usage();
    let usage = build_kiro_usage_summary(
        &ctx.model,
        KiroUsageInputs {
            request_input_tokens: ctx.request_input_tokens,
            context_input_tokens: Some(input_tokens),
            output_tokens,
            credit_usage,
            credit_usage_missing,
            cache_estimation_enabled: ctx.route.cache_estimation_enabled,
        },
        &ctx.cache_ctx,
    );
    let assistant_message = stream_ctx.final_assistant_message();
    let mut content = build_inline_thinking_content_blocks(
        &assistant_message.content,
        &ctx.model,
        ctx.thinking_enabled,
    );
    if let Some(tool_uses) = assistant_message.tool_uses.clone() {
        content.extend(tool_uses.into_iter().map(|tool_use| {
            serde_json::json!({
                "type": "tool_use",
                "id": tool_use.tool_use_id,
                "name": tool_use.name,
                "input": tool_use.input,
            })
        }));
    }
    let stop_reason = stream_ctx.state_manager.get_stop_reason();
    ctx.kiro_cache_simulator.record_success(
        &ctx.cache_ctx.projection,
        &assistant_message,
        &ctx.cache_ctx.conversation_id,
        ctx.route.cache_estimation_enabled,
        ctx.cache_ctx.simulation_config,
        Instant::now(),
    );
    if let Err(err) = record_kiro_usage(KiroUsageRecord {
        control_store: ctx.control_store.as_ref(),
        key: &ctx.key,
        route: &ctx.route,
        endpoint: &ctx.public_path,
        model: &ctx.model,
        status,
        usage,
        cache_ctx: &ctx.cache_ctx,
    })
    .await
    {
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("failed to record kiro usage: {err}"))
            .into_response();
    }
    let body = serde_json::json!({
        "id": format!("msg_{}", uuid::Uuid::new_v4().simple()),
        "type": "message",
        "role": "assistant",
        "content": content,
        "model": ctx.model,
        "stop_reason": stop_reason,
        "stop_sequence": null,
        "usage": anthropic_usage_json(
            usage.input_uncached_tokens + usage.input_cached_tokens,
            usage.output_tokens,
            usage.input_cached_tokens,
        ),
    });
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(body.to_string()))
        .unwrap_or_else(|_| {
            (StatusCode::BAD_GATEWAY, "kiro json response build failed").into_response()
        })
}

async fn pass_through_kiro_error_response(response: reqwest::Response) -> Response {
    let status = response.status();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/json")
        .to_string();
    let bytes = response.bytes().await.unwrap_or_else(|_| Bytes::new());
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(bytes))
        .unwrap_or_else(|_| {
            (StatusCode::BAD_GATEWAY, "kiro upstream error response build failed").into_response()
        })
}

const KIRO_REQUEST_SESSION_ID_HEADERS: [&str; 8] = [
    "x-claude-code-session-id",
    "x-codex-session-id",
    "x-openclaw-session-id",
    "conversation_id",
    "conversation-id",
    "session_id",
    "session-id",
    "x-session-id",
];

#[derive(Debug, Clone, Copy)]
struct KiroUsageSummary {
    input_uncached_tokens: i32,
    input_cached_tokens: i32,
    output_tokens: i32,
    credit_usage: Option<f64>,
    credit_usage_missing: bool,
}

#[derive(Debug, Clone, Copy)]
struct KiroUsageInputs {
    request_input_tokens: i32,
    context_input_tokens: Option<i32>,
    output_tokens: i32,
    credit_usage: Option<f64>,
    credit_usage_missing: bool,
    cache_estimation_enabled: bool,
}

fn normalized_kiro_messages_path(path: &str) -> Option<(&'static str, bool)> {
    match path {
        "/cc/v1/messages" | "/api/kiro-gateway/cc/v1/messages" => Some(("/cc/v1/messages", true)),
        "/api/kiro-gateway/v1/messages" => Some(("/v1/messages", false)),
        _ => None,
    }
}

fn kiro_access_token_from_auth_json(auth_json: &str) -> Option<String> {
    let value = serde_json::from_str::<Value>(auth_json).ok()?;
    value
        .get("accessToken")
        .and_then(Value::as_str)
        .or_else(|| value.get("access_token").and_then(Value::as_str))
        .or_else(|| {
            value
                .get("tokens")
                .and_then(|tokens| tokens.get("access_token"))
                .and_then(Value::as_str)
        })
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(ToString::to_string)
}

fn provider_client(proxy: Option<&ProviderProxyConfig>) -> anyhow::Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder();
    if let Some(proxy_config) = proxy {
        let mut proxy = reqwest::Proxy::all(&proxy_config.proxy_url)?;
        if let Some(username) = proxy_config.proxy_username.as_deref() {
            proxy =
                proxy.basic_auth(username, proxy_config.proxy_password.as_deref().unwrap_or(""));
        }
        builder = builder.proxy(proxy);
    }
    Ok(builder.build()?)
}

fn kiro_json_error(status: StatusCode, error_type: &str, message: &str) -> Response {
    let body = serde_json::json!({
        "error": {
            "type": error_type,
            "message": message,
        }
    });
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap_or_else(|_| {
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to build error").into_response()
        })
}

fn kiro_conversion_error_response(err: ConversionError) -> Response {
    match err {
        ConversionError::UnsupportedModel(model) => kiro_json_error(
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            &format!("Unsupported model: {model}"),
        ),
        ConversionError::EmptyMessages => {
            kiro_json_error(StatusCode::BAD_REQUEST, "invalid_request_error", "messages are empty")
        },
        ConversionError::InvalidRequest(message) => {
            kiro_json_error(StatusCode::BAD_REQUEST, "invalid_request_error", &message)
        },
    }
}

fn override_kiro_thinking_from_model_name(payload: &mut MessagesRequest) {
    let model = payload.model.to_lowercase();
    if !model.contains("thinking") {
        return;
    }
    let is_opus_46 = model.contains("opus") && (model.contains("4-6") || model.contains("4.6"));
    payload.thinking = Some(Thinking {
        thinking_type: if is_opus_46 { "adaptive".to_string() } else { "enabled".to_string() },
        budget_tokens: 20_000,
    });
    if is_opus_46 {
        let output_config = payload.output_config.get_or_insert(OutputConfig {
            effort: None,
            format: None,
        });
        if output_config.effort.is_none() {
            output_config.effort = Some("xhigh".to_string());
        }
    }
}

fn resolve_kiro_request_session(
    headers: &HeaderMap,
    metadata: Option<&llm_access_kiro::anthropic::types::Metadata>,
) -> ResolvedConversationId {
    let mut first_invalid_header: Option<(&'static str, String)> = None;
    for header_name in KIRO_REQUEST_SESSION_ID_HEADERS {
        let Some(raw_value) = headers
            .get(header_name)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
        else {
            continue;
        };
        if uuid::Uuid::try_parse(&raw_value).is_ok() {
            return ResolvedConversationId {
                conversation_id: raw_value.clone(),
                session_tracking: SessionTracking {
                    source: SessionIdSource::RequestHeader,
                    source_name: Some(header_name),
                    source_value_preview: Some(preview_session_value(&raw_value)),
                },
            };
        }
        if first_invalid_header.is_none() {
            first_invalid_header = Some((header_name, preview_session_value(&raw_value)));
        }
    }

    let mut resolved = resolve_conversation_id_from_metadata(metadata);
    if matches!(resolved.session_tracking.source, SessionIdSource::GeneratedFallback(_)) {
        if let Some((header_name, preview)) = first_invalid_header {
            resolved.session_tracking = SessionTracking {
                source: SessionIdSource::GeneratedFallback(
                    SessionFallbackReason::InvalidHeaderSessionId,
                ),
                source_name: Some(header_name),
                source_value_preview: Some(preview),
            };
        }
    }
    resolved
}

fn build_kiro_cache_context(
    route: &ProviderKiroRoute,
    conversation_state: &llm_access_kiro::wire::ConversationState,
    cache_simulator: &KiroCacheSimulator,
) -> anyhow::Result<KiroCacheContext> {
    let policy = if route.cache_policy_json.trim().is_empty() {
        default_kiro_cache_policy()
    } else {
        serde_json::from_str::<KiroCachePolicy>(&route.cache_policy_json)?
    };
    validate_kiro_cache_policy(&policy)?;
    let simulation_config = KiroCacheSimulationConfig {
        mode: KiroCacheSimulationMode::from_runtime_value(&route.prefix_cache_mode),
        prefix_cache_max_tokens: route.prefix_cache_max_tokens,
        prefix_cache_entry_ttl: Duration::from_secs(route.prefix_cache_entry_ttl_seconds),
        conversation_anchor_max_entries: route.conversation_anchor_max_entries as usize,
        conversation_anchor_ttl: Duration::from_secs(route.conversation_anchor_ttl_seconds),
    };
    let projection = PromptProjection::from_conversation_state(conversation_state);
    let prefix_cache_match = if route.cache_estimation_enabled
        && simulation_config.mode == KiroCacheSimulationMode::PrefixTree
    {
        cache_simulator.match_prefix(&projection, simulation_config, Instant::now())
    } else {
        llm_access_kiro::cache_sim::PrefixCacheMatch::default()
    };
    Ok(KiroCacheContext {
        policy,
        simulation_config,
        projection,
        prefix_cache_match,
        conversation_id: conversation_state.conversation_id.clone(),
        cache_kmodels: parse_kiro_cache_kmodels_json(&route.cache_kmodels_json)?,
        billable_model_multipliers: parse_kiro_billable_model_multipliers_json(
            &route.billable_model_multipliers_json,
        )?,
    })
}

fn parse_kiro_cache_kmodels_json(value: &str) -> anyhow::Result<BTreeMap<String, f64>> {
    let map = serde_json::from_str::<BTreeMap<String, f64>>(value)?;
    for (model, kmodel) in &map {
        if !kmodel.is_finite() || *kmodel <= 0.0 {
            anyhow::bail!("kiro cache kmodel `{model}` must be positive and finite");
        }
    }
    Ok(map)
}

fn parse_kiro_billable_model_multipliers_json(
    value: &str,
) -> anyhow::Result<BTreeMap<String, f64>> {
    let map = serde_json::from_str::<BTreeMap<String, f64>>(value)?;
    for (family, multiplier) in &map {
        if !matches!(family.as_str(), "opus" | "sonnet" | "haiku") {
            anyhow::bail!("kiro billable multiplier family `{family}` is invalid");
        }
        if !multiplier.is_finite() || *multiplier <= 0.0 {
            anyhow::bail!("kiro billable multiplier `{family}` must be positive and finite");
        }
    }
    Ok(map)
}

fn decode_kiro_events_from_bytes(bytes: &[u8]) -> Result<Vec<Event>, String> {
    let mut decoder = EventStreamDecoder::new();
    let _ = decoder.feed(bytes);
    let mut events = Vec::new();
    for result in decoder.decode_iter() {
        let frame = result.map_err(|err| format!("failed to decode kiro event frame: {err}"))?;
        let event =
            Event::from_frame(frame).map_err(|err| format!("failed to parse kiro event: {err}"))?;
        events.push(event);
    }
    Ok(events)
}

fn build_kiro_usage_summary(
    model: &str,
    usage: KiroUsageInputs,
    cache_ctx: &KiroCacheContext,
) -> KiroUsageSummary {
    let (resolved_input_tokens, _) =
        resolve_input_tokens(usage.request_input_tokens, usage.context_input_tokens);
    if !usage.cache_estimation_enabled {
        return KiroUsageSummary {
            input_uncached_tokens: resolved_input_tokens,
            input_cached_tokens: 0,
            output_tokens: usage.output_tokens,
            credit_usage: usage.credit_usage,
            credit_usage_missing: usage.credit_usage_missing,
        };
    }
    let authoritative_input_tokens = adjust_input_tokens_for_cache_creation_cost_with_policy(
        &cache_ctx.policy,
        resolved_input_tokens,
        usage.credit_usage,
        usage.cache_estimation_enabled,
    );
    let cached = match cache_ctx.simulation_config.mode {
        KiroCacheSimulationMode::Formula => estimate_formula_cached_tokens(
            model,
            authoritative_input_tokens,
            usage.output_tokens,
            usage.credit_usage,
            &cache_ctx.cache_kmodels,
        ),
        KiroCacheSimulationMode::PrefixTree => {
            estimate_prefix_cached_tokens(authoritative_input_tokens, usage.credit_usage, cache_ctx)
        },
    };
    KiroUsageSummary {
        input_uncached_tokens: authoritative_input_tokens.saturating_sub(cached),
        input_cached_tokens: cached,
        output_tokens: usage.output_tokens,
        credit_usage: usage.credit_usage,
        credit_usage_missing: usage.credit_usage_missing,
    }
}

fn estimate_formula_cached_tokens(
    model: &str,
    input_tokens_total: i32,
    output_tokens: i32,
    credit_usage: Option<f64>,
    kmodels: &BTreeMap<String, f64>,
) -> i32 {
    let safe_input = input_tokens_total.max(0);
    let Some(observed_credit) = credit_usage.filter(|value| value.is_finite() && *value >= 0.0)
    else {
        return 0;
    };
    let Some(kmodel) = kmodels
        .get(normalize_kiro_kmodel_name(model))
        .copied()
        .filter(|value| value.is_finite() && *value > 0.0)
    else {
        return 0;
    };
    let safe_full_cost = kmodel * (safe_input as f64 + 5.0 * output_tokens.max(0) as f64);
    if !safe_full_cost.is_finite() || safe_full_cost <= observed_credit || safe_input <= 0 {
        return 0;
    }
    ((safe_full_cost - observed_credit) / (0.9 * kmodel))
        .floor()
        .max(0.0)
        .min(safe_input as f64) as i32
}

fn estimate_prefix_cached_tokens(
    authoritative_input_tokens: i32,
    credit_usage: Option<f64>,
    cache_ctx: &KiroCacheContext,
) -> i32 {
    let authoritative_input_u64 = authoritative_input_tokens.max(0) as u64;
    let projected_total = cache_ctx.projection.projected_input_token_count.max(1);
    let matched = cache_ctx
        .prefix_cache_match
        .matched_tokens
        .min(projected_total);
    let prefix_cached = ((u128::from(authoritative_input_u64) * u128::from(matched))
        / u128::from(projected_total))
    .min(u128::from(authoritative_input_u64)) as i32;
    let Some(cap_basis_points) =
        prefix_tree_credit_ratio_cap_basis_points_with_policy(&cache_ctx.policy, credit_usage)
    else {
        return prefix_cached;
    };
    let ratio_cap = ((u128::from(authoritative_input_u64) * u128::from(cap_basis_points))
        / 10_000_u128)
        .min(u128::from(authoritative_input_u64)) as i32;
    prefix_cached.min(ratio_cap)
}

fn normalize_kiro_kmodel_name(model: &str) -> &str {
    match model {
        "claude-opus-4.6" => "claude-opus-4-6",
        _ => model,
    }
}

struct KiroUsageRecord<'a> {
    control_store: &'a dyn ControlStore,
    key: &'a AuthenticatedKey,
    route: &'a ProviderKiroRoute,
    endpoint: &'a str,
    model: &'a str,
    status: StatusCode,
    usage: KiroUsageSummary,
    cache_ctx: &'a KiroCacheContext,
}

async fn record_kiro_usage(record: KiroUsageRecord<'_>) -> anyhow::Result<()> {
    let billable_tokens = kiro_billable_tokens(record.model, record.usage, record.cache_ctx);
    let event = UsageEvent {
        event_id: format!("llm-usage-{}", uuid::Uuid::new_v4()),
        created_at_ms: now_millis(),
        provider_type: ProviderType::Kiro,
        protocol_family: ProtocolFamily::Anthropic,
        key_id: record.key.key_id.clone(),
        key_name: record.key.key_name.clone(),
        account_name: Some(record.route.account_name.clone()),
        route_strategy_at_event: None,
        endpoint: record.endpoint.to_string(),
        model: Some(record.model.to_string()),
        mapped_model: None,
        status_code: record.status.as_u16() as i64,
        request_body_bytes: None,
        input_uncached_tokens: i64::from(record.usage.input_uncached_tokens.max(0)),
        input_cached_tokens: i64::from(record.usage.input_cached_tokens.max(0)),
        output_tokens: i64::from(record.usage.output_tokens.max(0)),
        billable_tokens: clamp_u64_to_i64(billable_tokens),
        credit_usage: record
            .usage
            .credit_usage
            .map(|value| value.max(0.0).to_string()),
        usage_missing: false,
        credit_usage_missing: record.usage.credit_usage_missing,
        timing: UsageTiming::default(),
    };
    record.control_store.apply_usage_rollup(&event).await
}

fn kiro_billable_tokens(model: &str, usage: KiroUsageSummary, cache_ctx: &KiroCacheContext) -> u64 {
    let family = if model.contains("opus") {
        "opus"
    } else if model.contains("haiku") {
        "haiku"
    } else {
        "sonnet"
    };
    let multiplier = cache_ctx
        .billable_model_multipliers
        .get(family)
        .copied()
        .unwrap_or(1.0)
        .max(0.0);
    let weighted = usage.input_uncached_tokens.max(0) as f64
        + usage.input_cached_tokens.max(0) as f64
        + (usage.output_tokens.max(0) as f64 * 5.0);
    (weighted * multiplier)
        .round()
        .max(0.0)
        .min(u64::MAX as f64) as u64
}

/// Axum entrypoint for provider requests.
pub async fn provider_entry_handler(
    State(state): State<ProviderState>,
    request: Request<Body>,
) -> Response {
    provider_entry(state, request).await
}

/// Authenticate a provider request before handing it to provider dispatch.
pub async fn provider_entry(state: ProviderState, request: Request<Body>) -> Response {
    let Some(secret) = presented_secret(request.headers(), request.uri().path()).map(str::to_owned)
    else {
        return (StatusCode::UNAUTHORIZED, "missing bearer token").into_response();
    };
    let key = match state
        .control_store
        .authenticate_bearer_secret(&secret)
        .await
    {
        Ok(Some(key)) => key,
        Ok(None) => return (StatusCode::UNAUTHORIZED, "invalid bearer token").into_response(),
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "authentication backend error")
                .into_response();
        },
    };
    if !is_active_key(&key) {
        return (StatusCode::FORBIDDEN, "llm key is not active").into_response();
    }
    if !key_matches_route(&key, request.uri().path()) {
        return (StatusCode::FORBIDDEN, "llm key does not match provider route").into_response();
    }
    if is_quota_exhausted(&key) {
        return quota_exhausted_response(&key);
    }

    state
        .dispatcher
        .dispatch(
            key,
            request,
            Arc::clone(&state.route_store),
            Arc::clone(&state.control_store),
            Arc::clone(&state.kiro_cache_simulator),
            Arc::clone(&state.request_limiter),
        )
        .await
}

fn presented_secret<'a>(headers: &'a HeaderMap, path: &str) -> Option<&'a str> {
    if is_kiro_data_plane_route(path) {
        x_api_key_secret(headers).or_else(|| bearer_secret(headers))
    } else {
        bearer_secret(headers)
    }
}

fn is_kiro_data_plane_route(path: &str) -> bool {
    provider_route_requirement(path)
        .map(|requirement| requirement.provider_type == ProviderType::Kiro)
        .unwrap_or(false)
}

fn x_api_key_secret(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get("x-api-key")?.to_str().ok()?.trim();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn bearer_secret(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    if token.is_empty() {
        None
    } else {
        Some(token)
    }
}

fn is_active_key(key: &AuthenticatedKey) -> bool {
    key.status == "active"
}

fn key_matches_route(key: &AuthenticatedKey, path: &str) -> bool {
    let Some(requirement) = provider_route_requirement(path) else {
        return true;
    };
    ProviderType::from_storage_str(&key.provider_type) == Some(requirement.provider_type)
        && ProtocolFamily::from_storage_str(&key.protocol_family)
            == Some(requirement.protocol_family)
}

fn is_quota_exhausted(key: &AuthenticatedKey) -> bool {
    key.remaining_billable() <= 0
}

fn quota_exhausted_response(key: &AuthenticatedKey) -> Response {
    if ProviderType::from_storage_str(&key.provider_type) == Some(ProviderType::Kiro) {
        (StatusCode::PAYMENT_REQUIRED, "Kiro key quota exhausted").into_response()
    } else {
        (StatusCode::TOO_MANY_REQUESTS, "quota_exceeded").into_response()
    }
}

fn should_serve_local_codex_models(key: &AuthenticatedKey, request: &Request<Body>) -> bool {
    ProviderType::from_storage_str(&key.provider_type) == Some(ProviderType::Codex)
        && request.method() == Method::GET
        && normalized_codex_gateway_path(request.uri().path()) == Some("/v1/models")
}

fn normalized_codex_gateway_path(path: &str) -> Option<&str> {
    if path == "/v1/models" {
        return Some(path);
    }
    if path == "/v1/chat/completions"
        || path == "/v1/responses"
        || path.starts_with("/v1/responses/")
    {
        return Some(path);
    }
    path.strip_prefix("/api/llm-gateway")
        .or_else(|| path.strip_prefix("/api/codex-gateway"))
        .filter(|value| {
            *value == "/v1/models"
                || *value == "/v1/chat/completions"
                || *value == "/v1/responses"
                || value.starts_with("/v1/responses/")
        })
}

fn codex_access_token_from_auth_json(auth_json: &str) -> Option<String> {
    let value = serde_json::from_str::<Value>(auth_json).ok()?;
    value
        .get("access_token")
        .and_then(Value::as_str)
        .or_else(|| {
            value
                .get("tokens")
                .and_then(|tokens| tokens.get("access_token"))
                .and_then(Value::as_str)
        })
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(ToString::to_string)
}

struct CompletedCodexSse {
    response: Value,
    usage: Option<UsageBreakdown>,
}

fn completed_response_from_sse_bytes(bytes: &[u8]) -> Result<CompletedCodexSse, &'static str> {
    let mut usage = None;
    for data in sse_data_payloads(bytes) {
        if data.trim() == "[DONE]" {
            continue;
        }
        let value =
            serde_json::from_str::<Value>(&data).map_err(|_| "invalid codex upstream SSE JSON")?;
        if let Some(observed_usage) = extract_usage_from_bytes(data.as_bytes()) {
            usage = Some(observed_usage);
        }
        if value.get("type").and_then(Value::as_str) == Some("response.completed") {
            let response = value
                .get("response")
                .cloned()
                .ok_or("codex upstream response.completed event is missing response")?;
            return Ok(CompletedCodexSse {
                response,
                usage,
            });
        }
    }
    Err("codex upstream SSE stream did not include response.completed")
}

fn sse_data_payloads(bytes: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(bytes).replace("\r\n", "\n");
    text.split("\n\n")
        .filter_map(|event| {
            let data = event
                .lines()
                .filter_map(|line| line.strip_prefix("data:"))
                .map(|line| line.strip_prefix(' ').unwrap_or(line))
                .collect::<Vec<_>>();
            if data.is_empty() {
                None
            } else {
                Some(data.join("\n"))
            }
        })
        .collect()
}

fn rewrite_json_value_model_alias(
    value: Value,
    model_from: Option<&str>,
    model_to: Option<&str>,
) -> Value {
    let Ok(bytes) = serde_json::to_vec(&value) else {
        return value;
    };
    rewrite_json_response_model_alias(&bytes, model_from, model_to)
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .unwrap_or(value)
}

async fn record_codex_usage(
    control_store: &dyn ControlStore,
    key: &AuthenticatedKey,
    prepared: &PreparedGatewayRequest,
    status: StatusCode,
    account_name: &str,
    usage: UsageBreakdown,
) -> anyhow::Result<()> {
    let event = UsageEvent {
        event_id: format!("llm-usage-{}", uuid::Uuid::new_v4()),
        created_at_ms: now_millis(),
        provider_type: ProviderType::Codex,
        protocol_family: ProtocolFamily::OpenAi,
        key_id: key.key_id.clone(),
        key_name: key.key_name.clone(),
        account_name: Some(account_name.to_string()),
        route_strategy_at_event: None,
        endpoint: prepared.original_path.clone(),
        model: prepared
            .client_visible_model
            .clone()
            .or_else(|| prepared.model.clone()),
        mapped_model: prepared.model.clone(),
        status_code: i64::from(status.as_u16()),
        request_body_bytes: Some(clamp_usize_to_i64(prepared.client_request_body.len())),
        input_uncached_tokens: clamp_u64_to_i64(usage.input_uncached_tokens),
        input_cached_tokens: clamp_u64_to_i64(usage.input_cached_tokens),
        output_tokens: clamp_u64_to_i64(usage.output_tokens),
        billable_tokens: clamp_u64_to_i64(
            usage.billable_tokens_with_multiplier(prepared.billable_multiplier),
        ),
        credit_usage: None,
        usage_missing: usage.usage_missing,
        credit_usage_missing: false,
        timing: UsageTiming::default(),
    };
    control_store.apply_usage_rollup(&event).await
}

fn missing_codex_usage() -> UsageBreakdown {
    UsageBreakdown {
        usage_missing: true,
        ..UsageBreakdown::default()
    }
}

fn now_millis() -> i64 {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    millis.min(i64::MAX as u128) as i64
}

fn clamp_u64_to_i64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

fn clamp_usize_to_i64(value: usize) -> i64 {
    value.min(i64::MAX as usize) as i64
}

fn local_codex_models_response() -> Response {
    let body = match llm_access_codex::models::default_openai_models_response_json(now_seconds()) {
        Ok(body) => body,
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "failed to build codex models response")
                .into_response();
        },
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(body))
        .unwrap_or_else(|_| {
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to build codex models response")
                .into_response()
        })
}

fn now_seconds() -> i64 {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    seconds.min(i64::MAX as u64) as i64
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use axum::{
        body::{to_bytes, Body},
        extract::State,
        http::{header, HeaderMap, Request, StatusCode},
        response::{IntoResponse, Response},
        routing::post,
        Router,
    };
    use llm_access_core::store::{
        AuthenticatedKey, ControlStore, EmptyProviderRouteStore, ProviderCodexRoute,
        ProviderKiroRoute, ProviderRouteStore,
    };
    use serde_json::json;

    use super::ProviderDispatcher;

    static KIRO_UPSTREAM_ENV_LOCK: Mutex<()> = Mutex::new(());

    #[derive(Default)]
    struct TestStore;

    #[async_trait]
    impl ControlStore for TestStore {
        async fn authenticate_bearer_secret(
            &self,
            secret: &str,
        ) -> anyhow::Result<Option<AuthenticatedKey>> {
            let (key_id, key_name, provider_type, protocol_family, status) = match secret {
                "valid-secret" => ("key-1", "test-key", "kiro", "anthropic", "active"),
                "codex-secret" => ("key-2", "codex-key", "codex", "openai", "active"),
                "paused-secret" => ("key-1", "test-key", "kiro", "anthropic", "paused"),
                "exhausted-kiro-secret" => {
                    ("key-3", "exhausted-kiro-key", "kiro", "anthropic", "active")
                },
                "exhausted-codex-secret" => {
                    ("key-4", "exhausted-codex-key", "codex", "openai", "active")
                },
                _ => return Ok(None),
            };
            let billable_tokens_used =
                if matches!(secret, "exhausted-kiro-secret" | "exhausted-codex-secret") {
                    100
                } else {
                    0
                };
            Ok(Some(AuthenticatedKey {
                key_id: key_id.to_string(),
                key_name: key_name.to_string(),
                provider_type: provider_type.to_string(),
                protocol_family: protocol_family.to_string(),
                status: status.to_string(),
                quota_billable_limit: 100,
                billable_tokens_used,
            }))
        }

        async fn apply_usage_rollup(
            &self,
            _event: &llm_access_core::usage::UsageEvent,
        ) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct FailingStore;

    #[async_trait]
    impl ControlStore for FailingStore {
        async fn authenticate_bearer_secret(
            &self,
            _secret: &str,
        ) -> anyhow::Result<Option<AuthenticatedKey>> {
            Err(anyhow::anyhow!("store unavailable"))
        }

        async fn apply_usage_rollup(
            &self,
            _event: &llm_access_core::usage::UsageEvent,
        ) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct CapturingDispatcher {
        seen: Mutex<Vec<(String, String)>>,
    }

    #[derive(Clone)]
    struct StaticRouteStore {
        codex_route: ProviderCodexRoute,
        kiro_route: ProviderKiroRoute,
    }

    #[async_trait]
    impl ProviderRouteStore for StaticRouteStore {
        async fn resolve_codex_route(
            &self,
            _key: &AuthenticatedKey,
        ) -> anyhow::Result<Option<ProviderCodexRoute>> {
            Ok(Some(self.codex_route.clone()))
        }

        async fn resolve_kiro_route(
            &self,
            _key: &AuthenticatedKey,
        ) -> anyhow::Result<Option<ProviderKiroRoute>> {
            Ok(Some(self.kiro_route.clone()))
        }
    }

    #[derive(Debug, Default)]
    struct CapturedCodexUpstream {
        requests: Mutex<Vec<CapturedCodexRequest>>,
    }

    #[derive(Debug)]
    struct CapturedCodexRequest {
        path: String,
        authorization: Option<String>,
        accept: Option<String>,
        body: serde_json::Value,
    }

    #[derive(Debug, Default)]
    struct CapturedKiroUpstream {
        requests: Mutex<Vec<CapturedKiroRequest>>,
    }

    #[derive(Debug)]
    struct CapturedKiroRequest {
        path: String,
        authorization: Option<String>,
        body: serde_json::Value,
    }

    #[derive(Default)]
    struct RecordingControlStore {
        usage_events: Mutex<Vec<llm_access_core::usage::UsageEvent>>,
    }

    #[async_trait]
    impl ControlStore for RecordingControlStore {
        async fn authenticate_bearer_secret(
            &self,
            secret: &str,
        ) -> anyhow::Result<Option<AuthenticatedKey>> {
            let (key_id, key_name, provider_type, protocol_family) = match secret {
                "codex-secret" => ("key-usage", "usage-key", "codex", "openai"),
                "valid-secret" => ("key-kiro-usage", "kiro-usage-key", "kiro", "anthropic"),
                _ => return Ok(None),
            };
            Ok(Some(AuthenticatedKey {
                key_id: key_id.to_string(),
                key_name: key_name.to_string(),
                provider_type: provider_type.to_string(),
                protocol_family: protocol_family.to_string(),
                status: "active".to_string(),
                quota_billable_limit: 1000,
                billable_tokens_used: 0,
            }))
        }

        async fn apply_usage_rollup(
            &self,
            event: &llm_access_core::usage::UsageEvent,
        ) -> anyhow::Result<()> {
            self.usage_events
                .lock()
                .expect("usage events")
                .push(event.clone());
            Ok(())
        }
    }

    #[async_trait]
    impl ProviderDispatcher for CapturingDispatcher {
        async fn dispatch(
            &self,
            key: AuthenticatedKey,
            request: Request<Body>,
            _route_store: Arc<dyn ProviderRouteStore>,
            _control_store: Arc<dyn ControlStore>,
            _kiro_cache_simulator: Arc<llm_access_kiro::cache_sim::KiroCacheSimulator>,
            _request_limiter: Arc<super::RequestLimiter>,
        ) -> Response {
            self.seen
                .lock()
                .expect("dispatcher state")
                .push((key.key_id, request.uri().path().to_string()));
            (StatusCode::ACCEPTED, "dispatched").into_response()
        }
    }

    fn request_with_bearer_to_path(path: &str, secret: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder().uri(path);
        if let Some(secret) = secret {
            builder = builder.header(header::AUTHORIZATION, secret);
        }
        builder.body(Body::empty()).expect("request")
    }

    fn request_with_bearer(secret: Option<&str>) -> Request<Body> {
        request_with_bearer_to_path("/api/kiro-gateway/v1/messages", secret)
    }

    fn empty_route_store() -> Arc<dyn ProviderRouteStore> {
        Arc::new(EmptyProviderRouteStore)
    }

    fn static_codex_route_store() -> Arc<dyn ProviderRouteStore> {
        Arc::new(StaticRouteStore {
            codex_route: ProviderCodexRoute {
                account_name: "codex-a".to_string(),
                auth_json: r#"{"access_token":"upstream-token"}"#.to_string(),
                map_gpt53_codex_to_spark: true,
                request_max_concurrency: None,
                request_min_start_interval_ms: None,
                account_request_max_concurrency: None,
                account_request_min_start_interval_ms: None,
                proxy: None,
            },
            kiro_route: static_kiro_route(),
        })
    }

    fn static_kiro_route_store() -> Arc<dyn ProviderRouteStore> {
        Arc::new(StaticRouteStore {
            codex_route: ProviderCodexRoute {
                account_name: "codex-a".to_string(),
                auth_json: r#"{"access_token":"upstream-token"}"#.to_string(),
                map_gpt53_codex_to_spark: true,
                request_max_concurrency: None,
                request_min_start_interval_ms: None,
                account_request_max_concurrency: None,
                account_request_min_start_interval_ms: None,
                proxy: None,
            },
            kiro_route: static_kiro_route(),
        })
    }

    fn static_kiro_route() -> ProviderKiroRoute {
        ProviderKiroRoute {
            account_name: "kiro-a".to_string(),
            auth_json: r#"{"accessToken":"kiro-upstream-token"}"#.to_string(),
            profile_arn: Some("arn:aws:kiro:test".to_string()),
            api_region: "us-east-1".to_string(),
            request_validation_enabled: true,
            cache_estimation_enabled: true,
            cache_kmodels_json: llm_access_core::store::default_kiro_cache_kmodels_json(),
            cache_policy_json: llm_access_core::store::default_kiro_cache_policy_json(),
            prefix_cache_mode: "formula".to_string(),
            prefix_cache_max_tokens: 100_000,
            prefix_cache_entry_ttl_seconds: 3600,
            conversation_anchor_max_entries: 1024,
            conversation_anchor_ttl_seconds: 3600,
            billable_model_multipliers_json:
                llm_access_core::store::default_kiro_billable_model_multipliers_json(),
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
            account_request_max_concurrency: None,
            account_request_min_start_interval_ms: None,
            proxy: None,
        }
    }

    fn test_state() -> super::ProviderState {
        super::ProviderState::new(Arc::new(TestStore), empty_route_store())
    }

    fn test_state_with_dispatcher(dispatcher: Arc<dyn ProviderDispatcher>) -> super::ProviderState {
        super::ProviderState::with_dispatcher(Arc::new(TestStore), empty_route_store(), dispatcher)
    }

    fn request_with_x_api_key_to_path(path: &str, secret: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder().uri(path);
        if let Some(secret) = secret {
            builder = builder.header("x-api-key", secret);
        }
        builder.body(Body::empty()).expect("request")
    }

    async fn fake_codex_responses(
        State(captured): State<Arc<CapturedCodexUpstream>>,
        headers: HeaderMap,
        request: Request<Body>,
    ) -> Response {
        let path = request.uri().path().to_string();
        let body = to_bytes(request.into_body(), usize::MAX)
            .await
            .expect("upstream request body");
        let body = serde_json::from_slice::<serde_json::Value>(&body).expect("upstream json");
        captured
            .requests
            .lock()
            .expect("captured requests")
            .push(CapturedCodexRequest {
                path,
                authorization: headers
                    .get(header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok())
                    .map(ToString::to_string),
                accept: headers
                    .get(header::ACCEPT)
                    .and_then(|value| value.to_str().ok())
                    .map(ToString::to_string),
                body,
            });

        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from(format!(
                "event: response.output_text.delta\ndata: {}\n\nevent: \
                 response.output_text.delta\ndata: {}\n\nevent: response.completed\ndata: {}\n\n",
                json!({
                    "type": "response.output_text.delta",
                    "response_id": "resp_1",
                    "created": 123,
                    "model": "gpt-5.3-codex-spark",
                    "delta": "hello "
                }),
                json!({
                    "type": "response.output_text.delta",
                    "response_id": "resp_1",
                    "created": 123,
                    "model": "gpt-5.3-codex-spark",
                    "delta": "back"
                }),
                json!({
                    "type": "response.completed",
                    "response": {
                        "id": "resp_1",
                        "created_at": 123,
                        "model": "gpt-5.3-codex-spark",
                        "output": [{
                            "type": "message",
                            "content": [{
                                "type": "output_text",
                                "text": "hello back"
                            }]
                        }],
                        "usage": {
                            "input_tokens": 12,
                            "input_tokens_details": {
                                "cached_tokens": 2
                            },
                            "output_tokens": 3
                        }
                    }
                })
            )))
            .expect("upstream response")
    }

    async fn fake_kiro_generate(
        State(captured): State<Arc<CapturedKiroUpstream>>,
        headers: HeaderMap,
        request: Request<Body>,
    ) -> Response {
        let path = request.uri().path().to_string();
        let body = to_bytes(request.into_body(), usize::MAX)
            .await
            .expect("upstream request body");
        let body = serde_json::from_slice::<serde_json::Value>(&body).expect("upstream json");
        captured
            .requests
            .lock()
            .expect("captured requests")
            .push(CapturedKiroRequest {
                path,
                authorization: headers
                    .get(header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok())
                    .map(ToString::to_string),
                body,
            });
        let body = kiro_eventstream_body(vec![
            kiro_event_frame("assistantResponseEvent", &json!({"content":"hello "})),
            kiro_event_frame("assistantResponseEvent", &json!({"content":"back"})),
            kiro_event_frame("contextUsageEvent", &json!({"contextUsagePercentage":0.01})),
            kiro_event_frame("meteringEvent", &json!({"unit":"credit","usage":0.25})),
        ]);
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/vnd.amazon.eventstream")
            .body(Body::from(body))
            .expect("upstream response")
    }

    fn kiro_eventstream_body(frames: Vec<Vec<u8>>) -> Vec<u8> {
        frames.into_iter().flatten().collect()
    }

    fn kiro_event_frame(event_type: &str, payload: &serde_json::Value) -> Vec<u8> {
        let payload = serde_json::to_vec(payload).expect("payload json");
        let mut headers = Vec::new();
        push_aws_string_header(&mut headers, ":message-type", "event");
        push_aws_string_header(&mut headers, ":event-type", event_type);
        let total_length = 12 + headers.len() + payload.len() + 4;
        let mut frame = Vec::with_capacity(total_length);
        frame.extend_from_slice(&(total_length as u32).to_be_bytes());
        frame.extend_from_slice(&(headers.len() as u32).to_be_bytes());
        let prelude_crc = llm_access_kiro::parser::crc::crc32(&frame);
        frame.extend_from_slice(&prelude_crc.to_be_bytes());
        frame.extend_from_slice(&headers);
        frame.extend_from_slice(&payload);
        let message_crc = llm_access_kiro::parser::crc::crc32(&frame);
        frame.extend_from_slice(&message_crc.to_be_bytes());
        frame
    }

    fn push_aws_string_header(headers: &mut Vec<u8>, name: &str, value: &str) {
        headers.push(name.len() as u8);
        headers.extend_from_slice(name.as_bytes());
        headers.push(7);
        headers.extend_from_slice(&(value.len() as u16).to_be_bytes());
        headers.extend_from_slice(value.as_bytes());
    }

    async fn spawn_fake_kiro_upstream(captured: Arc<CapturedKiroUpstream>) -> String {
        let app = Router::new()
            .route("/generateAssistantResponse", post(fake_kiro_generate))
            .with_state(captured);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fake upstream");
        let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve fake upstream");
        });
        upstream_base
    }

    #[tokio::test]
    async fn provider_entry_rejects_missing_bearer_token() {
        let state = test_state();
        let response = super::provider_entry(state, request_with_bearer(None)).await;

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn provider_entry_rejects_malformed_bearer_token() {
        let state = test_state();
        for value in ["valid-secret", "Basic valid-secret", "Bearer "] {
            let response =
                super::provider_entry(state.clone(), request_with_bearer(Some(value))).await;
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
    }

    #[tokio::test]
    async fn provider_entry_rejects_unknown_bearer_token() {
        let state = test_state();
        let response =
            super::provider_entry(state, request_with_bearer(Some("Bearer unknown-secret"))).await;

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn provider_entry_accepts_x_api_key_on_kiro_routes() {
        let dispatcher = Arc::new(CapturingDispatcher::default());
        let state = test_state_with_dispatcher(dispatcher.clone());

        let response = super::provider_entry(
            state,
            request_with_x_api_key_to_path("/api/kiro-gateway/v1/messages", Some("valid-secret")),
        )
        .await;

        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert_eq!(dispatcher.seen.lock().expect("dispatcher state").as_slice(), &[(
            "key-1".to_string(),
            "/api/kiro-gateway/v1/messages".to_string()
        )]);
    }

    #[tokio::test]
    async fn provider_entry_rejects_x_api_key_on_codex_routes() {
        let dispatcher = Arc::new(CapturingDispatcher::default());
        let state = test_state_with_dispatcher(dispatcher.clone());

        let response = super::provider_entry(
            state,
            request_with_x_api_key_to_path("/v1/responses", Some("codex-secret")),
        )
        .await;

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(dispatcher.seen.lock().expect("dispatcher state").is_empty());
    }

    #[tokio::test]
    async fn provider_entry_rejects_non_active_key() {
        let state = test_state();
        let response =
            super::provider_entry(state, request_with_bearer(Some("Bearer paused-secret"))).await;

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn provider_entry_reports_store_errors_as_server_errors() {
        let state = super::ProviderState::new(Arc::new(FailingStore), empty_route_store());
        let response =
            super::provider_entry(state, request_with_bearer(Some("Bearer valid-secret"))).await;

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn provider_entry_accepts_known_bearer_token_before_dispatch() {
        let state = test_state();
        let response =
            super::provider_entry(state, request_with_bearer(Some("Bearer valid-secret"))).await;

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn provider_entry_handler_uses_axum_state() {
        let state = test_state();
        let response = super::provider_entry_handler(
            axum::extract::State(state),
            request_with_bearer(Some("Bearer valid-secret")),
        )
        .await;

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn codex_dispatch_adapts_non_streaming_chat_completion_through_responses_sse() {
        static CODEX_UPSTREAM_ENV_LOCK: Mutex<()> = Mutex::new(());
        let _guard = CODEX_UPSTREAM_ENV_LOCK
            .lock()
            .expect("codex upstream env lock");
        let captured = Arc::new(CapturedCodexUpstream::default());
        let app = Router::new()
            .route("/v1/responses", post(fake_codex_responses))
            .with_state(captured.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fake upstream");
        let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve fake upstream");
        });
        std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);

        let state = super::ProviderState::new(Arc::new(TestStore), static_codex_route_store());
        let response = super::provider_entry(
            state,
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header(header::AUTHORIZATION, "Bearer codex-secret")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{
                        "model": "gpt-5.3-codex",
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": false
                    }"#,
                ))
                .expect("request"),
        )
        .await;

        std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body");
        let body = serde_json::from_slice::<serde_json::Value>(&body).expect("json response");
        assert_eq!(body["object"], "chat.completion");
        assert_eq!(body["model"], "gpt-5.3-codex");
        assert_eq!(body["choices"][0]["message"]["content"], "hello back");
        assert_eq!(body["usage"]["input_tokens"], 12);
        assert_eq!(body["usage"]["output_tokens"], 3);

        let requests = captured.requests.lock().expect("captured requests");
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].path, "/v1/responses");
        assert_eq!(requests[0].authorization.as_deref(), Some("Bearer upstream-token"));
        assert_eq!(requests[0].accept.as_deref(), Some("text/event-stream"));
        assert_eq!(requests[0].body["model"], "gpt-5.3-codex-spark");
        assert_eq!(requests[0].body["stream"], true);
    }

    #[tokio::test]
    async fn codex_dispatch_streams_chat_completion_chunks_from_responses_sse() {
        static CODEX_UPSTREAM_ENV_LOCK: Mutex<()> = Mutex::new(());
        let _guard = CODEX_UPSTREAM_ENV_LOCK
            .lock()
            .expect("codex upstream env lock");
        let captured = Arc::new(CapturedCodexUpstream::default());
        let app = Router::new()
            .route("/v1/responses", post(fake_codex_responses))
            .with_state(captured.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fake upstream");
        let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve fake upstream");
        });
        std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);

        let state = super::ProviderState::new(Arc::new(TestStore), static_codex_route_store());
        let response = super::provider_entry(
            state,
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header(header::AUTHORIZATION, "Bearer codex-secret")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{
                        "model": "gpt-5.3-codex",
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": true
                    }"#,
                ))
                .expect("request"),
        )
        .await;

        std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/event-stream")
        );
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body");
        let body = String::from_utf8(body.to_vec()).expect("utf8 response");
        assert!(body.contains(r#""object":"chat.completion.chunk""#));
        assert!(body.contains(r#""model":"gpt-5.3-codex""#));
        assert!(body.contains(r#""content":"hello ""#));
        assert!(body.contains(r#""content":"back""#));
        assert!(body.contains("data: [DONE]\n\n"));

        let requests = captured.requests.lock().expect("captured requests");
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].path, "/v1/responses");
        assert_eq!(requests[0].accept.as_deref(), Some("text/event-stream"));
        assert_eq!(requests[0].body["stream"], true);
    }

    #[tokio::test]
    async fn codex_dispatch_records_usage_rollup_from_completed_response() {
        static CODEX_UPSTREAM_ENV_LOCK: Mutex<()> = Mutex::new(());
        let _guard = CODEX_UPSTREAM_ENV_LOCK
            .lock()
            .expect("codex upstream env lock");
        let captured = Arc::new(CapturedCodexUpstream::default());
        let app = Router::new()
            .route("/v1/responses", post(fake_codex_responses))
            .with_state(captured);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fake upstream");
        let upstream_base = format!("http://{}", listener.local_addr().expect("local addr"));
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve fake upstream");
        });
        std::env::set_var("CODEX_UPSTREAM_BASE_URL", upstream_base);

        let store = Arc::new(RecordingControlStore::default());
        let state = super::ProviderState::new(store.clone(), static_codex_route_store());
        let response = super::provider_entry(
            state,
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header(header::AUTHORIZATION, "Bearer codex-secret")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{
                        "model": "gpt-5.3-codex",
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": false
                    }"#,
                ))
                .expect("request"),
        )
        .await;

        std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

        assert_eq!(response.status(), StatusCode::OK);
        let events = store.usage_events.lock().expect("usage events");
        assert_eq!(events.len(), 1);
        let event = &events[0];
        assert_eq!(event.key_id, "key-usage");
        assert_eq!(event.key_name, "usage-key");
        assert_eq!(event.account_name.as_deref(), Some("codex-a"));
        assert_eq!(event.endpoint, "/v1/chat/completions");
        assert_eq!(event.model.as_deref(), Some("gpt-5.3-codex"));
        assert_eq!(event.mapped_model.as_deref(), Some("gpt-5.3-codex-spark"));
        assert_eq!(event.status_code, 200);
        assert_eq!(event.input_uncached_tokens, 10);
        assert_eq!(event.input_cached_tokens, 2);
        assert_eq!(event.output_tokens, 3);
        assert_eq!(event.billable_tokens, 25);
        assert!(!event.usage_missing);
    }

    #[tokio::test]
    async fn kiro_dispatch_adapts_non_streaming_messages_from_eventstream() {
        let _guard = KIRO_UPSTREAM_ENV_LOCK
            .lock()
            .expect("kiro upstream env lock");
        let captured = Arc::new(CapturedKiroUpstream::default());
        let upstream_base = spawn_fake_kiro_upstream(captured.clone()).await;
        std::env::set_var("KIRO_UPSTREAM_BASE_URL", upstream_base);

        let state = super::ProviderState::new(Arc::new(TestStore), static_kiro_route_store());
        let response = super::provider_entry(
            state,
            Request::builder()
                .method("POST")
                .uri("/api/kiro-gateway/v1/messages")
                .header("x-api-key", "valid-secret")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{
                        "model": "claude-sonnet-4-6",
                        "max_tokens": 128,
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": false
                    }"#,
                ))
                .expect("request"),
        )
        .await;

        std::env::remove_var("KIRO_UPSTREAM_BASE_URL");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body");
        let body = serde_json::from_slice::<serde_json::Value>(&body).expect("json response");
        assert_eq!(body["type"], "message");
        assert_eq!(body["content"][0]["type"], "text");
        assert_eq!(body["content"][0]["text"], "hello back");
        assert_eq!(body["usage"]["input_tokens"], 50);
        assert_eq!(body["usage"]["cache_creation_input_tokens"], 50);
        assert_eq!(body["usage"]["output_tokens"], 3);

        let requests = captured.requests.lock().expect("captured requests");
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].path, "/generateAssistantResponse");
        assert_eq!(requests[0].authorization.as_deref(), Some("Bearer kiro-upstream-token"));
        assert_eq!(requests[0].body["profileArn"], "arn:aws:kiro:test");
    }

    #[tokio::test]
    async fn kiro_dispatch_streams_messages_from_eventstream() {
        let _guard = KIRO_UPSTREAM_ENV_LOCK
            .lock()
            .expect("kiro upstream env lock");
        let captured = Arc::new(CapturedKiroUpstream::default());
        let upstream_base = spawn_fake_kiro_upstream(captured.clone()).await;
        std::env::set_var("KIRO_UPSTREAM_BASE_URL", upstream_base);

        let state = super::ProviderState::new(Arc::new(TestStore), static_kiro_route_store());
        let response = super::provider_entry(
            state,
            Request::builder()
                .method("POST")
                .uri("/api/kiro-gateway/v1/messages")
                .header("x-api-key", "valid-secret")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{
                        "model": "claude-sonnet-4-6",
                        "max_tokens": 128,
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": true
                    }"#,
                ))
                .expect("request"),
        )
        .await;

        std::env::remove_var("KIRO_UPSTREAM_BASE_URL");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/event-stream")
        );
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body");
        let body = String::from_utf8(body.to_vec()).expect("utf8 response");
        assert!(body.contains("event: message_start"));
        assert!(body.contains("hello "));
        assert!(body.contains("back"));
        assert!(body.contains("event: message_stop"));
    }

    #[tokio::test]
    async fn kiro_dispatch_buffers_cc_stream_until_context_usage_is_available() {
        let _guard = KIRO_UPSTREAM_ENV_LOCK
            .lock()
            .expect("kiro upstream env lock");
        let captured = Arc::new(CapturedKiroUpstream::default());
        let upstream_base = spawn_fake_kiro_upstream(captured).await;
        std::env::set_var("KIRO_UPSTREAM_BASE_URL", upstream_base);

        let state = super::ProviderState::new(Arc::new(TestStore), static_kiro_route_store());
        let response = super::provider_entry(
            state,
            Request::builder()
                .method("POST")
                .uri("/api/kiro-gateway/cc/v1/messages")
                .header("x-api-key", "valid-secret")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{
                        "model": "claude-sonnet-4-6",
                        "max_tokens": 128,
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": true
                    }"#,
                ))
                .expect("request"),
        )
        .await;

        std::env::remove_var("KIRO_UPSTREAM_BASE_URL");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body");
        let body = String::from_utf8(body.to_vec()).expect("utf8 response");
        assert!(body.contains("event: message_start"));
        assert!(body.contains(r#""input_tokens":100"#));
        assert!(body.contains("hello "));
        assert!(body.contains("back"));
    }

    #[tokio::test]
    async fn kiro_dispatch_records_usage_rollup_from_eventstream() {
        let _guard = KIRO_UPSTREAM_ENV_LOCK
            .lock()
            .expect("kiro upstream env lock");
        let captured = Arc::new(CapturedKiroUpstream::default());
        let upstream_base = spawn_fake_kiro_upstream(captured).await;
        std::env::set_var("KIRO_UPSTREAM_BASE_URL", upstream_base);

        let store = Arc::new(RecordingControlStore::default());
        let state = super::ProviderState::new(store.clone(), static_kiro_route_store());
        let response = super::provider_entry(
            state,
            Request::builder()
                .method("POST")
                .uri("/api/kiro-gateway/v1/messages")
                .header("x-api-key", "valid-secret")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{
                        "model": "claude-sonnet-4-6",
                        "max_tokens": 128,
                        "messages": [{"role": "user", "content": "hello"}],
                        "stream": false
                    }"#,
                ))
                .expect("request"),
        )
        .await;

        std::env::remove_var("KIRO_UPSTREAM_BASE_URL");

        assert_eq!(response.status(), StatusCode::OK);
        let events = store.usage_events.lock().expect("usage events");
        assert_eq!(events.len(), 1);
        let event = &events[0];
        assert_eq!(event.provider_type, llm_access_core::provider::ProviderType::Kiro);
        assert_eq!(event.protocol_family, llm_access_core::provider::ProtocolFamily::Anthropic);
        assert_eq!(event.key_id, "key-kiro-usage");
        assert_eq!(event.account_name.as_deref(), Some("kiro-a"));
        assert_eq!(event.endpoint, "/v1/messages");
        assert_eq!(event.model.as_deref(), Some("claude-sonnet-4-6"));
        assert_eq!(event.input_uncached_tokens, 100);
        assert_eq!(event.input_cached_tokens, 0);
        assert_eq!(event.output_tokens, 3);
        assert_eq!(event.billable_tokens, 115);
        assert_eq!(event.credit_usage.as_deref(), Some("0.25"));
        assert!(!event.credit_usage_missing);
    }

    #[tokio::test]
    async fn provider_entry_rejects_kiro_key_on_codex_route_before_dispatch() {
        let dispatcher = Arc::new(CapturingDispatcher::default());
        let state = test_state_with_dispatcher(dispatcher.clone());

        let response = super::provider_entry(
            state,
            request_with_bearer_to_path("/v1/responses", Some("Bearer valid-secret")),
        )
        .await;

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(dispatcher.seen.lock().expect("dispatcher state").is_empty());
    }

    #[tokio::test]
    async fn provider_entry_rejects_codex_key_on_kiro_route_before_dispatch() {
        let dispatcher = Arc::new(CapturingDispatcher::default());
        let state = test_state_with_dispatcher(dispatcher.clone());

        let response = super::provider_entry(
            state,
            request_with_bearer_to_path(
                "/api/kiro-gateway/v1/messages",
                Some("Bearer codex-secret"),
            ),
        )
        .await;

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(dispatcher.seen.lock().expect("dispatcher state").is_empty());
    }

    #[tokio::test]
    async fn provider_entry_rejects_exhausted_kiro_key_before_dispatch() {
        let dispatcher = Arc::new(CapturingDispatcher::default());
        let state = test_state_with_dispatcher(dispatcher.clone());

        let response = super::provider_entry(
            state,
            request_with_bearer_to_path(
                "/api/kiro-gateway/v1/messages",
                Some("Bearer exhausted-kiro-secret"),
            ),
        )
        .await;

        assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);
        assert!(dispatcher.seen.lock().expect("dispatcher state").is_empty());
    }

    #[tokio::test]
    async fn provider_entry_rejects_exhausted_codex_key_before_dispatch() {
        let dispatcher = Arc::new(CapturingDispatcher::default());
        let state = test_state_with_dispatcher(dispatcher.clone());

        let response = super::provider_entry(
            state,
            request_with_bearer_to_path("/v1/responses", Some("Bearer exhausted-codex-secret")),
        )
        .await;

        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(dispatcher.seen.lock().expect("dispatcher state").is_empty());
    }

    #[tokio::test]
    async fn provider_entry_dispatches_authenticated_active_requests() {
        let dispatcher = Arc::new(CapturingDispatcher::default());
        let state = test_state_with_dispatcher(dispatcher.clone());

        let response =
            super::provider_entry(state, request_with_bearer(Some("Bearer valid-secret"))).await;

        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert_eq!(dispatcher.seen.lock().expect("dispatcher state").as_slice(), &[(
            "key-1".to_string(),
            "/api/kiro-gateway/v1/messages".to_string()
        )]);
    }

    #[tokio::test]
    async fn provider_entry_dispatches_codex_key_on_codex_routes() {
        let dispatcher = Arc::new(CapturingDispatcher::default());
        let state = test_state_with_dispatcher(dispatcher.clone());

        let response = super::provider_entry(
            state,
            request_with_bearer_to_path(
                "/api/codex-gateway/v1/responses",
                Some("Bearer codex-secret"),
            ),
        )
        .await;

        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert_eq!(dispatcher.seen.lock().expect("dispatcher state").as_slice(), &[(
            "key-2".to_string(),
            "/api/codex-gateway/v1/responses".to_string()
        )]);
    }

    #[tokio::test]
    async fn provider_entry_serves_codex_models_locally_after_auth() {
        let state = test_state();
        let request = Request::builder()
            .method("GET")
            .uri("/api/llm-gateway/v1/models")
            .header(header::AUTHORIZATION, "Bearer codex-secret")
            .body(Body::empty())
            .expect("request");

        let response = super::provider_entry(state, request).await;

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let body = String::from_utf8(body.to_vec()).expect("utf8 body");
        assert!(body.contains(r#""object":"list""#));
        assert!(body.contains(r#""id":"gpt-5.5""#));
        assert!(body.contains(r#""owned_by":"static-flow""#));
    }
}
