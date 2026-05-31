//! Provider-facing HTTP entrypoints for `llm-access`.

mod kiro_error;
mod kiro_session_affinity;

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    num::NonZeroUsize,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{bail, Context};
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
    anthropic_messages::{
        convert_json_response_to_anthropic_message, convert_response_event_to_anthropic_sse_chunks,
        AnthropicStreamMetadata,
    },
    request::{
        align_responses_store_with_upstream, apply_codex_fast_policy,
        apply_gpt53_codex_spark_mapping, external_origin, extract_client_ip_from_headers,
        extract_last_message_content as extract_codex_last_message_content,
        prepare_gateway_request_from_bytes, resolve_request_url_from_headers,
        serialize_headers_json,
    },
    response::{
        adapt_completed_response_json, apply_upstream_response_headers,
        convert_json_response_to_chat_completion, convert_response_event_to_chat_chunk,
        encode_json_sse_chunk, encode_sse_event_with_model_alias, extract_usage_from_bytes,
        rewrite_json_response_model_alias, rewrite_json_value_model_alias, SseUsageCollector,
    },
    types::{ChatStreamMetadata, GatewayResponseAdapter, PreparedGatewayRequest, UsageBreakdown},
};
use llm_access_core::{
    provider::{ProtocolFamily, ProviderType},
    routes::provider_route_requirement,
    store::{
        compute_kiro_billable_tokens, is_terminal_codex_auth_error, AdminConfigStore,
        AdminKiroStatusCacheUpdate, AuthenticatedKey, ControlStore, EmptyAdminConfigStore,
        ProviderCodexAuthUpdate, ProviderCodexRoute, ProviderKiroAuthUpdate, ProviderKiroRoute,
        ProviderProxyConfig, ProviderRouteStore,
    },
    usage::{UsageEvent, UsageStreamDetails, UsageTiming},
};
use llm_access_kiro::{
    anthropic::{
        converter::{
            convert_normalized_request_with_resolved_session, current_user_message_range,
            extract_tool_result_content, get_context_window_size, normalize_request,
            preview_session_value, resolve_conversation_id_from_metadata, ResolvedConversationId,
            ResponseModelIdentity, SessionFallbackReason, SessionIdSource, SessionTracking,
        },
        stream::{anthropic_usage_json, resolve_input_tokens_with_threshold, StreamContext},
        supported_models_response,
        types::{MessagesRequest, OutputConfig, Thinking},
        websearch::{self, McpResponse},
    },
    auth_file::KiroAuthRecord,
    cache_policy::{
        adjust_input_tokens_for_cache_creation_cost_with_policy, default_kiro_cache_policy,
        prefix_tree_credit_ratio_cap_basis_points_with_policy, validate_kiro_cache_policy,
        KiroCachePolicy,
    },
    cache_sim::{
        KiroCacheRuntimeStats, KiroCacheSimulationConfig, KiroCacheSimulationMode,
        KiroCacheSimulator, RuntimePromptProjection,
    },
    parser::decoder::EventStreamDecoder,
    scheduler::{KiroRequestLease, KiroRequestScheduler},
    token,
    wire::{ConversationState, Event, KiroRequest},
};
use lru::LruCache;
use rand::Rng;
use serde_json::{json, Value};

use self::{
    kiro_error::{
        kiro_conversion_error_response, kiro_json_error, kiro_upstream_error_response,
        KiroRouteFailure, KiroRouteFailureKind,
    },
    kiro_session_affinity::KiroSessionAffinity,
};
use crate::{
    activity::RequestActivityTracker, codex_refresh, geoip::GeoIpResolver, kiro_headers,
    kiro_latency::KiroLatencyRanker, kiro_refresh,
};

const MAX_PROVIDER_PROXY_BODY_BYTES: usize = 32 * 1024 * 1024;
const DEFAULT_WIRE_ORIGINATOR: &str = "codex_cli_rs";
const MAX_CODEX_CLIENT_VERSION_LEN: usize = 64;
const KIRO_PROVIDER_AWS_SDK_VERSION: &str = "1.0.34";
const KIRO_REMOTE_IMAGE_MAX_BYTES: usize = 1_000_000;
const KIRO_REMOTE_DOCUMENT_MAX_BYTES: usize = 8 * 1024 * 1024;
const KIRO_REMOTE_MEDIA_TIMEOUT: Duration = Duration::from_secs(15);
const KIRO_LAST_MESSAGE_PART_PREVIEW_CHARS: usize = 320;
const KIRO_LAST_MESSAGE_TOTAL_PREVIEW_CHARS: usize = 1_024;
const CODEX_QUOTA_EXHAUSTION_COOLDOWN: Duration = Duration::from_secs(5 * 60);
const DEFAULT_PROVIDER_CLIENT_CACHE_CAPACITY: usize = 50;
const MAX_PROVIDER_CLIENT_CACHE_CAPACITY: usize = 128;
const DEFAULT_PROVIDER_CLIENT_POOL_IDLE_TIMEOUT_SECONDS: u64 = 600;
const MIN_PROVIDER_CLIENT_POOL_IDLE_TIMEOUT_SECONDS: u64 = 30;
const MAX_PROVIDER_CLIENT_POOL_IDLE_TIMEOUT_SECONDS: u64 = 3600;
const DEFAULT_PROVIDER_CLIENT_POOL_MAX_IDLE_PER_HOST: usize = 4;
const MAX_PROVIDER_CLIENT_POOL_MAX_IDLE_PER_HOST: usize = 16;
const CODEX_TRANSIENT_ACCOUNT_FAILURE_COOLDOWN_MIN: Duration = Duration::from_secs(45);
const CODEX_TRANSIENT_ACCOUNT_FAILURE_COOLDOWN_MAX: Duration = Duration::from_secs(90);

#[derive(Debug, Clone)]
struct CodexDispatchRuntimeConfig {
    client_version: String,
    account_attempt_limit: usize,
}

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
    stream_completed_cleanly: Option<bool>,
    downstream_disconnect: Option<bool>,
    final_event_type: Option<String>,
    bytes_streamed: Option<i64>,
    quota_failover_count: u64,
    routing_diagnostics_json: Option<String>,
    client_ip: String,
    ip_region: String,
    request_headers_json: String,
    last_message_content: Option<String>,
    client_request_body_json: Option<Bytes>,
    upstream_request_body_json: Option<Bytes>,
    full_request_json: Option<Bytes>,
    error_message: Option<String>,
    error_body: Option<String>,
}

impl ProviderUsageMetadata {
    async fn from_request_parts(
        method: &Method,
        uri: &axum::http::Uri,
        headers: &HeaderMap,
        geoip: &GeoIpResolver,
    ) -> Self {
        let client_ip = extract_client_ip_from_headers(headers);
        let ip_region = geoip.resolve_region(&client_ip).await;
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
            stream_completed_cleanly: None,
            downstream_disconnect: None,
            final_event_type: None,
            bytes_streamed: None,
            quota_failover_count: 0,
            routing_diagnostics_json: None,
            client_ip,
            ip_region,
            request_headers_json: serialize_headers_json(headers),
            last_message_content: None,
            client_request_body_json: None,
            upstream_request_body_json: None,
            full_request_json: None,
            error_message: None,
            error_body: None,
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

    fn observe_stream_write(&mut self, bytes_len: usize, event_type: Option<&str>) {
        self.mark_first_sse_write();
        self.stream_completed_cleanly.get_or_insert(false);
        self.downstream_disconnect.get_or_insert(false);
        self.bytes_streamed = Some(
            self.bytes_streamed
                .unwrap_or_default()
                .saturating_add(clamp_usize_to_i64(bytes_len)),
        );
        if let Some(event_type) = event_type.map(str::trim).filter(|value| !value.is_empty()) {
            self.final_event_type = Some(event_type.to_string());
        }
    }

    fn mark_stream_finish(&mut self) {
        self.stream_finish_ms = Some(self.elapsed_ms());
    }

    fn mark_stream_completed_cleanly(&mut self) {
        self.stream_completed_cleanly = Some(true);
        self.downstream_disconnect = Some(false);
        self.mark_stream_finish();
    }

    fn mark_stream_internal_incomplete(&mut self) {
        self.stream_completed_cleanly = Some(false);
        self.downstream_disconnect = Some(false);
        self.mark_stream_finish();
    }

    fn mark_downstream_disconnect(&mut self) {
        self.stream_completed_cleanly = Some(false);
        self.downstream_disconnect = Some(true);
        self.mark_stream_finish();
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

    fn to_stream_details(&self) -> UsageStreamDetails {
        UsageStreamDetails {
            stream_completed_cleanly: self.stream_completed_cleanly,
            downstream_disconnect: self.downstream_disconnect,
            final_event_type: self.final_event_type.clone(),
            bytes_streamed: self.bytes_streamed,
        }
    }
}

fn capture_client_request_body_json(meta: &mut ProviderUsageMetadata, body: &[u8]) {
    if meta.client_request_body_json.is_none() {
        meta.client_request_body_json = Some(Bytes::copy_from_slice(body));
    }
}

fn capture_upstream_request_body_json(meta: &mut ProviderUsageMetadata, body: &[u8]) {
    if meta.upstream_request_body_json.is_none() {
        meta.upstream_request_body_json = Some(Bytes::copy_from_slice(body));
    }
}

fn capture_error_message(meta: &mut ProviderUsageMetadata, message: &str) {
    if meta.error_message.is_some() {
        return;
    }
    let trimmed = message.trim();
    if !trimmed.is_empty() {
        meta.error_message = Some(trimmed.to_string());
    }
}

fn capture_error_body(meta: &mut ProviderUsageMetadata, body: &str) {
    if meta.error_body.is_some() {
        return;
    }
    let trimmed = body.trim();
    if !trimmed.is_empty() {
        meta.error_body = Some(trimmed.to_string());
    }
}

fn capture_error_bytes(meta: &mut ProviderUsageMetadata, bytes: &Bytes) {
    capture_error_message(meta, &summarize_error_bytes(bytes));
    let body = String::from_utf8_lossy(bytes.as_ref());
    capture_error_body(meta, &body);
}

fn capture_codex_dispatch_request_json(
    meta: &mut ProviderUsageMetadata,
    client_body: &Bytes,
    prepared: &PreparedGatewayRequest,
) {
    if meta.client_request_body_json.is_none() {
        meta.client_request_body_json = Some(client_body.clone());
    }
    meta.upstream_request_body_json = Some(prepared.request_body.clone());
}

fn capture_codex_prepared_request_json(
    meta: &mut ProviderUsageMetadata,
    prepared: &PreparedGatewayRequest,
) {
    if meta.client_request_body_json.is_none() {
        meta.client_request_body_json = Some(prepared.client_request_body_or_upstream().clone());
    }
    if meta.upstream_request_body_json.is_none() {
        meta.upstream_request_body_json = Some(prepared.request_body.clone());
    }
}

fn strip_codex_stream_request_bodies(
    mut prepared: PreparedGatewayRequest,
) -> PreparedGatewayRequest {
    prepared.client_request_body = None;
    prepared.request_body = Bytes::new();
    prepared
}

fn captured_body_json(body: &Option<Bytes>) -> Option<String> {
    body.as_ref()
        .map(|bytes| String::from_utf8_lossy(bytes.as_ref()).into_owned())
}

fn extract_model_from_json_body(body: &Bytes) -> Option<String> {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| value.get("model").cloned())
        .and_then(|value| value.as_str().map(str::trim).map(str::to_string))
        .filter(|value| !value.is_empty())
}

/// Shared provider request state.
#[derive(Clone)]
pub struct ProviderState {
    control_store: Arc<dyn ControlStore>,
    route_store: Arc<dyn ProviderRouteStore>,
    geoip: GeoIpResolver,
    admin_config_store: Arc<dyn AdminConfigStore>,
    dispatcher: Arc<dyn ProviderDispatcher>,
    kiro_cache_simulator: Arc<KiroCacheSimulator>,
    request_limiter: Arc<RequestLimiter>,
    codex_account_cooldowns: Arc<CodexAccountCooldowns>,
    kiro_request_scheduler: Arc<KiroRequestScheduler>,
    kiro_session_affinity: Arc<KiroSessionAffinity>,
    kiro_latency_ranker: Arc<KiroLatencyRanker>,
    request_activity: Arc<RequestActivityTracker>,
}

/// Runtime dependencies passed from the authenticated provider entrypoint into
/// the provider dispatcher.
#[derive(Clone)]
pub struct ProviderDispatchDeps {
    route_store: Arc<dyn ProviderRouteStore>,
    control_store: Arc<dyn ControlStore>,
    geoip: GeoIpResolver,
    admin_config_store: Arc<dyn AdminConfigStore>,
    kiro_cache_simulator: Arc<KiroCacheSimulator>,
    request_limiter: Arc<RequestLimiter>,
    codex_account_cooldowns: Arc<CodexAccountCooldowns>,
    kiro_request_scheduler: Arc<KiroRequestScheduler>,
    kiro_session_affinity: Arc<KiroSessionAffinity>,
    kiro_latency_ranker: Arc<KiroLatencyRanker>,
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
            GeoIpResolver::disabled(),
        )
    }

    pub(crate) fn new_with_config_store_and_activity(
        control_store: Arc<dyn ControlStore>,
        route_store: Arc<dyn ProviderRouteStore>,
        admin_config_store: Arc<dyn AdminConfigStore>,
        request_activity: Arc<RequestActivityTracker>,
        geoip: GeoIpResolver,
    ) -> Self {
        Self::new_with_config_store_activity_and_latency(
            control_store,
            route_store,
            admin_config_store,
            request_activity,
            geoip,
            Arc::new(KiroLatencyRanker::default()),
        )
    }

    pub(crate) fn new_with_config_store_activity_and_latency(
        control_store: Arc<dyn ControlStore>,
        route_store: Arc<dyn ProviderRouteStore>,
        admin_config_store: Arc<dyn AdminConfigStore>,
        request_activity: Arc<RequestActivityTracker>,
        geoip: GeoIpResolver,
        kiro_latency_ranker: Arc<KiroLatencyRanker>,
    ) -> Self {
        Self::with_dispatcher_and_config_store(
            control_store,
            route_store,
            admin_config_store,
            Arc::new(DefaultProviderDispatcher),
            request_activity,
            geoip,
            kiro_latency_ranker,
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
            GeoIpResolver::disabled(),
            Arc::new(KiroLatencyRanker::default()),
        )
    }

    fn with_dispatcher_and_config_store(
        control_store: Arc<dyn ControlStore>,
        route_store: Arc<dyn ProviderRouteStore>,
        admin_config_store: Arc<dyn AdminConfigStore>,
        dispatcher: Arc<dyn ProviderDispatcher>,
        request_activity: Arc<RequestActivityTracker>,
        geoip: GeoIpResolver,
        kiro_latency_ranker: Arc<KiroLatencyRanker>,
    ) -> Self {
        Self {
            control_store,
            route_store,
            geoip,
            admin_config_store,
            dispatcher,
            kiro_cache_simulator: Arc::new(KiroCacheSimulator::default()),
            request_limiter: Arc::new(RequestLimiter::default()),
            codex_account_cooldowns: Arc::new(CodexAccountCooldowns::default()),
            kiro_request_scheduler: KiroRequestScheduler::new(),
            kiro_session_affinity: Arc::new(KiroSessionAffinity::from_env()),
            kiro_latency_ranker,
            request_activity,
        }
    }

    pub(crate) fn route_store(&self) -> Arc<dyn ProviderRouteStore> {
        Arc::clone(&self.route_store)
    }

    pub(crate) async fn authenticate_bearer_secret(
        &self,
        secret: &str,
    ) -> anyhow::Result<Option<AuthenticatedKey>> {
        self.control_store.authenticate_bearer_secret(secret).await
    }

    pub(crate) async fn dispatch_admin_probe_with_proxy(
        &self,
        key: AuthenticatedKey,
        request: Request<Body>,
        proxy: ProviderProxyConfig,
    ) -> Response {
        if !is_active_key(&key) {
            return (StatusCode::FORBIDDEN, "llm key is not active").into_response();
        }
        if !key_matches_route(&key, request.uri().path()) {
            return (StatusCode::FORBIDDEN, "llm key does not match provider route")
                .into_response();
        }
        if is_quota_exhausted(&key) {
            return quota_exhausted_response(&key);
        }

        let mut deps = self.dispatch_deps();
        deps.route_store = Arc::new(ForcedProxyRouteStore {
            inner: Arc::clone(&self.route_store),
            proxy,
        });
        let _activity_guard = self.request_activity.start(&key.key_id);
        self.dispatcher.dispatch(key, request, deps).await
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
            geoip: self.geoip.clone(),
            admin_config_store: Arc::clone(&self.admin_config_store),
            kiro_cache_simulator: Arc::clone(&self.kiro_cache_simulator),
            request_limiter: Arc::clone(&self.request_limiter),
            codex_account_cooldowns: Arc::clone(&self.codex_account_cooldowns),
            kiro_request_scheduler: Arc::clone(&self.kiro_request_scheduler),
            kiro_session_affinity: Arc::clone(&self.kiro_session_affinity),
            kiro_latency_ranker: Arc::clone(&self.kiro_latency_ranker),
        }
    }
}

