//! Local continuation helpers for Codex responses compatibility.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Canonical local anchor stored for one completed Codex responses turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredResponseAnchor {
    /// Client-visible local response id accepted on future
    /// `previous_response_id` requests.
    pub client_response_id: String,
    /// Canonical explicit history items that reproduce the completed turn.
    pub history_items: Vec<Value>,
}

/// Stream-scoped ID rewrite state for direct `/v1/responses` passthrough.
#[derive(Debug, Clone, Default)]
pub struct ResponsesContinuationMetadata {
    client_response_id: Option<String>,
    message_id_by_upstream: BTreeMap<String, String>,
}

impl ResponsesContinuationMetadata {
    fn ensure_client_response_id(&mut self, current: Option<&str>) -> String {
        if let Some(existing) = self.client_response_id.as_deref() {
            return existing.to_string();
        }
        let allocated = current
            .filter(|value| value.starts_with("resp_"))
            .map(ToString::to_string)
            .unwrap_or_else(|| format!("resp_{}", uuid::Uuid::new_v4().simple()));
        self.client_response_id = Some(allocated.clone());
        allocated
    }

    fn ensure_client_message_id(&mut self, current: Option<&str>) -> String {
        if let Some(current) = current {
            if current.starts_with("msg_") {
                return current.to_string();
            }
            if let Some(existing) = self.message_id_by_upstream.get(current) {
                return existing.clone();
            }
        }
        let allocated = format!("msg_{}", uuid::Uuid::new_v4().simple());
        if let Some(current) = current {
            self.message_id_by_upstream
                .insert(current.to_string(), allocated.clone());
        }
        allocated
    }
}

/// Expand a locally stored `previous_response_id` anchor into explicit input
/// history. When no local anchor exists but the request already carries full
/// input history, the stale upstream `previous_response_id` is stripped so the
/// request can still proceed with `store=false`.
pub fn expand_local_previous_response_id(
    root: &mut Map<String, Value>,
    anchor_items: Option<&[Value]>,
) {
    let has_previous = root
        .get("previous_response_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .is_some_and(|value| !value.is_empty());
    if !has_previous {
        return;
    }

    let current_input = root
        .get("input")
        .map(coerce_current_input_items)
        .unwrap_or_default();

    if let Some(anchor_items) = anchor_items {
        let mut expanded = anchor_items.to_vec();
        expanded.extend(current_input);
        root.insert("input".to_string(), Value::Array(expanded));
        root.remove("previous_response_id");
        return;
    }

    if input_already_contains_replayable_history(&current_input) {
        root.remove("previous_response_id");
    }
}

fn coerce_current_input_items(input: &Value) -> Vec<Value> {
    match input {
        Value::Array(items) => items.clone(),
        Value::Object(_) => vec![input.clone()],
        Value::String(text) => {
            let mut content = serde_json::Map::new();
            content.insert("type".to_string(), Value::String("input_text".to_string()));
            content.insert("text".to_string(), Value::String(text.clone()));

            let mut message = serde_json::Map::new();
            message.insert("type".to_string(), Value::String("message".to_string()));
            message.insert("role".to_string(), Value::String("user".to_string()));
            message.insert("content".to_string(), Value::Array(vec![Value::Object(content)]));
            vec![Value::Object(message)]
        },
        _ => Vec::new(),
    }
}

fn input_already_contains_replayable_history(items: &[Value]) -> bool {
    items.iter().any(|item| {
        item.as_object()
            .and_then(|obj| obj.get("type"))
            .and_then(Value::as_str)
            .is_some_and(|item_type| {
                matches!(item_type, "message" | "function_call" | "custom_tool_call")
            })
    })
}

/// Rewrite one completed responses JSON payload so downstream clients only see
/// local, replay-safe response/message IDs. Returns the anchor snapshot that
/// should be retained for future `previous_response_id` lookups.
pub fn rewrite_completed_response_for_local_continuation(
    request_body: &[u8],
    response: &mut Value,
) -> Result<StoredResponseAnchor, String> {
    let mut metadata = ResponsesContinuationMetadata::default();
    rewrite_response_value_ids(response, &mut metadata);
    let client_response_id = response
        .get("id")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| "rewritten responses payload is missing id".to_string())?;
    build_stored_response_anchor(request_body, response, client_response_id)
}

/// Build a local anchor from an already rewritten responses payload.
pub fn stored_response_anchor_from_rewritten_response(
    request_body: &[u8],
    response: &Value,
) -> Result<StoredResponseAnchor, String> {
    let client_response_id = response
        .get("id")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| "rewritten responses payload is missing id".to_string())?;
    build_stored_response_anchor(request_body, response, client_response_id)
}

fn build_stored_response_anchor(
    request_body: &[u8],
    response: &Value,
    client_response_id: String,
) -> Result<StoredResponseAnchor, String> {
    let history_items = extract_request_history_items(request_body)?
        .into_iter()
        .chain(extract_response_output_items(response))
        .collect();
    Ok(StoredResponseAnchor {
        client_response_id,
        history_items,
    })
}

