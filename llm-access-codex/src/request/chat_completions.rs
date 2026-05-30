//! OpenAI Chat Completions -> /responses input adaptation: role/content
//! mapping, message assembly.


use std::collections::BTreeSet;

use serde_json::{json, Map, Value};

use super::{
    coerce_non_empty_scalar_to_string,
    normalization::normalize_reasoning_effort,
    tools::{
        build_openai_tool_name_map, build_openai_tool_name_restore_map, get_dynamic_tools_array,
        is_openai_chat_function_tool, legacy_openai_function_name_value,
        map_openai_chat_function_tool, normalize_tool_parameters_schema,
        openai_chat_tool_name_value, shorten_openai_tool_name_with_map,
    },
};
use crate::{
    error::{bad_request, bad_request_with_detail, CodexGatewayError, CodexGatewayResult},
    types::OpenAiChatAdaptedRequest,
};
/// Map OpenAI chat roles into the role set accepted by the responses API.
fn normalize_openai_role_for_responses(role: &str) -> Option<&'static str> {
    match role {
        "system" | "developer" => Some("developer"),
        "user" => Some("user"),
        "assistant" => Some("assistant"),
        "tool" => Some("tool"),
        _ => None,
    }
}
/// Flatten mixed chat message content into plain text instructions.
fn extract_openai_message_content_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(items) => {
            let mut out = String::new();
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
            out
        },
        Value::Null => String::new(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}
