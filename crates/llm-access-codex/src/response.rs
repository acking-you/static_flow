//! Codex response, usage, and SSE adaptation helpers.

use std::collections::BTreeMap;

use axum::body::Bytes;
use eventsource_stream::Event as SseEvent;
use serde_json::{json, Map, Value};

use crate::{
    anthropic_messages::map_response_to_anthropic_message,
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

fn responses_tool_call_arguments_string(item_obj: &Map<String, Value>, item_type: &str) -> String {
    let value = match item_type {
        "custom_tool_call" => item_obj.get("input"),
        _ => item_obj.get("arguments"),
    };
    value.map_or_else(
        || "{}".to_string(),
        |raw| {
            if let Some(text) = raw.as_str() {
                text.to_string()
            } else {
                serde_json::to_string(raw).unwrap_or_else(|_| "{}".to_string())
            }
        },
    )
}

fn responses_tool_call_event_delta_text(value: &Value) -> &str {
    value
        .get("delta")
        .or_else(|| value.get("input"))
        .or_else(|| value.get("arguments"))
        .and_then(Value::as_str)
        .unwrap_or("")
}

fn serialize_json_string(value: &Value, default: &str) -> String {
    if let Some(text) = value.as_str() {
        return text.to_string();
    }
    serde_json::to_string(value).unwrap_or_else(|_| default.to_string())
}

fn chat_tool_call_from_responses_item(
    item_obj: &Map<String, Value>,
    item_type: &str,
    tool_name_restore_map: Option<&BTreeMap<String, String>>,
) -> Option<Value> {
    match item_type {
        "function_call" | "custom_tool_call" => {
            let call_id = item_obj
                .get("call_id")
                .or_else(|| item_obj.get("id"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())?;
            let name = item_obj
                .get("name")
                .and_then(Value::as_str)
                .map(|name| restore_openai_tool_name(name, tool_name_restore_map))
                .unwrap_or_else(|| "tool".to_string());
            let arguments = responses_tool_call_arguments_string(item_obj, item_type);
            Some(json!({
                "id": call_id,
                "type": "function",
                "function": {
                    "name": name,
                    "arguments": arguments
                }
            }))
        },
        "local_shell_call" => {
            let call_id = item_obj
                .get("call_id")
                .or_else(|| item_obj.get("id"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())?;
            let arguments = item_obj
                .get("action")
                .map(|action| serialize_json_string(action, "{}"))
                .unwrap_or_else(|| "{}".to_string());
            Some(json!({
                "id": call_id,
                "type": "function",
                "function": {
                    "name": "local_shell",
                    "arguments": arguments
                }
            }))
        },
        _ => None,
    }
}

fn chat_extension_item_from_responses_item(
    item_obj: &Map<String, Value>,
    item_type: &str,
) -> Option<Value> {
    match item_type {
        "web_search_call" | "image_generation_call" => Some(Value::Object(item_obj.clone())),
        _ => None,
    }
}

fn object_u64_field(map: &Map<String, Value>, key: &str) -> Option<u64> {
    map.get(key).and_then(Value::as_u64)
}

fn nested_u64_field(map: &Map<String, Value>, object_key: &str, key: &str) -> Option<u64> {
    map.get(object_key)
        .and_then(Value::as_object)
        .and_then(|obj| obj.get(key))
        .and_then(Value::as_u64)
}

fn ensure_object_field<'a>(
    map: &'a mut Map<String, Value>,
    key: &str,
) -> &'a mut Map<String, Value> {
    if !map.get(key).is_some_and(Value::is_object) {
        map.insert(key.to_string(), json!({}));
    }
    map.get_mut(key)
        .and_then(Value::as_object_mut)
        .expect("object field")
}

fn copy_detail_field_if_absent(
    target: &mut Map<String, Value>,
    source: Option<&Map<String, Value>>,
    key: &str,
) {
    if target.contains_key(key) {
        return;
    }
    if let Some(value) = source.and_then(|details| details.get(key)).cloned() {
        target.insert(key.to_string(), value);
    }
}

fn normalize_responses_usage_for_chat_completion(usage: Value) -> Value {
    let Value::Object(mut map) = usage else {
        return usage;
    };

    let input_tokens = object_u64_field(&map, "input_tokens");
    let output_tokens = object_u64_field(&map, "output_tokens");
    let prompt_tokens = object_u64_field(&map, "prompt_tokens").or(input_tokens);
    let completion_tokens = object_u64_field(&map, "completion_tokens").or(output_tokens);
    let total_tokens = object_u64_field(&map, "total_tokens").or_else(|| {
        prompt_tokens
            .or(completion_tokens)
            .map(|_| prompt_tokens.unwrap_or(0) + completion_tokens.unwrap_or(0))
    });
    let cached_tokens = nested_u64_field(&map, "prompt_tokens_details", "cached_tokens")
        .or_else(|| nested_u64_field(&map, "input_tokens_details", "cached_tokens"))
        .or_else(|| object_u64_field(&map, "prompt_cache_hit_tokens"))
        .or_else(|| object_u64_field(&map, "cached_tokens"));
    let input_details = map
        .get("input_tokens_details")
        .and_then(Value::as_object)
        .cloned();
    let output_details = map
        .get("output_tokens_details")
        .and_then(Value::as_object)
        .cloned();

    if let Some(prompt_tokens) = prompt_tokens {
        map.insert("prompt_tokens".to_string(), json!(prompt_tokens));
    }
    if let Some(completion_tokens) = completion_tokens {
        map.insert("completion_tokens".to_string(), json!(completion_tokens));
    }
    if let Some(total_tokens) = total_tokens {
        map.insert("total_tokens".to_string(), json!(total_tokens));
    }

    if cached_tokens.is_some() || input_details.is_some() {
        let prompt_details = ensure_object_field(&mut map, "prompt_tokens_details");
        if let Some(cached_tokens) = cached_tokens {
            prompt_details.insert("cached_tokens".to_string(), json!(cached_tokens));
        }
        for key in ["cached_creation_tokens", "text_tokens", "audio_tokens", "image_tokens"] {
            copy_detail_field_if_absent(prompt_details, input_details.as_ref(), key);
        }
    }

    if let Some(output_details) = output_details.as_ref() {
        let completion_details = ensure_object_field(&mut map, "completion_tokens_details");
        for key in ["reasoning_tokens", "text_tokens", "audio_tokens"] {
            copy_detail_field_if_absent(completion_details, Some(output_details), key);
        }
    }

    Value::Object(map)
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
    let mut codex_output_items = Vec::<Value>::new();
    if let Some(output_items) = source.get("output").and_then(Value::as_array) {
        for item in output_items {
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
                "function_call" | "custom_tool_call" | "local_shell_call" => {
                    if let Some(tool_call) = chat_tool_call_from_responses_item(
                        item_obj,
                        item_type,
                        tool_name_restore_map,
                    ) {
                        tool_calls.push(tool_call);
                    }
                },
                "function_call_output" | "custom_tool_call_output" => {
                    if let Some(output) = item_obj.get("output") {
                        map_response_content_text(output, &mut assistant_text);
                    }
                },
                "web_search_call" | "image_generation_call" => {
                    if let Some(extension) =
                        chat_extension_item_from_responses_item(item_obj, item_type)
                    {
                        codex_output_items.push(extension);
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
    if !codex_output_items.is_empty() {
        message.insert("codex_output_items".to_string(), Value::Array(codex_output_items));
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
                        .is_some_and(|kind| {
                            matches!(kind, "function_call" | "custom_tool_call" | "local_shell_call")
                        })
                })) {
                "tool_calls"
            } else {
                "stop"
            }
        })]),
    );
    if let Some(usage) = usage {
        out.insert("usage".to_string(), normalize_responses_usage_for_chat_completion(usage));
    }
    Value::Object(out)
}

