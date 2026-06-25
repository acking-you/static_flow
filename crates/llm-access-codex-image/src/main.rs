//! Standalone Codex image gateway executable.

use std::{
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
use clap::{Parser, Subcommand};
use llm_access_codex_image::{
    dispatch::{eligible_image_routes, should_failover_status},
    limiter::ImageAccountLimiter,
    logging::{
        build_image_log_event, ImageLogConfig, ImageLogInput, ImageLogWriter, UpstreamLogInput,
    },
    request::{normalize_image_gateway_path, parse_image_request, upstream_image_path},
};
use llm_access_core::{
    provider::{ProtocolFamily, ProviderType},
    store::{
        codex_auth_access_token_expires_at_ms, AdminConfigStore, AuthenticatedKey, ControlStore,
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
const DEFAULT_UPSTREAM_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
const MAX_IMAGE_REQUEST_BODY_BYTES: usize = 24 * 1024 * 1024;

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
    image_log: Arc<Mutex<ImageLogWriter>>,
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
        image_log: Arc::new(Mutex::new(image_log)),
        upstream_base: codex_upstream_base_url(),
        codex_client_version: normalize_codex_client_version(&runtime_config.codex_client_version)
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
    let mut failover_count = 0_u64;
    let mut concurrency_blocked = 0_usize;
    let mut last_error_class = None::<String>;
    for candidate in candidates {
        let Some(route) = hydrate_codex_image_route(state, candidate).await else {
            failover_count += 1;
            last_error_class = Some("auth_missing".to_string());
            continue;
        };
        if access_token_expired(&route.auth_json) {
            failover_count += 1;
            last_error_class = Some("auth_expired".to_string());
            continue;
        }
        let Some(access_token) = access_token_from_auth_json(&route.auth_json) else {
            failover_count += 1;
            last_error_class = Some("auth_missing".to_string());
            continue;
        };
        let Some(_permit) = state.limiter.try_acquire(
            &route.account_name,
            Some(route.account_codex_image_generation_max_concurrency),
        ) else {
            concurrency_blocked += 1;
            continue;
        };
        let client = match provider_client(route.proxy.as_ref()) {
            Ok(client) => client,
            Err(err) => {
                tracing::warn!(account = %route.account_name, error = %err, "codex image client build failed");
                failover_count += 1;
                last_error_class = Some("proxy_client".to_string());
                continue;
            },
        };
        let upstream_url = compute_codex_upstream_url(&state.upstream_base, ctx.upstream_path);
        let request_builder = add_image_upstream_headers(
            client.post(upstream_url),
            ctx.headers,
            &access_token,
            account_id_from_auth_json(&route.auth_json),
            fedramp_from_auth_json(&route.auth_json),
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
        let bytes = match upstream.bytes().await {
            Ok(bytes) => bytes,
            Err(err) => {
                tracing::warn!(account = %route.account_name, error = %err, "codex image upstream response read failed");
                failover_count += 1;
                last_error_class = Some("upstream_body".to_string());
                continue;
            },
        };
        let metrics = response_image_metrics(&bytes);
        let usage_tokens = usage_tokens_from_response(&bytes);
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
    let status = if concurrency_blocked > 0 {
        StatusCode::TOO_MANY_REQUESTS
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
    if let Err(err) = state
        .image_log
        .lock()
        .expect("codex image log mutex poisoned")
        .append(&event)
    {
        tracing::warn!(error = %err, "codex image request log write failed");
    }
}

#[derive(Debug, Default)]
struct ImageResponseMetrics {
    image_count: Option<u64>,
    image_bytes: Option<u64>,
}

fn response_image_metrics(bytes: &[u8]) -> ImageResponseMetrics {
    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
        return ImageResponseMetrics::default();
    };
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

fn usage_tokens_from_response(bytes: &[u8]) -> Option<u64> {
    let value = serde_json::from_slice::<Value>(bytes).ok()?;
    value
        .pointer("/usage/total_tokens")
        .and_then(Value::as_u64)
        .or_else(|| value.pointer("/usage/tokens").and_then(Value::as_u64))
}

fn approx_base64_bytes(value: &str) -> u64 {
    let padding = value.chars().rev().take_while(|ch| *ch == '=').count() as u64;
    ((value.len() as u64).saturating_mul(3) / 4).saturating_sub(padding)
}

fn access_token_from_auth_json(auth_json: &str) -> Option<String> {
    let value = serde_json::from_str::<Value>(auth_json).ok()?;
    json_string_any(&value, &["access_token", "accessToken"]).or_else(|| {
        value
            .get("tokens")
            .and_then(|tokens| json_string_any(tokens, &["access_token", "accessToken"]))
    })
}

fn account_id_from_auth_json(auth_json: &str) -> Option<String> {
    let value = serde_json::from_str::<Value>(auth_json).ok()?;
    json_string_any(&value, &["account_id", "accountId"]).or_else(|| {
        value
            .get("account")
            .and_then(|account| json_string_any(account, &["id", "account_id", "accountId"]))
    })
}

fn fedramp_from_auth_json(auth_json: &str) -> bool {
    serde_json::from_str::<Value>(auth_json)
        .ok()
        .and_then(|value| {
            value
                .get("is_fedramp_account")
                .or_else(|| value.get("isFedrampAccount"))
                .and_then(Value::as_bool)
        })
        .unwrap_or(false)
}

fn json_string_any(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn access_token_expired(auth_json: &str) -> bool {
    codex_auth_access_token_expires_at_ms(auth_json)
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

fn codex_upstream_base_url() -> String {
    std::env::var("CODEX_UPSTREAM_BASE_URL")
        .or_else(|_| std::env::var("STATICFLOW_LLM_GATEWAY_UPSTREAM_BASE_URL"))
        .map(|value| llm_access_codex::request::normalize_upstream_base_url(&value))
        .unwrap_or_else(|_| DEFAULT_UPSTREAM_BASE_URL.to_string())
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
    if trimmed.is_empty() || trimmed.len() > 64 {
        return None;
    }
    trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_'))
        .then(|| trimmed.to_string())
}

fn codex_user_agent(client_version: &str) -> String {
    format!("codex_cli_rs/{client_version}")
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
