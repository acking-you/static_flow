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
use lopdf::Document as PdfDocument;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::types::{ContentBlock, MessagesRequest, Metadata};
use crate::wire::{
    AssistantMessage, ConversationState, CurrentMessage, HistoryAssistantMessage,
    HistoryUserMessage, InputSchema, KiroImage, Message, Tool, ToolResult, ToolSpecification,
    ToolUseEntry, UserInputMessage, UserInputMessageContext, UserMessage,
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
const PDF_DOCUMENT_OPEN_TAG: &str = "<document media_type=\"application/pdf\">";
const PDF_DOCUMENT_CLOSE_TAG: &str = "</document>";
const TEXT_DOCUMENT_OPEN_TAG: &str = "<document media_type=\"text/plain\">";
const TEXT_DOCUMENT_CLOSE_TAG: &str = "</document>";
const STRUCTURED_OUTPUT_TOOL_NAME_BASE: &str = "sf_emit_structured_output";
const STRUCTURED_OUTPUT_TOOL_DESCRIPTION: &str =
    "Return the final answer as structured JSON that exactly matches the provided schema. Call \
     this tool exactly once and do not emit any free-form text outside the tool call.";

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
        if model.contains("4-5") || model.contains("4.5") {
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
        Some(mapped) if mapped == "claude-sonnet-4.6" || mapped == "claude-opus-4.6" => 1_000_000,
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
    events: &mut Vec<NormalizationEvent>,
) -> Result<Option<super::types::Message>, ConversionError> {
    match &message.content {
        serde_json::Value::String(text) => {
            if message.role == "assistant" && text.trim().is_empty() {
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

            // Keep user-only whitespace turns intact so the explicit
            // request-validation toggle still controls whether those payloads
            // are accepted. Only assistant-side no-op turns are removed here.
            if retained_items.is_empty() && message.role != "assistant" {
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

    match source_type {
        "base64" => {
            if media_type != "application/pdf" {
                return Err(invalid_request(format!(
                    "message {message_index} document block {block_index} only supports \
                     source.media_type=`application/pdf` for base64 documents"
                )));
            }

            let encoded_pdf = source_data.trim();
            let extracted_text = extract_pdf_document_text(encoded_pdf).map_err(|reason| {
                invalid_request(format!(
                    "message {message_index} document block {block_index} could not be converted \
                     from PDF: {reason}"
                ))
            })?;

            push_normalization_event(
                events,
                message_index,
                "user",
                Some(block_index),
                Some("document"),
                "rewrite_content_block",
                "pdf_document_converted_to_text",
            );

            Ok(serde_json::json!({
                "type": "text",
                "text": format!(
                    "{PDF_DOCUMENT_OPEN_TAG}\n{extracted_text}\n{PDF_DOCUMENT_CLOSE_TAG}"
                )
            }))
        },
        "text" => {
            if media_type != "text/plain" {
                return Err(invalid_request(format!(
                    "message {message_index} document block {block_index} only supports \
                     source.media_type=`text/plain` for text documents"
                )));
            }
            let normalized_text = source_data.replace("\r\n", "\n").replace('\r', "\n");

            push_normalization_event(
                events,
                message_index,
                "user",
                Some(block_index),
                Some("document"),
                "rewrite_content_block",
                "text_document_converted_to_text",
            );

            Ok(serde_json::json!({
                "type": "text",
                "text": format!(
                    "{TEXT_DOCUMENT_OPEN_TAG}\n{}\n{TEXT_DOCUMENT_CLOSE_TAG}",
                    normalized_text.trim()
                )
            }))
        },
        _ => Err(invalid_request(format!(
            "message {message_index} document block {block_index} must use source.type=`base64` \
             or source.type=`text`"
        ))),
    }
}

fn extract_pdf_document_text(encoded_pdf: &str) -> Result<String, String> {
    let pdf_bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded_pdf.as_bytes())
        .map_err(|err| format!("invalid base64 payload: {err}"))?;
    let document = load_pdf_document_with_xref_repair(&pdf_bytes)
        .map_err(|err| format!("invalid PDF payload: {err}"))?;
    let page_numbers = document.get_pages().into_keys().collect::<Vec<_>>();
    if page_numbers.is_empty() {
        return Err("pdf has no pages".to_string());
    }

    let text = document
        .extract_text(&page_numbers)
        .map_err(|err| format!("failed to extract PDF text: {err}"))?;
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let trimmed = normalized.trim();
    if trimmed.is_empty() {
        return Err("pdf contained no extractable text".to_string());
    }
    Ok(trimmed.to_string())
}

fn load_pdf_document_with_xref_repair(pdf_bytes: &[u8]) -> Result<PdfDocument, String> {
    match PdfDocument::load_mem(pdf_bytes) {
        Ok(document) => Ok(document),
        Err(primary_err) => {
            let repaired = repair_pdf_xref(pdf_bytes).ok_or_else(|| primary_err.to_string())?;
            PdfDocument::load_mem(&repaired).map_err(|secondary_err| {
                format!(
                    "{primary_err}; xref repair failed to produce a readable PDF: {secondary_err}"
                )
            })
        },
    }
}

fn repair_pdf_xref(pdf_bytes: &[u8]) -> Option<Vec<u8>> {
    let object_offsets = scan_indirect_object_offsets(pdf_bytes);
    if object_offsets.is_empty() {
        return None;
    }

    let root_reference = find_named_reference(pdf_bytes, b"/Root")
        .or_else(|| find_catalog_reference(pdf_bytes, &object_offsets))?;
    let max_object_id = *object_offsets.keys().max()?;
    let mut repaired = pdf_bytes.to_vec();
    if !repaired.ends_with(b"\n") {
        repaired.push(b'\n');
    }

    let xref_start = repaired.len();
    repaired.extend_from_slice(format!("xref\n0 {}\n", max_object_id + 1).as_bytes());
    repaired.extend_from_slice(b"0000000000 65535 f \n");
    for object_id in 1..=max_object_id {
        if let Some((offset, generation)) = object_offsets.get(&object_id) {
            repaired.extend_from_slice(format!("{offset:010} {generation:05} n \n").as_bytes());
        } else {
            repaired.extend_from_slice(b"0000000000 00000 f \n");
        }
    }
    repaired.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root {} {} R >>\nstartxref\n{}\n%%EOF\n",
            max_object_id + 1,
            root_reference.0,
            root_reference.1,
            xref_start
        )
        .as_bytes(),
    );
    Some(repaired)
}

fn scan_indirect_object_offsets(pdf_bytes: &[u8]) -> BTreeMap<u32, (usize, u16)> {
    let mut offsets = BTreeMap::new();
    let mut index = 0;

    while index < pdf_bytes.len() {
        if !pdf_bytes[index].is_ascii_digit()
            || (index > 0 && pdf_bytes[index - 1].is_ascii_digit())
        {
            index += 1;
            continue;
        }

        let object_start = index;
        let Some((object_id, after_object_id)) = parse_unsigned_pdf_integer(pdf_bytes, index)
        else {
            index += 1;
            continue;
        };
        let Some(after_object_ws) = skip_required_pdf_whitespace(pdf_bytes, after_object_id) else {
            index += 1;
            continue;
        };
        let Some((generation, after_generation)) =
            parse_unsigned_pdf_integer(pdf_bytes, after_object_ws)
        else {
            index += 1;
            continue;
        };
        let Some(after_generation_ws) = skip_required_pdf_whitespace(pdf_bytes, after_generation)
        else {
            index += 1;
            continue;
        };

        if pdf_bytes.get(after_generation_ws..after_generation_ws + 3) != Some(b"obj")
            || pdf_bytes
                .get(after_generation_ws + 3)
                .is_some_and(|byte| is_pdf_name_char(*byte))
        {
            index += 1;
            continue;
        }

        let generation = match u16::try_from(generation) {
            Ok(value) => value,
            Err(_) => {
                index += 1;
                continue;
            },
        };
        offsets
            .entry(object_id)
            .or_insert((object_start, generation));
        index = after_generation_ws + 3;
    }

    offsets
}

fn find_named_reference(pdf_bytes: &[u8], name: &[u8]) -> Option<(u32, u16)> {
    let mut index = 0;
    let mut last_match = None;

    while let Some(relative_pos) = find_subslice(&pdf_bytes[index..], name) {
        let start = index + relative_pos + name.len();
        let value_start = skip_pdf_whitespace(pdf_bytes, start);
        let Some((object_id, after_object_id)) = parse_unsigned_pdf_integer(pdf_bytes, value_start)
        else {
            index = start;
            continue;
        };
        let Some(after_object_ws) = skip_required_pdf_whitespace(pdf_bytes, after_object_id) else {
            index = start;
            continue;
        };
        let Some((generation, after_generation)) =
            parse_unsigned_pdf_integer(pdf_bytes, after_object_ws)
        else {
            index = start;
            continue;
        };
        let Some(after_generation_ws) = skip_required_pdf_whitespace(pdf_bytes, after_generation)
        else {
            index = start;
            continue;
        };

        if pdf_bytes.get(after_generation_ws) == Some(&b'R') {
            let generation = match u16::try_from(generation) {
                Ok(value) => value,
                Err(_) => {
                    index = start;
                    continue;
                },
            };
            last_match = Some((object_id, generation));
        }
        index = start;
    }

    last_match
}

fn find_catalog_reference(
    pdf_bytes: &[u8],
    object_offsets: &BTreeMap<u32, (usize, u16)>,
) -> Option<(u32, u16)> {
    for (object_id, (offset, generation)) in object_offsets {
        let Some(end_relative) = find_subslice(&pdf_bytes[*offset..], b"endobj") else {
            continue;
        };
        let object_bytes = &pdf_bytes[*offset..*offset + end_relative];
        if object_bytes
            .windows(b"/Type".len())
            .any(|window| window == b"/Type")
            && object_bytes
                .windows(b"/Catalog".len())
                .any(|window| window == b"/Catalog")
        {
            return Some((*object_id, *generation));
        }
    }
    None
}

fn parse_unsigned_pdf_integer(pdf_bytes: &[u8], start: usize) -> Option<(u32, usize)> {
    let mut end = start;
    while end < pdf_bytes.len() && pdf_bytes[end].is_ascii_digit() {
        end += 1;
    }
    if end == start {
        return None;
    }
    let value = std::str::from_utf8(&pdf_bytes[start..end])
        .ok()?
        .parse()
        .ok()?;
    Some((value, end))
}

fn skip_pdf_whitespace(pdf_bytes: &[u8], start: usize) -> usize {
    let mut index = start;
    while index < pdf_bytes.len()
        && matches!(pdf_bytes[index], b' ' | b'\t' | b'\n' | b'\r' | 0x0c | 0x00)
    {
        index += 1;
    }
    index
}

fn skip_required_pdf_whitespace(pdf_bytes: &[u8], start: usize) -> Option<usize> {
    let end = skip_pdf_whitespace(pdf_bytes, start);
    (end > start).then_some(end)
}

fn is_pdf_name_char(byte: u8) -> bool {
    !matches!(
        byte,
        b' ' | b'\t'
            | b'\n'
            | b'\r'
            | 0x0c
            | 0x00
            | b'('
            | b')'
            | b'<'
            | b'>'
            | b'['
            | b']'
            | b'{'
            | b'}'
            | b'/'
            | b'%'
    )
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
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

fn request_message_contains_image(message: &super::types::Message) -> bool {
    match &message.content {
        serde_json::Value::Array(items) => items
            .iter()
            .any(|item| item.get("type").and_then(|value| value.as_str()) == Some("image")),
        _ => false,
    }
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
    let mut events = Vec::new();
    let mut normalized_messages = Vec::with_capacity(last_user_idx + 1);
    let mut message_index_map = Vec::with_capacity(last_user_idx + 1);

    for (message_index, message) in req.messages.iter().enumerate() {
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

        if let Some(normalized) = normalize_message(message, message_index, &mut events)? {
            message_index_map.push(message_index);
            normalized_messages.push(normalized);
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
            system: req.system.clone(),
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

pub fn classify_tool_name_rewrite_reason(name: &str) -> &'static str {
    let has_unsupported_characters = name
        .chars()
        .any(|ch| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'));
    match (has_unsupported_characters, name.len() > TOOL_NAME_MAX_LEN) {
        (true, true) => "unsupported_characters_and_length",
        (true, false) => "unsupported_characters",
        (false, true) => "length_limit",
        (false, false) => "unchanged",
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

    let agent_continuation_id = Uuid::new_v4().to_string();
    let mut user_input = merge_current_user_messages(&current_messages, &model_id)?;
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
            .with_agent_continuation_id(agent_continuation_id)
            .with_agent_task_type("vibe")
            .with_chat_trigger_type("MANUAL")
            .with_current_message(CurrentMessage::new(user_input))
            .with_history(history),
        tool_name_map,
        session_tracking: resolved_conversation.session_tracking,
        has_history_images,
        structured_output_tool_name,
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

// Extracts text, images, and tool_results from a message's polymorphic
// `content` field (string or array of typed blocks).
fn process_message_content(
    content: &serde_json::Value,
) -> Result<(String, Vec<KiroImage>, Vec<ToolResult>), ConversionError> {
    let mut text_parts = Vec::new();
    let mut images = Vec::new();
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
                                if let Some(format) = get_image_format(&source.media_type) {
                                    images.push(KiroImage::from_base64(format, source.data));
                                }
                            }
                        },
                        "tool_result" => {
                            if let Some(tool_use_id) = block.tool_use_id {
                                let result_content = extract_tool_result_content(&block.content);
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
    Ok((text_parts.join("\n"), images, tool_results))
}

fn get_image_format(media_type: &str) -> Option<String> {
    match media_type {
        "image/jpeg" => Some("jpeg".to_string()),
        "image/png" => Some("png".to_string()),
        "image/gif" => Some("gif".to_string()),
        "image/webp" => Some("webp".to_string()),
        _ => None,
    }
}

pub fn extract_tool_result_content(content: &Option<serde_json::Value>) -> String {
    match content {
        Some(serde_json::Value::String(text)) => text.clone(),
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .filter_map(|item| item.get("text").and_then(|value| value.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        Some(value) => value.to_string(),
        None => String::new(),
    }
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
                let has_tool_results = !context.tool_results.is_empty();
                if has_content || has_images || has_tool_results {
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

// Generates the XML thinking-mode prefix to inject into the system prompt
// based on the request's thinking configuration.
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
    content.contains("<thinking_mode>") || content.contains("<max_thinking_length>")
}

fn requested_model_identity_id(model: &str) -> &str {
    model.strip_suffix("-thinking").unwrap_or(model)
}

fn requested_model_identity_name(model: &str) -> Option<&'static str> {
    match requested_model_identity_id(model) {
        "claude-opus-4-7" => Some("Opus 4.7"),
        "claude-opus-4-6" => Some("Opus 4.6"),
        "claude-sonnet-4-6" => Some("Sonnet 4.6"),
        "claude-sonnet-4-5" => Some("Sonnet 4.5"),
        "claude-haiku-4-5" => Some("Haiku 4.5"),
        _ => None,
    }
}

fn normalize_claude_code_model_identity(content: String, requested_model: &str) -> String {
    let Some(model_name) = requested_model_identity_name(requested_model) else {
        return content;
    };
    let model_id = requested_model_identity_id(requested_model);
    let replacement = format!(
        "You are powered by the model named {model_name}. The exact model ID is {model_id}."
    );
    content
        .lines()
        .map(|line| {
            let trimmed = line.trim_start();
            if trimmed.contains("You are powered by the model named")
                && trimmed.contains("The exact model ID is")
            {
                let indent = &line[..line.len() - trimmed.len()];
                format!("{indent}{replacement}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn strip_volatile_claude_code_billing_header(content: String) -> String {
    let mut line_start = 0;
    while line_start < content.len() {
        let line_end = content[line_start..]
            .find('\n')
            .map(|offset| line_start + offset)
            .unwrap_or(content.len());
        let next_line_start = if line_end < content.len() { line_end + 1 } else { line_end };
        let line = &content[line_start..line_end];
        if line.starts_with(CLAUDE_CODE_BILLING_HEADER_PREFIX) {
            let remainder = content[next_line_start..].trim_start();
            if starts_with_claude_identity(remainder) {
                let prefix = &content[..line_start];
                return if prefix.is_empty() {
                    remainder.to_string()
                } else if prefix.ends_with('\n') {
                    format!("{prefix}{remainder}")
                } else {
                    format!("{prefix}\n{remainder}")
                };
            }
        }
        if line_end == content.len() {
            break;
        }
        line_start = next_line_start;
    }
    content
}

fn starts_with_claude_identity(content: &str) -> bool {
    content.starts_with(CLAUDE_CODE_CLI_SYSTEM_IDENTITY_LINE)
        || content.starts_with(CLAUDE_AGENT_SDK_SYSTEM_IDENTITY_LINE)
}

fn build_injected_system_content(
    req: &MessagesRequest,
    structured_output_tool_name: Option<&str>,
) -> Option<String> {
    let thinking_prefix = generate_thinking_prefix(req);
    let system_content = req
        .system
        .as_ref()
        .map(|system| {
            system
                .iter()
                .map(|message| message.text.clone())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .filter(|content| !content.is_empty())
        .map(strip_volatile_claude_code_billing_header)
        .map(|content| normalize_claude_code_model_identity(content, &req.model))
        .map(|content| format!("{content}\n{SYSTEM_CHUNKED_POLICY}"));

    let mut parts = Vec::new();
    match (thinking_prefix, system_content) {
        (Some(prefix), Some(content)) => {
            if has_thinking_tags(&content) {
                parts.push(content);
            } else {
                parts.push(format!("{prefix}\n{content}"));
            }
        },
        (Some(prefix), None) => parts.push(prefix),
        (None, Some(content)) => parts.push(content),
        (None, None) => {},
    }
    if let Some(tool_name) = structured_output_tool_name {
        parts.push(structured_output_instruction(tool_name));
    }
    (!parts.is_empty()).then(|| parts.join("\n"))
}

// Builds the Kiro history from Anthropic messages that precede the current
// trailing user turn. Injects system prompt (with thinking prefix) as a
// synthetic user/assistant turn pair at the start, then merges consecutive
// same-role messages into single turns.
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
    let mut tool_results = Vec::new();
    for message in messages {
        let (text, message_images, message_tool_results) =
            process_message_content(&message.content)?;
        if !text.is_empty() {
            content_parts.push(text);
        }
        images.extend(message_images);
        tool_results.extend(message_tool_results);
    }
    let content = content_parts.join("\n");
    let mut user_message = UserInputMessage::new(&content, model_id);
    if !images.is_empty() {
        user_message = user_message.with_images(images);
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
    let mut tool_results = Vec::new();
    for message in messages {
        let (text, message_images, message_tool_results) =
            process_message_content(&message.content)?;
        if !text.is_empty() {
            content_parts.push(text);
        }
        let _ = message_images;
        tool_results.extend(message_tool_results);
    }
    let content = content_parts.join("\n");
    let mut user_message = UserMessage::new(&content, model_id);
    // Kiro rejects replaying image payloads inside history user turns. We keep
    // the textual turn content and rely on a stable upstream session for any
    // prior multimodal context.
    if !tool_results.is_empty() {
        user_message = user_message
            .with_context(UserInputMessageContext::new().with_tool_results(tool_results));
    }
    Ok(HistoryUserMessage {
        user_input_message: user_message,
    })
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

    #[test]
    fn get_context_window_size_matches_latest_kiro_model_rules() {
        assert_eq!(get_context_window_size("claude-sonnet-4-6"), 1_000_000);
        assert_eq!(get_context_window_size("claude-opus-4-20250514"), 1_000_000);
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
        assert_eq!(result.conversation_state.history.len(), 2);
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

        let history_tool_name = match &result.conversation_state.history[1] {
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

        let history_tool_name = match &result.conversation_state.history[1] {
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

        let history_tool_name = match &result.conversation_state.history[1] {
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
    fn convert_request_injects_enabled_thinking_budget_prefix() {
        let mut req = base_request(vec![AnthropicMessage {
            role: "user".to_string(),
            content: serde_json::json!("Hello"),
        }]);
        req.thinking = Some(crate::anthropic::types::Thinking {
            thinking_type: "enabled".to_string(),
            budget_tokens: 4096,
        });

        let result = convert_request(&req).expect("conversion should succeed");
        let system_prefix = match &result.conversation_state.history[0] {
            Message::User(message) => &message.user_input_message.content,
            other => panic!("expected injected system user message, got {other:?}"),
        };

        assert!(system_prefix.contains("<thinking_mode>enabled</thinking_mode>"));
        assert!(system_prefix.contains("<max_thinking_length>4096</max_thinking_length>"));
    }

    #[test]
    fn preserves_thinking_effort_when_output_config_is_supplied() {
        let mut req = base_request(vec![AnthropicMessage {
            role: "user".to_string(),
            content: serde_json::json!("Hello"),
        }]);
        req.thinking = Some(crate::anthropic::types::Thinking {
            thinking_type: "adaptive".to_string(),
            budget_tokens: 20_000,
        });
        req.output_config = Some(crate::anthropic::types::OutputConfig {
            effort: Some("medium".to_string()),
            format: None,
        });

        let result = convert_request(&req).expect("conversion should succeed");
        let system_prefix = match &result.conversation_state.history[0] {
            Message::User(message) => &message.user_input_message.content,
            other => panic!("expected injected system user message, got {other:?}"),
        };

        assert!(system_prefix.contains("<thinking_mode>adaptive</thinking_mode>"));
        assert!(system_prefix.contains("<thinking_effort>medium</thinking_effort>"));
    }

    #[test]
    fn convert_request_defaults_adaptive_thinking_effort_to_xhigh() {
        let mut req = base_request(vec![AnthropicMessage {
            role: "user".to_string(),
            content: serde_json::json!("Hello"),
        }]);
        req.thinking = Some(crate::anthropic::types::Thinking {
            thinking_type: "adaptive".to_string(),
            budget_tokens: 20_000,
        });

        let result = convert_request(&req).expect("conversion should succeed");
        let system_prefix = match &result.conversation_state.history[0] {
            Message::User(message) => &message.user_input_message.content,
            other => panic!("expected injected system user message, got {other:?}"),
        };

        assert!(system_prefix.contains("<thinking_effort>xhigh</thinking_effort>"));
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
    fn convert_request_strips_volatile_claude_code_billing_header_before_upstream() {
        let mut req = base_request(vec![AnthropicMessage {
            role: "user".to_string(),
            content: serde_json::json!("Hello"),
        }]);
        req.model = "claude-opus-4-6".to_string();
        req.thinking = Some(crate::anthropic::types::Thinking {
            thinking_type: "adaptive".to_string(),
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

        assert!(system_prefix.contains("<thinking_effort>high</thinking_effort>"));
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
        assert!(current.origin.is_none());
    }

    #[test]
    fn convert_request_extracts_pdf_documents_into_user_text() {
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

        let result = convert_request(&req).expect("pdf document block should be converted");
        let current = &result.conversation_state.current_message.user_input_message;
        assert!(current.content.contains("hvoywpkd"));
        assert!(current.content.contains("What text does this PDF contain?"));
        assert!(current.images.is_empty());
    }

    #[test]
    fn convert_request_extracts_text_documents_into_user_text() {
        let req = base_request(vec![AnthropicMessage {
            role: "user".to_string(),
            content: serde_json::json!([
                {
                    "type": "document",
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

        let result = convert_request(&req).expect("text document block should be converted");
        let current = &result.conversation_state.current_message.user_input_message;
        assert!(current
            .content
            .contains("<document media_type=\"text/plain\">"));
        assert!(current.content.contains("plain document body"));
        assert!(current.content.contains("Summarize the text document."));
        assert!(current.images.is_empty());
    }

    #[test]
    fn convert_request_drops_images_from_history_turns() {
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
        let history_user = match &result.conversation_state.history[0] {
            Message::User(message) => &message.user_input_message,
            other => panic!("expected user history entry, got {other:?}"),
        };
        assert_eq!(history_user.content, "Describe this image");
        assert!(history_user.images.is_empty());
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
        assert_eq!(result.conversation_state.history.len(), 2);
        let assistant = match &result.conversation_state.history[1] {
            Message::Assistant(message) => &message.assistant_response_message,
            other => panic!("expected assistant history entry, got {other:?}"),
        };
        assert!(assistant.content.contains("I'll search for StaticFlow."));
        assert!(assistant.content.contains("StaticFlow result summary"));
        assert!(assistant.tool_uses.is_none());
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

        assert_eq!(result.conversation_state.history.len(), 2);
        let assistant = match &result.conversation_state.history[1] {
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

        assert_eq!(result.conversation_state.history.len(), 2);
        let assistant = match &result.conversation_state.history[1] {
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
        let assistant = match &result.conversation_state.history[1] {
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
        assert_eq!(result.conversation_state.history.len(), 2);
        let first_user = match &result.conversation_state.history[0] {
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
        assert_eq!(result.conversation_state.history.len(), 1);
        match &result.conversation_state.history[0] {
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
