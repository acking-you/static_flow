//! Shared Codex image gateway.
//!
//! [`CodexImageGateway`] is the single OpenAI-compatible `gpt-image` dispatch
//! engine used by both entrypoints of this feature:
//! - the standalone `llm-access-codex-image` binary ([`main`](crate)), and
//! - the main `llm-access` Codex API binary, which embeds it in its router.
//!
//! The two entrypoints differ only in [`ImageGatewayMode`] (which per-key gate
//! is enforced) and in how the dependencies are wired; the request handling,
//! account failover, concurrency limiting, redacted logging, and usage rollup
//! all live here so the behavior cannot drift between the two binaries.
//!
//! Request flow: [`CodexImageGateway::handle_request`] validates the public
//! request (path/method/content-type/bearer/key gate/body) and hands a parsed
//! [`request::CodexImageRequest`] to [`CodexImageGateway::dispatch_image_request`],
//! which acquires a per-key permit and then walks the eligible Codex accounts,
//! failing over on auth/transient errors until one succeeds or all are
//! exhausted.

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context};
use axum::{
    body::{to_bytes, Body, Bytes},
    http::{header, HeaderMap, Method, Request, Response, StatusCode},
    response::IntoResponse,
};
use bytes::BytesMut;
use llm_access_codex::request::{
    codex_user_agent, extract_client_ip_from_headers, extract_header_value,
};
use llm_access_core::{
    provider::{ProtocolFamily, ProviderType},
    store::{
        codex_access_token_expires_at_ms, AuthenticatedKey, ControlStore, ProviderCodexRoute,
        ProviderProxyConfig, ProviderRouteStore, KEY_STATUS_ACTIVE,
    },
    usage::{UsageEvent, UsageStreamDetails, UsageTiming},
};
use serde_json::Value;

use crate::{
    dispatch::{eligible_image_routes, should_failover_status, ImageGatewayMode},
    limiter::{ImageAccountLimiter, ImageKeyLimitRejection, ImageKeyLimiter},
    logging::{build_image_log_event, ImageLogInput, ImageLogWriter, UpstreamLogInput},
    request::{normalize_image_gateway_path, parse_image_request, upstream_image_path},
    util::{lock_unpoisoned, now_ms},
};

/// Upper bound on the downstream image request body. Covers prompts plus the
/// base64 reference images of an `images/edits` call while rejecting payloads
/// large enough to be a memory-exhaustion vector.
const MAX_IMAGE_REQUEST_BODY_BYTES: usize = 24 * 1024 * 1024;
/// Upper bound on the buffered upstream image response. `gpt-image` responses
/// carry base64 image data (size × `n`), so this is generous, but it is still
/// capped so a single response cannot exhaust process memory.
const MAX_IMAGE_UPSTREAM_RESPONSE_BODY_BYTES: u64 = 96 * 1024 * 1024;

/// Runtime configuration for one Codex image gateway instance.
pub struct CodexImageGatewayConfig {
    /// Entrypoint mode controlling which per-key gate is enforced.
    pub mode: ImageGatewayMode,
    /// Bearer-key authentication and image usage recorder.
    pub control_store: Arc<dyn ControlStore>,
    /// Codex route/account resolution source.
    pub route_store: Arc<dyn ProviderRouteStore>,
    /// Redacted image request log writer.
    pub image_log: ImageLogWriter,
    /// Codex upstream base URL.
    pub upstream_base: String,
    /// Codex CLI client version sent to upstream.
    pub codex_client_version: String,
}

/// Shared Codex image gateway used by both the standalone binary and the main
/// llm-access Codex API service.
#[derive(Clone)]
pub struct CodexImageGateway {
    mode: ImageGatewayMode,
    control_store: Arc<dyn ControlStore>,
    route_store: Arc<dyn ProviderRouteStore>,
    limiter: Arc<ImageAccountLimiter>,
    key_limiter: Arc<ImageKeyLimiter>,
    image_log: Arc<Mutex<ImageLogWriter>>,
    default_client: reqwest::Client,
    proxy_clients: Arc<Mutex<HashMap<String, reqwest::Client>>>,
    upstream_base: String,
    codex_client_version: String,
}

impl CodexImageGateway {
    /// Build one process-local Codex image gateway.
    pub fn new(config: CodexImageGatewayConfig) -> anyhow::Result<Self> {
        Ok(Self {
            mode: config.mode,
            control_store: config.control_store,
            route_store: config.route_store,
            limiter: Arc::new(ImageAccountLimiter::default()),
            key_limiter: Arc::new(ImageKeyLimiter::default()),
            image_log: Arc::new(Mutex::new(config.image_log)),
            default_client: provider_client(None)?,
            proxy_clients: Arc::new(Mutex::new(HashMap::new())),
            upstream_base: config.upstream_base,
            codex_client_version: config.codex_client_version,
        })
    }

