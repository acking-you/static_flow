//! Anthropic-style messages adaptation on top of the Codex responses wire API.

use std::collections::{BTreeMap, BTreeSet};

use axum::body::Bytes;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use eventsource_stream::Event as SseEvent;
use serde_json::{json, Map, Value};

use crate::{
    error::{bad_request, bad_request_with_detail, CodexGatewayError, CodexGatewayResult},
    request::normalize_tool_parameters_schema,
    types::OpenAiChatAdaptedRequest,
    MAX_OPENAI_TOOL_NAME_LEN,
};

/// Stream-scoped state for converting responses SSE into Anthropic SSE.
#[derive(Debug, Clone, Default)]
pub struct AnthropicStreamMetadata {
    message_started: bool,
    text_block_index: Option<usize>,
    text_block_started: bool,
    next_content_index: usize,
    tool_blocks: BTreeMap<String, AnthropicToolBlockState>,
}

#[derive(Debug, Clone)]
struct AnthropicToolBlockState {
    index: usize,
    delta_seen: bool,
    start_had_payload: bool,
    stopped: bool,
}

impl AnthropicStreamMetadata {
    fn ensure_text_block_index(&mut self) -> usize {
        if let Some(index) = self.text_block_index {
            return index;
        }
        let index = self.next_content_index;
        self.text_block_index = Some(index);
        self.next_content_index += 1;
        index
    }

    fn ensure_tool_block_index(&mut self, lookup_key: &str) -> usize {
        if let Some(state) = self.tool_blocks.get(lookup_key) {
            return state.index;
        }
        let index = self.next_content_index;
        self.next_content_index += 1;
        self.tool_blocks
            .insert(lookup_key.to_string(), AnthropicToolBlockState {
                index,
                delta_seen: false,
                start_had_payload: false,
                stopped: false,
            });
        index
    }

    fn tool_block_state(&self, lookup_key: &str) -> Option<&AnthropicToolBlockState> {
        self.tool_blocks.get(lookup_key)
    }

    fn mark_tool_block_started(&mut self, lookup_key: &str, had_payload: bool) -> usize {
        let index = self.ensure_tool_block_index(lookup_key);
        let state = self
            .tool_blocks
            .get_mut(lookup_key)
            .expect("tool block state exists");
        state.start_had_payload = state.start_had_payload || had_payload;
        index
    }

    fn mark_tool_block_delta_seen(&mut self, lookup_key: &str) -> usize {
        let index = self.ensure_tool_block_index(lookup_key);
        let state = self
            .tool_blocks
            .get_mut(lookup_key)
            .expect("tool block state exists");
        state.delta_seen = true;
        index
    }

    fn mark_tool_block_stopped(&mut self, lookup_key: &str) -> Option<usize> {
        let state = self.tool_blocks.get_mut(lookup_key)?;
        if state.stopped {
            return None;
        }
        state.stopped = true;
        Some(state.index)
    }
}

fn anthropic_messages_bad_request(index: usize, message: impl AsRef<str>) -> CodexGatewayError {
    bad_request(&format!("messages message {index}: {}", message.as_ref()))
}

fn normalize_reasoning_effort(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "low" => Some("low"),
        "medium" => Some("medium"),
        "high" => Some("high"),
        "max" => Some("xhigh"),
        "xhigh" | "extra_high" => Some("xhigh"),
        _ => None,
    }
}

fn shorten_tool_name_candidate(name: &str) -> String {
    if name.len() <= MAX_OPENAI_TOOL_NAME_LEN {
        return name.to_string();
    }
    if name.starts_with("mcp__") {
        if let Some(idx) = name.rfind("__") {
            if idx > 0 {
                let mut candidate = format!("mcp__{}", &name[idx + 2..]);
                if candidate.len() > MAX_OPENAI_TOOL_NAME_LEN {
                    candidate.truncate(MAX_OPENAI_TOOL_NAME_LEN);
                }
                return candidate;
            }
        }
    }
    name.chars().take(MAX_OPENAI_TOOL_NAME_LEN).collect()
}

fn shorten_tool_name_with_map(name: &str, tool_name_map: &BTreeMap<String, String>) -> String {
    tool_name_map
        .get(name)
        .cloned()
        .unwrap_or_else(|| shorten_tool_name_candidate(name))
}

/// Restore one shortened tool name back to the client-visible name.
pub fn restore_tool_name(
    name: &str,
    tool_name_restore_map: Option<&BTreeMap<String, String>>,
) -> String {
    tool_name_restore_map
        .and_then(|map| map.get(name))
        .cloned()
        .unwrap_or_else(|| name.to_string())
}

fn build_tool_name_restore_map(
    tool_name_map: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut restore_map = BTreeMap::new();
    for (original, shortened) in tool_name_map {
        if original != shortened {
            restore_map.insert(shortened.clone(), original.clone());
        }
    }
    restore_map
}

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

fn flush_user_input_parts(input_items: &mut Vec<Value>, pending_parts: &mut Vec<Value>) {
    if pending_parts.is_empty() {
        return;
    }
    input_items.push(json!({
        "type": "message",
        "role": "user",
        "content": pending_parts.clone(),
    }));
    pending_parts.clear();
}

fn extract_text_content(value: &Value) -> Result<String, String> {
    match value {
        Value::String(text) => Ok(text.to_string()),
        Value::Array(items) => {
            let mut out = String::new();
            for item in items {
                let Some(item_obj) = item.as_object() else {
                    return Err("content array items must be objects".to_string());
                };
                let item_type = item_obj
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if item_type != "text" {
                    return Err(format!("unsupported content block `{item_type}`"));
                }
                let text = item_obj
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "text block is missing text".to_string())?;
                out.push_str(text);
            }
            Ok(out)
        },
        Value::Object(obj) => {
            let item_type = obj.get("type").and_then(Value::as_str).unwrap_or_default();
            if item_type != "text" {
                return Err(format!("unsupported content block `{item_type}`"));
            }
            obj.get("text")
                .and_then(Value::as_str)
                .map(ToString::to_string)
                .ok_or_else(|| "text block is missing text".to_string())
        },
        Value::Null => Ok(String::new()),
        _ => Err("content must be a string, object, or array".to_string()),
    }
}

