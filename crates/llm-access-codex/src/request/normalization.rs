//! Upstream request normalization: base-url, model/effort, field
//! filter/validate, JSON-mode + tool-call checks; hosts
//! normalize_responses_request.


use std::collections::BTreeSet;

use axum::body::Bytes;
use serde_json::{Map, Value};

use super::{extract_non_empty_string, DEFAULT_PUBLIC_GPT_MODEL_ID};
use crate::{
    error::{bad_request, internal_error, CodexGatewayResult},
    instructions::codex_default_instructions,
};
/// Rewrite chat/completions requests onto the upstream responses endpoint.
pub fn rewrite_responses_path(path: &str, query: &str) -> String {
    let rewritten = if let Some(suffix) = path.strip_prefix("/v1/chat/completions") {
        format!("/v1/responses{suffix}")
    } else {
        path.to_string()
    };
    format!("{rewritten}{query}")
}
/// Normalize configured upstream base URLs into the Codex backend form.
pub fn normalize_upstream_base_url(base: &str) -> String {
    let mut normalized = base.trim().trim_end_matches('/').to_string();
    let lower = normalized.to_ascii_lowercase();
    if (lower.starts_with("https://chatgpt.com") || lower.starts_with("https://chat.openai.com"))
        && !lower.contains("/backend-api")
    {
        normalized = format!("{normalized}/backend-api/codex");
    }
    normalized
}
/// Detect whether an upstream base URL points at an Azure OpenAI deployment.
///
/// Azure's `/responses` surface differs from chatgpt.com (notably it persists
/// responses, so `store` must be `true`). Recognizes the common Azure hostname
/// markers case-insensitively so callers can branch on those semantics.
pub fn is_azure_responses_upstream_base(base_url: &str) -> bool {
    let base_url = base_url.to_ascii_lowercase();
    const AZURE_MARKERS: [&str; 6] = [
        "openai.azure.",
        "cognitiveservices.azure.",
        "aoai.azure.",
        "azure-api.",
        "azurefd.",
        "windows.net/openai",
    ];
    AZURE_MARKERS.iter().any(|marker| base_url.contains(marker))
}
/// Collapse user-provided reasoning-effort aliases into supported values.
pub fn normalize_reasoning_effort(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "low" => Some("low"),
        "medium" => Some("medium"),
        "high" => Some("high"),
        "max" => Some("xhigh"),
        "xhigh" | "extra_high" => Some("xhigh"),
        _ => None,
    }
}
/// Fall back to the default public GPT model for non-`gpt-` model ids.
///
/// Public clients may send model ids the upstream does not recognize. When the
/// id does not start with `gpt-`, the request body's `model` field is rewritten
/// to [`DEFAULT_PUBLIC_GPT_MODEL_ID`] and the resolved id returned alongside
/// the re-encoded body. `gpt-`-prefixed ids and absent models pass through.
/// Returns an internal error if the body is not a JSON object.
pub fn normalize_codex_public_model(
    request_body: Bytes,
    model: Option<String>,
) -> CodexGatewayResult<(Bytes, Option<String>)> {
    let Some(model) = model else {
        return Ok((request_body, None));
    };
    if model.starts_with("gpt-") {
        return Ok((request_body, Some(model)));
    }

    let mut value = serde_json::from_slice::<Value>(&request_body).map_err(|err| {
        internal_error("Failed to parse llm gateway request body for model fallback", err)
    })?;
    let Some(root) = value.as_object_mut() else {
        return Err(internal_error(
            "Failed to normalize llm gateway request model",
            "request body is not a JSON object",
        ));
    };
    root.insert("model".to_string(), Value::String(DEFAULT_PUBLIC_GPT_MODEL_ID.to_string()));
    let request_body = Bytes::from(serde_json::to_vec(&value).map_err(|err| {
        internal_error("Failed to encode llm gateway request body after model fallback", err)
    })?);
    Ok((request_body, Some(DEFAULT_PUBLIC_GPT_MODEL_ID.to_string())))
}
/// Strip a `/responses` request body down to the upstream-supported fields.
///
/// Retains only the allow-listed top-level keys for the given path, dropping
/// anything else the client sent (the upstream rejects unknown fields). The
/// allow-list differs between `/v1/responses` and `/v1/responses/compact`; any
/// other path is left untouched.
pub fn filter_responses_request_fields(path: &str, root: &mut Map<String, Value>) {
    root.retain(|key, _| match path {
        "/v1/responses" => matches!(
            key.as_str(),
            "model"
                | "instructions"
                | "previous_response_id"
                | "input"
                | "tools"
                | "tool_choice"
                | "parallel_tool_calls"
                | "reasoning"
                | "store"
                | "stream"
                | "include"
                | "service_tier"
                | "prompt_cache_key"
                | "text"
                | "client_metadata"
        ),
        "/v1/responses/compact" => matches!(
            key.as_str(),
            "model"
                | "instructions"
                | "input"
                | "tools"
                | "parallel_tool_calls"
                | "reasoning"
                | "service_tier"
                | "prompt_cache_key"
                | "text"
        ),
        _ => true,
    });
}
/// Structurally validate a `/responses` request before it leaves the gateway.
///
/// No-op for non-`/responses` paths. For `/v1/responses` it checks that any
/// JSON-object input messages are well formed. For other `/responses` paths
/// (e.g. `/compact`) it additionally validates tool-call/tool-result pairing in
/// the conversation history. Returns a `400` on the first violation.
pub fn validate_responses_request(path: &str, root: &Map<String, Value>) -> CodexGatewayResult<()> {
    if !path.starts_with("/v1/responses") {
        return Ok(());
    }

    validate_json_object_input_messages(root)?;
    if path == "/v1/responses" {
        return Ok(());
    }
    validate_tool_call_history(root)?;
    Ok(())
}
fn normalize_codex_input_message_roles(root: &mut Map<String, Value>) {
    let Some(input) = root.get_mut("input") else {
        return;
    };
    normalize_codex_input_message_roles_value(input);
}
fn normalize_codex_input_message_roles_value(value: &mut Value) {
    match value {
        Value::Array(items) => {
            for item in items {
                normalize_codex_input_message_roles_value(item);
            }
        },
        Value::Object(obj) => {
            if obj.get("type").and_then(Value::as_str) == Some("message")
                && obj.get("role").and_then(Value::as_str) == Some("system")
            {
                obj.insert("role".to_string(), Value::String("developer".to_string()));
            }
        },
        _ => {},
    }
}
fn validate_json_object_input_messages(root: &Map<String, Value>) -> CodexGatewayResult<()> {
    let format_type = root
        .get("text")
        .and_then(Value::as_object)
        .and_then(|text| text.get("format"))
        .and_then(Value::as_object)
        .and_then(|format| format.get("type"))
        .and_then(Value::as_str);
    if format_type != Some("json_object") {
        return Ok(());
    }

    if responses_input_messages_contain_json_keyword(root.get("input")) {
        return Ok(());
    }

    Err(bad_request(
        "responses text.format.type `json_object` requires at least one input message containing \
         `json`",
    ))
}
fn responses_input_messages_contain_json_keyword(input: Option<&Value>) -> bool {
    let Some(input) = input else {
        return false;
    };
    match input {
        Value::String(text) => text_contains_json_keyword(text),
        Value::Array(items) => items.iter().any(response_input_item_contains_json_keyword),
        Value::Object(_) => response_input_item_contains_json_keyword(input),
        _ => false,
    }
}
fn response_input_item_contains_json_keyword(item: &Value) -> bool {
    let Some(obj) = item.as_object() else {
        return false;
    };
    if obj.get("type").and_then(Value::as_str) != Some("message") {
        return false;
    }
    obj.get("content")
        .is_some_and(response_content_contains_json_keyword)
}
fn response_content_contains_json_keyword(value: &Value) -> bool {
    match value {
        Value::String(text) => text_contains_json_keyword(text),
        Value::Array(items) => items.iter().any(response_content_contains_json_keyword),
        Value::Object(obj) => {
            obj.get("content")
                .is_some_and(response_content_contains_json_keyword)
                || obj
                    .get("text")
                    .and_then(Value::as_str)
                    .is_some_and(text_contains_json_keyword)
        },
        _ => false,
    }
}
fn text_contains_json_keyword(text: &str) -> bool {
    text.to_ascii_lowercase().contains("json")
}
fn validate_tool_call_history(root: &Map<String, Value>) -> CodexGatewayResult<()> {
    let Some(input) = root.get("input") else {
        return Ok(());
    };
    let items = match input {
        Value::Array(items) => items.iter().collect::<Vec<_>>(),
        Value::Object(_) => vec![input],
        _ => return Ok(()),
    };
    let mut seen_calls = BTreeSet::new();
    let mut pending_calls = BTreeSet::new();
    let mut seen_outputs = BTreeSet::new();

    for (index, item) in items.iter().enumerate() {
        let Some(obj) = item.as_object() else {
            continue;
        };
        let item_type = obj.get("type").and_then(Value::as_str).unwrap_or_default();
        match item_type {
            "function_call" | "custom_tool_call" => {
                let call_id =
                    extract_non_empty_string(obj.get("call_id").or_else(|| obj.get("id")))
                        .ok_or_else(|| {
                            bad_request(&format!(
                                "responses input item {index} ({item_type}) is missing call_id"
                            ))
                        })?;
                if !seen_calls.insert(call_id.to_string()) {
                    return Err(bad_request(&format!(
                        "responses input contains duplicate function call `{call_id}`"
                    )));
                }
                pending_calls.insert(call_id.to_string());
            },
            "function_call_output" | "custom_tool_call_output" => {
                let call_id = extract_non_empty_string(obj.get("call_id")).ok_or_else(|| {
                    bad_request(&format!(
                        "responses input item {index} ({item_type}) is missing call_id"
                    ))
                })?;
                if !seen_outputs.insert(call_id.to_string()) {
                    return Err(bad_request(&format!(
                        "responses input contains duplicate tool output for function call \
                         `{call_id}`"
                    )));
                }
                if pending_calls.remove(call_id) {
                    continue;
                }
                return Err(bad_request(&format!(
                    "responses input contains tool output for unknown function call `{call_id}`"
                )));
            },
            _ => {},
        }
    }

    if let Some(call_id) = pending_calls.into_iter().next() {
        return Err(bad_request(&format!("No tool output found for function call {call_id}")));
    }

    Ok(())
}
/// Fill defaults and normalize request shape for upstream responses endpoints.
pub fn normalize_responses_request(
    path: &str,
    root: &mut serde_json::Map<String, Value>,
    thread_anchor: Option<&str>,
) {
    normalize_codex_input_message_roles(root);

    if root
        .get("instructions")
        .and_then(Value::as_str)
        .is_some_and(|value| value.trim().is_empty())
        || matches!(root.get("instructions"), Some(Value::Null))
    {
        root.remove("instructions");
    }
    if path == "/v1/responses/compact" {
        root.insert("store".to_string(), Value::Bool(false));
    }
    if matches!(path, "/v1/responses" | "/v1/responses/compact") {
        root.entry("instructions".to_string())
            .or_insert_with(|| Value::String(codex_default_instructions().to_string()));
    }
    root.entry("tools".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));

    if path == "/v1/responses" {
        root.entry("include".to_string())
            .or_insert_with(|| Value::Array(Vec::new()));
    }

    let has_non_empty_tools = root
        .get("tools")
        .and_then(Value::as_array)
        .is_some_and(|items| !items.is_empty());
    if !has_non_empty_tools {
        root.entry("parallel_tool_calls".to_string())
            .or_insert(Value::Bool(false));
    }

    if matches!(path, "/v1/responses" | "/v1/responses/compact") {
        if let Some(thread_anchor) = thread_anchor
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            let needs_prompt_cache_key = root
                .get("prompt_cache_key")
                .and_then(Value::as_str)
                .map(str::trim)
                .is_none_or(|value| value.is_empty());
            if needs_prompt_cache_key {
                root.insert(
                    "prompt_cache_key".to_string(),
                    Value::String(thread_anchor.to_string()),
                );
            }
        }
    }

    if let Some(input) = root.get_mut("input") {
        match input {
            Value::String(text) => {
                let mut content = serde_json::Map::new();
                content.insert("type".to_string(), Value::String("input_text".to_string()));
                content.insert("text".to_string(), Value::String(text.clone()));

                let mut message = serde_json::Map::new();
                message.insert("type".to_string(), Value::String("message".to_string()));
                message.insert("role".to_string(), Value::String("user".to_string()));
                message.insert("content".to_string(), Value::Array(vec![Value::Object(content)]));
                *input = Value::Array(vec![Value::Object(message)]);
            },
            Value::Object(_) => {
                *input = Value::Array(vec![input.clone()]);
            },
            _ => {},
        }
    }

    if path == "/v1/responses" {
        let has_reasoning = root.contains_key("reasoning");
        let include = root
            .entry("include".to_string())
            .or_insert_with(|| Value::Array(Vec::new()));
        if !include.is_array() {
            *include = Value::Array(Vec::new());
        }
        if has_reasoning {
            let include_array = include
                .as_array_mut()
                .expect("include array just initialized");
            if !include_array.iter().any(|value| {
                value
                    .as_str()
                    .map(|item| item == "reasoning.encrypted_content")
                    .unwrap_or(false)
            }) {
                include_array.push(Value::String("reasoning.encrypted_content".to_string()));
            }
        }
        if let Some(service_tier) = root.get_mut("service_tier") {
            if service_tier
                .as_str()
                .is_some_and(|raw| raw.eq_ignore_ascii_case("fast"))
            {
                *service_tier = Value::String("priority".to_string());
            }
        }
    }
}
