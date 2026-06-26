//! Standalone Codex image gateway executable.

use std::{
    collections::HashMap,
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Context};
use axum::{
    body::{to_bytes, Body, Bytes},
    extract::State,
    http::{header, HeaderMap, Method, Request, Response, StatusCode},
    response::IntoResponse,
    routing::get,
    Router,
};
use bytes::BytesMut;
use clap::{Parser, Subcommand};
use llm_access_codex_image::{
    dispatch::{eligible_image_routes, should_failover_status},
    limiter::{ImageAccountLimiter, ImageKeyLimitRejection, ImageKeyLimiter},
    logging::{
        build_image_log_event, ImageLogConfig, ImageLogInput, ImageLogWriter, UpstreamLogInput,
    },
    request::{normalize_image_gateway_path, parse_image_request, upstream_image_path},
};
use llm_access_core::{
    provider::{ProtocolFamily, ProviderType},
    store::{
        codex_access_token_expires_at_ms, AdminConfigStore, AuthenticatedKey, ControlStore,
        ProviderCodexRoute, ProviderProxyConfig, ProviderRouteStore, DEFAULT_CODEX_CLIENT_VERSION,
        KEY_STATUS_ACTIVE,
    },
};
use llm_access_store::{postgres::PostgresControlRepository, request_cache::RequestCacheConfig};
use serde_json::Value;
use tokio::net::TcpListener;

const DEFAULT_BIND_ADDR: &str = "127.0.0.1:19082";
const DEFAULT_CONTROL_DATABASE_URL_ENV: &str = "LLM_ACCESS_CODEX_IMAGE_CONTROL_DATABASE_URL";
const DEFAULT_REQUEST_CACHE_URL_ENV: &str = "LLM_ACCESS_REQUEST_CACHE_URL";
const DEFAULT_REQUEST_CACHE_KEY_PREFIX: &str = "llma";
const MAX_IMAGE_REQUEST_BODY_BYTES: usize = 24 * 1024 * 1024;
const MAX_IMAGE_UPSTREAM_RESPONSE_BODY_BYTES: u64 = 96 * 1024 * 1024;

#[derive(Debug, Parser)]
#[command(name = "llm-access-codex-image")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Serve(ServeArgs),
}

#[derive(Debug, Parser)]
struct ServeArgs {
    #[arg(long, default_value = DEFAULT_BIND_ADDR)]
    bind: SocketAddr,
    #[arg(long)]
    state_root: PathBuf,
    #[arg(long, default_value = DEFAULT_CONTROL_DATABASE_URL_ENV)]
    postgres_control_database_url_env: String,
    #[arg(long, default_value = DEFAULT_REQUEST_CACHE_URL_ENV)]
    request_cache_url_env: String,
    #[arg(long, default_value = DEFAULT_REQUEST_CACHE_KEY_PREFIX)]
    request_cache_key_prefix: String,
    #[arg(long)]
    image_log_dir: Option<PathBuf>,
}

#[derive(Clone)]
struct AppState {
    control: Arc<PostgresControlRepository>,
    limiter: Arc<ImageAccountLimiter>,
    key_limiter: Arc<ImageKeyLimiter>,
    image_log: Arc<Mutex<ImageLogWriter>>,
    default_client: reqwest::Client,
    proxy_clients: Arc<Mutex<HashMap<String, reqwest::Client>>>,
    upstream_base: String,
    codex_client_version: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| {
            "warn,llm_access_codex_image=info,llm_access_store=info".to_string()
        }))
        .try_init();
    match Cli::parse().command {
        Command::Serve(args) => serve(args).await,
    }
}

async fn serve(args: ServeArgs) -> anyhow::Result<()> {
    let database_url =
        std::env::var(&args.postgres_control_database_url_env).with_context(|| {
            format!("missing control database env `{}`", args.postgres_control_database_url_env)
        })?;
    let request_cache_config = request_cache_config(&args)?;
    let control = Arc::new(
        PostgresControlRepository::connect_read_only(&database_url, request_cache_config)
            .await
            .context("connect read-only postgres control repository")?,
    );
    control
        .verify_codex_image_gateway_schema()
        .await
        .context("verify codex image gateway control-plane schema")?;
    let runtime_config = control
        .get_admin_runtime_config()
        .await
        .context("load runtime config")?;
    let image_log_dir = args
        .image_log_dir
        .unwrap_or_else(|| args.state_root.join("codex-image-logs"));
    let image_log = ImageLogWriter::new(ImageLogConfig {
        log_dir: image_log_dir,
        max_file_bytes: runtime_config.usage_journal_max_file_bytes,
        max_file_age_ms: runtime_config.usage_journal_max_file_age_ms,
        max_files: usize::try_from(runtime_config.usage_journal_max_files.max(1))
            .unwrap_or(usize::MAX),
    })?;
    let state = Arc::new(AppState {
        control,
        limiter: Arc::new(ImageAccountLimiter::default()),
        key_limiter: Arc::new(ImageKeyLimiter::default()),
        image_log: Arc::new(Mutex::new(image_log)),
        default_client: provider_client(None)?,
        proxy_clients: Arc::new(Mutex::new(HashMap::new())),
        upstream_base: llm_access_codex::request::codex_upstream_base_url_from_env(),
        codex_client_version: llm_access_codex::request::normalize_codex_client_version(
            &runtime_config.codex_client_version,
        )
        .unwrap_or_else(|| DEFAULT_CODEX_CLIENT_VERSION.to_string()),
    });
    let app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .fallback(handle_image_request)
        .with_state(state);
    let listener = TcpListener::bind(args.bind)
        .await
        .with_context(|| format!("bind codex image gateway on {}", args.bind))?;
    tracing::info!(bind = %args.bind, "codex image gateway listening");
    axum::serve(listener, app)
        .await
        .context("serve codex image gateway")
}

