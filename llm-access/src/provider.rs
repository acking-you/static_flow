//! Provider-facing HTTP entrypoints for `llm-access`.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
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
use base64::Engine as _;
use eventsource_stream::Eventsource;
use futures_util::{StreamExt, TryStreamExt};
use llm_access_codex::{
    request::{
        apply_gpt53_codex_spark_mapping, external_origin, extract_client_ip_from_headers,
        extract_last_message_content, prepare_gateway_request_from_bytes,
        resolve_request_url_from_headers, serialize_headers_json,
    },
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
        compute_kiro_billable_tokens, AdminConfigStore, AuthenticatedKey, ControlStore,
        EmptyAdminConfigStore, ProviderCodexRoute, ProviderKiroRoute, ProviderProxyConfig,
        ProviderRouteStore,
    },
    usage::{UsageEvent, UsageTiming},
};
use llm_access_kiro::{
    anthropic::{
        converter::{
            convert_normalized_request_with_resolved_session, current_user_message_range,
            extract_tool_result_content, map_model, normalize_request, preview_session_value,
            resolve_conversation_id_from_metadata, ConversionError, ResolvedConversationId,
            SessionFallbackReason, SessionIdSource, SessionTracking,
        },
        stream::{
            anthropic_usage_json, build_inline_thinking_content_blocks, resolve_input_tokens,
            BufferedStreamContext, StreamContext,
        },
        types::{MessagesRequest, OutputConfig, Thinking},
        websearch::{self, McpResponse},
    },
    auth_file::{
        KiroAuthRecord, DEFAULT_KIRO_VERSION, DEFAULT_NODE_VERSION, DEFAULT_SYSTEM_VERSION,
    },
    cache_policy::{
        adjust_input_tokens_for_cache_creation_cost_with_policy, default_kiro_cache_policy,
        prefix_tree_credit_ratio_cap_basis_points_with_policy, validate_kiro_cache_policy,
        KiroCachePolicy,
    },
    cache_sim::{
        KiroCacheRuntimeStats, KiroCacheSimulationConfig, KiroCacheSimulationMode,
        KiroCacheSimulator, PromptProjection,
    },
    machine_id,
    parser::decoder::EventStreamDecoder,
    scheduler::{KiroRequestLease, KiroRequestScheduler},
    token,
    wire::{
        ConversationState, CurrentMessage, Event, KiroImage, KiroRequest, UserInputMessage,
        UserInputMessageContext,
    },
};
use serde_json::Value;

use crate::{activity::RequestActivityTracker, codex_refresh, kiro_refresh};

const MAX_PROVIDER_PROXY_BODY_BYTES: usize = 32 * 1024 * 1024;
const DEFAULT_WIRE_ORIGINATOR: &str = "codex_cli_rs";
const MAX_CODEX_CLIENT_VERSION_LEN: usize = 64;
const KIRO_PROVIDER_AWS_SDK_VERSION: &str = "1.0.34";
const KIRO_GENERATE_REQUEST_MAX_BODY_BYTES: usize = 1_600_000;
const KIRO_REMOTE_IMAGE_MAX_BYTES: usize = 1_000_000;
const KIRO_REMOTE_DOCUMENT_MAX_BYTES: usize = 8 * 1024 * 1024;
const KIRO_REMOTE_MEDIA_TIMEOUT: Duration = Duration::from_secs(15);
const KIRO_LAST_MESSAGE_PART_PREVIEW_CHARS: usize = 320;
const KIRO_LAST_MESSAGE_TOTAL_PREVIEW_CHARS: usize = 1_024;
const KIRO_VISION_BRIDGE_MODEL: &str = "claude-sonnet-4.6";
const KIRO_VISION_BRIDGE_PROMPT: &str = "Describe the attached image(s) for another Claude model \
                                         that will answer the user's request. Include visible \
                                         text, objects, colors, layout, charts, tables, and \
                                         uncertainty. Return concise numbered visual facts only.";

#[derive(Debug, Clone)]
struct ProviderUsageMetadata {
    started_at: Instant,
    request_method: String,
    request_url: String,
    request_body_bytes: Option<i64>,
    request_body_read_ms: Option<i64>,
    request_json_parse_ms: Option<i64>,
    pre_handler_ms: Option<i64>,
    routing_wait_ms: Option<i64>,
    upstream_headers_ms: Option<i64>,
    post_headers_body_ms: Option<i64>,
    first_sse_write_ms: Option<i64>,
    stream_finish_ms: Option<i64>,
    quota_failover_count: u64,
    routing_diagnostics_json: Option<String>,
    client_ip: String,
    ip_region: String,
    request_headers_json: String,
    last_message_content: Option<String>,
    client_request_body_json: Option<String>,
    upstream_request_body_json: Option<String>,
    full_request_json: Option<String>,
}

impl ProviderUsageMetadata {
    fn from_request_parts(method: &Method, uri: &axum::http::Uri, headers: &HeaderMap) -> Self {
        Self {
            started_at: Instant::now(),
            request_method: method.as_str().to_string(),
            request_url: resolve_request_url_from_headers(headers, uri),
            request_body_bytes: None,
            request_body_read_ms: None,
            request_json_parse_ms: None,
            pre_handler_ms: None,
            routing_wait_ms: None,
            upstream_headers_ms: None,
            post_headers_body_ms: None,
            first_sse_write_ms: None,
            stream_finish_ms: None,
            quota_failover_count: 0,
            routing_diagnostics_json: None,
            client_ip: extract_client_ip_from_headers(headers),
            ip_region: "unknown".to_string(),
            request_headers_json: serialize_headers_json(headers),
            last_message_content: None,
            client_request_body_json: None,
            upstream_request_body_json: None,
            full_request_json: None,
        }
    }

    fn elapsed_ms(&self) -> i64 {
        self.started_at.elapsed().as_millis().min(i64::MAX as u128) as i64
    }

    fn with_request_body(mut self, body: &Bytes, read_ms: i64) -> Self {
        self.request_body_bytes = Some(clamp_usize_to_i64(body.len()));
        self.request_body_read_ms = Some(read_ms);
        self
    }

    fn mark_pre_handler_done(&mut self, parse_ms: i64) {
        self.request_json_parse_ms = Some(parse_ms);
        self.pre_handler_ms = Some(self.elapsed_ms());
    }

    fn mark_upstream_headers(&mut self) {
        self.upstream_headers_ms = Some(self.elapsed_ms());
    }

    fn mark_failover(&mut self) {
        self.quota_failover_count = self.quota_failover_count.saturating_add(1);
    }

    fn add_routing_wait(&mut self, elapsed_ms: i64) {
        self.routing_wait_ms = Some(
            self.routing_wait_ms
                .unwrap_or_default()
                .saturating_add(elapsed_ms),
        );
    }

    fn mark_post_headers_body(&mut self) {
        self.post_headers_body_ms = Some(
            self.elapsed_ms()
                .saturating_sub(self.upstream_headers_ms.unwrap_or_default()),
        );
    }

    fn mark_first_sse_write(&mut self) {
        if self.first_sse_write_ms.is_none() {
            self.first_sse_write_ms = Some(self.elapsed_ms());
        }
    }

    fn mark_stream_finish(&mut self) {
        self.stream_finish_ms = Some(self.elapsed_ms());
    }

    fn to_timing(&self) -> UsageTiming {
        UsageTiming {
            latency_ms: self.stream_finish_ms.or(Some(self.elapsed_ms())),
            routing_wait_ms: self.routing_wait_ms,
            upstream_headers_ms: self.upstream_headers_ms,
            post_headers_body_ms: self.post_headers_body_ms,
            request_body_read_ms: self.request_body_read_ms,
            request_json_parse_ms: self.request_json_parse_ms,
            pre_handler_ms: self.pre_handler_ms,
            first_sse_write_ms: self.first_sse_write_ms,
            stream_finish_ms: self.stream_finish_ms,
        }
    }
}

fn capture_client_request_body_json(meta: &mut ProviderUsageMetadata, body: &[u8]) {
    if meta.client_request_body_json.is_none() {
        meta.client_request_body_json = Some(String::from_utf8_lossy(body).into_owned());
    }
}

fn capture_upstream_request_body_json(meta: &mut ProviderUsageMetadata, body: &[u8]) {
    if meta.upstream_request_body_json.is_none() {
        meta.upstream_request_body_json = Some(String::from_utf8_lossy(body).into_owned());
    }
}

/// Shared provider request state.
#[derive(Clone)]
pub struct ProviderState {
    control_store: Arc<dyn ControlStore>,
    route_store: Arc<dyn ProviderRouteStore>,
    admin_config_store: Arc<dyn AdminConfigStore>,
    dispatcher: Arc<dyn ProviderDispatcher>,
    kiro_cache_simulator: Arc<KiroCacheSimulator>,
    request_limiter: Arc<RequestLimiter>,
    kiro_request_scheduler: Arc<KiroRequestScheduler>,
    request_activity: Arc<RequestActivityTracker>,
}

/// Runtime dependencies passed from the authenticated provider entrypoint into
/// the provider dispatcher.
#[derive(Clone)]
pub struct ProviderDispatchDeps {
    route_store: Arc<dyn ProviderRouteStore>,
    control_store: Arc<dyn ControlStore>,
    admin_config_store: Arc<dyn AdminConfigStore>,
    kiro_cache_simulator: Arc<KiroCacheSimulator>,
    request_limiter: Arc<RequestLimiter>,
    kiro_request_scheduler: Arc<KiroRequestScheduler>,
}

impl ProviderState {
    /// Create provider request state.
    pub fn new(
        control_store: Arc<dyn ControlStore>,
        route_store: Arc<dyn ProviderRouteStore>,
    ) -> Self {
        Self::with_dispatcher(control_store, route_store, Arc::new(DefaultProviderDispatcher))
    }

    /// Create provider request state with an explicit admin runtime config
    /// source.
    pub fn new_with_config_store(
        control_store: Arc<dyn ControlStore>,
        route_store: Arc<dyn ProviderRouteStore>,
        admin_config_store: Arc<dyn AdminConfigStore>,
    ) -> Self {
        Self::new_with_config_store_and_activity(
            control_store,
            route_store,
            admin_config_store,
            Arc::new(RequestActivityTracker::new()),
        )
    }

    pub(crate) fn new_with_config_store_and_activity(
        control_store: Arc<dyn ControlStore>,
        route_store: Arc<dyn ProviderRouteStore>,
        admin_config_store: Arc<dyn AdminConfigStore>,
        request_activity: Arc<RequestActivityTracker>,
    ) -> Self {
        Self::with_dispatcher_and_config_store(
            control_store,
            route_store,
            admin_config_store,
            Arc::new(DefaultProviderDispatcher),
            request_activity,
        )
    }

    /// Create provider request state with an explicit dispatcher.
    pub fn with_dispatcher(
        control_store: Arc<dyn ControlStore>,
        route_store: Arc<dyn ProviderRouteStore>,
        dispatcher: Arc<dyn ProviderDispatcher>,
    ) -> Self {
        Self::with_dispatcher_and_config_store(
            control_store,
            route_store,
            Arc::new(EmptyAdminConfigStore),
            dispatcher,
            Arc::new(RequestActivityTracker::new()),
        )
    }

    fn with_dispatcher_and_config_store(
        control_store: Arc<dyn ControlStore>,
        route_store: Arc<dyn ProviderRouteStore>,
        admin_config_store: Arc<dyn AdminConfigStore>,
        dispatcher: Arc<dyn ProviderDispatcher>,
        request_activity: Arc<RequestActivityTracker>,
    ) -> Self {
        Self {
            control_store,
            route_store,
            admin_config_store,
            dispatcher,
            kiro_cache_simulator: Arc::new(KiroCacheSimulator::default()),
            request_limiter: Arc::new(RequestLimiter::default()),
            kiro_request_scheduler: KiroRequestScheduler::new(),
            request_activity,
        }
    }

    pub(crate) fn route_store(&self) -> Arc<dyn ProviderRouteStore> {
        Arc::clone(&self.route_store)
    }

    pub(crate) fn kiro_cache_stats(
        &self,
        config: KiroCacheSimulationConfig,
    ) -> KiroCacheRuntimeStats {
        self.kiro_cache_simulator
            .snapshot_stats(config, Instant::now())
    }

    fn dispatch_deps(&self) -> ProviderDispatchDeps {
        ProviderDispatchDeps {
            route_store: Arc::clone(&self.route_store),
            control_store: Arc::clone(&self.control_store),
            admin_config_store: Arc::clone(&self.admin_config_store),
            kiro_cache_simulator: Arc::clone(&self.kiro_cache_simulator),
            request_limiter: Arc::clone(&self.request_limiter),
            kiro_request_scheduler: Arc::clone(&self.kiro_request_scheduler),
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

#[derive(Debug, Clone)]
struct LimitRejection {
    reason: &'static str,
    in_flight: u64,
    max_concurrency: Option<u64>,
    min_start_interval_ms: Option<u64>,
    wait: Option<Duration>,
    elapsed_since_last_start_ms: Option<u64>,
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
    fn try_acquire(
        self: &Arc<Self>,
        scope: String,
        max_concurrency: Option<u64>,
        min_start_interval_ms: Option<u64>,
    ) -> Result<LimitPermit, LimitRejection> {
        let max_concurrency = max_concurrency.filter(|value| *value > 0);
        let min_interval = min_start_interval_ms
            .filter(|value| *value > 0)
            .map(Duration::from_millis);
        let mut scopes = self.scopes.lock().expect("request limiter mutex poisoned");
        let state = scopes.entry(scope.clone()).or_default();
        let concurrency_ready = max_concurrency
            .map(|limit| state.in_flight < limit)
            .unwrap_or(true);
        let elapsed_since_last_start = state.last_start.map(|last_start| last_start.elapsed());
        let interval_wait = min_interval.and_then(|interval| {
            elapsed_since_last_start.and_then(|elapsed| interval.checked_sub(elapsed))
        });
        if concurrency_ready && interval_wait.is_none() {
            state.in_flight = state.in_flight.saturating_add(1);
            state.last_start = Some(Instant::now());
            return Ok(LimitPermit {
                limiter: Arc::clone(self),
                scope,
            });
        }
        let reason = if !concurrency_ready { "max_concurrency" } else { "min_start_interval" };
        Err(LimitRejection {
            reason,
            in_flight: state.in_flight,
            max_concurrency,
            min_start_interval_ms,
            wait: interval_wait.or_else(|| Some(Duration::from_millis(10))),
            elapsed_since_last_start_ms: elapsed_since_last_start
                .map(|elapsed| elapsed.as_millis().min(u128::from(u64::MAX)) as u64),
        })
    }
}

fn try_acquire_key_permit(
    limiter: &Arc<RequestLimiter>,
    key: &AuthenticatedKey,
    max_concurrency: Option<u64>,
    min_start_interval_ms: Option<u64>,
) -> Result<LimitPermit, LimitRejection> {
    limiter.try_acquire(format!("key:{}", key.key_id), max_concurrency, min_start_interval_ms)
}

async fn wait_for_limit(rejection: Option<&LimitRejection>) {
    tokio::time::sleep(
        rejection
            .and_then(|rejection| rejection.wait)
            .unwrap_or_else(|| Duration::from_millis(10)),
    )
    .await;
}

fn codex_key_limit_response(rejection: &LimitRejection) -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        format!(
            "key request limit reached: {} in_flight={} request_max_concurrency={} \
             request_min_start_interval_ms={} wait_ms={} elapsed_since_last_start_ms={}",
            rejection.reason,
            rejection.in_flight,
            rejection
                .max_concurrency
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unlimited".to_string()),
            rejection
                .min_start_interval_ms
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unlimited".to_string()),
            rejection
                .wait
                .map(|value| value.as_millis() as u64)
                .unwrap_or(0),
            rejection.elapsed_since_last_start_ms.unwrap_or(0),
        ),
    )
        .into_response()
}

fn kiro_key_limit_response(rejection: &LimitRejection) -> Response {
    kiro_json_error(
        StatusCode::TOO_MANY_REQUESTS,
        "rate_limit_error",
        &format!(
            "Kiro key request limit reached: {} in_flight={} request_max_concurrency={} \
             request_min_start_interval_ms={} wait_ms={}",
            rejection.reason,
            rejection.in_flight,
            rejection
                .max_concurrency
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unlimited".to_string()),
            rejection
                .min_start_interval_ms
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unlimited".to_string()),
            rejection
                .wait
                .map(|value| value.as_millis() as u64)
                .unwrap_or(0),
        ),
    )
}

async fn select_codex_route_with_account_permit(
    limiter: &Arc<RequestLimiter>,
    routes: &[ProviderCodexRoute],
    failed_accounts: &HashSet<String>,
) -> Result<(ProviderCodexRoute, LimitPermit), Response> {
    if routes.is_empty() {
        return Err(
            (StatusCode::SERVICE_UNAVAILABLE, "codex route is not configured").into_response()
        );
    }
    loop {
        let mut saw_limit = false;
        let mut shortest_wait: Option<LimitRejection> = None;
        for route in routes {
            if failed_accounts.contains(&route.account_name) {
                continue;
            }
            match limiter.try_acquire(
                format!("account:{}:{}", ProviderType::Codex.as_storage_str(), route.account_name),
                route.account_request_max_concurrency,
                route.account_request_min_start_interval_ms,
            ) {
                Ok(permit) => return Ok((route.clone(), permit)),
                Err(rejection) => {
                    saw_limit = true;
                    if shortest_wait
                        .as_ref()
                        .and_then(|current| current.wait)
                        .map(|current| rejection.wait.unwrap_or(current) < current)
                        .unwrap_or(true)
                    {
                        shortest_wait = Some(rejection);
                    }
                },
            }
        }
        if !failed_accounts.is_empty()
            && routes
                .iter()
                .all(|route| failed_accounts.contains(&route.account_name))
        {
            return Err((
                StatusCode::BAD_GATEWAY,
                "all eligible codex accounts failed for this request",
            )
                .into_response());
        }
        if saw_limit {
            wait_for_limit(shortest_wait.as_ref()).await;
            continue;
        }
        return Err((StatusCode::SERVICE_UNAVAILABLE, "no usable codex account is configured")
            .into_response());
    }
}

async fn select_kiro_route_with_account_permit(
    scheduler: &Arc<KiroRequestScheduler>,
    routes: &[ProviderKiroRoute],
    failed_accounts: &HashSet<String>,
) -> Result<(ProviderKiroRoute, KiroRequestLease), Response> {
    if routes.is_empty() {
        return Err(kiro_json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "api_error",
            "kiro route is not configured",
        ));
    }
    let queued_at = Instant::now();
    loop {
        let mut saw_limit = false;
        let mut shortest_wait: Option<Duration> = None;
        for route in selection_ordered_kiro_routes(routes, scheduler) {
            if failed_accounts.contains(&route.account_name) {
                continue;
            }
            if let Some(cooldown) = scheduler.cooldown_for_account(&route.routing_identity) {
                saw_limit = true;
                shortest_wait = Some(match shortest_wait {
                    Some(current) => current.min(cooldown.remaining),
                    None => cooldown.remaining,
                });
                continue;
            }
            match scheduler.try_acquire(
                &route.routing_identity,
                route
                    .account_request_max_concurrency
                    .unwrap_or(llm_access_core::store::DEFAULT_KIRO_CHANNEL_MAX_CONCURRENCY),
                route
                    .account_request_min_start_interval_ms
                    .unwrap_or(llm_access_core::store::DEFAULT_KIRO_CHANNEL_MIN_START_INTERVAL_MS),
                queued_at,
            ) {
                Ok(permit) => return Ok((route.clone(), permit)),
                Err(rejection) => {
                    saw_limit = true;
                    if let Some(wait) = rejection.wait {
                        shortest_wait = Some(match shortest_wait {
                            Some(current) => current.min(wait),
                            None => wait,
                        });
                    }
                },
            }
        }
        if !failed_accounts.is_empty()
            && routes
                .iter()
                .all(|route| failed_accounts.contains(&route.account_name))
        {
            return Err(kiro_json_error(
                StatusCode::BAD_GATEWAY,
                "api_error",
                "all eligible kiro accounts failed for this request",
            ));
        }
        if saw_limit {
            scheduler.wait_for_available(shortest_wait).await;
            continue;
        }
        return Err(kiro_json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "api_error",
            "no usable kiro account is configured",
        ));
    }
}

fn selection_ordered_kiro_routes<'a>(
    routes: &'a [ProviderKiroRoute],
    scheduler: &KiroRequestScheduler,
) -> Vec<&'a ProviderKiroRoute> {
    #[derive(Clone, Copy)]
    struct Candidate<'a> {
        route: &'a ProviderKiroRoute,
        proxy_in_cooldown: bool,
        last_started_at: Option<Instant>,
        remaining: f64,
    }

    let last_started_snapshot = scheduler.last_started_snapshot();
    let proxy_cooldowns = scheduler.proxy_cooldown_snapshot();
    let mut sorted = routes
        .iter()
        .map(|route| {
            let proxy_key = proxy_cooldown_key_for_route(route);
            Candidate {
                route,
                proxy_in_cooldown: proxy_key
                    .as_deref()
                    .is_some_and(|key| proxy_cooldowns.contains_key(key)),
                last_started_at: last_started_snapshot.get(&route.routing_identity).copied(),
                remaining: route.cached_remaining_credits.unwrap_or(-1.0),
            }
        })
        .collect::<Vec<_>>();
    sorted.sort_by(|left, right| {
        match (left.proxy_in_cooldown, right.proxy_in_cooldown) {
            (false, true) => return std::cmp::Ordering::Less,
            (true, false) => return std::cmp::Ordering::Greater,
            _ => {},
        }
        match (left.last_started_at, right.last_started_at) {
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(left_started), Some(right_started)) => {
                let ordering = left_started.cmp(&right_started);
                if ordering != std::cmp::Ordering::Equal {
                    return ordering;
                }
            },
            (None, None) => {},
        }
        right
            .remaining
            .total_cmp(&left.remaining)
            .then_with(|| left.route.account_name.cmp(&right.route.account_name))
    });
    sorted
        .into_iter()
        .map(|candidate| candidate.route)
        .collect()
}

