//! Inline `<thinking>` block extraction.
//!
//! Splits assistant content containing inline `<thinking>...</thinking>` spans
//! into thinking/text segments (quote-aware so escaped tags are ignored), and
//! builds Anthropic content blocks with synthetic signatures attached.

use serde_json::json;

use super::signature::synthetic_thinking_signature;

#[derive(Debug, Clone, PartialEq, Eq)]
enum InlineThinkingBlock {
    Thinking(String),
    Text(String),
}

pub fn build_inline_thinking_content_blocks(
    content: &str,
    model: &str,
    thinking_enabled: bool,
) -> Vec<serde_json::Value> {
    let mut blocks = Vec::new();
    for block in split_inline_thinking_content(content, thinking_enabled) {
        match block {
            InlineThinkingBlock::Thinking(thinking) => blocks.push(json!({
                "type": "thinking",
                "thinking": thinking,
                "signature": synthetic_thinking_signature(model, &thinking),
            })),
            InlineThinkingBlock::Text(text) => {
                if !text.is_empty() {
                    blocks.push(json!({"type": "text", "text": text}));
                }
            },
        }
    }
    blocks
}

// Characters that indicate a tag is inside a quoted/escaped context
// and should not be treated as a real thinking boundary.
const QUOTE_CHARS: &[u8] = b"`\"'\\";

// Checks whether the byte at `pos` is a quote/escape character.
fn is_quote_char(buffer: &str, pos: usize) -> bool {
    buffer
        .as_bytes()
        .get(pos)
        .map(|value| QUOTE_CHARS.contains(value))
        .unwrap_or(false)
}

/// Finds `<thinking>` that is not inside quotes. Skips false positives
/// where the tag is adjacent to quote characters.
pub fn find_real_thinking_start_tag(buffer: &str) -> Option<usize> {
    find_real_tag(buffer, "<thinking>", false)
}

/// Finds `</thinking>` followed by `\n\n` (mid-stream boundary).
/// Returns None if the double-newline hasn't arrived yet (partial buffer).
pub fn find_real_thinking_end_tag(buffer: &str) -> Option<usize> {
    const TAG: &str = "</thinking>";
    let mut search_start = 0usize;
    while let Some(pos) = buffer[search_start..].find(TAG) {
        let absolute_pos = search_start + pos;
        let after_pos = absolute_pos + TAG.len();
        if (absolute_pos > 0 && is_quote_char(buffer, absolute_pos - 1))
            || is_quote_char(buffer, after_pos)
        {
            search_start = absolute_pos + 1;
            continue;
        }
        let after_content = &buffer[after_pos..];
        if after_content.len() < 2 {
            return None;
        }
        if after_content.starts_with("\n\n") {
            return Some(absolute_pos);
        }
        search_start = absolute_pos + 1;
    }
    None
}

/// Finds `</thinking>` at the end of the buffer (for tool_use or final flush),
/// where the double-newline requirement is relaxed to trailing whitespace.
pub fn find_real_thinking_end_tag_at_buffer_end(buffer: &str) -> Option<usize> {
    const TAG: &str = "</thinking>";
    let mut search_start = 0usize;

    while let Some(pos) = buffer[search_start..].find(TAG) {
        let absolute_pos = search_start + pos;
        let after_pos = absolute_pos + TAG.len();
        if (absolute_pos > 0 && is_quote_char(buffer, absolute_pos - 1))
            || is_quote_char(buffer, after_pos)
        {
            search_start = absolute_pos + 1;
            continue;
        }
        if buffer[after_pos..].trim().is_empty() {
            return Some(absolute_pos);
        }
        search_start = absolute_pos + 1;
    }

    None
}

fn find_real_tag(buffer: &str, tag: &str, require_double_newline_after: bool) -> Option<usize> {
    let mut search_start = 0usize;
    while let Some(pos) = buffer[search_start..].find(tag) {
        let absolute_pos = search_start + pos;
        let after_pos = absolute_pos + tag.len();
        if (absolute_pos > 0 && is_quote_char(buffer, absolute_pos - 1))
            || is_quote_char(buffer, after_pos)
        {
            search_start = absolute_pos + 1;
            continue;
        }
        if require_double_newline_after {
            let after_content = &buffer[after_pos..];
            if after_content.len() < 2 {
                return None;
            }
            if !after_content.starts_with("\n\n") {
                search_start = absolute_pos + 1;
                continue;
            }
        }
        return Some(absolute_pos);
    }
    None
}

fn split_inline_thinking_content(
    content: &str,
    thinking_enabled: bool,
) -> Vec<InlineThinkingBlock> {
    if content.is_empty() {
        return Vec::new();
    }
    if !thinking_enabled {
        return vec![InlineThinkingBlock::Text(content.to_string())];
    }

    let Some(start_pos) = find_real_thinking_start_tag(content) else {
        return vec![InlineThinkingBlock::Text(content.to_string())];
    };

    let mut blocks = Vec::new();
    let before = &content[..start_pos];
    if !before.trim().is_empty() {
        blocks.push(InlineThinkingBlock::Text(before.to_string()));
    }

    let mut remaining = &content[start_pos + "<thinking>".len()..];
    if remaining.starts_with('\n') {
        remaining = &remaining[1..];
    }

    let end_pos = if let Some(end_pos) = find_real_thinking_end_tag(remaining) {
        end_pos
    } else if let Some(end_pos) = find_real_thinking_end_tag_at_buffer_end(remaining) {
        end_pos
    } else {
        return vec![InlineThinkingBlock::Text(content.to_string())];
    };

    blocks.push(InlineThinkingBlock::Thinking(remaining[..end_pos].to_string()));

    let after_tag = &remaining[end_pos + "</thinking>".len()..];
    let after_thinking = after_tag.strip_prefix("\n\n").unwrap_or(after_tag);
    if !after_thinking.is_empty() {
        blocks.push(InlineThinkingBlock::Text(after_thinking.to_string()));
    }

    blocks
}

/// Returns `content` with any inline `<thinking>` spans removed, keeping only
/// the surrounding text segments.
pub fn strip_inline_thinking_content(content: &str) -> String {
    split_inline_thinking_content(content, true)
        .into_iter()
        .filter_map(|block| match block {
            InlineThinkingBlock::Text(text) => Some(text),
            InlineThinkingBlock::Thinking(_) => None,
        })
        .collect::<String>()
}

#[cfg(test)]
mod tests {
    use super::{split_inline_thinking_content, InlineThinkingBlock};

    #[test]
    fn split_inline_thinking_content_extracts_non_stream_blocks() {
        let blocks = split_inline_thinking_content(
            "<thinking>\nCount carefully.\n</thinking>\n\nbeta",
            true,
        );

        assert_eq!(blocks, vec![
            InlineThinkingBlock::Thinking("Count carefully.\n".to_string()),
            InlineThinkingBlock::Text("beta".to_string()),
        ]);
    }
}