/// Adapt a completed upstream response according to the selected response mode.
pub fn adapt_completed_response_json(
    response: Value,
    adapter: GatewayResponseAdapter,
    tool_name_restore_map: Option<&BTreeMap<String, String>>,
) -> Value {
    match adapter {
        GatewayResponseAdapter::Responses => response,
        GatewayResponseAdapter::ChatCompletions => {
            map_response_to_chat_completion(&response, tool_name_restore_map)
        },
        GatewayResponseAdapter::AnthropicMessages => {
            map_response_to_anthropic_message(&response, tool_name_restore_map)
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

fn stream_tool_call_identity_from_item(item: &Value) -> Option<(String, String)> {
    let item_obj = item.as_object()?;
    if let Some(call_id) = item_obj
        .get("call_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let call_id = call_id.to_string();
        return Some((format!("call:{call_id}"), call_id));
    }
    let item_id = item_obj
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_string();
    Some((format!("item:{item_id}"), item_id))
}

fn stream_tool_call_identity_from_event(value: &Value) -> Option<(String, String)> {
    if let Some(call_id) = value
        .get("call_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let call_id = call_id.to_string();
        return Some((format!("call:{call_id}"), call_id));
    }
    let item_id = value
        .get("item_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_string();
    Some((format!("item:{item_id}"), item_id))
}

/// Convert one responses stream event into an OpenAI chat chunk when possible.
fn convert_response_value_to_chat_chunk(
    value: &Value,
    tool_name_restore_map: Option<&BTreeMap<String, String>>,
    metadata: &mut ChatStreamMetadata,
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
        "response.output_item.added" => {
            let item = value.get("item").or_else(|| value.get("output_item"))?;
            let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
            if matches!(item_type, "function_call" | "custom_tool_call" | "local_shell_call") {
                let (lookup_key, call_id) = stream_tool_call_identity_from_item(item)?;
                let tool_call_index = metadata.tool_call_index(&lookup_key);
                let tool_call = item.as_object().and_then(|obj| {
                    chat_tool_call_from_responses_item(obj, item_type, tool_name_restore_map)
                })?;
                let name = tool_call["function"]["name"].as_str().unwrap_or("tool");
                let arguments = tool_call["function"]["arguments"]
                    .as_str()
                    .unwrap_or("{}")
                    .to_string();
                let has_payload = !arguments.is_empty() && arguments != "{}";
                if !metadata.mark_tool_call_started(&lookup_key, has_payload) {
                    return None;
                }
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
                                "index": tool_call_index,
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
            if let Some(item_obj) = item.as_object() {
                if let Some(extension) =
                    chat_extension_item_from_responses_item(item_obj, item_type)
                {
                    return Some(json!({
                        "id": stream_event_response_id(value),
                        "object": "chat.completion.chunk",
                        "created": stream_event_created(value),
                        "model": stream_event_model(value),
                        "choices": [{
                            "index": 0,
                            "delta": {
                                "codex_output_items": [extension]
                            },
                            "finish_reason": Value::Null
                        }]
                    }));
                }
            }
            None
        },
        "response.output_item.done" => {
            let item = value.get("item").or_else(|| value.get("output_item"))?;
            let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
            if matches!(item_type, "function_call" | "custom_tool_call" | "local_shell_call") {
                let (lookup_key, call_id) = stream_tool_call_identity_from_item(item)?;
                let tool_call_index = metadata.tool_call_index(&lookup_key);
                let tool_call = item.as_object().and_then(|obj| {
                    chat_tool_call_from_responses_item(obj, item_type, tool_name_restore_map)
                })?;
                let name = tool_call["function"]["name"].as_str().unwrap_or("tool");
                let arguments = tool_call["function"]["arguments"]
                    .as_str()
                    .unwrap_or("{}")
                    .to_string();
                let has_payload = !arguments.is_empty() && arguments != "{}";
                let first_start = metadata.mark_tool_call_started(&lookup_key, has_payload);
                if first_start {
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
                                    "index": tool_call_index,
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
                if metadata.tool_call_delta_seen(&lookup_key)
                    || metadata.tool_call_start_had_payload(&lookup_key)
                    || !has_payload
                {
                    return None;
                }
                metadata.mark_tool_call_delta_seen(&lookup_key);
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
                                "index": tool_call_index,
                                "id": call_id,
                                "type": "function",
                                "function": {
                                    "arguments": arguments
                                }
                            }]
                        },
                        "finish_reason": Value::Null
                    }]
                }));
            }
            if let Some(item_obj) = item.as_object() {
                if let Some(extension) =
                    chat_extension_item_from_responses_item(item_obj, item_type)
                {
                    return Some(json!({
                        "id": stream_event_response_id(value),
                        "object": "chat.completion.chunk",
                        "created": stream_event_created(value),
                        "model": stream_event_model(value),
                        "choices": [{
                            "index": 0,
                            "delta": {
                                "codex_output_items": [extension]
                            },
                            "finish_reason": Value::Null
                        }]
                    }));
                }
            }
            None
        },
        "response.function_call_arguments.delta" | "response.custom_tool_call_input.delta" => {
            let (lookup_key, call_id) = stream_tool_call_identity_from_event(value)?;
            let arguments = responses_tool_call_event_delta_text(value);
            if arguments.is_empty() {
                return None;
            }
            if !metadata.tool_call_started(&lookup_key) {
                return None;
            }
            let tool_call_index = metadata.tool_call_index(&lookup_key);
            metadata.mark_tool_call_delta_seen(&lookup_key);
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
                            "index": tool_call_index,
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
                                matches!(
                                    kind,
                                    "function_call" | "custom_tool_call" | "local_shell_call"
                                )
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
                    obj.insert(
                        "usage".to_string(),
                        normalize_responses_usage_for_chat_completion(usage),
                    );
                }
            }
            Some(out)
        },
        _ => None,
    }
}

