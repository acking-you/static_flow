//! Converts Anthropic Messages API requests into Kiro wire `ConversationState`.
//!
//! Handles model name mapping, system prompt injection, thinking mode prefixes,
//! tool schema normalization, conversation history building (with consecutive
//! same-role message merging), and tool-result pairing validation.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    ops::Range,
};

use base64::Engine as _;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::types::{ContentBlock, MessagesRequest, Metadata, SystemMessage};
use crate::wire::{
    AssistantMessage, ConversationState, CurrentMessage, HistoryAssistantMessage,
    HistoryUserMessage, InputSchema, KiroDocument, KiroImage, Message, Tool, ToolResult,
    ToolSpecification, ToolUseEntry, UserInputMessage, UserInputMessageContext, UserMessage,
};

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

fn permissive_object_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {},
        "required": [],
        "additionalProperties": true
    })
}

// Ensures a JSON schema object has all required top-level fields
// (type, properties, required, additionalProperties) so Kiro's
// tool validation does not reject it.
fn normalize_json_schema(schema: serde_json::Value) -> serde_json::Value {
    let serde_json::Value::Object(mut obj) = schema else {
        return permissive_object_schema();
    };
    if obj
        .get("type")
        .and_then(|v| v.as_str())
        .is_none_or(|s| s.is_empty())
    {
        obj.insert("type".to_string(), serde_json::Value::String("object".to_string()));
    }
    match obj.get("properties") {
        Some(serde_json::Value::Object(_)) => {},
        _ => {
            obj.insert("properties".to_string(), serde_json::Value::Object(serde_json::Map::new()));
        },
    }
    let required = match obj.remove("required") {
        Some(serde_json::Value::Array(items)) => serde_json::Value::Array(
            items
                .into_iter()
                .filter_map(|value| value.as_str().map(|text| serde_json::json!(text)))
                .collect(),
        ),
        _ => serde_json::Value::Array(Vec::new()),
    };
    obj.insert("required".to_string(), required);
    match obj.get("additionalProperties") {
        Some(serde_json::Value::Bool(_)) | Some(serde_json::Value::Object(_)) => {},
        _ => {
            obj.insert("additionalProperties".to_string(), serde_json::Value::Bool(true));
        },
    }
    serde_json::Value::Object(obj)
}

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

/// Maps an Anthropic model name (e.g. `"claude-sonnet-4-6"`) to the
/// canonical Kiro model identifier. Returns `None` for unrecognized models.
pub fn map_model(model: &str) -> Option<String> {
    let model = model.to_lowercase();
    if model.contains("sonnet") {
        if model.contains("4-6") || model.contains("4.6") {
            Some("claude-sonnet-4.6".to_string())
        } else {
            Some("claude-sonnet-4.5".to_string())
        }
    } else if model.contains("opus") {
        if model.contains("4-8") || model.contains("4.8") {
            Some("claude-opus-4.8".to_string())
        } else if model.contains("4-7") || model.contains("4.7") {
            Some("claude-opus-4.7".to_string())
        } else if model.contains("4-5") || model.contains("4.5") {
            Some("claude-opus-4.5".to_string())
        } else {
            Some("claude-opus-4.6".to_string())
        }
    } else if model.contains("haiku") {
        Some("claude-haiku-4.5".to_string())
    } else {
        None
    }
}

/// Returns the context window size (in tokens) for the given model.
/// 4.6-generation models get 1M; everything else defaults to 200K.
pub fn get_context_window_size(model: &str) -> i32 {
    match map_model(model) {
        Some(mapped)
            if mapped == "claude-sonnet-4.6"
                || mapped == "claude-opus-4.6"
                || mapped == "claude-opus-4.7"
                || mapped == "claude-opus-4.8" =>
        {
            1_000_000
        },
        _ => 200_000,
    }
}

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

pub fn preview_session_value(value: &str) -> String {
    let mut preview = value
        .chars()
        .take(SESSION_SOURCE_PREVIEW_MAX_CHARS)
        .collect::<String>();
    if value.chars().count() > SESSION_SOURCE_PREVIEW_MAX_CHARS {
        preview.push_str("...[truncated]");
    }
    preview
}

fn trailing_user_message_start(
    messages: &[super::types::Message],
) -> Result<usize, ConversionError> {
    let Some(mut start) = messages.iter().rposition(|message| message.role == "user") else {
        return Err(no_user_message_error(messages));
    };
    while start > 0 && messages[start - 1].role == "user" {
        start -= 1;
    }
    Ok(start)
}

pub fn current_user_message_range(
    messages: &[super::types::Message],
) -> Result<Range<usize>, ConversionError> {
    let end = messages
        .iter()
        .rposition(|message| message.role == "user")
        .map(|index| index + 1)
        .ok_or_else(|| no_user_message_error(messages))?;
    let start = trailing_user_message_start(&messages[..end])?;
    Ok(start..end)
}

fn no_user_message_error(messages: &[super::types::Message]) -> ConversionError {
    if messages.is_empty() {
        ConversionError::EmptyMessages
    } else {
        invalid_request("messages must include at least one user message before assistant prefill")
    }
}

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

fn push_normalization_event(
    events: &mut Vec<NormalizationEvent>,
    message_index: usize,
    role: &str,
    content_block_index: Option<usize>,
    block_type: Option<&str>,
    action: &'static str,
    reason: &'static str,
) {
    events.push(NormalizationEvent {
        message_index,
        role: role.to_string(),
        content_block_index,
        block_type: block_type.map(str::to_string),
        action,
        reason,
    });
}

fn normalize_message(
    message: &super::types::Message,
    message_index: usize,
    drop_empty_user_noop: bool,
    events: &mut Vec<NormalizationEvent>,
) -> Result<Option<super::types::Message>, ConversionError> {
    match &message.content {
        serde_json::Value::String(text) => {
            if text.trim().is_empty()
                && (message.role == "assistant" || (message.role == "user" && drop_empty_user_noop))
            {
                push_normalization_event(
                    events,
                    message_index,
                    &message.role,
                    None,
                    None,
                    "drop_message",
                    "whitespace_only_string_message",
                );
                Ok(None)
            } else {
                Ok(Some(message.clone()))
            }
        },
        serde_json::Value::Array(items) => {
            let mut retained_items = Vec::with_capacity(items.len());
            let mut normalized_any = false;
            for (block_index, item) in items.iter().enumerate() {
                let Some(obj) = item.as_object() else {
                    retained_items.push(item.clone());
                    continue;
                };
                let Some(block_type) = obj.get("type").and_then(serde_json::Value::as_str) else {
                    retained_items.push(item.clone());
                    continue;
                };

                if message.role == "user" && block_type == "document" {
                    retained_items.push(normalize_user_document_block(
                        obj,
                        message_index,
                        block_index,
                        events,
                    )?);
                    normalized_any = true;
                    continue;
                }

                if message.role == "assistant" && block_type == "server_tool_use" {
                    push_normalization_event(
                        events,
                        message_index,
                        &message.role,
                        Some(block_index),
                        Some(block_type),
                        "drop_content_block",
                        "server_tool_use_not_representable_in_kiro_history",
                    );
                    normalized_any = true;
                    continue;
                }

                if message.role == "assistant" && block_type == "web_search_tool_result" {
                    if let Some(text) = web_search_tool_result_text(obj) {
                        retained_items.push(serde_json::json!({
                            "type": "text",
                            "text": text
                        }));
                    }
                    push_normalization_event(
                        events,
                        message_index,
                        &message.role,
                        Some(block_index),
                        Some(block_type),
                        "rewrite_content_block",
                        "web_search_tool_result_converted_to_text",
                    );
                    normalized_any = true;
                    continue;
                }

                if message.role == "assistant" && block_type == "tool_result" {
                    let content = extract_tool_result_content(&obj.get("content").cloned());
                    if !content.trim().is_empty() {
                        retained_items.push(serde_json::json!({
                            "type": "text",
                            "text": content
                        }));
                    }
                    push_normalization_event(
                        events,
                        message_index,
                        &message.role,
                        Some(block_index),
                        Some(block_type),
                        if content.trim().is_empty() {
                            "drop_content_block"
                        } else {
                            "rewrite_content_block"
                        },
                        "assistant_tool_result_converted_to_text",
                    );
                    normalized_any = true;
                    continue;
                }

                let drop_reason = match block_type {
                    "text" => obj
                        .get("text")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|text| text.trim().is_empty())
                        .then_some("whitespace_only_text_block"),
                    "thinking" => obj
                        .get("thinking")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|thinking| thinking.trim().is_empty())
                        .then_some("whitespace_only_thinking_block"),
                    _ => None,
                };

                if let Some(reason) = drop_reason {
                    push_normalization_event(
                        events,
                        message_index,
                        &message.role,
                        Some(block_index),
                        Some(block_type),
                        "drop_content_block",
                        reason,
                    );
                    normalized_any = true;
                    continue;
                }

                retained_items.push(item.clone());
            }

            if !normalized_any {
                return Ok(Some(message.clone()));
            }

            // Keep current-turn user whitespace intact so the explicit
            // request-validation toggle still controls whether those payloads
            // are accepted. History-side no-op user turns are removed because
            // they cannot add context and Kiro rejects empty history entries.
            if retained_items.is_empty()
                && message.role != "assistant"
                && !(message.role == "user" && drop_empty_user_noop)
            {
                return Ok(Some(message.clone()));
            }

            if retained_items.is_empty() {
                push_normalization_event(
                    events,
                    message_index,
                    &message.role,
                    None,
                    None,
                    "drop_message",
                    "message_became_empty_after_normalization",
                );
                Ok(None)
            } else {
                let mut normalized = message.clone();
                normalized.content = serde_json::Value::Array(retained_items);
                Ok(Some(normalized))
            }
        },
        _ => Ok(Some(message.clone())),
    }
}