    /// Handle one OpenAI-compatible image request.
    pub async fn handle_request(&self, request: Request<Body>) -> axum::response::Response {
        let request_id = new_request_id();
        let started = Instant::now();
        let path = request.uri().path().to_string();
        let Some(endpoint) = normalize_image_gateway_path(&path) else {
            return (StatusCode::NOT_FOUND, "unsupported image gateway path").into_response();
        };
        if request.method() != Method::POST {
            return (StatusCode::METHOD_NOT_ALLOWED, "image endpoint requires POST")
                .into_response();
        }
        if !is_json_request(request.headers()) {
            return (StatusCode::UNSUPPORTED_MEDIA_TYPE, "image endpoint requires JSON")
                .into_response();
        }
        let headers = request.headers().clone();
        let Some(secret) = bearer_secret(&headers) else {
            return (StatusCode::UNAUTHORIZED, "missing bearer token").into_response();
        };
        let key = match self.control_store.authenticate_bearer_secret(secret).await {
            Ok(Some(key)) => key,
            Ok(None) => return (StatusCode::UNAUTHORIZED, "invalid bearer token").into_response(),
            Err(err) => {
                tracing::warn!(error = %err, "codex image authentication failed");
                return (StatusCode::INTERNAL_SERVER_ERROR, "authentication backend error")
                    .into_response();
            },
        };
        if let Some(response) = reject_key(&key) {
            return response;
        }
        let body = match to_bytes(request.into_body(), MAX_IMAGE_REQUEST_BODY_BYTES).await {
            Ok(body) => body,
            Err(_) => {
                return (StatusCode::BAD_REQUEST, "invalid image request body").into_response()
            },
        };
        let payload = match serde_json::from_slice::<Value>(&body) {
            Ok(payload) => payload,
            Err(_) => {
                return (StatusCode::BAD_REQUEST, "request body must be valid JSON").into_response()
            },
        };
        let image_request = match parse_image_request(endpoint, payload) {
            Ok(image_request) => image_request,
            Err(err) => return (err.status, err.message).into_response(),
        };
        self.dispatch_image_request(ImageDispatchRequest {
            request_id: &request_id,
            started,
            headers: &headers,
            key: &key,
            endpoint_name: endpoint_name(endpoint),
            upstream_path: upstream_image_path(endpoint),
            image_request,
        })
        .await
    }

