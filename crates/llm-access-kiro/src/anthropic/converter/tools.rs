//! Tool-definition normalization/conversion and structured-output tool
//! injection (the synthetic `sf_emit_structured_output` tool).

use std::collections::{HashMap, HashSet};

use super::{
    invalid_request,
    schema::{collect_schema_keywords, normalize_json_schema},
    tool_name::map_tool_name,
    ConversionError, ToolNormalizationEvent, ToolNormalizationResult, ToolValidationSummary,
    EDIT_TOOL_DESCRIPTION_SUFFIX, STRUCTURED_OUTPUT_TOOL_DESCRIPTION,
    STRUCTURED_OUTPUT_TOOL_NAME_BASE, WRITE_TOOL_DESCRIPTION_SUFFIX,
};
use crate::{
    anthropic::types::MessagesRequest,
    wire::{InputSchema, Tool, ToolSpecification},
};

fn normalize_tool_description(name: &str, description: &str) -> Option<String> {
    if description.trim().is_empty() {
        Some(format!("Client-provided tool '{name}'"))
    } else {
        None
    }
}

pub fn normalize_tools(
    tools: &Option<Vec<crate::anthropic::types::Tool>>,
) -> Result<ToolNormalizationResult, ConversionError> {
    let Some(tools) = tools else {
        return Ok((None, Vec::new(), ToolValidationSummary::default()));
    };

    let mut normalized_tools = Vec::with_capacity(tools.len());
    let mut events = Vec::new();
    let mut summary = ToolValidationSummary::default();

    for (tool_index, tool) in tools.iter().enumerate() {
        let name = tool.name.trim();
        if name.is_empty() {
            summary.empty_tool_name_count += 1;
            return Err(invalid_request(format!("tool {tool_index} has empty name")));
        }

        let mut normalized_tool = tool.clone();
        normalized_tool.name = name.to_string();

        if let Some(description) = normalize_tool_description(name, &tool.description) {
            normalized_tool.description = description;
            summary.normalized_tool_description_count += 1;
            events.push(ToolNormalizationEvent {
                tool_index,
                tool_name: normalized_tool.name.clone(),
                action: "fill_tool_description",
                reason: "empty_tool_description",
            });
        }

        let schema =
            serde_json::Value::Object(normalized_tool.input_schema.clone().into_iter().collect());
        collect_schema_keywords(&schema, &mut summary.schema_keyword_counts);

        normalized_tools.push(normalized_tool);
    }

    Ok((Some(normalized_tools), events, summary))
}

// Converts Anthropic tool definitions to Kiro wire Tool specs.
// Appends chunked-write policy suffixes to Write/Edit tool descriptions
// and truncates descriptions to 10K chars.
pub fn convert_tools(
    tools: &Option<Vec<crate::anthropic::types::Tool>>,
    tool_name_map: &mut HashMap<String, String>,
) -> Vec<Tool> {
    let Some(tools) = tools else {
        return Vec::new();
    };
    tools
        .iter()
        .map(|tool| {
            let mut description = tool.description.clone();
            let suffix = match tool.name.as_str() {
                "Write" => WRITE_TOOL_DESCRIPTION_SUFFIX,
                "Edit" => EDIT_TOOL_DESCRIPTION_SUFFIX,
                _ => "",
            };
            if !suffix.is_empty() {
                description.push('\n');
                description.push_str(suffix);
            }
            let description = match description.char_indices().nth(10_000) {
                Some((idx, _)) => description[..idx].to_string(),
                None => description,
            };
            Tool {
                tool_specification: ToolSpecification {
                    name: map_tool_name(&tool.name, tool_name_map),
                    description,
                    input_schema: InputSchema::from_json(normalize_json_schema(serde_json::json!(
                        tool.input_schema
                    ))),
                },
            }
        })
        .collect()
}

fn extract_structured_output_schema(req: &MessagesRequest) -> Option<serde_json::Value> {
    req.output_config
        .as_ref()
        .and_then(|config| config.json_schema())
        .cloned()
        .map(normalize_json_schema)
}

fn make_structured_output_tool_name(existing_tools: &[Tool]) -> String {
    let existing = existing_tools
        .iter()
        .map(|tool| tool.tool_specification.name.to_lowercase())
        .collect::<HashSet<_>>();
    if !existing.contains(STRUCTURED_OUTPUT_TOOL_NAME_BASE) {
        return STRUCTURED_OUTPUT_TOOL_NAME_BASE.to_string();
    }
    for suffix in 1.. {
        let candidate = format!("{STRUCTURED_OUTPUT_TOOL_NAME_BASE}_{suffix}");
        if !existing.contains(&candidate) {
            return candidate;
        }
    }
    unreachable!("finite tool name search should always terminate")
}

pub fn structured_output_instruction(tool_name: &str) -> String {
    format!(
        "Return the final answer by calling the `{tool_name}` tool exactly once. Do not emit any \
         free-form text outside that tool call."
    )
}

pub fn append_structured_output_tool(
    req: &MessagesRequest,
    tools: &mut Vec<Tool>,
) -> Option<String> {
    let schema = extract_structured_output_schema(req)?;
    let tool_name = make_structured_output_tool_name(tools);
    tools.push(Tool {
        tool_specification: ToolSpecification {
            name: tool_name.clone(),
            description: STRUCTURED_OUTPUT_TOOL_DESCRIPTION.to_string(),
            input_schema: InputSchema::from_json(schema),
        },
    });
    Some(tool_name)
}
