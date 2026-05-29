//! Codex non-retryable/request-shape error classification and
//! retry-without-encrypted-reasoning request rewriting.

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

pub(crate) fn is_codex_invalid_encrypted_content_response(
    status: StatusCode,
    bytes: &Bytes,
) -> bool {
    if status != StatusCode::BAD_REQUEST {
        return false;
    }
    if codex_error_code_from_bytes(bytes).as_deref() == Some("invalid_encrypted_content") {
        return true;
    }
    std::str::from_utf8(bytes.as_ref())
        .map(|body| body.contains("invalid_encrypted_content"))
        .unwrap_or(false)
}
pub(crate) fn is_codex_non_retryable_client_error_response(
    status: StatusCode,
    bytes: &Bytes,
) -> bool {
    if status != StatusCode::BAD_REQUEST
        || is_codex_invalid_encrypted_content_response(status, bytes)
    {
        return false;
    }

    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
        return false;
    };
    let error = value.get("error").unwrap_or(&value);
    if json_string_field(error, "code")
        .as_deref()
        .is_some_and(codex_error_code_is_request_shape_failure)
    {
        return true;
    }

    extract_error_message_from_json_value(&value)
        .as_deref()
        .is_some_and(codex_message_indicates_request_shape_failure)
}
pub(crate) fn codex_error_code_is_request_shape_failure(code: &str) -> bool {
    matches!(
        code,
        "invalid_value"
            | "unsupported_value"
            | "invalid_type"
            | "missing_required_parameter"
            | "unknown_parameter"
            | "unsupported_parameter"
    )
}
pub(crate) fn codex_message_indicates_request_shape_failure(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    (normalized.contains("invalid value") && normalized.contains("supported values"))
        || normalized.contains("invalid type")
        || normalized.contains("missing required parameter")
        || normalized.contains("unknown parameter")
        || normalized.contains("unsupported parameter")
}
pub(crate) fn codex_error_code_from_bytes(bytes: &Bytes) -> Option<String> {
    serde_json::from_slice::<Value>(bytes)
        .ok()
        .and_then(|value| codex_error_code_from_value(&value))
}
pub(crate) fn codex_error_code_from_value(value: &Value) -> Option<String> {
    let error = value.get("error").unwrap_or(value);
    if let Some(code) = json_string_field(error, "code") {
        return Some(code);
    }
    let message = json_string_field(error, "message")?;
    serde_json::from_str::<Value>(&message)
        .ok()
        .and_then(|nested| codex_error_code_from_value(&nested))
}
pub(crate) fn retry_codex_without_encrypted_reasoning(
    prepared: &PreparedGatewayRequest,
) -> Option<PreparedGatewayRequest> {
    let mut value = serde_json::from_slice::<Value>(&prepared.request_body).ok()?;
    let root = value.as_object_mut()?;
    if !strip_codex_encrypted_reasoning_items(root) {
        return None;
    }
    let request_body = Bytes::from(serde_json::to_vec(&value).ok()?);
    let mut retry = prepared.clone();
    retry.request_body = request_body;
    Some(retry)
}
pub(crate) fn strip_codex_encrypted_reasoning_items(
    root: &mut serde_json::Map<String, Value>,
) -> bool {
    let Some(input) = root.get_mut("input") else {
        return false;
    };
    let mut remove_input = false;
    let changed = match input {
        Value::Array(items) => {
            let mut changed = false;
            let mut filtered = Vec::with_capacity(items.len());
            for mut item in std::mem::take(items) {
                let keep = sanitize_codex_encrypted_reasoning_item(&mut item, &mut changed);
                if keep {
                    filtered.push(item);
                }
            }
            if changed {
                if filtered.is_empty() {
                    remove_input = true;
                } else {
                    *items = filtered;
                }
            }
            changed
        },
        Value::Object(_) => {
            let mut changed = false;
            let keep = sanitize_codex_encrypted_reasoning_item(input, &mut changed);
            if changed && !keep {
                remove_input = true;
            }
            changed
        },
        _ => false,
    };
    if remove_input {
        root.remove("input");
    }
    changed
}
pub(crate) fn sanitize_codex_encrypted_reasoning_item(
    item: &mut Value,
    changed: &mut bool,
) -> bool {
    let Some(obj) = item.as_object_mut() else {
        return true;
    };
    if obj.get("type").and_then(Value::as_str) != Some("reasoning") {
        return true;
    }
    if obj.remove("encrypted_content").is_none() {
        return true;
    }
    *changed = true;
    obj.len() > 1
}
