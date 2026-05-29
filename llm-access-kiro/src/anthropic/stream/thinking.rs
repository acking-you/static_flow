//! `<thinking>` tag detection and stripping in raw upstream content,
//! including quote-escaping awareness and double-newline-terminated tags.

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

// Checks whether the byte at `pos` is a quote/escape character.
pub(crate) fn is_quote_char(buffer: &str, pos: usize) -> bool {
    buffer
        .as_bytes()
        .get(pos)
        .map(|value| QUOTE_CHARS.contains(value))
        .unwrap_or(false)
}
// Finds `<thinking>` that is not inside quotes. Skips false positives
// where the tag is adjacent to quote characters.
pub(crate) fn find_real_thinking_start_tag(buffer: &str) -> Option<usize> {
    find_real_tag(buffer, "<thinking>", false)
}
// Finds `</thinking>` followed by `\n\n` (mid-stream boundary).
// Returns None if the double-newline hasn't arrived yet (partial buffer).
pub(crate) fn find_real_thinking_end_tag(buffer: &str) -> Option<usize> {
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
// Finds `</thinking>` at the end of the buffer (for tool_use or final flush),
// where the double-newline requirement is relaxed to trailing whitespace.
pub(crate) fn find_real_thinking_end_tag_at_buffer_end(buffer: &str) -> Option<usize> {
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
pub(crate) fn find_real_tag(
    buffer: &str,
    tag: &str,
    require_double_newline_after: bool,
) -> Option<usize> {
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
pub(crate) fn strip_inline_thinking_content(content: &str) -> String {
    split_inline_thinking_content(content, true)
        .into_iter()
        .filter_map(|block| match block {
            InlineThinkingBlock::Text(text) => Some(text),
            InlineThinkingBlock::Thinking(_) => None,
        })
        .collect::<Vec<_>>()
        .join("")
}
