use std::collections::BTreeMap;

use axum::body::Bytes;
use eventsource_stream::Event as SseEvent;
use serde_json::{json, Map, Value};

use super::{
    request::restore_openai_tool_name,
    types::{ChatStreamMetadata, GatewayResponseAdapter, UsageBreakdown},
};

fn rewrite_model_alias_in_value(value: &mut Value, from: &str, to: &str) {
    match value {
        Value::Object(map) => {
            if let Some(model) = map.get_mut("model") {
                if model.as_str() == Some(from) {
                    *model = Value::String(to.to_string());
                }
            }
            for child in map.values_mut() {
                rewrite_model_alias_in_value(child, from, to);
            }
        },
        Value::Array(items) => {
            for item in items {
                rewrite_model_alias_in_value(item, from, to);
            }
        },
        _ => {},
    }
}

fn maybe_apply_model_alias(
    mut value: Value,
    model_from: Option<&str>,
    model_to: Option<&str>,
) -> Value {
    if let (Some(from), Some(to)) = (model_from, model_to) {
        if from != to {
            rewrite_model_alias_in_value(&mut value, from, to);
        }
    }
    value
}

/// Flatten text-like content fragments from a responses payload.
fn map_response_content_text(content: &Value, out: &mut String) {
    match content {
        Value::String(text) => out.push_str(text),
        Value::Array(items) => {
            for item in items {
                if let Some(text) = item.as_str() {
                    out.push_str(text);
                    continue;
                }
                let Some(item_obj) = item.as_object() else {
                    continue;
                };
                let item_type = item_obj
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if matches!(item_type, "text" | "input_text" | "output_text") {
                    if let Some(text) = item_obj.get("text").and_then(Value::as_str) {
                        out.push_str(text);
                    }
                }
            }
        },
        Value::Object(map) => {
            if let Some(text) = map.get("text").and_then(Value::as_str) {
                out.push_str(text);
            }
            if let Some(inner) = map.get("content") {
                map_response_content_text(inner, out);
            }
        },
        _ => {},
    }
}

/// Adapt a completed responses payload into the classic chat/completions
/// schema.
fn map_response_to_chat_completion(
    value: &Value,
    tool_name_restore_map: Option<&BTreeMap<String, String>>,
) -> Value {
    let source = value.get("response").unwrap_or(value);
    let id = source
        .get("id")
        .cloned()
        .unwrap_or_else(|| Value::String(String::new()));
    let created = source
        .get("created")
        .cloned()
        .or_else(|| source.get("created_at").cloned())
        .unwrap_or_else(|| Value::Number(0.into()));
    let model = source
        .get("model")
        .cloned()
        .unwrap_or_else(|| Value::String(String::new()));
    let usage = source.get("usage").cloned();

    let mut assistant_text = String::new();
    let mut tool_calls = Vec::<Value>::new();
    if let Some(output_items) = source.get("output").and_then(Value::as_array) {
        for (idx, item) in output_items.iter().enumerate() {
            let Some(item_obj) = item.as_object() else {
                continue;
            };
            let item_type = item_obj
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            match item_type {
                "message" => {
                    if let Some(content) = item_obj.get("content") {
                        map_response_content_text(content, &mut assistant_text);
                    }
                },
                "function_call" | "custom_tool_call" => {
                    let call_id = item_obj
                        .get("call_id")
                        .or_else(|| item_obj.get("id"))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("call_{idx}"));
                    let name = item_obj
                        .get("name")
                        .and_then(Value::as_str)
                        .map(|name| restore_openai_tool_name(name, tool_name_restore_map))
                        .unwrap_or_else(|| "tool".to_string());
                    let arguments = item_obj
                        .get("arguments")
                        .map(|raw| {
                            if let Some(text) = raw.as_str() {
                                text.to_string()
                            } else {
                                serde_json::to_string(raw).unwrap_or_else(|_| "{}".to_string())
                            }
                        })
                        .unwrap_or_else(|| "{}".to_string());
                    tool_calls.push(json!({
                        "id": call_id,
                        "type": "function",
                        "function": {
                            "name": name,
                            "arguments": arguments
                        }
                    }));
                },
                "function_call_output" | "custom_tool_call_output" => {
                    if let Some(output) = item_obj.get("output") {
                        map_response_content_text(output, &mut assistant_text);
                    }
                },
                _ => {},
            }
        }
    }

    let mut message = Map::new();
    message.insert("role".to_string(), Value::String("assistant".to_string()));
    message.insert(
        "content".to_string(),
        if assistant_text.is_empty() { Value::Null } else { Value::String(assistant_text) },
    );
    if !tool_calls.is_empty() {
        message.insert("tool_calls".to_string(), Value::Array(tool_calls));
    }

    let mut out = Map::new();
    out.insert("id".to_string(), id);
    out.insert("object".to_string(), Value::String("chat.completion".to_string()));
    out.insert("created".to_string(), created);
    out.insert("model".to_string(), model);
    out.insert(
        "choices".to_string(),
        Value::Array(vec![json!({
            "index": 0,
            "message": Value::Object(message),
            "finish_reason": if source
                .get("output")
                .and_then(Value::as_array)
                .is_some_and(|items| items.iter().any(|item| {
                    item.get("type")
                        .and_then(Value::as_str)
                        .is_some_and(|kind| matches!(kind, "function_call" | "custom_tool_call"))
                })) {
                "tool_calls"
            } else {
                "stop"
            }
        })]),
    );
    if let Some(usage) = usage {
        out.insert("usage".to_string(), usage);
    }
    Value::Object(out)
}