/// Convert one parsed SSE event into a chat chunk and update stream defaults.
pub fn convert_response_event_to_chat_chunk(
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
    let mut chunk = convert_response_value_to_chat_chunk(&value, tool_name_restore_map, metadata)?;
    fill_chat_chunk_defaults(&mut chunk, metadata);
    Some(chunk)
}

/// Encode a JSON payload as a single SSE `data:` chunk.
pub fn encode_json_sse_chunk(value: &Value) -> Bytes {
    let body = serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string());
    Bytes::from(format!("data: {body}\n\n"))
}

/// Convert a non-streaming responses JSON payload into chat/completions JSON.
pub fn convert_json_response_to_chat_completion(
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::convert_response_value_to_chat_chunk;
    use crate::types::ChatStreamMetadata;

    #[test]
    fn streamed_tool_calls_keep_stable_indices_per_call_id() {
        let mut metadata = ChatStreamMetadata::default();
        let first = json!({
            "type": "response.output_item.done",
            "item": {
                "type": "function_call",
                "call_id": "callauto11",
                "name": "alpha",
                "arguments": "{}"
            }
        });
        let second = json!({
            "type": "response.output_item.done",
            "item": {
                "type": "function_call",
                "call_id": "callauto12",
                "name": "beta",
                "arguments": "{}"
            }
        });
        let second_delta = json!({
            "type": "response.function_call_arguments.delta",
            "call_id": "callauto12",
            "delta": "{\"k\":1}"
        });

        let first_chunk =
            convert_response_value_to_chat_chunk(&first, None, &mut metadata).expect("chunk");
        let second_chunk =
            convert_response_value_to_chat_chunk(&second, None, &mut metadata).expect("chunk");
        let second_delta_chunk =
            convert_response_value_to_chat_chunk(&second_delta, None, &mut metadata)
                .expect("chunk");

        assert_eq!(first_chunk["choices"][0]["delta"]["tool_calls"][0]["index"], json!(0));
        assert_eq!(second_chunk["choices"][0]["delta"]["tool_calls"][0]["index"], json!(1));
        assert_eq!(second_delta_chunk["choices"][0]["delta"]["tool_calls"][0]["index"], json!(1));
    }

    #[test]
    fn streamed_tool_call_without_any_identifier_is_dropped() {
        let mut metadata = ChatStreamMetadata::default();
        let value = json!({
            "type": "response.output_item.done",
            "item": {
                "type": "function_call",
                "name": "alpha",
                "arguments": "{}"
            }
        });

        assert!(convert_response_value_to_chat_chunk(&value, None, &mut metadata).is_none());
    }

    #[test]
    fn streamed_tool_call_delta_before_start_is_dropped() {
        let mut metadata = ChatStreamMetadata::default();
        let delta = json!({
            "type": "response.function_call_arguments.delta",
            "call_id": "callauto13",
            "delta": "{\"k\":1}"
        });

        assert!(convert_response_value_to_chat_chunk(&delta, None, &mut metadata).is_none());
    }

    #[test]
    fn streamed_tool_call_done_after_dropped_delta_emits_full_start_chunk() {
        let mut metadata = ChatStreamMetadata::default();
        let delta = json!({
            "type": "response.function_call_arguments.delta",
            "call_id": "callauto13",
            "delta": "{\"k\":1}"
        });
        let done = json!({
            "type": "response.output_item.done",
            "item": {
                "type": "function_call",
                "call_id": "callauto13",
                "name": "alpha",
                "arguments": "{\"k\":1}"
            }
        });

        assert!(convert_response_value_to_chat_chunk(&delta, None, &mut metadata).is_none());
        let done_chunk =
            convert_response_value_to_chat_chunk(&done, None, &mut metadata).expect("done");

        assert_eq!(done_chunk["choices"][0]["delta"]["tool_calls"][0]["index"], json!(0));
        assert_eq!(
            done_chunk["choices"][0]["delta"]["tool_calls"][0]["function"]["name"],
            json!("alpha")
        );
        assert_eq!(
            done_chunk["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"],
            json!("{\"k\":1}")
        );
    }

    #[test]
    fn streamed_tool_call_done_after_start_backfills_arguments_without_second_start() {
        let mut metadata = ChatStreamMetadata::default();
        let added = json!({
            "type": "response.output_item.added",
            "item": {
                "type": "function_call",
                "call_id": "callauto14",
                "name": "alpha",
                "arguments": "{}"
            }
        });
        let done = json!({
            "type": "response.output_item.done",
            "item": {
                "type": "function_call",
                "call_id": "callauto14",
                "name": "alpha",
                "arguments": "{\"k\":1}"
            }
        });

        let start_chunk =
            convert_response_value_to_chat_chunk(&added, None, &mut metadata).expect("start");
        let done_chunk =
            convert_response_value_to_chat_chunk(&done, None, &mut metadata).expect("done");

        assert_eq!(
            start_chunk["choices"][0]["delta"]["tool_calls"][0]["function"]["name"],
            json!("alpha")
        );
        assert_eq!(
            start_chunk["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"],
            json!("{}")
        );
        assert_eq!(done_chunk["choices"][0]["delta"]["tool_calls"][0]["index"], json!(0));
        assert!(done_chunk["choices"][0]["delta"]["tool_calls"][0]["function"]
            .get("name")
            .is_none());
        assert_eq!(
            done_chunk["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"],
            json!("{\"k\":1}")
        );
    }

    #[test]
    fn streamed_custom_tool_call_input_delta_uses_same_tool_index_without_duplicate_done_chunk() {
        let mut metadata = ChatStreamMetadata::default();
        let added = json!({
            "type": "response.output_item.added",
            "item": {
                "type": "custom_tool_call",
                "call_id": "callpatch1",
                "name": "apply_patch",
                "input": ""
            }
        });
        let delta = json!({
            "type": "response.custom_tool_call_input.delta",
            "call_id": "callpatch1",
            "delta": "*** Begin Patch"
        });
        let done = json!({
            "type": "response.output_item.done",
            "item": {
                "type": "custom_tool_call",
                "call_id": "callpatch1",
                "name": "apply_patch",
                "input": "*** Begin Patch"
            }
        });

        let start_chunk =
            convert_response_value_to_chat_chunk(&added, None, &mut metadata).expect("start");
        let delta_chunk =
            convert_response_value_to_chat_chunk(&delta, None, &mut metadata).expect("delta");
        let done_chunk = convert_response_value_to_chat_chunk(&done, None, &mut metadata);

        assert_eq!(start_chunk["choices"][0]["delta"]["tool_calls"][0]["index"], json!(0));
        assert_eq!(delta_chunk["choices"][0]["delta"]["tool_calls"][0]["index"], json!(0));
        assert_eq!(
            delta_chunk["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"],
            json!("*** Begin Patch")
        );
        assert!(done_chunk.is_none());
    }

    #[test]
    fn completed_chat_completion_maps_local_shell_call_and_preserves_server_tools() {
        let value = json!({
            "id": "resp_1",
            "model": "gpt-5.3-codex",
            "output": [
                {
                    "type": "local_shell_call",
                    "call_id": "callshell1",
                    "status": "completed",
                    "action": {"type":"exec","command":["pwd"]}
                },
                {
                    "type": "web_search_call",
                    "id": "ws_1",
                    "status": "completed",
                    "action": {"type":"search","query":"weather seattle"}
                },
                {
                    "type": "image_generation_call",
                    "id": "ig_1",
                    "status": "completed",
                    "revised_prompt": "blue square",
                    "result": "Zm9v"
                }
            ]
        });

        let mapped = super::map_response_to_chat_completion(&value, None);
        assert_eq!(
            mapped["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
            json!("local_shell")
        );
        assert_eq!(
            mapped["choices"][0]["message"]["codex_output_items"][0]["type"],
            json!("web_search_call")
        );
        assert_eq!(
            mapped["choices"][0]["message"]["codex_output_items"][1]["type"],
            json!("image_generation_call")
        );
        assert_eq!(mapped["choices"][0]["finish_reason"], json!("tool_calls"));
    }

    #[test]
    fn completed_chat_completion_normalizes_responses_usage_for_openai_clients() {
        let value = json!({
            "id": "resp_1",
            "model": "gpt-5.3-codex",
            "usage": {
                "input_tokens": 100,
                "input_tokens_details": {
                    "cached_tokens": 40
                },
                "output_tokens": 25
            },
            "output": [
                {
                    "type": "message",
                    "content": [
                        {"type": "output_text", "text": "done"}
                    ]
                }
            ]
        });

        let mapped = super::map_response_to_chat_completion(&value, None);
        assert_eq!(mapped["usage"]["prompt_tokens"], json!(100));
        assert_eq!(mapped["usage"]["completion_tokens"], json!(25));
        assert_eq!(mapped["usage"]["total_tokens"], json!(125));
        assert_eq!(mapped["usage"]["prompt_tokens_details"]["cached_tokens"], json!(40));
        assert_eq!(mapped["usage"]["input_tokens"], json!(100));
        assert_eq!(mapped["usage"]["output_tokens"], json!(25));
    }

    #[test]
    fn streamed_completed_chat_chunk_normalizes_responses_usage_for_openai_clients() {
        let mut metadata = ChatStreamMetadata::default();
        let value = json!({
            "type": "response.completed",
            "response": {
                "id": "resp_1",
                "model": "gpt-5.3-codex",
                "usage": {
                    "input_tokens": 50,
                    "input_tokens_details": {
                        "cached_tokens": 30
                    },
                    "output_tokens": 20
                }
            }
        });

        let chunk =
            convert_response_value_to_chat_chunk(&value, None, &mut metadata).expect("chunk");
        assert_eq!(chunk["usage"]["prompt_tokens"], json!(50));
        assert_eq!(chunk["usage"]["completion_tokens"], json!(20));
        assert_eq!(chunk["usage"]["total_tokens"], json!(70));
        assert_eq!(chunk["usage"]["prompt_tokens_details"]["cached_tokens"], json!(30));
        assert_eq!(chunk["usage"]["input_tokens"], json!(50));
        assert_eq!(chunk["usage"]["output_tokens"], json!(20));
    }

    #[test]
    fn streamed_web_search_call_is_preserved_as_chat_extension_chunk() {
        let mut metadata = ChatStreamMetadata::default();
        let value = json!({
            "type": "response.output_item.done",
            "item": {
                "type": "web_search_call",
                "id": "ws_1",
                "status": "completed",
                "action": {"type":"search","query":"weather seattle"}
            }
        });

        let chunk =
            convert_response_value_to_chat_chunk(&value, None, &mut metadata).expect("chunk");
        assert_eq!(
            chunk["choices"][0]["delta"]["codex_output_items"][0]["type"],
            json!("web_search_call")
        );
    }
}

/// Rewrite model aliases inside a non-streaming JSON response body.
pub fn rewrite_json_response_model_alias(
    bytes: &[u8],
    model_from: Option<&str>,
    model_to: Option<&str>,
) -> Option<Vec<u8>> {
    if !should_rewrite_model_alias(model_from, model_to) {
        return None;
    }
    let value = serde_json::from_slice::<Value>(bytes).ok()?;
    let value = maybe_apply_model_alias(value, model_from, model_to);
    serde_json::to_vec(&value).ok()
}

/// Rewrite model aliases inside an already-parsed JSON value.
pub fn rewrite_json_value_model_alias(
    value: Value,
    model_from: Option<&str>,
    model_to: Option<&str>,
) -> Value {
    if !should_rewrite_model_alias(model_from, model_to) {
        return value;
    }
    maybe_apply_model_alias(value, model_from, model_to)
}

fn should_rewrite_model_alias(model_from: Option<&str>, model_to: Option<&str>) -> bool {
    matches!((model_from, model_to), (Some(from), Some(to)) if from != to)
}

/// Parse a non-streaming JSON body and extract usage accounting when present.
pub fn extract_usage_from_bytes(bytes: &[u8]) -> Option<UsageBreakdown> {
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
pub struct SseUsageCollector {
    /// Latest usage breakdown observed in the stream.
    pub usage: Option<UsageBreakdown>,
    /// Terminal completed response observed in the stream.
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
pub fn encode_sse_event(event: &SseEvent) -> Bytes {
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

/// Re-encode an SSE event after applying a recursive model alias rewrite.
pub fn encode_sse_event_with_model_alias(
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
pub fn apply_upstream_response_headers(
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
