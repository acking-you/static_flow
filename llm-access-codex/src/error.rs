//! HTTP-shaped errors produced by Codex request normalization.

use http::StatusCode;

/// Error returned by Codex gateway normalization before backend adaptation.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct CodexGatewayError {
    /// HTTP status code that should be returned to the caller.
    pub status: StatusCode,
    /// Client-visible error message.
    pub message: String,
}

impl CodexGatewayError {
    /// Build an error with a concrete status and message.
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

/// Result alias for Codex gateway normalization.
pub type CodexGatewayResult<T> = Result<T, CodexGatewayError>;

/// Build a `400 Bad Request` error.
pub fn bad_request(message: &str) -> CodexGatewayError {
    CodexGatewayError::new(StatusCode::BAD_REQUEST, message)
}

/// Build a `400 Bad Request` error with structured detail text.
pub fn bad_request_with_detail(message: &str, detail: impl std::fmt::Display) -> CodexGatewayError {
    CodexGatewayError::new(StatusCode::BAD_REQUEST, format!("{message}: {detail}"))
}

/// Build a `500 Internal Server Error` error with structured detail text.
pub fn internal_error(message: &str, detail: impl std::fmt::Display) -> CodexGatewayError {
    CodexGatewayError::new(StatusCode::INTERNAL_SERVER_ERROR, format!("{message}: {detail}"))
}

/// Build a `405 Method Not Allowed` error.
pub fn method_not_allowed(message: &str) -> CodexGatewayError {
    CodexGatewayError::new(StatusCode::METHOD_NOT_ALLOWED, message)
}

/// Build a `404 Not Found` error.
pub fn not_found(message: &str) -> CodexGatewayError {
    CodexGatewayError::new(StatusCode::NOT_FOUND, message)
}
