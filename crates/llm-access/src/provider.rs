//! Provider-facing HTTP entrypoints for `llm-access`.

mod client;
mod codex_auth;
mod codex_dispatch;
mod codex_models;
mod codex_sse;
mod entry;
mod errors;
mod kiro_dispatch;
mod kiro_error;
mod kiro_media;
mod kiro_model;
mod kiro_protocol;
mod kiro_session_affinity;
mod kiro_summary;
mod kiro_usage;
mod limiter;
mod route_selection;
mod state;
mod stream_guards;
mod usage_meta;
mod util;

use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use async_trait::async_trait;
#[cfg(test)]
use axum::http::Method;
use axum::{
    body::{Body, Bytes},
    http::{Request, StatusCode},
    response::Response,
};
use client::{
    build_provider_client, provider_client_cache_capacity, provider_client_pool_idle_timeout,
    provider_client_pool_max_idle_per_host,
};
pub(crate) use codex_auth::{
    codex_upstream_base_url, compute_codex_upstream_url, resolve_codex_client_version,
};
#[cfg(test)]
use codex_auth::{header_value, normalized_codex_gateway_path};
pub(crate) use codex_models::{
    codex_public_model_catalog_response, default_codex_public_model_catalog_response,
};
pub use entry::{provider_entry, provider_entry_handler};
use errors::{anthropic_json_error, summarize_error_bytes};
#[cfg(test)]
use errors::{
    daily_request_limit_cooldown, is_monthly_request_limit, kiro_text_is_content_length_exceeded,
    transient_invalid_model_cooldown,
};
pub(crate) use kiro_dispatch::call_kiro_generate_for_route;
#[cfg(test)]
use kiro_dispatch::{account_names_for_kiro_routing_identity, call_kiro_mcp_for_route};
#[cfg(test)]
use kiro_media::{resolve_kiro_remote_media_sources_with_fetcher, strip_kiro_remote_media_sources};
pub(crate) use kiro_model::decode_kiro_events_from_bytes;
#[cfg(test)]
use kiro_model::{build_kiro_cache_context, override_kiro_thinking_from_model_name};
#[cfg(test)]
use kiro_protocol::normalized_kiro_messages_path;
#[cfg(test)]
use kiro_usage::{
    anthropic_usage_json_with_policy, kiro_billable_tokens_with_multipliers,
    normalize_kiro_kmodel_name, record_kiro_usage, record_kiro_websearch_usage,
};
use llm_access_codex::{
    response::SseUsageCollector,
    types::{PreparedGatewayRequest, UsageBreakdown},
};
use llm_access_core::store::{
    AdminConfigStore, AuthenticatedKey, ControlStore, ProviderCodexRoute, ProviderKiroRoute,
    ProviderProxyConfig, ProviderRouteStore,
};
use llm_access_kiro::{
    anthropic::{
        converter::ResponseModelIdentity,
        stream::StreamContext,
        types::MessagesRequest,
        websearch::{self},
    },
    cache_policy::KiroCachePolicy,
    cache_sim::{KiroCacheSimulationConfig, KiroCacheSimulator, RuntimePromptProjection},
    scheduler::{KiroRequestLease, KiroRequestScheduler},
};
use lru::LruCache;
#[cfg(test)]
use route_selection::{
    select_codex_route_with_account_permit, select_kiro_route_with_account_permit,
    selection_ordered_kiro_routes,
};
use serde_json::Value;

use self::kiro_session_affinity::KiroSessionAffinity;
use crate::{
    activity::RequestActivityTracker, geoip::GeoIpResolver, kiro_latency::KiroLatencyRanker,
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

struct ForcedProxyRouteStore {
    inner: Arc<dyn ProviderRouteStore>,
    proxy: ProviderProxyConfig,
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

struct CodexUpstreamResponseContext {
    prepared: PreparedGatewayRequest,
    key: AuthenticatedKey,
    route: ProviderCodexRoute,
    control_store: Arc<dyn ControlStore>,
    permits: Vec<LimitPermit>,
    usage_meta: ProviderUsageMetadata,
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

struct CodexStreamContext {
    prepared: PreparedGatewayRequest,
    key: AuthenticatedKey,
    route: ProviderCodexRoute,
    control_store: Arc<dyn ControlStore>,
    permits: Vec<LimitPermit>,
    usage_meta: ProviderUsageMetadata,
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct StrippedKiroRemoteMediaSource {
    message_index: usize,
    block_index: usize,
    block_type: String,
    url_summary: String,
}

struct PendingKiroRemoteMediaSource {
    kind: KiroRemoteMediaKind,
    block_type: &'static str,
    url: String,
    source_media_type: Option<String>,
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
            .dns_resolver(Arc::new(kiro_media::PrivateFilteringDnsResolver))
            .pool_idle_timeout(provider_client_pool_idle_timeout())
            .pool_max_idle_per_host(provider_client_pool_max_idle_per_host())
            .tcp_keepalive(Duration::from_secs(30))
            .build()
            .expect("kiro remote media client should build")
    });

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

#[derive(Debug, Clone)]
struct CodexAuthSnapshot {
    access_token: String,
    account_id: Option<String>,
    is_fedramp_account: bool,
}

#[derive(Debug, Default)]
struct CodexTurnMetadataHeader {
    session_id: Option<String>,
    thread_id: Option<String>,
}

#[derive(Debug, Default)]
struct CodexUpstreamSessionHeaders {
    conversation_id: Option<String>,
    session_id: Option<String>,
    thread_id: Option<String>,
    client_request_id: Option<String>,
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

struct SsePayload {
    event: Option<String>,
    data: String,
}

struct CodexPreflightFailureRecord<'a> {
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
