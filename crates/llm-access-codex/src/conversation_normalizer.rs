//! Canonical Codex conversation repair before upstream validation.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde_json::{json, Map, Value};

use crate::{error::CodexGatewayResult, request::extract_non_empty_string};

#[derive(Debug)]
struct PendingCall {
    original_call_id: String,
    retained_index: usize,
}

/// Repair responses-style input items in place before final validation.
pub fn repair_responses_request(root: &mut Map<String, Value>) -> CodexGatewayResult<()> {
    let Some(input) = root.get_mut("input") else {
        return Ok(());
    };
    let Some(items) = input.as_array_mut() else {
        return Ok(());
    };

    let original_items = std::mem::take(items);
    let mut repaired = Vec::with_capacity(items.len());
    let mut pending_by_normalized = BTreeMap::<String, PendingCall>::new();
    let mut pending_order_by_original = BTreeMap::<String, VecDeque<String>>::new();
    let mut seen_call_counts = BTreeMap::<String, usize>::new();
    let mut paired_call_indices = BTreeSet::<usize>::new();
    let mut next_message_id = 0usize;

    for item in original_items {
        let Value::Object(obj) = item else {
            repaired.push(item);
            continue;
        };
        let item_type = obj
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        match item_type.as_str() {
            "message" => {
                if let Some(message) = repair_message_item(obj, &mut next_message_id) {
                    repaired.push(Value::Object(message));
                }
            },
            "function_call" | "custom_tool_call" => {
                let Some(mut call) = repair_function_call_item(obj, item_type.as_str()) else {
                    continue;
                };
                let Some(original_call_id) =
                    extract_non_empty_string(call.get("call_id").or_else(|| call.get("id")))
                        .map(ToString::to_string)
                else {
                    continue;
                };
                let normalized_call_id =
                    normalized_call_id(&original_call_id, &mut seen_call_counts);
                call.insert("call_id".to_string(), Value::String(normalized_call_id.clone()));
                if call.contains_key("id") {
                    call.insert("id".to_string(), Value::String(normalized_call_id.clone()));
                }
                let retained_index = repaired.len();
                repaired.push(Value::Object(call));
                pending_by_normalized.insert(normalized_call_id.clone(), PendingCall {
                    original_call_id: original_call_id.clone(),
                    retained_index,
                });
                pending_order_by_original
                    .entry(original_call_id)
                    .or_default()
                    .push_back(normalized_call_id);
            },
            "function_call_output" | "custom_tool_call_output" => {
                let Some(mut output) = repair_function_call_output_item(obj, item_type.as_str())
                else {
                    continue;
                };
                let Some(raw_call_id) =
                    extract_non_empty_string(output.get("call_id")).map(ToString::to_string)
                else {
                    continue;
                };
                let Some((normalized_call_id, retained_index)) = resolve_pending_call(
                    &raw_call_id,
                    &mut pending_by_normalized,
                    &mut pending_order_by_original,
                ) else {
                    repaired.push(Value::Object(output));
                    continue;
                };
                output.insert("call_id".to_string(), Value::String(normalized_call_id));
                repaired.push(Value::Object(output));
                paired_call_indices.insert(retained_index);
            },
            _ => repaired.push(Value::Object(obj)),
        }
    }

    let unresolved = pending_by_normalized
        .values()
        .map(|entry| entry.retained_index)
        .collect::<BTreeSet<_>>();
    *items = repaired
        .into_iter()
        .enumerate()
        .filter_map(|(index, item)| {
            if unresolved.contains(&index) && !paired_call_indices.contains(&index) {
                None
            } else {
                Some(item)
            }
        })
        .collect();
    Ok(())
}

fn repair_message_item(
    mut obj: Map<String, Value>,
    next_message_id: &mut usize,
) -> Option<Map<String, Value>> {
    let role = obj.get("role").and_then(Value::as_str)?.trim().to_string();
    if role.is_empty() {
        return None;
    }
    let content = obj.remove("content")?;
    let repaired_content = repair_message_content(content, role.as_str())?;
    obj.insert("content".to_string(), Value::Array(repaired_content));

    if let Some(raw_id) = extract_non_empty_string(obj.get("id")) {
        if !raw_id.starts_with("msg_") {
            obj.insert("id".to_string(), Value::String(next_message_id_value(next_message_id)));
        }
    }
    Some(obj)
}