struct ForcedProxyRouteStore {
    inner: Arc<dyn ProviderRouteStore>,
    proxy: ProviderProxyConfig,
}

impl ForcedProxyRouteStore {
    fn force_codex_proxy(&self, mut route: ProviderCodexRoute) -> ProviderCodexRoute {
        route.proxy = Some(self.proxy.clone());
        route
    }

    fn force_kiro_proxy(&self, mut route: ProviderKiroRoute) -> ProviderKiroRoute {
        route.proxy = Some(self.proxy.clone());
        route
    }
}

#[async_trait]
impl ProviderRouteStore for ForcedProxyRouteStore {
    async fn resolve_codex_route(
        &self,
        key: &AuthenticatedKey,
    ) -> anyhow::Result<Option<ProviderCodexRoute>> {
        Ok(self
            .inner
            .resolve_codex_route(key)
            .await?
            .map(|route| self.force_codex_proxy(route)))
    }

    async fn resolve_codex_route_candidates(
        &self,
        key: &AuthenticatedKey,
    ) -> anyhow::Result<Vec<ProviderCodexRoute>> {
        Ok(self
            .inner
            .resolve_codex_route_candidates(key)
            .await?
            .into_iter()
            .map(|route| self.force_codex_proxy(route))
            .collect())
    }

    async fn resolve_codex_account_route(
        &self,
        account_name: &str,
    ) -> anyhow::Result<Option<ProviderCodexRoute>> {
        Ok(self
            .inner
            .resolve_codex_account_route(account_name)
            .await?
            .map(|route| self.force_codex_proxy(route)))
    }

    async fn resolve_kiro_route(
        &self,
        key: &AuthenticatedKey,
    ) -> anyhow::Result<Option<ProviderKiroRoute>> {
        Ok(self
            .inner
            .resolve_kiro_route(key)
            .await?
            .map(|route| self.force_kiro_proxy(route)))
    }

    async fn resolve_kiro_route_candidates(
        &self,
        key: &AuthenticatedKey,
    ) -> anyhow::Result<Vec<ProviderKiroRoute>> {
        Ok(self
            .inner
            .resolve_kiro_route_candidates(key)
            .await?
            .into_iter()
            .map(|route| self.force_kiro_proxy(route))
            .collect())
    }

    async fn resolve_kiro_account_route(
        &self,
        account_name: &str,
    ) -> anyhow::Result<Option<ProviderKiroRoute>> {
        Ok(self
            .inner
            .resolve_kiro_account_route(account_name)
            .await?
            .map(|route| self.force_kiro_proxy(route)))
    }

    async fn save_kiro_auth_update(&self, update: ProviderKiroAuthUpdate) -> anyhow::Result<()> {
        self.inner.save_kiro_auth_update(update).await
    }

    async fn save_codex_auth_update(&self, update: ProviderCodexAuthUpdate) -> anyhow::Result<()> {
        self.inner.save_codex_auth_update(update).await
    }

    async fn set_codex_account_auto_refresh_enabled(
        &self,
        account_name: &str,
        enabled: bool,
        updated_at_ms: i64,
    ) -> anyhow::Result<()> {
        self.inner
            .set_codex_account_auto_refresh_enabled(account_name, enabled, updated_at_ms)
            .await
    }

    async fn mark_kiro_account_quota_exhausted(
        &self,
        account_name: &str,
        error_message: &str,
        checked_at_ms: i64,
    ) -> anyhow::Result<()> {
        self.inner
            .mark_kiro_account_quota_exhausted(account_name, error_message, checked_at_ms)
            .await
    }

    async fn save_kiro_status_cache_update(
        &self,
        update: AdminKiroStatusCacheUpdate,
    ) -> anyhow::Result<()> {
        self.inner.save_kiro_status_cache_update(update).await
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

#[derive(Default)]
struct CodexAccountCooldowns {
    blocked_until: Mutex<HashMap<String, Instant>>,
}

#[derive(Debug, Clone, Copy)]
struct ActiveCooldown {
    remaining: Duration,
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

impl CodexAccountCooldowns {
    /// Return the remaining request-path cooldown for one Codex account.
    ///
    /// This state is intentionally local and ephemeral:
    /// - it is only used to keep request routing from hammering an account that
    ///   just failed in the request path;
    /// - it does not participate in background refresh or token refresh;
    /// - it lazily expires on read so we do not need a separate cleanup task.
    fn cooldown_for_account(&self, account_name: &str) -> Option<ActiveCooldown> {
        let Ok(mut blocked_until) = self.blocked_until.lock() else {
            return None;
        };
        let blocked_until_at = blocked_until.get(account_name).copied()?;
        let remaining = blocked_until_at.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            blocked_until.remove(account_name);
            return None;
        }
        Some(ActiveCooldown {
            remaining,
        })
    }

    /// Mark one Codex account as temporarily unavailable for request routing.
    ///
    /// The write semantics are deliberately "single-flight-like": once one
    /// request has already established a cooldown window, concurrent failures
    /// do not shorten it by overwriting with a smaller randomly sampled
    /// TTL. A new write only takes effect when it extends the blocked-until
    /// instant.
    fn mark_account_cooldown(&self, account_name: &str, cooldown: Duration) {
        if cooldown.is_zero() {
            return;
        }
        let Ok(mut blocked_until) = self.blocked_until.lock() else {
            return;
        };
        let next_until = Instant::now() + cooldown;
        match blocked_until.get_mut(account_name) {
            Some(existing_until) if *existing_until >= next_until => {},
            Some(existing_until) => *existing_until = next_until,
            None => {
                blocked_until.insert(account_name.to_string(), next_until);
            },
        }
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
    codex_account_cooldowns: &Arc<CodexAccountCooldowns>,
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
        let mut saw_account_cooldown = false;
        let mut saw_terminal_auth_error = false;
        let mut shortest_wait: Option<LimitRejection> = None;
        for route in routes {
            if failed_accounts.contains(&route.account_name) {
                continue;
            }
            if let Some(error) = route
                .cached_error_message
                .as_deref()
                .filter(|message| is_terminal_codex_auth_error(message))
            {
                saw_terminal_auth_error = true;
                tracing::warn!(
                    account = %route.account_name,
                    error,
                    "skipping codex account with terminal auth error"
                );
                continue;
            }
            if let Some(cooldown) =
                codex_account_cooldowns.cooldown_for_account(&route.account_name)
            {
                saw_account_cooldown = true;
                tracing::debug!(
                    account = %route.account_name,
                    cooldown_remaining_ms = cooldown.remaining.as_millis() as u64,
                    "skipping codex account on temporary request-path cooldown"
                );
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
        if saw_account_cooldown {
            return Err((StatusCode::TOO_MANY_REQUESTS, "quota_exceeded").into_response());
        }
        if saw_terminal_auth_error {
            return Err((
                StatusCode::BAD_GATEWAY,
                "all eligible codex accounts failed for this request",
            )
                .into_response());
        }
        return Err((StatusCode::SERVICE_UNAVAILABLE, "no usable codex account is configured")
            .into_response());
    }
}

async fn select_kiro_route_with_account_permit(
    scheduler: &Arc<KiroRequestScheduler>,
    routes: &[ProviderKiroRoute],
    failed_accounts: &HashSet<String>,
    latency_ranker: &KiroLatencyRanker,
    preferred_account_name: Option<&str>,
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
        let proxy_cooldowns = scheduler.proxy_cooldown_snapshot();
        if let Some(preferred_route) = preferred_account_name
            .and_then(|account_name| {
                routes.iter().find(|route| {
                    route.account_name == account_name
                        && !failed_accounts.contains(&route.account_name)
                })
            })
            .filter(|route| {
                proxy_cooldown_key_for_route(route)
                    .is_none_or(|key| !proxy_cooldowns.contains_key(&key))
            })
        {
            if scheduler
                .cooldown_for_account(&preferred_route.routing_identity)
                .is_none()
            {
                if let Ok(permit) = scheduler.try_acquire(
                    &preferred_route.routing_identity,
                    preferred_route
                        .account_request_max_concurrency
                        .unwrap_or(llm_access_core::store::DEFAULT_KIRO_CHANNEL_MAX_CONCURRENCY),
                    preferred_route
                        .account_request_min_start_interval_ms
                        .unwrap_or(
                            llm_access_core::store::DEFAULT_KIRO_CHANNEL_MIN_START_INTERVAL_MS,
                        ),
                    queued_at,
                ) {
                    return Ok((preferred_route.clone(), permit));
                }
            }
        }
        for route in selection_ordered_kiro_routes(routes, scheduler, latency_ranker, now_millis())
        {
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

async fn hydrate_codex_route_for_dispatch(
    route: ProviderCodexRoute,
    route_store: &dyn ProviderRouteStore,
) -> Result<ProviderCodexRoute, Response> {
    if !route.auth_json.is_empty() {
        return Ok(route);
    }
    let account_name = route.account_name.clone();
    let loaded = route_store
        .resolve_codex_account_route(&account_name)
        .await
        .map_err(|_| {
            (StatusCode::INTERNAL_SERVER_ERROR, "codex route resolution failed").into_response()
        })?;
    let Some(loaded) = loaded else {
        return Err((
            StatusCode::BAD_GATEWAY,
            "all eligible codex accounts failed for this request",
        )
            .into_response());
    };
    let mut route = route;
    route.auth_json = loaded.auth_json;
    route.map_gpt53_codex_to_spark = loaded.map_gpt53_codex_to_spark;
    route.auth_refresh_enabled = loaded.auth_refresh_enabled;
    route.account_request_max_concurrency = loaded.account_request_max_concurrency;
    route.account_request_min_start_interval_ms = loaded.account_request_min_start_interval_ms;
    route.cached_error_message = loaded.cached_error_message;
    route.proxy = loaded.proxy;
    Ok(route)
}

async fn hydrate_kiro_route_for_dispatch(
    route: ProviderKiroRoute,
    route_store: &dyn ProviderRouteStore,
) -> Result<ProviderKiroRoute, Response> {
    if !route.auth_json.is_empty() {
        return Ok(route);
    }
    let account_name = route.account_name.clone();
    let loaded = route_store
        .resolve_kiro_account_route(&account_name)
        .await
        .map_err(|_| {
            kiro_json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "api_error",
                "kiro route resolution failed",
            )
        })?;
    let Some(loaded) = loaded else {
        return Err(kiro_json_error(
            StatusCode::BAD_GATEWAY,
            "api_error",
            "all eligible kiro accounts failed for this request",
        ));
    };
    let mut route = route;
    route.auth_json = loaded.auth_json;
    if route.profile_arn.is_none() {
        route.profile_arn = loaded.profile_arn;
    }
    if route.api_region.trim().is_empty() {
        route.api_region = loaded.api_region;
    }
    route.account_request_max_concurrency = loaded.account_request_max_concurrency;
    route.account_request_min_start_interval_ms = loaded.account_request_min_start_interval_ms;
    route.proxy = loaded.proxy;
    Ok(route)
}

fn selection_ordered_kiro_routes<'a>(
    routes: &'a [ProviderKiroRoute],
    scheduler: &KiroRequestScheduler,
    latency_ranker: &KiroLatencyRanker,
    now_ms: i64,
) -> Vec<&'a ProviderKiroRoute> {
    #[derive(Clone, Copy)]
    struct Candidate<'a> {
        route: &'a ProviderKiroRoute,
        proxy_in_cooldown: bool,
        last_started_at: Option<Instant>,
        latency_score_ms: Option<f64>,
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
                latency_score_ms: latency_ranker.route_score_ms(route, now_ms),
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
        match (left.latency_score_ms, right.latency_score_ms) {
            (Some(left_score), Some(right_score)) => {
                let ordering = left_score.total_cmp(&right_score);
                if ordering != std::cmp::Ordering::Equal {
                    return ordering;
                }
            },
            (Some(_), None) => return std::cmp::Ordering::Less,
            (None, Some(_)) => return std::cmp::Ordering::Greater,
            (None, None) => {},
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
            return dispatch_codex_proxy(key, request, deps).await;
        }
        if ProviderType::from_storage_str(&key.provider_type) == Some(ProviderType::Kiro) {
            return dispatch_kiro_proxy(key, request, deps).await;
        }
        (StatusCode::NOT_IMPLEMENTED, "provider dispatch is not wired").into_response()
    }
}

async fn dispatch_codex_proxy(
    key: AuthenticatedKey,
    request: Request<Body>,
    deps: ProviderDispatchDeps,
) -> Response {
    let ProviderDispatchDeps {
        route_store,
        control_store,
        geoip,
        admin_config_store,
        request_limiter,
        codex_account_cooldowns,
        ..
    } = deps;
    let mut usage_meta = ProviderUsageMetadata::from_request_parts(
        request.method(),
        request.uri(),
        request.headers(),
        &geoip,
    )
    .await;
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
    let runtime_config = match load_codex_dispatch_runtime_config(admin_config_store.as_ref()).await
    {
        Ok(config) => config,
        Err(response) => return response,
    };

    if gateway_path == "/v1/models" && method == Method::GET {
        let Some(route) = routes.into_iter().next() else {
            return (StatusCode::SERVICE_UNAVAILABLE, "codex route is not configured")
                .into_response();
        };
        let route = match hydrate_codex_route_for_dispatch(route, route_store.as_ref()).await {
            Ok(route) => route,
            Err(response) => return response,
        };
        return codex_openai_models_response(
            route,
            route_store,
            &request_headers,
            query.trim_start_matches('?'),
            &upstream_base,
            &runtime_config.client_version,
        )
        .await;
    }

    let body_read_started = Instant::now();
    let body = match to_bytes(request.into_body(), MAX_PROVIDER_PROXY_BODY_BYTES).await {
        Ok(body) => body,
        Err(_) => {
            let message = "request body is too large";
            capture_error_message(&mut usage_meta, message);
            capture_error_body(
                &mut usage_meta,
                &codex_surface_error_body(&gateway_path, StatusCode::BAD_REQUEST, message),
            );
            record_codex_preflight_failure(CodexPreflightFailureRecord {
                control_store: control_store.as_ref(),
                key: &key,
                endpoint: &gateway_path,
                model: None,
                status: StatusCode::BAD_REQUEST,
                meta: &mut usage_meta,
            })
            .await;
            return codex_surface_error_response(
                &gateway_path,
                StatusCode::BAD_REQUEST,
                "request body is too large",
            );
        },
    };
    usage_meta =
        usage_meta.with_request_body(&body, clamp_duration_ms(body_read_started.elapsed()));
    let parse_started = Instant::now();
    let prepared = match prepare_gateway_request_from_bytes(
        &gateway_path,
        &query,
        method,
        &request_headers,
        body.clone(),
        MAX_PROVIDER_PROXY_BODY_BYTES,
    ) {
        Ok(prepared) => prepared,
        Err(err) => {
            capture_client_request_body_json(&mut usage_meta, &body);
            if usage_meta.last_message_content.is_none() {
                usage_meta.last_message_content =
                    extract_codex_last_message_content(&body).ok().flatten();
            }
            tracing::error!(
                key_id = %key.key_id,
                endpoint = %gateway_path,
                status = %err.status,
                error_message = %err.message,
                "codex request rejected before upstream dispatch"
            );
            capture_error_message(&mut usage_meta, &err.message);
            capture_error_body(
                &mut usage_meta,
                &codex_surface_error_body(&gateway_path, err.status, &err.message),
            );
            record_codex_preflight_failure(CodexPreflightFailureRecord {
                control_store: control_store.as_ref(),
                key: &key,
                endpoint: &gateway_path,
                model: extract_model_from_json_body(&body),
                status: err.status,
                meta: &mut usage_meta,
            })
            .await;
            return codex_surface_error_response(&gateway_path, err.status, &err.message);
        },
    };
    usage_meta.mark_pre_handler_done(clamp_duration_ms(parse_started.elapsed()));
    usage_meta.last_message_content = prepared.last_message_content.clone();
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
    let account_attempt_limit = runtime_config.account_attempt_limit;
    let mut key_permit = Some(key_permit);
    let mut failed_accounts = HashSet::new();
    let mut attempt_count = 0_usize;
    loop {
        let route_started = Instant::now();
        let (route, account_permit) = match select_codex_route_with_account_permit(
            &request_limiter,
            &codex_account_cooldowns,
            &routes,
            &failed_accounts,
        )
        .await
        {
            Ok(value) => value,
            Err(response) => return response,
        };
        usage_meta.add_routing_wait(clamp_duration_ms(route_started.elapsed()));
        attempt_count = attempt_count.saturating_add(1);
        let selected_account_name = route.account_name.clone();
        let route = match hydrate_codex_route_for_dispatch(route, route_store.as_ref()).await {
            Ok(route) => route,
            Err(_) => {
                mark_codex_transient_request_failure_cooldown(
                    &codex_account_cooldowns,
                    &selected_account_name,
                );
                usage_meta.mark_failover();
                failed_accounts.insert(selected_account_name);
                if attempt_count >= account_attempt_limit {
                    return (
                        StatusCode::BAD_GATEWAY,
                        "all eligible codex accounts failed for this request",
                    )
                        .into_response();
                }
                continue;
            },
        };
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
                mark_codex_transient_request_failure_cooldown(
                    &codex_account_cooldowns,
                    &route.account_name,
                );
                usage_meta.mark_failover();
                failed_accounts.insert(route.account_name.clone());
                if attempt_count >= account_attempt_limit {
                    return (
                        StatusCode::BAD_GATEWAY,
                        "all eligible codex accounts failed for this request",
                    )
                        .into_response();
                }
                continue;
            },
        };
        let prepared =
            match apply_gpt53_codex_spark_mapping(&prepared, route.map_gpt53_codex_to_spark) {
                Ok(prepared) => prepared,
                Err(err) => return (err.status, err.message).into_response(),
            };
        let prepared = match apply_codex_fast_policy(&prepared, route.codex_fast_enabled) {
            Ok(prepared) => prepared,
            Err(err) => return (err.status, err.message).into_response(),
        };
        let prepared = match align_responses_store_with_upstream(&prepared, &upstream_base) {
            Ok(prepared) => prepared,
            Err(err) => return (err.status, err.message).into_response(),
        };
        let upstream_url = compute_codex_upstream_url(&upstream_base, &prepared.upstream_path);
        let client = match provider_client(route.proxy.as_ref()) {
            Ok(client) => client,
            Err(_) => {
                mark_codex_transient_request_failure_cooldown(
                    &codex_account_cooldowns,
                    &route.account_name,
                );
                usage_meta.mark_failover();
                failed_accounts.insert(route.account_name.clone());
                if attempt_count >= account_attempt_limit {
                    return (
                        StatusCode::BAD_GATEWAY,
                        "all eligible codex accounts failed for this request",
                    )
                        .into_response();
                }
                continue;
            },
        };
        let upstream = add_codex_upstream_headers(
            client.request(method.clone(), upstream_url.clone()),
            &request_headers,
            &prepared,
            &auth,
            &runtime_config.client_version,
        );
        let mut response = match upstream.send().await {
            Ok(response) => {
                usage_meta.mark_upstream_headers();
                response
            },
            Err(_) => {
                mark_codex_transient_request_failure_cooldown(
                    &codex_account_cooldowns,
                    &route.account_name,
                );
                usage_meta.mark_failover();
                failed_accounts.insert(route.account_name.clone());
                if attempt_count >= account_attempt_limit {
                    return (
                        StatusCode::BAD_GATEWAY,
                        "all eligible codex accounts failed for this request",
                    )
                        .into_response();
                }
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
                        client.request(method.clone(), upstream_url.clone()),
                        &request_headers,
                        &prepared,
                        &auth,
                        &runtime_config.client_version,
                    );
                    response = match retry.send().await {
                        Ok(response) => {
                            usage_meta.mark_upstream_headers();
                            response
                        },
                        Err(_) => {
                            mark_codex_transient_request_failure_cooldown(
                                &codex_account_cooldowns,
                                &route.account_name,
                            );
                            usage_meta.mark_failover();
                            failed_accounts.insert(route.account_name.clone());
                            if attempt_count >= account_attempt_limit {
                                return (
                                    StatusCode::BAD_GATEWAY,
                                    "all eligible codex accounts failed for this request",
                                )
                                    .into_response();
                            }
                            continue;
                        },
                    };
                },
                Err(_) => {
                    mark_codex_transient_request_failure_cooldown(
                        &codex_account_cooldowns,
                        &route.account_name,
                    );
                    usage_meta.mark_failover();
                    failed_accounts.insert(route.account_name.clone());
                    if attempt_count >= account_attempt_limit {
                        return (
                            StatusCode::BAD_GATEWAY,
                            "all eligible codex accounts failed for this request",
                        )
                            .into_response();
                    }
                    continue;
                },
            }
        }
        if matches!(response.status(), StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
            let status = response.status();
            let upstream_headers = response.headers().clone();
            let content_type = upstream_headers
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("application/json")
                .to_string();
            let bytes = match response.bytes().await {
                Ok(bytes) => bytes,
                Err(_) => {
                    return (StatusCode::BAD_GATEWAY, "codex upstream response read failed")
                        .into_response()
                },
            };
            codex_refresh::persist_terminal_request_auth_error(
                &route,
                route_store.as_ref(),
                status,
                &bytes,
            )
            .await;
            mark_codex_transient_request_failure_cooldown(
                &codex_account_cooldowns,
                &route.account_name,
            );
            if attempt_count < account_attempt_limit
                && routes.iter().any(|candidate| {
                    !failed_accounts.contains(&candidate.account_name)
                        && candidate.account_name != route.account_name
                })
            {
                usage_meta.mark_failover();
                failed_accounts.insert(route.account_name.clone());
                continue;
            }
            let permits = vec![
                key_permit
                    .take()
                    .expect("codex key permit should be held until response is returned"),
                account_permit,
            ];
            capture_codex_dispatch_request_json(&mut usage_meta, &body, &prepared);
            return adapt_codex_upstream_response_from_parts(
                CodexUpstreamResponseParts {
                    status,
                    upstream_headers,
                    content_type,
                    bytes,
                },
                CodexCompletedResponseContext {
                    prepared,
                    key,
                    route,
                    control_store,
                    permits,
                    usage_meta,
                },
            )
            .await;
        }
        if response.status().is_success() {
            let permits = vec![
                key_permit
                    .take()
                    .expect("codex key permit should be held until response is returned"),
                account_permit,
            ];
            return adapt_codex_upstream_response(response, CodexUpstreamResponseContext {
                prepared,
                key,
                route,
                control_store,
                permits,
                usage_meta,
            })
            .await;
        }
        let mut response_prepared = prepared.clone();
        let mut status = response.status();
        let mut upstream_headers = response.headers().clone();
        let mut content_type = upstream_headers
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("application/json")
            .to_string();
        let mut bytes = match response.bytes().await {
            Ok(bytes) => bytes,
            Err(_) => {
                return (StatusCode::BAD_GATEWAY, "codex upstream response read failed")
                    .into_response()
            },
        };
        if is_codex_invalid_encrypted_content_response(status, &bytes) {
            if let Some(retry_prepared) = retry_codex_without_encrypted_reasoning(&prepared) {
                let retry = add_codex_upstream_headers(
                    client.request(method.clone(), upstream_url.clone()),
                    &request_headers,
                    &retry_prepared,
                    &auth,
                    &runtime_config.client_version,
                );
                response = match retry.send().await {
                    Ok(response) => {
                        usage_meta.mark_upstream_headers();
                        response
                    },
                    Err(_) => {
                        mark_codex_transient_request_failure_cooldown(
                            &codex_account_cooldowns,
                            &route.account_name,
                        );
                        usage_meta.mark_failover();
                        failed_accounts.insert(route.account_name.clone());
                        if attempt_count >= account_attempt_limit {
                            return (
                                StatusCode::BAD_GATEWAY,
                                "all eligible codex accounts failed for this request",
                            )
                                .into_response();
                        }
                        continue;
                    },
                };
                if response.status().is_success() {
                    let permits = vec![
                        key_permit
                            .take()
                            .expect("codex key permit should be held until response is returned"),
                        account_permit,
                    ];
                    return adapt_codex_upstream_response(response, CodexUpstreamResponseContext {
                        prepared: retry_prepared,
                        key,
                        route,
                        control_store,
                        permits,
                        usage_meta,
                    })
                    .await;
                }
                response_prepared = retry_prepared;
                status = response.status();
                upstream_headers = response.headers().clone();
                content_type = upstream_headers
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("application/json")
                    .to_string();
                bytes = match response.bytes().await {
                    Ok(bytes) => bytes,
                    Err(_) => {
                        return (StatusCode::BAD_GATEWAY, "codex upstream response read failed")
                            .into_response()
                    },
                };
            }
        }
        if let Some(cooldown) = codex_temporary_request_failure_cooldown(status, &bytes) {
            codex_account_cooldowns.mark_account_cooldown(&route.account_name, cooldown);
        }
        if !is_codex_invalid_encrypted_content_response(status, &bytes)
            && !is_codex_non_retryable_client_error_response(status, &bytes)
            && attempt_count < account_attempt_limit
            && routes.iter().any(|candidate| {
                !failed_accounts.contains(&candidate.account_name)
                    && candidate.account_name != route.account_name
            })
        {
            usage_meta.mark_failover();
            failed_accounts.insert(route.account_name.clone());
            continue;
        }
        let permits = vec![
            key_permit
                .take()
                .expect("codex key permit should be held until response is returned"),
            account_permit,
        ];
        capture_codex_dispatch_request_json(&mut usage_meta, &body, &response_prepared);
        return adapt_codex_upstream_response_from_parts(
            CodexUpstreamResponseParts {
                status,
                upstream_headers,
                content_type,
                bytes,
            },
            CodexCompletedResponseContext {
                prepared: response_prepared,
                key,
                route,
                control_store,
                permits,
                usage_meta,
            },
        )
        .await;
    }
}