/// Provider runtime dispatch after key authentication succeeds.
#[async_trait]
pub trait ProviderDispatcher: Send + Sync {
    /// Dispatch an authenticated request to the selected provider runtime.
    async fn dispatch(
        &self,
        key: AuthenticatedKey,
        request: Request<Body>,
        deps: ProviderDispatchDeps,
    ) -> Response;
}

struct DefaultProviderDispatcher;

#[async_trait]
impl ProviderDispatcher for DefaultProviderDispatcher {
    async fn dispatch(
        &self,
        key: AuthenticatedKey,
        request: Request<Body>,
        deps: ProviderDispatchDeps,
    ) -> Response {
        if ProviderType::from_storage_str(&key.provider_type) == Some(ProviderType::Codex) {
            return dispatch_codex_proxy(
                key,
                request,
                deps.route_store,
                deps.control_store,
                deps.admin_config_store,
                deps.request_limiter,
            )
            .await;
        }
        if ProviderType::from_storage_str(&key.provider_type) == Some(ProviderType::Kiro) {
            return dispatch_kiro_proxy(
                key,
                request,
                deps.route_store,
                deps.control_store,
                deps.kiro_cache_simulator,
                deps.request_limiter,
                deps.kiro_request_scheduler,
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
    admin_config_store: Arc<dyn AdminConfigStore>,
    request_limiter: Arc<RequestLimiter>,
) -> Response {
    let mut usage_meta = ProviderUsageMetadata::from_request_parts(
        request.method(),
        request.uri(),
        request.headers(),
    );
    let routes = match route_store.resolve_codex_route_candidates(&key).await {
        Ok(routes) if !routes.is_empty() => routes,
        Ok(_) => {
            return (StatusCode::SERVICE_UNAVAILABLE, "codex route is not configured")
                .into_response()
        },
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "codex route resolution failed")
                .into_response()
        },
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
    let upstream_base = codex_upstream_base_url();
    let method = request.method().clone();
    let request_headers = request.headers().clone();
    let codex_client_version = match load_codex_client_version(admin_config_store.as_ref()).await {
        Ok(version) => version,
        Err(response) => return response,
    };

    if gateway_path == "/v1/models" && method == Method::GET {
        let Some(route) = routes.into_iter().next() else {
            return (StatusCode::SERVICE_UNAVAILABLE, "codex route is not configured")
                .into_response();
        };
        return codex_openai_models_response(
            route,
            route_store,
            &request_headers,
            query.trim_start_matches('?'),
            &upstream_base,
            &codex_client_version,
        )
        .await;
    }

    let body_read_started = Instant::now();
    let body = match to_bytes(request.into_body(), MAX_PROVIDER_PROXY_BODY_BYTES).await {
        Ok(body) => body,
        Err(_) => return (StatusCode::BAD_REQUEST, "request body is too large").into_response(),
    };
    usage_meta =
        usage_meta.with_request_body(&body, clamp_duration_ms(body_read_started.elapsed()));
    let parse_started = Instant::now();
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
    usage_meta.mark_pre_handler_done(clamp_duration_ms(parse_started.elapsed()));
    usage_meta.last_message_content = extract_last_message_content(&prepared.client_request_body)
        .ok()
        .flatten();
    let method = match reqwest::Method::from_bytes(prepared.method.as_str().as_bytes()) {
        Ok(method) => method,
        Err(_) => return (StatusCode::METHOD_NOT_ALLOWED, "unsupported method").into_response(),
    };
    let key_permit = match try_acquire_key_permit(
        &request_limiter,
        &key,
        routes[0].request_max_concurrency,
        routes[0].request_min_start_interval_ms,
    ) {
        Ok(permit) => permit,
        Err(rejection) => return codex_key_limit_response(&rejection),
    };
    let mut key_permit = Some(key_permit);
    let mut failed_accounts = HashSet::new();
    loop {
        let route_started = Instant::now();
        let (route, account_permit) = match select_codex_route_with_account_permit(
            &request_limiter,
            &routes,
            &failed_accounts,
        )
        .await
        {
            Ok(value) => value,
            Err(response) => return response,
        };
        usage_meta.add_routing_wait(clamp_duration_ms(route_started.elapsed()));
        let mut auth = match codex_refresh::ensure_context_for_route(
            &route,
            route_store.as_ref(),
            false,
        )
        .await
        {
            Ok(ctx) => CodexAuthSnapshot {
                access_token: ctx.access_token,
                account_id: ctx.account_id,
                is_fedramp_account: ctx.is_fedramp_account,
            },
            Err(_) => {
                usage_meta.mark_failover();
                failed_accounts.insert(route.account_name);
                continue;
            },
        };
        let prepared =
            match apply_gpt53_codex_spark_mapping(&prepared, route.map_gpt53_codex_to_spark) {
                Ok(prepared) => prepared,
                Err(err) => return (err.status, err.message).into_response(),
            };
        let upstream_url = compute_codex_upstream_url(&upstream_base, &prepared.upstream_path);
        let client = match provider_client(route.proxy.as_ref()) {
            Ok(client) => client,
            Err(_) => {
                usage_meta.mark_failover();
                failed_accounts.insert(route.account_name);
                continue;
            },
        };
        let upstream = add_codex_upstream_headers(
            client.request(method.clone(), upstream_url.clone()),
            &request_headers,
            &prepared,
            &auth,
            &codex_client_version,
        );
        let mut response = match upstream.send().await {
            Ok(response) => {
                usage_meta.mark_upstream_headers();
                response
            },
            Err(_) => {
                usage_meta.mark_failover();
                failed_accounts.insert(route.account_name);
                continue;
            },
        };
        if matches!(response.status(), StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
            match codex_refresh::ensure_context_for_route(&route, route_store.as_ref(), true).await
            {
                Ok(ctx) => {
                    auth = CodexAuthSnapshot {
                        access_token: ctx.access_token,
                        account_id: ctx.account_id,
                        is_fedramp_account: ctx.is_fedramp_account,
                    };
                    let retry = add_codex_upstream_headers(
                        client.request(method.clone(), upstream_url),
                        &request_headers,
                        &prepared,
                        &auth,
                        &codex_client_version,
                    );
                    response = match retry.send().await {
                        Ok(response) => {
                            usage_meta.mark_upstream_headers();
                            response
                        },
                        Err(_) => {
                            usage_meta.mark_failover();
                            failed_accounts.insert(route.account_name);
                            continue;
                        },
                    };
                },
                Err(_) => {
                    usage_meta.mark_failover();
                    failed_accounts.insert(route.account_name);
                    continue;
                },
            }
        }
        if !response.status().is_success()
            && routes.iter().any(|candidate| {
                !failed_accounts.contains(&candidate.account_name)
                    && candidate.account_name != route.account_name
            })
        {
            usage_meta.mark_failover();
            failed_accounts.insert(route.account_name);
            continue;
        }
        let permits = vec![
            key_permit
                .take()
                .expect("codex key permit should be held until response is returned"),
            account_permit,
        ];
        return adapt_codex_upstream_response(
            prepared,
            response,
            key,
            route,
            control_store,
            permits,
            usage_meta,
        )
        .await;
    }
}

async fn adapt_codex_upstream_response(
    prepared: PreparedGatewayRequest,
    response: reqwest::Response,
    key: AuthenticatedKey,
    route: ProviderCodexRoute,
    control_store: Arc<dyn ControlStore>,
    permits: Vec<LimitPermit>,
    mut usage_meta: ProviderUsageMetadata,
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
        usage_meta.mark_post_headers_body();
        usage_meta.mark_stream_finish();
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
            &route,
            completed.usage.unwrap_or_else(missing_codex_usage),
            &usage_meta,
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
                route,
                control_store,
                permits,
                usage_meta,
            },
        );
    }

    let bytes = match response.bytes().await {
        Ok(bytes) => bytes,
        Err(_) => {
            return (StatusCode::BAD_GATEWAY, "codex upstream response read failed").into_response()
        },
    };
    usage_meta.mark_post_headers_body();
    usage_meta.mark_stream_finish();
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
    let usage = if status.is_success() {
        extract_usage_from_bytes(&bytes).unwrap_or_else(missing_codex_usage)
    } else {
        missing_codex_usage()
    };
    if let Err(err) = record_codex_usage(
        control_store.as_ref(),
        &key,
        &prepared,
        status,
        &route,
        usage,
        &usage_meta,
    )
    .await
    {
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("failed to record codex usage: {err}"))
            .into_response();
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
    route: ProviderCodexRoute,
    control_store: Arc<dyn ControlStore>,
    permits: Vec<LimitPermit>,
    usage_meta: ProviderUsageMetadata,
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
            route,
            control_store,
            permits,
            mut usage_meta,
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
                            usage_meta.mark_first_sse_write();
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
                                usage_meta.mark_first_sse_write();
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
            usage_meta.mark_first_sse_write();
            yield Ok::<Bytes, std::io::Error>(Bytes::from_static(b"data: [DONE]\n\n"));
        }
        usage_meta.mark_post_headers_body();
        usage_meta.mark_stream_finish();
        if let Err(err) = record_codex_usage(
            control_store.as_ref(),
            &key,
            &prepared,
            status,
            &route,
            usage_collector.usage.unwrap_or_else(missing_codex_usage),
            &usage_meta,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KiroRemoteMediaKind {
    Image,
    Document,
}

#[derive(Debug, Clone, Copy)]
struct KiroRemoteMediaRequest<'a> {
    url: &'a str,
    kind: KiroRemoteMediaKind,
}

#[derive(Debug)]
struct ResolvedKiroRemoteMedia {
    media_type: Option<String>,
    bytes: Bytes,
}

#[derive(Debug)]
struct KiroRemoteMediaResolutionError {
    message: String,
}

impl KiroRemoteMediaResolutionError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    fn with_context(self, context: impl AsRef<str>) -> Self {
        Self {
            message: format!("{}: {}", context.as_ref(), self.message),
        }
    }
}

impl std::fmt::Display for KiroRemoteMediaResolutionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

#[async_trait]
trait KiroRemoteMediaFetcher: Sync {
    async fn fetch(
        &self,
        request: KiroRemoteMediaRequest<'_>,
    ) -> Result<ResolvedKiroRemoteMedia, KiroRemoteMediaResolutionError>;
}

struct ReqwestKiroRemoteMediaFetcher {
    client: reqwest::Client,
}

#[async_trait]
impl KiroRemoteMediaFetcher for ReqwestKiroRemoteMediaFetcher {
    async fn fetch(
        &self,
        request: KiroRemoteMediaRequest<'_>,
    ) -> Result<ResolvedKiroRemoteMedia, KiroRemoteMediaResolutionError> {
        let url = validate_kiro_remote_media_url(request.url)?;
        validate_kiro_remote_media_resolved_addresses(&url).await?;
        let max_bytes = match request.kind {
            KiroRemoteMediaKind::Image => KIRO_REMOTE_IMAGE_MAX_BYTES,
            KiroRemoteMediaKind::Document => KIRO_REMOTE_DOCUMENT_MAX_BYTES,
        };
        let response = self
            .client
            .get(url.clone())
            .header(reqwest::header::ACCEPT, kiro_remote_media_accept_header(request.kind))
            .send()
            .await
            .map_err(|err| {
                KiroRemoteMediaResolutionError::new(format!("failed to fetch URL source: {err}"))
            })?;
        if !response.status().is_success() {
            return Err(KiroRemoteMediaResolutionError::new(format!(
                "URL source returned HTTP {}",
                response.status()
            )));
        }
        if response
            .content_length()
            .is_some_and(|length| length > max_bytes as u64)
        {
            return Err(KiroRemoteMediaResolutionError::new(format!(
                "URL source exceeds {} byte limit",
                max_bytes
            )));
        }
        let media_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(normalize_media_type);
        let bytes = response.bytes().await.map_err(|err| {
            KiroRemoteMediaResolutionError::new(format!("failed to read URL source body: {err}"))
        })?;
        if bytes.len() > max_bytes {
            return Err(KiroRemoteMediaResolutionError::new(format!(
                "URL source exceeds {} byte limit",
                max_bytes
            )));
        }
        if bytes.is_empty() {
            return Err(KiroRemoteMediaResolutionError::new("URL source body is empty"));
        }
        Ok(ResolvedKiroRemoteMedia {
            media_type,
            bytes,
        })
    }
}

fn kiro_remote_media_accept_header(kind: KiroRemoteMediaKind) -> &'static str {
    match kind {
        KiroRemoteMediaKind::Image => "image/jpeg,image/png,image/gif,image/webp",
        KiroRemoteMediaKind::Document => "application/pdf,text/plain,text/*",
    }
}

async fn resolve_kiro_remote_media_sources(
    payload: &mut MessagesRequest,
) -> Result<(), KiroRemoteMediaResolutionError> {
    if !payload_has_kiro_remote_media_sources(payload) {
        return Ok(());
    }
    let client = reqwest::Client::builder()
        .timeout(KIRO_REMOTE_MEDIA_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|err| {
            KiroRemoteMediaResolutionError::new(format!(
                "failed to initialize URL source client: {err}"
            ))
        })?;
    let fetcher = ReqwestKiroRemoteMediaFetcher {
        client,
    };
    resolve_kiro_remote_media_sources_with_fetcher(payload, &fetcher).await
}

fn payload_has_kiro_remote_media_sources(payload: &MessagesRequest) -> bool {
    payload.messages.iter().any(|message| {
        message.role == "user"
            && message
                .content
                .as_array()
                .is_some_and(|items| items.iter().any(is_kiro_remote_media_source_block))
    })
}

fn is_kiro_remote_media_source_block(item: &serde_json::Value) -> bool {
    let Some(object) = item.as_object() else {
        return false;
    };
    let Some("image" | "document") = object.get("type").and_then(serde_json::Value::as_str) else {
        return false;
    };
    object
        .get("source")
        .and_then(serde_json::Value::as_object)
        .and_then(|source| source.get("type"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        == Some("url")
}

async fn resolve_kiro_remote_media_sources_with_fetcher(
    payload: &mut MessagesRequest,
    fetcher: &(dyn KiroRemoteMediaFetcher + Sync),
) -> Result<(), KiroRemoteMediaResolutionError> {
    for (message_index, message) in payload.messages.iter_mut().enumerate() {
        if message.role != "user" {
            continue;
        }
        let Some(items) = message.content.as_array_mut() else {
            continue;
        };
        for (block_index, item) in items.iter_mut().enumerate() {
            let Some(source) = pending_kiro_remote_media_source(item, message_index, block_index)?
            else {
                continue;
            };
            let remote = fetcher
                .fetch(KiroRemoteMediaRequest {
                    url: &source.url,
                    kind: source.kind,
                })
                .await
                .map_err(|err| {
                    err.with_context(format!(
                        "message {message_index} {} block {block_index}",
                        source.block_type
                    ))
                })?;
            let replacement = match source.kind {
                KiroRemoteMediaKind::Image => build_kiro_remote_image_source(
                    source.source_media_type.as_deref(),
                    remote.media_type.as_deref(),
                    &source.url,
                    &remote.bytes,
                )?,
                KiroRemoteMediaKind::Document => build_kiro_remote_document_source(
                    source.source_media_type.as_deref(),
                    remote.media_type.as_deref(),
                    &source.url,
                    &remote.bytes,
                )?,
            };
            if let Some(object) = item.as_object_mut() {
                object.insert("source".to_string(), replacement);
            }
        }
    }
    Ok(())
}

struct PendingKiroRemoteMediaSource {
    kind: KiroRemoteMediaKind,
    block_type: &'static str,
    url: String,
    source_media_type: Option<String>,
}

fn pending_kiro_remote_media_source(
    item: &serde_json::Value,
    message_index: usize,
    block_index: usize,
) -> Result<Option<PendingKiroRemoteMediaSource>, KiroRemoteMediaResolutionError> {
    let Some(object) = item.as_object() else {
        return Ok(None);
    };
    let Some(block_type) = object.get("type").and_then(serde_json::Value::as_str) else {
        return Ok(None);
    };
    let (kind, block_type) = match block_type {
        "image" => (KiroRemoteMediaKind::Image, "image"),
        "document" => (KiroRemoteMediaKind::Document, "document"),
        _ => return Ok(None),
    };
    let Some(source) = object.get("source").and_then(serde_json::Value::as_object) else {
        return Ok(None);
    };
    let Some(source_type) = source
        .get("type")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
    else {
        return Ok(None);
    };
    if source_type != "url" {
        return Ok(None);
    }
    let Some(url) = source
        .get("url")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Err(KiroRemoteMediaResolutionError::new(format!(
            "message {message_index} {block_type} block {block_index} URL source is missing url"
        )));
    };
    Ok(Some(PendingKiroRemoteMediaSource {
        kind,
        block_type,
        url: url.to_string(),
        source_media_type: source
            .get("media_type")
            .and_then(serde_json::Value::as_str)
            .and_then(normalize_media_type),
    }))
}

fn build_kiro_remote_image_source(
    source_media_type: Option<&str>,
    response_media_type: Option<&str>,
    url: &str,
    bytes: &[u8],
) -> Result<serde_json::Value, KiroRemoteMediaResolutionError> {
    if bytes.is_empty() {
        return Err(KiroRemoteMediaResolutionError::new("URL source body is empty"));
    }
    let media_type = response_media_type
        .and_then(canonical_image_media_type)
        .or_else(|| source_media_type.and_then(canonical_image_media_type))
        .or_else(|| image_media_type_from_url(url))
        .ok_or_else(|| {
            KiroRemoteMediaResolutionError::new(
                "URL image source must resolve to image/jpeg, image/png, image/gif, or image/webp",
            )
        })?;
    Ok(serde_json::json!({
        "type": "base64",
        "media_type": media_type,
        "data": base64::engine::general_purpose::STANDARD.encode(bytes)
    }))
}

fn build_kiro_remote_document_source(
    source_media_type: Option<&str>,
    response_media_type: Option<&str>,
    url: &str,
    bytes: &[u8],
) -> Result<serde_json::Value, KiroRemoteMediaResolutionError> {
    if bytes.is_empty() {
        return Err(KiroRemoteMediaResolutionError::new("URL source body is empty"));
    }
    let media_type = response_media_type
        .and_then(canonical_document_media_type)
        .or_else(|| source_media_type.and_then(canonical_document_media_type))
        .or_else(|| document_media_type_from_url(url))
        .ok_or_else(|| {
            KiroRemoteMediaResolutionError::new(
                "URL document source must resolve to application/pdf or text/plain",
            )
        })?;
    match media_type {
        "application/pdf" => Ok(serde_json::json!({
            "type": "base64",
            "media_type": "application/pdf",
            "data": base64::engine::general_purpose::STANDARD.encode(bytes)
        })),
        "text/plain" => {
            let text = std::str::from_utf8(bytes).map_err(|err| {
                KiroRemoteMediaResolutionError::new(format!(
                    "URL text document source is not valid UTF-8: {err}"
                ))
            })?;
            Ok(serde_json::json!({
                "type": "text",
                "media_type": "text/plain",
                "data": text
            }))
        },
        _ => unreachable!("document media type is normalized to the supported set"),
    }
}

fn normalize_media_type(value: &str) -> Option<String> {
    value
        .split(';')
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
}

fn canonical_image_media_type(media_type: &str) -> Option<&'static str> {
    match media_type {
        "image/jpeg" | "image/jpg" => Some("image/jpeg"),
        "image/png" => Some("image/png"),
        "image/gif" => Some("image/gif"),
        "image/webp" => Some("image/webp"),
        _ => None,
    }
}

fn canonical_document_media_type(media_type: &str) -> Option<&'static str> {
    match media_type {
        "application/pdf" => Some("application/pdf"),
        "text/plain" => Some("text/plain"),
        value if value.starts_with("text/") => Some("text/plain"),
        _ => None,
    }
}

fn image_media_type_from_url(url: &str) -> Option<&'static str> {
    match lower_url_path_extension(url).as_deref() {
        Some("jpg" | "jpeg") => Some("image/jpeg"),
        Some("png") => Some("image/png"),
        Some("gif") => Some("image/gif"),
        Some("webp") => Some("image/webp"),
        _ => None,
    }
}

fn document_media_type_from_url(url: &str) -> Option<&'static str> {
    match lower_url_path_extension(url).as_deref() {
        Some("pdf") => Some("application/pdf"),
        Some("txt") => Some("text/plain"),
        _ => None,
    }
}

fn lower_url_path_extension(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    parsed
        .path_segments()
        .and_then(Iterator::last)
        .and_then(|name| {
            name.rsplit_once('.')
                .map(|(_, ext)| ext.to_ascii_lowercase())
        })
}