fn content_items(value: &Value) -> Vec<Value> {
    if let Some(items) = value.as_array() {
        return items.clone();
    }
    if value.is_object() {
        return vec![value.clone()];
    }
    Vec::new()
}

fn image_source_to_responses_item(source: &Map<String, Value>) -> Result<Value, String> {
    let source_type = source
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match source_type {
        "base64" => {
            let media_type = source
                .get("media_type")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "image source is missing media_type".to_string())?;
            let data = source
                .get("data")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "image source is missing data".to_string())?;
            Ok(json!({
                "type": "input_image",
                "image_url": format!("data:{media_type};base64,{data}"),
            }))
        },
        "url" => {
            let url = source
                .get("url")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "image source is missing url".to_string())?;
            Ok(json!({
                "type": "input_image",
                "image_url": url,
            }))
        },
        _ => Err(format!("unsupported image source type `{source_type}`")),
    }
}

fn canonical_document_media_type(media_type: &str) -> Option<&'static str> {
    match media_type.trim().to_ascii_lowercase().as_str() {
        "application/pdf" => Some("application/pdf"),
        "text/csv" => Some("text/csv"),
        "application/msword" => Some("application/msword"),
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => {
            Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document")
        },
        "application/vnd.ms-excel" => Some("application/vnd.ms-excel"),
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => {
            Some("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet")
        },
        "text/html" => Some("text/html"),
        "text/plain" => Some("text/plain"),
        "text/markdown" | "text/md" | "text/x-markdown" => Some("text/markdown"),
        _ => None,
    }
}

fn document_media_type_supports_text_source(media_type: &str) -> bool {
    matches!(media_type, "text/plain" | "text/markdown" | "text/html" | "text/csv")
}

fn normalize_document_name(raw_name: Option<&str>, media_type: &str) -> String {
    let fallback = match media_type {
        "application/pdf" => "document.pdf",
        "text/csv" => "document.csv",
        "application/msword" => "document.doc",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => {
            "document.docx"
        },
        "application/vnd.ms-excel" => "document.xls",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => "document.xlsx",
        "text/html" => "document.html",
        "text/markdown" => "document.md",
        _ => "document.txt",
    };
    raw_name
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback)
        .to_string()
}

fn document_source_to_responses_item(
    item_obj: &Map<String, Value>,
    source: &Map<String, Value>,
) -> Result<Value, String> {
    let source_type = source
        .get("type")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "document source is missing type".to_string())?;
    let media_type = source
        .get("media_type")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "document source is missing media_type".to_string())?;
    let media_type = canonical_document_media_type(media_type)
        .ok_or_else(|| format!("unsupported document media_type `{media_type}`"))?;
    let data = source
        .get("data")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "document source is missing data".to_string())?;
    let file_data = match source_type {
        "base64" => data.trim().to_string(),
        "text" => {
            if !document_media_type_supports_text_source(media_type) {
                return Err("document source.type=`text` is only supported for plain text, \
                            markdown, html, or csv"
                    .to_string());
            }
            STANDARD.encode(data.replace("\r\n", "\n").replace('\r', "\n").as_bytes())
        },
        other => return Err(format!("unsupported document source type `{other}`")),
    };
    let filename =
        normalize_document_name(item_obj.get("name").and_then(Value::as_str), media_type);
    Ok(json!({
        "type": "input_file",
        "filename": filename,
        "file_data": format!("data:{media_type};base64,{file_data}"),
    }))
}

fn map_user_content_item_to_responses_item(item: &Value) -> Result<Option<Value>, String> {
    let Some(item_obj) = item.as_object() else {
        return Err("user content items must be objects".to_string());
    };
    let item_type = item_obj
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match item_type {
        "text" => Ok(item_obj
            .get("text")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|text| {
                json!({
                    "type": "input_text",
                    "text": text,
                })
            })),
        "image" => {
            let source = item_obj
                .get("source")
                .and_then(Value::as_object)
                .ok_or_else(|| "image block is missing source".to_string())?;
            image_source_to_responses_item(source).map(Some)
        },
        "document" => {
            let source = item_obj
                .get("source")
                .and_then(Value::as_object)
                .ok_or_else(|| "document block is missing source".to_string())?;
            document_source_to_responses_item(item_obj, source).map(Some)
        },
        other => Err(format!("unsupported user content block `{other}`")),
    }
}

fn convert_tool_result_content_to_responses_output(value: Option<&Value>) -> Result<Value, String> {
    let Some(value) = value else {
        return Ok(Value::String(String::new()));
    };
    if value.is_null() {
        return Ok(Value::String(String::new()));
    }
    if let Some(text) = value.as_str() {
        return Ok(Value::String(text.to_string()));
    }
    let items = content_items(value);
    if items.is_empty() {
        return serde_json::to_string(value)
            .map(Value::String)
            .map_err(|err| format!("serialize tool result content failed: {err}"));
    }
    let mut mapped = Vec::new();
    for item in items {
        let Some(item_obj) = item.as_object() else {
            return Err("tool_result content items must be objects".to_string());
        };
        let item_type = item_obj
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match item_type {
            "text" => {
                if let Some(text) = item_obj
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    mapped.push(json!({
                        "type": "input_text",
                        "text": text,
                    }));
                }
            },
            "image" => {
                let source = item_obj
                    .get("source")
                    .and_then(Value::as_object)
                    .ok_or_else(|| "tool_result image block is missing source".to_string())?;
                mapped.push(image_source_to_responses_item(source)?);
            },
            other => return Err(format!("unsupported tool_result content block `{other}`")),
        }
    }
    if mapped.is_empty() {
        Ok(Value::String(String::new()))
    } else {
        Ok(Value::Array(mapped))
    }
}

fn append_system_message(
    input_items: &mut Vec<Value>,
    system: Option<&Value>,
) -> CodexGatewayResult<()> {
    let Some(system) = system else {
        return Ok(());
    };
    let text = extract_text_content(system)
        .map_err(|err| bad_request_with_detail("Invalid messages system prompt", err))?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    input_items.push(json!({
        "type": "message",
        "role": "developer",
        "content": [{
            "type": "input_text",
            "text": trimmed,
        }]
    }));
    Ok(())
}

