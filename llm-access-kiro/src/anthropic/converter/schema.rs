//! JSON-schema normalization for tool parameters and multimodal
//! tool-schema compatibility checks (some schema keywords are rejected by the
//! upstream when images are present).

use std::collections::BTreeMap;

use super::MULTIMODAL_UNSUPPORTED_SCHEMA_KEYWORDS;
use crate::wire::{InputSchema, Tool};

pub fn permissive_object_schema() -> serde_json::Value {
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
pub fn normalize_json_schema(schema: serde_json::Value) -> serde_json::Value {
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

pub fn collect_schema_keywords(value: &serde_json::Value, counts: &mut BTreeMap<String, usize>) {
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

pub fn request_message_contains_image(message: &crate::anthropic::types::Message) -> bool {
    content_value_contains_image(&message.content)
}

pub fn apply_multimodal_tool_schema_compatibility(tools: &mut [Tool], has_images: bool) {
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


#[cfg(test)]
mod tests {
    use super::*;

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
}
