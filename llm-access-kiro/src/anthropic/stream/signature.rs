//! Deterministic thinking-signature generation: minimal protobuf varint/
//! bytes-field encoding and the signature body/length derivation used to
//! reconstruct inline thinking content blocks.

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

pub(crate) fn encode_proto_varint(mut value: u64, out: &mut Vec<u8>) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}
pub(crate) fn encode_proto_key(field_number: u32, wire_type: u8, out: &mut Vec<u8>) {
    encode_proto_varint(((field_number as u64) << 3) | u64::from(wire_type), out);
}
pub(crate) fn proto_varint_len(mut value: usize) -> usize {
    let mut len = 1usize;
    while value >= 0x80 {
        value >>= 7;
        len += 1;
    }
    len
}
pub(crate) fn proto_bytes_field_encoded_len(field_number: u32, content_len: usize) -> usize {
    proto_varint_len(((field_number as usize) << 3) | 2)
        + proto_varint_len(content_len)
        + content_len
}
pub(crate) fn encode_proto_varint_field(field_number: u32, value: u64, out: &mut Vec<u8>) {
    encode_proto_key(field_number, 0, out);
    encode_proto_varint(value, out);
}
pub(crate) fn encode_proto_bytes_field(field_number: u32, value: &[u8], out: &mut Vec<u8>) {
    encode_proto_key(field_number, 2, out);
    encode_proto_varint(value.len() as u64, out);
    out.extend_from_slice(value);
}
pub(crate) fn derive_deterministic_signature_bytes(
    model: &str,
    thinking: &str,
    label: &[u8],
    len: usize,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    let mut counter = 0u32;
    while out.len() < len {
        let mut hasher = Sha512::new();
        hasher.update(THINKING_SIGNATURE_DOMAIN);
        hasher.update(label);
        hasher.update([0]);
        hasher.update(model.as_bytes());
        hasher.update([0]);
        hasher.update(thinking.as_bytes());
        hasher.update(counter.to_le_bytes());
        out.extend_from_slice(&hasher.finalize());
        counter = counter.wrapping_add(1);
    }
    out.truncate(len);
    out
}
pub(crate) fn signature_body_target_len(thinking: &str) -> usize {
    let thinking_len = thinking.len();
    thinking_len.clamp(THINKING_SIGNATURE_BODY_MIN_LEN, THINKING_SIGNATURE_BODY_MAX_LEN)
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