    /// Resolve eligible Codex accounts and forward the image request, failing
    /// over across accounts until one returns a non-retryable response.
    ///
    /// A per-key permit (concurrency + min-start-interval) is acquired once for
    /// the whole request and held by RAII until this function returns. Each
    /// candidate then takes a per-account concurrency permit for the duration
    /// of its single upstream attempt. Auth gaps, expired tokens, blocked
    /// account slots, proxy/transport errors, and retryable upstream statuses
    /// ([`should_failover_status`]) advance to the next candidate; every other
    /// status is returned to the client. When all candidates are exhausted the
    /// aggregate status reflects the dominant failure mode (429 when only
    /// account slots were busy, 503 on auth exhaustion, else 502).
    async fn dispatch_image_request(
        &self,
        ctx: ImageDispatchRequest<'_>,
    ) -> axum::response::Response {
        let candidates = match self
            .route_store
            .resolve_codex_route_candidates(ctx.key)
            .await
        {
            Ok(candidates) => candidates,
            Err(err) => {
                tracing::warn!(error = %err, "codex image route lookup failed");
                return (StatusCode::INTERNAL_SERVER_ERROR, "route store error").into_response();
            },
        };
        let candidates = match eligible_image_routes(self.mode, candidates) {
            Ok(candidates) => candidates,
            Err(err) => return (err.status, err.message).into_response(),
        };
        let key_limits = candidates
            .first()
            .map(|candidate| {
                (candidate.request_max_concurrency, candidate.request_min_start_interval_ms)
            })
            .unwrap_or((None, None));
        let _key_permit =
            match self
                .key_limiter
                .try_acquire(&ctx.key.key_id, key_limits.0, key_limits.1)
            {
                Ok(permit) => permit,
                Err(rejection) => {
                    self.log_image_event(
                        ctx.request_id,
                        ctx.key,
                        None,
                        ctx.endpoint_name,
                        &ctx.image_request,
                        UpstreamLogInput {
                            status: Some(StatusCode::TOO_MANY_REQUESTS.as_u16()),
                            duration_ms: duration_ms(ctx.started),
                            failover_count: 0,
                            error_class: Some(rejection.reason),
                            response_image_count: None,
                            response_image_bytes: None,
                            usage_tokens: None,
                            usage_missing: true,
                        },
                    );
                    tracing::info!(
                        request_id = ctx.request_id,
                        mode = ?self.mode,
                        key_id = %ctx.key.key_id,
                        reason = rejection.reason,
                        "codex image request rejected by per-key limit"
                    );
                    self.spawn_image_usage_event(
                        &ctx,
                        ImageUsageOutcome {
                            account_name: None,
                            status: StatusCode::TOO_MANY_REQUESTS,
                            response_image_count: None,
                            usage_tokens: None,
                            error_class: Some(rejection.reason.to_string()),
                            error_message: None,
                            failover_count: 0,
                        },
                    );
                    return image_key_limit_response(&rejection);
                },
            };
        let mut failover_count = 0_u64;
        let mut concurrency_blocked = 0_usize;
        let mut last_error_class = None::<String>;
        for candidate in candidates {
            let Some(route) = self.hydrate_codex_image_route(candidate).await else {
                failover_count += 1;
                last_error_class = Some("auth_missing".to_string());
                continue;
            };
            let auth = match parse_codex_image_auth(&route.auth_json) {
                Ok(auth) => auth,
                Err(error_class) => {
                    failover_count += 1;
                    last_error_class = Some(error_class.to_string());
                    continue;
                },
            };
            if access_token_expired(&auth.access_token) {
                failover_count += 1;
                last_error_class = Some("auth_expired".to_string());
                continue;
            }
            let Some(_permit) = self.limiter.try_acquire(
                &route.account_name,
                Some(route.account_codex_image_generation_max_concurrency),
            ) else {
                concurrency_blocked += 1;
                last_error_class = Some("concurrency_blocked".to_string());
                continue;
            };
            tracing::debug!(
                request_id = ctx.request_id,
                account = %route.account_name,
                endpoint = ctx.endpoint_name,
                attempt = failover_count + 1,
                "codex image dispatching to account"
            );
            let client = match self.provider_client_for_route(route.proxy.as_ref()) {
                Ok(client) => client,
                Err(err) => {
                    tracing::warn!(account = %route.account_name, error = %err, "codex image client build failed");
                    failover_count += 1;
                    last_error_class = Some("proxy_client".to_string());
                    continue;
                },
            };
            let upstream_url = llm_access_codex::request::compute_codex_upstream_url(
                &self.upstream_base,
                ctx.upstream_path,
            );
            let request_builder = add_image_upstream_headers(
                client.post(upstream_url),
                ctx.headers,
                &auth.access_token,
                auth.account_id,
                auth.fedramp,
                &self.codex_client_version,
            )
            .body(ctx.image_request.raw.to_string());
            let upstream = match request_builder.send().await {
                Ok(response) => response,
                Err(err) => {
                    tracing::warn!(account = %route.account_name, error = %err, "codex image upstream request failed");
                    failover_count += 1;
                    last_error_class = Some("upstream_request".to_string());
                    continue;
                },
            };
            let status = upstream.status();
            let headers = upstream.headers().clone();
            let bytes = match limited_upstream_bytes(
                upstream,
                MAX_IMAGE_UPSTREAM_RESPONSE_BODY_BYTES,
            )
            .await
            {
                Ok(bytes) => bytes,
                Err(err) => {
                    tracing::warn!(account = %route.account_name, error = %err, "codex image upstream response read failed");
                    failover_count += 1;
                    last_error_class = Some("upstream_body".to_string());
                    continue;
                },
            };
            let response_json = serde_json::from_slice::<Value>(&bytes).ok();
            let metrics = response_json
                .as_ref()
                .map(response_image_metrics)
                .unwrap_or_default();
            let usage_tokens = response_json.as_ref().and_then(usage_tokens_from_response);
            self.log_image_event(
                ctx.request_id,
                ctx.key,
                Some(&route.account_name),
                ctx.endpoint_name,
                &ctx.image_request,
                UpstreamLogInput {
                    status: Some(status.as_u16()),
                    duration_ms: duration_ms(ctx.started),
                    failover_count,
                    error_class: if status.is_success() {
                        None
                    } else {
                        Some(status_error_class(status))
                    },
                    response_image_count: metrics.image_count,
                    response_image_bytes: metrics.image_bytes,
                    usage_tokens,
                    usage_missing: usage_tokens.is_none(),
                },
            );
            if should_failover_status(status) {
                failover_count += 1;
                last_error_class = Some(status_error_class(status).to_string());
                continue;
            }
            if status.is_success() {
                self.spawn_record_codex_image_usage(ctx.key, usage_tokens);
            }
            let error_class =
                (!status.is_success()).then(|| status_error_class(status).to_string());
            let error_message = if status.is_success() {
                None
            } else {
                error_message_from_bytes(&bytes)
            };
            tracing::info!(
                request_id = ctx.request_id,
                mode = ?self.mode,
                key_id = %ctx.key.key_id,
                account = %route.account_name,
                endpoint = ctx.endpoint_name,
                status = status.as_u16(),
                failover = failover_count,
                duration_ms = duration_ms(ctx.started),
                image_count = ?metrics.image_count,
                usage_tokens = ?usage_tokens,
                "codex image request completed"
            );
            self.spawn_image_usage_event(
                &ctx,
                ImageUsageOutcome {
                    account_name: Some(route.account_name.clone()),
                    status,
                    response_image_count: metrics.image_count,
                    usage_tokens,
                    error_class,
                    error_message,
                    failover_count,
                },
            );
            return upstream_response(status, &headers, bytes);
        }
        let status = if concurrency_blocked > 0 && failover_count == 0 {
            StatusCode::TOO_MANY_REQUESTS
        } else if matches!(
            last_error_class.as_deref(),
            Some("auth_missing" | "auth_invalid_json" | "auth_expired" | "auth")
        ) {
            StatusCode::SERVICE_UNAVAILABLE
        } else {
            StatusCode::BAD_GATEWAY
        };
        self.log_image_event(
            ctx.request_id,
            ctx.key,
            None,
            ctx.endpoint_name,
            &ctx.image_request,
            UpstreamLogInput {
                status: Some(status.as_u16()),
                duration_ms: duration_ms(ctx.started),
                failover_count,
                error_class: Some(last_error_class.as_deref().unwrap_or("no_eligible_account")),
                response_image_count: None,
                response_image_bytes: None,
                usage_tokens: None,
                usage_missing: true,
            },
        );
        tracing::warn!(
            request_id = ctx.request_id,
            mode = ?self.mode,
            key_id = %ctx.key.key_id,
            endpoint = ctx.endpoint_name,
            status = status.as_u16(),
            failover = failover_count,
            concurrency_blocked,
            error_class = last_error_class.as_deref().unwrap_or("no_eligible_account"),
            "codex image request exhausted all eligible accounts"
        );
        self.spawn_image_usage_event(
            &ctx,
            ImageUsageOutcome {
                account_name: None,
                status,
                response_image_count: None,
                usage_tokens: None,
                error_class: Some(
                    last_error_class
                        .clone()
                        .unwrap_or_else(|| "no_eligible_account".to_string()),
                ),
                error_message: Some(
                    "all eligible codex image accounts failed for this request".to_string(),
                ),
                failover_count,
            },
        );
        (status, "all eligible codex image accounts failed for this request").into_response()
    }