fn validate_kiro_remote_media_url(
    raw_url: &str,
) -> Result<url::Url, KiroRemoteMediaResolutionError> {
    let url = url::Url::parse(raw_url)
        .map_err(|err| KiroRemoteMediaResolutionError::new(format!("invalid URL source: {err}")))?;
    match url.scheme() {
        "http" | "https" => {},
        _ => {
            return Err(KiroRemoteMediaResolutionError::new(
                "URL source scheme must be http or https",
            ))
        },
    }
    let host = url
        .host_str()
        .ok_or_else(|| KiroRemoteMediaResolutionError::new("URL source is missing host"))?;
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".localhost") {
        return Err(KiroRemoteMediaResolutionError::new("URL source host must not be localhost"));
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        reject_private_kiro_remote_media_ip(ip)?;
    }
    Ok(url)
}

async fn validate_kiro_remote_media_resolved_addresses(
    url: &url::Url,
) -> Result<(), KiroRemoteMediaResolutionError> {
    let host = url
        .host_str()
        .ok_or_else(|| KiroRemoteMediaResolutionError::new("URL source is missing host"))?;
    if host.parse::<IpAddr>().is_ok() {
        return Ok(());
    }
    let port = url
        .port_or_known_default()
        .ok_or_else(|| KiroRemoteMediaResolutionError::new("URL source is missing port"))?;
    let addresses = tokio::net::lookup_host((host, port)).await.map_err(|err| {
        KiroRemoteMediaResolutionError::new(format!("failed to resolve URL source host: {err}"))
    })?;
    let mut resolved_any = false;
    for address in addresses {
        resolved_any = true;
        reject_private_kiro_remote_media_ip(address.ip())?;
    }
    if !resolved_any {
        return Err(KiroRemoteMediaResolutionError::new(
            "URL source host resolved to no addresses",
        ));
    }
    Ok(())
}

fn reject_private_kiro_remote_media_ip(ip: IpAddr) -> Result<(), KiroRemoteMediaResolutionError> {
    if is_private_kiro_remote_media_ip(ip) {
        Err(KiroRemoteMediaResolutionError::new(
            "URL source host resolves to a private or local address",
        ))
    } else {
        Ok(())
    }
}

fn is_private_kiro_remote_media_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_private_kiro_remote_media_ipv4(ip),
        IpAddr::V6(ip) => is_private_kiro_remote_media_ipv6(ip),
    }
}

fn is_private_kiro_remote_media_ipv4(ip: Ipv4Addr) -> bool {
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip == Ipv4Addr::UNSPECIFIED
}

fn is_private_kiro_remote_media_ipv6(ip: Ipv6Addr) -> bool {
    ip.is_loopback()
        || ip.is_unique_local()
        || ip.is_unicast_link_local()
        || ip.is_unspecified()
        || matches!(ip.segments(), [0x2001, 0x0db8, _, _, _, _, _, _])
}

#[derive(Debug, Clone)]
struct KiroVisionBridgeImage {
    format: String,
    data: String,
}

fn kiro_opus_vision_bridge_required(payload: &MessagesRequest) -> bool {
    matches!(map_model(&payload.model).as_deref(), Some("claude-opus-4.6"))
        && !collect_kiro_vision_bridge_images(payload).is_empty()
}

fn collect_kiro_vision_bridge_images(payload: &MessagesRequest) -> Vec<KiroVisionBridgeImage> {
    let mut images = Vec::new();
    for message in &payload.messages {
        if message.role != "user" {
            continue;
        }
        let Some(items) = message.content.as_array() else {
            continue;
        };
        for item in items {
            let Some(object) = item.as_object() else {
                continue;
            };
            if object.get("type").and_then(serde_json::Value::as_str) != Some("image") {
                continue;
            }
            let Some(source) = object.get("source").and_then(serde_json::Value::as_object) else {
                continue;
            };
            if source.get("type").and_then(serde_json::Value::as_str) != Some("base64") {
                continue;
            }
            let Some(media_type) = source
                .get("media_type")
                .and_then(serde_json::Value::as_str)
                .and_then(kiro_image_format_from_media_type)
            else {
                continue;
            };
            let Some(data) = source
                .get("data")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                continue;
            };
            images.push(KiroVisionBridgeImage {
                format: media_type.to_string(),
                data: data.to_string(),
            });
        }
    }
    images
}

fn kiro_image_format_from_media_type(media_type: &str) -> Option<&'static str> {
    match canonical_image_media_type(media_type) {
        Some("image/jpeg") => Some("jpeg"),
        Some("image/png") => Some("png"),
        Some("image/gif") => Some("gif"),
        Some("image/webp") => Some("webp"),
        _ => None,
    }
}

fn kiro_vision_bridge_user_context(payload: &MessagesRequest) -> String {
    let mut parts = Vec::new();
    for message in &payload.messages {
        if message.role != "user" {
            continue;
        }
        match &message.content {
            serde_json::Value::String(text) => {
                if !text.trim().is_empty() {
                    parts.push(text.trim().to_string());
                }
            },
            serde_json::Value::Array(items) => {
                for item in items {
                    if item.get("type").and_then(serde_json::Value::as_str) == Some("text") {
                        if let Some(text) = item
                            .get("text")
                            .and_then(serde_json::Value::as_str)
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                        {
                            parts.push(text.to_string());
                        }
                    }
                }
            },
            _ => {},
        }
    }
    parts.join("\n")
}

fn build_kiro_vision_bridge_request(
    route: &ProviderKiroRoute,
    images: &[KiroVisionBridgeImage],
    user_context: &str,
) -> anyhow::Result<Vec<u8>> {
    let mut content = KIRO_VISION_BRIDGE_PROMPT.to_string();
    if !user_context.trim().is_empty() {
        content.push_str("\n\nUser request context:\n");
        content.push_str(user_context.trim());
    }
    let user_input = UserInputMessage {
        user_input_message_context: UserInputMessageContext::default(),
        content,
        model_id: KIRO_VISION_BRIDGE_MODEL.to_string(),
        images: images
            .iter()
            .map(|image| KiroImage::from_base64(image.format.clone(), image.data.clone()))
            .collect(),
        origin: Some("AI_EDITOR".to_string()),
    };
    let conversation_state = ConversationState::new(uuid::Uuid::new_v4().to_string())
        .with_agent_continuation_id(uuid::Uuid::new_v4().to_string())
        .with_agent_task_type("vibe")
        .with_chat_trigger_type("MANUAL")
        .with_current_message(CurrentMessage::new(user_input));
    Ok(serde_json::to_vec(&KiroRequest {
        conversation_state,
        profile_arn: route.profile_arn.clone(),
    })?)
}

fn kiro_assistant_text_from_response_bytes(bytes: &[u8]) -> Result<String, String> {
    let events = decode_kiro_events_from_bytes(bytes)?;
    let mut text = String::new();
    for event in events {
        match event {
            Event::AssistantResponse(delta) => text.push_str(&delta.content),
            Event::Error {
                error_code,
                error_message,
            } => {
                return Err(format!("{error_code}: {error_message}"));
            },
            Event::Exception {
                exception_type,
                message,
            } => {
                return Err(format!("{exception_type}: {message}"));
            },
            Event::ToolUse(tool) => {
                return Err(format!("vision bridge unexpectedly requested tool `{}`", tool.name));
            },
            Event::Metering(_) | Event::ContextUsage(_) | Event::Unknown {} => {},
        }
    }
    let text = text.trim().to_string();
    if text.is_empty() {
        Err("vision bridge returned empty description".to_string())
    } else {
        Ok(text)
    }
}

async fn describe_kiro_images_with_sonnet_bridge(
    routes: &[ProviderKiroRoute],
    route_store: &(dyn ProviderRouteStore + 'static),
    kiro_request_scheduler: &Arc<KiroRequestScheduler>,
    images: &[KiroVisionBridgeImage],
    user_context: &str,
) -> Result<String, Response> {
    let mut failed_accounts = HashSet::new();
    loop {
        let (route, _account_permit) = match select_kiro_route_with_account_permit(
            kiro_request_scheduler,
            routes,
            &failed_accounts,
        )
        .await
        {
            Ok(value) => value,
            Err(response) => return Err(response),
        };
        let request_body = match build_kiro_vision_bridge_request(&route, images, user_context) {
            Ok(body) => body,
            Err(err) => {
                return Err(kiro_json_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "api_error",
                    &format!("failed to encode kiro vision bridge request: {err}"),
                ));
            },
        };
        if request_body.len() > KIRO_GENERATE_REQUEST_MAX_BODY_BYTES {
            return Err(kiro_json_error(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "image payload is too large for Kiro vision bridge",
            ));
        }
        let upstream_url = format!(
            "{}/generateAssistantResponse",
            std::env::var("KIRO_UPSTREAM_BASE_URL")
                .map(|value| value.trim_end_matches('/').to_string())
                .unwrap_or_else(|_| format!("https://q.{}.amazonaws.com", route.api_region))
        );
        let response =
            match call_kiro_generate_for_route(&route, route_store, upstream_url, &request_body)
                .await
            {
                Ok(response) => response,
                Err(failure) => {
                    let body = failure.body_text();
                    match failure.kind {
                        KiroRouteFailureKind::QuotaExhausted => {
                            for account_name in account_names_for_kiro_routing_identity(
                                routes,
                                &route.routing_identity,
                            ) {
                                failed_accounts.insert(account_name.clone());
                                let _ = route_store
                                    .mark_kiro_account_quota_exhausted(
                                        &account_name,
                                        &body,
                                        now_millis(),
                                    )
                                    .await;
                            }
                        },
                        KiroRouteFailureKind::RateLimited {
                            cooldown,
                            mark_proxy,
                        } => {
                            kiro_request_scheduler.mark_account_cooldown(
                                &route.routing_identity,
                                cooldown,
                                &body,
                            );
                            if mark_proxy {
                                if let Some(proxy_key) = proxy_cooldown_key_for_route(&route) {
                                    kiro_request_scheduler
                                        .mark_proxy_cooldown(&proxy_key, cooldown, &body);
                                }
                            }
                            failed_accounts.insert(route.account_name.clone());
                        },
                        KiroRouteFailureKind::RetryNext => {
                            failed_accounts.insert(route.account_name.clone());
                        },
                        KiroRouteFailureKind::Fatal => return Err(failure.into_response()),
                    }
                    if has_remaining_kiro_candidate(routes, &failed_accounts, &route.account_name) {
                        continue;
                    }
                    return Err(kiro_json_error(
                        StatusCode::BAD_GATEWAY,
                        "api_error",
                        "Kiro vision bridge has no available account",
                    ));
                },
            };
        let status = response.status();
        let bytes = match response.bytes().await {
            Ok(bytes) => bytes,
            Err(err) => {
                return Err(kiro_json_error(
                    StatusCode::BAD_GATEWAY,
                    "api_error",
                    &format!("failed to read Kiro vision bridge response: {err}"),
                ));
            },
        };
        if !status.is_success() {
            return Err(kiro_json_error(
                StatusCode::BAD_GATEWAY,
                "api_error",
                &format!(
                    "Kiro vision bridge returned {status}: {}",
                    String::from_utf8_lossy(&bytes)
                ),
            ));
        }
        return kiro_assistant_text_from_response_bytes(&bytes).map_err(|err| {
            kiro_json_error(
                StatusCode::BAD_GATEWAY,
                "api_error",
                &format!("failed to parse Kiro vision bridge response: {err}"),
            )
        });
    }
}

fn inject_kiro_vision_bridge_context(payload: &mut MessagesRequest, description: &str) {
    let context = format!(
        "<image_context source=\"kiro-sonnet-4.6-vision\">\n{}\n</image_context>",
        description.trim()
    );
    let mut last_user_array_with_image = None;
    for (message_index, message) in payload.messages.iter_mut().enumerate() {
        if message.role != "user" {
            continue;
        }
        let Some(items) = message.content.as_array_mut() else {
            continue;
        };
        let original_len = items.len();
        items.retain(|item| item.get("type").and_then(serde_json::Value::as_str) != Some("image"));
        if items.len() != original_len {
            last_user_array_with_image = Some(message_index);
        }
    }
    if let Some(message_index) = last_user_array_with_image {
        if let Some(items) = payload.messages[message_index].content.as_array_mut() {
            items.push(serde_json::json!({
                "type": "text",
                "text": context
            }));
        }
    }
}

async fn dispatch_kiro_proxy(
    key: AuthenticatedKey,
    request: Request<Body>,
    route_store: Arc<dyn ProviderRouteStore>,
    control_store: Arc<dyn ControlStore>,
    kiro_cache_simulator: Arc<KiroCacheSimulator>,
    request_limiter: Arc<RequestLimiter>,
    kiro_request_scheduler: Arc<KiroRequestScheduler>,
) -> Response {
    let mut usage_meta = ProviderUsageMetadata::from_request_parts(
        request.method(),
        request.uri(),
        request.headers(),
    );
    let routes = match route_store.resolve_kiro_route_candidates(&key).await {
        Ok(routes) if !routes.is_empty() => routes,
        Ok(_) => {
            return (StatusCode::SERVICE_UNAVAILABLE, "kiro route is not configured")
                .into_response()
        },
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "kiro route resolution failed")
                .into_response()
        },
    };
    let Some((public_path, buffered_for_cc)) = normalized_kiro_messages_path(request.uri().path())
    else {
        return (StatusCode::NOT_FOUND, "unsupported kiro gateway endpoint").into_response();
    };
    usage_meta.request_url = external_origin(request.headers())
        .map(|origin| format!("{origin}/api/kiro-gateway{public_path}"))
        .unwrap_or_else(|| format!("/api/kiro-gateway{public_path}"));
    if request.method() != Method::POST {
        return (StatusCode::METHOD_NOT_ALLOWED, "unsupported kiro method").into_response();
    }
    let request_headers = request.headers().clone();
    let body_read_started = Instant::now();
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
    usage_meta =
        usage_meta.with_request_body(&body, clamp_duration_ms(body_read_started.elapsed()));
    let parse_started = Instant::now();
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
    usage_meta.mark_pre_handler_done(clamp_duration_ms(parse_started.elapsed()));
    usage_meta.last_message_content = extract_last_message_from_kiro_messages(&payload);
    if let Err(err) = apply_kiro_model_mapping(&routes[0].model_name_map_json, &mut payload) {
        return kiro_json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "api_error",
            &format!("Kiro model mapping configuration is invalid: {err}"),
        );
    }
    let effective_model = payload.model.clone();
    let route_mcp_web_search = websearch::should_route_mcp_web_search(&payload);
    if !route_mcp_web_search {
        websearch::remove_web_search_tools(&mut payload);
    }
    if let Err(err) = resolve_kiro_remote_media_sources(&mut payload).await {
        let response =
            kiro_json_error(StatusCode::BAD_REQUEST, "invalid_request_error", &err.to_string());
        capture_client_request_body_json(&mut usage_meta, &body);
        record_kiro_preflight_failure(KiroPreflightFailureRecord {
            control_store: control_store.as_ref(),
            key: &key,
            route: &routes[0],
            endpoint: public_path,
            model: &effective_model,
            status: StatusCode::BAD_REQUEST,
            meta: &mut usage_meta,
            cache_simulator: kiro_cache_simulator.as_ref(),
        })
        .await;
        return response;
    }
    if route_mcp_web_search {
        let request_input_tokens = token::count_all_tokens(
            payload.model.clone(),
            payload.system.clone(),
            payload.messages.clone(),
            payload.tools.clone(),
        ) as i32;
        override_kiro_thinking_from_model_name(&mut payload);
        if routes[0].full_request_logging_enabled {
            capture_client_request_body_json(&mut usage_meta, &body);
        }
        return dispatch_kiro_websearch(KiroWebsearchDispatch {
            key,
            payload,
            routes,
            control_store,
            route_store,
            request_limiter,
            kiro_request_scheduler,
            request_input_tokens,
            usage_meta,
        })
        .await;
    }
    let mut key_permit = None;
    if kiro_opus_vision_bridge_required(&payload) {
        let permit = match try_acquire_key_permit(
            &request_limiter,
            &key,
            routes[0].request_max_concurrency,
            routes[0].request_min_start_interval_ms,
        ) {
            Ok(permit) => permit,
            Err(rejection) => return kiro_key_limit_response(&rejection),
        };
        let images = collect_kiro_vision_bridge_images(&payload);
        let user_context = kiro_vision_bridge_user_context(&payload);
        let description = match describe_kiro_images_with_sonnet_bridge(
            &routes,
            route_store.as_ref(),
            &kiro_request_scheduler,
            &images,
            &user_context,
        )
        .await
        {
            Ok(description) => description,
            Err(response) => return response,
        };
        inject_kiro_vision_bridge_context(&mut payload, &description);
        key_permit = Some(permit);
    }
    let request_input_tokens = token::count_all_tokens(
        payload.model.clone(),
        payload.system.clone(),
        payload.messages.clone(),
        payload.tools.clone(),
    ) as i32;
    override_kiro_thinking_from_model_name(&mut payload);
    let normalized = match normalize_request(&payload) {
        Ok(normalized) => normalized,
        Err(err) => {
            let response = kiro_conversion_error_response(err);
            capture_client_request_body_json(&mut usage_meta, &body);
            record_kiro_preflight_failure(KiroPreflightFailureRecord {
                control_store: control_store.as_ref(),
                key: &key,
                route: &routes[0],
                endpoint: public_path,
                model: &effective_model,
                status: StatusCode::BAD_REQUEST,
                meta: &mut usage_meta,
                cache_simulator: kiro_cache_simulator.as_ref(),
            })
            .await;
            return response;
        },
    };
    let resolved_session =
        resolve_kiro_request_session(&request_headers, payload.metadata.as_ref());
    let conversion = match convert_normalized_request_with_resolved_session(
        normalized,
        routes[0].request_validation_enabled,
        resolved_session,
    ) {
        Ok(conversion) => conversion,
        Err(err) => {
            let response = kiro_conversion_error_response(err);
            capture_client_request_body_json(&mut usage_meta, &body);
            record_kiro_preflight_failure(KiroPreflightFailureRecord {
                control_store: control_store.as_ref(),
                key: &key,
                route: &routes[0],
                endpoint: public_path,
                model: &effective_model,
                status: StatusCode::BAD_REQUEST,
                meta: &mut usage_meta,
                cache_simulator: kiro_cache_simulator.as_ref(),
            })
            .await;
            return response;
        },
    };
    let thinking_enabled = payload
        .thinking
        .as_ref()
        .is_some_and(|thinking| thinking.is_enabled());
    if let Some(message) = unsupported_history_image_replay_message(
        conversion.has_history_images,
        &conversion.session_tracking,
    ) {
        let response = kiro_json_error(StatusCode::BAD_REQUEST, "invalid_request_error", &message);
        capture_client_request_body_json(&mut usage_meta, &body);
        record_kiro_preflight_failure(KiroPreflightFailureRecord {
            control_store: control_store.as_ref(),
            key: &key,
            route: &routes[0],
            endpoint: public_path,
            model: &effective_model,
            status: StatusCode::BAD_REQUEST,
            meta: &mut usage_meta,
            cache_simulator: kiro_cache_simulator.as_ref(),
        })
        .await;
        return response;
    }
    let base_conversation_state = conversion.conversation_state.clone();
    let key_permit = match key_permit.take() {
        Some(permit) => permit,
        None => match try_acquire_key_permit(
            &request_limiter,
            &key,
            routes[0].request_max_concurrency,
            routes[0].request_min_start_interval_ms,
        ) {
            Ok(permit) => permit,
            Err(rejection) => return kiro_key_limit_response(&rejection),
        },
    };
    let mut key_permit = Some(key_permit);
    let mut failed_accounts = HashSet::new();
    loop {
        let route_started = Instant::now();
        let (route, account_permit) = match select_kiro_route_with_account_permit(
            &kiro_request_scheduler,
            &routes,
            &failed_accounts,
        )
        .await
        {
            Ok(value) => value,
            Err(response) => return response,
        };
        usage_meta.add_routing_wait(clamp_duration_ms(route_started.elapsed()));
        let mut conversation_state = base_conversation_state.clone();
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
        if route.zero_cache_debug_enabled || route.full_request_logging_enabled {
            capture_client_request_body_json(&mut usage_meta, &body);
            capture_upstream_request_body_json(&mut usage_meta, &request_body);
        }
        if request_body.len() > KIRO_GENERATE_REQUEST_MAX_BODY_BYTES {
            capture_client_request_body_json(&mut usage_meta, &body);
            capture_upstream_request_body_json(&mut usage_meta, &request_body);
            usage_meta.mark_stream_finish();
            if let Err(err) = record_kiro_usage(KiroUsageRecord {
                control_store: control_store.as_ref(),
                key: &key,
                route: &route,
                endpoint: public_path,
                model: &effective_model,
                status: StatusCode::BAD_REQUEST,
                usage: zero_kiro_usage_summary(),
                cache_ctx: &cache_ctx,
                meta: &usage_meta,
            })
            .await
            {
                tracing::warn!(
                    key_id = %key.key_id,
                    account = %route.account_name,
                    error = %err,
                    "failed to record kiro oversized request usage"
                );
            }
            return kiro_json_error(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "Context window is full. Reduce conversation history, system prompt, or tools.",
            );
        }
        let upstream_url = format!(
            "{}/generateAssistantResponse",
            std::env::var("KIRO_UPSTREAM_BASE_URL")
                .map(|value| value.trim_end_matches('/').to_string())
                .unwrap_or_else(|_| format!("https://q.{}.amazonaws.com", route.api_region))
        );
        let response = match call_kiro_generate_for_route(
            &route,
            route_store.as_ref(),
            upstream_url,
            &request_body,
        )
        .await
        {
            Ok(response) => {
                usage_meta.mark_upstream_headers();
                response
            },
            Err(failure) => {
                let should_try_next = match failure.kind {
                    KiroRouteFailureKind::QuotaExhausted => {
                        let error_message = failure.body_text();
                        for account_name in account_names_for_kiro_routing_identity(
                            &routes,
                            &route.routing_identity,
                        ) {
                            failed_accounts.insert(account_name.clone());
                            let _ = route_store
                                .mark_kiro_account_quota_exhausted(
                                    &account_name,
                                    &error_message,
                                    now_millis(),
                                )
                                .await;
                        }
                        true
                    },
                    KiroRouteFailureKind::RateLimited {
                        cooldown,
                        mark_proxy,
                    } => {
                        kiro_request_scheduler.mark_account_cooldown(
                            &route.routing_identity,
                            cooldown,
                            failure.body_text(),
                        );
                        if mark_proxy {
                            if let Some(proxy_key) = proxy_cooldown_key_for_route(&route) {
                                kiro_request_scheduler.mark_proxy_cooldown(
                                    &proxy_key,
                                    cooldown,
                                    failure.body_text(),
                                );
                            }
                        }
                        usage_meta.mark_failover();
                        continue;
                    },
                    KiroRouteFailureKind::RetryNext => {
                        failed_accounts.insert(route.account_name.clone());
                        true
                    },
                    KiroRouteFailureKind::Fatal => false,
                };
                if should_try_next
                    && routes.iter().any(|candidate| {
                        !failed_accounts.contains(&candidate.account_name)
                            && candidate.account_name != route.account_name
                    })
                {
                    usage_meta.mark_failover();
                    continue;
                }
                let status = failure.status;
                capture_client_request_body_json(&mut usage_meta, &body);
                capture_upstream_request_body_json(&mut usage_meta, &request_body);
                usage_meta.mark_stream_finish();
                let error_response = failure.into_response();
                let usage = build_kiro_usage_summary(
                    &effective_model,
                    KiroUsageInputs {
                        request_input_tokens,
                        context_input_tokens: None,
                        output_tokens: 0,
                        credit_usage: None,
                        credit_usage_missing: true,
                        cache_estimation_enabled: false,
                    },
                    &cache_ctx,
                );
                if let Err(err) = record_kiro_usage(KiroUsageRecord {
                    control_store: control_store.as_ref(),
                    key: &key,
                    route: &route,
                    endpoint: public_path,
                    model: &effective_model,
                    status,
                    usage,
                    cache_ctx: &cache_ctx,
                    meta: &usage_meta,
                })
                .await
                {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("failed to record kiro usage: {err}"),
                    )
                        .into_response();
                }
                return error_response;
            },
        };
        if !response.status().is_success() {
            let status = response.status();
            capture_client_request_body_json(&mut usage_meta, &body);
            capture_upstream_request_body_json(&mut usage_meta, &request_body);
            usage_meta.mark_stream_finish();
            let usage = build_kiro_usage_summary(
                &effective_model,
                KiroUsageInputs {
                    request_input_tokens,
                    context_input_tokens: None,
                    output_tokens: 0,
                    credit_usage: None,
                    credit_usage_missing: true,
                    cache_estimation_enabled: false,
                },
                &cache_ctx,
            );
            if let Err(err) = record_kiro_usage(KiroUsageRecord {
                control_store: control_store.as_ref(),
                key: &key,
                route: &route,
                endpoint: public_path,
                model: &effective_model,
                status,
                usage,
                cache_ctx: &cache_ctx,
                meta: &usage_meta,
            })
            .await
            {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("failed to record kiro usage: {err}"),
                )
                    .into_response();
            }
            return pass_through_kiro_error_response(response).await;
        }
        let response_ctx = KiroResponseContext {
            key,
            route,
            public_path: public_path.to_string(),
            model: effective_model,
            request_input_tokens,
            thinking_enabled,
            tool_name_map: conversion.tool_name_map.clone(),
            structured_output_tool_name: conversion.structured_output_tool_name.clone(),
            cache_ctx,
            control_store,
            kiro_cache_simulator,
            usage_meta,
            _key_permit: key_permit
                .take()
                .expect("kiro key permit should be held until response is returned"),
            _account_permit: account_permit,
        };
        if payload.stream {
            if buffered_for_cc {
                return buffered_kiro_stream_response(response, response_ctx).await;
            }
            return stream_kiro_upstream_response(response, response_ctx);
        }

        return non_stream_kiro_response(response, response_ctx).await;
    }
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
    usage_meta: ProviderUsageMetadata,
    _key_permit: LimitPermit,
    _account_permit: KiroRequestLease,
}