fn append_user_content_to_responses_input(
    input_items: &mut Vec<Value>,
    content: &Value,
) -> Result<(), String> {
    if let Some(text) = content.as_str() {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Ok(());
        }
        input_items.push(json!({
            "type": "message",
            "role": "user",
            "content": [{
                "type": "input_text",
                "text": trimmed,
            }]
        }));
        return Ok(());
    }

    let items = content_items(content);
    let mut pending_parts = Vec::new();
    for item in items {
        let Some(item_obj) = item.as_object() else {
            return Err("user content items must be objects".to_string());
        };
        let item_type = item_obj
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match item_type {
            "tool_result" => {
                flush_user_input_parts(input_items, &mut pending_parts);
                let call_id = item_obj
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| "tool_result block is missing tool_use_id".to_string())?;
                let output =
                    convert_tool_result_content_to_responses_output(item_obj.get("content"))?;
                input_items.push(json!({
                    "type": "function_call_output",
                    "call_id": call_id,
                    "output": output,
                }));
            },
            _ => {
                if let Some(mapped) = map_user_content_item_to_responses_item(&item)? {
                    pending_parts.push(mapped);
                }
            },
        }
    }
    flush_user_input_parts(input_items, &mut pending_parts);
    Ok(())
}

fn append_assistant_content_to_responses_input(
    input_items: &mut Vec<Value>,
    content: &Value,
    tool_name_map: &BTreeMap<String, String>,
) -> Result<(), String> {
    if let Some(text) = content.as_str() {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Ok(());
        }
        input_items.push(json!({
            "type": "message",
            "role": "assistant",
            "content": [{
                "type": "output_text",
                "text": trimmed,
            }]
        }));
        return Ok(());
    }

    let items = content_items(content);
    let mut pending_parts = Vec::new();
    for item in items {
        let Some(item_obj) = item.as_object() else {
            return Err("assistant content items must be objects".to_string());
        };
        let item_type = item_obj
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match item_type {
            "text" => {
                if let Some(text) = item_obj
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    pending_parts.push(json!({
                        "type": "output_text",
                        "text": text,
                    }));
                }
            },
            "tool_use" => {
                flush_assistant_output_parts(input_items, &mut pending_parts);
                let call_id = item_obj
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| "tool_use block is missing id".to_string())?;
                let name = item_obj
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| "tool_use block is missing name".to_string())?;
                let name = shorten_tool_name_with_map(name, tool_name_map);
                let arguments = item_obj.get("input").cloned().unwrap_or_else(|| json!({}));
                input_items.push(json!({
                    "type": "function_call",
                    "call_id": call_id,
                    "name": name,
                    "arguments": serde_json::to_string(&arguments)
                        .unwrap_or_else(|_| "{}".to_string()),
                }));
            },
            other => return Err(format!("unsupported assistant content block `{other}`")),
        }
    }
    flush_assistant_output_parts(input_items, &mut pending_parts);
    Ok(())
}

