use std::{
    env,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result};
use axum::{
    extract::{Query, State},
    http::{HeaderMap, Method, StatusCode},
    Json,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    handlers::{ensure_admin_access, ErrorResponse},
    state::AppState,
};

const DEFAULT_CONFIG_PATH: &str = "conf/gpt2api-rs.json";
const DEFAULT_TIMEOUT_SECONDS: u64 = 60;
const MAX_TIMEOUT_SECONDS: u64 = 300;

type HandlerResult<T> = Result<T, (StatusCode, Json<ErrorResponse>)>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Gpt2ApiRsConfig {
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub admin_token: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
}

impl Default for Gpt2ApiRsConfig {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            admin_token: String::new(),
            api_key: String::new(),
            timeout_seconds: default_timeout_seconds(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminGpt2ApiRsConfigEnvelope {
    pub config_path: String,
    pub configured: bool,
    pub config: Gpt2ApiRsConfig,
}

#[derive(Debug, Deserialize)]
pub struct UsageLimitQuery {
    #[serde(default)]
    pub limit: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct AdminImageEditRequest {
    pub prompt: String,
    #[serde(default = "default_image_model")]
    pub model: String,
    #[serde(default = "default_image_count")]
    pub n: usize,
    pub image_base64: String,
    #[serde(default)]
    pub file_name: Option<String>,
    #[serde(default)]
    pub mime_type: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Gpt2ApiRsState {
    config_path: PathBuf,
    config: Arc<RwLock<Gpt2ApiRsConfig>>,
    client: reqwest::Client,
}

#[derive(Clone, Copy)]
enum TokenScope {
    Admin,
    Public,
}

impl Gpt2ApiRsState {
    pub async fn load_from_env() -> Result<Self> {
        let config_path = resolve_config_path();
        let config = load_config_or_default(&config_path).await?;
        let normalized = normalize_config(config)?;
        Ok(Self {
            config_path,
            config: Arc::new(RwLock::new(normalized)),
            client: reqwest::Client::builder()
                .build()
                .context("failed to build gpt2api-rs admin client")?,
        })
    }

    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    pub fn snapshot(&self) -> Gpt2ApiRsConfig {
        self.config.read().clone()
    }

    pub async fn replace(&self, next: Gpt2ApiRsConfig) -> Result<Gpt2ApiRsConfig> {
        let normalized = normalize_config(next)?;
        save_config(&self.config_path, &normalized).await?;
        *self.config.write() = normalized.clone();
        Ok(normalized)
    }
}

pub async fn get_admin_config(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> HandlerResult<Json<AdminGpt2ApiRsConfigEnvelope>> {
    ensure_admin_access(&state, &headers)?;
    Ok(Json(config_envelope(state.gpt2api_rs.as_ref())))
}

pub async fn update_admin_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<Gpt2ApiRsConfig>,
) -> HandlerResult<Json<AdminGpt2ApiRsConfigEnvelope>> {
    ensure_admin_access(&state, &headers)?;
    state
        .gpt2api_rs
        .replace(request)
        .await
        .map_err(internal_error)?;
    Ok(Json(config_envelope(state.gpt2api_rs.as_ref())))
}

pub async fn get_admin_status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    let config = state.gpt2api_rs.snapshot();
    if !is_configured(&config) {
        return Ok(Json(json!({
            "configured": false,
            "config_path": state.gpt2api_rs.config_path().display().to_string(),
        })));
    }
    let mut payload = proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Admin,
        Method::GET,
        "/admin/status",
        None,
        None,
    )
    .await?
    .0;
    if let Some(map) = payload.as_object_mut() {
        map.insert("configured".to_string(), Value::Bool(true));
        map.insert(
            "config_path".to_string(),
            Value::String(state.gpt2api_rs.config_path().display().to_string()),
        );
    }
    Ok(Json(payload))
}

pub async fn get_public_version(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Public,
        Method::GET,
        "/version",
        None,
        None,
    )
    .await
}

pub async fn get_public_models(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Public,
        Method::GET,
        "/v1/models",
        None,
        None,
    )
    .await
}

pub async fn post_public_login(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Public,
        Method::POST,
        "/auth/login",
        None,
        Some(Value::Object(serde_json::Map::new())),
    )
    .await
}

pub async fn list_admin_accounts(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Admin,
        Method::GET,
        "/admin/accounts",
        None,
        None,
    )
    .await
}

pub async fn import_admin_accounts(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Admin,
        Method::POST,
        "/admin/accounts/import",
        None,
        Some(request),
    )
    .await
}

pub async fn delete_admin_accounts(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Admin,
        Method::DELETE,
        "/admin/accounts",
        None,
        Some(request),
    )
    .await
}

