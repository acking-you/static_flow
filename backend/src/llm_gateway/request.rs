//! Backend-compatible wrappers around standalone Codex request normalization.

use axum::{
    body::{Body, Bytes},
    http::{HeaderMap, Method, StatusCode},
    response::Json,
};
use llm_access_codex::{error::CodexGatewayError, types::PreparedGatewayRequest};

use crate::handlers::ErrorResponse;

type GatewayHandlerResult<T> = Result<T, (StatusCode, Json<ErrorResponse>)>;

pub(crate) use llm_access_codex::request::{
    external_origin, extract_client_ip_from_headers, extract_header_value,
    extract_last_message_content, extract_presented_key, extract_query_param, is_models_path,
    normalize_upstream_base_url, resolve_request_url_from_headers, serialize_headers_json,
};

fn adapt_codex_error(err: CodexGatewayError) -> (StatusCode, Json<ErrorResponse>) {
    (
        err.status,
        Json(ErrorResponse {
            code: err.status.as_u16(),
            error: err.message,
        }),
    )
}

pub(crate) async fn read_gateway_request_body(
    body: Body,
    max_request_body_bytes: usize,
) -> GatewayHandlerResult<Bytes> {
    llm_access_codex::request::read_gateway_request_body(body, max_request_body_bytes)
        .await
        .map_err(adapt_codex_error)
}

pub(crate) fn prepare_gateway_request_from_bytes(
    gateway_path: &str,
    query: &str,
    method: Method,
    headers: &HeaderMap,
    body: Bytes,
    max_request_body_bytes: usize,
) -> GatewayHandlerResult<PreparedGatewayRequest> {
    llm_access_codex::request::prepare_gateway_request_from_bytes(
        gateway_path,
        query,
        method,
        headers,
        body,
        max_request_body_bytes,
    )
    .map_err(adapt_codex_error)
}

pub(crate) fn apply_gpt53_codex_spark_mapping(
    prepared: &PreparedGatewayRequest,
    enabled: bool,
) -> GatewayHandlerResult<PreparedGatewayRequest> {
    llm_access_codex::request::apply_gpt53_codex_spark_mapping(prepared, enabled)
        .map_err(adapt_codex_error)
}

pub(crate) fn ensure_supported_gateway_path(path: &str) -> GatewayHandlerResult<()> {
    llm_access_codex::request::ensure_supported_gateway_path(path).map_err(adapt_codex_error)
}

pub(crate) fn normalize_name(raw: &str) -> GatewayHandlerResult<String> {
    llm_access_codex::request::normalize_name(raw).map_err(adapt_codex_error)
}

pub(crate) fn normalize_status(raw: &str) -> GatewayHandlerResult<String> {
    llm_access_codex::request::normalize_status(raw).map_err(adapt_codex_error)
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;

    use super::ensure_supported_gateway_path;

    #[test]
    fn backend_request_wrapper_preserves_error_status_and_body_code() {
        let err = ensure_supported_gateway_path("/v1/unsupported")
            .expect_err("unsupported path should be rejected");

        assert_eq!(err.0, StatusCode::NOT_FOUND);
        assert_eq!(err.1 .0.code, StatusCode::NOT_FOUND.as_u16());
        assert!(err.1 .0.error.contains("Unsupported llm gateway endpoint"));
    }
}
