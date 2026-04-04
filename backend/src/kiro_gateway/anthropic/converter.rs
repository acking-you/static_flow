//! Converts Anthropic Messages API requests into Kiro wire `ConversationState`.
//!
//! Handles model name mapping, system prompt injection, thinking mode prefixes,
//! tool schema normalization, conversation history building (with consecutive
//! same-role message merging), and tool-result pairing validation.

use std::collections::{HashMap, HashSet};

use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::types::{ContentBlock, MessagesRequest};
use crate::kiro_gateway::wire::{
    AssistantMessage, ConversationState, CurrentMessage, HistoryAssistantMessage,
    HistoryUserMessage, InputSchema, KiroImage, Message, Tool, ToolResult, ToolSpecification,
    ToolUseEntry, UserInputMessage, UserInputMessageContext, UserMessage,
};

// Ensures a JSON schema object has all required top-level fields
// (type, properties, required, additionalProperties) so Kiro's
// tool validation does not reject it.
fn normalize_json_schema(schema: serde_json::Value) -> serde_json::Value {
    let serde_json::Value::Object(mut obj) = schema else {
        return serde_json::json!({
            "type": "object",
            "properties": {},
            "required": [],
            "additionalProperties": true
        });
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
    pub tool_use_id_rewrites: Vec<ToolUseIdRewrite>,
}

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

#[derive(Debug)]
struct ActiveToolUse {
    normalized_id: String,
    rewrite_index: Option<usize>,
}

#[derive(Debug)]
struct NormalizedRequest {
    request: MessagesRequest,
    tool_use_id_rewrites: Vec<ToolUseIdRewrite>,
}

fn normalize_tool_use_ids(req: &MessagesRequest) -> Result<NormalizedRequest, ConversionError> {
    let mut request = req.clone();
    if request
        .messages
        .last()
        .is_some_and(|message| message.role != "user")
    {
        let last_user_idx = request
            .messages
            .iter()
            .rposition(|message| message.role == "user")
            .ok_or(ConversionError::EmptyMessages)?;
        request.messages.truncate(last_user_idx + 1);
    }

    let mut used_ids = collect_existing_tool_use_ids(&request.messages);
    let mut seen_counts = HashMap::<String, usize>::new();
    let mut active_by_original = HashMap::<String, ActiveToolUse>::new();
    let mut rewrites = Vec::<ToolUseIdRewrite>::new();

    for (message_index, message) in request.messages.iter_mut().enumerate() {
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
                            "message {message_index} tool_use block {block_index} reuses \
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
                            assistant_message_index: message_index,
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
                                    "message {message_index} tool_result block {block_index} \
                                     references duplicate tool_use id `{original_id}` after its \
                                     rewritten call already completed"
                                )));
                            }
                        },
                    }
                }
            },
            _ => {},
        }
    }

    Ok(NormalizedRequest {
        request,
        tool_use_id_rewrites: rewrites,
    })
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
            input_schema: InputSchema::from_json(serde_json::json!({
                "$schema": "http://json-schema.org/draft-07/schema#",
                "type": "object",
                "properties": {},
                "required": [],
                "additionalProperties": true
            })),
        },
    }
}

const TOOL_NAME_MAX_LEN: usize = 63;

fn shorten_tool_name(name: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(name.as_bytes());
    let hash_hex = format!("{:x}", hasher.finalize());
    let hash_suffix = &hash_hex[..8];
    let prefix_max = TOOL_NAME_MAX_LEN - 1 - 8;
    let prefix = match name.char_indices().nth(prefix_max) {
        Some((idx, _)) => &name[..idx],
        None => name,
    };
    format!("{prefix}_{hash_suffix}")
}