pub async fn refresh_admin_accounts(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Admin,
        Method::POST,
        "/admin/accounts/refresh",
        None,
        Some(request),
    )
    .await
}

pub async fn update_admin_account(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Admin,
        Method::POST,
        "/admin/accounts/update",
        None,
        Some(request),
    )
    .await
}

pub async fn list_admin_keys(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Admin,
        Method::GET,
        "/admin/keys",
        None,
        None,
    )
    .await
}

pub async fn list_admin_usage(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<UsageLimitQuery>,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    let limit = query.limit.unwrap_or(50);
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Admin,
        Method::GET,
        "/admin/usage",
        Some(&[("limit", limit.to_string())]),
        None,
    )
    .await
}

pub async fn post_image_generation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Public,
        Method::POST,
        "/v1/images/generations",
        None,
        Some(request),
    )
    .await
}

pub async fn post_image_edit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<AdminImageEditRequest>,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    let config = state.gpt2api_rs.snapshot();
    let url = configured_url(&config, "/v1/images/edits").map_err(bad_request)?;
    let bearer = configured_token(&config, TokenScope::Public).map_err(bad_request)?;
    let image_bytes = BASE64
        .decode(request.image_base64.trim())
        .map_err(|err| bad_request(format!("invalid image_base64: {err}")))?;
    let file_name = request
        .file_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("image.png")
        .to_string();
    let mime_type = request
        .mime_type
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("image/png")
        .to_string();
    let form = reqwest::multipart::Form::new()
        .text("prompt", request.prompt.trim().to_string())
        .text("model", request.model.trim().to_string())
        .text("n", request.n.to_string())
        .part(
            "image",
            reqwest::multipart::Part::bytes(image_bytes)
                .file_name(file_name)
                .mime_str(&mime_type)
                .map_err(internal_error)?,
        );
    let response = state
        .gpt2api_rs
        .client
        .post(url)
        .timeout(Duration::from_secs(config.timeout_seconds))
        .bearer_auth(bearer)
        .multipart(form)
        .send()
        .await
        .map_err(bad_gateway)?;
    decode_json_response(response).await
}

pub async fn post_chat_completions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Public,
        Method::POST,
        "/v1/chat/completions",
        None,
        Some(request),
    )
    .await
}

pub async fn post_responses(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Public,
        Method::POST,
        "/v1/responses",
        None,
        Some(request),
    )
    .await
}

fn config_envelope(state: &Gpt2ApiRsState) -> AdminGpt2ApiRsConfigEnvelope {
    let config = state.snapshot();
    AdminGpt2ApiRsConfigEnvelope {
        config_path: state.config_path().display().to_string(),
        configured: is_configured(&config),
        config,
    }
}

async fn proxy_json_request(
    state: &Gpt2ApiRsState,
    scope: TokenScope,
    method: Method,
    path: &str,
    query: Option<&[(&str, String)]>,
    body: Option<Value>,
) -> HandlerResult<Json<Value>> {
    let config = state.snapshot();
    let url = configured_url(&config, path).map_err(bad_request)?;
    let bearer = configured_token(&config, scope).map_err(bad_request)?;
    let mut request = state
        .client
        .request(method, url)
        .timeout(Duration::from_secs(config.timeout_seconds))
        .bearer_auth(bearer);
    if let Some(query) = query {
        request = request.query(query);
    }
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await.map_err(bad_gateway)?;
    decode_json_response(response).await
}

async fn decode_json_response(response: reqwest::Response) -> HandlerResult<Json<Value>> {
    let status = StatusCode::from_u16(response.status().as_u16())
        .map_err(|err| internal_error(format!("invalid upstream status: {err}")))?;
    let bytes = response.bytes().await.map_err(bad_gateway)?;
    if status.is_success() {
        let payload = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice::<Value>(&bytes)
                .map_err(|err| internal_error(format!("failed to decode upstream json: {err}")))?
        };
        return Ok(Json(payload));
    }
    let payload = serde_json::from_slice::<ErrorResponse>(&bytes).unwrap_or_else(|_| {
        let message = String::from_utf8_lossy(&bytes).trim().to_string();
        ErrorResponse {
            error: if message.is_empty() {
                "gpt2api-rs request failed".to_string()
            } else {
                message
            },
            code: status.as_u16(),
        }
    });
    Err((status, Json(payload)))
}

fn configured_url(config: &Gpt2ApiRsConfig, path: &str) -> Result<String> {
    let base = config.base_url.trim();
    if base.is_empty() {
        anyhow::bail!("gpt2api-rs base_url is empty");
    }
    let path = path.trim();
    if !path.starts_with('/') {
        anyhow::bail!("gpt2api-rs relative path must start with `/`");
    }
    Ok(format!("{base}{path}"))
}

