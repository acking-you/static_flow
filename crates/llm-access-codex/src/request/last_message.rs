//! Best-effort extraction of the last message text for usage logging/previews.


use axum::body::Bytes;
use serde_json::Value;
/// Extract the last text-like message content from the request body.
///
/// This intentionally parses request-format structures (`messages` for chat
/// requests, `input` for responses requests) instead of storing the whole
/// body. Unsupported shapes return `Ok(None)` so the main request flow is not
/// blocked; malformed JSON returns `Err(...)` and the caller can record a
/// failure marker if desired.
/// Extract the last text-like message content from an OpenAI-compatible body.
pub fn extract_last_message_content(body: &Bytes) -> Result<Option<String>, String> {
    if body.is_empty() {
        return Ok(None);
    }
    let value: Value = serde_json::from_slice(body)
        .map_err(|err| format!("failed to parse request body: {err}"))?;
    if !value.is_object() {
        return Err("request body is not a JSON object".to_string());
    }
    Ok(extract_last_message_content_from_value(&value))
}
/// Dispatch last-message extraction by request shape.
///
/// Routes to the chat-style extractor when a `messages` array is present, or
/// the responses-style extractor when an `input` field is present. Returns
/// `None` for any other shape.
pub fn extract_last_message_content_from_value(value: &Value) -> Option<String> {
    let root = value.as_object()?;

    if let Some(messages) = root.get("messages").and_then(Value::as_array) {
        return extract_last_message_from_chat_messages(messages);
    }
    if let Some(input) = root.get("input") {
        return extract_last_text_from_responses_input(input);
    }
    None
}
fn extract_last_message_from_chat_messages(messages: &[Value]) -> Option<String> {
    messages.iter().rev().find_map(|message| {
        message
            .as_object()
            .and_then(|obj| obj.get("content"))
            .and_then(extract_last_text_from_generic_content)
    })
}
fn extract_last_text_from_responses_input(input: &Value) -> Option<String> {
    match input {
        Value::String(text) => normalized_non_empty_text(text),
        Value::Array(items) => items
            .iter()
            .rev()
            .find_map(extract_last_text_from_responses_item),
        Value::Object(_) => extract_last_text_from_responses_item(input),
        _ => None,
    }
}
fn extract_last_text_from_responses_item(item: &Value) -> Option<String> {
    match item {
        Value::String(text) => normalized_non_empty_text(text),
        Value::Object(obj) => {
            let item_type = obj.get("type").and_then(Value::as_str).unwrap_or_default();
            match item_type {
                "message" => obj
                    .get("content")
                    .and_then(extract_last_text_from_generic_content),
                "function_call_output" | "custom_tool_call_output" => obj
                    .get("output")
                    .and_then(extract_last_text_from_generic_content),
                "text" | "input_text" | "output_text" => obj
                    .get("text")
                    .and_then(Value::as_str)
                    .and_then(normalized_non_empty_text),
                _ => obj
                    .get("content")
                    .and_then(extract_last_text_from_generic_content)
                    .or_else(|| {
                        obj.get("output")
                            .and_then(extract_last_text_from_generic_content)
                    })
                    .or_else(|| {
                        obj.get("text")
                            .and_then(Value::as_str)
                            .and_then(normalized_non_empty_text)
                    }),
            }
        },
        Value::Array(items) => items
            .iter()
            .rev()
            .find_map(extract_last_text_from_generic_content),
        _ => None,
    }
}
fn extract_last_text_from_generic_content(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => normalized_non_empty_text(text),
        Value::Array(items) => items
            .iter()
            .rev()
            .find_map(extract_last_text_from_generic_content),
        Value::Object(map) => map
            .get("content")
            .and_then(extract_last_text_from_generic_content)
            .or_else(|| {
                map.get("output")
                    .and_then(extract_last_text_from_generic_content)
            })
            .or_else(|| {
                map.get("text")
                    .and_then(Value::as_str)
                    .and_then(normalized_non_empty_text)
            }),
        _ => None,
    }
}
fn normalized_non_empty_text(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}