struct KiroWebsearchDispatch {
    key: AuthenticatedKey,
    payload: MessagesRequest,
    routes: Vec<ProviderKiroRoute>,
    control_store: Arc<dyn ControlStore>,
    route_store: Arc<dyn ProviderRouteStore>,
    request_limiter: Arc<RequestLimiter>,
    kiro_request_scheduler: Arc<KiroRequestScheduler>,
    request_input_tokens: i32,
    usage_meta: ProviderUsageMetadata,
}

async fn dispatch_kiro_websearch(input: KiroWebsearchDispatch) -> Response {
    let KiroWebsearchDispatch {
        key,
        payload,
        routes,
        control_store,
        route_store,
        request_limiter,
        kiro_request_scheduler,
        request_input_tokens,
        mut usage_meta,
    } = input;
    let key_permit = match try_acquire_key_permit(
        &request_limiter,
        &key,
        routes[0].request_max_concurrency,
        routes[0].request_min_start_interval_ms,
    ) {
        Ok(permit) => permit,
        Err(rejection) => return kiro_key_limit_response(&rejection),
    };
    let Some(query) = websearch::extract_search_query(&payload) else {
        return kiro_json_error(
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            "Unable to extract web search query from messages.",
        );
    };
    let (tool_use_id, mcp_request) = websearch::create_mcp_request(&query);
    let request_body = match serde_json::to_string(&mcp_request) {
        Ok(body) => body,
        Err(err) => {
            return kiro_json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "api_error",
                &format!("failed to encode kiro mcp request: {err}"),
            )
        },
    };

    let mut key_permit = Some(key_permit);
    let mut failed_accounts = HashSet::new();
    loop {
        let route_started = Instant::now();
        let (route, account_permit) = match select_kiro_route_with_account_permit(
            &kiro_request_scheduler,
            &routes,
            &failed_accounts,
        )
        .await
        {
            Ok(value) => value,
            Err(response) => return response,
        };
        usage_meta.add_routing_wait(clamp_duration_ms(route_started.elapsed()));
        let mut route_usage_meta = usage_meta.clone();
        match call_kiro_mcp_for_route(&route, route_store.as_ref(), &request_body).await {
            Ok(mcp_response) => {
                let capture_request_details = route.full_request_logging_enabled;
                if capture_request_details {
                    capture_upstream_request_body_json(
                        &mut route_usage_meta,
                        request_body.as_bytes(),
                    );
                }
                route_usage_meta.mark_upstream_headers();
                route_usage_meta.mark_post_headers_body();
                route_usage_meta.mark_stream_finish();
                return build_kiro_websearch_response(WebsearchResponseInput {
                    key,
                    route,
                    payload,
                    query,
                    tool_use_id,
                    search_results: websearch::parse_search_results(&mcp_response),
                    request_input_tokens,
                    status: StatusCode::OK,
                    control_store,
                    usage_meta: route_usage_meta,
                    capture_request_details,
                    _key_permit: key_permit
                        .take()
                        .expect("kiro key permit should be held until response is returned"),
                    _account_permit: account_permit,
                })
                .await;
            },
            Err(failure) => {
                match failure.kind {
                    KiroRouteFailureKind::QuotaExhausted => {
                        let error_message = failure.body_text();
                        for account_name in account_names_for_kiro_routing_identity(
                            &routes,
                            &route.routing_identity,
                        ) {
                            failed_accounts.insert(account_name.clone());
                            let _ = route_store
                                .mark_kiro_account_quota_exhausted(
                                    &account_name,
                                    &error_message,
                                    now_millis(),
                                )
                                .await;
                        }
                        if has_remaining_kiro_candidate(
                            &routes,
                            &failed_accounts,
                            &route.account_name,
                        ) {
                            usage_meta.mark_failover();
                            continue;
                        }
                        return failure.into_response();
                    },
                    KiroRouteFailureKind::RateLimited {
                        cooldown,
                        mark_proxy,
                    } => {
                        kiro_request_scheduler.mark_account_cooldown(
                            &route.routing_identity,
                            cooldown,
                            failure.body_text(),
                        );
                        if mark_proxy {
                            if let Some(proxy_key) = proxy_cooldown_key_for_route(&route) {
                                kiro_request_scheduler.mark_proxy_cooldown(
                                    &proxy_key,
                                    cooldown,
                                    failure.body_text(),
                                );
                            }
                        }
                        usage_meta.mark_failover();
                        continue;
                    },
                    KiroRouteFailureKind::RetryNext => {
                        failed_accounts.insert(route.account_name.clone());
                        if has_remaining_kiro_candidate(
                            &routes,
                            &failed_accounts,
                            &route.account_name,
                        ) {
                            usage_meta.mark_failover();
                            continue;
                        }
                    },
                    KiroRouteFailureKind::Fatal => {},
                }
                let message = failure.body_text();
                if websearch::should_propagate_mcp_error_text(&message) {
                    return kiro_json_error(StatusCode::BAD_GATEWAY, "api_error", &message);
                }
                capture_upstream_request_body_json(&mut route_usage_meta, request_body.as_bytes());
                route_usage_meta.mark_stream_finish();
                return build_kiro_websearch_response(WebsearchResponseInput {
                    key,
                    route,
                    payload,
                    query,
                    tool_use_id,
                    search_results: None,
                    request_input_tokens,
                    status: StatusCode::OK,
                    control_store,
                    usage_meta: route_usage_meta,
                    capture_request_details: true,
                    _key_permit: key_permit
                        .take()
                        .expect("kiro key permit should be held until response is returned"),
                    _account_permit: account_permit,
                })
                .await;
            },
        }
    }
}

struct WebsearchResponseInput {
    key: AuthenticatedKey,
    route: ProviderKiroRoute,
    payload: MessagesRequest,
    query: String,
    tool_use_id: String,
    search_results: Option<websearch::WebSearchResults>,
    request_input_tokens: i32,
    status: StatusCode,
    control_store: Arc<dyn ControlStore>,
    usage_meta: ProviderUsageMetadata,
    capture_request_details: bool,
    _key_permit: LimitPermit,
    _account_permit: KiroRequestLease,
}

async fn build_kiro_websearch_response(input: WebsearchResponseInput) -> Response {
    let summary = websearch::generate_search_summary(&input.query, &input.search_results);
    let output_tokens = websearch::estimate_output_tokens(&summary);
    let usage = KiroUsageSummary {
        input_uncached_tokens: input.request_input_tokens,
        input_cached_tokens: 0,
        output_tokens,
        credit_usage: None,
        credit_usage_missing: true,
    };
    if let Err(err) = record_kiro_websearch_usage(KiroWebsearchUsageRecord {
        control_store: input.control_store.as_ref(),
        key: &input.key,
        route: &input.route,
        model: &input.payload.model,
        status: input.status,
        usage,
        meta: &input.usage_meta,
        capture_request_details: input.capture_request_details,
    })
    .await
    {
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("failed to record kiro usage: {err}"))
            .into_response();
    }

    if input.payload.stream {
        let body = websearch::generate_websearch_events(
            &input.payload.model,
            &input.query,
            &input.tool_use_id,
            input.search_results.as_ref(),
            input.request_input_tokens,
            &summary,
            output_tokens,
        )
        .into_iter()
        .map(|event| event.to_sse_string())
        .collect::<String>();
        return Response::builder()
            .status(input.status)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .header(header::CACHE_CONTROL, "no-cache")
            .header(header::CONNECTION, "keep-alive")
            .body(Body::from(body))
            .unwrap_or_else(|_| {
                (StatusCode::BAD_GATEWAY, "kiro web_search stream response build failed")
                    .into_response()
            });
    }

    let body = serde_json::json!({
        "id": format!("msg_{}", uuid::Uuid::new_v4().simple()),
        "type": "message",
        "role": "assistant",
        "content": websearch::create_non_stream_content_blocks(
            &input.query,
            &input.tool_use_id,
            &input.search_results,
            &summary,
        ),
        "model": input.payload.model,
        "stop_reason": "end_turn",
        "stop_sequence": null,
        "usage": anthropic_usage_json(input.request_input_tokens, output_tokens, 0),
    });
    Response::builder()
        .status(input.status)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(body.to_string()))
        .unwrap_or_else(|_| {
            (StatusCode::BAD_GATEWAY, "kiro web_search json response build failed").into_response()
        })
}

#[derive(Debug, Clone, Copy)]
enum KiroRouteFailureKind {
    RetryNext,
    Fatal,
    QuotaExhausted,
    RateLimited { cooldown: Duration, mark_proxy: bool },
}

#[derive(Debug)]
struct KiroRouteFailure {
    status: StatusCode,
    content_type: String,
    body: Bytes,
    kind: KiroRouteFailureKind,
}

impl KiroRouteFailure {
    fn synthetic(status: StatusCode, message: String, kind: KiroRouteFailureKind) -> Self {
        let body = serde_json::json!({
            "error": {
                "type": "api_error",
                "message": message,
            }
        })
        .to_string();
        Self {
            status,
            content_type: "application/json".to_string(),
            body: Bytes::from(body),
            kind,
        }
    }

    async fn from_response(response: reqwest::Response, kind: KiroRouteFailureKind) -> Self {
        let status = response.status();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("application/json")
            .to_string();
        let body = response.bytes().await.unwrap_or_else(|_| Bytes::new());
        Self {
            status,
            content_type,
            body,
            kind,
        }
    }

    fn with_kind(mut self, kind: KiroRouteFailureKind) -> Self {
        self.kind = kind;
        self
    }

    fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    fn into_response(self) -> Response {
        Response::builder()
            .status(self.status)
            .header(header::CONTENT_TYPE, self.content_type)
            .header(header::CACHE_CONTROL, "no-store")
            .body(Body::from(self.body))
            .unwrap_or_else(|_| {
                (StatusCode::BAD_GATEWAY, "kiro upstream error response build failed")
                    .into_response()
            })
    }
}

async fn call_kiro_generate_for_route(
    route: &ProviderKiroRoute,
    route_store: &dyn ProviderRouteStore,
    upstream_url: String,
    request_body: &[u8],
) -> Result<reqwest::Response, KiroRouteFailure> {
    let mut force_refresh = false;
    let mut last_failure: Option<KiroRouteFailure> = None;
    for attempt in 0..3 {
        let call_ctx =
            match kiro_refresh::ensure_context_for_route(route, route_store, force_refresh).await {
                Ok(ctx) => ctx,
                Err(err) => {
                    return Err(KiroRouteFailure::synthetic(
                        StatusCode::BAD_GATEWAY,
                        format!("kiro auth refresh failed for {}: {err}", route.account_name),
                        KiroRouteFailureKind::RetryNext,
                    ));
                },
            };
        let response = match send_kiro_generate_request(
            route,
            &call_ctx,
            upstream_url.clone(),
            request_body.to_vec(),
        )
        .await
        {
            Ok(response) => response,
            Err(err) => {
                last_failure = Some(KiroRouteFailure::synthetic(
                    StatusCode::BAD_GATEWAY,
                    format!("kiro upstream transport failure: {err}"),
                    KiroRouteFailureKind::RetryNext,
                ));
                tokio::time::sleep(Duration::from_millis(350)).await;
                continue;
            },
        };
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status();
        let failure = KiroRouteFailure::from_response(response, KiroRouteFailureKind::Fatal).await;
        let body = failure.body_text();
        if status.as_u16() == 402 && is_monthly_request_limit(&body) {
            return Err(failure.with_kind(KiroRouteFailureKind::QuotaExhausted));
        }
        if status.as_u16() == 429 {
            if let Some(cooldown) = daily_request_limit_cooldown(&body) {
                return Err(failure.with_kind(KiroRouteFailureKind::RateLimited {
                    cooldown,
                    mark_proxy: false,
                }));
            }
        }
        if status.as_u16() == 400 {
            if let Some(cooldown) = transient_invalid_model_cooldown(&body) {
                return Err(failure.with_kind(KiroRouteFailureKind::RateLimited {
                    cooldown,
                    mark_proxy: true,
                }));
            }
            return Err(failure.with_kind(KiroRouteFailureKind::Fatal));
        }
        if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) && !force_refresh {
            force_refresh = true;
            last_failure = Some(failure.with_kind(KiroRouteFailureKind::RetryNext));
            continue;
        }
        if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
            return Err(failure.with_kind(KiroRouteFailureKind::RetryNext));
        }
        if matches!(status, StatusCode::REQUEST_TIMEOUT | StatusCode::TOO_MANY_REQUESTS)
            || status.is_server_error()
        {
            last_failure = Some(failure.with_kind(KiroRouteFailureKind::RetryNext));
            if attempt < 2 {
                tokio::time::sleep(Duration::from_millis(350)).await;
                continue;
            }
            return Err(last_failure.expect("retryable kiro failure should be captured"));
        }
        return Err(failure.with_kind(KiroRouteFailureKind::Fatal));
    }
    Err(last_failure.unwrap_or_else(|| {
        KiroRouteFailure::synthetic(
            StatusCode::BAD_GATEWAY,
            "kiro upstream request failed".to_string(),
            KiroRouteFailureKind::RetryNext,
        )
    }))
}

async fn call_kiro_mcp_for_route(
    route: &ProviderKiroRoute,
    route_store: &dyn ProviderRouteStore,
    request_body: &str,
) -> Result<McpResponse, KiroRouteFailure> {
    let upstream_url = format!(
        "{}/mcp",
        std::env::var("KIRO_UPSTREAM_BASE_URL")
            .map(|value| value.trim_end_matches('/').to_string())
            .unwrap_or_else(|_| format!("https://q.{}.amazonaws.com", route.api_region))
    );
    let mut force_refresh = false;
    let mut last_failure: Option<KiroRouteFailure> = None;
    let mut attempt = 0usize;
    let response = loop {
        attempt += 1;
        let call_ctx =
            match kiro_refresh::ensure_context_for_route(route, route_store, force_refresh).await {
                Ok(ctx) => ctx,
                Err(err) => {
                    break Err(KiroRouteFailure::synthetic(
                        StatusCode::BAD_GATEWAY,
                        format!("kiro mcp auth refresh failed for {}: {err}", route.account_name),
                        KiroRouteFailureKind::RetryNext,
                    ));
                },
            };
        let response = match send_kiro_mcp_request(
            route,
            &call_ctx,
            upstream_url.clone(),
            request_body.to_string(),
        )
        .await
        {
            Ok(response) => response,
            Err(err) => {
                last_failure = Some(KiroRouteFailure::synthetic(
                    StatusCode::BAD_GATEWAY,
                    format!("kiro mcp transport failure: {err}"),
                    KiroRouteFailureKind::RetryNext,
                ));
                if attempt < 3 {
                    tokio::time::sleep(Duration::from_millis(350)).await;
                    continue;
                }
                break Err(last_failure.expect("mcp transport failure should be captured"));
            },
        };
        if response.status().is_success() {
            break Ok(response);
        }
        let status = response.status();
        let failure = KiroRouteFailure::from_response(response, KiroRouteFailureKind::Fatal).await;
        let body = failure.body_text();
        if status.as_u16() == 402 && is_monthly_request_limit(&body) {
            break Err(failure.with_kind(KiroRouteFailureKind::QuotaExhausted));
        }
        if status.as_u16() == 429 {
            if let Some(cooldown) = daily_request_limit_cooldown(&body) {
                break Err(failure.with_kind(KiroRouteFailureKind::RateLimited {
                    cooldown,
                    mark_proxy: false,
                }));
            }
        }
        if status.as_u16() == 400 {
            if let Some(cooldown) = transient_invalid_model_cooldown(&body) {
                break Err(failure.with_kind(KiroRouteFailureKind::RateLimited {
                    cooldown,
                    mark_proxy: true,
                }));
            }
            break Err(failure.with_kind(KiroRouteFailureKind::Fatal));
        }
        if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) && !force_refresh {
            force_refresh = true;
            last_failure = Some(failure.with_kind(KiroRouteFailureKind::RetryNext));
            continue;
        }
        if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
            break Err(failure.with_kind(KiroRouteFailureKind::RetryNext));
        }
        if matches!(status, StatusCode::REQUEST_TIMEOUT | StatusCode::TOO_MANY_REQUESTS)
            || status.is_server_error()
        {
            last_failure = Some(failure.with_kind(KiroRouteFailureKind::RetryNext));
            if attempt < 3 {
                tokio::time::sleep(Duration::from_millis(350)).await;
                continue;
            }
            break Err(last_failure.expect("retryable mcp failure should be captured"));
        }
        break Err(failure.with_kind(KiroRouteFailureKind::Fatal));
    }?;
    let body = response.text().await.map_err(|err| {
        KiroRouteFailure::synthetic(
            StatusCode::BAD_GATEWAY,
            format!("read kiro mcp response body: {err}"),
            KiroRouteFailureKind::RetryNext,
        )
    })?;
    let mcp_response = serde_json::from_str::<McpResponse>(&body).map_err(|err| {
        KiroRouteFailure::synthetic(
            StatusCode::BAD_GATEWAY,
            format!("parse kiro mcp response body: {err}; body={body}"),
            KiroRouteFailureKind::Fatal,
        )
    })?;
    if let Some(error) = &mcp_response.error {
        return Err(KiroRouteFailure::synthetic(
            StatusCode::BAD_GATEWAY,
            format!(
                "MCP error: {} - {}",
                error.code.unwrap_or(-1),
                error.message.as_deref().unwrap_or("Unknown error")
            ),
            KiroRouteFailureKind::Fatal,
        ));
    }
    Ok(mcp_response)
}