    /// Emit a DuckDB usage event for a completed image request when running
    /// inside the main Codex API binary ([`ImageGatewayMode::IntegratedCodexApi`]).
    ///
    /// Image traffic then shows up in the usage analytics alongside text/Kiro
    /// requests, with the gpt-image usage tokens counted toward the key's
    /// billable quota (Codex bills image generation by token) and the returned
    /// image count surfaced for the usage visualization. The standalone binary
    /// has no usage worker/journal, so it intentionally skips this and relies on
    /// the redacted JSONL image log plus the per-key image-token rollup instead.
    ///
    /// Fire-and-forget: enqueuing onto the usage journal must not block the
    /// client response, mirroring [`Self::spawn_record_codex_image_usage`].
    fn spawn_image_usage_event(&self, ctx: &ImageDispatchRequest<'_>, outcome: ImageUsageOutcome) {
        if self.mode != ImageGatewayMode::IntegratedCodexApi {
            return;
        }
        // Codex bills image generation by token, so fold the reported usage into
        // both the output and billable columns; `usage_missing` records when the
        // upstream omitted a usage block so the UI can flag estimate-free rows.
        let billable = outcome
            .usage_tokens
            .map(|tokens| i64::try_from(tokens).unwrap_or(i64::MAX))
            .unwrap_or(0);
        let endpoint = format!("/v1/images/{}", ctx.endpoint_name);
        let event = UsageEvent {
            event_id: format!("llm-usage-{}", ctx.request_id),
            created_at_ms: i64::try_from(now_ms()).unwrap_or(i64::MAX),
            provider_type: ProviderType::Codex,
            protocol_family: ProtocolFamily::OpenAi,
            key_id: ctx.key.key_id.clone(),
            key_name: ctx.key.key_name.clone(),
            account_name: outcome.account_name,
            account_group_id_at_event: None,
            route_strategy_at_event: None,
            request_method: "POST".to_string(),
            request_url: endpoint.clone(),
            endpoint,
            model: Some(ctx.image_request.model.clone()),
            mapped_model: Some(ctx.image_request.model.clone()),
            status_code: i64::from(outcome.status.as_u16()),
            request_body_bytes: None,
            quota_failover_count: outcome.failover_count,
            routing_diagnostics_json: None,
            input_uncached_tokens: 0,
            input_cached_tokens: 0,
            output_tokens: billable,
            billable_tokens: billable,
            credit_usage: None,
            usage_missing: outcome.usage_tokens.is_none(),
            credit_usage_missing: false,
            client_ip: extract_client_ip_from_headers(ctx.headers),
            // The image gateway has no GeoIP resolver (it lives in `llm-access`),
            // so region enrichment is left to the decoder's default.
            ip_region: "unknown".to_string(),
            request_headers_json: "{}".to_string(),
            last_message_content: None,
            client_request_body_json: None,
            upstream_request_body_json: None,
            full_request_json: None,
            error_message: outcome.error_message,
            error_class: outcome.error_class,
            session_blocked: false,
            response_image_count: outcome
                .response_image_count
                .map(|count| i64::try_from(count).unwrap_or(i64::MAX)),
            error_body: None,
            response_body: None,
            timing: UsageTiming {
                latency_ms: Some(i64::try_from(duration_ms(ctx.started)).unwrap_or(i64::MAX)),
                ..UsageTiming::default()
            },
            stream: UsageStreamDetails::default(),
        };
        let control_store = Arc::clone(&self.control_store);
        let request_id = ctx.request_id.to_string();
        tokio::spawn(async move {
            if let Err(err) = control_store.apply_usage_rollup_owned(event).await {
                tracing::warn!(
                    request_id = %request_id,
                    error = %err,
                    "codex image usage event write failed"
                );
            }
        });
    }

