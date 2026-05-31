//! Converts Anthropic Messages API requests into Kiro wire `ConversationState`.
//!
//! Handles model name mapping, system prompt injection, thinking mode prefixes,
//! tool schema normalization, conversation history building (with consecutive
//! same-role message merging), and tool-result pairing validation.
//!
//! ## Module map
//!
//! `converter/` splits Anthropic->Kiro request conversion into focused
//! submodules; this `mod.rs` facade keeps the shared data model, errors,
//! and consts, and re-exports the public entry points by name.

mod convert;
mod document;
mod identity;
mod image;
mod model;
mod normalize;
mod schema;
mod session;
mod system;
mod thinking;
mod tool_name;
mod tool_pairing;
mod tool_result;
mod tools;
mod validate;

use std::collections::{BTreeMap, HashMap};

pub use convert::{convert_normalized_request_with_resolved_session, current_user_message_range};
pub use model::{get_context_window_size, map_model};
pub use normalize::normalize_request;
pub use session::{preview_session_value, resolve_conversation_id_from_metadata};
pub use tool_result::extract_tool_result_content;

use super::types::{MessagesRequest, SystemMessage};
use crate::wire::{ConversationState, KiroDocument, KiroImage, ToolResult};

const MULTIMODAL_UNSUPPORTED_SCHEMA_KEYWORDS: &[&str] = &[
    "anyOf",
    "oneOf",
    "allOf",
    "contains",
    "dependentSchemas",
    "patternProperties",
    "$defs",
    "definitions",
    "prefixItems",
    "unevaluatedProperties",
];
const CLAUDE_CODE_BILLING_HEADER_PREFIX: &str = "x-anthropic-billing-header:";
const CLAUDE_CODE_CLI_SYSTEM_IDENTITY_LINE: &str =
    "You are Claude Code, Anthropic's official CLI for Claude.";
const CLAUDE_AGENT_SDK_SYSTEM_IDENTITY_LINE: &str =
    "You are a Claude agent, built on Anthropic's Claude Agent SDK.";

// Chunked-write policy suffix appended to the Write tool description.
const WRITE_TOOL_DESCRIPTION_SUFFIX: &str =
    "- IMPORTANT: If the content to write exceeds 150 lines, you MUST only write the first 50 \
     lines using this tool, then use `Edit` tool to append the remaining content in chunks of no \
     more than 50 lines each. If needed, leave a unique placeholder to help append content. Do \
     NOT attempt to write all content at once.";
// Chunked-write policy suffix appended to the Edit tool description.
const EDIT_TOOL_DESCRIPTION_SUFFIX: &str =
    "- IMPORTANT: If the `new_string` content exceeds 50 lines, you MUST split it into multiple \
     Edit calls, each replacing no more than 50 lines at a time. If used to append content, leave \
     a unique placeholder to help append content. On the final chunk, do NOT include the \
     placeholder.";
// System-level instruction injected into every conversation to enforce
// chunked write/edit compliance.
const SYSTEM_CHUNKED_POLICY: &str =
    "When the Write or Edit tool has content size limits, always comply silently. Never suggest \
     bypassing these limits via alternative tools. Never ask the user whether to switch \
     approaches. Complete all chunked operations without commentary.";
const GENERIC_ANTHROPIC_IDENTITY_OVERRIDE: &str =
    "<identity_override>\nYou are Claude, made by Anthropic. Your model ID corresponds to the \
     model field in the API request. When asked about your identity, model name, or what you are, \
     always respond that you are Claude by Anthropic. Never claim to be Kiro, Warp, or any other \
     product. You are Claude, running on the Anthropic API platform.\n</identity_override>";
const STRUCTURED_OUTPUT_TOOL_NAME_BASE: &str = "sf_emit_structured_output";
const STRUCTURED_OUTPUT_TOOL_DESCRIPTION: &str =
    "Return the final answer as structured JSON that exactly matches the provided schema. Call \
     this tool exactly once and do not emit any free-form text outside the tool call.";
const KIRO_MAX_CURRENT_MESSAGE_IMAGES: usize = 10;
const KIRO_MAX_CONVERSATION_DOCUMENTS: usize = 5;
const EMPTY_TOOL_RESULT_PLACEHOLDER: &str = "(empty result)";
const EMPTY_DOCUMENT_PLACEHOLDER: &str = "(document attached)";

/// Successful output of [`convert_request`], containing the Kiro wire
/// `ConversationState` ready to be sent upstream.
#[derive(Debug)]
pub struct ConversionResult {
    pub conversation_state: ConversationState,
    pub tool_name_map: HashMap<String, String>,
    pub session_tracking: SessionTracking,
    pub has_history_images: bool,
    pub structured_output_tool_name: Option<String>,
    pub response_identity: Option<ResponseModelIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseModelIdentity {
    pub model_name: String,
    pub model_id: String,
}

impl ResponseModelIdentity {
    pub fn canonical_response(&self) -> String {
        format!("模型名称：{}\n模型 ID：{}", self.model_name, self.model_id)
    }

