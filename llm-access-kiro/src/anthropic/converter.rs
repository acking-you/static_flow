//! Converts Anthropic Messages API requests into Kiro wire `ConversationState`.
//!
//! Handles model name mapping, system prompt injection, thinking mode prefixes,
//! tool schema normalization, conversation history building (with consecutive
//! same-role message merging), and tool-result pairing validation.
//!
//! ## Module map
//!
//! `converter.rs` is the facade: it owns the shared data model (all `pub`
//! structs/enums such as `ConversionResult`, `NormalizedRequest`,
//! `SessionTracking`, `ConversionError`), the module constants, the
//! `invalid_request` helper, and the unit tests. Each conversion concern lives
//! in a focused descendant submodule (descendants can freely read the parent's
//! private struct fields, so no field had to be made public):
//!
//! ```text
//!  Anthropic MessagesRequest
//!        |
//!        v
//!  [validate]  reject malformed user/assistant content up front
//!        |
//!        v
//!  [normalize] per-message normalization + tool-schema normalization
//!        |          (uses: schema, tools, tool_name, document, image)
//!        v
//!  [convert]   build Kiro ConversationState: history + current turn
//!        |          (uses: system, identity, thinking, tool_result,
//!        |           tool_pairing, session, document, image)
//!        v
//!  Kiro wire ConversationState  ->  ConversionResult
//! ```
//!
//! Supporting submodules: `model` (id mapping / context window), `schema`
//! (JSON-schema normalization + multimodal compatibility), `document`/`image`
//! (attachment normalization + format detection), `tool_name` (name/id
//! sanitization & rewriting), `tools` (tool + structured-output conversion),
//! `tool_result` (tool-result extraction), `tool_pairing` (orphan pruning),
//! `session` (conversation-id resolution), `identity` (model-identity
//! handling), `thinking` (thinking-prefix injection), `system` (system-prompt
//! assembly).

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    ops::Range,
};

use base64::Engine as _;
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub(crate) use super::types;
use super::types::{ContentBlock, MessagesRequest, Metadata, SystemMessage};
use crate::wire::{
    AssistantMessage, ConversationState, CurrentMessage, HistoryAssistantMessage,
    HistoryUserMessage, InputSchema, KiroDocument, KiroImage, Message, Tool, ToolResult,
    ToolSpecification, ToolUseEntry, UserInputMessage, UserInputMessageContext, UserMessage,
};
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

pub use convert::*;
pub(crate) use document::*;
pub(crate) use identity::*;
pub(crate) use image::*;
pub use model::*;
pub use normalize::*;
pub(crate) use schema::*;
pub use session::*;
pub(crate) use system::*;
pub(crate) use thinking::*;
pub use tool_name::*;
pub(crate) use tool_pairing::*;
pub use tool_result::*;
pub(crate) use tools::*;
pub(crate) use validate::*;

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
pub(crate) struct ProcessedMessageContent {
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
pub(crate) type ToolNormalizationResult =
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
pub(crate) struct ActiveToolUse {
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
const TOOL_NAME_MAX_LEN: usize = 63;

#[cfg(test)]
mod tests;