fn request_cache_config(args: &ServeArgs) -> anyhow::Result<Option<RequestCacheConfig>> {
    let Some(url) = std::env::var(&args.request_cache_url_env)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    let key_prefix = args.request_cache_key_prefix.trim();
    if key_prefix.is_empty() {
        return Err(anyhow!("--request-cache-key-prefix cannot be empty"));
    }
    Ok(Some(RequestCacheConfig {
        url,
        key_prefix: key_prefix.to_string(),
    }))
}

async fn handle_image_request(
    State(state): State<Arc<AppState>>,
    request: Request<Body>,
) -> axum::response::Response {
    let request_id = new_request_id();
    let started = Instant::now();
    let path = request.uri().path().to_string();
    let Some(endpoint) = normalize_image_gateway_path(&path) else {
        return (StatusCode::NOT_FOUND, "unsupported image gateway path").into_response();
    };
    if request.method() != Method::POST {
        return (StatusCode::METHOD_NOT_ALLOWED, "image endpoint requires POST").into_response();
    }
    if !is_json_request(request.headers()) {
        return (StatusCode::UNSUPPORTED_MEDIA_TYPE, "image endpoint requires JSON")
            .into_response();
    }
    let headers = request.headers().clone();
    let Some(secret) = bearer_secret(&headers) else {
        return (StatusCode::UNAUTHORIZED, "missing bearer token").into_response();
    };
    let key = match state.control.authenticate_bearer_secret(secret).await {
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
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid image request body").into_response(),
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
    let response = dispatch_image_request(state.as_ref(), ImageDispatchRequest {
        request_id: &request_id,
        started,
        headers: &headers,
        key: &key,
        endpoint_name: endpoint_name(endpoint),
        upstream_path: upstream_image_path(endpoint),
        image_request,
    })
    .await;
    response
}

struct ImageDispatchRequest<'a> {
    request_id: &'a str,
    started: Instant,
    headers: &'a HeaderMap,
    key: &'a AuthenticatedKey,
    endpoint_name: &'static str,
    upstream_path: &'a str,
    image_request: llm_access_codex_image::request::CodexImageRequest,
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

async fn dispatch_image_request(
    state: &AppState,
    ctx: ImageDispatchRequest<'_>,
) -> axum::response::Response {
    let candidates = match state.control.resolve_codex_route_candidates(ctx.key).await {
        Ok(candidates) => candidates,
        Err(err) => {
            tracing::warn!(error = %err, "codex image route lookup failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "route store error").into_response();
        },
    };
    let candidates = match eligible_image_routes(candidates) {
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
        match state
            .key_limiter
            .try_acquire(&ctx.key.key_id, key_limits.0, key_limits.1)
        {
            Ok(permit) => permit,
            Err(rejection) => {
                log_image_event(
                    state,
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
                return image_key_limit_response(&rejection);
            },
        };
    let mut failover_count = 0_u64;
    let mut concurrency_blocked = 0_usize;
    let mut last_error_class = None::<String>;
    for candidate in candidates {
        let Some(route) = hydrate_codex_image_route(state, candidate).await else {
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
        let Some(_permit) = state.limiter.try_acquire(
            &route.account_name,
            Some(route.account_codex_image_generation_max_concurrency),
        ) else {
            concurrency_blocked += 1;
            last_error_class = Some("concurrency_blocked".to_string());
            continue;
        };
        let client = match provider_client_for_route(state, route.proxy.as_ref()) {
            Ok(client) => client,
            Err(err) => {
                tracing::warn!(account = %route.account_name, error = %err, "codex image client build failed");
                failover_count += 1;
                last_error_class = Some("proxy_client".to_string());
                continue;
            },
        };
        let upstream_url = llm_access_codex::request::compute_codex_upstream_url(
            &state.upstream_base,
            ctx.upstream_path,
        );
        let request_builder = add_image_upstream_headers(
            client.post(upstream_url),
            ctx.headers,
            &auth.access_token,
            auth.account_id,
            auth.fedramp,
            &state.codex_client_version,
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
        let bytes = match limited_upstream_bytes(upstream, MAX_IMAGE_UPSTREAM_RESPONSE_BODY_BYTES)
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
        log_image_event(
            state,
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
    log_image_event(
        state,
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
    (status, "all eligible codex image accounts failed for this request").into_response()
}

async fn hydrate_codex_image_route(
    state: &AppState,
    candidate: ProviderCodexRoute,
) -> Option<ProviderCodexRoute> {
    let hydrated = state
        .control
        .resolve_codex_account_route(&candidate.account_name)
        .await
        .ok()
        .flatten()?;
    Some(ProviderCodexRoute {
        account_name: candidate.account_name,
        account_group_id_at_event: candidate.account_group_id_at_event,
        route_strategy_at_event: candidate.route_strategy_at_event,
        auth_json: hydrated.auth_json,
        map_gpt53_codex_to_spark: candidate.map_gpt53_codex_to_spark,
        auth_refresh_enabled: candidate.auth_refresh_enabled,
        codex_fast_enabled: candidate.codex_fast_enabled,
        codex_strict_session_rejection_enabled: candidate.codex_strict_session_rejection_enabled,
        codex_image_generation_enabled: candidate.codex_image_generation_enabled,
        request_max_concurrency: candidate.request_max_concurrency,
        request_min_start_interval_ms: candidate.request_min_start_interval_ms,
        account_request_max_concurrency: candidate.account_request_max_concurrency,
        account_request_min_start_interval_ms: candidate.account_request_min_start_interval_ms,
        account_codex_image_generation_enabled: candidate.account_codex_image_generation_enabled,
        account_codex_image_generation_max_concurrency: candidate
            .account_codex_image_generation_max_concurrency,
        cached_error_message: candidate
            .cached_error_message
            .or(hydrated.cached_error_message),
        proxy: candidate.proxy.or(hydrated.proxy),
    })
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
            header_value(headers, header::USER_AGENT.as_str())
                .unwrap_or_else(|| codex_user_agent(codex_client_version)),
        )
        .header(
            reqwest::header::HeaderName::from_static("originator"),
            header_value(headers, "originator").unwrap_or_else(|| "codex_cli_rs".to_string()),
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
        if let Some(value) = header_value(headers, header_name) {
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

fn provider_client_for_route(
    state: &AppState,
    proxy: Option<&ProviderProxyConfig>,
) -> anyhow::Result<reqwest::Client> {
    let Some(proxy_config) = proxy else {
        return Ok(state.default_client.clone());
    };
    let cache_key = proxy_client_key(proxy_config);
    let mut clients = state
        .proxy_clients
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(client) = clients.get(&cache_key) {
        return Ok(client.clone());
    }
    let client = provider_client(Some(proxy_config))?;
    clients.insert(cache_key, client.clone());
    Ok(client)
}

fn proxy_client_key(proxy_config: &ProviderProxyConfig) -> String {
    format!(
        "{}\0{}\0{}",
        proxy_config.proxy_url,
        proxy_config.proxy_username.as_deref().unwrap_or_default(),
        proxy_config.proxy_password.as_deref().unwrap_or_default()
    )
}

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

fn log_image_event(
    state: &AppState,
    request_id: &str,
    key: &AuthenticatedKey,
    account_name: Option<&str>,
    endpoint: &str,
    image_request: &llm_access_codex_image::request::CodexImageRequest,
    upstream: UpstreamLogInput<'_>,
) {
    let input_images = image_request
        .images
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let event = build_image_log_event(ImageLogInput {
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
    let image_log = Arc::clone(&state.image_log);
    let handle = tokio::task::spawn_blocking(move || {
        let mut writer = image_log
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Err(err) = writer.append(&event) {
            tracing::warn!(error = %err, "codex image request log write failed");
        }
    });
    drop(handle);
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
    let padding = value.chars().rev().take_while(|ch| *ch == '=').count() as u64;
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

fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn codex_user_agent(client_version: &str) -> String {
    llm_access_codex::request::codex_user_agent(client_version)
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

fn endpoint_name(endpoint: llm_access_codex_image::request::CodexImageEndpoint) -> &'static str {
    match endpoint {
        llm_access_codex_image::request::CodexImageEndpoint::Generations => "generations",
        llm_access_codex_image::request::CodexImageEndpoint::Edits => "edits",
    }
}

fn duration_ms(started: Instant) -> u64 {
    started.elapsed().as_millis() as u64
}

fn new_request_id() -> String {
    format!("codex-img-{}-{}", std::process::id(), now_ms())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
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

        assert_eq!(parse_codex_image_auth(r#"{"accountId":"acct"}"#).unwrap_err(), "auth_missing");
        assert_eq!(parse_codex_image_auth("not-json").unwrap_err(), "auth_invalid_json");
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
        headers.insert(header::CONTENT_TYPE, "application/json; charset=utf-8".parse().unwrap());
        assert!(is_json_request(&headers));

        assert_eq!(bearer_secret(&headers), None);
        headers.insert(header::AUTHORIZATION, "Bearer secret-1".parse().unwrap());
        assert_eq!(bearer_secret(&headers), Some("secret-1"));
        headers.insert(header::AUTHORIZATION, "Basic secret-1".parse().unwrap());
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
