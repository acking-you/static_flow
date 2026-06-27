use axum::http::StatusCode;
use llm_access_core::store::ProviderCodexRoute;

/// Runtime entrypoint used for one Codex image request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageGatewayMode {
    /// Request entered through the standalone `llm-access-codex-image` binary.
    StandaloneBinary,
    /// Request entered directly through the main `llm-access` Codex API
    /// service.
    IntegratedCodexApi,
}

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
    mode: ImageGatewayMode,
    routes: Vec<ProviderCodexRoute>,
) -> Result<Vec<ProviderCodexRoute>, ImageDispatchError> {
    let disabled = routes.iter().any(|route| match mode {
        ImageGatewayMode::StandaloneBinary => !route.codex_image_generation_enabled,
        ImageGatewayMode::IntegratedCodexApi => !route.codex_image_direct_generation_enabled,
    });
    if disabled {
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