/// Convert one chat user content item into a responses input item.
fn map_openai_user_content_item_to_responses_item(item: &Value) -> Option<Value> {
    if let Some(text) = item.as_str() {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return None;
        }
        return Some(json!({
            "type": "input_text",
            "text": trimmed,
        }));
    }

    let obj = item.as_object()?;
    let item_type = obj.get("type").and_then(Value::as_str).unwrap_or_default();
    match item_type {
        "text" | "input_text" | "output_text" => obj
            .get("text")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|text| {
                json!({
                    "type": "input_text",
                    "text": text,
                })
            }),
        "input_image" => {
            let mut mapped = Map::new();
            mapped.insert("type".to_string(), Value::String("input_image".to_string()));
            if let Some(image_url) = obj.get("image_url").cloned() {
                mapped.insert("image_url".to_string(), image_url);
            } else if let Some(file_id) = obj.get("file_id").cloned() {
                mapped.insert("file_id".to_string(), file_id);
            } else {
                return None;
            }
            Some(Value::Object(mapped))
        },
        "image_url" => {
            let image_url = obj
                .get("image_url")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .or_else(|| {
                    obj.get("image_url")
                        .and_then(Value::as_object)
                        .and_then(|image_url| image_url.get("url"))
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(ToString::to_string)
                })?;
            Some(json!({
                "type": "input_image",
                "image_url": image_url,
            }))
        },
        _ => None,
    }
}
/// Convert chat-style user content into responses input content items.
pub fn convert_user_message_content_to_responses_items(content: &Value) -> Vec<Value> {
    match content {
        Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                Vec::new()
            } else {
                vec![json!({
                    "type": "input_text",
                    "text": trimmed,
                })]
            }
        },
        Value::Array(items) => items
            .iter()
            .filter_map(map_openai_user_content_item_to_responses_item)
            .collect(),
        Value::Null => Vec::new(),
        other => {
            let text = serde_json::to_string(other).unwrap_or_default();
            let trimmed = text.trim();
            if trimmed.is_empty() {
                Vec::new()
            } else {
                vec![json!({
                    "type": "input_text",
                    "text": trimmed,
                })]
            }
        },
    }
}
/// Convert a chat tool-output content item into a responses output item.
fn map_tool_result_content_item_to_responses_output_item(item: &Value) -> Option<Value> {
    if let Some(text) = item.as_str() {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return None;
        }
        return Some(json!({
            "type": "input_text",
            "text": trimmed,
        }));
    }

    let obj = item.as_object()?;
    let item_type = obj.get("type").and_then(Value::as_str).unwrap_or_default();
    match item_type {
        "text" | "input_text" => obj
            .get("text")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|text| {
                json!({
                    "type": "input_text",
                    "text": text,
                })
            }),
        "input_image" => {
            let mut mapped = Map::new();
            mapped.insert("type".to_string(), Value::String("input_image".to_string()));
            if let Some(image_url) = obj.get("image_url").cloned() {
                mapped.insert("image_url".to_string(), image_url);
            } else if let Some(file_id) = obj.get("file_id").cloned() {
                mapped.insert("file_id".to_string(), file_id);
            } else {
                return None;
            }
            Some(Value::Object(mapped))
        },
        _ => serde_json::to_string(item).ok().and_then(|text| {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(json!({
                    "type": "input_text",
                    "text": trimmed,
                }))
            }
        }),
    }
}
/// Convert a chat `tool` role payload into the responses function-call output
/// shape.
pub fn convert_tool_message_content_to_responses_output(
    value: Option<&Value>,
) -> Result<Value, String> {
    let Some(value) = value else {
        return Ok(Value::String(String::new()));
    };
    if value.is_null() {
        return Ok(Value::String(String::new()));
    }
    if let Some(text) = value.as_str() {
        return Ok(Value::String(text.to_string()));
    }
    if let Some(items) = value.as_array() {
        let mapped_items = items
            .iter()
            .filter_map(map_tool_result_content_item_to_responses_output_item)
            .collect::<Vec<_>>();
        if mapped_items.is_empty() {
            return Ok(Value::String(String::new()));
        }
        return Ok(Value::Array(mapped_items));
    }
    if let Some(item) = map_tool_result_content_item_to_responses_output_item(value) {
        return Ok(Value::Array(vec![item]));
    }
    serde_json::to_string(value)
        .map(Value::String)
        .map_err(|err| format!("serialize tool result content failed: {err}"))
}
/// Flush pending assistant text parts into a single responses assistant
/// message.
fn flush_assistant_output_parts(input_items: &mut Vec<Value>, pending_parts: &mut Vec<Value>) {
    if pending_parts.is_empty() {
        return;
    }
    input_items.push(json!({
        "type": "message",
        "role": "assistant",
        "content": pending_parts.clone(),
    }));
    pending_parts.clear();
}
/// Convert assistant chat history into responses message items.
fn append_assistant_content_to_responses_input(
    input_items: &mut Vec<Value>,
    content: &Value,
) -> Result<(), String> {
    if let Some(text) = content.as_str() {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            input_items.push(json!({
                "type": "message",
                "role": "assistant",
                "content": [{ "type": "output_text", "text": trimmed }]
            }));
        }
        return Ok(());
    }

    let items = if let Some(array) = content.as_array() {
        array.to_vec()
    } else if content.is_object() {
        vec![content.clone()]
    } else if content.is_null() {
        Vec::new()
    } else {
        return Err("unsupported assistant content".to_string());
    };

    let mut pending_parts = Vec::new();
    for item in items {
        let Some(item_obj) = item.as_object() else {
            continue;
        };
        let item_type = item_obj
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match item_type {
            "text" | "output_text" => {
                if let Some(text) = item_obj.get("text").and_then(Value::as_str) {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        pending_parts.push(json!({
                            "type": "output_text",
                            "text": trimmed,
                        }));
                    }
                }
            },
            _ => {},
        }
    }
    flush_assistant_output_parts(input_items, &mut pending_parts);
    Ok(())
}
fn chat_message_bad_request(index: usize, message: impl AsRef<str>) -> CodexGatewayError {
    bad_request(&format!("chat.completions message {index}: {}", message.as_ref()))
}
/// Adapt an OpenAI chat/completions request into the upstream responses format.
pub fn adapt_openai_chat_completions_request(
    obj: &Map<String, Value>,
) -> CodexGatewayResult<OpenAiChatAdaptedRequest> {
    let source_messages = obj
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| bad_request("chat.completions messages field is required"))?;
    let tool_name_map = build_openai_tool_name_map(obj);
    let tool_name_restore_map = build_openai_tool_name_restore_map(&tool_name_map);

    let mut input_items = Vec::<Value>::new();
    let mut seen_tool_call_ids = BTreeSet::<String>::new();

    for (index, message) in source_messages.iter().enumerate() {
        let Some(message_obj) = message.as_object() else {
            return Err(chat_message_bad_request(index, "must be an object"));
        };
        let Some(role) = message_obj.get("role").and_then(Value::as_str) else {
            return Err(chat_message_bad_request(index, "is missing role"));
        };
        let Some(normalized_role) = normalize_openai_role_for_responses(role) else {
            return Err(chat_message_bad_request(index, format!("has unsupported role `{role}`")));
        };
        match normalized_role {
            "developer" => {
                let Some(content) = message_obj.get("content") else {
                    return Err(chat_message_bad_request(index, "is missing content"));
                };
                if !matches!(content, Value::String(_) | Value::Array(_)) {
                    return Err(chat_message_bad_request(
                        index,
                        "content must be a string or array",
                    ));
                }
                let text = extract_openai_message_content_text(content);
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    continue;
                }
                input_items.push(json!({
                    "type": "message",
                    "role": "developer",
                    "content": [{
                        "type": "input_text",
                        "text": trimmed,
                    }]
                }));
            },
            "user" => {
                let Some(content) = message_obj.get("content") else {
                    return Err(chat_message_bad_request(index, "is missing content"));
                };
                if !matches!(content, Value::String(_) | Value::Array(_)) {
                    return Err(chat_message_bad_request(
                        index,
                        "content must be a string or array",
                    ));
                }
                let content_items = convert_user_message_content_to_responses_items(content);
                if content_items.is_empty() {
                    continue;
                }
                input_items.push(json!({
                    "type": "message",
                    "role": "user",
                    "content": content_items
                }));
            },
            "assistant" => {
                let mut emitted_content = false;
                if let Some(content) = message_obj.get("content") {
                    if !matches!(
                        content,
                        Value::String(_) | Value::Array(_) | Value::Object(_) | Value::Null
                    ) {
                        return Err(chat_message_bad_request(
                            index,
                            "content has unsupported shape",
                        ));
                    }
                    let prior_len = input_items.len();
                    append_assistant_content_to_responses_input(&mut input_items, content)
                        .map_err(|err| bad_request_with_detail("Invalid assistant content", err))?;
                    emitted_content = input_items.len() > prior_len;
                }
                let mut emitted_tool_call = false;
                if let Some(raw_tool_calls) = message_obj.get("tool_calls") {
                    let Some(tool_calls) = raw_tool_calls.as_array() else {
                        return Err(chat_message_bad_request(index, "tool_calls must be an array"));
                    };
                    for (tool_call_index, tool_call) in tool_calls.iter().enumerate() {
                        let Some(tool_obj) = tool_call.as_object() else {
                            return Err(bad_request(&format!(
                                "chat.completions assistant tool_call {tool_call_index} must be \
                                 an object"
                            )));
                        };
                        let call_id = tool_obj
                            .get("id")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .ok_or_else(|| {
                                bad_request(&format!(
                                    "chat.completions assistant tool_call {tool_call_index} is \
                                     missing id"
                                ))
                            })?;
                        let Some(function_name) = coerce_non_empty_scalar_to_string(
                            tool_obj.get("function").and_then(|value| value.get("name")),
                        ) else {
                            continue;
                        };
                        let function_name =
                            shorten_openai_tool_name_with_map(&function_name, &tool_name_map);
                        let arguments = tool_obj
                            .get("function")
                            .and_then(|value| value.get("arguments"))
                            .map(|value| {
                                if let Some(text) = value.as_str() {
                                    text.to_string()
                                } else {
                                    serde_json::to_string(value)
                                        .unwrap_or_else(|_| "{}".to_string())
                                }
                            })
                            .unwrap_or_else(|| "{}".to_string());
                        input_items.push(json!({
                            "type": "function_call",
                            "call_id": call_id,
                            "name": function_name,
                            "arguments": arguments
                        }));
                        seen_tool_call_ids.insert(call_id.to_string());
                        emitted_tool_call = true;
                    }
                }
                if !emitted_content && !emitted_tool_call {
                    continue;
                }
            },
            "tool" => {
                let call_id = message_obj
                    .get("tool_call_id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| bad_request("tool role message missing tool_call_id"))?;
                if !seen_tool_call_ids.contains(call_id) {
                    continue;
                }
                let output =
                    convert_tool_message_content_to_responses_output(message_obj.get("content"))
                        .map_err(|err| bad_request_with_detail("Invalid tool content", err))?;
                input_items.push(json!({
                    "type": "function_call_output",
                    "call_id": call_id,
                    "output": output
                }));
            },
            _ => {},
        }
    }

    let mut out = Map::new();
    if let Some(model) = obj.get("model") {
        out.insert("model".to_string(), model.clone());
    }
    out.insert("input".to_string(), Value::Array(input_items));

    let stream = obj.get("stream").and_then(Value::as_bool).unwrap_or(false);
    out.insert("stream".to_string(), Value::Bool(stream));
    out.insert("store".to_string(), Value::Bool(false));

    let reasoning_effort = obj
        .get("reasoning_effort")
        .and_then(Value::as_str)
        .and_then(normalize_reasoning_effort)
        .or_else(|| {
            obj.get("reasoning")
                .and_then(|reasoning| reasoning.get("effort"))
                .and_then(Value::as_str)
                .and_then(normalize_reasoning_effort)
        })
        .unwrap_or("medium");
    out.insert(
        "reasoning".to_string(),
        json!({
            "effort": reasoning_effort
        }),
    );

    let parallel_tool_calls = obj
        .get("parallel_tool_calls")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    out.insert("parallel_tool_calls".to_string(), Value::Bool(parallel_tool_calls));
    out.insert(
        "include".to_string(),
        Value::Array(vec![Value::String("reasoning.encrypted_content".to_string())]),
    );

    if let Some(service_tier) = obj.get("service_tier") {
        out.insert("service_tier".to_string(), service_tier.clone());
    }

    if let Some(tools) = obj.get("tools").and_then(Value::as_array) {
        let mut mapped_tools = Vec::with_capacity(tools.len());
        for tool in tools {
            let Some(tool_obj) = tool.as_object() else {
                continue;
            };
            if !is_openai_chat_function_tool(tool_obj) {
                mapped_tools.push(tool.clone());
                continue;
            }
            if let Some(mapped) = map_openai_chat_function_tool(tool_obj, &tool_name_map) {
                mapped_tools.push(mapped);
            }
        }
        if !mapped_tools.is_empty() {
            out.insert("tools".to_string(), Value::Array(mapped_tools));
        }
    }

    if let Some(functions) = obj.get("functions").and_then(Value::as_array) {
        let mut mapped_functions = out
            .remove("tools")
            .and_then(|value| value.as_array().cloned())
            .unwrap_or_default();
        for function in functions {
            let Some(function_obj) = function.as_object() else {
                continue;
            };
            let mut wrapped = Map::new();
            wrapped.insert("type".to_string(), Value::String("function".to_string()));
            wrapped.insert("function".to_string(), Value::Object(function_obj.clone()));
            if let Some(mapped) = map_openai_chat_function_tool(&wrapped, &tool_name_map) {
                mapped_functions.push(mapped);
            }
        }
        if !mapped_functions.is_empty() {
            out.insert("tools".to_string(), Value::Array(mapped_functions));
        }
    }

    if let Some(dynamic_tools) = get_dynamic_tools_array(obj) {
        let mut mapped_dynamic_tools = out
            .remove("tools")
            .and_then(|value| value.as_array().cloned())
            .unwrap_or_default();
        for dynamic_tool in dynamic_tools {
            let Some(tool_obj) = dynamic_tool.as_object() else {
                continue;
            };
            let name = tool_obj
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|name| shorten_openai_tool_name_with_map(name, &tool_name_map));
            let Some(name) = name else {
                continue;
            };
            let description = tool_obj
                .get("description")
                .cloned()
                .unwrap_or_else(|| Value::String(String::new()));
            let parameters = tool_obj
                .get("input_schema")
                .or_else(|| tool_obj.get("inputSchema"))
                .or_else(|| tool_obj.get("parameters"))
                .cloned()
                .map(normalize_tool_parameters_schema)
                .unwrap_or_else(|| json!({ "type": "object", "properties": {} }));
            let mut mapped = Map::new();
            mapped.insert("type".to_string(), Value::String("function".to_string()));
            mapped.insert("name".to_string(), Value::String(name));
            mapped.insert("description".to_string(), description);
            mapped.insert("parameters".to_string(), parameters);
            if let Some(strict) = tool_obj.get("strict") {
                mapped.insert("strict".to_string(), strict.clone());
            }
            mapped_dynamic_tools.push(Value::Object(mapped));
        }
        if !mapped_dynamic_tools.is_empty() {
            out.insert("tools".to_string(), Value::Array(mapped_dynamic_tools));
        }
    }

    if let Some(tool_choice) = obj.get("tool_choice") {
        if let Some(tool_choice_str) = tool_choice.as_str() {
            out.insert("tool_choice".to_string(), Value::String(tool_choice_str.to_string()));
        } else if let Some(tool_choice_obj) = tool_choice.as_object() {
            let tool_type = tool_choice_obj
                .get("type")
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or_default();
            if tool_type == "function" {
                let function = tool_choice_obj.get("function").and_then(Value::as_object);
                if let Some(name) = coerce_non_empty_scalar_to_string(openai_chat_tool_name_value(
                    tool_choice_obj,
                    function,
                )) {
                    out.insert(
                        "tool_choice".to_string(),
                        json!({
                            "type": "function",
                            "name": shorten_openai_tool_name_with_map(&name, &tool_name_map),
                        }),
                    );
                }
            } else {
                out.insert("tool_choice".to_string(), tool_choice.clone());
            }
        }
    }

    if !out.contains_key("tool_choice") {
        if let Some(function_call) = obj.get("function_call") {
            if let Some(function_call_str) = function_call.as_str() {
                out.insert("tool_choice".to_string(), Value::String(function_call_str.to_string()));
            } else if let Some(function_call_obj) = function_call.as_object() {
                if let Some(name) = coerce_non_empty_scalar_to_string(
                    legacy_openai_function_name_value(function_call_obj),
                ) {
                    out.insert(
                        "tool_choice".to_string(),
                        json!({
                            "type": "function",
                            "name": shorten_openai_tool_name_with_map(&name, &tool_name_map),
                        }),
                    );
                }
            }
        }
    }

    let mut text = obj
        .get("text")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(verbosity) = obj
        .get("verbosity")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        text.insert("verbosity".to_string(), Value::String(verbosity.to_string()));
    }
    if let Some(format) = obj.get("response_format").cloned() {
        text.insert("format".to_string(), format);
    }
    if !text.is_empty() {
        out.insert("text".to_string(), Value::Object(text));
    }

    Ok((out, tool_name_restore_map))
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use serde_json::json;

    use super::adapt_openai_chat_completions_request;

    #[test]
    fn adapt_openai_chat_completions_request_rejects_message_without_role() {
        let obj = json!({
            "model": "gpt-5.3-codex",
            "messages": [
                {
                    "content": "hello"
                }
            ]
        });

        let err = adapt_openai_chat_completions_request(
            obj.as_object().expect("sample request should be an object"),
        )
        .expect_err("request without message role should be rejected");
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        assert!(err.message.contains("role"));
    }

    #[test]
    fn adapt_openai_chat_completions_request_drops_user_message_without_supported_content() {
        let obj = json!({
            "model": "gpt-5.3-codex",
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "image_url"
                        }
                    ]
                }
            ]
        });

        let (adapted, _) = adapt_openai_chat_completions_request(
            obj.as_object().expect("sample request should be an object"),
        )
        .expect("unsupported user content should be dropped");
        assert_eq!(adapted["input"], json!([]));
    }
}