struct CodexUpstreamResponseContext {
    prepared: PreparedGatewayRequest,
    key: AuthenticatedKey,
    route: ProviderCodexRoute,
    control_store: Arc<dyn ControlStore>,
    permits: Vec<LimitPermit>,
    usage_meta: ProviderUsageMetadata,
}

async fn adapt_codex_upstream_response(
    response: reqwest::Response,
    ctx: CodexUpstreamResponseContext,
) -> Response {
    let CodexUpstreamResponseContext {
        prepared,
        key,
        route,
        control_store,
        permits,
        mut usage_meta,
    } = ctx;
    let status = response.status();
    let upstream_headers = response.headers().clone();
    let content_type = upstream_headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/json")
        .to_string();
    let has_event_stream_content_type =
        status.is_success() && content_type.contains("text/event-stream");
    let expects_stream_response =
        status.is_success() && (has_event_stream_content_type || prepared.wants_stream);

    if status.is_success()
        && !prepared.wants_stream
        && (has_event_stream_content_type || prepared.force_upstream_stream)
    {
        let bytes = match response.bytes().await {
            Ok(bytes) => bytes,
            Err(_) => {
                return (StatusCode::BAD_GATEWAY, "codex upstream response read failed")
                    .into_response()
            },
        };
        if !has_event_stream_content_type && serde_json::from_slice::<Value>(&bytes).is_ok() {
            return adapt_codex_upstream_response_from_parts(
                CodexUpstreamResponseParts {
                    status,
                    upstream_headers,
                    content_type,
                    bytes,
                },
                CodexCompletedResponseContext {
                    prepared,
                    key,
                    route,
                    control_store,
                    permits,
                    usage_meta,
                },
            )
            .await;
        }
        usage_meta.mark_post_headers_body();
        usage_meta.mark_stream_finish();
        let completed = match completed_response_from_sse_bytes(&bytes) {
            Ok(value) => value,
            Err(err) => {
                tracing::error!(
                    endpoint = %prepared.original_path,
                    status = %err.status,
                    message = %err.message,
                    "codex forced-SSE upstream request failed before response.completed"
                );
                capture_codex_prepared_request_json(&mut usage_meta, &prepared);
                capture_error_message(&mut usage_meta, &err.message);
                if let Some(body) = err.body.as_deref() {
                    capture_error_body(&mut usage_meta, body);
                }
                if let Err(record_err) = record_codex_usage(
                    control_store.as_ref(),
                    &key,
                    &prepared,
                    err.status,
                    &route,
                    missing_codex_usage(),
                    &usage_meta,
                )
                .await
                {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("failed to record codex usage: {record_err}"),
                    )
                        .into_response();
                }
                return codex_surface_error_response(
                    &prepared.original_path,
                    err.status,
                    &err.message,
                );
            },
        };
        let completed_response = rewrite_json_value_model_alias(
            completed.response,
            prepared.model.as_deref(),
            prepared.client_visible_model.as_deref(),
        );
        let adapted = adapt_completed_response_json(
            completed_response,
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

    if expects_stream_response {
        let prepared = strip_codex_stream_request_bodies(prepared);
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
    adapt_codex_upstream_response_from_parts(
        CodexUpstreamResponseParts {
            status,
            upstream_headers,
            content_type,
            bytes,
        },
        CodexCompletedResponseContext {
            prepared,
            key,
            route,
            control_store,
            permits,
            usage_meta,
        },
    )
    .await
}

struct CodexUpstreamResponseParts {
    status: StatusCode,
    upstream_headers: reqwest::header::HeaderMap,
    content_type: String,
    bytes: Bytes,
}

struct CodexCompletedResponseContext {
    prepared: PreparedGatewayRequest,
    key: AuthenticatedKey,
    route: ProviderCodexRoute,
    control_store: Arc<dyn ControlStore>,
    permits: Vec<LimitPermit>,
    usage_meta: ProviderUsageMetadata,
}

async fn adapt_codex_upstream_response_from_parts(
    parts: CodexUpstreamResponseParts,
    ctx: CodexCompletedResponseContext,
) -> Response {
    let CodexUpstreamResponseParts {
        status,
        upstream_headers,
        content_type,
        bytes,
    } = parts;
    let CodexCompletedResponseContext {
        prepared,
        key,
        route,
        control_store,
        permits: _permits,
        mut usage_meta,
    } = ctx;
    usage_meta.mark_post_headers_body();
    usage_meta.mark_stream_finish();
    let effective_success_bytes = &bytes;
    let usage = if status.is_success() {
        extract_usage_from_bytes(effective_success_bytes).unwrap_or_else(missing_codex_usage)
    } else {
        capture_error_bytes(&mut usage_meta, &bytes);
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
    if !status.is_success()
        && prepared.response_adapter == GatewayResponseAdapter::AnthropicMessages
    {
        let message = summarize_error_bytes(&bytes);
        let body = json!({
            "error": {
                "type": codex_error_type_for_status(status),
                "message": message,
            }
        });
        let builder = Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::CACHE_CONTROL, "no-store");
        return apply_upstream_response_headers(builder, &upstream_headers)
            .body(Body::from(body.to_string()))
            .unwrap_or_else(|_| {
                (StatusCode::BAD_GATEWAY, "codex upstream response build failed").into_response()
            });
    }
    let response_content_type =
        if status.is_success() && prepared.response_adapter != GatewayResponseAdapter::Responses {
            "application/json"
        } else {
            &content_type
        };
    let response_body = if status.is_success() {
        match prepared.response_adapter {
            GatewayResponseAdapter::Responses => {
                if let Some(body) = rewrite_json_response_model_alias(
                    effective_success_bytes,
                    prepared.model.as_deref(),
                    prepared.client_visible_model.as_deref(),
                ) {
                    Body::from(body)
                } else {
                    Body::from(bytes)
                }
            },
            GatewayResponseAdapter::ChatCompletions => {
                match convert_json_response_to_chat_completion(
                    &bytes,
                    Some(&prepared.tool_name_restore_map),
                    prepared.model.as_deref(),
                    prepared.client_visible_model.as_deref(),
                ) {
                    Ok(body) => Body::from(body),
                    Err(err) => return (StatusCode::BAD_GATEWAY, err).into_response(),
                }
            },
            GatewayResponseAdapter::AnthropicMessages => {
                match convert_json_response_to_anthropic_message(
                    &bytes,
                    Some(&prepared.tool_name_restore_map),
                    prepared.model.as_deref(),
                    prepared.client_visible_model.as_deref(),
                ) {
                    Ok(body) => Body::from(body),
                    Err(err) => return (StatusCode::BAD_GATEWAY, err).into_response(),
                }
            },
        }
    } else if let Some(body) = rewrite_json_response_model_alias(
        &bytes,
        prepared.model.as_deref(),
        prepared.client_visible_model.as_deref(),
    ) {
        Body::from(body)
    } else {
        Body::from(bytes)
    };
    let builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, response_content_type)
        .header(header::CACHE_CONTROL, "no-store");
    apply_upstream_response_headers(builder, &upstream_headers)
        .body(response_body)
        .unwrap_or_else(|_| {
            (StatusCode::BAD_GATEWAY, "codex upstream response build failed").into_response()
        })
}

fn codex_quota_exhaustion_cooldown(status: StatusCode, bytes: &Bytes) -> Option<Duration> {
    if !matches!(
        status,
        StatusCode::TOO_MANY_REQUESTS | StatusCode::PAYMENT_REQUIRED | StatusCode::FORBIDDEN
    ) {
        return None;
    }
    let body = String::from_utf8_lossy(bytes.as_ref());
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes.as_ref()) {
        for pointer in ["/error/code", "/code", "/response/error/code"] {
            if value.pointer(pointer).and_then(serde_json::Value::as_str)
                == Some("insufficient_quota")
            {
                return Some(CODEX_QUOTA_EXHAUSTION_COOLDOWN);
            }
        }
        for pointer in ["/error/message", "/message", "/response/error/message"] {
            if value
                .pointer(pointer)
                .and_then(serde_json::Value::as_str)
                .is_some_and(codex_message_indicates_usage_limit)
            {
                return Some(CODEX_QUOTA_EXHAUSTION_COOLDOWN);
            }
        }
    }
    if codex_message_indicates_usage_limit(&body) {
        return Some(CODEX_QUOTA_EXHAUSTION_COOLDOWN);
    }
    None
}

