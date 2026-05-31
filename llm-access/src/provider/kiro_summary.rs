//! Kiro last-message/tool-result summarization helpers.

use std::collections::HashMap;

use llm_access_kiro::anthropic::{
    converter::{current_user_message_range, extract_tool_result_content},
    types::MessagesRequest,
};
use serde_json::Value;

use super::{KIRO_LAST_MESSAGE_PART_PREVIEW_CHARS, KIRO_LAST_MESSAGE_TOTAL_PREVIEW_CHARS};

pub fn extract_last_message_from_kiro_messages(payload: &MessagesRequest) -> Option<String> {
    let current_range = current_user_message_range(&payload.messages).ok()?;
    let tool_name_by_id = collect_kiro_tool_name_map(&payload.messages[..current_range.start]);
    let mut parts = Vec::new();
    for message in &payload.messages[current_range] {
        append_kiro_message_summary_parts(&message.content, &tool_name_by_id, &mut parts);
    }
    if parts.is_empty() {
        None
    } else {
        Some(truncate_summary(&parts.join("\n"), KIRO_LAST_MESSAGE_TOTAL_PREVIEW_CHARS))
    }
}
fn collect_kiro_tool_name_map(
    messages: &[llm_access_kiro::anthropic::types::Message],
) -> HashMap<String, String> {
    let mut tool_name_by_id = HashMap::new();
    for message in messages {
        let Some(blocks) = message.content.as_array() else {
            continue;
        };
        for block in blocks {
            if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                continue;
            }
            let Some(tool_use_id) = block
                .get("id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                continue;
            };
            let Some(tool_name) = block
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                continue;
            };
            tool_name_by_id.insert(tool_use_id.to_string(), tool_name.to_string());
        }
    }
    tool_name_by_id
}
fn append_kiro_message_summary_parts(
    content: &Value,
    tool_name_by_id: &HashMap<String, String>,
    parts: &mut Vec<String>,
) {
    match content {
        Value::String(text) => {
            if let Some(summary) = summarize_text(text) {
                parts.push(summary);
            }
        },
        Value::Array(blocks) => {
            for block in blocks {
                match block.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        if let Some(text) = block.get("text").and_then(Value::as_str) {
                            if let Some(summary) = summarize_text(text) {
                                parts.push(summary);
                            }
                        }
                    },
                    Some("tool_result") => {
                        if let Some(summary) = summarize_tool_result(block, tool_name_by_id) {
                            parts.push(summary);
                        }
                    },
                    Some("tool_use") => {
                        if let Some(name) = block.get("name").and_then(Value::as_str) {
                            if let Some(summary) = summarize_text(&format!("[tool_use:{name}]")) {
                                parts.push(summary);
                            }
                        }
                    },
                    Some("image") => parts.push("[image]".to_string()),
                    _ => {},
                }
            }
        },
        _ => {},
    }
}
fn summarize_tool_result(
    block: &Value,
    tool_name_by_id: &HashMap<String, String>,
) -> Option<String> {
    let tool_use_id = block
        .get("tool_use_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let label = tool_name_by_id
        .get(tool_use_id)
        .map(String::as_str)
        .unwrap_or(tool_use_id);
    let preview = extract_tool_result_content(&block.get("content").cloned());
    let preview = compact_preview(&preview, KIRO_LAST_MESSAGE_PART_PREVIEW_CHARS);
    Some(if preview.is_empty() {
        format!("[tool_result:{label}]")
    } else {
        format!("[tool_result:{label}] {preview}")
    })
}
fn summarize_text(text: &str) -> Option<String> {
    let preview = compact_preview(text, KIRO_LAST_MESSAGE_PART_PREVIEW_CHARS);
    (!preview.is_empty()).then_some(preview)
}
fn compact_preview(text: &str, max_chars: usize) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_summary(compact.trim(), max_chars)
}
fn truncate_summary(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}
