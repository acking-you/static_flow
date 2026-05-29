//! Provider-facing HTTP entrypoints for `llm-access`.
//! ## Module map
//!
//! `provider.rs` is the facade for the provider-facing HTTP entrypoints. It
//! keeps the core types and their `impl` blocks (`ProviderState`,
//! `RequestLimiter`, `DefaultProviderDispatcher`, the stream-record guards, the
//! SSE accumulator, `ProviderUsageMetadata`, ...), the module
//! constants/statics, and the tests. The free functions are grouped by concern
//! into descendant submodules:
//!
//! ```text
//!  request -> [entry] auth/classify -> dispatcher
//!     |                                    |
//!     |   Codex plane                      |   Kiro plane
//!     +-- [codex_config]  url/version      +-- [kiro_payload]  model/cache shaping
//!     +-- [codex_headers] upstream headers +-- [kiro_request_headers] upstream headers
//!     +-- [codex_dispatch] proxy+adapt     +-- [kiro_dispatch] proxy/generate/mcp
//!     +-- [codex_retry]   reshape/retry    +-- [kiro_media]    remote media
//!     +-- [codex_sse]     sse accumulate   +-- [kiro_summary]  message summaries
//!     +-- [codex_usage]   usage record     +-- [kiro_usage]    usage/billing
//!     +-- [codex_models]  model catalog
//!  shared: [limiter] [routing] [client] [errors] [usage_meta] [util]
//! ```

mod kiro_error;

