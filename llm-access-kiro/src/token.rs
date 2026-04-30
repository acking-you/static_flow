//! Token counting heuristics for Anthropic-compatible billing estimation.
//!
//! Provides a fast, approximate token count without loading a real tokenizer.
//! Non-western characters (CJK, etc.) are weighted at 4 char-units each to
//! reflect their higher byte cost in BPE tokenizers. Short texts receive a
//! scaling factor to compensate for per-message overhead.

use super::anthropic::types::{Message, SystemMessage, Tool};

/// Returns `true` for characters outside the Latin/extended-Latin Unicode
/// ranges, used to weight CJK and other multi-byte scripts more heavily.
fn is_non_western_char(ch: char) -> bool {
    !matches!(
        ch,
        '\u{0000}'..='\u{007F}'
            | '\u{0080}'..='\u{00FF}'
            | '\u{0100}'..='\u{024F}'
            | '\u{1E00}'..='\u{1EFF}'
            | '\u{2C60}'..='\u{2C7F}'
            | '\u{A720}'..='\u{A7FF}'
            | '\u{AB30}'..='\u{AB6F}'
    )
}

/// Estimate the token count of a single text string.
///
/// Uses a 4-char-unit-per-token heuristic with a scaling factor for short
/// texts to account for tokenizer overhead on small inputs.
pub fn count_tokens(text: &str) -> u64 {
    let char_units: f64 = text
        .chars()
        .map(|ch| if is_non_western_char(ch) { 4.0 } else { 1.0 })
        .sum();
    let tokens = char_units / 4.0;
    let adjusted = if tokens < 100.0 {
        tokens * 1.5
    } else if tokens < 200.0 {
        tokens * 1.3
    } else if tokens < 300.0 {
        tokens * 1.25
    } else if tokens < 800.0 {
        tokens * 1.2
    } else {
        tokens
    };
    adjusted.max(1.0) as u64
}

/// Estimate total input tokens across system messages, conversation messages,
/// and tool definitions. The `_model` parameter is reserved for future
/// model-specific adjustments.
pub fn count_all_tokens(
    _model: String,
    system: Option<Vec<SystemMessage>>,
    messages: Vec<Message>,
    tools: Option<Vec<Tool>>,
) -> u64 {
    let mut total = 0;
    if let Some(system) = system {
        for message in system {
            total += count_tokens(&message.text);
        }
    }
    for message in messages {
        match message.content {
            serde_json::Value::String(text) => total += count_tokens(&text),
            serde_json::Value::Array(items) => {
                for item in items {
                    if let Some(text) = item.get("text").and_then(|value| value.as_str()) {
                        total += count_tokens(text);
                    }
                }
            },
            _ => {},
        }
    }
    if let Some(tools) = tools {
        for tool in tools {
            total += count_tokens(&tool.name);
            total += count_tokens(&tool.description);
            total += count_tokens(&serde_json::to_string(&tool.input_schema).unwrap_or_default());
        }
    }
    total.max(1)
}

/// Estimate the output token count from an array of content blocks
/// (text blocks and tool_use blocks).
pub fn estimate_output_tokens(content: &[serde_json::Value]) -> i32 {
    let mut total = 0;
    for block in content {
        if let Some(text) = block.get("text").and_then(|value| value.as_str()) {
            total += count_tokens(text) as i32;
        }
        if block.get("type").and_then(|value| value.as_str()) == Some("tool_use") {
            total += count_tokens(&serde_json::to_string(block).unwrap_or_default()) as i32;
        }
    }
    total.max(1)
}
