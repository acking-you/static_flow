//! Provider-facing HTTP entrypoints for `llm-access`.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};

/// Reject provider requests until request authentication and runtime dispatch
/// are wired into this crate.
pub async fn provider_entry() -> Response {
    (StatusCode::UNAUTHORIZED, "missing bearer token").into_response()
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;

    #[tokio::test]
    async fn provider_entry_rejects_missing_bearer_token() {
        let response = super::provider_entry().await;

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}
