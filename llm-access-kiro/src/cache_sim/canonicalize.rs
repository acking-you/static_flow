//! Canonicalization of conversation history and the current turn into stable
//! `CanonicalInputUnit`s: user/assistant/tool message normalization plus JSON
//! and text canonicalization.

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

pub(crate) fn canonicalize_history(history: &[Message]) -> Vec<CanonicalInputUnit> {
    let mut units = Vec::new();
    for message in history {
        match message {
            Message::User(message) => {
                units
                    .extend(canonicalize_user_message("history_user", &message.user_input_message));
            },
            Message::Assistant(message) => units.extend(canonicalize_assistant_segments(
                "history_assistant",
                &message.assistant_response_message,
            )),
        }
    }
    units
}
pub(crate) fn build_runtime_prompt_projection(
    state: &ConversationState,
) -> RuntimePromptProjection {
    let mut builder = RuntimePromptProjectionBuilder::new();

    for message in &state.history {
        match message {
            Message::User(message) => {
                builder.add_history_units(canonicalize_user_message(
                    "history_user",
                    &message.user_input_message,
                ));
            },
            Message::Assistant(message) => {
                builder.add_history_units(canonicalize_assistant_segments(
                    "history_assistant",
                    &message.assistant_response_message,
                ));
            },
        }
    }

    builder.add_stable_units(canonicalize_tools(
        &state
            .current_message
            .user_input_message
            .user_input_message_context
            .tools,
    ));
    builder.add_current_input_units(canonicalize_current_turn_for_input(
        &state.current_message.user_input_message,
    ));
    builder.add_current_history_units(canonicalize_current_turn_as_history(
        &state.current_message.user_input_message,
    ));

    builder.finish()
}
pub(crate) fn canonicalize_current_turn_as_history(message: &UserInputMessage) -> Vec<String> {
    canonicalize_user_input_message("history_user", message)
        .into_iter()
        .map(|unit| unit.key)
        .collect()
}
pub(crate) fn canonicalize_current_turn_for_input(
    message: &UserInputMessage,
) -> Vec<CanonicalInputUnit> {
    canonicalize_user_input_message("current_user", message)
}
pub(crate) fn canonicalize_user_message(
    kind_prefix: &str,
    message: &UserMessage,
) -> Vec<CanonicalInputUnit> {
    canonicalize_user_message_parts(kind_prefix, UserMessageParts {
        content: &message.content,
        images: &message.images,
        documents: &message.documents,
        context: &message.user_input_message_context,
    })
}
pub(crate) fn canonicalize_user_input_message(
    kind_prefix: &str,
    message: &UserInputMessage,
) -> Vec<CanonicalInputUnit> {
    canonicalize_user_message_parts(kind_prefix, UserMessageParts {
        content: &message.content,
        images: &message.images,
        documents: &message.documents,
        context: &message.user_input_message_context,
    })
}
pub(crate) fn canonicalize_user_message_parts(
    kind_prefix: &str,
    message: UserMessageParts<'_>,
) -> Vec<CanonicalInputUnit> {
    let mut units = Vec::new();
    let normalized_content = normalize_text(message.content);
    if !normalized_content.is_empty() {
        let key = serialize_canonical_segment(&CanonicalTextSegment {
            kind: format!("{kind_prefix}_text"),
            text: normalized_content.clone(),
        });
        units.push(CanonicalInputUnit {
            key,
            token_atoms: tokenize_text_atoms(&normalized_content),
        });
    }

    for image in message.images {
        let key = serialize_canonical_segment(&CanonicalImageSegment {
            kind: format!("{kind_prefix}_image"),
            format: normalize_text(&image.format),
            digest: sha256_hex(image.source.bytes.as_bytes()),
        });
        units.push(CanonicalInputUnit {
            key,
            token_atoms: Vec::new(),
        });
    }

    for document in message.documents {
        let key = serialize_canonical_segment(&CanonicalDocumentSegment {
            kind: format!("{kind_prefix}_document"),
            name: normalize_text(&document.name),
            format: normalize_text(&document.format),
            digest: sha256_hex(document.source.bytes.as_bytes()),
        });
        units.push(CanonicalInputUnit {
            key,
            token_atoms: Vec::new(),
        });
    }

    for result in &message.context.tool_results {
        let canonical_content = canonical_tool_result_content(&result.content);
        let key = serialize_canonical_segment(&CanonicalToolResultSegment {
            kind: format!("{kind_prefix}_tool_result"),
            tool_use_id: normalize_text(&result.tool_use_id),
            status: result
                .status
                .as_deref()
                .map(normalize_text)
                .unwrap_or_default(),
            is_error: result.is_error,
            content: canonical_content.clone(),
        });
        let token_source = format!(
            "{}\n{}\n{}",
            result.tool_use_id,
            result.status.as_deref().unwrap_or_default(),
            serde_json::to_string(&canonical_content).unwrap_or_default()
        );
        units.push(CanonicalInputUnit {
            key,
            token_atoms: tokenize_text_atoms(&token_source),
        });
    }

    units
}
pub(crate) fn canonicalize_assistant_message(message: &AssistantMessage) -> Vec<String> {
    canonicalize_assistant_segments("history_assistant", message)
        .into_iter()
        .map(|unit| unit.key)
        .collect()
}
pub(crate) fn canonicalize_assistant_segments(
    kind_prefix: &str,
    message: &AssistantMessage,
) -> Vec<CanonicalInputUnit> {
    let mut units = Vec::new();
    let normalized_content = normalize_text(&message.content);
    if !normalized_content.is_empty() {
        let key = serialize_canonical_segment(&CanonicalTextSegment {
            kind: format!("{kind_prefix}_text"),
            text: normalized_content.clone(),
        });
        units.push(CanonicalInputUnit {
            key,
            token_atoms: tokenize_text_atoms(&normalized_content),
        });
    }

    for tool_use in message.tool_uses.as_deref().unwrap_or(&[]) {
        let canonical_input = canonicalize_json(&tool_use.input);
        let key = serialize_canonical_segment(&CanonicalToolUseSegment {
            kind: format!("{kind_prefix}_tool_use"),
            tool_use_id: normalize_text(&tool_use.tool_use_id),
            name: normalize_text(&tool_use.name),
            input: canonical_input.clone(),
        });
        let token_source = format!(
            "{}\n{}\n{}",
            tool_use.tool_use_id,
            tool_use.name,
            serde_json::to_string(&canonical_input).unwrap_or_default()
        );
        units.push(CanonicalInputUnit {
            key,
            token_atoms: tokenize_text_atoms(&token_source),
        });
    }

    units
}
pub(crate) fn canonicalize_tools(tools: &[Tool]) -> Vec<CanonicalInputUnit> {
    let mut units = Vec::with_capacity(tools.len());
    for tool in tools {
        let name = normalize_text(&tool.tool_specification.name);
        let description = normalize_text(&tool.tool_specification.description);
        let canonical_schema = canonicalize_json(&tool.tool_specification.input_schema.json);
        let key = serialize_canonical_segment(&CanonicalToolDefinitionSegment {
            kind: "stable_tool_definition".to_string(),
            name: name.clone(),
            description: description.clone(),
            input_schema: canonical_schema.clone(),
        });
        let token_source = format!(
            "{name}\n{description}\n{}",
            serde_json::to_string(&canonical_schema).unwrap_or_default()
        );
        units.push(CanonicalInputUnit {
            key,
            token_atoms: tokenize_text_atoms(&token_source),
        });
    }
    units
}
pub(crate) fn canonical_tool_result_content(content: &[Map<String, Value>]) -> Value {
    Value::Array(
        content
            .iter()
            .map(|item| canonicalize_json(&Value::Object(item.clone())))
            .collect(),
    )
}
pub(crate) fn normalize_text(raw: &str) -> String {
    raw.replace("\r\n", "\n").trim().to_string()
}
pub(crate) fn canonicalize_json(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(canonicalize_json).collect()),
        Value::Object(map) => {
            let sorted = map
                .iter()
                .map(|(key, value)| (key.clone(), canonicalize_json(value)))
                .collect::<BTreeMap<_, _>>();
            let mut normalized = Map::new();
            for (key, value) in sorted {
                normalized.insert(key, value);
            }
            Value::Object(normalized)
        },
        _ => value.clone(),
    }
}