fn system_message_from_role_message(
    message: &super::types::Message,
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

fn web_search_tool_result_text(
    block: &serde_json::Map<String, serde_json::Value>,
) -> Option<String> {
    let content = block.get("content")?;
    let Some(items) = content.as_array() else {
        return Some(format!("Web search results:\n{content}"));
    };
    let mut lines = Vec::new();
    for item in items {
        let Some(object) = item.as_object() else {
            continue;
        };
        if object.get("type").and_then(serde_json::Value::as_str) != Some("web_search_result") {
            continue;
        }
        let title = object
            .get("title")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Untitled");
        let url = object
            .get("url")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let snippet = object
            .get("encrypted_content")
            .or_else(|| object.get("snippet"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let mut entry = format!("Result: {title}");
        if !url.is_empty() {
            entry.push_str(&format!("\nURL: {url}"));
        }
        if !snippet.is_empty() {
            entry.push_str(&format!("\nSnippet: {snippet}"));
        }
        lines.push(entry);
    }
    (!lines.is_empty()).then(|| format!("Web search results:\n{}", lines.join("\n\n")))
}

fn normalize_user_document_block(
    block: &serde_json::Map<String, serde_json::Value>,
    message_index: usize,
    block_index: usize,
    events: &mut Vec<NormalizationEvent>,
) -> Result<serde_json::Value, ConversionError> {
    let normalized = normalize_document_block_payload(block, message_index, block_index)?;
    if normalized != serde_json::Value::Object(block.clone()) {
        push_normalization_event(
            events,
            message_index,
            "user",
            Some(block_index),
            Some("document"),
            "rewrite_content_block",
            "document_block_normalized",
        );
    }
    Ok(normalized)
}

fn normalize_document_block_payload(
    block: &serde_json::Map<String, serde_json::Value>,
    message_index: usize,
    block_index: usize,
) -> Result<serde_json::Value, ConversionError> {
    let Some(source) = block.get("source").and_then(serde_json::Value::as_object) else {
        return Err(invalid_request(format!(
            "message {message_index} document block {block_index} is missing source"
        )));
    };
    let Some(source_type) = source
        .get("type")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Err(invalid_request(format!(
            "message {message_index} document block {block_index} is missing source.type"
        )));
    };
    let Some(media_type) = source
        .get("media_type")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Err(invalid_request(format!(
            "message {message_index} document block {block_index} is missing source.media_type"
        )));
    };
    let Some(source_data) = source
        .get("data")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
    else {
        return Err(invalid_request(format!(
            "message {message_index} document block {block_index} is missing source.data"
        )));
    };

    let normalized_media_type = canonical_document_media_type(media_type).ok_or_else(|| {
        invalid_request(format!(
            "message {message_index} document block {block_index} has unsupported \
             source.media_type `{media_type}`"
        ))
    })?;
    let normalized_data = match source_type {
        "base64" => source_data.trim().to_string(),
        "text" => {
            if !document_media_type_supports_text_source(normalized_media_type) {
                return Err(invalid_request(format!(
                    "message {message_index} document block {block_index} only supports \
                     source.type=`text` for plain text, markdown, html, or csv documents"
                )));
            }
            source_data.replace("\r\n", "\n").replace('\r', "\n")
        },
        _ => {
            return Err(invalid_request(format!(
                "message {message_index} document block {block_index} must use \
                 source.type=`base64` or source.type=`text`"
            )))
        },
    };
    let normalized_name = normalize_document_name(
        block.get("name").and_then(serde_json::Value::as_str),
        normalized_media_type,
        &normalized_data,
    );

    Ok(serde_json::json!({
        "type": "document",
        "name": normalized_name,
        "source": {
            "type": source_type,
            "media_type": normalized_media_type,
            "data": normalized_data,
        }
    }))
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

fn document_format_from_media_type(media_type: &str) -> Option<&'static str> {
    match media_type {
        "application/pdf" => Some("pdf"),
        "text/csv" => Some("csv"),
        "application/msword" => Some("doc"),
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => Some("docx"),
        "application/vnd.ms-excel" => Some("xls"),
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => Some("xlsx"),
        "text/html" => Some("html"),
        "text/plain" => Some("txt"),
        "text/markdown" => Some("md"),
        _ => None,
    }
}

fn normalize_document_name(raw_name: Option<&str>, media_type: &str, data: &str) -> String {
    match raw_name.map(str::trim).filter(|value| !value.is_empty()) {
        Some(raw_name) => sanitize_document_name(raw_name),
        None => generate_document_name(media_type, data),
    }
}

fn sanitize_document_name(name: &str) -> String {
    let without_extension = name.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(name);
    let mut sanitized = String::with_capacity(without_extension.len());
    let mut previous_dash = false;
    let mut previous_space = false;
    for ch in without_extension.chars() {
        let normalized = if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '(' | ')' | '[' | ']')
        {
            previous_dash = false;
            previous_space = false;
            Some(ch)
        } else if ch.is_ascii_whitespace() {
            if previous_space {
                None
            } else {
                previous_space = true;
                previous_dash = false;
                Some(' ')
            }
        } else if previous_dash {
            None
        } else {
            previous_dash = true;
            previous_space = false;
            Some('-')
        };
        if let Some(ch) = normalized {
            sanitized.push(ch);
        }
    }
    let trimmed = sanitized.trim();
    if trimmed.is_empty() {
        "document".to_string()
    } else {
        trimmed.chars().take(200).collect()
    }
}

fn generate_document_name(media_type: &str, data: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(media_type.as_bytes());
    hasher.update([0]);
    hasher.update(data.as_bytes());
    let hash_hex = format!("{:x}", hasher.finalize());
    format!("document-{}", &hash_hex[..12])
}

fn normalize_tool_description(name: &str, description: &str) -> Option<String> {
    if description.trim().is_empty() {
        Some(format!("Client-provided tool '{name}'"))
    } else {
        None
    }
}

fn collect_schema_keywords(value: &serde_json::Value, counts: &mut BTreeMap<String, usize>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                if MULTIMODAL_UNSUPPORTED_SCHEMA_KEYWORDS.contains(&key.as_str()) {
                    *counts.entry(key.clone()).or_default() += 1;
                }
                collect_schema_keywords(child, counts);
            }
        },
        serde_json::Value::Array(items) => {
            for child in items {
                collect_schema_keywords(child, counts);
            }
        },
        _ => {},
    }
}

fn schema_contains_multimodal_unsupported_keywords(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(map) => map.iter().any(|(key, child)| {
            MULTIMODAL_UNSUPPORTED_SCHEMA_KEYWORDS.contains(&key.as_str())
                || schema_contains_multimodal_unsupported_keywords(child)
        }),
        serde_json::Value::Array(items) => items
            .iter()
            .any(schema_contains_multimodal_unsupported_keywords),
        _ => false,
    }
}

fn content_value_contains_image(content: &serde_json::Value) -> bool {
    let serde_json::Value::Array(items) = content else {
        return false;
    };
    items.iter().any(|item| {
        let Some(block_type) = item.get("type").and_then(|value| value.as_str()) else {
            return false;
        };
        match block_type {
            "image" => true,
            "tool_result" => item
                .get("content")
                .is_some_and(content_value_contains_image),
            _ => false,
        }
    })
}

fn request_message_contains_image(message: &super::types::Message) -> bool {
    content_value_contains_image(&message.content)
}

fn apply_multimodal_tool_schema_compatibility(tools: &mut [Tool], has_images: bool) {
    if !has_images {
        return;
    }
    for tool in tools {
        let schema = &tool.tool_specification.input_schema.json;
        if schema_contains_multimodal_unsupported_keywords(schema) {
            tool.tool_specification.input_schema =
                InputSchema::from_json(permissive_object_schema());
        }
    }
}

fn normalize_tools(
    tools: &Option<Vec<super::types::Tool>>,
) -> Result<ToolNormalizationResult, ConversionError> {
    let Some(tools) = tools else {
        return Ok((None, Vec::new(), ToolValidationSummary::default()));
    };

    let mut normalized_tools = Vec::with_capacity(tools.len());
    let mut events = Vec::new();
    let mut summary = ToolValidationSummary::default();

    for (tool_index, tool) in tools.iter().enumerate() {
        let name = tool.name.trim();
        if name.is_empty() {
            summary.empty_tool_name_count += 1;
            return Err(invalid_request(format!("tool {tool_index} has empty name")));
        }

        let mut normalized_tool = tool.clone();
        normalized_tool.name = name.to_string();

        if let Some(description) = normalize_tool_description(name, &tool.description) {
            normalized_tool.description = description;
            summary.normalized_tool_description_count += 1;
            events.push(ToolNormalizationEvent {
                tool_index,
                tool_name: normalized_tool.name.clone(),
                action: "fill_tool_description",
                reason: "empty_tool_description",
            });
        }

        let schema =
            serde_json::Value::Object(normalized_tool.input_schema.clone().into_iter().collect());
        collect_schema_keywords(&schema, &mut summary.schema_keyword_counts);

        normalized_tools.push(normalized_tool);
    }

    Ok((Some(normalized_tools), events, summary))
}