fn collect_tool_names(obj: &Map<String, Value>) -> Vec<String> {
    let mut names = Vec::new();

    if let Some(tools) = obj.get("tools").and_then(Value::as_array) {
        for tool in tools {
            let Some(name) = tool
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                continue;
            };
            names.push(name.to_string());
        }
    }

    if let Some(name) = obj
        .get("tool_choice")
        .and_then(Value::as_object)
        .and_then(|tool_choice| {
            if tool_choice.get("type").and_then(Value::as_str) != Some("tool") {
                return None;
            }
            tool_choice
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
    {
        names.push(name.to_string());
    }

    if let Some(messages) = obj.get("messages").and_then(Value::as_array) {
        for message in messages {
            let Some(message_obj) = message.as_object() else {
                continue;
            };
            let Some(content) = message_obj.get("content") else {
                continue;
            };
            for item in content_items(content) {
                let Some(name) = item
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                else {
                    continue;
                };
                if item.get("type").and_then(Value::as_str) == Some("tool_use") {
                    names.push(name.to_string());
                }
            }
        }
    }

    names
}

fn build_tool_name_map(obj: &Map<String, Value>) -> BTreeMap<String, String> {
    let mut unique_names = BTreeSet::new();
    for name in collect_tool_names(obj) {
        unique_names.insert(name);
    }

    let mut used = BTreeSet::new();
    let mut out = BTreeMap::new();
    for name in unique_names {
        let base = shorten_tool_name_candidate(name.as_str());
        let mut candidate = base.clone();
        let mut suffix = 1usize;
        while used.contains(&candidate) {
            let suffix_text = format!("_{suffix}");
            let mut truncated = base.clone();
            let limit = MAX_OPENAI_TOOL_NAME_LEN.saturating_sub(suffix_text.len());
            if truncated.len() > limit {
                truncated = truncated.chars().take(limit).collect();
            }
            candidate = format!("{truncated}{suffix_text}");
            suffix += 1;
        }
        used.insert(candidate.clone());
        out.insert(name, candidate);
    }
    out
}

/// Convert one Anthropic-compatible messages request into responses shape.
pub fn adapt_anthropic_messages_request(
    obj: &Map<String, Value>,
) -> CodexGatewayResult<OpenAiChatAdaptedRequest> {
    let source_messages = obj
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| bad_request("messages messages field is required"))?;
    let tool_name_map = build_tool_name_map(obj);
    let tool_name_restore_map = build_tool_name_restore_map(&tool_name_map);

    let mut input_items = Vec::<Value>::new();
    append_system_message(&mut input_items, obj.get("system"))?;

    for (index, message) in source_messages.iter().enumerate() {
        let Some(message_obj) = message.as_object() else {
            return Err(anthropic_messages_bad_request(index, "must be an object"));
        };
        let Some(role) = message_obj.get("role").and_then(Value::as_str) else {
            return Err(anthropic_messages_bad_request(index, "is missing role"));
        };
        let Some(content) = message_obj.get("content") else {
            return Err(anthropic_messages_bad_request(index, "is missing content"));
        };
        let prior_len = input_items.len();
        match role {
            "user" => {
                append_user_content_to_responses_input(&mut input_items, content)
                    .map_err(|err| bad_request_with_detail("Invalid messages user content", err))?;
            },
            "assistant" => {
                append_assistant_content_to_responses_input(
                    &mut input_items,
                    content,
                    &tool_name_map,
                )
                .map_err(|err| {
                    bad_request_with_detail("Invalid messages assistant content", err)
                })?;
            },
            _ => {
                return Err(anthropic_messages_bad_request(
                    index,
                    format!("has unsupported role `{role}`"),
                ));
            },
        }
        if input_items.len() == prior_len {
            continue;
        }
    }

    let mut out = Map::new();
    if let Some(model) = obj.get("model") {
        out.insert("model".to_string(), model.clone());
    }
    out.insert("input".to_string(), Value::Array(input_items));
    out.insert(
        "stream".to_string(),
        Value::Bool(obj.get("stream").and_then(Value::as_bool).unwrap_or(false)),
    );
    out.insert("store".to_string(), Value::Bool(false));

    let reasoning_effort = obj
        .get("output_config")
        .and_then(Value::as_object)
        .and_then(|output| output.get("effort"))
        .and_then(Value::as_str)
        .and_then(normalize_reasoning_effort)
        .or_else(|| {
            obj.get("thinking")
                .and_then(Value::as_object)
                .and_then(|thinking| thinking.get("effort"))
                .and_then(Value::as_str)
                .and_then(normalize_reasoning_effort)
        })
        .or_else(|| {
            obj.get("thinking")
                .and_then(Value::as_object)
                .and_then(|thinking| thinking.get("type"))
                .and_then(Value::as_str)
                .and_then(|thinking_type| {
                    if thinking_type == "enabled" {
                        Some("xhigh")
                    } else if thinking_type == "adaptive" {
                        Some("high")
                    } else {
                        None
                    }
                })
        })
        .unwrap_or("medium");
    out.insert(
        "reasoning".to_string(),
        json!({
            "effort": reasoning_effort
        }),
    );

    if let Some(service_tier) = obj.get("service_tier") {
        out.insert("service_tier".to_string(), service_tier.clone());
    }

    if let Some(tools) = obj.get("tools").and_then(Value::as_array) {
        let mapped_tools = tools
            .iter()
            .filter_map(|tool| {
                let tool_obj = tool.as_object()?;
                let name = tool_obj
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(|name| shorten_tool_name_with_map(name, &tool_name_map))?;
                let mut mapped = Map::new();
                mapped.insert("type".to_string(), Value::String("function".to_string()));
                mapped.insert("name".to_string(), Value::String(name));
                if let Some(description) = tool_obj.get("description") {
                    mapped.insert("description".to_string(), description.clone());
                }
                if let Some(parameters) = tool_obj
                    .get("input_schema")
                    .or_else(|| tool_obj.get("inputSchema"))
                {
                    mapped.insert(
                        "parameters".to_string(),
                        normalize_tool_parameters_schema(parameters.clone()),
                    );
                }
                Some(Value::Object(mapped))
            })
            .collect::<Vec<_>>();
        if !mapped_tools.is_empty() {
            out.insert("tools".to_string(), Value::Array(mapped_tools));
        }
    }

    if let Some(tool_choice) = obj.get("tool_choice").and_then(Value::as_object) {
        let mapped_tool_choice = match tool_choice.get("type").and_then(Value::as_str) {
            Some("auto") => Some(Value::String("auto".to_string())),
            Some("any") => Some(Value::String("required".to_string())),
            Some("none") => Some(Value::String("none".to_string())),
            Some("tool") => tool_choice
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|name| {
                    json!({
                        "type": "function",
                        "name": shorten_tool_name_with_map(name, &tool_name_map),
                    })
                }),
            _ => None,
        };
        if let Some(tool_choice) = mapped_tool_choice {
            out.insert("tool_choice".to_string(), tool_choice);
        }
    }

    let mut text = Map::new();
    if let Some(format) = obj
        .get("output_config")
        .and_then(Value::as_object)
        .and_then(|output| output.get("format"))
    {
        text.insert("format".to_string(), format.clone());
    }
    if !text.is_empty() {
        out.insert("text".to_string(), Value::Object(text));
    }

    Ok((out, tool_name_restore_map))
}

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

fn parse_tool_arguments_value(value: Option<&Value>) -> Value {
    let Some(value) = value else {
        return json!({});
    };
    if let Some(text) = value.as_str() {
        return serde_json::from_str::<Value>(text)
            .unwrap_or_else(|_| Value::String(text.to_string()));
    }
    value.clone()
}

fn parse_responses_tool_input_value(item_obj: &Map<String, Value>, item_type: &str) -> Value {
    match item_type {
        "custom_tool_call" => parse_tool_arguments_value(item_obj.get("input")),
        _ => parse_tool_arguments_value(item_obj.get("arguments")),
    }
}

fn tool_input_delta_text(value: &Value) -> &str {
    value
        .get("delta")
        .or_else(|| value.get("input"))
        .or_else(|| value.get("arguments"))
        .and_then(Value::as_str)
        .unwrap_or("")
}

fn anthropic_client_tool_block(
    item_obj: &Map<String, Value>,
    name: String,
    input: Value,
) -> Option<Value> {
    let call_id = item_obj
        .get("call_id")
        .or_else(|| item_obj.get("id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    Some(json!({
        "type": "tool_use",
        "id": call_id,
        "name": name,
        "input": input,
    }))
}

fn anthropic_server_tool_block(
    item_obj: &Map<String, Value>,
    name: &str,
    input: Value,
) -> Option<Value> {
    let tool_id = item_obj
        .get("id")
        .or_else(|| item_obj.get("call_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    Some(json!({
        "type": "server_tool_use",
        "id": tool_id,
        "name": name,
        "input": input,
    }))
}