async fn send_kiro_generate_request(
    route: &ProviderKiroRoute,
    call_ctx: &kiro_refresh::KiroCallContext,
    upstream_url: String,
    request_body: Vec<u8>,
) -> anyhow::Result<reqwest::Response> {
    let client = provider_client(route.proxy.as_ref())?;
    Ok(add_kiro_upstream_headers(
        client.post(upstream_url),
        route,
        &call_ctx.access_token,
        Some(&call_ctx.auth),
    )?
    .body(request_body)
    .send()
    .await?)
}

async fn send_kiro_mcp_request(
    route: &ProviderKiroRoute,
    call_ctx: &kiro_refresh::KiroCallContext,
    upstream_url: String,
    request_body: String,
) -> anyhow::Result<reqwest::Response> {
    let client = provider_client(route.proxy.as_ref())?;
    Ok(add_kiro_mcp_headers(
        client.post(upstream_url),
        route,
        &call_ctx.access_token,
        Some(&call_ctx.auth),
    )?
    .body(request_body)
    .send()
    .await?)
}

fn has_remaining_kiro_candidate(
    routes: &[ProviderKiroRoute],
    failed_accounts: &HashSet<String>,
    current_account_name: &str,
) -> bool {
    routes.iter().any(|candidate| {
        candidate.account_name != current_account_name
            && !failed_accounts.contains(&candidate.account_name)
    })
}

fn account_names_for_kiro_routing_identity(
    routes: &[ProviderKiroRoute],
    routing_identity: &str,
) -> Vec<String> {
    routes
        .iter()
        .filter(|route| route.routing_identity == routing_identity)
        .map(|route| route.account_name.clone())
        .collect()
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
        let mut usage_meta = ctx.usage_meta.clone();
        let mut stream_ctx = StreamContext::new_with_thinking(
            &ctx.model,
            ctx.request_input_tokens,
            ctx.thinking_enabled,
            ctx.tool_name_map,
            ctx.structured_output_tool_name.clone(),
        );
        for event in stream_ctx.generate_initial_events() {
            usage_meta.mark_first_sse_write();
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
                    usage_meta.mark_first_sse_write();
                    yield Ok::<Bytes, std::io::Error>(Bytes::from(sse_event.to_sse_string()));
                }
            }
        }
        usage_meta.mark_post_headers_body();
        let (_resolved_input_tokens, output_tokens) = stream_ctx.final_usage();
        let (credit_usage, credit_usage_missing) = stream_ctx.final_credit_usage();
        let usage = build_kiro_usage_summary(
            &ctx.model,
            KiroUsageInputs {
                request_input_tokens: ctx.request_input_tokens,
                context_input_tokens: stream_ctx.context_input_tokens(),
                output_tokens,
                credit_usage,
                credit_usage_missing,
                cache_estimation_enabled: ctx.route.cache_estimation_enabled,
            },
            &ctx.cache_ctx,
        );
        let mut final_events = stream_ctx.generate_final_events();
        let anthropic_usage = anthropic_usage_json_from_summary_with_policy(usage, &ctx.cache_ctx);
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
        usage_meta.mark_stream_finish();
        if let Err(err) = record_kiro_usage(KiroUsageRecord {
            control_store: ctx.control_store.as_ref(),
            key: &ctx.key,
            route: &ctx.route,
            endpoint: &ctx.public_path,
            model: &ctx.model,
            status,
            usage,
            cache_ctx: &ctx.cache_ctx,
            meta: &usage_meta,
        }).await {
            yield Err(std::io::Error::other(format!("failed to record kiro usage: {err}")));
            return;
        }
        for event in final_events {
            usage_meta.mark_first_sse_write();
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
    let mut usage_meta = ctx.usage_meta.clone();
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
    usage_meta.mark_post_headers_body();
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
    let (_resolved_input_tokens, output_tokens) = stream_ctx.final_usage();
    let (credit_usage, credit_usage_missing) = stream_ctx.final_credit_usage();
    let usage = build_kiro_usage_summary(
        &ctx.model,
        KiroUsageInputs {
            request_input_tokens: ctx.request_input_tokens,
            context_input_tokens: stream_ctx.context_input_tokens(),
            output_tokens,
            credit_usage,
            credit_usage_missing,
            cache_estimation_enabled: ctx.route.cache_estimation_enabled,
        },
        &ctx.cache_ctx,
    );
    let mut sse_events = stream_ctx.finish_and_get_all_events();
    let anthropic_usage = anthropic_usage_json_from_summary_with_policy(usage, &ctx.cache_ctx);
    for event in &mut sse_events {
        if event.event == "message_delta" {
            if let Some(value) = event.data.get_mut("usage") {
                *value = anthropic_usage.clone();
            }
        }
    }
    let assistant_message = stream_ctx.final_assistant_message();
    usage_meta.mark_first_sse_write();
    usage_meta.mark_stream_finish();
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
        meta: &usage_meta,
    })
    .await
    {
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("failed to record kiro usage: {err}"))
            .into_response();
    }
    let body_stream = futures_util::stream::iter(
        sse_events
            .into_iter()
            .map(|event| Ok::<Bytes, std::io::Error>(Bytes::from(event.to_sse_string()))),
    );
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONNECTION, "keep-alive")
        .body(Body::from_stream(body_stream))
        .unwrap_or_else(|_| {
            (StatusCode::BAD_GATEWAY, "kiro buffered response build failed").into_response()
        })
}

async fn non_stream_kiro_response(
    response: reqwest::Response,
    ctx: KiroResponseContext,
) -> Response {
    let status = response.status();
    let mut usage_meta = ctx.usage_meta.clone();
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
    usage_meta.mark_post_headers_body();
    usage_meta.mark_stream_finish();
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
    let (_resolved_input_tokens, output_tokens) = stream_ctx.final_usage();
    let (credit_usage, credit_usage_missing) = stream_ctx.final_credit_usage();
    let usage = build_kiro_usage_summary(
        &ctx.model,
        KiroUsageInputs {
            request_input_tokens: ctx.request_input_tokens,
            context_input_tokens: stream_ctx.context_input_tokens(),
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
        meta: &usage_meta,
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
        "usage": anthropic_usage_json_from_summary_with_policy(usage, &ctx.cache_ctx),
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
        "/v1/messages" | "/api/kiro-gateway/v1/messages" => Some(("/v1/messages", false)),
        _ => None,
    }
}

fn kiro_provider_user_agents(machine_id: &str) -> (String, String) {
    (
        format!(
            "aws-sdk-js/{KIRO_PROVIDER_AWS_SDK_VERSION} \
             KiroIDE-{DEFAULT_KIRO_VERSION}-{machine_id}"
        ),
        format!(
            "aws-sdk-js/{KIRO_PROVIDER_AWS_SDK_VERSION} ua/2.1 os/{DEFAULT_SYSTEM_VERSION} \
             lang/js md/nodejs#{DEFAULT_NODE_VERSION} \
             api/codewhispererstreaming#{KIRO_PROVIDER_AWS_SDK_VERSION} m/E \
             KiroIDE-{DEFAULT_KIRO_VERSION}-{machine_id}"
        ),
    )
}

fn add_kiro_upstream_headers(
    mut upstream: reqwest::RequestBuilder,
    route: &ProviderKiroRoute,
    access_token: &str,
    auth_record: Option<&KiroAuthRecord>,
) -> anyhow::Result<reqwest::RequestBuilder> {
    let auth = auth_record.ok_or_else(|| anyhow::anyhow!("invalid kiro auth record"))?;
    let machine_id = machine_id::generate_from_auth(auth)
        .ok_or_else(|| anyhow::anyhow!("failed to derive kiro machine id"))?;
    let (x_amz_user_agent, user_agent) = kiro_provider_user_agents(&machine_id);
    let host = format!("q.{}.amazonaws.com", route.api_region);
    upstream = upstream
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(reqwest::header::ACCEPT, "application/vnd.amazon.eventstream")
        .header("x-amzn-codewhisperer-optout", "true")
        .header("x-amzn-kiro-agent-mode", "vibe")
        .header("x-amz-user-agent", x_amz_user_agent)
        .header(reqwest::header::USER_AGENT, user_agent)
        .header("host", host)
        .header("amz-sdk-invocation-id", uuid::Uuid::new_v4().to_string())
        .header("amz-sdk-request", "attempt=1; max=3")
        .header("authorization", format!("Bearer {access_token}"))
        .header("connection", "close");
    Ok(upstream)
}

fn add_kiro_mcp_headers(
    mut upstream: reqwest::RequestBuilder,
    route: &ProviderKiroRoute,
    access_token: &str,
    auth_record: Option<&KiroAuthRecord>,
) -> anyhow::Result<reqwest::RequestBuilder> {
    let auth = auth_record.ok_or_else(|| anyhow::anyhow!("invalid kiro auth record"))?;
    let machine_id = machine_id::generate_from_auth(auth)
        .ok_or_else(|| anyhow::anyhow!("failed to derive kiro machine id"))?;
    let (x_amz_user_agent, user_agent) = kiro_provider_user_agents(&machine_id);
    let host = format!("q.{}.amazonaws.com", route.api_region);
    upstream = upstream
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header("x-amz-user-agent", x_amz_user_agent)
        .header(reqwest::header::USER_AGENT, user_agent)
        .header("host", host)
        .header("amz-sdk-invocation-id", uuid::Uuid::new_v4().to_string())
        .header("amz-sdk-request", "attempt=1; max=3")
        .header("authorization", format!("Bearer {access_token}"))
        .header("connection", "close");
    if let Some(profile_arn) = route
        .profile_arn
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        upstream = upstream.header("x-amzn-kiro-profile-arn", profile_arn);
    }
    Ok(upstream)
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

fn proxy_cooldown_key_for_route(route: &ProviderKiroRoute) -> Option<String> {
    route
        .proxy
        .as_ref()
        .map(|proxy| format!("url:{}", proxy.proxy_url))
}

fn is_monthly_request_limit(body: &str) -> bool {
    body.contains("MONTHLY_REQUEST_COUNT")
        || kiro_error_reason(body).as_deref() == Some("MONTHLY_REQUEST_COUNT")
}

fn daily_request_limit_cooldown(body: &str) -> Option<Duration> {
    if body.contains("5-minute credit limit exceeded") {
        return Some(Duration::from_secs(5 * 60));
    }
    if kiro_error_reason(body).as_deref() == Some("DAILY_REQUEST_COUNT") {
        return Some(Duration::from_secs(5 * 60));
    }
    None
}

fn transient_invalid_model_cooldown(body: &str) -> Option<Duration> {
    if !body.contains("Invalid model") {
        return None;
    }
    if kiro_error_reason(body).as_deref() == Some("INVALID_MODEL_ID") {
        return Some(Duration::from_secs(60));
    }
    None
}

fn kiro_error_reason(body: &str) -> Option<String> {
    let value = serde_json::from_str::<serde_json::Value>(body).ok()?;
    value
        .get("reason")
        .and_then(|item| item.as_str())
        .or_else(|| {
            value
                .pointer("/error/reason")
                .and_then(|item| item.as_str())
        })
        .map(str::to_string)
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

fn apply_kiro_model_mapping(
    model_name_map_json: &str,
    payload: &mut MessagesRequest,
) -> anyhow::Result<Option<(String, String)>> {
    let trimmed = model_name_map_json.trim();
    if trimmed.is_empty() || trimmed == "{}" {
        return Ok(None);
    }
    let map = serde_json::from_str::<BTreeMap<String, String>>(trimmed)?;
    let Some(target_model) = map.get(&payload.model).cloned() else {
        return Ok(None);
    };
    if target_model == payload.model {
        return Ok(None);
    }
    let source_model = payload.model.clone();
    payload.model = target_model.clone();
    Ok(Some((source_model, target_model)))
}

fn unsupported_history_image_replay_message(
    has_history_images: bool,
    session_tracking: &SessionTracking,
) -> Option<String> {
    (has_history_images && matches!(session_tracking.source, SessionIdSource::GeneratedFallback(_)))
        .then(|| {
            "Historical image turns require a stable session id. Re-send the image in the current \
             message or provide a stable session id via request headers or metadata."
                .to_string()
        })
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

fn anthropic_usage_json_with_policy(
    policy: &KiroCachePolicy,
    input_tokens_total: i32,
    output_tokens: i32,
    cache_read_input_tokens: i32,
) -> serde_json::Value {
    let input_tokens_total = input_tokens_total.max(0);
    let cache_read_input_tokens = cache_read_input_tokens.max(0).min(input_tokens_total);
    let non_cached_input_tokens_total = input_tokens_total.saturating_sub(cache_read_input_tokens);
    let cache_creation_input_tokens = if cache_read_input_tokens == 0 {
        non_cached_input_tokens_total / 2
    } else {
        let ratio = policy.anthropic_cache_creation_input_ratio;
        if !ratio.is_finite() || ratio <= 0.0 {
            0
        } else {
            (((non_cached_input_tokens_total as f64) * ratio).floor() as i32)
                .max(0)
                .min(non_cached_input_tokens_total)
        }
    };
    let input_tokens = non_cached_input_tokens_total.saturating_sub(cache_creation_input_tokens);
    serde_json::json!({
        "input_tokens": input_tokens,
        "output_tokens": output_tokens.max(0),
        "cache_creation_input_tokens": cache_creation_input_tokens,
        "cache_read_input_tokens": cache_read_input_tokens,
    })
}

fn anthropic_usage_json_from_summary_with_policy(
    usage: KiroUsageSummary,
    cache_ctx: &KiroCacheContext,
) -> serde_json::Value {
    anthropic_usage_json_with_policy(
        &cache_ctx.policy,
        usage.input_uncached_tokens + usage.input_cached_tokens,
        usage.output_tokens,
        usage.input_cached_tokens,
    )
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
    meta: &'a ProviderUsageMetadata,
}

struct KiroPreflightFailureRecord<'a> {
    control_store: &'a dyn ControlStore,
    key: &'a AuthenticatedKey,
    route: &'a ProviderKiroRoute,
    endpoint: &'a str,
    model: &'a str,
    status: StatusCode,
    meta: &'a mut ProviderUsageMetadata,
    cache_simulator: &'a KiroCacheSimulator,
}

fn zero_kiro_usage_summary() -> KiroUsageSummary {
    KiroUsageSummary {
        input_uncached_tokens: 0,
        input_cached_tokens: 0,
        output_tokens: 0,
        credit_usage: None,
        credit_usage_missing: false,
    }
}

async fn record_kiro_preflight_failure(record: KiroPreflightFailureRecord<'_>) {
    record.meta.mark_stream_finish();
    let conversation_state = ConversationState::new(uuid::Uuid::new_v4().to_string());
    let cache_ctx =
        match build_kiro_cache_context(record.route, &conversation_state, record.cache_simulator) {
            Ok(cache_ctx) => cache_ctx,
            Err(err) => {
                tracing::warn!(
                    key_id = %record.key.key_id,
                    account = %record.route.account_name,
                    error = %err,
                    "failed to build kiro cache context for preflight failure usage"
                );
                return;
            },
        };
    if let Err(err) = record_kiro_usage(KiroUsageRecord {
        control_store: record.control_store,
        key: record.key,
        route: record.route,
        endpoint: record.endpoint,
        model: record.model,
        status: record.status,
        usage: zero_kiro_usage_summary(),
        cache_ctx: &cache_ctx,
        meta: record.meta,
    })
    .await
    {
        tracing::warn!(
            key_id = %record.key.key_id,
            account = %record.route.account_name,
            error = %err,
            "failed to record kiro preflight failure usage"
        );
    }
}

async fn record_kiro_usage(record: KiroUsageRecord<'_>) -> anyhow::Result<()> {
    let billable_tokens = kiro_billable_tokens(record.model, record.usage, record.cache_ctx);
    let capture_request_details = record.status.as_u16() >= 400
        || record.route.full_request_logging_enabled
        || (record.route.zero_cache_debug_enabled
            && record.status.is_success()
            && record.usage.input_cached_tokens <= 0);
    let client_request_body_json = capture_request_details
        .then(|| record.meta.client_request_body_json.clone())
        .flatten();
    let upstream_request_body_json = capture_request_details
        .then(|| record.meta.upstream_request_body_json.clone())
        .flatten();
    let full_request_json = capture_request_details
        .then(|| {
            record
                .meta
                .full_request_json
                .clone()
                .or_else(|| record.meta.client_request_body_json.clone())
        })
        .flatten();
    let event = UsageEvent {
        event_id: format!("llm-usage-{}", uuid::Uuid::new_v4()),
        created_at_ms: now_millis(),
        provider_type: ProviderType::Kiro,
        protocol_family: ProtocolFamily::Anthropic,
        key_id: record.key.key_id.clone(),
        key_name: record.key.key_name.clone(),
        account_name: Some(record.route.account_name.clone()),
        account_group_id_at_event: record.route.account_group_id_at_event.clone(),
        route_strategy_at_event: Some(record.route.route_strategy_at_event),
        request_method: record.meta.request_method.clone(),
        request_url: record.meta.request_url.clone(),
        endpoint: record.endpoint.to_string(),
        model: Some(record.model.to_string()),
        mapped_model: None,
        status_code: record.status.as_u16() as i64,
        request_body_bytes: record.meta.request_body_bytes,
        quota_failover_count: record.meta.quota_failover_count,
        routing_diagnostics_json: record.meta.routing_diagnostics_json.clone(),
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
        client_ip: record.meta.client_ip.clone(),
        ip_region: record.meta.ip_region.clone(),
        request_headers_json: record.meta.request_headers_json.clone(),
        last_message_content: record.meta.last_message_content.clone(),
        client_request_body_json,
        upstream_request_body_json,
        full_request_json,
        timing: record.meta.to_timing(),
    };
    record.control_store.apply_usage_rollup(&event).await
}

struct KiroWebsearchUsageRecord<'a> {
    control_store: &'a dyn ControlStore,
    key: &'a AuthenticatedKey,
    route: &'a ProviderKiroRoute,
    model: &'a str,
    status: StatusCode,
    usage: KiroUsageSummary,
    meta: &'a ProviderUsageMetadata,
    capture_request_details: bool,
}

async fn record_kiro_websearch_usage(record: KiroWebsearchUsageRecord<'_>) -> anyhow::Result<()> {
    let multipliers =
        parse_kiro_billable_model_multipliers_json(&record.route.billable_model_multipliers_json)?;
    let event = UsageEvent {
        event_id: format!("llm-usage-{}", uuid::Uuid::new_v4()),
        created_at_ms: now_millis(),
        provider_type: ProviderType::Kiro,
        protocol_family: ProtocolFamily::Anthropic,
        key_id: record.key.key_id.clone(),
        key_name: record.key.key_name.clone(),
        account_name: Some(record.route.account_name.clone()),
        account_group_id_at_event: record.route.account_group_id_at_event.clone(),
        route_strategy_at_event: Some(record.route.route_strategy_at_event),
        request_method: record.meta.request_method.clone(),
        request_url: record.meta.request_url.clone(),
        endpoint: "/mcp".to_string(),
        model: Some(record.model.to_string()),
        mapped_model: None,
        status_code: record.status.as_u16() as i64,
        request_body_bytes: record.meta.request_body_bytes,
        quota_failover_count: record.meta.quota_failover_count,
        routing_diagnostics_json: record.meta.routing_diagnostics_json.clone(),
        input_uncached_tokens: i64::from(record.usage.input_uncached_tokens.max(0)),
        input_cached_tokens: i64::from(record.usage.input_cached_tokens.max(0)),
        output_tokens: i64::from(record.usage.output_tokens.max(0)),
        billable_tokens: clamp_u64_to_i64(kiro_billable_tokens_with_multipliers(
            record.model,
            record.usage,
            &multipliers,
        )),
        credit_usage: None,
        usage_missing: false,
        credit_usage_missing: true,
        client_ip: record.meta.client_ip.clone(),
        ip_region: record.meta.ip_region.clone(),
        request_headers_json: record.meta.request_headers_json.clone(),
        last_message_content: record.meta.last_message_content.clone(),
        client_request_body_json: record
            .capture_request_details
            .then(|| record.meta.client_request_body_json.clone())
            .flatten(),
        upstream_request_body_json: record
            .capture_request_details
            .then(|| record.meta.upstream_request_body_json.clone())
            .flatten(),
        full_request_json: record
            .capture_request_details
            .then(|| {
                record
                    .meta
                    .full_request_json
                    .clone()
                    .or_else(|| record.meta.client_request_body_json.clone())
            })
            .flatten(),
        timing: record.meta.to_timing(),
    };
    record.control_store.apply_usage_rollup(&event).await
}

fn kiro_billable_tokens(model: &str, usage: KiroUsageSummary, cache_ctx: &KiroCacheContext) -> u64 {
    kiro_billable_tokens_with_multipliers(model, usage, &cache_ctx.billable_model_multipliers)
}

fn kiro_billable_tokens_with_multipliers(
    model: &str,
    usage: KiroUsageSummary,
    multipliers: &BTreeMap<String, f64>,
) -> u64 {
    compute_kiro_billable_tokens(
        Some(model),
        usage.input_uncached_tokens.max(0) as u64,
        usage.input_cached_tokens.max(0) as u64,
        usage.output_tokens.max(0) as u64,
        multipliers,
    )
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

    let _activity_guard = state.request_activity.start(&key.key_id);
    state
        .dispatcher
        .dispatch(key, request, state.dispatch_deps())
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

#[derive(Debug, Clone)]
struct CodexAuthSnapshot {
    access_token: String,
    account_id: Option<String>,
    is_fedramp_account: bool,
}

pub(crate) fn codex_upstream_base_url() -> String {
    std::env::var("CODEX_UPSTREAM_BASE_URL")
        .or_else(|_| std::env::var("STATICFLOW_LLM_GATEWAY_UPSTREAM_BASE_URL"))
        .map(|value| llm_access_codex::request::normalize_upstream_base_url(&value))
        .unwrap_or_else(|_| "https://chatgpt.com/backend-api/codex".to_string())
}

fn compute_codex_upstream_url(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    if base.contains("/backend-api/codex") && path.starts_with("/v1/") {
        format!("{}{}", base, path.trim_start_matches("/v1"))
    } else if base.ends_with("/v1") && path.starts_with("/v1") {
        format!("{}{}", base.trim_end_matches("/v1"), path)
    } else {
        format!("{base}{path}")
    }
}

fn normalize_codex_client_version(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_CODEX_CLIENT_VERSION_LEN {
        return None;
    }
    if !trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_'))
    {
        return None;
    }
    Some(trimmed.to_string())
}

pub(crate) fn resolve_codex_client_version(raw: Option<&str>) -> String {
    raw.and_then(normalize_codex_client_version)
        .unwrap_or_else(|| llm_access_core::store::DEFAULT_CODEX_CLIENT_VERSION.to_string())
}

async fn load_codex_client_version(
    admin_config_store: &dyn AdminConfigStore,
) -> Result<String, Response> {
    match admin_config_store.get_admin_runtime_config().await {
        Ok(config) => Ok(resolve_codex_client_version(Some(&config.codex_client_version))),
        Err(_) => {
            Err((StatusCode::INTERNAL_SERVER_ERROR, "runtime config store error").into_response())
        },
    }
}

fn codex_user_agent(client_version: &str) -> String {
    format!("{DEFAULT_WIRE_ORIGINATOR}/{client_version}")
}

fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn add_codex_upstream_headers(
    mut upstream: reqwest::RequestBuilder,
    request_headers: &HeaderMap,
    prepared: &PreparedGatewayRequest,
    auth: &CodexAuthSnapshot,
    codex_client_version: &str,
) -> reqwest::RequestBuilder {
    let incoming_session_id = header_value(request_headers, "session_id");
    let incoming_client_request_id = header_value(request_headers, "x-client-request-id");
    let mut incoming_turn_state = header_value(request_headers, "x-codex-turn-state");
    let thread_anchor = prepared
        .thread_anchor
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let is_compact_request = prepared.original_path.starts_with("/v1/responses/compact");
    let effective_client_request_id = if is_compact_request {
        incoming_client_request_id.as_deref()
    } else {
        thread_anchor.or(incoming_client_request_id.as_deref())
    };
    if let (Some(anchor), Some(legacy_session_id)) = (thread_anchor, incoming_session_id.as_deref())
    {
        if legacy_session_id.trim() != anchor {
            incoming_turn_state = None;
        }
    } else if incoming_session_id.is_none() && thread_anchor.is_none() {
        incoming_turn_state = None;
    }
    let effective_session_id = thread_anchor.or(incoming_session_id.as_deref());

    upstream = upstream
        .bearer_auth(&auth.access_token)
        .header(
            reqwest::header::ACCEPT,
            if prepared.wants_stream || prepared.force_upstream_stream {
                "text/event-stream"
            } else {
                "application/json"
            },
        )
        .header(
            reqwest::header::USER_AGENT,
            header_value(request_headers, header::USER_AGENT.as_str())
                .unwrap_or_else(|| codex_user_agent(codex_client_version)),
        )
        .header(
            reqwest::header::HeaderName::from_static("originator"),
            header_value(request_headers, "originator")
                .unwrap_or_else(|| DEFAULT_WIRE_ORIGINATOR.to_string()),
        );
    if !prepared.request_body.is_empty() {
        upstream = upstream
            .header(reqwest::header::CONTENT_TYPE, prepared.content_type.as_str())
            .body(prepared.request_body.clone());
    }
    if let Some(client_request_id) = effective_client_request_id {
        upstream = upstream.header("x-client-request-id", client_request_id);
    }
    if let Some(turn_state) = incoming_turn_state.as_deref() {
        upstream = upstream.header("x-codex-turn-state", turn_state);
    }
    for header_name in [
        "openai-beta",
        "x-openai-subagent",
        "x-codex-beta-features",
        "x-codex-turn-metadata",
        "x-codex-installation-id",
        "x-codex-parent-thread-id",
        "x-codex-window-id",
        "x-openai-memgen-request",
        "x-responsesapi-include-timing-metrics",
        "traceparent",
        "tracestate",
        "baggage",
    ] {
        if let Some(value) = header_value(request_headers, header_name) {
            upstream = upstream.header(header_name, value);
        }
    }
    if let Some(session_id) = effective_session_id {
        upstream = upstream.header("session_id", session_id);
    }
    if let Some(account_id) = auth.account_id.as_deref() {
        upstream = upstream.header("chatgpt-account-id", account_id);
    }
    if auth.is_fedramp_account {
        upstream = upstream.header("x-openai-fedramp", "true");
    }
    upstream
}

struct CompletedCodexSse {
    response: Value,
    usage: Option<UsageBreakdown>,
}

#[derive(Default)]
struct CompletedCodexSseAccumulator {
    response: Option<Value>,
    usage: Option<UsageBreakdown>,
    output_items: BTreeMap<u64, Value>,
    delta_text: String,
    done_text: Option<String>,
    fallback_item_id: Option<String>,
}

impl CompletedCodexSseAccumulator {
    fn observe_payload(&mut self, data: &str) -> Result<(), &'static str> {
        let value =
            serde_json::from_str::<Value>(data).map_err(|_| "invalid codex upstream SSE JSON")?;
        if let Some(observed_usage) = extract_usage_from_bytes(data.as_bytes()) {
            self.usage = Some(observed_usage);
        }

        match value.get("type").and_then(Value::as_str) {
            Some("response.output_item.done") => {
                if let Some(item) = value.get("item") {
                    let output_index = value
                        .get("output_index")
                        .and_then(Value::as_u64)
                        .unwrap_or(self.output_items.len() as u64);
                    self.output_items.insert(output_index, item.clone());
                }
            },
            Some("response.output_text.delta") => {
                self.capture_fallback_item_id(&value);
                if let Some(delta) = value.get("delta").and_then(Value::as_str) {
                    self.delta_text.push_str(delta);
                }
            },
            Some("response.output_text.done") => {
                self.capture_fallback_item_id(&value);
                if let Some(text) = value.get("text").and_then(Value::as_str) {
                    self.done_text = Some(text.to_string());
                }
            },
            Some("response.completed") => {
                self.response = Some(
                    value
                        .get("response")
                        .cloned()
                        .ok_or("codex upstream response.completed event is missing response")?,
                );
            },
            _ => {},
        }

        Ok(())
    }

    fn capture_fallback_item_id(&mut self, value: &Value) {
        if self.fallback_item_id.is_none() {
            self.fallback_item_id = value
                .get("item_id")
                .and_then(Value::as_str)
                .map(ToString::to_string);
        }
    }

    fn finish(mut self) -> Result<CompletedCodexSse, &'static str> {
        let mut response = self
            .response
            .take()
            .ok_or("codex upstream SSE stream did not include response.completed")?;
        self.patch_empty_completed_output(&mut response);
        Ok(CompletedCodexSse {
            response,
            usage: self.usage,
        })
    }

    fn patch_empty_completed_output(&self, response: &mut Value) {
        if response
            .get("output")
            .and_then(Value::as_array)
            .is_some_and(|items| !items.is_empty())
        {
            return;
        }

        let output = if self.output_items.is_empty() {
            let Some(text) = self
                .done_text
                .as_deref()
                .filter(|text| !text.is_empty())
                .or_else(|| (!self.delta_text.is_empty()).then_some(self.delta_text.as_str()))
            else {
                return;
            };
            let item_id = self.fallback_item_id.as_deref().unwrap_or("msg_0");
            serde_json::json!([{
                "id": item_id,
                "type": "message",
                "role": "assistant",
                "status": "completed",
                "content": [{
                    "type": "output_text",
                    "text": text
                }]
            }])
        } else {
            Value::Array(self.output_items.values().cloned().collect())
        };

        if let Some(response) = response.as_object_mut() {
            response.insert("output".to_string(), output);
        }
    }
}