fn codex_message_indicates_usage_limit(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    normalized.contains("usage limit")
        || normalized.contains("insufficient_quota")
        || normalized.contains("quota_exceeded")
        || normalized.contains("quota exceeded")
}

fn randomized_codex_transient_account_failure_cooldown<R: Rng + ?Sized>(rng: &mut R) -> Duration {
    let min_ms = CODEX_TRANSIENT_ACCOUNT_FAILURE_COOLDOWN_MIN
        .as_millis()
        .min(u128::from(u64::MAX)) as u64;
    let max_ms = CODEX_TRANSIENT_ACCOUNT_FAILURE_COOLDOWN_MAX
        .as_millis()
        .min(u128::from(u64::MAX)) as u64;
    Duration::from_millis(rng.gen_range(min_ms..=max_ms))
}

fn codex_temporary_request_failure_cooldown(status: StatusCode, bytes: &Bytes) -> Option<Duration> {
    // Request-shape failures must stay on the existing same-account retry path.
    // Cooling the account for those errors would poison healthy accounts for a
    // client-side bug that is independent of the selected route.
    if is_codex_invalid_encrypted_content_response(status, bytes) {
        return None;
    }

    // Explicit upstream quota signals still deserve the stronger existing
    // cooldown window because they are not a transient transport blip.
    if let Some(cooldown) = codex_quota_exhaustion_cooldown(status, bytes) {
        return Some(cooldown);
    }

    // Everything else here is a request-path account failure signal: a
    // transport/proxy/upstream problem happened after we already selected an
    // account. We do not write this into persisted account status. We only
    // keep the account out of the selection pool for a short randomized window
    // so subsequent requests stop paying the same failover tax immediately.
    if status.is_server_error()
        || matches!(
            status,
            StatusCode::UNAUTHORIZED
                | StatusCode::FORBIDDEN
                | StatusCode::PAYMENT_REQUIRED
                | StatusCode::TOO_MANY_REQUESTS
                | StatusCode::REQUEST_TIMEOUT
        )
    {
        return Some(randomized_codex_transient_account_failure_cooldown(&mut rand::thread_rng()));
    }

    None
}

fn mark_codex_transient_request_failure_cooldown(
    codex_account_cooldowns: &Arc<CodexAccountCooldowns>,
    account_name: &str,
) {
    let cooldown = randomized_codex_transient_account_failure_cooldown(&mut rand::thread_rng());
    codex_account_cooldowns.mark_account_cooldown(account_name, cooldown);
}

fn codex_status_from_error_json_value(value: &Value) -> Option<StatusCode> {
    for pointer in ["/error/status", "/status", "/response/error/status"] {
        if let Some(status) = value.pointer(pointer).and_then(Value::as_u64) {
            if let Ok(status) = u16::try_from(status) {
                if let Ok(status) = StatusCode::from_u16(status) {
                    return Some(status);
                }
            }
        }
    }

    for pointer in ["/error/code", "/code", "/response/error/code"] {
        match value.pointer(pointer).and_then(Value::as_str) {
            Some("invalid_api_key") => return Some(StatusCode::UNAUTHORIZED),
            Some("insufficient_quota" | "quota_exceeded" | "rate_limit_exceeded") => {
                return Some(StatusCode::TOO_MANY_REQUESTS)
            },
            Some("bad_gateway") => return Some(StatusCode::BAD_GATEWAY),
            _ => {},
        }
    }

    for pointer in ["/error/type", "/type", "/response/error/type"] {
        match value.pointer(pointer).and_then(Value::as_str) {
            Some("invalid_request_error") => return Some(StatusCode::BAD_REQUEST),
            Some("authentication_error") => return Some(StatusCode::UNAUTHORIZED),
            Some("permission_error") => return Some(StatusCode::FORBIDDEN),
            Some("not_found_error") => return Some(StatusCode::NOT_FOUND),
            Some("rate_limit_error") => return Some(StatusCode::TOO_MANY_REQUESTS),
            Some("api_error") => return Some(StatusCode::BAD_GATEWAY),
            _ => {},
        }
    }

    if extract_error_message_from_json_value(value)
        .as_deref()
        .is_some_and(codex_message_indicates_usage_limit)
    {
        return Some(StatusCode::TOO_MANY_REQUESTS);
    }

    None
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
            usage_meta,
        } = ctx;
        let _permits = permits;
        let mut events = response
            .bytes_stream()
            .map_err(std::io::Error::other)
            .eventsource();
        let mut chat_metadata = ChatStreamMetadata::default();
        let mut anthropic_metadata = AnthropicStreamMetadata::default();
        let mut guard = CodexStreamRecordGuard {
            prepared,
            key,
            route,
            control_store,
            status,
            usage_meta,
            usage_collector: SseUsageCollector::default(),
            state: StreamRecordState::Pending,
            record_committed: false,
        };
        while let Some(event) = events.next().await {
            match event {
                Ok(event) => {
                    guard.usage_collector.observe_event(&event);
                    match response_adapter {
                        GatewayResponseAdapter::Responses => {
                            let bytes = encode_sse_event_with_model_alias(
                                &event,
                                guard.prepared.model.as_deref(),
                                guard.prepared.client_visible_model.as_deref(),
                            );
                            guard.observe_chunk(&bytes, Some(event.event.as_str()));
                            yield Ok::<Bytes, std::io::Error>(bytes);
                        },
                        GatewayResponseAdapter::ChatCompletions => {
                            if let Some(chunk) = convert_response_event_to_chat_chunk(
                                &event,
                                Some(&guard.prepared.tool_name_restore_map),
                                &mut chat_metadata,
                                guard.prepared.model.as_deref(),
                                guard.prepared.client_visible_model.as_deref(),
                            ) {
                                let bytes = encode_json_sse_chunk(&chunk);
                                guard.observe_chunk(&bytes, Some(event.event.as_str()));
                                yield Ok::<Bytes, std::io::Error>(bytes);
                            }
                        },
                        GatewayResponseAdapter::AnthropicMessages => {
                            for bytes in convert_response_event_to_anthropic_sse_chunks(
                                &event,
                                Some(&guard.prepared.tool_name_restore_map),
                                &mut anthropic_metadata,
                                guard.prepared.model.as_deref(),
                                guard.prepared.client_visible_model.as_deref(),
                            ) {
                                guard.observe_chunk(&bytes, Some(event.event.as_str()));
                                yield Ok::<Bytes, std::io::Error>(bytes);
                            }
                        },
                    }
                },
                Err(err) => {
                    guard.mark_internal_failure();
                    yield Err(std::io::Error::other(format!(
                        "failed to parse codex upstream SSE event: {err}"
                    )));
                    return;
                },
            }
        }
        if response_adapter == GatewayResponseAdapter::ChatCompletions {
            let bytes = Bytes::from_static(b"data: [DONE]\n\n");
            guard.observe_chunk(&bytes, Some("done"));
            yield Ok::<Bytes, std::io::Error>(bytes);
        }
        guard.finish_success().await;
    };
    let response_content_type = if response_adapter != GatewayResponseAdapter::Responses {
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
        KiroRemoteMediaKind::Document => {
            "application/pdf,text/csv,application/msword,application/vnd.\
             openxmlformats-officedocument.wordprocessingml.document,application/vnd.ms-excel,\
             application/vnd.openxmlformats-officedocument.spreadsheetml.sheet,text/html,text/\
             plain,text/markdown"
        },
    }
}