pub(crate) use std::{
    collections::{BTreeMap, HashMap, HashSet},
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    num::NonZeroUsize,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub(crate) use anyhow::{bail, Context};
pub(crate) use async_stream::stream;
pub(crate) use async_trait::async_trait;
pub(crate) use axum::{
    body::{to_bytes, Body, Bytes},
    extract::State,
    http::{header, HeaderMap, Method, Request, StatusCode},
    response::{IntoResponse, Response},
};
pub(crate) use base64::Engine as _;
pub(crate) use eventsource_stream::Eventsource;
pub(crate) use futures_util::{StreamExt, TryStreamExt};
pub(crate) use llm_access_codex::{
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
pub(crate) use llm_access_core::{
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
pub(crate) use llm_access_kiro::{
    anthropic::{
        converter::{
            convert_normalized_request_with_resolved_session, current_user_message_range,
            extract_tool_result_content, normalize_request, preview_session_value,
            resolve_conversation_id_from_metadata, ResolvedConversationId, ResponseModelIdentity,
            SessionFallbackReason, SessionIdSource, SessionTracking,
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
pub(crate) use lru::LruCache;
pub(crate) use rand::Rng;
pub(crate) use serde_json::{json, Value};

pub(crate) use self::kiro_error::{
    kiro_conversion_error_response, kiro_json_error, kiro_upstream_error_response,
    KiroRouteFailure, KiroRouteFailureKind,
};
pub(crate) use crate::{
    activity::RequestActivityTracker, codex_refresh, geoip::GeoIpResolver, kiro_headers,
    kiro_latency::KiroLatencyRanker, kiro_refresh,
};

mod client;
mod codex_config;
mod codex_dispatch;
mod codex_headers;
mod codex_models;
mod codex_retry;
mod codex_sse;
mod codex_usage;
mod entry;
mod errors;
mod kiro_dispatch;
mod kiro_media;
mod kiro_payload;
mod kiro_request_headers;
mod kiro_summary;
mod kiro_usage;
mod limiter;
mod routing;
mod usage_meta;
mod util;

pub(crate) use client::*;
pub(crate) use codex_config::*;
pub(crate) use codex_dispatch::*;
pub(crate) use codex_headers::*;
pub(crate) use codex_models::*;
pub(crate) use codex_retry::*;
pub(crate) use codex_sse::*;
pub(crate) use codex_usage::*;
pub use entry::*;
pub(crate) use errors::*;
pub(crate) use kiro_dispatch::*;
pub(crate) use kiro_media::*;
pub(crate) use kiro_payload::*;
pub(crate) use kiro_request_headers::*;
pub(crate) use kiro_summary::*;
pub(crate) use kiro_usage::*;
pub(crate) use limiter::*;
pub(crate) use routing::*;
pub(crate) use usage_meta::*;
pub(crate) use util::*;

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
pub(crate) struct CodexDispatchRuntimeConfig {
    client_version: String,
    account_attempt_limit: usize,
}
#[derive(Debug, Clone)]
pub(crate) struct ProviderUsageMetadata {
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
            kiro_latency_ranker: Arc::clone(&self.kiro_latency_ranker),
        }
    }
}
pub(crate) struct ForcedProxyRouteStore {
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
pub(crate) struct LimitScope {
    in_flight: u64,
    last_start: Option<Instant>,
}
pub(crate) struct LimitPermit {
    limiter: Arc<RequestLimiter>,
    scope: String,
}
#[derive(Debug, Clone)]
pub(crate) struct LimitRejection {
    reason: &'static str,
    in_flight: u64,
    max_concurrency: Option<u64>,
    min_start_interval_ms: Option<u64>,
    wait: Option<Duration>,
    elapsed_since_last_start_ms: Option<u64>,
}
#[derive(Default)]
pub(crate) struct CodexAccountCooldowns {
    blocked_until: Mutex<HashMap<String, Instant>>,
}
#[derive(Debug, Clone, Copy)]
pub(crate) struct ActiveCooldown {
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
pub(crate) struct DefaultProviderDispatcher;
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
pub(crate) struct CodexUpstreamResponseContext {
    prepared: PreparedGatewayRequest,
    key: AuthenticatedKey,
    route: ProviderCodexRoute,
    control_store: Arc<dyn ControlStore>,
    permits: Vec<LimitPermit>,
    usage_meta: ProviderUsageMetadata,
}
pub(crate) struct CodexUpstreamResponseParts {
    status: StatusCode,
    upstream_headers: reqwest::header::HeaderMap,
    content_type: String,
    bytes: Bytes,
}
pub(crate) struct CodexCompletedResponseContext {
    prepared: PreparedGatewayRequest,
    key: AuthenticatedKey,
    route: ProviderCodexRoute,
    control_store: Arc<dyn ControlStore>,
    permits: Vec<LimitPermit>,
    usage_meta: ProviderUsageMetadata,
}
pub(crate) struct CodexStreamContext {
    prepared: PreparedGatewayRequest,
    key: AuthenticatedKey,
    route: ProviderCodexRoute,
    control_store: Arc<dyn ControlStore>,
    permits: Vec<LimitPermit>,
    usage_meta: ProviderUsageMetadata,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KiroRemoteMediaKind {
    Image,
    Document,
}
#[derive(Debug, Clone, Copy)]
pub(crate) struct KiroRemoteMediaRequest<'a> {
    url: &'a str,
    kind: KiroRemoteMediaKind,
}
#[derive(Debug)]
pub(crate) struct ResolvedKiroRemoteMedia {
    media_type: Option<String>,
    bytes: Bytes,
}
#[derive(Debug)]
pub(crate) struct KiroRemoteMediaResolutionError {
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
pub(crate) trait KiroRemoteMediaFetcher: Sync {
    async fn fetch(
        &self,
        request: KiroRemoteMediaRequest<'_>,
    ) -> Result<ResolvedKiroRemoteMedia, KiroRemoteMediaResolutionError>;
}
pub(crate) struct ReqwestKiroRemoteMediaFetcher {
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StrippedKiroRemoteMediaSource {
    message_index: usize,
    block_index: usize,
    block_type: String,
    url_summary: String,
}
pub(crate) struct PendingKiroRemoteMediaSource {
    kind: KiroRemoteMediaKind,
    block_type: &'static str,
    url: String,
    source_media_type: Option<String>,
}
pub(crate) struct KiroResponseContext {
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
    _key_permit: LimitPermit,
    _account_permit: KiroRequestLease,
}
pub(crate) struct KiroWebsearchDispatch {
    key: AuthenticatedKey,
    payload: MessagesRequest,
    routes: Vec<ProviderKiroRoute>,
    control_store: Arc<dyn ControlStore>,
    route_store: Arc<dyn ProviderRouteStore>,
    request_limiter: Arc<RequestLimiter>,
    kiro_request_scheduler: Arc<KiroRequestScheduler>,
    kiro_latency_ranker: Arc<KiroLatencyRanker>,
    request_input_tokens: i32,
    usage_meta: ProviderUsageMetadata,
}
pub(crate) struct WebsearchResponseInput {
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
const KIRO_EMPTY_STREAM_MAX_RETRIES: usize = 2;
pub(crate) struct KiroPeekedStream {
    status: StatusCode,
    first_chunk: Bytes,
    remaining: futures_util::stream::BoxStream<'static, Result<Bytes, reqwest::Error>>,
}
pub(crate) enum KiroStreamPeekError {
    Empty,
    Read(reqwest::Error),
}
#[derive(Clone)]
pub(crate) struct KiroCacheContext {
    policy: KiroCachePolicy,
    simulation_config: KiroCacheSimulationConfig,
    projection: RuntimePromptProjection,
    prefix_cache_match: llm_access_kiro::cache_sim::PrefixCacheMatch,
    conversation_id: String,
    cache_kmodels: BTreeMap<String, f64>,
    billable_model_multipliers: BTreeMap<String, f64>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum StreamRecordState {
    Pending,
    InternalFailure,
}
pub(crate) struct CodexStreamRecordGuard {
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
pub(crate) struct KiroStreamRecordGuard {
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
pub(crate) struct KiroUsageSummary {
    input_uncached_tokens: i32,
    input_cached_tokens: i32,
    output_tokens: i32,
    credit_usage: Option<f64>,
    credit_usage_missing: bool,
}
#[derive(Debug, Clone, Copy)]
pub(crate) struct KiroUsageInputs {
    request_input_tokens: i32,
    context_input_tokens: Option<i32>,
    context_usage_min_request_tokens: u64,
    output_tokens: i32,
    credit_usage: Option<f64>,
    credit_usage_missing: bool,
    cache_estimation_enabled: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ProviderClientCacheKey {
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
pub(crate) struct KiroUsageRecord<'a> {
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
pub(crate) struct KiroPreflightFailureRecord<'a> {
    control_store: &'a dyn ControlStore,
    key: &'a AuthenticatedKey,
    route: &'a ProviderKiroRoute,
    endpoint: &'a str,
    model: &'a str,
    status: StatusCode,
    meta: &'a mut ProviderUsageMetadata,
    cache_simulator: &'a KiroCacheSimulator,
}
pub(crate) struct KiroWebsearchUsageRecord<'a> {
    control_store: &'a dyn ControlStore,
    key: &'a AuthenticatedKey,
    route: &'a ProviderKiroRoute,
    model: &'a str,
    status: StatusCode,
    usage: KiroUsageSummary,
    meta: &'a ProviderUsageMetadata,
    capture_request_details: bool,
}
#[derive(Debug, Clone)]
pub(crate) struct CodexAuthSnapshot {
    access_token: String,
    account_id: Option<String>,
    is_fedramp_account: bool,
}
#[derive(Debug, Default)]
pub(crate) struct CodexTurnMetadataHeader {
    session_id: Option<String>,
    thread_id: Option<String>,
}
#[derive(Debug, Default)]
pub(crate) struct CodexUpstreamSessionHeaders {
    conversation_id: Option<String>,
    session_id: Option<String>,
    thread_id: Option<String>,
    client_request_id: Option<String>,
}
pub(crate) struct CompletedCodexSse {
    response: Value,
    usage: Option<UsageBreakdown>,
}
pub(crate) struct CompletedCodexSseError {
    status: StatusCode,
    message: String,
    body: Option<String>,
}
#[derive(Default)]
pub(crate) struct CompletedCodexSseAccumulator {
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
pub(crate) struct SsePayload {
    event: Option<String>,
    data: String,
}
pub(crate) struct CodexPreflightFailureRecord<'a> {
    control_store: &'a dyn ControlStore,
    key: &'a AuthenticatedKey,
    endpoint: &'a str,
    model: Option<String>,
    status: StatusCode,
    meta: &'a mut ProviderUsageMetadata,
}

#[cfg(test)]
#[allow(
    clippy::await_holding_lock,
    reason = "provider tests serialize process-wide upstream env var overrides across awaited \
              requests"
)]
mod tests;