    /// Refresh the per-account auth/proxy fields of a route candidate from the
    /// account store just before dispatch, since the route-selection snapshot
    /// can be stale relative to the latest token refresh.
    ///
    /// Only `auth_json` (the freshly resolved token), `cached_error_message`,
    /// and `proxy` are taken from the hydrated account; every other key-level
    /// field is kept from `candidate` via struct-update so newly added
    /// `ProviderCodexRoute` fields are carried through automatically.
    async fn hydrate_codex_image_route(
        &self,
        candidate: ProviderCodexRoute,
    ) -> Option<ProviderCodexRoute> {
        let hydrated = self
            .route_store
            .resolve_codex_account_route(&candidate.account_name)
            .await
            .ok()
            .flatten()?;
        Some(ProviderCodexRoute {
            auth_json: hydrated.auth_json,
            cached_error_message: candidate
                .cached_error_message
                .or(hydrated.cached_error_message),
            proxy: candidate.proxy.or(hydrated.proxy),
            ..candidate
        })
    }

    /// Return a pooled `reqwest::Client` for the route's proxy configuration.
    ///
    /// Clients own their connection pool, so they are reused rather than rebuilt
    /// per request: the no-proxy case shares `default_client`, and each distinct
    /// proxy config is built once and memoized (cloning a `Client` is a cheap
    /// `Arc` bump that shares the pool). The cache is keyed by the full proxy
    /// URL + credentials and is bounded by the number of configured proxies.
    fn provider_client_for_route(
        &self,
        proxy: Option<&ProviderProxyConfig>,
    ) -> anyhow::Result<reqwest::Client> {
        let Some(proxy_config) = proxy else {
            return Ok(self.default_client.clone());
        };
        let cache_key = proxy_client_key(proxy_config);
        let mut clients = lock_unpoisoned(&self.proxy_clients);
        if let Some(client) = clients.get(&cache_key) {
            return Ok(client.clone());
        }
        let client = provider_client(Some(proxy_config))?;
        clients.insert(cache_key, client.clone());
        Ok(client)
    }

    /// Append one redacted image request log event off the request path.
    ///
    /// The event is built synchronously (cheap) but the blocking file append is
    /// moved to a `spawn_blocking` task so neither disk I/O nor the writer mutex
    /// stalls the async request handler. The join handle is intentionally
    /// dropped: logging is best-effort and must not delay or fail the response.
    fn log_image_event(
        &self,
        request_id: &str,
        key: &AuthenticatedKey,
        account_name: Option<&str>,
        endpoint: &str,
        image_request: &crate::request::CodexImageRequest,
        upstream: UpstreamLogInput<'_>,
    ) {
        let input_images = image_request
            .images
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        let event = build_image_log_event(ImageLogInput {
            gateway_mode: self.mode.as_log_str(),
            request_id,
            key_id: &key.key_id,
            key_name: &key.key_name,
            account_name,
            endpoint,
            prompt: &image_request.prompt,
            size: image_request.size.as_deref(),
            quality: image_request.quality.as_deref(),
            n: image_request.n,
            input_images: &input_images,
            upstream,
        });
        let image_log = Arc::clone(&self.image_log);
        let handle = tokio::task::spawn_blocking(move || {
            let mut writer = lock_unpoisoned(&image_log);
            if let Err(err) = writer.append(&event) {
                tracing::warn!(error = %err, "codex image request log write failed");
            }
        });
        drop(handle);
    }

    /// Fire-and-forget the per-key Codex image usage rollup.
    ///
    /// Spawned rather than awaited so the held per-account and per-key
    /// concurrency permits release and the image bytes return to the client
    /// without blocking on a control-plane write. This rollup is an
    /// admin-visibility metric only — quota/billing is gated up front in
    /// [`reject_key`] via `remaining_billable`, so losing a row to a
    /// mid-flight crash is acceptable and never over-serves a key.
    fn spawn_record_codex_image_usage(&self, key: &AuthenticatedKey, usage_tokens: Option<u64>) {
        let control_store = Arc::clone(&self.control_store);
        let key_id = key.key_id.clone();
        let key_name = key.key_name.clone();
        let used_at_ms = i64::try_from(now_ms()).unwrap_or(i64::MAX);
        tokio::spawn(async move {
            if let Err(err) = control_store
                .record_codex_image_key_usage(&key_id, usage_tokens, used_at_ms)
                .await
            {
                tracing::warn!(
                    key_id = %key_id,
                    key_name = %key_name,
                    error = %err,
                    "codex image key usage rollup write failed"
                );
            }
        });
    }
}

