//! Kiro local compatibility endpoints.

use axum::{extract::Json as JsonExtractor, response::IntoResponse, Json};
use llm_access_kiro::anthropic::{
    count_tokens_response, supported_models_response, types::CountTokensRequest,
};

/// Return the Kiro Anthropic-compatible model catalog.
pub async fn get_models() -> impl IntoResponse {
    Json(supported_models_response())
}

/// Estimate input tokens for a Kiro Anthropic-compatible request.
pub async fn count_tokens(
    JsonExtractor(payload): JsonExtractor<CountTokensRequest>,
) -> impl IntoResponse {
    Json(count_tokens_response(payload))
}
