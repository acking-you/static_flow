//! System-prompt assembly: role-message-to-system conversion, system text
//! cleanup, and building the injected system content block.

use super::{
    identity::{
        anthropic_identity_override, effective_response_identity_for_request,
        normalize_claude_code_model_identity, strip_volatile_claude_code_billing_header,
    },
    invalid_request,
    tools::structured_output_instruction,
    ConversionError, SYSTEM_CHUNKED_POLICY, SYSTEM_PROMPT_PRIVACY_POLICY,
    VISIBLE_THINKING_PRIVACY_POLICY,
};
use crate::anthropic::types::{MessagesRequest, SystemMessage};

pub fn system_message_from_role_message(
    message: &crate::anthropic::types::Message,
    message_index: usize,
) -> Result<SystemMessage, ConversionError> {
    let role = message.role.as_str();
    let text = match &message.content {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(items) => {
            let mut text_parts = Vec::new();
            for (block_index, item) in items.iter().enumerate() {
                let Some(obj) = item.as_object() else {
                    return Err(invalid_request(format!(
                        "message {message_index} {role} block {block_index} must be an object"
                    )));
                };
                let Some(block_type) = obj.get("type").and_then(serde_json::Value::as_str) else {
                    return Err(invalid_request(format!(
                        "message {message_index} {role} block {block_index} is missing type"
                    )));
                };
                if block_type != "text" {
                    return Err(invalid_request(format!(
                        "message {message_index} {role} block {block_index} has unsupported type \
                         `{block_type}`"
                    )));
                }
                let Some(text) = obj.get("text").and_then(serde_json::Value::as_str) else {
                    return Err(invalid_request(format!(
                        "message {message_index} {role} text block {block_index} is missing text"
                    )));
                };
                text_parts.push(text.to_string());
            }
            text_parts.join("\n")
        },
        _ => {
            return Err(invalid_request(format!(
                "message {message_index} {role} content must be a string or array"
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
    cctest_text_handling_enabled: bool,
) -> Option<String> {
    if !cctest_text_handling_enabled {
        let mut parts = Vec::new();
        if let Some(system_content) = req
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
        {
            parts.push(system_content);
        }
        if let Some(tool_name) = structured_output_tool_name {
            parts.push(structured_output_instruction(tool_name));
        }
        return (!parts.is_empty()).then(|| parts.join("\n"));
    }

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
        .map(|content| {
            [
                content,
                SYSTEM_CHUNKED_POLICY.to_string(),
                VISIBLE_THINKING_PRIVACY_POLICY.to_string(),
                SYSTEM_PROMPT_PRIVACY_POLICY.to_string(),
                identity_override.clone(),
            ]
            .join("\n")
        });

    let mut parts = Vec::new();
    parts.push(system_content.unwrap_or_else(|| {
        [VISIBLE_THINKING_PRIVACY_POLICY, SYSTEM_PROMPT_PRIVACY_POLICY, identity_override.as_str()]
            .join("\n")
    }));
    if let Some(tool_name) = structured_output_tool_name {
        parts.push(structured_output_instruction(tool_name));
    }
    Some(parts.join("\n"))
}