fn configured_token(config: &Gpt2ApiRsConfig, scope: TokenScope) -> Result<String> {
    let value = match scope {
        TokenScope::Admin => config.admin_token.trim(),
        TokenScope::Public => config.api_key.trim(),
    };
    if value.is_empty() {
        let label = match scope {
            TokenScope::Admin => "admin_token",
            TokenScope::Public => "api_key",
        };
        anyhow::bail!("gpt2api-rs {label} is empty");
    }
    Ok(value.to_string())
}

fn is_configured(config: &Gpt2ApiRsConfig) -> bool {
    !config.base_url.trim().is_empty()
        && !config.admin_token.trim().is_empty()
        && !config.api_key.trim().is_empty()
}

fn default_timeout_seconds() -> u64 {
    DEFAULT_TIMEOUT_SECONDS
}

fn default_image_model() -> String {
    "gpt-image-1".to_string()
}

const fn default_image_count() -> usize {
    1
}

fn resolve_config_path() -> PathBuf {
    if let Ok(raw) = env::var("GPT2API_RS_CONFIG") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    PathBuf::from(DEFAULT_CONFIG_PATH)
}

async fn load_config_or_default(path: &Path) -> Result<Gpt2ApiRsConfig> {
    match tokio::fs::read_to_string(path).await {
        Ok(raw) => serde_json::from_str::<Gpt2ApiRsConfig>(&raw)
            .with_context(|| format!("invalid gpt2api-rs config json: {}", path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Gpt2ApiRsConfig::default()),
        Err(err) => Err(err).with_context(|| format!("failed to read {}", path.display())),
    }
}

async fn save_config(path: &Path, config: &Gpt2ApiRsConfig) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create config dir {}", parent.display()))?;
    }
    let content = serde_json::to_string_pretty(config)?;
    tokio::fs::write(path, format!("{content}\n"))
        .await
        .with_context(|| format!("failed to write {}", path.display()))
}

fn normalize_config(mut config: Gpt2ApiRsConfig) -> Result<Gpt2ApiRsConfig> {
    config.base_url = config.base_url.trim().trim_end_matches('/').to_string();
    config.admin_token = config.admin_token.trim().to_string();
    config.api_key = config.api_key.trim().to_string();
    if !config.base_url.is_empty() {
        let parsed = reqwest::Url::parse(&config.base_url)
            .with_context(|| format!("invalid gpt2api-rs base_url: {}", config.base_url))?;
        match parsed.scheme() {
            "http" | "https" => {},
            scheme => {
                anyhow::bail!("gpt2api-rs base_url scheme must be http/https, got `{scheme}`")
            },
        }
    }
    config.timeout_seconds = config.timeout_seconds.clamp(1, MAX_TIMEOUT_SECONDS);
    Ok(config)
}

fn bad_request(err: impl std::fmt::Display) -> (StatusCode, Json<ErrorResponse>) {
    error_response(StatusCode::BAD_REQUEST, err.to_string())
}

fn bad_gateway(err: impl std::fmt::Display) -> (StatusCode, Json<ErrorResponse>) {
    tracing::error!("gpt2api-rs upstream error: {err}");
    error_response(StatusCode::BAD_GATEWAY, "gpt2api-rs service is unavailable".to_string())
}

fn internal_error(err: impl std::fmt::Display) -> (StatusCode, Json<ErrorResponse>) {
    tracing::error!("gpt2api-rs internal error: {err}");
    error_response(StatusCode::INTERNAL_SERVER_ERROR, "gpt2api-rs proxy failed".to_string())
}

fn error_response(
    status: StatusCode,
    message: impl Into<String>,
) -> (StatusCode, Json<ErrorResponse>) {
    (
        status,
        Json(ErrorResponse {
            error: message.into(),
            code: status.as_u16(),
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_config_trims_and_clamps_timeout() {
        let config = normalize_config(Gpt2ApiRsConfig {
            base_url: " https://example.com/root/ ".to_string(),
            admin_token: " admin ".to_string(),
            api_key: " key ".to_string(),
            timeout_seconds: 999,
        })
        .expect("config should normalize");
        assert_eq!(config.base_url, "https://example.com/root");
        assert_eq!(config.admin_token, "admin");
        assert_eq!(config.api_key, "key");
        assert_eq!(config.timeout_seconds, MAX_TIMEOUT_SECONDS);
    }

    #[tokio::test]
    async fn save_and_load_round_trip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("gpt2api-rs.json");
        let config = Gpt2ApiRsConfig {
            base_url: "http://127.0.0.1:8787".to_string(),
            admin_token: "admin-token".to_string(),
            api_key: "public-key".to_string(),
            timeout_seconds: 42,
        };
        save_config(&path, &config).await.expect("save");
        let loaded = load_config_or_default(&path).await.expect("load");
        assert_eq!(loaded, config);
    }
}