/// Rewrite one parsed responses JSON value or SSE event payload in place.
pub fn rewrite_response_value_ids(value: &mut Value, metadata: &mut ResponsesContinuationMetadata) {
    let Some(obj) = value.as_object_mut() else {
        return;
    };

    if let Some(response) = obj.get_mut("response") {
        rewrite_response_value_ids(response, metadata);
    }

    if obj.contains_key("output") {
        let current_id = obj.get("id").and_then(Value::as_str);
        let client_response_id = metadata.ensure_client_response_id(current_id);
        obj.insert("id".to_string(), Value::String(client_response_id));
        if let Some(output) = obj.get_mut("output").and_then(Value::as_array_mut) {
            rewrite_output_items(output, metadata);
        }
    }

    if let Some(response_id) = obj.get("response_id").and_then(Value::as_str) {
        let client_response_id = metadata.ensure_client_response_id(Some(response_id));
        obj.insert("response_id".to_string(), Value::String(client_response_id));
    }

    if let Some(item) = obj.get_mut("item").and_then(Value::as_object_mut) {
        if item.get("type").and_then(Value::as_str) == Some("message") {
            item.entry("role".to_string())
                .or_insert_with(|| Value::String("assistant".to_string()));
            let current = item.get("id").and_then(Value::as_str);
            let client_id = metadata.ensure_client_message_id(current);
            item.insert("id".to_string(), Value::String(client_id));
        }
    }

    if let Some(item_id) = obj.get("item_id").and_then(Value::as_str) {
        let client_id = metadata.ensure_client_message_id(Some(item_id));
        obj.insert("item_id".to_string(), Value::String(client_id));
    }
}

fn rewrite_output_items(items: &mut [Value], metadata: &mut ResponsesContinuationMetadata) {
    for item in items {
        let Some(obj) = item.as_object_mut() else {
            continue;
        };
        if obj.get("type").and_then(Value::as_str) != Some("message") {
            continue;
        }
        obj.entry("role".to_string())
            .or_insert_with(|| Value::String("assistant".to_string()));
        let current = obj.get("id").and_then(Value::as_str);
        let client_id = metadata.ensure_client_message_id(current);
        obj.insert("id".to_string(), Value::String(client_id));
    }
}

fn extract_request_history_items(request_body: &[u8]) -> Result<Vec<Value>, String> {
    let value = serde_json::from_slice::<Value>(request_body)
        .map_err(|_| "invalid prepared request body json".to_string())?;
    Ok(value
        .get("input")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

fn extract_response_output_items(response: &Value) -> Vec<Value> {
    response
        .get("output")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::{
        expand_local_previous_response_id, rewrite_completed_response_for_local_continuation,
        rewrite_response_value_ids, stored_response_anchor_from_rewritten_response,
        ResponsesContinuationMetadata,
    };

    #[test]
    fn completed_response_rewrites_response_and_message_ids() {
        let mut response = json!({
            "id": "rs_old",
            "output": [{
                "id": "item_old",
                "type": "message",
                "role": "assistant",
                "content": [{"type":"output_text","text":"pong"}]
            }]
        });
        let request_body = br#"{"input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"ping"}]}]}"#;

        let anchor = rewrite_completed_response_for_local_continuation(request_body, &mut response)
            .expect("rewrite succeeds");

        assert!(response["id"]
            .as_str()
            .unwrap_or_default()
            .starts_with("resp_"));
        assert!(response["output"][0]["id"]
            .as_str()
            .unwrap_or_default()
            .starts_with("msg_"));
        assert_eq!(anchor.history_items.len(), 2);
        let rebuilt = stored_response_anchor_from_rewritten_response(request_body, &response)
            .expect("anchor rebuild succeeds");
        assert_eq!(rebuilt.client_response_id, anchor.client_response_id);
    }

    #[test]
    fn stale_previous_response_id_is_removed_when_history_is_present() {
        let mut root = serde_json::Map::new();
        root.insert("previous_response_id".to_string(), Value::String("rs_stale".to_string()));
        root.insert(
            "input".to_string(),
            json!([
                {"type":"message","role":"user","content":[{"type":"input_text","text":"hello"}]}
            ]),
        );

        expand_local_previous_response_id(&mut root, None);

        assert!(!root.contains_key("previous_response_id"));
    }

    #[test]
    fn expanding_previous_response_id_preserves_string_input_as_user_message() {
        let mut root = serde_json::Map::new();
        root.insert("previous_response_id".to_string(), Value::String("resp_local".to_string()));
        root.insert("input".to_string(), Value::String("next compact".to_string()));

        expand_local_previous_response_id(
            &mut root,
            Some(&[json!({
                "type":"message",
                "role":"assistant",
                "content":[{"type":"output_text","text":"hello"}]
            })]),
        );

        let input = root["input"].as_array().expect("expanded input array");
        assert_eq!(input.len(), 2);
        assert_eq!(input[0]["role"], json!("assistant"));
        assert_eq!(input[1]["role"], json!("user"));
        assert_eq!(input[1]["content"][0]["text"], json!("next compact"));
    }

    #[test]
    fn stream_payload_rewrite_keeps_stable_ids() {
        let mut metadata = ResponsesContinuationMetadata::default();
        let mut first = json!({
            "type":"response.output_text.delta",
            "response_id":"rs_old",
            "item_id":"item_old",
            "delta":"he"
        });
        let mut second = json!({
            "type":"response.output_item.done",
            "response_id":"rs_old",
            "item":{"id":"item_old","type":"message","role":"assistant"}
        });

        rewrite_response_value_ids(&mut first, &mut metadata);
        rewrite_response_value_ids(&mut second, &mut metadata);

        assert_eq!(first["response_id"], second["response_id"]);
        assert_eq!(first["item_id"], second["item"]["id"]);
    }
}