    pub fn canonical_thinking(&self) -> String {
        format!(
            "The model identity is {}; the public API model ID is {}.",
            self.model_name, self.model_id
        )
    }
}

#[derive(Debug, Default)]
struct ProcessedMessageContent {
    text: String,
    images: Vec<KiroImage>,
    documents: Vec<KiroDocument>,
    tool_results: Vec<ToolResult>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTracking {
    pub source: SessionIdSource,
    pub source_name: Option<&'static str>,
    pub source_value_preview: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedConversationId {
    pub conversation_id: String,
    pub session_tracking: SessionTracking,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionIdSource {
    RequestHeader,
    MetadataJson,
    MetadataLegacy,
    RecoveredAnchor(SessionFallbackReason),
    GeneratedFallback(SessionFallbackReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionFallbackReason {
    InvalidHeaderSessionId,
    MissingMetadata,
    MissingUserId,
    MissingJsonSessionId,
    InvalidJsonSessionId,
    MissingLegacySessionId,
    InvalidLegacySessionId,
}

impl SessionFallbackReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::InvalidHeaderSessionId => "invalid_header_session_id",
            Self::MissingMetadata => "missing_metadata",
            Self::MissingUserId => "missing_user_id",
            Self::MissingJsonSessionId => "missing_json_session_id",
            Self::InvalidJsonSessionId => "invalid_json_session_id",
            Self::MissingLegacySessionId => "missing_legacy_session_id",
            Self::InvalidLegacySessionId => "invalid_legacy_session_id",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizationEvent {
    pub message_index: usize,
    pub role: String,
    pub content_block_index: Option<usize>,
    pub block_type: Option<String>,
    pub action: &'static str,
    pub reason: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolNormalizationEvent {
    pub tool_index: usize,
    pub tool_name: String,
    pub action: &'static str,
    pub reason: &'static str,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolValidationSummary {
    pub normalized_tool_description_count: usize,
    pub empty_tool_name_count: usize,
    pub schema_keyword_counts: BTreeMap<String, usize>,
}

type ToolNormalizationResult =
    (Option<Vec<super::types::Tool>>, Vec<ToolNormalizationEvent>, ToolValidationSummary);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolUseIdRewrite {
    pub original_tool_use_id: String,
    pub rewritten_tool_use_id: String,
    pub assistant_message_index: usize,
    pub content_block_index: usize,
    pub rewritten_tool_result_count: usize,
}

/// Errors that can occur during Anthropic-to-Kiro request conversion.
#[derive(Debug)]
pub enum ConversionError {
    UnsupportedModel(String),
    EmptyMessages,
    InvalidRequest(String),
}

impl std::fmt::Display for ConversionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedModel(model) => write!(f, "unsupported model: {model}"),
            Self::EmptyMessages => write!(f, "messages are empty"),
            Self::InvalidRequest(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for ConversionError {}

fn invalid_request(message: impl Into<String>) -> ConversionError {
    ConversionError::InvalidRequest(message.into())
}

const SESSION_SOURCE_PREVIEW_MAX_CHARS: usize = 160;

#[derive(Debug)]
struct ActiveToolUse {
    normalized_id: String,
    rewrite_index: Option<usize>,
}

#[derive(Debug)]
pub struct NormalizedRequest {
    pub request: MessagesRequest,
    pub tool_use_id_rewrites: Vec<ToolUseIdRewrite>,
    pub normalization_events: Vec<NormalizationEvent>,
    pub tool_normalization_events: Vec<ToolNormalizationEvent>,
    pub tool_validation_summary: ToolValidationSummary,
    message_index_map: Vec<usize>,
}

enum SystemRoleDisposition {
    StableSystemPrefix(SystemMessage),
    DropDynamicNoise,
    DropEmpty,
    PreserveInOrderUserContext { message: super::types::Message, reason: &'static str },
}

const TOOL_NAME_MAX_LEN: usize = 63;


#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{
        super::types::{
            Message as AnthropicMessage, Metadata, SystemMessage, Tool as AnthropicTool,
        },
        convert::{convert_request, convert_request_with_validation},
        document::generate_document_name,
        schema::permissive_object_schema,
        session::is_valid_uuid,
        *,
    };
    use crate::{anthropic::types::MessagesRequest, wire::Message};

    const SAMPLE_PDF_BASE64: &str = concat!(
        "JVBERi0xLjQKMSAwIG9iago8PCAvVHlwZSAvQ2F0YWxvZyAvUGFnZXMgMiAwIFIgPj4KZW5kb2JqCjIgMCBv",
        "YmoKPDwgL1R5cGUgL1BhZ2VzIC9LaWRzIFszIDAgUl0gL0NvdW50IDEgPj4KZW5kb2JqCjMgMCBvYmoKPDwg",
        "L1R5cGUgL1BhZ2UgL1BhcmVudCAyIDAgUiAvTWVkaWFCb3ggWzAgMCAxNTAgNTBdIC9SZXNvdXJjZXMgPDwg",
        "L0ZvbnQgPDwgL0YxIDUgMCBSID4+ID4+IC9Db250ZW50cyA0IDAgUiA+PgplbmRvYmoKNCAwIG9iago8PCAv",
        "TGVuZ3RoIDM4ID4+CnN0cmVhbQpCVCAvRjEgMTQgVGYgMTAgMjAgVGQgKGh2b3l3cGtkKSBUaiBFVAplbmRz",
        "dHJlYW0KZW5kb2JqCjUgMCBvYmoKPDwgL1R5cGUgL0ZvbnQgL1N1YnR5cGUgL1R5cGUxIC9CYXNlRm9udCAv",
        "SGVsdmV0aWNhID4+CmVuZG9iagp4cmVmCjAgNgowMDAwMDAwMDAwIDY1NTM1IGYgCnRyYWlsZXIKPDwgL1Np",
        "emUgNiAvUm9vdCAxIDAgUiA+PgpzdGFydHhyZWYKMAolJUVPRg=="
    );

    fn base_request(messages: Vec<AnthropicMessage>) -> MessagesRequest {
        MessagesRequest {
            model: "claude-sonnet-4-6".to_string(),
            _max_tokens: 1024,
            messages,
            stream: false,
            system: None,
            tools: None,
            _tool_choice: None,
            thinking: None,
            output_config: None,
            metadata: None,
        }
    }

    fn semantic_history(result: &ConversionResult) -> &[Message] {
        let history = result.conversation_state.history.as_slice();
        if history.len() < 2 {
            return history;
        }
        let has_identity_prefix = match (&history[0], &history[1]) {
            (Message::User(user), Message::Assistant(assistant)) => {
                user.user_input_message
                    .content
                    .contains("<identity_override>")
                    && assistant.assistant_response_message.content
                        == "I will follow these instructions."
            },
            _ => false,
        };
        if has_identity_prefix {
            &history[2..]
        } else {
            history
        }
    }

    #[test]
    fn convert_request_uses_session_metadata_as_conversation_id() {
        let mut req = base_request(vec![AnthropicMessage {
            role: "user".to_string(),
            content: serde_json::json!("Hello"),
        }]);
        req.metadata = Some(Metadata {
            user_id: Some(
                "user_abc_account__session_a0662283-7fd3-4399-a7eb-52b9a717ae88".to_string(),
            ),
        });

        let result = convert_request(&req).expect("conversion should succeed");
        assert_eq!(
            result.conversation_state.conversation_id,
            "a0662283-7fd3-4399-a7eb-52b9a717ae88"
        );
        assert_eq!(result.session_tracking.source, SessionIdSource::MetadataLegacy);
    }

    #[test]
    fn convert_request_uses_json_session_metadata_as_conversation_id() {
        let mut req = base_request(vec![AnthropicMessage {
            role: "user".to_string(),
            content: serde_json::json!("Hello"),
        }]);
        req.metadata = Some(Metadata {
            user_id: Some(
                r#"{"device_id":"dev","account_uuid":"acct","session_id":"c4dd850d-929f-48d1-9282-f0cfefeec16e"}"#
                    .to_string(),
            ),
        });

        let result = convert_request(&req).expect("conversion should succeed");
        assert_eq!(
            result.conversation_state.conversation_id,
            "c4dd850d-929f-48d1-9282-f0cfefeec16e"
        );
        assert_eq!(result.session_tracking.source, SessionIdSource::MetadataJson);
    }

    #[test]
    fn convert_request_marks_missing_metadata_as_session_fallback() {
        let req = base_request(vec![AnthropicMessage {
            role: "user".to_string(),
            content: serde_json::json!("Hello"),
        }]);

        let result = convert_request(&req).expect("conversion should succeed");
        assert_eq!(
            result.session_tracking.source,
            SessionIdSource::GeneratedFallback(SessionFallbackReason::MissingMetadata)
        );
        assert!(result.session_tracking.source_value_preview.is_none());
        assert!(is_valid_uuid(&result.conversation_state.conversation_id));
    }

    #[test]
    fn convert_request_marks_invalid_user_id_as_session_fallback() {
        let mut req = base_request(vec![AnthropicMessage {
            role: "user".to_string(),
            content: serde_json::json!("Hello"),
        }]);
        req.metadata = Some(Metadata {
            user_id: Some(r#"{"session_id":"invalid-uuid"}"#.to_string()),
        });

        let result = convert_request(&req).expect("conversion should succeed");
        assert_eq!(
            result.session_tracking.source,
            SessionIdSource::GeneratedFallback(SessionFallbackReason::InvalidJsonSessionId)
        );
        assert_eq!(
            result.session_tracking.source_value_preview.as_deref(),
            Some(r#"{"session_id":"invalid-uuid"}"#)
        );
        assert!(is_valid_uuid(&result.conversation_state.conversation_id));
    }

    #[test]
    fn convert_request_drops_trailing_assistant_prefill() {
        let req = base_request(vec![
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("first user"),
            },
            AnthropicMessage {
                role: "assistant".to_string(),
                content: serde_json::json!("first assistant"),
            },
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("actual current user"),
            },
            AnthropicMessage {
                role: "assistant".to_string(),
                content: serde_json::json!("prefill that should be dropped"),
            },
        ]);

        let result = convert_request(&req).expect("conversion should succeed");
        assert_eq!(
            result
                .conversation_state
                .current_message
                .user_input_message
                .content,
            "actual current user"
        );
        assert_eq!(semantic_history(&result).len(), 2);
    }

    #[test]
    fn convert_request_rejects_assistant_only_prefill_with_specific_error() {
        let req = base_request(vec![AnthropicMessage {
            role: "assistant".to_string(),
            content: serde_json::json!("{"),
        }]);

        let err = convert_request(&req).expect_err("assistant-only prefill is not representable");

        assert_eq!(
            err.to_string(),
            "messages must include at least one user message before assistant prefill"
        );
    }

    #[test]
    fn ignores_whitespace_only_placeholder_blocks() {
        let req = base_request(vec![
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("first user"),
            },
            AnthropicMessage {
                role: "assistant".to_string(),
                content: serde_json::json!([
                    {"type": "text", "text": "\n"},
                    {"type": "thinking", "thinking": "  "}
                ]),
            },
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("actual current user"),
            },
        ]);

        let normalized = normalize_request(&req).expect("normalization should succeed");

        assert_eq!(normalized.request.messages.len(), 2);
        assert_eq!(normalized.request.messages[0].role, "user");
        assert_eq!(normalized.request.messages[1].role, "user");
        assert!(normalized.normalization_events.iter().any(|event| {
            event.message_index == 1
                && event.role == "assistant"
                && event.content_block_index == Some(0)
                && event.block_type.as_deref() == Some("text")
                && event.action == "drop_content_block"
                && event.reason == "whitespace_only_text_block"
        }));
        assert!(normalized.normalization_events.iter().any(|event| {
            event.message_index == 1
                && event.role == "assistant"
                && event.content_block_index == Some(1)
                && event.block_type.as_deref() == Some("thinking")
                && event.action == "drop_content_block"
                && event.reason == "whitespace_only_thinking_block"
        }));
        assert!(normalized.normalization_events.iter().any(|event| {
            event.message_index == 1
                && event.role == "assistant"
                && event.content_block_index.is_none()
                && event.action == "drop_message"
                && event.reason == "message_became_empty_after_normalization"
        }));
    }

    #[test]
    fn normalize_request_drops_empty_history_user_error_pairs() {
        let req = base_request(vec![
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!(""),
            },
            AnthropicMessage {
                role: "assistant".to_string(),
                content: serde_json::json!(
                    r#"{"error":{"message":"用户额度不足","type":"new_api_error"}}"#
                ),
            },
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("  "),
            },
            AnthropicMessage {
                role: "assistant".to_string(),
                content: serde_json::json!(
                    r#"{"error":{"message":"message 0 content must not be empty","type":"invalid_request_error"}}"#
                ),
            },
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("解释一下 Kiro API"),
            },
            AnthropicMessage {
                role: "assistant".to_string(),
                content: serde_json::json!("Kiro API 是一组兼容接口。"),
            },
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("再用一句话说明"),
            },
        ]);

        let normalized = normalize_request(&req).expect("normalization should succeed");

        assert_eq!(normalized.request.messages.len(), 3);
        assert_eq!(normalized.message_index_map, vec![4, 5, 6]);
        assert_eq!(normalized.request.messages[0].content, serde_json::json!("解释一下 Kiro API"));
        assert_eq!(
            normalized.request.messages[1].content,
            serde_json::json!("Kiro API 是一组兼容接口。")
        );
        assert_eq!(normalized.request.messages[2].content, serde_json::json!("再用一句话说明"));
        assert!(normalized.normalization_events.iter().any(|event| {
            event.message_index == 0
                && event.role == "user"
                && event.action == "drop_message"
                && event.reason == "whitespace_only_string_message"
        }));
        assert!(normalized.normalization_events.iter().any(|event| {
            event.message_index == 1
                && event.role == "assistant"
                && event.action == "drop_message"
                && event.reason == "assistant_after_empty_user_noop"
        }));
        assert!(normalized.normalization_events.iter().any(|event| {
            event.message_index == 3
                && event.role == "assistant"
                && event.action == "drop_message"
                && event.reason == "assistant_after_empty_user_noop"
        }));

        let result = convert_request(&req).expect("conversion should succeed");
        assert_eq!(semantic_history(&result).len(), 2);
        assert_eq!(
            result
                .conversation_state
                .current_message
                .user_input_message
                .content,
            "再用一句话说明"
        );
    }

    #[test]
    fn convert_request_promotes_stable_system_role_messages_for_supported_kiro_models() {
        let models = [
            "claude-sonnet-4-5-20250929",
            "claude-sonnet-4-5-20250929-thinking",
            "claude-opus-4-5-20251101",
            "claude-opus-4-5-20251101-thinking",
            "claude-sonnet-4-6",
            "claude-sonnet-4-6-thinking",
            "claude-opus-4-6",
            "claude-opus-4-6-thinking",
            "claude-opus-4-7",
            "claude-opus-4-7-thinking",
            "claude-opus-4-8",
            "claude-opus-4-8-thinking",
            "claude-haiku-4-5-20251001",
            "claude-haiku-4-5-20251001-thinking",
        ];

        for model in models {
            let mut req = base_request(vec![
                AnthropicMessage {
                    role: "user".to_string(),
                    content: serde_json::json!("first question"),
                },
                AnthropicMessage {
                    role: "system".to_string(),
                    content: serde_json::json!(
                        "You are Claude Code, Anthropic's official CLI for Claude."
                    ),
                },
                AnthropicMessage {
                    role: "system".to_string(),
                    content: serde_json::json!(
                        "SessionStart hook additional context: stable skill body"
                    ),
                },
                AnthropicMessage {
                    role: "assistant".to_string(),
                    content: serde_json::json!("first answer"),
                },
                AnthropicMessage {
                    role: "user".to_string(),
                    content: serde_json::json!("second question"),
                },
            ]);
            req.model = model.to_string();

            let normalized = normalize_request(&req).expect("normalization should succeed");
            assert_eq!(
                normalized
                    .request
                    .messages
                    .iter()
                    .map(|message| message.role.as_str())
                    .collect::<Vec<_>>(),
                vec!["user", "assistant", "user"],
                "{model}"
            );
            assert_eq!(
                normalized.request.system.as_ref().map(|messages| messages
                    .iter()
                    .map(|message| message.text.as_str())
                    .collect::<Vec<_>>()),
                Some(vec![
                    "You are Claude Code, Anthropic's official CLI for Claude.",
                    "SessionStart hook additional context: stable skill body",
                ]),
                "{model}"
            );
            assert!(
                normalized.normalization_events.iter().any(|event| {
                    event.message_index == 1
                        && event.role == "system"
                        && event.action == "promote_message"
                        && event.reason == "stable_system_role_promoted_to_top_level"
                }),
                "{model}"
            );
            assert!(
                normalized.normalization_events.iter().any(|event| {
                    event.message_index == 2
                        && event.role == "system"
                        && event.action == "promote_message"
                        && event.reason == "stable_system_role_promoted_to_top_level"
                }),
                "{model}"
            );

            let result =
                convert_request(&req).expect("conversion should accept promoted system role");
            let system_prefix = match &result.conversation_state.history[0] {
                Message::User(message) => &message.user_input_message.content,
                other => panic!("expected injected system user message for {model}, got {other:?}"),
            };
            assert!(
                system_prefix.contains("You are Claude Code, Anthropic's official CLI"),
                "{model}"
            );
            assert!(
                system_prefix.contains("SessionStart hook additional context: stable skill body"),
                "{model}"
            );
            assert_eq!(
                result
                    .conversation_state
                    .current_message
                    .user_input_message
                    .content,
                "second question",
                "{model}"
            );
        }
    }