fn repair_message_content(content: Value, role: &str) -> Option<Vec<Value>> {
    let role_part_type = if role == "assistant" { "output_text" } else { "input_text" };
    let mut repaired = Vec::new();
    match content {
        Value::String(text) => {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                repaired.push(json!({
                    "type": role_part_type,
                    "text": trimmed,
                }));
            }
        },
        Value::Array(items) => {
            for item in items {
                if let Some(part) = repair_message_content_part(item, role_part_type, role) {
                    repaired.push(part);
                }
            }
        },
        Value::Object(_) => {
            if let Some(part) = repair_message_content_part(content, role_part_type, role) {
                repaired.push(part);
            }
        },
        Value::Null => {},
        other => {
            let text = other.to_string();
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                repaired.push(json!({
                    "type": role_part_type,
                    "text": trimmed,
                }));
            }
        },
    }
    (!repaired.is_empty()).then_some(repaired)
}

fn repair_message_content_part(item: Value, role_part_type: &str, role: &str) -> Option<Value> {
    if let Value::String(text) = item {
        let trimmed = text.trim();
        return (!trimmed.is_empty()).then(|| {
            json!({
                "type": role_part_type,
                "text": trimmed,
            })
        });
    }
    let Value::Object(obj) = item else {
        return None;
    };
    let item_type = obj.get("type").and_then(Value::as_str).unwrap_or_default();
    match item_type {
        "text" | "input_text" | "output_text" => obj
            .get("text")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|text| {
                json!({
                    "type": role_part_type,
                    "text": text,
                })
            }),
        "input_image" | "image_url" if role != "assistant" => Some(Value::Object(obj)),
        _ => None,
    }
}

fn repair_function_call_item(
    mut obj: Map<String, Value>,
    item_type: &str,
) -> Option<Map<String, Value>> {
    let call_id = extract_non_empty_string(obj.get("call_id").or_else(|| obj.get("id")))?;
    let name = extract_non_empty_string(obj.get("name"))?;
    let mut out = Map::new();
    out.insert("type".to_string(), Value::String(item_type.to_string()));
    out.insert("call_id".to_string(), Value::String(call_id.to_string()));
    out.insert("name".to_string(), Value::String(name.to_string()));
    match item_type {
        "custom_tool_call" => {
            let input = obj
                .remove("input")
                .or_else(|| obj.remove("arguments"))
                .unwrap_or_else(|| Value::String(String::new()));
            out.insert("input".to_string(), normalize_custom_tool_input(input));
        },
        _ => {
            let arguments = obj
                .remove("arguments")
                .or_else(|| obj.remove("input"))
                .unwrap_or_else(|| Value::String("{}".to_string()));
            out.insert("arguments".to_string(), normalize_arguments(arguments));
        },
    }
    Some(out)
}

fn repair_function_call_output_item(
    mut obj: Map<String, Value>,
    item_type: &str,
) -> Option<Map<String, Value>> {
    let call_id = extract_non_empty_string(obj.get("call_id"))?;
    let mut out = Map::new();
    out.insert("type".to_string(), Value::String(item_type.to_string()));
    out.insert("call_id".to_string(), Value::String(call_id.to_string()));
    out.insert(
        "output".to_string(),
        obj.remove("output")
            .unwrap_or_else(|| Value::String(String::new())),
    );
    Some(out)
}

fn normalize_arguments(arguments: Value) -> Value {
    match arguments {
        Value::String(text) => Value::String(text),
        other => Value::String(serde_json::to_string(&other).unwrap_or_else(|_| "{}".to_string())),
    }
}

fn normalize_custom_tool_input(input: Value) -> Value {
    match input {
        Value::String(text) => Value::String(text),
        other => Value::String(serde_json::to_string(&other).unwrap_or_else(|_| String::new())),
    }
}

fn normalized_call_id(original: &str, seen_counts: &mut BTreeMap<String, usize>) -> String {
    let count = seen_counts.entry(original.to_string()).or_insert(0);
    *count += 1;
    if *count == 1 {
        original.to_string()
    } else {
        format!("{original}__sf_{count}")
    }
}

fn resolve_pending_call(
    raw_call_id: &str,
    pending_by_normalized: &mut BTreeMap<String, PendingCall>,
    pending_order_by_original: &mut BTreeMap<String, VecDeque<String>>,
) -> Option<(String, usize)> {
    if let Some(entry) = pending_by_normalized.remove(raw_call_id) {
        if let Some(queue) = pending_order_by_original.get_mut(&entry.original_call_id) {
            while queue.front().is_some_and(|value| value == raw_call_id) {
                queue.pop_front();
            }
        }
        return Some((raw_call_id.to_string(), entry.retained_index));
    }

    let queue = pending_order_by_original.get_mut(raw_call_id)?;
    while let Some(normalized_id) = queue.pop_front() {
        if let Some(entry) = pending_by_normalized.remove(&normalized_id) {
            return Some((normalized_id, entry.retained_index));
        }
    }
    None
}