fn completed_response_from_sse_bytes(bytes: &[u8]) -> Result<CompletedCodexSse, &'static str> {
    let mut accumulator = CompletedCodexSseAccumulator::default();
    for data in sse_data_payloads(bytes) {
        if data.trim() == "[DONE]" {
            continue;
        }
        accumulator.observe_payload(&data)?;
    }
    accumulator.finish()
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
    route: &ProviderCodexRoute,
    usage: UsageBreakdown,
    meta: &ProviderUsageMetadata,
) -> anyhow::Result<()> {
    let event = UsageEvent {
        event_id: format!("llm-usage-{}", uuid::Uuid::new_v4()),
        created_at_ms: now_millis(),
        provider_type: ProviderType::Codex,
        protocol_family: ProtocolFamily::OpenAi,
        key_id: key.key_id.clone(),
        key_name: key.key_name.clone(),
        account_name: Some(route.account_name.clone()),
        account_group_id_at_event: route.account_group_id_at_event.clone(),
        route_strategy_at_event: Some(route.route_strategy_at_event),
        request_method: meta.request_method.clone(),
        request_url: meta.request_url.clone(),
        endpoint: prepared.original_path.clone(),
        model: prepared
            .client_visible_model
            .clone()
            .or_else(|| prepared.model.clone()),
        mapped_model: prepared.model.clone(),
        status_code: i64::from(status.as_u16()),
        request_body_bytes: meta
            .request_body_bytes
            .or(Some(clamp_usize_to_i64(prepared.client_request_body.len()))),
        quota_failover_count: meta.quota_failover_count,
        routing_diagnostics_json: meta.routing_diagnostics_json.clone(),
        input_uncached_tokens: clamp_u64_to_i64(usage.input_uncached_tokens),
        input_cached_tokens: clamp_u64_to_i64(usage.input_cached_tokens),
        output_tokens: clamp_u64_to_i64(usage.output_tokens),
        billable_tokens: clamp_u64_to_i64(
            usage.billable_tokens_with_multiplier(prepared.billable_multiplier),
        ),
        credit_usage: None,
        usage_missing: usage.usage_missing,
        credit_usage_missing: false,
        client_ip: meta.client_ip.clone(),
        ip_region: meta.ip_region.clone(),
        request_headers_json: meta.request_headers_json.clone(),
        last_message_content: meta.last_message_content.clone(),
        client_request_body_json: meta.client_request_body_json.clone(),
        upstream_request_body_json: meta.upstream_request_body_json.clone(),
        full_request_json: meta.full_request_json.clone(),
        timing: meta.to_timing(),
    };
    control_store.apply_usage_rollup(&event).await
}

fn missing_codex_usage() -> UsageBreakdown {
    UsageBreakdown {
        usage_missing: true,
        ..UsageBreakdown::default()
    }
}

fn extract_last_message_from_kiro_messages(payload: &MessagesRequest) -> Option<String> {
    let current_range = current_user_message_range(&payload.messages).ok()?;
    let tool_name_by_id = collect_kiro_tool_name_map(&payload.messages[..current_range.start]);
    let mut parts = Vec::new();
    for message in &payload.messages[current_range] {
        append_kiro_message_summary_parts(&message.content, &tool_name_by_id, &mut parts);
    }
    if parts.is_empty() {
        None
    } else {
        Some(truncate_summary(&parts.join("\n"), KIRO_LAST_MESSAGE_TOTAL_PREVIEW_CHARS))
    }
}

fn collect_kiro_tool_name_map(
    messages: &[llm_access_kiro::anthropic::types::Message],
) -> HashMap<String, String> {
    let mut tool_name_by_id = HashMap::new();
    for message in messages {
        let Some(blocks) = message.content.as_array() else {
            continue;
        };
        for block in blocks {
            if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                continue;
            }
            let Some(tool_use_id) = block
                .get("id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                continue;
            };
            let Some(tool_name) = block
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                continue;
            };
            tool_name_by_id.insert(tool_use_id.to_string(), tool_name.to_string());
        }
    }
    tool_name_by_id
}

fn append_kiro_message_summary_parts(
    content: &Value,
    tool_name_by_id: &HashMap<String, String>,
    parts: &mut Vec<String>,
) {
    match content {
        Value::String(text) => {
            if let Some(summary) = summarize_text(text) {
                parts.push(summary);
            }
        },
        Value::Array(blocks) => {
            for block in blocks {
                match block.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        if let Some(text) = block.get("text").and_then(Value::as_str) {
                            if let Some(summary) = summarize_text(text) {
                                parts.push(summary);
                            }
                        }
                    },
                    Some("tool_result") => {
                        if let Some(summary) = summarize_tool_result(block, tool_name_by_id) {
                            parts.push(summary);
                        }
                    },
                    Some("tool_use") => {
                        if let Some(name) = block.get("name").and_then(Value::as_str) {
                            if let Some(summary) = summarize_text(&format!("[tool_use:{name}]")) {
                                parts.push(summary);
                            }
                        }
                    },
                    Some("image") => parts.push("[image]".to_string()),
                    _ => {},
                }
            }
        },
        _ => {},
    }
}

fn summarize_tool_result(
    block: &Value,
    tool_name_by_id: &HashMap<String, String>,
) -> Option<String> {
    let tool_use_id = block
        .get("tool_use_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let label = tool_name_by_id
        .get(tool_use_id)
        .map(String::as_str)
        .unwrap_or(tool_use_id);
    let preview = extract_tool_result_content(&block.get("content").cloned());
    let preview = compact_preview(&preview, KIRO_LAST_MESSAGE_PART_PREVIEW_CHARS);
    Some(if preview.is_empty() {
        format!("[tool_result:{label}]")
    } else {
        format!("[tool_result:{label}] {preview}")
    })
}

fn summarize_text(text: &str) -> Option<String> {
    let preview = compact_preview(text, KIRO_LAST_MESSAGE_PART_PREVIEW_CHARS);
    (!preview.is_empty()).then_some(preview)
}

fn compact_preview(text: &str, max_chars: usize) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_summary(compact.trim(), max_chars)
}

fn truncate_summary(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
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

fn clamp_duration_ms(duration: Duration) -> i64 {
    duration.as_millis().min(i64::MAX as u128) as i64
}

pub(crate) async fn codex_openai_models_response(
    route: ProviderCodexRoute,
    route_store: Arc<dyn ProviderRouteStore>,
    request_headers: &HeaderMap,
    query: &str,
    upstream_base: &str,
    default_codex_client_version: &str,
) -> Response {
    let (payload, etag) = match fetch_codex_models_payload(
        &route,
        route_store,
        request_headers,
        query,
        upstream_base,
        default_codex_client_version,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    let body = match serde_json::to_vec(
        &llm_access_codex::models::openai_models_response_value_from_catalog(
            &payload,
            route.map_gpt53_codex_to_spark,
            now_seconds(),
        ),
    ) {
        Ok(body) => body,
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "failed to encode codex models response")
                .into_response()
        },
    };
    codex_models_json_response(
        body,
        "application/json",
        None,
        etag.as_deref(),
        "failed to build codex models response",
    )
}

pub(crate) async fn codex_public_model_catalog_response(
    route: ProviderCodexRoute,
    route_store: Arc<dyn ProviderRouteStore>,
    request_headers: &HeaderMap,
    query: &str,
    upstream_base: &str,
    default_codex_client_version: &str,
) -> Response {
    let (payload, etag) = match fetch_codex_models_payload(
        &route,
        route_store,
        request_headers,
        query,
        upstream_base,
        default_codex_client_version,
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    let catalog = match llm_access_codex::models::normalize_public_model_catalog_value(
        payload,
        route.map_gpt53_codex_to_spark,
    ) {
        Ok(value) => value,
        Err(_) => {
            return (StatusCode::BAD_GATEWAY, "failed to normalize codex model catalog")
                .into_response()
        },
    };
    let body = match serde_json::to_vec(&catalog) {
        Ok(body) => body,
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "failed to encode codex model catalog")
                .into_response()
        },
    };
    codex_models_json_response(
        body,
        "application/json; charset=utf-8",
        Some(r#"inline; filename="model_catalog.json""#),
        etag.as_deref(),
        "failed to build codex model catalog response",
    )
}

pub(crate) fn default_codex_public_model_catalog_response() -> Response {
    let body = match llm_access_codex::models::default_public_model_catalog_json() {
        Ok(body) => body,
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "failed to build model catalog")
                .into_response()
        },
    };
    codex_models_json_response(
        body,
        "application/json; charset=utf-8",
        Some(r#"inline; filename="model_catalog.json""#),
        None,
        "failed to build model catalog response",
    )
}

fn codex_models_json_response(
    body: Vec<u8>,
    content_type: &'static str,
    content_disposition: Option<&'static str>,
    etag: Option<&str>,
    build_error: &'static str,
) -> Response {
    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "no-store");
    if let Some(value) = content_disposition {
        builder = builder.header(header::CONTENT_DISPOSITION, value);
    }
    if let Some(value) = etag {
        builder = builder.header(header::ETAG, value);
    }
    builder
        .body(Body::from(body))
        .unwrap_or_else(|_| (StatusCode::INTERNAL_SERVER_ERROR, build_error).into_response())
}

async fn fetch_codex_models_payload(
    route: &ProviderCodexRoute,
    route_store: Arc<dyn ProviderRouteStore>,
    request_headers: &HeaderMap,
    query: &str,
    upstream_base: &str,
    default_codex_client_version: &str,
) -> Result<(Value, Option<String>), Response> {
    let mut auth =
        match codex_refresh::ensure_context_for_route(route, route_store.as_ref(), false).await {
            Ok(ctx) => CodexAuthSnapshot {
                access_token: ctx.access_token,
                account_id: ctx.account_id,
                is_fedramp_account: ctx.is_fedramp_account,
            },
            Err(_) => {
                return Err((StatusCode::BAD_GATEWAY, "codex auth refresh failed").into_response())
            },
        };
    let client_version = codex_models_client_version(query, default_codex_client_version);
    let upstream_url = llm_access_codex::models::append_client_version_query(
        &compute_codex_upstream_url(upstream_base, "/v1/models"),
        &client_version,
    );
    let client = provider_client(route.proxy.as_ref()).map_err(|_| {
        (StatusCode::BAD_GATEWAY, "codex proxy configuration failed").into_response()
    })?;
    let mut response =
        send_codex_models_request(&client, &upstream_url, request_headers, &auth, &client_version)
            .await?;
    if matches!(response.status(), StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        auth = match codex_refresh::ensure_context_for_route(route, route_store.as_ref(), true)
            .await
        {
            Ok(ctx) => CodexAuthSnapshot {
                access_token: ctx.access_token,
                account_id: ctx.account_id,
                is_fedramp_account: ctx.is_fedramp_account,
            },
            Err(_) => {
                return Err((StatusCode::BAD_GATEWAY, "codex auth refresh failed").into_response())
            },
        };
        response = send_codex_models_request(
            &client,
            &upstream_url,
            request_headers,
            &auth,
            &client_version,
        )
        .await?;
    }
    parse_codex_models_payload(response).await
}

fn codex_models_client_version(query: &str, default_codex_client_version: &str) -> String {
    query
        .trim_start_matches('?')
        .split('&')
        .filter_map(|part| part.split_once('='))
        .find_map(|(name, value)| {
            (name == "client_version")
                .then_some(value)
                .and_then(normalize_codex_client_version)
        })
        .unwrap_or_else(|| resolve_codex_client_version(Some(default_codex_client_version)))
}

async fn send_codex_models_request(
    client: &reqwest::Client,
    upstream_url: &str,
    request_headers: &HeaderMap,
    auth: &CodexAuthSnapshot,
    client_version: &str,
) -> Result<reqwest::Response, Response> {
    let mut request = client
        .get(upstream_url)
        .bearer_auth(&auth.access_token)
        .header(reqwest::header::ACCEPT, "application/json")
        .header(
            reqwest::header::USER_AGENT,
            header_value(request_headers, header::USER_AGENT.as_str())
                .unwrap_or_else(|| codex_user_agent(client_version)),
        )
        .header(
            reqwest::header::HeaderName::from_static("originator"),
            header_value(request_headers, "originator")
                .unwrap_or_else(|| DEFAULT_WIRE_ORIGINATOR.to_string()),
        );
    if let Some(account_id) = auth.account_id.as_deref() {
        request = request.header("chatgpt-account-id", account_id);
    }
    if auth.is_fedramp_account {
        request = request.header("x-openai-fedramp", "true");
    }
    request.send().await.map_err(|_| {
        (StatusCode::BAD_GATEWAY, "codex models upstream request failed").into_response()
    })
}