/// Adapt a completed upstream response according to the selected response mode.
pub(crate) fn adapt_completed_response_json(
    response: &Value,
    adapter: GatewayResponseAdapter,
    tool_name_restore_map: Option<&BTreeMap<String, String>>,
) -> Value {
    match adapter {
        GatewayResponseAdapter::Responses => response.clone(),
        GatewayResponseAdapter::ChatCompletions => {
            map_response_to_chat_completion(response, tool_name_restore_map)
        },
    }
}

/// Extract the response id from a streamed responses event.
fn stream_event_response_id(value: &Value) -> String {
    value
        .get("response_id")
        .and_then(Value::as_str)
        .or_else(|| value.get("id").and_then(Value::as_str))
        .or_else(|| {
            value
                .get("response")
                .and_then(|response| response.get("id"))
                .and_then(Value::as_str)
        })
        .unwrap_or("")
        .to_string()
}

/// Extract the model id from a streamed responses event.
fn stream_event_model(value: &Value) -> String {
    value
        .get("model")
        .and_then(Value::as_str)
        .or_else(|| {
            value
                .get("response")
                .and_then(|response| response.get("model"))
                .and_then(Value::as_str)
        })
        .unwrap_or("")
        .to_string()
}

/// Extract the created timestamp from a streamed responses event.
fn stream_event_created(value: &Value) -> i64 {
    value
        .get("created")
        .and_then(Value::as_i64)
        .or_else(|| {
            value
                .get("response")
                .and_then(|response| response.get("created"))
                .and_then(Value::as_i64)
        })
        .or_else(|| {
            value
                .get("response")
                .and_then(|response| response.get("created_at"))
                .and_then(Value::as_i64)
        })
        .unwrap_or(0)
}

/// Extract displayable text from one streamed responses event payload.
fn extract_stream_event_text(value: &Value) -> String {
    if let Some(delta) = value.get("delta") {
        if let Some(text) = delta.as_str() {
            return text.to_string();
        }
        if let Some(text) = delta.get("text").and_then(Value::as_str) {
            return text.to_string();
        }
    }
    if let Some(text) = value.get("text").and_then(Value::as_str) {
        return text.to_string();
    }
    let mut out = String::new();
    if let Some(part) = value.get("part").or_else(|| value.get("content_part")) {
        map_response_content_text(part, &mut out);
        if !out.is_empty() {
            return out;
        }
    }
    if let Some(item) = value.get("item").or_else(|| value.get("output_item")) {
        if let Some(content) = item.get("content") {
            map_response_content_text(content, &mut out);
        } else {
            map_response_content_text(item, &mut out);
        }
    }
    out
}

/// Build one OpenAI chat.completion.chunk carrying assistant text.
fn build_openai_chat_text_chunk(value: &Value, text: &str) -> Value {
    json!({
        "id": stream_event_response_id(value),
        "object": "chat.completion.chunk",
        "created": stream_event_created(value),
        "model": stream_event_model(value),
        "choices": [{
            "index": 0,
            "delta": {
                "role": "assistant",
                "content": text
            },
            "finish_reason": Value::Null
        }]
    })
}

