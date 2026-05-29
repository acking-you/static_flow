//! Small text helpers: rough token estimation, UTF-8 char-boundary search,
//! and structured-output JSON canonicalization.

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

pub(crate) fn canonicalize_structured_output_json(input: &str) -> String {
    let value = if input.is_empty() {
        json!({})
    } else {
        serde_json::from_str(input).unwrap_or_else(|_| json!({}))
    };
    serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string())
}
// Rough token estimate: CJK chars ~0.67 tokens each, others ~0.25 each.
pub(crate) fn estimate_tokens(text: &str) -> i32 {
    let mut chinese_count = 0;
    let mut other_count = 0;
    for ch in text.chars() {
        if ('\u{4E00}'..='\u{9FFF}').contains(&ch) {
            chinese_count += 1;
        } else {
            other_count += 1;
        }
    }
    (((chinese_count * 2 + 2) / 3) + ((other_count + 3) / 4)).max(1)
}
// Finds the nearest valid UTF-8 char boundary at or before `target`.
pub(crate) fn find_char_boundary(s: &str, target: usize) -> usize {
    if target >= s.len() {
        return s.len();
    }
    if target == 0 {
        return 0;
    }
    let mut pos = target;
    while pos > 0 && !s.is_char_boundary(pos) {
        pos -= 1;
    }
    pos
}
