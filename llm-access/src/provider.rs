//! Provider-facing HTTP entrypoints for `llm-access`.

use std::sync::Arc;

use async_trait::async_trait;
use axum::{
    body::{to_bytes, Body},
    extract::State,
    http::{header, HeaderMap, Method, Request, StatusCode},
    response::{IntoResponse, Response},
};
use llm_access_core::{
    provider::{ProtocolFamily, ProviderType},
    routes::provider_route_requirement,
    store::{AuthenticatedKey, ControlStore, ProviderRouteStore},
};
use serde_json::Value;

const MAX_PROVIDER_PROXY_BODY_BYTES: usize = 32 * 1024 * 1024;

/// Shared provider request state.
#[derive(Clone)]
pub struct ProviderState {
    control_store: Arc<dyn ControlStore>,
    route_store: Arc<dyn ProviderRouteStore>,
    dispatcher: Arc<dyn ProviderDispatcher>,
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
        route_store: Arc<dyn ProviderRouteStore>,
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
    ) -> Response {
        if should_serve_local_codex_models(&key, &request) {
            return local_codex_models_response();
        }
        if ProviderType::from_storage_str(&key.provider_type) == Some(ProviderType::Codex) {
            return dispatch_codex_proxy(key, request, route_store).await;
        }
        (StatusCode::NOT_IMPLEMENTED, "provider dispatch is not wired").into_response()
    }
}

async fn dispatch_codex_proxy(
    key: AuthenticatedKey,
    request: Request<Body>,
    route_store: Arc<dyn ProviderRouteStore>,
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
    let upstream_path = codex_upstream_path_and_query(request.uri().path(), request.uri().query());
    let upstream_base = std::env::var("CODEX_UPSTREAM_BASE_URL")
        .map(|value| llm_access_codex::request::normalize_upstream_base_url(&value))
        .unwrap_or_else(|_| "https://chatgpt.com/backend-api/codex".to_string());
    let upstream_url = format!("{}{}", upstream_base.trim_end_matches('/'), upstream_path);
    let method = match reqwest::Method::from_bytes(request.method().as_str().as_bytes()) {
        Ok(method) => method,
        Err(_) => return (StatusCode::METHOD_NOT_ALLOWED, "unsupported method").into_response(),
    };
    let request_headers = request.headers().clone();
    let body = match to_bytes(request.into_body(), MAX_PROVIDER_PROXY_BODY_BYTES).await {
        Ok(body) => body,
        Err(_) => return (StatusCode::BAD_REQUEST, "request body is too large").into_response(),
    };
    let client = reqwest::Client::new();
    let mut upstream = client
        .request(method, upstream_url)
        .bearer_auth(access_token)
        .body(body.to_vec());
    for name in [header::CONTENT_TYPE, header::ACCEPT, header::ACCEPT_LANGUAGE, header::USER_AGENT]
    {
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
    let status = response.status();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .cloned();
    let bytes = match response.bytes().await {
        Ok(bytes) => bytes,
        Err(_) => {
            return (StatusCode::BAD_GATEWAY, "codex upstream response read failed").into_response()
        },
    };
    let mut builder = Response::builder().status(status);
    if let Some(content_type) = content_type {
        builder = builder.header(header::CONTENT_TYPE, content_type.as_bytes());
    }
    builder.body(Body::from(bytes)).unwrap_or_else(|_| {
        (StatusCode::BAD_GATEWAY, "codex upstream response build failed").into_response()
    })
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
        .dispatch(key, request, Arc::clone(&state.route_store))
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
    path.strip_prefix("/api/llm-gateway")
        .or_else(|| path.strip_prefix("/api/codex-gateway"))
        .filter(|value| *value == "/v1/models")
}

fn codex_upstream_path_and_query(path: &str, query: Option<&str>) -> String {
    let upstream_path = path
        .strip_prefix("/api/llm-gateway")
        .or_else(|| path.strip_prefix("/api/codex-gateway"))
        .unwrap_or(path);
    match query {
        Some(query) if !query.is_empty() => format!("{upstream_path}?{query}"),
        _ => upstream_path.to_string(),
    }
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
        body::Body,
        http::{header, Request, StatusCode},
        response::{IntoResponse, Response},
    };
    use llm_access_core::store::{
        AuthenticatedKey, ControlStore, EmptyProviderRouteStore, ProviderRouteStore,
    };

    use super::ProviderDispatcher;

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

    #[async_trait]
    impl ProviderDispatcher for CapturingDispatcher {
        async fn dispatch(
            &self,
            key: AuthenticatedKey,
            request: Request<Body>,
            _route_store: Arc<dyn ProviderRouteStore>,
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

        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    }

    #[tokio::test]
    async fn provider_entry_handler_uses_axum_state() {
        let state = test_state();
        let response = super::provider_entry_handler(
            axum::extract::State(state),
            request_with_bearer(Some("Bearer valid-secret")),
        )
        .await;

        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
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
