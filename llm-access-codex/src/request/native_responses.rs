//! Native `/responses` request handling: instruction injection, upstream
//! field stripping, tool-role message repair, and structural validation.

use super::*;

pub(crate) fn inject_default_instructions_when_missing(root: &mut Map<String, Value>) {
    let needs_default_instructions = match root.get("instructions") {
        None | Some(Value::Null) => true,
        Some(Value::String(value)) => value.trim().is_empty(),
        Some(_) => false,
    };
    if needs_default_instructions {
        root.insert(
            "instructions".to_string(),
            Value::String(codex_default_instructions().to_string()),
        );
    }
}
pub(crate) fn normalize_native_responses_request(path: &str, root: &mut Map<String, Value>) {
    root.remove("max_output_tokens");
    if path == "/v1/responses" {
        remove_native_responses_upstream_unsupported_fields(root);
        normalize_native_responses_input_for_upstream(root);
    }
    if path == "/v1/responses/compact" {
        retain_native_compact_fields(root);
    }
}
pub(crate) fn remove_native_responses_upstream_unsupported_fields(root: &mut Map<String, Value>) {
    for field in NATIVE_RESPONSES_UPSTREAM_UNSUPPORTED_FIELDS {
        root.remove(*field);
    }
}
pub(crate) fn normalize_native_responses_input_for_upstream(root: &mut Map<String, Value>) {
    let Some(input) = root.get_mut("input") else {
        return;
    };
    match input {
        Value::String(text) => {
            let mut content = Map::new();
            content.insert("type".to_string(), Value::String("input_text".to_string()));
            content.insert("text".to_string(), Value::String(text.clone()));

            let mut message = Map::new();
            message.insert("type".to_string(), Value::String("message".to_string()));
            message.insert("role".to_string(), Value::String("user".to_string()));
            message.insert("content".to_string(), Value::Array(vec![Value::Object(content)]));
            *input = Value::Array(vec![Value::Object(message)]);
        },
        Value::Object(_) => {
            let item = std::mem::take(input);
            *input = Value::Array(vec![item]);
        },
        _ => {},
    }
}
pub(crate) fn repair_native_responses_request(
    path: &str,
    root: &mut Map<String, Value>,
) -> CodexGatewayResult<()> {
    if path != "/v1/responses" {
        return Ok(());
    }
    repair_native_responses_tool_role_messages(root)
}
pub(crate) fn repair_native_responses_tool_role_messages(
    root: &mut Map<String, Value>,
) -> CodexGatewayResult<()> {
    let Some(Value::Array(items)) = root.get_mut("input") else {
        return Ok(());
    };

    for item in items {
        let Some(item_obj) = item.as_object() else {
            continue;
        };
        if item_obj.get("role").and_then(Value::as_str) != Some("tool") {
            continue;
        }

        let call_id = extract_non_empty_string(
            item_obj
                .get("call_id")
                .or_else(|| item_obj.get("tool_call_id"))
                .or_else(|| item_obj.get("id")),
        )
        .map(ToString::to_string);

        let repaired = if let Some(call_id) = call_id {
            let output = convert_tool_message_content_to_responses_output(
                item_obj.get("content").or_else(|| item_obj.get("output")),
            )
            .map_err(|err| bad_request_with_detail("Invalid tool content", err))?;
            json!({
                "type": "function_call_output",
                "call_id": call_id,
                "output": output
            })
        } else {
            let mut content_items = item_obj
                .get("content")
                .or_else(|| item_obj.get("output"))
                .map(convert_user_message_content_to_responses_items)
                .unwrap_or_default();
            if content_items.is_empty() {
                content_items.push(json!({
                    "type": "input_text",
                    "text": "(empty)",
                }));
            }
            json!({
                "type": "message",
                "role": "user",
                "content": content_items
            })
        };
        *item = repaired;
    }

    Ok(())
}
pub(crate) fn validate_native_responses_request(
    path: &str,
    root: &Map<String, Value>,
) -> CodexGatewayResult<()> {
    if path != "/v1/responses" {
        return Ok(());
    }
    validate_native_responses_input_roles(root.get("input"))
}
pub(crate) fn validate_native_responses_input_roles(
    input: Option<&Value>,
) -> CodexGatewayResult<()> {
    let Some(Value::Array(items)) = input else {
        return Ok(());
    };

    for (index, item) in items.iter().enumerate() {
        let Some(item_obj) = item.as_object() else {
            continue;
        };
        let Some(role) = item_obj.get("role").and_then(Value::as_str) else {
            continue;
        };
        if NATIVE_RESPONSES_MESSAGE_ROLES.contains(&role) {
            continue;
        }
        if role == "tool" {
            let message = format!(
                "responses input item {index} uses Chat Completions role `tool`; send tool \
                 outputs as `function_call_output` items with `call_id` and `output`"
            );
            return Err(bad_request(&message));
        }
        let message = format!(
            "responses input item {index} has unsupported role `{role}`; supported roles are \
             `assistant`, `system`, `developer`, and `user`"
        );
        return Err(bad_request(&message));
    }

    Ok(())
}
pub(crate) fn strip_input_item_ids(root: &mut Map<String, Value>) -> bool {
    let Some(Value::Array(items)) = root.get_mut("input") else {
        return false;
    };
    let mut removed_any = false;
    for item in items {
        let Some(item_obj) = item.as_object_mut() else {
            continue;
        };
        if item_obj.remove("id").is_some() {
            removed_any = true;
        }
    }
    removed_any
}
pub(crate) fn retain_native_compact_fields(root: &mut Map<String, Value>) {
    root.retain(|key, _| {
        matches!(
            key.as_str(),
            "model"
                | "instructions"
                | "input"
                | "tools"
                | "parallel_tool_calls"
                | "reasoning"
                | "service_tier"
                | "prompt_cache_key"
                | "text"
        )
    });
}
