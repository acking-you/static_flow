use std::collections::BTreeMap;

use axum::{http::StatusCode, response::Response};
use llm_access_kiro::anthropic::{
    preflight::{preprocess_messages_request, PreprocessedMessagesRequest},
    types::MessagesRequest,
};
use serde_json::Value;

use super::kiro_error::kiro_json_error;

pub(super) struct DirectAnthropicPreparedPayload {
    pub(super) original_model: String,
    pub(super) preflight: PreprocessedMessagesRequest,
}

pub(super) fn prepare_direct_anthropic_payload(
    body: &[u8],
) -> Result<DirectAnthropicPreparedPayload, DirectAnthropicPayloadError> {
    let raw = serde_json::from_slice::<Value>(body)
        .map_err(|_| DirectAnthropicPayloadError::InvalidPayload)?;
    let original_model = raw
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(DirectAnthropicPayloadError::MissingModel)?
        .to_string();
    let request = serde_json::from_value::<MessagesRequest>(raw)
        .map_err(|_| DirectAnthropicPayloadError::InvalidPayload)?;
    let preflight = preprocess_messages_request(&request)
        .map_err(|err| DirectAnthropicPayloadError::InvalidRequest(err.to_string()))?;

    Ok(DirectAnthropicPreparedPayload {
        original_model,
        preflight,
    })
}

#[derive(Debug)]
pub(super) enum DirectAnthropicPayloadError {
    InvalidPayload,
    MissingModel,
    InvalidRequest(String),
}

impl DirectAnthropicPayloadError {
    pub(super) fn into_response(self) -> Response {
        match self {
            Self::InvalidPayload => kiro_json_error(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "request body must be a valid Anthropic messages JSON payload",
            ),
            Self::MissingModel => kiro_json_error(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "model is required",
            ),
            Self::InvalidRequest(message) => {
                kiro_json_error(StatusCode::BAD_REQUEST, "invalid_request_error", &message)
            },
        }
    }
}

fn apply_model_mapping_to_request(
    model_name_map_json: &str,
    payload: &mut MessagesRequest,
) -> anyhow::Result<Option<String>> {
    let trimmed = model_name_map_json.trim();
    if trimmed.is_empty() || trimmed == "{}" {
        return Ok(None);
    }
    let map = serde_json::from_str::<BTreeMap<String, String>>(trimmed)?;
    let Some(target) = map.get(&payload.model).cloned() else {
        return Ok(None);
    };
    if target == payload.model {
        return Ok(None);
    }
    payload.model = target.clone();
    Ok(Some(target))
}

pub(super) fn build_route_payload(
    model_name_map_json: &str,
    payload: &MessagesRequest,
) -> anyhow::Result<(MessagesRequest, Option<String>)> {
    let mut route_payload = payload.clone();
    let mapped_model = apply_model_mapping_to_request(model_name_map_json, &mut route_payload)?;
    Ok((route_payload, mapped_model))
}