fn map_tool_name(name: &str, tool_name_map: &mut HashMap<String, String>) -> String {
    if name.len() <= TOOL_NAME_MAX_LEN {
        return name.to_string();
    }

    let short = shorten_tool_name(name);
    tool_name_map.insert(short.clone(), name.to_string());
    short
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

pub fn convert_request_with_validation(
    req: &MessagesRequest,
    request_validation_enabled: bool,
) -> Result<ConversionResult, ConversionError> {
    let normalized = normalize_tool_use_ids(req)?;
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

    // Reuse session UUID from metadata as conversation_id for continuity;
    // fall back to a fresh UUID.
    let conversation_id = req
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.user_id.as_deref())
        .and_then(extract_session_id)
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let agent_continuation_id = Uuid::new_v4().to_string();
    let last_message = messages.last().ok_or(ConversionError::EmptyMessages)?;
    let (text_content, images, tool_results) = process_message_content(&last_message.content)?;
    let mut tool_name_map = HashMap::new();
    let mut tools = convert_tools(&req.tools, &mut tool_name_map);
    let mut history = build_history(req, messages, &model_id, &mut tool_name_map)?;
    let (validated_tool_results, orphaned_tool_use_ids) =
        validate_tool_pairing(&history, &tool_results);
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

    let mut context = UserInputMessageContext::new();
    if !tools.is_empty() {
        context = context.with_tools(tools);
    }
    if !validated_tool_results.is_empty() {
        context = context.with_tool_results(validated_tool_results);
    }

    let mut user_input = UserInputMessage::new(text_content, &model_id)
        .with_context(context)
        .with_origin("AI_EDITOR");
    if !images.is_empty() {
        user_input = user_input.with_images(images);
    }

    Ok(ConversionResult {
        conversation_state: ConversationState::new(conversation_id)
            .with_agent_continuation_id(agent_continuation_id)
            .with_agent_task_type("vibe")
            .with_chat_trigger_type("MANUAL")
            .with_current_message(CurrentMessage::new(user_input))
            .with_history(history),
        tool_name_map,
        tool_use_id_rewrites: normalized.tool_use_id_rewrites,
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

fn extract_tool_result_content(content: &Option<serde_json::Value>) -> String {
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

// Generates the XML thinking-mode prefix to inject into the system prompt
// based on the request's thinking configuration.
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
                .map(|config| config.effort.as_str())
                .unwrap_or("high");
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

// Builds the Kiro history from Anthropic messages. Injects system prompt
// (with thinking prefix) as a synthetic user/assistant turn pair at the
// start. Merges consecutive same-role messages into single turns.
fn build_history(
    req: &MessagesRequest,
    messages: &[super::types::Message],
    model_id: &str,
    tool_name_map: &mut HashMap<String, String>,
) -> Result<Vec<Message>, ConversionError> {
    let mut history = Vec::new();
    let thinking_prefix = generate_thinking_prefix(req);

    if let Some(system) = &req.system {
        let system_content = system
            .iter()
            .map(|message| message.text.clone())
            .collect::<Vec<_>>()
            .join("\n");
        if !system_content.is_empty() {
            let system_content = format!("{system_content}\n{SYSTEM_CHUNKED_POLICY}");
            let final_content = if let Some(prefix) = &thinking_prefix {
                if !has_thinking_tags(&system_content) {
                    format!("{prefix}\n{system_content}")
                } else {
                    system_content
                }
            } else {
                system_content
            };
            history.push(Message::User(HistoryUserMessage::new(final_content, model_id)));
            history.push(Message::Assistant(HistoryAssistantMessage::new(
                "I will follow these instructions.",
            )));
        }
    } else if let Some(prefix) = &thinking_prefix {
        history.push(Message::User(HistoryUserMessage::new(prefix.clone(), model_id)));
        history.push(Message::Assistant(HistoryAssistantMessage::new(
            "I will follow these instructions.",
        )));
    }

    let history_end_index = messages.len().saturating_sub(1);
    let mut user_buffer = Vec::new();
    let mut assistant_buffer = Vec::new();

    for message in messages.iter().take(history_end_index) {
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
    // If history ends with buffered user messages but no following assistant
    // turn, append a synthetic "OK" assistant reply so the history alternates.
    if !user_buffer.is_empty() {
        history.push(Message::User(merge_user_messages(&user_buffer, model_id)?));
        history.push(Message::Assistant(HistoryAssistantMessage::new("OK")));
    }

    Ok(history)
}

fn merge_user_messages(
    messages: &[&super::types::Message],
    model_id: &str,
) -> Result<HistoryUserMessage, ConversionError> {
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
    let mut user_message = UserMessage::new(&content, model_id);
    if !images.is_empty() {
        user_message = user_message.with_images(images);
    }
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
        super::types::{Message as AnthropicMessage, Metadata, Tool as AnthropicTool},
        *,
    };

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
        let (short_name, original_name) = result.tool_name_map.iter().next().unwrap();
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
    fn convert_request_injects_enabled_thinking_budget_prefix() {
        let mut req = base_request(vec![AnthropicMessage {
            role: "user".to_string(),
            content: serde_json::json!("Hello"),
        }]);
        req.thinking = Some(crate::kiro_gateway::anthropic::types::Thinking {
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
    fn convert_request_injects_adaptive_thinking_effort_prefix() {
        let mut req = base_request(vec![AnthropicMessage {
            role: "user".to_string(),
            content: serde_json::json!("Hello"),
        }]);
        req.thinking = Some(crate::kiro_gateway::anthropic::types::Thinking {
            thinking_type: "adaptive".to_string(),
            budget_tokens: 20_000,
        });
        req.output_config = Some(crate::kiro_gateway::anthropic::types::OutputConfig {
            effort: "medium".to_string(),
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
    fn convert_request_defaults_adaptive_thinking_effort_to_high() {
        let mut req = base_request(vec![AnthropicMessage {
            role: "user".to_string(),
            content: serde_json::json!("Hello"),
        }]);
        req.thinking = Some(crate::kiro_gateway::anthropic::types::Thinking {
            thinking_type: "adaptive".to_string(),
            budget_tokens: 20_000,
        });

        let result = convert_request(&req).expect("conversion should succeed");
        let system_prefix = match &result.conversation_state.history[0] {
            Message::User(message) => &message.user_input_message.content,
            other => panic!("expected injected system user message, got {other:?}"),
        };

        assert!(system_prefix.contains("<thinking_effort>high</thinking_effort>"));
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
