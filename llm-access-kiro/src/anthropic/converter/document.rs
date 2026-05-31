//! Document attachment normalization: media-type canonicalization, name
//! sanitization/generation, and conversion into Kiro document blocks.

use base64::Engine as _;
use sha2::{Digest, Sha256};

use super::{
    invalid_request, normalize::push_normalization_event, ConversionError, NormalizationEvent,
};
use crate::wire::KiroDocument;

pub fn normalize_user_document_block(
    block: &serde_json::Map<String, serde_json::Value>,
    message_index: usize,
    block_index: usize,
    events: &mut Vec<NormalizationEvent>,
) -> Result<serde_json::Value, ConversionError> {
    let normalized = normalize_document_block_payload(block, message_index, block_index)?;
    if normalized != serde_json::Value::Object(block.clone()) {
        push_normalization_event(
            events,
            message_index,
            "user",
            Some(block_index),
            Some("document"),
            "rewrite_content_block",
            "document_block_normalized",
        );
    }
    Ok(normalized)
}

pub fn normalize_document_block_payload(
    block: &serde_json::Map<String, serde_json::Value>,
    message_index: usize,
    block_index: usize,
) -> Result<serde_json::Value, ConversionError> {
    let Some(source) = block.get("source").and_then(serde_json::Value::as_object) else {
        return Err(invalid_request(format!(
            "message {message_index} document block {block_index} is missing source"
        )));
    };
    let Some(source_type) = source
        .get("type")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Err(invalid_request(format!(
            "message {message_index} document block {block_index} is missing source.type"
        )));
    };
    let Some(media_type) = source
        .get("media_type")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Err(invalid_request(format!(
            "message {message_index} document block {block_index} is missing source.media_type"
        )));
    };
    let Some(source_data) = source
        .get("data")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
    else {
        return Err(invalid_request(format!(
            "message {message_index} document block {block_index} is missing source.data"
        )));
    };

    let normalized_media_type = canonical_document_media_type(media_type).ok_or_else(|| {
        invalid_request(format!(
            "message {message_index} document block {block_index} has unsupported \
             source.media_type `{media_type}`"
        ))
    })?;
    let normalized_data = match source_type {
        "base64" => source_data.trim().to_string(),
        "text" => {
            if !document_media_type_supports_text_source(normalized_media_type) {
                return Err(invalid_request(format!(
                    "message {message_index} document block {block_index} only supports \
                     source.type=`text` for plain text, markdown, html, or csv documents"
                )));
            }
            source_data.replace("\r\n", "\n").replace('\r', "\n")
        },
        _ => {
            return Err(invalid_request(format!(
                "message {message_index} document block {block_index} must use \
                 source.type=`base64` or source.type=`text`"
            )))
        },
    };
    let normalized_name = normalize_document_name(
        block.get("name").and_then(serde_json::Value::as_str),
        normalized_media_type,
        &normalized_data,
    );

    Ok(serde_json::json!({
        "type": "document",
        "name": normalized_name,
        "source": {
            "type": source_type,
            "media_type": normalized_media_type,
            "data": normalized_data,
        }
    }))
}

fn canonical_document_media_type(media_type: &str) -> Option<&'static str> {
    match media_type.trim().to_ascii_lowercase().as_str() {
        "application/pdf" => Some("application/pdf"),
        "text/csv" => Some("text/csv"),
        "application/msword" => Some("application/msword"),
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => {
            Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document")
        },
        "application/vnd.ms-excel" => Some("application/vnd.ms-excel"),
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => {
            Some("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet")
        },
        "text/html" => Some("text/html"),
        "text/plain" => Some("text/plain"),
        "text/markdown" | "text/md" | "text/x-markdown" => Some("text/markdown"),
        _ => None,
    }
}

fn document_media_type_supports_text_source(media_type: &str) -> bool {
    matches!(media_type, "text/plain" | "text/markdown" | "text/html" | "text/csv")
}

fn document_format_from_media_type(media_type: &str) -> Option<&'static str> {
    match media_type {
        "application/pdf" => Some("pdf"),
        "text/csv" => Some("csv"),
        "application/msword" => Some("doc"),
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => Some("docx"),
        "application/vnd.ms-excel" => Some("xls"),
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => Some("xlsx"),
        "text/html" => Some("html"),
        "text/plain" => Some("txt"),
        "text/markdown" => Some("md"),
        _ => None,
    }
}

fn normalize_document_name(raw_name: Option<&str>, media_type: &str, data: &str) -> String {
    match raw_name.map(str::trim).filter(|value| !value.is_empty()) {
        Some(raw_name) => sanitize_document_name(raw_name),
        None => generate_document_name(media_type, data),
    }
}

fn sanitize_document_name(name: &str) -> String {
    let without_extension = name.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(name);
    let mut sanitized = String::with_capacity(without_extension.len());
    let mut previous_dash = false;
    let mut previous_space = false;
    for ch in without_extension.chars() {
        let normalized = if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '(' | ')' | '[' | ']')
        {
            previous_dash = false;
            previous_space = false;
            Some(ch)
        } else if ch.is_ascii_whitespace() {
            if previous_space {
                None
            } else {
                previous_space = true;
                previous_dash = false;
                Some(' ')
            }
        } else if previous_dash {
            None
        } else {
            previous_dash = true;
            previous_space = false;
            Some('-')
        };
        if let Some(ch) = normalized {
            sanitized.push(ch);
        }
    }
    let trimmed = sanitized.trim();
    if trimmed.is_empty() {
        "document".to_string()
    } else {
        trimmed.chars().take(200).collect()
    }
}

pub fn generate_document_name(media_type: &str, data: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(media_type.as_bytes());
    hasher.update([0]);
    hasher.update(data.as_bytes());
    let hash_hex = format!("{:x}", hasher.finalize());
    format!("document-{}", &hash_hex[..12])
}

fn kiro_document_from_source(
    name: String,
    source: crate::anthropic::types::ImageSource,
) -> Option<KiroDocument> {
    let format = document_format_from_media_type(&source.media_type)?;
    let bytes = match source.source_type.as_str() {
        "base64" => source.data,
        "text" if document_media_type_supports_text_source(&source.media_type) => {
            base64::engine::general_purpose::STANDARD.encode(source.data.as_bytes())
        },
        _ => return None,
    };
    Some(KiroDocument::from_base64(name, format, bytes))
}

pub fn kiro_document_from_block(
    name: Option<String>,
    mut source: crate::anthropic::types::ImageSource,
) -> Result<Option<KiroDocument>, ConversionError> {
    let Some(normalized_media_type) = canonical_document_media_type(&source.media_type) else {
        return Ok(None);
    };
    let normalized_data = match source.source_type.as_str() {
        "base64" => {
            let normalized_data = source.data.trim().to_string();
            source.data = normalized_data.clone();
            normalized_data
        },
        "text" if document_media_type_supports_text_source(normalized_media_type) => {
            let normalized_data = source.data.replace("\r\n", "\n").replace('\r', "\n");
            source.data = normalized_data.clone();
            normalized_data
        },
        _ => source.data.clone(),
    };
    let normalized_name =
        normalize_document_name(name.as_deref(), normalized_media_type, &normalized_data);
    source.media_type = normalized_media_type.to_string();
    Ok(kiro_document_from_source(normalized_name, source))
}
