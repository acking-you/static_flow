//! Native /responses handling: instruction injection, field stripping,
//! tool-role repair, validation.


use serde_json::{json, Map, Value};

use super::{
    chat_completions::{
        convert_tool_message_content_to_responses_output,
        convert_user_message_content_to_responses_items,
    },
    extract_non_empty_string, NATIVE_RESPONSES_MESSAGE_ROLES,
    NATIVE_RESPONSES_UPSTREAM_UNSUPPORTED_FIELDS,
};
use crate::{
    error::{bad_request, bad_request_with_detail, CodexGatewayResult},
    instructions::codex_default_instructions,
};
/// Insert the default Codex instructions when the request omits them.
///
/// The upstream `/responses` API requires a non-empty `instructions` field.
/// When the client sends no `instructions`, a JSON null, or a whitespace-only
/// string, this fills in [`codex_default_instructions`]; a meaningful
/// client-supplied value is left untouched.
pub fn inject_default_instructions_when_missing(root: &mut Map<String, Value>) {
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
/// Normalize a native `/responses` request body in place for the upstream.
///
/// Always drops `max_output_tokens` (unsupported upstream). For the standard
/// `/v1/responses` path it also strips upstream-unsupported fields and coerces
/// a scalar/object `input` into the array-of-items shape the upstream expects.
/// For `/v1/responses/compact` it instead retains only the compact-safe field
/// set.
pub fn normalize_native_responses_request(path: &str, root: &mut Map<String, Value>) {
    root.remove("max_output_tokens");
    if path == "/v1/responses" {
        remove_native_responses_upstream_unsupported_fields(root);
        normalize_native_responses_input_for_upstream(root);
    }
    if path == "/v1/responses/compact" {
        retain_native_compact_fields(root);
    }
}
fn remove_native_responses_upstream_unsupported_fields(root: &mut Map<String, Value>) {
    for field in NATIVE_RESPONSES_UPSTREAM_UNSUPPORTED_FIELDS {
        root.remove(*field);
    }
}
fn normalize_native_responses_input_for_upstream(root: &mut Map<String, Value>) {
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
/// Repair Chat-Completions-style `tool` messages in a native `/responses` body.
///
/// No-op for non-`/v1/responses` paths. Upstream rejects items with a `tool`
/// role, so each such input item is rewritten in place: with a call id it
/// becomes a `function_call_output` item carrying the tool output; otherwise it
/// degrades to a plain `user` message (with an `(empty)` placeholder when there
/// is no usable content).
pub fn repair_native_responses_request(
    path: &str,
    root: &mut Map<String, Value>,
) -> CodexGatewayResult<()> {
    if path != "/v1/responses" {
        return Ok(());
    }
    repair_native_responses_tool_role_messages(root)
}
fn repair_native_responses_tool_role_messages(
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
/// Validate input-item roles in a native `/responses` request body.
///
/// No-op for non-`/v1/responses` paths. Returns a `400` if any input item
/// carries an unsupported role: a Chat-Completions `tool` role gets a targeted
/// message pointing at `function_call_output`, and any other unknown role is
/// rejected with the list of supported roles. Items without a role are left for
/// the upstream.
pub fn validate_native_responses_request(
    path: &str,
    root: &Map<String, Value>,
) -> CodexGatewayResult<()> {
    if path != "/v1/responses" {
        return Ok(());
    }
    validate_native_responses_input_roles(root.get("input"))
}
fn validate_native_responses_input_roles(input: Option<&Value>) -> CodexGatewayResult<()> {
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
/// Strip per-item `id` fields from a native `/responses` `input` array.
///
/// Returns `true` if any `id` was removed. Used when realigning the request
/// with upstream `store` semantics: item ids are only valid when the upstream
/// persists the response, so they must be dropped otherwise.
pub fn strip_input_item_ids(root: &mut Map<String, Value>) -> bool {
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
fn retain_native_compact_fields(root: &mut Map<String, Value>) {
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
