//! Synthetic Anthropic thinking-signature synthesis.
//!
//! Kiro exposes summarized thinking text but not Anthropic's encrypted
//! signature. This module emits a deterministic protobuf envelope matching the
//! observed Claude Code field layout. It is synthetic, not cryptographically
//! valid.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use sha2::{Digest, Sha512};

const THINKING_SIGNATURE_DOMAIN: &[u8] =
    b"staticflow-kiro-anthropic-thinking-signature-anthropic-shape-v6\0";
/// Protobuf header field-1 value identifying the signature kind.
pub const THINKING_SIGNATURE_HEADER_KIND: u64 = 12;
/// Protobuf header field-3 value identifying the signature mode.
pub const THINKING_SIGNATURE_HEADER_MODE: u64 = 2;
/// Byte length of the header field-5 body block.
pub const THINKING_SIGNATURE_HEADER_BODY_LEN: usize = 64;
/// Byte length of the inner nonce fields (2 and 3).
pub const THINKING_SIGNATURE_HEADER_NONCE_LEN: usize = 12;
/// Byte length of the inner proof field (4).
pub const THINKING_SIGNATURE_HEADER_PROOF_LEN: usize = 48;
/// Minimum byte length of the inner signature body field (5).
pub const THINKING_SIGNATURE_BODY_MIN_LEN: usize = 619;
const THINKING_SIGNATURE_BODY_MAX_LEN: usize = 8_192;

fn encode_proto_varint(mut value: u64, out: &mut Vec<u8>) {
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

fn encode_proto_key(field_number: u32, wire_type: u8, out: &mut Vec<u8>) {
    encode_proto_varint(((field_number as u64) << 3) | u64::from(wire_type), out);
}

fn proto_varint_len(mut value: usize) -> usize {
    let mut len = 1usize;
    while value >= 0x80 {
        value >>= 7;
        len += 1;
    }
    len
}

fn proto_bytes_field_encoded_len(field_number: u32, content_len: usize) -> usize {
    proto_varint_len(((field_number as usize) << 3) | 2)
        + proto_varint_len(content_len)
        + content_len
}

fn encode_proto_varint_field(field_number: u32, value: u64, out: &mut Vec<u8>) {
    encode_proto_key(field_number, 0, out);
    encode_proto_varint(value, out);
}

fn encode_proto_bytes_field(field_number: u32, value: &[u8], out: &mut Vec<u8>) {
    encode_proto_key(field_number, 2, out);
    encode_proto_varint(value.len() as u64, out);
    out.extend_from_slice(value);
}

fn derive_deterministic_signature_bytes(
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

fn signature_body_target_len(thinking: &str) -> usize {
    let thinking_len = thinking.len();
    thinking_len.clamp(THINKING_SIGNATURE_BODY_MIN_LEN, THINKING_SIGNATURE_BODY_MAX_LEN)
}

/// Build a deterministic protobuf envelope matching the field layout of recent
/// Claude Code signatures observed locally:
/// outer field-2 payload + outer field-3=1; inner fields 1/2/3/4/5; header
/// fields 1=12, 3=2, 5=64-byte body, 6=model string, 7=0.
///
/// Kiro exposes summarized thinking text but not Anthropic's encrypted
/// signature. This remains synthetic and is not a cryptographically valid
/// signature.
pub fn synthetic_thinking_signature(model: &str, thinking: &str) -> String {
    let mut header = Vec::new();
    encode_proto_varint_field(1, THINKING_SIGNATURE_HEADER_KIND, &mut header);
    encode_proto_varint_field(3, THINKING_SIGNATURE_HEADER_MODE, &mut header);
    let header_body = derive_deterministic_signature_bytes(
        model,
        thinking,
        b"header-body",
        THINKING_SIGNATURE_HEADER_BODY_LEN,
    );
    encode_proto_bytes_field(5, &header_body, &mut header);
    encode_proto_bytes_field(6, model.as_bytes(), &mut header);
    encode_proto_varint_field(7, 0, &mut header);

    let field_2 = derive_deterministic_signature_bytes(
        model,
        thinking,
        b"field-2",
        THINKING_SIGNATURE_HEADER_NONCE_LEN,
    );
    let field_3 = derive_deterministic_signature_bytes(
        model,
        thinking,
        b"field-3",
        THINKING_SIGNATURE_HEADER_NONCE_LEN,
    );
    let field_4 = derive_deterministic_signature_bytes(
        model,
        thinking,
        b"field-4",
        THINKING_SIGNATURE_HEADER_PROOF_LEN,
    );
    let body_len = signature_body_target_len(thinking);
    let field_5 = derive_deterministic_signature_bytes(model, thinking, b"field-5", body_len);
    let fixed_payload_len = proto_bytes_field_encoded_len(1, header.len())
        + proto_bytes_field_encoded_len(2, field_2.len())
        + proto_bytes_field_encoded_len(3, field_3.len())
        + proto_bytes_field_encoded_len(4, field_4.len())
        + proto_bytes_field_encoded_len(5, field_5.len());

    let mut payload = Vec::new();
    encode_proto_bytes_field(1, &header, &mut payload);
    encode_proto_bytes_field(2, &field_2, &mut payload);
    encode_proto_bytes_field(3, &field_3, &mut payload);
    encode_proto_bytes_field(4, &field_4, &mut payload);
    encode_proto_bytes_field(5, &field_5, &mut payload);
    debug_assert_eq!(payload.len(), fixed_payload_len);

    let mut envelope = Vec::new();
    encode_proto_bytes_field(2, &payload, &mut envelope);
    encode_proto_varint_field(3, 1, &mut envelope);

    STANDARD.encode(envelope)
}
