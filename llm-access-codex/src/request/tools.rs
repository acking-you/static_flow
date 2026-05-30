//! Tool-definition handling: JSON-schema normalization + OpenAI tool-name
//! mangle/restore maps.


use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Map, Value};

use super::coerce_non_empty_scalar_to_string;
use crate::MAX_OPENAI_TOOL_NAME_LEN;
/// Recursively normalize a tool's JSON-Schema into the upstream-accepted shape.
///
/// Walks the schema tree and applies the fixups the Codex `/responses` tool API
/// requires: rewrites a `"$ref": "X[]"` shorthand into a proper `array`/`items`
/// schema; guarantees every `"type": "object"` node carries a `properties` map
/// (the upstream rejects objects without one); and recurses through `items`,
/// `additionalProperties`, `not`, the `allOf`/`anyOf`/`oneOf`/`prefixItems`
/// arrays, and the `$defs`/`definitions` maps. Non-object/array values pass
/// through unchanged.
pub fn normalize_tool_parameters_schema(value: Value) -> Value {
    match value {
        Value::Object(mut obj) => {
            if let Some(array_ref) = obj
                .get("$ref")
                .and_then(Value::as_str)
                .map(str::trim)
                .and_then(|reference| reference.strip_suffix("[]"))
                .filter(|reference| !reference.is_empty())
            {
                let mut rewritten = Map::new();
                rewritten.insert("type".to_string(), Value::String("array".to_string()));
                rewritten.insert(
                    "items".to_string(),
                    normalize_tool_parameters_schema(json!({ "$ref": array_ref })),
                );
                for (key, child) in obj {
                    if key == "$ref" {
                        continue;
                    }
                    rewritten.insert(key, normalize_tool_parameters_schema(child));
                }
                return Value::Object(rewritten);
            }

            if obj
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|schema_type| schema_type == "object")
            {
                match obj.get_mut("properties") {
                    Some(Value::Object(properties)) => {
                        for child in properties.values_mut() {
                            *child = normalize_tool_parameters_schema(child.take());
                        }
                    },
                    Some(other) => {
                        *other = Value::Object(Map::new());
                    },
                    None => {
                        obj.insert("properties".to_string(), Value::Object(Map::new()));
                    },
                }
            }

            for key in ["items", "additionalProperties", "not"] {
                if let Some(child) = obj.get_mut(key) {
                    *child = normalize_tool_parameters_schema(child.take());
                }
            }
            for key in ["allOf", "anyOf", "oneOf", "prefixItems"] {
                if let Some(Value::Array(items)) = obj.get_mut(key) {
                    for item in items {
                        *item = normalize_tool_parameters_schema(item.take());
                    }
                }
            }
            for key in ["$defs", "definitions"] {
                if let Some(Value::Object(defs)) = obj.get_mut(key) {
                    for schema in defs.values_mut() {
                        *schema = normalize_tool_parameters_schema(schema.take());
                    }
                }
            }
            Value::Object(obj)
        },
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(normalize_tool_parameters_schema)
                .collect(),
        ),
        other => other,
    }
}
/// Shorten tool names so they fit the upstream name length budget.
fn shorten_openai_tool_name_candidate(name: &str) -> String {
    if name.len() <= MAX_OPENAI_TOOL_NAME_LEN {
        return name.to_string();
    }
    if name.starts_with("mcp__") {
        if let Some(idx) = name.rfind("__") {
            if idx > 0 {
                let mut candidate = format!("mcp__{}", &name[idx + 2..]);
                if candidate.len() > MAX_OPENAI_TOOL_NAME_LEN {
                    // Truncate on a char boundary; `String::truncate` panics if
                    // the byte index splits a multi-byte UTF-8 char (a tool name
                    // with Unicode after `mcp__` could otherwise crash the
                    // handler). Matches the char-based fallback below.
                    candidate = candidate.chars().take(MAX_OPENAI_TOOL_NAME_LEN).collect();
                }
                return candidate;
            }
        }
    }
    name.chars().take(MAX_OPENAI_TOOL_NAME_LEN).collect()
}
/// Apply the stable shortening map for one OpenAI tool/function name.
pub fn shorten_openai_tool_name_with_map(
    name: &str,
    tool_name_map: &BTreeMap<String, String>,
) -> String {
    tool_name_map
        .get(name)
        .cloned()
        .unwrap_or_else(|| shorten_openai_tool_name_candidate(name))
}
/// Restore shortened tool names when adapting responses back to chat format.
pub fn restore_openai_tool_name(
    name: &str,
    tool_name_restore_map: Option<&BTreeMap<String, String>>,
) -> String {
    tool_name_restore_map
        .and_then(|map| map.get(name))
        .cloned()
        .unwrap_or_else(|| name.to_string())
}
/// Return the dynamic tools array regardless of the chosen field casing.
pub fn get_dynamic_tools_array(obj: &Map<String, Value>) -> Option<&Vec<Value>> {
    obj.get("dynamic_tools")
        .or_else(|| obj.get("dynamicTools"))
        .and_then(Value::as_array)
}
/// Detect whether a tool object is an OpenAI Chat Completions function tool.
///
/// Matches an explicit `"type": "function"`, and also treats a tool with no
/// `type` as a function when it carries a `function` object or a bare `name`
/// (the legacy Chat Completions shapes). Other tool types return `false`.
pub fn is_openai_chat_function_tool(tool_obj: &Map<String, Value>) -> bool {
    let tool_type = tool_obj
        .get("type")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    tool_type == "function"
        || (tool_type.is_empty()
            && (tool_obj.get("function").is_some() || tool_obj.get("name").is_some()))
}
fn openai_chat_tool_field<'a>(
    tool_obj: &'a Map<String, Value>,
    function: Option<&'a Map<String, Value>>,
    key: &str,
) -> Option<&'a Value> {
    tool_obj
        .get(key)
        .or_else(|| function.and_then(|function| function.get(key)))
}
/// Resolve a tool's `name` from the tool object or its nested `function` body.
///
/// OpenAI Chat Completions tools may carry `name` at the top level or inside a
/// `function` object; this returns the first one present so callers see a
/// single name regardless of which shape the client sent.
pub fn openai_chat_tool_name_value<'a>(
    tool_obj: &'a Map<String, Value>,
    function: Option<&'a Map<String, Value>>,
) -> Option<&'a Value> {
    openai_chat_tool_field(tool_obj, function, "name")
}
/// Return the `name` field of a legacy `{ "function": { ... } }` tool body.
///
/// Used only for the legacy Chat Completions function shape, where the name
/// lives directly on the inner `function` object.
pub fn legacy_openai_function_name_value(function_obj: &Map<String, Value>) -> Option<&Value> {
    function_obj.get("name")
}
/// Convert one OpenAI Chat Completions function tool into a responses tool.
///
/// Resolves the tool name (top-level or nested `function`), shortens it through
/// `tool_name_map` to fit the upstream length budget, and copies across
/// `description`, schema-normalized `parameters`, and `strict` when present.
/// Returns `None` when the tool has no usable, non-empty name.
pub fn map_openai_chat_function_tool(
    tool_obj: &Map<String, Value>,
    tool_name_map: &BTreeMap<String, String>,
) -> Option<Value> {
    let function = tool_obj.get("function").and_then(Value::as_object);
    let name = coerce_non_empty_scalar_to_string(openai_chat_tool_name_value(tool_obj, function))
        .map(|name| shorten_openai_tool_name_with_map(&name, tool_name_map))?;
    let mut mapped = Map::new();
    mapped.insert("type".to_string(), Value::String("function".to_string()));
    mapped.insert("name".to_string(), Value::String(name));
    if let Some(description) = openai_chat_tool_field(tool_obj, function, "description") {
        mapped.insert("description".to_string(), description.clone());
    }
    if let Some(parameters) = openai_chat_tool_field(tool_obj, function, "parameters") {
        mapped
            .insert("parameters".to_string(), normalize_tool_parameters_schema(parameters.clone()));
    }
    if let Some(strict) = openai_chat_tool_field(tool_obj, function, "strict") {
        mapped.insert("strict".to_string(), strict.clone());
    }
    Some(Value::Object(mapped))
}
/// Collect every function/tool name referenced anywhere in the request.
fn collect_openai_tool_names(obj: &Map<String, Value>) -> Vec<String> {
    let mut names = Vec::new();

    if let Some(tools) = obj.get("tools").and_then(Value::as_array) {
        for tool in tools {
            let Some(tool_obj) = tool.as_object() else {
                continue;
            };
            if !is_openai_chat_function_tool(tool_obj) {
                continue;
            }
            let function = tool_obj.get("function").and_then(Value::as_object);
            let name = openai_chat_tool_name_value(tool_obj, function)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            if let Some(name) = name {
                names.push(name.to_string());
            }
        }
    }

    if let Some(functions) = obj.get("functions").and_then(Value::as_array) {
        for function in functions {
            let Some(function_obj) = function.as_object() else {
                continue;
            };
            let name =
                coerce_non_empty_scalar_to_string(legacy_openai_function_name_value(function_obj));
            if let Some(name) = name {
                names.push(name);
            }
        }
    }

    if let Some(dynamic_tools) = get_dynamic_tools_array(obj) {
        for dynamic_tool in dynamic_tools {
            let Some(name) = dynamic_tool
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                continue;
            };
            names.push(name.to_string());
        }
    }

    if let Some(name) = obj
        .get("tool_choice")
        .and_then(Value::as_object)
        .and_then(|tool_choice| {
            let tool_type = tool_choice
                .get("type")
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or_default();
            if tool_type != "function" {
                return None;
            }
            let function = tool_choice.get("function").and_then(Value::as_object);
            openai_chat_tool_name_value(tool_choice, function)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
    {
        names.push(name.to_string());
    }

    if let Some(name) = obj
        .get("function_call")
        .and_then(Value::as_object)
        .and_then(legacy_openai_function_name_value)
        .and_then(|value| coerce_non_empty_scalar_to_string(Some(value)))
    {
        names.push(name);
    }

    if let Some(messages) = obj.get("messages").and_then(Value::as_array) {
        for message in messages {
            let Some(message_obj) = message.as_object() else {
                continue;
            };
            if message_obj.get("role").and_then(Value::as_str) != Some("assistant") {
                continue;
            }
            let Some(tool_calls) = message_obj.get("tool_calls").and_then(Value::as_array) else {
                continue;
            };
            for tool_call in tool_calls {
                let Some(name) = tool_call
                    .get("function")
                    .and_then(|function| function.get("name"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                else {
                    continue;
                };
                names.push(name.to_string());
            }
        }
    }

    names
}
/// Build a stable deduplicated shortening map for all tool names in a request.
pub fn build_openai_tool_name_map(obj: &Map<String, Value>) -> BTreeMap<String, String> {
    let mut unique_names = BTreeSet::new();
    for name in collect_openai_tool_names(obj) {
        unique_names.insert(name);
    }

    let mut used = BTreeSet::new();
    let mut out = BTreeMap::new();
    for name in unique_names {
        let base = shorten_openai_tool_name_candidate(name.as_str());
        let mut candidate = base.clone();
        let mut suffix = 1usize;
        while used.contains(&candidate) {
            let suffix_text = format!("_{suffix}");
            let mut truncated = base.clone();
            let limit = MAX_OPENAI_TOOL_NAME_LEN.saturating_sub(suffix_text.len());
            if truncated.len() > limit {
                truncated = truncated.chars().take(limit).collect();
            }
            candidate = format!("{truncated}{suffix_text}");
            suffix += 1;
        }
        used.insert(candidate.clone());
        out.insert(name, candidate);
    }
    out
}
/// Build the reverse map used when adapting responses back to chat format.
pub fn build_openai_tool_name_restore_map(
    tool_name_map: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut restore_map = BTreeMap::new();
    for (original, shortened) in tool_name_map {
        if original != shortened {
            restore_map.insert(shortened.clone(), original.clone());
        }
    }
    restore_map
}