fn anthropic_tool_block_from_responses_item(
    item_obj: &Map<String, Value>,
    item_type: &str,
    tool_name_restore_map: Option<&BTreeMap<String, String>>,
) -> Option<Value> {
    match item_type {
        "function_call" | "custom_tool_call" => {
            let name = item_obj
                .get("name")
                .and_then(Value::as_str)
                .map(|name| restore_tool_name(name, tool_name_restore_map))
                .unwrap_or_else(|| "tool".to_string());
            anthropic_client_tool_block(
                item_obj,
                name,
                parse_responses_tool_input_value(item_obj, item_type),
            )
        },
        "local_shell_call" => anthropic_client_tool_block(
            item_obj,
            "local_shell".to_string(),
            item_obj.get("action").cloned().unwrap_or_else(|| json!({})),
        ),
        "web_search_call" => anthropic_server_tool_block(
            item_obj,
            "web_search",
            item_obj.get("action").cloned().unwrap_or_else(|| json!({})),
        ),
        "image_generation_call" => {
            let mut input = Map::new();
            if let Some(status) = item_obj.get("status") {
                input.insert("status".to_string(), status.clone());
            }
            if let Some(revised_prompt) = item_obj.get("revised_prompt") {
                input.insert("revised_prompt".to_string(), revised_prompt.clone());
            }
            if let Some(result) = item_obj.get("result") {
                input.insert("result".to_string(), result.clone());
            }
            anthropic_server_tool_block(item_obj, "image_generation", Value::Object(input))
        },
        _ => None,
    }
}

fn anthropic_block_input_string(block: &Value) -> String {
    let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
    if let Some(text) = input.as_str() {
        text.to_string()
    } else {
        serde_json::to_string(&input).unwrap_or_else(|_| String::new())
    }
}

fn anthropic_usage_from_response(source: &Value) -> Value {
    let usage = source.get("usage").cloned().unwrap_or(Value::Null);
    let input_total = usage
        .get("input_tokens")
        .and_then(Value::as_u64)
        .or_else(|| usage.get("prompt_tokens").and_then(Value::as_u64))
        .unwrap_or(0);
    let cached_tokens = usage
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
    json!({
        "input_tokens": input_total.saturating_sub(cached_tokens),
        "cache_read_input_tokens": cached_tokens,
        "output_tokens": output_tokens,
    })
}

fn stop_reason(source: &Value) -> &'static str {
    if source
        .get("output")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items.iter().any(|item| {
                item.get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|kind| {
                        matches!(kind, "function_call" | "custom_tool_call" | "local_shell_call")
                    })
            })
        })
    {
        "tool_use"
    } else {
        "end_turn"
    }
}

/// Adapt a completed responses payload into an Anthropic messages payload.
pub fn map_response_to_anthropic_message(
    value: &Value,
    tool_name_restore_map: Option<&BTreeMap<String, String>>,
) -> Value {
    let source = value.get("response").unwrap_or(value);
    let mut content = Vec::<Value>::new();

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
                    if let Some(parts) = item_obj.get("content").and_then(Value::as_array) {
                        for part in parts {
                            let Some(part_obj) = part.as_object() else {
                                continue;
                            };
                            let part_type = part_obj
                                .get("type")
                                .and_then(Value::as_str)
                                .unwrap_or_default();
                            if !matches!(part_type, "text" | "input_text" | "output_text") {
                                continue;
                            }
                            let Some(text) = part_obj.get("text").and_then(Value::as_str) else {
                                continue;
                            };
                            content.push(json!({
                                "type": "text",
                                "text": text,
                            }));
                        }
                    }
                },
                "function_call"
                | "custom_tool_call"
                | "local_shell_call"
                | "web_search_call"
                | "image_generation_call" => {
                    if let Some(block) = anthropic_tool_block_from_responses_item(
                        item_obj,
                        item_type,
                        tool_name_restore_map,
                    ) {
                        content.push(block);
                    }
                },
                _ => {},
            }
        }
    }

    json!({
        "id": source.get("id").cloned().unwrap_or_else(|| Value::String(String::new())),
        "type": "message",
        "role": "assistant",
        "model": source.get("model").cloned().unwrap_or_else(|| Value::String(String::new())),
        "content": content,
        "stop_reason": stop_reason(source),
        "stop_sequence": Value::Null,
        "usage": anthropic_usage_from_response(source),
    })
}

/// Convert one non-streaming responses JSON body into Anthropic messages JSON.
pub fn convert_json_response_to_anthropic_message(
    bytes: &[u8],
    tool_name_restore_map: Option<&BTreeMap<String, String>>,
    model_from: Option<&str>,
    model_to: Option<&str>,
) -> Result<Vec<u8>, String> {
    let value = serde_json::from_slice::<Value>(bytes)
        .map_err(|_| "invalid upstream json payload".to_string())?;
    let value = maybe_apply_model_alias(value, model_from, model_to);
    serde_json::to_vec(&map_response_to_anthropic_message(&value, tool_name_restore_map))
        .map_err(|err| format!("serialize anthropic message json failed: {err}"))
}

fn encode_named_json_sse_chunk(event_name: &str, value: &Value) -> Bytes {
    let body = serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string());
    Bytes::from(format!("event: {event_name}\ndata: {body}\n\n"))
}

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
    String::new()
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

fn push_message_start(
    chunks: &mut Vec<Bytes>,
    value: &Value,
    metadata: &mut AnthropicStreamMetadata,
) {
    if metadata.message_started {
        return;
    }
    metadata.message_started = true;
    chunks.push(encode_named_json_sse_chunk(
        "message_start",
        &json!({
            "type": "message_start",
            "message": {
                "id": stream_event_response_id(value),
                "type": "message",
                "role": "assistant",
                "model": stream_event_model(value),
                "content": [],
                "stop_reason": Value::Null,
                "stop_sequence": Value::Null,
                "usage": {
                    "input_tokens": 0,
                    "cache_read_input_tokens": 0,
                    "output_tokens": 0,
                }
            }
        }),
    ));
}

