use std::time::Duration;

use axum::{body::Bytes, http::StatusCode, response::Response};
use llm_access_kiro::anthropic::converter::ConversionError;

use super::errors::anthropic_json_error_body;

const USER_VISIBLE_UPSTREAM_NAME: &str = "AWS Bedrock";
pub(super) const AWS_BEDROCK_ALL_ACCOUNTS_COOLING_DOWN_MESSAGE: &str =
    "all eligible AWS Bedrock accounts are cooling down";
pub(super) const AWS_BEDROCK_ROUTE_SELECTION_FAILED_MESSAGE: &str =
    "AWS Bedrock route selection failed";

#[derive(Debug, Clone, Copy)]
pub(super) enum KiroRouteFailureKind {
    RetryNext,
    Fatal,
    QuotaExhausted,
    RateLimited { cooldown: Duration, mark_proxy: bool },
}

#[derive(Debug)]
pub(crate) struct KiroRouteFailure {
    pub(super) status: StatusCode,
    pub(super) body: Bytes,
    pub(super) kind: KiroRouteFailureKind,
}

impl KiroRouteFailure {
    pub(super) fn synthetic(
        status: StatusCode,
        message: String,
        kind: KiroRouteFailureKind,
    ) -> Self {
        let body = serde_json::json!({
            "error": {
                "type": "api_error",
                "message": message,
            }
        })
        .to_string();
        Self {
            status,
            body: Bytes::from(body),
            kind,
        }
    }

    pub(super) async fn from_response(
        response: reqwest::Response,
        kind: KiroRouteFailureKind,
    ) -> Self {
        let status = response.status();
        let body = response.bytes().await.unwrap_or_else(|_| Bytes::new());
        Self {
            status,
            body,
            kind,
        }
    }

    pub(super) fn with_kind(mut self, kind: KiroRouteFailureKind) -> Self {
        self.kind = kind;
        self
    }

    pub(crate) fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub(crate) fn status(&self) -> StatusCode {
        self.status
    }

    pub(super) fn into_response(self) -> Response {
        let message = super::summarize_error_bytes(&self.body);
        kiro_json_error(self.status, kiro_error_type_for_status(self.status), &message)
    }
}

fn kiro_error_type_for_status(status: StatusCode) -> &'static str {
    match status {
        StatusCode::PAYMENT_REQUIRED | StatusCode::TOO_MANY_REQUESTS => "rate_limit_error",
        StatusCode::UNAUTHORIZED => "authentication_error",
        StatusCode::FORBIDDEN => "permission_error",
        StatusCode::NOT_FOUND => "not_found_error",
        _ if status.is_client_error() => "invalid_request_error",
        _ => "api_error",
    }
}

fn kiro_default_user_error_message(status: StatusCode) -> &'static str {
    match status {
        StatusCode::BAD_REQUEST => "Request is invalid.",
        StatusCode::UNAUTHORIZED => "Authentication failed.",
        StatusCode::FORBIDDEN => "Permission denied.",
        StatusCode::NOT_FOUND => "Endpoint not found.",
        StatusCode::METHOD_NOT_ALLOWED => "Method not allowed.",
        StatusCode::PAYMENT_REQUIRED => "Quota exceeded.",
        StatusCode::TOO_MANY_REQUESTS => "Rate limit exceeded.",
        StatusCode::SERVICE_UNAVAILABLE => "Service unavailable.",
        StatusCode::INTERNAL_SERVER_ERROR => "Internal server error.",
        _ if status.is_server_error() => "Upstream service unavailable.",
        _ => "Request failed.",
    }
}

fn kiro_user_visible_message(status: StatusCode, message: &str) -> String {
    let trimmed = message.trim();
    let fallback = kiro_default_user_error_message(status);
    if trimmed.is_empty() {
        return fallback.to_string();
    }
    if matches!(
        status,
        StatusCode::TOO_MANY_REQUESTS
            | StatusCode::PAYMENT_REQUIRED
            | StatusCode::METHOD_NOT_ALLOWED
            | StatusCode::NOT_FOUND
            | StatusCode::SERVICE_UNAVAILABLE
            | StatusCode::INTERNAL_SERVER_ERROR
            | StatusCode::BAD_GATEWAY
            | StatusCode::GATEWAY_TIMEOUT
    ) {
        return fallback.to_string();
    }
    if status.is_server_error() {
        return fallback.to_string();
    }
    rebrand_kiro_for_user(trimmed)
}

fn rebrand_kiro_for_user(message: &str) -> String {
    let lower = message.to_ascii_lowercase();
    if !lower.contains("kiro") {
        return message.to_string();
    }

    let mut output = String::with_capacity(message.len() + USER_VISIBLE_UPSTREAM_NAME.len());
    let mut index = 0;
    while index < message.len() {
        if lower[index..].starts_with("kiro") {
            output.push_str(USER_VISIBLE_UPSTREAM_NAME);
            index += "kiro".len();
            continue;
        }
        let ch = message[index..]
            .chars()
            .next()
            .expect("index always points to a char boundary");
        output.push(ch);
        index += ch.len_utf8();
    }
    output
}

pub(super) fn kiro_json_error(status: StatusCode, error_type: &str, message: &str) -> Response {
    let _ = error_type;
    super::anthropic_json_error(
        status,
        kiro_error_type_for_status(status),
        &kiro_user_visible_message(status, message),
    )
}

fn kiro_bedrock_exception_message(exception_type: &str, message: &str) -> String {
    let exception_type = exception_type.trim();
    let message = message.trim();
    if message.is_empty() {
        return format!("{USER_VISIBLE_UPSTREAM_NAME} {exception_type}");
    }
    format!("{USER_VISIBLE_UPSTREAM_NAME} {exception_type}: {message}")
}

pub(super) fn kiro_bedrock_anthropic_error_body(
    error_type: &str,
    exception_type: &str,
    message: &str,
) -> String {
    anthropic_json_error_body(error_type, &kiro_bedrock_exception_message(exception_type, message))
}

pub(super) fn kiro_bedrock_anthropic_error(
    status: StatusCode,
    error_type: &str,
    exception_type: &str,
    message: &str,
) -> Response {
    super::anthropic_json_error(
        status,
        error_type,
        &kiro_bedrock_exception_message(exception_type, message),
    )
}

pub(super) fn kiro_upstream_error_response(
    status: StatusCode,
    _content_type: &str,
    bytes: Bytes,
) -> Response {
    let message = super::summarize_error_bytes(&bytes);
    kiro_json_error(status, kiro_error_type_for_status(status), &message)
}

pub(super) fn kiro_conversion_error_response(err: ConversionError) -> Response {
    match err {
        ConversionError::UnsupportedModel(model) => kiro_json_error(
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            &format!("Unsupported model: {model}"),
        ),
        ConversionError::EmptyMessages => {
            kiro_json_error(StatusCode::BAD_REQUEST, "invalid_request_error", "messages are empty")
        },
        ConversionError::InvalidRequest(message) => {
            kiro_json_error(StatusCode::BAD_REQUEST, "invalid_request_error", &message)
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_visible_message_rebrands_kiro_as_aws_bedrock() {
        let message = kiro_user_visible_message(
            StatusCode::BAD_REQUEST,
            "Kiro model mapping configuration is invalid: missing route",
        );

        assert_eq!(message, "AWS Bedrock model mapping configuration is invalid: missing route");
        assert!(!message.to_ascii_lowercase().contains("kiro"));
    }
}