/// Track sticky stream metadata so later chunks can fill missing fields.
fn observe_chat_stream_metadata(value: &Value, metadata: &mut ChatStreamMetadata) {
    let response_id = stream_event_response_id(value);
    if !response_id.is_empty() {
        metadata.response_id = Some(response_id);
    }
    let model = stream_event_model(value);
    if !model.is_empty() {
        metadata.model = Some(model);
    }
    let created = stream_event_created(value);
    if created > 0 {
        metadata.created = Some(created);
    }
}

/// Fill missing chunk metadata from the last seen stream-level defaults.
fn fill_chat_chunk_defaults(chunk: &mut Value, metadata: &ChatStreamMetadata) {
    let Some(obj) = chunk.as_object_mut() else {
        return;
    };

    let missing_id = obj
        .get("id")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty);
    if missing_id {
        if let Some(id) = metadata.response_id.as_deref() {
            obj.insert("id".to_string(), Value::String(id.to_string()));
        }
    }

    let missing_model = obj
        .get("model")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty);
    if missing_model {
        if let Some(model) = metadata.model.as_deref() {
            obj.insert("model".to_string(), Value::String(model.to_string()));
        }
    }

    let missing_created = obj.get("created").and_then(Value::as_i64).unwrap_or(0) <= 0;
    if missing_created {
        if let Some(created) = metadata.created {
            obj.insert("created".to_string(), Value::Number(created.into()));
        }
    }
}