fn push_text_block_start(chunks: &mut Vec<Bytes>, metadata: &mut AnthropicStreamMetadata) -> usize {
    let index = metadata.ensure_text_block_index();
    if !metadata.text_block_started {
        metadata.text_block_started = true;
        chunks.push(encode_named_json_sse_chunk(
            "content_block_start",
            &json!({
                "type": "content_block_start",
                "index": index,
                "content_block": {
                    "type": "text",
                    "text": "",
                }
            }),
        ));
    }
    index
}

fn emit_full_response_as_anthropic_stream(
    response: &Value,
    tool_name_restore_map: Option<&BTreeMap<String, String>>,
    chunks: &mut Vec<Bytes>,
    metadata: &mut AnthropicStreamMetadata,
) {
    push_message_start(chunks, response, metadata);
    let message = map_response_to_anthropic_message(response, tool_name_restore_map);
    if let Some(content) = message.get("content").and_then(Value::as_array) {
        for item in content {
            match item.get("type").and_then(Value::as_str) {
                Some("text") => {
                    let index = push_text_block_start(chunks, metadata);
                    if let Some(text) = item.get("text").and_then(Value::as_str) {
                        if !text.is_empty() {
                            chunks.push(encode_named_json_sse_chunk(
                                "content_block_delta",
                                &json!({
                                    "type": "content_block_delta",
                                    "index": index,
                                    "delta": {
                                        "type": "text_delta",
                                        "text": text,
                                    }
                                }),
                            ));
                        }
                    }
                    chunks.push(encode_named_json_sse_chunk(
                        "content_block_stop",
                        &json!({
                            "type": "content_block_stop",
                            "index": index,
                        }),
                    ));
                },
                Some("tool_use") | Some("server_tool_use") => {
                    let lookup_key = item
                        .get("id")
                        .and_then(Value::as_str)
                        .map(|id| format!("call:{id}"));
                    let Some(lookup_key) = lookup_key else {
                        continue;
                    };
                    let index = metadata.mark_tool_block_started(&lookup_key, true);
                    chunks.push(encode_named_json_sse_chunk(
                        "content_block_start",
                        &json!({
                            "type": "content_block_start",
                            "index": index,
                            "content_block": item,
                        }),
                    ));
                    chunks.push(encode_named_json_sse_chunk(
                        "content_block_stop",
                        &json!({
                            "type": "content_block_stop",
                            "index": index,
                        }),
                    ));
                },
                _ => {},
            }
        }
    }
    chunks.push(encode_named_json_sse_chunk(
        "message_delta",
        &json!({
            "type": "message_delta",
            "delta": {
                "stop_reason": stop_reason(response.get("response").unwrap_or(response)),
                "stop_sequence": Value::Null,
            },
            "usage": anthropic_usage_from_response(response.get("response").unwrap_or(response)),
        }),
    ));
    chunks.push(encode_named_json_sse_chunk("message_stop", &json!({"type":"message_stop"})));
}