async fn parse_codex_models_payload(
    response: reqwest::Response,
) -> Result<(Value, Option<String>), Response> {
    let status = response.status();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);
    let body = response.bytes().await.map_err(|_| {
        (StatusCode::BAD_GATEWAY, "codex models upstream response read failed").into_response()
    })?;
    if !status.is_success() {
        return Err((
            StatusCode::BAD_GATEWAY,
            format!(
                "codex models upstream returned status={} body={}",
                status,
                summarize_body_hint(body.as_ref())
            ),
        )
            .into_response());
    }
    if content_type.contains("text/html") {
        return Err((
            StatusCode::BAD_GATEWAY,
            format!(
                "codex models upstream returned html body={}",
                summarize_body_hint(body.as_ref())
            ),
        )
            .into_response());
    }
    let value = serde_json::from_slice::<Value>(body.as_ref()).map_err(|_| {
        (StatusCode::BAD_GATEWAY, "codex models upstream returned invalid json").into_response()
    })?;
    Ok((value, etag))
}

fn summarize_body_hint(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        "empty body".to_string()
    } else {
        trimmed.chars().take(200).collect()
    }
}

fn now_seconds() -> i64 {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    seconds.min(i64::MAX as u64) as i64
}

#[cfg(test)]
#[allow(
    clippy::await_holding_lock,
    reason = "provider tests serialize process-wide upstream env var overrides across awaited \
              requests"
)]
mod tests {
    use std::{
        collections::BTreeMap,
        sync::{Arc, Mutex},
        time::{Duration, Instant},
    };

    use async_trait::async_trait;
    use axum::{
        body::{to_bytes, Body},
        extract::State,
        http::{header, HeaderMap, Request, StatusCode},
        response::{IntoResponse, Response},
        routing::{get, post},
        Json, Router,
    };
    use llm_access_core::{
        provider::RouteStrategy,
        store::{
            AdminConfigStore, AdminKiroStatusCacheUpdate, AdminRuntimeConfig, AuthenticatedKey,
            ControlStore, EmptyProviderRouteStore, ProviderCodexRoute, ProviderKiroAuthUpdate,
            ProviderKiroRoute, ProviderProxyConfig, ProviderRouteStore,
        },
    };
    use serde_json::json;
    use tokio::sync::Notify;

    use super::ProviderDispatcher;

    static CODEX_UPSTREAM_ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn codex_backend_api_base_uses_upstream_codex_paths() {
        assert_eq!(
            super::compute_codex_upstream_url(
                "https://chatgpt.com/backend-api/codex",
                "/v1/responses"
            ),
            "https://chatgpt.com/backend-api/codex/responses"
        );
        assert_eq!(
            super::compute_codex_upstream_url("https://api.example.com/v1", "/v1/responses"),
            "https://api.example.com/v1/responses"
        );
    }

    #[derive(Debug)]
    struct StaticRemoteMediaFetcher {
        media_type: &'static str,
        bytes: &'static [u8],
    }

    #[async_trait]
    impl super::KiroRemoteMediaFetcher for StaticRemoteMediaFetcher {
        async fn fetch(
            &self,
            request: super::KiroRemoteMediaRequest<'_>,
        ) -> Result<super::ResolvedKiroRemoteMedia, super::KiroRemoteMediaResolutionError> {
            assert_eq!(request.url, "https://example.test/asset");
            Ok(super::ResolvedKiroRemoteMedia {
                media_type: Some(self.media_type.to_string()),
                bytes: super::Bytes::from_static(self.bytes),
            })
        }
    }

    #[tokio::test]
    async fn kiro_remote_media_resolver_rewrites_url_image_sources() {
        let mut payload = serde_json::from_value::<llm_access_kiro::anthropic::types::MessagesRequest>(
            json!({
                "model": "claude-sonnet-4-6",
                "max_tokens": 128,
                "messages": [{
                    "role": "user",
                    "content": [
                        {"type": "image", "source": {"type": "url", "url": "https://example.test/asset"}},
                        {"type": "text", "text": "Describe it"}
                    ]
                }]
            }),
        )
        .expect("request payload");

        super::resolve_kiro_remote_media_sources_with_fetcher(
            &mut payload,
            &StaticRemoteMediaFetcher {
                media_type: "image/png",
                bytes: b"hello",
            },
        )
        .await
        .expect("remote media should resolve");

        let source = &payload.messages[0].content[0]["source"];
        assert_eq!(source["type"], "base64");
        assert_eq!(source["media_type"], "image/png");
        assert_eq!(source["data"], "aGVsbG8=");
    }

    #[tokio::test]
    async fn kiro_remote_media_resolver_rewrites_url_pdf_documents() {
        let mut payload =
            serde_json::from_value::<llm_access_kiro::anthropic::types::MessagesRequest>(json!({
                "model": "claude-sonnet-4-6",
                "max_tokens": 128,
                "messages": [{
                    "role": "user",
                    "content": [{
                        "type": "document",
                        "source": {"type": "url", "url": "https://example.test/asset"}
                    }]
                }]
            }))
            .expect("request payload");

        super::resolve_kiro_remote_media_sources_with_fetcher(
            &mut payload,
            &StaticRemoteMediaFetcher {
                media_type: "application/pdf",
                bytes: b"%PDF-1.4",
            },
        )
        .await
        .expect("remote PDF should resolve");

        let source = &payload.messages[0].content[0]["source"];
        assert_eq!(source["type"], "base64");
        assert_eq!(source["media_type"], "application/pdf");
        assert_eq!(source["data"], "JVBERi0xLjQ=");
    }

    #[test]
    fn normalized_kiro_messages_path_accepts_root_anthropic_messages() {
        assert_eq!(
            super::normalized_kiro_messages_path("/v1/messages"),
            Some(("/v1/messages", false))
        );
    }

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

    struct StaticAdminConfigStore {
        config: AdminRuntimeConfig,
    }

    #[async_trait]
    impl AdminConfigStore for StaticAdminConfigStore {
        async fn get_admin_runtime_config(&self) -> anyhow::Result<AdminRuntimeConfig> {
            Ok(self.config.clone())
        }

        async fn update_admin_runtime_config(
            &self,
            config: AdminRuntimeConfig,
        ) -> anyhow::Result<AdminRuntimeConfig> {
            Ok(config)
        }
    }

    #[derive(Default)]
    struct CapturingDispatcher {
        seen: Mutex<Vec<(String, String)>>,
    }

    #[derive(Default)]
    struct BlockingDispatcher {
        entered: Notify,
        release: Notify,
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