/// Convert one responses stream event into an OpenAI chat chunk when possible.
fn convert_response_value_to_chat_chunk(
    value: &Value,
    tool_name_restore_map: Option<&BTreeMap<String, String>>,
) -> Option<Value> {
    let chunk_type = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match chunk_type {
        "response.output_text.delta" => {
            let text = extract_stream_event_text(value);
            if text.is_empty() {
                None
            } else {
                Some(build_openai_chat_text_chunk(value, text.as_str()))
            }
        },
        "response.output_item.added" | "response.output_item.done" => {
            let item = value.get("item").or_else(|| value.get("output_item"))?;
            let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
            if matches!(item_type, "function_call" | "custom_tool_call") {
                let call_id = item
                    .get("call_id")
                    .or_else(|| item.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or("call_0");
                let name = item.get("name").and_then(Value::as_str).unwrap_or("tool");
                let name = restore_openai_tool_name(name, tool_name_restore_map);
                let arguments = item
                    .get("arguments")
                    .map(|raw| {
                        if let Some(text) = raw.as_str() {
                            text.to_string()
                        } else {
                            serde_json::to_string(raw).unwrap_or_else(|_| "{}".to_string())
                        }
                    })
                    .unwrap_or_else(|| "{}".to_string());
                return Some(json!({
                    "id": stream_event_response_id(value),
                    "object": "chat.completion.chunk",
                    "created": stream_event_created(value),
                    "model": stream_event_model(value),
                    "choices": [{
                        "index": 0,
                        "delta": {
                            "role": "assistant",
                            "tool_calls": [{
                                "index": 0,
                                "id": call_id,
                                "type": "function",
                                "function": {
                                    "name": name,
                                    "arguments": arguments
                                }
                            }]
                        },
                        "finish_reason": Value::Null
                    }]
                }));
            }
            None
        },
        "response.function_call_arguments.delta" | "response.function_call_arguments.done" => {
            let call_id = value
                .get("call_id")
                .or_else(|| value.get("item_id"))
                .and_then(Value::as_str)
                .unwrap_or("call_0");
            let arguments = value
                .get("delta")
                .or_else(|| value.get("arguments"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if arguments.is_empty() {
                return None;
            }
            Some(json!({
                "id": stream_event_response_id(value),
                "object": "chat.completion.chunk",
                "created": stream_event_created(value),
                "model": stream_event_model(value),
                "choices": [{
                    "index": 0,
                    "delta": {
                        "role": "assistant",
                        "tool_calls": [{
                            "index": 0,
                            "id": call_id,
                            "type": "function",
                            "function": {
                                "arguments": arguments
                            }
                        }]
                    },
                    "finish_reason": Value::Null
                }]
            }))
        },
        "response.completed" | "response.done" => {
            let response = value.get("response").unwrap_or(&Value::Null);
            let finish_reason = if response
                .get("output")
                .and_then(Value::as_array)
                .is_some_and(|items| {
                    items.iter().any(|item| {
                        item.get("type")
                            .and_then(Value::as_str)
                            .is_some_and(|kind| {
                                matches!(kind, "function_call" | "custom_tool_call")
                            })
                    })
                }) {
                "tool_calls"
            } else {
                "stop"
            };
            let mut out = json!({
                "id": response
                    .get("id")
                    .cloned()
                    .unwrap_or_else(|| Value::String(stream_event_response_id(value))),
                "object": "chat.completion.chunk",
                "created": response
                    .get("created")
                    .cloned()
                    .or_else(|| response.get("created_at").cloned())
                    .unwrap_or_else(|| Value::Number(stream_event_created(value).into())),
                "model": response
                    .get("model")
                    .cloned()
                    .unwrap_or_else(|| Value::String(stream_event_model(value))),
                "choices": [{
                    "index": 0,
                    "delta": {},
                    "finish_reason": finish_reason
                }]
            });
            if let Some(usage) = response
                .get("usage")
                .cloned()
                .or_else(|| value.get("usage").cloned())
            {
                if let Some(obj) = out.as_object_mut() {
                    obj.insert("usage".to_string(), usage);
                }
            }
            Some(out)
        },
        _ => None,
    }
}

/// Convert one parsed SSE event into a chat chunk and update stream defaults.
pub(crate) fn convert_response_event_to_chat_chunk(
    event: &SseEvent,
    tool_name_restore_map: Option<&BTreeMap<String, String>>,
    metadata: &mut ChatStreamMetadata,
    model_from: Option<&str>,
    model_to: Option<&str>,
) -> Option<Value> {
    let payload = event.data.trim();
    if payload.is_empty() || payload == "[DONE]" {
        return None;
    }
    let value =
        maybe_apply_model_alias(serde_json::from_str::<Value>(payload).ok()?, model_from, model_to);
    observe_chat_stream_metadata(&value, metadata);
    let mut chunk = convert_response_value_to_chat_chunk(&value, tool_name_restore_map)?;
    fill_chat_chunk_defaults(&mut chunk, metadata);
    Some(chunk)
}

/// Encode a JSON payload as a single SSE `data:` chunk.
pub(crate) fn encode_json_sse_chunk(value: &Value) -> Bytes {
    let body = serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string());
    Bytes::from(format!("data: {body}\n\n"))
}

/// Convert a non-streaming responses JSON payload into chat/completions JSON.
pub(crate) fn convert_json_response_to_chat_completion(
    bytes: &[u8],
    tool_name_restore_map: Option<&BTreeMap<String, String>>,
    model_from: Option<&str>,
    model_to: Option<&str>,
) -> Result<Vec<u8>, String> {
    let value = serde_json::from_slice::<Value>(bytes)
        .map_err(|_| "invalid upstream json payload".to_string())?;
    let value = maybe_apply_model_alias(value, model_from, model_to);
    serde_json::to_vec(&map_response_to_chat_completion(&value, tool_name_restore_map))
        .map_err(|err| format!("serialize chat.completion json failed: {err}"))
}

pub(crate) fn rewrite_json_response_model_alias(
    bytes: &[u8],
    model_from: Option<&str>,
    model_to: Option<&str>,
) -> Option<Vec<u8>> {
    let value = serde_json::from_slice::<Value>(bytes).ok()?;
    let value = maybe_apply_model_alias(value, model_from, model_to);
    serde_json::to_vec(&value).ok()
}

/// Parse a non-streaming JSON body and extract usage accounting when present.
pub(crate) fn extract_usage_from_bytes(bytes: &[u8]) -> Option<UsageBreakdown> {
    serde_json::from_slice::<Value>(bytes)
        .ok()
        .and_then(|value| extract_usage_from_value(&value))
}

/// Recursively search a JSON value for a nested usage payload.
fn extract_usage_from_value(value: &Value) -> Option<UsageBreakdown> {
    match value {
        Value::Object(map) => {
            if let Some(usage) = map.get("usage").and_then(usage_breakdown_from_usage_value) {
                return Some(usage);
            }
            map.values().find_map(extract_usage_from_value)
        },
        Value::Array(items) => items.iter().find_map(extract_usage_from_value),
        _ => None,
    }
}

/// Normalize an OpenAI-style usage object into the gateway billing breakdown.
fn usage_breakdown_from_usage_value(value: &Value) -> Option<UsageBreakdown> {
    let usage = value.as_object()?;
    let input_total = usage
        .get("input_tokens")
        .and_then(Value::as_u64)
        .or_else(|| usage.get("prompt_tokens").and_then(Value::as_u64))
        .unwrap_or(0);
    let input_cached_tokens = usage
        .get("input_tokens_details")
        .and_then(Value::as_object)
        .and_then(|obj| obj.get("cached_tokens"))
        .and_then(Value::as_u64)
        .or_else(|| {
            usage
                .get("prompt_tokens_details")
                .and_then(Value::as_object)
                .and_then(|obj| obj.get("cached_tokens"))
                .and_then(Value::as_u64)
        })
        .unwrap_or(0);
    let output_tokens = usage
        .get("output_tokens")
        .and_then(Value::as_u64)
        .or_else(|| usage.get("completion_tokens").and_then(Value::as_u64))
        .unwrap_or(0);

    if input_total == 0 && input_cached_tokens == 0 && output_tokens == 0 {
        return None;
    }

    Some(UsageBreakdown {
        input_uncached_tokens: input_total.saturating_sub(input_cached_tokens),
        input_cached_tokens,
        output_tokens,
        usage_missing: false,
    })
}

/// Incrementally collects usage and the terminal response from an SSE stream.
#[derive(Default)]
pub(crate) struct SseUsageCollector {
    pub usage: Option<UsageBreakdown>,
    pub completed_response: Option<Value>,
}

impl SseUsageCollector {
    /// Observe one upstream SSE event and extract usage or terminal response
    /// state.
    pub fn observe_event(&mut self, event: &SseEvent) {
        let payload = event.data.trim();
        if payload.is_empty() || payload == "[DONE]" {
            return;
        }
        if let Ok(value) = serde_json::from_str::<Value>(payload) {
            if let Some(usage) = extract_usage_from_value(&value) {
                self.usage = Some(usage);
            }
            if value.get("type").and_then(Value::as_str) == Some("response.completed") {
                if let Some(response) = value.get("response") {
                    self.completed_response = Some(response.clone());
                }
            }
        }
    }
}

/// Re-encode a parsed SSE event so it can be forwarded downstream unchanged.
pub(crate) fn encode_sse_event(event: &SseEvent) -> Bytes {
    let mut encoded = String::new();
    if !event.event.is_empty() {
        encoded.push_str("event: ");
        encoded.push_str(&event.event);
        encoded.push('\n');
    }
    if !event.id.is_empty() {
        encoded.push_str("id: ");
        encoded.push_str(&event.id);
        encoded.push('\n');
    }
    if let Some(retry) = event.retry {
        encoded.push_str("retry: ");
        encoded.push_str(&retry.as_millis().to_string());
        encoded.push('\n');
    }
    for line in event.data.split('\n') {
        encoded.push_str("data: ");
        encoded.push_str(line);
        encoded.push('\n');
    }
    encoded.push('\n');
    Bytes::from(encoded)
}

pub(crate) fn encode_sse_event_with_model_alias(
    event: &SseEvent,
    model_from: Option<&str>,
    model_to: Option<&str>,
) -> Bytes {
    let payload = event.data.trim();
    if payload.is_empty() || payload == "[DONE]" {
        return encode_sse_event(event);
    }
    let Ok(value) = serde_json::from_str::<Value>(payload) else {
        return encode_sse_event(event);
    };
    let value = maybe_apply_model_alias(value, model_from, model_to);
    let data = serde_json::to_string(&value).unwrap_or_else(|_| event.data.clone());
    let aliased_event = SseEvent {
        event: event.event.clone(),
        data,
        id: event.id.clone(),
        retry: event.retry,
    };
    encode_sse_event(&aliased_event)
}

/// Copy selected upstream headers onto the final downstream response.
pub(crate) fn apply_upstream_response_headers(
    mut builder: axum::http::response::Builder,
    upstream_headers: &reqwest::header::HeaderMap,
) -> axum::http::response::Builder {
    for (name, value) in upstream_headers {
        if should_forward_upstream_header(name) {
            builder = builder.header(name, value);
        }
    }
    builder
}

/// Filter hop-by-hop and locally rewritten headers out of forwarded responses.
fn should_forward_upstream_header(name: &reqwest::header::HeaderName) -> bool {
    let lower = name.as_str().to_ascii_lowercase();
    !matches!(
        lower.as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailers"
            | "transfer-encoding"
            | "upgrade"
            | "content-length"
            | "content-encoding"
            | "content-type"
            | "cache-control"
    )
}