impl ImageGatewayMode {
    fn as_log_str(self) -> &'static str {
        match self {
            Self::StandaloneBinary => "standalone",
            Self::IntegratedCodexApi => "direct",
        }
    }
}

struct ImageDispatchRequest<'a> {
    request_id: &'a str,
    started: Instant,
    headers: &'a HeaderMap,
    key: &'a AuthenticatedKey,
    endpoint_name: &'static str,
    upstream_path: &'a str,
    image_request: crate::request::CodexImageRequest,
}

/// The terminal outcome of an image request, used to build at most one
/// integrated-mode usage event per request (not one per failover attempt).
struct ImageUsageOutcome {
    /// Account that served the request, or `None` when no account was reached.
    account_name: Option<String>,
    /// Final HTTP status returned to the client.
    status: StatusCode,
    /// Number of images in a successful response, when known.
    response_image_count: Option<u64>,
    /// gpt-image usage tokens reported by the upstream, when present.
    usage_tokens: Option<u64>,
    /// Stable error class for failed requests.
    error_class: Option<String>,
    /// Short human-readable error surfaced inline in the usage UI.
    error_message: Option<String>,
    /// Number of account failovers before the terminal outcome.
    failover_count: u64,
}

/// Best-effort short error message extracted from an upstream image error body,
/// so a failed image request surfaces a reason inline in the usage UI without
/// opening the detail view. Mirrors the text gateway's error-message capture and
/// caps length to keep the usage row compact.
fn error_message_from_bytes(bytes: &[u8]) -> Option<String> {
    let value = serde_json::from_slice::<Value>(bytes).ok()?;
    let message = value
        .pointer("/error/message")
        .and_then(Value::as_str)
        .or_else(|| value.get("message").and_then(Value::as_str))?;
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.chars().take(500).collect())
}

fn reject_key(key: &AuthenticatedKey) -> Option<axum::response::Response> {
    if key.status != KEY_STATUS_ACTIVE {
        return Some((StatusCode::FORBIDDEN, "llm key is not active").into_response());
    }
    if ProviderType::from_storage_str(&key.provider_type) != Some(ProviderType::Codex)
        || ProtocolFamily::from_storage_str(&key.protocol_family) != Some(ProtocolFamily::OpenAi)
    {
        return Some(
            (StatusCode::FORBIDDEN, "llm key does not match codex image route").into_response(),
        );
    }
    if key.remaining_billable() <= 0 {
        return Some((StatusCode::TOO_MANY_REQUESTS, "quota_exceeded").into_response());
    }
    None
}

fn image_key_limit_response(rejection: &ImageKeyLimitRejection) -> axum::response::Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        format!(
            "key image request limit reached: {} in_flight={} request_max_concurrency={} \
             request_min_start_interval_ms={}",
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
        ),
    )
        .into_response()
}

fn add_image_upstream_headers(
    builder: reqwest::RequestBuilder,
    headers: &HeaderMap,
    access_token: &str,
    account_id: Option<String>,
    fedramp: bool,
    codex_client_version: &str,
) -> reqwest::RequestBuilder {
    let mut builder = builder
        .bearer_auth(access_token)
        .header(reqwest::header::ACCEPT, "application/json")
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(
            reqwest::header::USER_AGENT,
            extract_header_value(headers, header::USER_AGENT.as_str())
                .unwrap_or_else(|| codex_user_agent(codex_client_version)),
        )
        .header(
            reqwest::header::HeaderName::from_static("originator"),
            extract_header_value(headers, "originator")
                .unwrap_or_else(|| "codex_cli_rs".to_string()),
        );
    for header_name in [
        "openai-beta",
        "x-openai-subagent",
        "x-codex-beta-features",
        "x-codex-installation-id",
        "traceparent",
        "tracestate",
        "baggage",
    ] {
        if let Some(value) = extract_header_value(headers, header_name) {
            builder = builder.header(header_name, value);
        }
    }
    if let Some(account_id) = account_id {
        builder = builder.header("chatgpt-account-id", account_id);
    }
    if fedramp {
        builder = builder.header("x-openai-fedramp", "true");
    }
    builder
}

fn provider_client(proxy: Option<&ProviderProxyConfig>) -> anyhow::Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .pool_idle_timeout(Duration::from_secs(90))
        .pool_max_idle_per_host(32)
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

fn proxy_client_key(proxy_config: &ProviderProxyConfig) -> String {
    format!(
        "{}\0{}\0{}",
        proxy_config.proxy_url,
        proxy_config.proxy_username.as_deref().unwrap_or_default(),
        proxy_config.proxy_password.as_deref().unwrap_or_default()
    )
}

