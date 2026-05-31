//! Extraction of tool-result text and attachments from Anthropic
//! tool_result blocks (including stringified JSON content).

use super::{
    document::kiro_document_from_block, image::get_image_format_from_source, ConversionError,
};
use crate::{
    anthropic::types::ContentBlock,
    wire::{KiroDocument, KiroImage},
};

pub fn extract_tool_result_attachments(
    content: &Option<serde_json::Value>,
) -> Result<(Vec<KiroImage>, Vec<KiroDocument>), ConversionError> {
    if let Some(parsed_content) = parse_stringified_tool_result_content(content) {
        return extract_tool_result_attachments_from_value(&parsed_content);
    }
    match content {
        Some(value) => extract_tool_result_attachments_from_value(value),
        None => Ok((Vec::new(), Vec::new())),
    }
}

fn extract_tool_result_attachments_from_value(
    value: &serde_json::Value,
) -> Result<(Vec<KiroImage>, Vec<KiroDocument>), ConversionError> {
    let mut images = Vec::new();
    let mut documents = Vec::new();

    match value {
        serde_json::Value::Array(items) => {
            for item in items {
                let Ok(block) = serde_json::from_value::<ContentBlock>(item.clone()) else {
                    continue;
                };
                match block.block_type.as_str() {
                    "image" => {
                        if let Some(source) = block.source {
                            if let Some(format) = get_image_format_from_source(&source) {
                                images.push(KiroImage::from_base64(format, source.data));
                            }
                        }
                    },
                    "document" => {
                        if let Some(source) = block.source {
                            if let Some(document) = kiro_document_from_block(block.name, source)? {
                                documents.push(document);
                            }
                        }
                    },
                    _ => {},
                }
            }
        },
        serde_json::Value::Object(_) if looks_like_tool_result_content_block(value) => {
            let Ok(block) = serde_json::from_value::<ContentBlock>(value.clone()) else {
                return Ok((images, documents));
            };
            match block.block_type.as_str() {
                "image" => {
                    if let Some(source) = block.source {
                        if let Some(format) = get_image_format_from_source(&source) {
                            images.push(KiroImage::from_base64(format, source.data));
                        }
                    }
                },
                "document" => {
                    if let Some(source) = block.source {
                        if let Some(document) = kiro_document_from_block(block.name, source)? {
                            documents.push(document);
                        }
                    }
                },
                _ => {},
            }
        },
        _ => {},
    }

    Ok((images, documents))
}

pub fn extract_tool_result_content(content: &Option<serde_json::Value>) -> String {
    if let Some(parsed_content) = parse_stringified_tool_result_content(content) {
        return extract_tool_result_content_from_value(&parsed_content);
    }
    match content {
        Some(value) => extract_tool_result_content_from_value(value),
        None => String::new(),
    }
}

fn extract_tool_result_content_from_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(|item| item.get("text").and_then(|value| value.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        serde_json::Value::Object(object) if looks_like_tool_result_content_block(value) => object
            .get("text")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        other => other.to_string(),
    }
}

fn parse_stringified_tool_result_content(
    content: &Option<serde_json::Value>,
) -> Option<serde_json::Value> {
    let serde_json::Value::String(text) = content.as_ref()? else {
        return None;
    };
    let trimmed = text.trim();
    if trimmed.is_empty() || !(trimmed.starts_with('[') || trimmed.starts_with('{')) {
        return None;
    }
    let parsed = serde_json::from_str::<serde_json::Value>(trimmed).ok()?;
    match &parsed {
        serde_json::Value::Array(items)
            if !items.is_empty() && items.iter().all(looks_like_tool_result_content_block) =>
        {
            Some(parsed)
        },
        serde_json::Value::Object(_) if looks_like_tool_result_content_block(&parsed) => {
            Some(parsed)
        },
        _ => None,
    }
}

fn looks_like_tool_result_content_block(value: &serde_json::Value) -> bool {
    matches!(
        value.get("type").and_then(serde_json::Value::as_str),
        Some("text" | "image" | "document")
    )
}