// Performs a conservative cleanup pass before validation/conversion.
//
// This stage is intentionally narrow:
// - Drop trailing turns after the last user message because they can never
//   affect the request sent upstream.
// - Promote `system` role turns into the Anthropic top-level `system` field,
//   keeping the conversation history limited to user/assistant turns.
// - Remove whitespace-only text/thinking blocks and any message that becomes an
//   empty no-op after that cleanup.
// - Keep malformed/unknown structures intact so the strict validator can still
//   reject genuinely broken payloads instead of silently guessing.
//
// The goal is to accept harmless transport noise from upstream proxies without
// inventing new semantics or rewriting the conversation history.
pub fn normalize_request(req: &MessagesRequest) -> Result<NormalizedRequest, ConversionError> {
    let last_user_idx = req
        .messages
        .iter()
        .rposition(|message| message.role == "user")
        .ok_or_else(|| no_user_message_error(&req.messages))?;
    let current_user_start = trailing_user_message_start(&req.messages[..last_user_idx + 1])?;
    let mut events = Vec::new();
    let mut normalized_messages = Vec::with_capacity(last_user_idx + 1);
    let mut message_index_map = Vec::with_capacity(last_user_idx + 1);
    let mut system_messages = req.system.clone().unwrap_or_default();
    let mut drop_assistant_after_empty_user_noop = false;

    for (message_index, message) in req.messages.iter().enumerate() {
        if message.role == "system" {
            system_messages.push(system_message_from_role_message(message, message_index)?);
            push_normalization_event(
                &mut events,
                message_index,
                &message.role,
                None,
                None,
                "promote_message",
                "system_role_promoted_to_top_level",
            );
            continue;
        }

        if message_index > last_user_idx {
            push_normalization_event(
                &mut events,
                message_index,
                &message.role,
                None,
                None,
                "drop_message",
                "trailing_after_last_user",
            );
            continue;
        }

        if drop_assistant_after_empty_user_noop {
            if message.role == "assistant" {
                push_normalization_event(
                    &mut events,
                    message_index,
                    &message.role,
                    None,
                    None,
                    "drop_message",
                    "assistant_after_empty_user_noop",
                );
                continue;
            }
            if message.role == "user" {
                drop_assistant_after_empty_user_noop = false;
            }
        }

        let drop_empty_user_noop = message.role == "user" && message_index < current_user_start;
        match normalize_message(message, message_index, drop_empty_user_noop, &mut events)? {
            Some(normalized) => {
                message_index_map.push(message_index);
                normalized_messages.push(normalized);
            },
            None => {
                if drop_empty_user_noop {
                    drop_assistant_after_empty_user_noop = true;
                }
            },
        }
    }

    if let Some(last_retained_user_idx) = normalized_messages
        .iter()
        .rposition(|message| message.role == "user")
    {
        for dropped_index in last_retained_user_idx + 1..normalized_messages.len() {
            push_normalization_event(
                &mut events,
                message_index_map[dropped_index],
                &normalized_messages[dropped_index].role,
                None,
                None,
                "drop_message",
                "trailing_after_last_retained_user",
            );
        }
        normalized_messages.truncate(last_retained_user_idx + 1);
        message_index_map.truncate(last_retained_user_idx + 1);
    } else {
        normalized_messages.clear();
        message_index_map.clear();
    }

    let (normalized_tools, tool_normalization_events, tool_validation_summary) =
        normalize_tools(&req.tools)?;

    rewrite_duplicate_tool_use_ids(NormalizedRequest {
        request: MessagesRequest {
            model: req.model.clone(),
            _max_tokens: req._max_tokens,
            messages: normalized_messages,
            stream: req.stream,
            system: (!system_messages.is_empty()).then_some(system_messages),
            tools: normalized_tools,
            _tool_choice: req._tool_choice.clone(),
            thinking: req.thinking.clone(),
            output_config: req.output_config.clone(),
            metadata: req.metadata.clone(),
        },
        tool_use_id_rewrites: Vec::new(),
        normalization_events: events,
        tool_normalization_events,
        tool_validation_summary,
        message_index_map,
    })
}

