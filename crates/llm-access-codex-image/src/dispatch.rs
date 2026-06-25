use axum::http::StatusCode;
use llm_access_core::store::ProviderCodexRoute;

/// Error produced before attempting any upstream image request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageDispatchError {
    /// HTTP status to return to the downstream caller.
    pub status: StatusCode,
    /// Human-readable error message.
    pub message: String,
}

impl ImageDispatchError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

/// Filters resolved Codex routes to accounts allowed to serve image requests.
pub fn eligible_image_routes(
    routes: Vec<ProviderCodexRoute>,
) -> Result<Vec<ProviderCodexRoute>, ImageDispatchError> {
    if routes
        .iter()
        .any(|route| !route.codex_image_generation_enabled)
    {
        return Err(ImageDispatchError::new(
            StatusCode::FORBIDDEN,
            "codex image generation is disabled for this key",
        ));
    }
    let routes = routes
        .into_iter()
        .filter(|route| route.account_codex_image_generation_enabled)
        .collect::<Vec<_>>();
    if routes.is_empty() {
        return Err(ImageDispatchError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "no codex image-enabled accounts are available",
        ));
    }
    Ok(routes)
}

/// Classifies upstream statuses that may be retried on another account.
pub fn should_failover_status(status: StatusCode) -> bool {
    status == StatusCode::UNAUTHORIZED
        || status == StatusCode::FORBIDDEN
        || status == StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
}
