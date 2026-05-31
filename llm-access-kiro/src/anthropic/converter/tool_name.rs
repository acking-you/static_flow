//! Tool name and tool-use-id normalization: sanitization, length-limit
//! truncation, hashed aliasing, and duplicate tool-use-id rewriting.

use std::collections::{HashMap, HashSet};

use sha2::{Digest, Sha256};

use super::{
    invalid_request, schema::permissive_object_schema, ActiveToolUse, ConversionError,
    NormalizedRequest, ToolUseIdRewrite, TOOL_NAME_MAX_LEN,
};
use crate::wire::{InputSchema, Message, Tool, ToolSpecification};

pub fn rewrite_duplicate_tool_use_ids(
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

fn collect_existing_tool_use_ids(messages: &[crate::anthropic::types::Message]) -> HashSet<String> {
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

// Collects unique tool names from assistant messages in history, used to
// synthesize placeholder tool specs for tools referenced only in history.
pub fn collect_history_tool_names(history: &[Message]) -> Vec<String> {
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
pub fn create_placeholder_tool(name: &str) -> Tool {
    Tool {
        tool_specification: ToolSpecification {
            name: name.to_string(),
            description: "Tool used in conversation history".to_string(),
            input_schema: InputSchema::from_json(permissive_object_schema()),
        },
    }
}

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
pub fn map_tool_name(name: &str, tool_name_map: &mut HashMap<String, String>) -> String {
    let sanitized = sanitize_tool_name(name);
    if sanitized == name && name.len() <= TOOL_NAME_MAX_LEN {
        return name.to_string();
    }

    let alias = make_hashed_tool_name_alias(name, &sanitized);
    tool_name_map.insert(alias.clone(), name.to_string());
    alias
}


#[cfg(test)]
mod tests {
    use super::{super::TOOL_NAME_MAX_LEN, *};

    #[test]
    fn shorten_tool_name_is_deterministic_and_bounded() {
        let long_name =
            "tool_with_a_name_far_beyond_the_supported_sixty_three_character_limit_for_kiro";
        let short1 = shorten_tool_name(long_name);
        let short2 = shorten_tool_name(long_name);

        assert_eq!(short1, short2);
        assert!(short1.len() <= TOOL_NAME_MAX_LEN);
    }
}
