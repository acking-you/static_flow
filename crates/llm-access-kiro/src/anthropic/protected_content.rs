//! Strict validation for client-supplied protected Anthropic content.

use std::fmt;

use serde_json::Value;

use crate::anthropic::{
    stream::{find_real_thinking_start_tag, verify_protected_thinking_signature},
    types::{Message, MessagesRequest},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtectedContentError {
    message: String,
}

impl fmt::Display for ProtectedContentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ProtectedContentError {}

fn protected_error(message: impl Into<String>) -> ProtectedContentError {
    ProtectedContentError {
        message: message.into(),
    }
}

pub fn validate_protected_content(
    req: &MessagesRequest,
    key_id: &str,
    model: &str,
    secret: &str,
) -> Result<(), ProtectedContentError> {
    for (message_index, message) in req.messages.iter().enumerate() {
        validate_message(message, message_index, key_id, model, secret)?;
    }
    Ok(())
}

fn validate_message(
    message: &Message,
    message_index: usize,
    key_id: &str,
    model: &str,
    secret: &str,
) -> Result<(), ProtectedContentError> {
    reject_raw_assistant_thinking_tags(message, message_index)?;
    reject_encrypted_content_for_message(message, message_index)?;
    let Value::Array(items) = &message.content else {
        return Ok(());
    };

    for (block_index, item) in items.iter().enumerate() {
        let Some(obj) = item.as_object() else {
            continue;
        };
        let block_type = obj.get("type").and_then(Value::as_str).unwrap_or("");
        if obj.contains_key("signature")
            && !(message.role == "assistant" && block_type == "thinking")
        {
            return Err(protected_error(format!(
                "message {message_index} content block {block_index} has signature on unsupported \
                 protected content"
            )));
        }
        if message.role != "assistant" || block_type != "thinking" {
            continue;
        }
        let thinking = obj
            .get("thinking")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                protected_error(format!(
                    "message {message_index} thinking block {block_index} is missing thinking"
                ))
            })?;
        let signature = obj
            .get("signature")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                protected_error(format!(
                    "message {message_index} thinking block {block_index} is missing signature"
                ))
            })?;
        if !verify_protected_thinking_signature(model, thinking, key_id, secret, signature) {
            return Err(protected_error(format!(
                "message {message_index} thinking block {block_index} has invalid thinking \
                 signature"
            )));
        }
    }

    Ok(())
}

fn reject_raw_assistant_thinking_tags(
    message: &Message,
    message_index: usize,
) -> Result<(), ProtectedContentError> {
    if message.role != "assistant" {
        return Ok(());
    }
    match &message.content {
        Value::String(text) => reject_raw_thinking_text(text, message_index),
        Value::Array(items) => {
            for item in items {
                let Some(obj) = item.as_object() else {
                    continue;
                };
                if obj.get("type").and_then(Value::as_str) != Some("text") {
                    continue;
                }
                if let Some(text) = obj.get("text").and_then(Value::as_str) {
                    reject_raw_thinking_text(text, message_index)?;
                }
            }
            Ok(())
        },
        _ => Ok(()),
    }
}

fn reject_raw_thinking_text(text: &str, message_index: usize) -> Result<(), ProtectedContentError> {
    if find_real_thinking_start_tag(text).is_some() {
        return Err(protected_error(format!(
            "message {message_index} contains unsigned thinking tags"
        )));
    }
    Ok(())
}

fn reject_encrypted_content_for_message(
    message: &Message,
    message_index: usize,
) -> Result<(), ProtectedContentError> {
    let Value::Array(items) = &message.content else {
        return reject_encrypted_content(&message.content, message_index, false);
    };
    for item in items {
        let allow_gateway_web_search_result = item
            .as_object()
            .and_then(|obj| obj.get("type"))
            .and_then(Value::as_str)
            == Some("web_search_tool_result");
        reject_encrypted_content(item, message_index, allow_gateway_web_search_result)?;
    }
    Ok(())
}

