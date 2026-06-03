//! System-prompt assembly: role-message-to-system conversion, system text
//! cleanup, and building the injected system content block.

use super::{
    identity::{
        anthropic_identity_override, effective_response_identity_for_request,
        normalize_claude_code_model_identity, strip_volatile_claude_code_billing_header,
    },
    invalid_request,
    tools::structured_output_instruction,
    ConversionError, SYSTEM_CHUNKED_POLICY,
};
use crate::anthropic::types::{MessagesRequest, SystemMessage};

pub fn system_message_from_role_message(
    message: &crate::anthropic::types::Message,
    message_index: usize,
) -> Result<SystemMessage, ConversionError> {
    let text = match &message.content {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(items) => {
            let mut text_parts = Vec::new();
            for (block_index, item) in items.iter().enumerate() {
                let Some(obj) = item.as_object() else {
                    return Err(invalid_request(format!(
                        "message {message_index} system block {block_index} must be an object"
                    )));
                };
                let Some(block_type) = obj.get("type").and_then(serde_json::Value::as_str) else {
                    return Err(invalid_request(format!(
                        "message {message_index} system block {block_index} is missing type"
                    )));
                };
                if block_type != "text" {
                    return Err(invalid_request(format!(
                        "message {message_index} system block {block_index} has unsupported type \
                         `{block_type}`"
                    )));
                }
                let Some(text) = obj.get("text").and_then(serde_json::Value::as_str) else {
                    return Err(invalid_request(format!(
                        "message {message_index} system text block {block_index} is missing text"
                    )));
                };
                text_parts.push(text.to_string());
            }
            text_parts.join("\n")
        },
        _ => {
            return Err(invalid_request(format!(
                "message {message_index} system content must be a string or array"
            )));
        },
    };
    Ok(SystemMessage {
        text,
    })
}

pub fn cleaned_system_message_text(message: &SystemMessage) -> Option<String> {
    let content = strip_volatile_claude_code_billing_header(message.text.clone());
    (!content.trim().is_empty()).then_some(content)
}

pub fn build_injected_system_content(
    req: &MessagesRequest,
    structured_output_tool_name: Option<&str>,
) -> Option<String> {
    let identity = effective_response_identity_for_request(req);
    let identity_override = anthropic_identity_override(identity.as_ref());
    let system_content = req
        .system
        .as_ref()
        .map(|system| {
            system
                .iter()
                .filter_map(cleaned_system_message_text)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .filter(|content| !content.is_empty())
        .map(strip_volatile_claude_code_billing_header)
        .map(|content| normalize_claude_code_model_identity(content, identity.as_ref()))
        .map(|content| format!("{content}\n{SYSTEM_CHUNKED_POLICY}\n{identity_override}"));

    let mut parts = Vec::new();
    parts.push(system_content.unwrap_or(identity_override));
    if let Some(tool_name) = structured_output_tool_name {
        parts.push(structured_output_instruction(tool_name));
    }
    Some(parts.join("\n"))
}