fn rewrite_duplicate_tool_use_ids(
    mut normalized: NormalizedRequest,
) -> Result<NormalizedRequest, ConversionError> {
    let request = &mut normalized.request;

    let mut used_ids = collect_existing_tool_use_ids(&request.messages);
    let mut seen_counts = HashMap::<String, usize>::new();
    let mut active_by_original = HashMap::<String, ActiveToolUse>::new();
    let mut rewrites = Vec::<ToolUseIdRewrite>::new();

    for (message_index, message) in request.messages.iter_mut().enumerate() {
        let original_message_index = normalized.message_index_map[message_index];
        let Some(items) = message.content.as_array_mut() else {
            continue;
        };
        match message.role.as_str() {
            "assistant" => {
                for (block_index, item) in items.iter_mut().enumerate() {
                    let Some(obj) = item.as_object_mut() else {
                        continue;
                    };
                    if obj.get("type").and_then(serde_json::Value::as_str) != Some("tool_use") {
                        continue;
                    }
                    let Some(original_id) = obj
                        .get("id")
                        .and_then(serde_json::Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(str::to_string)
                    else {
                        continue;
                    };
                    if active_by_original.contains_key(&original_id) {
                        return Err(invalid_request(format!(
                            "message {original_message_index} tool_use block {block_index} reuses \
                             duplicate tool_use id `{original_id}` before the previous call \
                             completed"
                        )));
                    }

                    let seen_count = seen_counts.entry(original_id.clone()).or_insert(0);
                    *seen_count += 1;

                    let normalized_id = if *seen_count == 1 {
                        original_id.clone()
                    } else {
                        next_rewritten_tool_use_id(&original_id, *seen_count, &used_ids)
                    };
                    let rewrite_index = if normalized_id != original_id {
                        obj.insert(
                            "id".to_string(),
                            serde_json::Value::String(normalized_id.clone()),
                        );
                        rewrites.push(ToolUseIdRewrite {
                            original_tool_use_id: original_id.clone(),
                            rewritten_tool_use_id: normalized_id.clone(),
                            assistant_message_index: original_message_index,
                            content_block_index: block_index,
                            rewritten_tool_result_count: 0,
                        });
                        Some(rewrites.len() - 1)
                    } else {
                        None
                    };
                    used_ids.insert(normalized_id.clone());
                    active_by_original.insert(original_id, ActiveToolUse {
                        normalized_id,
                        rewrite_index,
                    });
                }
            },
            "user" => {
                for (block_index, item) in items.iter_mut().enumerate() {
                    let Some(obj) = item.as_object_mut() else {
                        continue;
                    };
                    if obj.get("type").and_then(serde_json::Value::as_str) != Some("tool_result") {
                        continue;
                    }
                    let Some(original_id) = obj
                        .get("tool_use_id")
                        .and_then(serde_json::Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(str::to_string)
                    else {
                        continue;
                    };

                    match active_by_original.remove(&original_id) {
                        Some(active) => {
                            if active.normalized_id != original_id {
                                obj.insert(
                                    "tool_use_id".to_string(),
                                    serde_json::Value::String(active.normalized_id),
                                );
                                if let Some(rewrite_index) = active.rewrite_index {
                                    rewrites[rewrite_index].rewritten_tool_result_count += 1;
                                }
                            }
                        },
                        None => {
                            if seen_counts.get(&original_id).copied().unwrap_or_default() > 1 {
                                return Err(invalid_request(format!(
                                    "message {original_message_index} tool_result block \
                                     {block_index} references duplicate tool_use id \
                                     `{original_id}` after its rewritten call already completed"
                                )));
                            }
                        },
                    }
                }
            },
            _ => {},
        }
    }

    normalized.tool_use_id_rewrites = rewrites;
    Ok(normalized)
}

fn collect_existing_tool_use_ids(messages: &[super::types::Message]) -> HashSet<String> {
    let mut ids = HashSet::new();
    for message in messages {
        let Some(items) = message.content.as_array() else {
            continue;
        };
        for item in items {
            let Some(obj) = item.as_object() else {
                continue;
            };
            if obj.get("type").and_then(serde_json::Value::as_str) != Some("tool_use") {
                continue;
            }
            if let Some(id) = obj
                .get("id")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                ids.insert(id.to_string());
            }
        }
    }
    ids
}

fn next_rewritten_tool_use_id(
    original_id: &str,
    occurrence: usize,
    used_ids: &HashSet<String>,
) -> String {
    let mut suffix = occurrence;
    loop {
        let candidate = format!("{original_id}__sfdup{suffix}");
        if !used_ids.contains(&candidate) {
            return candidate;
        }
        suffix += 1;
    }
}

// Extracts a UUID session ID from the Anthropic `user_id` metadata field.
// Supports either a JSON payload containing `session_id` or the legacy
// `..._session_<uuid>...` string format.
#[cfg(test)]
fn extract_session_id(user_id: &str) -> Option<String> {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(user_id) {
        if let Some(session_id) = value.get("session_id").and_then(|value| value.as_str()) {
            if is_valid_uuid(session_id) {
                return Some(session_id.to_string());
            }
        }
    }

    let pos = user_id.find("session_")?;
    let session_part = &user_id[pos + 8..];
    if session_part.len() < 36 {
        return None;
    }

    let uuid = &session_part[..36];
    is_valid_uuid(uuid).then(|| uuid.to_string())
}

fn is_valid_uuid(value: &str) -> bool {
    value.len() == 36 && value.chars().filter(|ch| *ch == '-').count() == 4
}

fn generated_fallback(
    reason: SessionFallbackReason,
    source_name: Option<&'static str>,
    source_value_preview: Option<String>,
) -> ResolvedConversationId {
    ResolvedConversationId {
        conversation_id: Uuid::new_v4().to_string(),
        session_tracking: SessionTracking {
            source: SessionIdSource::GeneratedFallback(reason),
            source_name,
            source_value_preview,
        },
    }
}

pub fn resolve_conversation_id_from_metadata(
    metadata: Option<&Metadata>,
) -> ResolvedConversationId {
    let Some(metadata) = metadata else {
        return generated_fallback(SessionFallbackReason::MissingMetadata, None, None);
    };

    let Some(user_id) = metadata.user_id.as_deref() else {
        return generated_fallback(SessionFallbackReason::MissingUserId, None, None);
    };

    let user_id_preview = Some(preview_session_value(user_id));
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(user_id) {
        if let Some(session_id) = value.get("session_id").and_then(|value| value.as_str()) {
            if is_valid_uuid(session_id) {
                return ResolvedConversationId {
                    conversation_id: session_id.to_string(),
                    session_tracking: SessionTracking {
                        source: SessionIdSource::MetadataJson,
                        source_name: None,
                        source_value_preview: user_id_preview,
                    },
                };
            }
            return generated_fallback(
                SessionFallbackReason::InvalidJsonSessionId,
                None,
                user_id_preview,
            );
        }
        return generated_fallback(
            SessionFallbackReason::MissingJsonSessionId,
            None,
            user_id_preview,
        );
    }

    let Some(pos) = user_id.find("session_") else {
        return generated_fallback(
            SessionFallbackReason::MissingLegacySessionId,
            None,
            user_id_preview,
        );
    };
    let session_part = &user_id[pos + 8..];
    if session_part.len() < 36 {
        return generated_fallback(
            SessionFallbackReason::InvalidLegacySessionId,
            None,
            user_id_preview,
        );
    }

    let uuid = &session_part[..36];
    if is_valid_uuid(uuid) {
        ResolvedConversationId {
            conversation_id: uuid.to_string(),
            session_tracking: SessionTracking {
                source: SessionIdSource::MetadataLegacy,
                source_name: None,
                source_value_preview: user_id_preview,
            },
        }
    } else {
        generated_fallback(SessionFallbackReason::InvalidLegacySessionId, None, user_id_preview)
    }
}

// Collects unique tool names from assistant messages in history, used to
// synthesize placeholder tool specs for tools referenced only in history.
fn collect_history_tool_names(history: &[Message]) -> Vec<String> {
    let mut tool_names = Vec::new();
    for message in history {
        if let Message::Assistant(message) = message {
            if let Some(tool_uses) = &message.assistant_response_message.tool_uses {
                for tool_use in tool_uses {
                    if !tool_names.contains(&tool_use.name) {
                        tool_names.push(tool_use.name.clone());
                    }
                }
            }
        }
    }
    tool_names
}

// Creates a minimal placeholder tool spec so Kiro doesn't reject
// tool_use entries in history that reference tools not in the current set.
fn create_placeholder_tool(name: &str) -> Tool {
    Tool {
        tool_specification: ToolSpecification {
            name: name.to_string(),
            description: "Tool used in conversation history".to_string(),
            input_schema: InputSchema::from_json(permissive_object_schema()),
        },
    }
}

const TOOL_NAME_MAX_LEN: usize = 63;

fn truncate_tool_name_prefix(name: &str, prefix_max: usize) -> &str {
    match name.char_indices().nth(prefix_max) {
        Some((idx, _)) => &name[..idx],
        None => name,
    }
}

fn sanitize_tool_name(name: &str) -> String {
    let mut sanitized = String::with_capacity(name.len());
    let mut previous_was_separator = false;

    for ch in name.chars() {
        let mapped = if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' { ch } else { '_' };
        if mapped == '_' {
            if previous_was_separator {
                continue;
            }
            previous_was_separator = true;
        } else {
            previous_was_separator = false;
        }
        sanitized.push(mapped);
    }

    let sanitized = sanitized.trim_matches('_');
    if sanitized.is_empty() {
        "tool".to_string()
    } else {
        sanitized.to_string()
    }
}


fn make_hashed_tool_name_alias(original_name: &str, visible_base: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(original_name.as_bytes());
    let hash_hex = format!("{:x}", hasher.finalize());
    let hash_suffix = &hash_hex[..8];
    let prefix_max = TOOL_NAME_MAX_LEN - 1 - 8;
    let prefix = truncate_tool_name_prefix(visible_base, prefix_max);
    format!("{prefix}_{hash_suffix}")
}

#[cfg(test)]
fn shorten_tool_name(name: &str) -> String {
    make_hashed_tool_name_alias(name, name)
}

// Kiro rejects certain otherwise valid Anthropic tool names, notably
// package-style names such as `package:subtool`. We therefore normalize every
// tool name into a Kiro-safe identifier before the request leaves our process,
// while keeping a reverse map so responses can restore the original name.
fn map_tool_name(name: &str, tool_name_map: &mut HashMap<String, String>) -> String {
    let sanitized = sanitize_tool_name(name);
    if sanitized == name && name.len() <= TOOL_NAME_MAX_LEN {
        return name.to_string();
    }

    let alias = make_hashed_tool_name_alias(name, &sanitized);
    tool_name_map.insert(alias.clone(), name.to_string());
    alias
}

/// Converts an Anthropic `MessagesRequest` into a Kiro `ConversationState`.
///
/// Steps: map model, build history (merging consecutive same-role messages),
/// inject system prompt + thinking prefix, validate tool-result pairing,
/// strip orphaned tool_uses, and assemble the final wire payload.
#[cfg(test)]
pub fn convert_request(req: &MessagesRequest) -> Result<ConversionResult, ConversionError> {
    convert_request_with_validation(req, true)
}

#[cfg(test)]
fn convert_request_with_validation(
    req: &MessagesRequest,
    request_validation_enabled: bool,
) -> Result<ConversionResult, ConversionError> {
    let normalized = normalize_request(req)?;
    convert_normalized_request_with_validation(normalized, request_validation_enabled)
}

#[cfg(test)]
pub(crate) fn convert_normalized_request_with_validation(
    normalized: NormalizedRequest,
    request_validation_enabled: bool,
) -> Result<ConversionResult, ConversionError> {
    let resolved_conversation =
        resolve_conversation_id_from_metadata(normalized.request.metadata.as_ref());
    convert_normalized_request_with_resolved_session(
        normalized,
        request_validation_enabled,
        resolved_conversation,
    )
}

pub fn convert_normalized_request_with_resolved_session(
    normalized: NormalizedRequest,
    request_validation_enabled: bool,
    resolved_conversation: ResolvedConversationId,
) -> Result<ConversionResult, ConversionError> {
    let req = &normalized.request;
    let model_id = map_model(&req.model)
        .ok_or_else(|| ConversionError::UnsupportedModel(req.model.clone()))?;
    if request_validation_enabled {
        validate_messages_request(req)?;
    }
    let messages = req.messages.as_slice();
    if messages.is_empty() {
        return Err(ConversionError::EmptyMessages);
    }
    let current_range = current_user_message_range(messages)?;
    let current_messages = messages[current_range.clone()].iter().collect::<Vec<_>>();
    let has_history_images = messages[..current_range.start]
        .iter()
        .any(request_message_contains_image);

    let mut user_input = merge_current_user_messages(&current_messages, &model_id)?;
    let response_identity = response_identity_for_current_turn(req, &user_input.content);
    apply_thinking_prefix_to_current_turn(req, &mut user_input);
    let mut tool_name_map = HashMap::new();
    let mut tools = convert_tools(&req.tools, &mut tool_name_map);
    let structured_output_tool_name = append_structured_output_tool(req, &mut tools);
    let mut history = build_history(
        req,
        &messages[..current_range.start],
        &model_id,
        &mut tool_name_map,
        structured_output_tool_name.as_deref(),
    )?;
    dedupe_history_and_current_documents(&mut history, &mut user_input)?;
    prune_orphaned_history_tool_results(&mut history);
    let current_tool_results = user_input.user_input_message_context.tool_results.clone();
    let (validated_tool_results, orphaned_tool_use_ids) =
        validate_tool_pairing(&history, &current_tool_results);
    remove_orphaned_tool_uses(&mut history, &orphaned_tool_use_ids);

    // Inject placeholder tool specs for tools that appear in history but
    // are not in the current tool set, so Kiro accepts the conversation.
    let existing_tool_names: HashSet<String> = tools
        .iter()
        .map(|tool| tool.tool_specification.name.to_lowercase())
        .collect();
    for tool_name in collect_history_tool_names(&history) {
        if !existing_tool_names.contains(&tool_name.to_lowercase()) {
            tools.push(create_placeholder_tool(&tool_name));
        }
    }
    apply_multimodal_tool_schema_compatibility(
        &mut tools,
        !user_input.images.is_empty() || has_history_images,
    );

    let mut context = user_input.user_input_message_context.clone();
    if !tools.is_empty() {
        context = context.with_tools(tools);
    }
    context = context.with_tool_results(validated_tool_results);
    user_input = user_input.with_context(context).with_origin("AI_EDITOR");

    Ok(ConversionResult {
        conversation_state: ConversationState::new(resolved_conversation.conversation_id)
            .with_chat_trigger_type("MANUAL")
            .with_current_message(CurrentMessage::new(user_input))
            .with_history(history),
        tool_name_map,
        session_tracking: resolved_conversation.session_tracking,
        has_history_images,
        structured_output_tool_name,
        response_identity,
    })
}

fn validate_messages_request(req: &MessagesRequest) -> Result<(), ConversionError> {
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

// Extracts text, images, documents, and tool_results from a message's
// polymorphic `content` field (string or array of typed blocks).
fn process_message_content(
    content: &serde_json::Value,
) -> Result<ProcessedMessageContent, ConversionError> {
    let mut text_parts = Vec::new();
    let mut images = Vec::new();
    let mut documents = Vec::new();
    let mut tool_results = Vec::new();
    match content {
        serde_json::Value::String(text) => text_parts.push(text.clone()),
        serde_json::Value::Array(items) => {
            for item in items {
                if let Ok(block) = serde_json::from_value::<ContentBlock>(item.clone()) {
                    match block.block_type.as_str() {
                        "text" => {
                            if let Some(text) = block.text.filter(|text| !text.trim().is_empty()) {
                                text_parts.push(text);
                            }
                        },
                        "image" => {
                            if let Some(source) = block.source {
                                if let Some(format) = get_image_format_from_source(&source) {
                                    images.push(KiroImage::from_base64(format, source.data));
                                }
                            }
                        },
                        "document" => {
                            if let Some(source) = block.source {
                                if let Some(document) =
                                    kiro_document_from_block(block.name, source)?
                                {
                                    documents.push(document);
                                }
                            }
                        },
                        "tool_result" => {
                            if let Some(tool_use_id) = block.tool_use_id {
                                let (tool_result_images, tool_result_documents) =
                                    extract_tool_result_attachments(&block.content)?;
                                let has_tool_result_attachments = !tool_result_images.is_empty()
                                    || !tool_result_documents.is_empty();
                                images.extend(tool_result_images);
                                documents.extend(tool_result_documents);
                                let mut result_content =
                                    extract_tool_result_content(&block.content);
                                if result_content.trim().is_empty() && has_tool_result_attachments {
                                    result_content = EMPTY_TOOL_RESULT_PLACEHOLDER.to_string();
                                }
                                let is_error = block.is_error.unwrap_or(false);
                                let mut result = if is_error {
                                    ToolResult::error(&tool_use_id, result_content)
                                } else {
                                    ToolResult::success(&tool_use_id, result_content)
                                };
                                result.status =
                                    Some(if is_error { "error" } else { "success" }.to_string());
                                tool_results.push(result);
                            }
                        },
                        _ => {},
                    }
                }
            }
        },
        _ => {},
    }
    Ok(ProcessedMessageContent {
        text: text_parts.join("\n"),
        images,
        documents,
        tool_results,
    })
}

fn get_image_format_from_source(source: &super::types::ImageSource) -> Option<String> {
    detect_image_format_from_base64(&source.data)
        .or_else(|| get_image_format(&source.media_type))
        .map(str::to_string)
}

fn detect_image_format_from_base64(data: &str) -> Option<&'static str> {
    let mut prefix = data
        .chars()
        .filter(|ch| !ch.is_ascii_whitespace())
        .take(64)
        .collect::<String>();
    if prefix.is_empty() {
        return None;
    }
    while prefix.len() % 4 != 0 {
        prefix.push('=');
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(prefix.as_bytes())
        .ok()?;
    detect_image_format_from_bytes(&bytes)
}

fn detect_image_format_from_bytes(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some("jpeg");
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("png");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("gif");
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("webp");
    }
    None
}

fn get_image_format(media_type: &str) -> Option<&'static str> {
    match media_type {
        "image/jpeg" => Some("jpeg"),
        "image/png" => Some("png"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        _ => None,
    }
}

fn kiro_document_from_source(
    name: String,
    source: super::types::ImageSource,
) -> Option<KiroDocument> {
    let format = document_format_from_media_type(&source.media_type)?;
    let bytes = match source.source_type.as_str() {
        "base64" => source.data,
        "text" if document_media_type_supports_text_source(&source.media_type) => {
            base64::engine::general_purpose::STANDARD.encode(source.data.as_bytes())
        },
        _ => return None,
    };
    Some(KiroDocument::from_base64(name, format, bytes))
}

fn kiro_document_from_block(
    name: Option<String>,
    mut source: super::types::ImageSource,
) -> Result<Option<KiroDocument>, ConversionError> {
    let Some(normalized_media_type) = canonical_document_media_type(&source.media_type) else {
        return Ok(None);
    };
    let normalized_data = match source.source_type.as_str() {
        "base64" => {
            let normalized_data = source.data.trim().to_string();
            source.data = normalized_data.clone();
            normalized_data
        },
        "text" if document_media_type_supports_text_source(normalized_media_type) => {
            let normalized_data = source.data.replace("\r\n", "\n").replace('\r', "\n");
            source.data = normalized_data.clone();
            normalized_data
        },
        _ => source.data.clone(),
    };
    let normalized_name =
        normalize_document_name(name.as_deref(), normalized_media_type, &normalized_data);
    source.media_type = normalized_media_type.to_string();
    Ok(kiro_document_from_source(normalized_name, source))
}

fn extract_tool_result_attachments(
    content: &Option<serde_json::Value>,
) -> Result<(Vec<KiroImage>, Vec<KiroDocument>), ConversionError> {
    if let Some(parsed_content) = parse_stringified_tool_result_content(content) {
        return extract_tool_result_attachments_from_value(&parsed_content);
    }
    match content {
        Some(value) => extract_tool_result_attachments_from_value(value),
        None => Ok((Vec::new(), Vec::new())),
    }
}

fn extract_tool_result_attachments_from_value(
    value: &serde_json::Value,
) -> Result<(Vec<KiroImage>, Vec<KiroDocument>), ConversionError> {
    let mut images = Vec::new();
    let mut documents = Vec::new();

    match value {
        serde_json::Value::Array(items) => {
            for item in items {
                let Ok(block) = serde_json::from_value::<ContentBlock>(item.clone()) else {
                    continue;
                };
                match block.block_type.as_str() {
                    "image" => {
                        if let Some(source) = block.source {
                            if let Some(format) = get_image_format_from_source(&source) {
                                images.push(KiroImage::from_base64(format, source.data));
                            }
                        }
                    },
                    "document" => {
                        if let Some(source) = block.source {
                            if let Some(document) = kiro_document_from_block(block.name, source)? {
                                documents.push(document);
                            }
                        }
                    },
                    _ => {},
                }
            }
        },
        serde_json::Value::Object(_) if looks_like_tool_result_content_block(value) => {
            let Ok(block) = serde_json::from_value::<ContentBlock>(value.clone()) else {
                return Ok((images, documents));
            };
            match block.block_type.as_str() {
                "image" => {
                    if let Some(source) = block.source {
                        if let Some(format) = get_image_format_from_source(&source) {
                            images.push(KiroImage::from_base64(format, source.data));
                        }
                    }
                },
                "document" => {
                    if let Some(source) = block.source {
                        if let Some(document) = kiro_document_from_block(block.name, source)? {
                            documents.push(document);
                        }
                    }
                },
                _ => {},
            }
        },
        _ => {},
    }

    Ok((images, documents))
}

pub fn extract_tool_result_content(content: &Option<serde_json::Value>) -> String {
    if let Some(parsed_content) = parse_stringified_tool_result_content(content) {
        return extract_tool_result_content_from_value(&parsed_content);
    }
    match content {
        Some(value) => extract_tool_result_content_from_value(value),
        None => String::new(),
    }
}

fn extract_tool_result_content_from_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(|item| item.get("text").and_then(|value| value.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        serde_json::Value::Object(object) if looks_like_tool_result_content_block(value) => object
            .get("text")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        other => other.to_string(),
    }
}

fn parse_stringified_tool_result_content(
    content: &Option<serde_json::Value>,
) -> Option<serde_json::Value> {
    let serde_json::Value::String(text) = content.as_ref()? else {
        return None;
    };
    let trimmed = text.trim();
    if trimmed.is_empty() || !(trimmed.starts_with('[') || trimmed.starts_with('{')) {
        return None;
    }
    let parsed = serde_json::from_str::<serde_json::Value>(trimmed).ok()?;
    match &parsed {
        serde_json::Value::Array(items)
            if !items.is_empty() && items.iter().all(looks_like_tool_result_content_block) =>
        {
            Some(parsed)
        },
        serde_json::Value::Object(_) if looks_like_tool_result_content_block(&parsed) => {
            Some(parsed)
        },
        _ => None,
    }
}

fn looks_like_tool_result_content_block(value: &serde_json::Value) -> bool {
    matches!(
        value.get("type").and_then(serde_json::Value::as_str),
        Some("text" | "image" | "document")
    )
}

// Validates that every tool_result in the current message has a matching
// tool_use in history. Returns the validated results and the set of
// orphaned tool_use IDs that have no corresponding result anywhere.
fn validate_tool_pairing(
    history: &[Message],
    tool_results: &[ToolResult],
) -> (Vec<ToolResult>, HashSet<String>) {
    let mut all_tool_use_ids = HashSet::new();
    let mut history_tool_result_ids = HashSet::new();

    for message in history {
        match message {
            Message::Assistant(message) => {
                if let Some(tool_uses) = &message.assistant_response_message.tool_uses {
                    for tool_use in tool_uses {
                        all_tool_use_ids.insert(tool_use.tool_use_id.clone());
                    }
                }
            },
            Message::User(message) => {
                for result in &message
                    .user_input_message
                    .user_input_message_context
                    .tool_results
                {
                    history_tool_result_ids.insert(result.tool_use_id.clone());
                }
            },
        }
    }

    let mut unpaired_tool_use_ids: HashSet<String> = all_tool_use_ids
        .difference(&history_tool_result_ids)
        .cloned()
        .collect();
    let mut filtered_results = Vec::new();
    for result in tool_results {
        if unpaired_tool_use_ids.contains(&result.tool_use_id) {
            filtered_results.push(result.clone());
            unpaired_tool_use_ids.remove(&result.tool_use_id);
        }
    }
    (filtered_results, unpaired_tool_use_ids)
}

// Removes tool_use entries from assistant messages in history whose IDs
// are in the orphaned set (no matching tool_result exists).
fn remove_orphaned_tool_uses(history: &mut [Message], orphaned_ids: &HashSet<String>) {
    if orphaned_ids.is_empty() {
        return;
    }
    for message in history.iter_mut() {
        if let Message::Assistant(message) = message {
            if let Some(tool_uses) = message.assistant_response_message.tool_uses.as_mut() {
                tool_uses.retain(|entry| !orphaned_ids.contains(&entry.tool_use_id));
                if tool_uses.is_empty() {
                    message.assistant_response_message.tool_uses = None;
                }
            }
        }
    }
}

// Drops history tool_results that do not correspond to an earlier assistant
// tool_use in the preserved history prefix. Kiro rejects history turns that
// contain tool results without a prior tool call, so we enforce that invariant
// before validating the current turn.
fn prune_orphaned_history_tool_results(history: &mut Vec<Message>) {
    let mut pending_tool_use_ids = HashSet::<String>::new();
    let mut retained = Vec::with_capacity(history.len());

    for message in history.drain(..) {
        match message {
            Message::Assistant(message) => {
                if let Some(tool_uses) = &message.assistant_response_message.tool_uses {
                    for tool_use in tool_uses {
                        pending_tool_use_ids.insert(tool_use.tool_use_id.clone());
                    }
                }
                retained.push(Message::Assistant(message));
            },
            Message::User(mut message) => {
                let context = &mut message.user_input_message.user_input_message_context;
                if !context.tool_results.is_empty() {
                    context
                        .tool_results
                        .retain(|result| pending_tool_use_ids.remove(&result.tool_use_id));
                }

                let has_content = !message.user_input_message.content.trim().is_empty();
                let has_images = !message.user_input_message.images.is_empty();
                let has_documents = !message.user_input_message.documents.is_empty();
                let has_tool_results = !context.tool_results.is_empty();
                if has_content || has_images || has_documents || has_tool_results {
                    retained.push(Message::User(message));
                }
            },
        }
    }

    *history = retained;
}

// Converts Anthropic tool definitions to Kiro wire Tool specs.
// Appends chunked-write policy suffixes to Write/Edit tool descriptions
// and truncates descriptions to 10K chars.
fn convert_tools(
    tools: &Option<Vec<super::types::Tool>>,
    tool_name_map: &mut HashMap<String, String>,
) -> Vec<Tool> {
    let Some(tools) = tools else {
        return Vec::new();
    };
    tools
        .iter()
        .map(|tool| {
            let mut description = tool.description.clone();
            let suffix = match tool.name.as_str() {
                "Write" => WRITE_TOOL_DESCRIPTION_SUFFIX,
                "Edit" => EDIT_TOOL_DESCRIPTION_SUFFIX,
                _ => "",
            };
            if !suffix.is_empty() {
                description.push('\n');
                description.push_str(suffix);
            }
            let description = match description.char_indices().nth(10_000) {
                Some((idx, _)) => description[..idx].to_string(),
                None => description,
            };
            Tool {
                tool_specification: ToolSpecification {
                    name: map_tool_name(&tool.name, tool_name_map),
                    description,
                    input_schema: InputSchema::from_json(normalize_json_schema(serde_json::json!(
                        tool.input_schema
                    ))),
                },
            }
        })
        .collect()
}

fn extract_structured_output_schema(req: &MessagesRequest) -> Option<serde_json::Value> {
    req.output_config
        .as_ref()
        .and_then(|config| config.json_schema())
        .cloned()
        .map(normalize_json_schema)
}

fn make_structured_output_tool_name(existing_tools: &[Tool]) -> String {
    let existing = existing_tools
        .iter()
        .map(|tool| tool.tool_specification.name.to_lowercase())
        .collect::<HashSet<_>>();
    if !existing.contains(&STRUCTURED_OUTPUT_TOOL_NAME_BASE.to_lowercase()) {
        return STRUCTURED_OUTPUT_TOOL_NAME_BASE.to_string();
    }
    for suffix in 1.. {
        let candidate = format!("{STRUCTURED_OUTPUT_TOOL_NAME_BASE}_{suffix}");
        if !existing.contains(&candidate.to_lowercase()) {
            return candidate;
        }
    }
    unreachable!("finite tool name search should always terminate")
}

fn structured_output_instruction(tool_name: &str) -> String {
    format!(
        "Return the final answer by calling the `{tool_name}` tool exactly once. Do not emit any \
         free-form text outside that tool call."
    )
}

fn append_structured_output_tool(req: &MessagesRequest, tools: &mut Vec<Tool>) -> Option<String> {
    let schema = extract_structured_output_schema(req)?;
    let tool_name = make_structured_output_tool_name(tools);
    tools.push(Tool {
        tool_specification: ToolSpecification {
            name: tool_name.clone(),
            description: STRUCTURED_OUTPUT_TOOL_DESCRIPTION.to_string(),
            input_schema: InputSchema::from_json(schema),
        },
    });
    Some(tool_name)
}

// Generates the XML thinking-mode prefix from the request's thinking
// configuration. This is intentionally applied only to the current user turn:
// Kiro has no first-class thinking-level request field, and placing the dynamic
// level/budget in history would poison the stable cache prefix.
//
// For adaptive thinking, preserve the caller's exact effort label instead of
// normalizing it. Local verification against Kiro showed that `low`,
// `medium`, `high`, `xhigh`, and `max` are not interchangeable:
// `low`/`medium` can stay minimal, `high` produces short hidden rationale, and
// `xhigh`/`max` produce materially deeper hidden reasoning in both buffered and
// streaming paths.
fn generate_thinking_prefix(req: &MessagesRequest) -> Option<String> {
    if let Some(thinking) = &req.thinking {
        if thinking.thinking_type == "enabled" {
            return Some(format!(
                "<thinking_mode>enabled</thinking_mode><max_thinking_length>{}</\
                 max_thinking_length>",
                thinking.budget_tokens
            ));
        }
        if thinking.thinking_type == "adaptive" {
            let effort = req
                .output_config
                .as_ref()
                .map(|config| config.effective_effort())
                .unwrap_or("xhigh");
            return Some(format!(
                "<thinking_mode>adaptive</thinking_mode><thinking_effort>{effort}</\
                 thinking_effort>"
            ));
        }
    }
    None
}

fn has_thinking_tags(content: &str) -> bool {
    content.contains("<thinking_mode>")
        || content.contains("<max_thinking_length>")
        || content.contains("<thinking_effort>")
}

fn apply_thinking_prefix_to_current_turn(req: &MessagesRequest, user_input: &mut UserInputMessage) {
    let Some(prefix) = generate_thinking_prefix(req) else {
        return;
    };
    if has_thinking_tags(&user_input.content) {
        return;
    }
    user_input.content = if user_input.content.is_empty() {
        prefix
    } else {
        format!("{prefix}\n{}", user_input.content)
    };
}

fn requested_model_identity_id(model: &str) -> &str {
    model.strip_suffix("-thinking").unwrap_or(model)
}

fn requested_model_identity_name(model: &str) -> Option<&'static str> {
    match requested_model_identity_id(model) {
        "claude-opus-4-8" => Some("Opus 4.8"),
        "claude-opus-4-7" => Some("Opus 4.7"),
        "claude-opus-4-6" => Some("Opus 4.6"),
        "claude-sonnet-4-6" => Some("Sonnet 4.6"),
        "claude-sonnet-4-5" => Some("Sonnet 4.5"),
        "claude-haiku-4-5" => Some("Haiku 4.5"),
        _ => None,
    }
}