/// Buffer an upstream response body, rejecting it once it exceeds `max_bytes`.
///
/// Checks the advertised `Content-Length` first for a cheap early reject, then
/// enforces the cap incrementally while streaming chunks so a missing or lying
/// length header still cannot drive unbounded memory growth.
async fn limited_upstream_bytes(
    mut response: reqwest::Response,
    max_bytes: u64,
) -> anyhow::Result<Bytes> {
    if response
        .content_length()
        .is_some_and(|content_length| content_length > max_bytes)
    {
        return Err(anyhow!("codex image upstream response exceeds {} bytes", max_bytes));
    }
    let mut body = BytesMut::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .context("read codex image upstream response chunk")?
    {
        let next_len = u64::try_from(body.len())
            .unwrap_or(u64::MAX)
            .saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX));
        if next_len > max_bytes {
            return Err(anyhow!("codex image upstream response exceeds {} bytes", max_bytes));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body.freeze())
}

fn upstream_response(
    status: StatusCode,
    headers: &reqwest::header::HeaderMap,
    bytes: Bytes,
) -> axum::response::Response {
    let mut builder = Response::builder().status(status);
    if let Some(content_type) = headers.get(reqwest::header::CONTENT_TYPE) {
        builder = builder.header(header::CONTENT_TYPE, content_type);
    }
    builder.body(Body::from(bytes)).unwrap_or_else(|_| {
        (StatusCode::BAD_GATEWAY, "failed to build upstream image response").into_response()
    })
}

#[derive(Debug, Default)]
struct ImageResponseMetrics {
    image_count: Option<u64>,
    image_bytes: Option<u64>,
}

fn response_image_metrics(value: &Value) -> ImageResponseMetrics {
    let Some(items) = value.get("data").and_then(Value::as_array) else {
        return ImageResponseMetrics::default();
    };
    let mut image_bytes = 0_u64;
    for item in items {
        if let Some(b64) = item.get("b64_json").and_then(Value::as_str) {
            image_bytes = image_bytes.saturating_add(approx_base64_bytes(b64));
        }
    }
    ImageResponseMetrics {
        image_count: Some(items.len() as u64),
        image_bytes: (image_bytes > 0).then_some(image_bytes),
    }
}

fn usage_tokens_from_response(value: &Value) -> Option<u64> {
    value
        .pointer("/usage/total_tokens")
        .and_then(Value::as_u64)
        .or_else(|| value.pointer("/usage/tokens").and_then(Value::as_u64))
}

fn approx_base64_bytes(value: &str) -> u64 {
    // base64 is ASCII, so count trailing '=' padding over bytes and skip char
    // decoding of a potentially multi-megabyte image payload.
    let padding = value.bytes().rev().take_while(|byte| *byte == b'=').count() as u64;
    ((value.len() as u64).saturating_mul(3) / 4).saturating_sub(padding)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CodexImageAuth {
    access_token: String,
    account_id: Option<String>,
    fedramp: bool,
}

fn parse_codex_image_auth(auth_json: &str) -> Result<CodexImageAuth, &'static str> {
    let value = serde_json::from_str::<Value>(auth_json).map_err(|_| "auth_invalid_json")?;
    let access_token = json_string_any(&value, &["access_token", "accessToken"]).or_else(|| {
        value
            .get("tokens")
            .and_then(|tokens| json_string_any(tokens, &["access_token", "accessToken"]))
    });
    let Some(access_token) = access_token else {
        return Err("auth_missing");
    };
    let account_id = json_string_any(&value, &["account_id", "accountId"]).or_else(|| {
        value
            .get("account")
            .and_then(|account| json_string_any(account, &["id", "account_id", "accountId"]))
    });
    let fedramp = value
        .get("is_fedramp_account")
        .or_else(|| value.get("isFedrampAccount"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Ok(CodexImageAuth {
        access_token,
        account_id,
        fedramp,
    })
}

fn json_string_any(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn access_token_expired(access_token: &str) -> bool {
    codex_access_token_expires_at_ms(Some(access_token))
        .is_some_and(|expires_at| i64::try_from(now_ms()).unwrap_or(i64::MAX) >= expires_at)
}

fn is_json_request(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.split(';').next().unwrap_or("").trim())
        .is_some_and(|value| value.eq_ignore_ascii_case("application/json"))
}

fn bearer_secret(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    (!token.is_empty()).then_some(token)
}

fn status_error_class(status: StatusCode) -> &'static str {
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        "auth"
    } else if status == StatusCode::TOO_MANY_REQUESTS {
        "rate_limited"
    } else if status.is_server_error() {
        "upstream_server"
    } else if status.is_client_error() {
        "client_error"
    } else {
        "upstream_status"
    }
}