fn reject_encrypted_content(
    value: &Value,
    message_index: usize,
    allow_gateway_web_search_result: bool,
) -> Result<(), ProtectedContentError> {
    match value {
        Value::Object(obj) => {
            let is_gateway_web_search_result = allow_gateway_web_search_result
                && obj.get("type").and_then(Value::as_str) == Some("web_search_result");
            if obj.contains_key("encrypted_content") && !is_gateway_web_search_result {
                return Err(protected_error(format!(
                    "message {message_index} contains unverifiable encrypted_content"
                )));
            }
            for child in obj.values() {
                reject_encrypted_content(child, message_index, allow_gateway_web_search_result)?;
            }
        },
        Value::Array(items) => {
            for child in items {
                reject_encrypted_content(child, message_index, allow_gateway_web_search_result)?;
            }
        },
        _ => {},
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::anthropic::{
        protected_content::validate_protected_content, stream::protected_thinking_signature,
        types::MessagesRequest,
    };

    fn request_with_messages(messages: serde_json::Value) -> MessagesRequest {
        serde_json::from_value(json!({
            "model": "claude-opus-4-8",
            "max_tokens": 128,
            "messages": messages
        }))
        .expect("request should deserialize")
    }

    #[test]
    fn validates_signed_assistant_thinking_history() {
        let signature = protected_thinking_signature(
            "claude-opus-4-8",
            "private reasoning",
            "kiro-key-1",
            "server-secret",
        );
        let req = request_with_messages(json!([
            {"role": "user", "content": "hello"},
            {"role": "assistant", "content": [
                {
                    "type": "thinking",
                    "thinking": "private reasoning",
                    "signature": signature
                },
                {"type": "text", "text": "answer"}
            ]},
            {"role": "user", "content": "continue"}
        ]));

        validate_protected_content(&req, "kiro-key-1", "claude-opus-4-8", "server-secret")
            .expect("valid signature should pass");
    }

    #[test]
    fn rejects_tampered_thinking_signature() {
        let signature = protected_thinking_signature(
            "claude-opus-4-8",
            "private reasoning",
            "kiro-key-1",
            "server-secret",
        );
        let req = request_with_messages(json!([
            {"role": "user", "content": "hello"},
            {"role": "assistant", "content": [
                {
                    "type": "thinking",
                    "thinking": "changed reasoning",
                    "signature": signature
                }
            ]},
            {"role": "user", "content": "continue"}
        ]));

        let err =
            validate_protected_content(&req, "kiro-key-1", "claude-opus-4-8", "server-secret")
                .expect_err("tampered thinking should fail");
        assert!(err.to_string().contains("invalid thinking signature"));
    }

    #[test]
    fn rejects_unverifiable_encrypted_content() {
        let req = request_with_messages(json!([
            {"role": "user", "content": "hello"},
            {"role": "assistant", "content": [
                {
                    "type": "text",
                    "text": "answer",
                    "encrypted_content": "opaque"
                }
            ]},
            {"role": "user", "content": "continue"}
        ]));

        let err =
            validate_protected_content(&req, "kiro-key-1", "claude-opus-4-8", "server-secret")
                .expect_err("encrypted content should fail");
        assert!(err.to_string().contains("encrypted_content"));
    }

    #[test]
    fn allows_gateway_web_search_history_content() {
        let req = request_with_messages(json!([
            {"role": "user", "content": "search"},
            {"role": "assistant", "content": [
                {
                    "type": "web_search_tool_result",
                    "content": [{
                        "type": "web_search_result",
                        "title": "result",
                        "url": "https://example.com",
                        "encrypted_content": "StaticFlow plaintext snippet"
                    }]
                }
            ]},
            {"role": "user", "content": "continue"}
        ]));

        validate_protected_content(&req, "kiro-key-1", "claude-opus-4-8", "server-secret")
            .expect("gateway web-search history should pass");
    }

    #[test]
    fn rejects_raw_thinking_tags_in_assistant_string_content() {
        let req = request_with_messages(json!([
            {"role": "user", "content": "hello"},
            {"role": "assistant", "content": "<thinking>unsigned</thinking>\n\nanswer"},
            {"role": "user", "content": "continue"}
        ]));

        let err =
            validate_protected_content(&req, "kiro-key-1", "claude-opus-4-8", "server-secret")
                .expect_err("raw thinking tags should fail");
        assert!(err.to_string().contains("unsigned thinking tags"));
    }

    #[test]
    fn rejects_raw_thinking_tags_in_assistant_text_block() {
        let req = request_with_messages(json!([
            {"role": "user", "content": "hello"},
            {"role": "assistant", "content": [
                {"type": "text", "text": "<thinking>unsigned</thinking>\n\nanswer"}
            ]},
            {"role": "user", "content": "continue"}
        ]));

        let err =
            validate_protected_content(&req, "kiro-key-1", "claude-opus-4-8", "server-secret")
                .expect_err("raw thinking tags should fail");
        assert!(err.to_string().contains("unsigned thinking tags"));
    }
}