fn response_model_identity(model: &str) -> Option<ResponseModelIdentity> {
    let model_name = requested_model_identity_name(model)?;
    Some(ResponseModelIdentity {
        model_name: format!("Claude {model_name}"),
        model_id: requested_model_identity_id(model).to_string(),
    })
}

fn response_identity_for_current_turn(
    req: &MessagesRequest,
    current_content: &str,
) -> Option<ResponseModelIdentity> {
    is_model_identity_probe(current_content).then(|| response_model_identity(&req.model))?
}

fn is_model_identity_probe(content: &str) -> bool {
    let lower = content.to_lowercase();
    let compact = lower
        .chars()
        .filter(|ch| !ch.is_whitespace() && *ch != '-' && *ch != '_' && *ch != '`')
        .collect::<String>();
    let asks_identity = lower.contains("who are you")
        || lower.contains("what are you")
        || lower.contains("your identity")
        || lower.contains("are you claude")
        || lower.contains("are you kiro")
        || content.contains("你是谁")
        || content.contains("你是什么")
        || content.contains("你的身份")
        || content.contains("你是Claude")
        || content.contains("你是 Claude")
        || content.contains("你是Kiro")
        || content.contains("你是 Kiro");
    let asks_model_identity = lower.contains("what model are you")
        || lower.contains("which model are you")
        || lower.contains("your model")
        || (compact.contains("modelid") && (lower.contains("you") || lower.contains("your")))
        || content.contains("你的模型")
        || content.contains("你是什么模型")
        || content.contains("你是哪种模型")
        || ((content.contains("模型ID") || content.contains("模型 ID"))
            && (content.contains("你") || content.contains("你的")));

    asks_identity || asks_model_identity
}