/// Convert one responses SSE event into zero or more Anthropic SSE chunks.
pub fn convert_response_event_to_anthropic_sse_chunks(
    event: &SseEvent,
    tool_name_restore_map: Option<&BTreeMap<String, String>>,
    metadata: &mut AnthropicStreamMetadata,
    model_from: Option<&str>,
    model_to: Option<&str>,
) -> Vec<Bytes> {
    let payload = event.data.trim();
    if payload.is_empty() || payload == "[DONE]" {
        return Vec::new();
    }
    let Ok(value) = serde_json::from_str::<Value>(payload) else {
        return Vec::new();
    };
    let value = maybe_apply_model_alias(value, model_from, model_to);
    let chunk_type = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut chunks = Vec::new();

    match chunk_type {
        "response.output_text.delta" => {
            let text = extract_stream_event_text(&value);
            if text.is_empty() {
                return Vec::new();
            }
            push_message_start(&mut chunks, &value, metadata);
            let index = push_text_block_start(&mut chunks, metadata);
            chunks.push(encode_named_json_sse_chunk(
                "content_block_delta",
                &json!({
                    "type": "content_block_delta",
                    "index": index,
                    "delta": {
                        "type": "text_delta",
                        "text": text,
                    }
                }),
            ));
        },
        "response.output_item.added" | "response.output_item.done" => {
            let Some(item) = value.get("item").or_else(|| value.get("output_item")) else {
                return Vec::new();
            };
            let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
            if !matches!(
                item_type,
                "function_call"
                    | "custom_tool_call"
                    | "local_shell_call"
                    | "web_search_call"
                    | "image_generation_call"
            ) {
                return Vec::new();
            }
            let Some((lookup_key, _tool_id)) = stream_tool_call_identity_from_item(item) else {
                return Vec::new();
            };
            push_message_start(&mut chunks, &value, metadata);
            let Some(block) = item.as_object().and_then(|obj| {
                anthropic_tool_block_from_responses_item(obj, item_type, tool_name_restore_map)
            }) else {
                return Vec::new();
            };
            let payload = anthropic_block_input_string(&block);
            let has_payload = !payload.is_empty() && payload != "{}";
            let existing = metadata.tool_block_state(&lookup_key).cloned();
            let index = metadata.mark_tool_block_started(&lookup_key, has_payload);
            if existing.is_none() {
                chunks.push(encode_named_json_sse_chunk(
                    "content_block_start",
                    &json!({
                        "type": "content_block_start",
                        "index": index,
                        "content_block": json!({
                            "type": block.get("type").cloned().unwrap_or_else(|| json!("tool_use")),
                            "id": block.get("id").cloned().unwrap_or(Value::Null),
                            "name": block.get("name").cloned().unwrap_or_else(|| json!("tool")),
                            "input": {}
                        })
                    }),
                ));
            }
            if chunk_type == "response.output_item.added" && existing.is_none() && has_payload {
                chunks.push(encode_named_json_sse_chunk(
                    "content_block_delta",
                    &json!({
                        "type": "content_block_delta",
                        "index": index,
                        "delta": {
                            "type": "input_json_delta",
                            "partial_json": payload,
                        }
                    }),
                ));
            }
            if chunk_type == "response.output_item.done" {
                if item_type == "function_call" {
                    if let Some(stop_index) = metadata.mark_tool_block_stopped(&lookup_key) {
                        chunks.push(encode_named_json_sse_chunk(
                            "content_block_stop",
                            &json!({
                                "type": "content_block_stop",
                                "index": stop_index,
                            }),
                        ));
                    }
                    return chunks;
                }
                let delta_seen = existing.as_ref().is_some_and(|state| state.delta_seen);
                let start_had_payload = existing
                    .as_ref()
                    .is_some_and(|state| state.start_had_payload);
                if !delta_seen && !start_had_payload && has_payload {
                    chunks.push(encode_named_json_sse_chunk(
                        "content_block_delta",
                        &json!({
                            "type": "content_block_delta",
                            "index": index,
                            "delta": {
                                "type": "input_json_delta",
                                "partial_json": payload,
                            }
                        }),
                    ));
                }
                if let Some(stop_index) = metadata.mark_tool_block_stopped(&lookup_key) {
                    chunks.push(encode_named_json_sse_chunk(
                        "content_block_stop",
                        &json!({
                            "type": "content_block_stop",
                            "index": stop_index,
                        }),
                    ));
                }
            }
        },
        "response.function_call_arguments.delta" | "response.custom_tool_call_input.delta" => {
            let Some((lookup_key, _call_id)) = stream_tool_call_identity_from_event(&value) else {
                return Vec::new();
            };
            let partial_json = tool_input_delta_text(&value);
            if partial_json.is_empty() {
                return Vec::new();
            }
            if metadata.tool_block_state(&lookup_key).is_none() {
                return Vec::new();
            }
            push_message_start(&mut chunks, &value, metadata);
            let index = metadata.mark_tool_block_delta_seen(&lookup_key);
            chunks.push(encode_named_json_sse_chunk(
                "content_block_delta",
                &json!({
                    "type": "content_block_delta",
                    "index": index,
                    "delta": {
                        "type": "input_json_delta",
                        "partial_json": partial_json,
                    }
                }),
            ));
        },
        "response.function_call_arguments.done" => {
            let Some((lookup_key, _call_id)) = stream_tool_call_identity_from_event(&value) else {
                return Vec::new();
            };
            let partial_json = tool_input_delta_text(&value);
            let Some(existing) = metadata.tool_block_state(&lookup_key).cloned() else {
                return Vec::new();
            };
            if !partial_json.is_empty() && !existing.delta_seen {
                let index = metadata.mark_tool_block_delta_seen(&lookup_key);
                chunks.push(encode_named_json_sse_chunk(
                    "content_block_delta",
                    &json!({
                        "type": "content_block_delta",
                        "index": index,
                        "delta": {
                            "type": "input_json_delta",
                            "partial_json": partial_json,
                        }
                    }),
                ));
            }
            if let Some(stop_index) = metadata.mark_tool_block_stopped(&lookup_key) {
                chunks.push(encode_named_json_sse_chunk(
                    "content_block_stop",
                    &json!({
                        "type": "content_block_stop",
                        "index": stop_index,
                    }),
                ));
            }
        },
        "response.completed" | "response.done" => {
            let response = value.get("response").unwrap_or(&value);
            if !metadata.message_started {
                emit_full_response_as_anthropic_stream(
                    response,
                    tool_name_restore_map,
                    &mut chunks,
                    metadata,
                );
                return chunks;
            }
            if let Some(index) = metadata.text_block_index {
                chunks.push(encode_named_json_sse_chunk(
                    "content_block_stop",
                    &json!({
                        "type": "content_block_stop",
                        "index": index,
                    }),
                ));
                metadata.text_block_started = false;
            }
            let open_tool_indices = metadata
                .tool_blocks
                .iter()
                .filter_map(|(lookup_key, state)| {
                    (!state.stopped).then_some((lookup_key.clone(), state.index))
                })
                .collect::<Vec<_>>();
            for (lookup_key, index) in open_tool_indices {
                if metadata.mark_tool_block_stopped(&lookup_key).is_some() {
                    chunks.push(encode_named_json_sse_chunk(
                        "content_block_stop",
                        &json!({
                            "type": "content_block_stop",
                            "index": index,
                        }),
                    ));
                }
            }
            chunks.push(encode_named_json_sse_chunk(
                "message_delta",
                &json!({
                    "type": "message_delta",
                    "delta": {
                        "stop_reason": stop_reason(response),
                        "stop_sequence": Value::Null,
                    },
                    "usage": anthropic_usage_from_response(response),
                }),
            ));
            chunks
                .push(encode_named_json_sse_chunk("message_stop", &json!({"type":"message_stop"})));
        },
        _ => {},
    }

    chunks
}

#[cfg(test)]
mod tests {
    use eventsource_stream::Event as SseEvent;
    use serde_json::{json, Value};

    use super::{
        adapt_anthropic_messages_request, convert_response_event_to_anthropic_sse_chunks,
        map_response_to_anthropic_message, AnthropicStreamMetadata,
    };

    fn sse_event(value: Value) -> SseEvent {
        SseEvent {
            event: String::new(),
            data: serde_json::to_string(&value).expect("serialize test event"),
            id: String::new(),
            retry: None,
        }
    }

    fn parse_named_sse_json(chunks: &[axum::body::Bytes], event_name: &str) -> Vec<Value> {
        chunks
            .iter()
            .filter_map(|chunk| {
                let text = std::str::from_utf8(chunk).ok()?;
                let prefix = format!("event: {event_name}\n");
                if !text.starts_with(&prefix) {
                    return None;
                }
                let data = text.strip_prefix(&prefix)?.strip_prefix("data: ")?.trim();
                serde_json::from_str::<Value>(data).ok()
            })
            .collect()
    }

    #[test]
    fn streamed_custom_tool_call_delta_requires_start_event() {
        let mut metadata = AnthropicStreamMetadata::default();
        let delta = sse_event(json!({
            "type": "response.custom_tool_call_input.delta",
            "call_id": "callpatch1",
            "delta": "*** Begin Patch"
        }));

        let chunks =
            convert_response_event_to_anthropic_sse_chunks(&delta, None, &mut metadata, None, None);

        assert!(chunks.is_empty());
    }