fn endpoint_name(endpoint: crate::request::CodexImageEndpoint) -> &'static str {
    match endpoint {
        crate::request::CodexImageEndpoint::Generations => "generations",
        crate::request::CodexImageEndpoint::Edits => "edits",
    }
}

fn duration_ms(started: Instant) -> u64 {
    started.elapsed().as_millis() as u64
}

/// Build a process-unique request id for log correlation.
///
/// `now_ms()` alone collides for requests served within the same millisecond,
/// so a monotonic per-process counter is appended to keep ids unique.
fn new_request_id() -> String {
    static REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let sequence = REQUEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("codex-img-{}-{}-{}", std::process::id(), now_ms(), sequence)
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderMap;
    use llm_access_core::store::{AuthenticatedKey, KEY_STATUS_ACTIVE};
    use serde_json::json;

    use super::*;

    #[test]
    fn codex_image_auth_parser_accepts_flat_and_nested_shapes() {
        let flat = parse_codex_image_auth(
            r#"{
                "accessToken": " token-a ",
                "accountId": "acct-a",
                "isFedrampAccount": true
            }"#,
        )
        .expect("flat auth json");
        assert_eq!(flat.access_token, "token-a");
        assert_eq!(flat.account_id.as_deref(), Some("acct-a"));
        assert!(flat.fedramp);

        let nested = parse_codex_image_auth(
            r#"{
                "tokens": { "access_token": "token-b" },
                "account": { "id": "acct-b" }
            }"#,
        )
        .expect("nested auth json");
        assert_eq!(nested.access_token, "token-b");
        assert_eq!(nested.account_id.as_deref(), Some("acct-b"));
        assert!(!nested.fedramp);

        assert_eq!(
            parse_codex_image_auth(r#"{"accountId":"acct"}"#)
                .expect_err("auth without a token must be rejected"),
            "auth_missing"
        );
        assert_eq!(
            parse_codex_image_auth("not-json").expect_err("non-json auth must be rejected"),
            "auth_invalid_json"
        );
    }

    #[test]
    fn response_metrics_and_usage_share_one_parsed_json_value() {
        let value = json!({
            "data": [
                { "b64_json": "QUJDRA==" },
                { "b64_json": "QUI=" }
            ],
            "usage": { "total_tokens": 42 }
        });

        let metrics = response_image_metrics(&value);
        assert_eq!(metrics.image_count, Some(2));
        assert_eq!(metrics.image_bytes, Some(6));
        assert_eq!(usage_tokens_from_response(&value), Some(42));
    }

    #[test]
    fn image_request_header_helpers_are_strict() {
        let mut headers = HeaderMap::new();
        assert!(!is_json_request(&headers));
        headers.insert(
            header::CONTENT_TYPE,
            "application/json; charset=utf-8".parse().expect("content-type header"),
        );
        assert!(is_json_request(&headers));

        assert_eq!(bearer_secret(&headers), None);
        headers.insert(
            header::AUTHORIZATION,
            "Bearer secret-1".parse().expect("authorization header"),
        );
        assert_eq!(bearer_secret(&headers), Some("secret-1"));
        headers.insert(
            header::AUTHORIZATION,
            "Basic secret-1".parse().expect("authorization header"),
        );
        assert_eq!(bearer_secret(&headers), None);
    }

    #[test]
    fn reject_key_enforces_status_provider_and_quota_gate() {
        let active = AuthenticatedKey {
            key_id: "key-1".to_string(),
            key_name: "Key One".to_string(),
            provider_type: "codex".to_string(),
            protocol_family: "openai".to_string(),
            status: KEY_STATUS_ACTIVE.to_string(),
            quota_billable_limit: 10,
            billable_tokens_used: 0,
        };
        assert!(reject_key(&active).is_none());

        let mut disabled = active.clone();
        disabled.status = "disabled".to_string();
        assert!(reject_key(&disabled).is_some());

        let mut wrong_provider = active.clone();
        wrong_provider.provider_type = "kiro".to_string();
        assert!(reject_key(&wrong_provider).is_some());

        let mut exhausted = active;
        exhausted.billable_tokens_used = 10;
        assert!(reject_key(&exhausted).is_some());
    }

    #[test]
    fn shared_codex_upstream_helpers_cover_backend_and_v1_bases() {
        assert_eq!(
            llm_access_codex::request::compute_codex_upstream_url(
                "https://chatgpt.com/backend-api/codex",
                "/v1/images/generations",
            ),
            "https://chatgpt.com/backend-api/codex/images/generations"
        );
        assert_eq!(
            llm_access_codex::request::compute_codex_upstream_url(
                "https://api.example.com/v1",
                "/v1/images/edits",
            ),
            "https://api.example.com/v1/images/edits"
        );
        assert_eq!(
            llm_access_codex::request::normalize_codex_client_version(" 0.142.0 "),
            Some("0.142.0".to_string())
        );
        assert_eq!(codex_user_agent("0.142.0"), "codex_cli_rs/0.142.0");
    }
}
