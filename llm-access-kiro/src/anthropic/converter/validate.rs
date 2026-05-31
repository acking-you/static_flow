//! Up-front validation of incoming Anthropic message requests.

use super::{document::normalize_document_block_payload, invalid_request, ConversionError};
use crate::anthropic::types::MessagesRequest;

pub fn validate_messages_request(req: &MessagesRequest) -> Result<(), ConversionError> {
    for (message_index, message) in req.messages.iter().enumerate() {
        match message.role.as_str() {
            "user" => validate_user_message_content(&message.content, message_index)?,
            "assistant" => validate_assistant_message_content(&message.content, message_index)?,
            other => {
                return Err(invalid_request(format!(
                    "message {message_index} has unsupported role `{other}`"
                )));
            },
        }
    }
    Ok(())
}

fn validate_user_message_content(
    content: &serde_json::Value,
    message_index: usize,
) -> Result<(), ConversionError> {
    match content {
        serde_json::Value::String(text) => {
            if text.trim().is_empty() {
                return Err(invalid_request(format!(
                    "message {message_index} content must not be empty"
                )));
            }
            Ok(())
        },
        serde_json::Value::Array(items) => {
            if items.is_empty() {
                return Err(invalid_request(format!(
                    "message {message_index} content blocks must not be empty"
                )));
            }
            let mut has_supported_content = false;
            for (block_index, item) in items.iter().enumerate() {
                let Some(obj) = item.as_object() else {
                    return Err(invalid_request(format!(
                        "message {message_index} content block {block_index} must be an object"
                    )));
                };
                let Some(block_type) = obj
                    .get("type")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                else {
                    return Err(invalid_request(format!(
                        "message {message_index} content block {block_index} is missing type"
                    )));
                };
                match block_type {
                    "text" => {
                        if obj
                            .get("text")
                            .and_then(serde_json::Value::as_str)
                            .map(str::trim)
                            .is_some_and(|value| !value.is_empty())
                        {
                            has_supported_content = true;
                        }
                    },
                    "image" => {
                        let Some(source) = obj.get("source").and_then(serde_json::Value::as_object)
                        else {
                            return Err(invalid_request(format!(
                                "message {message_index} image block {block_index} is missing \
                                 source"
                            )));
                        };
                        let Some(source_type) = source
                            .get("type")
                            .and_then(serde_json::Value::as_str)
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                        else {
                            return Err(invalid_request(format!(
                                "message {message_index} image block {block_index} is missing \
                                 source.type"
                            )));
                        };
                        if source_type != "base64" {
                            return Err(invalid_request(format!(
                                "message {message_index} image block {block_index} must use \
                                 source.type=`base64`"
                            )));
                        }
                        if source
                            .get("media_type")
                            .and_then(serde_json::Value::as_str)
                            .map(str::trim)
                            .is_none_or(|value| value.is_empty())
                        {
                            return Err(invalid_request(format!(
                                "message {message_index} image block {block_index} is missing \
                                 source.media_type"
                            )));
                        }
                        if source
                            .get("data")
                            .and_then(serde_json::Value::as_str)
                            .map(str::trim)
                            .is_none_or(|value| value.is_empty())
                        {
                            return Err(invalid_request(format!(
                                "message {message_index} image block {block_index} is missing \
                                 source.data"
                            )));
                        }
                        has_supported_content = true;
                    },
                    "document" => {
                        let _ = normalize_document_block_payload(obj, message_index, block_index)?;
                        has_supported_content = true;
                    },
                    "tool_result" => {
                        if obj
                            .get("tool_use_id")
                            .and_then(serde_json::Value::as_str)
                            .map(str::trim)
                            .is_none_or(|value| value.is_empty())
                        {
                            return Err(invalid_request(format!(
                                "message {message_index} tool_result block {block_index} is \
                                 missing tool_use_id"
                            )));
                        }
                        has_supported_content = true;
                    },
                    other => {
                        return Err(invalid_request(format!(
                            "message {message_index} content block {block_index} has unsupported \
                             type `{other}` for role `user`"
                        )));
                    },
                }
            }
            if !has_supported_content {
                return Err(invalid_request(format!(
                    "message {message_index} has no supported content blocks"
                )));
            }
            Ok(())
        },
        _ => Err(invalid_request(format!(
            "message {message_index} content must be a string or array"
        ))),
    }
}

fn validate_assistant_message_content(
    content: &serde_json::Value,
    message_index: usize,
) -> Result<(), ConversionError> {
    match content {
        serde_json::Value::String(text) => {
            if text.trim().is_empty() {
                return Err(invalid_request(format!(
                    "message {message_index} content must not be empty"
                )));
            }
            Ok(())
        },
        serde_json::Value::Array(items) => {
            if items.is_empty() {
                return Err(invalid_request(format!(
                    "message {message_index} content blocks must not be empty"
                )));
            }
            let mut has_supported_content = false;
            for (block_index, item) in items.iter().enumerate() {
                let Some(obj) = item.as_object() else {
                    return Err(invalid_request(format!(
                        "message {message_index} content block {block_index} must be an object"
                    )));
                };
                let Some(block_type) = obj
                    .get("type")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                else {
                    return Err(invalid_request(format!(
                        "message {message_index} content block {block_index} is missing type"
                    )));
                };
                match block_type {
                    "text" => {
                        if obj
                            .get("text")
                            .and_then(serde_json::Value::as_str)
                            .map(str::trim)
                            .is_some_and(|value| !value.is_empty())
                        {
                            has_supported_content = true;
                        }
                    },
                    "thinking" => {
                        if obj
                            .get("thinking")
                            .and_then(serde_json::Value::as_str)
                            .map(str::trim)
                            .is_some_and(|value| !value.is_empty())
                        {
                            has_supported_content = true;
                        }
                    },
                    "tool_use" => {
                        if obj
                            .get("id")
                            .and_then(serde_json::Value::as_str)
                            .map(str::trim)
                            .is_none_or(|value| value.is_empty())
                        {
                            return Err(invalid_request(format!(
                                "message {message_index} tool_use block {block_index} is missing \
                                 id"
                            )));
                        }
                        if obj
                            .get("name")
                            .and_then(serde_json::Value::as_str)
                            .map(str::trim)
                            .is_none_or(|value| value.is_empty())
                        {
                            return Err(invalid_request(format!(
                                "message {message_index} tool_use block {block_index} is missing \
                                 name"
                            )));
                        }
                        has_supported_content = true;
                    },
                    other => {
                        return Err(invalid_request(format!(
                            "message {message_index} content block {block_index} has unsupported \
                             type `{other}` for role `assistant`"
                        )));
                    },
                }
            }
            if !has_supported_content {
                return Err(invalid_request(format!(
                    "message {message_index} has no supported content blocks"
                )));
            }
            Ok(())
        },
        _ => Err(invalid_request(format!(
            "message {message_index} content must be a string or array"
        ))),
    }
}
