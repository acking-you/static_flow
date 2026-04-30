//! Provider-facing HTTP entrypoints for `llm-access`.

use std::sync::Arc;

use axum::{
    body::Body,
    extract::State,
    http::{header, HeaderMap, Request, StatusCode},
    response::{IntoResponse, Response},
};
use llm_access_core::store::{AuthenticatedKey, ControlStore};

/// Shared provider request state.
#[derive(Clone)]
pub struct ProviderState {
    control_store: Arc<dyn ControlStore>,
}

impl ProviderState {
    /// Create provider request state.
    pub fn new(control_store: Arc<dyn ControlStore>) -> Self {
        Self {
            control_store,
        }
    }
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
    let Some(secret) = bearer_secret(request.headers()).map(str::to_owned) else {
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

    (StatusCode::NOT_IMPLEMENTED, "provider dispatch is not wired").into_response()
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use axum::{
        body::Body,
        http::{header, Request, StatusCode},
    };
    use llm_access_core::store::{AuthenticatedKey, ControlStore};

    #[derive(Default)]
    struct TestStore;

    #[async_trait]
    impl ControlStore for TestStore {
        async fn authenticate_bearer_secret(
            &self,
            secret: &str,
        ) -> anyhow::Result<Option<AuthenticatedKey>> {
            let status = match secret {
                "valid-secret" => "active",
                "paused-secret" => "paused",
                _ => return Ok(None),
            };
            Ok(Some(AuthenticatedKey {
                key_id: "key-1".to_string(),
                key_name: "test-key".to_string(),
                provider_type: "kiro".to_string(),
                protocol_family: "anthropic".to_string(),
                status: status.to_string(),
                quota_billable_limit: 100,
                billable_tokens_used: 0,
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

    fn request_with_bearer(secret: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder().uri("/api/kiro-gateway/v1/messages");
        if let Some(secret) = secret {
            builder = builder.header(header::AUTHORIZATION, secret);
        }
        builder.body(Body::empty()).expect("request")
    }

    #[tokio::test]
    async fn provider_entry_rejects_missing_bearer_token() {
        let state = super::ProviderState::new(Arc::new(TestStore));
        let response = super::provider_entry(state, request_with_bearer(None)).await;

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn provider_entry_rejects_malformed_bearer_token() {
        let state = super::ProviderState::new(Arc::new(TestStore));
        for value in ["valid-secret", "Basic valid-secret", "Bearer "] {
            let response =
                super::provider_entry(state.clone(), request_with_bearer(Some(value))).await;
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
    }

    #[tokio::test]
    async fn provider_entry_rejects_unknown_bearer_token() {
        let state = super::ProviderState::new(Arc::new(TestStore));
        let response =
            super::provider_entry(state, request_with_bearer(Some("Bearer unknown-secret"))).await;

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn provider_entry_rejects_non_active_key() {
        let state = super::ProviderState::new(Arc::new(TestStore));
        let response =
            super::provider_entry(state, request_with_bearer(Some("Bearer paused-secret"))).await;

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn provider_entry_reports_store_errors_as_server_errors() {
        let state = super::ProviderState::new(Arc::new(FailingStore));
        let response =
            super::provider_entry(state, request_with_bearer(Some("Bearer valid-secret"))).await;

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn provider_entry_accepts_known_bearer_token_before_dispatch() {
        let state = super::ProviderState::new(Arc::new(TestStore));
        let response =
            super::provider_entry(state, request_with_bearer(Some("Bearer valid-secret"))).await;

        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    }

    #[tokio::test]
    async fn provider_entry_handler_uses_axum_state() {
        let state = super::ProviderState::new(Arc::new(TestStore));
        let response = super::provider_entry_handler(
            axum::extract::State(state),
            request_with_bearer(Some("Bearer valid-secret")),
        )
        .await;

        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    }
}