    #[test]
    fn normalize_request_filters_dynamic_system_noise_for_cache_stability() {
        let mut req = base_request(vec![
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("first question"),
            },
            AnthropicMessage {
                role: "system".to_string(),
                content: serde_json::json!(
                    "SessionStart hook additional context: stable skill body"
                ),
            },
            AnthropicMessage {
                role: "assistant".to_string(),
                content: serde_json::json!("first answer"),
            },
            AnthropicMessage {
                role: "system".to_string(),
                content: serde_json::json!(
                    "The user sent a new message while you were working:\nvolatile \
                     interrupt\n\nIMPORTANT: After completing your current task, you MUST address \
                     the user's message above. Do not ignore it."
                ),
            },
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("second question"),
            },
            AnthropicMessage {
                role: "system".to_string(),
                content: serde_json::json!(
                    "The task tools haven't been used recently. Use them to keep progress visible."
                ),
            },
            AnthropicMessage {
                role: "assistant".to_string(),
                content: serde_json::json!("second answer"),
            },
            AnthropicMessage {
                role: "system".to_string(),
                content: serde_json::json!("Project-local rule that should stay in order."),
            },
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("third question"),
            },
        ]);
        req.system = Some(vec![SystemMessage {
            text: "Top level system.".to_string(),
        }]);

        let normalized = normalize_request(&req).expect("normalization should succeed");
        assert_eq!(
            normalized
                .request
                .messages
                .iter()
                .map(|message| message.role.as_str())
                .collect::<Vec<_>>(),
            vec!["user", "assistant", "user", "user", "assistant", "user", "user"]
        );
        assert_eq!(normalized.message_index_map, vec![0, 2, 3, 4, 6, 7, 8]);
        assert_eq!(
            normalized
                .request
                .system
                .as_ref()
                .expect("system messages should exist")
                .iter()
                .map(|message| message.text.as_str())
                .collect::<Vec<_>>(),
            vec!["Top level system.", "SessionStart hook additional context: stable skill body",]
        );
        assert_eq!(normalized.request.messages[2].content, serde_json::json!("volatile interrupt"));
        assert_eq!(
            normalized.request.messages[5].content,
            serde_json::json!(
                "<system_context>\nProject-local rule that should stay in \
                 order.\n</system_context>"
            )
        );
        assert!(normalized.normalization_events.iter().any(|event| {
            event.message_index == 1
                && event.role == "system"
                && event.action == "promote_message"
                && event.reason == "stable_system_role_promoted_to_top_level"
        }));
        assert!(normalized.normalization_events.iter().any(|event| {
            event.message_index == 3
                && event.role == "system"
                && event.action == "convert_message"
                && event.reason == "interrupted_user_message_preserved_as_user_context"
        }));
        assert!(normalized.normalization_events.iter().any(|event| {
            event.message_index == 5
                && event.role == "system"
                && event.action == "drop_message"
                && event.reason == "dynamic_system_noise_for_cache_stability"
        }));
        assert!(normalized.normalization_events.iter().any(|event| {
            event.message_index == 7
                && event.role == "system"
                && event.action == "convert_message"
                && event.reason == "system_role_preserved_in_order_as_user_context"
        }));

        let result = convert_request(&req).expect("conversion should succeed");
        let system_prefix = match &result.conversation_state.history[0] {
            Message::User(message) => &message.user_input_message.content,
            other => panic!("expected injected system user message, got {other:?}"),
        };
        assert!(system_prefix.contains("Top level system."));
        assert!(system_prefix.contains("SessionStart hook additional context: stable skill body"));
        assert!(!system_prefix.contains("volatile interrupt"));
        assert!(!system_prefix.contains("The user sent a new message while you were working"));
        assert!(!system_prefix.contains("task tools haven't been used recently"));
        assert!(!system_prefix.contains("Project-local rule that should stay in order."));

        let current = &result
            .conversation_state
            .current_message
            .user_input_message
            .content;
        assert!(current.contains("<system_context>"));
        assert!(current.contains("Project-local rule that should stay in order."));
        assert!(current.contains("third question"));
        assert!(!current.contains("task tools haven't been used recently"));
    }

    #[test]
    fn normalize_request_preserves_trailing_unknown_system_role_as_current_user_context() {
        let req = base_request(vec![
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("current request"),
            },
            AnthropicMessage {
                role: "system".to_string(),
                content: serde_json::json!("Per-turn constraint that follows the final user."),
            },
        ]);

        let normalized = normalize_request(&req).expect("normalization should succeed");
        assert_eq!(
            normalized
                .request
                .messages
                .iter()
                .map(|message| message.role.as_str())
                .collect::<Vec<_>>(),
            vec!["user", "user"]
        );
        assert_eq!(normalized.message_index_map, vec![0, 1]);

        let result = convert_request(&req).expect("conversion should succeed");
        let current = &result
            .conversation_state
            .current_message
            .user_input_message
            .content;
        assert!(current.contains("current request"));
        assert!(current.contains("Per-turn constraint that follows the final user."));
    }

    #[test]
    fn normalize_request_preserves_interrupted_user_message_payload() {
        let req = base_request(vec![
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("previous request"),
            },
            AnthropicMessage {
                role: "assistant".to_string(),
                content: serde_json::json!("working on previous request"),
            },
            AnthropicMessage {
                role: "system".to_string(),
                content: serde_json::json!(
                    "The user sent a new message while you were working:\nwrite the report from \
                     the Word and PPT files\n\nIMPORTANT: After completing your current task, you \
                     MUST address the user's message above. Do not ignore it."
                ),
            },
        ]);

        let normalized = normalize_request(&req).expect("normalization should succeed");
        assert_eq!(
            normalized
                .request
                .messages
                .iter()
                .map(|message| message.role.as_str())
                .collect::<Vec<_>>(),
            vec!["user", "assistant", "user"]
        );
        assert_eq!(
            normalized.request.messages[2].content,
            serde_json::json!("write the report from the Word and PPT files")
        );

        let result = convert_request(&req).expect("conversion should succeed");
        let current = &result
            .conversation_state
            .current_message
            .user_input_message
            .content;
        assert_eq!(current, "write the report from the Word and PPT files");
    }

    #[test]
    fn convert_request_still_rejects_current_empty_user_message() {
        let req = base_request(vec![
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("解释一下 Kiro API"),
            },
            AnthropicMessage {
                role: "assistant".to_string(),
                content: serde_json::json!("Kiro API 是一组兼容接口。"),
            },
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!(""),
            },
        ]);

        let err = convert_request(&req).expect_err("current empty user should reject");
        assert_eq!(err.to_string(), "message 2 content must not be empty");
    }

    #[test]
    fn normalize_request_fills_empty_tool_description_with_stable_placeholder() {
        let mut req = base_request(vec![AnthropicMessage {
            role: "user".to_string(),
            content: serde_json::json!("Hello"),
        }]);
        req.tools = Some(vec![AnthropicTool {
            tool_type: None,
            name: "demo_tool".to_string(),
            description: "".to_string(),
            input_schema: HashMap::from([
                ("type".to_string(), serde_json::json!("object")),
                ("properties".to_string(), serde_json::json!({})),
                ("required".to_string(), serde_json::json!([])),
                ("additionalProperties".to_string(), serde_json::json!(true)),
            ]),
            max_uses: None,
        }]);

        let normalized = normalize_request(&req).expect("normalization should succeed");
        let tool = normalized
            .request
            .tools
            .as_ref()
            .and_then(|tools| tools.first())
            .expect("tool should exist after normalization");

        assert_eq!(tool.description, "Client-provided tool 'demo_tool'");
    }

    #[test]
    fn convert_request_rejects_tool_with_empty_name() {
        let mut req = base_request(vec![AnthropicMessage {
            role: "user".to_string(),
            content: serde_json::json!("Hello"),
        }]);
        req.tools = Some(vec![AnthropicTool {
            tool_type: None,
            name: "   ".to_string(),
            description: "demo".to_string(),
            input_schema: HashMap::from([
                ("type".to_string(), serde_json::json!("object")),
                ("properties".to_string(), serde_json::json!({})),
                ("required".to_string(), serde_json::json!([])),
                ("additionalProperties".to_string(), serde_json::json!(true)),
            ]),
            max_uses: None,
        }]);

        let err = convert_request(&req).expect_err("empty tool name should be rejected");
        let message = err.to_string();
        assert!(message.contains("tool 0 has empty name"));
    }

    #[test]
    fn convert_request_keeps_anyof_tool_schema_intact() {
        let mut req = base_request(vec![AnthropicMessage {
            role: "user".to_string(),
            content: serde_json::json!("Hello"),
        }]);
        req.tools = Some(vec![AnthropicTool {
            tool_type: None,
            name: "convert_number".to_string(),
            description: "Convert a number".to_string(),
            input_schema: HashMap::from([
                ("type".to_string(), serde_json::json!("object")),
                (
                    "properties".to_string(),
                    serde_json::json!({
                        "size": {
                            "anyOf": [{"type": "integer"}, {"type": "null"}]
                        }
                    }),
                ),
                ("required".to_string(), serde_json::json!([])),
                ("additionalProperties".to_string(), serde_json::json!(true)),
            ]),
            max_uses: None,
        }]);

        let result = convert_request(&req).expect("anyOf schema should remain allowed");
        assert_eq!(
            result
                .conversation_state
                .current_message
                .user_input_message
                .user_input_message_context
                .tools
                .len(),
            1
        );
        let schema = &result
            .conversation_state
            .current_message
            .user_input_message
            .user_input_message_context
            .tools[0]
            .tool_specification
            .input_schema
            .json;
        assert_eq!(
            schema["properties"]["size"]["anyOf"],
            serde_json::json!([{ "type": "integer" }, { "type": "null" }])
        );
    }

    #[test]
    fn convert_request_rewrites_anyof_tool_schema_for_current_image_turn() {
        let mut req = base_request(vec![AnthropicMessage {
            role: "user".to_string(),
            content: serde_json::json!([
                {
                    "type": "text",
                    "text": "Describe this image"
                },
                {
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": "image/png",
                        "data": "aGVsbG8="
                    }
                }
            ]),
        }]);
        req.tools = Some(vec![AnthropicTool {
            tool_type: None,
            name: "convert_number".to_string(),
            description: "Convert a number".to_string(),
            input_schema: HashMap::from([
                ("type".to_string(), serde_json::json!("object")),
                (
                    "properties".to_string(),
                    serde_json::json!({
                        "size": {
                            "anyOf": [{"type": "integer"}, {"type": "null"}]
                        }
                    }),
                ),
                ("required".to_string(), serde_json::json!([])),
                ("additionalProperties".to_string(), serde_json::json!(true)),
            ]),
            max_uses: None,
        }]);

        let result = convert_request(&req).expect("image request should still convert");
        let schema = &result
            .conversation_state
            .current_message
            .user_input_message
            .user_input_message_context
            .tools[0]
            .tool_specification
            .input_schema
            .json;
        assert_eq!(
            schema,
            &serde_json::json!({
                "type": "object",
                "properties": {},
                "required": [],
                "additionalProperties": true
            })
        );
    }

    #[test]
    fn convert_request_rewrites_anyof_tool_schema_for_history_image_turn() {
        let mut req = base_request(vec![
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!([
                    {
                        "type": "text",
                        "text": "Describe this image"
                    },
                    {
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": "image/png",
                            "data": "aGVsbG8="
                        }
                    }
                ]),
            },
            AnthropicMessage {
                role: "assistant".to_string(),
                content: serde_json::json!("I can help"),
            },
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("继续"),
            },
        ]);
        req.tools = Some(vec![AnthropicTool {
            tool_type: None,
            name: "convert_number".to_string(),
            description: "Convert a number".to_string(),
            input_schema: HashMap::from([
                ("type".to_string(), serde_json::json!("object")),
                (
                    "properties".to_string(),
                    serde_json::json!({
                        "size": {
                            "anyOf": [{"type": "integer"}, {"type": "null"}]
                        }
                    }),
                ),
                ("required".to_string(), serde_json::json!([])),
                ("additionalProperties".to_string(), serde_json::json!(true)),
            ]),
            max_uses: None,
        }]);

        let result = convert_request(&req).expect("history image request should still convert");
        let schema = &result
            .conversation_state
            .current_message
            .user_input_message
            .user_input_message_context
            .tools[0]
            .tool_specification
            .input_schema
            .json;
        assert_eq!(
            schema,
            &serde_json::json!({
                "type": "object",
                "properties": {},
                "required": [],
                "additionalProperties": true
            })
        );
    }

    #[test]
    fn convert_request_adds_placeholder_tools_for_history_usage() {
        let mut req = base_request(vec![
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("Read the file"),
            },
            AnthropicMessage {
                role: "assistant".to_string(),
                content: serde_json::json!([
                    {"type": "text", "text": "I'll read the file."},
                    {"type": "tool_use", "id": "tool-1", "name": "read", "input": {"path": "/tmp/test.txt"}}
                ]),
            },
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!([
                    {"type": "tool_result", "tool_use_id": "tool-1", "content": "file content"}
                ]),
            },
        ]);
        req.tools = Some(vec![AnthropicTool {
            tool_type: None,
            name: "write".to_string(),
            description: "Write file".to_string(),
            input_schema: HashMap::new(),
            max_uses: None,
        }]);

        let result = convert_request(&req).expect("conversion should succeed");
        let tools = &result
            .conversation_state
            .current_message
            .user_input_message
            .user_input_message_context
            .tools;

        assert!(tools
            .iter()
            .any(|tool| tool.tool_specification.name == "read"));
        assert!(tools
            .iter()
            .any(|tool| tool.tool_specification.name == "write"));
    }

    #[test]
    fn convert_request_maps_long_tool_names_in_tools_and_history() {
        let long_name =
            "tool_name_that_is_far_too_long_for_kiro_and_must_be_shortened_consistently_12345";
        let mut req = base_request(vec![
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("Use the tool"),
            },
            AnthropicMessage {
                role: "assistant".to_string(),
                content: serde_json::json!([
                    {"type": "tool_use", "id": "tool-1", "name": long_name, "input": {"path": "/tmp/test.txt"}}
                ]),
            },
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!([
                    {"type": "tool_result", "tool_use_id": "tool-1", "content": "ok"}
                ]),
            },
        ]);
        req.tools = Some(vec![AnthropicTool {
            tool_type: None,
            name: long_name.to_string(),
            description: "Long tool".to_string(),
            input_schema: HashMap::new(),
            max_uses: None,
        }]);

        let result = convert_request(&req).expect("conversion should succeed");
        assert_eq!(result.tool_name_map.len(), 1);
        let (short_name, original_name) = result
            .tool_name_map
            .iter()
            .next()
            .expect("normalized tool name should be recorded");
        assert_eq!(original_name, long_name);
        assert!(short_name.len() <= TOOL_NAME_MAX_LEN);

        let tools = &result
            .conversation_state
            .current_message
            .user_input_message
            .user_input_message_context
            .tools;
        assert!(tools
            .iter()
            .any(|tool| tool.tool_specification.name == *short_name));

        let history_tool_name = match &semantic_history(&result)[1] {
            Message::Assistant(message) => message
                .assistant_response_message
                .tool_uses
                .as_ref()
                .and_then(|tool_uses| tool_uses.first())
                .map(|entry| entry.name.as_str())
                .expect("history tool use should exist"),
            other => panic!("expected assistant history entry, got {other:?}"),
        };
        assert_eq!(history_tool_name, short_name);
    }

    #[test]
    fn convert_request_normalizes_unsupported_tool_name_characters_consistently() {
        let original_name = "termux_exec:run_command";
        let req = base_request(vec![
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("Run the command"),
            },
            AnthropicMessage {
                role: "assistant".to_string(),
                content: serde_json::json!([
                    {"type": "tool_use", "id": "tool-1", "name": original_name, "input": {"command": "pwd"}}
                ]),
            },
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!([
                    {"type": "tool_result", "tool_use_id": "tool-1", "content": "ok"}
                ]),
            },
        ]);

        let result = convert_request(&req).expect("conversion should succeed");
        let (mapped_name, original) = result
            .tool_name_map
            .iter()
            .next()
            .expect("normalized tool name should be recorded");
        assert_eq!(original, original_name);
        assert!(!mapped_name.contains(':'));

        let history_tool_name = match &semantic_history(&result)[1] {
            Message::Assistant(message) => message
                .assistant_response_message
                .tool_uses
                .as_ref()
                .and_then(|tool_uses| tool_uses.first())
                .map(|entry| entry.name.as_str())
                .expect("history tool use should exist"),
            other => panic!("expected assistant history entry, got {other:?}"),
        };
        assert_eq!(history_tool_name, mapped_name);

        let tools = &result
            .conversation_state
            .current_message
            .user_input_message
            .user_input_message_context
            .tools;
        assert!(tools
            .iter()
            .any(|tool| tool.tool_specification.name == *mapped_name));
    }

    #[test]
    fn convert_request_normalizes_placeholder_history_tool_names() {
        let original_name = "termux_exec:run_command";
        let mut req = base_request(vec![
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("Run the command"),
            },
            AnthropicMessage {
                role: "assistant".to_string(),
                content: serde_json::json!([
                    {"type": "tool_use", "id": "tool-1", "name": original_name, "input": {"command": "pwd"}}
                ]),
            },
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!([
                    {"type": "tool_result", "tool_use_id": "tool-1", "content": "ok"}
                ]),
            },
        ]);
        req.tools = Some(vec![AnthropicTool {
            tool_type: None,
            name: "read_file".to_string(),
            description: "Read file".to_string(),
            input_schema: HashMap::new(),
            max_uses: None,
        }]);

        let result = convert_request(&req).expect("conversion should succeed");
        let mapped_name = result
            .tool_name_map
            .iter()
            .find_map(|(mapped, original)| (original == original_name).then_some(mapped.as_str()))
            .expect("normalized tool name should be tracked");
        assert!(!mapped_name.contains(':'));

        let tools = &result
            .conversation_state
            .current_message
            .user_input_message
            .user_input_message_context
            .tools;
        assert!(tools
            .iter()
            .any(|tool| tool.tool_specification.name == mapped_name));

        let history_tool_name = match &semantic_history(&result)[1] {
            Message::Assistant(message) => message
                .assistant_response_message
                .tool_uses
                .as_ref()
                .and_then(|tool_uses| tool_uses.first())
                .map(|entry| entry.name.as_str())
                .expect("history tool use should exist"),
            other => panic!("expected assistant history entry, got {other:?}"),
        };
        assert_eq!(history_tool_name, mapped_name);
    }

    #[test]
    fn convert_request_injects_enabled_thinking_budget_prefix_into_current_turn() {
        let mut req = base_request(vec![AnthropicMessage {
            role: "user".to_string(),
            content: serde_json::json!("Hello"),
        }]);
        req.thinking = Some(crate::anthropic::types::Thinking {
            thinking_type: "enabled".to_string(),
            display: None,
            budget_tokens: 4096,
        });

        let result = convert_request(&req).expect("conversion should succeed");
        let current = &result
            .conversation_state
            .current_message
            .user_input_message
            .content;

        assert!(semantic_history(&result).is_empty());
        assert!(current.contains("<thinking_mode>enabled</thinking_mode>"));
        assert!(current.contains("<max_thinking_length>4096</max_thinking_length>"));
        assert!(current.contains("Hello"));
    }

    #[test]
    fn preserves_thinking_effort_on_current_turn_when_output_config_is_supplied() {
        let mut req = base_request(vec![AnthropicMessage {
            role: "user".to_string(),
            content: serde_json::json!("Hello"),
        }]);
        req.thinking = Some(crate::anthropic::types::Thinking {
            thinking_type: "adaptive".to_string(),
            display: None,
            budget_tokens: 20_000,
        });
        req.output_config = Some(crate::anthropic::types::OutputConfig {
            effort: Some("medium".to_string()),
            format: None,
        });

        let result = convert_request(&req).expect("conversion should succeed");
        let current = &result
            .conversation_state
            .current_message
            .user_input_message
            .content;

        assert!(semantic_history(&result).is_empty());
        assert!(current.contains("<thinking_mode>adaptive</thinking_mode>"));
        assert!(current.contains("<thinking_effort>medium</thinking_effort>"));
        assert!(current.contains("Hello"));
    }

    #[test]
    fn convert_request_defaults_adaptive_thinking_effort_to_xhigh_on_current_turn() {
        let mut req = base_request(vec![AnthropicMessage {
            role: "user".to_string(),
            content: serde_json::json!("Hello"),
        }]);
        req.thinking = Some(crate::anthropic::types::Thinking {
            thinking_type: "adaptive".to_string(),
            display: None,
            budget_tokens: 20_000,
        });

        let result = convert_request(&req).expect("conversion should succeed");
        let current = &result
            .conversation_state
            .current_message
            .user_input_message
            .content;

        assert!(semantic_history(&result).is_empty());
        assert!(current.contains("<thinking_effort>xhigh</thinking_effort>"));
        assert!(current.contains("Hello"));
    }

    #[test]
    fn convert_request_keeps_thinking_model_dynamic_tags_out_of_system_prefix() {
        let mut req = base_request(vec![AnthropicMessage {
            role: "user".to_string(),
            content: serde_json::json!("Hello"),
        }]);
        req.model = "claude-opus-4-8-thinking".to_string();
        req.system = Some(vec![SystemMessage {
            text: "You are Claude Code, Anthropic's official CLI for Claude.".to_string(),
        }]);
        req.thinking = Some(crate::anthropic::types::Thinking {
            thinking_type: "adaptive".to_string(),
            display: None,
            budget_tokens: 20_000,
        });
        req.output_config = Some(crate::anthropic::types::OutputConfig {
            effort: Some("xhigh".to_string()),
            format: None,
        });

        let result = convert_request(&req).expect("conversion should succeed");
        let system_prefix = match &result.conversation_state.history[0] {
            Message::User(message) => &message.user_input_message.content,
            other => panic!("expected injected system user message, got {other:?}"),
        };
        let current = &result
            .conversation_state
            .current_message
            .user_input_message
            .content;

        assert!(current.contains("<thinking_effort>xhigh</thinking_effort>"));
        assert!(!system_prefix.contains("<thinking_effort>xhigh</thinking_effort>"));
        assert!(system_prefix.contains(
            "You are powered by the model named Opus 4.8. The exact model ID is claude-opus-4-8."
        ));
        assert!(!system_prefix.contains("claude-opus-4-8-thinking"));
    }

    #[test]
    fn convert_request_does_not_send_random_agent_continuation_metadata_by_default() {
        let req = base_request(vec![AnthropicMessage {
            role: "user".to_string(),
            content: serde_json::json!("Hello"),
        }]);

        let result = convert_request(&req).expect("conversion should succeed");

        assert_eq!(result.conversation_state.chat_trigger_type.as_deref(), Some("MANUAL"));
        assert!(result.conversation_state.agent_continuation_id.is_none());
        assert!(result.conversation_state.agent_task_type.is_none());
    }

    #[test]
    fn convert_request_normalizes_claude_code_model_identity_to_requested_model() {
        let mut req = base_request(vec![AnthropicMessage {
            role: "user".to_string(),
            content: serde_json::json!("Hello"),
        }]);
        req.model = "claude-opus-4-6".to_string();
        req.system = Some(vec![
            SystemMessage {
                text: "You are Claude Code, Anthropic's official CLI for Claude.".to_string(),
            },
            SystemMessage {
                text: "You are powered by the model named Sonnet 4.6. The exact model ID is \
                       claude-sonnet-4-6."
                    .to_string(),
            },
        ]);

        let result = convert_request(&req).expect("conversion should succeed");
        let system_prefix = match &result.conversation_state.history[0] {
            Message::User(message) => &message.user_input_message.content,
            other => panic!("expected injected system user message, got {other:?}"),
        };

        assert!(system_prefix.contains(
            "You are powered by the model named Opus 4.6. The exact model ID is claude-opus-4-6."
        ));
        assert!(!system_prefix.contains("claude-sonnet-4-6"));
    }

    #[test]
    fn convert_request_injects_missing_claude_code_model_identity() {
        let mut req = base_request(vec![AnthropicMessage {
            role: "user".to_string(),
            content: serde_json::json!("Hello"),
        }]);
        req.model = "claude-opus-4-8".to_string();
        req.system = Some(vec![SystemMessage {
            text: "You are Claude Code, Anthropic's official CLI for Claude.".to_string(),
        }]);

        let result = convert_request(&req).expect("conversion should succeed");
        let system_prefix = match &result.conversation_state.history[0] {
            Message::User(message) => &message.user_input_message.content,
            other => panic!("expected injected system user message, got {other:?}"),
        };

        assert!(system_prefix.contains("You are Claude Code, Anthropic's official CLI"));
        assert!(system_prefix.contains(
            "You are powered by the model named Opus 4.8. The exact model ID is claude-opus-4-8."
        ));
    }

    #[test]
    fn convert_request_marks_model_identity_probe_for_response_normalization() {
        let mut req = base_request(vec![AnthropicMessage {
            role: "user".to_string(),
            content: serde_json::json!("请只回答你的模型名称和模型ID，不要解释。"),
        }]);
        req.model = "claude-opus-4-7".to_string();

        let result = convert_request(&req).expect("conversion should succeed");

        assert_eq!(
            result
                .response_identity
                .as_ref()
                .map(|identity| identity.model_name.as_str()),
            Some("Claude Opus 4.7")
        );
        assert_eq!(
            result
                .response_identity
                .as_ref()
                .map(|identity| identity.model_id.as_str()),
            Some("claude-opus-4-7")
        );
        let system_prefix = match &result.conversation_state.history[0] {
            Message::User(message) => &message.user_input_message.content,
            other => panic!("expected injected identity user message, got {other:?}"),
        };
        assert!(system_prefix.contains("your model name is Claude Opus 4.7"));
        assert!(system_prefix.contains("your public API model ID is claude-opus-4-7"));
    }

    #[test]
    fn convert_request_injects_anthropic_identity_when_system_is_absent() {
        let req = base_request(vec![AnthropicMessage {
            role: "user".to_string(),
            content: serde_json::json!("Who are you?"),
        }]);

        let result = convert_request(&req).expect("conversion should succeed");
        let system_prefix = match &result.conversation_state.history[0] {
            Message::User(message) => &message.user_input_message.content,
            other => panic!("expected injected identity user message, got {other:?}"),
        };

        assert!(system_prefix.contains("You are Claude, made by Anthropic."));
        assert!(system_prefix.contains("Never claim to be Kiro"));
    }

    #[test]
    fn convert_request_appends_anthropic_identity_to_client_system_prompt() {
        let mut req = base_request(vec![AnthropicMessage {
            role: "user".to_string(),
            content: serde_json::json!("Who are you?"),
        }]);
        req.system = Some(vec![SystemMessage {
            text: "Answer concisely.".to_string(),
        }]);

        let result = convert_request(&req).expect("conversion should succeed");
        let system_prefix = match &result.conversation_state.history[0] {
            Message::User(message) => &message.user_input_message.content,
            other => panic!("expected injected system user message, got {other:?}"),
        };

        assert!(system_prefix.contains("Answer concisely."));
        assert!(system_prefix.contains("You are Claude, made by Anthropic."));
        assert!(system_prefix.contains("Never claim to be Kiro"));
    }

    #[test]
    fn convert_request_strips_volatile_claude_code_billing_header_before_upstream() {
        let mut req = base_request(vec![AnthropicMessage {
            role: "user".to_string(),
            content: serde_json::json!("Hello"),
        }]);
        req.model = "claude-opus-4-6".to_string();
        req.thinking = Some(crate::anthropic::types::Thinking {
            thinking_type: "adaptive".to_string(),
            display: None,
            budget_tokens: 20_000,
        });
        req.output_config = Some(crate::anthropic::types::OutputConfig {
            effort: Some("high".to_string()),
            format: None,
        });
        req.system = Some(vec![SystemMessage {
            text: concat!(
                "你是 Claude Opus 4.7，知识库截至时间 2026-01。\n",
                "x-anthropic-billing-header: cc_version=2.1.123.074; ",
                "cc_entrypoint=cli; cch=ea527;\n",
                "You are Claude Code, Anthropic's official CLI for Claude."
            )
            .to_string(),
        }]);

        let result = convert_request(&req).expect("conversion should succeed");
        let system_prefix = match &result.conversation_state.history[0] {
            Message::User(message) => &message.user_input_message.content,
            other => panic!("expected injected system user message, got {other:?}"),
        };
        let current = &result
            .conversation_state
            .current_message
            .user_input_message
            .content;

        assert!(current.contains("<thinking_effort>high</thinking_effort>"));
        assert!(!system_prefix.contains("<thinking_effort>high</thinking_effort>"));
        assert!(system_prefix.contains("你是 Claude Opus 4.7，知识库截至时间 2026-01。"));
        assert!(system_prefix.contains("You are Claude Code, Anthropic's official CLI for Claude."));
        assert!(!system_prefix.contains("x-anthropic-billing-header:"));
        assert!(!system_prefix.contains("cch=ea527"));
    }

    #[test]
    fn convert_request_strips_legacy_claude_code_billing_header_at_system_start() {
        let mut req = base_request(vec![AnthropicMessage {
            role: "user".to_string(),
            content: serde_json::json!("Hello"),
        }]);
        req.system = Some(vec![SystemMessage {
            text: concat!(
                "x-anthropic-billing-header: cc_version=2.1.114.069; ",
                "cc_entrypoint=cli; cch=638d8;\n",
                "You are Claude Code, Anthropic's official CLI for Claude.\n",
                "You are an interactive agent that helps users with software engineering tasks."
            )
            .to_string(),
        }]);

        let result = convert_request(&req).expect("conversion should succeed");
        let system_prefix = match &result.conversation_state.history[0] {
            Message::User(message) => &message.user_input_message.content,
            other => panic!("expected injected system user message, got {other:?}"),
        };

        assert!(system_prefix.starts_with("You are Claude Code, Anthropic's official CLI"));
        assert!(!system_prefix.contains("x-anthropic-billing-header:"));
        assert!(!system_prefix.contains("cch=638d8"));
    }

    #[test]
    fn convert_request_strips_billing_header_block_with_leading_whitespace() {
        let mut req = base_request(vec![AnthropicMessage {
            role: "user".to_string(),
            content: serde_json::json!("Hello"),
        }]);
        req.system = Some(vec![
            SystemMessage {
                text: "  x-anthropic-billing-header: cc_version=2.1.130.abc; cc_entrypoint=cli; \
                       cch=11111;"
                    .to_string(),
            },
            SystemMessage {
                text: "You are Claude Code, Anthropic's official CLI for Claude.".to_string(),
            },
            SystemMessage {
                text: "Project prompt".to_string(),
            },
        ]);

        let result = convert_request(&req).expect("conversion should succeed");
        let system_prefix = match &result.conversation_state.history[0] {
            Message::User(message) => &message.user_input_message.content,
            other => panic!("expected injected system user message, got {other:?}"),
        };

        assert!(system_prefix.starts_with("You are Claude Code, Anthropic's official CLI"));
        assert!(system_prefix.contains("Project prompt"));
        assert!(!system_prefix.contains("x-anthropic-billing-header:"));
        assert!(!system_prefix.contains("cch=11111"));
    }

    #[test]
    fn convert_request_strips_agent_sdk_billing_header_after_existing_thinking_tags() {
        let mut req = base_request(vec![AnthropicMessage {
            role: "user".to_string(),
            content: serde_json::json!("Hello"),
        }]);
        req.system = Some(vec![SystemMessage {
            text: concat!(
                "<thinking_mode>adaptive</thinking_mode>",
                "<thinking_effort>max</thinking_effort>\n",
                "x-anthropic-billing-header: cc_version=2.1.114.eee; ",
                "cc_entrypoint=sdk-cli; cch=fb0be;\n",
                "You are a Claude agent, built on Anthropic's Claude Agent SDK.\n",
                "You are an interactive agent that helps users with software engineering tasks."
            )
            .to_string(),
        }]);

        let result = convert_request(&req).expect("conversion should succeed");
        let system_prefix = match &result.conversation_state.history[0] {
            Message::User(message) => &message.user_input_message.content,
            other => panic!("expected injected system user message, got {other:?}"),
        };

        assert!(system_prefix.contains("<thinking_effort>max</thinking_effort>"));
        assert!(system_prefix
            .contains("You are a Claude agent, built on Anthropic's Claude Agent SDK."));
        assert!(!system_prefix.contains("x-anthropic-billing-header:"));
        assert!(!system_prefix.contains("cch=fb0be"));
    }

    #[test]
    fn convert_request_preserves_billing_header_not_followed_by_claude_identity() {
        let mut req = base_request(vec![AnthropicMessage {
            role: "user".to_string(),
            content: serde_json::json!("Hello"),
        }]);
        req.system = Some(vec![SystemMessage {
            text: concat!(
                "x-anthropic-billing-header: this is user supplied text\n",
                "This is not a Claude Code identity block."
            )
            .to_string(),
        }]);

        let result = convert_request(&req).expect("conversion should succeed");
        let system_prefix = match &result.conversation_state.history[0] {
            Message::User(message) => &message.user_input_message.content,
            other => panic!("expected injected system user message, got {other:?}"),
        };

        assert!(system_prefix.contains("x-anthropic-billing-header: this is user supplied text"));
        assert!(system_prefix.contains("This is not a Claude Code identity block."));
    }

    #[test]
    fn convert_request_maps_json_schema_output_to_hidden_tool() {
        let mut req = base_request(vec![AnthropicMessage {
            role: "user".to_string(),
            content: serde_json::json!("计算 4 乘以 4 等于多少"),
        }]);
        req.output_config = Some(crate::anthropic::types::OutputConfig {
            effort: None,
            format: Some(crate::anthropic::types::OutputFormat {
                format_type: "json_schema".to_string(),
                schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "expression": { "type": "string" },
                        "result": { "type": "integer" }
                    },
                    "required": ["expression", "result"],
                    "additionalProperties": false
                })),
            }),
        });

        let result = convert_request(&req).expect("conversion should succeed");
        let tool_name = result
            .structured_output_tool_name
            .as_deref()
            .expect("structured output tool should be injected");
        let current = &result.conversation_state.current_message.user_input_message;
        let tools = &current.user_input_message_context.tools;
        assert!(tools
            .iter()
            .any(|tool| tool.tool_specification.name == tool_name
                && tool.tool_specification.input_schema.json["required"]
                    == serde_json::json!(["expression", "result"])));
        let system_prefix = match &result.conversation_state.history[0] {
            Message::User(message) => &message.user_input_message.content,
            other => panic!("expected injected system user message, got {other:?}"),
        };
        assert!(system_prefix.contains(tool_name));
        assert!(system_prefix.contains("Return the final answer by calling"));
    }

    #[test]
    fn convert_request_rejects_last_user_message_without_supported_content() {
        let req = base_request(vec![AnthropicMessage {
            role: "user".to_string(),
            content: serde_json::json!([
                {
                    "type": "image"
                }
            ]),
        }]);

        assert!(convert_request(&req).is_err());
    }

    #[test]
    fn rejects_messages_that_become_empty_after_filtering() {
        let req = base_request(vec![AnthropicMessage {
            role: "user".to_string(),
            content: serde_json::json!([
                {"type": "text", "text": " \n\t"},
                {"type": "thinking", "thinking": "  "}
            ]),
        }]);

        let err = convert_request(&req).expect_err("empty normalized current turn should reject");
        match err {
            ConversionError::InvalidRequest(message) => {
                assert!(!message.is_empty());
            },
            other => panic!("expected invalid_request_error-equivalent failure, got {other:?}"),
        }
    }

    #[test]
    fn convert_request_rejects_unknown_message_role() {
        let req = base_request(vec![AnthropicMessage {
            role: "tool".to_string(),
            content: serde_json::json!("tool output"),
        }]);

        assert!(convert_request(&req).is_err());
    }

    #[test]
    fn convert_request_accepts_supported_user_text_and_image_blocks() {
        let req = base_request(vec![AnthropicMessage {
            role: "user".to_string(),
            content: serde_json::json!([
                {
                    "type": "text",
                    "text": "Describe this image"
                },
                {
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": "image/png",
                        "data": "aGVsbG8="
                    }
                }
            ]),
        }]);

        let result = convert_request(&req).expect("supported user content should pass");
        let current = &result.conversation_state.current_message.user_input_message;
        assert_eq!(current.content, "Describe this image");
        assert_eq!(current.images.len(), 1);
        assert_eq!(current.images[0].format, "png");
        assert_eq!(current.origin.as_deref(), Some("AI_EDITOR"));
    }

    #[test]
    fn convert_request_preserves_pdf_documents_as_attachments() {
        let req = base_request(vec![AnthropicMessage {
            role: "user".to_string(),
            content: serde_json::json!([
                {
                    "type": "document",
                    "name": "report.pdf",
                    "source": {
                        "type": "base64",
                        "media_type": "application/pdf",
                        "data": SAMPLE_PDF_BASE64
                    }
                },
                {
                    "type": "text",
                    "text": "What text does this PDF contain?"
                }
            ]),
        }]);

        let result = convert_request(&req).expect("pdf document block should remain supported");
        let current =
            serde_json::to_value(&result.conversation_state.current_message.user_input_message)
                .expect("serialize current message");
        assert_eq!(current["content"], "What text does this PDF contain?");
        assert_eq!(current["documents"].as_array().map(Vec::len), Some(1));
        assert_eq!(current["documents"][0]["name"], "report");
        assert_eq!(current["documents"][0]["format"], "pdf");
        assert_eq!(current["documents"][0]["source"]["bytes"], SAMPLE_PDF_BASE64);
    }

    #[test]
    fn convert_request_keeps_pdf_documents_as_document_attachments() {
        let req = base_request(vec![AnthropicMessage {
            role: "user".to_string(),
            content: serde_json::json!([
                {
                    "type": "document",
                    "name": "report.pdf",
                    "source": {
                        "type": "base64",
                        "media_type": "application/pdf",
                        "data": SAMPLE_PDF_BASE64
                    }
                },
                {
                    "type": "text",
                    "text": "What text does this PDF contain?"
                }
            ]),
        }]);

        let result = convert_request(&req).expect("pdf document block should remain supported");
        let current =
            serde_json::to_value(&result.conversation_state.current_message.user_input_message)
                .expect("serialize current message");

        assert_eq!(current["content"], "What text does this PDF contain?");
        assert_eq!(current["documents"].as_array().map(Vec::len), Some(1));
        assert_eq!(current["documents"][0]["name"], "report");
        assert_eq!(current["documents"][0]["format"], "pdf");
        assert_eq!(current["documents"][0]["source"]["bytes"], SAMPLE_PDF_BASE64);
        assert!(!current["content"]
            .as_str()
            .expect("content string")
            .contains("PDF extracted text:"));
    }

    #[test]
    fn convert_request_generates_name_for_document_without_name() {
        let req = base_request(vec![AnthropicMessage {
            role: "user".to_string(),
            content: serde_json::json!([
                {
                    "type": "document",
                    "source": {
                        "type": "base64",
                        "media_type": "application/pdf",
                        "data": SAMPLE_PDF_BASE64
                    }
                },
                {
                    "type": "text",
                    "text": "What text does this PDF contain?"
                }
            ]),
        }]);

        let result = convert_request(&req).expect("missing document name should be synthesized");
        let current =
            serde_json::to_value(&result.conversation_state.current_message.user_input_message)
                .expect("serialize current message");

        assert_eq!(current["documents"].as_array().map(Vec::len), Some(1));
        assert_eq!(
            current["documents"][0]["name"],
            generate_document_name("application/pdf", SAMPLE_PDF_BASE64)
        );
        assert_eq!(current["documents"][0]["format"], "pdf");
    }

    #[test]
    fn convert_request_preserves_text_documents_as_attachments() {
        let req = base_request(vec![AnthropicMessage {
            role: "user".to_string(),
            content: serde_json::json!([
                {
                    "type": "document",
                    "name": "plain.txt",
                    "source": {
                        "type": "text",
                        "media_type": "text/plain",
                        "data": "plain document body"
                    }
                },
                {
                    "type": "text",
                    "text": "Summarize the text document."
                }
            ]),
        }]);

        let result = convert_request(&req).expect("text document block should remain supported");
        let current =
            serde_json::to_value(&result.conversation_state.current_message.user_input_message)
                .expect("serialize current message");
        assert_eq!(current["content"], "Summarize the text document.");
        assert_eq!(current["documents"].as_array().map(Vec::len), Some(1));
        assert_eq!(current["documents"][0]["name"], "plain");
        assert_eq!(current["documents"][0]["format"], "txt");
        assert_eq!(current["documents"][0]["source"]["bytes"], "cGxhaW4gZG9jdW1lbnQgYm9keQ==");
    }

    #[test]
    fn convert_request_keeps_markdown_documents_as_document_attachments() {
        let req = base_request(vec![AnthropicMessage {
            role: "user".to_string(),
            content: serde_json::json!([
                {
                    "type": "document",
                    "name": "notes.md",
                    "source": {
                        "type": "text",
                        "media_type": "text/markdown",
                        "data": "# Heading\n\nbody"
                    }
                },
                {
                    "type": "text",
                    "text": "Summarize the markdown document."
                }
            ]),
        }]);

        let result =
            convert_request(&req).expect("markdown document block should remain supported");
        let current =
            serde_json::to_value(&result.conversation_state.current_message.user_input_message)
                .expect("serialize current message");

        assert_eq!(current["content"], "Summarize the markdown document.");
        assert_eq!(current["documents"].as_array().map(Vec::len), Some(1));
        assert_eq!(current["documents"][0]["name"], "notes");
        assert_eq!(current["documents"][0]["format"], "md");
        assert_eq!(current["documents"][0]["source"]["bytes"], "IyBIZWFkaW5nCgpib2R5");
        assert!(!current["content"]
            .as_str()
            .expect("content string")
            .contains("<document media_type="));
    }

    #[test]
    fn convert_request_preserves_document_only_history_turns() {
        let req = base_request(vec![
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!([
                    {
                        "type": "document",
                        "name": "report.pdf",
                        "source": {
                            "type": "base64",
                            "media_type": "application/pdf",
                            "data": SAMPLE_PDF_BASE64
                        }
                    }
                ]),
            },
            AnthropicMessage {
                role: "assistant".to_string(),
                content: serde_json::json!("I have the document."),
            },
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("Continue"),
            },
        ]);

        let result = convert_request(&req).expect("document-only history turn should survive");

        let history = semantic_history(&result);
        assert_eq!(history.len(), 2);
        let Message::User(history_user_message) = &history[0] else {
            panic!("expected first history message to be user");
        };
        let history_user = serde_json::to_value(&history_user_message.user_input_message)
            .expect("serialize history user");

        assert_eq!(history_user["content"], "(document attached)");
        assert_eq!(history_user["documents"].as_array().map(Vec::len), Some(1));
        assert_eq!(history_user["documents"][0]["name"], "report");
        assert_eq!(history_user["documents"][0]["format"], "pdf");
    }

    #[test]
    fn convert_request_dedupes_document_names_across_history_and_current_turn() {
        let req = base_request(vec![
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!([
                    {
                        "type": "document",
                        "name": "notes.md",
                        "source": {
                            "type": "text",
                            "media_type": "text/markdown",
                            "data": "# History"
                        }
                    },
                    {
                        "type": "text",
                        "text": "Keep this document in history."
                    }
                ]),
            },
            AnthropicMessage {
                role: "assistant".to_string(),
                content: serde_json::json!("acknowledged"),
            },
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!([
                    {
                        "type": "document",
                        "name": "notes.md",
                        "source": {
                            "type": "text",
                            "media_type": "text/markdown",
                            "data": "# Duplicate"
                        }
                    },
                    {
                        "type": "document",
                        "name": "report.pdf",
                        "source": {
                            "type": "base64",
                            "media_type": "application/pdf",
                            "data": SAMPLE_PDF_BASE64
                        }
                    },
                    {
                        "type": "text",
                        "text": "Summarize the surviving attachments."
                    }
                ]),
            },
        ]);

        let result = convert_request(&req).expect("duplicate documents should be deduped");
        let current =
            serde_json::to_value(&result.conversation_state.current_message.user_input_message)
                .expect("serialize current message");
        let Message::User(history_user_message) = &semantic_history(&result)[0] else {
            panic!("expected first history message to be user");
        };
        let history_user = serde_json::to_value(&history_user_message.user_input_message)
            .expect("serialize history user");

        assert_eq!(history_user["documents"].as_array().map(Vec::len), Some(1));
        assert_eq!(history_user["documents"][0]["name"], "notes");
        assert_eq!(current["documents"].as_array().map(Vec::len), Some(1));
        assert_eq!(current["documents"][0]["name"], "report");
    }

    #[test]
    fn convert_request_preserves_images_from_history_turns() {
        let req = base_request(vec![
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!([
                    {
                        "type": "text",
                        "text": "Describe this image"
                    },
                    {
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": "image/png",
                            "data": "aGVsbG8="
                        }
                    }
                ]),
            },
            AnthropicMessage {
                role: "assistant".to_string(),
                content: serde_json::json!("I can help"),
            },
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("继续"),
            },
        ]);

        let result = convert_request(&req).expect("history image request should still convert");
        assert!(result.has_history_images);
        let history_user = match &semantic_history(&result)[0] {
            Message::User(message) => &message.user_input_message,
            other => panic!("expected user history entry, got {other:?}"),
        };
        assert_eq!(history_user.content, "Describe this image");
        assert_eq!(history_user.images.len(), 1);
        assert_eq!(history_user.images[0].format, "png");
        assert_eq!(history_user.origin.as_deref(), Some("AI_EDITOR"));
    }

    #[test]
    fn convert_request_accepts_supported_tool_result_turn() {
        let req = base_request(vec![
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("Read the file"),
            },
            AnthropicMessage {
                role: "assistant".to_string(),
                content: serde_json::json!([
                    {
                        "type": "tool_use",
                        "id": "tool-1",
                        "name": "read_file",
                        "input": {"path": "/tmp/test.txt"}
                    }
                ]),
            },
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!([
                    {
                        "type": "tool_result",
                        "tool_use_id": "tool-1",
                        "content": "file content"
                    }
                ]),
            },
        ]);

        let result = convert_request(&req).expect("supported tool_result turn should pass");
        let current = &result.conversation_state.current_message.user_input_message;
        assert!(current.content.is_empty());
        assert_eq!(current.user_input_message_context.tool_results.len(), 1);
        assert_eq!(current.user_input_message_context.tool_results[0].tool_use_id, "tool-1");
    }

    #[test]
    fn convert_request_extracts_images_from_tool_result_content_into_current_turn() {
        let req = base_request(vec![
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("Read the screenshot"),
            },
            AnthropicMessage {
                role: "assistant".to_string(),
                content: serde_json::json!([
                    {
                        "type": "tool_use",
                        "id": "tool-1",
                        "name": "read_image",
                        "input": {"path": "/tmp/screenshot.png"}
                    }
                ]),
            },
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!([
                    {
                        "type": "tool_result",
                        "tool_use_id": "tool-1",
                        "content": [
                            {
                                "type": "image",
                                "source": {
                                    "type": "base64",
                                    "media_type": "image/png",
                                    "data": "aGVsbG8="
                                }
                            }
                        ]
                    }
                ]),
            },
        ]);

        let result =
            convert_request(&req).expect("tool_result image content should become current images");
        let current =
            serde_json::to_value(&result.conversation_state.current_message.user_input_message)
                .expect("serialize current message");

        assert_eq!(current["images"].as_array().map(Vec::len), Some(1));
        assert_eq!(current["images"][0]["format"], "png");
        assert_eq!(
            current["userInputMessageContext"]["toolResults"]
                .as_array()
                .map(Vec::len),
            Some(1)
        );
        assert_eq!(
            current["userInputMessageContext"]["toolResults"][0]["content"][0]["text"],
            "(empty result)"
        );
    }

    #[test]
    fn convert_request_extracts_images_from_stringified_tool_result_content() {
        let req = base_request(vec![
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("Read the screenshot"),
            },
            AnthropicMessage {
                role: "assistant".to_string(),
                content: serde_json::json!([
                    {
                        "type": "tool_use",
                        "id": "tool-1",
                        "name": "read_image",
                        "input": {"path": "/tmp/screenshot.png"}
                    }
                ]),
            },
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!([
                    {
                        "type": "tool_result",
                        "tool_use_id": "tool-1",
                        "content": serde_json::json!([
                            {
                                "type": "image",
                                "source": {
                                    "type": "base64",
                                    "media_type": "image/png",
                                    "data": "aGVsbG8="
                                }
                            }
                        ])
                        .to_string()
                    }
                ]),
            },
        ]);

        let result = convert_request(&req)
            .expect("stringified tool_result image content should become current images");
        let current =
            serde_json::to_value(&result.conversation_state.current_message.user_input_message)
                .expect("serialize current message");

        assert_eq!(current["images"].as_array().map(Vec::len), Some(1));
        assert_eq!(current["images"][0]["format"], "png");
        assert_eq!(
            current["userInputMessageContext"]["toolResults"][0]["content"][0]["text"],
            "(empty result)"
        );
    }

    #[test]
    fn convert_request_preserves_tool_result_text_while_extracting_images() {
        let req = base_request(vec![
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("Read the screenshot"),
            },
            AnthropicMessage {
                role: "assistant".to_string(),
                content: serde_json::json!([
                    {
                        "type": "tool_use",
                        "id": "tool-1",
                        "name": "read_image",
                        "input": {"path": "/tmp/screenshot.png"}
                    }
                ]),
            },
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!([
                    {
                        "type": "tool_result",
                        "tool_use_id": "tool-1",
                        "content": [
                            {
                                "type": "text",
                                "text": "Screenshot captured"
                            },
                            {
                                "type": "image",
                                "source": {
                                    "type": "base64",
                                    "media_type": "image/png",
                                    "data": "aGVsbG8="
                                }
                            }
                        ]
                    }
                ]),
            },
        ]);

        let result =
            convert_request(&req).expect("mixed tool_result content should stay supported");
        let current =
            serde_json::to_value(&result.conversation_state.current_message.user_input_message)
                .expect("serialize current message");

        assert_eq!(current["images"].as_array().map(Vec::len), Some(1));
        assert_eq!(
            current["userInputMessageContext"]["toolResults"][0]["content"][0]["text"],
            "Screenshot captured"
        );
    }

    #[test]
    fn convert_request_extracts_multiple_images_from_single_tool_result() {
        let req = base_request(vec![
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("Read the screenshots"),
            },
            AnthropicMessage {
                role: "assistant".to_string(),
                content: serde_json::json!([
                    {
                        "type": "tool_use",
                        "id": "tool-1",
                        "name": "read_image",
                        "input": {"path": "/tmp/screenshots"}
                    }
                ]),
            },
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!([
                    {
                        "type": "tool_result",
                        "tool_use_id": "tool-1",
                        "content": [
                            {
                                "type": "image",
                                "source": {
                                    "type": "base64",
                                    "media_type": "image/png",
                                    "data": "cG5n"
                                }
                            },
                            {
                                "type": "image",
                                "source": {
                                    "type": "base64",
                                    "media_type": "image/jpeg",
                                    "data": "anBlZw=="
                                }
                            }
                        ]
                    }
                ]),
            },
        ]);

        let result = convert_request(&req)
            .expect("multiple tool_result images should become current message images");
        let current =
            serde_json::to_value(&result.conversation_state.current_message.user_input_message)
                .expect("serialize current message");

        assert_eq!(current["images"].as_array().map(Vec::len), Some(2));
        assert_eq!(current["images"][0]["format"], "png");
        assert_eq!(current["images"][1]["format"], "jpeg");
        assert_eq!(
            current["userInputMessageContext"]["toolResults"][0]["content"][0]["text"],
            "(empty result)"
        );
    }

    #[test]
    fn convert_request_extracts_documents_from_tool_result_content_into_current_turn() {
        let req = base_request(vec![
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("Read the document"),
            },
            AnthropicMessage {
                role: "assistant".to_string(),
                content: serde_json::json!([
                    {
                        "type": "tool_use",
                        "id": "tool-1",
                        "name": "read_document",
                        "input": {"path": "/tmp/plain.txt"}
                    }
                ]),
            },
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!([
                    {
                        "type": "tool_result",
                        "tool_use_id": "tool-1",
                        "content": [
                            {
                                "type": "document",
                                "name": "plain.txt",
                                "source": {
                                    "type": "text",
                                    "media_type": "text/plain",
                                    "data": "plain document body"
                                }
                            }
                        ]
                    }
                ]),
            },
        ]);

        let result = convert_request(&req)
            .expect("tool_result document content should become current documents");
        let current =
            serde_json::to_value(&result.conversation_state.current_message.user_input_message)
                .expect("serialize current message");

        assert_eq!(current["documents"].as_array().map(Vec::len), Some(1));
        assert_eq!(current["documents"][0]["name"], "plain");
        assert_eq!(current["documents"][0]["format"], "txt");
        assert_eq!(
            current["userInputMessageContext"]["toolResults"][0]["content"][0]["text"],
            "(empty result)"
        );
    }

    #[test]
    fn convert_request_generates_name_for_tool_result_document_without_name() {
        let req = base_request(vec![
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("Read the document"),
            },
            AnthropicMessage {
                role: "assistant".to_string(),
                content: serde_json::json!([
                    {
                        "type": "tool_use",
                        "id": "tool-1",
                        "name": "read_document",
                        "input": {"path": "/tmp/plain.txt"}
                    }
                ]),
            },
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!([
                    {
                        "type": "tool_result",
                        "tool_use_id": "tool-1",
                        "content": [
                            {
                                "type": "document",
                                "source": {
                                    "type": "text",
                                    "media_type": "text/plain",
                                    "data": "plain document body"
                                }
                            }
                        ]
                    }
                ]),
            },
        ]);

        let result =
            convert_request(&req).expect("missing nested document name should be synthesized");
        let current =
            serde_json::to_value(&result.conversation_state.current_message.user_input_message)
                .expect("serialize current message");

        assert_eq!(current["documents"].as_array().map(Vec::len), Some(1));
        assert_eq!(
            current["documents"][0]["name"],
            generate_document_name("text/plain", "plain document body")
        );
        assert_eq!(current["documents"][0]["format"], "txt");
    }

    #[test]
    fn convert_request_adds_placeholder_text_for_document_only_current_turn() {
        let req = base_request(vec![
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("Read the document"),
            },
            AnthropicMessage {
                role: "assistant".to_string(),
                content: serde_json::json!([
                    {
                        "type": "tool_use",
                        "id": "tool-1",
                        "name": "read_document",
                        "input": {"path": "/tmp/report.pdf"}
                    }
                ]),
            },
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!([
                    {
                        "type": "tool_result",
                        "tool_use_id": "tool-1",
                        "content": "document captured"
                    },
                    {
                        "type": "document",
                        "name": "report.pdf",
                        "source": {
                            "type": "base64",
                            "media_type": "application/pdf",
                            "data": SAMPLE_PDF_BASE64
                        }
                    }
                ]),
            },
        ]);

        let result = convert_request(&req).expect("document-only current turn should pass");
        let current =
            serde_json::to_value(&result.conversation_state.current_message.user_input_message)
                .expect("serialize current message");

        assert_eq!(current["content"], "(document attached)");
        assert_eq!(current["documents"].as_array().map(Vec::len), Some(1));
        assert_eq!(
            current["userInputMessageContext"]["toolResults"][0]["content"][0]["text"],
            "document captured"
        );
    }

    #[test]
    fn convert_request_detects_images_inside_history_tool_results() {
        let mut req = base_request(vec![
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("Read the screenshot"),
            },
            AnthropicMessage {
                role: "assistant".to_string(),
                content: serde_json::json!([
                    {
                        "type": "tool_use",
                        "id": "tool-1",
                        "name": "read_image",
                        "input": {"path": "/tmp/screenshot.png"}
                    }
                ]),
            },
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!([
                    {
                        "type": "tool_result",
                        "tool_use_id": "tool-1",
                        "content": [
                            {
                                "type": "image",
                                "source": {
                                    "type": "base64",
                                    "media_type": "image/png",
                                    "data": "aGVsbG8="
                                }
                            }
                        ]
                    }
                ]),
            },
            AnthropicMessage {
                role: "assistant".to_string(),
                content: serde_json::json!("Done"),
            },
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("继续"),
            },
        ]);
        req.tools = Some(vec![AnthropicTool {
            tool_type: None,
            name: "analyze_image".to_string(),
            description: "Analyze an image".to_string(),
            input_schema: HashMap::from([(
                "anyOf".to_string(),
                serde_json::json!([
                    {"type": "object"},
                    {"type": "string"}
                ]),
            )]),
            max_uses: None,
        }]);

        let result = convert_request(&req).expect("history tool_result image should be detected");

        assert!(result.has_history_images);
        assert_eq!(
            result
                .conversation_state
                .current_message
                .user_input_message
                .user_input_message_context
                .tools[0]
                .tool_specification
                .input_schema
                .json,
            permissive_object_schema()
        );
        let history_user_with_image = result
            .conversation_state
            .history
            .iter()
            .find_map(|message| match message {
                Message::User(message) if !message.user_input_message.images.is_empty() => {
                    Some(&message.user_input_message)
                },
                _ => None,
            })
            .expect("history tool_result turn should retain extracted images");
        assert_eq!(history_user_with_image.images.len(), 1);
        assert_eq!(history_user_with_image.images[0].format, "png");
    }

    #[test]
    fn convert_request_normalizes_server_web_search_history() {
        let req = base_request(vec![
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("Find StaticFlow"),
            },
            AnthropicMessage {
                role: "assistant".to_string(),
                content: serde_json::json!([
                    {"type": "text", "text": "I'll search for StaticFlow."},
                    {
                        "type": "server_tool_use",
                        "id": "srvtoolu_test",
                        "name": "web_search",
                        "input": {"query": "StaticFlow"}
                    },
                    {
                        "type": "web_search_tool_result",
                        "content": [{
                            "type": "web_search_result",
                            "title": "StaticFlow",
                            "url": "https://example.com/staticflow",
                            "encrypted_content": "StaticFlow result summary"
                        }]
                    }
                ]),
            },
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("Use that result."),
            },
        ]);

        let result = convert_request(&req).expect("server web_search history should normalize");
        let history = semantic_history(&result);
        assert_eq!(history.len(), 2);
        let assistant = match &history[1] {
            Message::Assistant(message) => &message.assistant_response_message,
            other => panic!("expected assistant history entry, got {other:?}"),
        };
        assert!(assistant.content.contains("I'll search for StaticFlow."));
        assert!(assistant.content.contains("StaticFlow result summary"));
        assert!(assistant.tool_uses.is_none());
    }

    #[test]
    fn convert_request_converts_assistant_tool_result_history_to_text() {
        let req = base_request(vec![
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("Read remote docs"),
            },
            AnthropicMessage {
                role: "assistant".to_string(),
                content: serde_json::json!([
                    {
                        "type": "text",
                        "text": "Tool output follows."
                    },
                    {
                        "type": "tool_result",
                        "tool_use_id": "call_web_reader",
                        "content": "[{\"title\":\"Docs\",\"content\":\"Use the binary release.\"}]"
                    },
                    {
                        "type": "tool_use",
                        "id": "call_next",
                        "name": "Edit",
                        "input": {"file_path": "scripts/prepare-runtime-resources.mjs"}
                    }
                ]),
            },
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!([
                    {
                        "type": "tool_result",
                        "tool_use_id": "call_next",
                        "content": "patched"
                    }
                ]),
            },
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("Continue"),
            },
        ]);

        let normalized = normalize_request(&req).expect("normalization should succeed");
        let assistant_blocks = normalized.request.messages[1]
            .content
            .as_array()
            .expect("assistant content should stay as blocks");
        assert_eq!(
            assistant_blocks
                .iter()
                .map(|block| block["type"].as_str().unwrap_or_default())
                .collect::<Vec<_>>(),
            vec!["text", "text", "tool_use"]
        );
        assert!(normalized.normalization_events.iter().any(|event| {
            event.message_index == 1
                && event.content_block_index == Some(1)
                && event.block_type.as_deref() == Some("tool_result")
                && event.action == "rewrite_content_block"
                && event.reason == "assistant_tool_result_converted_to_text"
        }));

        let result = convert_request(&req).expect("assistant tool_result history should normalize");
        let assistant = match &semantic_history(&result)[1] {
            Message::Assistant(message) => &message.assistant_response_message,
            other => panic!("expected assistant history entry, got {other:?}"),
        };
        assert!(assistant.content.contains("Tool output follows."));
        assert!(assistant.content.contains("Use the binary release."));
        assert_eq!(
            assistant.tool_uses.as_ref().map(Vec::len),
            Some(1),
            "regular assistant tool_use should be preserved"
        );
    }

    #[test]
    fn convert_request_uses_image_bytes_when_declared_media_type_is_wrong() {
        let req = base_request(vec![AnthropicMessage {
            role: "user".to_string(),
            content: serde_json::json!([
                {
                    "type": "text",
                    "text": "Describe this animation"
                },
                {
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": "image/jpeg",
                        "data": "R0lGODlhAQABAIAAAAAAAP///ywAAAAAAQABAAACAUwAOw=="
                    }
                }
            ]),
        }]);

        let result = convert_request(&req).expect("image with mismatched media type should pass");
        let current = &result.conversation_state.current_message.user_input_message;

        assert_eq!(current.images.len(), 1);
        assert_eq!(current.images[0].format, "gif");
    }

    #[test]
    fn convert_request_merges_trailing_user_tool_results_into_current_turn() {
        let req = base_request(vec![
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("帮我获得这个的vip"),
            },
            AnthropicMessage {
                role: "assistant".to_string(),
                content: serde_json::json!([
                    {
                        "type": "text",
                        "text": "好的，让我先分析一下这个 APK 的结构。"
                    },
                    {
                        "type": "tool_use",
                        "id": "tool-manifest",
                        "name": "get_manifest",
                        "input": {}
                    },
                    {
                        "type": "tool_use",
                        "id": "tool-search",
                        "name": "search_classes",
                        "input": {"keyword": "vip"}
                    }
                ]),
            },
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!([
                    {
                        "type": "tool_result",
                        "tool_use_id": "tool-manifest",
                        "content": "manifest output"
                    }
                ]),
            },
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!([
                    {
                        "type": "tool_result",
                        "tool_use_id": "tool-search",
                        "content": "search output"
                    }
                ]),
            },
        ]);

        let result =
            convert_request(&req).expect("trailing user tool results should merge into current");
        let current = &result.conversation_state.current_message.user_input_message;
        assert!(current.content.is_empty());
        assert_eq!(current.user_input_message_context.tool_results.len(), 2);
        assert_eq!(current.user_input_message_context.tool_results[0].tool_use_id, "tool-manifest");
        assert_eq!(current.user_input_message_context.tool_results[1].tool_use_id, "tool-search");

        let history = semantic_history(&result);
        assert_eq!(history.len(), 2);
        let assistant = match &history[1] {
            Message::Assistant(message) => &message.assistant_response_message,
            other => panic!("expected assistant history entry, got {other:?}"),
        };
        assert_ne!(assistant.content, "OK");
        assert_eq!(assistant.tool_uses.as_ref().map(Vec::len), Some(2));
    }

    #[test]
    fn convert_request_merges_trailing_user_text_and_tool_result_into_current_turn() {
        let req = base_request(vec![
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("Read the file"),
            },
            AnthropicMessage {
                role: "assistant".to_string(),
                content: serde_json::json!([
                    {
                        "type": "tool_use",
                        "id": "tool-1",
                        "name": "read_file",
                        "input": {"path": "/tmp/test.txt"}
                    }
                ]),
            },
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("Please continue"),
            },
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!([
                    {
                        "type": "tool_result",
                        "tool_use_id": "tool-1",
                        "content": "file content"
                    }
                ]),
            },
        ]);

        let result = convert_request(&req)
            .expect("trailing user text and tool result should merge into current");
        let current = &result.conversation_state.current_message.user_input_message;
        assert_eq!(current.content, "Please continue");
        assert_eq!(current.user_input_message_context.tool_results.len(), 1);
        assert_eq!(current.user_input_message_context.tool_results[0].tool_use_id, "tool-1");

        let history = semantic_history(&result);
        assert_eq!(history.len(), 2);
        let assistant = match &history[1] {
            Message::Assistant(message) => &message.assistant_response_message,
            other => panic!("expected assistant history entry, got {other:?}"),
        };
        assert_ne!(assistant.content, "OK");
        assert_eq!(assistant.tool_uses.as_ref().map(Vec::len), Some(1));
    }

    #[test]
    fn convert_request_allows_empty_assistant_text_placeholder_with_tool_use() {
        let req = base_request(vec![
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("Read the file"),
            },
            AnthropicMessage {
                role: "assistant".to_string(),
                content: serde_json::json!([
                    {
                        "type": "text",
                        "text": " "
                    },
                    {
                        "type": "tool_use",
                        "id": "tool-1",
                        "name": "read_file",
                        "input": {"path": "/tmp/test.txt"}
                    }
                ]),
            },
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!([
                    {
                        "type": "tool_result",
                        "tool_use_id": "tool-1",
                        "content": "file content"
                    }
                ]),
            },
        ]);

        let result = convert_request(&req).expect("empty assistant text placeholder should pass");
        let assistant = match &semantic_history(&result)[1] {
            Message::Assistant(message) => &message.assistant_response_message,
            other => panic!("expected assistant history entry, got {other:?}"),
        };
        assert_eq!(assistant.content, " ");
        assert_eq!(assistant.tool_uses.as_ref().map(Vec::len), Some(1));
    }

    #[test]
    fn convert_request_validation_toggle_can_bypass_empty_text_rejection() {
        let req = base_request(vec![AnthropicMessage {
            role: "user".to_string(),
            content: serde_json::json!([
                {
                    "type": "text",
                    "text": " "
                }
            ]),
        }]);

        assert!(convert_request(&req).is_err());
        assert!(convert_request_with_validation(&req, false).is_ok());
    }

    #[test]
    fn convert_request_drops_orphaned_history_tool_results_without_prior_tool_use() {
        let req = base_request(vec![
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!([
                    {
                        "type": "tool_result",
                        "tool_use_id": "tool-1",
                        "content": "stale tool output"
                    },
                    {
                        "type": "text",
                        "text": "The previous command was interrupted."
                    }
                ]),
            },
            AnthropicMessage {
                role: "assistant".to_string(),
                content: serde_json::json!("No response requested."),
            },
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("Please continue"),
            },
        ]);

        let result = convert_request(&req).expect("conversion should succeed");
        let history = semantic_history(&result);
        assert_eq!(history.len(), 2);
        let first_user = match &history[0] {
            Message::User(message) => &message.user_input_message,
            other => panic!("expected first history message to stay user, got {other:?}"),
        };
        assert_eq!(first_user.content, "The previous command was interrupted.");
        assert!(first_user
            .user_input_message_context
            .tool_results
            .is_empty());
    }

    #[test]
    fn convert_request_drops_empty_history_user_turn_after_orphaned_tool_results_removed() {
        let req = base_request(vec![
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!([
                    {
                        "type": "tool_result",
                        "tool_use_id": "tool-1",
                        "content": "stale tool output"
                    }
                ]),
            },
            AnthropicMessage {
                role: "assistant".to_string(),
                content: serde_json::json!("No response requested."),
            },
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("Please continue"),
            },
        ]);

        let result = convert_request(&req).expect("conversion should succeed");
        let history = semantic_history(&result);
        assert_eq!(history.len(), 1);
        match &history[0] {
            Message::Assistant(message) => {
                assert_eq!(message.assistant_response_message.content, "No response requested.");
            },
            other => panic!("expected only assistant history message to remain, got {other:?}"),
        }
    }

    #[test]
    fn convert_request_rewrites_duplicate_completed_tool_use_ids() {
        let req = base_request(vec![
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("Run npm list"),
            },
            AnthropicMessage {
                role: "assistant".to_string(),
                content: serde_json::json!([
                    {
                        "type": "tool_use",
                        "id": "dup-tool",
                        "name": "package_proxy",
                        "input": {"tool_name": "termux_node:npm_list"}
                    }
                ]),
            },
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!([
                    {
                        "type": "tool_result",
                        "tool_use_id": "dup-tool",
                        "content": "{\"success\":true}"
                    }
                ]),
            },
            AnthropicMessage {
                role: "assistant".to_string(),
                content: serde_json::json!([
                    {
                        "type": "text",
                        "text": "Run it again"
                    },
                    {
                        "type": "tool_use",
                        "id": "dup-tool",
                        "name": "package_proxy",
                        "input": {"tool_name": "termux_node:npm_list"}
                    }
                ]),
            },
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!([
                    {
                        "type": "tool_result",
                        "tool_use_id": "dup-tool",
                        "content": "{\"success\":true,\"again\":true}"
                    }
                ]),
            },
        ]);

        let result = convert_request(&req).expect("duplicate completed tool_use id should rewrite");
        let current = &result.conversation_state.current_message.user_input_message;
        assert_eq!(current.user_input_message_context.tool_results.len(), 1);
        let rewritten_result_id = &current.user_input_message_context.tool_results[0].tool_use_id;
        assert_ne!(rewritten_result_id, "dup-tool");
        assert!(rewritten_result_id.starts_with("dup-tool__sfdup"));

        let last_assistant = match result.conversation_state.history.last() {
            Some(Message::Assistant(message)) => &message.assistant_response_message,
            other => panic!("expected last history message to be assistant, got {other:?}"),
        };
        let last_tool_uses = last_assistant
            .tool_uses
            .as_ref()
            .expect("rewritten assistant tool_use should remain in history");
        assert_eq!(last_tool_uses.len(), 1);
        assert_eq!(last_tool_uses[0].tool_use_id, *rewritten_result_id);
    }

    #[test]
    fn convert_request_rejects_ambiguous_duplicate_active_tool_use_ids() {
        let req = base_request(vec![
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("Run two things"),
            },
            AnthropicMessage {
                role: "assistant".to_string(),
                content: serde_json::json!([
                    {
                        "type": "tool_use",
                        "id": "dup-tool",
                        "name": "package_proxy",
                        "input": {"tool_name": "termux_node:npm_list"}
                    }
                ]),
            },
            AnthropicMessage {
                role: "assistant".to_string(),
                content: serde_json::json!([
                    {
                        "type": "tool_use",
                        "id": "dup-tool",
                        "name": "package_proxy",
                        "input": {"tool_name": "termux_python:pip_list"}
                    }
                ]),
            },
            AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!([
                    {
                        "type": "tool_result",
                        "tool_use_id": "dup-tool",
                        "content": "{\"success\":true}"
                    }
                ]),
            },
        ]);

        let err =
            convert_request(&req).expect_err("duplicate active tool_use id should be rejected");
        let message = err.to_string();
        assert!(message.contains("duplicate tool_use id `dup-tool`"));
        assert!(message.contains("before the previous call completed"));
    }
}