fn next_message_id_value(next_message_id: &mut usize) -> String {
    let value = format!("msg_sf_{}", *next_message_id);
    *next_message_id += 1;
    value
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::repair_responses_request;

    #[test]
    fn repair_drops_empty_messages_and_rewrites_message_ids() {
        let mut root = serde_json::Map::new();
        root.insert(
            "input".to_string(),
            json!([
                {"type":"message","role":"user","content":"   "},
                {"type":"message","role":"assistant","id":"item_bad","content":[{"type":"output_text","text":"pong"}]}
            ]),
        );

        repair_responses_request(&mut root).expect("repair should succeed");

        let input = root["input"].as_array().expect("input array");
        assert_eq!(input.len(), 1);
        assert_eq!(input[0]["id"].as_str().unwrap_or_default(), "msg_sf_0");
    }

    #[test]
    fn repair_pairs_duplicate_function_calls_and_drops_orphans() {
        let mut root = serde_json::Map::new();
        root.insert(
            "input".to_string(),
            json!([
                {"type":"function_call","call_id":"callauto12","name":"lookup","arguments":"{}"},
                {"type":"function_call","call_id":"callauto12","name":"lookup","arguments":"{}"},
                {"type":"function_call_output","call_id":"callauto12","output":"ok"}
            ]),
        );

        repair_responses_request(&mut root).expect("repair should succeed");

        let input = root["input"].as_array().expect("input array");
        assert_eq!(input.len(), 2);
        assert_eq!(input[0]["call_id"], json!("callauto12"));
        assert_eq!(input[1]["call_id"], json!("callauto12"));
    }

    #[test]
    fn repair_rewrites_second_output_to_rewritten_duplicate_call_id() {
        let mut root = serde_json::Map::new();
        root.insert(
            "input".to_string(),
            json!([
                {"type":"function_call","call_id":"callauto12","name":"lookup","arguments":"{}"},
                {"type":"function_call_output","call_id":"callauto12","output":"first"},
                {"type":"function_call","call_id":"callauto12","name":"lookup","arguments":"{}"},
                {"type":"function_call_output","call_id":"callauto12","output":"second"}
            ]),
        );

        repair_responses_request(&mut root).expect("repair should succeed");

        let input = root["input"].as_array().expect("input array");
        assert_eq!(input[0]["call_id"], json!("callauto12"));
        assert_eq!(input[1]["call_id"], json!("callauto12"));
        assert_eq!(input[2]["call_id"], json!("callauto12__sf_2"));
        assert_eq!(input[3]["call_id"], json!("callauto12__sf_2"));
    }

    #[test]
    fn repair_custom_tool_call_preserves_input_contract() {
        let mut root = serde_json::Map::new();
        root.insert(
            "input".to_string(),
            json!([
                {"type":"custom_tool_call","call_id":"callcustom1","name":"apply_patch","input":"*** Begin Patch"},
                {"type":"custom_tool_call_output","call_id":"callcustom1","output":"ok"}
            ]),
        );

        repair_responses_request(&mut root).expect("repair should succeed");

        let input = root["input"].as_array().expect("input array");
        assert_eq!(input[0]["type"], json!("custom_tool_call"));
        assert_eq!(input[0]["input"], json!("*** Begin Patch"));
        assert!(input[0].get("arguments").is_none());
    }

    #[test]
    fn repair_custom_tool_call_recovers_buggy_arguments_field() {
        let mut root = serde_json::Map::new();
        root.insert(
            "input".to_string(),
            json!([
                {"type":"custom_tool_call","call_id":"callcustom1","name":"apply_patch","arguments":"*** Begin Patch"},
                {"type":"custom_tool_call_output","call_id":"callcustom1","output":"ok"}
            ]),
        );

        repair_responses_request(&mut root).expect("repair should succeed");

        let input = root["input"].as_array().expect("input array");
        assert_eq!(input[0]["type"], json!("custom_tool_call"));
        assert_eq!(input[0]["input"], json!("*** Begin Patch"));
        assert!(input[0].get("arguments").is_none());
    }
}
