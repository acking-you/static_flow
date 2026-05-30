//! Request normalization pipeline: per-message normalization, normalization
//! event recording, and the top-level `normalize_request` entry point.

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

pub(crate) fn push_normalization_event(
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
pub(crate) fn normalize_message(
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
    let current_turn = current_user_message_range(&req.messages)?;
    let current_user_start = current_turn.start;
    let last_user_idx = current_turn.end - 1;
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