    #[test]
    fn streamed_custom_tool_call_added_with_payload_emits_input_json_delta() {
        let mut metadata = AnthropicStreamMetadata::default();
        let added = sse_event(json!({
            "type": "response.output_item.added",
            "item": {
                "type": "custom_tool_call",
                "call_id": "callpatch1",
                "name": "apply_patch",
                "input": "*** Begin Patch"
            }
        }));

        let chunks =
            convert_response_event_to_anthropic_sse_chunks(&added, None, &mut metadata, None, None);

        let starts = parse_named_sse_json(&chunks, "content_block_start");
        let deltas = parse_named_sse_json(&chunks, "content_block_delta");
        assert_eq!(starts.len(), 1);
        assert_eq!(starts[0]["content_block"]["type"], json!("tool_use"));
        assert_eq!(starts[0]["content_block"]["id"], json!("callpatch1"));
        assert_eq!(starts[0]["content_block"]["name"], json!("apply_patch"));
        assert_eq!(starts[0]["content_block"]["input"], json!({}));
        assert_eq!(deltas.len(), 1);
        assert_eq!(deltas[0]["delta"]["type"], json!("input_json_delta"));
        assert_eq!(deltas[0]["delta"]["partial_json"], json!("*** Begin Patch"));
    }

    #[test]
    fn streamed_custom_tool_call_done_without_prior_start_emits_full_tool_block_once() {
        let mut metadata = AnthropicStreamMetadata::default();
        let done = sse_event(json!({
            "type": "response.output_item.done",
            "item": {
                "type": "custom_tool_call",
                "call_id": "callpatch1",
                "name": "apply_patch",
                "input": "*** Begin Patch"
            }
        }));

        let chunks =
            convert_response_event_to_anthropic_sse_chunks(&done, None, &mut metadata, None, None);

        let starts = parse_named_sse_json(&chunks, "content_block_start");
        let deltas = parse_named_sse_json(&chunks, "content_block_delta");
        let stops = parse_named_sse_json(&chunks, "content_block_stop");
        assert_eq!(starts.len(), 1);
        assert_eq!(deltas.len(), 1);
        assert_eq!(stops.len(), 1);
        assert_eq!(deltas[0]["delta"]["partial_json"], json!("*** Begin Patch"));
    }

    #[test]
    fn streamed_function_call_arguments_done_emits_delta_then_stop() {
        let mut metadata = AnthropicStreamMetadata::default();
        let added = sse_event(json!({
            "type": "response.output_item.added",
            "item": {
                "type": "function_call",
                "call_id": "callauto14",
                "name": "alpha",
                "arguments": "{}"
            }
        }));
        let done = sse_event(json!({
            "type": "response.function_call_arguments.done",
            "call_id": "callauto14",
            "arguments": "{\"k\":1}"
        }));

        let start_chunks =
            convert_response_event_to_anthropic_sse_chunks(&added, None, &mut metadata, None, None);
        let done_chunks =
            convert_response_event_to_anthropic_sse_chunks(&done, None, &mut metadata, None, None);

        let starts = parse_named_sse_json(&start_chunks, "content_block_start");
        let deltas = parse_named_sse_json(&done_chunks, "content_block_delta");
        let stops = parse_named_sse_json(&done_chunks, "content_block_stop");
        assert_eq!(starts.len(), 1);
        assert_eq!(deltas.len(), 1);
        assert_eq!(stops.len(), 1);
        assert_eq!(deltas[0]["delta"]["partial_json"], json!("{\"k\":1}"));
    }

    #[test]
    fn completed_anthropic_message_maps_client_and_server_side_tools() {
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

        let mapped = map_response_to_anthropic_message(&value, None);
        assert_eq!(mapped["content"][0]["type"], json!("tool_use"));
        assert_eq!(mapped["content"][0]["name"], json!("local_shell"));
        assert_eq!(mapped["content"][1]["type"], json!("server_tool_use"));
        assert_eq!(mapped["content"][1]["name"], json!("web_search"));
        assert_eq!(mapped["content"][2]["type"], json!("server_tool_use"));
        assert_eq!(mapped["content"][2]["name"], json!("image_generation"));
        assert_eq!(mapped["stop_reason"], json!("tool_use"));
    }

    #[test]
    fn streamed_web_search_call_maps_to_server_tool_use_block() {
        let mut metadata = AnthropicStreamMetadata::default();
        let done = sse_event(json!({
            "type": "response.output_item.done",
            "item": {
                "type": "web_search_call",
                "id": "ws_1",
                "status": "completed",
                "action": {"type":"search","query":"weather seattle"}
            }
        }));

        let chunks =
            convert_response_event_to_anthropic_sse_chunks(&done, None, &mut metadata, None, None);

        let starts = parse_named_sse_json(&chunks, "content_block_start");
        let deltas = parse_named_sse_json(&chunks, "content_block_delta");
        assert_eq!(starts.len(), 1);
        assert_eq!(starts[0]["content_block"]["type"], json!("server_tool_use"));
        assert_eq!(starts[0]["content_block"]["name"], json!("web_search"));
        assert_eq!(deltas.len(), 1);
        assert_eq!(deltas[0]["delta"]["type"], json!("input_json_delta"));
    }

    #[test]
    fn anthropic_request_accepts_document_user_content_block() {
        let request = json!({
            "model": "gpt-5.5",
            "messages": [{
                "role": "user",
                "content": [
                    {
                        "type": "document",
                        "name": "notes.md",
                        "source": {
                            "type": "text",
                            "media_type": "text/markdown",
                            "data": "# Heading\nbody"
                        }
                    }
                ]
            }]
        });

        let (adapted, _) = adapt_anthropic_messages_request(
            request.as_object().expect("request should be object"),
        )
        .expect("document block should normalize");

        assert_eq!(adapted["input"][0]["role"], json!("user"));
        assert_eq!(adapted["input"][0]["content"][0]["type"], json!("input_file"));
        assert_eq!(adapted["input"][0]["content"][0]["filename"], json!("notes.md"));
        assert_eq!(
            adapted["input"][0]["content"][0]["file_data"],
            json!("data:text/markdown;base64,IyBIZWFkaW5nCmJvZHk=")
        );
    }
}
