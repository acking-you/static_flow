//! Shared preflight for Anthropic-compatible requests entering the Kiro
//! surface.
//!
//! This module intentionally stops before Kiro wire conversion. It provides the
//! normalized Anthropic request shape that both Kiro-native dispatch and direct
//! Anthropic upstream dispatch can reuse without duplicating cleanup rules.

use super::{
    converter::{
        normalize_request, ConversionError, NormalizationEvent, ToolNormalizationEvent,
        ToolUseIdRewrite, ToolValidationSummary,
    },
    types::MessagesRequest,
};

#[derive(Debug)]
pub struct PreprocessedMessagesRequest {
    pub request: MessagesRequest,
    pub tool_use_id_rewrites: Vec<ToolUseIdRewrite>,
    pub normalization_events: Vec<NormalizationEvent>,
    pub tool_normalization_events: Vec<ToolNormalizationEvent>,
    pub tool_validation_summary: ToolValidationSummary,
}

pub fn preprocess_messages_request(
    request: &MessagesRequest,
) -> Result<PreprocessedMessagesRequest, ConversionError> {
    let normalized = normalize_request(request)?;
    Ok(PreprocessedMessagesRequest {
        request: normalized.request,
        tool_use_id_rewrites: normalized.tool_use_id_rewrites,
        normalization_events: normalized.normalization_events,
        tool_normalization_events: normalized.tool_normalization_events,
        tool_validation_summary: normalized.tool_validation_summary,
    })
}

#[cfg(test)]
mod tests {
    use crate::anthropic::{
        preflight::preprocess_messages_request,
        types::{Message, MessagesRequest},
    };

    fn request_with_invalid_history_tool_use_id() -> MessagesRequest {
        MessagesRequest {
            model: "claude-opus-4-8".to_string(),
            _max_tokens: 128,
            messages: vec![
                Message {
                    role: "user".to_string(),
                    content: serde_json::json!("Run the tool"),
                },
                Message {
                    role: "assistant".to_string(),
                    content: serde_json::json!([
                        {
                            "type": "tool_use",
                            "id": "toolu.01:bad",
                            "name": "read_file",
                            "input": {"path": "/tmp/test.txt"}
                        }
                    ]),
                },
                Message {
                    role: "user".to_string(),
                    content: serde_json::json!([
                        {
                            "type": "tool_result",
                            "tool_use_id": "toolu.01:bad",
                            "content": "file content"
                        }
                    ]),
                },
            ],
            stream: false,
            system: None,
            tools: None,
            _tool_choice: None,
            thinking: None,
            output_config: None,
            metadata: None,
        }
    }

    #[test]
    fn preflight_normalizes_tool_use_ids_for_shared_kiro_anthropic_surface() {
        let preflight = preprocess_messages_request(&request_with_invalid_history_tool_use_id())
            .expect("preflight should normalize reusable Kiro Anthropic request shape");

        assert_eq!(preflight.tool_use_id_rewrites.len(), 1);
        let rewrite = &preflight.tool_use_id_rewrites[0];
        assert_eq!(rewrite.original_tool_use_id, "toolu.01:bad");
        assert!(rewrite
            .rewritten_tool_use_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'));

        let assistant_id = preflight.request.messages[1].content[0]["id"]
            .as_str()
            .expect("assistant tool_use id should remain a string");
        let result_id = preflight.request.messages[2].content[0]["tool_use_id"]
            .as_str()
            .expect("tool_result id should remain a string");
        assert_eq!(assistant_id, rewrite.rewritten_tool_use_id);
        assert_eq!(result_id, rewrite.rewritten_tool_use_id);
    }
}