fn anthropic_identity_override(requested_model: &str) -> String {
    let Some(identity) = response_model_identity(requested_model) else {
        return GENERIC_ANTHROPIC_IDENTITY_OVERRIDE.to_string();
    };
    format!(
        "<identity_override>\nYou are Claude, made by Anthropic. For this request, your model \
         name is {model_name} and your public API model ID is {model_id}. When asked about your \
         identity, model name, or model ID, answer with this Claude identity. Never claim to be \
         Kiro, Warp, or any other product. You are Claude, running on the Anthropic API \
         platform.\n</identity_override>",
        model_name = identity.model_name,
        model_id = identity.model_id
    )
}

fn normalize_claude_code_model_identity(content: String, requested_model: &str) -> String {
    let Some(model_name) = requested_model_identity_name(requested_model) else {
        return content;
    };
    let model_id = requested_model_identity_id(requested_model);
    let replacement = format!(
        "You are powered by the model named {model_name}. The exact model ID is {model_id}."
    );
    let has_existing_model_identity = content.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed.contains("You are powered by the model named")
            && trimmed.contains("The exact model ID is")
    });
    let mut replaced_existing = false;
    let mut inserted_after_identity = false;
    content
        .lines()
        .map(|line| {
            let trimmed = line.trim_start();
            if trimmed.contains("You are powered by the model named")
                && trimmed.contains("The exact model ID is")
            {
                replaced_existing = true;
                let indent = &line[..line.len() - trimmed.len()];
                format!("{indent}{replacement}")
            } else if !has_existing_model_identity
                && !replaced_existing
                && !inserted_after_identity
                && (trimmed == CLAUDE_CODE_CLI_SYSTEM_IDENTITY_LINE
                    || trimmed == CLAUDE_AGENT_SDK_SYSTEM_IDENTITY_LINE)
            {
                inserted_after_identity = true;
                format!("{line}\n{replacement}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn strip_volatile_claude_code_billing_header(content: String) -> String {
    content
        .lines()
        .filter(|line| !is_claude_code_billing_header_text(line))
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_claude_code_billing_header_text(content: &str) -> bool {
    let trimmed = content.trim_start();
    trimmed.starts_with(CLAUDE_CODE_BILLING_HEADER_PREFIX)
        && (trimmed.contains("cc_version=")
            || trimmed.contains("cc_entrypoint=")
            || trimmed.contains("cch="))
}

fn cleaned_system_message_text(message: &SystemMessage) -> Option<String> {
    let content = strip_volatile_claude_code_billing_header(message.text.clone());
    (!content.trim().is_empty()).then_some(content)
}

fn build_injected_system_content(
    req: &MessagesRequest,
    structured_output_tool_name: Option<&str>,
) -> Option<String> {
    let identity_override = anthropic_identity_override(&req.model);
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
        .map(|content| normalize_claude_code_model_identity(content, &req.model))
        .map(|content| format!("{content}\n{SYSTEM_CHUNKED_POLICY}\n{identity_override}"));

    let mut parts = Vec::new();
    parts.push(system_content.unwrap_or(identity_override));
    if let Some(tool_name) = structured_output_tool_name {
        parts.push(structured_output_instruction(tool_name));
    }
    Some(parts.join("\n"))
}

// Builds the Kiro history from Anthropic messages that precede the current
// trailing user turn. Injects stable system prompt text as a synthetic
// user/assistant turn pair at the start, then merges consecutive same-role
// messages into single turns.
fn build_history(
    req: &MessagesRequest,
    messages: &[super::types::Message],
    model_id: &str,
    tool_name_map: &mut HashMap<String, String>,
    structured_output_tool_name: Option<&str>,
) -> Result<Vec<Message>, ConversionError> {
    let mut history = Vec::new();
    if let Some(system_content) = build_injected_system_content(req, structured_output_tool_name) {
        history.push(Message::User(HistoryUserMessage::new(system_content, model_id)));
        history.push(Message::Assistant(HistoryAssistantMessage::new(
            "I will follow these instructions.",
        )));
    }

    let mut user_buffer = Vec::new();
    let mut assistant_buffer = Vec::new();

    for message in messages {
        if message.role == "user" {
            if !assistant_buffer.is_empty() {
                history.push(Message::Assistant(merge_assistant_messages(
                    &assistant_buffer,
                    tool_name_map,
                )?));
                assistant_buffer.clear();
            }
            user_buffer.push(message);
        } else if message.role == "assistant" {
            if !user_buffer.is_empty() {
                history.push(Message::User(merge_user_messages(&user_buffer, model_id)?));
                user_buffer.clear();
            }
            assistant_buffer.push(message);
        }
    }

    if !assistant_buffer.is_empty() {
        history
            .push(Message::Assistant(merge_assistant_messages(&assistant_buffer, tool_name_map)?));
    }
    if !user_buffer.is_empty() {
        history.push(Message::User(merge_user_messages(&user_buffer, model_id)?));
    }

    Ok(history)
}

fn merge_current_user_messages(
    messages: &[&super::types::Message],
    model_id: &str,
) -> Result<UserInputMessage, ConversionError> {
    let mut content_parts = Vec::new();
    let mut images = Vec::new();
    let mut documents = Vec::new();
    let mut tool_results = Vec::new();
    for message in messages {
        let processed = process_message_content(&message.content)?;
        if !processed.text.is_empty() {
            content_parts.push(processed.text);
        }
        images.extend(processed.images);
        documents.extend(processed.documents);
        tool_results.extend(processed.tool_results);
    }
    let content = content_parts.join("\n");
    if images.len() > KIRO_MAX_CURRENT_MESSAGE_IMAGES {
        let keep_from = images.len() - KIRO_MAX_CURRENT_MESSAGE_IMAGES;
        images.drain(0..keep_from);
    }
    dedupe_documents_in_place(&mut documents, &mut HashSet::new());
    let mut user_message = UserInputMessage::new(&content, model_id);
    if !images.is_empty() {
        user_message = user_message.with_images(images);
    }
    if !documents.is_empty() {
        user_message = user_message.with_documents(documents);
    }
    if !tool_results.is_empty() {
        user_message = user_message
            .with_context(UserInputMessageContext::new().with_tool_results(tool_results));
    }
    Ok(user_message)
}

fn merge_user_messages(
    messages: &[&super::types::Message],
    model_id: &str,
) -> Result<HistoryUserMessage, ConversionError> {
    let mut content_parts = Vec::new();
    let mut images = Vec::new();
    let mut documents = Vec::new();
    let mut tool_results = Vec::new();
    for message in messages {
        let processed = process_message_content(&message.content)?;
        if !processed.text.is_empty() {
            content_parts.push(processed.text);
        }
        images.extend(processed.images);
        documents.extend(processed.documents);
        tool_results.extend(processed.tool_results);
    }
    let content = content_parts.join("\n");
    let mut user_message = UserMessage::new(&content, model_id);
    if !images.is_empty() {
        user_message = user_message.with_images(images);
    }
    dedupe_documents_in_place(&mut documents, &mut HashSet::new());
    if !documents.is_empty() {
        user_message = user_message.with_documents(documents);
    }
    if !tool_results.is_empty() {
        user_message = user_message
            .with_context(UserInputMessageContext::new().with_tool_results(tool_results));
    }
    Ok(HistoryUserMessage {
        user_input_message: user_message,
    })
}

fn dedupe_documents_in_place(documents: &mut Vec<KiroDocument>, seen: &mut HashSet<String>) {
    documents.retain(|document| seen.insert(document.name.clone()));
}

fn dedupe_history_and_current_documents(
    history: &mut [Message],
    current: &mut UserInputMessage,
) -> Result<(), ConversionError> {
    let mut seen = HashSet::new();
    for message in history.iter_mut() {
        if let Message::User(user_message) = message {
            dedupe_documents_in_place(&mut user_message.user_input_message.documents, &mut seen);
        }
    }
    dedupe_documents_in_place(&mut current.documents, &mut seen);
    if seen.len() > KIRO_MAX_CONVERSATION_DOCUMENTS {
        return Err(invalid_request(format!(
            "Too many documents attached ({}). Maximum is {} per conversation.",
            seen.len(),
            KIRO_MAX_CONVERSATION_DOCUMENTS
        )));
    }
    for message in history.iter_mut() {
        if let Message::User(user_message) = message {
            ensure_document_content_placeholder(
                &mut user_message.user_input_message.content,
                &user_message.user_input_message.documents,
            );
        }
    }
    ensure_document_content_placeholder(&mut current.content, &current.documents);
    Ok(())
}

fn ensure_document_content_placeholder(content: &mut String, documents: &[KiroDocument]) {
    if content.trim().is_empty() && !documents.is_empty() {
        *content = EMPTY_DOCUMENT_PLACEHOLDER.to_string();
    }
}

fn convert_assistant_message(
    message: &super::types::Message,
    tool_name_map: &mut HashMap<String, String>,
) -> Result<HistoryAssistantMessage, ConversionError> {
    let mut thinking_content = String::new();
    let mut text_content = String::new();
    let mut tool_uses = Vec::new();
    match &message.content {
        serde_json::Value::String(text) => text_content = text.clone(),
        serde_json::Value::Array(items) => {
            for item in items {
                if let Ok(block) = serde_json::from_value::<ContentBlock>(item.clone()) {
                    match block.block_type.as_str() {
                        "thinking" => {
                            if let Some(thinking) = block
                                .thinking
                                .filter(|thinking| !thinking.trim().is_empty())
                            {
                                thinking_content.push_str(&thinking);
                            }
                        },
                        "text" => {
                            if let Some(text) = block.text.filter(|text| !text.trim().is_empty()) {
                                text_content.push_str(&text);
                            }
                        },
                        "tool_use" => {
                            if let (Some(id), Some(name)) = (block.id, block.name) {
                                let mapped_name = map_tool_name(&name, tool_name_map);
                                tool_uses.push(ToolUseEntry::new(id, mapped_name).with_input(
                                    block.input.unwrap_or_else(|| serde_json::json!({})),
                                ));
                            }
                        },
                        _ => {},
                    }
                }
            }
        },
        _ => {},
    }
    // When an assistant message has only tool_uses and no text, use a
    // single space as content placeholder (Kiro requires non-empty content).
    let final_content = if !thinking_content.is_empty() {
        if !text_content.is_empty() {
            format!("<thinking>{thinking_content}</thinking>\n\n{text_content}")
        } else {
            format!("<thinking>{thinking_content}</thinking>")
        }
    } else if text_content.is_empty() && !tool_uses.is_empty() {
        " ".to_string()
    } else {
        text_content
    };
    let mut assistant = AssistantMessage::new(final_content);
    if !tool_uses.is_empty() {
        assistant = assistant.with_tool_uses(tool_uses);
    }
    Ok(HistoryAssistantMessage {
        assistant_response_message: assistant,
    })
}

fn merge_assistant_messages(
    messages: &[&super::types::Message],
    tool_name_map: &mut HashMap<String, String>,
) -> Result<HistoryAssistantMessage, ConversionError> {
    if messages.len() == 1 {
        return convert_assistant_message(messages[0], tool_name_map);
    }
    let mut tool_uses = Vec::new();
    let mut content_parts = Vec::new();
    for message in messages {
        let converted = convert_assistant_message(message, tool_name_map)?;
        let assistant_message = converted.assistant_response_message;
        if !assistant_message.content.trim().is_empty() {
            content_parts.push(assistant_message.content);
        }
        if let Some(items) = assistant_message.tool_uses {
            tool_uses.extend(items);
        }
    }
    let content = if content_parts.is_empty() && !tool_uses.is_empty() {
        " ".to_string()
    } else {
        content_parts.join("\n\n")
    };
    let mut assistant = AssistantMessage::new(content);
    if !tool_uses.is_empty() {
        assistant = assistant.with_tool_uses(tool_uses);
    }
    Ok(HistoryAssistantMessage {
        assistant_response_message: assistant,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{
        super::types::{
            Message as AnthropicMessage, Metadata, SystemMessage, Tool as AnthropicTool,
        },
        *,
    };

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
    fn get_context_window_size_matches_latest_kiro_model_rules() {
        assert_eq!(get_context_window_size("claude-sonnet-4-6"), 1_000_000);
        assert_eq!(get_context_window_size("claude-opus-4-20250514"), 1_000_000);
        assert_eq!(map_model("claude-opus-4-8"), Some("claude-opus-4.8".to_string()));
        assert_eq!(map_model("claude-opus-4.8"), Some("claude-opus-4.8".to_string()));
        assert_eq!(get_context_window_size("claude-opus-4-8"), 1_000_000);
        assert_eq!(map_model("claude-opus-4-7"), Some("claude-opus-4.7".to_string()));
        assert_eq!(get_context_window_size("claude-opus-4-7"), 1_000_000);
        assert_eq!(get_context_window_size("claude-sonnet-4-5-20250929"), 200_000);
    }

    #[test]
    fn extract_session_id_handles_valid_and_invalid_values() {
        assert_eq!(
            extract_session_id("user_x_account__session_8bb5523b-ec7c-4540-a9ca-beb6d79f1552"),
            Some("8bb5523b-ec7c-4540-a9ca-beb6d79f1552".to_string())
        );
        assert_eq!(
            extract_session_id(
                r#"{"device_id":"dev","account_uuid":"acct","session_id":"a0662283-7fd3-4399-a7eb-52b9a717ae88"}"#
            ),
            Some("a0662283-7fd3-4399-a7eb-52b9a717ae88".to_string())
        );
        assert_eq!(extract_session_id(r#"{"session_id":"invalid-uuid"}"#), None);
        assert_eq!(extract_session_id("user_without_session"), None);
        assert_eq!(extract_session_id("user_x__session_invalid-uuid"), None);
    }

    #[test]
    fn shorten_tool_name_is_deterministic_and_bounded() {
        let long_name =
            "tool_with_a_name_far_beyond_the_supported_sixty_three_character_limit_for_kiro";
        let short1 = shorten_tool_name(long_name);
        let short2 = shorten_tool_name(long_name);

        assert_eq!(short1, short2);
        assert!(short1.len() <= TOOL_NAME_MAX_LEN);
    }

    #[test]
    fn normalize_json_schema_repairs_null_fields() {
        let normalized = normalize_json_schema(serde_json::json!({
            "type": null,
            "properties": null,
            "required": null,
            "additionalProperties": null
        }));

        assert_eq!(normalized["type"], "object");
        assert_eq!(normalized["properties"], serde_json::json!({}));
        assert_eq!(normalized["required"], serde_json::json!([]));
        assert_eq!(normalized["additionalProperties"], serde_json::json!(true));
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
    fn convert_request_promotes_system_role_messages_for_supported_kiro_models() {
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
                normalized
                    .request
                    .system
                    .as_ref()
                    .and_then(|messages| messages.first())
                    .map(|message| message.text.as_str()),
                Some("You are Claude Code, Anthropic's official CLI for Claude."),
                "{model}"
            );
            assert!(
                normalized.normalization_events.iter().any(|event| {
                    event.message_index == 1
                        && event.role == "system"
                        && event.action == "promote_message"
                        && event.reason == "system_role_promoted_to_top_level"
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
    fn convert_assistant_message_tool_use_only_uses_space_placeholder() {
        let message = AnthropicMessage {
            role: "assistant".to_string(),
            content: serde_json::json!([
                {"type": "tool_use", "id": "toolu_01ABC", "name": "read_file", "input": {"path": "/tmp/test.txt"}}
            ]),
        };

        let result = convert_assistant_message(&message, &mut HashMap::new())
            .expect("conversion should succeed");
        assert_eq!(result.assistant_response_message.content, " ");
        let tool_uses = result
            .assistant_response_message
            .tool_uses
            .expect("tool use should exist");
        assert_eq!(tool_uses.len(), 1);
        assert_eq!(tool_uses[0].tool_use_id, "toolu_01ABC");
    }

    #[test]
    fn merge_consecutive_assistant_messages_keeps_thinking_and_tool_use() {
        let first = AnthropicMessage {
            role: "assistant".to_string(),
            content: serde_json::json!([
                {"type": "thinking", "thinking": "Let me think."},
                {"type": "text", "text": " "}
            ]),
        };
        let second = AnthropicMessage {
            role: "assistant".to_string(),
            content: serde_json::json!([
                {"type": "thinking", "thinking": "I should read the file."},
                {"type": "text", "text": "Let me read that file."},
                {"type": "tool_use", "id": "toolu_01ABC", "name": "read_file", "input": {"path": "/tmp/test.txt"}}
            ]),
        };

        let result = merge_assistant_messages(&[&first, &second], &mut HashMap::new())
            .expect("merge should succeed");
        let content = &result.assistant_response_message.content;
        assert!(content.contains("<thinking>"));
        assert!(content.contains("Let me read that file."));
        let tool_uses = result
            .assistant_response_message
            .tool_uses
            .expect("tool use should exist");
        assert_eq!(tool_uses.len(), 1);
        assert_eq!(tool_uses[0].tool_use_id, "toolu_01ABC");
    }

    #[test]
    fn validate_tool_pairing_ignores_duplicate_results_already_paired_in_history() {
        let mut user_with_result = UserMessage::new("", "claude-sonnet-4.5");
        user_with_result = user_with_result.with_context(
            UserInputMessageContext::new()
                .with_tool_results(vec![ToolResult::success("tool-1", "history result")]),
        );

        let history = vec![
            Message::User(HistoryUserMessage::new("Read the file", "claude-sonnet-4.5")),
            Message::Assistant(HistoryAssistantMessage {
                assistant_response_message: AssistantMessage::new("I'll read the file")
                    .with_tool_uses(vec![ToolUseEntry::new("tool-1", "read_file")]),
            }),
            Message::User(HistoryUserMessage {
                user_input_message: user_with_result,
            }),
            Message::Assistant(HistoryAssistantMessage::new("Done")),
        ];

        let (filtered, orphaned) =
            validate_tool_pairing(&history, &[ToolResult::success("tool-1", "duplicate result")]);
        assert!(filtered.is_empty());
        assert!(orphaned.is_empty());
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