async fn resolve_kiro_remote_media_sources(
    payload: &mut MessagesRequest,
) -> Result<(), KiroRemoteMediaResolutionError> {
    if !payload_has_kiro_remote_media_sources(payload) {
        return Ok(());
    }
    let fetcher = ReqwestKiroRemoteMediaFetcher {
        client: KIRO_REMOTE_MEDIA_CLIENT.clone(),
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct StrippedKiroRemoteMediaSource {
    message_index: usize,
    block_index: usize,
    block_type: String,
    url_summary: String,
}

fn strip_kiro_remote_media_sources(
    payload: &mut MessagesRequest,
) -> Vec<StrippedKiroRemoteMediaSource> {
    let mut removed = Vec::new();
    for (message_index, message) in payload.messages.iter_mut().enumerate() {
        if message.role != "user" {
            continue;
        }
        let Some(items) = message.content.as_array_mut() else {
            continue;
        };
        let mut retained = Vec::with_capacity(items.len());
        for (block_index, item) in std::mem::take(items).into_iter().enumerate() {
            if let Some(stripped) =
                stripped_kiro_remote_media_source(&item, message_index, block_index)
            {
                removed.push(stripped);
            } else {
                retained.push(item);
            }
        }
        *items = retained;
    }
    removed
}

fn stripped_kiro_remote_media_source(
    item: &serde_json::Value,
    message_index: usize,
    block_index: usize,
) -> Option<StrippedKiroRemoteMediaSource> {
    let object = item.as_object()?;
    let block_type = object
        .get("type")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)?;
    if !matches!(block_type, "image" | "document") {
        return None;
    }
    let source = object
        .get("source")
        .and_then(serde_json::Value::as_object)?;
    if source
        .get("type")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        != Some("url")
    {
        return None;
    }
    let url_summary = source
        .get("url")
        .and_then(serde_json::Value::as_str)
        .map(summarize_kiro_remote_media_url)
        .unwrap_or_else(|| "(missing url)".to_string());
    Some(StrippedKiroRemoteMediaSource {
        message_index,
        block_index,
        block_type: block_type.to_string(),
        url_summary,
    })
}

fn summarize_kiro_remote_media_url(raw_url: &str) -> String {
    let trimmed = raw_url.trim();
    if trimmed.is_empty() {
        return "(empty url)".to_string();
    }
    if let Ok(mut parsed) = url::Url::parse(trimmed) {
        parsed.set_query(None);
        parsed.set_fragment(None);
        return parsed.to_string();
    }
    trimmed.chars().take(160).collect()
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
                "URL document source must resolve to a supported document type",
            )
        })?;
    match media_type {
        "application/pdf"
        | "application/msword"
        | "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        | "application/vnd.ms-excel"
        | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => {
            Ok(serde_json::json!({
                "type": "base64",
                "media_type": media_type,
                "data": base64::engine::general_purpose::STANDARD.encode(bytes)
            }))
        },
        "text/plain" | "text/markdown" | "text/html" | "text/csv" => {
            let text = std::str::from_utf8(bytes).map_err(|err| {
                KiroRemoteMediaResolutionError::new(format!(
                    "URL text document source is not valid UTF-8: {err}"
                ))
            })?;
            Ok(serde_json::json!({
                "type": "text",
                "media_type": media_type,
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
        "text/csv" => Some("text/csv"),
        "application/msword" => Some("application/msword"),
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => {
            Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document")
        },
        "application/vnd.ms-excel" => Some("application/vnd.ms-excel"),
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => {
            Some("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet")
        },
        "text/html" => Some("text/html"),
        "text/plain" => Some("text/plain"),
        "text/markdown" | "text/md" | "text/x-markdown" => Some("text/markdown"),
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
        Some("csv") => Some("text/csv"),
        Some("doc") => Some("application/msword"),
        Some("docx") => {
            Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document")
        },
        Some("xls") => Some("application/vnd.ms-excel"),
        Some("xlsx") => Some("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"),
        Some("html" | "htm") => Some("text/html"),
        Some("txt") => Some("text/plain"),
        Some("md" | "markdown") => Some("text/markdown"),
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

async fn dispatch_kiro_proxy(
    key: AuthenticatedKey,
    request: Request<Body>,
    deps: ProviderDispatchDeps,
) -> Response {
    let ProviderDispatchDeps {
        route_store,
        control_store,
        geoip,
        kiro_cache_simulator,
        request_limiter,
        kiro_request_scheduler,
        kiro_session_affinity,
        kiro_latency_ranker,
        ..
    } = deps;
    if request.uri().path() == "/v1/models" {
        if request.method() == Method::GET {
            return axum::Json(supported_models_response()).into_response();
        }
        return kiro_json_error(
            StatusCode::METHOD_NOT_ALLOWED,
            "invalid_request_error",
            "unsupported method",
        );
    }
    let mut usage_meta = ProviderUsageMetadata::from_request_parts(
        request.method(),
        request.uri(),
        request.headers(),
        &geoip,
    )
    .await;
    let routes = match route_store.resolve_kiro_route_candidates(&key).await {
        Ok(routes) if !routes.is_empty() => routes,
        Ok(_) => {
            return kiro_json_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "api_error",
                "route is not configured",
            )
        },
        Err(_) => {
            return kiro_json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "api_error",
                "route resolution failed",
            )
        },
    };
    let Some(public_path) = normalized_kiro_messages_path(request.uri().path()) else {
        return kiro_json_error(
            StatusCode::NOT_FOUND,
            "invalid_request_error",
            "unsupported endpoint",
        );
    };
    usage_meta.request_url = external_origin(request.headers())
        .map(|origin| format!("{origin}/api/kiro-gateway{public_path}"))
        .unwrap_or_else(|| format!("/api/kiro-gateway{public_path}"));
    if request.method() != Method::POST {
        return kiro_json_error(
            StatusCode::METHOD_NOT_ALLOWED,
            "invalid_request_error",
            "unsupported method",
        );
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
    let resolved_session =
        resolve_kiro_request_session(&request_headers, payload.metadata.as_ref());
    let affinity_session_id = kiro_affinity_session_id(&resolved_session).map(str::to_string);
    if routes[0].remote_media_resolution_enabled {
        if let Err(err) = resolve_kiro_remote_media_sources(&mut payload).await {
            let message = err.to_string();
            let response =
                kiro_json_error(StatusCode::BAD_REQUEST, "invalid_request_error", &message);
            capture_error_message(&mut usage_meta, &message);
            capture_error_body(
                &mut usage_meta,
                &anthropic_json_error_body("invalid_request_error", &message),
            );
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
    } else {
        let removed_sources = strip_kiro_remote_media_sources(&mut payload);
        if !removed_sources.is_empty() {
            tracing::warn!(
                key_id = %key.key_id,
                key_name = %key.key_name,
                endpoint = %public_path,
                request_url = %usage_meta.request_url,
                model = %effective_model,
                removed_remote_media_sources = removed_sources.len(),
                removed_remote_media_details = ?removed_sources,
                "kiro remote media sources were stripped because key remote media resolution is disabled"
            );
        }
    }
    if route_mcp_web_search {
        let request_input_tokens = token::count_all_tokens(
            &payload.model,
            payload.system.as_deref(),
            &payload.messages,
            payload.tools.as_deref(),
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
            kiro_session_affinity,
            kiro_latency_ranker,
            affinity_session_id,
            request_input_tokens,
            usage_meta,
        })
        .await;
    }
    let request_input_tokens = token::count_all_tokens(
        &payload.model,
        payload.system.as_deref(),
        &payload.messages,
        payload.tools.as_deref(),
    ) as i32;
    override_kiro_thinking_from_model_name(&mut payload);
    let normalized = match normalize_request(&payload) {
        Ok(normalized) => normalized,
        Err(err) => {
            let message = err.to_string();
            let response = kiro_conversion_error_response(err);
            capture_error_message(&mut usage_meta, &message);
            capture_error_body(
                &mut usage_meta,
                &anthropic_json_error_body("invalid_request_error", &message),
            );
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
    let conversion = match convert_normalized_request_with_resolved_session(
        normalized,
        routes[0].request_validation_enabled,
        resolved_session,
    ) {
        Ok(conversion) => conversion,
        Err(err) => {
            let message = err.to_string();
            let response = kiro_conversion_error_response(err);
            capture_error_message(&mut usage_meta, &message);
            capture_error_body(
                &mut usage_meta,
                &anthropic_json_error_body("invalid_request_error", &message),
            );
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
    let thinking_enabled = payload.thinking.as_ref().is_some_and(|thinking| {
        thinking.exposes_anthropic_thinking(payload.output_config.as_ref())
    });
    let hidden_thinking_enabled = payload.thinking.as_ref().is_some_and(|thinking| {
        thinking.is_enabled()
            && !thinking.exposes_anthropic_thinking(payload.output_config.as_ref())
    });
    let base_conversation_state = conversion.conversation_state.clone();
    let key_permit = match try_acquire_key_permit(
        &request_limiter,
        &key,
        routes[0].request_max_concurrency,
        routes[0].request_min_start_interval_ms,
    ) {
        Ok(permit) => permit,
        Err(rejection) => return kiro_key_limit_response(&rejection),
    };
    let mut key_permit = Some(key_permit);
    let mut failed_accounts = HashSet::new();
    let preferred_account_name = affinity_session_id
        .as_deref()
        .and_then(|session_id| kiro_session_affinity.lookup(&key.key_id, session_id));
    loop {
        let route_started = Instant::now();
        let (route, account_permit) = match select_kiro_route_with_account_permit(
            &kiro_request_scheduler,
            &routes,
            &failed_accounts,
            kiro_latency_ranker.as_ref(),
            preferred_account_name.as_deref(),
        )
        .await
        {
            Ok(value) => value,
            Err(response) => return response,
        };
        usage_meta.add_routing_wait(clamp_duration_ms(route_started.elapsed()));
        let selected_account_name = route.account_name.clone();
        let route = match hydrate_kiro_route_for_dispatch(route, route_store.as_ref()).await {
            Ok(route) => route,
            Err(response) => {
                usage_meta.mark_failover();
                failed_accounts.insert(selected_account_name);
                if has_remaining_kiro_candidate(&routes, &failed_accounts, "") {
                    continue;
                }
                return response;
            },
        };
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
            if let Some(recovered) = kiro_cache_simulator
                .recover_conversation_id_from_runtime_projection(
                    &cache_ctx.projection,
                    cache_ctx.simulation_config,
                    Instant::now(),
                )
            {
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
        let upstream_url = format!(
            "{}/generateAssistantResponse",
            kiro_refresh::runtime_upstream_base_url(&route.api_region)
        );
        let response = match call_kiro_generate_for_route(
            &route,
            route_store.as_ref(),
            upstream_url.clone(),
            &request_body,
        )
        .await
        {
            Ok(response) => {
                usage_meta.mark_upstream_headers();
                response
            },
            Err(failure) => {
                if should_failover_after_kiro_route_failure(
                    &failure,
                    &route,
                    &routes,
                    &mut failed_accounts,
                    route_store.as_ref(),
                    &kiro_request_scheduler,
                )
                .await
                {
                    usage_meta.mark_failover();
                    continue;
                }
                let prompt_too_long_response = kiro_prompt_too_long_response_for_body(
                    failure.status,
                    &failure.body,
                    &effective_model,
                    request_input_tokens,
                );
                let status = if prompt_too_long_response.is_some() {
                    StatusCode::PAYLOAD_TOO_LARGE
                } else {
                    failure.status
                };
                capture_client_request_body_json(&mut usage_meta, &body);
                capture_upstream_request_body_json(&mut usage_meta, &request_body);
                capture_error_bytes(&mut usage_meta, &failure.body);
                usage_meta.mark_stream_finish();
                let error_response =
                    prompt_too_long_response.unwrap_or_else(|| failure.into_response());
                let usage = build_kiro_usage_summary(
                    &effective_model,
                    KiroUsageInputs {
                        request_input_tokens,
                        context_input_tokens: None,
                        context_usage_min_request_tokens: route.context_usage_min_request_tokens,
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
                    tracing::error!(
                        error = %err,
                        "Failed to record gateway usage for route establishment failure"
                    );
                    return kiro_json_error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "api_error",
                        "failed to record usage",
                    );
                }
                return error_response;
            },
        };
        if !response.status().is_success() {
            let upstream_status = response.status();
            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("application/json")
                .to_string();
            let bytes = response.bytes().await.unwrap_or_else(|_| Bytes::new());
            capture_client_request_body_json(&mut usage_meta, &body);
            capture_upstream_request_body_json(&mut usage_meta, &request_body);
            capture_error_bytes(&mut usage_meta, &bytes);
            usage_meta.mark_stream_finish();
            let prompt_too_long_response = kiro_prompt_too_long_response_for_body(
                upstream_status,
                &bytes,
                &effective_model,
                request_input_tokens,
            );
            let status = if prompt_too_long_response.is_some() {
                StatusCode::PAYLOAD_TOO_LARGE
            } else {
                upstream_status
            };
            let usage = build_kiro_usage_summary(
                &effective_model,
                KiroUsageInputs {
                    request_input_tokens,
                    context_input_tokens: None,
                    context_usage_min_request_tokens: route.context_usage_min_request_tokens,
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
                tracing::error!(
                    error = %err,
                    "Failed to record gateway usage for upstream error response"
                );
                return kiro_json_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "api_error",
                    "failed to record usage",
                );
            }
            return prompt_too_long_response.unwrap_or_else(|| {
                kiro_upstream_error_response(upstream_status, &content_type, bytes)
            });
        }
        if payload.stream {
            let stream_response = match prepare_kiro_stream_response_for_route(
                response,
                &route,
                route_store.as_ref(),
                &upstream_url,
                &request_body,
                &effective_model,
                request_input_tokens,
            )
            .await
            {
                Ok(stream_response) => stream_response,
                Err(failure) => {
                    if should_failover_after_kiro_route_failure(
                        &failure,
                        &route,
                        &routes,
                        &mut failed_accounts,
                        route_store.as_ref(),
                        &kiro_request_scheduler,
                    )
                    .await
                    {
                        usage_meta.mark_failover();
                        continue;
                    }
                    let status = failure.status;
                    capture_client_request_body_json(&mut usage_meta, &body);
                    capture_upstream_request_body_json(&mut usage_meta, &request_body);
                    capture_error_bytes(&mut usage_meta, &failure.body);
                    usage_meta.mark_stream_finish();
                    let usage = build_kiro_usage_summary(
                        &effective_model,
                        KiroUsageInputs {
                            request_input_tokens,
                            context_input_tokens: None,
                            context_usage_min_request_tokens: route
                                .context_usage_min_request_tokens,
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
                        tracing::error!(
                            error = %err,
                            "Failed to record gateway usage for buffered stream failure"
                        );
                        return kiro_json_error(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "api_error",
                            "failed to record usage",
                        );
                    }
                    return failure.into_response();
                },
            };
            remember_kiro_session_affinity(
                kiro_session_affinity.as_ref(),
                &key.key_id,
                affinity_session_id.as_deref(),
                &route.account_name,
            );
            let response_ctx = KiroResponseContext {
                key,
                route,
                public_path: public_path.to_string(),
                model: effective_model,
                request_input_tokens,
                thinking_enabled,
                hidden_thinking_enabled,
                tool_name_map: conversion.tool_name_map.clone(),
                structured_output_tool_name: conversion.structured_output_tool_name.clone(),
                response_identity: conversion.response_identity.clone(),
                cache_ctx,
                control_store,
                kiro_cache_simulator,
                usage_meta,
                affinity_update: None,
                _key_permit: key_permit
                    .take()
                    .expect("kiro key permit should be held until response is returned"),
                _account_permit: account_permit,
            };
            return stream_kiro_upstream_response(stream_response, response_ctx);
        }
        let affinity_update =
            affinity_session_id
                .clone()
                .map(|session_id| KiroResponseAffinityUpdate {
                    affinity: Arc::clone(&kiro_session_affinity),
                    session_id,
                });
        let response_ctx = KiroResponseContext {
            key,
            route,
            public_path: public_path.to_string(),
            model: effective_model,
            request_input_tokens,
            thinking_enabled,
            hidden_thinking_enabled,
            tool_name_map: conversion.tool_name_map.clone(),
            structured_output_tool_name: conversion.structured_output_tool_name.clone(),
            response_identity: conversion.response_identity.clone(),
            cache_ctx,
            control_store,
            kiro_cache_simulator,
            usage_meta,
            affinity_update,
            _key_permit: key_permit
                .take()
                .expect("kiro key permit should be held until response is returned"),
            _account_permit: account_permit,
        };
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
    hidden_thinking_enabled: bool,
    tool_name_map: std::collections::HashMap<String, String>,
    structured_output_tool_name: Option<String>,
    response_identity: Option<ResponseModelIdentity>,
    cache_ctx: KiroCacheContext,
    control_store: Arc<dyn ControlStore>,
    kiro_cache_simulator: Arc<KiroCacheSimulator>,
    usage_meta: ProviderUsageMetadata,
    affinity_update: Option<KiroResponseAffinityUpdate>,
    _key_permit: LimitPermit,
    _account_permit: KiroRequestLease,
}

struct KiroResponseAffinityUpdate {
    affinity: Arc<KiroSessionAffinity>,
    session_id: String,
}

struct KiroWebsearchDispatch {
    key: AuthenticatedKey,
    payload: MessagesRequest,
    routes: Vec<ProviderKiroRoute>,
    control_store: Arc<dyn ControlStore>,
    route_store: Arc<dyn ProviderRouteStore>,
    request_limiter: Arc<RequestLimiter>,
    kiro_request_scheduler: Arc<KiroRequestScheduler>,
    kiro_session_affinity: Arc<KiroSessionAffinity>,
    kiro_latency_ranker: Arc<KiroLatencyRanker>,
    affinity_session_id: Option<String>,
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
        kiro_session_affinity,
        kiro_latency_ranker,
        affinity_session_id,
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
    let preferred_account_name = affinity_session_id
        .as_deref()
        .and_then(|session_id| kiro_session_affinity.lookup(&key.key_id, session_id));
    loop {
        let route_started = Instant::now();
        let (route, account_permit) = match select_kiro_route_with_account_permit(
            &kiro_request_scheduler,
            &routes,
            &failed_accounts,
            kiro_latency_ranker.as_ref(),
            preferred_account_name.as_deref(),
        )
        .await
        {
            Ok(value) => value,
            Err(response) => return response,
        };
        usage_meta.add_routing_wait(clamp_duration_ms(route_started.elapsed()));
        let selected_account_name = route.account_name.clone();
        let route = match hydrate_kiro_route_for_dispatch(route, route_store.as_ref()).await {
            Ok(route) => route,
            Err(response) => {
                usage_meta.mark_failover();
                failed_accounts.insert(selected_account_name);
                if has_remaining_kiro_candidate(&routes, &failed_accounts, "") {
                    continue;
                }
                return response;
            },
        };
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
                remember_kiro_session_affinity(
                    kiro_session_affinity.as_ref(),
                    &key.key_id,
                    affinity_session_id.as_deref(),
                    &route.account_name,
                );
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
                if should_failover_after_kiro_route_failure(
                    &failure,
                    &route,
                    &routes,
                    &mut failed_accounts,
                    route_store.as_ref(),
                    &kiro_request_scheduler,
                )
                .await
                {
                    usage_meta.mark_failover();
                    continue;
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
        tracing::error!(error = %err, "Failed to record gateway usage for web search response");
        return kiro_json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "api_error",
            "failed to record usage",
        );
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
                kiro_json_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "api_error",
                    "failed to build stream response",
                )
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
            kiro_json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "api_error",
                "failed to build response",
            )
        })
}

const KIRO_EMPTY_STREAM_MAX_RETRIES: usize = 2;

struct KiroPeekedStream {
    status: StatusCode,
    buffered_prefix: Bytes,
    remaining: futures_util::stream::BoxStream<'static, Result<Bytes, reqwest::Error>>,
}

enum KiroStreamPeekError {
    Empty,
    Incomplete,
    Decode(String),
    Read(reqwest::Error),
}

async fn peek_kiro_stream(
    response: reqwest::Response,
) -> Result<KiroPeekedStream, KiroStreamPeekError> {
    let status = response.status();
    let mut body_stream = response.bytes_stream();
    let mut buffered_prefix = Vec::new();
    let mut decoder = EventStreamDecoder::new();
    while let Some(chunk_result) = body_stream.next().await {
        match chunk_result {
            Ok(chunk) if !chunk.is_empty() => {
                decoder
                    .feed(&chunk)
                    .map_err(|err| KiroStreamPeekError::Decode(err.to_string()))?;
                buffered_prefix.extend_from_slice(chunk.as_ref());
                let mut decoded_frame = false;
                for frame in decoder.decode_iter() {
                    frame.map_err(|err| KiroStreamPeekError::Decode(err.to_string()))?;
                    decoded_frame = true;
                }
                if decoded_frame {
                    return Ok(KiroPeekedStream {
                        status,
                        buffered_prefix: Bytes::from(buffered_prefix),
                        remaining: body_stream.boxed(),
                    });
                }
            },
            Ok(_) => continue,
            Err(err) => return Err(KiroStreamPeekError::Read(err)),
        }
    }
    if buffered_prefix.is_empty() {
        Err(KiroStreamPeekError::Empty)
    } else {
        Err(KiroStreamPeekError::Incomplete)
    }
}

async fn prepare_kiro_stream_response_for_route(
    initial_response: reqwest::Response,
    route: &ProviderKiroRoute,
    route_store: &dyn ProviderRouteStore,
    upstream_url: &str,
    request_body: &[u8],
    model: &str,
    request_input_tokens: i32,
) -> Result<KiroPeekedStream, KiroRouteFailure> {
    let mut response = initial_response;
    for retry in 0..=KIRO_EMPTY_STREAM_MAX_RETRIES {
        match peek_kiro_stream(response).await {
            Ok(stream) => {
                if kiro_chunk_contains_content_length_exceeded(&stream.buffered_prefix) {
                    return Err(KiroRouteFailure::synthetic(
                        StatusCode::PAYLOAD_TOO_LARGE,
                        kiro_prompt_too_long_message(model, request_input_tokens),
                        KiroRouteFailureKind::Fatal,
                    ));
                }
                if retry > 0 {
                    tracing::info!(
                        model = %model,
                        attempt = retry + 1,
                        "Kiro empty stream retry succeeded"
                    );
                }
                return Ok(stream);
            },
            Err(KiroStreamPeekError::Empty) if retry < KIRO_EMPTY_STREAM_MAX_RETRIES => {
                tracing::warn!(
                    model = %model,
                    attempt = retry + 1,
                    "Kiro returned an empty generateAssistantResponse stream; retrying"
                );
                tokio::time::sleep(Duration::from_millis(200 * (retry as u64 + 1))).await;
                response = call_kiro_generate_for_route(
                    route,
                    route_store,
                    upstream_url.to_string(),
                    request_body,
                )
                .await?;
            },
            Err(KiroStreamPeekError::Empty) => {
                tracing::error!(
                    model = %model,
                    attempts = KIRO_EMPTY_STREAM_MAX_RETRIES + 1,
                    "Kiro returned an empty generateAssistantResponse stream after retries"
                );
                return Err(KiroRouteFailure::synthetic(
                    StatusCode::BAD_GATEWAY,
                    "kiro upstream returned empty generateAssistantResponse stream after retries"
                        .to_string(),
                    KiroRouteFailureKind::RetryNext,
                ));
            },
            Err(KiroStreamPeekError::Incomplete) => {
                tracing::error!(
                    model = %model,
                    "Kiro upstream stream ended before the first complete eventstream frame"
                );
                return Err(KiroRouteFailure::synthetic(
                    StatusCode::BAD_GATEWAY,
                    "kiro upstream ended before the first complete eventstream frame".to_string(),
                    KiroRouteFailureKind::RetryNext,
                ));
            },
            Err(KiroStreamPeekError::Decode(err)) => {
                tracing::error!(
                    model = %model,
                    error = %err,
                    "Failed to decode Kiro upstream stream before sending any response bytes"
                );
                return Err(KiroRouteFailure::synthetic(
                    StatusCode::BAD_GATEWAY,
                    format!("failed to decode kiro upstream stream: {err}"),
                    KiroRouteFailureKind::RetryNext,
                ));
            },
            Err(KiroStreamPeekError::Read(err)) => {
                tracing::error!(
                    model = %model,
                    error = %err,
                    "Failed to read Kiro upstream stream before sending any response bytes"
                );
                return Err(KiroRouteFailure::synthetic(
                    StatusCode::BAD_GATEWAY,
                    format!("failed to read kiro upstream stream: {err}"),
                    KiroRouteFailureKind::RetryNext,
                ));
            },
        }
    }
    unreachable!("bounded kiro empty stream retry loop should return")
}

pub(crate) async fn call_kiro_generate_for_route(
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
    let upstream_url =
        format!("{}/mcp", kiro_refresh::runtime_upstream_base_url(&route.api_region));
    let mut force_refresh = false;
    let mut last_failure: Option<KiroRouteFailure> = None;
    let mut attempt = 0usize;
    let response = loop {
        attempt += 1;
        let call_ctx = match kiro_refresh::ensure_context_for_route_requiring_profile(
            route,
            route_store,
            force_refresh,
        )
        .await
        {
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
    let request_body =
        kiro_request_body_with_profile_arn(request_body, call_ctx.auth.profile_arn.as_deref())?;
    Ok(add_kiro_upstream_headers(
        client.post(&upstream_url),
        &upstream_url,
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
        client.post(&upstream_url),
        &upstream_url,
        call_ctx.auth.profile_arn.as_deref(),
        &call_ctx.access_token,
        Some(&call_ctx.auth),
    )?
    .body(request_body)
    .send()
    .await?)
}

fn kiro_request_body_with_profile_arn(
    request_body: Vec<u8>,
    profile_arn: Option<&str>,
) -> anyhow::Result<Vec<u8>> {
    let mut value: serde_json::Value =
        serde_json::from_slice(&request_body).context("parse kiro request body json")?;
    let Some(object) = value.as_object_mut() else {
        bail!("kiro request body must be a json object");
    };
    if let Some(profile_arn) = profile_arn.map(str::trim).filter(|value| !value.is_empty()) {
        object.insert("profileArn".to_string(), serde_json::Value::String(profile_arn.to_string()));
    } else {
        object.remove("profileArn");
    }
    serde_json::to_vec(&value).context("serialize kiro request body json")
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

async fn should_failover_after_kiro_route_failure(
    failure: &KiroRouteFailure,
    route: &ProviderKiroRoute,
    routes: &[ProviderKiroRoute],
    failed_accounts: &mut HashSet<String>,
    route_store: &dyn ProviderRouteStore,
    scheduler: &KiroRequestScheduler,
) -> bool {
    match failure.kind {
        KiroRouteFailureKind::QuotaExhausted => {
            let error_message = failure.body_text();
            for account_name in
                account_names_for_kiro_routing_identity(routes, &route.routing_identity)
            {
                failed_accounts.insert(account_name.clone());
                let _ = route_store
                    .mark_kiro_account_quota_exhausted(&account_name, &error_message, now_millis())
                    .await;
            }
            has_remaining_kiro_candidate(routes, failed_accounts, &route.account_name)
        },
        KiroRouteFailureKind::RateLimited {
            cooldown,
            mark_proxy,
        } => {
            let error_message = failure.body_text();
            scheduler.mark_account_cooldown(
                &route.routing_identity,
                cooldown,
                error_message.clone(),
            );
            if mark_proxy {
                if let Some(proxy_key) = proxy_cooldown_key_for_route(route) {
                    scheduler.mark_proxy_cooldown(&proxy_key, cooldown, error_message);
                }
            }
            true
        },
        KiroRouteFailureKind::RetryNext => {
            failed_accounts.insert(route.account_name.clone());
            has_remaining_kiro_candidate(routes, failed_accounts, &route.account_name)
        },
        KiroRouteFailureKind::Fatal => false,
    }
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
    projection: RuntimePromptProjection,
    prefix_cache_match: llm_access_kiro::cache_sim::PrefixCacheMatch,
    conversation_id: String,
    cache_kmodels: BTreeMap<String, f64>,
    billable_model_multipliers: BTreeMap<String, f64>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StreamRecordState {
    Pending,
    InternalFailure,
}

struct CodexStreamRecordGuard {
    prepared: PreparedGatewayRequest,
    key: AuthenticatedKey,
    route: ProviderCodexRoute,
    control_store: Arc<dyn ControlStore>,
    status: StatusCode,
    usage_meta: ProviderUsageMetadata,
    usage_collector: SseUsageCollector,
    state: StreamRecordState,
    record_committed: bool,
}

impl CodexStreamRecordGuard {
    fn observe_chunk(&mut self, bytes: &Bytes, event_type: Option<&str>) {
        self.usage_meta
            .observe_stream_write(bytes.len(), event_type);
    }

    fn mark_internal_failure(&mut self) {
        self.state = StreamRecordState::InternalFailure;
    }

    async fn finish_success(mut self) {
        self.usage_meta.mark_post_headers_body();
        self.usage_meta.mark_stream_completed_cleanly();
        let usage = self
            .usage_collector
            .usage
            .clone()
            .unwrap_or_else(missing_codex_usage);
        if let Err(err) = record_codex_usage(
            self.control_store.as_ref(),
            &self.key,
            &self.prepared,
            self.status,
            &self.route,
            usage,
            &self.usage_meta,
        )
        .await
        {
            tracing::warn!(
                key_id = %self.key.key_id,
                account = %self.route.account_name,
                error = %err,
                "failed to record codex stream usage"
            );
        }
        self.record_committed = true;
    }
}

impl Drop for CodexStreamRecordGuard {
    fn drop(&mut self) {
        if self.record_committed {
            return;
        }
        match self.state {
            StreamRecordState::Pending => self.usage_meta.mark_downstream_disconnect(),
            StreamRecordState::InternalFailure => self.usage_meta.mark_stream_internal_incomplete(),
        }
        let control_store = self.control_store.clone();
        let key = self.key.clone();
        let prepared = self.prepared.clone();
        let route = self.route.clone();
        let status = self.status;
        let usage = self
            .usage_collector
            .usage
            .clone()
            .unwrap_or_else(missing_codex_usage);
        let meta = self.usage_meta.clone();
        tokio::spawn(async move {
            if let Err(err) = record_codex_usage(
                control_store.as_ref(),
                &key,
                &prepared,
                status,
                &route,
                usage,
                &meta,
            )
            .await
            {
                tracing::warn!(
                    key_id = %key.key_id,
                    account = %route.account_name,
                    error = %err,
                    "failed to record incomplete codex stream usage"
                );
            }
        });
        self.record_committed = true;
    }
}

struct KiroStreamRecordGuard {
    control_store: Arc<dyn ControlStore>,
    key: AuthenticatedKey,
    route: ProviderKiroRoute,
    endpoint: String,
    model: String,
    status: StatusCode,
    cache_ctx: KiroCacheContext,
    usage_meta: ProviderUsageMetadata,
    stream_ctx: StreamContext,
    state: StreamRecordState,
    record_committed: bool,
}

impl KiroStreamRecordGuard {
    fn observe_chunk(&mut self, bytes: &Bytes, event_type: Option<&str>) {
        self.usage_meta
            .observe_stream_write(bytes.len(), event_type);
    }

    fn mark_internal_failure(&mut self) {
        self.state = StreamRecordState::InternalFailure;
    }

    fn current_usage_summary(&self) -> KiroUsageSummary {
        let (_resolved_input_tokens, output_tokens) = self.stream_ctx.final_usage();
        let (credit_usage, credit_usage_missing) = self.stream_ctx.final_credit_usage();
        build_kiro_usage_summary(
            &self.model,
            KiroUsageInputs {
                request_input_tokens: self.stream_ctx.request_input_tokens(),
                context_input_tokens: self.stream_ctx.context_input_tokens(),
                context_usage_min_request_tokens: self.route.context_usage_min_request_tokens,
                output_tokens,
                credit_usage,
                credit_usage_missing,
                cache_estimation_enabled: self.route.cache_estimation_enabled,
            },
            &self.cache_ctx,
        )
    }

    async fn finish_success(mut self, usage: KiroUsageSummary) {
        self.usage_meta.mark_stream_completed_cleanly();
        if let Err(err) = record_kiro_usage(KiroUsageRecord {
            control_store: self.control_store.as_ref(),
            key: &self.key,
            route: &self.route,
            endpoint: &self.endpoint,
            model: &self.model,
            status: self.status,
            usage,
            cache_ctx: &self.cache_ctx,
            meta: &self.usage_meta,
        })
        .await
        {
            tracing::warn!(
                key_id = %self.key.key_id,
                account = %self.route.account_name,
                error = %err,
                "failed to record kiro stream usage"
            );
        }
        self.record_committed = true;
    }
}

impl Drop for KiroStreamRecordGuard {
    fn drop(&mut self) {
        if self.record_committed {
            return;
        }
        match self.state {
            StreamRecordState::Pending => self.usage_meta.mark_downstream_disconnect(),
            StreamRecordState::InternalFailure => self.usage_meta.mark_stream_internal_incomplete(),
        }
        let control_store = self.control_store.clone();
        let key = self.key.clone();
        let route = self.route.clone();
        let endpoint = self.endpoint.clone();
        let model = self.model.clone();
        let status = self.status;
        let cache_ctx = self.cache_ctx.clone();
        let usage = self.current_usage_summary();
        let meta = self.usage_meta.clone();
        tokio::spawn(async move {
            if let Err(err) = record_kiro_usage(KiroUsageRecord {
                control_store: control_store.as_ref(),
                key: &key,
                route: &route,
                endpoint: &endpoint,
                model: &model,
                status,
                usage,
                cache_ctx: &cache_ctx,
                meta: &meta,
            })
            .await
            {
                tracing::warn!(
                    key_id = %key.key_id,
                    account = %route.account_name,
                    error = %err,
                    "failed to record incomplete kiro stream usage"
                );
            }
        });
        self.record_committed = true;
    }
}

fn stream_kiro_upstream_response(response: KiroPeekedStream, ctx: KiroResponseContext) -> Response {
    let status = response.status;
    let body_stream = stream! {
        let KiroResponseContext {
            key,
            route,
            public_path,
            model,
            request_input_tokens,
            thinking_enabled,
            hidden_thinking_enabled,
            tool_name_map,
            structured_output_tool_name,
            response_identity,
            cache_ctx,
            control_store,
            kiro_cache_simulator,
            usage_meta,
            affinity_update: _affinity_update,
            _key_permit,
            _account_permit,
        } = ctx;
        let stream_model = model.clone();
        let context_usage_min_request_tokens = route.context_usage_min_request_tokens;
        let mut guard = KiroStreamRecordGuard {
            control_store,
            key,
            route,
            endpoint: public_path,
            model,
            status,
            cache_ctx,
            usage_meta,
            stream_ctx: StreamContext::new_with_thinking_visibility(
                &stream_model,
                request_input_tokens,
                thinking_enabled,
                hidden_thinking_enabled,
                tool_name_map,
                structured_output_tool_name,
            )
            .with_context_usage_min_request_tokens(context_usage_min_request_tokens)
            .with_response_identity(response_identity),
            state: StreamRecordState::Pending,
            record_committed: false,
        };
        for event in guard.stream_ctx.generate_initial_events() {
            let bytes = Bytes::from(event.to_sse_string());
            guard.observe_chunk(&bytes, Some(event.event.as_str()));
            yield Ok::<Bytes, std::io::Error>(bytes);
        }
        let mut body_stream = futures_util::stream::once(async move { Ok(response.buffered_prefix) })
            .chain(response.remaining)
            .boxed();
        let mut decoder = EventStreamDecoder::new();
        while let Some(chunk_result) = body_stream.next().await {
            let chunk = match chunk_result {
                Ok(chunk) => chunk,
                Err(err) => {
                    guard.mark_internal_failure();
                    yield Err(std::io::Error::other(format!("failed to read kiro upstream stream: {err}")));
                    return;
                },
            };
            let _ = decoder.feed(&chunk);
            for frame in decoder.decode_iter() {
                let frame = match frame {
                    Ok(frame) => frame,
                    Err(err) => {
                        guard.mark_internal_failure();
                        yield Err(std::io::Error::other(format!("failed to decode kiro event frame: {err}")));
                        return;
                    },
                };
                let event = match Event::from_frame(frame) {
                    Ok(event) => event,
                    Err(err) => {
                        guard.mark_internal_failure();
                        yield Err(std::io::Error::other(format!("failed to parse kiro event: {err}")));
                        return;
                    },
                };
                for sse_event in guard.stream_ctx.process_kiro_event(&event) {
                    let bytes = Bytes::from(sse_event.to_sse_string());
                    guard.observe_chunk(&bytes, Some(sse_event.event.as_str()));
                    yield Ok::<Bytes, std::io::Error>(bytes);
                }
            }
        }
        guard.usage_meta.mark_post_headers_body();
        let (_resolved_input_tokens, output_tokens) = guard.stream_ctx.final_usage();
        let (credit_usage, credit_usage_missing) = guard.stream_ctx.final_credit_usage();
        let usage = build_kiro_usage_summary(
            &guard.model,
            KiroUsageInputs {
                request_input_tokens,
                context_input_tokens: guard.stream_ctx.context_input_tokens(),
                context_usage_min_request_tokens: guard.route.context_usage_min_request_tokens,
                output_tokens,
                credit_usage,
                credit_usage_missing,
                cache_estimation_enabled: guard.route.cache_estimation_enabled,
            },
            &guard.cache_ctx,
        );
        let mut final_events = guard.stream_ctx.generate_final_events();
        let anthropic_usage = anthropic_usage_json_from_summary_with_policy(usage, &guard.cache_ctx);
        for event in &mut final_events {
            if event.event == "message_delta" {
                if let Some(value) = event.data.get_mut("usage") {
                    *value = anthropic_usage.clone();
                }
            }
        }
        let assistant_message = guard.stream_ctx.final_assistant_message();
        kiro_cache_simulator.record_success_from_runtime_projection(
            &guard.cache_ctx.projection,
            &assistant_message,
            &guard.cache_ctx.conversation_id,
            guard.route.cache_estimation_enabled,
            guard.cache_ctx.simulation_config,
            Instant::now(),
        );
        for event in final_events {
            let bytes = Bytes::from(event.to_sse_string());
            guard.observe_chunk(&bytes, Some(event.event.as_str()));
            yield Ok::<Bytes, std::io::Error>(bytes);
        }
        guard.finish_success(usage).await;
    };
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONNECTION, "keep-alive")
        .body(Body::from_stream(body_stream))
        .unwrap_or_else(|_| {
            kiro_json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "api_error",
                "failed to build stream response",
            )
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
    if kiro_events_contain_content_length_exceeded(&events) {
        let status = StatusCode::PAYLOAD_TOO_LARGE;
        let message = kiro_prompt_too_long_message(&ctx.model, ctx.request_input_tokens);
        let response = anthropic_json_error(status, "invalid_request_error", &message);
        capture_error_message(&mut usage_meta, &message);
        capture_error_body(
            &mut usage_meta,
            &anthropic_json_error_body("invalid_request_error", &message),
        );
        let usage = build_kiro_usage_summary(
            &ctx.model,
            KiroUsageInputs {
                request_input_tokens: ctx.request_input_tokens,
                context_input_tokens: None,
                context_usage_min_request_tokens: ctx.route.context_usage_min_request_tokens,
                output_tokens: 0,
                credit_usage: None,
                credit_usage_missing: true,
                cache_estimation_enabled: false,
            },
            &ctx.cache_ctx,
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
            tracing::error!(
                error = %err,
                "Failed to record gateway usage for non-stream content length exception"
            );
            return kiro_json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "api_error",
                "failed to record usage",
            );
        }
        return response;
    }
    let mut stream_ctx = StreamContext::new_with_thinking_visibility(
        &ctx.model,
        ctx.request_input_tokens,
        ctx.thinking_enabled,
        ctx.hidden_thinking_enabled,
        ctx.tool_name_map,
        ctx.structured_output_tool_name.clone(),
    )
    .with_context_usage_min_request_tokens(ctx.route.context_usage_min_request_tokens)
    .with_response_identity(ctx.response_identity.clone());
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
            context_usage_min_request_tokens: ctx.route.context_usage_min_request_tokens,
            output_tokens,
            credit_usage,
            credit_usage_missing,
            cache_estimation_enabled: ctx.route.cache_estimation_enabled,
        },
        &ctx.cache_ctx,
    );
    let assistant_message = stream_ctx.final_assistant_message();
    let mut content = stream_ctx.final_content_blocks();
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
    ctx.kiro_cache_simulator
        .record_success_from_runtime_projection(
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
        tracing::error!(error = %err, "Failed to record gateway usage for non-stream response");
        return kiro_json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "api_error",
            "failed to record usage",
        );
    }
    if let Some(affinity_update) = &ctx.affinity_update {
        affinity_update.affinity.remember(
            &ctx.key.key_id,
            &affinity_update.session_id,
            &ctx.route.account_name,
        );
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
            kiro_json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "api_error",
                "failed to build response",
            )
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
    context_usage_min_request_tokens: u64,
    output_tokens: i32,
    credit_usage: Option<f64>,
    credit_usage_missing: bool,
    cache_estimation_enabled: bool,
}

fn normalized_kiro_messages_path(path: &str) -> Option<&'static str> {
    match path {
        "/cc/v1/messages" | "/api/kiro-gateway/cc/v1/messages" => Some("/cc/v1/messages"),
        "/v1/messages" | "/api/kiro-gateway/v1/messages" => Some("/v1/messages"),
        _ => None,
    }
}

fn add_kiro_upstream_headers(
    upstream: reqwest::RequestBuilder,
    upstream_url: &str,
    access_token: &str,
    auth_record: Option<&KiroAuthRecord>,
) -> anyhow::Result<reqwest::RequestBuilder> {
    let auth = auth_record.ok_or_else(|| anyhow::anyhow!("invalid kiro auth record"))?;
    let host = kiro_refresh::upstream_host_header(upstream_url)?;
    kiro_headers::add_kiro_headers(upstream, auth, kiro_headers::KiroHeaderConfig {
        upstream_host: &host,
        access_token,
        service: kiro_headers::KiroAwsService::Streaming,
        client_version: KIRO_PROVIDER_AWS_SDK_VERSION,
        sdk_request: "attempt=1; max=3",
        content_type: Some("application/json"),
        accept: Some("application/vnd.amazon.eventstream"),
        connection_close: false,
        agent_mode: Some("vibe"),
        include_opt_out: true,
    })
}

fn add_kiro_mcp_headers(
    mut upstream: reqwest::RequestBuilder,
    upstream_url: &str,
    profile_arn: Option<&str>,
    access_token: &str,
    auth_record: Option<&KiroAuthRecord>,
) -> anyhow::Result<reqwest::RequestBuilder> {
    let auth = auth_record.ok_or_else(|| anyhow::anyhow!("invalid kiro auth record"))?;
    let host = kiro_refresh::upstream_host_header(upstream_url)?;
    upstream = kiro_headers::add_kiro_headers(upstream, auth, kiro_headers::KiroHeaderConfig {
        upstream_host: &host,
        access_token,
        service: kiro_headers::KiroAwsService::Streaming,
        client_version: KIRO_PROVIDER_AWS_SDK_VERSION,
        sdk_request: "attempt=1; max=3",
        content_type: Some("application/json"),
        accept: None,
        connection_close: false,
        agent_mode: None,
        include_opt_out: false,
    })?;
    if let Some(profile_arn) = profile_arn.map(str::trim).filter(|value| !value.is_empty()) {
        upstream = upstream.header("x-amzn-kiro-profile-arn", profile_arn);
    }
    Ok(upstream)
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ProviderClientCacheKey {
    proxy_url: String,
    proxy_username: Option<String>,
    proxy_password: Option<String>,
}

static DEFAULT_PROVIDER_CLIENT: std::sync::LazyLock<reqwest::Client> =
    std::sync::LazyLock::new(|| {
        build_provider_client(None).expect("default provider client should build")
    });
static PROVIDER_CLIENT_CACHE: std::sync::LazyLock<
    Mutex<LruCache<ProviderClientCacheKey, reqwest::Client>>,
> = std::sync::LazyLock::new(|| Mutex::new(LruCache::new(provider_client_cache_capacity())));
static KIRO_REMOTE_MEDIA_CLIENT: std::sync::LazyLock<reqwest::Client> =
    std::sync::LazyLock::new(|| {
        reqwest::Client::builder()
            .timeout(KIRO_REMOTE_MEDIA_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .pool_idle_timeout(provider_client_pool_idle_timeout())
            .pool_max_idle_per_host(provider_client_pool_max_idle_per_host())
            .tcp_keepalive(Duration::from_secs(30))
            .build()
            .expect("kiro remote media client should build")
    });

fn build_provider_client(proxy: Option<&ProviderProxyConfig>) -> anyhow::Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .pool_idle_timeout(provider_client_pool_idle_timeout())
        .pool_max_idle_per_host(provider_client_pool_max_idle_per_host())
        .tcp_keepalive(Duration::from_secs(30));
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

fn provider_client(proxy: Option<&ProviderProxyConfig>) -> anyhow::Result<reqwest::Client> {
    let Some(proxy_config) = proxy else {
        return Ok(DEFAULT_PROVIDER_CLIENT.clone());
    };
    let cache_key = ProviderClientCacheKey {
        proxy_url: proxy_config.proxy_url.clone(),
        proxy_username: proxy_config.proxy_username.clone(),
        proxy_password: proxy_config.proxy_password.clone(),
    };
    {
        let mut cache = PROVIDER_CLIENT_CACHE
            .lock()
            .expect("provider client cache lock");
        if let Some(client) = cache.get(&cache_key).cloned() {
            return Ok(client);
        }
    }
    let client = build_provider_client(Some(proxy_config))?;
    PROVIDER_CLIENT_CACHE
        .lock()
        .expect("provider client cache lock")
        .put(cache_key, client.clone());
    Ok(client)
}

fn provider_client_cache_capacity() -> NonZeroUsize {
    let capacity = std::env::var("LLM_ACCESS_PROVIDER_CLIENT_CACHE_CAPACITY")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .map(|value| value.clamp(1, MAX_PROVIDER_CLIENT_CACHE_CAPACITY))
        .unwrap_or(DEFAULT_PROVIDER_CLIENT_CACHE_CAPACITY);
    NonZeroUsize::new(capacity).expect("provider client cache capacity is non-zero")
}

fn provider_client_pool_idle_timeout() -> Duration {
    let seconds = std::env::var("LLM_ACCESS_PROVIDER_CLIENT_POOL_IDLE_TIMEOUT_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(|value| {
            value.clamp(
                MIN_PROVIDER_CLIENT_POOL_IDLE_TIMEOUT_SECONDS,
                MAX_PROVIDER_CLIENT_POOL_IDLE_TIMEOUT_SECONDS,
            )
        })
        .unwrap_or(DEFAULT_PROVIDER_CLIENT_POOL_IDLE_TIMEOUT_SECONDS);
    Duration::from_secs(seconds)
}

fn provider_client_pool_max_idle_per_host() -> usize {
    std::env::var("LLM_ACCESS_PROVIDER_CLIENT_POOL_MAX_IDLE_PER_HOST")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .map(|value| value.min(MAX_PROVIDER_CLIENT_POOL_MAX_IDLE_PER_HOST))
        .unwrap_or(DEFAULT_PROVIDER_CLIENT_POOL_MAX_IDLE_PER_HOST)
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

fn anthropic_json_error_body(error_type: &str, message: &str) -> String {
    serde_json::json!({
        "error": {
            "type": error_type,
            "message": message,
        }
    })
    .to_string()
}

fn anthropic_json_error(status: StatusCode, error_type: &str, message: &str) -> Response {
    let body = anthropic_json_error_body(error_type, message);
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .unwrap_or_else(|_| {
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to build error").into_response()
        })
}

fn codex_error_type_for_status(status: StatusCode) -> &'static str {
    if status.is_client_error() {
        "invalid_request_error"
    } else {
        "api_error"
    }
}

fn codex_json_error_body(status: StatusCode, message: &str) -> String {
    json!({
        "error": {
            "message": message,
            "type": codex_error_type_for_status(status),
            "param": Value::Null,
            "code": Value::Null,
        }
    })
    .to_string()
}

fn codex_json_error(status: StatusCode, message: &str) -> Response {
    let body = codex_json_error_body(status, message);
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .unwrap_or_else(|_| {
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to build error").into_response()
        })
}

fn codex_endpoint_prefers_anthropic_errors(endpoint: &str) -> bool {
    endpoint == "/v1/messages" || endpoint.starts_with("/v1/messages?")
}

fn codex_surface_error_body(endpoint: &str, status: StatusCode, message: &str) -> String {
    if codex_endpoint_prefers_anthropic_errors(endpoint) {
        anthropic_json_error_body(codex_error_type_for_status(status), message)
    } else {
        codex_json_error_body(status, message)
    }
}

fn codex_surface_error_response(endpoint: &str, status: StatusCode, message: &str) -> Response {
    if codex_endpoint_prefers_anthropic_errors(endpoint) {
        anthropic_json_error(status, codex_error_type_for_status(status), message)
    } else {
        codex_json_error(status, message)
    }
}

fn extract_error_message_from_json_value(value: &Value) -> Option<String> {
    if let Some(message) = value.get("error").and_then(Value::as_str) {
        return Some(message.to_string());
    }
    if let Some(error) = value.get("error").and_then(Value::as_object) {
        if let Some(message) = error.get("message").and_then(Value::as_str) {
            return Some(message.to_string());
        }
    }
    if let Some(message) = value
        .pointer("/response/error/message")
        .and_then(Value::as_str)
        .map(ToString::to_string)
    {
        return Some(message);
    }
    value
        .get("message")
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn summarize_error_bytes(bytes: &Bytes) -> String {
    if let Ok(value) = serde_json::from_slice::<Value>(bytes.as_ref()) {
        if let Some(message) = extract_error_message_from_json_value(&value)
            .map(|message| message.trim().to_string())
            .filter(|message| !message.is_empty())
        {
            return message;
        }
    }
    let body = String::from_utf8_lossy(bytes.as_ref()).trim().to_string();
    if body.is_empty() {
        "Unknown upstream error".to_string()
    } else {
        body
    }
}

fn kiro_prompt_too_long_message(model: &str, request_input_tokens: i32) -> String {
    let limit_tokens = get_context_window_size(model).max(1);
    let actual_tokens = request_input_tokens.max(limit_tokens.saturating_add(1));
    format!(
        "Prompt is too long: {actual_tokens} tokens > {limit_tokens} tokens for the model context \
         window."
    )
}

fn kiro_prompt_too_long_response_for_body(
    status: StatusCode,
    bytes: &Bytes,
    model: &str,
    request_input_tokens: i32,
) -> Option<Response> {
    if status != StatusCode::PAYLOAD_TOO_LARGE && !kiro_body_is_content_length_exceeded(bytes) {
        return None;
    }
    let message = kiro_prompt_too_long_message(model, request_input_tokens);
    Some(anthropic_json_error(StatusCode::PAYLOAD_TOO_LARGE, "invalid_request_error", &message))
}

fn kiro_body_is_content_length_exceeded(bytes: &Bytes) -> bool {
    kiro_text_is_content_length_exceeded(&String::from_utf8_lossy(bytes.as_ref()))
}

fn kiro_events_contain_content_length_exceeded(events: &[Event]) -> bool {
    events.iter().any(kiro_event_is_content_length_exceeded)
}

fn kiro_chunk_contains_content_length_exceeded(chunk: &Bytes) -> bool {
    let mut decoder = EventStreamDecoder::new();
    let _ = decoder.feed(chunk);
    decoder.decode_iter().any(|result| {
        let Ok(frame) = result else {
            return false;
        };
        Event::from_frame(frame)
            .ok()
            .as_ref()
            .is_some_and(kiro_event_is_content_length_exceeded)
    })
}

fn kiro_event_is_content_length_exceeded(event: &Event) -> bool {
    match event {
        Event::Error {
            error_code,
            error_message,
        } => {
            kiro_text_is_content_length_exceeded(error_code)
                || kiro_text_is_content_length_exceeded(error_message)
        },
        Event::Exception {
            exception_type,
            message,
        } => {
            kiro_text_is_content_length_exceeded(exception_type)
                || kiro_text_is_content_length_exceeded(message)
        },
        _ => false,
    }
}

fn kiro_text_is_content_length_exceeded(text: &str) -> bool {
    let normalized = text.to_ascii_lowercase();
    normalized.contains("content_length_exceeds_threshold")
        || normalized.contains("contentlengthexceededexception")
        || normalized.contains("input content length exceeds threshold")
        || normalized.contains("input is too long")
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

fn override_kiro_thinking_from_model_name(payload: &mut MessagesRequest) {
    let model = payload.model.to_lowercase();
    if !model.contains("thinking") {
        return;
    }
    let is_high_reasoning_opus = model.contains("opus")
        && (model.contains("4-6")
            || model.contains("4.6")
            || model.contains("4-7")
            || model.contains("4.7")
            || model.contains("4-8")
            || model.contains("4.8"));
    payload.thinking = Some(Thinking {
        thinking_type: if is_high_reasoning_opus {
            "adaptive".to_string()
        } else {
            "enabled".to_string()
        },
        display: None,
        budget_tokens: 20_000,
    });
    if is_high_reasoning_opus {
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

fn kiro_affinity_session_id(resolved_session: &ResolvedConversationId) -> Option<&str> {
    if matches!(resolved_session.session_tracking.source, SessionIdSource::GeneratedFallback(_)) {
        return None;
    }
    let session_id = resolved_session.conversation_id.trim();
    (!session_id.is_empty()).then_some(session_id)
}

fn remember_kiro_session_affinity(
    affinity: &KiroSessionAffinity,
    key_id: &str,
    session_id: Option<&str>,
    account_name: &str,
) {
    let Some(session_id) = session_id else {
        return;
    };
    affinity.remember(key_id, session_id, account_name);
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
    let projection = RuntimePromptProjection::from_conversation_state(conversation_state);
    let prefix_cache_match = if route.cache_estimation_enabled
        && simulation_config.mode == KiroCacheSimulationMode::PrefixTree
    {
        cache_simulator.match_prefix_from_runtime_projection(
            &projection,
            simulation_config,
            Instant::now(),
        )
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

pub(crate) fn decode_kiro_events_from_bytes(bytes: &[u8]) -> Result<Vec<Event>, String> {
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
    let (resolved_input_tokens, _) = resolve_input_tokens_with_threshold(
        usage.request_input_tokens,
        usage.context_input_tokens,
        usage.context_usage_min_request_tokens,
    );
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
    let projected_total = cache_ctx.projection.projected_input_token_count().max(1);
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
        "claude-opus-4.7" => "claude-opus-4-7",
        "claude-opus-4.8" => "claude-opus-4-8",
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
        .then(|| captured_body_json(&record.meta.client_request_body_json))
        .flatten();
    let upstream_request_body_json = capture_request_details
        .then(|| captured_body_json(&record.meta.upstream_request_body_json))
        .flatten();
    let full_request_json = capture_request_details
        .then(|| {
            captured_body_json(&record.meta.full_request_json)
                .or_else(|| captured_body_json(&record.meta.client_request_body_json))
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
        error_message: record.meta.error_message.clone(),
        error_body: record.meta.error_body.clone(),
        timing: record.meta.to_timing(),
        stream: record.meta.to_stream_details(),
    };
    record.control_store.apply_usage_rollup_owned(event).await
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
    let capture_request_details = record.capture_request_details || !record.status.is_success();
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
        client_request_body_json: capture_request_details
            .then(|| captured_body_json(&record.meta.client_request_body_json))
            .flatten(),
        upstream_request_body_json: capture_request_details
            .then(|| captured_body_json(&record.meta.upstream_request_body_json))
            .flatten(),
        full_request_json: capture_request_details
            .then(|| {
                captured_body_json(&record.meta.full_request_json)
                    .or_else(|| captured_body_json(&record.meta.client_request_body_json))
            })
            .flatten(),
        error_message: record.meta.error_message.clone(),
        error_body: record.meta.error_body.clone(),
        timing: record.meta.to_timing(),
        stream: record.meta.to_stream_details(),
    };
    record.control_store.apply_usage_rollup_owned(event).await
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
    if accepts_anthropic_api_key_header(path) {
        x_api_key_secret(headers).or_else(|| bearer_secret(headers))
    } else {
        bearer_secret(headers)
    }
}

fn accepts_anthropic_api_key_header(path: &str) -> bool {
    path == "/v1/models"
        || is_kiro_data_plane_route(path)
        || is_codex_anthropic_messages_route(path)
}

fn is_kiro_data_plane_route(path: &str) -> bool {
    provider_route_requirement(path)
        .map(|requirement| requirement.provider_type == ProviderType::Kiro)
        .unwrap_or(false)
}

fn is_codex_anthropic_messages_route(path: &str) -> bool {
    normalized_codex_gateway_path(path) == Some("/v1/messages")
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
    if path == "/v1/models" {
        return true;
    }
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
        kiro_json_error(StatusCode::PAYMENT_REQUIRED, "rate_limit_error", "key quota exhausted")
    } else {
        (StatusCode::TOO_MANY_REQUESTS, "quota_exceeded").into_response()
    }
}

fn normalized_codex_gateway_path(path: &str) -> Option<&str> {
    if matches!(path, "/v1/models" | "/v1/messages") {
        return Some(path);
    }
    if path == "/v1/chat/completions"
        || path == "/v1/responses"
        || path.starts_with("/v1/responses/")
    {
        return Some(path);
    }
    let alias = path
        .strip_prefix("/api/llm-gateway")
        .or_else(|| path.strip_prefix("/api/codex-gateway"))?;
    match alias {
        "/models" | "/v1/models" => Some("/v1/models"),
        "/chat/completions" | "/v1/chat/completions" => Some("/v1/chat/completions"),
        "/responses" | "/v1/responses" => Some("/v1/responses"),
        "/responses/compact" | "/v1/responses/compact" => Some("/v1/responses/compact"),
        "/messages" | "/v1/messages" => Some("/v1/messages"),
        value if value.starts_with("/v1/responses/") => Some(value),
        _ => None,
    }
}

fn codex_protocol_family_for_endpoint(endpoint: &str) -> ProtocolFamily {
    if endpoint == "/v1/messages" || endpoint.starts_with("/v1/messages?") {
        ProtocolFamily::Anthropic
    } else {
        ProtocolFamily::OpenAi
    }
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

pub(crate) fn compute_codex_upstream_url(base: &str, path: &str) -> String {
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

fn resolve_codex_account_attempt_limit(raw: u64) -> usize {
    usize::try_from(raw).unwrap_or(usize::MAX).max(1)
}

async fn load_codex_dispatch_runtime_config(
    admin_config_store: &dyn AdminConfigStore,
) -> Result<CodexDispatchRuntimeConfig, Response> {
    match admin_config_store.get_admin_runtime_config().await {
        Ok(config) => Ok(CodexDispatchRuntimeConfig {
            client_version: resolve_codex_client_version(Some(&config.codex_client_version)),
            account_attempt_limit: resolve_codex_account_attempt_limit(
                config.account_failure_retry_limit,
            ),
        }),
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

fn first_header_value(headers: &HeaderMap, names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| header_value(headers, name))
}

#[derive(Debug, Default)]
struct CodexTurnMetadataHeader {
    session_id: Option<String>,
    thread_id: Option<String>,
}

fn parse_codex_turn_metadata_header(headers: &HeaderMap) -> CodexTurnMetadataHeader {
    let Some(raw) = header_value(headers, "x-codex-turn-metadata") else {
        return CodexTurnMetadataHeader::default();
    };
    let Ok(value) = serde_json::from_str::<Value>(&raw) else {
        return CodexTurnMetadataHeader::default();
    };
    CodexTurnMetadataHeader {
        session_id: json_string_field(&value, "session_id"),
        thread_id: json_string_field(&value, "thread_id"),
    }
}

fn json_string_field(value: &Value, name: &str) -> Option<String> {
    value
        .get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn is_standard_codex_responses_path(prepared: &PreparedGatewayRequest) -> bool {
    prepared
        .upstream_path
        .split('?')
        .next()
        .is_some_and(|path| path == "/v1/responses")
}

#[derive(Debug, Default)]
struct CodexUpstreamSessionHeaders {
    conversation_id: Option<String>,
    session_id: Option<String>,
    thread_id: Option<String>,
    client_request_id: Option<String>,
}

fn resolve_codex_upstream_session_headers(
    request_headers: &HeaderMap,
    prepared: &PreparedGatewayRequest,
) -> CodexUpstreamSessionHeaders {
    let metadata = parse_codex_turn_metadata_header(request_headers);
    let thread_anchor = prepared
        .thread_anchor
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let should_reconstruct = is_standard_codex_responses_path(prepared);
    let session_id =
        first_header_value(request_headers, &["session_id", "session-id"]).or_else(|| {
            if should_reconstruct {
                metadata
                    .session_id
                    .clone()
                    .or_else(|| thread_anchor.map(ToString::to_string))
            } else {
                None
            }
        });
    let thread_id =
        first_header_value(request_headers, &["thread_id", "thread-id"]).or_else(|| {
            if should_reconstruct {
                metadata
                    .thread_id
                    .clone()
                    .or_else(|| thread_anchor.map(ToString::to_string))
                    .or_else(|| session_id.clone())
            } else {
                None
            }
        });
    let conversation_id = header_value(request_headers, "conversation_id").or_else(|| {
        if should_reconstruct {
            thread_anchor
                .map(ToString::to_string)
                .or_else(|| metadata.thread_id.clone())
        } else {
            None
        }
    });
    let client_request_id = header_value(request_headers, "x-client-request-id").or_else(|| {
        if should_reconstruct {
            thread_id
                .clone()
                .or_else(|| thread_anchor.map(ToString::to_string))
        } else {
            None
        }
    });

    CodexUpstreamSessionHeaders {
        conversation_id,
        session_id,
        thread_id,
        client_request_id,
    }
}

fn is_codex_invalid_encrypted_content_response(status: StatusCode, bytes: &Bytes) -> bool {
    if status != StatusCode::BAD_REQUEST {
        return false;
    }
    if codex_error_code_from_bytes(bytes).as_deref() == Some("invalid_encrypted_content") {
        return true;
    }
    std::str::from_utf8(bytes.as_ref())
        .map(|body| body.contains("invalid_encrypted_content"))
        .unwrap_or(false)
}

fn is_codex_non_retryable_client_error_response(status: StatusCode, bytes: &Bytes) -> bool {
    if status != StatusCode::BAD_REQUEST
        || is_codex_invalid_encrypted_content_response(status, bytes)
    {
        return false;
    }

    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
        return false;
    };
    let error = value.get("error").unwrap_or(&value);
    if json_string_field(error, "code")
        .as_deref()
        .is_some_and(codex_error_code_is_request_shape_failure)
    {
        return true;
    }

    extract_error_message_from_json_value(&value)
        .as_deref()
        .is_some_and(codex_message_indicates_request_shape_failure)
}

fn codex_error_code_is_request_shape_failure(code: &str) -> bool {
    matches!(
        code,
        "invalid_value"
            | "unsupported_value"
            | "invalid_type"
            | "missing_required_parameter"
            | "unknown_parameter"
            | "unsupported_parameter"
    )
}

fn codex_message_indicates_request_shape_failure(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    (normalized.contains("invalid value") && normalized.contains("supported values"))
        || normalized.contains("invalid type")
        || normalized.contains("missing required parameter")
        || normalized.contains("unknown parameter")
        || normalized.contains("unsupported parameter")
}

fn codex_error_code_from_bytes(bytes: &Bytes) -> Option<String> {
    serde_json::from_slice::<Value>(bytes)
        .ok()
        .and_then(|value| codex_error_code_from_value(&value))
}

fn codex_error_code_from_value(value: &Value) -> Option<String> {
    let error = value.get("error").unwrap_or(value);
    if let Some(code) = json_string_field(error, "code") {
        return Some(code);
    }
    let message = json_string_field(error, "message")?;
    serde_json::from_str::<Value>(&message)
        .ok()
        .and_then(|nested| codex_error_code_from_value(&nested))
}

fn retry_codex_without_encrypted_reasoning(
    prepared: &PreparedGatewayRequest,
) -> Option<PreparedGatewayRequest> {
    let mut value = serde_json::from_slice::<Value>(&prepared.request_body).ok()?;
    let root = value.as_object_mut()?;
    if !strip_codex_encrypted_reasoning_items(root) {
        return None;
    }
    let request_body = Bytes::from(serde_json::to_vec(&value).ok()?);
    let mut retry = prepared.clone();
    retry.request_body = request_body;
    Some(retry)
}

fn strip_codex_encrypted_reasoning_items(root: &mut serde_json::Map<String, Value>) -> bool {
    let Some(input) = root.get_mut("input") else {
        return false;
    };
    let mut remove_input = false;
    let changed = match input {
        Value::Array(items) => {
            let mut changed = false;
            let mut filtered = Vec::with_capacity(items.len());
            for mut item in std::mem::take(items) {
                let keep = sanitize_codex_encrypted_reasoning_item(&mut item, &mut changed);
                if keep {
                    filtered.push(item);
                }
            }
            if changed {
                if filtered.is_empty() {
                    remove_input = true;
                } else {
                    *items = filtered;
                }
            }
            changed
        },
        Value::Object(_) => {
            let mut changed = false;
            let keep = sanitize_codex_encrypted_reasoning_item(input, &mut changed);
            if changed && !keep {
                remove_input = true;
            }
            changed
        },
        _ => false,
    };
    if remove_input {
        root.remove("input");
    }
    changed
}

fn sanitize_codex_encrypted_reasoning_item(item: &mut Value, changed: &mut bool) -> bool {
    let Some(obj) = item.as_object_mut() else {
        return true;
    };
    if obj.get("type").and_then(Value::as_str) != Some("reasoning") {
        return true;
    }
    if obj.remove("encrypted_content").is_none() {
        return true;
    }
    *changed = true;
    obj.len() > 1
}

fn add_codex_upstream_headers(
    mut upstream: reqwest::RequestBuilder,
    request_headers: &HeaderMap,
    prepared: &PreparedGatewayRequest,
    auth: &CodexAuthSnapshot,
    codex_client_version: &str,
) -> reqwest::RequestBuilder {
    let session_headers = resolve_codex_upstream_session_headers(request_headers, prepared);
    let incoming_turn_state = header_value(request_headers, "x-codex-turn-state");

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
    if let Some(conversation_id) = session_headers.conversation_id.as_deref() {
        upstream = upstream.header("conversation_id", conversation_id);
    }
    if let Some(client_request_id) = session_headers.client_request_id.as_deref() {
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
    if let Some(session_id) = session_headers.session_id.as_deref() {
        upstream = upstream
            .header("session_id", session_id)
            .header("session-id", session_id);
    }
    if let Some(thread_id) = session_headers.thread_id.as_deref() {
        upstream = upstream
            .header("thread_id", thread_id)
            .header("thread-id", thread_id);
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

struct CompletedCodexSseError {
    status: StatusCode,
    message: String,
    body: Option<String>,
}

#[derive(Default)]
struct CompletedCodexSseAccumulator {
    response: Option<Value>,
    usage: Option<UsageBreakdown>,
    output_items: BTreeMap<u64, Value>,
    delta_text: String,
    done_text: Option<String>,
    fallback_item_id: Option<String>,
    failure: Option<Value>,
}

impl CompletedCodexSseAccumulator {
    fn observe_payload(
        &mut self,
        event_type: Option<&str>,
        data: &str,
    ) -> Result<(), &'static str> {
        let mut value =
            serde_json::from_str::<Value>(data).map_err(|_| "invalid codex upstream SSE JSON")?;
        if let (Some(event_type), Some(object)) = (event_type, value.as_object_mut()) {
            object
                .entry("type")
                .or_insert_with(|| Value::String(event_type.to_string()));
        }
        if let Some(observed_usage) = extract_usage_from_bytes(data.as_bytes()) {
            self.usage = Some(observed_usage);
        }
        self.capture_failure(&value);

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

    fn capture_failure(&mut self, value: &Value) {
        if self.failure.is_some() || self.response.is_some() {
            return;
        }
        let event_type = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let looks_like_failure = matches!(
            event_type,
            "error" | "response.error" | "response.failed" | "response.incomplete"
        ) || value.pointer("/response/error").is_some()
            || value.get("error").is_some();
        if looks_like_failure
            && extract_error_message_from_json_value(value)
                .map(|message| !message.trim().is_empty())
                .unwrap_or(false)
        {
            self.failure = Some(value.clone());
        }
    }

    fn capture_fallback_item_id(&mut self, value: &Value) {
        if self.fallback_item_id.is_none() {
            self.fallback_item_id = value
                .get("item_id")
                .and_then(Value::as_str)
                .map(ToString::to_string);
        }
    }

    fn finish(mut self) -> Result<CompletedCodexSse, CompletedCodexSseError> {
        let Some(mut response) = self.response.take() else {
            if let Some(failure) = self.failure.as_ref() {
                return Err(completed_codex_sse_error_from_value(failure));
            }
            return Err(CompletedCodexSseError {
                status: StatusCode::BAD_GATEWAY,
                message: "codex upstream SSE stream did not include response.completed".to_string(),
                body: None,
            });
        };
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

fn completed_codex_sse_error_from_value(value: &Value) -> CompletedCodexSseError {
    let message = extract_error_message_from_json_value(value)
        .map(|message| message.trim().to_string())
        .filter(|message| !message.is_empty())
        .unwrap_or_else(|| "Unknown upstream error".to_string());
    let status = codex_status_from_error_json_value(value).unwrap_or(StatusCode::BAD_GATEWAY);
    CompletedCodexSseError {
        status,
        message,
        body: Some(value.to_string()),
    }
}

fn completed_response_from_sse_bytes(
    bytes: &[u8],
) -> Result<CompletedCodexSse, CompletedCodexSseError> {
    let mut accumulator = CompletedCodexSseAccumulator::default();
    for payload in sse_payloads(bytes) {
        let data = payload.data;
        if data.trim() == "[DONE]" {
            continue;
        }
        accumulator
            .observe_payload(payload.event.as_deref(), &data)
            .map_err(|message| CompletedCodexSseError {
                status: StatusCode::BAD_GATEWAY,
                message: message.to_string(),
                body: None,
            })?;
    }
    accumulator.finish()
}

struct SsePayload {
    event: Option<String>,
    data: String,
}

fn sse_payloads(bytes: &[u8]) -> Vec<SsePayload> {
    let text = String::from_utf8_lossy(bytes).replace("\r\n", "\n");
    text.split("\n\n")
        .filter_map(|event| {
            let event_type = event.lines().find_map(|line| {
                line.strip_prefix("event:")
                    .map(|value| value.strip_prefix(' ').unwrap_or(value).trim())
                    .filter(|value| !value.is_empty())
                    .map(ToString::to_string)
            });
            let data = event
                .lines()
                .filter_map(|line| line.strip_prefix("data:"))
                .map(|line| line.strip_prefix(' ').unwrap_or(line))
                .collect::<Vec<_>>();
            if data.is_empty() {
                None
            } else {
                Some(SsePayload {
                    event: event_type,
                    data: data.join("\n"),
                })
            }
        })
        .collect()
}

struct CodexPreflightFailureRecord<'a> {
    control_store: &'a dyn ControlStore,
    key: &'a AuthenticatedKey,
    endpoint: &'a str,
    model: Option<String>,
    status: StatusCode,
    meta: &'a mut ProviderUsageMetadata,
}

async fn record_codex_preflight_failure(record: CodexPreflightFailureRecord<'_>) {
    record.meta.mark_stream_finish();
    let event = UsageEvent {
        event_id: format!("llm-usage-{}", uuid::Uuid::new_v4()),
        created_at_ms: now_millis(),
        provider_type: ProviderType::Codex,
        protocol_family: codex_protocol_family_for_endpoint(record.endpoint),
        key_id: record.key.key_id.clone(),
        key_name: record.key.key_name.clone(),
        account_name: None,
        account_group_id_at_event: None,
        route_strategy_at_event: None,
        request_method: record.meta.request_method.clone(),
        request_url: record.meta.request_url.clone(),
        endpoint: record.endpoint.to_string(),
        model: record.model,
        mapped_model: None,
        status_code: i64::from(record.status.as_u16()),
        request_body_bytes: record.meta.request_body_bytes,
        quota_failover_count: record.meta.quota_failover_count,
        routing_diagnostics_json: record.meta.routing_diagnostics_json.clone(),
        input_uncached_tokens: 0,
        input_cached_tokens: 0,
        output_tokens: 0,
        billable_tokens: 0,
        credit_usage: None,
        usage_missing: true,
        credit_usage_missing: false,
        client_ip: record.meta.client_ip.clone(),
        ip_region: record.meta.ip_region.clone(),
        request_headers_json: record.meta.request_headers_json.clone(),
        last_message_content: record.meta.last_message_content.clone(),
        client_request_body_json: captured_body_json(&record.meta.client_request_body_json),
        upstream_request_body_json: captured_body_json(&record.meta.upstream_request_body_json),
        full_request_json: captured_body_json(&record.meta.full_request_json),
        error_message: record.meta.error_message.clone(),
        error_body: record.meta.error_body.clone(),
        timing: record.meta.to_timing(),
        stream: record.meta.to_stream_details(),
    };
    if let Err(err) = record.control_store.apply_usage_rollup_owned(event).await {
        tracing::warn!(
            key_id = %record.key.key_id,
            endpoint = record.endpoint,
            status = %record.status,
            error = %err,
            "failed to record codex preflight failure usage"
        );
    }
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
    let capture_request_details = !status.is_success();
    let event = UsageEvent {
        event_id: format!("llm-usage-{}", uuid::Uuid::new_v4()),
        created_at_ms: now_millis(),
        provider_type: ProviderType::Codex,
        protocol_family: codex_protocol_family_for_endpoint(&prepared.original_path),
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
            .or(Some(clamp_usize_to_i64(prepared.request_body.len()))),
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
        client_request_body_json: capture_request_details
            .then(|| captured_body_json(&meta.client_request_body_json))
            .flatten(),
        upstream_request_body_json: capture_request_details
            .then(|| captured_body_json(&meta.upstream_request_body_json))
            .flatten(),
        full_request_json: capture_request_details
            .then(|| {
                captured_body_json(&meta.full_request_json)
                    .or_else(|| captured_body_json(&meta.client_request_body_json))
            })
            .flatten(),
        error_message: meta.error_message.clone(),
        error_body: meta.error_body.clone(),
        timing: meta.to_timing(),
        stream: meta.to_stream_details(),
    };
    control_store.apply_usage_rollup_owned(event).await
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
mod tests;
