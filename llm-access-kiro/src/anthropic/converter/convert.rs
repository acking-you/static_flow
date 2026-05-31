//! Conversion pipeline: turns a normalized request into a Kiro
//! `ConversationState` by assembling history and the current user turn,
//! merging consecutive same-role messages, and deduplicating documents.

use std::{
    collections::{HashMap, HashSet},
    ops::Range,
};

#[cfg(test)]
use super::normalize::normalize_request;
#[cfg(test)]
use super::session::resolve_conversation_id_from_metadata;
use super::{
    document::kiro_document_from_block,
    identity::response_identity_for_current_turn,
    image::get_image_format_from_source,
    invalid_request,
    model::map_model,
    schema::{apply_multimodal_tool_schema_compatibility, request_message_contains_image},
    system::build_injected_system_content,
    thinking::apply_thinking_prefix_to_current_turn,
    tool_name::{collect_history_tool_names, create_placeholder_tool, map_tool_name},
    tool_pairing::{
        prune_orphaned_history_tool_results, remove_orphaned_tool_uses, validate_tool_pairing,
    },
    tool_result::{extract_tool_result_attachments, extract_tool_result_content},
    tools::{append_structured_output_tool, convert_tools},
    validate::validate_messages_request,
    ConversionError, ConversionResult, NormalizedRequest, ProcessedMessageContent,
    ResolvedConversationId, EMPTY_DOCUMENT_PLACEHOLDER, EMPTY_TOOL_RESULT_PLACEHOLDER,
    KIRO_MAX_CONVERSATION_DOCUMENTS, KIRO_MAX_CURRENT_MESSAGE_IMAGES,
};
use crate::{
    anthropic::types::{ContentBlock, MessagesRequest},
    wire::{
        AssistantMessage, ConversationState, CurrentMessage, HistoryAssistantMessage,
        HistoryUserMessage, KiroDocument, KiroImage, Message, ToolResult, ToolUseEntry,
        UserInputMessage, UserInputMessageContext, UserMessage,
    },
};

fn trailing_user_message_start(
    messages: &[crate::anthropic::types::Message],
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
    messages: &[crate::anthropic::types::Message],
) -> Result<Range<usize>, ConversionError> {
    let end = messages
        .iter()
        .rposition(|message| message.role == "user")
        .map(|index| index + 1)
        .ok_or_else(|| no_user_message_error(messages))?;
    let start = trailing_user_message_start(&messages[..end])?;
    Ok(start..end)
}

pub fn no_user_message_error(messages: &[crate::anthropic::types::Message]) -> ConversionError {
    if messages.is_empty() {
        ConversionError::EmptyMessages
    } else {
        invalid_request("messages must include at least one user message before assistant prefill")
    }
}

pub fn web_search_tool_result_text(
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
pub fn convert_request_with_validation(
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

// Builds the Kiro history from Anthropic messages that precede the current
// trailing user turn. Injects stable system prompt text as a synthetic
// user/assistant turn pair at the start, then merges consecutive same-role
// messages into single turns.
fn build_history(
    req: &MessagesRequest,
    messages: &[crate::anthropic::types::Message],
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
    messages: &[&crate::anthropic::types::Message],
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
    messages: &[&crate::anthropic::types::Message],
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
    message: &crate::anthropic::types::Message,
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
    messages: &[&crate::anthropic::types::Message],
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

    use super::*;
    use crate::anthropic::types::Message as AnthropicMessage;

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
}