        async fn save_kiro_auth_update(
            &self,
            _update: ProviderKiroAuthUpdate,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        async fn save_codex_auth_update(
            &self,
            _update: llm_access_core::store::ProviderCodexAuthUpdate,
        ) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[derive(Clone)]
    struct CapturingKiroStatusRouteStore {
        route: Arc<Mutex<ProviderKiroRoute>>,
        updates: Arc<Mutex<Vec<AdminKiroStatusCacheUpdate>>>,
    }

    #[async_trait]
    impl ProviderRouteStore for CapturingKiroStatusRouteStore {
        async fn resolve_codex_route(
            &self,
            _key: &AuthenticatedKey,
        ) -> anyhow::Result<Option<ProviderCodexRoute>> {
            Ok(None)
        }

        async fn resolve_kiro_route(
            &self,
            _key: &AuthenticatedKey,
        ) -> anyhow::Result<Option<ProviderKiroRoute>> {
            Ok(Some(self.route.lock().expect("route").clone()))
        }

        async fn save_kiro_auth_update(
            &self,
            _update: ProviderKiroAuthUpdate,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        async fn save_codex_auth_update(
            &self,
            _update: llm_access_core::store::ProviderCodexAuthUpdate,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        async fn save_kiro_status_cache_update(
            &self,
            update: AdminKiroStatusCacheUpdate,
        ) -> anyhow::Result<()> {
            {
                let mut route = self.route.lock().expect("route");
                route.cached_status = Some(update.cache.status.clone());
                route.cached_remaining_credits =
                    update.balance.as_ref().map(|balance| balance.remaining);
                route.routing_identity = update
                    .balance
                    .as_ref()
                    .and_then(|balance| balance.user_id.clone())
                    .unwrap_or_else(|| route.account_name.clone());
                route.cached_balance = update.balance.clone();
                route.cached_cache = Some(update.cache.clone());
            }
            self.updates.lock().expect("updates").push(update);
            Ok(())
        }
    }

    #[derive(Debug, Default)]
    struct CapturedCodexUpstream {
        requests: Mutex<Vec<CapturedCodexRequest>>,
    }

    #[derive(Debug)]
    struct CapturedCodexRequest {
        path: String,
        query: Option<String>,
        authorization: Option<String>,
        accept: Option<String>,
        user_agent: Option<String>,
        x_client_request_id: Option<String>,
        session_id: Option<String>,
        x_codex_turn_state: Option<String>,
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
            _deps: super::ProviderDispatchDeps,
        ) -> Response {
            self.seen
                .lock()
                .expect("dispatcher state")
                .push((key.key_id, request.uri().path().to_string()));
            (StatusCode::ACCEPTED, "dispatched").into_response()
        }
    }

    #[async_trait]
    impl ProviderDispatcher for BlockingDispatcher {
        async fn dispatch(
            &self,
            _key: AuthenticatedKey,
            _request: Request<Body>,
            _deps: super::ProviderDispatchDeps,
        ) -> Response {
            self.entered.notify_one();
            self.release.notified().await;
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
                account_group_id_at_event: None,
                route_strategy_at_event: RouteStrategy::Auto,
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
                account_group_id_at_event: None,
                route_strategy_at_event: RouteStrategy::Auto,
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
            account_group_id_at_event: None,
            route_strategy_at_event: RouteStrategy::Auto,
            auth_json: r#"{"accessToken":"kiro-upstream-token","machineId":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#.to_string(),
            profile_arn: Some("arn:aws:kiro:test".to_string()),
            api_region: "us-east-1".to_string(),
            request_validation_enabled: true,
            cache_estimation_enabled: true,
            zero_cache_debug_enabled: false,
            full_request_logging_enabled: false,
            model_name_map_json: "{}".to_string(),
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
            routing_identity: "kiro-a".to_string(),
            cached_status: Some("ready".to_string()),
            cached_remaining_credits: Some(100.0),
            cached_balance: Some(llm_access_core::store::AdminKiroBalanceView {
                current_usage: 0.0,
                usage_limit: 100.0,
                remaining: 100.0,
                next_reset_at: None,
                subscription_title: None,
                user_id: Some("kiro-a".to_string()),
            }),
            cached_cache: Some(llm_access_core::store::AdminKiroCacheView {
                status: "ready".to_string(),
                refresh_interval_seconds: 300,
                last_checked_at: Some(1),
                last_success_at: Some(1),
                error_message: None,
            }),
            status_refresh_interval_seconds: 300,
            minimum_remaining_credits_before_block: 0.0,
        }
    }

    fn kiro_route_for_selection(
        account_name: &str,
        routing_identity: &str,
        remaining: f64,
        proxy_url: Option<&str>,
    ) -> ProviderKiroRoute {
        let mut route = static_kiro_route();
        route.account_name = account_name.to_string();
        route.routing_identity = routing_identity.to_string();
        route.cached_remaining_credits = Some(remaining);
        route.proxy = proxy_url.map(|proxy_url| ProviderProxyConfig {
            proxy_url: proxy_url.to_string(),
            proxy_username: None,
            proxy_password: None,
        });
        route
    }

    #[test]
    fn anthropic_usage_json_with_policy_matches_backend_cache_creation_semantics() {
        let mut policy = llm_access_kiro::cache_policy::default_kiro_cache_policy();
        policy.anthropic_cache_creation_input_ratio = 0.25;

        let usage = super::anthropic_usage_json_with_policy(&policy, 200, 7, 20);

        assert_eq!(usage["input_tokens"], 135);
        assert_eq!(usage["cache_creation_input_tokens"], 45);
        assert_eq!(usage["cache_read_input_tokens"], 20);
        assert_eq!(usage["output_tokens"], 7);

        policy.anthropic_cache_creation_input_ratio = 0.0;
        let no_cache_read = super::anthropic_usage_json_with_policy(&policy, 100, 3, 0);
        assert_eq!(no_cache_read["input_tokens"], 50);
        assert_eq!(no_cache_read["cache_creation_input_tokens"], 50);
        assert_eq!(no_cache_read["cache_read_input_tokens"], 0);
    }

    #[test]
    fn kiro_selection_prefers_balance_then_least_recently_started_identity() {
        let scheduler = llm_access_kiro::scheduler::KiroRequestScheduler::new();
        let routes = vec![
            kiro_route_for_selection("alpha", "user-alpha", 90.0, None),
            kiro_route_for_selection("beta", "user-beta", 10.0, None),
        ];

        let ordered = super::selection_ordered_kiro_routes(&routes, scheduler.as_ref());
        assert_eq!(ordered[0].account_name, "alpha");

        let lease = scheduler
            .try_acquire("user-alpha", 1, 0, Instant::now())
            .expect("alpha should acquire");
        drop(lease);

        let ordered = super::selection_ordered_kiro_routes(&routes, scheduler.as_ref());
        assert_eq!(ordered[0].account_name, "beta");
    }

    #[test]
    fn kiro_selection_deprioritizes_routes_on_cooled_proxy() {
        let scheduler = llm_access_kiro::scheduler::KiroRequestScheduler::new();
        scheduler.mark_proxy_cooldown(
            "url:http://proxy-a",
            Duration::from_secs(60),
            "transient invalid model",
        );
        let routes = vec![
            kiro_route_for_selection("alpha", "user-alpha", 90.0, Some("http://proxy-a")),
            kiro_route_for_selection("beta", "user-beta", 10.0, Some("http://proxy-b")),
        ];

        let ordered = super::selection_ordered_kiro_routes(&routes, scheduler.as_ref());
        assert_eq!(ordered[0].account_name, "beta");
    }

    #[test]
    fn kiro_quota_exhaustion_groups_accounts_by_routing_identity() {
        let routes = vec![
            kiro_route_for_selection("alpha", "same-user", 90.0, None),
            kiro_route_for_selection("beta", "same-user", 10.0, None),
            kiro_route_for_selection("gamma", "other-user", 80.0, None),
        ];

        assert_eq!(super::account_names_for_kiro_routing_identity(&routes, "same-user"), vec![
            "alpha".to_string(),
            "beta".to_string()
        ]);
    }

    #[test]
    fn kiro_opus_vision_bridge_is_required_only_for_opus_46_images() {
        let opus_payload = serde_json::from_value(json!({
            "model": "claude-opus-4-6",
            "max_tokens": 64,
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "What color is this?"},
                    {
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": "image/png",
                            "data": "aGVsbG8="
                        }
                    }
                ]
            }]
        }))
        .expect("request should deserialize");
        assert!(super::kiro_opus_vision_bridge_required(&opus_payload));

        let mut sonnet_payload = opus_payload.clone();
        sonnet_payload.model = "claude-sonnet-4-6".to_string();
        assert!(!super::kiro_opus_vision_bridge_required(&sonnet_payload));

        let mut text_payload = opus_payload;
        text_payload.messages[0].content = json!("hello");
        assert!(!super::kiro_opus_vision_bridge_required(&text_payload));
    }

    #[test]
    fn kiro_vision_bridge_request_uses_sonnet_origin_and_images() {
        let route = static_kiro_route();
        let body = super::build_kiro_vision_bridge_request(
            &route,
            &[super::KiroVisionBridgeImage {
                format: "png".to_string(),
                data: "aGVsbG8=".to_string(),
            }],
            "What color is this?",
        )
        .expect("bridge request should encode");
        let value: serde_json::Value =
            serde_json::from_slice(&body).expect("bridge request should be JSON");
        let current = &value["conversationState"]["currentMessage"]["userInputMessage"];
        assert_eq!(current["modelId"], "claude-sonnet-4.6");
        assert_eq!(current["origin"], "AI_EDITOR");
        assert_eq!(current["images"][0]["format"], "png");
        assert_eq!(value["profileArn"], "arn:aws:kiro:test");
    }

    #[test]
    fn kiro_vision_bridge_context_replaces_image_blocks_with_text() {
        let mut payload: llm_access_kiro::anthropic::types::MessagesRequest =
            serde_json::from_value(json!({
                "model": "claude-opus-4-6",
                "max_tokens": 64,
                "messages": [{
                    "role": "user",
                    "content": [
                        {"type": "text", "text": "What color is this?"},
                        {
                            "type": "image",
                            "source": {
                                "type": "base64",
                                "media_type": "image/png",
                                "data": "aGVsbG8="
                            }
                        }
                    ]
                }]
            }))
            .expect("request should deserialize");

        super::inject_kiro_vision_bridge_context(&mut payload, "Image 1: a solid red square.");

        let items = payload.messages[0]
            .content
            .as_array()
            .expect("content should remain array");
        assert!(!items
            .iter()
            .any(|item| item.get("type").and_then(serde_json::Value::as_str) == Some("image")));
        assert!(items.iter().any(|item| item
            .get("text")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|text| text.contains("solid red square"))));
    }

    #[test]
    fn kiro_upstream_error_classifiers_match_legacy_cooldowns() {
        assert_eq!(
            super::daily_request_limit_cooldown(r#"{"reason":"DAILY_REQUEST_COUNT"}"#),
            Some(Duration::from_secs(5 * 60))
        );
        assert_eq!(
            super::transient_invalid_model_cooldown(
                r#"{"reason":"INVALID_MODEL_ID","message":"Invalid model"}"#
            ),
            Some(Duration::from_secs(60))
        );
        assert!(super::is_monthly_request_limit(r#"{"error":{"reason":"MONTHLY_REQUEST_COUNT"}}"#));
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
        let query = request.uri().query().map(ToString::to_string);
        let body = to_bytes(request.into_body(), usize::MAX)
            .await
            .expect("upstream request body");
        let body = if body.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice::<serde_json::Value>(&body).expect("upstream json")
        };
        captured
            .requests
            .lock()
            .expect("captured requests")
            .push(CapturedCodexRequest {
                path,
                query,
                authorization: headers
                    .get(header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok())
                    .map(ToString::to_string),
                accept: headers
                    .get(header::ACCEPT)
                    .and_then(|value| value.to_str().ok())
                    .map(ToString::to_string),
                user_agent: headers
                    .get(header::USER_AGENT)
                    .and_then(|value| value.to_str().ok())
                    .map(ToString::to_string),
                x_client_request_id: headers
                    .get("x-client-request-id")
                    .and_then(|value| value.to_str().ok())
                    .map(ToString::to_string),
                session_id: headers
                    .get("session_id")
                    .and_then(|value| value.to_str().ok())
                    .map(ToString::to_string),
                x_codex_turn_state: headers
                    .get("x-codex-turn-state")
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

    async fn fake_codex_responses_empty_completed_output(
        State(captured): State<Arc<CapturedCodexUpstream>>,
        headers: HeaderMap,
        request: Request<Body>,
    ) -> Response {
        let path = request.uri().path().to_string();
        let query = request.uri().query().map(ToString::to_string);
        let body = to_bytes(request.into_body(), usize::MAX)
            .await
            .expect("upstream request body");
        captured
            .requests
            .lock()
            .expect("captured requests")
            .push(CapturedCodexRequest {
                path,
                query,
                authorization: headers
                    .get(header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok())
                    .map(ToString::to_string),
                accept: headers
                    .get(header::ACCEPT)
                    .and_then(|value| value.to_str().ok())
                    .map(ToString::to_string),
                user_agent: headers
                    .get(header::USER_AGENT)
                    .and_then(|value| value.to_str().ok())
                    .map(ToString::to_string),
                x_client_request_id: headers
                    .get("x-client-request-id")
                    .and_then(|value| value.to_str().ok())
                    .map(ToString::to_string),
                session_id: headers
                    .get("session_id")
                    .and_then(|value| value.to_str().ok())
                    .map(ToString::to_string),
                x_codex_turn_state: headers
                    .get("x-codex-turn-state")
                    .and_then(|value| value.to_str().ok())
                    .map(ToString::to_string),
                body: if body.is_empty() {
                    serde_json::Value::Null
                } else {
                    serde_json::from_slice::<serde_json::Value>(&body).expect("upstream json")
                },
            });

        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from(format!(
                "event: response.output_text.delta\ndata: {}\n\nevent: \
                 response.output_text.done\ndata: {}\n\nevent: response.output_item.done\ndata: \
                 {}\n\nevent: response.completed\ndata: {}\n\n",
                json!({
                    "type": "response.output_text.delta",
                    "response_id": "resp_1",
                    "created": 123,
                    "model": "gpt-5.3-codex-spark",
                    "item_id": "msg_1",
                    "output_index": 0,
                    "content_index": 0,
                    "delta": "hello back"
                }),
                json!({
                    "type": "response.output_text.done",
                    "response_id": "resp_1",
                    "created": 123,
                    "model": "gpt-5.3-codex-spark",
                    "item_id": "msg_1",
                    "output_index": 0,
                    "content_index": 0,
                    "text": "hello back"
                }),
                json!({
                    "type": "response.output_item.done",
                    "output_index": 0,
                    "item": {
                        "id": "msg_1",
                        "type": "message",
                        "role": "assistant",
                        "status": "completed",
                        "content": [{
                            "type": "output_text",
                            "text": "hello back"
                        }]
                    }
                }),
                json!({
                    "type": "response.completed",
                    "response": {
                        "id": "resp_1",
                        "created_at": 123,
                        "model": "gpt-5.3-codex-spark",
                        "output": [],
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

    async fn fake_codex_models(
        State(captured): State<Arc<CapturedCodexUpstream>>,
        headers: HeaderMap,
        request: Request<Body>,
    ) -> Response {
        let path = request.uri().path().to_string();
        let query = request.uri().query().map(ToString::to_string);
        let body = to_bytes(request.into_body(), usize::MAX)
            .await
            .expect("upstream request body");
        captured
            .requests
            .lock()
            .expect("captured requests")
            .push(CapturedCodexRequest {
                path,
                query,
                authorization: headers
                    .get(header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok())
                    .map(ToString::to_string),
                accept: headers
                    .get(header::ACCEPT)
                    .and_then(|value| value.to_str().ok())
                    .map(ToString::to_string),
                user_agent: headers
                    .get(header::USER_AGENT)
                    .and_then(|value| value.to_str().ok())
                    .map(ToString::to_string),
                x_client_request_id: headers
                    .get("x-client-request-id")
                    .and_then(|value| value.to_str().ok())
                    .map(ToString::to_string),
                session_id: headers
                    .get("session_id")
                    .and_then(|value| value.to_str().ok())
                    .map(ToString::to_string),
                x_codex_turn_state: headers
                    .get("x-codex-turn-state")
                    .and_then(|value| value.to_str().ok())
                    .map(ToString::to_string),
                body: if body.is_empty() {
                    serde_json::Value::Null
                } else {
                    serde_json::from_slice::<serde_json::Value>(&body).expect("upstream json")
                },
            });

        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ETAG, r#""models-test""#)
            .body(Body::from(
                json!({
                    "models": [
                        {"slug": "gpt-5.3-codex-spark"},
                        {"slug": "gpt-5.5"}
                    ]
                })
                .to_string(),
            ))
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

    async fn fake_kiro_usage_limits(
        State(captured): State<Arc<CapturedKiroUpstream>>,
        headers: HeaderMap,
    ) -> Response {
        captured
            .requests
            .lock()
            .expect("captured requests")
            .push(CapturedKiroRequest {
                path: "/getUsageLimits".to_string(),
                authorization: headers
                    .get(header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok())
                    .map(ToString::to_string),
                body: serde_json::Value::Null,
            });
        Json(json!({
            "subscriptionInfo": {"subscriptionTitle": "Pro"},
            "usageBreakdownList": [{
                "currentUsageWithPrecision": 10.0,
                "usageLimitWithPrecision": 100.0,
                "bonuses": [],
                "nextDateReset": 900.0
            }],
            "userInfo": {"userId": "upstream-user-1"}
        }))
        .into_response()
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
            .route("/getUsageLimits", get(fake_kiro_usage_limits))
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
    async fn provider_entry_tracks_rpm_and_in_flight_for_authenticated_requests() {
        let dispatcher = Arc::new(BlockingDispatcher::default());
        let state = test_state_with_dispatcher(dispatcher.clone());
        let request =
            request_with_x_api_key_to_path("/api/kiro-gateway/v1/messages", Some("valid-secret"));
        let task_state = state.clone();

        let handle = tokio::spawn(async move { super::provider_entry(task_state, request).await });
        dispatcher.entered.notified().await;

        let total = state.request_activity.snapshot(None);
        let key = state.request_activity.snapshot(Some("key-1"));
        assert_eq!(total.rpm, 1);
        assert_eq!(total.in_flight, 1);
        assert_eq!(key.rpm, 1);
        assert_eq!(key.in_flight, 1);

        dispatcher.release.notify_one();
        let response = handle.await.expect("provider task");
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert_eq!(state.request_activity.snapshot(Some("key-1")).in_flight, 0);
        assert_eq!(state.request_activity.snapshot(Some("key-1")).rpm, 1);
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
    async fn codex_dispatch_reconstructs_non_streaming_output_from_sse_item_events() {
        let _guard = CODEX_UPSTREAM_ENV_LOCK
            .lock()
            .expect("codex upstream env lock");
        let captured = Arc::new(CapturedCodexUpstream::default());
        let app = Router::new()
            .route("/v1/responses", post(fake_codex_responses_empty_completed_output))
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
        assert_eq!(body["usage"]["output_tokens"], 3);
    }

    #[tokio::test]
    async fn codex_dispatch_rewrites_thread_headers_like_legacy_backend() {
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
                .uri("/v1/responses")
                .header(header::AUTHORIZATION, "Bearer codex-secret")
                .header(header::CONTENT_TYPE, "application/json")
                .header("session_id", "legacy-session")
                .header("x-client-request-id", "client-request")
                .header("x-codex-turn-state", "stale-turn-state")
                .body(Body::from(
                    r#"{
                        "model": "gpt-5.3-codex",
                        "prompt_cache_key": "thread-anchor",
                        "input": "hello",
                        "stream": false
                    }"#,
                ))
                .expect("request"),
        )
        .await;

        std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

        assert_eq!(response.status(), StatusCode::OK);
        let requests = captured.requests.lock().expect("captured requests");
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].session_id.as_deref(), Some("thread-anchor"));
        assert_eq!(requests[0].x_client_request_id.as_deref(), Some("thread-anchor"));
        assert_eq!(requests[0].x_codex_turn_state, None);
    }

    #[tokio::test]
    async fn codex_models_fetches_upstream_with_runtime_client_version() {
        let _guard = CODEX_UPSTREAM_ENV_LOCK
            .lock()
            .expect("codex upstream env lock");
        let captured = Arc::new(CapturedCodexUpstream::default());
        let app = Router::new()
            .route("/v1/models", get(fake_codex_models))
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

        let config = AdminRuntimeConfig {
            codex_client_version: "0.125.0".to_string(),
            ..AdminRuntimeConfig::default()
        };
        let state = super::ProviderState::new_with_config_store(
            Arc::new(TestStore),
            static_codex_route_store(),
            Arc::new(StaticAdminConfigStore {
                config,
            }),
        );
        let response = super::provider_entry(
            state,
            Request::builder()
                .method("GET")
                .uri("/api/llm-gateway/v1/models")
                .header(header::AUTHORIZATION, "Bearer codex-secret")
                .body(Body::empty())
                .expect("request"),
        )
        .await;

        std::env::remove_var("CODEX_UPSTREAM_BASE_URL");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::ETAG)
                .and_then(|value| value.to_str().ok()),
            Some(r#""models-test""#)
        );
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body");
        let body = serde_json::from_slice::<serde_json::Value>(&body).expect("json response");
        assert_eq!(body["object"], "list");
        let ids = body["data"]
            .as_array()
            .expect("model data")
            .iter()
            .filter_map(|item| item.get("id").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>();
        assert!(ids.contains(&"gpt-5.3-codex"));
        assert!(ids.contains(&"gpt-5.5"));

        let requests = captured.requests.lock().expect("captured requests");
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].path, "/v1/models");
        assert_eq!(requests[0].query.as_deref(), Some("client_version=0.125.0"));
        assert_eq!(requests[0].authorization.as_deref(), Some("Bearer upstream-token"));
        assert_eq!(requests[0].accept.as_deref(), Some("application/json"));
        assert_eq!(requests[0].user_agent.as_deref(), Some("codex_cli_rs/0.125.0"));
    }

    #[tokio::test]
    async fn codex_dispatch_streams_chat_completion_chunks_from_responses_sse() {
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
        let _guard = crate::KIRO_UPSTREAM_ENV_LOCK
            .lock()
            .expect("kiro upstream env lock");
        let captured = Arc::new(CapturedKiroUpstream::default());
        let upstream_base = spawn_fake_kiro_upstream(captured.clone()).await;
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
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body");
        let body = serde_json::from_slice::<serde_json::Value>(&body).expect("json response");
        assert_eq!(body["type"], "message");
        assert_eq!(body["content"][0]["type"], "text");
        assert_eq!(body["content"][0]["text"], "hello back");
        assert_eq!(body["usage"]["input_tokens"], 1);
        assert_eq!(body["usage"]["cache_creation_input_tokens"], 0);
        assert_eq!(body["usage"]["output_tokens"], 3);

        let requests = captured.requests.lock().expect("captured requests");
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].path, "/generateAssistantResponse");
        assert_eq!(requests[0].authorization.as_deref(), Some("Bearer kiro-upstream-token"));
        assert_eq!(requests[0].body["profileArn"], "arn:aws:kiro:test");
    }

    #[tokio::test]
    async fn kiro_dispatch_does_not_refresh_missing_status_on_request_path() {
        let _guard = crate::KIRO_UPSTREAM_ENV_LOCK
            .lock()
            .expect("kiro upstream env lock");
        let captured = Arc::new(CapturedKiroUpstream::default());
        let upstream_base = spawn_fake_kiro_upstream(captured.clone()).await;
        std::env::set_var("KIRO_UPSTREAM_BASE_URL", upstream_base);

        let mut route = static_kiro_route();
        route.auth_json = format!(
            r#"{{
                "accessToken":"kiro-upstream-token",
                "machineId":"{}",
                "apiRegion":"us-east-1"
            }}"#,
            "a".repeat(64)
        );
        route.cached_status = None;
        route.cached_remaining_credits = None;
        route.cached_balance = None;
        route.cached_cache = None;
        let route_store = Arc::new(CapturingKiroStatusRouteStore {
            route: Arc::new(Mutex::new(route)),
            updates: Arc::new(Mutex::new(Vec::new())),
        });
        let state = super::ProviderState::new(Arc::new(TestStore), route_store.clone());
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
        let updates = route_store.updates.lock().expect("updates");
        assert!(updates.is_empty());
        let requests = captured.requests.lock().expect("captured requests");
        let paths = requests
            .iter()
            .map(|request| request.path.as_str())
            .collect::<Vec<_>>();
        assert_eq!(paths, vec!["/generateAssistantResponse"]);
    }

    #[tokio::test]
    async fn kiro_dispatch_applies_key_model_mapping_before_upstream_conversion() {
        let _guard = crate::KIRO_UPSTREAM_ENV_LOCK
            .lock()
            .expect("kiro upstream env lock");
        let captured = Arc::new(CapturedKiroUpstream::default());
        let upstream_base = spawn_fake_kiro_upstream(captured.clone()).await;
        std::env::set_var("KIRO_UPSTREAM_BASE_URL", upstream_base);

        let mut route = static_kiro_route();
        route.model_name_map_json =
            r#"{"claude-haiku-4-5-20251001":"claude-sonnet-4-6"}"#.to_string();
        let state = super::ProviderState::new(
            Arc::new(TestStore),
            Arc::new(StaticRouteStore {
                codex_route: ProviderCodexRoute {
                    account_name: "codex-a".to_string(),
                    account_group_id_at_event: None,
                    route_strategy_at_event: RouteStrategy::Auto,
                    auth_json: r#"{"access_token":"upstream-token"}"#.to_string(),
                    map_gpt53_codex_to_spark: true,
                    request_max_concurrency: None,
                    request_min_start_interval_ms: None,
                    account_request_max_concurrency: None,
                    account_request_min_start_interval_ms: None,
                    proxy: None,
                },
                kiro_route: route,
            }),
        );
        let response = super::provider_entry(
            state,
            Request::builder()
                .method("POST")
                .uri("/api/kiro-gateway/v1/messages")
                .header("x-api-key", "valid-secret")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{
                        "model": "claude-haiku-4-5-20251001",
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
        let requests = captured.requests.lock().expect("captured requests");
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].body["conversationState"]["currentMessage"]["userInputMessage"]["modelId"],
            "claude-sonnet-4.6"
        );
    }

    #[tokio::test]
    async fn kiro_dispatch_rejects_history_images_without_stable_session_before_upstream() {
        let _guard = crate::KIRO_UPSTREAM_ENV_LOCK
            .lock()
            .expect("kiro upstream env lock");
        let captured = Arc::new(CapturedKiroUpstream::default());
        let upstream_base = spawn_fake_kiro_upstream(captured.clone()).await;
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
                        "messages": [
                            {
                                "role": "user",
                                "content": [
                                    {
                                        "type": "image",
                                        "source": {
                                            "type": "base64",
                                            "media_type": "image/png",
                                            "data": "aGVsbG8="
                                        }
                                    },
                                    {"type": "text", "text": "old image"}
                                ]
                            },
                            {"role": "assistant", "content": "ok"},
                            {"role": "user", "content": "continue"}
                        ],
                        "stream": false
                    }"#,
                ))
                .expect("request"),
        )
        .await;

        std::env::remove_var("KIRO_UPSTREAM_BASE_URL");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body");
        let body = String::from_utf8(body.to_vec()).expect("utf8 response");
        assert!(body.contains("Historical image turns require a stable session id"));
        assert!(captured
            .requests
            .lock()
            .expect("captured requests")
            .is_empty());
        let events = store.usage_events.lock().expect("usage events");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].status_code, 400);
        assert_eq!(events[0].endpoint, "/v1/messages");
        assert_eq!(events[0].request_url, "/api/kiro-gateway/v1/messages");
        assert!(events[0].client_request_body_json.is_some());
    }

    #[tokio::test]
    async fn kiro_dispatch_rejects_oversized_upstream_generate_body_before_upstream() {
        let _guard = crate::KIRO_UPSTREAM_ENV_LOCK
            .lock()
            .expect("kiro upstream env lock");
        let captured = Arc::new(CapturedKiroUpstream::default());
        let upstream_base = spawn_fake_kiro_upstream(captured.clone()).await;
        std::env::set_var("KIRO_UPSTREAM_BASE_URL", upstream_base);

        let oversized_text = "a".repeat(super::KIRO_GENERATE_REQUEST_MAX_BODY_BYTES + 1);
        let body = serde_json::json!({
            "model": "claude-sonnet-4-6",
            "max_tokens": 128,
            "messages": [{"role": "user", "content": oversized_text}],
            "stream": false
        });
        let state = super::ProviderState::new(Arc::new(TestStore), static_kiro_route_store());
        let response = super::provider_entry(
            state,
            Request::builder()
                .method("POST")
                .uri("/api/kiro-gateway/v1/messages")
                .header("x-api-key", "valid-secret")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await;

        std::env::remove_var("KIRO_UPSTREAM_BASE_URL");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body");
        let body = String::from_utf8(body.to_vec()).expect("utf8 response");
        assert!(body.contains("Context window is full"));
        assert!(captured
            .requests
            .lock()
            .expect("captured requests")
            .is_empty());
    }

    #[tokio::test]
    async fn kiro_dispatch_streams_messages_from_eventstream() {
        let _guard = crate::KIRO_UPSTREAM_ENV_LOCK
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
        let _guard = crate::KIRO_UPSTREAM_ENV_LOCK
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
        assert!(body.contains(r#""input_tokens":1"#));
        assert!(body.contains("hello "));
        assert!(body.contains("back"));
    }

    #[tokio::test]
    async fn kiro_dispatch_records_usage_rollup_from_eventstream() {
        let _guard = crate::KIRO_UPSTREAM_ENV_LOCK
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
        assert_eq!(event.input_uncached_tokens, 1);
        assert_eq!(event.input_cached_tokens, 0);
        assert_eq!(event.output_tokens, 3);
        assert_eq!(event.billable_tokens, 16);
        assert_eq!(event.credit_usage.as_deref(), Some("0.25"));
        assert!(!event.credit_usage_missing);
        assert_eq!(event.request_method, "POST");
        assert_eq!(event.request_url, "/api/kiro-gateway/v1/messages");
        assert!(event.request_body_bytes.unwrap_or_default() > 0);
        assert!(event.timing.request_body_read_ms.is_some());
        assert!(event.timing.request_json_parse_ms.is_some());
        assert!(event.timing.pre_handler_ms.is_some());
        assert!(event.timing.routing_wait_ms.is_some());
        assert!(event.timing.upstream_headers_ms.is_some());
        assert!(event.timing.post_headers_body_ms.is_some());
        assert!(event.timing.stream_finish_ms.is_some());
        assert_eq!(event.last_message_content.as_deref(), Some("hello"));
    }

    #[test]
    fn kiro_billable_tokens_discounts_cached_input_like_legacy_gateway() {
        let usage = super::KiroUsageSummary {
            input_uncached_tokens: 100,
            input_cached_tokens: 1_000,
            output_tokens: 4,
            credit_usage: None,
            credit_usage_missing: false,
        };
        let multipliers = BTreeMap::from([("sonnet".to_string(), 2.0)]);

        let billable =
            super::kiro_billable_tokens_with_multipliers("claude-sonnet-4-6", usage, &multipliers);

        assert_eq!(billable, (100 + 1_000 / 10 + 4 * 5) * 2);
    }

    #[tokio::test]
    async fn kiro_websearch_usage_omits_heavy_payload_on_success() {
        let store = RecordingControlStore::default();
        let key = AuthenticatedKey {
            key_id: "kiro-key".to_string(),
            key_name: "Kiro key".to_string(),
            provider_type: "kiro".to_string(),
            protocol_family: "anthropic".to_string(),
            status: "active".to_string(),
            quota_billable_limit: 1_000,
            billable_tokens_used: 0,
        };
        let meta = super::ProviderUsageMetadata {
            started_at: Instant::now(),
            request_method: "POST".to_string(),
            request_url: "/api/kiro-gateway/v1/messages".to_string(),
            request_body_bytes: Some(128),
            request_body_read_ms: None,
            request_json_parse_ms: None,
            pre_handler_ms: None,
            routing_wait_ms: None,
            upstream_headers_ms: None,
            post_headers_body_ms: None,
            first_sse_write_ms: None,
            stream_finish_ms: None,
            quota_failover_count: 0,
            routing_diagnostics_json: None,
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: "{}".to_string(),
            last_message_content: Some("search query".to_string()),
            client_request_body_json: Some(r#"{"client":true}"#.to_string()),
            upstream_request_body_json: Some(r#"{"mcp":true}"#.to_string()),
            full_request_json: Some(r#"{"full":true}"#.to_string()),
        };

        let route = static_kiro_route();
        super::record_kiro_websearch_usage(super::KiroWebsearchUsageRecord {
            control_store: &store,
            key: &key,
            route: &route,
            model: "claude-sonnet-4-6",
            status: StatusCode::OK,
            usage: super::KiroUsageSummary {
                input_uncached_tokens: 10,
                input_cached_tokens: 0,
                output_tokens: 3,
                credit_usage: None,
                credit_usage_missing: true,
            },
            meta: &meta,
            capture_request_details: false,
        })
        .await
        .expect("record websearch usage");

        let events = store.usage_events.lock().expect("usage events");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].endpoint, "/mcp");
        assert_eq!(events[0].last_message_content.as_deref(), Some("search query"));
        assert_eq!(events[0].client_request_body_json, None);
        assert_eq!(events[0].upstream_request_body_json, None);
        assert_eq!(events[0].full_request_json, None);
    }

    #[tokio::test]
    async fn kiro_usage_captures_full_payload_when_key_full_request_logging_enabled() {
        let store = RecordingControlStore::default();
        let key = AuthenticatedKey {
            key_id: "kiro-key".to_string(),
            key_name: "Kiro key".to_string(),
            provider_type: "kiro".to_string(),
            protocol_family: "anthropic".to_string(),
            status: "active".to_string(),
            quota_billable_limit: 1_000,
            billable_tokens_used: 0,
        };
        let mut route = static_kiro_route();
        route.full_request_logging_enabled = true;
        let conversation_state =
            llm_access_kiro::wire::ConversationState::new("diag-conversation".to_string());
        let cache_simulator = llm_access_kiro::cache_sim::KiroCacheSimulator::default();
        let cache_ctx =
            super::build_kiro_cache_context(&route, &conversation_state, &cache_simulator)
                .expect("cache context");
        let meta = super::ProviderUsageMetadata {
            started_at: Instant::now(),
            request_method: "POST".to_string(),
            request_url: "/api/kiro-gateway/v1/messages".to_string(),
            request_body_bytes: Some(128),
            request_body_read_ms: None,
            request_json_parse_ms: None,
            pre_handler_ms: None,
            routing_wait_ms: None,
            upstream_headers_ms: None,
            post_headers_body_ms: None,
            first_sse_write_ms: None,
            stream_finish_ms: None,
            quota_failover_count: 0,
            routing_diagnostics_json: None,
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: "{}".to_string(),
            last_message_content: Some("normal cached request".to_string()),
            client_request_body_json: Some(r#"{"client":true}"#.to_string()),
            upstream_request_body_json: Some(r#"{"upstream":true}"#.to_string()),
            full_request_json: Some(r#"{"full":true}"#.to_string()),
        };

        super::record_kiro_usage(super::KiroUsageRecord {
            control_store: &store,
            key: &key,
            route: &route,
            endpoint: "/v1/messages",
            model: "claude-sonnet-4-6",
            status: StatusCode::OK,
            usage: super::KiroUsageSummary {
                input_uncached_tokens: 10,
                input_cached_tokens: 200,
                output_tokens: 3,
                credit_usage: None,
                credit_usage_missing: true,
            },
            cache_ctx: &cache_ctx,
            meta: &meta,
        })
        .await
        .expect("record kiro usage");

        let events = store.usage_events.lock().expect("usage events");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].client_request_body_json.as_deref(), Some(r#"{"client":true}"#));
        assert_eq!(events[0].upstream_request_body_json.as_deref(), Some(r#"{"upstream":true}"#));
        assert_eq!(events[0].full_request_json.as_deref(), Some(r#"{"full":true}"#));
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
    async fn provider_entry_requires_codex_route_for_models_after_auth() {
        let state = test_state();
        let request = Request::builder()
            .method("GET")
            .uri("/api/llm-gateway/v1/models")
            .header(header::AUTHORIZATION, "Bearer codex-secret")
            .body(Body::empty())
            .expect("request");

        let response = super::provider_entry(state, request).await;

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
